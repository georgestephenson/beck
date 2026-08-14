//! Monomorphisation: one compiled function per *instantiation* of a generic definition.
//!
//! # Why this is a backend pass and not a compiler one
//!
//! [`docs/38`](../../../../../docs/38-literature-survey.md) §38.1 settles the question this module
//! would otherwise reopen: **dictionaries are the semantics and monomorphisation is a backend
//! choice**. Its own reason for the split is the one this respects — whole-program specialisation
//! fights incrementality, which is why dictionaries stay the IR's ground truth. So this runs
//! inside the native backends, on a `Program` clone, and nothing in `beck-core`, the evaluator, the
//! checker or the engine can tell it happened. The tree-walker keeps executing the generic
//! definition once, uniformly, exactly as it did.
//!
//! Shared between the two emitters for the reason [`crate::heap`] is: it is not a *code generator*,
//! it is the program both of them are given, and two copies would mean two different subsets
//! compile ([`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.8 is about the emitters, not
//! about everything upstream of them).
//!
//! # Why the type arguments do not have to be recorded anywhere
//!
//! There is no type-argument list in `Core` and no instantiation table on `Program`, and neither is
//! needed, because **every `Core` node already carries its solved type**. A call to a generic
//! definition is `App { func: Global(name), .. }`, and inference writes the *instantiated* function
//! type onto that `Global` node, which `resolve_types` then grounds out with everything else. The
//! definition's own `params` and `ret` still name the rigid `Con("T", [])`. Matching one against the
//! other, positionally, recovers `T := Int`; `recover` in this module is that walk.
//!
//! That is the whole of the mechanism, and it is why this is a pass rather than a project: the
//! checker was already carrying the answer and nobody had read it.
//!
//! # What is refused, and why the budget is not a formality
//!
//! **Polymorphic recursion terminates the language and not this pass.** `def f[T](x: T)` may call
//! `f` at `list[T]`, whose instantiation calls it at `list[list[T]]`, and the set of instantiations
//! is infinite where the program is finite. That is the same wall [`crate::heap`]'s layout survey
//! hits for the same reason, and the answer here is the same: a budget, and a refusal that says so
//! rather than a compiler that does not stop.
//!
//! A template is **kept** — left in the program, refused by name, its call sites unrewritten —
//! when any one of its sites could not be specialised: a budget spent, or a type argument that is
//! still an inference variable. Keeping it is what makes a partial answer safe: a site this pass
//! did not rewrite still names a definition that exists.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use beck_core::check::{Def, Program};
use beck_core::core::{Arm, CoreKind};
use beck_core::ty::Ty;
use beck_core::Core;

/// How many instantiations one generic definition may have.
///
/// Not a judgement about how much code is reasonable — it is the bound that makes polymorphic
/// recursion terminate, so what matters is that it is far above any real program and finite.
/// Across the corpus, both benchmark suites, both SICP chapters, the examples and the standard
/// library there are **65 templates and 28 instantiations**, and the most any one definition has is
/// **three** (`sicp/ch3.beck`'s `seq_map`) — so this is about twenty-one times the biggest real one;
/// `native.rs::a_polymorphically_recursive_definition_is_refused_rather_than_compiled_forever` is
/// the gate, and it needs this to be a number rather than a hope.
pub const MAX_INSTANTIATIONS: usize = 64;

/// What specialising a program produced.
pub struct Mono {
    /// The program the emitters are given: templates replaced by their instantiations.
    pub program: Program,
    /// Which instantiations came from which template, for the report — `firstly` → `firstly@Int`.
    pub made: BTreeMap<Arc<str>, Vec<Arc<str>>>,
    /// Templates left in the program because at least one of their call sites could not be
    /// rewritten, with the reason. These refuse by name, exactly as every generic did before.
    pub kept: BTreeMap<Arc<str>, String>,
}

/// One compiled function per instantiation of every generic definition that has one.
///
/// Never fails: a program this cannot specialise comes back as itself, with every generic
/// definition still in it and [`Mono::kept`] saying why — which is the behaviour this backend had
/// before the pass existed.
///
/// # Why this runs more than once
///
/// A template discovered to be un-specialisable **part way through** has already had some of its
/// instantiations built, and those are worse than useless: each one refuses for calling the next,
/// so a program with one polymorphically recursive definition in it would report sixty-four
/// refusals that are all the same refusal. So a round that keeps a template it had been
/// specialising is thrown away and re-run with that template forbidden from the start, which
/// leaves exactly one refusal naming exactly one definition. `kept` only ever grows, so this
/// terminates in at most one round per template and takes one round for every program in this
/// tree.
pub fn specialise(program: &Program) -> Mono {
    let mut forbidden: BTreeMap<Arc<str>, String> = BTreeMap::new();
    loop {
        let mono = attempt(program, &forbidden);
        if mono.kept.len() == forbidden.len() {
            return mono;
        }
        forbidden = mono.kept;
    }
}

