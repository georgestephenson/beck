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
//! is a documented subset — a scan with a `where`, an `order by` and a `limit`; an inner equi-join;
//! a `group by` with `count`, `min`, `max` and `sum`; and `distinct` — and there are no subqueries,
//! no expressions and no `having`. It exists so that an outside tool can read what the program
//! holds, which is what §5.3's row is for.
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
    pub tables: Vec<Table>,
}

impl Schema {
    /// The name of the catalogue table. There is no `pg_catalog` here, and a client that cannot
    /// find out what exists cannot use what exists, so the schema is a table in itself.
    pub const CATALOGUE: &'static str = "beck_columns";

    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name.as_ref() == name)
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
        Schema { tables }
    }

    /// The catalogue's own rows: this schema, described.
    pub fn catalogue_rows(&self) -> Vec<Vec<Cell>> {
        let mut rows = Vec::new();
        for t in &self.tables {
            for (i, c) in t.columns.iter().enumerate() {
                rows.push(vec![
                    Some(Datum::Text(t.name.to_string())),
                    Some(Datum::Text(c.name.to_string())),
                    Some(Datum::Text(c.ty.name().to_string())),
                    Some(Datum::Boolean(c.nullable)),
                    Some(Datum::Bigint(i as i64 + 1)),
                ]);
            }
        }
        rows
    }

    /// The catalogue's own rows as **values**, which is what a query that joins it reads.
    ///
    /// [`Schema::catalogue_rows`] answers a scan and this answers the plan, and they are the same
    /// five columns because a table a join may name has to be the table a scan named. It is built
    /// on demand either way: the catalogue is a handful of rows describing a schema that cannot
    /// change while a process is running.
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
/// No expressions beyond a column, a literal and the four aggregates, because an expression
/// language is what [`04`](../../../../../docs/04-compiler-architecture.md) §4.2 says the `Query`
/// sub-language is *for* and this is not it.
///
/// What it does have is the **relational** half: several tables joined by an equality, a `group by`
/// with its aggregates, and `distinct`. None of those is interpreted here — [`crate::query`]
/// compiles them into a [`crate::plan::Plan`] and [`crate::engine`] runs it, so the join in a
/// `select` and the join in a `for` loop are one operator
/// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 9).
#[derive(Clone, Debug)]
pub struct Select {
    /// `select distinct` — the algebra's δ, and [`crate::plan::Op::Distinct`] is what answers it.
    pub distinct: bool,
    pub items: Vec<Item>,
    /// The tables, in the order they were written. Empty for `select 1`; one for a scan; more when
    /// the query joins, and every entry after the first carries the equality that joins it.
    pub from: Vec<From>,
    pub filter: Vec<Vec<Cond>>,
    /// `group by` — the columns whose distinct values are the output's rows.
    pub group: Vec<Name>,
    pub order: Option<(Name, bool)>,
    pub limit: Option<usize>,
    pub offset: usize,
}

/// One entry of the `from` list: a table, what this query calls it, and what joins it.
#[derive(Clone, Debug)]
pub struct From {
    /// The table's name in the schema.
    pub table: String,
    /// The name a qualified column reference uses — the alias, or the table's own name.
    pub alias: String,
    /// The `on` equalities, as (this table's column, an earlier table's column). Empty for the
    /// first entry, which nothing joins to.
    pub on: Vec<(Name, Name)>,
}

/// A column reference, qualified by a table's name in this query or not.
#[derive(Clone, Debug, PartialEq, Eq)]
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
}

impl Item {
    /// Whether this item is an aggregate, which is what decides that a query groups.
    pub fn aggregates(&self) -> bool {
        matches!(self, Item::Count(_) | Item::Aggregate(..))
    }
}

#[derive(Clone, Debug)]
pub struct Cond {
    pub column: Name,
    pub op: CmpOp,
    pub value: Option<Datum>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Is,
    IsNot,
}

/// A statement, which is a `select` or one of the two things a client says before it asks for
/// anything.
#[derive(Clone, Debug)]
pub enum Stmt {
    Select(Select),
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
        t.columns
            .iter()
            .map(|c| Field {
                column: c.clone(),
                of: Some(t.name.clone()),
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
        }
    }

