//! Reproducible current-workspace qualification through the actual bounded import API.
use omloom_contracts::{wire::AssetKind, workspace as api};
use omloom_store::{Budget, CLASSES, Policy, Store, workspace::*};
use prost::Message;
use std::{error::Error, time::Instant};
fn main() -> Result<(), Box<dyn Error>> {
    let count: usize = std::env::args()
        .nth(1)
        .ok_or("Supply the inventory size")?
        .parse()?;
    if !(100..=10_000).contains(&count) {
        return Err("Size must be between 100 and 10000".into());
    }
    let directory = tempfile::tempdir()?;
    let policy = Policy {
        classes: [Budget {
            rows: 100_000,
            bytes: 64 * 1_048_576,
        }; CLASSES],
        total_bytes: 256 * 1_048_576,
        detail_age_ms: 60_000,
        review_age_ms: 60_000,
        review_per_source: Budget {
            rows: 512,
            bytes: 2 * 1_048_576,
        },
        batch_rows: 128,
    };
    let memory_mib: u64 = std::env::args()
        .nth(2)
        .ok_or("Supply statement memory in MiB")?
        .parse()?;
    let store = Store::open_with_query_memory(
        directory.path(),
        policy.clone(),
        memory_mib
            .checked_mul(1_048_576)
            .ok_or("Memory size overflow")?,
    )?;
    println!("statement memory={memory_mib} MiB");
    let context = Context {
        actor: "benchmark".into(),
        fixture: "benchmark".into(),
        grants: vec![Grant {
            scope: Scope::All,
            read: Fields::All,
            write: Fields::All,
            writable: true,
        }],
        teams: Vec::new(),
        recipients: Vec::new(),
        schema_admin: true,
        authority_revision: 1,
    };
    let execute = |command| {
        store.workspace(
            &context,
            api::Request {
                command: Some(command),
                working: Some(api::WorkingContext::default()),
            },
            100,
        )
    };
    let start = Instant::now();
    for base in (0..count).step_by(128) {
        let mut csv = String::from("external_id,name\n");
        for index in base..(base + 128).min(count) {
            csv.push_str(&format!("host-{index},edge-{index:05}\n"));
        }
        let response = execute(api::request::Command::Import(api::Import {
            csv,
            source: "qualification".into(),
            kind: AssetKind::Device as i32,
            mappings: vec![api::Mapping {
                source: "name".into(),
                field: "name".into(),
            }],
            ..Default::default()
        }))
        .map_err(|error| format!("Preview at row {base}: {error}"))?;
        let Some(api::reply::Result::Preview(preview)) = response.result else {
            return Err("Expected preview".into());
        };
        execute(api::request::Command::Import(api::Import {
            preview: Some(preview.id.ok_or("Invalid fixture CSV")?),
            ..Default::default()
        }))
        .map_err(|error| format!("Accept at row {base}: {error}"))?;
        if base % 1024 == 0 {
            println!("Accepted {} resources", (base + 128).min(count));
        }
    }
    println!("Seeded {count} resources in {:?}", start.elapsed());
    store.diagnostic_database().persist()?;
    let mut taps = Vec::new();
    for partition in coordinode_storage::engine::partition::Partition::all() {
        let (tap, _) = store
            .diagnostic_database()
            .engine()
            .tap_writes(*partition)?;
        taps.push(tap);
    }
    let before = store.usage()?;
    let syncs = store.diagnostic_database().engine().journal_syncs();
    for search in ["", "edge-00042", "edge"] {
        let mut samples = Vec::with_capacity(30);
        for _ in 0..30 {
            let start = Instant::now();
            let response = execute(api::request::Command::Query(api::Query {
                limit: 25,
                search: search.into(),
                columns: vec!["name".into()],
                ..Default::default()
            }))?;
            let Some(api::reply::Result::Page(page)) = response.result else {
                return Err("Expected page".into());
            };
            if search == "edge-00042" {
                assert_eq!(page.total, 1);
            } else {
                assert_eq!(page.total, count as u64);
            }
            samples.push(start.elapsed());
        }
        samples.sort_unstable();
        println!(
            "query={search:?} P50={:?} P95={:?} P99={:?}",
            samples[15], samples[28], samples[29]
        );
    }
    assert_eq!(store.usage()?, before, "Retrieval must not write");
    for tap in taps {
        let writes = tap.take();
        assert_eq!(
            writes,
            coordinode_storage::engine::tap::Tapped::Keys(Vec::new()),
            "Unexpected {:?} write during retrieval: {writes:?}",
            tap.partition()
        );
    }
    assert_eq!(
        store.diagnostic_database().engine().journal_syncs(),
        syncs,
        "Retrieval must not sync the journal"
    );
    println!(
        "logical bytes={} rows={}",
        before.bytes.iter().sum::<u64>(),
        before.rows.iter().sum::<u64>()
    );
    projection_cases(&policy, &context)?;
    Ok(())
}

