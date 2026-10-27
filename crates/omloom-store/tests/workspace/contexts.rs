use super::*;
use api::{reply::Result as Reply, request::Command};

// Retaining a mapping cannot bypass exclusive-interface checks when destination knowledge becomes configuration.
#[test]
fn retained_binding_rechecks_configured_owner_without_explicit_rebinding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let owner = create(&store, &actor, K::Interface, "reference owner")?;
    let participant = create(&store, &actor, K::Interface, "allocation participant")?;
    let canonical = changed(call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "canonical".into(),
            cidr: "192.0.2.10/32".into(),
            address_space: Some(space),
            ..Default::default()
        }),
    )?);
    call(
        &store,
        &actor,
        Command::Member(api::Member {
            address: Some(canonical.clone()),
            interface: Some(participant),
            expected: store.asset(AssetRef::try_from(&canonical)?)?.revision,
            ..Default::default()
        }),
    )?;
    let fact = changed(within(
        &store,
        &actor,
        focused(&store, &owner)?,
        Command::Address(api::Address {
            address: Some("192.0.2.10".into()),
            meaning: Some(api::AddressMeaning::Destination as i32),
            parent: Some(owner),
            ..Default::default()
        }),
    )?);
    within(
        &store,
        &actor,
        api::WorkingContext {
            expected: Some(store.usage()?.revision),
            ..Default::default()
        },
        Command::Address(api::Address {
            target: Some(fact.clone()),
            expected: Some(1),
            binding: Some(canonical.clone()),
            ..Default::default()
        }),
    )?;
    for binding in [Some(canonical), None] {
        let before = store.usage()?;
        assert!(
            matches!(
                within(
                    &store,
                    &actor,
                    api::WorkingContext {
                        expected: Some(before.revision),
                        ..Default::default()
                    },
                    Command::Address(api::Address {
                        target: Some(fact.clone()),
                        expected: Some(2),
                        meaning: Some(api::AddressMeaning::Configured as i32),
                        binding,
                        ..Default::default()
                    })
                ),
                Err(Error::Conflict)
            ),
            "retained and explicit mappings must enforce the same ownership invariant"
        );
        assert_eq!(store.usage()?, before);
    }
    // An unrelated local description still needs no access to the canonical allocation.
    let mut local = actor.clone();
    local.grants = vec![Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&fact)?]),
        read: Fields::All,
        write: Fields::Selected(vec!["description".into()]),
        writable: true,
    }];
    let mut edit = api::Address {
        target: Some(fact.clone()),
        expected: Some(2),
        changes: vec![text_field("description", "local note")],
        ..Default::default()
    };
    within(
        &store,
        &local,
        focused(&store, &fact)?,
        Command::Address(edit.clone()),
    )?;
    edit.expected = Some(3);
    let before = store.usage()?;
    within(
        &store,
        &local,
        focused(&store, &fact)?,
        Command::Address(edit),
    )?;
    assert_eq!(
        store.usage()?,
        before,
        "identical local edits must not write"
    );
    assert!(
        detail(&store, &actor, &fact)?
            .item
            .expect("fact")
            .fields
            .contains(&text_field(
                "meaning",
                api::AddressMeaning::Destination.as_str_name()
            ))
    );
    Ok(())
}

