mod support;
use omloom_contracts::{
    ids::AssetRef,
    wire::{self, AssetKind as K},
    workspace as api,
};
use omloom_store::{Error, Store, workspace::*};
use support::*;
#[path = "workspace/accounts.rs"]
mod accounts;
#[path = "workspace/contexts.rs"]
mod contexts;
#[path = "workspace/correction.rs"]
mod correction;
#[path = "workspace/definitions.rs"]
mod definitions;
#[path = "workspace/exchange.rs"]
mod exchange;
#[path = "workspace/groups.rs"]
mod groups;
#[path = "workspace/host_import.rs"]
mod host_import;
#[path = "workspace/host_network.rs"]
mod host_network;
#[path = "workspace/projections.rs"]
mod projections;
#[path = "workspace/responsibility.rs"]
mod responsibility;
#[path = "workspace/software.rs"]
mod software;
#[path = "workspace/views.rs"]
mod views;

fn operator() -> Context {
    Context {
        actor: "fixture-operator".into(),
        fixture: "operator".into(),
        grants: vec![Grant {
            scope: Scope::All,
            read: Fields::All,
            write: Fields::All,
            writable: true,
        }],
        teams: vec!["operations".into()],
        recipients: vec!["fixture-developer".into()],
        schema_admin: true,
        authority_revision: 1,
    }
}
fn call(
    store: &Store,
    context: &Context,
    command: api::request::Command,
) -> Result<api::reply::Result, Error> {
    store
        .workspace(
            context,
            api::Request {
                command: Some(command),
                working: Some(api::WorkingContext::default()),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)
}
fn create(store: &Store, context: &Context, kind: K, name: &str) -> Result<wire::AssetRef, Error> {
    let api::reply::Result::Mutation(m) = call(
        store,
        context,
        api::request::Command::Write(api::Write {
            kind: kind as i32,
            changes: vec![text_field("name", name)],
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    m.targets.into_iter().next().ok_or(Error::Corrupt)
}
fn query() -> api::Query {
    api::Query {
        limit: 20,
        columns: vec!["name".into()],
        ..Default::default()
    }
}

fn unplaced_search_matches(search: &str) -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let device = create(&store, &actor, K::Device, "needle")?;
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let Reply::Mutation(ip) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "address".into(),
            cidr: "192.0.2.5/32".into(),
            address_space: Some(space),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
    )?
    else {
        panic!("allocation");
    };
    let expected = if search == "needle" {
        device
    } else {
        ip.targets[0].clone()
    };
    let before = store.usage()?;
    for unplaced in [false, true] {
        let Reply::Page(page) = call(
            &store,
            &actor,
            Command::Query(api::Query {
                search: search.into(),
                unplaced,
                ..query()
            }),
        )?
        else {
            panic!("page");
        };
        assert_eq!(page.total, 1, "unplaced={unplaced}");
        assert_eq!(page.items[0].target.as_ref(), Some(&expected));
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Adding physical-placement probes must preserve non-level text-search bindings.
#[test]
fn unplaced_search_keeps_text_binding() -> TestResult {
    unplaced_search_matches("needle")
}

// Exact address search must retain its namespace-row binding through placement probes.
#[test]
fn unplaced_search_keeps_ip_binding() -> TestResult {
    unplaced_search_matches("192.0.2.5")
}

// Placement probes must also preserve the namespace lookup used by scoped IPAM.
#[test]
fn unplaced_keeps_network_namespace_binding() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let space = create(&store, &actor, K::AddressSpace, "space")?;
    let Reply::Mutation(ip) = call(
        &store,
        &actor,
        Command::Network(api::Network {
            kind: K::IpAddress as i32,
            name: "address".into(),
            cidr: "192.0.2.5/32".into(),
            address_space: Some(space.clone()),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
    )?
    else {
        panic!("allocation");
    };
    let Reply::Page(page) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            roots: vec![space],
            kinds: vec![K::IpAddress as i32],
            unplaced: true,
            ..query()
        }),
    )?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&ip.targets[0]));
    Ok(())
}

