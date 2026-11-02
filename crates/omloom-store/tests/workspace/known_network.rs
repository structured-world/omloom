use super::*;
use api::{reply::Result as R, request::Command as C};

fn command_at(store: &Store, command: C, revision: u64) -> Result<api::reply::Result, Error> {
    store
        .workspace(
            &operator(),
            api::Request {
                command: Some(command),
                working: Some(api::WorkingContext {
                    expected: Some(revision),
                    ..Default::default()
                }),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)
}
fn revision(store: &Store) -> Result<u64, Error> {
    let R::Page(page) = call(store, &operator(), C::Query(query()))? else {
        return Err(Error::Corrupt);
    };
    Ok(page.revision)
}

// A preview ID recovers an accepted import even when the caller omitted a revision.
#[test]
fn csv_unscoped_apply_recovers_lost_reply_after_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let R::Preview(preview) = call(
        &store,
        &actor,
        C::Import(api::Import {
            source: "unscoped-preview".into(),
            kind: K::AddressReference as i32,
            csv: "external_id,address\nendpoint,192.0.2.31\n".into(),
            mappings: vec![api::Mapping {
                source: "address".into(),
                field: "address".into(),
            }],
            ..Default::default()
        }),
    )?
    else {
        panic!("import preview")
    };
    assert!(preview.id.is_some(), "{preview:?}");
    let apply = C::Import(api::Import {
        preview: preview.id,
        ..Default::default()
    });
    let accepted = call(&store, &actor, apply.clone())?;
    let usage = store.usage()?;
    assert_eq!(call(&store, &actor, apply.clone())?, accepted);
    assert_eq!(store.usage()?, usage);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(call(&store, &actor, apply)?, accepted);
    assert_eq!(store.usage()?, usage);
    Ok(())
}

// The trusted record API must enforce the same CIDR and allocation boundaries as workspace writes.
#[test]
fn public_prefix_membership_enforces_network_facts() -> TestResult {
    for mismatch in ["cidr", "namespace", "link", "none"] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let space = create(&store, &actor, K::AddressSpace, "namespace")?;
        let other = create(&store, &actor, K::AddressSpace, "other namespace")?;
        let link = create(&store, &actor, K::L2Domain, "link")?;
        let other_link = create(&store, &actor, K::L2Domain, "other link")?;
        let ip = if mismatch == "link" {
            "fe80::1"
        } else if mismatch == "cidr" {
            "198.51.100.1"
        } else {
            "192.0.2.1"
        };
        let mut targets = Vec::new();
        for (kind, cidr, namespace, local_link) in [
            (
                K::Prefix,
                if mismatch == "link" {
                    "fe80::/64".into()
                } else {
                    "192.0.2.0/24".into()
                },
                space.clone(),
                (mismatch == "link").then_some(link.clone()),
            ),
            (
                K::IpAddress,
                format!("{ip}/{}", if mismatch == "link" { 128 } else { 32 }),
                if mismatch == "namespace" {
                    other.clone()
                } else {
                    space.clone()
                },
                (mismatch == "link").then_some(other_link),
            ),
        ] {
            let R::Mutation(saved) = command_at(
                &store,
                C::Network(api::Network {
                    kind: kind as i32,
                    cidr,
                    address_space: Some(namespace),
                    link: local_link,
                    name: "allocation".into(),
                    sharing: api::SharedUse::Exclusive as i32,
                    ..Default::default()
                }),
                revision(&store)?,
            )?
            else {
                panic!("allocation")
            };
            targets.push(saved.targets[0].clone());
        }
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some(ip.into()),
                binding: Some(targets[1].clone()),
                zone: (mismatch == "link").then(|| "en0".into()),
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("known address")
        };
        let record = admit(wire::record::Body::Relation(wire::Relation {
            relation_id: omloom_contracts::ids::RelationId::generate().to_string(),
            authority_id: omloom_contracts::ids::AuthorityId::generate().to_string(),
            revision: 1,
            kind: wire::RelationKind::MemberOf as i32,
            from: Some(saved.targets[0].clone()),
            to: Some(targets[0].clone()),
        }));
        let usage = store.usage()?;
        let result = store.save_relation(&record, None, &actor.actor, 100);
        if mismatch == "none" {
            assert_eq!(result?, 1);
        } else {
            assert!(
                matches!(result, Err(Error::Conflict)),
                "{mismatch}: {result:?}"
            );
            assert_eq!(store.usage()?, usage);
        }
    }
    Ok(())
}

