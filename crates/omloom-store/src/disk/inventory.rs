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

fn field_matches(field: &wire::Field, p: &Properties) -> bool {
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
        let mut intent_changed = old.as_ref().is_none_or(|p| {
            p.get("kind") != Some(&Value::Int(asset.kind.into()))
                || p.get("adoption") != Some(&Value::Int(asset.adoption.into()))
        });
        let mut keep = std::collections::BTreeSet::new();
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
            tx.put_against(&key, "assets", Class::Inventory, data, old.as_ref())?;
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
        let from = AssetRef::try_from(relation.from.as_ref().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        let to = AssetRef::try_from(relation.to.as_ref().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        let mut tx = self.begin()?;
        let from_row = tx.asset(from)?;
        let to_row = tx.asset(to)?;
        use wire::{AssetKind as A, RelationKind as R};
        let from_kind =
            A::try_from(number(&from_row, "kind")? as i32).map_err(|_| Error::Corrupt)?;
        let to_kind = A::try_from(number(&to_row, "kind")? as i32).map_err(|_| Error::Corrupt)?;
        let kind = R::try_from(relation.kind).map_err(|_| Error::Invalid)?;
        let compatible = match kind {
            R::InstalledIn => matches!(
                (from_kind, to_kind),
                (A::Device, A::Rack)
                    | (A::Rack, A::Site | A::Location)
                    | (A::Location, A::Site | A::Location)
            ),
            R::Contains => matches!((from_kind, to_kind), (A::Device, A::Device)),
            R::HasPort => matches!((from_kind, to_kind), (A::Device, A::Port)),
            R::Carries => matches!((from_kind, to_kind), (A::Port, A::Interface)),
            R::ConnectedTo => matches!(
                (from_kind, to_kind),
                (A::Port, A::Cable) | (A::Cable, A::Port)
            ),
            R::AssignedTo => matches!((from_kind, to_kind), (A::IpAddress, A::Interface)),
            R::RunsOn => matches!(
                (from_kind, to_kind),
                (A::VirtualMachine, A::Device) | (A::Service, A::Device | A::VirtualMachine)
            ),
            R::StoredOn => matches!((from_kind, to_kind), (A::VirtualMachine, A::Storage)),
            R::On => matches!((from_kind, to_kind), (A::Storage, A::Device)),
            R::Uses => matches!((from_kind, to_kind), (A::Device, A::Interface)),
            R::PoweredBy => matches!(
                (from_kind, to_kind),
                (A::Device, A::Port) | (A::Port, A::PowerFeed)
            ),
            R::Manages => matches!(
                (from_kind, to_kind),
                (A::Device, A::Device) | (A::Service, A::Device | A::VirtualMachine)
            ),
            R::MemberOf => matches!(
                (from_kind, to_kind),
                (A::Interface, A::Lag) | (A::Device, A::Cluster)
            ),
            R::DependsOn => matches!((from_kind, to_kind), (A::Service, A::Service | A::Device)),
            R::Unspecified => false,
        };
        if !compatible {
            return Err(Error::Invalid);
        }
        let key = format!("relation/{}", relation.relation_id);
        let old = tx.get(&key)?;
        if old.as_ref().map(|p| number(p, "revision")).transpose()? != expected {
            return Err(Error::Conflict);
        }
        let revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Limit)?;
        let mut p = target_props(from);
        p.extend(props([
            ("to_asset", text(to.asset.to_string())),
            ("to_incarnation", text(to.incarnation.to_string())),
            ("kind", Value::Int(relation.kind.into())),
            ("authority", text(&relation.authority_id)),
            ("revision", int(revision)?),
        ]));
        tx.put(&key, "relations", Class::Inventory, p)?;
        tx.audit(&key, actor, "relation.accepted", revision, now)?;
        tx.commit()?;
        Ok(revision)
    }

    pub fn relation(&self, id: omloom_contracts::ids::RelationId) -> Result<wire::Relation> {
        let p = self
            .read(&format!("relation/{id}"))?
            .ok_or(Error::Missing)?;
        Ok(wire::Relation {
            relation_id: id.to_string(),
            from: Some(wire::AssetRef {
                asset_id: string(&p, "asset")?,
                incarnation: string(&p, "incarnation")?,
            }),
            to: Some(wire::AssetRef {
                asset_id: string(&p, "to_asset")?,
                incarnation: string(&p, "to_incarnation")?,
            }),
            kind: number(&p, "kind")? as i32,
            revision: number(&p, "revision")?,
            authority_id: string(&p, "authority")?,
        })
    }
}
