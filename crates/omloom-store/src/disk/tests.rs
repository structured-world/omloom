use super::*;

// Exhausting a supported statement memory ceiling is admission failure, not corruption.
#[test]
fn statement_memory_exhaustion_is_a_resource_limit() {
    let error = Error::from(DatabaseError::Budget(
        coordinode_core::budget::BudgetStop::Memory {
            requested: 4096,
            used: 8192,
            limit: 8192,
        },
    ));
    assert!(matches!(error, Error::Limit));
}

// A real old catalog differs from a new catalog with an old application marker.
// Opening must admit the additive declaration before migrating any row.
#[test]
fn old_strict_catalog_upgrades_without_recreating_the_database()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let policy = Policy {
        classes: [crate::Budget {
            rows: 1024,
            bytes: 1_048_576,
        }; CLASSES],
        total_bytes: 8_388_608,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: crate::Budget {
            rows: 128,
            bytes: 1_048_576,
        },
        batch_rows: 128,
    };
    let store = Store::open(dir.path(), policy.clone())?;
    let mut old = store
        .db
        .label_schemas()?
        .into_iter()
        .find(|s| s.name == "OmRow")
        .ok_or(Error::Corrupt)?;
    for field in [
        "address",
        "meaning",
        "zone",
        "binding",
        "ip_order",
        "prefix_length",
        "working",
    ] {
        old.properties.remove(field);
    }
    let expected_revision = old.schema_revision;
    let mut declaration = LabelSchema::new_node_id("OmRow");
    declaration.properties = old.properties;
    let publication = store.db.publish_schema(SchemaPackage {
        labels: vec![LabelDeclaration {
            schema: declaration,
            intent: Intent::Change { expected_revision },
        }],
        ..Default::default()
    })?;
    assert!(
        store
            .db
            .await_schema(&publication, Duration::from_secs(20))?
            .all_ready()
    );
    let mut tx = store.begin()?;
    tx.root.insert("schema".into(), Value::Int(2));
    tx.changed = true;
    tx.commit()?;
    drop(store);
    let store = Store::open(dir.path(), policy)?;
    assert_eq!(
        number(&store.read(ROOT)?.ok_or(Error::Corrupt)?, "schema")?,
        SCHEMA_VERSION
    );
    Ok(())
}

// A resumed format upgrade rebuilds only current projections and preserves a
// restored installation's mutation fence, including after another reopen.
#[test]
fn network_projection_upgrade_resumes_without_removing_restore_fence()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let policy = Policy {
        classes: [crate::Budget {
            rows: 1024,
            bytes: 1_048_576,
        }; CLASSES],
        total_bytes: 8_388_608,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: crate::Budget {
            rows: 128,
            bytes: 1_048_576,
        },
        batch_rows: 128,
    };
    for restored in [false, true] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy.clone())?;
        let mut tx = store.begin()?;
        for index in 0..130 {
            let mut data = props([("cidr", text(format!("192.0.2.{index}/32")))]);
            // The first batch models an already committed migration prefix.
            if index < 128 {
                data.insert("ip_order".into(), text(format!("4/c00002{index:02x}")));
                data.insert("prefix_len".into(), Value::Int(32));
            }
            tx.put(
                &format!("network/fixture-{index}"),
                "networks",
                Class::Inventory,
                data,
            )?;
        }
        tx.root.insert("schema".into(), Value::Int(2));
        tx.root.insert("restored".into(), Value::Bool(restored));
        tx.changed = true;
        tx.commit()?;
        drop(store);
        let store = Store::open(dir.path(), policy.clone())?;
        let mut tx = store.begin()?;
        assert_eq!(number(&tx.root, "schema")?, SCHEMA_VERSION);
        assert_eq!(tx.root.get("restored"), Some(&Value::Bool(restored)));
        assert!(tx.query("MATCH (n:OmRow {parent:'networks'}) WHERE n.ip_order IS NULL OR n.prefix_len IS NULL RETURN properties(n) AS p",None)?.is_empty());
        if restored {
            assert!(matches!(
                tx.put("blocked", "assets", Class::Inventory, Properties::new()),
                Err(Error::RecoveryRequired)
            ));
        }
        tx.commit()?;
        let before = store.usage()?;
        drop(store);
        let store = Store::open(dir.path(), policy.clone())?;
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}
