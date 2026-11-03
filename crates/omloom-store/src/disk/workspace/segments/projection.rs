use super::*;

impl Tx<'_> {
    fn segment_subjects(
        &self,
        context: &Context,
        rows: &[Properties],
    ) -> Result<BTreeMap<AssetRef, api::Item>> {
        let mut origins = BTreeMap::new();
        let mut frontier = BTreeSet::new();
        let mut nodes = BTreeMap::new();
        let mut owners: BTreeMap<AssetRef, BTreeSet<AssetRef>> = BTreeMap::new();
        for row in rows {
            let root = target(row)?;
            origins.insert(root, BTreeSet::from([root]));
            if number(row, "kind")? == wire::AssetKind::VirtualMachine as u64 {
                nodes.insert(root, row.clone());
                owners.entry(root).or_default().insert(root);
            } else {
                frontier.insert(asset_key(root));
            }
        }
        let mut visited = frontier.clone();
        while !frontier.is_empty() {
            let mut next = BTreeSet::new();
            for (child, parent, _) in self.context_edges(context, &frontier, false)? {
                let child = target(&child)?;
                let target = target(&parent)?;
                let inherited = origins.get(&child).ok_or(Error::Corrupt)?.clone();
                let kind = number(&parent, "kind")? as i32;
                if kind == wire::AssetKind::VirtualMachine as i32 {
                    for origin in inherited {
                        owners.entry(origin).or_default().insert(target);
                    }
                    nodes.insert(target, parent);
                } else if matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::Interface
                        | wire::AssetKind::SoftwareInstallation
                        | wire::AssetKind::Service
                        | wire::AssetKind::Container
                        | wire::AssetKind::Workload)
                ) {
                    let entry = origins.entry(target).or_default();
                    let before = entry.len();
                    entry.extend(inherited);
                    if entry.len() != before {
                        next.insert(asset_key(target));
                    }
                    visited.insert(asset_key(target));
                    if visited.len() > 1024 {
                        return Err(Error::Limit);
                    }
                }
            }
            frontier = next;
        }
        let mut subjects = BTreeMap::new();
        for (source, targets) in owners {
            if targets.len() == 1 {
                let target = *targets.first().ok_or(Error::Corrupt)?;
                subjects.insert(
                    source,
                    summary(context, target, nodes.get(&target).ok_or(Error::Corrupt)?)?,
                );
            }
        }
        Ok(subjects)
    }

    pub(super) fn segment_sources(
        &self,
        context: &Context,
        vms: &BTreeMap<AssetRef, api::Item>,
    ) -> Result<BTreeMap<AssetRef, (api::Item, AssetRef)>> {
        let mut sources = BTreeMap::new();
        for (vm, item) in vms {
            if item.kind != wire::AssetKind::VirtualMachine as i32
                || !context.field(
                    *vm,
                    wire::AssetKind::VirtualMachine as i32,
                    "relations",
                    false,
                )
            {
                return Err(Error::Denied);
            }
            sources.insert(*vm, (item.clone(), *vm));
        }
        let mut frontier: BTreeSet<_> = sources.keys().map(|t| asset_key(*t)).collect();
        let mut visited = frontier.clone();
        let mut owner_by_key: BTreeMap<_, _> = sources
            .iter()
            .map(|(target, item)| (asset_key(*target), item.1))
            .collect();
        while !frontier.is_empty() {
            let mut next = BTreeSet::new();
            for (parent, child, _) in self.context_edges(context, &frontier, true)? {
                let child_target = target(&child)?;
                let kind = number(&child, "kind")? as i32;
                // A nested VM is a different participant. Only its own selected
                // root may claim its interfaces; lateral On links are never followed.
                if !matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::Interface
                        | wire::AssetKind::SoftwareInstallation
                        | wire::AssetKind::Service
                        | wire::AssetKind::Workload
                        | wire::AssetKind::Container)
                ) {
                    continue;
                }
                let key = asset_key(child_target);
                let owner = *owner_by_key
                    .get(&asset_key(target(&parent)?))
                    .ok_or(Error::Corrupt)?;
                if let Some(prior) = owner_by_key.get(&key)
                    && *prior != owner
                {
                    return Err(Error::Conflict);
                }
                owner_by_key.insert(key.clone(), owner);
                if visited.insert(key.clone()) {
                    if visited.len() > 1024 {
                        return Err(Error::Limit);
                    }
                    if kind == wire::AssetKind::Interface as i32 {
                        sources.insert(
                            child_target,
                            (summary(context, child_target, &child)?, owner),
                        );
                    }
                    next.insert(key);
                }
            }
            frontier = next;
        }
        Ok(sources)
    }
}

