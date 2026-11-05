use super::*;

// Port geometry refines the existing HasPort binding, never installs a port as
// another device. Replay is a no-op and removed bindings cannot retain anchors.
#[test]
fn port_anchors_preserve_binding_authority_and_do_not_amplify_replays() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Anchor owner")?;
    let port = create(&store, &actor, K::Port, "Measured port")?;
    call(
        &store,
        &actor,
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
    let binding = store
        .relations(AssetRef::try_from(&owner)?, 25)?
        .into_iter()
        .find(|r| r.kind == wire::RelationKind::HasPort as i32)
        .ok_or("port binding")?;
    let edit = api::PhysicalPortAnchorEdit {
        binding_id: binding.relation_id.clone(),
        expected_binding_revision: binding.revision,
        volume: Some(volume()),
    };
    let mut denied = actor.clone();
    denied.grants[0].write = Fields::Selected(vec!["name".into()]);
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &denied,
            api::physical::Action::PortAnchor(edit.clone())
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    physical(
        &store,
        &actor,
        api::physical::Action::PortAnchor(edit.clone()),
    )?;
    let saved = store.usage()?;
    physical(
        &store,
        &actor,
        api::physical::Action::PortAnchor(edit.clone()),
    )?;
    assert_eq!(store.usage()?, saved);
    let action = || {
        api::physical::Action::Scene(api::PhysicalSceneQuery {
            subjects: vec![owner.clone(), port.clone()],
        })
    };
    let scene = physical(&store, &actor, action())?;
    assert_eq!(scene.occupancy.len(), 1);
    let placement = scene.occupancy[0].placement.as_ref().ok_or("anchor")?;
    assert_eq!(placement.item, Some(port.clone()));
    assert_eq!(placement.enclosure, Some(owner.clone()));
    assert_eq!(placement.volume, edit.volume);
    assert_eq!(store.usage()?, saved);
    let mut stale = edit.clone();
    stale.expected_binding_revision += 1;
    assert!(matches!(
        physical(&store, &actor, api::physical::Action::PortAnchor(stale)),
        Err(Error::Conflict)
    ));
    call(
        &store,
        &actor,
        Command::Relation(api::Relationship {
            relation: Some(binding),
            remove: true,
        }),
    )?;
    assert!(physical(&store, &actor, action())?.occupancy.is_empty());
    assert!(matches!(
        physical(&store, &actor, api::physical::Action::PortAnchor(edit)),
        Err(Error::Missing)
    ));
    Ok(())
}

// Scene reads keep explicit scope and field rights, preserve actual poses and
// leave storage untouched. They never hydrate descendants not selected by UI.
#[test]
fn scene_is_current_scoped_authorized_and_read_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Scene rack")?;
    let server = create(&store, &actor, K::Device, "Scene server")?;
    let other = create(&store, &actor, K::Device, "Unselected device")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let plan = draft(&store, &actor, server.clone(), rack.clone())?;
    report_steps(&store, &actor, &plan, vec![0])?;
    let before = store.usage()?;
    let action = || {
        api::physical::Action::Scene(api::PhysicalSceneQuery {
            subjects: vec![rack.clone(), server.clone()],
        })
    };
    let view = physical(&store, &actor, action())?;
    assert_eq!(view.items.len(), 2);
    assert_eq!(view.definitions.len(), 1);
    assert_eq!(view.definitions[0].target, Some(rack.clone()));
    assert_eq!(view.occupancy.len(), 1);
    assert_eq!(
        view.occupancy[0]
            .placement
            .as_ref()
            .ok_or("placement")?
            .item,
        Some(server.clone())
    );
    assert!(view.items.iter().all(|i| i.target != Some(other.clone())));
    let lone = physical(
        &store,
        &actor,
        api::physical::Action::Scene(api::PhysicalSceneQuery {
            subjects: vec![rack.clone()],
        }),
    )?;
    assert_eq!(lone.items.len(), 1);
    assert!(lone.occupancy.is_empty());
    let mut limited = actor.clone();
    limited.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(matches!(
        physical(&store, &limited, action()),
        Err(Error::Denied)
    ));
    let result = store.workspace(
        &actor,
        api::Request {
            working: Some(api::WorkingContext {
                roots: vec![other],
                ..Default::default()
            }),
            command: Some(Command::Physical(api::Physical {
                action: Some(action()),
            })),
        },
        100,
    );
    assert!(matches!(result, Err(Error::Denied)));
    assert_eq!(store.usage()?, before);
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Scene(api::PhysicalSceneQuery {
                subjects: vec![rack.clone(); 129]
            })
        ),
        Err(Error::Limit)
    ));
    Ok(())
}