fn attempt(program: &Program, forbidden: &BTreeMap<Arc<str>, String>) -> Mono {
    let templates: BTreeMap<Arc<str>, Def> = program
        .defs
        .iter()
        // A **bounded** definition is not a template. `expand_bounds` already turned its bounds
        // into value parameters holding function values, so specialising it would leave a closure
        // in the signature — which is a different refusal with a different fix, and one this pass
        // must not appear to have answered.
        .filter(|(_, d)| !d.typarams.is_empty() && d.bounds.is_empty())
        // A template an earlier round found it could not finish is not started again: see the note
        // on [`specialise`] about why a half-specialised template is worse than an untouched one.
        .filter(|(n, _)| !forbidden.contains_key(*n))
        .map(|(n, d)| (n.clone(), d.clone()))
        .collect();
    if templates.is_empty() && forbidden.is_empty() {
        return Mono {
            program: program.clone(),
            made: BTreeMap::new(),
            kept: BTreeMap::new(),
        };
    }

    let mut work = Specialiser {
        templates,
        made: BTreeMap::new(),
        kept: forbidden.clone(),
        bodies: Vec::new(),
        names: HashMap::new(),
    };

    // The roots are the definitions that are not templates: whatever the host can call, plus
    // whatever those reach. A template with no concrete caller is reached from nowhere and is
    // dropped, which is correct — it is dead code for this backend.
    let mut program = program.clone();
    for name in program.def_order.clone() {
        if work.templates.contains_key(&name) {
            continue;
        }
        if let Some(def) = program.defs.get_mut(&name) {
            work.rewrite(&mut def.body);
        }
    }
    // Each instantiation's body is itself a caller, and its types are concrete by construction — so
    // the walk closes over the ones an instantiation reaches, including its own recursive calls.
    let mut at = 0;
    while at < work.bodies.len() {
        let mut def = work.bodies[at].clone();
        work.rewrite(&mut def.body);
        work.bodies[at] = def;
        at += 1;
    }

    for def in std::mem::take(&mut work.bodies) {
        program.def_order.push(def.name.clone());
        program.defs.insert(def.name.clone(), def);
    }
    // A template every site of which was rewritten is unreachable, and saying it is "refused"
    // would overstate what this backend cannot do. Two kinds stay:
    //
    //   * one that was **kept** — a site this could not rewrite still names it; and
    //   * one that made **nothing**, because no concrete definition calls it. Dropping that would
    //     be reporting nothing at all about a definition somebody wrote, where "generic, and this
    //     backend was never asked for it at a type" is a true thing a reader can act on.
    for name in work.templates.keys() {
        if work.kept.contains_key(name) || work.made.get(name).is_none_or(|m| m.is_empty()) {
            continue;
        }
        program.defs.remove(name);
        program.def_order.retain(|n| n != name);
    }

    Mono {
        program,
        made: work.made,
        kept: work.kept,
    }
}

struct Specialiser {
    templates: BTreeMap<Arc<str>, Def>,
    made: BTreeMap<Arc<str>, Vec<Arc<str>>>,
    kept: BTreeMap<Arc<str>, String>,
    bodies: Vec<Def>,
    /// Instantiation name by (template, its type arguments rendered), so one is built once.
    names: HashMap<(Arc<str>, String), Arc<str>>,
}

