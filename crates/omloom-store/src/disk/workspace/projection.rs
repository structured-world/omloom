use super::*;

const MAX_CONTEXT: usize = 2048;

pub(super) fn local_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_link_local() || ip.is_unspecified(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v| v.is_loopback() || v.is_link_local())
        }
    }
}

impl Tx<'_> {
    fn assigned_addresses(
        &self,
        context: &Context,
        interfaces: &BTreeSet<AssetRef>,
        owners: &BTreeMap<AssetRef, BTreeSet<AssetRef>>,
        by_owner: &mut BTreeMap<AssetRef, BTreeSet<String>>,
    ) -> Result<()> {
        let ids: BTreeSet<_> = interfaces
            .iter()
            .filter(|t| {
                context.field(**t, wire::AssetKind::Interface as i32, "network", false)
                    && context.field(**t, wire::AssetKind::Interface as i32, "relations", false)
            })
            .map(|t| t.asset.to_string())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        // Use the actual membership rows, with one indexed batch for displayed interfaces.
        let members = self.query(
            "UNWIND $ids AS id MATCH (m:OmRow {asset:id}) WHERE m.parent='members/'+m.group RETURN properties(m) AS p LIMIT 2049",
            params([("ids", Value::Array(ids.iter().map(text).collect()))]),
        )?;
        if members.len() > MAX_CONTEXT {
            return Err(Error::Limit);
        }
        let mut memberships: BTreeMap<AssetRef, BTreeSet<AssetRef>> = BTreeMap::new();
        for member in members {
            let interface = target(&member)?;
            if interfaces.contains(&interface) {
                let owner = network::groups::reference(&string(&member, "group")?)?;
                memberships.entry(owner).or_default().insert(interface);
            }
        }
        if memberships.is_empty() {
            return Ok(());
        }
        let roots = self.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 2049",
            params([(
                "keys",
                Value::Array(memberships.keys().map(|t| text(asset_key(*t))).collect()),
            )]),
        )?;
        let mut direct = Vec::new();
        let mut groups = Vec::new();
        for root in roots {
            let owner = target(&root)?;
            let kind = number(&root, "kind")? as i32;
            if number(&root, "adoption")? != wire::Adoption::Adopted as u64
                || !context.field(owner, kind, "network", false)
                || !context.field(owner, kind, "relations", false)
            {
                continue;
            }
            if kind == wire::AssetKind::IpAddress as i32 {
                direct.push(text(format!("network/{}", network::identity(owner))));
            } else if kind == wire::AssetKind::AddressGroup as i32 {
                groups.push(text(network::identity(owner)));
            }
        }
        let mut attached = 0;
        for (property, keys) in [("key", direct), ("group", groups)] {
            if keys.is_empty() {
                continue;
            }
            let mut values = HashMap::from([("keys".into(), Value::Array(keys))]);
            let visible = ["cidr", "network", "relations"]
                .map(|field| assets::visible_clause_for(context, Some(field), &mut values, "a"))
                .join(" AND ");
            let rows = self.query(
                &format!("UNWIND $keys AS key MATCH (n:OmRow {{{property}:key}}) WHERE n.parent='networks' WITH n, 'asset/'+n.asset AS asset_key MATCH (a:OmRow {{key:asset_key}}) WHERE a.incarnation=n.incarnation AND a.kind={} AND a.adoption={} AND {visible} RETURN properties(n) AS p LIMIT 2049", wire::AssetKind::IpAddress as i32, wire::Adoption::Adopted as i32),
                Some(values),
            )?;
            if rows.len() > MAX_CONTEXT {
                return Err(Error::Limit);
            }
            for row in rows {
                let address = target(&row)?;
                let owner = network::groups::group_of(&row)?.unwrap_or(address);
                let cidr: ipnet::IpNet =
                    string(&row, "cidr")?.parse().map_err(|_| Error::Corrupt)?;
                if local_address(cidr.addr()) {
                    continue;
                }
                for interface in memberships.get(&owner).into_iter().flatten() {
                    if attached == MAX_CONTEXT {
                        return Err(Error::Limit);
                    }
                    attached += 1;
                    for owner in owners.get(interface).into_iter().flatten() {
                        by_owner
                            .entry(*owner)
                            .or_default()
                            .insert(cidr.addr().to_string());
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn context_edges(
        &self,
        context: &Context,
        keys: &BTreeSet<String>,
        children: bool,
    ) -> Result<Vec<(Properties, Properties, wire::Relation)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        if keys.len() > MAX_CONTEXT {
            return Err(Error::Limit);
        }
        let mut rows = Vec::new();
        for reverse in [false, true] {
            let mut values = HashMap::new();
            let owned = assets::ownership_clause(Some(reverse), &[]);
            let assigned = if children && reverse {
                format!(
                    "(p.kind={} AND a.kind={} AND r.kind={} AND r.to_asset=p.asset AND r.to_incarnation=p.incarnation AND r.asset=a.asset AND r.incarnation=a.incarnation)",
                    wire::AssetKind::Interface as i32,
                    wire::AssetKind::IpAddress as i32,
                    wire::RelationKind::AssignedTo as i32
                )
            } else {
                "false".into()
            };
            let parent_visible =
                assets::visible_clause_for(context, Some("relations"), &mut values, "p");
            let child_visible =
                assets::visible_clause_for(context, Some("relations"), &mut values, "a");
            let name = if children {
                "true".into()
            } else {
                assets::visible_clause_for(context, Some("name"), &mut values, "p")
            };
            let carrier = if children {
                "true".into()
            } else {
                format!("r.kind<>{}", wire::RelationKind::Carries as i32)
            };
            values.insert("keys".into(), Value::Array(keys.iter().map(text).collect()));
            let path = assets::ownership_path(children, reverse);
            let (root, other) = if children { ("p", "a") } else { ("a", "p") };
            rows.extend(self.raw(&format!("UNWIND $keys AS key MATCH {path} WHERE p.parent='assets' AND a.parent='assets' AND p.key<>a.key AND p.adoption={} AND a.adoption={} AND {parent_visible} AND {child_visible} AND {name} AND {carrier} AND ({owned} OR {assigned}) RETURN properties({root}) AS a, properties({other}) AS b, properties(r) AS r LIMIT 2049",wire::Adoption::Adopted as i32,wire::Adoption::Adopted as i32),values)?);
            if rows.len() > MAX_CONTEXT {
                return Err(Error::Limit);
            }
        }
        let mut result = Vec::new();
        for mut row in rows {
            let (Some(Value::Map(a)), Some(Value::Map(b)), Some(Value::Map(r))) =
                (row.remove("a"), row.remove("b"), row.remove("r"))
            else {
                return Err(Error::Corrupt);
            };
            let relation = super::super::inventory::relation_projection(&r)?;
            if !children && relation.kind == wire::RelationKind::Carries as i32 {
                continue;
            }
            let (parent, child) = if children { (&a, &b) } else { (&b, &a) };
            let parent_ref = target(parent)?;
            let parent_kind = wire::AssetKind::try_from(number(parent, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?;
            let child_kind = wire::AssetKind::try_from(number(child, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?;
            let from = parse_target(relation.from.as_ref())?;
            let to = parse_target(relation.to.as_ref())?;
            let owned =
                working::refines(parent_ref, parent_kind, child_kind, relation.kind, from, to);
            let assigned = children
                && parent_kind == wire::AssetKind::Interface
                && child_kind == wire::AssetKind::IpAddress
                && relation.kind == wire::RelationKind::AssignedTo as i32
                && to == parent_ref;
            if owned || assigned {
                result.push((a, b, relation));
            }
        }
        Ok(result)
    }

    pub(super) fn item_contexts(
        &self,
        context: &Context,
        rows: &[Properties],
        query: &api::Query,
    ) -> Result<Vec<api::ItemContext>> {
        let stops: BTreeSet<_> = query
            .roots
            .iter()
            .chain(query.focus.iter())
            .map(|r| parse_target(Some(r)))
            .collect::<Result<_>>()?;
        let mut nodes = rows
            .iter()
            .map(|r| Ok((target(r)?, r.clone())))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut parents: BTreeMap<AssetRef, BTreeSet<AssetRef>> = BTreeMap::new();
        let mut frontier: BTreeSet<_> = rows.iter().filter(|r| matches!(number(r,"kind").ok(),Some(n) if n==wire::AssetKind::Service as u64 || n==wire::AssetKind::Interface as u64 || n==wire::AssetKind::SoftwareInstallation as u64 || n==wire::AssetKind::AddressReference as u64)).map(|r| string(r,"key")).collect::<Result<_>>()?;
        for _ in 0..6 {
            let mut next = BTreeSet::new();
            for (child, parent, _) in self.context_edges(context, &frontier, false)? {
                let c = target(&child)?;
                let p = target(&parent)?;
                let ck = number(&child, "kind")? as i32;
                let pk = number(&parent, "kind")? as i32;
                if stops.contains(&c)
                    || !context.field(c, ck, "relations", false)
                    || !context.field(p, pk, "name", false)
                    || !context.field(p, pk, "relations", false)
                    || number(&parent, "adoption")? != wire::Adoption::Adopted as u64
                {
                    continue;
                }
                parents.entry(c).or_default().insert(p);
                if !nodes.contains_key(&p)
                    && !stops.contains(&p)
                    && ![
                        wire::AssetKind::Device as i32,
                        wire::AssetKind::VirtualMachine as i32,
                    ]
                    .contains(&pk)
                {
                    next.insert(asset_key(p));
                }
                nodes.insert(p, parent);
            }
            if nodes.len() > MAX_CONTEXT {
                return Err(Error::Limit);
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        // Address hints belong only to displayed services/interfaces, never their ancestors.
        let roots: Vec<_> = rows.iter().filter(|r|matches!(number(r,"kind").ok(),Some(n) if [wire::AssetKind::Service as u64,wire::AssetKind::Interface as u64].contains(&n))).map(target).collect::<Result<_>>()?;
        let mut owners: BTreeMap<AssetRef, BTreeSet<AssetRef>> =
            roots.iter().map(|r| (*r, BTreeSet::from([*r]))).collect();
        let mut frontier: BTreeSet<_> = roots.iter().map(|r| asset_key(*r)).collect();
        let mut address_nodes = BTreeMap::new();
        let mut interfaces: BTreeSet<_> = rows
            .iter()
            .filter(|r| number(r, "kind").ok() == Some(wire::AssetKind::Interface as u64))
            .map(target)
            .collect::<Result<_>>()?;
        for _ in 0..3 {
            let mut next = BTreeSet::new();
            for (parent, child, _) in self.context_edges(context, &frontier, true)? {
                let p = target(&parent)?;
                let c = target(&child)?;
                let pk = number(&parent, "kind")? as i32;
                let ck = number(&child, "kind")? as i32;
                if !context.field(p, pk, "relations", false)
                    || !context.field(c, ck, "relations", false)
                    || number(&child, "adoption")? != wire::Adoption::Adopted as u64
                {
                    continue;
                }
                if ![
                    wire::AssetKind::SoftwareInstallation as i32,
                    wire::AssetKind::Interface as i32,
                    wire::AssetKind::AddressReference as i32,
                    wire::AssetKind::IpAddress as i32,
                ]
                .contains(&ck)
                {
                    continue;
                }
                let inherited: Vec<_> = owners.get(&p).into_iter().flatten().copied().collect();
                let entry = owners.entry(c).or_default();
                entry.extend(inherited);
                if ck == wire::AssetKind::Interface as i32 {
                    interfaces.insert(c);
                }
                if ck == wire::AssetKind::AddressReference as i32
                    || ck == wire::AssetKind::IpAddress as i32
                {
                    address_nodes.insert(c, child);
                } else {
                    next.insert(asset_key(c));
                }
            }
            if owners.len() > MAX_CONTEXT {
                return Err(Error::Limit);
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        let keys: Vec<_> = address_nodes
            .keys()
            .map(|t| text(format!("network/{}", network::identity(*t))))
            .collect();
        let addresses = if keys.is_empty() {
            Vec::new()
        } else {
            self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 2049",params([("keys",Value::Array(keys))]))?
        };
        if addresses.len() > MAX_CONTEXT {
            return Err(Error::Limit);
        }
        let mut by_owner: BTreeMap<AssetRef, BTreeSet<String>> = BTreeMap::new();
        self.assigned_addresses(context, &interfaces, &owners, &mut by_owner)?;
        for row in addresses {
            let a = target(&row)?;
            let root = address_nodes.get(&a).ok_or(Error::Corrupt)?;
            let k = number(root, "kind")? as i32;
            let field = if k == wire::AssetKind::AddressReference as i32 {
                "address"
            } else {
                "cidr"
            };
            if !context.field(a, k, field, false) {
                continue;
            }
            // A contact destination is not a local interface address; its fact remains in inventory.
            if k == wire::AssetKind::AddressReference as i32
                && (!context.field(a, k, "meaning", false)
                    || row.get("meaning").and_then(Value::as_str)
                        == Some(api::AddressMeaning::Destination.as_str_name()))
            {
                continue;
            }
            let literal = string(&row, field)?;
            let ip = literal
                .split('/')
                .next()
                .ok_or(Error::Corrupt)?
                .parse()
                .map_err(|_| Error::Corrupt)?;
            if local_address(ip) {
                continue;
            }
            for owner in owners.get(&a).into_iter().flatten() {
                by_owner.entry(*owner).or_default().insert(ip.to_string());
            }
        }
        if !interfaces.is_empty() {
            let interface_keys: BTreeMap<_, _> = interfaces
                .iter()
                .map(|t| (interface_facts::address_layer(*t), *t))
                .collect();
            let rows = self.query("UNWIND $keys AS layer MATCH (n:OmRow {layer:layer}) RETURN properties(n) AS p LIMIT 2049",params([("keys",Value::Array(interface_keys.keys().map(text).collect()))]))?;
            if rows.len() > MAX_CONTEXT {
                return Err(Error::Limit);
            }
            for row in rows {
                let parent = string(&row, "layer")?;
                let interface = interface_keys.get(&parent).ok_or(Error::Corrupt)?;
                if !context.field(
                    *interface,
                    wire::AssetKind::Interface as i32,
                    "current_addresses",
                    false,
                ) {
                    continue;
                }
                let address = api::CurrentAddress::decode(binary(&row, "payload")?)
                    .map_err(|_| Error::Corrupt)?;
                if address.state == api::AddressState::Deprecated as i32 {
                    continue;
                }
                for owner in owners.get(interface).into_iter().flatten() {
                    by_owner
                        .entry(*owner)
                        .or_default()
                        .insert(address.address.clone());
                }
            }
        }
        let layers = rows
            .iter()
            .filter(|r| number(r, "kind").ok() == Some(wire::AssetKind::Service as u64))
            .map(|r| target(r).map(|t| text(network_facts::socket_layer(t))))
            .collect::<Result<Vec<_>>>()?;
        let mut endpoints: BTreeMap<AssetRef, Vec<api::SocketBind>> = BTreeMap::new();
        if !layers.is_empty() {
            let sockets=self.query("UNWIND $layers AS layer MATCH (n:OmRow {layer:layer}) RETURN properties(n) AS p LIMIT 2049",params([("layers",Value::Array(layers))]))?;
            if sockets.len() > MAX_CONTEXT {
                return Err(Error::Limit);
            }
            for row in sockets {
                let socket = api::SocketBind::decode(binary(&row, "payload")?)
                    .map_err(|_| Error::Corrupt)?;
                let service = parse_target(socket.service.as_ref())?;
                let installation = target(&row)?;
                if !network_facts::socket_visible(&socket)
                    || !context.field(
                        service,
                        wire::AssetKind::Service as i32,
                        "network_evidence",
                        false,
                    )
                    || !context.field(service, wire::AssetKind::Service as i32, "relations", false)
                    || !context.field(
                        installation,
                        wire::AssetKind::SoftwareInstallation as i32,
                        "network_evidence",
                        false,
                    )
                    || !parents
                        .get(&service)
                        .is_some_and(|p| p.contains(&installation))
                {
                    continue;
                }
                if let Ok(ip) = socket
                    .bind
                    .split('%')
                    .next()
                    .ok_or(Error::Corrupt)?
                    .parse::<std::net::IpAddr>()
                    && !projection::local_address(ip)
                    && socket.address_state != api::AddressState::Deprecated as i32
                {
                    by_owner.entry(service).or_default().insert(ip.to_string());
                }
                let current = endpoints.entry(service).or_default();
                if !current.contains(&socket) {
                    current.push(socket);
                }
            }
        }
        rows.iter()
            .map(|row| {
                let t = target(row)?;
                let mut chain = Vec::new();
                let mut alternatives = Vec::new();
                let mut cursor = t;
                let mut seen = BTreeSet::from([t]);
                while let Some(possible) = parents.get(&cursor) {
                    if possible.len() != 1 {
                        for p in possible {
                            let r = nodes.get(p).ok_or(Error::Corrupt)?;
                            alternatives.push(api::OwnerContext {
                                target: Some(target_wire(*p)),
                                kind: number(r, "kind")? as i32,
                                name: r
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            });
                        }
                        break;
                    }
                    let p = possible.first().ok_or(Error::Corrupt)?;
                    if !seen.insert(*p) || chain.len() >= 6 {
                        break;
                    }
                    let r = nodes.get(p).ok_or(Error::Corrupt)?;
                    chain.push(api::OwnerContext {
                        target: Some(target_wire(*p)),
                        kind: number(r, "kind")? as i32,
                        name: r
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    });
                    if [
                        wire::AssetKind::Device as u64,
                        wire::AssetKind::VirtualMachine as u64,
                    ]
                    .contains(&number(r, "kind")?)
                    {
                        break;
                    }
                    cursor = *p;
                }
                chain.reverse();
                Ok(api::ItemContext {
                    target: Some(target_wire(t)),
                    owners: chain,
                    alternative_owners: alternatives,
                    addresses: by_owner.get(&t).into_iter().flatten().cloned().collect(),
                    endpoints: endpoints.remove(&t).unwrap_or_default(),
                })
            })
            .collect()
    }
}
