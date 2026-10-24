#[path = "disk/metrics.rs"]
mod metrics;
mod support;
use omloom_contracts::{
    ids::*,
    wire::{EffectOutcome, OperationState, record},
};
use omloom_store::*;
use support::*;

// Identical input and current retrieval must not write or accumulate history.
#[test]
fn identical_input_reads_and_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    let record = admit(record::Body::Asset(asset.clone()));
    assert_eq!(store.save_asset(&record, None, "local-admin", 10)?, 1);
    let before = store.usage()?;
    for _ in 0..10 {
        assert_eq!(store.save_asset(&record, Some(1), "local-admin", 11)?, 1);
        assert_eq!(store.asset(target)?.revision, 1);
    }
    assert_eq!(store.usage()?, before);
    let plan = store.query_plan()?;
    eprintln!("{plan}");
    assert!(plan.contains("Index"), "current identity must use index");
    let children = store.diagnostic_database().explain_cypher(
        "MATCH (n:OmRow) WHERE n.parent='asset/example' RETURN properties(n) AS p LIMIT 2049",
    )?;
    eprintln!("children: {children}");
    assert!(
        children.contains("IndexScan"),
        "current fields must use parent index"
    );
    drop(store);
    let reopened = Store::open(dir.path(), policy())?;
    assert_eq!(reopened.asset(target)?.intended, asset.intended);
    assert_eq!(reopened.usage()?, before);
    Ok(())
}

// Freshening B cannot freshen omitted A, rewrite its value, or bump accepted intent.
#[test]
fn independent_partial_freshness_and_conflicting_sources() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut asset = asset();
    let target = target(&asset);
    let source = SourceId::generate().to_string();
    asset.observed = vec![observation(
        &source,
        100,
        vec![text_field("a", "old"), text_field("b", "current")],
    )];
    store.save_asset(
        &admit(record::Body::Asset(asset.clone())),
        None,
        "admin",
        100,
    )?;
    asset.observed = vec![observation(&source, 200, vec![text_field("b", "current")])];
    let value_key = format!("asset/{}/observed/{source}/b", target.asset);
    let before = row_version(&store, &value_key)?;
    assert_eq!(
        store.refresh_asset(
            &admit(record::Body::Asset(asset.clone())),
            1,
            store.usage()?.revision,
            "admin",
            200
        )?,
        1
    );
    assert_eq!(row_version(&store, &value_key)?, before);
    asset.observed = vec![observation(
        &SourceId::generate().to_string(),
        250,
        vec![text_field("b", "conflict")],
    )];
    store.refresh_asset(
        &admit(record::Body::Asset(asset)),
        1,
        store.usage()?.revision,
        "admin",
        250,
    )?;
    let current = store.asset(target)?;
    let fields: Vec<_> = current
        .observed
        .iter()
        .flat_map(|o| o.evidence.iter())
        .flat_map(|e| e.fields.iter().map(move |f| (&f.name, e.observed_at_ms)))
        .collect();
    assert!(fields.contains(&(&"a".to_string(), 100)));
    assert!(fields.contains(&(&"b".to_string(), 200)));
    assert!(fields.contains(&(&"b".to_string(), 250)));
    assert_eq!(current.revision, 1);
    assert_eq!(current.intended.len(), 1);
    Ok(())
}

// Database CAS admits one of two writers; a stale edit never overwrites the winner.
#[test]
fn stale_revision_and_concurrent_writers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let original = asset();
    store.save_asset(
        &admit(record::Body::Asset(original.clone())),
        None,
        "admin",
        1,
    )?;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let barrier = &barrier;
                let store = &store;
                let mut asset = original.clone();
                asset.intended[0].field = Some(text_field("name", &format!("winner-{index}")));
                scope.spawn(move || {
                    let record = admit(record::Body::Asset(asset));
                    barrier.wait();
                    store.save_asset(&record, Some(1), "admin", 2)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("test worker"))
            .collect::<Vec<_>>()
    });
    eprintln!("concurrent outcomes: {results:?}");
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().any(|r| matches!(r, Err(Error::Conflict))));
    assert_eq!(store.asset(target(&original))?.revision, 2);
    Ok(())
}

