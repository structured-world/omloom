use super::*;
use api::{reply::Result as R, request::Command as C};

fn graph(store: &Store, actor: &Context, query: api::Query) -> Result<api::Page, Error> {
    match call(store, actor, C::Query(query))? {
        R::Page(page) => Ok(page),
        _ => Err(Error::Corrupt),
    }
}

fn graph_query() -> api::Query {
    api::Query {
        view: api::View::Network as i32,
        network: Some(Box::new(api::NetworkSelection {
            include_topology: true,
            ..Default::default()
        })),
        ..query()
    }
}

// Links and their continuation share a current cut with the permitted node page.
#[test]
fn topology_projects_known_links_without_disclosing_denied_endpoints() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let Some(R::Mutation(_)) = store
        .workspace(
            &actor,
            api::Request {
                command: Some(C::Address(api::Address {
                    address: Some("192.0.2.9/24".into()),
                    create_prefix: true,
                    ..Default::default()
                })),
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
            },
            100,
        )?
        .result
    else {
        return Err("known address result".into());
    };
    let before = store.usage()?;
    let page = graph(&store, &actor, graph_query())?;
    assert_eq!(page.total, 2);
    let topology = page.topology.ok_or("topology")?;
    assert_eq!(topology.edges.len(), 1);
    assert_eq!(topology.edges[0].kind, wire::RelationKind::MemberOf as i32);
    for edge in &topology.edges {
        assert!(page.items.iter().any(|item| item.target == edge.from));
        assert!(page.items.iter().any(|item| item.target == edge.to));
    }
    assert!(!topology.more_edges);
    let mut reader = operator();
    reader.grants[0].read = Fields::Selected(vec!["name".into()]);
    let restricted = graph(&store, &reader, graph_query())?;
    let topology = restricted.topology.ok_or("restricted topology")?;
    assert!(topology.edges.is_empty());
    assert!(!topology.more_edges);
    reader.grants[0].scope = Scope::Kinds(vec![K::Prefix]);
    let restricted = graph(&store, &reader, graph_query())?;
    assert_eq!(restricted.total, 1);
    assert!(
        restricted
            .topology
            .ok_or("restricted topology")?
            .edges
            .is_empty()
    );
    assert_eq!(store.usage()?, before);

    let mut selector = graph_query();
    selector.limit = 1;
    let first = graph(&store, &actor, selector.clone())?;
    assert_eq!(first.items.len(), 1);
    assert!(first.topology.ok_or("first topology")?.edges.is_empty());
    selector.cursor = first.next;
    assert!(selector.cursor.is_some());
    let second = graph(&store, &actor, selector.clone())?;
    assert_eq!(second.items.len(), 1);
    assert_ne!(second.items[0].target, first.items[0].target);
    assert!(second.topology.ok_or("second topology")?.edges.is_empty());
    create(&store, &actor, K::Vrf, "New revision")?;
    assert!(matches!(
        graph(&store, &actor, selector),
        Err(Error::Expired)
    ));
    Ok(())
}

fn child(
    store: &Store,
    actor: &Context,
    parent: &wire::AssetRef,
    kind: K,
    name: &str,
) -> Result<wire::AssetRef, Error> {
    let response = store.workspace(
        actor,
        api::Request {
            command: Some(C::Write(api::Write {
                kind: kind as i32,
                parent: Some(parent.clone()),
                changes: vec![text_field("name", name)],
                ..Default::default()
            })),
            working: Some(api::WorkingContext {
                focus: Some(parent.clone()),
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
        },
        100,
    )?;
    let Some(R::Mutation(mutation)) = response.result else {
        return Err(Error::Corrupt);
    };
    mutation.targets.into_iter().next().ok_or(Error::Corrupt)
}

// Location intersects project selection before count and page; absence stays explicit.
#[test]
fn topology_intersects_sites_with_independent_selection() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let east = create(&store, &actor, K::Site, "East")?;
    let west = create(&store, &actor, K::Site, "West")?;
    let east_host = child(&store, &actor, &east, K::Device, "East host")?;
    let west_host = child(&store, &actor, &west, K::Device, "West host")?;
    let east_port = child(&store, &actor, &east_host, K::Interface, "East interface")?;
    let west_port = child(&store, &actor, &west_host, K::Interface, "West interface")?;
    let unplaced_host = create(&store, &actor, K::Device, "Unplaced host")?;
    let unplaced = child(
        &store,
        &actor,
        &unplaced_host,
        K::Interface,
        "Unplaced interface",
    )?;
    let mut selector = graph_query();
    selector.kinds = vec![K::Interface as i32];
    assert_eq!(graph(&store, &actor, selector.clone())?.total, 3);
    selector.roots = vec![east_host, west_host];
    assert_eq!(graph(&store, &actor, selector.clone())?.total, 2);
    selector.network.as_mut().ok_or("selection")?.sites = vec![east.clone()];
    let page = graph(&store, &actor, selector.clone())?;
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&east_port));
    selector.roots = vec![west_port, unplaced];
    assert_eq!(graph(&store, &actor, selector.clone())?.total, 0);
    let mut denied = operator();
    denied.grants[0].scope = Scope::Kinds(vec![K::Interface]);
    assert!(matches!(
        graph(&store, &denied, selector),
        Err(Error::Denied)
    ));
    let mut unknown = graph_query();
    unknown.kinds = vec![K::Interface as i32];
    unknown.network.as_mut().ok_or("network")?.unplaced = true;
    assert_eq!(graph(&store, &actor, unknown)?.total, 1);
    Ok(())
}

