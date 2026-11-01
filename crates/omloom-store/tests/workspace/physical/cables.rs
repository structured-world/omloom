use super::*;

fn edit(cable: &wire::AssetRef, key: &str) -> api::PhysicalCableEndEdit {
    api::PhysicalCableEndEdit {
        cable: Some(cable.clone()),
        end: Some(api::PhysicalCableEnd {
            key: key.into(),
            name: format!("Cable end {key}"),
            basis: "Operator-labelled physical cable".into(),
            connector: Some("LC/UPC duplex".into()),
            ..Default::default()
        }),
        expected: None,
    }
}

// Changed connector evidence blocks ordinary completion while preserving truthful actual-state reports.
#[test]
fn wiring_completion_rechecks_changed_port_mating() -> TestResult {
    for changed_role in [Some(api::PhysicalMatingRole::SideA), None] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let cable = create(&store, &actor, K::Cable, "Reserved cable")?;
        let port = create(&store, &actor, K::Port, "Changed connector")?;
        let mut end = edit(&cable, "A");
        end.end.as_mut().ok_or(Error::Corrupt)?.mating =
            Some(mating(api::PhysicalMatingRole::SideA));
        let saved = physical(&store, &actor, api::physical::Action::CableEnd(end))?
            .cable_ends
            .pop()
            .ok_or(Error::Missing)?;
        let mut definition = api::PhysicalDefinition {
            target: Some(port.clone()),
            basis: "Measured connector interface".into(),
            mating: Some(mating(api::PhysicalMatingRole::SideB)),
            ..Default::default()
        };
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(definition.clone()),
            }),
        )?;
        let plan = wiring_plan(
            &store,
            &actor,
            &cable,
            saved.revision,
            api::physical_wiring::After::Port(port.clone()),
            api::PhysicalWiringIntent::Preserve,
        )?;
        let reserved = reserve(&store, &actor, plan)?.plan.ok_or(Error::Missing)?;
        definition.mating = changed_role.map(mating);
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(definition),
            }),
        )?;
        let before = store.usage()?;
        let result = report(&store, &actor, &reserved, false);
        if changed_role.is_some() {
            assert!(matches!(result, Err(Error::Conflict)));
        } else {
            assert!(matches!(result, Err(Error::Incomplete)));
        }
        assert_eq!(store.usage()?, before);
        let actual = report(&store, &actor, &reserved, true)?;
        assert_eq!(actual.state, api::PhysicalPlanState::Completed as i32);
        let current = ends(&store, &actor, &cable, None)?
            .cable_ends
            .pop()
            .ok_or(Error::Missing)?;
        assert!(
            matches!(current.attachment, Some(api::physical_cable_end::Attachment::Connected(ref value)) if value.port.as_ref() == Some(&port))
        );
    }
    Ok(())
}

