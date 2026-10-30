use super::*;
use api::{reply::Result as Reply, request::Command};
use omloom_store::Class;
#[path = "correction/continuity.rs"]
mod continuity;
#[path = "correction/safety.rs"]
mod safety;

fn child(
    store: &Store,
    parent: &wire::AssetRef,
    kind: K,
    name: &str,
) -> Result<wire::AssetRef, Error> {
    let value = store.workspace(
        &operator(),
        api::Request {
            working: Some(api::WorkingContext {
                focus: Some(parent.clone()),
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
            command: Some(Command::Write(api::Write {
                kind: kind as i32,
                parent: Some(parent.clone()),
                changes: vec![text_field("name", name)],
                ..Default::default()
            })),
        },
        100,
    )?;
    let Some(Reply::Mutation(value)) = value.result else {
        return Err(Error::Corrupt);
    };
    value.targets.into_iter().next().ok_or(Error::Corrupt)
}
fn mapping(from: &wire::AssetRef, to: &wire::AssetRef) -> api::IdentityMapping {
    api::IdentityMapping {
        duplicate: Some(from.clone()),
        retained: Some(to.clone()),
        fields: vec![api::IdentityFieldChoice {
            name: "name".into(),
            use_duplicate: false,
        }],
        ..Default::default()
    }
}
fn prepare(
    store: &Store,
    mappings: Vec<api::IdentityMapping>,
    decision: api::IdentityDecision,
) -> Result<api::IdentityCorrectionView, Error> {
    let Some(Reply::IdentityCorrection(value)) = store
        .workspace(
            &operator(),
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                    decision: decision as i32,
                    mappings,
                    ..Default::default()
                })),
            },
            100,
        )?
        .result
    else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}
fn apply(store: &Store, id: &str) -> Result<api::IdentityCorrectionApplied, Error> {
    let Reply::IdentityCorrected(value) = call(
        store,
        &operator(),
        Command::CorrectIdentity(api::IdentityCorrection {
            preview: Some(id.into()),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}
fn read(store: &Store, target: &wire::AssetRef) -> Result<api::Detail, Error> {
    let Reply::Detail(value) = call(
        store,
        &operator(),
        Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(*value)
}

// Reversal must restore adjacency as well as the row of a redundant relation.
#[test]
fn undo_restores_deleted_relation_adjacency() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let retained = create(&store, &operator(), K::Device, "Retained")?;
    let duplicate = create(&store, &operator(), K::Device, "Duplicate")?;
    let retained_os = child(&store, &retained, K::SoftwareInstallation, "Retained OS")?;
    let os = child(&store, &duplicate, K::SoftwareInstallation, "OS")?;
    let plan = prepare(
        &store,
        vec![mapping(&duplicate, &retained), mapping(&os, &retained_os)],
        api::IdentityDecision::DuplicateInstallation,
    )?;
    let relation_id = read(&store, &os)?
        .relations
        .into_iter()
        .find(|r| r.from.as_ref() == Some(&duplicate))
        .ok_or("relation before correction")?
        .relation_id;
    let applied = apply(&store, &plan.id)?;
    call(
        &store,
        &operator(),
        Command::CorrectIdentity(api::IdentityCorrection {
            undo: Some(applied.id),
            ..Default::default()
        }),
    )?;
    assert!(
        read(&store, &duplicate)?
            .relations
            .iter()
            .any(|r| r.relation_id == relation_id
                && r.from.as_ref() == Some(&duplicate)
                && r.to.as_ref() == Some(&os))
    );
    assert!(
        read(&store, &os)?
            .relations
            .iter()
            .any(|r| r.relation_id == relation_id
                && r.from.as_ref() == Some(&duplicate)
                && r.to.as_ref() == Some(&os))
    );
    Ok(())
}

// Membership is evaluated on an intact graph, including a child being moved.
#[test]
fn scoped_correction_keeps_original_child_membership() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let retained = create(&store, &operator(), K::Device, "Retained")?;
    let duplicate = create(&store, &operator(), K::Device, "Duplicate")?;
    let os = child(&store, &duplicate, K::SoftwareInstallation, "OS")?;
    let mut working = api::WorkingContext {
        roots: vec![retained.clone(), duplicate.clone()],
        expected: Some(store.usage()?.revision),
        ..Default::default()
    };
    let Some(Reply::IdentityCorrection(preview)) = store
        .workspace(
            &operator(),
            api::Request {
                working: Some(working.clone()),
                command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                    decision: api::IdentityDecision::DistinctInstallations as i32,
                    mappings: vec![mapping(&duplicate, &retained)],
                    ..Default::default()
                })),
            },
            100,
        )?
        .result
    else {
        return Err("preview".into());
    };
    working.expected = Some(preview.revision);
    store.workspace(
        &operator(),
        api::Request {
            working: Some(working),
            command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                preview: Some(preview.id),
                ..Default::default()
            })),
        },
        100,
    )?;
    assert!(
        read(&store, &os)?
            .relations
            .iter()
            .any(|r| r.from.as_ref() == Some(&retained))
    );
    Ok(())
}

