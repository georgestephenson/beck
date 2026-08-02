//! The incremental view engine: the thing that flows deltas through a [`crate::plan::Plan`].
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.8:
//! "`remaining` updates by ±1 per event, never by recount." Until now that sentence described an
//! intention. This is the machine that makes it true, and
//! [`docs/24-incremental-views-report.md`](../../../../docs/24-incremental-views-report.md) is the
//! measurement.
//!
//! # The one hard problem, and where it is solved
//!
//! A Beck program's state is a *value*: `todos = durable(fold(apply_event, empty, events))` produces
//! a whole new accumulator per event. A dataflow plan consumes *changes*. Something has to convert
//! one into the other, and doing it by comparing the old and new accumulator entry by entry would
//! be `O(n)` per event — which is the recount §3.8 exists to abolish, moved one level down where it
//! is harder to see.
//!
//! [`crate::pmap::PMap::diff`] is the conversion, and it is `O(δ log n)` because `Map[K, V]` is a
//! persistent tree: two versions that differ by one insert *share* every subtree the insert did not
//! pass through, by pointer, and the diff skips a shared subtree whole. So the delta at the source
//! costs what the delta is worth. Everything downstream of that is ordinary differential dataflow.
//!
//! # Correctness before speed
//!
//! The engine's output must be *identical* to recomputing the view — not close, identical, because
//! the rendered page is diffed into a patch stream and replayed bit for bit (§4.8). Three things
//! make that checkable rather than hoped for:
//!
//! 1. **Every operator the plan cannot decompose is a full recompute** ([`Op::Pointwise`]), so a
//!    program the analysis does not understand is *slow*, never wrong.
//! 2. **Order is a key, not a sort.** Each arrangement is a `BTreeMap` whose key reproduces the
//!    order the recompute would have produced ([`crate::plan`]), so `map_values` order, `sort_by`
//!    stability and `concat_lists` position all fall out of the key rather than out of a final pass.
//! 3. **An error resets the engine.** A per-element function that fails leaves an arrangement
//!    half-updated, so [`Engine::render`] discards everything and the next call rebuilds. A stale
//!    arrangement is the one failure mode that would be invisible.
//!
//! `beck-cli/tests/incremental_engine.rs` is the harness: every corpus program, every event of a
//! generated log, engine against recompute, byte for byte.
//!
//! # What "changed" means, and why it is never a deep comparison
//!
//! A pointwise operator re-runs when an input changed. Deciding that by structural equality would
//! reintroduce the `O(n)` this module exists to remove, so `same` is a *conservative* test:
//! scalars compare by value, collections and rendered trees by pointer. It answers "unchanged" only
//! when it is certain, and "changed" costs a recompute that the old runtime did unconditionally.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};

use beck_diag::Span;

use crate::backend::{Backend, Callable, ExecError};
use crate::core::Value;
use crate::plan::{Fun, Op, OpId, Plan};
use crate::pmap::PMap;
use crate::split::Placed;

/// What orders an entry inside an arrangement. See [`crate::plan`] for where each operator's key
/// comes from.
pub type Key = Arc<[Value]>;

/// One entry's fate at an operator's output.
#[derive(Clone, Debug)]
pub struct Change {
    pub key: Key,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

/// An operator's output as a keyed collection — §5.3's "arrangement".
#[derive(Clone, Debug, Default)]
struct Arrangement {
    entries: BTreeMap<Key, Value>,
    /// The `Value::List` a pointwise consumer needs, built on demand and dropped when the
    /// arrangement moves. This is the boundary between the maintained region and the rest: a
    /// consumer that only asks for the *size* never forces it, which is exactly why `list_len` is
    /// an operator rather than a pointwise call.
    ///
    /// A `OnceLock` rather than an `Option` because a *shared* arrangement is read by many
    /// subscribers at once, through a read lock ([`SharedDataflow`]): the first one to need the
    /// list builds it, and the rest get the same `Arc`. With an `Option` the cache would need the
    /// write lock, which would serialise every subscriber behind the first — and the list is the
    /// one thing worth sharing most, because building it is the `O(n)` §24.6 named.
    listed: OnceLock<Value>,
}

impl Arrangement {
    fn touch(&mut self) {
        self.listed = OnceLock::new();
    }

    /// The list this arrangement stands for, and how many entries had to be copied to build it —
    /// zero when another reader already had.
    fn listed_value(&self) -> (Value, u64) {
        if let Some(v) = self.listed.get() {
            return (v.clone(), 0);
        }
        let listed = Value::List(Arc::new(self.entries.values().cloned().collect()));
        // A race loses the loser's copy and keeps the winner's; both are the same list, so which
        // one wins is not observable. `get_or_init` would be neater and would hold a lock.
        match self.listed.set(listed.clone()) {
            Ok(()) => (listed, self.entries.len() as u64),
            Err(_) => (
                self.listed.get().cloned().unwrap_or(listed),
                self.entries.len() as u64,
            ),
        }
    }
}

#[derive(Clone, Debug)]
enum Out {
    Val(Value),
    Arr(Arrangement),
}

impl Default for Out {
    fn default() -> Self {
        Out::Val(Value::Unit)
    }
}

/// One operator's runtime state.
#[derive(Default)]
struct Cell {
    out: Out,
    changed: bool,
    /// For an arrangement: what moved this tick.
    changes: Vec<Change>,
    /// Set when this operator threw its arrangement away and rebuilt it this tick.
    ///
    /// A rebuild emits *inserts only* — there is no previous arrangement left to derive removals
    /// from — so a consumer that merely applied those inserts to its own arrangement would keep
    /// every entry the rebuild dropped. This is how a subscriber switching sessions saw another
    /// subscriber's rows, and the flag is the fix: a rebuild is contagious downstream.
    rebuilt: bool,
    /// `map_values`: the map this operator last saw, so the next one can be diffed against it.
    seen_map: PMap<Value, Value>,
    /// For each input that arrives as a plain list rather than as an arrangement, the copy this
    /// operator last saw. A list-valued input has no deltas of its own, so the operator makes them.
    shadow: Vec<BTreeMap<Key, Value>>,
    /// `sort_by`: where each input key currently sits in the output order.
    positions: BTreeMap<Key, Key>,
    /// `flatten`: how many entries each input key currently contributes, so the old ones can be
    /// withdrawn without scanning the arrangement.
    counts: BTreeMap<Key, usize>,
}

/// What one [`Engine::render`] cost, in units that do not depend on the machine.
///
/// Wall-clock is measured in the harness; this is what a test asserts on, because "the count did
/// not visit every row" is the claim, and a timing assertion in CI is a flake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// Per-element functions applied — the `f` of a `map_list`, the predicate of a `filter_list`.
    pub applications: u64,
    /// Entries a delta operator inserted, updated or removed.
    pub touched: u64,
    /// Entries copied to hand a pointwise consumer a `Value::List`.
    pub materialised: u64,
    /// Pointwise operators re-evaluated.
    pub recomputed: u64,
}

impl Work {
    /// Everything that scales with the collection rather than with the change.
    pub fn total(&self) -> u64 {
        self.applications + self.touched + self.materialised + self.recomputed
    }
}

