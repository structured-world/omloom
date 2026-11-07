use super::*;

const MAX_EDGES: usize = 128;

impl Tx<'_> {
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

    pub(super) fn located_selection(
        &self,
        context: &Context,
        selection: &api::NetworkSelection,
    ) -> Result<BTreeSet<AssetRef>> {
        if !selection.unplaced {
            self.validate_sites(context, &selection.sites)?;
            return self.working_scope(context, &selection.sites);
        }
        if !selection.sites.is_empty() {
            return Err(Error::Invalid);
        }
        let mut values = HashMap::new();
        let permitted = assets::visible_clause_for(context, Some("relations"), &mut values, "a");
        let rows = self.raw(
            &format!("MATCH (a:OmRow {{parent:'assets'}}) WHERE a.kind={} AND a.adoption={} AND {permitted} RETURN a.asset AS asset, a.incarnation AS incarnation LIMIT {}", wire::AssetKind::Site as i32, wire::Adoption::Adopted as i32, MAX_BULK + 1),
            values,
        )?;
        if rows.len() > MAX_BULK {
            return Err(Error::Limit);
        }
        let sites = rows
            .iter()
            .map(|row| {
                Ok(wire::AssetRef {
                    asset_id: string(row, "asset")?,
                    incarnation: string(row, "incarnation")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.working_scope(context, &sites)
    }

    pub(super) fn network_topology(
        &self,
        context: &Context,
        items: &[api::Item],
        available_bytes: usize,
    ) -> Result<api::NetworkTopology> {
        use wire::RelationKind as R;
        let mut identities = Vec::with_capacity(items.len());
        let mut assets = Vec::with_capacity(items.len());
        for item in items {
            let target = parse_target(item.target.as_ref())?;
            if context.field(target, item.kind, "relations", false) {
                identities.push(text(format!("{}/{}", target.asset, target.incarnation)));
                assets.push(text(target.asset.to_string()));
            }
        }
        let mut topology = api::NetworkTopology::default();
        if identities.is_empty() {
            return Ok(topology);
        }
        let kinds = [
            R::AssignedTo,
            R::MemberOf,
            R::On,
            R::Contains,
            R::Carries,
            R::ConnectedTo,
            R::Uses,
        ]
        .into_iter()
        .map(|kind| Value::Int(kind as i64))
        .collect();
        // Both endpoints must be authorized nodes of this exact emitted page.
        let rows = self.query(
            "UNWIND $assets AS asset MATCH (r:OmRow {asset:asset}) WHERE r.parent='relations' AND r.kind IN $kinds AND (r.asset+'/'+r.incarnation) IN $ids AND (r.to_asset+'/'+r.to_incarnation) IN $ids RETURN properties(r) AS p ORDER BY r.key LIMIT $limit",
            params([
                ("assets", Value::Array(assets)),
                ("ids", Value::Array(identities)),
                ("kinds", Value::Array(kinds)),
                ("limit", Value::Int((MAX_EDGES + 1) as i64)),
            ]),
        )?;
        let mut remaining = available_bytes;
        for row in rows {
            let relation = super::super::inventory::relation_projection(&row)?;
            let edge = api::NetworkLink {
                relation_id: relation.relation_id,
                from: relation.from,
                to: relation.to,
                kind: relation.kind,
            };
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
