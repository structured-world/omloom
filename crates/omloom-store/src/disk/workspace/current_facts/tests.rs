use super::*;
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn policy() -> Policy {
    Policy {
        classes: [crate::Budget {
            rows: 20_000,
            bytes: 32 * 1_048_576,
        }; CLASSES],
        total_bytes: 128 * 1_048_576,
        detail_age_ms: 1000,
        review_age_ms: 500,
        review_per_source: crate::Budget {
            rows: 128,
            bytes: 1_048_576,
        },
        batch_rows: 128,
    }
}
fn evidence(target: AssetRef) -> api::HostNetwork {
    api::HostNetwork {
        installation: Some(target_wire(target)),
        source: "collector".into(),
        basis: "boot".into(),
        observed_at: 100,
        socket_coverage: "verified".into(),
        route_coverage: "verified".into(),
        application_coverage: "verified".into(),
        packet_filter_coverage: "blocked".into(),
        routes: vec![api::LocalRoute {
            destination: "default".into(),
            gateway: "192.0.2.1".into(),
            interface: "en0".into(),
            flags: "UG".into(),
            family: "IPv4".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

// Each migration iteration must consume its matched row, even at a legacy alias key.
#[test]
fn legacy_network_migration_consumes_the_matched_key() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let owner = new_target();
    let value = evidence(owner);
    let mut tx = store.begin()?;
    let key = "legacy-alias/host-network";
    tx.put(
        key,
        &asset_key(owner),
        Class::Inventory,
        props([
            ("layer", text("host-network")),
            ("payload", Value::Binary(value.encode_to_vec())),
        ]),
    )?;
    let row = tx.get(key)?.ok_or("legacy row")?;
    super::super::network_facts::migrate_network_row(&mut tx, &row)?;
    assert!(
        tx.get(key)?.is_none(),
        "the upgrade loop would select the same row forever"
    );
    tx.commit()?;
    let before = store.usage()?;
    super::super::network_facts::upgrade(&store)?;
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Reopening old disk state migrates values once and never invents precision lost by the old format.
#[test]
fn legacy_network_and_address_rows_migrate_atomically_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let installation = new_target();
    let interface = new_target();
    let value = evidence(installation);
    let mut tx = store.begin()?;
    let network_key = format!("{}/host-network", asset_key(installation));
    let mut old = value.clone();
    old.observed_at = 0;
    tx.put(
        &format!("{network_key}-body"),
        &asset_key(installation),
        Class::Inventory,
        props([
            ("layer", text("host-network")),
            ("payload", Value::Binary(old.encode_to_vec())),
        ]),
    )?;
    tx.put(
        &network_key,
        &asset_key(installation),
        Class::Inventory,
        props([("observed", int(100)?)]),
    )?;
    tx.put(
        &asset_key(interface),
        "assets",
        Class::Inventory,
        target_props(interface),
    )?;
    let address = api::CurrentAddress {
        address: "192.0.2.10".into(),
        ..Default::default()
    };
    tx.put(
        &format!("{}/current-addresses", asset_key(interface)),
        &asset_key(interface),
        Class::Inventory,
        props([
            ("observed", int(150)?),
            ("vtext", text("collector")),
            ("vflag", Value::Bool(false)),
        ]),
    )?;
    tx.put(
        &format!("{}/current-address/192.0.2.10", asset_key(interface)),
        &asset_key(interface),
        Class::Inventory,
        props([
            ("layer", text("interface-address")),
            ("payload", Value::Binary(address.encode_to_vec())),
        ]),
    )?;
    tx.root.insert("schema".into(), int(6)?);
    tx.changed = true;
    tx.commit()?;
    let mut schema = store
        .db
        .label_schemas()?
        .into_iter()
        .find(|s| s.name == "OmRow")
        .ok_or("schema")?;
    schema.properties.remove("count");
    schema.properties.remove("bytes");
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
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &backup, policy(), backup.len() as u64)?;
    assert_eq!(
        number(&restored.read(ROOT)?.ok_or("root")?, "schema")?,
        crate::disk::SCHEMA_VERSION
    );
    assert_eq!(
        restored.read(ROOT)?.ok_or("root")?.get("restored"),
        Some(&Value::Bool(true))
    );
    let store = Store::open(dir.path(), policy())?;
    assert!(store.read(&network_key)?.is_none());
    let tx = store.begin()?;
    let rows = tx.query(
        "MATCH (n:OmRow {layer:'network-route'}) RETURN properties(n) AS p",
        None,
    )?;
    assert_eq!(rows.len(), 1);
    let windows = tx.fact_windows(&rows)?;
    assert_eq!(
        number(windows.values().next().ok_or("window")?, "observed")?,
        100
    );
    let rows = tx.query(
        "MATCH (n:OmRow {layer:$layer}) RETURN properties(n) AS p",
        params([("layer", text(interface_facts::address_layer(interface)))]),
    )?;
    assert_eq!(rows.len(), 1);
    assert_eq!(binary(&rows[0], "payload")?, address.encode_to_vec());
    assert!(
        !tx.fact_windows(&rows)?
            .values()
            .next()
            .ok_or("window")?
            .contains_key("observed")
    );
    tx.commit()?;
    let before = store.usage()?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Metadata refresh rewrites no value, exact replay fsyncs nothing, and archive reclaims owned facts.
#[test]
fn current_fact_write_cost_and_retirement_are_bounded() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let owner = new_target();
    let mut value = evidence(owner);
    let mut tx = store.begin()?;
    tx.accept_network(&value)?;
    tx.commit()?;
    store.db.persist()?;
    let usage = store.usage()?;
    let syncs = store.db.engine().journal_syncs();
    store.db.query_registry().reset();
    let mut tx = store.begin()?;
    tx.accept_network(&value)?;
    tx.commit()?;
    assert_eq!(store.usage()?, usage);
    assert_eq!(store.db.engine().journal_syncs(), syncs);
    assert!(
        store
            .db
            .query_registry()
            .top_by_impact(64)
            .iter()
            .all(|q| !q.canonical_query.contains("SET"))
    );
    value.observed_at = 200;
    store.db.query_registry().reset();
    let mut tx = store.begin()?;
    tx.accept_network(&value)?;
    tx.commit()?;
    let writes: u64 = store
        .db
        .query_registry()
        .top_by_impact(64)
        .iter()
        .filter(|q| q.canonical_query.contains("SET"))
        .map(|q| q.count)
        .sum();
    assert_eq!(
        writes, 3,
        "only the source checkpoint, fact window and transaction root may change"
    );
    assert_eq!(store.usage()?.rows, usage.rows);
    assert_eq!(store.usage()?.bytes, usage.bytes);
    let mut tx = store.begin()?;
    tx.retire_current_facts(owner)?;
    tx.commit()?;
    assert_eq!(store.usage()?.rows[Class::Inventory as usize], 0);
    Ok(())
}

// Native route flags can distinguish two concurrent entries with the same visible next hop.
#[test]
fn native_route_variants_remain_distinct_until_complete_replacement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut value = evidence(new_target());
    value.routes[0].flags = "UGScg".into();
    let mut scoped = value.routes[0].clone();
    scoped.flags = "UGS1cIg".into();
    value.routes.push(scoped);
    let mut tx = store.begin()?;
    tx.accept_network(&value)?;
    tx.commit()?;
    let tx = store.begin()?;
    let routes = tx.query(
        "MATCH (n:OmRow {layer:'network-route'}) RETURN properties(n) AS p",
        None,
    )?;
    assert_eq!(routes.len(), 2);
    tx.commit()?;
    value.observed_at = 200;
    value.routes_complete = true;
    value.routes.remove(0);
    let mut tx = store.begin()?;
    tx.accept_network(&value)?;
    tx.commit()?;
    let tx = store.begin()?;
    let routes = tx.query(
        "MATCH (n:OmRow {layer:'network-route'}) RETURN properties(n) AS p",
        None,
    )?;
    assert_eq!(routes.len(), 1);
    assert_eq!(
        binary(&routes[0], "payload")?,
        value.routes[0].encode_to_vec()
    );
    tx.commit()?;
    Ok(())
}
