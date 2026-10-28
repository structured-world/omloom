use super::*;
use api::{reply::Result as Reply, request::Command};

fn draft() -> api::HostImport {
    api::HostImport {
        source: "Native fixture".into(),
        report: Some(Box::new(api::HostReport {
            version: 1,
            platform: "macos".into(),
            source_basis: "a".repeat(64),
            boot_basis: "b".repeat(64),
            observed_at: 100,
            findings: vec![
                api::HostFinding {
                    key: "hardware".into(),
                    kind: K::Device as i32,
                    fields: vec![text_field("name", "Native host")],
                    ..Default::default()
                },
                api::HostFinding {
                    key: "installation".into(),
                    kind: K::SoftwareInstallation as i32,
                    parent: Some("hardware".into()),
                    fields: vec![text_field("name", "Native OS")],
                    ..Default::default()
                },
                api::HostFinding {
                    key: "interface/en0".into(),
                    kind: K::Interface as i32,
                    parent: Some("installation".into()),
                    fields: vec![text_field("name", "en0")],
                    addresses: vec![api::CurrentAddress {
                        address: "192.0.2.7".into(),
                        prefix_length: Some(24),
                        stability: api::AddressStability::Stable as i32,
                        state: api::AddressState::Preferred as i32,
                    }],
                    addresses_complete: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        })),
        selection: ["hardware", "installation", "interface/en0"]
            .into_iter()
            .map(|key| api::HostSelection {
                key: key.into(),
                target: None,
            })
            .collect(),
        preview: None,
        network_parts: Vec::new(),
    }
}
fn preview(store: &Store, request: api::HostImport) -> Result<api::Preview, Error> {
    let Reply::Preview(value) = call(store, &operator(), Command::HostImport(request))? else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}
fn accept(store: &Store, preview: &api::Preview) -> Result<api::Mutation, Error> {
    let Reply::Mutation(value) = call(
        store,
        &operator(),
        Command::HostImport(api::HostImport {
            preview: preview.id.clone(),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}
// Preview cannot adopt inventory. Acceptance commits the whole selected tree;
// a repeated exact report after reopen must create no writes or duplicate assets.
#[test]
fn reviewed_host_import_is_atomic_and_replay_is_a_noop() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let proposed = preview(&store, draft())?;
    let Reply::Page(page) = call(
        &store,
        &operator(),
        Command::Query(api::Query {
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("page".into());
    };
    assert_eq!(page.total, 0);
    let accepted = accept(&store, &proposed)?;
    assert_eq!(accepted.targets.len(), 3);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let Reply::Mutation(result) = call(&store, &operator(), Command::HostImport(draft()))? else {
        return Err("no-op".into());
    };
    assert!(result.targets.is_empty());
    assert_eq!(before, store.usage()?);
    Ok(())
}
// Unselected parent identity is not replaced with a fabricated OS/device.
#[test]
fn selection_cannot_invent_a_missing_parent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let mut request = draft();
    request.selection.remove(0);
    assert!(matches!(preview(&store, request), Err(Error::Invalid)));
    assert_eq!(before, store.usage()?);
    Ok(())
}
// Temporary addresses are refused at the store boundary even if a modified
// collector bypasses its own serialization filter.
#[test]
fn temporary_address_in_a_host_report_never_reaches_inventory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let mut request = draft();
    request.report.as_mut().unwrap().findings[2].addresses[0].stability =
        api::AddressStability::Temporary as i32;
    assert!(matches!(preview(&store, request), Err(Error::Invalid)));
    assert_eq!(before, store.usage()?);
    Ok(())
}
// Concurrent inventory edits invalidate the reviewed revision rather than
// accepting an old report against a different ancestry or field authority.
#[test]
fn host_preview_rechecks_the_commit_revision() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let proposed = preview(&store, draft())?;
    create(&store, &operator(), K::Device, "Concurrent host")?;
    assert!(matches!(accept(&store, &proposed), Err(Error::Conflict)));
    Ok(())
}

// Every complete parent prefix is independently reviewable; validating a
// nested report cannot require committing its parents before the preview.
#[test]
fn each_selected_parent_prefix_can_be_previewed() -> TestResult {
    for count in 1..=3 {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let mut request = draft();
        request.selection.truncate(count);
        let result = preview(&store, request);
        assert!(result.is_ok(), "selected parent prefix {count}: {result:?}");
    }
    Ok(())
}
