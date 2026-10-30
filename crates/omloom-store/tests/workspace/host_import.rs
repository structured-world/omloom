use super::*;
use api::{reply::Result as Reply, request::Command};

pub(super) fn draft() -> api::HostImport {
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
                kind: None,
            })
            .collect(),
        preview: None,
        network_parts: Vec::new(),
    }
}

fn identity_claim(
    kind: api::HostIdentityKind,
    namespace: &str,
    digest: char,
) -> api::HostIdentityClaim {
    api::HostIdentityClaim {
        kind: kind as i32,
        namespace: namespace.into(),
        digest: digest.to_string().repeat(64),
        quality: api::HostIdentityQuality::Reported as i32,
    }
}

// Identity equality is layer-specific evidence, not a host/OS merge. Missing
// later claims retain their exact old collection window rather than freshening.
#[test]
fn host_identity_evidence_is_current_scoped_and_not_freshened_by_omission() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    let report = request.report.as_mut().unwrap();
    report.findings[0].identity.push(identity_claim(
        api::HostIdentityKind::HardwareUuid,
        "apple/platform-uuid",
        'a',
    ));
    report.findings[1].identity.push(identity_claim(
        api::HostIdentityKind::InstallationId,
        "macos/apfs-volume",
        'a',
    ));
    let accepted = accept(&store, &preview(&store, request.clone())?)?;
    let host = accepted.targets[0].clone();
    let installation = accepted.targets[1].clone();
    let read = |target: wire::AssetRef| -> Result<api::Detail, Error> {
        let Reply::Detail(value) = call(
            &store,
            &operator(),
            Command::Read(api::Read {
                target: Some(target),
                ..Default::default()
            }),
        )?
        else {
            return Err(Error::Corrupt);
        };
        Ok(*value)
    };
    let first = read(host.clone())?;
    let os = read(installation)?;
    assert_eq!(first.identity.len(), 1);
    assert_eq!(os.identity.len(), 1);
    assert_ne!(
        first.identity[0].claim.as_ref().unwrap().kind,
        os.identity[0].claim.as_ref().unwrap().kind
    );
    assert_eq!(first.identity[0].observed_at, 100);
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &operator(), Command::HostImport(request.clone()))?,
        Reply::Mutation(_)
    ));
    assert_eq!(
        before,
        store.usage()?,
        "identical claim input writes nothing"
    );
    request.report.as_mut().unwrap().observed_at = 200;
    request.report.as_mut().unwrap().findings[0]
        .identity
        .clear();
    accept(&store, &preview(&store, request)?)?;
    let current = read(host)?;
    assert_eq!(
        first.identity, current.identity,
        "omission does not reattest identity"
    );
    let before = store.usage()?;
    read(accepted.targets[0].clone())?;
    assert_eq!(
        before,
        store.usage()?,
        "reading claims creates no history or touches"
    );
    Ok(())
}

// A source cannot change a claim at the same instant or place installation
// evidence on a hardware identity, even through a forged native report.
#[test]
fn host_identity_rejects_layer_forgery_and_same_instant_equivocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request.report.as_mut().unwrap().findings[0]
        .identity
        .push(identity_claim(
            api::HostIdentityKind::InstallationId,
            "linux/machine-id",
            'a',
        ));
    let before = store.usage()?;
    assert!(matches!(
        preview(&store, request.clone()),
        Err(Error::Invalid)
    ));
    assert_eq!(before, store.usage()?);
    request.report.as_mut().unwrap().findings[0].identity[0].kind =
        api::HostIdentityKind::HardwareUuid as i32;
    accept(&store, &preview(&store, request.clone())?)?;
    request.report.as_mut().unwrap().findings[0].identity[0].digest = "b".repeat(64);
    let before = store.usage()?;
    assert!(matches!(preview(&store, request), Err(Error::Conflict)));
    assert_eq!(before, store.usage()?);
    Ok(())
}

