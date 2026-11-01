//! Selected accepted supply paths, read in the evaluator's existing transaction.
use super::*;
use crate::physical::power as graph;

pub(super) fn validate(
    condition: &api::PowerPathRequirement,
) -> Result<(Vec<AssetRef>, Vec<AssetRef>)> {
    if !(2..=graph::MAX_SOURCES).contains(&condition.sources.len())
        || condition.boundaries.len() > graph::MAX_SOURCES
    {
        return Err(Error::Invalid);
    }
    let parse = |values: &[wire::AssetRef]| {
        values
            .iter()
            .map(|v| parse_target(Some(v)))
            .collect::<Result<Vec<_>>>()
    };
    let sources = parse(&condition.sources)?;
    let boundaries = parse(&condition.boundaries)?;
    let unique: BTreeSet<_> = boundaries.iter().copied().collect();
    if unique.len() != boundaries.len() {
        return Err(Error::Invalid);
    }
    Ok((sources, boundaries))
}

/// One graph-work allowance for the entire selected requirement request.
pub(super) struct Budget {
    nodes: usize,
    edges: usize,
}
impl Budget {
    pub(super) fn new() -> Self {
        Self {
            nodes: graph::MAX_NODES,
            edges: graph::MAX_EDGES,
        }
    }
}

