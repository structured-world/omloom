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
        changes: vec![text("name", "Unpacked node")],
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
        )?
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
    assert_eq!(
        asset.fields.len(),
        1,
        "Planned characteristics and unknown serial are not actual facts"
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
            changes: vec![text("name", "Later corrected name")],
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
        api::physical::Action::ReceiptOutcome(api::PhysicalReceiptLookup { revision: base }),
    )?;
    assert_eq!(recovered.receipt, Some(outcome));
    assert_eq!(
        store.usage()?,
        before,
        "Recovery and exact replay must write nothing"
    );
    let mut different = receipt;
    different.changes = vec![text("name", "Different arguments")];
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
