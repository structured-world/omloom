use super::*;
use api::{reply::Result as Reply, request::Command};
use wire::{equipment_claim::Assertion as A, field::Value as V};

fn possibilities() -> wire::Field {
    wire::Field {
        name: "model".into(),
        value: Some(V::Equipment(wire::EquipmentClaim {
            basis: "Front label; rear not observed".into(),
            assertion: Some(A::Alternatives(wire::EquipmentAlternatives {
                candidates: vec![
                    wire::EquipmentAlternative {
                        value: "DL160".into(),
                        probability_ppm: Some(600_000),
                        ..Default::default()
                    },
                    wire::EquipmentAlternative {
                        value: "DL120".into(),
                        probability_ppm: Some(200_000),
                        ..Default::default()
                    },
                ],
                domain: "HPE rack models".into(),
                calibration_basis: Some("operator supplied corpus/v1".into()),
                residual_ppm: Some(200_000),
                ..Default::default()
            })),
        })),
    }
}
fn write(
    store: &Store,
    actor: &Context,
    target: &wire::AssetRef,
    fields: Vec<wire::Field>,
) -> Result<Reply, Error> {
    call(
        store,
        actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(
                store
                    .asset(AssetRef::try_from(target).map_err(|_| Error::Invalid)?)?
                    .revision,
            ),
            kind: K::Device as i32,
            changes: fields,
            ..Default::default()
        }),
    )
}
fn read(store: &Store, actor: &Context, target: &wire::AssetRef) -> Result<api::Detail, Error> {
    match call(
        store,
        actor,
        Command::Read(api::Read {
            target: Some(target.clone()),
            ..Default::default()
        }),
    )? {
        Reply::Detail(d) => Ok(*d),
        _ => Err(Error::Corrupt),
    }
}

#[test]
fn partial_equipment_refinement_keeps_identity_current_only_and_reopenable() -> TestResult {
    // Different classification axes and twelve visible bays never instantiate disks.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Partly identified HPE")?;
    let quantity = wire::Field {
        name: "drive_bays".into(),
        value: Some(V::Equipment(wire::EquipmentClaim {
            basis: "12 front bays visible; total unknown".into(),
            assertion: Some(A::Quantity(wire::QuantityRange {
                lower: Some(12),
                upper: None,
                unit: "count".into(),
                scale: 0,
            })),
        })),
    };
    write(
        &store,
        &actor,
        &target,
        vec![
            text_field("manufacturer", "HPE"),
            text_field("generation", "Gen10"),
            quantity,
            possibilities(),
        ],
    )?;
    let before = store.usage()?;
    let d = read(&store, &actor, &target)?;
    let item = d.item.ok_or("item")?;
    assert_eq!(item.target, Some(target.clone()));
    assert!(item.fields.iter().any(|f| f.name == "model"));
    let mut reordered = possibilities();
    let Some(V::Equipment(c)) = &mut reordered.value else {
        panic!()
    };
    let Some(A::Alternatives(a)) = &mut c.assertion else {
        panic!()
    };
    a.candidates.reverse();
    write(&store, &actor, &target, vec![reordered])?;
    read(&store, &actor, &target)?;
    assert_eq!(store.usage()?, before);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(read(&store, &actor, &target)?.item, Some(item));
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            kind: K::Port as i32,
            parent: Some(target.clone()),
            changes: vec![text_field("name", "Retained physical port")],
            ..Default::default()
        }),
    )?;
    let relations = read(&store, &actor, &target)?.relations;
    assert!(!relations.is_empty());
    let reference = wire::EquipmentReference {
        issuer: "private/owner".into(),
        key: "verified-model".into(),
        basis: "measured/v1".into(),
    };
    write(
        &store,
        &actor,
        &target,
        vec![wire::Field {
            name: "model".into(),
            value: Some(V::Equipment(wire::EquipmentClaim {
                basis: "Reviewed chassis label".into(),
                assertion: Some(A::Model(reference.clone())),
            })),
        }],
    )?;
    let d = read(&store, &actor, &target)?;
    assert_eq!(
        d.item.as_ref().and_then(|i| i.target.as_ref()),
        Some(&target)
    );
    assert_eq!(d.relations, relations);
    assert!(d.item.ok_or("item")?.fields.iter().any(|f|f.name=="model" && matches!(&f.value,Some(V::Equipment(c)) if c.assertion==Some(A::Model(reference.clone())))));
    Ok(())
}