// Selecting a network branch includes its deeper permitted interfaces and keeps the enclosing project.
#[test]
fn network_branch_queries_nested_resources_within_enclosing_roots() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let site = create(&store, &actor, K::Site, "Production site")?;
    let host = child(&store, &actor, &site, K::Device, "Production switch")?;
    let interface = child(&store, &actor, &host, K::Interface, "Production uplink")?;
    let other = create(&store, &actor, K::Site, "Other site")?;
    let outside = child(&store, &actor, &other, K::Device, "Other switch")?;
    child(&store, &actor, &outside, K::Interface, "Other uplink")?;
    let mut selector = graph_query();
    selector.roots = vec![site.clone()];
    selector.focus = Some(site);
    selector.kinds = vec![K::Interface as i32];
    selector.level_only = false;
    let page = graph(&store, &actor, selector.clone())?;
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&interface));
    selector.level_only = true;
    assert_eq!(graph(&store, &actor, selector.clone())?.total, 0);
    selector.level_only = false;
    selector.focus = Some(other);
    assert!(matches!(
        graph(&store, &actor, selector),
        Err(Error::Denied)
    ));
    Ok(())
}

// Filtering for one interface must not consume the unrelated site inventory.
#[test]
fn topology_unplaced_narrow_query_ignores_unrelated_sites() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let site = create(&store, &actor, K::Site, "Located site")?;
    let host = child(&store, &actor, &site, K::Device, "Located host")?;
    child(&store, &actor, &host, K::Interface, "Needle located")?;
    let expected = create(&store, &actor, K::Interface, "Needle unplaced")?;
    for index in 0..128 {
        create(&store, &actor, K::Site, &format!("Unrelated {index}"))?;
    }
    let mut selector = graph_query();
    selector.kinds = vec![K::Interface as i32];
    selector.search = "Needle".into();
    selector.network.as_mut().ok_or("selection")?.unplaced = true;
    let page = graph(&store, &actor, selector)?;
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&expected));
    Ok(())
}

// The same stable asset ID must not attach links from an earlier incarnation.
#[test]
fn topology_never_attaches_links_to_a_different_incarnation() -> TestResult {
    use coordinode_core::graph::types::Value;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let interface = create(&store, &actor, K::Interface, "Uplink")?;
    let segment = create(&store, &actor, K::L2Domain, "Fabric")?;
    call(
        &store,
        &actor,
        C::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(interface.clone()),
                to: Some(segment.clone()),
                kind: wire::RelationKind::On as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
    )?;
    assert_eq!(
        graph(&store, &actor, graph_query())?
            .topology
            .ok_or("topology")?
            .edges
            .len(),
        1
    );
    let incarnation = omloom_contracts::ids::IncarnationId::generate().to_string();
    store.diagnostic_database().execute_cypher_shared(
        "MATCH (n:OmRow {key:$key}) SET n.incarnation=$incarnation",
        Some(
            [
                (
                    "key".into(),
                    Value::String(format!("asset/{}", segment.asset_id)),
                ),
                ("incarnation".into(), Value::String(incarnation.clone())),
            ]
            .into_iter()
            .collect(),
        ),
        None,
        None,
        None,
    )?;
    let page = graph(&store, &actor, graph_query())?;
    assert!(
        page.items
            .iter()
            .any(|item| item.target.as_ref().is_some_and(|target| {
                target.asset_id == segment.asset_id && target.incarnation == incarnation
            }))
    );
    assert!(page.topology.ok_or("topology")?.edges.is_empty());
    Ok(())
}

