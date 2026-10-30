use super::*;

// Syntax and explicit types cannot be replaced with length-only checks or registration policy.
#[test]
fn typed_locators_validate_syntax_without_normalizing_identity() {
    use api::LocatorKind as K;
    for (kind, valid, invalid) in [
        (K::Email, "User+tag@example.test", "not an email"),
        (K::Email, "\"a b\"@localhost", "Alice <user@example.com>"),
        (K::Phone, "+442079460018", "02079460018"),
        (K::Phone, "+12025550123", "+123"),
        (
            K::Web,
            "https://example.test/support",
            "javascript:alert(1)",
        ),
        (
            K::Web,
            "http://localhost/support",
            "https://user:password@example.test/",
        ),
        (K::Domain, "münchen.de", "bad domain.com"),
        (K::Domain, "internal.test", "a..test"),
        (K::Domain, "co.uk", "example.com.."),
        (K::Domain, "example.com.", "192.0.2.1"),
    ] {
        assert!(locator(kind as i32, valid).is_ok(), "{kind:?}: {valid}");
        assert!(
            matches!(locator(kind as i32, invalid), Err(Error::Invalid)),
            "{kind:?}: {invalid}"
        );
    }
    assert!(locator(K::Unspecified as i32, "Ask the account owner").is_ok());
    assert!(locator(999, "anything").is_err());
}

// Existing format-12 databases and backups acquire optional discriminator
// columns without reclassifying legacy routes or rewriting their payloads.
#[test]
fn typed_values_upgrade_format_twelve_schema_and_backup()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let policy = Policy {
        classes: [crate::Budget {
            rows: 1000,
            bytes: 1_048_576,
        }; CLASSES],
        total_bytes: 8 * 1_048_576,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: crate::Budget {
            rows: 128,
            bytes: 1_048_576,
        },
        batch_rows: 128,
    };
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy.clone())?;
    let mut tx = store.begin()?;
    tx.put(
        "legacy-route",
        "fixture",
        Class::Inventory,
        props([("contact", text("Ask the owner"))]),
    )?;
    tx.root.insert("schema".into(), int(12)?);
    tx.changed = true;
    tx.commit()?;
    let mut schema = store
        .db
        .label_schemas()?
        .into_iter()
        .find(|s| s.name == "OmRow")
        .ok_or("schema")?;
    for name in ["contact_kind", "sender_kind", "recovery_route_kind"] {
        schema.properties.remove(name);
    }
    let expected_revision = schema.schema_revision;
    let mut declaration = LabelSchema::new_node_id("OmRow");
    declaration.properties = schema.properties;
    let publication = store.db.publish_schema(SchemaPackage {
        labels: vec![LabelDeclaration {
            intent: Intent::Change { expected_revision },
            schema: declaration,
        }],
        ..Default::default()
    })?;
    assert!(
        store
            .db
            .await_schema(&publication, Duration::from_secs(20))?
            .all_ready()
    );
    let mut backup = Vec::new();
    store.export(&mut backup)?;
    drop(store);
    let opened = Store::open(dir.path(), policy.clone())?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &backup, policy, backup.len() as u64)?;
    for store in [opened, restored] {
        assert_eq!(number(&store.read(ROOT)?.ok_or("root")?, "schema")?, 13);
        let legacy = store.read("legacy-route")?.ok_or("route")?;
        assert_eq!(string(&legacy, "contact")?, "Ask the owner");
        assert_eq!(
            read_kind(&legacy, "contact_kind")?,
            api::LocatorKind::Unspecified as i32
        );
        assert!(!legacy.contains_key("contact_kind"));
        let schema = store
            .db
            .label_schemas()?
            .into_iter()
            .find(|s| s.name == "OmRow")
            .ok_or("schema")?;
        for name in ["contact_kind", "sender_kind", "recovery_route_kind"] {
            assert_eq!(schema.properties[name].property_type, PropertyType::Int);
        }
    }
    Ok(())
}