#[test]
fn equipment_denial_validation_and_stale_edits_are_atomic() -> TestResult {
    // A visible device does not disclose its hidden alternatives or permit their edit.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Opaque device")?;
    write(&store, &actor, &target, vec![possibilities()])?;
    let mut limited = operator();
    limited.grants[0].read = Fields::Selected(vec!["name".into()]);
    limited.grants[0].write = Fields::Selected(vec!["description".into()]);
    let before = store.usage()?;
    assert!(
        !read(&store, &limited, &target)?
            .item
            .ok_or("item")?
            .fields
            .iter()
            .any(|f| f.name == "model")
    );
    assert!(matches!(
        write(&store, &limited, &target, vec![possibilities()]),
        Err(Error::Denied)
    ));
    let mut bad = possibilities();
    let Some(V::Equipment(c)) = &mut bad.value else {
        panic!()
    };
    let Some(A::Alternatives(a)) = &mut c.assertion else {
        panic!()
    };
    a.residual_ppm = Some(0);
    assert!(matches!(
        write(&store, &actor, &target, vec![bad]),
        Err(Error::Invalid)
    ));
    let bad_unit = wire::Field {
        name: "drive_bays".into(),
        value: Some(V::Equipment(wire::EquipmentClaim {
            basis: "Visible bays".into(),
            assertion: Some(A::Quantity(wire::QuantityRange {
                lower: Some(12),
                unit: "mm".into(),
                ..Default::default()
            })),
        })),
    };
    assert!(matches!(
        write(&store, &actor, &target, vec![bad_unit]),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    let old = store.asset(AssetRef::try_from(&target)?)?.revision;
    write(
        &store,
        &actor,
        &target,
        vec![text_field("manufacturer", "HPE")],
    )?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(target),
                expected: Some(old),
                kind: K::Device as i32,
                changes: vec![text_field("manufacturer", "Other")],
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    Ok(())
}

#[test]
fn equipment_csv_roundtrip_preserves_uncertainty_and_scalar_values() -> TestResult {
    // Machine CSV must not flatten a hypothesis into an authoritative model label.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "CSV equipment")?;
    let mut alternatives = possibilities();
    let Some(V::Equipment(wire::EquipmentClaim {
        assertion: Some(A::Alternatives(value)),
        ..
    })) = alternatives.value.as_mut()
    else {
        panic!("alternatives fixture")
    };
    value.excluded.push(wire::EquipmentExclusion {
        value: "Tower-only chassis".into(),
        reason: "Rack ears and horizontal enclosure visible".into(),
        coverage: "Entire front visible".into(),
        ..Default::default()
    });
    write(
        &store,
        &actor,
        &target,
        vec![
            text_field("manufacturer", "HPE"),
            text_field("family", "equipment/claim/v1:literal label"),
            alternatives,
        ],
    )?;
    let q = api::Query {
        roots: vec![target.clone()],
        columns: vec![
            "name".into(),
            "manufacturer".into(),
            "family".into(),
            "model".into(),
        ],
        limit: 25,
        ..Default::default()
    };
    let Reply::Download(download) = call(
        &store,
        &actor,
        Command::Export(api::Export {
            query: Some(q),
            spreadsheet: false,
        }),
    )?
    else {
        panic!()
    };
    assert!(download.csv.contains("equipment/claim/v1:"));
    assert!(download.csv.contains("equipment/text/v1:"));
    let mut reader = csv::Reader::from_reader(download.csv.as_bytes());
    let headers = reader.headers()?.clone();
    let row = reader.records().next().ok_or("row")??;
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["external_id", "name", "manufacturer", "family", "model"])?;
    let mut values = vec!["copied-equipment".to_string()];
    for name in ["name", "manufacturer", "family", "model"] {
        let i = headers.iter().position(|h| h == name).ok_or("column")?;
        values.push(row[i].to_string());
    }
    writer.write_record(values)?;
    let csv = String::from_utf8(writer.into_inner()?)?;
    let Reply::Preview(p) = call(
        &store,
        &actor,
        Command::Import(api::Import {
            source: "equipment-fixture".into(),
            kind: K::Device as i32,
            csv,
            mappings: ["name", "manufacturer", "family", "model"]
                .into_iter()
                .map(|name| api::Mapping {
                    source: name.into(),
                    field: name.into(),
                })
                .collect(),
            ..Default::default()
        }),
    )?
    else {
        panic!()
    };
    assert!(p.id.is_some(), "{p:?}");
    let Reply::Mutation(m) = call(
        &store,
        &actor,
        Command::Import(api::Import {
            preview: p.id,
            ..Default::default()
        }),
    )?
    else {
        panic!()
    };
    let original = read(&store, &actor, &target)?.item.ok_or("item")?;
    let copied = read(&store, &actor, &m.targets[0])?.item.ok_or("item")?;
    assert_eq!(original.fields, copied.fields);
    assert_ne!(original.target, copied.target);
    Ok(())
}

#[test]
fn equipment_csv_empty_cells_preserve_unknown_and_existing_values() -> TestResult {
    // A mixed export contains blanks; they are absence, never zero or a clear.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let import = |csv: &str| -> Result<Vec<wire::AssetRef>, Error> {
        let Reply::Preview(preview) = call(
            &store,
            &actor,
            Command::Import(api::Import {
                source: "mixed-equipment".into(),
                kind: K::Device as i32,
                csv: csv.into(),
                mappings: ["name", "drive_bays", "manufacturer", "model", "height"]
                    .into_iter()
                    .map(|name| api::Mapping {
                        source: name.into(),
                        field: name.into(),
                    })
                    .collect(),
                ..Default::default()
            }),
        )?
        else {
            panic!("preview")
        };
        assert!(preview.id.is_some(), "{preview:?}");
        let Reply::Mutation(result) = call(
            &store,
            &actor,
            Command::Import(api::Import {
                preview: preview.id,
                ..Default::default()
            }),
        )?
        else {
            panic!("accepted import")
        };
        Ok(result.targets)
    };
    let targets = import(
        "external_id,name,drive_bays,manufacturer,model,height\nknown,Known equipment,12,HPE,,\nunknown,Unknown equipment,,,,\n",
    )?;
    let items = targets
        .iter()
        .map(|t| read(&store, &actor, t).map(|d| d.item.expect("item")))
        .collect::<Result<Vec<_>, _>>()?;
    let known = items
        .iter()
        .find(|i| i.fields.contains(&text_field("name", "Known equipment")))
        .ok_or("known equipment")?;
    let unknown = items
        .iter()
        .find(|i| i.fields.contains(&text_field("name", "Unknown equipment")))
        .ok_or("unknown equipment")?;
    assert_eq!(
        unknown.fields,
        vec![text_field("name", "Unknown equipment")]
    );
    let count = wire::Field {
        name: "drive_bays".into(),
        value: Some(V::Integer(12)),
    };
    assert!(known.fields.contains(&count));
    let target = known.target.as_ref().ok_or("target")?;
    import("external_id,name,drive_bays,manufacturer,model,height\nknown,Renamed equipment,,,,\n")?;
    assert!(
        read(&store, &actor, target)?
            .item
            .ok_or("item")?
            .fields
            .contains(&count)
    );
    import(
        "external_id,name,drive_bays,manufacturer,model,height\nknown,Renamed equipment,<clear>,,,\n",
    )?;
    assert!(
        read(&store, &actor, target)?
            .item
            .ok_or("item")?
            .fields
            .contains(&wire::Field {
                name: "drive_bays".into(),
                value: Some(V::Clear(0))
            })
    );
    Ok(())
}