// A breakout remains one adopted assembly with explicit internal paths, not cloned cables.
#[test]
fn breakout_channels_have_two_ends_and_one_assembly_identity() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "DAC breakout assembly")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(cable.clone()),
                basis: "Vendor-declared inseparable copper assembly".into(),
                assembly_kind: api::PhysicalAssemblyKind::Dac as i32,
                ..Default::default()
            }),
        }),
    )?;
    for (key, channels) in [
        ("trunk", vec![1, 2, 3, 4]),
        ("leg-1", vec![1]),
        ("leg-2", vec![2]),
        ("leg-3", vec![3]),
        ("leg-4", vec![4]),
    ] {
        let mut value = edit(&cable, key);
        value.end.as_mut().ok_or("end")?.channels = channels;
        physical(&store, &actor, api::physical::Action::CableEnd(value))?;
    }
    let before = store.usage()?;
    let mut invalid = edit(&cable, "duplicate-leg");
    invalid.end.as_mut().ok_or("end")?.channels = vec![1];
    assert!(matches!(
        physical(&store, &actor, api::physical::Action::CableEnd(invalid)),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let current = ends(&store, &actor, &cable, None)?.cable_ends;
    assert_eq!(current.len(), 5);
    assert!(
        current.iter().all(|e| e.attachment.is_none()),
        "Channel declarations do not invent actual wiring"
    );
    let mut identical = edit(&cable, "trunk");
    let trunk = current.iter().find(|e| e.key == "trunk").ok_or("trunk")?;
    identical.expected = Some(trunk.revision);
    identical.end.as_mut().ok_or("end")?.channels = vec![4, 3, 2, 1];
    let saved = physical(&store, &actor, api::physical::Action::CableEnd(identical))?;
    assert_eq!(saved.cable_ends[0].revision, trunk.revision);
    assert_eq!(
        store.usage()?,
        before,
        "Reordering the same channel set is not a change"
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let current = ends(&store, &actor, &cable, None)?.cable_ends;
    assert_eq!(current.iter().map(|e| e.channels.len()).sum::<usize>(), 8);
    let profile = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(cable),
            occupants: false,
            after: None,
        }),
    )?;
    assert_eq!(
        profile.definitions[0].assembly_kind,
        api::PhysicalAssemblyKind::Dac as i32
    );
    Ok(())
}
fn ends(
    store: &Store,
    actor: &Context,
    cable: &wire::AssetRef,
    after: Option<String>,
) -> Result<api::PhysicalView, Error> {
    physical(
        store,
        actor,
        api::physical::Action::CableEnds(api::PhysicalCableEnds {
            cable: Some(cable.clone()),
            after,
        }),
    )
}

