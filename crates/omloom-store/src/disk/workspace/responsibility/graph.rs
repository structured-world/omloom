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
                        "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
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
                    let rows = tx.query("UNWIND $keys AS key MATCH (n:OmRow {parent:key}) WHERE n.facet IS NOT NULL RETURN properties(n) AS p LIMIT 257", params([("keys", Value::Array(keys.clone()))]))?;
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
                    let keys = bindings.keys().map(text).collect();
                    let rows=tx.raw("UNWIND $keys AS key MATCH (r:OmRow {key:key})-[:OmRelationFrom|OmRelationTo]-(p:OmRow) WHERE p.parent='assets' RETURN properties(r) AS relation, properties(p) AS source LIMIT 513",params([("keys",Value::Array(keys))]).ok_or(Error::Corrupt)?)?;
                    if rows.len() > 512 || rows.len() > MAX_EDGES - edges {
                        return Err(Error::Limit);
                    }
                    edges += rows.len();
                    for mut row in rows {
                        let (Some(Value::Map(relation)), Some(Value::Map(source))) =
                            (row.remove("relation"), row.remove("source"))
                        else {
                            return Err(Error::Corrupt);
                        };
                        let parent = super::super::super::target(&source)?;
                        let relation = inventory::relation_projection(&relation)?;
                        let parent_kind =
                            wire::AssetKind::try_from(number(&source, "kind")? as i32)
                                .map_err(|_| Error::Corrupt)?;
                        for child in bindings
                            .get(&format!("relation/{}", relation.relation_id))
                            .ok_or(Error::Corrupt)?
                        {
                            let node = graph.nodes.get_mut(child).ok_or(Error::Corrupt)?;
                            let Some(child_row) = &node.row else {
                                continue;
                            };
                            let child_kind =
                                wire::AssetKind::try_from(number(child_row, "kind")? as i32)
                                    .map_err(|_| Error::Corrupt)?;
                            if qualified(*child, child_kind, parent, parent_kind, &relation, true)?
                            {
                                node.sources.insert(
                                    relation.relation_id.clone(),
                                    (relation.revision, parent),
                                );
                            }
                        }
                    }
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
        let facet = Facet::try_from(filter.facet).map_err(|_| Error::Invalid)?;
        field_name(facet)?;
        bounded_value(&filter.issuer, 256, true)?;
        bounded_value(&filter.subject, 256, true)?;
        let seeds = self.query(
            "MATCH (n:OmRow {party_lookup:$party}) RETURN properties(n) AS p LIMIT 4097",
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
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
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
                let keys = chunk.iter().map(|t| text(asset_key(*t))).collect();
                let rows = self.raw("UNWIND $keys AS key MATCH (p:OmRow {key:key})-[:OmRelationFrom|OmRelationTo]-(r:OmRow)-[:OmRelationFrom|OmRelationTo]-(a:OmRow) WHERE a.parent='assets' AND a.key<>p.key AND r.kind IN $kinds RETURN properties(p) AS source, properties(a) AS child, properties(r) AS relation LIMIT 8193", params([("keys", Value::Array(keys)), ("kinds", Value::Array([wire::RelationKind::Contains, wire::RelationKind::HasPort, wire::RelationKind::Carries, wire::RelationKind::RunsOn].into_iter().map(|k| Value::Int(k as i64)).collect()))]).ok_or(Error::Corrupt)?)?;
                if rows.len() > 8192 || rows.len() > MAX_EDGES - edges {
                    return Err(Error::Limit);
                }
                edges += rows.len();
                for mut row in rows {
                    let (
                        Some(Value::Map(source)),
                        Some(Value::Map(child)),
                        Some(Value::Map(relation)),
                    ) = (
                        row.remove("source"),
                        row.remove("child"),
                        row.remove("relation"),
                    )
                    else {
                        return Err(Error::Corrupt);
                    };
                    let parent = super::super::super::target(&source)?;
                    let target = super::super::super::target(&child)?;
                    let pk = wire::AssetKind::try_from(number(&source, "kind")? as i32)
                        .map_err(|_| Error::Corrupt)?;
                    let ck = wire::AssetKind::try_from(number(&child, "kind")? as i32)
                        .map_err(|_| Error::Corrupt)?;
                    if !qualified(
                        target,
                        ck,
                        parent,
                        pk,
                        &inventory::relation_projection(&relation)?,
                        true,
                    )? {
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
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
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
        let facet = Facet::try_from(filter.facet).map_err(|_| Error::Invalid)?;
        let field = field_name(facet)?;
        bounded_value(&filter.issuer, 256, true)?;
        bounded_value(&filter.subject, 256, true)?;
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
