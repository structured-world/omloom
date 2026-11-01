use super::*;

fn loads(store: &Store, actor: &Context, item: &wire::AssetRef, value: u64) -> TestResult {
    let mut profile = define(item.clone());
    profile.mounting = Some("floor-standing".into());
    profile.demand_mw = Some(value);
    profile.cooling_mw = Some(value);
    physical(
        store,
        actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(profile),
        }),
    )?;
    let asset = store.asset(AssetRef::try_from(item)?)?;
    call(
        store,
        actor,
        Command::Write(api::Write {
            target: Some(item.clone()),
            expected: Some(asset.revision),
            kind: asset.kind,
            changes: vec![equipment_quantity("weight", i64::try_from(value)?, "g", 0)],
            ..Default::default()
        }),
    )?;
    Ok(())
}

fn fixture(
    store: &Store,
    actor: &Context,
    kind: K,
    axis: usize,
) -> Result<(api::PhysicalReplacement, wire::AssetRef), Box<dyn std::error::Error>> {
    let enclosure = create(store, actor, kind, "Shared enclosure")?;
    let old = create(store, actor, K::Device, "Previous machine")?;
    let retained = create(store, actor, K::Device, "Retained machine")?;
    let incoming = create(store, actor, K::Device, "Incoming machine")?;
    let mut profile = define(enclosure.clone());
    profile.mounting = Some("floor-standing".into());
    match axis {
        0 => profile.max_mass_g = Some(100),
        1 => profile.capacity_mw = Some(100),
        _ => profile.cooling_capacity_mw = Some(100),
    }
    physical(
        store,
        actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(profile),
        }),
    )?;
    for item in [&old, &retained, &incoming] {
        equipment_dimensions(store, actor, item)?;
        loads(store, actor, item, if item == &old { 0 } else { 60 })?;
    }
    for item in [&old, &retained] {
        call(
            store,
            actor,
            Command::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(item.clone()),
                    to: Some(enclosure.clone()),
                    kind: wire::RelationKind::InstalledIn as i32,
                    ..Default::default()
                }),
                remove: false,
            }),
        )?;
    }
    let prior = binding(store, &old)?;
    Ok((
        api::PhysicalReplacement {
            position: Some(enclosure),
            previous: Some(old),
            replacement: Some(incoming),
            previous_binding: Some(prior.relation_id),
            previous_revision: Some(prior.revision),
            ..Default::default()
        },
        retained,
    ))
}

// Unpositioned devices still consume shared weight, power and cooling budgets.
#[test]
fn unpositioned_replacements_include_retained_loads() -> TestResult {
    for kind in [K::Site, K::Location] {
        for axis in 0..3 {
            let dir = tempfile::tempdir()?;
            let store = Store::open(dir.path(), policy())?;
            let actor = operator();
            let (replacement, retained) = fixture(&store, &actor, kind, axis)?;
            let plan = replacement_plan(&store, &actor, vec![replacement])?;
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
                "{kind:?}, axis {axis}"
            );
            assert!(matches!(
                reserve(&store, &actor, plan.clone()),
                Err(Error::Conflict)
            ));
            assert_eq!(store.usage()?, before);
            loads(&store, &actor, &retained, 40)?;
            let reserved = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
            loads(&store, &actor, &retained, 41)?;
            let completion = api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: reserved.id.ok_or(Error::Corrupt)?,
                    revision: reserved.revision,
                }),
                completed_steps: vec![0],
                ..Default::default()
            };
            let before = store.usage()?;
            assert!(matches!(
                physical(
                    &store,
                    &actor,
                    api::physical::Action::Complete(completion.clone())
                ),
                Err(Error::Conflict)
            ));
            assert_eq!(store.usage()?, before);
            loads(&store, &actor, &retained, 40)?;
            let completed = physical(&store, &actor, api::physical::Action::Complete(completion))?
                .plan
                .ok_or(Error::Corrupt)?;
            assert_eq!(completed.state, api::PhysicalPlanState::Completed as i32);
        }
    }
    Ok(())
}

// An accepted unpositioned replacement holds capacity across reopen until recorded or stopped.
#[test]
fn unpositioned_replacements_include_other_plan_reservations() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let (replacement, retained) = fixture(&store, &actor, K::Site, 1)?;
    loads(&store, &actor, &retained, 0)?;
    let other = create(&store, &actor, K::Device, "Another incoming machine")?;
    equipment_dimensions(&store, &actor, &other)?;
    loads(&store, &actor, &other, 60)?;
    let prior = binding(&store, &retained)?;
    let second = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: replacement.position.clone(),
            previous: Some(retained),
            replacement: Some(other),
            previous_binding: Some(prior.relation_id),
            previous_revision: Some(prior.revision),
            ..Default::default()
        }],
    )?;
    let first = replacement_plan(&store, &actor, vec![replacement])?;
    let first = reserve(&store, &actor, first)?.plan.ok_or(Error::Corrupt)?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, second.clone()),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: first.id.ok_or(Error::Corrupt)?,
                revision: first.revision,
            }),
            completed_steps: vec![0],
            ..Default::default()
        }),
    )?;
    assert!(matches!(
        reserve(&store, &actor, second),
        Err(Error::Conflict)
    ));
    Ok(())
}

// A real capacity-only hold has unknown geometry, not a corrupt placement record.
#[test]
fn unpositioned_hold_keeps_geometric_admission_unknown() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let (replacement, retained) = fixture(&store, &actor, K::Site, 1)?;
    loads(&store, &actor, &retained, 0)?;
    let site = replacement.position.clone().ok_or(Error::Corrupt)?;
    let mut profile = define(site.clone());
    profile.mounting = Some("floor-standing".into());
    profile.capacity_mw = Some(100);
    profile
        .volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .length_um = 1_500_000;
    profile.occupancy_coverage = profile.volume.map(Box::new);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(profile),
        }),
    )?;
    for (index, item) in [
        replacement.previous.clone().ok_or(Error::Corrupt)?,
        retained,
    ]
    .into_iter()
    .enumerate()
    {
        let mut plan = draft(&store, &actor, item, site.clone())?;
        let Some(api::physical_step::Action::Placement(p)) = &mut plan.steps[0].action else {
            return Err(Error::Corrupt.into());
        };
        p.volume
            .as_mut()
            .ok_or(Error::Corrupt)?
            .x
            .as_mut()
            .ok_or(Error::Corrupt)?
            .start_um = index as i64 * 500_000;
        let plan = physical(
            &store,
            &actor,
            api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
        )?
        .plan
        .ok_or(Error::Corrupt)?;
        report_steps(&store, &actor, &plan, vec![0])?;
    }
    let pending = replacement_plan(&store, &actor, vec![replacement])?;
    reserve(&store, &actor, pending)?;
    let third = create(&store, &actor, K::Device, "Geometric candidate")?;
    equipment_dimensions(&store, &actor, &third)?;
    loads(&store, &actor, &third, 0)?;
    let mut proposal = draft(&store, &actor, third, site)?;
    let Some(api::physical_step::Action::Placement(p)) = &mut proposal.steps[0].action else {
        return Err(Error::Corrupt.into());
    };
    p.volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .start_um = 1_000_000;
    let before = store.usage()?;
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(proposal),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Unknown as i32);
    assert_eq!(preview.reason, api::RequirementReason::Missing as i32);
    assert_eq!(store.usage()?, before);
    Ok(())
}
