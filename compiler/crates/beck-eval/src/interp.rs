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
//!
//! To which a fourth was added by [`docs/31-tail-calls-report.md`](../../../../docs/31-tail-calls-report.md):
//!
//! * **A call in tail position does not grow the host stack, and no program aborts the process.**
//!   [`Interp::eval`] is a trampoline: `Interp::step` walks the tail positions of a body without
//!   recursing and hands the call it lands on back to the loop, so an iterative process is
//!   iterative (SICP §1.2.1, R7RS §3.5). Recursion that is *not* in tail position still spends a
//!   host frame, so it is bounded by a counted depth — deterministically, because a limit read off
//!   the host's remaining stack would make a fold's outcome depend on the build profile and §3.7
//!   needs it to depend only on the log.

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
    /// The value a `raise` failed with, if this is a raise rather than a fault.
    ///
    /// The two travel the same way — a raise unwinds — and are distinguished here rather than in a
    /// second error type, because everything between the raise and its handler has to pass both
    /// along unchanged and neither of them is the ordinary path. `Prim::Try` is the only reader.
    pub raised: Option<Box<(Arc<str>, Value)>>,
}

impl EvalError {
    pub fn new(message: impl Into<String>, span: Span) -> EvalError {
        EvalError {
            message: message.into(),
            span,
            raised: None,
        }
    }

