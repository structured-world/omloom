//! Reviewed corrections reuse inventory validation, transaction CAS, audit and
//! finite review/detail custody. No operation or account authority is retargeted.
use super::*;
mod sources;
mod undo;

fn pair(value: &api::IdentityMapping) -> Result<(AssetRef, AssetRef)> {
    Ok((
        parse_target(value.duplicate.as_ref())?,
        parse_target(value.retained.as_ref())?,
    ))
}
fn strip(mut value: Properties) -> Properties {
    operations::strip_envelope(&mut value);
    value
}
impl Tx<'_> {
    fn correction_guard(
        &self,
        context: &Context,
        target: AssetRef,
        working: &api::WorkingContext,
    ) -> Result<Properties> {
        let row = self.visible_asset(context, target)?;
        let kind = number(&row, "kind")? as i32;
        if !context.field(target, kind, "relations", false) {
            return Err(Error::Denied);
        }
        self.selection(context, working, false)?
            .contains(target, self)?;
        for field in ["identity_correction", "relations", "remove"] {
            if !context.field(target, kind, field, true) {
                return Err(Error::Denied);
            }
        }
        self.require_no_account_references(target)?;
        self.require_no_ipam_state(target)?;
        self.require_no_pending_operations(target)?;
        Ok(row)
    }
    fn correction_fields(
        &mut self,
        context: &Context,
        mapping: &api::IdentityMapping,
        kind: i32,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        let (from, to) = pair(mapping)?;
        let rows = self.fact_rows(&asset_key(from), MAX_FIELDS + 5)?;
        let choices: BTreeMap<_, _> = mapping
            .fields
            .iter()
            .map(|v| (v.name.as_str(), v.use_duplicate))
            .collect();
        if choices.len() != mapping.fields.len() || choices.len() > MAX_FIELDS {
            return Err(Error::Invalid);
        }
        let mut used = BTreeSet::new();
        let mut changes = Vec::new();
        for row in rows {
            if row.get("layer") == Some(&text("intent")) {
                let name = string(&row, "field")?;
                let take = choices.get(name.as_str()).ok_or(Error::Conflict)?;
                for target in [from, to] {
                    if !context.field(target, kind, &name, false)
                        || ((target == from || *take) && !context.field(target, kind, &name, true))
                    {
                        return Err(Error::Denied);
                    }
                }
                used.insert(name);
                if *take {
                    changes.push(inventory::read_field(&row)?);
                }
            } else if row.contains_key("facet") {
                // Declarations are not transferred. An inherited binding is
                // recomputed through its changed relation; explicit conflicts
                // must first be resolved in the responsibility editor.
                if number(&row, "facet")? != api::ResponsibilityFacet::Creator as u64
                    && number(&row, "mode")? != api::AssignmentMode::Inherit as u64
                {
                    return Err(Error::Conflict);
                }
            } else {
                return Err(Error::Conflict);
            }
        }
        if used.len() != choices.len() {
            return Err(Error::Invalid);
        }
        if !changes.is_empty() {
            self.context_write(
                context,
                api::Write {
                    target: Some(target_wire(to)),
                    expected: Some(number(&self.asset(to)?, "revision")?),
                    kind,
                    changes,
                    ..Default::default()
                },
                working,
                now,
            )?;
        }
        Ok(())
    }
    fn apply_correction(
        &mut self,
        context: &Context,
        request: &api::IdentityCorrection,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<Vec<api::IdentityImpact>> {
        use api::IdentityDecision as D;
        let decision = D::try_from(request.decision).map_err(|_| Error::Invalid)?;
        if request.mappings.is_empty()
            || request.mappings.len() > 8
            || !matches!(
                decision,
                D::DistinctInstallations | D::DuplicateInstallation
            )
        {
            return Err(Error::Invalid);
        }
        let mut mappings = BTreeMap::new();
        let mut kinds = BTreeMap::new();
        let mut relations = BTreeMap::new();
        for mapping in &request.mappings {
            let (from, to) = pair(mapping)?;
            if from == to || mappings.insert(from, to).is_some() {
                return Err(Error::Invalid);
            }
            let old = self.correction_guard(context, from, working)?;
            let retained = self.correction_guard(context, to, working)?;
            let kind = number(&old, "kind")? as i32;
            if kind != number(&retained, "kind")? as i32
                || !matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::Device
                        | wire::AssetKind::VirtualMachine
                        | wire::AssetKind::SoftwareInstallation
                        | wire::AssetKind::Interface)
                )
            {
                return Err(Error::Invalid);
            }
            if decision == D::DistinctInstallations
                && !matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::Device | wire::AssetKind::VirtualMachine)
                )
            {
                return Err(Error::Invalid);
            }
            kinds.insert(from, kind);
            for relation in self.working_relations(from)? {
                if relations.len() == 128 {
                    return Err(Error::Limit);
                }
                relations.insert(relation.relation_id.clone(), relation);
            }
        }
        if mappings.values().any(|v| mappings.contains_key(v)) {
            return Err(Error::Invalid);
        }
        let mut attachments = BTreeSet::new();
        for mapping in &request.mappings {
            if mapping.attachments.len() > 128 {
                return Err(Error::Limit);
            }
            for id in &mapping.attachments {
                if !relations.contains_key(id) || !attachments.insert(id.as_str()) {
                    return Err(Error::Invalid);
                }
            }
        }
        for relation in relations.values() {
            if matches!(
                wire::RelationKind::try_from(relation.kind),
                Ok(wire::RelationKind::HasPort
                    | wire::RelationKind::Carries
                    | wire::RelationKind::InstalledIn
                    | wire::RelationKind::ConnectedTo
                    | wire::RelationKind::On)
            ) && !attachments.contains(relation.relation_id.as_str())
            {
                return Err(Error::Conflict);
            }
        }
        let selection = self.selection(context, working, false)?;
        let authority = string(
            &self.get("workspace/authority")?.ok_or(Error::Missing)?,
            "authority",
        )?;
        let mut impacts = Vec::new();
        // Remove old attachments before validating the final graph, so an
        // intermediate exclusive-parent conflict cannot choose the result.
        for before in relations.values() {
            let (from, to) = self.check_relationship(context, &selection, before)?;
            for endpoint in [from, to] {
                let kind = number(&self.asset(endpoint)?, "kind")? as i32;
                if !context.field(endpoint, kind, "relations", false) {
                    return Err(Error::Denied);
                }
            }
            if before.authority_id != authority {
                return Err(Error::Conflict);
            }
        }
        for mapping in &request.mappings {
            let (from, _) = pair(mapping)?;
            self.correction_fields(context, mapping, kinds[&from], working, now)?;
        }
        for before in relations.values() {
            self.unlink_correction_relation(&before.relation_id)?;
        }
        for before in relations.into_values() {
            let from = parse_target(before.from.as_ref())?;
            let to = parse_target(before.to.as_ref())?;
            let mut after = before.clone();
            after.from = Some(target_wire(*mappings.get(&from).unwrap_or(&from)));
            after.to = Some(target_wire(*mappings.get(&to).unwrap_or(&to)));
            if after.from == after.to {
                // Only an internal containment edge can become redundant.
                if !matches!(
                    wire::RelationKind::try_from(after.kind),
                    Ok(wire::RelationKind::Contains)
                ) {
                    return Err(Error::Conflict);
                }
                self.delete(&format!("relation/{}", before.relation_id))?;
                impacts.push(api::IdentityImpact {
                    before: Some(before),
                    after: None,
                });
                continue;
            }
            // Every original endpoint and mapped destination was authorized
            // before detachment. Intermediate topology cannot redefine scope.
            let redundant = self
                .working_relations(parse_target(after.from.as_ref())?)?
                .into_iter()
                .find(|r| r.from == after.from && r.to == after.to && r.kind == after.kind);
            if let Some(retained) = redundant {
                self.rebind_correction_responsibility(context, &before, &retained)?;
                self.delete(&format!("relation/{}", before.relation_id))?;
                impacts.push(api::IdentityImpact {
                    before: Some(before),
                    after: Some(retained),
                });
                continue;
            }
            after.revision = before.revision.checked_add(1).ok_or(Error::Limit)?;
            self.save_rebound_relation(&after, before.revision, &context.actor, now)?;
            self.rebind_correction_responsibility(context, &before, &after)?;
            impacts.push(api::IdentityImpact {
                before: Some(before),
                after: Some(after),
            });
        }
        for mapping in &request.mappings {
            let (from, to) = pair(mapping)?;
            self.move_correction_sources(context, mapping, from, to, kinds[&from])?;
            // Native and explicitly selected field state have moved. The
            // generic delete guard still owns all remaining bindings.
            let old = self.asset(from)?;
            self.workspace_remove(context, from, &old, now)?;
            let key = format!("removed/{}", from.asset);
            let mut fence = target_props(from);
            fence.insert("to_asset".into(), text(to.asset.to_string()));
            fence.insert("to_incarnation".into(), text(to.incarnation.to_string()));
            self.put(&key, "removed", Class::Checkpoint, fence)?;
            self.workspace_advance(to, &context.actor, "identity.corrected", now)?;
        }
        Ok(impacts)
    }
    fn unlink_correction_relation(&mut self, id: &str) -> Result<()> {
        let key = format!("relation/{id}");
        for query in [
            "MATCH (r:OmRow {key:$key})<-[e:OmRelationFrom]-() DELETE e",
            "MATCH (r:OmRow {key:$key})-[e:OmRelationTo]->() DELETE e",
        ] {
            self.store.db.execute_in_transaction(
                self.id.ok_or(Error::Corrupt)?,
                query,
                params([("key", text(&key))]),
            )?;
        }
        Ok(())
    }
    fn rebind_correction_responsibility(
        &mut self,
        context: &Context,
        before: &wire::Relation,
        after: &wire::Relation,
    ) -> Result<()> {
        for endpoint in [&before.from, &before.to].into_iter().flatten() {
            let target = parse_target(Some(endpoint))?;
            for mut row in self.fact_rows(&asset_key(target), MAX_FIELDS + 5)? {
                if row.get("source_relation") != Some(&text(&before.relation_id)) {
                    continue;
                }
                let facet = number(&row, "facet")?;
                let field = match api::ResponsibilityFacet::try_from(facet as i32) {
                    Ok(api::ResponsibilityFacet::ClaimedOwner) => "claimed_owner",
                    Ok(api::ResponsibilityFacet::Operator) => "operator",
                    _ => return Err(Error::Conflict),
                };
                if !context.field(
                    target,
                    number(&self.asset(target)?, "kind")? as i32,
                    field,
                    true,
                ) {
                    return Err(Error::Denied);
                }
                let key = string(&row, "key")?;
                row = strip(row);
                row.insert("source_binding_revision".into(), int(after.revision)?);
                row.insert("source_relation".into(), text(&after.relation_id));
                self.put(&key, &asset_key(target), Class::Inventory, row)?;
            }
        }
        Ok(())
    }
}