// Admission writes exact recovery arguments, audit and work together; retries use one ID.
#[test]
fn operation_atomicity_idempotency_and_argument_binding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let arguments = [text_field("mode", "start")];
    let mut plan = plan(target, &arguments);
    let id = store.accept(&plan, 10)?;
    assert_eq!(store.accept(&plan, 11)?, id);
    assert_eq!(store.pending()?, vec![id]);
    assert_eq!(store.operation(id)?.arguments, arguments);
    plan.action = "delete";
    assert!(matches!(store.accept(&plan, 12), Err(Error::Conflict)));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(store.pending()?, vec![id]);
    assert_eq!(store.operation(id)?.arguments, arguments);
    Ok(())
}

// An expired sender cannot settle work; takeover of unknown work cannot resend it.
#[test]
fn fenced_unknown_effect_requires_reconciliation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let id = store.accept(&plan(target, &[]), 10)?;
    let first = store.claim(id, "first", 20, 30)?;
    assert_eq!(store.prepare_dispatch(&first, 1, 21)?, 2);
    let second = store.claim(id, "second", 31, 60)?;
    assert!(second.epoch > first.epoch);
    let operation = store.operation(id)?;
    assert_eq!(operation.state, OperationState::Reconciling);
    assert_eq!(operation.outcome, EffectOutcome::Unknown);
    assert!(matches!(
        store.prepare_dispatch(&second, operation.revision, 32),
        Err(Error::UnsafeTransition)
    ));
    assert!(matches!(
        store.reconcile(
            &first,
            operation.revision,
            OperationState::Succeeded,
            EffectOutcome::Confirmed,
            Some("task-accepted"),
            32
        ),
        Err(Error::Conflict)
    ));
    store.reconcile(
        &second,
        operation.revision,
        OperationState::Succeeded,
        EffectOutcome::Confirmed,
        Some("task-accepted"),
        33,
    )?;
    assert!(store.pending()?.is_empty());
    assert!(store.operation(id)?.arguments.is_empty());
    Ok(())
}

// Quota refusal rolls back every side record, including counters and outbox.
#[test]
fn atomic_quota_refusal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut p = policy();
    p.classes[Class::Work as usize].rows = 1;
    let store = Store::open(dir.path(), p)?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let first = store.accept(&plan(target, &[]), 10)?;
    let before = store.usage()?;
    let mut second = plan(target, &[]);
    second.retry_key = "another";
    assert!(matches!(store.accept(&second, 11), Err(Error::Limit)));
    assert_eq!(store.usage()?, before);
    assert_eq!(store.pending()?, vec![first]);
    Ok(())
}

// Restoring an older logical image cannot permit completed effects to run again.
#[test]
fn old_backup_is_read_only_for_effects_across_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let dest = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let id = store.accept(&plan(target, &[]), 10)?;
    let mut bytes = Vec::new();
    store.export(&mut bytes)?;
    let lease = store.claim(id, "worker", 20, 30)?;
    store.prepare_dispatch(&lease, 1, 21)?;
    store.reconcile(
        &lease,
        2,
        OperationState::Succeeded,
        EffectOutcome::Confirmed,
        None,
        22,
    )?;
    let restored = Store::restore(dest.path(), &bytes, policy())?;
    assert!(restored.recovery_required()?);
    assert!(matches!(
        restored.claim(id, "worker", 23, 30),
        Err(Error::RecoveryRequired)
    ));
    drop(restored);
    let reopened = Store::open(dest.path(), policy())?;
    assert!(reopened.recovery_required()?);
    assert!(matches!(
        reopened.accept(&plan(target, &[]), 24),
        Err(Error::RecoveryRequired)
    ));
    Ok(())
}

// Cleanup preserves pinned detail and unresolved effects, and resumes from a cursor.
#[test]
fn bounded_cleanup_preserves_recovery_and_pin() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let audit = format!("audit/asset/{}/1", target.asset);
    store.pin_detail(&audit, true, store.usage()?.revision)?;
    let id = store.accept(&plan(target, &[]), 10)?;
    let cleanup = store.cleanup(10_000, None)?;
    assert!(cleanup.pinned > 0);
    assert_eq!(store.pending()?, vec![id]);
    assert!(store.operation(id).is_ok());
    store.pin_detail(&audit, false, store.usage()?.revision)?;
    let cleanup = store.cleanup(10_000, None)?;
    assert!(cleanup.removed > 0);
    assert!(store.operation(id).is_ok());
    Ok(())
}

