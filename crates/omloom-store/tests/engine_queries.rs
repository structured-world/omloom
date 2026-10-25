#![cfg(feature = "std")]

use coordinode_core::graph::types::Value;
use coordinode_embed::Database;

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Nullable search branches must keep a match from another visible typed field.
#[test]
fn disk_queries_preserve_three_valued_boolean_results() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut db = Database::open(dir.path())?;
    let rows = db.execute_cypher(
        "RETURN null OR true AS a, null AND false AS b, null OR false AS c, \
         null AND true AS d, NOT (null AND false) AS e",
    )?;
    assert_eq!(rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(rows[0].get("b"), Some(&Value::Bool(false)));
    assert_eq!(rows[0].get("c"), Some(&Value::Null));
    assert_eq!(rows[0].get("d"), Some(&Value::Null));
    assert_eq!(rows[0].get("e"), Some(&Value::Bool(true)));
    Ok(())
}

// Conditional network/group keys must correlate per asset, before and after reopen.
#[test]
fn disk_queries_correlate_case_in_and_subscript_keys() -> TestResult {
    for indexed in [false, true] {
        let dir = tempfile::tempdir()?;
        let mut db = Database::open(dir.path())?;
        if indexed {
            db.execute_cypher("CREATE UNIQUE INDEX om_query_key ON :OmQuery(key)")?;
        }
        db.execute_cypher("CREATE (:OmQuery {key:'group/one'}), (:OmQuery {key:'network/two'})")?;
        drop(db);
        let mut db = Database::open(dir.path())?;
        for expression in [
            "CASE e WHEN 1 THEN 'group/one' WHEN 2 THEN 'network/two' END",
            "CASE WHEN e IN [1] THEN 'group/one' WHEN e IN [2] THEN 'network/two' END",
            "['group/one','network/two','missing'][e-1]",
        ] {
            let rows = db.execute_cypher(&format!(
                "UNWIND [1,2,3] AS e MATCH (n:OmQuery) WHERE n.key={expression} \
                 RETURN n.key AS key ORDER BY key"
            ))?;
            assert_eq!(
                rows.iter().map(|r| r.get("key")).collect::<Vec<_>>(),
                vec![Some(&Value::String("group/one".into())), Some(&Value::String("network/two".into()))],
                "indexed={indexed}, expression={expression}"
            );
        }
    }
    Ok(())
}
