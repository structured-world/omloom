use super::*;

#[derive(Default)]
struct RelationPage {
    relations: Vec<wire::Relation>,
    related: BTreeMap<AssetRef, Properties>,
    next: Option<String>,
}

fn parameter(values: &mut HashMap<String, Value>, value: Value) -> String {
    let name = format!("p{}", values.len());
    values.insert(name.clone(), value);
    format!("${name}")
}

// Use the same typed ownership rules as creation; peer links never form levels.
pub(super) fn ownership_clause(
    reverse_only: Option<bool>,
    child_kinds: &[wire::AssetKind],
) -> String {
    use wire::AssetKind as K;
    let kinds = [
        K::Site,
        K::Location,
        K::Rack,
        K::Device,
        K::VirtualMachine,
        K::SoftwareInstallation,
        K::Service,
        K::Interface,
        K::AddressReference,
        K::Port,
        K::Storage,
        K::ProviderAccount,
        K::SoftwareProduct,
        K::SoftwareRelease,
        K::Workload,
        K::Container,
    ];
    let mut groups = BTreeMap::<(i32, bool, i32), Vec<i32>>::new();
    for parent in kinds {
        for child in kinds {
            if !child_kinds.is_empty() && !child_kinds.contains(&child) {
                continue;
            }
            if let Some((relation, reverse)) = working::attachment(parent, child) {
                if reverse_only.is_some_and(|direction| direction != reverse) {
                    continue;
                }
                groups
                    .entry((relation as i32, reverse, parent as i32))
                    .or_default()
                    .push(child as i32);
            }
        }
    }
    let mut terms = Vec::new();
    for ((relation, reverse, parent), children) in groups {
        let owner = if reverse {
            "r.to_asset=p.asset AND r.to_incarnation=p.incarnation AND r.asset=a.asset AND r.incarnation=a.incarnation"
        } else {
            "r.asset=p.asset AND r.incarnation=p.incarnation AND r.to_asset=a.asset AND r.to_incarnation=a.incarnation"
        };
        terms.push(format!(
            "(p.kind={parent} AND a.kind IN {children:?} AND r.kind={relation} AND {owner})"
        ));
    }
    if terms.is_empty() {
        return "false".into();
    }
    format!("({})", terms.join(" OR "))
}

pub(super) fn ownership_path(children: bool, reverse: bool) -> &'static str {
    match (children, reverse) {
        (true, false) => {
            "(p:OmRow {key:key})-[:OmRelationFrom]->(r:OmRow)-[:OmRelationTo]->(a:OmRow)"
        }
        (true, true) => {
            "(p:OmRow {key:key})<-[:OmRelationTo]-(r:OmRow)<-[:OmRelationFrom]-(a:OmRow)"
        }
        (false, false) => {
            "(a:OmRow {key:key})<-[:OmRelationTo]-(r:OmRow)<-[:OmRelationFrom]-(p:OmRow)"
        }
        (false, true) => {
            "(a:OmRow {key:key})-[:OmRelationFrom]->(r:OmRow)-[:OmRelationTo]->(p:OmRow)"
        }
    }
}

fn browse_clause(
    context: &Context,
    values: &mut HashMap<String, Value>,
    reverse: Option<bool>,
    kinds: &[wire::AssetKind],
) -> String {
    format!(
        "p.parent='assets' AND p.key<>a.key AND p.adoption={} AND {} AND {} AND {}",
        wire::Adoption::Adopted as i32,
        visible_clause_for(context, Some("relations"), values, "p"),
        visible_clause_for(context, Some("relations"), values, "a"),
        ownership_clause(reverse, kinds)
    )
}

impl Tx<'_> {
    fn relation_page(
        &self,
        context: &Context,
        target: AssetRef,
        working: &api::WorkingContext,
        cursor: Option<&str>,
    ) -> Result<RelationPage> {
        let revision = number(&self.root, "revision")?;
        let query = api::Query {
            roots: working.roots.clone(),
            focus: Some(target_wire(target)),
            search: format!("relations/{:?}", working.focus),
            ..Default::default()
        };
        let fingerprint = query_fingerprint(&query, context);
        let mut offset = 0;
        if let Some(cursor) = cursor {
            if cursor.len() > 256 {
                return Err(Error::Invalid);
            }
            let parts: Vec<_> = cursor.split(':').collect();
            if parts.len() != 4
                || parts[0].parse::<u64>().ok() != Some(revision)
                || parts[1] != fingerprint
                || parts[2].parse::<u64>().ok() != Some(context.authority_revision)
            {
                return Err(Error::Expired);
            }
            offset = parts[3].parse::<u64>().map_err(|_| Error::Invalid)?;
            if offset > 100_000 {
                return Err(Error::Limit);
            }
        }
        let mut values = HashMap::new();
        let permitted = visible_clause_for(context, Some("relations"), &mut values, "a");
        let local = if working.focus.is_some() {
            ownership_clause(None, &[])
        } else {
            "true".into()
        };
        values.insert("key".into(), text(asset_key(target)));
        values.insert("offset".into(), int(offset)?);
        let mut rows=self.raw(&format!("MATCH (p:OmRow {{key:$key}})-[:OmRelationFrom|OmRelationTo]-(r:OmRow)-[:OmRelationFrom|OmRelationTo]-(a:OmRow) WHERE a.parent='assets' AND a.key<>p.key AND a.adoption={} AND {permitted} AND {local} RETURN DISTINCT properties(r) AS r, properties(a) AS a ORDER BY r.key SKIP $offset LIMIT 26",wire::Adoption::Adopted as i32),values)?;
        let next = if rows.len() > 25 {
            rows.truncate(25);
            Some(format!(
                "{revision}:{fingerprint}:{}:{}",
                context.authority_revision,
                offset + 25
            ))
        } else {
            None
        };
        let mut relations = Vec::new();
        let mut related = BTreeMap::new();
        for mut row in rows {
            let (Some(Value::Map(r)), Some(Value::Map(a))) = (row.remove("r"), row.remove("a"))
            else {
                return Err(Error::Corrupt);
            };
            related.insert(super::super::target(&a)?, a);
            relations.push(super::super::inventory::relation_projection(&r)?);
        }
        Ok(RelationPage {
            relations,
            related,
            next,
        })
    }
    pub(super) fn verify_owned_focus(
        &self,
        context: &Context,
        roots: &[wire::AssetRef],
        focus: AssetRef,
    ) -> Result<()> {
        let roots = roots
            .iter()
            .map(|r| parse_target(Some(r)).map(asset_key))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut seen = BTreeSet::new();
        let mut frontier = BTreeSet::from([asset_key(focus)]);
        while !frontier.is_empty() {
            if frontier.iter().any(|key| roots.contains(key)) {
                return Ok(());
            }
            let mut next = BTreeSet::new();
            for reverse in [false, true] {
                let mut values = HashMap::new();
                let owner = browse_clause(context, &mut values, Some(reverse), &[]);
                values.insert(
                    "keys".into(),
                    Value::Array(frontier.iter().map(text).collect()),
                );
                let path = ownership_path(false, reverse);
                let rows=self.raw(&format!("UNWIND $keys AS key MATCH {path} WHERE {owner} RETURN DISTINCT p.key AS key LIMIT 1025"),values)?;
                for row in rows {
                    next.insert(string(&row, "key")?);
                }
                if next.len() > 1024 {
                    return Err(Error::Limit);
                }
            }
            seen.extend(frontier);
            frontier = next;
            frontier.retain(|key| !seen.contains(key));
            if seen.len() + frontier.len() > 1024 {
                return Err(Error::Limit);
            }
        }
        Err(Error::Denied)
    }
}
fn scope_clause(scope: &Scope, values: &mut HashMap<String, Value>, alias: &str) -> String {
    match scope {
        Scope::All => "true".into(),
        Scope::Kinds(kinds) => format!(
            "{alias}.kind IN {}",
            parameter(
                values,
                Value::Array(kinds.iter().map(|k| Value::Int(*k as i32 as i64)).collect())
            )
        ),
        Scope::Targets(targets) => {
            let clauses: Vec<_> = targets
                .iter()
                .map(|t| {
                    format!(
                        "({alias}.key={} AND {alias}.incarnation={})",
                        parameter(values, text(asset_key(*t))),
                        parameter(values, text(t.incarnation.to_string()))
                    )
                })
                .collect();
            if clauses.is_empty() {
                "false".into()
            } else {
                format!("({})", clauses.join(" OR "))
            }
        }
    }
}
fn visible_clause(
    context: &Context,
    field: Option<&str>,
    values: &mut HashMap<String, Value>,
) -> String {
    visible_clause_for(context, field, values, "a")
}
pub(super) fn visible_clause_for(
    context: &Context,
    field: Option<&str>,
    values: &mut HashMap<String, Value>,
    alias: &str,
) -> String {
    let visible = visible_clause_raw(context, field, values, alias);
    if field != Some("name") || visible == "false" {
        return visible;
    }
    let address = visible_clause_raw(context, Some("address"), values, alias);
    if address == "true" {
        return visible;
    }
    format!(
        "({visible} AND ({alias}.kind<>{} OR {address}))",
        wire::AssetKind::AddressReference as i32
    )
}
fn visible_clause_raw(
    context: &Context,
    field: Option<&str>,
    values: &mut HashMap<String, Value>,
    alias: &str,
) -> String {
    if context
        .grants
        .iter()
        .any(|g| field.is_none_or(|f| g.read.contains(f)) && matches!(g.scope, Scope::All))
    {
        return "true".into();
    }
    let clauses: Vec<_> = context
        .grants
        .iter()
        .filter(|g| field.is_none_or(|f| g.read.contains(f)))
        .map(|g| scope_clause(&g.scope, values, alias))
        .collect();
    if clauses.is_empty() {
        "false".into()
    } else {
        format!("({})", clauses.join(" OR "))
    }
}
pub(super) fn kind_for_view(view: api::View) -> &'static [wire::AssetKind] {
    use wire::AssetKind as K;
    match view {
        api::View::Overview => &[],
        api::View::Hardware => &[
            K::Device,
            K::Port,
            K::Cable,
            K::Rack,
            K::Site,
            K::Location,
            K::PowerFeed,
        ],
        api::View::Network => &[
            K::Interface,
            K::IpAddress,
            K::Prefix,
            K::Vlan,
            K::Vrf,
            K::AddressSpace,
            K::L2Domain,
            K::Port,
            K::Cable,
            K::Lag,
            K::AddressGroup,
            K::AddressReference,
        ],
        api::View::Virtualization => &[K::VirtualMachine, K::SoftwareInstallation, K::Cluster],
        api::View::Services => &[
            K::Service,
            K::SoftwareInstallation,
            K::Storage,
            K::Domain,
            K::ProviderAccount,
            K::SoftwareProduct,
            K::SoftwareRelease,
            K::Workload,
            K::Container,
        ],
    }
}
fn value_clause(
    value: &wire::Field,
    alias: &str,
    values: &mut HashMap<String, Value>,
) -> Result<String> {
    let (kind, column, value) = match value.value.as_ref().ok_or(Error::Invalid)? {
        field::Value::Text(s) => (1, "vtext", text(s)),
        field::Value::Integer(n) => (2, "vint", Value::Int(*n)),
        field::Value::Flag(b) => (3, "vflag", Value::Bool(*b)),
        field::Value::Clear(EXPLICIT_CLEAR) => return Ok(format!("{alias}.vtype=4")),
        _ => return Err(Error::Invalid),
    };
    Ok(format!(
        "{alias}.vtype={kind} AND {alias}.{column}={}",
        parameter(values, value)
    ))
}

