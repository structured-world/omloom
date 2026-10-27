use super::*;
use api::{reply::Result as Reply, request::Command};

// Destination knowledge remains an address fact, never an interface's or service's local address.
#[test]
fn destination_references_do_not_become_local_address_hints() -> TestResult {
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let service = create(&store, &actor, K::Service, "application")?;
    let interface = child(&store, &actor, &service, K::Interface, "eth0")?;
    let mut destinations = Vec::new();
    for (parent, address, meaning) in [
        (&interface, "192.0.2.10", api::AddressMeaning::Configured),
        (
            &interface,
            "198.51.100.10",
            api::AddressMeaning::Unspecified,
        ),
        (&interface, "203.0.113.20", api::AddressMeaning::Destination),
        (&service, "203.0.113.30", api::AddressMeaning::Destination),
    ] {
        let api::reply::Result::Mutation(created) = store
            .workspace(
                &actor,
                api::Request {
                    working: Some(api::WorkingContext {
                        focus: Some(parent.clone()),
                        expected: Some(store.usage()?.revision),
                        ..Default::default()
                    }),
                    command: Some(Command::Address(api::Address {
                        parent: Some(parent.clone()),
                        address: Some(address.into()),
                        meaning: Some(meaning as i32),
                        ..Default::default()
                    })),
                },
                100,
            )?
            .result
            .ok_or("address result")?
        else {
            panic!("address")
        };
        if meaning == api::AddressMeaning::Destination {
            destinations.push((created.targets[0].clone(), address));
        }
    }
    let mut q = query();
    q.kinds = vec![K::Service as i32, K::Interface as i32];
    q.include_context = true;
    let before = store.usage()?;
    let mut samples = Vec::new();
    let mut last = None;
    for _ in 0..9 {
        let start = std::time::Instant::now();
        let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
            panic!("page")
        };
        samples.push(start.elapsed());
        last = Some(page);
    }
    samples.sort_unstable();
    let page = last.expect("page");
    eprintln!(
        "destination hint projection: median={:?} max_sample={:?} response_bytes={} logical_bytes={}",
        samples[4],
        samples[8],
        page.encoded_len(),
        before.bytes.iter().sum::<u64>()
    );
    assert_eq!(page.contexts.len(), 2);
    for context in page.contexts {
        assert_eq!(
            context.addresses,
            ["192.0.2.10", "198.51.100.10"],
            "remote destinations must not appear as local addresses"
        );
    }
    for (target, address) in destinations {
        let Reply::Detail(detail) = call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(target),
                ..Default::default()
            }),
        )?
        else {
            panic!("destination detail")
        };
        let fields = detail.item.expect("destination fact").fields;
        assert!(fields.contains(&text_field("address", address)));
        assert!(fields.contains(&text_field(
            "meaning",
            api::AddressMeaning::Destination.as_str_name()
        )));
        let mut search = query();
        search.kinds = vec![K::AddressReference as i32];
        search.search = address.into();
        let Reply::Page(found) = call(&store, &actor, Command::Query(search))? else {
            panic!("search")
        };
        assert_eq!(found.total, 1, "the destination remains searchable");
    }
    let mut limited = actor.clone();
    limited.grants[0].read =
        Fields::Selected(vec!["name".into(), "address".into(), "relations".into()]);
    let Reply::Page(hidden) = call(&store, &limited, Command::Query(q))? else {
        panic!("restricted page")
    };
    assert!(
        hidden.contexts.iter().all(|c| c.addresses.is_empty()),
        "classification cannot bypass meaning-field visibility"
    );
    assert_eq!(
        store.usage()?,
        before,
        "hint filtering cannot mutate or remove facts"
    );
    Ok(())
}