// Another operation must not steal a target while a prior effect is unknown.
#[test]
fn another_operation_cannot_bypass_uncertain_target() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let first = store.accept(&plan(target, &[]), 10)?;
    let lease = store.claim(first, "first", 20, 30)?;
    store.prepare_dispatch(&lease, 1, 21)?;
    let mut second_plan = plan(target, &[]);
    second_plan.retry_key = "new-request";
    let second = store.accept(&second_plan, 31)?;
    assert!(matches!(
        store.claim(second, "second", 32, 50),
        Err(Error::Conflict)
    ));
    Ok(())
}

// Missing optional evidence and explicit clear must remain no-op on exact replay.
#[test]
fn explicit_clear_replay_has_no_durable_mutation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut asset = asset();
    asset.observed = vec![observation(
        &SourceId::generate().to_string(),
        100,
        vec![omloom_contracts::wire::Field {
            name: "missing".into(),
            value: Some(omloom_contracts::wire::field::Value::Clear(0)),
        }],
    )];
    let record = admit(record::Body::Asset(asset));
    store.save_asset(&record, None, "admin", 100)?;
    let before = store.usage()?;
    store.save_asset(&record, Some(1), "admin", 101)?;
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A crash marker left by interrupted restore must prohibit dispatch after open.
#[test]
fn interrupted_restore_cannot_open_with_effects_enabled() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let id = store.accept(&plan(target, &[]), 10)?;
    drop(store);
    std::fs::write(dir.path().join("RESTORE_REQUIRED"), b"restore in progress")?;
    let store = Store::open(dir.path(), policy())?;
    assert!(store.recovery_required()?);
    assert!(matches!(
        store.claim(id, "worker", 20, 40),
        Err(Error::RecoveryRequired)
    ));
    Ok(())
}

// A response prepared against old evidence cannot replace a newer source window.
#[test]
fn stale_evidence_snapshot_conflicts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut asset = asset();
    let source = SourceId::generate().to_string();
    asset.observed = vec![observation(&source, 100, vec![text_field("name", "old")])];
    let stale = admit(record::Body::Asset(asset.clone()));
    store.save_asset(
        &admit(record::Body::Asset(asset.clone())),
        None,
        "admin",
        100,
    )?;
    let snapshot = store.usage()?.revision;
    asset.observed = vec![observation(&source, 200, vec![text_field("name", "new")])];
    // The accepted-intent revision is unchanged by observations; it is not a source CAS.
    let record = admit(record::Body::Asset(asset));
    store.refresh_asset(&record, 1, snapshot, "admin", 200)?;
    assert_ne!(store.usage()?.revision, snapshot);
    assert!(matches!(
        store.save_asset(&stale, Some(1), "admin", 201),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.refresh_asset(&stale, 1, snapshot, "admin", 201),
        Err(Error::Conflict)
    ));
    Ok(())
}

// The child bypasses Database Drop after acknowledged application transactions.
#[test]
fn abrupt_store_child() -> TestResult {
    let Some(root) = std::env::var_os("OMLOOM_STORE_CRASH_ROOT") else {
        return Ok(());
    };
    let root = std::path::PathBuf::from(root);
    let bytes = std::fs::read(root.join("asset.pb"))?;
    let record = omloom_contracts::decode(
        &bytes,
        omloom_contracts::Limits::default(),
        &omloom_contracts::MemoryBudget::new(32 * 1_048_576),
    )?;
    let Some(record::Body::Asset(asset)) = &record.wire().body else {
        panic!("fixture asset")
    };
    let store = Store::open(&root.join("db"), policy())?;
    let id = store.accept(&plan(target(asset), &[text_field("mode", "start")]), 10)?;
    let lease = store.claim(id, "crashed", 20, 30)?;
    store.prepare_dispatch(&lease, 1, 21)?;
    std::process::exit(0);
}

