use super::*;
use api::{reply::Result as Reply, request::Command};
use std::collections::BTreeSet;

fn volume() -> api::PhysicalVolume {
    let extent = |length| {
        Some(api::PhysicalExtent {
            start_um: 0,
            length_um: length,
        })
    };
    api::PhysicalVolume {
        x: extent(500_000),
        y: extent(2_000_000),
        z: extent(1_000_000),
    }
}
fn define(target: wire::AssetRef) -> api::PhysicalDefinition {
    api::PhysicalDefinition {
        target: Some(target),
        volume: Some(volume()),
        basis: "Measured mounting envelope".into(),
        ..Default::default()
    }
}
fn equipment_quantity(name: &str, value: i64, unit: &str, scale: u32) -> wire::Field {
    wire::Field {
        name: name.into(),
        value: Some(wire::field::Value::Equipment(wire::EquipmentClaim {
            basis: "Measured equipment specification".into(),
            assertion: Some(wire::equipment_claim::Assertion::Quantity(
                wire::QuantityRange {
                    lower: Some(value),
                    upper: Some(value),
                    unit: unit.into(),
                    scale,
                },
            )),
        })),
    }
}
fn equipment_dimensions(
    store: &Store,
    actor: &Context,
    target: &wire::AssetRef,
) -> Result<(), Error> {
    let asset = store.asset(AssetRef::try_from(target).map_err(|_| Error::Invalid)?)?;
    call(
        store,
        actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(asset.revision),
            kind: asset.kind,
            changes: vec![
                equipment_quantity("width", 500_000, "mm", 3),
                equipment_quantity("height", 2_000_000, "mm", 3),
                equipment_quantity("depth", 1_000_000, "mm", 3),
            ],
            ..Default::default()
        }),
    )?;
    Ok(())
}
fn physical(
    store: &Store,
    actor: &Context,
    action: api::physical::Action,
) -> Result<api::PhysicalView, Error> {
    match call(
        store,
        actor,
        Command::Physical(api::Physical {
            action: Some(action),
        }),
    )? {
        Reply::Physical(value) => Ok(value),
        _ => Err(Error::Corrupt),
    }
}

// Current facts survive reopen; repeated input and read-only inspection do not grow state.
#[test]
fn physical_definition_is_current_bounded_and_durable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Measured rack")?;
    let definition = define(rack.clone());
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(definition.clone()),
        }),
    )?;
    let before = store.usage()?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(definition.clone()),
        }),
    )?;
    let state = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: false,
        }),
    )?;
    assert_eq!(state.definitions, vec![definition.clone()]);
    assert_eq!(store.usage()?, before);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let state = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: false,
        }),
    )?;
    assert_eq!(state.definitions, vec![definition]);
    Ok(())
}

// Removing an unlinked resource must reclaim its current physical facts, not leave an unreachable row.
#[test]
fn resource_removal_reclaims_its_current_physical_profile() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let item = create(&store, &actor, K::Device, "Unlinked chassis")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(item.clone())),
        }),
    )?;
    let before = store.usage()?;
    call(
        &store,
        &actor,
        Command::Bulk(api::Bulk {
            targets: vec![item],
            expected: before.revision,
            remove: true,
            ..Default::default()
        }),
    )?;
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Inventory as usize],
        before.rows[omloom_store::Class::Inventory as usize] - 3,
        "Asset, name and physical profile are one removal lifecycle"
    );
    Ok(())
}

