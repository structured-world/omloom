use super::*;

fn demand() -> api::PhysicalMaterial {
    api::PhysicalMaterial {
        name: "Required compute nodes".into(),
        quantity: u32::MAX,
        kind: K::Device as i32,
        specifications: vec![equipment_quantity("width", 500_000, "mm", 3)],
        physical: Some(Box::new(api::PhysicalDefinition {
            basis: "Supplier mounting specification".into(),
            mounting: Some("19-inch rails".into()),
            ..Default::default()
        })),
        ..Default::default()
    }
}
fn save_material_plan(
    store: &Store,
    actor: &Context,
    plan: api::PhysicalPlan,
) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
    )?
    .plan
    .ok_or(Error::Missing)
}

// Late partial receipt uses the same serialized locks; cancellation and expiry do not release unresolved work.
#[test]
fn received_materials_share_reservation_and_resolution_lifecycle() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Receiving rack")?;
    let item = create(&store, &actor, K::Device, "Planned node")?;
    let spare = create(&store, &actor, K::Device, "Late received node")?;
    equipment_dimensions(&store, &actor, &item)?;
    equipment_dimensions(&store, &actor, &spare)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let mut plan = draft(&store, &actor, item.clone(), rack.clone())?;
    let mut line = demand();
    line.quantity = 3;
    line.allocations.push(api::PhysicalMaterialAllocation {
        unit: 0,
        item: Some(item.clone()),
        basis: "Selected node for the installation step".into(),
    });
    plan.materials.push(line);
    let plan = save_material_plan(&store, &actor, plan)?;
    let baseline = store.usage()?.rows[omloom_store::Class::Pending as usize];
    let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Missing)?;
    let request = api::PhysicalMaterialReceipt {
        plan: Some(api::PhysicalPlanKey {
            id: plan.id.clone().ok_or(Error::Missing)?,
            revision: plan.revision,
        }),
        material: 0,
        allocation: Some(api::PhysicalMaterialAllocation {
            unit: 2,
            item: Some(spare.clone()),
            basis: "Late physical receipt".into(),
        }),
    };
    let received = physical(
        &store,
        &actor,
        api::physical::Action::Receive(request.clone()),
    )?
    .plan
    .ok_or(Error::Missing)?;
    assert_eq!(received.state, api::PhysicalPlanState::Reserved as i32);
    assert_eq!(received.materials[0].allocations.len(), 2);
    let usage = store.usage()?;
    let mut repeated = request;
    repeated.plan.as_mut().ok_or(Error::Missing)?.revision = received.revision;
    assert_eq!(
        physical(&store, &actor, api::physical::Action::Receive(repeated))?.plan,
        Some(received.clone())
    );
    assert_eq!(store.usage()?, usage);
    let substitution = api::PhysicalMaterialReceipt {
        plan: Some(api::PhysicalPlanKey {
            id: received.id.clone().ok_or(Error::Missing)?,
            revision: received.revision,
        }),
        material: 0,
        allocation: Some(api::PhysicalMaterialAllocation {
            unit: 0,
            item: Some(spare.clone()),
            basis: "Different resource would change the accepted step".into(),
        }),
    };
    assert!(matches!(
        physical(&store, &actor, api::physical::Action::Receive(substitution)),
        Err(Error::UnsafeTransition)
    ));
    assert_eq!(store.usage()?, usage);
    let other = draft(&store, &actor, spare, rack)?;
    assert!(matches!(
        reserve(&store, &actor, other),
        Err(Error::Conflict)
    ));
    let cancelled = physical(
        &store,
        &actor,
        api::physical::Action::Cancel(api::PhysicalPlanKey {
            id: received.id.clone().ok_or(Error::Missing)?,
            revision: received.revision,
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    // Only the unresolved installation's item and geometry remain held after cancellation.
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        baseline + 2
    );
    let done = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: cancelled.id.ok_or(Error::Missing)?,
                revision: cancelled.revision,
            }),
            not_performed_steps: vec![0],
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    assert_eq!(done.materials[0].allocations.len(), 2);
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        baseline
    );
    Ok(())
}

// Unreceived quantities are scalar intent, not stock rows or adopted inventory. Reloads and equal saves do not append history.
#[test]
fn future_materials_are_sparse_plan_local_and_durable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let original = api::PhysicalPlan {
        name: "Receiving before installation".into(),
        materials: vec![demand()],
        ..Default::default()
    };
    let plan = save_material_plan(&store, &actor, original)?;
    assert!(plan.steps.is_empty());
    assert_eq!(plan.materials[0].quantity, u32::MAX);
    assert!(plan.materials[0].allocations.is_empty());
    let usage = store.usage()?;
    assert_eq!(
        physical(
            &store,
            &actor,
            api::physical::Action::Load(api::PhysicalPlanKey {
                id: plan.id.clone().ok_or(Error::Missing)?,
                revision: plan.revision
            })
        )?
        .plan,
        Some(plan.clone())
    );
    let saved = save_material_plan(&store, &actor, plan.clone())?;
    assert_eq!(saved, plan);
    assert_eq!(store.usage()?, usage);
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(plan.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Unknown as i32);
    assert_eq!(store.usage()?, usage);
    let key = api::PhysicalPlanKey {
        id: plan.id.clone().ok_or(Error::Missing)?,
        revision: plan.revision,
    };
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Reserve(api::PhysicalReserve {
                plan: Some(key.clone()),
                until_ms: 100_000
            })
        ),
        Err(Error::Incomplete)
    ));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        physical(&store, &actor, api::physical::Action::Load(key))?.plan,
        Some(plan)
    );
    assert_eq!(store.usage()?, usage);
    Ok(())
}