#[test]
fn legacy_equipment_evidence_survives_full_save_and_freshness_refresh() -> TestResult {
    // Older stores admitted scalar evidence that new equipment edits must reject.
    use coordinode_core::graph::types::Value;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut record = asset();
    let identity = target(&record);
    let observed = omloom_contracts::ids::SourceId::generate().to_string();
    let imported = omloom_contracts::ids::SourceId::generate().to_string();
    record.observed = vec![observation(
        &observed,
        100,
        vec![text_field("manufacturer", "HPE")],
    )];
    record.imported = vec![observation(
        &imported,
        100,
        vec![text_field("manufacturer", "HPE")],
    )];
    record.intended.push(wire::IntendedField {
        field: Some(text_field("manufacturer", "HPE")),
        authority_id: omloom_contracts::ids::AuthorityId::generate().to_string(),
    });
    store.save_asset(
        &admit(wire::record::Body::Asset(record)),
        None,
        "fixture",
        100,
    )?;
    let legacy = "x".repeat(200);
    for suffix in [
        format!("observed/{observed}/manufacturer"),
        format!("imported/{imported}/manufacturer"),
        "intent/manufacturer".into(),
    ] {
        store.diagnostic_database().execute_cypher_shared(
            "MATCH (n:OmRow {key:$key}) SET n.vtext=$value RETURN n.key",
            Some(std::collections::HashMap::from([
                (
                    "key".into(),
                    Value::String(format!("asset/{}/{suffix}", identity.asset)),
                ),
                ("value".into(), Value::String(legacy.clone())),
            ])),
            None,
            None,
            None,
        )?;
    }
    let mut current = store.asset(identity)?;
    current.intended[0].field = Some(text_field("name", "Renamed legacy device"));
    store.save_asset(
        &admit(wire::record::Body::Asset(current.clone())),
        Some(current.revision),
        "fixture",
        200,
    )?;
    current = store.asset(identity)?;
    for group in current.observed.iter_mut().chain(&mut current.imported) {
        let evidence = group.evidence.as_mut().ok_or("evidence")?;
        evidence.observed_at_ms = 300;
        evidence.valid_until_ms = 1300;
    }
    store.refresh_asset(
        &admit(wire::record::Body::Asset(current.clone())),
        current.revision,
        store.usage()?.revision,
        "fixture",
        300,
    )?;
    let mut after = store.asset(identity)?;
    for group in after.observed.iter().chain(&after.imported) {
        let evidence = group.evidence.as_ref().ok_or("evidence")?;
        assert_eq!(evidence.fields, vec![text_field("manufacturer", &legacy)]);
        assert_eq!(evidence.observed_at_ms, 300);
    }
    let before = store.usage()?;
    after.observed[0]
        .evidence
        .as_mut()
        .ok_or("evidence")?
        .fields[0] = text_field("manufacturer", &"y".repeat(200));
    assert!(matches!(
        store.save_asset(
            &admit(wire::record::Body::Asset(after.clone())),
            Some(after.revision),
            "fixture",
            400
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    assert!(
        store
            .asset(identity)?
            .intended
            .iter()
            .any(|v| v.field == Some(text_field("manufacturer", &legacy)))
    );
    let mut reassigned = store.asset(identity)?;
    reassigned
        .intended
        .iter_mut()
        .find(|v| v.field.as_ref().is_some_and(|f| f.name == "manufacturer"))
        .ok_or("manufacturer")?
        .authority_id = omloom_contracts::ids::AuthorityId::generate().to_string();
    assert!(matches!(
        store.save_asset(
            &admit(wire::record::Body::Asset(reassigned.clone())),
            Some(reassigned.revision),
            "fixture",
            400
        ),
        Err(Error::Invalid)
    ));
    Ok(())
}

#[test]
fn equipment_sources_and_restore_keep_conflicts_independent() -> TestResult {
    // Source disagreement cannot overwrite accepted intent or another source's window.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut record = asset();
    let identity = target(&record);
    record.intended.push(wire::IntendedField {
        field: Some(possibilities()),
        authority_id: omloom_contracts::ids::AuthorityId::generate().to_string(),
    });
    let first = omloom_contracts::ids::SourceId::generate().to_string();
    let second = omloom_contracts::ids::SourceId::generate().to_string();
    record.observed = vec![
        observation(&first, 100, vec![text_field("model", "DL120")]),
        observation(&second, 200, vec![text_field("model", "DL160")]),
    ];
    store
        .save_asset(
            &admit(wire::record::Body::Asset(record.clone())),
            None,
            "fixture",
            200,
        )
        .map_err(|e| format!("saving source claims: {e}"))?;
    let before = store.usage()?;
    let d = read(&store, &operator(), record.target.as_ref().ok_or("target")?)
        .map_err(|e| format!("reading source claims: {e}"))?;
    assert_eq!(d.observed.len(), 2);
    assert!(matches!(
        d.item
            .ok_or("item")?
            .fields
            .iter()
            .find(|f| f.name == "model")
            .and_then(|f| f.value.as_ref()),
        Some(V::Equipment(_))
    ));
    assert_eq!(store.usage()?, before);
    record.observed = vec![observation(
        &second,
        300,
        vec![text_field("model", "Other")],
    )];
    store
        .refresh_asset(
            &admit(wire::record::Body::Asset(record)),
            1,
            before.revision,
            "fixture",
            300,
        )
        .map_err(|e| format!("refreshing source claims: {e}"))?;
    let current = store.asset(identity)?;
    assert_eq!(current.observed.len(), 2);
    let mut restricted = operator();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    let reference = wire::AssetRef {
        asset_id: identity.asset.to_string(),
        incarnation: identity.incarnation.to_string(),
    };
    let restricted_detail = read(&store, &restricted, &reference)?;
    assert!(restricted_detail.observed.is_empty());
    restricted.grants[0].scope = Scope::Targets(Vec::new());
    assert!(matches!(
        read(&store, &restricted, &reference),
        Err(Error::Denied)
    ));
    assert!(current.observed.iter().any(|o| {
        o.evidence
            .as_ref()
            .is_some_and(|e| e.source_id == first && e.observed_at_ms == 100)
    }));
    let mut image = Vec::new();
    store
        .export(&mut image)
        .map_err(|e| format!("exporting source claims: {e}"))?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &image, policy(), 16 * 1_048_576)
        .map_err(|e| format!("restoring source claims: {e}"))?;
    assert_eq!(restored.asset(identity)?, current);
    Ok(())
}

#[test]
fn concurrent_equipment_refinement_admits_one_reviewed_revision() -> TestResult {
    // Two editors cannot replace the same model basis from one accepted revision.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Concurrent device")?;
    write(&store, &actor, &target, vec![possibilities()])?;
    let expected = store.asset(AssetRef::try_from(&target)?)?.revision;
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            call(
                &store,
                &actor,
                Command::Write(api::Write {
                    target: Some(target.clone()),
                    expected: Some(expected),
                    kind: K::Device as i32,
                    changes: vec![text_field("model", "DL120")],
                    ..Default::default()
                }),
            )
        });
        let second = scope.spawn(|| {
            call(
                &store,
                &actor,
                Command::Write(api::Write {
                    target: Some(target.clone()),
                    expected: Some(expected),
                    kind: K::Device as i32,
                    changes: vec![text_field("model", "DL160")],
                    ..Default::default()
                }),
            )
        });
        [
            first.join().expect("first editor"),
            second.join().expect("second editor"),
        ]
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    Ok(())
}

