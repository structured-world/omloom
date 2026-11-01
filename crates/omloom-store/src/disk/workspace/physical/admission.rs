//! Read-only admission shared by transient previews and atomic reservation.
use super::*;
use crate::requirements::{Decision, Reason};

pub(super) struct Failure {
    pub error: Error,
    pub decision: Option<Decision>,
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        let reason = match error {
            Error::Incomplete => Some(Reason::Missing),
            Error::Denied => Some(Reason::Restricted),
            Error::Conflict => Some(Reason::Conflict),
            Error::Limit => Some(Reason::Incomplete),
            _ => None,
        };
        Self {
            error,
            decision: reason.map(Decision::unknown),
        }
    }
}
impl Failure {
    pub(super) fn mismatch() -> Self {
        Self {
            error: Error::Conflict,
            decision: Some(Decision::FAIL),
        }
    }
    pub(super) fn basis(error: Error) -> Self {
        if matches!(error, Error::Conflict) {
            Self {
                error,
                decision: Some(Decision::unknown(Reason::Stale)),
            }
        } else {
            error.into()
        }
    }
}

fn require_mounting(
    required: Option<&str>,
    actual: Option<&str>,
) -> core::result::Result<(), Failure> {
    match (required, actual) {
        (Some(required), Some(actual)) if required == actual => Ok(()),
        (Some(_), Some(_)) => Err(Failure::mismatch()),
        _ => Err(Error::Incomplete.into()),
    }
}

fn component_load_limits(
    incoming: AssetRef,
    target: &api::PhysicalDefinition,
    item: &api::PhysicalDefinition,
) -> core::result::Result<Option<(AssetRef, u64)>, Failure> {
    for (limit, demand) in [
        (target.capacity_mw, item.demand_mw),
        (target.cooling_capacity_mw, item.cooling_mw),
    ] {
        if let Some(limit) = limit {
            let demand = demand.ok_or(Error::Incomplete)?;
            if demand > limit {
                return Err(Failure::mismatch());
            }
        }
    }
    Ok(target.max_mass_g.map(|limit| (incoming, limit)))
}

