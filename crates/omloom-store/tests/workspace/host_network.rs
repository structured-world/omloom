use super::*;
use api::{reply::Result as Reply, request::Command};

// Accepted partial batches must never make the installation inspector exceed its wire budget.
#[test]
fn large_network_evidence_remains_readable_within_reply_budget() -> TestResult {
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let os = create(&store, &actor, K::SoftwareInstallation, "OS")?;
    let other = create(&store, &actor, K::SoftwareInstallation, "Other OS")?;
    for base in (0..2048).step_by(128) {
        scoped(
            &store,
            &actor,
            &os,
            Command::HostNetwork(api::HostNetwork {
                installation: Some(os.clone()),
                source: "collector".into(),
                basis: "b".repeat(128),
                observed_at: 100 + base as u64,
                socket_coverage: "not collected".into(),
                route_coverage: "not collected".into(),
                application_coverage: "verified".into(),
                packet_filter_coverage: "not collected".into(),
                rules: (base..base + 128)
                    .map(|index| api::ApplicationRule {
                        subject: format!("{index:04}{}", "x".repeat(96)),
                        action: "allow incoming".into(),
                    })
                    .collect(),
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    let result = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(os.clone()),
            ..Default::default()
        }),
    )?;
    let bytes = api::Reply {
        result: Some(result.clone()),
    }
    .encoded_len();
    assert!(
        bytes <= MAX_REPLY_BYTES,
        "accepted evidence exceeds transport: {bytes}"
    );
    let Reply::Detail(mut detail) = result else {
        panic!("detail");
    };
    let first_cursor = detail.network_next.clone().ok_or("expected continuation")?;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        assert!(
            api::Reply {
                result: Some(Reply::Detail(detail.clone()))
            }
            .encoded_len()
                <= MAX_REPLY_BYTES
        );
        for source in &detail.host_network {
            for observation in &source.observations {
                let Some(api::network_observation::Value::Rule(rule)) = &observation.value else {
                    panic!("rule");
                };
                let index: usize = rule.subject[..4].parse()?;
                assert!(seen.insert(index));
                assert_eq!(observation.basis, "b".repeat(128));
                assert_eq!(observation.observed_at, 100 + (index / 128 * 128) as u64);
            }
        }
        let Some(cursor) = detail.network_next else {
            break;
        };
        let Reply::Detail(next) = call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(os.clone()),
                network_cursor: Some(cursor),
                ..Default::default()
            }),
        )?
        else {
            panic!("detail");
        };
        assert!(next.item.is_none());
        detail = next;
    }
    assert_eq!(seen.len(), 2048);
    assert_eq!(store.usage()?, before);
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(os.clone()),
                relations_cursor: Some("unused".into()),
                network_cursor: Some(first_cursor.clone())
            })
        ),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(os.clone()),
                relations_cursor: Some(first_cursor.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Expired)
    ));
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(other),
                network_cursor: Some(first_cursor.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Expired)
    ));
    assert!(matches!(
        store.workspace(
            &actor,
            api::Request {
                working: Some(api::WorkingContext {
                    focus: Some(os.clone()),
                    ..Default::default()
                }),
                command: Some(Command::Read(api::Read {
                    target: Some(os.clone()),
                    network_cursor: Some(first_cursor.clone()),
                    ..Default::default()
                }))
            },
            100
        ),
        Err(Error::Expired)
    ));
    let mut changed = actor.clone();
    changed.authority_revision += 1;
    assert!(matches!(
        call(
            &store,
            &changed,
            Command::Read(api::Read {
                target: Some(os.clone()),
                network_cursor: Some(first_cursor.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Expired)
    ));
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(os.clone()),
            expected: Some(store.asset(AssetRef::try_from(&os)?)?.revision),
            kind: K::SoftwareInstallation as i32,
            changes: vec![text_field("description", "changed")],
            ..Default::default()
        }),
    )?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(os),
                network_cursor: Some(first_cursor),
                ..Default::default()
            })
        ),
        Err(Error::Expired)
    ));
    Ok(())
}

