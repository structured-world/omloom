use super::*;

// Exercise the native OS source on each qualified host, without assuming that
// an unreadable or unset hardware UUID exists or inventing one from an address.
#[tokio::test]
async fn native_identity_observation_keeps_layers_separate() -> Result<()> {
    let value = identity(&mut Runner::new()).await?;
    let namespace = match std::env::consts::OS {
        "macos" => "macos/apfs-volume",
        "linux" => "linux/machine-id",
        "windows" => "windows/machine-guid",
        _ => return Err("Unsupported native identity platform".into()),
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

// Identical normalized claims compare across boots without storing the raw UUID.
// A MAC, IP or hostname is never substituted for missing hardware evidence.
#[test]
fn hardware_claims_are_scoped_normalized_and_explicitly_untrusted() {
    let first = hardware_claim("smbios/system-uuid", "8D60C19A-489A-40C9-B381-D97E5D568DF8")
        .expect("source UUID");
    let second = hardware_claim("smbios/system-uuid", "8d60c19a-489a-40c9-b381-d97e5d568df8")
        .expect("source UUID");
    assert_eq!(first, second);
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
