use super::*;

pub(super) fn replacement_row(value:&api::PhysicalReplacement)->Result<Properties> {
    let mut row=props([("enclosure",text(network::identity(parse_target(value.position.as_ref())?))),("state",int(if value.completed {api::PhysicalPlanState::Completed as u64}else{api::PhysicalPlanState::Draft as u64})?)]);
    if let Some(item)=&value.previous {row.insert("old_item".into(),text(network::identity(parse_target(Some(item))?)));}
    if let Some(item)=&value.replacement {row.insert("item".into(),text(network::identity(parse_target(Some(item))?)));}
    if let Some(position)=&value.source_position {row.insert("source".into(),text(network::identity(parse_target(Some(position))?)));}
    if let Some(binding)=&value.previous_binding {row.insert("claim".into(),text(binding));}
    if let Some(revision)=value.previous_revision {row.insert("expected".into(),int(revision)?);}
    Ok(row)
}
pub(super) fn read_replacement(row:&Properties)->Result<api::PhysicalReplacement> {
    let reference=|name:&str|->Result<Option<wire::AssetRef>>{row.get(name).map(|_|string(row,name).and_then(|s|lifecycle::reference(&s)).map(target_wire)).transpose()};
    Ok(api::PhysicalReplacement {
        position:Some(target_wire(lifecycle::reference(&string(row,"enclosure")?)?)),
        previous:reference("old_item")?,replacement:reference("item")?,source_position:reference("source")?,
        previous_binding:row.get("claim").map(|_|string(row,"claim")).transpose()?,
        previous_revision:row.get("expected").map(|_|number(row,"expected")).transpose()?,
        completed:number(row,"state")?==api::PhysicalPlanState::Completed as u64,
    })
}
impl Tx<'_> {
    pub(super) fn replacement_access(&self,context:&Context,value:&api::PhysicalReplacement,working:&api::WorkingContext,write:bool)->Result<()> {
        for reference in [value.position.as_ref(),value.source_position.as_ref(),value.previous.as_ref(),value.replacement.as_ref()].into_iter().flatten() {
            let target=parse_target(Some(reference))?;
            self.physical_access(context,target,working,write)?;
            let kind=i32::try_from(number(&self.asset(target)?,"kind")?).map_err(|_|Error::Corrupt)?;
            if !context.field(target,kind,"relations",false) || (write && !context.field(target,kind,"relations",true)) {return Err(Error::Denied);}
        }
        Ok(())
    }
    pub(super) fn check_replacements(&self,context:&Context,plan:&api::PhysicalPlan,working:&api::WorkingContext,write:bool)->Result<()> {
        let mut positions=BTreeSet::new();let mut parts=BTreeSet::new();
        for replacement in &plan.replacements {
            self.replacement_access(context,replacement,working,write)?;
            let position=parse_target(replacement.position.as_ref())?;
            if !positions.insert(position) || replacement.completed || (replacement.previous.is_none() && replacement.replacement.is_none()) {return Err(Error::Invalid);}
            for item in [replacement.previous.as_ref(),replacement.replacement.as_ref()].into_iter().flatten() {
                let item=parse_target(Some(item))?;
                let kind=wire::AssetKind::try_from(number(&self.asset(item)?,"kind")? as i32).map_err(|_|Error::Corrupt)?;
                if item==position || !matches!(kind,wire::AssetKind::PhysicalComponent|wire::AssetKind::Device) {return Err(Error::Invalid);}
            }
            if let Some(item)=&replacement.replacement {
                if !parts.insert(parse_target(Some(item))?) {return Err(Error::Invalid);}
            }
            let position_kind=wire::AssetKind::try_from(number(&self.asset(position)?,"kind")? as i32).map_err(|_|Error::Corrupt)?;
            if !matches!(position_kind,wire::AssetKind::PhysicalPosition|wire::AssetKind::Rack|wire::AssetKind::Location|wire::AssetKind::Site) {return Err(Error::Invalid);}
            match (&replacement.previous_binding,replacement.previous_revision) {
                (Some(id),Some(revision)) if replacement.previous.is_some() && revision!=0 => {RelationId::parse(id).map_err(|_|Error::Invalid)?;},
                (None,None)=>{},
                _=>return Err(Error::Invalid),
            }
            if replacement.source_position.is_some() && replacement.previous.is_none() {return Err(Error::Invalid);}
        }
        Ok(())
    }
    pub(super) fn replacement_basis(&self,replacement:&api::PhysicalReplacement)->Result<Option<wire::Relation>> {
        let Some(previous)=replacement.previous.as_ref() else {return Ok(None);};
        let id=RelationId::parse(replacement.previous_binding.as_ref().ok_or(Error::Incomplete)?).map_err(|_|Error::Invalid)?;
        let row=self.get(&format!("relation/{id}"))?.ok_or(Error::Conflict)?;
        let relation=inventory::relation_projection(&row)?;
        let source=replacement.source_position.as_ref().or(replacement.position.as_ref());
        if relation.from.as_ref()!=Some(previous) || relation.to.as_ref()!=source || Some(relation.revision)!=replacement.previous_revision || relation.kind!=wire::RelationKind::InstalledIn as i32 {return Err(Error::Conflict);}
        Ok(Some(relation))
    }
}
