//! InstalledIn owns current placement. Only independently reported geometry is stored beside it.
use super::*;

pub(super) fn pose_key(binding: &str) -> String {
    format!("physical-pose/{binding}")
}
pub(super) struct PhysicalUse {
    pub volumes: Vec<Volume>,
    pub items: BTreeSet<AssetRef>,
    pub inspected: usize,
}
fn map<'a>(row: &'a Properties, name: &str) -> Result<&'a Properties> {
    match row.get(name) {
        Some(Value::Map(value)) => Ok(value),
        _ => Err(Error::Corrupt),
    }
}
fn binding_id(row: &Properties) -> Result<String> {
    let key = string(row, "key")?;
    let id = key.strip_prefix("relation/").ok_or(Error::Corrupt)?;
    RelationId::parse(id).map_err(|_| Error::Corrupt)?;
    Ok(id.into())
}
impl Tx<'_> {
    pub(in crate::disk) fn require_no_physical_reservations(&self, target: AssetRef) -> Result<()> {
        let identity = network::identity(target);
        for key in [
            format!("physical-lock/{identity}"),
            format!("physical-slot/{identity}"),
        ] {
            if self.get(&key)?.is_some() {
                return Err(Error::Conflict);
            }
        }
        let reservations = self.query(
            "MATCH (n:OmRow {parent:$parent}) RETURN properties(n) AS p LIMIT 1",
            params([("parent", text(format!("physical-reservations/{identity}")))]),
        )?;
        if !reservations.is_empty() {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(in crate::disk) fn remove_physical_pose(&mut self, binding: &str) -> Result<()> {
        self.delete(&pose_key(binding))
    }
    pub(in crate::disk) fn clear_position_empty_report(&mut self, target: AssetRef) -> Result<()> {
        let key = format!("position/{}", network::identity(target));
        let Some(mut row) = self.get(&key)? else {
            return Ok(());
        };
        if !row.contains_key("vflag") {
            return Ok(());
        }
        operations::strip_envelope(&mut row);
        row.remove("vflag");
        row.insert(
            "revision".into(),
            int(number(&self.root, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?)?,
        );
        self.put(
            &key,
            &format!("positions/{}", string(&row, "enclosure")?),
            Class::Inventory,
            row,
        )?;
        Ok(())
    }
    pub(super) fn current_physical_rows(
        &self,
        enclosure: AssetRef,
        limit: u64,
    ) -> Result<Vec<Properties>> {
        self.query("MATCH (p:OmRow {key:$key})<-[:OmRelationTo]-(r:OmRow) WHERE r.parent='relations' AND r.kind=$kind AND r.to_asset=$asset AND r.to_incarnation=$incarnation RETURN properties(r) AS p LIMIT $limit",params([("key",text(asset_key(enclosure))),("kind",int(wire::RelationKind::InstalledIn as u64)?),("asset",text(enclosure.asset.to_string())),("incarnation",text(enclosure.incarnation.to_string())),("limit",int(limit)?)]))
    }
    pub(super) fn physical_use(
        &self,
        enclosure: AssetRef,
        plan: &api::PhysicalPlan,
        remaining: usize,
    ) -> Result<PhysicalUse> {
        debug_assert!(remaining <= 2048);
        let current = self.current_physical_rows(
            enclosure,
            u64::try_from(remaining + 1).map_err(|_| Error::Limit)?,
        )?;
        if current.len() > remaining {
            return Err(Error::Limit);
        }
        let pending = self.query(
            "MATCH (p:OmRow {parent:$parent}) RETURN properties(p) AS p LIMIT $limit",
            params([
                (
                    "parent",
                    text(format!(
                        "physical-reservations/{}",
                        network::identity(enclosure)
                    )),
                ),
                ("limit", int((remaining - current.len() + 1) as u64)?),
            ]),
        )?;
        let inspected = current.len() + pending.len();
        if inspected > remaining {
            return Err(Error::Limit);
        }
        let removed = replacement_steps(plan)
            .filter_map(|r| r.previous_binding.as_deref())
            .collect::<BTreeSet<_>>();
        let mut bindings = Vec::with_capacity(current.len());
        for row in current {
            let id = binding_id(&row)?;
            if !removed.contains(id.as_str()) {
                bindings.push((id, target(&row)?));
            }
        }
        let mut keys = bindings
            .iter()
            .map(|(id, _)| text(pose_key(id)))
            .collect::<Vec<_>>();
        for row in &pending {
            keys.push(text(string(row, "claim")?));
        }
        let mut poses = BTreeMap::new();
        if !keys.is_empty() {
            for row in self.query("UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p LIMIT 2048",params([("keys",Value::Array(keys))]))? {poses.insert(string(&row,"key")?,row);}
        }
        let mut output = PhysicalUse {
            volumes: Vec::with_capacity(bindings.len() + pending.len()),
            items: BTreeSet::new(),
            inspected,
        };
        for (id, item) in bindings {
            let pose = poses.get(&pose_key(&id)).ok_or(Error::Incomplete)?;
            output
                .volumes
                .push(volume(&read_volume(pose)?.ok_or(Error::Corrupt)?)?);
            output.items.insert(item);
        }
        for reservation in pending {
            let claim = string(&reservation, "claim")?;
            let row = poses.get(&claim).ok_or(Error::Corrupt)?;
            let step = read_step(row)?;
            if step.state != api::PhysicalStepState::Reserved as i32 {
                return Err(Error::Corrupt);
            }
            let Some(api::physical_step::Action::Placement(placement)) = step.action else {
                return Err(Error::Corrupt);
            };
            if parse_target(placement.enclosure.as_ref())? != enclosure {
                return Err(Error::Corrupt);
            }
            output
                .volumes
                .push(volume(placement.volume.as_ref().ok_or(Error::Corrupt)?)?);
            output.items.insert(parse_target(placement.item.as_ref())?);
        }
        Ok(output)
    }
    pub(super) fn store_physical_pose(
        &mut self,
        binding: &wire::Relation,
        value: &api::PhysicalVolume,
        actor: &str,
    ) -> Result<()> {
        let mut row = props([("actor", text(actor))]);
        write_volume(&mut row, value)?;
        let key = pose_key(&binding.relation_id);
        if let Some(prior) = self.get(&key)?
            && row.iter().all(|(key, value)| prior.get(key) == Some(value))
        {
            return Ok(());
        }
        row.insert(
            "revision".into(),
            int(number(&self.root, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?)?,
        );
        self.put(&key, "physical-poses", Class::Inventory, row)?;
        Ok(())
    }
    pub(super) fn read_physical_current(
        &self,
        context: &Context,
        enclosure: AssetRef,
        working: &api::WorkingContext,
        view: &mut api::PhysicalView,
    ) -> Result<()> {
        if let Some(focus) = working.focus.as_ref()
            && parse_target(Some(focus))? != enclosure
        {
            return Ok(());
        }
        let mut values = HashMap::from([
            ("key".into(), text(asset_key(enclosure))),
            ("asset".into(), text(enclosure.asset.to_string())),
            (
                "incarnation".into(),
                text(enclosure.incarnation.to_string()),
            ),
            ("kind".into(), int(wire::RelationKind::InstalledIn as u64)?),
            ("adopted".into(), int(wire::Adoption::Adopted as u64)?),
        ]);
        let visible = assets::visible_clause_for(context, Some("physical"), &mut values, "a");
        let rows=self.query(&format!("MATCH (p:OmRow {{key:$key}})<-[:OmRelationTo]-(r:OmRow) WHERE r.parent='relations' AND r.kind=$kind AND r.to_asset=$asset AND r.to_incarnation=$incarnation WITH r, 'asset/'+r.asset AS key MATCH (a:OmRow {{key:key}}) WHERE a.incarnation=r.incarnation AND a.adoption=$adopted AND {visible} RETURN {{binding:properties(r), item:properties(a)}} AS p LIMIT 129"),Some(values))?;
        if rows.len() > 128 {
            return Err(Error::Limit);
        }
        let mut poses = BTreeMap::new();
        let keys = rows
            .iter()
            .map(|row| {
                let relation = map(row, "binding")?;
                Ok(text(pose_key(&binding_id(relation)?)))
            })
            .collect::<Result<Vec<_>>>()?;
        if !keys.is_empty() {
            for row in self.query(
                "UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p LIMIT 128",
                params([("keys", Value::Array(keys))]),
            )? {
                poses.insert(string(&row, "key")?, row);
            }
        }
        let enclosure_kind =
            i32::try_from(number(&self.asset(enclosure)?, "kind")?).map_err(|_| Error::Corrupt)?;
        for row in rows {
            let relation = map(&row, "binding")?;
            let item = map(&row, "item")?;
            let target = target(item)?;
            let kind = i32::try_from(number(item, "kind")?).map_err(|_| Error::Corrupt)?;
            let id = binding_id(relation)?;
            let pose = poses.get(&pose_key(&id));
            let fields = if context.field(target, kind, "name", false) {
                item.get("name")
                    .and_then(Value::as_str)
                    .map(|value| wire::Field {
                        name: "name".into(),
                        value: Some(field::Value::Text(value.into())),
                    })
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            };
            view.items.push(api::Item {
                target: Some(target_wire(target)),
                kind,
                revision: number(item, "revision")?,
                fields,
            });
            let binding = if context.field(target, kind, "relations", false)
                && context.field(enclosure, enclosure_kind, "relations", false)
            {
                Some(inventory::relation_projection(relation)?)
            } else {
                None
            };
            view.occupancy.push(api::PhysicalOccupancy {
                placement: Some(api::PhysicalPlacement {
                    item: Some(target_wire(target)),
                    enclosure: Some(target_wire(enclosure)),
                    volume: pose.map(read_volume).transpose()?.flatten(),
                }),
                reported_by: relation
                    .get("actor")
                    .map(|_| string(relation, "actor"))
                    .transpose()?,
                revision: number(relation, "revision")?,
                binding,
                pose_revision: pose.map(|row| number(row, "revision")).transpose()?,
                pose_reported_by: pose.map(|row| string(row, "actor")).transpose()?,
            });
        }
        Ok(())
    }
}
