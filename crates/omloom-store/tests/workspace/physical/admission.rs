use super::*;

// One predecessor cannot fund two distinct replacement steps or acquire their reservations.
#[test]
fn replacement_drafts_reject_duplicate_predecessor_bindings() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Chassis")?;
    let (source, old) = mountable_component_fixture(&store, &actor, owner.clone(), "first")?;
    let destination = position_fixture(
        &store,
        &actor,
        owner,
        "second",
        api::PhysicalComponentKind::Disk,
    )?;
    let previous = binding(&store, &old)?;
    let mut steps = Vec::new();
    for position in [&source, &destination] {
        let incoming = create(
            &store,
            &actor,
            K::PhysicalComponent,
            "Distinct incoming part",
        )?;
        steps.push(api::PhysicalStep {
            action: Some(api::physical_step::Action::Replacement(
                api::PhysicalReplacement {
                    position: Some(position.clone()),
                    previous: Some(old.clone()),
                    replacement: Some(incoming),
                    source_position: Some(source.clone()),
                    previous_binding: Some(previous.relation_id.clone()),
                    previous_revision: Some(previous.revision),
                    ..Default::default()
                },
            )),
            ..Default::default()
        });
    }
    let plan = api::PhysicalPlan {
        name: "Duplicate predecessor".into(),
        steps,
        ..Default::default()
    };
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Save(api::PhysicalSave {
                plan: Some(plan.clone())
            })
        ),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Preview(api::PhysicalPreview { plan: Some(plan) })
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(binding(&store, &old)?, previous);
    Ok(())
}

// Generic relation and asset APIs cannot strand a structural position descriptor.
#[test]
fn structural_positions_reject_generic_retirement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Chassis")?;
    for archived in [true, false] {
        let position = position_fixture(
            &store,
            &actor,
            owner.clone(),
            if archived { "retired" } else { "retyped" },
            api::PhysicalComponentKind::Disk,
        )?;
        let contains = store
            .relations(AssetRef::try_from(&owner)?, 32)?
            .into_iter()
            .find(|r| {
                r.to.as_ref() == Some(&position) && r.kind == wire::RelationKind::Contains as i32
            })
            .ok_or(Error::Corrupt)?;
        assert!(matches!(
            call(
                &store,
                &actor,
                Command::Relation(api::Relationship {
                    relation: Some(contains),
                    remove: true,
                }),
            ),
            Err(Error::Conflict)
        ));
        let mut asset = store.asset(AssetRef::try_from(&position)?)?;
        let revision = asset.revision;
        asset.revision += 1;
        if archived {
            asset.adoption = wire::Adoption::Archived as i32;
        } else {
            asset.kind = K::Device as i32;
        }
        assert!(matches!(
            store.save_asset(
                &admit(wire::record::Body::Asset(asset)),
                Some(revision),
                "fixture",
                100,
            ),
            Err(Error::Conflict)
        ));
    }
    let current = position_fixture(
        &store,
        &actor,
        owner.clone(),
        "current",
        api::PhysicalComponentKind::Disk,
    )?;
    let before = store.usage()?;
    let page = physical(
        &store,
        &actor,
        api::physical::Action::Positions(api::PhysicalPositionQuery {
            owner: Some(owner),
            limit: 25,
            ..Default::default()
        }),
    )?;
    assert_eq!(page.positions.len(), 3);
    assert!(
        page.positions
            .iter()
            .any(|p| p.target.as_ref() == Some(&current))
    );
    assert!(page.next.is_none());
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn rack_replacement_fixture(
    store: &Store,
    actor: &Context,
) -> Result<api::PhysicalReplacement, Error> {
    let rack = create(store, actor, K::Rack, "Replacement enclosure")?;
    let old = create(store, actor, K::Device, "Previous chassis")?;
    let incoming = create(store, actor, K::Device, "Incoming chassis")?;
    for item in [&old, &incoming] {
        equipment_dimensions(store, actor, item)?;
        let mut profile = define(item.clone());
        profile.mounting = Some("rack-19".into());
        profile.demand_mw = Some(20);
        profile.cooling_mw = Some(20);
        physical(
            store,
            actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(profile),
            }),
        )?;
    }
    let mut enclosure = define(rack.clone());
    enclosure.mounting = Some("rack-19".into());
    enclosure.capacity_mw = Some(20);
    enclosure.cooling_capacity_mw = Some(20);
    physical(
        store,
        actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(enclosure),
        }),
    )?;
    let installed = draft(store, actor, old.clone(), rack.clone())?;
    report_steps(store, actor, &installed, vec![0])?;
    let previous = binding(store, &old)?;
    Ok(api::PhysicalReplacement {
        position: Some(rack),
        previous: Some(old),
        replacement: Some(incoming),
        previous_binding: Some(previous.relation_id),
        previous_revision: Some(previous.revision),
        volume: Some(volume()),
        ..Default::default()
    })
}

