use super::*;

// Reordered or repeated roots describe the same disclosure scope and one current contribution.
#[test]
fn replacement_feedback_scope_canonicalizes_root_identity_sets() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Enclosure")?;
    let (slot, old) = mountable_component_fixture(&store, &actor, owner.clone(), "disk-bay")?;
    let incoming = create(
        &store,
        &actor,
        K::PhysicalComponent,
        "Unqualified replacement",
    )?;
    let previous = binding(&store, &old)?;
    let plan = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(slot.clone()),
            previous: Some(old.clone()),
            replacement: Some(incoming.clone()),
            previous_binding: Some(previous.relation_id),
            previous_revision: Some(previous.revision),
            ..Default::default()
        }],
    )?;
    let scoped = |actor: &Context,
                  roots: Vec<wire::AssetRef>,
                  action: api::physical::Action|
     -> Result<api::PhysicalView, Error> {
        let revision = physical(
            &store,
            actor,
            api::physical::Action::List(api::PhysicalPlanList::default()),
        )?
        .revision;
        let reply = store.workspace(
            actor,
            api::Request {
                command: Some(Command::Physical(api::Physical {
                    action: Some(action),
                })),
                working: Some(api::WorkingContext {
                    roots,
                    expected: Some(revision),
                    ..Default::default()
                }),
            },
            100,
        )?;
        match reply.result {
            Some(Reply::Physical(view)) => Ok(view),
            _ => Err(Error::Corrupt),
        }
    };
    let roots = vec![owner.clone(), slot, old, incoming];
    let inspect = |plan: &api::PhysicalPlan| api::PhysicalReplacementReview {
        plan: Some(api::PhysicalPlanKey {
            id: plan.id.clone().expect("saved plan"),
            revision: plan.revision,
        }),
        step: 0,
        ..Default::default()
    };
    let explanation = scoped(
        &actor,
        roots.clone(),
        api::physical::Action::ReviewReplacement(inspect(&plan)),
    )?;
    let judgment = api::PhysicalReplacementReview {
        decision: Some(api::PhysicalReplacementDecision::Confirmed as i32),
        basis: Some(
            explanation
                .replacement_hypothesis
                .ok_or(Error::Corrupt)?
                .basis,
        ),
        ..inspect(&plan)
    };
    let accepted = scoped(
        &actor,
        roots.clone(),
        api::physical::Action::ReviewReplacement(judgment),
    )?
    .plan
    .ok_or(Error::Corrupt)?;
    let mut reversed = roots.clone();
    reversed.reverse();
    let quality_action = || {
        api::physical::Action::ReplacementQuality(api::PhysicalReplacementQualityQuery::default())
    };
    let before = store.usage()?;
    let counts = scoped(&actor, reversed.clone(), quality_action())?;
    assert_eq!(
        counts
            .replacement_quality
            .iter()
            .map(|v| v.count)
            .sum::<u64>(),
        1
    );
    assert_eq!(store.usage()?, before);
    let explanation = scoped(
        &actor,
        reversed.clone(),
        api::physical::Action::ReviewReplacement(inspect(&accepted)),
    )?;
    let corrected = api::PhysicalReplacementReview {
        decision: Some(api::PhysicalReplacementDecision::Rejected as i32),
        basis: Some(
            explanation
                .replacement_hypothesis
                .ok_or(Error::Corrupt)?
                .basis,
        ),
        ..inspect(&accepted)
    };
    scoped(
        &actor,
        reversed.clone(),
        api::physical::Action::ReviewReplacement(corrected),
    )?;
    reversed.extend(roots.clone());
    let counts = scoped(&actor, reversed, quality_action())?.replacement_quality;
    assert_eq!(counts.iter().map(|v| v.count).sum::<u64>(), 1);
    assert!(
        counts
            .iter()
            .all(|v| v.decision == api::PhysicalReplacementDecision::Rejected as i32)
    );
    assert!(
        scoped(&actor, vec![owner.clone()], quality_action())?
            .replacement_quality
            .is_empty()
    );
    let mut changed = actor.clone();
    changed.authority_revision += 1;
    assert!(
        scoped(&changed, roots, quality_action())?
            .replacement_quality
            .is_empty()
    );
    assert!(matches!(
        scoped(&actor, vec![owner; MAX_BULK + 1], quality_action()),
        Err(Error::Limit)
    ));
    Ok(())
}

