//! The standard library of the walking skeleton.
//!
//! Small on purpose. §3.2's promise is that "effect polymorphism is what keeps one standard
//! library" — `map : (list[a], (a -> b ! e)) -> list[b] ! e`. Phase 2 has effect rows, so that
//! signature is now written as written: `map_list` is polymorphic in what its function argument
//! does, and mapping an effectful function over a list is effectful *in exactly that way*. One
//! library, one definition per operation, usable from any tier the placement solver allows.
//!
//! The rows here are the source of truth for inference. [`Prim::effects`] is the same information
//! for the atoms a primitive performs *itself*, and a test holds the two in agreement.
//!
//! Everything here is a [`Prim`], which means the evaluator implements it and the eventual
//! Cranelift/LLVM backends implement it — never a Beck-source shim that would have to be compiled
//! twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::core::Prim;
use crate::ty::{Effect, MethodSig, Row, RowVarId, Scheme, TraitSig, Ty, TyDecl, Variant};

/// A fresh type variable id for a scheme. Scheme variables are numbered from a private range that
/// never collides with the inference variables `Subst` mints, because `instantiate` replaces them.
const A: u32 = 1_000_000;
const B: u32 = 1_000_001;
const C: u32 = 1_000_002;

/// Row-variable ids for the schemes below, in their own range for the same reason `A`/`B`/`C` are:
/// `instantiate` replaces them, so they can never collide with an inference variable.
const E: RowVarId = 2_000_000;

fn v(id: u32) -> Ty {
    Ty::Var(id)
}

fn poly(vars: &[u32], ty: Ty) -> Scheme {
    Scheme {
        vars: vars.to_vec(),
        row_vars: Vec::new(),
        params: Vec::new(),
        ty,
    }
}

/// A scheme polymorphic in both dimensions — §3.2's `(list[a], (a -> b ! e)) -> list[b] ! e`.
fn poly_eff(vars: &[u32], row_vars: &[RowVarId], ty: Ty) -> Scheme {
    Scheme {
        vars: vars.to_vec(),
        row_vars: row_vars.to_vec(),
        params: Vec::new(),
        ty,
    }
}

/// A pure function type.
fn fun(params: Vec<Ty>, ret: Ty) -> Ty {
    Ty::fun(params, ret)
}

/// A function type with an effect row.
fn fun_eff(params: Vec<Ty>, ret: Ty, row: Row) -> Ty {
    Ty::fun_eff(params, ret, row)
}