// Explicit allocations can leave front/side service space before the incoming body.
#[test]
fn replacement_clearance_uses_the_proposed_allocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut replacement = rack_replacement_fixture(&store, &actor)?;
    let incoming = replacement.replacement.clone().ok_or(Error::Corrupt)?;
    let mut enclosure = define(replacement.position.clone().ok_or(Error::Corrupt)?);
    enclosure.mounting = Some("rack-19".into());
    enclosure.capacity_mw = Some(20);
    enclosure.cooling_capacity_mw = Some(20);
    enclosure
        .volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .length_um += 1;
    enclosure.occupancy_coverage = enclosure.volume.map(Box::new);
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(enclosure),
        }),
    )?;
    let mut item = define(incoming.clone());
    item.mounting = Some("rack-19".into());
    item.demand_mw = Some(20);
    item.cooling_mw = Some(20);
    item.clearance = Some(Box::new(api::PhysicalClearance {
        before_x_um: 1,
        ..Default::default()
    }));
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(item),
        }),
    )?;
    replacement
        .volume
        .as_mut()
        .ok_or(Error::Corrupt)?
        .x
        .as_mut()
        .ok_or(Error::Corrupt)?
        .start_um = 1;
    let plan = replacement_plan(&store, &actor, vec![replacement])?;
    let before = store.usage()?;
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(plan.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Satisfied as i32);
    assert_eq!(store.usage()?, before);
    let reserved = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let completed = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: reserved.id.ok_or(Error::Corrupt)?,
                revision: reserved.revision,
            }),
            completed_steps: vec![0],
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(completed.state, api::PhysicalPlanState::Completed as i32);
    Ok(())
}

// Replacement allocations pass the same requested-coordinate checks as ordinary placements.
#[test]
fn replacement_preview_and_reservation_reject_outside_coordinates() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let base = rack_replacement_fixture(&store, &actor)?;
    for axis in 0..3 {
        for displacement in [-1, 1] {
            let mut replacement = base.clone();
            let pose = replacement.volume.as_mut().ok_or(Error::Corrupt)?;
            let extent = match axis {
                0 => &mut pose.x,
                1 => &mut pose.y,
                _ => &mut pose.z,
            };
            extent.as_mut().ok_or(Error::Corrupt)?.start_um = displacement;
            let plan = replacement_plan(&store, &actor, vec![replacement])?;
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
                reserve(&store, &actor, plan),
                Err(Error::Conflict)
            ));
            assert_eq!(store.usage()?, before);
        }
    }
    Ok(())
}

// Reservation is not proof that current enclosure budgets still admit the incoming body.
#[test]
fn rack_replacement_completion_rechecks_current_capacity() -> TestResult {
    assert_rack_replacement_readmission("power")?;
    assert_rack_replacement_readmission("cooling")
}

// A once-valid proposed pose must not become accepted occupancy after the enclosure shrinks.
#[test]
fn rack_replacement_completion_rechecks_current_envelope() -> TestResult {
    assert_rack_replacement_readmission("envelope")
}

