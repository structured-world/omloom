use super::*;
use api::{reply::Result as Reply, request::Command};

fn adopt(store: &Store) -> Result<api::Mutation, Error> {
    host_import::accept(store, &host_import::preview(store, host_import::draft())?)
}
fn refresh(store: &Store, host: &wire::AssetRef) -> Result<api::HostImport, Error> {
    let mut request = host_import::draft();
    request.source.clear();
    request.report.as_mut().unwrap().observed_at = 200;
    let Reply::HostSourceOptions(options) = call(
        store,
        &operator(),
        Command::HostSources(api::HostSources {
            host: Some(host.clone()),
            report: request.report.clone(),
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    let source = options.sources.into_iter().next().ok_or(Error::Missing)?;
    request.selection = source.matches;
    request.refresh = Some(api::HostRefresh {
        host: Some(host.clone()),
        source_ref: source.reference,
    });
    Ok(request)
}
fn read(store: &Store, target: &wire::AssetRef) -> Result<api::Detail, Error> {
    let Reply::Detail(detail) = call(
        store,
        &operator(),
        Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(*detail)
}

// Refresh uses a source binding and cannot rename accepted intent, adopt new
// findings, or update parents merely because their child is selected.
#[test]
fn existing_only_refresh_preserves_intent_and_unselected_parents() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let before_host = read(&store, &accepted.targets[0])?;
    let before_os = read(&store, &accepted.targets[1])?;
    let mut request = refresh(&store, &accepted.targets[0])?;
    request.selection.retain(|v| v.key == "interface/en0");
    request.report.as_mut().unwrap().findings[2].fields =
        vec![text_field("name", "Observed interface")];
    let review = host_import::preview(&store, request)?;
    assert_eq!(review.rows.len(), 1);
    let result = host_import::accept(&store, &review)?;
    assert_eq!(result.targets, vec![accepted.targets[2].clone()]);
    assert_eq!(before_host, read(&store, &accepted.targets[0])?);
    assert_eq!(before_os, read(&store, &accepted.targets[1])?);
    let interface = read(&store, &accepted.targets[2])?;
    assert_eq!(
        interface.item.as_ref().unwrap().fields,
        vec![text_field("name", "en0")]
    );
    assert_eq!(interface.observed.len(), 1);
    assert_eq!(
        interface.observed[0].evidence.as_ref().unwrap().fields,
        vec![text_field("name", "Observed interface")]
    );
    Ok(())
}

// An unknown finding cannot acquire a permanent identity through the refresh
// command, even when a caller bypasses the disabled adoption controls.
#[test]
fn refresh_rejects_new_findings_and_wrong_source_without_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let mut request = refresh(&store, &accepted.targets[0])?;
    let node = request
        .report
        .as_mut()
        .unwrap()
        .findings
        .last()
        .unwrap()
        .clone();
    request
        .report
        .as_mut()
        .unwrap()
        .findings
        .push(api::HostFinding {
            key: "interface/new".into(),
            ..node
        });
    request.selection.push(api::HostSelection {
        key: "interface/new".into(),
        ..Default::default()
    });
    let before = store.usage()?;
    assert!(matches!(
        host_import::preview(&store, request),
        Err(Error::Conflict)
    ));
    let mut request = refresh(&store, &accepted.targets[0])?;
    request.refresh.as_mut().unwrap().source_ref = format!("host-mapping/{}", "f".repeat(64));
    assert!(matches!(
        host_import::preview(&store, request),
        Err(Error::Missing)
    ));
    assert_eq!(before, store.usage()?);
    Ok(())
}

// Repeated packages are no-ops. A later partial observation reattests only
// supplied fields, leaving omitted evidence at its original collection instant.
#[test]
fn equal_and_partial_refresh_keep_current_only_field_windows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let mut request = refresh(&store, &accepted.targets[0])?;
    request.selection.retain(|v| v.key == "interface/en0");
    request.report.as_mut().unwrap().findings[2]
        .fields
        .push(text_field("description", "Useful interface"));
    host_import::accept(&store, &host_import::preview(&store, request.clone())?)?;
    let evidence = read(&store, &accepted.targets[2])?;
    let source = &evidence.observed[0].evidence.as_ref().unwrap().source_id;
    let value_key = format!(
        "asset/{}/observed/{source}/name",
        accepted.targets[2].asset_id
    );
    let value_version = row_version(&store, &value_key)?;
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &operator(), Command::HostImport(request.clone()))?,
        Reply::Mutation(_)
    ));
    assert_eq!(before, store.usage()?);
    request.report.as_mut().unwrap().observed_at = 300;
    request.report.as_mut().unwrap().findings[2]
        .fields
        .retain(|v| v.name == "name");
    let updated = host_import::preview(&store, request)?;
    assert_eq!(
        updated.rows[0].previous_fields,
        vec![text_field("name", "en0")]
    );
    host_import::accept(&store, &updated)?;
    let detail = read(&store, &accepted.targets[2])?;
    let window = |name| {
        detail
            .observed
            .iter()
            .filter_map(|o| o.evidence.as_ref())
            .find(|e| e.fields.iter().any(|f| f.name == name))
            .unwrap()
            .observed_at_ms
    };
    assert_eq!(window("name"), 300);
    assert_eq!(window("description"), 200);
    assert_eq!(
        row_version(&store, &value_key)?,
        value_version,
        "freshening an equal value must not rewrite its row"
    );
    let before = store.usage()?;
    read(&store, &accepted.targets[2])?;
    assert_eq!(before, store.usage()?);
    Ok(())
}

