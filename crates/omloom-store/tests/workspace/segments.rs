use super::*;
use api::{reply::Result as R, request::Command as C};

fn run(
    store: &Store,
    actor: &Context,
    command: C,
    working: api::WorkingContext,
) -> Result<R, Error> {
    store
        .workspace(
            actor,
            api::Request {
                command: Some(command),
                working: Some(working),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)
}
fn selection(store: &Store, targets: &[wire::AssetRef]) -> Result<api::WorkingContext, Error> {
    Ok(api::WorkingContext {
        roots: targets.to_vec(),
        expected: Some(store.usage()?.revision),
        ..Default::default()
    })
}
fn proposal(targets: &[wire::AssetRef]) -> api::Segment {
    api::Segment {
        participants: targets
            .iter()
            .map(|subject| api::SegmentParticipant {
                subject: Some(subject.clone()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn create_segment(store: &Store, subjects: &[wire::AssetRef]) -> Result<wire::AssetRef, Error> {
    let R::Mutation(result) = run(
        store,
        &operator(),
        C::Segment(proposal(subjects)),
        selection(store, subjects)?,
    )?
    else {
        return Err(Error::Corrupt);
    };
    result.targets.last().cloned().ok_or(Error::Corrupt)
}

// Import mappings must require the same field authority as new-segment admission.
#[test]
fn participant_mapping_requires_network_and_relation_write() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    for (fields, permitted) in [
        (vec!["name", "relations"], false),
        (vec!["name", "network"], false),
        (vec!["name", "relations", "network"], true),
    ] {
        let mut actor = operator();
        actor.grants[0].write = Fields::Selected(fields.into_iter().map(str::to_owned).collect());
        let R::Actions(actions) = call(&store, &actor, C::Prepare(api::Prepare::default()))? else {
            panic!("actions");
        };
        let creation = actions
            .creations
            .iter()
            .find(|creation| creation.kind == K::L2Domain as i32)
            .ok_or("ordinary segment creation remains available")?;
        assert_eq!(
            creation.fields.iter().any(|field| field == "participants"),
            permitted
        );
        assert!(creation.fields.iter().any(|field| field == "name"));
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Cards and the selected network projection expose the actual relation without hidden peers or read writes.
#[test]
fn projection_keeps_attributed_identity_and_filters_hidden_segments() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let segment = create_segment(&store, std::slice::from_ref(&vm))?;
    let relation = store.relations(AssetRef::try_from(&vm)?, 20)?.remove(0);
    let before = store.usage()?;
    let request = C::SegmentMemberships(api::SegmentMemberships {
        targets: vec![vm.clone()],
        cursor: None,
    });
    let R::SegmentMembershipPage(page) = run(
        &store,
        &actor,
        request.clone(),
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("page");
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].relation, Some(relation));
    assert_eq!(
        page.entries[0]
            .segment
            .as_ref()
            .and_then(|i| i.target.as_ref()),
        Some(&segment)
    );
    assert_eq!(
        page.entries[0]
            .subject
            .as_ref()
            .and_then(|i| i.target.as_ref()),
        Some(&vm)
    );
    let mut limited = actor.clone();
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&vm)?]);
    let R::SegmentMembershipPage(hidden) = run(
        &store,
        &limited,
        request,
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("page");
    };
    assert!(hidden.entries.is_empty());
    assert!(hidden.next.is_none());
    assert_eq!(store.usage()?, before);
    Ok(())
}

// More than one page preserves stable ordering and rejects cursors after inventory or authority changes.
#[test]
fn membership_pages_are_revision_and_authority_bound() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    for _ in 0..27 {
        create_segment(&store, std::slice::from_ref(&vm))?;
    }
    let read = |actor: &Context, cursor| {
        run(
            &store,
            actor,
            C::SegmentMemberships(api::SegmentMemberships {
                targets: vec![vm.clone()],
                cursor,
            }),
            selection(&store, std::slice::from_ref(&vm))?,
        )
    };
    let before = store.usage()?;
    let R::SegmentMembershipPage(first) = read(&actor, None)? else {
        panic!("page");
    };
    assert_eq!(first.entries.len(), 25);
    let cursor = first.next.ok_or("continuation")?;
    let R::SegmentMembershipPage(second) = read(&actor, Some(cursor.clone()))? else {
        panic!("page");
    };
    assert_eq!(second.entries.len(), 2);
    assert!(second.next.is_none());
    assert!(
        !first
            .entries
            .iter()
            .any(|a| second.entries.iter().any(|b| a.relation == b.relation))
    );
    let mut changed = actor.clone();
    changed.authority_revision += 1;
    assert!(matches!(
        read(&changed, Some(cursor.clone())),
        Err(Error::Expired)
    ));
    assert_eq!(store.usage()?, before);
    create(&store, &actor, K::Device, "new revision")?;
    assert!(matches!(read(&actor, Some(cursor)), Err(Error::Expired)));
    Ok(())
}

// CSV uses the same atomic membership command and never persists its transport-only participants field.
#[test]
fn csv_segment_import_is_atomic_and_preserves_membership_identity_on_refresh() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let csv = format!(
        "external_id,name,participants,description\nsegment-1,,{}/{},Manual segment\n",
        vm.asset_id, vm.incarnation
    );
    let request = api::Import {
        source: "manual-segments".into(),
        kind: K::L2Domain as i32,
        csv,
        mappings: ["name", "participants", "description"]
            .map(|v| api::Mapping {
                source: v.into(),
                field: v.into(),
            })
            .into(),
        ..Default::default()
    };
    let mut target = None;
    let mut relation = None;
    for _ in 0..2 {
        let R::Preview(preview) = call(&store, &actor, C::Import(request.clone()))? else {
            panic!("preview");
        };
        assert!(preview.id.is_some(), "{preview:?}");
        let apply = api::Import {
            preview: preview.id,
            ..Default::default()
        };
        let working = api::WorkingContext {
            expected: Some(preview.revision),
            ..Default::default()
        };
        let R::Mutation(saved) = run(&store, &actor, C::Import(apply.clone()), working.clone())?
        else {
            panic!("mutation");
        };
        if let Some(target) = &target {
            assert_eq!(saved.targets.first(), Some(target));
        }
        target = saved.targets.first().cloned();
        let current = store.relations(AssetRef::try_from(&vm)?, 20)?;
        assert_eq!(current.len(), 1);
        if let Some(relation) = &relation {
            assert_eq!(&current[0], relation);
        }
        relation = Some(current[0].clone());
        let before = store.usage()?;
        assert_eq!(
            run(&store, &actor, C::Import(apply), working)?,
            R::Mutation(saved)
        );
        assert_eq!(store.usage()?, before);
    }
    let R::Detail(detail) = call(
        &store,
        &actor,
        C::Read(api::Read {
            target,
            ..Default::default()
        }),
    )?
    else {
        panic!("detail");
    };
    let fields = detail.item.ok_or("item")?.fields;
    assert!(fields.iter().all(|field| field.name != "participants"));
    assert_eq!(
        fields.iter().find(|field| field.name == "description"),
        Some(&text_field("description", "Manual segment"))
    );
    Ok(())
}

