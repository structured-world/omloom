use super::*;

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
