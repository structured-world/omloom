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
    })
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
        let proposed = value.end.ok_or(Error::Invalid)?;
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
        let revision = number(&self.root, "revision")?
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let mut row = props([
            ("name", text(&proposed.name)),
            ("basis", text(&proposed.basis)),
            ("revision", int(revision)?),
        ]);
        if let Some(connector) = &proposed.connector {
            row.insert("meaning".into(), text(connector));
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
        let ends = rows.iter().take(25).map(end).collect::<Result<Vec<_>>>()?;
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