// Explicit L2 correction reuses the common custody/undo mechanism without auto-merging equal labels.
#[test]
fn segment_merge_retargets_only_reviewed_memberships_and_undo_restores_them() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let first = create_segment(&store, std::slice::from_ref(&vm))?;
    let second = create_segment(&store, std::slice::from_ref(&vm))?;
    let original = store.relations(AssetRef::try_from(&vm)?, 20)?;
    assert_eq!(original.len(), 2);
    let attachments = store
        .relations(AssetRef::try_from(&first)?, 20)?
        .into_iter()
        .map(|r| r.relation_id)
        .collect();
    let R::IdentityCorrection(preview) = run(
        &store,
        &actor,
        C::CorrectIdentity(api::IdentityCorrection {
            decision: api::IdentityDecision::DuplicateResource as i32,
            mappings: vec![api::IdentityMapping {
                duplicate: Some(first.clone()),
                retained: Some(second.clone()),
                attachments,
                ..Default::default()
            }],
            ..Default::default()
        }),
        selection(&store, &[])?,
    )
    .map_err(|error| format!("segment correction preview: {error}"))?
    else {
        panic!("preview");
    };
    let R::IdentityCorrected(receipt) = run(
        &store,
        &actor,
        C::CorrectIdentity(api::IdentityCorrection {
            preview: Some(preview.id),
            ..Default::default()
        }),
        selection(&store, &[])?,
    )
    .map_err(|error| format!("segment correction commit: {error}"))?
    else {
        panic!("receipt");
    };
    let after = store.relations(AssetRef::try_from(&vm)?, 20)?;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].to.as_ref(), Some(&second));
    run(
        &store,
        &actor,
        C::CorrectIdentity(api::IdentityCorrection {
            undo: Some(receipt.id),
            ..Default::default()
        }),
        selection(&store, &[])?,
    )
    .map_err(|error| format!("segment correction undo: {error}"))?;
    let restored = store.relations(AssetRef::try_from(&vm)?, 20)?;
    assert_eq!(restored.len(), 2);
    assert!(original.iter().all(|r| {
        restored
            .iter()
            .any(|v| v.relation_id == r.relation_id && v.from == r.from && v.to == r.to)
    }));
    Ok(())
}