/// Every primitive's name and type.
pub fn prims() -> Vec<(&'static str, Prim, Scheme)> {
    let int = Ty::int();
    let bool_ = Ty::bool_();
    let str_ = Ty::str_();
    let float = Ty::con(Ty::FLOAT);
    let html = Ty::html();
    let attr = Ty::con(Ty::ATTR);

    vec![
        // `+` is resolved bidirectionally in `check` so that it can also concatenate strings
        // without introducing a numeric type class; the scheme here is its Int form.
        (
            "+",
            Prim::Add,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "-",
            Prim::Sub,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "*",
            Prim::Mul,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "/",
            Prim::Div,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "%",
            Prim::Rem,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        // The reals. `abs` is written for both tiers in SICP and is resolved from its operand in
        // `check`, exactly as `+` is; the scheme here is its `Int` form, which is what a reference
        // to it *as a value* gets (`docs/32` §32.3).
        (
            "abs",
            Prim::Abs,
            Scheme::mono(fun(vec![int.clone()], int.clone())),
        ),
        (
            "sqrt",
            Prim::Sqrt,
            Scheme::mono(fun(vec![float.clone()], float.clone())),
        ),
        (
            "float",
            Prim::ToFloat,
            Scheme::mono(fun(vec![int.clone()], float.clone())),
        ),
        (
            "negate",
            Prim::Neg,
            Scheme::mono(fun(vec![int.clone()], int.clone())),
        ),
        (
            "==",
            Prim::Eq,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "!=",
            Prim::Ne,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "<",
            Prim::Lt,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "<=",
            Prim::Le,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            ">",
            Prim::Gt,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            ">=",
            Prim::Ge,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "and",
            Prim::And,
            Scheme::mono(fun(vec![bool_.clone(), bool_.clone()], bool_.clone())),
        ),
        (
            "or",
            Prim::Or,
            Scheme::mono(fun(vec![bool_.clone(), bool_.clone()], bool_.clone())),
        ),
        (
            "not",
            Prim::Not,
            Scheme::mono(fun(vec![bool_.clone()], bool_.clone())),
        ),
        (
            "str",
            Prim::ToStr,
            poly(&[A], fun(vec![v(A)], str_.clone())),
        ),
        (
            "str_trim",
            Prim::StrTrim,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        // The canonical fallible operation, and the reason it is here rather than in the standard
        // library Wave 2 will write: `corpus/29-fallible.beck` needs one thing that can genuinely
        // fail on its input, and a parse is that thing in every language.
        (
            "str_to_int",
            Prim::StrToInt,
            Scheme::mono(fun(vec![str_.clone()], Ty::option(Ty::int()))),
        ),
        // ------------------------------------------------------------------------ strings
        //
        // Wave 2's string half ([`docs/08`](../../../../../docs/08-roadmap.md) §8.5.4). Every one of
        // these is a primitive rather than a definition written in Beck, and the reason is the
        // same in each case: a string is where the host has to be asked. `str_upper` is a Unicode
        // table, `str_split` is an allocation strategy, and writing either of them over a
        // `list[Str]` of characters in Beck would be a slower, less correct copy of what the host
        // already has. Where there *is* something to express — a `Decimal`, a `Json` document —
        // Wave 2 writes it in Beck instead, which is the distinction §1.1 claims to be able to
        // make.
        //
        // Positions are counted in **characters** — Unicode scalar values — and `str_len`,
        // `str_slice` and `str_index_of` are one unit or they are a trap;
        // `stdlib.rs::string_positions_are_characters_everywhere_or_nowhere` is where that is held.
        //
        // `str_slice(s, start, count)` takes a **count**, not an end index. Worth stating because
        // the signature cannot: a primitive's parameters have no names in the generated reference,
        // so `(Str, Int, Int) -> Str` reads either way and the first caller to pass a non-zero
        // start with a real count got it wrong (`docs/55` §55.5).
        //
        // Both are clamped rather than refused: a slice past the end is the empty string, not a
        // failure. That is a decision and not an oversight — a slice is not a parse, and `raises`
        // is for a program's own vocabulary rather than for the standard library's arithmetic
        // ([`45`](../../../../../docs/45-error-rows-report.md)).
        (
            "str_len",
            Prim::StrLen,
            Scheme::mono(fun(vec![str_.clone()], int.clone())),
        ),
        (
            "str_slice",
            Prim::StrSlice,
            Scheme::mono(fun(
                vec![str_.clone(), int.clone(), int.clone()],
                str_.clone(),
            )),
        ),
        (
            "str_split",
            Prim::StrSplit,
            Scheme::mono(fun(
                vec![str_.clone(), str_.clone()],
                Ty::list(str_.clone()),
            )),
        ),
        (
            "str_join",
            Prim::StrJoin,
            Scheme::mono(fun(
                vec![Ty::list(str_.clone()), str_.clone()],
                str_.clone(),
            )),
        ),
        (
            "str_contains",
            Prim::StrContains,
            Scheme::mono(fun(vec![str_.clone(), str_.clone()], bool_.clone())),
        ),
        (
            "str_starts_with",
            Prim::StrStartsWith,
            Scheme::mono(fun(vec![str_.clone(), str_.clone()], bool_.clone())),
        ),
        (
            "str_ends_with",
            Prim::StrEndsWith,
            Scheme::mono(fun(vec![str_.clone(), str_.clone()], bool_.clone())),
        ),
        (
            "str_upper",
            Prim::StrUpper,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        (
            "str_lower",
            Prim::StrLower,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        (
            "str_replace",
            Prim::StrReplace,
            Scheme::mono(fun(
                vec![str_.clone(), str_.clone(), str_.clone()],
                str_.clone(),
            )),
        ),
        (
            "str_index_of",
            Prim::StrIndexOf,
            Scheme::mono(fun(
                vec![str_.clone(), str_.clone()],
                Ty::option(int.clone()),
            )),
        ),
        (
            "str_repeat",
            Prim::StrRepeat,
            Scheme::mono(fun(vec![str_.clone(), int.clone()], str_.clone())),
        ),
        (
            "str_chars",
            Prim::StrChars,
            Scheme::mono(fun(vec![str_.clone()], Ty::list(str_.clone()))),
        ),
        // ------------------------------------------------------------------------ collections
        //
        // The higher-order ones are row-polymorphic in the argument's effects — §3.2's
        // `map : (list[a], (a -> b ! e)) -> list[b] ! e`, which
        // [`33`](../../../../../docs/33-effect-polymorphism-and-list-patterns-report.md) made true of
        // a *user's* definitions too. A pure caller of `list_fold` stays pure however another
        // caller uses it.
        (
            "list_get",
            Prim::ListGet,
            poly(
                &[A],
                fun(vec![Ty::list(v(A)), int.clone()], Ty::option(v(A))),
            ),
        ),
        (
            "list_slice",
            Prim::ListSlice,
            poly(
                &[A],
                fun(
                    vec![Ty::list(v(A)), int.clone(), int.clone()],
                    Ty::list(v(A)),
                ),
            ),
        ),
        (
            "list_reverse",
            Prim::ListReverse,
            poly(&[A], fun(vec![Ty::list(v(A))], Ty::list(v(A)))),
        ),
        (
            "list_take",
            Prim::ListTake,
            poly(&[A], fun(vec![Ty::list(v(A)), int.clone()], Ty::list(v(A)))),
        ),
        (
            "list_drop",
            Prim::ListDrop,
            poly(&[A], fun(vec![Ty::list(v(A)), int.clone()], Ty::list(v(A)))),
        ),
        (
            "list_contains",
            Prim::ListContains,
            poly(&[A], fun(vec![Ty::list(v(A)), v(A)], bool_.clone())),
        ),
        (
            "list_index_of",
            Prim::ListIndexOf,
            poly(
                &[A],
                fun(vec![Ty::list(v(A)), v(A)], Ty::option(int.clone())),
            ),
        ),
        (
            "list_append",
            Prim::ListAppend,
            poly(&[A], fun(vec![Ty::list(v(A)), v(A)], Ty::list(v(A)))),
        ),
        // Zip *with* a function rather than zip into a pair: Beck has no tuple type, and inventing
        // one for this would be a language change hiding inside a library addition. The shorter
        // list decides the length, which is the convention every language that has this agrees on.
        (
            "list_zip_with",
            Prim::ListZip,
            poly_eff(
                &[A, B, C],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        Ty::list(v(B)),
                        fun_eff(vec![v(A), v(B)], v(C), Row::var(E)),
                    ],
                    Ty::list(v(C)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "list_fold",
            Prim::ListFold,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        v(B),
                        fun_eff(vec![v(B), v(A)], v(B), Row::var(E)),
                    ],
                    v(B),
                    Row::var(E),
                ),
            ),
        ),
        (
            "list_all",
            Prim::ListAll,
            poly_eff(
                &[A],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        fun_eff(vec![v(A)], bool_.clone(), Row::var(E)),
                    ],
                    bool_.clone(),
                    Row::var(E),
                ),
            ),
        ),
        (
            "list_any",
            Prim::ListAny,
            poly_eff(
                &[A],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        fun_eff(vec![v(A)], bool_.clone(), Row::var(E)),
                    ],
                    bool_.clone(),
                    Row::var(E),
                ),
            ),
        ),
        (
            "list_flat_map",
            Prim::ListFlatMap,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        fun_eff(vec![v(A)], Ty::list(v(B)), Row::var(E)),
                    ],
                    Ty::list(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "map_keys",
            Prim::MapKeys,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B))], Ty::list(v(A)))),
        ),
        (
            "map_merge",
            Prim::MapMerge,
            poly(
                &[A, B],
                fun(
                    vec![Ty::map(v(A), v(B)), Ty::map(v(A), v(B))],
                    Ty::map(v(A), v(B)),
                ),
            ),
        ),
        // ------------------------------------------------------------------------ JSON and time
        //
        // `json_parse` and `time_parse` **raise** rather than returning a `Result`, and that is the
        // whole reason [`08`](../../../../../docs/08-roadmap.md) §8.5.3's trap 2 said the standard
        // library had to wait for [`45`](../../../../../docs/45-error-rows-report.md). A caller who
        // wants a `Result` writes `try:`; a caller already inside something fallible writes
        // nothing. Had these been written first, every one of their signatures would have had to
        // change.
        (
            "json_parse",
            Prim::JsonParse,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                Ty::con("Json"),
                Row::of([Effect::Raises(Arc::from("JsonError"))]),
            )),
        ),
        (
            "json_render",
            Prim::JsonRender,
            Scheme::mono(fun(vec![Ty::con("Json")], str_.clone())),
        ),
        // RFC 3339 in UTC, and only that: a time zone is a database with a release schedule, and
        // one is not being embedded in a compiler on the way past. `now()` gives the milliseconds
        // these two are the calendar over.
        (
            "time_format",
            Prim::TimeFormat,
            Scheme::mono(fun(vec![int.clone()], str_.clone())),
        ),
        (
            "time_parse",
            Prim::TimeParse,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                int.clone(),
                Row::of([Effect::Raises(Arc::from("TimeError"))]),
            )),
        ),
        // ------------------------------------------------- digests, encodings and identifiers
        //
        // Wave 2's crypto item, host half. A hash function is a table and base64 is a grammar, so
        // both are here rather than in `lib/`; what a program *does* with a digest — a token, a
        // fingerprint, a check that reads two halves apart — is `lib/crypto.beck`.
        //
        // A digest is **pure**. That is the line between this group and `uuid()`/`now()`, which are
        // the two nondeterministic things a crypto library is usually asked for: the same input
        // digests to the same string on every replay, so nothing here has to be recorded on an
        // envelope, and §3.7's rule about folds does not reach it.
        (
            "digest",
            Prim::Digest,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        // The one function whose input is a `secret[Str]` and whose output is a `Str`.
        //
        // A message authentication code exists to be given to somebody who must not learn the key,
        // so the declassification is what the operation *is* rather than a hole in §3.5. It is
        // charged `cap.sign` for the reason `reveal` is charged `cap.internal`: no client tier
        // discharges a capability, so a view cannot mint a token, and a server that mints one has
        // said so in its row. `adr/0014` is the decision and `security.rs` is the gate that keeps
        // this the *only* one.
        (
            "digest_keyed",
            Prim::DigestKeyed,
            Scheme::mono(fun_eff(
                vec![Ty::secret(str_.clone()), str_.clone()],
                str_.clone(),
                Row::of([Effect::Cap(Arc::from("sign"))]),
            )),
        ),
        // Comparing a digest with `==` returns at the first differing byte, which tells whoever is
        // guessing how much of their guess was right. This does not.
        (
            "digest_eq",
            Prim::DigestEq,
            Scheme::mono(fun(vec![str_.clone(), str_.clone()], bool_.clone())),
        ),
        (
            "hex_encode",
            Prim::HexEncode,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        (
            "hex_decode",
            Prim::HexDecode,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                str_.clone(),
                Row::of([Effect::Raises(Arc::from("EncodingError"))]),
            )),
        ),
        // RFC 4648 §5 — the URL-safe alphabet, unpadded — because every place a Beck program puts
        // one of these is a place `+`, `/` and `=` have to be escaped.
        (
            "base64_encode",
            Prim::Base64Encode,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        (
            "base64_decode",
            Prim::Base64Decode,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                str_.clone(),
                Row::of([Effect::Raises(Arc::from("EncodingError"))]),
            )),
        ),
        // `uuid()` has minted one since Phase 1 and nothing has ever read one back. This
        // *normalises* rather than only validating: two spellings of one identifier must not be
        // two map keys, and a `Str` that has been through here is canonical.
        (
            "uuid_parse",
            Prim::UuidParse,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                str_.clone(),
                Row::of([Effect::Raises(Arc::from("UuidError"))]),
            )),
        ),
        (
            "uuid_version",
            Prim::UuidVersion,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                int.clone(),
                Row::of([Effect::Raises(Arc::from("UuidError"))]),
            )),
        ),
        // ------------------------------------------------------------------------ the outbound call
        //
        // The row here is half of the truth, and the half that is a constant. `net.out(host)` is
        // charged at the *call site* from the literal first argument (`check::prim_call`), because
        // an effect atom whose argument is a value is the one thing this language has no way to
        // write in a scheme — and because the egress policy §6.5 derives is exactly the set of
        // those atoms, so a host that were not written where the call is would not be derivable.
        (
            "http_fetch",
            Prim::HttpFetch,
            Scheme::mono(fun_eff(
                vec![str_.clone(), Ty::con("HttpRequest")],
                Ty::con("HttpResponse"),
                Row::of([Effect::Raises(Arc::from("HttpError"))]),
            )),
        ),
        (
            "str_is_empty",
            Prim::StrIsEmpty,
            Scheme::mono(fun(vec![str_.clone()], bool_.clone())),
        ),
        (
            "list_len",
            Prim::ListLen,
            poly(&[A], fun(vec![Ty::list(v(A))], int.clone())),
        ),
        (
            "list_is_empty",
            Prim::ListIsEmpty,
            poly(&[A], fun(vec![Ty::list(v(A))], bool_.clone())),
        ),
        // §3.2, verbatim: `map : (list[a], (a -> b ! e)) -> list[b] ! e`. Mapping a function that
        // touches the dom over a list touches the dom; mapping a pure one does not.
        (
            "map_list",
            Prim::MapList,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![Ty::list(v(A)), fun_eff(vec![v(A)], v(B), Row::var(E))],
                    Ty::list(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "filter_list",
            Prim::FilterList,
            poly_eff(
                &[A],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        fun_eff(vec![v(A)], bool_.clone(), Row::var(E)),
                    ],
                    Ty::list(v(A)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "concat_lists",
            Prim::ConcatLists,
            poly(&[A], fun(vec![Ty::list(Ty::list(v(A)))], Ty::list(v(A)))),
        ),
        (
            "sort_by",
            Prim::SortBy,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![Ty::list(v(A)), fun_eff(vec![v(A)], v(B), Row::var(E))],
                    Ty::list(v(A)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "map_get",
            Prim::MapGet,
            poly(
                &[A, B],
                fun(vec![Ty::map(v(A), v(B)), v(A)], Ty::option(v(B))),
            ),
        ),
        (
            "map_insert",
            Prim::MapInsert,
            poly(
                &[A, B],
                fun(vec![Ty::map(v(A), v(B)), v(A), v(B)], Ty::map(v(A), v(B))),
            ),
        ),
        (
            "map_remove",
            Prim::MapRemove,
            poly(
                &[A, B],
                fun(vec![Ty::map(v(A), v(B)), v(A)], Ty::map(v(A), v(B))),
            ),
        ),
        (
            "map_values",
            Prim::MapValues,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B))], Ty::list(v(B)))),
        ),
        (
            "map_contains",
            Prim::MapContains,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B)), v(A)], bool_.clone())),
        ),
        (
            "map_len",
            Prim::MapLen,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B))], int.clone())),
        ),
        (
            "is_some",
            Prim::OptionIsSome,
            poly(&[A], fun(vec![Ty::option(v(A))], bool_.clone())),
        ),
        (
            "unwrap_or",
            Prim::OptionUnwrapOr,
            poly(&[A], fun(vec![Ty::option(v(A)), v(A)], v(A))),
        ),
        (
            "html_el",
            Prim::HtmlEl,
            Scheme::mono(fun(
                vec![str_.clone(), Ty::list(attr.clone()), Ty::list(html.clone())],
                html.clone(),
            )),
        ),
        (
            "html_text",
            Prim::HtmlText,
            poly(&[A], fun(vec![v(A)], html.clone())),
        ),
        (
            "html_attr",
            Prim::HtmlAttr,
            poly(&[A], fun(vec![str_.clone(), v(A)], attr.clone())),
        ),
        (
            "html_on",
            Prim::HtmlOn,
            poly(&[A], fun(vec![str_.clone(), v(A)], attr.clone())),
        ),
        (
            "html_key",
            Prim::HtmlKey,
            poly(&[A], fun(vec![v(A)], attr.clone())),
        ),
        (
            "uuid",
            Prim::NewUuid,
            Scheme::mono(fun_eff(vec![], str_.clone(), Row::of([Effect::Nondet]))),
        ),
        // The other half of §3.7's forbidden pair. `now()` is legal anywhere a clock exists and
        // illegal inside a fold — which is a statement about its row, not about its name.
        (
            "now",
            Prim::Now,
            Scheme::mono(fun_eff(vec![], int.clone(), Row::of([Effect::Nondet]))),
        ),
        // §3.5's `type ApiKey = secret[str]`, given a source. Reading the process environment is
        // `env`, which no client discharges — so a secret cannot even be *obtained* on the tier it
        // must not reach, before Sendable is consulted at the boundary.
        (
            "secret_env",
            Prim::SecretEnv,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                Ty::secret(str_.clone()),
                Row::of([Effect::Env]),
            )),
        ),
        // §3.5's missing quadrant: storable, never Sendable.
        //
        // Wrapping is pure and free — recording a fact is not an effect. *Reading* one performs
        // `cap.internal`, which no tier but the server discharges and which
        // [`crate::secure`] discharges only inside the authority chokepoint. So a view cannot
        // unwrap one to render it: not because rendering is forbidden, but because the view is not
        // somewhere a capability is held.
        (
            "internal_of",
            Prim::InternalOf,
            poly(&[A], fun(vec![v(A)], Ty::internal(v(A)))),
        ),
        (
            "reveal",
            Prim::Reveal,
            poly(
                &[A],
                fun_eff(
                    vec![Ty::internal(v(A))],
                    v(A),
                    Row::of([Effect::Cap(Arc::from("internal"))]),
                ),
            ),
        ),
        // ---- the signal vocabulary (§3.7) ----
        //
        // `merge_clients : () -> Stream[(Session × Command)] ! { ingress }`. Phase 1 has no tuple
        // type, so the pair is the `Proposal` model the prelude declares below — the same shape,
        // named.
        (
            "merge_clients",
            Prim::MergeClients,
            Scheme::mono(fun_eff(
                vec![],
                Ty::stream(Ty::con("Proposal")),
                Row::of([Effect::Ingress]),
            )),
        ),
        (
            "filter_map",
            Prim::StreamFilterMap,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![
                        Ty::stream(v(A)),
                        fun_eff(vec![v(A)], Ty::option(v(B)), Row::var(E)),
                    ],
                    Ty::stream(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        // §3.7: "`fold`'s function must be *replay-pure*: effect row ⊆ {}". That could be written
        // as a closed empty row here, and unification would reject an impure fold — with a message
        // about rows failing to unify. The row is a *variable* instead, so the row is inferred and
        // then judged by `place`, which can say which effect, where it came from, and why the rule
        // exists. A checked property is worth no more than the diagnostic that delivers it.
        (
            "fold",
            Prim::Fold,
            poly_eff(
                &[A, B],
                &[E],
                fun(
                    vec![
                        fun_eff(
                            vec![v(A), Ty::app(Ty::ENVELOPE, vec![v(B)])],
                            v(A),
                            Row::var(E),
                        ),
                        v(A),
                        Ty::stream(v(B)),
                    ],
                    Ty::signal(v(A)),
                ),
            ),
        ),
        (
            "durable",
            Prim::Durable,
            poly(
                &[A],
                fun_eff(
                    vec![Ty::signal(v(A))],
                    Ty::signal(v(A)),
                    Row::of([Effect::Durable]),
                ),
            ),
        ),
        // A signal edge carries its function's row to the signal, which is what makes a view that
        // reaches the log a *placement* error on the client rather than a runtime surprise.
        (
            "signal_map",
            Prim::SignalMap,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![Ty::signal(v(A)), fun_eff(vec![v(A)], v(B), Row::var(E))],
                    Ty::signal(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "map2",
            Prim::SignalMap2,
            poly_eff(
                &[A, B, C],
                &[E],
                fun_eff(
                    vec![
                        fun_eff(vec![v(A), v(B)], v(C), Row::var(E)),
                        Ty::signal(v(A)),
                        Ty::signal(v(B)),
                    ],
                    Ty::signal(v(C)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "per_session",
            Prim::PerSession,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![
                        Ty::signal(v(A)),
                        fun_eff(vec![v(A), Ty::con("Session")], v(B), Row::var(E)),
                    ],
                    Ty::signal(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        // `validate : (Session, Command) -> list[Event]` (§3.7), with the accumulator threaded so
        // that client-minted ids can be checked for freshness and ownership against the actor —
        // the two obligations F2 puts on validation and the todo sketch deliberately skips.
        (
            "decide",
            Prim::Decide,
            poly_eff(
                &[A, B, C],
                &[E],
                fun_eff(
                    vec![
                        Ty::stream(Ty::con("Proposal")),
                        Ty::signal(v(A)),
                        fun_eff(
                            vec![v(A), Ty::con("Proposal")],
                            Ty::app(Ty::RESULT, vec![Ty::list(v(B)), v(C)]),
                            Row::var(E),
                        ),
                    ],
                    Ty::stream(v(B)),
                    Row::var(E),
                ),
            ),
        ),
    ]
}

/// Types every program has: `Option`, `Result`, `Envelope`, `Session`, `Proposal`.
///
/// `Envelope` is §3.7's, field for field — "`seq`: position in the total order — assigned here,
/// nowhere else; `at`: wall-clock, captured as data; `actor`: stable authenticated identity —
/// **never** the live `Session` capability or token".
pub fn types() -> BTreeMap<Arc<str>, TyDecl> {
    let mut out = BTreeMap::new();
    let mut add = |d: TyDecl| {
        out.insert(d.name().clone(), d);
    };

    add(TyDecl::Union {
        name: Arc::from(Ty::OPTION),
        params: vec![Arc::from("T")],
        variants: vec![
            Variant {
                name: Arc::from("Some"),
                fields: vec![(Arc::from("value"), Ty::Var(A))],
            },
            Variant {
                name: Arc::from("None"),
                fields: vec![],
            },
        ],
    });
    add(TyDecl::Union {
        name: Arc::from(Ty::RESULT),
        params: vec![Arc::from("T"), Arc::from("E")],
        variants: vec![
            Variant {
                name: Arc::from("Ok"),
                fields: vec![(Arc::from("value"), Ty::Var(A))],
            },
            Variant {
                name: Arc::from("Err"),
                fields: vec![(Arc::from("error"), Ty::Var(B))],
            },
        ],
    });
    // JSON as data, so a program reads a document with `match` and builds one with ordinary
    // constructors. There is no reflection and no derive: `Json` is a union like any other, and
    // turning a `model` into one is a function somebody writes — which is what `@derive` is for
    // when it exists, and is not a reason to put a second kind of value in the language now.
    //
    // The variants are prefixed because a union's constructors are global names: `Str` and `Bool`
    // are taken, and `List` would be taken by anybody's own union the day they wrote one.
    add(TyDecl::Union {
        name: Arc::from("Json"),
        params: Vec::new(),
        variants: vec![
            Variant {
                name: Arc::from("JsonNull"),
                fields: vec![],
            },
            Variant {
                name: Arc::from("JsonBool"),
                fields: vec![(Arc::from("value"), Ty::bool_())],
            },
            // One number type, and it is the `Float` §32 built rather than a second numeric
            // tower: JSON's own grammar has one, and a reader who wants an integer asks for one.
            Variant {
                name: Arc::from("JsonNumber"),
                fields: vec![(Arc::from("value"), Ty::con(Ty::FLOAT))],
            },
            Variant {
                name: Arc::from("JsonStr"),
                fields: vec![(Arc::from("value"), Ty::str_())],
            },
            Variant {
                name: Arc::from("JsonList"),
                fields: vec![(Arc::from("items"), Ty::list(Ty::con("Json")))],
            },
            Variant {
                name: Arc::from("JsonObject"),
                fields: vec![(Arc::from("fields"), Ty::map(Ty::str_(), Ty::con("Json")))],
            },
        ],
    });
    // The error `json_parse` raises. A declared type rather than a `Str`, because
    // `docs/45` §45.1's atom names the type and a `raises(Str)` would make every string failure in
    // a program the same failure.
    add(TyDecl::Union {
        name: Arc::from("JsonError"),
        params: Vec::new(),
        variants: vec![Variant {
            name: Arc::from("BadJson"),
            fields: vec![(Arc::from("why"), Ty::str_())],
        }],
    });
    // The outbound call's three types. A request carries a port and *not* a host: the host is the
    // atom the call site performs, so it is an argument of `http_fetch` rather than a field
    // anything can compute.
    add(TyDecl::Model {
        name: Arc::from("HttpRequest"),
        params: Vec::new(),
        fields: vec![
            (Arc::from("method"), Ty::str_()),
            // Origin-form, sent as written: `/v1/todos?limit=10`. Nothing percent-encodes it,
            // because only the program that built it knows which part of it was data.
            (Arc::from("path"), Ty::str_()),
            // One value per name, which loses a repeated header (`Set-Cookie`). Said out loud
            // here rather than discovered: a `map` is what a program wants to read, and the day a
            // caller needs the repeats this becomes a `list[(Str, Str)]` and every reader changes.
            (Arc::from("headers"), Ty::map(Ty::str_(), Ty::str_())),
            (Arc::from("body"), Ty::str_()),
            (Arc::from("port"), Ty::int()),
            // Headers whose *value* is a secret, kept apart from the ones whose value is a `Str`.
            //
            // §3.5 makes a `secret[T]` unreadable — there is no `reveal` for one, which is the
            // whole claim — so `"Bearer " + key` cannot be written and an authenticated request
            // would be inexpressible. These are merged into the headers by the runtime at the
            // edge, so the credential goes on the wire without ever having been a `Str` the
            // program could put somewhere else. A request carrying one is not Sendable, which is
            // the property doing the work: it cannot be built on, or sent to, a client.
            (
                Arc::from("secrets"),
                Ty::map(Ty::str_(), Ty::secret(Ty::str_())),
            ),
        ],
    });
    add(TyDecl::Model {
        name: Arc::from("HttpResponse"),
        params: Vec::new(),
        fields: vec![
            (Arc::from("status"), Ty::int()),
            (Arc::from("headers"), Ty::map(Ty::str_(), Ty::str_())),
            (Arc::from("body"), Ty::str_()),
        ],
    });
    // A status is a reply and not a failure — a 500 arrived, and a program that treats it as an
    // exception has lost the body that says why. These are the cases where *nothing* arrived,
    // plus the one the library raises when a caller asks for a status it did not get.
    add(TyDecl::Union {
        name: Arc::from("HttpError"),
        params: Vec::new(),
        variants: vec![
            Variant {
                name: Arc::from("HttpUnreachable"),
                fields: vec![
                    (Arc::from("host"), Ty::str_()),
                    (Arc::from("why"), Ty::str_()),
                ],
            },
            Variant {
                name: Arc::from("HttpTimedOut"),
                fields: vec![
                    (Arc::from("host"), Ty::str_()),
                    (Arc::from("millis"), Ty::int()),
                ],
            },
            Variant {
                name: Arc::from("HttpBadResponse"),
                fields: vec![(Arc::from("why"), Ty::str_())],
            },
            // Raised by `lib/http.beck`'s `require_ok`, never by the primitive.
            Variant {
                name: Arc::from("HttpStatus"),
                fields: vec![
                    (Arc::from("status"), Ty::int()),
                    (Arc::from("body"), Ty::str_()),
                ],
            },
        ],
    });
    add(TyDecl::Union {
        name: Arc::from("TimeError"),
        params: Vec::new(),
        variants: vec![Variant {
            name: Arc::from("BadTime"),
            fields: vec![(Arc::from("why"), Ty::str_())],
        }],
    });
    // The two the decoders raise. Separate types rather than one `BadInput`, because a caller
    // reading a base64 field and a caller reading an identifier are recovering from different
    // things: the first re-reads the message, the second rejects the request.
    add(TyDecl::Union {
        name: Arc::from("EncodingError"),
        params: Vec::new(),
        variants: vec![Variant {
            name: Arc::from("BadEncoding"),
            fields: vec![
                (Arc::from("encoding"), Ty::str_()),
                (Arc::from("why"), Ty::str_()),
            ],
        }],
    });
    add(TyDecl::Union {
        name: Arc::from("UuidError"),
        params: Vec::new(),
        variants: vec![Variant {
            name: Arc::from("BadUuid"),
            fields: vec![(Arc::from("why"), Ty::str_())],
        }],
    });
    add(TyDecl::Model {
        name: Arc::from(Ty::ENVELOPE),
        params: vec![Arc::from("T")],
        fields: vec![
            (Arc::from("seq"), Ty::int()),
            (Arc::from("at"), Ty::int()),
            (Arc::from("actor"), Ty::str_()),
            (Arc::from("body"), Ty::Var(A)),
        ],
    });
    // "`Session` is minted by the identity subsystem … with verified claims mapped to typed
    // capabilities" (§3.7). Phase 1 carries the actor only — dev-mode identity, as Phase 0 had.
    add(TyDecl::Model {
        name: Arc::from("Session"),
        params: Vec::new(),
        fields: vec![(Arc::from("actor"), Ty::str_())],
    });
    add(TyDecl::Model {
        name: Arc::from("Proposal"),
        params: Vec::new(),
        fields: vec![
            (Arc::from("session"), Ty::con("Session")),
            (Arc::from("command"), Ty::con("Command")),
        ],
    });
    out
}

/// The traits every program has.
///
/// One, and it is the one SICP §2.5.1 builds by hand: **generic arithmetic**. The book's answer to
/// "how do rationals join a tower that already has integers" is a set of generic operations —
/// `add`, `sub`, `mul`, `div` — that each type installs an implementation for, and that is exactly
/// a trait. `docs/32` §32.3 resolved `+` from its operands and said an ad-hoc resolution was "the
/// honest thing to build before traits exist"; they exist, so `+` resolves through this when its
/// operands are neither `Int` nor `Float` nor `Str`.
///
/// The method names are the book's. A tower is only worth having if a third floor can be added
/// from outside the language, and `impl Num for Rational` is how §2.1.1's exercise stops being
/// about function names and starts being about data abstraction.
///
/// It is **not published**: `own_traits` is what a `.becki` carries, and this belongs to the
/// language rather than to any module. Nor is it implemented for `Int` or `Float` — those go
/// through the primitives, because a tower whose bottom floor is a dictionary call would make every
/// existing program slower to prove a point.
pub fn traits() -> Vec<TraitSig> {
    let binary = |name: &str| MethodSig {
        name: Arc::from(name),
        params: vec![
            (Arc::from("self"), Ty::con(SELF)),
            (Arc::from("other"), Ty::con(SELF)),
        ],
        ret: Ty::con(SELF),
        effects: Vec::new(),
    };
    vec![TraitSig {
        name: Arc::from(NUM),
        methods: vec![binary("add"), binary("sub"), binary("mul"), binary("div")],
    }]
}

/// The trait `+`, `-`, `*` and `/` resolve through.
pub const NUM: &str = "Num";

/// The abstract receiver a trait's signatures are written in terms of.
const SELF: &str = "Self";

/// Which method of [`NUM`] an operator is.
pub fn num_method(op: Prim) -> Option<&'static str> {
    Some(match op {
        Prim::Add => "add",
        Prim::Sub => "sub",
        Prim::Mul => "mul",
        Prim::Div => "div",
        _ => return None,
    })
}

/// The type-constructor arities the checker knows without a declaration.
pub fn builtin_arity(name: &str) -> Option<usize> {
    Some(match name {
        Ty::INT | Ty::STR | Ty::BOOL | Ty::FLOAT | Ty::UNIT | Ty::HTML | Ty::ATTR => 0,
        Ty::LIST
        | Ty::OPTION
        | Ty::STREAM
        | Ty::SIGNAL
        | Ty::ENVELOPE
        | Ty::SECRET
        | Ty::INTERNAL => 1,
        Ty::MAP | Ty::RESULT => 2,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prim_has_a_signature_and_the_names_are_unique() {
        let all = prims();
        let mut names: Vec<&str> = all.iter().map(|(n, _, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate prelude name");
        for (name, prim, _) in &all {
            assert_eq!(*name, prim.name(), "prelude name must match Prim::name");
        }
    }

    #[test]
    fn folds_type_the_way_section_3_7_says() {
        // `fold(f, init, s) : Signal[S]` where `f : (S, Envelope[E]) -> S`.
        let all = prims();
        let (_, _, scheme) = all
            .iter()
            .find(|(n, _, _)| *n == "fold")
            .expect("fold exists");
        match &scheme.ty {
            Ty::Fun(params, ret, _) => {
                assert_eq!(params.len(), 3);
                assert_eq!(ret.con_name(), Some(Ty::SIGNAL));
                assert!(matches!(&params[0], Ty::Fun(ps, _, _) if ps.len() == 2));
                assert_eq!(params[2].con_name(), Some(Ty::STREAM));
            }
            other => panic!("fold should be a function, got {other}"),
        }
    }

    #[test]
    fn the_standard_library_is_effect_polymorphic_where_section_3_2_says_it_is() {
        // "Effect polymorphism is what keeps one standard library." If `map_list` were monomorphic
        // in its function's row there would have to be a pure `map` and an effectful `map`, and the
        // choice would be the caller's problem rather than the compiler's.
        let all = prims();
        for name in [
            "map_list",
            "filter_list",
            "sort_by",
            "signal_map",
            "per_session",
            "decide",
        ] {
            let (_, _, scheme) = all
                .iter()
                .find(|(n, _, _)| *n == name)
                .unwrap_or_else(|| panic!("{name} exists"));
            assert!(
                !scheme.row_vars.is_empty(),
                "`{name}` takes a function, so it must be polymorphic in that function's row"
            );
        }
        // …and the effectful primitives carry their atom, closed.
        for (name, atom) in [
            ("merge_clients", Effect::Ingress),
            ("durable", Effect::Durable),
            ("uuid", Effect::Nondet),
            ("now", Effect::Nondet),
            ("secret_env", Effect::Env),
        ] {
            let (_, _, scheme) = all.iter().find(|(n, _, _)| *n == name).unwrap();
            let Ty::Fun(_, _, row) = &scheme.ty else {
                panic!("{name} is a function")
            };
            assert!(
                row.atoms.contains(&atom),
                "`{name}` should perform `{atom}`"
            );
        }
    }

    #[test]
    fn every_primitives_own_atoms_agree_with_its_scheme() {
        // Two statements of the same fact — the table `Prim::effects` returns and the row in the
        // scheme — so a primitive cannot acquire an effect in one and not the other.
        for (name, prim, scheme) in prims() {
            let Ty::Fun(_, _, row) = &scheme.ty else {
                continue;
            };
            for e in &prim.effects() {
                assert!(
                    row.atoms.contains(e),
                    "`{name}` performs `{e}` but its scheme does not say so"
                );
            }
            for e in &row.atoms {
                assert!(
                    prim.effects().contains(e),
                    "`{name}`'s scheme carries `{e}` but `Prim::effects` does not"
                );
            }
        }
    }

    #[test]
    fn the_envelope_carries_an_actor_and_never_a_session() {
        let ts = types();
        match ts.get(Ty::ENVELOPE).expect("Envelope exists") {
            TyDecl::Model { fields, .. } => {
                let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_ref()).collect();
                assert_eq!(names, ["seq", "at", "actor", "body"]);
                assert!(!names.contains(&"session"), "F5: no capability in the log");
            }
            other => panic!("Envelope should be a model, got {other:?}"),
        }
    }
}
