use super::*;

// Archival must preserve usable namespaces and participant identities referenced by current IPAM.
#[test]
fn archival_rejects_inbound_ipam_references() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let link = create(&store, &actor, K::L2Domain, "LAN")?;
    let routing = create(&store, &actor, K::Vrf, "routing")?;
    let interface = create(&store, &actor, K::Interface, "eth0")?;
    let Reply::Mutation(created) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "address".into(),
            cidr: "192.0.2.1/24".into(),
            address_space: Some(space.clone()),
            link: Some(link.clone()),
            routing: Some(routing.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
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
    for target in [&space, &link, &routing, &interface] {
        let target = AssetRef::try_from(target)?;
        let mut asset = store.asset(target)?;
        let revision = asset.revision;
        asset.adoption = wire::Adoption::Archived as i32;
        let before = store.usage()?;
        assert!(matches!(
            store.save_asset(
                &admit(wire::record::Body::Asset(asset)),
                Some(revision),
                "operator",
                200
            ),
            Err(Error::Conflict)
        ));
        assert_eq!(store.usage()?, before);
        assert_eq!(
            store.asset(target)?.adoption,
            wire::Adoption::Adopted as i32
        );
        let Reply::Detail(detail) = call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(address.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("detail");
        };
        assert_eq!(detail.participants[0].interface.as_ref(), Some(&interface));
    }
    Ok(())
}

