use super::*;

struct CsvOutput(Vec<u8>);
fn csv_write_error(error: csv::Error) -> Error {
    match error.kind() {
        csv::ErrorKind::Io(_) => Error::Limit,
        _ => Error::Invalid,
    }
}
impl std::io::Write for CsvOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_CSV - self.0.len() {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn csv_value(
    name: &str,
    value: &str,
    definitions: &[api::Definition],
    budget: &omloom_contracts::MemoryBudget,
    inputs: &mut Vec<omloom_contracts::equipment::Input>,
) -> Result<wire::Field> {
    let kind = definitions.iter().find(|d| d.id == name).map(|d| d.r#type);
    if omloom_contracts::equipment::FIELDS.contains(&name) {
        if let Some(body) = value.strip_prefix("equipment/claim/v1:") {
            let mut input =
                omloom_contracts::equipment::decode_hex(body, budget).map_err(|e| match e {
                    omloom_contracts::Error::Limit => Error::Limit,
                    _ => Error::Invalid,
                })?;
            let claim = input.take();
            inputs.push(input);
            return Ok(wire::Field {
                name: name.into(),
                value: Some(field::Value::Equipment(claim)),
            });
        }
        if let Some(body) = value.strip_prefix("equipment/text/v1:") {
            if body.len() > (if name == "model" { 1024 } else { 320 })
                || !body.len().is_multiple_of(2)
            {
                return Err(Error::Invalid);
            }
            let bytes = body
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    std::str::from_utf8(c)
                        .ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                        .ok_or(Error::Invalid)
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(wire::Field {
                name: name.into(),
                value: Some(field::Value::Text(
                    String::from_utf8(bytes).map_err(|_| Error::Invalid)?,
                )),
            });
        }
    }
    let value = if value == "<clear>" {
        field::Value::Clear(EXPLICIT_CLEAR)
    } else if kind == Some(api::FieldType::Integer as i32)
        || matches!(name, "prefix_length" | "vlan_tag" | "drive_bays")
    {
        field::Value::Integer(value.parse().map_err(|_| Error::Invalid)?)
    } else if kind == Some(api::FieldType::Boolean as i32) {
        match value {
            "true" => field::Value::Flag(true),
            "false" => field::Value::Flag(false),
            _ => return Err(Error::Invalid),
        }
    } else {
        field::Value::Text(value.into())
    };
    Ok(wire::Field {
        name: name.into(),
        value: Some(value),
    })
}
fn mapping_key(actor: &str, source: &str, external: &str) -> String {
    let mut hash = Sha256::new();
    for part in [actor, source, external] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("csv-mapping/{:x}", hash.finalize())
}
impl Store {
    pub(super) fn workspace_import(
        &self,
        context: &Context,
        request: api::Import,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if let Some(id) = request.preview {
            id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
            if !request.csv.is_empty() || !request.mappings.is_empty() {
                return Err(Error::Invalid);
            }
            let key = format!("preview/{id}");
            let mut tx = self.begin()?;
            tx.selection(context, working, true)?;
            let row = tx.get(&key)?.ok_or(Error::Expired)?;
            if string(&row, "owner")? != context.actor {
                return Err(Error::Denied);
            }
            if number(&row, "expires")? <= now {
                return Err(Error::Expired);
            }
            if number(&row, "revision")? != number(&tx.root, "revision")? {
                return Err(Error::Conflict);
            }
            let mut selector = working.clone();
            selector.expected = None;
            if !row.contains_key("working") {
                return Err(Error::Expired);
            }
            if binary(&row, "working")? != selector.encode_to_vec() {
                return Err(Error::Conflict);
            }
            let source = string(&row, "source")?;
            let preview =
                api::Preview::decode(binary(&row, "query")?).map_err(|_| Error::Corrupt)?;
            let mut targets = Vec::with_capacity(preview.rows.len());
            for line in preview.rows {
                let change = line.change.ok_or(Error::Corrupt)?;
                let mapping = tx
                    .get(&format!("{key}/{}", line.row))?
                    .ok_or(Error::Corrupt)?;
                let external = string(&mapping, "object_id")?;
                let target = tx.context_write(context, change, working, now)?;
                let mut data = target_props(target);
                data.insert("source".into(), text(&source));
                tx.put(
                    &mapping_key(&context.actor, &source, &external),
                    "csv-mappings",
                    Class::Inventory,
                    data,
                )?;
                tx.delete(&string(&mapping, "key")?)?;
                targets.push(target_wire(target));
            }
            tx.delete(&key)?;
            return Ok(mutation(tx.commit()?, targets));
        }
        if request.csv.len() > MAX_CSV
            || request.csv.is_empty()
            || request.mappings.is_empty()
            || request.mappings.len() > MAX_FIELDS
            || request.source.is_empty()
            || request.source.len() > 100
        {
            return Err(Error::Invalid);
        }
        let definitions = self.workspace_definitions(context)?;
        // Keep decoded-claim reservations for the entire preview construction.
        let claim_budget = omloom_contracts::MemoryBudget::new(16 * 1_048_576);
        let mut claim_inputs = Vec::new();
        let mut reader = csv::ReaderBuilder::new()
            .flexible(false)
            .from_reader(request.csv.as_bytes());
        let headers = reader.headers().map_err(|_| Error::Invalid)?.clone();
        if headers.len() > 40 || headers.iter().collect::<BTreeSet<_>>().len() != headers.len() {
            return Err(Error::Invalid);
        }
        let external_index = headers
            .iter()
            .position(|h| h == "external_id")
            .ok_or(Error::Invalid)?;
        let mut mapping = Vec::new();
        let mut destinations = BTreeSet::new();
        for selected in &request.mappings {
            if !destinations.insert(&selected.field) {
                return Err(Error::Invalid);
            }
            super::assets::validate_column(&selected.field, &definitions)?;
            let index = headers
                .iter()
                .position(|h| h == selected.source)
                .ok_or(Error::Invalid)?;
            mapping.push((index, selected.field.as_str()));
        }
        let mut tx = self.begin()?;
        let selection = tx.selection(context, working, true)?;
        let base = number(&tx.root, "revision")?;
        let mut rows = Vec::new();
        let mut external_ids = BTreeSet::new();
        for (index, line) in reader.records().enumerate() {
            if index >= MAX_IMPORT {
                return Err(Error::Limit);
            }
            let row_number = (index + 2) as u32;
            let mut diagnostic = "CSV record".to_owned();
            let parsed = (|| {
                let line = line.map_err(|_| Error::Invalid)?;
                diagnostic = "column 'external_id'".into();
                let external = line.get(external_index).ok_or(Error::Invalid)?.to_owned();
                if external.is_empty()
                    || external.len() > 256
                    || !external_ids.insert(external.clone())
                {
                    return Err(Error::Invalid);
                }
                diagnostic = "source identity and current scope".into();
                let prior = tx.get(&mapping_key(&context.actor, &request.source, &external))?;
                let target = prior.as_ref().map(super::super::target).transpose()?;
                let existing = target.map(|t| tx.visible_asset(context, t)).transpose()?;
                if let Some(target) = target {
                    selection.contains(target, &tx)?;
                }
                let kind = existing
                    .as_ref()
                    .map(|p| number(p, "kind").map(|n| n as i32))
                    .transpose()?
                    .unwrap_or(request.kind);
                diagnostic = "resource kind".into();
                let typed_kind = wire::AssetKind::try_from(kind).map_err(|_| Error::Invalid)?;
                if typed_kind == wire::AssetKind::Unspecified {
                    return Err(Error::Invalid);
                }
                if target.is_none()
                    && matches!(
                        typed_kind,
                        wire::AssetKind::IpAddress
                            | wire::AssetKind::Prefix
                            | wire::AssetKind::Vlan
                            | wire::AssetKind::AddressGroup
                    )
                {
                    diagnostic = "resource kind (use reviewed network allocation)".into();
                    return Err(Error::Invalid);
                }
                if target.is_none() {
                    tx.creation_parent(context, working, request.parent.as_ref(), typed_kind)?;
                }
                let expected = existing
                    .as_ref()
                    .map(|p| number(p, "revision"))
                    .transpose()?;
                let write_target = target.unwrap_or_else(new_target);
                let mut fields = Vec::with_capacity(mapping.len());
                for (index, name) in &mapping {
                    diagnostic = format!("column '{}'", headers.get(*index).ok_or(Error::Invalid)?);
                    let cell = line.get(*index).ok_or(Error::Invalid)?;
                    if cell.is_empty()
                        && typed_kind == wire::AssetKind::Device
                        && omloom_contracts::equipment::FIELDS.contains(name)
                    {
                        continue;
                    }
                    let field =
                        csv_value(name, cell, &definitions, &claim_budget, &mut claim_inputs)?;
                    tx.require_changes(context, write_target, kind, std::slice::from_ref(&field))?;
                    fields.push(field);
                }
                let change = api::Write {
                    target: target.map(target_wire),
                    expected,
                    kind,
                    changes: fields,
                    parent: if target.is_none() {
                        request.parent.clone()
                    } else {
                        None
                    },
                };
                if typed_kind == wire::AssetKind::AddressReference {
                    diagnostic = "address facts (IP family, mask and local zone; resolve canonical mappings separately)".into();
                    tx.preview_address(&change)?;
                }
                tx.validate_software_state(write_target, typed_kind, &change.changes)?;
                Ok((external, change))
            })();
            match parsed {
                Ok((external, change)) => rows.push((external, api::ImportRow { row: row_number, change: Some(change), diagnostic: String::new(), addresses: None })),
                Err(error) => rows.push((String::new(), api::ImportRow { row: row_number, change: None, diagnostic: format!("Row {row_number}, {diagnostic}: {error}. Review the mapping, declared type and current scope."), addresses: None })),
            }
        }
        if rows.is_empty() {
            return Err(Error::Invalid);
        }
        let valid = rows.iter().all(|(_, r)| r.change.is_some());
        let mut preview = api::Preview {
            id: None,
            rows: rows.iter().map(|(_, r)| r.clone()).collect(),
            revision: base,
        };
        if valid {
            let pending = tx.query("MATCH (n:OmRow {parent:'workspace-previews'}) WHERE n.owner=$owner RETURN properties(n) AS p LIMIT 5", params([("owner", text(&context.actor))]))?;
            if pending.len() > 4 {
                return Err(Error::Limit);
            }
            // Committing this preview invalidates earlier revisions. Replace only this actor's drafts.
            for prior in pending {
                if number(&prior, "revision")? > base {
                    return Err(Error::Corrupt);
                }
                let key = string(&prior, "key")?;
                let children = tx.children(&key)?;
                if children.len() > MAX_IMPORT {
                    return Err(Error::Limit);
                }
                for child in children {
                    tx.delete(&string(&child, "key")?)?;
                }
                tx.delete(&key)?;
            }
            let id = AuthorityId::generate().to_string();
            let key = format!("preview/{id}");
            let expires = now
                .checked_add(self.policy.review_age_ms)
                .ok_or(Error::Limit)?;
            preview.id = Some(id);
            preview.revision = base.checked_add(1).ok_or(Error::Limit)?;
            let bytes = preview.encode_to_vec();
            let mut selector = working.clone();
            selector.expected = None;
            if bytes.len() > MAX_CSV {
                return Err(Error::Limit);
            }
            tx.put(
                &key,
                "workspace-previews",
                Class::Review,
                props([
                    ("owner", text(&context.actor)),
                    ("source", text(&request.source)),
                    ("revision", int(preview.revision)?),
                    ("expires", int(expires)?),
                    ("query", Value::Binary(bytes)),
                    ("working", Value::Binary(selector.encode_to_vec())),
                ]),
            )?;
            for (external, row) in &rows {
                tx.put(
                    &format!("{key}/{}", row.row),
                    &key,
                    Class::Review,
                    props([("object_id", text(external)), ("expires", int(expires)?)]),
                )?;
            }
        }
        tx.commit()?;
        Ok(reply(api::reply::Result::Preview(preview)))
    }

    pub(super) fn workspace_export(
        &self,
        context: &Context,
        export: api::Export,
    ) -> Result<api::Reply> {
        let mut query = export.query.ok_or(Error::Invalid)?;
        query.cursor = None;
        query.limit = MAX_PAGE as u32;
        let mut columns = query.columns.clone();
        if columns.is_empty() {
            columns.push("name".into());
        }
        let mut writer = csv::Writer::from_writer(CsvOutput(Vec::with_capacity(MAX_CSV)));
        let mut headers = vec![
            "asset_id".to_owned(),
            "incarnation".to_owned(),
            "kind".to_owned(),
        ];
        headers.extend(columns.iter().cloned());
        writer.write_record(&headers).map_err(csv_write_error)?;
        let mut basis = None;
        let mut emitted = 0usize;
        loop {
            let page = self.workspace_query(context, query.clone())?;
            if basis.is_some_and(|revision| revision != page.revision) {
                return Err(Error::Expired);
            }
            basis = Some(page.revision);
            for item in page.items {
                emitted += 1;
                if emitted > 10_000 {
                    return Err(Error::Limit);
                }
                let target = item.target.ok_or(Error::Corrupt)?;
                let mut row = vec![
                    target.asset_id,
                    target.incarnation,
                    wire::AssetKind::try_from(item.kind)
                        .map_err(|_| Error::Corrupt)?
                        .as_str_name()
                        .into(),
                ];
                for name in &columns {
                    let cell = match field_value(&item.fields, name) {
                        Some(field::Value::Equipment(v)) => format!(
                            "equipment/claim/v1:{}",
                            hex(&omloom_contracts::equipment::encode(v)
                                .map_err(|_| Error::Corrupt)?)
                        ),
                        Some(field::Value::Text(s))
                            if omloom_contracts::equipment::FIELDS.contains(&name.as_str())
                                && (s.starts_with("equipment/") || s == "<clear>") =>
                        {
                            format!("equipment/text/v1:{}", hex(s.as_bytes()))
                        }
                        None => String::new(),
                        Some(field::Value::Clear(EXPLICIT_CLEAR)) => "<clear>".into(),
                        // OWASP CSV Injection: protect literal text, including control
                        // and full-width prefixes. Typed numbers cannot contain formulas.
                        // https://community.owasp.org/attacks/CSV_Injection
                        Some(field::Value::Text(s))
                            if export.spreadsheet
                                && (s.starts_with(['\t', '\r', '\n'])
                                    || s.trim_start_matches([' ', '\t', '\r', '\n'])
                                        .starts_with([
                                            '=', '+', '-', '@', '＝', '＋', '－', '＠',
                                        ])) =>
                        {
                            format!("'{s}")
                        }
                        Some(field::Value::Text(s)) => s.clone(),
                        Some(field::Value::Integer(n)) => n.to_string(),
                        Some(field::Value::Flag(b)) => b.to_string(),
                        _ => return Err(Error::Corrupt),
                    };
                    row.push(cell);
                }
                writer.write_record(row).map_err(csv_write_error)?;
            }
            match page.next {
                Some(cursor) => query.cursor = Some(cursor),
                None => break,
            }
        }
        let bytes = writer.into_inner().map_err(|_| Error::Limit)?.0;
        if bytes.len() > MAX_CSV {
            return Err(Error::Limit);
        }
        Ok(reply(api::reply::Result::Download(api::Download {
            csv: String::from_utf8(bytes).map_err(|_| Error::Corrupt)?,
            revision: basis.ok_or(Error::Corrupt)?,
        })))
    }
}
