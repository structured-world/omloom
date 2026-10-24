use super::*;

// A host-wide backup may exceed Omloom's logical quota without changing its stored policy.
#[test]
fn shared_database_backup_restores_with_the_original_application_quota() -> TestResult {
    let source = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    let mut limits = policy();
    limits.total_bytes = 16_384;
    let db = coordinode_embed::Database::open(source.path())?;
    let payload = "x".repeat(65_536);
    db.execute_cypher_shared(
        "CREATE (n:HostData {payload:$payload})",
        Some(std::collections::HashMap::from([(
            "payload".into(),
            coordinode_core::graph::types::Value::String(payload.clone()),
        )])),
        None,
        None,
        None,
    )?;
    let store = Store::from_database(db, limits.clone())?;
    let object = asset();
    let target = target(&object);
    store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
    let mut image = Vec::new();
    store.export(&mut image)?;
    assert!(image.len() as u64 > limits.total_bytes);
    let max_backup_bytes = image.len() as u64;
    let refused = destination.path().join("over-budget");
    for ceiling in [0, max_backup_bytes - 1] {
        assert!(matches!(
            Store::restore(&refused, &image, limits.clone(), ceiling),
            Err(Error::Limit)
        ));
        assert!(
            !refused.exists(),
            "input admission must precede opening the database"
        );
    }
    let usage = store.usage()?;
    let restored = Store::restore(destination.path(), &image, limits, max_backup_bytes)?;
    assert_eq!(restored.usage()?.bytes, usage.bytes);
    assert!(restored.recovery_required()?);
    assert_eq!(restored.asset(target)?.revision, 1);
    let rows = restored
        .diagnostic_database()
        .execute_cypher_shared(
            "MATCH (n:HostData) RETURN n.payload AS payload",
            None,
            None,
            None,
            None,
        )?
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]
            .get("payload")
            .and_then(coordinode_core::graph::types::Value::as_str),
        Some(payload.as_str())
    );
    Ok(())
}

// Partial failure and operator attention preserve confirmed effects independently of success.
#[test]
fn partial_failure_preserves_confirmed_effect_knowledge() -> TestResult {
    for attention in [false, true] {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path(), policy())?;
        let object = asset();
        let target = target(&object);
        store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
        let id = store.accept(&plan(target, &[]), 2)?;
        let lease = store.claim(id, "worker", 10, 100)?;
        store.prepare_dispatch(&lease, 1, 11)?;
        if attention {
            store.reconcile(
                &lease,
                2,
                OperationState::NeedsAttention,
                EffectOutcome::Confirmed,
                Some("partial-task"),
                12,
            )?;
            let before = store.usage()?;
            for outcome in [EffectOutcome::Unknown, EffectOutcome::ConfirmedNoEffect] {
                assert!(matches!(
                    store.reconcile(&lease, 3, OperationState::NeedsAttention, outcome, None, 13),
                    Err(Error::UnsafeTransition)
                ));
                assert_eq!(store.usage()?, before);
            }
        }
        drop(store);
        let store = Store::open(directory.path(), policy())?;
        let revision = store.operation(id)?.revision;
        store.reconcile(
            &lease,
            revision,
            OperationState::Failed,
            EffectOutcome::Confirmed,
            Some("partial-task"),
            14,
        )?;
        let operation = store.operation(id)?;
        assert_eq!(operation.state, OperationState::Failed);
        assert_eq!(operation.outcome, EffectOutcome::Confirmed);
        assert_eq!(operation.provider_task.as_deref(), Some("partial-task"));
        assert!(store.pending(None)?.is_empty());
    }
    Ok(())
}

// A settled target becomes available immediately, while the previous epoch stays fenced.
#[test]
fn settled_lease_does_not_delay_the_next_operation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let object = asset();
    let target = target(&object);
    store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
    let first = store.accept(&plan(target, &[]), 2)?;
    let mut next_intent = plan(target, &[]);
    next_intent.retry_key = "next";
    let next = store.accept(&next_intent, 3)?;
    let old_lease = store.claim(first, "first", 10, 100)?;
    assert!(matches!(
        store.claim(next, "second", 11, 200),
        Err(Error::Conflict)
    ));
    store.prepare_dispatch(&old_lease, 1, 11)?;
    store.reconcile(
        &old_lease,
        2,
        OperationState::Succeeded,
        EffectOutcome::Confirmed,
        None,
        12,
    )?;
    drop(store);
    let store = Store::open(directory.path(), policy())?;
    let lease = store.claim(next, "second", 13, 200)?;
    assert!(lease.epoch > old_lease.epoch);
    assert!(matches!(
        store.reconcile(
            &old_lease,
            3,
            OperationState::Succeeded,
            EffectOutcome::Confirmed,
            None,
            14
        ),
        Err(Error::Conflict)
    ));
    store.prepare_dispatch(&lease, 1, 14)?;
    Ok(())
}

