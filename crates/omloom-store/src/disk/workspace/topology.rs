use super::*;
use wire::RelationKind as R;

const MAX_EDGES: usize = 128;
const RELATIONS: &[R] = &[
    R::MemberOf,
    R::On,
    R::AssignedTo,
    R::Contains,
    R::Carries,
    R::ConnectedTo,
    R::Uses,
];

fn pair_key(from: AssetRef, to: AssetRef) -> Value {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&from.to_bytes());
    bytes.extend_from_slice(&to.to_bytes());
    Value::Binary(bytes)
}

pub(in crate::disk) fn derive_pair(data: &mut Properties) -> Result<()> {
    let kind = R::try_from(number(data, "kind")? as i32).map_err(|_| Error::Corrupt)?;
    if RELATIONS.contains(&kind) {
        let from = target(data)?;
        let to = AssetRef {
            asset: string(data, "to_asset")?
                .parse()
                .map_err(|_| Error::Corrupt)?,
            incarnation: string(data, "to_incarnation")?
                .parse()
                .map_err(|_| Error::Corrupt)?,
        };
        data.insert("topology_pair".into(), pair_key(from, to));
    } else {
        data.remove("topology_pair");
    }
    Ok(())
}

impl Tx<'_> {
    /// Project configured participants by exact membership keys, without expanding other interfaces.
    fn topology_assignments(
        &self,
        context: &Context,
        items: &[api::Item],
    ) -> Result<Vec<api::NetworkLink>> {
        use wire::AssetKind as K;
        let mut owners = Vec::new();
        let mut interfaces = Vec::new();
        let mut allocation_keys = Vec::new();
        for item in items {
            let target = parse_target(item.target.as_ref())?;
            if !context.field(target, item.kind, "relations", false) {
                continue;
            }
            match K::try_from(item.kind) {
                Ok(K::Interface) => interfaces.push(target),
                Ok(K::IpAddress | K::AddressGroup)
                    if context.field(target, item.kind, "network", false) =>
                {
                    if item.kind == K::IpAddress as i32 {
                        allocation_keys
                            .push(text(format!("network/{}", network::identity(target))));
                    } else {
                        owners.push((target, target));
                    }
                }
                _ => {}
            }
        }
        if interfaces.is_empty() || (owners.is_empty() && allocation_keys.is_empty()) {
            return Ok(Vec::new());
        }
        if !allocation_keys.is_empty() {
            let rows = self.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
                params([("keys", Value::Array(allocation_keys))]),
            )?;
            for row in rows {
                let address = target(&row)?;
                let owner = network::groups::group_of(&row)?.unwrap_or(address);
                if owner != address {
                    let kind = K::AddressGroup as i32;
                    if !context.field(owner, kind, "network", false)
                        || number(&self.asset(owner)?, "adoption")?
                            != wire::Adoption::Adopted as u64
                    {
                        continue;
                    }
                }
                owners.push((address, owner));
            }
        }
        let pairs: Vec<_> = owners
            .iter()
            .flat_map(|&(address, owner)| {
                let parent = format!("members/{}", network::identity(owner));
                interfaces.iter().map(move |&interface| {
                    (
                        address,
                        interface,
                        format!("{parent}/{}", network::identity(interface)),
                    )
                })
            })
            .collect();
        let keys: BTreeSet<_> = pairs.iter().map(|(_, _, key)| key.as_str()).collect();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self.raw(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN n.key AS key",
            [(
                "keys".into(),
                Value::Array(keys.into_iter().map(text).collect()),
            )]
            .into(),
        )?;
        let existing = rows
            .iter()
            .map(|row| row.get("key").and_then(Value::as_str).ok_or(Error::Corrupt))
            .collect::<Result<BTreeSet<_>>>()?;
        Ok(pairs
            .into_iter()
            .filter(|(_, _, key)| existing.contains(key.as_str()))
            .take(MAX_EDGES + 1)
            .map(|(from, to, _)| api::NetworkLink {
                relation_id: format!(
                    "assignment/{}/{}",
                    network::identity(from),
                    network::identity(to)
                ),
                from: Some(target_wire(from)),
                to: Some(target_wire(to)),
                kind: wire::RelationKind::AssignedTo as i32,
            })
            .collect())
    }

    pub(super) fn validate_sites(&self, context: &Context, sites: &[wire::AssetRef]) -> Result<()> {
        if sites.len() > 16 {
            return Err(Error::Limit);
        }
        for site in sites {
            let target = parse_target(Some(site))?;
            let row = self.visible_asset(context, target)?;
            if number(&row, "kind")? != wire::AssetKind::Site as u64 {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }

    /// Search upward only from candidates admitted by the other view filters.
    pub(super) fn unplaced_candidates(
        &self,
        context: &Context,
        bases: &[String],
        values: &HashMap<String, Value>,
    ) -> Result<BTreeSet<String>> {
        const CANDIDATES: usize = 4096;
        const ANCESTORS: usize = 8192;
        const BATCH: usize = 128;
        const BATCH_EDGES: usize = 2048;
        let site = wire::AssetKind::Site as i32;
        let mut candidates = BTreeSet::new();
        let mut located = BTreeSet::new();
        for base in bases {
            let rows = self.raw(
                &format!("{base} RETURN DISTINCT a.key AS key, a.asset AS asset, a.incarnation AS incarnation, a.kind AS kind LIMIT {}", CANDIDATES + 1),
                values.clone(),
            )?;
            for row in rows {
                let key = string(&row, "key")?;
                if number(&row, "kind")? == site as u64
                    && context.field(target(&row)?, site, "relations", false)
                {
                    located.insert(key.clone());
                }
                candidates.insert(key);
                if candidates.len() > CANDIDATES {
                    return Err(Error::Limit);
                }
            }
        }
        let mut visited = candidates.clone();
        let mut queue: Vec<_> = candidates.difference(&located).cloned().collect();
        let mut children = BTreeMap::<String, BTreeSet<String>>::new();
        let mut work = 0;
        while !queue.is_empty() {
            let start = queue.len() - queue.len().min(BATCH);
            let keys = queue.split_off(start);
            for reverse in [false, true] {
                let mut values = HashMap::new();
                let owned = assets::browse_clause(context, &mut values, Some(reverse), &[]);
                let path = assets::ownership_path(false, reverse);
                values.insert("keys".into(), Value::Array(keys.iter().map(text).collect()));
                let rows = self.raw(&format!(
                    "UNWIND $keys AS key MATCH {path} WHERE {owned} RETURN DISTINCT key AS child,p.key AS parent,p.kind AS kind LIMIT {}", BATCH_EDGES + 1
                ), values)?;
                if rows.len() > BATCH_EDGES {
                    return Err(Error::Limit);
                }
                work += rows.len();
                if work > ANCESTORS {
                    return Err(Error::Limit);
                }
                for row in rows {
                    let parent = string(&row, "parent")?;
                    children
                        .entry(parent.clone())
                        .or_default()
                        .insert(string(&row, "child")?);
                    if number(&row, "kind")? == site as u64 {
                        located.insert(parent);
                    } else if visited.insert(parent.clone()) {
                        queue.push(parent);
                    }
                }
            }
        }
        let mut queue: Vec<_> = located.iter().cloned().collect();
        while let Some(parent) = queue.pop() {
            if let Some(descendants) = children.remove(&parent) {
                for child in descendants {
                    if located.insert(child.clone()) {
                        queue.push(child);
                    }
                }
            }
        }
        candidates.retain(|key| !located.contains(key));
        Ok(candidates)
    }

    pub(super) fn network_topology(
        &self,
        context: &Context,
        items: &[api::Item],
        available_bytes: usize,
    ) -> Result<api::NetworkTopology> {
        let mut authorized = Vec::with_capacity(items.len());
        for item in items {
            let target = parse_target(item.target.as_ref())?;
            if context.field(target, item.kind, "relations", false) {
                authorized.push((
                    target,
                    wire::AssetKind::try_from(item.kind).map_err(|_| Error::Corrupt)?,
                ));
            }
        }
        let mut topology = api::NetworkTopology::default();
        // The initial schema includes the pair index, and relation writes supply its keys.
        // Exact pair probes keep off-page adjacency out of the read even for hubs.
        let mut pairs = Vec::new();
        for &(from, source_kind) in &authorized {
            for &(to, target_kind) in &authorized {
                if RELATIONS
                    .iter()
                    .any(|&kind| inventory::compatible(source_kind, target_kind, kind))
                {
                    pairs.push(pair_key(from, to));
                }
            }
        }
        let rows = if pairs.is_empty() {
            Vec::new()
        } else {
            self.query(
            "UNWIND $pairs AS pair MATCH (r:OmRow {topology_pair:pair}) RETURN properties(r) AS p ORDER BY r.key LIMIT $limit",
            params([
                ("pairs", Value::Array(pairs)),
                ("limit", Value::Int((MAX_EDGES + 1) as i64)),
            ]),
        )?
        };
        let mut edges = Vec::with_capacity(rows.len());
        for row in rows {
            let relation = super::super::inventory::relation_projection(&row)?;
            edges.push(api::NetworkLink {
                relation_id: relation.relation_id,
                from: relation.from,
                to: relation.to,
                kind: relation.kind,
            });
        }
        if edges.len() <= MAX_EDGES {
            edges.extend(self.topology_assignments(context, items)?);
        }
        let mut remaining = available_bytes;
        for edge in edges {
            let size = edge.encoded_len();
            let bytes = 1 + prost::length_delimiter_len(size) + size;
            if topology.edges.len() == MAX_EDGES || bytes > remaining {
                topology.more_edges = true;
                break;
            }
            remaining -= bytes;
            topology.edges.push(edge);
        }
        Ok(topology)
    }
}
