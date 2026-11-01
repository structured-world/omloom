use super::*;

pub(super) fn reference(value: &str) -> Result<AssetRef> {
    let (asset, incarnation) = value.split_once('/').ok_or(Error::Corrupt)?;
    Ok(AssetRef {
        asset: asset.parse().map_err(|_| Error::Corrupt)?,
        incarnation: incarnation.parse().map_err(|_| Error::Corrupt)?,
    })
}
fn key(value: &api::PhysicalPlanKey) -> Result<String> {
    let id = PlanId::parse(&value.id).map_err(|_| Error::Invalid)?;
    Ok(format!("plan/{id}"))
}
impl Tx<'_> {
    pub(super) fn load_physical_plan(
        &self,
        context: &Context,
        request: &api::PhysicalPlanKey,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<api::PhysicalPlan> {
        let key = key(request)?;
        let row = self.get(&key)?.ok_or(Error::Missing)?;
        if string(&row, "action")? != "physical" {
            return Err(Error::Invalid);
        }
        if string(&row, "actor")? != context.actor {
            return Err(Error::Denied);
        }
        if request.revision != 0 && number(&row, "revision")? != request.revision {
            return Err(Error::Conflict);
        }
        let state = i32::try_from(number(&row, "state")?).map_err(|_| Error::Corrupt)?;
        let terminal = matches!(
            api::PhysicalPlanState::try_from(state),
            Ok(api::PhysicalPlanState::Completed | api::PhysicalPlanState::Cancelled)
        );
        let count = number(&row, "count")?;
        if count > 64 {
            return Err(Error::Corrupt);
        }
        let materials = self.load_materials(
            &key,
            if row.contains_key("material_count") {
                usize::try_from(number(&row, "material_count")?).map_err(|_| Error::Corrupt)?
            } else {
                0
            },
            number(&row, "class")? == Class::Detail as u64,
        )?;
        let mut steps = Vec::with_capacity(count as usize);
        for i in 0..count {
            let p = self.get(&format!("{key}/{i}"))?.ok_or(if terminal {
                Error::Expired
            } else {
                Error::Corrupt
            })?;
            let step = read_step(&p)?;
            match step.action.as_ref().ok_or(Error::Corrupt)? {
                api::physical_step::Action::Placement(p) => {
                    if let Some(item) = Placement::from(p).actual(&materials)? {
                        self.physical_access(context, item, working, write)?;
                    }
                    self.physical_access(
                        context,
                        parse_target(p.enclosure.as_ref())?,
                        working,
                        write,
                    )?;
                }
                api::physical_step::Action::Replacement(r) => {
                    self.replacement_access(context, r, working, write)?
                }
                api::physical_step::Action::Wiring(w) => {
                    self.wiring_access(context, w, working, write)?
                }
            }
            steps.push(step);
        }
        let plan = api::PhysicalPlan {
            id: Some(request.id.clone()),
            name: string(&row, "name")?,
            steps,
            materials,
            state,
            revision: number(&row, "revision")?,
            reserved_until_ms: if row.contains_key("until") {
                Some(number(&row, "until")?)
            } else {
                None
            },
        };
        if !plan.materials.is_empty() {
            self.material_access(context, &plan.materials, working, write)?;
        }
        Ok(plan)
    }
    pub(super) fn physical_state(
        &mut self,
        context: &Context,
        plan: &mut api::PhysicalPlan,
        state: api::PhysicalPlanState,
        now: u64,
    ) -> Result<()> {
        let id = plan.id.as_ref().ok_or(Error::Corrupt)?;
        let key = format!("plan/{id}");
        let mut row = self.get(&key)?.ok_or(Error::Corrupt)?;
        operations::strip_envelope(&mut row);
        plan.state = state as i32;
        plan.revision = number(&self.root, "revision")?
            .checked_add(1)
            .ok_or(Error::Limit)?;
        row.insert("state".into(), int(state as u64)?);
        row.insert("revision".into(), int(plan.revision)?);
        if let Some(until) = plan.reserved_until_ms {
            row.insert("until".into(), int(until)?);
        } else {
            row.remove("until");
        }
        let terminal_detail = matches!(
            state,
            api::PhysicalPlanState::Completed | api::PhysicalPlanState::Cancelled
        ) && plan.steps.iter().all(|step| {
            matches!(
                api::PhysicalStepState::try_from(step.state),
                Ok(api::PhysicalStepState::Recorded | api::PhysicalStepState::NotPerformed)
            )
        });
        let class = if terminal_detail {
            Class::Detail
        } else {
            Class::Inventory
        };
        if terminal_detail {
            row.insert("at".into(), int(now)?);
            if !plan.materials.is_empty() {
                self.retire_materials(&key, now)?;
            }
            for index in 0..plan.steps.len() {
                let child_key = format!("{key}/{index}");
                let mut child = self.get(&child_key)?.ok_or(Error::Corrupt)?;
                operations::strip_envelope(&mut child);
                child.insert("at".into(), int(now)?);
                self.put(&child_key, &key, Class::Detail, child)?;
            }
        }
        self.put(&key, "plans", class, row)?;
        self.audit(id, &context.actor, "physical-plan", plan.revision, now)?;
        Ok(())
    }
    pub(super) fn reserve_physical(
        &mut self,
        context: &Context,
        key: &api::PhysicalPlanKey,
        working: &api::WorkingContext,
        now: u64,
        until: u64,
    ) -> Result<api::PhysicalPlan> {
        if key.revision == 0 || until <= now {
            return Err(Error::Invalid);
        }
        int(until)?;
        let mut plan = self.load_physical_plan(context, key, working, true)?;
        if plan.state != api::PhysicalPlanState::Draft as i32 {
            return Err(Error::Conflict);
        }
        let claims = self
            .assess_physical(context, &plan, working)
            .map_err(|failure| failure.error)?;
        self.acquire_physical_claims(claims, &key.id)?;
        for (index, p) in placement_steps(&plan) {
            let enclosure = parse_target(p.enclosure.as_ref())?;
            self.put(
                &reservations::reservation_key(&key.id, index),
                &format!("physical-reservations/{}", network::identity(enclosure)),
                Class::Pending,
                props([("claim", text(format!("plan/{}/{index}", key.id)))]),
            )?;
        }
        for (index, step) in plan.steps.iter_mut().enumerate() {
            step.state = api::PhysicalStepState::Reserved as i32;
            self.put(
                &format!("plan/{}/{index}", key.id),
                &format!("plan/{}", key.id),
                Class::Inventory,
                step_row(step)?,
            )?;
        }
        plan.reserved_until_ms = Some(until);
        self.physical_state(context, &mut plan, api::PhysicalPlanState::Reserved, now)?;
        Ok(plan)
    }
    pub(super) fn cancel_physical(
        &mut self,
        context: &Context,
        key: &api::PhysicalPlanKey,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalPlan> {
        if key.revision == 0 {
            return Err(Error::Invalid);
        }
        let mut plan = self.load_physical_plan(context, key, working, true)?;
        if plan.state == api::PhysicalPlanState::Cancelled as i32 {
            return Ok(plan);
        }
        if plan.state == api::PhysicalPlanState::Completed as i32 {
            return Err(Error::UnsafeTransition);
        }
        let held = plan.state != api::PhysicalPlanState::Draft as i32;
        // Cancelling future intent never silently releases unresolved reserved work.
        if plan.state == api::PhysicalPlanState::Draft as i32 {
            for (index, step) in plan.steps.iter_mut().enumerate() {
                step.state = api::PhysicalStepState::NotPerformed as i32;
                self.put(
                    &format!("plan/{}/{index}", key.id),
                    &format!("plan/{}", key.id),
                    Class::Inventory,
                    step_row(step)?,
                )?;
            }
        }
        self.physical_state(context, &mut plan, api::PhysicalPlanState::Cancelled, now)?;
        if held {
            self.release_resolved_physical_locks(&plan)?;
        }
        Ok(plan)
    }
}
