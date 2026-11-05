use super::*;
use api::{reply::Result as Reply, request::Command};
use std::collections::BTreeSet;

// Anonymous packages reuse only explicitly confirmed target custody. New boots
// cannot silently inherit a prior package's host/OS/interface associations.
#[test]
fn anonymous_packages_require_fresh_matching_and_keep_current_custody() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut input = host_import::draft();
    input.report.as_mut().ok_or("report")?.source_basis.clear();
    let review = host_import::preview(&store, input.clone())?;
    let accepted = host_import::accept(&store, &review)?;
    let before = store.usage()?;
    assert_eq!(
        host_import::accept(&store, &review)?.targets,
        accepted.targets
    );
    assert_eq!(store.usage()?, before, "lost-reply replay writes nothing");
    input.report.as_mut().ok_or("report")?.observed_at = 200;
    for (selection, target) in input.selection.iter_mut().zip(&accepted.targets) {
        selection.target = Some(target.clone());
    }
    input.report.as_mut().ok_or("report")?.findings[1].fields =
        vec![text_field("name", "Observed OS label")];
    host_import::accept(&store, &host_import::preview(&store, input.clone())?)?;
    let Reply::Detail(detail) = call(
        &store,
        &operator(),
        Command::Read(api::Read {
            target: Some(accepted.targets[1].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("detail".into());
    };
    assert_eq!(
        detail.item.as_ref().ok_or("item")?.fields,
        vec![text_field("name", "Native OS")]
    );
    assert_eq!(
        detail.observed[0]
            .evidence
            .as_ref()
            .ok_or("evidence")?
            .fields,
        vec![text_field("name", "Observed OS label")]
    );
    let rows = store.usage()?.rows;
    input.report.as_mut().ok_or("report")?.observed_at = 300;
    host_import::accept(&store, &host_import::preview(&store, input.clone())?)?;
    assert_eq!(store.usage()?.rows[0], rows[0]);
    assert_eq!(
        store.usage()?.rows[1],
        rows[1],
        "fresh equal packages do not accumulate evidence history"
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &operator(), Command::HostImport(input.clone()))?,
        Reply::Mutation(_)
    ));
    assert_eq!(store.usage()?, before);
    input.selection.iter_mut().for_each(|v| v.target = None);
    let other = host_import::accept(&store, &host_import::preview(&store, input)?)?;
    assert_ne!(
        other.targets, accepted.targets,
        "a separately confirmed adoption cannot silently reuse anonymous identities"
    );
    Ok(())
}

fn candidates(
    store: &Store,
    actor: &Context,
    finding: api::HostFinding,
    cursor: Option<String>,
) -> Result<api::IdentityCandidates, Error> {
    let Reply::IdentityCandidates(value) = call(
        store,
        actor,
        Command::FindIdentityCandidates(api::FindIdentityCandidates {
            finding: Some(finding),
            cursor,
            ..Default::default()
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    Ok(value)
}

// Same-shape fleets remain tied and pageable. Contradictory serials do not
// disappear behind a generic shape match, and hidden evidence never admits a hit.
#[test]
fn descriptive_candidates_are_explained_paged_scoped_and_read_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut targets = Vec::new();
    for index in 0..30 {
        let Reply::Mutation(value) = call(
            &store,
            &operator(),
            Command::Write(api::Write {
                kind: K::Device as i32,
                changes: vec![
                    text_field("name", &format!("Fleet {index}")),
                    text_field("model", "Same chassis"),
                    text_field("serial", &format!("serial-{index}")),
                ],
                ..Default::default()
            }),
        )?
        else {
            return Err("create".into());
        };
        targets.push(value.targets[0].clone());
    }
    let finding = api::HostFinding {
        key: "hardware".into(),
        kind: K::Device as i32,
        fields: vec![
            text_field("model", "Same chassis"),
            text_field("serial", "new serial"),
        ],
        ..Default::default()
    };
    let before = store.usage()?;
    let started = std::time::Instant::now();
    #[cfg(feature = "diagnostics")]
    let mut store = store;
    #[cfg(feature = "diagnostics")]
    store.set_diagnostic_query_check(|database, query| {
        if query.starts_with("UNWIND $probes") {
            let plan = database.explain_cypher(query).expect("candidate plan");
            assert!(plan.contains("omloom_claim"), "{plan}");
            assert!(plan.contains("omloom_key"), "{plan}");
            assert!(!plan.contains("Traverse"), "{plan}");
        }
    });
    let first = candidates(&store, &operator(), finding.clone(), None)?;
    assert_eq!(first.items.len(), 25);
    assert_eq!(first.explanations.len(), 25);
    assert!(
        first
            .explanations
            .iter()
            .all(|v| v.matches.iter().any(|v| v.starts_with("model:"))
                && v.conflicts.iter().any(|v| v.starts_with("serial:"))
                && !v.unknowns.is_empty())
    );
    let next = first.next.clone();
    let second = candidates(&store, &operator(), finding.clone(), first.next)?;
    assert_eq!(second.items.len(), 5);
    assert!(second.next.is_none());
    let actual = first
        .items
        .iter()
        .chain(&second.items)
        .map(|v| {
            v.target
                .as_ref()
                .ok_or("target")
                .map(|t| t.asset_id.clone())
        })
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    assert_eq!(actual.len(), targets.len());
    let mut limited = operator();
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&targets[0])?]);
    let visible = candidates(&store, &limited, finding.clone(), None)?;
    assert_eq!(visible.items.len(), 1);
    assert_eq!(visible.items[0].target.as_ref(), Some(&targets[0]));
    limited.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(
        candidates(&store, &limited, finding.clone(), None)?
            .items
            .is_empty()
    );
    eprintln!(
        "descriptive fleet: coverage=30/30, automatic bindings=0, pages=2, elapsed={:?}",
        started.elapsed()
    );
    assert_eq!(
        store.usage()?,
        before,
        "candidate inspection writes nothing"
    );
    let mut changed_authority = operator();
    changed_authority.authority_revision += 1;
    assert!(matches!(
        candidates(&store, &changed_authority, finding.clone(), next.clone()),
        Err(Error::Expired)
    ));
    create(&store, &operator(), K::Device, "An unrelated edit")?;
    assert!(matches!(
        candidates(&store, &operator(), finding, next),
        Err(Error::Expired)
    ));
    Ok(())
}