fn mating(role: api::PhysicalMatingRole) -> api::PhysicalMating {
    api::PhysicalMating {
        profile: Some(wire::EquipmentReference {
            issuer: "private:bench".into(),
            key: "lc-duplex".into(),
            basis: "Measured connector pair revision 1".into(),
        }),
        role: role as i32,
    }
}
fn wiring_plan(
    store: &Store,
    actor: &Context,
    cable: &wire::AssetRef,
    revision: u64,
    after: api::physical_wiring::After,
    intent: api::PhysicalWiringIntent,
) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Cable work".into(),
                steps: vec![api::PhysicalStep {
                    action: Some(api::physical_step::Action::Wiring(api::PhysicalWiring {
                        cable: Some(cable.clone()),
                        end_key: "A".into(),
                        basis: Some(api::physical_wiring::Basis::ExpectedEndRevision(revision)),
                        after: Some(after),
                        intent: intent as i32,
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)
}
fn report(
    store: &Store,
    actor: &Context,
    plan: &api::PhysicalPlan,
    actual: bool,
) -> Result<api::PhysicalPlan, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.clone().ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            completed_steps: vec![0],
            actual_state: actual,
            discrepancy: actual.then(|| "Externally reported attachment uncertainty".into()),
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)
}

// Reported unplugging preserves intended wiring; unknown attachment cannot release its possible port.
#[test]
fn cable_attachment_intent_and_unknown_capacity_remain_independent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "Fiber")?;
    let port = create(&store, &actor, K::Port, "Optic connector")?;
    let mut end = edit(&cable, "A");
    end.end.as_mut().ok_or(Error::Corrupt)?.mating = Some(mating(api::PhysicalMatingRole::SideA));
    let saved = physical(&store, &actor, api::physical::Action::CableEnd(end))?
        .cable_ends
        .pop()
        .ok_or(Error::Corrupt)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(port.clone()),
                basis: "Measured socket".into(),
                mating: Some(mating(api::PhysicalMatingRole::SideB)),
                ..Default::default()
            }),
        }),
    )?;
    let draft = wiring_plan(
        &store,
        &actor,
        &cable,
        saved.revision,
        api::physical_wiring::After::Port(port.clone()),
        api::PhysicalWiringIntent::AdoptDestination,
    )?;
    let reserved = reserve(&store, &actor, draft)?.plan.ok_or(Error::Corrupt)?;
    let done = report(&store, &actor, &reserved, false)?;
    assert_eq!(done.state, api::PhysicalPlanState::Completed as i32);
    let attached = ends(&store, &actor, &cable, None)?.cable_ends.remove(0);
    assert_eq!(attached.intended_port, Some(port.clone()));
    assert!(
        matches!(&attached.attachment,Some(api::physical_cable_end::Attachment::Connected(a)) if a.port.as_ref()==Some(&port))
    );
    let usage = store.usage()?;
    assert!(matches!(
        report(&store, &actor, &reserved, false),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    let unknown = wiring_plan(
        &store,
        &actor,
        &cable,
        attached.revision,
        api::physical_wiring::After::Unknown(api::Empty {}),
        api::PhysicalWiringIntent::Preserve,
    )?;
    report(&store, &actor, &unknown, true)?;
    let uncertain = ends(&store, &actor, &cable, None)?.cable_ends.remove(0);
    assert!(uncertain.attachment.is_none());
    assert_eq!(uncertain.intended_port, Some(port.clone()));
    assert_eq!(
        store
            .relations(AssetRef::try_from(&cable).map_err(|_| Error::Invalid)?, 32)?
            .len(),
        1
    );
    // Unknown keeps the established socket occupied against another cable's reservation.
    let competitor = create(&store, &actor, K::Cable, "Competing cable")?;
    let mut competing_end = edit(&competitor, "A");
    competing_end.end.as_mut().ok_or(Error::Corrupt)?.mating =
        Some(mating(api::PhysicalMatingRole::SideA));
    let competing_end = physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(competing_end),
    )?
    .cable_ends
    .remove(0);
    let competing = wiring_plan(
        &store,
        &actor,
        &competitor,
        competing_end.revision,
        api::physical_wiring::After::Port(port.clone()),
        api::PhysicalWiringIntent::Preserve,
    )?;
    let usage = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, competing),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    let loose = wiring_plan(
        &store,
        &actor,
        &cable,
        uncertain.revision,
        api::physical_wiring::After::Loose(api::Empty {}),
        api::PhysicalWiringIntent::Preserve,
    )?;
    let reserved = reserve(&store, &actor, loose)?.plan.ok_or(Error::Corrupt)?;
    report(&store, &actor, &reserved, false)?;
    let loose = ends(&store, &actor, &cable, None)?.cable_ends.remove(0);
    assert!(matches!(
        loose.attachment,
        Some(api::physical_cable_end::Attachment::Loose(_))
    ));
    assert_eq!(loose.intended_port, Some(port));
    assert!(
        store
            .relations(AssetRef::try_from(&cable).map_err(|_| Error::Invalid)?, 32)?
            .is_empty()
    );
    let usage = store.usage()?;
    ends(&store, &actor, &cable, None)?;
    assert_eq!(store.usage()?, usage);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(ends(&store, &actor, &cable, None)?.cable_ends[0], loose);
    Ok(())
}

