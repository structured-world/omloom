//! Reviewed native input reuses current inventory and observation rows. Only
//! selected proposals survive in bounded, expiring review custody until acceptance.
use super::*;
#[cfg(test)]
mod tests;

fn mapping(actor: &str, source: &str, basis: &str, key: &str) -> String {
    // Source names select independent import configurations, not cosmetic
    // labels. Equal unauthenticated source evidence cannot merge configurations.
    let mut hash = Sha256::new();
    for part in [actor, source, basis, key] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("host-mapping/{:x}", hash.finalize())
}
fn validate(report: &api::HostReport) -> Result<()> {
    if report.version != 1
        || !matches!(report.platform.as_str(), "macos" | "linux" | "windows")
        || report.findings.is_empty()
        || report.findings.len() > MAX_IMPORT
        || report.binds.len() > 256
        || report.coverage.len() > 32
        || report.encoded_len() > MAX_CSV
        || report.observed_at == 0
        || report.observed_at > i64::MAX as u64
    {
        return Err(Error::Limit);
    }
    for basis in [&report.source_basis, &report.boot_basis] {
        if basis.len() != 64 || !basis.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err(Error::Invalid);
        }
    }
    let mut keys = BTreeMap::new();
    for node in &report.findings {
        super::host_identity::validate(node)?;
        if node.key.is_empty()
            || node.key.len() > 256
            || node.key.chars().any(char::is_control)
            || node.fields.is_empty()
            || node.fields.len() > MAX_FIELDS
            || node.addresses.len() > 64
        {
            return Err(Error::Invalid);
        }
        if !matches!(
            wire::AssetKind::try_from(node.kind),
            Ok(wire::AssetKind::Unspecified
                | wire::AssetKind::Device
                | wire::AssetKind::VirtualMachine
                | wire::AssetKind::SoftwareInstallation
                | wire::AssetKind::Port
                | wire::AssetKind::Interface
                | wire::AssetKind::Service
                | wire::AssetKind::Workload
                | wire::AssetKind::Container)
        ) {
            return Err(Error::Invalid);
        }
        if node.kind == wire::AssetKind::Unspecified as i32
            && (node.key != "hardware" || node.parent.is_some() || node.runtime.is_some())
        {
            return Err(Error::Invalid);
        }
        // These targets are authorized as installations during import. Bind
        // their types before any target resolution or mutation can use them.
        if node.key == "installation" && node.kind != wire::AssetKind::SoftwareInstallation as i32 {
            return Err(Error::Invalid);
        }
        for parent in [&node.parent, &node.runtime].into_iter().flatten() {
            if !keys.contains_key(parent) {
                return Err(Error::Invalid);
            }
        }
        if let Some(runtime) = &node.runtime
            && keys.get(runtime) != Some(&(wire::AssetKind::SoftwareInstallation as i32))
        {
            return Err(Error::Invalid);
        }
        if keys.insert(node.key.clone(), node.kind).is_some() {
            return Err(Error::Invalid);
        }
        if !node.addresses.is_empty() && node.kind != wire::AssetKind::Interface as i32 {
            return Err(Error::Invalid);
        }
        for value in &node.addresses {
            interface_facts::validate_address(value)?;
        }
    }
    for item in &report.coverage {
        if item.module.is_empty()
            || item.module.len() > 64
            || item.detail.len() > 512
            || !matches!(
                item.status.as_str(),
                "verified" | "partial" | "failed" | "blocked" | "unavailable"
            )
            || item.detail.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
    }
    for bind in &report.binds {
        if (!bind.owner.is_empty() && !keys.contains_key(&bind.owner))
            || bind.value.as_ref().is_none_or(|v| v.service.is_some())
        {
            return Err(Error::Invalid);
        }
        if !bind.owner.is_empty()
            && !report
                .findings
                .iter()
                .any(|v| v.key == bind.owner && v.kind == wire::AssetKind::Service as i32)
        {
            return Err(Error::Invalid);
        }
        let value = bind.value.as_ref().ok_or(Error::Invalid)?;
        if value.address_stability == api::AddressStability::Temporary as i32
            || !(1..=65535).contains(&value.port)
            || !matches!(value.protocol.as_str(), "tcp" | "udp")
            || !matches!(value.family.as_str(), "IPv4" | "IPv6" | "IPv4+IPv6")
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

impl Tx<'_> {
    fn apply_host(
        &mut self,
        context: &Context,
        request: &api::HostImport,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<Vec<api::ImportRow>> {
        let report = request.report.as_deref().ok_or(Error::Invalid)?;
        validate(report)?;
        if request.selection.is_empty()
            || request.selection.len() > MAX_IMPORT
            || request.source.is_empty()
            || request.source.len() > 80
            || request.source.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        self.selection(context, working, true)?;
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        let mut selected = BTreeMap::new();
        for value in &request.selection {
            if !report.findings.iter().any(|f| f.key == value.key)
                || selected.insert(value.key.as_str(), value).is_some()
            {
                return Err(Error::Invalid);
            }
        }
        let mut resolved = BTreeMap::<&str, AssetRef>::new();
        let mut rows = Vec::with_capacity(request.selection.len());
        for node in &report.findings {
            let Some(explicit) = selected.get(node.key.as_str()) else {
                continue;
            };
            let key = mapping(
                &context.actor,
                &request.source,
                &report.source_basis,
                &node.key,
            );
            let old = self.get(&key)?;
            let mut hash = Sha256::new();
            for field in &node.fields {
                hash.update((field.encoded_len() as u64).to_be_bytes());
                hash.update(field.encode_to_vec());
            }
            let digest = hash.finalize().to_vec();
            let mut accepted_fields = false;
            if let Some(old) = &old {
                let observed = number(old, "observed")?;
                accepted_fields = binary(old, "digest")? == digest;
                if observed > report.observed_at
                    || (observed == report.observed_at
                        && (!accepted_fields || string(old, "vtext")? != report.boot_basis))
                {
                    return Err(Error::Conflict);
                }
            }
            let prior = old.as_ref().map(super::super::target).transpose()?;
            let projection_source = old
                .as_ref()
                .and_then(|row| row.get("namespace"))
                .and_then(Value::as_str)
                .unwrap_or(&request.source)
                .to_owned();
            let declared_kind = explicit.kind;
            if declared_kind.is_some()
                && (node.kind != wire::AssetKind::Unspecified as i32 || explicit.target.is_some())
            {
                return Err(Error::Invalid);
            }
            let explicit = explicit
                .target
                .as_ref()
                .map(|v| parse_target(Some(v)))
                .transpose()?;
            if explicit.is_some() && prior.is_some() && explicit != prior {
                return Err(Error::Conflict);
            }
            let existing = explicit.or(prior);
            let mut kind = node.kind;
            let expected = if let Some(target) = existing {
                let row = self.visible_asset(context, target)?;
                let actual_kind = number(&row, "kind")? as i32;
                if kind == wire::AssetKind::Unspecified as i32 {
                    if !matches!(
                        wire::AssetKind::try_from(actual_kind),
                        Ok(wire::AssetKind::Device | wire::AssetKind::VirtualMachine)
                    ) || declared_kind.is_some_and(|v| v != actual_kind)
                    {
                        return Err(Error::Conflict);
                    }
                    kind = actual_kind;
                } else if actual_kind != kind {
                    return Err(Error::Invalid);
                }
                self.selection(context, working, false)?
                    .contains(target, self)?;
                Some(number(&row, "revision")?)
            } else {
                if kind == wire::AssetKind::Unspecified as i32 {
                    kind = declared_kind.ok_or(Error::HostClassificationRequired)?;
                    if !matches!(
                        wire::AssetKind::try_from(kind),
                        Ok(wire::AssetKind::Device | wire::AssetKind::VirtualMachine)
                    ) {
                        return Err(Error::Invalid);
                    }
                }
                None
            };
            let parent = node
                .parent
                .as_deref()
                .map(|key| resolved.get(key).copied().ok_or(Error::Invalid))
                .transpose()?;
            let local = if existing.is_none() && parent.is_some() {
                api::WorkingContext {
                    roots: working.roots.clone(),
                    focus: parent.map(target_wire),
                    expected: Some(number(&self.root, "revision")?),
                }
            } else {
                working.clone()
            };
            let mut change = api::Write {
                target: existing.map(target_wire),
                expected,
                kind,
                changes: node.fields.clone(),
                parent: if existing.is_none() {
                    parent.map(target_wire)
                } else {
                    None
                },
            };
            let keep_fields = accepted_fields
                || (node.kind == wire::AssetKind::Unspecified as i32 && existing.is_some());
            let target = if keep_fields {
                change.changes.clear();
                existing.ok_or(Error::Corrupt)?
            } else {
                self.context_write(context, change.clone(), &local, now)?
            };
            // An existing match must retain the reviewed local ownership, not
            // silently move across machines when a source key is reused.
            if let Some(parent) = parent.filter(|_| existing.is_some())
                && !self.direct_child(context, parent, target)?
            {
                return Err(Error::Conflict);
            }
            if let Some(runtime) = node.runtime.as_deref() {
                let runtime = *resolved.get(runtime).ok_or(Error::Invalid)?;
                let relations = self.working_relations(target)?;
                let prior = relations.iter().find(|r| {
                    r.kind == wire::RelationKind::RunsOn as i32
                        && r.from.as_ref() == Some(&target_wire(target))
                });
                if let Some(prior) = prior {
                    if prior.to.as_ref() != Some(&target_wire(runtime)) {
                        return Err(Error::Conflict);
                    }
                } else {
                    if !context.field(target, node.kind, "relations", true)
                        || !context.field(
                            runtime,
                            wire::AssetKind::SoftwareInstallation as i32,
                            "relations",
                            true,
                        )
                    {
                        return Err(Error::Denied);
                    }
                    let authority_id = self.workspace_authority()?;
                    self.save_typed_relation(
                        &wire::Relation {
                            relation_id: RelationId::generate().to_string(),
                            from: Some(target_wire(target)),
                            to: Some(target_wire(runtime)),
                            kind: wire::RelationKind::RunsOn as i32,
                            revision: 1,
                            authority_id,
                        },
                        None,
                        &context.actor,
                        now,
                    )?;
                }
            }
            let mut addresses = None;
            if node.kind == wire::AssetKind::Interface as i32 {
                if !context.field(target, node.kind, "current_addresses", true) {
                    return Err(Error::Denied);
                }
                addresses = self.accept_native_addresses(
                    context,
                    &api::InterfaceAddresses {
                        interface: Some(target_wire(target)),
                        source: projection_source.clone(),
                        observed_at: report.observed_at,
                        addresses: node.addresses.clone(),
                        complete: node.addresses_complete,
                    },
                    &key,
                )?;
            }
            self.accept_host_identity(
                context,
                target,
                kind,
                super::host_identity::Source {
                    mapping: &key,
                    name: &projection_source,
                    observed: report.observed_at,
                    boot: &report.boot_basis,
                },
                &node.identity,
            )?;
            let mut data = target_props(target);
            data.insert("source".into(), text(&request.source));
            if projection_source != request.source {
                data.insert("namespace".into(), text(&projection_source));
            }
            // One current checkpoint fences stale replay and same-instant
            // equivocation; it does not retain another report or field history.
            data.insert("observed".into(), int(report.observed_at)?);
            data.insert("digest".into(), Value::Binary(digest));
            data.insert("vtext".into(), text(&report.boot_basis));
            self.put_projection(
                &key,
                &format!("{}/host-mappings", asset_key(target)),
                Class::Inventory,
                data,
                old.as_ref(),
            )?;
            resolved.insert(&node.key, target);
            rows.push(api::ImportRow {
                row: rows.len() as u32 + 1,
                change: Some(change),
                addresses,
                diagnostic: if keep_fields {
                    "Keep previously accepted identity and local fields"
                } else if existing.is_some() {
                    "Update reviewed existing identity"
                } else {
                    "Adopt a new identity"
                }
                .into(),
            });
        }
        if let Some(installation) = resolved.get("installation")
            && let Some(source) = &report.network
        {
            let metadata = self
                .get(&mapping(
                    &context.actor,
                    &request.source,
                    &report.source_basis,
                    "installation",
                ))?
                .ok_or(Error::Corrupt)?;
            let projection_source = metadata
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or(&request.source);
            let mut network = source.clone();
            network.installation = Some(target_wire(*installation));
            network.observed_at = report.observed_at;
            network.basis = report.boot_basis.clone();
            network.sockets.clear();
            for bind in &report.binds {
                let Some(service) = resolved.get(bind.owner.as_str()) else {
                    network.sockets_complete = false;
                    continue;
                };
                let mut value = bind.value.clone().ok_or(Error::Invalid)?;
                if !self.direct_child(context, *installation, *service)? {
                    return Err(Error::Denied);
                }
                value.service = Some(target_wire(*service));
                network.sockets.push(value);
            }
            if !context.field(
                *installation,
                wire::AssetKind::SoftwareInstallation as i32,
                "network_evidence",
                true,
            ) {
                return Err(Error::Denied);
            }
            for socket in &network.sockets {
                let target = parse_target(socket.service.as_ref())?;
                if !context.field(
                    target,
                    wire::AssetKind::Service as i32,
                    "network_evidence",
                    true,
                ) {
                    return Err(Error::Denied);
                }
            }
            // Partial selection cannot remove a hidden/unselected service's facts.
            network.sockets_complete = false;
            // Independent projection sources let a user select another section
            // of the same file without inventing a new observation timestamp.
            for part in &request.network_parts {
                let mut selected = network.clone();
                let part = api::HostNetworkPart::try_from(*part).map_err(|_| Error::Invalid)?;
                let suffix = match part {
                    api::HostNetworkPart::Sockets => "sockets",
                    api::HostNetworkPart::Routes => "routes",
                    api::HostNetworkPart::ApplicationFirewall => "application-fw",
                    _ => return Err(Error::Invalid),
                };
                selected.source = format!("{projection_source} / {suffix}");
                if part != api::HostNetworkPart::Sockets {
                    selected.sockets.clear();
                    selected.sockets_complete = false;
                    selected.socket_coverage = "Not selected for this projection source".into();
                }
                if part != api::HostNetworkPart::Routes {
                    selected.routes.clear();
                    selected.routes_complete = false;
                    selected.route_coverage = "Not selected for this projection source".into();
                }
                if part != api::HostNetworkPart::ApplicationFirewall {
                    selected.rules.clear();
                    selected.rules_complete = false;
                    selected.firewall_enabled = None;
                    selected.block_all = None;
                    selected.allow_builtin_signed = None;
                    selected.allow_downloaded_signed = None;
                    selected.application_coverage =
                        "Not selected for this projection source".into();
                    selected.packet_filter_coverage =
                        "Not selected for this projection source".into();
                }
                network_facts::validate(&selected)?;
                let base = mapping(
                    &context.actor,
                    &request.source,
                    &report.source_basis,
                    "installation",
                );
                self.accept_native_network(&selected, &format!("{base}/{suffix}"), &base)?;
            }
        }
        Ok(rows)
    }
}
impl Store {
    pub(super) fn workspace_host_import(
        &self,
        context: &Context,
        mut request: api::HostImport,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if let Some(id) = request.preview {
            id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
            if request.report.is_some()
                || !request.selection.is_empty()
                || !request.source.is_empty()
                || !request.network_parts.is_empty()
            {
                return Err(Error::Invalid);
            }
            let mut tx = self.begin()?;
            let key = format!("preview/{id}");
            let row = tx.get(&key)?.ok_or(Error::Expired)?;
            if string(&row, "owner")? != context.actor {
                return Err(Error::Denied);
            }
            if number(&row, "expires")? <= now {
                return Err(Error::Expired);
            }
            if number(&row, "revision")? != number(&tx.root, "revision")? {
                return Err(Error::Conflict);
            }
            let mut selector = working.clone();
            selector.expected = None;
            if binary(&row, "working")? != selector.encode_to_vec() {
                return Err(Error::Conflict);
            }
            let request =
                api::HostImport::decode(binary(&row, "payload")?).map_err(|_| Error::Corrupt)?;
            let rows = tx.apply_host(context, &request, working, now)?;
            let mut targets = Vec::with_capacity(rows.len());
            let report = request.report.as_deref().ok_or(Error::Corrupt)?;
            for selected in &request.selection {
                let row = tx
                    .get(&mapping(
                        &context.actor,
                        &request.source,
                        &report.source_basis,
                        &selected.key,
                    ))?
                    .ok_or(Error::Corrupt)?;
                targets.push(target_wire(super::super::target(&row)?));
            }
            tx.delete(&key)?;
            return Ok(mutation(tx.commit()?, targets));
        }
        let report = request.report.as_mut().ok_or(Error::Invalid)?;
        validate(report)?;
        let parts: BTreeSet<_> = request.network_parts.iter().copied().collect();
        if parts.len() != request.network_parts.len()
            || parts.iter().any(|v| {
                !matches!(
                    api::HostNetworkPart::try_from(*v),
                    Ok(api::HostNetworkPart::Sockets
                        | api::HostNetworkPart::Routes
                        | api::HostNetworkPart::ApplicationFirewall)
                )
            })
        {
            return Err(Error::Invalid);
        }
        if parts.is_empty() {
            report.network = None;
        } else {
            if !request.selection.iter().any(|v| v.key == "installation") {
                return Err(Error::Invalid);
            }
            let network = report.network.as_mut().ok_or(Error::Invalid)?;
            if !parts.contains(&(api::HostNetworkPart::Routes as i32)) {
                network.routes.clear();
                network.routes_complete = false;
            }
            if !parts.contains(&(api::HostNetworkPart::ApplicationFirewall as i32)) {
                network.rules.clear();
                network.rules_complete = false;
                network.firewall_enabled = None;
                network.block_all = None;
                network.allow_builtin_signed = None;
                network.allow_downloaded_signed = None;
            }
        }
        if !parts.contains(&(api::HostNetworkPart::Sockets as i32)) {
            report.binds.clear();
        }
        // Only selected proposals enter review custody. The original file and
        // unselected findings are never stored as a second inventory.
        let keys: BTreeSet<_> = request.selection.iter().map(|v| v.key.as_str()).collect();
        report.findings.retain(|v| keys.contains(v.key.as_str()));
        report.binds.retain(|v| keys.contains(v.owner.as_str()));
        let tx = self.begin()?;
        tx.selection(context, working, true)?;
        let base = number(&tx.root, "revision")?;
        drop(tx);
        let (rows, changed) = {
            // CoordiNode interactive transactions buffer until commit. Running
            // the real mutation checks here and rolling back validates the whole
            // ancestry atomically without journal writes or a second validator.
            let mut dry = self.begin()?;
            let rows = dry.apply_host(context, &request, working, now)?;
            (rows, dry.changed)
        };
        if !changed {
            return Ok(mutation(base, Vec::new()));
        }
        let bytes = request.encode_to_vec();
        if bytes.len() > MAX_CSV {
            return Err(Error::Limit);
        }
        let mut tx = self.begin()?;
        if number(&tx.root, "revision")? != base {
            return Err(Error::Conflict);
        }
        let old=tx.query("MATCH (n:OmRow {parent:'workspace-previews'}) WHERE n.owner=$owner RETURN properties(n) AS p LIMIT 5",params([("owner",text(&context.actor))]))?;
        if old.len() > 4 {
            return Err(Error::Limit);
        }
        let mut selector = working.clone();
        selector.expected = None;
        let fingerprint = current_facts::digest(&bytes);
        for row in old {
            if row.get("digest") == Some(&Value::Binary(fingerprint.as_bytes().to_vec()))
                && binary(&row, "working")? == selector.encode_to_vec()
                && number(&row, "revision")? == base
                && number(&row, "expires")? > now
            {
                // Reuse custody, not an old disclosure decision. The dry run
                // above derived these rows under the caller's current rights.
                let key = string(&row, "key")?;
                let preview = api::Preview {
                    id: Some(key.strip_prefix("preview/").ok_or(Error::Corrupt)?.into()),
                    revision: base,
                    rows,
                };
                return Ok(reply(api::reply::Result::Preview(preview)));
            }
            let key = string(&row, "key")?;
            for child in tx.children(&key)? {
                tx.delete(&string(&child, "key")?)?;
            }
            tx.delete(&key)?;
        }
        let id = AuthorityId::generate().to_string();
        let revision = base.checked_add(1).ok_or(Error::Limit)?;
        let preview = api::Preview {
            id: Some(id.clone()),
            revision,
            rows,
        };
        tx.put(
            &format!("preview/{id}"),
            "workspace-previews",
            Class::Review,
            props([
                ("owner", text(&context.actor)),
                (
                    "expires",
                    int(now
                        .checked_add(self.policy.review_age_ms)
                        .ok_or(Error::Limit)?)?,
                ),
                ("revision", int(revision)?),
                ("working", Value::Binary(selector.encode_to_vec())),
                ("digest", Value::Binary(fingerprint.into_bytes())),
                ("payload", Value::Binary(bytes)),
            ]),
        )?;
        tx.commit()?;
        Ok(reply(api::reply::Result::Preview(preview)))
    }
}