    /// What a statement's result looks like, without running it. `Describe` needs this before
    /// `Execute` has happened.
    pub fn describe(&self, sql: &str) -> Result<Vec<Column>, SqlError> {
        match parse(sql)? {
            Stmt::Ignored(_) => Ok(Vec::new()),
            Stmt::Select(s) => {
                if crate::query::relational(&s) {
                    let compiled = crate::query::compile(self, &s)?;
                    if compiled.projected {
                        return Ok(compiled.fields.iter().map(|f| f.column.clone()).collect());
                    }
                    return Ok(self.project_columns(&s, &compiled.fields)?.0);
                }
                let fields = match self.resolve_from(&s)? {
                    Some(t) => Field::of_table(t),
                    None => Vec::new(),
                };
                Ok(self.project_columns(&s, &fields)?.0)
            }
        }
    }

    /// The one table a non-relational query reads, if it has one.
    fn resolve_from(&self, s: &Select) -> Result<Option<&Table>, SqlError> {
        match s.from.first() {
            None => Ok(None),
            Some(f) => match self.table(&f.table) {
                Some(t) => Ok(Some(t)),
                None => Err(SqlError::no_table(format!(
                    "there is no read model called \"{}\". `select * from {}` lists what there is",
                    f.table,
                    Schema::CATALOGUE
                ))),
            },
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
            );
        }

