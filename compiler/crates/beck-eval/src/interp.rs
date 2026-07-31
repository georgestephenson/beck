//! The tree-walker itself. [`crate`] wraps it as a [`beck_core::backend::Backend`].
//!
//! Deliberately bad at everything, complete end-to-end
//! ([`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) Phase 1). It is a tree-walker over
//! typed `Core`, with three properties that are not negotiable and are tested:
//!
//! * **Replay purity.** Nothing here reads a clock, a random source, or performs I/O. `uuid()` is
//!   a primitive the *checker* refuses inside a fold (§3.7), and even outside one it is supplied
//!   by the host rather than taken from the ambient environment, so a replay is reproducible.
//! * **Total order everywhere.** Maps are `BTreeMap`s and `sort_by` is stable, so two runs over
//!   the same log render identically — Phase 0 §18.5 item 4 learned this the hard way.
//! * **Errors are values, not panics.** A partial operation returns an [`EvalError`] carrying the
//!   span, because a language server has to survive evaluating half-written code.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_diag::Span;

use beck_core::core::{Closure, Const, Core, CoreKind, Env, Pattern, Prim, Value};
use beck_core::html::Html;
use beck_core::PMap;

#[derive(Clone, Debug)]
pub struct EvalError {
    pub message: String,
    pub span: Span,
}

