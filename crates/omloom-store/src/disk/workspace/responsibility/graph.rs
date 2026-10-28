use super::*;

const MAX_NODES: usize = 4096;
const MAX_EDGES: usize = 32768;
#[derive(Default)]
struct Node {
    row: Option<Properties>,
    assignments: BTreeMap<i32, api::ResponsibilityAssignment>,
    sources: BTreeMap<String, (u64, AssetRef)>,
    expanded: u8,
    loaded_mask: u8,
}

// This cache lives for one read transaction. Each wave batches exact indexed
// keys and accepted adjacent bindings; no effective assignment is persisted.
pub(super) struct Graph {
    nodes: BTreeMap<AssetRef, Node>,
}
impl Graph {
    pub fn load(tx: &Tx<'_>, masks: BTreeMap<AssetRef, u8>) -> Result<Self> {
        let mut graph = Self {
            nodes: BTreeMap::new(),
        };
        let mut pending = masks;
        let mut edges = 0;
        for _ in 0..MAX_ANCESTORS {
            if pending.is_empty() {
                return Ok(graph);
            }
            if pending
                .keys()
                .filter(|target| !graph.nodes.contains_key(target))
                .count()
                > MAX_NODES - graph.nodes.len()
            {
                return Err(Error::Limit);
            }
            let wave: Vec<_> = pending.into_iter().collect();
            pending = BTreeMap::new();
            for chunk in wave.chunks(64) {
                let keys: Vec<_> = chunk
                    .iter()
                    .filter(|(target, _)| !graph.nodes.contains_key(target))
                    .map(|(target, _)| text(asset_key(*target)))
                    .collect();
                let by_key: BTreeMap<_, _> = chunk
                    .iter()
                    .map(|(target, _)| (asset_key(*target), *target))
                    .collect();
                for (target, _) in chunk {
                    graph.nodes.entry(*target).or_default();
                }
                if !keys.is_empty() {
                    let rows = tx.query(
                    "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption,revision:n.revision} AS p",
                        params([("keys", Value::Array(keys.clone()))]),
                    )?;
                    for row in rows {
                        let target = super::super::super::target(&row)?;
                        if let Some(node) = graph.nodes.get_mut(&target)
                            && number(&row, "adoption")? == wire::Adoption::Adopted as u64
                        {
                            node.row = Some(row);
                        }
                    }
                    let rows = tx.query("UNWIND $keys AS key MATCH (n:OmRow {parent:key}) WHERE n.facet IS NOT NULL RETURN {parent:n.parent,facet:n.facet,mode:n.mode,issuer:n.issuer,subject:n.subject,party_kind:n.party_kind,name:n.name,contact:n.contact,source_relation:n.source_relation,source_binding_revision:n.source_binding_revision} AS p LIMIT 257", params([("keys", Value::Array(keys))]))?;
                    if rows.len() > chunk.len() * FACETS.len() {
                        return Err(Error::Corrupt);
                    }
                    for row in rows {
                        let target = by_key.get(&string(&row, "parent")?).ok_or(Error::Corrupt)?;
                        let assignment = read_assignment(&row)?;
                        let node = graph.nodes.get_mut(target).ok_or(Error::Corrupt)?;
                        if node
                            .assignments
                            .insert(assignment.facet, assignment)
                            .is_some()
                        {
                            return Err(Error::Corrupt);
                        }
                    }
                }
                let mut bindings = BTreeMap::<String, BTreeSet<AssetRef>>::new();
                for (target, mask) in chunk {
                    let node = graph.nodes.get_mut(target).ok_or(Error::Corrupt)?;
                    for facet in [Facet::ClaimedOwner, Facet::Operator] {
                        let bit = 1 << (facet as i32 - 1);
                        if *mask & bit == 0 || node.loaded_mask & bit != 0 {
                            continue;
                        }
                        if let Some(assignment) = node.assignments.get(&(facet as i32))
                            && assignment.mode == Mode::Inherit as i32
                            && let Some(id) = &assignment.source_relation
                        {
                            bindings
                                .entry(format!("relation/{id}"))
                                .or_default()
                                .insert(*target);
                        }
                        node.loaded_mask |= bit;
                    }
                }
                if !bindings.is_empty() {
                    graph.load_sources(tx, &bindings, &mut edges)?;
                }
                for (target, mask) in chunk {
                    let node = graph.nodes.get(target).ok_or(Error::Corrupt)?;
                    let active = *mask & !node.expanded;
                    for facet in [Facet::ClaimedOwner, Facet::Operator] {
                        let bit = 1 << (facet as i32 - 1);
                        if active & bit == 0 {
                            continue;
                        }
                        let assignment = node.assignments.get(&(facet as i32));
                        if assignment.is_some_and(|a| a.mode != Mode::Inherit as i32) {
                            continue;
                        }
                        let (parent, status) = Self::parent(node, assignment);
                        if status == Status::Inherited
                            && let Some(parent) = parent
                            && graph
                                .nodes
                                .get(&parent)
                                .is_none_or(|node| node.expanded & bit == 0)
                        {
                            *pending.entry(parent).or_insert(0) |= bit;
                        }
                    }
                    let node = graph.nodes.get_mut(target).ok_or(Error::Corrupt)?;
                    node.expanded |= active;
                }
            }
            pending.retain(|target, mask| {
                graph
                    .nodes
                    .get(target)
                    .is_none_or(|node| node.expanded & *mask != *mask)
            });
        }
        if !pending.is_empty() {
            return Err(Error::Limit);
        }
        Ok(graph)
    }
    fn load_sources(
        &mut self,
        tx: &Tx<'_>,
        bindings: &BTreeMap<String, BTreeSet<AssetRef>>,
        edges: &mut usize,
    ) -> Result<()> {
        let keys = bindings.keys().map(text).collect();
        let rows = tx.query(
            "UNWIND $keys AS key MATCH (r:OmRow {key:key}) RETURN properties(r) AS p LIMIT 257",
            params([("keys", Value::Array(keys))]),
        )?;
        if rows.len() > 256 || rows.len() > MAX_EDGES - *edges {
            return Err(Error::Limit);
        }
        *edges += rows.len();
        let mut parents = BTreeSet::new();
        let mut relations = Vec::with_capacity(rows.len());
        for row in rows {
            let relation = inventory::relation_projection(&row)?;
            let from = parse_target(relation.from.as_ref())?;
            let to = parse_target(relation.to.as_ref())?;
            let children = bindings
                .get(&format!("relation/{}", relation.relation_id))
                .ok_or(Error::Corrupt)?;
            for child in children {
                if *child == from {
                    parents.insert(to);
                } else if *child == to {
                    parents.insert(from);
                }
            }
            relations.push((relation, from, to, children));
        }
        let keys = parents
            .iter()
            .map(|target| text(asset_key(*target)))
            .collect();
        let mut kinds = BTreeMap::new();
        for row in tx.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind} AS p", params([("keys", Value::Array(keys))]))? {
            kinds.insert(super::super::super::target(&row)?, wire::AssetKind::try_from(number(&row, "kind")? as i32).map_err(|_| Error::Corrupt)?);
        }
        for (relation, from, to, children) in relations {
            for child in children {
                let parent = if *child == from {
                    to
                } else if *child == to {
                    from
                } else {
                    continue;
                };
                let Some(parent_kind) = kinds.get(&parent) else {
                    continue;
                };
                let node = self.nodes.get_mut(child).ok_or(Error::Corrupt)?;
                let Some(row) = &node.row else {
                    continue;
                };
                let child_kind = wire::AssetKind::try_from(number(row, "kind")? as i32)
                    .map_err(|_| Error::Corrupt)?;
                if qualified(*child, child_kind, parent, *parent_kind, &relation, true)? {
                    node.sources
                        .insert(relation.relation_id.clone(), (relation.revision, parent));
                }
            }
        }
        Ok(())
    }
    fn parent(
        node: &Node,
        assignment: Option<&api::ResponsibilityAssignment>,
    ) -> (Option<AssetRef>, Status) {
        if let Some(assignment) = assignment
            && let Some(id) = &assignment.source_relation
        {
            return match node.sources.get(id) {
                Some((revision, parent)) if Some(*revision) == assignment.source_revision => {
                    (Some(*parent), Status::Inherited)
                }
                _ => (None, Status::SourceUnavailable),
            };
        }
        (None, Status::Unknown)
    }
    pub fn assignment(
        &self,
        target: AssetRef,
        facet: Facet,
    ) -> Option<&api::ResponsibilityAssignment> {
        self.nodes.get(&target)?.assignments.get(&(facet as i32))
    }
    pub fn asset(&self, target: AssetRef) -> Result<&Properties> {
        self.nodes
            .get(&target)
            .and_then(|node| node.row.as_ref())
            .ok_or(Error::Missing)
    }
    pub fn source_disclosable(
        &self,
        context: &Context,
        target: AssetRef,
        id: &str,
    ) -> Result<bool> {
        let Some((_, parent)) = self
            .nodes
            .get(&target)
            .and_then(|node| node.sources.get(id))
        else {
            return Ok(false);
        };
        let Some(row) = self.nodes.get(parent).and_then(|node| node.row.as_ref()) else {
            return Ok(false);
        };
        let kind = number(row, "kind")? as i32;
        Ok(context.field(*parent, kind, "name", false)
            && context.field(*parent, kind, "relations", false))
    }
    pub fn resolve(
        &self,
        context: &Context,
        target: AssetRef,
        facet: Facet,
    ) -> Result<api::EffectiveResponsibility> {
        let row = self
            .nodes
            .get(&target)
            .and_then(|n| n.row.as_ref())
            .ok_or(Error::Missing)?;
        let field = field_name(facet)?;
        let contact_visible = context.field(
            target,
            number(row, "kind")? as i32,
            &format!("{field}.contact"),
            false,
        );
        let source_visible = context.field(target, number(row, "kind")? as i32, "relations", false);
        let mut result = api::EffectiveResponsibility {
            facet: facet as i32,
            status: Status::Unknown as i32,
            ..Default::default()
        };
        let mut current = target;
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current) {
                return Err(Error::Invalid);
            }
            if seen.len() > MAX_ANCESTORS {
                return Err(Error::Limit);
            }
            let Some(node) = self.nodes.get(&current) else {
                return Err(Error::Corrupt);
            };
            let Some(row) = &node.row else {
                result.status = Status::SourceUnavailable as i32;
                return Ok(result);
            };
            let assignment = node.assignments.get(&(facet as i32));
            if let Some(assignment) = assignment {
                match Mode::try_from(assignment.mode).map_err(|_| Error::Corrupt)? {
                    Mode::Explicit | Mode::Unassigned => {
                        result.status = if assignment.mode == Mode::Unassigned as i32 {
                            Status::Unassigned
                        } else if current == target {
                            Status::Explicit
                        } else {
                            Status::Inherited
                        } as i32;
                        result.party = assignment.party.clone();
                        if contact_visible {
                            result.contact = assignment.contact.clone();
                        }
                        if (current == target || source_visible)
                            && context.field(current, number(row, "kind")? as i32, "name", false)
                            && (current == target
                                || context.field(
                                    current,
                                    number(row, "kind")? as i32,
                                    "relations",
                                    false,
                                ))
                        {
                            result.source = Some(target_wire(current));
                            result.source_revision = Some(number(row, "revision")?);
                        }
                        return Ok(result);
                    }
                    Mode::Inherit => {}
                    Mode::Unspecified => return Err(Error::Corrupt),
                }
            }
            if !inheritable(facet) {
                return Ok(result);
            }
            let (parent, status) = Self::parent(node, assignment);
            if let Some(parent) = parent {
                current = parent;
            } else {
                result.status = status as i32;
                return Ok(result);
            }
        }
    }
}

