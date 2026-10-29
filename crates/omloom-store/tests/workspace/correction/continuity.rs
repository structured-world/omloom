use super::*;

// Another actor's personal CSV approval cannot become approval for a new target.
#[test]
fn correction_preserves_personal_csv_target_and_undo_binding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut importer = operator();
    importer.actor = "personal-importer".into();
    let input = api::Import {
        source: "Personal source".into(),
        kind: K::Device as i32,
        csv: "external_id,name\nsource-id,Duplicate\n".into(),
        mappings: vec![api::Mapping {
            source: "name".into(),
            field: "name".into(),
        }],
        ..Default::default()
    };
    let Reply::Preview(preview) = call(&store, &importer, Command::Import(input.clone()))? else {
        return Err("preview".into());
    };
    let Reply::Mutation(created) = call(
        &store,
        &importer,
        Command::Import(api::Import {
            preview: preview.id,
            ..Default::default()
        }),
    )?
    else {
        return Err("import".into());
    };
    let duplicate = &created.targets[0];
    let retained = create(&store, &operator(), K::Device, "Retained")?;
    let preview = prepare(
        &store,
        vec![mapping(duplicate, &retained)],
        api::IdentityDecision::DistinctInstallations,
    )?;
    let applied = apply(&store, &preview.id)?;
    let before = store.usage()?;
    let Reply::Preview(retry) = call(&store, &importer, Command::Import(input.clone()))? else {
        return Err("retry".into());
    };
    assert!(
        retry.id.is_none(),
        "personal approval must not silently retarget: {retry:?}"
    );
    assert!(
        retry
            .rows
            .iter()
            .all(|row| row.change.is_none() && !row.diagnostic.is_empty())
    );
    assert_eq!(
        store.usage()?,
        before,
        "a retired binding cannot create a new inventory identity"
    );
    call(
        &store,
        &operator(),
        Command::CorrectIdentity(api::IdentityCorrection {
            undo: Some(applied.id),
            ..Default::default()
        }),
    )?;
    let Reply::Preview(restored) = call(&store, &importer, Command::Import(input))? else {
        return Err("restored".into());
    };
    assert_eq!(
        restored.rows[0]
            .change
            .as_ref()
            .and_then(|change| change.target.as_ref()),
        Some(duplicate)
    );
    Ok(())
}

