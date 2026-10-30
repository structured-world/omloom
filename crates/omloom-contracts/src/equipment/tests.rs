use super::*;
fn possible() -> EquipmentClaim {
    EquipmentClaim {
        basis: "Front label and visible bays".into(),
        assertion: Some(equipment_claim::Assertion::Alternatives(
            EquipmentAlternatives {
                candidates: vec![
                    EquipmentAlternative {
                        value: "DL160".into(),
                        probability_ppm: Some(600_000),
                        ..Default::default()
                    },
                    EquipmentAlternative {
                        value: "DL120".into(),
                        probability_ppm: Some(200_000),
                        ..Default::default()
                    },
                ],
                domain: "HPE rack models".into(),
                calibration_basis: Some("asserted corpus/v1".into()),
                residual_ppm: Some(200_000),
                ..Default::default()
            },
        )),
    }
}
#[test]
fn incomplete_probabilities_keep_unknown_mass_and_are_order_independent() {
    // A top-k list cannot become certainty; presentation order cannot cause writes.
    let a = possible();
    let mut b = a.clone();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut b.assertion else {
        panic!()
    };
    v.candidates.reverse();
    assert_eq!(encode(&a), encode(&b));
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut b.assertion else {
        panic!()
    };
    v.residual_ppm = Some(0);
    assert_eq!(validate(&b), Err(Error::Value));
}
#[test]
fn probability_score_and_calibration_are_distinct() {
    // Mixed scales or an undocumented calibration cannot masquerade as probability.
    let mut a = possible();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut a.assertion else {
        panic!()
    };
    v.candidates[0].score_ppm = Some(900_000);
    assert_eq!(validate(&a), Err(Error::Value));
    let mut a = possible();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut a.assertion else {
        panic!()
    };
    v.calibration_basis = None;
    assert_eq!(validate(&a), Err(Error::Missing));
}
#[test]
fn quantities_preserve_open_bounds_and_units() {
    // Twelve visible bays establish a minimum, not twelve disks or an exact total.
    let mut a = EquipmentClaim {
        basis: "Twelve visible bays; rear concealed".into(),
        assertion: Some(equipment_claim::Assertion::Quantity(QuantityRange {
            lower: Some(12),
            upper: None,
            unit: "count".into(),
            scale: 0,
        })),
    };
    assert_eq!(validate(&a), Ok(()));
    let Some(equipment_claim::Assertion::Quantity(v)) = &mut a.assertion else {
        panic!()
    };
    v.upper = Some(10);
    assert_eq!(validate(&a), Err(Error::Value));
}
#[test]
fn equivalent_fixed_point_quantities_have_one_current_encoding() {
    // Decimal spelling cannot create a replacement row for the same measurement.
    let mut a = EquipmentClaim {
        basis: "Measured height".into(),
        assertion: Some(equipment_claim::Assertion::Quantity(QuantityRange {
            lower: Some(120),
            upper: None,
            unit: "mm".into(),
            scale: 1,
        })),
    };
    let encoded = encode(&a).expect("bounded quantity");
    let Some(equipment_claim::Assertion::Quantity(q)) = &mut a.assertion else {
        panic!()
    };
    q.lower = Some(12);
    q.scale = 0;
    assert_eq!(encoded, encode(&a).expect("same quantity"));
}
#[test]
fn exclusions_need_evidence_and_work_is_bounded() {
    // Occlusion is not exclusion evidence; oversized sets are rejected before copying.
    let mut a = possible();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut a.assertion else {
        panic!()
    };
    v.excluded.push(EquipmentExclusion {
        value: "DL120".into(),
        ..Default::default()
    });
    assert_eq!(validate(&a), Err(Error::Value));
    let mut a = possible();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut a.assertion else {
        panic!()
    };
    v.candidates.resize(17, EquipmentAlternative::default());
    assert_eq!(encode(&a), Err(Error::Limit));
}
#[test]
fn conflicting_supported_exclusion_remains_available_for_review() {
    // A contradictory claim is preserved, rather than selecting the last arrival.
    let mut a = possible();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut a.assertion else {
        panic!()
    };
    v.excluded.push(EquipmentExclusion {
        value: "DL160".into(),
        reason: "Rear mounting pattern differs".into(),
        coverage: "Rear inspected".into(),
        reference: None,
    });
    assert_eq!(validate(&a), Ok(()));
    let input = decode(
        &encode(&a).expect("bounded conflicting claims"),
        &crate::MemoryBudget::new(1_048_576),
    )
    .expect("preserved conflict");
    let Some(equipment_claim::Assertion::Alternatives(v)) = input.value.assertion else {
        panic!()
    };
    assert!(v.candidates.iter().any(|c| c.value == "DL160"));
    assert_eq!(
        v.excluded,
        vec![EquipmentExclusion {
            value: "DL160".into(),
            reason: "Rear mounting pattern differs".into(),
            coverage: "Rear inspected".into(),
            reference: None
        }]
    );
}

#[test]
fn untrusted_candidate_count_is_refused_before_decode_allocation() {
    // The wire walk enforces the semantic count, not just a post-allocation check.
    let mut a = possible();
    let Some(equipment_claim::Assertion::Alternatives(v)) = &mut a.assertion else {
        panic!()
    };
    v.candidates.resize(17, EquipmentAlternative::default());
    let budget = crate::MemoryBudget::new(16 * 1_048_576);
    assert!(matches!(
        decode(&a.encode_to_vec(), &budget),
        Err(Error::Limit)
    ));
    assert_eq!(budget.used(), 0);
    let mut input = decode(&possible().encode_to_vec(), &budget).expect("bounded input");
    assert!(budget.used() > 0);
    let _value = input.take();
    assert!(budget.used() > 0);
    drop(input);
    assert_eq!(budget.used(), 0);
    assert!(matches!(
        decode_hex("00", &crate::MemoryBudget::new(0)),
        Err(Error::Limit)
    ));
    assert!(matches!(decode_hex("xz", &budget), Err(Error::Wire)));
    assert_eq!(budget.used(), 0);
}
