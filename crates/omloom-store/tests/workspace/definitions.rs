use super::*;
use api::{reply::Result as Reply, request::Command};

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