// Only current actual named-end attachments can contribute cable diagrams.
// A reader of one port cannot discover the remote port or a hidden cable.
#[test]
fn scene_cables_do_not_reveal_remote_or_hidden_endpoints() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "Scene cable")?;
    let a = create(&store, &actor, K::Port, "Near port")?;
    let b = create(&store, &actor, K::Port, "Remote port")?;
    let mut steps = Vec::new();
    for (key, port) in [("a", &a), ("b", &b)] {
        let end = physical(
            &store,
            &actor,
            api::physical::Action::CableEnd(api::PhysicalCableEndEdit {
                cable: Some(cable.clone()),
                end: Some(api::PhysicalCableEnd {
                    key: key.into(),
                    name: key.into(),
                    basis: "Inspected named end".into(),
                    ..Default::default()
                }),
                expected: None,
            }),
        )?
        .cable_ends
        .pop()
        .ok_or("end")?;
        steps.push(api::PhysicalStep {
            action: Some(api::physical_step::Action::Wiring(api::PhysicalWiring {
                cable: Some(cable.clone()),
                end_key: key.into(),
                basis: Some(api::physical_wiring::Basis::ExpectedEndRevision(
                    end.revision,
                )),
                after: Some(api::physical_wiring::After::Port(port.clone())),
                intent: api::PhysicalWiringIntent::Preserve as i32,
                ..Default::default()
            })),
            ..Default::default()
        });
    }
    let plan = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(api::PhysicalPlan {
                name: "Cable inspection".into(),
                steps,
                ..Default::default()
            }),
        }),
    )?
    .plan
    .ok_or("plan")?;
    report_steps(&store, &actor, &plan, vec![0, 1])?;
    let action = || {
        api::physical::Action::Scene(api::PhysicalSceneQuery {
            subjects: vec![a.clone()],
        })
    };
    let before = store.usage()?;
    let view = physical(&store, &actor, action())?;
    assert_eq!(view.scene_connections.len(), 1);
    assert_eq!(view.scene_connections[0].port, Some(a.clone()));
    assert!(view.items.iter().all(|i| i.target != Some(b.clone())));
    assert!(view.items.iter().any(|i| i.target == Some(cable.clone())));
    let mut limited = actor.clone();
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&a)?]);
    let hidden = physical(&store, &limited, action())?;
    assert!(hidden.scene_connections.is_empty());
    assert_eq!(hidden.items.len(), 1);
    assert_eq!(store.usage()?, before);
    let both = || {
        api::physical::Action::Scene(api::PhysicalSceneQuery {
            subjects: vec![a.clone(), b.clone()],
        })
    };
    assert_eq!(
        physical(&store, &actor, both())?.two_end_cables,
        vec![cable.clone()]
    );
    physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(api::PhysicalCableEndEdit {
            cable: Some(cable.clone()),
            end: Some(api::PhysicalCableEnd {
                key: "unselected-leg".into(),
                name: "Additional end".into(),
                basis: "Inspected third end".into(),
                ..Default::default()
            }),
            expected: None,
        }),
    )?;
    let partial = physical(&store, &actor, both())?;
    assert_eq!(partial.scene_connections.len(), 2);
    assert!(partial.two_end_cables.is_empty());
    Ok(())
}