// Direct and shared assignments are visible hints only through authorized interface membership.
#[test]
fn membership_address_hints_cover_interfaces_services_and_field_permissions() -> TestResult {
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let mut expected = Vec::new();
    for index in 0..25 {
        let service = create(&store, &actor, K::Service, &format!("service {index:02}"))?;
        let interface = child(
            &store,
            &actor,
            &service,
            K::Interface,
            &format!("interface {index:02}"),
        )?;
        let Reply::Mutation(created) = call(
            &store,
            &actor,
            Command::Network(api::Network {
                kind: K::IpAddress as i32,
                name: format!("address {index:02}"),
                cidr: format!("192.0.2.{}/32", index + 1),
                address_space: Some(space.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("allocation")
        };
        let address = created.targets[0].clone();
        call(
            &store,
            &actor,
            Command::Member(api::Member {
                address: Some(address.clone()),
                interface: Some(interface.clone()),
                expected: store.asset(AssetRef::try_from(&address)?)?.revision,
                ..Default::default()
            }),
        )?;
        expected.push((service, interface, format!("192.0.2.{}", index + 1)));
    }
    let Reply::Mutation(created) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "shared first".into(),
            cidr: "198.51.100.1/32".into(),
            address_space: Some(space.clone()),
            sharing: api::SharedUse::Anycast as i32,
            group_name: "shared".into(),
            ..Default::default()
        }),
    )?
    else {
        panic!("shared address")
    };
    let first = created.targets[0].clone();
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(first.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("group")
    };
    let group = detail.group.expect("group");
    call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "shared second".into(),
            cidr: "198.51.100.2/32".into(),
            address_space: Some(space),
            group: group.target.clone(),
            expected_group: Some(group.revision),
            sharing: api::SharedUse::Unspecified as i32,
            ..Default::default()
        }),
    )?;
    let group = group.target.expect("group target");
    call(
        &store,
        &actor,
        Command::Member(api::Member {
            address: Some(group.clone()),
            interface: Some(expected[0].1.clone()),
            expected: store.asset(AssetRef::try_from(&group)?)?.revision,
            ..Default::default()
        }),
    )?;
    store.diagnostic_database().persist()?;
    store.diagnostic_database().query_registry().reset();
    let before = store.usage()?;
    let mut pages = Vec::new();
    for kind in [K::Interface, K::Service] {
        let mut q = query();
        q.kinds = vec![kind as i32];
        q.limit = 25;
        q.include_context = true;
        let mut samples = Vec::new();
        let mut last = None;
        for _ in 0..9 {
            let start = std::time::Instant::now();
            let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
                panic!("page")
            };
            samples.push(start.elapsed());
            last = Some(page);
        }
        samples.sort_unstable();
        let page = last.expect("page");
        eprintln!(
            "membership hints {kind:?}: median={:?} max_sample={:?} response_bytes={} logical_bytes={}",
            samples[4],
            samples[8],
            page.encoded_len(),
            before.bytes.iter().sum::<u64>()
        );
        pages.push(page);
    }
    assert_eq!(store.usage()?, before, "projections must not write");
    let plans = store
        .diagnostic_database()
        .query_registry()
        .top_by_impact(64);
    for index in ["omloom_asset(asset)", "omloom_group(group)"] {
        assert!(
            plans
                .iter()
                .any(|q| q.last_plan.as_ref().is_some_and(|p| p.contains(index))),
            "membership hints must use {index}"
        );
    }
    for (index, page) in pages.iter().enumerate() {
        for (service, interface, address) in &expected {
            let target = if index == 0 { interface } else { service };
            let hints = &page
                .contexts
                .iter()
                .find(|c| c.target.as_ref() == Some(target))
                .expect("context")
                .addresses;
            let mut wanted = vec![address.clone()];
            if target
                == (if index == 0 {
                    &expected[0].1
                } else {
                    &expected[0].0
                })
            {
                wanted.extend(["198.51.100.1".into(), "198.51.100.2".into()]);
            }
            assert_eq!(
                hints, &wanted,
                "assignments must be visible at their actual interface and service"
            );
        }
    }
    // Neither an interface-only grant nor hidden network/cidr fields can reveal allocations.
    for hidden in ["network", "cidr", "relations", "group", "asset"] {
        let mut limited = actor.clone();
        limited.grants = vec![Grant {
            scope: if hidden == "asset" {
                Scope::Kinds(vec![K::Service, K::Interface])
            } else {
                Scope::All
            },
            read: Fields::Selected(
                ["name", "relations", "network", "cidr"]
                    .into_iter()
                    .filter(|f| *f != hidden)
                    .map(str::to_owned)
                    .collect(),
            ),
            write: Fields::All,
            writable: true,
        }];
        if hidden == "group" {
            limited.grants[0].scope = Scope::Kinds(vec![K::Service, K::Interface, K::IpAddress]);
        }
        let mut q = query();
        q.kinds = vec![K::Interface as i32];
        q.limit = 25;
        q.include_context = true;
        let Reply::Page(page) = call(&store, &limited, Command::Query(q))? else {
            panic!("limited page")
        };
        for context in page.contexts {
            if hidden == "group" {
                assert!(
                    context
                        .addresses
                        .iter()
                        .all(|ip| !ip.starts_with("198.51.100."))
                );
            } else {
                assert!(
                    context.addresses.is_empty(),
                    "hidden {hidden} leaked an address"
                );
            }
        }
    }
    Ok(())
}