impl Tx<'_> {
    pub(super) fn requirement_power_paths(
        &self,
        context: &Context,
        condition: &api::PowerPathRequirement,
        budget: &mut Budget,
        response: &mut api::RequirementResult,
    ) -> Result<core::Decision> {
        let unknown = core::Decision::unknown;
        let (sources, boundaries) = validate(condition)?;
        let capacity = budget.nodes;
        let mut nodes = Vec::with_capacity(capacity);
        let mut indices = BTreeMap::new();
        let mut roles = Vec::with_capacity(sources.len());
        for source in sources {
            let index = match indices.get(&source) {
                Some(&index) => index,
                None => {
                    if nodes.len() == capacity {
                        return Ok(unknown(core::Reason::Incomplete));
                    }
                    let index = nodes.len();
                    nodes.push(source);
                    indices.insert(source, index);
                    index
                }
            };
            roles.push(index);
        }
        let mut complete = [false; graph::MAX_NODES];
        let mut edges = Vec::with_capacity(budget.edges);
        let mut start = 0;
        let mut cut_boundary = false;
        while start < nodes.len() {
            let end = nodes.len();
            let width = end - start;
            if width > budget.nodes {
                return Ok(unknown(core::Reason::Incomplete));
            }
            budget.nodes -= width;
            let keys = nodes[start..end]
                .iter()
                .map(|&n| text(asset_key(n)))
                .collect();
            let rows = self.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption} AS p LIMIT 128",
                params([("keys", Value::Array(keys))]),
            )?;
            if rows.len() != width {
                return Ok(unknown(core::Reason::Restricted));
            }
            let mut seen = [false; graph::MAX_NODES];
            for row in rows {
                let asset = target(&row)?;
                let Some(&index) = indices.get(&asset) else {
                    return Ok(unknown(core::Reason::Restricted));
                };
                if index < start || index >= end {
                    return Err(Error::Corrupt);
                }
                if seen[index] {
                    return Ok(unknown(core::Reason::Restricted));
                }
                seen[index] = true;
                let kind = i32::try_from(number(&row, "kind")?).map_err(|_| Error::Corrupt)?;
                if !context.readable(asset, kind)
                    || !context.field(asset, kind, "physical", false)
                    || !context.field(asset, kind, "relations", false)
                    || number(&row, "adoption")? != wire::Adoption::Adopted as u64
                {
                    return Ok(unknown(core::Reason::Restricted));
                }
                if !matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::Device
                        | wire::AssetKind::PhysicalComponent
                        | wire::AssetKind::Port
                        | wire::AssetKind::PowerFeed)
                ) {
                    return Ok(unknown(core::Reason::Unsupported));
                }
            }
            for present in &seen[start..end] {
                if !present {
                    return Ok(unknown(core::Reason::Restricted));
                }
            }
            // A terminal boundary may not conceal known upstream equipment.
            // Authorize its direct targets, then stop without traversing further.
            if cut_boundary {
                return Ok(unknown(core::Reason::ScopeMismatch));
            }
            let terminals: Vec<_> = boundaries
                .iter()
                .filter_map(|b| indices.get(b).copied())
                .collect();
            // All nodes through end, including newly discovered parents, are authorized above.
            // A visible counterexample needs no traversal of its private upstream network.
            let partial = graph::Graph {
                complete: &complete[..nodes.len()],
                edges: &edges,
                sources: &roles,
                boundaries: &terminals,
            }
            .evaluate()
            .map_err(|_| Error::Corrupt)?;
            if partial.decision.state == core::State::Failed {
                return Ok(power_assessment(partial, &nodes, response));
            }
            let keys = nodes[start..end]
                .iter()
                .map(|&n| text(format!("physical/{}", network::identity(n))))
                .collect();
            for row in self.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,approved:n.approved} AS p LIMIT 128",
                params([("keys", Value::Array(keys))]),
            )? {
                let asset = target(&row)?;
                let &index = indices.get(&asset).ok_or(Error::Corrupt)?;
                complete[index] = match row.get("approved") {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(v)) => *v,
                    _ => return Err(Error::Corrupt),
                };
            }
            let keys = nodes[start..end]
                .iter()
                .map(|&n| text(asset_key(n)))
                .collect();
            let rows = self.query("UNWIND $keys AS key MATCH (a:OmRow {key:key})-[:OmRelationFrom]->(r:OmRow) WHERE r.parent='relations' AND r.kind=$kind AND r.incarnation=a.incarnation RETURN {asset:r.asset,incarnation:r.incarnation,to_asset:r.to_asset,to_incarnation:r.to_incarnation} AS p LIMIT $limit", params([("keys",Value::Array(keys)),("kind",int(wire::RelationKind::PoweredBy as u64)?),("limit",int((budget.edges+1) as u64)?)]))?;
            if rows.len() > budget.edges {
                return Ok(unknown(core::Reason::Incomplete));
            }
            budget.edges -= rows.len();
            for row in rows {
                let read = |a: &str, i: &str| -> Result<AssetRef> {
                    Ok(AssetRef {
                        asset: string(&row, a)?.parse().map_err(|_| Error::Corrupt)?,
                        incarnation: string(&row, i)?.parse().map_err(|_| Error::Corrupt)?,
                    })
                };
                let from = target(&row)?;
                let to = read("to_asset", "to_incarnation")?;
                let &child = indices.get(&from).ok_or(Error::Corrupt)?;
                if boundaries.contains(&from) {
                    cut_boundary = true;
                }
                let parent = match indices.get(&to) {
                    Some(&index) => index,
                    None => {
                        if nodes.len() == capacity {
                            return Ok(unknown(core::Reason::Incomplete));
                        }
                        let index = nodes.len();
                        nodes.push(to);
                        indices.insert(to, index);
                        index
                    }
                };
                edges.push((child, parent));
            }
            start = end;
        }
        if cut_boundary {
            return Ok(unknown(core::Reason::ScopeMismatch));
        }
        let terminals: Vec<_> = boundaries
            .iter()
            .filter_map(|b| indices.get(b).copied())
            .collect();
        if terminals.is_empty() {
            return Ok(unknown(core::Reason::ScopeMismatch));
        }
        let assessment = graph::Graph {
            complete: &complete[..nodes.len()],
            edges: &edges,
            sources: &roles,
            boundaries: &terminals,
        }
        .evaluate()
        .map_err(|_| Error::Corrupt)?;
        Ok(power_assessment(assessment, &nodes, response))
    }
}
fn power_assessment(
    assessment: graph::Assessment,
    nodes: &[AssetRef],
    response: &mut api::RequirementResult,
) -> core::Decision {
    if let Some(index) = assessment.subject {
        let resource = nodes[index];
        response.target = Some(target_wire(resource));
        response.field = Some("physical".into());
        if assessment.decision.reason == core::Reason::Missing {
            response
                .resolutions
                .push(api::RequirementResolution::ReviewSource as i32);
        }
    }
    assessment.decision
}
