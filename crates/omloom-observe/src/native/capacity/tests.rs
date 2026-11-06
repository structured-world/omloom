use super::*;

fn report(platform: &str) -> api::HostReport {
    api::HostReport {
        platform: platform.into(),
        findings: vec![finding(
            "installation",
            wire::AssetKind::SoftwareInstallation,
            None,
            platform,
        )],
        ..Default::default()
    }
}

// Capacity is an attributed OS descriptor with explicit byte units. Repeated
// processor descriptions are one descriptor, never repeated identity evidence.
#[tokio::test]
async fn linux_capacity_uses_bytes_and_deduplicates_models() -> Result<()> {
    const INPUTS: &[(&str, &str)] = &[
        ("getconf", "8\n"),
        ("cat", "MemTotal: 16384 kB\nMemFree: 100 kB\n"),
        (
            "cat",
            "processor: 0\nmodel name: Fixture CPU\nprocessor: 1\nmodel name: Fixture CPU\n",
        ),
    ];
    let mut report = report("linux");
    collect(&mut Runner::scripted(INPUTS, 16 * 1024 * 1024), &mut report).await?;
    let fields = &report.findings[0].fields;
    assert_eq!(
        fields
            .iter()
            .find(|f| f.name == "visible_memory_bytes")
            .ok_or("memory")?
            .value,
        Some(wire::field::Value::Integer(16_777_216))
    );
    assert_eq!(
        fields
            .iter()
            .find(|f| f.name == "cpu_model")
            .ok_or("model")?
            .value,
        Some(wire::field::Value::Text("Fixture CPU".into()))
    );
    assert_eq!(report.coverage[0].status, "verified");
    assert!(report.findings[0].identity.is_empty());
    Ok(())
}

// Native macOS byte/count sources retain their actual units without claiming
// that visible capacity certifies chassis inventory or permanent identity.
#[tokio::test]
async fn mac_capacity_preserves_native_counts_and_bytes() -> Result<()> {
    const INPUTS: &[(&str, &str)] = &[
        ("/usr/sbin/sysctl", "12\n"),
        ("/usr/sbin/sysctl", "34359738368\n"),
        ("/usr/sbin/sysctl", "Fixture CPU\n"),
    ];
    let mut report = report("macos");
    collect(&mut Runner::scripted(INPUTS, 16 * 1024 * 1024), &mut report).await?;
    assert_eq!(
        report.findings[0]
            .fields
            .iter()
            .find(|f| f.name == "logical_cpus")
            .ok_or("cpus")?
            .value,
        Some(wire::field::Value::Integer(12))
    );
    assert_eq!(report.coverage[0].status, "verified");
    assert!(report.findings[0].identity.is_empty());
    Ok(())
}

// An absent Windows property is unknown, not zero, and does not discard the
// other readable capacity fields or invent an installation ID.
#[tokio::test]
async fn windows_missing_capacity_is_partial_not_zero() -> Result<()> {
    const INPUTS: &[(&str, &str)] = &[(
        "Win32_ComputerSystem",
        r#"{"cpus":8,"memory":null,"model":"Fixture CPU"}"#,
    )];
    let mut report = report("windows");
    collect(&mut Runner::scripted(INPUTS, 16 * 1024 * 1024), &mut report).await?;
    assert!(
        !report.findings[0]
            .fields
            .iter()
            .any(|f| f.name == "visible_memory_bytes")
    );
    assert!(
        report.findings[0]
            .fields
            .iter()
            .any(|f| f.name == "logical_cpus")
    );
    assert_eq!(report.coverage[0].status, "partial");
    Ok(())
}

// Unsupported units and overflowing source values cannot become believable
// matching evidence through normalization or saturating arithmetic.
#[tokio::test]
async fn capacity_rejects_unsupported_units_and_overflow() {
    const UNITS: &[(&str, &str)] = &[("getconf", "8"), ("cat", "MemTotal: 16384 MB\n")];
    const OVERFLOW: &[(&str, &str)] = &[
        ("getconf", "8"),
        ("cat", "MemTotal: 9223372036854775807 kB\n"),
    ];
    for inputs in [UNITS, OVERFLOW] {
        let mut report = report("linux");
        assert!(
            collect(&mut Runner::scripted(inputs, 16 * 1024 * 1024), &mut report)
                .await
                .is_err()
        );
        assert_eq!(report.findings[0].fields.len(), 1);
    }
}

// A malformed model must not make an otherwise useful report unimportable.
// All native profiles omit that value and retain independent CPU/memory facts.
#[tokio::test]
async fn cpu_model_controls_are_unavailable_on_all_profiles() -> Result<()> {
    const MAC: &[(&str, &str)] = &[
        ("/usr/sbin/sysctl", "8"),
        ("/usr/sbin/sysctl", "16384"),
        ("/usr/sbin/sysctl", "Bad\0CPU"),
    ];
    const LINUX: &[(&str, &str)] = &[
        ("getconf", "8"),
        ("cat", "MemTotal: 16 kB\n"),
        ("cat", "model name: Bad\0CPU\n"),
    ];
    const WINDOWS: &[(&str, &str)] = &[(
        "Win32_ComputerSystem",
        r#"{"cpus":8,"memory":16384,"model":"Bad\u0000CPU"}"#,
    )];
    for (platform, inputs) in [("macos", MAC), ("linux", LINUX), ("windows", WINDOWS)] {
        let mut value = report(platform);
        collect(&mut Runner::scripted(inputs, 16 * 1024 * 1024), &mut value).await?;
        assert!(
            !value.findings[0]
                .fields
                .iter()
                .any(|f| f.name == "cpu_model"),
            "{platform}"
        );
        assert!(
            value.findings[0]
                .fields
                .iter()
                .any(|f| f.name == "logical_cpus"),
            "{platform}"
        );
        assert_eq!(value.coverage[0].status, "partial", "{platform}");
    }
    Ok(())
}

// An omitted MemTotal is unknown, not a reason to discard already read CPUs or
// stop collecting the following model descriptor.
#[tokio::test]
async fn missing_memtotal_retains_other_capacity_fields() -> Result<()> {
    const INPUTS: &[(&str, &str)] = &[
        ("getconf", "8"),
        ("cat", "MemFree: 16 kB\n"),
        ("cat", "model name: Fixture CPU\n"),
    ];
    let mut value = report("linux");
    collect(&mut Runner::scripted(INPUTS, 16 * 1024 * 1024), &mut value).await?;
    assert!(
        value.findings[0]
            .fields
            .iter()
            .any(|f| f.name == "logical_cpus")
    );
    assert!(
        value.findings[0]
            .fields
            .iter()
            .any(|f| f.name == "cpu_model")
    );
    assert!(
        !value.findings[0]
            .fields
            .iter()
            .any(|f| f.name == "visible_memory_bytes")
    );
    assert_eq!(value.coverage[0].status, "partial");
    Ok(())
}