// Physical containment and port ownership exclude children, while roots remain selectable.
#[test]
fn unplaced_excludes_contained_and_port_hardware() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut roots = Vec::new();
    for index in 0..2 {
        let parent = create(&store, &actor, K::Device, &format!("parent-{index:02}"))?;
        roots.push(parent.clone());
        for (kind, relation) in [
            (K::Device, wire::RelationKind::Contains),
            (K::Port, wire::RelationKind::HasPort),
        ] {
            let child = create(&store, &actor, kind, &format!("child-{index:02}-{kind:?}"))?;
            call(
                &store,
                &actor,
                Command::Relation(api::Relationship {
                    relation: Some(wire::Relation {
                        from: Some(parent.clone()),
                        to: Some(child),
                        kind: relation as i32,
                        ..Default::default()
                    }),
                    remove: false,
                }),
            )?;
        }
    }
    let q = api::Query {
        view: api::View::Hardware as i32,
        unplaced: true,
        limit: 100,
        ..query()
    };
    let before = store.usage()?;
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(page.total, roots.len() as u64);
    assert_eq!(page.items.len(), roots.len());
    for item in page.items {
        assert!(roots.contains(&item.target.ok_or("target")?));
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Wide accepted values must page by wire bytes, preserving every requested field and identity.
#[test]
fn wide_pages_continue_within_transport_budget() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut columns = vec!["name".to_owned()];
    for index in 0..11 {
        let Reply::Definitions(definitions) = call(
            &store,
            &actor,
            Command::Definition(api::Definition {
                name: format!("Payload {index}"),
                kinds: vec![K::Device as i32],
                r#type: api::FieldType::Text as i32,
                ..Default::default()
            }),
        )?
        else {
            panic!("definitions");
        };
        columns.push(
            definitions
                .items
                .iter()
                .find(|d| d.name == format!("Payload {index}"))
                .ok_or("definition")?
                .id
                .clone(),
        );
    }
    for index in 0..25 {
        let mut changes = vec![text_field("name", &format!("device-{index:02}"))];
        changes.extend(
            columns[1..]
                .iter()
                .map(|name| text_field(name, &"x".repeat(2048))),
        );
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                kind: K::Device as i32,
                changes,
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    let mut q = api::Query {
        columns: columns.clone(),
        limit: 25,
        include_context: true,
        ..query()
    };
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let Reply::Page(page) = call(&store, &actor, Command::Query(q.clone()))? else {
            panic!("page");
        };
        assert!(
            api::Reply {
                result: Some(Reply::Page(page.clone()))
            }
            .encoded_len()
                <= 512 * 1024
        );
        assert_eq!(page.total, 25);
        assert!(!page.items.is_empty());
        assert_eq!(page.contexts.len(), page.items.len());
        for (item, context) in page.items.iter().zip(&page.contexts) {
            assert_eq!(context.target, item.target);
            assert!(seen.insert(item.target.as_ref().ok_or("target")?.asset_id.clone()));
            assert_eq!(item.fields.len(), columns.len());
            for field in item.fields.iter().filter(|f| f.name != "name") {
                assert_eq!(
                    field.value,
                    Some(wire::field::Value::Text("x".repeat(2048)))
                );
            }
        }
        let Some(next) = page.next else {
            break;
        };
        q.cursor = Some(next);
    }
    assert_eq!(seen.len(), 25);
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Removing an imported asset releases only its mappings; a new import gets a new identity.
#[test]
fn removed_import_mapping_does_not_poison_reimport() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let import = api::Import {
        source: "register".into(),
        kind: K::Device as i32,
        csv: "external_id,name\na,First\nb,Retained".into(),
        mappings: vec![api::Mapping {
            source: "name".into(),
            field: "name".into(),
        }],
        ..Default::default()
    };
    let Reply::Preview(preview) = call(&store, &actor, Command::Import(import.clone()))? else {
        panic!("preview");
    };
    let Reply::Mutation(created) = call(
        &store,
        &actor,
        Command::Import(api::Import {
            preview: preview.id,
            ..Default::default()
        }),
    )?
    else {
        panic!("accept");
    };
    let first = created.targets[0].clone();
    let retained = created.targets[1].clone();
    call(
        &store,
        &actor,
        Command::Bulk(api::Bulk {
            targets: vec![first.clone()],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    let retained_before = store.asset(AssetRef::try_from(&retained)?)?;
    let Reply::Preview(next) = call(&store, &actor, Command::Import(import))? else {
        panic!("new preview");
    };
    assert!(
        next.id.is_some(),
        "stale mapping poisoned preview: {next:?}"
    );
    assert!(next.rows.iter().all(|row| row.change.is_some()));
    let Reply::Mutation(accepted) = call(
        &store,
        &actor,
        Command::Import(api::Import {
            preview: next.id,
            ..Default::default()
        }),
    )?
    else {
        panic!("accept");
    };
    assert_ne!(accepted.targets[0], first);
    assert_eq!(accepted.targets[1], retained);
    assert_eq!(
        store.asset(AssetRef::try_from(&retained)?)?,
        retained_before
    );
    Ok(())
}

// A bounded CSV writer must report admission overflow, not malformed user input.
#[test]
fn csv_export_overflow_reports_limit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let long = "x".repeat(2048);
    for i in 0..128 {
        let target = create(&store, &actor, K::Device, &format!("Export {i}"))?;
        call(
            &store,
            &actor,
            api::request::Command::Write(api::Write {
                target: Some(target),
                expected: Some(1),
                kind: K::Device as i32,
                changes: vec![text_field("description", &long)],
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    let mut q = query();
    q.columns = vec!["name".into(), "description".into()];
    let result = call(
        &store,
        &actor,
        api::request::Command::Export(api::Export {
            query: Some(q),
            spreadsheet: true,
        }),
    );
    assert!(
        matches!(result, Err(Error::Limit)),
        "unexpected export: {result:?}"
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A malformed mapped value identifies its column without retaining a rejected preview.
#[test]
fn csv_errors_identify_the_failing_field_without_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let api::reply::Result::Definitions(created) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Capacity".into(),
            r#type: api::FieldType::Integer as i32,
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition");
    };
    let before = store.usage()?;
    let api::reply::Result::Preview(preview) = call(
        &store,
        &context,
        api::request::Command::Import(api::Import {
            csv: "external_id,name,capacity\none,router,not-a-number\n".into(),
            source: "test".into(),
            kind: K::Device as i32,
            mappings: vec![
                api::Mapping {
                    source: "name".into(),
                    field: "name".into(),
                },
                api::Mapping {
                    source: "capacity".into(),
                    field: created.items[0].id.clone(),
                },
            ],
            ..Default::default()
        }),
    )?
    else {
        panic!("preview");
    };
    assert!(preview.id.is_none());
    assert!(
        preview.rows[0].diagnostic.contains("column 'capacity'"),
        "{}",
        preview.rows[0].diagnostic
    );
    assert!(!preview.rows[0].diagnostic.contains("not-a-number"));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A preview must reject creation modes that require reviewed network allocation.
#[test]
fn csv_preview_does_not_promise_unsupported_network_creation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let api::reply::Result::Preview(preview) = call(
        &store,
        &operator(),
        api::request::Command::Import(api::Import {
            csv: "external_id,name\none,address\n".into(),
            source: "test".into(),
            kind: K::IpAddress as i32,
            mappings: vec![api::Mapping {
                source: "name".into(),
                field: "name".into(),
            }],
            ..Default::default()
        }),
    )?
    else {
        panic!("preview");
    };
    assert!(preview.id.is_none());
    assert!(preview.rows[0].diagnostic.contains("network allocation"));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Readable endpoints do not disclose a relationship whose field is hidden.
#[test]
fn relationship_disclosure_and_scope_require_field_permission() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let host = create(&store, &context, K::Device, "host")?;
    let vm = create(&store, &context, K::VirtualMachine, "guest")?;
    call(
        &store,
        &context,
        api::request::Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(vm),
                to: Some(host.clone()),
                kind: wire::RelationKind::RunsOn as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    let mut restricted = context;
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    let api::reply::Result::Detail(detail) = call(
        &store,
        &restricted,
        api::request::Command::Read(api::Read {
            target: Some(host.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert!(detail.relations.is_empty());
    let mut scoped = query();
    scoped.roots = vec![host];
    let api::reply::Result::Page(page) =
        call(&store, &restricted, api::request::Command::Query(scoped))?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    Ok(())
}

// Retrieval and repeated input must preserve durable usage; stale edits cannot win.
#[test]
fn current_inventory_query_reopen_and_conflict() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let target = create(&store, &context, K::Device, "edge-router")?;
    let before = store.usage()?;
    let api::reply::Result::Page(page) =
        call(&store, &context, api::request::Command::Query(query()))?
    else {
        panic!("page")
    };
    assert_eq!(page.total, 1);
    assert_eq!(
        page.items[0].fields,
        vec![text_field("name", "edge-router")]
    );
    let write = api::Write {
        target: Some(target.clone()),
        expected: Some(1),
        kind: K::Device as i32,
        changes: vec![text_field("name", "edge-router")],
        parent: None,
    };
    call(
        &store,
        &context,
        api::request::Command::Write(write.clone()),
    )?;
    assert_eq!(store.usage()?, before);
    let mut changed = write.clone();
    changed.changes[0] = text_field("name", "edge-renamed");
    call(&store, &context, api::request::Command::Write(changed))?;
    assert!(matches!(
        call(&store, &context, api::request::Command::Write(write)),
        Err(Error::Conflict)
    ));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        store.asset(AssetRef::try_from(&target)?)?.intended[0]
            .field
            .as_ref(),
        Some(&text_field("name", "edge-renamed"))
    );
    Ok(())
}

// Multiple matching fields must neither duplicate resources nor destabilize page ties.
#[test]
fn search_pages_deduplicate_before_limit_and_preserve_order() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let mut expected = Vec::new();
    for _ in 0..5 {
        let target = create(&store, &context, K::Device, "shared-name")?;
        call(
            &store,
            &context,
            api::request::Command::Write(api::Write {
                target: Some(target.clone()),
                expected: Some(1),
                kind: K::Device as i32,
                changes: vec![text_field("description", "shared-description")],
                parent: None,
            }),
        )?;
        expected.push(target.asset_id);
    }
    expected.sort();
    let mut q = query();
    q.search = "shared".into();
    q.limit = 2;
    let mut actual = Vec::new();
    loop {
        let api::reply::Result::Page(page) =
            call(&store, &context, api::request::Command::Query(q.clone()))?
        else {
            panic!("page");
        };
        assert_eq!(page.total, 5);
        assert!(page.items.len() <= 2);
        actual.extend(
            page.items
                .into_iter()
                .map(|item| item.target.expect("identity").asset_id),
        );
        let Some(cursor) = page.next else {
            break;
        };
        q.cursor = Some(cursor);
    }
    assert_eq!(actual, expected);
    q.cursor = None;
    q.predicates.push(api::Predicate {
        field: "description".into(),
        value: Some(text_field("description", "shared-description")),
        definition_generation: None,
    });
    let api::reply::Result::Page(page) = call(&store, &context, api::request::Command::Query(q))?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 5);
    assert_eq!(page.items.len(), 2);
    Ok(())
}

// Scope and field projection must precede search, count and requested-column output.
#[test]
fn scopes_and_search_do_not_disclose_hidden_fields() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    create(&store, &context, K::Device, "hidden-host")?;
    let vm = create(&store, &context, K::VirtualMachine, "visible-vm")?;
    call(
        &store,
        &context,
        api::request::Command::Write(api::Write {
            target: Some(vm),
            expected: Some(1),
            kind: K::VirtualMachine as i32,
            changes: vec![text_field("description", "private-contract")],
            parent: None,
        }),
    )?;
    let mut limited = operator();
    limited.grants = vec![Grant {
        scope: Scope::Kinds(vec![K::VirtualMachine]),
        read: Fields::Selected(vec!["name".into()]),
        write: Fields::Selected(Vec::new()),
        writable: false,
    }];
    let mut q = query();
    q.columns.push("description".into());
    let api::reply::Result::Page(page) =
        call(&store, &limited, api::request::Command::Query(q.clone()))?
    else {
        panic!("page")
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].fields.len(), 1);
    for search in ["hidden-host", "private-contract"] {
        q.search = search.into();
        let api::reply::Result::Page(page) =
            call(&store, &limited, api::request::Command::Query(q.clone()))?
        else {
            panic!("page")
        };
        assert_eq!(page.total, 0);
    }
    Ok(())
}

// A numeric or boolean term may match either text or an explicitly searchable typed value.
#[test]
fn typed_search_branches_do_not_suppress_each_other_with_missing_properties() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let text = create(&store, &context, K::Device, "42")?;
    call(
        &store,
        &context,
        api::request::Command::Write(api::Write {
            target: Some(text),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field("description", "true")],
            parent: None,
        }),
    )?;
    let typed = create(&store, &context, K::Device, "typed")?;
    for (kind, value) in [
        (api::FieldType::Integer, wire::field::Value::Integer(42)),
        (api::FieldType::Boolean, wire::field::Value::Flag(true)),
    ] {
        let api::reply::Result::Definitions(definitions) = call(
            &store,
            &context,
            api::request::Command::Definition(api::Definition {
                name: format!("{kind:?}"),
                kinds: vec![K::Device as i32],
                r#type: kind as i32,
                searchable: true,
                ..Default::default()
            }),
        )?
        else {
            panic!("definition");
        };
        let name = definitions.items[0].id.clone();
        let expected = store.asset(AssetRef::try_from(&typed)?)?.revision;
        call(
            &store,
            &context,
            api::request::Command::Write(api::Write {
                target: Some(typed.clone()),
                expected: Some(expected),
                kind: K::Device as i32,
                changes: vec![wire::Field {
                    name,
                    value: Some(value),
                }],
                parent: None,
            }),
        )?;
    }
    for term in ["42", "true"] {
        let mut q = query();
        q.search = term.into();
        let api::reply::Result::Page(page) =
            call(&store, &context, api::request::Command::Query(q.clone()))?
        else {
            panic!("page");
        };
        assert_eq!(page.total, 2, "term={term}");
        let mut limited = context.clone();
        limited.grants[0].read = Fields::Selected(vec!["name".into()]);
        let api::reply::Result::Page(page) =
            call(&store, &limited, api::request::Command::Query(q))?
        else {
            panic!("page");
        };
        assert_eq!(page.total, if term == "42" { 1 } else { 0 });
    }
    Ok(())
}

// A matching name is not an identity, and an arbitrary caller UUID cannot create one.
#[test]
fn supplied_missing_target_cannot_create_inventory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let forged = asset().target;
    assert!(matches!(
        call(
            &store,
            &operator(),
            api::request::Command::Write(api::Write {
                target: forged,
                expected: None,
                kind: K::Device as i32,
                changes: vec![text_field("name", "forged")],
                parent: None,
            })
        ),
        Err(Error::Missing)
    ));
    Ok(())
}

// Ordinary address duplicates remain refused while identical CIDRs in independent spaces work.
#[test]
fn allocations_scoped_unique_and_point_to_point() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let one = create(&store, &context, K::AddressSpace, "customer-a")?;
    let two = create(&store, &context, K::AddressSpace, "customer-b")?;
    let request = api::Network {
        kind: K::IpAddress as i32,
        address_space: Some(one),
        cidr: "192.0.2.0/31".into(),
        name: "uplink".into(),
        sharing: api::SharedUse::Exclusive as i32,
        ..Default::default()
    };
    call(
        &store,
        &context,
        api::request::Command::Network(request.clone()),
    )?;
    assert!(matches!(
        call(
            &store,
            &context,
            api::request::Command::Network(request.clone())
        ),
        Err(Error::Conflict)
    ));
    let mut second = request.clone();
    second.next_available = true;
    call(
        &store,
        &context,
        api::request::Command::Network(second.clone()),
    )?;
    assert!(matches!(
        call(&store, &context, api::request::Command::Network(second)),
        Err(Error::Limit)
    ));
    let mut independent = request;
    independent.address_space = Some(two);
    call(
        &store,
        &context,
        api::request::Command::Network(independent),
    )?;
    Ok(())
}

// Preview rows must reclaim through the owned bounded lifecycle, never grow indefinitely.
#[test]
fn replacing_import_preview_reclaims_invalidated_rows_within_owner() -> TestResult {
    use omloom_store::Class;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut peer = actor.clone();
    peer.actor = "other importer".into();
    let mut import = api::Import {
        csv: "external_id,name\nedge-1,Draft 00".into(),
        source: "register".into(),
        kind: K::Device as i32,
        mappings: vec![api::Mapping {
            source: "name".into(),
            field: "name".into(),
        }],
        ..Default::default()
    };
    let api::reply::Result::Preview(peer_preview) =
        call(&store, &peer, api::request::Command::Import(import.clone()))?
    else {
        panic!("peer preview")
    };
    let mut prior = None;
    let mut retained_bytes = None;
    for index in 0..8 {
        import.csv = format!("external_id,name\nedge-1,Draft {index:02}");
        let api::reply::Result::Preview(preview) = call(
            &store,
            &actor,
            api::request::Command::Import(import.clone()),
        )?
        else {
            panic!("preview")
        };
        let usage = store.usage()?;
        assert_eq!(
            usage.rows[Class::Review as usize],
            4,
            "only one parent/child pair per importer may remain"
        );
        if let Some(bytes) = retained_bytes {
            assert_eq!(
                usage.bytes[Class::Review as usize],
                bytes,
                "draft revisions must not accumulate payloads"
            );
        }
        retained_bytes = Some(usage.bytes[Class::Review as usize]);
        if let Some(old) = prior {
            let before = store.usage()?;
            assert!(matches!(
                call(
                    &store,
                    &actor,
                    api::request::Command::Import(api::Import {
                        preview: Some(old),
                        ..Default::default()
                    })
                ),
                Err(Error::Expired)
            ));
            assert_eq!(store.usage()?, before);
        }
        prior = preview.id;
    }
    let before = store.usage()?;
    import.csv = "external_id,name\n,invalid identity".into();
    let api::reply::Result::Preview(invalid) =
        call(&store, &actor, api::request::Command::Import(import))?
    else {
        panic!("invalid preview")
    };
    assert!(invalid.id.is_none());
    assert_eq!(
        store.usage()?,
        before,
        "invalid input cannot discard the accepted review draft"
    );
    // The other user's stale preview remains theirs to refresh or expire.
    assert!(matches!(
        call(
            &store,
            &peer,
            api::request::Command::Import(api::Import {
                preview: peer_preview.id,
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    call(
        &store,
        &actor,
        api::request::Command::Import(api::Import {
            preview: prior,
            ..Default::default()
        }),
    )?;
    assert_eq!(store.usage()?.rows[Class::Review as usize], 2);
    let assets = store.assets(None, None, 10)?.assets;
    assert_eq!(assets.len(), 1);
    assert!(
        store
            .asset(assets[0].target)?
            .intended
            .iter()
            .any(|v| v.field.as_ref() == Some(&text_field("name", "Draft 07")))
    );
    Ok(())
}

// A failed replacement after deleting old rows must restore the still-usable original preview.
#[test]
fn import_preview_replacement_rolls_back_when_new_draft_exceeds_budget() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut budget = policy();
    budget.classes[omloom_store::Class::Review as usize].rows = 2;
    let store = Store::open(dir.path(), budget)?;
    let actor = operator();
    let mut import = api::Import {
        csv: "external_id,name\none,Original draft".into(),
        source: "register".into(),
        kind: K::Device as i32,
        mappings: vec![api::Mapping {
            source: "name".into(),
            field: "name".into(),
        }],
        ..Default::default()
    };
    let api::reply::Result::Preview(prior) = call(
        &store,
        &actor,
        api::request::Command::Import(import.clone()),
    )?
    else {
        panic!("preview")
    };
    let before = store.usage()?;
    import.csv = "external_id,name\none,Replacement\ntwo,Too many rows".into();
    assert!(matches!(
        call(&store, &actor, api::request::Command::Import(import)),
        Err(Error::Limit)
    ));
    assert_eq!(
        store.usage()?,
        before,
        "failed replacement must roll back reclamation"
    );
    call(
        &store,
        &actor,
        api::request::Command::Import(api::Import {
            preview: prior.id,
            ..Default::default()
        }),
    )?;
    let assets = store.assets(None, None, 10)?.assets;
    assert_eq!(assets.len(), 1);
    assert!(
        store
            .asset(assets[0].target)?
            .intended
            .iter()
            .any(|v| v.field.as_ref() == Some(&text_field("name", "Original draft")))
    );
    Ok(())
}

// Preview rows must reclaim through the owned bounded lifecycle, never grow indefinitely.
#[test]
fn import_preview_expiry_and_idempotent_acceptance() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let import = api::Import {
        csv: "external_id,name\nedge-1,Imported edge".into(),
        source: "register".into(),
        kind: K::Device as i32,
        mappings: vec![api::Mapping {
            source: "name".into(),
            field: "name".into(),
        }],
        ..Default::default()
    };
    let api::reply::Result::Preview(preview) = call(
        &store,
        &context,
        api::request::Command::Import(import.clone()),
    )?
    else {
        panic!("preview")
    };
    assert_eq!(store.assets(None, None, 10)?.assets.len(), 0);
    let mut after = None;
    loop {
        let report = store.cleanup_reviews(1000, after.as_deref())?;
        after = report.cursor;
        if after.is_none() {
            break;
        }
    }
    assert!(matches!(
        call(
            &store,
            &context,
            api::request::Command::Import(api::Import {
                preview: preview.id,
                ..Default::default()
            })
        ),
        Err(Error::Expired)
    ));
    for _ in 0..2 {
        let api::reply::Result::Preview(preview) = call(
            &store,
            &context,
            api::request::Command::Import(import.clone()),
        )?
        else {
            panic!("preview")
        };
        call(
            &store,
            &context,
            api::request::Command::Import(api::Import {
                preview: preview.id,
                ..Default::default()
            }),
        )?;
    }
    assert_eq!(store.assets(None, None, 10)?.assets.len(), 1);
    assert_eq!(store.assets(None, None, 10)?.assets[0].revision, 1);
    Ok(())
}

// Definition migration must invalidate stale asset edits while preserving typed false/clear.
#[test]
fn typed_definition_migration_updates_asset_revision() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let target = create(&store, &context, K::Device, "migration-host")?;
    let api::reply::Result::Definitions(definitions) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Allocation".into(),
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition")
    };
    let mut d = definitions.items[0].clone();
    call(
        &store,
        &context,
        api::request::Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(&d.id, "42")],
            parent: None,
        }),
    )?;
    d.r#type = api::FieldType::Integer as i32;
    call(
        &store,
        &context,
        api::request::Command::Migration(api::Migration {
            definition: Some(d),
            expected: store.usage()?.revision,
            apply: true,
            convert_types: true,
            conversions: Vec::new(),
            remove_inapplicable: false,
        }),
    )?;
    assert_eq!(store.asset(AssetRef::try_from(&target)?)?.revision, 3);
    Ok(())
}