// One saved plan cannot promise the same unoccupied socket to two cable ends.
#[test]
fn cable_plan_rejects_parallel_double_booking() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let port = create(&store, &actor, K::Port, "One socket")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(port.clone()),
                basis: "Measured socket".into(),
                mating: Some(mating(api::PhysicalMatingRole::SideB)),
                ..Default::default()
            }),
        }),
    )?;
    let mut steps = Vec::new();
    for name in ["Cable A", "Cable B"] {
        let cable = create(&store, &actor, K::Cable, name)?;
        let mut edit = edit(&cable, "A");
        edit.end.as_mut().ok_or(Error::Corrupt)?.mating =
            Some(mating(api::PhysicalMatingRole::SideA));
        let end = physical(&store, &actor, api::physical::Action::CableEnd(edit))?
            .cable_ends
            .remove(0);
        let plan = wiring_plan(
            &store,
            &actor,
            &cable,
            end.revision,
            api::physical_wiring::After::Port(port.clone()),
            api::PhysicalWiringIntent::Preserve,
        )?;
        steps.extend(plan.steps);
    }
    let plan = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Conflicting connections".into(),
                steps,
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    assert!(matches!(
        reserve(&store, &actor, plan),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    Ok(())
}

// A reported disconnect releases its socket for another cable only along the explicit dependency.
#[test]
fn ordered_socket_handoff_admits_two_cables_without_parallel_booking() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let port = create(&store, &actor, K::Port, "Handoff socket")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(port.clone()),
                basis: "Measured socket".into(),
                mating: Some(mating(api::PhysicalMatingRole::SideB)),
                ..Default::default()
            }),
        }),
    )?;
    let mut steps = Vec::new();
    for name in ["First cable", "Next cable"] {
        let cable = create(&store, &actor, K::Cable, name)?;
        let mut end = edit(&cable, "A");
        end.end.as_mut().ok_or(Error::Corrupt)?.mating =
            Some(mating(api::PhysicalMatingRole::SideA));
        let end = physical(&store, &actor, api::physical::Action::CableEnd(end))?
            .cable_ends
            .remove(0);
        let mut plan = wiring_plan(
            &store,
            &actor,
            &cable,
            end.revision,
            api::physical_wiring::After::Port(port.clone()),
            api::PhysicalWiringIntent::Preserve,
        )?;
        if steps.is_empty() {
            let mut disconnect = plan.steps[0].clone();
            let Some(api::physical_step::Action::Wiring(w)) = &mut disconnect.action else {
                return Err(Error::Corrupt.into());
            };
            w.basis = Some(api::physical_wiring::Basis::AfterStep(0));
            w.after = Some(api::physical_wiring::After::Loose(api::Empty {}));
            steps.append(&mut plan.steps);
            steps.push(disconnect);
        } else {
            plan.steps[0].control = Some(api::PlanControl {
                after: vec![api::PlanDependency {
                    step: 1,
                    when: api::PlanTrigger::Reported as i32,
                }],
                ..Default::default()
            });
            steps.append(&mut plan.steps);
        }
    }
    let plan = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Ordered socket handoff".into(),
                steps,
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(
        reserve(&store, &actor, plan)?
            .plan
            .ok_or(Error::Corrupt)?
            .state,
        api::PhysicalPlanState::Reserved as i32
    );
    Ok(())
}

// An AfterStep basis belongs to one exact cable end, not a neighboring end or cable.
#[test]
fn cable_chain_rejects_cross_end_and_cross_cable_bases_during_save() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "First cable")?;
    let other = create(&store, &actor, K::Cable, "Other cable")?;
    for (cable, key) in [(&cable, "A"), (&cable, "B"), (&other, "A")] {
        physical(
            &store,
            &actor,
            api::physical::Action::CableEnd(edit(cable, key)),
        )?;
    }
    let end = physical(
        &store,
        &actor,
        api::physical::Action::CableEnds(api::PhysicalCableEnds {
            cable: Some(cable.clone()),
            ..Default::default()
        }),
    )?
    .cable_ends
    .into_iter()
    .find(|e| e.key == "A")
    .ok_or(Error::Missing)?;
    let base = wiring_plan(
        &store,
        &actor,
        &cable,
        end.revision,
        api::physical_wiring::After::Loose(api::Empty {}),
        api::PhysicalWiringIntent::Preserve,
    )?;
    for (cable, key) in [(cable, "B"), (other, "A")] {
        let mut plan = base.clone();
        let mut second = plan.steps[0].clone();
        let Some(api::physical_step::Action::Wiring(wire)) = &mut second.action else {
            return Err(Error::Corrupt.into());
        };
        wire.cable = Some(cable);
        wire.end_key = key.into();
        wire.basis = Some(api::physical_wiring::Basis::AfterStep(0));
        plan.steps.push(second);
        let before = store.usage()?;
        assert!(matches!(
            physical(
                &store,
                &actor,
                api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) })
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}