fn require_coverage(
    definition: &api::PhysicalDefinition,
    region: crate::physical::Region,
) -> core::result::Result<(), Failure> {
    let decision = coverage::assess(definition.occupancy_coverage.as_deref(), region)?;
    if decision.state != crate::requirements::State::Satisfied {
        return Err(Failure {
            error: Error::Incomplete,
            decision: Some(decision),
        });
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct Claims {
    pub(super) parts: BTreeSet<AssetRef>,
    positions: BTreeSet<AssetRef>,
    pub(super) ends: BTreeSet<String>,
}

impl Tx<'_> {
    // The claims were checked in this same transaction; namespace CAS protects final admission.
    pub(super) fn acquire_physical_claims(&mut self, claims: Claims, owner: &str) -> Result<()> {
        for key in claims.ends {
            let parent = key.rsplit_once('/').ok_or(Error::Corrupt)?.0;
            self.put(
                &key,
                parent,
                Class::Pending,
                props([("owner", text(owner))]),
            )?;
        }
        for (targets, prefix, parent) in [
            (claims.parts, "physical-lock", "physical-locks"),
            (claims.positions, "physical-slot", "physical-slots"),
        ] {
            for target in targets {
                self.put(
                    &format!("{prefix}/{}", network::identity(target)),
                    parent,
                    Class::Pending,
                    props([("owner", text(owner))]),
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn assess_physical(
        &self,
        context: &Context,
        plan: &api::PhysicalPlan,
        working: &api::WorkingContext,
    ) -> core::result::Result<Claims, Failure> {
        if plan.steps.is_empty() {
            return Err(Error::Incomplete.into());
        }
        let mut claims = self.check_replacement_capacity(context, plan, working)?;
        for allocation in plan.materials.iter().flat_map(|m| &m.allocations) {
            claims.parts.insert(parse_target(allocation.item.as_ref())?);
        }
        let graph = flow::analysis(&plan.steps)?;
        self.assess_wiring(context, plan, working, &graph, &mut claims)?;
        let profiles = self.placement_profiles(plan)?;
        let mut enclosures = BTreeMap::<AssetRef, current::PhysicalUse>::new();
        let mut rows_read = 0;
        for (index, p) in placement_steps(plan) {
            let item = p.actual(&plan.materials)?;
            let material = p.requirement(&plan.materials)?;
            let declared = if item.is_none() {
                material.and_then(|m| m.physical.as_deref())
            } else {
                None
            };
            let enclosure = parse_target(p.enclosure.as_ref())?;
            let row = self.visible_asset(context, enclosure)?;
            if !context.field(
                enclosure,
                number(&row, "kind")? as i32,
                "physical_availability",
                false,
            ) {
                return Err(Error::Denied.into());
            }
            if let Some(item) = item {
                claims.parts.insert(item);
                let reference = p.reference(&plan.materials)?;
                if !self.installed_bindings(item)?.is_empty()
                    && !replacement_steps(plan).any(|r| r.previous.as_ref() == reference)
                {
                    return Err(Failure::mismatch());
                }
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = enclosures.entry(enclosure) {
                let occupied =
                    self.physical_use(enclosure, plan, &graph.ancestors, 2048 - rows_read)?;
                rows_read += occupied.inspected;
                entry.insert(occupied);
            }
            let occupied = enclosures.get_mut(&enclosure).ok_or(Error::Corrupt)?;
            let definition = &occupied.definition;
            let mounting = |target: &AssetRef| {
                profiles
                    .get(target)
                    .and_then(|row| row.get("mounting"))
                    .and_then(Value::as_str)
            };
            let item_mounting = match item {
                Some(item) => mounting(&item),
                None => declared.and_then(|p| p.mounting.as_deref()),
            };
            if mounting(&enclosure).is_some() || item_mounting.is_some() {
                require_mounting(mounting(&enclosure), item_mounting)?;
            }
            let item_volume = match item {
                Some(item) => self.equipment_envelope(context, item)?,
                None => equipment::declared_envelope(material.ok_or(Error::Invalid)?)?,
            }
            .ok_or(Error::Incomplete)?;
            let requested = p.volume.as_ref().ok_or(Error::Invalid)?;
            for (actual, proposed) in [
                (&item_volume.x, &requested.x),
                (&item_volume.y, &requested.y),
                (&item_volume.z, &requested.z),
            ] {
                if actual.as_ref().ok_or(Error::Incomplete)?.length_um
                    > proposed.as_ref().ok_or(Error::Invalid)?.length_um
                {
                    return Err(Failure::mismatch());
                }
            }
            let envelope = definition.volume.as_ref().map(volume).transpose()?;
            let proposed = volume(requested)?;
            let margins = item
                .and_then(|item| profiles.get(&item))
                .map(clearance::read)
                .transpose()?
                .flatten();
            let margin = match item {
                Some(_) => margins.as_ref(),
                None => declared.and_then(|p| p.clearance.as_deref()),
            };
            let region = clearance::region(proposed, margin)?;
            match crate::physical::placement(envelope, Some(region), &occupied.volumes).state {
                crate::requirements::State::Satisfied => {}
                crate::requirements::State::Failed => return Err(Failure::mismatch()),
                crate::requirements::State::Unknown => return Err(Error::Incomplete.into()),
            }
            for (removal, prior) in &occupied.conditional_removals {
                if index == *removal || graph.ancestors[index] & (1u64 << removal) != 0 {
                    continue;
                }
                let prior = prior.ok_or(Error::Incomplete)?;
                if crate::physical::placement(envelope, Some(region), core::slice::from_ref(&prior))
                    .state
                    == crate::requirements::State::Failed
                {
                    return Err(Error::Conflict.into());
                }
            }
            require_coverage(definition, region)?;
            occupied.volumes.push(region);
            if let Some(item) = item {
                occupied.items.insert(item);
            } else {
                let needed = [
                    definition.max_mass_g.is_some(),
                    definition.capacity_mw.is_some(),
                    definition.cooling_capacity_mw.is_some(),
                ];
                if needed.iter().any(|v| *v) {
                    occupied.future_loads.push(equipment::declared_loads(
                        material.ok_or(Error::Invalid)?,
                        needed,
                    )?);
                }
            }
        }
        // Constraint disclosure never includes hidden occupants or their individual demands.
        for (enclosure, occupied) in &enclosures {
            let definition = &occupied.definition;
            let limits = [
                definition.max_mass_g,
                definition.capacity_mw,
                definition.cooling_capacity_mw,
            ];
            if limits.iter().all(Option::is_none) {
                continue;
            }
            let envelope = definition
                .volume
                .as_ref()
                .map(volume)
                .transpose()?
                .ok_or(Error::Incomplete)?;
            require_coverage(definition, clearance::region(envelope, None)?)?;
            if limits[0].is_some() {
                for placement in placements(plan) {
                    if parse_target(placement.enclosure.as_ref())? != *enclosure {
                        continue;
                    }
                    let Some(item) = placement.actual(&plan.materials)? else {
                        continue;
                    };
                    let root = self.visible_asset(context, item)?;
                    if !context.field(item, number(&root, "kind")? as i32, "weight", false) {
                        return Err(Error::Denied.into());
                    }
                }
            }
            let mut demands =
                self.physical_loads(&occupied.items, limits.map(|value| value.is_some()))?;
            for demand in &occupied.future_loads {
                equipment::add_loads(&mut demands, *demand)?;
            }
            for (limit, demand) in limits.into_iter().zip(demands) {
                if let Some(limit) = limit {
                    let demand = demand.ok_or(Error::Incomplete)?;
                    if demand > limit {
                        // A conservative bound may include an old load and its successor.
                        // Do not claim a measured overload when the plan lacks a common release phase.
                        if !occupied.conditional_removals.is_empty() {
                            return Err(Error::Incomplete.into());
                        }
                        return Err(Failure::mismatch());
                    }
                }
            }
        }
        for part in &claims.parts {
            self.require_no_pending_operations(*part)?;
            self.check_physical_lock("physical-lock", *part, plan.id.as_deref())?;
        }
        for position in &claims.positions {
            self.check_physical_lock("physical-slot", *position, plan.id.as_deref())?;
        }
        Ok(claims)
    }

    fn placement_profiles(
        &self,
        plan: &api::PhysicalPlan,
    ) -> Result<BTreeMap<AssetRef, Properties>> {
        if plan.steps.len() > 64 {
            return Err(Error::Limit);
        }
        let mut selected = BTreeSet::new();
        for placement in placements(plan) {
            if let Some(item) = placement.actual(&plan.materials)? {
                selected.insert(item);
            }
            selected.insert(parse_target(placement.enclosure.as_ref())?);
        }
        if selected.is_empty() {
            return Ok(BTreeMap::new());
        }
        let keys = selected
            .iter()
            .map(|target| text(format!("physical/{}", network::identity(*target))))
            .collect();
        let rows=self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,mounting:n.mounting,counts:n.counts} AS p LIMIT 128",params([("keys",Value::Array(keys))]))?;
        let mut values = BTreeMap::new();
        for row in rows {
            values.insert(target(&row)?, row);
        }
        Ok(values)
    }

    pub(in crate::disk) fn check_physical_lock(
        &self,
        prefix: &str,
        target: AssetRef,
        owner: Option<&str>,
    ) -> Result<()> {
        if let Some(row) = self.get(&format!("{prefix}/{}", network::identity(target)))? {
            let actual = string(&row, "owner")?;
            if Some(actual.as_str()) != owner {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }

    fn check_replacement_capacity(
        &self,
        context: &Context,
        plan: &api::PhysicalPlan,
        working: &api::WorkingContext,
    ) -> core::result::Result<Claims, Failure> {
        let mut claims = Claims::default();
        let mut mass_limits = Vec::new();
        for replacement in replacement_steps(plan) {
            self.replacement_access(context, replacement, working, false)?;
            let destination = parse_target(replacement.position.as_ref())?;
            let destination_row = self.asset(destination)?;
            if !context.field(
                destination,
                number(&destination_row, "kind")? as i32,
                "physical_availability",
                false,
            ) {
                return Err(Error::Denied.into());
            }
            self.replacement_basis(replacement)
                .map_err(Failure::basis)?;
            if replacement.previous.is_none() {
                let position = self
                    .get(&format!("position/{}", network::identity(destination)))?
                    .ok_or(Error::Incomplete)?;
                if position.get("vflag") != Some(&Value::Bool(true)) {
                    return Err(Error::Incomplete.into());
                }
            }
            if let Some(incoming) = &replacement.replacement {
                let incoming = parse_target(Some(incoming))?;
                if number(&destination_row, "kind")? == wire::AssetKind::Rack as u64
                    && replacement.volume.is_none()
                {
                    return Err(Error::Incomplete.into());
                }
                let item = self
                    .physical_definition(incoming)?
                    .ok_or(Error::Incomplete)?;
                let target = self
                    .physical_definition(destination)?
                    .ok_or(Error::Incomplete)?;
                if replacement.volume.is_none()
                    && let Some(limit) = component_load_limits(incoming, &target, &item)?
                {
                    mass_limits.push(limit);
                }
                if number(&destination_row, "kind")? == wire::AssetKind::PhysicalPosition as u64 {
                    self.require_position_class(destination, item.component_kind)?;
                }
                require_mounting(target.mounting.as_deref(), item.mounting.as_deref())?;
                let target_volume = target
                    .volume
                    .as_ref()
                    .map(volume)
                    .transpose()?
                    .ok_or(Error::Incomplete)?;
                let item_volume = self
                    .equipment_envelope(context, incoming)?
                    .as_ref()
                    .map(volume)
                    .transpose()?
                    .ok_or(Error::Incomplete)?;
                let locate = |target: Span, item: Span| {
                    Span::new(
                        target.start(),
                        u64::try_from(item.end() - item.start()).map_err(|_| Error::Corrupt)?,
                    )
                    .map_err(|_| Error::Invalid)
                };
                let located = Volume {
                    x: locate(target_volume.x, item_volume.x)?,
                    y: locate(target_volume.y, item_volume.y)?,
                    z: locate(target_volume.z, item_volume.z)?,
                };
                let region = clearance::region(located, item.clearance.as_deref())?;
                if crate::physical::placement(Some(target_volume), Some(region), &[]).state
                    != crate::requirements::State::Satisfied
                {
                    return Err(Failure::mismatch());
                }
            }
            for part in [
                replacement.previous.as_ref(),
                replacement.replacement.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                claims.parts.insert(parse_target(Some(part))?);
            }
            for position in [
                replacement.position.as_ref(),
                replacement.source_position.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                let position = parse_target(Some(position))?;
                if number(&self.asset(position)?, "kind")?
                    == wire::AssetKind::PhysicalPosition as u64
                {
                    claims.positions.insert(position);
                }
            }
        }
        let removed: BTreeSet<_> = replacement_steps(plan)
            .filter_map(|replacement| replacement.previous_binding.as_deref())
            .collect();
        for replacement in replacement_steps(plan) {
            self.require_replacement_space(replacement, &removed)
                .map_err(|error| {
                    if matches!(error, Error::Conflict) {
                        Failure::mismatch()
                    } else {
                        error.into()
                    }
                })?;
        }
        self.check_replacement_masses(context, &mass_limits)?;
        Ok(claims)
    }

    fn require_position_class(
        &self,
        destination: AssetRef,
        incoming: i32,
    ) -> core::result::Result<(), Failure> {
        let position = self
            .get(&format!("position/{}", network::identity(destination)))?
            .ok_or(Error::Corrupt)?;
        let expected = number(&position, "vint")?;
        if expected == api::PhysicalComponentKind::Unspecified as u64
            || incoming == api::PhysicalComponentKind::Unspecified as i32
        {
            return Err(Error::Incomplete.into());
        }
        if expected != incoming as u64 {
            return Err(Failure::mismatch());
        }
        Ok(())
    }

    pub(super) fn check_replacement_class_and_loads(
        &self,
        context: &Context,
        replacements: &[&api::PhysicalReplacement],
        working: &api::WorkingContext,
    ) -> core::result::Result<(), Failure> {
        let mut mass_limits = Vec::new();
        for replacement in replacements {
            let Some(incoming) = replacement.replacement.as_ref() else {
                continue;
            };
            self.replacement_access(context, replacement, working, false)?;
            let incoming = parse_target(Some(incoming))?;
            let destination = parse_target(replacement.position.as_ref())?;
            let item = self
                .physical_definition(incoming)?
                .ok_or(Error::Incomplete)?;
            if number(&self.asset(destination)?, "kind")?
                == wire::AssetKind::PhysicalPosition as u64
            {
                self.require_position_class(destination, item.component_kind)?;
            }
            if replacement.volume.is_some() {
                continue;
            }
            let target = self
                .physical_definition(destination)?
                .ok_or(Error::Incomplete)?;
            if let Some(limit) = component_load_limits(incoming, &target, &item)? {
                mass_limits.push(limit);
            }
        }
        self.check_replacement_masses(context, &mass_limits)
    }

    fn check_replacement_masses(
        &self,
        context: &Context,
        mass_limits: &[(AssetRef, u64)],
    ) -> core::result::Result<(), Failure> {
        if !mass_limits.is_empty() {
            let selected: BTreeSet<_> = mass_limits.iter().map(|(item, _)| *item).collect();
            let keys = selected.iter().map(|item| text(asset_key(*item))).collect();
            let metadata = self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind} AS p LIMIT 64", params([("keys", Value::Array(keys))]))?;
            let metadata = metadata
                .into_iter()
                .map(|row| Ok((target(&row)?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            for item in &selected {
                let row = metadata.get(item).ok_or(Error::Conflict)?;
                if !context.field(*item, number(row, "kind")? as i32, "weight", false) {
                    return Err(Error::Denied.into());
                }
            }
            let keys = selected
                .iter()
                .map(|item| text(format!("{}/intent/weight", asset_key(*item))))
                .collect();
            let rows = self.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 64",
                params([("keys", Value::Array(keys))]),
            )?;
            let rows = rows
                .into_iter()
                .map(|row| Ok((string(&row, "key")?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            for &(item, limit) in mass_limits {
                let row = rows.get(&format!("{}/intent/weight", asset_key(item)));
                let demand = equipment::row_loads(None, row, [true, false, false])?[0]
                    .ok_or(Error::Incomplete)?;
                if demand > limit {
                    return Err(Failure::mismatch());
                }
            }
        }
        Ok(())
    }
}
