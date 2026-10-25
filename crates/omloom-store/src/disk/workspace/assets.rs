use super::*;

fn parameter(values: &mut HashMap<String, Value>, value: Value) -> String {
    let name = format!("p{}", values.len());
    values.insert(name.clone(), value);
    format!("${name}")
}
fn scope_clause(scope: &Scope, values: &mut HashMap<String, Value>) -> String {
    match scope {
        Scope::All => "true".into(),
        Scope::Kinds(kinds) => format!(
            "a.kind IN {}",
            parameter(
                values,
                Value::Array(kinds.iter().map(|k| Value::Int(*k as i32 as i64)).collect())
            )
        ),
        Scope::Targets(targets) => {
            let clauses: Vec<_> = targets
                .iter()
                .map(|t| {
                    format!(
                        "(a.key={} AND a.incarnation={})",
                        parameter(values, text(asset_key(*t))),
                        parameter(values, text(t.incarnation.to_string()))
                    )
                })
                .collect();
            if clauses.is_empty() {
                "false".into()
            } else {
                format!("({})", clauses.join(" OR "))
            }
        }
    }
}
fn visible_clause(
    context: &Context,
    field: Option<&str>,
    values: &mut HashMap<String, Value>,
) -> String {
    let clauses: Vec<_> = context
        .grants
        .iter()
        .filter(|g| field.is_none_or(|f| g.read.contains(f)))
        .map(|g| scope_clause(&g.scope, values))
        .collect();
    if clauses.is_empty() {
        "false".into()
    } else {
        format!("({})", clauses.join(" OR "))
    }
}
fn kind_for_view(view: api::View) -> &'static [wire::AssetKind] {
    use wire::AssetKind as K;
    match view {
        api::View::Overview => &[],
        api::View::Hardware => &[
            K::Device,
            K::Port,
            K::Cable,
            K::Rack,
            K::Site,
            K::Location,
            K::PowerFeed,
        ],
        api::View::Network => &[
            K::Interface,
            K::IpAddress,
            K::Prefix,
            K::Vlan,
            K::Vrf,
            K::AddressSpace,
            K::L2Domain,
            K::Port,
            K::Cable,
            K::Lag,
        ],
        api::View::Virtualization => &[K::VirtualMachine, K::Device, K::Cluster, K::Storage],
        api::View::Services => &[K::Service, K::Device, K::VirtualMachine, K::Storage],
    }
}
fn value_clause(
    value: &wire::Field,
    alias: &str,
    values: &mut HashMap<String, Value>,
) -> Result<String> {
    let (kind, column, value) = match value.value.as_ref().ok_or(Error::Invalid)? {
        field::Value::Text(s) => (1, "vtext", text(s)),
        field::Value::Integer(n) => (2, "vint", Value::Int(*n)),
        field::Value::Flag(b) => (3, "vflag", Value::Bool(*b)),
        field::Value::Clear(0) => return Ok(format!("{alias}.vtype=4")),
        _ => return Err(Error::Invalid),
    };
    Ok(format!(
        "{alias}.vtype={kind} AND {alias}.{column}={}",
        parameter(values, value)
    ))
}