// Domain categories filter before pagination; host labels do not make devices services.
#[test]
fn service_category_does_not_return_hardware() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    create(&store, &actor, K::Device, "ordinary host")?;
    create(&store, &actor, K::Service, "application")?;
    let mut q = query();
    q.view = api::View::Services as i32;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page")
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, K::Service as i32);
    Ok(())
}

fn child(
    store: &Store,
    actor: &Context,
    parent: &wire::AssetRef,
    kind: K,
    name: &str,
) -> Result<wire::AssetRef, Error> {
    let result = store
        .workspace(
            actor,
            api::Request {
                working: Some(api::WorkingContext {
                    focus: Some(parent.clone()),
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(Command::Write(api::Write {
                    parent: Some(parent.clone()),
                    kind: kind as i32,
                    changes: vec![text_field("name", name)],
                    ..Default::default()
                })),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)?;
    let Reply::Mutation(m) = result else {
        return Err(Error::Corrupt);
    };
    m.targets.into_iter().next().ok_or(Error::Corrupt)
}

// Large local containers must remain readable and editable through bounded pages.
#[test]
fn dense_container_opens_without_enumerating_its_children() -> TestResult {
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let host = create(&store, &actor, K::Device, "dense host")?;
    for i in 0..129 {
        child(&store, &actor, &host, K::Port, &format!("port {i:03}"))?;
    }
    let before = store.usage()?;
    let result = store
        .workspace(
            &actor,
            api::Request {
                working: Some(api::WorkingContext {
                    focus: Some(host.clone()),
                    ..Default::default()
                }),
                command: Some(Command::Read(api::Read {
                    target: Some(host.clone()),
                    ..Default::default()
                })),
            },
            100,
        )?
        .result
        .ok_or("detail")?;
    let Reply::Detail(detail) = result else {
        panic!("detail")
    };
    assert!(detail.relations.len() <= 25);
    let mut seen = std::collections::BTreeSet::new();
    let mut detail = detail;
    loop {
        for relation in &detail.relations {
            assert!(seen.insert(relation.relation_id.clone()));
        }
        let Some(cursor) = detail.relations_next else {
            break;
        };
        let Reply::Detail(next) = store
            .workspace(
                &actor,
                api::Request {
                    working: Some(api::WorkingContext {
                        focus: Some(host.clone()),
                        ..Default::default()
                    }),
                    command: Some(Command::Read(api::Read {
                        target: Some(host.clone()),
                        relations_cursor: Some(cursor),
                        network_cursor: None,
                    })),
                },
                100,
            )?
            .result
            .ok_or("detail")?
        else {
            panic!("detail")
        };
        assert!(next.host_network.is_empty() && next.current_addresses.is_empty());
        detail = next;
    }
    assert_eq!(seen.len(), 129);
    let mut q = query();
    q.focus = Some(host.clone());
    q.level_only = true;
    q.limit = 25;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page")
    };
    assert_eq!(page.total, 129);
    assert_eq!(page.items.len(), 25);
    assert!(page.next.is_some());
    assert_eq!(store.usage()?, before);
    child(&store, &actor, &host, K::Port, "another port")?;
    Ok(())
}

// Owner hints must follow parents, without hydrating an unselected sibling's contents.
#[test]
fn service_owner_lookup_does_not_read_sibling_resources() -> TestResult {
    use coordinode_core::graph::types::Value;
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let host = create(&store, &actor, K::Device, "host")?;
    let os = child(&store, &actor, &host, K::SoftwareInstallation, "OS")?;
    let service = child(&store, &actor, &os, K::Service, "selected listener")?;
    let sibling = child(&store, &actor, &os, K::Service, "unselected sibling")?;
    // Unrelated corrupt content must not enter this selected resource's projection.
    store.diagnostic_database().execute_cypher_shared(
        "MATCH (n:OmRow {key:$key}) SET n.kind=999",
        Some(std::collections::HashMap::from([(
            "key".into(),
            Value::String(format!("asset/{}", sibling.asset_id)),
        )])),
        None,
        None,
        None,
    )?;
    let mut q = query();
    q.search = "selected listener".into();
    q.include_context = true;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page")
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&service));
    assert_eq!(
        page.contexts[0]
            .owners
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        vec!["host", "OS"]
    );
    Ok(())
}