// Pair comparison reads only the two selected current subjects, applies each
// field's actual rights and cannot freshen evidence or change inventory.
#[test]
fn host_identity_comparison_is_read_only_and_checks_both_subjects() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request.report.as_mut().unwrap().findings[0]
        .identity
        .push(identity_claim(
            api::HostIdentityKind::HardwareUuid,
            "apple/platform-uuid",
            'a',
        ));
    let accepted = accept(&store, &preview(&store, request)?)?;
    let pair = api::CompareHostIdentity {
        left: Some(accepted.targets[0].clone()),
        right: Some(accepted.targets[1].clone()),
        include_relations: false,
    };
    let before = store.usage()?;
    let Reply::HostIdentityComparison(value) = call(
        &store,
        &operator(),
        Command::CompareHostIdentity(pair.clone()),
    )?
    else {
        return Err("comparison".into());
    };
    assert_eq!(value.left.as_ref().unwrap().identity.len(), 1);
    assert!(value.right.as_ref().unwrap().identity.is_empty());
    assert_eq!(value.revision, before.revision);
    assert_eq!(before, store.usage()?);
    let mut restricted = operator();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    let Reply::HostIdentityComparison(value) = call(
        &store,
        &restricted,
        Command::CompareHostIdentity(pair.clone()),
    )?
    else {
        return Err("restricted comparison".into());
    };
    assert!(
        value.left.unwrap().identity.is_empty(),
        "hidden claims are not disclosed on a readable host"
    );
    restricted.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&accepted.targets[0])?]);
    assert!(matches!(
        call(
            &store,
            &restricted,
            Command::CompareHostIdentity(pair.clone())
        ),
        Err(Error::Denied)
    ));
    assert_eq!(before, store.usage()?);
    let unrelated = create(&store, &operator(), K::Device, "Outside selected scope")?;
    let before = store.usage()?;
    let result = store.workspace(
        &operator(),
        api::Request {
            working: Some(api::WorkingContext {
                roots: vec![unrelated],
                ..Default::default()
            }),
            command: Some(Command::CompareHostIdentity(pair)),
        },
        100,
    );
    assert!(
        matches!(result, Err(Error::Denied)),
        "a broader actor grant cannot silently widen the working context"
    );
    assert_eq!(before, store.usage()?);
    Ok(())
}

