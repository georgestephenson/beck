//! The read model, and the wire an outside tool reaches it on.
//!
//! Two halves, deliberately separated because they fail for different reasons.
//!
//! * **The schema** is derived from the program: which tables a corpus program has, what their
//!   columns are, and the rule that decides. That half needs no socket and no runtime.
//! * **The wire** is driven by `tokio-postgres` — somebody else's Postgres client, the same one
//!   `beck-rt`'s Postgres log store uses. A protocol server tested by a client written beside it
//!   tests agreement with itself, which is what `docs/82` §82.10 found four gates doing; this one
//!   is held to a driver that has never heard of Beck.
//!
//! `docs/23-incremental-views-report.md` is the report.

use std::sync::Arc;

use beck_core::read::{Cardinality, Schema, Source, SqlTy};
use beck_rt::{App, AppConfig, MemoryLog};

mod support;
use support::{command, todo_program, todo_runtime};

fn schema_of(src: &str, name: &str) -> Schema {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("compiles");
    let plan = beck_core::plan::Plan::compile(&placed);
    Schema::of(&placed, &plan)
}

// -------------------------------------------------------------------------------------------
// The schema
// -------------------------------------------------------------------------------------------

#[test]
fn the_sketchs_read_model_is_its_todos() {
    let placed = todo_program();
    let plan = beck_core::plan::Plan::compile(&placed);
    let schema = Schema::of(&placed, &plan);

    let todos = schema.table("todos").expect("a table per collection field");
    assert_eq!(todos.source, Source::State(vec![Arc::from("todos")]));
    assert_eq!(todos.cardinality, Cardinality::Many);
    let columns: Vec<(&str, SqlTy)> = todos
        .columns
        .iter()
        .map(|c| (c.name.as_ref(), c.ty))
        .collect();
    // Declared order, not name order — and `Id`, a newtype over `Str`, is text.
    assert_eq!(
        columns,
        vec![
            ("id", SqlTy::Text),
            ("text", SqlTy::Text),
            ("done", SqlTy::Boolean),
            ("owner", SqlTy::Text),
        ]
    );

    // The page is not a table, and neither is the accumulator: `Html` is not a relation, and
    // `State`'s only field is already `todos`.
    assert!(schema.table("page").is_none());
    assert!(schema.table("state").is_none());
    let names: Vec<&str> = schema.tables.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, vec!["todos", Schema::CATALOGUE]);
}

#[test]
fn a_state_that_is_not_a_collection_is_one_row() {
    let schema = schema_of(
        include_str!("../../../corpus/03-billing.beck"),
        "corpus/03-billing.beck",
    );
    let t = schema.table("ledger").expect("the fold's own name");
    assert_eq!(t.cardinality, Cardinality::One);
    assert_eq!(
        t.columns
            .iter()
            .map(|c| c.name.as_ref())
            .collect::<Vec<_>>(),
        vec!["charged", "refused"]
    );
}

#[test]
fn a_derived_signal_is_a_table_read_from_the_maintained_node() {
    let schema = schema_of(
        include_str!("../../../corpus/22-shared.beck"),
        "corpus/22-shared.beck",
    );
    // `tally: Signal[Tally] = signal_map(ballot, summarise)` — one row, and its rows come from the
    // plan rather than from the state, which is the half the view engine earns.
    let t = schema.table("tally").expect("the derived signal");
    assert!(matches!(t.source, Source::View(_)), "{:?}", t.source);
    assert_eq!(t.cardinality, Cardinality::One);
    // And the accumulator it derives from is not a table of its own.
    assert!(schema.table("ballot").is_none());
}

#[test]
fn two_folds_give_the_fields_of_both() {
    let schema = schema_of(
        include_str!("../../../corpus/21-two-folds.beck"),
        "corpus/21-two-folds.beck",
    );
    // The accumulator is fused, so a table's path starts at the field this fold occupies.
    let here = schema.table("here").expect("the roster's collection");
    assert_eq!(
        here.source,
        Source::State(vec![Arc::from("roster"), Arc::from("here")])
    );
    let tally = schema.table("tally").expect("the other fold's scalars");
    assert_eq!(tally.source, Source::State(vec![Arc::from("tally")]));
}

