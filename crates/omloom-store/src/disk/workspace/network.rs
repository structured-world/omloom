use super::*;
use ipnet::IpNet;
use std::net::IpAddr;
pub(super) mod groups;

pub(super) fn identity(target: AssetRef) -> String {
    format!("{}/{}", target.asset, target.incarnation)
}
pub(super) fn address_order(address: IpAddr) -> String {
    match address {
        IpAddr::V4(ip) => format!("4/{:08x}", u32::from(ip)),
        IpAddr::V6(ip) => format!("6/{:032x}", u128::from(ip)),
    }
}
pub(super) fn link_local(address: IpAddr) -> bool {
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
        mut request: api::Network,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if request.address_space.is_some()
            || request.routing.is_some()
            || !request.cidr.is_empty()
            || request.vrid.is_some()
            || request.next_available
            || request.group.is_some()
            || !request.group_name.is_empty()
            || request.expected_group.is_some()
            || request.previous_group_revision.is_some()
            || request.sharing != api::SharedUse::Exclusive as i32
        {
            return Err(Error::Invalid);
        }
        let Some(vid @ 1..=4094) = request.vlan_id else {
            return Err(Error::Invalid);
        };
        let mut tx = self.begin()?;
        tx.resolve_allocation_parent(context, working, &mut request)?;
        tx.check_network_context(context, working, &request)?;
        let link = parse_target(request.link.as_ref())?;
        if number(&tx.visible_asset(context, link)?, "kind")? != wire::AssetKind::L2Domain as u64 {
            return Err(Error::Invalid);
        }
        let target = tx.workspace_network_write(
            context,
            api::Write {
                target: request.target,
                expected: request.expected,
                kind: request.kind,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(request.name)),
                }],
                parent: None,
            },
            now,
        )?;
        if !context.field(target, request.kind, "network", true) {
            return Err(Error::Denied);
        }
        let key = format!("network/{}", identity(target));
        let old = tx.get(&key)?;
        let owner_key = format!("vlan/{}/{vid}", identity(link));
        if let Some(owner) = tx.get(&owner_key)?
            && !same_target(&owner, target)
        {
            return Err(Error::Conflict);
        }
        if let Some(old) = &old
            && old.get("namespace") != Some(&text(&owner_key))
        {
            tx.delete(&string(old, "namespace")?)?;
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
        request: api::Network,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        use wire::AssetKind as K;
        let kind = K::try_from(request.kind).map_err(|_| Error::Invalid)?;
        if kind == K::AddressGroup {
            return self.workspace_group_settings(context, request, working, now);
        }
        if kind == K::Vlan {
            return self.workspace_vlan(context, request, working, now);
        }
        let mut tx = self.begin()?;
        let targets = self.workspace_network_in(&mut tx, context, request, working, now)?;
        Ok(mutation(tx.commit()?, targets))
    }

    pub(super) fn workspace_network_in(
        &self,
        tx: &mut Tx<'_>,
        context: &Context,
        mut request: api::Network,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<Vec<wire::AssetRef>> {
        use wire::AssetKind as K;
        let kind = K::try_from(request.kind).map_err(|_| Error::Invalid)?;
        if !matches!(kind, K::IpAddress | K::Prefix) {
            return Err(Error::Invalid);
        }
        if request.vlan_id.is_some() {
            return Err(Error::Invalid);
        }
        tx.resolve_allocation_parent(context, working, &mut request)?;
        let space = parse_target(request.address_space.as_ref())?;
        let routing = request
            .routing
            .as_ref()
            .map(|r| parse_target(Some(r)))
            .transpose()?;
        if request.cidr.len() > 64 {
            return Err(Error::Invalid);
        }
        let mut network: IpNet = request.cidr.parse().map_err(|_| Error::Invalid)?;
        let family = if network.addr().is_ipv4() { 4u64 } else { 6 };
        if kind == K::Prefix
            && (request.group.is_some()
                || !request.group_name.is_empty()
                || request.sharing != api::SharedUse::Exclusive as i32)
        {
            return Err(Error::Invalid);
        }
        tx.check_network_context(context, working, &request)?;
        let group = tx.workspace_shared_group(context, &mut request, family, now)?;
        let link = request
            .link
            .as_ref()
            .map(|r| parse_target(Some(r)))
            .transpose()?;
        let sharing = api::SharedUse::try_from(request.sharing).map_err(|_| Error::Invalid)?;
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
        if link_local(network.addr()) && link.is_none() {
            return Err(Error::Invalid);
        }
        if sharing == api::SharedUse::Vrrp {
            if kind != K::IpAddress
                || link.is_none()
                || (request.group.is_none() && !matches!(request.vrid, Some(1..=255)))
            {
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
        let target = tx.workspace_network_write(
            context,
            api::Write {
                target: old_target.map(target_wire),
                expected: request.expected,
                kind: request.kind,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(request.name)),
                }],
                parent: None,
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
        if let Some(owner) = tx.get(&owner_key)?
            && !same_target(&owner, target)
        {
            return Err(Error::Conflict);
        }
        let members = tx.children(&format!("members/{}", identity(target)))?;
        if sharing == api::SharedUse::Exclusive && members.len() > 1 {
            return Err(Error::Conflict);
        }
        if old
            .as_ref()
            .is_some_and(|p| p.get("namespace") != Some(&text(&owner_key)))
        {
            if !members.is_empty() {
                return Err(Error::Conflict);
            }
            // Reconciled references depend on identity even without assigned interfaces.
            let references = tx.query(
                "MATCH (n:OmRow {parent:'networks'}) WHERE n.binding=$binding RETURN properties(n) AS p LIMIT 1",
                params([("binding", text(identity(target)))]),
            )?;
            if !references.is_empty() {
                return Err(Error::Conflict);
            }
        }
        let previous_group = old.as_ref().map(groups::group_of).transpose()?.flatten();
        let mut changed_groups = BTreeSet::new();
        if let Some(previous) = previous_group
            && Some(previous) != group
        {
            tx.workspace_group_member_owner(context, previous, request.previous_group_revision)?;
            changed_groups.insert(previous);
        } else if request.previous_group_revision.is_some()
            || (previous_group.is_none()
                && request.group.is_none()
                && request.expected_group.is_some())
        {
            return Err(Error::Invalid);
        }
        if let Some(group) = group {
            let parent = format!("members/{}", identity(group));
            let current = tx.children(&parent)?;
            for member in &members {
                let interface = super::super::target(member)?;
                tx.visible_asset(context, interface)?;
                if !context.field(
                    interface,
                    wire::AssetKind::Interface as i32,
                    "network",
                    true,
                ) {
                    return Err(Error::Denied);
                }
                if current.len() >= 128 && !current.iter().any(|p| same_target(p, interface)) {
                    return Err(Error::Limit);
                }
                // RFC 9568 section 6.1: a converted address owner retains 255.
                if sharing == api::SharedUse::Vrrp
                    && current.iter().any(|p| {
                        p.get("priority") == Some(&Value::Int(255)) && !same_target(p, interface)
                    })
                {
                    return Err(Error::Conflict);
                }
                let mut data = target_props(interface);
                data.insert("group".into(), text(identity(group)));
                if sharing == api::SharedUse::Vrrp {
                    data.insert("priority".into(), Value::Int(255));
                }
                tx.put(
                    &format!("{parent}/{}", identity(interface)),
                    &parent,
                    Class::Inventory,
                    data,
                )?;
                tx.delete(&string(member, "key")?)?;
            }
            if request.group.is_some() && (previous_group != Some(group) || !members.is_empty()) {
                changed_groups.insert(group);
            }
        }
        let mut data = target_props(target);
        data.extend(props([
            ("address_space", text(identity(space))),
            ("cidr", text(network.to_string())),
            ("ip_order", text(address_order(network.addr()))),
            ("prefix_len", int(u64::from(network.prefix_len()))?),
            ("namespace", text(&owner_key)),
            ("sharing", text(sharing.as_str_name())),
        ]));
        if let Some(link) = link {
            data.insert("link_scope".into(), text(identity(link)));
        }
        if let Some(routing) = routing {
            data.insert("routing_context".into(), text(identity(routing)));
        }
        if let Some(group) = group {
            data.insert("group".into(), text(identity(group)));
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
        if tx.put_against(&key, "networks", Class::Inventory, data, old.as_ref())? {
            tx.workspace_advance(target, &context.actor, "network.accepted", now)?;
        }
        let mut targets = vec![target_wire(target)];
        for changed in changed_groups {
            tx.workspace_advance(changed, &context.actor, "group.addresses", now)?;
        }
        if let Some(group) = group {
            targets.push(target_wire(group));
        }
        Ok(targets)
    }

    pub(super) fn workspace_member(
        &self,
        context: &Context,
        member: api::Member,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        let mut tx = self.begin()?;
        let owner = self.workspace_member_in(&mut tx, context, member, working, now)?;
        Ok(mutation(tx.commit()?, vec![target_wire(owner)]))
    }

    pub(super) fn workspace_member_in(
        &self,
        tx: &mut Tx<'_>,
        context: &Context,
        member: api::Member,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<AssetRef> {
        let address = parse_target(member.address.as_ref())?;
        let interface = parse_target(member.interface.as_ref())?;
        let selection = tx.selection(context, working, true)?;
        if selection.focus.is_some() {
            return Err(Error::Denied);
        }
        selection.contains(address, tx)?;
        selection.contains(interface, tx)?;
        let row = tx.visible_asset(context, address)?;
        if number(&row, "revision")? != member.expected {
            return Err(Error::Conflict);
        }
        let direct_group = number(&row, "kind")? == wire::AssetKind::AddressGroup as u64;
        if (!direct_group && number(&row, "kind")? != wire::AssetKind::IpAddress as u64)
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
        let (network, owner) = if direct_group {
            if member.expected_group.is_some() {
                return Err(Error::Invalid);
            }
            (
                tx.get(&format!("group/{}", identity(address)))?
                    .ok_or(Error::Corrupt)?,
                address,
            )
        } else {
            let network = tx
                .get(&format!("network/{}", identity(address)))?
                .ok_or(Error::Missing)?;
            if let Some(group) = groups::group_of(&network)? {
                (
                    tx.workspace_group_member_owner(context, group, member.expected_group)?,
                    group,
                )
            } else {
                if member.expected_group.is_some() {
                    return Err(Error::Invalid);
                }
                (network, address)
            }
        };
        selection.contains(owner, tx)?;
        let parent = format!("members/{}", identity(owner));
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
        if !member.remove
            && member.priority == Some(255)
            && members
                .iter()
                .any(|p| p.get("priority") == Some(&Value::Int(255)) && !same_target(p, interface))
        {
            return Err(Error::Conflict);
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
            data.insert("group".into(), text(identity(owner)));
            if let Some(priority) = member.priority {
                data.insert("priority".into(), int(u64::from(priority))?);
            }
            tx.put_against(&key, &parent, Class::Inventory, data, old.as_ref())?
        };
        if changed {
            tx.workspace_advance(owner, &context.actor, "network.participant", now)?;
        }
        Ok(owner)
    }

    pub(super) fn workspace_network_fields(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
        fields: &mut Vec<wire::Field>,
    ) -> Result<(Vec<api::Participant>, bool, Option<api::Item>)> {
        let tx = self.begin()?;
        let direct_group = kind == wire::AssetKind::AddressGroup as i32;
        let row = tx.get(&format!(
            "{}/{}",
            if direct_group { "group" } else { "network" },
            identity(target)
        ))?;
        if let Some(row) = &row {
            let columns = [
                "cidr",
                "address_space",
                "link_scope",
                "routing_context",
                "sharing",
                "vrid",
                "vlan_id",
                "address",
                "prefix_length",
                "meaning",
                "zone",
                "binding",
                "name",
            ]
            .map(str::to_owned);
            fields.extend(
                addresses::visible_projection(context, row, &columns)?
                    .into_iter()
                    .filter(|f| context.field(target, kind, &f.name, false)),
            );
        }
        let group = if direct_group {
            None
        } else {
            row.as_ref().map(groups::group_of).transpose()?.flatten()
        };
        let mut summary = None;
        let mut participants = Vec::new();
        let mut complete = context.field(target, kind, "network", false);
        if let Some(group) = group {
            let root = tx.asset(group)?;
            let group_kind = wire::AssetKind::AddressGroup as i32;
            if !context.readable(group, group_kind)
                || number(&root, "adoption")? != wire::Adoption::Adopted as u64
            {
                tx.commit()?;
                return Ok((participants, false, None));
            }
            let settings = tx
                .get(&format!("group/{}", identity(group)))?
                .ok_or(Error::Corrupt)?;
            if context.field(target, kind, "vrid", false)
                && context.field(group, group_kind, "vrid", false)
            {
                fields.extend(projection(&settings, &["vrid".into()])?);
            }
            let mut group_fields = projection(
                &settings,
                &[
                    "sharing".into(),
                    "link_scope".into(),
                    "vrid".into(),
                    "family".into(),
                ],
            )?;
            if context.field(group, group_kind, "name", false) {
                group_fields.push(wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(string(&root, "name")?)),
                });
            }
            group_fields.retain(|field| context.field(group, group_kind, &field.name, false));
            if complete {
                summary = Some(api::Item {
                    target: Some(target_wire(group)),
                    kind: group_kind,
                    revision: number(&root, "revision")?,
                    fields: group_fields,
                });
            }
            complete &= context.field(group, group_kind, "network", false);
        }
        let owner = group.unwrap_or(target);
        let members = tx.children(&format!("members/{}", identity(owner)))?;
        let membership_visible = complete;
        for row in members {
            let interface = super::super::target(&row)?;
            if !context.readable(interface, wire::AssetKind::Interface as i32)
                || !membership_visible
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
        Ok((participants, complete, summary))
    }
}
impl Tx<'_> {
    fn resolve_allocation_parent(
        &self,
        context: &Context,
        working: &api::WorkingContext,
        request: &mut api::Network,
    ) -> Result<()> {
        let selection = self.selection(context, working, true)?;
        if request.target.is_some() {
            if request.parent.is_some() {
                return Err(Error::Invalid);
            }
            return Ok(());
        }
        let parent = request
            .parent
            .as_deref()
            .map(|p| parse_target(Some(p)))
            .transpose()?;
        if selection.focus.is_some() && parent != selection.focus {
            return Err(Error::Invalid);
        }
        if !working.roots.is_empty() && parent.is_none() {
            return Err(Error::Invalid);
        }
        let Some(parent) = parent else {
            return Ok(());
        };
        selection.contains(parent, self)?;
        let row = self.visible_asset(context, parent)?;
        let kind = number(&row, "kind")? as i32;
        if !context.field(parent, kind, "relations", true) {
            return Err(Error::Denied);
        }
        let (space, link) = match wire::AssetKind::try_from(kind).map_err(|_| Error::Corrupt)? {
            wire::AssetKind::AddressSpace
                if request.kind == wire::AssetKind::IpAddress as i32
                    || request.kind == wire::AssetKind::Prefix as i32 =>
            {
                (Some(parent), None)
            }
            wire::AssetKind::Prefix
                if request.kind == wire::AssetKind::IpAddress as i32
                    || request.kind == wire::AssetKind::Prefix as i32 =>
            {
                if !context.field(parent, kind, "cidr", false)
                    || !context.field(parent, kind, "address_space", false)
                {
                    return Err(Error::Denied);
                }
                let network = self
                    .get(&format!("network/{}", identity(parent)))?
                    .ok_or(Error::Missing)?;
                let block: IpNet = string(&network, "cidr")?
                    .parse()
                    .map_err(|_| Error::Corrupt)?;
                let proposed: IpNet = request.cidr.parse().map_err(|_| Error::Invalid)?;
                if !block.contains(&proposed.addr())
                    || ((request.kind == wire::AssetKind::Prefix as i32 || request.next_available)
                        && proposed.prefix_len() < block.prefix_len())
                {
                    return Err(Error::Invalid);
                }
                (
                    Some(groups::reference(&string(&network, "address_space")?)?),
                    if link_local(block.addr()) {
                        Some(groups::reference(&string(&network, "link_scope")?)?)
                    } else {
                        None
                    },
                )
            }
            wire::AssetKind::L2Domain if request.kind == wire::AssetKind::Vlan as i32 => {
                (None, Some(parent))
            }
            _ => return Err(Error::Invalid),
        };
        for (provided, derived) in [
            (&mut request.address_space, space),
            (&mut request.link, link),
        ] {
            if let Some(derived) = derived {
                if provided
                    .as_ref()
                    .is_some_and(|p| parse_target(Some(p)).ok() != Some(derived))
                {
                    return Err(Error::Conflict);
                }
                *provided = Some(target_wire(derived));
            }
        }
        Ok(())
    }
    pub(super) fn check_network_context(
        &self,
        context: &Context,
        working: &api::WorkingContext,
        request: &api::Network,
    ) -> Result<()> {
        let selection = self.selection(context, working, true)?;
        for reference in [
            request.target.as_ref(),
            request.address_space.as_ref(),
            request.link.as_ref(),
            request.routing.as_ref(),
            request.group.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if request.target.is_none()
                && request.parent.is_some()
                && request.address_space.as_ref() == Some(reference)
            {
                continue;
            }
            if request.target.is_none()
                && request.link.as_ref() == Some(reference)
                && let Some(parent) = request.parent.as_deref()
                && let Some(row) = self.get(&format!(
                    "network/{}",
                    identity(parse_target(Some(parent))?)
                ))?
                && row.get("link_scope") == Some(&text(identity(parse_target(Some(reference))?)))
                && link_local(
                    string(&row, "cidr")?
                        .parse::<IpNet>()
                        .map_err(|_| Error::Corrupt)?
                        .addr(),
                )
            {
                continue;
            }
            selection.contains(parse_target(Some(reference))?, self)?;
        }
        if let Some(target) = &request.target {
            let target = parse_target(Some(target))?;
            if let Some(old) = self.get(&format!("network/{}", identity(target)))? {
                for field in ["address_space", "link_scope", "routing_context", "group"] {
                    if let Some(value) = old.get(field) {
                        selection.contains(
                            groups::reference(value.as_str().ok_or(Error::Corrupt)?)?,
                            self,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
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
