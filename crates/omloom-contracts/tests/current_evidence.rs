mod support;
use omloom_contracts::{Error, Limits, MemoryBudget, decode, ids::SourceId, wire::*};
use prost::Message;

fn asset() -> Asset {
    let case = support::cases()
        .into_iter()
        .find(|case| case.name == "asset" && case.error.is_none())
        .expect("valid asset fixture");
    let record = Record::decode(case.bytes.as_slice()).unwrap();
    let Some(record::Body::Asset(asset)) = record.body else {
        panic!("asset fixture")
    };
    asset
}

fn admit(asset: Asset) -> Result<omloom_contracts::Record, Error> {
    let bytes = Record {
        body: Some(record::Body::Asset(asset)),
    }
    .encode_to_vec();
    decode(
        &bytes,
        Limits::default(),
        &MemoryBudget::new(16 * 1_048_576),
    )
}

#[test]
fn distinct_field_windows_have_bounded_current_cardinality() {
    // All 256 fields may have distinct provenance. This prevents replacing the
    // old 32-source limit with a 32-window limit that still loses partial data.
    let mut current = asset();
    current.imported.clear();
    let template = current.observed[0].clone();
    current.observed = (0..256)
        .map(|index| {
            let mut group = template.clone();
            let ev = group.evidence.as_mut().unwrap();
            ev.observed_at_ms = index + 1;
            ev.valid_until_ms = index + 2;
            ev.fields = vec![Field {
                name: format!("field.{index}"),
                value: Some(field::Value::Flag(false)),
            }];
            group
        })
        .collect();
    assert!(admit(current.clone()).is_ok());
    let mut too_many_groups = current.clone();
    let mut another_source = too_many_groups.observed[0].clone();
    another_source.evidence.as_mut().unwrap().source_id = SourceId::generate().to_string();
    too_many_groups.observed.push(another_source);
    assert_eq!(admit(too_many_groups).err(), Some(Error::Limit));
    // Keep 256 groups but exceed the per-source field count: splitting groups
    // must not bypass the bound on the source's current claims.
    current.observed[0]
        .evidence
        .as_mut()
        .unwrap()
        .fields
        .push(Field {
            name: "field.extra".into(),
            value: Some(field::Value::Flag(true)),
        });
    assert_eq!(admit(current).err(), Some(Error::Limit));
}

#[test]
fn source_and_group_limits_are_independent() {
    // Independent sources may disagree on one field. A 33rd source is still
    // rejected even when far fewer than 256 provenance groups are present.
    let mut current = asset();
    let template = current.observed[0].clone();
    current.observed = (0..32)
        .map(|_| {
            let mut group = template.clone();
            let ev = group.evidence.as_mut().unwrap();
            ev.source_id = SourceId::generate().to_string();
            ev.fields.truncate(1);
            group
        })
        .collect();
    assert!(admit(current.clone()).is_ok());
    let mut extra = template;
    extra.evidence.as_mut().unwrap().source_id = SourceId::generate().to_string();
    current.observed.push(extra);
    assert_eq!(admit(current).err(), Some(Error::Limit));
}

#[test]
fn overlapping_fields_are_rejected_in_both_current_layers() {
    // A newer sample cannot be appended beside the old value in either the
    // observed or proposed layer. Different layers retain different meanings.
    for imported in [false, true] {
        let mut current = asset();
        let groups = if imported {
            &mut current.imported
        } else {
            &mut current.observed
        };
        let mut newer = groups[0].clone();
        newer.evidence.as_mut().unwrap().source_revision = Some("newer".into());
        newer.evidence.as_mut().unwrap().fields.truncate(1);
        groups.push(newer);
        assert_eq!(admit(current).err(), Some(Error::Duplicate));
    }
}

#[test]
fn replacement_and_clear_preserve_omitted_evidence_and_intent() {
    // Exercise a consumer editing a decoded current record: split out B,
    // refresh it repeatedly, then clear it. Admission/roundtrip must preserve
    // A's original evidence and accepted intent without retaining B's samples.
    let mut current = asset();
    current.observed[0]
        .evidence
        .as_mut()
        .unwrap()
        .fields
        .truncate(2);
    let original = current.observed[0].clone();
    let intent = current.intended.clone();
    let mut refreshed = original.clone();
    refreshed.evidence.as_mut().unwrap().fields.remove(0);
    current.observed[0]
        .evidence
        .as_mut()
        .unwrap()
        .fields
        .truncate(1);
    current.observed.push(refreshed);
    for instant in [300, 400, 500] {
        let ev = current.observed[1].evidence.as_mut().unwrap();
        ev.observed_at_ms = instant;
        ev.valid_until_ms = instant + 50;
        ev.fields[0].value = Some(if instant == 500 {
            field::Value::Clear(NullValue::NullValue as i32)
        } else {
            field::Value::Text("unchanged value".into())
        });
        let admitted = admit(current).unwrap();
        let encoded = admitted.encode().unwrap();
        let next = Record::decode(encoded.as_ref()).unwrap();
        let Some(record::Body::Asset(next)) = next.body else {
            panic!("asset fixture")
        };
        assert_eq!(next.observed.len(), 2);
        let old = next.observed[0].evidence.as_ref().unwrap();
        assert_eq!(
            old.observed_at_ms,
            original.evidence.as_ref().unwrap().observed_at_ms
        );
        assert_eq!(
            old.valid_until_ms,
            original.evidence.as_ref().unwrap().valid_until_ms
        );
        assert_eq!(old.fields, original.evidence.as_ref().unwrap().fields[..1]);
        assert_eq!(next.intended, intent);
        assert_eq!(
            next.observed[1].evidence.as_ref().unwrap().observed_at_ms,
            instant
        );
        current = next;
    }
    assert_eq!(
        current.observed[1].evidence.as_ref().unwrap().fields[0].value,
        Some(field::Value::Clear(NullValue::NullValue as i32))
    );
}
