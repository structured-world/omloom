//! One accepted inventory creation and sparse allocation share the existing commit/outcome.
use super::*;
use addresses::knowledge::Outcome;

impl Tx<'_> {
    pub(super) fn plan_context(
        &self,
        context: &Context,
        id: &str,
        working: &api::WorkingContext,
    ) -> Result<api::WorkingContext> {
        self.selection(context, working, false)?;
        PlanId::parse(id).map_err(|_| Error::Invalid)?;
        let row = self.get(&format!("plan/{id}"))?.ok_or(Error::Missing)?;
        if string(&row, "actor")? != context.actor {
            return Err(Error::Denied);
        }
        if string(&row, "action")? != "physical" {
            return Err(Error::Invalid);
        }
        let mut selected = working.clone();
        if working.roots.is_empty() && working.focus.is_none() {
            return Ok(selected);
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
        let mut roots = working
            .roots
            .iter()
            .map(|r| parse_target(Some(r)))
            .collect::<Result<BTreeSet<_>>>()?;
        if let Some(focus) = &working.focus {
            roots.insert(parse_target(Some(focus))?);
        }
        for allocation in materials.iter().flat_map(|m| &m.allocations) {
            let target = parse_target(allocation.item.as_ref())?;
            self.visible_asset(context, target)?;
            roots.insert(target);
            if roots.len() > MAX_BULK {
                return Err(Error::Limit);
            }
        }
        selected.roots = roots.into_iter().map(target_wire).collect();
        selected.focus = None;
        Ok(selected)
    }

    pub(super) fn receipt_outcome(
        &self,
        context: &Context,
        base: u64,
        expected_plan: &str,
    ) -> Result<api::PhysicalReceiptOutcome> {
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
        let plan = self.get(&format!("plan/{id}"))?.ok_or(Error::Expired)?;
        if string(&plan, "actor")? != context.actor {
            return Err(Error::Denied);
        }
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
        request: api::PhysicalDeviceReceipt,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalReceiptOutcome> {
        let base = working.expected.ok_or(Error::Conflict)?;
        let key = request.plan.as_ref().ok_or(Error::Invalid)?;
        let inventory = request.inventory.as_ref().ok_or(Error::Invalid)?;
        if inventory.expected != Some(base)
            || inventory.focus.is_some()
            || !inventory.roots.is_empty()
        {
            return Err(Error::Invalid);
        }
        if request.changes.len() > MAX_FIELDS {
            return Err(Error::Limit);
        }
        if request
            .explanation
            .as_ref()
            .is_some_and(|v| v.len() > 480 || v.chars().any(char::is_control))
        {
            return Err(Error::Invalid);
        }
        let input = request.encode_to_vec();
        let outcome = || Outcome::Receipt {
            plan: &key.id,
            material: request.material,
            unit: request.unit,
        };
        // The fingerprint includes both contexts; lookup precedes fresh lifecycle/CAS admission.
        if self
            .creation_outcome(context, working, &input, outcome())?
            .is_some()
        {
            return self.receipt_outcome(context, base, &key.id);
        }
        self.selection(context, working, true)?;
        let plan_working = self.plan_context(context, &key.id, working)?;
        let plan = self.load_physical_plan(context, key, &plan_working, true)?;
        let material = plan
            .materials
            .get(request.material as usize)
            .ok_or(Error::Invalid)?;
        if material.kind != wire::AssetKind::Device as i32 || request.unit >= material.quantity {
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
        let item = self.context_write(
            context,
            api::Write {
                kind: wire::AssetKind::Device as i32,
                changes: request.changes.clone(),
                ..Default::default()
            },
            inventory,
            now,
        )?;
        let mut selected = plan_working;
        if !selected.roots.is_empty() {
            selected.roots.push(target_wire(item));
        }
        let basis = match request.explanation.as_deref() {
            Some(value) if !value.trim().is_empty() => format!("Receipt asserted: {value}"),
            _ => "Receipt asserted".into(),
        };
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
            &selected,
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
    }
}
