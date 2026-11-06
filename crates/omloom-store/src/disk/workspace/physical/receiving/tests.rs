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