#[test]
fn hidden_equipment_payload_is_not_decoded_or_explained() -> TestResult {
    // Corrupt source contents cannot turn a denied field into an observable error.
    use coordinode_core::graph::types::Value;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut actor = operator();
    let target = create(&store, &actor, K::Device, "Visible device")?;
    write(&store, &actor, &target, vec![possibilities()])?;
    store.diagnostic_database().execute_cypher_shared(
        "MATCH (n:OmRow {key:$key}) SET n.payload=$payload RETURN n.key",
        Some(std::collections::HashMap::from([
            (
                "key".into(),
                Value::String(format!("asset/{}/intent/model", target.asset_id)),
            ),
            ("payload".into(), Value::Binary(vec![255])),
        ])),
        None,
        None,
        None,
    )?;
    actor.grants[0].read = Fields::Selected(vec!["name".into()]);
    let detail = read(&store, &actor, &target)?;
    assert_eq!(
        detail.item.ok_or("item")?.fields,
        vec![text_field("name", "Visible device")]
    );
    assert!(detail.observed.is_empty());
    let query = api::Query {
        roots: vec![target.clone()],
        columns: vec!["name".into(), "model".into()],
        limit: 25,
        ..Default::default()
    };
    let Reply::Page(page) = call(&store, &actor, Command::Query(query.clone()))? else {
        panic!("page")
    };
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].fields,
        vec![text_field("name", "Visible device")]
    );
    assert!(matches!(
        call(&store, &operator(), Command::Query(query)),
        Err(Error::Corrupt)
    ));
    assert!(matches!(
        read(&store, &operator(), &target),
        Err(Error::Corrupt)
    ));
    Ok(())
}