fn claim(kind: api::HostIdentityKind, digest: char) -> api::HostIdentityClaim {
    api::HostIdentityClaim {
        kind: kind as i32,
        namespace: "fixture/identifiers".into(),
        digest: digest.to_string().repeat(64),
        quality: api::HostIdentityQuality::Movable as i32,
    }
}

// Clones and moved components can repeat real identifiers. Layer-specific
// evidence must return both candidates without choosing or merging either.
#[test]
fn clones_reinstall_and_moved_interfaces_remain_explained_candidates() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut targets = Vec::new();
    let mut report = None;
    for basis in ['c', 'd'] {
        let mut input = host_import::draft();
        let value = input.report.as_mut().ok_or("report")?;
        value.source_basis = basis.to_string().repeat(64);
        value.findings[0].identity = vec![claim(api::HostIdentityKind::HardwareUuid, '1')];
        value.findings[1].identity = vec![claim(api::HostIdentityKind::InstallationId, '2')];
        value.findings[2].identity = vec![claim(api::HostIdentityKind::FactoryMac, '3')];
        report = Some(value.clone());
        targets.push(host_import::accept(&store, &host_import::preview(&store, input)?)?.targets);
    }
    let report = report.ok_or("report")?;
    let before = store.usage()?;
    for (index, finding) in report.findings.iter().enumerate() {
        let result = candidates(&store, &operator(), finding.clone(), None)?;
        assert_eq!(result.items.len(), 2);
        let actual = result
            .items
            .iter()
            .filter_map(|v| v.target.clone())
            .collect::<Vec<_>>();
        assert!(actual.contains(&targets[0][index]));
        assert!(actual.contains(&targets[1][index]));
        assert!(
            result
                .explanations
                .iter()
                .all(|v| v.matches.iter().any(|v| v.contains("cloning or movement")))
        );
    }
    let Some(Reply::IdentityCandidates(rooted)) = store
        .workspace(
            &operator(),
            api::Request {
                command: Some(Command::FindIdentityCandidates(
                    api::FindIdentityCandidates {
                        finding: Some(report.findings[1].clone()),
                        ..Default::default()
                    },
                )),
                working: Some(api::WorkingContext {
                    roots: vec![targets[0][0].clone()],
                    ..Default::default()
                }),
            },
            100,
        )?
        .result
    else {
        return Err("rooted candidates".into());
    };
    assert_eq!(rooted.items.len(), 1);
    assert_eq!(rooted.items[0].target.as_ref(), Some(&targets[0][1]));
    let mut reinstall = report.findings[1].clone();
    reinstall.identity[0].digest = "4".repeat(64);
    let result = candidates(&store, &operator(), reinstall, None)?;
    assert_eq!(
        result.items.len(),
        2,
        "descriptive search still works when the installation ID changes"
    );
    assert!(result.explanations.iter().all(|v| {
        v.conflicts
            .iter()
            .any(|v| v.contains("different reported identifier"))
    }));
    let mut forged_host = report.findings[2].clone();
    forged_host.kind = K::Device as i32;
    assert!(matches!(
        candidates(&store, &operator(), forged_host, None),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}