// Exact identity search does not depend on a name value; ordinary removals retain replay guards.
#[test]
fn exact_identity_search_and_pending_removal_guard() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let original = asset();
    let target = target(&original);
    store.save_asset(
        &admit(omloom_contracts::wire::record::Body::Asset(original)),
        None,
        "operator",
        1,
    )?;
    let mut q = query();
    q.search = target.asset.to_string();
    let api::reply::Result::Page(page) = call(&store, &context, api::request::Command::Query(q))?
    else {
        panic!("page")
    };
    assert_eq!(page.total, 1);
    let arguments = vec![text_field("power", "on")];
    store.accept(&plan(target, &arguments), 10)?;
    assert!(matches!(
        call(
            &store,
            &context,
            api::request::Command::Bulk(api::Bulk {
                targets: vec![wire::AssetRef {
                    asset_id: target.asset.to_string(),
                    incarnation: target.incarnation.to_string()
                }],
                expected: store.usage()?.revision,
                remove: true,
                changes: Vec::new()
            })
        ),
        Err(Error::UnsafeTransition)
    ));
    Ok(())
}

// Link-local host identities include their link, while ordinary hosts cannot bypass a space via VLAN context.
#[test]
fn link_local_scopes_and_shared_conversion() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let space = create(&store, &context, K::AddressSpace, "space")?;
    let a = create(&store, &context, K::L2Domain, "lan-a")?;
    let b = create(&store, &context, K::L2Domain, "lan-b")?;
    let iface = create(&store, &context, K::Interface, "owner-interface")?;
    let other = create(&store, &context, K::Interface, "backup-interface")?;
    let request = api::Network {
        kind: K::IpAddress as i32,
        address_space: Some(space),
        link: Some(a),
        cidr: "fe80::1/64".into(),
        name: "gateway".into(),
        sharing: api::SharedUse::Exclusive as i32,
        ..Default::default()
    };
    let api::reply::Result::Mutation(created) = call(
        &store,
        &context,
        api::request::Command::Network(request.clone()),
    )?
    else {
        panic!("allocation")
    };
    let target = created.targets[0].clone();
    let mut second_link = request.clone();
    second_link.link = Some(b);
    call(
        &store,
        &context,
        api::request::Command::Network(second_link),
    )?;
    let revision = store.asset(AssetRef::try_from(&target)?)?.revision;
    call(
        &store,
        &context,
        api::request::Command::Member(api::Member {
            address: Some(target.clone()),
            interface: Some(iface.clone()),
            expected: revision,
            ..Default::default()
        }),
    )?;
    let revision = store.asset(AssetRef::try_from(&target)?)?.revision;
    assert!(matches!(
        call(
            &store,
            &context,
            api::request::Command::Member(api::Member {
                address: Some(target.clone()),
                interface: Some(other.clone()),
                expected: revision,
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    let mut conversion = request;
    conversion.target = Some(target.clone());
    conversion.expected = Some(revision);
    conversion.sharing = api::SharedUse::Vrrp as i32;
    conversion.vrid = Some(7);
    conversion.group_name = "reviewed gateway group".into();
    call(&store, &context, api::request::Command::Network(conversion))?;
    let revision = store.asset(AssetRef::try_from(&target)?)?.revision;
    let api::reply::Result::Detail(group_detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("group");
    };
    call(
        &store,
        &context,
        api::request::Command::Member(api::Member {
            address: Some(target.clone()),
            interface: Some(other),
            expected: revision,
            priority: Some(100),
            expected_group: Some(group_detail.group.expect("reviewed group").revision),
            ..Default::default()
        }),
    )?;
    let api::reply::Result::Detail(detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert!(detail.participants_complete);
    assert_eq!(detail.participants.len(), 2);
    assert!(
        detail
            .participants
            .iter()
            .any(|p| p.interface.as_ref() == Some(&iface) && p.priority == Some(255))
    );
    let mut limited = operator();
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&target)?]);
    let api::reply::Result::Detail(detail) = call(
        &store,
        &limited,
        api::request::Command::Read(api::Read {
            target: Some(target),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail")
    };
    assert!(detail.participants.is_empty());
    assert!(!detail.participants_complete);
    Ok(())
}

// Browser acceptance changes only intent, never provider evidence or unrelated values.
#[test]
fn editing_preserves_partial_source_evidence_without_writes() -> TestResult {
    use omloom_contracts::{ids::SourceId, wire::record};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let source = SourceId::generate().to_string();
    let mut original = asset();
    let target = target(&original);
    original.observed = vec![observation(
        &source,
        50,
        vec![text_field("description", "source value")],
    )];
    store.save_asset(&admit(record::Body::Asset(original)), None, "operator", 50)?;
    let evidence_key = format!("asset/{}/observed/{source}/description", target.asset);
    let before = row_version(&store, &evidence_key)?;
    call(
        &store,
        &operator(),
        api::request::Command::Write(api::Write {
            target: Some(wire::AssetRef {
                asset_id: target.asset.to_string(),
                incarnation: target.incarnation.to_string(),
            }),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field("name", "accepted name")],
            parent: None,
        }),
    )?;
    assert_eq!(row_version(&store, &evidence_key)?, before);
    let current = store.asset(target)?;
    assert_eq!(
        current.observed[0]
            .evidence
            .as_ref()
            .expect("evidence")
            .fields,
        vec![text_field("description", "source value")]
    );
    let before = store.usage()?;
    for _ in 0..3 {
        call(
            &store,
            &operator(),
            api::request::Command::Read(api::Read {
                target: current.target.clone(),
                ..Default::default()
            }),
        )?;
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Removal has its own revision and cannot overwrite the last accepted edit's actor or time.
#[test]
fn removal_preserves_the_last_edit_audit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut actor = operator();
    let target = create(&store, &actor, K::Device, "audit resource")?;
    actor.actor = "editor".into();
    call(
        &store,
        &actor,
        api::request::Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field("description", "accepted edit")],
            ..Default::default()
        }),
    )?;
    let owner = format!("asset/{}", target.asset_id);
    let before = store.activity(&owner, 10)?;
    actor.actor = "remover".into();
    call(
        &store,
        &actor,
        api::request::Command::Bulk(api::Bulk {
            targets: vec![target],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    let after = store.activity(&owner, 10)?;
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(&after[1..], before.as_slice());
    assert_eq!(after[0].action, "inventory.removed");
    assert_eq!(after[0].actor, "remover");
    assert_eq!(after[0].revision, before[0].revision + 1);
    Ok(())
}

// Removal cannot allow an old provider record to resurrect the removed identity.
#[test]
fn removal_blocks_replay_and_releases_only_its_allocation() -> TestResult {
    use omloom_contracts::wire::record;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let original = asset();
    let target = target(&original);
    let record = admit(record::Body::Asset(original));
    store.save_asset(&record, None, "operator", 1)?;
    call(
        &store,
        &operator(),
        api::request::Command::Bulk(api::Bulk {
            targets: vec![wire::AssetRef {
                asset_id: target.asset.to_string(),
                incarnation: target.incarnation.to_string(),
            }],
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    assert!(matches!(
        store.save_asset(&record, None, "provider", 200),
        Err(Error::Conflict)
    ));
    Ok(())
}

// A prepared store pays mandatory index recovery guards at startup, not on a user's first search.
#[test]
fn cold_search_does_not_write_after_store_readiness() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    create(&store, &operator(), K::Device, "cold-router")?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    store.diagnostic_database().persist()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    let mut taps = Vec::new();
    for partition in coordinode_storage::engine::partition::Partition::all() {
        let (tap, _) = store
            .diagnostic_database()
            .engine()
            .tap_writes(*partition)?;
        taps.push(tap);
    }
    let before = store.usage()?;
    let mut q = query();
    q.search = "cold".into();
    call(&store, &operator(), api::request::Command::Query(q))?;
    for tap in taps {
        assert_eq!(
            tap.take(),
            coordinode_storage::engine::tap::Tapped::Keys(Vec::new()),
            "First search wrote {:?}",
            tap.partition()
        );
    }
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Archival must not strand relations whose archived endpoints can no longer be edited.
#[test]
fn archival_rejects_attached_relations_atomically() -> TestResult {
    use api::{reply::Result as Reply, request::Command};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let from = create(&store, &actor, K::Service, "Application")?;
    let to = create(&store, &actor, K::Service, "Database")?;
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(from.clone()),
                to: Some(to.clone()),
                kind: wire::RelationKind::DependsOn as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    let before = store.usage()?;
    for reference in [&from, &to] {
        let target = AssetRef::try_from(reference)?;
        let original = store.asset(target)?;
        let mut archived = original.clone();
        archived.adoption = wire::Adoption::Archived as i32;
        assert!(matches!(
            store.save_asset(
                &admit(wire::record::Body::Asset(archived)),
                Some(original.revision),
                "provider",
                200
            ),
            Err(Error::Conflict)
        ));
        assert_eq!(
            store.usage()?,
            before,
            "refusal must not write inventory, evidence or audit"
        );
        assert_eq!(store.asset(target)?, original);
    }
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let Reply::Detail(detail) = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(from.clone()),
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    assert_eq!(detail.relations.len(), 1);
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(detail.relations[0].clone()),
            remove: true,
        }),
    )?;
    for reference in [&from, &to] {
        let target = AssetRef::try_from(reference)?;
        let mut archived = store.asset(target)?;
        let revision = archived.revision;
        archived.adoption = wire::Adoption::Archived as i32;
        store.save_asset(
            &admit(wire::record::Body::Asset(archived)),
            Some(revision),
            "provider",
            201,
        )?;
        assert_eq!(
            store.asset(target)?.adoption,
            wire::Adoption::Archived as i32
        );
    }
    Ok(())
}

// An archived identity cannot be revived by an inventory edit.
#[test]
fn archived_identity_cannot_be_edited_back_to_live() -> TestResult {
    use omloom_contracts::wire::{Adoption, record};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let original = asset();
    let target = target(&original);
    store.save_asset(
        &admit(record::Body::Asset(original.clone())),
        None,
        "operator",
        1,
    )?;
    let mut archived = store.asset(target)?;
    archived.adoption = Adoption::Archived as i32;
    store.save_asset(
        &admit(record::Body::Asset(archived)),
        Some(1),
        "operator",
        2,
    )?;
    assert!(matches!(
        call(
            &store,
            &operator(),
            api::request::Command::Write(api::Write {
                target: original.target,
                expected: Some(2),
                kind: K::Device as i32,
                changes: vec![text_field("name", "revived")],
                parent: None,
            })
        ),
        Err(Error::Conflict)
    ));
    Ok(())
}

// One accepted physical parent and acyclic containment are transactional inventory invariants.
#[test]
fn physical_placement_is_exclusive_and_acyclic() -> TestResult {
    use omloom_contracts::wire::RelationKind as R;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let device = create(&store, &context, K::Device, "device")?;
    let rack_a = create(&store, &context, K::Rack, "rack-a")?;
    let rack_b = create(&store, &context, K::Rack, "rack-b")?;
    let relation = |from, to, kind| {
        api::request::Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(from),
                to: Some(to),
                kind: kind as i32,
                ..Default::default()
            }),
            remove: false,
        })
    };
    call(
        &store,
        &context,
        relation(device.clone(), rack_a, R::InstalledIn),
    )?;
    assert!(matches!(
        call(&store, &context, relation(device, rack_b, R::InstalledIn)),
        Err(Error::Conflict)
    ));
    let a = create(&store, &context, K::Location, "room-a")?;
    let b = create(&store, &context, K::Location, "room-b")?;
    call(
        &store,
        &context,
        relation(a.clone(), b.clone(), R::InstalledIn),
    )?;
    assert!(matches!(
        call(&store, &context, relation(b, a, R::InstalledIn)),
        Err(Error::Invalid)
    ));
    Ok(())
}