#[test]
fn equivalent_measurements_do_not_rewrite_the_field() -> TestResult {
    // Fixed-point spelling is not a new measurement or a reason to write audit/history.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Measured chassis")?;
    let mut field = wire::Field {
        name: "height".into(),
        value: Some(V::Equipment(wire::EquipmentClaim {
            basis: "Independent ruler measurement".into(),
            assertion: Some(A::Quantity(wire::QuantityRange {
                lower: Some(120),
                upper: None,
                unit: "mm".into(),
                scale: 1,
            })),
        })),
    };
    write(&store, &actor, &target, vec![field.clone()])?;
    let before = store.usage()?;
    let key = format!("asset/{}/intent/height", target.asset_id);
    let version = row_version(&store, &key)?;
    let Some(V::Equipment(c)) = &mut field.value else {
        panic!()
    };
    let Some(A::Quantity(q)) = &mut c.assertion else {
        panic!()
    };
    q.lower = Some(12);
    q.scale = 0;
    write(&store, &actor, &target, vec![field.clone()])?;
    assert_eq!(store.usage()?, before);
    assert_eq!(row_version(&store, &key)?, version);
    assert!(
        read(&store, &actor, &target)?
            .item
            .ok_or("item")?
            .fields
            .contains(&field)
    );
    Ok(())
}