impl Store {
    pub(super) fn workspace_query(
        &self,
        context: &Context,
        mut query: api::Query,
    ) -> Result<api::Page> {
        if query.search.len() > MAX_QUERY
            || query.roots.len() > 128
            || query.kinds.len() > 32
            || query.columns.len() > MAX_FIELDS
            || query.predicates.len() > 8
            || query.limit == 0
            || query.limit as usize > MAX_PAGE
        {
            return Err(Error::Invalid);
        }
        let view = api::View::try_from(query.view).map_err(|_| Error::Invalid)?;
        if let Some(filter) = &query.responsibility {
            super::responsibility::validate_filter(filter)?;
        }
        let view_kinds = kind_for_view(view);
        let basis = self.usage()?.revision;
        if query.columns.is_empty() {
            query.columns.push("name".into());
        }
        let definitions = self.workspace_definitions(context)?;
        for field in &query.columns {
            validate_column(field, &definitions)?;
        }
        let tx = self.begin()?;
        let revision = number(&tx.root, "revision")?;
        if revision != basis {
            return Err(Error::Expired);
        }
        let fingerprint = query_fingerprint(&query, context);
        let offset = match &query.cursor {
            None => 0u64,
            Some(cursor) => {
                if cursor.len() > 256 {
                    return Err(Error::Invalid);
                }
                let parts: Vec<_> = cursor.split(':').collect();
                if parts.len() != 4
                    || parts[0].parse::<u64>().ok() != Some(revision)
                    || parts[1] != fingerprint
                    || parts[2].parse::<u64>().ok() != Some(context.authority_revision)
                {
                    return Err(Error::Expired);
                }
                parts[3].parse().map_err(|_| Error::Invalid)?
            }
        };
        if offset > 100_000 {
            return Err(Error::Limit);
        }
        let mut values = HashMap::new();
        let mut clauses = vec![
            "a.parent='assets'".to_owned(),
            visible_clause(context, None, &mut values),
        ];
        clauses.push(format!(
            "a.adoption={}",
            parameter(
                &mut values,
                Value::Int(wire::Adoption::Adopted as i32 as i64)
            )
        ));
        let mut entry = "MATCH (a:OmRow {parent:'assets'})".to_owned();
        let mut bindings = vec!["a".to_owned()];
        let mut predicate_joins = Vec::new();
        if query.omit_host_local
            && (view_kinds.is_empty()
                || view_kinds.iter().any(|kind| {
                    matches!(
                        kind,
                        wire::AssetKind::IpAddress | wire::AssetKind::AddressReference
                    )
                }))
            && (query.kinds.is_empty()
                || query.kinds.iter().any(|kind| {
                    [
                        wire::AssetKind::IpAddress as i32,
                        wire::AssetKind::AddressReference as i32,
                    ]
                    .contains(kind)
                }))
        {
            predicate_joins.push(" OPTIONAL MATCH (local_ip:OmRow) WHERE local_ip.key='network/'+a.asset+'/'+a.incarnation".into());
            bindings.push("local_ip".into());
            let address = "(CASE WHEN local_ip.address IS NOT NULL THEN local_ip.address WHEN local_ip.cidr IS NOT NULL THEN local_ip.cidr ELSE '' END)";
            let mut local: Vec<_> = [
                "127.",
                "169.254.",
                "fe8",
                "fe9",
                "fea",
                "feb",
                "::ffff:127.",
                "::ffff:169.254.",
            ]
            .iter()
            .map(|prefix| format!("{address} STARTS WITH '{}'", prefix))
            .collect();
            local.extend(
                ["::1", "::1/128", "0.0.0.0", "0.0.0.0/32", "::", "::/128"]
                    .iter()
                    .map(|value| format!("{address}='{value}'")),
            );
            clauses.push(format!(
                "(NOT (a.kind IN [{},{}]) OR NOT ({}))",
                wire::AssetKind::AddressReference as i32,
                wire::AssetKind::IpAddress as i32,
                local.join(" OR ")
            ));
        }
        let mut network_scope = false;
        if view == api::View::Virtualization && query.kinds.is_empty() {
            clauses.push(format!("a.kind<>{}", wire::AssetKind::Cluster as i32));
        }
        if !view_kinds.is_empty() {
            clauses.push(format!(
                "a.kind IN {}",
                parameter(
                    &mut values,
                    Value::Array(
                        view_kinds
                            .iter()
                            .map(|k| Value::Int(*k as i32 as i64))
                            .collect()
                    )
                )
            ));
        }
        if view == api::View::Virtualization {
            let role_visible = visible_clause(context, Some("virtualization_role"), &mut values);
            clauses.push(format!(
                "(a.kind<>{} OR (a.virtualization_role={} AND {role_visible}))",
                wire::AssetKind::SoftwareInstallation as i32,
                parameter(
                    &mut values,
                    text(api::VirtualizationRole::Hypervisor.as_str_name())
                )
            ));
        }
        if !query.kinds.is_empty() {
            for kind in &query.kinds {
                if wire::AssetKind::try_from(*kind)
                    .ok()
                    .filter(|k| *k != wire::AssetKind::Unspecified)
                    .is_none()
                {
                    return Err(Error::Invalid);
                }
            }
            clauses.push(format!(
                "a.kind IN {}",
                parameter(
                    &mut values,
                    Value::Array(
                        query
                            .kinds
                            .iter()
                            .map(|n| Value::Int((*n).into()))
                            .collect()
                    )
                )
            ));
        }
        let working = api::WorkingContext {
            roots: query.roots.clone(),
            focus: query.focus.clone(),
            expected: None,
        };
        let browse_roots: Vec<_> = query
            .focus
            .iter()
            .chain(query.roots.iter().filter(|_| query.focus.is_none()))
            .cloned()
            .collect();
        let mut level_only = query.level_only || query.focus.is_some();
        for root in &browse_roots {
            let row = tx.visible_asset(context, parse_target(Some(root))?)?;
            if [
                wire::AssetKind::AddressSpace as u64,
                wire::AssetKind::Prefix as u64,
                wire::AssetKind::L2Domain as u64,
            ]
            .contains(&number(&row, "kind")?)
            {
                // IPAM containment is an indexed namespace/range selection, not ownership.
                level_only = false;
            }
        }
        if level_only {
            if query.focus.is_some() && !query.roots.is_empty() {
                tx.selection(context, &working, false)?;
            }
            let child_kinds = if query.kinds.is_empty() {
                view_kinds.to_vec()
            } else {
                query
                    .kinds
                    .iter()
                    .map(|kind| wire::AssetKind::try_from(*kind).map_err(|_| Error::Invalid))
                    .collect::<Result<Vec<_>>>()?
            };
            if browse_roots.is_empty() {
                predicate_joins.push(format!(
                    " WITH {} WHERE {}",
                    bindings.join(", "),
                    clauses.join(" AND ")
                ));
                clauses.clear();
                // Probe indexed relation endpoints directly; repeating a resource
                // probe adds a join before reaching the same adjacent relation.
                for (reverse, path) in [
                    (
                        false,
                        "(r:OmRow {to_asset:a.asset})<-[:OmRelationFrom]-(p:OmRow)",
                    ),
                    (true, "(r:OmRow {asset:a.asset})-[:OmRelationTo]->(p:OmRow)"),
                ] {
                    let mut owner =
                        browse_clause(context, &mut values, Some(reverse), &child_kinds);
                    if view == api::View::Virtualization {
                        // Hardware groups this view, but cannot hide installations or direct VM placements.
                        owner.push_str(&format!(
                            " AND NOT (a.kind IN [{},{}] AND p.kind={})",
                            wire::AssetKind::SoftwareInstallation as i32,
                            wire::AssetKind::VirtualMachine as i32,
                            wire::AssetKind::Device as i32,
                        ));
                        let visible = visible_clause_for(
                            context,
                            Some("virtualization_role"),
                            &mut values,
                            "p",
                        );
                        let enabled = parameter(
                            &mut values,
                            text(api::VirtualizationRole::Hypervisor.as_str_name()),
                        );
                        // Only a role-visible enabled installation can group guests in this view.
                        owner.push_str(&format!(
                            " AND (a.kind<>{} OR p.kind<>{} OR ({visible} AND p.virtualization_role={enabled}))",
                            wire::AssetKind::VirtualMachine as i32,
                            wire::AssetKind::SoftwareInstallation as i32,
                        ));
                    }
                    let flag = if reverse {
                        "reverse_owned"
                    } else {
                        "forward_owned"
                    };
                    predicate_joins.push(format!(
                        " OPTIONAL MATCH {path} WHERE {owner} WITH {}, p IS NOT NULL AS {flag}",
                        bindings.join(", ")
                    ));
                    if reverse {
                        bindings.push(flag.into());
                        clauses.push(format!("NOT {flag}"));
                    } else {
                        // Owned components cannot become roots in the reverse
                        // branch. Reject them before performing another probe.
                        predicate_joins.push(format!(" WHERE NOT {flag}"));
                    }
                }
            } else {
                let owner = browse_clause(context, &mut values, None, &child_kinds);
                let keys = browse_roots
                    .iter()
                    .map(|r| parse_target(Some(r)).map(|t| text(asset_key(t))))
                    .collect::<Result<Vec<_>>>()?;
                let keys = parameter(&mut values, Value::Array(keys));
                entry = format!(
                    "UNWIND {keys} AS root_key MATCH (p:OmRow {{key:root_key}})-[:OmRelationFrom|OmRelationTo]-(r:OmRow)-[:OmRelationFrom|OmRelationTo]-(a:OmRow)"
                );
                bindings.extend(["p".into(), "r".into()]);
                clauses.push(owner);
            }
        }
        let unscoped = api::WorkingContext::default();
        let mut selection = tx.selection(
            context,
            if level_only { &unscoped } else { &working },
            false,
        )?;
        if !level_only && query.focus.is_none() && !working.roots.is_empty() {
            selection.members = Some(tx.working_scope(context, &working.roots)?);
        }
        if let Some(roots) = &selection.members {
            let mut selected = vec![format!(
                "a.key IN {}",
                parameter(
                    &mut values,
                    Value::Array(roots.iter().map(|t| text(asset_key(*t))).collect())
                )
            )];
            let mut networks = Vec::new();
            let namespace_visible = visible_clause(context, Some("address_space"), &mut values);
            for scope in &selection.networks {
                use working::NetworkScope as N;
                let space = match scope {
                    N::Space(space) | N::Prefix { space, .. } => *space,
                    N::Link(link) => {
                        network_scope = true;
                        let visible = visible_clause(context, Some("link_scope"), &mut values);
                        let link = parameter(&mut values, text(network::identity(*link)));
                        selected.push(format!(
                            "(a.kind={} AND {visible} AND ns.link_scope={link})",
                            wire::AssetKind::Vlan as i32
                        ));
                        continue;
                    }
                };
                let mut terms = vec![format!(
                    "ns.address_space={}",
                    parameter(&mut values, text(network::identity(space)))
                )];
                if let N::Prefix { block, link, .. } = scope {
                    terms.push(visible_clause(context, Some("cidr"), &mut values));
                    terms.push(format!(
                        "ns.ip_order>={} AND ns.ip_order<={}",
                        parameter(&mut values, text(network::address_order(block.network()))),
                        parameter(&mut values, text(network::address_order(block.broadcast())))
                    ));
                    terms.push(format!(
                        "(a.kind<>{} OR ns.prefix_len>={})",
                        wire::AssetKind::Prefix as i32,
                        block.prefix_len()
                    ));
                    if let Some(link) = link {
                        terms.push(visible_clause(context, Some("link_scope"), &mut values));
                        terms.push(format!(
                            "ns.link_scope={}",
                            parameter(&mut values, text(network::identity(*link)))
                        ));
                    }
                }
                networks.push(format!("({})", terms.join(" AND ")));
            }
            if !networks.is_empty() {
                network_scope = true;
                selected.push(format!(
                    "(a.kind IN [{},{}] AND {namespace_visible} AND ({}))",
                    wire::AssetKind::IpAddress as i32,
                    wire::AssetKind::Prefix as i32,
                    networks.join(" OR ")
                ));
            }
            clauses.push(format!("({})", selected.join(" OR ")));
        }
        if !query.search.trim().is_empty() {
            let term = query.search.trim().to_lowercase();
            if term.parse::<AssetId>().is_ok() {
                clauses.push(format!("a.asset={}", parameter(&mut values, text(&term))));
            } else if let Ok(ip) = term.parse::<std::net::IpAddr>() {
                let literal = parameter(&mut values, text(ip.to_string()));
                let cidr = parameter(&mut values, text(format!("{ip}/")));
                if level_only {
                    predicate_joins.push(
                        " MATCH (ip:OmRow) WHERE ip.key='network/'+a.asset+'/'+a.incarnation"
                            .into(),
                    );
                    clauses.push(format!(
                        "(ip.cidr STARTS WITH {cidr} OR ip.address={literal})"
                    ));
                } else {
                    entry = format!(
                        "MATCH (ip:OmRow {{parent:'networks'}}) WHERE ip.cidr STARTS WITH {cidr} OR ip.address={literal} MATCH (a:OmRow) WHERE a.key='asset/'+ip.asset"
                    );
                }
                clauses.push("ip.incarnation=a.incarnation".into());
                bindings.push("ip".into());
                let canonical = visible_clause(context, Some("cidr"), &mut values);
                let reference = visible_clause(context, Some("address"), &mut values);
                clauses.push(format!(
                    "((a.kind={} AND {reference}) OR (a.kind<>{} AND {canonical}))",
                    wire::AssetKind::AddressReference as i32,
                    wire::AssetKind::AddressReference as i32
                ));
            } else {
                let mut searchable = vec![
                    "name".to_owned(),
                    "description".to_owned(),
                    "serial".to_owned(),
                ];
                searchable.extend(
                    definitions
                        .iter()
                        .filter(|d| d.searchable && !d.retired)
                        .map(|d| d.id.clone()),
                );
                let mut value_matches = vec![format!(
                    "(CASE WHEN s.vtype=1 THEN toLower(s.vtext) CONTAINS {} ELSE false END)",
                    parameter(&mut values, text(&term))
                )];
                if let Ok(integer) = term.parse::<i64>() {
                    value_matches.push(format!(
                        "(CASE WHEN s.vtype=2 THEN s.vint={} ELSE false END)",
                        parameter(&mut values, Value::Int(integer))
                    ));
                }
                if term == "true" || term == "false" {
                    value_matches.push(format!(
                        "(CASE WHEN s.vtype=3 THEN s.vflag={} ELSE false END)",
                        parameter(&mut values, Value::Bool(term == "true"))
                    ));
                }
                let fields = parameter(&mut values, strings(&searchable));
                let matches = value_matches.join(" OR ");
                if level_only {
                    predicate_joins.push(format!(" MATCH (s:OmRow) WHERE s.parent=a.key AND s.layer='intent' AND s.field IN {fields} AND ({matches})"));
                } else {
                    entry = format!(
                        "MATCH (s:OmRow {{layer:'intent'}}) WHERE s.field IN {fields} AND ({matches}) MATCH (a:OmRow) WHERE a.key=s.parent"
                    );
                }
                bindings.push("s".into());
                let search_scopes: Vec<_> = searchable
                    .iter()
                    .filter_map(|f| {
                        let allowed = visible_clause(context, Some(f), &mut values);
                        if allowed == "false" {
                            return None;
                        }
                        let field = format!("s.field={}", parameter(&mut values, text(f)));
                        Some(if allowed == "true" {
                            field
                        } else {
                            format!("({field} AND {allowed})")
                        })
                    })
                    .collect();
                clauses.push(if search_scopes.is_empty() {
                    "false".into()
                } else {
                    format!("({})", search_scopes.join(" OR "))
                });
            }
        }
        if network_scope {
            predicate_joins.push(format!(" WITH {}, 'network/'+a.asset+'/'+a.incarnation AS scope_key OPTIONAL MATCH (ns:OmRow) WHERE ns.key=scope_key",bindings.join(", ")));
            bindings.push("ns".into());
        }
        if query.unplaced {
            predicate_joins.push(format!(
                " WITH {} WHERE {}",
                bindings.join(", "),
                clauses.join(" AND ")
            ));
            clauses.clear();
            // Preserve search and namespace bindings before the indexed placement probes.
            for (path, kinds, flag) in [
                (
                    "(probe:OmRow {key:a.key})-[:OmRelationFrom]->(placement:OmRow)",
                    &[wire::RelationKind::InstalledIn as i32][..],
                    "installed",
                ),
                (
                    "(probe:OmRow {key:a.key})<-[:OmRelationTo]-(placement:OmRow)",
                    &[
                        wire::RelationKind::Contains as i32,
                        wire::RelationKind::HasPort as i32,
                    ][..],
                    "contained",
                ),
            ] {
                predicate_joins.push(format!(" OPTIONAL MATCH {path} WHERE placement.kind IN {kinds:?} WITH {}, placement IS NOT NULL AS {flag}", bindings.join(", ")));
                bindings.push(flag.into());
                clauses.push(format!("NOT {flag}"));
            }
        }
        // Separate MATCH clauses make the key equality an indexed correlated
        // lookup. A comma-separated pattern materializes a Cartesian product.
        for (index, predicate) in query.predicates.iter().enumerate() {
            validate_predicate(predicate, &definitions)?;
            if predicate.field == "name"
                && let Some(field::Value::Text(name)) = predicate
                    .value
                    .as_ref()
                    .and_then(|value| value.value.as_ref())
            {
                // Display/sort names already live on the root, including derived
                // address labels that have no intent/name row.
                clauses.push(visible_clause(context, Some("name"), &mut values));
                clauses.push(format!("a.name={}", parameter(&mut values, text(name))));
                continue;
            }
            let alias = format!("f{index}");
            let network = network_column(&predicate.field);
            let lookup = if network {
                format!(
                    "CASE WHEN a.kind={} THEN 'group/'+a.asset+'/'+a.incarnation ELSE 'network/'+a.asset+'/'+a.incarnation END",
                    wire::AssetKind::AddressGroup as i32
                )
            } else {
                format!(
                    "a.key+'/intent/'+{}",
                    parameter(&mut values, text(&predicate.field))
                )
            };
            // Bind conditional keys before point MATCH so every branch uses
            // the same correlated indexed access path.
            if network {
                predicate_joins.push(format!(" WITH {}, {lookup} AS nk{index} MATCH ({alias}:OmRow) WHERE {alias}.key=nk{index}", bindings.join(", ")));
            } else {
                predicate_joins.push(format!(" MATCH ({alias}:OmRow) WHERE {alias}.key={lookup}"));
            }
            bindings.push(alias.clone());
            let value_alias = if predicate.field == "vrid" {
                let group_alias = format!("g{index}");
                let root_alias = format!("ga{index}");
                predicate_joins.push(format!(" WITH {}, CASE WHEN a.kind={} THEN {alias}.key ELSE 'group/'+{alias}.group END AS gk{index} MATCH ({group_alias}:OmRow) WHERE {group_alias}.key=gk{index} MATCH ({root_alias}:OmRow) WHERE {root_alias}.key='asset/'+{group_alias}.asset", bindings.join(", "), wire::AssetKind::AddressGroup as i32));
                bindings.extend([group_alias.clone(), root_alias.clone()]);
                clauses.push(format!("{root_alias}.incarnation={group_alias}.incarnation AND {root_alias}.adoption={}", wire::Adoption::Adopted as i32));
                clauses.push(visible_clause_for(
                    context,
                    Some("vrid"),
                    &mut values,
                    &root_alias,
                ));
                group_alias
            } else {
                alias.clone()
            };
            clauses.push(visible_clause(context, Some(&predicate.field), &mut values));
            let value = predicate.value.as_ref().ok_or(Error::Invalid)?;
            if network {
                let operand = match value.value.as_ref() {
                    Some(field::Value::Text(s)) => text(s),
                    Some(field::Value::Integer(n)) => Value::Int(*n),
                    _ => return Err(Error::Invalid),
                };
                clauses.push(format!(
                    "{value_alias}.{}={}",
                    predicate.field,
                    parameter(&mut values, operand)
                ));
            } else {
                clauses.push(format!("{alias}.layer='intent'"));
                clauses.push(value_clause(value, &alias, &mut values)?);
            }
        }
        if let Some(filter) = &query.responsibility {
            let keys = if query.roots.is_empty()
                && query.focus.is_none()
                && query.search.trim().is_empty()
                && query.predicates.is_empty()
                && query.kinds.is_empty()
                && view == api::View::Overview
            {
                tx.responsibility_filter(context, filter)?
            } else {
                let scoped = format!(
                    "{entry}{} WITH {} WHERE {} RETURN DISTINCT {{asset:a.asset,incarnation:a.incarnation,kind:a.kind,adoption:a.adoption}} AS p LIMIT 4097",
                    predicate_joins.join(""),
                    bindings.join(", "),
                    clauses.join(" AND ")
                );
                let rows = tx.query(&scoped, Some(values.clone()))?;
                tx.responsibility_filter_rows(context, filter, rows)?
            };
            let keys = parameter(
                &mut values,
                Value::Array(keys.into_iter().map(text).collect()),
            );
            if entry == "MATCH (a:OmRow {parent:'assets'})" {
                entry = format!(
                    "UNWIND {keys} AS responsibility_key MATCH (a:OmRow {{key:responsibility_key}})"
                );
            } else {
                clauses.push(format!("a.key IN {keys}"));
            }
        }
        clauses.retain(|clause| clause != "true");
        let base = format!(
            "{entry}{} WITH {} WHERE {}",
            predicate_joins.join(""),
            bindings.join(", "),
            clauses.join(" AND ")
        );
        let count = tx.raw(
            &format!("{base} RETURN count(DISTINCT a.key) AS total"),
            values.clone(),
        )?;
        let total = count
            .first()
            .and_then(|r| r.get("total"))
            .and_then(Value::as_int)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(Error::Corrupt)?;
        let sort = match query.sort.as_str() {
            "" | "name" => format!(
                "CASE WHEN {} THEN a.name_sort ELSE a.asset END",
                visible_clause(context, Some("name"), &mut values)
            ),
            "kind" => "a.kind".into(),
            _ => return Err(Error::Invalid),
        };
        let direction = if query.descending { "DESC" } else { "ASC" };
        let limit = query.limit;
        values.insert("offset".into(), int(offset)?);
        values.insert("limit".into(), int(u64::from(limit))?);
        let selected = tx.raw(&format!("{base} RETURN DISTINCT a.key AS key, {sort} AS ordering ORDER BY ordering {direction}, key ASC SKIP $offset LIMIT $limit"), values)?;
        let keys: Vec<_> = selected
            .iter()
            .map(|p| string(p, "key").map(text))
            .collect::<Result<_>>()?;
        let fetched = tx.query(
            "UNWIND $keys AS key MATCH (a:OmRow) WHERE a.key=key RETURN properties(a) AS p LIMIT $ceiling",
            params([("keys", Value::Array(keys.clone())), ("ceiling", Value::Int((MAX_PAGE + 1) as i64))]),
        )?;
        let mut by_key = fetched
            .into_iter()
            .map(|row| Ok((string(&row, "key")?, row)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let rows = selected
            .iter()
            .map(|item| by_key.remove(&string(item, "key")?).ok_or(Error::Corrupt))
            .collect::<Result<Vec<_>>>()?;
        let fields = tx.query("UNWIND $keys AS owner UNWIND $fields AS name MATCH (f:OmRow) WHERE f.key=owner+'/intent/'+name RETURN properties(f) AS p LIMIT $ceiling", params([
            ("keys", Value::Array(keys)), ("fields", strings(&query.columns)),
            ("ceiling", Value::Int((MAX_PAGE * MAX_FIELDS + 1) as i64)),
        ]))?;
        if fields.len() > MAX_PAGE * MAX_FIELDS {
            return Err(Error::Limit);
        }
        let mut grouped: BTreeMap<String, Vec<wire::Field>> = BTreeMap::new();
        for f in &fields {
            let key = f
                .get("parent")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            grouped
                .entry(key.to_owned())
                .or_default()
                .push(super::super::inventory::read_field(f)?);
        }
        let network_keys = rows
            .iter()
            .map(|p| {
                target(p).and_then(|t| {
                    Ok(text(format!(
                        "{}/{}",
                        if number(p, "kind")? == wire::AssetKind::AddressGroup as u64 {
                            "group"
                        } else {
                            "network"
                        },
                        network::identity(t)
                    )))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let networks = tx.query(
            "UNWIND $keys AS key MATCH (n:OmRow) WHERE n.key=key RETURN properties(n) AS p LIMIT $ceiling",
            params([("keys", Value::Array(network_keys)), ("ceiling", Value::Int((MAX_PAGE + 1) as i64))]),
        )?;
        let mut group_metadata = BTreeMap::new();
        if query.columns.iter().any(|field| field == "vrid") {
            let group_refs = networks
                .iter()
                .map(network::groups::group_of)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .filter(|t| {
                    context.readable(*t, wire::AssetKind::AddressGroup as i32)
                        && context.field(*t, wire::AssetKind::AddressGroup as i32, "vrid", false)
                })
                .map(|t| text(network::identity(t)))
                .collect::<Vec<_>>();
            if !group_refs.is_empty() {
                let settings = tx.query("UNWIND $groups AS selected MATCH (g:OmRow) WHERE g.key='group/'+selected MATCH (a:OmRow) WHERE a.key='asset/'+g.asset WITH a,g WHERE a.incarnation=g.incarnation AND a.adoption=$adopted RETURN properties(g) AS p LIMIT $ceiling", params([("groups", Value::Array(group_refs)), ("adopted", Value::Int(wire::Adoption::Adopted as i32 as i64)), ("ceiling", Value::Int((MAX_PAGE + 1) as i64))]))?;
                for row in settings {
                    group_metadata.insert(target(&row)?, row);
                }
            }
        }
        for row in &networks {
            let owner = target(row)?;
            let key = asset_key(owner);
            let mut values = addresses::visible_projection(context, row, &query.columns)?;
            if query.columns.iter().any(|field| field == "vrid")
                && let Some(group) = network::groups::group_of(row)?
                && let Some(settings) = group_metadata.get(&group)
            {
                values.extend(network::projection(settings, &["vrid".into()])?);
            }
            if let Some(root) = rows.iter().find(|p| p.get("key") == Some(&text(&key))) {
                let kind = number(root, "kind")? as i32;
                for value in values {
                    if context.field(owner, kind, &value.name, false) {
                        grouped.entry(key.clone()).or_default().push(value);
                    }
                }
            }
        }
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            let target = target(row)?;
            let kind = number(row, "kind")? as i32;
            let key = row
                .get("key")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            let fields = grouped
                .remove(key)
                .unwrap_or_default()
                .into_iter()
                .filter(|f| context.field(target, kind, &f.name, false))
                .collect();
            items.push(api::Item {
                target: Some(target_wire(target)),
                kind,
                revision: number(row, "revision")?,
                fields,
            });
        }
        let mut contexts = if query.include_context {
            let mut hints = tx.item_contexts(context, &rows, &query)?;
            tx.workload_ownership(context, &rows, &mut hints)?;
            hints
        } else {
            Vec::new()
        };
        // Reserve the longest continuation and the Reply envelope once. Each
        // item/context is measured without encoding or copying its field payload.
        let mut bytes = api::Page {
            total,
            revision,
            ..Default::default()
        }
        .encoded_len();
        // Three decimal u64 components and separators bound the cursor without
        // allocating a placeholder on every page, including terminal pages.
        let cursor_bytes = 3 * 20 + 3 + fingerprint.len();
        bytes += 1 + prost::length_delimiter_len(cursor_bytes) + cursor_bytes;
        let envelope = 1 + prost::length_delimiter_len(MAX_REPLY_BYTES);
        let mut emitted = 0;
        for (index, item) in items.iter().enumerate() {
            let size = item.encoded_len();
            let mut addition = 1 + prost::length_delimiter_len(size) + size;
            if query.include_context {
                let size = contexts.get(index).ok_or(Error::Corrupt)?.encoded_len();
                addition += 1 + prost::length_delimiter_len(size) + size;
            }
            if bytes + addition > MAX_REPLY_BYTES - envelope {
                break;
            }
            bytes += addition;
            emitted += 1;
        }
        if emitted == 0 && !items.is_empty() {
            return Err(Error::Limit);
        }
        items.truncate(emitted);
        if query.include_context {
            contexts.truncate(emitted);
        }
        tx.commit()?;
        if self.usage()?.revision != revision {
            return Err(Error::Expired);
        }
        let next_offset = offset.checked_add(emitted as u64).ok_or(Error::Limit)?;
        Ok(api::Page {
            items,
            contexts,
            total,
            revision,
            next: if next_offset < total {
                Some(format!(
                    "{revision}:{fingerprint}:{}:{next_offset}",
                    context.authority_revision
                ))
            } else {
                None
            },
        })
    }

    pub(super) fn workspace_read(
        &self,
        context: &Context,
        read: api::Read,
        working: &api::WorkingContext,
    ) -> Result<api::Detail> {
        let basis = self.usage()?.revision;
        let target = parse_target(read.target.as_ref())?;
        let tx = self.begin()?;
        let selection = tx.selection(context, working, false)?;
        selection.contains(target, &tx)?;
        if read.network_cursor.is_some() {
            if read.relations_cursor.is_some() {
                return Err(Error::Invalid);
            }
            let row = tx.visible_asset(context, target)?;
            let mut detail = api::Detail::default();
            tx.read_network_page(
                context,
                target,
                number(&row, "kind")? as i32,
                working,
                read.network_cursor.as_deref(),
                &mut detail,
            )?;
            tx.commit()?;
            if self.usage()?.revision != basis {
                return Err(Error::Expired);
            }
            return Ok(detail);
        }
        let asset = self.asset(target)?;
        if asset.adoption != wire::Adoption::Adopted as i32 {
            return Err(Error::Missing);
        }
        if !context.readable(target, asset.kind) {
            return Err(Error::Denied);
        }
        let mut fields: Vec<_> = asset
            .intended
            .into_iter()
            .filter_map(|f| f.field)
            .filter(|f| context.field(target, asset.kind, &f.name, false))
            .collect();
        let mut relations = Vec::new();
        let mut related_keys = BTreeSet::new();
        let RelationPage {
            relations: candidates,
            related: adjacent,
            next: relations_next,
        } = if context.field(target, asset.kind, "relations", false) {
            tx.relation_page(context, target, working, read.relations_cursor.as_deref())?
        } else {
            RelationPage::default()
        };
        for relation in candidates {
            let from = parse_target(relation.from.as_ref())?;
            let to = parse_target(relation.to.as_ref())?;
            let other = if from == target { to } else { from };
            let row = adjacent.get(&other).ok_or(Error::Corrupt)?;
            let other_kind = wire::AssetKind::try_from(number(row, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?;
            if number(row, "adoption")? != wire::Adoption::Adopted as u64
                || !context.readable(other, other_kind as i32)
                || !context.field(other, other_kind as i32, "relations", false)
            {
                continue;
            }
            let local = working::refines(
                target,
                wire::AssetKind::try_from(asset.kind).map_err(|_| Error::Corrupt)?,
                other_kind,
                relation.kind,
                from,
                to,
            );
            if !local {
                if working.focus.is_some() {
                    continue;
                }
                match selection.contains(other, &tx) {
                    Ok(()) => {}
                    Err(Error::Denied) => continue,
                    Err(error) => return Err(error),
                }
            }
            related_keys.insert(other);
            relations.push(relation);
        }
        let mut related = Vec::with_capacity(related_keys.len());
        for (target, row) in adjacent {
            if !related_keys.contains(&target) {
                continue;
            }
            let kind = number(&row, "kind")? as i32;
            let fields = if context.field(target, kind, "name", false) {
                row.get("name")
                    .and_then(Value::as_str)
                    .map(|s| wire::Field {
                        name: "name".into(),
                        value: Some(field::Value::Text(s.into())),
                    })
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            };
            related.push(api::Item {
                target: Some(target_wire(target)),
                kind,
                revision: number(&row, "revision")?,
                fields,
            });
        }
        let redact = |observations: Vec<wire::Observation>| {
            observations
                .into_iter()
                .filter_map(|mut o| {
                    let evidence = o.evidence.as_mut()?;
                    evidence
                        .fields
                        .retain(|f| context.field(target, asset.kind, &f.name, false));
                    if evidence.fields.is_empty() {
                        None
                    } else {
                        Some(o)
                    }
                })
                .collect()
        };
        let (participants, participants_complete, group) =
            self.workspace_network_fields(context, target, asset.kind, &mut fields)?;
        let current_addresses = if read.relations_cursor.is_none() {
            tx.current_addresses(context, target, asset.kind)?
        } else {
            Vec::new()
        };
        let mut detail = api::Detail {
            item: Some(api::Item {
                target: Some(target_wire(target)),
                kind: asset.kind,
                revision: asset.revision,
                fields,
            }),
            relations,
            observed: redact(asset.observed),
            imported: redact(asset.imported),
            participants,
            participants_complete,
            group,
            related,
            host_network: Vec::new(),
            current_addresses,
            relations_next,
            network_next: None,
            identity: if read.relations_cursor.is_none() {
                tx.host_identity(context, target, asset.kind)?
            } else {
                Vec::new()
            },
        };
        if read.relations_cursor.is_none() {
            tx.read_network_page(context, target, asset.kind, working, None, &mut detail)?;
        }
        tx.commit()?;
        if self.usage()?.revision != basis {
            return Err(Error::Expired);
        }
        Ok(detail)
    }

    pub(super) fn workspace_write(
        &self,
        context: &Context,
        write: api::Write,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        let mut tx = self.begin()?;
        let target = tx.context_write(context, write, working, now)?;
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
    pub(super) fn workspace_create(
        &self,
        context: &Context,
        create: api::Create,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        use wire::AssetKind as K;
        if create.addresses.len() > 16
            || (!create.addresses.is_empty()
                && !matches!(
                    K::try_from(create.kind),
                    Ok(K::Device | K::VirtualMachine | K::SoftwareInstallation)
                ))
        {
            return Err(Error::Invalid);
        }
        let mut seen = BTreeSet::new();
        for address in &create.addresses {
            let ip = address
                .address
                .parse::<std::net::IpAddr>()
                .map_err(|_| Error::Invalid)?;
            if address.vlan_tag.is_some_and(|n| !(1..=4094).contains(&n))
                || address.interface_name.as_ref().is_some_and(|n| {
                    n.is_empty() || n.len() > 160 || n.chars().any(char::is_control)
                })
                || !seen.insert((ip, address.vlan_tag, address.interface_name.clone()))
            {
                return Err(Error::Invalid);
            }
        }
        let mut tx = self.begin()?;
        let target = tx.context_write(
            context,
            api::Write {
                kind: create.kind,
                changes: create.changes,
                parent: create.parent,
                ..Default::default()
            },
            working,
            now,
        )?;
        let local = api::WorkingContext {
            roots: working.roots.clone(),
            focus: Some(target_wire(target)),
            expected: Some(number(&tx.root, "revision")?),
        };
        let mut interfaces = BTreeMap::new();
        for address in create.addresses {
            let parent = if let Some(name) = &address.interface_name {
                if let Some(interface) = interfaces.get(name) {
                    *interface
                } else {
                    let interface = tx.context_write(
                        context,
                        api::Write {
                            kind: K::Interface as i32,
                            parent: Some(target_wire(target)),
                            changes: vec![wire::Field {
                                name: "name".into(),
                                value: Some(field::Value::Text(name.clone())),
                            }],
                            ..Default::default()
                        },
                        &local,
                        now,
                    )?;
                    interfaces.insert(name.clone(), interface);
                    interface
                }
            } else {
                target
            };
            let changes = address
                .vlan_tag
                .map(|n| wire::Field {
                    name: "vlan_tag".into(),
                    value: Some(field::Value::Integer(n.into())),
                })
                .into_iter()
                .collect();
            tx.workspace_address(
                context,
                api::Address {
                    address: Some(address.address),
                    prefix_length: address.prefix_length,
                    parent: Some(target_wire(parent)),
                    changes,
                    ..Default::default()
                },
                &api::WorkingContext {
                    focus: Some(target_wire(parent)),
                    ..local.clone()
                },
                now,
            )?;
        }
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
    pub(super) fn workspace_bulk(
        &self,
        context: &Context,
        bulk: api::Bulk,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if bulk.targets.is_empty()
            || bulk.targets.len() > MAX_BULK
            || (bulk.remove && !bulk.changes.is_empty())
        {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        let selection = tx.selection(context, working, true)?;
        if number(&tx.root, "revision")? != bulk.expected {
            return Err(Error::Conflict);
        }
        let mut seen = BTreeSet::new();
        for target in &bulk.targets {
            let target = parse_target(Some(target))?;
            selection.contains(target, &tx)?;
            if !seen.insert(target) {
                return Err(Error::Invalid);
            }
            let row = tx.visible_asset(context, target)?;
            if bulk.remove {
                tx.workspace_remove(context, target, &row, now)?;
            } else {
                tx.context_write(
                    context,
                    api::Write {
                        target: Some(target_wire(target)),
                        expected: Some(number(&row, "revision")?),
                        kind: number(&row, "kind")? as i32,
                        changes: bulk.changes.clone(),
                        parent: None,
                    },
                    working,
                    now,
                )?;
            }
        }
        Ok(mutation(tx.commit()?, bulk.targets))
    }
}

impl Tx<'_> {
    pub(super) fn context_write(
        &mut self,
        context: &Context,
        write: api::Write,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<AssetRef> {
        if write.kind == wire::AssetKind::AddressReference as i32 {
            if write.changes.is_empty() || write.changes.len() > MAX_FIELDS {
                return Err(Error::Invalid);
            }
            let mut seen = BTreeSet::new();
            for field in &write.changes {
                if !seen.insert(&field.name) {
                    return Err(Error::Invalid);
                }
                self.validate_field(write.kind, field)?;
            }
            return self.workspace_address(
                context,
                super::addresses::from_write(write)?,
                working,
                now,
            );
        }
        let parent = if write.target.is_none() {
            self.creation_parent(
                context,
                working,
                write.parent.as_ref(),
                wire::AssetKind::try_from(write.kind).map_err(|_| Error::Invalid)?,
            )?
        } else {
            if write.parent.is_some() {
                return Err(Error::Invalid);
            }
            self.selection(context, working, true)?
                .contains(parse_target(write.target.as_ref())?, self)?;
            None
        };
        let kind = wire::AssetKind::try_from(write.kind).map_err(|_| Error::Invalid)?;
        let target = self.workspace_write(context, write, now)?;
        self.attach_created(context, target, kind, parent, now)?;
        Ok(target)
    }
}

impl Store {
    pub(super) fn workspace_relation(
        &self,
        context: &Context,
        request: api::Relationship,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        let mut relation = request.relation.ok_or(Error::Invalid)?;
        if relation.kind == wire::RelationKind::AssignedTo as i32 {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        let selection = tx.selection(context, working, true)?;
        let authority = tx.workspace_authority()?;
        if relation.revision == 0 {
            if !relation.relation_id.is_empty()
                || !relation.authority_id.is_empty()
                || request.remove
            {
                return Err(Error::Invalid);
            }
            relation.relation_id = RelationId::generate().to_string();
        } else {
            let id: RelationId = relation.relation_id.parse().map_err(|_| Error::Invalid)?;
            let row = tx.get(&format!("relation/{id}"))?.ok_or(Error::Missing)?;
            let existing = super::super::inventory::relation_projection(&row)?;
            if existing.revision != relation.revision || existing.authority_id != authority {
                return Err(Error::Conflict);
            }
            tx.check_relationship(context, &selection, &existing)?;
            if request.remove {
                if existing.kind == wire::RelationKind::RunsOn as i32 {
                    let guest = parse_target(existing.from.as_ref())?;
                    if number(&tx.asset(guest)?, "kind")? == wire::AssetKind::VirtualMachine as u64
                    {
                        tx.require_no_pending_operations(guest)?;
                    }
                }
                if existing.kind == wire::RelationKind::ReleaseOf as i32
                    && tx.release_is_installed(parse_target(existing.from.as_ref())?)?
                {
                    return Err(Error::Conflict);
                }
                let key = format!("relation/{id}");
                // Each relation owns one edge in each direction. Delete those
                // exact attachments before removing their metadata row.
                for query in [
                    "MATCH (r:OmRow {key:$key})<-[e:OmRelationFrom]-() DELETE e",
                    "MATCH (r:OmRow {key:$key})-[e:OmRelationTo]->() DELETE e",
                ] {
                    self.db.execute_in_transaction(
                        tx.id.ok_or(Error::Corrupt)?,
                        query,
                        params([("key", text(&key))]),
                    )?;
                }
                tx.delete(&key)?;
                tx.audit(
                    &key,
                    &context.actor,
                    "relation.removed",
                    relation.revision.checked_add(1).ok_or(Error::Limit)?,
                    now,
                )?;
                return Ok(mutation(tx.commit()?, Vec::new()));
            }
        }
        relation.authority_id = authority;
        let (from, to) = tx.check_relationship(context, &selection, &relation)?;
        if relation.kind == wire::RelationKind::RunsOn as i32
            && number(&tx.asset(from)?, "kind")? == wire::AssetKind::VirtualMachine as u64
            && number(&tx.asset(to)?, "kind")? == wire::AssetKind::SoftwareInstallation as u64
            && !context.field(
                to,
                wire::AssetKind::SoftwareInstallation as i32,
                "virtualization_role",
                false,
            )
        {
            return Err(Error::Denied);
        }
        if relation.revision == 0 {
            let rows=tx.query("MATCH (a:OmRow {key:$from})-[:OmRelationFrom]->(r:OmRow)-[:OmRelationTo]->(b:OmRow {key:$to}) WHERE r.kind=$kind RETURN properties(r) AS p LIMIT 1",params([("from",text(asset_key(from))),("to",text(asset_key(to))),("kind",Value::Int(relation.kind as i64))]))?;
            for row in rows {
                let existing = super::super::inventory::relation_projection(&row)?;
                if existing.from == relation.from
                    && existing.to == relation.to
                    && existing.kind == relation.kind
                    && existing.authority_id == relation.authority_id
                {
                    return Ok(mutation(
                        tx.commit()?,
                        vec![target_wire(from), target_wire(to)],
                    ));
                }
            }
        }
        let expected = if relation.revision == 0 {
            None
        } else {
            Some(relation.revision)
        };
        relation.revision = expected.unwrap_or(0).checked_add(1).ok_or(Error::Limit)?;
        tx.save_typed_relation(&relation, expected, &context.actor, now)?;
        Ok(mutation(
            tx.commit()?,
            vec![target_wire(from), target_wire(to)],
        ))
    }
}

impl Tx<'_> {
    pub(super) fn workspace_write(
        &mut self,
        context: &Context,
        write: api::Write,
        now: u64,
    ) -> Result<AssetRef> {
        self.workspace_write_inner(context, write, now, false)
    }
    pub(super) fn workspace_network_write(
        &mut self,
        context: &Context,
        write: api::Write,
        now: u64,
    ) -> Result<AssetRef> {
        self.workspace_write_inner(context, write, now, true)
    }
    fn workspace_write_inner(
        &mut self,
        context: &Context,
        write: api::Write,
        now: u64,
        network: bool,
    ) -> Result<AssetRef> {
        let target = write
            .target
            .as_ref()
            .map(|t| parse_target(Some(t)))
            .transpose()?
            .unwrap_or_else(new_target);
        let kind = wire::AssetKind::try_from(write.kind).map_err(|_| Error::Invalid)?;
        if kind == wire::AssetKind::Unspecified {
            return Err(Error::Invalid);
        }
        if write.target.is_none()
            && !network
            && matches!(
                kind,
                wire::AssetKind::IpAddress
                    | wire::AssetKind::Prefix
                    | wire::AssetKind::Vlan
                    | wire::AssetKind::AddressGroup
            )
        {
            return Err(Error::Invalid);
        }
        let key = asset_key(target);
        let old = self.get(&key)?;
        if write.target.is_some() && old.is_none() {
            return Err(Error::Missing);
        }
        if self.get(&format!("removed/{}", target.asset))?.is_some() {
            return Err(Error::Conflict);
        }
        if old.as_ref().map(|p| number(p, "revision")).transpose()? != write.expected {
            return Err(Error::Conflict);
        }
        if let Some(row) = &old {
            if number(row, "adoption")? != wire::Adoption::Adopted as u64 {
                return Err(Error::Conflict);
            }
            if super::super::target(row)? != target || number(row, "kind")? as i32 != write.kind {
                return Err(Error::Conflict);
            }
            // Network forms echo a readable name; unchanged text needs no rename grant.
            if network
                && context.field(target, write.kind, "name", false)
                && let [
                    wire::Field {
                        name,
                        value: Some(wire::field::Value::Text(value)),
                    },
                ] = write.changes.as_slice()
                && name == "name"
                && row.get("name").and_then(Value::as_str) == Some(value.as_str())
            {
                return Ok(target);
            }
        }
        self.require_changes(context, target, write.kind, &write.changes)?;
        self.validate_software_state(target, kind, &write.changes)?;
        if old.is_none()
            && kind == wire::AssetKind::Device
            && write
                .changes
                .iter()
                .any(|f| f.name == "virtualization_role")
        {
            return Err(Error::Invalid);
        }
        let authority = self.workspace_authority()?;
        let mut changed = old.is_none();
        for field in &write.changes {
            let field_key = format!("{key}/intent/{}", field.name);
            let prior = self.get(&field_key)?;
            if kind == wire::AssetKind::Device
                && field.name == "virtualization_role"
                && prior.is_none()
            {
                return Err(Error::Invalid);
            }
            if prior
                .as_ref()
                .is_some_and(|p| super::super::inventory::field_matches(field, p))
            {
                continue;
            }
            let mut data = super::super::inventory::field_props(field)?;
            data.extend(props([
                ("layer", text("intent")),
                ("authority", text(&authority)),
            ]));
            changed |=
                self.put_against(&field_key, &key, Class::Inventory, data, prior.as_ref())?;
        }
        if !changed {
            return Ok(target);
        }
        let revision = write
            .expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let mut row = target_props(target);
        row.extend(props([
            ("kind", Value::Int(write.kind.into())),
            (
                "adoption",
                Value::Int(wire::Adoption::Adopted as i32 as i64),
            ),
            ("revision", int(revision)?),
        ]));
        for field in ["name", "virtualization_role"] {
            if let Some(value) = self.get(&format!("{key}/intent/{field}"))? {
                hot_field(&inventory::read_field(&value)?, &mut row);
            }
        }
        self.put_against(&key, "assets", Class::Inventory, row, old.as_ref())?;
        if kind == wire::AssetKind::Container
            && write.changes.iter().any(|field| {
                matches!(
                    field.name.as_str(),
                    "runtime_object_id" | "runtime_lifetime"
                )
            })
        {
            self.bind_container_runtime(target, None)?;
        }
        self.audit(&key, &context.actor, "inventory.accepted", revision, now)?;
        Ok(target)
    }
    pub(super) fn workspace_authority(&mut self) -> Result<String> {
        match self.get("workspace/authority")? {
            Some(p) => string(&p, "authority"),
            None => {
                let id = AuthorityId::generate().to_string();
                self.put(
                    "workspace/authority",
                    "workspace",
                    Class::Inventory,
                    props([("authority", text(&id))]),
                )?;
                Ok(id)
            }
        }
    }
    pub(super) fn workspace_remove(
        &mut self,
        context: &Context,
        target: AssetRef,
        row: &Properties,
        now: u64,
    ) -> Result<()> {
        if !context.field(target, number(row, "kind")? as i32, "remove", true) {
            return Err(Error::Denied);
        }
        let key = asset_key(target);
        // Existing operation/retirement code owns uncertain effects and identity fences.
        self.require_no_account_references(target)?;
        self.require_no_pending_operations(target)?;
        let linked = self.query("MATCH (n:OmRow) WHERE n.parent='relations' AND (n.asset=$asset OR n.to_asset=$asset) RETURN properties(n) AS p LIMIT 1",params([("asset", text(target.asset.to_string()))]))?;
        if !linked.is_empty() {
            return Err(Error::Conflict);
        }
        let scope = format!("{}/{}", target.asset, target.incarnation);
        self.require_no_ipam_references(target)?;
        let network_key = format!("network/{scope}");
        if let Some(network) = self.get(&network_key)? {
            let members = self.children(&format!("members/{scope}"))?;
            if !members.is_empty() {
                return Err(Error::Conflict);
            }
            if let Some(namespace) = network.get("namespace") {
                self.delete(namespace.as_str().ok_or(Error::Corrupt)?)?;
            }
            if let Some(group) = network::groups::group_of(&network)? {
                let root = self.asset(group)?;
                self.workspace_group_member_owner(
                    context,
                    group,
                    Some(number(&root, "revision")?),
                )?;
                self.workspace_advance(group, &context.actor, "group.addresses", now)?;
            }
            self.delete(&network_key)?;
        }
        let group_key = format!("group/{scope}");
        if let Some(group) = self.get(&group_key)? {
            if !self.children(&format!("members/{scope}"))?.is_empty() {
                return Err(Error::Conflict);
            }
            if let Some(namespace) = group.get("namespace").and_then(Value::as_str) {
                self.delete(namespace)?;
            }
            self.delete(&group_key)?;
        }
        self.retire_current_facts(target)?;
        let mappings = self.query("MATCH (n:OmRow {asset:$asset}) WHERE n.parent='csv-mappings' AND n.incarnation=$incarnation AND n.class=$class RETURN properties(n) AS p LIMIT 2049", params([("asset", text(target.asset.to_string())), ("incarnation", text(target.incarnation.to_string())), ("class", Value::Int(Class::Inventory as i64))]))?;
        if mappings.len() > MAX_CHILDREN {
            return Err(Error::Limit);
        }
        for mapping in mappings {
            self.delete(&string(&mapping, "key")?)?;
        }
        for child in self.children(&key)? {
            let child_key = string(&child, "key")?;
            if child.get("layer") == Some(&text("mapping")) {
                let mut checkpoint = child.clone();
                super::super::operations::strip_envelope(&mut checkpoint);
                self.put_against(
                    &child_key,
                    &key,
                    Class::Checkpoint,
                    checkpoint,
                    Some(&child),
                )?;
            } else {
                self.delete(&child_key)?;
            }
        }
        self.delete(&key)?;
        self.put(
            &format!("removed/{}", target.asset),
            "removed",
            Class::Checkpoint,
            target_props(target),
        )?;
        self.audit(
            &key,
            &context.actor,
            "inventory.removed",
            number(row, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?,
            now,
        )?;
        Ok(())
    }
}

pub(super) fn query_fingerprint(query: &api::Query, context: &Context) -> String {
    let mut canonical = query.clone();
    canonical.cursor = None;
    let mut digest = Sha256::new();
    digest.update(canonical.encode_to_vec());
    digest.update(context.actor.as_bytes());
    digest.update(format!("{:?}", context.grants).as_bytes());
    format!("{:x}", digest.finalize())
}
fn network_column(name: &str) -> bool {
    [
        "cidr",
        "address_space",
        "link_scope",
        "routing_context",
        "sharing",
        "vrid",
        "vlan_id",
        "address",
        "prefix_length",
        "meaning",
        "zone",
        "binding",
    ]
    .contains(&name)
}
pub(super) fn validate_column(name: &str, definitions: &[api::Definition]) -> Result<()> {
    if [
        "name",
        "description",
        "serial",
        "virtualization_role",
        "vlan_tag",
        "cidr",
        "address_space",
        "link_scope",
        "routing_context",
        "sharing",
        "vrid",
        "vlan_id",
        "address",
        "prefix_length",
        "meaning",
        "zone",
        "binding",
    ]
    .contains(&name)
        || software::known_field(name)
        || definitions.iter().any(|d| d.id == name && !d.retired)
    {
        Ok(())
    } else {
        Err(Error::Incomplete)
    }
}
pub(super) fn validate_predicate(
    predicate: &api::Predicate,
    definitions: &[api::Definition],
) -> Result<()> {
    // Mapping identities are explicitly reviewed in the reconciliation flow,
    // not searchable text that could reveal an undisclosable canonical target.
    if predicate.field == "binding" {
        return Err(Error::Invalid);
    }
    validate_column(&predicate.field, definitions)?;
    if let Some(definition) = definitions.iter().find(|d| d.id == predicate.field) {
        if predicate.definition_generation != Some(definition.generation) {
            return Err(Error::Incomplete);
        }
        super::definitions::validate_value(
            definition,
            predicate.value.as_ref().ok_or(Error::Invalid)?,
        )?;
    } else {
        if predicate.definition_generation.is_some() {
            return Err(Error::Invalid);
        }
        match predicate.value.as_ref().and_then(|v| v.value.as_ref()) {
            Some(field::Value::Integer(_))
                if predicate.field == "vrid"
                    || predicate.field == "vlan_id"
                    || predicate.field == "vlan_tag"
                    || predicate.field == "prefix_length" => {}
            Some(field::Value::Text(s))
                if s.len() <= 2048
                    && predicate.field != "vrid"
                    && predicate.field != "vlan_id"
                    && predicate.field != "vlan_tag"
                    && predicate.field != "prefix_length" => {}
            Some(field::Value::Clear(EXPLICIT_CLEAR)) if !network_column(&predicate.field) => {}
            _ => return Err(Error::Invalid),
        }
    }
    if predicate
        .value
        .as_ref()
        .is_none_or(|v| v.name != predicate.field)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
