use super::*;

impl Tx<'_> {
    pub(super) fn refresh_source(
        &self,
        context: &Context,
        request: &api::HostImport,
        working: &api::WorkingContext,
    ) -> Result<(String, String)> {
        let refresh = request.refresh.as_ref().ok_or(Error::Invalid)?;
        if refresh.source_ref.len() != 77 || !refresh.source_ref.starts_with("host-mapping/") {
            return Err(Error::Invalid);
        }
        let host = parse_target(refresh.host.as_ref())?;
        let row = self.visible_asset(context, host)?;
        if !matches!(
            wire::AssetKind::try_from(number(&row, "kind")? as i32),
            Ok(wire::AssetKind::Device | wire::AssetKind::VirtualMachine)
        ) {
            return Err(Error::Invalid);
        }
        self.selection(context, working, false)?
            .contains(host, self)?;
        let kind = number(&row, "kind")? as i32;
        if !context.field(host, kind, "identity_evidence", false) {
            return Err(Error::Denied);
        }
        let report = request.report.as_deref().ok_or(Error::Invalid)?;
        let source = self.get(&refresh.source_ref)?.ok_or(Error::Missing)?;
        let name = string(&source, "source")?;
        // Older bindings can be qualified only by their original importer.
        // Once qualified, this attribution stays fixed across permitted refreshers.
        let importer = source
            .get("caller")
            .and_then(Value::as_str)
            .unwrap_or(&context.actor)
            .to_owned();
        if super::super::super::target(&source)? != host
            || string(&source, "parent")? != format!("{}/host-mappings", asset_key(host))
            || refresh.source_ref != mapping(&importer, &name, &report.source_basis, "hardware")
            || (!request.source.is_empty() && request.source != name)
        {
            return Err(Error::Conflict);
        }
        Ok((name, importer))
    }

    pub(super) fn host_parent(
        &self,
        context: &Context,
        request: &api::HostImport,
        name: &str,
        importer: &str,
        resolved: &BTreeMap<&str, AssetRef>,
    ) -> Result<AssetRef> {
        if let Some(target) = resolved.get(name) {
            return Ok(*target);
        }
        if request.refresh.is_none() {
            return Err(Error::Invalid);
        }
        let report = request.report.as_deref().ok_or(Error::Invalid)?;
        let row = self
            .get(&mapping(
                importer,
                &request.source,
                &report.source_basis,
                name,
            ))?
            .ok_or(Error::Conflict)?;
        let target = super::super::super::target(&row)?;
        self.visible_asset(context, target)?;
        Ok(target)
    }

    pub(super) fn refresh_fields(
        &mut self,
        context: &Context,
        target: AssetRef,
        old: &Properties,
        node: &api::HostFinding,
        report: &api::HostReport,
    ) -> Result<(Option<String>, Vec<wire::Field>)> {
        let row = self.visible_asset(context, target)?;
        let kind = number(&row, "kind")? as i32;
        let mut source = old
            .get("authority")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.require_changes(context, target, kind, &node.fields)?;
        for field in &node.fields {
            if !context.field(target, kind, &field.name, false) {
                return Err(Error::Denied);
            }
        }
        // Replaying the original adoption package does not manufacture a second
        // observation of the same collection. New windows retain current claims.
        if number(old, "observed")? == report.observed_at && source.is_none() {
            return Ok((None, Vec::new()));
        }
        let source = source.get_or_insert_with(|| SourceId::generate().to_string());
        let asset = asset_key(target);
        let children = self.children(&asset)?;
        let additional = node
            .fields
            .iter()
            .filter(|field| {
                let key = format!("{asset}/observed/{source}/{}", field.name);
                !children
                    .iter()
                    .any(|row| row.get("key").and_then(Value::as_str) == Some(key.as_str()))
            })
            .count()
            * 2;
        if children.len() + additional > MAX_CHILDREN {
            return Err(Error::Limit);
        }
        let by_key = children
            .iter()
            .map(|row| {
                Ok((
                    row.get("key")
                        .and_then(Value::as_str)
                        .ok_or(Error::Corrupt)?,
                    row,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let observation = wire::Observation {
            evidence: Some(wire::Evidence {
                source_id: source.clone(),
                source_revision: Some(report.boot_basis.clone()),
                observed_at_ms: report.observed_at,
                // A file proves its collection instant, not a future health lease.
                valid_until_ms: report.observed_at,
                fields: node.fields.clone(),
            }),
            scan_id: None,
        };
        let mut previous = Vec::new();
        for field in &node.fields {
            let key = format!("{asset}/observed/{source}/{}", field.name);
            if let Some(row) = by_key.get(key.as_str()) {
                previous.push(super::super::super::inventory::read_field(row)?);
            }
        }
        let update = wire::Asset {
            observed: vec![observation],
            ..Default::default()
        };
        super::super::super::inventory::validate_evidence(&update, &children, &by_key)?;
        self.put_observations(&asset, kind, &update.observed, "observed", false, &by_key)?;
        Ok((Some(source.clone()), previous))
    }
}

impl Store {
    pub(in crate::disk::workspace) fn workspace_host_sources(
        &self,
        context: &Context,
        request: api::HostSources,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        let report = request.report.as_deref().ok_or(Error::Invalid)?;
        validate_parts(report, true)?;
        let host = parse_target(request.host.as_ref())?;
        let tx = self.begin()?;
        let host_row = tx.visible_asset(context, host)?;
        let kind = number(&host_row, "kind")? as i32;
        if !matches!(
            wire::AssetKind::try_from(kind),
            Ok(wire::AssetKind::Device | wire::AssetKind::VirtualMachine)
        ) {
            return Err(Error::Invalid);
        }
        if !context.field(host, kind, "identity_evidence", false) {
            return Err(Error::Denied);
        }
        tx.selection(context, working, false)?.contains(host, &tx)?;
        let scope = api::WorkingContext {
            roots: vec![target_wire(host)],
            ..Default::default()
        };
        let mut sources = Vec::new();
        // HostSourceOptions is Reply field 27, with a two-byte tag. Reserve the
        // largest length prefix before admitting independently bounded sources.
        let mut reply_bytes = 2 + prost::length_delimiter_len(MAX_REPLY_BYTES);
        for row in tx.fact_rows(&format!("{}/host-mappings", asset_key(host)), MAX_IMPORT)? {
            let reference = string(&row, "key")?;
            let name = string(&row, "source")?;
            let importer = row
                .get("caller")
                .and_then(Value::as_str)
                .unwrap_or(&context.actor);
            if reference != mapping(importer, &name, &report.source_basis, "hardware") {
                continue;
            }
            let mut matches = Vec::new();
            let keys = report
                .findings
                .iter()
                .map(|node| text(mapping(importer, &name, &report.source_basis, &node.key)))
                .collect();
            let mapped = tx.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 129",
                params([("keys", Value::Array(keys))]),
            )?;
            let by_key = mapped
                .iter()
                .map(|row| Ok((string(row, "key")?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            let asset_keys = mapped
                .iter()
                .map(|row| super::super::super::target(row).map(|target| text(asset_key(target))))
                .collect::<Result<Vec<_>>>()?;
            let assets = tx.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 129",
                params([("keys", Value::Array(asset_keys))]),
            )?;
            let assets = assets
                .iter()
                .map(|row| Ok((string(row, "key")?, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            let targets = mapped
                .iter()
                .map(super::super::super::target)
                .collect::<Result<Vec<_>>>()?;
            let within_host = tx.owned_focuses(context, &scope.roots, &targets)?;
            for node in &report.findings {
                let Some(mapped) =
                    by_key.get(&mapping(importer, &name, &report.source_basis, &node.key))
                else {
                    continue;
                };
                let target = super::super::super::target(mapped)?;
                let Some(asset) = assets.get(&asset_key(target)).copied() else {
                    continue;
                };
                match tx.require_visible_asset(context, target, asset) {
                    Ok(()) => {}
                    Err(Error::Denied | Error::Missing) => continue,
                    Err(error) => return Err(error),
                }
                if !within_host.contains(&target) {
                    continue;
                }
                if node.kind != wire::AssetKind::Unspecified as i32
                    && number(asset, "kind")? as i32 != node.kind
                {
                    continue;
                }
                let actual = number(asset, "kind")? as i32;
                if actual == wire::AssetKind::Interface as i32
                    && !context.field(target, actual, "current_addresses", true)
                {
                    continue;
                }
                if node.fields.iter().any(|field| {
                    !context.field(target, actual, &field.name, false)
                        || !context.field(target, actual, &field.name, true)
                }) {
                    continue;
                }
                matches.push(api::HostSelection {
                    key: node.key.clone(),
                    target: Some(target_wire(target)),
                    kind: None,
                });
            }
            if matches.is_empty() {
                continue;
            }
            let source = api::HostSource {
                reference,
                label: row
                    .get("namespace")
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .into(),
                matches,
            };
            let size = source.encoded_len();
            reply_bytes += 1 + prost::length_delimiter_len(size) + size;
            if reply_bytes > MAX_REPLY_BYTES {
                return Err(Error::Limit);
            }
            sources.push(source);
        }
        Ok(reply(api::reply::Result::HostSourceOptions(
            api::HostSourceOptions { sources },
        )))
    }
}