// Pending reservations cannot become unreconcilable through ordinary asset removal.
#[test]
fn pending_physical_reservation_protects_both_spare_and_enclosure_removal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Reserved rack")?;
    let item = create(&store, &actor, K::Device, "Reserved spare")?;
    equipment_dimensions(&store, &actor, &item)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let plan = draft(&store, &actor, item.clone(), rack.clone())?;
    reserve(&store, &actor, plan)?;
    for target in [item, rack] {
        let before = store.usage()?;
        assert!(matches!(
            call(
                &store,
                &actor,
                Command::Bulk(api::Bulk {
                    targets: vec![target],
                    expected: before.revision,
                    remove: true,
                    ..Default::default()
                })
            ),
            Err(Error::Conflict)
        ));
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

// A physical-field denial protects both predicate inputs and mutation, independently of names.
#[test]
fn physical_field_denial_precedes_decoding_and_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Protected rack")?;
    let mut reader = actor.clone();
    reader.grants[0].read = Fields::Selected(vec!["name".into()]);
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &reader,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack.clone()),
                occupants: false,
            })
        ),
        Err(Error::Denied)
    ));
    assert!(matches!(
        physical(
            &store,
            &reader,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(define(rack))
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Physical planning consumes canonical equipment facts rather than requiring a duplicate dimension record.
#[test]
fn canonical_equipment_dimensions_are_the_only_placement_size_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    equipment_dimensions(&store, &actor, &item)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let plan = draft(&store, &actor, item, rack)?;
    reserve(&store, &actor, plan)?;
    Ok(())
}

// Interior usable space cannot replace the part's canonical external size during admission.
#[test]
fn equipment_internal_space_does_not_shrink_its_required_external_envelope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Chassis")?;
    equipment_dimensions(&store, &actor, &item)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let mut interior = define(item.clone());
    let small = api::PhysicalVolume {
        x: Some(api::PhysicalExtent {
            start_um: 0,
            length_um: 1,
        }),
        y: Some(api::PhysicalExtent {
            start_um: 0,
            length_um: 1,
        }),
        z: Some(api::PhysicalExtent {
            start_um: 0,
            length_um: 1,
        }),
    };
    interior.volume = Some(small);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(interior),
        }),
    )?;
    let plan = api::PhysicalPlan {
        name: "Undersized reservation".into(),
        steps: vec![api::PhysicalStep {
            action: Some(api::physical_step::Action::Placement(
                api::PhysicalPlacement {
                    item: Some(item),
                    enclosure: Some(rack),
                    volume: Some(small),
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let saved = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
    )?
    .plan
    .ok_or("saved plan")?;
    let before = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, saved),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A selected-resource inspection neither expands installed children nor bypasses equipment-field grants.
#[test]
fn physical_fact_projection_stays_shallow_and_filters_equipment_before_decoding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    equipment_dimensions(&store, &actor, &item)?;
    let plan = draft(&store, &actor, item.clone(), rack.clone())?;
    report_steps(&store, &actor, &plan, vec![0])?;
    let before = store.usage()?;
    let facts = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: false,
        }),
    )?;
    assert!(facts.occupancy.is_empty());
    assert!(facts.items.is_empty());
    let occupants = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert_eq!(occupants.occupancy.len(), 1);
    let all = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(item.clone()),
            occupants: false,
        }),
    )?;
    assert_eq!(
        all.items[0]
            .fields
            .iter()
            .filter(|field| matches!(field.name.as_str(), "width" | "height" | "depth"))
            .count(),
        3
    );
    let mut restricted = actor.clone();
    restricted.grants[0].read =
        Fields::Selected(vec!["physical".into(), "name".into(), "height".into()]);
    let selected = physical(
        &store,
        &restricted,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(item),
            occupants: false,
        }),
    )?;
    let names = selected.items[0]
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, BTreeSet::from(["height", "name"]));
    assert_eq!(
        store.usage()?,
        before,
        "Facts and projections must not write access metadata"
    );
    Ok(())
}

// Draft authoring requires the actual write grant; a preview alone grants no saved plan.
#[test]
fn read_only_actor_cannot_save_physical_draft() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    let mut reader = actor.clone();
    reader.grants[0].writable = false;
    let plan = api::PhysicalPlan {
        name: "Proposed placement".into(),
        steps: vec![api::PhysicalStep {
            action: Some(api::physical_step::Action::Placement(
                api::PhysicalPlacement {
                    item: Some(item),
                    enclosure: Some(rack),
                    volume: Some(volume()),
                },
            )),
            ..Default::default()
        }],
        ..Default::default()
    };
    let before = store.usage()?;
    physical(
        &store,
        &reader,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(plan.clone()),
        }),
    )?;
    assert!(matches!(
        physical(
            &store,
            &reader,
            api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn draft(
    store: &Store,
    actor: &Context,
    item: wire::AssetRef,
    rack: wire::AssetRef,
) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Install server".into(),
                steps: vec![api::PhysicalStep {
                    action: Some(api::physical_step::Action::Placement(
                        api::PhysicalPlacement {
                            item: Some(item),
                            enclosure: Some(rack),
                            volume: Some(volume()),
                        },
                    )),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)
}
fn reserve(
    store: &Store,
    actor: &Context,
    plan: api::PhysicalPlan,
) -> Result<api::PhysicalView, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Reserve(api::PhysicalReserve {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            until_ms: 1000,
        }),
    )
}

// A serialized spare cannot be reserved in two distinct enclosures, even with free geometry.
#[test]
fn serialized_item_reservation_is_exclusive_across_enclosures() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::Rack, "Rack A")?;
    let b = create(&store, &actor, K::Rack, "Rack B")?;
    for target in [&a, &b] {
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(define(target.clone())),
            }),
        )?;
    }
    let item = create(&store, &actor, K::Device, "One serialized spare")?;
    equipment_dimensions(&store, &actor, &item)?;
    let first = draft(&store, &actor, item.clone(), a)?;
    let second = draft(&store, &actor, item, b)?;
    reserve(&store, &actor, first)?;
    let before = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, second),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Manual completion couples occupancy and the existing containment relation in one transaction.
