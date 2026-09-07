//! `pg_catalog`, as read models over the schema the program already derives.
//!
//! [`docs/23-incremental-views-report.md`](../../../../../docs/23-incremental-views-report.md)
//! §23.19 held this open as "the correct long-run answer is a small read-only emulation of
//! `pg_class` and friends", and [`docs/08`](../../../../../docs/08-roadmap.md)'s Phase 3 exit row
//! carried it as the one thing a DBA still could not do: `psql`'s `\d` is a join over four
//! catalogue relations, and those relations did not exist.
//!
//! # What this is
//!
//! Every relation here is a [`crate::read::Table`] like any other, with a
//! [`crate::read::Source::Pg`] saying where its rows come from — which is *this module*, from the
//! [`Schema`] the compiler derived. So `\d` is answered by the same parser, the same
//! [`crate::plan::Op::Join`] and the same scan that answer `select * from todos`, and there is no
//! branch anywhere in [`beck_rt::pgwire`](../../beck_rt/pgwire/index.html) that knows what a
//! backslash command is. A catalogue that were a special case in the wire protocol would be a
//! second query path to keep true; this one cannot disagree with the schema because it *is* the
//! schema, read through a different set of column names.
//!
//! # What it is not
//!
//! It is not PostgreSQL's catalogue. There are no indexes, no constraints, no triggers, no
//! sequences, no functions and no roles, because a read model has none of those things — so the
//! relations that would describe them are here with their columns and **no rows**, which is the
//! true answer rather than an absent one: `\d todos` asks about policies and publications on its
//! way to printing a table, and "none" is what it needs to hear.
//!
//! A relation that is not here at all is refused **by name** ([`Rel::missing`]), because a client
//! that is told "there is no `pg_catalog.pg_proc`" knows what happened and a client handed an empty
//! answer does not.
//!
//! # The four decisions in the rows
//!
//! * **Every read model is `relkind = 'r'`** — an ordinary table.
//!   [`docs/05`](../../../../../docs/05-tier-lowering.md) §5.3's promise is that a tool "sees
//!   materialized views as ordinary tables", and a maintained arrangement answered as `'m'` would
//!   send `psql` looking for a view definition this has no SQL to give it. What a table is derived
//!   *from* is in [`Schema::CATALOGUE`], which is the table that answers that question.
//! * **One namespace for the program, one for the catalogue.** The read models are in `public` and
//!   these relations are in `pg_catalog`, which is what makes `\d` list the program's tables and
//!   not its own: `psql` filters on `nspname <> 'pg_catalog'`, and that filter has to have
//!   something to filter.
//! * **The oids are positions, not identities.** A relation's oid is its index in a list that is
//!   built from the schema and cannot change while the process runs. Nothing persists one.
//! * **Owner and access method are constants**, because there is nobody to own a read model and no
//!   index to choose a method for.
//!
//! Every column `psql` names is here; a column it does not name is not, so this file is short for
//! the same reason the SQL is a subset.

use std::sync::Arc;

use crate::core::{Fields, Value};
use crate::read::{Cardinality, Cell, Column, Datum, Schema, SqlTy, Table};

/// The namespace a program's read models are in.
pub const PUBLIC: &str = "public";
/// The namespace these relations are in, and the one `psql` hides from `\d`.
pub const CATALOG: &str = "pg_catalog";

/// The oid of the `pg_catalog` namespace, and of `public` — PostgreSQL's own, because a client
/// that hard-codes one hard-codes these.
const NSP_CATALOG: i64 = 11;
const NSP_PUBLIC: i64 = 2200;

/// Where relation oids start. Above every oid PostgreSQL reserves for its own catalogue, which is
/// where a real server's first user table lands too.
const FIRST_OID: i64 = 16384;

/// The one role every read model is owned by. There is no authentication on this port
/// ([`adr/0020`](../../../../../docs/adr/0020-the-read-model-speaks-pgwire-by-hand.md)), so there
/// is no user to be the owner and this is the name `pg_get_userbyid` answers with.
pub const OWNER: &str = "beck";

