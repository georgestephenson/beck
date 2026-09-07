//! The read model: a program's maintained state as relations, and a small SQL over them.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.3 names this as one of
//! the four things the data tier owes:
//!
//! > Read models … one-shot queries and **pgwire access for the outside world**: `psql`, BI tools,
//! > DBeaver see materialized views as ordinary tables — the single cheapest trust-builder for
//! > adopting teams
//!
//! # What a read model is here, and what it is not
//!
//! §5.3's row also says "generated tables in the same Postgres", and that is **not** what this
//! builds. A read model is not a second copy of the state written on the append path; it is the
//! collection the fold already holds and the arrangement [`crate::engine`] already maintains,
//! *projected*. Three consequences, and they are the argument for it:
//!
//! * **A read model costs nothing per event.** Nothing is written, nothing is projected, and the
//!   sequencer is untouched — which is [`docs/23`](../../../../../docs/23-incremental-views-report.md)
//!   §23.9's rule ("who advances it: not the sequencer") applied to a second kind of reader rather
//!   than argued with.
//! * **It cannot disagree with the page.** A durable projection is a second code path, and a second
//!   code path over the same events is a thing that can drift. These rows are read from the same
//!   arrangement the view renders from, so the recompute oracle already covers them.
//! * **It is exactly as fresh as the query.** A query advances the dataflow to the log's head and
//!   then reads, so a `SELECT` issued after an ack sees that ack's event. There is no projection
//!   lag because there is no projection.
//!
//! What that costs is the one-transaction property [`67`](../../../../../docs/67-sqlite-report.md)
//! §67.1 held open: an append and its projection are still not one transaction, because there is
//! still no projection. §23.19 is the row-by-row list.
//!
//! # Where the tables come from
//!
//! | Table | Rows | Read from |
//! |---|---|---|
//! | a collection-valued field of the accumulator | its elements | the state value |
//! | the accumulator's remaining scalar fields | one | the state value |
//! | a declared signal that does not read the session | its elements, or one | the maintained node |
//!
//! The third row is the interesting one: **a read model is a view that does not depend on who is
//! asking**, which is the same cut §5.3 draws for arrangement sharing. A `per_session` signal is not
//! a table because a SQL client is not a session — it has no `Session` to be rendered for, and
//! inventing one would answer a question nobody asked.
//!
//! # What this is not
//!
//! It is not a query planner, and it does not become one by having a join. What [`parse`] accepts
//! is a documented subset — a scan with a `where`, an `order by` and a `limit`; an equi-join, inner
//! or left; a `group by` with `count`, `min`, `max` and `sum`; `distinct`; and `union` — with the
//! `from` list fixing a left-deep join order rather than a cost model choosing one. It exists so
//! that an outside tool can read what the program holds, which is what §5.3's row is for.
//!
//! It does have a **scalar expression language** ([`Expr`]) — `case`, a call, a cast, `in`, `is`,
//! `||`, and the four POSIX regular-expression operators — and that is a debt `pg_catalog` called
//! in rather than a change of mind: `psql` writes all of them into the queries it sends the
//! catalogue ([`crate::pg`]), and a catalogue that is a read model is read by this SQL or by
//! nothing. It computes no arithmetic, because nothing asks for any. A subquery is parsed and
//! answered only where it is **provably** row-independent (see [`Eval::subquery`]); a correlated
//! one is refused by name, as is `having`.
//!
//! **What the relational half is made of is not here either.** A join, a `group by` and a
//! `distinct` are compiled into a [`crate::plan::Plan`] by [`crate::query`] and run by
//! [`crate::engine`], so the operators answering them are [`crate::plan::Op::Join`],
//! [`crate::plan::Op::ArrangeBy`], [`crate::plan::Op::GroupBy`] and
//! [`crate::plan::Op::Distinct`] — the ones a program's own view compiles to
//! ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 9). This
//! module keeps the parser, the schema, the scan and the row-level `where`, `order by` and
//! `limit` that are the same either way.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::core::Value;
use crate::plan::{Agg, OpId, Plan};
use crate::split::Placed;
use crate::ty::{Ty, TyDecl};

// -------------------------------------------------------------------------------------------
// Types
// -------------------------------------------------------------------------------------------

/// The four SQL types a Beck scalar maps onto.
///
/// Deliberately four. Every one of them is a type OID a Postgres client already knows, so a driver
/// never has to ask the catalogue what it just received — which matters more than breadth here,
/// because there is no catalogue to ask ([`Schema::CATALOGUE`] is what stands in for one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlTy {
    Boolean,
    Bigint,
    Double,
    Text,
}

impl SqlTy {
    /// The Postgres type OID, as it goes on the wire in a `RowDescription`.
    pub fn oid(self) -> u32 {
        match self {
            SqlTy::Boolean => 16,
            SqlTy::Bigint => 20,
            SqlTy::Text => 25,
            SqlTy::Double => 701,
        }
    }

    /// The width a fixed-size type has, or -1 for a variable one.
    pub fn width(self) -> i16 {
        match self {
            SqlTy::Boolean => 1,
            SqlTy::Bigint | SqlTy::Double => 8,
            SqlTy::Text => -1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SqlTy::Boolean => "boolean",
            SqlTy::Bigint => "bigint",
            SqlTy::Double => "double precision",
            SqlTy::Text => "text",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    pub name: Arc<str>,
    pub ty: SqlTy,
    /// Only an `Option[T]` field is nullable. Beck has no null, so this is the one place one comes
    /// from — and a column that is not an `Option` never holds one.
    pub nullable: bool,
}

/// Where a table's rows are read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A path of field names from the accumulator to a collection, or to the accumulator itself.
    ///
    /// Read from the state value the fold produced, not from an arrangement: a base table's rows
    /// *are* the fold's collection, and a scan is `O(rows)` in any database.
    State(Vec<Arc<str>>),
    /// A plan operator that does not read the session, read from the maintained dataflow.
    ///
    /// This is the one that earns the engine its keep: the rows of a derived table are whatever the
    /// arrangement holds, and the arrangement was maintained for the page.
    View(OpId),
    /// The schema describing itself.
    Catalogue,
    /// One relation of the emulated `pg_catalog`, built from the schema by [`crate::pg`].
    Pg(crate::pg::Rel),
}

/// How many rows a table can have, which is a fact about its shape rather than about its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    /// A collection: as many rows as it has elements.
    Many,
    /// A record or a scalar: exactly one row, always.
    One,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub name: Arc<str>,
    pub columns: Vec<Column>,
    pub source: Source,
    pub cardinality: Cardinality,
    /// The Beck type one row stands for, for `beck explain sql` to print.
    pub element: Arc<str>,
}

impl Table {
    pub fn column(&self, name: &str) -> Option<(usize, &Column)> {
        self.columns
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.as_ref() == name)
    }

    /// One value as one row's worth of *values*, one per column.
    ///
    /// This is the rule that says what a column **is**, and it is shared rather than restated:
    /// [`Table::row`] builds a scan's cells from it and [`crate::query`] normalises a table's rows
    /// with it before compiling them into a plan, so a `select` that scans and a `select` that
    /// joins cannot disagree about which part of an element a column names.
    ///
    /// [`Value::Unit`] stands for a field the value does not have, which becomes NULL in every
    /// column type.
    pub fn row_values(&self, v: &Value) -> Vec<Value> {
        match unwrap(v) {
            // A record: one column per field, by name.
            Value::Data(d) if d.variant.is_none() && !d.fields.is_empty() => self
                .columns
                .iter()
                .map(|c| match d.fields.get(&c.name) {
                    Some(f) => column_value(f),
                    None => Value::Unit,
                })
                .collect(),
            // A scalar, or anything else: the single column this table then has.
            other => self.columns.iter().map(|_| column_value(other)).collect(),
        }
    }

    /// One value as one row, coerced to the columns this table declares.
    ///
    /// Coerced rather than trusted: the column types come from the *declared* type and the value
    /// comes from a running program, so a value that does not fit its column becomes NULL rather
    /// than a wrongly-encoded field on the wire. Nothing in the corpus reaches that branch; the
    /// branch is there because "cannot happen" is not a wire format.
    pub fn row(&self, v: &Value) -> Vec<Cell> {
        self.row_values(v)
            .iter()
            .zip(&self.columns)
            .map(|(v, c)| cell_of(v, c))
            .collect()
    }
}

/// A value in one column, or SQL NULL.
pub type Cell = Option<Datum>;

#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Boolean(bool),
    Bigint(i64),
    Double(f64),
    Text(String),
}

impl Datum {
    pub fn ty(&self) -> SqlTy {
        match self {
            Datum::Boolean(_) => SqlTy::Boolean,
            Datum::Bigint(_) => SqlTy::Bigint,
            Datum::Double(_) => SqlTy::Double,
            Datum::Text(_) => SqlTy::Text,
        }
    }

    /// The text form, which is both what the simple query protocol sends and what `ORDER BY`
    /// compares for a text column.
    pub fn text(&self) -> String {
        match self {
            Datum::Boolean(b) => if *b { "t" } else { "f" }.to_string(),
            Datum::Bigint(i) => i.to_string(),
            // Postgres prints a float with enough digits to round-trip, and so does Rust's `{}`
            // for `f64` — except that Rust drops the fractional part of a whole number, where
            // Postgres keeps none either. `1` and `1` agree; nothing here needs `1.0`.
            Datum::Double(f) => f.to_string(),
            Datum::Text(s) => s.clone(),
        }
    }
}

/// One Beck value in one column, or NULL.
pub fn cell_of(v: &Value, c: &Column) -> Cell {
    let v = unwrap(v);
    // `None` is the only null this language has, and it is only reachable through an `Option`
    // column — a non-nullable column holding one would be a value that does not fit its type.
    if let Value::Data(d) = v {
        if d.variant.as_deref() == Some("None") {
            return None;
        }
        if d.variant.as_deref() == Some("Some") {
            return match d.fields.values().next() {
                Some(inner) => cell_of(inner, c),
                None => None,
            };
        }
    }
    match (c.ty, v) {
        (SqlTy::Boolean, Value::Bool(b)) => Some(Datum::Boolean(*b)),
        (SqlTy::Bigint, Value::Int(i)) => Some(Datum::Bigint(*i)),
        (SqlTy::Double, _) => v.as_f64().map(Datum::Double),
        (SqlTy::Text, Value::Str(s)) => Some(Datum::Text(s.to_string())),
        // A composite column — a list, a map, a nested record, a union variant. JSON is the wire
        // form this language already has for a value a browser reads (`Value::to_json`), so it is
        // the one a SQL client gets too rather than a second rendering invented here.
        (SqlTy::Text, other) => Some(Datum::Text(match other {
            Value::Unit => return None,
            _ => serde_json::to_string(&other.to_json()).unwrap_or_else(|_| other.display()),
        })),
        _ => None,
    }
}

/// One field as the value its column *is*: a newtype seen through, an `Option` flattened, and
/// [`Value::Unit`] for the SQL NULL a `None` becomes.
///
/// The point is that two things agree. [`cell_of`] already saw through both when it built the cell
/// a client is shown, so a `Str` behind a newtype has always **displayed** as its payload; what a
/// join compares is the [`Value`] itself, and a key that compared `Id("p1")` where the column shows
/// `p1` would answer no rows for two columns a person can see are equal. Normalising here rather
/// than at the comparison is what makes that one rule instead of two.
fn column_value(v: &Value) -> Value {
    let v = unwrap(v);
    if let Value::Data(d) = v {
        match d.variant.as_deref() {
            Some("None") => return Value::Unit,
            Some("Some") => {
                return match d.fields.values().next() {
                    Some(inner) => column_value(inner),
                    None => Value::Unit,
                }
            }
            _ => {}
        }
    }
    v.clone()
}

/// See through a newtype, which at run time is a one-field record with no variant.
fn unwrap(v: &Value) -> &Value {
    match v {
        Value::Data(d) if d.variant.is_none() && d.fields.len() == 1 => {
            match d.fields.values().next() {
                Some(inner) => unwrap(inner),
                None => v,
            }
        }
        _ => v,
    }
}

// -------------------------------------------------------------------------------------------
// The schema
// -------------------------------------------------------------------------------------------

/// Every table a program's read model has.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    /// The program's own read models, in the `public` namespace — and [`Schema::CATALOGUE`].
    pub tables: Vec<Table>,
    /// `pg_catalog`, in the `pg_catalog` namespace: the same schema under the names an outside
    /// tool already knows ([`crate::pg`]).
    ///
    /// A separate list rather than more entries in `tables`, because these describe the read
    /// models and are not read models themselves: `select * from beck_columns` and
    /// `beck explain sql` are about what the *program* holds, and a catalogue that listed itself
    /// there would answer a question nobody asked.
    pub pg: Vec<Table>,
}