impl Store {
    pub(super) fn workspace_correct_identity(
        &self,
        context: &Context,
        request: api::IdentityCorrection,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if let Some(id) = &request.undo {
            if request.preview.is_some() || request.decision != 0 || !request.mappings.is_empty() {
                return Err(Error::Invalid);
            }
            return self.undo_identity_correction(context, id, working, now);
        }
        if let Some(id) = &request.preview {
            id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
            if request.decision != 0 || !request.mappings.is_empty() {
                return Err(Error::Invalid);
            }
            let mut tx = self.begin()?;
            let key = format!("preview/{id}");
            let row = match tx.get(&key)? {
                Some(row) => row,
                None => {
                    drop(tx);
                    return self.identity_correction_receipt(context, id, working);
                }
            };
            if string(&row, "owner")? != context.actor
                || number(&row, "source_binding_revision")? != context.authority_revision
            {
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
            if row.get("object_kind") != Some(&text("identity-correction"))
                || binary(&row, "working")? != selector.encode_to_vec()
            {
                return Err(Error::Conflict);
            }
            let input = api::IdentityCorrection::decode(binary(&row, "payload")?)
                .map_err(|_| Error::Corrupt)?;
            tx.inverse = Some(deltas::Capture::default());
            tx.apply_correction(context, &input, working, now)?;
            let inverse = tx.inverse.take().ok_or(Error::Corrupt)?;
            let until = now
                .checked_add(self.policy.detail_age_ms)
                .ok_or(Error::Limit)?;
            let revision = number(&tx.root, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?;
            tx.put(
                &format!("correction/{id}"),
                "detail/identity-corrections",
                Class::Detail,
                props([
                    ("owner", text(&context.actor)),
                    ("at", int(now)?),
                    ("expires", int(until)?),
                    ("revision", int(revision)?),
                    ("source_binding_revision", int(context.authority_revision)?),
                    ("payload", Value::Binary(inverse.encode()?)),
                    ("query", Value::Binary(input.encode_to_vec())),
                ]),
            )?;
            tx.delete(&key)?;
            let retained = input
                .mappings
                .iter()
                .filter_map(|v| v.retained.clone())
                .collect();
            let revision = tx.commit()?;
            return Ok(reply(api::reply::Result::IdentityCorrected(
                api::IdentityCorrectionApplied {
                    revision,
                    id: id.clone(),
                    undo_until: until,
                    retained,
                },
            )));
        }
        let mut tx = self.begin()?;
        if request.decision == api::IdentityDecision::KeepSeparate as i32 {
            if !request.mappings.is_empty() {
                return Err(Error::Invalid);
            }
            return Ok(mutation(tx.commit()?, Vec::new()));
        }
        let base = number(&tx.root, "revision")?;
        if working.expected != Some(base) {
            return Err(Error::Conflict);
        }
        drop(tx);
        let relations = {
            let mut dry = self.begin()?;
            dry.inverse = Some(deltas::Capture::default());
            let result = dry.apply_correction(context, &request, working, now)?;
            dry.inverse.as_ref().ok_or(Error::Corrupt)?.encode()?;
            result
        };
        tx = self.begin()?;
        if number(&tx.root, "revision")? != base {
            return Err(Error::Conflict);
        }
        let payload = request.encode_to_vec();
        let mut selector = working.clone();
        selector.expected = None;
        // One replaceable preview per actor, shared with ordinary import.
        let previous=tx.query("MATCH (n:OmRow {parent:'workspace-previews'}) WHERE n.owner=$owner RETURN properties(n) AS p LIMIT 5",params([("owner",text(&context.actor))]))?;
        if previous.len() > 4 {
            return Err(Error::Limit);
        }
        for old in previous {
            let key = string(&old, "key")?;
            if old.get("object_kind") == Some(&text("identity-correction"))
                && binary(&old, "payload")? == payload
                && number(&old, "revision")? == base
                && number(&old, "expires")? > now
                && number(&old, "source_binding_revision")? == context.authority_revision
                && binary(&old, "working")? == selector.encode_to_vec()
            {
                return Ok(reply(api::reply::Result::IdentityCorrection(
                    api::IdentityCorrectionView {
                        id: key.strip_prefix("preview/").ok_or(Error::Corrupt)?.into(),
                        revision: base,
                        mappings: request.mappings,
                        relations,
                        expires_at: number(&old, "expires")?,
                    },
                )));
            }
            for child in tx.children(&key)? {
                tx.delete(&string(&child, "key")?)?;
            }
            tx.delete(&key)?;
        }
        let id = AuthorityId::generate().to_string();
        let revision = base.checked_add(1).ok_or(Error::Limit)?;
        let expires_at = now
            .checked_add(self.policy.review_age_ms)
            .ok_or(Error::Limit)?;
        tx.put(
            &format!("preview/{id}"),
            "workspace-previews",
            Class::Review,
            props([
                ("owner", text(&context.actor)),
                ("revision", int(revision)?),
                ("expires", int(expires_at)?),
                ("source_binding_revision", int(context.authority_revision)?),
                ("object_kind", text("identity-correction")),
                ("working", Value::Binary(selector.encode_to_vec())),
                ("payload", Value::Binary(payload)),
            ]),
        )?;
        tx.commit()?;
        Ok(reply(api::reply::Result::IdentityCorrection(
            api::IdentityCorrectionView {
                id,
                revision,
                mappings: request.mappings,
                relations,
                expires_at,
            },
        )))
    }
    fn identity_correction_receipt(
        &self,
        context: &Context,
        id: &str,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        let tx = self.begin()?;
        let row = tx.get(&format!("correction/{id}"))?.ok_or(Error::Expired)?;
        if string(&row, "owner")? != context.actor
            || number(&row, "source_binding_revision")? != context.authority_revision
        {
            return Err(Error::Denied);
        }
        let receipt = if row.get("retired") == Some(&Value::Bool(true)) {
            api::IdentityCorrectionApplied::decode(binary(&row, "payload")?)
                .map_err(|_| Error::Corrupt)?
        } else {
            let input = api::IdentityCorrection::decode(binary(&row, "query")?)
                .map_err(|_| Error::Corrupt)?;
            api::IdentityCorrectionApplied {
                revision: number(&row, "revision")?,
                id: id.into(),
                undo_until: number(&row, "expires")?,
                retained: input
                    .mappings
                    .into_iter()
                    .filter_map(|v| v.retained)
                    .collect(),
            }
        };
        let selection = tx.selection(context, working, false)?;
        for reference in &receipt.retained {
            let target = parse_target(Some(reference))?;
            tx.visible_asset(context, target)?;
            selection.contains(target, &tx)?;
        }
        tx.commit()?;
        Ok(reply(api::reply::Result::IdentityCorrected(receipt)))
    }
}