// Interface refinement and reads share the same ownership boundary, including stacked interfaces.
#[test]
fn interface_refinement_projects_owned_layers_and_replays_after_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let child = |parent: &wire::AssetRef, kind, name| -> Result<wire::AssetRef, Error> {
        let R::Mutation(result) = run(
            &store,
            &actor,
            C::Write(api::Write {
                kind,
                parent: Some(parent.clone()),
                changes: vec![text_field("name", name)],
                ..Default::default()
            }),
            api::WorkingContext {
                focus: Some(parent.clone()),
                expected: Some(store.usage()?.revision),
                ..Default::default()
            },
        )?
        else {
            return Err(Error::Corrupt);
        };
        result.targets.first().cloned().ok_or(Error::Corrupt)
    };
    let os = child(&vm, K::SoftwareInstallation as i32, "OS")?;
    let interface = child(&os, K::Interface as i32, "eth0")?;
    let stacked = child(&interface, K::Interface as i32, "eth0.20")?;
    let segment = create_segment(&store, std::slice::from_ref(&vm))?;
    let original = store
        .relations(AssetRef::try_from(&segment)?, 20)?
        .remove(0);
    let working = selection(&store, &[vm.clone(), segment.clone()])?;
    let request = api::Segment {
        target: Some(segment.clone()),
        participants: vec![api::SegmentParticipant {
            subject: Some(vm.clone()),
            interface: Some(stacked.clone()),
            relation_id: Some(original.relation_id.clone()),
        }],
        ..Default::default()
    };
    let accepted = run(&store, &actor, C::Segment(request.clone()), working.clone())?;
    let before = store.usage()?;
    let R::SegmentMembershipPage(page) = run(
        &store,
        &actor,
        C::SegmentMemberships(api::SegmentMemberships {
            targets: vec![vm.clone()],
            cursor: None,
        }),
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("page");
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        page.entries[0]
            .source
            .as_ref()
            .and_then(|i| i.target.as_ref()),
        Some(&stacked)
    );
    assert_eq!(
        page.entries[0]
            .relation
            .as_ref()
            .map(|r| r.relation_id.as_str()),
        Some(original.relation_id.as_str())
    );
    assert_eq!(store.usage()?, before);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    assert_eq!(run(&store, &actor, C::Segment(request), working)?, accepted);
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A source's assertion is readable but cannot be silently adopted or moved by manual admission.
#[test]
fn foreign_assertion_and_hidden_merge_endpoint_preserve_all_state() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let segment = create_segment(&store, std::slice::from_ref(&vm))?;
    let second = create_segment(&store, std::slice::from_ref(&vm))?;
    let mut relation = store
        .relations(AssetRef::try_from(&segment)?, 20)?
        .remove(0);
    let expected = relation.revision;
    relation.authority_id = omloom_contracts::ids::AuthorityId::generate().to_string();
    let record = admit(wire::record::Body::Relation(relation.clone()));
    store.save_relation(&record, Some(expected), &actor.actor, 100)?;
    let before = store.usage()?;
    let request = api::Segment {
        target: Some(segment.clone()),
        ..proposal(std::slice::from_ref(&vm))
    };
    for command in [C::PreviewSegment(request.clone()), C::Segment(request)] {
        assert!(matches!(
            run(
                &store,
                &actor,
                command,
                selection(&store, &[vm.clone(), segment.clone()])?
            ),
            Err(Error::Conflict)
        ));
    }
    let mut limited = actor.clone();
    limited.grants[0].scope = Scope::Targets(vec![
        AssetRef::try_from(&segment)?,
        AssetRef::try_from(&second)?,
    ]);
    let request = api::IdentityCorrection {
        decision: api::IdentityDecision::DuplicateResource as i32,
        mappings: vec![api::IdentityMapping {
            duplicate: Some(segment.clone()),
            retained: Some(second.clone()),
            attachments: vec![relation.relation_id],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        run(
            &store,
            &limited,
            C::CorrectIdentity(request),
            selection(&store, &[])?
        )
        .is_err()
    );
    assert_eq!(store.usage()?, before);
    assert_eq!(store.relations(AssetRef::try_from(&vm)?, 20)?.len(), 2);
    Ok(())
}

// Concurrent retries share one accepted outcome, which remains recoverable from a logical backup.
#[test]
fn concurrent_creation_and_restore_keep_one_domain_and_assertion() -> TestResult {
    let dir = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let request = proposal(std::slice::from_ref(&vm));
    let working = selection(&store, std::slice::from_ref(&vm))?;
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            run(&store, &actor, C::Segment(request.clone()), working.clone())
        });
        let second = scope.spawn(|| {
            barrier.wait();
            run(&store, &actor, C::Segment(request.clone()), working.clone())
        });
        [
            first.join().expect("first mutation"),
            second.join().expect("second mutation"),
        ]
    });
    let saved = run(&store, &actor, C::Segment(request.clone()), working.clone())?;
    assert!(outcomes.iter().any(Result::is_ok));
    for outcome in outcomes {
        match outcome {
            Ok(value) => assert_eq!(value, saved),
            Err(Error::Conflict) => {}
            Err(error) => return Err(error.into()),
        }
    }
    assert_eq!(store.relations(AssetRef::try_from(&vm)?, 20)?.len(), 1);
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored = Store::restore(destination.path(), &image, policy(), image.len() as u64)?;
    let before = restored.usage()?;
    assert_eq!(run(&restored, &actor, C::Segment(request), working)?, saved);
    assert_eq!(restored.usage()?, before);
    assert_eq!(restored.relations(AssetRef::try_from(&vm)?, 20)?.len(), 1);
    Ok(())
}

