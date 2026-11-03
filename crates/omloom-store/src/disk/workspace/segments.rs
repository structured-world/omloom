//! A membership is one attributed relation, refined in place when its interface is known.
use super::*;
mod projection;

enum ParticipationChoice {
    One(AssetRef),
    Several,
}

fn summary(context: &Context, target: AssetRef, row: &Properties) -> Result<api::Item> {
    let kind = number(row, "kind")? as i32;
    let fields = if context.field(target, kind, "name", false) {
        row.get("name")
            .and_then(Value::as_str)
            .map(|name| wire::Field {
                name: "name".into(),
                value: Some(field::Value::Text(name.into())),
            })
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    Ok(api::Item {
        target: Some(target_wire(target)),
        kind,
        revision: number(row, "revision")?,
        fields,
    })
}

impl Store {
    pub(super) fn workspace_segment(
        &self,
        context: &Context,
        request: api::Segment,
        working: &api::WorkingContext,
        now: u64,
        preview: bool,
    ) -> Result<api::Reply> {
        if request.participants.is_empty() || request.participants.len() >= MAX_BULK {
            return Err(Error::Limit);
        }
        if request.name.as_ref().is_some_and(|name| name.len() > 160) {
            return Err(Error::Limit);
        }
        let mut tx = self.begin()?;
        let input = request.encode_to_vec();
        if !preview
            && let Some(saved) = tx.creation_outcome(
                context,
                working,
                &input,
                addresses::knowledge::Outcome::Revision("segment"),
            )?
        {
            return Ok(reply(api::reply::Result::Mutation(saved)));
        }
        if !preview && working.expected != Some(number(&tx.root, "revision")?) {
            return Err(Error::Conflict);
        }
        let review = tx.prepare_segment(context, &request, working)?;
        if preview {
            tx.commit()?;
            return Ok(reply(api::reply::Result::SegmentReview(review)));
        }
        let targets = tx.apply_segment(context, request, &review, Vec::new(), now)?;
        if tx.changed {
            tx.save_creation_outcome(
                context,
                working,
                &input,
                &targets,
                now,
                addresses::knowledge::Outcome::Revision("segment"),
            )?;
        }
        Ok(mutation(tx.commit()?, targets))
    }
}

impl Tx<'_> {
    pub(super) fn import_segment(
        &mut self,
        context: &Context,
        mut write: api::Write,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<AssetRef> {
        if write.parent.is_some() {
            return Err(Error::Invalid);
        }
        let mut participants = None;
        let mut name = None;
        let mut metadata = Vec::new();
        for change in write.changes.drain(..) {
            match change.name.as_str() {
                "participants" => {
                    if participants.is_some() {
                        return Err(Error::Invalid);
                    }
                    let Some(field::Value::Text(value)) = change.value else {
                        return Err(Error::Invalid);
                    };
                    participants = Some(csv_participants(&value)?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(Error::Invalid);
                    }
                    name = Some(match change.value {
                        Some(field::Value::Text(value)) => value,
                        Some(field::Value::Clear(EXPLICIT_CLEAR)) => String::new(),
                        _ => return Err(Error::Invalid),
                    });
                }
                _ => metadata.push(change),
            }
        }
        let request = api::Segment {
            target: write.target,
            name,
            participants: participants.ok_or(Error::Invalid)?,
        };
        let review = self.prepare_segment(context, &request, working)?;
        if write.expected.is_some()
            && write.expected != review.segment.as_ref().map(|item| item.revision)
        {
            return Err(Error::Conflict);
        }
        self.apply_segment(context, request, &review, metadata, now)?
            .last()
            .map(|v| parse_target(Some(v)))
            .ok_or(Error::Corrupt)?
    }

    fn prepare_segment(
        &self,
        context: &Context,
        request: &api::Segment,
        working: &api::WorkingContext,
    ) -> Result<api::SegmentReview> {
        if request.participants.is_empty() || request.participants.len() >= MAX_BULK {
            return Err(Error::Limit);
        }
        if let Some(name) = &request.name {
            self.validate_field(
                wire::AssetKind::L2Domain as i32,
                &wire::Field {
                    name: "name".into(),
                    value: Some(if name.is_empty() {
                        field::Value::Clear(EXPLICIT_CLEAR)
                    } else {
                        field::Value::Text(name.clone())
                    }),
                },
            )?;
        }
        let tx = self;
        let selection = tx.selection(context, working, false)?;
        let existing = request
            .target
            .as_ref()
            .map(|target| parse_target(Some(target)))
            .transpose()?;
        let segment = existing.unwrap_or_else(new_target);
        let mut keys = BTreeSet::new();
        if let Some(target) = existing {
            keys.insert(asset_key(target));
        }
        let mut subjects = BTreeSet::new();
        for participant in &request.participants {
            let subject = parse_target(participant.subject.as_ref())?;
            if !subjects.insert(subject) {
                return Err(Error::Invalid);
            }
            keys.insert(asset_key(subject));
            if let Some(interface) = participant.interface.as_ref() {
                keys.insert(asset_key(parse_target(Some(interface))?));
            }
        }
        let rows = tx.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
            params([("keys", Value::Array(keys.iter().map(text).collect()))]),
        )?;
        let rows: BTreeMap<_, _> = rows
            .iter()
            .map(|row| Ok((string(row, "key")?, row)))
            .collect::<Result<_>>()?;
        let checked = |target: AssetRef, kind: wire::AssetKind| -> Result<&Properties> {
            let row = *rows.get(&asset_key(target)).ok_or(Error::Missing)?;
            tx.require_visible_asset(context, target, row)?;
            if number(row, "kind")? != kind as u64 {
                return Err(Error::Invalid);
            }
            for write in [false, true] {
                if !context.field(target, kind as i32, "relations", write) {
                    return Err(Error::Denied);
                }
            }
            Ok(row)
        };
        let segment_item = if let Some(target) = existing {
            let row = checked(target, wire::AssetKind::L2Domain)?;
            selection.contains(target, tx)?;
            Some(summary(context, target, row)?)
        } else {
            if !context.field(segment, wire::AssetKind::L2Domain as i32, "network", true)
                || !context.field(segment, wire::AssetKind::L2Domain as i32, "relations", true)
            {
                return Err(Error::Denied);
            }
            None
        };
        if request.name.is_some()
            && !context.field(segment, wire::AssetKind::L2Domain as i32, "name", true)
        {
            return Err(Error::Denied);
        }
        let mut reviewed = Vec::with_capacity(request.participants.len());
        let mut subject_items = BTreeMap::new();
        for subject in &subjects {
            let row = checked(*subject, wire::AssetKind::VirtualMachine)?;
            selection.contains(*subject, tx)?;
            subject_items.insert(*subject, summary(context, *subject, row)?);
        }
        let interfaces = if existing.is_some()
            || request
                .participants
                .iter()
                .any(|p| p.interface.is_some() || p.relation_id.is_some())
        {
            tx.segment_sources(context, &subject_items)?
        } else {
            BTreeMap::new()
        };
        let authority = tx
            .get("workspace/authority")?
            .as_ref()
            .map(|row| string(row, "authority"))
            .transpose()?;
        for participant in &request.participants {
            let subject = parse_target(participant.subject.as_ref())?;
            let interface = participant
                .interface
                .as_ref()
                .map(|target| parse_target(Some(target)))
                .transpose()?;
            let interface_item = if let Some(interface) = interface {
                let row = checked(interface, wire::AssetKind::Interface)?;
                if interfaces.get(&interface).map(|owner| owner.1) != Some(subject) {
                    return Err(Error::Denied);
                }
                Some(summary(context, interface, row)?)
            } else {
                None
            };
            let binding = if let Some(id) = participant.relation_id.as_ref() {
                let id: RelationId = id.parse().map_err(|_| Error::Invalid)?;
                let row = tx.get(&format!("relation/{id}"))?.ok_or(Error::Missing)?;
                let relation = inventory::relation_projection(&row)?;
                if existing.is_none()
                    || relation.kind != wire::RelationKind::On as i32
                    || relation.to.as_ref() != request.target.as_ref()
                {
                    return Err(Error::Conflict);
                }
                let from = parse_target(relation.from.as_ref())?;
                if from != subject {
                    let row = tx.visible_asset(context, from)?;
                    if number(&row, "kind")? != wire::AssetKind::Interface as u64
                        || !context.field(
                            from,
                            wire::AssetKind::Interface as i32,
                            "relations",
                            true,
                        )
                    {
                        return Err(Error::Denied);
                    }
                    if interfaces.get(&from).map(|owner| owner.1) != Some(subject) {
                        return Err(Error::Denied);
                    }
                }
                if Some(relation.authority_id.as_str()) != authority.as_deref() {
                    return Err(Error::Conflict);
                }
                Some(relation)
            } else {
                None
            };
            reviewed.push(api::SegmentParticipantView {
                subject: Some(subject_items.remove(&subject).ok_or(Error::Corrupt)?),
                interface: interface_item,
                relation: binding,
            });
        }
        if let Some(segment) = existing {
            let ids: BTreeSet<_> = interfaces
                .keys()
                .map(|target| target.asset.to_string())
                .collect();
            let rows = tx.query("UNWIND $ids AS id MATCH (r:OmRow {asset:id}) WHERE r.parent='relations' AND r.to_asset=$to AND r.to_incarnation=$incarnation AND r.kind=$kind RETURN properties(r) AS p LIMIT 1025", params([("ids", Value::Array(ids.iter().map(text).collect())), ("to",text(segment.asset.to_string())),("incarnation",text(segment.incarnation.to_string())),("kind",int(wire::RelationKind::On as u64)?)]))?;
            if rows.len() > 1024 {
                return Err(Error::Limit);
            }
            let mut prior = BTreeMap::new();
            for row in &rows {
                let relation = inventory::relation_projection(row)?;
                if prior
                    .insert(parse_target(relation.from.as_ref())?, relation)
                    .is_some()
                {
                    return Err(Error::Conflict);
                }
            }
            let mut choices = BTreeMap::new();
            for source in prior.keys() {
                if let Some((_, owner)) = interfaces.get(source) {
                    let vm = *owner;
                    match choices.entry(vm) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(ParticipationChoice::One(*source));
                        }
                        std::collections::btree_map::Entry::Occupied(mut entry) => {
                            entry.insert(ParticipationChoice::Several);
                        }
                    }
                }
            }
            for participant in &mut reviewed {
                if participant.relation.is_none() && participant.interface.is_none() {
                    let subject = participant
                        .subject
                        .as_ref()
                        .and_then(|item| item.target.as_ref())
                        .ok_or(Error::Corrupt)?;
                    let subject = parse_target(Some(subject))?;
                    if let Some(choice) = choices.get(&subject) {
                        let ParticipationChoice::One(source) = choice else {
                            return Err(Error::Conflict);
                        };
                        let (source_item, _) = interfaces.get(source).ok_or(Error::Corrupt)?;
                        if !context.field(*source, source_item.kind, "relations", true) {
                            return Err(Error::Denied);
                        }
                        if source_item.kind == wire::AssetKind::Interface as i32 {
                            participant.interface = Some(source_item.clone());
                        }
                    }
                }
                let source = parse_target(
                    participant
                        .interface
                        .as_ref()
                        .or(participant.subject.as_ref())
                        .and_then(|item| item.target.as_ref()),
                )?;
                if let Some(relation) = prior.remove(&source) {
                    if Some(relation.authority_id.as_str()) != authority.as_deref()
                        || participant
                            .relation
                            .as_ref()
                            .is_some_and(|previous| previous.relation_id != relation.relation_id)
                    {
                        return Err(Error::Conflict);
                    }
                    participant.relation = Some(relation);
                }
            }
        }
        Ok(api::SegmentReview {
            revision: number(&tx.root, "revision")?,
            segment: segment_item,
            participants: reviewed,
        })
    }

    fn apply_segment(
        &mut self,
        context: &Context,
        request: api::Segment,
        review: &api::SegmentReview,
        metadata: Vec<wire::Field>,
        now: u64,
    ) -> Result<Vec<wire::AssetRef>> {
        let tx = self;
        let existing = request
            .target
            .as_ref()
            .map(|target| parse_target(Some(target)))
            .transpose()?;
        let authority = tx.workspace_authority()?;
        let segment = if existing.is_none() || request.name.is_some() || !metadata.is_empty() {
            let changes = request
                .name
                .as_ref()
                .map(|name| wire::Field {
                    name: "name".into(),
                    value: Some(if name.is_empty() {
                        field::Value::Clear(EXPLICIT_CLEAR)
                    } else {
                        field::Value::Text(name.clone())
                    }),
                })
                .into_iter()
                .chain(metadata)
                .collect();
            tx.workspace_network_write(
                context,
                api::Write {
                    target: request.target.clone(),
                    expected: review.segment.as_ref().map(|item| item.revision),
                    kind: wire::AssetKind::L2Domain as i32,
                    changes,
                    parent: None,
                },
                now,
            )?
        } else {
            existing.ok_or(Error::Invalid)?
        };
        let mut subjects = BTreeSet::new();
        for participant in &review.participants {
            let subject =
                parse_target(participant.subject.as_ref().and_then(|v| v.target.as_ref()))?;
            subjects.insert(subject);
            let from = participant
                .interface
                .as_ref()
                .and_then(|v| v.target.as_ref())
                .map(|v| parse_target(Some(v)))
                .transpose()?
                .unwrap_or(subject);
            let prior = participant.relation.as_ref();
            let relation = wire::Relation {
                relation_id: prior
                    .map(|r| r.relation_id.clone())
                    .unwrap_or_else(|| RelationId::generate().to_string()),
                from: Some(target_wire(from)),
                to: Some(target_wire(segment)),
                kind: wire::RelationKind::On as i32,
                revision: prior.map_or(1, |r| r.revision),
                authority_id: authority.clone(),
            };
            tx.save_typed_relation(&relation, prior.map(|r| r.revision), &context.actor, now)?;
        }
        let mut targets: Vec<_> = subjects.into_iter().map(target_wire).collect();
        targets.push(target_wire(segment));
        Ok(targets)
    }
}

/// CSV carries only explicit qualified references, never names or inferred interfaces.
pub(super) fn csv_participants(value: &str) -> Result<Vec<api::SegmentParticipant>> {
    if value.is_empty() || value.len() > 32_768 || value.split(';').count() >= MAX_BULK {
        return Err(Error::Limit);
    }
    fn reference(value: &str) -> Result<wire::AssetRef> {
        let (asset, incarnation) = value.split_once('/').ok_or(Error::Invalid)?;
        let target = wire::AssetRef {
            asset_id: asset.into(),
            incarnation: incarnation.into(),
        };
        parse_target(Some(&target))?;
        Ok(target)
    }
    value
        .split(';')
        .map(|value| {
            let (value, relation_id) = match value.split_once('#') {
                Some((value, id)) => {
                    id.parse::<RelationId>().map_err(|_| Error::Invalid)?;
                    (value, Some(id.to_owned()))
                }
                None => (value, None),
            };
            let (subject, interface) = match value.split_once('@') {
                Some((subject, interface)) => (subject, Some(reference(interface)?)),
                None => (value, None),
            };
            Ok(api::SegmentParticipant {
                subject: Some(reference(subject)?),
                interface,
                relation_id,
            })
        })
        .collect()
}