// One chassis can keep both OS installations, their services and their own
// interface addresses. Explicit reversal restores only the correction delta.
#[test]
fn dual_boot_correction_is_reviewed_atomic_reopenable_and_reversible() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let retained = create(&store, &operator(), K::Device, "Retained chassis")?;
    let duplicate = create(&store, &operator(), K::Device, "Duplicate chassis")?;
    let linux = child(&store, &retained, K::SoftwareInstallation, "Linux")?;
    let windows = child(&store, &duplicate, K::SoftwareInstallation, "Windows")?;
    let service = child(&store, &windows, K::Service, "SMTP")?;
    let before = store.usage()?;
    let plan = prepare(
        &store,
        vec![mapping(&duplicate, &retained)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    assert_eq!(plan.relations.len(), 1);
    assert!(
        read(&store, &duplicate).is_ok(),
        "preview does not retire a subject"
    );
    assert_eq!(
        store.usage()?.rows[Class::Inventory as usize],
        before.rows[Class::Inventory as usize]
    );
    let result = apply(&store, &plan.id)?;
    let before_retry = store.usage()?;
    assert_eq!(
        apply(&store, &plan.id)?,
        result,
        "lost-response retry returns the same accepted receipt"
    );
    assert_eq!(store.usage()?, before_retry);
    assert_eq!(result.retained, vec![retained.clone()]);
    assert!(matches!(read(&store, &duplicate), Err(Error::Missing)));
    assert!(read(&store, &linux).is_ok());
    assert!(read(&store, &windows).is_ok());
    assert!(read(&store, &service).is_ok());
    let relation = read(&store, &windows)?
        .relations
        .into_iter()
        .find(|r| r.kind == wire::RelationKind::Contains as i32 && r.to.as_ref() == Some(&windows))
        .ok_or("OS placement")?;
    assert_eq!(relation.from.as_ref(), Some(&retained));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let Some(Reply::IdentityCorrected(inverse)) = store
        .workspace(
            &operator(),
            api::Request {
                working: Some(api::WorkingContext {
                    roots: vec![retained.clone(), duplicate.clone()],
                    ..Default::default()
                }),
                command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                    undo: Some(result.id.clone()),
                    ..Default::default()
                })),
            },
            100,
        )?
        .result
    else {
        return Err("inverse".into());
    };
    assert_eq!(inverse.undo_until, 0);
    let before_retry = store.usage()?;
    let repeated = call(
        &store,
        &operator(),
        Command::CorrectIdentity(api::IdentityCorrection {
            undo: Some(result.id),
            ..Default::default()
        }),
    )?;
    assert_eq!(repeated, Reply::IdentityCorrected(inverse.clone()));
    assert_eq!(store.usage()?, before_retry);
    assert!(read(&store, &duplicate)?.item.as_ref().unwrap().revision > 1);
    let relation = read(&store, &windows)?
        .relations
        .into_iter()
        .find(|r| r.kind == wire::RelationKind::Contains as i32 && r.to.as_ref() == Some(&windows))
        .ok_or("restored placement")?;
    assert_eq!(relation.from.as_ref(), Some(&duplicate));
    assert!(read(&store, &service).is_ok());
    Ok(())
}

