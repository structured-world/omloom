use super::*;
use std::net::IpAddr;
pub(super) mod knowledge;

fn validate_details(address: IpAddr, mask: Option<i64>, zone: Option<&str>) -> Result<()> {
    if mask.is_some_and(|n| n < 0 || n > if address.is_ipv4() { 32 } else { 128 })
        || zone.is_some_and(|s| {
            s.is_empty()
                || s.len() > 64
                || s.chars().any(char::is_control)
                || !matches!(address,IpAddr::V6(ip) if ip.is_unicast_link_local())
        })
    {
        return Err(Error::Invalid);
    }
    Ok(())
}

pub(super) fn projection(row: &Properties, columns: &[String]) -> Result<Vec<wire::Field>> {
    let mut fields = network::projection(row, columns)?;
    if row.contains_key("address") && columns.iter().any(|s| s == "name") {
        fields.push(wire::Field {
            name: "name".into(),
            value: Some(field::Value::Text(string(row, "address")?)),
        });
    }
    Ok(fields)
}

pub(super) fn visible_projection(
    context: &Context,
    row: &Properties,
    columns: &[String],
) -> Result<Vec<wire::Field>> {
    let mut fields = projection(row, columns)?;
    if let Some(binding) = row.get("binding") {
        let canonical = network::groups::reference(binding.as_str().ok_or(Error::Corrupt)?)?;
        if !context.readable(canonical, wire::AssetKind::IpAddress as i32) {
            fields.retain(|field| field.name != "binding");
        }
    }
    Ok(fields)
}

pub(super) fn from_write(write: api::Write) -> Result<api::Address> {
    let mut request = api::Address {
        target: write.target,
        expected: write.expected,
        parent: write.parent,
        ..Default::default()
    };
    for field in write.changes {
        match (field.name.as_str(), field.value.as_ref()) {
            ("address", Some(field::Value::Text(s))) => request.address = Some(s.clone()),
            ("prefix_length", Some(field::Value::Integer(n))) => {
                request.prefix_length = Some(u32::try_from(*n).map_err(|_| Error::Invalid)?)
            }
            ("prefix_length", Some(field::Value::Clear(EXPLICIT_CLEAR))) => {
                request.clear_prefix_length = true
            }
            ("meaning", Some(field::Value::Text(s))) => {
                request.meaning =
                    Some(api::AddressMeaning::from_str_name(s).ok_or(Error::Invalid)? as i32)
            }
            ("zone", Some(field::Value::Text(s))) => request.zone = Some(s.clone()),
            ("zone", Some(field::Value::Clear(EXPLICIT_CLEAR))) => request.clear_zone = true,
            ("binding", Some(field::Value::Text(s))) => {
                request.binding = Some(target_wire(network::groups::reference(s)?))
            }
            ("binding", Some(field::Value::Clear(EXPLICIT_CLEAR))) => request.clear_binding = true,
            ("broadcast", Some(field::Value::Text(s))) => request.broadcast = Some(s.clone()),
            ("broadcast", Some(field::Value::Clear(EXPLICIT_CLEAR))) => {
                request.clear_broadcast = true
            }
            ("prefix", Some(field::Value::Text(s))) => {
                request.prefix = Some(target_wire(network::groups::reference(s)?))
            }
            ("prefix", Some(field::Value::Clear(EXPLICIT_CLEAR))) => request.clear_prefix = true,
            ("create_prefix", Some(field::Value::Flag(value))) => request.create_prefix = *value,
            ("prefix_name", Some(field::Value::Text(s))) => request.prefix_name = Some(s.clone()),
            ("address" | "prefix_length" | "meaning" | "zone" | "binding", _) => {
                return Err(Error::Invalid);
            }
            _ => request.changes.push(field),
        }
    }
    knowledge::normalize(&mut request)?;
    Ok(request)
}

