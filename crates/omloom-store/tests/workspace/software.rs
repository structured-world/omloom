use super::*;
use api::{reply::Result as Reply, request::Command};
use wire::RelationKind as R;

// Software labels are inventory facts. A generic version policy must not become
// a built-in field with parsing or precedence semantics for every product.
#[test]
fn release_labels_have_no_universal_version_scheme() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    assert!(matches!(
        write(
            &store,
            None,
            K::SoftwareRelease,
            &[
                ("name", "Runtime basis"),
                ("version", "1.2.3"),
                ("version_scheme", "VERSION_SCHEME_SEMVER"),
            ],
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A service implementation is only an execution context. Its interface remains
// usable without identifying a package/release or fabricating a remote tunnel end.
#[test]
fn network_context_does_not_require_a_software_catalog() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let os = write(
        &store,
        None,
        K::SoftwareInstallation,
        &[
            ("name", "Local OS"),
            ("installation_kind", "INSTALLATION_KIND_OPERATING_SYSTEM"),
        ],
    )?;
    let service = create(&store, &operator(), K::Service, "Network runtime")?;
    let interface = create(&store, &operator(), K::Interface, "Local tunnel interface")?;
    link(&store, &os, &service, R::Contains)?;
    link(&store, &service, &interface, R::Contains)?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    let Reply::Page(page) = call(
        &store,
        &operator(),
        Command::Query(api::Query {
            view: api::View::Network as i32,
            roots: vec![service.clone()],
            level_only: true,
            columns: vec!["name".into()],
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("page".into());
    };
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&interface));
    assert!(
        store
            .asset(AssetRef::try_from(&os)?)?
            .intended
            .iter()
            .filter_map(|v| v.field.as_ref())
            .all(|f| f.name != "version")
    );
    let Reply::Page(catalog) = call(
        &store,
        &operator(),
        Command::Query(api::Query {
            view: api::View::Services as i32,
            kinds: vec![K::SoftwareProduct as i32, K::SoftwareRelease as i32],
            columns: vec!["name".into()],
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("catalog page".into());
    };
    assert!(catalog.items.is_empty());
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn write(
    store: &Store,
    target: Option<&wire::AssetRef>,
    kind: K,
    values: &[(&str, &str)],
) -> Result<wire::AssetRef, Error> {
    let expected = target
        .map(|target| {
            store
                .asset(AssetRef::try_from(target).map_err(|_| Error::Invalid)?)
                .map(|a| a.revision)
        })
        .transpose()?;
    let Reply::Mutation(mutation) = call(
        store,
        &operator(),
        Command::Write(api::Write {
            target: target.cloned(),
            expected,
            kind: kind as i32,
            changes: values
                .iter()
                .map(|(name, value)| text_field(name, value))
                .collect(),
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    mutation.targets.into_iter().next().ok_or(Error::Corrupt)
}
fn link(store: &Store, from: &wire::AssetRef, to: &wire::AssetRef, kind: R) -> Result<(), Error> {
    call(
        store,
        &operator(),
        Command::Relation(api::Relationship {
            relation: Some(wire::Relation {
                from: Some(from.clone()),
                to: Some(to.clone()),
                kind: kind as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
    )?;
    Ok(())
}
// Vendor version labels are opaque current metadata. Reading or repeating them
// neither interprets precedence nor adds an application history or journal sync.
#[test]
fn release_labels_are_opaque_current_facts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let target = write(
        &store,
        None,
        K::SoftwareRelease,
        &[
            ("name", "Router runtime basis"),
            ("version", "Vendor train 1.2 (LTS) / build 7"),
        ],
    )?;
    store.diagnostic_database().persist()?;
    let before = store.usage()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    for _ in 0..10 {
        let asset = store.asset(AssetRef::try_from(&target)?)?;
        assert!(
            asset
                .intended
                .iter()
                .filter_map(|v| v.field.as_ref())
                .any(|f| f == &text_field("version", "Vendor train 1.2 (LTS) / build 7"))
        );
        write(
            &store,
            Some(&target),
            K::SoftwareRelease,
            &[("version", "Vendor train 1.2 (LTS) / build 7")],
        )?;
    }
    assert_eq!(store.diagnostic_database().engine().journal_syncs(), syncs);
    assert_eq!(store.usage()?, before);
    Ok(())
}

// An installation updates its release binding without becoming new hardware or
// mutating a reusable release already referenced by another installation.
#[test]
fn installations_keep_hardware_identity_and_current_release_after_restart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let hardware = create(&store, &operator(), K::Device, "Dual boot workstation")?;
    let windows = write(
        &store,
        None,
        K::SoftwareInstallation,
        &[
            ("name", "Windows"),
            ("installation_kind", "INSTALLATION_KIND_OPERATING_SYSTEM"),
        ],
    )?;
    let linux = write(
        &store,
        None,
        K::SoftwareInstallation,
        &[
            ("name", "Linux"),
            ("installation_kind", "INSTALLATION_KIND_OPERATING_SYSTEM"),
        ],
    )?;
    link(&store, &hardware, &windows, R::Contains)?;
    link(&store, &hardware, &linux, R::Contains)?;
    let release = write(
        &store,
        None,
        K::SoftwareRelease,
        &[("name", "Linux release"), ("version", "6.12")],
    )?;
    link(&store, &linux, &release, R::InstalledRelease)?;
    assert!(matches!(
        write(
            &store,
            Some(&release),
            K::SoftwareRelease,
            &[("version", "6.13")]
        ),
        Err(Error::Conflict)
    ));
    let before = store.usage()?;
    write(
        &store,
        Some(&release),
        K::SoftwareRelease,
        &[("version", "6.12")],
    )?;
    assert_eq!(store.usage()?, before);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        store.asset(AssetRef::try_from(&hardware)?)?.kind,
        K::Device as i32
    );
    assert_eq!(
        store.asset(AssetRef::try_from(&windows)?)?.kind,
        K::SoftwareInstallation as i32
    );
    assert_eq!(
        store.asset(AssetRef::try_from(&linux)?)?.kind,
        K::SoftwareInstallation as i32
    );
    Ok(())
}

// Compose names and container IDs are facts within runtime contexts, never
// cross-daemon identity. Launch details may remain unknown and cannot hold env/argv.
#[test]
fn workload_facts_are_typed_and_container_lifetimes_do_not_mutate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let workload = write(
        &store,
        None,
        K::Workload,
        &[
            ("name", "Research"),
            ("workload_kind", "WORKLOAD_KIND_COMPOSE"),
            ("compose_project", "research"),
            (
                "compose_files",
                "/project/compose.yml\n/project/overrides.yml",
            ),
        ],
    )?;
    let runtime = write(
        &store,
        None,
        K::SoftwareInstallation,
        &[
            ("name", "Docker"),
            ("installation_kind", "INSTALLATION_KIND_CONTAINER_RUNTIME"),
        ],
    )?;
    link(&store, &workload, &runtime, R::RunsOn)?;
    let container = write(
        &store,
        None,
        K::Container,
        &[
            ("name", "worker"),
            ("runtime_object_id", "native-id"),
            ("runtime_lifetime", "creation-evidence"),
        ],
    )?;
    link(&store, &workload, &container, R::Contains)?;
    link(&store, &container, &runtime, R::RunsOn)?;
    let before = store.usage()?;
    assert!(matches!(
        write(
            &store,
            Some(&container),
            K::Container,
            &[("runtime_object_id", "replacement")]
        ),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        write(
            &store,
            Some(&workload),
            K::Workload,
            &[("environment", "TOKEN=secret")]
        ),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        write(
            &store,
            Some(&workload),
            K::Workload,
            &[("repository_reference", "https://token@example.test/repo")]
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// A CSV preview validates bounded labels before creating inventory.
// Accepted raw labels and source qualification survive restart and field redaction.
#[test]
fn software_csv_review_preserves_raw_values_and_field_boundaries() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mappings: Vec<_> = ["name", "version", "raw_version", "normalization_basis"]
        .into_iter()
        .map(|name| api::Mapping {
            source: name.into(),
            field: name.into(),
        })
        .collect();
    let import = |version: &str| {
        Command::Import(api::Import {
            source: "operator-reviewed".into(),
            kind: K::SoftwareRelease as i32,
            csv: format!(
                "external_id,name,version,raw_version,normalization_basis\nr1,Reviewed release,{version},Vendor release 1.2,Publisher mapping revision 3\n"
            ),
            mappings: mappings.clone(),
            ..Default::default()
        })
    };
    let before = store.usage()?;
    let Reply::Preview(invalid) = call(&store, &operator(), import("Vendor\u{0001}label"))? else {
        return Err("preview".into());
    };
    assert!(invalid.id.is_none());
    assert!(invalid.rows[0].change.is_none());
    assert_eq!(store.usage()?, before);
    let Reply::Preview(preview) = call(&store, &operator(), import("Vendor train 1.2 (LTS)"))?
    else {
        return Err("preview".into());
    };
    assert!(preview.id.is_some());
    let Reply::Mutation(accepted) = call(
        &store,
        &operator(),
        Command::Import(api::Import {
            preview: preview.id,
            ..Default::default()
        }),
    )?
    else {
        return Err("mutation".into());
    };
    let target = accepted.targets[0].clone();
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let mut limited = operator();
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "version".into()]);
    limited.grants[0].write = Fields::Selected(vec!["name".into()]);
    let Reply::Detail(detail) = call(
        &store,
        &limited,
        Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    assert!(
        detail
            .item
            .ok_or("item")?
            .fields
            .iter()
            .all(|f| matches!(f.name.as_str(), "name" | "version"))
    );
    assert!(matches!(
        call(
            &store,
            &limited,
            Command::Write(api::Write {
                target: Some(target.clone()),
                expected: Some(1),
                kind: K::SoftwareRelease as i32,
                changes: vec![text_field("raw_version", "erase")],
                ..Default::default()
            })
        ),
        Err(Error::Denied)
    ));
    let asset = store.asset(AssetRef::try_from(&target)?)?;
    assert!(
        asset
            .intended
            .iter()
            .filter_map(|v| v.field.as_ref())
            .any(|f| f == &text_field("raw_version", "Vendor release 1.2"))
    );
    Ok(())
}

// Runtime hosting cannot become a responsibility parent, whereas the selected
// workload's owned container can inherit. Creator attribution never inherits.
#[test]
fn runtime_and_catalog_links_never_transfer_responsibility() -> TestResult {
    use api::{AssignmentMode as M, ResponsibilityAction as A, ResponsibilityFacet as F};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let runtime = create(
        &store,
        &operator(),
        K::SoftwareInstallation,
        "Shared runtime",
    )?;
    let workload = create(&store, &operator(), K::Workload, "Team workload")?;
    let container = create(&store, &operator(), K::Container, "Current container")?;
    link(&store, &workload, &runtime, R::RunsOn)?;
    link(&store, &workload, &container, R::Contains)?;
    let party = |subject: &str| api::PartyRef {
        issuer: "company.example".into(),
        subject: subject.into(),
        name: subject.into(),
        kind: api::PartyKind::Team as i32,
    };
    let assign = |target: &wire::AssetRef,
                  changes: Vec<api::ResponsibilityAssignment>|
     -> Result<(), Error> {
        store.workspace(
            &operator(),
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(Command::Responsibility(api::Responsibility {
                    target: Some(target.clone()),
                    expected: Some(
                        store
                            .asset(AssetRef::try_from(target).map_err(|_| Error::Invalid)?)?
                            .revision,
                    ),
                    action: A::Apply as i32,
                    changes,
                    ..Default::default()
                })),
            },
            100,
        )?;
        Ok(())
    };
    assign(
        &runtime,
        vec![api::ResponsibilityAssignment {
            facet: F::ClaimedOwner as i32,
            mode: M::Explicit as i32,
            party: Some(party("operators")),
            ..Default::default()
        }],
    )?;
    assign(
        &workload,
        vec![
            api::ResponsibilityAssignment {
                facet: F::ClaimedOwner as i32,
                mode: M::Explicit as i32,
                party: Some(party("developers")),
                ..Default::default()
            },
            api::ResponsibilityAssignment {
                facet: F::Creator as i32,
                mode: M::Explicit as i32,
                party: Some(party("creator")),
                ..Default::default()
            },
        ],
    )?;
    assign(
        &container,
        vec![api::ResponsibilityAssignment {
            facet: F::ClaimedOwner as i32,
            mode: M::Inherit as i32,
            ..Default::default()
        }],
    )?;
    let Reply::Responsibility(state) = call(
        &store,
        &operator(),
        Command::Responsibility(api::Responsibility {
            target: Some(container),
            action: A::Read as i32,
            ..Default::default()
        }),
    )?
    else {
        return Err("responsibility".into());
    };
    let owner = state
        .effective
        .iter()
        .find(|r| r.facet == F::ClaimedOwner as i32)
        .ok_or("owner")?;
    assert_eq!(owner.party.as_ref().ok_or("party")?.subject, "developers");
    let creator = state
        .effective
        .iter()
        .find(|r| r.facet == F::Creator as i32)
        .ok_or("creator")?;
    assert!(creator.party.is_none());
    Ok(())
}

// Removing a referenced release's product binding must not allow the same
// release identity to be repurposed under another product in a later request.
#[test]
fn referenced_release_cannot_drop_its_product_qualification() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let product = create(&store, &operator(), K::SoftwareProduct, "Product")?;
    let release = create(&store, &operator(), K::SoftwareRelease, "Release")?;
    let installation = create(&store, &operator(), K::SoftwareInstallation, "Installed")?;
    link(&store, &release, &product, R::ReleaseOf)?;
    link(&store, &installation, &release, R::InstalledRelease)?;
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(release),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    let relation = detail
        .relations
        .into_iter()
        .find(|r| r.kind == R::ReleaseOf as i32)
        .ok_or("product relation")?;
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::Relation(api::Relationship {
                relation: Some(relation),
                remove: true
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Unlinking a runtime is not permission to relabel a container lifetime as
// belonging to another daemon, or to adopt the same qualified lifetime twice.
#[test]
fn container_runtime_identity_survives_unlinking() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let runtime_a = create(&store, &operator(), K::SoftwareInstallation, "Runtime A")?;
    let runtime_b = create(&store, &operator(), K::SoftwareInstallation, "Runtime B")?;
    let values = [
        ("name", "worker"),
        ("runtime_object_id", "id"),
        ("runtime_lifetime", "created"),
    ];
    let container = write(&store, None, K::Container, &values)?;
    link(&store, &container, &runtime_a, R::RunsOn)?;
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(container.clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    let relation = detail
        .relations
        .into_iter()
        .find(|r| r.kind == R::RunsOn as i32)
        .ok_or("runtime")?;
    call(
        &store,
        &operator(),
        Command::Relation(api::Relationship {
            relation: Some(relation),
            remove: true,
        }),
    )?;
    assert!(matches!(
        link(&store, &container, &runtime_b, R::RunsOn),
        Err(Error::Conflict)
    ));
    let other_daemon = write(&store, None, K::Container, &values)?;
    link(&store, &other_daemon, &runtime_b, R::RunsOn)?;
    let duplicate = write(&store, None, K::Container, &values)?;
    assert!(matches!(
        link(&store, &duplicate, &runtime_a, R::RunsOn),
        Err(Error::Conflict)
    ));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert!(matches!(
        link(&store, &container, &runtime_b, R::RunsOn),
        Err(Error::Conflict)
    ));
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &image, policy(), 8 * 1_048_576)?;
    assert!(matches!(
        link(&restored, &container, &runtime_b, R::RunsOn),
        Err(Error::Conflict)
    ));
    // Removing the inventory object reclaims its current identity records.
    call(
        &store,
        &operator(),
        Command::Bulk(api::Bulk {
            targets: vec![container],
            expected: store.usage()?.revision,
            remove: true,
            ..Default::default()
        }),
    )?;
    link(&store, &duplicate, &runtime_a, R::RunsOn)?;
    Ok(())
}

// The host-facing full-record path must enforce the same lifetime invariant as
// the field editor, even when a replacement omits an existing identity field.
#[test]
fn full_record_replacement_cannot_erase_container_lifetime() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let target = write(
        &store,
        None,
        K::Container,
        &[
            ("name", "worker"),
            ("runtime_object_id", "id"),
            ("runtime_lifetime", "created"),
        ],
    )?;
    let mut asset = store.asset(AssetRef::try_from(&target)?)?;
    let revision = asset.revision;
    asset.intended.retain(|f| {
        f.field
            .as_ref()
            .is_none_or(|v| v.name != "runtime_lifetime")
    });
    let before = store.usage()?;
    assert!(matches!(
        store.save_asset(
            &admit(wire::record::Body::Asset(asset)),
            Some(revision),
            "fixture",
            100
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Resolving an unknown release fact is allowed; replacing an already asserted
// version is a new release, not a mutation of every installation using it.
#[test]
fn referenced_release_allows_filling_unknown_version() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let release = create(
        &store,
        &operator(),
        K::SoftwareRelease,
        "Not yet identified",
    )?;
    let installation = create(&store, &operator(), K::SoftwareInstallation, "Installed")?;
    link(&store, &installation, &release, R::InstalledRelease)?;
    write(
        &store,
        Some(&release),
        K::SoftwareRelease,
        &[("version", "Vendor build 1")],
    )?;
    assert!(matches!(
        write(
            &store,
            Some(&release),
            K::SoftwareRelease,
            &[("version", "Vendor build 2")]
        ),
        Err(Error::Conflict)
    ));
    Ok(())
}

// An upgrade replaces the current release edge, preserving both the installation
// and the old release still used elsewhere. A stale upgrade cannot overwrite it.
#[test]
fn upgrade_rebinds_one_installation_without_rewriting_other_consumers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let a = create(
        &store,
        &operator(),
        K::SoftwareInstallation,
        "Installation A",
    )?;
    let b = create(
        &store,
        &operator(),
        K::SoftwareInstallation,
        "Installation B",
    )?;
    let old = create(&store, &operator(), K::SoftwareRelease, "Original release")?;
    let new = create(&store, &operator(), K::SoftwareRelease, "Updated release")?;
    link(&store, &a, &old, R::InstalledRelease)?;
    link(&store, &b, &old, R::InstalledRelease)?;
    let read = |target: &wire::AssetRef| -> Result<wire::Relation, Error> {
        let Reply::Detail(detail) = call(
            &store,
            &operator(),
            Command::Read(api::Read {
                target: Some(target.clone()),
                ..Default::default()
            }),
        )?
        else {
            return Err(Error::Corrupt);
        };
        detail
            .relations
            .into_iter()
            .find(|r| r.kind == R::InstalledRelease as i32)
            .ok_or(Error::Corrupt)
    };
    let mut change = read(&a)?;
    change.to = Some(new.clone());
    call(
        &store,
        &operator(),
        Command::Relation(api::Relationship {
            relation: Some(change.clone()),
            ..Default::default()
        }),
    )?;
    assert_eq!(read(&a)?.to, Some(new));
    assert_eq!(read(&b)?.to, Some(old));
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::Relation(api::Relationship {
                relation: Some(change),
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    Ok(())
}

// Concurrent admission of one fully qualified runtime lifetime has one winner.
// The database transaction protects the claim, independently of process locks.
#[test]
fn concurrent_container_identity_admission_has_one_winner() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let runtime = create(&store, &operator(), K::SoftwareInstallation, "Runtime")?;
    let values = [
        ("name", "worker"),
        ("runtime_object_id", "id"),
        ("runtime_lifetime", "created"),
    ];
    let first = write(&store, None, K::Container, &values)?;
    let second = write(&store, None, K::Container, &values)?;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            link(&store, &first, &runtime, R::RunsOn)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            link(&store, &second, &runtime, R::RunsOn)
        });
        (a.join(), b.join())
    });
    let results = [
        results.0.map_err(|_| "thread failed")?,
        results.1.map_err(|_| "thread failed")?,
    ];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter(|r| r.is_err())
            .all(|r| matches!(r, Err(Error::Conflict)))
    );
    Ok(())
}

// An explicit clear retires a version claim; it is not an immutable version
// assertion that prevents subsequently identifying an unknown release.
#[test]
fn cleared_unknown_release_version_can_be_identified() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let release = create(&store, &operator(), K::SoftwareRelease, "Unknown release")?;
    call(
        &store,
        &operator(),
        Command::Write(api::Write {
            target: Some(release.clone()),
            expected: Some(1),
            kind: K::SoftwareRelease as i32,
            changes: vec![wire::Field {
                name: "version".into(),
                value: Some(wire::field::Value::Clear(wire::NullValue::NullValue as i32)),
            }],
            ..Default::default()
        }),
    )?;
    let installation = create(&store, &operator(), K::SoftwareInstallation, "Installed")?;
    link(&store, &installation, &release, R::InstalledRelease)?;
    write(
        &store,
        Some(&release),
        K::SoftwareRelease,
        &[("version", "Vendor build 1")],
    )?;
    Ok(())
}

// Ordered launch references have a fixed eight-path ceiling and reject duplicate
// paths without sorting away their effective merge order.
#[test]
fn compose_reference_validation_preserves_order_and_bounds_work() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let eight = (0..8)
        .map(|n| format!("/project/{n}.yml"))
        .collect::<Vec<_>>()
        .join("\n");
    let target = write(
        &store,
        None,
        K::Workload,
        &[("name", "Bounded recipe"), ("compose_files", &eight)],
    )?;
    let before = store.usage()?;
    assert!(matches!(
        write(
            &store,
            Some(&target),
            K::Workload,
            &[("compose_files", &format!("{eight}\n/project/extra.yml"))]
        ),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        write(
            &store,
            Some(&target),
            K::Workload,
            &[("compose_files", "/project/base.yml\n/project/base.yml")]
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    let asset = store.asset(AssetRef::try_from(&target)?)?;
    assert!(
        asset
            .intended
            .iter()
            .filter_map(|v| v.field.as_ref())
            .any(|v| v == &text_field("compose_files", &eight))
    );
    Ok(())
}

// The page resolves only its visible workload/container owners through the
// common mechanism. Hidden owner fields are not confused with unassigned ones.
#[test]
fn workload_page_ownership_is_bounded_redacted_and_read_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let workload = create(&store, &operator(), K::Workload, "Alpha deployment")?;
    create(&store, &operator(), K::Workload, "Other deployment")?;
    store.workspace(
        &operator(),
        api::Request {
            working: Some(api::WorkingContext {
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
            command: Some(Command::Responsibility(api::Responsibility {
                target: Some(workload.clone()),
                expected: Some(1),
                action: api::ResponsibilityAction::Apply as i32,
                changes: vec![api::ResponsibilityAssignment {
                    facet: api::ResponsibilityFacet::ClaimedOwner as i32,
                    mode: api::AssignmentMode::Explicit as i32,
                    party: Some(api::PartyRef {
                        issuer: "company.example".into(),
                        subject: "research".into(),
                        name: "Research team".into(),
                        kind: api::PartyKind::Team as i32,
                    }),
                    contact: Some("private contact".into()),
                    ..Default::default()
                }],
                ..Default::default()
            })),
        },
        100,
    )?;
    let Reply::Mutation(children) = call(
        &store,
        &operator(),
        Command::Write(api::Write {
            kind: K::Container as i32,
            parent: Some(workload.clone()),
            changes: vec![text_field("name", "Owned instance")],
            ..Default::default()
        }),
    )?
    else {
        return Err("child".into());
    };
    // Complete fixture workers before measuring reads of the durable inventory.
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let request = api::Query {
        kinds: vec![K::Workload as i32],
        include_context: true,
        limit: 1,
        columns: vec!["name".into()],
        ..Default::default()
    };
    let before = store.usage()?;
    // Completed syncs can belong to setup, and the snapshot clock advances on
    // reads. The journal cursor observes actual new durable mutation entries.
    let engine = store.diagnostic_database().engine();
    let journal = engine.oplog_read_since(0)?.ok_or("journal required")?;
    let next_entry = journal
        .last()
        .ok_or("fixture journal empty")?
        .index
        .checked_add(1)
        .ok_or("journal index overflow")?;
    let Reply::Page(page) = call(&store, &operator(), Command::Query(request.clone()))? else {
        return Err("page".into());
    };
    assert_eq!(page.total, 2);
    assert_eq!(page.contexts.len(), 1);
    let owner = page.contexts[0]
        .responsibility
        .as_ref()
        .ok_or("page ownership missing")?;
    assert_eq!(owner.party.as_ref().ok_or("party")?.subject, "research");
    assert!(owner.contact.is_none());
    assert!(owner.source.is_none());
    let Reply::Page(instances) = call(
        &store,
        &operator(),
        Command::Query(api::Query {
            kinds: vec![K::Container as i32],
            include_context: true,
            limit: 1,
            columns: vec!["name".into()],
            ..Default::default()
        }),
    )?
    else {
        return Err("instances".into());
    };
    assert_eq!(instances.items[0].target.as_ref(), children.targets.first());
    let inherited = instances.contexts[0]
        .responsibility
        .as_ref()
        .ok_or("inherited ownership")?;
    assert_eq!(
        inherited.status,
        api::ResponsibilityStatus::Inherited as i32
    );
    assert_eq!(inherited.party.as_ref().ok_or("party")?.subject, "research");
    assert!(inherited.contact.is_none());
    assert!(inherited.source.is_none());
    let mut limited = operator();
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    let Reply::Page(hidden) = call(&store, &limited, Command::Query(request))? else {
        return Err("page".into());
    };
    assert!(hidden.contexts[0].responsibility.is_none());
    assert_eq!(store.usage()?, before);
    assert!(
        engine
            .oplog_read_since(next_entry)?
            .ok_or("journal required")?
            .is_empty(),
        "read-only queries appended journal entries"
    );
    let plans = store
        .diagnostic_database()
        .query_registry()
        .top_by_impact(64);
    assert!(
        plans
            .iter()
            .any(|q| q.canonical_query.contains("n.facet = $facet")
                && !q.canonical_query.contains("contact: n.contact")
                && q.last_plan
                    .as_ref()
                    .is_some_and(|p| p.contains("omloom_parent(parent)"))),
        "owner page plans: {:?}",
        plans
            .iter()
            .filter(|q| q.canonical_query.contains("facet"))
            .map(|q| (&q.canonical_query, &q.last_plan))
            .collect::<Vec<_>>()
    );
    // A real mutation must be visible to the same observer, so an empty or
    // disconnected diagnostic source cannot make the read-only assertion pass.
    create(
        &store,
        &operator(),
        K::Workload,
        "Write observation control",
    )?;
    assert!(
        !engine
            .oplog_read_since(next_entry)?
            .ok_or("journal required")?
            .is_empty()
    );
    Ok(())
}
