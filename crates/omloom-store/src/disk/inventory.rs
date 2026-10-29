use super::*;

pub(super) fn field_props(field: &wire::Field) -> Result<Properties> {
    let (kind, name, value) = match field.value.as_ref().ok_or(Error::Invalid)? {
        field::Value::Text(v) => (1, "vtext", text(v)),
        field::Value::Integer(v) => (2, "vint", Value::Int(*v)),
        field::Value::Flag(v) => (3, "vflag", Value::Bool(*v)),
        field::Value::Clear(_) => (4, "vtext", Value::Null),
    };
    Ok(props([
        ("field", text(&field.name)),
        ("vtype", Value::Int(kind)),
        (name, value),
    ]))
}
pub(super) fn read_field(p: &Properties) -> Result<wire::Field> {
    let value = match number(p, "vtype")? {
        1 => field::Value::Text(string(p, "vtext")?),
        2 => field::Value::Integer(
            p.get("vint")
                .and_then(Value::as_int)
                .ok_or(Error::Corrupt)?,
        ),
        3 => field::Value::Flag(
            p.get("vflag")
                .and_then(Value::as_bool)
                .ok_or(Error::Corrupt)?,
        ),
        4 => field::Value::Clear(0),
        _ => return Err(Error::Corrupt),
    };
    Ok(wire::Field {
        name: string(p, "field")?,
        value: Some(value),
    })
}

pub(super) fn field_matches(field: &wire::Field, p: &Properties) -> bool {
    match field.value.as_ref() {
        Some(field::Value::Text(value)) => {
            p.get("vtype").and_then(Value::as_int) == Some(1)
                && p.get("vtext").and_then(Value::as_str) == Some(value.as_str())
        }
        Some(field::Value::Integer(value)) => {
            p.get("vtype").and_then(Value::as_int) == Some(2)
                && p.get("vint").and_then(Value::as_int) == Some(*value)
        }
        Some(field::Value::Flag(value)) => {
            p.get("vtype").and_then(Value::as_int) == Some(3)
                && p.get("vflag").and_then(Value::as_bool) == Some(*value)
        }
        Some(field::Value::Clear(0)) => p.get("vtype").and_then(Value::as_int) == Some(4),
        _ => false,
    }
}

type EvidenceWindow<'a> = (Option<&'a str>, Option<&'a str>, u64, u64);
fn validate_evidence(
    asset: &wire::Asset,
    children: &[Properties],
    by_key: &BTreeMap<&str, &Properties>,
) -> Result<()> {
    fn borrow<'a>(p: &'a Properties, k: &str) -> Result<&'a str> {
        p.get(k).and_then(Value::as_str).ok_or(Error::Corrupt)
    }
    let mut claims: BTreeMap<(&str, &str, &str), EvidenceWindow<'_>> = BTreeMap::new();
    for p in children {
        let Some(Value::String(layer)) = p.get("layer") else {
            continue;
        };
        if layer != "observed" && layer != "imported" {
            continue;
        }
        let fresh_key = format!("{}/freshness", borrow(p, "key")?);
        let fresh = by_key.get(fresh_key.as_str()).ok_or(Error::Corrupt)?;
        claims.insert(
            (layer, borrow(p, "source")?, borrow(p, "field")?),
            (
                fresh.get("source_revision").and_then(Value::as_str),
                fresh.get("scan").and_then(Value::as_str),
                number(fresh, "observed")?,
                number(fresh, "valid")?,
            ),
        );
    }
    for (layer, observations) in [("imported", &asset.imported), ("observed", &asset.observed)] {
        for observation in observations {
            let evidence = observation.evidence.as_ref().ok_or(Error::Invalid)?;
            for field in &evidence.fields {
                claims.insert(
                    (layer, &evidence.source_id, &field.name),
                    (
                        evidence.source_revision.as_deref(),
                        observation.scan_id.as_deref(),
                        evidence.observed_at_ms,
                        evidence.valid_until_ms,
                    ),
                );
            }
        }
    }
    if claims.len() * 2 + asset.intended.len() + asset.sources.len() > MAX_CHILDREN {
        return Err(Error::Limit);
    }
    let mut per_source = BTreeMap::new();
    let mut groups = std::collections::BTreeSet::new();
    for ((layer, source, _), window) in claims {
        let count = per_source.entry((layer, source)).or_insert(0usize);
        *count += 1;
        if *count > 256 {
            return Err(Error::Limit);
        }
        groups.insert((layer, source, window));
    }
    for layer in ["observed", "imported"] {
        if per_source.keys().filter(|(l, _)| *l == layer).count() > 32
            || groups.iter().filter(|(l, _, _)| *l == layer).count() > 256
        {
            return Err(Error::Limit);
        }
    }
    Ok(())
}