#[test]
fn completion_preserves_actual_occupancy_and_reuses_inventory_containment() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    equipment_dimensions(&store, &actor, &item)?;
    let plan = draft(&store, &actor, item.clone(), rack.clone())?;
    let reserved = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let done = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: reserved.id.ok_or(Error::Corrupt)?,
                revision: reserved.revision,
            }),
            completed_steps: vec![0],
            discrepancy: None,
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(done.state, api::PhysicalPlanState::Completed as i32);
    let before = store.usage()?;
    let view = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: true,
        }),
    )?;
    assert_eq!(view.occupancy.len(), 1);
    assert_eq!(
        view.occupancy[0]
            .placement
            .as_ref()
            .and_then(|p| p.item.as_ref()),
        Some(&item)
    );
    assert_eq!(store.usage()?, before);
    let relationships = store.relations(AssetRef::try_from(&item)?, 16)?;
    assert!(
        relationships
            .iter()
            .any(|r| r.kind == wire::RelationKind::InstalledIn as i32
                && r.to.as_ref() == Some(&rack))
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let view = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert_eq!(view.occupancy.len(), 1);
    // Expiring terminal plan details cannot free the authoritative installed footprint.
    assert!(store.cleanup(5000, None)?.removed > 0);
    let view = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: view.occupancy[0]
                .placement
                .as_ref()
                .and_then(|p| p.enclosure.clone()),
            occupants: true,
        }),
    )?;
    assert_eq!(view.occupancy.len(), 1);
    Ok(())
}

// Saved-plan discovery is owner-scoped and does not hydrate resources or write metadata.
#[test]
fn saved_plan_list_is_personal_bounded_and_read_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Equipment")?;
    let plan = draft(&store, &actor, item, rack)?;
    let before = store.usage()?;
    let list = physical(
        &store,
        &actor,
        api::physical::Action::List(api::PhysicalPlanList {
            limit: 1,
            after: None,
        }),
    )?;
    assert_eq!(list.plans.len(), 1);
    assert_eq!(Some(&list.plans[0].id), plan.id.as_ref());
    assert!(list.plan.is_none());
    assert!(list.occupancy.is_empty());
    let mut other = actor.clone();
    other.actor = "another-operator".into();
    assert!(
        physical(
            &store,
            &other,
            api::physical::Action::List(api::PhysicalPlanList::default())
        )?
        .plans
        .is_empty()
    );
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::List(api::PhysicalPlanList {
                limit: 26,
                after: None
            })
        ),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Geometry cannot admit a placement whose sourced aggregate mass exceeds its enclosure limit.
