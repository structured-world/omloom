use super::*;

pub(super) fn reservation_key(plan: &str, index: usize) -> String {
    format!("reservation/{plan}/{index}")
}
impl Tx<'_> {
    pub(super) fn lock_physical_part(&mut self, item: AssetRef, plan: &str) -> Result<()> {
        self.require_no_pending_operations(item)?;
        let key = format!("physical-lock/{}", network::identity(item));
        if let Some(row) = self.get(&key)? {
            if string(&row, "owner")? != plan {
                return Err(Error::Conflict);
            }
            return Ok(());
        }
        self.put(
            &key,
            "physical-locks",
            Class::Pending,
            props([("owner", text(plan))]),
        )?;
        Ok(())
    }
    pub(super) fn clear_physical_reservation(&mut self, plan: &str, index: usize) -> Result<()> {
        let key = reservation_key(plan, index);
        if let Some(row) = self.get(&key)? {
            if string(&row, "claim")? != format!("plan/{plan}/{index}") {
                return Err(Error::Corrupt);
            }
            self.delete(&key)?;
        }
        Ok(())
    }
    pub(super) fn release_resolved_physical_locks(
        &mut self,
        plan: &api::PhysicalPlan,
    ) -> Result<()> {
        let owner = plan.id.as_deref().ok_or(Error::Corrupt)?;
        let mut resolved = BTreeSet::new();
        let mut pending = BTreeSet::new();
        for step in &plan.steps {
            let terminal = matches!(
                api::PhysicalStepState::try_from(step.state),
                Ok(api::PhysicalStepState::Recorded | api::PhysicalStepState::NotPerformed)
            );
            let keys = if terminal {
                &mut resolved
            } else {
                &mut pending
            };
            match step.action.as_ref().ok_or(Error::Corrupt)? {
                api::physical_step::Action::Placement(p) => {
                    if let Some(item) = Placement::from(p).actual(&plan.materials)? {
                        keys.insert(format!("physical-lock/{}", network::identity(item)));
                    }
                }
                api::physical_step::Action::Replacement(r) => {
                    for item in [r.previous.as_ref(), r.replacement.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        keys.insert(format!(
                            "physical-lock/{}",
                            network::identity(parse_target(Some(item))?)
                        ));
                    }
                    for item in [r.position.as_ref(), r.source_position.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        keys.insert(format!(
                            "physical-slot/{}",
                            network::identity(parse_target(Some(item))?)
                        ));
                    }
                }
                api::physical_step::Action::Wiring(w) => {
                    keys.insert(wiring::hold_key(
                        parse_target(w.cable.as_ref())?,
                        &w.end_key,
                    ));
                    for target in wiring::targets(w)? {
                        keys.insert(format!("physical-lock/{}", network::identity(target)));
                    }
                }
            }
        }
        // Keep unused BOM units for accepted future work, never after its cancellation.
        let future_work = plan.steps.iter().any(|s| {
            !matches!(
                api::PhysicalStepState::try_from(s.state),
                Ok(api::PhysicalStepState::Recorded | api::PhysicalStepState::NotPerformed)
            )
        }) && matches!(
            api::PhysicalPlanState::try_from(plan.state),
            Ok(api::PhysicalPlanState::Reserved | api::PhysicalPlanState::Partial)
        );
        for allocation in plan.materials.iter().flat_map(|m| &m.allocations) {
            let key = format!(
                "physical-lock/{}",
                network::identity(parse_target(allocation.item.as_ref())?)
            );
            if future_work && !resolved.contains(&key) {
                pending.insert(key);
            } else {
                resolved.insert(key);
            }
        }
        for key in resolved.difference(&pending) {
            if let Some(row) = self.get(key)? {
                if string(&row, "owner")? == owner {
                    self.delete(key)?;
                }
            }
        }
        Ok(())
    }
}
