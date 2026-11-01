use super::*;

// A resolved step's released lock may be reused without blocking the original plan's remaining work.
#[test]
fn resolved_resource_reuse_does_not_block_completion_recheck_or_cancellation() -> TestResult {
    for cancel in [false, true] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let a = create(&store, &actor, K::Rack, "First enclosure")?;
        let b = create(&store, &actor, K::Rack, "Second enclosure")?;
        for rack in [&a, &b] {
            physical(
                &store,
                &actor,
                api::physical::Action::Define(api::PhysicalDefinitionEdit {
                    definition: Some(define(rack.clone())),
                }),
            )?;
        }
        let first = create(&store, &actor, K::Device, "First installation")?;
        let second = create(&store, &actor, K::Device, "Independent installation")?;
        for item in [&first, &second] {
            equipment_dimensions(&store, &actor, item)?;
        }
        let mut plan = draft(&store, &actor, first.clone(), a.clone())?;
        let mut step = plan.steps[0].clone();
        let Some(api::physical_step::Action::Placement(p)) = &mut step.action else {
            return Err(Error::Corrupt.into());
        };
        p.item = Some(second);
        p.enclosure = Some(b);
        plan.steps.push(step);
        let plan = physical(
            &store,
            &actor,
            api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
        )?
        .plan
        .ok_or(Error::Corrupt)?;
        let plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
        let mut plan = report_steps(&store, &actor, &plan, vec![0])?;
        let old = binding(&store, &first)?;
        let removal = api::PhysicalReplacement {
            position: Some(a),
            previous: Some(first.clone()),
            previous_binding: Some(old.relation_id),
            previous_revision: Some(old.revision),
            ..Default::default()
        };
        let next = replacement_plan(&store, &actor, vec![removal.clone()])?;
        let next = reserve(&store, &actor, next)?.plan.ok_or(Error::Corrupt)?;
        let rechecked = physical(
            &store,
            &actor,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: plan.id.clone().ok_or(Error::Corrupt)?,
                    revision: plan.revision,
                }),
                ..Default::default()
            }),
        );
        assert!(
            rechecked.is_ok(),
            "a reused lock must not block a postcondition recheck"
        );
        plan = rechecked?.plan.ok_or(Error::Corrupt)?;
        if cancel {
            plan = physical(
                &store,
                &actor,
                api::physical::Action::Cancel(api::PhysicalPlanKey {
                    id: plan.id.clone().ok_or(Error::Corrupt)?,
                    revision: plan.revision,
                }),
            )?
            .plan
            .ok_or(Error::Corrupt)?;
        }
        let resolved = physical(
            &store,
            &actor,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: plan.id.ok_or(Error::Corrupt)?,
                    revision: plan.revision,
                }),
                completed_steps: if cancel { vec![] } else { vec![1] },
                not_performed_steps: if cancel { vec![1] } else { vec![] },
                ..Default::default()
            }),
        )?
        .plan
        .ok_or(Error::Corrupt)?;
        assert_eq!(
            resolved.steps[0].state,
            api::PhysicalStepState::Recorded as i32
        );
        assert_eq!(
            resolved.steps[1].state,
            if cancel {
                api::PhysicalStepState::NotPerformed
            } else {
                api::PhysicalStepState::Recorded
            } as i32
        );
        let competitor = replacement_plan(&store, &actor, vec![removal])?;
        let before = store.usage()?;
        assert!(matches!(
            reserve(&store, &actor, competitor),
            Err(Error::Conflict)
        ));
        assert_eq!(store.usage()?, before);
        assert_eq!(next.state, api::PhysicalPlanState::Reserved as i32);
    }
    Ok(())
}

// Large enclosures remain readable through bounded pages rather than failing the entire projection.
#[test]
fn occupancy_beyond_128_parts_returns_a_bounded_first_page() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Large enclosure")?;
    for index in 0..129 {
        let item = create(
            &store,
            &actor,
            K::Device,
            &format!("Installed unit {index}"),
        )?;
        call(
            &store,
            &actor,
            Command::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(item),
                    to: Some(rack.clone()),
                    kind: wire::RelationKind::InstalledIn as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    let page = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: true,
            after: None,
        }),
    )?;
    assert_eq!(page.occupancy.len(), 25);
    assert!(page.next.is_some());
    let mut seen: BTreeSet<_> = page
        .occupancy
        .iter()
        .filter_map(|v| v.binding.as_ref().map(|v| v.relation_id.clone()))
        .collect();
    let first_cursor = page.next.clone();
    let mut after = page.next;
    let mut sizes = vec![page.occupancy.len()];
    while let Some(cursor) = after {
        let page = physical(
            &store,
            &actor,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack.clone()),
                occupants: true,
                after: Some(cursor),
            }),
        )?;
        sizes.push(page.occupancy.len());
        for value in page.occupancy {
            assert!(seen.insert(value.binding.ok_or(Error::Corrupt)?.relation_id));
        }
        after = page.next;
    }
    assert_eq!(sizes, vec![25, 25, 25, 25, 25, 4]);
    assert_eq!(seen.len(), 129);
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["physical".into(), "name".into()]);
    let private_page = physical(
        &store,
        &restricted,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: true,
            after: None,
        }),
    )?;
    assert_eq!(
        private_page
            .next
            .as_deref()
            .and_then(|v| v.rsplit(':').next())
            .and_then(|v| v.parse::<usize>().ok()),
        Some(25),
        "Continuation must not disclose a withheld binding ID"
    );
    assert!(
        private_page
            .occupancy
            .iter()
            .all(|v| v.binding.is_none() && v.revision == 0)
    );
    assert_eq!(store.usage()?, before);
    let mut changed = actor.clone();
    changed.authority_revision += 1;
    assert!(matches!(
        physical(
            &store,
            &changed,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack.clone()),
                occupants: true,
                after: first_cursor.clone()
            })
        ),
        Err(Error::Expired)
    ));
    create(&store, &actor, K::Device, "Revision change")?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack),
                occupants: true,
                after: first_cursor
            })
        ),
        Err(Error::Expired)
    ));
    Ok(())
}
