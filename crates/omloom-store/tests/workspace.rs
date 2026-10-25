mod support;
use omloom_contracts::{
    ids::AssetRef,
    wire::{self, AssetKind as K},
    workspace as api,
};
use omloom_store::{Error, Store, workspace::*};
use support::*;
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
                changes: vec![text_field("name", "forged")]
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
    call(&store, &context, api::request::Command::Network(conversion))?;
    let revision = store.asset(AssetRef::try_from(&target)?)?.revision;
    call(
        &store,
        &context,
        api::request::Command::Member(api::Member {
            address: Some(target.clone()),
            interface: Some(other),
            expected: revision,
            priority: Some(100),
            ..Default::default()
        }),
    )?;
    let api::reply::Result::Detail(detail) = call(
        &store,
        &context,
        api::request::Command::Read(api::Read {
            target: Some(target.clone()),
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
            }),
        )?;
    }
    assert_eq!(store.usage()?, before);
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