#[test]
fn every_corpus_program_has_a_schema_and_none_of_it_is_a_page() {
    let mut with_tables = 0;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("beck") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let name = path.display().to_string();
        let (placed, diags, map) = beck_core::compile_str(&name, &src);
        assert!(!diags.has_errors(), "{name}: {}", diags.render(&map));
        let Some(placed) = placed else { continue };
        if !placed.is_application() {
            continue;
        }
        let plan = beck_core::plan::Plan::compile(&placed);
        let schema = Schema::of(&placed, &plan);
        // The catalogue is always there, so "has a read model" means more than one table.
        if schema.tables.len() > 1 {
            with_tables += 1;
        }
        for t in &schema.tables {
            assert!(
                t.name.as_ref() != placed.roles.page_name.as_ref(),
                "{name}: the page is a table"
            );
            assert!(!t.columns.is_empty(), "{name}: {} has no columns", t.name);
            // Nothing per-session: a SQL client has no session to be one for.
            if let Source::View(op) = &t.source {
                assert!(
                    !plan.nodes[*op].per_session,
                    "{name}: {} reads the session",
                    t.name
                );
            }
        }
        // The DDL is what a person is shown, so it has to be parseable by the SQL that shows it.
        let ddl = schema.ddl();
        assert!(ddl.contains("create table"), "{name}: no DDL");
    }
    assert!(
        with_tables >= 25,
        "only {with_tables} corpus programs have a read model"
    );
}

// -------------------------------------------------------------------------------------------
// The SQL
// -------------------------------------------------------------------------------------------

/// A running application with three todos in it, and its schema.
async fn app_with_todos() -> Arc<App> {
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("starts");
    for (id, text) in [("1", "milk"), ("2", "bread"), ("3", "jam")] {
        app.propose(
            format!("cmd-{id}"),
            "ana",
            command("Add", &[("id", id), ("text", text)]),
        )
        .await
        .expect("accepted");
    }
    app.propose(
        "cmd-toggle".into(),
        "ana",
        command("Toggle", &[("id", "2")]),
    )
    .await
    .expect("accepted");
    app
}

/// Connect a real Postgres client to a served read model.
async fn connect(app: Arc<App>) -> tokio_postgres::Client {
    let listener = beck_rt::pgwire::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("binds");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = beck_rt::pgwire::serve_on(listener, app).await;
    });
    let (client, connection) = tokio_postgres::connect(
        &format!(
            "host=127.0.0.1 port={} user=nobody dbname=beck",
            addr.port()
        ),
        tokio_postgres::NoTls,
    )
    .await
    .expect("a Postgres client connects with no password");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

#[tokio::test]
async fn a_postgres_client_reads_the_rows_the_fold_holds() {
    let app = app_with_todos().await;
    let client = connect(app).await;

    // The extended query protocol, in binary format: this is what every driver does.
    let rows = client
        .query("select id, text, done from todos order by text", &[])
        .await
        .expect("a query");
    let got: Vec<(String, String, bool)> = rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    assert_eq!(
        got,
        vec![
            ("2".to_string(), "bread".to_string(), true),
            ("3".to_string(), "jam".to_string(), false),
            ("1".to_string(), "milk".to_string(), false),
        ]
    );

    let count: i64 = client
        .query_one("select count(*) from todos", &[])
        .await
        .expect("a count")
        .get(0);
    assert_eq!(count, 3);

    let open: i64 = client
        .query_one("select count(*) from todos where done = false", &[])
        .await
        .expect("a filtered count")
        .get(0);
    assert_eq!(open, 2);
}

#[tokio::test]
async fn a_query_sees_an_event_as_soon_as_it_is_acknowledged() {
    let app = app_with_todos().await;
    let client = connect(app.clone()).await;

    let before: i64 = client
        .query_one("select count(*) from todos", &[])
        .await
        .unwrap()
        .get(0);

    app.propose(
        "cmd-4".into(),
        "ana",
        command("Add", &[("id", "4"), ("text", "tea")]),
    )
    .await
    .expect("accepted");

    // No subscriber has rendered, nothing has been projected, and no lag was waited out: the
    // ack means committed, and the query advances the dataflow itself.
    let after: i64 = client
        .query_one("select count(*) from todos", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(after, before + 1);

    let text: String = client
        .query_one("select text from todos where id = '4'", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(text, "tea");
}

#[tokio::test]
async fn the_simple_query_protocol_answers_in_text() {
    let app = app_with_todos().await;
    let client = connect(app).await;
    let messages = client
        .simple_query("select text from todos order by text desc limit 1")
        .await
        .expect("a simple query");
    let row = messages
        .iter()
        .find_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some(r),
            _ => None,
        })
        .expect("one row");
    assert_eq!(row.get(0), Some("milk"));
}

