//! Current physical definitions use scalar indexed inventory rows, not wire blobs.
use super::*;
use crate::physical::{Span, Volume};
mod admission;
mod availability;
mod cables;
mod clearance;
mod completion;
mod connector;
mod coverage;
mod current;
mod equipment;
mod flow;
mod lifecycle;
mod materials;
mod positions;
mod rack_grid;
mod replacements;
mod reservations;
mod reviews;
mod wiring;

#[derive(Clone, Copy)]
struct Placement<'a> {
    item: &'a Option<wire::AssetRef>,
    material: Option<&'a api::PhysicalMaterialUnit>,
    enclosure: &'a Option<wire::AssetRef>,
    volume: &'a Option<api::PhysicalVolume>,
}
impl<'a> From<&'a api::PhysicalPlacement> for Placement<'a> {
    fn from(value: &'a api::PhysicalPlacement) -> Self {
        Self {
            item: &value.item,
            material: value.material.as_ref(),
            enclosure: &value.enclosure,
            volume: &value.volume,
        }
    }
}
impl<'a> Placement<'a> {
    fn requirement(
        &self,
        materials: &'a [api::PhysicalMaterial],
    ) -> Result<Option<&'a api::PhysicalMaterial>> {
        match (self.item.is_some(), self.material) {
            (true, None) => Ok(None),
            (false, Some(unit)) => {
                let material = materials
                    .get(unit.material as usize)
                    .ok_or(Error::Invalid)?;
                if unit.unit >= material.quantity {
                    return Err(Error::Invalid);
                }
                Ok(Some(material))
            }
            _ => Err(Error::Invalid),
        }
    }
    fn reference(
        &self,
        materials: &'a [api::PhysicalMaterial],
    ) -> Result<Option<&'a wire::AssetRef>> {
        let Some(material) = self.requirement(materials)? else {
            return Ok(self.item.as_ref());
        };
        let unit = self.material.ok_or(Error::Invalid)?.unit;
        Ok(material
            .allocations
            .iter()
            .find(|a| a.unit == unit)
            .and_then(|a| a.item.as_ref()))
    }
    fn actual(&self, materials: &'a [api::PhysicalMaterial]) -> Result<Option<AssetRef>> {
        self.reference(materials)?
            .map(|r| parse_target(Some(r)))
            .transpose()
    }
}
fn step_allocation(step: &api::PhysicalStep) -> Option<Placement<'_>> {
    match &step.action {
        Some(api::physical_step::Action::Placement(value)) => Some(Placement::from(value)),
        Some(api::physical_step::Action::Replacement(value))
            if value.volume.is_some() || value.replacement.is_some() =>
        {
            Some(Placement {
                item: &value.replacement,
                material: None,
                enclosure: &value.position,
                volume: &value.volume,
            })
        }
        _ => None,
    }
}
fn allocation_steps(plan: &api::PhysicalPlan) -> impl Iterator<Item = (usize, Placement<'_>)> {
    plan.steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| step_allocation(step).map(|p| (index, p)))
}
fn step_placement(step: &api::PhysicalStep) -> Option<Placement<'_>> {
    let allocation = step_allocation(step)?;
    if matches!(
        step.action,
        Some(api::physical_step::Action::Replacement(_))
    ) && allocation.volume.is_none()
    {
        return None;
    }
    Some(allocation)
}
fn shared_enclosure(kind: u64) -> bool {
    kind == wire::AssetKind::Site as u64 || kind == wire::AssetKind::Location as u64
}
fn placement_steps(plan: &api::PhysicalPlan) -> impl Iterator<Item = (usize, Placement<'_>)> {
    plan.steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| step_placement(step).map(|p| (index, p)))
}

