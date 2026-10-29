use super::*;

impl Store {
    pub(super) fn undo_identity_correction(
        &self,
        context: &Context,
        id: &str,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
        let mut tx = self.begin()?;
        let key = format!("correction/{id}");
        let row = tx.get(&key)?.ok_or(Error::Expired)?;
        if string(&row, "owner")? != context.actor
            || number(&row, "source_binding_revision")? != context.authority_revision
        {
            return Err(Error::Denied);
        }
        if row.get("retired") == Some(&Value::Bool(true)) {
            drop(tx);
            return self.identity_correction_receipt(context, id, working);
        }
        if number(&row, "expires")? <= now {
            return Err(Error::Expired);
        }
        if number(&row, "revision")? != number(&tx.root, "revision")? {
            return Err(Error::Conflict);
        }
        let input =
            api::IdentityCorrection::decode(binary(&row, "query")?).map_err(|_| Error::Corrupt)?;
        let inverse = deltas::Capture::decode(binary(&row, "payload")?)?;
        for mapping in &input.mappings {
            let (from, to) = pair(mapping)?;
            let original = inverse
                .rows
                .get(&asset_key(from))
                .and_then(Option::as_ref)
                .ok_or(Error::Corrupt)?;
            let kind = number(original, "kind")? as i32;
            for target in [from, to] {
                if !context.readable(target, kind) {
                    return Err(Error::Denied);
                }
                for field in ["identity_correction", "relations", "remove"] {
                    if !context.field(target, kind, field, true) {
                        return Err(Error::Denied);
                    }
                }
                for choice in &mapping.fields {
                    if !context.field(target, kind, &choice.name, true)
                        || !context.field(target, kind, &choice.name, false)
                    {
                        return Err(Error::Denied);
                    }
                }
            }
            for (field, parent) in [
                ("identity_evidence", host_identity::parent(from)),
                (
                    "identity_evidence",
                    format!("{}/host-mappings", asset_key(from)),
                ),
                (
                    "current_addresses",
                    source_layout::Family::Addresses.parent(from),
                ),
                (
                    "network_evidence",
                    source_layout::Family::Network.parent(from),
                ),
            ] {
                if inverse
                    .rows
                    .values()
                    .flatten()
                    .any(|p| p.get("parent") == Some(&text(&parent)))
                {
                    for target in [from, to] {
                        if !context.field(target, kind, field, false)
                            || !context.field(target, kind, field, true)
                        {
                            return Err(Error::Denied);
                        }
                    }
                }
            }
            let fence = tx
                .get(&format!("removed/{}", from.asset))?
                .ok_or(Error::Conflict)?;
            if super::super::super::target(&fence)? != from
                || string(&fence, "to_asset")? != to.asset.to_string()
                || string(&fence, "to_incarnation")? != to.incarnation.to_string()
            {
                return Err(Error::Conflict);
            }
            tx.correction_guard(context, to, &api::WorkingContext::default())?;
        }
        for before in inverse.rows.values().flatten() {
            if string(before, "parent")?.ends_with("/sockets") {
                let bind = api::SocketBind::decode(binary(before, "payload")?)
                    .map_err(|_| Error::Corrupt)?;
                let service = parse_target(bind.service.as_ref())?;
                for write in [false, true] {
                    if !context.field(
                        service,
                        wire::AssetKind::Service as i32,
                        "network_evidence",
                        write,
                    ) {
                        return Err(Error::Denied);
                    }
                }
            }
        }
        for item in inverse
            .rows
            .keys()
            .filter_map(|key| key.strip_prefix("relation/"))
        {
            tx.unlink_correction_relation(item)?;
        }
        for (key, before) in &inverse.rows {
            if key.starts_with("relation/") {
                continue;
            }
            match before {
                None => tx.delete(key)?,
                Some(before) => {
                    let parent = string(before, "parent")?;
                    let class = match number(before, "class")? {
                        0 => Class::Inventory,
                        5 => Class::Checkpoint,
                        _ => return Err(Error::Corrupt),
                    };
                    let mut data = strip(before.clone());
                    if parent == "assets" {
                        let prior = number(before, "revision")?;
                        let revision = match tx.get(key)? {
                            Some(current) => {
                                number(&current, "revision")?.max(prior).checked_add(1)
                            }
                            None => prior.checked_add(2),
                        }
                        .ok_or(Error::Limit)?;
                        data.insert("revision".into(), int(revision)?);
                    }
                    tx.put(key, &parent, class, data)?;
                }
            }
        }
        for (key, before) in &inverse.rows {
            if !key.starts_with("relation/") {
                continue;
            }
            let Some(before) = before else {
                tx.delete(key)?;
                continue;
            };
            let original = inventory::relation_projection(before)?;
            let current = tx.get(key)?;
            let expected = if let Some(current) = current {
                number(&current, "revision")?
            } else {
                tx.put(key, "relations", Class::Inventory, strip(before.clone()))?;
                original.revision
            };
            let mut restored = original.clone();
            restored.revision = expected.checked_add(1).ok_or(Error::Limit)?;
            tx.save_rebound_relation(&restored, expected, &context.actor, now)?;
            tx.rebind_correction_responsibility(context, &original, &restored)?;
        }
        // Validate the fully restored topology, including retired roots, before
        // committing. A partial restoration is never exposed to another reader.
        let selection = tx.selection(context, working, false)?;
        for mapping in &input.mappings {
            let (from, to) = pair(mapping)?;
            for target in [from, to] {
                tx.visible_asset(context, target)?;
                selection.contains(target, &tx)?;
            }
        }
        for (key, before) in &inverse.rows {
            if key.starts_with("relation/")
                && let Some(before) = before
            {
                tx.check_relationship(
                    context,
                    &selection,
                    &inventory::relation_projection(before)?,
                )?;
            }
        }
        // Reversal has its own audit event. Drop bulky inverse custody now;
        // the small receipt is sufficient for an exact retry until cleanup.
        let receipt = api::IdentityCorrectionApplied {
            revision: number(&tx.root, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?,
            id: id.into(),
            undo_until: 0,
            retained: input
                .mappings
                .into_iter()
                .filter_map(|v| v.duplicate)
                .collect(),
        };
        tx.put(
            &key,
            "detail/identity-corrections",
            Class::Detail,
            props([
                ("owner", text(&context.actor)),
                ("at", int(now)?),
                ("expires", int(now)?),
                ("source_binding_revision", int(context.authority_revision)?),
                ("retired", Value::Bool(true)),
                ("payload", Value::Binary(receipt.encode_to_vec())),
            ]),
        )?;
        let revision = tx.commit()?;
        debug_assert_eq!(revision, receipt.revision);
        Ok(reply(api::reply::Result::IdentityCorrected(receipt)))
    }
}
