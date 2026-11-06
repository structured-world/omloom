use super::*;

fn saved(store: &Store, actor: &Context) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Unpack received nodes".into(),
                materials: vec![api::PhysicalMaterial {
                    name: "Compute nodes".into(),
                    quantity: 3,
                    kind: K::Device as i32,
                    specifications: vec![equipment_quantity("width", 500_000, "mm", 3)],
                    ..Default::default()
                }],
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Missing)
}
fn request(plan: &api::PhysicalPlan, base: u64) -> Result<api::PhysicalDeviceReceipt, Error> {
    Ok(api::PhysicalDeviceReceipt {
        plan: Some(api::PhysicalPlanKey {
            id: plan.id.clone().ok_or(Error::Missing)?,
            revision: plan.revision,
        }),
        material: 0,
        unit: 1,
        inventory: Some(api::WorkingContext {
            expected: Some(base),
            ..Default::default()
        }),
        changes: vec![text_field("name", "Unpacked node")],
        explanation: None,
    })
}
fn submit(
    store: &Store,
    actor: &Context,
    working: &api::WorkingContext,
    action: api::physical::Action,
) -> Result<api::PhysicalView, Error> {
    let result = store
        .workspace(
            actor,
            api::Request {
                working: Some(working.clone()),
                command: Some(Command::Physical(api::Physical {
                    action: Some(action),
                })),
            },
            100,
        )
        .map_err(|error| match error {
            Error::Rejected(error) => *error,
            error => error,
        })?
        .result
        .ok_or(Error::Missing)?;
    match result {
        Reply::Physical(view) => Ok(view),
        _ => Err(Error::Corrupt),
    }
}

