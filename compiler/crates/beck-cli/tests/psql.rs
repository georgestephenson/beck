//! `psql` itself, against a running program's read models.
//!
//! # Why the binary and not a client written here
//!
//! `docs/82` §82.10 found four gates in this project that could not have failed, and the shape was
//! the same each time: the thing under test was checked by something written beside it. A `\d`
//! answered by a client this repository wrote would be a `\d` this repository defined — and the
//! whole claim of [`beck_core::pg`] is that **`psql`'s own idea of `\d`** is answered, catalogue
//! joins, regular expressions, `case` arms and all. So the gate runs the binary a DBA would run,
//! reads what it printed, and asserts the table is in it.
//!
//! `read_models.rs` holds the same discipline one level down, with `tokio-postgres` driving the
//! wire. This is the level above: a tool that has never heard of Beck, sending queries nobody here
//! wrote.
//!
//! # The skip, and how to forbid it
//!
//! `psql` is not in every environment, so this suite **skips loudly** — `BECK_REQUIRE_PSQL=1`
//! forbids the skip, in the same shape as `BECK_REQUIRE_TAR` (`image.rs`) and
//! `BECK_REQUIRE_BROWSER` (`browser.rs`). `docs/19` §19.4 item 10 is why the skip prints: a gate
//! nobody ran that reports success is worse than one that reports nothing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use beck_rt::{App, AppConfig, MemoryLog};

mod support;
use support::{command, todo_runtime};

/// The `psql` this runs, or nothing — and a printed reason, unless a skip is forbidden.
fn psql() -> Option<PathBuf> {
    let found = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| Path::new(d).join("psql"))
        .find(|p| p.is_file());
    if found.is_none() {
        assert!(
            std::env::var("BECK_REQUIRE_PSQL").as_deref() != Ok("1"),
            "BECK_REQUIRE_PSQL=1 and there is no `psql` on the path"
        );
        println!("skipped: no `psql`. Set BECK_REQUIRE_PSQL=1 to make this a failure.");
    }
    found
}

/// A served read model, and the port it is on.
///
/// The listener is bound before the task is spawned so the port is known and open by the time
/// `psql` is started: a test that raced its own server would be a test that fails for the wrong
/// reason.
///
/// Every test here is `multi_thread` for the other half of that: `psql` is run with a **blocking**
/// `Command`, and on a current-thread runtime the server task it is talking to would not be polled
/// while that call waits. The symptom is a connection that never completes rather than an error,
/// which is why the flavour is stated at each test rather than left to the default.
async fn serve() -> u16 {
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
    let listener = beck_rt::pgwire::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("binds");
    let port = listener.local_addr().expect("an address").port();
    tokio::spawn(async move {
        let _ = beck_rt::pgwire::serve_on(listener, app).await;
    });
    port
}

