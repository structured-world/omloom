use super::*;

// Final duplicate placements must reconcile identically in either relation-ID order.
#[test]
fn correction_deduplicates_vm_placements_before_exclusivity_checks() -> TestResult {
    use super::super::vm_placement::hypervisor;
    for duplicate_first in [true, false] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let a = create(&store, &operator(), K::VirtualMachine, "Duplicate VM")?;
        let b = create(&store, &operator(), K::VirtualMachine, "Retained VM")?;
        let h1 = hypervisor(&store, "Original hypervisor")?;
        let h2 = hypervisor(&store, "Retained hypervisor")?;
        for vm in if duplicate_first { [&a, &b] } else { [&b, &a] } {
            call(
                &store,
                &operator(),
                Command::Relation(api::Relationship {
                    relation: Some(wire::Relation {
                        from: Some(vm.clone()),
                        to: Some(h1.clone()),
                        kind: wire::RelationKind::RunsOn as i32,
                        ..Default::default()
                    }),
                    remove: false,
                }),
            )?;
        }
        let mut host = mapping(&h1, &h2);
        host.fields.push(api::IdentityFieldChoice {
            name: "virtualization_role".into(),
            use_duplicate: false,
        });
        let plan = prepare(
            &store,
            vec![mapping(&a, &b), host],
            api::IdentityDecision::DuplicateInstallation,
        )?;
        let applied = apply(&store, &plan.id)?;
        let placements = read(&store, &b)?
            .relations
            .into_iter()
            .filter(|r| r.kind == wire::RelationKind::RunsOn as i32)
            .collect::<Vec<_>>();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].to.as_ref(), Some(&h2));
        call(
            &store,
            &operator(),
            Command::CorrectIdentity(api::IdentityCorrection {
                undo: Some(applied.id),
                ..Default::default()
            }),
        )?;
        for vm in [&a, &b] {
            let placements = read(&store, vm)?
                .relations
                .into_iter()
                .filter(|r| r.kind == wire::RelationKind::RunsOn as i32)
                .collect::<Vec<_>>();
            assert_eq!(placements.len(), 1);
            assert_eq!(placements[0].to.as_ref(), Some(&h1));
        }
    }
    Ok(())
}

// Keeping a retained field must not require permission to rewrite that unchanged field on undo.
#[test]
fn correction_undo_keeps_retained_fields_without_write_permission() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let retained = create(&store, &operator(), K::Device, "Retained")?;
    let duplicate = create(&store, &operator(), K::Device, "Duplicate")?;
    let mut actor = operator();
    actor.grants[0].write = Fields::Selected(vec![
        "identity_correction".into(),
        "relations".into(),
        "remove".into(),
    ]);
    actor.grants.push(Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&duplicate)?]),
        read: Fields::All,
        write: Fields::All,
        writable: true,
    });
    let correct = |command| -> Result<Reply, Error> {
        store
            .workspace(
                &actor,
                api::Request {
                    working: Some(api::WorkingContext {
                        expected: Some(store.usage()?.revision),
                        ..Default::default()
                    }),
                    command: Some(command),
                },
                100,
            )?
            .result
            .ok_or(Error::Corrupt)
    };
    let Reply::IdentityCorrection(plan) =
        correct(Command::CorrectIdentity(api::IdentityCorrection {
            decision: api::IdentityDecision::DuplicateInstallation as i32,
            mappings: vec![mapping(&duplicate, &retained)],
            ..Default::default()
        }))?
    else {
        return Err("preview".into());
    };
    let Reply::IdentityCorrected(applied) =
        correct(Command::CorrectIdentity(api::IdentityCorrection {
            preview: Some(plan.id),
            ..Default::default()
        }))?
    else {
        return Err("correction".into());
    };
    correct(Command::CorrectIdentity(api::IdentityCorrection {
        undo: Some(applied.id),
        ..Default::default()
    }))?;
    assert_eq!(
        read(&store, &retained)?
            .item
            .ok_or("retained")?
            .fields
            .into_iter()
            .find(|f| f.name == "name")
            .ok_or("retained name")?
            .value,
        Some(wire::field::Value::Text("Retained".into()))
    );
    assert_eq!(
        read(&store, &duplicate)?
            .item
            .ok_or("duplicate")?
            .fields
            .into_iter()
            .find(|f| f.name == "name")
            .ok_or("duplicate name")?
            .value,
        Some(wire::field::Value::Text("Duplicate".into()))
    );
    Ok(())
}

