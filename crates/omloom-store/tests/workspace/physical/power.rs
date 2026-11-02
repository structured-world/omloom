use super::*;

// A common upstream is a counterexample only after that subject is authorized, not merely discovered.
#[test]
fn hidden_shared_power_upstream_is_restricted_without_target_disclosure() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::PowerFeed, "Visible feed A")?;
    let b = create(&store, &actor, K::PowerFeed, "Visible feed B")?;
    let hidden = create(&store, &actor, K::Device, "Hidden shared upstream")?;
    for item in [&a, &b, &hidden] {
        survey(&store, &actor, item)?;
    }
    supply(&store, &actor, &a, &hidden)?;
    supply(&store, &actor, &b, &hidden)?;
    let mut limited = actor.clone();
    limited.grants[0].scope =
        Scope::Targets(vec![AssetRef::try_from(&a)?, AssetRef::try_from(&b)?]);
    let sources = [a, b];
    let before = store.usage()?;
    for boundaries in [Vec::new(), vec![hidden.clone()]] {
        let result = evaluate(&store, &limited, &sources, &boundaries)?;
        assert_eq!(result.reason, api::RequirementReason::Restricted as i32);
        assert!(result.results.iter().all(|v| v.target.is_none()));
    }
    let allowed = evaluate(&store, &actor, &sources, &[])?;
    assert_eq!(allowed.state, api::RequirementState::Failed as i32);
    assert_eq!(allowed.results[0].target.as_ref(), Some(&hidden));
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn survey(store: &Store, actor: &Context, resource: &wire::AssetRef) -> TestResult {
    let definition = api::PhysicalDefinition {
        target: Some(resource.clone()),
        basis: "Reviewed current upstream supply connections".into(),
        power_sources_complete: true,
        ..Default::default()
    };
    let view = physical(
        store,
        actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(definition.clone()),
        }),
    )?;
    assert_eq!(view.definitions, vec![definition]);
    Ok(())
}
fn supply(
    store: &Store,
    actor: &Context,
    from: &wire::AssetRef,
    to: &wire::AssetRef,
) -> TestResult {
    call(
        store,
        actor,
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(from.clone()),
                to: Some(to.clone()),
                kind: wire::RelationKind::PoweredBy as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    Ok(())
}
fn evaluate(
    store: &Store,
    actor: &Context,
    sources: &[wire::AssetRef],
    boundaries: &[wire::AssetRef],
) -> Result<api::RequirementEvaluation, Error> {
    let result = call(
        store,
        actor,
        Command::EvaluateRequirements(api::EvaluateRequirements {
            anchor: sources.first().cloned(),
            nodes: vec![api::RequirementNode {
                expression: Some(api::requirement_node::Expression::PowerPaths(
                    api::PowerPathRequirement {
                        sources: sources.into(),
                        boundaries: boundaries.into(),
                    },
                )),
            }],
            root: 0,
        }),
    )?;
    match result {
        Reply::RequirementEvaluation(v) => Ok(v),
        _ => Err(Error::Corrupt),
    }
}

// Feed names and absent edges do not qualify independent paths; only complete bounded paths do.
#[test]
fn power_path_checks_use_current_scoped_topology_without_snapshots() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::PowerFeed, "Feed A")?;
    let b = create(&store, &actor, K::PowerFeed, "Feed B")?;
    let root_a = create(&store, &actor, K::PowerFeed, "Terminal A")?;
    let root_b = create(&store, &actor, K::PowerFeed, "Terminal B")?;
    let sources = [a.clone(), b.clone()];
    assert_eq!(
        evaluate(&store, &actor, &sources, &sources)?.reason,
        api::RequirementReason::Missing as i32
    );
    for resource in [&a, &b] {
        survey(&store, &actor, resource)?;
    }
    assert_eq!(
        evaluate(&store, &actor, &sources, &sources)?.state,
        api::RequirementState::Satisfied as i32
    );
    assert_eq!(
        evaluate(
            &store,
            &actor,
            &[a.clone(), a.clone()],
            std::slice::from_ref(&a)
        )?
        .state,
        api::RequirementState::Failed as i32
    );
    supply(&store, &actor, &a, &root_a)?;
    supply(&store, &actor, &b, &root_b)?;
    assert_eq!(
        evaluate(&store, &actor, &sources, &sources)?.reason,
        api::RequirementReason::ScopeMismatch as i32
    );
    let terminals = [root_a.clone(), root_b.clone()];
    assert_eq!(
        evaluate(&store, &actor, &sources, &terminals)?.reason,
        api::RequirementReason::Missing as i32
    );
    for resource in &terminals {
        survey(&store, &actor, resource)?;
    }
    let before = store.usage()?;
    survey(&store, &actor, &root_b)?;
    assert_eq!(
        store.usage()?,
        before,
        "Equal source assertions add no rows or audit detail"
    );
    assert_eq!(
        evaluate(&store, &actor, &sources, &terminals)?.state,
        api::RequirementState::Satisfied as i32
    );
    assert_eq!(store.usage()?, before);
    supply(&store, &actor, &b, &root_a)?;
    let before = store.usage()?;
    assert_eq!(
        evaluate(&store, &actor, &sources, &terminals)?.state,
        api::RequirementState::Failed as i32
    );
    let mut limited = actor.clone();
    limited.grants[0].scope = Scope::Targets(vec![
        AssetRef::try_from(&a)?,
        AssetRef::try_from(&b)?,
        AssetRef::try_from(&root_b)?,
    ]);
    let denied = evaluate(&store, &limited, &sources, &terminals)?;
    assert_eq!(denied.reason, api::RequirementReason::Restricted as i32);
    assert!(
        denied
            .results
            .iter()
            .all(|r| r.target.is_none() && r.evidence.is_empty())
    );
    limited = actor.clone();
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    assert_eq!(
        evaluate(&store, &limited, &sources, &terminals)?.reason,
        api::RequirementReason::Restricted as i32
    );
    assert_eq!(store.usage()?, before);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        evaluate(&store, &actor, &sources, &terminals)?.state,
        api::RequirementState::Failed as i32
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A shared physical UPS is a real dependency, not another duplicated power-feed inventory.
#[test]
fn feed_paths_follow_existing_hardware_and_input_ports() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::PowerFeed, "Rack feed A")?;
    let b = create(&store, &actor, K::PowerFeed, "Rack feed B")?;
    let ups = create(&store, &actor, K::Device, "One physical UPS")?;
    let inlet = create(&store, &actor, K::Port, "UPS input")?;
    let utility = create(&store, &actor, K::PowerFeed, "Selected utility boundary")?;
    for resource in [&a, &b, &ups, &inlet, &utility] {
        survey(&store, &actor, resource)?;
    }
    for (from, to) in [(&a, &ups), (&b, &ups), (&ups, &inlet), (&inlet, &utility)] {
        supply(&store, &actor, from, to)?;
    }
    let sources = [a.clone(), b.clone()];
    let result = evaluate(&store, &actor, &sources, std::slice::from_ref(&utility))?;
    assert_eq!(result.state, api::RequirementState::Failed as i32);
    assert_eq!(result.results[0].target, Some(ups.clone()));
    let mut limited = actor;
    limited.grants[0].scope = Scope::Targets(vec![
        AssetRef::try_from(&a)?,
        AssetRef::try_from(&b)?,
        AssetRef::try_from(&ups)?,
    ]);
    let result = evaluate(&store, &limited, &sources, &[utility])?;
    assert_eq!(
        result.state,
        api::RequirementState::Failed as i32,
        "A visible shared UPS is a complete counterexample without reading its private upstream network"
    );
    assert_eq!(result.results[0].target, Some(ups));
    let partial = evaluate(&store, &limited, &sources, &[])?;
    assert_eq!(
        partial.state,
        api::RequirementState::Failed as i32,
        "A known shared UPS needs no invented terminal source to establish the counterexample"
    );
    Ok(())
}

