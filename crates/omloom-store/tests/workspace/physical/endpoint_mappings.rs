use super::*;
#[cfg(feature = "diagnostics")]
static PORT_BATCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn port(
    store: &Store,
    actor: &Context,
    owner: &wire::AssetRef,
    name: &str,
) -> Result<wire::AssetRef, Error> {
    let port = create(store, actor, K::Port, name)?;
    call(
        store,
        actor,
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(owner.clone()),
                to: Some(port.clone()),
                kind: wire::RelationKind::HasPort as i32,
                ..Default::default()
            }),
            remove: false,
        }),
    )?;
    Ok(port)
}

// Endpoint judgments are independent from a replacement event and never reparent ports.
#[test]
fn endpoint_mapping_is_bounded_persistent_and_does_not_rewire() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let chassis = create(&store, &actor, K::Device, "Chassis")?;
    let position = position_fixture(
        &store,
        &actor,
        chassis,
        "card/1",
        api::PhysicalComponentKind::LineCard,
    )?;
    let old = create(&store, &actor, K::PhysicalComponent, "Old line card")?;
    let new = create(
        &store,
        &actor,
        K::PhysicalComponent,
        "Different incoming model",
    )?;
    let a = port(&store, &actor, &old, "Old port 1")?;
    let b = port(&store, &actor, &old, "Old port 2")?;
    let c = port(&store, &actor, &new, "New port 1")?;
    install_component(&store, &actor, position.clone(), old.clone())?;
    let previous = binding(&store, &old)?;
    let check = api::EvaluateRequirements {
        anchor: Some(old.clone()),
        nodes: vec![api::RequirementNode {
            expression: Some(api::requirement_node::Expression::Binding(
                api::BindingRequirement {
                    relation_id: previous.relation_id.clone(),
                    revision: previous.revision,
                    from: previous.from.clone(),
                    to: previous.to.clone(),
                    kind: previous.kind,
                },
            )),
        }],
        root: 0,
    };
    let evaluate = || -> Result<api::RequirementEvaluation, Error> {
        match call(&store, &actor, Command::EvaluateRequirements(check.clone()))? {
            Reply::RequirementEvaluation(value) => Ok(value),
            _ => Err(Error::Corrupt),
        }
    };
    assert_eq!(evaluate()?.state, api::RequirementState::Satisfied as i32);
    let replacement = api::PhysicalReplacement {
        position: Some(position.clone()),
        previous: Some(old.clone()),
        replacement: Some(new.clone()),
        previous_binding: Some(previous.relation_id),
        previous_revision: Some(previous.revision),
        endpoints: vec![
            api::PhysicalEndpointMapping {
                previous: Some(a.clone()),
                replacement: Some(c.clone()),
                decision: api::PhysicalMappingDecision::Accepted as i32,
                previous_channel: None,
                replacement_channel: None,
            },
            api::PhysicalEndpointMapping {
                previous: Some(b.clone()),
                replacement: None,
                decision: api::PhysicalMappingDecision::Unresolved as i32,
                previous_channel: None,
                replacement_channel: None,
            },
        ],
        ..Default::default()
    };
    let plan = replacement_plan(&store, &actor, vec![replacement.clone()])?;
    let before = store.usage()?;
    let same = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(plan.clone()),
        }),
    )?
    .plan
    .ok_or("plan")?;
    assert_eq!(same.revision, plan.revision);
    assert_eq!(
        store.usage()?,
        before,
        "Unchanged endpoint review has no durable write"
    );
    let mut duplicate = replacement.clone();
    duplicate.endpoints[1].replacement = Some(c.clone());
    duplicate.endpoints[1].decision = api::PhysicalMappingDecision::Accepted as i32;
    assert!(matches!(
        replacement_plan(&store, &actor, vec![duplicate]),
        Err(Error::Conflict)
    ));
    let mut foreign = replacement;
    foreign.endpoints[0].replacement = Some(b.clone());
    assert!(matches!(
        replacement_plan(&store, &actor, vec![foreign]),
        Err(Error::Invalid)
    ));
    let done = report_steps(&store, &actor, &plan, vec![0])?;
    assert_eq!(done.state, api::PhysicalPlanState::Completed as i32);
    assert_eq!(binding(&store, &new)?.to, Some(position));
    let usage = store.usage()?;
    assert_eq!(
        evaluate()?.state,
        api::RequirementState::Failed as i32,
        "The shared evaluator rejects a stale physical realization after replacement"
    );
    assert_eq!(
        store.usage()?,
        usage,
        "Checking changed path evidence is read-only"
    );
    for (owner, child) in [(&old, &a), (&old, &b), (&new, &c)] {
        assert!(
            store
                .relations(AssetRef::try_from(owner)?, 32)?
                .iter()
                .any(|r| r.kind == wire::RelationKind::HasPort as i32
                    && r.to.as_ref() == Some(child))
        );
    }
    let key = api::PhysicalPlanKey {
        id: done.id.ok_or("id")?,
        revision: done.revision,
    };
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let loaded = physical(&store, &actor, api::physical::Action::Load(key))?
        .plan
        .ok_or("reopened plan")?;
    let Some(api::physical_step::Action::Replacement(value)) = &loaded.steps[0].action else {
        return Err("replacement".into());
    };
    assert_eq!(value.endpoints.len(), 2);
    assert_eq!(
        value.endpoints[1].decision,
        api::PhysicalMappingDecision::Unresolved as i32
    );
    let mut denied = actor;
    denied.grants = vec![Grant {
        scope: Scope::All,
        read: Fields::Selected(vec!["name".into(), "relations".into()]),
        write: Fields::All,
        writable: true,
    }];
    assert!(matches!(
        physical(
            &store,
            &denied,
            api::physical::Action::Load(api::PhysicalPlanKey {
                id: loaded.id.ok_or("id")?,
                revision: 0
            })
        ),
        Err(Error::Denied)
    ));
    Ok(())
}

