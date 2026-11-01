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
        let mut claims = self.check_replacement_capacity(context, plan, working)?;
        let graph = flow::analysis(&plan.steps)?;
        self.assess_wiring(context, plan, working, &graph, &mut claims)?;
        let profiles = self.placement_profiles(plan)?;
        let mut enclosures = BTreeMap::<AssetRef, current::PhysicalUse>::new();
        let mut rows_read = 0;
        for (index, p) in placement_steps(plan) {
            let item = parse_target(p.item.as_ref())?;
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
            claims.parts.insert(item);
            if !self.installed_bindings(item)?.is_empty()
                && !replacement_steps(plan).any(|r| r.previous.as_ref() == p.item.as_ref())
            {
                return Err(Failure::mismatch());
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
            if mounting(&enclosure).is_some() || mounting(&item).is_some() {
                require_mounting(mounting(&enclosure), mounting(&item))?;
            }
            let item_volume = self
                .equipment_envelope(context, item)?
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
            let margins = profiles
                .get(&item)
                .map(clearance::read)
                .transpose()?
                .flatten();
            let region = clearance::region(proposed, margins.as_ref())?;
            match crate::physical::placement(envelope, Some(region), &occupied.volumes).state {
                crate::requirements::State::Satisfied => {}
                crate::requirements::State::Failed => return Err(Failure::mismatch()),
                crate::requirements::State::Unknown => return Err(Error::Incomplete.into()),
            }
            for (removal, prior) in &occupied.conditional_removals {
                if graph.ancestors[index] & (1u64 << removal) != 0 {
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
            occupied.items.insert(item);
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
                    let item = parse_target(placement.item.as_ref())?;
                    let root = self.visible_asset(context, item)?;
                    if !context.field(item, number(&root, "kind")? as i32, "weight", false) {
                        return Err(Error::Denied.into());
                    }
                }
            }
            let demands =
                self.physical_loads(&occupied.items, limits.map(|value| value.is_some()))?;
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
            selected.insert(parse_target(placement.item.as_ref())?);
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

    fn check_physical_lock(
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
                if number(&destination_row, "kind")? == wire::AssetKind::Rack as u64 {
                    let mut has_region = false;
                    for placement in placements(plan) {
                        if parse_target(placement.item.as_ref())? == incoming
                            && parse_target(placement.enclosure.as_ref())? == destination
                        {
                            has_region = true;
                            break;
                        }
                    }
                    if !has_region {
                        return Err(Error::Incomplete.into());
                    }
                }
                let item = self
                    .physical_definition(incoming)?
                    .ok_or(Error::Incomplete)?;
                let target = self
                    .physical_definition(destination)?
                    .ok_or(Error::Incomplete)?;
                if number(&destination_row, "kind")? == wire::AssetKind::PhysicalPosition as u64 {
                    let position = self
                        .get(&format!("position/{}", network::identity(destination)))?
                        .ok_or(Error::Corrupt)?;
                    let expected = number(&position, "vint")?;
                    if expected == api::PhysicalComponentKind::Unspecified as u64
                        || item.component_kind == api::PhysicalComponentKind::Unspecified as i32
                    {
                        return Err(Error::Incomplete.into());
                    }
                    if expected != item.component_kind as u64 {
                        return Err(Failure::mismatch());
                    }
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
                let region = clearance::region(located, item.clearance.as_ref())?;
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
        Ok(claims)
    }
}