// A lost process after dispatch intent cannot lose work or enable a duplicate send.
#[test]
fn abrupt_exit_recovers_application_intent_and_uncertainty() -> TestResult {
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let asset = asset();
    let target = target(&asset);
    let frame = omloom_contracts::wire::Record {
        body: Some(record::Body::Asset(asset.clone())),
    }
    .encode_to_vec();
    std::fs::write(dir.path().join("asset.pb"), frame)?;
    let store = Store::open(&dir.path().join("db"), policy())?;
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    drop(store);
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "abrupt_store_child", "--nocapture"])
        .env("OMLOOM_STORE_CRASH_ROOT", dir.path())
        .status()?;
    assert!(status.success());
    let store = Store::open(&dir.path().join("db"), policy())?;
    let id = store.accept(&plan(target, &[text_field("mode", "start")]), 31)?;
    assert_eq!(store.pending()?, vec![id]);
    assert_eq!(store.operation(id)?.outcome, EffectOutcome::Unknown);
    let lease = store.claim(id, "replacement", 31, 50)?;
    let operation = store.operation(id)?;
    assert_eq!(operation.state, OperationState::Reconciling);
    assert!(matches!(
        store.prepare_dispatch(&lease, operation.revision, 32),
        Err(Error::UnsafeTransition)
    ));
    Ok(())
}

// An expired retry must be rejected even after recent detail has been cleaned.
#[test]
fn terminal_detail_expiry_does_not_reopen_retry() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let plan = plan(target, &[]);
    let id = store.accept(&plan, 10)?;
    let lease = store.claim(id, "worker", 20, 50)?;
    store.reconcile(
        &lease,
        1,
        OperationState::Cancelled,
        EffectOutcome::NotSent,
        None,
        21,
    )?;
    let before = store.usage()?.rows[Class::Checkpoint as usize];
    let mut cursor = None;
    loop {
        let report = store.cleanup(200_000, cursor.as_deref())?;
        cursor = report.cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(matches!(store.operation(id), Err(Error::Missing)));
    assert_eq!(store.usage()?.rows[Class::Checkpoint as usize], before);
    assert!(matches!(store.accept(&plan, 200_000), Err(Error::Expired)));
    assert!(store.pending()?.is_empty());
    Ok(())
}

// Pages must be stable or explicitly expired, and migrations must preserve populated data.
#[test]
fn pagination_and_populated_schema_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    for _ in 0..3 {
        store.save_asset(&admit(record::Body::Asset(asset())), None, "admin", 1)?;
    }
    let page = store.assets(None, None, 2)?;
    assert_eq!(page.assets.len(), 2);
    assert!(page.next.is_some());
    let last = store.assets(page.next.as_deref(), Some(page.revision), 2)?;
    assert_eq!(last.assets.len(), 1);
    assert!(last.next.is_none());
    store.save_asset(&admit(record::Body::Asset(asset())), None, "admin", 2)?;
    assert!(matches!(
        store.assets(page.next.as_deref(), Some(page.revision), 2),
        Err(Error::Expired)
    ));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(store.assets(None, None, 10)?.assets.len(), 4);
    assert!(matches!(store.assets(None, None, 129), Err(Error::Invalid)));
    Ok(())
}

// A pin that wins its CAS race must survive cleanup; deletion cannot be resurrected.
#[test]
fn pin_cleanup_race_is_serializable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let key = format!("audit/asset/{}/1", target.asset);
    let revision = store.usage()?.revision;
    let barrier = std::sync::Barrier::new(2);
    let (pin, cleanup) = std::thread::scope(|scope| {
        let p = scope.spawn(|| {
            barrier.wait();
            store.pin_detail(&key, true, revision)
        });
        let c = scope.spawn(|| {
            barrier.wait();
            store.cleanup(2000, None)
        });
        (
            p.join().expect("pin worker"),
            c.join().expect("cleanup worker"),
        )
    });
    if pin.is_ok() {
        let report = store.cleanup(2000, None)?;
        assert!(report.pinned > 0);
    } else {
        assert!(matches!(pin, Err(Error::Conflict) | Err(Error::Missing)));
    }
    if let Err(error) = cleanup {
        assert!(matches!(error, Error::Conflict));
    }
    Ok(())
}

// Retained audit must not be part of the hot current-field projection.
#[test]
fn current_fields_do_not_scan_activity_history() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset.clone())), None, "admin", 1)?;
    for revision in 1..6 {
        asset.intended[0].field = Some(text_field("name", &format!("edit-{revision}")));
        store.save_asset(
            &admit(record::Body::Asset(asset.clone())),
            Some(revision),
            "admin",
            revision + 1,
        )?;
    }
    let rows = store
        .diagnostic_database()
        .execute_cypher_shared(
            "MATCH (n:OmRow) WHERE n.parent=$parent RETURN n.key AS key",
            Some(std::collections::HashMap::from([(
                "parent".into(),
                coordinode_core::graph::types::Value::String(format!("asset/{}", target.asset)),
            )])),
            None,
            None,
            None,
        )?
        .rows;
    assert_eq!(
        rows.len(),
        1,
        "the current-field index must contain only the one live intended field"
    );
    assert_eq!(store.asset(target)?.revision, 6);
    Ok(())
}