impl EvalError {
    pub fn new(message: impl Into<String>, span: Span) -> EvalError {
        EvalError {
            message: message.into(),
            span,
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

/// What the evaluator needs from outside itself: the top-level definitions, and the impure
/// capabilities that the host supplies rather than the program taking.
///
/// Each of these is exactly one effect atom made concrete — `nondet` for the first two, `env` for
/// the third — so a host that refuses one is refusing an effect, not disabling a feature.
pub trait Host {
    fn global(&self, name: &str) -> Option<&Core>;
    /// Mint an id. Called only where the checker has proved we are not inside a fold.
    fn new_uuid(&self) -> Arc<str>;
    /// Read the wall clock. `nondet`, and forbidden inside a fold for the same reason `uuid` is:
    /// time is data on the envelope.
    fn now_millis(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
    /// Read a secret from the process environment — `env`, which no client tier discharges.
    fn secret(&self, name: &str) -> Arc<str> {
        std::env::var(name).unwrap_or_default().into()
    }
}

pub struct Interp<'h> {
    pub host: &'h dyn Host,
    /// Bounded so that a non-terminating program in a request handler cannot wedge the server.
    fuel: std::cell::Cell<u64>,
}

const DEFAULT_FUEL: u64 = 50_000_000;

impl<'h> Interp<'h> {
    pub fn new(host: &'h dyn Host) -> Interp<'h> {
        Interp {
            host,
            fuel: std::cell::Cell::new(DEFAULT_FUEL),
        }
    }

    pub fn with_fuel(host: &'h dyn Host, fuel: u64) -> Interp<'h> {
        Interp {
            host,
            fuel: std::cell::Cell::new(fuel),
        }
    }

    pub fn reset_fuel(&self) {
        self.fuel.set(DEFAULT_FUEL);
    }

    fn burn(&self, span: Span) -> Result<(), EvalError> {
        let left = self.fuel.get();
        if left == 0 {
            return Err(EvalError::new("evaluation ran out of fuel", span));
        }
        self.fuel.set(left - 1);
        Ok(())
    }

    /// Apply a callable value to arguments — the entry point the runtime uses for `validate`,
    /// `apply_event` and `view`.
    pub fn apply(&self, f: &Value, args: Vec<Value>, span: Span) -> EvalResult {
        match f {
            Value::Closure(c) => {
                if c.params.len() != args.len() {
                    return Err(EvalError::new(
                        format!("expected {} arguments, got {}", c.params.len(), args.len()),
                        span,
                    ));
                }
                let env = c.env.extend(c.params.iter().copied().zip(args).collect());
                self.eval(&c.body, &env)
            }
            other => Err(EvalError::new(
                format!("not callable: {}", other.display()),
                span,
            )),
        }
    }

    /// Evaluate a top-level definition by name.
    pub fn global(&self, name: &str, span: Span) -> EvalResult {
        match self.host.global(name) {
            Some(core) => self.eval(core, &Env::new()),
            None => Err(EvalError::new(format!("no such definition: {name}"), span)),
        }
    }

    pub fn eval(&self, c: &Core, env: &Env) -> EvalResult {
        self.burn(c.span)?;
        match &c.kind {
            CoreKind::Const(k) => Ok(match k {
                Const::Unit => Value::Unit,
                Const::Bool(b) => Value::Bool(*b),
                Const::Int(i) => Value::Int(*i),
                Const::Float(f) => Value::float(*f),
                Const::Str(s) => Value::Str(s.clone()),
            }),
            CoreKind::Var(v) => env
                .get(*v)
                .cloned()
                .ok_or_else(|| EvalError::new(format!("unbound variable {v} at runtime"), c.span)),
            CoreKind::Global(name) => self.global(name, c.span),
            CoreKind::Lam { params, body } => Ok(Value::Closure(Arc::new(Closure {
                params: params.clone(),
                body: (**body).clone(),
                env: env.clone(),
            }))),
            CoreKind::App { func, args } => {
                let f = self.eval(func, env)?;
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a, env)?);
                }
                self.apply(&f, vals, c.span)
            }
            CoreKind::Prim { op, args } => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a, env)?);
                }
                self.prim(*op, vals, c.span)
            }
            CoreKind::Let { var, value, body } => {
                let v = self.eval(value, env)?;
                let inner = env.extend(vec![(*var, v)]);
                self.eval(body, &inner)
            }
            CoreKind::If { cond, then, alt } => {
                let c0 = self.eval(cond, env)?;
                match c0.as_bool() {
                    Some(true) => self.eval(then, env),
                    Some(false) => self.eval(alt, env),
                    None => Err(EvalError::new("condition is not a Bool", cond.span)),
                }
            }
            CoreKind::Match { scrutinee, arms } => {
                let v = self.eval(scrutinee, env)?;
                for arm in arms {
                    if let Some(bindings) = match_pattern(&arm.pattern, &v) {
                        let inner = env.extend(bindings);
                        return self.eval(&arm.body, &inner);
                    }
                }
                Err(EvalError::new(
                    format!("no match arm applies to {}", v.display()),
                    c.span,
                ))
            }
            CoreKind::Make {
                ty,
                variant,
                fields,
            } => {
                let mut map = BTreeMap::new();
                for (name, expr) in fields {
                    map.insert(name.clone(), self.eval(expr, env)?);
                }
                Ok(Value::Data {
                    ty: ty.clone(),
                    variant: variant.clone(),
                    fields: Arc::new(map),
                })
            }
            CoreKind::Field { base, name } => {
                let v = self.eval(base, env)?;
                v.field(name).cloned().ok_or_else(|| {
                    EvalError::new(format!("no field `{name}` on {}", v.display()), c.span)
                })
            }
            CoreKind::With { base, fields } => {
                let v = self.eval(base, env)?;
                let Value::Data {
                    ty,
                    variant,
                    fields: old,
                } = v
                else {
                    return Err(EvalError::new("`with` expects a record", c.span));
                };
                let mut map = (*old).clone();
                for (name, expr) in fields {
                    map.insert(name.clone(), self.eval(expr, env)?);
                }
                Ok(Value::Data {
                    ty,
                    variant,
                    fields: Arc::new(map),
                })
            }
            CoreKind::ListLit(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(self.eval(i, env)?);
                }
                Ok(Value::List(Arc::new(out)))
            }
            CoreKind::MapLit(kvs) => {
                let mut out = PMap::new();
                for (k, v) in kvs {
                    out = out.insert(self.eval(k, env)?, self.eval(v, env)?);
                }
                Ok(Value::Map(out))
            }
        }
    }

    fn prim(&self, op: Prim, mut args: Vec<Value>, span: Span) -> EvalResult {
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

        macro_rules! arith {
            ($f:expr) => {{
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                match (&a, &b) {
                    (Value::Int(x), Value::Int(y)) => {
                        let f: fn(i64, i64) -> Option<i64> = $f;
                        f(*x, *y).map(Value::Int).ok_or_else(|| {
                            EvalError::new(
                                format!("`{}` overflowed or divided by zero", op.name()),
                                span,
                            )
                        })
                    }
                    _ => Err(EvalError::new(
                        format!("`{}` expects two Ints", op.name()),
                        span,
                    )),
                }
            }};
        }

        match op {
            Prim::Add => {
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                match (&a, &b) {
                    (Value::Int(x), Value::Int(y)) => x
                        .checked_add(*y)
                        .map(Value::Int)
                        .ok_or_else(|| EvalError::new("`+` overflowed", span)),
                    // `+` on strings concatenates, which is what the sketch's footer wants.
                    (Value::Str(x), Value::Str(y)) => Ok(Value::str_(format!("{x}{y}"))),
                    _ => Err(EvalError::new("`+` expects two Ints or two Strs", span)),
                }
            }
            Prim::Sub => arith!(i64::checked_sub),
            Prim::Mul => arith!(i64::checked_mul),
            Prim::Div => arith!(i64::checked_div),
            Prim::Rem => arith!(i64::checked_rem),
            Prim::Neg => {
                want(1)?;
                match args.pop().expect("arity checked") {
                    Value::Int(x) => Ok(Value::Int(-x)),
                    _ => Err(EvalError::new("`-` expects an Int", span)),
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
                Ok(Value::Html(Arc::new(Html::text(v.display()))))
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
                let mut el = Html::el(tag.display());
                for a in attrs.as_list().map(|v| v.as_slice()).unwrap_or(&[]) {
                    match a {
                        Value::Attr(at) => match &**at {
                            beck_core::core::AttrValue::Plain(k, v) => {
                                // An empty attribute value is dropped rather than emitted, so the
                                // differ has nothing to churn on — Phase 0's `attr_if`.
                                if !v.is_empty() {
                                    el = el.attr(k.to_string(), v.to_string());
                                }
                            }
                            beck_core::core::AttrValue::On(ev, cmd) => {
                                el = el.on(ev, cmd.to_json());
                            }
                            beck_core::core::AttrValue::Key(k) => el = el.key(k.to_string()),
                        },
                        other => {
                            return Err(EvalError::new(
                                format!("not an attribute: {}", other.display()),
                                span,
                            ))
                        }
                    }
                }
                for ch in children.as_list().map(|v| v.as_slice()).unwrap_or(&[]) {
                    match ch.as_html() {
                        Some(h) => el = el.child(h.clone()),
                        None => {
                            return Err(EvalError::new(
                                format!("not an Html child: {}", ch.display()),
                                span,
                            ))
                        }
                    }
                }
                Ok(Value::Html(Arc::new(el)))
            }
            Prim::NewUuid => {
                want(0)?;
                Ok(Value::Str(self.host.new_uuid()))
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
                Ok(Value::Data {
                    ty: Arc::from(beck_core::Ty::INTERNAL),
                    variant: None,
                    fields: Arc::new(std::collections::BTreeMap::from([(Arc::from("value"), v)])),
                })
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
                Ok(Value::Data {
                    ty: Arc::from(beck_core::Ty::SECRET),
                    variant: None,
                    fields: Arc::new(std::collections::BTreeMap::from([(
                        Arc::from("value"),
                        Value::Str(self.host.secret(name)),
                    )])),
                })
            }
            // The signal vocabulary is *declarative*: the splitter reads these nodes out of the
            // program and wires the runtime accordingly (`split.rs`). Reaching one here means a
            // signal expression ended up somewhere the splitter did not claim, which is a
            // compiler bug rather than a program error — so it says so.
            Prim::MergeClients
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
                (Const::Str(a), Value::Str(b)) => a == b,
                (Const::Float(a), Value::Float(_)) => Some(*a) == v.as_f64(),
                _ => false,
            };
            matches.then(Vec::new)
        }
        Pattern::Ctor { variant, binds } => {
            if v.variant() != Some(variant.as_ref()) {
                return None;
            }
            let mut out = Vec::with_capacity(binds.len());
            for (field, id) in binds {
                out.push((*id, v.field(field)?.clone()));
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
        Interp::new(&host).eval(c, &Env::new())
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

    #[test]
    fn overflow_and_division_by_zero_are_errors_not_panics() {
        assert!(run(&prim(Prim::Div, vec![int(1), int(0)])).is_err());
        assert!(run(&prim(Prim::Add, vec![int(i64::MAX), int(1)])).is_err());
    }

    #[test]
    fn sort_by_is_stable_so_ties_replay_identically() {
        let host = NoHost;
        let interp = Interp::new(&host);
        let xs = Value::List(Arc::new(vec![
            Value::Data {
                ty: Arc::from("T"),
                variant: None,
                fields: Arc::new(BTreeMap::from([
                    (Arc::from("k"), Value::str_("a")),
                    (Arc::from("n"), Value::Int(1)),
                ])),
            },
            Value::Data {
                ty: Arc::from("T"),
                variant: None,
                fields: Arc::new(BTreeMap::from([
                    (Arc::from("k"), Value::str_("a")),
                    (Arc::from("n"), Value::Int(2)),
                ])),
            },
        ]));
        let key = Value::Closure(Arc::new(Closure {
            params: vec![0],
            body: Core::new(
                CoreKind::Field {
                    base: Box::new(Core::new(CoreKind::Var(0), Ty::int(), Span::NONE)),
                    name: Arc::from("k"),
                },
                Ty::str_(),
                Span::NONE,
            ),
            env: Env::new(),
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

    #[test]
    fn fuel_bounds_a_runaway_program() {
        let host = NoHost;
        let interp = Interp::with_fuel(&host, 3);
        let deep = prim(
            Prim::Add,
            vec![prim(Prim::Add, vec![int(1), int(1)]), int(1)],
        );
        assert!(interp.eval(&deep, &Env::new()).is_err());
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
