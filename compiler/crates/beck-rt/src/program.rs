//! The bridge between a compiled program and the runtime that drives it.
//!
//! This is the "Roc platform" of Beck ([`docs/05-tier-lowering.md`] §5.2): an effectful Rust host
//! owning I/O, scheduling and memory, executing the pure program. The program supplies four
//! closures the splitter sliced out of the signal graph — `validate`, the fold, its initial state,
//! and the view — and the host supplies everything those closures are not allowed to have.
//!
//! Note what is *not* here: no domain types, no todo, no HTML template. That is the whole claim of
//! Phase 1 over Phase 0 — the same runtime, with the application arriving as compiled `Core`
//! rather than as hand-written Rust.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use beck_core::backend::{Backend, Callable};
use beck_core::core::CoreKind;
use beck_core::{Core, Html, Placed, Value};

use crate::log::Envelope;

/// The compiled program plus the capabilities the host holds on its behalf.
///
/// Note what is absent: any mention of *how* the program executes. The roles are [`Callable`]s a
/// [`Backend`] prepared, so a native backend is a different argument to [`Runtime::new`] rather
/// than a change here — and §4.8's differential test between backends is two `Runtime`s over the
/// same `Placed`.
pub struct Runtime {
    placed: Placed,
    backend: Arc<dyn Backend>,
    /// Prepared once at startup: the roles the splitter sliced out of the signal graph.
    validate: Callable,
    fold_fn: Callable,
    view_fn: Callable,
    init: Value,
    /// The one impure capability the program may reach: minting ids outside a fold.
    uuid: Box<dyn Fn() -> Arc<str> + Send + Sync>,
}

impl Runtime {
    /// Prepare a program for execution by a given backend.
    ///
    /// The backend is an argument rather than a default because a default is how `beck-rt` ends up
    /// naming one implementation again. This crate does not depend on any backend crate, and that
    /// is the property worth keeping.
    pub fn new(placed: Placed, backend: Arc<dyn Backend>) -> Result<Runtime> {
        let role = |code: &Core, what: &str| -> Result<Callable> {
            backend
                .function(code)
                .map_err(|e| anyhow!("preparing {what}: {e}"))
        };
        let validate = role(&placed.roles.validate, "`validate`")?;
        let fold_fn = role(&placed.roles.fold, "the fold")?;
        let view_fn = role(&placed.roles.view, "the view")?;
        let init = backend
            .constant(&placed.roles.init)
            .map_err(|e| anyhow!("evaluating the initial state: {e}"))?;

        Ok(Runtime {
            placed,
            backend,
            validate,
            fold_fn,
            view_fn,
            init,
            uuid: Box::new(|| Arc::from(uuid::Uuid::now_v7().to_string())),
        })
    }

