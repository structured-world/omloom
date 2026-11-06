use super::*;

// A missing permanent subject ID must not discard useful local observations.
// It supplies no continuity and is distinct from malformed supplied evidence.
#[test]
fn anonymous_report_is_valid_without_a_permanent_source_basis() {
    let report = api::HostReport {
        version: 1,
        platform: "linux".into(),
        boot_basis: "b".repeat(64),
        observed_at: 100,
        findings: vec![api::HostFinding {
            key: "hardware".into(),
            fields: vec![wire::Field {
                name: "name".into(),
                value: Some(wire::field::Value::Text("Observed host".into())),
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(validate_parts(&report, false).is_ok());
    assert!(validate_parts(&report, true).is_ok());
}

// Reject a forged runtime type at report validation, before resolving targets
// or attempting relation writes with installation-scoped authorization.
#[test]
fn runtime_candidate_is_an_installation_before_mutation() {
    let field = || wire::Field {
        name: "name".into(),
        value: Some(wire::field::Value::Text("Synthetic candidate".into())),
    };
    let mut report = api::HostReport {
        version: 1,
        platform: "linux".into(),
        source_basis: "a".repeat(64),
        boot_basis: "b".repeat(64),
        observed_at: 100,
        findings: vec![
            api::HostFinding {
                key: "runtime".into(),
                kind: wire::AssetKind::Service as i32,
                fields: vec![field()],
                ..Default::default()
            },
            api::HostFinding {
                key: "workload".into(),
                kind: wire::AssetKind::Workload as i32,
                fields: vec![field()],
                runtime: Some("runtime".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!(matches!(
        validate_parts(&report, false),
        Err(Error::Invalid)
    ));
    assert!(matches!(validate_parts(&report, true), Err(Error::Invalid)));
    // A partial refresh may omit runtime context or list it later. A declared
    // incompatible runtime must still be rejected before selecting findings.
    report.findings.reverse();
    assert!(matches!(validate_parts(&report, true), Err(Error::Invalid)));
}
