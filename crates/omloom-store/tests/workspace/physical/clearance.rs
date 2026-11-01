use super::*;

// A free body-sized region is not usable when it consumes another device's declared connector/service space.
#[test]
fn current_and_reserved_clearance_cannot_be_consumed_by_another_body() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Clearance rack")?;
    let mut rack_facts = define(rack.clone());
    rack_facts
        .volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .length_um = 1_500_000;
    rack_facts.occupancy_coverage = rack_facts.volume.map(Box::new);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(rack_facts),
        }),
    )?;
    let first = create(&store, &actor, K::Device, "Server with side access")?;
    let second = create(&store, &actor, K::Device, "Another server")?;
    for target in [&first, &second] {
        equipment_dimensions(&store, &actor, target)?;
    }
    let facts = api::PhysicalDefinition {
        target: Some(first.clone()),
        basis: "Measured required side connector access".into(),
        clearance: Some(Box::new(api::PhysicalClearance {
            after_x_um: 500_000,
            ..Default::default()
        })),
        ..Default::default()
    };
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(facts.clone()),
        }),
    )?;
    let read = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(first.clone()),
            occupants: false,
            after: None,
        }),
    )?;
    assert_eq!(read.definitions[0].clearance, facts.clearance);
    let first_plan = draft(&store, &actor, first, rack.clone())?;
    let first_plan = reserve(&store, &actor, first_plan)?
        .plan
        .ok_or(Error::Corrupt)?;
    let mut second_plan = draft(&store, &actor, second, rack)?;
    let Some(api::physical_step::Action::Placement(p)) = &mut second_plan.steps[0].action else {
        return Err(Error::Corrupt.into());
    };
    p.volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .start_um = 500_000;
    let second_plan = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(second_plan),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    for recorded in [false, true] {
        if recorded {
            report_steps(&store, &actor, &first_plan, vec![0])?;
        }
        let preview = physical(
            &store,
            &actor,
            api::physical::Action::Preview(api::PhysicalPreview {
                plan: Some(second_plan.clone()),
            }),
        )?;
        assert_eq!(preview.state, api::RequirementState::Failed as i32);
        assert!(matches!(
            reserve(&store, &actor, second_plan.clone()),
            Err(Error::Conflict)
        ));
        if !recorded {
            assert_eq!(store.usage()?, usage);
        }
    }
    Ok(())
}
