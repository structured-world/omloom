use super::*;
use api::network_observation::Value as Fact;

pub(super) const FIELD: &str = "network_evidence";
fn source_parent(target: AssetRef) -> String {
    format!(
        "{}/{}/network-sources",
        asset_key(target),
        target.incarnation
    )
}

pub(super) fn validate(value: &api::HostNetwork) -> Result<()> {
    use current_facts::{MAX_BYTES, MAX_FACTS};
    if value.sockets.len() + value.routes.len() + value.rules.len() > MAX_FACTS
        || value.encoded_len() > MAX_BYTES
        || value.observed_at == 0
        || value.observed_at > i64::MAX as u64
    {
        return Err(Error::Limit);
    }
    for (s, max) in [
        (&value.source, 100),
        (&value.basis, 128),
        (&value.socket_coverage, 512),
        (&value.route_coverage, 512),
        (&value.application_coverage, 512),
        (&value.packet_filter_coverage, 512),
    ] {
        if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
            return Err(Error::Invalid);
        }
    }
    for s in &value.sockets {
        if !matches!(
            api::AddressStability::try_from(s.address_stability),
            Ok(api::AddressStability::Unspecified | api::AddressStability::Stable)
        ) || api::AddressState::try_from(s.address_state).is_err()
            || !matches!(s.protocol.as_str(), "tcp" | "udp")
            || !matches!(s.family.as_str(), "IPv4" | "IPv6" | "IPv4+IPv6")
            || !(1..=65535).contains(&s.port)
            || s.bind.len() > 128
            || s.bind.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        parse_target(s.service.as_ref())?;
        if s.bind != "*" {
            if let Some((_, zone)) = s.bind.split_once('%')
                && (zone.is_empty() || zone.len() > 64 || zone.contains('%') || s.family != "IPv6")
            {
                return Err(Error::Invalid);
            }
            let ip: std::net::IpAddr = s
                .bind
                .split('%')
                .next()
                .ok_or(Error::Invalid)?
                .parse()
                .map_err(|_| Error::Invalid)?;
            if (ip.is_ipv4() && s.family != "IPv4") || (ip.is_ipv6() && s.family != "IPv6") {
                return Err(Error::Invalid);
            }
        }
    }
    for r in &value.routes {
        if !matches!(r.family.as_str(), "IPv4" | "IPv6") {
            return Err(Error::Invalid);
        }
        if r.destination.is_empty() {
            return Err(Error::Invalid);
        }
        for s in [&r.destination, &r.gateway, &r.flags, &r.interface] {
            if s.len() > 128 || s.chars().any(char::is_control) {
                return Err(Error::Invalid);
            }
        }
    }
    for r in &value.rules {
        if r.subject.is_empty()
            || r.subject.len() > 1024
            || r.subject.chars().any(char::is_control)
            || !matches!(r.action.as_str(), "allow incoming" | "block incoming")
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

pub(super) fn sections(value: &api::HostNetwork) -> Result<[BTreeMap<String, Properties>; 4]> {
    let mut sections: [BTreeMap<String, Properties>; 4] = Default::default();
    for s in &value.sockets {
        let identity = format!(
            "{}/{}/{}/{}/{}",
            asset_key(parse_target(s.service.as_ref())?),
            s.protocol,
            s.family,
            s.bind,
            s.port
        );
        let layer = socket_layer(parse_target(s.service.as_ref())?);
        let mut data = target_props(parse_target(value.installation.as_ref())?);
        data.insert("layer".into(), text(layer));
        data.insert("payload".into(), Value::Binary(s.encode_to_vec()));
        if sections[0]
            .insert(current_facts::digest(identity.as_bytes()), data)
            .is_some()
        {
            return Err(Error::Invalid);
        }
    }
    for r in &value.routes {
        // Native flags can distinguish concurrent interface-scoped/global routes:
        // https://github.com/apple-oss-distributions/network_cmds/blob/main/netstat.tproj/netstat.1
        let identity = r.encode_to_vec();
        if sections[1]
            .insert(
                current_facts::digest(&identity),
                props([
                    ("layer", text("network-route")),
                    ("payload", Value::Binary(identity)),
                ]),
            )
            .is_some()
        {
            return Err(Error::Invalid);
        }
    }
    for r in &value.rules {
        if sections[2]
            .insert(
                current_facts::digest(r.subject.as_bytes()),
                props([
                    ("layer", text("network-rule")),
                    ("payload", Value::Binary(r.encode_to_vec())),
                ]),
            )
            .is_some()
        {
            return Err(Error::Invalid);
        }
    }
    for (kind, enabled) in [
        (api::firewall_setting::Kind::Enabled, value.firewall_enabled),
        (api::firewall_setting::Kind::BlockAll, value.block_all),
        (
            api::firewall_setting::Kind::AllowBuiltinSigned,
            value.allow_builtin_signed,
        ),
        (
            api::firewall_setting::Kind::AllowDownloadedSigned,
            value.allow_downloaded_signed,
        ),
    ] {
        if let Some(enabled) = enabled {
            let policy = api::FirewallSetting {
                kind: kind as i32,
                enabled,
            };
            sections[3].insert(
                (kind as i32).to_string(),
                props([
                    ("layer", text("network-policy")),
                    ("payload", Value::Binary(policy.encode_to_vec())),
                ]),
            );
        }
    }
    Ok(sections)
}
pub(super) fn socket_layer(service: AssetRef) -> String {
    format!(
        "network-socket/{}/{}",
        asset_key(service),
        service.incarnation
    )
}

pub(super) fn source_digest(
    coverage: &api::HostNetwork,
    sections: &[BTreeMap<String, Properties>; 4],
) -> Result<Vec<u8>> {
    let mut hash = Sha256::new();
    hash.update(b"omloom/network-evidence/1");
    for section in sections {
        hash.update((section.len() as u32).to_le_bytes());
        for (id, data) in section {
            hash.update((id.len() as u32).to_le_bytes());
            hash.update(id.as_bytes());
            let payload = binary(data, "payload")?;
            hash.update((payload.len() as u32).to_le_bytes());
            hash.update(payload);
        }
    }
    hash.update(coverage.encode_to_vec());
    Ok(hash.finalize().to_vec())
}
pub(super) fn coverage(value: &api::HostNetwork) -> api::HostNetwork {
    api::HostNetwork {
        installation: value.installation.clone(),
        source: value.source.clone(),
        observed_at: value.observed_at,
        basis: value.basis.clone(),
        socket_coverage: value.socket_coverage.clone(),
        route_coverage: value.route_coverage.clone(),
        application_coverage: value.application_coverage.clone(),
        packet_filter_coverage: value.packet_filter_coverage.clone(),
        firewall_enabled: value.firewall_enabled,
        block_all: value.block_all,
        allow_builtin_signed: value.allow_builtin_signed,
        allow_downloaded_signed: value.allow_downloaded_signed,
        sockets_complete: value.sockets_complete,
        routes_complete: value.routes_complete,
        rules_complete: value.rules_complete,
        ..Default::default()
    }
}
const PARTS: [&str; 4] = ["sockets", "routes", "rules", "policies"];

struct NetworkSource {
    id: String,
    binding_unresolved: bool,
    coverage: Option<api::HostNetwork>,
    observations: Vec<api::NetworkObservation>,
    // None marks a known label already moved into the output page.
    services: BTreeMap<AssetRef, Option<api::OwnerContext>>,
}

impl Tx<'_> {
    pub(super) fn read_network_page(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
        working: &api::WorkingContext,
        cursor: Option<&str>,
        detail: &mut api::Detail,
    ) -> Result<()> {
        let revision = number(&self.root, "revision")?;
        let fingerprint = assets::query_fingerprint(
            &api::Query {
                roots: working.roots.clone(),
                focus: Some(target_wire(target)),
                search: format!("network/{:?}", working.focus),
                ..Default::default()
            },
            context,
        );
        let mut offset = 0usize;
        if let Some(cursor) = cursor {
            if cursor.len() > 256 {
                return Err(Error::Invalid);
            }
            let parts: Vec<_> = cursor.split(':').collect();
            if parts.len() != 4
                || parts[0].parse::<u64>().ok() != Some(revision)
                || parts[1] != fingerprint
                || parts[2].parse::<u64>().ok() != Some(context.authority_revision)
            {
                return Err(Error::Expired);
            }
            offset = parts[3].parse().map_err(|_| Error::Invalid)?;
            if offset > current_facts::MAX_FACTS {
                return Err(Error::Invalid);
            }
        }
        // Reserve the cursor and outer Reply envelope. Track wire sizes without
        // re-encoding growing pages or copying observation payloads.
        let cursor_bytes = 3 * 20 + 3 + fingerprint.len();
        let mut bytes =
            detail.encoded_len() + 1 + prost::length_delimiter_len(cursor_bytes) + cursor_bytes;
        let limit = MAX_REPLY_BYTES - 1 - prost::length_delimiter_len(MAX_REPLY_BYTES);
        if bytes > limit {
            return Err(Error::Limit);
        }
        // Reject stale cursors and oversized base details before reading facts.
        let mut sources = self.read_host_network(context, target, kind)?;
        debug_assert!(detail.host_network.is_empty());
        detail.host_network = sources
            .iter_mut()
            .map(|source| {
                let view = api::HostNetworkView {
                    source_id: source.id.clone(),
                    binding_unresolved: source.binding_unresolved,
                    coverage: source.coverage.take(),
                    ..Default::default()
                };
                let size = view.encoded_len();
                bytes += 1 + prost::length_delimiter_len(size) + size;
                view
            })
            .collect();
        if bytes > limit {
            return Err(Error::Limit);
        }
        let mut position = 0;
        let mut emitted = 0;
        let mut more = false;
        'sources: for (source, page) in sources.into_iter().zip(&mut detail.host_network) {
            let mut labels = source.services;
            let mut view_bytes = page.encoded_len();
            for observation in source.observations {
                position += 1;
                if position <= offset {
                    continue;
                }
                let mut absent = None;
                let label = if let Some(Fact::Socket(socket)) = &observation.value {
                    labels
                        .get_mut(&parse_target(socket.service.as_ref())?)
                        .ok_or(Error::Corrupt)?
                } else {
                    &mut absent
                };
                let size = observation.encoded_len();
                let mut addition = 1 + prost::length_delimiter_len(size) + size;
                if let Some(label) = label {
                    let size = label.encoded_len();
                    addition += 1 + prost::length_delimiter_len(size) + size;
                }
                let next_view = view_bytes + addition;
                let delta = addition + prost::length_delimiter_len(next_view)
                    - prost::length_delimiter_len(view_bytes);
                if bytes + delta > limit {
                    if emitted == 0 {
                        return Err(Error::Limit);
                    }
                    more = true;
                    break 'sources;
                }
                bytes += delta;
                view_bytes = next_view;
                page.observations.push(observation);
                if let Some(label) = label.take() {
                    page.services.push(label);
                }
                emitted += 1;
            }
        }
        if more {
            detail.network_next = Some(format!(
                "{revision}:{fingerprint}:{}:{}",
                context.authority_revision,
                offset + emitted
            ));
        }
        Ok(())
    }
    fn network_service_labels(
        &self,
        context: &Context,
        installation: AssetRef,
        rows: &[Properties],
    ) -> Result<BTreeMap<AssetRef, api::OwnerContext>> {
        if !context.field(
            installation,
            wire::AssetKind::SoftwareInstallation as i32,
            "relations",
            false,
        ) {
            return Ok(BTreeMap::new());
        }
        let mut keys = BTreeSet::new();
        for row in rows {
            let socket =
                api::SocketBind::decode(binary(row, "payload")?).map_err(|_| Error::Corrupt)?;
            keys.insert(asset_key(parse_target(socket.service.as_ref())?));
        }
        if keys.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows=self.query("UNWIND $keys AS key MATCH (s:OmRow {key:key})<-[x:OmRelationTo]-(r:OmRow)<-[y:OmRelationFrom]-(p:OmRow {key:$parent}) WHERE r.kind=$contains AND s.kind=$service AND s.adoption=$adopted RETURN DISTINCT properties(s) AS p LIMIT 2049",params([
            ("keys",Value::Array(keys.iter().map(text).collect())),("parent",text(asset_key(installation))),
            ("contains",Value::Int(wire::RelationKind::Contains as i64)),("service",Value::Int(wire::AssetKind::Service as i64)),("adopted",Value::Int(wire::Adoption::Adopted as i64)),
        ]))?;
        let mut labels = BTreeMap::new();
        for row in rows {
            let service = target(&row)?;
            if context.field(service, wire::AssetKind::Service as i32, FIELD, false)
                && context.field(service, wire::AssetKind::Service as i32, "relations", false)
                && context.field(service, wire::AssetKind::Service as i32, "name", false)
            {
                labels.insert(
                    service,
                    api::OwnerContext {
                        target: Some(target_wire(service)),
                        kind: wire::AssetKind::Service as i32,
                        name: match row.get("name") {
                            Some(Value::String(name)) => name.clone(),
                            None => String::new(),
                            _ => return Err(Error::Corrupt),
                        },
                    },
                );
            }
        }
        Ok(labels)
    }
    pub(super) fn accept_network(&mut self, value: &api::HostNetwork) -> Result<()> {
        self.accept_network_owned(value, None)
    }
    pub(super) fn accept_native_network(
        &mut self,
        value: &api::HostNetwork,
        binding: &str,
    ) -> Result<()> {
        self.accept_network_owned(value, Some(binding))
    }
    fn accept_network_owned(
        &mut self,
        value: &api::HostNetwork,
        binding: Option<&str>,
    ) -> Result<()> {
        let target = parse_target(value.installation.as_ref())?;
        let parent = source_parent(target);
        let key = source_layout::Family::Network.key(target, &value.source, binding);
        let sources = self.fact_rows(&parent, current_facts::MAX_SOURCES)?;
        let old = self.get(&key)?;
        if old.is_none() && sources.len() >= current_facts::MAX_SOURCES {
            return Err(Error::Limit);
        }
        let sections = sections(value)?;
        let mut coverage = coverage(value);
        let digest = source_digest(&coverage, &sections)?;
        if let Some(old) = &old {
            let observed = number(old, "observed")?;
            if observed > value.observed_at
                || (observed == value.observed_at && binary(old, "digest")? != digest)
            {
                return Err(Error::Conflict);
            }
        }
        coverage.observed_at = 0;
        coverage.firewall_enabled = None;
        coverage.block_all = None;
        coverage.allow_builtin_signed = None;
        coverage.allow_downloaded_signed = None;
        let body_key = format!("{key}/coverage");
        let body = self.get(&body_key)?;
        self.put_projection(
            &body_key,
            &key,
            Class::Inventory,
            props([("payload", Value::Binary(coverage.encode_to_vec()))]),
            body.as_ref(),
        )?;
        let complete = [
            value.sockets_complete,
            value.routes_complete,
            value.rules_complete,
            false,
        ];
        let mut count = 0usize;
        let mut bytes = 0usize;
        for (i, facts) in sections.into_iter().enumerate() {
            let (n, b) = self.replace_facts(
                &format!("{key}/{}", PARTS[i]),
                facts,
                complete[i],
                value.observed_at,
                &value.basis,
                current_facts::MAX_FACTS,
            )?;
            count += n;
            bytes += b;
        }
        let own_count = count;
        let own_bytes = bytes;
        for source in sources.iter().filter(|s| s.get("key") != Some(&text(&key))) {
            count += number(source, "count")? as usize;
            bytes += number(source, "bytes")? as usize;
        }
        if count > current_facts::MAX_FACTS || bytes > current_facts::MAX_BYTES {
            return Err(Error::Limit);
        }
        let mut metadata = props([
            ("layer", text("network-source")),
            ("source", text(&value.source)),
            ("observed", int(value.observed_at)?),
            ("count", int(own_count as u64)?),
            ("bytes", int(own_bytes as u64)?),
            ("digest", Value::Binary(digest)),
        ]);
        if let Some(binding) = binding {
            metadata.insert("binding".into(), text(binding));
        }
        self.put_projection(&key, &parent, Class::Inventory, metadata, old.as_ref())?;
        Ok(())
    }

    fn read_host_network(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
    ) -> Result<Vec<NetworkSource>> {
        let installation = if kind == wire::AssetKind::SoftwareInstallation as i32 {
            target
        } else if kind == wire::AssetKind::Service as i32 {
            let parents = self.service_installations(context, target)?;
            if parents.len() > 1 {
                return Err(Error::Conflict);
            }
            let Some(parent) = parents.first() else {
                return Ok(Vec::new());
            };
            *parent
        } else {
            return Ok(Vec::new());
        };
        if !context.field(
            installation,
            wire::AssetKind::SoftwareInstallation as i32,
            FIELD,
            false,
        ) || !context.field(target, kind, FIELD, false)
        {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        let mut sources =
            self.fact_rows(&source_parent(installation), current_facts::MAX_SOURCES)?;
        sources.sort_unstable_by(|a, b| {
            a.get("key")
                .and_then(Value::as_str)
                .cmp(&b.get("key").and_then(Value::as_str))
        });
        for source in sources {
            let key = string(&source, "key")?;
            let body = self
                .get(&format!("{key}/coverage"))?
                .ok_or(Error::Corrupt)?;
            let mut coverage =
                api::HostNetwork::decode(binary(&body, "payload")?).map_err(|_| Error::Corrupt)?;
            coverage.observed_at = number(&source, "observed")?;
            let mut observations = Vec::new();
            let mut services = BTreeMap::new();
            for (i, part) in PARTS.iter().enumerate() {
                if kind == wire::AssetKind::Service as i32 && i != 0 {
                    continue;
                }
                let mut rows = if kind == wire::AssetKind::Service as i32 {
                    self.query("MATCH (n:OmRow {layer:$layer}) WHERE n.parent=$parent RETURN properties(n) AS p LIMIT 2049",params([("layer",text(socket_layer(target))),("parent",text(format!("{key}/{part}")))]))?
                } else {
                    self.fact_rows(&format!("{key}/{part}"), current_facts::MAX_FACTS)?
                };
                if rows.len() > current_facts::MAX_FACTS {
                    return Err(Error::Limit);
                }
                rows.sort_unstable_by(|a, b| {
                    a.get("key")
                        .and_then(Value::as_str)
                        .cmp(&b.get("key").and_then(Value::as_str))
                });
                let windows = self.fact_windows(&rows)?;
                let labels = if i == 0 {
                    self.network_service_labels(context, installation, &rows)?
                } else {
                    BTreeMap::new()
                };
                for row in rows {
                    let payload = binary(&row, "payload")?;
                    let fact = match i {
                        0 => {
                            let socket =
                                api::SocketBind::decode(payload).map_err(|_| Error::Corrupt)?;
                            let service = parse_target(socket.service.as_ref())?;
                            if !socket_visible(&socket) {
                                continue;
                            }
                            let Some(label) = labels.get(&service) else {
                                continue;
                            };
                            services
                                .entry(service)
                                .or_insert_with(|| Some(label.clone()));
                            Fact::Socket(socket)
                        }
                        1 => {
                            let route =
                                api::LocalRoute::decode(payload).map_err(|_| Error::Corrupt)?;
                            if !route_visible(&route) {
                                continue;
                            }
                            Fact::Route(route)
                        }
                        2 => Fact::Rule(
                            api::ApplicationRule::decode(payload).map_err(|_| Error::Corrupt)?,
                        ),
                        _ => Fact::Policy(
                            api::FirewallSetting::decode(payload).map_err(|_| Error::Corrupt)?,
                        ),
                    };
                    let window = windows.get(&string(&row, "key")?).ok_or(Error::Corrupt)?;
                    observations.push(api::NetworkObservation {
                        observed_at: number(window, "observed")?,
                        basis: string(window, "scan")?,
                        value: Some(fact),
                    });
                }
            }
            if kind == wire::AssetKind::Service as i32 {
                coverage.route_coverage.clear();
                coverage.application_coverage.clear();
                coverage.packet_filter_coverage.clear();
            }
            result.push(NetworkSource {
                id: key,
                binding_unresolved: source.get("source_revision")
                    == Some(&text("unresolved-native-binding")),
                coverage: Some(coverage),
                observations,
                services,
            });
        }
        Ok(result)
    }
    pub(super) fn service_installations(
        &self,
        context: &Context,
        service: AssetRef,
    ) -> Result<Vec<AssetRef>> {
        let rows=self.query("MATCH (s:OmRow {key:$key})<-[x:OmRelationTo]-(r:OmRow)<-[y:OmRelationFrom]-(p:OmRow) WHERE r.kind=$kind AND p.kind=$installation RETURN properties(p) AS p LIMIT 9",params([("key",text(asset_key(service))),("kind",Value::Int(wire::RelationKind::Contains as i64)),("installation",Value::Int(wire::AssetKind::SoftwareInstallation as i64))]))?;
        if rows.len() > 8 {
            return Err(Error::Limit);
        }
        rows.into_iter()
            .filter_map(|r| match target(&r) {
                Ok(t)
                    if context.field(
                        t,
                        wire::AssetKind::SoftwareInstallation as i32,
                        "relations",
                        false,
                    ) =>
                {
                    Some(Ok(t))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }
}

pub(super) fn socket_visible(socket: &api::SocketBind) -> bool {
    socket
        .bind
        .split('%')
        .next()
        .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        .is_none_or(|ip| ip.is_unspecified() || !projection::local_address(ip))
}
fn route_visible(route: &api::LocalRoute) -> bool {
    [&route.destination, &route.gateway].into_iter().all(|s| {
        s.split('/')
            .next()
            .and_then(|s| s.split('%').next())
            .and_then(|s| s.parse::<std::net::IpAddr>().ok())
            .is_none_or(|ip| ip.is_unspecified() || !projection::local_address(ip))
    })
}
impl Store {
    pub(super) fn workspace_host_network(
        &self,
        context: &Context,
        value: api::HostNetwork,
        working: &api::WorkingContext,
        _now: u64,
    ) -> Result<api::Reply> {
        validate(&value)?;
        let target = parse_target(value.installation.as_ref())?;
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        tx.selection(context, working, true)?
            .contains(target, &tx)?;
        if number(&tx.visible_asset(context, target)?, "kind")?
            != wire::AssetKind::SoftwareInstallation as u64
        {
            return Err(Error::Invalid);
        }
        if !context.field(
            target,
            wire::AssetKind::SoftwareInstallation as i32,
            FIELD,
            true,
        ) {
            return Err(Error::Denied);
        }
        for socket in &value.sockets {
            let service = parse_target(socket.service.as_ref())?;
            if number(&tx.visible_asset(context, service)?, "kind")?
                != wire::AssetKind::Service as u64
                || !tx.direct_child(context, target, service)?
                || !context.field(service, wire::AssetKind::Service as i32, FIELD, true)
            {
                return Err(Error::Denied);
            }
        }
        if value.sockets_complete {
            let source = current_facts::source_key(&source_parent(target), &value.source);
            for row in tx.fact_rows(&format!("{source}/sockets"), current_facts::MAX_FACTS)? {
                let socket = api::SocketBind::decode(binary(&row, "payload")?)
                    .map_err(|_| Error::Corrupt)?;
                let service = parse_target(socket.service.as_ref())?;
                if !context.field(service, wire::AssetKind::Service as i32, FIELD, true) {
                    return Err(Error::Denied);
                }
            }
        }
        tx.accept_network(&value)?;
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
}

pub(super) fn migrate_network_row(tx: &mut Tx<'_>, row: &Properties) -> Result<()> {
    let mut value =
        api::HostNetwork::decode(binary(row, "payload")?).map_err(|_| Error::Corrupt)?;
    let target = parse_target(value.installation.as_ref())?;
    let key = format!("{}/host-network", asset_key(target));
    if value.observed_at == 0 {
        value.observed_at = number(&tx.get(&key)?.ok_or(Error::Corrupt)?, "observed")?;
    }
    tx.accept_network(&value)?;
    // Consume the selected legacy row even when its key differs from the payload owner.
    tx.delete_projection(&string(row, "key")?)?;
    tx.delete_projection(&key)?;
    tx.delete_projection(&format!("{key}-body"))?;
    Ok(())
}

pub(in crate::disk) fn upgrade(store: &Store) -> Result<()> {
    loop {
        let mut tx = store.begin()?;
        let old=tx.query("MATCH (n:OmRow) WHERE n.layer='host-network' OR (n.payload IS NOT NULL AND n.key ENDS WITH '/host-network') RETURN properties(n) AS p LIMIT 1",None)?;
        if let Some(row) = old.first() {
            migrate_network_row(&mut tx, row)?;
            tx.commit()?;
            continue;
        }
        let old=tx.query("MATCH (n:OmRow) WHERE n.key ENDS WITH '/current-addresses' RETURN properties(n) AS p LIMIT 1",None)?;
        if let Some(meta) = old.first() {
            let parent = string(meta, "parent")?;
            let owner = tx.get(&parent)?.ok_or(Error::Corrupt)?;
            let rows=tx.query("MATCH (n:OmRow {parent:$parent}) WHERE n.layer='interface-address' RETURN properties(n) AS p LIMIT 65",params([("parent",text(&parent))]))?;
            if rows.len() > 64 {
                return Err(Error::Limit);
            }
            let value = api::InterfaceAddresses {
                interface: Some(target_wire(target(&owner)?)),
                source: string(meta, "vtext")?,
                observed_at: number(meta, "observed")?,
                complete: flag(meta, "vflag")?,
                addresses: rows
                    .iter()
                    .map(|r| {
                        api::CurrentAddress::decode(binary(r, "payload")?)
                            .map_err(|_| Error::Corrupt)
                    })
                    .collect::<Result<_>>()?,
            };
            tx.accept_addresses(&value)?;
            if !value.complete {
                // The old shared checkpoint cannot establish omitted facts' exact dates.
                let source = interface_facts::source_key(target(&owner)?, &value.source);
                for fact in tx.fact_rows(&format!("{source}/values"), 64)? {
                    let key = string(&fact, "key")?;
                    let fresh = format!("{key}/freshness");
                    let prior = tx.get(&fresh)?;
                    tx.put_projection(
                        &fresh,
                        &key,
                        Class::Inventory,
                        props([("scan", text("legacy-partial"))]),
                        prior.as_ref(),
                    )?;
                }
            }
            for row in rows {
                tx.delete_projection(&string(&row, "key")?)?;
            }
            tx.delete_projection(&string(meta, "key")?)?;
            tx.commit()?;
            continue;
        }
        tx.commit()?;
        return Ok(());
    }
}