// Address replacement follows the existing reviewed mapping, affects only its
// source and cannot move a service's literal bind to the new interface address.
#[test]
fn host_address_preview_is_a_source_local_delta_without_identity_changes() -> TestResult {
    fn detail(store: &Store, target: &wire::AssetRef) -> Result<api::Detail, Error> {
        let Reply::Detail(value) = call(
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
        Ok(*value)
    }
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request
        .report
        .as_mut()
        .unwrap()
        .findings
        .push(api::HostFinding {
            key: "service".into(),
            kind: K::Service as i32,
            parent: Some("installation".into()),
            fields: vec![text_field("name", "Literal listener")],
            ..Default::default()
        });
    request.selection.push(api::HostSelection {
        key: "service".into(),
        ..Default::default()
    });
    let initial = preview(&store, request.clone()).map_err(|e| format!("initial preview: {e}"))?;
    let accepted = accept(&store, &initial).map_err(|e| format!("initial acceptance: {e}"))?;
    let interface = &accepted.targets[2];
    let service = &accepted.targets[3];
    let address = |value: &str| api::CurrentAddress {
        address: value.into(),
        prefix_length: Some(24),
        stability: api::AddressStability::Stable as i32,
        state: api::AddressState::Preferred as i32,
    };
    call(
        &store,
        &operator(),
        Command::InterfaceAddresses(api::InterfaceAddresses {
            interface: Some(interface.clone()),
            source: "Independent source".into(),
            observed_at: 100,
            addresses: vec![address("192.0.2.8")],
            complete: true,
        }),
    )?;
    store.workspace(
        &operator(),
        api::Request {
            working: Some(api::WorkingContext {
                focus: Some(accepted.targets[1].clone()),
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
            command: Some(Command::HostNetwork(api::HostNetwork {
                installation: Some(accepted.targets[1].clone()),
                source: "Listener source".into(),
                observed_at: 100,
                basis: "b".repeat(64),
                sockets: vec![api::SocketBind {
                    service: Some(service.clone()),
                    protocol: "tcp".into(),
                    family: "IPv4".into(),
                    bind: "192.0.2.7".into(),
                    port: 443,
                    ..Default::default()
                }],
                socket_coverage: "verified".into(),
                route_coverage: "not collected".into(),
                application_coverage: "not collected".into(),
                packet_filter_coverage: "not collected".into(),
                ..Default::default()
            })),
        },
        100,
    )?;
    let listener = detail(&store, service)
        .map_err(|e| format!("read listener: {e}"))?
        .host_network;
    assert!(!listener.is_empty());
    let report = request.report.as_mut().unwrap();
    report.observed_at = 200;
    report.findings[2].addresses = vec![address("192.0.2.19")];
    let plan = preview(&store, request.clone()).map_err(|e| format!("replacement preview: {e}"))?;
    let delta = plan.rows[2].addresses.as_ref().ok_or("address delta")?;
    assert_eq!(delta.source, "Native fixture");
    assert_eq!(delta.added, vec![address("192.0.2.19")]);
    assert_eq!(delta.removed, vec![address("192.0.2.7")]);
    assert!(delta.updated.is_empty());
    assert!(delta.complete);
    let mut restricted = operator();
    restricted.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    let before_restricted_review = store.usage()?;
    let Reply::Preview(hidden) = call(&store, &restricted, Command::HostImport(request.clone()))
        .map_err(|e| format!("restricted review: {e}"))?
    else {
        return Err("restricted preview".into());
    };
    assert_eq!(
        hidden.id, plan.id,
        "repeating the same review does not store another copy"
    );
    assert!(
        hidden.rows.iter().all(|row| row.addresses.is_none()),
        "a cached review cannot disclose previously readable addresses after revocation"
    );
    assert_eq!(store.usage()?, before_restricted_review);
    assert_eq!(accept(&store, &plan)?.targets, accepted.targets);
    assert_eq!(detail(&store, service)?.host_network, listener);
    let current = detail(&store, interface)?.current_addresses;
    assert_eq!(current.len(), 2);
    assert!(current.iter().any(
        |s| s.coverage.as_ref().unwrap().source == "Independent source"
            && s.observations[0].value.as_ref().unwrap().address == "192.0.2.8"
    ));
    let report = request.report.as_mut().unwrap();
    report.observed_at = 300;
    report.findings[2].addresses = vec![address("192.0.2.20")];
    report.findings[2].addresses_complete = false;
    let plan = preview(&store, request)?;
    let delta = plan.rows[2].addresses.as_ref().ok_or("partial delta")?;
    assert_eq!(delta.added, vec![address("192.0.2.20")]);
    assert!(delta.removed.is_empty());
    assert!(!delta.complete);
    accept(&store, &plan)?;
    let current = detail(&store, interface)?.current_addresses;
    let native = current
        .iter()
        .find(|s| s.coverage.as_ref().unwrap().source == "Native fixture")
        .ok_or("native source")?;
    assert_eq!(
        native
            .observations
            .iter()
            .map(|v| (v.value.as_ref().unwrap().address.as_str(), v.observed_at))
            .collect::<Vec<_>>(),
        vec![("192.0.2.19", Some(200)), ("192.0.2.20", Some(300))]
    );
    assert_eq!(detail(&store, service)?.host_network, listener);
    Ok(())
}
pub(super) fn preview(store: &Store, request: api::HostImport) -> Result<api::Preview, Error> {
    let Reply::Preview(value) = call(store, &operator(), Command::HostImport(request))? else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}
pub(super) fn accept(store: &Store, preview: &api::Preview) -> Result<api::Mutation, Error> {
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
        // This fixture contains synthetic inventory only, without provider
        // accounts or credentials; failure diagnostics retain the typed result.
        assert!(result.is_ok(), "selected parent prefix {count}: {result:?}");
    }
    Ok(())
}

// Equal address bytes in separate source scopes do not identify one device or
// create address assets. Both accepted interface incarnations remain distinct.
#[test]
fn equal_ip_on_different_hosts_does_not_merge_inventory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let first = accept(&store, &preview(&store, draft())?)?;
    let mut other = draft();
    other.report.as_mut().expect("report").source_basis = "c".repeat(64);
    let second = accept(&store, &preview(&store, other)?)?;
    assert_ne!(first.targets[2], second.targets[2]);
    let Reply::Page(page) = call(
        &store,
        &operator(),
        Command::Query(api::Query {
            kinds: vec![K::IpAddress as i32],
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("page".into());
    };
    assert_eq!(page.total, 0);
    Ok(())
}

// Dual boot shares explicitly matched hardware, never the OS interface or
// service namespace. Observing another installation cannot erase the first.
#[test]
fn another_os_can_match_hardware_without_reusing_its_interfaces() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let first = accept(&store, &preview(&store, draft())?)?;
    let mut second = draft();
    let report = second.report.as_mut().expect("report");
    report.platform = "windows".into();
    report.source_basis = "d".repeat(64);
    report.boot_basis = "e".repeat(64);
    second.selection[0].target = Some(first.targets[0].clone());
    let second = accept(&store, &preview(&store, second)?)?;
    assert_eq!(first.targets[0], second.targets[0]);
    assert_ne!(first.targets[1], second.targets[1]);
    assert_ne!(first.targets[2], second.targets[2]);
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(first.targets[2].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    assert_eq!(
        detail.current_addresses[0].observations[0]
            .value
            .as_ref()
            .expect("current address")
            .address,
        "192.0.2.7"
    );
    Ok(())
}

// Running on an OS does not prove that its execution host is physical. Match
// an explicitly known VM without retyping it or inventing another Device.
#[test]
fn an_unclassified_execution_host_can_match_an_existing_vm() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let vm = create(&store, &operator(), K::VirtualMachine, "Known VM")?;
    let mut request = draft();
    request.report.as_mut().expect("report").findings[0].kind = K::Unspecified as i32;
    request.selection[0].target = Some(vm.clone());
    let accepted = accept(&store, &preview(&store, request)?)?;
    assert_eq!(accepted.targets[0], vm);
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(vm),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    assert_eq!(
        detail
            .item
            .as_ref()
            .expect("VM")
            .fields
            .iter()
            .find(|v| v.name == "name")
            .expect("name"),
        &text_field("name", "Known VM")
    );
    Ok(())
}

// An OS cannot classify its execution host. New adoption needs an explicit
// physical/VM choice, and a later source mapping cannot retype that choice.
#[test]
fn new_execution_host_requires_a_reviewed_kind() -> TestResult {
    for kind in [K::Device, K::VirtualMachine] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let mut request = draft();
        request.report.as_mut().expect("report").findings[0].kind = K::Unspecified as i32;
        let before = store.usage()?;
        assert!(matches!(
            preview(&store, request.clone()),
            Err(Error::HostClassificationRequired)
        ));
        assert_eq!(store.usage()?, before);
        request.selection[0].kind = Some(kind as i32);
        let accepted = accept(&store, &preview(&store, request.clone())?)?;
        let Reply::Detail(detail) = call(
            &store,
            &operator(),
            Command::Read(api::Read {
                target: Some(accepted.targets[0].clone()),
                ..Default::default()
            }),
        )?
        else {
            return Err("detail".into());
        };
        assert_eq!(detail.item.expect("host").kind, kind as i32);
        request.selection[0].kind = Some(if kind == K::Device {
            K::VirtualMachine
        } else {
            K::Device
        } as i32);
        assert!(matches!(preview(&store, request), Err(Error::Conflict)));
    }
    Ok(())
}

// A read-only actor cannot use report review as a write privilege, and refusal
// cannot leave a partial tree, preview payload or mapping behind.
#[test]
fn host_preview_requires_write_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut actor = operator();
    actor.grants[0].writable = false;
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &actor, Command::HostImport(draft())),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A file cannot relabel a Device as the reserved OS installation to borrow
// SoftwareInstallation-only network permission. Refusal leaves no preview.
#[test]
fn network_import_rejects_a_forged_installation_kind() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    let report = request.report.as_mut().expect("report");
    report.findings = vec![api::HostFinding {
        key: "installation".into(),
        kind: K::Device as i32,
        fields: vec![text_field("name", "Forged installation")],
        ..Default::default()
    }];
    report.network = Some(api::HostNetwork {
        socket_coverage: "not collected".into(),
        route_coverage: "verified".into(),
        application_coverage: "not collected".into(),
        packet_filter_coverage: "not collected".into(),
        ..Default::default()
    });
    request.selection = vec![api::HostSelection {
        key: "installation".into(),
        ..Default::default()
    }];
    request.network_parts = vec![api::HostNetworkPart::Routes as i32];
    let mut actor = operator();
    actor.grants[0].write = Fields::Selected(vec!["name".into()]);
    actor.grants.push(Grant {
        scope: Scope::Kinds(vec![K::SoftwareInstallation]),
        read: Fields::All,
        write: Fields::Selected(vec!["network_evidence".into()]),
        writable: true,
    });
    let before = store.usage()?;
    let result = call(&store, &actor, Command::HostImport(request));
    assert!(
        matches!(result, Err(Error::Invalid)),
        "forged installation type must be rejected"
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Runtime references must name installations, not a Service on which the actor
// has no relation write grant. Report keys cannot override kind-scoped grants.
#[test]
fn runtime_import_rejects_a_service_as_the_runtime() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request.report.as_mut().expect("report").findings = vec![
        api::HostFinding {
            key: "runtime".into(),
            kind: K::Service as i32,
            fields: vec![text_field("name", "Forged runtime")],
            ..Default::default()
        },
        api::HostFinding {
            key: "workload".into(),
            kind: K::Workload as i32,
            fields: vec![text_field("name", "Selected workload")],
            runtime: Some("runtime".into()),
            ..Default::default()
        },
    ];
    request.selection = ["runtime", "workload"]
        .into_iter()
        .map(|key| api::HostSelection {
            key: key.into(),
            ..Default::default()
        })
        .collect();
    let mut actor = operator();
    actor.grants[0].write = Fields::Selected(vec!["name".into()]);
    actor.grants.push(Grant {
        scope: Scope::Kinds(vec![K::SoftwareInstallation, K::Workload]),
        read: Fields::All,
        write: Fields::Selected(vec!["relations".into()]),
        writable: true,
    });
    let before = store.usage()?;
    let result = call(&store, &actor, Command::HostImport(request));
    assert!(
        matches!(result, Err(Error::Invalid)),
        "service cannot be an installation runtime"
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Reading a report does not opt the installation into route or policy storage.
// Explicit route selection accepts on-link rows without inventing a next hop.
#[test]
fn report_network_sections_require_explicit_selection() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request.report.as_mut().expect("report").network = Some(api::HostNetwork {
        observed_at: 100,
        basis: "b".repeat(64),
        source: "native".into(),
        socket_coverage: "not collected".into(),
        route_coverage: "verified".into(),
        application_coverage: "not collected".into(),
        packet_filter_coverage: "blocked".into(),
        routes: vec![api::LocalRoute {
            destination: "192.0.2.0/24".into(),
            interface: "en0".into(),
            family: "IPv4".into(),
            ..Default::default()
        }],
        routes_complete: true,
        ..Default::default()
    });
    let first = accept(&store, &preview(&store, request.clone())?)?;
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(first.targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    assert!(detail.host_network.is_empty());
    request
        .network_parts
        .push(api::HostNetworkPart::Routes as i32);
    accept(&store, &preview(&store, request)?)?;
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(first.targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    assert_eq!(detail.host_network[0].observations.len(), 1);
    let Some(api::network_observation::Value::Route(route)) =
        &detail.host_network[0].observations[0].value
    else {
        return Err("route".into());
    };
    assert!(route.gateway.is_empty());
    assert_eq!(route.interface, "en0");
    let mut policy_request = draft();
    policy_request.report.as_mut().expect("report").network = Some(api::HostNetwork {
        observed_at: 100,
        basis: "b".repeat(64),
        source: "native".into(),
        socket_coverage: "not collected".into(),
        route_coverage: "verified".into(),
        application_coverage: "verified".into(),
        packet_filter_coverage: "blocked".into(),
        firewall_enabled: Some(true),
        ..Default::default()
    });
    policy_request
        .network_parts
        .push(api::HostNetworkPart::ApplicationFirewall as i32);
    // Selecting another section of the same instant is not a contradictory
    // observation and cannot require collecting a new report just to import it.
    accept(&store, &preview(&store, policy_request)?)?;
    Ok(())
}

// Explicit removal retires source continuity too. A later reviewed adoption
// creates a new incarnation instead of following a dangling map to a removed one.
#[test]
fn removing_an_imported_resource_retires_its_source_mapping() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request.selection.truncate(1);
    let first = accept(&store, &preview(&store, request.clone())?)?;
    call(
        &store,
        &operator(),
        Command::Bulk(api::Bulk {
            targets: first.targets.clone(),
            expected: store.usage()?.revision,
            remove: true,
            changes: Vec::new(),
        }),
    )?;
    let second = accept(&store, &preview(&store, request)?)?;
    assert_ne!(first.targets, second.targets);
    Ok(())
}

// Generic accepted fields need the same source fence as address/route facts.
// Selecting only hardware must not bypass stale or mixed-boot rejection.
#[test]
fn host_mapping_rejects_older_and_conflicting_same_instant_reports() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut current = draft();
    current.selection.truncate(1);
    current.report.as_mut().expect("report").observed_at = 200;
    accept(&store, &preview(&store, current.clone())?)?;
    let before = store.usage()?;
    let mut stale = current.clone();
    stale.report.as_mut().expect("report").observed_at = 100;
    stale.report.as_mut().expect("report").findings[0].fields[0] = text_field("name", "Stale name");
    assert!(matches!(preview(&store, stale), Err(Error::Conflict)));
    let mut mixed = current;
    mixed.report.as_mut().expect("report").boot_basis = "f".repeat(64);
    assert!(matches!(preview(&store, mixed), Err(Error::Conflict)));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// An already accepted proposal is not a standing override of later local intent.
// Replaying that file after a local edit must remain a no-op, not restore its name.
#[test]
fn identical_report_replay_does_not_undo_later_local_edits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut request = draft();
    request.selection.truncate(1);
    let accepted = accept(&store, &preview(&store, request.clone())?)?;
    let target = accepted.targets[0].clone();
    call(
        &store,
        &operator(),
        Command::Write(api::Write {
            target: Some(target),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field("name", "Local accepted name")],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &operator(), Command::HostImport(request))?,
        Reply::Mutation(_)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}