/// A plan with every operator's code prepared: one per *program*, shared by every subscription.
///
/// The split between this and [`Engine`] is the difference between a plan and an arrangement, and
/// it is load-bearing for §5.3's fanout. Preparing an operator means asking the backend to turn
/// `Core` into something callable, which a compiling backend does expensively and even a
/// tree-walker does by cloning the expression; doing it per subscriber cost about 90 KB of a
/// subscription that then held 60 entries. A thousand subscribers share one of these.
pub struct Prepared {
    plan: Arc<Plan>,
    /// The pointwise operators' bodies and the collection operators' per-element functions.
    code: Vec<Option<Callable>>,
    funs: Vec<Option<Callable>>,
    /// Constants, evaluated once here and never recomputed.
    consts: Vec<Option<Value>>,
}

impl Prepared {
    pub fn new(plan: Arc<Plan>, backend: &dyn Backend) -> Result<Prepared, ExecError> {
        let n = plan.nodes.len();
        let mut code: Vec<Option<Callable>> = Vec::with_capacity(n);
        let mut funs: Vec<Option<Callable>> = Vec::with_capacity(n);
        for node in &plan.nodes {
            code.push(match &node.op {
                Op::Pointwise { code } => Some(backend.function(code)?),
                _ => None,
            });
            funs.push(match &node.op {
                Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } => {
                    Some(backend.function(&f.code)?)
                }
                _ => None,
            });
        }
        let mut consts: Vec<Option<Value>> = vec![None; n];
        for (&id, expr) in &plan.constants {
            consts[id] = Some(backend.constant(expr)?);
        }
        Ok(Prepared {
            plan,
            code,
            funs,
            consts,
        })
    }

    /// Compile and prepare a sliced program's view in one step.
    pub fn compile(placed: &Placed, backend: &dyn Backend) -> Result<Prepared, ExecError> {
        Prepared::new(Arc::new(Plan::compile(placed)), backend)
    }

    pub fn plan(&self) -> &Arc<Plan> {
        &self.plan
    }
}

/// One subscriber's arrangements over a [`Prepared`] plan.
pub struct Engine {
    prepared: Arc<Prepared>,
    cells: Vec<Cell>,
    /// Which of the plan's operators this engine computes and holds.
    ///
    /// All of them for a standalone engine. For a subscriber attached to a [`SharedDataflow`] it is
    /// exactly the `per_session` nodes: the rest arrive from upstream, held once between every
    /// subscriber, which is §5.3's sentence.
    owns: Arc<[bool]>,
    /// Whether any state at all has been established. Cleared by an error, so the next render
    /// rebuilds rather than trusting a half-updated arrangement.
    warm: bool,
    /// The shared version this engine last rendered against, so the changes it has not yet seen can
    /// be found. Meaningless for a standalone engine, which has no upstream to lag behind.
    seen: u64,
    work: Work,
}

impl Engine {
    /// A fresh subscriber's view over a plan the program prepared once, computing every operator
    /// itself.
    pub fn new(prepared: Arc<Prepared>) -> Engine {
        let owns: Arc<[bool]> = (0..prepared.plan.nodes.len()).map(|_| true).collect();
        Engine::for_nodes(prepared, owns)
    }

    /// A subscriber's half of a plan whose shared prefix a [`SharedDataflow`] maintains.
    ///
    /// It owns the `per_session` operators and nothing else. Rendering it requires the shared side
    /// — [`SharedDataflow::render`] — because the operators it does not own are where its inputs
    /// come from.
    pub fn subscriber(prepared: Arc<Prepared>) -> Engine {
        let owns: Arc<[bool]> = prepared.plan.nodes.iter().map(|n| n.per_session).collect();
        Engine::for_nodes(prepared, owns)
    }

    fn for_nodes(prepared: Arc<Prepared>, owns: Arc<[bool]>) -> Engine {
        let mut cells: Vec<Cell> = (0..prepared.plan.nodes.len())
            .map(|_| Cell::default())
            .collect();
        for (i, v) in prepared.consts.iter().enumerate() {
            if let Some(v) = v {
                if owns[i] {
                    cells[i].out = Out::Val(v.clone());
                }
            }
        }
        Engine {
            prepared,
            cells,
            owns,
            warm: false,
            seen: 0,
            work: Work::default(),
        }
    }

    /// Whether this engine computes an operator itself, rather than reading it from upstream.
    fn owns(&self, id: OpId) -> bool {
        self.owns[id]
    }

    pub fn plan(&self) -> &Arc<Plan> {
        &self.prepared.plan
    }

    /// What the last [`Engine::render`] cost.
    pub fn work(&self) -> Work {
        self.work
    }

    /// How many entries every arrangement is holding — §5.3's per-session memory, in the unit that
    /// scales.
    pub fn arranged(&self) -> u64 {
        self.arrangement_entries(|_| true)
    }

    /// The same count, restricted to arrangements that do **not** read the session.
    ///
    /// This is the part §5.3 says a thousand subscribers should hold *once* between them. A
    /// subscriber attached to a [`SharedDataflow`] does not own those operators at all, so this is
    /// zero for it and the entries are counted once, on [`SharedDataflow::arranged`].
    pub fn arranged_shared(&self) -> u64 {
        self.arrangement_entries(|per_session| !per_session)
    }

    fn arrangement_entries(&self, want: impl Fn(bool) -> bool) -> u64 {
        self.cells
            .iter()
            .enumerate()
            .filter(|(i, _)| self.owns[*i])
            .map(|(i, c)| match &c.out {
                Out::Arr(a) if want(self.prepared.plan.nodes[i].per_session) => {
                    a.entries.len() as u64
                }
                _ => 0,
            })
            .sum()
    }

    /// Discard everything. The next render rebuilds from the state it is given.
    pub fn reset(&mut self) {
        for (i, cell) in self.cells.iter_mut().enumerate() {
            *cell = Cell::default();
            // A constant's value is still valid — only the arrangements are suspect.
            if let Some(v) = &self.prepared.consts[i] {
                if self.owns[i] {
                    cell.out = Out::Val(v.clone());
                }
            }
        }
        self.warm = false;
        self.seen = 0;
    }

    /// Render this subscriber's view of a state, maintaining whatever the plan can maintain.
    ///
    /// Correct for *any* state, not only the successor of the last one: an operator that cannot
    /// derive a delta rebuilds. That matters because a reconnecting subscriber is rendered against
    /// an older state (`beck-rt`'s resumption path), and an engine that assumed monotonic progress
    /// would quietly serve it the wrong page.
    pub fn render(&mut self, state: &Value, session: &Value) -> Result<Value, ExecError> {
        self.render_from(None, state, session)
    }

    /// The same render, with the operators this engine does not own arriving from upstream.
    fn render_from(
        &mut self,
        up: Option<Upstream<'_>>,
        state: &Value,
        session: &Value,
    ) -> Result<Value, ExecError> {
        self.work = Work::default();
        match self
            .tick(up, state, session)
            .and_then(|()| self.materialise(up, self.prepared.plan.root))
        {
            Ok(v) => {
                self.warm = true;
                Ok(v)
            }
            Err(e) => {
                // A failed per-element function leaves an arrangement holding entries from two
                // different states. Nothing downstream could detect that, so it is thrown away.
                self.reset();
                Err(e)
            }
        }
    }

