use super::*;

// Reporting performed work must not acquire unreported or declined assets held by another plan.
#[test]
fn draft_actual_report_does_not_lock_other_steps() -> TestResult {
    for decline in [false, true] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let a = create(&store, &actor, K::Rack, "Reported enclosure")?;
        let b = create(&store, &actor, K::Rack, "Other enclosure")?;
        for rack in [&a, &b] {
            physical(
                &store,
                &actor,
                api::physical::Action::Define(api::PhysicalDefinitionEdit {
                    definition: Some(define(rack.clone())),
                }),
            )?;
        }
        let first = create(&store, &actor, K::Device, "Performed installation")?;
        let second = create(&store, &actor, K::Device, "Separately reserved item")?;
        for item in [&first, &second] {
            equipment_dimensions(&store, &actor, item)?;
        }
        let mut plan = draft(&store, &actor, first.clone(), a.clone())?;
        let mut step = plan.steps[0].clone();
        let Some(api::physical_step::Action::Placement(p)) = &mut step.action else {
            return Err(Error::Corrupt.into());
        };
        p.item = Some(second.clone());
        p.enclosure = Some(b.clone());
        plan.steps.push(step);
        let plan = physical(
            &store,
            &actor,
            api::physical::Action::Save(api::PhysicalSave { plan: Some(plan) }),
        )?
        .plan
        .ok_or(Error::Corrupt)?;
        let other = draft(&store, &actor, second.clone(), b.clone())?;
        reserve(&store, &actor, other)?;
        let reported = physical(
            &store,
            &actor,
            api::physical::Action::Complete(api::PhysicalCompletion {
                plan: Some(api::PhysicalPlanKey {
                    id: plan.id.ok_or(Error::Corrupt)?,
                    revision: plan.revision,
                }),
                completed_steps: vec![0],
                not_performed_steps: if decline { vec![1] } else { vec![] },
                actual_state: true,
                discrepancy: Some("Only the first installation was performed".into()),
            }),
        )?
        .plan
        .ok_or(Error::Corrupt)?;
        assert_eq!(
            reported.steps[0].state,
            api::PhysicalStepState::Recorded as i32
        );
        assert_eq!(
            reported.steps[1].state,
            if decline {
                api::PhysicalStepState::NotPerformed
            } else {
                api::PhysicalStepState::Planned
            } as i32
        );
        assert_eq!(binding(&store, &first)?.to, Some(a));
        let third = draft(&store, &actor, second, b)?;
        let before = store.usage()?;
        assert!(matches!(
            reserve(&store, &actor, third),
            Err(Error::Conflict)
        ));
        assert_eq!(
            store.usage()?,
            before,
            "The other reservation remains authoritative"
        );
    }
    Ok(())
}

// Empty refreshes do not claim work started, bump revisions or write audit/history rows.
#[test]
fn empty_completion_keeps_unstarted_plans_unchanged() -> TestResult {
    for reserved in [true, false] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let rack = create(&store, &actor, K::Rack, "Unstarted enclosure")?;
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(define(rack.clone())),
            }),
        )?;
        let item = create(&store, &actor, K::Device, "Unstarted installation")?;
        equipment_dimensions(&store, &actor, &item)?;
        let mut plan = draft(&store, &actor, item, rack)?;
        if reserved {
            plan = reserve(&store, &actor, plan)?.plan.ok_or(Error::Corrupt)?;
        }
        for actual_state in [true, false] {
            if !reserved && !actual_state {
                continue;
            }
            let before = store.usage()?;
            let refreshed = physical(
                &store,
                &actor,
                api::physical::Action::Complete(api::PhysicalCompletion {
                    plan: Some(api::PhysicalPlanKey {
                        id: plan.id.clone().ok_or(Error::Corrupt)?,
                        revision: plan.revision,
                    }),
                    actual_state,
                    ..Default::default()
                }),
            )?
            .plan
            .ok_or(Error::Corrupt)?;
            assert_eq!(refreshed.state, plan.state);
            assert_eq!(refreshed.revision, plan.revision);
            assert_eq!(refreshed.steps[0].state, plan.steps[0].state);
            assert_eq!(store.usage()?, before);
        }
    }
    Ok(())
}

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

