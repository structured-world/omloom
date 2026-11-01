use super::*;

// A manual report records work already performed, not a new physical-effect authorization.
#[test]
fn changed_geometry_does_not_discard_an_attributed_manual_installation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Changed enclosure")?;
    let item = create(&store, &actor, K::Device, "Previously fitting server")?;
    equipment_dimensions(&store, &actor, &item)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let plan = draft(&store, &actor, item, rack.clone())?;
    let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let mut updated = define(rack.clone());
    updated
        .volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .length_um = 1;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(updated),
        }),
    )?;
    let report = api::PhysicalCompletion {
        plan: Some(api::PhysicalPlanKey {
            id: plan.id.ok_or(Error::Corrupt)?,
            revision: plan.revision,
        }),
        completed_steps: vec![0],
        discrepancy: Some(
            "Already installed; recorded enclosure width needs reconciliation".into(),
        ),
        ..Default::default()
    };
    physical(&store, &actor, api::physical::Action::Complete(report))?;
    let current = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
        }),
    )?;
    assert_eq!(current.occupancy.len(), 1);
    assert_eq!(
        current.definitions[0]
            .volume
            .as_ref()
            .ok_or(Error::Corrupt)?
            .x
            .as_ref()
            .ok_or(Error::Corrupt)?
            .length_um,
        1
    );
    Ok(())
}

// Merely listing a future removal cannot free current space for an independent step.
#[test]
fn replacement_space_credit_requires_the_shared_plan_prerequisite() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Occupied enclosure")?;
    let old = create(&store, &actor, K::Device, "Established server")?;
    let incoming = create(&store, &actor, K::Device, "Incoming server")?;
    for target in [&old, &incoming] {
        equipment_dimensions(&store, &actor, target)?;
    }
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let installed = draft(&store, &actor, old.clone(), rack.clone())?;
    report_steps(&store, &actor, &installed, vec![0])?;
    let previous = binding(&store, &old)?;
    let mut replacement = api::PhysicalPlan {
        name: "Remove then place, with explicit dependency".into(),
        steps: vec![
            api::PhysicalStep {
                action: Some(api::physical_step::Action::Replacement(
                    api::PhysicalReplacement {
                        position: Some(rack.clone()),
                        previous: Some(old.clone()),
                        previous_binding: Some(previous.relation_id),
                        previous_revision: Some(previous.revision),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            api::PhysicalStep {
                action: Some(api::physical_step::Action::Placement(
                    api::PhysicalPlacement {
                        item: Some(incoming.clone()),
                        enclosure: Some(rack.clone()),
                        volume: Some(volume()),
                    },
                )),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let unordered = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(replacement.clone()),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, unordered),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    replacement.steps[1].control = Some(api::PlanControl {
        after: vec![api::PlanDependency {
            step: 0,
            when: api::PlanTrigger::Reported as i32,
        }],
        ..Default::default()
    });
    // An unrelated placement in genuinely free space need not inherit the removal.
    let parallel = create(&store, &actor, K::Device, "Independent neighboring server")?;
    equipment_dimensions(&store, &actor, &parallel)?;
    let mut enclosure = define(rack.clone());
    enclosure
        .volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .length_um = 1_000_000;
    enclosure.occupancy_coverage = enclosure.volume.map(Box::new);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(enclosure.clone()),
        }),
    )?;
    let mut neighboring = volume();
    neighboring.x.as_mut().ok_or(Error::Corrupt)?.start_um = 500_000;
    replacement.steps.push(api::PhysicalStep {
        action: Some(api::physical_step::Action::Placement(
            api::PhysicalPlacement {
                item: Some(parallel.clone()),
                enclosure: Some(rack),
                volume: Some(neighboring),
            },
        )),
        ..Default::default()
    });
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(replacement.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Satisfied as i32);
    // Final load fits, but the independent neighbor could arrive before the old heavy server leaves.
    for (target, grams) in [(&old, 20), (&incoming, 5), (&parallel, 10)] {
        let current = store.asset(AssetRef::try_from(target)?)?;
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(target.clone()),
                kind: current.kind,
                expected: Some(current.revision),
                changes: vec![equipment_quantity("weight", grams, "g", 0)],
                ..Default::default()
            }),
        )?;
    }
    enclosure.max_mass_g = Some(25);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(enclosure),
        }),
    )?;
    let ordered = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(replacement),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, ordered.clone()),
        Err(Error::Incomplete)
    ));
    assert_eq!(store.usage()?, usage);
    let mut ordered = ordered;
    ordered.steps[2].control = ordered.steps[1].control.clone();
    let ordered = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(ordered),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    reserve(&store, &actor, ordered)?;
    Ok(())
}