impl Store {
    pub(super) fn workspace_query(
        &self,
        context: &Context,
        mut query: api::Query,
    ) -> Result<api::Page> {
        if query.search.len() > MAX_QUERY
            || query.roots.len() > 128
            || query.kinds.len() > 32
            || query.columns.len() > MAX_FIELDS
            || query.predicates.len() > 8
            || query.limit == 0
            || query.limit as usize > MAX_PAGE
        {
            return Err(Error::Invalid);
        }
        let view = api::View::try_from(query.view).map_err(|_| Error::Invalid)?;
        let basis = self.usage()?.revision;
        if query.columns.is_empty() {
            query.columns.push("name".into());
        }
        let definitions = self.workspace_definitions(context)?;
        for field in &query.columns {
            validate_column(field, &definitions)?;
        }
        let tx = self.begin()?;
        let revision = number(&tx.root, "revision")?;
        if revision != basis {
            return Err(Error::Expired);
        }
        let fingerprint = query_fingerprint(&query, context);
        let offset = match &query.cursor {
            None => 0u64,
            Some(cursor) => {
                if cursor.len() > 256 {
                    return Err(Error::Invalid);
                }
                let parts: Vec<_> = cursor.split(':').collect();
                if parts.len() != 4
                    || parts[0].parse::<u64>().ok() != Some(revision)
                    || parts[1] != fingerprint
                    || parts[2].parse::<u64>().ok() != Some(context.authority_revision)
                {
                    return Err(Error::Expired);
                }
                parts[3].parse().map_err(|_| Error::Invalid)?
            }
        };
        if offset > 100_000 {
            return Err(Error::Limit);
        }
        let mut values = HashMap::new();
        let mut clauses = vec![
            "a.parent='assets'".to_owned(),
            visible_clause(context, None, &mut values),
        ];
        let mut matches = vec!["(a:OmRow)".to_owned()];
        let view_kinds = kind_for_view(view);
        if !view_kinds.is_empty() {
            clauses.push(format!(
                "a.kind IN {}",
                parameter(
                    &mut values,
                    Value::Array(
                        view_kinds
                            .iter()
                            .map(|k| Value::Int(*k as i32 as i64))
                            .collect()
                    )
                )
            ));
        }
        if !query.kinds.is_empty() {
            for kind in &query.kinds {
                if wire::AssetKind::try_from(*kind)
                    .ok()
                    .filter(|k| *k != wire::AssetKind::Unspecified)
                    .is_none()
                {
                    return Err(Error::Invalid);
                }
            }
            clauses.push(format!(
                "a.kind IN {}",
                parameter(
                    &mut values,
                    Value::Array(
                        query
                            .kinds
                            .iter()
                            .map(|n| Value::Int((*n).into()))
                            .collect()
                    )
                )
            ));
        }
        if !query.roots.is_empty() {
            let roots = self.workspace_scope(context, &query.roots)?;
            clauses.push(format!(
                "a.key IN {}",
                parameter(
                    &mut values,
                    Value::Array(roots.iter().map(|t| text(asset_key(*t))).collect())
                )
            ));
        }
        if query.unplaced {
            // An accepted physical placement alone removes an object from Unplaced.
            clauses.push("NOT EXISTS { MATCH (a)-[:OmRelationFrom]->(placement:OmRow) WHERE placement.kind=1 }".into());
        }
        for (index, predicate) in query.predicates.iter().enumerate() {
            validate_predicate(predicate, &definitions)?;
            let alias = format!("f{index}");
            matches.push(format!("({alias}:OmRow)"));
            clauses.push(format!(
                "{alias}.parent=a.key AND {alias}.layer='intent' AND {alias}.field={}",
                parameter(&mut values, text(&predicate.field))
            ));
            clauses.push(visible_clause(context, Some(&predicate.field), &mut values));
            clauses.push(value_clause(
                predicate.value.as_ref().ok_or(Error::Invalid)?,
                &alias,
                &mut values,
            )?);
        }
        if !query.search.trim().is_empty() {
            let term = query.search.trim().to_lowercase();
            if term.parse::<AssetId>().is_ok() {
                clauses.push(format!("a.asset={}", parameter(&mut values, text(&term))));
            } else if let Ok(ip) = term.parse::<std::net::IpAddr>() {
                matches.push("(ip:OmRow)".into());
                clauses.push(format!("ip.parent='networks' AND ip.asset=a.asset AND ip.incarnation=a.incarnation AND ip.cidr STARTS WITH {}", parameter(&mut values, text(format!("{ip}/")))));
                clauses.push(visible_clause(context, Some("cidr"), &mut values));
            } else {
                let mut searchable = vec![
                    "name".to_owned(),
                    "description".to_owned(),
                    "serial".to_owned(),
                ];
                searchable.extend(
                    definitions
                        .iter()
                        .filter(|d| d.searchable && !d.retired)
                        .map(|d| d.id.clone()),
                );
                matches.push("(s:OmRow)".into());
                clauses.push("s.parent=a.key AND s.layer='intent'".into());
                let mut value_matches = vec![format!(
                    "(s.vtype=1 AND toLower(s.vtext) CONTAINS {})",
                    parameter(&mut values, text(&term))
                )];
                if let Ok(integer) = term.parse::<i64>() {
                    value_matches.push(format!(
                        "(s.vtype=2 AND s.vint={})",
                        parameter(&mut values, Value::Int(integer))
                    ));
                }
                if term == "true" || term == "false" {
                    value_matches.push(format!(
                        "(s.vtype=3 AND s.vflag={})",
                        parameter(&mut values, Value::Bool(term == "true"))
                    ));
                }
                clauses.push(format!("({})", value_matches.join(" OR ")));
                let search_scopes: Vec<_> = searchable
                    .iter()
                    .map(|f| {
                        let allowed = visible_clause(context, Some(f), &mut values);
                        format!(
                            "(s.field={} AND {allowed})",
                            parameter(&mut values, text(f))
                        )
                    })
                    .collect();
                clauses.push(format!("({})", search_scopes.join(" OR ")));
            }
        }
        let base = format!(
            "MATCH {} WHERE {}",
            matches.join(", "),
            clauses.join(" AND ")
        );
        let count = tx.raw(
            &format!("{base} RETURN count(DISTINCT a) AS total"),
            values.clone(),
        )?;
        let total = count
            .first()
            .and_then(|r| r.get("total"))
            .and_then(Value::as_int)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(Error::Corrupt)?;
        let sort = match query.sort.as_str() {
            "" | "name" => format!(
                "CASE WHEN {} THEN a.name_sort ELSE a.asset END",
                visible_clause(context, Some("name"), &mut values)
            ),
            "kind" => "a.kind".into(),
            _ => return Err(Error::Invalid),
        };
        let direction = if query.descending { "DESC" } else { "ASC" };
        let limit = query.limit;
        values.insert("offset".into(), int(offset)?);
        values.insert("limit".into(), int(u64::from(limit))?);
        let rows = tx.query(&format!("{base} RETURN DISTINCT properties(a) AS p, {sort} AS ordering, a.key AS tie ORDER BY ordering {direction}, tie ASC SKIP $offset LIMIT $limit"), Some(values))?;
        let keys: Vec<_> = rows
            .iter()
            .map(|p| string(p, "key").map(text))
            .collect::<Result<_>>()?;
        let fields = tx.query("MATCH (f:OmRow) WHERE f.parent IN $keys AND f.layer='intent' AND f.field IN $fields RETURN properties(f) AS p LIMIT 4097", params([
            ("keys", Value::Array(keys)), ("fields", strings(&query.columns)),
        ]))?;
        if fields.len() > MAX_PAGE * MAX_FIELDS {
            return Err(Error::Limit);
        }
        let mut grouped: BTreeMap<String, Vec<wire::Field>> = BTreeMap::new();
        for f in &fields {
            let key = f
                .get("parent")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            grouped
                .entry(key.to_owned())
                .or_default()
                .push(super::super::inventory::read_field(f)?);
        }
        let network_keys = rows
            .iter()
            .map(|p| target(p).map(|t| text(format!("network/{}/{}", t.asset, t.incarnation))))
            .collect::<Result<Vec<_>>>()?;
        let networks = tx.query(
            "MATCH (n:OmRow) WHERE n.key IN $keys RETURN properties(n) AS p LIMIT 101",
            params([("keys", Value::Array(network_keys))]),
        )?;
        for row in &networks {
            let owner = target(row)?;
            let key = asset_key(owner);
            let values = network::projection(row, &query.columns)?;
            if let Some(root) = rows.iter().find(|p| p.get("key") == Some(&text(&key))) {
                let kind = number(root, "kind")? as i32;
                for value in values {
                    if context.field(owner, kind, &value.name, false) {
                        grouped.entry(key.clone()).or_default().push(value);
                    }
                }
            }
        }
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            let target = target(row)?;
            let kind = number(row, "kind")? as i32;
            let key = row
                .get("key")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            let fields = grouped
                .remove(key)
                .unwrap_or_default()
                .into_iter()
                .filter(|f| context.field(target, kind, &f.name, false))
                .collect();
            items.push(api::Item {
                target: Some(target_wire(target)),
                kind,
                revision: number(row, "revision")?,
                fields,
            });
        }
        tx.commit()?;
        if self.usage()?.revision != revision {
            return Err(Error::Expired);
        }
        let next_offset = offset.checked_add(u64::from(limit)).ok_or(Error::Limit)?;
        Ok(api::Page {
            items,
            total,
            revision,
            next: if next_offset < total {
                Some(format!(
                    "{revision}:{fingerprint}:{}:{next_offset}",
                    context.authority_revision
                ))
            } else {
                None
            },
        })
    }

    fn workspace_scope(
        &self,
        context: &Context,
        roots: &[wire::AssetRef],
    ) -> Result<BTreeSet<AssetRef>> {
        let mut selected = BTreeSet::new();
        let mut pending = Vec::new();
        for root in roots {
            pending.push(parse_target(Some(root))?);
        }
        while let Some(current) = pending.pop() {
            if selected.contains(&current) {
                continue;
            }
            if selected.len() >= 1024 {
                return Err(Error::Limit);
            }
            let asset = self.asset(current)?;
            if !context.readable(current, asset.kind) {
                return Err(Error::Denied);
            }
            selected.insert(current);
            for relation in self.relations(current, 128)? {
                let kind =
                    wire::RelationKind::try_from(relation.kind).map_err(|_| Error::Corrupt)?;
                let from = parse_target(relation.from.as_ref())?;
                let to = parse_target(relation.to.as_ref())?;
                let child = match kind {
                    wire::RelationKind::InstalledIn
                    | wire::RelationKind::RunsOn
                    | wire::RelationKind::On
                        if to == current =>
                    {
                        Some(from)
                    }
                    wire::RelationKind::Contains
                    | wire::RelationKind::HasPort
                    | wire::RelationKind::Carries
                        if from == current =>
                    {
                        Some(to)
                    }
                    _ => None,
                };
                if let Some(child) = child {
                    let asset = self.asset(child)?;
                    if context.readable(child, asset.kind) {
                        pending.push(child);
                    }
                }
            }
        }
        Ok(selected)
    }

    pub(super) fn workspace_read(&self, context: &Context, read: api::Read) -> Result<api::Detail> {
        let basis = self.usage()?.revision;
        let target = parse_target(read.target.as_ref())?;
        let asset = self.asset(target)?;
        if !context.readable(target, asset.kind) {
            return Err(Error::Denied);
        }
        let mut fields: Vec<_> = asset
            .intended
            .into_iter()
            .filter_map(|f| f.field)
            .filter(|f| context.field(target, asset.kind, &f.name, false))
            .collect();
        let mut relations = Vec::new();
        for relation in self.relations(target, 128)? {
            let from = parse_target(relation.from.as_ref())?;
            let to = parse_target(relation.to.as_ref())?;
            if context.readable(from, self.asset(from)?.kind)
                && context.readable(to, self.asset(to)?.kind)
            {
                relations.push(relation);
            }
        }
        let redact = |observations: Vec<wire::Observation>| {
            observations
                .into_iter()
                .filter_map(|mut o| {
                    let evidence = o.evidence.as_mut()?;
                    evidence
                        .fields
                        .retain(|f| context.field(target, asset.kind, &f.name, false));
                    if evidence.fields.is_empty() {
                        None
                    } else {
                        Some(o)
                    }
                })
                .collect()
        };
        let (participants, participants_complete) =
            self.workspace_network_fields(context, target, asset.kind, &mut fields)?;
        if self.usage()?.revision != basis {
            return Err(Error::Expired);
        }
        Ok(api::Detail {
            item: Some(api::Item {
                target: Some(target_wire(target)),
                kind: asset.kind,
                revision: asset.revision,
                fields,
            }),
            relations,
            observed: redact(asset.observed),
            imported: redact(asset.imported),
            participants,
            participants_complete,
        })
    }

    pub(super) fn workspace_write(
        &self,
        context: &Context,
        write: api::Write,
        now: u64,
    ) -> Result<api::Reply> {
        let mut tx = self.begin()?;
        let target = tx.workspace_write(context, write, now)?;
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
    pub(super) fn workspace_bulk(
        &self,
        context: &Context,
        bulk: api::Bulk,
        now: u64,
    ) -> Result<api::Reply> {
        if bulk.targets.is_empty()
            || bulk.targets.len() > MAX_BULK
            || (bulk.remove && !bulk.changes.is_empty())
        {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        if number(&tx.root, "revision")? != bulk.expected {
            return Err(Error::Conflict);
        }
        let mut seen = BTreeSet::new();
        for target in &bulk.targets {
            let target = parse_target(Some(target))?;
            if !seen.insert(target) {
                return Err(Error::Invalid);
            }
            let row = tx.visible_asset(context, target)?;
            if bulk.remove {
                tx.workspace_remove(context, target, &row, now)?;
            } else {
                tx.workspace_write(
                    context,
                    api::Write {
                        target: Some(target_wire(target)),
                        expected: Some(number(&row, "revision")?),
                        kind: number(&row, "kind")? as i32,
                        changes: bulk.changes.clone(),
                    },
                    now,
                )?;
            }
        }
        Ok(mutation(tx.commit()?, bulk.targets))
    }
    pub(super) fn workspace_relation(
        &self,
        context: &Context,
        request: api::Relationship,
        now: u64,
    ) -> Result<api::Reply> {
        let mut relation = request.relation.ok_or(Error::Invalid)?;
        if relation.kind == wire::RelationKind::AssignedTo as i32 {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        let authority = tx.workspace_authority()?;
        if relation.revision == 0 {
            if !relation.relation_id.is_empty()
                || !relation.authority_id.is_empty()
                || request.remove
            {
                return Err(Error::Invalid);
            }
            relation.relation_id = RelationId::generate().to_string();
        } else {
            let id: RelationId = relation.relation_id.parse().map_err(|_| Error::Invalid)?;
            let existing = self.relation(id)?;
            if existing.revision != relation.revision || existing.authority_id != authority {
                return Err(Error::Conflict);
            }
            for reference in [existing.from.as_ref(), existing.to.as_ref()] {
                let target = parse_target(reference)?;
                let row = tx.visible_asset(context, target)?;
                if !context.field(target, number(&row, "kind")? as i32, "relations", true) {
                    return Err(Error::Denied);
                }
            }
            if request.remove {
                let key = format!("relation/{id}");
                self.db.execute_in_transaction(
                    tx.id.ok_or(Error::Corrupt)?,
                    "MATCH (r:OmRow {key:$key})-[e]-() DELETE e",
                    params([("key", text(&key))]),
                )?;
                tx.delete(&key)?;
                tx.audit(
                    &key,
                    &context.actor,
                    "relation.removed",
                    relation.revision.checked_add(1).ok_or(Error::Limit)?,
                    now,
                )?;
                return Ok(mutation(tx.commit()?, Vec::new()));
            }
        }
        relation.authority_id = authority;
        tx.commit()?;
        let from = parse_target(relation.from.as_ref())?;
        let to = parse_target(relation.to.as_ref())?;
        for target in [from, to] {
            let asset = self.asset(target)?;
            if !context.field(target, asset.kind, "relations", true) {
                return Err(Error::Denied);
            }
        }
        let expected = if relation.revision == 0 {
            None
        } else {
            Some(relation.revision)
        };
        relation.revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Limit)?;
        let bytes = wire::Record {
            body: Some(record::Body::Relation(relation)),
        }
        .encode_to_vec();
        let admitted = omloom_contracts::decode(
            &bytes,
            omloom_contracts::Limits::default(),
            &omloom_contracts::MemoryBudget::new(4 * 1_048_576),
        )
        .map_err(|_| Error::Invalid)?;
        self.save_relation(&admitted, expected, &context.actor, now)?;
        Ok(mutation(
            self.usage()?.revision,
            vec![target_wire(from), target_wire(to)],
        ))
    }
}

impl Tx<'_> {
    pub(super) fn workspace_write(
        &mut self,
        context: &Context,
        write: api::Write,
        now: u64,
    ) -> Result<AssetRef> {
        let target = write
            .target
            .as_ref()
            .map(|t| parse_target(Some(t)))
            .transpose()?
            .unwrap_or_else(new_target);
        let kind = wire::AssetKind::try_from(write.kind).map_err(|_| Error::Invalid)?;
        if kind == wire::AssetKind::Unspecified {
            return Err(Error::Invalid);
        }
        let key = asset_key(target);
        let old = self.get(&key)?;
        if write.target.is_some() && old.is_none() {
            return Err(Error::Missing);
        }
        if self.get(&format!("removed/{}", target.asset))?.is_some() {
            return Err(Error::Conflict);
        }
        if old.as_ref().map(|p| number(p, "revision")).transpose()? != write.expected {
            return Err(Error::Conflict);
        }
        if let Some(row) = &old {
            if super::super::target(row)? != target || number(row, "kind")? as i32 != write.kind {
                return Err(Error::Conflict);
            }
        }
        self.require_changes(context, target, write.kind, &write.changes)?;
        let authority = self.workspace_authority()?;
        let mut changed = old.is_none();
        for field in &write.changes {
            let field_key = format!("{key}/intent/{}", field.name);
            let prior = self.get(&field_key)?;
            if prior
                .as_ref()
                .is_some_and(|p| super::super::inventory::field_matches(field, p))
            {
                continue;
            }
            let mut data = super::super::inventory::field_props(field)?;
            data.extend(props([
                ("layer", text("intent")),
                ("authority", text(&authority)),
            ]));
            changed |=
                self.put_against(&field_key, &key, Class::Inventory, data, prior.as_ref())?;
        }
        if !changed {
            return Ok(target);
        }
        let revision = write
            .expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let mut row = target_props(target);
        row.extend(props([
            ("kind", Value::Int(write.kind.into())),
            (
                "adoption",
                Value::Int(wire::Adoption::Adopted as i32 as i64),
            ),
            ("revision", int(revision)?),
        ]));
        let name = self.get(&format!("{key}/intent/name"))?;
        if let Some(name) = name.as_ref().filter(|p| number(p, "vtype").ok() == Some(1)) {
            let name = string(name, "vtext")?;
            row.insert("name".into(), text(&name));
            row.insert("name_sort".into(), text(name.to_lowercase()));
        }
        self.put_against(&key, "assets", Class::Inventory, row, old.as_ref())?;
        self.audit(&key, &context.actor, "inventory.accepted", revision, now)?;
        Ok(target)
    }
    fn workspace_authority(&mut self) -> Result<String> {
        match self.get("workspace/authority")? {
            Some(p) => string(&p, "authority"),
            None => {
                let id = AuthorityId::generate().to_string();
                self.put(
                    "workspace/authority",
                    "workspace",
                    Class::Inventory,
                    props([("authority", text(&id))]),
                )?;
                Ok(id)
            }
        }
    }
    fn workspace_remove(
        &mut self,
        context: &Context,
        target: AssetRef,
        row: &Properties,
        now: u64,
    ) -> Result<()> {
        if !context.field(target, number(row, "kind")? as i32, "remove", true) {
            return Err(Error::Denied);
        }
        let key = asset_key(target);
        // Existing operation/retirement code owns uncertain effects and identity fences.
        let pending = self.query("MATCH (n:OmRow) WHERE n.parent='operations' AND n.class=$class AND n.asset=$asset AND n.incarnation=$incarnation RETURN properties(n) AS p LIMIT 1",params([("class", int(Class::Pending as u64)?), ("asset", text(target.asset.to_string())), ("incarnation", text(target.incarnation.to_string()))]))?;
        if !pending.is_empty() {
            return Err(Error::UnsafeTransition);
        }
        let linked = self.query("MATCH (n:OmRow) WHERE n.parent='relations' AND (n.asset=$asset OR n.to_asset=$asset) RETURN properties(n) AS p LIMIT 1",params([("asset", text(target.asset.to_string()))]))?;
        if !linked.is_empty() {
            return Err(Error::Conflict);
        }
        let scope = format!("{}/{}", target.asset, target.incarnation);
        let references = self.query("MATCH (n:OmRow) WHERE n.parent='networks' AND (n.address_space=$scope OR n.link_scope=$scope OR n.routing_context=$scope) RETURN properties(n) AS p LIMIT 1", params([("scope", text(&scope))]))?;
        if !references.is_empty() {
            return Err(Error::Conflict);
        }
        let memberships = self.query("MATCH (n:OmRow) WHERE n.group IS NOT NULL AND n.asset=$asset AND n.incarnation=$incarnation RETURN properties(n) AS p LIMIT 1", params([("asset", text(target.asset.to_string())), ("incarnation", text(target.incarnation.to_string()))]))?;
        if !memberships.is_empty() {
            return Err(Error::Conflict);
        }
        let network_key = format!("network/{scope}");
        if let Some(network) = self.get(&network_key)? {
            let members = self.children(&format!("members/{scope}"))?;
            if !members.is_empty() {
                return Err(Error::Conflict);
            }
            self.delete(&string(&network, "namespace")?)?;
            if let Some(group) = network.get("group").and_then(Value::as_str) {
                self.delete(group)?;
            }
            self.delete(&network_key)?;
        }
        for child in self.children(&key)? {
            let child_key = string(&child, "key")?;
            if child.get("layer") == Some(&text("mapping")) {
                let mut checkpoint = child.clone();
                super::super::operations::strip_envelope(&mut checkpoint);
                self.put_against(
                    &child_key,
                    &key,
                    Class::Checkpoint,
                    checkpoint,
                    Some(&child),
                )?;
            } else {
                self.delete(&child_key)?;
            }
        }
        self.delete(&key)?;
        self.put(
            &format!("removed/{}", target.asset),
            "removed",
            Class::Checkpoint,
            target_props(target),
        )?;
        self.audit(
            &key,
            &context.actor,
            "inventory.removed",
            number(row, "revision")?,
            now,
        )?;
        Ok(())
    }
}