fn placements(plan: &api::PhysicalPlan) -> impl Iterator<Item = Placement<'_>> {
    placement_steps(plan).map(|(_, p)| p)
}
fn replacement_steps(plan: &api::PhysicalPlan) -> impl Iterator<Item = &api::PhysicalReplacement> {
    plan.steps.iter().filter_map(|step| match &step.action {
        Some(api::physical_step::Action::Replacement(value)) => Some(value),
        _ => None,
    })
}
fn step_row(step: &api::PhysicalStep) -> Result<Properties> {
    let mut row = match step.action.as_ref().ok_or(Error::Invalid)? {
        api::physical_step::Action::Placement(p) => {
            let mut row = props([
                ("action", text("placement")),
                (
                    "enclosure",
                    text(network::identity(parse_target(p.enclosure.as_ref())?)),
                ),
            ]);
            match (&p.item, &p.material) {
                (Some(item), None) => {
                    row.insert(
                        "item".into(),
                        text(network::identity(parse_target(Some(item))?)),
                    );
                }
                (None, Some(unit)) => {
                    row.insert("vint".into(), int(u64::from(unit.material))?);
                    row.insert("generation".into(), int(u64::from(unit.unit))?);
                }
                _ => return Err(Error::Invalid),
            }
            write_volume(&mut row, p.volume.as_ref().ok_or(Error::Invalid)?)?;
            row
        }
        api::physical_step::Action::Replacement(r) => {
            let mut row = replacements::replacement_row(r)?;
            row.insert("action".into(), text("replacement"));
            row
        }
        api::physical_step::Action::Wiring(w) => wiring::step_row(w)?,
    };
    row.insert(
        "state".into(),
        int(u64::try_from(step.state).map_err(|_| Error::Invalid)?)?,
    );
    if let Some(control) = &step.control {
        // The bounded immutable control artifact is consumed whole by plan evaluation.
        row.insert("query".into(), Value::Binary(control.encode_to_vec()));
    }
    Ok(row)
}
fn read_step(row: &Properties) -> Result<api::PhysicalStep> {
    let state = i32::try_from(number(row, "state")?).map_err(|_| Error::Corrupt)?;
    if api::PhysicalStepState::try_from(state).map_err(|_| Error::Corrupt)?
        == api::PhysicalStepState::Unspecified
    {
        return Err(Error::Corrupt);
    }
    let action = match string(row, "action")?.as_str() {
        "placement" => api::physical_step::Action::Placement(api::PhysicalPlacement {
            item: row
                .get("item")
                .map(|_| {
                    string(row, "item")
                        .and_then(|s| lifecycle::reference(&s))
                        .map(target_wire)
                })
                .transpose()?,
            material: row
                .get("vint")
                .map(|_| -> Result<api::PhysicalMaterialUnit> {
                    Ok(api::PhysicalMaterialUnit {
                        material: u32::try_from(number(row, "vint")?)
                            .map_err(|_| Error::Corrupt)?,
                        unit: u32::try_from(number(row, "generation")?)
                            .map_err(|_| Error::Corrupt)?,
                    })
                })
                .transpose()?,
            enclosure: Some(target_wire(lifecycle::reference(&string(
                row,
                "enclosure",
            )?)?)),
            volume: read_volume(row)?,
        }),
        "replacement" => {
            api::physical_step::Action::Replacement(replacements::read_replacement(row)?)
        }
        "wiring" => api::physical_step::Action::Wiring(wiring::read_step(row)?),
        _ => return Err(Error::Corrupt),
    };
    Ok(api::PhysicalStep {
        state,
        action: Some(action),
        control: row
            .get("query")
            .map(|_| {
                let bytes = binary(row, "query")?;
                if bytes.len() > 65536 {
                    return Err(Error::Corrupt);
                }
                api::PlanControl::decode(bytes).map_err(|_| Error::Corrupt)
            })
            .transpose()?,
        ..Default::default()
    })
}

