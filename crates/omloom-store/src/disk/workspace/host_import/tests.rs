use super::*;

// Reject a forged runtime type at report validation, before resolving targets
// or attempting relation writes with installation-scoped authorization.
#[test]
fn runtime_candidate_is_an_installation_before_mutation() {
    let field = || wire::Field {
        name: "name".into(),
        value: Some(wire::field::Value::Text("Synthetic candidate".into())),
    };
    let report = api::HostReport {
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
    assert!(matches!(validate(&report), Err(Error::Invalid)));
}