// An empty shared group still owns its L2 protocol scope after the last address is removed.
#[test]
fn archival_rejects_address_group_link_reference() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let link = create(&store, &actor, K::L2Domain, "LAN")?;
    let Reply::Mutation(created) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "VIP".into(),
            cidr: "192.0.2.1/24".into(),
            address_space: Some(space),
            link: Some(link.clone()),
            sharing: api::SharedUse::Vrrp as i32,
            group_name: "routers".into(),
            vrid: Some(7),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    call(
        &store,
        &actor,
        Command::Bulk(api::Bulk {
            targets: vec![created.targets[0].clone()],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    let target = AssetRef::try_from(&link)?;
    let mut asset = store.asset(target)?;
    let revision = asset.revision;
    asset.adoption = wire::Adoption::Archived as i32;
    let before = store.usage()?;
    assert!(matches!(
        store.save_asset(
            &admit(wire::record::Body::Asset(asset)),
            Some(revision),
            "operator",
            200
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Network-only grants may edit settings while an echoed, readable name stays unchanged.
#[test]
fn network_settings_do_not_require_unchanged_name_permission() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let link = create(&store, &actor, K::L2Domain, "LAN")?;
    let mut limited = actor.clone();
    limited.grants[0].write = Fields::Selected(vec!["network".into()]);
    for mut request in [
        api::Network {
            kind: K::IpAddress as i32,
            name: "Address".into(),
            cidr: "192.0.2.10/32".into(),
            address_space: Some(space.clone()),
            ..Default::default()
        },
        api::Network {
            kind: K::Vlan as i32,
            name: "VLAN".into(),
            vlan_id: Some(10),
            link: Some(link.clone()),
            ..Default::default()
        },
    ] {
        let Reply::Mutation(created) = call(&store, &actor, Command::Network(request.clone()))?
        else {
            panic!("network");
        };
        let target = created.targets[0].clone();
        request.target = Some(target.clone());
        request.expected = Some(store.asset(AssetRef::try_from(&target)?)?.revision);
        if request.kind == K::IpAddress as i32 {
            request.cidr = "192.0.2.11/32".into();
        } else {
            request.vlan_id = Some(11);
        }
        call(&store, &limited, Command::Network(request.clone()))?;
        request.expected = Some(store.asset(AssetRef::try_from(&target)?)?.revision);
        let before = store.usage()?;
        call(&store, &limited, Command::Network(request.clone()))?;
        assert_eq!(store.usage()?, before, "settings replay cannot write");
        request.name = "Forbidden rename".into();
        assert!(matches!(
            call(&store, &limited, Command::Network(request)),
            Err(Error::Denied)
        ));
        assert_eq!(store.usage()?, before);
    }
    let Reply::Mutation(seed) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "VIP".into(),
            cidr: "198.51.100.1/24".into(),
            address_space: Some(space),
            link: Some(link.clone()),
            sharing: api::SharedUse::Vrrp as i32,
            group_name: "Gateway group".into(),
            vrid: Some(7),
            ..Default::default()
        }),
    )?
    else {
        panic!("seed");
    };
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(seed.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let group = detail.group.ok_or("group")?.target.ok_or("target")?;
    let mut request = api::Network {
        kind: K::AddressGroup as i32,
        target: Some(group.clone()),
        expected: Some(store.asset(AssetRef::try_from(&group)?)?.revision),
        name: "Gateway group".into(),
        link: Some(link),
        sharing: api::SharedUse::Vrrp as i32,
        vrid: Some(8),
        ..Default::default()
    };
    call(&store, &limited, Command::Network(request.clone()))?;
    request.expected = Some(store.asset(AssetRef::try_from(&group)?)?.revision);
    request.name = "Forbidden group rename".into();
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &limited, Command::Network(request)),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Archival cannot hide an active allocation or silently release its reserved address.
#[test]
fn archival_refuses_live_ipam_state_without_changes() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let request = api::Network {
        kind: K::IpAddress as i32,
        name: "retained allocation".into(),
        cidr: "192.0.2.10/32".into(),
        address_space: Some(space),
        ..Default::default()
    };
    let Reply::Mutation(created) = call(&store, &actor, Command::Network(request.clone()))? else {
        panic!("allocation");
    };
    let target = AssetRef::try_from(&created.targets[0])?;
    let mut asset = store.asset(target)?;
    let expected = asset.revision;
    asset.adoption = wire::Adoption::Archived as i32;
    let before = store.usage()?;
    let result = store.save_asset(
        &admit(wire::record::Body::Asset(asset)),
        Some(expected),
        "operator",
        200,
    );
    assert!(
        matches!(result, Err(Error::Conflict)),
        "active allocation archived: {result:?}"
    );
    assert_eq!(store.usage()?, before);
    assert_eq!(
        store.asset(target)?.adoption,
        wire::Adoption::Adopted as i32
    );
    assert!(matches!(
        call(&store, &actor, Command::Network(request)),
        Err(Error::Conflict)
    ));
    Ok(())
}

// A link-local prefix supplies its L2 domain; joining a group accepts only that same domain.
#[test]
fn prefix_allocation_accepts_matching_existing_vrrp_link() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let link = create(&store, &actor, K::L2Domain, "prefix LAN")?;
    let other = create(&store, &actor, K::L2Domain, "other LAN")?;
    let Reply::Mutation(prefix) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::Prefix as i32,
            cidr: "fe80::/64".into(),
            name: "link-local prefix".into(),
            address_space: Some(space.clone()),
            link: Some(link.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix");
    };
    let prefix = prefix.targets[0].clone();
    for (index, group_link) in [other, link.clone()].into_iter().enumerate() {
        let Reply::Mutation(seed) = call(
            &store,
            &actor,
            Command::Network(api::Network {
                kind: K::IpAddress as i32,
                cidr: "fe80::a/64".into(),
                name: "group seed".into(),
                address_space: Some(space.clone()),
                link: Some(group_link.clone()),
                sharing: api::SharedUse::Vrrp as i32,
                vrid: Some(7),
                group_name: format!("group {index}"),
                ..Default::default()
            }),
        )?
        else {
            panic!("seed");
        };
        let Reply::Detail(seed) = call(
            &store,
            &actor,
            Command::Read(api::Read {
                target: Some(seed.targets[0].clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("detail");
        };
        let group = seed.group.ok_or("group")?;
        let before = store.usage()?;
        let result = store.workspace(
            &actor,
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(before.revision),
                    ..Default::default()
                }),
                command: Some(Command::Network(api::Network {
                    kind: K::IpAddress as i32,
                    cidr: "fe80::b/64".into(),
                    name: "prefix VIP".into(),
                    parent: Some(Box::new(prefix.clone())),
                    group: group.target.clone(),
                    expected_group: Some(group.revision),
                    sharing: api::SharedUse::Unspecified as i32,
                    ..Default::default()
                })),
            },
            100,
        );
        if group_link != link {
            assert!(
                matches!(result, Err(Error::Invalid)),
                "foreign L2 accepted: {result:?}"
            );
            assert_eq!(store.usage()?, before);
        } else {
            let Reply::Mutation(created) = result
                .expect("matching prefix allocation")
                .result
                .ok_or("result")?
            else {
                panic!("allocation");
            };
            let Reply::Detail(created) = call(
                &store,
                &actor,
                Command::Read(api::Read {
                    target: Some(created.targets[0].clone()),
                    ..Default::default()
                }),
            )?
            else {
                panic!("detail");
            };
            assert_eq!(created.group.ok_or("group")?.target, group.target);
            assert!(created.item.ok_or("item")?.fields.contains(&text_field(
                "link_scope",
                &format!("{}/{}", link.asset_id, link.incarnation)
            )));
        }
    }
    Ok(())
}

// Racing ordinary/shared creation cannot create two canonical addresses; restored state keeps the winner.
#[test]
fn concurrent_shared_and_exclusive_allocation_survives_restore() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let space = create(&store, &context, K::AddressSpace, "space")?;
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = [api::SharedUse::Exclusive, api::SharedUse::Anycast]
            .into_iter()
            .map(|sharing| {
                let store = &store;
                let context = &context;
                let barrier = &barrier;
                let space = space.clone();
                scope.spawn(move || {
                    barrier.wait();
                    call(
                        store,
                        context,
                        api::request::Command::Network(api::Network {
                            kind: K::IpAddress as i32,
                            address_space: Some(space),
                            cidr: "2001:db8::1/64".into(),
                            name: "address".into(),
                            sharing: sharing as i32,
                            group_name: if sharing == api::SharedUse::Anycast {
                                "edge".into()
                            } else {
                                String::new()
                            },
                            ..Default::default()
                        }),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("test writer"))
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    let mut q = query();
    q.kinds = vec![K::IpAddress as i32];
    let api::reply::Result::Page(page) =
        call(&store, &context, api::request::Command::Query(q.clone()))?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    let target = page.items[0].target.clone();
    let duplicate_request = api::Network {
        kind: K::IpAddress as i32,
        address_space: Some(space),
        cidr: "2001:db8::1/64".into(),
        name: "duplicate".into(),
        sharing: api::SharedUse::Exclusive as i32,
        ..Default::default()
    };
    assert!(matches!(
        call(
            &store,
            &context,
            api::request::Command::Network(duplicate_request.clone())
        ),
        Err(Error::Conflict)
    ));
    let mut backup = Vec::new();
    store.export(&mut backup)?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &backup, policy(), backup.len() as u64)?;
    let api::reply::Result::Detail(detail) = call(
        &restored,
        &context,
        api::request::Command::Read(api::Read {
            target: target.clone(),
            ..Default::default()
        }),
    )?
    else {
        panic!("restored detail");
    };
    assert_eq!(detail.item.expect("item").target, target);
    assert!(detail.participants.is_empty());
    let api::reply::Result::Page(after) =
        call(&restored, &context, api::request::Command::Query(q))?
    else {
        panic!("page");
    };
    assert_eq!(after.items, page.items);
    // A logical restore retains inventory but refuses all writes pending host reconciliation.
    let duplicate = call(
        &restored,
        &context,
        api::request::Command::Network(duplicate_request),
    );
    assert!(
        matches!(duplicate, Err(Error::RecoveryRequired)),
        "{duplicate:?}"
    );
    Ok(())
}

// RFC 9568 section 5.2.5: one virtual router owns multiple addresses, with one member set.
#[test]
fn shared_group_spans_addresses_and_fences_cross_address_edits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let space = create(&store, &context, K::AddressSpace, "space")?;
    let lan = create(&store, &context, K::L2Domain, "lan")?;
    let interface = create(&store, &context, K::Interface, "router")?;
    let backup = create(&store, &context, K::Interface, "backup")?;
    let api::reply::Result::Mutation(first) = call(
        &store,
        &context,
        api::request::Command::Network(api::Network {
            kind: K::IpAddress as i32,
            address_space: Some(space.clone()),
            cidr: "192.0.2.1/24".into(),
            name: "first VIP".into(),
            sharing: api::SharedUse::Vrrp as i32,
            link: Some(lan),
            vrid: Some(7),
            group_name: "edge routers".into(),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    let first = first.targets[0].clone();
    let api::reply::Result::Detail(detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(first.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let group = detail.group.expect("separate group");
    assert_eq!(group.kind, K::AddressGroup as i32);
    let api::reply::Result::Mutation(second) = call(
        &store,
        &context,
        api::request::Command::Network(api::Network {
            kind: K::IpAddress as i32,
            address_space: Some(space),
            cidr: "192.0.2.2/24".into(),
            name: "second VIP".into(),
            sharing: api::SharedUse::Unspecified as i32,
            group: group.target.clone(),
            expected_group: Some(group.revision),
            ..Default::default()
        }),
    )?
    else {
        panic!("second address");
    };
    let second = second.targets[0].clone();
    let api::reply::Result::Detail(current) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(first.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("current group");
    };
    call(
        &store,
        &context,
        api::request::Command::Member(api::Member {
            address: Some(first),
            interface: Some(interface.clone()),
            expected: detail.item.expect("address").revision,
            expected_group: Some(current.group.expect("group").revision),
            priority: Some(100),
            ..Default::default()
        }),
    )?;
    let api::reply::Result::Detail(detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(second.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert_eq!(detail.participants.len(), 1);
    assert_eq!(detail.participants[0].interface.as_ref(), Some(&interface));
    assert!(matches!(
        call(
            &store,
            &context,
            api::request::Command::Member(api::Member {
                address: Some(second.clone()),
                interface: Some(backup.clone()),
                expected: detail.item.as_ref().expect("address").revision,
                expected_group: Some(group.revision),
                priority: Some(90),
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    call(
        &store,
        &context,
        api::request::Command::Member(api::Member {
            address: Some(second.clone()),
            interface: Some(backup.clone()),
            expected: detail.item.as_ref().expect("address").revision,
            expected_group: Some(detail.group.expect("group").revision),
            priority: Some(90),
            ..Default::default()
        }),
    )?;
    for permitted in [&interface, &backup] {
        let mut partial = context.clone();
        partial.grants[0].scope = Scope::Targets(vec![
            AssetRef::try_from(&second)?,
            AssetRef::try_from(group.target.as_ref().expect("group identity"))?,
            AssetRef::try_from(permitted)?,
        ]);
        let api::reply::Result::Detail(partial) = call(
            &store,
            &partial,
            api::request::Command::Read(api::Read {
                target: Some(second.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("partial group");
        };
        assert_eq!(partial.participants.len(), 1);
        assert_eq!(partial.participants[0].interface.as_ref(), Some(permitted));
        assert!(!partial.participants_complete);
    }
    let mut restricted = context;
    restricted.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&second)?]);
    let api::reply::Result::Detail(hidden) = call(
        &store,
        &restricted,
        api::request::Command::Read(api::Read {
            target: Some(second),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert!(hidden.group.is_none());
    assert!(hidden.participants.is_empty());
    assert!(!hidden.participants_complete);
    Ok(())
}

// VRID edits change one group, while address filters and columns read its current value.
#[test]
fn group_vrid_projection_and_filter_do_not_keep_stale_copies() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let space = create(&store, &context, K::AddressSpace, "space")?;
    let lan = create(&store, &context, K::L2Domain, "lan")?;
    let api::reply::Result::Mutation(created) = call(
        &store,
        &context,
        api::request::Command::Network(api::Network {
            kind: K::IpAddress as i32,
            address_space: Some(space),
            cidr: "192.0.2.1/24".into(),
            name: "VIP".into(),
            sharing: api::SharedUse::Vrrp as i32,
            link: Some(lan.clone()),
            vrid: Some(7),
            group_name: "routers".into(),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    let api::reply::Result::Detail(detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(created.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let group = detail.group.expect("group");
    let mut q = query();
    q.kinds = vec![K::IpAddress as i32];
    q.columns.push("vrid".into());
    q.predicates.push(api::Predicate {
        field: "vrid".into(),
        value: Some(wire::Field {
            name: "vrid".into(),
            value: Some(wire::field::Value::Integer(7)),
        }),
        definition_generation: None,
    });
    let api::reply::Result::Page(page) =
        call(&store, &context, api::request::Command::Query(q.clone()))?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert!(
        page.items[0]
            .fields
            .iter()
            .any(|f| f.name == "vrid" && f.value == Some(wire::field::Value::Integer(7)))
    );
    call(
        &store,
        &context,
        api::request::Command::Network(api::Network {
            target: group.target,
            expected: Some(group.revision),
            kind: K::AddressGroup as i32,
            name: "routers".into(),
            sharing: api::SharedUse::Vrrp as i32,
            link: Some(lan),
            vrid: Some(8),
            ..Default::default()
        }),
    )?;
    let api::reply::Result::Page(page) =
        call(&store, &context, api::request::Command::Query(q.clone()))?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 0);
    q.predicates[0].value.as_mut().expect("operand").value = Some(wire::field::Value::Integer(8));
    let api::reply::Result::Page(page) =
        call(&store, &context, api::request::Command::Query(q.clone()))?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert!(
        page.items[0]
            .fields
            .iter()
            .any(|f| f.name == "vrid" && f.value == Some(wire::field::Value::Integer(8)))
    );
    let mut limited = context;
    limited.grants[0].scope = Scope::Kinds(vec![K::IpAddress]);
    let api::reply::Result::Page(hidden) = call(&store, &limited, api::request::Command::Query(q))?
    else {
        panic!("page");
    };
    assert_eq!(hidden.total, 0);
    Ok(())
}

// Removing one address must retain its shared group; a referenced group cannot disappear.
#[test]
fn address_group_removal_releases_only_owned_protocol_namespace() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let space = create(&store, &context, K::AddressSpace, "space")?;
    let lan = create(&store, &context, K::L2Domain, "lan")?;
    let network = api::Network {
        kind: K::IpAddress as i32,
        address_space: Some(space),
        cidr: "192.0.2.1/24".into(),
        name: "VIP".into(),
        sharing: api::SharedUse::Vrrp as i32,
        link: Some(lan),
        vrid: Some(7),
        group_name: "routers".into(),
        ..Default::default()
    };
    let api::reply::Result::Mutation(created) = call(
        &store,
        &context,
        api::request::Command::Network(network.clone()),
    )?
    else {
        panic!("address");
    };
    let address = created.targets[0].clone();
    let api::reply::Result::Detail(detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(address.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let group = detail.group.expect("group").target.expect("identity");
    let removal = call(
        &store,
        &context,
        api::request::Command::Bulk(api::Bulk {
            targets: vec![group.clone()],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    );
    assert!(matches!(removal, Err(Error::Conflict)), "{removal:?}");
    call(
        &store,
        &context,
        api::request::Command::Bulk(api::Bulk {
            targets: vec![address],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    assert!(store.asset(AssetRef::try_from(&group)?).is_ok());
    call(
        &store,
        &context,
        api::request::Command::Bulk(api::Bulk {
            targets: vec![group.clone()],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    assert!(matches!(
        store.asset(AssetRef::try_from(&group)?),
        Err(Error::Missing)
    ));
    call(&store, &context, api::request::Command::Network(network))?;
    Ok(())
}
