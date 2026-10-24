use super::*;

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
