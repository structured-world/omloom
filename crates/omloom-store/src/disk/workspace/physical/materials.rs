//! Plan-local demand and sparse real allocations, never catalogue-derived assets.
use super::*;

const MAX_LINES: usize = 64;
const MAX_ALLOCATIONS: usize = 64;
const MAX_CLAIM_BYTES: usize = 65_536;
const MAX_ROWS: usize =
    MAX_LINES * (1 + omloom_contracts::equipment::FIELDS.len()) + MAX_ALLOCATIONS;

pub(super) struct Prepared {
    rows: Vec<PreparedRow>,
}
struct PreparedRow {
    path: String,
    parent: String,
    data: Properties,
}
fn label(value: &str, limit: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn material_kind(raw: i32) -> Result<wire::AssetKind> {
    let kind = wire::AssetKind::try_from(raw).map_err(|_| Error::Invalid)?;
    if !matches!(
        kind,
        wire::AssetKind::Device
            | wire::AssetKind::PhysicalComponent
            | wire::AssetKind::Rack
            | wire::AssetKind::Cable
    ) {
        return Err(Error::Invalid);
    }
    Ok(kind)
}
pub(super) fn normalize(plan: &mut api::PhysicalPlan) -> Result<()> {
    if plan.materials.len() > MAX_LINES {
        return Err(Error::Limit);
    }
    let mut allocations = 0;
    for material in &plan.materials {
        if material.allocations.len() > MAX_ALLOCATIONS - allocations {
            return Err(Error::Limit);
        }
        allocations += material.allocations.len();
    }
    for material in &mut plan.materials {
        if material.specifications.len() > omloom_contracts::equipment::FIELDS.len()
            || material.allocations.len() > MAX_ALLOCATIONS
        {
            return Err(Error::Limit);
        }
        for field in &mut material.specifications {
            if let Some(wire::field::Value::Equipment(value)) = &mut field.value {
                omloom_contracts::equipment::canonicalize(value).map_err(|_| Error::Invalid)?;
            }
        }
        material
            .specifications
            .sort_unstable_by(|a, b| a.name.cmp(&b.name));
        material.allocations.sort_unstable_by_key(|a| a.unit);
    }
    Ok(())
}
impl Tx<'_> {
    pub(super) fn material_access(
        &self,
        context: &Context,
        materials: &[api::PhysicalMaterial],
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<()> {
        if materials.len() > MAX_LINES {
            return Err(Error::Limit);
        }
        let mut count = 0;
        let mut references = BTreeMap::new();
        for material in materials {
            label(&material.name, 160)?;
            if material.quantity == 0 {
                return Err(Error::Invalid);
            }
            if material.allocations.len() > MAX_ALLOCATIONS - count {
                return Err(Error::Limit);
            }
            count += material.allocations.len();
            let kind = material_kind(material.kind)?;
            let mut units = BTreeSet::new();
            for allocation in &material.allocations {
                if allocation.unit >= material.quantity || !units.insert(allocation.unit) {
                    return Err(Error::Invalid);
                }
                label(&allocation.basis, 512)?;
                let target = parse_target(allocation.item.as_ref())?;
                if references.insert(target, kind).is_some() {
                    return Err(Error::Invalid);
                }
            }
        }
        let selection = self.selection(context, working, write)?;
        if !references.is_empty() {
            let keys = references.keys().map(|r| text(asset_key(*r))).collect();
            let metadata=self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption} AS p LIMIT 64",params([("keys",Value::Array(keys))]))?;
            let by_target = metadata
                .into_iter()
                .map(|row| Ok((target(&row)?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            for (target, expected) in references {
                selection.contains(target, self)?;
                if Self::physical_metadata_access(
                    context,
                    target,
                    by_target.get(&target).ok_or(Error::Conflict)?,
                    write,
                )? != expected
                {
                    return Err(Error::Invalid);
                }
            }
        }
        Ok(())
    }
    pub(super) fn receive_material(
        &mut self,
        context: &Context,
        receipt: api::PhysicalMaterialReceipt,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalPlan> {
        let key = receipt.plan.ok_or(Error::Invalid)?;
        if key.revision == 0 {
            return Err(Error::Invalid);
        }
        let mut plan = self.load_physical_plan(context, &key, working, true)?;
        if !matches!(
            api::PhysicalPlanState::try_from(plan.state),
            Ok(api::PhysicalPlanState::Draft
                | api::PhysicalPlanState::Reserved
                | api::PhysicalPlanState::Partial)
        ) {
            return Err(Error::UnsafeTransition);
        }
        let value = receipt.allocation.ok_or(Error::Invalid)?;
        let incoming = parse_target(value.item.as_ref())?;
        let allocation_count: usize = plan.materials.iter().map(|m| m.allocations.len()).sum();
        let material = plan
            .materials
            .get_mut(receipt.material as usize)
            .ok_or(Error::Invalid)?;
        let previous = if let Some(allocation) = material
            .allocations
            .iter_mut()
            .find(|a| a.unit == value.unit)
        {
            if *allocation == value {
                return Ok(plan);
            }
            Some(core::mem::replace(allocation, value))
        } else {
            if allocation_count >= MAX_ALLOCATIONS {
                return Err(Error::Limit);
            }
            material.allocations.push(value);
            None
        };
        if let Some(previous) = &previous
            && parse_target(previous.item.as_ref())? != incoming
        {
            let item = previous.item.as_ref();
            let old = parse_target(item)?;
            for step in &plan.steps {
                let used = match step.action.as_ref().ok_or(Error::Corrupt)? {
                    api::physical_step::Action::Placement(p) => p.item.as_ref() == item,
                    api::physical_step::Action::Replacement(r) => {
                        r.previous.as_ref() == item || r.replacement.as_ref() == item
                    }
                    api::physical_step::Action::Wiring(w) => wiring::targets(w)?.contains(&old),
                };
                if used {
                    return Err(Error::UnsafeTransition);
                }
            }
        }
        normalize(&mut plan)?;
        let prepared = self.prepare_materials(context, &plan, working, true)?;
        if plan.state != api::PhysicalPlanState::Draft as i32 {
            self.lock_physical_part(incoming, &key.id)?;
            if let Some(previous) = previous
                && parse_target(previous.item.as_ref())? != incoming
            {
                let old = parse_target(previous.item.as_ref())?;
                let lock = format!("physical-lock/{}", network::identity(old));
                let row = self.get(&lock)?.ok_or(Error::Corrupt)?;
                if string(&row, "owner")? != key.id {
                    return Err(Error::Conflict);
                }
                self.delete(&lock)?;
            }
        }
        self.save_materials(&format!("plan/{}", key.id), prepared, plan.materials.len())?;
        let state = api::PhysicalPlanState::try_from(plan.state).map_err(|_| Error::Corrupt)?;
        self.physical_state(context, &mut plan, state, now)?;
        Ok(plan)
    }
    pub(super) fn prepare_materials(
        &self,
        context: &Context,
        plan: &api::PhysicalPlan,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<Prepared> {
        if plan.materials.len() > MAX_LINES {
            return Err(Error::Limit);
        }
        if plan.materials.is_empty() {
            return Ok(Prepared { rows: Vec::new() });
        }
        let mut rows = plan.materials.len();
        let mut claims = 0;
        for material in &plan.materials {
            let kind = material_kind(material.kind)?;
            if material.specifications.len() > omloom_contracts::equipment::FIELDS.len() {
                return Err(Error::Limit);
            }
            rows += material.specifications.len() + material.allocations.len();
            let mut fields = BTreeSet::new();
            for field in &material.specifications {
                if !matches!(
                    kind,
                    wire::AssetKind::Device | wire::AssetKind::PhysicalComponent
                ) || !fields.insert(&field.name)
                {
                    return Err(Error::Invalid);
                }
                let Some(wire::field::Value::Equipment(value)) = &field.value else {
                    return Err(Error::Invalid);
                };
                omloom_contracts::equipment::validate_field(&field.name, value)
                    .map_err(|_| Error::Invalid)?;
                let size = field.encoded_len();
                if size > MAX_CLAIM_BYTES - claims {
                    return Err(Error::Limit);
                }
                claims += size;
            }
            if let Some(profile) = &material.physical {
                if profile.target.is_some()
                    || profile.occupancy_coverage.is_some()
                    || profile.power_sources_complete
                {
                    return Err(Error::Invalid);
                }
                definition_kind(kind, profile)?;
                let size = profile.encoded_len();
                if size > MAX_CLAIM_BYTES - claims {
                    return Err(Error::Limit);
                }
                claims += size;
            }
        }
        self.material_access(context, &plan.materials, working, write)?;
        let mut prepared = Prepared {
            rows: Vec::with_capacity(rows),
        };
        for (index, material) in plan.materials.iter().enumerate() {
            let path = index.to_string();
            let mut data = material
                .physical
                .as_deref()
                .map(definition_facts)
                .transpose()?
                .unwrap_or_default();
            data.extend(props([
                ("name", text(&material.name)),
                ("count", int(u64::from(material.quantity))?),
                (
                    "kind",
                    int(u64::try_from(material.kind).map_err(|_| Error::Invalid)?)?,
                ),
            ]));
            if !material.specifications.is_empty() {
                data.insert(
                    "expected".into(),
                    int(material.specifications.len() as u64)?,
                );
            }
            if !material.allocations.is_empty() {
                data.insert("generation".into(), int(material.allocations.len() as u64)?);
            }
            prepared.rows.push(PreparedRow {
                path: path.clone(),
                parent: String::new(),
                data,
            });
            for field in &material.specifications {
                prepared.rows.push(PreparedRow {
                    path: format!("{path}/field/{}", field.name),
                    parent: path.clone(),
                    data: inventory::field_props(field)?,
                });
            }
            for allocation in &material.allocations {
                let mut data = target_props(parse_target(allocation.item.as_ref())?);
                data.extend(props([
                    ("count", int(u64::from(allocation.unit))?),
                    ("basis", text(&allocation.basis)),
                ]));
                prepared.rows.push(PreparedRow {
                    path: format!("{path}/unit/{}", allocation.unit),
                    parent: path.clone(),
                    data,
                });
            }
        }
        Ok(prepared)
    }

    fn material_rows(&self, plan: &str) -> Result<(Vec<Properties>, Vec<Properties>)> {
        let parent = format!("{plan}/materials");
        let lines = self.query(
            "MATCH (n:OmRow {parent:$parent}) RETURN properties(n) AS p ORDER BY n.key LIMIT 65",
            params([("parent", text(&parent))]),
        )?;
        if lines.len() > MAX_LINES {
            return Err(Error::Corrupt);
        }
        let mut children = Vec::new();
        if !lines.is_empty() {
            let parents = lines
                .iter()
                .map(|row| string(row, "key").map(text))
                .collect::<Result<Vec<_>>>()?;
            children = self.query("UNWIND $parents AS parent MATCH (n:OmRow {parent:parent}) RETURN properties(n) AS p LIMIT 833",params([("parents",Value::Array(parents))]))?;
            if children.len() > MAX_ROWS - MAX_LINES {
                return Err(Error::Corrupt);
            }
        }
        Ok((lines, children))
    }
    pub(super) fn save_materials(
        &mut self,
        plan: &str,
        prepared: Prepared,
        previous_count: usize,
    ) -> Result<()> {
        if prepared.rows.is_empty() && previous_count == 0 {
            return Ok(());
        }
        let (lines, children) = if previous_count == 0 {
            (Vec::new(), Vec::new())
        } else {
            self.material_rows(plan)?
        };
        let mut previous = lines
            .into_iter()
            .chain(children)
            .map(|row| Ok((string(&row, "key")?, row)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for row in prepared.rows {
            let key = format!("{plan}/materials/{}", row.path);
            let parent = if row.parent.is_empty() {
                format!("{plan}/materials")
            } else {
                format!("{plan}/materials/{}", row.parent)
            };
            self.put_against(
                &key,
                &parent,
                Class::Inventory,
                row.data,
                previous.remove(&key).as_ref(),
            )?;
        }
        for key in previous.keys() {
            self.delete(key)?;
        }
        Ok(())
    }
    pub(super) fn load_materials(
        &self,
        plan: &str,
        count: usize,
        retired: bool,
    ) -> Result<Vec<api::PhysicalMaterial>> {
        if count > MAX_LINES {
            return Err(Error::Corrupt);
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let missing = || {
            if retired {
                Error::Expired
            } else {
                Error::Corrupt
            }
        };
        let (lines, children) = self.material_rows(plan)?;
        if lines.len() != count {
            return Err(missing());
        }
        let by_key = lines
            .iter()
            .map(|row| Ok((string(row, "key")?, row)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut children_by_parent = BTreeMap::<String, Vec<&Properties>>::new();
        for row in &children {
            children_by_parent
                .entry(string(row, "parent")?)
                .or_default()
                .push(row);
        }
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let key = format!("{plan}/materials/{index}");
            let row = by_key.get(&key).ok_or_else(missing)?;
            let mut material = api::PhysicalMaterial {
                name: string(row, "name")?,
                quantity: u32::try_from(number(row, "count")?).map_err(|_| Error::Corrupt)?,
                kind: i32::try_from(number(row, "kind")?).map_err(|_| Error::Corrupt)?,
                physical: if row.contains_key("basis") {
                    Some(Box::new(definition_facts_read(row)?))
                } else {
                    None
                },
                ..Default::default()
            };
            if let Some(children) = children_by_parent.get(&key) {
                for row in children {
                    if row.contains_key("field") {
                        let field = inventory::read_field(row)?;
                        if !matches!(field.value, Some(wire::field::Value::Equipment(_))) {
                            return Err(Error::Corrupt);
                        }
                        material.specifications.push(field);
                    } else {
                        material.allocations.push(api::PhysicalMaterialAllocation {
                            unit: u32::try_from(number(row, "count")?)
                                .map_err(|_| Error::Corrupt)?,
                            item: Some(target_wire(target(row)?)),
                            basis: string(row, "basis")?,
                        });
                    }
                }
            }
            material
                .specifications
                .sort_unstable_by(|a, b| a.name.cmp(&b.name));
            material.allocations.sort_unstable_by_key(|a| a.unit);
            let expected = if row.contains_key("expected") {
                number(row, "expected")?
            } else {
                0
            };
            let allocated = if row.contains_key("generation") {
                number(row, "generation")?
            } else {
                0
            };
            if material.specifications.len() as u64 != expected
                || material.allocations.len() as u64 != allocated
            {
                return Err(missing());
            }
            result.push(material);
        }
        Ok(result)
    }
    pub(super) fn retire_materials(&mut self, plan: &str, now: u64) -> Result<()> {
        let (lines, children) = self.material_rows(plan)?;
        for mut row in lines.into_iter().chain(children) {
            let key = string(&row, "key")?;
            let parent = string(&row, "parent")?;
            operations::strip_envelope(&mut row);
            row.insert("at".into(), int(now)?);
            self.put(&key, &parent, Class::Detail, row)?;
        }
        Ok(())
    }
}