impl Store {
    pub(in super::super) fn workspace_segment_interfaces(
        &self,
        context: &Context,
        request: api::SegmentInterfaces,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        if request.search.len() > 256 {
            return Err(Error::Limit);
        }
        let subject = parse_target(request.subject.as_ref())?;
        let tx = self.begin()?;
        tx.selection(context, working, false)?
            .contains(subject, &tx)?;
        let revision = number(&tx.root, "revision")?;
        let mut selector = request.clone();
        selector.cursor = None;
        let mut hash = Sha256::new();
        hash.update(selector.encode_to_vec());
        hash.update(working.encode_to_vec());
        hash.update(context.actor.as_bytes());
        let fingerprint = format!("{:x}", hash.finalize());
        let offset = request
            .cursor
            .as_deref()
            .map(|cursor| {
                assets::page_position(cursor, revision, &fingerprint, context.authority_revision)?
                    .parse::<usize>()
                    .map_err(|_| Error::Invalid)
            })
            .transpose()?
            .unwrap_or(0);
        if offset > 1024 {
            return Err(Error::Limit);
        }
        let row = tx.visible_asset(context, subject)?;
        let sources = tx.segment_sources(
            context,
            &BTreeMap::from([(subject, summary(context, subject, &row)?)]),
        )?;
        let search = request.search.to_lowercase();
        let mut candidates = Vec::new();
        for (source, _) in sources.values() {
            if source.kind != wire::AssetKind::Interface as i32 {
                continue;
            }
            let name = source
                .fields
                .first()
                .and_then(|f| match &f.value {
                    Some(field::Value::Text(v)) => Some(v.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            let normalized = name.to_lowercase();
            if !search.is_empty() && !normalized.contains(&search) {
                continue;
            }
            candidates.push((normalized, source));
        }
        candidates.sort_by(|(a, x), (b, y)| {
            a.cmp(b).then_with(|| {
                x.target
                    .as_ref()
                    .map(|t| &t.asset_id)
                    .cmp(&y.target.as_ref().map(|t| &t.asset_id))
            })
        });
        let total = candidates.len();
        let items = candidates
            .into_iter()
            .skip(offset)
            .take(25)
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>();
        let next = if offset + items.len() < total {
            Some(format!(
                "{revision}:{fingerprint}:{}:{}",
                context.authority_revision,
                offset + items.len()
            ))
        } else {
            None
        };
        // Only this candidate page receives owner/address hints. They are not
        // loaded for segment lists or unrelated VMs.
        let rows = tx.query(
            "UNWIND $keys AS key MATCH (a:OmRow {key:key}) RETURN properties(a) AS p",
            params([(
                "keys",
                Value::Array(
                    items
                        .iter()
                        .map(|i| parse_target(i.target.as_ref()).map(|t| text(asset_key(t))))
                        .collect::<Result<_>>()?,
                ),
            )]),
        )?;
        let contexts = tx.item_contexts(
            context,
            &rows,
            &api::Query {
                roots: vec![target_wire(subject)],
                ..Default::default()
            },
        )?;
        tx.commit()?;
        Ok(reply(api::reply::Result::Page(api::Page {
            items,
            total: total as u64,
            revision,
            next,
            contexts,
        })))
    }

    pub(in super::super) fn workspace_segment_memberships(
        &self,
        context: &Context,
        request: api::SegmentMemberships,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        if request.targets.is_empty() || request.targets.len() >= MAX_BULK {
            return Err(Error::Limit);
        }
        let tx = self.begin()?;
        let selection = tx.selection(context, working, false)?;
        let revision = number(&tx.root, "revision")?;
        let mut fingerprint = Sha256::new();
        fingerprint.update(working.encode_to_vec());
        fingerprint.update(context.actor.as_bytes());
        let mut selectors = request.clone();
        selectors.cursor = None;
        fingerprint.update(selectors.encode_to_vec());
        let fingerprint = format!("{:x}", fingerprint.finalize());
        let after = request
            .cursor
            .as_deref()
            .map(|cursor| {
                assets::page_position(cursor, revision, &fingerprint, context.authority_revision)
            })
            .transpose()?
            .unwrap_or("");
        let mut vms = BTreeMap::new();
        let mut domains = BTreeSet::new();
        for value in &request.targets {
            let target = parse_target(Some(value))?;
            selection.contains(target, &tx)?;
            let row = tx.visible_asset(context, target)?;
            let kind = number(&row, "kind")? as i32;
            if !context.field(target, kind, "relations", false) {
                return Err(Error::Denied);
            }
            match wire::AssetKind::try_from(kind) {
                Ok(wire::AssetKind::VirtualMachine) => {
                    vms.insert(target, summary(context, target, &row)?);
                }
                Ok(wire::AssetKind::L2Domain) => {
                    domains.insert(asset_key(target));
                }
                _ => return Err(Error::Invalid),
            }
        }
        let sources = tx.segment_sources(context, &vms)?;
        let source_keys: BTreeSet<_> = sources.keys().map(|t| asset_key(*t)).collect();
        let mut values = HashMap::new();
        let visible_from = assets::visible_clause_for(context, Some("relations"), &mut values, "a");
        let visible_to = assets::visible_clause_for(context, Some("relations"), &mut values, "b");
        values.insert("after".into(), text(after));
        values.insert("kind".into(), int(wire::RelationKind::On as u64)?);
        // Both input cohorts enter through indexed keys and native adjacency.
        // The union is explicit and bounded; no estate-wide asset scan is needed.
        let mut rows = BTreeMap::new();
        for (keys, path) in [
            (
                &source_keys,
                "(a:OmRow {key:key})-[:OmRelationFrom]->(r:OmRow)-[:OmRelationTo]->(b:OmRow)",
            ),
            (
                &domains,
                "(b:OmRow {key:key})<-[:OmRelationTo]-(r:OmRow)<-[:OmRelationFrom]-(a:OmRow)",
            ),
        ] {
            if keys.is_empty() {
                continue;
            }
            let mut parameters = values.clone();
            parameters.insert("keys".into(), Value::Array(keys.iter().map(text).collect()));
            let found = tx.raw(&format!("UNWIND $keys AS key MATCH {path} WHERE r.kind=$kind AND r.key>$after AND a.parent='assets' AND b.parent='assets' AND a.incarnation=r.incarnation AND b.incarnation=r.to_incarnation AND a.adoption={} AND b.adoption={} AND (a.kind={} OR a.kind={}) AND b.kind={} AND {visible_from} AND {visible_to} RETURN DISTINCT properties(a) AS a, properties(b) AS b, properties(r) AS r ORDER BY r.key LIMIT 26", wire::Adoption::Adopted as i32, wire::Adoption::Adopted as i32, wire::AssetKind::VirtualMachine as i32, wire::AssetKind::Interface as i32, wire::AssetKind::L2Domain as i32), parameters)?;
            for mut value in found {
                let (Some(Value::Map(a)), Some(Value::Map(b)), Some(Value::Map(r))) =
                    (value.remove("a"), value.remove("b"), value.remove("r"))
                else {
                    return Err(Error::Corrupt);
                };
                rows.insert(string(&r, "key")?, (a, b, r));
            }
        }
        let more = rows.len() > 25;
        let mut unknown_sources = Vec::new();
        for (row, _, _) in rows.values().take(25) {
            if !sources.contains_key(&target(row)?) {
                unknown_sources.push(row.clone());
            }
        }
        let subjects = tx.segment_subjects(context, &unknown_sources)?;
        let authority = if rows.is_empty() {
            None
        } else {
            tx.get("workspace/authority")?
                .as_ref()
                .map(|row| string(row, "authority"))
                .transpose()?
        };
        let mut entries = Vec::with_capacity(rows.len().min(25));
        let mut last = None;
        for (key, (a, b, r)) in rows.into_iter().take(25) {
            let source = target(&a)?;
            let subject = sources
                .get(&source)
                .and_then(|item| sources.get(&item.1))
                .map(|item| item.0.clone())
                .or_else(|| subjects.get(&source).cloned());
            let relation = inventory::relation_projection(&r)?;
            let refinable = if let Some(subject) = &subject {
                context.can_edit_relations(
                    parse_target(subject.target.as_ref())?,
                    wire::AssetKind::VirtualMachine as i32,
                ) && context.can_edit_relations(source, number(&a, "kind")? as i32)
                    && context.can_edit_relations(target(&b)?, wire::AssetKind::L2Domain as i32)
                    && authority.as_deref() == Some(relation.authority_id.as_str())
            } else {
                false
            };
            entries.push(api::SegmentMembership {
                source: Some(summary(context, source, &a)?),
                segment: Some(summary(context, target(&b)?, &b)?),
                relation: Some(relation),
                subject,
                refinable,
            });
            last = Some(key);
        }
        let next = if more {
            Some(format!(
                "{revision}:{fingerprint}:{}:{}",
                context.authority_revision,
                last.ok_or(Error::Corrupt)?
            ))
        } else {
            None
        };
        tx.commit()?;
        Ok(reply(api::reply::Result::SegmentMembershipPage(
            api::SegmentMembershipPage {
                entries,
                revision,
                next,
            },
        )))
    }
}