// Canonical VLAN references cannot be silently reallocated by a knowledge-only segment merge.
#[test]
fn segment_merge_refuses_a_canonical_vlan_reference_without_partial_changes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let first = create_segment(&store, std::slice::from_ref(&vm))?;
    let second = create_segment(&store, std::slice::from_ref(&vm))?;
    run(
        &store,
        &actor,
        C::Network(api::Network {
            kind: K::Vlan as i32,
            name: "VLAN 20".into(),
            vlan_id: Some(20),
            link: Some(first.clone()),
            sharing: api::SharedUse::Exclusive as i32,
            ..Default::default()
        }),
        selection(&store, &[])?,
    )?;
    let before = store.usage()?;
    let attachments = store
        .relations(AssetRef::try_from(&first)?, 20)?
        .into_iter()
        .map(|r| r.relation_id)
        .collect();
    let request = api::IdentityCorrection {
        decision: api::IdentityDecision::DuplicateResource as i32,
        mappings: vec![api::IdentityMapping {
            duplicate: Some(first),
            retained: Some(second),
            attachments,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(matches!(
        run(
            &store,
            &actor,
            C::CorrectIdentity(request),
            selection(&store, &[])?
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(store.relations(AssetRef::try_from(&vm)?, 20)?.len(), 2);
    Ok(())
}

// The actual selected projection uses indexed root access, not a global asset scan or history replay.
#[test]
fn selected_membership_plans_start_with_indexed_keys() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    store.set_diagnostic_query_check(|db, statement| {
        if !statement.contains("r.key>$after") {
            return;
        }
        let plan = db
            .explain_plan(statement, None)
            .expect("membership plan")
            .explain();
        assert!(
            plan.contains("ON omloom_key(key)"),
            "selected root must use its key: {plan}"
        );
        assert!(
            !plan.contains("NodeScan("),
            "membership must not scan the estate: {plan}"
        );
    });
    let vm = create(&store, &operator(), K::VirtualMachine, "VM")?;
    let segment = create_segment(&store, std::slice::from_ref(&vm))?;
    for target in [vm, segment] {
        let R::SegmentMembershipPage(page) = run(
            &store,
            &operator(),
            C::SegmentMemberships(api::SegmentMemberships {
                targets: vec![target],
                cursor: None,
            }),
            selection(&store, &[])?,
        )?
        else {
            panic!("page");
        };
        assert_eq!(page.entries.len(), 1);
    }
    Ok(())
}

// Repeating a coarser claim must not replace a known interface or append a redundant unknown one.
#[test]
fn coarser_membership_preserves_the_single_known_interface_without_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let R::Mutation(interface) = run(
        &store,
        &actor,
        C::Write(api::Write {
            kind: K::Interface as i32,
            parent: Some(vm.clone()),
            changes: vec![text_field("name", "eth0")],
            ..Default::default()
        }),
        api::WorkingContext {
            focus: Some(vm.clone()),
            expected: Some(store.usage()?.revision),
            ..Default::default()
        },
    )?
    else {
        panic!("interface");
    };
    let interface = interface.targets[0].clone();
    let R::Mutation(segment) = run(
        &store,
        &actor,
        C::Segment(api::Segment {
            participants: vec![api::SegmentParticipant {
                subject: Some(vm.clone()),
                interface: Some(interface.clone()),
                ..Default::default()
            }],
            ..Default::default()
        }),
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("segment");
    };
    let segment = segment.targets.last().ok_or("segment")?.clone();
    let before = store.usage()?;
    run(
        &store,
        &actor,
        C::Segment(api::Segment {
            target: Some(segment.clone()),
            ..proposal(std::slice::from_ref(&vm))
        }),
        selection(&store, &[vm.clone(), segment.clone()])?,
    )?;
    assert_eq!(store.usage()?, before);
    assert_eq!(store.relations(AssetRef::try_from(&segment)?, 20)?.len(), 1);
    Ok(())
}

// A nested hypervisor VM cannot claim its guest's NIC; candidates and commit enforce the same boundary.
#[test]
fn interface_candidates_and_admission_stop_at_another_vm() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "Outer VM")?;
    let child = |parent: &wire::AssetRef, kind, changes| -> Result<wire::AssetRef, Error> {
        let R::Mutation(value) = run(
            &store,
            &actor,
            C::Write(api::Write {
                parent: Some(parent.clone()),
                kind,
                changes,
                ..Default::default()
            }),
            api::WorkingContext {
                focus: Some(parent.clone()),
                expected: Some(store.usage()?.revision),
                ..Default::default()
            },
        )?
        else {
            return Err(Error::Corrupt);
        };
        value.targets.first().cloned().ok_or(Error::Corrupt)
    };
    let os = child(
        &vm,
        K::SoftwareInstallation as i32,
        vec![
            text_field("name", "Hypervisor OS"),
            text_field(
                "virtualization_role",
                api::VirtualizationRole::Hypervisor.as_str_name(),
            ),
        ],
    )?;
    let guest = child(
        &os,
        K::VirtualMachine as i32,
        vec![text_field("name", "Guest VM")],
    )?;
    let guest_nic = child(
        &guest,
        K::Interface as i32,
        vec![text_field("name", "eth0")],
    )?;
    let own_nic = child(&os, K::Interface as i32, vec![text_field("name", "eth0")])?;
    let before = store.usage()?;
    let mut reader = actor.clone();
    reader.grants[0].writable = false;
    let R::Page(page) = run(
        &store,
        &reader,
        C::SegmentInterfaces(api::SegmentInterfaces {
            subject: Some(vm.clone()),
            ..Default::default()
        }),
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("candidates");
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target, Some(own_nic));
    assert!(
        page.contexts
            .iter()
            .flat_map(|context| &context.owners)
            .any(|owner| owner.target.as_ref() == Some(&os))
    );
    let request = api::Segment {
        participants: vec![api::SegmentParticipant {
            subject: Some(vm.clone()),
            interface: Some(guest_nic),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(matches!(
        run(
            &store,
            &actor,
            C::Segment(request),
            selection(&store, std::slice::from_ref(&vm))?
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Two real interfaces on one segment require a choice, never coarsening or deduplicating their assertions.
#[test]
fn multiple_participations_need_an_explicit_interface_or_assertion() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let mut segment = None;
    let mut interfaces = Vec::new();
    for name in ["eth0", "eth1"] {
        let R::Mutation(created) = run(
            &store,
            &actor,
            C::Write(api::Write {
                kind: K::Interface as i32,
                parent: Some(vm.clone()),
                changes: vec![text_field("name", name)],
                ..Default::default()
            }),
            api::WorkingContext {
                focus: Some(vm.clone()),
                expected: Some(store.usage()?.revision),
                ..Default::default()
            },
        )?
        else {
            panic!("interface");
        };
        let interface = created.targets[0].clone();
        interfaces.push(interface.clone());
        let mut roots = vec![vm.clone()];
        roots.extend(segment.clone());
        let R::Mutation(created) = run(
            &store,
            &actor,
            C::Segment(api::Segment {
                target: segment.clone(),
                participants: vec![api::SegmentParticipant {
                    subject: Some(vm.clone()),
                    interface: Some(interface),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            selection(&store, &roots)?,
        )?
        else {
            panic!("segment");
        };
        segment = created.targets.last().cloned();
    }
    let segment = segment.ok_or("segment")?;
    let request = api::Segment {
        target: Some(segment.clone()),
        ..proposal(std::slice::from_ref(&vm))
    };
    let before = store.usage()?;
    let working = selection(&store, &[vm.clone(), segment.clone()])?;
    assert!(matches!(
        run(
            &store,
            &actor,
            C::PreviewSegment(request.clone()),
            working.clone()
        ),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        run(&store, &actor, C::Segment(request.clone()), working.clone()),
        Err(Error::Conflict)
    ));
    let mut request = request;
    request.participants[0].interface = Some(interfaces[0].clone());
    run(&store, &actor, C::Segment(request), working)?;
    assert_eq!(store.usage()?, before);
    assert_eq!(store.relations(AssetRef::try_from(&segment)?, 20)?.len(), 2);
    Ok(())
}

// Recording partial network knowledge is atomic, replay-safe and never creates interfaces or placement.
#[test]
fn unnamed_segment_records_selected_vms_and_replays_without_another_write() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let subjects = [
        create(&store, &actor, K::VirtualMachine, "first")?,
        create(&store, &actor, K::VirtualMachine, "second")?,
    ];
    let working = selection(&store, &subjects)?;
    let request = proposal(&subjects);
    let before = store.usage()?;
    let R::SegmentReview(preview) = run(
        &store,
        &actor,
        C::PreviewSegment(request.clone()),
        working.clone(),
    )?
    else {
        panic!("preview")
    };
    assert!(preview.segment.is_none());
    assert_eq!(preview.participants.len(), 2);
    assert!(
        preview
            .participants
            .iter()
            .all(|row| row.interface.is_none())
    );
    assert_eq!(store.usage()?, before);
    let R::Mutation(saved) = run(&store, &actor, C::Segment(request.clone()), working.clone())?
    else {
        panic!("mutation")
    };
    let segment = saved.targets.last().ok_or("segment")?.clone();
    for subject in &subjects {
        let relations = store.relations(AssetRef::try_from(subject)?, 20)?;
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].to.as_ref(), Some(&segment));
        assert_eq!(relations[0].kind, wire::RelationKind::On as i32);
    }
    let after = store.usage()?;
    let R::Mutation(replayed) = run(&store, &actor, C::Segment(request), working)? else {
        panic!("receipt")
    };
    assert_eq!(saved, replayed);
    assert_eq!(store.usage()?, after);
    let mut roots = subjects.to_vec();
    roots.push(segment.clone());
    let request = api::Segment {
        target: Some(segment),
        ..proposal(&subjects)
    };
    run(
        &store,
        &actor,
        C::Segment(request),
        selection(&store, &roots)?,
    )?;
    assert_eq!(store.usage()?, after);
    Ok(())
}

// Learning one interface keeps the chosen assertion ID and unrelated memberships intact.
#[test]
fn refining_one_participant_keeps_relation_identity_and_other_networks() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let vm = create(&store, &actor, K::VirtualMachine, "VM")?;
    let R::Mutation(first) = run(
        &store,
        &actor,
        C::Segment(proposal(std::slice::from_ref(&vm))),
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("first")
    };
    let first = first.targets.last().ok_or("segment")?.clone();
    let R::Mutation(second) = run(
        &store,
        &actor,
        C::Segment(proposal(std::slice::from_ref(&vm))),
        selection(&store, std::slice::from_ref(&vm))?,
    )?
    else {
        panic!("second")
    };
    let second = second.targets.last().ok_or("segment")?.clone();
    assert_ne!(first, second);
    let binding = store
        .relations(AssetRef::try_from(&vm)?, 20)?
        .into_iter()
        .find(|r| r.to.as_ref() == Some(&first))
        .ok_or("binding")?;
    let R::Mutation(interface) = call(
        &store,
        &actor,
        C::Write(api::Write {
            kind: K::Interface as i32,
            parent: Some(vm.clone()),
            changes: vec![text_field("name", "eth0")],
            ..Default::default()
        }),
    )?
    else {
        panic!("interface")
    };
    let interface = interface.targets[0].clone();
    let request = api::Segment {
        target: Some(first.clone()),
        participants: vec![api::SegmentParticipant {
            subject: Some(vm.clone()),
            interface: Some(interface.clone()),
            relation_id: Some(binding.relation_id.clone()),
        }],
        name: None,
    };
    run(
        &store,
        &actor,
        C::Segment(request),
        selection(&store, &[vm.clone(), first.clone()])?,
    )?;
    let refined = store.relation(binding.relation_id.parse()?)?;
    assert_eq!(refined.from.as_ref(), Some(&interface));
    assert_eq!(refined.to.as_ref(), Some(&first));
    assert_eq!(refined.revision, binding.revision + 1);
    assert!(
        store
            .relations(AssetRef::try_from(&vm)?, 20)?
            .iter()
            .any(|r| r.to.as_ref() == Some(&second))
    );
    Ok(())
}

// A revoked member and a stale reviewed revision reject the entire selected command.
#[test]
fn segment_admission_rejects_denied_and_stale_selections_without_partial_state() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let subjects = [
        create(&store, &actor, K::VirtualMachine, "first")?,
        create(&store, &actor, K::VirtualMachine, "second")?,
    ];
    let mut denied = actor.clone();
    denied.grants = vec![Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&subjects[0])?]),
        read: Fields::All,
        write: Fields::All,
        writable: true,
    }];
    let working = selection(&store, &subjects)?;
    let before = store.usage()?;
    assert!(
        run(
            &store,
            &denied,
            C::Segment(proposal(&subjects)),
            working.clone()
        )
        .is_err()
    );
    assert_eq!(store.usage()?, before);
    create(&store, &actor, K::Device, "independent")?;
    let before = store.usage()?;
    assert!(matches!(
        run(&store, &actor, C::Segment(proposal(&subjects)), working),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}
