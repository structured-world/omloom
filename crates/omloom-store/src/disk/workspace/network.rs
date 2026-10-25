use super::*;
use ipnet::IpNet;
use std::net::IpAddr;

fn identity(target: AssetRef) -> String {
    format!("{}/{}", target.asset, target.incarnation)
}
fn link_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V6(ip) => (ip.segments()[0] & 0xffc0) == 0xfe80,
        IpAddr::V4(ip) => ip.is_link_local(),
    }
}
fn allocation(
    space: AssetRef,
    network: IpNet,
    kind: wire::AssetKind,
    link: Option<AssetRef>,
) -> Result<String> {
    let address = if kind == wire::AssetKind::IpAddress {
        network.addr().to_string()
    } else {
        network.trunc().to_string()
    };
    let scoped = if link_local(network.addr()) {
        format!("/{}/", identity(link.ok_or(Error::Invalid)?))
    } else {
        "/".into()
    };
    Ok(format!(
        "allocation/{}/{}{scoped}{address}",
        identity(space),
        kind as i32
    ))
}
fn same_target(row: &Properties, target: AssetRef) -> bool {
    super::super::target(row).ok() == Some(target)
}
fn next_address(address: IpAddr) -> Result<IpAddr> {
    match address {
        IpAddr::V4(ip) => u32::from(ip).checked_add(1).map(|n| IpAddr::V4(n.into())),
        IpAddr::V6(ip) => u128::from(ip).checked_add(1).map(|n| IpAddr::V6(n.into())),
    }
    .ok_or(Error::Limit)
}
fn first_address(prefix: IpNet) -> Result<IpAddr> {
    match prefix {
        // RFC 3021 section 2: both /31 addresses are usable on a point-to-point link.
        IpNet::V4(n) if n.prefix_len() < 31 => next_address(n.network().into()),
        // RFC 6164 section 5: /127 point-to-point links have two usable endpoints.
        _ => Ok(prefix.network()),
    }
}
fn usable(prefix: IpNet, address: IpAddr) -> bool {
    if !prefix.contains(&address) {
        return false;
    }
    !matches!(prefix, IpNet::V4(n) if n.prefix_len() < 31 && address == IpAddr::V4(n.broadcast()))
}
pub(super) fn projection(row: &Properties, columns: &[String]) -> Result<Vec<wire::Field>> {
    let mut fields = Vec::new();
    for name in columns {
        let value = match row.get(name) {
            Some(Value::String(s)) => Some(field::Value::Text(s.clone())),
            Some(Value::Int(n)) => Some(field::Value::Integer(*n)),
            None => None,
            _ => return Err(Error::Corrupt),
        };
        if let Some(value) = value {
            fields.push(wire::Field {
                name: name.clone(),
                value: Some(value),
            });
        }
    }
    Ok(fields)
}