#[test]
fn reservation_enforces_declared_mass_power_and_cooling_limits() -> TestResult {
    for axis in 0..3 {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let rack = create(&store, &actor, K::Rack, "Capacity-limited rack")?;
        let item = create(&store, &actor, K::Device, "Demanding equipment")?;
        equipment_dimensions(&store, &actor, &item)?;
        let mut enclosure = define(rack.clone());
        let mut equipment = api::PhysicalDefinition {
            target: Some(item.clone()),
            basis: "Declared design demand".into(),
            ..Default::default()
        };
        match axis {
            0 => {
                enclosure.max_mass_g = Some(1000);
                let asset = store.asset(AssetRef::try_from(&item)?)?;
                call(
                    &store,
                    &actor,
                    Command::Write(api::Write {
                        target: Some(item.clone()),
                        kind: asset.kind,
                        expected: Some(asset.revision),
                        changes: vec![equipment_quantity("weight", 2000, "g", 0)],
                        ..Default::default()
                    }),
                )?;
            }
            1 => {
                enclosure.capacity_mw = Some(1000);
                equipment.demand_mw = Some(2000);
            }
            _ => {
                enclosure.cooling_capacity_mw = Some(1000);
                equipment.cooling_mw = Some(2000);
            }
        }
        for definition in [enclosure, equipment] {
            physical(
                &store,
                &actor,
                api::physical::Action::Define(api::PhysicalDefinitionEdit {
                    definition: Some(definition),
                }),
            )?;
        }
        let plan = draft(&store, &actor, item, rack)?;
        let before = store.usage()?;
        assert!(
            matches!(reserve(&store, &actor, plan), Err(Error::Conflict)),
            "Capacity axis {axis} must reject admission"
        );
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

// Structural keys identify positions only inside their actual owner's incarnation, not occupants.
#[test]
fn structural_positions_are_scoped_current_and_not_populated_implicitly() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::Device, "First chassis")?;
    let second = create(&store, &actor, K::Device, "Second chassis")?;
    let edit = api::PhysicalPositionEdit {
        owner: Some(first.clone()),
        structural_key: "psu/1".into(),
        name: "PSU bay 1".into(),
        component_kind: api::PhysicalComponentKind::PowerSupply as i32,
        expected: None,
    };
    let created = physical(
        &store,
        &actor,
        api::physical::Action::Position(edit.clone()),
    )?;
    let position = &created.positions[0];
    let before = store.usage()?;
    let mut again = edit.clone();
    again.expected = Some(position.revision);
    assert_eq!(
        physical(&store, &actor, api::physical::Action::Position(again))?.positions,
        created.positions
    );
    assert_eq!(store.usage()?, before);
    let other = physical(
        &store,
        &actor,
        api::physical::Action::Position(api::PhysicalPositionEdit {
            owner: Some(second),
            ..edit
        }),
    )?;
    assert_ne!(other.positions[0].target, position.target);
    let list = physical(
        &store,
        &actor,
        api::physical::Action::Positions(api::PhysicalPositionQuery {
            owner: Some(first),
            after: None,
            limit: 25,
        }),
    )?;
    assert_eq!(list.positions, created.positions);
    assert_eq!(
        list.items
            .iter()
            .filter(|i| i.target == position.target)
            .count(),
        1
    );
    let read = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: position.target.clone(),
            occupants: true,
        }),
    )?;
    assert!(read.occupancy.is_empty());
    assert!(matches!(
        create(&store, &actor, K::PhysicalPosition, "Unscoped invented bay"),
        Err(Error::Invalid)
    ));
    Ok(())
}

// The descriptor owns a position's accepted class; a physical profile cannot introduce another value.
#[test]
fn structural_position_class_has_one_owner() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Chassis")?;
    let position = position_fixture(
        &store,
        &actor,
        owner,
        "optic/1",
        api::PhysicalComponentKind::Transceiver,
    )?;
    let mut profile = define(position);
    profile.component_kind = api::PhysicalComponentKind::Disk as i32;
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(profile)
            })
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A draft cannot reserve an attachment the canonical inventory relation rules would reject later.
#[test]
fn physical_draft_reuses_inventory_endpoint_compatibility() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    for (from, to) in [
        (K::Service, K::Rack),
        (K::Device, K::Interface),
        (K::Port, K::Rack),
    ] {
        let item = create(&store, &actor, from, "Invalid placed resource")?;
        let enclosure = create(&store, &actor, to, "Invalid enclosure")?;
        let before = store.usage()?;
        assert!(matches!(
            draft(&store, &actor, item, enclosure),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

// A hardware realization is a peer binding, not the owner of OS interface/configuration intent.
#[test]
fn physical_carriage_does_not_create_logical_endpoint_ownership() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let port = create(&store, &actor, K::Port, "Current physical connector")?;
    let interface = create(&store, &actor, K::Interface, "Surviving logical interface")?;
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(port.clone()),
                to: Some(interface),
                kind: wire::RelationKind::Carries as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    let Reply::Page(page) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            view: api::View::Network as i32,
            focus: Some(port),
            level_only: true,
            columns: vec!["name".into()],
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("expected page".into());
    };
    assert_eq!(
        page.total, 0,
        "Removing a hardware realization must not remove or scope-own logical configuration"
    );
    Ok(())
}

// Dedicated management/console endpoints can explicitly belong to a surviving physical port.
#[test]
fn explicit_port_containment_owns_a_dedicated_logical_endpoint() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let port = create(&store, &actor, K::Port, "Dedicated console port")?;
    let interface = create(&store, &actor, K::Interface, "Port-bound console channel")?;
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(port.clone()),
                to: Some(interface.clone()),
                kind: wire::RelationKind::Contains as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    let Reply::Page(page) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            view: api::View::Network as i32,
            focus: Some(port),
            level_only: true,
            columns: vec!["name".into()],
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("expected page".into());
    };
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.target.clone())
            .collect::<Vec<_>>(),
        vec![Some(interface)]
    );
    Ok(())
}