// Source options and preview must not become a permission grant. Revocation
// after review and a competing accepted import both make acceptance fail atomically.
#[test]
fn refresh_rechecks_permissions_and_basis_at_acceptance() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let request = refresh(&store, &accepted.targets[0])?;
    let review = host_import::preview(&store, request.clone())?;
    let mut denied = operator();
    denied.grants[0].writable = false;
    let before = store.usage()?;
    assert!(
        call(
            &store,
            &denied,
            Command::HostImport(api::HostImport {
                preview: review.id.clone(),
                ..Default::default()
            })
        )
        .is_err()
    );
    assert_eq!(before, store.usage()?);
    let mut other = request;
    other.report.as_mut().unwrap().observed_at = 300;
    let competing = host_import::preview(&store, other)?;
    host_import::accept(&store, &competing)?;
    assert!(matches!(
        host_import::accept(&store, &review),
        Err(Error::Rejected(error)) if matches!(*error, Error::Expired)
    ));
    Ok(())
}

// The importer is source provenance, not an implicit exclusive writer. Another
// currently authorized operator reuses the binding, without forking its source.
#[test]
fn permitted_operator_refreshes_original_source_without_duplicate_mapping() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let mut actor = operator();
    actor.actor = "another-operator".into();
    let mut request = refresh(&store, &accepted.targets[0])?;
    let before = store.usage()?;
    let Reply::HostSourceOptions(options) = call(
        &store,
        &actor,
        Command::HostSources(api::HostSources {
            host: Some(accepted.targets[0].clone()),
            report: request.report.clone(),
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(options.sources.len(), 1);
    assert_eq!(before, store.usage()?);
    request.refresh.as_mut().unwrap().source_ref = options.sources[0].reference.clone();
    let Reply::Preview(preview) = call(&store, &actor, Command::HostImport(request))? else {
        return Err(Error::Corrupt.into());
    };
    let Reply::Mutation(result) = call(
        &store,
        &actor,
        Command::HostImport(api::HostImport {
            preview: preview.id,
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(result.targets, accepted.targets);
    let Reply::Page(page) = call(&store, &actor, Command::Query(query()))? else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(page.total, 3);
    Ok(())
}

// Independent configurations stay independent despite equal host identity and
// labels. A selected source never overwrites another source's current fields.
#[test]
fn refresh_keeps_other_source_fields_and_rejects_old_collection() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let mut second = host_import::draft();
    second.source = "Independent source".into();
    for (selection, target) in second.selection.iter_mut().zip(&accepted.targets) {
        selection.target = Some(target.clone());
    }
    host_import::accept(&store, &host_import::preview(&store, second)?)?;
    let mut first = refresh(&store, &accepted.targets[0])?;
    first.selection.retain(|v| v.key == "interface/en0");
    first.report.as_mut().unwrap().findings[2].fields =
        vec![text_field("name", "First source observation")];
    host_import::accept(&store, &host_import::preview(&store, first.clone())?)?;
    let mut second = first.clone();
    let Reply::HostSourceOptions(options) = call(
        &store,
        &operator(),
        Command::HostSources(api::HostSources {
            host: Some(accepted.targets[0].clone()),
            report: second.report.clone(),
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    second.refresh.as_mut().unwrap().source_ref = options
        .sources
        .iter()
        .find(|s| s.label == "Independent source")
        .unwrap()
        .reference
        .clone();
    second.report.as_mut().unwrap().findings[2].fields =
        vec![text_field("name", "Second source observation")];
    host_import::accept(&store, &host_import::preview(&store, second)?)?;
    let detail = read(&store, &accepted.targets[2])?;
    assert_eq!(detail.observed.len(), 2);
    let mut values = detail
        .observed
        .iter()
        .flat_map(|o| &o.evidence.as_ref().unwrap().fields)
        .cloned()
        .collect::<Vec<_>>();
    values.sort_by_key(|f| format!("{f:?}"));
    assert_eq!(
        values,
        vec![
            text_field("name", "First source observation"),
            text_field("name", "Second source observation")
        ]
    );
    first.report.as_mut().unwrap().observed_at = 150;
    let before = store.usage()?;
    assert!(matches!(
        host_import::preview(&store, first),
        Err(Error::Conflict)
    ));
    assert_eq!(before, store.usage()?);
    Ok(())
}

// Parent context is read for validation only. Updating an existing interface
// must not require write authority over the enclosing host or its OS.
#[test]
fn refreshing_child_requires_no_parent_write_grant() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let mut request = refresh(&store, &accepted.targets[0])?;
    request.selection.retain(|v| v.key == "interface/en0");
    let mut actor = operator();
    actor.grants[0].writable = false;
    actor.grants.push(Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&accepted.targets[2])?]),
        read: Fields::All,
        write: Fields::All,
        writable: true,
    });
    let Reply::Preview(review) = call(&store, &actor, Command::HostImport(request))? else {
        return Err(Error::Corrupt.into());
    };
    let Reply::Mutation(result) = call(
        &store,
        &actor,
        Command::HostImport(api::HostImport {
            preview: review.id,
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(result.targets, vec![accepted.targets[2].clone()]);
    assert!(read(&store, &accepted.targets[0])?.observed.is_empty());
    assert!(read(&store, &accepted.targets[1])?.observed.is_empty());
    Ok(())
}

// Concurrent delivery may race at the database CAS boundary. Every retry must
// settle to the one retained outcome, without duplicate field claims or assets.
#[test]
fn concurrent_acceptance_settles_to_one_refresh_outcome() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let review = host_import::preview(&store, refresh(&store, &accepted.targets[0])?)?;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|_| {
                let store = &store;
                let review = &review;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    host_import::accept(store, review)
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(results.iter().any(Result::is_ok));
    let before = store.usage()?;
    let final_result = host_import::accept(&store, &review)?;
    assert_eq!(final_result.targets, accepted.targets);
    assert_eq!(before, store.usage()?);
    assert_eq!(read(&store, &accepted.targets[2])?.observed.len(), 1);
    Ok(())
}

// Batch membership uses the same relation visibility as individual scope
// checks. A readable interface cannot disclose or cross a hidden parent path.
#[test]
fn source_options_and_refresh_do_not_cross_hidden_ownership_edges() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let accepted = adopt(&store)?;
    let mut request = refresh(&store, &accepted.targets[0])?;
    let mut actor = operator();
    store.set_diagnostic_query_check(|db, statement| {
        if statement.contains("AS parent, a.key AS child")
            || statement.starts_with("UNWIND $keys AS key MATCH (n:OmRow {key:key})")
        {
            let plan = db.explain_cypher(statement).expect("source lookup plan");
            eprintln!("source lookup plan: {plan}");
            assert!(
                plan.contains("IndexScan"),
                "source lookups must start from indexed selected keys"
            );
        }
    });
    actor.grants[0].read = Fields::Selected(vec![
        "name".into(),
        "identity_evidence".into(),
        "current_addresses".into(),
    ]);
    let before = store.usage()?;
    let Reply::HostSourceOptions(options) = call(
        &store,
        &actor,
        Command::HostSources(api::HostSources {
            host: Some(accepted.targets[0].clone()),
            report: request.report.clone(),
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(
        options.sources[0]
            .matches
            .iter()
            .map(|m| m.key.as_str())
            .collect::<Vec<_>>(),
        vec!["hardware"]
    );
    request.selection.retain(|m| m.key == "interface/en0");
    assert!(matches!(
        call(&store, &actor, Command::HostImport(request)),
        Err(Error::Denied)
    ));
    assert_eq!(before, store.usage()?);
    Ok(())
}
