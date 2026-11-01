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

// Unreceived units reserve real geometry without fabricated assets; later receipt supplies identity to the same step.
#[test]
fn future_unit_placement_reserves_before_receipt_and_records_only_real_inventory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Future unit rack")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let mut material = demand();
    material.physical.as_mut().ok_or(Error::Missing)?.mounting = None;
    material.specifications = vec![
        equipment_quantity("width", 500_000, "mm", 3),
        equipment_quantity("height", 2_000_000, "mm", 3),
        equipment_quantity("depth", 1_000_000, "mm", 3),
    ];
    let plan = save_material_plan(
        &store,
        &actor,
        api::PhysicalPlan {
            name: "Receive after layout reservation".into(),
            materials: vec![material],
            steps: vec![api::PhysicalStep {
                action: Some(api::physical_step::Action::Placement(
                    api::PhysicalPlacement {
                        item: None,
                        enclosure: Some(rack.clone()),
                        volume: Some(volume()),
                        material: Some(api::PhysicalMaterialUnit {
                            material: 0,
                            unit: u32::MAX - 1,
                        }),
                    },
                )),
                ..Default::default()
            }],
            ..Default::default()
        },
    )?;
    let reserved = reserve(&store, &actor, plan)?.plan.ok_or(Error::Missing)?;
    let before = store.usage()?;
    let key = api::PhysicalPlanKey {
        id: reserved.id.clone().ok_or(Error::Missing)?,
        revision: reserved.revision,
    };
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(key.clone()),
                completed_steps: vec![0],
                ..Default::default()
            })
        ),
        Err(Error::Incomplete)
    ));
    assert_eq!(store.usage()?, before);
    let actual = create(&store, &actor, K::Device, "Received actual unit")?;
    equipment_dimensions(&store, &actor, &actual)?;
    let occupied = draft(&store, &actor, actual.clone(), rack.clone())?;
    assert!(matches!(
        reserve(&store, &actor, occupied),
        Err(Error::Conflict)
    ));
    let received = physical(
        &store,
        &actor,
        api::physical::Action::Receive(api::PhysicalMaterialReceipt {
            plan: Some(key),
            material: 0,
            allocation: Some(api::PhysicalMaterialAllocation {
                unit: u32::MAX - 1,
                item: Some(actual.clone()),
                basis: "Received the planned unit".into(),
            }),
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    let done = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: received.id.ok_or(Error::Missing)?,
                revision: received.revision,
            }),
            completed_steps: vec![0],
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Missing)?;
    assert_eq!(done.state, api::PhysicalPlanState::Completed as i32);
    let view = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert_eq!(view.occupancy.len(), 1);
    assert_eq!(
        view.occupancy[0]
            .placement
            .as_ref()
            .ok_or(Error::Missing)?
            .item,
        Some(actual)
    );
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        0
    );
    Ok(())
}

// Another plan's unreceived unit consumes its declared load as well as geometry, without creating an inventory identity.
#[test]
fn future_reservations_account_for_other_plans_material_demands() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Future demand capacity")?;
    let mut enclosure = define(rack.clone());
    enclosure.max_mass_g = Some(15);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(enclosure),
        }),
    )?;
    let mut material = demand();
    material.quantity = 1;
    material.physical = None;
    material.specifications = vec![
        equipment_quantity("width", 500_000, "mm", 3),
        equipment_quantity("height", 1_000_000, "mm", 3),
        equipment_quantity("depth", 1_000_000, "mm", 3),
        equipment_quantity("weight", 10, "g", 0),
    ];
    let mut lower = volume();
    lower.y.as_mut().ok_or(Error::Missing)?.length_um = 1_000_000;
    let mut plan = api::PhysicalPlan {
        name: "First future load".into(),
        materials: vec![material],
        steps: vec![api::PhysicalStep {
            action: Some(api::physical_step::Action::Placement(
                api::PhysicalPlacement {
                    item: None,
                    enclosure: Some(rack.clone()),
                    volume: Some(lower),
                    material: Some(api::PhysicalMaterialUnit {
                        material: 0,
                        unit: 0,
                    }),
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let first = save_material_plan(&store, &actor, plan.clone())?;
    reserve(&store, &actor, first)?;
    // A disjoint offered region cannot hide the load retained elsewhere in the enclosure.
    let mut reduced = define(rack.clone());
    reduced.max_mass_g = Some(5);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(reduced),
        }),
    )?;
    let mut offered_region = volume();
    offered_region.y.as_mut().ok_or(Error::Missing)?.start_um = 1_000_000;
    offered_region.y.as_mut().ok_or(Error::Missing)?.length_um = 1_000_000;
    let offered = physical(
        &store,
        &actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit {
            offer: Some(api::PhysicalOffer {
                enclosure: Some(rack.clone()),
                region: Some(offered_region),
                recipient: "capacity-reader".into(),
                name: "Disjoint remaining region".into(),
                mass: true,
                ..Default::default()
            }),
        }),
    )?
    .offers
    .remove(0);
    let before = store.usage()?;
    let availability = physical(
        &store,
        &actor,
        api::physical::Action::Availability(api::PhysicalOfferRead {
            id: offered.id.ok_or(Error::Missing)?,
        }),
    )?;
    assert_eq!(availability.state, api::RequirementState::Failed as i32);
    assert_eq!(store.usage()?, before);
    let mut restored = define(rack.clone());
    restored.max_mass_g = Some(15);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(restored),
        }),
    )?;
    plan.name = "Second future load".into();
    let Some(api::physical_step::Action::Placement(placement)) = &mut plan.steps[0].action else {
        return Err(Error::Corrupt.into());
    };
    placement
        .volume
        .as_mut()
        .ok_or(Error::Missing)?
        .y
        .as_mut()
        .ok_or(Error::Missing)?
        .start_um = 1_000_000;
    let overweight = save_material_plan(&store, &actor, plan.clone())?;
    let before = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, overweight),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    plan.materials[0]
        .specifications
        .retain(|f| f.name != "weight");
    plan.materials[0]
        .specifications
        .push(equipment_quantity("weight", 5, "g", 0));
    let exact = save_material_plan(&store, &actor, plan)?;
    assert_eq!(
        reserve(&store, &actor, exact)?
            .plan
            .ok_or(Error::Missing)?
            .state,
        api::PhysicalPlanState::Reserved as i32
    );
    Ok(())
}