impl Specialiser {
    /// Rewrite every reference to a template in `c` to the instantiation it asks for.
    ///
    /// A `Global` is handled wherever it appears and not only under an `App`, because a generic
    /// definition may be referred to as a **value**: the node's type is the instantiated function
    /// type either way, which is the whole reason this needs no special case.
    fn rewrite(&mut self, c: &mut Core) {
        if let CoreKind::Global(name) = &c.kind {
            if let Some(to) = self.instantiate(name, &c.ty) {
                c.kind = CoreKind::Global(to);
            }
        }
        // The same walk `check::resolve_types` performs, and for the same reason: a node this
        // missed keeps a name that is about to stop existing. `docs/91` §91.3 counted fourteen
        // walks that an arm's **guard** was new to, which is why the arms go through `exprs_mut`.
        match &mut c.kind {
            CoreKind::Lam { body, .. } => self.rewrite(Arc::make_mut(body)),
            CoreKind::App { func, args } => {
                self.rewrite(func);
                for a in args {
                    self.rewrite(a);
                }
            }
            CoreKind::Prim { args, .. } => {
                for a in args {
                    self.rewrite(a);
                }
            }
            CoreKind::Let { value, body, .. } => {
                self.rewrite(value);
                self.rewrite(body);
            }
            CoreKind::If { cond, then, alt } => {
                self.rewrite(cond);
                self.rewrite(then);
                self.rewrite(alt);
            }
            CoreKind::Match { scrutinee, arms } => {
                self.rewrite(scrutinee);
                for e in arms.iter_mut().flat_map(|a: &mut Arm| a.exprs_mut()) {
                    self.rewrite(e);
                }
            }
            CoreKind::Make { fields, .. } | CoreKind::With { fields, .. } => {
                for (_, f) in fields {
                    self.rewrite(f);
                }
            }
            CoreKind::Field { base, .. } => self.rewrite(base),
            CoreKind::ListLit(xs) => {
                for x in xs {
                    self.rewrite(x);
                }
            }
            CoreKind::MapLit(kvs) => {
                for (k, v) in kvs {
                    self.rewrite(k);
                    self.rewrite(v);
                }
            }
            CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
        }
        if let CoreKind::With { base, .. } = &mut c.kind {
            self.rewrite(base);
        }
    }

    /// The instantiation `name` is being asked for at `at`, built if it is new.
    ///
    /// `None` when `name` is not a template, or when this site cannot be specialised — in which
    /// case the template is **kept**, so the reference this leaves alone still names a definition
    /// that exists.
    fn instantiate(&mut self, name: &Arc<str>, at: &Ty) -> Option<Arc<str>> {
        let template = self.templates.get(name)?.clone();
        let Some(args) = recover(&template, at) else {
            self.kept.entry(name.clone()).or_insert_with(|| {
                format!("a call to `{name}` does not say what its type parameters are")
            });
            return None;
        };
        // An argument that is still an inference variable is not a type this backend can lay out,
        // and minting a name from one would make the symbol depend on a variable counter rather
        // than on the program — which `the_module_is_a_function_of_the_program` would catch, and
        // which would be a determinism defect either way.
        if args.values().any(open) {
            self.kept.entry(name.clone()).or_insert_with(|| {
                format!("`{name}` is called where its type parameters are not yet decided")
            });
            return None;
        }

        let shown = template
            .typarams
            .iter()
            .map(|p| args[p].to_string())
            .collect::<Vec<_>>()
            .join(",");
        let key = (name.clone(), shown.clone());
        if let Some(made) = self.names.get(&key) {
            return Some(made.clone());
        }
        let seen = self.made.entry(name.clone()).or_default();
        if seen.len() >= MAX_INSTANTIATIONS {
            self.kept.entry(name.clone()).or_insert_with(|| {
                format!(
                    "generic over {}, and specialising it needs more than {MAX_INSTANTIATIONS} \
                     instantiations — which is what polymorphic recursion looks like from here",
                    template.typarams.join(", ")
                )
            });
            return None;
        }

        // `@` is the separator `Trait::method@Target` already uses for the same purpose, and no
        // source name can contain one.
        let made: Arc<str> = Arc::from(format!("{name}@{shown}"));
        seen.push(made.clone());
        self.names.insert(key, made.clone());

        let mut def = template;
        def.name = made.clone();
        def.typarams = Vec::new();
        for (_, _, ty) in &mut def.params {
            *ty = substitute(ty, &args);
        }
        def.ret = substitute(&def.ret, &args);
        retype(&mut def.body, &args);
        // Pushed rather than walked here: its own body may ask for this same instantiation, and
        // the name is registered above, so the recursion closes on the queue instead of the stack.
        self.bodies.push(def);
        Some(made)
    }
}

/// What the type parameters of `template` were at a use whose type is `at`.
///
/// The definition's declared types still name the rigid `Con("T", [])` that checking minted for the
/// parameter; the use's type is that shape with concrete types in those positions. So walking the
/// two together, one structure at a time, reads the substitution straight off.
///
/// `None` when the two do not have the same shape, which is not something a checked program
/// produces — but this pass is the one place that would turn a wrong assumption into a wrong
/// *program*, so it declines rather than guesses.
fn recover(template: &Def, at: &Ty) -> Option<BTreeMap<Arc<str>, Ty>> {
    let Ty::Fun(params, ret, _) = at else {
        return None;
    };
    if params.len() != template.params.len() {
        return None;
    }
    let names: BTreeSet<Arc<str>> = template.typarams.iter().cloned().collect();
    let mut out = BTreeMap::new();
    for ((_, _, declared), concrete) in template.params.iter().zip(params) {
        walk(declared, concrete, &names, &mut out)?;
    }
    walk(&template.ret, ret, &names, &mut out)?;
    // Every parameter has to have been pinned. One that was not is a definition whose type
    // parameter appears nowhere in its signature, which the checker does not produce and which
    // this could not name an instantiation for.
    template
        .typarams
        .iter()
        .all(|p| out.contains_key(p))
        .then_some(out)
}

