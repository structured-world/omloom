use super::*;
use api::{reply::Result as Reply, request::Command};

// Spreadsheet protection applies to untrusted text, never to typed negative numbers.
#[test]
fn csv_preserves_numbers_and_neutralizes_text_prefixes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut columns = Vec::new();
    for (name, kind) in [
        ("Payload", api::FieldType::Text),
        ("Count", api::FieldType::Integer),
        ("Enabled", api::FieldType::Boolean),
    ] {
        let Reply::Definitions(definitions) = call(
            &store,
            &actor,
            Command::Definition(api::Definition {
                name: name.into(),
                r#type: kind as i32,
                kinds: vec![K::Device as i32],
                ..Default::default()
            }),
        )?
        else {
            panic!("definition");
        };
        columns.push(definitions.items[0].id.clone());
    }
    let values = [
        "-5",
        "=1+1",
        " +1",
        "@SUM(1)",
        "\tplain",
        "\rplain",
        "\nplain",
        "＝1+1",
        "＋1",
        "－1",
        "＠SUM(1)",
        "ordinary",
    ];
    for (index, value) in values.iter().enumerate() {
        let target = create(&store, &actor, K::Device, &format!("case-{index:02}"))?;
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                target: Some(target),
                expected: Some(1),
                kind: K::Device as i32,
                changes: vec![
                    text_field(&columns[0], value),
                    wire::Field {
                        name: columns[1].clone(),
                        value: Some(wire::field::Value::Integer(-5)),
                    },
                    wire::Field {
                        name: columns[2].clone(),
                        value: Some(wire::field::Value::Flag(true)),
                    },
                ],
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    for spreadsheet in [true, false] {
        let mut query = query();
        query.columns = columns.clone();
        let Reply::Download(download) = call(
            &store,
            &actor,
            Command::Export(api::Export {
                query: Some(query),
                spreadsheet,
            }),
        )?
        else {
            panic!("download");
        };
        let mut reader = csv::Reader::from_reader(download.csv.as_bytes());
        let rows = reader.records().collect::<Result<Vec<_>, _>>()?;
        assert_eq!(rows.len(), values.len());
        for (row, value) in rows.iter().zip(values) {
            assert_eq!(&row[4], "-5", "typed integer must stay numeric");
            assert_eq!(&row[5], "true");
            let expected = if spreadsheet && value != "ordinary" {
                format!("'{value}")
            } else {
                value.into()
            };
            assert_eq!(&row[3], expected);
        }
    }
    assert_eq!(store.usage()?, before, "exports cannot write");
    Ok(())
}
