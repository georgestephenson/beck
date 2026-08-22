//! The read model's relational half: a `select` compiled into the plan.
//!
//! [`docs/99-the-data-tier-means-of-combination.md`](../../../../../docs/99-the-data-tier-means-of-combination.md)
//! §99.9 item 9:
//!
//! > The read-model SQL grows joins and `group by` **by compiling into the plan**, not by growing
//! > its own interpreter — which closes §23.19 and §12.5 together and keeps one code path.
//!
//! # What this module is, in one sentence
//!
//! It writes the Beck expression a person would have written, and hands it to
//! [`crate::plan::Plan::of_query`]. Nothing here joins anything, groups anything or deduplicates
//! anything: `select … join … on b.k = a.k` becomes the loop `for a in as: for b in bs where
//! b.k == a.k`, which [`crate::relate`] already reads as an equi-join over an index, and `group by
//! g` becomes the loop over `list_unique` that `corpus/35-workload.beck` writes by hand. The
//! operators are [`crate::plan::Op::Join`], [`crate::plan::Op::ArrangeBy`],
//! [`crate::plan::Op::GroupBy`] and [`crate::plan::Op::Distinct`] — the same ones, with the same
//! delta rules, that a program's view compiles to.
//!
//! That is the whole of item 9, and it is worth naming what the alternative would have cost: a
//! second join, a second set of aggregates and a second `distinct` living beside the first,
//! agreeing by inspection rather than by construction, and a differential harness that covers one
//! of the two.
//!
//! # The shape each query compiles to
//!
//! | SQL | The expression |
//! |---|---|
//! | `from a join b on b.k = a.k` | `concat_lists(map_list(a, λx. map_list(filter_list(b, λy. y.k == x.k), λy. row)))` — one stage per join, left-deep, so each stage is its own `map_list` in the plan and therefore gets its own join and its own index |
//! | `group by g` | `map_list(list_unique(map_list(R, λr. g(r))), λk. …)`, each aggregate a question about `filter_list(R, λr. g(r) == k)` |
//! | `count(*)` per group | `list_len` of that filter — the join's own tally, so no group is built |
//! | `min/max/sum(c)` per group | `list_min(map_list(that filter, λr. r.c))` — [`crate::plan::Op::GroupBy`] |
//! | `distinct` | `list_unique` of the projected row |
//! | an aggregate with no `group by` | the aggregate of the whole collection, as one row |
//!
//! # What a column is, and why the rows are normalised first
//!
//! A table's columns are a schema fact; a table's element is a run-time value — a record for a
//! collection of models, the element itself for a collection of scalars. Rather than teach the
//! compiled expression that difference, every table's rows go through [`Table::row_values`] first,
//! which is the function [`Table::row`] builds a scan's cells with. So a column is `c{n}` inside
//! the plan, for every table, and the scan and the join cannot disagree about what a column is.
//!
//! # What is deliberately not here
//!
//! * **No cost-based ordering.** The `from` list fixes the left-deep order, exactly as a `for` loop
//!   fixes it ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.8's
//!   "an inferred surface postpones the solver", arrived at from the other side: a *written*
//!   surface fixes the order outright, so there is still nothing for a solver to choose).
//! * **No outer join**, because an unmatched row would need columns invented for it.
//! * **No `having`**: a `where` narrows the rows before they are grouped, and a filter over the
//!   groups themselves would be a second predicate language.

use std::collections::BTreeSet;
use std::sync::Arc;

use beck_diag::Span;

use crate::core::{Const, Core, CoreKind, Fields, Prim, Value, VarId};
use crate::engine::{Engine, Prepared};
use crate::plan::{Agg, Plan, Relate};
use crate::read::{
    self, Cell, Column, Cond, Datum, Field, Item, Name, Schema, Select, SqlError, SqlTy, Table,
};
use crate::ty::{Tier, Ty};

/// A conjunction of disjunctions — the shape [`crate::read`] parses a `where` into, because `and`
/// binds tighter than `or`.
type Filter = Vec<Vec<Cond>>;

