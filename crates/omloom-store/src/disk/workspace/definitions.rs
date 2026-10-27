use super::*;

fn definition(p: &Properties) -> Result<api::Definition> {
    let mut kinds = match p.get("kinds") {
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
    kinds.sort_unstable();
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
        generation: number(p, "generation")?,
    })
}
fn properties(d: &api::Definition) -> Result<Properties> {
    validate_definition(d)?;
    Ok(props([
        ("label", text(&d.name)),
        ("help", text(&d.description)),
        ("definition_type", Value::Int(d.r#type.into())),
        ("revision", int(d.revision)?),
        ("generation", int(d.generation)?),
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
    if s.len() != 10
        || s.bytes().enumerate().any(|(index, byte)| {
            if index == 4 || index == 7 {
                byte != b'-'
            } else {
                !byte.is_ascii_digit()
            }
        })
    {
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
        field::Value::Clear(EXPLICIT_CLEAR) => Ok(()),
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
        d.kinds.sort_unstable();
        let mut tx = self.begin()?;
        let old = if d.id.is_empty() {
            if d.revision != 0 {
                return Err(Error::Invalid);
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
            d.generation = previous.generation;
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
        } else {
            d.generation = 1;
        }
        d.revision = d.revision.checked_add(1).ok_or(Error::Limit)?;
        tx.require_definition_capacity(&d)?;
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
        next.kinds.sort_unstable();
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
        next.generation = previous.generation;
        let meaning_changed = next.r#type != previous.r#type
            || next.kinds != previous.kinds
            || next.choices != previous.choices;
        if next.revision != previous.revision {
            return Err(Error::Conflict);
        }
        // One bounded atomic migration. Failure/crash leaves definition and values
        // together on their prior generation, requiring no application history.
        let values = tx.query("MATCH (n:OmRow {layer:'intent'}) WHERE n.field=$field RETURN properties(n) AS p LIMIT 10001", params([("field", text(&next.id))]))?;
        if values.len() > 10_000 {
            return Err(Error::Limit);
        }
        let mut staged = Vec::with_capacity(values.len());
        let mut bytes = 0usize;
        let mut preview = api::MigrationPreview {
            revision: migration.expected,
            affected: values.len() as u64,
            ..Default::default()
        };
        for row in &values {
            let parent = string(row, "parent")?;
            let asset = tx.get(&parent)?.ok_or(Error::Corrupt)?;
            let target = super::super::target(&asset)?;
            let kind = number(&asset, "kind")? as i32;
            if !context.field(target, kind, &next.id, false)
                || !context.field(target, kind, &next.id, true)
            {
                return Err(Error::Denied);
            }
            let value = super::super::inventory::read_field(row)?;
            let applicable = next.kinds.contains(&kind);
            let replacement = if !applicable && migration.remove_inapplicable {
                preview.removed += 1;
                None
            } else {
                let converted = if applicable {
                    convert(
                        &value,
                        &next,
                        &migration.conversions,
                        migration.convert_types,
                    )
                } else {
                    Err(Error::Invalid)
                };
                match converted {
                    Ok(value) => Some(value),
                    Err(Error::Invalid) => {
                        preview.invalid += 1;
                        if preview.issues.len() < 32 {
                            preview.issues.push(api::MigrationIssue {
                                target: Some(target_wire(target)),
                                value: Some(value),
                                diagnostic: if applicable {
                                    "Repair this value or supply an explicit valid conversion.".into()
                                } else {
                                    "This resource kind is excluded. Explicitly review removal or retain its applicability.".into()
                                },
                            });
                        }
                        None
                    }
                    Err(error) => return Err(error),
                }
            };
            bytes = bytes
                .checked_add(replacement.as_ref().map_or(0, Message::encoded_len) + 512)
                .ok_or(Error::Limit)?;
            if bytes > 8 * 1_048_576 {
                return Err(Error::Limit);
            }
            staged.push(replacement);
        }
        let affected = staged.len() as u64;
        if !migration.apply {
            tx.commit()?;
            return Ok(reply(api::reply::Result::MigrationPreview(preview)));
        }
        if preview.invalid != 0 {
            return Err(Error::Invalid);
        }
        if next == previous
            && values.iter().zip(&staged).all(|(row, value)| {
                value
                    .as_ref()
                    .is_some_and(|value| super::super::inventory::field_matches(value, row))
            })
        {
            return Ok(reply(api::reply::Result::Mutation(api::Mutation {
                revision: tx.commit()?,
                targets: Vec::new(),
                affected,
            })));
        }
        if migration.apply {
            if meaning_changed {
                next.generation = previous.generation.checked_add(1).ok_or(Error::Limit)?;
            }
            next.revision = next.revision.checked_add(1).ok_or(Error::Limit)?;
            tx.require_definition_capacity(&next)?;
            for (row, value) in values.iter().zip(&staged) {
                let changed = if let Some(value) = value {
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
                    )?
                } else {
                    tx.delete(&string(row, "key")?)?;
                    true
                };
                let parent = string(row, "parent")?;
                let asset = tx.get(&parent)?.ok_or(Error::Corrupt)?;
                if changed || meaning_changed {
                    tx.workspace_advance(
                        super::super::target(&asset)?,
                        &context.actor,
                        "definition.value-migrated",
                        now,
                    )?;
                }
            }
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
    /// Pass the returned cursor to continue; absence starts a new bounded pass.
    pub fn workspace_cleanup_definitions(
        &self,
        limit: u16,
        after: Option<&str>,
    ) -> Result<Cleanup> {
        if limit == 0 || limit > 32 {
            return Err(Error::Invalid);
        }
        if let Some(cursor) = after {
            cursor
                .strip_prefix("definition/")
                .ok_or(Error::Invalid)?
                .parse::<AuthorityId>()
                .map_err(|_| Error::Invalid)?;
        }
        let mut tx = self.begin()?;
        let mut rows = tx.query("MATCH (n:OmRow {parent:'definitions'}) WHERE n.retired=true AND ($after IS NULL OR n.key>$after) RETURN properties(n) AS p ORDER BY n.key LIMIT $limit", params([("after", after.map(text).unwrap_or(Value::Null)), ("limit", int(u64::from(limit)+1)?)]))?;
        let cursor = if rows.len() > usize::from(limit) {
            rows.truncate(usize::from(limit));
            Some(string(rows.last().ok_or(Error::Corrupt)?, "key")?)
        } else {
            None
        };
        let mut removed = 0;
        let mut pinned = 0;
        for row in rows {
            let definition = definition(&row)?;
            let current = tx.query(
                "MATCH (n:OmRow) WHERE n.field=$field RETURN properties(n) AS p LIMIT 1",
                params([("field", text(&definition.id))]),
            )?;
            if current.is_empty() {
                tx.delete(&string(&row, "key")?)?;
                removed += 1;
            } else {
                pinned += 1;
            }
        }
        tx.commit()?;
        Ok(Cleanup {
            removed,
            pinned,
            cursor,
        })
    }
}

impl Tx<'_> {
    fn require_definition_capacity(&self, next: &api::Definition) -> Result<()> {
        let size = next.encoded_len();
        let mut bytes = 1 + prost::length_delimiter_len(size) + size;
        let mut count = 1;
        let key = format!("definition/{}", next.id);
        // Check the unfiltered schema in the mutation transaction, so every authorized
        // subset fits the transport without adding counters or work to metadata reads.
        let rows = self.query(
            "MATCH (n:OmRow {parent:'definitions'}) RETURN properties(n) AS p LIMIT 129",
            None,
        )?;
        for row in &rows {
            if row.get("key").and_then(Value::as_str) == Some(key.as_str()) {
                continue;
            }
            count += 1;
            let size = definition(row)?.encoded_len();
            bytes += 1 + prost::length_delimiter_len(size) + size;
            if count > MAX_DEFINITIONS || bytes > MAX_REPLY_BYTES {
                return Err(Error::Limit);
            }
        }
        // Definitions is field 5 of Reply; both message tags occupy one byte.
        if 1 + prost::length_delimiter_len(bytes) + bytes > MAX_REPLY_BYTES {
            return Err(Error::Limit);
        }
        Ok(())
    }
    pub(super) fn validate_field(&self, kind: i32, value: &wire::Field) -> Result<()> {
        if value.name.len() > 100 {
            return Err(Error::Invalid);
        }
        match value.name.as_str() {
            "vlan_tag" => match (wire::AssetKind::try_from(kind), value.value.as_ref()) {
                (
                    Ok(wire::AssetKind::AddressReference),
                    Some(field::Value::Integer(1..=4094) | field::Value::Clear(EXPLICIT_CLEAR)),
                ) => Ok(()),
                _ => Err(Error::Invalid),
            },
            "virtualization_role" => {
                // Existing on-disk device fields may be explicitly retired, never reassigned.
                if kind == wire::AssetKind::Device as i32
                    && matches!(value.value, Some(field::Value::Clear(EXPLICIT_CLEAR)))
                {
                    return Ok(());
                }
                if kind != wire::AssetKind::SoftwareInstallation as i32 {
                    return Err(Error::Invalid);
                }
                match value.value.as_ref() {
                    Some(field::Value::Text(s))
                        if matches!(
                            api::VirtualizationRole::from_str_name(s),
                            Some(
                                api::VirtualizationRole::Hypervisor
                                    | api::VirtualizationRole::NotHypervisor
                            )
                        ) =>
                    {
                        Ok(())
                    }
                    Some(field::Value::Clear(EXPLICIT_CLEAR)) => Ok(()),
                    _ => Err(Error::Invalid),
                }
            }
            "serial"
                if !matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::Device
                        | wire::AssetKind::Rack
                        | wire::AssetKind::Port
                        | wire::AssetKind::Cable)
                ) =>
            {
                Err(Error::Invalid)
            }
            "name" | "serial" if kind == wire::AssetKind::AddressReference as i32 => {
                Err(Error::Invalid)
            }
            "address" | "prefix_length" | "meaning" | "zone" | "binding"
                if kind == wire::AssetKind::AddressReference as i32 =>
            {
                match (value.name.as_str(), value.value.as_ref()) {
                    ("address", Some(field::Value::Text(s)))
                        if s.parse::<std::net::IpAddr>().is_ok() =>
                    {
                        Ok(())
                    }
                    ("prefix_length", Some(field::Value::Integer(0..=128))) => Ok(()),
                    ("meaning", Some(field::Value::Text(s)))
                        if api::AddressMeaning::from_str_name(s).is_some() =>
                    {
                        Ok(())
                    }
                    ("zone", Some(field::Value::Text(s)))
                        if !s.is_empty() && s.len() <= 64 && !s.chars().any(char::is_control) =>
                    {
                        Ok(())
                    }
                    ("binding", Some(field::Value::Text(s)))
                        if super::network::groups::reference(s).is_ok() =>
                    {
                        Ok(())
                    }
                    (
                        "prefix_length" | "zone" | "binding",
                        Some(field::Value::Clear(EXPLICIT_CLEAR)),
                    ) => Ok(()),
                    _ => Err(Error::Invalid),
                }
            }
            "name" | "description" | "serial" => match value.value.as_ref() {
                Some(field::Value::Text(s))
                    if s.len() <= (if value.name == "name" { 160 } else { 2048 })
                        && (value.name != "name" || !s.trim().is_empty()) =>
                {
                    Ok(())
                }
                Some(field::Value::Clear(EXPLICIT_CLEAR)) if value.name != "name" => Ok(()),
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
