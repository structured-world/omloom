//! One accepted inventory creation and sparse allocation share the existing commit/outcome.
use super::*;
use addresses::knowledge::Outcome;
#[cfg(test)]
mod tests;

impl Store {
    pub(super) fn commit_receipt(
        &self,
        mut tx: Tx<'_>,
        context: &Context,
        request: &api::PhysicalDeviceReceipt,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        let receipt = tx.receive_device(context, request, working, now)?;
        let (revision, receipt) = match tx.commit() {
            Ok(revision) => (revision, receipt),
            Err(Error::Conflict) => {
                // Only reread the winner; never execute this mutation again.
                let fresh = self.begin()?;
                let key = request.plan.as_ref().ok_or(Error::Invalid)?;
                let winner = fresh
                    .creation_outcome(
                        context,
                        working,
                        &request.encode_to_vec(),
                        Outcome::Receipt {
                            plan: &key.id,
                            material: request.material,
                            unit: request.unit,
                        },
                    )
                    .map_err(|error| match error {
                        Error::Conflict => Error::Rejected(Box::new(Error::Conflict)),
                        error => error,
                    })?;
                if winner.is_none() {
                    let base = working.expected.ok_or(Error::Conflict)?;
                    // One intervening commit cannot both accept and expire an outcome.
                    // A larger revision gap can contain an accepted, reclaimed result.
                    if number(&fresh.root, "revision")?
                        == base.checked_add(1).ok_or(Error::Limit)?
                    {
                        return Err(Error::Rejected(Box::new(Error::Conflict)));
                    }
                    return Err(Error::Conflict);
                }
                let receipt = fresh.receipt_outcome(
                    context,
                    working.expected.ok_or(Error::Conflict)?,
                    &key.id,
                )?;
                (number(&fresh.root, "revision")?, receipt)
            }
            Err(error) => return Err(error),
        };
        Ok(reply(api::reply::Result::Physical(api::PhysicalView {
            revision,
            receipt: Some(Box::new(receipt)),
            state: api::RequirementState::Unknown as i32,
            reason: api::RequirementReason::Missing as i32,
            ..Default::default()
        })))
    }
}