fn replacement_plan(
    store: &Store,
    actor: &Context,
    replacements: Vec<api::PhysicalReplacement>,
) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Reviewed physical transition".into(),
                steps: replacements
                    .into_iter()
                    .map(|r| api::PhysicalStep {
                        action: Some(api::physical_step::Action::Replacement(r)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)
}
fn report_steps(
    store: &Store,
    actor: &Context,
    plan: &api::PhysicalPlan,
    indexes: Vec<u32>,
) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.clone().ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            completed_steps: indexes,
            actual_state: true,
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)
}
fn position_fixture(
    store: &Store,
    actor: &Context,
    owner: wire::AssetRef,
    key: &str,
    kind: api::PhysicalComponentKind,
) -> Result<wire::AssetRef, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Position(api::PhysicalPositionEdit {
            owner: Some(owner),
            structural_key: key.into(),
            name: key.into(),
            component_kind: kind as i32,
            expected: None,
        }),
    )?
    .positions
    .into_iter()
    .next()
    .and_then(|p| p.target)
    .ok_or(Error::Corrupt)
}
fn binding(store: &Store, item: &wire::AssetRef) -> Result<wire::Relation, Error> {
    store
        .relations(AssetRef::try_from(item).map_err(|_| Error::Invalid)?, 32)?
        .into_iter()
        .find(|r| r.kind == wire::RelationKind::InstalledIn as i32)
        .ok_or(Error::Corrupt)
}
fn install_component(
    store: &Store,
    actor: &Context,
    position: wire::AssetRef,
    item: wire::AssetRef,
) -> Result<api::PhysicalPlan, Error> {
    let plan = replacement_plan(
        store,
        actor,
        vec![api::PhysicalReplacement {
            position: Some(position),
            replacement: Some(item),
            ..Default::default()
        }],
    )?;
    report_steps(store, actor, &plan, vec![0])
}

// One typed transition preserves position identity across optics, disks, PSUs, NICs and line cards.
#[test]
fn component_replacement_preserves_position_and_does_not_merge_part_identity() -> TestResult {
    for kind in [
        api::PhysicalComponentKind::Transceiver,
        api::PhysicalComponentKind::Disk,
        api::PhysicalComponentKind::PowerSupply,
        api::PhysicalComponentKind::NetworkAdapter,
        api::PhysicalComponentKind::LineCard,
    ] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let owner = create(&store, &actor, K::Device, "Owning chassis")?;
        let position = position_fixture(&store, &actor, owner, "slot/1", kind)?;
        let old = create(
            &store,
            &actor,
            K::PhysicalComponent,
            "Old independently identified part",
        )?;
        let new = create(
            &store,
            &actor,
            K::PhysicalComponent,
            "New independently identified part",
        )?;
        install_component(&store, &actor, position.clone(), old.clone())?;
        let previous = binding(&store, &old)?;
        let plan = replacement_plan(
            &store,
            &actor,
            vec![api::PhysicalReplacement {
                position: Some(position.clone()),
                previous: Some(old.clone()),
                replacement: Some(new.clone()),
                previous_binding: Some(previous.relation_id),
                previous_revision: Some(previous.revision),
                ..Default::default()
            }],
        )?;
        let done = report_steps(&store, &actor, &plan, vec![0])?;
        assert_eq!(done.state, api::PhysicalPlanState::Completed as i32);
        assert_eq!(binding(&store, &new)?.to, Some(position));
        assert!(matches!(binding(&store, &old), Err(Error::Corrupt)));
        assert_eq!(
            store.asset(AssetRef::try_from(&old)?)?.target,
            Some(old.clone()),
            "Removal does not dispose of the old adopted spare"
        );
        assert_ne!(old, new);
    }
    Ok(())
}

