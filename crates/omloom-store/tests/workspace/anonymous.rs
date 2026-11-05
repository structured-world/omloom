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
    for query in [
        "MATCH (a:OmRow {parent:'assets'}) RETURN {asset:a.asset,incarnation:a.incarnation,revision:a.revision} AS p LIMIT 1",
        "MATCH (a:OmRow {parent:'assets'}) WITH a,min(2) AS priority RETURN {asset:a.asset,incarnation:a.incarnation,revision:a.revision,priority:priority} AS p LIMIT 1",
        "MATCH (a:OmRow {parent:'assets'}) WITH a.asset AS asset,a.incarnation AS incarnation,a.revision AS revision,min(2) AS priority RETURN {asset:asset,incarnation:incarnation,revision:revision,priority:priority} AS p LIMIT 1",
    ] {
        eprintln!(
            "candidate projection diagnostic: {query}\n{:?}",
            store
                .diagnostic_database()
                .execute_cypher_shared(query, None)?
        );
    }
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
        candidates(&store, &limited, finding, None)?
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
    Ok(())
}