// Individually valid partial updates cannot accumulate more current sources than the contract allows.
#[test]
fn cumulative_partial_source_limit_is_enforced() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset.clone())), None, "admin", 1)?;
    for index in 0..32 {
        asset.observed = vec![observation(
            &SourceId::generate().to_string(),
            index + 10,
            vec![text_field("name", "seen")],
        )];
        store.refresh_asset(
            &admit(record::Body::Asset(asset.clone())),
            1,
            store.usage()?.revision,
            "admin",
            index + 10,
        )?;
    }
    asset.observed = vec![observation(
        &SourceId::generate().to_string(),
        100,
        vec![text_field("name", "extra")],
    )];
    let before = store.usage()?;
    assert!(matches!(
        store.refresh_asset(
            &admit(record::Body::Asset(asset)),
            1,
            before.revision,
            "admin",
            100
        ),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(store.asset(target)?.observed.len(), 32);
    Ok(())
}

// A relation must bind compatible inventory kinds and exact existing incarnations.
#[test]
fn typed_relations_validate_endpoints_and_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut vm = asset();
    vm.kind = omloom_contracts::wire::AssetKind::VirtualMachine as i32;
    let host = asset();
    let vm_target = target(&vm);
    let host_target = target(&host);
    store.save_asset(&admit(record::Body::Asset(vm.clone())), None, "admin", 1)?;
    store.save_asset(&admit(record::Body::Asset(host.clone())), None, "admin", 1)?;
    let id = RelationId::generate();
    let mut relation = omloom_contracts::wire::Relation {
        relation_id: id.to_string(),
        from: vm.target.clone(),
        to: host.target.clone(),
        kind: omloom_contracts::wire::RelationKind::RunsOn as i32,
        revision: 1,
        authority_id: AuthorityId::generate().to_string(),
    };
    store.save_relation(
        &admit(record::Body::Relation(relation.clone())),
        None,
        "admin",
        2,
    )?;
    let graph = store
        .diagnostic_database()
        .execute_cypher_shared(
            "MATCH (a:OmRow {key:$key})-[:OmRelationFrom]->(r:OmRow)-[:OmRelationTo]->(b:OmRow) RETURN r.key AS key",
            Some(std::collections::HashMap::from([(
                "key".into(),
                coordinode_core::graph::types::Value::String(format!("asset/{}", vm_target.asset)),
            )])),
            None,
            None,
            None,
        )?
        .rows;
    assert_eq!(
        graph.len(),
        1,
        "relations must use the database's native adjacency"
    );
    assert_eq!(store.relations(vm_target, 10)?.len(), 1);
    let mut incompatible_host = host.clone();
    incompatible_host.kind = omloom_contracts::wire::AssetKind::Site as i32;
    assert!(matches!(
        store.save_asset(
            &admit(record::Body::Asset(incompatible_host)),
            Some(1),
            "admin",
            3
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.relation(id)?.from, vm.target);
    relation.relation_id = RelationId::generate().to_string();
    relation.from = host.target.clone();
    relation.to = vm.target.clone();
    assert!(matches!(
        store.save_relation(&admit(record::Body::Relation(relation)), None, "admin", 3),
        Err(Error::Invalid)
    ));
    assert_eq!(store.asset(host_target)?.revision, 1);
    assert_eq!(store.asset(vm_target)?.revision, 1);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        store.relation(id)?.kind,
        omloom_contracts::wire::RelationKind::RunsOn as i32
    );
    Ok(())
}

// Moving one relation must replace only its adjacency, preserving parallel identities.
#[test]
fn relation_moves_preserve_parallel_edges() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut vm = asset();
    vm.kind = omloom_contracts::wire::AssetKind::VirtualMachine as i32;
    let first = asset();
    let second = asset();
    for object in [&vm, &first, &second] {
        store.save_asset(
            &admit(record::Body::Asset(object.clone())),
            None,
            "admin",
            1,
        )?;
    }
    let mut relation = omloom_contracts::wire::Relation {
        relation_id: RelationId::generate().to_string(),
        from: vm.target.clone(),
        to: first.target.clone(),
        kind: omloom_contracts::wire::RelationKind::RunsOn as i32,
        revision: 1,
        authority_id: AuthorityId::generate().to_string(),
    };
    store.save_relation(
        &admit(record::Body::Relation(relation.clone())),
        None,
        "admin",
        2,
    )?;
    let mut parallel = relation.clone();
    parallel.relation_id = RelationId::generate().to_string();
    store.save_relation(
        &admit(record::Body::Relation(parallel.clone())),
        None,
        "admin",
        3,
    )?;
    relation.to = second.target.clone();
    relation.revision = 2;
    store.save_relation(
        &admit(record::Body::Relation(relation.clone())),
        Some(1),
        "admin",
        4,
    )?;
    assert_eq!(store.relations(target(&vm), 10)?.len(), 2);
    assert_eq!(store.relations(target(&first), 10)?, vec![parallel]);
    assert_eq!(store.relations(target(&second), 10)?, vec![relation]);
    assert!(matches!(store.relations(target(&vm), 1), Err(Error::Limit)));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(store.relations(target(&vm), 10)?.len(), 2);
    assert_eq!(store.relations(target(&first), 10)?.len(), 1);
    assert_eq!(store.relations(target(&second), 10)?.len(), 1);
    Ok(())
}

