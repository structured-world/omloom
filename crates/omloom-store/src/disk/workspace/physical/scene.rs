//! Reconstructible scene projection over current facts, without another inventory.
use super::*;

impl Tx<'_> {
    pub(super) fn save_port_anchor(
        &mut self,
        context: &Context,
        edit: api::PhysicalPortAnchorEdit,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        let id = RelationId::parse(&edit.binding_id).map_err(|_| Error::Invalid)?;
        let row = self.get(&format!("relation/{id}"))?.ok_or(Error::Missing)?;
        let binding = inventory::relation_projection(&row)?;
        let owner = parse_target(binding.from.as_ref())?;
        let port = parse_target(binding.to.as_ref())?;
        for subject in [owner, port] {
            let kind = self.physical_access(context, subject, working, true)?;
            if !context.field(subject, kind as i32, "relations", false) {
                return Err(Error::Denied);
            }
        }
        if binding.kind != wire::RelationKind::HasPort as i32 {
            return Err(Error::Invalid);
        }
        if binding.revision != edit.expected_binding_revision {
            return Err(Error::Conflict);
        }
        let body = edit.volume.ok_or(Error::Invalid)?;
        volume(&body)?;
        let before = self.get(&current::pose_key(&binding.relation_id))?;
        self.store_physical_pose(&binding, &body, &context.actor)?;
        if self.get(&current::pose_key(&binding.relation_id))? != before {
            self.audit(
                &binding.relation_id,
                &context.actor,
                "port-anchor",
                number(&self.root, "revision")?
                    .checked_add(1)
                    .ok_or(Error::Limit)?,
                now,
            )?;
        }
        Ok(())
    }
    pub(super) fn physical_scene(
        &self,
        context: &Context,
        request: api::PhysicalSceneQuery,
        working: &api::WorkingContext,
        view: &mut api::PhysicalView,
    ) -> Result<()> {
        if request.subjects.is_empty() || request.subjects.len() > 128 {
            return Err(Error::Limit);
        }
        let targets = request
            .subjects
            .iter()
            .map(|v| parse_target(Some(v)))
            .collect::<Result<BTreeSet<_>>>()?;
        if targets.len() != request.subjects.len() {
            return Err(Error::Invalid);
        }
        let selection = self.selection(context, working, false)?;
        let permitted = if let Some(focus) = selection.focus {
            let mut members: BTreeSet<_> =
                self.local_children(context, focus)?.into_iter().collect();
            members.insert(focus);
            members
        } else if !working.roots.is_empty() {
            self.owned_focuses(
                context,
                &working.roots,
                &targets.iter().copied().collect::<Vec<_>>(),
            )?
        } else {
            targets.clone()
        };
        if !targets.is_subset(&permitted) {
            return Err(Error::Denied);
        }
        let keys = targets.iter().map(|v| text(asset_key(*v))).collect();
        let rows = self.query(
            "UNWIND $keys AS key MATCH (a:OmRow {key:key}) RETURN properties(a) AS p LIMIT 128",
            params([("keys", Value::Array(keys))]),
        )?;
        if rows.len() != targets.len() {
            return Err(Error::Missing);
        }
        let mut kinds = BTreeMap::new();
        for row in &rows {
            let subject = target(row)?;
            if !targets.contains(&subject) {
                return Err(Error::Conflict);
            }
            let kind = Self::physical_metadata_access(context, subject, row, false)?;
            if !matches!(
                kind,
                wire::AssetKind::Site
                    | wire::AssetKind::Location
                    | wire::AssetKind::Rack
                    | wire::AssetKind::Device
                    | wire::AssetKind::Port
                    | wire::AssetKind::Cable
                    | wire::AssetKind::PhysicalComponent
                    | wire::AssetKind::PhysicalPosition
                    | wire::AssetKind::PowerFeed
            ) {
                return Err(Error::Invalid);
            }
            kinds.insert(subject, kind as i32);
            let mut fields = Vec::new();
            if context.field(subject, kind as i32, "name", false)
                && let Some(name) = row.get("name").and_then(Value::as_str)
            {
                fields.push(wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(name.into())),
                });
            }
            view.items.push(api::Item {
                target: Some(target_wire(subject)),
                kind: kind as i32,
                revision: number(row, "revision")?,
                fields,
            });
        }
        let frame = parse_target(request.subjects.first())?;
        if let Some(definition) = self.physical_definition(frame)? {
            view.definitions.push(definition);
        }
        let assets = targets.iter().map(|v| text(v.asset.to_string())).collect();
        let bindings=self.query("UNWIND $assets AS asset MATCH (r:OmRow {asset:asset}) WHERE r.parent='relations' AND r.to_asset IN $assets AND (r.kind=$kind OR r.kind=$port) RETURN properties(r) AS p LIMIT 257",params([("assets",Value::Array(assets)),("kind",int(wire::RelationKind::InstalledIn as u64)?),("port",int(wire::RelationKind::HasPort as u64)?)]))?;
        if bindings.len() > 256 {
            return Err(Error::Limit);
        }
        let mut accepted = Vec::new();
        for row in bindings {
            let binding = inventory::relation_projection(&row)?;
            let from = parse_target(binding.from.as_ref())?;
            let to = parse_target(binding.to.as_ref())?;
            if let (Some(a), Some(b)) = (kinds.get(&from), kinds.get(&to))
                && context.field(from, *a, "relations", false)
                && context.field(to, *b, "relations", false)
            {
                accepted.push(binding);
            }
        }
        let pose_keys = accepted
            .iter()
            .map(|r| text(current::pose_key(&r.relation_id)))
            .collect();
        let poses = self.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 256",
            params([("keys", Value::Array(pose_keys))]),
        )?;
        let poses = poses
            .into_iter()
            .map(|row| Ok((string(&row, "key")?, row)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for binding in accepted {
            let (item, enclosure) = if binding.kind == wire::RelationKind::HasPort as i32 {
                (binding.to.clone(), binding.from.clone())
            } else {
                (binding.from.clone(), binding.to.clone())
            };
            let pose = poses.get(&current::pose_key(&binding.relation_id));
            view.occupancy.push(api::PhysicalOccupancy {
                placement: Some(api::PhysicalPlacement {
                    item,
                    enclosure,
                    volume: pose.map(read_volume).transpose()?.flatten(),
                    material: None,
                }),
                revision: binding.revision,
                binding: Some(binding),
                ..Default::default()
            });
        }
        self.scene_connections(context, &kinds, view)?;
        Ok(())
    }
    fn scene_connections(
        &self,
        context: &Context,
        kinds: &BTreeMap<AssetRef, i32>,
        view: &mut api::PhysicalView,
    ) -> Result<()> {
        let ports: Vec<_> = kinds
            .iter()
            .filter(|(target, kind)| {
                **kind == wire::AssetKind::Port as i32
                    && context.field(**target, **kind, "relations", false)
            })
            .map(|(target, _)| text(asset_key(*target)))
            .collect();
        if ports.is_empty() {
            return Ok(());
        }
        let mut bindings = BTreeMap::new();
        let mut cables = BTreeSet::new();
        for edge in ["OmRelationFrom", "OmRelationTo"] {
            let rows=self.query(&format!("UNWIND $ports AS key MATCH (p:OmRow {{key:key}})<-[:{edge}]-(r:OmRow) WHERE r.parent='relations' AND r.kind=$kind RETURN properties(r) AS p LIMIT 257"),params([("ports",Value::Array(ports.clone())),("kind",int(wire::RelationKind::ConnectedTo as u64)?)]))?;
            if rows.len() > 256 {
                return Err(Error::Limit);
            }
            for row in rows {
                let relation = inventory::relation_projection(&row)?;
                let from = parse_target(relation.from.as_ref())?;
                let to = parse_target(relation.to.as_ref())?;
                if kinds.get(&from) != Some(&(wire::AssetKind::Port as i32))
                    && kinds.get(&to) != Some(&(wire::AssetKind::Port as i32))
                {
                    continue;
                }
                let cable = if kinds.get(&from) == Some(&(wire::AssetKind::Port as i32)) {
                    to
                } else {
                    from
                };
                cables.insert(cable);
                bindings.insert(relation.relation_id.clone(), relation);
            }
        }
        if cables.len() > 64 || bindings.len() > 256 {
            return Err(Error::Limit);
        }
        let rows = self.query(
            "UNWIND $keys AS key MATCH (a:OmRow {key:key}) RETURN properties(a) AS p LIMIT 64",
            params([(
                "keys",
                Value::Array(cables.iter().map(|v| text(asset_key(*v))).collect()),
            )]),
        )?;
        let mut allowed = BTreeMap::new();
        let mut parents = Vec::new();
        for row in rows {
            let cable = target(&row)?;
            let kind = wire::AssetKind::Cable as i32;
            if cables.contains(&cable)
                && number(&row, "kind")? == kind as u64
                && number(&row, "adoption")? == wire::Adoption::Adopted as u64
                && context.readable(cable, kind)
                && context.field(cable, kind, "physical", false)
                && context.field(cable, kind, "relations", false)
            {
                let parent = format!("cable-ends/{}", network::identity(cable));
                parents.push(text(parent.clone()));
                allowed.insert(parent, cable);
                if !view
                    .items
                    .iter()
                    .any(|v| v.target.as_ref() == Some(&target_wire(cable)))
                {
                    let fields = if context.field(cable, kind, "name", false) {
                        row.get("name")
                            .and_then(Value::as_str)
                            .map(|v| wire::Field {
                                name: "name".into(),
                                value: Some(field::Value::Text(v.into())),
                            })
                            .into_iter()
                            .collect()
                    } else {
                        Vec::new()
                    };
                    view.items.push(api::Item {
                        target: Some(target_wire(cable)),
                        kind,
                        revision: number(&row, "revision")?,
                        fields,
                    });
                }
            }
        }
        let rows=self.query("UNWIND $parents AS parent MATCH (e:OmRow {parent:parent}) RETURN properties(e) AS p LIMIT 257",params([("parents",Value::Array(parents))]))?;
        if rows.len() > 256 {
            return Err(Error::Limit);
        }
        let mut end_counts = BTreeMap::new();
        for row in &rows {
            *end_counts.entry(string(row, "parent")?).or_insert(0usize) += 1;
        }
        for row in rows {
            if row.get("state") != Some(&Value::Int(1)) {
                continue;
            }
            let Some(id) = row.get("binding").and_then(Value::as_str) else {
                continue;
            };
            let Some(binding) = bindings.get(id) else {
                continue;
            };
            if binding.revision != number(&row, "expected")? {
                continue;
            }
            let parent = string(&row, "parent")?;
            let Some(cable) = allowed.get(&parent) else {
                continue;
            };
            let port = cables::attached_port(binding, *cable)?;
            if kinds.get(&port) == Some(&(wire::AssetKind::Port as i32)) {
                let key = string(&row, "key")?;
                view.scene_connections.push(api::PhysicalSceneConnection {
                    cable: Some(target_wire(*cable)),
                    port: Some(target_wire(port)),
                    end_key: key.rsplit('/').next().ok_or(Error::Corrupt)?.into(),
                });
            }
        }
        for (parent, cable) in allowed {
            let cable = target_wire(cable);
            if end_counts.get(&parent) == Some(&2)
                && view
                    .scene_connections
                    .iter()
                    .filter(|end| end.cable.as_ref() == Some(&cable))
                    .count()
                    == 2
            {
                view.two_end_cables.push(cable);
            }
        }
        Ok(())
    }
}