// Browsing must not flatten ports into the host list or enumerate a whole subtree.
#[test]
fn level_browse_returns_containers_then_direct_children() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let host = create(&store, &actor, K::Device, "host")?;
    let port = child(&store, &actor, &host, K::Port, "socket")?;
    let board = child(&store, &actor, &host, K::Device, "board")?;
    child(&store, &actor, &board, K::Port, "nested socket")?;
    let mut q = query();
    q.view = api::View::Hardware as i32;
    q.level_only = true;
    q.limit = 1;
    let before = store.usage()?;
    let Reply::Page(top) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(top.total, 1);
    assert_eq!(top.items[0].target.as_ref(), Some(&host));
    assert!(top.next.is_none());
    q.view = api::View::Network as i32;
    let Reply::Page(network) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(network.total, 0);
    q.view = api::View::Hardware as i32;
    q.roots = vec![host.clone()];
    let Reply::Page(first) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(first.total, 2);
    assert_eq!(first.items[0].target.as_ref(), Some(&board));
    q.cursor = first.next;
    let Reply::Page(second) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(second.items[0].target.as_ref(), Some(&port));
    assert!(second.next.is_none());
    q.cursor = None;
    q.search = "socket".into();
    let Reply::Page(filtered) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.items[0].target.as_ref(), Some(&port));
    q.search.clear();
    q.focus = Some(board.clone());
    let Reply::Page(nested) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(nested.total, 1);
    assert_eq!(
        nested.items[0].fields[0],
        text_field("name", "nested socket")
    );
    q.focus = Some(port.clone());
    q.roots = vec![board.clone()];
    assert!(matches!(
        call(&store, &actor, Command::Query(q.clone())),
        Err(Error::Denied)
    ));
    q.focus = None;
    assert_eq!(store.usage()?, before);
    // A hidden owner cannot hide an otherwise permitted component or leak its name.
    let mut limited = actor.clone();
    limited.grants[0].scope = Scope::Kinds(vec![K::Port]);
    q.roots.clear();
    q.cursor = None;
    let Reply::Page(visible) = call(&store, &limited, Command::Query(q))? else {
        panic!("page")
    };
    assert_eq!(visible.total, 2);
    Ok(())
}