impl Tx<'_> {
    pub(super) fn recover_receipt(
        &mut self,
        context: &Context,
        lookup: &api::PhysicalReceiptLookup,
        working: &api::WorkingContext,
        view: &mut api::PhysicalView,
    ) -> Result<()> {
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        let current = number(&self.root, "revision")?;
        if lookup.revision == 0 || lookup.revision > current {
            return Err(Error::Invalid);
        }
        // Loading the current plan establishes its owner and ordinary operand rights.
        let plan = self.load_physical_plan(
            context,
            &api::PhysicalPlanKey {
                id: lookup.plan.clone(),
                revision: 0,
            },
            working,
            current == lookup.revision,
        )?;
        match self.receipt_outcome(context, lookup.revision, &lookup.plan) {
            Ok(outcome) => {
                view.receipt = Some(Box::new(outcome));
            }
            Err(Error::Conflict) => {
                view.receipt_recovery = api::PhysicalReceiptRecovery::NotCommitted as i32;
            }
            Err(Error::Denied) => {
                // A different actor's outcome cannot belong to this plan owner.
                // Resource-rights loss on our own result must remain restricted.
                let row = self
                    .get(&format!("mutation-outcome/{}", lookup.revision))?
                    .ok_or(Error::Expired)?;
                if string(&row, "actor")? == context.actor {
                    return Err(Error::Denied);
                }
                view.receipt_recovery = api::PhysicalReceiptRecovery::NotCommitted as i32;
            }
            Err(Error::Expired) => {
                if current == lookup.revision {
                    // Advancing the existing root CAS fences every in-flight old snapshot.
                    // No receipt ledger, inventory mutation or retry is needed.
                    self.changed = true;
                    view.receipt_recovery = api::PhysicalReceiptRecovery::NotCommitted as i32;
                } else if current == lookup.revision.checked_add(1).ok_or(Error::Invalid)? {
                    view.receipt_recovery = api::PhysicalReceiptRecovery::NotCommitted as i32;
                } else {
                    view.receipt_recovery = api::PhysicalReceiptRecovery::Unavailable as i32;
                }
            }
            Err(error) => return Err(error),
        }
        view.plan = Some(plan);
        Ok(())
    }

    pub(super) fn admit_plan_operands(
        &mut self,
        context: &Context,
        id: &str,
        working: &api::WorkingContext,
    ) -> Result<()> {
        self.selection(context, working, false)?;
        PlanId::parse(id).map_err(|_| Error::Invalid)?;
        let row = self.get(&format!("plan/{id}"))?.ok_or(Error::Missing)?;
        if string(&row, "actor")? != context.actor {
            return Err(Error::Denied);
        }
        if string(&row, "action")? != "physical" {
            return Err(Error::Invalid);
        }
        if working.roots.is_empty() && working.focus.is_none() {
            return Ok(());
        }
        let count = row
            .get("material_count")
            .map(|_| number(&row, "material_count"))
            .transpose()?
            .unwrap_or(0);
        let materials = self.load_materials(
            &format!("plan/{id}"),
            usize::try_from(count).map_err(|_| Error::Corrupt)?,
            number(&row, "class")? == Class::Detail as u64,
        )?;
        let mut allocations = BTreeSet::new();
        for allocation in materials.iter().flat_map(|m| &m.allocations) {
            let target = parse_target(allocation.item.as_ref())?;
            allocations.insert(target);
            if allocations.len() > MAX_BULK {
                return Err(Error::Limit);
            }
        }
        if !allocations.is_empty() {
            let keys = allocations
                .iter()
                .map(|target| text(asset_key(*target)))
                .collect();
            let rows = self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption} AS p LIMIT 64", params([("keys", Value::Array(keys))]))?;
            let metadata = rows
                .into_iter()
                .map(|row| Ok((target(&row)?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            for target in &allocations {
                self.require_visible_asset(
                    context,
                    *target,
                    metadata.get(target).ok_or(Error::Missing)?,
                )?;
            }
        }
        self.physical_operands.extend(allocations);
        Ok(())
    }

    pub(super) fn receipt_outcome(
        &self,
        context: &Context,
        base: u64,
        expected_plan: &str,
    ) -> Result<api::PhysicalReceiptOutcome> {
        PlanId::parse(expected_plan).map_err(|_| Error::Invalid)?;
        let row = self
            .get(&format!("mutation-outcome/{base}"))?
            .ok_or(Error::Expired)?;
        if string(&row, "actor")? != context.actor {
            return Err(Error::Denied);
        }
        if row.get("action").and_then(Value::as_str) != Some("physical-receipt") {
            return Err(Error::Conflict);
        }
        let Some(Value::Array(targets)) = row.get("targets") else {
            return Err(Error::Corrupt);
        };
        if targets.len() != 1 {
            return Err(Error::Corrupt);
        }
        let target = network::groups::reference(targets[0].as_str().ok_or(Error::Corrupt)?)?;
        self.physical_access(context, target, &api::WorkingContext::default(), false)?;
        let id = string(&row, "owner")?;
        if id != expected_plan {
            return Err(Error::Conflict);
        }
        // The retained outcome owns this immutable actor/plan binding even
        // after terminal-plan cleanup. Device disclosure still uses current rights.
        let revision = base.checked_add(1).ok_or(Error::Corrupt)?;
        Ok(api::PhysicalReceiptOutcome {
            revision,
            item: Some(target_wire(target)),
            plan: Some(api::PhysicalPlanKey { id, revision }),
            material: u32::try_from(number(&row, "count")?).map_err(|_| Error::Corrupt)?,
            unit: u32::try_from(number(&row, "generation")?).map_err(|_| Error::Corrupt)?,
        })
    }

    pub(super) fn receive_device(
        &mut self,
        context: &Context,
        request: &api::PhysicalDeviceReceipt,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalReceiptOutcome> {
        // These malformed inputs cannot have been accepted under this command contract.
        let reject = |cause| Error::Rejected(Box::new(cause));
        let base = working.expected.ok_or_else(|| reject(Error::Conflict))?;
        let key = request
            .plan
            .as_ref()
            .ok_or_else(|| reject(Error::Invalid))?;
        let inventory = request
            .inventory
            .as_ref()
            .ok_or_else(|| reject(Error::Invalid))?;
        if inventory.expected.is_some() || inventory.focus.is_some() || !inventory.roots.is_empty()
        {
            return Err(reject(Error::Invalid));
        }
        if request.changes.len() > MAX_FIELDS {
            return Err(reject(Error::Limit));
        }
        if request
            .explanation
            .as_ref()
            .is_some_and(|v| v.len() > 160 || v.chars().any(char::is_control))
        {
            return Err(reject(Error::Invalid));
        }
        let input = request.encode_to_vec();
        let outcome = || Outcome::Receipt {
            plan: &key.id,
            material: request.material,
            unit: request.unit,
        };
        // The fingerprint includes both contexts; lookup precedes fresh lifecycle/CAS admission.
        let previous = self
            .creation_outcome(context, working, &input, outcome())
            .map_err(|error| match error {
                // A different committed fingerprint proves this input was not accepted.
                Error::Conflict => Error::Rejected(Box::new(Error::Conflict)),
                error => error,
            })?;
        if previous.is_some() {
            return self.receipt_outcome(context, base, &key.id);
        }
        // Several commits can include acceptance followed by outcome cleanup.
        // Fresh-admission failures cannot establish what that old request did.
        if number(&self.root, "revision")? > base.checked_add(1).ok_or(Error::Limit)? {
            return Err(Error::Conflict);
        }
        (|| {
            self.selection(context, working, true)?;
            self.admit_plan_operands(context, &key.id, working)?;
            let plan = self.load_physical_plan(context, key, working, true)?;
            let material = plan
                .materials
                .get(request.material as usize)
                .ok_or(Error::Invalid)?;
            if material.kind != wire::AssetKind::Device as i32 || request.unit >= material.quantity
            {
                return Err(Error::Invalid);
            }
            if material.allocations.iter().any(|a| a.unit == request.unit) {
                return Err(Error::Conflict);
            }
            if plan
                .materials
                .iter()
                .map(|m| m.allocations.len())
                .sum::<usize>()
                >= 64
            {
                return Err(Error::Limit);
            }
            let mut creation = inventory.clone();
            creation.expected = Some(base);
            let item = self.context_write(
                context,
                api::Write {
                    kind: wire::AssetKind::Device as i32,
                    changes: request.changes.clone(),
                    ..Default::default()
                },
                &creation,
                now,
            )?;
            self.physical_operands.insert(item);
            let mut basis = format!(
                "Receipt asserted by {} at {now} ms UTC (recording time)",
                context.actor
            );
            if let Some(explanation) = &request.explanation
                && !explanation.trim().is_empty()
            {
                basis.push_str(": ");
                basis.push_str(explanation);
            }
            self.receive_material(
                context,
                api::PhysicalMaterialReceipt {
                    plan: Some(key.clone()),
                    material: request.material,
                    allocation: Some(api::PhysicalMaterialAllocation {
                        unit: request.unit,
                        item: Some(target_wire(item)),
                        basis,
                    }),
                },
                working,
                now,
            )?;
            self.save_creation_outcome(
                context,
                working,
                &input,
                &[target_wire(item)],
                now,
                outcome(),
            )?;
            self.receipt_outcome(context, base, &key.id)
        })()
        .map_err(|error| Error::Rejected(Box::new(error)))
    }
}
