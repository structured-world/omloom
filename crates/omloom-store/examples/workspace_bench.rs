//! Reproducible current-workspace qualification through the actual bounded import API.
use omloom_contracts::{wire::AssetKind, workspace as api};
use omloom_store::{Budget, CLASSES, Policy, Store, workspace::*};
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
    let store = Store::open(directory.path(), policy)?;
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
        }))?;
        let Some(api::reply::Result::Preview(preview)) = response.result else {
            return Err("Expected preview".into());
        };
        execute(api::request::Command::Import(api::Import {
            preview: Some(preview.id.ok_or("Invalid fixture CSV")?),
            ..Default::default()
        }))?;
    }
    println!("Seeded {count} resources in {:?}", start.elapsed());
    let before = store.usage()?;
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
    println!(
        "logical bytes={} rows={}",
        before.bytes.iter().sum::<u64>(),
        before.rows.iter().sum::<u64>()
    );
    Ok(())
}
