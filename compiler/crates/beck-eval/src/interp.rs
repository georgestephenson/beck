//! The tree-walker itself. [`crate`] wraps it as a [`beck_core::backend::Backend`].
//!
//! Deliberately bad at everything, complete end-to-end
//! ([`docs/08-roadmap.md`](../../../../../docs/08-roadmap.md) Phase 1). It is a tree-walker over
//! typed `Core`, with three properties that are not negotiable and are tested:
//!
//! * **Replay purity.** Nothing here reads a clock, a random source, or a socket *of its own*.
//!   `uuid()` is a primitive the *checker* refuses inside a fold (§3.7), and even outside one it
//!   is supplied by the host rather than taken from the ambient environment; `http_fetch` goes to
//!   [`beck_core::host::Atoms::fetch`] for the same reason. A replay is reproducible because every reading of the
//!   outside world enters through the host, where a replay can decide what it says.
//! * **Total order everywhere.** Maps are `BTreeMap`s and `sort_by` is stable, so two runs over
//!   the same log render identically — Phase 0 §18.5 item 4 learned this the hard way.
//! * **Errors are values, not panics.** A partial operation returns an [`EvalError`] carrying the
//!   span, because a language server has to survive evaluating half-written code.
//!
//! To which a fourth was added by [`docs/27-the-walls-come-down-report.md`](../../../../../docs/27-the-walls-come-down-report.md):
//!
//! * **A call in tail position does not grow the host stack, and no program aborts the process.**
//!   [`Interp::eval`] is a trampoline: `Interp::step` walks the tail positions of a body without
//!   recursing and hands the call it lands on back to the loop, so an iterative process is
//!   iterative (SICP §1.2.1, R7RS §3.5). Recursion that is *not* in tail position still spends a
//!   host frame, so it is bounded by a counted depth — deterministically, because a limit read off
//!   the host's remaining stack would make a fold's outcome depend on the build profile and §3.7
//!   needs it to depend only on the log.

use std::sync::Arc;

use beck_diag::Span;

use beck_core::core::{
    Closure, Const, Core, CoreKind, Env, Fields, Pattern, Prim, Record, Value, VarId,
};
use beck_core::digest;
use beck_core::PMap;

/// The three "this primitive wanted a `T`" conversions, so twenty-odd library primitives do not
/// each spell out the same `ok_or_else`. The message names the primitive, because "expects a Str"
/// with no subject is the least useful diagnostic a runtime can produce.
fn as_str<'a>(v: &'a Value, who: &str, span: Span) -> Result<&'a str, EvalError> {
    v.as_str()
        .ok_or_else(|| EvalError::new(format!("`{who}` expects a Str"), span))
}

/// The digest, encoding and identifier primitives.
///
/// Out of line on purpose — see the arm in [`Interp::prim`] that calls it.
#[inline(never)]
fn digest_prim(op: Prim, mut args: Vec<Value>, span: Span) -> Result<Value, EvalError> {
    let arity = match op {
        Prim::DigestKeyed | Prim::DigestEq => 2,
        _ => 1,
    };
    if args.len() != arity {
        return Err(EvalError::new(
            format!(
                "`{}` takes {arity} argument(s), given {}",
                op.name(),
                args.len()
            ),
            span,
        ));
    }
    match op {
        Prim::Digest => {
            let v = args.pop().expect("arity checked");
            Ok(Value::str_(digest::of(as_str(&v, "digest", span)?)))
        }
        Prim::DigestKeyed => {
            let message = args.pop().expect("arity checked");
            let key = args.pop().expect("arity checked");
            // The key arrives as the `secret[Str]` newtype `secret_env` built, and this is the one
            // place in the tree that takes one apart. What comes back is a code, which is what the
            // capability in the row is charged for.
            let key = key
                .field("value")
                .and_then(Value::as_str)
                .ok_or_else(|| EvalError::new("`digest_keyed` expects a `secret[Str]`", span))?;
            Ok(Value::str_(digest::keyed(
                key,
                as_str(&message, "digest_keyed", span)?,
            )))
        }
        Prim::DigestEq => {
            let b = args.pop().expect("arity checked");
            let a = args.pop().expect("arity checked");
            Ok(Value::Bool(digest::same(
                as_str(&a, "digest_eq", span)?,
                as_str(&b, "digest_eq", span)?,
            )))
        }
        Prim::HexEncode | Prim::Base64Encode => {
            let v = args.pop().expect("arity checked");
            let text = as_str(&v, op.name(), span)?;
            Ok(Value::str_(if op == Prim::HexEncode {
                digest::hex_encode(text)
            } else {
                digest::base64_encode(text)
            }))
        }
        Prim::HexDecode | Prim::Base64Decode => {
            let v = args.pop().expect("arity checked");
            let text = as_str(&v, op.name(), span)?;
            let (encoding, decoded) = if op == Prim::HexDecode {
                ("hex", digest::hex_decode(text))
            } else {
                ("base64", digest::base64_decode(text))
            };
            decoded.map(Value::str_).map_err(|why| {
                raised(
                    "EncodingError",
                    "BadEncoding",
                    [
                        ("encoding", Value::str_(encoding)),
                        ("why", Value::str_(why)),
                    ],
                    span,
                )
            })
        }
        // `uuid_parse` and `uuid_version` differ only in which half of the answer they keep.
        _ => {
            let v = args.pop().expect("arity checked");
            let text = as_str(&v, op.name(), span)?;
            let canonical = digest::uuid_normalise(text)
                .map_err(|why| raised("UuidError", "BadUuid", [("why", Value::str_(why))], span))?;
            Ok(if op == Prim::UuidParse {
                Value::str_(canonical)
            } else {
                Value::Int(digest::uuid_version(&canonical))
            })
        }
    }
}

/// A raised value of a prelude-declared union, built from its variant's fields.
///
/// The shape `json_parse` writes out inline; `time_parse` and the three decoders raise four of
/// these between them, and four copies of a `Fields::from_iter` would be four chances to name a
/// field something the prelude does not declare.
fn raised<const N: usize>(
    ty: &str,
    variant: &str,
    fields: [(&str, Value); N],
    span: Span,
) -> EvalError {
    EvalError::raise(
        Arc::from(ty),
        Value::data(
            Arc::from(ty),
            Some(Arc::from(variant)),
            fields
                .into_iter()
                .map(|(n, v)| (Arc::from(n), v))
                .collect::<Fields>(),
        ),
        span,
    )
}

fn as_int(v: &Value, who: &str, span: Span) -> Result<i64, EvalError> {
    v.as_int()
        .ok_or_else(|| EvalError::new(format!("`{who}` expects an Int"), span))
}

fn as_list<'a>(v: &'a Value, who: &str, span: Span) -> Result<&'a Vec<Value>, EvalError> {
    v.as_list()
        .ok_or_else(|| EvalError::new(format!("`{who}` expects a list"), span))
}

// ------------------------------------------------------------------------------------ JSON

fn json_node(variant: &str, field: &str, value: Value) -> Value {
    Value::data(
        Arc::from("Json"),
        Some(Arc::from(variant)),
        Fields::from_iter([(Arc::from(field), value)]),
    )
}

fn json_to_value(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::data(
            Arc::from("Json"),
            Some(Arc::from("JsonNull")),
            Fields::new(),
        ),
        serde_json::Value::Bool(b) => json_node("JsonBool", "value", Value::Bool(*b)),
        // JSON has one number type and so does this union, which is why an integer document reads
        // back as a `Float` and a caller who wants an `Int` says so.
        serde_json::Value::Number(n) => json_node(
            "JsonNumber",
            "value",
            Value::float(n.as_f64().unwrap_or(0.0)),
        ),
        serde_json::Value::String(s) => json_node("JsonStr", "value", Value::str_(s)),
        serde_json::Value::Array(xs) => json_node(
            "JsonList",
            "items",
            Value::List(Arc::new(xs.iter().map(json_to_value).collect())),
        ),
        serde_json::Value::Object(fields) => {
            let mut m = beck_core::PMap::new();
            for (k, v) in fields {
                m = m.insert(Value::str_(k), json_to_value(v));
            }
            json_node("JsonObject", "fields", Value::Map(m))
        }
    }
}

fn value_to_json(v: &Value, span: Span) -> Result<serde_json::Value, EvalError> {
    let field = |name: &str| v.field(name).cloned();
    match v.variant() {
        Some("JsonNull") => Ok(serde_json::Value::Null),
        Some("JsonBool") => Ok(serde_json::Value::Bool(
            field("value").and_then(|x| x.as_bool()).unwrap_or(false),
        )),
        Some("JsonNumber") => {
            let n = field("value").and_then(|x| x.as_f64()).unwrap_or(0.0);
            Ok(serde_json::Number::from_f64(n)
                .map(serde_json::Value::Number)
                // A non-finite float has no JSON spelling at all, so `null` is the only total
                // answer and is what every encoder in every language does here.
                .unwrap_or(serde_json::Value::Null))
        }
        Some("JsonStr") => Ok(serde_json::Value::String(
            field("value")
                .and_then(|x| x.as_str().map(|s| s.to_string()))
                .unwrap_or_default(),
        )),
        Some("JsonList") => {
            let items = field("items");
            let xs = items
                .as_ref()
                .and_then(|x| x.as_list())
                .ok_or_else(|| EvalError::new("`json_render` expects a list of Json", span))?;
            Ok(serde_json::Value::Array(
                xs.iter()
                    .map(|x| value_to_json(x, span))
                    .collect::<Result<_, _>>()?,
            ))
        }
        Some("JsonObject") => {
            let fields = field("fields");
            let m = fields
                .as_ref()
                .and_then(|x| x.as_map())
                .ok_or_else(|| EvalError::new("`json_render` expects a Map of Json", span))?;
            let mut out = serde_json::Map::new();
            for (k, val) in m.iter() {
                out.insert(
                    k.as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| k.display()),
                    value_to_json(val, span)?,
                );
            }
            Ok(serde_json::Value::Object(out))
        }
        _ => Err(EvalError::new("`json_render` expects a Json", span)),
    }
}

#[derive(Clone, Debug)]
pub struct EvalError {
    pub message: String,
    pub span: Span,
    /// The value a `raise` failed with, if this is a raise rather than a fault.
    ///
    /// The two travel the same way — a raise unwinds — and are distinguished here rather than in a
    /// second error type, because everything between the raise and its handler has to pass both
    /// along unchanged and neither of them is the ordinary path. `Prim::Try` is the only reader.
    pub raised: Option<Box<(Arc<str>, Value)>>,
    /// Whether this is a `parallel:` child that was **stopped because a sibling failed**, rather
    /// than a failure of its own.
    ///
    /// It is a flag rather than a message because the scope has to *discard* it: the answer a
    /// scope gives is the earliest child in source order that failed for a reason of its own, and
    /// a child that was cancelled did not fail — it was not allowed to finish. Nothing outside
    /// a `parallel:` scope reads this, and nothing outside one can produce it.
    pub cancelled: bool,
}

impl EvalError {
    pub fn new(message: impl Into<String>, span: Span) -> EvalError {
        EvalError {
            message: message.into(),
            span,
            raised: None,
            cancelled: false,
        }
    }

    /// A failure a program *chose*, carrying the value it chose to fail with and that value's type.
    pub fn raise(ty: Arc<str>, value: Value, span: Span) -> EvalError {
        EvalError {
            message: format!("raised `{}`", value.display()),
            span,
            raised: Some(Box::new((ty, value))),
            cancelled: false,
        }
    }