#[tokio::test]
async fn the_catalogue_says_what_there_is() {
    let app = app_with_todos().await;
    let client = connect(app).await;
    let rows = client
        .query(
            "select column_name, data_type from beck_columns where table_name = 'todos' \
             order by position",
            &[],
        )
        .await
        .expect("the catalogue");
    let got: Vec<(String, String)> = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    assert_eq!(
        got,
        vec![
            ("id".to_string(), "text".to_string()),
            ("text".to_string(), "text".to_string()),
            ("done".to_string(), "boolean".to_string()),
            ("owner".to_string(), "text".to_string()),
        ]
    );
}

#[tokio::test]
async fn a_write_is_refused_and_the_connection_survives_it() {
    let app = app_with_todos().await;
    let client = connect(app).await;

    let e = client
        .simple_query("delete from todos")
        .await
        .expect_err("a read model is read-only");
    let db = e.as_db_error().expect("a real error response");
    assert_eq!(db.code().code(), "0A000");
    assert!(db.message().contains("read-only"), "{}", db.message());

    // The session recovers: an error is followed by ReadyForQuery, not by a closed socket.
    let count: i64 = client
        .query_one("select count(*) from todos", &[])
        .await
        .expect("still usable")
        .get(0);
    assert_eq!(count, 3);
}

#[tokio::test]
async fn a_missing_table_names_the_catalogue() {
    let app = app_with_todos().await;
    let client = connect(app).await;
    let e = client
        .query("select * from orders", &[])
        .await
        .expect_err("no such table");
    let db = e.as_db_error().expect("a real error response");
    assert_eq!(db.code().code(), "42P01");
    assert!(db.message().contains(Schema::CATALOGUE), "{}", db.message());
}