fn projection_cases(policy: &Policy, context: &Context) -> Result<(), Box<dyn Error>> {
    use api::{reply::Result as Reply, request::Command};
    use omloom_contracts::wire::{Field, RelationKind, field};
    for wide in [false, true] {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path(), policy.clone())?;
        let execute = |command| {
            store.workspace(
                context,
                api::Request {
                    command: Some(command),
                    working: Some(api::WorkingContext::default()),
                },
                100,
            )
        };
        let field = |name: &str, value: String| Field {
            name: name.into(),
            value: Some(field::Value::Text(value)),
        };
        let mut columns = vec!["name".to_owned()];
        if wide {
            for index in 0..11 {
                let name = format!("Payload {index}");
                let Some(Reply::Definitions(definitions)) =
                    execute(Command::Definition(api::Definition {
                        name: name.clone(),
                        kinds: vec![AssetKind::Device as i32],
                        r#type: api::FieldType::Text as i32,
                        ..Default::default()
                    }))?
                    .result
                else {
                    return Err("Expected definitions".into());
                };
                columns.push(
                    definitions
                        .items
                        .iter()
                        .find(|d| d.name == name)
                        .ok_or("Missing definition")?
                        .id
                        .clone(),
                );
            }
        }
        for index in 0..if wide { 25 } else { 2 } {
            let mut changes = vec![field("name", format!("device-{index:02}"))];
            if wide {
                changes.extend(
                    columns[1..]
                        .iter()
                        .map(|name| field(name, "x".repeat(2048))),
                );
            }
            let Some(Reply::Mutation(parent)) = execute(Command::Write(api::Write {
                kind: AssetKind::Device as i32,
                changes,
                ..Default::default()
            }))?
            .result
            else {
                return Err("Expected resource".into());
            };
            if !wide {
                for (kind, relation) in [
                    (AssetKind::Device, RelationKind::Contains),
                    (AssetKind::Port, RelationKind::HasPort),
                ] {
                    let Some(Reply::Mutation(child)) = execute(Command::Write(api::Write {
                        kind: kind as i32,
                        changes: vec![field("name", format!("child-{index}-{kind:?}"))],
                        ..Default::default()
                    }))?
                    .result
                    else {
                        return Err("Expected child".into());
                    };
                    execute(Command::Relation(api::Relationship {
                        relation: Some(omloom_contracts::wire::Relation {
                            from: Some(parent.targets[0].clone()),
                            to: Some(child.targets[0].clone()),
                            kind: relation as i32,
                            ..Default::default()
                        }),
                        remove: false,
                    }))?;
                }
            }
        }
        let before = store.usage()?;
        let mut samples = Vec::with_capacity(9);
        let mut bytes = 0;
        for _ in 0..9 {
            let start = Instant::now();
            let response = execute(Command::Query(api::Query {
                limit: 25,
                columns: columns.clone(),
                unplaced: !wide,
                include_context: wide,
                ..Default::default()
            }))?;
            samples.push(start.elapsed());
            bytes = response.encoded_len();
            let Some(Reply::Page(page)) = response.result else {
                return Err("Expected page".into());
            };
            assert_eq!(page.total, if wide { 25 } else { 2 });
        }
        samples.sort_unstable();
        println!(
            "projection={} P50={:?} reply_bytes={bytes} logical_bytes={}",
            if wide { "wide" } else { "unplaced" },
            samples[4],
            before.bytes.iter().sum::<u64>()
        );
        assert_eq!(store.usage()?, before);
    }
    Ok(())
}
