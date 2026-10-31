use super::*;

pub(super) fn reservation_key(plan:&str,index:usize)->String {format!("reservation/{plan}/{index}")}
impl Tx<'_> {
    pub(super) fn lock_physical_part(&mut self,item:AssetRef,plan:&str,until:Option<u64>)->Result<()> {
        self.require_no_pending_operations(item)?;
        let key=format!("physical-lock/{}",network::identity(item));
        if let Some(row)=self.get(&key)? {
            if string(&row,"owner")?!=plan {return Err(Error::Conflict);}
            return Ok(());
        }
        let mut row=props([("owner",text(plan))]);if let Some(until)=until {row.insert("until".into(),int(until)?);}
        self.put(&key,"physical-locks",Class::Pending,row)?;
        Ok(())
    }
    pub(super) fn clear_physical_reservation(&mut self,plan:&str,index:usize)->Result<()> {
        let key=reservation_key(plan,index);
        if let Some(row)=self.get(&key)? {
            if string(&row,"owner")?!=plan {return Err(Error::Conflict);}
            self.delete(&key)?;
        }
        Ok(())
    }
    pub(super) fn release_resolved_physical_locks(&mut self,plan:&api::PhysicalPlan)->Result<()> {
        let owner=plan.id.as_deref().ok_or(Error::Corrupt)?;
        let mut resolved=BTreeSet::new();let mut pending=BTreeSet::new();
        for step in &plan.steps {
            let terminal=matches!(api::PhysicalStepState::try_from(step.state),Ok(api::PhysicalStepState::Recorded|api::PhysicalStepState::NotPerformed));
            let keys=if terminal {&mut resolved}else{&mut pending};
            match step.action.as_ref().ok_or(Error::Corrupt)? {
                api::physical_step::Action::Placement(p)=>{keys.insert(format!("physical-lock/{}",network::identity(parse_target(p.item.as_ref())?)));},
                api::physical_step::Action::Replacement(r)=>{
                    for item in [r.previous.as_ref(),r.replacement.as_ref()].into_iter().flatten() {keys.insert(format!("physical-lock/{}",network::identity(parse_target(Some(item))?)));}
                    for item in [r.position.as_ref(),r.source_position.as_ref()].into_iter().flatten() {keys.insert(format!("physical-slot/{}",network::identity(parse_target(Some(item))?)));}
                }
            }
        }
        for key in resolved.difference(&pending) {if let Some(row)=self.get(key)? {if string(&row,"owner")?!=owner {return Err(Error::Conflict);}self.delete(key)?;}}
        Ok(())
    }
    pub(super) fn reserve_replacement_steps(&mut self,context:&Context,plan:&api::PhysicalPlan,working:&api::WorkingContext,owner:&str,until:u64)->Result<()> {
        for replacement in replacement_steps(plan) {
            self.replacement_access(context,replacement,working,true)?;
            let destination=parse_target(replacement.position.as_ref())?;
            let destination_row=self.asset(destination)?;
            if !context.field(destination,number(&destination_row,"kind")? as i32,"physical_availability",false) {return Err(Error::Denied);}
            self.replacement_basis(replacement)?;
            if replacement.previous.is_none() {
                let position=self.get(&format!("position/{}",network::identity(destination)))?.ok_or(Error::Incomplete)?;
                if position.get("vflag")!=Some(&Value::Bool(true)) {return Err(Error::Incomplete);}
            }
            if let Some(incoming)=&replacement.replacement {
                let item=self.physical_definition(parse_target(Some(incoming))?)?.ok_or(Error::Incomplete)?;
                let target=self.physical_definition(destination)?.ok_or(Error::Incomplete)?;
                if number(&destination_row,"kind")?==wire::AssetKind::PhysicalPosition as u64 {
                    let position=self.get(&format!("position/{}",network::identity(destination)))?.ok_or(Error::Corrupt)?;
                    let expected=number(&position,"vint")?;
                    if expected==api::PhysicalComponentKind::Unspecified as u64 || item.component_kind==api::PhysicalComponentKind::Unspecified as i32 {return Err(Error::Incomplete);}
                    if expected!=item.component_kind as u64 {return Err(Error::Conflict);}
                }
                if let (Some(required),Some(actual))=(&target.mounting,&item.mounting) {if required!=actual {return Err(Error::Conflict);}}else{return Err(Error::Incomplete);}
                let target_volume=target.volume.as_ref().map(volume).transpose()?.ok_or(Error::Incomplete)?;
                let item_volume=item.volume.as_ref().map(volume).transpose()?.ok_or(Error::Incomplete)?;
                let located=Volume {x:Span::new(target_volume.x.start(),u64::try_from(item_volume.x.end()-item_volume.x.start()).map_err(|_|Error::Corrupt)?).map_err(|_|Error::Invalid)?,y:Span::new(target_volume.y.start(),u64::try_from(item_volume.y.end()-item_volume.y.start()).map_err(|_|Error::Corrupt)?).map_err(|_|Error::Invalid)?,z:Span::new(target_volume.z.start(),u64::try_from(item_volume.z.end()-item_volume.z.start()).map_err(|_|Error::Corrupt)?).map_err(|_|Error::Invalid)?};
                if !target_volume.contains(located) {return Err(Error::Conflict);}
            }
            for part in [replacement.previous.as_ref(),replacement.replacement.as_ref()].into_iter().flatten() {self.lock_physical_part(parse_target(Some(part))?,owner,Some(until))?;}
            for position in [replacement.position.as_ref(),replacement.source_position.as_ref()].into_iter().flatten() {
                let position=parse_target(Some(position))?;
                if number(&self.asset(position)?,"kind")?!=wire::AssetKind::PhysicalPosition as u64 {continue;}
                let key=format!("physical-slot/{}",network::identity(position));
                if let Some(row)=self.get(&key)? {if string(&row,"owner")?!=owner {return Err(Error::Conflict);}}else{self.put(&key,"physical-slots",Class::Pending,props([("owner",text(owner)),("until",int(until)?)]))?;}
            }
        }
        Ok(())
    }
}
