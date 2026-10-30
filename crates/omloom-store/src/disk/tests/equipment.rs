use super::*;
use prost::Message;

#[test]
fn format_thirteen_keeps_definite_fields_without_hypothesis_rows()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    // The format marker advances once; definite values and their current rows survive.
    let dir = tempfile::tempdir()?;
    let policy = Policy {
        classes: [crate::Budget {
            rows: 2048,
            bytes: 4 * 1_048_576,
        }; CLASSES],
        total_bytes: 32 * 1_048_576,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: crate::Budget {
            rows: 128,
            bytes: 1_048_576,
        },
        batch_rows: 128,
    };
    let store = Store::open(dir.path(), policy.clone())?;
    let mut value = wire::Asset {
        target: Some(wire::AssetRef {
            asset_id: omloom_contracts::ids::AssetId::generate().to_string(),
            incarnation: omloom_contracts::ids::IncarnationId::generate().to_string(),
        }),
        kind: wire::AssetKind::Device as i32,
        adoption: wire::Adoption::Adopted as i32,
        revision: 1,
        intended: vec![wire::IntendedField {
            authority_id: omloom_contracts::ids::AuthorityId::generate().to_string(),
            field: Some(wire::Field {
                name: "name".into(),
                value: Some(field::Value::Text("Definite old device".into())),
            }),
        }],
        ..Default::default()
    };
    value.intended.push(wire::IntendedField {
        authority_id: omloom_contracts::ids::AuthorityId::generate().to_string(),
        field: Some(wire::Field {
            name: "model".into(),
            value: Some(field::Value::Text("M".repeat(300))),
        }),
    });
    let identity =
        omloom_contracts::ids::AssetRef::try_from(value.target.as_ref().ok_or(Error::Invalid)?)?;
    let bytes = wire::Record {
        body: Some(wire::record::Body::Asset(value)),
    }
    .encode_to_vec();
    let record = omloom_contracts::decode(
        &bytes,
        omloom_contracts::Limits::default(),
        &omloom_contracts::MemoryBudget::new(16 * 1_048_576),
    )?;
    store.save_asset(&record, None, "fixture", 100)?;
    let expected = store.asset(identity)?;
    let mut tx = store.begin()?;
    tx.root.insert("schema".into(), int(13)?);
    tx.changed = true;
    tx.commit()?;
    let rows = store.usage()?.rows[Class::Inventory as usize];
    drop(store);
    let store = Store::open(dir.path(), policy)?;
    assert_eq!(store.asset(identity)?, expected);
    assert_eq!(store.usage()?.rows[Class::Inventory as usize], rows);
    let before = store.usage()?;
    store.asset(identity)?;
    assert_eq!(store.usage()?, before);
    Ok(())
}
