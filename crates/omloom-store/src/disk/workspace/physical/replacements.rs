use super::*;

fn claim_endpoint(
    set: &mut BTreeSet<(AssetRef, Option<u32>)>,
    port: AssetRef,
    channel: Option<u32>,
) -> Result<()> {
    if channel.is_none() {
        if set
            .range((port, None)..=(port, Some(4096)))
            .next()
            .is_some()
        {
            return Err(Error::Conflict);
        }
    } else if set.contains(&(port, None)) {
        return Err(Error::Conflict);
    }
    if !set.insert((port, channel)) {
        return Err(Error::Conflict);
    }
    Ok(())
}

pub(super) fn replacement_row(value: &api::PhysicalReplacement) -> Result<Properties> {
    let mut row = props([(
        "enclosure",
        text(network::identity(parse_target(value.position.as_ref())?)),
    )]);
    if let Some(item) = &value.previous {
        row.insert(
            "old_item".into(),
            text(network::identity(parse_target(Some(item))?)),
        );
    }
    if let Some(item) = &value.replacement {
        row.insert(
            "item".into(),
            text(network::identity(parse_target(Some(item))?)),
        );
    }
    if let Some(position) = &value.source_position {
        row.insert(
            "source".into(),
            text(network::identity(parse_target(Some(position))?)),
        );
    }
    if let Some(binding) = &value.previous_binding {
        row.insert("claim".into(), text(binding));
    }
    if let Some(revision) = value.previous_revision {
        row.insert("expected".into(), int(revision)?);
    }
    if let Some(volume) = &value.volume {
        write_volume(&mut row, volume)?;
    }
    if !value.endpoints.is_empty() {
        if value.endpoints.len() > 64 {
            return Err(Error::Limit);
        }
        let mappings = api::PhysicalEndpointMappings {
            pairs: value.endpoints.clone(),
        };
        if mappings.pairs.len() > 64 || mappings.encoded_len() > 16_384 {
            return Err(Error::Limit);
        }
        // One bounded review consumes every pair together; none is indexed or
        // duplicated into the authoritative wiring or endpoint inventory.
        row.insert("payload".into(), Value::Binary(mappings.encode_to_vec()));
    }
    if let Some(feedback) = &value.feedback {
        if feedback.encoded_len() > 512 {
            return Err(Error::Limit);
        }
        row.insert("working".into(), Value::Binary(feedback.encode_to_vec()));
    }
    Ok(row)
}
pub(super) fn read_replacement(row: &Properties) -> Result<api::PhysicalReplacement> {
    let reference = |name: &str| -> Result<Option<wire::AssetRef>> {
        row.get(name)
            .map(|_| {
                string(row, name)
                    .and_then(|s| lifecycle::reference(&s))
                    .map(target_wire)
            })
            .transpose()
    };
    Ok(api::PhysicalReplacement {
        position: Some(target_wire(lifecycle::reference(&string(
            row,
            "enclosure",
        )?)?)),
        previous: reference("old_item")?,
        replacement: reference("item")?,
        source_position: reference("source")?,
        volume: read_volume(row)?,
        previous_binding: row.get("claim").map(|_| string(row, "claim")).transpose()?,
        previous_revision: row
            .get("expected")
            .map(|_| number(row, "expected"))
            .transpose()?,
        endpoints: if row.contains_key("payload") {
            let bytes = binary(row, "payload")?;
            if bytes.len() > 16_384 {
                return Err(Error::Corrupt);
            }
            let mappings =
                api::PhysicalEndpointMappings::decode(bytes).map_err(|_| Error::Corrupt)?;
            if mappings.pairs.len() > 64 {
                return Err(Error::Corrupt);
            }
            mappings.pairs
        } else {
            Vec::new()
        },
        feedback: row
            .get("working")
            .map(|_| -> Result<Box<api::PhysicalReplacementFeedback>> {
                let bytes = binary(row, "working")?;
                if bytes.len() > 512 {
                    return Err(Error::Corrupt);
                }
                api::PhysicalReplacementFeedback::decode(bytes)
                    .map(Box::new)
                    .map_err(|_| Error::Corrupt)
            })
            .transpose()?,
    })
}
impl Tx<'_> {
    pub(super) fn require_replacement_space<T: core::borrow::Borrow<str> + Ord>(
        &self,
        replacement: &api::PhysicalReplacement,
        removed: &BTreeSet<T>,
        destination_kind: wire::AssetKind,
    ) -> Result<()> {
        let position = parse_target(replacement.position.as_ref())?;
        if destination_kind == wire::AssetKind::PhysicalPosition {
            let rows=self.query("MATCH (p:OmRow {key:$key})<-[:OmRelationTo]-(r:OmRow) WHERE r.kind=$kind RETURN properties(r) AS p LIMIT 2",params([("key",text(asset_key(position))),("kind",int(wire::RelationKind::InstalledIn as u64)?)]))?;
            if rows.len() > 1 {
                return Err(Error::Conflict);
            }
            for row in rows {
                let binding = inventory::relation_projection(&row)?;
                if !removed.contains(binding.relation_id.as_str())
                    && binding.from != replacement.replacement
                {
                    return Err(Error::Conflict);
                }
            }
        }
        if let Some(incoming) = &replacement.replacement {
            for binding in self.installed_bindings(parse_target(Some(incoming))?)? {
                if !removed.contains(binding.relation_id.as_str())
                    && binding.to != replacement.position
                {
                    return Err(Error::Conflict);
                }
            }
        }
        Ok(())
    }

    pub(super) fn record_physical_replacements(
        &mut self,
        context: &Context,
        replacements: &[&api::PhysicalReplacement],
        report: &api::PhysicalCompletion,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        let plan = report.plan.as_ref().ok_or(Error::Invalid)?.id.as_str();
        let selection = self.selection(context, working, true)?;
        let mut old = Vec::with_capacity(replacements.len());
        let mut removed = BTreeSet::new();
        for replacement in replacements {
            self.replacement_access(context, replacement, working, true)?;
            for part in [
                replacement.previous.as_ref(),
                replacement.replacement.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                let part = parse_target(Some(part))?;
                self.require_no_pending_operations(part)?;
                if let Some(lock) =
                    self.get(&format!("physical-lock/{}", network::identity(part)))?
                    && string(&lock, "owner")? != plan
                {
                    return Err(Error::Conflict);
                }
            }
            if let Some(binding) = self.replacement_basis(replacement)? {
                self.check_relationship(context, &selection, &binding)?;
                if !removed.insert(binding.relation_id.clone()) {
                    return Err(Error::Invalid);
                }
                old.push(binding);
            }
        }
        let mut incoming_positions = BTreeSet::new();
        for replacement in replacements {
            let position = parse_target(replacement.position.as_ref())?;
            if replacement.replacement.is_some() {
                incoming_positions.insert(position);
            }
            let kind = wire::AssetKind::try_from(
                i32::try_from(number(&self.asset(position)?, "kind")?)
                    .map_err(|_| Error::Corrupt)?,
            )
            .map_err(|_| Error::Corrupt)?;
            self.require_replacement_space(replacement, &removed, kind)?;
        }
        // Detach every selected old binding before attaching incoming parts, including swaps.
        for binding in &old {
            self.remove_binding_relation(binding, &context.actor, now)?;
        }
        for replacement in replacements {
            let position = parse_target(replacement.position.as_ref())?;
            let Some(incoming) = &replacement.replacement else {
                continue;
            };
            let item = parse_target(Some(incoming))?;
            let existing = self.installed_bindings(item)?;
            let binding = if existing.is_empty() {
                let binding = wire::Relation {
                    relation_id: RelationId::generate().to_string(),
                    from: Some(target_wire(item)),
                    to: Some(target_wire(position)),
                    kind: wire::RelationKind::InstalledIn as i32,
                    revision: 1,
                    authority_id: self.workspace_authority()?,
                };
                self.check_relationship(context, &selection, &binding)?;
                self.save_physical_relation(
                    &binding,
                    &context.actor,
                    now,
                    plan,
                    report.actual_state,
                )?;
                binding
            } else {
                existing.into_iter().next().ok_or(Error::Corrupt)?
            };
            if let Some(volume) = &replacement.volume {
                self.store_physical_pose(&binding, volume, &context.actor)?;
            }
        }
        let mut touched = BTreeSet::new();
        for binding in &old {
            touched.insert(parse_target(binding.to.as_ref())?);
        }
        for position in touched {
            // An incoming binding already clears a prior empty report atomically.
            if incoming_positions.contains(&position) {
                continue;
            }
            if number(&self.asset(position)?, "kind")? != wire::AssetKind::PhysicalPosition as u64 {
                continue;
            }
            let key = format!("position/{}", network::identity(position));
            let mut row = self.get(&key)?.ok_or(Error::Corrupt)?;
            let parent = string(&row, "parent")?;
            operations::strip_envelope(&mut row);
            row.remove("enclosure");
            row.insert("vflag".into(), Value::Bool(true));
            row.insert("actor".into(), text(&context.actor));
            row.insert(
                "revision".into(),
                int(number(&self.root, "revision")?
                    .checked_add(1)
                    .ok_or(Error::Limit)?)?,
            );
            self.put(&key, &parent, Class::Inventory, row)?;
        }
        Ok(())
    }
    pub(super) fn replacement_access(
        &self,
        context: &Context,
        value: &api::PhysicalReplacement,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<()> {
        for reference in [
            value.position.as_ref(),
            value.source_position.as_ref(),
            value.previous.as_ref(),
            value.replacement.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let target = parse_target(Some(reference))?;
            self.physical_access(context, target, working, write)?;
            let kind =
                i32::try_from(number(&self.asset(target)?, "kind")?).map_err(|_| Error::Corrupt)?;
            if !context.field(target, kind, "relations", false)
                || (write && !context.field(target, kind, "relations", true))
            {
                return Err(Error::Denied);
            }
        }
        self.endpoint_mapping_access(context, value, working, write)?;
        Ok(())
    }
    fn endpoint_mapping_access(
        &self,
        context: &Context,
        value: &api::PhysicalReplacement,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<()> {
        if value.endpoints.len() > 64 {
            return Err(Error::Limit);
        }
        let ports = self.mapping_ports(context, value, working, write)?;
        let mut sources = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        for pair in &value.endpoints {
            let source = parse_target(pair.previous.as_ref())?;
            for channel in [pair.previous_channel, pair.replacement_channel]
                .into_iter()
                .flatten()
            {
                if channel == 0 || channel > 4096 {
                    return Err(Error::Invalid);
                }
            }
            claim_endpoint(&mut sources, source, pair.previous_channel)?;
            match api::PhysicalMappingDecision::try_from(pair.decision)
                .map_err(|_| Error::Invalid)?
            {
                api::PhysicalMappingDecision::Unresolved
                    if pair.replacement.is_none() && pair.replacement_channel.is_none() => {}
                api::PhysicalMappingDecision::Accepted | api::PhysicalMappingDecision::Rejected
                    if pair.replacement.is_some() => {}
                _ => return Err(Error::Invalid),
            }
            if let Some(destination) = &pair.replacement {
                let destination = parse_target(Some(destination))?;
                if pair.decision == api::PhysicalMappingDecision::Accepted as i32 {
                    claim_endpoint(&mut destinations, destination, pair.replacement_channel)?;
                    for (port, channel) in [
                        (source, pair.previous_channel),
                        (destination, pair.replacement_channel),
                    ] {
                        if let Some(channel) = channel {
                            let declared = ports.get(&port).is_some_and(|d| d.contains(&channel));
                            if !declared {
                                return Err(Error::Incomplete);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn mapping_ports(
        &self,
        context: &Context,
        value: &api::PhysicalReplacement,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<BTreeMap<AssetRef, Vec<u32>>> {
        let mut owners: BTreeMap<AssetRef, BTreeSet<AssetRef>> = BTreeMap::new();
        let mut ports = BTreeSet::new();
        let mut channels = BTreeSet::new();
        for pair in &value.endpoints {
            for (owner, port, channel) in [
                (
                    value.previous.as_ref(),
                    pair.previous.as_ref(),
                    pair.previous_channel,
                ),
                (
                    value.replacement.as_ref(),
                    pair.replacement.as_ref(),
                    pair.replacement_channel,
                ),
            ] {
                if let Some(port) = port {
                    let port = parse_target(Some(port))?;
                    let owner = parse_target(owner)?;
                    owners.entry(owner).or_default().insert(port);
                    ports.insert(port);
                    if channel.is_some() {
                        channels.insert(port);
                    }
                }
            }
        }
        if ports.is_empty() {
            return Ok(BTreeMap::new());
        }
        let selection = self.selection(context, working, write)?;
        let keys = ports.iter().map(|p| text(asset_key(*p))).collect();
        let mut values = HashMap::from([("keys".into(), Value::Array(keys))]);
        let visible = assets::visible_clause_for(context, Some("physical"), &mut values, "a");
        let rows=self.query(&format!("UNWIND $keys AS key MATCH (a:OmRow {{key:key}}) WHERE {visible} RETURN {{asset:a.asset,incarnation:a.incarnation,kind:a.kind,adoption:a.adoption}} AS p LIMIT 128"),Some(values))?;
        if rows.len() != ports.len() {
            return Err(Error::Denied);
        }
        for row in rows {
            let target = target(&row)?;
            if !ports.contains(&target) {
                return Err(Error::Conflict);
            }
            selection.contains(target, self)?;
            if Self::physical_metadata_access(context, target, &row, write)?
                != wire::AssetKind::Port
            {
                return Err(Error::Invalid);
            }
            if !context.field(target, wire::AssetKind::Port as i32, "relations", false) {
                return Err(Error::Denied);
            }
        }
        // At most two actual suppliers, each with indexed endpoint probes in one
        // statement. Check exact incarnations before accepting any supplied port.
        for (owner, expected) in owners {
            let rows=self.query("UNWIND $ports AS port MATCH (r:OmRow {to_asset:port}) WHERE r.parent='relations' AND r.kind=$kind AND r.asset=$owner AND r.incarnation=$incarnation AND r.to_incarnation IN $incarnations RETURN {asset:r.to_asset,incarnation:r.to_incarnation} AS p LIMIT 129",params([
                ("ports",Value::Array(expected.iter().map(|p|text(p.asset.to_string())).collect())),
                ("incarnations",Value::Array(expected.iter().map(|p|text(p.incarnation.to_string())).collect())),
                ("kind",int(wire::RelationKind::HasPort as u64)?),("owner",text(owner.asset.to_string())),("incarnation",text(owner.incarnation.to_string())),
            ]))?;
            if rows.len() > 128 {
                return Err(Error::Limit);
            }
            let mut found = BTreeSet::new();
            for row in rows {
                let target = target(&row)?;
                if expected.contains(&target) {
                    found.insert(target);
                }
            }
            if found != expected {
                return Err(Error::Invalid);
            }
        }
        let mut result = BTreeMap::new();
        if !channels.is_empty() {
            let keys = channels
                .iter()
                .map(|p| text(format!("physical/{}", network::identity(*p))))
                .collect();
            for row in self.query(
                "UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p LIMIT 128",
                params([("keys", Value::Array(keys))]),
            )? {
                let definition = definition(&row)?;
                result.insert(
                    parse_target(definition.target.as_ref())?,
                    definition.channels,
                );
            }
        }
        Ok(result)
    }
    pub(super) fn check_replacements(
        &self,
        context: &Context,
        plan: &api::PhysicalPlan,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<()> {
        let mut positions = BTreeSet::new();
        let mut parts = BTreeSet::new();
        let mut bindings = BTreeSet::new();
        let mut mapping_count = 0usize;
        for replacement in replacement_steps(plan) {
            if replacement.endpoints.len() > 64 - mapping_count {
                return Err(Error::Limit);
            }
            mapping_count += replacement.endpoints.len();
            self.replacement_access(context, replacement, working, write)?;
            let position = parse_target(replacement.position.as_ref())?;
            if replacement.previous.is_none() && replacement.replacement.is_none() {
                return Err(Error::Invalid);
            }
            for item in [
                replacement.previous.as_ref(),
                replacement.replacement.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                let item = parse_target(Some(item))?;
                let kind = wire::AssetKind::try_from(number(&self.asset(item)?, "kind")? as i32)
                    .map_err(|_| Error::Corrupt)?;
                if item == position
                    || !matches!(
                        kind,
                        wire::AssetKind::PhysicalComponent | wire::AssetKind::Device
                    )
                {
                    return Err(Error::Invalid);
                }
            }
            if let Some(item) = &replacement.replacement
                && !parts.insert(parse_target(Some(item))?)
            {
                return Err(Error::Invalid);
            }
            let position_kind =
                wire::AssetKind::try_from(number(&self.asset(position)?, "kind")? as i32)
                    .map_err(|_| Error::Corrupt)?;
            if !matches!(
                position_kind,
                wire::AssetKind::PhysicalPosition
                    | wire::AssetKind::Rack
                    | wire::AssetKind::Location
                    | wire::AssetKind::Site
            ) {
                return Err(Error::Invalid);
            }
            if position_kind == wire::AssetKind::PhysicalPosition && !positions.insert(position) {
                return Err(Error::Invalid);
            }
            match (&replacement.previous_binding, replacement.previous_revision) {
                (Some(id), Some(revision)) if replacement.previous.is_some() && revision != 0 => {
                    if !bindings.insert(RelationId::parse(id).map_err(|_| Error::Invalid)?) {
                        return Err(Error::Invalid);
                    }
                }
                (None, None) if replacement.previous.is_none() => {}
                _ => return Err(Error::Invalid),
            }
            if replacement.source_position.is_some() && replacement.previous.is_none() {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub(super) fn replacement_basis(
        &self,
        replacement: &api::PhysicalReplacement,
    ) -> Result<Option<wire::Relation>> {
        let Some(previous) = replacement.previous.as_ref() else {
            return Ok(None);
        };
        let id = RelationId::parse(
            replacement
                .previous_binding
                .as_ref()
                .ok_or(Error::Incomplete)?,
        )
        .map_err(|_| Error::Invalid)?;
        let row = self
            .get(&format!("relation/{id}"))?
            .ok_or(Error::Conflict)?;
        let relation = inventory::relation_projection(&row)?;
        let source = replacement
            .source_position
            .as_ref()
            .or(replacement.position.as_ref());
        if relation.from.as_ref() != Some(previous)
            || relation.to.as_ref() != source
            || Some(relation.revision) != replacement.previous_revision
            || relation.kind != wire::RelationKind::InstalledIn as i32
        {
            return Err(Error::Conflict);
        }
        Ok(Some(relation))
    }
}