// Receiving real hardware replaces the hypothetical load used by other plans' admission.
#[test]
fn received_units_replace_declared_loads_in_other_reservations() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Received load rack")?;
    let mut facts = define(rack.clone());
    facts.max_mass_g = Some(15);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(facts),
        }),
    )?;
    let material = api::PhysicalMaterial {
        name: "Ten gram requested unit".into(),
        quantity: 1,
        kind: K::Device as i32,
        specifications: vec![
            equipment_quantity("width", 500_000, "mm", 3),
            equipment_quantity("height", 1_000_000, "mm", 3),
            equipment_quantity("depth", 1_000_000, "mm", 3),
            equipment_quantity("weight", 10, "g", 0),
        ],
        ..Default::default()
    };
    let mut region = volume();
    region.y.as_mut().ok_or(Error::Missing)?.length_um = 1_000_000;
    let mut proposal = api::PhysicalPlan {
        name: "Reserved requested unit".into(),
        materials: vec![material],
        steps: vec![api::PhysicalStep {
            action: Some(api::physical_step::Action::Placement(
                api::PhysicalPlacement {
                    enclosure: Some(rack.clone()),
                    volume: Some(region),
                    material: Some(api::PhysicalMaterialUnit {
                        material: 0,
                        unit: 0,
                    }),
                    item: None,
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let reserved = reserve(
        &store,
        &actor,
        save_material_plan(&store, &actor, proposal.clone())?,
    )?
    .plan
    .ok_or(Error::Missing)?;
    let actual = create(&store, &actor, K::Device, "Twenty gram received unit")?;
    let asset = store.asset(AssetRef::try_from(&actual).map_err(|_| Error::Invalid)?)?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(actual.clone()),
            kind: K::Device as i32,
            expected: Some(asset.revision),
            changes: vec![
                equipment_quantity("width", 500_000, "mm", 3),
                equipment_quantity("height", 1_000_000, "mm", 3),
                equipment_quantity("depth", 1_000_000, "mm", 3),
                equipment_quantity("weight", 20, "g", 0),
            ],
            ..Default::default()
        }),
    )?;
    physical(
        &store,
        &actor,
        api::physical::Action::Receive(api::PhysicalMaterialReceipt {
            plan: Some(api::PhysicalPlanKey {
                id: reserved.id.ok_or(Error::Missing)?,
                revision: reserved.revision,
            }),
            material: 0,
            allocation: Some(api::PhysicalMaterialAllocation {
                unit: 0,
                item: Some(actual),
                basis: "Actual receipt, heavier than requested".into(),
            }),
        }),
    )?;
    proposal.materials[0]
        .specifications
        .retain(|f| f.name != "weight");
    proposal.materials[0]
        .specifications
        .push(equipment_quantity("weight", 5, "g", 0));
    let Some(api::physical_step::Action::Placement(placement)) = &mut proposal.steps[0].action
    else {
        return Err(Error::Corrupt.into());
    };
    placement
        .volume
        .as_mut()
        .ok_or(Error::Missing)?
        .y
        .as_mut()
        .ok_or(Error::Missing)?
        .start_um = 1_000_000;
    let second = save_material_plan(&store, &actor, proposal)?;
    let before = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, second),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
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
