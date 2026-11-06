use super::*;

pub(in crate::disk::workspace) enum PrefixCreation {
    New,
    RefreshRelated,
}

pub(in crate::disk::workspace) enum Outcome<'a> {
    Revision(&'a str),
    Csv(&'a str),
    Receipt {
        plan: &'a str,
        material: u32,
        unit: u32,
    },
}

impl Outcome<'_> {
    fn key(&self, working: &api::WorkingContext) -> Option<String> {
        match self {
            Self::Revision(_) | Self::Receipt { .. } => working
                .expected
                .map(|base| format!("mutation-outcome/{base}")),
            Self::Csv(id) => Some(format!("mutation-outcome/preview/{id}")),
        }
    }

    fn domain(&self) -> &str {
        match self {
            Self::Revision(domain) => domain,
            Self::Csv(_) => "csv",
            Self::Receipt { .. } => "physical-receipt",
        }
    }
}

pub(in crate::disk::workspace) fn normalize(request: &mut api::Address) -> Result<()> {
    parse_input(request)?;
    if request.broadcast.is_some() && request.target.is_none() && request.prefix_length.is_none() {
        return Err(Error::Invalid);
    }
    Ok(())
}

// Field syntax may use a mask supplied by another field or retained by the edited address.
pub(in crate::disk::workspace) fn parse_input(request: &mut api::Address) -> Result<()> {
    let Some(raw) = request.address.as_deref() else {
        return Ok(());
    };
    if raw.len() > 192 {
        return Err(Error::Limit);
    }
    let mut words = raw.split_ascii_whitespace();
    let literal = words.next().ok_or(Error::Invalid)?;
    let (literal, mask) = match literal.split_once('/') {
        Some((ip, mask)) => {
            if mask.is_empty() || !mask.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(Error::Invalid);
            }
            (ip, Some(mask.parse::<u32>().map_err(|_| Error::Invalid)?))
        }
        None => (literal, None),
    };
    let (literal, zone) = match literal.split_once('%') {
        Some((ip, zone)) => (ip, Some(zone)),
        None => (literal, None),
    };
    let address: IpAddr = literal.parse().map_err(|_| Error::Invalid)?;
    if let Some(mask) = mask {
        if request.clear_prefix_length || request.prefix_length.is_some_and(|v| v != mask) {
            return Err(Error::Invalid);
        }
        request.prefix_length = Some(mask);
    }
    if let Some(zone) = zone {
        if request.clear_zone || request.zone.as_deref().is_some_and(|v| v != zone) {
            return Err(Error::Invalid);
        }
        request.zone = Some(zone.into());
    }
    if let Some(token) = words.next() {
        if token != "brd" {
            return Err(Error::Invalid);
        }
        let broadcast = words.next().ok_or(Error::Invalid)?;
        if !address.is_ipv4()
            || request.clear_broadcast
            || request.broadcast.as_deref().is_some_and(|v| v != broadcast)
            || words.next().is_some()
        {
            return Err(Error::Invalid);
        }
        broadcast
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| Error::Invalid)?;
        request.broadcast = Some(broadcast.into());
    }
    validate_details(
        address,
        request.prefix_length.map(i64::from),
        request.zone.as_deref(),
    )?;
    // RFC 3021 section 2 and RFC 4291 section 2: /31 P2P and IPv6 have no directed broadcast.
    if request.broadcast.is_some()
        && (!address.is_ipv4() || request.prefix_length.is_some_and(|mask| mask >= 31))
    {
        return Err(Error::Invalid);
    }
    request.address = Some(address.to_string());
    Ok(())
}

fn fingerprint(working: &api::WorkingContext, input: &[u8], domain: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update(working.encode_to_vec());
    digest.update(input);
    digest.finalize().to_vec()
}

