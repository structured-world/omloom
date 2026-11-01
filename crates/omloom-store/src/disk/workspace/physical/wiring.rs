use super::*;

fn end_parent(cable: AssetRef) -> String {
    format!("cable-ends/{}", network::identity(cable))
}
pub(super) fn hold_key(cable: AssetRef, end: &str) -> String {
    format!("physical-end-holds/{}/{end}", network::identity(cable))
}
fn row_key(cable: AssetRef, end: &str) -> Result<String> {
    if end.is_empty()
        || end.len() > 64
        || !end
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        return Err(Error::Invalid);
    }
    Ok(format!("{}/{end}", end_parent(cable)))
}
pub(super) fn targets(step: &api::PhysicalWiring) -> Result<Vec<AssetRef>> {
    let mut targets = vec![parse_target(step.cable.as_ref())?];
    if let Some(api::physical_wiring::After::Port(port)) = &step.after {
        targets.push(parse_target(Some(port))?);
    }
    if let Some(port) = step.previous.as_ref().and_then(|b| b.port.as_ref()) {
        targets.push(parse_target(Some(port))?);
    }
    if let Some(port) = &step.previous_intended_port {
        targets.push(parse_target(Some(port))?);
    }
    Ok(targets)
}
pub(super) fn step_row(value: &api::PhysicalWiring) -> Result<Properties> {
    let cable = parse_target(value.cable.as_ref())?;
    row_key(cable, &value.end_key)?;
    let mut row = props([
        ("action", text("wiring")),
        ("item", text(network::identity(cable))),
        ("basis", text(&value.end_key)),
        ("vint", int(value.intent as u64)?),
    ]);
    match value.basis.as_ref().ok_or(Error::Invalid)? {
        api::physical_wiring::Basis::ExpectedEndRevision(value) => {
            row.insert("expected".into(), int(*value)?);
        }
        api::physical_wiring::Basis::AfterStep(value) => {
            row.insert("generation".into(), int(u64::from(*value))?);
        }
    }
    if let Some(revision) = value.result_revision {
        row.insert("source_binding_revision".into(), int(revision)?);
    }
    match value.after.as_ref().ok_or(Error::Invalid)? {
        api::physical_wiring::After::Port(port) => {
            row.insert("meaning".into(), text("port"));
            row.insert(
                "enclosure".into(),
                text(network::identity(parse_target(Some(port))?)),
            );
        }
        api::physical_wiring::After::Loose(_) => {
            row.insert("meaning".into(), text("loose"));
        }
        api::physical_wiring::After::Unknown(_) => {
            row.insert("meaning".into(), text("unknown"));
        }
    }
    if let Some(binding) = &value.previous {
        row.insert("binding".into(), text(&binding.binding_id));
        row.insert("epoch".into(), int(binding.binding_revision)?);
        row.insert(
            "old_item".into(),
            text(network::identity(parse_target(binding.port.as_ref())?)),
        );
    }
    if let Some(port) = &value.previous_intended_port {
        let port = parse_target(Some(port))?;
        row.insert("to_asset".into(), text(port.asset.to_string()));
        row.insert("to_incarnation".into(), text(port.incarnation.to_string()));
    }
    Ok(row)
}
pub(super) fn read_step(row: &Properties) -> Result<api::PhysicalWiring> {
    let after = match string(row, "meaning")?.as_str() {
        "port" => api::physical_wiring::After::Port(target_wire(lifecycle::reference(&string(
            row,
            "enclosure",
        )?)?)),
        "loose" => api::physical_wiring::After::Loose(api::Empty {}),
        "unknown" => api::physical_wiring::After::Unknown(api::Empty {}),
        _ => return Err(Error::Corrupt),
    };
    let basis = if row.contains_key("generation") {
        api::physical_wiring::Basis::AfterStep(
            u32::try_from(number(row, "generation")?).map_err(|_| Error::Corrupt)?,
        )
    } else {
        api::physical_wiring::Basis::ExpectedEndRevision(number(row, "expected")?)
    };
    let intent = i32::try_from(number(row, "vint")?).map_err(|_| Error::Corrupt)?;
    api::PhysicalWiringIntent::try_from(intent).map_err(|_| Error::Corrupt)?;
    Ok(api::PhysicalWiring {
        cable: Some(target_wire(lifecycle::reference(&string(row, "item")?)?)),
        end_key: string(row, "basis")?,
        basis: Some(basis),
        intent,
        after: Some(after),
        previous: if row.contains_key("binding") {
            Some(api::PhysicalCableAttachment {
                port: Some(target_wire(lifecycle::reference(&string(
                    row, "old_item",
                )?)?)),
                binding_id: string(row, "binding")?,
                binding_revision: number(row, "epoch")?,
            })
        } else {
            None
        },
        previous_intended_port: cables::intended(row)?.map(target_wire),
        result_revision: row
            .get("source_binding_revision")
            .map(|_| number(row, "source_binding_revision"))
            .transpose()?,
    })
}
fn wiring(step: &api::PhysicalStep) -> Option<&api::PhysicalWiring> {
    match &step.action {
        Some(api::physical_step::Action::Wiring(value)) => Some(value),
        _ => None,
    }
}
fn vacates(step: &api::PhysicalWiring, port: &wire::AssetRef) -> bool {
    match &step.after {
        Some(api::physical_wiring::After::Loose(_)) => true,
        Some(api::physical_wiring::After::Port(next)) => next != port,
        _ => false,
    }
}
fn check_planned_sockets(plan: &api::PhysicalPlan, graph: &flow::Analysis) -> Result<()> {
    // At most 64 steps. Compare conflicting claims and prove an intervening release in the shared DAG.
    for (index, step) in plan.steps.iter().enumerate() {
        let Some(current) = wiring(step) else {
            continue;
        };
        let Some(api::physical_wiring::After::Port(port)) = &current.after else {
            continue;
        };
        for (other, step) in plan.steps[..index].iter().enumerate() {
            let Some(prior) = wiring(step) else {
                continue;
            };
            if prior.after != current.after {
                continue;
            }
            let (before, after, first, second) = if graph.ancestors[index] & (1u64 << other) != 0 {
                (other, index, prior, current)
            } else if graph.ancestors[other] & (1u64 << index) != 0 {
                (index, other, current, prior)
            } else {
                return Err(Error::Conflict);
            };
            if first.cable == second.cable && first.end_key == second.end_key {
                continue;
            }
            let mut released = false;
            for (middle, step) in plan.steps.iter().enumerate() {
                if graph.ancestors[after] & (1u64 << middle) == 0
                    || graph.ancestors[middle] & (1u64 << before) == 0
                {
                    continue;
                }
                let Some(step) = wiring(step) else {
                    continue;
                };
                if step.cable == first.cable && step.end_key == first.end_key && vacates(step, port)
                {
                    released = true;
                    break;
                }
            }
            if !released {
                return Err(Error::Conflict);
            }
        }
    }
    Ok(())
}
impl Tx<'_> {
    pub(in crate::disk) fn cable_binding_at(
        &self,
        port: AssetRef,
    ) -> Result<Option<wire::Relation>> {
        let mut bindings = Vec::with_capacity(2);
        for pattern in [
            "(p:OmRow {key:$port})-[:OmRelationFrom]->(r:OmRow)",
            "(p:OmRow {key:$port})<-[:OmRelationTo]-(r:OmRow)",
        ] {
            let query = format!(
                "MATCH {pattern} WHERE r.parent='relations' AND r.kind=$kind RETURN properties(r) AS p LIMIT 2"
            );
            let rows = self.query(
                &query,
                params([
                    ("port", text(asset_key(port))),
                    ("kind", int(wire::RelationKind::ConnectedTo as u64)?),
                ]),
            )?;
            for row in rows {
                if !bindings.is_empty() {
                    return Err(Error::Conflict);
                }
                bindings.push(inventory::relation_projection(&row)?);
            }
        }
        Ok(bindings.pop())
    }
    pub(super) fn assess_wiring(
        &self,
        context: &Context,
        plan: &api::PhysicalPlan,
        working: &api::WorkingContext,
        graph: &flow::Analysis,
        claims: &mut admission::Claims,
    ) -> core::result::Result<(), admission::Failure> {
        check_planned_sockets(plan, graph).map_err(|error| {
            if matches!(error, Error::Conflict) {
                admission::Failure::mismatch()
            } else {
                error.into()
            }
        })?;
        let mut initial = BTreeSet::new();
        for (index, step) in plan.steps.iter().enumerate() {
            let Some(api::physical_step::Action::Wiring(wire)) = &step.action else {
                continue;
            };
            self.wiring_access(context, wire, working, true)?;
            let cable = parse_target(wire.cable.as_ref())?;
            let key = row_key(cable, &wire.end_key)?;
            let row = self.get(&key)?.ok_or(Error::Missing)?;
            if let Some(api::physical_wiring::Basis::ExpectedEndRevision(revision)) = wire.basis {
                if !initial.insert(key.clone()) {
                    return Err(Error::Invalid.into());
                }
                if number(&row, "revision")? != revision {
                    return Err(admission::Failure::basis(Error::Conflict));
                }
            }
            let end = hold_key(cable, &wire.end_key);
            if let Some(held) = self.get(&end)?
                && Some(string(&held, "owner")?.as_str()) != plan.id.as_deref()
            {
                return Err(admission::Failure::mismatch());
            }
            claims.ends.insert(end);
            claims.parts.insert(cable);
            if let Some(previous) = &wire.previous {
                claims.parts.insert(parse_target(previous.port.as_ref())?);
            }
            match wire.after.as_ref().ok_or(Error::Invalid)? {
                api::physical_wiring::After::Unknown(_) => return Err(Error::Incomplete.into()),
                api::physical_wiring::After::Loose(_) => {}
                api::physical_wiring::After::Port(target) => {
                    let port = parse_target(Some(target))?;
                    if let Some(binding) = self.cable_binding_at(port)? {
                        let mut permitted = wire.previous.as_ref().is_some_and(|p| {
                            p.binding_id == binding.relation_id
                                && p.binding_revision == binding.revision
                        });
                        if !permitted {
                            for (prior, step) in plan.steps.iter().enumerate() {
                                if graph.ancestors[index] & (1u64 << prior) == 0 {
                                    continue;
                                }
                                let Some(api::physical_step::Action::Wiring(candidate)) =
                                    &step.action
                                else {
                                    continue;
                                };
                                let Some(previous) = &candidate.previous else {
                                    continue;
                                };
                                if previous.binding_id != binding.relation_id
                                    || previous.binding_revision != binding.revision
                                {
                                    continue;
                                }
                                if candidate.cable == wire.cable
                                    && candidate.end_key == wire.end_key
                                {
                                    permitted = true;
                                    break;
                                }
                                match &candidate.after {
                                    Some(api::physical_wiring::After::Loose(_)) => {
                                        permitted = true;
                                        break;
                                    }
                                    Some(api::physical_wiring::After::Port(next))
                                        if previous.port.as_ref() != Some(next) =>
                                    {
                                        permitted = true;
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if !permitted {
                            return Err(admission::Failure::mismatch());
                        }
                    }
                    if !context.field(port, wire::AssetKind::Port as i32, "physical", false) {
                        return Err(Error::Denied.into());
                    }
                    let end_mating = connector::read(&row)?;
                    let port_row = self.get(&format!("physical/{}", network::identity(port)))?;
                    let port_mating = port_row
                        .as_ref()
                        .map(connector::read)
                        .transpose()?
                        .flatten();
                    let result = crate::physical::mating(
                        end_mating
                            .as_ref()
                            .map(connector::declaration)
                            .transpose()?,
                        port_mating
                            .as_ref()
                            .map(connector::declaration)
                            .transpose()?,
                    );
                    match result.state {
                        crate::requirements::State::Satisfied => {}
                        crate::requirements::State::Failed => {
                            return Err(admission::Failure::mismatch());
                        }
                        crate::requirements::State::Unknown => return Err(Error::Incomplete.into()),
                    }
                    claims.parts.insert(port);
                }
            }
        }
        for port in &claims.parts {
            if let Some(held) = self.get(&format!("physical-lock/{}", network::identity(*port)))?
                && Some(string(&held, "owner")?.as_str()) != plan.id.as_deref()
            {
                return Err(admission::Failure::mismatch());
            }
        }
        Ok(())
    }
    pub(super) fn prepare_wiring_plan(
        &self,
        context: &Context,
        plan: &mut api::PhysicalPlan,
        working: &api::WorkingContext,
    ) -> Result<()> {
        if plan.steps.is_empty() || plan.steps.len() > 64 {
            return Err(Error::Invalid);
        }
        flow::analysis(&plan.steps)?;
        for step in &mut plan.steps {
            if let Some(api::physical_step::Action::Wiring(wire)) = &mut step.action {
                self.capture_wiring(context, wire, working)?;
            }
        }
        Ok(())
    }
    pub(super) fn record_wiring(
        &mut self,
        context: &Context,
        step: &mut api::PhysicalWiring,
        expected: u64,
        owner: &str,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        self.wiring_access(context, step, working, true)?;
        let cable = parse_target(step.cable.as_ref())?;
        let key = row_key(cable, &step.end_key)?;
        let mut row = self.get(&key)?.ok_or(Error::Missing)?;
        if number(&row, "revision")? != expected {
            return Err(Error::Conflict);
        }
        let hold = hold_key(cable, &step.end_key);
        if let Some(held) = self.get(&hold)?
            && string(&held, "owner")? != owner
        {
            return Err(Error::Conflict);
        }
        let previous = if let Some(id) = row.get("binding") {
            let binding = self
                .get(&format!("relation/{}", id.as_str().ok_or(Error::Corrupt)?))?
                .ok_or(Error::Corrupt)?;
            let binding = inventory::relation_projection(&binding)?;
            if binding.revision != number(&row, "expected")? {
                return Err(Error::Conflict);
            }
            Some(binding)
        } else {
            None
        };
        step.previous = previous
            .as_ref()
            .map(|b| -> Result<api::PhysicalCableAttachment> {
                Ok(api::PhysicalCableAttachment {
                    port: Some(target_wire(cables::attached_port(b, cable)?)),
                    binding_id: b.relation_id.clone(),
                    binding_revision: b.revision,
                })
            })
            .transpose()?;
        step.previous_intended_port = cables::intended(&row)?.map(target_wire);
        self.wiring_access(context, step, working, true)?;
        for target in targets(step)? {
            self.lock_physical_part(target, owner)?;
        }
        let destination = match step.after.as_ref().ok_or(Error::Invalid)? {
            api::physical_wiring::After::Port(port) => Some(parse_target(Some(port))?),
            _ => None,
        };
        if let Some(port) = destination
            && let Some(occupied) = self.cable_binding_at(port)?
            && previous
                .as_ref()
                .is_none_or(|p| p.relation_id != occupied.relation_id)
        {
            return Err(Error::Conflict);
        }
        // Unknown attachment preserves the last possible binding and its capacity claim.
        if !matches!(step.after, Some(api::physical_wiring::After::Unknown(_))) {
            if let Some(binding) = &previous {
                self.remove_binding_relation_retaining_end(
                    binding,
                    &context.actor,
                    now,
                    Some(&key),
                )?;
            }
            row.remove("binding");
            row.remove("expected");
            if let Some(port) = destination {
                let relation = wire::Relation {
                    relation_id: RelationId::generate().to_string(),
                    from: Some(target_wire(cable)),
                    to: Some(target_wire(port)),
                    kind: wire::RelationKind::ConnectedTo as i32,
                    revision: 0,
                    authority_id: self.workspace_authority()?,
                };
                let selection = self.selection(context, working, true)?;
                self.check_relationship(context, &selection, &relation)?;
                let revision =
                    self.save_relation_retaining_end(&relation, None, &context.actor, now, &key)?;
                row.insert("binding".into(), text(&relation.relation_id));
                row.insert("expected".into(), int(revision)?);
                row.insert("state".into(), int(1)?);
            } else {
                row.insert("state".into(), int(2)?);
            }
        } else {
            row.remove("state");
        }
        match api::PhysicalWiringIntent::try_from(step.intent).map_err(|_| Error::Invalid)? {
            api::PhysicalWiringIntent::Preserve => {}
            api::PhysicalWiringIntent::ClearDestination => {
                row.remove("to_asset");
                row.remove("to_incarnation");
            }
            api::PhysicalWiringIntent::AdoptDestination => {
                let port = destination.ok_or(Error::Invalid)?;
                row.insert("to_asset".into(), text(port.asset.to_string()));
                row.insert("to_incarnation".into(), text(port.incarnation.to_string()));
            }
            _ => return Err(Error::Invalid),
        }
        let revision = expected.checked_add(1).ok_or(Error::Limit)?;
        row.insert("revision".into(), int(revision)?);
        row.insert("actor".into(), text(&context.actor));
        operations::strip_envelope(&mut row);
        self.put(&key, &end_parent(cable), Class::Inventory, row)?;
        step.result_revision = Some(revision);
        self.audit(&key, &context.actor, "cable-end.reported", revision, now)?;
        Ok(())
    }
    pub(super) fn wiring_access(
        &self,
        context: &Context,
        step: &api::PhysicalWiring,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<()> {
        let cable = parse_target(step.cable.as_ref())?;
        if self.physical_access(context, cable, working, write)? != wire::AssetKind::Cable {
            return Err(Error::Invalid);
        }
        row_key(cable, &step.end_key)?;
        let selection = self.selection(context, working, write)?;
        for target in targets(step)? {
            selection.contains(target, self)?;
            let row = self.visible_asset(context, target)?;
            let kind = number(&row, "kind")? as i32;
            if target != cable && kind != wire::AssetKind::Port as i32 {
                return Err(Error::Invalid);
            }
            if !context.field(target, kind, "relations", false)
                || (write && !context.field(target, kind, "relations", true))
            {
                return Err(Error::Denied);
            }
        }
        Ok(())
    }
    pub(super) fn capture_wiring(
        &self,
        context: &Context,
        step: &mut api::PhysicalWiring,
        working: &api::WorkingContext,
    ) -> Result<()> {
        step.previous = None;
        step.previous_intended_port = None;
        step.result_revision = None;
        self.wiring_access(context, step, working, true)?;
        if step.after.is_none() {
            return Err(Error::Invalid);
        }
        let cable = parse_target(step.cable.as_ref())?;
        let row = self
            .get(&row_key(cable, &step.end_key)?)?
            .ok_or(Error::Missing)?;
        match step.basis.as_ref().ok_or(Error::Invalid)? {
            api::physical_wiring::Basis::ExpectedEndRevision(revision)
                if *revision != 0 && *revision == number(&row, "revision")? => {}
            api::physical_wiring::Basis::AfterStep(_) => {}
            _ => return Err(Error::Conflict),
        }
        if matches!(
            step.basis,
            Some(api::physical_wiring::Basis::ExpectedEndRevision(_))
        ) && let Some(id) = row.get("binding")
        {
            let binding = self
                .get(&format!("relation/{}", id.as_str().ok_or(Error::Corrupt)?))?
                .ok_or(Error::Corrupt)?;
            let binding = inventory::relation_projection(&binding)?;
            if binding.revision != number(&row, "expected")? {
                return Err(Error::Conflict);
            }
            step.previous = Some(api::PhysicalCableAttachment {
                port: Some(target_wire(cables::attached_port(&binding, cable)?)),
                binding_id: binding.relation_id,
                binding_revision: binding.revision,
            });
        }
        if matches!(
            step.basis,
            Some(api::physical_wiring::Basis::ExpectedEndRevision(_))
        ) {
            step.previous_intended_port = cables::intended(&row)?.map(target_wire);
        }
        self.wiring_access(context, step, working, true)?;
        match api::PhysicalWiringIntent::try_from(step.intent).map_err(|_| Error::Invalid)? {
            api::PhysicalWiringIntent::Preserve | api::PhysicalWiringIntent::ClearDestination => {}
            api::PhysicalWiringIntent::AdoptDestination
                if matches!(step.after, Some(api::physical_wiring::After::Port(_))) => {}
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }
}
