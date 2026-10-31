use super::*;
use api::{reply::Result as Reply, request::Command};

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
        }),
    )?;
    assert_eq!(state.definitions, vec![definition]);
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
                enclosure: Some(rack.clone())
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
        placements: vec![api::PhysicalPlacement {
            item: Some(item),
            enclosure: Some(rack),
            volume: Some(volume()),
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
                placements: vec![api::PhysicalPlacement {
                    item: Some(item),
                    enclosure: Some(rack),
                    volume: Some(volume()),
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
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(item.clone())),
        }),
    )?;
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
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(item.clone())),
        }),
    )?;
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
            installed: vec![item.clone()],
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
        }),
    )?;
    assert_eq!(view.occupancy.len(), 1);
    // Expiring terminal plan details cannot free the authoritative installed footprint.
    assert!(store.cleanup(5000, None)?.removed > 0);
    let view = physical(&store, &actor, api::physical::Action::Read(api::PhysicalRead { enclosure: view.occupancy[0].placement.as_ref().and_then(|p|p.enclosure.clone()) }))?;
    assert_eq!(view.occupancy.len(), 1);
    Ok(())
}

// Saved-plan discovery is owner-scoped and does not hydrate resources or write metadata.
#[test]
fn saved_plan_list_is_personal_bounded_and_read_only() -> TestResult {
    let dir=tempfile::tempdir()?;
    let store=Store::open(dir.path(),policy())?;
    let actor=operator();
    let rack=create(&store,&actor,K::Rack,"Rack")?;
    let item=create(&store,&actor,K::Device,"Equipment")?;
    let plan=draft(&store,&actor,item,rack)?;
    let before=store.usage()?;
    let list=physical(&store,&actor,api::physical::Action::List(api::PhysicalPlanList {limit:1,after:None}))?;
    assert_eq!(list.plans.len(),1);
    assert_eq!(Some(&list.plans[0].id),plan.id.as_ref());
    assert!(list.plan.is_none());
    assert!(list.occupancy.is_empty());
    let mut other=actor.clone();other.actor="another-operator".into();
    assert!(physical(&store,&other,api::physical::Action::List(api::PhysicalPlanList::default()))?.plans.is_empty());
    assert!(matches!(physical(&store,&actor,api::physical::Action::List(api::PhysicalPlanList {limit:26,after:None})),Err(Error::Limit)));
    assert_eq!(store.usage()?,before);
    Ok(())
}

// Geometry cannot admit a placement whose sourced aggregate mass exceeds its enclosure limit.
#[test]
fn reservation_enforces_declared_mass_power_and_cooling_limits() -> TestResult {
    for axis in 0..3 {
        let dir=tempfile::tempdir()?;
        let store=Store::open(dir.path(),policy())?;
        let actor=operator();
        let rack=create(&store,&actor,K::Rack,"Capacity-limited rack")?;
        let item=create(&store,&actor,K::Device,"Demanding equipment")?;
        let mut enclosure=define(rack.clone());
        let mut equipment=define(item.clone());
        match axis {
            0=>{enclosure.max_mass_g=Some(1000);equipment.mass_g=Some(2000);},
            1=>{enclosure.capacity_mw=Some(1000);equipment.demand_mw=Some(2000);},
            _=>{enclosure.cooling_capacity_mw=Some(1000);equipment.cooling_mw=Some(2000);},
        }
        for definition in [enclosure,equipment] {
            physical(&store,&actor,api::physical::Action::Define(api::PhysicalDefinitionEdit { definition:Some(definition) }))?;
        }
        let plan=draft(&store,&actor,item,rack)?;
        let before=store.usage()?;
        assert!(matches!(reserve(&store,&actor,plan),Err(Error::Conflict)),"Capacity axis {axis} must reject admission");
        assert_eq!(store.usage()?,before);
    }
    Ok(())
}

// Structural keys identify positions only inside their actual owner's incarnation, not occupants.
#[test]
fn structural_positions_are_scoped_current_and_not_populated_implicitly() -> TestResult {
    let dir=tempfile::tempdir()?;let store=Store::open(dir.path(),policy())?;let actor=operator();
    let first=create(&store,&actor,K::Device,"First chassis")?;
    let second=create(&store,&actor,K::Device,"Second chassis")?;
    let edit=api::PhysicalPositionEdit {owner:Some(first.clone()),structural_key:"psu/1".into(),name:"PSU bay 1".into(),component_kind:api::PhysicalComponentKind::PowerSupply as i32,expected:None};
    let created=physical(&store,&actor,api::physical::Action::Position(edit.clone()))?;
    let position=&created.positions[0];
    let before=store.usage()?;
    let mut again=edit.clone();again.expected=Some(position.revision);
    assert_eq!(physical(&store,&actor,api::physical::Action::Position(again))?.positions,created.positions);
    assert_eq!(store.usage()?,before);
    let other=physical(&store,&actor,api::physical::Action::Position(api::PhysicalPositionEdit {owner:Some(second),..edit}))?;
    assert_ne!(other.positions[0].target,position.target);
    let list=physical(&store,&actor,api::physical::Action::Positions(api::PhysicalPositionQuery {owner:Some(first),after:None,limit:25}))?;
    assert_eq!(list.positions,created.positions);
    assert_eq!(list.items.iter().filter(|i|i.target==position.target).count(),1);
    let read=physical(&store,&actor,api::physical::Action::Read(api::PhysicalRead {enclosure:position.target.clone()}))?;
    assert!(read.occupancy.is_empty());
    assert!(matches!(create(&store,&actor,K::PhysicalPosition,"Unscoped invented bay"),Err(Error::Invalid)));
    Ok(())
}

// A hardware realization is a peer binding, not the owner of OS interface/configuration intent.
#[test]
fn logical_interface_does_not_become_a_child_of_its_hardware_port() -> TestResult {
    let dir=tempfile::tempdir()?;let store=Store::open(dir.path(),policy())?;let actor=operator();
    let port=create(&store,&actor,K::Port,"Current physical connector")?;
    let interface=create(&store,&actor,K::Interface,"Surviving logical interface")?;
    call(&store,&actor,Command::Relation(api::Relationship {relation:Some(wire::Relation {from:Some(port.clone()),to:Some(interface),kind:wire::RelationKind::Carries as i32,..Default::default()}),remove:false}))?;
    let Reply::Page(page)=call(&store,&actor,Command::Query(api::Query {view:api::View::Network as i32,focus:Some(port),level_only:true,columns:vec!["name".into()],limit:25,..Default::default()}))? else { return Err("expected page".into()); };
    assert_eq!(page.total,0,"Removing a hardware realization must not remove or scope-own logical configuration");
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
    assert!(!unchanged.items.is_empty(),"A no-op save retains the same permitted subject labels as a changed save");
    let same=unchanged.plan.ok_or(Error::Corrupt)?;
    assert_eq!(same, plan);
    assert_eq!(store.usage()?, before);
    Ok(())
}