impl Store {
    pub(super) fn workspace_address(
        &self,
        context: &Context,
        request: api::Address,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        self.workspace_address_inner(context, request, working, now, false)
    }
    pub(super) fn workspace_preview_address(
        &self,
        context: &Context,
        request: api::Address,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        self.workspace_address_inner(context, request, working, now, true)
    }
    fn workspace_address_inner(
        &self,
        context: &Context,
        mut request: api::Address,
        working: &api::WorkingContext,
        now: u64,
        preview: bool,
    ) -> Result<api::Reply> {
        knowledge::normalize(&mut request)?;
        let replay = if request.target.is_none() {
            Some(request.encode_to_vec())
        } else {
            None
        };
        let mut tx = self.begin()?;
        if !preview
            && let Some(input) = &replay
            && let Some(saved) = tx.creation_outcome(context, working, input, "address")?
        {
            return Ok(reply(api::reply::Result::Mutation(saved)));
        }
        if (request.allocation.is_some() || request.interface.is_some())
            && working.expected.is_none()
        {
            return Err(Error::Conflict);
        }
        if let Some(interface) = request.interface.as_deref() {
            let interface = parse_target(Some(interface))?;
            let row = tx.visible_asset(context, interface)?;
            tx.selection(context, working, true)?
                .contains(interface, &tx)?;
            if number(&row, "kind")? != wire::AssetKind::Interface as u64
                || !context.field(
                    interface,
                    wire::AssetKind::Interface as i32,
                    "network",
                    true,
                )
            {
                return Err(Error::Denied);
            }
        }
        if request.allocation.is_some() && (request.binding.is_some() || request.clear_binding) {
            return Err(Error::Invalid);
        }
        if let Some(allocation) = request.allocation.take() {
            if working.focus.is_some()
                || allocation.target.is_some()
                || allocation.expected.is_some()
                || allocation.kind != wire::AssetKind::IpAddress as i32
                || allocation.next_available
            {
                return Err(Error::Invalid);
            }
            request.binding = Some(
                self.workspace_network_in(&mut tx, context, *allocation, working, now)?
                    .into_iter()
                    .next()
                    .ok_or(Error::Corrupt)?,
            );
        }
        let selected_prefix = tx.prepare_address_prefix(
            context,
            &mut request,
            working,
            now,
            knowledge::PrefixCreation::New,
        )?;
        let clear_prefix = request.clear_prefix;
        let assignment = request.interface.take();
        let priority = request.priority.take();
        if priority.is_some() && assignment.is_none() {
            return Err(Error::Invalid);
        }
        let target = tx.workspace_address(context, request, working, now)?;
        tx.set_address_prefix(context, target, selected_prefix, clear_prefix, working, now)?;
        if let Some(interface) = assignment {
            let fact = tx
                .get(&format!("network/{}", network::identity(target)))?
                .ok_or(Error::Corrupt)?;
            if fact.get("meaning") != Some(&text(api::AddressMeaning::Configured.as_str_name())) {
                return Err(Error::Invalid);
            }
            let binding = fact.get("binding").ok_or(Error::Invalid)?;
            let canonical = network::groups::reference(binding.as_str().ok_or(Error::Corrupt)?)?;
            let root = tx.asset(canonical)?;
            let row = tx
                .get(&format!("network/{}", network::identity(canonical)))?
                .ok_or(Error::Corrupt)?;
            let group = network::groups::group_of(&row)?
                .map(|g| tx.asset(g))
                .transpose()?;
            self.workspace_member_in(
                &mut tx,
                context,
                api::Member {
                    address: Some(target_wire(canonical)),
                    interface: Some(*interface),
                    expected: number(&root, "revision")?,
                    expected_group: group.as_ref().map(|g| number(g, "revision")).transpose()?,
                    priority,
                    remove: false,
                },
                working,
                now,
            )?;
        }
        let mut targets = vec![target_wire(target)];
        if let Some(prefix) = selected_prefix {
            targets.push(target_wire(prefix));
        }
        if preview {
            let fact = tx
                .get(&format!("network/{}", network::identity(target)))?
                .ok_or(Error::Corrupt)?;
            let readable = |field| {
                context.field(
                    target,
                    wire::AssetKind::AddressReference as i32,
                    field,
                    false,
                )
            };
            let summary = api::AddressReview {
                revision: number(&tx.root, "revision")?,
                address: if readable("address") {
                    Some(string(&fact, "address")?)
                } else {
                    None
                },
                prefix_length: if readable("prefix_length") {
                    fact.get("prefix_length")
                        .and_then(Value::as_int)
                        .map(|n| u32::try_from(n).map_err(|_| Error::Corrupt))
                        .transpose()?
                } else {
                    None
                },
                zone: if readable("zone") {
                    fact.get("zone").and_then(Value::as_str).map(str::to_owned)
                } else {
                    None
                },
                broadcast: if readable("broadcast") {
                    fact.get("broadcast")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                },
                prefix_cidr: selected_prefix
                    .filter(|prefix| {
                        context.field(*prefix, wire::AssetKind::Prefix as i32, "cidr", false)
                    })
                    .map(|prefix| {
                        tx.get(&format!("network/{}", network::identity(prefix)))?
                            .ok_or(Error::Missing)
                            .and_then(|row| string(&row, "cidr"))
                    })
                    .transpose()?,
            };
            return Ok(reply(api::reply::Result::AddressReview(summary)));
        }
        if let Some(input) = replay {
            tx.save_creation_outcome(context, working, &input, &targets, now, "address")?;
        }
        Ok(mutation(tx.commit()?, targets))
    }
}

