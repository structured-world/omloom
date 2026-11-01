use super::*;

// Slot-only replacements enforce each declared load budget even without a placement volume.
#[test]
fn component_replacement_checks_mass_power_and_cooling_limits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Capacity owner")?;
    let position = position_fixture(
        &store,
        &actor,
        owner,
        "disk/1",
        api::PhysicalComponentKind::Disk,
    )?;
    let old = create(&store, &actor, K::PhysicalComponent, "Previous drive")?;
    let incoming = create(
        &store,
        &actor,
        K::PhysicalComponent,
        "Oversized drive demand",
    )?;
    install_component(&store, &actor, position.clone(), old.clone())?;
    equipment_dimensions(&store, &actor, &incoming)?;
    let old_binding = binding(&store, &old)?;
    let plan = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            previous: Some(old),
            replacement: Some(incoming.clone()),
            previous_binding: Some(old_binding.relation_id),
            previous_revision: Some(old_binding.revision),
            ..Default::default()
        }],
    )?;
    for selected in 0..3 {
        let mut slot = define(position.clone());
        slot.mounting = Some("drive-carrier-v1".into());
        slot.max_mass_g = (selected == 0).then_some(10);
        slot.capacity_mw = (selected == 1).then_some(10);
        slot.cooling_capacity_mw = (selected == 2).then_some(10);
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(slot),
            }),
        )?;
        let actual = api::PhysicalDefinition {
            target: Some(incoming.clone()),
            basis: "Accepted drive requirements".into(),
            component_kind: api::PhysicalComponentKind::Disk as i32,
            mounting: Some("drive-carrier-v1".into()),
            demand_mw: Some(20),
            cooling_mw: Some(20),
            ..Default::default()
        };
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(actual.clone()),
            }),
        )?;
        let asset = store.asset(AssetRef::try_from(&incoming)?)?;
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(incoming.clone()),
                kind: K::PhysicalComponent as i32,
                expected: Some(asset.revision),
                changes: vec![equipment_quantity("weight", 20, "g", 0)],
                ..Default::default()
            }),
        )?;
        let before = store.usage()?;
        let preview = physical(
            &store,
            &actor,
            api::physical::Action::Preview(api::PhysicalPreview {
                plan: Some(plan.clone()),
            }),
        )?;
        assert_eq!(
            preview.state,
            api::RequirementState::Failed as i32,
            "Selected budget {selected}"
        );
        assert!(matches!(
            reserve(&store, &actor, plan.clone()),
            Err(Error::Conflict)
        ));
        assert_eq!(store.usage()?, before);
        let mut unknown = actual;
        unknown.demand_mw = None;
        unknown.cooling_mw = None;
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(unknown.clone()),
            }),
        )?;
        let asset = store.asset(AssetRef::try_from(&incoming)?)?;
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(incoming.clone()),
                kind: K::PhysicalComponent as i32,
                expected: Some(asset.revision),
                changes: vec![wire::Field {
                    name: "weight".into(),
                    value: Some(wire::field::Value::Clear(wire::NullValue::NullValue as i32)),
                }],
                ..Default::default()
            }),
        )?;
        let preview = physical(
            &store,
            &actor,
            api::physical::Action::Preview(api::PhysicalPreview {
                plan: Some(plan.clone()),
            }),
        )?;
        assert_eq!(preview.state, api::RequirementState::Unknown as i32);
        unknown.demand_mw = Some(10);
        unknown.cooling_mw = Some(10);
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(unknown),
            }),
        )?;
        let asset = store.asset(AssetRef::try_from(&incoming)?)?;
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(incoming.clone()),
                kind: K::PhysicalComponent as i32,
                expected: Some(asset.revision),
                changes: vec![equipment_quantity("weight", 10, "g", 0)],
                ..Default::default()
            }),
        )?;
        let preview = physical(
            &store,
            &actor,
            api::physical::Action::Preview(api::PhysicalPreview {
                plan: Some(plan.clone()),
            }),
        )?;
        assert_eq!(preview.state, api::RequirementState::Satisfied as i32);
    }
    Ok(())
}

// Geometry alone cannot satisfy a declared rail requirement; preview and reservation share admission.
#[test]
fn rack_mounting_requirements_gate_preview_and_reservation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Mounting rack")?;
    let item = create(&store, &actor, K::Device, "Rail-mounted server")?;
    equipment_dimensions(&store, &actor, &item)?;
    let mut rack_facts = define(rack.clone());
    rack_facts.mounting = Some("qualified-rail-A".into());
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(rack_facts),
        }),
    )?;
    let mut item_facts = api::PhysicalDefinition {
        target: Some(item.clone()),
        basis: "Manufacturer rail specification".into(),
        mounting: Some("qualified-rail-B".into()),
        ..Default::default()
    };
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(item_facts.clone()),
        }),
    )?;
    let plan = draft(&store, &actor, item, rack)?;
    let before = store.usage()?;
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(plan.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Failed as i32);
    assert!(matches!(
        reserve(&store, &actor, plan.clone()),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    item_facts.mounting = None;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(item_facts.clone()),
        }),
    )?;
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(plan.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Unknown as i32);
    assert!(matches!(
        reserve(&store, &actor, plan.clone()),
        Err(Error::Incomplete)
    ));
    item_facts.mounting = Some("qualified-rail-A".into());
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(item_facts),
        }),
    )?;
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(plan.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Satisfied as i32);
    reserve(&store, &actor, plan)?;
    Ok(())
}