// Clearing the required IPv6 zone must fail identically for explicit and retained reconciliation.
#[test]
fn retained_link_local_binding_requires_zone_after_details_change() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let link = create(&store, &actor, K::L2Domain, "link")?;
    let canonical = changed(call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "canonical".into(),
            cidr: "fe80::1/128".into(),
            address_space: Some(space),
            link: Some(link),
            ..Default::default()
        }),
    )?);
    let fact = changed(within(
        &store,
        &actor,
        api::WorkingContext {
            expected: Some(store.usage()?.revision),
            ..Default::default()
        },
        Command::Address(api::Address {
            address: Some("fe80::1".into()),
            zone: Some("en0".into()),
            binding: Some(canonical.clone()),
            ..Default::default()
        }),
    )?);
    for binding in [Some(canonical.clone()), None] {
        let before = store.usage()?;
        assert!(
            matches!(
                within(
                    &store,
                    &actor,
                    api::WorkingContext {
                        expected: Some(before.revision),
                        ..Default::default()
                    },
                    Command::Address(api::Address {
                        target: Some(fact.clone()),
                        expected: Some(1),
                        clear_zone: true,
                        binding,
                        ..Default::default()
                    })
                ),
                Err(Error::Incomplete)
            ),
            "a reconciled link-local reference cannot lose its zone"
        );
        assert_eq!(store.usage()?, before);
    }
    within(
        &store,
        &actor,
        focused(&store, &fact)?,
        Command::Address(api::Address {
            target: Some(fact.clone()),
            expected: Some(1),
            zone: Some("en1".into()),
            ..Default::default()
        }),
    )?;
    let fields = detail(&store, &actor, &fact)?.item.expect("fact").fields;
    assert!(fields.contains(&text_field("zone", "en1")));
    assert!(fields.contains(&text_field(
        "binding",
        &format!("{}/{}", canonical.asset_id, canonical.incarnation)
    )));
    Ok(())
}

// Dedicated address semantics cannot be bypassed through ordinary description/company changes.
#[test]
fn direct_address_changes_reject_reserved_network_fields() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let fact = changed(call(
        &store,
        &actor,
        Command::Address(api::Address {
            address: Some("192.0.2.10".into()),
            ..Default::default()
        }),
    )?);
    for value in [
        wire::Field {
            name: "prefix_length".into(),
            value: Some(wire::field::Value::Integer(64)),
        },
        text_field("address", "192.0.2.11"),
        text_field("meaning", api::AddressMeaning::Configured.as_str_name()),
        text_field("zone", "en0"),
        text_field(
            "binding",
            &format!("{}/{}", fact.asset_id, fact.incarnation),
        ),
    ] {
        for target in [None, Some(fact.clone())] {
            let before = store.usage()?;
            let request = api::Address {
                expected: target.as_ref().map(|_| 1),
                address: target.is_none().then(|| "192.0.2.20".into()),
                target,
                changes: vec![value.clone()],
                ..Default::default()
            };
            assert!(
                matches!(
                    call(&store, &actor, Command::Address(request)),
                    Err(Error::Invalid)
                ),
                "reserved field {}",
                value.name
            );
            assert_eq!(store.usage()?, before);
        }
    }
    call(
        &store,
        &actor,
        Command::Address(api::Address {
            target: Some(fact.clone()),
            expected: Some(1),
            changes: vec![text_field("description", "reviewed fact")],
            ..Default::default()
        }),
    )?;
    let fields = detail(&store, &actor, &fact)?.item.expect("fact").fields;
    assert!(fields.contains(&text_field("address", "192.0.2.10")));
    assert!(fields.contains(&text_field("description", "reviewed fact")));
    assert!(
        fields
            .iter()
            .all(|f| !["prefix_length", "zone", "binding"].contains(&f.name.as_str()))
    );
    Ok(())
}

// Next-available cannot search a range wider than the selected parent, for either IP family.
#[test]
fn next_available_rejects_ranges_outside_the_parent_prefix() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    for (parent_cidr, wider, inside) in [
        ("192.0.2.0/24", "192.0.2.1/16", "192.0.2.1/24"),
        ("2001:db8:1::/64", "2001:db8:1::1/48", "2001:db8:1::1/64"),
    ] {
        let prefix = changed(call(
            &store,
            &actor,
            Command::Network(api::Network {
                kind: K::Prefix as i32,
                name: parent_cidr.into(),
                cidr: parent_cidr.into(),
                address_space: Some(space.clone()),
                ..Default::default()
            }),
        )?);
        let mut request = api::Network {
            kind: K::IpAddress as i32,
            name: "first available".into(),
            parent: Some(Box::new(prefix.clone())),
            cidr: wider.into(),
            next_available: true,
            ..Default::default()
        };
        let before = store.usage()?;
        assert!(matches!(
            within(
                &store,
                &actor,
                focused(&store, &prefix)?,
                Command::Network(request.clone())
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, before);
        request.cidr = inside.into();
        let allocated = changed(within(
            &store,
            &actor,
            focused(&store, &prefix)?,
            Command::Network(request),
        )?);
        let mut q = query();
        q.focus = Some(prefix);
        q.kinds = vec![K::IpAddress as i32];
        let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
            panic!("page")
        };
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].target.as_ref(), Some(&allocated));
    }
    Ok(())
}

