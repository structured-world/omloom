use super::*;

fn refreshed(store: &Store, basis: char) -> Result<Vec<wire::AssetRef>, Error> {
    let mut input = super::super::host_import::draft();
    input.report.as_mut().unwrap().source_basis = basis.to_string().repeat(64);
    let accepted = super::super::host_import::accept(
        store,
        &super::super::host_import::preview(store, input.clone())?,
    )?;
    input.report.as_mut().unwrap().observed_at = 200;
    for node in &mut input.report.as_mut().unwrap().findings {
        node.fields
            .push(text_field("description", &format!("Source {basis}")));
    }
    let Reply::HostSourceOptions(options) = call(
        store,
        &operator(),
        Command::HostSources(api::HostSources {
            host: Some(accepted.targets[0].clone()),
            report: input.report.clone(),
        }),
    )?
    else {
        return Err(Error::Corrupt);
    };
    input.source.clear();
    input.refresh = Some(api::HostRefresh {
        host: Some(accepted.targets[0].clone()),
        source_ref: options.sources[0].reference.clone(),
    });
    super::super::host_import::accept(
        store,
        &super::super::host_import::preview(store, input.clone())?,
    )?;
    input.report.as_mut().unwrap().observed_at = 300;
    input.report.as_mut().unwrap().findings[2].fields =
        vec![text_field("name", &format!("Observed {basis}"))];
    input.selection.retain(|m| m.key == "interface/en0");
    super::super::host_import::accept(store, &super::super::host_import::preview(store, input)?)?;
    let source = omloom_contracts::ids::SourceId::generate().to_string();
    for target in &accepted.targets {
        let mut asset = store.asset(AssetRef::try_from(target)?)?;
        let revision = asset.revision;
        asset.imported.push(observation(
            &source,
            250,
            vec![text_field("description", &format!("Imported {basis}"))],
        ));
        store.save_asset(
            &admit(wire::record::Body::Asset(asset)),
            Some(revision),
            "fixture-operator",
            300,
        )?;
    }
    Ok(accepted.targets)
}

fn ordered(mut values: Vec<wire::Observation>) -> Vec<wire::Observation> {
    for value in &mut values {
        value
            .evidence
            .as_mut()
            .unwrap()
            .fields
            .sort_by(|a, b| a.name.cmp(&b.name));
    }
    values.sort_by(|a, b| {
        let a = a.evidence.as_ref().unwrap();
        let b = b.evidence.as_ref().unwrap();
        (&a.source_id, a.observed_at_ms).cmp(&(&b.source_id, b.observed_at_ms))
    });
    values
}

// Correcting refreshed duplicates must preserve both source identities and
// partial freshness windows in both evidence layers. Reversal rechecks every
// observed field's rights.
#[test]
fn refreshed_duplicate_correction_preserves_evidence_and_undo() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let retained = refreshed(&store, 'a')?;
    let duplicate = refreshed(&store, 'd')?;
    let before_retained = retained
        .iter()
        .map(|t| read(&store, t).map(|d| (d.observed, d.imported)))
        .collect::<Result<Vec<_>, _>>()?;
    let before_duplicate = duplicate
        .iter()
        .map(|t| read(&store, t).map(|d| (d.observed, d.imported)))
        .collect::<Result<Vec<_>, _>>()?;
    let mappings = duplicate
        .iter()
        .zip(&retained)
        .map(|(a, b)| mapping(a, b))
        .collect::<Vec<_>>();
    let evidence_rows = store.usage()?.rows[Class::Evidence as usize];
    let preview = prepare(
        &store,
        mappings.clone(),
        api::IdentityDecision::DuplicateInstallation,
    )?;
    let applied = apply(&store, &preview.id)?;
    assert_eq!(store.usage()?.rows[Class::Evidence as usize], evidence_rows);
    for index in 0..3 {
        let detail = read(&store, &retained[index])?;
        let mut expected = before_retained[index].0.clone();
        expected.extend(before_duplicate[index].0.clone());
        assert_eq!(ordered(detail.observed), ordered(expected));
        let mut expected = before_retained[index].1.clone();
        expected.extend(before_duplicate[index].1.clone());
        assert_eq!(ordered(detail.imported), ordered(expected));
        assert!(matches!(
            read(&store, &duplicate[index]),
            Err(Error::Missing)
        ));
    }
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    for index in 0..3 {
        let mut expected = before_retained[index].1.clone();
        expected.extend(before_duplicate[index].1.clone());
        assert_eq!(
            ordered(read(&store, &retained[index])?.imported),
            ordered(expected)
        );
    }
    let mut restricted = operator();
    restricted.grants[0].write = Fields::Selected(
        vec![
            "name",
            "identity_correction",
            "relations",
            "remove",
            "identity_evidence",
            "current_addresses",
            "claimed_owner",
            "operator",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    );
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &restricted,
            Command::CorrectIdentity(api::IdentityCorrection {
                undo: Some(applied.id.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    // Read and write revocation are independent: write authority cannot reveal
    // the old description held by the correction's inverse.
    let mut unreadable = operator();
    unreadable.grants[0].read = restricted.grants[0].write.clone();
    assert!(matches!(
        call(
            &store,
            &unreadable,
            Command::CorrectIdentity(api::IdentityCorrection {
                undo: Some(applied.id.clone()),
                ..Default::default()
            })
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    call(
        &store,
        &operator(),
        Command::CorrectIdentity(api::IdentityCorrection {
            undo: Some(applied.id),
            ..Default::default()
        }),
    )?;
    for index in 0..3 {
        let retained_detail = read(&store, &retained[index])?;
        let duplicate_detail = read(&store, &duplicate[index])?;
        assert_eq!(
            ordered(retained_detail.observed),
            ordered(before_retained[index].0.clone())
        );
        assert_eq!(
            ordered(duplicate_detail.observed),
            ordered(before_duplicate[index].0.clone())
        );
        assert_eq!(
            ordered(retained_detail.imported),
            ordered(before_retained[index].1.clone())
        );
        assert_eq!(
            ordered(duplicate_detail.imported),
            ordered(before_duplicate[index].1.clone())
        );
    }
    let preview = prepare(
        &store,
        mappings,
        api::IdentityDecision::DuplicateInstallation,
    )?;
    apply(&store, &preview.id)?;
    let mut input = super::super::host_import::draft();
    input.source.clear();
    input.report.as_mut().unwrap().source_basis = "d".repeat(64);
    input.report.as_mut().unwrap().observed_at = 400;
    let Reply::HostSourceOptions(options) = call(
        &store,
        &operator(),
        Command::HostSources(api::HostSources {
            host: Some(retained[0].clone()),
            report: input.report.clone(),
        }),
    )?
    else {
        return Err(Error::Corrupt.into());
    };
    assert_eq!(options.sources.len(), 1);
    input.selection = options.sources[0].matches.clone();
    input.refresh = Some(api::HostRefresh {
        host: Some(retained[0].clone()),
        source_ref: options.sources[0].reference.clone(),
    });
    super::super::host_import::accept(&store, &super::super::host_import::preview(&store, input)?)?;
    for target in &retained {
        let values = read(&store, target)?.observed;
        let ids = values
            .iter()
            .map(|o| &o.evidence.as_ref().unwrap().source_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            ids.len(),
            2,
            "correction and refresh must not fork evidence sources"
        );
        assert!(
            values
                .iter()
                .any(|value| value.evidence.as_ref().unwrap().observed_at_ms == 400)
        );
    }
    Ok(())
}
