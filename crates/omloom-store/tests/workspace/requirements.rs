use super::*;
use api::{reply::Result as Reply, request::Command};

#[test]
fn focused_fact_read_is_selective_authorized_and_read_only() -> TestResult {
    // One correction must not expand sources/relations or confuse a denied field with unknown.
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Fact owner")?;
    store.set_diagnostic_query_check(|db, statement| {
        let plan = db
            .explain_plan(statement, None)
            .expect("focused read plan")
            .explain();
        assert!(plan.contains("IndexScan("), "point lookup: {plan}");
        assert!(
            !plan.contains("NodeScan(") && !plan.contains("Traverse("),
            "no expansion: {plan}"
        );
    });
    let before = store.usage()?;
    let request = api::Read {
        target: Some(target.clone()),
        accepted_field: Some("drive_bays".into()),
        ..Default::default()
    };
    let Reply::Detail(detail) = call(&store, &actor, Command::Read(request.clone()))? else {
        return Err("detail".into());
    };
    let item = detail.item.as_ref().ok_or("item")?;
    assert_eq!(item.fields, vec![text_field("name", "Fact owner")]);
    assert_eq!(
        detail.as_ref(),
        &api::Detail {
            item: Some(item.clone()),
            ..Default::default()
        }
    );
    assert_eq!(store.usage()?, before);
    let start = std::time::Instant::now();
    for _ in 0..50 {
        let Reply::Detail(next) = call(&store, &actor, Command::Read(request.clone()))? else {
            return Err("detail".into());
        };
        assert_eq!(next, detail);
    }
    eprintln!("50 indexed focused reads: {:?}", start.elapsed());
    let mut limited = actor.clone();
    limited.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(matches!(
        call(&store, &limited, Command::Read(request.clone())),
        Err(Error::Denied)
    ));
    limited.grants[0].read = Fields::Selected(vec!["drive_bays".into()]);
    let Reply::Detail(detail) = call(&store, &limited, Command::Read(request.clone()))? else {
        return Err("detail".into());
    };
    assert!(detail.item.ok_or("item")?.fields.is_empty());
    for invalid in [
        api::Read {
            relations_cursor: Some("untrusted".into()),
            ..request.clone()
        },
        api::Read {
            network_cursor: Some("untrusted".into()),
            ..request.clone()
        },
        api::Read {
            accepted_field: Some(String::new()),
            ..request.clone()
        },
        api::Read {
            accepted_field: Some("a".repeat(65)),
            ..request
        },
    ] {
        assert!(matches!(
            call(&store, &actor, Command::Read(invalid)),
            Err(Error::Invalid)
        ));
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn focused_fact_read_returns_only_the_populated_permitted_field() -> TestResult {
    // A focused correction must decode its actual value without returning unrelated facts.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Populated fact")?;
    let original = store.asset(AssetRef::try_from(&target)?)?;
    let bays = wire::Field {
        name: "drive_bays".into(),
        value: Some(wire::field::Value::Integer(8)),
    };
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(original.revision),
            kind: K::Device as i32,
            changes: vec![bays.clone(), text_field("description", "Unrelated fact")],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    let read = api::Read {
        target: Some(target.clone()),
        accepted_field: Some("drive_bays".into()),
        ..Default::default()
    };
    let Reply::Detail(detail) = call(&store, &actor, Command::Read(read.clone()))? else {
        return Err("detail".into());
    };
    let item = detail.item.as_ref().ok_or("item")?;
    assert_eq!(item.target.as_ref(), Some(&target));
    assert_eq!(item.revision, original.revision + 1);
    assert_eq!(
        item.fields,
        vec![bays.clone(), text_field("name", "Populated fact")]
    );
    assert_eq!(
        detail.as_ref(),
        &api::Detail {
            item: Some(item.clone()),
            ..Default::default()
        }
    );
    let mut limited = actor.clone();
    limited.grants[0].read = Fields::Selected(vec!["drive_bays".into()]);
    let Reply::Detail(detail) = call(&store, &limited, Command::Read(read.clone()))? else {
        return Err("detail".into());
    };
    assert_eq!(detail.item.ok_or("item")?.fields, vec![bays]);
    limited.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(matches!(
        call(&store, &limited, Command::Read(read)),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn focused_fact_delta_rechecks_the_same_question_and_fences_old_edits() -> TestResult {
    // Recording eight bays answers a ten-bay question without rewriting its threshold or other facts.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Question target")?;
    let mut predicate = condition(&target, "drive_bays", wire::field::Value::Integer(10));
    let Some(api::requirement_node::Expression::Field(field)) = &mut predicate.expression else {
        return Err("field".into());
    };
    field.comparison = api::RequirementComparison::Minimum as i32;
    let unknown = evaluate(&store, &actor, &target, vec![predicate.clone()], 0)?;
    assert_eq!(unknown.reason, api::RequirementReason::Missing as i32);
    let original = store.asset(AssetRef::try_from(&target)?)?;
    let write = api::Write {
        target: Some(target.clone()),
        expected: Some(original.revision),
        kind: K::Device as i32,
        changes: vec![wire::Field {
            name: "drive_bays".into(),
            value: Some(wire::field::Value::Integer(8)),
        }],
        ..Default::default()
    };
    let mut denied = actor.clone();
    denied.grants[0].write = Fields::Selected(vec!["description".into()]);
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &denied, Command::Write(write.clone())),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    call(&store, &actor, Command::Write(write.clone()))?;
    let before = store.usage()?;
    let failed = evaluate(&store, &actor, &target, vec![predicate.clone()], 0)?;
    assert_eq!(failed.state, api::RequirementState::Failed as i32);
    assert_eq!(
        failed.results[0].semantic_key,
        unknown.results[0].semantic_key
    );
    assert!(matches!(
        call(&store, &actor, Command::Write(write)),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let current = store.asset(AssetRef::try_from(&target)?)?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(current.revision),
            kind: K::Device as i32,
            changes: vec![wire::Field {
                name: "drive_bays".into(),
                value: Some(wire::field::Value::Equipment(wire::EquipmentClaim {
                    basis: "Reviewed incomplete count".into(),
                    assertion: Some(wire::equipment_claim::Assertion::Quantity(
                        wire::QuantityRange {
                            lower: Some(8),
                            upper: Some(12),
                            unit: "count".into(),
                            scale: 0,
                        },
                    )),
                })),
            }],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    let partial = evaluate(&store, &actor, &target, vec![predicate], 0)?;
    assert_eq!(partial.state, api::RequirementState::Unknown as i32);
    assert_eq!(
        partial.results[0].semantic_key,
        unknown.results[0].semantic_key
    );
    let current = store.asset(AssetRef::try_from(&target)?)?;
    assert_eq!(
        current
            .intended
            .iter()
            .find_map(|v| v.field.as_ref().filter(|f| f.name == "name")),
        Some(&text_field("name", "Question target"))
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn condition(
    target: &wire::AssetRef,
    field: &str,
    value: wire::field::Value,
) -> api::RequirementNode {
    api::RequirementNode {
        expression: Some(api::requirement_node::Expression::Field(
            api::FieldRequirement {
                target: Some(target.clone()),
                expected: Some(wire::Field {
                    name: field.into(),
                    value: Some(value),
                }),
                comparison: api::RequirementComparison::Equal as i32,
                layer: api::RequirementLayer::Accepted as i32,
            },
        )),
    }
}
fn evaluate(
    store: &Store,
    actor: &Context,
    anchor: &wire::AssetRef,
    nodes: Vec<api::RequirementNode>,
    root: u32,
) -> Result<api::RequirementEvaluation, Error> {
    match call(
        store,
        actor,
        Command::EvaluateRequirements(api::EvaluateRequirements {
            anchor: Some(anchor.clone()),
            nodes,
            root,
        }),
    )? {
        Reply::RequirementEvaluation(value) => Ok(value),
        _ => Err(Error::Corrupt),
    }
}

#[test]
fn requirements_use_real_current_facts_without_writes_or_unselected_expansion() -> TestResult {
    // A corrected current fact changes only its result, never the semantic question identity.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Measured machine")?;
    let predicate = condition(
        &target,
        "name",
        wire::field::Value::Text("Measured machine".into()),
    );
    let mut unused = predicate.clone();
    let Some(api::requirement_node::Expression::Field(field)) = &mut unused.expression else {
        panic!("field")
    };
    field.comparison = api::RequirementComparison::Unspecified as i32;
    let before = store.usage()?;
    let original = evaluate(&store, &actor, &target, vec![predicate.clone(), unused], 0)?;
    assert_eq!(original.state, api::RequirementState::Satisfied as i32);
    assert_eq!(original.results.len(), 1);
    assert!(original.results[0].resolutions.is_empty());
    assert_eq!(store.usage()?, before);
    let old = store.asset(AssetRef::try_from(&target)?)?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            kind: K::Device as i32,
            expected: Some(old.revision),
            changes: vec![text_field("name", "Corrected machine")],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    let changed = evaluate(&store, &actor, &target, vec![predicate], 0)?;
    assert_eq!(changed.state, api::RequirementState::Failed as i32);
    assert_eq!(
        changed.results[0].semantic_key,
        original.results[0].semantic_key
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_hide_denied_and_missing_dependencies_without_false_success() -> TestResult {
    // A narrow resource screen cannot turn a denied upstream fact into an empty safe set.
    use omloom_contracts::ids::{AssetId, IncarnationId};
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let anchor = create(&store, &actor, K::VirtualMachine, "Visible VM")?;
    let hidden = create(&store, &actor, K::Device, "Private hypervisor")?;
    let mut limited = actor;
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&anchor)?]);
    let before = store.usage()?;
    let missing = wire::AssetRef {
        asset_id: AssetId::generate().to_string(),
        incarnation: IncarnationId::generate().to_string(),
    };
    let mismatch = wire::AssetRef {
        asset_id: hidden.asset_id.clone(),
        incarnation: IncarnationId::generate().to_string(),
    };
    // Missing, protected and mismatched incarnations must have the same public result.
    for dependency in [hidden, missing, mismatch] {
        let result = evaluate(
            &store,
            &limited,
            &anchor,
            vec![condition(
                &dependency,
                "name",
                wire::field::Value::Text("Private hypervisor".into()),
            )],
            0,
        )?;
        assert_eq!(result.state, api::RequirementState::Unknown as i32);
        assert_eq!(result.reason, api::RequirementReason::Restricted as i32);
        assert!(result.results[0].target.is_none());
        assert!(result.results[0].field.is_none());
        assert!(result.results[0].evidence.is_empty());
        assert!(result.results[0].resolutions.is_empty());
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_conflicting_sources_cannot_pass_the_same_allowed_set() -> TestResult {
    // Distinct exact model claims conflict even if both individually belong to the allowed set.
    use omloom_contracts::ids::AuthorityId;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Source disagreement")?;
    let mut asset = store.asset(AssetRef::try_from(&target)?)?;
    let previous = asset.revision;
    asset.observed = vec![
        observation(
            &AuthorityId::generate().to_string(),
            50,
            vec![text_field("model", "A")],
        ),
        observation(
            &AuthorityId::generate().to_string(),
            50,
            vec![text_field("model", "B")],
        ),
    ];
    store.refresh_asset(
        &admit(wire::record::Body::Asset(asset)),
        previous,
        store.usage()?.revision,
        "fixture",
        60,
    )?;
    let mut node = condition(
        &target,
        "model",
        wire::field::Value::Equipment(wire::EquipmentClaim {
            basis: "Either model is acceptable".into(),
            assertion: Some(wire::equipment_claim::Assertion::Alternatives(
                wire::EquipmentAlternatives {
                    exhaustive: true,
                    candidates: ["A", "B"]
                        .into_iter()
                        .map(|value| wire::EquipmentAlternative {
                            value: value.into(),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
            )),
        }),
    );
    let Some(api::requirement_node::Expression::Field(field)) = &mut node.expression else {
        panic!("field")
    };
    field.layer = api::RequirementLayer::Observed as i32;
    let before = store.usage()?;
    let result = evaluate(&store, &actor, &target, vec![node.clone()], 0)?;
    assert_eq!(result.reason, api::RequirementReason::Conflict as i32);
    let Some(api::requirement_node::Expression::Field(field)) = &mut node.expression else {
        panic!("field")
    };
    let Some(wire::field::Value::Equipment(claim)) =
        &mut field.expected.as_mut().ok_or(Error::Invalid)?.value
    else {
        panic!("claim")
    };
    claim.basis = "Same predicate with another explanation".into();
    let Some(wire::equipment_claim::Assertion::Alternatives(values)) = &mut claim.assertion else {
        panic!("alternatives")
    };
    values.candidates.reverse();
    let reordered = evaluate(&store, &actor, &target, vec![node], 0)?;
    assert_eq!(
        result.results[0].semantic_key,
        reordered.results[0].semantic_key
    );
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_owner_only_account_explains_a_request_without_granting_access() -> TestResult {
    // Company/resource rights cannot manufacture provider delegation or secret release.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let account = create(&store, &actor, K::ProviderAccount, "Owner-held registrar")?;
    store.workspace(
        &actor,
        api::Request {
            working: Some(api::WorkingContext {
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
            command: Some(Command::ExternalAccount(api::ExternalAccount {
                target: Some(account.clone()),
                expected: Some(store.asset(AssetRef::try_from(&account)?)?.revision),
                metadata: Some(api::AccountMetadata {
                    issuer: "registrar.example".into(),
                    subject: "selected-account".into(),
                    kind: api::ExternalAccountKind::Administration as i32,
                    delegation: api::AccountDelegation::OwnerOnly as i32,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        },
        100,
    )?;
    let node = condition(
        &account,
        "delegation",
        wire::field::Value::Text(api::AccountDelegation::Scoped.as_str_name().into()),
    );
    let before = store.usage()?;
    let result = evaluate(&store, &actor, &account, vec![node.clone()], 0)?;
    assert_eq!(
        result.reason,
        api::RequirementReason::OwnerActionRequired as i32
    );
    assert!(
        result.results[0]
            .resolutions
            .contains(&(api::RequirementResolution::OwnerAction as i32))
    );
    let mut limited = actor;
    limited.grants[0].writable = false;
    let result = evaluate(&store, &limited, &account, vec![node], 0)?;
    assert_eq!(
        result.reason,
        api::RequirementReason::OwnerActionRequired as i32
    );
    assert!(result.results[0].resolutions.is_empty());
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_observation_expiry_and_field_rights_preserve_unknown() -> TestResult {
    // A retained but expired claim is not current evidence; field denial reveals no provenance.
    use omloom_contracts::ids::AuthorityId;
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Dated measurement")?;
    store.set_diagnostic_query_check(|db, statement| {
        if statement.contains("n.layer='observed'") || statement.contains("n.layer='freshness'") {
            let plan = db
                .explain_plan(statement, None)
                .expect("selected evidence plan")
                .explain();
            assert!(
                plan.contains("IndexScan"),
                "selected evidence must use its parent index: {plan}"
            );
        }
    });
    let mut asset = store.asset(AssetRef::try_from(&target)?)?;
    let revision = asset.revision;
    let mut observed = observation(
        &AuthorityId::generate().to_string(),
        50,
        vec![text_field("model", "A")],
    );
    let scan = observed.scan_id.clone();
    observed
        .evidence
        .as_mut()
        .ok_or(Error::Invalid)?
        .valid_until_ms = 99;
    asset.observed = vec![observed];
    store.refresh_asset(
        &admit(wire::record::Body::Asset(asset)),
        revision,
        store.usage()?.revision,
        "fixture",
        60,
    )?;
    let mut node = condition(&target, "model", wire::field::Value::Text("A".into()));
    let Some(api::requirement_node::Expression::Field(field)) = &mut node.expression else {
        panic!("field")
    };
    field.layer = api::RequirementLayer::Observed as i32;
    let before = store.usage()?;
    let result = evaluate(&store, &actor, &target, vec![node.clone()], 0)?;
    assert_eq!(result.reason, api::RequirementReason::Stale as i32);
    assert!(
        !result.results[0]
            .resolutions
            .contains(&(api::RequirementResolution::RecordFact as i32))
    );
    assert_eq!(result.results[0].evidence[0].scan_id, scan);
    assert_eq!(result.results[0].evidence[0].valid_until_ms, Some(99));
    let mut limited = actor;
    limited.grants[0].read = Fields::Selected(vec!["name".into()]);
    limited.grants[0].write = Fields::Selected(vec!["name".into()]);
    let result = evaluate(&store, &limited, &target, vec![node], 0)?;
    assert_eq!(result.reason, api::RequirementReason::Restricted as i32);
    assert!(result.results[0].target.is_none());
    assert!(result.results[0].field.is_none());
    assert!(result.results[0].evidence.is_empty());
    assert!(result.results[0].resolutions.is_empty());
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_api_combines_distinct_subjects_and_rejects_cycles() -> TestResult {
    // A threshold uses distinct scoped resources; graph cycles never admit an action.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::Device, "A")?;
    let b = create(&store, &actor, K::Device, "B")?;
    let first = condition(&a, "name", wire::field::Value::Text("A".into()));
    let second = condition(&b, "name", wire::field::Value::Text("B".into()));
    let group = |members, minimum| api::RequirementNode {
        expression: Some(api::requirement_node::Expression::Group(
            api::RequirementGroup {
                kind: api::RequirementGroupKind::AtLeast as i32,
                members,
                minimum,
            },
        )),
    };
    let before = store.usage()?;
    let result = evaluate(
        &store,
        &actor,
        &a,
        vec![first.clone(), second.clone(), group(vec![0, 1], 2)],
        2,
    )?;
    assert_eq!(result.state, api::RequirementState::Satisfied as i32);
    let reordered = evaluate(
        &store,
        &actor,
        &a,
        vec![group(vec![2, 1], 2), second, first.clone()],
        0,
    )?;
    assert_eq!(
        result
            .results
            .iter()
            .find(|r| r.node == 2)
            .ok_or(Error::Missing)?
            .semantic_key,
        reordered
            .results
            .iter()
            .find(|r| r.node == 0)
            .ok_or(Error::Missing)?
            .semantic_key
    );
    let result = evaluate(
        &store,
        &actor,
        &a,
        vec![first.clone(), group(vec![0, 0], 2)],
        1,
    )?;
    assert_eq!(result.state, api::RequirementState::Failed as i32);
    let result = evaluate(&store, &actor, &a, vec![first, group(vec![1], 1)], 1)?;
    assert_eq!(result.reason, api::RequirementReason::Cycle as i32);
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_many_sources_use_two_bounded_evidence_queries() -> TestResult {
    // Freshness is fetched as one selected batch, not one query per source.
    use omloom_contracts::ids::AuthorityId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static QUERIES: AtomicUsize = AtomicUsize::new(0);
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Thirty-two sources")?;
    let mut asset = store.asset(AssetRef::try_from(&target)?)?;
    let revision = asset.revision;
    asset.observed = (0..32)
        .map(|_| {
            observation(
                &AuthorityId::generate().to_string(),
                50,
                vec![text_field("model", "A")],
            )
        })
        .collect();
    store.refresh_asset(
        &admit(wire::record::Body::Asset(asset)),
        revision,
        store.usage()?.revision,
        "fixture",
        60,
    )?;
    store.set_diagnostic_query_check(|_, statement| {
        if statement.contains("n.layer='observed'") || statement.contains("n.layer='freshness'") {
            QUERIES.fetch_add(1, Ordering::Relaxed);
        }
    });
    let mut node = condition(&target, "model", wire::field::Value::Text("A".into()));
    let Some(api::requirement_node::Expression::Field(field)) = &mut node.expression else {
        panic!("field")
    };
    field.layer = api::RequirementLayer::Observed as i32;
    let before = store.usage()?;
    let result = evaluate(&store, &actor, &target, vec![node], 0)?;
    assert_eq!(result.state, api::RequirementState::Satisfied as i32);
    assert_eq!(result.results[0].evidence.len(), 32);
    assert_eq!(QUERIES.load(Ordering::Relaxed), 2);
    assert_eq!(store.usage()?, before);
    Ok(())
}

#[test]
fn requirements_wrapping_one_resource_cannot_manufacture_two_voters() -> TestResult {
    // Logical grouping is not an independently qualified voter identity.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "One voter")?;
    let group = |kind, members, minimum| api::RequirementNode {
        expression: Some(api::requirement_node::Expression::Group(
            api::RequirementGroup {
                kind: kind as i32,
                members,
                minimum,
            },
        )),
    };
    let result = evaluate(
        &store,
        &actor,
        &target,
        vec![
            condition(
                &target,
                "name",
                wire::field::Value::Text("One voter".into()),
            ),
            group(api::RequirementGroupKind::All, vec![0], 0),
            group(api::RequirementGroupKind::Any, vec![0], 0),
            group(api::RequirementGroupKind::AtLeast, vec![1, 2], 2),
        ],
        3,
    )?;
    assert_eq!(result.state, api::RequirementState::Unknown as i32);
    assert_eq!(result.reason, api::RequirementReason::Incomplete as i32);
    Ok(())
}

#[test]
fn requirements_api_defers_unselected_group_semantics() -> TestResult {
    // Both the wire adapter and common core must ignore unfinished alternative methods.
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Selected method")?;
    let selected = condition(
        &target,
        "name",
        wire::field::Value::Text("Selected method".into()),
    );
    let before = store.usage()?;
    for (kind, members, minimum) in [
        (api::RequirementGroupKind::All, vec![], 0),
        (api::RequirementGroupKind::AtLeast, vec![0], 2),
        (api::RequirementGroupKind::Any, vec![0], 1),
        (api::RequirementGroupKind::Unspecified, vec![0], 0),
    ] {
        let other = api::RequirementNode {
            expression: Some(api::requirement_node::Expression::Group(
                api::RequirementGroup {
                    kind: kind as i32,
                    members,
                    minimum,
                },
            )),
        };
        let nodes = vec![selected.clone(), other];
        let result = evaluate(&store, &actor, &target, nodes.clone(), 0)?;
        assert_eq!(result.state, api::RequirementState::Satisfied as i32);
        assert_eq!(result.results.len(), 1);
        assert!(matches!(
            evaluate(&store, &actor, &target, nodes, 1),
            Err(Error::Invalid)
        ));
    }
    assert_eq!(store.usage()?, before);
    Ok(())
}