/// One record per row, as the fields of the [`CoreKind::Make`] that builds it.
type Row = Vec<(Arc<str>, Core)>;

/// A `select`'s relational half, compiled and ready to run.
pub struct Compiled {
    /// What the rows this produces are called, in order.
    pub fields: Vec<Field>,
    /// Whether those rows are already the select list's answer.
    ///
    /// True for a `group by` and for a `distinct`, because the operator that produced them was
    /// compiled *from* that list — the projection happened inside the plan. False for a plain join,
    /// whose rows carry every column of every table so that a `where` or an `order by` can name one
    /// the select list does not.
    pub projected: bool,
    /// The `where` groups this did **not** apply, for the caller to apply to the rows.
    ///
    /// A condition names one column and therefore one table, so a disjunctive group whose
    /// conditions all name the same table is pushed into that table's scan. One that spans two
    /// tables cannot be, and is left here.
    pub residual: Filter,
    /// The tables the plan reads, in the order its state record holds them.
    inputs: Vec<Input>,
    plan: Plan,
    /// Whether the plan's root is one row rather than a collection of them — an aggregate with no
    /// `group by` is a question about the whole table, and its answer is one row.
    single: bool,
}

/// One table the compiled plan reads.
struct Input {
    table: Arc<str>,
    /// Where this table's columns start in the row the plan reads, so `c{n}` names the same column
    /// everywhere in it.
    base: usize,
    prefilter: Filter,
}

impl Compiled {
    /// The plan this query compiles to — what `beck explain` prints, and what a gate reads.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Read the tables, run the plan, and answer with the rows.
    pub fn run(&self, schema: &Schema, rows: &dyn read::Rows) -> Result<Vec<Vec<Cell>>, SqlError> {
        Ok(self.run_measured(schema, rows)?.0)
    }

    /// The same, and what the engine did to produce it.
    ///
    /// [`crate::engine::Work`] counts applications, entries touched and operators recomputed, so a
    /// gate can say "answering this join did not reconsider every pair" without a clock in it —
    /// which is what `scaling.rs` asserts about a program's loops and now asserts about a query's.
    pub fn run_measured(
        &self,
        schema: &Schema,
        rows: &dyn read::Rows,
    ) -> Result<(Vec<Vec<Cell>>, crate::engine::Work), SqlError> {
        let backend = rows.backend().ok_or_else(|| SqlError {
            message: "this reader has no executor behind it, so a join, a `group by` and a \
                      `distinct` cannot be answered: they are compiled into the view plan, and a \
                      plan is prepared by a backend"
                .to_string(),
            code: "0A000",
        })?;

        let mut state = Fields::with_capacity(self.inputs.len());
        for (i, input) in self.inputs.iter().enumerate() {
            let table = schema.table(&input.table).ok_or_else(|| {
                SqlError::no_table(format!("there is no read model called \"{}\"", input.table))
            })?;
            let values = scan(schema, table, rows)?;
            let mut out = Vec::with_capacity(values.len());
            for v in &values {
                if !input.prefilter.is_empty() && !kept(table, v, &input.prefilter)? {
                    continue;
                }
                out.push(normalise(table, v, input.base));
            }
            state.insert(Arc::from(table_field(i)), Value::List(Arc::new(out)));
        }
        let state = Value::data("Query", None, state);

        let prepared = Prepared::new(Arc::new(self.plan.clone()), backend).map_err(exec)?;
        let mut engine = Engine::new(Arc::new(prepared));
        let out = engine
            .render(&state, &Value::Unit, &Value::Unit)
            .map_err(exec)?;

        let values: Vec<Value> = match (&out, self.single) {
            (_, true) => vec![out.clone()],
            (Value::List(xs), false) => xs.as_ref().clone(),
            (Value::Map(m), false) => m.iter().map(|(_, v)| v.clone()).collect(),
            (other, false) => vec![(*other).clone()],
        };
        let out = values
            .iter()
            .map(|v| {
                self.fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| match v.field(&field_name(i)) {
                        Some(x) => read::cell_of(x, &f.column),
                        None => None,
                    })
                    .collect()
            })
            .collect();
        Ok((out, engine.work()))
    }
}

