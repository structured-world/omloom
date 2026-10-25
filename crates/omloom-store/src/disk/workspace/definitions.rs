use super::*;

fn definition(p: &Properties) -> Result<api::Definition> {
    let kinds = match p.get("kinds") {
        Some(Value::Array(v)) => v
            .iter()
            .map(|v| {
                v.as_int()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or(Error::Corrupt)
            })
            .collect::<Result<Vec<_>>>()?,
        _ => return Err(Error::Corrupt),
    };
    Ok(api::Definition {
        id: string(p, "key")?
            .strip_prefix("definition/")
            .ok_or(Error::Corrupt)?
            .into(),
        name: string(p, "label")?,
        description: string(p, "help")?,
        r#type: number(p, "definition_type")? as i32,
        kinds,
        choices: read_strings(p, "choices")?,
        searchable: flag(p, "searchable")?,
        revision: number(p, "revision")?,
        retired: flag(p, "retired")?,
    })
}
fn properties(d: &api::Definition) -> Result<Properties> {
    validate_definition(d)?;
    Ok(props([
        ("label", text(&d.name)),
        ("help", text(&d.description)),
        ("definition_type", Value::Int(d.r#type.into())),
        ("revision", int(d.revision)?),
        (
            "kinds",
            Value::Array(d.kinds.iter().map(|n| Value::Int((*n).into())).collect()),
        ),
        ("choices", strings(&d.choices)),
        ("searchable", Value::Bool(d.searchable)),
        ("retired", Value::Bool(d.retired)),
    ]))
}
fn validate_definition(d: &api::Definition) -> Result<()> {
    if d.name.trim().is_empty()
        || d.name.len() > 100
        || d.description.len() > 1000
        || d.kinds.is_empty()
        || d.kinds.len() > 32
        || d.choices.len() > 64
        || d.choices.iter().any(|c| c.is_empty() || c.len() > 160)
    {
        return Err(Error::Invalid);
    }
    let kind = api::FieldType::try_from(d.r#type).map_err(|_| Error::Invalid)?;
    if (kind == api::FieldType::Choice) != !d.choices.is_empty() {
        return Err(Error::Invalid);
    }
    if d.choices.iter().collect::<BTreeSet<_>>().len() != d.choices.len()
        || d.kinds.iter().collect::<BTreeSet<_>>().len() != d.kinds.len()
    {
        return Err(Error::Invalid);
    }
    for kind in &d.kinds {
        if wire::AssetKind::try_from(*kind)
            .ok()
            .filter(|k| *k != wire::AssetKind::Unspecified)
            .is_none()
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
fn date(s: &str) -> bool {
    if s.len() != 10 || !s.is_ascii() || &s[4..5] != "-" || &s[7..8] != "-" {
        return false;
    }
    let Ok(year) = s[..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = s[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = s[8..].parse::<u32>() else {
        return false;
    };
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    year > 0 && day > 0 && day <= days
}
pub(super) fn validate_value(d: &api::Definition, value: &wire::Field) -> Result<()> {
    use api::FieldType as T;
    match value.value.as_ref().ok_or(Error::Invalid)? {
        field::Value::Clear(0) => Ok(()),
        field::Value::Text(s) if s.len() <= 2048 => {
            match T::try_from(d.r#type).map_err(|_| Error::Invalid)? {
                T::Text => Ok(()),
                T::Date if date(s) => Ok(()),
                T::Choice if d.choices.contains(s) => Ok(()),
                _ => Err(Error::Invalid),
            }
        }
        field::Value::Integer(_) if d.r#type == T::Integer as i32 => Ok(()),
        field::Value::Flag(_) if d.r#type == T::Boolean as i32 => Ok(()),
        _ => Err(Error::Invalid),
    }
}
fn convert(
    value: &wire::Field,
    next: &api::Definition,
    rules: &[api::Conversion],
    automatic: bool,
) -> Result<wire::Field> {
    if let Some(rule) = rules.iter().find(|rule| rule.from.as_ref() == Some(value)) {
        let replacement = rule.to.as_ref().ok_or(Error::Invalid)?;
        if replacement.name != value.name {
            return Err(Error::Invalid);
        }
        validate_value(next, replacement)?;
        return Ok(replacement.clone());
    }
    if validate_value(next, value).is_ok() {
        return Ok(value.clone());
    }
    if !automatic {
        return Err(Error::Invalid);
    }
    let converted = match (
        value.value.as_ref().ok_or(Error::Invalid)?,
        api::FieldType::try_from(next.r#type).map_err(|_| Error::Invalid)?,
    ) {
        (field::Value::Integer(n), api::FieldType::Text) => field::Value::Text(n.to_string()),
        (field::Value::Flag(b), api::FieldType::Text) => field::Value::Text(b.to_string()),
        (field::Value::Text(s), api::FieldType::Integer) => {
            field::Value::Integer(s.parse().map_err(|_| Error::Invalid)?)
        }
        (field::Value::Text(s), api::FieldType::Boolean) if s == "true" || s == "false" => {
            field::Value::Flag(s == "true")
        }
        (
            field::Value::Text(s),
            api::FieldType::Text | api::FieldType::Date | api::FieldType::Choice,
        ) => field::Value::Text(s.clone()),
        _ => return Err(Error::Invalid),
    };
    let output = wire::Field {
        name: value.name.clone(),
        value: Some(converted),
    };
    validate_value(next, &output)?;
    Ok(output)
}

impl Store {
    pub(super) fn workspace_definitions(&self, context: &Context) -> Result<Vec<api::Definition>> {
        self.workspace_rows("definitions", MAX_DEFINITIONS)?
            .iter()
            .map(definition)
            .filter(|r| match r {
                Err(_) => true,
                Ok(d) => {
                    context.schema_admin || context.grants.iter().any(|g| g.read.contains(&d.id))
                }
            })
            .collect()
    }
    pub(super) fn workspace_definition(
        &self,
        context: &Context,
        mut d: api::Definition,
        now: u64,
    ) -> Result<api::Reply> {
        context.admin()?;
        validate_definition(&d)?;
        let mut tx = self.begin()?;
        let old = if d.id.is_empty() {
            if d.revision != 0 {
                return Err(Error::Invalid);
            }
            if self.workspace_rows("definitions", MAX_DEFINITIONS)?.len() >= MAX_DEFINITIONS {
                return Err(Error::Limit);
            }
            d.id = AuthorityId::generate().to_string();
            None
        } else {
            d.id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
            Some(
                tx.get(&format!("definition/{}", d.id))?
                    .ok_or(Error::Missing)?,
            )
        };
        if let Some(row) = &old {
            let previous = definition(row)?;
            if d.revision != previous.revision {
                return Err(Error::Conflict);
            }
            if previous.r#type != d.r#type
                || previous.kinds != d.kinds
                || previous.choices != d.choices
            {
                return Err(Error::Conflict);
            }
            if previous == d {
                return Ok(mutation(tx.commit()?, Vec::new()));
            }
        }
        d.revision = d.revision.checked_add(1).ok_or(Error::Limit)?;
        tx.put_against(
            &format!("definition/{}", d.id),
            "definitions",
            Class::Inventory,
            properties(&d)?,
            old.as_ref(),
        )?;
        tx.audit(
            &format!("definition/{}", d.id),
            &context.actor,
            "definition.accepted",
            d.revision,
            now,
        )?;
        tx.commit()?;
        Ok(reply(api::reply::Result::Definitions(api::Definitions {
            items: vec![d],
        })))
    }

    pub(super) fn workspace_migration(
        &self,
        context: &Context,
        migration: api::Migration,
        now: u64,
    ) -> Result<api::Reply> {
        context.admin()?;
        let mut next = migration.definition.ok_or(Error::Invalid)?;
        validate_definition(&next)?;
        if migration.conversions.len() > 128 {
            return Err(Error::Limit);
        }
        let key = format!("definition/{}", next.id);
        let mut tx = self.begin()?;
        if number(&tx.root, "revision")? != migration.expected {
            return Err(Error::Conflict);
        }
        let old = tx.get(&key)?.ok_or(Error::Missing)?;
        let previous = definition(&old)?;
        if next.revision != previous.revision {
            return Err(Error::Conflict);
        }
        // One bounded atomic migration. Failure/crash leaves definition and values
        // together on their prior generation, requiring no application history.
        let values = tx.query("MATCH (n:OmRow) WHERE n.layer='intent' AND n.field=$field RETURN properties(n) AS p LIMIT 10001", params([("field", text(&next.id))]))?;
        if values.len() > 10_000 {
            return Err(Error::Limit);
        }
        let mut staged = Vec::with_capacity(values.len());
        let mut bytes = 0usize;
        for row in &values {
            let value = super::super::inventory::read_field(row)?;
            let replacement = convert(
                &value,
                &next,
                &migration.conversions,
                migration.convert_types,
            )?;
            bytes = bytes
                .checked_add(replacement.encoded_len() + 512)
                .ok_or(Error::Limit)?;
            if bytes > 8 * 1_048_576 {
                return Err(Error::Limit);
            }
            let parent = string(row, "parent")?;
            let asset = tx.get(&parent)?.ok_or(Error::Corrupt)?;
            if !next.kinds.contains(&(number(&asset, "kind")? as i32)) {
                return Err(Error::Invalid);
            }
            staged.push(replacement);
        }
        let affected = staged.len() as u64;
        if migration.apply {
            for (row, value) in values.iter().zip(&staged) {
                let mut data = super::super::inventory::field_props(value)?;
                data.extend(props([
                    ("layer", text("intent")),
                    ("authority", text(string(row, "authority")?)),
                ]));
                tx.put_against(
                    &string(row, "key")?,
                    &string(row, "parent")?,
                    Class::Inventory,
                    data,
                    Some(row),
                )?;
                let parent = string(row, "parent")?;
                let asset = tx.get(&parent)?.ok_or(Error::Corrupt)?;
                tx.workspace_advance(
                    super::super::target(&asset)?,
                    &context.actor,
                    "definition.value-migrated",
                    now,
                )?;
            }
            next.revision = next.revision.checked_add(1).ok_or(Error::Limit)?;
            tx.put_against(
                &key,
                "definitions",
                Class::Inventory,
                properties(&next)?,
                Some(&old),
            )?;
            tx.audit(
                &key,
                &context.actor,
                "definition.migrated",
                next.revision,
                now,
            )?;
        }
        Ok(reply(api::reply::Result::Mutation(api::Mutation {
            revision: tx.commit()?,
            targets: Vec::new(),
            affected,
        })))
    }

    /// Reclaims only retired definitions with no current values. UUIDs are never
    /// accepted for creation, so removing an unused definition cannot reuse it.
    pub fn workspace_cleanup_definitions(&self, limit: u16) -> Result<u16> {
        if limit == 0 || limit > 32 {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        let rows = tx.query("MATCH (n:OmRow) WHERE n.parent='definitions' AND n.retired=true RETURN properties(n) AS p LIMIT $limit", params([("limit", int(u64::from(limit))?)]))?;
        let mut removed = 0;
        for row in rows {
            let definition = definition(&row)?;
            let current = tx.query(
                "MATCH (n:OmRow) WHERE n.field=$field RETURN properties(n) AS p LIMIT 1",
                params([("field", text(&definition.id))]),
            )?;
            if current.is_empty() {
                tx.delete(&string(&row, "key")?)?;
                removed += 1;
            }
        }
        tx.commit()?;
        Ok(removed)
    }
}

impl Tx<'_> {
    pub(super) fn validate_field(&self, kind: i32, value: &wire::Field) -> Result<()> {
        if value.name.len() > 100 {
            return Err(Error::Invalid);
        }
        match value.name.as_str() {
            "name" | "description" | "serial" => match value.value.as_ref() {
                Some(field::Value::Text(s))
                    if s.len() <= (if value.name == "name" { 160 } else { 2048 })
                        && (value.name != "name" || !s.trim().is_empty()) =>
                {
                    Ok(())
                }
                Some(field::Value::Clear(0)) if value.name != "name" => Ok(()),
                _ => Err(Error::Invalid),
            },
            _ => {
                value
                    .name
                    .parse::<AuthorityId>()
                    .map_err(|_| Error::Invalid)?;
                let row = self
                    .get(&format!("definition/{}", value.name))?
                    .ok_or(Error::Incomplete)?;
                let d = definition(&row)?;
                if d.retired || !d.kinds.contains(&kind) {
                    return Err(Error::Incomplete);
                }
                validate_value(&d, value)
            }
        }
    }
}
