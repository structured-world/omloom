use super::*;
mod equipment;

// Name projections may exhaust an old budget; explicit increases must unblock recovery without relaxing retention or restore fences.
#[test]
fn capacity_increase_resumes_quota_bound_name_migration()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let initial = Policy {
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
    let store = Store::open(dir.path(), initial)?;
    let mut tx = store.begin()?;
    tx.put(
        "asset/legacy",
        "assets",
        Class::Inventory,
        props([("revision", Value::Int(4))]),
    )?;
    tx.put(
        "asset/legacy/intent/name",
        "asset/legacy",
        Class::Inventory,
        props([
            ("field", text("name")),
            ("vtype", Value::Int(1)),
            ("vtext", text("Legacy device")),
        ]),
    )?;
    tx.root.insert("schema".into(), Value::Int(7));
    tx.root.insert("restored".into(), Value::Bool(true));
    tx.changed = true;
    tx.commit()?;
    let mut previous = store.policy.clone();
    previous.classes[Class::Inventory as usize].bytes =
        store.usage()?.bytes[Class::Inventory as usize];
    // Reproduce a real legacy quota, rather than reserving migration-only uncharged bytes.
    let legacy = Store {
        db: store.db,
        policy: previous.clone(),
        #[cfg(feature = "diagnostics")]
        query_check: None,
    };
    let mut tx = legacy.begin()?;
    tx.root
        .insert("policy".into(), text(legacy.policy_identity()));
    tx.changed = true;
    tx.commit()?;
    drop(legacy);
    assert!(matches!(
        Store::open(dir.path(), previous.clone()),
        Err(Error::Limit)
    ));
    let mut next = previous.clone();
    next.classes[Class::Inventory as usize].bytes += 4096;
    assert!(matches!(
        Store::open(dir.path(), next.clone()),
        Err(Error::PolicyChanged)
    ));
    // A committed total increase can still leave the class full. Reopening
    // must resume against that accepted policy, without losing the old rows.
    let mut partial = previous.clone();
    partial.total_bytes += 4096;
    assert!(matches!(
        Store::open_with_capacity_increase(dir.path(), &previous, partial.clone()),
        Err(Error::Limit)
    ));
    assert!(matches!(
        Store::open(dir.path(), partial.clone()),
        Err(Error::Limit)
    ));
    assert!(matches!(
        Store::open(dir.path(), previous),
        Err(Error::PolicyChanged)
    ));
    let previous = partial;
    next.total_bytes = previous.total_bytes;
    let store = Store::open_with_capacity_increase(dir.path(), &previous, next.clone())?;
    assert_eq!(
        string(&store.read("asset/legacy")?.ok_or(Error::Missing)?, "name")?,
        "Legacy device"
    );
    assert_eq!(
        number(
            &store.read("asset/legacy")?.ok_or(Error::Missing)?,
            "revision"
        )?,
        4
    );
    assert_eq!(
        number(&store.read(ROOT)?.ok_or(Error::Missing)?, "schema")?,
        SCHEMA_VERSION
    );
    assert_eq!(
        store.read(ROOT)?.ok_or(Error::Missing)?.get("restored"),
        Some(&Value::Bool(true))
    );
    let before = store.usage()?;
    drop(store);
    let store = Store::open(dir.path(), next.clone())?;
    assert_eq!(store.usage()?, before);
    drop(store);
    let mut wrong = next.clone();
    wrong.detail_age_ms += 1;
    assert!(matches!(
        Store::open_with_capacity_increase(dir.path(), &next, wrong),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Store::open_with_capacity_increase(dir.path(), &next, previous.clone()),
        Err(Error::Invalid)
    ));
    let mut raised = next.clone();
    raised.total_bytes += 4096;
    assert!(matches!(
        Store::open_with_capacity_increase(dir.path(), &previous, raised),
        Err(Error::PolicyChanged)
    ));
    assert_eq!(Store::open(dir.path(), next)?.usage()?, before);
    Ok(())
}

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
    let old = store
        .db
        .label_schemas()?
        .into_iter()
        .find(|s| s.name == "OmRow")
        .ok_or(Error::Corrupt)?;
    let expected_revision = old.schema_revision;
    // The complete version-2 declaration is independent of the current schema.
    let mut declaration = LabelSchema::new_node_id("OmRow");
    declaration.add_property(PropertyDef::new("key", PropertyType::String).not_null());
    for name in [
        "candidate",
        "cursor",
        "snapshot",
        "parent",
        "policy",
        "asset",
        "incarnation",
        "layer",
        "authority",
        "connection",
        "object_kind",
        "object_id",
        "lifetime",
        "field",
        "source",
        "claim",
        "source_revision",
        "scan",
        "operation",
        "caller",
        "action",
        "provider_task",
        "worker",
        "actor",
        "to_asset",
        "to_incarnation",
        "vtext",
    ] {
        declaration.add_property(PropertyDef::new(name, PropertyType::String));
    }
    for name in [
        "completeness",
        "review_rows",
        "review_bytes",
        "schema",
        "revision",
        "class",
        "size",
        "kind",
        "adoption",
        "observed",
        "valid",
        "inventory_revision",
        "state",
        "outcome",
        "at",
        "expires",
        "epoch",
        "until",
        "vtype",
        "vint",
        "settlement_rows",
        "settlement_bytes",
    ] {
        declaration.add_property(PropertyDef::new(name, PropertyType::Int));
    }
    for name in ["restored", "pinned", "vflag", "settled"] {
        declaration.add_property(PropertyDef::new(name, PropertyType::Bool));
    }
    for name in ["counts", "sizes"] {
        declaration.add_property(PropertyDef::new(
            name,
            PropertyType::Array(Box::new(PropertyType::Int)),
        ));
    }
    declaration.add_property(PropertyDef::new("digest", PropertyType::Binary));
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
    tx.put(
        "preserved-v2-field",
        "legacy",
        Class::Inventory,
        props([
            ("field", text("description")),
            ("vtype", Value::Int(1)),
            ("vtext", text("preserved value")),
        ]),
    )?;
    for (key, name) in [("asset/zulu", "Alpha"), ("asset/alpha", "Zulu")] {
        tx.put(
            key,
            "assets",
            Class::Inventory,
            props([("revision", Value::Int(4))]),
        )?;
        tx.put(
            &format!("{key}/intent/name"),
            key,
            Class::Inventory,
            props([
                ("field", text("name")),
                ("vtype", Value::Int(1)),
                ("vtext", text(name)),
            ]),
        )?;
    }
    tx.root.insert("schema".into(), Value::Int(2));
    tx.changed = true;
    tx.commit()?;
    drop(store);
    let store = Store::open(dir.path(), policy.clone())?;
    assert_eq!(
        number(&store.read(ROOT)?.ok_or(Error::Corrupt)?, "schema")?,
        SCHEMA_VERSION
    );
    assert_eq!(
        inventory::read_field(&store.read("preserved-v2-field")?.ok_or(Error::Missing)?)?.value,
        Some(wire::field::Value::Text("preserved value".into()))
    );
    let before = store.usage()?;
    let tx = store.begin()?;
    let named = tx.query(
        "MATCH (n:OmRow {parent:'assets'}) RETURN properties(n) AS p ORDER BY n.name_sort",
        None,
    )?;
    assert_eq!(string(&named[0], "name")?, "Alpha");
    assert_eq!(string(&named[1], "name")?, "Zulu");
    assert_eq!(number(&named[0], "revision")?, 4);
    tx.commit()?;
    drop(store);
    let store = Store::open(dir.path(), policy)?;
    assert_eq!(
        store.usage()?,
        before,
        "reopening an upgraded catalog must not rewrite it"
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
            let key = format!("asset/{index:03}");
            let name = format!("Resource {index:03}");
            let mut asset = props([("revision", Value::Int(3))]);
            if index < 128 {
                asset.insert("name".into(), text(&name));
                asset.insert("name_sort".into(), text(name.to_lowercase()));
            }
            tx.put(&key, "assets", Class::Inventory, asset)?;
            tx.put(
                &format!("{key}/intent/name"),
                &key,
                Class::Inventory,
                props([
                    ("field", text("name")),
                    ("vtype", Value::Int(1)),
                    ("vtext", text(name)),
                ]),
            )?;
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
        assert!(tx.query("MATCH (n:OmRow {parent:'assets'}) WHERE n.name IS NULL OR n.name_sort IS NULL OR n.revision<>3 RETURN properties(n) AS p",None)?.is_empty());
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