// Unrelated matching claims cannot consume another working scope's allowance.
#[test]
fn candidate_limit_counts_only_current_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let claim = api::HostIdentityClaim {
        kind: api::HostIdentityKind::HardwareUuid as i32,
        namespace: "apple/platform-uuid".into(),
        digest: "c".repeat(64),
        quality: api::HostIdentityQuality::Reported as i32,
    };
    let mut targets = Vec::new();
    for batch in 0..2 {
        let mut input = super::super::host_import::draft();
        let report = input.report.as_mut().ok_or("report")?;
        report.source_basis = format!("{:064x}", batch + 10);
        report.findings = (0..65)
            .map(|index| api::HostFinding {
                key: format!("hardware/{index}"),
                kind: K::Device as i32,
                fields: vec![text_field("name", &format!("Candidate {batch}/{index}"))],
                identity: vec![claim.clone()],
                ..Default::default()
            })
            .collect();
        input.selection = report
            .findings
            .iter()
            .map(|finding| api::HostSelection {
                key: finding.key.clone(),
                ..Default::default()
            })
            .collect();
        targets.extend(
            super::super::host_import::accept(
                &store,
                &super::super::host_import::preview(&store, input)?,
            )?
            .targets,
        );
    }
    assert_eq!(targets.len(), 130);
    let before = store.usage()?;
    let started = std::time::Instant::now();
    let result = store.workspace(
        &operator(),
        api::Request {
            working: Some(api::WorkingContext {
                roots: vec![targets[0].clone()],
                ..Default::default()
            }),
            command: Some(Command::FindIdentityCandidates(
                api::FindIdentityCandidates {
                    kind: K::Device as i32,
                    claims: vec![claim],
                    ..Default::default()
                },
            )),
        },
        100,
    );
    eprintln!(
        "130 matching claims, one scoped candidate: {:?}; inventory usage: {:?}",
        started.elapsed(),
        before
    );
    let Some(Reply::IdentityCandidates(value)) = result?.result else {
        return Err("candidates".into());
    };
    assert_eq!(value.items.len(), 1);
    assert_eq!(value.items[0].target.as_ref(), Some(&targets[0]));
    assert_eq!(store.usage()?, before, "candidate reads must not write");
    // No readable claims require neither candidate expansion nor persisted state.
    let mut limited = operator();
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "relations".into()]);
    let Some(Reply::IdentityCandidates(empty)) = store
        .workspace(
            &limited,
            api::Request {
                working: Some(api::WorkingContext {
                    roots: vec![targets[0].clone()],
                    ..Default::default()
                }),
                command: Some(Command::FindIdentityCandidates(
                    api::FindIdentityCandidates {
                        target: Some(targets[0].clone()),
                        ..Default::default()
                    },
                )),
            },
            100,
        )?
        .result
    else {
        return Err("empty candidates".into());
    };
    assert!(empty.items.is_empty());
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Reboot/address refresh follows only an existing source binding. Reinstall,
// cloned UUIDs/machine IDs and a movable NIC remain separate until reviewed.
#[test]
fn identity_evidence_never_collapses_reinstall_clone_or_movable_interface() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut input = super::super::host_import::draft();
    let report = input.report.as_mut().ok_or("report")?;
    for (index, kind, namespace, quality) in [
        (
            0,
            api::HostIdentityKind::HardwareUuid,
            "smbios/system-uuid",
            api::HostIdentityQuality::Reported,
        ),
        (
            1,
            api::HostIdentityKind::InstallationId,
            "linux/machine-id",
            api::HostIdentityQuality::Reported,
        ),
        (
            2,
            api::HostIdentityKind::FactoryMac,
            "ethernet/permanent",
            api::HostIdentityQuality::Movable,
        ),
    ] {
        report.findings[index]
            .identity
            .push(api::HostIdentityClaim {
                kind: kind as i32,
                namespace: namespace.into(),
                digest: "c".repeat(64),
                quality: quality as i32,
            });
    }
    let original = super::super::host_import::accept(
        &store,
        &super::super::host_import::preview(&store, input.clone())?,
    )?
    .targets;
    let mut reboot = input.clone();
    let report = reboot.report.as_mut().ok_or("report")?;
    report.observed_at = 101;
    report.boot_basis = "e".repeat(64);
    report.findings[2].addresses[0].address = "192.0.2.99".into();
    let refresh = super::super::host_import::accept(
        &store,
        &super::super::host_import::preview(&store, reboot)?,
    )?;
    assert_eq!(refresh.targets, original);
    assert_eq!(
        read(&store, &original[2])?.current_addresses[0].observations[0]
            .value
            .as_ref()
            .ok_or("value")?
            .address,
        "192.0.2.99"
    );
    // A new source can discover candidates before adopting anything.
    let claims = input.report.as_ref().ok_or("report")?.findings[0]
        .identity
        .clone();
    let before = store.usage()?;
    let plan=store.diagnostic_database().explain_cypher("UNWIND $claims AS claim MATCH (n:OmRow {claim:claim}) MATCH (a:OmRow {key:'asset/'+n.asset}) WHERE a.kind=$kind RETURN DISTINCT properties(a) AS p LIMIT 129")?;
    assert!(plan.contains("omloom_claim"), "{plan}");
    let started = std::time::Instant::now();
    let Reply::IdentityCandidates(candidates) = call(
        &store,
        &operator(),
        Command::FindIdentityCandidates(api::FindIdentityCandidates {
            claims: claims.clone(),
            kind: K::Device as i32,
            ..Default::default()
        }),
    )?
    else {
        return Err("candidates".into());
    };
    assert_eq!(candidates.items.len(), 1);
    eprintln!(
        "Scoped native candidate lookup: {:?}; plan: {plan}",
        started.elapsed()
    );
    assert_eq!(candidates.items[0].target.as_ref(), Some(&original[0]));
    assert_eq!(store.usage()?, before);
    let mut wrong = api::FindIdentityCandidates {
        claims,
        kind: K::Interface as i32,
        ..Default::default()
    };
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::FindIdentityCandidates(wrong.clone())
        ),
        Err(Error::Invalid)
    ));
    wrong.kind = K::Device as i32;
    wrong.target = Some(original[0].clone());
    assert!(matches!(
        call(&store, &operator(), Command::FindIdentityCandidates(wrong)),
        Err(Error::Invalid)
    ));
    let mut reinstall = input.clone();
    let report = reinstall.report.as_mut().ok_or("report")?;
    report.source_basis = "d".repeat(64);
    report.findings[1].identity[0].digest = "d".repeat(64);
    let second = super::super::host_import::accept(
        &store,
        &super::super::host_import::preview(&store, reinstall)?,
    )?
    .targets;
    let mut cloned = input;
    cloned.report.as_mut().ok_or("report")?.source_basis = "f".repeat(64);
    let third = super::super::host_import::accept(
        &store,
        &super::super::host_import::preview(&store, cloned)?,
    )?
    .targets;
    assert_ne!(second[0], original[0]);
    assert_ne!(third[0], original[0]);
    assert_ne!(third[0], second[0]);
    for subject in [&original[0], &second[0], &third[0]] {
        assert!(read(&store, subject).is_ok());
    }
    let Reply::IdentityCandidates(candidates) = call(
        &store,
        &operator(),
        Command::FindIdentityCandidates(api::FindIdentityCandidates {
            target: Some(original[2].clone()),
            ..Default::default()
        }),
    )?
    else {
        return Err("NIC candidates".into());
    };
    assert_eq!(
        candidates.items.len(),
        2,
        "movable evidence supports review without merging either NIC"
    );
    Ok(())
}
