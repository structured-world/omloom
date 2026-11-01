use super::*;

// Readable Windows tables must produce verified coverage and preserve their endpoint/route values.
#[tokio::test]
async fn readable_windows_tables_have_verified_coverage() -> Result<()> {
    let mut report = api::HostReport {
        boot_basis: "fixture-boot".into(),
        platform: "windows".into(),
        network: Some(api::HostNetwork::default()),
        ..Default::default()
    };
    const INPUTS: &[(&str, &str)] = &[
        (
            "Get-NetIPAddress",
            r#"[{"InterfaceIndex":5,"InterfaceAlias":"Ethernet","IPAddress":"192.0.2.5","PrefixLength":24,"State":"Preferred"}]"#,
        ),
        (
            "Get-NetTCPConnection",
            r#"[{"protocol":"tcp","bind":"0.0.0.0","port":443,"pid":17}]"#,
        ),
        (
            "Get-NetRoute",
            r#"[{"DestinationPrefix":"192.0.2.0/24","NextHop":"192.0.2.1","InterfaceIndex":5,"Family":"IPv4","Protocol":"Local","CompartmentId":1}]"#,
        ),
        (
            "Get-NetFirewallProfile",
            r#"[{"Name":"Public","Enabled":"True","Inbound":"Block","Outbound":"Allow"}]"#,
        ),
    ];
    let bytes = INPUTS.iter().map(|(_, output)| output.len()).sum::<usize>();
    let budget = 16 * 1024 * 1024;
    let mut runner = Runner::scripted(INPUTS, budget);
    collect(&mut runner, &mut report).await?;
    assert_eq!(runner.budget(), (4, budget - bytes));
    let network = report.network.as_ref().expect("network");
    assert!(network.routes_complete);
    assert_eq!(network.routes.len(), 1);
    assert_eq!(network.routes[0].destination, "192.0.2.0/24");
    assert_eq!(network.routes[0].interface, "5");
    assert_eq!(report.binds.len(), 1);
    assert_eq!(report.binds[0].value.as_ref().expect("socket").port, 443);
    assert!(report.network.as_ref().expect("network").sockets_complete);
    assert_eq!(
        report
            .coverage
            .iter()
            .filter(|v| ["interfaces", "sockets", "routes", "packet-filter"]
                .contains(&v.module.as_str()))
            .map(|v| (v.module.as_str(), v.status.as_str()))
            .collect::<Vec<_>>(),
        [
            ("interfaces", "verified"),
            ("sockets", "verified"),
            ("routes", "verified"),
            ("packet-filter", "verified")
        ]
    );
    assert!(windows_routes(r#"[{"DestinationPrefix":"192.0.2.0/24"}]"#).is_err());
    assert!(windows_sockets(r#"[{"protocol":"tcp"}]"#, &mut report).is_err());
    Ok(())
}

// Parser failures identify the same native query as command failures without exposing source data.
#[test]
fn windows_identity_parser_errors_name_the_source() {
    for input in [
        "untrusted-private-output",
        "{}",
        r#"{"source":"installation"}"#,
        r#"{"source":"installation","boot":"session"}"#,
    ] {
        let error = windows_identity(input).err().expect("incomplete source");
        assert!(error.contains(WINDOWS_IDENTITY_SOURCE), "{error}");
        assert!(!error.contains("untrusted-private-output"));
    }
}

// Command timeouts leave source coverage unavailable, never verified or a fatal collection status.
#[test]
fn timed_out_sources_are_unavailable_with_safe_command_context() {
    assert_eq!(
        failure_status(
            "Command deadline exceeded (powershell.exe); source: Windows routes (Get-NetRoute)"
        ),
        "unavailable"
    );
    assert_eq!(
        failure_status("Permission denied; source: Windows firewall (Get-NetFirewallProfile)"),
        "blocked"
    );
}

// Exercise the native OS source on each qualified host, without assuming that
// an unreadable or unset hardware UUID exists or inventing one from an address.
#[tokio::test]
async fn native_identity_observation_keeps_layers_separate() -> Result<()> {
    let namespace = match std::env::consts::OS {
        "macos" => "macos/apfs-volume",
        "linux" => "linux/machine-id",
        "windows" => "windows/machine-guid",
        _ => return Err("Unsupported native identity platform".into()),
    };
    let value = match identity(&mut Runner::new()).await {
        Ok(value) => value,
        Err(error) => {
            let dir = tempfile::tempdir().expect("temporary directory");
            let path = dir.path().join("unavailable.omloom");
            assert!(
                crate::tests::source_unavailable(&error),
                "Unexpected identity failure: {error}"
            );
            crate::finish_collection(Err(error), &path)?;
            assert!(!path.exists());
            return Ok(());
        }
    };
    assert!(!value.source.is_empty());
    assert!(!value.boot.is_empty());
    assert_eq!(value.installation.namespace, namespace);
    assert_eq!(
        value.installation.kind,
        api::HostIdentityKind::InstallationId as i32
    );
    assert_eq!(value.installation.digest.len(), 64);
    if let Some(hardware) = value.hardware {
        assert_eq!(hardware.kind, api::HostIdentityKind::HardwareUuid as i32);
        assert_eq!(hardware.quality, api::HostIdentityQuality::Reported as i32);
        assert_eq!(hardware.digest.len(), 64);
        assert_ne!(hardware.namespace, value.installation.namespace);
    }
    Ok(())
}

// Normalized claims compare across boots, while distinct hardware UUIDs stay distinct.
// A MAC, IP or hostname is never substituted for missing hardware evidence.
#[test]
fn hardware_claims_are_scoped_normalized_and_explicitly_untrusted() {
    let first = hardware_claim("smbios/system-uuid", "8D60C19A-489A-40C9-B381-D97E5D568DF8")
        .expect("source UUID");
    let second = hardware_claim("smbios/system-uuid", "8d60c19a-489a-40c9-b381-d97e5d568df8")
        .expect("source UUID");
    assert_eq!(first, second);
    let other = hardware_claim("smbios/system-uuid", "11111111-2222-4333-8444-555555555555")
        .expect("distinct source UUID");
    assert_ne!(first.digest, other.digest);
    assert_eq!(first.quality, api::HostIdentityQuality::Reported as i32);
    assert_eq!(first.digest.len(), 64);
    assert!(!first.digest.contains("8d60c19a"));
    assert_ne!(first.namespace, "apple/platform-uuid");
    for unknown in [
        "",
        "192.0.2.1",
        "To be filled by OEM",
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ] {
        assert!(hardware_claim("smbios/system-uuid", unknown).is_none());
    }
}

// An installation ID remains a distinct namespace and layer even when a source
// happens to report the same bytes as its hardware UUID.
#[test]
fn installation_claims_do_not_collapse_into_hardware_identity() {
    let raw = "8d60c19a-489a-40c9-b381-d97e5d568df8";
    let hardware = hardware_claim("smbios/system-uuid", raw).expect("UUID");
    let installation = identity_claim(
        api::HostIdentityKind::InstallationId,
        "windows/machine-guid",
        raw,
    )
    .expect("installation");
    assert_eq!(hardware.digest, installation.digest);
    assert_ne!(hardware.namespace, installation.namespace);
    assert_ne!(hardware.kind, installation.kind);
    assert!(
        identity_claim(
            api::HostIdentityKind::InstallationId,
            "linux/machine-id",
            "\n"
        )
        .is_err()
    );
}