fn assert_rack_replacement_readmission(change: &str) -> TestResult {
    for actual_state in [false, true] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let replacement = rack_replacement_fixture(&store, &actor)?;
        let old = replacement.previous.clone().ok_or(Error::Corrupt)?;
        let previous = binding(&store, &old)?;
        let plan = replacement_plan(&store, &actor, vec![replacement.clone()])?;
        let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
        let mut enclosure = define(replacement.position.clone().ok_or(Error::Corrupt)?);
        enclosure.mounting = Some("rack-19".into());
        enclosure.capacity_mw = (change == "power").then_some(10);
        enclosure.cooling_capacity_mw = (change == "cooling").then_some(10);
        if change == "envelope" {
            enclosure
                .volume
                .as_mut()
                .ok_or(Error::Corrupt)?
                .x
                .as_mut()
                .ok_or(Error::Corrupt)?
                .length_um = 100_000;
        }
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(enclosure),
            }),
        )?;
        let mut completion = api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.ok_or(Error::Corrupt)?,
                revision: plan.revision,
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
        assert_eq!(binding(&store, &old)?, previous);
        completion.actual_state = actual_state;
        if actual_state {
            completion.discrepancy = Some(
                "Performed replacement does not meet the revised enclosure constraints".into(),
            );
        } else {
            let mut restored = define(replacement.position.clone().ok_or(Error::Corrupt)?);
            restored.mounting = Some("rack-19".into());
            restored.capacity_mw = Some(20);
            restored.cooling_capacity_mw = Some(20);
            physical(
                &store,
                &actor,
                api::physical::Action::Define(api::PhysicalDefinitionEdit {
                    definition: Some(restored),
                }),
            )?;
        }
        physical(&store, &actor, api::physical::Action::Complete(completion))?;
        assert_eq!(
            binding(
                &store,
                replacement.replacement.as_ref().ok_or(Error::Corrupt)?
            )?
            .to,
            replacement.position
        );
    }
    Ok(())
}

// Changed mounting or usable space cannot be bypassed by completing a reserved slot replacement.
#[test]
fn component_completion_rechecks_current_mounting_and_envelope() -> TestResult {
    for change in ["mounting", "envelope"] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let owner = create(&store, &actor, K::Device, "Enclosure")?;
        let (position, item) = mountable_component_fixture(&store, &actor, owner, "drive")?;
        let old = binding(&store, &item)?;
        let plan = replacement_plan(
            &store,
            &actor,
            vec![api::PhysicalReplacement {
                position: Some(position.clone()),
                previous: Some(item.clone()),
                replacement: Some(item.clone()),
                previous_binding: Some(old.relation_id.clone()),
                previous_revision: Some(old.revision),
                ..Default::default()
            }],
        )?;
        let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
        let mut slot = define(position.clone());
        slot.mounting = Some(
            if change == "mounting" {
                "different-carrier"
            } else {
                "drive-carrier-v1"
            }
            .into(),
        );
        if change == "envelope" {
            slot.volume
                .as_mut()
                .ok_or(Error::Corrupt)?
                .x
                .as_mut()
                .ok_or(Error::Corrupt)?
                .length_um = 100_000;
        }
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(slot),
            }),
        )?;
        let mut completion = api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            completed_steps: vec![0],
            ..Default::default()
        };
        let before = store.usage()?;
        assert!(
            matches!(
                physical(
                    &store,
                    &actor,
                    api::physical::Action::Complete(completion.clone())
                ),
                Err(Error::Conflict)
            ),
            "{change}"
        );
        assert_eq!(store.usage()?, before);
        assert_eq!(binding(&store, &item)?, old);
        completion.actual_state = true;
        completion.discrepancy =
            Some("Recorded work does not meet the revised slot specification".into());
        physical(&store, &actor, api::physical::Action::Complete(completion))?;
        assert_eq!(binding(&store, &item)?.to, Some(position));
    }
    Ok(())
}