/// Run one `psql` command and answer what it printed, standard error included.
///
/// `ON_ERROR_STOP` is off on purpose: a refusal is one of the things this asserts, and it has to
/// come back as text rather than as a process that died. The whole output is one string because
/// what `psql` prints is the artefact — a `\d` that answered with the right rows and formatted
/// them as an error would still be wrong.
fn run(psql: &Path, port: u16, argument: &str) -> String {
    let out = Command::new(psql)
        .arg(format!(
            "host=127.0.0.1 port={port} user=nobody dbname=beck connect_timeout=10"
        ))
        .arg("-c")
        .arg(argument)
        // No .psqlrc, no pager, no colour: the machine running this must not be able to change
        // what the test reads.
        .arg("--no-psqlrc")
        .arg("--pset=pager=off")
        .env("PGCONNECT_TIMEOUT", "10")
        .env("PGPASSWORD", "")
        .output()
        .expect("psql runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// **`\d` lists the program's read models, and nothing that is not one.**
///
/// The negative half is the one that would go red if the catalogue were faked: the fourteen
/// `pg_catalog` relations are *in* `pg_class`, and `psql` hides them with
/// `nspname <> 'pg_catalog'` over a `left join`. A catalogue that put everything in one namespace,
/// or a `where` pushed into the null-supplying side of that join, lists all sixteen — and this
/// asserts on the count as well as on the names, so it cannot pass by listing too much.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn psql_lists_the_read_models() {
    let Some(psql) = psql() else { return };
    let port = serve().await;

    let out = run(&psql, port, "\\d");
    assert!(out.contains("List of relations"), "{out}");
    assert!(out.contains("todos"), "{out}");
    assert!(out.contains("beck_columns"), "{out}");
    assert!(out.contains("(2 rows)"), "{out}");
    // A read model is an ordinary table in a namespace a person can name.
    assert!(out.contains("public"), "{out}");
    assert!(out.contains("table"), "{out}");
    assert!(!out.contains("pg_class"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");

    // `\dt` is the same query with a narrower `relkind`, and it answers the same tables.
    let out = run(&psql, port, "\\dt");
    assert!(out.contains("todos") && out.contains("(2 rows)"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");
}

/// **`\d <table>` prints the columns the compiler derived, with their types and nullability.**
///
/// This is the whole of the row `docs/08` carried: the four columns of `Todo` come from the Beck
/// type, `Id` is a newtype over `Str` and prints as `text`, and none of them is nullable because
/// none is an `Option`. `psql` sends eight queries to print this — the last five about policies,
/// statistics, publications and inheritance — so it also asserts that the relations a read model
/// has *none* of answered with no rows rather than with an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn psql_describes_a_read_model() {
    let Some(psql) = psql() else { return };
    let port = serve().await;

    let out = run(&psql, port, "\\d todos");
    assert!(out.contains("Table \"public.todos\""), "{out}");
    for column in ["id", "text", "done", "owner"] {
        assert!(out.contains(column), "{column} is missing from:\n{out}");
    }
    // The types are the ones the schema derived, not a default.
    assert!(out.contains("boolean"), "{out}");
    assert!(out.contains("not null"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");

    // A pattern is a regular expression `psql` builds, and it reaches the same table.
    let out = run(&psql, port, "\\d tod*");
    assert!(out.contains("Table \"public.todos\""), "{out}");

    // And a name that is not a read model is not found — rather than answered with nothing.
    let out = run(&psql, port, "\\d nosuch");
    assert!(out.contains("Did not find any relation"), "{out}");
}

/// **`\l` and `\dn` fall out of the same relations**, because they are the same joins over them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn psql_lists_the_database_and_the_schema() {
    let Some(psql) = psql() else { return };
    let port = serve().await;

    let out = run(&psql, port, "\\l");
    assert!(out.contains("List of databases"), "{out}");
    assert!(out.contains("UTF8"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");

    let out = run(&psql, port, "\\dn");
    assert!(out.contains("List of schemas"), "{out}");
    assert!(out.contains("public"), "{out}");
    // `\dn` hides `pg_catalog` with `nspname !~ '^pg_'`, so a regular expression that matched
    // nothing would list it.
    assert!(!out.contains("pg_catalog"), "{out}");
    assert!(!out.contains("ERROR"), "{out}");
}

/// **What this catalogue does not have is refused by the name of the relation it asked for.**
///
/// An empty answer would be a client told that a program has no functions and no roles, which is
/// a different claim from "this is a read model and has none". The gate is that the refusal
/// *names* what was missing: a catalogue that grew `pg_proc` full of no rows would pass a test
/// that only asserted `\df` printed nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn psql_is_refused_by_name_for_what_a_read_model_does_not_have() {
    let Some(psql) = psql() else { return };
    let port = serve().await;

    for (backslash, relation) in [
        ("\\df", "pg_proc"),
        ("\\du", "pg_roles"),
        ("\\di", "pg_index"),
    ] {
        let out = run(&psql, port, backslash);
        assert!(out.contains("ERROR"), "{backslash}:\n{out}");
        assert!(
            out.contains(relation),
            "{backslash} does not name {relation}:\n{out}"
        );
    }
}

/// **The catalogue is a set of tables, so `psql` can query it as one** — and it agrees with
/// `beck_columns`, which is the same schema said the other way.
///
/// Two descriptions of one schema that disagreed would mean one of them was built rather than
/// derived, and this is the assertion that would catch it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_catalogue_is_queryable_and_agrees_with_the_read_models_own() {
    let Some(psql) = psql() else { return };
    let port = serve().await;

    // `-t -A` is tuples only, unaligned: what comes back is the value and nothing else.
    let count = |sql: &str| -> String {
        let out = Command::new(&psql)
            .arg(format!(
                "host=127.0.0.1 port={port} user=nobody dbname=beck"
            ))
            .args(["--no-psqlrc", "-t", "-A", "-c", sql])
            .output()
            .expect("psql runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    // Every column of `todos`, counted through `pg_catalog` and through `beck_columns`.
    let through_catalogue = count(
        "select count(*) from pg_catalog.pg_attribute a \
         join pg_catalog.pg_class c on a.attrelid = c.oid where c.relname = 'todos'",
    );
    let through_beck = count("select count(*) from beck_columns where table_name = 'todos'");
    assert_eq!(through_catalogue, "4", "through pg_catalog");
    assert_eq!(through_beck, through_catalogue);

    // And an ordinary query still answers, which is what the catalogue is for.
    assert_eq!(count("select count(*) from todos"), "3");
}