// A service name is optional inventory data; socket ownership remains usable without it.
#[test]
fn unnamed_service_network_evidence_remains_readable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let os = create(&store, &actor, K::SoftwareInstallation, "OS")?;
    let Reply::Mutation(created) = scoped(
        &store,
        &actor,
        &os,
        Command::Write(api::Write {
            parent: Some(os.clone()),
            kind: K::Service as i32,
            changes: vec![text_field("description", "unnamed listener")],
            ..Default::default()
        }),
    )?
    else {
        panic!("service");
    };
    let service = created.targets[0].clone();
    scoped(
        &store,
        &actor,
        &os,
        Command::HostNetwork(api::HostNetwork {
            installation: Some(os.clone()),
            source: "collector".into(),
            basis: "boot".into(),
            observed_at: 100,
            socket_coverage: "verified".into(),
            route_coverage: "not collected".into(),
            application_coverage: "not collected".into(),
            packet_filter_coverage: "not collected".into(),
            sockets: vec![api::SocketBind {
                service: Some(service.clone()),
                protocol: "tcp".into(),
                family: "IPv4".into(),
                bind: "192.0.2.10".into(),
                port: 8080,
                ..Default::default()
            }],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    for owner in [&os, &service] {
        let Reply::Detail(detail) = call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(owner.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("detail");
        };
        assert_eq!(detail.host_network[0].observations.len(), 1);
        assert_eq!(
            detail.host_network[0].services[0].target.as_ref(),
            Some(&service)
        );
        assert!(detail.host_network[0].services[0].name.is_empty());
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Bulk retirement must reclaim incarnation-owned observations, not just direct asset children.
#[test]
fn bulk_removal_reclaims_current_network_and_address_facts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    create(&store, &actor, K::Device, "retained authority owner")?;
    let initial = store.usage()?;
    for kind in [K::Interface, K::SoftwareInstallation] {
        let owner = create(&store, &actor, kind, "observed owner")?;
        if kind == K::Interface {
            call(
                &store,
                &actor,
                Command::InterfaceAddresses(api::InterfaceAddresses {
                    interface: Some(owner.clone()),
                    source: "collector".into(),
                    observed_at: 100,
                    complete: true,
                    addresses: vec![api::CurrentAddress {
                        address: "192.0.2.10".into(),
                        stability: api::AddressStability::Stable as i32,
                        state: api::AddressState::Preferred as i32,
                        ..Default::default()
                    }],
                }),
            )?;
        } else {
            call(
                &store,
                &actor,
                Command::HostNetwork(api::HostNetwork {
                    installation: Some(owner.clone()),
                    source: "collector".into(),
                    basis: "boot".into(),
                    observed_at: 100,
                    route_coverage: "verified".into(),
                    socket_coverage: "not collected".into(),
                    application_coverage: "not collected".into(),
                    packet_filter_coverage: "not collected".into(),
                    routes_complete: true,
                    routes: vec![api::LocalRoute {
                        destination: "default".into(),
                        gateway: "192.0.2.1".into(),
                        interface: "en0".into(),
                        family: "IPv4".into(),
                        flags: "UG".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            )?;
        }
        call(
            &store,
            &actor,
            Command::Bulk(api::Bulk {
                targets: vec![owner],
                expected: store.usage()?.revision,
                remove: true,
                changes: vec![],
            }),
        )?;
        let after = store.usage()?;
        let class = omloom_store::Class::Inventory as usize;
        assert_eq!(
            after.rows[class], initial.rows[class],
            "retirement left current rows"
        );
        assert_eq!(
            after.bytes[class], initial.bytes[class],
            "retirement left charged bytes"
        );
    }
    Ok(())
}

fn reconstruct(
    view: &api::HostNetworkView,
) -> Result<api::HostNetwork, Box<dyn std::error::Error>> {
    let mut value = view.coverage.clone().ok_or("coverage")?;
    for o in &view.observations {
        use api::network_observation::Value;
        match o.value.as_ref().ok_or("fact")? {
            Value::Socket(s) => value.sockets.push(s.clone()),
            Value::Route(r) => value.routes.push(r.clone()),
            Value::Rule(r) => value.rules.push(r.clone()),
            Value::Policy(p) => match api::firewall_setting::Kind::try_from(p.kind)? {
                api::firewall_setting::Kind::Enabled => value.firewall_enabled = Some(p.enabled),
                api::firewall_setting::Kind::BlockAll => value.block_all = Some(p.enabled),
                api::firewall_setting::Kind::AllowBuiltinSigned => {
                    value.allow_builtin_signed = Some(p.enabled)
                }
                api::firewall_setting::Kind::AllowDownloadedSigned => {
                    value.allow_downloaded_signed = Some(p.enabled)
                }
                api::firewall_setting::Kind::Unspecified => return Err("policy".into()),
            },
        }
    }
    Ok(value)
}

fn scoped(
    store: &Store,
    actor: &Context,
    parent: &wire::AssetRef,
    command: Command,
) -> Result<Reply, Error> {
    store
        .workspace(
            actor,
            api::Request {
                working: Some(api::WorkingContext {
                    focus: Some(parent.clone()),
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(command),
            },
            200,
        )?
        .result
        .ok_or(Error::Corrupt)
}
fn child(
    store: &Store,
    actor: &Context,
    parent: &wire::AssetRef,
    kind: K,
    name: &str,
) -> Result<wire::AssetRef, Error> {
    let Reply::Mutation(result) = scoped(
        store,
        actor,
        parent,
        Command::Write(api::Write {
            parent: Some(parent.clone()),
            kind: kind as i32,
            changes: vec![text_field("name", name)],
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    result.targets.into_iter().next().ok_or(Error::Corrupt)
}

// Equivalent sets cannot create revisions, and partial observations must accept new facts.
#[test]
fn network_sets_are_canonical_and_partial_sources_are_independent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let host = create(&store, &actor, K::Device, "host")?;
    let os = child(&store, &actor, &host, K::SoftwareInstallation, "OS")?;
    let service = child(&store, &actor, &os, K::Service, "listener")?;
    let mut evidence = api::HostNetwork {
        installation: Some(os.clone()),
        source: "first".into(),
        basis: "boot".into(),
        observed_at: 100,
        socket_coverage: "verified".into(),
        route_coverage: "verified".into(),
        application_coverage: "verified".into(),
        packet_filter_coverage: "blocked".into(),
        sockets_complete: true,
        routes_complete: true,
        rules_complete: true,
        sockets: [8080, 8081]
            .into_iter()
            .map(|port| api::SocketBind {
                service: Some(service.clone()),
                protocol: "tcp".into(),
                family: "IPv4".into(),
                bind: "192.0.2.10".into(),
                port,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    scoped(&store, &actor, &os, Command::HostNetwork(evidence.clone()))?;
    let before = store.usage()?;
    evidence.sockets.reverse();
    scoped(&store, &actor, &os, Command::HostNetwork(evidence.clone()))?;
    assert_eq!(store.usage()?, before, "set reordering must be a no-op");
    evidence.observed_at = 200;
    evidence.sockets_complete = false;
    evidence.sockets.truncate(1);
    scoped(&store, &actor, &os, Command::HostNetwork(evidence.clone()))?;
    evidence.source = "second".into();
    scoped(&store, &actor, &os, Command::HostNetwork(evidence))?;
    let mut q = query();
    q.kinds = vec![K::Service as i32];
    q.include_context = true;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page")
    };
    assert_eq!(page.contexts[0].addresses, vec!["192.0.2.10"]);
    Ok(())
}

// Independent observers must not overwrite or reject one another's interface claims.
#[test]
fn interface_sources_coexist() -> TestResult {
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let interface = create(&store, &actor, K::Interface, "interface")?;
    for source in ["first", "second"] {
        scoped(
            &store,
            &actor,
            &interface,
            Command::InterfaceAddresses(api::InterfaceAddresses {
                interface: Some(interface.clone()),
                source: source.into(),
                observed_at: 100,
                complete: true,
                addresses: vec![api::CurrentAddress {
                    address: "192.0.2.10".into(),
                    ..Default::default()
                }],
            }),
        )?;
    }
    Ok(())
}

// Installation write access cannot erase an omitted service's separately protected evidence.
#[test]
fn complete_collection_cannot_delete_a_denied_service_claim() -> TestResult {
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let os = create(&store, &actor, K::SoftwareInstallation, "OS")?;
    let first = child(&store, &actor, &os, K::Service, "first")?;
    let second = child(&store, &actor, &os, K::Service, "second")?;
    let mut value = api::HostNetwork {
        installation: Some(os.clone()),
        source: "collector".into(),
        basis: "boot".into(),
        observed_at: 100,
        socket_coverage: "verified".into(),
        route_coverage: "verified".into(),
        application_coverage: "verified".into(),
        packet_filter_coverage: "blocked".into(),
        sockets_complete: true,
        sockets: [first.clone(), second]
            .into_iter()
            .map(|service| api::SocketBind {
                service: Some(service),
                protocol: "tcp".into(),
                family: "IPv4".into(),
                bind: "*".into(),
                port: 8080,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    scoped(&store, &actor, &os, Command::HostNetwork(value.clone()))?;
    let mut limited = actor;
    limited.grants[0].scope =
        Scope::Targets(vec![AssetRef::try_from(&os)?, AssetRef::try_from(&first)?]);
    value.observed_at = 200;
    value.sockets.truncate(1);
    let before = store.usage()?;
    assert!(matches!(
        scoped(&store, &limited, &os, Command::HostNetwork(value.clone())),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    value.sockets_complete = false;
    scoped(&store, &limited, &os, Command::HostNetwork(value))?;
    Ok(())
}

// Partial updates retain omitted dates; complete replacement removes only that source's claims.
#[test]
fn partial_addresses_keep_independent_windows_and_complete_source_boundaries() -> TestResult {
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let interface = create(&store, &actor, K::Interface, "interface")?;
    let mut value = api::InterfaceAddresses {
        interface: Some(interface.clone()),
        source: "first".into(),
        observed_at: 100,
        complete: true,
        addresses: vec![api::CurrentAddress {
            address: "192.0.2.10".into(),
            ..Default::default()
        }],
    };
    scoped(
        &store,
        &actor,
        &interface,
        Command::InterfaceAddresses(value.clone()),
    )?;
    value.observed_at = 200;
    value.complete = false;
    value.addresses[0].address = "192.0.2.11".into();
    scoped(
        &store,
        &actor,
        &interface,
        Command::InterfaceAddresses(value.clone()),
    )?;
    value.source = "second".into();
    value.addresses[0].prefix_length = Some(24);
    scoped(
        &store,
        &actor,
        &interface,
        Command::InterfaceAddresses(value.clone()),
    )?;
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(interface.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert_eq!(detail.current_addresses.len(), 2);
    let first = detail
        .current_addresses
        .iter()
        .find(|s| s.coverage.as_ref().is_some_and(|c| c.source == "first"))
        .ok_or("first")?;
    assert_eq!(
        first
            .observations
            .iter()
            .map(|f| (
                f.value.as_ref().expect("value").address.as_str(),
                f.observed_at
            ))
            .collect::<Vec<_>>(),
        vec![("192.0.2.10", Some(100)), ("192.0.2.11", Some(200))]
    );
    value.source = "first".into();
    value.observed_at = 300;
    value.complete = true;
    value.addresses.clear();
    scoped(
        &store,
        &actor,
        &interface,
        Command::InterfaceAddresses(value),
    )?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(interface),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert_eq!(
        detail
            .current_addresses
            .iter()
            .map(|s| s.observations.len())
            .sum::<usize>(),
        1
    );
    let second = detail
        .current_addresses
        .iter()
        .find(|s| s.coverage.as_ref().is_some_and(|c| c.source == "second"))
        .ok_or("second")?;
    assert_eq!(
        second.observations[0]
            .value
            .as_ref()
            .ok_or("value")?
            .prefix_length,
        Some(24)
    );
    assert_eq!(second.observations[0].observed_at, Some(200));
    Ok(())
}

// One service endpoint cannot require an interface or deserialize unrelated routing payloads.
#[test]
fn listener_context_and_detail_do_not_depend_on_other_network_sections() -> TestResult {
    use coordinode_core::graph::types::Value;
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let host = create(&store, &actor, K::Device, "host")?;
    let os = child(&store, &actor, &host, K::SoftwareInstallation, "OS")?;
    let service = child(&store, &actor, &os, K::Service, "listener")?;
    let mut value = api::HostNetwork {
        installation: Some(os.clone()),
        source: "first".into(),
        basis: "boot-a".into(),
        observed_at: 100,
        socket_coverage: "verified".into(),
        route_coverage: "verified".into(),
        application_coverage: "verified".into(),
        packet_filter_coverage: "blocked".into(),
        sockets: vec![api::SocketBind {
            service: Some(service.clone()),
            protocol: "tcp".into(),
            family: "IPv4".into(),
            bind: "192.0.2.10".into(),
            port: 8080,
            ..Default::default()
        }],
        routes: vec![api::LocalRoute {
            destination: "default".into(),
            gateway: "192.0.2.1".into(),
            interface: "en0".into(),
            flags: "UG".into(),
            family: "IPv4".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    scoped(&store, &actor, &os, Command::HostNetwork(value.clone()))?;
    let mut q = query();
    q.kinds = vec![K::Service as i32];
    q.include_context = true;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page")
    };
    assert_eq!(page.contexts[0].addresses, vec!["192.0.2.10"]);
    assert_eq!(page.contexts[0].endpoints, value.sockets);
    value.observed_at = 200;
    value.basis = "boot-b".into();
    value.routes.clear();
    scoped(&store, &actor, &os, Command::HostNetwork(value))?;
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(os.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    let facts = &detail.host_network[0].observations;
    assert!(facts.iter().any(|o| matches!(
        &o.value,
        Some(api::network_observation::Value::Route(_))
    ) && o.observed_at == 100
        && o.basis == "boot-a"));
    assert!(facts.iter().any(|o| matches!(
        &o.value,
        Some(api::network_observation::Value::Socket(_))
    ) && o.observed_at == 200
        && o.basis == "boot-b"));
    // Fault injection proves a service read never decodes another section's body.
    store.diagnostic_database().execute_cypher_shared(
        "MATCH (n:OmRow {layer:'network-route'}) SET n.payload=$bad",
        Some(std::collections::HashMap::from([(
            "bad".into(),
            Value::Binary(vec![255]),
        )])),
        None,
        None,
        None,
    )?;
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(service.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert_eq!(detail.host_network[0].observations.len(), 1);
    assert_eq!(detail.host_network[0].services[0].name, "listener");
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(os),
                ..Default::default()
            })
        ),
        Err(Error::Corrupt)
    ));
    let mut denied = actor;
    denied.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    let Reply::Page(hidden) = call(&store, &denied, Command::Query(q))? else {
        panic!("page")
    };
    assert!(hidden.contexts[0].addresses.is_empty() && hidden.contexts[0].endpoints.is_empty());
    Ok(())
}

// Interface addresses are replaceable source facts, never additional inventory assets or history.
#[test]
fn interface_addresses_replace_current_facts_without_assets() -> TestResult {
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let host = create(&store, &actor, K::Device, "host")?;
    let os = child(&store, &actor, &host, K::SoftwareInstallation, "OS")?;
    let iface = child(&store, &actor, &os, K::Interface, "en0")?;
    let mut value = api::InterfaceAddresses {
        interface: Some(iface.clone()),
        source: "local source".into(),
        observed_at: 100,
        complete: true,
        addresses: vec![api::CurrentAddress {
            address: "2001:db8::1".into(),
            prefix_length: Some(64),
            stability: api::AddressStability::Stable as i32,
            state: api::AddressState::Deprecated as i32,
        }],
    };
    let Reply::Page(before) = call(&store, &actor, Command::Query(query()))? else {
        panic!("page")
    };
    scoped(
        &store,
        &actor,
        &iface,
        Command::InterfaceAddresses(value.clone()),
    )?;
    let unchanged = store.usage()?;
    scoped(
        &store,
        &actor,
        &iface,
        Command::InterfaceAddresses(value.clone()),
    )?;
    assert_eq!(store.usage()?, unchanged);
    value.addresses[0].stability = api::AddressStability::Temporary as i32;
    assert!(matches!(
        scoped(
            &store,
            &actor,
            &iface,
            Command::InterfaceAddresses(value.clone())
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, unchanged);
    value.addresses[0].stability = api::AddressStability::Stable as i32;
    value.addresses[0].address = "2001:db8::2".into();
    value.addresses[0].state = api::AddressState::Preferred as i32;
    value.observed_at = 101;
    scoped(
        &store,
        &actor,
        &iface,
        Command::InterfaceAddresses(value.clone()),
    )?;
    let Reply::Page(after) = call(&store, &actor, Command::Query(query()))? else {
        panic!("page")
    };
    assert_eq!(before.total, after.total);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(iface.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert_eq!(
        detail
            .current_addresses
            .first()
            .ok_or("addresses")?
            .observations
            .iter()
            .map(|o| o.value.clone().ok_or("address"))
            .collect::<Result<Vec<_>, _>>()?,
        value.addresses
    );
    let usage = store.usage()?;
    value.observed_at = 99;
    assert!(matches!(
        scoped(&store, &actor, &iface, Command::InterfaceAddresses(value)),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    let Reply::Detail(hidden) = call(
        &store,
        &restricted,
        Command::Read(api::Read {
            target: Some(iface),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert!(hidden.current_addresses.is_empty());
    Ok(())
}

// Current source evidence survives reopen, is read-only on GET, and does not copy history on replay.
#[test]
fn network_evidence_roundtrip_scope_and_idempotence() -> TestResult {
    let dir = tempfile::tempdir()?;
    let actor = operator();
    let store = Store::open(dir.path(), policy())?;
    let host = create(&store, &actor, K::Device, "host")?;
    let os = child(
        &store,
        &actor,
        &host,
        K::SoftwareInstallation,
        "OS installation",
    )?;
    let service = child(&store, &actor, &os, K::Service, "tunnel")?;
    child(&store, &actor, &service, K::Interface, "tunnel interface")?;
    let evidence = api::HostNetwork {
        installation: Some(os.clone()),
        source: "local source".into(),
        observed_at: 100,
        basis: "boot basis".into(),
        socket_coverage: "verified kernel, partial processes".into(),
        route_coverage: "verified".into(),
        application_coverage: "verified".into(),
        packet_filter_coverage: "not run or blocked: permission denied".into(),
        sockets: vec![api::SocketBind {
            service: Some(service.clone()),
            protocol: "udp".into(),
            family: "IPv4".into(),
            bind: "*".into(),
            port: 12345,
            ..Default::default()
        }],
        routes: vec![api::LocalRoute {
            destination: "default".into(),
            gateway: "192.0.2.1".into(),
            interface: "en0".into(),
            flags: "UGSc".into(),
            family: "IPv4".into(),
            link_layer: false,
        }],
        rules: vec![api::ApplicationRule {
            subject: "/example/app".into(),
            action: "allow incoming".into(),
        }],
        firewall_enabled: Some(true),
        ..Default::default()
    };
    let mut temporary = evidence.clone();
    temporary.sockets[0].bind = "192.0.2.1".into();
    temporary.sockets[0].address_stability = api::AddressStability::Temporary as i32;
    assert!(matches!(
        scoped(&store, &actor, &os, Command::HostNetwork(temporary)),
        Err(Error::Invalid)
    ));
    scoped(&store, &actor, &os, Command::HostNetwork(evidence.clone()))?;
    let before = store.usage()?;
    scoped(&store, &actor, &os, Command::HostNetwork(evidence.clone()))?;
    assert_eq!(store.usage()?, before);
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(os.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert_eq!(reconstruct(&detail.host_network[0])?, evidence);
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(service.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let selected = reconstruct(detail.host_network.first().ok_or("service evidence")?)?;
    assert_eq!(selected.sockets, evidence.sockets);
    assert!(selected.routes.is_empty() && selected.rules.is_empty());
    assert_eq!(store.usage()?, before);
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    let Reply::Detail(detail) = call(
        &store,
        &restricted,
        Command::Read(api::Read {
            target: Some(os.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert!(detail.host_network.is_empty());
    let mut invalid = evidence.clone();
    invalid.sockets[0].port = 65536;
    assert!(matches!(
        scoped(&store, &actor, &os, Command::HostNetwork(invalid)),
        Err(Error::Invalid)
    ));
    let mut invalid = evidence.clone();
    invalid.sockets[0].family = "IPv6".into();
    invalid.sockets[0].bind = "::1%bad\nzone".into();
    assert!(matches!(
        scoped(&store, &actor, &os, Command::HostNetwork(invalid)),
        Err(Error::Invalid)
    ));
    let foreign = create(&store, &actor, K::Service, "outside")?;
    let mut invalid = evidence.clone();
    invalid.sockets[0].service = Some(foreign);
    assert!(matches!(
        scoped(&store, &actor, &os, Command::HostNetwork(invalid)),
        Err(Error::Denied)
    ));
    let mut incomplete = evidence.clone();
    incomplete.observed_at = 101;
    incomplete.rules.clear();
    scoped(&store, &actor, &os, Command::HostNetwork(incomplete))?;
    // Freshness changes only the metadata row and adds no payload or audit history.
    let before_refresh = store.usage()?;
    let mut refreshed = evidence.clone();
    refreshed.observed_at += 2;
    scoped(&store, &actor, &os, Command::HostNetwork(refreshed.clone()))?;
    let after_refresh = store.usage()?;
    assert_eq!(after_refresh.rows, before_refresh.rows);
    assert_eq!(after_refresh.bytes, before_refresh.bytes);
    drop(store);
    let reopened = Store::open(dir.path(), policy())?;
    let Reply::Detail(detail) = call(
        &reopened,
        &actor,
        Command::Read(api::Read {
            target: Some(os),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert_eq!(reconstruct(&detail.host_network[0])?, refreshed);
    Ok(())
}
