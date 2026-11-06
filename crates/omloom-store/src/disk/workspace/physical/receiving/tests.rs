use super::*;
use crate::{Budget, CLASSES, Policy};

fn policy() -> Policy {
    Policy {
        classes: [Budget {
            rows: 4096,
            bytes: 4_194_304,
        }; CLASSES],
        total_bytes: 33_554_432,
        detail_age_ms: 1000,
        review_age_ms: 1000,
        review_per_source: Budget {
            rows: 128,
            bytes: 1_048_576,
        },
        batch_rows: 128,
        physical_reservation_max_ms: 1000,
    }
}
fn actor() -> Context {
    Context {
        actor: "receiving-operator".into(),
        fixture: "operator".into(),
        grants: vec![Grant {
            scope: Scope::All,
            read: Fields::All,
            write: Fields::All,
            writable: true,
        }],
        teams: vec![],
        recipients: vec![],
        schema_admin: true,
        authority_revision: 1,
    }
}
fn save(store: &Store, actor: &Context, name: &str) -> Result<api::PhysicalPlan> {
    let reply = store.workspace_physical(
        actor,
        api::Physical {
            action: Some(api::physical::Action::Save(api::PhysicalSave {
                plan: Some(api::PhysicalPlan {
                    name: name.into(),
                    materials: vec![api::PhysicalMaterial {
                        name: "Compute node".into(),
                        kind: wire::AssetKind::Device as i32,
                        quantity: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            })),
        },
        Some(&api::WorkingContext::default()),
        100,
    )?;
    match reply.result {
        Some(api::reply::Result::Physical(view)) => view.plan.ok_or(Error::Missing),
        _ => Err(Error::Corrupt),
    }
}
fn request(plan: api::PhysicalPlan) -> Result<api::PhysicalDeviceReceipt> {
    Ok(api::PhysicalDeviceReceipt {
        plan: Some(api::PhysicalPlanKey {
            id: plan.id.ok_or(Error::Missing)?,
            revision: plan.revision,
        }),
        inventory: Some(api::WorkingContext::default()),
        changes: vec![wire::Field {
            name: "name".into(),
            value: Some(field::Value::Text("Received node".into())),
        }],
        ..Default::default()
    })
}

// An unrelated commit closes this receipt's snapshot without ever creating its
// outcome. The client must receive a definitive refusal, with no orphan device.
#[test]
fn unrelated_commit_loss_is_definitively_rejected()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let actor = actor();
    let plan = save(&store, &actor, "Receiving")?;
    let working = api::WorkingContext {
        expected: Some(store.usage()?.revision),
        ..Default::default()
    };
    let tx = store.begin()?;
    save(&store, &actor, "Unrelated plan")?;
    let before = store.usage()?;
    let request = request(plan)?;
    let result = store.commit_receipt(tx, &actor, &request, &working, 100);
    assert!(matches!(result, Err(Error::Rejected(cause)) if matches!(*cause, Error::Conflict)));
    assert_eq!(store.usage()?, before);
    let rows = store
        .db
        .execute_cypher("MATCH (n:OmRow {parent:'assets'}) RETURN n.key AS key")?;
    assert!(rows.is_empty());
    Ok(())
}

// A winner can be accepted and cleaned while the old snapshot is still open.
// Missing detail after several commits cannot be reported as non-acceptance.
#[test]
fn expired_concurrent_winner_remains_unresolved()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = actor();
    let request = request(save(&store, &actor, "Receiving")?)?;
    let working = api::WorkingContext {
        expected: Some(store.usage()?.revision),
        ..Default::default()
    };
    let losing = store.begin()?;
    store.commit_receipt(store.begin()?, &actor, &request, &working, 100)?;
    store.cleanup(2000, None)?;
    assert!(
        store
            .read(&format!(
                "mutation-outcome/{}",
                working.expected.ok_or(Error::Missing)?
            ))?
            .is_none()
    );
    let before = store.usage()?;
    assert!(matches!(
        store.commit_receipt(losing, &actor, &request, &working, 100),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn recover(
    store: &Store,
    actor: &Context,
    lookup: api::PhysicalReceiptLookup,
) -> Result<api::PhysicalView> {
    match store
        .workspace_physical(
            actor,
            api::Physical {
                action: Some(api::physical::Action::RecoverReceipt(lookup)),
            },
            Some(&api::WorkingContext::default()),
            2000,
        )?
        .result
    {
        Some(api::reply::Result::Physical(view)) => Ok(view),
        _ => Err(Error::Corrupt),
    }
}

// Recovery fences a request that has not committed, including one whose old
// transaction is still open. Repeated recovery neither creates data nor writes.
#[test]
fn recovery_fences_inflight_creation_without_an_extra_ledger()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = actor();
    let plan = save(&store, &actor, "Receiving")?;
    let lookup = api::PhysicalReceiptLookup {
        revision: store.usage()?.revision,
        plan: plan.id.clone().ok_or(Error::Missing)?,
    };
    let working = api::WorkingContext {
        expected: Some(lookup.revision),
        ..Default::default()
    };
    let old = store.begin()?;
    let before = store.usage()?;
    let view = recover(&store, &actor, lookup.clone())?;
    assert_eq!(
        view.receipt_recovery,
        api::PhysicalReceiptRecovery::NotCommitted as i32
    );
    let fenced = store.usage()?;
    assert_eq!(fenced.revision, before.revision + 1);
    assert_eq!(fenced.rows, before.rows);
    assert_eq!(fenced.bytes, before.bytes);
    assert!(
        matches!(store.commit_receipt(old,&actor,&request(plan)?,&working,100),Err(Error::Rejected(cause)) if matches!(*cause,Error::Conflict))
    );
    assert_eq!(
        recover(&store, &actor, lookup)?.receipt_recovery,
        api::PhysicalReceiptRecovery::NotCommitted as i32
    );
    assert_eq!(store.usage()?, fenced);
    Ok(())
}

// Expiry removes detail, not the real inventory. Recovery returns current
// authorized facts without claiming that a missing outcome means non-acceptance.
#[test]
fn automatic_recovery_preserves_expired_and_restored_meanings()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = actor();
    let plan = save(&store, &actor, "Receiving")?;
    let lookup = api::PhysicalReceiptLookup {
        revision: store.usage()?.revision,
        plan: plan.id.clone().ok_or(Error::Missing)?,
    };
    store.commit_receipt(
        store.begin()?,
        &actor,
        &request(plan)?,
        &api::WorkingContext {
            expected: Some(lookup.revision),
            ..Default::default()
        },
        100,
    )?;
    let before = store.usage()?;
    let accepted = recover(&store, &actor, lookup.clone())?
        .receipt
        .ok_or(Error::Missing)?;
    assert_eq!(
        accepted.plan.as_ref().ok_or(Error::Missing)?.id,
        lookup.plan
    );
    assert_eq!(store.usage()?, before);
    let mut read_only = actor.clone();
    read_only.grants[0].writable = false;
    assert_eq!(
        recover(&store, &read_only, lookup.clone())?.receipt,
        Some(accepted)
    );
    assert_eq!(store.usage()?, before);
    let mut foreign = actor.clone();
    foreign.actor = "another-operator".into();
    assert!(matches!(
        recover(&store, &foreign, lookup.clone()),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    store.cleanup(2000, None)?;
    let before = store.usage()?;
    let view = recover(&store, &actor, lookup.clone())?;
    assert_eq!(
        view.receipt_recovery,
        api::PhysicalReceiptRecovery::Unavailable as i32
    );
    assert!(view.receipt.is_none());
    assert_eq!(
        view.plan.ok_or(Error::Missing)?.materials[0]
            .allocations
            .len(),
        1
    );
    assert_eq!(store.usage()?, before);
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &image, policy(), image.len() as u64)?;
    let before = restored.usage()?;
    assert!(matches!(
        recover(&restored, &actor, lookup),
        Err(Error::RecoveryRequired)
    ));
    assert_eq!(restored.usage()?, before);
    Ok(())
}

// A concurrent actor's successful receipt cannot be this plan owner's result.
// Classify the refusal without disclosing that actor's plan or created identity.
#[test]
fn recovery_does_not_confuse_another_actors_outcome()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let first = actor();
    let mut second = first.clone();
    second.actor = "other-operator".into();
    let first_plan = save(&store, &first, "First plan")?;
    let second_plan = save(&store, &second, "Second plan")?;
    let base = store.usage()?.revision;
    store.commit_receipt(
        store.begin()?,
        &first,
        &request(first_plan)?,
        &api::WorkingContext {
            expected: Some(base),
            ..Default::default()
        },
        100,
    )?;
    let before = store.usage()?;
    let view = recover(
        &store,
        &second,
        api::PhysicalReceiptLookup {
            revision: base,
            plan: second_plan.id.ok_or(Error::Missing)?,
        },
    )?;
    assert_eq!(
        view.receipt_recovery,
        api::PhysicalReceiptRecovery::NotCommitted as i32
    );
    assert!(view.receipt.is_none());
    assert!(
        view.plan.ok_or(Error::Missing)?.materials[0]
            .allocations
            .is_empty()
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}
