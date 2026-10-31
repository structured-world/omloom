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
        if string(&row,"action")? != "physical" { return Err(Error::Invalid); }
        if string(&row, "actor")? != context.actor {
            return Err(Error::Denied);
        }
        if request.revision != 0 && number(&row, "revision")? != request.revision {
            return Err(Error::Conflict);
        }
        let state = i32::try_from(number(&row,"state")?).map_err(|_| Error::Corrupt)?;
        let terminal = matches!(api::PhysicalPlanState::try_from(state),Ok(api::PhysicalPlanState::Completed|api::PhysicalPlanState::Cancelled));
        let count = number(&row, "count")?;
        let replacement_count=number(&row,"replacement_count")?;
        if count > 64 || replacement_count>64 || count+replacement_count>64 {
            return Err(Error::Corrupt);
        }
        let mut placements = Vec::with_capacity(count as usize);
        for i in 0..count {
            let p = self.get(&format!("{key}/{i}"))?.ok_or(if terminal { Error::Expired } else { Error::Corrupt })?;
            let item = reference(&string(&p, "item")?)?;
            let enclosure = reference(&string(&p, "enclosure")?)?;
            self.physical_access(context, item, working, write)?;
            self.physical_access(context, enclosure, working, write)?;
            placements.push(api::PhysicalPlacement {
                item: Some(target_wire(item)),
                enclosure: Some(target_wire(enclosure)),
                volume: read_volume(&p)?,
            });
        }
        let mut replacements=Vec::with_capacity(replacement_count as usize);
        for i in 0..replacement_count {
            let row=self.get(&format!("{key}/replacement/{i}"))?.ok_or(if terminal {Error::Expired}else{Error::Corrupt})?;
            let replacement=super::replacements::read_replacement(&row)?;
            self.replacement_access(context,&replacement,working,write)?;
            replacements.push(replacement);
        }
        Ok(api::PhysicalPlan {
            id: Some(request.id.clone()),
            name: string(&row, "name")?,
            placements,
            replacements,
            state,
            revision: number(&row, "revision")?,
            reserved_until_ms: if row.contains_key("until") {
                Some(number(&row, "until")?)
            } else {
                None
            },
        })
    }
    fn physical_state(
        &mut self,
        context: &Context,
        plan: &mut api::PhysicalPlan,
        state: api::PhysicalPlanState,
        now: u64,
    ) -> Result<()> {
        let id = plan.id.as_ref().ok_or(Error::Corrupt)?;
        let key = format!("plan/{id}");
        let mut row = self.get(&key)?.ok_or(Error::Corrupt)?;
        let previous_state = plan.state;
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
        let terminal_detail = state == api::PhysicalPlanState::Completed
            || (state == api::PhysicalPlanState::Cancelled && previous_state == api::PhysicalPlanState::Draft as i32);
        let class = if terminal_detail { Class::Detail } else { Class::Inventory };
        if terminal_detail {
            row.insert("at".into(),int(now)?);
            for index in 0..plan.placements.len() {
                let child_key = format!("{key}/{index}");
                let mut child = self.get(&child_key)?.ok_or(Error::Corrupt)?;
                operations::strip_envelope(&mut child);
                child.insert("at".into(),int(now)?);
                self.put(&child_key,&key,Class::Detail,child)?;
            }
            for index in 0..plan.replacements.len() {
                let child_key=format!("{key}/replacement/{index}");
                let mut child=self.get(&child_key)?.ok_or(Error::Corrupt)?;
                operations::strip_envelope(&mut child);
                child.insert("at".into(),int(now)?);
                self.put(&child_key,&key,Class::Detail,child)?;
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
        if key.revision == 0
            || until <= now
            || until.checked_sub(now).ok_or(Error::Invalid)? > 86_400_000
        {
            return Err(Error::Invalid);
        }
        let mut plan = self.load_physical_plan(context, key, working, true)?;
        if plan.state != api::PhysicalPlanState::Draft as i32 {
            return Err(Error::Conflict);
        }
        let mut enclosures = BTreeMap::<String, Vec<Volume>>::new();
        let mut rows_read = 0;
        for p in &plan.placements {
            let item = parse_target(p.item.as_ref())?;
            let enclosure = parse_target(p.enclosure.as_ref())?;
            let row = self.visible_asset(context, enclosure)?;
            if !context.field(
                enclosure,
                number(&row, "kind")? as i32,
                "physical_availability",
                false,
            ) {
                return Err(Error::Denied);
            }
            self.require_no_pending_operations(item)?;
            if self
                .get(&format!("allocation/{}", network::identity(item)))?
                .is_some()
            {
                return Err(Error::Conflict);
            }
            let enclosure_key = network::identity(enclosure);
            if !enclosures.contains_key(&enclosure_key) {
                let parent = format!("allocation/{enclosure_key}");
                let rows = self.query(
                    "MATCH (n:OmRow {parent:$parent}) RETURN properties(n) AS p LIMIT 2049",
                    params([("parent", text(parent))]),
                )?;
                rows_read += rows.len();
                if rows_read > 2048 {
                    return Err(Error::Limit);
                }
                let mut occupied = Vec::with_capacity(rows.len());
                for row in rows {
                    // Installed and unresolved allocations are never freed by a clock alone.
                    if let Some(v) = read_volume(&row)? {
                        occupied.push(volume(&v)?);
                    } else {
                        return Err(Error::Corrupt);
                    }
                }
                enclosures.insert(enclosure_key.clone(), occupied);
            }
            let definition = self
                .physical_definition(enclosure)?
                .ok_or(Error::Incomplete)?;
            let item_definition = self.physical_definition(item)?.ok_or(Error::Incomplete)?;
            let item_volume = item_definition.volume.as_ref().ok_or(Error::Incomplete)?;
            let requested = p.volume.as_ref().ok_or(Error::Invalid)?;
            for (actual, proposed) in [
                (&item_volume.x, &requested.x),
                (&item_volume.y, &requested.y),
                (&item_volume.z, &requested.z),
            ] {
                if actual.as_ref().ok_or(Error::Incomplete)?.length_um
                    != proposed.as_ref().ok_or(Error::Invalid)?.length_um
                {
                    return Err(Error::Conflict);
                }
            }
            let envelope = definition.volume.as_ref().map(volume).transpose()?;
            let proposed = volume(p.volume.as_ref().ok_or(Error::Invalid)?)?;
            let occupied = enclosures.get_mut(&enclosure_key).ok_or(Error::Corrupt)?;
            if crate::physical::placement(envelope, Some(proposed), occupied)
                != crate::requirements::Decision::PASS
            {
                return Err(Error::Conflict);
            }
            occupied.push(proposed);
        }
        // Availability permits only the result, never hidden occupants or their demands.
        for enclosure_key in enclosures.keys() {
            let enclosure = reference(enclosure_key)?;
            let definition = self.physical_definition(enclosure)?.ok_or(Error::Incomplete)?;
            let limits=[definition.max_mass_g,definition.capacity_mw,definition.cooling_capacity_mw];
            if limits.iter().all(Option::is_none) { continue; }
            let parent=format!("allocation/{enclosure_key}");
            let rows=self.query("MATCH (n:OmRow {parent:$parent}) RETURN properties(n) AS p LIMIT 2049",params([("parent",text(parent))]))?;
            if rows.len()>2048 { return Err(Error::Limit); }
            let mut demands=[Some(0_u64);3];
            let mut add=|item:AssetRef|->Result<()> {
                let item=self.physical_definition(item)?.ok_or(Error::Incomplete)?;
                for (total,value) in demands.iter_mut().zip([item.mass_g,item.demand_mw,item.cooling_mw]) {
                    *total=match (*total,value) {
                        (Some(total),Some(value))=>Some(total.checked_add(value).ok_or(Error::Limit)?),
                        _=>None,
                    };
                }
                Ok(())
            };
            for row in &rows { add(reference(&string(row,"item")?)?)?; }
            for placement in &plan.placements {
                if parse_target(placement.enclosure.as_ref())?==enclosure { add(parse_target(placement.item.as_ref())?)?; }
            }
            for (limit,demand) in limits.into_iter().zip(demands) {
                if let Some(limit)=limit {
                    let demand=demand.ok_or(Error::Incomplete)?;
                    if demand>limit { return Err(Error::Conflict); }
                }
            }
        }
        for p in &plan.placements {
            let item = parse_target(p.item.as_ref())?;
            let enclosure = parse_target(p.enclosure.as_ref())?;
            let mut row = props([
                ("item", text(network::identity(item))),
                ("enclosure", text(network::identity(enclosure))),
                ("owner", text(&key.id)),
                ("state", int(api::PhysicalPlanState::Reserved as u64)?),
                ("until", int(until)?),
            ]);
            write_volume(&mut row, p.volume.as_ref().ok_or(Error::Invalid)?)?;
            self.put(
                &format!("allocation/{}", network::identity(item)),
                &format!("allocation/{}", network::identity(enclosure)),
                Class::Pending,
                row,
            )?;
        }
        plan.reserved_until_ms = Some(until);
        self.physical_state(context, &mut plan, api::PhysicalPlanState::Reserved, now)?;
        Ok(plan)
    }
    pub(super) fn complete_physical(
        &mut self,
        context: &Context,
        value: api::PhysicalCompletion,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalPlan> {
        let key = value.plan.ok_or(Error::Invalid)?;
        let mut plan = self.load_physical_plan(context, &key, working, true)?;
        if key.revision == 0
            || !matches!(
                api::PhysicalPlanState::try_from(plan.state),
                Ok(api::PhysicalPlanState::Reserved | api::PhysicalPlanState::Partial)
            )
            || value.installed.len() > 64
            || value.discrepancy.as_ref().is_some_and(|s| s.len() > 2048)
        {
            return Err(Error::Invalid);
        }
        let mut seen = BTreeSet::new();
        for value in &value.installed {
            let item = parse_target(Some(value))?;
            if !seen.insert(item) {
                return Err(Error::Invalid);
            }
            let p = plan
                .placements
                .iter()
                .find(|p| p.item.as_ref() == Some(value))
                .ok_or(Error::Invalid)?;
            let enclosure = parse_target(p.enclosure.as_ref())?;
            let allocation_key = format!("allocation/{}", network::identity(item));
            let mut row = self.get(&allocation_key)?.ok_or(Error::Conflict)?;
            if string(&row, "owner")? != key.id {
                return Err(Error::Conflict);
            }
            if number(&row, "state")? != api::PhysicalPlanState::Completed as u64 {
                let relation = wire::Relation {
                    relation_id: RelationId::generate().to_string(),
                    from: Some(target_wire(item)),
                    to: Some(target_wire(enclosure)),
                    kind: wire::RelationKind::InstalledIn as i32,
                    revision: 0,
                    authority_id: self.workspace_authority()?,
                };
                let selection = self.selection(context, working, true)?;
                self.check_relationship(context, &selection, &relation)?;
                self.save_typed_relation(&relation, None, &context.actor, now)?;
                row.insert("claim".into(), text(relation.relation_id));
            }
            operations::strip_envelope(&mut row);
            row.remove("until");
            row.insert("actor".into(), text(&context.actor));
            row.insert(
                "state".into(),
                int(api::PhysicalPlanState::Completed as u64)?,
            );
            row.insert(
                "revision".into(),
                int(number(&self.root, "revision")?
                    .checked_add(1)
                    .ok_or(Error::Limit)?)?,
            );
            self.put(
                &allocation_key,
                &format!("allocation/{}", network::identity(enclosure)),
                Class::Inventory,
                row,
            )?;
        }
        let mut done = true;
        for p in &plan.placements {
            let item = parse_target(p.item.as_ref())?;
            let row = self
                .get(&format!("allocation/{}", network::identity(item)))?
                .ok_or(Error::Corrupt)?;
            if number(&row, "state")? != api::PhysicalPlanState::Completed as u64 {
                done = false;
            }
        }
        if let Some(discrepancy) = value.discrepancy {
            let mut row = self
                .get(&format!("plan/{}", key.id))?
                .ok_or(Error::Corrupt)?;
            operations::strip_envelope(&mut row);
            row.insert("description".into(), text(discrepancy));
            self.put(&format!("plan/{}", key.id), "plans", Class::Inventory, row)?;
        }
        self.physical_state(
            context,
            &mut plan,
            if done {
                api::PhysicalPlanState::Completed
            } else {
                api::PhysicalPlanState::Partial
            },
            now,
        )?;
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
        // A cancellation cannot assert that reserved real-world work did not happen.
        if plan.state != api::PhysicalPlanState::Draft as i32 {
            return Err(Error::UnsafeTransition);
        }
        self.physical_state(context, &mut plan, api::PhysicalPlanState::Cancelled, now)?;
        Ok(plan)
    }
}
