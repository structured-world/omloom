use super::*;
use api::{reply::Result as Reply, request::Command};

// Accepted schemas must remain readable through the host's bounded metadata reply.
#[test]
fn definition_admission_preserves_metadata_reply_budget() -> TestResult {
    use prost::Message;
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut accepted = Vec::new();
    let mut rejected = false;
    for index in 0..52 {
        let before = store.usage()?;
        let result = call(
            &store,
            &actor,
            Command::Definition(api::Definition {
                name: format!("Choices {index}"),
                r#type: api::FieldType::Choice as i32,
                kinds: vec![K::Device as i32],
                choices: (0..64)
                    .map(|n| format!("{n:03}{}", "x".repeat(157)))
                    .collect(),
                ..Default::default()
            }),
        );
        match result {
            Ok(Reply::Definitions(created)) => accepted.push(created.items[0].clone()),
            Err(Error::Limit) => {
                assert_eq!(store.usage()?, before, "capacity refusal must be atomic");
                rejected = true;
                break;
            }
            other => panic!("unexpected definition result: {other:?}"),
        }
    }
    assert!(rejected, "oversized definition set was admitted");
    // Updates and migrations share admission, including changes to retired metadata.
    let Reply::Definitions(small) = call(
        &store,
        &actor,
        Command::Definition(api::Definition {
            name: "Small field".into(),
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("small definition");
    };
    accepted.push(small.items[0].clone());
    let mut expanded = small.items[0].clone();
    expanded.r#type = api::FieldType::Choice as i32;
    expanded.choices = accepted[0].choices.clone();
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Migration(api::Migration {
                definition: Some(expanded),
                expected: before.revision,
                apply: true,
                ..Default::default()
            })
        ),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, before);
    let mut update_rejected = false;
    for original in &accepted {
        let mut updated = original.clone();
        updated.description = "d".repeat(1000);
        updated.retired = true;
        let before = store.usage()?;
        match call(&store, &actor, Command::Definition(updated)) {
            Ok(Reply::Definitions(_)) => {}
            Err(Error::Limit) => {
                assert_eq!(store.usage()?, before);
                update_rejected = true;
                break;
            }
            other => panic!("unexpected update: {other:?}"),
        }
    }
    assert!(
        update_rejected,
        "schema growth bypassed aggregate admission"
    );
    let before = store.usage()?;
    let Reply::Definitions(metadata) = call(&store, &actor, Command::Definitions(api::Empty {}))?
    else {
        panic!("metadata");
    };
    let count = metadata.items.len();
    let reply = api::Reply {
        result: Some(Reply::Definitions(metadata)),
    };
    assert!(reply.encoded_len() <= MAX_REPLY_BYTES);
    assert_eq!(count, accepted.len());
    assert_eq!(store.usage()?, before, "metadata reads must not write");
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert!(
        matches!(call(&store, &actor, Command::Definitions(api::Empty {}))?, Reply::Definitions(items) if items.items.len() == count)
    );
    Ok(())
}

fn reordered_applicability(migrate: bool) -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "typed resource")?;
    let Reply::Definitions(created) = call(
        &store,
        &actor,
        Command::Definition(api::Definition {
            name: "Purpose".into(),
            kinds: vec![K::Device as i32, K::VirtualMachine as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition");
    };
    let mut definition = created.items[0].clone();
    // Earlier writers preserved caller order. Reading and saving that row must not rewrite it.
    store.diagnostic_database().execute_cypher_shared(
        &format!(
            "MATCH (n:OmRow {{key:'definition/{}'}}) SET n.kinds=[{},{}]",
            definition.id, definition.kinds[1], definition.kinds[0]
        ),
        None,
        None,
        None,
        None,
    )?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(&definition.id, "backup")],
            ..Default::default()
        }),
    )?;
    let before = store.usage()?;
    let asset = store.asset(AssetRef::try_from(&target)?)?;
    let original = definition.clone();
    definition.kinds.reverse();
    let command = if migrate {
        Command::Migration(api::Migration {
            definition: Some(definition),
            expected: before.revision,
            apply: true,
            ..Default::default()
        })
    } else {
        Command::Definition(definition)
    };
    call(&store, &actor, command)?;
    assert_eq!(
        store.usage()?,
        before,
        "reordered set must not write or advance revisions"
    );
    assert_eq!(store.asset(AssetRef::try_from(&target)?)?, asset);
    let Reply::Definitions(current) = call(&store, &actor, Command::Definitions(api::Empty {}))?
    else {
        panic!("definitions");
    };
    assert_eq!(current.items[0], original);
    let mut q = query();
    q.predicates.push(api::Predicate {
        field: original.id.clone(),
        definition_generation: Some(original.generation),
        value: Some(text_field(&original.id, "backup")),
    });
    let Reply::Page(page) = call(&store, &actor, Command::Query(q))? else {
        panic!("page");
    };
    assert_eq!(page.total, 1, "existing predicates retain their meaning");
    Ok(())
}

// Rechecking the same applicable kinds in a different order is a metadata no-op.
#[test]
fn reordered_applicability_does_not_conflict_or_write() -> TestResult {
    reordered_applicability(false)
}

// A migration with the same kind set must not invalidate values or saved predicates.
#[test]
fn reordered_applicability_does_not_advance_generations() -> TestResult {
    reordered_applicability(true)
}

// Numeric parsing accepts plus signs; typed dates require exactly YYYY-MM-DD and a real date.
#[test]
fn date_fields_reject_signed_components_without_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "dated resource")?;
    let Reply::Definitions(created) = call(
        &store,
        &actor,
        Command::Definition(api::Definition {
            name: "Installation date".into(),
            r#type: api::FieldType::Date as i32,
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition");
    };
    let id = &created.items[0].id;
    for date in [
        "+202-01-01",
        "2024-+1-01",
        "2024-01-+1",
        "2024-+1-+1",
        "2023-02-29",
        "2024-04-31",
        "0000-01-01",
    ] {
        let before = store.usage()?;
        let result = call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(target.clone()),
                expected: Some(1),
                kind: K::Device as i32,
                changes: vec![text_field(id, date)],
                ..Default::default()
            }),
        );
        assert!(
            matches!(result, Err(Error::Invalid)),
            "accepted noncanonical date {date}: {result:?}"
        );
        assert_eq!(store.usage()?, before);
    }
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(target.clone()),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(id, "2024-02-29")],
            ..Default::default()
        }),
    )?;
    assert!(
        store
            .asset(AssetRef::try_from(&target)?)?
            .intended
            .iter()
            .any(|value| value.field.as_ref() == Some(&text_field(id, "2024-02-29")))
    );
    Ok(())
}