// Archival discards bulky current claims, while unresolved accepted work prevents removal.
#[test]
fn archive_reclaims_claims_without_releasing_unknown_effects() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut asset = asset();
    let target = target(&asset);
    asset.observed = vec![observation(
        &SourceId::generate().to_string(),
        10,
        vec![text_field("name", "observed")],
    )];
    store.save_asset(&admit(record::Body::Asset(asset.clone())), None, "admin", 1)?;
    let id = store.accept(&plan(target, &[]), 10)?;
    asset.adoption = omloom_contracts::wire::Adoption::Archived as i32;
    let archived = admit(record::Body::Asset(asset));
    assert!(matches!(
        store.save_asset(&archived, Some(1), "admin", 20),
        Err(Error::Conflict)
    ));
    let lease = store.claim(id, "worker", 20, 50)?;
    store.reconcile(
        &lease,
        1,
        OperationState::Cancelled,
        EffectOutcome::NotSent,
        None,
        21,
    )?;
    assert_eq!(store.save_asset(&archived, Some(1), "admin", 22)?, 2);
    let current = store.asset(target)?;
    assert!(current.observed.is_empty());
    assert!(current.intended.is_empty());
    assert_eq!(store.usage()?.rows[Class::Evidence as usize], 0);
    assert!(matches!(
        store.accept(&plan(target, &[]), 100_000),
        Err(Error::Expired)
    ));
    let activity = store.activity(&format!("asset/{}", target.asset), 10)?;
    assert_eq!(activity[0].action, "inventory.archived");
    Ok(())
}

