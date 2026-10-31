use super::*;

impl Tx<'_> {
    pub(super) fn installed_bindings(&self, item: AssetRef) -> Result<Vec<wire::Relation>> {
        let rows=self.query("MATCH (a:OmRow {key:$key})-[:OmRelationFrom]->(r:OmRow) WHERE r.kind=$kind RETURN properties(r) AS p LIMIT 2",params([("key",text(asset_key(item))),("kind",int(wire::RelationKind::InstalledIn as u64)?)]))?;
        if rows.len() > 1 {
            return Err(Error::Conflict);
        }
        rows.iter().map(inventory::relation_projection).collect()
    }
    pub(super) fn record_physical_placement(
        &mut self,
        context: &Context,
        placement: &api::PhysicalPlacement,
        report: &api::PhysicalCompletion,
        index: usize,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        let plan = report.plan.as_ref().ok_or(Error::Invalid)?.id.as_str();
        let item = parse_target(placement.item.as_ref())?;
        let enclosure = parse_target(placement.enclosure.as_ref())?;
        self.physical_access(context, item, working, true)?;
        self.physical_access(context, enclosure, working, true)?;
        self.require_no_pending_operations(item)?;
        let reservation = self.get(&reservations::reservation_key(plan, index))?;
        if reservation.is_none() && !report.actual_state {
            return Err(Error::Conflict);
        }
        if let Some(row) = &reservation
            && string(row, "claim")? != format!("plan/{plan}/{index}")
        {
            return Err(Error::Corrupt);
        }
        if let Some(lock) = self.get(&format!("physical-lock/{}", network::identity(item)))?
            && string(&lock, "owner")? != plan
        {
            return Err(Error::Conflict);
        }
        let existing = self.installed_bindings(item)?;
        let binding = if let Some(existing) = existing.first() {
            if existing.to != placement.enclosure {
                return Err(Error::UnsafeTransition);
            }
            existing.clone()
        } else {
            let relation = wire::Relation {
                relation_id: RelationId::generate().to_string(),
                from: Some(target_wire(item)),
                to: Some(target_wire(enclosure)),
                kind: wire::RelationKind::InstalledIn as i32,
                revision: 1,
                authority_id: self.workspace_authority()?,
            };
            let selection = self.selection(context, working, true)?;
            self.check_relationship(context, &selection, &relation)?;
            self.save_typed_relation(&relation, None, &context.actor, now)?;
            relation
        };
        self.store_physical_pose(
            &binding,
            placement.volume.as_ref().ok_or(Error::Invalid)?,
            &context.actor,
        )?;
        self.clear_physical_reservation(plan, index)?;
        Ok(())
    }
    pub(super) fn complete_physical_steps(
        &mut self,
        context: &Context,
        value: api::PhysicalCompletion,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalPlan> {
        let key = value.plan.as_ref().ok_or(Error::Invalid)?;
        if key.revision == 0
            || value.completed_steps.len() + value.not_performed_steps.len() > 64
            || value.discrepancy.as_ref().is_some_and(|s| s.len() > 2048)
        {
            return Err(Error::Invalid);
        }
        let mut plan = self.load_physical_plan(context, key, working, true)?;
        let state = api::PhysicalPlanState::try_from(plan.state).map_err(|_| Error::Corrupt)?;
        if !matches!(
            state,
            api::PhysicalPlanState::Reserved
                | api::PhysicalPlanState::Partial
                | api::PhysicalPlanState::Cancelled
        ) && !(value.actual_state && state == api::PhysicalPlanState::Draft)
        {
            return Err(Error::UnsafeTransition);
        }
        let mut selected = BTreeMap::new();
        for (indexes, done) in [
            (&value.completed_steps, true),
            (&value.not_performed_steps, false),
        ] {
            for index in indexes {
                if *index as usize >= plan.steps.len() || selected.insert(*index, done).is_some() {
                    return Err(Error::Invalid);
                }
            }
        }
        if value.actual_state && state == api::PhysicalPlanState::Draft {
            for step in &plan.steps {
                match step.action.as_ref().ok_or(Error::Corrupt)? {
                    api::physical_step::Action::Placement(p) => {
                        self.lock_physical_part(parse_target(p.item.as_ref())?, &key.id)?
                    }
                    api::physical_step::Action::Replacement(r) => {
                        for part in [r.previous.as_ref(), r.replacement.as_ref()]
                            .into_iter()
                            .flatten()
                        {
                            self.lock_physical_part(parse_target(Some(part))?, &key.id)?;
                        }
                    }
                }
            }
        }
        let replacements = selected
            .iter()
            .filter_map(|(&index, &done)| {
                let step = &plan.steps[index as usize];
                if !done || step.state == api::PhysicalStepState::Recorded as i32 {
                    return None;
                }
                match &step.action {
                    Some(api::physical_step::Action::Replacement(r)) => Some(r),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        if !replacements.is_empty() {
            self.record_physical_replacements(context, &replacements, &key.id, working, now)?;
        }
        let mut changed = false;
        // Explicit hardware transitions precede placement realization in this atomic report.
        for replacements_first in [true, false] {
            for (&index, &done) in &selected {
                let step = &mut plan.steps[index as usize];
                let action = step.action.as_ref().ok_or(Error::Corrupt)?;
                if matches!(action, api::physical_step::Action::Replacement(_))
                    != replacements_first
                {
                    continue;
                }
                let desired = if done {
                    api::PhysicalStepState::Recorded
                } else {
                    api::PhysicalStepState::NotPerformed
                };
                if step.state == desired as i32 {
                    continue;
                }
                if matches!(
                    api::PhysicalStepState::try_from(step.state),
                    Ok(api::PhysicalStepState::Recorded | api::PhysicalStepState::NotPerformed)
                ) {
                    return Err(Error::Conflict);
                }
                if done {
                    match action {
                        api::physical_step::Action::Placement(p) => self
                            .record_physical_placement(
                                context,
                                p,
                                &value,
                                index as usize,
                                working,
                                now,
                            )?,
                        api::physical_step::Action::Replacement(_) => {}
                    }
                } else {
                    self.clear_physical_reservation(&key.id, index as usize)?;
                }
                step.state = desired as i32;
                self.put(
                    &format!("plan/{}/{index}", key.id),
                    &format!("plan/{}", key.id),
                    Class::Inventory,
                    step_row(step)?,
                )?;
                changed = true;
            }
        }
        if let Some(discrepancy) = value.discrepancy {
            let header = format!("plan/{}", key.id);
            let mut row = self.get(&header)?.ok_or(Error::Corrupt)?;
            if row.get("description").and_then(Value::as_str) != Some(discrepancy.as_str()) {
                operations::strip_envelope(&mut row);
                row.insert("description".into(), text(discrepancy));
                self.put(&header, "plans", Class::Inventory, row)?;
                changed = true;
            }
        }
        if !changed {
            return Ok(plan);
        }
        self.release_resolved_physical_locks(&plan)?;
        let terminal = plan.steps.iter().all(|step| {
            matches!(
                api::PhysicalStepState::try_from(step.state),
                Ok(api::PhysicalStepState::Recorded | api::PhysicalStepState::NotPerformed)
            )
        });
        let next = if terminal {
            plan.reserved_until_ms = None;
            if plan
                .steps
                .iter()
                .any(|s| s.state == api::PhysicalStepState::Recorded as i32)
            {
                api::PhysicalPlanState::Completed
            } else {
                api::PhysicalPlanState::Cancelled
            }
        } else {
            api::PhysicalPlanState::Partial
        };
        self.physical_state(context, &mut plan, next, now)?;
        Ok(plan)
    }
}