        let table = self.resolve_from(s)?;
        let fields = match table {
            Some(t) => Field::of_table(t),
            None => Vec::new(),
        };
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
                && s.order.is_none()
                && s.limit.is_none()
                && s.offset == 0;
            if bare {
                if let Some(n) = rows_of.count(t)? {
                    let n = match t.cardinality {
                        Cardinality::Many => n,
                        Cardinality::One => n.min(1),
                    };
                    let row: Vec<Cell> = proj
                        .iter()
                        .map(|p| match p {
                            Proj::Count => Some(Datum::Bigint(n as i64)),
                            Proj::Literal(d) => Some(d.clone()),
                            Proj::Column(_) => None,
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
            let row: Vec<Cell> = proj
                .iter()
                .map(|p| match p {
                    Proj::Literal(d) => Some(d.clone()),
                    Proj::Count => Some(Datum::Bigint(1)),
                    Proj::Column(_) => None,
                })
                .collect();
            return Ok(Answer {
                tag: "SELECT 1".to_string(),
                columns,
                rows: vec![row],
            });
        };

        let rows: Vec<Vec<Cell>> = match &t.source {
            Source::Catalogue => self.catalogue_rows(),
            _ => {
                let values = rows_of.scan(t)?;
                match t.cardinality {
                    Cardinality::Many => values.iter().map(|v| t.row(v)).collect(),
                    // A singleton is one row even when the reader hands over the value inside a
                    // one-element list, which is what `elements` does with a record.
                    Cardinality::One => values.iter().take(1).map(|v| t.row(v)).collect(),
                }
            }
        };
        self.finish(s, &fields, rows, &s.filter, false)
    }

    /// Filter, order, project, and cut — the half of a `select` that is the same whether the rows
    /// were scanned out of a collection or produced by the plan's operators.
    fn finish(
        &self,
        s: &Select,
        fields: &[Field],
        mut rows: Vec<Vec<Cell>>,
        filter: &[Vec<Cond>],
        projected: bool,
    ) -> Result<Answer, SqlError> {
        // Filter, then order, then offset and limit — the order SQL specifies, and the order that
        // makes `limit` mean what a person expects.
        for disjunction in filter {
            let mut tested = Vec::with_capacity(rows.len());
            for row in rows {
                if any_matches(fields, disjunction, &row)? {
                    tested.push(row);
                }
            }
            rows = tested;
        }
        if let Some((name, asc)) = &s.order {
            let i = resolve_field(fields, name).map_err(|e| match projected {
                // A grouped or deduplicated query has only the columns it selected, and saying
                // *that* is more use than "no such column": the column may well exist in the table
                // and simply not have survived the operator.
                true => SqlError::no_column(format!(
                    "cannot order by \"{name}\": this query groups or is distinct, so it can only \
                     be ordered by something it selected — {}",
                    names_of(fields)
                )),
                false => e,
            })?;
            // Stable, so the order the collection holds its elements in survives ties — which is
            // the arrangement's key order, and therefore the order the page renders in.
            rows.sort_by(|a, b| {
                let o = compare(&a[i], &b[i]);
                if *asc {
                    o
                } else {
                    o.reverse()
                }
            });
        }
        if s.offset > 0 {
            rows = rows.split_off(s.offset.min(rows.len()));
        }
        if let Some(n) = s.limit {
            rows.truncate(n);
        }

        // The plan already projected: its operators were compiled from the select list, so the rows
        // are the answer and there is nothing left to pick out of them.
        if projected {
            return Ok(Answer {
                tag: format!("SELECT {}", rows.len()),
                columns: fields.iter().map(|f| f.column.clone()).collect(),
                rows,
            });
        }
        let (columns, proj) = self.project_columns(s, fields)?;

        // `count(*)` collapses. Mixing it with a column would be a group-by, which this path has
        // none of, so it is refused at parse time rather than answered wrongly.
        let out: Vec<Vec<Cell>> = if proj.iter().any(|p| matches!(p, Proj::Count)) {
            vec![proj
                .iter()
                .map(|p| match p {
                    Proj::Count => Some(Datum::Bigint(rows.len() as i64)),
                    Proj::Literal(d) => Some(d.clone()),
                    Proj::Column(_) => None,
                })
                .collect()]
        } else {
            rows.iter()
                .map(|r| {
                    proj.iter()
                        .map(|p| match p {
                            Proj::Column(i) => r[*i].clone(),
                            Proj::Literal(d) => Some(d.clone()),
                            Proj::Count => None,
                        })
                        .collect()
                })
                .collect()
        };
        Ok(Answer {
            tag: format!("SELECT {}", out.len()),
            columns,
            rows: out,
        })
    }
}

enum Proj {
    Column(usize),
    Count,
    Literal(Datum),
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

fn any_matches(fields: &[Field], conds: &[Cond], row: &[Cell]) -> Result<bool, SqlError> {
    for c in conds {
        let i = resolve_field(fields, &c.column)?;
        if matches(&row[i], c) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether one cell satisfies one condition.
pub fn matches(cell: &Cell, c: &Cond) -> bool {
    match c.op {
        CmpOp::Is => cell.is_none() == c.value.is_none() && (c.value.is_none() || equal(cell, c)),
        CmpOp::IsNot => {
            !(cell.is_none() == c.value.is_none() && (c.value.is_none() || equal(cell, c)))
        }
        // Three-valued logic, in the one place it shows up: a comparison against NULL is unknown,
        // and unknown is not true.
        _ => match (cell, &c.value) {
            (Some(a), Some(b)) => {
                let o = compare(&Some(a.clone()), &Some(b.clone()));
                match c.op {
                    CmpOp::Eq => o.is_eq(),
                    CmpOp::Ne => o.is_ne(),
                    CmpOp::Lt => o.is_lt(),
                    CmpOp::Le => o.is_le(),
                    CmpOp::Gt => o.is_gt(),
                    CmpOp::Ge => o.is_ge(),
                    CmpOp::Is | CmpOp::IsNot => false,
                }
            }
            _ => false,
        },
    }
}

fn equal(cell: &Cell, c: &Cond) -> bool {
    compare(cell, &c.value).is_eq()
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
            // compared against a column of another type does.
            _ => x.text().cmp(&y.text()),
        },
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
            // Two-character operators first, so `<=` does not lex as `<` then `=`.
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            if matches!(two.as_str(), "<=" | ">=" | "<>" | "!=") {
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
}

impl P {
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
    let mut p = P { toks, i: 0 };
    let head = p.keyword().unwrap_or_default();
    match head.as_str() {
        "select" => {
            p.i += 1;
            let s = select(&mut p)?;
            // A trailing `;` is a statement separator, and a second statement is not supported —
            // saying so beats answering the first and dropping the rest.
            p.eat_sym(";");
            if p.peek().is_some() {
                return Err(SqlError::unsupported(
                    "one statement per query: this SQL has no multi-statement form",
                ));
            }
            Ok(Stmt::Select(s))
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
        from.push(From {
            table: p
                .name()
                .ok_or_else(|| SqlError::syntax("`from` wants a table name"))?,
            alias: String::new(),
            on: Vec::new(),
        });
        from[0].alias = table_alias(p)?.unwrap_or_else(|| from[0].table.clone());
        loop {
            // The joins this SQL has, and the ones it says it does not. An outer join keeps a left
            // row that matched nothing, which is a row this query would have to invent columns for;
            // the operator underneath answers with the group and an empty group is no rows, so
            // saying so is better than a `left join` that quietly means `join`.
            if p.eat_keyword("cross") || p.eat_keyword("natural") {
                return Err(SqlError::unsupported(
                    "the joins here are `join … on <equality>`; a cross or natural join names no \
                     key, and every join in this SQL is an equi-join because that is the operator \
                     underneath it",
                ));
            }
            for outer in ["left", "right", "full", "outer"] {
                if p.keyword().as_deref() == Some(outer) {
                    return Err(SqlError::unsupported(format!(
                        "`{outer} join` is not in this SQL subset: an unmatched row would need \
                         columns invented for it, and `join … on` is the inner join this has"
                    )));
                }
            }
            p.eat_keyword("inner");
            if !p.eat_keyword("join") {
                break;
            }
            let table = p
                .name()
                .ok_or_else(|| SqlError::syntax("`join` wants a table name"))?;
            let alias = table_alias(p)?.unwrap_or_else(|| table.clone());
            if !p.eat_keyword("on") {
                return Err(SqlError::syntax(format!(
                    "`join {table}` wants `on <column> = <column>`"
                )));
            }
            let mut on = Vec::new();
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
                on.push((left, right));
                if !p.eat_keyword("and") {
                    break;
                }
            }
            from.push(From { table, alias, on });
        }
    }

    let mut filter = Vec::new();
    if p.eat_keyword("where") {
        filter = where_clause(p)?;
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

    let mut order = None;
    if p.eat_keyword("order") {
        if !p.eat_keyword("by") {
            return Err(SqlError::syntax("`order` wants `by`"));
        }
        let col = column_name(p).ok_or_else(|| SqlError::syntax("`order by` wants a column"))?;
        let asc = if p.eat_keyword("desc") {
            false
        } else {
            p.eat_keyword("asc");
            true
        };
        order = Some((col, asc));
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
        | Some("full") | Some("outer") | Some("cross") | Some("natural") | Some("on") | None => {
            Ok(None)
        }
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
    if let Some(lit) = p.literal() {
        let d = lit.ok_or_else(|| SqlError::unsupported("`select null` has no column type"))?;
        return Ok(Item::Literal(d, alias(p)));
    }
    let name = p
        .name()
        .ok_or_else(|| SqlError::syntax("a select list wants a column, `*`, or a literal"))?;
    // `t.*` and `t.c`: the qualifier is read here rather than in [`column_name`] because a select
    // item may be a call, and `count` is a name until the `(` says otherwise.
    if p.eat_sym(".") {
        if p.eat_sym("*") {
            return Ok(Item::All(Some(name)));
        }
        let column = p
            .name()
            .ok_or_else(|| SqlError::syntax(format!("`{name}.` wants a column or `*`")))?;
        return Ok(Item::Column(
            Name {
                table: Some(name),
                column,
            },
            alias(p),
        ));
    }
    if p.eat_sym("(") {
        // The three zero-argument functions a client asks before it trusts a connection, and the
        // four aggregates. Every one of the four is a plan operator rather than a loop here:
        // `count` is the join's own tally and the other three are [`crate::plan::Op::GroupBy`]'s
        // (docs/99 §99.9 item 6).
        let f = match name.as_str() {
            "count" => {
                if !p.eat_sym("*") {
                    return Err(SqlError::unsupported(
                        "`count` counts rows here: `count(*)` is the only form, and `count(c)` \
                         would be a count of the rows whose `c` is not null",
                    ));
                }
                Item::Count(None)
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
                Item::Aggregate(agg, column, None)
            }
            "version" => Item::Literal(Datum::Text(version()), None),
            "current_database" | "current_schema" | "current_catalog" => {
                Item::Literal(Datum::Text("beck".into()), None)
            }
            other => {
                return Err(SqlError::unsupported(format!(
                    "`{other}(…)` is not a function this read model has"
                )))
            }
        };
        if !p.eat_sym(")") {
            return Err(SqlError::syntax("a call is not closed"));
        }
        let a = alias(p);
        return Ok(match f {
            Item::Count(_) => Item::Count(a),
            Item::Aggregate(agg, c, _) => Item::Aggregate(agg, c, a),
            Item::Literal(d, _) => Item::Literal(d, a.or(Some(name))),
            other => other,
        });
    }
    Ok(Item::Column(Name::bare(name), alias(p)))
}

fn alias(p: &mut P) -> Option<String> {
    if p.eat_keyword("as") {
        return p.name();
    }
    // A bare alias, but not one of the words that ends a select item.
    match p.keyword().as_deref() {
        Some("from") | Some("where") | Some("group") | Some("having") | Some("order")
        | Some("limit") | Some("offset") | Some("as") | None => None,
        Some(_) => p.name(),
    }
}

/// `a = 1 and b = 2 or c = 3`, as a conjunction of disjunctions.
///
/// `and` binds tighter than `or` in SQL, so the natural reading of the parse is the other way
/// round; this collects disjunctive groups and requires every group to have a true member, which
/// is the same thing said so the evaluator is a loop.
fn where_clause(p: &mut P) -> Result<Vec<Vec<Cond>>, SqlError> {
    let mut and_groups: Vec<Vec<Cond>> = vec![vec![cond(p)?]];
    loop {
        if p.eat_keyword("and") {
            and_groups.push(vec![cond(p)?]);
        } else if p.eat_keyword("or") {
            let c = cond(p)?;
            and_groups
                .last_mut()
                .expect("there is always one group")
                .push(c);
        } else {
            return Ok(and_groups);
        }
    }
}

fn cond(p: &mut P) -> Result<Cond, SqlError> {
    if p.eat_sym("(") {
        return Err(SqlError::unsupported(
            "a parenthesised condition is not in this SQL subset",
        ));
    }
    let column = column_name(p).ok_or_else(|| SqlError::syntax("`where` wants a column"))?;
    let op = if p.eat_keyword("is") {
        if p.eat_keyword("not") {
            CmpOp::IsNot
        } else {
            CmpOp::Is
        }
    } else if p.eat_sym("=") {
        CmpOp::Eq
    } else if p.eat_sym("<>") || p.eat_sym("!=") {
        CmpOp::Ne
    } else if p.eat_sym("<=") {
        CmpOp::Le
    } else if p.eat_sym(">=") {
        CmpOp::Ge
    } else if p.eat_sym("<") {
        CmpOp::Lt
    } else if p.eat_sym(">") {
        CmpOp::Gt
    } else {
        return Err(SqlError::unsupported(format!(
            "the comparisons here are =, <>, <, <=, >, >= and `is null`; \
             \"{column}\" is followed by none of them"
        )));
    };
    let value = p
        .literal()
        .ok_or_else(|| SqlError::syntax(format!("\"{column}\" is compared against nothing")))?;
    Ok(Cond { column, op, value })
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

    #[test]
    fn a_select_is_case_folded_and_a_quoted_name_is_not() {
        let s = ok("SELECT Text FROM Todos");
        assert_eq!(s.from[0].table, "todos");
        assert!(matches!(&s.items[0], Item::Column(c, _) if c.column == "text"));
        let s = ok(r#"select "Text" from "Todos""#);
        assert_eq!(s.from[0].table, "Todos");
        assert!(matches!(&s.items[0], Item::Column(c, _) if c.column == "Text"));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // `a or b and c` is `(a or b) and c` — two groups, the first with two members.
        let s = ok("select * from t where a = 1 or b = 2 and c = 3");
        assert_eq!(s.filter.len(), 2);
        assert_eq!(s.filter[0].len(), 2);
        assert_eq!(s.filter[1].len(), 1);
    }

    #[test]
    fn a_negative_literal_is_one_number() {
        let s = ok("select * from t where n < -3");
        assert_eq!(s.filter[0][0].value, Some(Datum::Bigint(-3)));
    }

    #[test]
    fn an_escaped_quote_is_one_character() {
        let s = ok("select * from t where name = 'it''s'");
        assert_eq!(s.filter[0][0].value, Some(Datum::Text("it's".to_string())));
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
        let c = Cond {
            column: Name::bare("x"),
            op: CmpOp::Eq,
            value: Some(Datum::Bigint(1)),
        };
        assert!(!matches(&None, &c));
        let is_null = Cond {
            column: Name::bare("x"),
            op: CmpOp::Is,
            value: None,
        };
        assert!(matches(&None, &is_null));
        assert!(!matches(&Some(Datum::Bigint(1)), &is_null));
        assert!(compare(&None, &Some(Datum::Bigint(1))).is_gt());
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
        assert_eq!(
            s.order.as_ref().map(|(n, _)| n.to_string()).as_deref(),
            Some("o.id")
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

    #[test]
    fn an_outer_join_is_refused_by_name_rather_than_read_as_an_inner_one() {
        for sql in [
            "select * from a left join b on a.k = b.k",
            "select * from a full outer join b on a.k = b.k",
            "select * from a cross join b",
        ] {
            let e = parse(sql).expect_err("refused");
            assert_eq!(e.code, "0A000", "{sql}");
        }
        // A join on something that is not an equality says so rather than parsing as a filter.
        let e = parse("select * from a join b on a.k < b.k").expect_err("refused");
        assert!(e.message.contains("equality"), "{}", e.message);
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