// A small estate must not re-expand the estate for every candidate's parent probe.
#[test]
fn level_browse_small_estate_has_bounded_latency() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    for index in 0..16 {
        let host = create(&store, &actor, K::Device, &format!("host {index}"))?;
        for port in 0..2 {
            child(
                &store,
                &actor,
                &host,
                K::Port,
                &format!("port {index}/{port}"),
            )?;
        }
    }
    let db = store.diagnostic_database();
    db.persist()?;
    db.query_registry().reset();
    let before = store.usage()?;
    let mut q = query();
    q.level_only = true;
    q.include_context = true;
    q.omit_host_local = true;
    q.view = api::View::Hardware as i32;
    let start = std::time::Instant::now();
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page")
    };
    let elapsed = start.elapsed();
    let mut browse_plans = 0;
    for query in db.query_registry().top_by_impact(32) {
        if query.canonical_query.contains("forward_owned") {
            browse_plans += 1;
            let plan = query.last_plan.ok_or("missing browse plan")?;
            assert_eq!(
                plan.matches("IndexScan(probe:OmRow ON omloom_key(key))")
                    .count(),
                2,
                "parent probes must use point indexes: {plan}"
            );
            eprintln!("browse statement: {} us", query.total_time_us);
        }
    }
    assert_eq!(browse_plans, 2, "count and page plans must both be checked");
    eprintln!("level browse elapsed: {elapsed:?}");
    assert_eq!(page.total, 16);
    assert_eq!(page.items.len(), 16);
    assert_eq!(store.usage()?, before);
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "small estate browse: {elapsed:?}"
    );
    Ok(())
}

// One hundred hosts must still return one bounded page without their two hundred ports.
#[test]
fn level_browse_hundred_hosts_only_projects_one_page() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    for index in 0..100 {
        let host = create(&store, &actor, K::Device, &format!("host {index}"))?;
        for port in 0..2 {
            child(
                &store,
                &actor,
                &host,
                K::Port,
                &format!("port {index}/{port}"),
            )?;
        }
    }
    store.diagnostic_database().persist()?;
    let before = store.usage()?;
    let mut q = query();
    q.limit = 25;
    q.view = api::View::Hardware as i32;
    q.level_only = true;
    q.include_context = true;
    q.omit_host_local = true;
    let start = std::time::Instant::now();
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page")
    };
    let elapsed = start.elapsed();
    eprintln!("hundred-host browse elapsed: {elapsed:?}");
    assert_eq!(page.total, 100);
    assert_eq!(page.items.len(), 25);
    assert!(page.next.is_some());
    assert!(page.items.iter().all(|item| item.kind == K::Device as i32));
    assert!(
        page.contexts
            .iter()
            .all(|context| context.addresses.is_empty())
    );
    assert_eq!(store.usage()?, before);
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "hundred-host browse: {elapsed:?}"
    );
    Ok(())
}

// Owner labels must not trigger address traversal through ancestors and sibling services.
#[test]
fn bounded_owner_and_address_projection_disambiguates_services() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut services = Vec::new();
    for (name, ip) in [("first", "192.0.2.10"), ("second", "198.51.100.10")] {
        let host = create(&store, &actor, K::Device, name)?;
        let os = child(
            &store,
            &actor,
            &host,
            K::SoftwareInstallation,
            &format!("{name} OS"),
        )?;
        let service = child(&store, &actor, &os, K::Service, "nginx")?;
        let iface = child(&store, &actor, &os, K::Interface, "eth0")?;
        store.workspace(
            &actor,
            api::Request {
                working: Some(api::WorkingContext {
                    focus: Some(iface.clone()),
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(Command::Address(api::Address {
                    parent: Some(iface),
                    address: Some(ip.into()),
                    ..Default::default()
                })),
            },
            100,
        )?;
        services.push(service);
    }
    let before = store.usage()?;
    let mut q = query();
    q.view = api::View::Services as i32;
    q.kinds = vec![K::Service as i32];
    q.include_context = true;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(page.total, 2);
    for (i, service) in services.iter().enumerate() {
        let c = page
            .contexts
            .iter()
            .find(|c| c.target.as_ref() == Some(service))
            .ok_or("context")?;
        assert_eq!(
            c.owners.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
            if i == 0 {
                vec!["first", "first OS"]
            } else {
                vec!["second", "second OS"]
            }
        );
        assert!(c.addresses.is_empty());
    }
    assert_eq!(store.usage()?, before);
    let mut restricted = actor.clone();
    restricted.grants[0].scope = Scope::Kinds(vec![K::Service]);
    let Reply::Page(page) = call(&store, &restricted, Command::Query(q))? else {
        panic!("page");
    };
    assert!(
        page.contexts
            .iter()
            .all(|c| c.owners.is_empty() && c.addresses.is_empty())
    );
    Ok(())
}

// Local address suppression happens before the count and page limit, preserving LAN addresses.
#[test]
fn ordinary_projection_omits_host_local_before_pagination() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    for address in [
        "127.0.0.1",
        "127.10.20.30",
        "::1",
        "169.254.1.2",
        "fe80::1",
        "192.168.1.5",
        "2001:db8::1",
    ] {
        call(
            &store,
            &actor,
            Command::Address(api::Address {
                address: Some(address.into()),
                ..Default::default()
            }),
        )?;
    }
    let mut q = query();
    q.omit_host_local = true;
    q.limit = 1;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(page.total, 2);
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_some());
    q.cursor = page.next;
    let Reply::Page(last) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(last.total, 2);
    assert_eq!(last.items.len(), 1);
    assert!(last.next.is_none());
    Ok(())
}