impl Schema {
    /// The name of the catalogue table, which says what a table is *derived from* — the question
    /// `pg_catalog` has no column for, because PostgreSQL has no such thing to describe.
    pub const CATALOGUE: &'static str = "beck_columns";

    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name.as_ref() == name)
    }

    /// A relation named by a `from` entry: a read model, or one of `pg_catalog`'s.
    ///
    /// An unqualified name is a read model first, so a program with a table called `pg_class`
    /// gets its own. A name qualified with `pg_catalog` is only ever the catalogue's, and one
    /// qualified with anything else is refused rather than searched for: two namespaces is all
    /// there is, and a client that asked a third a question deserves to be told so.
    pub fn relation(&self, namespace: Option<&str>, name: &str) -> Result<&Table, SqlError> {
        match namespace {
            None => self
                .table(name)
                .or_else(|| self.pg.iter().find(|t| t.name.as_ref() == name))
                .ok_or_else(|| {
                    SqlError::no_table(format!(
                        "there is no read model called \"{name}\". `select * from {}` lists what \
                         there is",
                        Schema::CATALOGUE
                    ))
                }),
            Some(crate::pg::CATALOG) => self
                .pg
                .iter()
                .find(|t| t.name.as_ref() == name)
                .ok_or_else(|| SqlError::no_table(crate::pg::Rel::missing(name))),
            Some(crate::pg::PUBLIC) => self.table(name).ok_or_else(|| {
                SqlError::no_table(format!(
                    "there is no read model called \"{name}\". `select * from {}` lists what \
                     there is",
                    Schema::CATALOGUE
                ))
            }),
            Some(other) => Err(SqlError::no_table(format!(
                "there is no schema called \"{other}\" here. A program's read models are in \
                 \"{}\" and the catalogue that describes them is in \"{}\"",
                crate::pg::PUBLIC,
                crate::pg::CATALOG
            ))),
        }
    }

    /// The rows of a table this module builds rather than reads from a running program.
    ///
    /// One place, so a scan and a join get the same rows: [`Rows::scan`] never sees one of these,
    /// and a reader with no program behind it can still answer the catalogue.
    pub fn builtin_rows(&self, t: &Table) -> Option<Vec<Value>> {
        match &t.source {
            Source::Catalogue => Some(self.catalogue_values()),
            Source::Pg(rel) => Some(crate::pg::rows(*rel, self)),
            _ => None,
        }
    }

    /// Derive the read model of a sliced program.
    pub fn of(placed: &Placed, plan: &Plan) -> Schema {
        let types = &placed.program.types;
        let mut tables: Vec<Table> = Vec::new();
        let mut taken: BTreeSet<Arc<str>> = BTreeSet::new();

        for role in &placed.roles.states {
            let base: Vec<Arc<str>> = role.field.iter().cloned().collect();
            let ty = resolve(&role.ty, types);
            match collection_elem(&ty, types) {
                // The whole accumulator is a collection: one table, named after the fold.
                Some(elem) => push(
                    &mut tables,
                    &mut taken,
                    table(
                        role.name.clone(),
                        &elem,
                        types,
                        Source::State(base),
                        Cardinality::Many,
                    ),
                ),
                None => {
                    let fields = model_fields(&ty, types).unwrap_or_default();
                    let mut scalars: Vec<(Arc<str>, Ty)> = Vec::new();
                    for (name, fty) in fields {
                        let fty = resolve(&fty, types);
                        match collection_elem(&fty, types) {
                            Some(elem) => {
                                let mut path = base.clone();
                                path.push(name.clone());
                                push(
                                    &mut tables,
                                    &mut taken,
                                    table(
                                        name,
                                        &elem,
                                        types,
                                        Source::State(path),
                                        Cardinality::Many,
                                    ),
                                );
                            }
                            None => scalars.push((name, fty)),
                        }
                    }
                    // Whatever is left of the accumulator is a singleton: `State(charged=0,
                    // refused=0)` is one row of two columns, which is the relational shape of a
                    // state that is not a collection. A fold whose every field is a collection
                    // leaves nothing here and gets no such table.
                    if !scalars.is_empty() {
                        push(
                            &mut tables,
                            &mut taken,
                            Table {
                                name: role.name.clone(),
                                columns: scalars
                                    .iter()
                                    .map(|(n, t)| column(n.clone(), t, types))
                                    .collect(),
                                source: Source::State(base.clone()),
                                cardinality: Cardinality::One,
                                element: Arc::from(ty.to_string()),
                            },
                        );
                    }
                }
            }
        }

        // Derived signals, which is where a maintained arrangement becomes a table. The page is
        // excluded by its type rather than by its name: `Html` is not a relation.
        let by_op: BTreeMap<&str, OpId> = plan
            .signals
            .iter()
            .map(|(n, id)| (n.as_ref(), *id))
            .collect();
        let folds: BTreeSet<&str> = placed
            .roles
            .states
            .iter()
            .map(|s| s.name.as_ref())
            .collect();
        for (name, &sig) in &placed.graph.by_name {
            let Some(&op) = by_op.get(name.as_ref()) else {
                continue;
            };
            if plan.nodes[op].per_session {
                continue;
            }
            // The accumulator is not a table: its collections and its scalars are, and they are
            // above. A `Signal[State]` here would be one row whose collection fields are rendered
            // as JSON — the same data, in the shape nothing can query.
            if folds.contains(name.as_ref()) {
                continue;
            }
            let ty = resolve(
                &crate::signal::signal_elem(&placed.graph.node(sig).ty),
                types,
            );
            let t = match collection_elem(&ty, types) {
                Some(elem) => table(
                    name.clone(),
                    &elem,
                    types,
                    Source::View(op),
                    Cardinality::Many,
                ),
                // A record or a scalar signal is one row. A `Signal[State]` is neither useful nor
                // harmful here — its fields are already base tables — so a name already taken
                // wins, which `push` decides.
                None if model_fields(&ty, types).is_some() || scalar(&ty).is_some() => {
                    table(name.clone(), &ty, types, Source::View(op), Cardinality::One)
                }
                None => continue,
            };
            push(&mut tables, &mut taken, t);
        }

        tables.sort_by(|a, b| a.name.cmp(&b.name));
        tables.push(Table {
            name: Arc::from(Schema::CATALOGUE),
            columns: [
                "table_name",
                "column_name",
                "data_type",
                "nullable",
                "position",
            ]
            .iter()
            .enumerate()
            .map(|(i, n)| Column {
                name: Arc::from(*n),
                ty: if i == 4 {
                    SqlTy::Bigint
                } else if i == 3 {
                    SqlTy::Boolean
                } else {
                    SqlTy::Text
                },
                nullable: false,
            })
            .collect(),
            source: Source::Catalogue,
            cardinality: Cardinality::Many,
            element: Arc::from("Column"),
        });
        Schema {
            tables,
            pg: crate::pg::relations(),
        }
    }

    /// The catalogue's own rows as **values**, which is what a query that joins it reads.
    ///
    /// It is built on demand: the catalogue is a handful of rows describing a schema that cannot
    /// change while a process is running, and a cache would be a second copy of it to keep true.
    pub fn catalogue_values(&self) -> Vec<Value> {
        let mut rows = Vec::new();
        for t in &self.tables {
            for (i, c) in t.columns.iter().enumerate() {
                rows.push(Value::record(
                    "Column",
                    None,
                    [
                        ("table_name", Value::text(t.name.to_string())),
                        ("column_name", Value::text(c.name.to_string())),
                        ("data_type", Value::text(c.ty.name().to_string())),
                        ("nullable", Value::Bool(c.nullable)),
                        ("position", Value::Int(i as i64 + 1)),
                    ],
                ));
            }
        }
        rows
    }

    /// The schema as `CREATE TABLE` statements, for `beck explain sql`.
    ///
    /// Nothing executes this — there is no database to execute it against, and saying so is the
    /// point. It is the shape a person needs in order to write the query they were going to write.
    pub fn ddl(&self) -> String {
        let mut out = String::new();
        for t in &self.tables {
            let what = match &t.source {
                Source::State(path) if path.is_empty() => "the accumulator".to_string(),
                Source::State(path) => format!("state.{}", join(path)),
                Source::View(op) => format!("plan operator {op}, maintained and shared"),
                Source::Catalogue => "this schema".to_string(),
                Source::Pg(rel) => format!("this schema, as {}", rel.name()),
            };
            let _ = writeln!(
                out,
                "-- {} of {}, from {what}",
                match t.cardinality {
                    Cardinality::Many => "the elements",
                    Cardinality::One => "one row",
                },
                t.element
            );
            let _ = writeln!(out, "create table {} (", quote_ident(&t.name));
            let n = t.columns.len();
            for (i, c) in t.columns.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "    {:<20} {}{}{}",
                    quote_ident(&c.name),
                    c.ty.name(),
                    if c.nullable { "" } else { " not null" },
                    if i + 1 == n { "" } else { "," }
                );
            }
            let _ = writeln!(out, ");");
        }
        out
    }
}

/// The words this SQL reads as syntax rather than as a name.
///
/// A Beck field may be called `distinct` or `order` — `corpus/17-derived.beck` has a `Summary` with
/// a field called `distinct` — and a column whose name has to be quoted is a column a person must
/// be *told* to quote. So the DDL quotes it, which is the one place they will see it written down.
const RESERVED: &[&str] = &[
    "abort", "and", "as", "asc", "begin", "by", "commit", "count", "cross", "desc", "discard",
    "distinct", "end", "false", "from", "full", "group", "having", "inner", "is", "join", "left",
    "limit", "natural", "not", "null", "offset", "on", "or", "order", "outer", "right", "rollback",
    "select", "set", "start", "table", "true", "where",
];