// Correcting a migrated host cannot reactivate inheritance tied to the old placement.
#[test]
fn correction_and_undo_keep_stale_placement_inheritance_unavailable() -> TestResult {
    use super::super::vm_placement::{hypervisor, responsibility};
    use api::{AssignmentMode as M, ResponsibilityFacet as F, ResponsibilityStatus as S};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "Original host")?;
    let duplicate = hypervisor(&store, "Migrated host")?;
    let retained = hypervisor(&store, "Retained host")?;
    responsibility(
        &store,
        &first,
        vec![api::ResponsibilityAssignment {
            facet: F::ClaimedOwner as i32,
            mode: M::Explicit as i32,
            party: Some(api::PartyRef {
                issuer: "company.example".into(),
                subject: "original".into(),
                name: "Original team".into(),
                kind: api::PartyKind::Team as i32,
            }),
            ..Default::default()
        }],
    )?;
    responsibility(
        &store,
        &retained,
        vec![api::ResponsibilityAssignment {
            facet: F::ClaimedOwner as i32,
            mode: M::Explicit as i32,
            party: Some(api::PartyRef {
                issuer: "company.example".into(),
                subject: "operations".into(),
                name: "New team".into(),
                kind: api::PartyKind::Team as i32,
            }),
            ..Default::default()
        }],
    )?;
    call(
        &store,
        &operator(),
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(guest.clone()),
                to: Some(first),
                kind: wire::RelationKind::RunsOn as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    let mut placement = read(&store, &guest)?
        .relations
        .into_iter()
        .find(|r| r.kind == wire::RelationKind::RunsOn as i32)
        .ok_or("placement")?;
    responsibility(
        &store,
        &guest,
        vec![api::ResponsibilityAssignment {
            facet: F::ClaimedOwner as i32,
            mode: M::Inherit as i32,
            source_relation: Some(placement.relation_id.clone()),
            source_revision: Some(placement.revision),
            ..Default::default()
        }],
    )?;
    let inherited = responsibility(&store, &guest, Vec::new())?;
    assert_eq!(
        inherited
            .effective
            .iter()
            .find(|v| v.facet == F::ClaimedOwner as i32)
            .and_then(|v| v.party.as_ref())
            .ok_or("inherited owner before migration")?
            .subject,
        "original"
    );
    placement.to = Some(duplicate.clone());
    call(
        &store,
        &operator(),
        Command::Relation(api::Relationship {
            relation: Some(placement),
            remove: false,
        }),
    )?;
    let unavailable = |store: &Store| -> TestResult {
        let state = responsibility(store, &guest, Vec::new())?;
        let owner = state
            .effective
            .iter()
            .find(|v| v.facet == F::ClaimedOwner as i32)
            .ok_or("owner")?;
        assert_eq!(owner.status, S::SourceUnavailable as i32);
        assert!(owner.party.is_none());
        Ok(())
    };
    unavailable(&store)?;
    let mut choice = mapping(&duplicate, &retained);
    choice.fields.push(api::IdentityFieldChoice {
        name: "virtualization_role".into(),
        use_duplicate: false,
    });
    let preview = prepare(
        &store,
        vec![choice],
        api::IdentityDecision::DuplicateInstallation,
    )?;
    let applied = apply(&store, &preview.id)?;
    unavailable(&store)?;
    call(
        &store,
        &operator(),
        Command::CorrectIdentity(api::IdentityCorrection {
            undo: Some(applied.id),
            ..Default::default()
        }),
    )?;
    unavailable(&store)?;
    Ok(())
}

