use super::*;

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