impl Tx<'_> {
    pub(in super::super) fn responsibility_filter(
        &self,
        context: &Context,
        filter: &api::ResponsibilityFilter,
    ) -> Result<Vec<String>> {
        let facet = validate_filter(filter)?;
        let seeds = self.query(
            "MATCH (n:OmRow {party_lookup:$party}) RETURN {parent:n.parent} AS p LIMIT 4097",
            params([(
                "party",
                text(party_lookup(facet, &filter.issuer, &filter.subject)),
            )]),
        )?;
        if seeds.len() > MAX_NODES {
            return Err(Error::Limit);
        }
        let keys: Vec<_> = seeds
            .iter()
            .map(|r| r.get("parent").cloned().ok_or(Error::Corrupt))
            .collect::<Result<_>>()?;
        let rows = self.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation} AS p",
            params([("keys", Value::Array(keys))]),
        )?;
        let mut candidates = BTreeSet::new();
        for row in rows {
            candidates.insert(super::super::super::target(&row)?);
        }
        let mut pending: Vec<_> = candidates.iter().copied().collect();
        let mut edges = 0;
        while !pending.is_empty() && inheritable(facet) {
            let mut next = Vec::new();
            for chunk in pending.chunks(64) {
                let ids =
                    || Value::Array(chunk.iter().map(|t| text(t.asset.to_string())).collect());
                let mut rows = self.query("UNWIND $ids AS id MATCH (r:OmRow {asset:id}) WHERE r.parent='relations' AND r.kind IN $kinds RETURN properties(r) AS p LIMIT 8193", params([("ids", ids()), ("kinds", Value::Array([wire::RelationKind::Contains, wire::RelationKind::HasPort, wire::RelationKind::Carries].into_iter().map(|k| Value::Int(k as i64)).collect()))]))?;
                rows.extend(self.query("UNWIND $ids AS id MATCH (r:OmRow {to_asset:id}) WHERE r.parent='relations' AND r.kind=$kind RETURN properties(r) AS p LIMIT 8193", params([("ids", ids()), ("kind", Value::Int(wire::RelationKind::RunsOn as i64))]))?);
                if rows.len() > 8192 || rows.len() > MAX_EDGES - edges {
                    return Err(Error::Limit);
                }
                edges += rows.len();
                let mut relations = Vec::new();
                for row in rows {
                    let relation = inventory::relation_projection(&row)?;
                    let from = parse_target(relation.from.as_ref())?;
                    let to = parse_target(relation.to.as_ref())?;
                    let (parent, child) = if relation.kind == wire::RelationKind::RunsOn as i32 {
                        (to, from)
                    } else {
                        (from, to)
                    };
                    if !chunk.contains(&parent) {
                        continue;
                    }
                    relations.push((parent, child, relation));
                }
                let children: BTreeMap<_, _> = relations
                    .iter()
                    .map(|(_, child, _)| (assignment_key(*child, facet), *child))
                    .collect();
                if children.len() > MAX_NODES {
                    return Err(Error::Limit);
                }
                let keys = children.keys().map(text).collect();
                let mut selected = BTreeMap::new();
                for row in self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) WHERE n.mode=$mode RETURN {key:n.key,source_relation:n.source_relation} AS p LIMIT 4097", params([
                    ("keys", Value::Array(keys)), ("mode", Value::Int(Mode::Inherit as i64))
                ]))? {
                    let child = *children.get(&string(&row, "key")?).ok_or(Error::Corrupt)?;
                    if let Some(id) = row.get("source_relation").and_then(Value::as_str)
                        && selected.insert(child, id.to_owned()).is_some() {
                        return Err(Error::Corrupt);
                    }
                }
                // Placement is not ownership. A selected but stale binding still
                // reaches resolution, which reports incompleteness rather than guessing.
                relations.retain(|(_, child, relation)| {
                    selected.get(child) == Some(&relation.relation_id)
                });
                if relations.is_empty() {
                    continue;
                }
                let endpoints: BTreeSet<_> = relations
                    .iter()
                    .flat_map(|(parent, child, _)| [*parent, *child])
                    .collect();
                let keys = endpoints
                    .iter()
                    .map(|target| text(asset_key(*target)))
                    .collect();
                let mut kinds = BTreeMap::new();
                for row in self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind} AS p", params([("keys", Value::Array(keys))]))? {
                    kinds.insert(super::super::super::target(&row)?, wire::AssetKind::try_from(number(&row,"kind")? as i32).map_err(|_| Error::Corrupt)?);
                }
                for (parent, target, relation) in relations {
                    let (Some(pk), Some(ck)) = (kinds.get(&parent), kinds.get(&target)) else {
                        continue;
                    };
                    if !qualified(target, *ck, parent, *pk, &relation, true)? {
                        continue;
                    }
                    if !candidates.contains(&target) {
                        if candidates.len() == MAX_NODES {
                            return Err(Error::Limit);
                        }
                        candidates.insert(target);
                        next.push(target);
                    }
                }
            }
            pending = next;
        }
        let targets: Vec<_> = candidates.into_iter().collect();
        let keys = targets
            .iter()
            .map(|target| text(asset_key(*target)))
            .collect();
        let rows = self.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption} AS p",
            params([("keys", Value::Array(keys))]),
        )?;
        self.responsibility_filter_rows(context, filter, rows)
    }
    pub(in super::super) fn responsibility_filter_rows(
        &self,
        context: &Context,
        filter: &api::ResponsibilityFilter,
        rows: Vec<Properties>,
    ) -> Result<Vec<String>> {
        if rows.len() > MAX_NODES {
            return Err(Error::Limit);
        }
        let facet = validate_filter(filter)?;
        let field = field_name(facet)?;
        let mut targets = Vec::new();
        for row in rows {
            let target = super::super::super::target(&row)?;
            if number(&row, "adoption")? == wire::Adoption::Adopted as u64
                && context.field(target, number(&row, "kind")? as i32, field, false)
            {
                targets.push(target);
            }
        }
        let graph = Graph::load(
            self,
            targets
                .iter()
                .map(|target| {
                    (
                        *target,
                        if facet == Facet::ClaimedOwner {
                            1
                        } else if facet == Facet::Operator {
                            2
                        } else {
                            0
                        },
                    )
                })
                .collect(),
        )?;
        let mut matching = Vec::new();
        for target in targets {
            let Some(row) = graph.nodes.get(&target).and_then(|n| n.row.as_ref()) else {
                continue;
            };
            if !context.field(target, number(row, "kind")? as i32, field, false) {
                continue;
            }
            let resolved = graph.resolve(context, target, facet)?;
            if matches!(
                Status::try_from(resolved.status),
                Ok(Status::Ambiguous | Status::SourceUnavailable)
            ) {
                return Err(Error::Incomplete);
            }
            if let Some(party) = resolved.party
                && party.issuer == filter.issuer
                && party.subject == filter.subject
            {
                matching.push(asset_key(target));
            }
        }
        Ok(matching)
    }
}