// A canonical edit cannot make a reconciled address reference point at another address.
#[test]
fn canonical_address_edit_preserves_bound_facts_without_participants() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let mut allocation = api::Network {
        kind: K::IpAddress as i32,
        name: "canonical".into(),
        cidr: "192.0.2.10/32".into(),
        address_space: Some(space),
        ..Default::default()
    };
    let canonical = changed(call(&store, &actor, Command::Network(allocation.clone()))?);
    let fact = changed(within(
        &store,
        &actor,
        api::WorkingContext {
            expected: Some(store.usage()?.revision),
            ..Default::default()
        },
        Command::Address(api::Address {
            address: Some("192.0.2.10".into()),
            binding: Some(canonical.clone()),
            meaning: Some(api::AddressMeaning::Destination as i32),
            ..Default::default()
        }),
    )?);
    assert!(detail(&store, &actor, &canonical)?.participants.is_empty());
    allocation.target = Some(canonical.clone());
    allocation.expected = Some(store.asset(AssetRef::try_from(&canonical)?)?.revision);
    allocation.cidr = "192.0.2.11/32".into();
    let before = store.usage()?;
    let result = call(&store, &actor, Command::Network(allocation.clone()));
    assert!(
        matches!(result, Err(Error::Conflict)),
        "unexpected edit result: {result:?}"
    );
    assert_eq!(store.usage()?, before);
    assert!(
        detail(&store, &actor, &canonical)?
            .item
            .expect("canonical")
            .fields
            .contains(&text_field("cidr", "192.0.2.10/32"))
    );
    assert!(
        detail(&store, &actor, &fact)?
            .item
            .expect("fact")
            .fields
            .contains(&text_field("address", "192.0.2.10"))
    );
    allocation.cidr = "192.0.2.10/32".into();
    allocation.name = "renamed canonical".into();
    call(&store, &actor, Command::Network(allocation))?;
    Ok(())
}

// IPv4 link-local uses the explicit L2 identity; only IPv6 needs a host-local zone.
#[test]
fn link_local_bindings_require_zones_only_for_ipv6() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let link = create(&store, &actor, K::L2Domain, "local link")?;
    for (address, cidr, zone) in [
        ("169.254.1.1", "169.254.1.1/32", None),
        ("fe80::1", "fe80::1/128", Some("en0")),
    ] {
        let canonical = changed(call(
            &store,
            &actor,
            Command::Network(api::Network {
                kind: K::IpAddress as i32,
                name: address.into(),
                cidr: cidr.into(),
                address_space: Some(space.clone()),
                link: Some(link.clone()),
                ..Default::default()
            }),
        )?);
        let mut request = api::Address {
            address: Some(address.into()),
            binding: Some(canonical.clone()),
            meaning: Some(api::AddressMeaning::Destination as i32),
            ..Default::default()
        };
        let working = api::WorkingContext {
            expected: Some(store.usage()?.revision),
            ..Default::default()
        };
        if zone.is_some() {
            let before = store.usage()?;
            assert!(matches!(
                within(
                    &store,
                    &actor,
                    working.clone(),
                    Command::Address(request.clone())
                ),
                Err(Error::Incomplete)
            ));
            assert_eq!(store.usage()?, before);
        }
        request.zone = zone.map(str::to_owned);
        let fact = changed(within(&store, &actor, working, Command::Address(request))?);
        let item = detail(&store, &actor, &fact)?.item.expect("bound fact");
        assert!(item.fields.contains(&text_field(
            "binding",
            &format!("{}/{}", canonical.asset_id, canonical.incarnation)
        )));
        assert_eq!(
            item.fields.iter().find(|f| f.name == "zone"),
            zone.map(|z| text_field("zone", z)).as_ref()
        );
    }
    Ok(())
}

