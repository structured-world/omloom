//! Named cable-end definitions, independent of their replaceable attachments.
use super::*;

fn parent(cable: AssetRef) -> String {
    format!("cable-ends/{}", network::identity(cable))
}
fn token(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn end(row: &Properties) -> Result<api::PhysicalCableEnd> {
    let key = row
        .get("key")
        .and_then(Value::as_str)
        .and_then(|s| s.rsplit_once('/'))
        .map(|(_, key)| key)
        .ok_or(Error::Corrupt)?;
    token(key).map_err(|_| Error::Corrupt)?;
    Ok(api::PhysicalCableEnd {
        key: key.into(),
        name: string(row, "name")?,
        basis: string(row, "basis")?,
        connector: row
            .get("meaning")
            .map(|_| string(row, "meaning"))
            .transpose()?,
        revision: number(row, "revision")?,
        attachment: None,
        intended_port: None,
        mating: connector::read(row)?,
        channels: read_channels(row)?,
    })
}
pub(super) fn normalize_channels(channels: &mut [u32]) -> Result<()> {
    if channels.len() > 64 {
        return Err(Error::Limit);
    }
    channels.sort_unstable();
    let mut previous = 0;
    for channel in channels {
        if *channel <= previous || *channel > 4096 {
            return Err(Error::Invalid);
        }
        previous = *channel;
    }
    Ok(())
}
fn read_channels(row: &Properties) -> Result<Vec<u32>> {
    let Some(Value::Array(values)) = row.get("counts") else {
        if row.contains_key("counts") {
            return Err(Error::Corrupt);
        }
        return Ok(Vec::new());
    };
    if values.len() > 64 {
        return Err(Error::Corrupt);
    }
    let mut result = Vec::with_capacity(values.len());
    let mut previous = 0;
    for value in values {
        let Value::Int(value) = value else {
            return Err(Error::Corrupt);
        };
        let value = u32::try_from(*value).map_err(|_| Error::Corrupt)?;
        if value <= previous || value > 4096 {
            return Err(Error::Corrupt);
        }
        result.push(value);
        previous = value;
    }
    Ok(result)
}
impl Tx<'_> {
    pub(super) fn save_cable_end(
        &mut self,
        context: &Context,
        value: api::PhysicalCableEndEdit,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalCableEnd> {
        let cable = parse_target(value.cable.as_ref())?;
        if self.physical_access(context, cable, working, true)? != wire::AssetKind::Cable {
            return Err(Error::Invalid);
        }
        let mut proposed = value.end.ok_or(Error::Invalid)?;
        normalize_channels(&mut proposed.channels)?;
        if proposed.attachment.is_some() || proposed.intended_port.is_some() {
            return Err(Error::Invalid);
        }
        token(&proposed.key)?;
        for (text, max) in [(&proposed.name, 128), (&proposed.basis, 512)] {
            if text.trim().is_empty() || text.len() > max || text.chars().any(char::is_control) {
                return Err(Error::Invalid);
            }
        }
        if proposed.connector.as_ref().is_some_and(|s| {
            s.trim().is_empty() || s.len() > 128 || s.chars().any(char::is_control)
        }) {
            return Err(Error::Invalid);
        }
        let parent = parent(cable);
        let key = format!("{parent}/{}", proposed.key);
        let prior = self.get(&key)?;
        if prior
            .as_ref()
            .map(|row| number(row, "revision"))
            .transpose()?
            != value.expected
        {
            return Err(Error::Conflict);
        }
        if let Some(row) = &prior {
            let previous = end(row)?;
            if previous.name == proposed.name
                && previous.basis == proposed.basis
                && previous.connector == proposed.connector
                && previous.mating == proposed.mating
                && previous.channels == proposed.channels
            {
                return Ok(previous);
            }
        } else {
            let existing = self.query(
                "MATCH (n:OmRow {parent:$parent}) RETURN {key:n.key} AS p LIMIT 65",
                params([("parent", text(&parent))]),
            )?;
            if existing.len() >= 64 {
                return Err(Error::Limit);
            }
        }
        if !proposed.channels.is_empty() {
            // At most 64 real end definitions, not one inventory object per lane.
            // A physical channel has at most two ends even in a breakout assembly.
            let rows = self.query(
                "MATCH (n:OmRow {parent:$parent}) RETURN properties(n) AS p LIMIT 65",
                params([("parent", text(&parent))]),
            )?;
            if rows.len() > 64 {
                return Err(Error::Corrupt);
            }
            let mut counts = BTreeMap::new();
            for row in rows {
                if string(&row, "key")? == key {
                    continue;
                }
                for channel in read_channels(&row)? {
                    *counts.entry(channel).or_insert(0usize) += 1;
                }
            }
            if proposed
                .channels
                .iter()
                .any(|channel| counts.get(channel).is_some_and(|count| *count >= 2))
            {
                return Err(Error::Conflict);
            }
        }
        let revision = match prior.as_ref() {
            Some(row) => number(row, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?,
            None => 1,
        };
        let mut row = prior.unwrap_or_default();
        operations::strip_envelope(&mut row);
        row.extend(props([
            ("name", text(&proposed.name)),
            ("basis", text(&proposed.basis)),
            ("revision", int(revision)?),
        ]));
        if let Some(connector) = &proposed.connector {
            row.insert("meaning".into(), text(connector));
        } else {
            row.remove("meaning");
        }
        connector::store(&mut row, proposed.mating.as_ref())?;
        if proposed.channels.is_empty() {
            row.remove("counts");
        } else {
            row.insert(
                "counts".into(),
                Value::Array(
                    proposed
                        .channels
                        .iter()
                        .map(|v| Value::Int(i64::from(*v)))
                        .collect(),
                ),
            );
        }
        self.put(&key, &parent, Class::Inventory, row)?;
        self.audit(
            &format!("{}/{}", cable.asset, proposed.key),
            &context.actor,
            "cable-end",
            revision,
            now,
        )?;
        Ok(api::PhysicalCableEnd {
            revision,
            ..proposed
        })
    }
    pub(super) fn cable_ends(
        &self,
        context: &Context,
        value: api::PhysicalCableEnds,
        working: &api::WorkingContext,
    ) -> Result<(Vec<api::PhysicalCableEnd>, Option<String>)> {
        let cable = parse_target(value.cable.as_ref())?;
        if self.physical_access(context, cable, working, false)? != wire::AssetKind::Cable {
            return Err(Error::Invalid);
        }
        if let Some(after) = &value.after {
            token(after)?;
        }
        let parent = parent(cable);
        let after = match value.after {
            Some(value) => format!("{parent}/{value}"),
            None => String::new(),
        };
        let rows=self.query("MATCH (n:OmRow {parent:$parent}) WHERE n.key>$after RETURN properties(n) AS p ORDER BY n.key LIMIT 26",params([("parent",text(parent)),("after",text(after))]))?;
        let selected = &rows[..rows.len().min(25)];
        let mut ends = selected.iter().map(end).collect::<Result<Vec<_>>>()?;
        self.project_cable_attachments(context, cable, selected, &mut ends)?;
        let next = if rows.len() > 25 {
            Some(ends.last().ok_or(Error::Corrupt)?.key.clone())
        } else {
            None
        };
        Ok((ends, next))
    }
    pub(in crate::disk::workspace) fn remove_cable_ends(&mut self, cable: AssetRef) -> Result<()> {
        let rows = self.query(
            "MATCH (n:OmRow {parent:$parent}) RETURN {key:n.key} AS p LIMIT 65",
            params([("parent", text(parent(cable)))]),
        )?;
        if rows.len() > 64 {
            return Err(Error::Corrupt);
        }
        for row in rows {
            self.delete(&string(&row, "key")?)?;
        }
        Ok(())
    }
}

pub(super) fn intended(row: &Properties) -> Result<Option<AssetRef>> {
    match (row.get("to_asset"), row.get("to_incarnation")) {
        (None, None) => Ok(None),
        (Some(asset), Some(incarnation)) => Ok(Some(AssetRef {
            asset: asset
                .as_str()
                .ok_or(Error::Corrupt)?
                .parse()
                .map_err(|_| Error::Corrupt)?,
            incarnation: incarnation
                .as_str()
                .ok_or(Error::Corrupt)?
                .parse()
                .map_err(|_| Error::Corrupt)?,
        })),
        _ => Err(Error::Corrupt),
    }
}
pub(super) fn attached_port(binding: &wire::Relation, cable: AssetRef) -> Result<AssetRef> {
    let from = parse_target(binding.from.as_ref())?;
    let to = parse_target(binding.to.as_ref())?;
    if binding.kind != wire::RelationKind::ConnectedTo as i32 {
        return Err(Error::Corrupt);
    }
    if from == cable {
        Ok(to)
    } else if to == cable {
        Ok(from)
    } else {
        Err(Error::Corrupt)
    }
}
impl Tx<'_> {
    fn project_cable_attachments(
        &self,
        context: &Context,
        cable: AssetRef,
        rows: &[Properties],
        output: &mut [api::PhysicalCableEnd],
    ) -> Result<()> {
        if !context.field(cable, wire::AssetKind::Cable as i32, "relations", false) {
            return Ok(());
        }
        let mut keys = Vec::with_capacity(rows.len());
        for row in rows {
            if row.get("state") == Some(&Value::Int(1))
                && let Some(id) = row.get("binding")
            {
                keys.push(text(format!(
                    "relation/{}",
                    id.as_str().ok_or(Error::Corrupt)?
                )));
            }
        }
        let mut bindings = BTreeMap::new();
        let mut ports = BTreeSet::new();
        if !keys.is_empty() {
            for row in self.query(
                "UNWIND $keys AS key MATCH (r:OmRow {key:key}) RETURN properties(r) AS p LIMIT 25",
                params([("keys", Value::Array(keys))]),
            )? {
                let binding = inventory::relation_projection(&row)?;
                ports.insert(attached_port(&binding, cable)?);
                if bindings
                    .insert(binding.relation_id.clone(), binding)
                    .is_some()
                {
                    return Err(Error::Corrupt);
                }
            }
        }
        for row in rows {
            if let Some(port) = intended(row)? {
                ports.insert(port);
            }
        }
        let mut visible = BTreeSet::new();
        if !ports.is_empty() {
            let keys = ports.iter().map(|p| text(asset_key(*p))).collect();
            for row in self.query("UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN {asset:p.asset,incarnation:p.incarnation,kind:p.kind,adoption:p.adoption} AS p LIMIT 50",params([("keys",Value::Array(keys))]))? {
                let target=target(&row)?;
                if number(&row,"kind")?==wire::AssetKind::Port as u64&&number(&row,"adoption")?==wire::Adoption::Adopted as u64&&context.readable(target,wire::AssetKind::Port as i32)&&context.field(target,wire::AssetKind::Port as i32,"relations",false){visible.insert(target);}
            }
        }
        for (row, result) in rows.iter().zip(output) {
            if let Some(port) = intended(row)?
                && visible.contains(&port)
            {
                result.intended_port = Some(target_wire(port));
            }
            if row.get("state") == Some(&Value::Int(2)) {
                if row.contains_key("binding") {
                    return Err(Error::Corrupt);
                }
                result.attachment = Some(api::physical_cable_end::Attachment::Loose(api::Empty {}));
                continue;
            }
            if row.get("state") != Some(&Value::Int(1)) {
                continue;
            }
            let Some(id) = row.get("binding").and_then(Value::as_str) else {
                continue;
            };
            let Some(binding) = bindings.get(id) else {
                continue;
            };
            let port = attached_port(binding, cable)?;
            if binding.revision == number(row, "expected")? && visible.contains(&port) {
                result.attachment = Some(api::physical_cable_end::Attachment::Connected(
                    api::PhysicalCableAttachment {
                        port: Some(target_wire(port)),
                        binding_id: binding.relation_id.clone(),
                        binding_revision: binding.revision,
                    },
                ));
            }
        }
        Ok(())
    }
}

impl Tx<'_> {
    // A generic binding edit cannot silently move an identified cable end.
    pub(in crate::disk) fn invalidate_cable_attachment(
        &mut self,
        relation: &wire::Relation,
        skip: Option<&str>,
        retained_revision: Option<u64>,
    ) -> Result<()> {
        if relation.kind != wire::RelationKind::ConnectedTo as i32 {
            return Ok(());
        }
        let from = parse_target(relation.from.as_ref())?;
        let to = parse_target(relation.to.as_ref())?;
        let cable = if number(&self.asset(from)?, "kind")? == wire::AssetKind::Cable as u64 {
            from
        } else if number(&self.asset(to)?, "kind")? == wire::AssetKind::Cable as u64 {
            to
        } else {
            return Err(Error::Corrupt);
        };
        let rows=self.query("MATCH (n:OmRow {parent:$parent}) WHERE n.binding=$binding RETURN properties(n) AS p LIMIT 2",params([("parent",text(parent(cable))),("binding",text(&relation.relation_id))]))?;
        if rows.len() > 1 {
            return Err(Error::Corrupt);
        }
        for mut row in rows {
            let key = string(&row, "key")?;
            if skip == Some(key.as_str()) {
                continue;
            }
            if retained_revision.is_none() {
                return Err(Error::Conflict);
            }
            operations::strip_envelope(&mut row);
            if let Some(revision) = retained_revision {
                row.insert("expected".into(), int(revision)?);
            } else {
                row.remove("binding");
                row.remove("expected");
                row.remove("state");
            }
            row.insert(
                "revision".into(),
                int(number(&row, "revision")?
                    .checked_add(1)
                    .ok_or(Error::Limit)?)?,
            );
            self.put(&key, &parent(cable), Class::Inventory, row)?;
        }
        Ok(())
    }
}