    /// Advance the operators this engine owns, without assembling a page from them.
    ///
    /// This is the shared half of [`SharedDataflow`]: the root of the plan is per-session and this
    /// engine does not own it, so there is nothing at the top to materialise.
    fn advance(&mut self, state: &Value) -> Result<(), ExecError> {
        self.work = Work::default();
        match self.tick(None, state, &Value::Unit) {
            Ok(()) => {
                self.warm = true;
                Ok(())
            }
            Err(e) => {
                self.reset();
                Err(e)
            }
        }
    }

    fn tick(
        &mut self,
        up: Option<Upstream<'_>>,
        state: &Value,
        session: &Value,
    ) -> Result<(), ExecError> {
        let cold = !self.warm;
        // The plan is behind an `Arc`, so this is one refcount rather than a clone of every
        // operator's `Core` — which is what matching on `self.plan` directly would have cost, once
        // per node per event.
        let plan = self.prepared.plan.clone();
        for id in 0..plan.nodes.len() {
            // Not ours: it belongs to the shared dataflow, and reading it goes through `up`.
            if !self.owns(id) {
                continue;
            }
            match &plan.nodes[id].op {
                Op::State => {
                    self.cells[id].rebuilt = false;
                    // Always "changed": the caller renders because the fold moved, and proving it
                    // did not would cost a structural comparison of the whole accumulator — the
                    // recount this engine exists to remove. Every consumer below is either a field
                    // read (`O(1)`) or a `map_values` (`O(δ log n)`).
                    self.cells[id].out = Out::Val(state.clone());
                    self.cells[id].changed = true;
                }
                Op::Session => {
                    self.cells[id].rebuilt = false;
                    let changed =
                        cold || !matches!(&self.cells[id].out, Out::Val(v) if same(v, session));
                    self.cells[id].out = Out::Val(session.clone());
                    self.cells[id].changed = changed;
                }
                Op::Const => {
                    self.cells[id].rebuilt = false;
                    self.cells[id].changed = cold;
                }
                Op::Pointwise { .. } => self.pointwise(up, id, cold)?,
                Op::MapValues => self.map_values(up, id, cold)?,
                Op::MapList { f } => self.map_list(up, id, f, cold)?,
                Op::FilterList { f } => self.filter_list(up, id, f, cold)?,
                Op::SortBy { f } => self.sort_by(up, id, f, cold)?,
                Op::Concat => self.concat(up, id, cold)?,
                Op::Flatten => self.flatten(up, id, cold)?,
                Op::Count => self.aggregate(up, id, cold, false)?,
                Op::IsEmpty => self.aggregate(up, id, cold, true)?,
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Operators
    // ---------------------------------------------------------------------------------------

    fn pointwise(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        cold: bool,
    ) -> Result<(), ExecError> {
        self.cells[id].rebuilt = false;
        // An `Arc` bump, not a copy: this runs for every pointwise operator on every tick.
        let plan = self.prepared.plan.clone();
        let inputs = &plan.nodes[id].inputs;
        if !cold && !inputs.iter().any(|&i| self.changed_of(up, i)) {
            self.cells[id].changed = false;
            return Ok(());
        }
        let mut args = Vec::with_capacity(inputs.len());
        for &i in inputs {
            args.push(self.materialise(up, i)?);
        }
        let f = self.prepared.code[id]
            .as_ref()
            .ok_or_else(|| ExecError::new("a pointwise operator has no prepared body", Span::NONE))?
            .clone();
        let next = f(args)?;
        self.work.recomputed += 1;
        let changed = match &self.cells[id].out {
            Out::Val(prev) => !same(prev, &next),
            Out::Arr(_) => true,
        };
        self.cells[id].out = Out::Val(next);
        self.cells[id].changed = changed || cold;
        Ok(())
    }

    /// `map_values(m)` — the source. Every other operator's deltas descend from this one.
    fn map_values(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        cold: bool,
    ) -> Result<(), ExecError> {
        let input = self.prepared.plan.nodes[id].inputs[0];
        if !cold && !self.changed_of(up, input) {
            self.cells[id].changed = false;
            self.cells[id].changes.clear();
            self.cells[id].rebuilt = false;
            return Ok(());
        }
        let source = match self.out_of(up, input)? {
            Out::Val(Value::Map(m)) => Some(m.clone()),
            // Not a map. The plan said this was `map_values`, so the only way here is a program the
            // checker would have refused; rebuild wholesale rather than guess.
            _ => None,
        };
        let Some(next) = source else {
            let whole = self.materialise(up, input)?;
            let entries = list_entries(&whole);
            return self.replace(id, entries);
        };
        let seen = if cold {
            PMap::new()
        } else {
            self.cells[id].seen_map.clone()
        };
        let mut arr = if cold {
            Arrangement::default()
        } else {
            match std::mem::take(&mut self.cells[id].out) {
                Out::Arr(a) => a,
                Out::Val(_) => Arrangement::default(),
            }
        };
        let mut changes = Vec::new();
        for c in seen.diff(&next) {
            let key: Key = Arc::from(vec![c.key]);
            match &c.new {
                Some(v) => {
                    arr.entries.insert(key.clone(), v.clone());
                }
                None => {
                    arr.entries.remove(&key);
                }
            }
            changes.push(Change {
                key,
                old: c.old,
                new: c.new,
            });
        }
        self.cells[id].seen_map = next;
        self.publish(id, arr, changes, cold);
        Ok(())
    }

    fn map_list(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        f: &Fun,
        cold: bool,
    ) -> Result<(), ExecError> {
        let (incoming, rebuild) = self.incoming(up, id, 0, f, cold)?;
        if incoming.is_empty() && !rebuild {
            self.cells[id].changed = false;
            self.cells[id].changes.clear();
            // Cleared, and this is not housekeeping. `rebuilt` means "threw its arrangement away
            // *this tick*"; leaving the cold start's `true` here made it mean "has ever rebuilt",
            // and a rebuild is contagious downstream — so every operator below a collection that
            // had stopped changing rebuilt on every event, for the life of the subscription.
            // `concat` and `flatten` always cleared it; these three never did.
            self.cells[id].rebuilt = false;
            return Ok(());
        }
        let call = self.fun_of(id)?;
        let captured = self.captures(up, f)?;
        let mut arr = self.take_arrangement(id, rebuild);
        let mut changes = Vec::new();
        for c in incoming {
            match c.new {
                Some(v) => {
                    let mut args = captured.clone();
                    args.push(v);
                    let mapped = call(args)?;
                    self.work.applications += 1;
                    let old = arr.entries.insert(c.key.clone(), mapped.clone());
                    changes.push(Change {
                        key: c.key,
                        old,
                        new: Some(mapped),
                    });
                }
                None => {
                    let old = arr.entries.remove(&c.key);
                    if old.is_some() {
                        changes.push(Change {
                            key: c.key,
                            old,
                            new: None,
                        });
                    }
                }
            }
        }
        self.publish(id, arr, changes, rebuild);
        Ok(())
    }

    fn filter_list(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        f: &Fun,
        cold: bool,
    ) -> Result<(), ExecError> {
        let (incoming, rebuild) = self.incoming(up, id, 0, f, cold)?;
        if incoming.is_empty() && !rebuild {
            self.cells[id].changed = false;
            self.cells[id].changes.clear();
            // Cleared, and this is not housekeeping. `rebuilt` means "threw its arrangement away
            // *this tick*"; leaving the cold start's `true` here made it mean "has ever rebuilt",
            // and a rebuild is contagious downstream — so every operator below a collection that
            // had stopped changing rebuilt on every event, for the life of the subscription.
            // `concat` and `flatten` always cleared it; these three never did.
            self.cells[id].rebuilt = false;
            return Ok(());
        }
        let call = self.fun_of(id)?;
        let captured = self.captures(up, f)?;
        let mut arr = self.take_arrangement(id, rebuild);
        let mut changes = Vec::new();
        for c in incoming {
            let keep = match &c.new {
                Some(v) => {
                    let mut args = captured.clone();
                    args.push(v.clone());
                    let verdict = call(args)?;
                    self.work.applications += 1;
                    verdict.as_bool().unwrap_or(false)
                }
                None => false,
            };
            if keep {
                let v = c.new.expect("kept means present");
                let old = arr.entries.insert(c.key.clone(), v.clone());
                changes.push(Change {
                    key: c.key,
                    old,
                    new: Some(v),
                });
            } else if let Some(old) = arr.entries.remove(&c.key) {
                changes.push(Change {
                    key: c.key,
                    old: Some(old),
                    new: None,
                });
            }
        }
        self.publish(id, arr, changes, rebuild);
        Ok(())
    }

    /// `sort_by(xs, k)` — an ordered arrangement, maintained by insertion.
    ///
    /// The output key is `k(x)` followed by the input's key. That second component is what makes
    /// the sort *stable* in the same way the recompute's is: two elements with equal keys keep the
    /// order they had at the input, and "the order they had" is exactly the input's key.
    fn sort_by(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        f: &Fun,
        cold: bool,
    ) -> Result<(), ExecError> {
        let (incoming, rebuild) = self.incoming(up, id, 0, f, cold)?;
        if incoming.is_empty() && !rebuild {
            self.cells[id].changed = false;
            self.cells[id].changes.clear();
            // Cleared, and this is not housekeeping. `rebuilt` means "threw its arrangement away
            // *this tick*"; leaving the cold start's `true` here made it mean "has ever rebuilt",
            // and a rebuild is contagious downstream — so every operator below a collection that
            // had stopped changing rebuilt on every event, for the life of the subscription.
            // `concat` and `flatten` always cleared it; these three never did.
            self.cells[id].rebuilt = false;
            return Ok(());
        }
        let call = self.fun_of(id)?;
        let captured = self.captures(up, f)?;
        let mut arr = self.take_arrangement(id, rebuild);
        if rebuild {
            self.cells[id].positions.clear();
        }
        let mut positions = std::mem::take(&mut self.cells[id].positions);
        let mut changes = Vec::new();
        for c in incoming {
            if let Some(was) = positions.remove(&c.key) {
                if let Some(old) = arr.entries.remove(&was) {
                    changes.push(Change {
                        key: was,
                        old: Some(old),
                        new: None,
                    });
                }
            }
            let Some(v) = c.new else { continue };
            let mut args = captured.clone();
            args.push(v.clone());
            let sort_key = call(args)?;
            self.work.applications += 1;
            let mut out_key: Vec<Value> = vec![sort_key];
            out_key.extend(c.key.iter().cloned());
            let out_key: Key = Arc::from(out_key);
            arr.entries.insert(out_key.clone(), v.clone());
            positions.insert(c.key, out_key.clone());
            changes.push(Change {
                key: out_key,
                old: None,
                new: Some(v),
            });
        }
        self.cells[id].positions = positions;
        self.publish(id, arr, changes, rebuild);
        Ok(())
    }

    /// `concat_lists([a, b, …])` — a union of delta streams, keyed by which stream.
    fn concat(&mut self, up: Option<Upstream<'_>>, id: OpId, cold: bool) -> Result<(), ExecError> {
        let plan = self.prepared.plan.clone();
        let inputs = &plan.nodes[id].inputs;
        let rebuild = cold || inputs.iter().any(|&i| self.rebuilt_of(up, i));
        let mut arr = self.take_arrangement(id, rebuild);
        let mut changes = Vec::new();
        for (slot, input) in inputs.iter().copied().enumerate() {
            let incoming = self.feed(up, id, slot, input, rebuild)?;
            for c in incoming {
                let mut key: Vec<Value> = vec![Value::Int(slot as i64)];
                key.extend(c.key.iter().cloned());
                let key: Key = Arc::from(key);
                match c.new {
                    Some(v) => {
                        let old = arr.entries.insert(key.clone(), v.clone());
                        changes.push(Change {
                            key,
                            old,
                            new: Some(v),
                        });
                    }
                    None => {
                        if let Some(old) = arr.entries.remove(&key) {
                            changes.push(Change {
                                key,
                                old: Some(old),
                                new: None,
                            });
                        }
                    }
                }
            }
        }
        if changes.is_empty() && !rebuild {
            self.cells[id].out = Out::Arr(arr);
            self.cells[id].changed = false;
            self.cells[id].changes.clear();
            self.cells[id].rebuilt = false;
            return Ok(());
        }
        self.publish(id, arr, changes, rebuild);
        Ok(())
    }

    /// `concat_lists(xs)` where `xs` is a collection of lists — a flatten, and what every `for`
    /// loop in a `ui:` block compiles to.
    ///
    /// The output key is the input's key followed by the position inside that element's list, so
    /// one row's children move without disturbing anybody else's, and the order is the order the
    /// recompute would have produced.
    fn flatten(&mut self, up: Option<Upstream<'_>>, id: OpId, cold: bool) -> Result<(), ExecError> {
        let input = self.prepared.plan.nodes[id].inputs[0];
        let rebuild = cold || self.rebuilt_of(up, input);
        let incoming = self.feed(up, id, 0, input, rebuild)?;
        if incoming.is_empty() && !rebuild {
            self.cells[id].changed = false;
            self.cells[id].changes.clear();
            self.cells[id].rebuilt = false;
            return Ok(());
        }
        let mut arr = self.take_arrangement(id, rebuild);
        if rebuild {
            self.cells[id].counts.clear();
        }
        let mut counts = std::mem::take(&mut self.cells[id].counts);
        let mut changes = Vec::new();
        for c in incoming {
            if let Some(n) = counts.remove(&c.key) {
                for i in 0..n {
                    let key = inner_key(&c.key, i);
                    if let Some(old) = arr.entries.remove(&key) {
                        changes.push(Change {
                            key,
                            old: Some(old),
                            new: None,
                        });
                    }
                }
            }
            let Some(v) = c.new else { continue };
            let items = v.as_list().cloned().unwrap_or_default();
            for (i, item) in items.iter().enumerate() {
                let key = inner_key(&c.key, i);
                arr.entries.insert(key.clone(), item.clone());
                changes.push(Change {
                    key,
                    old: None,
                    new: Some(item.clone()),
                });
            }
            counts.insert(c.key, items.len());
        }
        self.cells[id].counts = counts;
        self.publish(id, arr, changes, rebuild);
        Ok(())
    }

    /// `list_len` and `list_is_empty`: read the arrangement's size.
    ///
    /// This is §3.8's sentence, mechanised. It reads `entries.len()` — `O(1)` — and, crucially,
    /// never calls [`Engine::materialise`], so a program that only asks how many there are never
    /// pays for a list of them.
    fn aggregate(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        cold: bool,
        emptiness: bool,
    ) -> Result<(), ExecError> {
        self.cells[id].rebuilt = false;
        let input = self.prepared.plan.nodes[id].inputs[0];
        if !cold && !self.changed_of(up, input) {
            self.cells[id].changed = false;
            return Ok(());
        }
        let n = match self.out_of(up, input)? {
            Out::Arr(a) => a.entries.len(),
            Out::Val(Value::List(xs)) => xs.len(),
            Out::Val(Value::Map(m)) => m.len(),
            Out::Val(_) => {
                let whole = self.materialise(up, input)?;
                whole.as_list().map(|l| l.len()).unwrap_or(0)
            }
        };
        let next = if emptiness {
            Value::Bool(n == 0)
        } else {
            Value::Int(n as i64)
        };
        let changed = match &self.cells[id].out {
            Out::Val(prev) => !same(prev, &next),
            Out::Arr(_) => true,
        };
        self.cells[id].out = Out::Val(next);
        self.cells[id].changed = changed || cold;
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Plumbing
    // ---------------------------------------------------------------------------------------

    /// The changes arriving at a collection operator, and whether it has to rebuild.
    ///
    /// Three things force a rebuild, and only the first is interesting:
    ///
    /// * the operator's per-element function *captured* something that moved —
    ///   `lambda t: t.owner == session.actor` is a different predicate for a different session, so
    ///   every element has to be reconsidered. This is the one case where the answer genuinely does
    ///   depend on the whole collection;
    /// * an input rebuilt, because a rebuild's changes are inserts with no matching removals;
    /// * the engine is cold.
    fn incoming(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        slot: usize,
        f: &Fun,
        cold: bool,
    ) -> Result<(Vec<Change>, bool), ExecError> {
        let input = self.prepared.plan.nodes[id].inputs[slot];
        let rebuild = cold
            || self.rebuilt_of(up, input)
            || f.captures.iter().any(|&c| self.changed_of(up, c));
        let changes = self.feed(up, id, slot, input, rebuild)?;
        Ok((changes, rebuild))
    }

    /// Changes at one input, whether it is an arrangement or a plain list.
    fn feed(
        &mut self,
        up: Option<Upstream<'_>>,
        id: OpId,
        slot: usize,
        input: OpId,
        whole: bool,
    ) -> Result<Vec<Change>, ExecError> {
        let is_arr = matches!(self.out_of(up, input)?, Out::Arr(_));
        if is_arr && !whole {
            return Ok(if self.changed_of(up, input) {
                self.changes_of(up, input)
            } else {
                Vec::new()
            });
        }
        if is_arr {
            let changes: Vec<Change> = match self.out_of(up, input)? {
                Out::Arr(a) => a
                    .entries
                    .iter()
                    .map(|(k, v)| Change {
                        key: k.clone(),
                        old: None,
                        new: Some(v.clone()),
                    })
                    .collect(),
                Out::Val(_) => Vec::new(),
            };
            while self.cells[id].shadow.len() <= slot {
                self.cells[id].shadow.push(BTreeMap::new());
            }
            self.cells[id].shadow[slot].clear();
            return Ok(changes);
        }
        // A plain list: no deltas of its own, so this operator makes them by comparing against the
        // copy it last saw. `O(n)` in the list's length — which is the honest cost of a collection
        // that arrived from a `match` or an `if` rather than from an arrangement.
        if !whole && !self.changed_of(up, input) {
            return Ok(Vec::new());
        }
        let value = self.materialise(up, input)?;
        let next: BTreeMap<Key, Value> = list_entries(&value).into_iter().collect();
        while self.cells[id].shadow.len() <= slot {
            self.cells[id].shadow.push(BTreeMap::new());
        }
        if whole {
            self.cells[id].shadow[slot].clear();
        }
        // Diff before storing, so `next` moves into the shadow rather than being cloned into it.
        let prev = &self.cells[id].shadow[slot];
        let mut changes = Vec::new();
        for (key, v) in &next {
            match prev.get(key) {
                Some(before) if before == v => {}
                before => changes.push(Change {
                    key: key.clone(),
                    old: before.cloned(),
                    new: Some(v.clone()),
                }),
            }
        }
        for (key, before) in prev {
            if !next.contains_key(key) {
                changes.push(Change {
                    key: key.clone(),
                    old: Some(before.clone()),
                    new: None,
                });
            }
        }
        changes.sort_by(|a, b| a.key.cmp(&b.key));
        self.cells[id].shadow[slot] = next;
        Ok(changes)
    }

    fn take_arrangement(&mut self, id: OpId, rebuild: bool) -> Arrangement {
        if rebuild {
            self.cells[id].positions.clear();
            return Arrangement::default();
        }
        match std::mem::take(&mut self.cells[id].out) {
            Out::Arr(a) => a,
            Out::Val(_) => Arrangement::default(),
        }
    }

    fn publish(&mut self, id: OpId, mut arr: Arrangement, changes: Vec<Change>, rebuilt: bool) {
        self.work.touched += changes.len() as u64;
        arr.touch();
        self.cells[id].changed = !changes.is_empty() || rebuilt;
        self.cells[id].changes = changes;
        self.cells[id].rebuilt = rebuilt;
        self.cells[id].out = Out::Arr(arr);
    }

    /// The whole collection as inserts — the path taken when an operator cannot derive a delta.
    fn replace(&mut self, id: OpId, entries: Vec<(Key, Value)>) -> Result<(), ExecError> {
        let mut arr = Arrangement::default();
        let mut changes = Vec::new();
        for (key, v) in entries {
            arr.entries.insert(key.clone(), v.clone());
            changes.push(Change {
                key,
                old: None,
                new: Some(v),
            });
        }
        self.publish(id, arr, changes, true);
        Ok(())
    }

    fn fun_of(&self, id: OpId) -> Result<Callable, ExecError> {
        self.prepared.funs[id].clone().ok_or_else(|| {
            ExecError::new("a collection operator has no prepared function", Span::NONE)
        })
    }

    fn captures(&mut self, up: Option<Upstream<'_>>, f: &Fun) -> Result<Vec<Value>, ExecError> {
        let mut out = Vec::with_capacity(f.captures.len());
        for &c in &f.captures {
            out.push(self.materialise(up, c)?);
        }
        Ok(out)
    }

    /// The value of a node, building the list an arrangement stands for if a consumer needs it.
    ///
    /// This is where the remaining `O(n)` lives, and naming it is the point: assembling `n`
    /// elements into a `Value::List` for a pointwise consumer copies `n` handles per event even
    /// when one of them moved. What it does *not* do is re-derive the elements — those came from
    /// the arrangement, and only the changed ones were computed.
    ///
    /// For a *shared* arrangement it is also copied only once between every subscriber, because the
    /// cache lives beside the arrangement rather than in the engine that asked.
    fn materialise(&mut self, up: Option<Upstream<'_>>, id: OpId) -> Result<Value, ExecError> {
        let (listed, n) = match self.out_of(up, id)? {
            Out::Val(v) => return Ok(v.clone()),
            Out::Arr(a) => a.listed_value(),
        };
        self.work.materialised += n;
        Ok(listed)
    }

    // ---------------------------------------------------------------------------------------
    // Reading a node this engine may not own
    // ---------------------------------------------------------------------------------------

    /// A node's output, from this engine's own cells or from the shared dataflow above it.
    fn out_of<'e>(&'e self, up: Option<Upstream<'e>>, id: OpId) -> Result<&'e Out, ExecError> {
        if self.owns(id) {
            return Ok(&self.cells[id].out);
        }
        match up {
            Some(u) => Ok(u.out(id)),
            None => Err(missing_upstream(id)),
        }
    }

    /// Whether a node moved since this engine last looked at it.
    ///
    /// For an upstream node that is "since the version this subscriber last rendered", not "at the
    /// latest version" — a subscriber that skipped three events has to see all three, or an
    /// operator below it would keep an entry the shared side has already withdrawn.
    fn changed_of(&self, up: Option<Upstream<'_>>, id: OpId) -> bool {
        if self.owns(id) {
            return self.cells[id].changed;
        }
        // No upstream where one is needed is an error the caller will raise when it reads the
        // value; answering "changed" here keeps it on the path that does.
        up.map(|u| u.changed(id)).unwrap_or(true)
    }

    fn rebuilt_of(&self, up: Option<Upstream<'_>>, id: OpId) -> bool {
        if self.owns(id) {
            return self.cells[id].rebuilt;
        }
        up.map(|u| u.rebuilt(id)).unwrap_or(true)
    }

    fn changes_of(&self, up: Option<Upstream<'_>>, id: OpId) -> Vec<Change> {
        if self.owns(id) {
            return self.cells[id].changes.clone();
        }
        up.map(|u| u.changes(id)).unwrap_or_default()
    }
}

fn missing_upstream(id: OpId) -> ExecError {
    ExecError::new(
        format!("operator {id} belongs to the shared dataflow, and none was supplied"),
        Span::NONE,
    )
}

// -------------------------------------------------------------------------------------------
// The shared dataflow (§5.3)
// -------------------------------------------------------------------------------------------

/// What the shared dataflow did in advancing from one state version to the next.
///
/// A subscriber renders when it is woken, not when the fold moves, so it can be several versions
/// behind by the time it looks. Its per-session operators need every change since *its* last
/// render, not the latest one — an entry withdrawn at version 8 and never mentioned again would
/// otherwise survive in a subscriber that last rendered at version 7 and next renders at 9.
///
/// A rebuilt operator's changes are deliberately **not** kept: a consumer downstream of a rebuild
/// re-reads the whole arrangement instead of applying changes, so storing them would retain a copy
/// of the collection per remembered version for nothing.
struct Step {
    from: u64,
    to: u64,
    changed: BTreeSet<OpId>,
    rebuilt: BTreeSet<OpId>,
    changes: BTreeMap<OpId, Arc<[Change]>>,
}

struct SharedInner {
    engine: Engine,
    version: u64,
    /// Whether the shared prefix has been computed at all.
    ///
    /// Separate from `version` because a freshly recovered application is at version 0 with a real
    /// accumulator behind it — an empty log is a state, not the absence of one — so "already at the
    /// version you asked for" and "never advanced" are different facts and only one of them means
    /// there is nothing to do.
    started: bool,
    /// Oldest first, and contiguous: `history[k].to == history[k + 1].from`.
    history: VecDeque<Step>,
}

/// The operators of a plan that do not read the session, arranged **once** for every subscriber.
///
/// [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.3:
///
/// > a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one*
/// > shared dataflow whose final per-session operators (filter, project, diff) run per subscriber
///
/// [`crate::plan::Plan`] has said which nodes those are since the plan existed — `per_session` is
/// false for exactly the operators reachable from the accumulator without passing through the
/// session. What was missing was somewhere for them to live that is not one subscriber's engine.
///
/// # The three choices §24.7 said this design had in it
///
/// 1. **Who advances it.** Not the sequencer: that would put view maintenance on the write path and
///    do it for a state nobody is looking at. The *first subscriber to render at a new version*
///    advances it, under a write lock, and every subscriber that renders at that version afterwards
///    finds it done. So the work happens once per version, is paid by a renderer that was about to
///    do it anyway, and does not happen at all when nobody is subscribed.
/// 2. **What a subscriber holds while it renders.** A read lock, for the whole of its own render.
///    Readers do not block readers, so a thousand subscribers render concurrently; the only writer
///    is the advance, which is `O(δ)`. The alternative — publishing an immutable snapshot per
///    version — has to copy any arrangement that moved, which is the `O(n)` this engine exists to
///    remove.
/// 3. **What happens to a subscriber that fell behind.** It replays the changes it missed, from a
///    bounded history of recent versions (`Step`). Beyond that history it rebuilds — correct at
///    any lag, because a rebuild reads the current arrangement whole and a rebuild is already
///    contagious downstream (`Cell::rebuilt`).
///
/// # What is still not shared
///
/// The *page* is per-session in every corpus program, so what is shared is the prefix below the
/// session, not the render. `24-feed.beck` is the case where that prefix is most of the plan and
/// the sketch is the case where it is least; `docs/26` has the table.
pub struct SharedDataflow {
    inner: RwLock<SharedInner>,
    /// How many versions of changes to remember. Beyond this a lagging subscriber rebuilds.
    depth: usize,
    /// How many times the shared prefix has actually been advanced.
    ///
    /// The metric the whole design turns on: a thousand subscribers rendering at one version must
    /// advance it *once*, and a counter is how that is a test rather than a claim.
    advances: std::sync::atomic::AtomicU64,
}

/// How many versions of change history a shared dataflow keeps.
///
/// The cost is one `Change` per entry that moved per remembered version — a delta, not a
/// collection, because a rebuilt operator's changes are not kept. The benefit is that a subscriber
/// this many events behind still updates by delta rather than rebuilding. 64 is well past the point
/// where a subscriber that far behind is the bottleneck.
const HISTORY: usize = 64;

impl SharedDataflow {
    pub fn new(prepared: Arc<Prepared>) -> SharedDataflow {
        let owns: Arc<[bool]> = prepared.plan.nodes.iter().map(|n| !n.per_session).collect();
        SharedDataflow {
            inner: RwLock::new(SharedInner {
                engine: Engine::for_nodes(prepared, owns),
                version: 0,
                started: false,
                history: VecDeque::new(),
            }),
            depth: HISTORY,
            advances: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// A subscriber's engine over the same plan: the per-session operators, and nothing else.
    pub fn subscriber(&self) -> Engine {
        let inner = self.read();
        Engine::subscriber(inner.engine.prepared.clone())
    }

    /// Render one subscriber's page, maintaining the shared prefix once for all of them.
    ///
    /// `version` identifies the state: two calls with the same `version` must pass the same
    /// `state`, because the second is served from what the first computed. Returns the page and the
    /// version it actually reflects, which may be **newer** than the one asked for — another
    /// subscriber may have advanced the shared side in between, and rendering the newer state is
    /// correct where rendering the older one would mean unwinding an arrangement.
    ///
    /// That returned version is not a courtesy. A patch frame is labelled with a `seq` and a
    /// resuming client is served the difference from it (§4.3), so a frame labelled with a state
    /// the page does not reflect is a wrong DOM after the next reconnect.
    pub fn render(
        &self,
        engine: &mut Engine,
        state: &Value,
        version: u64,
        session: &Value,
    ) -> Result<(Value, u64), ExecError> {
        self.advance(state, version)?;
        let inner = self.read();
        let up = Upstream::new(&inner, engine.seen);
        let page = engine.render_from(Some(up), state, session)?;
        engine.seen = inner.version;
        Ok((page, inner.version))
    }

    /// Bring the shared prefix up to `version`, if some other subscriber has not already.
    fn advance(&self, state: &Value, version: u64) -> Result<(), ExecError> {
        {
            let inner = self.read();
            if inner.started && inner.version >= version {
                return Ok(());
            }
        }
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked again under the write lock: between the read above and here, another subscriber
        // may have done exactly this.
        if inner.started && inner.version >= version {
            return Ok(());
        }
        let from = inner.version;
        if let Err(e) = inner.engine.advance(state) {
            // The engine has already discarded its arrangements. The history describes a dataflow
            // that no longer exists, so it goes too, and every subscriber rebuilds.
            inner.history.clear();
            inner.started = false;
            inner.version = 0;
            return Err(e);
        }
        inner.started = true;
        self.advances
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let step = inner.engine.step(from, version);
        inner.history.push_back(step);
        while inner.history.len() > self.depth {
            inner.history.pop_front();
        }
        inner.version = version;
        Ok(())
    }

    /// The version the shared prefix currently reflects.
    pub fn version(&self) -> u64 {
        self.read().version
    }

    /// How many times the shared prefix has been advanced since the process started.
    ///
    /// §5.3's claim is that a thousand subscribers of one view share one dataflow. This is the
    /// number that says so: it counts advances, not renders, so it stays flat as subscribers are
    /// added and moves only when the fold does.
    pub fn advances(&self) -> u64 {
        self.advances.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Entries across every shared arrangement — held once, however many subscribers there are.
    pub fn arranged(&self) -> u64 {
        self.read().engine.arranged()
    }

    /// What the shared prefix retains beyond the accumulator — once, for every subscriber.
    pub fn footprint(&self, base: &Value) -> Footprint {
        self.read().engine.footprint(base)
    }

    pub fn work(&self) -> Work {
        self.read().engine.work()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, SharedInner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Engine {
    /// What this engine's owned operators did in one advance, as a replayable step.
    fn step(&self, from: u64, to: u64) -> Step {
        let mut changed = BTreeSet::new();
        let mut rebuilt = BTreeSet::new();
        let mut changes = BTreeMap::new();
        for (id, cell) in self.cells.iter().enumerate() {
            if !self.owns[id] {
                continue;
            }
            if cell.changed {
                changed.insert(id);
            }
            if cell.rebuilt {
                rebuilt.insert(id);
            } else if !cell.changes.is_empty() {
                // From the slice, one copy: this runs under the shared dataflow's write lock.
                changes.insert(id, Arc::<[Change]>::from(cell.changes.as_slice()));
            }
        }
        Step {
            from,
            to,
            changed,
            rebuilt,
            changes,
        }
    }
}

/// One subscriber's window onto the shared dataflow: its arrangements now, and everything that
/// moved since this subscriber last looked.
#[derive(Clone, Copy)]
struct Upstream<'a> {
    inner: &'a SharedInner,
    since: u64,
    /// Whether the history still covers `since`. When it does not, every upstream node reads as
    /// changed *and* rebuilt, so the subscriber re-reads the arrangements whole — slow, and right.
    resolvable: bool,
}

impl<'a> Upstream<'a> {
    fn new(inner: &'a SharedInner, since: u64) -> Upstream<'a> {
        let resolvable = since == inner.version
            || inner
                .history
                .iter()
                .find(|s| s.to > since)
                .is_some_and(|s| s.from == since);
        Upstream {
            inner,
            since,
            resolvable,
        }
    }

    fn out(&self, id: OpId) -> &'a Out {
        &self.inner.engine.cells[id].out
    }

    fn window(&self) -> impl Iterator<Item = &'a Step> {
        let since = self.since;
        self.inner.history.iter().filter(move |s| s.to > since)
    }

    fn changed(&self, id: OpId) -> bool {
        !self.resolvable || self.window().any(|s| s.changed.contains(&id))
    }

    fn rebuilt(&self, id: OpId) -> bool {
        !self.resolvable || self.window().any(|s| s.rebuilt.contains(&id))
    }

    /// Everything that moved at this node since `since`, in the order it moved.
    ///
    /// Concatenation rather than coalescing: a consumer applies changes in order, so a key that
    /// moved twice is applied twice and lands where the second one put it. Coalescing would save a
    /// consumer one application per repeat and cost a pass over the window; the window is a handful
    /// of deltas.
    fn changes(&self, id: OpId) -> Vec<Change> {
        self.window()
            .filter_map(|s| s.changes.get(&id))
            .flat_map(|c| c.iter().cloned())
            .collect()
    }
}

/// One element of a flattened collection: the outer key, then the position within it.
fn inner_key(outer: &Key, i: usize) -> Key {
    let mut k: Vec<Value> = outer.to_vec();
    k.push(Value::Int(i as i64));
    Arc::from(k)
}

/// A list, as an arrangement keyed by position.
fn list_entries(v: &Value) -> Vec<(Key, Value)> {
    match v {
        Value::List(xs) => xs
            .iter()
            .enumerate()
            .map(|(i, x)| (Arc::from(vec![Value::Int(i as i64)]), x.clone()))
            .collect(),
        Value::Map(m) => m
            .iter()
            .map(|(k, v)| (Arc::from(vec![k.clone()]), v.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// A conservative "did this value move" test: `true` only when it certainly did not.
///
/// Structural equality would be `O(size)`, and doing it once per operator per event would put back
/// the cost this engine removes. Collections and rendered trees therefore compare by *pointer*: two
/// equal-but-separately-built lists answer `false`, which costs one recompute that the old runtime
/// performed unconditionally. Records compare field by field, because that is how a program's own
/// small values — a `Summary`, a `Tally` — are built, and the whole point of a plan is that an
/// event which does not move the summary does not re-render the page below it.
// Not `pub`: this is a *conservative* changed-test — `Arc::ptr_eq` for lists and Html — and a
// caller reading it as equality would be misled.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(x), Value::List(y)) => Arc::ptr_eq(x, y),
        (Value::Map(x), Value::Map(y)) => x.same_root(y),
        (Value::Html(x), Value::Html(y)) => Arc::ptr_eq(x, y),
        (Value::Attr(x), Value::Attr(y)) => Arc::ptr_eq(x, y),
        (
            Value::Data {
                ty: t1,
                variant: v1,
                fields: f1,
            },
            Value::Data {
                ty: t2,
                variant: v2,
                fields: f2,
            },
        ) => {
            if Arc::ptr_eq(f1, f2) {
                return t1 == t2 && v1 == v2;
            }
            t1 == t2
                && v1 == v2
                && f1.len() == f2.len()
                && f1
                    .iter()
                    .zip(f2.iter())
                    .all(|((n1, x), (n2, y))| n1 == n2 && same(x, y))
        }
        _ => false,
    }
}

// -------------------------------------------------------------------------------------------
// Footprint
// -------------------------------------------------------------------------------------------

/// A deterministic byte estimate for the memory a subscription's engine retains.
///
/// `docs/05-tier-lowering.md` §5.3 names per-session memory as one of three metrics to export,
/// and Phase 0's kill gate is written in kilobytes per idle session
/// (`docs/18-phase-0-report.md` §18.3). An engine per subscription is a memory-for-time trade,
/// so the number has to exist.
///
/// It is computed rather than sampled. A resident-set reading moves with the allocator's arena and
/// swung by 2× between runs of the same measurement; a counting allocator would be exact and needs
/// `unsafe`, which this workspace forbids. So this walks what is actually retained and adds up
/// `size_of` plus the bytes behind each allocation, **counting shared structure once**: a `Todo` an
/// arrangement holds is the same `Arc` the accumulator holds, and charging a subscription for it
/// would be the difference between "a handle per row" and "a row per row".
///
/// What it excludes, and therefore under-reports: allocator overhead per allocation, which for many
/// small allocations is substantial. It is a floor on the true cost, not a ceiling.
#[derive(Clone, Copy, Debug, Default)]
pub struct Footprint {
    /// Bytes retained by this engine's cells, arrangements and keys.
    pub bytes: u64,
    /// Of those, the ones in arrangements that do not read the session — what §5.3 says a thousand
    /// subscribers should hold once between them, and this engine holds once each.
    pub shared_bytes: u64,
    pub entries: u64,
}

impl Engine {
    /// What this subscription retains **beyond** the accumulator it renders from.
    ///
    /// `base` is that accumulator, and walking it first is not a detail: an arrangement over
    /// `map_values(s.todos)` holds the *same* `Todo` records the fold holds, by `Arc`, so charging
    /// a subscription for them would report a row per row where the truth is a handle per row. What
    /// remains after the exclusion is what a thousand subscribers actually multiply.
    ///
    /// See [`Footprint`] for what the number does and does not include.
    pub fn footprint(&self, base: &Value) -> Footprint {
        let mut seen = BTreeSet::new();
        value_bytes(base, &mut seen);
        let mut acc = Footprint::default();
        self.footprint_into(&mut seen, &mut acc);
        acc
    }

    /// The same walk, against an exclusion set some other engine has already contributed to.
    ///
    /// Separate from [`Engine::footprint`] because summing per-engine footprints across a fanout
    /// over-reports, and over-reports **exactly the thing this work is about**: with a shared
    /// dataflow, two subscribers' pages hold the same `ul` by `Arc`, and charging both of them for
    /// it would report the sharing as costing what it saves.
    fn footprint_into(&self, seen: &mut BTreeSet<usize>, acc: &mut Footprint) {
        let (mut bytes, mut shared_bytes, mut entries) = (0u64, 0u64, 0u64);
        for (i, cell) in self.cells.iter().enumerate() {
            // An operator this engine does not own costs it nothing: the shared dataflow holds it,
            // and `SharedDataflow::footprint` is where it is charged — once, not once per
            // subscriber, which is the whole point of the split.
            if !self.owns[i] {
                continue;
            }
            let mut here = std::mem::size_of::<Cell>() as u64;
            match &cell.out {
                Out::Val(v) => here += value_bytes(v, seen),
                Out::Arr(a) => {
                    entries += a.entries.len() as u64;
                    for (k, v) in &a.entries {
                        // A `BTreeMap` node holds up to 11 entries plus links; charged per entry as
                        // the pair plus a share of the node.
                        here +=
                            (std::mem::size_of::<Key>() + std::mem::size_of::<Value>() + 24) as u64;
                        here += k.len() as u64 * std::mem::size_of::<Value>() as u64;
                        here += value_bytes(v, seen);
                    }
                    if let Some(listed) = a.listed.get() {
                        here += value_bytes(listed, seen);
                    }
                }
            }
            for (k, v) in &cell.positions {
                here += (k.len() + v.len()) as u64 * std::mem::size_of::<Value>() as u64 + 24;
            }
            bytes += here;
            if !self.prepared.plan.nodes[i].per_session {
                shared_bytes += here;
            }
        }
        acc.bytes += bytes;
        acc.shared_bytes += shared_bytes;
        acc.entries += entries;
    }
}

/// What a whole fanout retains: the accumulator once, the shared dataflow once, and each
/// subscriber's own operators — with every shared allocation counted **exactly once across all of
/// them**.
///
/// Summing [`Engine::footprint`] over the subscribers is the wrong number once there is a shared
/// dataflow, and wrong in the direction that flatters nothing: two subscribers' pages hold the same
/// `ul` by `Arc`, so charging both would report sharing as costing what it saves. This is the
/// number a fanout estimate should be built from, and `docs/26` is where it is.
pub fn fanout_footprint(
    base: &Value,
    shared: Option<&SharedDataflow>,
    engines: &[&Engine],
) -> Footprint {
    let mut seen = BTreeSet::new();
    value_bytes(base, &mut seen);
    let mut acc = Footprint::default();
    if let Some(shared) = shared {
        shared.read().engine.footprint_into(&mut seen, &mut acc);
    }
    for engine in engines {
        engine.footprint_into(&mut seen, &mut acc);
    }
    acc
}

/// Bytes behind a value, counting each shared allocation once.
fn value_bytes(v: &Value, seen: &mut std::collections::BTreeSet<usize>) -> u64 {
    let mut fresh = |p: usize| seen.insert(p);
    match v {
        Value::Unit | Value::Bool(_) | Value::Int(_) | Value::Float(_) => 0,
        Value::Str(s) => {
            if fresh(s.as_ptr() as usize) {
                s.len() as u64
            } else {
                0
            }
        }
        Value::List(xs) => {
            if !fresh(Arc::as_ptr(xs) as usize) {
                return 0;
            }
            let mut n = (xs.len() * std::mem::size_of::<Value>()) as u64;
            for x in xs.iter() {
                n += value_bytes(x, seen);
            }
            n
        }
        Value::Map(m) => {
            let mut n = 0;
            for (k, v) in m.iter() {
                // A tree node: key, value, size and two links.
                n += (2 * std::mem::size_of::<Value>() + 24) as u64;
                n += value_bytes(k, seen) + value_bytes(v, seen);
            }
            n
        }
        Value::Data { fields, .. } => {
            if !fresh(Arc::as_ptr(fields) as usize) {
                return 0;
            }
            let mut n = (fields.len() * (std::mem::size_of::<Value>() + 16 + 24)) as u64;
            for f in fields.values() {
                n += value_bytes(f, seen);
            }
            n
        }
        Value::Html(h) => {
            if !fresh(Arc::as_ptr(h) as usize) {
                return 0;
            }
            html_bytes(h)
        }
        Value::Attr(a) => {
            if fresh(Arc::as_ptr(a) as usize) {
                std::mem::size_of::<crate::core::AttrValue>() as u64
            } else {
                0
            }
        }
        Value::Closure(_) => 0,
    }
}

/// The same estimate for a rendered page, so "what the engine added" has a baseline.
pub fn html_footprint(h: &crate::html::Html) -> u64 {
    html_bytes(h)
}

fn html_bytes(h: &crate::html::Html) -> u64 {
    use crate::html::Html;
    match h {
        Html::Text { text, .. } => std::mem::size_of::<Html>() as u64 + text.len() as u64,
        Html::Element {
            tag,
            attrs,
            key,
            children,
            ..
        } => {
            let mut n = std::mem::size_of::<Html>() as u64 + tag.len() as u64;
            n += key.as_ref().map(|k| k.len()).unwrap_or(0) as u64;
            for (a, b) in attrs {
                n += (a.len() + b.len() + 48) as u64;
            }
            n += (children.len() * std::mem::size_of::<Html>()) as u64;
            for c in children {
                n += html_bytes(c);
            }
            n
        }
    }
}