// Prefix contexts derive only their declared namespace and enforce containment.
#[test]
fn prefix_creation_remains_visible_without_reselecting_upstream_topology() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "isolated")?;
    let prefix = changed(within(
        &store,
        &actor,
        focused(&store, &space)?,
        Command::Network(api::Network {
            kind: K::Prefix as i32,
            name: "subnet".into(),
            cidr: "192.0.2.0/24".into(),
            parent: Some(Box::new(space)),
            ..Default::default()
        }),
    )?);
    let request = api::Network {
        kind: K::IpAddress as i32,
        name: "host".into(),
        cidr: "192.0.2.10/32".into(),
        parent: Some(Box::new(prefix.clone())),
        ..Default::default()
    };
    let address = changed(within(
        &store,
        &actor,
        focused(&store, &prefix)?,
        Command::Network(request.clone()),
    )?);
    let mut q = query();
    q.focus = Some(prefix.clone());
    q.kinds = vec![K::IpAddress as i32];
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&address));
    for search in ["192.0.2.10", "host"] {
        let mut q = query();
        q.focus = Some(prefix.clone());
        q.search = search.into();
        q.kinds = vec![K::IpAddress as i32];
        let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
            panic!("page");
        };
        assert_eq!(page.total, 1, "scoped search {search}");
        assert_eq!(page.items[0].target.as_ref(), Some(&address));
    }
    let before = store.usage()?;
    let mut outside = request;
    outside.cidr = "198.51.100.1/32".into();
    assert!(matches!(
        within(
            &store,
            &actor,
            focused(&store, &prefix)?,
            Command::Network(outside)
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Equal IPv6 link-local ranges on separate L2 domains must remain distinct.
#[test]
fn link_local_prefix_context_preserves_the_explicit_link_boundary() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let mut prefixes = Vec::new();
    for name in ["first link", "second link"] {
        let link = create(&store, &actor, K::L2Domain, name)?;
        let prefix = changed(call(
            &store,
            &actor,
            Command::Network(api::Network {
                kind: K::Prefix as i32,
                name: name.into(),
                cidr: "fe80::/64".into(),
                address_space: Some(space.clone()),
                link: Some(link),
                ..Default::default()
            }),
        )?);
        within(
            &store,
            &actor,
            focused(&store, &prefix)?,
            Command::Network(api::Network {
                kind: K::IpAddress as i32,
                name: name.into(),
                cidr: "fe80::1/128".into(),
                parent: Some(Box::new(prefix.clone())),
                ..Default::default()
            }),
        )?;
        prefixes.push(prefix);
    }
    for prefix in prefixes {
        let mut q = query();
        q.focus = Some(prefix);
        q.kinds = vec![K::IpAddress as i32];
        let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
            panic!("page");
        };
        assert_eq!(page.total, 1);
    }
    Ok(())
}

// Canonical admission, assignment and mapping share one transaction; a denied
// assignment must not consume the address or lose the original incomplete fact.
#[test]
fn address_resolution_is_atomic_and_preserves_conflicting_facts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let interface = create(&store, &actor, K::Interface, "eth0")?;
    let fact = changed(within(
        &store,
        &actor,
        focused(&store, &interface)?,
        Command::Address(api::Address {
            address: Some("192.0.2.20".into()),
            meaning: Some(api::AddressMeaning::Configured as i32),
            parent: Some(interface.clone()),
            ..Default::default()
        }),
    )?);
    let request = api::Address {
        target: Some(fact.clone()),
        expected: Some(1),
        allocation: Some(Box::new(api::Network {
            kind: K::IpAddress as i32,
            name: "canonical".into(),
            cidr: "192.0.2.20/32".into(),
            address_space: Some(space),
            ..Default::default()
        })),
        interface: Some(Box::new(interface.clone())),
        ..Default::default()
    };
    let mut limited = actor.clone();
    limited.grants[0].write = Fields::Selected(vec!["name".into(), "binding".into()]);
    limited.grants.push(Grant {
        scope: Scope::Kinds(vec![K::IpAddress]),
        read: Fields::All,
        write: Fields::All,
        writable: true,
    });
    // Settle fixture journal work before measuring the rejected request itself.
    store.diagnostic_database().persist()?;
    let before = store.usage()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    assert!(matches!(
        within(
            &store,
            &limited,
            api::WorkingContext {
                expected: Some(before.revision),
                ..Default::default()
            },
            Command::Address(request.clone())
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    within(
        &store,
        &actor,
        api::WorkingContext {
            expected: Some(before.revision),
            ..Default::default()
        },
        Command::Address(request),
    )?;
    let accepted = detail(&store, &actor, &fact)?.item.expect("fact");
    let binding = accepted
        .fields
        .iter()
        .find(|f| f.name == "binding")
        .expect("binding");
    let Some(wire::field::Value::Text(binding)) = &binding.value else {
        panic!("binding identity");
    };
    let (asset_id, incarnation) = binding.split_once('/').expect("identity");
    let canonical = wire::AssetRef {
        asset_id: asset_id.into(),
        incarnation: incarnation.into(),
    };
    assert_eq!(
        detail(&store, &actor, &canonical)?.participants[0]
            .interface
            .as_ref(),
        Some(&interface)
    );
    let mut local = actor.clone();
    local.grants = vec![Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&fact)?]),
        read: Fields::All,
        write: Fields::Selected(vec!["description".into()]),
        writable: true,
    }];
    assert!(
        detail(&store, &local, &fact)?
            .item
            .expect("local fact")
            .fields
            .iter()
            .all(|f| f.name != "binding")
    );
    within(
        &store,
        &local,
        focused(&store, &fact)?,
        Command::Address(api::Address {
            target: Some(fact.clone()),
            expected: Some(2),
            changes: vec![text_field("description", "checked locally")],
            ..Default::default()
        }),
    )?;
    assert!(
        detail(&store, &actor, &fact)?
            .item
            .expect("preserved mapping")
            .fields
            .iter()
            .any(|f| f.name == "binding")
    );
    let other = create(&store, &actor, K::Interface, "other")?;
    let conflicting = changed(within(
        &store,
        &actor,
        focused(&store, &other)?,
        Command::Address(api::Address {
            address: Some("192.0.2.20".into()),
            meaning: Some(api::AddressMeaning::Configured as i32),
            parent: Some(other),
            ..Default::default()
        }),
    )?);
    let before = store.usage()?;
    assert!(matches!(
        within(
            &store,
            &actor,
            api::WorkingContext {
                expected: Some(before.revision),
                ..Default::default()
            },
            Command::Address(api::Address {
                target: Some(conflicting.clone()),
                expected: Some(1),
                binding: Some(canonical),
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert!(
        detail(&store, &actor, &conflicting)?
            .item
            .expect("preserved fact")
            .fields
            .iter()
            .all(|f| f.name != "binding")
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert!(
        detail(&store, &actor, &fact)?
            .item
            .expect("retained mapping")
            .fields
            .iter()
            .any(|f| f.name == "binding")
    );
    Ok(())
}

// An invalid IPv4 mask must fail during preview, without admitting an unusable
// preview whose acceptance would fail later or write anything outside its parent.
#[test]
fn scoped_address_import_validates_joint_fields_before_acceptance() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "guest")?;
    let request = api::Import {
        kind: K::AddressReference as i32,
        parent: Some(vm.clone()),
        source: "interface-facts".into(),
        csv: "external_id,ip,mask\na,192.0.2.5,64\n".into(),
        mappings: vec![
            api::Mapping {
                source: "ip".into(),
                field: "address".into(),
            },
            api::Mapping {
                source: "mask".into(),
                field: "prefix_length".into(),
            },
        ],
        ..Default::default()
    };
    let before = store.usage()?;
    let Reply::Preview(invalid) = within(
        &store,
        &actor,
        focused(&store, &vm)?,
        Command::Import(request.clone()),
    )?
    else {
        panic!("preview");
    };
    assert!(invalid.id.is_none());
    assert!(!invalid.rows[0].diagnostic.is_empty());
    assert_eq!(store.usage()?, before);
    let mut valid = request;
    valid.csv = "external_id,ip,mask\na,192.0.2.5,24\n".into();
    let Reply::Preview(preview) = within(
        &store,
        &actor,
        focused(&store, &vm)?,
        Command::Import(valid),
    )?
    else {
        panic!("preview");
    };
    let id = preview.id.expect("valid preview");
    assert!(matches!(
        within(
            &store,
            &actor,
            api::WorkingContext {
                expected: Some(preview.revision),
                ..Default::default()
            },
            Command::Import(api::Import {
                preview: Some(id.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    let target = changed(within(
        &store,
        &actor,
        focused(&store, &vm)?,
        Command::Import(api::Import {
            preview: Some(id),
            ..Default::default()
        }),
    )?);
    let known = detail(&store, &actor, &target)?
        .item
        .expect("imported fact");
    assert!(known.fields.contains(&text_field("address", "192.0.2.5")));
    let before = store.usage()?;
    assert!(
        detail(&store, &actor, &vm)?
            .relations
            .iter()
            .any(|r| r.to.as_ref() == Some(&target))
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn within(
    store: &Store,
    context: &Context,
    working: api::WorkingContext,
    command: Command,
) -> Result<Reply, Error> {
    store
        .workspace(
            context,
            api::Request {
                command: Some(command),
                working: Some(working),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)
}
fn focused(store: &Store, focus: &wire::AssetRef) -> Result<api::WorkingContext, Error> {
    Ok(api::WorkingContext {
        focus: Some(focus.clone()),
        expected: Some(store.usage()?.revision),
        ..Default::default()
    })
}
fn changed(reply: Reply) -> wire::AssetRef {
    let Reply::Mutation(result) = reply else {
        panic!("mutation");
    };
    result.targets.into_iter().next().expect("changed identity")
}
fn detail(store: &Store, context: &Context, target: &wire::AssetRef) -> Result<api::Detail, Error> {
    let Reply::Detail(result) = within(
        store,
        context,
        focused(store, target)?,
        Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(*result)
}

// A VM form offers local components; neither peers nor the host's network are inherited.
#[test]
fn creation_choices_and_projection_follow_the_resource() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let host = create(&store, &actor, K::Device, "hypervisor")?;
    let vm = create(&store, &actor, K::VirtualMachine, "guest")?;
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(vm.clone()),
                to: Some(host.clone()),
                kind: wire::RelationKind::RunsOn as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    let working = focused(&store, &vm)?;
    let before = store.usage()?;
    let Reply::Actions(actions) = within(
        &store,
        &actor,
        working.clone(),
        Command::Prepare(api::Prepare {
            view: api::View::Network as i32,
            ..Default::default()
        }),
    )?
    else {
        panic!("actions");
    };
    assert!(
        actions
            .creations
            .iter()
            .any(|a| a.kind == K::Interface as i32)
    );
    assert!(
        actions
            .creations
            .iter()
            .any(|a| a.kind == K::AddressReference as i32)
    );
    assert!(!actions.creations.iter().any(|a| matches!(
        K::try_from(a.kind),
        Ok(K::VirtualMachine | K::AddressSpace | K::Vrf)
    )));
    assert!(
        actions
            .creations
            .iter()
            .all(|a| a.parent.as_ref().and_then(|p| p.target.as_ref()) == Some(&vm))
    );
    assert!(detail(&store, &actor, &vm)?.relations.is_empty());
    assert_eq!(store.usage()?, before);
    let invalid = within(
        &store,
        &actor,
        working.clone(),
        Command::Write(api::Write {
            kind: K::VirtualMachine as i32,
            parent: Some(vm.clone()),
            changes: vec![text_field("name", "peer")],
            ..Default::default()
        }),
    );
    assert!(matches!(invalid, Err(Error::Invalid)));
    let child = changed(within(
        &store,
        &actor,
        working,
        Command::Write(api::Write {
            kind: K::Interface as i32,
            parent: Some(vm.clone()),
            changes: vec![text_field("name", "eth0")],
            ..Default::default()
        }),
    )?);
    let before = store.usage()?;
    assert!(matches!(
        within(
            &store,
            &actor,
            focused(&store, &child)?,
            Command::Write(api::Write {
                target: Some(child.clone()),
                expected: Some(1),
                kind: K::Interface as i32,
                changes: vec![text_field("serial", "not a physical component")],
                ..Default::default()
            })
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    let read = detail(&store, &actor, &vm)?;
    assert_eq!(read.relations.len(), 1);
    assert_eq!(read.relations[0].to.as_ref(), Some(&child));
    assert_eq!(
        detail(&store, &actor, &child)?
            .item
            .expect("interface")
            .fields,
        vec![text_field("name", "eth0")]
    );
    let mut q = query();
    q.view = api::View::Network as i32;
    q.focus = Some(vm.clone());
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&child));
    assert!(matches!(
        within(
            &store,
            &actor,
            focused(&store, &vm)?,
            Command::Write(api::Write {
                target: Some(host),
                expected: Some(1),
                kind: K::Device as i32,
                changes: vec![text_field("name", "hidden underlay")],
                parent: None
            })
        ),
        Err(Error::Denied)
    ));
    Ok(())
}

// Failure to attach a new component must roll back its fields, identity and audit together.
#[test]
fn child_attachment_is_atomic_and_does_not_inherit_grants() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "guest")?;
    let mut limited = actor.clone();
    limited.grants = vec![
        Grant {
            scope: Scope::Kinds(vec![K::VirtualMachine]),
            read: Fields::All,
            write: Fields::All,
            writable: true,
        },
        Grant {
            scope: Scope::Kinds(vec![K::Interface]),
            read: Fields::All,
            write: Fields::Selected(vec!["name".into()]),
            writable: true,
        },
    ];
    let before = store.usage()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    let outcome = within(
        &store,
        &limited,
        focused(&store, &vm)?,
        Command::Write(api::Write {
            kind: K::Interface as i32,
            parent: Some(vm),
            changes: vec![text_field("name", "must roll back")],
            ..Default::default()
        }),
    );
    assert!(matches!(outcome, Err(Error::Denied)));
    assert_eq!(store.usage()?, before);
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let Reply::Page(page) = call(&store, &actor, Command::Query(query()))? else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    Ok(())
}

// A logical interface can belong to a device while a physical port carries it.
#[test]
fn ownership_and_physical_carriage_are_independent_dimensions() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let device = create(&store, &actor, K::Device, "server")?;
    let mut children = Vec::new();
    for (kind, name) in [
        (K::Interface, "host interface"),
        (K::Port, "physical connector"),
    ] {
        children.push(changed(within(
            &store,
            &actor,
            focused(&store, &device)?,
            Command::Write(api::Write {
                kind: kind as i32,
                parent: Some(device.clone()),
                changes: vec![text_field("name", name)],
                ..Default::default()
            }),
        )?));
    }
    within(
        &store,
        &actor,
        api::WorkingContext {
            roots: vec![device],
            expected: Some(store.usage()?.revision),
            focus: None,
        },
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(children[1].clone()),
                to: Some(children[0].clone()),
                kind: wire::RelationKind::Carries as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    Ok(())
}

// Unknown network knowledge is accepted inventory, with no allocation or mask invented.
#[test]
fn address_facts_survive_reopen_without_topology_and_deduplicate_updates() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::VirtualMachine, "one")?;
    let second = create(&store, &actor, K::VirtualMachine, "two")?;
    let mut references = Vec::new();
    for vm in [&first, &second] {
        let reference = changed(within(
            &store,
            &actor,
            focused(&store, vm)?,
            Command::Address(api::Address {
                address: Some("192.0.2.10".into()),
                parent: Some(vm.clone()),
                meaning: Some(api::AddressMeaning::Configured as i32),
                ..Default::default()
            }),
        )?);
        let read = detail(&store, &actor, &reference)?.item.expect("reference");
        assert_eq!(read.kind, K::AddressReference as i32);
        assert!(read.fields.contains(&text_field("name", "192.0.2.10")));
        assert!(read.fields.iter().all(|f| {
            !["cidr", "address_space", "prefix_length", "routing_context"]
                .contains(&f.name.as_str())
        }));
        references.push(reference);
    }
    assert_ne!(references[0], references[1]);
    let before = store.usage()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    within(
        &store,
        &actor,
        focused(&store, &references[0])?,
        Command::Address(api::Address {
            target: Some(references[0].clone()),
            expected: Some(1),
            address: Some("192.0.2.10".into()),
            ..Default::default()
        }),
    )?;
    let mut q = query();
    q.search = "192.0.2.10".into();
    let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(page.total, 2);
    q.focus = Some(first);
    let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&references[0]));
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    let Reply::Page(page) = call(&store, &restricted, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(page.total, 0);
    assert_eq!(store.usage()?, before);
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert!(
        detail(&store, &actor, &references[0])?
            .item
            .expect("retained reference")
            .fields
            .contains(&text_field("address", "192.0.2.10"))
    );
    Ok(())
}

// Peer edits and removal need an explicit encompassing selection, including unrelated parents.
#[test]
fn peer_relationships_require_the_enclosing_context_for_every_edit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::Service, "application")?;
    let second = create(&store, &actor, K::Service, "backup")?;
    let third = create(&store, &actor, K::Service, "other team")?;
    let request = api::Relationship {
        relation: Some(wire::Relation {
            from: Some(first.clone()),
            to: Some(second.clone()),
            kind: wire::RelationKind::DependsOn as i32,
            ..Default::default()
        }),
        remove: false,
    };
    let before = store.usage()?;
    assert!(matches!(
        within(
            &store,
            &actor,
            focused(&store, &first)?,
            Command::Relation(request.clone())
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    let enclosing = |store: &Store| {
        Ok::<_, Error>(api::WorkingContext {
            roots: vec![first.clone(), second.clone()],
            expected: Some(store.usage()?.revision),
            focus: None,
        })
    };
    within(
        &store,
        &actor,
        enclosing(&store)?,
        Command::Relation(request.clone()),
    )?;
    let unchanged = store.usage()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    within(
        &store,
        &actor,
        enclosing(&store)?,
        Command::Relation(request),
    )?;
    assert_eq!(store.usage()?, unchanged);
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    let Reply::Detail(read) = within(
        &store,
        &actor,
        enclosing(&store)?,
        Command::Read(api::Read {
            target: Some(first.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let relation = read.relations[0].clone();
    assert!(detail(&store, &actor, &first)?.relations.is_empty());
    let before = store.usage()?;
    let mut changed = relation.clone();
    changed.to = Some(third);
    assert!(matches!(
        within(
            &store,
            &actor,
            enclosing(&store)?,
            Command::Relation(api::Relationship {
                relation: Some(changed),
                remove: false
            })
        ),
        Err(Error::Denied)
    ));
    assert!(matches!(
        within(
            &store,
            &actor,
            focused(&store, &first)?,
            Command::Relation(api::Relationship {
                relation: Some(relation.clone()),
                remove: true
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    within(
        &store,
        &actor,
        enclosing(&store)?,
        Command::Relation(api::Relationship {
            relation: Some(relation),
            remove: true,
        }),
    )?;
    Ok(())
}

// Old context revisions and ambiguous parents cannot accept orphaned children.
#[test]
fn stale_and_multi_root_creation_is_rejected_without_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::VirtualMachine, "one")?;
    let stale = focused(&store, &first)?;
    let second = create(&store, &actor, K::VirtualMachine, "two")?;
    let before = store.usage()?;
    let write = api::Write {
        kind: K::Interface as i32,
        changes: vec![text_field("name", "eth0")],
        parent: Some(first.clone()),
        ..Default::default()
    };
    assert!(matches!(
        within(&store, &actor, stale, Command::Write(write)),
        Err(Error::Conflict)
    ));
    let working = api::WorkingContext {
        roots: vec![first, second],
        expected: Some(before.revision),
        focus: None,
    };
    assert!(matches!(
        within(
            &store,
            &actor,
            working,
            Command::Write(api::Write {
                kind: K::Interface as i32,
                changes: vec![text_field("name", "eth0")],
                ..Default::default()
            })
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    let missing = store.workspace(
        &actor,
        api::Request {
            command: Some(Command::Write(api::Write {
                kind: K::Device as i32,
                changes: vec![text_field("name", "no context")],
                ..Default::default()
            })),
            working: None,
        },
        100,
    );
    assert!(matches!(missing, Err(Error::Invalid)));
    Ok(())
}