/// One relation of the emulated catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rel {
    Class,
    Namespace,
    Attribute,
    Type,
    Database,
    /// The relations below have columns and no rows: a read model has no such object, and a query
    /// that joins one is answered with the no rows that is the truth rather than refused.
    Am,
    Attrdef,
    Collation,
    Inherits,
    Policy,
    Publication,
    PublicationNamespace,
    PublicationRel,
    StatisticExt,
}

/// Every relation, in the order [`relations`] builds them and therefore in oid order.
pub const ALL: &[Rel] = &[
    Rel::Class,
    Rel::Namespace,
    Rel::Attribute,
    Rel::Type,
    Rel::Database,
    Rel::Am,
    Rel::Attrdef,
    Rel::Collation,
    Rel::Inherits,
    Rel::Policy,
    Rel::Publication,
    Rel::PublicationNamespace,
    Rel::PublicationRel,
    Rel::StatisticExt,
];

use SqlTy::{Bigint, Boolean, Text};

/// A column of a catalogue relation: its name, its type, and whether this catalogue ever writes a
/// NULL into it.
///
/// Nullability is a fact worth stating rather than a default, because a **join key must not be
/// nullable**: this SQL's join is the index's own equality, where a null would match a null
/// ([`crate::query`] refuses one by name). Every column `psql` joins on is an object identifier
/// this module always writes, and every column that stands for something a read model does not
/// have — an ACL, a stored expression, a locale — is a NULL and says so.
type Col = (&'static str, SqlTy, bool);

const SET: bool = false;
const NULLABLE: bool = true;

impl Rel {
    pub fn name(self) -> &'static str {
        match self {
            Rel::Class => "pg_class",
            Rel::Namespace => "pg_namespace",
            Rel::Attribute => "pg_attribute",
            Rel::Type => "pg_type",
            Rel::Database => "pg_database",
            Rel::Am => "pg_am",
            Rel::Attrdef => "pg_attrdef",
            Rel::Collation => "pg_collation",
            Rel::Inherits => "pg_inherits",
            Rel::Policy => "pg_policy",
            Rel::Publication => "pg_publication",
            Rel::PublicationNamespace => "pg_publication_namespace",
            Rel::PublicationRel => "pg_publication_rel",
            Rel::StatisticExt => "pg_statistic_ext",
        }
    }

    /// The columns, in `pg_catalog`'s own order and spelling.
    ///
    /// An `oid`-shaped column is [`SqlTy::Bigint`] rather than a type of its own: PostgreSQL's
    /// `oid` is a 32-bit unsigned integer, every comparison `psql` writes against one is against a
    /// decimal literal, and a fifth SQL type would have to be a type OID a driver knows.
    /// A `"char"` column — `relkind`, `relpersistence`, `attidentity` — is [`SqlTy::Text`] of one
    /// character, which is what it compares as.
    pub fn columns(self) -> &'static [Col] {
        match self {
            Rel::Class => &[
                ("oid", Bigint, SET),
                ("relname", Text, SET),
                ("relnamespace", Bigint, SET),
                ("relkind", Text, SET),
                ("relowner", Bigint, SET),
                ("relam", Bigint, SET),
                ("reltablespace", Bigint, SET),
                ("reltoastrelid", Bigint, SET),
                ("reloftype", Bigint, SET),
                ("relnatts", Bigint, SET),
                ("reltuples", Bigint, SET),
                ("relchecks", Bigint, SET),
                ("relhasindex", Boolean, SET),
                ("relhasrules", Boolean, SET),
                ("relhastriggers", Boolean, SET),
                ("relhassubclass", Boolean, SET),
                ("relrowsecurity", Boolean, SET),
                ("relforcerowsecurity", Boolean, SET),
                ("relispartition", Boolean, SET),
                ("relpersistence", Text, SET),
                ("relreplident", Text, SET),
                ("reloptions", Text, NULLABLE),
                ("relpartbound", Text, NULLABLE),
                ("relacl", Text, NULLABLE),
            ],
            Rel::Namespace => &[
                ("oid", Bigint, SET),
                ("nspname", Text, SET),
                ("nspowner", Bigint, SET),
                ("nspacl", Text, NULLABLE),
            ],
            Rel::Attribute => &[
                ("attrelid", Bigint, SET),
                ("attname", Text, SET),
                ("atttypid", Bigint, SET),
                ("atttypmod", Bigint, SET),
                ("attnum", Bigint, SET),
                ("attnotnull", Boolean, SET),
                ("atthasdef", Boolean, SET),
                ("attisdropped", Boolean, SET),
                ("attislocal", Boolean, SET),
                ("attcollation", Bigint, SET),
                ("attstattarget", Bigint, SET),
                ("attidentity", Text, SET),
                ("attgenerated", Text, SET),
                ("attcompression", Text, SET),
            ],
            Rel::Type => &[
                ("oid", Bigint, SET),
                ("typname", Text, SET),
                ("typnamespace", Bigint, SET),
                ("typtype", Text, SET),
                ("typelem", Bigint, SET),
                ("typcollation", Bigint, SET),
            ],
            Rel::Database => &[
                ("oid", Bigint, SET),
                ("datname", Text, SET),
                ("datdba", Bigint, SET),
                ("encoding", Bigint, SET),
                ("datlocprovider", Text, SET),
                ("datcollate", Text, SET),
                ("datctype", Text, SET),
                ("daticulocale", Text, NULLABLE),
                ("datallowconn", Boolean, SET),
                ("datistemplate", Boolean, SET),
                ("datacl", Text, NULLABLE),
            ],
            Rel::Am => &[
                ("oid", Bigint, SET),
                ("amname", Text, SET),
                ("amhandler", Bigint, NULLABLE),
                ("amtype", Text, SET),
            ],
            Rel::Attrdef => &[
                ("oid", Bigint, SET),
                ("adrelid", Bigint, SET),
                ("adnum", Bigint, SET),
                ("adbin", Text, NULLABLE),
            ],
            Rel::Collation => &[
                ("oid", Bigint, SET),
                ("collname", Text, SET),
                ("collnamespace", Bigint, SET),
                ("collcollate", Text, NULLABLE),
                ("collctype", Text, NULLABLE),
            ],
            Rel::Inherits => &[
                ("inhrelid", Bigint, SET),
                ("inhparent", Bigint, SET),
                ("inhseqno", Bigint, SET),
                ("inhdetachpending", Boolean, SET),
            ],
            Rel::Policy => &[
                ("oid", Bigint, SET),
                ("polname", Text, SET),
                ("polrelid", Bigint, SET),
                ("polcmd", Text, SET),
                ("polpermissive", Boolean, SET),
                ("polroles", Text, NULLABLE),
                ("polqual", Text, NULLABLE),
                ("polwithcheck", Text, NULLABLE),
            ],
            Rel::Publication => &[
                ("oid", Bigint, SET),
                ("pubname", Text, SET),
                ("pubowner", Bigint, SET),
                ("puballtables", Boolean, SET),
            ],
            Rel::PublicationNamespace => &[
                ("oid", Bigint, SET),
                ("pnpubid", Bigint, SET),
                ("pnnspid", Bigint, SET),
            ],
            Rel::PublicationRel => &[
                ("oid", Bigint, SET),
                ("prpubid", Bigint, SET),
                ("prrelid", Bigint, SET),
                ("prqual", Text, NULLABLE),
                ("prattrs", Text, NULLABLE),
            ],
            Rel::StatisticExt => &[
                ("oid", Bigint, SET),
                ("stxrelid", Bigint, SET),
                ("stxnamespace", Bigint, SET),
                ("stxname", Text, SET),
                ("stxowner", Bigint, SET),
                ("stxkind", Text, NULLABLE),
                ("stxstattarget", Bigint, SET),
            ],
        }
    }

    /// What a client is told when it names a `pg_catalog` relation that is not here.
    ///
    /// By name, and with the list, because the alternative — an empty answer — is a client that
    /// believes the program has no functions rather than one that knows this catalogue has no
    /// `pg_proc`.
    pub fn missing(name: &str) -> String {
        format!(
            "\"pg_catalog.{name}\" is not one of the catalogue relations this read model has. \
             There is {}. `\\d`, `\\d <table>`, `\\dt`, `\\dn` and `\\l` are the backslash \
             commands they answer; anything else asks for an object a read model does not have",
            ALL.iter()
                .map(|r| format!("\"{}\"", r.name()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// The catalogue's relations as tables, in oid order.
pub fn relations() -> Vec<Table> {
    ALL.iter()
        .map(|rel| Table {
            name: Arc::from(rel.name()),
            columns: rel
                .columns()
                .iter()
                .map(|(n, ty, nullable)| Column {
                    name: Arc::from(*n),
                    ty: *ty,
                    nullable: *nullable,
                })
                .collect(),
            source: crate::read::Source::Pg(*rel),
            cardinality: Cardinality::Many,
            element: Arc::from(rel.name()),
        })
        .collect()
}

/// The oid of a relation: its position in the schema's own list, then in the catalogue's.
///
/// A function rather than a field because it is derived from the same order twice — once to build
/// `pg_class` and once for `pg_attribute` to point at it — and two lists that had to agree by
/// inspection is the thing this project keeps refusing to write.
fn oids(schema: &Schema) -> Vec<(i64, &Table, i64)> {
    let mut out = Vec::new();
    let mut next = FIRST_OID;
    for t in &schema.tables {
        out.push((next, t, NSP_PUBLIC));
        next += 1;
    }
    for t in &schema.pg {
        out.push((next, t, NSP_CATALOG));
        next += 1;
    }
    out
}

/// The type oid of a column, which is the OID that goes on the wire for it too
/// ([`SqlTy::oid`]) — one mapping, so `format_type` cannot disagree with `RowDescription`.
fn type_oid(ty: SqlTy) -> i64 {
    ty.oid() as i64
}

/// One relation's rows, derived from the schema.
///
/// `O(tables + columns)` for every relation, and nothing is cached: the whole catalogue of a
/// 40-table program is a few thousand small values, built once per query that reads it and thrown
/// away with the answer. The alternative — a cache — would be a second copy of the schema to keep
/// true, which is the thing this module exists to not be.
pub fn rows(rel: Rel, schema: &Schema) -> Vec<Value> {
    let relations = oids(schema);
    match rel {
        Rel::Class => relations
            .iter()
            .map(|(oid, t, nsp)| {
                record(
                    rel,
                    [
                        ("oid", Value::Int(*oid)),
                        ("relname", Value::text(t.name.to_string())),
                        ("relnamespace", Value::Int(*nsp)),
                        // See the module docs: a read model is an ordinary table, whatever
                        // maintains it.
                        ("relkind", Value::text("r".into())),
                        ("relowner", Value::Int(10)),
                        ("relam", Value::Int(0)),
                        ("reltablespace", Value::Int(0)),
                        ("reltoastrelid", Value::Int(0)),
                        ("reloftype", Value::Int(0)),
                        ("relnatts", Value::Int(t.columns.len() as i64)),
                        // -1 is "never analysed", which is the truth: nothing counts these rows
                        // until somebody asks.
                        ("reltuples", Value::Int(-1)),
                        ("relchecks", Value::Int(0)),
                        ("relhasindex", Value::Bool(false)),
                        ("relhasrules", Value::Bool(false)),
                        ("relhastriggers", Value::Bool(false)),
                        ("relhassubclass", Value::Bool(false)),
                        ("relrowsecurity", Value::Bool(false)),
                        ("relforcerowsecurity", Value::Bool(false)),
                        ("relispartition", Value::Bool(false)),
                        ("relpersistence", Value::text("p".into())),
                        ("relreplident", Value::text("d".into())),
                        ("reloptions", Value::Unit),
                        ("relpartbound", Value::Unit),
                        ("relacl", Value::Unit),
                    ],
                )
            })
            .collect(),
        Rel::Namespace => [(NSP_CATALOG, CATALOG), (NSP_PUBLIC, PUBLIC)]
            .iter()
            .map(|(oid, name)| {
                record(
                    rel,
                    [
                        ("oid", Value::Int(*oid)),
                        ("nspname", Value::text((*name).into())),
                        ("nspowner", Value::Int(10)),
                        ("nspacl", Value::Unit),
                    ],
                )
            })
            .collect(),
        Rel::Attribute => relations
            .iter()
            .flat_map(|(oid, t, _)| {
                t.columns.iter().enumerate().map(move |(i, c)| {
                    record(
                        rel,
                        [
                            ("attrelid", Value::Int(*oid)),
                            ("attname", Value::text(c.name.to_string())),
                            ("atttypid", Value::Int(type_oid(c.ty))),
                            // No type modifier: none of the four types has one.
                            ("atttypmod", Value::Int(-1)),
                            ("attnum", Value::Int(i as i64 + 1)),
                            ("attnotnull", Value::Bool(!c.nullable)),
                            ("atthasdef", Value::Bool(false)),
                            ("attisdropped", Value::Bool(false)),
                            ("attislocal", Value::Bool(true)),
                            ("attcollation", Value::Int(0)),
                            ("attstattarget", Value::Int(-1)),
                            ("attidentity", Value::text(String::new())),
                            ("attgenerated", Value::text(String::new())),
                            ("attcompression", Value::text(String::new())),
                        ],
                    )
                })
            })
            .collect(),
        // The four types a Beck scalar maps onto, under the names PostgreSQL's own catalogue
        // spells them with — `format_type` is what turns one into the name a person reads.
        Rel::Type => [
            (SqlTy::Boolean, "bool"),
            (SqlTy::Bigint, "int8"),
            (SqlTy::Text, "text"),
            (SqlTy::Double, "float8"),
        ]
        .iter()
        .map(|(ty, name)| {
            record(
                rel,
                [
                    ("oid", Value::Int(type_oid(*ty))),
                    ("typname", Value::text((*name).into())),
                    ("typnamespace", Value::Int(NSP_CATALOG)),
                    ("typtype", Value::text("b".into())),
                    ("typelem", Value::Int(0)),
                    ("typcollation", Value::Int(0)),
                ],
            )
        })
        .collect(),
        // One database, because a process serves one program's state.
        Rel::Database => vec![record(
            rel,
            [
                ("oid", Value::Int(FIRST_OID - 1)),
                ("datname", Value::text(OWNER.into())),
                ("datdba", Value::Int(10)),
                // 6 is UTF8 in PostgreSQL's encoding table, which is the encoding this wire
                // announces in its startup parameters.
                ("encoding", Value::Int(6)),
                ("datlocprovider", Value::text("c".into())),
                ("datcollate", Value::text("C".into())),
                ("datctype", Value::text("C".into())),
                ("daticulocale", Value::Unit),
                ("datallowconn", Value::Bool(true)),
                ("datistemplate", Value::Bool(false)),
                ("datacl", Value::Unit),
            ],
        )],
        // The relations that describe what a read model does not have.
        Rel::Am
        | Rel::Attrdef
        | Rel::Collation
        | Rel::Inherits
        | Rel::Policy
        | Rel::Publication
        | Rel::PublicationNamespace
        | Rel::PublicationRel
        | Rel::StatisticExt => Vec::new(),
    }
}

fn record<const N: usize>(rel: Rel, fields: [(&str, Value); N]) -> Value {
    debug_assert_eq!(
        N,
        rel.columns().len(),
        "{} builds a row of {N} fields for {} columns",
        rel.name(),
        rel.columns().len()
    );
    let fields: Fields = fields
        .into_iter()
        .map(|(k, v)| (Arc::from(k), v))
        .collect::<Vec<_>>()
        .into_iter()
        .collect();
    Value::data(rel.name(), None, fields)
}

// -------------------------------------------------------------------------------------------
// The functions a catalogue query calls
// -------------------------------------------------------------------------------------------

/// A `pg_catalog` function, applied to the arguments a row produced.
///
/// `None` means this catalogue has no function of that name, and the caller refuses it **by name**
/// — the same rule as [`Rel::missing`], and for the same reason: `psql` writes these calls into
/// queries it expects to work, so one that silently answered NULL would produce a `\d` that is
/// wrong rather than one that says what it could not do.
///
/// Every one of them is a constant or a lookup. There is nothing to compute because there is
/// nothing behind them: no roles, no search path, no defaults, no ACLs.
pub fn call(name: &str, args: &[Cell]) -> Option<Result<Cell, String>> {
    let text = |s: &str| Some(Ok(Some(Datum::Text(s.to_string()))));
    let null = || Some(Ok(None));
    let arg = |i: usize| args.get(i).cloned().flatten();
    match name {
        // There is one owner and one search path, so these are the same answer for every row.
        "pg_get_userbyid" => text(OWNER),
        "pg_table_is_visible" | "pg_type_is_visible" | "pg_function_is_visible" => {
            Some(Ok(Some(Datum::Boolean(true))))
        }
        // Nothing is published and nothing is replicated.
        "pg_relation_is_publishable" => Some(Ok(Some(Datum::Boolean(false)))),
        // A column's type as a person reads it, from the same OID that goes on the wire for it.
        "format_type" => Some(Ok(match arg(0) {
            Some(Datum::Bigint(oid)) => [SqlTy::Boolean, SqlTy::Bigint, SqlTy::Text, SqlTy::Double]
                .iter()
                .find(|t| type_oid(**t) == oid)
                .map(|t| Datum::Text(t.name().to_string())),
            _ => None,
        })),
        // The stored form of an expression: a default, a partition bound, a policy's predicate.
        // A read model has none of the three, and every row that could carry one is a row that
        // does not exist.
        "pg_get_expr" | "pg_get_constraintdef" | "pg_get_indexdef" | "pg_get_partkeydef" => null(),
        "pg_encoding_to_char" => match arg(0) {
            Some(Datum::Bigint(6)) => text("UTF8"),
            _ => null(),
        },
        // An array joined into a string. Every array-shaped column here is NULL, and NULL is what
        // PostgreSQL answers for one; an array with elements in it would be a fifth type.
        "array_to_string" => match arg(0) {
            None => null(),
            Some(_) => Some(Err(
                "`array_to_string` has nothing to join here: the catalogue's array-shaped columns \
                 are all null, because a read model has no ACL, no policy roles and no statistics \
                 kinds"
                    .to_string(),
            )),
        },
        "current_database" | "current_catalog" => text(OWNER),
        "current_schema" => text(PUBLIC),
        "current_user" | "session_user" | "user" => text(OWNER),
        "version" => text(&crate::read::version()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every relation builds rows whose fields are exactly the columns it declares.
    ///
    /// The negative half is what this is for: a column added to [`Rel::columns`] and forgotten in
    /// [`rows`] would be a column `psql` reads as NULL — a `\d` that prints a table with no
    /// nullability rather than one that fails.
    #[test]
    fn every_row_has_every_column_of_its_relation() {
        let schema = Schema {
            tables: Vec::new(),
            pg: relations(),
        };
        for rel in ALL {
            let names: Vec<&str> = rel.columns().iter().map(|(n, ..)| *n).collect();
            for row in rows(*rel, &schema) {
                let Value::Data(d) = &row else {
                    panic!("{} builds a row that is not a record", rel.name())
                };
                let mut got: Vec<&str> = d.fields.iter().map(|(k, _)| k.as_ref()).collect();
                let mut want = names.clone();
                got.sort_unstable();
                want.sort_unstable();
                assert_eq!(got, want, "{}", rel.name());
            }
        }
    }

    #[test]
    fn a_relation_that_is_not_here_is_refused_by_name() {
        let m = Rel::missing("pg_proc");
        assert!(m.contains("pg_catalog.pg_proc"), "{m}");
        assert!(m.contains("pg_class"), "{m}");
    }
}