fn review(
    store: &Store,
    actor: &Context,
    plan: &api::PhysicalPlan,
    decision: Option<api::PhysicalReplacementDecision>,
    basis: Option<String>,
) -> Result<api::PhysicalView, Error> {
    physical(
        store,
        actor,
        api::physical::Action::ReviewReplacement(api::PhysicalReplacementReview {
            plan: Some(api::PhysicalPlanKey {
                id: plan.id.clone().ok_or(Error::Corrupt)?,
                revision: plan.revision,
            }),
            step: 0,
            decision: decision.map(|v| v as i32),
            basis,
            ..Default::default()
        }),
    )
}
fn quality(store: &Store, actor: &Context) -> Result<Vec<api::PhysicalReplacementQuality>, Error> {
    Ok(physical(
        store,
        actor,
        api::physical::Action::ReplacementQuality(api::PhysicalReplacementQualityQuery::default()),
    )?
    .replacement_quality)
}

// One current contribution survives retry and correction, without treating deferral as rejection.
#[test]
fn replacement_feedback_is_atomic_scoped_and_expires_without_relabelling() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let owner = create(&store, &actor, K::Device, "Chassis")?;
    let position = position_fixture(
        &store,
        &actor,
        owner,
        "slot/1",
        api::PhysicalComponentKind::Disk,
    )?;
    let old = create(&store, &actor, K::PhysicalComponent, "Previous disk")?;
    let incoming = create(
        &store,
        &actor,
        K::PhysicalComponent,
        "Unqualified incoming disk",
    )?;
    install_component(&store, &actor, position.clone(), old.clone())?;
    let predecessor_binding = binding(&store, &old)?;
    let plan = replacement_plan(
        &store,
        &actor,
        vec![api::PhysicalReplacement {
            position: Some(position.clone()),
            previous: Some(old),
            replacement: Some(incoming.clone()),
            previous_binding: Some(predecessor_binding.relation_id),
            previous_revision: Some(predecessor_binding.revision),
            ..Default::default()
        }],
    )?;
    let usage = store.usage()?;
    let explanation = review(&store, &actor, &plan, None, None)?;
    assert_eq!(
        store.usage()?,
        usage,
        "Explaining a hypothesis does not write"
    );
    let hypothesis = explanation.replacement_hypothesis.ok_or("hypothesis")?;
    assert_eq!(
        hypothesis.strength,
        api::PhysicalReplacementStrength::Contextual as i32
    );
    let mut accepted = review(
        &store,
        &actor,
        &plan,
        Some(api::PhysicalReplacementDecision::Confirmed),
        Some(hypothesis.basis.clone()),
    )?
    .plan
    .ok_or("plan")?;
    assert_eq!(
        quality(&store, &actor)?
            .iter()
            .map(|v| v.count)
            .sum::<u64>(),
        1
    );
    let usage = store.usage()?;
    let mut auditor = actor.clone();
    auditor.actor = "independent-read-only-auditor".into();
    auditor.fixture = "audit".into();
    auditor.grants[0].writable = false;
    auditor.grants[0].write = Fields::Selected(Vec::new());
    assert_eq!(
        quality(&store, &auditor)?
            .iter()
            .map(|v| v.count)
            .sum::<u64>(),
        1,
        "A reader authorized for the complete contributing scope can inspect its aggregate without being its author"
    );
    let repeated = review(
        &store,
        &actor,
        &accepted,
        Some(api::PhysicalReplacementDecision::Confirmed),
        Some(hypothesis.basis.clone()),
    )?
    .plan
    .ok_or("plan")?;
    assert_eq!(repeated.revision, accepted.revision);
    assert_eq!(store.usage()?, usage);
    // Removing and reintroducing a draft event replaces its contribution, never accumulates it.
    let original_steps = accepted.steps.clone();
    accepted.steps.clear();
    accepted.materials.push(api::PhysicalMaterial {
        name: "Remaining unplaced demand".into(),
        quantity: 1,
        kind: K::Device as i32,
        ..Default::default()
    });
    let removed = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(accepted),
        }),
    )?
    .plan
    .ok_or("plan")?;
    assert!(quality(&store, &actor)?.is_empty());
    let mut restored_event = removed;
    restored_event.steps = original_steps;
    let Some(api::physical_step::Action::Replacement(r)) = &mut restored_event.steps[0].action
    else {
        return Err("replacement".into());
    };
    r.feedback = None;
    let restored_event = physical(
        &store,
        &actor,
        api::physical::Action::Save(api::PhysicalSave {
            plan: Some(restored_event),
        }),
    )?
    .plan
    .ok_or("plan")?;
    accepted = review(
        &store,
        &actor,
        &restored_event,
        Some(api::PhysicalReplacementDecision::Confirmed),
        Some(hypothesis.basis.clone()),
    )?
    .plan
    .ok_or("plan")?;
    assert_eq!(
        quality(&store, &actor)?
            .iter()
            .map(|v| v.count)
            .sum::<u64>(),
        1
    );
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &image, policy(), image.len() as u64)?;
    assert!(
        matches!(quality(&restored, &actor), Err(Error::RecoveryRequired)),
        "A restored aggregate cannot claim current disclosure or correction state"
    );
    assert!(
        matches!(binding(&store, &incoming), Err(Error::Corrupt)),
        "A decision never installs the incoming part"
    );
    let corrected = review(
        &store,
        &actor,
        &accepted,
        Some(api::PhysicalReplacementDecision::Deferred),
        Some(hypothesis.basis.clone()),
    )?
    .plan
    .ok_or("plan")?;
    let counts = quality(&store, &actor)?;
    assert_eq!(counts.len(), 1);
    assert_eq!(
        counts[0].decision,
        api::PhysicalReplacementDecision::Deferred as i32
    );
    assert_eq!(counts[0].count, 1);
    let mut revoked = actor.clone();
    revoked.authority_revision += 1;
    assert!(quality(&store, &revoked)?.is_empty());
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(quality(&store, &restricted)?.is_empty());
    assert!(matches!(
        review(
            &store,
            &actor,
            &accepted,
            Some(api::PhysicalReplacementDecision::Rejected),
            Some(hypothesis.basis.clone())
        ),
        Err(Error::Conflict)
    ));
    let until = counts[0].until_ms;
    let request = api::Request {
        command: Some(Command::Physical(api::Physical {
            action: Some(api::physical::Action::ReplacementQuality(
                api::PhysicalReplacementQualityQuery::default(),
            )),
        })),
        working: Some(api::WorkingContext::default()),
    };
    let Some(Reply::Physical(expired)) = store.workspace(&actor, request, until)?.result else {
        return Err("quality".into());
    };
    assert!(
        expired.replacement_quality.is_empty(),
        "Expired buckets are hidden before cleanup"
    );
    let mut cursor = None;
    loop {
        let pass = store.cleanup(until, cursor.as_deref())?;
        cursor = pass.cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        quality(&store, &actor)?.is_empty(),
        "Cleanup actually removes the bucket"
    );
    let late = api::Request {
        command: Some(Command::Physical(api::Physical {
            action: Some(api::physical::Action::ReviewReplacement(
                api::PhysicalReplacementReview {
                    plan: Some(api::PhysicalPlanKey {
                        id: corrected.id.ok_or("id")?,
                        revision: corrected.revision,
                    }),
                    step: 0,
                    decision: Some(api::PhysicalReplacementDecision::Rejected as i32),
                    basis: Some(hypothesis.basis),
                    ..Default::default()
                },
            )),
        })),
        working: Some(api::WorkingContext::default()),
    };
    assert!(
        matches!(store.workspace(&actor, late, until), Err(Error::Expired)),
        "Expiry cannot silently count the same event again"
    );
    Ok(())
}
