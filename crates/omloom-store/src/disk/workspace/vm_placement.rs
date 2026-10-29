//! Placement review reuses conditional relationship mutation. No move preview,
//! address history or second authoritative inventory is persisted.
use super::*;

impl Store {
    pub(super) fn workspace_prepare_vm_placement(
        &self,
        context: &Context,
        request: api::PrepareVmPlacement,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        let vm = parse_target(request.vm.as_ref())?;
        let tx = self.begin()?;
        let selection = tx.selection(context, working, false)?;
        selection.contains(vm, &tx)?;
        let item = tx.placement_item(context, vm)?;
        if item.kind != wire::AssetKind::VirtualMachine as i32 {
            return Err(Error::Invalid);
        }
        if !context.field(vm, item.kind, "relations", false)
            || !context.field(vm, item.kind, "relations", true)
        {
            return Err(Error::Denied);
        }
        // Do not filter hidden placements into a false "unplaced" answer.
        let rows = tx.query(
            "MATCH (r:OmRow {asset:$asset}) WHERE r.parent='relations' AND r.incarnation=$inc AND r.kind=$kind RETURN properties(r) AS p LIMIT 2",
            params([
                ("asset", text(vm.asset.to_string())),
                ("inc", text(vm.incarnation.to_string())),
                ("kind", int(wire::RelationKind::RunsOn as u64)?),
            ]),
        )?;
        if rows.len() > 1 {
            return Err(Error::Conflict);
        }
        let mut review = api::VmPlacementReview {
            revision: number(&tx.root, "revision")?,
            vm: Some(item),
            ..Default::default()
        };
        if let Some(row) = rows.first() {
            let relation = inventory::relation_projection(row)?;
            let (_, old_host) = tx.check_relationship(context, &selection, &relation)?;
            let host = tx.placement_item(context, old_host)?;
            if !context.field(old_host, host.kind, "relations", false) {
                return Err(Error::Denied);
            }
            review.current_host = Some(host);
            review.current = Some(relation);
        }
        if let Some(destination) = request.destination.as_ref() {
            let destination = parse_target(Some(destination))?;
            let host = tx.placement_item(context, destination)?;
            if host.kind != wire::AssetKind::SoftwareInstallation as i32 {
                return Err(Error::Invalid);
            }
            if !context.field(destination, host.kind, "virtualization_role", false) {
                return Err(Error::Denied);
            }
            let row = tx.asset(destination)?;
            if row.get("virtualization_role").and_then(Value::as_str)
                != Some(api::VirtualizationRole::Hypervisor.as_str_name())
            {
                return Err(Error::Invalid);
            }
            let proposed = wire::Relation {
                from: Some(target_wire(vm)),
                to: Some(target_wire(destination)),
                kind: wire::RelationKind::RunsOn as i32,
                ..Default::default()
            };
            tx.check_relationship(context, &selection, &proposed)?;
            let unchanged = review.current.as_ref().is_some_and(|r| r.to == proposed.to);
            if !unchanged {
                tx.require_no_pending_operations(vm)?;
                let key = review
                    .current
                    .as_ref()
                    .map(|r| format!("relation/{}", r.relation_id));
                tx.check_placement_cycle(vm, destination, key.as_deref())?;
                if let Some(current) = review.current.as_ref() {
                    review.invalidated_inheritance =
                        tx.placement_inheritance(context, vm, current)?;
                }
            }
            review.destination = Some(host);
        }
        let hosts: BTreeSet<_> = review
            .current_host
            .iter()
            .chain(review.destination.iter())
            .map(|item| parse_target(item.target.as_ref()))
            .collect::<Result<_>>()?;
        let rows = hosts
            .into_iter()
            .map(|host| tx.asset(host))
            .collect::<Result<Vec<_>>>()?;
        review.contexts = tx.item_contexts(
            context,
            &rows,
            &api::Query {
                roots: working.roots.clone(),
                ..Default::default()
            },
        )?;
        tx.commit()?;
        Ok(reply(api::reply::Result::VmPlacementReview(Box::new(
            review,
        ))))
    }
}

impl Tx<'_> {
    fn placement_item(&self, context: &Context, target: AssetRef) -> Result<api::Item> {
        let row = self.visible_asset(context, target)?;
        let kind = number(&row, "kind")? as i32;
        let mut fields = Vec::new();
        if context.field(target, kind, "name", false)
            && let Some(field) = self.get(&format!("{}/intent/name", asset_key(target)))?
        {
            fields.push(inventory::read_field(&field)?);
        }
        Ok(api::Item {
            target: Some(target_wire(target)),
            kind,
            revision: number(&row, "revision")?,
            fields,
        })
    }
}