// Blind write permission cannot disclose either a mapped root's or its child's relations.
#[test]
fn correction_requires_relation_read_on_all_endpoints() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let retained = create(&store, &operator(), K::Device, "Retained")?;
    let duplicate = create(&store, &operator(), K::Device, "Duplicate")?;
    let os = child(&store, &duplicate, K::SoftwareInstallation, "OS")?;
    for permitted in [
        Vec::new(),
        vec![retained.clone(), duplicate.clone()],
        vec![duplicate.clone(), os.clone()],
    ] {
        let mut actor = operator();
        actor.grants[0].read = Fields::Selected(vec!["name".into(), "identity_evidence".into()]);
        actor.grants.push(Grant {
            scope: Scope::Targets(
                permitted
                    .iter()
                    .map(AssetRef::try_from)
                    .collect::<Result<_, _>>()?,
            ),
            read: Fields::Selected(vec!["relations".into()]),
            write: Fields::Selected(Vec::new()),
            writable: false,
        });
        let before = store.usage()?;
        let result = store.workspace(
            &actor,
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(before.revision),
                    ..Default::default()
                }),
                command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                    decision: api::IdentityDecision::DistinctInstallations as i32,
                    mappings: vec![mapping(&duplicate, &retained)],
                    ..Default::default()
                })),
            },
            100,
        );
        assert!(
            matches!(result, Err(Error::Denied)),
            "unreadable impact must not escape: {result:?}"
        );
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

fn native_pair(store: &Store) -> Result<(Vec<wire::AssetRef>, Vec<wire::AssetRef>), Error> {
    let mut first = super::super::host_import::draft();
    let a = super::super::host_import::accept(
        store,
        &super::super::host_import::preview(store, first.clone())?,
    )?
    .targets;
    first.report.as_mut().ok_or(Error::Corrupt)?.source_basis = "e".repeat(64);
    let b = super::super::host_import::accept(
        store,
        &super::super::host_import::preview(store, first)?,
    )?
    .targets;
    Ok((a, b))
}