impl Tx<'_> {
    pub(in crate::disk::workspace) fn prepare_address_prefix(
        &mut self,
        context: &Context,
        request: &mut api::Address,
        working: &api::WorkingContext,
        now: u64,
        creation: PrefixCreation,
    ) -> Result<Option<AssetRef>> {
        if let Some(target) = request.target.as_ref()
            && !request.create_prefix
            && !request.clear_prefix
            && (request.address.is_some() || request.binding.is_some() || request.clear_binding)
        {
            let address = parse_target(Some(target))?;
            let mut prefixes = Vec::new();
            if let Some(prefix) = request.prefix.as_ref() {
                prefixes.push(parse_target(Some(prefix))?);
            } else {
                for relation in self.working_relations(address)? {
                    if relation.kind == wire::RelationKind::MemberOf as i32
                        && parse_target(relation.from.as_ref())? == address
                    {
                        if !context.field(
                            address,
                            wire::AssetKind::AddressReference as i32,
                            "relations",
                            false,
                        ) {
                            return Err(Error::Denied);
                        }
                        prefixes.push(parse_target(relation.to.as_ref())?);
                    }
                }
            }
            if !prefixes.is_empty() {
                let mut fact = self
                    .get(&format!("network/{}", network::identity(address)))?
                    .ok_or(Error::Missing)?;
                if let Some(value) = request.address.as_ref() {
                    fact.insert("address".into(), text(value));
                }
                if request.clear_binding {
                    fact.remove("binding");
                } else if let Some(value) = request.binding.as_ref() {
                    fact.insert(
                        "binding".into(),
                        text(network::identity(parse_target(Some(value))?)),
                    );
                }
                for prefix in prefixes {
                    self.validate_prefix_membership(context, address, &fact, prefix)?;
                }
            }
        }
        if request.target.is_none() {
            let root = working.focus.as_ref().or_else(|| {
                if working.roots.len() == 1 {
                    working.roots.first()
                } else {
                    None
                }
            });
            if let Some(root) = root {
                let root = parse_target(Some(root))?;
                if number(&self.visible_asset(context, root)?, "kind")?
                    == wire::AssetKind::Prefix as u64
                {
                    if request.create_prefix || request.clear_prefix {
                        return Err(Error::Conflict);
                    }
                    if request.prefix.is_none() {
                        request.prefix = Some(target_wire(root));
                    }
                }
            }
        }
        if !request.create_prefix {
            if request.prefix_name.is_some() {
                return Err(Error::Invalid);
            }
            let prefix = request
                .prefix
                .as_ref()
                .map(|t| parse_target(Some(t)))
                .transpose()?;
            if let Some(prefix) = prefix {
                self.selection(context, working, true)?
                    .contains(prefix, self)?;
            }
            return Ok(prefix);
        }
        if request.prefix.is_some() || request.clear_prefix {
            return Err(Error::Invalid);
        }
        if working.expected.is_none() {
            return Err(Error::Conflict);
        }
        if request.clear_prefix_length {
            return Err(Error::Invalid);
        }
        let retained = if request.address.is_none() || request.prefix_length.is_none() {
            let target = parse_target(request.target.as_ref())?;
            let row = self.visible_asset(context, target)?;
            if number(&row, "kind")? != wire::AssetKind::AddressReference as u64 {
                return Err(Error::Invalid);
            }
            for (field, needed) in [
                ("address", request.address.is_none()),
                ("prefix_length", request.prefix_length.is_none()),
            ] {
                if needed
                    && !context.field(
                        target,
                        wire::AssetKind::AddressReference as i32,
                        field,
                        false,
                    )
                {
                    return Err(Error::Denied);
                }
            }
            Some(
                self.get(&format!("network/{}", network::identity(target)))?
                    .ok_or(Error::Missing)?,
            )
        } else {
            None
        };
        let address: IpAddr = if let Some(address) = request.address.as_deref() {
            address.parse().map_err(|_| Error::Invalid)?
        } else {
            string(retained.as_ref().ok_or(Error::Invalid)?, "address")?
                .parse()
                .map_err(|_| Error::Corrupt)?
        };
        let mask = request
            .prefix_length
            .map(i64::from)
            .or_else(|| retained.as_ref()?.get("prefix_length")?.as_int())
            .ok_or(Error::Invalid)?;
        let block = ipnet::IpNet::new(address, u8::try_from(mask).map_err(|_| Error::Invalid)?)
            .map_err(|_| Error::Invalid)?
            .trunc();
        if let Some(target) = request.target.as_ref()
            && matches!(creation, PrefixCreation::RefreshRelated)
        {
            for relation in self.working_relations(parse_target(Some(target))?)? {
                if relation.kind != wire::RelationKind::MemberOf as i32
                    || relation.from.as_ref() != Some(target)
                {
                    continue;
                }
                let prefix = parse_target(relation.to.as_ref())?;
                if number(&self.visible_asset(context, prefix)?, "kind")?
                    != wire::AssetKind::Prefix as u64
                {
                    continue;
                }
                if !context.field(prefix, wire::AssetKind::Prefix as i32, "cidr", false) {
                    return Err(Error::Denied);
                }
                let row = self
                    .get(&format!("network/{}", network::identity(prefix)))?
                    .ok_or(Error::Missing)?;
                if row.get("cidr") == Some(&text(block.to_string())) {
                    return Ok(Some(prefix));
                }
            }
        }
        let store = self.store;
        let targets = store.workspace_network_in(
            self,
            context,
            api::Network {
                kind: wire::AssetKind::Prefix as i32,
                cidr: block.to_string(),
                name: request.prefix_name.take().unwrap_or_default(),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            },
            &api::WorkingContext {
                expected: working.expected,
                ..Default::default()
            },
            now,
            Vec::new(),
        )?;
        Ok(Some(parse_target(targets.first())?))
    }

    pub(in crate::disk::workspace) fn creation_outcome(
        &self,
        context: &Context,
        working: &api::WorkingContext,
        input: &[u8],
        outcome: Outcome<'_>,
    ) -> Result<Option<api::Mutation>> {
        let Some(key) = outcome.key(working) else {
            return Ok(None);
        };
        let Some(row) = self.get(&key)? else {
            return Ok(None);
        };
        if string(&row, "actor")? != context.actor
            || binary(&row, "digest")? != fingerprint(working, input, outcome.domain())
        {
            return Err(Error::Conflict);
        }
        let Some(Value::Array(values)) = row.get("targets") else {
            return Err(Error::Corrupt);
        };
        if values.len() > MAX_IMPORT {
            return Err(Error::Corrupt);
        }
        let mut selector = if matches!(outcome, Outcome::Receipt { .. }) {
            api::WorkingContext::default()
        } else {
            working.clone()
        };
        selector.expected = None;
        let selection = self.selection(context, &selector, false)?;
        let mut targets = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let target = network::groups::reference(value.as_str().ok_or(Error::Corrupt)?)?;
            self.visible_asset(context, target)?;
            if index == 0 {
                selection.contains(target, self)?;
            }
            targets.push(target_wire(target));
        }
        Ok(Some(api::Mutation {
            revision: match outcome {
                Outcome::Revision(_) | Outcome::Receipt { .. } => working
                    .expected
                    .ok_or(Error::Corrupt)?
                    .checked_add(1)
                    .ok_or(Error::Limit)?,
                Outcome::Csv(_) => number(&row, "revision")?,
            },
            targets,
            affected: 0,
        }))
    }

    pub(in crate::disk::workspace) fn save_creation_outcome(
        &mut self,
        context: &Context,
        working: &api::WorkingContext,
        input: &[u8],
        targets: &[wire::AssetRef],
        now: u64,
        outcome: Outcome<'_>,
    ) -> Result<()> {
        let Some(key) = outcome.key(working) else {
            return Ok(());
        };
        let base = number(&self.root, "revision")?;
        if working.expected.is_some_and(|expected| expected != base) {
            return Err(Error::Conflict);
        }
        let mut data = props([
            ("actor", text(&context.actor)),
            ("at", int(now)?),
            (
                "digest",
                Value::Binary(fingerprint(working, input, outcome.domain())),
            ),
            (
                "targets",
                Value::Array(
                    targets
                        .iter()
                        .map(|t| parse_target(Some(t)).map(|t| text(network::identity(t))))
                        .collect::<Result<_>>()?,
                ),
            ),
        ]);
        if matches!(outcome, Outcome::Csv(_)) {
            data.insert(
                "revision".into(),
                int(base.checked_add(1).ok_or(Error::Limit)?)?,
            );
        }
        if let Outcome::Receipt {
            plan,
            material,
            unit,
        } = outcome
        {
            data.insert("action".into(), text("physical-receipt"));
            data.insert("owner".into(), text(plan));
            data.insert("count".into(), int(u64::from(material))?);
            data.insert("generation".into(), int(u64::from(unit))?);
        }
        // Expiry leaves the old creation revision or consumed preview inadmissible.
        self.put(&key, "mutation-outcomes", Class::Detail, data)?;
        Ok(())
    }

    pub(in crate::disk::workspace) fn set_address_prefix(
        &mut self,
        context: &Context,
        address: AssetRef,
        prefix: Option<AssetRef>,
        clear: bool,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        if !clear && prefix.is_none() {
            return Ok(());
        }
        if clear && prefix.is_some() {
            return Err(Error::Invalid);
        }
        if working.expected.is_none() {
            return Err(Error::Conflict);
        }
        let relations = self.working_relations(address)?;
        let address_wire = target_wire(address);
        if relations
            .iter()
            .filter(|relation| {
                relation.kind == wire::RelationKind::MemberOf as i32
                    && relation.from.as_ref() == Some(&address_wire)
            })
            .count()
            > 1
        {
            return Err(Error::Conflict);
        }
        let authority = self.workspace_authority()?;
        for relation in &relations {
            if relation.kind != wire::RelationKind::MemberOf as i32
                || parse_target(relation.from.as_ref())? != address
            {
                continue;
            }
            let previous = parse_target(relation.to.as_ref())?;
            if !context.field(
                address,
                wire::AssetKind::AddressReference as i32,
                "relations",
                false,
            ) {
                return Err(Error::Denied);
            }
            let row = self.visible_asset(context, previous)?;
            if number(&row, "kind")? != wire::AssetKind::Prefix as u64 {
                continue;
            }
            for (target, kind) in [
                (address, wire::AssetKind::AddressReference),
                (previous, wire::AssetKind::Prefix),
            ] {
                if !context.field(target, kind as i32, "relations", true) {
                    return Err(Error::Denied);
                }
            }
            if relation.authority_id != authority {
                return Err(Error::Conflict);
            }
            if prefix == Some(previous) {
                return Ok(());
            }
            self.remove_binding_relation(relation, &context.actor, now)?;
        }
        let Some(prefix) = prefix else {
            return Ok(());
        };
        let row = self.visible_asset(context, prefix)?;
        if number(&row, "kind")? != wire::AssetKind::Prefix as u64
            || !context.field(prefix, wire::AssetKind::Prefix as i32, "cidr", false)
            || !context.field(prefix, wire::AssetKind::Prefix as i32, "relations", true)
            || !context.field(
                address,
                wire::AssetKind::AddressReference as i32,
                "relations",
                true,
            )
        {
            return Err(Error::Denied);
        }
        let fact = self
            .get(&format!("network/{}", network::identity(address)))?
            .ok_or(Error::Missing)?;
        self.validate_prefix_membership(context, address, &fact, prefix)?;
        let relation = wire::Relation {
            relation_id: RelationId::generate().to_string(),
            from: Some(target_wire(address)),
            to: Some(target_wire(prefix)),
            kind: wire::RelationKind::MemberOf as i32,
            revision: 1,
            authority_id: authority,
        };
        self.save_typed_relation(&relation, None, &context.actor, now)?;
        Ok(())
    }

    fn validate_prefix_membership(
        &self,
        context: &Context,
        address: AssetRef,
        fact: &Properties,
        prefix: AssetRef,
    ) -> Result<()> {
        let row = self.visible_asset(context, prefix)?;
        if number(&row, "kind")? != wire::AssetKind::Prefix as u64 {
            return Err(Error::Invalid);
        }
        let block = self
            .get(&format!("network/{}", network::identity(prefix)))?
            .ok_or(Error::Missing)?;
        self.check_prefix_fact(context, address, fact, &block)
    }
}
