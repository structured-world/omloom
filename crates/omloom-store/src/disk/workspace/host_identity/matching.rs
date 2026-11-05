//! Descriptive probes index existing current fields, never a machine fingerprint.
//! Comparison has no writes or automatic identity/adoption decision.
use super::*;

pub(in crate::disk) const DESCRIPTORS: &[&str] = &[
    "name",
    "model",
    "manufacturer",
    "serial",
    "cpu_model",
    "logical_cpus",
    "visible_memory_bytes",
    "installation_kind",
    "interface_kind",
];

pub(in crate::disk) fn subject_kind(kind: i32) -> bool {
    matches!(
        wire::AssetKind::try_from(kind),
        Ok(wire::AssetKind::Device
            | wire::AssetKind::VirtualMachine
            | wire::AssetKind::SoftwareInstallation
            | wire::AssetKind::Interface)
    )
}

pub(in crate::disk) fn descriptor(field: &wire::Field) -> Option<String> {
    if !DESCRIPTORS.contains(&field.name.as_str()) {
        return None;
    }
    let value = match &field.value {
        Some(field::Value::Text(value)) if !value.trim().is_empty() => {
            format!("text/{}", value.trim().to_lowercase())
        }
        Some(field::Value::Integer(value)) if *value > 0 => format!("integer/{value}"),
        _ => return None,
    };
    Some(format!(
        "descriptor/{}/{:x}",
        field.name,
        Sha256::digest(value.as_bytes())
    ))
}

pub(in crate::disk) fn upgrade(store: &Store) -> Result<()> {
    let mut after = String::new();
    loop {
        let mut tx = store.begin()?;
        let rows = tx.query(
            "MATCH (n:OmRow) WHERE n.key>$after AND n.layer IN ['intent','observed','imported'] AND n.field IN $fields MATCH (a:OmRow {key:n.parent}) WHERE a.parent='assets' AND a.kind IN $kinds RETURN properties(n) AS p ORDER BY n.key LIMIT 128",
            params([("after", text(&after)), ("fields", Value::Array(DESCRIPTORS.iter().map(|v| text(*v)).collect())), ("kinds", Value::Array([wire::AssetKind::Device, wire::AssetKind::VirtualMachine, wire::AssetKind::SoftwareInstallation, wire::AssetKind::Interface].into_iter().map(|v| Value::Int(v as i32 as i64)).collect()))]),
        )?;
        if rows.is_empty() {
            return Ok(());
        }
        for old in rows {
            let key = string(&old, "key")?;
            if let Some(probe) = descriptor(&inventory::read_field(&old)?) {
                let mut data = old.clone();
                operations::strip_envelope(&mut data);
                data.insert("claim".into(), text(probe));
                let class = match number(&old, "class")? {
                    0 => Class::Inventory,
                    1 => Class::Evidence,
                    _ => return Err(Error::Corrupt),
                };
                tx.put_projection(&key, &string(&old, "parent")?, class, data, Some(&old))?;
            }
            after = key;
        }
        tx.commit()?;
    }
}