// A smaller replacement keeps excess endpoints explicit and paginates direct hardware reads.
#[test]
fn cross_model_mapping_preserves_unmapped_ports_and_requires_declared_breakout_lanes() -> TestResult
{
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    #[cfg(feature = "diagnostics")]
    let mut store = store;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Modular owner")?;
    let position = position_fixture(
        &store,
        &actor,
        owner,
        "switch/1",
        api::PhysicalComponentKind::Chassis,
    )?;
    let old = create(&store, &actor, K::Device, "48-port switch")?;
    let new = create(&store, &actor, K::Device, "24-port replacement")?;
    let mut old_ports = Vec::new();
    let mut new_ports = Vec::new();
    for index in 0..48 {
        old_ports.push(port(&store, &actor, &old, &format!("Old {index:02}"))?);
    }
    for index in 0..24 {
        new_ports.push(port(&store, &actor, &new, &format!("New {index:02}"))?);
    }
    let query = api::Query {
        roots: vec![old.clone()],
        kinds: vec![K::Port as i32],
        view: api::View::Hardware as i32,
        level_only: true,
        limit: 25,
        columns: vec!["name".into()],
        ..Default::default()
    };
    let Reply::Page(first) = call(&store, &actor, Command::Query(query.clone()))? else {
        return Err("page".into());
    };
    assert_eq!(first.total, 48);
    assert_eq!(first.items.len(), 25);
    let Reply::Page(second) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            cursor: first.next,
            ..query
        }),
    )?
    else {
        return Err("page".into());
    };
    assert_eq!(second.items.len(), 23);
    let mut replacement = api::PhysicalReplacement {
        position: Some(position),
        previous: Some(old),
        replacement: Some(new),
        endpoints: old_ports
            .iter()
            .enumerate()
            .map(|(index, old)| api::PhysicalEndpointMapping {
                previous: Some(old.clone()),
                replacement: new_ports.get(index).cloned(),
                decision: if index < 24 {
                    api::PhysicalMappingDecision::Accepted as i32
                } else {
                    api::PhysicalMappingDecision::Unresolved as i32
                },
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    #[cfg(feature = "diagnostics")]
    {
        PORT_BATCHES.store(0, std::sync::atomic::Ordering::Relaxed);
        store.set_diagnostic_query_check(|db, statement| {
            if statement.starts_with("UNWIND $ports AS port") {
                PORT_BATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let plan = db
                    .explain_plan(statement, None)
                    .expect("endpoint batch plan")
                    .explain();
                assert!(
                    plan.contains("IndexScan") && !plan.contains("Traverse"),
                    "Endpoint membership must use indexed probes, not a topology walk: {plan}"
                );
            }
        });
    }
    let plan = replacement_plan(&store, &actor, vec![replacement.clone()])?;
    #[cfg(feature = "diagnostics")]
    assert_eq!(
        PORT_BATCHES.load(std::sync::atomic::Ordering::Relaxed),
        2,
        "Two supplier batches, independent of the 48-port map size"
    );
    let Some(api::physical_step::Action::Replacement(saved)) = &plan.steps[0].action else {
        return Err("replacement".into());
    };
    assert_eq!(
        saved
            .endpoints
            .iter()
            .filter(|p| p.replacement.is_none())
            .count(),
        24
    );
    replacement.endpoints = vec![
        api::PhysicalEndpointMapping {
            previous: Some(old_ports[0].clone()),
            replacement: Some(new_ports[0].clone()),
            previous_channel: Some(1),
            decision: api::PhysicalMappingDecision::Accepted as i32,
            ..Default::default()
        },
        api::PhysicalEndpointMapping {
            previous: Some(old_ports[0].clone()),
            replacement: Some(new_ports[1].clone()),
            previous_channel: Some(2),
            decision: api::PhysicalMappingDecision::Accepted as i32,
            ..Default::default()
        },
    ];
    assert!(
        matches!(
            replacement_plan(&store, &actor, vec![replacement.clone()]),
            Err(Error::Incomplete)
        ),
        "A lane number cannot invent hardware support"
    );
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(old_ports[0].clone()),
                basis: "Vendor-qualified breakout port channels".into(),
                channels: vec![1, 2, 3, 4],
                ..Default::default()
            }),
        }),
    )?;
    assert!(replacement_plan(&store, &actor, vec![replacement.clone()]).is_ok());
    replacement.endpoints[0].replacement_channel = Some(3);
    assert!(matches!(
        replacement_plan(&store, &actor, vec![replacement.clone()]),
        Err(Error::Incomplete)
    ));
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(new_ports[0].clone()),
                basis: "Vendor-qualified destination breakout channels".into(),
                channels: vec![3],
                ..Default::default()
            }),
        }),
    )?;
    assert!(replacement_plan(&store, &actor, vec![replacement.clone()]).is_ok());
    replacement.endpoints[1].previous_channel = None;
    assert!(
        matches!(
            replacement_plan(&store, &actor, vec![replacement]),
            Err(Error::Conflict)
        ),
        "A whole-port allocation overlaps all of its lanes"
    );
    Ok(())
}
