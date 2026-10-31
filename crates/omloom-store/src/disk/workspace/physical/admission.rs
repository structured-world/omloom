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
        self.assess_wiring(context, plan, working, &mut claims)?;
        let mut enclosures = BTreeMap::<AssetRef, current::PhysicalUse>::new();
        let mut rows_read = 0;
        for p in placements(plan) {
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
                let occupied = self.physical_use(enclosure, plan, 2048 - rows_read)?;
                rows_read += occupied.inspected;
                entry.insert(occupied);
            }
            let definition = self
                .physical_definition(enclosure)?
                .ok_or(Error::Incomplete)?;
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
            let occupied = enclosures.get_mut(&enclosure).ok_or(Error::Corrupt)?;
            match crate::physical::placement(envelope, Some(proposed), &occupied.volumes).state {
                crate::requirements::State::Satisfied => {}
                crate::requirements::State::Failed => return Err(Failure::mismatch()),
                crate::requirements::State::Unknown => return Err(Error::Incomplete.into()),
            }
            occupied.volumes.push(proposed);
            occupied.items.insert(item);
        }
        // Constraint disclosure never includes hidden occupants or their individual demands.
        for (enclosure, occupied) in &enclosures {
            let definition = self
                .physical_definition(*enclosure)?
                .ok_or(Error::Incomplete)?;
            let limits = [
                definition.max_mass_g,
                definition.capacity_mw,
                definition.cooling_capacity_mw,
            ];
            if limits.iter().all(Option::is_none) {
                continue;
            }
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
                match (&target.mounting, &item.mounting) {
                    (Some(required), Some(actual)) if required == actual => {}
                    (Some(_), Some(_)) => return Err(Failure::mismatch()),
                    _ => return Err(Error::Incomplete.into()),
                }
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
                if !target_volume.contains(located) {
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