// Two references to one asset with different incarnations cannot alias one metadata row.
#[test]
fn power_paths_validate_every_requested_incarnation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::PowerFeed, "Live feed")?;
    let b = create(&store, &actor, K::PowerFeed, "Different incarnation")?;
    survey(&store, &actor, &a)?;
    let stale = wire::AssetRef {
        asset_id: a.asset_id.clone(),
        incarnation: b.incarnation,
    };
    let inputs = [a, stale];
    let result = evaluate(&store, &actor, &inputs, &inputs)?;
    assert_eq!(result.reason, api::RequirementReason::Restricted as i32);
    assert!(result.results.iter().all(|r| r.target.is_none()));
    Ok(())
}

// Recording manual work evaluates the persisted power postcondition through the common evaluator.
#[test]
fn recorded_step_evaluates_its_power_postcondition() -> TestResult {
    assert_recorded_power_postcondition(true)
}

// Healthy recorded paths must remain satisfied, not inherit a failure-only completion result.
#[test]
fn recorded_step_preserves_a_satisfied_power_postcondition() -> TestResult {
    assert_recorded_power_postcondition(false)
}

fn assert_recorded_power_postcondition(shared_supply: bool) -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Planning rack")?;
    let item = create(&store, &actor, K::Device, "Installed server")?;
    let feeds = [
        create(&store, &actor, K::PowerFeed, "Terminal feed A")?,
        create(&store, &actor, K::PowerFeed, "Terminal feed B")?,
    ];
    let terminals = [
        create(&store, &actor, K::PowerFeed, "Independent terminal A")?,
        create(&store, &actor, K::PowerFeed, "Independent terminal B")?,
    ];
    for feed in feeds.iter().chain(&terminals) {
        survey(&store, &actor, feed)?;
    }
    supply(&store, &actor, &feeds[0], &terminals[0])?;
    supply(&store, &actor, &feeds[1], &terminals[1])?;
    assert_eq!(
        evaluate(&store, &actor, &feeds, &terminals)?.state,
        api::RequirementState::Satisfied as i32
    );
    let mut plan = draft(&store, &actor, item, rack.clone())?;
    plan.steps[0].control = Some(api::PlanControl {
        check: Some(api::EvaluateRequirements {
            anchor: Some(rack),
            nodes: vec![api::RequirementNode {
                expression: Some(api::requirement_node::Expression::PowerPaths(
                    api::PowerPathRequirement {
                        sources: feeds.to_vec(),
                        boundaries: terminals.to_vec(),
                    },
                )),
            }],
            root: 0,
        }),
        ..Default::default()
    });
    let saved = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    if shared_supply {
        supply(&store, &actor, &feeds[1], &terminals[0])?;
    }
    let expected = evaluate(&store, &actor, &feeds, &terminals)?;
    assert_eq!(
        expected.state,
        if shared_supply {
            api::RequirementState::Failed
        } else {
            api::RequirementState::Satisfied
        } as i32
    );
    let recorded = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: saved.id.ok_or(Error::Corrupt)?,
                revision: saved.revision,
            }),
            completed_steps: vec![0],
            actual_state: true,
            discrepancy: Some("Recorded manually completed installation".into()),
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(
        recorded.steps[0].state,
        api::PhysicalStepState::Recorded as i32
    );
    assert_eq!(recorded.steps[0].checked_state, Some(expected.state));
    assert_eq!(recorded.steps[0].checked_reason, Some(expected.reason));
    Ok(())
}