fn query_fingerprint(query: &api::Query, context: &Context) -> String {
    let mut canonical = query.clone();
    canonical.cursor = None;
    let mut digest = Sha256::new();
    digest.update(canonical.encode_to_vec());
    digest.update(context.actor.as_bytes());
    digest.update(format!("{:?}", context.grants).as_bytes());
    format!("{:x}", digest.finalize())
}
pub(super) fn validate_column(name: &str, definitions: &[api::Definition]) -> Result<()> {
    if [
        "name",
        "description",
        "serial",
        "cidr",
        "address_space",
        "link_scope",
        "routing_context",
        "sharing",
        "vrid",
        "vlan_id",
    ]
    .contains(&name)
        || definitions.iter().any(|d| d.id == name && !d.retired)
    {
        Ok(())
    } else {
        Err(Error::Incomplete)
    }
}
pub(super) fn validate_predicate(
    predicate: &api::Predicate,
    definitions: &[api::Definition],
) -> Result<()> {
    validate_column(&predicate.field, definitions)?;
    if let Some(definition) = definitions.iter().find(|d| d.id == predicate.field) {
        if predicate.definition_revision != Some(definition.revision) {
            return Err(Error::Incomplete);
        }
    }
    if predicate
        .value
        .as_ref()
        .is_none_or(|v| v.name != predicate.field)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