// Dense membership retains independent links and bounds the permitted output.
#[test]
fn topology_caps_dense_pages_and_keeps_parallel_links() -> TestResult {
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let host = create(&store, &actor, K::Device, "Edge")?;
    let port = child(&store, &actor, &host, K::Port, "Physical uplink")?;
    let interface = child(&store, &actor, &port, K::Interface, "Logical uplink")?;
    call(
        &store,
        &actor,
        C::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(port.clone()),
                to: Some(interface.clone()),
                kind: wire::RelationKind::Carries as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
    )?;
    let mut selection = graph_query();
    selection.roots = vec![port, interface];
    let parallel = graph(&store, &actor, selection.clone())?
        .topology
        .ok_or("topology")?;
    assert_eq!(parallel.edges.len(), 2);
    assert_ne!(parallel.edges[0].relation_id, parallel.edges[1].relation_id);
    assert_eq!(parallel.edges[0].from, parallel.edges[1].from);
    assert_eq!(parallel.edges[0].to, parallel.edges[1].to);
    selection.roots.clear();
    selection.search = "Fabric".into();
    selection.limit = 25;
    let mut ports = Vec::new();
    for index in 0..16 {
        ports.push(child(
            &store,
            &actor,
            &host,
            K::Interface,
            &format!("Fabric port {index:02}"),
        )?);
    }
    let mut segments = Vec::new();
    for index in 0..9 {
        segments.push(create(
            &store,
            &actor,
            K::L2Domain,
            &format!("Fabric segment {index}"),
        )?);
    }
    let connect = |port: &wire::AssetRef, segment: &wire::AssetRef| {
        call(
            &store,
            &actor,
            C::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(port.clone()),
                    to: Some(segment.clone()),
                    kind: wire::RelationKind::On as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
    };
    for port in &ports {
        for segment in &segments {
            connect(port, segment)?;
        }
    }
    for index in 0..64 {
        let off_page = create(&store, &actor, K::L2Domain, &format!("Outside {index:02}"))?;
        connect(&ports[0], &off_page)?;
    }
    let before = store.usage()?;
    store.set_diagnostic_query_check(|db, statement| {
        if statement.contains("RETURN properties(r) AS p ORDER BY r.key LIMIT $limit") {
            let plan = db
                .explain_plan(statement, None)
                .expect("topology plan")
                .explain();
            assert!(
                plan.contains("IndexScan(r:OmRow ON omloom_topology_pair(topology_pair))"),
                "{plan}"
            );
        }
    });
    graph(&store, &actor, selection.clone())?;
    store.set_diagnostic_query_check(|_, _| {});
    let mut times = Vec::new();
    let mut payload = 0;
    for _ in 0..30 {
        let start = std::time::Instant::now();
        let page = graph(&store, &actor, selection.clone())?;
        times.push(start.elapsed());
        assert_eq!(page.total, 25);
        payload = page.encoded_len();
        assert!(payload < 32 * 1024);
        let topology = page.topology.ok_or("topology")?;
        assert_eq!(topology.edges.len(), 128);
        assert!(topology.more_edges);
        assert_eq!(
            topology
                .edges
                .iter()
                .map(|edge| &edge.relation_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            128
        );
    }
    times.sort_unstable();
    println!(
        "Dense topology 25 nodes, 144 relations, {payload} bytes, 30 native debug reads: p95 {:?}, p99 {:?}",
        times[28], times[29]
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Saved coordinates belong to exact visible incarnations and use the existing CAS.
#[test]
fn topology_layout_survives_reopen_and_filters_hidden_pins() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let segment = create(&store, &actor, K::L2Domain, "Segment")?;
    let pin = api::NetworkPin {
        target: Some(segment),
        x: -20,
        y: 43,
    };
    let visible_pin = api::NetworkPin {
        target: Some(create(&store, &actor, K::Vrf, "Visible VRF")?),
        x: 100,
        y: 50,
    };
    let mut query = graph_query();
    query.network.as_mut().ok_or("network")?.layout = Some(api::NetworkLayout {
        pins: vec![pin.clone(), visible_pin.clone()],
        table: true,
    });
    let R::Views(saved) = call(
        &store,
        &actor,
        C::SaveView(api::SaveView {
            view: Some(api::SavedView {
                name: "Network work".into(),
                query: Some(Box::new(query)),
                audiences: vec!["operations".into()],
                editors: vec!["operations".into()],
                ..Default::default()
            }),
            publish: true,
            ..Default::default()
        }),
    )?
    else {
        return Err("save result".into());
    };
    let original = saved.items.into_iter().next().ok_or("view")?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let R::Views(reopened) = call(&store, &actor, C::Views(api::Empty {}))? else {
        return Err("views".into());
    };
    assert_eq!(reopened.items, vec![original.clone()]);

    let mut editor = operator();
    editor.actor = "other-editor".into();
    editor.grants[0].scope = Scope::Kinds(vec![K::Vrf]);
    let before = store.usage()?;
    let R::Views(restricted) = call(&store, &editor, C::Views(api::Empty {}))? else {
        return Err("views".into());
    };
    let layout = restricted.items[0]
        .query
        .as_ref()
        .and_then(|q| q.network.as_ref())
        .and_then(|network| network.layout.as_ref())
        .ok_or("layout")?;
    assert!(layout.table);
    assert_eq!(layout.pins, vec![visible_pin.clone()]);
    assert_eq!(store.usage()?, before);
    call(
        &store,
        &editor,
        C::SaveView(api::SaveView {
            view: Some(restricted.items[0].clone()),
            publish: true,
            ..Default::default()
        }),
    )?;
    assert_eq!(store.usage()?, before);

    let mut renamed = restricted.items[0].clone();
    renamed.name = "Renamed by restricted editor".into();
    let R::Views(result) = call(
        &store,
        &editor,
        C::SaveView(api::SaveView {
            view: Some(renamed),
            publish: true,
            ..Default::default()
        }),
    )?
    else {
        return Err("save result".into());
    };
    assert_eq!(
        result.items[0]
            .query
            .as_ref()
            .and_then(|q| q.network.as_ref())
            .and_then(|n| n.layout.as_ref())
            .ok_or("layout")?
            .pins,
        vec![visible_pin.clone()]
    );
    let R::Views(author) = call(&store, &actor, C::Views(api::Empty {}))? else {
        return Err("views".into());
    };
    assert_eq!(
        author.items[0]
            .query
            .as_ref()
            .and_then(|q| q.network.as_ref())
            .and_then(|n| n.layout.as_ref())
            .ok_or("layout")?
            .pins,
        vec![pin, visible_pin]
    );

    let mut changed = author.items[0].clone();
    changed.name = "First concurrent edit".into();
    call(
        &store,
        &actor,
        C::SaveView(api::SaveView {
            view: Some(changed),
            publish: true,
            ..Default::default()
        }),
    )?;
    assert!(matches!(
        call(
            &store,
            &editor,
            C::SaveView(api::SaveView {
                view: Some(original),
                publish: true,
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    Ok(())
}

// Deleting a definition only requires current authority over that definition.
#[test]
fn topology_owner_can_remove_a_view_with_an_unreadable_site() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut actor = operator();
    let site = create(&store, &actor, K::Site, "Site")?;
    let mut query = graph_query();
    query.network.as_mut().ok_or("selection")?.sites.push(site);
    let R::Views(saved) = call(
        &store,
        &actor,
        C::SaveView(api::SaveView {
            view: Some(api::SavedView {
                name: "Site view".into(),
                query: Some(Box::new(query)),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )?
    else {
        return Err("saved view".into());
    };
    actor.grants[0].scope = Scope::Kinds(vec![K::Interface]);
    let result = call(
        &store,
        &actor,
        C::SaveView(api::SaveView {
            view: Some(saved.items[0].clone()),
            remove: true,
            ..Default::default()
        }),
    )?;
    assert!(matches!(result, R::Mutation(_)));
    let R::Views(remaining) = call(&store, &actor, C::Views(api::Empty {}))? else {
        return Err("views".into());
    };
    assert!(remaining.items.is_empty());
    Ok(())
}

// Configured allocations are attached through Member rather than Relation.
#[test]
fn topology_projects_authorized_membership_assignments() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let interface = create(&store, &actor, K::Interface, "Management")?;
    let space = create(&store, &actor, K::AddressSpace, "Management space")?;
    let R::Mutation(created) = call(
        &store,
        &actor,
        C::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "Management IP".into(),
            cidr: "192.0.2.3/24".into(),
            address_space: Some(space),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
    )?
    else {
        return Err("address".into());
    };
    let address = created.targets[0].clone();
    call(
        &store,
        &actor,
        C::Member(api::Member {
            address: Some(address.clone()),
            interface: Some(interface.clone()),
            expected: store.asset(AssetRef::try_from(&address)?)?.revision,
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    let edge = graph(&store, &actor, graph_query())?
        .topology
        .ok_or("topology")?
        .edges
        .into_iter()
        .find(|edge| edge.kind == wire::RelationKind::AssignedTo as i32)
        .ok_or("assignment link missing")?;
    assert_eq!(edge.from, Some(address));
    assert_eq!(edge.to, Some(interface));
    let mut reader = operator();
    reader.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    assert!(
        graph(&store, &reader, graph_query())?
            .topology
            .ok_or("topology")?
            .edges
            .is_empty()
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}