// A lost reply and later edits never create another item or replay the old field values.
#[test]
fn received_device_is_atomic_unplaced_and_recoverable_after_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Receiving rack")?;
    let plan = saved(&store, &actor)?;
    let base = store.usage()?.revision;
    let working = api::WorkingContext {
        roots: vec![rack],
        expected: Some(base),
        ..Default::default()
    };
    let receipt = request(&plan, base)?;
    let outcome = submit(
        &store,
        &actor,
        &working,
        api::physical::Action::ReceiveDevice(receipt.clone()),
    )?
    .receipt
    .ok_or(Error::Missing)?;
    let item = outcome.item.clone().ok_or(Error::Missing)?;
    let asset = store.asset(AssetRef::try_from(&item).map_err(|_| Error::Invalid)?)?;
    assert_eq!(asset.kind, K::Device as i32);
    let detail = call(
        &store,
        &actor,
        Command::Read(api::Read {
            target: Some(item.clone()),
            ..Default::default()
        }),
    )?;
    let Reply::Detail(detail) = detail else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(
        detail.item.ok_or(Error::Missing)?.fields,
        vec![text_field("name", "Unpacked node")]
    );
    let loaded = submit(
        &store,
        &actor,
        &working,
        api::physical::Action::Load(api::PhysicalPlanKey {
            id: plan.id.clone().ok_or(Error::Missing)?,
            revision: 0,
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    assert_eq!(loaded.materials[0].allocations.len(), 1);
    assert_eq!(loaded.materials[0].allocations[0].unit, 1);
    assert_eq!(loaded.materials[0].allocations[0].item, Some(item.clone()));
    assert_eq!(
        loaded.materials[0].allocations[0].basis,
        "Receipt asserted by fixture-operator at 100 ms UTC (recording time)"
    );
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        0,
        "Draft assignment is not a reservation"
    );
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(item.clone()),
            expected: Some(asset.revision),
            kind: K::Device as i32,
            changes: vec![text_field("name", "Later corrected name")],
            ..Default::default()
        }),
    )?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let replay = submit(
        &store,
        &actor,
        &working,
        api::physical::Action::ReceiveDevice(receipt.clone()),
    )?;
    assert_eq!(replay.receipt, Some(outcome.clone()));
    let recovered = submit(
        &store,
        &actor,
        &api::WorkingContext::default(),
        api::physical::Action::ReceiptOutcome(api::PhysicalReceiptLookup {
            revision: base,
            plan: plan.id.ok_or(Error::Missing)?,
        }),
    )?;
    assert_eq!(recovered.receipt, Some(outcome));
    assert_eq!(
        store.usage()?,
        before,
        "Recovery and exact replay must write nothing"
    );
    let mut different = receipt;
    different.changes = vec![text_field("name", "Different arguments")];
    assert!(matches!(
        submit(
            &store,
            &actor,
            &working,
            api::physical::Action::ReceiveDevice(different)
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A stale unit, invalid kind, or denied creation must roll back the entire combined mutation.
#[test]
fn receiving_refusal_leaves_no_orphan_inventory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let plan = saved(&store, &actor)?;
    let base = store.usage()?.revision;
    let working = api::WorkingContext {
        expected: Some(base),
        ..Default::default()
    };
    let mut value = request(&plan, base)?;
    value.unit = 3;
    let before = store.usage()?;
    assert!(matches!(
        submit(
            &store,
            &actor,
            &working,
            api::physical::Action::ReceiveDevice(value)
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    let mut denied = actor.clone();
    for grant in &mut denied.grants {
        grant.writable = false;
    }
    assert!(matches!(
        submit(
            &store,
            &denied,
            &working,
            api::physical::Action::ReceiveDevice(request(&plan, base)?)
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Expiring bounded result detail closes recovery; restoring an older image cannot
// turn the same previously submitted request into a new device.
#[test]
fn receipt_expiry_and_older_restore_keep_admission_closed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let plan = saved(&store, &actor)?;
    let base = store.usage()?.revision;
    let working = api::WorkingContext {
        expected: Some(base),
        ..Default::default()
    };
    let request = request(&plan, base)?;
    let mut old = Vec::new();
    store.export(&mut old)?;
    let accepted = submit(
        &store,
        &actor,
        &working,
        api::physical::Action::ReceiveDevice(request.clone()),
    )?;
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored = Store::restore(destination.path(), &image, policy(), image.len() as u64)?;
    let before = restored.usage()?;
    assert_eq!(
        submit(
            &restored,
            &actor,
            &working,
            api::physical::Action::ReceiveDevice(request.clone())
        )?
        .receipt,
        accepted.receipt
    );
    assert_eq!(restored.usage()?, before);
    let older_path = tempfile::tempdir()?;
    let older = Store::restore(older_path.path(), &old, policy(), old.len() as u64)?;
    let before = older.usage()?;
    assert!(matches!(
        submit(
            &older,
            &actor,
            &working,
            api::physical::Action::ReceiveDevice(request.clone())
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(older.usage()?, before);
    let mut cursor = None;
    loop {
        let pass = store.cleanup(2000, cursor.as_deref())?;
        cursor = pass.next;
        if cursor.is_none() {
            break;
        }
    }
    let before = store.usage()?;
    assert!(matches!(
        submit(
            &store,
            &actor,
            &api::WorkingContext::default(),
            api::physical::Action::ReceiptOutcome(api::PhysicalReceiptLookup {
                revision: base,
                plan: plan.id.ok_or(Error::Missing)?
            })
        ),
        Err(Error::Expired)
    ));
    assert!(matches!(
        submit(
            &store,
            &actor,
            &working,
            api::physical::Action::ReceiveDevice(request)
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Two different items cannot consume the same reviewed unit, even through concurrent transactions.
#[test]
fn concurrent_receipts_commit_one_identity_and_one_allocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let plan = saved(&store, &actor)?;
    let base = store.usage()?.revision;
    let working = api::WorkingContext {
        expected: Some(base),
        ..Default::default()
    };
    let first = request(&plan, base)?;
    let mut second = first.clone();
    second.changes = vec![text_field("name", "Concurrent other device")];
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            submit(
                &store,
                &actor,
                &working,
                api::physical::Action::ReceiveDevice(first),
            )
        });
        let b = scope.spawn(|| {
            barrier.wait();
            submit(
                &store,
                &actor,
                &working,
                api::physical::Action::ReceiveDevice(second),
            )
        });
        (a.join(), b.join())
    });
    let a = results.0.map_err(|_| "first receiver panicked")?;
    let b = results.1.map_err(|_| "second receiver panicked")?;
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let loser = if a.is_err() { a } else { b };
    assert!(matches!(loser, Err(Error::Conflict)));
    let Reply::Page(devices) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(devices.total, 1);
    let loaded = physical(
        &store,
        &actor,
        api::physical::Action::Load(api::PhysicalPlanKey {
            id: plan.id.ok_or(Error::Missing)?,
            revision: 0,
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    assert_eq!(loaded.materials[0].allocations.len(), 1);
    Ok(())
}

// An actual received item need not fit its proposed location. Its hold survives
// cancellation while installation is unresolved; receipt cannot certify the fit.
#[test]
fn reserved_receipt_preserves_actual_dimensions_and_unresolved_hold() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Reserved receiving rack")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let plan = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Reserved future body".into(),
                materials: vec![api::PhysicalMaterial {
                    name: "Requested server".into(),
                    quantity: 3,
                    kind: K::Device as i32,
                    specifications: vec![
                        equipment_quantity("width", 500_000, "mm", 3),
                        equipment_quantity("height", 2_000_000, "mm", 3),
                        equipment_quantity("depth", 1_000_000, "mm", 3),
                    ],
                    physical: Some(Box::new(api::PhysicalDefinition {
                        basis: "Reviewed supplier specification".into(),
                        clearance: Some(Box::default()),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                steps: vec![api::PhysicalStep {
                    action: Some(api::physical_step::Action::Placement(
                        api::PhysicalPlacement {
                            enclosure: Some(rack.clone()),
                            material: Some(api::PhysicalMaterialUnit {
                                material: 0,
                                unit: 1,
                            }),
                            volume: Some(volume()),
                            item: None,
                        },
                    )),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    let reserved = reserve(&store, &actor, plan)?.plan.ok_or(Error::Missing)?;
    let base = store.usage()?.revision;
    let working = api::WorkingContext {
        roots: vec![rack],
        expected: Some(base),
        ..Default::default()
    };
    let mut request = request(&reserved, base)?;
    request.changes.extend([
        equipment_quantity("height", 3_000_000, "mm", 3),
        equipment_quantity("width", 500_000, "mm", 3),
        equipment_quantity("depth", 1_000_000, "mm", 3),
    ]);
    let outcome = submit(
        &store,
        &actor,
        &working,
        api::physical::Action::ReceiveDevice(request),
    )?
    .receipt
    .ok_or(Error::Missing)?;
    let target = outcome.item.ok_or(Error::Missing)?;
    let mut selected = working.clone();
    selected.expected = Some(store.usage()?.revision);
    let loaded = submit(
        &store,
        &actor,
        &selected,
        api::physical::Action::Load(api::PhysicalPlanKey {
            id: reserved.id.clone().ok_or(Error::Missing)?,
            revision: 0,
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    let before = store.usage()?;
    assert!(matches!(
        submit(
            &store,
            &actor,
            &selected,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: loaded.id.clone().ok_or(Error::Missing)?,
                    revision: loaded.revision
                }),
                completed_steps: vec![0],
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let cancelled = submit(
        &store,
        &actor,
        &selected,
        api::physical::Action::Cancel(api::PhysicalPlanKey {
            id: loaded.id.ok_or(Error::Missing)?,
            revision: loaded.revision,
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    assert_eq!(cancelled.state, api::PhysicalPlanState::Cancelled as i32);
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        2
    );
    assert_eq!(
        store
            .asset(AssetRef::try_from(&target).map_err(|_| Error::Invalid)?)?
            .kind,
        K::Device as i32
    );
    Ok(())
}