// Deduplication removes orphan poses; only an unchanged binding keeps geometry, and undo restores both.
#[test]
fn identity_correction_reclaims_removed_binding_poses_and_undo_restores_them() -> TestResult {
    for retained_installed in [true, false] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let actor = operator();
        let rack = create(&store, &actor, K::Rack, "Shared physical enclosure")?;
        let retained = create(&store, &actor, K::Device, "Retained identity")?;
        let first = create(&store, &actor, K::Device, "First duplicate identity")?;
        let second = if retained_installed {
            retained.clone()
        } else {
            create(&store, &actor, K::Device, "Second duplicate identity")?
        };
        for target in [&first, &second] {
            let plan = draft(&store, &actor, target.clone(), rack.clone()).expect("physical draft");
            report_steps(&store, &actor, &plan, vec![0]).expect("physical actual report");
        }
        let poses = || {
            store.diagnostic_database().execute_cypher_shared(
            "MATCH (n:OmRow {parent:'physical-poses'}) RETURN n.key AS key, n.x_length AS x, n.y_length AS y, n.z_length AS z, n.actor AS actor ORDER BY n.key LIMIT 8", None, None, None, None
        )
        };
        let before = poses()?.rows;
        assert_eq!(before.len(), 2);
        let correct = |request: api::IdentityCorrection| -> Result<api::reply::Result, Error> {
            store
                .workspace(
                    &actor,
                    api::Request {
                        working: Some(api::WorkingContext {
                            expected: Some(store.usage()?.revision),
                            ..Default::default()
                        }),
                        command: Some(Command::CorrectIdentity(request)),
                    },
                    100,
                )?
                .result
                .ok_or(Error::Corrupt)
        };
        let duplicates = if retained_installed {
            vec![first]
        } else {
            vec![first, second]
        };
        let mut mappings = Vec::new();
        for duplicate in duplicates {
            mappings.push(api::IdentityMapping {
                attachments: vec![binding(&store, &duplicate)?.relation_id],
                duplicate: Some(duplicate),
                retained: Some(retained.clone()),
                fields: vec![api::IdentityFieldChoice {
                    name: "name".into(),
                    use_duplicate: false,
                }],
                ..Default::default()
            });
        }
        let Reply::IdentityCorrection(preview) = correct(api::IdentityCorrection {
            decision: api::IdentityDecision::DuplicateInstallation as i32,
            mappings,
            ..Default::default()
        })
        .expect("identity correction preview") else {
            return Err(Error::Corrupt.into());
        };
        let Reply::IdentityCorrected(applied) = correct(api::IdentityCorrection {
            preview: Some(preview.id),
            ..Default::default()
        })
        .expect("identity correction apply") else {
            return Err(Error::Corrupt.into());
        };
        let current = poses()?.rows;
        assert_eq!(
            current.len(),
            usize::from(retained_installed),
            "Only unchanged retained bindings keep their measured geometry"
        );
        let survivor = binding(&store, &retained)?;
        if retained_installed {
            assert_eq!(
                current[0].get("key").and_then(|v| v.as_str()),
                Some(format!("physical-pose/{}", survivor.relation_id).as_str())
            );
            assert!(before.contains(&current[0]));
        }
        let projected = physical(
            &store,
            &actor,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack),
                occupants: true,
                after: None,
            }),
        )?;
        assert_eq!(projected.occupancy.len(), 1);
        let placement = projected.occupancy[0]
            .placement
            .as_ref()
            .ok_or(Error::Corrupt)?;
        assert_eq!(placement.item.as_ref(), Some(&retained));
        assert_eq!(
            placement.volume.is_some(),
            retained_installed,
            "A rewritten binding requires fresh geometry rather than transferring an old basis"
        );
        correct(api::IdentityCorrection {
            undo: Some(applied.id),
            ..Default::default()
        })?;
        let restored = poses()?.rows;
        assert_eq!(
            restored.len(),
            2,
            "Undo restores geometry after rebinding both original identities"
        );
        assert_eq!(
            restored, before,
            "Undo restores both poses and their original reporters"
        );
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
