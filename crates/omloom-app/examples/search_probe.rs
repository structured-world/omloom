//! Inspect actual database plans and stage costs on an existing test-owned inventory.
use omloom_contracts::workspace as api;
use omloom_store::Store;
use std::{error::Error, path::Path, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let data = args.next().ok_or("Supply a test-owned data directory")?;
    let search = args.next().ok_or("Supply a search term (empty for list)")?;
    let output = args.next().ok_or("Supply the diagnostic report path")?;
    let browse = match args.next().as_deref() {
        None => false,
        Some("browse") => true,
        Some(_) => return Err("The optional mode must be browse".into()),
    };
    let store =
        Store::open_with_query_memory(Path::new(&data), omloom_app::policy(), 128 * 1_048_576)?;
    let context = omloom_app::fixture("operator")?;
    let request = api::Request {
        working: None,
        command: Some(api::request::Command::Query(api::Query {
            search: search.clone(),
            view: if browse {
                api::View::Hardware as i32
            } else {
                api::View::Overview as i32
            },
            level_only: browse,
            include_context: browse,
            omit_host_local: browse,
            limit: 25,
            sort: if browse { "name".into() } else { String::new() },
            columns: if browse {
                vec!["name".into(), "description".into()]
            } else {
                vec!["name".into(), "description".into(), "serial".into()]
            },
            ..Default::default()
        })),
    };
    for _ in 0..if browse { 0 } else { 3 } {
        store.workspace(&context, request.clone(), 1)?;
    }
    let db = store.diagnostic_database();
    db.persist()?;
    db.query_registry().reset();
    let before = store.usage()?;
    let syncs = db.engine().journal_syncs();
    let mut samples = Vec::new();
    for _ in 0..if browse { 1 } else { 5 } {
        let start = Instant::now();
        let reply = store.workspace(&context, request.clone(), 1)?;
        let Some(api::reply::Result::Page(page)) = reply.result else {
            return Err("Expected page".into());
        };
        samples.push(serde_json::json!({"wall_us": start.elapsed().as_micros(), "total": page.total, "returned": page.items.len()}));
    }
    assert_eq!(store.usage()?, before);
    assert_eq!(db.engine().journal_syncs(), syncs);
    let queries: Vec<_> = db
        .query_registry()
        .top_by_impact(32)
        .into_iter()
        .map(|q| {
            serde_json::json!({
                "query": q.canonical_query, "calls": q.count, "total_us": q.total_time_us,
                "min_us": q.min_time_us, "max_us": q.max_time_us, "plan": q.last_plan,
            })
        })
        .collect();
    let report = serde_json::json!({"search": search, "browse": browse, "samples": samples, "queries": queries});
    std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    Ok(())
}