// Abandoning an unstarted dependent step needs no fictional output from its predecessor.
#[test]
fn cable_chain_cancellation_needs_no_effect_result() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "Unstarted cable")?;
    let end = physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(edit(&cable, "A")),
    )?
    .cable_ends
    .remove(0);
    let mut plan = wiring_plan(
        &store,
        &actor,
        &cable,
        end.revision,
        api::physical_wiring::After::Loose(api::Empty {}),
        api::PhysicalWiringIntent::Preserve,
    )?;
    let mut second = plan.steps[0].clone();
    let Some(api::physical_step::Action::Wiring(w)) = &mut second.action else {
        return Err(Error::Corrupt.into());
    };
    w.basis = Some(api::physical_wiring::Basis::AfterStep(0));
    plan.steps.push(second);
    let plan = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    let cancelled = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            not_performed_steps: vec![0, 1],
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(cancelled.state, api::PhysicalPlanState::Cancelled as i32);
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        0
    );
    assert_eq!(ends(&store, &actor, &cable, None)?.cable_ends[0], end);
    assert!(store.usage()?.revision > usage.revision);
    Ok(())
}

// Defining ends never invents a live attachment; repeats and reads do not write history.
#[test]
fn cable_end_definitions_are_scoped_bounded_and_durable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::Cable, "Cable A").expect("Create cable A fixture");
    let b = create(&store, &actor, K::Cable, "Cable B").expect("Create cable B fixture");
    let mut value = edit(&a, "A");
    let saved = physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(value.clone()),
    )
    .expect("Save named cable end")
    .cable_ends
    .pop()
    .ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    value.expected = Some(saved.revision);
    physical(&store, &actor, api::physical::Action::CableEnd(value))?;
    assert_eq!(
        ends(&store, &actor, &a, None)?.cable_ends,
        vec![saved.clone()]
    );
    assert!(ends(&store, &actor, &b, None)?.cable_ends.is_empty());
    assert_eq!(store.usage()?, usage);
    assert!(
        store
            .relations(AssetRef::try_from(&a).map_err(|_| Error::Invalid)?, 32)?
            .is_empty()
    );
    for i in 1..64 {
        physical(
            &store,
            &actor,
            api::physical::Action::CableEnd(edit(&a, &format!("end{i:02}"))),
        )?;
    }
    let usage = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::CableEnd(edit(&a, "overflow"))
        ),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, usage);
    let first = ends(&store, &actor, &a, None)?;
    assert_eq!(first.cable_ends.len(), 25);
    let second = ends(&store, &actor, &a, first.next)?;
    assert_eq!(second.cable_ends.len(), 25);
    let last = ends(&store, &actor, &a, second.next)?;
    assert_eq!(last.cable_ends.len(), 14);
    assert!(last.next.is_none());
    assert_eq!(store.usage()?, usage);
    drop(store);
    let reopened = Store::open(dir.path(), policy())?;
    assert_eq!(ends(&reopened, &actor, &a, None)?.cable_ends[0], saved);
    Ok(())
}

// Scoped rights protect end definitions too; deleting an unlinked cable reclaims its own ends.
#[test]
fn cable_ends_follow_the_cables_rights_and_removal_lifecycle() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "Unlinked cable")?;
    physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(edit(&cable, "A")),
    )?;
    let usage = store.usage()?;
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(matches!(
        ends(&store, &restricted, &cable, None),
        Err(Error::Denied)
    ));
    assert!(matches!(
        physical(
            &store,
            &restricted,
            api::physical::Action::CableEnd(edit(&cable, "B"))
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, usage);
    call(
        &store,
        &actor,
        Command::Bulk(api::Bulk {
            targets: vec![cable],
            expected: usage.revision,
            remove: true,
            ..Default::default()
        }),
    )?;
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Inventory as usize],
        usage.rows[omloom_store::Class::Inventory as usize] - 3
    );
    Ok(())
}
