use super::*;

fn visible(view: &api::SavedView, context: &Context) -> bool {
    view.owner == context.actor || view.audiences.iter().any(|a| context.teams.contains(a))
}
fn editable(view: &api::SavedView, context: &Context) -> bool {
    view.owner == context.actor || view.editors.iter().any(|a| context.teams.contains(a))
}
fn decode(row: &Properties) -> Result<api::SavedView> {
    Ok(api::SavedView {
        id: string(row, "key")?
            .strip_prefix("view/")
            .ok_or(Error::Corrupt)?
            .into(),
        name: string(row, "label")?,
        owner: string(row, "owner")?,
        revision: number(row, "revision")?,
        query: Some(api::Query::decode(binary(row, "query")?).map_err(|_| Error::Corrupt)?),
        audiences: read_strings(row, "audiences")?,
        editors: read_strings(row, "editors")?,
    })
}
impl Store {
    pub(super) fn workspace_views(&self, context: &Context) -> Result<Vec<api::SavedView>> {
        self.workspace_rows("views", 256)?
            .iter()
            .map(decode)
            .filter(|r| match r {
                Err(_) => true,
                Ok(v) => visible(v, context),
            })
            .collect()
    }
    pub(super) fn workspace_save_view(
        &self,
        context: &Context,
        request: api::SaveView,
        now: u64,
    ) -> Result<api::Reply> {
        let mut view = request.view.ok_or(Error::Invalid)?;
        if view.name.trim().is_empty()
            || view.name.len() > 100
            || view.audiences.len() > 16
            || view.editors.len() > 16
        {
            return Err(Error::Invalid);
        }
        let query = view.query.as_mut().ok_or(Error::Invalid)?;
        query.cursor = None;
        if query.encoded_len() > 8192
            || query.predicates.len() > 8
            || query.columns.len() > MAX_FIELDS
        {
            return Err(Error::Limit);
        }
        // Definition metadata and its query validity are independent of whether
        // the current author happens to have matching assets.
        let definitions = self.workspace_definitions(context)?;
        for predicate in &query.predicates {
            super::assets::validate_predicate(predicate, &definitions)?;
        }
        for column in &query.columns {
            super::assets::validate_column(column, &definitions)?;
        }
        let mut tx = self.begin()?;
        let old = if view.id.is_empty() {
            if view.revision != 0 || (!view.owner.is_empty() && view.owner != context.actor) {
                return Err(Error::Denied);
            }
            if self.workspace_rows("views", 256)?.len() >= 256 {
                return Err(Error::Limit);
            }
            view.id = AuthorityId::generate().to_string();
            view.owner.clone_from(&context.actor);
            None
        } else {
            view.id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
            Some(
                tx.get(&format!("view/{}", view.id))?
                    .ok_or(Error::Missing)?,
            )
        };
        if let Some(row) = &old {
            let previous = decode(row)?;
            if !visible(&previous, context) || !editable(&previous, context) {
                return Err(Error::Denied);
            }
            if previous.revision != view.revision {
                return Err(Error::Conflict);
            }
            let audience_changed = previous.audiences != view.audiences
                || previous.editors != view.editors
                || previous.owner != view.owner;
            if audience_changed && previous.owner != context.actor {
                return Err(Error::Denied);
            }
            if previous.owner != view.owner && !context.recipients.contains(&view.owner) {
                return Err(Error::Denied);
            }
            if request.remove {
                if previous.owner != context.actor {
                    return Err(Error::Denied);
                }
                let key = format!("view/{}", view.id);
                tx.delete(&key)?;
                tx.audit(&key, &context.actor, "view.removed", view.revision, now)?;
                return Ok(mutation(tx.commit()?, Vec::new()));
            }
            if previous == view {
                tx.commit()?;
                return Ok(reply(api::reply::Result::Views(api::Views {
                    items: vec![view],
                })));
            }
        } else if request.remove {
            return Err(Error::Invalid);
        }
        if view.audiences.iter().any(|a| !context.teams.contains(a))
            || view.editors.iter().any(|a| !view.audiences.contains(a))
        {
            return Err(Error::Denied);
        }
        let disclosure = !view.audiences.is_empty()
            || old
                .as_ref()
                .is_some_and(|p| string(p, "owner").ok().as_ref() != Some(&view.owner));
        if disclosure && !request.publish {
            tx.commit()?;
            return Ok(reply(api::reply::Result::Views(api::Views {
                items: vec![view],
            })));
        }
        view.revision = view.revision.checked_add(1).ok_or(Error::Limit)?;
        let key = format!("view/{}", view.id);
        tx.put_against(
            &key,
            "views",
            Class::Inventory,
            props([
                ("label", text(&view.name)),
                ("owner", text(&view.owner)),
                ("revision", int(view.revision)?),
                ("audiences", strings(&view.audiences)),
                ("editors", strings(&view.editors)),
                (
                    "query",
                    Value::Binary(view.query.as_ref().ok_or(Error::Invalid)?.encode_to_vec()),
                ),
            ]),
            old.as_ref(),
        )?;
        tx.audit(&key, &context.actor, "view.saved", view.revision, now)?;
        tx.commit()?;
        Ok(reply(api::reply::Result::Views(api::Views {
            items: vec![view],
        })))
    }
}