// Unlike qualified native bindings, two explicit manual namespaces with the
// same label need a reviewed rename. No rejected preview may overwrite either.
#[test]
fn manual_sources_collide_explicitly_and_keep_individual_windows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (a, b) = native_pair(&store)?;
    for (target, address, time) in [(&a[2], "192.0.2.30", 50), (&b[2], "192.0.2.31", 60)] {
        call(
            &store,
            &operator(),
            Command::InterfaceAddresses(api::InterfaceAddresses {
                interface: Some(target.clone()),
                source: "Manual survey".into(),
                observed_at: time,
                addresses: vec![api::CurrentAddress {
                    address: address.into(),
                    stability: api::AddressStability::Stable as i32,
                    state: api::AddressState::Preferred as i32,
                    ..Default::default()
                }],
                complete: true,
            }),
        )?;
    }
    let mut mappings = vec![
        mapping(&b[0], &a[0]),
        mapping(&b[1], &a[1]),
        mapping(&b[2], &a[2]),
    ];
    let before = store.usage()?;
    assert!(matches!(
        prepare(
            &store,
            mappings.clone(),
            api::IdentityDecision::DuplicateInstallation
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    mappings[2].sources.push(api::IdentitySourceChoice {
        before: "Manual survey".into(),
        after: "Separate survey".into(),
    });
    let prepared = prepare(
        &store,
        mappings,
        api::IdentityDecision::DuplicateInstallation,
    )?;
    apply(&store, &prepared.id)?;
    let addresses = read(&store, &a[2])?.current_addresses;
    assert_eq!(addresses.len(), 4);
    for (name, address, time) in [
        ("Manual survey", "192.0.2.30", 50),
        ("Separate survey", "192.0.2.31", 60),
    ] {
        let view = addresses
            .iter()
            .find(|v| v.coverage.as_ref().is_some_and(|c| c.source == name))
            .ok_or("source")?;
        assert_eq!(view.observations[0].observed_at, Some(time));
        assert_eq!(
            view.observations[0].value.as_ref().ok_or("value")?.address,
            address
        );
    }
    Ok(())
}

// Hardware equality cannot silently carry physical port/cable attachments.
// Individual confirmation still passes through ordinary relation validation.
#[test]
fn hardware_correction_requires_each_physical_attachment_confirmation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let a = create(&store, &operator(), K::Device, "Retained")?;
    let b = create(&store, &operator(), K::Device, "Duplicate")?;
    let port = child(&store, &b, K::Port, "Physical socket")?;
    let relations = read(&store, &b)?.relations;
    let physical = relations
        .iter()
        .find(|r| r.kind == wire::RelationKind::HasPort as i32)
        .ok_or("physical attachment")?;
    let mut selected = mapping(&b, &a);
    let before = store.usage()?;
    assert!(matches!(
        prepare(
            &store,
            vec![selected.clone()],
            api::IdentityDecision::DistinctInstallations
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    selected.attachments.push(physical.relation_id.clone());
    let preview = prepare(
        &store,
        vec![selected],
        api::IdentityDecision::DistinctInstallations,
    )?;
    apply(&store, &preview.id)?;
    assert!(
        read(&store, &port)?
            .relations
            .iter()
            .any(|r| r.relation_id == physical.relation_id && r.from.as_ref() == Some(&a))
    );
    Ok(())
}

// Finite review and inverse custody expire independently of minimal retirement
// fences. Bounded cleanup removes detail without resurrecting a retired subject.
#[test]
fn correction_custody_expires_and_cleanup_preserves_alias_fences() -> TestResult {
    let dir = tempfile::tempdir()?;
    let limits = policy();
    let store = Store::open(dir.path(), limits.clone())?;
    let a = create(&store, &operator(), K::Device, "A")?;
    let b = create(&store, &operator(), K::Device, "B")?;
    let preview = prepare(
        &store,
        vec![mapping(&b, &a)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    let before = store.usage()?;
    let request = api::Request {
        working: Some(api::WorkingContext::default()),
        command: Some(Command::CorrectIdentity(api::IdentityCorrection {
            preview: Some(preview.id.clone()),
            ..Default::default()
        })),
    };
    assert!(matches!(
        store.workspace(&operator(), request, preview.expires_at),
        Err(Error::Expired)
    ));
    assert_eq!(store.usage()?, before);
    let applied = apply(&store, &preview.id)?;
    assert!(matches!(
        store.workspace(
            &operator(),
            api::Request {
                working: Some(api::WorkingContext::default()),
                command: Some(Command::CorrectIdentity(api::IdentityCorrection {
                    undo: Some(applied.id),
                    ..Default::default()
                })),
            },
            applied.undo_until
        ),
        Err(Error::Expired)
    ));
    let mut cursor = None;
    loop {
        let result = store.cleanup(10_000, cursor.as_deref())?;
        cursor = result.cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(store.usage()?.rows[Class::Detail as usize], 0);
    assert!(matches!(read(&store, &b), Err(Error::Missing)));
    let Reply::ResolvedIdentity(alias) = call(
        &store,
        &operator(),
        Command::ResolveIdentity(api::ResolveIdentity { original: Some(b) }),
    )?
    else {
        return Err("alias".into());
    };
    assert_eq!(alias.canonical.ok_or("canonical")?.target, Some(a));
    Ok(())
}

// Captured inverse storage is admitted before accepting an irreversible domain
// correction. A full detail quota leaves both subjects and the preview intact.
#[test]
fn inverse_quota_failure_rolls_back_the_complete_correction() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut limits = policy();
    // Two creation audits plus two correction audits fit; inverse custody does not.
    limits.classes[Class::Detail as usize].rows = 4;
    let store = Store::open(dir.path(), limits)?;
    let a = create(&store, &operator(), K::Device, "A")?;
    let b = create(&store, &operator(), K::Device, "B")?;
    let preview = prepare(
        &store,
        vec![mapping(&b, &a)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    let before = store.usage()?;
    assert!(matches!(apply(&store, &preview.id), Err(Error::Limit)));
    assert_eq!(store.usage()?, before);
    assert!(read(&store, &a).is_ok());
    assert!(read(&store, &b).is_ok());
    Ok(())
}

// A backup after an accepted correction preserves canonical resolution and the
// retired source fence; restore must not turn old source identities into assets.
#[test]
fn restored_correction_keeps_canonical_identity_and_retirement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let a = create(&store, &operator(), K::Device, "A")?;
    let b = create(&store, &operator(), K::Device, "B")?;
    let preview = prepare(
        &store,
        vec![mapping(&b, &a)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    apply(&store, &preview.id)?;
    let mut archive = Vec::new();
    store.export(&mut archive)?;
    let dest = tempfile::tempdir()?;
    let restored = Store::restore(dest.path(), &archive, policy(), archive.len() as u64)?;
    assert!(matches!(read(&restored, &b), Err(Error::Missing)));
    let Reply::ResolvedIdentity(alias) = call(
        &restored,
        &operator(),
        Command::ResolveIdentity(api::ResolveIdentity { original: Some(b) }),
    )?
    else {
        return Err("alias".into());
    };
    assert_eq!(alias.canonical.ok_or("canonical")?.target, Some(a));
    Ok(())
}

// An unresolved effect blocks correction. A proven terminal intent stays on
// its original incarnation and must not prevent ordinary inventory correction.
#[test]
fn terminal_operations_are_preserved_without_retargeting() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let a = create(&store, &operator(), K::VirtualMachine, "Retained VM")?;
    let b = create(&store, &operator(), K::VirtualMachine, "Duplicate VM")?;
    let target = AssetRef::try_from(&b)?;
    let id = store.accept(&plan(target, &[]), 100)?;
    assert!(matches!(
        prepare(
            &store,
            vec![mapping(&b, &a)],
            api::IdentityDecision::DistinctInstallations
        ),
        Err(Error::UnsafeTransition)
    ));
    let lease = store.claim(id, "worker", 101, 200)?;
    store.reconcile(
        &lease,
        1,
        wire::OperationState::Cancelled,
        wire::EffectOutcome::NotSent,
        None,
        102,
    )?;
    let preview = prepare(
        &store,
        vec![mapping(&b, &a)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    apply(&store, &preview.id)?;
    assert_eq!(store.operation(id)?.target, target);
    assert!(store.pending(None)?.is_empty());
    Ok(())
}

// Interface equivalence cannot choose between two still-independent OS parents,
// and a cyclic mapping cannot create an alias loop or partial relation changes.
#[test]
fn incomplete_layer_mapping_and_alias_cycles_are_atomic_conflicts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (a, b) = native_pair(&store)?;
    let before = store.usage()?;
    assert!(matches!(
        prepare(
            &store,
            vec![mapping(&b[2], &a[2])],
            api::IdentityDecision::DuplicateInstallation
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert!(matches!(
        prepare(
            &store,
            vec![mapping(&b[0], &a[0]), mapping(&a[0], &b[0])],
            api::IdentityDecision::DuplicateInstallation
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Concurrent acceptance of one reviewed change yields one canonical effect.
// A losing CAS can retry the exact receipt without another retirement/audit.
#[test]
fn concurrent_correction_acceptance_is_repeat_safe() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let a = create(&store, &operator(), K::Device, "A")?;
    let b = create(&store, &operator(), K::Device, "B")?;
    let preview = prepare(
        &store,
        vec![mapping(&b, &a)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    let gate = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            gate.wait();
            apply(&store, &preview.id)
        });
        let second = scope.spawn(|| {
            gate.wait();
            apply(&store, &preview.id)
        });
        [
            first.join().expect("first correction writer"),
            second.join().expect("second correction writer"),
        ]
    });
    let accepted = outcomes
        .iter()
        .find_map(|v| v.as_ref().ok())
        .ok_or("no correction accepted")?;
    for outcome in &outcomes {
        match outcome {
            Ok(value) => assert_eq!(value, accepted),
            Err(Error::Conflict) => {}
            Err(error) => return Err(error.to_string().into()),
        }
    }
    let before = store.usage()?;
    assert_eq!(apply(&store, &preview.id)?, *accepted);
    assert_eq!(store.usage()?, before);
    assert!(matches!(read(&store, &b), Err(Error::Missing)));
    assert!(read(&store, &a).is_ok());
    Ok(())
}