// Corrected slot facts do not authorize an incompatible admitted reinstall or erase actual work.
#[test]
fn changed_slot_class_is_rechecked_before_admitted_component_binding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Enclosure")?;
    let (position, item) = mountable_component_fixture(&store, &actor, owner.clone(), "disk-bay")?;
    let old = binding(&store, &item)?;
    let plan = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            previous: Some(item.clone()),
            replacement: Some(item.clone()),
            previous_binding: Some(old.relation_id.clone()),
            previous_revision: Some(old.revision),
            ..Default::default()
        }],
    )?;
    let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let descriptor = physical(
        &store,
        &actor,
        api::physical::Action::Positions(api::PhysicalPositionQuery {
            owner: Some(owner.clone()),
            ..Default::default()
        }),
    )?
    .positions
    .into_iter()
    .find(|p| p.target.as_ref() == Some(&position))
    .ok_or(Error::Corrupt)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Position(api::PhysicalPositionEdit {
            owner: Some(owner),
            structural_key: descriptor.structural_key,
            name: "Power supply bay".into(),
            component_kind: api::PhysicalComponentKind::PowerSupply as i32,
            expected: Some(descriptor.revision),
        }),
    )?;
    let mut report = api::PhysicalCompletion {
        plan: Some(api::PhysicalPlanKey {
            id: plan.id.ok_or(Error::Corrupt)?,
            revision: plan.revision,
        }),
        completed_steps: vec![0],
        ..Default::default()
    };
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Complete(report.clone())
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(binding(&store, &item)?, old);
    report.actual_state = true;
    report.discrepancy =
        Some("Recorded work needs reconciliation with the corrected slot class".into());
    physical(&store, &actor, api::physical::Action::Complete(report))?;
    assert_eq!(binding(&store, &item)?.to, Some(position));
    Ok(())
}

// Performed work remains recordable without consuming a different plan's future slot hold.
#[test]
fn actual_component_report_preserves_a_conflicting_future_reservation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Enclosure")?;
    let (position, item) = mountable_component_fixture(&store, &actor, owner, "disk-bay")?;
    let old = binding(&store, &item)?;
    let removal = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            previous: Some(item.clone()),
            previous_binding: Some(old.relation_id),
            previous_revision: Some(old.revision),
            ..Default::default()
        }],
    )?;
    report_steps(&store, &actor, &removal, vec![0])?;
    let future = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            replacement: Some(item),
            ..Default::default()
        }],
    )?;
    let future = reserve(&store, &actor, future)?
        .plan
        .ok_or(Error::Corrupt)?;
    let found = create(
        &store,
        &actor,
        K::PhysicalComponent,
        "Already installed component",
    )?;
    let actual = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            replacement: Some(found.clone()),
            ..Default::default()
        }],
    )?;
    physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: actual.id.ok_or(Error::Corrupt)?,
                revision: actual.revision,
            }),
            completed_steps: vec![0],
            actual_state: true,
            discrepancy: Some("Performed work discovered after the reservation".into()),
            ..Default::default()
        }),
    )?;
    assert_eq!(binding(&store, &found)?.to, Some(position.clone()));
    let held = store
        .diagnostic_database()
        .execute_cypher_shared(
            "MATCH (n:OmRow {key:$key}) RETURN n.owner AS owner",
            Some(std::collections::HashMap::from([(
                "key".into(),
                coordinode_core::graph::types::Value::String(format!(
                    "physical-slot/{}/{}",
                    position.asset_id, position.incarnation
                )),
            )])),
            None,
            None,
            None,
        )?
        .rows;
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0]
            .get("owner")
            .and_then(coordinode_core::graph::types::Value::as_str),
        future.id.as_deref()
    );
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: future.id.ok_or(Error::Corrupt)?,
                    revision: future.revision
                }),
                completed_steps: vec![0],
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Ordinary relation writes cannot occupy a held slot; the admitted plan retains its installation path.
#[test]
fn installed_in_writes_respect_the_physical_slot_reservation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Enclosure")?;
    let (position, item) = mountable_component_fixture(&store, &actor, owner, "disk-bay")?;
    let old = binding(&store, &item)?;
    let removal = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            previous: Some(item.clone()),
            previous_binding: Some(old.relation_id),
            previous_revision: Some(old.revision),
            ..Default::default()
        }],
    )?;
    report_steps(&store, &actor, &removal, vec![0])?;
    let plan = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            replacement: Some(item),
            ..Default::default()
        }],
    )?;
    let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let intruder = create(&store, &actor, K::PhysicalComponent, "Unplanned component")?;
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(intruder),
                    to: Some(position),
                    kind: wire::RelationKind::InstalledIn as i32,
                    ..Default::default()
                }),
                remove: false,
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let completed = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            completed_steps: vec![0],
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(completed.state, api::PhysicalPlanState::Completed as i32);
    Ok(())
}