fn exec(e: crate::backend::ExecError) -> SqlError {
    SqlError {
        message: format!("the query's operators could not be run: {e}"),
        code: "58000",
    }
}

/// A table's rows, from wherever that table's rows come from.
fn scan(schema: &Schema, table: &Table, rows: &dyn read::Rows) -> Result<Vec<Value>, SqlError> {
    match table.source {
        // The catalogue is built rather than scanned, and it is a table a join may name: "which
        // tables have a column called `id`" is a self-join over it, and it is the nearest thing
        // this read model has to `pg_catalog`.
        read::Source::Catalogue => Ok(schema.catalogue_values()),
        _ => rows.scan(table),
    }
}

/// Whether a row survives the `where` groups pushed into its own table's scan.
fn kept(table: &Table, v: &Value, groups: &Filter) -> Result<bool, SqlError> {
    let cells = table.row(v);
    for group in groups {
        let mut any = false;
        for c in group {
            let (i, _) = table.column(&c.column.column).ok_or_else(|| {
                SqlError::no_column(format!(
                    "\"{}\" has no column \"{}\"",
                    table.name, c.column.column
                ))
            })?;
            if read::matches(&cells[i], c) {
                any = true;
                break;
            }
        }
        if !any {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One element as the record the plan reads columns out of: `c{base+i}` per column.
fn normalise(table: &Table, v: &Value, base: usize) -> Value {
    Value::data(
        "Row",
        None,
        table
            .row_values(v)
            .into_iter()
            .enumerate()
            .map(|(i, v)| (Arc::from(field_name(base + i)), v))
            .collect(),
    )
}

/// What a column is called inside the plan, and inside the record it produces.
fn field_name(i: usize) -> String {
    format!("c{i}")
}

/// What a table is called in the record the plan is handed as its state.
fn table_field(i: usize) -> String {
    format!("t{i}")
}

// -------------------------------------------------------------------------------------------
// Deciding whether the plan is needed at all
// -------------------------------------------------------------------------------------------

/// Whether a `select` needs the plan.
///
/// The boundary is which *operators* the query wants, not how big it is. A scan with a `where` and
/// an `order by` is what [`crate::read`] has always answered directly and it keeps answering it —
/// `select count(*)` included, whose whole point is that a maintained arrangement already knows its
/// own size ([`docs/23`](../../../../../docs/23-incremental-views-report.md) §23.19) and that
/// compiling a plan to rediscover it would be work for nothing. What comes here is a query that
/// **relates, groups or deduplicates**, because those are the three things the plan has operators
/// for and this module has none.
pub fn relational(s: &Select) -> bool {
    s.from.len() > 1
        || !s.group.is_empty()
        || s.distinct
        || s.items.iter().any(|i| matches!(i, Item::Aggregate(..)))
}

// -------------------------------------------------------------------------------------------
// The compiler
// -------------------------------------------------------------------------------------------

/// Compile a `select` into a plan over the tables it names.
pub fn compile(schema: &Schema, s: &Select) -> Result<Compiled, SqlError> {
    compile_with(schema, s, Relate::default())
}

/// The same, with [`Relate`] said out loud.
///
/// [`docs/08`](../../../../../docs/08-roadmap.md) §8.3 item 8's off switch reaches this surface for
/// the reason it reaches a program's: the recognition is what turns a nested loop into an indexed
/// join, and a default nobody has run is a claim. [`Relate::Refuse`] compiles the same expression
/// to the nested loop it literally is, which is what a gate measures the operator against.
pub fn compile_with(schema: &Schema, s: &Select, relate: Relate) -> Result<Compiled, SqlError> {
    let mut q = Query {
        schema,
        entries: Vec::new(),
        fields: Vec::new(),
        fresh: 0,
    };
    q.resolve_from(s)?;
    if q.entries.is_empty() {
        return Err(SqlError::syntax(
            "a join, a `group by` and a `distinct` are all questions about a table, and this \
             query has no `from`",
        ));
    }
    q.fresh = q.entries.len() as VarId;

    let (residual, prefilters) = q.push_down(s)?;
    let grouping = !s.group.is_empty() || s.items.iter().any(Item::aggregates);
    if !residual.is_empty() && (grouping || s.distinct) {
        return Err(SqlError::unsupported(format!(
            "a `where` that spans two tables cannot be applied before a `{}`: every condition here \
             narrows one table, and an `or` across two of them would have to be a filter over the \
             joined rows",
            match grouping {
                true => "group by",
                false => "distinct",
            }
        )));
    }

    // The state's fields are positional rather than named after the tables: a query may join a
    // table to itself, and two fields with one name is not a record.
    let tables: Vec<Arc<str>> = (0..q.entries.len())
        .map(|i| Arc::from(table_field(i)))
        .collect();
    let rows = q.rows_expression();

    // The stages below read the joined rows more than once — a `group by` reads them once for its
    // keys and once per aggregate — so the rows are **bound** rather than rebuilt. A `let` is what
    // makes that one node with several consumers rather than several nodes computing the same
    // thing, which is §5.3's sharing at the granularity the plan shares at.
    let bound = q.fresh();
    let rows_var = var(bound);

    let (body, fields, projected, single) = if grouping {
        let (body, fields) = q.grouped(s, bound)?;
        (body, fields, true, s.group.is_empty())
    } else if s.distinct {
        let param = q.fresh();
        let (row, fields) = q.project(s, param)?;
        let body = prim(
            Prim::ListUnique,
            vec![prim(
                Prim::MapList,
                vec![rows_var.clone(), lam(vec![param], make(row))],
            )],
        );
        (body, fields, true, false)
    } else {
        (rows_var.clone(), q.fields.clone(), false, false)
    };
    // `distinct` over a grouped query is `list_unique` over the groups, which is what the two words
    // mean together; the operator is the same one either way.
    let body = match s.distinct && grouping && !single {
        true => prim(Prim::ListUnique, vec![body]),
        false => body,
    };

    let body = bind(bound, rows, body);
    let plan = Plan::of_query_with(&tables, &body, relate);
    let inputs = q
        .entries
        .iter()
        .zip(prefilters)
        .map(|(e, prefilter)| Input {
            table: e.table.name.clone(),
            base: e.base,
            prefilter,
        })
        .collect();
    Ok(Compiled {
        fields,
        projected,
        residual,
        inputs,
        plan,
        single,
    })
}

/// One `from` entry, resolved.
struct Entry<'a> {
    alias: Arc<str>,
    table: &'a Table,
    /// Where this table's columns start in the wide row.
    base: usize,
    /// The `on` equalities as (this table's column, an earlier table's column).
    on: Vec<(usize, usize)>,
}

struct Query<'a> {
    schema: &'a Schema,
    entries: Vec<Entry<'a>>,
    /// Every column of every table, in `from` order — the row a join produces.
    fields: Vec<Field>,
    fresh: VarId,
}

impl<'a> Query<'a> {
    fn fresh(&mut self) -> VarId {
        self.fresh += 1;
        self.fresh - 1
    }

    /// Resolve the `from` list: the tables, their names in this query, and where each one's columns
    /// sit in the row a join produces.
    fn resolve_from(&mut self, s: &'a Select) -> Result<(), SqlError> {
        for (i, f) in s.from.iter().enumerate() {
            let table = self.schema.table(&f.table).ok_or_else(|| {
                SqlError::no_table(format!(
                    "there is no read model called \"{}\"; `select * from {}` lists what there is",
                    f.table,
                    Schema::CATALOGUE
                ))
            })?;
            if self.entries.iter().any(|e| e.alias.as_ref() == f.alias) {
                return Err(SqlError::syntax(format!(
                    "\"{}\" is in this `from` twice; give one of them a name (`{} as x`)",
                    f.alias, f.table
                )));
            }
            let base = self.fields.len();
            self.fields.extend(table.columns.iter().map(|c| Field {
                column: c.clone(),
                of: Some(Arc::from(f.alias.as_str())),
            }));
            self.entries.push(Entry {
                alias: Arc::from(f.alias.as_str()),
                table,
                base,
                on: Vec::new(),
            });
            // Resolved after the entry exists, so a join may name its own columns.
            let mut on = Vec::new();
            for (l, r) in &f.on {
                let (li, ri) = (self.resolve(l)?, self.resolve(r)?);
                let pair = match (li >= base, ri >= base) {
                    (true, false) => (li, ri),
                    (false, true) => (ri, li),
                    _ => {
                        return Err(SqlError::unsupported(format!(
                            "`on {l} = {r}` does not join \"{}\" to a table before it: one side \
                             has to be a column of \"{}\" and the other a column of a table \
                             already in the `from`",
                            f.alias, f.alias
                        )))
                    }
                };
                // Two conditions on a join key, and both are the SQL semantics this operator does
                // not have rather than an implementation gap. `==` here is [`Value`]'s own equality
                // — the order the index is a `BTreeMap` in — so a `NULL` would equal a `NULL`,
                // where SQL says it equals nothing; and `Int` and `Float` are different values,
                // where SQL would coerce. Both are refused rather than answered differently from
                // every other database.
                let (a, b) = (&self.fields[pair.0], &self.fields[pair.1]);
                if a.column.nullable || b.column.nullable {
                    return Err(SqlError::unsupported(format!(
                        "`on {l} = {r}` joins on a column that can be null, and this join's \
                         equality is the index's own: a null would match a null, where SQL says a \
                         null matches nothing"
                    )));
                }
                if a.column.ty != b.column.ty {
                    return Err(SqlError::unsupported(format!(
                        "`on {l} = {r}` compares {} with {}, and this join's equality does not \
                         coerce between them: give the two columns the same type",
                        a.column.ty.name(),
                        b.column.ty.name()
                    )));
                }
                on.push(pair);
            }
            if i > 0 && on.is_empty() {
                return Err(SqlError::syntax(format!(
                    "`join {}` wants `on <column> = <column>`",
                    f.table
                )));
            }
            self.entries[i].on = on;
        }
        Ok(())
    }

    /// A column reference, as an index into the wide row.
    fn resolve(&self, n: &Name) -> Result<usize, SqlError> {
        let matching: Vec<usize> = (0..self.fields.len())
            .filter(|&i| {
                let f = &self.fields[i];
                f.column.name.as_ref() == n.column
                    && match &n.table {
                        Some(t) => f.of.as_deref() == Some(t.as_str()),
                        None => true,
                    }
            })
            .collect();
        match matching.as_slice() {
            [one] => Ok(*one),
            [] => Err(SqlError::no_column(match &n.table {
                Some(t) if !self.entries.iter().any(|e| e.alias.as_ref() == t.as_str()) => {
                    format!(
                        "\"{t}\" is not a table in this query; it has {}",
                        self.entries
                            .iter()
                            .map(|e| format!("\"{}\"", e.alias))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
                _ => format!(
                    "there is no column \"{n}\" here; there is {}",
                    read::names_of(&self.fields)
                ),
            })),
            _ => Err(SqlError::no_column(format!(
                "\"{n}\" is ambiguous: more than one table in this query has a column called \
                 \"{}\", so qualify it — `t.{}`",
                n.column, n.column
            ))),
        }
    }

    /// Split the `where` into what each table can be scanned with, and what is left over.
    fn push_down(&self, s: &Select) -> Result<(Filter, Vec<Filter>), SqlError> {
        let mut prefilters: Vec<Filter> = vec![Vec::new(); self.entries.len()];
        let mut residual = Vec::new();
        for group in &s.filter {
            let mut owners = BTreeSet::new();
            for cond in group {
                owners.insert(self.owner(self.resolve(&cond.column)?));
            }
            match owners.iter().copied().collect::<Vec<_>>().as_slice() {
                [one] => prefilters[*one].push(group.clone()),
                _ => residual.push(group.clone()),
            }
        }
        Ok((residual, prefilters))
    }

    /// Which `from` entry a wide column belongs to.
    fn owner(&self, column: usize) -> usize {
        self.entries
            .iter()
            .rposition(|e| e.base <= column)
            .unwrap_or(0)
    }

    /// The expression whose value is the rows the `from` list produces.
    ///
    /// One stage per join, left-deep — `concat_lists(map_list(prev, λr. map_list(filter_list(t, λy.
    /// y.k == r.k), λy. row)))`. Each stage is a `map_list` of its own in the plan and therefore
    /// gets its own [`crate::plan::Op::Join`] over its own index; nesting them inside one
    /// per-element function would index the first join and leave the rest as nested loops, which is
    /// the cost this exists to remove.
    fn rows_expression(&mut self) -> Core {
        let mut rows = var(0);
        for i in 1..self.entries.len() {
            let left = self.fresh();
            let right = self.fresh();
            let base = self.entries[i].base;
            let width = self.entries[i].table.columns.len();
            let pairs = self.entries[i].on.clone();
            let key_of = |v: VarId, mine: bool| -> Core {
                let at = |p: &(usize, usize)| if mine { p.0 } else { p.1 };
                match pairs.as_slice() {
                    [one] => field(var(v), &field_name(at(one))),
                    many => make(
                        many.iter()
                            .enumerate()
                            .map(|(k, p)| {
                                (
                                    Arc::from(format!("k{k}")),
                                    field(var(v), &field_name(at(p))),
                                )
                            })
                            .collect(),
                    ),
                }
            };
            let predicate = prim(Prim::Eq, vec![key_of(right, true), key_of(left, false)]);
            let combined = make(
                (0..base)
                    .map(|k| (Arc::from(field_name(k)), field(var(left), &field_name(k))))
                    .chain(
                        (base..base + width)
                            .map(|k| (Arc::from(field_name(k)), field(var(right), &field_name(k)))),
                    )
                    .collect(),
            );
            let inner = prim(
                Prim::MapList,
                vec![
                    prim(
                        Prim::FilterList,
                        vec![var(i as VarId), lam(vec![right], predicate)],
                    ),
                    lam(vec![right], combined),
                ],
            );
            rows = prim(
                Prim::ConcatLists,
                vec![prim(Prim::MapList, vec![rows, lam(vec![left], inner)])],
            );
        }
        rows
    }

    /// The select list as the fields of one record per row, and what its columns are called.
    fn project(&mut self, s: &Select, param: VarId) -> Result<(Row, Vec<Field>), SqlError> {
        let mut out = Vec::new();
        let mut fields = Vec::new();
        for item in &s.items {
            match item {
                Item::All(qualifier) => {
                    let mut any = false;
                    for i in 0..self.fields.len() {
                        if let Some(t) = qualifier {
                            if self.fields[i].of.as_deref() != Some(t.as_str()) {
                                continue;
                            }
                        }
                        any = true;
                        let f = self.fields[i].clone();
                        push(&mut out, &mut fields, f, field(var(param), &field_name(i)));
                    }
                    if !any {
                        return Err(SqlError::no_table(format!(
                            "\"{}\" is not a table in this query",
                            qualifier.as_deref().unwrap_or("")
                        )));
                    }
                }
                Item::Column(name, alias) => {
                    let i = self.resolve(name)?;
                    let mut f = self.fields[i].clone();
                    if let Some(a) = alias {
                        f.column.name = Arc::from(a.as_str());
                        f.of = None;
                    }
                    push(&mut out, &mut fields, f, field(var(param), &field_name(i)));
                }
                Item::Literal(d, alias) => {
                    let f = literal_field(d, alias.as_deref());
                    push(&mut out, &mut fields, f, constant(d));
                }
                Item::Count(_) | Item::Aggregate(..) => {
                    return Err(SqlError::unsupported(
                        "`select distinct` over an aggregate has nothing to be distinct about; a \
                         `group by` is what asks an aggregate per group",
                    ))
                }
            }
        }
        Ok((out, fields))
    }

    /// A `group by`, or an aggregate with no `group by` — which is the same question asked of the
    /// whole collection rather than of a group.
    ///
    /// The grouped form is the loop `corpus/35-workload.beck` writes by hand: the distinct keys are
    /// the rows, and each aggregate is a question about the filter that would have built the group.
    /// [`crate::relate`] reads exactly that shape, so what the plan ends up with is a
    /// [`crate::plan::Op::ArrangeBy`] for the counts, a [`crate::plan::Op::GroupBy`] per other
    /// aggregate, and a join per question — and no group is ever built.
    fn grouped(&mut self, s: &Select, rows: VarId) -> Result<(Core, Vec<Field>), SqlError> {
        let keys: Vec<usize> = s
            .group
            .iter()
            .map(|n| self.resolve(n))
            .collect::<Result<_, _>>()?;
        let ungrouped = keys.is_empty();
        let element = self.fresh();

        let mut out = Vec::new();
        let mut fields = Vec::new();
        for item in &s.items {
            match item {
                Item::All(_) => {
                    return Err(SqlError::unsupported(
                        "`select *` with a `group by` would name every column of every row, and a \
                         group is not a row: name the grouped columns and the aggregates",
                    ))
                }
                Item::Column(name, alias) => {
                    let i = self.resolve(name)?;
                    let at = keys.iter().position(|&k| k == i).ok_or_else(|| SqlError {
                        message: format!(
                            "\"{name}\" is not in the `group by`, so a group has more than one of \
                             it: put it in the `group by`, or ask an aggregate for it"
                        ),
                        code: "42803",
                    })?;
                    let mut f = self.fields[i].clone();
                    if let Some(a) = alias {
                        f.column.name = Arc::from(a.as_str());
                        f.of = None;
                    }
                    let value = match keys.len() {
                        1 => var(element),
                        _ => field(var(element), &format!("k{at}")),
                    };
                    push(&mut out, &mut fields, f, value);
                }
                Item::Literal(d, alias) => {
                    let f = literal_field(d, alias.as_deref());
                    push(&mut out, &mut fields, f, constant(d));
                }
                Item::Count(alias) => {
                    let f = Field {
                        column: Column {
                            name: Arc::from(alias.as_deref().unwrap_or("count")),
                            ty: SqlTy::Bigint,
                            nullable: false,
                        },
                        of: None,
                    };
                    let over = match ungrouped {
                        true => var(rows),
                        false => self.group_filter(rows, element, &keys),
                    };
                    push(&mut out, &mut fields, f, prim(Prim::ListLen, vec![over]));
                }
                Item::Aggregate(agg, name, alias) => {
                    let i = self.resolve(name)?;
                    let source = self.fields[i].clone();
                    if source.column.nullable {
                        return Err(SqlError::unsupported(format!(
                            "`{}({name})` is not answered here because \"{name}\" is an `Option` \
                             and SQL's aggregates skip nulls: this one is a function of what every \
                             row contributes, so a row contributing nothing has no answer",
                            agg.name()
                        )));
                    }
                    if *agg == Agg::Sum && source.column.ty != SqlTy::Bigint {
                        return Err(SqlError::unsupported(format!(
                            "`sum({name})` is not answered here because \"{name}\" is {} and a \
                             total over it would depend on the order it was added in — Beck's \
                             `list_sum` is exact over `Int` and has no `Float` form at all \
                             (docs/46 §46.16)",
                            source.column.ty.name()
                        )));
                    }
                    let f = Field {
                        column: Column {
                            name: Arc::from(
                                alias.clone().unwrap_or_else(|| agg.name().to_string()),
                            ),
                            ty: source.column.ty,
                            nullable: *agg != Agg::Sum,
                        },
                        of: None,
                    };
                    let over = match ungrouped {
                        true => var(rows),
                        false => self.group_filter(rows, element, &keys),
                    };
                    let row = self.fresh();
                    let projected = prim(
                        Prim::MapList,
                        vec![over, lam(vec![row], field(var(row), &field_name(i)))],
                    );
                    let op = match agg {
                        Agg::Min => Prim::ListMin,
                        Agg::Max => Prim::ListMax,
                        Agg::Sum => Prim::ListSum,
                    };
                    push(&mut out, &mut fields, f, prim(op, vec![projected]));
                }
            }
        }

        if ungrouped {
            return Ok((make(out), fields));
        }
        let key_param = self.fresh();
        let distinct = prim(
            Prim::ListUnique,
            vec![prim(
                Prim::MapList,
                vec![var(rows), lam(vec![key_param], group_key(&keys, key_param))],
            )],
        );
        Ok((
            prim(Prim::MapList, vec![distinct, lam(vec![element], make(out))]),
            fields,
        ))
    }

    /// `filter_list(R, λr. g(r) == k)` — the group, as the expression an aggregate asks about.
    ///
    /// Never evaluated as written: [`crate::relate`] reads it as the probe of an index keyed by
    /// `g`, and what the operator above it asks for decides whether a group is built at all. A
    /// count is answered from the join's tally and an extreme from
    /// [`crate::plan::Op::GroupBy`]'s multiset, so this expression is the *question* rather than
    /// the work.
    fn group_filter(&mut self, rows: VarId, element: VarId, keys: &[usize]) -> Core {
        let row = self.fresh();
        let predicate = prim(Prim::Eq, vec![group_key(keys, row), var(element)]);
        prim(Prim::FilterList, vec![var(rows), lam(vec![row], predicate)])
    }
}

/// The value a group's key has, as a function of one row: the column itself when a query groups by
/// one, and a record of them when it groups by several — which is a key a `BTreeMap` orders exactly
/// as `==` compares it, so the index and the equality agree by construction.
fn group_key(keys: &[usize], v: VarId) -> Core {
    match keys {
        [one] => field(var(v), &field_name(*one)),
        many => make(
            many.iter()
                .enumerate()
                .map(|(j, &i)| (Arc::from(format!("k{j}")), field(var(v), &field_name(i))))
                .collect(),
        ),
    }
}

fn push(out: &mut Row, fields: &mut Vec<Field>, f: Field, value: Core) {
    out.push((Arc::from(field_name(fields.len())), value));
    fields.push(f);
}

fn literal_field(d: &Datum, alias: Option<&str>) -> Field {
    Field {
        column: Column {
            name: Arc::from(alias.unwrap_or("?column?")),
            ty: d.ty(),
            nullable: false,
        },
        of: None,
    }
}

// -------------------------------------------------------------------------------------------
// Small `Core` constructors
// -------------------------------------------------------------------------------------------
//
// Types are `unit` throughout and that is not laziness: a plan runs after the checker, nothing
// downstream reads a type off these nodes, and giving them invented ones would be a second, wrong
// answer to a question this expression was never asked.

fn node(kind: CoreKind) -> Core {
    Core {
        kind,
        ty: Ty::unit(),
        tier: Tier::Any,
        span: Span::NONE,
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

fn var(v: VarId) -> Core {
    node(CoreKind::Var(v))
}

fn field(base: Core, name: &str) -> Core {
    node(CoreKind::Field {
        base: Box::new(base),
        name: Arc::from(name),
    })
}

fn prim(op: Prim, args: Vec<Core>) -> Core {
    node(CoreKind::Prim { op, args })
}

fn make(fields: Row) -> Core {
    node(CoreKind::Make {
        ty: Arc::from("Row"),
        variant: None,
        fields,
    })
}

fn lam(params: Vec<VarId>, body: Core) -> Core {
    node(CoreKind::Lam {
        params: params.into(),
        body: Arc::new(body),
    })
}

fn bind(v: VarId, value: Core, body: Core) -> Core {
    node(CoreKind::Let {
        var: v,
        value: Box::new(value),
        body: Box::new(body),
    })
}

fn constant(d: &Datum) -> Core {
    node(CoreKind::Const(match d {
        Datum::Boolean(b) => Const::Bool(*b),
        Datum::Bigint(i) => Const::Int(*i),
        Datum::Double(f) => Const::Float(*f),
        Datum::Text(s) => Const::Str(Arc::from(s.as_str())),
    }))
}