impl Store {
    /// Full accepted intent replacement and partial evidence updates. `expected`
    /// is the accepted inventory revision, never an observation timestamp.
    pub fn save_asset(
        &self,
        record: &Record,
        expected: Option<u64>,
        actor: &str,
        now: u64,
    ) -> Result<u64> {
        self.save_asset_inner(record, expected, None, actor, now)
    }
    /// Evidence refresh binds both accepted intent and the projection used to
    /// prepare it. Opaque provider revisions cannot be ordered by their text.
    pub fn refresh_asset(
        &self,
        record: &Record,
        expected: u64,
        projection: u64,
        actor: &str,
        now: u64,
    ) -> Result<u64> {
        self.save_asset_inner(record, Some(expected), Some(projection), actor, now)
    }
    fn save_asset_inner(
        &self,
        record: &Record,
        expected: Option<u64>,
        projection: Option<u64>,
        actor: &str,
        now: u64,
    ) -> Result<u64> {
        let Some(record::Body::Asset(asset)) = &record.wire().body else {
            return Err(Error::Invalid);
        };
        let target = AssetRef::try_from(asset.target.as_ref().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        bounded(actor)?;
        int(now)?;
        let key = asset_key(target);
        let mut tx = self.begin()?;
        if tx.get(&format!("removed/{}", target.asset))?.is_some() {
            return Err(Error::Conflict);
        }
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        if projection.is_some_and(|r| number(&tx.root, "revision").ok() != Some(r)) {
            return Err(Error::Conflict);
        }
        let old = tx.get(&key)?;
        match (&old, expected) {
            (None, None) => {}
            (Some(p), Some(rev))
                if number(p, "revision")? == rev && super::target(p)? == target => {}
            _ => return Err(Error::Conflict),
        }
        let children = tx.children(&key)?;
        if asset.kind != wire::AssetKind::Container as i32
            && old.as_ref().is_some_and(|p| {
                p.get("kind").and_then(Value::as_int) == Some(wire::AssetKind::Container as i64)
            })
        {
            return Err(Error::Conflict);
        }
        if old
            .as_ref()
            .is_some_and(|p| p.get("kind").and_then(Value::as_int) != Some(i64::from(asset.kind)))
            || asset.adoption == wire::Adoption::Archived as i32
        {
            // Retained allocations and relations require explicit release before
            // archival or retyping; hidden endpoints would strand those records.
            tx.require_no_ipam_state(target)?;
            tx.require_no_account_references(target)?;
            if children.iter().any(|row| row.contains_key("link_kind")) {
                return Err(Error::Conflict);
            }
            let attached=tx.query("MATCH (n:OmRow) WHERE n.parent='relations' AND (n.asset=$asset OR n.to_asset=$asset) RETURN {} AS p LIMIT 1",params([("asset",text(target.asset.to_string()))]))?;
            if !attached.is_empty() {
                return Err(Error::Conflict);
            }
        }
        if asset.adoption == wire::Adoption::Archived as i32 {
            let old = old.as_ref().ok_or(Error::Invalid)?;
            let pending=tx.query("MATCH (n:OmRow) WHERE n.parent='operations' AND n.class=$class AND n.asset=$asset RETURN properties(n) AS p LIMIT 1",params([("class",int(Class::Pending as u64)?),("asset",text(target.asset.to_string()))]))?;
            if !pending.is_empty() {
                return Err(Error::Conflict);
            }
            if number(old, "adoption")? == wire::Adoption::Archived as u64 {
                tx.commit()?;
                return number(old, "revision");
            }
            tx.retire_current_facts(target)?;
            for row in children {
                let child = string(&row, "key")?;
                if row.get("layer") == Some(&text("mapping")) {
                    let mut checkpoint = row;
                    super::operations::strip_envelope(&mut checkpoint);
                    tx.put(&child, &key, Class::Checkpoint, checkpoint)?;
                } else {
                    tx.delete(&child)?;
                }
            }
            let revision = number(old, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?;
            let mut data = target_props(target);
            data.extend(props([
                ("kind", Value::Int(asset.kind.into())),
                ("adoption", int(wire::Adoption::Archived as u64)?),
                ("revision", int(revision)?),
            ]));
            tx.put(&key, "assets", Class::Inventory, data)?;
            tx.audit(&key, actor, "inventory.archived", revision, now)?;
            tx.commit()?;
            return Ok(revision);
        }
        if old.as_ref().is_some_and(|p| {
            p.get("adoption") == Some(&Value::Int(wire::Adoption::Archived as i64))
        }) {
            return Err(Error::Conflict);
        }
        let by_key: BTreeMap<&str, &Properties> = children
            .iter()
            .map(|p| {
                Ok((
                    p.get("key").and_then(Value::as_str).ok_or(Error::Corrupt)?,
                    p,
                ))
            })
            .collect::<Result<_>>()?;
        validate_evidence(asset, &children, &by_key)?;
        tx.validate_full_software_asset(target, asset)?;
        let mut intent_changed = old.as_ref().is_none_or(|p| {
            p.get("kind") != Some(&Value::Int(asset.kind.into()))
                || p.get("adoption") != Some(&Value::Int(asset.adoption.into()))
        });
        let mut keep = std::collections::BTreeSet::new();
        let mut runtime_identity_changed = false;
        for intended in &asset.intended {
            let field = intended.field.as_ref().ok_or(Error::Invalid)?;
            let child = format!("{key}/intent/{}", field.name);
            keep.insert(child.clone());
            let previous = by_key.get(child.as_str()).copied();
            if previous.is_some_and(|p| {
                field_matches(field, p)
                    && p.get("authority").and_then(Value::as_str)
                        == Some(intended.authority_id.as_str())
            }) {
                continue;
            }
            let mut p = field_props(field)?;
            if asset.kind == wire::AssetKind::Container as i32
                && matches!(
                    field.name.as_str(),
                    "runtime_object_id" | "runtime_lifetime"
                )
                && !previous.is_some_and(|p| field_matches(field, p))
            {
                runtime_identity_changed = true;
            }
            p.insert("layer".into(), text("intent"));
            p.insert("authority".into(), text(&intended.authority_id));
            intent_changed |= tx.put_against(&child, &key, Class::Inventory, p, previous)?;
        }
        for p in &children {
            if p.get("layer") == Some(&text("intent")) && !keep.contains(&string(p, "key")?) {
                tx.delete(&string(p, "key")?)?;
                intent_changed = true;
            }
        }
        let mut mapping_keys = std::collections::BTreeSet::new();
        for source in &asset.sources {
            let object = source.object.as_ref().ok_or(Error::Invalid)?;
            let mapping = format!(
                "mapping/{}/{}/{}",
                source.connection_id,
                hex(object.object_kind.as_bytes()),
                hex(object.object_id.as_bytes())
            );
            mapping_keys.insert(mapping.clone());
            if let Some(previous) = by_key.get(mapping.as_str()) {
                if previous.get("lifetime").and_then(Value::as_str)
                    != object.incarnation_evidence.as_deref()
                {
                    return Err(Error::Conflict);
                }
                continue;
            }
            if tx.get(&mapping)?.is_some() {
                return Err(Error::Conflict);
            }
            let mut p = target_props(target);
            p.extend(props([
                ("layer", text("mapping")),
                ("connection", text(&source.connection_id)),
                ("object_kind", text(&object.object_kind)),
                ("object_id", text(&object.object_id)),
                (
                    "lifetime",
                    text(object.incarnation_evidence.as_ref().ok_or(Error::Invalid)?),
                ),
            ]));
            intent_changed |= tx.put_against(&mapping, &key, Class::Inventory, p, None)?;
        }
        for p in &children {
            if p.get("layer") == Some(&text("mapping"))
                && !mapping_keys.contains(&string(p, "key")?)
            {
                tx.delete(&string(p, "key")?)?;
                intent_changed = true;
            }
        }
        let mut evidence_changed = false;
        for (layer, observations) in [("imported", &asset.imported), ("observed", &asset.observed)]
        {
            for observation in observations {
                let evidence = observation.evidence.as_ref().ok_or(Error::Invalid)?;
                for field in &evidence.fields {
                    let child = format!("{key}/{layer}/{}/{}", evidence.source_id, field.name);
                    let previous = by_key.get(child.as_str()).copied();
                    if !previous.is_some_and(|p| field_matches(field, p)) {
                        let mut value = field_props(field)?;
                        value.extend(props([
                            ("layer", text(layer)),
                            ("source", text(&evidence.source_id)),
                        ]));
                        evidence_changed |=
                            tx.put_against(&child, &key, Class::Evidence, value, previous)?;
                    }
                    // A freshness-only refresh never rewrites the value row.
                    let fresh_key = format!("{child}/freshness");
                    let prior_fresh = by_key.get(fresh_key.as_str()).copied();
                    if prior_fresh.is_some_and(|p| {
                        p.get("source_revision").and_then(Value::as_str)
                            == evidence.source_revision.as_deref()
                            && p.get("scan").and_then(Value::as_str)
                                == observation.scan_id.as_deref()
                            && p.get("observed").and_then(Value::as_int)
                                == i64::try_from(evidence.observed_at_ms).ok()
                            && p.get("valid").and_then(Value::as_int)
                                == i64::try_from(evidence.valid_until_ms).ok()
                    }) {
                        continue;
                    }
                    evidence_changed |= tx.put_against(
                        &fresh_key,
                        &key,
                        Class::Evidence,
                        props([
                            ("layer", text("freshness")),
                            ("claim", text(&child)),
                            (
                                "source_revision",
                                evidence.source_revision.as_ref().map_or(Value::Null, text),
                            ),
                            (
                                "scan",
                                observation.scan_id.as_ref().map_or(Value::Null, text),
                            ),
                            ("observed", int(evidence.observed_at_ms)?),
                            ("valid", int(evidence.valid_until_ms)?),
                        ]),
                        prior_fresh,
                    )?;
                }
            }
        }
        if old.is_some() && evidence_changed && projection.is_none() {
            return Err(Error::Conflict);
        }
        let mut revision = match old.as_ref() {
            Some(p) => number(p, "revision")?,
            None => 0,
        };
        if intent_changed {
            revision = revision.checked_add(1).ok_or(Error::Limit)?;
        }
        if intent_changed {
            let mut data = target_props(target);
            data.extend(props([
                ("kind", Value::Int(asset.kind.into())),
                ("adoption", Value::Int(asset.adoption.into())),
                ("revision", int(revision)?),
            ]));
            workspace::hot_fields(asset, &mut data);
            tx.put_against(&key, "assets", Class::Inventory, data, old.as_ref())?;
            if runtime_identity_changed {
                tx.bind_container_runtime(target, None)?;
            }
            tx.audit(&key, actor, "inventory.accepted", revision, now)?;
        }
        tx.commit()?;
        Ok(revision)
    }

    /// Read one current projection in a pinned transaction, with no durable touch.
    pub fn asset(&self, target: AssetRef) -> Result<wire::Asset> {
        let tx = self.begin()?;
        let root = tx.asset(target)?;
        let key = asset_key(target);
        let rows = tx.children(&key)?;
        let mut output = wire::Asset {
            target: Some(wire::AssetRef {
                asset_id: target.asset.to_string(),
                incarnation: target.incarnation.to_string(),
            }),
            kind: number(&root, "kind")? as i32,
            adoption: number(&root, "adoption")? as i32,
            revision: number(&root, "revision")?,
            ..Default::default()
        };
        let by_key: BTreeMap<&str, &Properties> = rows
            .iter()
            .map(|p| {
                Ok((
                    p.get("key").and_then(Value::as_str).ok_or(Error::Corrupt)?,
                    p,
                ))
            })
            .collect::<Result<_>>()?;
        for p in &rows {
            let Some(Value::String(layer)) = p.get("layer") else {
                continue;
            };
            match layer.as_str() {
                "intent" => output.intended.push(wire::IntendedField {
                    field: Some(read_field(p)?),
                    authority_id: string(p, "authority")?,
                }),
                "mapping" => output.sources.push(wire::SourceIdentity {
                    connection_id: string(p, "connection")?,
                    object: Some(wire::SourceObject {
                        object_kind: string(p, "object_kind")?,
                        object_id: string(p, "object_id")?,
                        incarnation_evidence: Some(string(p, "lifetime")?),
                    }),
                }),
                "observed" | "imported" => {
                    let fresh_key = format!("{}/freshness", string(p, "key")?);
                    let fresh = by_key.get(fresh_key.as_str()).ok_or(Error::Corrupt)?;
                    let evidence = wire::Evidence {
                        source_id: string(p, "source")?,
                        source_revision: fresh
                            .get("source_revision")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        observed_at_ms: number(fresh, "observed")?,
                        valid_until_ms: number(fresh, "valid")?,
                        fields: vec![read_field(p)?],
                    };
                    let scan_id = fresh.get("scan").and_then(Value::as_str).map(str::to_owned);
                    let groups = if layer == "observed" {
                        &mut output.observed
                    } else {
                        &mut output.imported
                    };
                    if let Some(group) = groups.iter_mut().find(|o| {
                        o.scan_id == scan_id
                            && o.evidence.as_ref().is_some_and(|e| {
                                e.source_id == evidence.source_id
                                    && e.source_revision == evidence.source_revision
                                    && e.observed_at_ms == evidence.observed_at_ms
                                    && e.valid_until_ms == evidence.valid_until_ms
                            })
                    }) {
                        group
                            .evidence
                            .as_mut()
                            .ok_or(Error::Corrupt)?
                            .fields
                            .extend(evidence.fields);
                    } else {
                        groups.push(wire::Observation {
                            scan_id,
                            evidence: Some(evidence),
                        });
                    }
                }
                _ => {}
            }
        }
        tx.commit()?;
        Ok(output)
    }

    /// Stable pages expire on any concurrent application mutation. The host must
    /// pass an already authorized scope; this API never derives user permissions.
    pub fn assets(&self, after: Option<&str>, revision: Option<u64>, limit: u16) -> Result<Page> {
        if limit == 0 || limit > 128 {
            return Err(Error::Invalid);
        }
        if let Some(key) = after {
            bounded(key)?;
        }
        let tx = self.begin()?;
        let current = number(&tx.root, "revision")?;
        if revision.is_some_and(|r| r != current) {
            return Err(Error::Expired);
        }
        let after = after.unwrap_or(""); // The first page has an explicit empty lower bound.
        let rows=tx.query("MATCH (n:OmRow) WHERE n.parent='assets' AND n.key>$after RETURN properties(n) AS p, n.key AS ordering ORDER BY ordering LIMIT $limit",params([("after",text(after)),("limit",int(u64::from(limit)+1)?)]))?;
        let mut assets = Vec::with_capacity(rows.len().min(usize::from(limit)));
        for p in rows.iter().take(usize::from(limit)) {
            assets.push(AssetSummary {
                target: target(p)?,
                kind: wire::AssetKind::try_from(number(p, "kind")? as i32)
                    .map_err(|_| Error::Corrupt)?,
                revision: number(p, "revision")?,
            });
        }
        let next = if rows.len() > usize::from(limit) {
            rows.get(usize::from(limit) - 1)
                .map(|p| string(p, "key"))
                .transpose()?
        } else {
            None
        };
        tx.commit()?;
        Ok(Page {
            revision: current,
            assets,
            next,
        })
    }

    pub fn save_relation(
        &self,
        record: &Record,
        expected: Option<u64>,
        actor: &str,
        now: u64,
    ) -> Result<u64> {
        let Some(record::Body::Relation(relation)) = &record.wire().body else {
            return Err(Error::Invalid);
        };
        let mut tx = self.begin()?;
        let revision = tx.save_typed_relation(relation, expected, actor, now)?;
        tx.commit()?;
        Ok(revision)
    }

    pub fn relation(&self, id: omloom_contracts::ids::RelationId) -> Result<wire::Relation> {
        let p = self
            .read(&format!("relation/{id}"))?
            .ok_or(Error::Missing)?;
        relation_projection(&p)
    }
    /// One bounded native adjacency expansion; excess results are explicitly
    /// unavailable rather than an incomplete dependency set presented as complete.
    pub fn relations(&self, target: AssetRef, limit: u16) -> Result<Vec<wire::Relation>> {
        if limit == 0 || limit > 128 {
            return Err(Error::Invalid);
        }
        let tx = self.begin()?;
        tx.asset(target)?;
        let mut result = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for query in [
            "MATCH (a:OmRow {key:$asset})-[:OmRelationFrom]->(r:OmRow) RETURN properties(r) AS p LIMIT $limit",
            "MATCH (a:OmRow {key:$asset})<-[:OmRelationTo]-(r:OmRow) RETURN properties(r) AS p LIMIT $limit",
        ] {
            let rows = tx.query(
                query,
                params([
                    ("asset", text(asset_key(target))),
                    ("limit", int(u64::from(limit) + 1)?),
                ]),
            )?;
            for row in rows {
                if seen.insert(string(&row, "key")?) {
                    result.push(relation_projection(&row)?);
                }
            }
            if result.len() > usize::from(limit) {
                return Err(Error::Limit);
            }
        }
        tx.commit()?;
        Ok(result)
    }
}
pub(super) fn relation_projection(p: &Properties) -> Result<wire::Relation> {
    let key = p.get("key").and_then(Value::as_str).ok_or(Error::Corrupt)?;
    Ok(wire::Relation {
        relation_id: key.strip_prefix("relation/").ok_or(Error::Corrupt)?.into(),
        from: Some(wire::AssetRef {
            asset_id: string(p, "asset")?,
            incarnation: string(p, "incarnation")?,
        }),
        to: Some(wire::AssetRef {
            asset_id: string(p, "to_asset")?,
            incarnation: string(p, "to_incarnation")?,
        }),
        kind: number(p, "kind")? as i32,
        revision: number(p, "revision")?,
        authority_id: string(p, "authority")?,
    })
}

impl Tx<'_> {
    pub(super) fn save_typed_relation(
        &mut self,
        relation: &wire::Relation,
        expected: Option<u64>,
        actor: &str,
        now: u64,
    ) -> Result<u64> {
        self.save_relation_inner(relation, expected, actor, now, false)
    }
    pub(super) fn save_rebound_relation(
        &mut self,
        relation: &wire::Relation,
        expected: u64,
        actor: &str,
        now: u64,
    ) -> Result<u64> {
        self.save_relation_inner(relation, Some(expected), actor, now, true)
    }
    fn save_relation_inner(
        &mut self,
        relation: &wire::Relation,
        expected: Option<u64>,
        actor: &str,
        now: u64,
        detached: bool,
    ) -> Result<u64> {
        let from = AssetRef::try_from(relation.from.as_ref().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        let to = AssetRef::try_from(relation.to.as_ref().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        let from_row = self.asset(from)?;
        let to_row = self.asset(to)?;
        if number(&from_row, "adoption")? != wire::Adoption::Adopted as u64
            || number(&to_row, "adoption")? != wire::Adoption::Adopted as u64
        {
            return Err(Error::Invalid);
        }
        let from_kind = wire::AssetKind::try_from(number(&from_row, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        let to_kind = wire::AssetKind::try_from(number(&to_row, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        let kind = wire::RelationKind::try_from(relation.kind).map_err(|_| Error::Invalid)?;
        if !compatible(from_kind, to_kind, kind) {
            return Err(Error::Invalid);
        }
        self.check_software_relation(from, from_kind, kind, &relation.relation_id)?;
        if kind == wire::RelationKind::RunsOn
            && from_kind == wire::AssetKind::VirtualMachine
            && to_kind == wire::AssetKind::SoftwareInstallation
            && to_row.get("virtualization_role").and_then(Value::as_str)
                != Some(omloom_contracts::workspace::VirtualizationRole::Hypervisor.as_str_name())
        {
            return Err(Error::Invalid);
        }
        let key = format!("relation/{}", relation.relation_id);
        let old = self.get(&key)?;
        if old.as_ref().map(|p| number(p, "revision")).transpose()? != expected {
            return Err(Error::Conflict);
        }
        if let Some(old) = &old {
            self.check_software_relation_replacement(old, from, from_kind, to, kind, detached)?;
        }
        let placement = match kind {
            wire::RelationKind::InstalledIn => Some((from, to)),
            wire::RelationKind::Contains | wire::RelationKind::HasPort => Some((to, from)),
            _ => None,
        };
        if let Some((child, parent)) = placement {
            self.check_physical_parent(child, parent, &key)?;
        } else if kind == wire::RelationKind::RunsOn {
            self.check_placement_cycle(from, to, Some(&key))?;
        }
        if kind == wire::RelationKind::Contains
            && from_kind == wire::AssetKind::Interface
            && to_kind == wire::AssetKind::AddressReference
        {
            // Validate the proposed owner before either explicit or automatic attachment commits.
            self.check_bound_address_owner(to, from)?;
        }
        let mut data = target_props(from);
        data.extend(props([
            ("to_asset", text(to.asset.to_string())),
            ("to_incarnation", text(to.incarnation.to_string())),
            ("kind", Value::Int(relation.kind.into())),
            ("authority", text(&relation.authority_id)),
        ]));
        if old
            .as_ref()
            .is_some_and(|prior| data.iter().all(|(k, v)| prior.get(k) == Some(v)))
        {
            return expected.ok_or(Error::Corrupt);
        }
        let revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Limit)?;
        if kind == wire::RelationKind::RunsOn && from_kind == wire::AssetKind::VirtualMachine {
            self.require_no_pending_operations(from)?;
        }
        if kind == wire::RelationKind::RunsOn && from_kind == wire::AssetKind::Container {
            self.bind_container_runtime(from, Some(to))?;
        }
        data.insert("revision".into(), int(revision)?);
        let source_changed = detached
            || old.as_ref().is_none_or(|p| {
                p.get("asset") != data.get("asset")
                    || p.get("incarnation") != data.get("incarnation")
            });
        let target_changed = detached
            || old.as_ref().is_none_or(|p| {
                p.get("to_asset") != data.get("to_asset")
                    || p.get("to_incarnation") != data.get("to_incarnation")
            });
        self.put_against(&key, "relations", Class::Inventory, data, old.as_ref())?;
        if source_changed {
            if old.is_some() {
                self.store.db.execute_in_transaction(
                    self.id.ok_or(Error::Corrupt)?,
                    "MATCH (r:OmRow {key:$key})<-[e:OmRelationFrom]-() DELETE e",
                    params([("key", text(&key))]),
                )?;
            }
            self.store.db.execute_in_transaction(self.id.ok_or(Error::Corrupt)?, "MATCH (a:OmRow {key:$from}), (r:OmRow {key:$key}) CREATE (a)-[:OmRelationFrom]->(r)", params([("from", text(asset_key(from))), ("key", text(&key))]))?;
        }
        if target_changed {
            if old.is_some() {
                self.store.db.execute_in_transaction(
                    self.id.ok_or(Error::Corrupt)?,
                    "MATCH (r:OmRow {key:$key})-[e:OmRelationTo]->() DELETE e",
                    params([("key", text(&key))]),
                )?;
            }
            self.store.db.execute_in_transaction(
                self.id.ok_or(Error::Corrupt)?,
                "MATCH (r:OmRow {key:$key}), (a:OmRow {key:$to}) CREATE (r)-[:OmRelationTo]->(a)",
                params([("to", text(asset_key(to))), ("key", text(&key))]),
            )?;
        }
        self.audit(&key, actor, "relation.accepted", revision, now)?;
        Ok(revision)
    }
    fn physical_parent(&self, child: AssetRef, skip: Option<&str>) -> Result<Option<AssetRef>> {
        let mut parents = Vec::with_capacity(2);
        for (query, kinds, incoming) in [
            (
                "MATCH (a:OmRow {key:$asset})-[:OmRelationFrom]->(r:OmRow) WHERE r.kind IN $kinds AND ($skip IS NULL OR r.key<>$skip) RETURN properties(r) AS p LIMIT 2",
                vec![wire::RelationKind::InstalledIn],
                false,
            ),
            (
                "MATCH (a:OmRow {key:$asset})<-[:OmRelationTo]-(r:OmRow) WHERE r.kind IN $kinds AND ($skip IS NULL OR r.key<>$skip) RETURN properties(r) AS p LIMIT 2",
                vec![wire::RelationKind::Contains, wire::RelationKind::HasPort],
                true,
            ),
        ] {
            let rows = self.query(
                query,
                params([
                    ("asset", text(asset_key(child))),
                    ("skip", skip.map_or(Value::Null, text)),
                    (
                        "kinds",
                        Value::Array(
                            kinds
                                .into_iter()
                                .map(|k| Value::Int(k as i32 as i64))
                                .collect(),
                        ),
                    ),
                ]),
            )?;
            for row in rows {
                if parents.len() == 1 {
                    return Err(Error::Conflict);
                }
                let parent = if incoming {
                    super::target(&row)?
                } else {
                    AssetRef {
                        asset: string(&row, "to_asset")?
                            .parse()
                            .map_err(|_| Error::Corrupt)?,
                        incarnation: string(&row, "to_incarnation")?
                            .parse()
                            .map_err(|_| Error::Corrupt)?,
                    }
                };
                parents.push(parent);
            }
        }
        Ok(parents.pop())
    }
    fn check_physical_parent(&self, child: AssetRef, parent: AssetRef, skip: &str) -> Result<()> {
        if self.physical_parent(child, Some(skip))?.is_some() {
            return Err(Error::Conflict);
        }
        self.check_placement_cycle(child, parent, Some(skip))
    }
    pub(super) fn check_placement_cycle(
        &self,
        child: AssetRef,
        parent: AssetRef,
        skip: Option<&str>,
    ) -> Result<()> {
        // Execution placement and containment have opposite stored arrows but share ancestry.
        // Legacy multiple hosts are still traversed; None excludes no existing relation.
        let mut pending = vec![parent];
        let mut seen = std::collections::BTreeSet::from([parent]);
        while let Some(node) = pending.pop() {
            if node == child {
                return Err(Error::Invalid);
            }
            let physical = self.physical_parent(node, skip)?;
            let hosts = self.query(
                "MATCH (a:OmRow {key:$asset})-[:OmRelationFrom]->(r:OmRow)-[:OmRelationTo]->(p:OmRow) WHERE r.kind=$kind AND ($skip IS NULL OR r.key<>$skip) RETURN properties(p) AS p LIMIT 129",
                params([("asset", text(asset_key(node))), ("skip", skip.map_or(Value::Null, text)), ("kind", Value::Int(wire::RelationKind::RunsOn as i32 as i64))]),
            )?;
            if hosts.len() > 128 {
                return Err(Error::Limit);
            }
            for ancestor in physical
                .into_iter()
                .map(Ok)
                .chain(hosts.iter().map(super::target))
            {
                let ancestor = ancestor?;
                if seen.contains(&ancestor) {
                    continue;
                }
                if seen.len() == 1024 {
                    return Err(Error::Limit);
                }
                seen.insert(ancestor);
                pending.push(ancestor);
            }
        }
        Ok(())
    }
}

pub(super) fn compatible(
    from: wire::AssetKind,
    to: wire::AssetKind,
    kind: wire::RelationKind,
) -> bool {
    use wire::{AssetKind as K, RelationKind as R};
    match kind {
        R::InstalledIn => matches!(
            (from, to),
            (K::Device, K::Rack | K::Site | K::Location)
                | (K::Rack, K::Site | K::Location)
                | (K::Location, K::Site | K::Location)
        ),
        R::Contains => matches!(
            (from, to),
            (K::Device, K::Device | K::Interface | K::AddressReference)
                | (
                    K::VirtualMachine | K::Interface,
                    K::Interface | K::AddressReference
                )
                | (K::Service, K::Interface | K::AddressReference)
                | (
                    K::SoftwareInstallation,
                    K::Interface | K::AddressReference | K::Service | K::SoftwareInstallation
                )
                | (K::Device | K::VirtualMachine, K::SoftwareInstallation)
                | (K::Service, K::SoftwareInstallation)
                | (K::Service, K::ProviderAccount)
                | (K::Workload, K::Container | K::Service)
                | (
                    K::Container,
                    K::Service | K::Interface | K::SoftwareInstallation
                )
        ),
        R::HasPort => matches!((from, to), (K::Device, K::Port)),
        R::Carries => matches!((from, to), (K::Port, K::Interface)),
        R::ConnectedTo => matches!((from, to), (K::Port, K::Cable) | (K::Cable, K::Port)),
        R::AssignedTo => matches!((from, to), (K::IpAddress, K::Interface)),
        R::RunsOn => matches!(
            (from, to),
            (K::VirtualMachine, K::Device | K::SoftwareInstallation)
                | (K::Service, K::Device | K::VirtualMachine)
                | (K::Workload | K::Container, K::SoftwareInstallation)
        ),
        R::StoredOn => matches!((from, to), (K::VirtualMachine, K::Storage)),
        R::On => matches!(
            (from, to),
            (K::Storage | K::Vrf, K::Device) | (K::Interface, K::Vlan | K::L2Domain | K::Vrf)
        ),
        R::Uses => matches!(
            (from, to),
            (K::Device, K::Interface) | (K::Vrf, K::AddressSpace)
        ),
        R::PoweredBy => matches!((from, to), (K::Device, K::Port) | (K::Port, K::PowerFeed)),
        R::Manages => matches!(
            (from, to),
            (K::Device, K::Device) | (K::Service, K::Device | K::VirtualMachine)
        ),
        R::MemberOf => matches!((from, to), (K::Interface, K::Lag) | (K::Device, K::Cluster)),
        R::DependsOn => matches!((from, to), (K::Service, K::Service | K::Device)),
        R::ReleaseOf => from == K::SoftwareRelease && to == K::SoftwareProduct,
        R::InstalledRelease => from == K::SoftwareInstallation && to == K::SoftwareRelease,
        R::Unspecified => false,
    }
}