// Prefix address membership cannot become an interface participant, including by retyping.
#[test]
fn prefix_cannot_acquire_interface_participants() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "namespace")?;
    let interface = create(&store, &actor, K::Interface, "interface")?;
    let mut targets = Vec::new();
    for (kind, cidr, name) in [
        (K::Prefix, "192.0.2.0/24", "prefix"),
        (K::IpAddress, "192.0.2.10/32", "address"),
    ] {
        let R::Mutation(saved) = command_at(
            &store,
            C::Network(api::Network {
                kind: kind as i32,
                cidr: cidr.into(),
                name: name.into(),
                address_space: Some(space.clone()),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("allocation")
        };
        targets.push(saved.targets[0].clone());
    }
    let read = |target: &wire::AssetRef| -> Result<api::Item, Error> {
        let R::Detail(detail) = call(
            &store,
            &actor,
            C::Read(api::Read {
                target: Some(target.clone()),
                ..Default::default()
            }),
        )?
        else {
            return Err(Error::Corrupt);
        };
        detail.item.ok_or(Error::Missing)
    };
    let before = store.usage()?;
    assert!(matches!(
        command_at(
            &store,
            C::Member(api::Member {
                address: Some(targets[0].clone()),
                interface: Some(interface.clone()),
                expected: read(&targets[0])?.revision,
                ..Default::default()
            }),
            revision(&store)?
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);

    command_at(
        &store,
        C::Member(api::Member {
            address: Some(targets[1].clone()),
            interface: Some(interface.clone()),
            expected: read(&targets[1])?.revision,
            ..Default::default()
        }),
        revision(&store)?,
    )?;
    let before = store.usage()?;
    let address = read(&targets[1])?;
    assert!(matches!(
        command_at(
            &store,
            C::Network(api::Network {
                target: Some(targets[1].clone()),
                expected: Some(address.revision),
                kind: K::Prefix as i32,
                name: "prefix".into(),
                cidr: "192.0.2.0/24".into(),
                address_space: Some(space),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);

    let mut retyped = asset();
    retyped.target = Some(targets[1].clone());
    retyped.kind = K::Prefix as i32;
    retyped.revision = address.revision + 1;
    assert!(matches!(
        store.save_asset(
            &admit(wire::record::Body::Asset(retyped)),
            Some(address.revision),
            "operator",
            100
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let R::Detail(detail) = call(
        &store,
        &actor,
        C::Read(api::Read {
            target: Some(targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address")
    };
    assert_eq!(detail.item.ok_or("item")?.kind, K::IpAddress as i32);
    assert_eq!(detail.participants.len(), 1);
    assert_eq!(detail.participants[0].interface.as_ref(), Some(&interface));
    assert!(read(&targets[0])?.kind == K::Prefix as i32);
    Ok(())
}

// Resolving an allocation must recheck both preserved and explicitly selected prefix membership.
#[test]
fn allocation_resolution_revalidates_prefix_namespace() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::AddressSpace, "first")?;
    let other = create(&store, &actor, K::AddressSpace, "other")?;
    let R::Mutation(prefix) = command_at(
        &store,
        C::Network(api::Network {
            kind: K::Prefix as i32,
            cidr: "192.0.2.0/24".into(),
            address_space: Some(first),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("prefix")
    };
    let prefix = prefix.targets[0].clone();
    let R::Mutation(address) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.8".into()),
            prefix: Some(prefix.clone()),
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("address")
    };
    let address = address.targets[0].clone();
    let R::Detail(detail) = call(
        &store,
        &actor,
        C::Read(api::Read {
            target: Some(address.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    let expected = detail.item.ok_or("item")?.revision;
    let base = revision(&store)?;
    let before = store.usage()?;
    for selected in [None, Some(prefix)] {
        let request = api::Address {
            target: Some(address.clone()),
            expected: Some(expected),
            prefix: selected,
            allocation: Some(Box::new(api::Network {
                kind: K::IpAddress as i32,
                name: "wrong namespace".into(),
                cidr: "192.0.2.8/32".into(),
                address_space: Some(other.clone()),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            })),
            ..Default::default()
        };
        for command in [C::PreviewAddress(request.clone()), C::Address(request)] {
            assert!(matches!(
                command_at(&store, command, base),
                Err(Error::Conflict)
            ));
            assert_eq!(store.usage()?, before);
        }
    }
    let R::Page(page) = call(
        &store,
        &actor,
        C::Query(api::Query {
            kinds: vec![K::IpAddress as i32],
            ..query()
        }),
    )?
    else {
        panic!("allocations")
    };
    assert_eq!(page.total, 0);
    Ok(())
}

// The generic relation writer must preserve the address command's single selected prefix slot.
#[test]
fn generic_relation_cannot_add_a_second_selected_prefix() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut created = Vec::new();
    for _ in 0..2 {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some("192.0.2.8/24".into()),
                create_prefix: true,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("address with prefix")
        };
        created.push(saved.targets);
    }
    let relation = |prefix: wire::AssetRef| {
        C::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(created[0][0].clone()),
                to: Some(prefix),
                kind: wire::RelationKind::MemberOf as i32,
                ..Default::default()
            }),
            ..Default::default()
        })
    };
    let before = store.usage()?;
    command_at(&store, relation(created[0][1].clone()), revision(&store)?)?;
    assert_eq!(store.usage()?, before);
    assert!(matches!(
        command_at(&store, relation(created[1][1].clone()), revision(&store)?),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(created[0][0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    command_at(
        &store,
        C::Address(api::Address {
            target: Some(created[0][0].clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            clear_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?;
    let base = revision(&store)?;
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            command_at(&store, relation(created[0][1].clone()), base)
        });
        let right = scope.spawn(|| {
            barrier.wait();
            command_at(&store, relation(created[1][1].clone()), base)
        });
        [
            left.join().expect("first selection"),
            right.join().expect("second selection"),
        ]
    });
    let mut committed = 0;
    for outcome in outcomes {
        match outcome {
            Ok(R::Mutation(_)) => committed += 1,
            Err(Error::Conflict) => {}
            other => panic!("unexpected selection result: {other:?}"),
        }
    }
    assert_eq!(committed, 1);
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(created[0][0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("selected prefix")
    };
    assert_eq!(
        detail
            .relations
            .iter()
            .filter(|r| r.kind == wire::RelationKind::MemberOf as i32)
            .count(),
        1
    );
    Ok(())
}

// Refinement validates ordinary subnet populations without treating a read page as a degree limit.
#[test]
fn prefix_refinement_checks_members_beyond_one_page() -> TestResult {
    for count in [128, 129] {
        let dir = tempfile::tempdir()?;
        let mut store = Store::open(dir.path(), policy())?;
        let space = create(&store, &operator(), K::AddressSpace, "namespace")?;
        let R::Mutation(saved) = command_at(
            &store,
            C::Network(api::Network {
                kind: K::Prefix as i32,
                cidr: "192.0.2.0/24".into(),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("prefix")
        };
        let prefix = saved.targets[0].clone();
        let mut addresses = Vec::new();
        for host in 0..count {
            let R::Mutation(saved) = command_at(
                &store,
                C::Address(api::Address {
                    address: Some(format!("192.0.2.{host}")),
                    prefix: Some(prefix.clone()),
                    ..Default::default()
                }),
                revision(&store)?,
            )?
            else {
                panic!("member address")
            };
            addresses.push(saved.targets[0].clone());
        }
        #[cfg(feature = "diagnostics")]
        store.set_diagnostic_query_check(|db, statement| {
            if statement.starts_with("MATCH (r:OmRow {to_asset:$prefix})") {
                let plan = db.explain_cypher(statement).expect("prefix member plan");
                eprintln!("prefix member plan: {plan}");
                assert!(plan.contains("IndexScan"), "{plan}");
            }
        });
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(prefix.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("prefix detail")
        };
        let request = api::Network {
            target: Some(prefix.clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            kind: K::Prefix as i32,
            cidr: "192.0.2.0/24".into(),
            address_space: Some(space.clone()),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        };
        let base = revision(&store)?;
        let before = store.usage()?;
        let started = std::time::Instant::now();
        let result = command_at(&store, C::Network(request), base);
        eprintln!(
            "prefix refinement: members={count}, elapsed={:?}, retained_bytes={:?}, rows={:?}, outcome={:?}",
            started.elapsed(),
            before.bytes,
            before.rows,
            result.as_ref().err()
        );
        assert!(result.is_ok(), "refinement of {count} members: {result:?}");
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(prefix),
                ..Default::default()
            }),
        )?
        else {
            panic!("refined prefix")
        };
        let incompatible = api::Network {
            target: detail.item.as_ref().and_then(|i| i.target.clone()),
            expected: detail.item.as_ref().map(|i| i.revision),
            kind: K::Prefix as i32,
            cidr: "192.0.2.0/25".into(),
            address_space: Some(space.clone()),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        };
        if count == 129 {
            let before = store.usage()?;
            let mut cursor = None;
            let mut last: Option<wire::Relation> = None;
            loop {
                let R::Detail(page) = call(
                    &store,
                    &operator(),
                    C::Read(api::Read {
                        target: incompatible.target.clone(),
                        relations_cursor: cursor,
                        ..Default::default()
                    }),
                )?
                else {
                    panic!("members page")
                };
                for relation in page.relations {
                    if relation.kind == wire::RelationKind::MemberOf as i32
                        && last
                            .as_ref()
                            .is_none_or(|old| old.relation_id < relation.relation_id)
                    {
                        last = Some(relation);
                    }
                }
                cursor = page.relations_next;
                if cursor.is_none() {
                    break;
                }
            }
            let hidden = last.ok_or("last membership")?.from.ok_or("member")?;
            let mut restricted = operator();
            let mut allowed = addresses
                .iter()
                .filter(|a| **a != hidden)
                .map(AssetRef::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            allowed.push(AssetRef::try_from(
                incompatible.target.as_ref().ok_or("prefix")?,
            )?);
            allowed.push(AssetRef::try_from(&space)?);
            restricted.grants[0].scope = Scope::Targets(allowed);
            let mut enlarged = incompatible.clone();
            enlarged.cidr = "192.0.2.0/23".into();
            assert!(matches!(
                store.workspace(
                    &restricted,
                    api::Request {
                        command: Some(C::Network(enlarged)),
                        working: Some(api::WorkingContext {
                            expected: Some(revision(&store)?),
                            ..Default::default()
                        }),
                    },
                    100
                ),
                Err(Error::Denied)
            ));
            assert_eq!(store.usage()?, before);
            assert!(matches!(
                command_at(&store, C::Network(incompatible), revision(&store)?),
                Err(Error::Conflict)
            ));
            assert_eq!(store.usage()?, before);
        }
    }
    Ok(())
}

// Broadcast has one typed owner; arbitrary field changes cannot persist a second representation.
#[test]
fn address_changes_cannot_bypass_broadcast_validation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    for address in ["2001:db8::8", "192.0.2.8", "192.0.2.8/31"] {
        let request = api::Address {
            address: Some(address.into()),
            changes: vec![text_field("broadcast", "192.0.2.255")],
            ..Default::default()
        };
        for command in [C::PreviewAddress(request.clone()), C::Address(request)] {
            assert!(matches!(
                command_at(&store, command, revision(&store)?),
                Err(Error::Invalid)
            ));
            assert_eq!(store.usage()?, before);
        }
    }
    Ok(())
}

// A brd-only address edit uses the retained assignment mask, without weakening /31 or /32 rules.
#[test]
fn broadcast_edit_uses_the_retained_mask() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    for mask in [24, 31, 32] {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some(format!("192.0.2.8/{mask}")),
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("address")
        };
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(saved.targets[0].clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("detail")
        };
        let request = api::Address {
            target: Some(saved.targets[0].clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            address: Some("192.0.2.8 brd 192.0.2.254".into()),
            ..Default::default()
        };
        let before = store.usage()?;
        let base = revision(&store)?;
        if mask == 24 {
            let R::AddressReview(review) =
                command_at(&store, C::PreviewAddress(request.clone()), base)?
            else {
                panic!("preview")
            };
            assert_eq!(review.prefix_length, Some(mask));
            assert_eq!(review.broadcast.as_deref(), Some("192.0.2.254"));
            assert_eq!(store.usage()?, before);
            let mut cleared = request.clone();
            cleared.clear_prefix_length = true;
            assert!(matches!(
                command_at(&store, C::Address(cleared), base),
                Err(Error::Invalid)
            ));
            assert_eq!(store.usage()?, before);
            command_at(&store, C::Address(request), base)?;
            let R::Detail(detail) = call(
                &store,
                &operator(),
                C::Read(api::Read {
                    target: Some(saved.targets[0].clone()),
                    ..Default::default()
                }),
            )?
            else {
                panic!("updated address")
            };
            let fields = detail.item.ok_or("item")?.fields;
            assert_eq!(
                fields.iter().find(|f| f.name == "broadcast"),
                Some(&text_field("broadcast", "192.0.2.254"))
            );
        } else {
            for command in [C::PreviewAddress(request.clone()), C::Address(request)] {
                assert!(matches!(
                    command_at(&store, command, base),
                    Err(Error::Invalid)
                ));
                assert_eq!(store.usage()?, before);
            }
        }
    }
    Ok(())
}

// Searching a known-prefix scope must retain membership for text and exact IP searches.
#[test]
fn scoped_prefix_search_never_returns_unrelated_addresses() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Network(api::Network {
            kind: K::Prefix as i32,
            cidr: "192.0.2.0/24".into(),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("prefix")
    };
    let prefix = saved.targets[0].clone();
    let mut selected = Vec::new();
    for index in 0..10 {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some(if index == 1 { "192.0.2.9" } else { "192.0.2.8" }.into()),
                prefix: (index < 2).then(|| prefix.clone()),
                changes: vec![text_field("description", "needle")],
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("address")
        };
        if index < 2 {
            selected.push(saved.targets[0].clone());
        }
    }
    let before = store.usage()?;
    let mut pages = Vec::new();
    for term in ["needle", "192.0.2.8"] {
        let query = api::Query {
            roots: vec![prefix.clone()],
            kinds: vec![K::AddressReference as i32],
            view: api::View::Network as i32,
            columns: vec!["address".into(), "description".into()],
            search: term.into(),
            limit: 1,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let R::Page(page) = call(&store, &operator(), C::Query(query.clone()))? else {
            panic!("page")
        };
        eprintln!(
            "prefix search: term={term}, elapsed={:?}, total={}, retained_bytes={:?}",
            started.elapsed(),
            page.total,
            before.bytes
        );
        pages.push((query, page));
    }
    #[cfg(feature = "diagnostics")]
    store.set_diagnostic_query_check(|db, statement| {
        if statement.contains("prefix_scope") {
            let plan = db.explain_cypher(statement).expect("scoped search plan");
            if statement.contains("(s:OmRow") {
                assert!(plan.contains("IndexScan(s:"), "{plan}");
            }
            if statement.contains("(ip:OmRow") {
                assert!(plan.contains("IndexScan(ip:"), "{plan}");
            }
            eprintln!(
                "scoped search scans: {}",
                plan.lines()
                    .filter(|line| line.contains("Scan("))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
    });
    for (mut query, page) in pages {
        let R::Page(repeated) = call(&store, &operator(), C::Query(query.clone()))? else {
            panic!("planned page")
        };
        assert_eq!(repeated, page);
        assert_eq!(page.total, if query.search == "needle" { 2 } else { 1 });
        assert_eq!(page.items.len(), 1);
        assert!(
            selected
                .iter()
                .any(|target| page.items[0].target.as_ref() == Some(target))
        );
        if query.search == "needle" {
            query.cursor = Some(page.next.ok_or("next page")?);
            let R::Page(next) = call(&store, &operator(), C::Query(query))? else {
                panic!("next")
            };
            assert_eq!(next.total, 2);
            assert_eq!(next.items.len(), 1);
            assert!(
                selected
                    .iter()
                    .any(|target| next.items[0].target.as_ref() == Some(target))
            );
            assert_ne!(page.items[0].target, next.items[0].target);
            assert!(next.next.is_none());
        } else {
            assert!(page.next.is_none());
        }
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Equal link-local CIDRs in one address space remain distinct on different links.
#[test]
fn link_local_prefix_membership_checks_the_allocation_link() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let space = create(&store, &operator(), K::AddressSpace, "namespace")?;
    let first = create(&store, &operator(), K::L2Domain, "first link")?;
    let other = create(&store, &operator(), K::L2Domain, "other link")?;
    let replacement = create(&store, &operator(), K::L2Domain, "replacement link")?;
    for (cidr, address, host, zone) in [
        ("fe80::/64", "fe80::8", "fe80::8/128", Some("en0")),
        ("169.254.0.0/16", "169.254.0.8", "169.254.0.8/32", None),
    ] {
        let mut prefixes = Vec::new();
        for link in [&first, &other] {
            let R::Mutation(saved) = command_at(
                &store,
                C::Network(api::Network {
                    kind: K::Prefix as i32,
                    cidr: cidr.into(),
                    address_space: Some(space.clone()),
                    link: Some(link.clone()),
                    sharing: api::SharedUse::Exclusive as i32,
                    ..Default::default()
                }),
                revision(&store)?,
            )?
            else {
                panic!("prefix")
            };
            prefixes.push(saved.targets[0].clone());
        }
        let R::Mutation(allocation) = command_at(
            &store,
            C::Network(api::Network {
                kind: K::IpAddress as i32,
                name: "link-local address".into(),
                cidr: host.into(),
                address_space: Some(space.clone()),
                link: Some(other.clone()),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("allocation")
        };
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some(address.into()),
                zone: zone.map(str::to_owned),
                binding: Some(allocation.targets[0].clone()),
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("address")
        };
        let target = saved.targets[0].clone();
        let membership = |prefix: &wire::AssetRef| {
            C::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(target.clone()),
                    to: Some(prefix.clone()),
                    kind: wire::RelationKind::MemberOf as i32,
                    ..Default::default()
                }),
                ..Default::default()
            })
        };
        let before = store.usage()?;
        assert!(matches!(
            command_at(&store, membership(&prefixes[0]), revision(&store)?),
            Err(Error::Conflict)
        ));
        assert_eq!(store.usage()?, before);
        let mut restricted = operator();
        restricted.grants[0].read = Fields::Selected(
            [
                "name",
                "relations",
                "address",
                "binding",
                "cidr",
                "address_space",
                "network",
                "zone",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
        for prefix in &prefixes {
            assert!(matches!(
                store.workspace(
                    &restricted,
                    api::Request {
                        command: Some(membership(prefix)),
                        working: Some(api::WorkingContext {
                            expected: Some(revision(&store)?),
                            ..Default::default()
                        }),
                    },
                    100
                ),
                Err(Error::Denied)
            ));
            assert_eq!(store.usage()?, before);
        }
        command_at(&store, membership(&prefixes[1]), revision(&store)?)?;
        // A link-only edit must not leave the member bound to its old allocation link.
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(prefixes[1].clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("prefix detail")
        };
        let before = store.usage()?;
        assert!(matches!(
            command_at(
                &store,
                C::Network(api::Network {
                    target: Some(prefixes[1].clone()),
                    expected: Some(detail.item.ok_or("prefix item")?.revision),
                    kind: K::Prefix as i32,
                    cidr: cidr.into(),
                    address_space: Some(space.clone()),
                    link: Some(replacement.clone()),
                    sharing: api::SharedUse::Exclusive as i32,
                    ..Default::default()
                }),
                revision(&store)?
            ),
            Err(Error::Conflict)
        ));
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

// Field-level syntax checks must not discard a retained mask or a separate CSV mask column.
#[test]
fn generic_and_csv_broadcast_use_the_complete_mask() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.8/24".into()),
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("address")
    };
    let target = saved.targets[0].clone();
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    let edited = command_at(
        &store,
        C::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            kind: K::AddressReference as i32,
            changes: vec![text_field("address", "192.0.2.8 brd 192.0.2.254")],
            ..Default::default()
        }),
        revision(&store)?,
    );
    assert!(
        edited.is_ok(),
        "generic edit must use the retained /24 mask"
    );
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(target),
            ..Default::default()
        }),
    )?
    else {
        panic!("edited detail")
    };
    assert!(
        detail
            .item
            .ok_or("item")?
            .fields
            .contains(&text_field("broadcast", "192.0.2.254"))
    );
    for mask in ["24", "31", ""] {
        let R::Preview(preview) = command_at(
            &store,
            C::Import(api::Import {
                source: "broadcast-columns".into(),
                kind: K::AddressReference as i32,
                csv: format!(
                    "external_id,address,prefix_length\nendpoint,192.0.2.9 brd 192.0.2.254,{mask}\n"
                ),
                mappings: ["address", "prefix_length"]
                    .into_iter()
                    .map(|name| api::Mapping {
                        source: name.into(),
                        field: name.into(),
                    })
                    .collect(),
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("CSV preview")
        };
        assert_eq!(preview.id.is_some(), mask == "24");
        assert_eq!(preview.rows.len(), 1);
        assert_eq!(preview.rows[0].diagnostic.is_empty(), mask == "24");
        if mask == "24" {
            command_at(
                &store,
                C::Import(api::Import {
                    preview: preview.id,
                    ..Default::default()
                }),
                preview.revision,
            )?;
        }
    }
    Ok(())
}

// Title context must preserve all selected columns, enforce CIDR rights and never write.
#[test]
fn prefix_title_context_preserves_thirty_two_selected_fields() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Network(api::Network {
            kind: K::Prefix as i32,
            name: "Wide prefix".into(),
            cidr: "192.0.2.0/24".into(),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("prefix")
    };
    let target = saved.targets[0].clone();
    let mut fields = vec![text_field("name", "Wide prefix")];
    for index in 0..31 {
        let R::Definitions(definitions) = call(
            &store,
            &operator(),
            C::Definition(api::Definition {
                name: format!("Selected field {index}"),
                r#type: api::FieldType::Text as i32,
                kinds: vec![K::Prefix as i32],
                ..Default::default()
            }),
        )?
        else {
            panic!("definition")
        };
        fields.push(text_field(
            &definitions.items[0].id,
            &format!("value {index}"),
        ));
    }
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix detail")
    };
    command_at(
        &store,
        C::Write(api::Write {
            target: Some(target),
            expected: Some(detail.item.ok_or("prefix item")?.revision),
            kind: K::Prefix as i32,
            changes: fields[1..].to_vec(),
            ..Default::default()
        }),
        revision(&store)?,
    )?;
    let query = api::Query {
        view: api::View::Network as i32,
        kinds: vec![K::Prefix as i32],
        columns: fields.iter().map(|f| f.name.clone()).collect(),
        include_context: true,
        ..query()
    };
    let before = store.usage()?;
    for context in [true, false] {
        let mut selected = query.clone();
        selected.include_context = context;
        let R::Page(page) = call(&store, &operator(), C::Query(selected))? else {
            panic!("page")
        };
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].fields.len(), 32);
        for field in &fields {
            assert!(page.items[0].fields.contains(field));
        }
        if context {
            assert_eq!(
                page.contexts[0].prefix_cidr.as_deref(),
                Some("192.0.2.0/24")
            );
        } else {
            assert!(page.contexts.is_empty());
        }
    }
    let mut restricted = operator();
    restricted.grants[0].read = Fields::Selected(query.columns.clone());
    let R::Page(page) = call(&store, &restricted, C::Query(query.clone()))? else {
        panic!("restricted page")
    };
    assert_eq!(page.items[0].fields.len(), 32);
    assert!(page.contexts[0].prefix_cidr.is_none());
    for (actor, cidr) in [(operator(), "192.0.2.0/24"), (restricted, "")] {
        let R::Download(download) = call(
            &store,
            &actor,
            C::Export(api::Export {
                query: Some(query.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("CSV export")
        };
        let mut csv = csv::Reader::from_reader(download.csv.as_bytes());
        let headers = csv.headers()?.clone();
        let row = csv.records().next().ok_or("CSV row")??;
        assert_eq!(headers.len(), 36);
        for field in &fields {
            let column = headers
                .iter()
                .position(|name| name == field.name)
                .ok_or("selected column")?;
            let Some(wire::field::Value::Text(expected)) = field.value.as_ref() else {
                panic!("text")
            };
            assert_eq!(row.get(column), Some(expected.as_str()));
        }
        assert_eq!(
            row.get(
                headers
                    .iter()
                    .position(|name| name == "cidr")
                    .ok_or("CIDR column")?
            ),
            Some(cidr)
        );
    }
    let mut oversized = query;
    oversized.columns.push("cidr".into());
    assert!(matches!(
        call(&store, &operator(), C::Query(oversized)),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Reference names are a bounded read projection, independently authorized and never persisted.
#[test]
fn prefix_comparison_labels_respect_reference_permissions() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let space = create(&store, &operator(), K::AddressSpace, "Home namespace")?;
    let link = create(&store, &operator(), K::L2Domain, "Home LAN")?;
    let routing = create(&store, &operator(), K::Vrf, "Home forwarding")?;
    let mut targets = Vec::new();
    for canonical in [true, false] {
        let R::Mutation(saved) = command_at(
            &store,
            C::Network(api::Network {
                kind: K::Prefix as i32,
                cidr: "192.0.2.0/24".into(),
                address_space: canonical.then(|| space.clone()),
                link: canonical.then(|| link.clone()),
                routing: canonical.then(|| routing.clone()),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("prefix")
        };
        targets.push(saved.targets[0].clone());
    }
    let command = C::CompareHostIdentity(api::CompareHostIdentity {
        left: Some(targets[0].clone()),
        right: Some(targets[1].clone()),
        ..Default::default()
    });
    let usage = store.usage()?;
    let R::HostIdentityComparison(comparison) = call(&store, &operator(), command.clone())? else {
        panic!("comparison")
    };
    let labels = comparison.left.ok_or("left subject")?.domain_labels;
    assert_eq!(
        labels,
        vec![
            text_field("address_space", "Home namespace"),
            text_field("link_scope", "Home LAN"),
            text_field("routing_context", "Home forwarding")
        ]
    );
    assert!(
        comparison
            .right
            .ok_or("right subject")?
            .domain_labels
            .is_empty()
    );
    let mut restricted = operator();
    restricted.grants[0].scope = Scope::Kinds(vec![K::Prefix]);
    let R::HostIdentityComparison(comparison) = call(&store, &restricted, command.clone())? else {
        panic!("restricted comparison")
    };
    let subject = comparison.left.ok_or("restricted subject")?;
    assert!(subject.domain_labels.is_empty());
    assert!(
        subject
            .domain_fields
            .iter()
            .any(|f| f.name == "address_space")
    );
    let mut hidden_names = operator();
    hidden_names.grants[0].read = Fields::Selected(vec![
        "cidr".into(),
        "address_space".into(),
        "link_scope".into(),
        "routing_context".into(),
    ]);
    let R::HostIdentityComparison(comparison) = call(&store, &hidden_names, command.clone())?
    else {
        panic!("name-restricted comparison")
    };
    let subject = comparison.left.ok_or("name-restricted subject")?;
    assert!(subject.domain_labels.is_empty());
    assert!(
        subject
            .domain_fields
            .iter()
            .any(|f| f.name == "address_space")
    );
    // Readable names must not reveal the denied relationship to an address space.
    let mut hidden_reference = operator();
    hidden_reference.grants[0].read = Fields::Selected(vec![
        "name".into(),
        "cidr".into(),
        "link_scope".into(),
        "routing_context".into(),
    ]);
    let R::HostIdentityComparison(comparison) = call(&store, &hidden_reference, command)? else {
        panic!("reference-restricted comparison")
    };
    let subject = comparison.left.ok_or("reference-restricted subject")?;
    assert!(
        subject
            .domain_fields
            .iter()
            .all(|f| f.name != "address_space")
    );
    assert!(
        subject
            .domain_labels
            .iter()
            .all(|f| f.name != "address_space")
    );
    assert_eq!(store.usage()?, usage);
    Ok(())
}

// A combined network refinement and label removal is one authorized conditional write.
#[test]
fn prefix_cidr_and_label_clear_commit_together() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let space = create(&store, &operator(), K::AddressSpace, "namespace")?;
    for namespace in [None, Some(space)] {
        let R::Mutation(saved) = command_at(
            &store,
            C::Network(api::Network {
                kind: K::Prefix as i32,
                name: "Temporary label".into(),
                cidr: "192.0.2.0/24".into(),
                address_space: namespace,
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("prefix")
        };
        let target = saved.targets[0].clone();
        let before_invalid = store.usage()?;
        for kind in [K::Prefix, K::IpAddress, K::Vlan, K::AddressGroup] {
            assert!(matches!(
                command_at(
                    &store,
                    C::Network(api::Network {
                        target: Some(target.clone()),
                        kind: kind as i32,
                        name: if kind == K::Prefix {
                            "Contradictory label".into()
                        } else {
                            String::new()
                        },
                        clear_name: true,
                        cidr: "192.0.2.0/25".into(),
                        sharing: api::SharedUse::Exclusive as i32,
                        ..Default::default()
                    }),
                    revision(&store)?,
                ),
                Err(Error::Invalid)
            ));
            assert_eq!(store.usage()?, before_invalid);
        }
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(target.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("prefix detail")
        };
        let write = api::Write {
            target: Some(target.clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            kind: K::Prefix as i32,
            changes: vec![
                text_field("cidr", "192.0.2.0/25"),
                wire::Field {
                    name: "name".into(),
                    value: Some(wire::field::Value::Clear(wire::NullValue::NullValue as i32)),
                },
            ],
            ..Default::default()
        };
        command_at(&store, C::Write(write.clone()), revision(&store)?)?;
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(target.clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("refined prefix")
        };
        let item = detail.item.ok_or("item")?;
        assert_eq!(item.target.as_ref(), Some(&target));
        assert_eq!(
            item.fields.iter().find(|f| f.name == "cidr"),
            Some(&text_field("cidr", "192.0.2.0/25"))
        );
        assert!(
            item.fields
                .iter()
                .all(|f| f.name != "name" || matches!(f.value, Some(wire::field::Value::Clear(_))))
        );
        let mut repeated = write;
        repeated.expected = Some(item.revision);
        let before = store.usage()?;
        command_at(&store, C::Write(repeated.clone()), revision(&store)?)?;
        assert_eq!(store.usage()?, before);
        let mut restricted = operator();
        restricted.grants[0].write = Fields::Selected(vec!["cidr".into(), "network".into()]);
        repeated.changes[0] = text_field("cidr", "192.0.2.0/24");
        assert!(matches!(
            store.workspace(
                &restricted,
                api::Request {
                    command: Some(C::Write(repeated)),
                    working: Some(api::WorkingContext {
                        expected: Some(revision(&store)?),
                        ..Default::default()
                    }),
                },
                100
            ),
            Err(Error::Denied)
        ));
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

// Generic relationship commands cannot bypass the address owner's range invariant.
#[test]
fn ordinary_membership_validates_the_known_address() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut prefixes = Vec::new();
    for cidr in ["198.51.100.0/24", "192.0.2.0/24"] {
        let R::Mutation(saved) = command_at(
            &store,
            C::Network(api::Network {
                kind: K::Prefix as i32,
                cidr: cidr.into(),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("prefix")
        };
        prefixes.push(saved.targets[0].clone());
    }
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.31".into()),
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("address")
    };
    let membership = |prefix| {
        C::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(saved.targets[0].clone()),
                to: Some(prefix),
                kind: wire::RelationKind::MemberOf as i32,
                ..Default::default()
            }),
            ..Default::default()
        })
    };
    let usage = store.usage()?;
    assert!(matches!(
        command_at(&store, membership(prefixes[0].clone()), revision(&store)?),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    command_at(&store, membership(prefixes[1].clone()), revision(&store)?)?;
    Ok(())
}

// The generic API and CSV field vocabulary share the reviewed creation outcome contract.
#[test]
fn generic_knowledge_creation_recovers_its_original_outcome() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    for (kind, changes) in [
        (K::Prefix, vec![text_field("cidr", "198.51.100.0/24")]),
        (
            K::AddressReference,
            vec![
                text_field("address", "192.0.2.4/24"),
                wire::Field {
                    name: "create_prefix".into(),
                    value: Some(wire::field::Value::Flag(true)),
                },
            ],
        ),
    ] {
        let base = revision(&store)?;
        let command = C::Write(api::Write {
            kind: kind as i32,
            changes,
            ..Default::default()
        });
        let saved = command_at(&store, command.clone(), base)?;
        let usage = store.usage()?;
        assert_eq!(command_at(&store, command, base)?, saved);
        assert_eq!(store.usage()?, usage);
    }
    Ok(())
}

// Upgrade the exact prior property set on disk, retaining accepted identities and data.
#[test]
fn schema_22_catalog_additively_upgrades_for_known_addresses() -> TestResult {
    use coordinode_embed::{Intent, LabelDeclaration, SchemaPackage};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let existing = create(&store, &operator(), K::Device, "Existing inventory")?;
    let db = store.diagnostic_database();
    let mut schema = db
        .label_schemas()?
        .into_iter()
        .find(|schema| schema.name == "OmRow")
        .ok_or("schema")?;
    let expected_revision = schema.schema_revision;
    schema.properties.remove("broadcast");
    schema.properties.remove("targets");
    schema
        .remove_constraint("omloom_key")
        .ok_or("existing unique constraint")?;
    db.execute_cypher_shared(
        "MATCH (n:OmRow {key:'installation'}) SET n.`schema`=22",
        None,
        None,
        None,
        None,
    )?;
    let published = db.publish_schema(SchemaPackage {
        labels: vec![LabelDeclaration {
            schema,
            intent: Intent::Change { expected_revision },
        }],
        edge_types: Vec::new(),
        constraints: Vec::new(),
    })?;
    assert!(
        db.await_schema(&published, std::time::Duration::from_secs(20))?
            .all_ready()
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(existing.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("existing inventory");
    };
    assert_eq!(detail.item.ok_or("item")?.target, Some(existing));
    let schema = store
        .diagnostic_database()
        .label_schemas()?
        .into_iter()
        .find(|schema| schema.name == "OmRow")
        .ok_or("schema")?;
    assert!(schema.properties.contains_key("broadcast"));
    assert!(schema.properties.contains_key("targets"));
    command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.19/24 brd 192.0.2.254".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?;
    Ok(())
}

// A genuinely optional prefix label can be explicitly cleared without replacing the network.
#[test]
fn prefix_optional_name_can_be_cleared_without_changing_identity() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Network(api::Network {
            name: "Temporary label".into(),
            kind: K::Prefix as i32,
            cidr: "192.0.2.0/24".into(),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("prefix");
    };
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("read");
    };
    command_at(
        &store,
        C::Write(api::Write {
            target: Some(saved.targets[0].clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            kind: K::Prefix as i32,
            changes: vec![wire::Field {
                name: "name".into(),
                value: Some(wire::field::Value::Clear(wire::NullValue::NullValue as i32)),
            }],
            ..Default::default()
        }),
        revision(&store)?,
    )?;
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("read");
    };
    let item = detail.item.ok_or("item")?;
    assert_eq!(item.target, Some(saved.targets[0].clone()));
    assert_eq!(
        item.fields.iter().find(|field| field.name == "cidr"),
        Some(&text_field("cidr", "192.0.2.0/24"))
    );
    assert!(
        item.fields.iter().all(|field| field.name != "name"
            || matches!(field.value, Some(wire::field::Value::Clear(_))))
    );
    Ok(())
}

// An explicit new-prefix command must not silently reuse an equal current membership.
#[test]
fn explicit_new_prefix_remains_independent_of_equal_current_prefix() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(first) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.19/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("create");
    };
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(first.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    let R::Mutation(second) = command_at(
        &store,
        C::Address(api::Address {
            target: Some(first.targets[0].clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            address: Some("192.0.2.19/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("new prefix");
    };
    assert_eq!(second.targets[0], first.targets[0]);
    assert_ne!(second.targets[1], first.targets[1]);
    Ok(())
}

// Narrowing a prefix cannot leave its declared members outside the new range.
#[test]
fn prefix_cidr_refinement_revalidates_declared_members() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.19/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("create");
    };
    let R::Detail(prefix) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix");
    };
    let before = store.usage()?;
    assert!(matches!(
        command_at(
            &store,
            C::Network(api::Network {
                target: Some(saved.targets[1].clone()),
                expected: Some(prefix.item.ok_or("item")?.revision),
                kind: K::Prefix as i32,
                cidr: "192.0.2.128/25".into(),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Editing the address cannot preserve a now-false membership in an unrelated prefix.
#[test]
fn changing_known_address_revalidates_its_preserved_prefix() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.10/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("create");
    };
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    let before = store.usage()?;
    assert!(matches!(
        command_at(
            &store,
            C::Address(api::Address {
                target: Some(saved.targets[0].clone()),
                expected: Some(detail.item.ok_or("item")?.revision),
                address: Some("198.51.100.10".into()),
                ..Default::default()
            }),
            revision(&store)?
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Namespace refinement cannot contradict an address's already explicit canonical binding.
#[test]
fn prefix_namespace_refinement_rechecks_bound_addresses_atomically() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let first = create(&store, &operator(), K::AddressSpace, "First namespace")?;
    let other = create(&store, &operator(), K::AddressSpace, "Other namespace")?;
    let R::Mutation(allocation) = call(
        &store,
        &operator(),
        C::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "Allocated endpoint".into(),
            cidr: "192.0.2.10/32".into(),
            address_space: Some(first),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
    )?
    else {
        panic!("allocation");
    };
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.10/24".into()),
            binding: Some(allocation.targets[0].clone()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("known address");
    };
    let R::Detail(prefix) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix");
    };
    let before = store.usage()?;
    assert!(matches!(
        command_at(
            &store,
            C::Network(api::Network {
                target: Some(saved.targets[1].clone()),
                expected: Some(prefix.item.ok_or("item")?.revision),
                kind: K::Prefix as i32,
                cidr: "192.0.2.0/24".into(),
                address_space: Some(other),
                sharing: api::SharedUse::Exclusive as i32,
                ..Default::default()
            }),
            revision(&store)?
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Reviewing unchanged membership must not reveal a prefix field that the actor cannot read.
#[test]
fn address_review_filters_hidden_prefix_metadata() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.10/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("create");
    };
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    let mut restricted = operator();
    restricted.grants[0].read = Fields::Selected(vec!["address".into(), "relations".into()]);
    let before = store.usage()?;
    let reply = store.workspace(
        &restricted,
        api::Request {
            command: Some(C::PreviewAddress(api::Address {
                target: Some(saved.targets[0].clone()),
                expected: Some(detail.item.ok_or("item")?.revision),
                prefix: Some(saved.targets[1].clone()),
                ..Default::default()
            })),
            working: Some(api::WorkingContext {
                expected: Some(revision(&store)?),
                ..Default::default()
            }),
        },
        100,
    )?;
    let Some(R::AddressReview(review)) = reply.result else {
        panic!("preview");
    };
    assert_eq!(review.address.as_deref(), Some("192.0.2.10"));
    assert!(review.prefix_cidr.is_none());
    assert_eq!(store.usage()?, before);
    Ok(())
}

// An explicit Prefix context supplies membership, never an invented owning machine or allocation.
#[test]
fn known_address_created_in_prefix_uses_the_existing_prefix() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(first) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.10/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("create");
    };
    let reply = store.workspace(
        &operator(),
        api::Request {
            command: Some(C::Address(api::Address {
                address: Some("192.0.2.20".into()),
                ..Default::default()
            })),
            working: Some(api::WorkingContext {
                focus: Some(first.targets[1].clone()),
                expected: Some(revision(&store)?),
                ..Default::default()
            }),
        },
        100,
    )?;
    let Some(R::Mutation(saved)) = reply.result else {
        panic!("scoped creation");
    };
    assert_eq!(saved.targets[1], first.targets[1]);
    let R::Page(page) = call(
        &store,
        &operator(),
        C::Query(api::Query {
            focus: Some(first.targets[1].clone()),
            kinds: vec![K::AddressReference as i32],
            ..query()
        }),
    )?
    else {
        panic!("members");
    };
    assert_eq!(page.total, 2);
    Ok(())
}

// Concurrent delivery shares one atomic outcome; expired detail cannot admit the old request again.
#[test]
fn known_address_concurrent_creation_and_expired_outcome_are_fenced() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let base = revision(&store)?;
    let request = api::Address {
        address: Some("192.0.2.19/24".into()),
        create_prefix: true,
        ..Default::default()
    };
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            command_at(&store, C::Address(request.clone()), base)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            command_at(&store, C::Address(request.clone()), base)
        });
        [
            a.join().expect("creation worker"),
            b.join().expect("creation worker"),
        ]
    });
    let replay = command_at(&store, C::Address(request.clone()), base)?;
    for result in results {
        match result {
            Ok(outcome) => assert_eq!(outcome, replay),
            Err(Error::Conflict) => {}
            Err(error) => return Err(error.into()),
        }
    }
    let R::Page(page) = call(
        &store,
        &operator(),
        C::Query(api::Query {
            kinds: vec![K::Prefix as i32],
            ..query()
        }),
    )?
    else {
        panic!("prefixes");
    };
    assert_eq!(page.total, 1);
    let mut cursor = None;
    loop {
        let report = store.cleanup(2000, cursor.as_deref())?;
        cursor = report.cursor;
        if cursor.is_none() {
            break;
        }
    }
    let before = store.usage()?;
    assert!(matches!(
        command_at(&store, C::Address(request), base),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Updating or clearing an assignment mask does not silently modify its shared prefix.
#[test]
fn known_address_mask_refinement_preserves_prefix_identity_and_cidr() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let R::Mutation(saved) = command_at(
        &store,
        C::Address(api::Address {
            address: Some("192.0.2.19/24".into()),
            create_prefix: true,
            ..Default::default()
        }),
        revision(&store)?,
    )?
    else {
        panic!("create");
    };
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address");
    };
    command_at(
        &store,
        C::Address(api::Address {
            target: Some(saved.targets[0].clone()),
            expected: Some(detail.item.ok_or("item")?.revision),
            prefix_length: Some(25),
            ..Default::default()
        }),
        revision(&store)?,
    )?;
    let R::Detail(prefix) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix");
    };
    assert_eq!(
        prefix
            .item
            .ok_or("item")?
            .fields
            .iter()
            .find(|f| f.name == "cidr"),
        Some(&text_field("cidr", "192.0.2.0/24"))
    );
    Ok(())
}

// A prefix view follows declared membership, never equal CIDR text or hidden resources.
#[test]
fn known_prefix_scope_is_explicit_visible_and_survives_address_removal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let mut created = Vec::new();
    for ip in ["192.0.2.10/24", "192.0.2.11/24"] {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some(ip.into()),
                create_prefix: true,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("address");
        };
        created.push(saved.targets);
    }
    let mut basis = revision(&store)?;
    for suffix in 20..120 {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some(format!("192.0.2.{suffix}")),
                prefix: Some(created[0][1].clone()),
                ..Default::default()
            }),
            basis,
        )?
        else {
            panic!("member");
        };
        basis = saved.revision;
    }
    let scoped = api::Query {
        focus: Some(created[0][1].clone()),
        kinds: vec![K::AddressReference as i32],
        columns: vec!["address".into()],
        ..query()
    };
    store.set_diagnostic_query_check(|db, statement| {
        if !statement.contains("membership:OmRow") {
            return;
        }
        let plan = db.explain_plan(statement, None).expect("membership plan");
        let outer = plan.explain();
        assert!(
            outer.contains("IndexScan(membership:OmRow ON omloom_to_asset(to_asset))"),
            "prefix browse must start from its incoming memberships: {outer}"
        );
        assert!(
            !outer.contains("IndexScan(a:OmRow ON omloom_parent(parent))"),
            "prefix browse must not scan the global asset population: {outer}"
        );
    });
    let before = store.usage()?;
    let start = std::time::Instant::now();
    let R::Page(page) = call(&store, &operator(), C::Query(scoped.clone()))? else {
        panic!("page");
    };
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "a prefix page must remain within the existing browse latency bound"
    );
    assert_eq!(page.total, 101);
    assert_eq!(page.items.len(), 20);
    assert_eq!(page.items[0].target.as_ref(), Some(&created[0][0]));
    assert_eq!(store.usage()?, before);
    let mut restricted = operator();
    restricted.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&created[0][1])?]);
    let R::Page(hidden) = call(&store, &restricted, C::Query(scoped))? else {
        panic!("hidden page");
    };
    assert_eq!(hidden.total, 0);
    assert!(hidden.items.is_empty());
    let base = revision(&store)?;
    command_at(
        &store,
        C::Bulk(api::Bulk {
            targets: vec![created[0][0].clone()],
            expected: base,
            remove: true,
            ..Default::default()
        }),
        base,
    )?;
    let R::Detail(prefix) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(created[0][1].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix");
    };
    assert_eq!(
        prefix.item.ok_or("prefix item")?.target.as_ref(),
        Some(&created[0][1])
    );
    assert!(
        prefix
            .relations
            .iter()
            .all(|relation| relation.from.as_ref() != Some(&created[0][0]))
    );
    let R::Page(remaining) = call(
        &store,
        &operator(),
        C::Query(api::Query {
            focus: Some(created[0][1].clone()),
            kinds: vec![K::AddressReference as i32],
            ..query()
        }),
    )?
    else {
        panic!("remaining members");
    };
    assert_eq!(remaining.total, 100);
    Ok(())
}

// A logical backup retains the exact accepted outcome and rejects a different request at its revision.
#[test]
fn known_address_outcome_survives_export_restore_and_conflicting_replay() -> TestResult {
    let dir = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let base = revision(&store)?;
    let request = api::Address {
        address: Some("192.0.2.0/31".into()),
        create_prefix: true,
        ..Default::default()
    };
    let saved = command_at(&store, C::Address(request.clone()), base)?;
    let before = store.usage()?;
    assert!(matches!(
        command_at(
            &store,
            C::Address(api::Address {
                address: Some("192.0.2.2/31".into()),
                ..request.clone()
            }),
            base
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored = Store::restore(destination.path(), &image, policy(), image.len() as u64)?;
    assert_eq!(command_at(&restored, C::Address(request), base)?, saved);
    Ok(())
}

// Preview has no durable effect; lost commit replies recover the same two identities.
#[test]
fn known_address_atomic_preview_replay_and_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let base = revision(&store)?;
    let request = api::Address {
        address: Some("192.0.2.19/24 brd 192.0.2.254".into()),
        create_prefix: true,
        ..Default::default()
    };
    let usage = store.usage()?;
    let R::AddressReview(review) = command_at(&store, C::PreviewAddress(request.clone()), base)?
    else {
        panic!("review")
    };
    assert_eq!(review.address.as_deref(), Some("192.0.2.19"));
    assert_eq!(review.prefix_cidr.as_deref(), Some("192.0.2.0/24"));
    assert_eq!(review.broadcast.as_deref(), Some("192.0.2.254"));
    assert_eq!(store.usage()?, usage);
    let R::Mutation(saved) = command_at(&store, C::Address(request.clone()), base)? else {
        panic!("commit")
    };
    assert_eq!(saved.targets.len(), 2);
    let usage = store.usage()?;
    assert_eq!(
        command_at(&store, C::Address(request.clone()), base)?,
        R::Mutation(saved.clone())
    );
    assert_eq!(store.usage()?, usage);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        command_at(&store, C::Address(request), base)?,
        R::Mutation(saved.clone())
    );
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(saved.targets[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert!(
        detail
            .relations
            .iter()
            .any(|r| r.kind == wire::RelationKind::MemberOf as i32
                && r.to.as_ref() == Some(&saved.targets[1]))
    );
    Ok(())
}

// Zero is an explicit mask; scoped IPv6 preserves its host-local zone and never gains broadcast.
#[test]
fn known_address_mask_boundaries_and_zone_stay_distinct() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    for (literal, mask, zone, cidr) in [
        ("192.0.2.1", None, None, None),
        ("192.0.2.2/0", Some(0_u32), None, Some("0.0.0.0/0")),
        ("192.0.2.3/32", Some(32), None, Some("192.0.2.3/32")),
        ("fe80::1%en0/64", Some(64), Some("en0"), Some("fe80::/64")),
    ] {
        let request = api::Address {
            address: Some(literal.into()),
            create_prefix: mask.is_some(),
            ..Default::default()
        };
        let base = revision(&store)?;
        let usage = store.usage()?;
        let R::AddressReview(review) =
            command_at(&store, C::PreviewAddress(request.clone()), base)?
        else {
            panic!("review")
        };
        assert_eq!(review.prefix_length, mask);
        assert_eq!(review.zone.as_deref(), zone);
        assert_eq!(review.prefix_cidr.as_deref(), cidr);
        assert_eq!(review.broadcast, None);
        assert_eq!(store.usage()?, usage);
        let R::Mutation(saved) = command_at(&store, C::Address(request), base)? else {
            panic!("saved")
        };
        let R::Detail(detail) = call(
            &store,
            &operator(),
            C::Read(api::Read {
                target: Some(saved.targets[0].clone()),
                ..Default::default()
            }),
        )?
        else {
            panic!("detail")
        };
        let fields = detail.item.ok_or("item")?.fields;
        assert_eq!(
            fields
                .iter()
                .find(|field| field.name == "prefix_length")
                .and_then(|field| field.value.as_ref()),
            mask.map(|value| wire::field::Value::Integer(i64::from(value)))
                .as_ref()
        );
    }
    Ok(())
}

// The same CIDR is not identity; invalid syntax/broadcast cannot leave partial resources.
#[test]
fn unresolved_networks_do_not_merge_by_cidr_or_accept_invalid_broadcast() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut previous = None;
    for _ in 0..2 {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some("2001:db8::19/64".into()),
                create_prefix: true,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("create")
        };
        assert_ne!(previous.as_ref(), saved.targets.get(1));
        previous = saved.targets.get(1).cloned();
    }
    let usage = store.usage()?;
    for raw in [
        "192.0.2.3/31 brd 192.0.2.3",
        "192.0.2.3/32 brd 192.0.2.3",
        "2001:db8::3/64 brd 192.0.2.255",
        "192.0.2.3/24 dev en0",
        "192.0.2.3/33",
        "192.0.2.3/+24",
    ] {
        assert!(matches!(
            command_at(
                &store,
                C::Address(api::Address {
                    address: Some(raw.into()),
                    create_prefix: true,
                    ..Default::default()
                }),
                revision(&store)?
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, usage);
    }
    Ok(())
}

// Explicit duplicate correction rewires current membership and keeps the selected survivor.
#[test]
fn prefix_merge_preserves_membership_and_can_be_reversed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut created = Vec::new();
    for _ in 0..2 {
        let R::Mutation(saved) = command_at(
            &store,
            C::Address(api::Address {
                address: Some("192.0.2.10/24".into()),
                create_prefix: true,
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("create")
        };
        created.push(saved.targets);
    }
    let R::IdentityCorrection(preview) = command_at(
        &store,
        C::CorrectIdentity(api::IdentityCorrection {
            decision: api::IdentityDecision::DuplicateResource as i32,
            mappings: vec![api::IdentityMapping {
                duplicate: Some(created[0][1].clone()),
                retained: Some(created[1][1].clone()),
                ..Default::default()
            }],
            ..Default::default()
        }),
        revision(&store)?,
    )
    .map_err(|error| format!("prefix merge preview: {error}"))?
    else {
        panic!("merge preview")
    };
    let R::IdentityCorrected(receipt) = call(
        &store,
        &operator(),
        C::CorrectIdentity(api::IdentityCorrection {
            preview: Some(preview.id),
            ..Default::default()
        }),
    )
    .map_err(|error| format!("prefix merge commit: {error}"))?
    else {
        panic!("merged")
    };
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(created[0][0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("address")
    };
    assert!(
        detail
            .relations
            .iter()
            .any(|r| r.to.as_ref() == Some(&created[1][1]))
    );
    call(
        &store,
        &operator(),
        C::CorrectIdentity(api::IdentityCorrection {
            undo: Some(receipt.id),
            ..Default::default()
        }),
    )
    .map_err(|error| format!("prefix merge undo: {error}"))?;
    let R::Detail(detail) = call(
        &store,
        &operator(),
        C::Read(api::Read {
            target: Some(created[0][0].clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("restored address")
    };
    assert!(
        detail
            .relations
            .iter()
            .any(|r| r.to.as_ref() == Some(&created[0][1]))
    );
    Ok(())
}

// CSV uses the same address parser and atomic prefix command, preserving source mappings on replay.
#[test]
fn csv_known_address_creates_prefix_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    for _ in 0..2 {
        let R::Preview(preview) = command_at(
            &store,
            C::Import(api::Import {
                source: "network-fixture".into(),
                kind: K::AddressReference as i32,
                csv: "external_id,address,create_prefix,prefix_name,broadcast\nendpoint,192.0.2.31/24,true,Office subnet,\nremote,2001:db8::5,,,\n".into(),
                mappings: ["address", "create_prefix", "prefix_name", "broadcast"]
                    .into_iter()
                    .map(|name| api::Mapping {
                        source: name.into(),
                        field: name.into(),
                    })
                    .collect(),
                ..Default::default()
            }),
            revision(&store)?,
        )?
        else {
            panic!("import preview")
        };
        assert!(preview.id.is_some(), "{preview:?}");
        let command = C::Import(api::Import {
            preview: preview.id,
            ..Default::default()
        });
        let saved = command_at(&store, command.clone(), preview.revision)?;
        let usage = store.usage()?;
        assert_eq!(command_at(&store, command, preview.revision)?, saved);
        assert_eq!(store.usage()?, usage);
    }
    let R::Page(page) = call(
        &store,
        &operator(),
        C::Query(api::Query {
            kinds: vec![K::Prefix as i32],
            ..query()
        }),
    )?
    else {
        panic!("prefixes")
    };
    assert_eq!(page.total, 1);
    Ok(())
}

// A useful prefix can exist before an allocation namespace, without creating one.
#[test]
fn unresolved_prefix_keeps_identity_when_namespace_is_supplied() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let api::reply::Result::Mutation(created) = call(
        &store,
        &actor,
        api::request::Command::Network(api::Network {
            kind: K::Prefix as i32,
            cidr: "192.0.2.19/24".into(),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix result")
    };
    let target = created.targets[0].clone();
    let api::reply::Result::Detail(detail) = call(
        &store,
        &actor,
        api::request::Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("prefix detail")
    };
    let item = detail.item.ok_or(Error::Missing)?;
    assert!(!item.fields.iter().any(|f| f.name == "address_space"));
    assert!(
        item.fields
            .iter()
            .any(|f| f == &text_field("cidr", "192.0.2.0/24"))
    );
    let space = create(&store, &actor, K::AddressSpace, "Managed addresses")?;
    let api::reply::Result::Mutation(refined) = call(
        &store,
        &actor,
        api::request::Command::Network(api::Network {
            target: Some(target.clone()),
            expected: Some(item.revision),
            kind: K::Prefix as i32,
            cidr: "192.0.2.0/24".into(),
            address_space: Some(space),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
    )?
    else {
        panic!("refinement result")
    };
    assert_eq!(refined.targets, vec![target]);
    Ok(())
}