    /// Which backend prepared this program — for a diagnostic, and for the report that says two
    /// backends disagreed.
    pub fn backend(&self) -> &'static str {
        self.backend.name()
    }

    pub fn placed(&self) -> &Placed {
        &self.placed
    }

    pub fn wire_id(&self) -> &str {
        &self.placed.wire_id
    }

    pub fn initial_state(&self) -> Result<Value> {
        Ok(self.init.clone())
    }

    /// Build the `Proposal` record the program's `validate` expects.
    pub fn proposal(&self, actor: &str, command: Value) -> Value {
        Value::Data {
            ty: Arc::from("Proposal"),
            variant: None,
            fields: Arc::new(std::collections::BTreeMap::from([
                (Arc::from("session"), session(actor)),
                (Arc::from("command"), command),
            ])),
        }
    }

    /// The authority chokepoint. Returns the events a proposal becomes, or why it was refused.
    pub fn validate(&self, state: &Value, proposal: &Value) -> Result<Vec<Value>, String> {
        let out =
            (self.validate)(vec![state.clone(), proposal.clone()]).map_err(|e| e.to_string())?;
        match out.variant() {
            Some("Ok") => match out.field("value").and_then(|v| v.as_list()) {
                Some(events) => Ok(events.clone()),
                None => Err("validate returned Ok without a list of events".into()),
            },
            Some("Err") => Err(out
                .field("error")
                .map(|e| e.display())
                .unwrap_or_else(|| "rejected".into())),
            _ => Err(format!("validate returned {}", out.display())),
        }
    }

    /// The replay-pure fold. `env` supplies `seq`, `at` and `actor` **as data** (§3.7).
    pub fn fold(&self, state: &Value, env: &Envelope, event: Value) -> Result<Value> {
        (self.fold_fn)(vec![state.clone(), env.to_value(event)])
            .map_err(|e| anyhow!("folding at seq {}: {e}", env.seq))
    }

    /// The per-session view. In Mode A this runs server-side and its output is diffed (§5.1).
    pub fn view(&self, state: &Value, actor: &str) -> Result<Html> {
        let out = (self.view_fn)(vec![state.clone(), session(actor)])
            .map_err(|e| anyhow!("{e}"))
            .context("rendering the view")?;
        match out {
            Value::Html(h) => Ok((*h).clone()),
            other => Err(anyhow!(
                "the view produced {} rather than Html",
                other.display()
            )),
        }
    }

    /// Mint an id at the edge. Never called inside a fold — the checker guarantees that.
    pub fn new_uuid(&self) -> Arc<str> {
        (self.uuid)()
    }

    /// Decode a command from the wire, against the program's own `Command` union.
    ///
    /// §3.5: "the client's entire write surface is `send(cmd)` into a typed `Command` union. There
    /// is no other mutation path — mass assignment and over-posting have no representation." That
    /// property is enforced here: a field the union does not declare is not decoded, it is
    /// rejected.
    pub fn decode_command(&self, json: &serde_json::Value) -> Result<Value> {
        let name = self.placed.roles.command_ty.con_name().unwrap_or("Command");
        let Some(beck_core::TyDecl::Union { variants, .. }) = self.placed.program.types.get(name)
        else {
            return Err(anyhow!("`{name}` is not a union"));
        };
        let tag = json
            .get("c")
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow!("a command needs a `c` tag naming its variant"))?;
        let variant = variants
            .iter()
            .find(|v| v.name.as_ref() == tag)
            .ok_or_else(|| anyhow!("`{tag}` is not a variant of `{name}`"))?;

        let mut fields = std::collections::BTreeMap::new();
        for (field, ty) in &variant.fields {
            let raw = json
                .get(field.as_ref())
                .ok_or_else(|| anyhow!("`{tag}` needs a `{field}`"))?;
            fields.insert(field.clone(), decode_field(raw, ty, &self.placed.program)?);
        }
        Ok(Value::Data {
            ty: Arc::from(name),
            variant: Some(variant.name.clone()),
            fields: Arc::new(fields),
        })
    }
}

/// Build the `Session` the program sees. Phase 1 carries the actor only — dev-mode identity, as
/// Phase 0 had; D6's OIDC relying party is Phase 3.
fn session(actor: &str) -> Value {
    Value::Data {
        ty: Arc::from("Session"),
        variant: None,
        fields: Arc::new(std::collections::BTreeMap::from([(
            Arc::from("actor"),
            Value::str_(actor),
        )])),
    }
}

fn decode_field(
    raw: &serde_json::Value,
    ty: &beck_core::Ty,
    program: &beck_core::Program,
) -> Result<Value> {
    use beck_core::Ty;
    let name = ty.con_name().unwrap_or("");
    // A newtype is transparent on the wire and nominal in the type system — the whole point of
    // "ids of different entities must not be interchangeable" (§3.1).
    if let Some(beck_core::TyDecl::Newtype { inner, .. }) = program.types.get(name) {
        let inner = decode_field(raw, inner, program)?;
        return Ok(Value::Data {
            ty: Arc::from(name),
            variant: None,
            fields: Arc::new(std::collections::BTreeMap::from([(
                Arc::from("value"),
                inner,
            )])),
        });
    }
    match name {
        Ty::STR => raw
            .as_str()
            .map(Value::str_)
            .ok_or_else(|| anyhow!("expected a string, got {raw}")),
        Ty::INT => raw
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| anyhow!("expected an integer, got {raw}")),
        Ty::BOOL => raw
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| anyhow!("expected a boolean, got {raw}")),
        other => Err(anyhow!("Phase 1 cannot decode `{other}` from the wire")),
    }
}

/// A `Core` value's shape, for `beck explain`.
pub fn describe(c: &Core) -> String {
    match &c.kind {
        CoreKind::Lam { params, .. } => format!("fn/{}", params.len()),
        CoreKind::Global(n) => n.to_string(),
        CoreKind::Prim { op, .. } => op.name().to_string(),
        _ => format!("{}", c.ty),
    }
}
