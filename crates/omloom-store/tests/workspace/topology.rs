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
    selector.cursor = first.next;
    assert!(selector.cursor.is_some());
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
    let before = store.usage()?;
    store.set_diagnostic_query_check(|db, statement| {
        if statement.contains("r.to_asset+'/'+r.to_incarnation") {
            let plan = db
                .explain_plan(statement, None)
                .expect("topology plan")
                .explain();
            assert!(
                plan.contains("IndexScan(r:OmRow ON omloom_asset(asset))"),
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
    let mut query = graph_query();
    query.network.as_mut().ok_or("network")?.layout = Some(api::NetworkLayout {
        pins: vec![pin.clone()],
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
    assert!(layout.pins.is_empty());
    assert_eq!(store.usage()?, before);

    let mut changed = original.clone();
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