// Checkpoints and review rows are atomic; partial/denied scans cannot delete inventory.
#[test]
fn scan_replay_epoch_and_preview_expiry_are_independent_of_inventory() -> TestResult {
    use omloom_contracts::wire::{
        AssetKind, Evidence, ScanBatch, ScanCandidate, ScanCompleteness, SourceObject,
    };
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let asset = asset();
    let target = target(&asset);
    store.save_asset(&admit(record::Body::Asset(asset)), None, "admin", 1)?;
    let connection = ConnectionId::generate();
    let state = store.start_scan(connection, None)?;
    let mut batch = ScanBatch {
        scan_id: state.scan.to_string(),
        connection_id: connection.to_string(),
        prior_cursor: None,
        cursor: "page-a".into(),
        completeness: ScanCompleteness::Partial as i32,
        snapshot_revision: None,
        candidates: vec![ScanCandidate {
            candidate_id: CandidateId::generate().to_string(),
            source: Some(SourceObject {
                object_kind: "qemu".into(),
                object_id: "vm".into(),
                incarnation_evidence: Some("lifetime-1".into()),
            }),
            kind: AssetKind::VirtualMachine as i32,
            evidence: Some(Evidence {
                source_id: SourceId::generate().to_string(),
                source_revision: Some("source-a".into()),
                observed_at_ms: 100,
                valid_until_ms: 1000,
                fields: vec![
                    text_field("name", "review-only"),
                    text_field("label", "same-page"),
                ],
            }),
        }],
    };
    let page = admit(record::Body::Scan(batch.clone()));
    store.ingest_scan(&page, 100)?;
    let usage = store.usage()?;
    store.ingest_scan(&page, 101)?;
    let mut reordered = batch.clone();
    reordered.candidates[0]
        .evidence
        .as_mut()
        .expect("evidence")
        .fields
        .reverse();
    store.ingest_scan(&admit(record::Body::Scan(reordered)), 101)?;
    assert_eq!(store.usage()?, usage);
    assert_eq!(
        store.scan_state(connection)?.cursor.as_deref(),
        Some("page-a")
    );
    assert_eq!(
        store.candidate(connection, "qemu", "vm", 101)?.candidate_id,
        batch.candidates[0].candidate_id
    );
    batch.prior_cursor = Some("page-a".into());
    batch.cursor = "page-b".into();
    batch.completeness = ScanCompleteness::AccessDenied as i32;
    batch.candidates.clear();
    store.ingest_scan(&admit(record::Body::Scan(batch)), 102)?;
    assert_eq!(
        store.scan_state(connection)?.completeness,
        ScanCompleteness::AccessDenied
    );
    assert!(store.asset(target).is_ok());
    let next = store.start_scan(connection, Some(state.epoch))?;
    assert!(next.epoch > state.epoch);
    assert!(matches!(
        store.ingest_scan(&page, 103),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.candidate(connection, "qemu", "vm", 103),
        Err(Error::Expired)
    ));
    let mut cursor = None;
    loop {
        let cleanup = store.cleanup_reviews(1000, cursor.as_deref())?;
        cursor = cleanup.cursor;
        if cursor.is_none() {
            break;
        }
    }
    let current = store.scan_state(connection)?;
    assert_eq!(current.review_rows, 0);
    assert_eq!(current.review_bytes, 0);
    assert!(store.asset(target).is_ok());
    Ok(())
}