// A distributed logical service may have several owners, without inventing a containment chain.
#[test]
fn shared_service_context_preserves_multiple_owners() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let service = create(&store, &actor, K::Service, "shared service")?;
    for name in ["first host", "second host"] {
        let host = create(&store, &actor, K::Device, name)?;
        call(
            &store,
            &actor,
            Command::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(service.clone()),
                    to: Some(host),
                    kind: wire::RelationKind::RunsOn as i32,
                    ..Default::default()
                }),
                remove: false,
            }),
        )?;
    }
    let mut q = query();
    q.kinds = vec![K::Service as i32];
    q.include_context = true;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(page.contexts.len(), 1);
    assert!(page.contexts[0].owners.is_empty());
    assert_eq!(page.contexts[0].alternative_owners.len(), 2);
    Ok(())
}

// A workstation and unrelated storage are not virtualization resources; a role is explicit.
#[test]
fn virtualization_requires_an_explicit_host_role() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    create(&store, &actor, K::Device, "workstation with Docker")?;
    create(&store, &actor, K::Storage, "unrelated storage")?;
    let host = create(&store, &actor, K::Device, "virtualization host")?;
    create(&store, &actor, K::VirtualMachine, "guest")?;
    let mut q = query();
    q.view = api::View::Virtualization as i32;
    let Reply::Page(before) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(before.total, 1);
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(host.clone()),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(
                "virtualization_role",
                "VIRTUALIZATION_ROLE_HYPERVISOR",
            )],
            ..Default::default()
        }),
    )?;
    let Reply::Page(after) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(after.total, 2);
    assert!(after.items.iter().any(|i| i.target.as_ref() == Some(&host)));
    // A full-intent writer must preserve the role projection on an unrelated edit.
    let target = AssetRef::try_from(&host)?;
    let mut accepted = store.asset(target)?;
    accepted.intended.push(wire::IntendedField {
        field: Some(text_field("description", "updated description")),
        authority_id: accepted.intended[0].authority_id.clone(),
    });
    let expected = accepted.revision;
    let record = admit(wire::record::Body::Asset(accepted));
    store.save_asset(&record, Some(expected), "operator", 200)?;
    let Reply::Page(edited) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(edited.total, 2);
    assert!(
        edited
            .items
            .iter()
            .any(|i| i.target.as_ref() == Some(&host))
    );
    let unchanged = store.usage()?;
    store.save_asset(&record, Some(expected + 1), "operator", 201)?;
    assert_eq!(store.usage()?, unchanged);
    let mut accepted = store.asset(target)?;
    let expected = accepted.revision;
    accepted.intended.retain(|value| {
        value
            .field
            .as_ref()
            .is_none_or(|f| f.name != "virtualization_role")
    });
    store.save_asset(
        &admit(wire::record::Body::Asset(accepted)),
        Some(expected),
        "operator",
        202,
    )?;
    let Reply::Page(cleared) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(cleared.total, 1);
    Ok(())
}