#[tokio::test]
async fn a_derived_table_is_served_from_the_arrangement_the_page_uses() {
    let src = include_str!("../../../corpus/22-shared.beck");
    let (placed, diags, map) = beck_core::compile_str("corpus/22-shared.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("compiles");
    let backend = beck_eval::backend(&placed);
    let runtime = beck_rt::Runtime::new(placed, backend).expect("prepares");
    let app = App::start(runtime, Arc::new(MemoryLog::new()), AppConfig::default())
        .await
        .expect("starts");

    let client = connect(app.clone()).await;
    let (options, open): (i64, bool) = {
        let r = client
            .query_one("select options, \"open\" from tally", &[])
            .await
            .expect("the derived table");
        (r.get(0), r.get(1))
    };
    assert_eq!((options, open), (0, false));

    for (i, option) in ["yes", "no"].iter().enumerate() {
        app.propose(
            format!("v{i}"),
            "ana",
            command("Vote", &[("option", option)]),
        )
        .await
        .expect("accepted");
    }

    let r = client
        .query_one("select options, \"open\" from tally", &[])
        .await
        .expect("the derived table again");
    assert_eq!((r.get::<_, i64>(0), r.get::<_, bool>(1)), (2, true));

    // The dataflow was advanced by the query rather than by a subscriber: nothing has rendered a
    // page in this test, and `advances` counts the advances.
    assert!(
        app.shared_dataflow().advances() > 0,
        "the query did not advance the shared dataflow"
    );
    assert_eq!(
        app.shared_dataflow().readers(),
        1,
        "the connection is a reader"
    );
}

/// A query with no `from` is answered, which is what a driver asks before it trusts a connection.
///
/// That it is answered *without a credential* is the absence
/// `pending_security.rs::the_read_model_port_authenticates_nobody_and_answers_only_to_localhost`
/// asserts, and it is asserted there rather than here because it is security debt rather than a
/// feature of the read model.
#[tokio::test]
async fn a_query_with_no_table_is_answered() {
    let app = app_with_todos().await;
    let client = connect(app).await;
    let one: i64 = client.query_one("select 1", &[]).await.unwrap().get(0);
    assert_eq!(one, 1);
    let version: String = client
        .query_one("select version()", &[])
        .await
        .unwrap()
        .get(0);
    assert!(version.contains("beck"), "{version}");
}

// -------------------------------------------------------------------------------------------
// `count(*)` without scanning — docs/23 §23.19
// -------------------------------------------------------------------------------------------

/// A reader that knows how many rows there are and **refuses to produce one**.
///
/// The instrument, and it is the assertion rather than a way of measuring one: a query that had to
/// scan cannot be answered by this at all. Timing would say the same thing more slowly and would
/// flake ([`docs/13`](../../../../docs/13-testing.md) §13.7); counting scans would need somewhere
/// to put the counter.
struct Countless(u64);

impl beck_core::read::Rows for Countless {
    fn scan(
        &self,
        _table: &beck_core::read::Table,
    ) -> Result<Vec<beck_core::Value>, beck_core::read::SqlError> {
        Err(beck_core::read::SqlError {
            message: "this query scanned".to_string(),
            code: "XX000",
        })
    }

    fn count(
        &self,
        _table: &beck_core::read::Table,
    ) -> Result<Option<u64>, beck_core::read::SqlError> {
        Ok(Some(self.0))
    }
}

/// A reader that can only scan, which is what every implementation was before the seam existed.
struct Scanner(Vec<beck_core::Value>);

impl beck_core::read::Rows for Scanner {
    fn scan(
        &self,
        _table: &beck_core::read::Table,
    ) -> Result<Vec<beck_core::Value>, beck_core::read::SqlError> {
        Ok(self.0.clone())
    }
}

fn counted(answer: &beck_core::read::Answer) -> i64 {
    match answer.rows.first().and_then(|r| r.first()) {
        Some(Some(beck_core::read::Datum::Bigint(n))) => *n,
        other => panic!("that is not a count: {other:?}"),
    }
}

/// **`select count(*)` builds no row**, which is
/// [`docs/23`](../../../../docs/23-incremental-views-report.md) §23.19's last open row.
///
/// The plan's `list_len` has been `±1` per delta since the engine existed; the SQL count was over
/// the rows it had scanned, so asking `psql` how many todos there are cloned every todo and built a
/// `Cell` per column of every one, to answer with a single integer. A maintained arrangement is a
/// `BTreeMap` and a `Map` in the accumulator knows its length, and neither was being asked.
#[test]
fn a_bare_count_is_answered_without_building_a_row() {
    let schema = {
        let placed = todo_program();
        let plan = beck_core::plan::Plan::compile(&placed);
        Schema::of(&placed, &plan)
    };

    // A million rows and a reader that cannot produce one. If this answers, nothing scanned.
    let answer = schema
        .run("select count(*) from todos", &Countless(1_000_000))
        .expect("a count needs no rows");
    assert_eq!(counted(&answer), 1_000_000);

    // The harness has teeth: the same reader cannot answer anything that needs a row.
    let scanned = schema.run("select * from todos", &Countless(3));
    assert!(
        scanned.is_err(),
        "the refusing reader answered a query that needs rows, so the test above proves nothing"
    );
}

/// Where the fast path stops, asserted rather than left to be discovered.
///
/// It is conservative on purpose: a `where` needs the rows to test, and an `order`, `limit` or
/// `offset` is applied *before* the count collapses, so honouring one here would mean reproducing
/// that behaviour rather than skipping work. Each of those still scans, and each is still right.
#[test]
fn a_count_that_narrows_anything_still_scans() {
    let schema = {
        let placed = todo_program();
        let plan = beck_core::plan::Plan::compile(&placed);
        Schema::of(&placed, &plan)
    };

    for sql in [
        "select count(*) from todos where done = false",
        "select count(*) from todos limit 1",
        "select count(*) from todos offset 1",
        "select count(*) from todos order by id",
    ] {
        assert!(
            schema.run(sql, &Countless(7)).is_err(),
            "`{sql}` took the fast path, which does not honour what narrows it"
        );
    }
}

/// **A reader that does not implement `count` is still right**, which is what makes the seam safe.
///
/// The trait's default answers "not without a scan", so an implementation written before it existed
/// — or one for a source that genuinely cannot say — falls back and is exactly as correct and
/// exactly as slow as it was.
#[test]
fn a_reader_that_cannot_count_falls_back_to_scanning() {
    let schema = {
        let placed = todo_program();
        let plan = beck_core::plan::Plan::compile(&placed);
        Schema::of(&placed, &plan)
    };

    let mut rows = Vec::new();
    for (id, text) in [("1", "milk"), ("2", "bread")] {
        let mut fields = beck_core::core::Fields::new();
        fields.insert(Arc::from("id"), beck_core::Value::str_(id));
        fields.insert(Arc::from("text"), beck_core::Value::str_(text));
        fields.insert(Arc::from("done"), beck_core::Value::Bool(false));
        fields.insert(Arc::from("owner"), beck_core::Value::str_("ana"));
        rows.push(beck_core::Value::data(Arc::from("Todo"), None, fields));
    }

    let answer = schema
        .run("select count(*) from todos", &Scanner(rows))
        .expect("the fallback answers");
    assert_eq!(counted(&answer), 2);
}
