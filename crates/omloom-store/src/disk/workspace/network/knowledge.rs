use super::*;

impl Tx<'_> {
    pub(in crate::disk::workspace) fn check_prefix_members(
        &self,
        context: &Context,
        prefix: AssetRef,
        proposed: &Properties,
    ) -> Result<()> {
        let mut after = String::new();
        loop {
            let members = self.query(
                "MATCH (r:OmRow {to_asset:$prefix}) WHERE r.parent='relations' AND r.kind=$kind AND r.to_incarnation=$incarnation AND r.key>$after RETURN properties(r) AS p ORDER BY r.key LIMIT 129",
                params([
                    ("prefix", text(prefix.asset.to_string())),
                    ("incarnation", text(prefix.incarnation.to_string())),
                    ("kind", int(wire::RelationKind::MemberOf as u64)?),
                    ("after", text(&after)),
                ]),
            )?;
            let count = members.len().min(128);
            if count == 0 {
                return Ok(());
            }
            let mut keys = Vec::with_capacity(count * 2);
            for member in &members[..count] {
                let address = crate::disk::target(member)?;
                keys.push(text(asset_key(address)));
                keys.push(text(format!("network/{}", identity(address))));
            }
            let rows = self.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
                params([("keys", Value::Array(keys))]),
            )?;
            let by_key = rows
                .iter()
                .map(|row| {
                    let key = row
                        .get("key")
                        .and_then(Value::as_str)
                        .ok_or(Error::Corrupt)?;
                    Ok((key, row))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            for member in &members[..count] {
                let address = crate::disk::target(member)?;
                let row = by_key
                    .get(asset_key(address).as_str())
                    .ok_or(Error::Missing)?;
                self.require_visible_asset(context, address, row)?;
                if number(row, "kind")? != wire::AssetKind::AddressReference as u64 {
                    continue;
                }
                let key = format!("network/{}", identity(address));
                let fact = by_key.get(key.as_str()).ok_or(Error::Missing)?;
                if !context.field(prefix, wire::AssetKind::Prefix as i32, "relations", false)
                    || !context.field(
                        address,
                        wire::AssetKind::AddressReference as i32,
                        "relations",
                        false,
                    )
                {
                    return Err(Error::Denied);
                }
                self.check_prefix_fact(context, address, fact, proposed)?;
            }
            if members.len() == count {
                return Ok(());
            }
            after = string(&members[count - 1], "key")?;
        }
    }

    pub(in crate::disk::workspace) fn check_prefix_fact(
        &self,
        context: &Context,
        address: AssetRef,
        fact: &Properties,
        prefix: &Properties,
    ) -> Result<()> {
        if !context.field(
            crate::disk::target(prefix)?,
            wire::AssetKind::Prefix as i32,
            "cidr",
            false,
        ) || !context.field(
            address,
            wire::AssetKind::AddressReference as i32,
            "address",
            false,
        ) {
            return Err(Error::Denied);
        }
        self.check_prefix_binding(context, address, fact, prefix)?;
        crate::disk::inventory::prefix_fact_matches(fact, prefix)
    }

    pub(in crate::disk::workspace) fn check_prefix_binding(
        &self,
        context: &Context,
        address: AssetRef,
        fact: &Properties,
        prefix: &Properties,
    ) -> Result<()> {
        if !prefix.contains_key("address_space") {
            return Ok(());
        }
        let Some(binding) = fact.get("binding") else {
            return Ok(());
        };
        if !context.field(
            crate::disk::target(prefix)?,
            wire::AssetKind::Prefix as i32,
            "address_space",
            false,
        ) {
            return Err(Error::Denied);
        }
        if !context.field(
            address,
            wire::AssetKind::AddressReference as i32,
            "binding",
            false,
        ) {
            return Err(Error::Denied);
        }
        let binding = groups::reference(binding.as_str().ok_or(Error::Corrupt)?)?;
        self.visible_asset(context, binding)?;
        if !context.field(
            binding,
            wire::AssetKind::IpAddress as i32,
            "address_space",
            false,
        ) {
            return Err(Error::Denied);
        }
        let allocation = self
            .get(&format!("network/{}", identity(binding)))?
            .ok_or(Error::Missing)?;
        let ip: std::net::IpAddr = string(fact, "address")?
            .parse()
            .map_err(|_| Error::Corrupt)?;
        if link_local(ip) {
            if !context.field(
                crate::disk::target(prefix)?,
                wire::AssetKind::Prefix as i32,
                "link_scope",
                false,
            ) || !context.field(
                binding,
                wire::AssetKind::IpAddress as i32,
                "link_scope",
                false,
            ) {
                return Err(Error::Denied);
            }
        }
        crate::disk::inventory::prefix_allocation_matches(&allocation, prefix, ip)
    }

    pub(in crate::disk::workspace) fn prefix_write(
        &self,
        context: &Context,
        write: api::Write,
    ) -> Result<(api::Network, Vec<wire::Field>)> {
        if write.changes.is_empty() || write.changes.len() > MAX_FIELDS {
            return Err(Error::Invalid);
        }
        {
            let mut seen = BTreeSet::new();
            for field in &write.changes {
                if !seen.insert(&field.name) {
                    return Err(Error::Invalid);
                }
            }
        }
        let mut changes = Vec::new();
        let mut result = api::Network {
            target: write.target,
            expected: write.expected,
            kind: write.kind,
            parent: write.parent.map(Box::new),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        };
        if let Some(target) = &result.target {
            let target_ref = parse_target(Some(target))?;
            self.visible_asset(context, target_ref)?;
            let row = self
                .get(&format!(
                    "network/{}",
                    identity(parse_target(Some(target))?)
                ))?
                .ok_or(Error::Missing)?;
            for field in ["cidr", "address_space", "link_scope", "routing_context"] {
                if row.contains_key(field)
                    && !context.field(target_ref, wire::AssetKind::Prefix as i32, field, false)
                {
                    return Err(Error::Denied);
                }
            }
            result.cidr = string(&row, "cidr")?;
            result.address_space = row
                .get("address_space")
                .map(|v| groups::reference(v.as_str().ok_or(Error::Corrupt)?).map(target_wire))
                .transpose()?;
            result.link = row
                .get("link_scope")
                .map(|v| groups::reference(v.as_str().ok_or(Error::Corrupt)?).map(target_wire))
                .transpose()?;
            result.routing = row
                .get("routing_context")
                .map(|v| groups::reference(v.as_str().ok_or(Error::Corrupt)?).map(target_wire))
                .transpose()?;
        }
        for field in write.changes {
            if !matches!(field.name.as_str(), "cidr" | "name" | "address_space") {
                if crate::disk::workspace::assets::network_column(&field.name) {
                    return Err(Error::Invalid);
                }
                changes.push(field);
                continue;
            }
            match (field.name.as_str(), field.value) {
                ("cidr", Some(field::Value::Text(value))) => result.cidr = value,
                ("name", Some(field::Value::Text(value))) => result.name = value,
                ("name", Some(field::Value::Clear(EXPLICIT_CLEAR))) => result.clear_name = true,
                ("address_space", Some(field::Value::Text(value))) => {
                    result.address_space = Some(target_wire(groups::reference(&value)?))
                }
                _ => return Err(Error::Invalid),
            }
        }
        if result.cidr.len() > 64 || result.cidr.parse::<IpNet>().is_err() {
            return Err(Error::Invalid);
        }
        Ok((result, changes))
    }

    pub(super) fn workspace_unresolved_prefix(
        &mut self,
        context: &Context,
        request: api::Network,
        working: &api::WorkingContext,
        now: u64,
        mut changes: Vec<wire::Field>,
    ) -> Result<Vec<wire::AssetRef>> {
        if request.next_available
            || request.link.is_some()
            || request.routing.is_some()
            || request.group.is_some()
            || !request.group_name.is_empty()
            || request.expected_group.is_some()
            || request.previous_group_revision.is_some()
            || request.vrid.is_some()
            || request.sharing != api::SharedUse::Exclusive as i32
            || request.cidr.len() > 64
            || request.name.len() > 160
        {
            return Err(Error::Invalid);
        }
        let block: IpNet = request.cidr.parse().map_err(|_| Error::Invalid)?;
        self.check_network_context(context, working, &request)?;
        changes.extend(prefix_name_changes(
            request.name.trim().into(),
            request.clear_name,
        ));
        let target = self.workspace_network_write(
            context,
            api::Write {
                target: request.target.clone(),
                expected: request.expected,
                kind: request.kind,
                changes,
                parent: None,
            },
            now,
        )?;
        if !context.field(target, request.kind, "network", true) {
            return Err(Error::Denied);
        }
        let key = format!("network/{}", identity(target));
        let old = self.get(&key)?;
        // Omitting a namespace cannot demote an existing canonical allocation.
        if old
            .as_ref()
            .is_some_and(|p| p.contains_key("address_space"))
        {
            return Err(Error::Conflict);
        }
        let mut data = target_props(target);
        data.extend(props([
            ("cidr", text(block.trunc().to_string())),
            ("ip_order", text(address_order(block.network()))),
            ("prefix_len", int(u64::from(block.prefix_len()))?),
        ]));
        if old
            .as_ref()
            .is_some_and(|old| old.get("cidr") != data.get("cidr"))
        {
            self.check_prefix_members(context, target, &data)?;
        }
        if self.put_against(&key, "networks", Class::Inventory, data, old.as_ref())? {
            self.workspace_advance(target, &context.actor, "prefix.accepted", now)?;
        }
        Ok(vec![target_wire(target)])
    }
}
