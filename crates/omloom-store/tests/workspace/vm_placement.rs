use super::*;
use api::{reply::Result as Reply, request::Command};
use wire::RelationKind as R;

pub(super) fn hypervisor(store: &Store, name: &str) -> Result<wire::AssetRef, Error> {
    let Reply::Mutation(result) = call(
        store,
        &operator(),
        Command::Write(api::Write {
            kind: K::SoftwareInstallation as i32,
            changes: vec![
                text_field("name", name),
                text_field("virtualization_role", "VIRTUALIZATION_ROLE_HYPERVISOR"),
            ],
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    result.targets.into_iter().next().ok_or(Error::Corrupt)
}

fn place(store: &Store, guest: &wire::AssetRef, host: &wire::AssetRef) -> Result<Reply, Error> {
    call(
        store,
        &operator(),
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(guest.clone()),
                to: Some(host.clone()),
                kind: R::RunsOn as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )
}

fn placement(store: &Store, guest: &wire::AssetRef) -> Result<wire::Relation, Error> {
    let Reply::Detail(detail) = call(
        store,
        &operator(),
        Command::Read(api::Read {
            target: Some(guest.clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    detail
        .relations
        .into_iter()
        .find(|r| r.kind == R::RunsOn as i32)
        .ok_or(Error::Missing)
}

fn prepare(
    store: &Store,
    actor: &Context,
    guest: &wire::AssetRef,
    host: Option<&wire::AssetRef>,
) -> Result<api::VmPlacementReview, Error> {
    let Reply::VmPlacementReview(value) = call(
        store,
        actor,
        Command::PrepareVmPlacement(api::PrepareVmPlacement {
            vm: Some(guest.clone()),
            destination: host.cloned(),
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(*value)
}
fn record(
    store: &Store,
    actor: &Context,
    review: &api::VmPlacementReview,
) -> Result<api::Reply, Error> {
    let mut relation = review.current.clone().ok_or(Error::Missing)?;
    relation.to = review.destination.as_ref().and_then(|i| i.target.clone());
    store.workspace(
        actor,
        api::Request {
            working: Some(api::WorkingContext {
                expected: Some(review.revision),
                ..Default::default()
            }),
            command: Some(Command::Relation(api::Relationship {
                relation: Some(relation),
                remove: false,
            })),
        },
        100,
    )
}
fn child(store: &Store, parent: &wire::AssetRef, kind: K) -> Result<wire::AssetRef, Error> {
    let Reply::Mutation(value) = call(
        store,
        &operator(),
        Command::Write(api::Write {
            parent: Some(parent.clone()),
            kind: kind as i32,
            changes: vec![text_field("name", "Guest component")],
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    value.targets.into_iter().next().ok_or(Error::Corrupt)
}
fn address(store: &Store, interface: &wire::AssetRef, value: &str, time: u64) -> Result<(), Error> {
    call(
        store,
        &operator(),
        Command::InterfaceAddresses(api::InterfaceAddresses {
            interface: Some(interface.clone()),
            source: "Guest report".into(),
            observed_at: time,
            complete: true,
            addresses: vec![api::CurrentAddress {
                address: value.into(),
                stability: api::AddressStability::Stable as i32,
                state: api::AddressState::Preferred as i32,
                ..Default::default()
            }],
        }),
    )?;
    Ok(())
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

pub(super) fn responsibility(
    store: &Store,
    target: &wire::AssetRef,
    changes: Vec<api::ResponsibilityAssignment>,
) -> Result<api::ResponsibilityState, Error> {
    let request = api::Responsibility {
        target: Some(target.clone()),
        expected: (!changes.is_empty())
            .then(|| {
                store
                    .asset(AssetRef::try_from(target).map_err(|_| Error::Invalid)?)
                    .map(|a| a.revision)
            })
            .transpose()?,
        action: if changes.is_empty() {
            api::ResponsibilityAction::Read
        } else {
            api::ResponsibilityAction::Apply
        } as i32,
        changes,
        ..Default::default()
    };
    let value = store
        .workspace(
            &operator(),
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(Command::Responsibility(request)),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)?;
    let Reply::Responsibility(value) = value else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}

// Explicit placement-bound inheritance must become unavailable, rather than
// silently taking the new host's owner. Independent direct declarations survive.
#[test]
fn migration_previews_only_permitted_inheritance_and_never_transfers_ownership() -> TestResult {
    use api::{AssignmentMode as M, ResponsibilityFacet as F, ResponsibilityStatus as S};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "First host")?;
    let second = hypervisor(&store, "Second host")?;
    let assigned = |facet: F, subject: &str| api::ResponsibilityAssignment {
        facet: facet as i32,
        mode: M::Explicit as i32,
        party: Some(api::PartyRef {
            issuer: "company.example".into(),
            subject: subject.into(),
            name: subject.into(),
            kind: api::PartyKind::Team as i32,
        }),
        ..Default::default()
    };
    responsibility(
        &store,
        &first,
        vec![assigned(F::ClaimedOwner, "Original team")],
    )?;
    responsibility(
        &store,
        &second,
        vec![assigned(F::ClaimedOwner, "Other team")],
    )?;
    place(&store, &guest, &first)?;
    let old = placement(&store, &guest)?;
    responsibility(
        &store,
        &guest,
        vec![
            api::ResponsibilityAssignment {
                facet: F::ClaimedOwner as i32,
                mode: M::Inherit as i32,
                source_relation: Some(old.relation_id.clone()),
                source_revision: Some(old.revision),
                ..Default::default()
            },
            assigned(F::Operator, "Guest operators"),
        ],
    )?;
    let before = responsibility(&store, &guest, Vec::new())?;
    assert_eq!(
        before
            .effective
            .iter()
            .find(|v| v.facet == F::ClaimedOwner as i32)
            .ok_or("owner")?
            .party
            .as_ref()
            .ok_or("party")?
            .subject,
        "Original team"
    );
    let review = prepare(&store, &operator(), &guest, Some(&second))?;
    assert_eq!(review.invalidated_inheritance.len(), 1);
    assert!(review.invalidated_inheritance[0].contact.is_none());
    assert!(review.invalidated_inheritance[0].source.is_none());
    assert!(review.invalidated_inheritance[0].source_revision.is_none());
    assert_eq!(
        review.invalidated_inheritance[0]
            .party
            .as_ref()
            .ok_or("party")?
            .subject,
        "Original team"
    );
    let mut hidden = operator();
    hidden.grants[0].read = Fields::Selected(vec![
        "name".into(),
        "relations".into(),
        "virtualization_role".into(),
    ]);
    assert!(
        prepare(&store, &hidden, &guest, Some(&second))?
            .invalidated_inheritance
            .is_empty()
    );
    record(&store, &operator(), &review)?;
    let after = responsibility(&store, &guest, Vec::new())?;
    // The read projection hides the no-longer-resolvable source reference,
    // while preserving its INHERIT declaration and every independent facet.
    let mut expected = before.assignments.clone();
    for assignment in &mut expected {
        if assignment.source_relation.as_deref() == Some(old.relation_id.as_str()) {
            assignment.source_relation = None;
            assignment.source_revision = None;
        }
    }
    assert_eq!(after.assignments, expected);
    let owner = after
        .effective
        .iter()
        .find(|v| v.facet == F::ClaimedOwner as i32)
        .ok_or("owner")?;
    assert_eq!(owner.status, S::SourceUnavailable as i32);
    assert!(owner.party.is_none());
    assert_eq!(
        after
            .effective
            .iter()
            .find(|v| v.facet == F::Operator as i32)
            .ok_or("operator")?,
        before
            .effective
            .iter()
            .find(|v| v.facet == F::Operator as i32)
            .ok_or("operator")?
    );
    Ok(())
}

// Placement is one conditional relation change. Guest identity/subtree and
// source-qualified addresses survive it; a later address report is independent.
#[test]
fn migration_keeps_guest_components_and_accepts_a_separate_ip_change() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "First host")?;
    let second = hypervisor(&store, "Second host")?;
    place(&store, &guest, &first)?;
    let os = child(&store, &guest, K::SoftwareInstallation)?;
    let interface = child(&store, &os, K::Interface)?;
    let service = child(&store, &os, K::Service)?;
    address(&store, &interface, "192.0.2.30", 50)?;
    let before: Vec<_> = [&guest, &os, &interface, &service]
        .into_iter()
        .map(|t| {
            Ok((
                t.clone(),
                store.asset(AssetRef::try_from(t).map_err(|_| Error::Invalid)?)?,
            ))
        })
        .collect::<Result<_, Error>>()?;
    let addresses = read(&store, &interface)?.current_addresses;
    let usage = store.usage()?;
    let review = prepare(&store, &operator(), &guest, Some(&second))?;
    assert_eq!(
        review.current_host.as_ref().and_then(|i| i.target.as_ref()),
        Some(&first)
    );
    assert_eq!(store.usage()?, usage, "review must not write");
    record(&store, &operator(), &review)?;
    for (target, asset) in before {
        assert_eq!(store.asset(AssetRef::try_from(&target)?)?, asset);
    }
    let current = placement(&store, &guest)?;
    let previous = review.current.ok_or("placement")?;
    assert_eq!(current.relation_id, previous.relation_id);
    assert_eq!(current.revision, previous.revision + 1);
    assert_eq!(current.to, Some(second.clone()));
    assert_eq!(read(&store, &interface)?.current_addresses, addresses);
    let unchanged = prepare(&store, &operator(), &guest, Some(&second))?;
    let usage = store.usage()?;
    record(&store, &operator(), &unchanged)?;
    assert_eq!(
        store.usage()?,
        usage,
        "repeated unchanged placement must not write"
    );
    address(&store, &interface, "192.0.2.31", 60)?;
    let updated = read(&store, &interface)?.current_addresses;
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].observations.len(), 1);
    assert_eq!(
        updated[0].observations[0]
            .value
            .as_ref()
            .ok_or("address")?
            .address,
        "192.0.2.31"
    );
    assert_eq!(placement(&store, &guest)?, current);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(placement(&store, &guest)?, current);
    assert_eq!(read(&store, &interface)?.current_addresses, updated);
    Ok(())
}

// A caller cannot disguise a hidden old host as "unplaced", use a local VM
// context for peer placement, or apply an obsolete role/revision review.
#[test]
fn placement_review_rechecks_old_host_context_role_and_revision() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "First host")?;
    let second = hypervisor(&store, "Second host")?;
    place(&store, &guest, &first)?;
    let mut hidden = operator();
    hidden.grants[0].scope = Scope::Targets(vec![
        AssetRef::try_from(&guest)?,
        AssetRef::try_from(&second)?,
    ]);
    let usage = store.usage()?;
    assert!(matches!(
        prepare(&store, &hidden, &guest, None),
        Err(Error::Denied)
    ));
    let review = prepare(&store, &operator(), &guest, Some(&second))?;
    assert!(matches!(
        record(&store, &hidden, &review),
        Err(Error::Denied)
    ));
    let local = store.workspace(
        &operator(),
        api::Request {
            working: Some(api::WorkingContext {
                focus: Some(guest.clone()),
                ..Default::default()
            }),
            command: Some(Command::PrepareVmPlacement(api::PrepareVmPlacement {
                vm: Some(guest.clone()),
                destination: Some(second.clone()),
            })),
        },
        100,
    );
    assert!(matches!(local, Err(Error::Denied)));
    assert_eq!(store.usage()?, usage);
    call(
        &store,
        &operator(),
        Command::Write(api::Write {
            target: Some(second.clone()),
            expected: Some(store.asset(AssetRef::try_from(&second)?)?.revision),
            changes: vec![text_field(
                "virtualization_role",
                "VIRTUALIZATION_ROLE_NOT_HYPERVISOR",
            )],
            kind: K::SoftwareInstallation as i32,
            ..Default::default()
        }),
    )?;
    let usage = store.usage()?;
    assert!(matches!(
        record(&store, &operator(), &review),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        prepare(&store, &operator(), &guest, Some(&second)),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, usage);
    assert_eq!(placement(&store, &guest)?.to, Some(first));
    Ok(())
}

// A migration replaces the current placement, rather than adding a second
// accepted host and leaving the same VM visible under both hypervisors.
#[test]
fn second_current_host_requires_replacing_the_existing_placement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "First host")?;
    let second = hypervisor(&store, "Second host")?;
    place(&store, &guest, &first)?;
    let before = store.usage()?;
    assert!(matches!(
        place(&store, &guest, &second),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(placement(&store, &guest)?.to, Some(first));
    Ok(())
}

// In-flight effects keep their original target/placement. Reconciliation must
// settle them before recording another host or deleting the current relation.
#[test]
fn unresolved_effects_block_placement_changes_and_removal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "First host")?;
    let second = hypervisor(&store, "Second host")?;
    place(&store, &guest, &first)?;
    let review = prepare(&store, &operator(), &guest, Some(&second))?;
    let target = AssetRef::try_from(&guest)?;
    let operation = store.accept(&plan(target, &[]), 100)?;
    let usage = store.usage()?;
    assert!(matches!(
        prepare(&store, &operator(), &guest, Some(&second)),
        Err(Error::UnsafeTransition)
    ));
    let mut moved = review.current.clone().ok_or("placement")?;
    moved.to = Some(second.clone());
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::Relation(api::Relationship {
                relation: Some(moved),
                remove: false
            })
        ),
        Err(Error::UnsafeTransition)
    ));
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::Relation(api::Relationship {
                relation: review.current,
                remove: true
            })
        ),
        Err(Error::UnsafeTransition)
    ));
    assert_eq!(store.usage()?, usage);
    let lease = store.claim(operation, "worker", 101, 200)?;
    store.reconcile(
        &lease,
        1,
        wire::OperationState::Cancelled,
        wire::EffectOutcome::NotSent,
        None,
        102,
    )?;
    let fresh = prepare(&store, &operator(), &guest, Some(&second))?;
    record(&store, &operator(), &fresh)?;
    assert_eq!(store.operation(operation)?.target, target);
    assert_eq!(placement(&store, &guest)?.to, Some(second));
    Ok(())
}

// Two reviews share one root/relation basis. Exactly one can change it; the
// loser must obtain a new review rather than adding or overwriting a placement.
#[test]
fn concurrent_moves_accept_only_one_reviewed_destination() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let guest = create(&store, &operator(), K::VirtualMachine, "Guest")?;
    let first = hypervisor(&store, "First host")?;
    let second = hypervisor(&store, "Second host")?;
    let third = hypervisor(&store, "Third host")?;
    place(&store, &guest, &first)?;
    let a = prepare(&store, &operator(), &guest, Some(&second))?;
    let b = prepare(&store, &operator(), &guest, Some(&third))?;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = [&a, &b]
            .into_iter()
            .map(|review| {
                scope.spawn(|| {
                    barrier.wait();
                    record(&store, &operator(), review)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join())
            .collect::<Vec<_>>()
    });
    let results: Vec<_> = results
        .into_iter()
        .map(|r| r.map_err(|_| "placement worker panicked"))
        .collect::<Result<_, _>>()?;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    let host = placement(&store, &guest)?.to.ok_or("host")?;
    assert!(host == second || host == third);
    Ok(())
}
