use super::*;

fn position_key(owner:AssetRef,structural:&str)->Result<String> {
    if structural.is_empty() || structural.len()>128 || !structural.bytes().all(|b|b.is_ascii_alphanumeric()||b"_./:-".contains(&b)) {
        return Err(Error::Invalid);
    }
    Ok(format!("position-key/{}/{structural}",network::identity(owner)))
}
fn position(row:&Properties,owner:AssetRef)->Result<api::PhysicalPosition> {
    let kind=i32::try_from(number(row,"vint")?).map_err(|_|Error::Corrupt)?;
    api::PhysicalComponentKind::try_from(kind).map_err(|_|Error::Corrupt)?;
    Ok(api::PhysicalPosition {target:Some(target_wire(target(row)?)),owner:Some(target_wire(owner)),structural_key:string(row,"basis")?,component_kind:kind,revision:number(row,"revision")?})
}
impl Tx<'_> {
    pub(super) fn save_physical_position(&mut self,context:&Context,value:api::PhysicalPositionEdit,working:&api::WorkingContext,now:u64)->Result<api::PhysicalPosition> {
        let owner=parse_target(value.owner.as_ref())?;
        self.physical_access(context,owner,working,true)?;
        api::PhysicalComponentKind::try_from(value.component_kind).map_err(|_|Error::Invalid)?;
        let lookup=position_key(owner,&value.structural_key)?;
        let existing=self.get(&lookup)?.map(|row|target(&row)).transpose()?;
        let prior=existing.map(|target|self.get(&format!("position/{}",network::identity(target)))).transpose()?.flatten();
        if prior.as_ref().map(|row|number(row,"revision")).transpose()? != value.expected { return Err(Error::Conflict); }
        if existing.is_some() && prior.is_none() { return Err(Error::Corrupt); }
        if let Some(target)=existing {
            self.physical_access(context,target,working,true)?;
            if let Some(prior)=&prior {
                let root=self.asset(target)?;
                if root.get("name").and_then(Value::as_str)==Some(value.name.as_str()) && number(prior,"vint")?==value.component_kind as u64 {
                    return position(prior,owner);
                }
            }
        }
        let parent=self.creation_parent(context,working,Some(&target_wire(owner)),wire::AssetKind::PhysicalPosition)?;
        let target=self.workspace_write(context,api::Write {
            target:existing.map(target_wire),
            expected:existing.map(|target|self.asset(target).and_then(|row|number(&row,"revision"))).transpose()?,
            kind:wire::AssetKind::PhysicalPosition as i32,
            changes:vec![wire::Field {name:"name".into(),value:Some(field::Value::Text(value.name))}],
            ..Default::default()
        },now)?;
        let revision=number(&self.root,"revision")?.checked_add(1).ok_or(Error::Limit)?;
        let mut row=target_props(target);
        row.extend(props([("enclosure",text(network::identity(owner))),("basis",text(value.structural_key)),("vint",int(value.component_kind as u64)?),("revision",int(revision)?)]));
        self.put(&format!("position/{}",network::identity(target)),&format!("positions/{}",network::identity(owner)),Class::Inventory,row.clone())?;
        if existing.is_none() {
            self.put(&lookup,"position-keys",Class::Inventory,target_props(target))?;
            self.attach_created(context,target,wire::AssetKind::PhysicalPosition,parent,now)?;
        }
        self.audit(&network::identity(target),&context.actor,"physical-position",revision,now)?;
        position(&row,owner)
    }
    pub(super) fn physical_positions(&self,context:&Context,value:api::PhysicalPositionQuery,working:&api::WorkingContext)->Result<(Vec<api::PhysicalPosition>,Option<String>)> {
        let owner=parse_target(value.owner.as_ref())?;
        self.physical_access(context,owner,working,false)?;
        let limit=if value.limit==0 {25}else{value.limit};
        if limit>25 { return Err(Error::Limit); }
        let after=value.after.as_deref().unwrap_or("");
        if after.len()>256 { return Err(Error::Invalid); }
        let mut values=HashMap::from([("parent".into(),text(format!("positions/{}",network::identity(owner)))),("after".into(),text(after)),("limit".into(),int(u64::from(limit)+1)?)]);
        let visible=assets::visible_clause_for(context,Some("physical"),&mut values,"a");
        let rows=self.query(&format!("MATCH (p:OmRow {{parent:$parent}}) WITH p, 'asset/'+p.asset AS key MATCH (a:OmRow {{key:key}}) WHERE a.incarnation=p.incarnation AND p.key>$after AND {visible} RETURN properties(p) AS p ORDER BY p.key LIMIT $limit"),Some(values))?;
        let mut output=Vec::with_capacity(rows.len().min(limit as usize));
        for row in rows.iter().take(limit as usize) { output.push(position(row,owner)?); }
        let next=if rows.len()>limit as usize { Some(string(&rows[limit as usize-1],"key")?) }else{None};
        Ok((output,next))
    }
    pub(super) fn physical_labels(&self,context:&Context,view:&mut api::PhysicalView)->Result<()> {
        let mut targets=BTreeSet::new();
        for position in &view.positions {
            targets.insert(parse_target(position.target.as_ref())?);
            targets.insert(parse_target(position.owner.as_ref())?);
        }
        for placement in view.plan.iter().flat_map(|p|p.placements.iter()).chain(view.occupancy.iter().filter_map(|o|o.placement.as_ref())) {
            targets.insert(parse_target(placement.item.as_ref())?);
            targets.insert(parse_target(placement.enclosure.as_ref())?);
        }
        for replacement in view.plan.iter().flat_map(|p|p.replacements.iter()) {
            for item in [replacement.position.as_ref(),replacement.source_position.as_ref(),replacement.previous.as_ref(),replacement.replacement.as_ref()].into_iter().flatten() {
                targets.insert(parse_target(Some(item))?);
            }
        }
        if targets.is_empty() { return Ok(()); }
        if targets.len()>256 { return Err(Error::Limit); }
        let mut values=HashMap::from([("keys".into(),Value::Array(targets.iter().map(|t|text(asset_key(*t))).collect()))]);
        let visible=assets::visible_clause_for(context,None,&mut values,"a");
        let rows=self.query(&format!("UNWIND $keys AS key MATCH (a:OmRow {{key:key}}) WHERE {visible} RETURN properties(a) AS p LIMIT 256"),Some(values))?;
        for row in rows {
            let target=target(&row)?;
            if !targets.contains(&target) { continue; }
            let kind=i32::try_from(number(&row,"kind")?).map_err(|_|Error::Corrupt)?;
            let fields=if context.field(target,kind,"name",false) {
                row.get("name").and_then(Value::as_str).map(|value|wire::Field {name:"name".into(),value:Some(field::Value::Text(value.into()))}).into_iter().collect()
            }else{Vec::new()};
            view.items.push(api::Item { target:Some(target_wire(target)),kind,revision:number(&row,"revision")?,fields });
        }
        Ok(())
    }
}