    /// A failure a program *chose*, carrying the value it chose to fail with and that value's type.
    pub fn raise(ty: Arc<str>, value: Value, span: Span) -> EvalError {
        EvalError {
            message: format!("raised `{}`", value.display()),
            span,
            raised: Some(Box::new((ty, value))),
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
    ///
    /// Defaulted to the host's clock through the seam rather than to `SystemTime::now()` directly
    /// (`beck_core::clock`), so a host that wants a stated instant overrides one method and a host
    /// that does not still never names the standard library's clock.
    fn now_millis(&self) -> i64 {
        beck_core::clock::process_clock().now_millis()
    }
    /// Read a secret from the process environment — `env`, which no client tier discharges.
    fn secret(&self, name: &str) -> Arc<str> {
        std::env::var(name).unwrap_or_default().into()
    }

    /// Answer a call to a top-level definition without running its body.
    ///
    /// The evaluator's half of [`beck_core::backend::Interceptor`]. Defaulted to "run the real
    /// thing", so the only host that behaves differently is the one a `beck test` run installs.
    fn intercept(&self, _name: &str, _args: &[Value]) -> Option<Value> {
        None
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
}

const DEFAULT_FUEL: u64 = 50_000_000;

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
        }
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

    fn burn(&self, span: Span) -> Result<(), EvalError> {
        let left = self.fuel.get();
        if left == 0 {
            return Err(EvalError::new("evaluation ran out of fuel", span));
        }
        self.fuel.set(left - 1);
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

    /// Apply a callable value to arguments — the entry point the runtime uses for `validate`,
    /// `apply_event` and `view`.
    pub fn apply(&self, f: &Value, args: Vec<Value>, span: Span) -> EvalResult {
        match f {
            Value::Closure(c) => {
                let env = bind(c, args, span)?;
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

    /// The trampoline.
    ///
    /// One host frame is taken here, and a call in tail position replaces the loop's state rather
    /// than nesting inside it — so `fact_iter`, `gcd` and `find_divisor` run in constant space and
    /// SICP §1.2.1's distinction between a recursive and an iterative *process* is observable
    /// (`docs/31` §31.2).
    pub fn eval(&self, c: &Core, env: &Env) -> EvalResult {
        let _frame = self.enter(c.span)?;
        let mut step = self.step(c, env)?;
        loop {
            let (callee, args, span) = match step {
                Step::Done(v) => return Ok(v),
                Step::Tail { callee, args, span } => (callee, args, span),
            };
            let env = bind(&callee, args, span)?;
            step = self.step(&callee.body, &env)?;
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
    /// program is mostly these nodes. `docs/31` §31.5 has what the trampoline cost in the end.
    #[inline]
    fn operand(&self, c: &Core, env: &Env) -> EvalResult {
        match &c.kind {
            CoreKind::Const(k) => {
                self.burn(c.span)?;
                Ok(constant(k))
            }
            CoreKind::Var(v) => {
                self.burn(c.span)?;
                env.get(*v).cloned().ok_or_else(|| {
                    EvalError::new(format!("unbound variable {v} at runtime"), c.span)
                })
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
    fn step<'a>(&'a self, c: &'a Core, env0: &Env) -> Result<Step, EvalError> {
        let mut cur: &'a Core = c;
        // The environment is only *replaced* by a `let` or a matched arm, and most nodes replace
        // nothing. Holding the caller's by reference until something extends it keeps two atomic
        // refcount operations off every node that does not — which is most of them.
        let mut owned: Option<Env> = None;
        loop {
            let env: &Env = owned.as_ref().unwrap_or(env0);
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
                    let next = env.extend(vec![(*var, v)]);
                    owned = Some(next);
                    cur = body;
                }
                CoreKind::Match { scrutinee, arms } => {
                    let v = self.operand(scrutinee, env)?;
                    let hit = arms
                        .iter()
                        .find_map(|arm| match_pattern(&arm.pattern, &v).map(|b| (b, &arm.body)));
                    let Some((bindings, body)) = hit else {
                        return Err(EvalError::new(
                            format!("no match arm applies to {}", v.display()),
                            cur.span,
                        ));
                    };
                    let next = env.extend(bindings);
                    owned = Some(next);
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
                    if let CoreKind::Global(name) = &func.kind {
                        if let Some(v) = self.host.intercept(name, &vals) {
                            return Ok(Step::Done(v));
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
                // makes, which is most of what the trampoline would otherwise have cost (§31.5).
                CoreKind::Global(name) => {
                    let Some(body) = self.host.global(name) else {
                        return Err(EvalError::new(
                            format!("no such definition: {name}"),
                            cur.span,
                        ));
                    };
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
    /// level of a program's recursion the sum of the arms it did not take (`docs/31` §31.4).
    fn leaf(&self, c: &Core, env: &Env) -> EvalResult {
        match &c.kind {
            CoreKind::Const(k) => Ok(constant(k)),
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
            CoreKind::Prim { op, args } => self.eval_prim(*op, args, env, c.span),
            CoreKind::Make {
                ty,
                variant,
                fields,
            } => self.eval_make(ty, variant.as_ref(), fields, env),
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
    fn eval_prim(&self, op: Prim, args: &[Core], env: &Env, span: Span) -> EvalResult {
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
        env: &Env,
    ) -> EvalResult {
        let mut map = BTreeMap::new();
        for (name, expr) in fields {
            map.insert(name.clone(), self.operand(expr, env)?);
        }
        Ok(Value::Data {
            ty: ty.clone(),
            variant: variant.cloned(),
            fields: Arc::new(map),
        })
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_field(&self, base: &Core, name: &Arc<str>, env: &Env, span: Span) -> EvalResult {
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
        env: &Env,
        span: Span,
    ) -> EvalResult {
        let v = self.operand(base, env)?;
        let Value::Data {
            ty,
            variant,
            fields: old,
        } = v
        else {
            return Err(EvalError::new("`with` expects a record", span));
        };
        let mut map = (*old).clone();
        for (name, expr) in fields {
            map.insert(name.clone(), self.operand(expr, env)?);
        }
        Ok(Value::Data {
            ty,
            variant,
            fields: Arc::new(map),
        })
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_list(&self, items: &[Core], env: &Env) -> EvalResult {
        let mut out = Vec::with_capacity(items.len());
        for i in items {
            out.push(self.operand(i, env)?);
        }
        Ok(Value::List(Arc::new(out)))
    }

    #[cfg_attr(debug_assertions, inline(never))]
    fn eval_map(&self, kvs: &[(Core, Core)], env: &Env) -> EvalResult {
        let mut out = PMap::new();
        for (k, v) in kvs {
            out = out.insert(self.operand(k, env)?, self.operand(v, env)?);
        }
        Ok(Value::Map(out))
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

        // An `Int` operation is checked and a `Float` one is not, and that asymmetry is the whole
        // difference between the two tiers of the tower: `i64` overflow has no representable
        // answer, so it is an error, while IEEE 754 defines one for every real operation —
        // including division by zero, whose answer is an infinity. Making `1.0 / 0.0` an error
        // would be inventing a rule the format already has (`docs/32` §32.3).
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
                    Value::Data { ty, .. } => ty.clone(),
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
            Prim::Add => {
                want(2)?;
                let b = args.pop().expect("arity checked");
                let a = args.pop().expect("arity checked");
                match (&a, &b) {
                    (Value::Int(x), Value::Int(y)) => x
                        .checked_add(*y)
                        .map(Value::Int)
                        .ok_or_else(|| EvalError::new("`+` overflowed", span)),
                    (Value::Float(_), Value::Float(_)) => Ok(Value::float(
                        a.as_f64().expect("a Float") + b.as_f64().expect("a Float"),
                    )),
                    // `+` on strings concatenates, which is what the sketch's footer wants.
                    (Value::Str(x), Value::Str(y)) => Ok(Value::str_(format!("{x}{y}"))),
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
                    Value::Int(x) => Ok(Value::Int(-x)),
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
                Ok(match s.parse::<i64>() {
                    Ok(n) => Value::some(Value::Int(n)),
                    Err(_) => Value::none(),
                })
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

/// Bind a closure's parameters to arguments, giving the environment its body runs in.
///
/// The new frame extends the *closure's* environment, not the caller's, so a tail call replaces
/// the frame it returns into rather than stacking on top of it — the environment chain stays as
/// short at the ten-thousandth iteration as at the first.
fn bind(c: &Closure, args: Vec<Value>, span: Span) -> Result<Env, EvalError> {
    if c.params.len() != args.len() {
        return Err(EvalError::new(
            format!("expected {} arguments, got {}", c.params.len(), args.len()),
            span,
        ));
    }
    Ok(c.env.extend(c.params.iter().copied().zip(args).collect()))
}

fn constant(k: &Const) -> Value {
    match k {
        Const::Unit => Value::Unit,
        Const::Bool(b) => Value::Bool(*b),
        Const::Int(i) => Value::Int(*i),
        Const::Float(f) => Value::float(*f),
        Const::Str(s) => Value::Str(s.clone()),
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
        Pattern::List { binds, rest } => {
            let xs = v.as_list()?;
            // No tail binder means an exact length; a tail binder means "at least this many".
            match rest {
                None if xs.len() != binds.len() => return None,
                Some(_) if xs.len() < binds.len() => return None,
                _ => {}
            }
            let mut out = Vec::with_capacity(binds.len() + 1);
            for (b, x) in binds.iter().zip(xs.iter()) {
                if let Some(id) = b {
                    out.push((*id, x.clone()));
                }
            }
            if let Some(Some(id)) = rest {
                // The tail is a fresh list. `Arc<Vec<_>>` cannot share a suffix, so this is `O(n)`
                // per step and a fold written over it is `O(n²)` — stated in `docs/33` §33.6
                // rather than discovered on a long list.
                out.push((*id, Value::List(Arc::new(xs[binds.len()..].to_vec()))));
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

    /// A host that answers one global and records how much stack the evaluator had spent by the
    /// time it reached the bottom of the recursion. `new_uuid` is the probe because it is the one
    /// primitive that calls back out to the host from wherever evaluation happens to be.
    struct Probe {
        f: Core,
        deepest: std::cell::Cell<usize>,
    }

    impl Host for Probe {
        fn global(&self, name: &str) -> Option<&Core> {
            (name == "f").then_some(&self.f)
        }
        fn new_uuid(&self) -> Arc<str> {
            let here = 0u8;
            self.deepest.set(std::ptr::addr_of!(here) as usize);
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
            params: vec![0],
            body: Box::new(core(CoreKind::If {
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
            params: vec![0],
            body: Box::new(core(CoreKind::If {
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
            deepest: std::cell::Cell::new(0),
        };
        let top = 0u8;
        let top = std::ptr::addr_of!(top) as usize;
        let interp = Interp::new(&host);
        let call = core(CoreKind::App {
            func: Box::new(core(CoreKind::Global(Arc::from("f")))),
            args: vec![int(depth)],
        });
        interp
            .eval(&call, &Env::new())
            .expect("the probe recursion evaluates");
        let deepest = host.deepest.get();
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
                deepest: std::cell::Cell::new(0),
            };
            let interp = Interp::new(&host);
            let call = core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(DEFAULT_MAX_DEPTH as i64 * 10)],
            });
            assert_eq!(
                interp.eval(&call, &Env::new()).unwrap(),
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
            deepest: std::cell::Cell::new(0),
        };
        let call = |n: i64| {
            core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(n)],
            })
        };

        let shallow = Interp::new(&host).with_max_depth(100);
        assert!(
            shallow.eval(&call(200), &Env::new()).is_err(),
            "a lowered ceiling is the one that applies"
        );
        assert!(
            shallow.eval(&call(20), &Env::new()).is_ok(),
            "and it applies only past itself"
        );

        crate::on_the_evaluator_stack(|| {
            let host = Probe {
                f: non_tail_recursion(),
                deepest: std::cell::Cell::new(0),
            };
            let greedy = Interp::new(&host).with_max_depth(u32::MAX);
            let deep = core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(DEFAULT_MAX_DEPTH as i64 * 4)],
            });
            assert!(
                greedy.eval(&deep, &Env::new()).is_err(),
                "asking for more than the declared stack can hold does not get it"
            );
        });
    }

    #[test]
    fn non_tail_recursion_is_a_diagnostic_rather_than_an_abort() {
        crate::on_the_evaluator_stack(|| {
            let host = Probe {
                f: non_tail_recursion(),
                deepest: std::cell::Cell::new(0),
            };
            let interp = Interp::new(&host);
            let call = core(CoreKind::App {
                func: Box::new(core(CoreKind::Global(Arc::from("f")))),
                args: vec![int(DEFAULT_MAX_DEPTH as i64 * 10)],
            });
            let err = interp
                .eval(&call, &Env::new())
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