// An atomic swap detaches both old bindings before realizing either new one; partial swaps conflict.
#[test]
fn two_known_parts_swap_positions_without_changing_their_identities() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Modular chassis")?;
    let a = position_fixture(
        &store,
        &actor,
        owner.clone(),
        "optic/1",
        api::PhysicalComponentKind::Transceiver,
    )?;
    let b = position_fixture(
        &store,
        &actor,
        owner,
        "optic/2",
        api::PhysicalComponentKind::Transceiver,
    )?;
    let first = create(&store, &actor, K::PhysicalComponent, "First optic")?;
    let second = create(&store, &actor, K::PhysicalComponent, "Second optic")?;
    install_component(&store, &actor, a.clone(), first.clone())?;
    install_component(&store, &actor, b.clone(), second.clone())?;
    let mut changes = Vec::new();
    for (item, source, destination) in [(&first, &a, &b), (&second, &b, &a)] {
        let old = binding(&store, item)?;
        changes.push(api::PhysicalReplacement {
            position: Some(destination.clone()),
            source_position: Some(source.clone()),
            previous: Some(item.clone()),
            replacement: Some(item.clone()),
            previous_binding: Some(old.relation_id),
            previous_revision: Some(old.revision),
        });
    }
    let plan = replacement_plan(&store, &actor, changes)?;
    let before = store.usage()?;
    assert!(matches!(
        report_steps(&store, &actor, &plan, vec![0]),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    report_steps(&store, &actor, &plan, vec![0, 1])?;
    assert_eq!(binding(&store, &first)?.to, Some(b));
    assert_eq!(binding(&store, &second)?.to, Some(a));
    Ok(())
}

// Removing the owning binding also removes its pose; no parallel occupancy can survive it.
#[test]
fn generic_relation_removal_removes_current_placement_and_pose() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    let plan = draft(&store, &actor, item.clone(), rack.clone())?;
    report_steps(&store, &actor, &plan, vec![0])?;
    let before = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: true,
        }),
    )?;
    assert!(
        before.occupancy[0]
            .placement
            .as_ref()
            .ok_or("placement")?
            .volume
            .is_some()
    );
    let current = binding(&store, &item)?;
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(current),
            remove: true,
        }),
    )?;
    let after = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert!(after.occupancy.is_empty());
    assert!(matches!(binding(&store, &item), Err(Error::Corrupt)));
    Ok(())
}

// A changed binding preserves component identity but cannot carry coordinates from its old enclosure.
#[test]
fn generic_relation_rebinding_invalidates_old_coordinates() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Original rack")?;
    let other = create(&store, &actor, K::Rack, "Destination rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    let plan = draft(&store, &actor, item.clone(), rack.clone())?;
    report_steps(&store, &actor, &plan, vec![0])?;
    let mut current = binding(&store, &item)?;
    let id = current.relation_id.clone();
    current.to = Some(other.clone());
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(current),
            remove: false,
        }),
    )?;
    let old = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert!(old.occupancy.is_empty());
    let destination = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(other.clone()),
            occupants: true,
        }),
    )?;
    assert_eq!(destination.occupancy.len(), 1);
    let placement = destination.occupancy[0]
        .placement
        .as_ref()
        .ok_or("placement")?;
    assert_eq!(placement.item, Some(item.clone()));
    assert!(
        placement.volume.is_none(),
        "Unknown coordinates do not imply an empty enclosure"
    );
    assert_eq!(binding(&store, &item)?.relation_id, id);
    assert_eq!(binding(&store, &item)?.to, Some(other));
    Ok(())
}

