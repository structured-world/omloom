use super::*;
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
fn policy() -> Policy {
    Policy {
        classes: [crate::Budget {
            rows: 10000,
            bytes: 16 * 1024 * 1024,
        }; CLASSES],
        total_bytes: 128 * 1024 * 1024,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: crate::Budget {
            rows: 512,
            bytes: 1024 * 1024,
        },
        batch_rows: 128,
    }
}

// Qualifying a proven legacy binding preserves older omitted facts and their
// collection windows. Repeating the same partial input must remain a no-op.
#[test]
fn legacy_source_qualification_preserves_partial_windows_and_replay() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let target = new_target();
    let mut value = api::InterfaceAddresses {
        interface: Some(target_wire(target)),
        source: "Collector".into(),
        observed_at: 100,
        addresses: vec![api::CurrentAddress {
            address: "192.0.2.1".into(),
            stability: api::AddressStability::Stable as i32,
            state: api::AddressState::Preferred as i32,
            ..Default::default()
        }],
        complete: true,
    };
    let binding = "host-mapping/proof";
    let mut tx = store.begin()?;
    tx.accept_addresses(&value)?;
    value.observed_at = 150;
    value.complete = false;
    value.addresses[0].address = "192.0.2.2".into();
    tx.accept_addresses(&value)?;
    tx.put(
        binding,
        &format!("{}/host-mappings", asset_key(target)),
        Class::Inventory,
        props([("source", text(&value.source)), ("observed", int(150)?)]),
    )?;
    tx.commit()?;
    let mut tx = store.begin()?;
    let old = Family::Addresses.key(target, &value.source, None);
    let key = tx.native_source(target, &value.source, binding, binding, Family::Addresses)?;
    assert!(tx.get(&old)?.is_none());
    let facts = tx.fact_rows(&format!("{key}/values"), 64)?;
    assert_eq!(facts.len(), 2);
    let mut windows = Vec::new();
    for fact in facts {
        let row = tx
            .get(&format!("{}/freshness", string(&fact, "key")?))?
            .ok_or(Error::Corrupt)?;
        windows.push(number(&row, "observed")?);
    }
    windows.sort();
    assert_eq!(windows, vec![100, 150]);
    tx.commit()?;
    let context = Context {
        actor: "test".into(),
        fixture: "operator".into(),
        grants: vec![Grant {
            scope: Scope::All,
            read: Fields::All,
            write: Fields::All,
            writable: true,
        }],
        teams: Vec::new(),
        recipients: Vec::new(),
        schema_admin: false,
        authority_revision: 1,
    };
    let before = store.usage()?;
    let mut tx = store.begin()?;
    tx.accept_native_addresses(&context, &value, binding)?;
    tx.commit()?;
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Restartable claim backfill uses the regular accounting path and does not
// release a restore fence or alter observation freshness.
#[test]
fn identity_lookup_upgrade_preserves_restore_fence_and_evidence() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let target = new_target();
    let claim = api::HostIdentityClaim {
        kind: api::HostIdentityKind::HardwareUuid as i32,
        namespace: "smbios/system-uuid".into(),
        digest: "a".repeat(64),
        quality: api::HostIdentityQuality::Reported as i32,
    };
    let mut tx = store.begin()?;
    let mut data = target_props(target);
    data.insert("payload".into(), Value::Binary(claim.encode_to_vec()));
    data.insert("observed".into(), int(100)?);
    tx.put(
        "host-identity/legacy",
        &host_identity::parent(target),
        Class::Inventory,
        data,
    )?;
    tx.root.insert("schema".into(), int(11)?);
    tx.root.insert("restored".into(), Value::Bool(true));
    tx.changed = true;
    tx.commit()?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let tx = store.begin()?;
    assert_eq!(tx.root.get("restored"), Some(&Value::Bool(true)));
    assert_eq!(number(&tx.root, "schema")?, 12);
    let row = tx.get("host-identity/legacy")?.ok_or(Error::Corrupt)?;
    assert_eq!(number(&row, "observed")?, 100);
    assert_eq!(
        string(&row, "claim")?,
        format!(
            "{}/smbios/system-uuid/{}",
            api::HostIdentityKind::HardwareUuid as i32,
            "a".repeat(64)
        )
    );
    tx.commit()?;
    Ok(())
}
