use omloom_contracts::{
    Limits, MemoryBudget, Record,
    ids::*,
    wire::{self, field, record},
};
use omloom_store::*;
use prost::Message;
pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub fn policy() -> Policy {
    Policy {
        classes: [Budget {
            rows: 20_000,
            bytes: 32 * 1_048_576,
        }; CLASSES],
        total_bytes: 128 * 1_048_576,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: Budget {
            rows: 512,
            bytes: 2 * 1_048_576,
        },
        batch_rows: 128,
    }
}
pub fn text_field(name: &str, value: &str) -> wire::Field {
    wire::Field {
        name: name.into(),
        value: Some(field::Value::Text(value.into())),
    }
}
pub fn asset() -> wire::Asset {
    wire::Asset {
        target: Some(wire::AssetRef {
            asset_id: AssetId::generate().to_string(),
            incarnation: IncarnationId::generate().to_string(),
        }),
        kind: wire::AssetKind::Device as i32,
        adoption: wire::Adoption::Adopted as i32,
        revision: 1,
        intended: vec![wire::IntendedField {
            field: Some(text_field("name", "host")),
            authority_id: AuthorityId::generate().to_string(),
        }],
        ..Default::default()
    }
}
pub fn target(a: &wire::Asset) -> AssetRef {
    AssetRef::try_from(a.target.as_ref().expect("fixture target")).expect("fixture identity")
}
pub fn admit(body: record::Body) -> Record {
    let bytes = wire::Record { body: Some(body) }.encode_to_vec();
    omloom_contracts::decode(
        &bytes,
        Limits::default(),
        &MemoryBudget::new(32 * 1_048_576),
    )
    .expect("valid fixture")
}
pub fn observation(source: &str, at: u64, fields: Vec<wire::Field>) -> wire::Observation {
    wire::Observation {
        scan_id: Some(ScanId::generate().to_string()),
        evidence: Some(wire::Evidence {
            source_id: source.into(),
            source_revision: None,
            observed_at_ms: at,
            valid_until_ms: at + 1000,
            fields,
        }),
    }
}
pub fn plan<'a>(target: AssetRef, arguments: &'a [wire::Field]) -> Plan<'a> {
    Plan {
        target,
        inventory_revision: 1,
        caller: "operator",
        action: "start",
        arguments,
        retry_key: "request",
        expires_at_ms: 100_000,
    }
}
pub fn row_version(store: &Store, key: &str) -> Result<Option<u64>, Box<dyn std::error::Error>> {
    use coordinode_core::graph::{node::NodeId, types::Value};
    let rows = store
        .diagnostic_database()
        .execute_cypher_shared(
            "MATCH (n:OmRow {key:$key}) RETURN id(n) AS id",
            Some(std::collections::HashMap::from([(
                "key".into(),
                Value::String(key.into()),
            )])),
            None,
            None,
            None,
        )?
        .rows;
    let id = rows[0]
        .get("id")
        .and_then(Value::as_int)
        .expect("fixture row");
    Ok(store
        .diagnostic_database()
        .node_version(NodeId::from_raw(u64::try_from(id)?))?)
}
