use super::*;

pub(super) fn replacement_row(value:&api::PhysicalReplacement)->Result<Properties> {
    let mut row=props([("enclosure",text(network::identity(parse_target(value.position.as_ref())?)))]);
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
    })
}
impl Tx<'_> {
    pub(super) fn record_physical_replacements(&mut self,context:&Context,replacements:&[&api::PhysicalReplacement],plan:&str,working:&api::WorkingContext,now:u64)->Result<()> {
        let selection=self.selection(context,working,true)?;
        let mut old=Vec::with_capacity(replacements.len());let mut removed=BTreeSet::new();
        for replacement in replacements {
            self.replacement_access(context,replacement,working,true)?;
            for part in [replacement.previous.as_ref(),replacement.replacement.as_ref()].into_iter().flatten() {
                let part=parse_target(Some(part))?;self.require_no_pending_operations(part)?;
                if let Some(lock)=self.get(&format!("physical-lock/{}",network::identity(part)))? && string(&lock,"owner")?!=plan {return Err(Error::Conflict);}
            }
            if let Some(binding)=self.replacement_basis(replacement)? {
                self.check_relationship(context,&selection,&binding)?;
                if !removed.insert(binding.relation_id.clone()) {return Err(Error::Invalid);}
                old.push(binding);
            }
        }
        let mut incoming_positions=BTreeSet::new();
        for replacement in replacements {
            let position=parse_target(replacement.position.as_ref())?;
            if replacement.replacement.is_some() {incoming_positions.insert(position);}
            if number(&self.asset(position)?,"kind")?==wire::AssetKind::PhysicalPosition as u64 {
                let rows=self.query("MATCH (p:OmRow {key:$key})<-[:OmRelationTo]-(r:OmRow) WHERE r.kind=$kind RETURN properties(r) AS p LIMIT 2",params([("key",text(asset_key(position))),("kind",int(wire::RelationKind::InstalledIn as u64)?)]))?;
                if rows.len()>1 {return Err(Error::Conflict);}
                for row in rows {
                    let binding=inventory::relation_projection(&row)?;
                    if !removed.contains(&binding.relation_id) && binding.from!=replacement.replacement {return Err(Error::Conflict);}
                }
            }
            if let Some(incoming)=&replacement.replacement {
                for binding in self.installed_bindings(parse_target(Some(incoming))?)? {
                    if !removed.contains(&binding.relation_id) && binding.to!=replacement.position {return Err(Error::Conflict);}
                }
            }
        }
        // Detach every selected old binding before attaching incoming parts, including swaps.
        for binding in &old {
            self.remove_binding_relation(binding,&context.actor,now)?;
            let item=parse_target(binding.from.as_ref())?;
            let allocation=format!("allocation/{}",network::identity(item));
            if let Some(row)=self.get(&allocation)? {
                if string(&row,"claim")?!=binding.relation_id || number(&row,"state")?!=api::PhysicalPlanState::Completed as u64 {return Err(Error::Conflict);}
                self.delete(&allocation)?;
            }
        }
        for replacement in replacements {
            let position=parse_target(replacement.position.as_ref())?;
            let Some(incoming)=&replacement.replacement else {continue;};
            let item=parse_target(Some(incoming))?;
            let existing=self.installed_bindings(item)?;
            let claim=if let Some(binding)=existing.first() {binding.relation_id.clone()}else{
                let binding=wire::Relation {relation_id:RelationId::generate().to_string(),from:Some(target_wire(item)),to:Some(target_wire(position)),kind:wire::RelationKind::InstalledIn as i32,revision:1,authority_id:self.workspace_authority()?};
                self.check_relationship(context,&selection,&binding)?;
                self.save_typed_relation(&binding,None,&context.actor,now)?;
                binding.relation_id
            };
            let allocation=format!("allocation/{}",network::identity(item));
            if self.get(&allocation)?.is_none() {
                self.put(&allocation,&format!("allocation/{}",network::identity(position)),Class::Inventory,props([("item",text(network::identity(item))),("enclosure",text(network::identity(position))),("owner",text(plan)),("claim",text(claim)),("state",int(api::PhysicalPlanState::Completed as u64)?),("actor",text(&context.actor)),("revision",int(number(&self.root,"revision")?.checked_add(1).ok_or(Error::Limit)?)?)]))?;
            }
        }
        let mut touched=incoming_positions.clone();
        for binding in &old {touched.insert(parse_target(binding.to.as_ref())?);}
        for position in touched {
            if number(&self.asset(position)?,"kind")?!=wire::AssetKind::PhysicalPosition as u64 {continue;}
            let key=format!("position/{}",network::identity(position));let mut row=self.get(&key)?.ok_or(Error::Corrupt)?;
            operations::strip_envelope(&mut row);
            if incoming_positions.contains(&position) {row.remove("vflag");}else{row.insert("vflag".into(),Value::Bool(true));}
            row.insert("actor".into(),text(&context.actor));row.insert("revision".into(),int(number(&self.root,"revision")?.checked_add(1).ok_or(Error::Limit)?)?);
            self.put(&key,&format!("positions/{}",string(&row,"enclosure")?),Class::Inventory,row)?;
        }
        Ok(())
    }
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
        for replacement in replacement_steps(plan) {
            self.replacement_access(context,replacement,working,write)?;
            let position=parse_target(replacement.position.as_ref())?;
            if !positions.insert(position) || (replacement.previous.is_none() && replacement.replacement.is_none()) {return Err(Error::Invalid);}
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