// Reservations read their immutable plan step; resolving one frees only that future region.
#[test]
fn reservation_region_tracks_the_owning_step_until_explicit_resolution() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Shared rack")?;
    let first = create(&store, &actor, K::Device, "First server")?;
    let second = create(&store, &actor, K::Device, "Second server")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    equipment_dimensions(&store, &actor, &first)?;
    equipment_dimensions(&store, &actor, &second)?;
    let first = draft(&store, &actor, first, rack.clone())?;
    let second = draft(&store, &actor, second, rack.clone())?;
    let first = reserve(&store, &actor, first)?
        .plan
        .ok_or("first reserved plan")?;
    let before = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, second.clone()),
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.usage()?,
        before,
        "A rejected overlap cannot leave another hold"
    );
    physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: first.id.ok_or("plan id")?,
                revision: first.revision,
            }),
            not_performed_steps: vec![0],
            ..Default::default()
        }),
    )?;
    let second = reserve(&store, &actor, second)?
        .plan
        .ok_or("second reserved plan")?;
    report_steps(&store, &actor, &second, vec![0])?;
    let current = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert_eq!(current.occupancy.len(), 1);
    Ok(())
}

// Renaming a structural position must not erase an independently reported empty state.
#[test]
fn position_rename_preserves_reported_empty_state() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Chassis")?;
    let position = position_fixture(
        &store,
        &actor,
        owner.clone(),
        "disk/1",
        api::PhysicalComponentKind::Disk,
    )?;
    let old = create(&store, &actor, K::PhysicalComponent, "Removed drive")?;
    let incoming = create(&store, &actor, K::PhysicalComponent, "Spare drive")?;
    install_component(&store, &actor, position.clone(), old.clone())?;
    let previous = binding(&store, &old)?;
    let removal = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            previous: Some(old),
            previous_binding: Some(previous.relation_id),
            previous_revision: Some(previous.revision),
            ..Default::default()
        }],
    )?;
    report_steps(&store, &actor, &removal, vec![0])?;
    let positions = physical(
        &store,
        &actor,
        api::physical::Action::Positions(api::PhysicalPositionQuery {
            owner: Some(owner.clone()),
            limit: 1,
            ..Default::default()
        }),
    )?;
    physical(
        &store,
        &actor,
        api::physical::Action::Position(api::PhysicalPositionEdit {
            owner: Some(owner),
            structural_key: "disk/1".into(),
            name: "Boot drive bay".into(),
            component_kind: api::PhysicalComponentKind::Disk as i32,
            expected: Some(positions.positions[0].revision),
        }),
    )?;
    equipment_dimensions(&store, &actor, &incoming)?;
    for target in [&position, &incoming] {
        let mut definition = define(target.clone());
        if target == &incoming {
            definition.component_kind = api::PhysicalComponentKind::Disk as i32;
        }
        definition.mounting = Some("Declared disk bay standard".into());
        if target == &incoming {
            definition.volume = None;
        }
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(definition),
            }),
        )?;
    }
    let installation = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position),
            replacement: Some(incoming),
            ..Default::default()
        }],
    )?;
    reserve(&store, &actor, installation)?;
    Ok(())
}

// A slot's structural identity cannot migrate to a different chassis through an ordinary edge edit.
#[test]
fn generic_relation_cannot_reparent_a_structural_position() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Original chassis")?;
    let other = create(&store, &actor, K::Device, "Other chassis")?;
    let position = position_fixture(
        &store,
        &actor,
        owner,
        "slot/1",
        api::PhysicalComponentKind::LineCard,
    )?;
    let mut parent = store
        .relations(AssetRef::try_from(&position)?, 32)?
        .into_iter()
        .find(|r| r.kind == wire::RelationKind::Contains as i32)
        .ok_or("parent binding")?;
    parent.from = Some(other);
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Relation(api::Relationship {
                relation: Some(parent),
                remove: false
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Re-saving an unchanged accepted draft must not create a new revision or audit record.
#[test]
fn unchanged_saved_draft_is_a_noop() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    let item = create(&store, &actor, K::Device, "Server")?;
    let plan = draft(&store, &actor, item, rack)?;
    let before = store.usage()?;
    let unchanged = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(plan.clone()),
        }),
    )?;
    assert!(
        !unchanged.items.is_empty(),
        "A no-op save retains the same permitted subject labels as a changed save"
    );
    let same = unchanged.plan.ok_or(Error::Corrupt)?;
    assert_eq!(same, plan);
    assert_eq!(store.usage()?, before);
    Ok(())
}