fn volume(value: &api::PhysicalVolume) -> Result<Volume> {
    fn axis(value: Option<&api::PhysicalExtent>) -> Result<Span> {
        let value = value.ok_or(Error::Invalid)?;
        Span::new(value.start_um, value.length_um).map_err(|_| Error::Invalid)
    }
    Ok(Volume {
        x: axis(value.x.as_ref())?,
        y: axis(value.y.as_ref())?,
        z: axis(value.z.as_ref())?,
    })
}
fn write_volume(data: &mut Properties, value: &api::PhysicalVolume) -> Result<()> {
    volume(value)?;
    for (name, axis) in [("x", &value.x), ("y", &value.y), ("z", &value.z)] {
        let axis = axis.as_ref().ok_or(Error::Invalid)?;
        data.insert(format!("{name}_start"), Value::Int(axis.start_um));
        data.insert(format!("{name}_length"), int(axis.length_um)?);
    }
    Ok(())
}
fn read_volume(data: &Properties) -> Result<Option<api::PhysicalVolume>> {
    if !data.contains_key("x_start") {
        return Ok(None);
    }
    fn axis(data: &Properties, name: &str) -> Result<api::PhysicalExtent> {
        Ok(api::PhysicalExtent {
            start_um: data
                .get(&format!("{name}_start"))
                .and_then(Value::as_int)
                .ok_or(Error::Corrupt)?,
            length_um: number(data, &format!("{name}_length"))?,
        })
    }
    let value = api::PhysicalVolume {
        x: Some(axis(data, "x")?),
        y: Some(axis(data, "y")?),
        z: Some(axis(data, "z")?),
    };
    volume(&value).map_err(|_| Error::Corrupt)?;
    Ok(Some(value))
}
fn definition_row(value: &api::PhysicalDefinition) -> Result<Properties> {
    let target = parse_target(value.target.as_ref())?;
    let mut data = definition_facts(value)?;
    data.extend(props([
        ("asset", text(target.asset.to_string())),
        ("incarnation", text(target.incarnation.to_string())),
    ]));
    Ok(data)
}
fn definition_kind(kind: wire::AssetKind, value: &api::PhysicalDefinition) -> Result<()> {
    if !value.channels.is_empty() && kind != wire::AssetKind::Port {
        return Err(Error::Invalid);
    }
    if value.assembly_kind != api::PhysicalAssemblyKind::Unspecified as i32
        && kind != wire::AssetKind::Cable
    {
        return Err(Error::Invalid);
    }
    if (value.rack_grid.is_some() && kind != wire::AssetKind::Rack)
        || (value.power_sources_complete
            && !matches!(
                kind,
                wire::AssetKind::Device
                    | wire::AssetKind::PhysicalComponent
                    | wire::AssetKind::Port
                    | wire::AssetKind::PowerFeed
            ))
        || (value.component_kind != api::PhysicalComponentKind::Unspecified as i32
            && !matches!(
                kind,
                wire::AssetKind::Device | wire::AssetKind::PhysicalComponent
            ))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn definition_facts(value: &api::PhysicalDefinition) -> Result<Properties> {
    api::PhysicalAssemblyKind::try_from(value.assembly_kind).map_err(|_| Error::Invalid)?;
    api::PhysicalComponentKind::try_from(value.component_kind).map_err(|_| Error::Invalid)?;
    if value.basis.trim().is_empty()
        || value.basis.len() > 512
        || value.basis.chars().any(char::is_control)
        || value
            .mounting
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
    {
        return Err(Error::Invalid);
    }
    let mut data = props([("basis", text(&value.basis))]);
    if !value.channels.is_empty() {
        if value.channels.len() > 64 {
            return Err(Error::Limit);
        }
        let mut channels = value.channels.clone();
        cables::normalize_channels(&mut channels)?;
        data.insert(
            "query".into(),
            Value::Binary(api::PhysicalChannelSet { channels }.encode_to_vec()),
        );
    }
    if value.component_kind != api::PhysicalComponentKind::Unspecified as i32 {
        data.insert("vint".into(), int(value.component_kind as u64)?);
    }
    if value.assembly_kind != api::PhysicalAssemblyKind::Unspecified as i32 {
        data.insert("vtype".into(), int(value.assembly_kind as u64)?);
    }
    if let Some(v) = &value.volume {
        write_volume(&mut data, v)?;
    }
    for (key, n) in [
        ("max_mass_g", value.max_mass_g),
        ("demand_mw", value.demand_mw),
        ("capacity_mw", value.capacity_mw),
        ("cooling_mw", value.cooling_mw),
        ("cooling_capacity_mw", value.cooling_capacity_mw),
    ] {
        if let Some(n) = n {
            data.insert(key.into(), int(n)?);
        }
    }
    if let Some(mounting) = &value.mounting {
        data.insert("mounting".into(), text(mounting));
    }
    connector::store(&mut data, value.mating.as_ref())?;
    clearance::write(&mut data, value.clearance.as_deref())?;
    coverage::write(&mut data, value.occupancy_coverage.as_deref())?;
    rack_grid::write(&mut data, value.rack_grid.as_deref(), value.volume.as_ref())?;
    if value.power_sources_complete {
        data.insert("approved".into(), Value::Bool(true));
    }
    Ok(data)
}
fn definition(data: &Properties) -> Result<api::PhysicalDefinition> {
    let mut value = definition_facts_read(data)?;
    value.target = Some(target_wire(target(data)?));
    Ok(value)
}
fn definition_facts_read(data: &Properties) -> Result<api::PhysicalDefinition> {
    let component_kind = if data.contains_key("vint") {
        i32::try_from(number(data, "vint")?).map_err(|_| Error::Corrupt)?
    } else {
        api::PhysicalComponentKind::Unspecified as i32
    };
    api::PhysicalComponentKind::try_from(component_kind).map_err(|_| Error::Corrupt)?;
    let optional = |key: &str| -> Result<Option<u64>> {
        if data.contains_key(key) {
            number(data, key).map(Some)
        } else {
            Ok(None)
        }
    };
    Ok(api::PhysicalDefinition {
        target: None,
        channels: data
            .get("query")
            .map(|_| -> Result<Vec<u32>> {
                let bytes = binary(data, "query")?;
                if bytes.len() > 256 {
                    return Err(Error::Corrupt);
                }
                let mut channels = api::PhysicalChannelSet::decode(bytes)
                    .map_err(|_| Error::Corrupt)?
                    .channels;
                cables::normalize_channels(&mut channels).map_err(|_| Error::Corrupt)?;
                Ok(channels)
            })
            .transpose()?
            .unwrap_or_default(),
        assembly_kind: if data.contains_key("vtype") {
            let kind = i32::try_from(number(data, "vtype")?).map_err(|_| Error::Corrupt)?;
            api::PhysicalAssemblyKind::try_from(kind).map_err(|_| Error::Corrupt)?;
            kind
        } else {
            api::PhysicalAssemblyKind::Unspecified as i32
        },
        volume: read_volume(data)?,
        basis: string(data, "basis")?,
        component_kind,
        max_mass_g: optional("max_mass_g")?,
        demand_mw: optional("demand_mw")?,
        capacity_mw: optional("capacity_mw")?,
        cooling_mw: optional("cooling_mw")?,
        cooling_capacity_mw: optional("cooling_capacity_mw")?,
        mounting: data
            .get("mounting")
            .map(|_| string(data, "mounting"))
            .transpose()?,
        mating: connector::read(data)?,
        clearance: clearance::read(data)?.map(Box::new),
        occupancy_coverage: coverage::read(data)?.map(Box::new),
        power_sources_complete: match data.get("approved") {
            None => false,
            Some(Value::Bool(value)) => *value,
            _ => return Err(Error::Corrupt),
        },
        rack_grid: rack_grid::read(data)?.map(Box::new),
    })
}
impl Tx<'_> {
    fn physical_access(
        &self,
        context: &Context,
        target: AssetRef,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<wire::AssetKind> {
        self.selection(context, working, write)?
            .contains(target, self)?;
        let row = self.get(&asset_key(target))?.ok_or(Error::Missing)?;
        Self::physical_metadata_access(context, target, &row, write)
    }
    fn physical_metadata_access(
        context: &Context,
        expected: AssetRef,
        row: &Properties,
        write: bool,
    ) -> Result<wire::AssetKind> {
        if target(row)? != expected {
            return Err(Error::Conflict);
        }
        if number(row, "adoption")? != wire::Adoption::Adopted as u64 {
            return Err(Error::Missing);
        }
        let kind = number(row, "kind")? as i32;
        if !context.readable(expected, kind)
            || !context.field(expected, kind, "physical", false)
            || (write && !context.field(expected, kind, "physical", true))
        {
            return Err(Error::Denied);
        }
        wire::AssetKind::try_from(kind).map_err(|_| Error::Corrupt)
    }
    fn physical_definition(&self, target: AssetRef) -> Result<Option<api::PhysicalDefinition>> {
        self.get(&format!("physical/{}", network::identity(target)))?
            .as_ref()
            .map(definition)
            .transpose()
    }
}
impl Store {
    pub(super) fn workspace_physical(
        &self,
        context: &Context,
        request: api::Physical,
        working: Option<&api::WorkingContext>,
        now: u64,
    ) -> Result<api::Reply> {
        use api::physical::Action;
        let working = || working.ok_or(Error::Invalid);
        let mut tx = self.begin()?;
        let mut view = api::PhysicalView {
            revision: number(&tx.root, "revision")?,
            state: api::RequirementState::Unknown as i32,
            reason: api::RequirementReason::Missing as i32,
            ..Default::default()
        };
        match request.action.ok_or(Error::Invalid)? {
            Action::ReviewReplacement(value) => {
                let (plan, hypothesis) = tx.review_replacement(context, value, working()?, now)?;
                view.plan = Some(plan);
                view.replacement_hypothesis = Some(hypothesis);
            }
            Action::ReplacementQuality(value) => {
                let (buckets, next) = tx.replacement_quality(context, value, working()?, now)?;
                view.replacement_quality = buckets;
                view.next = next;
            }
            Action::Offer(value) => {
                view.offers
                    .push(tx.save_physical_offer(context, value, working()?, now)?)
            }
            Action::Offers(value) => {
                let (offers, next) = tx.physical_offers(context, value, working()?)?;
                view.offers = offers;
                view.next = next;
            }
            Action::Availability(value) => {
                working()?;
                let (offer, result) = tx.offered_availability(context, value)?;
                view.offers.push(offer);
                (view.state, view.reason) = requirements::wire_decision(result);
            }
            Action::CableEnd(value) => {
                view.cable_ends
                    .push(tx.save_cable_end(context, value, working()?, now)?)
            }
            Action::CableEnds(value) => {
                let (ends, next) = tx.cable_ends(context, value, working()?)?;
                view.cable_ends = ends;
                view.next = next;
            }
            Action::Position(value) => {
                view.positions
                    .push(tx.save_physical_position(context, value, working()?, now)?);
            }
            Action::Positions(value) => {
                let (positions, next) = tx.physical_positions(context, value, working()?)?;
                view.positions = positions;
                view.next = next;
            }
            Action::List(request) => {
                let limit = if request.limit == 0 {
                    25
                } else {
                    request.limit
                };
                if limit > 25 {
                    return Err(Error::Limit);
                }
                if request.after.as_ref().is_some_and(|s| s.len() > 64) {
                    return Err(Error::Invalid);
                }
                let after = request.after.as_deref().unwrap_or("");
                let rows = tx.query("MATCH (n:OmRow {parent:'plans'}) WHERE n.actor=$actor AND n.action='physical' AND n.key>$after RETURN properties(n) AS p ORDER BY n.key LIMIT $limit",params([("actor",text(&context.actor)),("after",text(after)),("limit",int(u64::from(limit)+1)?)]))?;
                for row in rows.iter().take(limit as usize) {
                    let key = string(row, "key")?;
                    let id = key.strip_prefix("plan/").ok_or(Error::Corrupt)?;
                    PlanId::parse(id).map_err(|_| Error::Corrupt)?;
                    view.plans.push(api::PhysicalPlanSummary {
                        id: id.into(),
                        name: string(row, "name")?,
                        revision: number(row, "revision")?,
                        state: i32::try_from(number(row, "state")?).map_err(|_| Error::Corrupt)?,
                    });
                }
                if rows.len() > limit as usize {
                    view.next = Some(string(&rows[limit as usize - 1], "key")?);
                }
            }
            Action::Read(read) => {
                if read.after.is_some() && !read.occupants {
                    return Err(Error::Invalid);
                }
                let target = parse_target(read.enclosure.as_ref())?;
                tx.physical_access(context, target, working()?, false)?;
                if let Some(value) = tx.physical_definition(target)? {
                    view.definitions.push(value);
                }
                tx.physical_equipment_projection(context, target, &mut view)?;
                if read.occupants {
                    tx.read_physical_current(
                        context,
                        target,
                        working()?,
                        read.after.as_deref(),
                        &mut view,
                    )?;
                }
            }
            Action::Define(edit) => {
                let value = edit.definition.ok_or(Error::Invalid)?;
                let target = parse_target(value.target.as_ref())?;
                let kind = tx.physical_access(context, target, working()?, true)?;
                definition_kind(kind, &value)?;
                let data = definition_row(&value)?;
                if tx.put(
                    &format!("physical/{}", network::identity(target)),
                    "physical",
                    Class::Inventory,
                    data,
                )? {
                    tx.audit(
                        &network::identity(target),
                        &context.actor,
                        "physical-definition",
                        view.revision.checked_add(1).ok_or(Error::Limit)?,
                        now,
                    )?;
                }
                view.definitions.push(value);
            }
            Action::Preview(preview) => {
                let mut plan = preview.plan.ok_or(Error::Invalid)?;
                materials::normalize(&mut plan)?;
                tx.prepare_wiring_plan(context, &mut plan, working()?)?;
                tx.validate_physical_draft(context, &plan, working()?, false)?;
                if let Some(id) = &plan.id {
                    let existing = tx.load_physical_plan(
                        context,
                        &api::PhysicalPlanKey {
                            id: id.clone(),
                            revision: plan.revision,
                        },
                        working()?,
                        false,
                    )?;
                    reviews::check_saved_feedback(&plan, Some(&existing))?;
                } else {
                    reviews::check_saved_feedback(&plan, None)?;
                }
                let decision = match tx.assess_physical(context, &plan, working()?) {
                    Ok(_) => crate::requirements::Decision::PASS,
                    Err(failure) => match failure.decision {
                        Some(decision) => decision,
                        None => return Err(failure.error),
                    },
                };
                (view.state, view.reason) = requirements::wire_decision(decision);
                view.plan = Some(plan);
            }
            Action::Save(save) => {
                let mut plan = save.plan.ok_or(Error::Invalid)?;
                materials::normalize(&mut plan)?;
                tx.prepare_wiring_plan(context, &mut plan, working()?)?;
                let materials = tx.validate_physical_draft(context, &plan, working()?, true)?;
                tx.refresh_plan_control(context, &mut plan, working()?, now)?;
                for p in placements(&plan) {
                    if let Some(item) = p.actual(&plan.materials)? {
                        tx.physical_access(context, item, working()?, true)?;
                    }
                    tx.physical_access(
                        context,
                        parse_target(p.enclosure.as_ref())?,
                        working()?,
                        true,
                    )?;
                }
                let id = match &plan.id {
                    Some(id) => PlanId::parse(id).map_err(|_| Error::Invalid)?,
                    None => PlanId::generate(),
                };
                let key = format!("plan/{id}");
                let mut previous_count = 0;
                let mut previous_material_count = 0;
                for step in &mut plan.steps {
                    step.state = api::PhysicalStepState::Planned as i32;
                }
                if let Some(old) = tx.get(&key)? {
                    if number(&old, "revision")? != plan.revision {
                        return Err(Error::Conflict);
                    }
                    if string(&old, "actor")? != context.actor {
                        return Err(Error::Denied);
                    }
                    let existing = tx.load_physical_plan(
                        context,
                        &api::PhysicalPlanKey {
                            id: id.to_string(),
                            revision: plan.revision,
                        },
                        working()?,
                        true,
                    )?;
                    let mut existing = existing;
                    tx.refresh_plan_control(context, &mut existing, working()?, now)?;
                    if existing.state != api::PhysicalPlanState::Draft as i32 {
                        return Err(Error::Conflict);
                    }
                    previous_count = existing.steps.len();
                    previous_material_count = existing.materials.len();
                    reviews::check_saved_feedback(&plan, Some(&existing))?;
                    plan.state = api::PhysicalPlanState::Draft as i32;
                    if existing == plan {
                        view.plan = Some(existing);
                        tx.physical_labels(context, &mut view)?;
                        view.revision = tx.commit()?;
                        return Ok(reply(api::reply::Result::Physical(view)));
                    }
                    tx.reconcile_saved_feedback(&existing, &plan, now)?;
                } else if plan.id.is_some() {
                    return Err(Error::Missing);
                } else {
                    reviews::check_saved_feedback(&plan, None)?;
                }
                plan.id = Some(id.to_string());
                plan.state = api::PhysicalPlanState::Draft as i32;
                plan.reserved_until_ms = None;
                plan.revision = view.revision.checked_add(1).ok_or(Error::Limit)?;
                let mut header = props([
                    ("name", text(&plan.name)),
                    ("actor", text(&context.actor)),
                    ("action", text("physical")),
                    ("state", int(api::PhysicalPlanState::Draft as u64)?),
                    ("revision", int(plan.revision)?),
                    ("count", int(plan.steps.len() as u64)?),
                ]);
                if !plan.materials.is_empty() {
                    header.insert("material_count".into(), int(plan.materials.len() as u64)?);
                }
                tx.put(&key, "plans", Class::Inventory, header)?;
                tx.save_materials(&key, materials, previous_material_count)?;
                for (i, step) in plan.steps.iter().enumerate() {
                    tx.put(
                        &format!("{key}/{i}"),
                        &key,
                        Class::Inventory,
                        step_row(step)?,
                    )?;
                }
                for i in plan.steps.len()..previous_count {
                    tx.delete(&format!("{key}/{i}"))?;
                }
                tx.audit(
                    &id.to_string(),
                    &context.actor,
                    "save-plan",
                    plan.revision,
                    now,
                )?;
                view.plan = Some(plan);
            }
            Action::Load(key) => {
                view.plan = Some(tx.load_physical_plan(context, &key, working()?, false)?);
            }
            Action::Receive(receipt) => {
                view.plan = Some(tx.receive_material(context, receipt, working()?, now)?);
            }
            Action::Reserve(value) => {
                let key = value.plan.ok_or(Error::Invalid)?;
                view.plan =
                    Some(tx.reserve_physical(context, &key, working()?, now, value.until_ms)?);
            }
            Action::Complete(value) => {
                view.plan = Some(tx.complete_physical_steps(context, value, working()?, now)?);
            }
            Action::Cancel(key) => {
                view.plan = Some(tx.cancel_physical(context, &key, working()?, now)?);
            }
        }
        if let Some(plan) = &mut view.plan {
            tx.refresh_plan_control(context, plan, working()?, now)?;
        }
        tx.physical_labels(context, &mut view)?;
        view.revision = tx.commit()?;
        Ok(reply(api::reply::Result::Physical(view)))
    }
}
impl Tx<'_> {
    fn validate_physical_draft(
        &self,
        context: &Context,
        plan: &api::PhysicalPlan,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<materials::Prepared> {
        if plan.name.is_empty()
            || plan.name.len() > 160
            || plan.name.chars().any(char::is_control)
            || (plan.steps.is_empty() && plan.materials.is_empty())
            || plan.steps.len() > 64
        {
            return Err(Error::Invalid);
        }
        if plan.reserved_until_ms.is_some()
            || !matches!(
                api::PhysicalPlanState::try_from(plan.state),
                Ok(api::PhysicalPlanState::Unspecified | api::PhysicalPlanState::Draft)
            )
        {
            return Err(Error::Invalid);
        }
        let mut items = BTreeSet::new();
        let mut units = BTreeSet::new();
        if !plan.steps.is_empty() {
            flow::analysis(&plan.steps)?;
        }
        let materials = self.prepare_materials(context, plan, working, write)?;
        let mut controls = 0usize;
        for step in &plan.steps {
            if let Some(control) = &step.control {
                let size = control.encoded_len();
                if size > 65536 - controls {
                    return Err(Error::Limit);
                }
                controls += size;
                if let Some(check) = &control.check {
                    self.validate_requirement_graph(context, check.clone(), working)?;
                }
            }
            if !matches!(
                api::PhysicalStepState::try_from(step.state),
                Ok(api::PhysicalStepState::Unspecified | api::PhysicalStepState::Planned)
            ) || step.action.is_none()
            {
                return Err(Error::Invalid);
            }
        }
        for p in placements(plan) {
            let item = p.actual(&plan.materials)?;
            let enclosure = parse_target(p.enclosure.as_ref())?;
            if let Some(unit) = p.material
                && !units.insert((unit.material, unit.unit))
            {
                return Err(Error::Invalid);
            }
            let item_kind = if let Some(item) = item {
                if item == enclosure || !items.insert(item) {
                    return Err(Error::Invalid);
                }
                self.physical_access(context, item, working, false)?
            } else {
                wire::AssetKind::try_from(
                    p.requirement(&plan.materials)?.ok_or(Error::Invalid)?.kind,
                )
                .map_err(|_| Error::Invalid)?
            };
            let enclosure_kind = self.physical_access(context, enclosure, working, false)?;
            if !inventory::compatible(item_kind, enclosure_kind, wire::RelationKind::InstalledIn) {
                return Err(Error::Invalid);
            }
            volume(p.volume.as_ref().ok_or(Error::Invalid)?)?;
        }
        self.check_replacements(context, plan, working, write)?;
        Ok(materials)
    }
}