// A source quota refusal must not advance its cursor or retain half of a review page.
#[test]
fn scan_source_quota_refuses_whole_page() -> TestResult {
    use omloom_contracts::wire::{
        AssetKind, Evidence, ScanBatch, ScanCandidate, ScanCompleteness, SourceObject,
    };
    let dir = tempfile::tempdir()?;
    let mut policy = policy();
    policy.review_per_source.rows = 1;
    let store = Store::open(dir.path(), policy)?;
    let connection = ConnectionId::generate();
    let state = store.start_scan(connection, None)?;
    let before = store.usage()?;
    let batch = ScanBatch {
        scan_id: state.scan.to_string(),
        connection_id: connection.to_string(),
        prior_cursor: None,
        cursor: "page".into(),
        completeness: ScanCompleteness::Partial as i32,
        snapshot_revision: None,
        candidates: vec![ScanCandidate {
            candidate_id: CandidateId::generate().to_string(),
            source: Some(SourceObject {
                object_kind: "qemu".into(),
                object_id: "vm".into(),
                incarnation_evidence: None,
            }),
            kind: AssetKind::VirtualMachine as i32,
            evidence: Some(Evidence {
                source_id: SourceId::generate().to_string(),
                source_revision: None,
                observed_at_ms: 100,
                valid_until_ms: 1000,
                fields: vec![text_field("name", "review")],
            }),
        }],
    };
    assert!(matches!(
        store.ingest_scan(&admit(record::Body::Scan(batch)), 100),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(store.scan_state(connection)?.cursor, None);
    assert!(matches!(
        store.candidate(connection, "qemu", "vm", 101),
        Err(Error::Missing)
    ));
    Ok(())
}

// An injected database retains the host's MVCC policy for other owned namespaces.
#[test]
fn injected_database_preserves_host_retention() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = coordinode_embed::Database::open(dir.path())?;
    db.set_retention_window(std::time::Duration::from_secs(60));
    let store = Store::from_database(db, policy())?;
    assert_eq!(
        store.diagnostic_database().retention_window(),
        std::time::Duration::from_secs(60)
    );
    Ok(())
}

// Bounded cleanup must progress across empty/ineligible pages after the MVCC window has elapsed.
#[test]
fn cleanup_after_idle_resumes_without_a_historical_snapshot() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut policy = policy();
    policy.batch_rows = 2;
    let store = Store::open(dir.path(), policy)?;
    for _ in 0..5 {
        store.save_asset(&admit(record::Body::Asset(asset())), None, "admin", 1)?;
    }
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let mut cursor = None;
    let mut removed = 0;
    let mut pages = 0;
    loop {
        let report = store.cleanup(2000, cursor.as_deref())?;
        removed += report.removed;
        cursor = report.cursor;
        pages += 1;
        assert!(pages < 100);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(removed, 5);
    assert!(pages > 1);
    assert_eq!(store.assets(None, None, 10)?.assets.len(), 5);
    Ok(())
}

// A new review window changes metadata, not the unchanged candidate value row.
#[test]
fn candidate_refresh_preserves_unchanged_value_storage() -> TestResult {
    use omloom_contracts::wire::{
        AssetKind, Evidence, ScanBatch, ScanCandidate, ScanCompleteness, SourceObject,
    };
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let connection = ConnectionId::generate();
    let state = store.start_scan(connection, None)?;
    let mut batch = ScanBatch {
        scan_id: state.scan.to_string(),
        connection_id: connection.to_string(),
        prior_cursor: None,
        cursor: "page-1".into(),
        completeness: ScanCompleteness::Partial as i32,
        snapshot_revision: None,
        candidates: vec![ScanCandidate {
            candidate_id: CandidateId::generate().to_string(),
            source: Some(SourceObject {
                object_kind: "qemu".into(),
                object_id: "vm".into(),
                incarnation_evidence: None,
            }),
            kind: AssetKind::VirtualMachine as i32,
            evidence: Some(Evidence {
                source_id: SourceId::generate().to_string(),
                source_revision: None,
                observed_at_ms: 100,
                valid_until_ms: 1000,
                fields: vec![text_field("name", "same")],
            }),
        }],
    };
    store.ingest_scan(&admit(record::Body::Scan(batch.clone())), 100)?;
    let rows = store
        .diagnostic_database()
        .execute_cypher_shared(
            "MATCH (n:OmRow) WHERE n.field='name' RETURN n.key AS key",
            None,
            None,
            None,
            None,
        )?
        .rows;
    let key = rows[0]
        .get("key")
        .and_then(coordinode_core::graph::types::Value::as_str)
        .expect("review field");
    let version = row_version(&store, key)?;
    batch.prior_cursor = Some("page-1".into());
    batch.cursor = "page-2".into();
    batch.candidates[0]
        .evidence
        .as_mut()
        .expect("evidence")
        .observed_at_ms = 200;
    store.ingest_scan(&admit(record::Body::Scan(batch)), 200)?;
    assert_eq!(row_version(&store, key)?, version);
    assert_eq!(
        store
            .candidate(connection, "qemu", "vm", 201)?
            .observation
            .expect("current observation")
            .evidence
            .expect("current evidence")
            .observed_at_ms,
        200
    );
    let mut cursor = None;
    loop {
        let report = store.cleanup_reviews(650, cursor.as_deref())?;
        cursor = report.cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        store
            .candidate(connection, "qemu", "vm", 650)?
            .observation
            .expect("observation")
            .evidence
            .expect("evidence")
            .fields[0],
        text_field("name", "same")
    );
    Ok(())
}

// Reaccepting the same relation is not a meaningful change or a new audit event.
#[test]
fn identical_relation_has_no_history_or_writes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let from = asset();
    let mut to = asset();
    to.kind = omloom_contracts::wire::AssetKind::Port as i32;
    store.save_asset(&admit(record::Body::Asset(from.clone())), None, "admin", 1)?;
    store.save_asset(&admit(record::Body::Asset(to.clone())), None, "admin", 1)?;
    let relation = omloom_contracts::wire::Relation {
        relation_id: RelationId::generate().to_string(),
        from: from.target,
        to: to.target,
        kind: omloom_contracts::wire::RelationKind::HasPort as i32,
        revision: 1,
        authority_id: AuthorityId::generate().to_string(),
    };
    let record = admit(record::Body::Relation(relation));
    store.save_relation(&record, None, "admin", 2)?;
    let before = store.usage()?;
    assert_eq!(store.save_relation(&record, Some(1), "admin", 3)?, 1);
    assert_eq!(store.usage()?, before);
    Ok(())
}