// Concurrent admissions cannot reuse the same outcome space; cancellation releases it.
#[test]
fn concurrent_admissions_cannot_share_settlement_capacity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut limits = policy();
    limits.classes[Class::Detail as usize].rows = 6;
    let store = Store::open(directory.path(), limits)?;
    let mut targets = Vec::new();
    for _ in 0..2 {
        let object = asset();
        targets.push(target(&object));
        store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
    }
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = targets
            .into_iter()
            .map(|target| {
                let barrier = &barrier;
                let store = &store;
                scope.spawn(move || {
                    let retry_key = target.asset.to_string();
                    let mut intent = plan(target, &[]);
                    intent.retry_key = &retry_key;
                    barrier.wait();
                    store.accept(&intent, 2)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("test worker"))
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .any(|result| matches!(result, Err(Error::Limit | Error::Conflict)))
    );
    let id = results
        .into_iter()
        .find_map(Result::ok)
        .expect("one admitted operation");
    assert_eq!(store.pending(None)?, vec![id]);
    let lease = store.claim(id, "worker", 10, 100)?;
    store.reconcile(
        &lease,
        1,
        OperationState::Cancelled,
        EffectOutcome::NotSent,
        None,
        11,
    )?;
    store.cleanup(2000, None)?;
    assert_eq!(store.usage()?.rows[Class::Detail as usize], 0);
    assert_eq!(store.usage()?.bytes[Class::Detail as usize], 0);
    Ok(())
}

// Admission must refuse an unfinishable operation before any external effect.
#[test]
fn detail_quota_cannot_admit_an_unfinishable_effect() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut limits = policy();
    limits.classes[Class::Detail as usize].rows = 2;
    let store = Store::open(directory.path(), limits)?;
    let object = asset();
    let target = target(&object);
    store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
    let before = store.usage()?;
    match store.accept(&plan(target, &[]), 2) {
        Err(Error::Limit) => {
            assert_eq!(store.usage()?, before);
            assert!(store.pending(None)?.is_empty());
        }
        Ok(id) => {
            store.cleanup(2000, None)?;
            let lease = store.claim(id, "worker", 2001, 3000)?;
            store.prepare_dispatch(&lease, 1, 2002)?;
            store.reconcile(
                &lease,
                2,
                OperationState::Succeeded,
                EffectOutcome::Confirmed,
                Some("task"),
                2003,
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

// Reserved outcome space survives quota exhaustion, restart and repeated recovery claims.
#[test]
fn settlement_reservation_survives_full_quotas_and_takeover() -> TestResult {
    for boundary in ["rows", "bytes", "total"] {
        let directory = tempfile::tempdir()?;
        let mut limits = policy();
        match boundary {
            "rows" => limits.classes[Class::Detail as usize].rows = 7,
            "bytes" => limits.classes[Class::Detail as usize].bytes = 12_000,
            "total" => limits.total_bytes = 20_000,
            _ => unreachable!(),
        }
        let store = Store::open(directory.path(), limits.clone())?;
        let object = asset();
        let target = target(&object);
        store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
        let caller = "c".repeat(256);
        let action = "a".repeat(64);
        let mut intent = plan(target, &[]);
        intent.caller = &caller;
        intent.action = &action;
        let id = store.accept(&intent, 2)?;
        let worker = "w".repeat(256);
        let lease = store.claim(id, &worker, 10, 100)?;
        let mut revision = store.prepare_dispatch(&lease, 1, 11)?;
        let mut filled = false;
        for now in 12..80 {
            let before = store.usage()?;
            match store.reconcile(
                &lease,
                revision,
                OperationState::NeedsAttention,
                EffectOutcome::Unknown,
                None,
                now,
            ) {
                Ok(next) => revision = next,
                Err(Error::Limit) => {
                    assert_eq!(store.usage()?, before);
                    filled = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        assert!(filled, "fixture must fill {boundary} capacity");
        assert_eq!(store.operation(id)?.state, OperationState::NeedsAttention);
        drop(store);
        let store = Store::open(directory.path(), limits)?;
        let recovery = store.claim(id, "recovery", 101, 200)?;
        assert_eq!(store.operation(id)?.state, OperationState::Reconciling);
        let operation = store.operation(id)?;
        let before = store.usage()?;
        assert!(matches!(
            store.reconcile(
                &recovery,
                operation.revision,
                OperationState::NeedsAttention,
                EffectOutcome::Unknown,
                None,
                102
            ),
            Err(Error::Limit)
        ));
        assert_eq!(store.usage()?, before);
        let next = store.claim(id, &worker, 201, 300)?;
        let operation = store.operation(id)?;
        let task = "t".repeat(256);
        store.reconcile(
            &next,
            operation.revision,
            OperationState::Succeeded,
            EffectOutcome::Confirmed,
            Some(&task),
            202,
        )?;
        assert!(next.epoch > recovery.epoch);
        assert_eq!(
            store.operation(id)?.provider_task.as_deref(),
            Some(task.as_str())
        );
        assert!(store.operation(id)?.arguments.is_empty());
        assert!(store.pending(None)?.is_empty());
        assert_eq!(store.accept(&intent, 203)?, id);
        store.cleanup(10_000, None)?;
        assert_eq!(store.usage()?.rows[Class::Detail as usize], 0);
        assert_eq!(store.usage()?.bytes[Class::Detail as usize], 0);
    }
    Ok(())
}

// Reconciliation may fill an unknown provider task, but cannot replace its identity.
#[test]
fn recorded_provider_task_cannot_be_replaced() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let object = asset();
    let target = target(&object);
    store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
    let id = store.accept(&plan(target, &[]), 2)?;
    let lease = store.claim(id, "worker", 10, 100)?;
    store.prepare_dispatch(&lease, 1, 11)?;
    store.reconcile(
        &lease,
        2,
        OperationState::Reconciling,
        EffectOutcome::Unknown,
        Some("task-a"),
        12,
    )?;
    let before = store.usage()?;
    assert!(matches!(
        store.reconcile(
            &lease,
            3,
            OperationState::Succeeded,
            EffectOutcome::Confirmed,
            Some("task-b"),
            13
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(
        store.operation(id)?.provider_task.as_deref(),
        Some("task-a")
    );
    drop(store);
    let store = Store::open(directory.path(), policy())?;
    store.reconcile(
        &lease,
        3,
        OperationState::NeedsAttention,
        EffectOutcome::Unknown,
        Some("task-a"),
        14,
    )?;
    store.reconcile(
        &lease,
        4,
        OperationState::Succeeded,
        EffectOutcome::Confirmed,
        None,
        15,
    )?;
    assert_eq!(
        store.operation(id)?.provider_task.as_deref(),
        Some("task-a")
    );
    Ok(())
}

// Unresolved work must not starve later targets when a recovery worker scans pages.
#[test]
fn pending_pages_reach_work_after_unresolved_operations() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut limits = policy();
    limits.batch_rows = 2;
    let store = Store::open(directory.path(), limits.clone())?;
    let mut accepted = Vec::new();
    for _ in 0..3 {
        let object = asset();
        let target = target(&object);
        store.save_asset(&admit(record::Body::Asset(object)), None, "admin", 1)?;
        let retry_key = target.asset.to_string();
        let mut intent = plan(target, &[]);
        intent.retry_key = &retry_key;
        accepted.push(store.accept(&intent, 2)?);
    }
    let first = store.pending(None)?;
    assert_eq!(first.len(), 2);
    for id in &first {
        let lease = store.claim(*id, "worker", 10, 100)?;
        store.prepare_dispatch(&lease, 1, 11)?;
        store.reconcile(
            &lease,
            2,
            OperationState::NeedsAttention,
            EffectOutcome::Unknown,
            None,
            12,
        )?;
    }
    drop(store);
    let store = Store::open(directory.path(), limits)?;
    let cursor = first.last().copied();
    let mut found = first;
    let last = store.pending(cursor)?;
    assert_eq!(last.len(), 1);
    assert!(store.pending(last.last().copied())?.is_empty());
    found.extend(last);
    found.sort_unstable();
    found.dedup();
    accepted.sort_unstable();
    assert_eq!(found, accepted);
    Ok(())
}