impl Tx<'_> {
    pub(in super::super) fn check_bound_address_owner(
        &self,
        reference: AssetRef,
        owner: AssetRef,
    ) -> Result<()> {
        let Some(fact) = self.get(&format!("network/{}", network::identity(reference)))? else {
            return Ok(());
        };
        if fact.get("meaning") != Some(&text(api::AddressMeaning::Configured.as_str_name())) {
            return Ok(());
        }
        let Some(binding) = fact.get("binding") else {
            return Ok(());
        };
        let binding = network::groups::reference(binding.as_str().ok_or(Error::Corrupt)?)?;
        let canonical = self
            .get(&format!("network/{}", network::identity(binding)))?
            .ok_or(Error::Missing)?;
        self.check_exclusive_address_owner(&canonical, owner)
    }

    fn check_exclusive_address_owner(&self, canonical: &Properties, owner: AssetRef) -> Result<()> {
        if canonical.get("sharing") != Some(&text(api::SharedUse::Exclusive.as_str_name())) {
            return Ok(());
        }
        let binding = super::super::target(canonical)?;
        let member_owner = network::groups::group_of(canonical)?.unwrap_or(binding);
        for member in self.children(&format!("members/{}", network::identity(member_owner)))? {
            if super::super::target(&member)? != owner {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }

    pub(super) fn preview_address(&self, write: &api::Write) -> Result<()> {
        let request = from_write(write.clone())?;
        if request.binding.is_some() || request.clear_binding {
            return Err(Error::Invalid);
        }
        let old = request
            .target
            .as_ref()
            .map(|t| parse_target(Some(t)))
            .transpose()?
            .map(|t| self.get(&format!("network/{}", network::identity(t))))
            .transpose()?
            .flatten();
        let address: IpAddr = if let Some(address) = request.address {
            address.parse().map_err(|_| Error::Invalid)?
        } else {
            string(old.as_ref().ok_or(Error::Invalid)?, "address")?
                .parse()
                .map_err(|_| Error::Corrupt)?
        };
        let mask = if request.clear_prefix_length {
            None
        } else {
            request.prefix_length.map(i64::from).or_else(|| {
                old.as_ref()
                    .and_then(|r| r.get("prefix_length"))
                    .and_then(Value::as_int)
            })
        };
        let zone = if request.clear_zone {
            None
        } else {
            request.zone.as_deref().or_else(|| {
                old.as_ref()
                    .and_then(|r| r.get("zone"))
                    .and_then(Value::as_str)
            })
        };
        validate_details(address, mask, zone)
    }
    pub(super) fn workspace_address(
        &mut self,
        context: &Context,
        request: api::Address,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<AssetRef> {
        if request.changes.iter().any(|f| {
            matches!(
                f.name.as_str(),
                "name"
                    | "serial"
                    | "address"
                    | "prefix_length"
                    | "meaning"
                    | "zone"
                    | "binding"
                    | "broadcast"
            )
        }) {
            return Err(Error::Invalid);
        }
        if (request.binding.is_some() || request.clear_binding) && working.expected.is_none() {
            return Err(Error::Conflict);
        }
        let supplied: Option<IpAddr> = request
            .address
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|_| Error::Invalid)?;
        if request.prefix_length.is_some_and(|n| n > 128)
            || request
                .zone
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 64 || s.chars().any(char::is_control))
            || (request.clear_binding && request.binding.is_some())
            || (request.clear_prefix_length && request.prefix_length.is_some())
            || (request.clear_zone && request.zone.is_some())
        {
            return Err(Error::Invalid);
        }
        let target = request
            .target
            .as_ref()
            .map(|t| parse_target(Some(t)))
            .transpose()?
            .unwrap_or_else(new_target);
        let kind = wire::AssetKind::AddressReference;
        let parent = if request.target.is_none() {
            self.creation_parent(context, working, request.parent.as_ref(), kind)?
        } else {
            if request.parent.is_some() {
                return Err(Error::Invalid);
            }
            self.selection(context, working, true)?
                .contains(target, self)?;
            None
        };
        if supplied.is_some() && !context.field(target, kind as i32, "address", true) {
            return Err(Error::Denied);
        }
        let root_key = asset_key(target);
        let old_root = self.get(&root_key)?;
        if request.target.is_some() && old_root.is_none() {
            return Err(Error::Missing);
        }
        if request.target.is_some() {
            self.visible_asset(context, target)?;
        }
        if self.get(&format!("removed/{}", target.asset))?.is_some() {
            return Err(Error::Conflict);
        }
        if old_root
            .as_ref()
            .map(|r| number(r, "revision"))
            .transpose()?
            != request.expected
        {
            return Err(Error::Conflict);
        }
        if let Some(old) = &old_root
            && (super::super::target(old)? != target
                || number(old, "kind")? != kind as u64
                || number(old, "adoption")? != wire::Adoption::Adopted as u64)
        {
            return Err(Error::Conflict);
        }
        let key = format!("network/{}", network::identity(target));
        let old = self.get(&key)?;
        let address: IpAddr = if let Some(address) = supplied {
            address
        } else {
            string(old.as_ref().ok_or(Error::Invalid)?, "address")?
                .parse()
                .map_err(|_| Error::Corrupt)?
        };
        for (name, provided) in [
            (
                "prefix_length",
                request.prefix_length.is_some() || request.clear_prefix_length,
            ),
            ("zone", request.zone.is_some() || request.clear_zone),
            ("meaning", request.meaning.is_some()),
        ] {
            if provided && !context.field(target, kind as i32, name, true) {
                return Err(Error::Denied);
            }
        }
        let meaning = request
            .meaning
            .map(api::AddressMeaning::try_from)
            .transpose()
            .map_err(|_| Error::Invalid)?
            .map(|m| m.as_str_name().to_owned())
            .or_else(|| {
                old.as_ref()
                    .and_then(|r| r.get("meaning"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| api::AddressMeaning::Unspecified.as_str_name().to_owned());
        let mut data = target_props(target);
        data.extend(props([
            ("address", text(address.to_string())),
            ("authority", text(self.workspace_authority()?)),
        ]));
        if request.clear_broadcast && request.broadcast.is_some() {
            return Err(Error::Invalid);
        }
        let broadcast = if request.clear_broadcast {
            None
        } else {
            request.broadcast.as_deref().or_else(|| {
                old.as_ref()
                    .and_then(|r| r.get("broadcast"))
                    .and_then(Value::as_str)
            })
        };
        if let Some(broadcast) = broadcast {
            let value: std::net::Ipv4Addr = broadcast.parse().map_err(|_| Error::Invalid)?;
            if !address.is_ipv4() {
                return Err(Error::Invalid);
            }
            data.insert("broadcast".into(), text(value.to_string()));
        }
        if meaning != api::AddressMeaning::Unspecified.as_str_name() {
            data.insert("meaning".into(), text(&meaning));
        }
        for (name, clear) in [
            ("prefix_length", request.clear_prefix_length),
            ("zone", request.clear_zone),
        ] {
            if !clear && let Some(value) = old.as_ref().and_then(|r| r.get(name)) {
                data.insert(name.into(), value.clone());
            }
        }
        if let Some(mask) = request.prefix_length {
            data.insert("prefix_length".into(), int(u64::from(mask))?);
        }
        if let Some(zone) = &request.zone {
            data.insert("zone".into(), text(zone));
        }
        validate_details(
            address,
            data.get("prefix_length").and_then(Value::as_int),
            data.get("zone").and_then(Value::as_str),
        )?;
        if data.contains_key("broadcast")
            && data
                .get("prefix_length")
                .and_then(Value::as_int)
                .is_none_or(|mask| mask >= 31)
        {
            return Err(Error::Invalid);
        }
        let binding = if request.clear_binding {
            None
        } else if let Some(binding) = &request.binding {
            Some(parse_target(Some(binding))?)
        } else {
            old.as_ref()
                .and_then(|r| r.get("binding"))
                .map(|v| network::groups::reference(v.as_str().ok_or(Error::Corrupt)?))
                .transpose()?
        };
        if let Some(binding) = binding {
            if request.binding.is_none()
                && old.as_ref().and_then(|r| r.get("address")) != data.get("address")
            {
                return Err(Error::Conflict);
            }
            // Meaning and zone changes must preserve reconciliation even when the mapping is retained.
            if request.binding.is_some()
                || old.as_ref().and_then(|r| r.get("meaning")) != data.get("meaning")
                || old.as_ref().and_then(|r| r.get("zone")) != data.get("zone")
            {
                if request.binding.is_some() {
                    let selection = self.selection(context, working, true)?;
                    if selection.focus.is_some() {
                        return Err(Error::Denied);
                    }
                    selection.contains(binding, self)?;
                    if !context.field(target, kind as i32, "binding", true) {
                        return Err(Error::Denied);
                    }
                }
                let allocation = self.visible_asset(context, binding)?;
                if number(&allocation, "kind")? != wire::AssetKind::IpAddress as u64
                    || !context.field(binding, wire::AssetKind::IpAddress as i32, "cidr", false)
                {
                    return Err(Error::Denied);
                }
                let canonical = self
                    .get(&format!("network/{}", network::identity(binding)))?
                    .ok_or(Error::Missing)?;
                let cidr: ipnet::IpNet = string(&canonical, "cidr")?
                    .parse()
                    .map_err(|_| Error::Corrupt)?;
                if cidr.addr() != address {
                    return Err(Error::Conflict);
                }
                if network::link_local(address) {
                    // RFC 4007 section 6: a host-local zone is not a portable link identity.
                    // An explicit enclosing L2 context is required to resolve the zone.
                    let link = network::groups::reference(&string(&canonical, "link_scope")?)?;
                    if request.binding.is_some() {
                        self.selection(context, working, true)?
                            .contains(link, self)?;
                    }
                    if address.is_ipv6() && !data.contains_key("zone") {
                        return Err(Error::Incomplete);
                    }
                }
                if meaning == api::AddressMeaning::Configured.as_str_name() {
                    let owners = self.working_relations(target)?;
                    for relation in owners {
                        let owner = parse_target(relation.from.as_ref())?;
                        if relation.kind != wire::RelationKind::Contains as i32
                            || parse_target(relation.to.as_ref())? != target
                        {
                            continue;
                        }
                        let owner_row = self.asset(owner)?;
                        if number(&owner_row, "kind")? != wire::AssetKind::Interface as u64 {
                            continue;
                        }
                        self.check_exclusive_address_owner(&canonical, owner)?;
                    }
                }
            }
            data.insert("binding".into(), text(network::identity(binding)));
        }
        if request.clear_binding {
            if !context.field(target, kind as i32, "binding", true) {
                return Err(Error::Denied);
            }
            let selection = self.selection(context, working, true)?;
            if selection.focus.is_some() {
                return Err(Error::Denied);
            }
            if let Some(old) = old.as_ref().and_then(|r| r.get("binding")) {
                let previous = network::groups::reference(old.as_str().ok_or(Error::Corrupt)?)?;
                selection.contains(previous, self)?;
                self.visible_asset(context, previous)?;
            }
        }
        for name in ["prefix_length", "meaning", "zone", "broadcast"] {
            if old.as_ref().and_then(|r| r.get(name)) != data.get(name)
                && !context.field(target, kind as i32, name, true)
            {
                return Err(Error::Denied);
            }
        }
        if !request.changes.is_empty() {
            self.require_changes(context, target, kind as i32, &request.changes)?;
        }
        let mut changed =
            self.put_against(&key, "networks", Class::Inventory, data, old.as_ref())?;
        for value in &request.changes {
            let key = format!("{root_key}/intent/{}", value.name);
            let prior = self.get(&key)?;
            if prior
                .as_ref()
                .is_some_and(|r| super::super::inventory::field_matches(value, r))
            {
                continue;
            }
            let mut data = super::super::inventory::field_props(value)?;
            data.extend(props([
                ("layer", text("intent")),
                ("authority", text(self.workspace_authority()?)),
            ]));
            changed |= self.put_against(&key, &root_key, Class::Inventory, data, prior.as_ref())?;
        }
        if changed {
            let revision = request
                .expected
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(Error::Limit)?;
            let mut root = old_root.clone().unwrap_or_default();
            super::super::operations::strip_envelope(&mut root);
            root.extend(target_props(target));
            root.extend(props([
                ("kind", Value::Int(kind as i32 as i64)),
                (
                    "adoption",
                    Value::Int(wire::Adoption::Adopted as i32 as i64),
                ),
                ("revision", int(revision)?),
                ("name", text(address.to_string())),
                ("name_sort", text(address.to_string())),
            ]));
            self.put_against(
                &root_key,
                "assets",
                Class::Inventory,
                root,
                old_root.as_ref(),
            )?;
            self.attach_created(context, target, kind, parent, now)?;
            self.audit(&root_key, &context.actor, "address.accepted", revision, now)?;
        }
        Ok(target)
    }
}