/// A name as it has to be written in this SQL: bare when it can be, quoted when it cannot.
pub fn quote_ident(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit());
    if plain && !RESERVED.contains(&name) {
        return name.to_string();
    }
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn join(path: &[Arc<str>]) -> String {
    path.iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// Add a table unless its name is taken. First wins, and the order is base tables then derived
/// ones, so a signal named after the fold does not shadow the fold's own collections.
fn push(tables: &mut Vec<Table>, taken: &mut BTreeSet<Arc<str>>, t: Table) {
    if taken.insert(t.name.clone()) {
        tables.push(t);
    }
}

fn table(
    name: Arc<str>,
    elem: &Ty,
    types: &BTreeMap<Arc<str>, TyDecl>,
    source: Source,
    cardinality: Cardinality,
) -> Table {
    let elem = resolve(elem, types);
    let columns = match model_fields(&elem, types) {
        Some(fields) => fields
            .into_iter()
            .map(|(n, t)| column(n, &t, types))
            .collect(),
        // A collection of scalars, or of anything else: one column, and the row is the element.
        None => vec![column(Arc::from("value"), &elem, types)],
    };
    Table {
        name,
        columns,
        source,
        cardinality,
        element: Arc::from(elem.to_string()),
    }
}

fn column(name: Arc<str>, ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Column {
    let (ty, nullable) = sql_ty(ty, types);
    Column { name, ty, nullable }
}

/// The SQL type of a Beck type, and whether it can be null.
fn sql_ty(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> (SqlTy, bool) {
    let ty = resolve(ty, types);
    if let Ty::Con(n, args) = &ty {
        if n.as_ref() == Ty::OPTION && args.len() == 1 {
            return (sql_ty(&args[0], types).0, true);
        }
    }
    (scalar(&ty).unwrap_or(SqlTy::Text), false)
}

/// The SQL type of a Beck *scalar*, or nothing if it is not one.
fn scalar(ty: &Ty) -> Option<SqlTy> {
    match ty {
        Ty::Con(n, args) if args.is_empty() => match n.as_ref() {
            Ty::INT => Some(SqlTy::Bigint),
            Ty::FLOAT => Some(SqlTy::Double),
            Ty::BOOL => Some(SqlTy::Boolean),
            Ty::STR => Some(SqlTy::Text),
            _ => None,
        },
        _ => None,
    }
}

/// See through aliases and newtypes, which are the two declarations that mean "this type, spelled
/// differently". A `model` and a `union` are not resolved: they are the thing itself.
fn resolve(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Ty {
    let mut ty = ty.clone();
    // Bounded because a `type` alias can be recursive in a program that did not compile, and this
    // runs over whatever it is handed.
    for _ in 0..16 {
        let Ty::Con(name, args) = &ty else { return ty };
        let next = match types.get(name) {
            Some(TyDecl::Newtype { params, inner, .. }) => substitute(inner, params, args),
            Some(TyDecl::Alias { params, ty: t, .. }) => substitute(t, params, args),
            _ => return ty,
        };
        ty = next;
    }
    ty
}

fn substitute(ty: &Ty, params: &[Arc<str>], args: &[Ty]) -> Ty {
    if params.is_empty() {
        return ty.clone();
    }
    match ty {
        Ty::Con(n, inner) if inner.is_empty() => match params.iter().position(|p| p == n) {
            Some(i) if i < args.len() => args[i].clone(),
            _ => ty.clone(),
        },
        Ty::Con(n, inner) => Ty::Con(
            n.clone(),
            inner.iter().map(|t| substitute(t, params, args)).collect(),
        ),
        _ => ty.clone(),
    }
}

/// The element type of a `list[T]` or a `Map[K, V]`, or nothing.
fn collection_elem(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Option<Ty> {
    match resolve(ty, types) {
        Ty::Con(n, args) if n.as_ref() == Ty::LIST && args.len() == 1 => Some(args[0].clone()),
        Ty::Con(n, args) if n.as_ref() == Ty::MAP && args.len() == 2 => Some(args[1].clone()),
        _ => None,
    }
}

/// A `model`'s fields, in the order they were written.
///
/// Declared order rather than name order, which is the one place this disagrees with the run-time
/// representation ([`crate::core::Fields`] sorts by name, and `docs/46` §46.6 pinned that). Columns
/// are read by name, so the disagreement costs nothing and the person reading `select *` gets their
/// own declaration back.
fn model_fields(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Option<Vec<(Arc<str>, Ty)>> {
    let Ty::Con(name, args) = resolve(ty, types) else {
        return None;
    };
    match types.get(&name) {
        Some(TyDecl::Model { params, fields, .. }) if !fields.is_empty() => Some(
            fields
                .iter()
                .map(|(n, t)| (n.clone(), substitute(t, params, &args)))
                .collect(),
        ),
        _ => None,
    }
}

/// The elements of a collection value, in the order it holds them.
pub fn elements(v: &Value) -> Vec<Value> {
    match v {
        Value::List(xs) => xs.to_vec(),
        Value::Map(m) => m.iter().map(|(_, v)| v.clone()).collect(),
        other => vec![other.clone()],
    }
}

/// Follow a path of field names into a value.
pub fn at_path(v: &Value, path: &[Arc<str>]) -> Option<Value> {
    let mut cur = v.clone();
    for step in path {
        let Value::Data(d) = &cur else { return None };
        cur = d.fields.get(step)?.clone();
    }
    Some(cur)
}

// -------------------------------------------------------------------------------------------
// The query
// -------------------------------------------------------------------------------------------

/// What a query asks for.
///
/// The **relational** half is several tables joined by an equality, a `group by` with its
/// aggregates, and `distinct`. None of those is interpreted here — [`crate::query`] compiles them
/// into a [`crate::plan::Plan`] and [`crate::engine`] runs it, so the join in a `select` and the
/// join in a `for` loop are one operator
/// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 9).
///
/// The **scalar** half is [`Expr`], and it is a per-row expression language rather than the
/// [`04`](../../../../../docs/04-compiler-architecture.md) §4.2 `Query` sub-language: it computes
/// nothing a program could not, and it exists because `psql` writes `case`, a function call and a
/// regular expression into the queries it sends the catalogue ([`crate::pg`]).
#[derive(Clone, Debug)]
pub struct Select {
    /// `select distinct` — the algebra's δ, and [`crate::plan::Op::Distinct`] is what answers it.
    pub distinct: bool,
    pub items: Vec<Item>,
    /// The tables, in the order they were written. Empty for `select 1`; one for a scan; more when
    /// the query joins, and every entry after the first carries the equality that joins it.
    pub from: Vec<From>,
    /// The `where`, as the conjunction it is: every term must be true of a row.
    ///
    /// A conjunction rather than one expression because that is the unit a term can be *pushed
    /// into a scan* as — [`crate::query`] splits these by the table each names — and `and` is the
    /// operator that makes splitting them sound.
    pub filter: Vec<Expr>,
    /// `group by` — the columns whose distinct values are the output's rows.
    pub group: Vec<Name>,
    /// `order by`, in the order the keys were written; the first that distinguishes two rows
    /// decides.
    pub order: Vec<Order>,
    pub limit: Option<usize>,
    pub offset: usize,
}

/// One `order by` key.
#[derive(Clone, Debug)]
pub struct Order {
    pub by: OrderBy,
    pub asc: bool,
}

/// What an `order by` key names.
#[derive(Clone, Debug)]
pub enum OrderBy {
    /// `order by 2` — the second column of the select list, one-based, as SQL numbers them.
    Ordinal(usize),
    /// An expression over the row, which for `order by c` is the column `c`. A name that matches
    /// an output column's name is that output column, and only then a column of a table: SQL
    /// resolves an `order by` against the select list first, and a query may order by something it
    /// computed.
    Expr(Expr),
}

/// One entry of the `from` list: a table, what this query calls it, and what joins it.
#[derive(Clone, Debug)]
pub struct From {
    /// The schema qualifying the table's name, if it was written: `pg_catalog.pg_class`.
    pub namespace: Option<String>,
    /// The table's name in the schema.
    pub table: String,
    /// The name a qualified column reference uses — the alias, or the table's own name.
    pub alias: String,
    /// The `on` equalities, as (this table's column, an earlier table's column). Empty for the
    /// first entry, which nothing joins to, and for a comma join, whose equality is in the
    /// `where`.
    pub on: Vec<(Name, Name)>,
    /// `left join`: a row of the table before this one survives with nulls where this one has no
    /// match. The catalogue is full of them — `pg_class left join pg_am` is how `psql` asks for a
    /// table's access method without losing the tables that have none.
    pub left: bool,
    /// A `from` item that is a function call rather than a relation. Parsed so that a query
    /// carrying one in a branch nothing evaluates is not refused for it, and refused by name if
    /// anything asks it for a row.
    pub function: Option<String>,
}

/// A column reference, qualified by a table's name in this query or not.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name {
    pub table: Option<String>,
    pub column: String,
}

impl Name {
    pub fn bare(column: impl Into<String>) -> Name {
        Name {
            table: None,
            column: column.into(),
        }
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.table {
            Some(t) => write!(f, "{t}.{}", self.column),
            None => f.write_str(&self.column),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    /// `*`, or `t.*`.
    All(Option<String>),
    Column(Name, Option<String>),
    Count(Option<String>),
    /// `min(c)`, `max(c)` or `sum(c)` — the three aggregates whose answer depends on what the rows
    /// say. [`crate::plan::Op::GroupBy`] is what maintains each, and `count` is not one of them
    /// because a count needs nothing of the row at all.
    Aggregate(Agg, Name, Option<String>),
    Literal(Datum, Option<String>),
    /// Anything else a select list can hold — a `case`, a call, a comparison, a cast.
    ///
    /// Separate from [`Item::Column`] and [`Item::Literal`] rather than subsuming them, because
    /// those two are what [`crate::query`] compiles into the plan for a `group by` and a
    /// `distinct`: an expression there would be an expression *inside* an operator, and this
    /// evaluates them over the rows an operator produced.
    Expr(Expr, Option<String>),
}

impl Item {
    /// Whether this item is an aggregate, which is what decides that a query groups.
    pub fn aggregates(&self) -> bool {
        matches!(self, Item::Count(_) | Item::Aggregate(..))
    }
}

/// A scalar expression, evaluated over one row.
///
/// Every variant here is something a catalogue query writes. What is *not* here is arithmetic,
/// because nothing asks for it: a read model computes its numbers in the program.
#[derive(Clone, Debug)]
pub enum Expr {
    Column(Name),
    /// A literal, or `null` — which is why this is a [`Cell`] rather than a [`Datum`].
    Literal(Cell),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    /// `a = b`, and the other five comparisons. Three-valued: a comparison against NULL is
    /// unknown, and unknown is not true.
    Cmp(Box<Expr>, CmpOp, Box<Expr>),
    /// `a is null` / `a is not null`, and `a is true` / `a is not false`.
    Is {
        value: Box<Expr>,
        /// `None` for `is null`.
        to: Option<bool>,
        negated: bool,
    },
    /// `c in ('r', 'v')` — the form `psql` narrows `relkind` with.
    In {
        value: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    /// `~`, `!~`, `~*`, `!~*` — a POSIX regular expression match, run by a simulation over a set
    /// of states rather than by backtracking, because the pattern arrives from a client.
    Match {
        value: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
        insensitive: bool,
    },
    /// `case … when … then … else … end`, in both SQL's forms.
    ///
    /// **Lazy**, as SQL specifies: the arms after the one that matched are not evaluated. That is
    /// load-bearing here rather than an optimisation — `psql` writes `case when c.reloftype = 0
    /// then '' else c.reloftype::regtype::text end`, and the branch this catalogue cannot compute
    /// is the branch no row reaches.
    Case {
        operand: Option<Box<Expr>>,
        arms: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    /// A function call. [`crate::pg::call`] is what answers one, and a name it does not know is
    /// refused *by name* rather than answered NULL.
    Call {
        name: String,
        args: Vec<Expr>,
    },
    /// `x::text`. A cast to a type this has is the value; to anything else it is refused by name,
    /// which is why the `case` above has to be lazy.
    Cast {
        value: Box<Expr>,
        ty: String,
    },
    /// `a || b`, string concatenation.
    Concat(Box<Expr>, Box<Expr>),
    /// A scalar subquery — `(select … from …)`.
    ///
    /// The `id` distinguishes two subqueries in one statement, so the answer to each can be
    /// computed once rather than once per row: see [`Eval::subquery`] for why it is the same for
    /// every row.
    Subquery {
        id: usize,
        select: Box<Select>,
    },
    /// `array(select …)` and `any(x)`: parsed, and refused by name if a row ever asks. Both appear
    /// in `psql`'s catalogue queries inside branches that no row reaches, and a query refused at
    /// parse time for a branch it never takes is a `\d` that does not work.
    Array {
        id: usize,
        select: Box<Select>,
    },
    Any(Box<Expr>),
    /// `x[i]`, an array subscript. Parsed for [`Expr::Array`]'s reason and refused for the same
    /// one.
    Subscript(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Every column this expression names, including those in a subquery it carries.
    ///
    /// [`crate::query`] uses it to decide which table a `where` term narrows, so a term that names
    /// a column inside a subquery counts as naming it: pushing such a term into one table's scan
    /// would evaluate the subquery against the wrong rows.
    pub fn names(&self, out: &mut Vec<Name>) {
        match self {
            Expr::Column(n) => out.push(n.clone()),
            Expr::Literal(_) => {}
            Expr::And(xs) | Expr::Or(xs) => xs.iter().for_each(|x| x.names(out)),
            Expr::Not(x) | Expr::Any(x) => x.names(out),
            Expr::Cmp(a, _, b) | Expr::Concat(a, b) | Expr::Subscript(a, b) => {
                a.names(out);
                b.names(out);
            }
            Expr::Is { value, .. } | Expr::Cast { value, .. } => value.names(out),
            Expr::In { value, list, .. } => {
                value.names(out);
                list.iter().for_each(|x| x.names(out));
            }
            Expr::Match { value, pattern, .. } => {
                value.names(out);
                pattern.names(out);
            }
            Expr::Case {
                operand,
                arms,
                otherwise,
            } => {
                if let Some(o) = operand {
                    o.names(out);
                }
                for (w, t) in arms {
                    w.names(out);
                    t.names(out);
                }
                if let Some(e) = otherwise {
                    e.names(out);
                }
            }
            Expr::Call { args, .. } => args.iter().for_each(|x| x.names(out)),
            Expr::Subquery { select, .. } | Expr::Array { select, .. } => {
                for item in &select.items {
                    match item {
                        Item::Column(n, _) => out.push(n.clone()),
                        Item::Aggregate(_, n, _) => out.push(n.clone()),
                        Item::Expr(e, _) => e.names(out),
                        _ => {}
                    }
                }
                select.filter.iter().for_each(|x| x.names(out));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn holds(self, o: std::cmp::Ordering) -> bool {
        match self {
            CmpOp::Eq => o.is_eq(),
            CmpOp::Ne => o.is_ne(),
            CmpOp::Lt => o.is_lt(),
            CmpOp::Le => o.is_le(),
            CmpOp::Gt => o.is_gt(),
            CmpOp::Ge => o.is_ge(),
        }
    }
}

/// A statement, which is a `select` or one of the two things a client says before it asks for
/// anything.
#[derive(Clone, Debug)]
pub enum Stmt {
    Select(Select),
    /// Two or more selects, `union`ed. The branches answer in turn and the answers are stacked;
    /// without `all` the stack is deduplicated, which is what `union` means.
    ///
    /// It is here because `psql` asks for a table's publications as a union of three queries over
    /// relations a read model has none of, and one of the three is what tells it there are none.
    Union {
        branches: Vec<Select>,
        all: bool,
        order: Vec<Order>,
        limit: Option<usize>,
        offset: usize,
    },
    /// `SET …` and `BEGIN`/`COMMIT`/`ROLLBACK`: acknowledged and ignored. A read model has nothing
    /// to set and nothing to roll back, and a driver that opens a transaction out of habit should
    /// not be refused for it.
    Ignored(&'static str),
}

/// What a query answered.
pub struct Answer {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    /// The `CommandComplete` tag.
    pub tag: String,
}

/// Why a query could not be answered. The message reaches the client verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlError {
    pub message: String,
    /// The five-character SQLSTATE. A driver reads this; a person reads the message.
    pub code: &'static str,
}

impl SqlError {
    pub fn syntax(m: impl Into<String>) -> SqlError {
        SqlError {
            message: m.into(),
            code: "42601",
        }
    }
    pub fn no_table(m: impl Into<String>) -> SqlError {
        SqlError {
            message: m.into(),
            code: "42P01",
        }
    }
    pub fn no_column(m: impl Into<String>) -> SqlError {
        SqlError {
            message: m.into(),
            code: "42703",
        }
    }
    pub fn unsupported(m: impl Into<String>) -> SqlError {
        SqlError {
            message: m.into(),
            code: "0A000",
        }
    }
}

impl std::fmt::Display for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SqlError {}

/// Where a table's rows come from. Implemented by whoever holds the running program.
pub trait Rows {
    /// Every row of one table, in the order the collection holds them.
    fn scan(&self, table: &Table) -> Result<Vec<Value>, SqlError>;

    /// **How many rows there are, if that can be answered without building them.**
    ///
    /// [`23`](../../../../../docs/23-incremental-views-report.md) §23.19: "`count(*)` without
    /// scanning — **not built**. The plan's `list_len` is ±1 per delta; the SQL count is over the
    /// rows it scanned." This is the seam that closes it. A maintained arrangement and a `Map` in
    /// the accumulator each know their size, so a query whose whole answer is a number should not
    /// clone every value and build a `Cell` for every column of every one.
    ///
    /// `None` means "not without a scan", and the caller falls back — so an implementation that
    /// does not override this is correct and merely as slow as it was. That default is the point:
    /// the seam cannot make a reader wrong, only faster.
    fn count(&self, table: &Table) -> Result<Option<u64>, SqlError> {
        let _ = table;
        Ok(None)
    }

    /// **The backend a relational query's operators are prepared against.**
    ///
    /// A `join`, a `group by` and a `distinct` are compiled into a [`crate::plan::Plan`] and run by
    /// [`crate::engine`] rather than interpreted here
    /// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 9), and
    /// preparing a plan means turning its per-element functions into
    /// [`crate::backend::Callable`]s — which is a backend's job and not this module's.
    ///
    /// `None` is the honest answer for a reader with no executor behind it: those three queries are
    /// refused with a message saying so, and every other query is answered exactly as before. The
    /// default is `None` for [`Rows::count`]'s reason — a seam may make a reader faster or narrower,
    /// never wrong.
    fn backend(&self) -> Option<&dyn crate::backend::Backend> {
        None
    }
}

/// One column of the rows a query produced, and the table name a reference may qualify it with.
///
/// A `Column` is what goes on the wire; this is what a `where`, an `order by` and a select list
/// resolve a name against. The two are separate because a joined row has columns from several
/// tables and two of them may share a name — which is a question about the *query* rather than
/// about the wire, where a column is a name and a type OID and nothing else.
#[derive(Clone, Debug)]
pub struct Field {
    pub column: Column,
    /// The table this came from, as this query knows it. `None` for a computed column — an
    /// aggregate, a literal, or anything given an alias, none of which a qualified name may reach.
    pub of: Option<Arc<str>>,
}

impl Field {
    /// A base table's own columns, which is what a query over one table resolves against.
    pub fn of_table(t: &Table) -> Vec<Field> {
        Field::of_table_as(t, t.name.clone())
    }

    /// The same, under the name the query calls the table.
    ///
    /// A qualified reference names the *alias* — `from pg_attribute a … where a.attnum > 0` — so a
    /// query over one table has to resolve names against the alias exactly as a join does. One
    /// rule, in both places, because a name that resolved one way in a scan and another in a join
    /// would be two.
    pub fn of_table_as(t: &Table, alias: Arc<str>) -> Vec<Field> {
        t.columns
            .iter()
            .map(|c| Field {
                column: c.clone(),
                of: Some(alias.clone()),
            })
            .collect()
    }
}

impl Schema {
    /// Parse and run one statement.
    pub fn run(&self, sql: &str, rows: &dyn Rows) -> Result<Answer, SqlError> {
        match parse(sql)? {
            Stmt::Ignored(tag) => Ok(Answer {
                columns: Vec::new(),
                rows: Vec::new(),
                tag: tag.to_string(),
            }),
            Stmt::Select(s) => self.select(&s, rows),
            Stmt::Union {
                branches,
                all,
                order,
                limit,
                offset,
            } => self.union(&branches, all, &order, limit, offset, rows),
        }
    }

    /// The branches in turn, stacked, and deduplicated unless `all`.
    ///
    /// `O(rows log rows)` for the deduplication, over the rows the branches produced together.
    #[allow(clippy::too_many_arguments)]
    fn union(
        &self,
        branches: &[Select],
        all: bool,
        order: &[Order],
        limit: Option<usize>,
        offset: usize,
        rows_of: &dyn Rows,
    ) -> Result<Answer, SqlError> {
        let mut columns: Vec<Column> = Vec::new();
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        for (i, b) in branches.iter().enumerate() {
            let answer = self.select(b, rows_of)?;
            if i == 0 {
                columns = answer.columns;
            } else if answer.columns.len() != columns.len() {
                return Err(SqlError::syntax(format!(
                    "each `union` branch has to answer the same number of columns; the first \
                     answers {} and this one answers {}",
                    columns.len(),
                    answer.columns.len()
                )));
            }
            rows.extend(answer.rows);
        }
        if !all {
            let mut seen = BTreeSet::new();
            rows.retain(|r| seen.insert(row_key(r)));
        }
        let fields: Vec<Field> = columns
            .iter()
            .map(|c| Field {
                column: c.clone(),
                of: None,
            })
            .collect();
        let proj: Vec<Proj> = (0..columns.len()).map(Proj::Column).collect();
        let ev = Eval::new(self, &fields, rows_of);
        let mut rows = order_rows(&ev, order, &columns, &proj, rows)?;
        cut(&mut rows, offset, limit);
        Ok(Answer {
            tag: format!("SELECT {}", rows.len()),
            columns,
            rows,
        })
    }

    /// What a statement's result looks like, without running it. `Describe` needs this before
    /// `Execute` has happened.
    pub fn describe(&self, sql: &str) -> Result<Vec<Column>, SqlError> {
        match parse(sql)? {
            Stmt::Ignored(_) => Ok(Vec::new()),
            Stmt::Union { branches, .. } => match branches.first() {
                Some(first) => self.describe_select(first),
                None => Ok(Vec::new()),
            },
            Stmt::Select(s) => self.describe_select(&s),
        }
    }

    fn describe_select(&self, s: &Select) -> Result<Vec<Column>, SqlError> {
        if crate::query::relational(s) {
            let compiled = crate::query::compile(self, s)?;
            if compiled.projected {
                return Ok(compiled.fields.iter().map(|f| f.column.clone()).collect());
            }
            return Ok(self.project_columns(s, &compiled.fields)?.0);
        }
        let fields = self.scan_fields(s)?;
        Ok(self.project_columns(s, &fields)?.0)
    }

    /// The columns a one-table query resolves names against, under the name it calls the table.
    fn scan_fields(&self, s: &Select) -> Result<Vec<Field>, SqlError> {
        Ok(match (self.resolve_from(s)?, s.from.first()) {
            (Some(t), Some(f)) => Field::of_table_as(t, Arc::from(f.alias.as_str())),
            _ => Vec::new(),
        })
    }

    /// The one table a non-relational query reads, if it has one.
    fn resolve_from(&self, s: &Select) -> Result<Option<&Table>, SqlError> {
        match s.from.first() {
            None => Ok(None),
            Some(f) => {
                if let Some(call) = &f.function {
                    return Err(SqlError::unsupported(format!(
                        "`{call}(…)` in a `from` is a set-returning function, and this read model \
                         has none: what a `from` names here is a relation"
                    )));
                }
                self.relation(f.namespace.as_deref(), &f.table).map(Some)
            }
        }
    }

    /// The columns a select produces, and how to build each from a source row.
    fn project_columns(
        &self,
        s: &Select,
        fields: &[Field],
    ) -> Result<(Vec<Column>, Vec<Proj>), SqlError> {
        let mut columns = Vec::new();
        let mut proj = Vec::new();
        for item in &s.items {
            match item {
                Item::All(qualifier) => {
                    if fields.is_empty() {
                        return Err(SqlError::syntax("`select *` needs a `from`"));
                    }
                    for (i, f) in fields.iter().enumerate() {
                        if let Some(t) = qualifier {
                            if f.of.as_deref() != Some(t.as_str()) {
                                continue;
                            }
                        }
                        columns.push(f.column.clone());
                        proj.push(Proj::Column(i));
                    }
                }
                Item::Column(name, alias) => {
                    if fields.is_empty() {
                        return Err(SqlError::no_column(format!(
                            "there is no column \"{name}\" here, because there is no `from`"
                        )));
                    }
                    let i = resolve_field(fields, name)?;
                    let mut c = fields[i].column.clone();
                    if let Some(a) = alias {
                        c.name = Arc::from(a.as_str());
                    }
                    columns.push(c);
                    proj.push(Proj::Column(i));
                }
                Item::Count(alias) => {
                    columns.push(Column {
                        name: Arc::from(alias.as_deref().unwrap_or("count")),
                        ty: SqlTy::Bigint,
                        nullable: false,
                    });
                    proj.push(Proj::Count);
                }
                // An aggregate reaches this only on the path that did not compile a plan, and
                // nothing routes one there: `query::relational` sends every query with one to the
                // plan, where the operator that answers it lives.
                Item::Aggregate(agg, name, _) => {
                    return Err(SqlError::unsupported(format!(
                        "`{}({name})` is a question about a group, and this query has none",
                        agg.name()
                    )))
                }
                Item::Literal(d, alias) => {
                    columns.push(Column {
                        name: Arc::from(alias.as_deref().unwrap_or("?column?")),
                        ty: d.ty(),
                        nullable: false,
                    });
                    proj.push(Proj::Literal(d.clone()));
                }
                // An expression's type is not worked out before it is evaluated: it may be a
                // `case` whose arms disagree, or a call whose answer depends on a row. Text is
                // what every client can read whatever comes back, and NULL is possible for all of
                // them, which is what `nullable` says.
                Item::Expr(e, alias) => {
                    columns.push(Column {
                        name: Arc::from(match alias.as_deref() {
                            Some(a) => a,
                            // A call's column is named after the function, as PostgreSQL names
                            // one — `select version()` has a column called `version`.
                            None => match e {
                                Expr::Call { name, .. } => name.as_str(),
                                _ => "?column?",
                            },
                        }),
                        ty: SqlTy::Text,
                        nullable: true,
                    });
                    proj.push(Proj::Expr(e.clone()));
                }
            }
        }
        Ok((columns, proj))
    }

    fn select(&self, s: &Select, rows_of: &dyn Rows) -> Result<Answer, SqlError> {
        // A join, a `group by` or a `distinct`: compiled into the plan and run by the engine, so
        // the operators are the ones a program's view uses (docs/99 §99.9 item 9). What comes back
        // is rows and what they are called; the `where` this could not push into a scan, the
        // `order by` and the `limit` are below, shared with every other query.
        if crate::query::relational(s) {
            let compiled = crate::query::compile(self, s)?;
            let rows = compiled.run(self, rows_of)?;
            return self.finish(
                s,
                &compiled.fields,
                rows,
                &compiled.residual,
                compiled.projected,
                rows_of,
            );
        }

        let table = self.resolve_from(s)?;
        let fields = self.scan_fields(s)?;
        let (columns, proj) = self.project_columns(s, &fields)?;

        // `select count(*) from t`, with nothing to narrow it: the answer is the collection's size,
        // and the collection already knows it (§23.19). Everything below this would clone every
        // value, build a `Cell` per column of every row, and then count the rows — `O(n)` cells for
        // an answer that is one integer.
        //
        // The conditions are conservative on purpose. A `where` needs the rows to test; an `order`,
        // `limit` or `offset` is applied *before* the collapse below, so honouring one of those on
        // this path would mean reproducing that behaviour rather than skipping work.
        if let Some(t) = table {
            let bare = proj.iter().any(|p| matches!(p, Proj::Count))
                && proj
                    .iter()
                    .all(|p| matches!(p, Proj::Count | Proj::Literal(_)))
                && s.filter.is_empty()
                && s.order.is_empty()
                && s.limit.is_none()
                && s.offset == 0;
            if bare {
                let n = match self.builtin_rows(t) {
                    Some(rows) => Some(rows.len() as u64),
                    None => rows_of.count(t)?,
                };
                if let Some(n) = n {
                    let n = match t.cardinality {
                        Cardinality::Many => n,
                        Cardinality::One => n.min(1),
                    };
                    let row: Vec<Cell> = proj
                        .iter()
                        .map(|p| match p {
                            Proj::Count => Some(Datum::Bigint(n as i64)),
                            Proj::Literal(d) => Some(d.clone()),
                            Proj::Column(_) | Proj::Expr(_) => None,
                        })
                        .collect();
                    return Ok(Answer {
                        tag: "SELECT 1".to_string(),
                        columns,
                        rows: vec![row],
                    });
                }
            }
        }

        // `select 1` and friends: one row, no table, and the four things a driver asks before it
        // trusts a connection.
        let Some(t) = table else {
            let ev = Eval::new(self, &fields, rows_of);
            let row: Vec<Cell> = proj
                .iter()
                .map(|p| match p {
                    Proj::Literal(d) => Ok(Some(d.clone())),
                    Proj::Count => Ok(Some(Datum::Bigint(1))),
                    Proj::Expr(e) => ev.cell(e, &[]),
                    Proj::Column(_) => Ok(None),
                })
                .collect::<Result<_, _>>()?;
            return Ok(Answer {
                tag: "SELECT 1".to_string(),
                columns,
                rows: vec![row],
            });
        };

        let rows: Vec<Vec<Cell>> = match self.builtin_rows(t) {
            Some(values) => values.iter().map(|v| t.row(v)).collect(),
            None => {
                let values = rows_of.scan(t)?;
                match t.cardinality {
                    Cardinality::Many => values.iter().map(|v| t.row(v)).collect(),
                    // A singleton is one row even when the reader hands over the value inside a
                    // one-element list, which is what `elements` does with a record.
                    Cardinality::One => values.iter().take(1).map(|v| t.row(v)).collect(),
                }
            }
        };
        self.finish(s, &fields, rows, &s.filter, false, rows_of)
    }

    /// Filter, order, project, and cut — the half of a `select` that is the same whether the rows
    /// were scanned out of a collection or produced by the plan's operators.
    fn finish(
        &self,
        s: &Select,
        fields: &[Field],
        mut rows: Vec<Vec<Cell>>,
        filter: &[Expr],
        projected: bool,
        rows_of: &dyn Rows,
    ) -> Result<Answer, SqlError> {
        let ev = Eval::new(self, fields, rows_of);

        // The plan already projected: its operators were compiled from the select list, so its
        // rows are the answer and the only thing left is what a `where` could not be pushed into.
        let (columns, proj) = match projected {
            true => (
                fields.iter().map(|f| f.column.clone()).collect(),
                (0..fields.len()).map(Proj::Column).collect(),
            ),
            false => self.project_columns(s, fields)?,
        };

        // Filter, then order, then offset and limit — the order SQL specifies, and the order that
        // makes `limit` mean what a person expects.
        if !filter.is_empty() {
            let mut kept = Vec::with_capacity(rows.len());
            for row in rows {
                if ev.holds(filter, &row)? {
                    kept.push(row);
                }
            }
            rows = kept;
        }
        rows = order_rows(&ev, &s.order, &columns, &proj, rows)?;
        cut(&mut rows, s.offset, s.limit);

        if projected {
            return Ok(Answer {
                tag: format!("SELECT {}", rows.len()),
                columns,
                rows,
            });
        }

        // `count(*)` collapses. Mixing it with a column would be a group-by, which this path has
        // none of, so it is refused at parse time rather than answered wrongly.
        let out: Vec<Vec<Cell>> = if proj.iter().any(|p| matches!(p, Proj::Count)) {
            let n = rows.len();
            vec![proj
                .iter()
                .map(|p| match p {
                    Proj::Count => Ok(Some(Datum::Bigint(n as i64))),
                    Proj::Literal(d) => Ok(Some(d.clone())),
                    // An expression beside `count(*)` is evaluated over the first row, which is
                    // the only row a collapsed answer has to be about; with no rows there is
                    // nothing to evaluate it over and it is null.
                    Proj::Expr(e) => match rows.first() {
                        Some(r) => ev.cell(e, r),
                        None => Ok(None),
                    },
                    Proj::Column(_) => Ok(None),
                })
                .collect::<Result<_, _>>()?]
        } else {
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(project(&ev, &proj, r)?);
            }
            out
        };
        Ok(Answer {
            tag: format!("SELECT {}", out.len()),
            columns,
            rows: out,
        })
    }
}

/// One row through the select list.
fn project(ev: &Eval, proj: &[Proj], row: &[Cell]) -> Result<Vec<Cell>, SqlError> {
    proj.iter()
        .map(|p| match p {
            Proj::Column(i) => Ok(row[*i].clone()),
            Proj::Literal(d) => Ok(Some(d.clone())),
            Proj::Expr(e) => ev.cell(e, row),
            Proj::Count => Ok(None),
        })
        .collect()
}

/// Sort rows by the `order by` keys, first key first.
///
/// A key is resolved the way SQL resolves one: an ordinal or a name that matches an output column
/// is *that output column*, computed for each row by the projection that produces it; anything
/// else is an expression over the row the query has. Both are functions of the row this holds, so
/// the sort happens before the projection either way and a `limit` still cuts the rows a person
/// asked to see.
///
/// `O(rows log rows)` comparisons, each `O(keys)`, with the keys computed once per row rather than
/// once per comparison — a sort key that called a function per comparison would be `O(n log n)`
/// calls for `O(n)` distinct answers.
fn order_rows(
    ev: &Eval,
    order: &[Order],
    columns: &[Column],
    proj: &[Proj],
    rows: Vec<Vec<Cell>>,
) -> Result<Vec<Vec<Cell>>, SqlError> {
    if order.is_empty() || rows.len() < 2 {
        return Ok(rows);
    }
    // What each key reads: a projection of the output, or an expression over the input.
    enum Key<'a> {
        Out(&'a Proj),
        Expr(&'a Expr),
    }
    let mut keys = Vec::with_capacity(order.len());
    for o in order {
        let key = match &o.by {
            OrderBy::Ordinal(n) => match n.checked_sub(1).and_then(|i| proj.get(i)) {
                Some(p) => Key::Out(p),
                None => {
                    return Err(SqlError::no_column(format!(
                        "`order by {n}` names the {n}th column of the select list, and there {}",
                        match columns.len() {
                            0 => "is none".to_string(),
                            1 => "is one".to_string(),
                            n => format!("are {n}"),
                        }
                    )))
                }
            },
            // A bare name that is an output column's name is that output column, **before** it is
            // a column of a table: that is SQL's own resolution order for an `order by`, and it is
            // what lets a query order by something it computed and gave a name to.
            OrderBy::Expr(Expr::Column(n))
                if n.table.is_none() && columns.iter().any(|c| c.name.as_ref() == n.column) =>
            {
                let i = columns
                    .iter()
                    .position(|c| c.name.as_ref() == n.column)
                    .expect("just found");
                Key::Out(&proj[i])
            }
            OrderBy::Expr(e) => Key::Expr(e),
        };
        keys.push((key, o.asc));
    }

    // The keys, computed once per row. A failure to *resolve* one is reported here rather than
    // per comparison, so a wrong `order by` is one error rather than `n log n` of them.
    let mut keyed: Vec<(Vec<Cell>, Vec<Cell>)> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut k = Vec::with_capacity(keys.len());
        for (key, _) in &keys {
            k.push(match key {
                Key::Out(p) => project(ev, std::slice::from_ref(*p), &row)?.remove(0),
                Key::Expr(e) => ev.cell(e, &row)?,
            });
        }
        keyed.push((k, row));
    }
    // Stable, so the order the collection holds its elements in survives ties — which is the
    // arrangement's key order, and therefore the order the page renders in.
    keyed.sort_by(|a, b| {
        for (i, (_, asc)) in keys.iter().enumerate() {
            let o = compare(&a.0[i], &b.0[i]);
            let o = if *asc { o } else { o.reverse() };
            if !o.is_eq() {
                return o;
            }
        }
        std::cmp::Ordering::Equal
    });
    Ok(keyed.into_iter().map(|(_, row)| row).collect())
}

fn cut(rows: &mut Vec<Vec<Cell>>, offset: usize, limit: Option<usize>) {
    if offset > 0 {
        *rows = rows.split_off(offset.min(rows.len()));
    }
    if let Some(n) = limit {
        rows.truncate(n);
    }
}

/// A row as something a `BTreeSet` can hold, for `union`'s deduplication.
///
/// The text form rather than the values: a `Datum` holds an `f64` and is therefore not `Ord`, and
/// two rows a client cannot tell apart are two rows `union` should not answer twice.
fn row_key(row: &[Cell]) -> Vec<Option<String>> {
    row.iter().map(|c| c.as_ref().map(Datum::text)).collect()
}

enum Proj {
    Column(usize),
    Count,
    Literal(Datum),
    Expr(Expr),
}

/// A column reference, as an index into the row a query produced.
///
/// Shared by the select list, the `where` and the `order by`, and by [`crate::query`]'s own
/// resolution of a join's `on` — one rule for what a name means, so a query cannot resolve a name
/// two ways.
pub fn resolve_field(fields: &[Field], n: &Name) -> Result<usize, SqlError> {
    let matching: Vec<usize> = (0..fields.len())
        .filter(|&i| {
            fields[i].column.name.as_ref() == n.column
                && match &n.table {
                    Some(t) => fields[i].of.as_deref() == Some(t.as_str()),
                    None => true,
                }
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(*one),
        [] => Err(SqlError::no_column(format!(
            "there is no column \"{n}\" here; there is {}",
            names_of(fields)
        ))),
        _ => Err(SqlError::no_column(format!(
            "\"{n}\" is ambiguous: more than one table in this query has a column called \
             \"{}\", so qualify it — `t.{}`",
            n.column, n.column
        ))),
    }
}

/// The columns a query has, as a person is told about them.
pub fn names_of(fields: &[Field]) -> String {
    fields
        .iter()
        .map(|f| match &f.of {
            Some(t) => format!("\"{t}.{}\"", f.column.name),
            None => format!("\"{}\"", f.column.name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// -------------------------------------------------------------------------------------------
// Evaluating an expression over a row
// -------------------------------------------------------------------------------------------

/// What an [`Expr`] is evaluated against: the columns a row has, and the reader behind them.
///
/// Two things are memoised, and both are memoised because they are the *same answer for every
/// row*:
///
/// * **A name's column index.** Resolution is a scan of the query's fields, and a query over a
///   million rows would otherwise do it a million times per reference.
/// * **A subquery's value.** See [`Eval::subquery`] — what is evaluated there is by construction
///   uncorrelated, so it cannot depend on the row.
///
/// Neither memo can make an answer wrong, only repeated: a name resolves to one index for the
/// whole query, and a query whose fields changed would be a different `Eval`.
pub struct Eval<'a> {
    schema: &'a Schema,
    fields: &'a [Field],
    rows: &'a dyn Rows,
    resolved: std::cell::RefCell<BTreeMap<Name, usize>>,
    subqueries: std::cell::RefCell<BTreeMap<usize, Cell>>,
}

impl<'a> Eval<'a> {
    pub fn new(schema: &'a Schema, fields: &'a [Field], rows: &'a dyn Rows) -> Eval<'a> {
        Eval {
            schema,
            fields,
            rows,
            resolved: Default::default(),
            subqueries: Default::default(),
        }
    }

    /// Where a name's value sits in a row.
    pub fn resolve(&self, n: &Name) -> Result<usize, SqlError> {
        if let Some(i) = self.resolved.borrow().get(n) {
            return Ok(*i);
        }
        let i = resolve_field(self.fields, n)?;
        self.resolved.borrow_mut().insert(n.clone(), i);
        Ok(i)
    }

    /// Whether every term of a conjunction is true of this row.
    pub fn holds(&self, terms: &[Expr], row: &[Cell]) -> Result<bool, SqlError> {
        for t in terms {
            if !truthy(&self.cell(t, row)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// One expression's value for one row.
    ///
    /// `O(1)` per node, and every node is visited at most once — except the arms of a `case`,
    /// which are visited until one matches and never after.
    pub fn cell(&self, e: &Expr, row: &[Cell]) -> Result<Cell, SqlError> {
        match e {
            Expr::Column(n) => Ok(row.get(self.resolve(n)?).cloned().flatten()),
            Expr::Literal(c) => Ok(c.clone()),
            // Three-valued `and` and `or`, which is what makes `null` propagate the way a client
            // expects: `false and null` is false, `true and null` is unknown.
            Expr::And(xs) => {
                let mut unknown = false;
                for x in xs {
                    match self.cell(x, row)? {
                        Some(Datum::Boolean(false)) => return Ok(Some(Datum::Boolean(false))),
                        Some(Datum::Boolean(true)) => {}
                        _ => unknown = true,
                    }
                }
                Ok(match unknown {
                    true => None,
                    false => Some(Datum::Boolean(true)),
                })
            }
            Expr::Or(xs) => {
                let mut unknown = false;
                for x in xs {
                    match self.cell(x, row)? {
                        Some(Datum::Boolean(true)) => return Ok(Some(Datum::Boolean(true))),
                        Some(Datum::Boolean(false)) => {}
                        _ => unknown = true,
                    }
                }
                Ok(match unknown {
                    true => None,
                    false => Some(Datum::Boolean(false)),
                })
            }
            Expr::Not(x) => Ok(match self.cell(x, row)? {
                Some(Datum::Boolean(b)) => Some(Datum::Boolean(!b)),
                _ => None,
            }),
            Expr::Cmp(a, op, b) => {
                let (a, b) = (self.cell(a, row)?, self.cell(b, row)?);
                Ok(match (a, b) {
                    (Some(a), Some(b)) => {
                        Some(Datum::Boolean(op.holds(compare(&Some(a), &Some(b)))))
                    }
                    // A comparison against NULL is unknown, and unknown is not true.
                    _ => None,
                })
            }
            Expr::Is { value, to, negated } => {
                let v = self.cell(value, row)?;
                let is = match to {
                    None => v.is_none(),
                    Some(b) => v == Some(Datum::Boolean(*b)),
                };
                Ok(Some(Datum::Boolean(is != *negated)))
            }
            Expr::In {
                value,
                list,
                negated,
            } => {
                let v = self.cell(value, row)?;
                if v.is_none() {
                    return Ok(None);
                }
                let mut unknown = false;
                for item in list {
                    match self.cell(item, row)? {
                        None => unknown = true,
                        other if other == v => return Ok(Some(Datum::Boolean(!*negated))),
                        _ => {}
                    }
                }
                Ok(match unknown {
                    true => None,
                    false => Some(Datum::Boolean(*negated)),
                })
            }
            Expr::Match {
                value,
                pattern,
                negated,
                insensitive,
            } => {
                let (v, p) = (self.cell(value, row)?, self.cell(pattern, row)?);
                let (Some(v), Some(p)) = (v, p) else {
                    return Ok(None);
                };
                let hit = regex::matches(&p.text(), &v.text(), *insensitive)?;
                Ok(Some(Datum::Boolean(hit != *negated)))
            }
            Expr::Case {
                operand,
                arms,
                otherwise,
            } => {
                let subject = match operand {
                    Some(o) => Some(self.cell(o, row)?),
                    None => None,
                };
                for (when, then) in arms {
                    let hit = match &subject {
                        // `case x when a then …`: an equality, and a NULL matches nothing.
                        Some(s) => {
                            let w = self.cell(when, row)?;
                            s.is_some() && w.is_some() && compare(s, &w).is_eq()
                        }
                        None => truthy(&self.cell(when, row)?),
                    };
                    if hit {
                        return self.cell(then, row);
                    }
                }
                match otherwise {
                    Some(e) => self.cell(e, row),
                    // A `case` with no `else` and no arm taken is NULL, which is what `psql` reads
                    // as an empty cell.
                    None => Ok(None),
                }
            }
            Expr::Call { name, args } => {
                let mut values = Vec::with_capacity(args.len());
                for a in args {
                    values.push(self.cell(a, row)?);
                }
                match crate::pg::call(name, &values) {
                    Some(Ok(c)) => Ok(c),
                    Some(Err(why)) => Err(SqlError::unsupported(why)),
                    None => Err(SqlError::unsupported(format!(
                        "`{name}(…)` is not a function this read model has. The catalogue answers \
                         the ones `psql` asks it — format_type, pg_get_userbyid, \
                         pg_table_is_visible, pg_get_expr, pg_encoding_to_char — and there is no \
                         expression language behind them"
                    ))),
                }
            }
            Expr::Cast { value, ty } => {
                let v = self.cell(value, row)?;
                Ok(match (v, ty.as_str()) {
                    (None, _) => None,
                    (Some(v), "text" | "varchar" | "name" | "char") => Some(Datum::Text(v.text())),
                    (Some(v), "bool" | "boolean") => match v {
                        Datum::Boolean(_) => Some(v),
                        other => Some(Datum::Boolean(other.text() == "t")),
                    },
                    (Some(v), "int" | "int2" | "int4" | "int8" | "bigint" | "integer" | "oid") => {
                        match v {
                            Datum::Bigint(_) => Some(v),
                            other => other.text().parse::<i64>().ok().map(Datum::Bigint),
                        }
                    }
                    (Some(v), "float4" | "float8" | "real" | "numeric") => match v {
                        Datum::Double(_) => Some(v),
                        other => other.text().parse::<f64>().ok().map(Datum::Double),
                    },
                    (Some(_), other) => {
                        return Err(SqlError::unsupported(format!(
                            "`::{other}` is a cast to a type this read model has no values of. \
                             The four types are boolean, bigint, double precision and text, and \
                             an object identifier printed as a name is a lookup in a catalogue \
                             with nothing to look up"
                        )))
                    }
                })
            }
            Expr::Concat(a, b) => {
                let (a, b) = (self.cell(a, row)?, self.cell(b, row)?);
                Ok(match (a, b) {
                    (Some(a), Some(b)) => Some(Datum::Text(format!("{}{}", a.text(), b.text()))),
                    _ => None,
                })
            }
            Expr::Subquery { id, select } => self.subquery(*id, select),
            Expr::Array { .. } => Err(SqlError::unsupported(
                "`array(select …)` builds an array, and an array is not one of this read model's \
                 four types",
            )),
            Expr::Any(_) => Err(SqlError::unsupported(
                "`any(…)` compares against the elements of an array, and an array is not one of \
                 this read model's four types",
            )),
            Expr::Subscript(..) => Err(SqlError::unsupported(
                "`x[i]` reads an element of an array, and an array is not one of this read \
                 model's four types",
            )),
        }
    }

    /// A scalar subquery's value, when it can be established without correlation.
    ///
    /// The rule, and it is the whole of it: **drop the terms that name something this subquery's
    /// own tables do not have** — those are the ones correlated to the outer row — and run what is
    /// left. Dropping a term can only *add* rows, so a widened query that answers nothing proves
    /// the original answers nothing, and a scalar subquery with no rows is NULL. That is the
    /// answer for every outer row, which is why it is computed once.
    ///
    /// When the widened query does have rows the answer depends on the correlation, and it is
    /// refused by name rather than guessed at. Every scalar subquery `psql` sends the catalogue
    /// asks about a default expression or a collation — relations a read model has none of — so
    /// the empty case is the one that happens, and the refusal is what would happen if that ever
    /// stopped being true.
    pub fn subquery(&self, id: usize, select: &Select) -> Result<Cell, SqlError> {
        if let Some(c) = self.subqueries.borrow().get(&id) {
            return Ok(c.clone());
        }
        let mut widened = select.clone();
        let own: Vec<Field> = widened
            .from
            .iter()
            .filter(|f| f.function.is_none())
            .try_fold(Vec::new(), |mut acc: Vec<Field>, f| {
                acc.extend(Field::of_table_as(
                    self.schema.relation(f.namespace.as_deref(), &f.table)?,
                    Arc::from(f.alias.as_str()),
                ));
                Ok::<_, SqlError>(acc)
            })?;
        widened.filter.retain(|term| {
            let mut names = Vec::new();
            term.names(&mut names);
            names.iter().all(|n| resolve_field(&own, n).is_ok())
        });
        let answer = self.schema.select(&widened, self.rows)?;
        let value = match answer.rows.len() {
            0 => None,
            _ => {
                return Err(SqlError::unsupported(format!(
                    "this subquery answers {} row{} once the conditions that mention the outer \
                     query are dropped, so its value depends on which row is asking — and a \
                     correlated subquery is not in this SQL subset",
                    answer.rows.len(),
                    match answer.rows.len() {
                        1 => "",
                        _ => "s",
                    }
                )))
            }
        };
        self.subqueries.borrow_mut().insert(id, value.clone());
        Ok(value)
    }
}

/// Whether a cell is SQL's `true`. NULL is not, which is the whole of three-valued logic where a
/// `where` is concerned.
fn truthy(c: &Cell) -> bool {
    matches!(c, Some(Datum::Boolean(true)))
}

/// NULLs sort last, as they do in Postgres for an ascending order.
fn compare(a: &Cell, b: &Cell) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => match (x, y) {
            (Datum::Bigint(p), Datum::Bigint(q)) => p.cmp(q),
            (Datum::Double(p), Datum::Double(q)) => p.partial_cmp(q).unwrap_or(Ordering::Equal),
            (Datum::Bigint(p), Datum::Double(q)) => {
                (*p as f64).partial_cmp(q).unwrap_or(Ordering::Equal)
            }
            (Datum::Double(p), Datum::Bigint(q)) => {
                p.partial_cmp(&(*q as f64)).unwrap_or(Ordering::Equal)
            }
            (Datum::Boolean(p), Datum::Boolean(q)) => p.cmp(q),
            (Datum::Text(p), Datum::Text(q)) => p.cmp(q),
            // Across kinds, compare the text. Nothing in a typed column reaches this; a literal
            // compared against a column of another type does, and `c.oid = '16384'` — a number
            // written as a string, which is how `psql` writes every one of them — is that.
            _ => x.text().cmp(&y.text()),
        },
    }
}

// -------------------------------------------------------------------------------------------
// The regular expression `~` matches against
// -------------------------------------------------------------------------------------------

/// POSIX regular expressions, for the four operators `~`, `!~`, `~*` and `!~*`.
///
/// Written here for [`crate::read`]'s reason and not a new one: `psql` sends `relname ~
/// '^(todos)$'` to find a table and `nspname !~ '^pg_toast'` to hide the ones it does not want,
/// so the catalogue cannot be read without one — and a regular expression crate would be a
/// dependency for two operators over patterns a client generates.
///
/// # Why it is a simulation and not a backtracker
///
/// **The pattern arrives from a client.** A backtracking matcher is exponential on patterns like
/// `(a|a)*b`, and one is three characters to type. This compiles the pattern to an NFA and
/// advances a *set* of states one character at a time, which is `O(pattern × text)` for every
/// pattern there is — the cost is a property of the algorithm rather than of the input, so there
/// is no budget to tune and nothing to refuse.
///
/// # What it has
///
/// `^ $ . * + ? | ( ) [ ] [^ ] -` and `\` before any character. What it does not have is
/// back-references — which is what makes the simulation possible — and counted repetition
/// `{n,m}`, which nothing sends; both are refused by name.
mod regex {
    use super::SqlError;

    /// Whether `text` matches `pattern` anywhere in it, which is what POSIX `~` asks.
    pub fn matches(pattern: &str, text: &str, insensitive: bool) -> Result<bool, SqlError> {
        let program = compile(pattern, insensitive)?;
        Ok(run(&program, text, insensitive))
    }

    enum Ins {
        Char(char),
        Any,
        /// A bracket expression, as ranges, and whether it is negated.
        Class(Vec<(char, char)>, bool),
        /// The zero-width assertions. There is no multi-line mode, so these are the ends of the
        /// text rather than of a line.
        Start,
        End,
        Split(usize, usize),
        Jmp(usize),
        Match,
    }

    /// A parsed pattern, as a tree, before it becomes instructions.
    enum Node {
        Empty,
        Char(char),
        Any,
        Class(Vec<(char, char)>, bool),
        Start,
        End,
        Concat(Vec<Node>),
        Alt(Vec<Node>),
        /// `x*`, `x+`, `x?` — the minimum and whether it repeats.
        Repeat(Box<Node>, u32, bool),
    }

    struct P<'a> {
        cs: &'a [char],
        i: usize,
    }

    impl P<'_> {
        fn peek(&self) -> Option<char> {
            self.cs.get(self.i).copied()
        }

        fn alt(&mut self) -> Result<Node, SqlError> {
            let mut branches = vec![self.concat()?];
            while self.peek() == Some('|') {
                self.i += 1;
                branches.push(self.concat()?);
            }
            Ok(match branches.len() {
                1 => branches.pop().expect("one branch"),
                _ => Node::Alt(branches),
            })
        }

        fn concat(&mut self) -> Result<Node, SqlError> {
            let mut parts = Vec::new();
            while !matches!(self.peek(), None | Some('|') | Some(')')) {
                parts.push(self.repeat()?);
            }
            Ok(match parts.len() {
                0 => Node::Empty,
                1 => parts.pop().expect("one part"),
                _ => Node::Concat(parts),
            })
        }

        fn repeat(&mut self) -> Result<Node, SqlError> {
            let mut node = self.atom()?;
            loop {
                node = match self.peek() {
                    Some('*') => Node::Repeat(Box::new(node), 0, true),
                    Some('+') => Node::Repeat(Box::new(node), 1, true),
                    Some('?') => Node::Repeat(Box::new(node), 0, false),
                    Some('{') => {
                        return Err(SqlError::unsupported(
                            "a counted repetition `{n,m}` is not in this regular expression \
                             subset: `*`, `+` and `?` are what there is",
                        ))
                    }
                    _ => return Ok(node),
                };
                self.i += 1;
            }
        }

        fn atom(&mut self) -> Result<Node, SqlError> {
            let Some(c) = self.peek() else {
                return Ok(Node::Empty);
            };
            self.i += 1;
            Ok(match c {
                '(' => {
                    let inner = self.alt()?;
                    if self.peek() != Some(')') {
                        return Err(SqlError::syntax(
                            "a group in a regular expression is not closed",
                        ));
                    }
                    self.i += 1;
                    inner
                }
                '[' => self.class()?,
                '.' => Node::Any,
                '^' => Node::Start,
                '$' => Node::End,
                '\\' => match self.peek() {
                    Some(e) => {
                        self.i += 1;
                        Node::Char(e)
                    }
                    None => {
                        return Err(SqlError::syntax(
                            "a regular expression ends in a backslash, which escapes nothing",
                        ))
                    }
                },
                other => Node::Char(other),
            })
        }

        fn class(&mut self) -> Result<Node, SqlError> {
            let negated = self.peek() == Some('^');
            if negated {
                self.i += 1;
            }
            let mut ranges = Vec::new();
            // A `]` first is a literal `]`, which is POSIX's rule for including one.
            let mut first = true;
            loop {
                let Some(c) = self.peek() else {
                    return Err(SqlError::syntax(
                        "a bracket expression in a regular expression is not closed",
                    ));
                };
                if c == ']' && !first {
                    self.i += 1;
                    return Ok(Node::Class(ranges, negated));
                }
                first = false;
                self.i += 1;
                let lo = match c {
                    '\\' => match self.peek() {
                        Some(e) => {
                            self.i += 1;
                            e
                        }
                        None => {
                            return Err(SqlError::syntax(
                                "a bracket expression ends in a backslash",
                            ))
                        }
                    },
                    other => other,
                };
                if self.peek() == Some('-') && self.cs.get(self.i + 1).is_some_and(|c| *c != ']') {
                    self.i += 1;
                    let hi = self.cs[self.i];
                    self.i += 1;
                    ranges.push((lo, hi));
                } else {
                    ranges.push((lo, lo));
                }
            }
        }
    }

    fn compile(pattern: &str, insensitive: bool) -> Result<Vec<Ins>, SqlError> {
        let cs: Vec<char> = pattern.chars().collect();
        let mut p = P { cs: &cs, i: 0 };
        let node = p.alt()?;
        if p.i < cs.len() {
            return Err(SqlError::syntax(format!(
                "a regular expression has a `{}` with no group to close",
                cs[p.i]
            )));
        }
        let mut out = Vec::new();
        emit(&node, &mut out, insensitive);
        out.push(Ins::Match);
        Ok(out)
    }

    fn emit(node: &Node, out: &mut Vec<Ins>, insensitive: bool) {
        let fold = |c: char| match insensitive {
            true => c.to_lowercase().next().unwrap_or(c),
            false => c,
        };
        match node {
            Node::Empty => {}
            Node::Char(c) => out.push(Ins::Char(fold(*c))),
            Node::Any => out.push(Ins::Any),
            Node::Class(ranges, negated) => out.push(Ins::Class(
                ranges.iter().map(|(a, b)| (fold(*a), fold(*b))).collect(),
                *negated,
            )),
            Node::Start => out.push(Ins::Start),
            Node::End => out.push(Ins::End),
            Node::Concat(parts) => parts.iter().for_each(|p| emit(p, out, insensitive)),
            Node::Alt(branches) => {
                // A chain of splits, each falling through to the next branch.
                let mut jumps = Vec::new();
                for (i, b) in branches.iter().enumerate() {
                    if i + 1 < branches.len() {
                        let split = out.len();
                        out.push(Ins::Split(0, 0));
                        emit(b, out, insensitive);
                        jumps.push(out.len());
                        out.push(Ins::Jmp(0));
                        let next = out.len();
                        out[split] = Ins::Split(split + 1, next);
                    } else {
                        emit(b, out, insensitive);
                    }
                }
                let end = out.len();
                for j in jumps {
                    out[j] = Ins::Jmp(end);
                }
            }
            Node::Repeat(inner, min, many) => {
                if *min == 1 {
                    // `x+` is `x` then `x*`, which keeps one copy of the instructions for the
                    // first iteration and one for the loop.
                    emit(inner, out, insensitive);
                }
                let split = out.len();
                out.push(Ins::Split(0, 0));
                emit(inner, out, insensitive);
                if *many {
                    out.push(Ins::Jmp(split));
                }
                let after = out.len();
                out[split] = Ins::Split(split + 1, after);
            }
        }
    }

    /// Advance a set of states over the text.
    ///
    /// `O(states × text)`: each character adds every state at most once to the next set, and the
    /// `on` vector is what makes "at most once" true.
    fn run(program: &[Ins], text: &str, insensitive: bool) -> bool {
        let cs: Vec<char> = match insensitive {
            true => text.to_lowercase().chars().collect(),
            false => text.chars().collect(),
        };
        let mut current: Vec<usize> = Vec::new();
        add(
            program,
            0,
            0,
            cs.len(),
            &mut current,
            &mut vec![false; program.len()],
        );
        for (i, c) in cs.iter().enumerate() {
            let mut next = Vec::new();
            // One "already added" flag per instruction per position, which is what bounds the set
            // to the program's length and the whole match to `O(states × text)`.
            let mut next_on = vec![false; program.len()];
            for pc in &current {
                let step = match &program[*pc] {
                    Ins::Char(want) => *want == *c,
                    Ins::Any => true,
                    Ins::Class(ranges, negated) => {
                        ranges.iter().any(|(lo, hi)| *lo <= *c && *c <= *hi) != *negated
                    }
                    Ins::Match => return true,
                    _ => continue,
                };
                if step {
                    add(program, pc + 1, i + 1, cs.len(), &mut next, &mut next_on);
                }
            }
            // POSIX `~` searches rather than anchors, so a match may start at any position: the
            // thread that has not started yet is added at every one.
            add(program, 0, i + 1, cs.len(), &mut next, &mut next_on);
            current = next;
        }
        current.iter().any(|pc| matches!(program[*pc], Ins::Match))
    }

    /// Add a state and everything reachable from it without consuming a character.
    fn add(
        program: &[Ins],
        pc: usize,
        at: usize,
        len: usize,
        set: &mut Vec<usize>,
        on: &mut [bool],
    ) {
        if on[pc] {
            return;
        }
        on[pc] = true;
        match &program[pc] {
            Ins::Jmp(to) => add(program, *to, at, len, set, on),
            Ins::Split(a, b) => {
                add(program, *a, at, len, set, on);
                add(program, *b, at, len, set, on);
            }
            Ins::Start if at == 0 => add(program, pc + 1, at, len, set, on),
            Ins::End if at == len => add(program, pc + 1, at, len, set, on),
            // An assertion that does not hold here kills the thread rather than advancing it.
            Ins::Start | Ins::End => {}
            _ => set.push(pc),
        }
    }
}

// -------------------------------------------------------------------------------------------
// The parser
// -------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Quoted(String),
    Str(String),
    Num(String),
    Sym(String),
}

fn lex(sql: &str) -> Result<Vec<Tok>, SqlError> {
    let cs: Vec<char> = sql.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && cs.get(i + 1) == Some(&'-') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if (c == 'E' || c == 'e') && cs.get(i + 1) == Some(&'\'') {
            // `E'\n'` — PostgreSQL's escape string, which `psql` writes the separator of a `\l`
            // with. The escapes are read rather than passed through, because a client that asked
            // for a newline and got a backslash and an `n` would be told something false.
            i += 2;
            let mut s = String::new();
            loop {
                match cs.get(i) {
                    None => return Err(SqlError::syntax("an escape string is not closed")),
                    Some('\'') if cs.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        i += 1;
                        let e = cs.get(i).copied().unwrap_or('\\');
                        i += 1;
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(s));
        } else if c == '_' || c.is_alphabetic() {
            let start = i;
            while i < cs.len() && (cs[i] == '_' || cs[i] == '$' || cs[i].is_alphanumeric()) {
                i += 1;
            }
            out.push(Tok::Word(cs[start..i].iter().collect()));
        } else if c.is_ascii_digit()
            || (c == '.' && cs.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(cs[start..i].iter().collect()));
        } else if c == '\'' {
            i += 1;
            let mut s = String::new();
            loop {
                match cs.get(i) {
                    None => return Err(SqlError::syntax("a string literal is not closed")),
                    // '' is an escaped quote, which is the only escape standard SQL has.
                    Some('\'') if cs.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(s));
        } else if c == '"' {
            i += 1;
            let mut s = String::new();
            loop {
                match cs.get(i) {
                    None => return Err(SqlError::syntax("a quoted name is not closed")),
                    Some('"') if cs.get(i + 1) == Some(&'"') => {
                        s.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Quoted(s));
        } else {
            // The longest operator first, so `<=` does not lex as `<` then `=` and `!~*` does not
            // lex as `!=`.
            let three: String = cs[i..(i + 3).min(cs.len())].iter().collect();
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            if three == "!~*" {
                out.push(Tok::Sym(three));
                i += 3;
            } else if matches!(
                two.as_str(),
                "<=" | ">=" | "<>" | "!=" | "::" | "||" | "!~" | "~*"
            ) {
                out.push(Tok::Sym(two));
                i += 2;
            } else {
                out.push(Tok::Sym(c.to_string()));
                i += 1;
            }
        }
    }
    Ok(out)
}

struct P {
    toks: Vec<Tok>,
    i: usize,
    /// How many subqueries have been parsed, which is what gives each one a name of its own so
    /// its answer can be computed once rather than once per row ([`Eval::subquery`]).
    subqueries: usize,
}

impl P {
    fn subquery_id(&mut self) -> usize {
        self.subqueries += 1;
        self.subqueries - 1
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    /// The next token as a lower-cased keyword, if it is a bare word.
    fn keyword(&self) -> Option<String> {
        match self.peek() {
            Some(Tok::Word(w)) => Some(w.to_lowercase()),
            _ => None,
        }
    }

    fn eat_keyword(&mut self, k: &str) -> bool {
        if self.keyword().as_deref() == Some(k) {
            self.i += 1;
            return true;
        }
        false
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        if self.peek() == Some(&Tok::Sym(s.to_string())) {
            self.i += 1;
            return true;
        }
        false
    }

    /// An identifier: a bare word, case-folded the way an unquoted SQL name is, or a quoted one
    /// taken exactly as written. Beck names are lower-case, so folding down is what matches.
    fn name(&mut self) -> Option<String> {
        match self.peek().cloned() {
            Some(Tok::Word(w)) => {
                self.i += 1;
                Some(w.to_lowercase())
            }
            Some(Tok::Quoted(w)) => {
                self.i += 1;
                Some(w)
            }
            _ => None,
        }
    }

    fn literal(&mut self) -> Option<Option<Datum>> {
        match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.i += 1;
                Some(Some(Datum::Text(s)))
            }
            Some(Tok::Num(n)) => {
                self.i += 1;
                Some(Some(match n.parse::<i64>() {
                    Ok(i) => Datum::Bigint(i),
                    Err(_) => Datum::Double(n.parse::<f64>().unwrap_or(0.0)),
                }))
            }
            Some(Tok::Sym(s)) if s == "-" => {
                self.i += 1;
                match self.literal() {
                    Some(Some(Datum::Bigint(i))) => Some(Some(Datum::Bigint(-i))),
                    Some(Some(Datum::Double(f))) => Some(Some(Datum::Double(-f))),
                    _ => None,
                }
            }
            Some(Tok::Word(w)) => match w.to_lowercase().as_str() {
                "true" => {
                    self.i += 1;
                    Some(Some(Datum::Boolean(true)))
                }
                "false" => {
                    self.i += 1;
                    Some(Some(Datum::Boolean(false)))
                }
                "null" => {
                    self.i += 1;
                    Some(None)
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// Parse one statement.
pub fn parse(sql: &str) -> Result<Stmt, SqlError> {
    let toks = lex(sql)?;
    let mut p = P {
        toks,
        i: 0,
        subqueries: 0,
    };
    let head = p.keyword().unwrap_or_default();
    match head.as_str() {
        "select" => {
            p.i += 1;
            let mut branches = vec![select(&mut p)?];
            let mut all = false;
            for word in ["intersect", "except"] {
                if p.keyword().as_deref() == Some(word) {
                    return Err(SqlError::unsupported(format!(
                        "`{word}` is not in this SQL subset; `union` is the one set operation \
                         there is"
                    )));
                }
            }
            while p.eat_keyword("union") {
                all |= p.eat_keyword("all");
                if !p.eat_keyword("select") {
                    return Err(SqlError::syntax("`union` wants a `select` after it"));
                }
                branches.push(select(&mut p)?);
            }
            // A trailing `;` is a statement separator, and a second statement is not supported —
            // saying so beats answering the first and dropping the rest.
            p.eat_sym(";");
            if p.peek().is_some() {
                return Err(SqlError::unsupported(
                    "one statement per query: this SQL has no multi-statement form",
                ));
            }
            if branches.len() == 1 {
                return Ok(Stmt::Select(branches.pop().expect("one branch")));
            }
            // `order by` and `limit` after the last branch belong to the union, which is what SQL
            // says and what the parse has to be told: each branch was parsed as a whole select.
            let last = branches.last_mut().expect("at least two branches");
            let order = std::mem::take(&mut last.order);
            let limit = last.limit.take();
            let offset = std::mem::replace(&mut last.offset, 0);
            Ok(Stmt::Union {
                branches,
                all,
                order,
                limit,
                offset,
            })
        }
        "set" => Ok(Stmt::Ignored("SET")),
        "begin" | "start" => Ok(Stmt::Ignored("BEGIN")),
        "commit" | "end" => Ok(Stmt::Ignored("COMMIT")),
        "rollback" | "abort" => Ok(Stmt::Ignored("ROLLBACK")),
        "discard" => Ok(Stmt::Ignored("DISCARD ALL")),
        "" => Err(SqlError::syntax("an empty query")),
        other => Err(SqlError::unsupported(format!(
            "a read model is read-only and this SQL is a subset: `{other}` is not one of \
             select, set, begin, commit, rollback"
        ))),
    }
}

fn select(p: &mut P) -> Result<Select, SqlError> {
    // `distinct` is the algebra's δ and it is a *plan* operator rather than a comparison over whole
    // rows here — `list_unique` names it and [`crate::plan::Op::Distinct`] maintains it, so this
    // surface grows it by compiling into the plan (docs/99 §99.9 item 7).
    let distinct = p.eat_keyword("distinct");
    if p.eat_keyword("on") {
        return Err(SqlError::unsupported(
            "`distinct on` is a PostgreSQL extension this SQL does not have; \
             `distinct` over the whole select list is what there is",
        ));
    }
    let mut items = Vec::new();
    loop {
        items.push(item(p)?);
        if !p.eat_sym(",") {
            break;
        }
    }

    let mut from = Vec::new();
    if p.eat_keyword("from") {
        from.push(from_item(p, false)?);
        loop {
            // A comma join is a cross product the `where` then narrows, which is the same query as
            // a `join … on` — and `crate::query` recognises the equality and gives it the same
            // indexed operator rather than leaving it a nested loop.
            if p.eat_sym(",") {
                from.push(from_item(p, false)?);
                continue;
            }
            if p.eat_keyword("natural") {
                return Err(SqlError::unsupported(
                    "a natural join names no key, and every join in this SQL is an equi-join \
                     because that is the operator underneath it: write `join … on <equality>`",
                ));
            }
            if p.eat_keyword("cross") {
                if !p.eat_keyword("join") {
                    return Err(SqlError::syntax("`cross` wants `join`"));
                }
                from.push(from_item(p, false)?);
                continue;
            }
            for outer in ["right", "full"] {
                if p.keyword().as_deref() == Some(outer) {
                    return Err(SqlError::unsupported(format!(
                        "`{outer} join` is not in this SQL subset. The `from` list is joined \
                         left-deep, one stage per entry, so the rows a `{outer} join` keeps are \
                         the ones no stage has yet produced; write it as a `left join` with the \
                         tables the other way round"
                    )));
                }
            }
            let left = p.eat_keyword("left");
            if left {
                p.eat_keyword("outer");
            } else {
                p.eat_keyword("inner");
            }
            if !p.eat_keyword("join") {
                break;
            }
            let mut entry = from_item(p, left)?;
            if !p.eat_keyword("on") {
                return Err(SqlError::syntax(format!(
                    "`join {}` wants `on <column> = <column>`",
                    entry.table
                )));
            }
            // `on (a = b)` and `on a = b` are the same thing, and `psql` writes both.
            let parenthesised = p.eat_sym("(");
            loop {
                let left = column_name(p).ok_or_else(|| SqlError::syntax("`on` wants a column"))?;
                if !p.eat_sym("=") {
                    return Err(SqlError::unsupported(format!(
                        "a join is an equality here: `on {left} = <column>`, and no other \
                         comparison"
                    )));
                }
                let right =
                    column_name(p).ok_or_else(|| SqlError::syntax("`on` wants a column"))?;
                entry.on.push((left, right));
                if !p.eat_keyword("and") {
                    break;
                }
            }
            if parenthesised && !p.eat_sym(")") {
                return Err(SqlError::syntax("an `on` in brackets is not closed"));
            }
            from.push(entry);
        }
    }

    let mut filter = Vec::new();
    if p.eat_keyword("where") {
        conjuncts(expr(p)?, &mut filter);
    }

    let mut group = Vec::new();
    if p.eat_keyword("group") {
        if !p.eat_keyword("by") {
            return Err(SqlError::syntax("`group` wants `by`"));
        }
        loop {
            group
                .push(column_name(p).ok_or_else(|| SqlError::syntax("`group by` wants a column"))?);
            if !p.eat_sym(",") {
                break;
            }
        }
    }
    if p.eat_keyword("having") {
        return Err(SqlError::unsupported(
            "`having` is not in this SQL subset: a `where` narrows the rows before they are \
             grouped, and there is no filter over the groups themselves",
        ));
    }

    // `count(*)` beside a column collapses one way with a `group by` and another without one, so
    // the check is about which of the two this is rather than about the item list alone.
    if group.is_empty()
        && items.iter().any(Item::aggregates)
        && items
            .iter()
            .any(|i| matches!(i, Item::All(_) | Item::Column(..)))
    {
        return Err(SqlError::unsupported(
            "an aggregate beside a column needs a `group by` saying which rows it aggregates",
        ));
    }

    let mut order = Vec::new();
    if p.eat_keyword("order") {
        if !p.eat_keyword("by") {
            return Err(SqlError::syntax("`order` wants `by`"));
        }
        loop {
            let e = expr(p)?;
            let by = match e {
                // `order by 2` is the second select item, which is SQL's own shorthand and the
                // only place a bare number means a column.
                Expr::Literal(Some(Datum::Bigint(n))) if n > 0 => OrderBy::Ordinal(n as usize),
                other => OrderBy::Expr(other),
            };
            let asc = if p.eat_keyword("desc") {
                false
            } else {
                p.eat_keyword("asc");
                true
            };
            // `nulls first` / `nulls last`: nulls sort last ascending here, as they do in
            // PostgreSQL, and a query that asked for the other order would be answered the
            // default one rather than told so.
            if p.eat_keyword("nulls") {
                let which = p.name().unwrap_or_default();
                return Err(SqlError::unsupported(format!(
                    "`nulls {which}` is not in this SQL subset: nulls sort last ascending and \
                     first descending, which is PostgreSQL's default and the only order there is"
                )));
            }
            order.push(Order { by, asc });
            if !p.eat_sym(",") {
                break;
            }
        }
    }

    let mut limit = None;
    let mut offset = 0;
    loop {
        if p.eat_keyword("limit") {
            match p.literal() {
                Some(Some(Datum::Bigint(n))) if n >= 0 => limit = Some(n as usize),
                _ => return Err(SqlError::syntax("`limit` wants a whole number")),
            }
        } else if p.eat_keyword("offset") {
            match p.literal() {
                Some(Some(Datum::Bigint(n))) if n >= 0 => offset = n as usize,
                _ => return Err(SqlError::syntax("`offset` wants a whole number")),
            }
        } else {
            break;
        }
    }

    Ok(Select {
        distinct,
        items,
        from,
        filter,
        group,
        order,
        limit,
        offset,
    })
}

/// The top-level `and` terms of a `where`, which is the unit a term is pushed into a scan as.
fn conjuncts(e: Expr, out: &mut Vec<Expr>) {
    match e {
        Expr::And(xs) => xs.into_iter().for_each(|x| conjuncts(x, out)),
        other => out.push(other),
    }
}

/// One entry of a `from` list: `t`, `s.t`, `t as x`, or a function call this does not have.
fn from_item(p: &mut P, left: bool) -> Result<From, SqlError> {
    let first = p
        .name()
        .ok_or_else(|| SqlError::syntax("`from` wants a table name"))?;
    let (namespace, name) = match p.eat_sym(".") {
        true => (
            Some(first),
            p.name()
                .ok_or_else(|| SqlError::syntax("a qualified name wants a table after the `.`"))?,
        ),
        false => (None, first),
    };
    // A set-returning function in a `from`. Its arguments are read so the rest of the statement
    // parses; what it is refused with is `resolve_from`'s message, and only if a row asks.
    let function = match p.eat_sym("(") {
        false => None,
        true => {
            let mut depth = 1;
            while depth > 0 {
                match p.peek() {
                    None => return Err(SqlError::syntax("a call in a `from` is not closed")),
                    Some(Tok::Sym(s)) if s == "(" => depth += 1,
                    Some(Tok::Sym(s)) if s == ")" => depth -= 1,
                    _ => {}
                }
                p.i += 1;
            }
            Some(name.clone())
        }
    };
    let alias = table_alias(p)?.unwrap_or_else(|| name.clone());
    Ok(From {
        namespace,
        table: name,
        alias,
        on: Vec::new(),
        left,
        function,
    })
}

/// The name a `from` entry is known by in the rest of the query: `t x`, `t as x`, or nothing.
///
/// Separate from [`alias`] because the words that may follow a table are not the words that may
/// follow a select item, and a `from todos where …` whose `where` was taken as an alias would
/// refuse the query for a missing clause it does have.
fn table_alias(p: &mut P) -> Result<Option<String>, SqlError> {
    if p.eat_keyword("as") {
        return Ok(Some(
            p.name()
                .ok_or_else(|| SqlError::syntax("`as` wants a name"))?,
        ));
    }
    match p.keyword().as_deref() {
        Some("where") | Some("order") | Some("group") | Some("having") | Some("limit")
        | Some("offset") | Some("join") | Some("inner") | Some("left") | Some("right")
        | Some("full") | Some("outer") | Some("cross") | Some("natural") | Some("on")
        | Some("union") | Some("intersect") | Some("except") | None => Ok(None),
        Some(_) => Ok(p.name()),
    }
}

/// A column reference: `c`, or `t.c`.
fn column_name(p: &mut P) -> Option<Name> {
    let first = p.name()?;
    if p.eat_sym(".") {
        return match p.name() {
            Some(column) => Some(Name {
                table: Some(first),
                column,
            }),
            None => Some(Name::bare(first)),
        };
    }
    Some(Name::bare(first))
}

fn item(p: &mut P) -> Result<Item, SqlError> {
    if p.eat_sym("*") {
        return Ok(Item::All(None));
    }
    // `t.*`, and the four aggregates. Both are decided by looking one or two tokens ahead and
    // then putting them back: a select item is an expression, and `count` is a name until the `(`
    // says otherwise.
    let save = p.i;
    if let Some(name) = p.name() {
        if p.eat_sym(".") && p.eat_sym("*") {
            return Ok(Item::All(Some(name)));
        }
        p.i = save;
    }
    let save = p.i;
    if let Some(name) = p.name() {
        if p.eat_sym("(") {
            // Every one of the four is a plan operator rather than a loop here: `count` is the
            // join's own tally and the other three are [`crate::plan::Op::GroupBy`]'s
            // (docs/99 §99.9 item 6).
            let aggregate = match name.as_str() {
                "count" => {
                    if !p.eat_sym("*") {
                        return Err(SqlError::unsupported(
                            "`count` counts rows here: `count(*)` is the only form, and \
                             `count(c)` would be a count of the rows whose `c` is not null",
                        ));
                    }
                    Some(Item::Count(None))
                }
                "min" | "max" | "sum" => {
                    let agg = match name.as_str() {
                        "min" => Agg::Min,
                        "max" => Agg::Max,
                        _ => Agg::Sum,
                    };
                    let column = column_name(p).ok_or_else(|| {
                        SqlError::unsupported(format!(
                            "`{name}` takes a column here: `{name}(c)`, and no expression"
                        ))
                    })?;
                    Some(Item::Aggregate(agg, column, None))
                }
                _ => None,
            };
            if let Some(aggregate) = aggregate {
                if !p.eat_sym(")") {
                    return Err(SqlError::syntax("a call is not closed"));
                }
                let a = alias(p);
                return Ok(match aggregate {
                    Item::Count(_) => Item::Count(a),
                    Item::Aggregate(agg, c, _) => Item::Aggregate(agg, c, a),
                    other => other,
                });
            }
        }
        p.i = save;
    }
    let e = expr(p)?;
    let a = alias(p);
    // A column and a literal keep their own item kinds: those are what `crate::query` compiles
    // into the plan for a `group by` and a `distinct`, and an expression there would have to be
    // compiled rather than evaluated over what came back.
    Ok(match e {
        Expr::Column(n) => Item::Column(n, a),
        Expr::Literal(Some(d)) => Item::Literal(d, a),
        other => Item::Expr(other, a),
    })
}

fn alias(p: &mut P) -> Option<String> {
    if p.eat_keyword("as") {
        return p.name();
    }
    // A bare alias, but not one of the words that ends a select item.
    match p.keyword().as_deref() {
        Some("from") | Some("where") | Some("group") | Some("having") | Some("order")
        | Some("limit") | Some("offset") | Some("as") | Some("union") | Some("intersect")
        | Some("except") | None => None,
        Some(_) => p.name(),
    }
}

// -------------------------------------------------------------------------------------------
// Expressions
// -------------------------------------------------------------------------------------------

/// `or` is the loosest, then `and`, then `not`, then the comparisons, then `||`, then a postfix
/// cast — PostgreSQL's precedence, and the reason a `where` is parsed as one expression rather
/// than as a list of conditions: `a = 1 or b = 2 and c = 3` means `a = 1 or (b = 2 and c = 3)`,
/// and a shape that could only hold a conjunction of disjunctions could not say so.
fn expr(p: &mut P) -> Result<Expr, SqlError> {
    let mut xs = vec![and_expr(p)?];
    while p.eat_keyword("or") {
        xs.push(and_expr(p)?);
    }
    Ok(match xs.len() {
        1 => xs.pop().expect("one term"),
        _ => Expr::Or(xs),
    })
}

fn and_expr(p: &mut P) -> Result<Expr, SqlError> {
    let mut xs = vec![not_expr(p)?];
    while p.eat_keyword("and") {
        xs.push(not_expr(p)?);
    }
    Ok(match xs.len() {
        1 => xs.pop().expect("one term"),
        _ => Expr::And(xs),
    })
}

fn not_expr(p: &mut P) -> Result<Expr, SqlError> {
    if p.eat_keyword("not") {
        return Ok(Expr::Not(Box::new(not_expr(p)?)));
    }
    cmp_expr(p)
}

fn cmp_expr(p: &mut P) -> Result<Expr, SqlError> {
    let lhs = concat_expr(p)?;
    if p.eat_keyword("is") {
        let negated = p.eat_keyword("not");
        let to =
            match p.keyword().as_deref() {
                Some("null") => None,
                Some("true") => Some(true),
                Some("false") => Some(false),
                _ => return Err(SqlError::unsupported(
                    "`is` is followed by `null`, `true` or `false` here; `is distinct from` and \
                     `is unknown` are not in this SQL subset",
                )),
            };
        p.i += 1;
        return Ok(Expr::Is {
            value: Box::new(lhs),
            to,
            negated,
        });
    }
    let negated = match p.keyword().as_deref() {
        Some("not") if p.toks.get(p.i + 1) == Some(&Tok::Word("in".into())) => {
            p.i += 1;
            true
        }
        _ => false,
    };
    if p.eat_keyword("in") {
        if !p.eat_sym("(") {
            return Err(SqlError::syntax("`in` wants a bracketed list"));
        }
        let mut list = Vec::new();
        if !p.eat_sym(")") {
            loop {
                list.push(expr(p)?);
                if !p.eat_sym(",") {
                    break;
                }
            }
            if !p.eat_sym(")") {
                return Err(SqlError::syntax("an `in` list is not closed"));
            }
        }
        return Ok(Expr::In {
            value: Box::new(lhs),
            list,
            negated,
        });
    }
    if negated {
        return Err(SqlError::syntax("`not` here wants `in`"));
    }
    // `a OPERATOR(pg_catalog.~) b` — the fully-qualified spelling of an operator, which is what
    // `psql` writes so that a search path cannot change what its own query means.
    let symbol = if p.eat_keyword("operator") {
        if !p.eat_sym("(") {
            return Err(SqlError::syntax(
                "`operator` wants a bracketed operator name",
            ));
        }
        let mut sym = String::new();
        loop {
            match p.peek().cloned() {
                Some(Tok::Sym(s)) if s == ")" => {
                    p.i += 1;
                    break;
                }
                // The schema qualifying it is read and dropped: there is one operator of each
                // name here, and it is this one.
                Some(Tok::Sym(s)) if s == "." => {
                    sym.clear();
                    p.i += 1;
                }
                Some(Tok::Sym(s)) => {
                    sym.push_str(&s);
                    p.i += 1;
                }
                Some(Tok::Word(_)) => {
                    p.i += 1;
                }
                _ => return Err(SqlError::syntax("an `operator(…)` is not closed")),
            }
        }
        Some(sym)
    } else {
        None
    };
    let take = |p: &mut P, s: &str| -> bool {
        match &symbol {
            Some(sym) => sym == s,
            None => p.eat_sym(s),
        }
    };
    let op = if take(p, "=") {
        CmpOp::Eq
    } else if take(p, "<>") || take(p, "!=") {
        CmpOp::Ne
    } else if take(p, "<=") {
        CmpOp::Le
    } else if take(p, ">=") {
        CmpOp::Ge
    } else if take(p, "<") {
        CmpOp::Lt
    } else if take(p, ">") {
        CmpOp::Gt
    } else {
        for (sym, negated, insensitive) in [
            ("~", false, false),
            ("!~", true, false),
            ("~*", false, true),
            ("!~*", true, true),
        ] {
            if take(p, sym) {
                return Ok(Expr::Match {
                    value: Box::new(lhs),
                    pattern: Box::new(concat_expr(p)?),
                    negated,
                    insensitive,
                });
            }
        }
        return match symbol {
            Some(sym) => Err(SqlError::unsupported(format!(
                "`operator({sym})` is not one of the comparisons here: =, <>, <, <=, >, >=, and \
                 the four regular-expression matches ~, !~, ~*, !~*"
            ))),
            None => Ok(lhs),
        };
    };
    Ok(Expr::Cmp(Box::new(lhs), op, Box::new(concat_expr(p)?)))
}

fn concat_expr(p: &mut P) -> Result<Expr, SqlError> {
    let mut e = postfix(p)?;
    while p.eat_sym("||") {
        e = Expr::Concat(Box::new(e), Box::new(postfix(p)?));
    }
    Ok(e)
}

/// A primary, then whatever follows it: a cast, a subscript, or a collation.
fn postfix(p: &mut P) -> Result<Expr, SqlError> {
    let mut e = primary(p)?;
    loop {
        if p.eat_sym("::") {
            // `pg_catalog.regtype` and `int2[]`: the schema and the array brackets are read and
            // dropped, because what a cast is refused for is the type's own name.
            let mut ty = p
                .name()
                .ok_or_else(|| SqlError::syntax("`::` wants a type name"))?;
            if p.eat_sym(".") {
                ty = p
                    .name()
                    .ok_or_else(|| SqlError::syntax("a qualified type wants a name"))?;
            }
            while p.eat_sym("[") {
                if !p.eat_sym("]") {
                    return Err(SqlError::syntax("an array type wants `[]`"));
                }
            }
            e = Expr::Cast {
                value: Box::new(e),
                ty,
            };
        } else if p.eat_sym("[") {
            let index = expr(p)?;
            if !p.eat_sym("]") {
                return Err(SqlError::syntax("a subscript is not closed"));
            }
            e = Expr::Subscript(Box::new(e), Box::new(index));
        } else if p.eat_keyword("collate") {
            // There is one collation and it is the one every text column already compares in.
            let _ = column_name(p);
        } else {
            return Ok(e);
        }
    }
}

fn primary(p: &mut P) -> Result<Expr, SqlError> {
    if let Some(lit) = p.literal() {
        return Ok(Expr::Literal(lit));
    }
    if p.eat_sym("(") {
        // A bracketed expression, or a subquery.
        let e = match p.keyword().as_deref() {
            Some("select") => {
                p.i += 1;
                let id = p.subquery_id();
                Expr::Subquery {
                    id,
                    select: Box::new(select(p)?),
                }
            }
            _ => expr(p)?,
        };
        if !p.eat_sym(")") {
            return Err(SqlError::syntax("a bracket is not closed"));
        }
        return Ok(e);
    }
    if p.eat_keyword("case") {
        let operand = match p.keyword().as_deref() {
            Some("when") => None,
            _ => Some(Box::new(expr(p)?)),
        };
        let mut arms = Vec::new();
        while p.eat_keyword("when") {
            let when = expr(p)?;
            if !p.eat_keyword("then") {
                return Err(SqlError::syntax("a `case` arm wants `then`"));
            }
            arms.push((when, expr(p)?));
        }
        let otherwise = match p.eat_keyword("else") {
            true => Some(Box::new(expr(p)?)),
            false => None,
        };
        if !p.eat_keyword("end") {
            return Err(SqlError::syntax("a `case` wants `end`"));
        }
        return Ok(Expr::Case {
            operand,
            arms,
            otherwise,
        });
    }
    let first = p
        .name()
        .ok_or_else(|| SqlError::syntax("an expression wants a column, a literal or a call"))?;
    if p.peek() == Some(&Tok::Sym("(".into())) {
        return call(p, first);
    }
    if p.eat_sym(".") {
        let second = p
            .name()
            .ok_or_else(|| SqlError::syntax(format!("`{first}.` wants a name")))?;
        // `pg_catalog.f(…)` — the schema is dropped, because there is one function of each name.
        if p.peek() == Some(&Tok::Sym("(".into())) {
            return call(p, second);
        }
        // `s.t.c` — a column qualified by a schema and a table. The schema is dropped for the
        // same reason a `from` entry's is: two namespaces, and a column belongs to a table.
        if p.eat_sym(".") {
            let third = p
                .name()
                .ok_or_else(|| SqlError::syntax("a qualified column wants a name"))?;
            return Ok(Expr::Column(Name {
                table: Some(second),
                column: third,
            }));
        }
        return Ok(Expr::Column(Name {
            table: Some(first),
            column: second,
        }));
    }
    Ok(Expr::Column(Name::bare(first)))
}

/// A call, with the `(` still unread.
fn call(p: &mut P, name: String) -> Result<Expr, SqlError> {
    p.eat_sym("(");
    // `array(select …)` is a constructor rather than a call, and it is the one place a subquery
    // stands where an argument would.
    if p.keyword().as_deref() == Some("select") {
        p.i += 1;
        let id = p.subquery_id();
        let inner = Box::new(select(p)?);
        if !p.eat_sym(")") {
            return Err(SqlError::syntax("a subquery is not closed"));
        }
        return Ok(match name.as_str() {
            "array" => Expr::Array { id, select: inner },
            _ => Expr::Call {
                name,
                args: vec![Expr::Subquery { id, select: inner }],
            },
        });
    }
    let mut args = Vec::new();
    if !p.eat_sym(")") {
        loop {
            args.push(expr(p)?);
            if !p.eat_sym(",") {
                break;
            }
        }
        if !p.eat_sym(")") {
            return Err(SqlError::syntax(format!("`{name}(` is not closed")));
        }
    }
    Ok(match (name.as_str(), args.len()) {
        ("any", 1) => Expr::Any(Box::new(args.pop().expect("one argument"))),
        _ => Expr::Call { name, args },
    })
}

/// What `select version()` answers.
///
/// It names Beck rather than pretending to be Postgres. A client that branches on this string is
/// better off failing on a name it does not know than succeeding on a version it will be wrong
/// about — and the `pg` prefix is there because a driver that parses this expects to find one.
pub fn version() -> String {
    format!(
        "PostgreSQL 15.0 (beck {}) — a read model, not a database",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(sql: &str) -> Select {
        match parse(sql).expect("parses") {
            Stmt::Select(s) => s,
            other => panic!("not a select: {other:?}"),
        }
    }

    /// A reader with no program behind it, for the expressions that do not read one.
    struct NoRows;

    impl Rows for NoRows {
        fn scan(&self, table: &Table) -> Result<Vec<Value>, SqlError> {
            Err(SqlError::no_table(format!(
                "no program: \"{}\"",
                table.name
            )))
        }
    }

    /// The one literal a `where` compares against, for the parse tests below.
    fn literal_of(e: &Expr) -> &Cell {
        match e {
            Expr::Cmp(_, _, rhs) => match &**rhs {
                Expr::Literal(c) => c,
                other => panic!("not a literal: {other:?}"),
            },
            other => panic!("not a comparison: {other:?}"),
        }
    }

    #[test]
    fn a_select_is_case_folded_and_a_quoted_name_is_not() {
        let s = ok("SELECT Text FROM Todos");
        assert_eq!(s.from[0].table, "todos");
        assert!(matches!(&s.items[0], Item::Column(c, _) if c.column == "text"));
        let s = ok(r#"select "Text" from "Todos""#);
        assert_eq!(s.from[0].table, "Todos");
        assert!(matches!(&s.items[0], Item::Column(c, _) if c.column == "Text"));
    }

    /// `and` binds tighter than `or`, which is SQL's precedence and the one a person expects.
    ///
    /// The negative half is the one that matters: the whole `where` is **one** term, because `or`
    /// is at the top of it. A `where` parsed the other way round — `(a or b) and c` — is two terms
    /// and `crate::query` would push one of them into a scan on its own, which answers a different
    /// question.
    #[test]
    fn and_binds_tighter_than_or() {
        let s = ok("select * from t where a = 1 or b = 2 and c = 3");
        assert_eq!(s.filter.len(), 1);
        let Expr::Or(branches) = &s.filter[0] else {
            panic!("not an `or`: {:?}", s.filter[0])
        };
        assert_eq!(branches.len(), 2);
        assert!(matches!(&branches[0], Expr::Cmp(..)));
        assert!(
            matches!(&branches[1], Expr::And(xs) if xs.len() == 2),
            "{:?}",
            branches[1]
        );
        // And `a and b or c` is `(a and b) or c` from the other side, still one term.
        let s = ok("select * from t where a = 1 and b = 2 or c = 3");
        assert_eq!(s.filter.len(), 1);
        assert!(matches!(&s.filter[0], Expr::Or(xs) if xs.len() == 2));
    }

    #[test]
    fn an_and_is_the_unit_a_where_is_pushed_down_as() {
        let s = ok("select * from t where a = 1 and b = 2 and c = 3");
        assert_eq!(s.filter.len(), 3);
    }

    #[test]
    fn a_negative_literal_is_one_number() {
        let s = ok("select * from t where n < -3");
        assert_eq!(literal_of(&s.filter[0]), &Some(Datum::Bigint(-3)));
    }

    #[test]
    fn an_escaped_quote_is_one_character() {
        let s = ok("select * from t where name = 'it''s'");
        assert_eq!(
            literal_of(&s.filter[0]),
            &Some(Datum::Text("it's".to_string()))
        );
        // And an escape string reads its escapes, which is what `E'\n'` is written for.
        let s = ok(r"select * from t where name = E'a\nb'");
        assert_eq!(
            literal_of(&s.filter[0]),
            &Some(Datum::Text("a\nb".to_string()))
        );
    }

    #[test]
    fn a_write_is_refused_by_name() {
        let e = parse("insert into todos values (1)").expect_err("refused");
        assert_eq!(e.code, "0A000");
        assert!(e.message.contains("read-only"), "{}", e.message);
    }

    #[test]
    fn an_aggregate_beside_a_column_needs_a_group_by() {
        let e = parse("select id, count(*) from todos").expect_err("refused");
        assert!(e.message.contains("group by"), "{}", e.message);
        // And with one, it is an ordinary query rather than a refusal.
        let s = ok("select id, count(*) from todos group by id");
        assert_eq!(s.group.len(), 1);
        assert!(crate::query::relational(&s));
    }

    #[test]
    fn a_second_statement_is_refused() {
        assert!(parse("select 1; select 2").is_err());
    }

    #[test]
    fn nulls_sort_last_and_compare_as_unknown() {
        let schema = Schema::default();
        let fields = vec![Field {
            column: Column {
                name: Arc::from("x"),
                ty: SqlTy::Bigint,
                nullable: true,
            },
            of: None,
        }];
        let ev = Eval::new(&schema, &fields, &NoRows);
        let term = |sql: &str| ok(&format!("select * from t where {sql}")).filter.remove(0);
        // A comparison against NULL is unknown, and unknown is neither true nor false.
        assert_eq!(ev.cell(&term("x = 1"), &[None]).expect("evaluates"), None);
        assert!(!ev.holds(&[term("x = 1")], &[None]).expect("evaluates"));
        assert!(!ev.holds(&[term("x <> 1")], &[None]).expect("evaluates"));
        // `is null` is the one comparison that answers about a NULL rather than propagating it.
        assert!(ev.holds(&[term("x is null")], &[None]).expect("evaluates"));
        assert!(!ev
            .holds(&[term("x is null")], &[Some(Datum::Bigint(1))])
            .expect("evaluates"));
        assert!(compare(&None, &Some(Datum::Bigint(1))).is_gt());
    }

    #[test]
    fn a_case_evaluates_the_arm_that_matches_and_no_other() {
        let schema = Schema::default();
        let fields = Vec::new();
        let ev = Eval::new(&schema, &fields, &NoRows);
        let one = |sql: &str| {
            let s = ok(sql);
            let Item::Expr(e, _) = &s.items[0] else {
                panic!("not an expression: {:?}", s.items[0])
            };
            ev.cell(e, &[]).expect("evaluates")
        };
        // The `else` names a function this read model does not have, and is never reached — which
        // is the property `psql`'s `case when c.reloftype = 0 then '' else …::regtype…` needs.
        assert_eq!(
            one("select case when 1 = 1 then 'yes' else no_such_function() end"),
            Some(Datum::Text("yes".into()))
        );
        // Both forms, and a `case` that matches nothing is null rather than an error.
        assert_eq!(
            one("select case 2 when 1 then 'a' when 2 then 'b' end"),
            Some(Datum::Text("b".into()))
        );
        assert_eq!(one("select case 9 when 1 then 'a' end"), None);
    }

    /// A regular expression is matched by a simulation, so a pattern that would make a
    /// backtracking matcher take exponential time takes linear time here instead.
    #[test]
    fn a_regular_expression_anchors_alternates_and_does_not_backtrack() {
        assert!(regex::matches("^(todos)$", "todos", false).expect("matches"));
        assert!(!regex::matches("^(todos)$", "my_todos", false).expect("matches"));
        assert!(regex::matches("^pg_", "pg_class", false).expect("matches"));
        assert!(!regex::matches("^pg_toast", "pg_class", false).expect("matches"));
        assert!(regex::matches("^(a|b)c$", "bc", false).expect("matches"));
        assert!(regex::matches("os", "todos", false).expect("matches"));
        assert!(regex::matches("^TODOS$", "todos", true).expect("matches"));
        assert!(regex::matches(r"^a\.b$", "a.b", false).expect("matches"));
        assert!(!regex::matches(r"^a\.b$", "axb", false).expect("matches"));
        assert!(regex::matches("^[a-c]+$", "abcabc", false).expect("matches"));
        assert!(!regex::matches("^[^a-c]+$", "abc", false).expect("matches"));
        // The one a backtracker cannot answer: 24 characters against a pattern whose branches
        // agree, which is `2^24` paths to try and one set of states to advance.
        assert!(!regex::matches("^(a|a)*b$", "aaaaaaaaaaaaaaaaaaaaaaaa", false).expect("matches"));
    }

    #[test]
    fn a_join_carries_its_tables_its_names_and_its_equality() {
        let s = ok(
            "select o.id, i.name from orders o join items as i on o.item = i.id \
             where i.stocked = true order by o.id limit 5",
        );
        assert_eq!(s.from.len(), 2);
        assert_eq!(
            (s.from[0].table.as_str(), s.from[0].alias.as_str()),
            ("orders", "o")
        );
        assert_eq!(
            (s.from[1].table.as_str(), s.from[1].alias.as_str()),
            ("items", "i")
        );
        assert_eq!(s.from[1].on.len(), 1);
        assert_eq!(s.from[1].on[0].0.to_string(), "o.item");
        assert_eq!(s.from[1].on[0].1.to_string(), "i.id");
        // The clauses after the join still parse: an alias must not swallow `where`.
        assert_eq!(s.filter.len(), 1);
        assert_eq!(s.order.len(), 1);
        assert!(
            matches!(&s.order[0].by, OrderBy::Expr(Expr::Column(n)) if n.to_string() == "o.id"),
            "{:?}",
            s.order[0].by
        );
        assert_eq!(s.limit, Some(5));
        assert!(crate::query::relational(&s));
    }

    #[test]
    fn a_bare_from_takes_the_tables_own_name_and_no_clause_becomes_an_alias() {
        for sql in [
            "select * from todos where done = true",
            "select * from todos order by text",
            "select count(*) from todos group by owner",
            "select * from todos limit 1",
        ] {
            let s = ok(sql);
            assert_eq!(s.from[0].alias, "todos", "{sql}");
        }
    }

    /// A `left join` keeps the rows the right side has no match for, and the two joins the
    /// left-deep `from` list cannot express are refused by name.
    #[test]
    fn a_left_join_parses_and_the_two_that_cannot_be_left_deep_are_refused() {
        let s = ok("select * from a left outer join b on a.k = b.k");
        assert_eq!(s.from.len(), 2);
        assert!(s.from[1].left);
        assert!(!s.from[0].left);
        for sql in [
            "select * from a right join b on a.k = b.k",
            "select * from a full outer join b on a.k = b.k",
            "select * from a natural join b",
        ] {
            let e = parse(sql).expect_err("refused");
            assert_eq!(e.code, "0A000", "{sql}");
        }
        // A comma join and a `cross join` are the same thing, and neither carries an `on`.
        for sql in [
            "select * from a, b where a.k = b.k",
            "select * from a cross join b",
        ] {
            let s = ok(sql);
            assert_eq!(s.from.len(), 2, "{sql}");
            assert!(s.from[1].on.is_empty(), "{sql}");
        }
        // A join on something that is not an equality says so rather than parsing as a filter.
        let e = parse("select * from a join b on a.k < b.k").expect_err("refused");
        assert!(e.message.contains("equality"), "{}", e.message);
    }

    #[test]
    fn a_catalogue_query_parses_the_way_psql_writes_one() {
        // The whole of `\d`'s list query, which is the shape everything here exists for.
        let s = ok("SELECT n.nspname as \"Schema\", c.relname as \"Name\", \
             CASE c.relkind WHEN 'r' THEN 'table' WHEN 'v' THEN 'view' END as \"Type\", \
             pg_catalog.pg_get_userbyid(c.relowner) as \"Owner\" \
             FROM pg_catalog.pg_class c \
             LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_catalog.pg_am am ON am.oid = c.relam \
             WHERE c.relkind IN ('r','p','v','m','S','f','') \
             AND n.nspname <> 'pg_catalog' AND n.nspname !~ '^pg_toast' \
             AND pg_catalog.pg_table_is_visible(c.oid) ORDER BY 1,2");
        assert_eq!(s.from.len(), 3);
        assert_eq!(s.from[0].namespace.as_deref(), Some("pg_catalog"));
        assert!(s.from[1].left && s.from[2].left);
        assert_eq!(s.filter.len(), 4);
        assert!(matches!(&s.filter[0], Expr::In { .. }));
        assert!(matches!(&s.filter[2], Expr::Match { negated: true, .. }));
        assert_eq!(s.order.len(), 2);
        assert!(matches!(s.order[0].by, OrderBy::Ordinal(1)));
        assert!(matches!(s.order[1].by, OrderBy::Ordinal(2)));
        // And the operator spelling `psql` uses so a search path cannot change what it means.
        let s = ok("select c.oid from pg_catalog.pg_class c \
             where c.relname OPERATOR(pg_catalog.~) '^(todos)$' COLLATE pg_catalog.default");
        assert!(matches!(&s.filter[0], Expr::Match { negated: false, .. }));
    }

    #[test]
    fn a_union_is_one_statement_and_its_order_by_is_the_unions() {
        let Stmt::Union {
            branches,
            all,
            order,
            ..
        } = parse("select a from t union select b from u order by 1").expect("parses")
        else {
            panic!("not a union")
        };
        assert_eq!(branches.len(), 2);
        assert!(!all);
        assert_eq!(order.len(), 1);
        assert!(branches[1].order.is_empty());
        assert!(parse("select 1 intersect select 2").is_err());
    }

    #[test]
    fn a_having_is_refused_and_says_what_a_where_does_instead() {
        let e = parse("select owner, count(*) from t group by owner having count(*) > 1")
            .expect_err("refused");
        assert!(
            e.message.contains("before they are grouped"),
            "{}",
            e.message
        );
    }

    #[test]
    fn only_a_query_that_relates_groups_or_deduplicates_needs_the_plan() {
        // The scan answers these, `count(*)` included: an arrangement already knows its size.
        for sql in [
            "select * from todos",
            "select count(*) from todos",
            "select count(*) from todos where done = false",
            "select 1",
        ] {
            assert!(!crate::query::relational(&ok(sql)), "{sql}");
        }
        for sql in [
            "select distinct owner from todos",
            "select owner, count(*) from todos group by owner",
            "select sum(amount) from postings",
            "select * from a join b on a.k = b.k",
        ] {
            assert!(crate::query::relational(&ok(sql)), "{sql}");
        }
    }
}
