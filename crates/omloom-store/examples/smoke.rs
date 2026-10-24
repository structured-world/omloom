//! Local storage exercise, not an application login or provider simulation.
use omloom_contracts::{
    Limits, MemoryBudget,
    ids::*,
    wire::{self, field, record},
};
use omloom_store::{Budget, CLASSES, Plan, Policy, Store};
use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let policy = Policy {
        classes: [Budget {
            rows: 20_000,
            bytes: 32 * 1_048_576,
        }; CLASSES],
        total_bytes: 128 * 1_048_576,
        detail_age_ms: 1000,
        batch_rows: 128,
    };
    let target = AssetRef {
        asset: AssetId::generate(),
        incarnation: IncarnationId::generate(),
    };
    let asset = wire::Asset {
        target: Some(wire::AssetRef {
            asset_id: target.asset.to_string(),
            incarnation: target.incarnation.to_string(),
        }),
        kind: wire::AssetKind::Device as i32,
        adoption: wire::Adoption::Adopted as i32,
        revision: 1,
        intended: vec![wire::IntendedField {
            authority_id: AuthorityId::generate().to_string(),
            field: Some(wire::Field {
                name: "name".into(),
                value: Some(field::Value::Text("local-storage-exercise".into())),
            }),
        }],
        ..Default::default()
    };
    let bytes = wire::Record {
        body: Some(record::Body::Asset(asset)),
    }
    .encode_to_vec();
    let record = omloom_contracts::decode(
        &bytes,
        Limits::default(),
        &MemoryBudget::new(32 * 1_048_576),
    )?;
    let store = Store::open(directory.path(), policy.clone())?;
    store.save_asset(&record, None, "local-storage-exercise", 1)?;
    let before = store.usage()?;
    for _ in 0..100 {
        store.asset(target)?;
        store.save_asset(&record, Some(1), "local-storage-exercise", 2)?;
    }
    assert_eq!(store.usage()?, before);
    let plan = Plan {
        target,
        inventory_revision: 1,
        caller: "local-storage-exercise",
        action: "inventory.export",
        arguments: &[],
        retry_key: "one-export",
        expires_at_ms: 100,
    };
    let operation = store.accept(&plan, 10)?;
    drop(store);
    let reopened = Store::open(directory.path(), policy)?;
    assert_eq!(reopened.asset(target)?.revision, 1);
    assert_eq!(reopened.pending()?, vec![operation]);
    assert_eq!(reopened.accept(&plan, 11)?, operation);
    println!(
        "verified: inventory and accepted work survive reopen; identical input and retrieval add no application writes"
    );
    Ok(())
}