// An occupied destination cannot be reserved unless its occupant is explicitly removed in the plan.
#[test]
fn component_move_admission_checks_destination_and_preserves_atomic_swaps() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Two-slot enclosure")?;
    let (a, first) = mountable_component_fixture(&store, &actor, owner.clone(), "disk/1")?;
    let (b, second) = mountable_component_fixture(&store, &actor, owner, "disk/2")?;
    let old = binding(&store, &first)?;
    let first_move = api::PhysicalReplacement {
        position: Some(b.clone()),
        source_position: Some(a.clone()),
        previous: Some(first.clone()),
        replacement: Some(first.clone()),
        previous_binding: Some(old.relation_id),
        previous_revision: Some(old.revision),
        ..Default::default()
    };
    let impossible = replacement_plan(&store, &actor, vec![first_move.clone()])?;
    let before = store.usage()?;
    let preview = physical(
        &store,
        &actor,
        api::physical::Action::Preview(api::PhysicalPreview {
            plan: Some(impossible.clone()),
        }),
    )?;
    assert_eq!(preview.state, api::RequirementState::Failed as i32);
    assert!(matches!(
        reserve(&store, &actor, impossible),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let old = binding(&store, &second)?;
    let swap = replacement_plan(
        &store,
        &actor,
        vec![
            first_move,
            api::PhysicalReplacement {
                position: Some(a.clone()),
                source_position: Some(b.clone()),
                previous: Some(second.clone()),
                replacement: Some(second.clone()),
                previous_binding: Some(old.relation_id),
                previous_revision: Some(old.revision),
                ..Default::default()
            },
        ],
    )?;
    let reserved = reserve(&store, &actor, swap)?.plan.ok_or(Error::Corrupt)?;
    let completed = physical(
        &store,
        &actor,
        api::physical::Action::Complete(api::PhysicalCompletion {
            plan: Some(api::PhysicalPlanKey {
                id: reserved.id.ok_or(Error::Corrupt)?,
                revision: reserved.revision,
            }),
            completed_steps: vec![0, 1],
            ..Default::default()
        }),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    assert_eq!(completed.state, api::PhysicalPlanState::Completed as i32);
    assert_eq!(binding(&store, &first)?.to, Some(b));
    assert_eq!(binding(&store, &second)?.to, Some(a));
    Ok(())
}

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
    let mut report = api::PhysicalCompletion {
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
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Complete(report.clone())
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    report.actual_state = true;
    physical(&store, &actor, api::physical::Action::Complete(report))?;
    let current = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: true,
            after: None,
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

// A serialized item cannot reuse an obsolete fit decision after its measured dimensions change.
#[test]
fn reserved_direct_item_is_readmitted_after_equipment_dimensions_change() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Enclosure")?;
    let item = create(&store, &actor, K::Device, "Serialized server")?;
    equipment_dimensions(&store, &actor, &item)?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let plan = draft(&store, &actor, item.clone(), rack)?;
    let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
    let current = store.asset(AssetRef::try_from(&item)?)?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(item.clone()),
            expected: Some(current.revision),
            kind: current.kind,
            changes: vec![equipment_quantity("width", 3_000_000, "mm", 3)],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: plan.id.ok_or(Error::Corrupt)?,
                    revision: plan.revision
                }),
                completed_steps: vec![0],
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert!(store.relations(AssetRef::try_from(&item)?, 32)?.is_empty());
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
                        material: None,
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
                material: None,
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