// The typed power predicate survives the existing plan codec without an early source/geometry gate.
#[test]
fn power_postcondition_uses_the_common_persisted_plan() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Unmeasured planning rack")?;
    let item = create(&store, &actor, K::Device, "Partly specified server")?;
    let a = create(&store, &actor, K::PowerFeed, "Unqualified feed A")?;
    let b = create(&store, &actor, K::PowerFeed, "Unqualified feed B")?;
    let mut plan = draft(&store, &actor, item, rack.clone())?;
    let check = api::EvaluateRequirements {
        anchor: Some(rack),
        nodes: vec![api::RequirementNode {
            expression: Some(api::requirement_node::Expression::PowerPaths(
                api::PowerPathRequirement {
                    sources: vec![a, b],
                    boundaries: vec![],
                },
            )),
        }],
        root: 0,
    };
    plan.steps[0].control = Some(api::PlanControl {
        check: Some(check.clone()),
        ..Default::default()
    });
    let saved = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(
        saved.steps[0]
            .control
            .as_ref()
            .and_then(|c| c.check.as_ref()),
        Some(&check)
    );
    let key = api::PhysicalPlanKey {
        id: saved.id.ok_or(Error::Corrupt)?,
        revision: saved.revision,
    };
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let loaded = physical(&store, &actor, api::physical::Action::Load(key))?
        .plan
        .ok_or(Error::Corrupt)?;
    assert_eq!(
        loaded.steps[0]
            .control
            .as_ref()
            .and_then(|c| c.check.as_ref()),
        Some(&check)
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}