impl Store {
    fn workspace_vlan(
        &self,
        context: &Context,
        request: api::Network,
        now: u64,
    ) -> Result<api::Reply> {
        if request.address_space.is_some()
            || request.routing.is_some()
            || !request.cidr.is_empty()
            || request.vrid.is_some()
            || request.next_available
            || request.sharing != api::SharedUse::Exclusive as i32
        {
            return Err(Error::Invalid);
        }
        let link = parse_target(request.link.as_ref())?;
        let Some(vid @ 1..=4094) = request.vlan_id else {
            return Err(Error::Invalid);
        };
        let mut tx = self.begin()?;
        if number(&tx.visible_asset(context, link)?, "kind")? != wire::AssetKind::L2Domain as u64 {
            return Err(Error::Invalid);
        }
        let target = tx.workspace_write(
            context,
            api::Write {
                target: request.target,
                expected: request.expected,
                kind: request.kind,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(request.name)),
                }],
            },
            now,
        )?;
        if !context.field(target, request.kind, "network", true) {
            return Err(Error::Denied);
        }
        let key = format!("network/{}", identity(target));
        let old = tx.get(&key)?;
        let owner_key = format!("vlan/{}/{vid}", identity(link));
        if let Some(owner) = tx.get(&owner_key)? {
            if !same_target(&owner, target) {
                return Err(Error::Conflict);
            }
        }
        if let Some(old) = &old {
            if old.get("namespace") != Some(&text(&owner_key)) {
                tx.delete(&string(old, "namespace")?)?;
            }
        }
        let mut data = target_props(target);
        data.extend(props([
            ("link_scope", text(identity(link))),
            ("vlan_id", int(u64::from(vid))?),
            ("namespace", text(&owner_key)),
        ]));
        tx.put(
            &owner_key,
            "allocations",
            Class::Inventory,
            target_props(target),
        )?;
        if tx.put_against(&key, "networks", Class::Inventory, data, old.as_ref())? {
            tx.workspace_advance(target, &context.actor, "vlan.accepted", now)?;
        }
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
    pub(super) fn workspace_network(
        &self,
        context: &Context,
        mut request: api::Network,
        now: u64,
    ) -> Result<api::Reply> {
        use wire::AssetKind as K;
        let kind = K::try_from(request.kind).map_err(|_| Error::Invalid)?;
        if kind == K::Vlan {
            return self.workspace_vlan(context, request, now);
        }
        if !matches!(kind, K::IpAddress | K::Prefix) {
            return Err(Error::Invalid);
        }
        if request.vlan_id.is_some() {
            return Err(Error::Invalid);
        }
        let space = parse_target(request.address_space.as_ref())?;
        let link = request
            .link
            .as_ref()
            .map(|r| parse_target(Some(r)))
            .transpose()?;
        let routing = request
            .routing
            .as_ref()
            .map(|r| parse_target(Some(r)))
            .transpose()?;
        let sharing = api::SharedUse::try_from(request.sharing).map_err(|_| Error::Invalid)?;
        if sharing == api::SharedUse::Unspecified {
            return Err(Error::Invalid);
        }
        let mut network: IpNet = request.cidr.parse().map_err(|_| Error::Invalid)?;
        if request.cidr.len() > 64 {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        for (target, expected_kind) in [
            (Some(space), K::AddressSpace),
            (link, K::L2Domain),
            (routing, K::Vrf),
        ] {
            if let Some(target) = target {
                let row = tx.visible_asset(context, target)?;
                if number(&row, "kind")? != expected_kind as u64 {
                    return Err(Error::Invalid);
                }
            }
        }
        if request.next_available {
            if request.target.is_some() || kind != K::IpAddress {
                return Err(Error::Invalid);
            }
            let prefix = network.trunc();
            let mut address = first_address(prefix)?;
            // Probe only the occupied consecutive run. No IPv6 host enumeration.
            let mut probes = 0;
            while usable(prefix, address) {
                network = IpNet::new(address, prefix.prefix_len()).map_err(|_| Error::Invalid)?;
                if tx.get(&allocation(space, network, kind, link)?)?.is_none() {
                    break;
                }
                probes += 1;
                if probes > 10_000 {
                    return Err(Error::Limit);
                }
                address = next_address(address)?;
            }
            if !usable(prefix, address) {
                return Err(Error::Limit);
            }
        }
        if kind == K::Prefix {
            network = network.trunc();
        }
        let family = if network.addr().is_ipv4() { 4u64 } else { 6 };
        if link_local(network.addr()) && link.is_none() {
            return Err(Error::Invalid);
        }
        if sharing == api::SharedUse::Vrrp {
            if kind != K::IpAddress || link.is_none() || !matches!(request.vrid, Some(1..=255)) {
                return Err(Error::Invalid);
            }
        } else if request.vrid.is_some()
            || (kind == K::Prefix && sharing != api::SharedUse::Exclusive)
        {
            return Err(Error::Invalid);
        }
        let old_target = request
            .target
            .as_ref()
            .map(|t| parse_target(Some(t)))
            .transpose()?;
        request.name = request.name.trim().to_owned();
        let target = tx.workspace_write(
            context,
            api::Write {
                target: old_target.map(target_wire),
                expected: request.expected,
                kind: request.kind,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(request.name)),
                }],
            },
            now,
        )?;
        if !context.field(target, request.kind, "network", true) {
            return Err(Error::Denied);
        }
        let key = format!("network/{}", identity(target));
        let old = tx.get(&key)?;
        if old_target.is_some() != old.is_some() {
            return Err(Error::Missing);
        }
        let owner_key = allocation(space, network, kind, link)?;
        if let Some(owner) = tx.get(&owner_key)? {
            if !same_target(&owner, target) {
                return Err(Error::Conflict);
            }
        }
        let members = tx.children(&format!("members/{}", identity(target)))?;
        if sharing == api::SharedUse::Exclusive && members.len() > 1 {
            return Err(Error::Conflict);
        }
        if old.as_ref().is_some_and(|p| {
            !members.is_empty()
                && (p.get("address_space") != Some(&text(identity(space)))
                    || p.get("cidr") != Some(&text(network.to_string()))
                    || p.get("link_scope") != link.map(|t| text(identity(t))).as_ref())
        }) {
            return Err(Error::Conflict);
        }
        let protocol_key = if sharing == api::SharedUse::Vrrp {
            Some(format!(
                "gateway/{}/{family}/{}",
                identity(link.ok_or(Error::Invalid)?),
                request.vrid.ok_or(Error::Invalid)?
            ))
        } else {
            None
        };
        if let Some(key) = &protocol_key {
            if let Some(owner) = tx.get(key)? {
                if !same_target(&owner, target) {
                    return Err(Error::Conflict);
                }
            }
        }
        if old.as_ref().is_some_and(|p| {
            p.get("sharing") == Some(&text(api::SharedUse::Exclusive.as_str_name()))
        }) && sharing == api::SharedUse::Vrrp
        {
            // RFC 9568 section 6.1: the existing address owner has priority 255.
            for member in &members {
                let mut data = member.clone();
                super::super::operations::strip_envelope(&mut data);
                data.insert("priority".into(), int(255)?);
                tx.put_against(
                    &string(member, "key")?,
                    &string(member, "parent")?,
                    Class::Inventory,
                    data,
                    Some(member),
                )?;
            }
        }
        let mut data = target_props(target);
        data.extend(props([
            ("address_space", text(identity(space))),
            ("cidr", text(network.to_string())),
            ("namespace", text(&owner_key)),
            ("sharing", text(sharing.as_str_name())),
            ("family", int(family)?),
            ("prefix_len", int(u64::from(network.prefix_len()))?),
        ]));
        if let Some(link) = link {
            data.insert("link_scope".into(), text(identity(link)));
        }
        if let Some(routing) = routing {
            data.insert("routing_context".into(), text(identity(routing)));
        }
        if let Some(vrid) = request.vrid {
            data.insert("vrid".into(), int(u64::from(vrid))?);
        }
        if let Some(key) = &protocol_key {
            data.insert("group".into(), text(key));
        }
        if let Some(previous) = old
            .as_ref()
            .and_then(|p| p.get("group"))
            .and_then(Value::as_str)
        {
            if protocol_key.as_deref() != Some(previous) {
                tx.delete(previous)?;
            }
        }
        if old
            .as_ref()
            .is_some_and(|p| p.get("namespace") != data.get("namespace"))
        {
            tx.delete(&string(old.as_ref().ok_or(Error::Corrupt)?, "namespace")?)?;
        }
        tx.put(
            &owner_key,
            "allocations",
            Class::Inventory,
            target_props(target),
        )?;
        if let Some(key) = protocol_key {
            tx.put(
                &key,
                "gateway-groups",
                Class::Inventory,
                target_props(target),
            )?;
        }
        if tx.put_against(&key, "networks", Class::Inventory, data, old.as_ref())? {
            tx.workspace_advance(target, &context.actor, "network.accepted", now)?;
        }
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }

    pub(super) fn workspace_member(
        &self,
        context: &Context,
        member: api::Member,
        now: u64,
    ) -> Result<api::Reply> {
        let address = parse_target(member.address.as_ref())?;
        let interface = parse_target(member.interface.as_ref())?;
        let mut tx = self.begin()?;
        let row = tx.visible_asset(context, address)?;
        if number(&row, "revision")? != member.expected {
            return Err(Error::Conflict);
        }
        if number(&row, "kind")? != wire::AssetKind::IpAddress as u64
            || number(&tx.visible_asset(context, interface)?, "kind")?
                != wire::AssetKind::Interface as u64
        {
            return Err(Error::Invalid);
        }
        for target in [address, interface] {
            let kind = number(&tx.asset(target)?, "kind")? as i32;
            if !context.field(target, kind, "network", true) {
                return Err(Error::Denied);
            }
        }
        let network = tx
            .get(&format!("network/{}", identity(address)))?
            .ok_or(Error::Missing)?;
        let parent = format!("members/{}", identity(address));
        let key = format!("{parent}/{}", identity(interface));
        let old = tx.get(&key)?;
        let members = tx.children(&parent)?;
        if members.len() >= 128 && old.is_none() {
            return Err(Error::Limit);
        }
        let sharing = string(&network, "sharing")?;
        if sharing == api::SharedUse::Exclusive.as_str_name()
            && !members.is_empty()
            && old.is_none()
        {
            return Err(Error::Conflict);
        }
        if member.priority.is_some()
            && (sharing != api::SharedUse::Vrrp.as_str_name()
                || !matches!(member.priority, Some(1..=255)))
        {
            return Err(Error::Invalid);
        }
        if sharing == api::SharedUse::Vrrp.as_str_name()
            && member.priority.is_none()
            && !member.remove
        {
            return Err(Error::Invalid);
        }
        let changed = if member.remove {
            if old.is_some() {
                tx.delete(&key)?;
                true
            } else {
                false
            }
        } else {
            let mut data = target_props(interface);
            data.insert("group".into(), text(identity(address)));
            if let Some(priority) = member.priority {
                data.insert("priority".into(), int(u64::from(priority))?);
            }
            tx.put_against(&key, &parent, Class::Inventory, data, old.as_ref())?
        };
        if changed {
            tx.workspace_advance(address, &context.actor, "network.participant", now)?;
        }
        Ok(mutation(tx.commit()?, vec![target_wire(address)]))
    }

    pub(super) fn workspace_network_fields(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
        fields: &mut Vec<wire::Field>,
    ) -> Result<(Vec<api::Participant>, bool)> {
        let tx = self.begin()?;
        if let Some(row) = tx.get(&format!("network/{}", identity(target)))? {
            let columns = [
                "cidr",
                "address_space",
                "link_scope",
                "routing_context",
                "sharing",
                "vrid",
                "vlan_id",
            ]
            .map(str::to_owned);
            fields.extend(
                projection(&row, &columns)?
                    .into_iter()
                    .filter(|f| context.field(target, kind, &f.name, false)),
            );
        }
        let members = tx.children(&format!("members/{}", identity(target)))?;
        let mut participants = Vec::new();
        let mut complete = context.field(target, kind, "network", false);
        for row in members {
            let interface = super::super::target(&row)?;
            if !context.readable(interface, wire::AssetKind::Interface as i32)
                || !context.field(target, kind, "network", false)
            {
                complete = false;
                continue;
            }
            tx.asset(interface)?;
            let priority = row
                .get("priority")
                .map(|_| {
                    number(&row, "priority")
                        .and_then(|n| u32::try_from(n).map_err(|_| Error::Corrupt))
                })
                .transpose()?;
            participants.push(api::Participant {
                interface: Some(target_wire(interface)),
                priority,
            });
        }
        tx.commit()?;
        Ok((participants, complete))
    }
}
impl Tx<'_> {
    pub(super) fn workspace_advance(
        &mut self,
        target: AssetRef,
        actor: &str,
        action: &str,
        now: u64,
    ) -> Result<()> {
        let key = asset_key(target);
        let old = self.asset(target)?;
        let mut data = old.clone();
        super::super::operations::strip_envelope(&mut data);
        let revision = number(&old, "revision")?
            .checked_add(1)
            .ok_or(Error::Limit)?;
        data.insert("revision".into(), int(revision)?);
        self.put_against(&key, "assets", Class::Inventory, data, Some(&old))?;
        self.audit(&key, actor, action, revision, now)
    }
}