// Exact lookup exposes only permitted candidates. A reviewed three-layer
// merge preserves independent sources and makes old reports repeat-safe.
#[test]
fn duplicate_installation_reconciles_sources_without_resurrection_or_grant_union() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut first = super::host_import::draft();
    first.report.as_mut().unwrap().findings[0]
        .identity
        .push(api::HostIdentityClaim {
            kind: api::HostIdentityKind::HardwareUuid as i32,
            namespace: "smbios/system-uuid".into(),
            digest: "a".repeat(64),
            quality: api::HostIdentityQuality::Reported as i32,
        });
    let mut second = first.clone();
    second.report.as_mut().unwrap().source_basis = "d".repeat(64);
    second.report.as_mut().unwrap().findings[2].addresses[0].address = "192.0.2.19".into();
    let a =
        super::host_import::accept(&store, &super::host_import::preview(&store, first)?)?.targets;
    let b = super::host_import::accept(
        &store,
        &super::host_import::preview(&store, second.clone())?,
    )?
    .targets;
    let before = store.usage()?;
    let Reply::IdentityCandidates(candidates) = call(
        &store,
        &operator(),
        Command::FindIdentityCandidates(api::FindIdentityCandidates {
            target: Some(a[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("candidates".into());
    };
    assert_eq!(candidates.items.len(), 1);
    assert_eq!(candidates.items[0].target.as_ref(), Some(&b[0]));
    assert_eq!(store.usage()?, before);
    let mut scoped = operator();
    scoped.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&a[0])?]);
    let Reply::IdentityCandidates(hidden) = call(
        &store,
        &scoped,
        Command::FindIdentityCandidates(api::FindIdentityCandidates {
            target: Some(a[0].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("hidden candidates".into());
    };
    assert!(hidden.items.is_empty());
    let mappings = vec![
        mapping(&b[0], &a[0]),
        mapping(&b[1], &a[1]),
        mapping(&b[2], &a[2]),
    ];
    let plan = prepare(
        &store,
        mappings.clone(),
        api::IdentityDecision::DuplicateInstallation,
    )?;
    let before = store.usage()?;
    assert_eq!(
        prepare(
            &store,
            mappings,
            api::IdentityDecision::DuplicateInstallation
        )?
        .id,
        plan.id
    );
    assert_eq!(
        store.usage()?,
        before,
        "an unchanged preview writes nothing"
    );
    apply(&store, &plan.id)?;
    let addresses = read(&store, &a[2])?.current_addresses;
    assert_eq!(addresses.len(), 2);
    assert!(
        addresses
            .iter()
            .all(|v| v.coverage.as_ref().unwrap().source == "Native fixture")
    );
    assert_ne!(
        addresses[0].source_id, addresses[1].source_id,
        "display labels do not identify collectors"
    );
    assert!(
        addresses
            .iter()
            .any(|v| v.observations[0].value.as_ref().unwrap().address == "192.0.2.19")
    );
    // Undo must not restore source evidence after that field right is revoked.
    let mut restricted = operator();
    restricted.grants[0].write = Fields::Selected(vec![
        "name".into(),
        "identity_correction".into(),
        "relations".into(),
        "remove".into(),
        "identity_evidence".into(),
        "claimed_owner".into(),
        "operator".into(),
    ]);
    let unchanged = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &restricted,
            Command::CorrectIdentity(api::IdentityCorrection {
                undo: Some(plan.id.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, unchanged);
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &operator(), Command::HostImport(second))?,
        Reply::Mutation(_)
    ));
    assert_eq!(
        store.usage()?,
        before,
        "old reviewed source mappings resolve to retained identities without re-adoption"
    );
    let Reply::ResolvedIdentity(alias) = call(
        &store,
        &operator(),
        Command::ResolveIdentity(api::ResolveIdentity {
            original: Some(b[0].clone()),
        }),
    )?
    else {
        return Err("alias".into());
    };
    assert_eq!(alias.original.as_ref(), Some(&b[0]));
    assert_eq!(alias.canonical.unwrap().target.as_ref(), Some(&a[0]));
    let mut retired_only = operator();
    retired_only.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&b[0])?]);
    assert!(matches!(
        call(
            &store,
            &retired_only,
            Command::ResolveIdentity(api::ResolveIdentity {
                original: Some(b[0].clone())
            })
        ),
        Err(Error::Denied)
    ));
    let mut canonical_only = operator();
    canonical_only.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&a[0])?]);
    assert!(matches!(
        call(
            &store,
            &canonical_only,
            Command::ResolveIdentity(api::ResolveIdentity {
                original: Some(b[0].clone())
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(
        store.usage()?,
        before,
        "reading aliases does not write or grant access"
    );
    Ok(())
}

// Stale preview/undo and revoked field rights must fail before any domain
// change. No inverse may overwrite edits accepted after the correction.
#[test]
fn identity_correction_rechecks_revision_fields_and_inverse_dependencies() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let a = create(&store, &operator(), K::Device, "A")?;
    let b = create(&store, &operator(), K::Device, "B")?;
    let plan = prepare(
        &store,
        vec![mapping(&a, &b)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    let unchanged = store.usage()?;
    assert!(matches!(
        store.workspace(
            &operator(),
            api::Request {
                working: Some(api::WorkingContext {
                    roots: vec![a.clone(), b.clone()],
                    ..Default::default()
                }),
                command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                    preview: Some(plan.id.clone()),
                    ..Default::default()
                }))
            },
            100
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.usage()?,
        unchanged,
        "a preview is tied to the reviewed working context"
    );
    let mut denied = operator();
    denied.grants[0].write = Fields::Selected(vec![
        "relations".into(),
        "remove".into(),
        "identity_correction".into(),
    ]);
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &denied,
            Command::CorrectIdentity(api::IdentityCorrection {
                preview: Some(plan.id.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    let result = apply(&store, &plan.id)?;
    create(&store, &operator(), K::Device, "Concurrent edit")?;
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::CorrectIdentity(api::IdentityCorrection {
                undo: Some(result.id),
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert!(matches!(read(&store, &a), Err(Error::Missing)));
    Ok(())
}
