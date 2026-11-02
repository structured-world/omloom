//! Exact accepted binding predicates share the requirement graph and snapshot.
use super::*;

pub(super) fn validate(value: &api::BindingRequirement) -> Result<()> {
    RelationId::parse(&value.relation_id).map_err(|_| Error::Invalid)?;
    if value.revision == 0
        || wire::RelationKind::try_from(value.kind).map_err(|_| Error::Invalid)?
            == wire::RelationKind::Unspecified
    {
        return Err(Error::Invalid);
    }
    parse_target(value.from.as_ref())?;
    parse_target(value.to.as_ref())?;
    Ok(())
}
impl Tx<'_> {
    pub(super) fn requirement_binding(
        &self,
        context: &Context,
        value: &api::BindingRequirement,
        working: &api::WorkingContext,
    ) -> Result<core::Decision> {
        validate(value)?;
        let selection = self.selection(context, working, false)?;
        for endpoint in [value.from.as_ref(), value.to.as_ref()] {
            let target = parse_target(endpoint)?;
            selection.contains(target, self)?;
            let row = match self.visible_asset(context, target) {
                Ok(row) => row,
                Err(Error::Denied | Error::Missing | Error::Conflict) => {
                    return Ok(core::Decision::unknown(core::Reason::Restricted));
                }
                Err(error) => return Err(error),
            };
            if !context.field(target, number(&row, "kind")? as i32, "relations", false) {
                return Ok(core::Decision::unknown(core::Reason::Restricted));
            }
        }
        let Some(row) = self.get(&format!("relation/{}", value.relation_id))? else {
            return Ok(core::Decision::FAIL);
        };
        let actual = inventory::relation_projection(&row)?;
        if actual.from != value.from
            || actual.to != value.to
            || actual.kind != value.kind
            || actual.revision != value.revision
        {
            return Ok(core::Decision::FAIL);
        }
        Ok(core::Decision::PASS)
    }
}