    /// A child that was stopped because a sibling failed.
    fn cancelled(span: Span) -> EvalError {
        EvalError {
            message: "a sibling in this `parallel:` scope failed, so this child was stopped".into(),
            span,
            raised: None,
            cancelled: true,
        }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EvalError {}

pub type EvalResult = Result<Value, EvalError>;

/// What the evaluator needs from outside itself: the top-level definitions, the stub seam, and —
/// through [`beck_core::host::Atoms`] — the impure capabilities a host supplies rather than the
/// program taking.
///
/// The four atoms are on that trait rather than on this one because the native backends ask the
/// same four questions, and two descriptions of one host would make the differential between the
/// backends a comparison of the descriptions rather than of the program.
pub trait Host: beck_core::host::Atoms {
    fn global(&self, name: &str) -> Option<&Core>;

    /// Answer a call to a top-level definition without running its body.
    ///
    /// The evaluator's half of [`beck_core::backend::Interceptor`]. Defaulted to "run the real
    /// thing", so the only host that behaves differently is the one a `beck test` run installs.
    fn intercept(&self, _name: &str, _args: &[Value]) -> Option<Value> {
        None
    }

    /// Whether [`Host::intercept`] can ever answer, asked once per call so that a host with no
    /// stubs installed does not pay for the arguments to be gathered into a slice.
    ///
    /// Defaulted to "yes" rather than "no": a host that overrides `intercept` and forgets this one
    /// is slower than it needs to be, which is the harmless direction.
    fn intercepts(&self) -> bool {
        true
    }
}

/// The signal that stops a `parallel:` scope's children when one of them fails.
///
/// # It stops the children *after* the failure, and only those
///
/// The obvious signal — a flag any failing child sets, which stops every sibling — is **wrong**,
/// and wrong in the one way that matters: it makes the scope's answer depend on which thread won.
/// Two children that both raise are a race, and whichever got there first would cancel the other
/// before it could raise, so a scope over `bad(2)` and `bad(1)` would report `Second` or `First`
/// depending on the scheduler. That is exactly the property
/// [`docs/80`](../../../../../docs/80-structured-concurrency-report.md) §80.3 exists to keep,
/// and `a_childs_failure_joins_at_the_scope_and_the_earliest_child_wins` caught it — eight failures
/// in forty runs of the suite, and none when run alone.
///
/// So what is stored is not "somebody failed" but **the lowest index that has failed**, and a child
/// is stopped only when a child *before* it has. That set is precisely the one an ordered join
/// would never have reached: under one, a failure at child `i` means children after `i` never ran,
/// and children before `i` had already finished. Cancelling exactly the former preserves the
/// ordered semantics bit for bit, and this is a change in *when* work stops rather than in what the
/// scope answers.
///
/// # Why it is a chain
///
/// A scope may nest inside a scope, and a grandchild has to stop when an enclosing scope's earlier
/// child fails. One link cannot say that, so each scope keeps the scope it is inside **and this
/// scope's own index within it**, and asking walks the chain. It is as deep as the scopes are
/// nested, which `a_scope_nests_inside_a_scope` puts at two.
#[derive(Debug)]
pub struct Cancel {
    /// The lowest child index that has failed for a reason of its own, or [`usize::MAX`].
    first_failed: std::sync::atomic::AtomicUsize,
    outer: Option<(Arc<Cancel>, usize)>,
}

impl Cancel {
    fn under(outer: Option<(Arc<Cancel>, usize)>) -> Arc<Cancel> {
        Arc::new(Cancel {
            first_failed: std::sync::atomic::AtomicUsize::new(usize::MAX),
            outer,
        })
    }

    /// Record that child `index` failed. `fetch_min`, so two failures leave the earlier one.
    fn failed(&self, index: usize) {
        self.first_failed
            .fetch_min(index, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the child at `mine` should stop: a child before it failed, or an enclosing scope
    /// says so.
    fn asked(&self, mine: usize) -> bool {
        if self.first_failed.load(std::sync::atomic::Ordering::Relaxed) < mine {
            return true;
        }
        self.outer
            .as_ref()
            .is_some_and(|(outer, ours)| outer.asked(*ours))
    }
}

pub struct Interp<'h> {
    pub host: &'h dyn Host,
    /// Bounded so that a non-terminating program in a request handler cannot wedge the server.
    fuel: std::cell::Cell<u64>,
    /// How many host frames the evaluator is currently holding, and the ceiling on that number.
    ///
    /// Fuel bounds *time*; this bounds *space*, and they are not the same failure. A program can
    /// spend a trivial amount of fuel and still recurse past the end of the thread's stack — which
    /// used to abort the process, taking the server and any diagnostic with it.
    depth: std::cell::Cell<u32>,
    max_depth: u32,
    /// The closure a top-level definition evaluates to, kept after the first time it is asked for.
    ///
    /// A definition *is* a lambda, and a lambda evaluates to a closure over the empty environment —
    /// so every reference to `f` produces a value equal to every other, and building it again meant
    /// a name lookup in the host, a copy of the parameter list, an `Arc` for the closure and a
    /// clone of the environment, **per call**. `docs/70` §70.9 named this as what remained once the
    /// body stopped being copied; it is a third of what a call cost.
    ///
    /// Only a `Closure` is cached. Nothing else a global can evaluate to is guaranteed to be the
    /// same value twice, and the cache is not the place to decide that.
    globals: std::cell::RefCell<std::collections::HashMap<Arc<str>, Value, BuildNameHasher>>,
    /// The scope this interpreter is a child of, if it is one.
    ///
    /// `None` for everything that is not a `parallel:` child, which is every interpreter the
    /// runtime, the LSP and `beck test` build — so the check in [`Interp::burn`] is a branch on a
    /// discriminant that is `None` for the whole of a program that never writes `parallel:`, and
    /// is loop-invariant where it is not. `docs/80` §80.8 is what that costs, measured.
    ///
    /// The index is this child's own place among its siblings, which is what makes the question
    /// "did a child *before* me fail" answerable — see [`Cancel`].
    cancel: Option<(Arc<Cancel>, usize)>,
}

/// A hash for definition names, on the path a call takes.
///
/// The standard-library hasher is SipHash, chosen to be hard to force collisions in. Nothing here
/// is attacker-supplied: the keys are the names of the definitions the program itself declares, and
/// the map is rebuilt per evaluation. This is FxHash — the multiply-and-rotate the Rust compiler
/// uses for its own interners — over the name's bytes.
#[derive(Default, Clone, Copy)]
pub struct BuildNameHasher;

impl std::hash::BuildHasher for BuildNameHasher {
    type Hasher = NameHasher;
    fn build_hasher(&self) -> NameHasher {
        NameHasher(0)
    }
}

pub struct NameHasher(u64);

impl NameHasher {
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(Self::SEED);
    }
}

impl std::hash::Hasher for NameHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some((word, tail)) = rest.split_first_chunk::<8>() {
            self.add(u64::from_le_bytes(*word));
            rest = tail;
        }
        let mut last = [0u8; 8];
        last[..rest.len()].copy_from_slice(rest);
        self.add(u64::from_le_bytes(last));
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The number of evaluation steps one call gets before it is stopped.
///
/// A runaway-program backstop, not a performance knob — but a backstop nothing can raise is a
/// ceiling, and three of Are We Fast Yet's fourteen benchmarks live above it at the size the suite
/// measures at ([`docs/53`](../../../../../docs/53-are-we-fast-yet-report.md) §53.3). `beck test --fuel`
/// is how a program that means it says so.
pub const DEFAULT_FUEL: u64 = 50_000_000;

/// The ceiling on non-tail evaluator nesting.
///
/// It is a *count*, not a measurement of the host stack, and that is the load-bearing choice: a
/// budget read from the stack pointer would let the same program and the same log evaluate in a
/// release build and refuse in a debug one, and §3.7 requires that a fold's result be a function
/// of the log alone. Two runs of `beck replay` on the same log agree about this the way they agree
/// about everything else.
///
/// The number the *host* has to supply so that this count is reachable is
/// [`crate::STACK_BYTES`], and `the_depth_ceiling_fits_the_smallest_stack_we_run_on` measures the
/// bytes one level actually costs rather than assuming them.
pub const DEFAULT_MAX_DEPTH: u32 = 4_000;

/// Read a local: **move** it out of the frame when the compiler proved this is its last read,
/// and clone it otherwise.
///
/// `last_use` is [`beck_core::liveness`]'s promise that no later evaluation in this body reads the
/// binding, and [`Env::take`] only empties a frame nothing else holds — so a value that arrives
/// here from a last read is one nobody else has, which is what lets `list_append` push into a list
/// instead of copying it (`docs/46` §46.14).
///
/// `#[inline]`, unlike [`Interp::leaf`]'s arms: this is the single hottest node in the interpreter
/// — every argument, every condition and every operand is one — and a call here costs a few percent
/// of every benchmark in the tree. Its locals are a `bool` and a reference, so the frame it widens
/// is a frame nobody notices; the arms `leaf` keeps out of line are the ones holding values.
#[inline]
fn read_var(c: &Core, v: VarId, env: &mut Env) -> EvalResult {
    env.read(v, c.last_use)
        .ok_or_else(|| EvalError::new(format!("unbound variable {v} at runtime"), c.span))
}

/// How much a primitive will touch, in elements, from the arguments it was given.
///
/// The budget used to count **nodes**, so `sort_by` over a million values and `list_len` over the
/// same list were both one step. `docs/70` §70.6 is the proof that this is not a bound on work at
/// all: over a loop whose wall clock quadrupled per doubling, the step count exactly doubled.
///
/// Only primitives whose cost is **proportional to a length the caller chose** appear here. A
/// constant-time one — `list_get`, `list_len`, `map_len`, `str_len` — is already bounded by the node
/// count, and charging it a length would make an ordinary indexed loop over a long list run out of
/// fuel for doing nothing wrong. `sort_by` is charged its `n` rather than `n log n`, which
/// understates it by a factor the budget does not need to be precise about.
fn work_of(op: Prim, args: &[Value]) -> usize {
    let list_len = |i: usize| match args.get(i) {
        Some(Value::List(xs)) => xs.len(),
        _ => 0,
    };
    let map_len = |i: usize| match args.get(i) {
        Some(Value::Map(m)) => m.len(),
        _ => 0,
    };
    let str_len = |i: usize| match args.get(i) {
        Some(Value::Str(t)) => t.len(),
        _ => 0,
    };
    match op {
        // A slice costs what it *takes*, not what it is taken from — charging the whole list makes
        // `str_join(list_slice(chars, i, k), "")` over a 10,245-element list cost 10,245 instead of
        // `k`, which is a 500× overcharge and was the first thing this accounting got wrong.
        Prim::ListSlice | Prim::ListTake => {
            match args.get(if op == Prim::ListSlice { 2 } else { 1 }) {
                Some(Value::Int(n)) => ((*n).max(0) as usize).min(list_len(0)),
                _ => 0,
            }
        }
        Prim::ListDrop => list_len(0).saturating_sub(match args.get(1) {
            Some(Value::Int(n)) => (*n).max(0) as usize,
            _ => 0,
        }),
        // Walks or rebuilds a whole list.
        Prim::ListReverse
        | Prim::ListContains
        | Prim::ListIndexOf
        | Prim::ListFold
        | Prim::ListAll
        | Prim::ListAny
        | Prim::ListFlatMap
        | Prim::MapList
        | Prim::FilterList
        | Prim::SortBy => list_len(0),
        Prim::ListZip => list_len(0) + list_len(1),
        Prim::ConcatLists => match args.first() {
            Some(Value::List(xs)) => xs
                .iter()
                .map(|x| match x {
                    Value::List(inner) => inner.len(),
                    _ => 1,
                })
                .sum(),
            _ => 0,
        },
        // Walks or rebuilds a map. `map_insert` and `map_remove` rebuild one path, not the tree.
        Prim::MapKeys | Prim::MapValues | Prim::MapMerge => map_len(0),
        // Walks a string. `str_len` is absent deliberately: it is O(1) since `docs/70`.
        // Joining costs the text it produces, which is the parts and not their number.
        Prim::StrJoin => match args.first() {
            Some(Value::List(xs)) => xs
                .iter()
                .map(|x| match x {
                    Value::Str(t) => t.len(),
                    _ => 1,
                })
                .sum(),
            _ => 0,
        },
        Prim::StrSplit
        | Prim::StrChars
        | Prim::StrContains
        | Prim::StrStartsWith
        | Prim::StrEndsWith
        | Prim::StrIndexOf
        | Prim::StrUpper
        | Prim::StrLower
        | Prim::StrReplace
        | Prim::StrTrim
        | Prim::StrToInt
        | Prim::Digest
        | Prim::DigestKeyed
        | Prim::DigestEq
        | Prim::HexEncode
        | Prim::HexDecode
        | Prim::Base64Encode
        | Prim::Base64Decode
        | Prim::JsonParse
        | Prim::JsonRender => str_len(0),
        // The length asked for, which is the only thing that bounds it.
        Prim::StrRepeat => str_len(0).saturating_mul(match args.get(1) {
            Some(Value::Int(n)) => (*n).max(0) as usize,
            _ => 0,
        }),
        // What it *takes*, which is the arm above's rule and was not applied here: `str_slice`
        // clamps, so "from `i` to the end" is ordinarily written with a length nobody bothered to
        // bound — and charging the number the caller wrote made `str_slice(s, 0, 1_000_000)` on a
        // five-character string cost a million steps. Found by the native differential, where the
        // compiled answer arrived and the evaluator's ran out of fuel (`docs/93` §93.6).
        Prim::StrSlice => {
            let chars = match args.first() {
                Some(Value::Str(t)) => t.chars_len(),
                _ => 0,
            };
            let at = |i: usize| match args.get(i) {
                Some(Value::Int(n)) => (*n).max(0) as usize,
                _ => 0,
            };
            at(2).min(chars.saturating_sub(at(1)))
        }
        _ => 0,
    }
}

/// One step of evaluation: a finished value, or a call in tail position that
/// [`Interp::eval`]'s loop should make *instead of* the one it is already making.
enum Step {
    Done(Value),
    Tail {
        callee: Arc<Closure>,
        args: Vec<Value>,
        span: Span,
    },
}

/// Decrements the depth counter however the frame it guards is left, including by `?`.
struct Frame<'i, 'h> {
    interp: &'i Interp<'h>,
}

impl Drop for Frame<'_, '_> {
    fn drop(&mut self) {
        self.interp
            .depth
            .set(self.interp.depth.get().saturating_sub(1));
    }
}

impl<'h> Interp<'h> {
    pub fn new(host: &'h dyn Host) -> Interp<'h> {
        Interp::with_fuel(host, DEFAULT_FUEL)
    }

    pub fn with_fuel(host: &'h dyn Host, fuel: u64) -> Interp<'h> {
        Interp {
            host,
            fuel: std::cell::Cell::new(fuel),
            depth: std::cell::Cell::new(0),
            max_depth: DEFAULT_MAX_DEPTH,
            globals: std::cell::RefCell::new(std::collections::HashMap::default()),
            cancel: None,
        }
    }

    /// The same, as child `index` of `cancel` — which stops it when an *earlier* child fails.
    fn under(host: &'h dyn Host, fuel: u64, cancel: Arc<Cancel>, index: usize) -> Interp<'h> {
        let mut interp = Interp::with_fuel(host, fuel);
        interp.cancel = Some((cancel, index));
        interp
    }

    /// Lower the depth ceiling. Only the harnesses use it: raising it is not offered, because the
    /// number that is safe is a property of the host stack rather than of the program.
    pub fn with_max_depth(mut self, max_depth: u32) -> Interp<'h> {
        self.max_depth = max_depth.min(DEFAULT_MAX_DEPTH);
        self
    }

    pub fn reset_fuel(&self) {
        self.fuel.set(DEFAULT_FUEL);
    }

    /// This interpreter's cancellation, as a question a host can ask while it is blocked.
    ///
    /// [`beck_core::net::Stop::never`] when there is no enclosing scope, which is the ordinary
    /// case and the one that lets a client skip the timer it would otherwise need: only a child of
    /// a `parallel:` can be cancelled at all.
    fn stop(&self) -> beck_core::net::Stop {
        match &self.cancel {
            Some((cancel, mine)) => {
                let (cancel, mine) = (Arc::clone(cancel), *mine);
                beck_core::net::Stop::when(move || cancel.asked(mine))
            }
            None => beck_core::net::Stop::never(),
        }
    }

    fn burn(&self, span: Span) -> Result<(), EvalError> {
        let left = self.fuel.get();
        if left == 0 {
            return Err(EvalError::new("evaluation ran out of fuel", span));
        }
        // Cancellation rides the step counter rather than getting a checkpoint of its own: this is
        // already the one place every evaluation step passes through, and "the program is making
        // progress" is exactly when stopping it is both possible and worth doing. A tail loop
        // spends fuel like anything else, so a child in one notices.
        if let Some((cancel, mine)) = &self.cancel {
            if cancel.asked(*mine) {
                return Err(EvalError::cancelled(span));
            }
        }
        self.fuel.set(left - 1);
        Ok(())
    }

    /// Charge for work a primitive does over `n` elements, on top of the one step the node cost.
    ///
    /// The budget counted **nodes** until `docs/70`, which meant it could not see a primitive that
    /// touched a million values: `list_slice` over a long list, a sort, a digest and a concatenation
    /// were each one step, so a program could do unbounded work inside a bounded number of them.
    /// `docs/70` §70.6 is the measurement that proves it — over a loop whose wall clock quadrupled
    /// per doubling, the step count exactly doubled.
    ///
    /// It is charged where the work is *proportional to a length the caller chose*, not for every
    /// allocation: the point is that the budget bounds what a program can make the evaluator do, and
    /// a constant is already bounded by the node count.
    fn burn_work(&self, n: usize, span: Span) -> Result<(), EvalError> {
        let left = self.fuel.get();
        let cost = n as u64;
        if left <= cost {
            self.fuel.set(0);
            return Err(EvalError::new("evaluation ran out of fuel", span));
        }
        self.fuel.set(left - cost);
        Ok(())
    }

    /// Take one level of host stack, or refuse — with a diagnostic a program can print, which is
    /// the whole difference from the abort this replaced.
    fn enter(&self, span: Span) -> Result<Frame<'_, 'h>, EvalError> {
        let depth = self.depth.get() + 1;
        if depth > self.max_depth {
            return Err(EvalError::new(
                format!(
                    "evaluation nested {} deep, which is the evaluator's limit — a call in *tail \
                     position* does not nest and has no limit, so a recursive process this deep \
                     has to be written as an iterative one (SICP \u{a7}1.2.1)",
                    self.max_depth
                ),
                span,
            ));
        }
        self.depth.set(depth);
        Ok(Frame { interp: self })
    }

    /// Run a `parallel:` scope's children — **at the same time**, on a thread each.
    ///
    /// # What makes this sound, and why it is three sentences rather than an analysis
    ///
    /// [`docs/80`](../../../../../docs/80-structured-concurrency-report.md) settled the hard
    /// half in the *checker*: the children are independent by construction, because none of them
    /// can name another, and no child may perform an effect another could observe. So the scope's
    /// answer does not depend on the order they ran in, and running them together is a correct
    /// implementation of the form rather than a reinterpretation of it. Nothing here re-derives
    /// that; it is a property of the program the checker already proved.
    ///
    /// # What each child gets, and what it shares
    ///
    /// | | |
    /// |---|---|
    /// | the host | **shared**, which is why [`beck_core::host::Atoms`] is `Send + Sync` |
    /// | a stack | **its own**, [`crate::STACK_BYTES`] of it, because a tree-walker nests frames |
    /// | the depth ceiling | **its own count** against the same ceiling — one per stack, which is what the ceiling was always about |
    /// | the globals cache | **its own**, rebuilt: sharing it wants a lock on the path a call takes, and `docs/70` §70.9 is what that path costs |
    /// | fuel | **a share of what is left**, and the paragraph below is why |
    ///
    /// # Fuel is split, not shared
    ///
    /// A shared budget is one atomic read-modify-write per evaluation step — on the hot path
    /// [`docs/70`](../../../../../docs/70-the-evaluator-gets-fast-report.md) spent a chapter on —
    /// and it makes *which* child runs out a race, so two runs of one program could differ in which
    /// error they report. Splitting is neither: each child gets an equal share of what remains, and
    /// what the scope charges the parent afterwards is the sum of what the children actually spent,
    /// so the **total** is what a serial run would have spent.
    ///
    /// What it costs is stated rather than hidden: a child that would have used more than its share
    /// runs out where a serial run would have let it continue. Fuel is a runaway-program backstop
    /// and not a performance knob ([`DEFAULT_FUEL`]), so dividing a backstop is a smaller change
    /// than it reads as — but it is a change, and `docs/80` §80.6 is where it is written down.
    ///
    /// # Failure
    ///
    /// The **first child in source order** that failed is the failure, whichever finished first, so
    /// the error a scope reports is a function of the program and not of the scheduler. Nothing is
    /// cancelled: §80.12's table says a backend that starts children together needs a cancellation
    /// signal and that nothing designs one, and this does not design one either — the siblings of a
    /// failed child run to completion and their answers are dropped.
    fn children(&self, thunks: &[Value], span: Span) -> Result<Vec<Value>, EvalError> {
        // One child is a scope somebody wrote for its shape rather than for its concurrency, and a
        // thread for it would be pure cost. This is the only case decided here rather than by
        // measurement, because it is not a threshold: there is nothing to overlap with.
        //
        // **`wasm32` has no threads**, and this crate is compiled to it twice — `beck-wasm` for a
        // Mode B client and `beck-play` for the playground, which runs a whole application in a
        // tab. A client cannot reach a scope at all (`spawn` is `Tier::Server`'s alone, and
        // `B0401` refuses a scope pinned to the browser), but the playground's server *half* can,
        // so this is reachable rather than theoretical. Running the children in order there is a
        // correct implementation of the form for §80's own reason — the order is unobservable, and
        // one order is an order — so what the playground loses is the overlap and never an answer.
        #[cfg(target_arch = "wasm32")]
        let alone = true;
        #[cfg(not(target_arch = "wasm32"))]
        let alone = thunks.len() < 2;
        if alone {
            let mut out = Vec::with_capacity(thunks.len());
            for thunk in thunks {
                out.push(self.apply(thunk, Vec::new(), span)?);
            }
            return Ok(out);
        }

        let each = self.fuel.get() / thunks.len() as u64;
        // The three things a child needs, taken out of `self` **before** the scope: an `Interp` is
        // deliberately not `Sync` — its fuel, its depth and its cache are `Cell`s on the hot path,
        // which is what `docs/70` bought — so what crosses to a thread is the host it borrows, two
        // numbers and the cancellation link, and never the interpreter that is running here.
        let (host, max_depth) = (self.host, self.max_depth);
        // Under the scope this one is inside, if any, so a grandchild stops when an outer scope
        // does. `Cancel::asked` is what walks it.
        let cancel = Cancel::under(self.cancel.clone());
        let mut answers: Vec<(EvalResult, u64)> = std::thread::scope(|scope| {
            let mut running = Vec::with_capacity(thunks.len());
            for (index, thunk) in thunks.iter().enumerate() {
                let cancel = cancel.clone();
                let handle = std::thread::Builder::new()
                    .stack_size(crate::STACK_BYTES)
                    .name("beck-parallel".into())
                    .spawn_scoped(scope, move || {
                        let child = Interp::under(host, each, cancel.clone(), index)
                            .with_max_depth(max_depth);
                        let answer = child.apply(thunk, Vec::new(), span);
                        // A child that failed for a reason of its own stops the children *after*
                        // it — here, as soon as it knows, rather than after the join, because a
                        // signal that waited for every child to finish would not be cancelling
                        // anything. `Cancel` is why it is "after" and not "every".
                        if answer.as_ref().is_err_and(|e| !e.cancelled) {
                            cancel.failed(index);
                        }
                        // Saturating rather than plain: `Interp::reset_fuel` is a
                        // public method, and a child whose budget went *up* should
                        // charge the parent nothing rather than panic.
                        (answer, each.saturating_sub(child.fuel.get()))
                    })
                    .expect("a thread for a parallel child");
                running.push(handle);
            }
            running
                .into_iter()
                // A panic in a child is the evaluator's own bug and not the program's — programs
                // get an `EvalError` — so it is put back on this thread rather than turned into
                // one, exactly as `on_the_evaluator_stack` does for the single-threaded case.
                .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                .collect()
        });

        // Charged before anything is answered, so a scope whose children failed still costs what
        // they spent — otherwise the cheapest way to run forever would be to fail.
        let spent: u64 = answers.iter().map(|(_, spent)| *spent).sum();
        self.fuel.set(self.fuel.get().saturating_sub(spent));

        // The earliest child in source order that failed **for a reason of its own**. A stopped
        // child did not fail, so its error is not an answer — and it cannot be the earliest one
        // either, since `Cancel` only ever stops children *after* the failure. Both halves are what
        // keep the scope's answer a function of the program rather than of the scheduler.
        if let Some((failed, _)) = answers
            .iter()
            .find(|(a, _)| a.as_ref().is_err_and(|e| !e.cancelled))
        {
            return Err(failed.as_ref().expect_err("just matched an error").clone());
        }
        let mut out = Vec::with_capacity(answers.len());
        for (answer, _) in answers.drain(..) {
            out.push(answer?);
        }
        Ok(out)
    }

    /// Apply a callable value to arguments — the entry point the runtime uses for `validate`,
    /// `apply_event` and `view`.
    pub fn apply(&self, f: &Value, args: Vec<Value>, span: Span) -> EvalResult {
        match f {
            Value::Closure(c) => {
                let mut env = bind(c, args.into_iter(), span)?;
                self.eval(&c.body, &mut env)
            }
            other => Err(EvalError::new(
                format!("not callable: {}", other.display()),
                span,
            )),
        }
    }

    /// Evaluate a top-level definition by name, and remember the closure it is.
    ///
    /// A definition is a lambda over the empty environment, so the value is the same every time and
    /// building it again per call was a name lookup, a parameter-list copy and two allocations.
    /// Anything that is *not* a closure is evaluated as before and not remembered.
    pub fn global(&self, name: &str, span: Span) -> EvalResult {
        if let Some(v) = self.globals.borrow().get(name) {
            return Ok(v.clone());
        }
        let Some(core) = self.host.global(name) else {
            return Err(EvalError::new(format!("no such definition: {name}"), span));
        };
        let v = self.eval(core, &mut Env::new())?;
        if matches!(v, Value::Closure(_)) {
            self.globals.borrow_mut().insert(Arc::from(name), v.clone());
        }
        Ok(v)
    }

    /// The trampoline.
    ///
    /// One host frame is taken here, and a call in tail position replaces the loop's state rather
    /// than nesting inside it — so `fact_iter`, `gcd` and `find_divisor` run in constant space and
    /// SICP §1.2.1's distinction between a recursive and an iterative *process* is observable
    /// (`docs/27` §27.2).
    pub fn eval(&self, c: &Core, env: &mut Env) -> EvalResult {
        let _frame = self.enter(c.span)?;
        let mut step = self.step(c, env)?;
        loop {
            let (callee, args, span) = match step {
                Step::Done(v) => return Ok(v),
                Step::Tail { callee, args, span } => (callee, args, span),
            };
            // The frame is owned here, which is what lets a last read move a value out of it
            // instead of copying it (`beck_core::liveness`).
            let mut env = bind(&callee, args.into_iter(), span)?;
            step = self.step(&callee.body, &mut env)?;
        }
    }

    /// Evaluate a subexpression that is *not* in tail position.
    ///
    /// Only five of `Core`'s kinds have a tail position in them — `If`, `Let`, `Match`, `App` and
    /// `Global` — and only those need the trampoline. Everything else can go straight to
    /// [`Interp::leaf`], and a constant or a variable can be answered here without so much as a
    /// depth check, because neither can contain a call and so neither can nest.
    ///
    /// This is where most of the trampoline's cost was paid back. Routing *every* subexpression
    /// through [`Interp::eval`] put a second host frame and a loop under each of them, and a real
    /// program is mostly these nodes. `docs/27` §27.2 has what the trampoline cost in the end.
    #[inline]
    fn operand(&self, c: &Core, env: &mut Env) -> EvalResult {
        match &c.kind {
            CoreKind::Const(k) => {
                self.burn(c.span)?;
                Ok(constant(k))
            }
            CoreKind::Var(v) => {
                self.burn(c.span)?;
                read_var(c, *v, env)
            }
            CoreKind::If { .. }
            | CoreKind::Let { .. }
            | CoreKind::Match { .. }
            | CoreKind::App { .. }
            | CoreKind::Global(_) => self.eval(c, env),
            _ => {
                // A frame, because these do evaluate subexpressions and so do nest.
                let _frame = self.enter(c.span)?;
                self.burn(c.span)?;
                self.leaf(c, env)
            }
        }
    }

    /// Walk a body's tail positions without recursing, and stop at the first thing that is either
    /// a value or a call.
    ///
    /// The subexpressions that are *not* in tail position — a condition, a `let`'s value, an
    /// argument — go through [`Interp::operand`] and may spend a frame, which is correct: their
    /// results are still needed when they return.
    ///
    /// This is a separate function from [`Interp::eval`] rather than one loop because of what
    /// `cur` borrows from. A tail call's body lives inside an `Arc<Closure>` the loop has just
    /// taken ownership of, and a `&Core` into a local the loop then reassigns is not something
    /// safe Rust will write. Returning the call to `eval` and re-entering here costs one host
    /// frame per *call* — not per level of recursion, which is the number that had to be zero.
    fn step<'a>(&'a self, c: &'a Core, env0: &mut Env) -> Result<Step, EvalError> {
        let mut cur: &'a Core = c;
        // The environment is only *replaced* by a `let` or a matched arm, and most nodes replace
        // nothing. Holding the caller's by reference until something extends it keeps two atomic
        // refcount operations off every node that does not — which is most of them.
        let mut owned: Option<Env> = None;
        loop {
            let env: &mut Env = match owned.as_mut() {
                Some(e) => e,
                None => &mut *env0,
            };
            self.burn(cur.span)?;
            match &cur.kind {
                CoreKind::If { cond, then, alt } => {
                    let c0 = self.operand(cond, env)?;
                    cur = match c0.as_bool() {
                        Some(true) => then,
                        Some(false) => alt,
                        None => return Err(EvalError::new("condition is not a Bool", cond.span)),
                    };
                }
                CoreKind::Let { var, value, body } => {
                    let v = self.operand(value, env)?;
                    // The call reserved a slot for this binding, so writing it costs nothing.
                    // `put` refuses if a closure has captured this environment or if the program
                    // was built without `beck_core::frames` having counted it, and then a `let`
                    // chains a scope of its own as it always did.
                    if let Err(v) = env.put_one(*var, v) {
                        owned = Some(env.extend(vec![(*var, v)]));
                    }
                    cur = body;
                }
                CoreKind::Match { scrutinee, arms } => {
                    let v = self.operand(scrutinee, env)?;
                    let mut hit = None;
                    for arm in arms {
                        let Some(bindings) = match_pattern(&arm.pattern, &v) else {
                            continue;
                        };
                        if let Some(guard) = &arm.guard {
                            // A guard reads what the pattern bound, so it needs a scope — and a
                            // scope of its own, because an arm whose guard is false has to leave
                            // the frame as it found it for the next arm to match into.
                            let mut scope = env.extend(bindings.clone());
                            let ok = self.operand(guard, &mut scope)?;
                            if ok.as_bool() != Some(true) {
                                continue;
                            }
                        }
                        hit = Some((bindings, &arm.body));
                        break;
                    }
                    let Some((bindings, body)) = hit else {
                        return Err(EvalError::new(
                            format!("no match arm applies to {}", v.display()),
                            cur.span,
                        ));
                    };
                    let mut bindings = bindings;
                    if !env.put(&mut bindings) {
                        owned = Some(env.extend(bindings));
                    }
                    cur = body;
                }
                CoreKind::App { func, args } => {
                    let mut vals = Vec::with_capacity(args.len());
                    for a in args {
                        vals.push(self.operand(a, env)?);
                    }
                    // A stub replaces the *call*, not the value: the arguments are evaluated first,
                    // because §21.3 rule 4 wants to answer "with what?" afterwards, and because a
                    // stubbed function's arguments are ordinary code the test still means to run.
                    //
                    // Only a direct call of a named definition is intercepted. A function passed as
                    // a value has lost its name by the time it is applied, and inventing one would
                    // be a guess; docs/22 records the limit rather than leaving it to be discovered.
                    //
                    // `intercepts` is asked first because it is a field test where `intercept` is a
                    // virtual call, and no stub is installed in all but a handful of runs.
                    if self.host.intercepts() {
                        if let CoreKind::Global(name) = &func.kind {
                            if let Some(v) = self.host.intercept(name, &vals) {
                                return Ok(Step::Done(v));
                            }
                        }
                    }
                    let f = self.operand(func, env)?;
                    let Value::Closure(callee) = f else {
                        return Err(EvalError::new(
                            format!("not callable: {}", f.display()),
                            cur.span,
                        ));
                    };
                    return Ok(Step::Tail {
                        callee,
                        args: vals,
                        span: cur.span,
                    });
                }
                // Not a tail position, but a *transparent* one: the value of a reference to a
                // top-level definition is the value of its body, in no environment at all. Walking
                // into it here rather than recursing saves a host frame on every call a program
                // makes, which is most of what the trampoline would otherwise have cost (§27.2).
                CoreKind::Global(name) => {
                    // This arm is reached once per call of a named function, and the value it
                    // produces is the same closure every time: a definition is a lambda over the
                    // empty environment. Cached, because building it again is a name lookup, a copy
                    // of the parameter list and two allocations — `docs/70` §70.9.
                    if let Some(v) = self.globals.borrow().get(name.as_ref()) {
                        return Ok(Step::Done(v.clone()));
                    }
                    let Some(body) = self.host.global(name) else {
                        return Err(EvalError::new(
                            format!("no such definition: {name}"),
                            cur.span,
                        ));
                    };
                    if let CoreKind::Lam { params, body: code } = &body.kind {
                        let v = Value::Closure(Arc::new(Closure {
                            params: Arc::clone(params),
                            body: Arc::clone(code),
                            env: Env::empty_shared(),
                            locals: body.locals,
                        }));
                        self.globals
                            .borrow_mut()
                            .insert(Arc::clone(name), v.clone());
                        return Ok(Step::Done(v));
                    }
                    // Anything that is not a lambda is evaluated as before and not remembered:
                    // nothing guarantees it is the same value twice.
                    owned = Some(Env::new());
                    cur = body;
                }
                CoreKind::Prim { op, args } => {
                    return self.eval_prim(*op, args, env, cur.span).map(Step::Done)
                }
                _ => return self.leaf(cur, env).map(Step::Done),
            }
        }
    }

    /// Everything with no tail position in it. Fuel has already been burnt for `c` by
    /// [`Interp::step`].
    ///
    /// Every arm with a local of its own is a separate `#[inline(never)]` method rather than a
    /// block, and that is not tidiness: an unoptimised build gives each arm's temporaries their own
    /// slot in the enclosing frame, so a single fat `match` on the recursive path was costing every
    /// level of a program's recursion the sum of the arms it did not take (`docs/27` §27.2).
    fn leaf(&self, c: &Core, env: &mut Env) -> EvalResult {
        match &c.kind {
            CoreKind::Const(k) => Ok(constant(k)),
            CoreKind::Var(v) => read_var(c, *v, env),
            CoreKind::Global(name) => self.global(name, c.span),
            // A refcount bump, not a copy of the code. This used to be `(**body).clone()` — a deep
            // copy of the whole function body, taken every time a `lam` node was evaluated, which
            // is once per call of a named function. `docs/70` §70.3 has what that cost.
            CoreKind::Lam { params, body } => Ok(Value::Closure(Arc::new(Closure {
                params: Arc::clone(params),
                body: Arc::clone(body),
                env: Arc::new(env.clone()),
                locals: c.locals,
            }))),
            CoreKind::Prim { op, args } => self.eval_prim(*op, args, env, c.span),
            CoreKind::Make {
                ty,
                variant,
                fields,
            } => self.eval_make(ty, variant.as_ref(), fields, c.order, env),
            CoreKind::Field { base, name } => self.eval_field(base, name, env, c.span),
            CoreKind::With { base, fields } => self.eval_with(base, fields, env, c.span),
            CoreKind::ListLit(items) => self.eval_list(items, env),
            CoreKind::MapLit(kvs) => self.eval_map(kvs, env),
            // `App`, `If`, `Let` and `Match` are the four kinds with a tail position in them, and
            // they never reach here: [`Interp::step`] consumes them without recursing, which is
            // what makes a tail call cost nothing.
            CoreKind::App { .. }
            | CoreKind::If { .. }
            | CoreKind::Let { .. }
            | CoreKind::Match { .. } => Err(EvalError::new(
                "internal: a tail-position node reached the leaf evaluator",
                c.span,
            )),
        }
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_prim(&self, op: Prim, args: &[Core], env: &mut Env, span: Span) -> EvalResult {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.operand(a, env)?);
        }
        self.prim(op, vals, span)
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_make(
        &self,
        ty: &Arc<str>,
        variant: Option<&Arc<str>>,
        fields: &[(Arc<str>, Core)],
        order: u32,
        env: &mut Env,
    ) -> EvalResult {
        // Evaluated in the order they are written, because a field expression can raise.
        let mut pairs = Vec::with_capacity(fields.len());
        for (name, expr) in fields {
            pairs.push((name.clone(), self.operand(expr, env)?));
        }
        // Then *placed*, when `beck_core::fields` decided where each one goes. A record's field
        // names are written in the source, so ordering them is work with a known answer, and this
        // is where the answer is spent: no comparison and no `memcmp`, in the vector that already
        // exists.
        let map = if order != beck_core::fields::UNORDERED
            && pairs.len() <= beck_core::fields::MAX_ORDERED
        {
            beck_core::fields::place(&mut pairs, order);
            Fields::from_sorted(pairs)
        } else {
            Fields::from_pairs(pairs)
        };
        Ok(Value::data(ty.clone(), variant.cloned(), map))
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_field(&self, base: &Core, name: &Arc<str>, env: &mut Env, span: Span) -> EvalResult {
        let v = self.operand(base, env)?;
        v.field(name)
            .cloned()
            .ok_or_else(|| EvalError::new(format!("no field `{name}` on {}", v.display()), span))
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_with(
        &self,
        base: &Core,
        fields: &[(Arc<str>, Core)],
        env: &mut Env,
        span: Span,
    ) -> EvalResult {
        let v = self.operand(base, env)?;
        let Value::Data(old) = v else {
            return Err(EvalError::new("`with` expects a record", span));
        };
        // The base of a `with` is usually a last read — `t.with(done=…)` — so when it is, the
        // record arrives here held by nobody else and is rebuilt rather than copied.
        let mut record = match Arc::try_unwrap(old) {
            // Nobody else holds it, so nothing else can be reading it: a field expression that
            // mentioned the base would have made the base's own read something other than a last
            // use, and this branch would not have been taken.
            Ok(owned) => owned,
            // Somebody does, and it is usually the field expressions themselves.
            // `x.with(f = g(x.f))` reads `x` twice — once as the base and once inside `g` — so a
            // straight clone leaves this copy holding `x.f` at the moment `g` reads it. That
            // second reference is what makes `list_append` copy instead of push, and it left the
            // accumulator idiom written with `with` at the `O(n²)` `docs/70` and `docs/70` removed
            // from every other spelling of it. The fields about to be replaced arrive empty
            // instead, which is one pass rather than the one the clone was already making
            // (`docs/63` §63.11).
            Err(shared) => Record {
                ty: shared.ty.clone(),
                variant: shared.variant.clone(),
                fields: Fields::from_sorted(
                    shared
                        .fields
                        .iter()
                        .map(|(name, value)| {
                            let replaced = fields.iter().any(|(f, _)| f == name);
                            (
                                name.clone(),
                                if replaced { Value::Unit } else { value.clone() },
                            )
                        })
                        .collect(),
                ),
            },
        };
        for (name, expr) in fields {
            let value = self.operand(expr, env)?;
            record.fields.insert(name.clone(), value);
        }
        Ok(Value::Data(Arc::new(record)))
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_list(&self, items: &[Core], env: &mut Env) -> EvalResult {
        let mut out = Vec::with_capacity(items.len());
        for i in items {
            out.push(self.operand(i, env)?);
        }
        Ok(Value::List(Arc::new(out)))
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_map(&self, kvs: &[(Core, Core)], env: &mut Env) -> EvalResult {
        let mut out = PMap::new();
        for (k, v) in kvs {
            out = out.insert(self.operand(k, env)?, self.operand(v, env)?);
        }
        Ok(Value::Map(out))
    }

    fn prim(&self, op: Prim, mut args: Vec<Value>, span: Span) -> EvalResult {
        // What this one will actually touch, charged before it touches it (`work_of`).
        let work = work_of(op, &args);
        if work > 0 {
            self.burn_work(work, span)?;
        }

        let want = |n: usize| -> Result<(), EvalError> {
            if args.len() == n {
                Ok(())
            } else {
                Err(EvalError::new(
                    format!("`{}` takes {n} arguments, got {}", op.name(), args.len()),
                    span,
                ))
            }
        };

        // An `Int` operation is checked and a `Float` one is not, and that asymmetry is the whole
        // difference between the two tiers of the tower: `i64` overflow has no representable
        // answer, so it is an error, while IEEE 754 defines one for every real operation —
        // including division by zero, whose answer is an infinity. Making `1.0 / 0.0` an error
        // would be inventing a rule the format already has (`docs/27` §27.2).
        macro_rules! arith {
            ($int:expr, $real:expr) => {{
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                match (&a, &b) {
                    (Value::Int(x), Value::Int(y)) => {
                        let f: fn(i64, i64) -> Option<i64> = $int;
                        f(*x, *y).map(Value::Int).ok_or_else(|| {
                            EvalError::new(
                                format!("`{}` overflowed or divided by zero", op.name()),
                                span,
                            )
                        })
                    }
                    (Value::Float(_), Value::Float(_)) => {
                        let f: fn(f64, f64) -> f64 = $real;
                        Ok(Value::float(f(
                            a.as_f64().expect("a Float"),
                            b.as_f64().expect("a Float"),
                        )))
                    }
                    _ => Err(EvalError::new(
                        format!("`{}` expects two Ints or two Floats", op.name()),
                        span,
                    )),
                }
            }};
        }

        match op {
            // `raise e` — unwind, carrying the value and its type name. The name is what a handler
            // matches on, so a `try:` for one error type does not swallow another.
            Prim::Raise => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let ty = match &v {
                    Value::Data(d) => d.ty.clone(),
                    other => Arc::from(other.display()),
                };
                Err(EvalError::raise(ty, v, span))
            }
            // `try: block` — run the thunk, and turn a raise of the named type into an `Err`.
            //
            // Anything else keeps travelling: a fault is not a failure, and a *different* error
            // type belongs to a handler further out. That is the whole reason the type name is an
            // argument rather than something this could infer.
            Prim::Try => {
                want(2)?;
                let caught = args.pop().expect("arity checked");
                let thunk = args.pop().expect("arity checked");
                let caught = caught.as_str().unwrap_or_default().to_string();
                match self.apply(&thunk, Vec::new(), span) {
                    Ok(v) => Ok(Value::ok(v)),
                    Err(e) => match &e.raised {
                        Some(r) if r.0.as_ref() == caught => Ok(Value::err(r.1.clone())),
                        _ => Err(e),
                    },
                }
            }
            // `parallel: block` — run the children, then the tail with their results bound.
            //
            // The children are run in the order they are written. The checker has proved that no
            // child can observe another (`check::observable_order`) and that none can name another,
            // so the scope's answer is the same whichever order they ran in — running them in one
            // is a correct implementation of a form whose meaning is that the order is nobody's
            // business. Nothing here runs them at the same time, and `docs/80` §80.12 says what
            // would have to change for something to.
            //
            // A child that raises stops the scope at that child. With an ordered join that is the
            // earliest failing child rather than the earliest failure, which is the deterministic
            // half of "cancellation is the error row crossing the scope".
            Prim::Parallel => {
                if args.len() < 2 {
                    return Err(EvalError::new(
                        "`parallel` expects its children and a continuation",
                        span,
                    ));
                }
                let k = args.pop().expect("length checked");
                let results = self.children(&args, span)?;
                self.apply(&k, results, span)
            }
            Prim::Add => {
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                // Strings first, and by *value*: `+` on two of them **pushes** into the left one
                // rather than copying both sides, when that one arrived from a last read and
                // nobody else holds it. It is what makes `done + piece` in a loop linear instead
                // of quadratic — `docs/70` §70.2 measured the quadratic, and `beck_core::liveness`
                // is what proves the ownership this consumes.
                if matches!(a, Value::Str(_)) && matches!(b, Value::Str(_)) {
                    let (Value::Str(x), Value::Str(y)) = (a, b) else {
                        unreachable!("just matched")
                    };
                    let left = match Arc::try_unwrap(x) {
                        Ok(owned) => owned,
                        Err(shared) => {
                            // Copied because somebody else holds it, so the copy is charged.
                            self.burn_work(shared.len(), span)?;
                            (*shared).clone()
                        }
                    };
                    return Ok(Value::Str(Arc::new(left.appended(y.as_str()))));
                }
                match (&a, &b) {
                    (Value::Int(x), Value::Int(y)) => x
                        .checked_add(*y)
                        .map(Value::Int)
                        .ok_or_else(|| EvalError::new("`+` overflowed", span)),
                    (Value::Float(_), Value::Float(_)) => Ok(Value::float(
                        a.as_f64().expect("a Float") + b.as_f64().expect("a Float"),
                    )),
                    _ => Err(EvalError::new(
                        "`+` expects two Ints, two Floats or two Strs",
                        span,
                    )),
                }
            }
            Prim::Sub => arith!(i64::checked_sub, |x, y| x - y),
            Prim::Mul => arith!(i64::checked_mul, |x, y| x * y),
            Prim::Div => arith!(i64::checked_div, |x, y| x / y),
            // `%` stays Int-only. `f64::rem` exists, but SICP never asks for it and a remainder
            // whose sign follows the dividend is a decision nothing here needs to take.
            Prim::Rem => arith!(i64::checked_rem, |x, _| x),
            Prim::Neg => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                match v {
                    // Checked, like every other integer operation here: `-i64::MIN` has no
                    // representable answer, so it is an error carrying a span and not a wrapped
                    // result.
                    Value::Int(x) => x
                        .checked_neg()
                        .map(Value::Int)
                        .ok_or_else(|| EvalError::new("`negate` overflowed", span)),
                    Value::Float(_) => Ok(Value::float(-v.as_f64().expect("a Float"))),
                    _ => Err(EvalError::new("`-` expects an Int or a Float", span)),
                }
            }
            Prim::Abs => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                match v {
                    Value::Int(x) => x
                        .checked_abs()
                        .map(Value::Int)
                        .ok_or_else(|| EvalError::new("`abs` overflowed", span)),
                    Value::Float(_) => Ok(Value::float(v.as_f64().expect("a Float").abs())),
                    _ => Err(EvalError::new("`abs` expects an Int or a Float", span)),
                }
            }
            Prim::Sqrt => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                v.as_f64()
                    .map(|f| Value::float(f.sqrt()))
                    .ok_or_else(|| EvalError::new("`sqrt` expects a Float", span))
            }
            Prim::Sin | Prim::Cos | Prim::Trunc => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let f = v.as_f64().ok_or_else(|| {
                    EvalError::new(format!("`{}` expects a Float", op.name()), span)
                })?;
                Ok(match op {
                    // The runtime library rather than the host's libm, and `beck_prim::math` is
                    // why: IEEE 754 pins neither of these, so a fold that reached the platform's
                    // implementation would replay to a different state on a platform with a
                    // different one.
                    Prim::Sin => Value::float(beck_prim::math::sin(f)),
                    Prim::Cos => Value::float(beck_prim::math::cos(f)),
                    // `as` on a float is saturating in Rust and toward zero, which is the rule the
                    // prelude states. A NaN becomes zero, which is the only total answer.
                    _ => Value::Int(f as i64),
                })
            }
            Prim::ToFloat => {
                want(1)?;
                match args.pop().expect("arity checked") {
                    Value::Int(x) => Ok(Value::float(x as f64)),
                    _ => Err(EvalError::new("`float` expects an Int", span)),
                }
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                let ord = a.cmp(&b);
                use std::cmp::Ordering::*;
                Ok(Value::Bool(match op {
                    Prim::Eq => ord == Equal,
                    Prim::Ne => ord != Equal,
                    Prim::Lt => ord == Less,
                    Prim::Le => ord != Greater,
                    Prim::Gt => ord == Greater,
                    _ => ord != Less,
                }))
            }
            Prim::And | Prim::Or => {
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => {
                        Ok(Value::Bool(if op == Prim::And { x && y } else { x || y }))
                    }
                    _ => Err(EvalError::new("expects two Bools", span)),
                }
            }
            Prim::Not => {
                want(1)?;
                args.pop()
                    .expect("arity checked")
                    .as_bool()
                    .map(|b| Value::Bool(!b))
                    .ok_or_else(|| EvalError::new("`not` expects a Bool", span))
            }
            Prim::ToStr => {
                want(1)?;
                Ok(Value::str_(args.pop().expect("arity checked").display()))
            }
            Prim::StrTrim => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                v.as_str()
                    .map(|s| Value::str_(s.trim()))
                    .ok_or_else(|| EvalError::new("`str_trim` expects a Str", span))
            }
            Prim::StrToInt => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let s = v
                    .as_str()
                    .ok_or_else(|| EvalError::new("`str_to_int` expects a Str", span))?;
                Ok(match beck_prim::text::to_int(s) {
                    Some(n) => Value::some(Value::Int(n)),
                    None => Value::none(),
                })
            }
            // ---- strings. Indices are byte offsets into UTF-8 and are clamped: a slice past the
            // end is the empty string. `str_chars` yields *characters*, so the two are different
            // units on purpose and a caller that needs character positions uses the second.
            Prim::StrLen => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                // Constant time: the count was taken when the string was built (`core::Text`).
                // It used to be `chars().count()`, which made `while i < str_len(s)` walk the
                // whole string once per iteration — half of `docs/70` §70.2's quadratic.
                match &v {
                    Value::Str(t) => Ok(Value::Int(t.chars_len() as i64)),
                    _ => {
                        let s = as_str(&v, "str_len", span)?;
                        Ok(Value::Int(s.chars().count() as i64))
                    }
                }
            }
            Prim::StrSlice => {
                want(3)?;
                let len = as_int(&args.pop().expect("arity checked"), "str_slice", span)?;
                let start = as_int(&args.pop().expect("arity checked"), "str_slice", span)?;
                let v = args.pop().expect("arity checked");
                let (start, len) = (start.max(0) as usize, len.max(0) as usize);
                // A character index is a byte index when every character is one byte, and the
                // string knows which it is (`core::Text`). That turns the common case from "skip
                // `start` characters" — `O(start)`, and quadratic in a loop that walks a string —
                // into a range. A non-ASCII string still walks, because there is nothing else it
                // could do without an index nobody has asked to pay for.
                if let Value::Str(t) = &v {
                    // A character index is a byte index when the text is ASCII, and otherwise the
                    // string's own chunked index finds the byte in at most a stride's worth of
                    // steps (`core::Text::byte_offset`). Either way this is `O(len)` in what is
                    // *taken* rather than `O(start)` in what is skipped over — which is what made
                    // walking a string by index quadratic (`docs/70`).
                    let from = t.byte_offset(start);
                    let to = t.byte_offset(start.saturating_add(len));
                    return Ok(Value::str_(&t.as_str()[from..to]));
                }
                let s = as_str(&v, "str_slice", span)?;
                let out: String = s.chars().skip(start).take(len).collect();
                Ok(Value::text(out))
            }
            Prim::StrSplit => {
                want(2)?;
                let sep = args.pop().expect("arity checked");
                let v = args.pop().expect("arity checked");
                let sep = as_str(&sep, "str_split", span)?.to_string();
                let s = as_str(&v, "str_split", span)?;
                // Splitting on the empty string is characters, which is what every caller who
                // writes it means and is the only total answer available.
                let parts: Vec<Value> = if sep.is_empty() {
                    s.chars().map(|c| Value::str_(c.to_string())).collect()
                } else {
                    s.split(sep.as_str()).map(Value::str_).collect()
                };
                Ok(Value::List(Arc::new(parts)))
            }
            Prim::StrJoin => {
                want(2)?;
                let sep = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let sep = as_str(&sep, "str_join", span)?.to_string();
                let xs = xs
                    .as_list()
                    .ok_or_else(|| EvalError::new("`str_join` expects a list", span))?;
                let parts: Vec<String> = xs
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| x.display())
                    })
                    .collect();
                Ok(Value::str_(parts.join(&sep)))
            }
            Prim::StrContains | Prim::StrStartsWith | Prim::StrEndsWith => {
                want(2)?;
                let needle = args.pop().expect("arity checked");
                let hay = args.pop().expect("arity checked");
                let needle = as_str(&needle, op.name(), span)?.to_string();
                let hay = as_str(&hay, op.name(), span)?;
                Ok(Value::Bool(match op {
                    Prim::StrContains => hay.contains(needle.as_str()),
                    Prim::StrStartsWith => hay.starts_with(needle.as_str()),
                    _ => hay.ends_with(needle.as_str()),
                }))
            }
            Prim::StrUpper | Prim::StrLower => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let s = as_str(&v, op.name(), span)?;
                // The runtime library's, so that a compiled program folding the same letter
                // reaches the same table rather than a second one that agrees so far (docs/93 §93.12).
                Ok(Value::str_(&if op == Prim::StrUpper {
                    beck_prim::text::upper(s)
                } else {
                    beck_prim::text::lower(s)
                }))
            }
            Prim::StrReplace => {
                want(3)?;
                let to = args.pop().expect("arity checked");
                let from = args.pop().expect("arity checked");
                let v = args.pop().expect("arity checked");
                let to = as_str(&to, "str_replace", span)?.to_string();
                let from = as_str(&from, "str_replace", span)?.to_string();
                let s = as_str(&v, "str_replace", span)?;
                Ok(Value::str_(beck_prim::text::replace(s, &from, &to)))
            }
            Prim::StrIndexOf => {
                want(2)?;
                let needle = args.pop().expect("arity checked");
                let v = args.pop().expect("arity checked");
                let needle = as_str(&needle, "str_index_of", span)?.to_string();
                let s = as_str(&v, "str_index_of", span)?;
                // In characters, to agree with `str_len` and `str_slice`.
                Ok(match s.find(needle.as_str()) {
                    Some(byte) => Value::some(Value::Int(s[..byte].chars().count() as i64)),
                    None => Value::none(),
                })
            }
            Prim::StrRepeat => {
                want(2)?;
                let n = as_int(&args.pop().expect("arity checked"), "str_repeat", span)?;
                let v = args.pop().expect("arity checked");
                let s = as_str(&v, "str_repeat", span)?;
                // A bound, because `"x" * 10_000_000_000` is a request nobody makes on purpose and
                // an allocation the process does not survive. Fuel is the general answer; this is
                // the specific one, and it is here rather than nowhere.
                let n = n.clamp(0, 1_000_000) as usize;
                Ok(Value::str_(s.repeat(n)))
            }
            Prim::StrChars => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let s = as_str(&v, "str_chars", span)?;
                Ok(Value::List(Arc::new(
                    s.chars().map(|c| Value::str_(c.to_string())).collect(),
                )))
            }
            // ---- collections
            Prim::ListGet => {
                want(2)?;
                let i = as_int(&args.pop().expect("arity checked"), "list_get", span)?;
                let xs = args.pop().expect("arity checked");
                let xs = as_list(&xs, "list_get", span)?;
                Ok(match usize::try_from(i).ok().and_then(|i| xs.get(i)) {
                    Some(v) => Value::some(v.clone()),
                    None => Value::none(),
                })
            }
            Prim::ListSlice | Prim::ListTake | Prim::ListDrop => {
                let (start, len) = if op == Prim::ListSlice {
                    want(3)?;
                    let len = as_int(&args.pop().expect("arity checked"), op.name(), span)?;
                    let start = as_int(&args.pop().expect("arity checked"), op.name(), span)?;
                    (start, len)
                } else {
                    want(2)?;
                    let n = as_int(&args.pop().expect("arity checked"), op.name(), span)?;
                    if op == Prim::ListTake {
                        (0, n)
                    } else {
                        (n, i64::MAX)
                    }
                };
                let xs = args.pop().expect("arity checked");
                let xs = as_list(&xs, op.name(), span)?;
                let out: Vec<Value> = xs
                    .iter()
                    .skip(start.max(0) as usize)
                    .take(len.max(0) as usize)
                    .cloned()
                    .collect();
                Ok(Value::List(Arc::new(out)))
            }
            Prim::ListReverse => {
                want(1)?;
                let xs = args.pop().expect("arity checked");
                let mut out = as_list(&xs, "list_reverse", span)?.to_vec();
                out.reverse();
                Ok(Value::List(Arc::new(out)))
            }
            Prim::ListContains | Prim::ListIndexOf => {
                want(2)?;
                let needle = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = as_list(&xs, op.name(), span)?;
                let found = xs.iter().position(|x| *x == needle);
                Ok(match op {
                    Prim::ListContains => Value::Bool(found.is_some()),
                    _ => match found {
                        Some(i) => Value::some(Value::Int(i as i64)),
                        None => Value::none(),
                    },
                })
            }
            Prim::ListAppend => {
                want(2)?;
                let x = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                // The other half of what `Env::take` starts: a list that arrived from a last read
                // is held by nobody else, so the push costs nothing. One that did not is copied,
                // which is what this always did. `docs/46` §46.14 is the measurement.
                if let Value::List(arc) = xs {
                    let mut out = match Arc::try_unwrap(arc) {
                        Ok(owned) => owned,
                        Err(shared) => {
                            // Ditto: a push costs nothing and a copy costs its length.
                            self.burn_work(shared.len(), span)?;
                            shared.to_vec()
                        }
                    };
                    out.push(x);
                    return Ok(Value::List(Arc::new(out)));
                }
                let mut out = as_list(&xs, "list_append", span)?.to_vec();
                out.push(x);
                Ok(Value::List(Arc::new(out)))
            }
            Prim::ListFold => {
                want(3)?;
                let f = args.pop().expect("arity checked");
                let init = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = as_list(&xs, "list_fold", span)?.to_vec();
                let mut acc = init;
                for x in xs {
                    acc = self.apply(&f, vec![acc, x], span)?;
                }
                Ok(acc)
            }
            Prim::ListAll | Prim::ListAny => {
                want(2)?;
                let f = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = as_list(&xs, op.name(), span)?.to_vec();
                let want_true = op == Prim::ListAny;
                for x in xs {
                    // Short-circuiting, which is observable when the predicate has effects — so it
                    // is a promise rather than an optimisation.
                    if self.apply(&f, vec![x], span)?.as_bool() == Some(want_true) {
                        return Ok(Value::Bool(want_true));
                    }
                }
                Ok(Value::Bool(!want_true))
            }
            Prim::ListFlatMap => {
                want(2)?;
                let f = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = as_list(&xs, "list_flat_map", span)?.to_vec();
                let mut out = Vec::new();
                for x in xs {
                    let ys = self.apply(&f, vec![x], span)?;
                    out.extend(as_list(&ys, "list_flat_map", span)?.iter().cloned());
                }
                Ok(Value::List(Arc::new(out)))
            }
            Prim::ListZip => {
                want(3)?;
                let f = args.pop().expect("arity checked");
                let ys = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let ys = as_list(&ys, "list_zip_with", span)?.to_vec();
                let xs = as_list(&xs, "list_zip_with", span)?.to_vec();
                let mut out = Vec::with_capacity(xs.len().min(ys.len()));
                for (x, y) in xs.into_iter().zip(ys) {
                    out.push(self.apply(&f, vec![x, y], span)?);
                }
                Ok(Value::List(Arc::new(out)))
            }
            Prim::MapKeys => {
                want(1)?;
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_keys` expects a Map", span))?;
                Ok(Value::List(Arc::new(m.keys().cloned().collect())))
            }
            Prim::MapMerge => {
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                let a = a
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_merge` expects a Map", span))?;
                let b = b
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_merge` expects a Map", span))?;
                // The second wins, which is the convention and the only one worth having: a merge
                // is how a default map is overridden by an explicit one.
                let mut out = a.clone();
                for (k, v) in b.iter() {
                    out = out.insert(k.clone(), v.clone());
                }
                Ok(Value::Map(out))
            }
            // ---- JSON. `serde_json` reads and writes the text; the shape in between is Beck's
            // own `Json` union, so a program pattern-matches a document rather than asking a
            // library what kind of node it is holding.
            Prim::JsonParse => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let text = as_str(&v, "json_parse", span)?;
                match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(j) => Ok(json_to_value(&j)),
                    Err(e) => Err(EvalError::raise(
                        Arc::from("JsonError"),
                        Value::data(
                            Arc::from("JsonError"),
                            Some(Arc::from("BadJson")),
                            Fields::from_iter([(Arc::from("why"), Value::str_(e.to_string()))]),
                        ),
                        span,
                    )),
                }
            }
            Prim::JsonRender => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                Ok(Value::str_(value_to_json(&v, span)?.to_string()))
            }
            // ---- time. RFC 3339 in UTC, over the milliseconds `now()` gives.
            Prim::TimeFormat => {
                want(1)?;
                let ms = as_int(&args.pop().expect("arity checked"), "time_format", span)?;
                Ok(Value::str_(beck_prim::time::format(ms)))
            }
            Prim::TimeParse => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let text = as_str(&v, "time_parse", span)?;
                match beck_prim::time::parse(text) {
                    Ok(ms) => Ok(Value::Int(ms)),
                    // The message is the runtime library's, because a compiled program raises this
                    // same value with the same `why` in it and one of them has to be the source.
                    Err(why) => Err(raised(
                        "TimeError",
                        "BadTime",
                        [("why", Value::str_(why))],
                        span,
                    )),
                }
            }
            // ---- digests, encodings and identifiers, all of `beck_core::digest`.
            //
            // In a function of their own rather than inline, and `#[inline(never)]` so they stay
            // there. This match is one arm per primitive and its frame is as large as the widest
            // arm; it is reached from `Interp::eval_prim`, which is on the *recursive* path, so
            // every local a new arm adds is a local every nested call carries — and inlining
            // merges the two frames into one. Adding these inline cost enough depth to break
            // `sicp.rs::what_bounds_a_recursive_types_depth_is_the_evaluator_and_not_the_checker`
            // in a debug build, which is [`adr/0007`](../../../../../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md)'s
            // budget being spent by a primitive that has nothing to do with recursion.
            Prim::Digest
            | Prim::DigestKeyed
            | Prim::DigestEq
            | Prim::HexEncode
            | Prim::HexDecode
            | Prim::Base64Encode
            | Prim::Base64Decode
            | Prim::UuidParse
            | Prim::UuidVersion => digest_prim(op, args, span),
            // ---- the outbound call. The host is the first argument because it is the atom this
            // performs; everything else the program computed is in the request.
            Prim::HttpFetch => {
                want(2)?;
                let request = args.pop().expect("arity checked");
                let host = args.pop().expect("arity checked");
                let host = as_str(&host, "http_fetch", span)?;
                let request = beck_core::host::request_of(host, &request)
                    .map_err(|why| EvalError::new(why, span))?;
                // The one place cancellation does *not* ride the step counter: a child blocked in
                // the socket takes no steps. `docs/80` §80.12 said this belongs on the seam rather
                // than in the scope, so what crosses is the same question `burn` asks — has a
                // child before me failed — as a predicate the client can poll.
                match self.host.fetch(&request, &self.stop()) {
                    Ok(reply) => Ok(beck_core::host::reply_value(&reply)),
                    // A stopped fetch is this scope's own doing, so it becomes the cancellation it
                    // came from rather than an `HttpError` the program would see. Re-asked rather
                    // than assumed: a client is free to answer normally and the scope may have
                    // been cancelled a moment later, and the next `burn` would catch that anyway.
                    Err(beck_core::net::Failure::Stopped) => Err(EvalError::cancelled(span)),
                    Err(f) => Err(EvalError::raise(
                        Arc::from("HttpError"),
                        beck_core::host::failure_value(host, &f),
                        span,
                    )),
                }
            }
            Prim::StrIsEmpty => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                v.as_str()
                    .map(|s| Value::Bool(s.is_empty()))
                    .ok_or_else(|| EvalError::new("`str_is_empty` expects a Str", span))
            }
            Prim::ListLen => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                v.as_list()
                    .map(|xs| Value::Int(xs.len() as i64))
                    .ok_or_else(|| EvalError::new("`list_len` expects a list", span))
            }
            Prim::ListIsEmpty => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                v.as_list()
                    .map(|xs| Value::Bool(xs.is_empty()))
                    .ok_or_else(|| EvalError::new("`list_is_empty` expects a list", span))
            }
            Prim::MapList => {
                want(2)?;
                let f = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = xs
                    .as_list()
                    .ok_or_else(|| EvalError::new("`map_list` expects a list", span))?;
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.apply(&f, vec![x.clone()], span)?);
                }
                Ok(Value::List(Arc::new(out)))
            }
            Prim::FilterList => {
                want(2)?;
                let f = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = xs
                    .as_list()
                    .ok_or_else(|| EvalError::new("`filter_list` expects a list", span))?;
                let mut out = Vec::new();
                for x in xs {
                    if self.apply(&f, vec![x.clone()], span)?.as_bool() == Some(true) {
                        out.push(x.clone());
                    }
                }
                Ok(Value::List(Arc::new(out)))
            }
            Prim::ConcatLists => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                let outer = v.as_list().ok_or_else(|| {
                    EvalError::new("`concat_lists` expects a list of lists", span)
                })?;
                let mut out = Vec::new();
                for inner in outer {
                    match inner.as_list() {
                        Some(xs) => out.extend(xs.iter().cloned()),
                        None => return Err(EvalError::new("`concat_lists` expects lists", span)),
                    }
                }
                Ok(Value::List(Arc::new(out)))
            }
            Prim::SortBy => {
                want(2)?;
                let key = args.pop().expect("arity checked");
                let xs = args.pop().expect("arity checked");
                let xs = xs
                    .as_list()
                    .ok_or_else(|| EvalError::new("`sort_by` expects a list", span))?;
                // Decorate–sort–undecorate, with a *stable* sort. Stability is what makes the
                // order total without a second key: the input order is itself deterministic
                // (a `Map`'s values come out in key order), so ties break the same way on replay.
                let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(xs.len());
                for x in xs {
                    keyed.push((self.apply(&key, vec![x.clone()], span)?, x.clone()));
                }
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(Value::List(Arc::new(
                    keyed.into_iter().map(|(_, v)| v).collect(),
                )))
            }
            Prim::MapGet => {
                want(2)?;
                let k = args.pop().expect("arity checked");
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_get` expects a Map", span))?;
                Ok(match m.get(&k) {
                    Some(v) => Value::some(v.clone()),
                    None => Value::none(),
                })
            }
            // `insert` and `remove` return a *new* map sharing everything they did not touch, so
            // they are `O(log n)` in both time and allocation. Copying here instead — which is
            // what `Arc<BTreeMap>` forced — made every fold `O(events × rows)`.
            Prim::MapInsert => {
                want(3)?;
                let v = args.pop().expect("arity checked");
                let k = args.pop().expect("arity checked");
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_insert` expects a Map", span))?;
                Ok(Value::Map(m.insert(k, v)))
            }
            Prim::MapRemove => {
                want(2)?;
                let k = args.pop().expect("arity checked");
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_remove` expects a Map", span))?;
                Ok(Value::Map(m.remove(&k)))
            }
            Prim::MapValues => {
                want(1)?;
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_values` expects a Map", span))?;
                Ok(Value::List(Arc::new(m.values().cloned().collect())))
            }
            Prim::MapContains => {
                want(2)?;
                let k = args.pop().expect("arity checked");
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_contains` expects a Map", span))?;
                Ok(Value::Bool(m.contains_key(&k)))
            }
            Prim::MapLen => {
                want(1)?;
                let m = args.pop().expect("arity checked");
                let m = m
                    .as_map()
                    .ok_or_else(|| EvalError::new("`map_len` expects a Map", span))?;
                Ok(Value::Int(m.len() as i64))
            }
            Prim::OptionIsSome => {
                want(1)?;
                Ok(Value::Bool(
                    args.pop().expect("arity checked").variant() == Some("Some"),
                ))
            }
            Prim::OptionUnwrapOr => {
                want(2)?;
                let fallback = args.pop().expect("arity checked");
                let opt = args.pop().expect("arity checked");
                Ok(match opt.field("value") {
                    Some(v) if opt.variant() == Some("Some") => v.clone(),
                    _ => fallback,
                })
            }
            Prim::HtmlText => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                match v {
                    // A child that is *already* a tree is spliced, not stringified. `ui:` lowers
                    // every non-element child through here — "a call with positional arguments is
                    // an ordinary function call producing text or Html" (`beck_macro::ui`) — so
                    // rendering `render_comment(r)` as its own markup, escaped, is the one reading
                    // of "or Html" that makes a view uncomposable out of functions.
                    Value::Html(h) => Ok(Value::Html(h)),
                    other => Ok(Value::Html(Arc::new(beck_core::html::text_of(&other)))),
                }
            }
            Prim::HtmlAttr => {
                want(2)?;
                let v = args.pop().expect("arity checked");
                let k = args.pop().expect("arity checked");
                Ok(Value::Attr(Arc::new(beck_core::core::AttrValue::Plain(
                    Arc::from(k.display()),
                    Arc::from(v.display()),
                ))))
            }
            Prim::HtmlOn => {
                want(2)?;
                let cmd = args.pop().expect("arity checked");
                let ev = args.pop().expect("arity checked");
                Ok(Value::Attr(Arc::new(beck_core::core::AttrValue::On(
                    Arc::from(ev.display()),
                    cmd,
                ))))
            }
            Prim::HtmlKey => {
                want(1)?;
                let k = args.pop().expect("arity checked");
                Ok(Value::Attr(Arc::new(beck_core::core::AttrValue::Key(
                    Arc::from(k.display()),
                ))))
            }
            Prim::HtmlEl => {
                want(3)?;
                let children = args.pop().expect("arity checked");
                let attrs = args.pop().expect("arity checked");
                let tag = args.pop().expect("arity checked");
                // `beck_core::html::element` and not a loop here: a compiled `view` answers with
                // these same three arguments and the host builds the tree out of them, so the
                // rules about a dropped attribute, a handler's JSON and a key are written once.
                let el = beck_core::html::element(
                    &tag,
                    attrs.as_list().map(|v| v.as_slice()).unwrap_or(&[]),
                    children.as_list().map(|v| v.as_slice()).unwrap_or(&[]),
                )
                .map_err(|why| EvalError::new(why, span))?;
                Ok(Value::Html(Arc::new(el)))
            }
            Prim::NewUuid => {
                want(0)?;
                Ok(Value::str_(self.host.new_uuid()))
            }
            Prim::Now => {
                want(0)?;
                Ok(Value::Int(self.host.now_millis()))
            }
            // `internal[T]` is a wrapper at runtime for the same reason `secret[T]` is: the wire
            // encoder and the digest have to be able to tell one from the `T` it holds.
            Prim::InternalOf => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                Ok(Value::data(
                    Arc::from(beck_core::Ty::INTERNAL),
                    None,
                    Fields::from_iter([(Arc::from("value"), v)]),
                ))
            }
            Prim::Reveal => {
                want(1)?;
                let v = args.pop().expect("arity checked");
                v.field("value")
                    .cloned()
                    .ok_or_else(|| EvalError::new("`reveal` expects an `internal[T]`", span))
            }
            Prim::SecretEnv => {
                want(1)?;
                let name = args.pop().expect("arity checked");
                let Some(name) = name.as_str() else {
                    return Err(EvalError::new("`secret_env` expects a Str", span));
                };
                // A `secret[Str]` is a newtype at runtime, which is what keeps it distinguishable
                // from the `Str` it wraps everywhere the wire format looks at a value.
                Ok(Value::data(
                    Arc::from(beck_core::Ty::SECRET),
                    None,
                    Fields::from_iter([(Arc::from("value"), Value::str_(self.host.secret(name)))]),
                ))
            }
            // The signal vocabulary is *declarative*: the splitter reads these nodes out of the
            // program and wires the runtime accordingly (`split.rs`). Reaching one here means a
            // signal expression ended up somewhere the splitter did not claim, which is a
            // compiler bug rather than a program error — so it says so.
            Prim::MergeClients
            | Prim::Presence
            | Prim::Freshness
            | Prim::StreamFilterMap
            | Prim::Fold
            | Prim::Durable
            | Prim::SignalMap
            | Prim::SignalMap2
            | Prim::PerSession
            | Prim::Decide => Err(EvalError::new(
                format!(
                    "`{}` is a signal-graph node and is wired by the splitter, not evaluated",
                    op.name()
                ),
                span,
            )),
        }
    }
}

/// Bind a closure's parameters to arguments, giving the environment its body runs in.
///
/// The new frame extends the *closure's* environment, not the caller's, so a tail call replaces
/// the frame it returns into rather than stacking on top of it — the environment chain stays as
/// short at the ten-thousandth iteration as at the first.
fn bind(
    c: &Closure,
    args: impl ExactSizeIterator<Item = Value>,
    span: Span,
) -> Result<Env, EvalError> {
    if c.params.len() != args.len() {
        return Err(EvalError::new(
            format!("expected {} arguments, got {}", c.params.len(), args.len()),
            span,
        ));
    }
    // One allocation for the frame — sized for the parameters *and* for every binding the body
    // will make, so that a `let` writes into a slot rather than allocating a scope (`docs/70`) —
    // and a refcount bump for the parent, which is already behind an `Arc` because a closure holds
    // it that way (`docs/70`).
    Ok(Env::call_frame(&c.env, &c.params, args, c.locals))
}

fn constant(k: &Const) -> Value {
    match k {
        Const::Unit => Value::Unit,
        Const::Bool(b) => Value::Bool(*b),
        Const::Int(i) => Value::Int(*i),
        Const::Float(f) => Value::float(*f),
        Const::Str(s) => Value::str_(s),
    }
}

/// Try a pattern against a value, returning the bindings it makes.
fn match_pattern(p: &Pattern, v: &Value) -> Option<Vec<(u32, Value)>> {
    match p {
        Pattern::Wildcard => Some(Vec::new()),
        Pattern::Bind(id) => Some(vec![(*id, v.clone())]),
        Pattern::Const(k) => {
            let matches = match (k, v) {
                (Const::Unit, Value::Unit) => true,
                (Const::Bool(a), Value::Bool(b)) => a == b,
                (Const::Int(a), Value::Int(b)) => a == b,
                (Const::Str(a), Value::Str(b)) => a.as_ref() == b.as_str(),
                (Const::Float(a), Value::Float(_)) => Some(*a) == v.as_f64(),
                _ => false,
            };
            matches.then(Vec::new)
        }
        // The name binds the whole value, and only if the pattern under it matched.
        Pattern::At { var, inner } => match_pattern(inner, v).map(|mut b| {
            b.push((*var, v.clone()));
            b
        }),
        // First alternative that matches wins, and the checker has made every alternative bind the
        // same variables — so the body reads one set of bindings whichever one it was.
        Pattern::Or(alts) => alts.iter().find_map(|a| match_pattern(a, v)),
        // Recursive since nested patterns arrived, and the whole of what the evaluator had to
        // learn: a field's pattern is matched the same way the scrutinee's was, and a failure
        // anywhere under it fails the arm. Depth is bounded by the checker's own counter, which
        // refuses a pattern that nests past the front end's ceiling before this ever sees it.
        Pattern::Ctor { variant, binds } => {
            if v.variant() != Some(variant.as_ref()) {
                return None;
            }
            let mut out = Vec::with_capacity(binds.len());
            for (field, sub) in binds {
                out.extend(match_pattern(sub, v.field(field)?)?);
            }
            Some(out)
        }
        Pattern::List { items, rest } => {
            let xs = v.as_list()?;
            // No tail binder means an exact length; a tail binder means "at least this many".
            match rest {
                None if xs.len() != items.len() => return None,
                Some(_) if xs.len() < items.len() => return None,
                _ => {}
            }
            let mut out = Vec::with_capacity(items.len() + 1);
            for (sub, x) in items.iter().zip(xs.iter()) {
                out.extend(match_pattern(sub, x)?);
            }
            if let Some(Some(id)) = rest {
                // The tail is a fresh list. `Arc<Vec<_>>` cannot share a suffix, so this is `O(n)`
                // per step and a fold written over it is `O(n²)` — stated in `docs/27` §27.3
                // rather than discovered on a long list.
                out.push((*id, Value::List(Arc::new(xs[items.len()..].to_vec()))));
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_core::{digest, Ty};

    struct NoHost;
    impl Host for NoHost {
        fn global(&self, _: &str) -> Option<&Core> {
            None
        }
    }
    impl beck_core::host::Atoms for NoHost {
        fn new_uuid(&self) -> Arc<str> {
            Arc::from("fixed-uuid")
        }
    }

    fn int(n: i64) -> Core {
        Core::new(CoreKind::Const(Const::Int(n)), Ty::int(), Span::NONE)
    }

    fn prim(op: Prim, args: Vec<Core>) -> Core {
        Core::new(CoreKind::Prim { op, args }, Ty::int(), Span::NONE)
    }

    fn run(c: &Core) -> EvalResult {
        let host = NoHost;
        Interp::new(&host).eval(c, &mut Env::new())
    }

    #[test]
    fn arithmetic_and_comparison() {
        assert_eq!(
            run(&prim(Prim::Add, vec![int(2), int(3)])).unwrap(),
            Value::Int(5)
        );
        assert_eq!(
            run(&prim(Prim::Lt, vec![int(2), int(3)])).unwrap(),
            Value::Bool(true)
        );
    }

    /// Every integer operation without a representable answer, and there is no exception.
    ///
    /// `negate` was one until `docs/93` §93.3: it computed `-x`, which *panics the compiler* on
    /// `i64::MIN` in a debug build and wraps in a release one. That is worse than either answer on
    /// its own — which programs run depended on how the compiler was built — and it is the shape
    /// of defect `docs/64` §64.4 found on the front end's axis. `%` and `/` are here for
    /// `i64::MIN / -1`, whose quotient overflows for the same reason and is not a division by
    /// zero.
    #[test]
    fn overflow_and_division_by_zero_are_errors_not_panics() {
        assert!(run(&prim(Prim::Div, vec![int(1), int(0)])).is_err());
        assert!(run(&prim(Prim::Rem, vec![int(1), int(0)])).is_err());
        assert!(run(&prim(Prim::Add, vec![int(i64::MAX), int(1)])).is_err());
        assert!(run(&prim(Prim::Sub, vec![int(i64::MIN), int(1)])).is_err());
        assert!(run(&prim(Prim::Mul, vec![int(i64::MAX), int(2)])).is_err());
        assert!(run(&prim(Prim::Div, vec![int(i64::MIN), int(-1)])).is_err());
        assert!(run(&prim(Prim::Rem, vec![int(i64::MIN), int(-1)])).is_err());
        assert!(run(&prim(Prim::Abs, vec![int(i64::MIN)])).is_err());
        assert!(run(&prim(Prim::Neg, vec![int(i64::MIN)])).is_err());
        // …and the ordinary case still answers.
        assert_eq!(run(&prim(Prim::Neg, vec![int(7)])).unwrap(), Value::Int(-7));
    }

    /// A slice is charged what it **takes**, and never what the caller asked for.
    ///
    /// "From here to the end" is ordinarily written with a length nobody bounded, so charging the
    /// number in the source made `str_slice(s, 0, 1_000_000)` on a five-character string cost a
    /// million steps and run an otherwise instant program out of fuel. Found by the native
    /// differential, where the compiled answer arrived and this one did not (`docs/93` §93.6).
    ///
    /// Asserted against `work_of` rather than through a whole program, because what went wrong is
    /// the accounting and this is where it is written.
    #[test]
    fn a_slice_is_charged_what_it_takes() {
        let s = Value::str_("héllo");
        let all = |from: i64, n: i64| {
            work_of(
                Prim::StrSlice,
                &[s.clone(), Value::Int(from), Value::Int(n)],
            )
        };
        assert_eq!(all(0, i64::MAX), 5, "the whole string, and not `i64::MAX`");
        assert_eq!(all(0, 1_000_000), 5);
        assert_eq!(all(2, 1_000_000), 3, "clamped from where it starts");
        assert_eq!(all(9, 1_000_000), 0, "past the end takes nothing");
        assert_eq!(all(1, 2), 2, "and a slice inside is still what it takes");
        // The control: a slice of a long string still costs what it takes out of it, so this is
        // not a rule that charges nothing.
        let long = Value::str_("x".repeat(10_000));
        assert_eq!(
            work_of(Prim::StrSlice, &[long, Value::Int(0), Value::Int(i64::MAX)]),
            10_000
        );
    }

    #[test]
    fn sort_by_is_stable_so_ties_replay_identically() {
        let host = NoHost;
        let interp = Interp::new(&host);
        let xs = Value::List(Arc::new(vec![
            Value::data(
                Arc::from("T"),
                None,
                Fields::from_iter([
                    (Arc::from("k"), Value::str_("a")),
                    (Arc::from("n"), Value::Int(1)),
                ]),
            ),
            Value::data(
                Arc::from("T"),
                None,
                Fields::from_iter([
                    (Arc::from("k"), Value::str_("a")),
                    (Arc::from("n"), Value::Int(2)),
                ]),
            ),
        ]));
        let key = Value::Closure(Arc::new(Closure {
            params: vec![0].into(),
            body: Arc::new(Core::new(
                CoreKind::Field {
                    base: Box::new(Core::new(CoreKind::Var(0), Ty::int(), Span::NONE)),
                    name: Arc::from("k"),
                },
                Ty::str_(),
                Span::NONE,
            )),
            env: Env::empty_shared(),
            locals: 0,
        }));
        let out = interp
            .prim(Prim::SortBy, vec![xs.clone(), key.clone()], Span::NONE)
            .unwrap();
        let again = interp
            .prim(Prim::SortBy, vec![xs, key], Span::NONE)
            .unwrap();
        assert_eq!(out, again);
        let ns: Vec<i64> = out
            .as_list()
            .unwrap()
            .iter()
            .map(|v| v.field("n").unwrap().as_int().unwrap())
            .collect();
        assert_eq!(ns, [1, 2], "a stable sort keeps input order for equal keys");
    }

    /// A host that answers one global and records how much stack the evaluator had spent by the
    /// time it reached the bottom of the recursion. `new_uuid` is the probe because it is the one
    /// primitive that calls back out to the host from wherever evaluation happens to be.
    struct Probe {
        f: Core,
        deepest: std::sync::atomic::AtomicUsize,
    }

    impl Host for Probe {
        fn global(&self, name: &str) -> Option<&Core> {
            (name == "f").then_some(&self.f)
        }
    }

    impl beck_core::host::Atoms for Probe {
        fn new_uuid(&self) -> Arc<str> {
            let here = 0u8;
            self.deepest.store(
                std::ptr::addr_of!(here) as usize,
                std::sync::atomic::Ordering::Relaxed,
            );
            Arc::from("bottom")
        }
    }

    fn core(kind: CoreKind) -> Core {
        Core::new(kind, Ty::str_(), Span::NONE)
    }

    /// `def f(n): if n == 0: return uuid(); return f(n - 1) + "x"` — recursion that is *not* in
    /// tail position, so every level costs a host frame.
    fn non_tail_recursion() -> Core {
        let n = || Core::new(CoreKind::Var(0), Ty::int(), Span::NONE);
        core(CoreKind::Lam {
            params: vec![0].into(),
            body: Arc::new(core(CoreKind::If {
                cond: Box::new(Core::new(
                    CoreKind::Prim {
                        op: Prim::Eq,
                        args: vec![n(), int(0)],
                    },
                    Ty::bool_(),
                    Span::NONE,
                )),
                then: Box::new(core(CoreKind::Prim {
                    op: Prim::NewUuid,
                    args: vec![],
                })),
                alt: Box::new(core(CoreKind::Prim {
                    op: Prim::Add,
                    args: vec![
                        core(CoreKind::App {
                            func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                            args: vec![prim(Prim::Sub, vec![n(), int(1)])],
                        }),
                        core(CoreKind::Const(Const::Str(Arc::from("x")))),
                    ],
                })),
            })),
        })
    }

    /// `def g(n): if n == 0: return uuid(); return g(n - 1)` — the same recursion in tail
    /// position, which is the one that must cost nothing.
    fn tail_recursion() -> Core {
        let n = || Core::new(CoreKind::Var(0), Ty::int(), Span::NONE);
        core(CoreKind::Lam {
            params: vec![0].into(),
            body: Arc::new(core(CoreKind::If {
                cond: Box::new(Core::new(
                    CoreKind::Prim {
                        op: Prim::Eq,
                        args: vec![n(), int(0)],
                    },
                    Ty::bool_(),
                    Span::NONE,
                )),
                then: Box::new(core(CoreKind::Prim {
                    op: Prim::NewUuid,
                    args: vec![],
                })),
                alt: Box::new(core(CoreKind::App {
                    func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                    args: vec![prim(Prim::Sub, vec![n(), int(1)])],
                })),
            })),
        })
    }

    /// Call `f(depth)` and report the host bytes the evaluator had spent at the bottom.
    fn stack_spent(f: Core, depth: i64) -> usize {
        let host = Probe {
            f,
            deepest: std::sync::atomic::AtomicUsize::new(0),
        };
        let top = 0u8;
        let top = std::ptr::addr_of!(top) as usize;
        let interp = Interp::new(&host);
        let call = core(CoreKind::App {
            func: Box::new(core(CoreKind::Global(Arc::from("f")))),
            args: vec![int(depth)],
        });
        interp
            .eval(&call, &mut Env::new())
            .expect("the probe recursion evaluates");
        let deepest = host.deepest.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            deepest < top,
            "the host stack is expected to grow downwards"
        );
        top - deepest
    }

    /// The measurement [`crate::STACK_BYTES`] is chosen from, run rather than quoted.
    ///
    /// It answers two questions at once: what one level of *non-tail* recursion costs in host stack
    /// — which is what the declared stack has to cover, at the ceiling — and what one level of
    /// *tail* recursion costs, which must be nothing at all. The second is measured at two depths
    /// forty times apart, because "small" and "constant" are different claims and only the second
    /// one is proper tail calls.
    #[test]
    fn the_depth_ceiling_fits_the_smallest_stack_we_run_on() {
        const DEPTH: i64 = 1_500;
        // The measurement must not itself be the thing that overflows, so it is taken on a stack
        // far larger than the one whose adequacy is being concluded.
        let (non_tail, tail_shallow, tail_deep) = std::thread::Builder::new()
            .stack_size(128 * 1024 * 1024)
            .spawn(|| {
                (
                    stack_spent(non_tail_recursion(), DEPTH),
                    stack_spent(tail_recursion(), DEPTH),
                    stack_spent(tail_recursion(), DEPTH * 40),
                )
            })
            .expect("a thread")
            .join()
            .expect("the probe runs");

        let per_level = non_tail / DEPTH as usize;
        println!(
            "non-tail: {non_tail} bytes for {DEPTH} levels ({per_level} per level)\n\
             tail: {tail_shallow} bytes for {DEPTH} levels, {tail_deep} for {} levels",
            DEPTH * 40
        );

        assert_eq!(
            tail_shallow,
            tail_deep,
            "a tail call must cost *no* host stack: {} tail calls spent {tail_deep} bytes and \
             {DEPTH} spent {tail_shallow}, so the cost is not constant",
            DEPTH * 40
        );
        assert!(
            tail_deep < per_level * 40,
            "and the constant it costs must be a handful of frames, not a hidden budget: \
             {tail_deep} bytes against {per_level} for one non-tail level"
        );

        // Twice over, so that whoever drives the evaluator has as much stack again above the
        // ceiling as the ceiling itself needs.
        let needed = DEFAULT_MAX_DEPTH as usize * per_level * 2;
        assert!(
            needed < crate::STACK_BYTES,
            "a ceiling of {DEFAULT_MAX_DEPTH} levels at {per_level} bytes each needs {needed} \
             bytes with the margin, against a declared STACK_BYTES of {} — raise the declaration \
             or lower the ceiling",
            crate::STACK_BYTES
        );
    }

    /// Both of the next two drive the evaluator to its ceiling, so both run on the stack the
    /// evaluator declares it needs — which is the contract [`crate::on_the_evaluator_stack`]
    /// exists to make checkable rather than assumed.
    #[test]
    fn a_tail_call_is_bounded_by_fuel_and_not_by_depth() {
        crate::on_the_evaluator_stack(|| {
            let host = Probe {
                f: tail_recursion(),
                deepest: std::sync::atomic::AtomicUsize::new(0),
            };
            let interp = Interp::new(&host);
            let call = core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(DEFAULT_MAX_DEPTH as i64 * 10)],
            });
            assert_eq!(
                interp.eval(&call, &mut Env::new()).unwrap(),
                Value::str_("bottom"),
                "ten times the depth ceiling, in tail position, is not deep at all"
            );
        })
    }

    /// The ceiling can be lowered but not raised, because the stack it is safe against is
    /// [`crate::STACK_BYTES`] and a caller who wanted more of it would have to supply more of that.
    #[test]
    fn the_depth_ceiling_can_be_lowered_and_not_raised() {
        let host = Probe {
            f: non_tail_recursion(),
            deepest: std::sync::atomic::AtomicUsize::new(0),
        };
        let call = |n: i64| {
            core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(n)],
            })
        };

        let shallow = Interp::new(&host).with_max_depth(100);
        assert!(
            shallow.eval(&call(200), &mut Env::new()).is_err(),
            "a lowered ceiling is the one that applies"
        );
        assert!(
            shallow.eval(&call(20), &mut Env::new()).is_ok(),
            "and it applies only past itself"
        );

        crate::on_the_evaluator_stack(|| {
            let host = Probe {
                f: non_tail_recursion(),
                deepest: std::sync::atomic::AtomicUsize::new(0),
            };
            let greedy = Interp::new(&host).with_max_depth(u32::MAX);
            let deep = core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(DEFAULT_MAX_DEPTH as i64 * 4)],
            });
            assert!(
                greedy.eval(&deep, &mut Env::new()).is_err(),
                "asking for more than the declared stack can hold does not get it"
            );
        });
    }

    #[test]
    fn non_tail_recursion_is_a_diagnostic_rather_than_an_abort() {
        crate::on_the_evaluator_stack(|| {
            let host = Probe {
                f: non_tail_recursion(),
                deepest: std::sync::atomic::AtomicUsize::new(0),
            };
            let interp = Interp::new(&host);
            let call = core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(DEFAULT_MAX_DEPTH as i64 * 10)],
            });
            let err = interp
                .eval(&call, &mut Env::new())
                .expect_err("past the ceiling");
            assert!(
                err.message.contains("which is the evaluator's limit"),
                "{}",
                err.message
            );
        })
    }

    #[test]
    fn fuel_bounds_a_runaway_program() {
        let host = NoHost;
        let interp = Interp::with_fuel(&host, 3);
        let deep = prim(
            Prim::Add,
            vec![prim(Prim::Add, vec![int(1), int(1)]), int(1)],
        );
        assert!(interp.eval(&deep, &mut Env::new()).is_err());
    }

    #[test]
    fn the_digest_is_a_function_of_structure() {
        let a = Value::List(Arc::new(vec![Value::Int(1), Value::str_("x")]));
        let b = Value::List(Arc::new(vec![Value::Int(1), Value::str_("x")]));
        let c = Value::List(Arc::new(vec![Value::Int(1), Value::str_("y")]));
        assert_eq!(digest(&a), digest(&b));
        assert_ne!(digest(&a), digest(&c));
    }

    #[test]
    fn signal_primitives_refuse_to_be_evaluated() {
        let err = run(&prim(Prim::Durable, vec![int(1)])).unwrap_err();
        assert!(err.message.contains("signal-graph node"));
    }
}