impl Store {
    pub(super) fn workspace_descriptive_candidates(
        &self,
        context: &Context,
        mut request: api::FindIdentityCandidates,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        if request.target.is_some() || !request.claims.is_empty() {
            return Err(Error::Invalid);
        }
        let finding = request.finding.as_ref().ok_or(Error::Invalid)?;
        validate(finding)?;
        if finding.fields.len() > MAX_FIELDS
            || finding.addresses.len() > 64
            || finding.key.len() > 256
        {
            return Err(Error::Limit);
        }
        let kind =
            if finding.kind == wire::AssetKind::Unspecified as i32 && finding.key == "hardware" {
                request.kind
            } else {
                if request.kind != 0 {
                    return Err(Error::Invalid);
                }
                finding.kind
            };
        let unclassified = finding.kind == wire::AssetKind::Unspecified as i32
            && finding.key == "hardware"
            && kind == 0;
        if !unclassified
            && !matches!(
                wire::AssetKind::try_from(kind),
                Ok(wire::AssetKind::Device
                    | wire::AssetKind::VirtualMachine
                    | wire::AssetKind::SoftwareInstallation
                    | wire::AssetKind::Interface)
            )
        {
            return Err(Error::Invalid);
        }
        let tx = self.begin()?;
        let selection = tx.selection(context, working, false)?;
        let mut fields = BTreeSet::new();
        for field in &finding.fields {
            if !fields.insert(field.name.as_str()) {
                return Err(Error::Invalid);
            }
            tx.validate_field(
                if unclassified {
                    wire::AssetKind::Device as i32
                } else {
                    kind
                },
                field,
            )?;
        }
        let cursor = request.cursor.take();
        let mut hash = Sha256::new();
        hash.update(request.encode_to_vec());
        hash.update(assets::query_fingerprint(
            &api::Query {
                roots: working.roots.clone(),
                focus: working.focus.clone(),
                ..Default::default()
            },
            context,
        ));
        let fingerprint = format!("{:x}", hash.finalize());
        let revision = number(&tx.root, "revision")?;
        let after = match &cursor {
            Some(cursor) => {
                assets::page_position(cursor, revision, &fingerprint, context.authority_revision)?
            }
            None => "",
        };
        let (priority, key) = if after.is_empty() {
            (0, "")
        } else {
            let (priority, key) = after.split_once(',').ok_or(Error::Invalid)?;
            (priority.parse::<u64>().map_err(|_| Error::Invalid)?, key)
        };
        let mut probes = Vec::with_capacity(MAX_FIELDS + MAX_CLAIMS);
        for claim in &finding.identity {
            if claim.quality != api::HostIdentityQuality::Unreliable as i32 {
                probes.push(Value::Map(BTreeMap::from([
                    ("key".into(), text(lookup(claim))),
                    ("rank".into(), Value::Int(1)),
                ])));
            }
        }
        for field in &finding.fields {
            if let Some(probe) = descriptor(field) {
                probes.push(Value::Map(BTreeMap::from([
                    ("key".into(), text(probe)),
                    (
                        "rank".into(),
                        Value::Int(if field.name == "name" { 3 } else { 2 }),
                    ),
                ])));
            }
        }
        if probes.is_empty() {
            return Ok(reply(api::reply::Result::IdentityCandidates(
                api::IdentityCandidates::default(),
            )));
        }
        let mut values = HashMap::from([
            ("probes".into(), Value::Array(probes)),
            (
                "kinds".into(),
                Value::Array(if unclassified {
                    vec![
                        Value::Int(wire::AssetKind::Device as i32 as i64),
                        Value::Int(wire::AssetKind::VirtualMachine as i32 as i64),
                    ]
                } else {
                    vec![Value::Int(kind.into())]
                }),
            ),
            ("priority".into(), int(priority)?),
            ("after".into(), text(key)),
        ]);
        let readable = assets::visible_clause_for(context, None, &mut values, "a");
        let identity = assets::visible_clause_for(context, Some(FIELD), &mut values, "a");
        let mut permitted = Vec::with_capacity(DESCRIPTORS.len());
        for name in DESCRIPTORS {
            let field = assets::visible_clause_for(context, Some(name), &mut values, "a");
            permitted.push(format!("(n.field='{name}' AND {field})"));
        }
        let members = if let Some(focus) = selection.focus {
            let mut members = BTreeSet::from([focus]);
            members.extend(tx.local_children(context, focus)?);
            Some(members)
        } else if !working.roots.is_empty() {
            Some(tx.working_scope(context, &working.roots)?)
        } else {
            None
        };
        let scope = if let Some(members) = members {
            values.insert(
                "members".into(),
                Value::Array(members.into_iter().map(|v| text(asset_key(v))).collect()),
            );
            "a.key IN $members"
        } else {
            "true"
        };
        // The same current-field probe index serves all selected descriptors.
        // Rights and scope precede grouping, ordering and page selection.
        let rows = tx.query(&format!(
            "UNWIND $probes AS probe MATCH (n:OmRow {{claim:probe.key}}) MATCH (a:OmRow {{key:CASE WHEN probe.rank=1 THEN 'asset/'+n.asset ELSE n.parent END}}) WHERE a.parent='assets' AND a.kind IN $kinds AND a.adoption={} AND {readable} AND {scope} AND ((probe.rank=1 AND n.incarnation=a.incarnation AND n.parent=a.key+'/'+a.incarnation+'/identity-evidence' AND {identity}) OR (probe.rank>1 AND n.layer IN ['intent','observed','imported'] AND ({}))) WITH a,min(probe.rank) AS priority WHERE priority>$priority OR (priority=$priority AND a.key>$after) RETURN {{key:a.key, asset:a.asset, incarnation:a.incarnation, kind:a.kind, revision:a.revision, priority:priority}} AS p ORDER BY priority,a.key LIMIT 26",
            wire::Adoption::Adopted as i32, permitted.join(" OR ")
        ), Some(values))?;
        let more = rows.len() > 25;
        let mut parents = Vec::with_capacity(50);
        for row in rows.iter().take(25) {
            let target = super::super::super::target(row)?;
            parents.push(text(asset_key(target)));
            parents.push(text(parent(target)));
        }
        let facts = tx.query(
            "UNWIND $parents AS parent MATCH (n:OmRow {parent:parent}) WHERE (n.layer IN ['intent','observed','imported'] AND n.field IN $fields) OR n.parent ENDS WITH '/identity-evidence' RETURN properties(n) AS p LIMIT 2049",
            params([("parents", Value::Array(parents)), ("fields", Value::Array(DESCRIPTORS.iter().map(|v| text(*v)).collect()))]),
        )?;
        if facts.len() > MAX_CHILDREN {
            return Err(Error::Limit);
        }
        let mut items = Vec::with_capacity(rows.len().min(25));
        let mut explanations = Vec::with_capacity(rows.len().min(25));
        let mut position = String::new();
        for row in rows.iter().take(25) {
            let target = super::super::super::target(row)?;
            let kind = number(row, "kind")? as i32;
            selection.contains(target, &tx)?;
            let asset = asset_key(target);
            let identity_parent = parent(target);
            let mut item = api::Item {
                target: Some(target_wire(target)),
                kind,
                revision: number(row, "revision")?,
                fields: Vec::new(),
            };
            let mut identity = Vec::with_capacity(MAX_CLAIMS * current_facts::MAX_SOURCES);
            let mut observations = Vec::new();
            for fact in &facts {
                if fact.get("parent").and_then(Value::as_str) == Some(&identity_parent)
                    && context.field(target, kind, FIELD, false)
                {
                    identity.push(
                        api::HostIdentityClaim::decode(binary(fact, "payload")?)
                            .map_err(|_| Error::Corrupt)?,
                    );
                } else if fact.get("parent").and_then(Value::as_str) == Some(&asset) {
                    let name = string(fact, "field")?;
                    if !context.field(target, kind, &name, false) {
                        continue;
                    }
                    let field = inventory::read_field(fact)?;
                    if fact.get("layer").and_then(Value::as_str) == Some("intent") {
                        item.fields.push(field);
                    } else {
                        observations.push((field, string(fact, "source")?));
                    }
                }
            }
            let mut explanation = api::IdentityCandidateExplanation::default();
            for claim in &finding.identity {
                let same_namespace = identity
                    .iter()
                    .filter(|v| v.kind == claim.kind && v.namespace == claim.namespace)
                    .collect::<Vec<_>>();
                if same_namespace.is_empty() {
                    explanation.unknowns.push(format!(
                        "{}: no permitted same-layer identifier",
                        claim.namespace
                    ));
                } else {
                    if same_namespace.iter().any(|v| v.digest == claim.digest) {
                        explanation.matches.push(format!(
                            "{}: equal reported identifier; cloning or movement remains possible",
                            claim.namespace
                        ));
                    }
                    if same_namespace.iter().any(|v| v.digest != claim.digest) {
                        explanation.conflicts.push(format!(
                            "{}: different reported identifier",
                            claim.namespace
                        ));
                    }
                }
            }
            if finding.identity.is_empty() {
                explanation
                    .unknowns
                    .push("No permanent subject identifier in this finding".into());
            }
            for proposed in &finding.fields {
                if !DESCRIPTORS.contains(&proposed.name.as_str()) {
                    continue;
                }
                if !context.field(target, kind, &proposed.name, false) {
                    continue;
                }
                for (observed, source) in &observations {
                    if observed.name != proposed.name {
                        continue;
                    }
                    let message = format!(
                        "{}: source {} reports {:?}; inspect its collection window before assuming continuity",
                        observed.name, source, observed.value
                    );
                    if descriptor(proposed).is_none() || descriptor(observed).is_none() {
                        explanation.unknowns.push(message);
                    } else if descriptor(proposed) == descriptor(observed) {
                        explanation.matches.push(message);
                    } else {
                        explanation.conflicts.push(message);
                    }
                }
                if let Some(accepted) = item.fields.iter().find(|v| v.name == proposed.name) {
                    if descriptor(proposed).is_none() || descriptor(accepted).is_none() {
                        explanation.unknowns.push(format!(
                            "{}: current evidence is not an exact comparable value",
                            proposed.name
                        ));
                    } else if descriptor(proposed) == descriptor(accepted) {
                        explanation.matches.push(format!(
                            "{}: same current declared value, not identity proof",
                            proposed.name
                        ));
                    } else {
                        explanation.conflicts.push(format!("{}: different declared value; replacement, upgrade or reinstall may explain it", proposed.name));
                    }
                } else {
                    explanation
                        .unknowns
                        .push(format!("{}: no current declared value", proposed.name));
                }
            }
            position = format!("{},{}", number(row, "priority")?, string(row, "key")?);
            items.push(item);
            explanations.push(explanation);
        }
        let response = reply(api::reply::Result::IdentityCandidates(
            api::IdentityCandidates {
                items,
                explanations,
                next: more.then(|| {
                    format!(
                        "{revision}:{fingerprint}:{}:{position}",
                        context.authority_revision
                    )
                }),
            },
        ));
        if response.encoded_len() > MAX_REPLY_BYTES {
            return Err(Error::Limit);
        }
        Ok(response)
    }
}