/// One structure of `declared` against the same structure of `concrete`, collecting the bindings.
fn walk(
    declared: &Ty,
    concrete: &Ty,
    names: &BTreeSet<Arc<str>>,
    out: &mut BTreeMap<Arc<str>, Ty>,
) -> Option<()> {
    if let Ty::Con(n, args) = declared {
        if args.is_empty() && names.contains(n) {
            // A parameter used twice must be the same type at both, which unification already
            // guaranteed — so disagreeing here means the shapes did not match after all.
            return match out.get(n) {
                Some(already) if already != concrete => None,
                _ => {
                    out.insert(n.clone(), concrete.clone());
                    Some(())
                }
            };
        }
    }
    match (declared, concrete) {
        (Ty::Con(a, xs), Ty::Con(b, ys)) if a == b && xs.len() == ys.len() => {
            for (x, y) in xs.iter().zip(ys) {
                walk(x, y, names, out)?;
            }
            Some(())
        }
        (Ty::Fun(ps, r, _), Ty::Fun(qs, s, _)) if ps.len() == qs.len() => {
            for (p, q) in ps.iter().zip(qs) {
                walk(p, q, names, out)?;
            }
            walk(r, s, names, out)
        }
        // A `Var` on the *declared* side is not a type parameter — it is an inference variable that
        // outlived checking, and nothing can be read off it.
        (Ty::Var(_), _) => Some(()),
        (a, b) if a == b => Some(()),
        _ => None,
    }
}

/// `ty` with every type parameter replaced by what it was at this instantiation.
fn substitute(ty: &Ty, args: &BTreeMap<Arc<str>, Ty>) -> Ty {
    match ty {
        Ty::Con(n, xs) if xs.is_empty() => args.get(n).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Con(n, xs) => Ty::Con(n.clone(), xs.iter().map(|x| substitute(x, args)).collect()),
        Ty::Fun(ps, r, row) => Ty::Fun(
            ps.iter().map(|p| substitute(p, args)).collect(),
            Box::new(substitute(r, args)),
            row.clone(),
        ),
        Ty::Var(_) => ty.clone(),
    }
}

/// The same substitution over every node of a body.
///
/// Only the *types* change. `last_use`, `order` and `locals` are answers about the tree's shape —
/// which reader is last, which written field belongs where, how many bindings a frame holds — and
/// specialising changes no shape at all, so the three passes that computed them do not run again.
fn retype(c: &mut Core, args: &BTreeMap<Arc<str>, Ty>) {
    c.ty = substitute(&c.ty, args);
    match &mut c.kind {
        CoreKind::Lam { body, .. } => retype(Arc::make_mut(body), args),
        CoreKind::App { func, args: xs } => {
            retype(func, args);
            for a in xs {
                retype(a, args);
            }
        }
        CoreKind::Prim { args: xs, .. } => {
            for a in xs {
                retype(a, args);
            }
        }
        CoreKind::Let { value, body, .. } => {
            retype(value, args);
            retype(body, args);
        }
        CoreKind::If { cond, then, alt } => {
            retype(cond, args);
            retype(then, args);
            retype(alt, args);
        }
        CoreKind::Match { scrutinee, arms } => {
            retype(scrutinee, args);
            for e in arms.iter_mut().flat_map(|a: &mut Arm| a.exprs_mut()) {
                retype(e, args);
            }
        }
        CoreKind::Make { fields, .. } | CoreKind::With { fields, .. } => {
            for (_, f) in fields {
                retype(f, args);
            }
        }
        CoreKind::Field { base, .. } => retype(base, args),
        CoreKind::ListLit(xs) => {
            for x in xs {
                retype(x, args);
            }
        }
        CoreKind::MapLit(kvs) => {
            for (k, v) in kvs {
                retype(k, args);
                retype(v, args);
            }
        }
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
    }
    if let CoreKind::With { base, .. } = &mut c.kind {
        retype(base, args);
    }
}

/// Whether a type still holds an inference variable.
fn open(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Con(_, xs) => xs.iter().any(open),
        Ty::Fun(ps, r, _) => ps.iter().any(open) || open(r),
    }
}