// Receiving links an explicitly adopted unit without installing it; replacing a sparse allocation does not retain its previous payload.
#[test]
fn material_receiving_tracks_only_real_partial_allocations() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let spare = create(&store, &actor, K::Device, "Received serialized node")?;
    let mut line = demand();
    line.allocations.push(api::PhysicalMaterialAllocation {
        unit: u32::MAX - 1,
        item: Some(spare.clone()),
        basis: "Received and inspected unit".into(),
    });
    let mut plan = save_material_plan(
        &store,
        &actor,
        api::PhysicalPlan {
            name: "Partial receiving".into(),
            materials: vec![line],
            ..Default::default()
        },
    )?;
    assert_eq!(plan.materials[0].allocations.len(), 1);
    assert!(
        store
            .relations(AssetRef::try_from(&spare).map_err(|_| Error::Invalid)?, 32)?
            .is_empty()
    );
    let usage = store.usage()?;
    let mut invalid = plan.clone();
    let duplicate = invalid.materials[0].allocations[0].clone();
    invalid.materials[0].allocations.push(duplicate);
    assert!(matches!(
        save_material_plan(&store, &actor, invalid),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, usage);
    plan.materials[0].allocations.clear();
    let plan = save_material_plan(&store, &actor, plan)?;
    assert!(plan.materials[0].allocations.is_empty());
    assert_eq!(
        store
            .asset(AssetRef::try_from(&spare).map_err(|_| Error::Invalid)?)?
            .adoption,
        wire::Adoption::Adopted as i32
    );
    Ok(())
}

// Future declarations cannot claim present occupancy, complete observed sources, arbitrary serial payloads or invalid supply quantities.
#[test]
fn material_claims_validate_before_persistence() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let usage = store.usage()?;
    for variant in 0..4 {
        let mut line = demand();
        match variant {
            0 => line.quantity = 0,
            1 => line.specifications = vec![text_field("serial", "Unreceived serial")],
            2 => {
                line.physical
                    .as_mut()
                    .ok_or(Error::Missing)?
                    .power_sources_complete = true
            }
            3 => {
                line.physical
                    .as_mut()
                    .ok_or(Error::Missing)?
                    .occupancy_coverage = Some(Box::new(volume()))
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            save_material_plan(
                &store,
                &actor,
                api::PhysicalPlan {
                    name: "Invalid future assertion".into(),
                    materials: vec![line],
                    ..Default::default()
                }
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, usage);
    }
    Ok(())
}

// Terminal BOM detail shares the existing finite cleanup lifecycle instead of leaving unbounded material rows.
#[test]
fn resolved_material_details_are_reclaimed_by_common_cleanup() -> TestResult {
    let dir = tempfile::tempdir()?;
    let policy = policy();
    let expiry = 101 + policy.detail_age_ms;
    let store = Store::open(dir.path(), policy)?;
    let actor = operator();
    let baseline = store.usage()?.rows[omloom_store::Class::Inventory as usize];
    let plan = save_material_plan(
        &store,
        &actor,
        api::PhysicalPlan {
            name: "Finished receiving study".into(),
            materials: vec![demand()],
            ..Default::default()
        },
    )?;
    let key = api::PhysicalPlanKey {
        id: plan.id.ok_or(Error::Missing)?,
        revision: plan.revision,
    };
    physical(&store, &actor, api::physical::Action::Cancel(key.clone()))?;
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Inventory as usize],
        baseline
    );
    let mut cursor = None;
    let mut completed = false;
    for _ in 0..32 {
        let progress = store.cleanup(expiry, cursor.as_deref())?;
        cursor = progress.cursor;
        if cursor.is_none() {
            completed = true;
            break;
        }
    }
    assert!(completed);
    assert_eq!(store.usage()?.rows[omloom_store::Class::Detail as usize], 0);
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Load(api::PhysicalPlanKey { revision: 0, ..key })
        ),
        Err(Error::Missing)
    ));
    Ok(())
}

// Plan material inspection uses current serial facts and never discloses a denied field or writes a snapshot.
#[test]
fn plan_material_serials_follow_current_field_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Materials rack")?;
    let item = create(&store, &actor, K::Device, "Selected serialized server")?;
    let current = store.asset(AssetRef::try_from(&item).map_err(|_| Error::Invalid)?)?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(item.clone()),
            expected: Some(current.revision),
            kind: current.kind,
            changes: vec![text_field("serial", "manufacturer-serial-1")],
            ..Default::default()
        }),
    )?;
    let plan = draft(&store, &actor, item.clone(), rack)?;
    let load = api::PhysicalPlanKey {
        id: plan.id.ok_or(Error::Corrupt)?,
        revision: plan.revision,
    };
    let usage = store.usage()?;
    let view = physical(&store, &actor, api::physical::Action::Load(load.clone()))?;
    let material = view
        .items
        .iter()
        .find(|i| i.target.as_ref() == Some(&item))
        .ok_or(Error::Missing)?;
    assert!(
        material
            .fields
            .contains(&text_field("serial", "manufacturer-serial-1"))
    );
    assert_eq!(store.usage()?, usage);
    let mut restricted = actor;
    for grant in &mut restricted.grants {
        grant.read = Fields::Selected(vec!["name".into(), "physical".into(), "relations".into()]);
    }
    let view = physical(&store, &restricted, api::physical::Action::Load(load))?;
    assert!(
        view.items
            .iter()
            .all(|i| i.fields.iter().all(|f| f.name != "serial"))
    );
    assert_eq!(store.usage()?, usage);
    Ok(())
}
