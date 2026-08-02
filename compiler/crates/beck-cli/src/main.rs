//! `beck` — one binary for the whole toolchain.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.6: "**One
//! binary** serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no separate
//! language server implementation to drift." Everything below goes through the same
//! [`beck_core::compile`], so a diagnostic the CLI prints is the diagnostic the editor will show.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};

mod bench;
use beck_core::Placed;
use beck_diag::{Diagnostics, SourceMap};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(name = "beck", version, about = "The Beck compiler and runtime")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Surface {
    /// The Python surface — the default, and the only one taught (§2.2).
    Py,
    /// The canonical S-expression surface: the spec's notation, and the macro author's dialect.
    Sexpr,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Store {
    /// Rung 0: an embedded append-only log. `beck run` needs no server (§6.6).
    Redb,
    /// The durable substrate above rung 0.
    Postgres,
    /// No disk at all — for tests and measurements.
    Memory,
}

#[derive(Subcommand)]
enum Cmd {
    /// Typecheck, infer and verify placement, and slice the signal graph.
    Check {
        file: PathBuf,
        /// Assert where something runs: `--assert-place page=client`. §3.4's assertability
        /// guardrail — a placement a test depends on should fail the build when it moves, not
        /// surface as a latency regression.
        #[arg(long = "assert-place", value_name = "NAME=TIER")]
        assert_place: Vec<String>,
        /// Write the solved placement to `beck.lock`.
        #[arg(long)]
        write_lock: bool,
        /// Fail if the solved placement differs from `beck.lock` — the CI form of §3.4's
        /// stability guardrail.
        #[arg(long)]
        locked: bool,
        /// Compare this module's contract against a previously released `.becki` and fail on a
        /// breaking change (§4.3). The check CI runs before a rolling deploy.
        #[arg(long, value_name = "PREVIOUS.becki")]
        wire_compat: Option<PathBuf>,
        /// Accept the breaking changes `--wire-compat` finds. §4.3's explicit marker: a breaking
        /// release is allowed, and saying so is the point.
        #[arg(long)]
        breaking: bool,
    },
    /// Print a program in either surface. `beck fmt` on commit normalises to `.beck` (§2.2).
    Fmt {
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = Surface::Py)]
        surface: Surface,
        /// Rewrite the file in place.
        #[arg(long)]
        write: bool,
    },
    /// Write the module's published signature — the `.becki` of §3.6.
    ///
    /// Generated, checked in, and reviewed like an .mli: it is what downstream modules compile
    /// against, and the file `beck check --wire-compat` compares releases of.
    Iface {
        file: PathBuf,
        /// Where to write it. Defaults to the source file with a `.becki` extension.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Print it instead of writing it.
        #[arg(long)]
        stdout: bool,
    },
    /// Dump the canonical AST — the notation macro authors work in (§2.7).
    Ast {
        file: PathBuf,
        /// Show the tree after macro expansion rather than as written.
        #[arg(long)]
        expanded: bool,
    },
    /// Interrogate a compiler decision (§4.7).
    Explain {
        #[command(subcommand)]
        what: Explain,
    },
    /// Every part of the program and the infrastructure it implies, and what depends on what.
    ///
    /// The same model the dashboard draws, as text — because a graph is more useful to a tool than
    /// to an eye, and the tool is usually the point.
    Graph {
        file: PathBuf,
        /// Emit the model as JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Include types, which are the noisiest and least often the question.
        #[arg(long)]
        types: bool,
    },
    /// What breaks if this changes — transitively, across code *and* infrastructure.
    Impact {
        file: PathBuf,
        /// A definition, a signal, a type, or a resource as `Kind/name`.
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Run the program's own `test` and `property` blocks (§21.2).
    ///
    /// A test is a log, a command and an expectation, so this needs no network, no database and no
    /// browser: the tiers are co-located and the roles are the ones the runtime drives.
    Test {
        file: PathBuf,
        /// Only run tests whose name contains this.
        #[arg(long, short)]
        filter: Option<String>,
        /// Say what was stubbed even when the test passed — §21.3 rule 1's hidden default,
        /// declaring itself.
        #[arg(long, short)]
        verbose: bool,
        /// Inputs per `property` block.
        #[arg(long, default_value_t = 100)]
        runs: u64,
    },
    /// Run the program in a single process — rung 0 of the parity ladder.
    Run {
        file: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
        #[arg(long, value_enum, default_value_t = Store::Redb)]
        store: Store,
        #[arg(long, default_value = "beck.log")]
        path: PathBuf,
        #[arg(long, env = "BECK_POSTGRES_URL")]
        url: Option<String>,
    },
    /// Fold a recorded log and report the state it produces (§3.7).
    Replay {
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = Store::Redb)]
        store: Store,
        #[arg(long, default_value = "beck.log")]
        path: PathBuf,
        #[arg(long, env = "BECK_POSTGRES_URL")]
        url: Option<String>,
        /// Ignore snapshots and fold from the first event — D3's genesis-replay discipline.
        #[arg(long)]
        genesis: bool,
        /// Fold twice and compare; also compare the snapshot path against a fold from genesis.
        #[arg(long)]
        verify: bool,
        /// Stop at this position.
        #[arg(long)]
        to: Option<u64>,
    },
    /// Emit the deployable system: the object graph and the image config (§4.1 stage 11).
    Build {
        file: PathBuf,
        #[arg(long, default_value = "target/beck")]
        out: PathBuf,
        /// Which deployment target to render for (§6.1's `Platform`): `kubernetes` or `compose`.
        #[arg(long, default_value = "kubernetes")]
        platform: String,
    },
    /// Measure the log against every substrate, so the store is a decision and not a habit.
    Bench {
        #[command(subcommand)]
        what: Bench,
    },
    /// Bring the program up on a local cluster or host — rung 2 or 3 (§6.6).
    Up {
        file: PathBuf,
        #[arg(long, default_value = "target/beck")]
        out: PathBuf,
        /// Emit and validate the manifests without touching a cluster.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "kubernetes")]
        platform: String,
    },
}

#[derive(Subcommand)]
enum Bench {
    /// Append, read and encode, against memory, redb and — with a URL — PostgreSQL.
    Log {
        /// The PostgreSQL log to include. Also read from `BECK_POSTGRES_URL`.
        #[arg(long, env = "BECK_POSTGRES_URL")]
        url: Option<String>,
        /// Where to put the temporary redb file.
        #[arg(long, default_value = "target/beck")]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum Explain {
    /// Where each definition runs, and why (§4.7).
    Place {
        file: PathBuf,
        /// One definition or signal, with its candidates and their costs.
        name: Option<String>,
    },
    /// The command channel's content-derived operation id (§4.3).
    Wire { file: PathBuf },
    /// The signal graph, and what the splitter made of it — or, given a type, everywhere that
    /// type reaches and everywhere it is refused (§4.7).
    Flow {
        file: PathBuf,
        /// A type name: `beck explain flow ApiKey`.
        ty: Option<String>,
    },
    /// Which views a dataflow plan could maintain by delta, and why the rest could not (§3.8).
    ///
    /// The analysis, not the engine: every view is a full recompute per event today, and the
    /// report says so before it says anything else.
    Incremental {
        file: PathBuf,
        /// One view, by the name `beck explain flow` gives it.
        view: Option<String>,
    },
    /// The infrastructure the program's effects imply (§6.5).
    Deploy { file: PathBuf },
}

mod capture;

fn main() -> Result<()> {
    // Two destinations for one set of records: the terminal, and the dashboard's ring.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "beck=info,beck_rt=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(capture::Capture)
        .init();

    let cli = Cli::parse();
    // Every command below may end up evaluating Beck code, and the evaluator spends host stack on
    // recursion that is not in tail position. It says how much it needs; this is where a `beck`
    // process supplies it, so that a deep program gets `beck-eval`'s diagnostic rather than the
    // process getting a SIGSEGV (`docs/28` §28.3).
    beck_eval::on_the_evaluator_stack(move || dispatch(cli))
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Cmd::Check {
            file,
            assert_place,
            write_lock,
            locked,
            wire_compat,
            breaking,
        } => check(
            &file,
            &assert_place,
            write_lock,
            locked,
            wire_compat.as_deref(),
            breaking,
        ),
        Cmd::Fmt {
            file,
            surface,
            write,
        } => fmt(&file, surface, write),
        Cmd::Ast { file, expanded } => ast(&file, expanded),
        Cmd::Iface { file, out, stdout } => iface(&file, out.as_deref(), stdout),
        Cmd::Explain { what } => explain(what),
        Cmd::Bench { what } => match what {
            Bench::Log { url, dir } => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(bench::run(url.as_deref(), &dir))
            }
        },
        Cmd::Test {
            file,
            filter,
            verbose,
            runs,
        } => test_cmd(&file, filter.as_deref(), verbose, runs),
        Cmd::Graph { file, json, types } => graph_cmd(&file, json, types),
        Cmd::Impact { file, name, json } => impact_cmd(&file, &name, json),
        Cmd::Run {
            file,
            addr,
            store,
            path,
            url,
        } => run(&file, &addr, store, &path, url.as_deref()),
        Cmd::Replay {
            file,
            store,
            path,
            url,
            genesis,
            verify,
            to,
        } => replay(&file, store, &path, url.as_deref(), genesis, verify, to),
        Cmd::Build {
            file,
            out,
            platform,
        } => {
            let platform = platform_named(&platform)?;
            let placed = compiled(&file)?;
            let source = read(&file)?;
            let written = beck_infra::emit_with(&placed, &source, &out, platform.as_ref())?;
            for w in &written {
                println!("{}", w.display());
            }
            println!("{} files, wire id {}", written.len(), placed.wire_id);
            Ok(())
        }
        Cmd::Up {
            file,
            out,
            dry_run,
            platform,
        } => up(&file, &out, dry_run, &platform),
    }
}

fn read(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))
}

/// The placement lock that sits beside a source file, if there is one.
///
/// Beside the *source*, not in the working directory: `beck.lock` records where a particular
/// program's code runs, and two programs in one directory do not share an answer.
fn lock_path(file: &Path) -> PathBuf {
    file.parent()
        .unwrap_or(Path::new("."))
        .join(beck_core::Lock::FILE)
}

fn read_lock(file: &Path) -> Option<beck_core::Lock> {
    let text = std::fs::read_to_string(lock_path(file)).ok()?;
    match beck_core::Lock::from_json(&text) {
        Some(l) => Some(l),
        None => {
            eprintln!(
                "warning: {} is not readable as a lock; solving without it",
                lock_path(file).display()
            );
            None
        }
    }
}

/// Resolve `import x` against the directory the root module lives in.
///
/// `x.becki` is the contract and `x.beck` is the code: the first is what downstream *checks*
/// against, the second what the link step needs (§3.6). Both are loaded when both exist, and the
/// interface wins for checking — otherwise a checked-in contract would be decorative.
struct Dir(PathBuf);

impl beck_core::project::Loader for Dir {
    fn load(&self, name: &str) -> Option<beck_core::Sources> {
        let path = self.0.join(format!("{name}.beck"));
        let module = std::fs::read_to_string(&path).ok();
        let interface = std::fs::read_to_string(self.0.join(format!("{name}.becki"))).ok();
        (module.is_some() || interface.is_some()).then_some(beck_core::Sources {
            module,
            interface,
            path: Some(path.display().to_string()),
        })
    }
}

fn module_name(file: &Path) -> String {
    beck_syntax::module_ident(&file.display().to_string())
}

/// Check and link, stopping before the slicer.
///
/// What `beck check` and `beck iface` need. A module with no merge point is a *library* — a policy
/// or a domain — and refusing to typecheck it because it is not a whole application would make
/// §3.6's separate compilation unusable for the modules it exists to serve.
fn checked_project(
    file: &Path,
) -> Result<(Option<beck_core::project::Project>, SourceMap, Diagnostics)> {
    let name = module_name(file);
    let mut map = SourceMap::new();
    let mut diags = Diagnostics::new();
    let lock = read_lock(file);
    let dir = Dir(file.parent().unwrap_or(Path::new(".")).to_path_buf());
    // The root is read from the path it was given, which need not be `<name>.beck` in the same
    // directory — `beck check /tmp/scratch.beck` has to work.
    let root_src = read(file)?;
    let root = Root {
        name: name.clone(),
        src: root_src,
        path: file.display().to_string(),
        dir,
    };
    let project =
        beck_core::project::check_project(&name, &root, lock.as_ref(), &mut map, &mut diags);
    Ok((project, map, diags))
}

/// The root file, plus the directory everything it imports comes from.
struct Root {
    name: String,
    src: String,
    /// The path as given, so the surface (`.beck` or `.sx`) and the diagnostics both name it.
    path: String,
    dir: Dir,
}

impl beck_core::project::Loader for Root {
    fn load(&self, name: &str) -> Option<beck_core::Sources> {
        if name == self.name {
            return Some(beck_core::Sources {
                module: Some(self.src.clone()),
                interface: None,
                path: Some(self.path.clone()),
            });
        }
        self.dir.load(name)
    }
}

fn compile(file: &Path) -> Result<(Option<Placed>, SourceMap, Diagnostics)> {
    let src = read(file)?;
    let name = file.display().to_string();
    let mut map = SourceMap::new();
    let id = map.add(name.clone(), src.clone());
    let mut diags = Diagnostics::new();
    let lock = read_lock(file);

    // A single-file program is the common case and stays the fast path — one parse, one check, no
    // directory walk. A program with imports goes through the project pipeline (§3.6).
    if beck_core::project::imports_of(id, &name, &src).is_empty() {
        let placed = beck_core::compile_with(id, &name, &src, lock.as_ref(), &mut diags);
        return Ok((placed, map, diags));
    }

    let root = Root {
        name: module_name(file),
        src: src.clone(),
        path: file.display().to_string(),
        dir: Dir(file.parent().unwrap_or(Path::new(".")).to_path_buf()),
    };
    let name = root.name.clone();
    let placed = beck_core::compile_project(&name, &root, lock.as_ref(), &mut map, &mut diags);
    Ok((placed, map, diags))
}

/// `beck iface` — write the module's published signature.
fn iface(file: &Path, out: Option<&Path>, to_stdout: bool) -> Result<()> {
    let (project, map, diags) = checked_project(file)?;
    print!("{}", diags.render(&map));
    let project = project.ok_or_else(|| anyhow::anyhow!("{} does not compile", file.display()))?;
    let text = project.interface.render();
    if to_stdout {
        print!("{text}");
        return Ok(());
    }
    let path = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| file.with_extension("becki"));
    std::fs::write(&path, &text)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

/// `beck check` — the whole front end, plus §3.4's stability and assertability guardrails.
#[allow(clippy::too_many_arguments)]
fn check(
    file: &Path,
    assertions: &[String],
    write_lock: bool,
    locked: bool,
    wire_compat: Option<&Path>,
    accept_breaking: bool,
) -> Result<()> {
    let (project, map, diags) = checked_project(file)?;
    print!("{}", diags.render(&map));
    let Some(project) = project else {
        bail!("{} diagnostic(s)", diags.len());
    };

    let placement = project.solution.clone();
    let interface = project.interface.clone();
    let (defs, signals) = (project.program.defs.len(), project.program.signals.len());

    // Slicing is the *application* question, and a module that is not one is still a module.
    let mut slicing = Diagnostics::new();
    match beck_core::project::slice(project, &mut slicing) {
        Some(p) => println!(
            "ok: {defs} definitions, {signals} signals, wire id {}",
            p.wire_id
        ),
        None if slicing
            .iter()
            .all(|d| beck_core::project::NOT_AN_APPLICATION.contains(&d.code)) =>
        {
            println!(
                "ok: {defs} definitions — a library. No merge point, so there is nothing to run; \n\
                 `beck iface` publishes what it offers."
            );
        }
        None => {
            print!("{}", slicing.render(&map));
            bail!("{} diagnostic(s)", slicing.len());
        }
    }

    // §3.4: "a one-line edit must not re-place unrelated code; previous solution persisted in
    // `beck.lock`, churn reported in CI".
    if !placement.churn.is_empty() {
        println!("\nplacement changed against {}:", beck_core::Lock::FILE);
        for (key, was, now) in &placement.churn {
            println!("  {key:<28} {} → {}", was.name(), now.name());
        }
        if locked {
            bail!(
                "--locked: {} placement(s) moved. Re-run with --write-lock if that is intended.",
                placement.churn.len()
            );
        }
        println!("(re-run with --write-lock to accept)");
    }

    // A tie is not an error — the tie-break is total and deterministic — but it *is* the place a
    // future edit could silently move code, so it is named rather than absorbed.
    for (key, tied) in &placement.ties {
        println!(
            "note: {key} could run on {} at the same cost; `@on(…)` or {} would pin it",
            tied.iter()
                .map(|t| t.name())
                .collect::<Vec<_>>()
                .join(" or "),
            beck_core::Lock::FILE
        );
    }

    let mut failed = Vec::new();
    for a in assertions {
        let Some((name, want)) = a.split_once('=') else {
            bail!("--assert-place takes NAME=TIER, got `{a}`");
        };
        let Some(want) = beck_core::Tier::parse(want.trim()) else {
            bail!("`{want}` is not a tier");
        };
        match placement.explanation(name.trim()) {
            Some(e) if e.chosen == want => {}
            Some(e) => failed.push(format!(
                "  {name}: expected {}, runs on {} — {}",
                want.name(),
                e.chosen.name(),
                e.because
            )),
            None => failed.push(format!("  {name}: no such definition or signal")),
        }
    }
    if !failed.is_empty() {
        println!("\nplacement assertions failed:");
        for f in &failed {
            println!("{f}");
        }
        bail!("{} placement assertion(s) failed", failed.len());
    }

    // §4.3: "runs in CI and fails on a breaking change without an explicit `@breaking` marker."
    if let Some(previous) = wire_compat {
        let text = read(previous)?;
        let mut pd = Diagnostics::new();
        let name = module_name(previous);
        let old = beck_core::Interface::parse(&name, &text, &mut pd);
        if pd.has_errors() {
            let mut pmap = SourceMap::new();
            pmap.add(previous.display().to_string(), text.clone());
            print!("{}", pd.render(&pmap));
            bail!("{} is not a readable interface", previous.display());
        }
        let changes = beck_core::compare(&old, &interface);
        println!("\nwire compatibility against {}", previous.display());
        if changes.is_empty() {
            println!("  no change to the contract");
        }
        for c in &changes {
            println!("  {c}");
            println!("      {}", c.because);
        }
        if beck_core::is_breaking(&changes) && !accept_breaking {
            bail!(
                "{} breaking change(s). An old client talking to a new server would fail. \
                 Pass --breaking to ship it anyway.",
                changes
                    .iter()
                    .filter(|c| c.severity == beck_core::compat::Severity::Breaking)
                    .count()
            );
        }
    }

    if write_lock {
        let path = lock_path(file);
        std::fs::write(&path, beck_core::Lock::of(&placement).to_json())?;
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

/// `beck explain place` — §4.7's derivation, not its conclusion.
fn explain_place(file: &Path, only: Option<&str>) -> Result<()> {
    use beck_core::cost::FORBIDDEN;

    let placed = compiled(file)?;
    let solution = &placed.placement;

    if let Some(name) = only {
        let Some(e) = solution.explanation(name) else {
            bail!("no `{name}` in this program");
        };
        println!("{}  →  {} tier\n", e.key.name(), e.chosen.name());
        println!(
            "  effects    : {}",
            if e.row.visible().is_empty() {
                "{}  (pure; placeable anywhere)".to_string()
            } else {
                format!("{}", e.row)
            }
        );
        let costs: Vec<String> = e
            .candidates
            .iter()
            .map(|(t, c)| {
                if *c >= FORBIDDEN {
                    format!("{} (cannot discharge this row)", t.name())
                } else {
                    format!("{} (cost {:.1})", t.name(), *c as f64 / 100.0)
                }
            })
            .collect();
        println!("  candidates : {}", costs.join(", "));
        println!("  chosen     : {}", e.chosen.name());
        println!("  because    : {}", e.because);
        println!(
            "\ncosts are whole-program: what this program would cost with `{}` on that tier and \n\
             everything else where it is. Solved {}.",
            e.key.name(),
            solution.method.name()
        );
        return Ok(());
    }

    println!("{:<20} {:<8} {:<10} effects", "name", "tier", "kind");
    for e in &solution.explanations {
        let kind = match &e.key {
            beck_core::Key::Def(_) => "definition",
            beck_core::Key::Signal(_) => "signal",
        };
        println!(
            "{:<20} {:<8} {:<10} {}",
            e.key.name(),
            e.chosen.name(),
            kind,
            e.row
        );
    }
    println!(
        "\nunplaced (`any`) means pure, so it compiles to every tier that needs it — that\n\
         duplication is the payoff, not waste. Solved {}; total cost {:.1}.\n\
         `beck explain place <file> <name>` shows one decision's candidates and their costs.",
        solution.method.name(),
        solution.total as f64 / 100.0
    );
    Ok(())
}

fn compiled(file: &Path) -> Result<Placed> {
    let (placed, map, diags) = compile(file)?;
    print!("{}", diags.render(&map));
    placed.ok_or_else(|| anyhow::anyhow!("{} does not compile", file.display()))
}

fn fmt(file: &Path, surface: Surface, write: bool) -> Result<()> {
    let src = read(file)?;
    let name = file.display().to_string();
    let mut map = SourceMap::new();
    let id = map.add(name.clone(), src.clone());
    let mut diags = Diagnostics::new();
    let node = beck_syntax::parse_file(id, &name, &src, &mut diags);
    if diags.has_errors() {
        print!("{}", diags.render(&map));
        bail!("cannot format a file that does not parse");
    }
    let out = match surface {
        Surface::Py => beck_syntax::print::to_python(&node),
        Surface::Sexpr => beck_syntax::print::to_sexpr_pretty(&node),
    };
    if write {
        std::fs::write(file, &out)?;
        eprintln!("formatted {}", file.display());
    } else {
        print!("{out}");
    }
    Ok(())
}

fn ast(file: &Path, expanded: bool) -> Result<()> {
    let src = read(file)?;
    let name = file.display().to_string();
    let mut map = SourceMap::new();
    let id = map.add(name.clone(), src.clone());
    let mut diags = Diagnostics::new();
    let node = beck_syntax::parse_file(id, &name, &src, &mut diags);
    let node = if expanded {
        beck_macro::expand_module(&node, &mut diags)
    } else {
        node
    };
    print!("{}", diags.render(&map));
    print!("{}", beck_syntax::print::to_sexpr_pretty(&node));
    Ok(())
}

/// `beck graph` — the whole model, grouped, with each part's direct dependencies.
///
/// Text by default and JSON on request, because the two audiences want different things: a person
/// scanning for the shape, and a program (or an agent reading a terminal) that needs the model as
/// data. Both come from the same [`beck_core::graph::DepGraph`], so neither can be stale relative
/// to the other or to what `beck build` emits.
fn graph_cmd(file: &Path, json: bool, types: bool) -> Result<()> {
    use beck_core::graph::NodeKind;

    let placed = compiled(file)?;
    let g = beck_infra::dependency_graph(&placed);

    if json {
        let nodes: Vec<_> = g
            .nodes()
            .map(|(id, n)| {
                serde_json::json!({
                    "name": n.name.as_ref(),
                    "kind": n.kind.as_str(),
                    "tier": format!("{:?}", n.tier).to_lowercase(),
                    "effects": n.effects.iter().map(|e| format!("{e:?}").to_lowercase()).collect::<Vec<_>>(),
                    "because": n.because,
                    "depends_on": g.dependencies(id).iter()
                        .map(|e| serde_json::json!({
                            "name": g.node(e.to).name.as_ref(), "kind": e.kind.as_str()
                        })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{:#}",
            serde_json::json!({
                "app": placed.program.name,
                "nodes": nodes,
                "cycles": g.cycles()
                    .map(|c| c.iter().map(|n| g.node(*n).name.as_ref()).collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }

    let cycles: Vec<Vec<&str>> = g
        .cycles()
        .map(|c| c.iter().map(|n| &*g.node(*n).name).collect())
        .collect();
    println!(
        "{} — {} nodes, {} edges, {} cycle{}\n",
        placed.program.name,
        g.len(),
        g.edge_count(),
        cycles.len(),
        if cycles.len() == 1 { "" } else { "s" }
    );

    let groups = [
        (NodeKind::Signal, "signals"),
        (NodeKind::Function, "functions"),
        (NodeKind::Resource, "resources"),
        (NodeKind::Type, "types"),
    ];
    for (kind, label) in groups {
        if kind == NodeKind::Type && !types {
            continue;
        }
        let members: Vec<_> = g.nodes().filter(|(_, n)| n.kind == kind).collect();
        if members.is_empty() {
            continue;
        }
        println!("{label}");
        for (id, n) in members {
            let mut tags = Vec::new();
            if n.tier != beck_core::Tier::Any {
                tags.push(format!("@on({})", format!("{:?}", n.tier).to_lowercase()));
            }
            tags.extend(n.effects.iter().map(|e| format!("{e:?}").to_lowercase()));
            let deps: Vec<&str> = g
                .dependencies(id)
                .iter()
                .map(|e| &*g.node(e.to).name)
                .collect();
            println!(
                "{}",
                format!("  {:<28}{}", n.name, tags.join(" ")).trim_end()
            );
            if !n.because.is_empty() {
                println!("  {:<28}{}", "", n.because);
            }
            if !deps.is_empty() {
                println!("  {:<28}← {}", "", deps.join(", "));
            }
        }
        println!();
    }

    if !cycles.is_empty() {
        println!("cycles");
        for c in &cycles {
            println!("  {}", c.join(" ⇄ "));
        }
        println!(
            "\na cycle in the signal graph is the architecture, not a fault: `events` is decided\n\
             from the state, and the state is folded from `events` (docs/03 §3.7)."
        );
    }
    Ok(())
}

/// `beck impact` — what breaks if this changes.
///
/// The question a person asks before an edit and a tool asks before a refactor, answered across the
/// whole stack: change a signal and the answer names the log store its `durable` implies, because
/// the code and the infrastructure are vertices in one graph.
fn impact_cmd(file: &Path, name: &str, json: bool) -> Result<()> {
    let placed = compiled(file)?;
    let g = beck_infra::dependency_graph(&placed);
    let Some(id) = g.id(name) else {
        let mut near: Vec<&str> = g
            .nodes()
            .map(|(_, n)| &*n.name)
            .filter(|n| n.to_lowercase().contains(&name.to_lowercase()))
            .collect();
        near.sort_unstable();
        bail!(
            "no `{name}` in this program{}",
            if near.is_empty() {
                String::new()
            } else {
                format!(". Did you mean: {}", near.join(", "))
            }
        );
    };

    let impact = g.impact(id);
    if json {
        println!(
            "{:#}",
            serde_json::json!({
                "of": name,
                "impacted": impact.iter().skip(1).map(|(n, d)| serde_json::json!({
                    "name": g.node(*n).name.as_ref(),
                    "kind": g.node(*n).kind.as_str(),
                    "hops": d,
                })).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }

    let n = impact.len() - 1;
    println!(
        "changing `{name}` affects {n} other thing{}\n",
        if n == 1 { "" } else { "s" }
    );
    for (node, hops) in impact.iter().skip(1) {
        let node = g.node(*node);
        println!(
            "  {:<10}{:<28}{}",
            node.kind.as_str(),
            node.name,
            if *hops == 1 {
                "directly".to_string()
            } else {
                format!("{hops} hops")
            }
        );
    }
    if n == 0 {
        println!("  nothing — it is a leaf");
    }
    Ok(())
}

fn explain(what: Explain) -> Result<()> {
    match what {
        Explain::Place { file, name } => explain_place(&file, name.as_deref()),
        Explain::Wire { file } => {
            let placed = compiled(&file)?;
            println!("operation id  {}", placed.wire_id);
            println!("command       {}", placed.roles.command_ty);
            println!("event         {}", placed.roles.event_ty);
            println!("state         {}", placed.roles.state_ty);
            println!(
                "\nthe id is content-derived from the module and those three types, so a body \
                 edit does not move it and a signature change does."
            );
            Ok(())
        }
        Explain::Flow { file, ty: Some(ty) } => {
            let placed = compiled(&file)?;
            let program = &placed.program;
            let Some(decl) = program.types.get(ty.as_str()) else {
                bail!("no type `{ty}` in this program");
            };
            let is_secret =
                beck_core::secure::sendable(&beck_core::Ty::con(&ty), &program.types).err();
            println!(
                "{ty} ({}) — {}",
                match decl {
                    beck_core::TyDecl::Model { .. } => "model",
                    beck_core::TyDecl::Union { .. } => "union",
                    beck_core::TyDecl::Newtype { .. } => "newtype",
                    beck_core::TyDecl::Alias { .. } => "alias",
                },
                match &is_secret {
                    Some(bad) => format!("not Sendable: {} at {}", bad.offender, bad.flow()),
                    None => "Sendable".to_string(),
                }
            );
            let reached = beck_core::secure::flow(program, &ty);
            if reached.is_empty() {
                println!("\n  reaches nothing — no signature mentions it");
                return Ok(());
            }
            println!();
            for r in &reached {
                match (&r.blocked, &is_secret) {
                    (Some(why), Some(_)) => {
                        println!("  BLOCKED: {:<18} {:<8} {why}", r.what, r.tier.name())
                    }
                    _ => println!("  reaches: {:<18} {:<8} ok", r.what, r.tier.name()),
                }
            }
            if is_secret.is_some() {
                println!(
                    "\na crossing requires Sendable, and `secret[T]` is deliberately not \
                     (docs/03 §3.5).\nWhat blocks the leak is the placement, so moving one of \
                     these to the client is the compile error."
                );
            }
            Ok(())
        }
        Explain::Flow { file, ty: None } => {
            let placed = compiled(&file)?;
            print!("{}", beck_core::split::flow_report(&placed));
            Ok(())
        }
        Explain::Incremental { file, view } => {
            let placed = compiled(&file)?;
            print!(
                "{}",
                beck_core::incremental::report(&placed, view.as_deref())
            );
            Ok(())
        }
        Explain::Deploy { file } => {
            let placed = compiled(&file)?;
            print!("{}", beck_infra::graph(&placed).explain());
            Ok(())
        }
    }
}

/// `beck run`, on a runtime whose worker threads have the evaluator's stack.
///
/// `#[tokio::main]` cannot say that, and the folds and views of a served program run on these
/// threads: a stack a worker did not have would take the server down rather than the request.
fn run(file: &Path, addr: &str, store: Store, path: &Path, url: Option<&str>) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(beck_eval::STACK_BYTES)
        .build()?
        .block_on(serve(file, addr, store, path, url))
}

async fn serve(
    file: &Path,
    addr: &str,
    store: Store,
    path: &Path,
    url: Option<&str>,
) -> Result<()> {
    let placed = compiled(file)?;
    // Built before the app starts and never rebuilt: the program cannot change under a running
    // process, so the dashboard's structural panes are computed once (docs/19 §19.8).
    let dashboard = Arc::new(dashboard(&placed));
    // The one place the process chooses how the program executes. A native backend is a different
    // expression here and nothing else (docs/19 §19.8).
    let backend = beck_eval::backend(&placed);
    let log = open_store(store, path, url).await?;
    let runtime = beck_rt::Runtime::new(placed, backend)?;
    let app = beck_rt::App::start(runtime, log, beck_rt::AppConfig::default()).await?;
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        // Graceful drain: stop accepting, let the fold finish, exit. Everything already
        // acknowledged is already durable, so there is nothing to flush.
        let _ = tx.send(true);
    });

    eprintln!(
        "beck run — store {}, head {}, open http://{addr}/?actor=dev\n\
         dashboard    http://{addr}/_beck",
        app.store_kind(),
        app.head()
    );
    beck_rt::http::serve_with_dashboard(app, addr.parse()?, rx, Some(dashboard)).await
}

/// `beck test` — §21.2's construct, run.
///
/// Note what this command does not take: no `--url`, no `--store`, no address. A test performs no
/// effects (`B0700` is a compile error) and its subject's effects are stubbed, so there is nothing
/// to point at anything.
fn test_cmd(file: &Path, filter: Option<&str>, verbose: bool, runs: u64) -> Result<()> {
    // Not `compiled`: a module with no merge point is a **library**, and a library's tests are the
    // ones a developer most wants to run (docs/22 §22.6, docs/25 §25.6 item 1, docs/27 §27.4).
    // `slice_or_library` gives one back instead of refusing it; every other diagnostic still does.
    let (project, map, mut diags) = checked_project(file)?;
    let placed = project.and_then(|p| beck_core::project::slice_or_library(p, &mut diags));
    print!("{}", diags.render(&map));
    let placed = placed.ok_or_else(|| anyhow::anyhow!("{} does not compile", file.display()))?;
    if placed.program.tests.is_empty() {
        eprintln!("no `test` or `property` blocks in {}", file.display());
        return Ok(());
    }
    let backend = beck_eval::backend(&placed);
    let opts = beck_rt::testing::Options {
        filter: filter.map(str::to_string),
        runs,
        base_dir: file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    let report = beck_rt::testing::run(&placed, backend, &opts);
    print!("{}", beck_rt::testing::render(&report, verbose));
    if !report.ok() {
        std::process::exit(1);
    }
    Ok(())
}

/// The dashboard's structural half: the dependency graph, and the resources the effects imply.
///
/// Both come from the same compile the runtime is about to execute, which is the whole claim —
/// there is no second description of the topology to keep in step.
fn dashboard(placed: &Placed) -> beck_rt::Dashboard {
    let infra = beck_infra::graph(placed);
    let rows = infra
        .nodes
        .iter()
        .map(|d| {
            let id = beck_infra::id_of(&d.node);
            let (kind, name) = id.split_once('/').unwrap_or(("", id.as_str()));
            beck_rt::ResourceRow {
                id: id.clone(),
                kind: kind.to_string(),
                name: name.to_string(),
                because: d.because.clone(),
                needs: d.needs.clone(),
                detail: detail_of(&d.node),
            }
        })
        .collect();
    beck_rt::Dashboard::new(placed, &beck_infra::dependency_graph(placed), rows)
}

/// The one line of an object that an operator wants without opening the manifest.
fn detail_of(n: &beck_infra::Node) -> String {
    use beck_infra::Node::*;
    match n {
        Image { entrypoint, .. } => entrypoint.clone(),
        Workload {
            replicas,
            serves_ui,
            ..
        } => {
            format!(
                "{replicas} replica, {}",
                if *serves_ui { "serves the ui" } else { "no ui" }
            )
        }
        Route {
            host, websocket, ..
        } => {
            format!("{host}{}", if *websocket { " (websocket)" } else { "" })
        }
        Service { port, headless, .. } => {
            format!(":{port}{}", if *headless { " headless" } else { "" })
        }
        LogStore { volume_gb, .. } => format!("{volume_gb} GiB volume"),
        SnapshotSchedule { every_events, .. } => format!("every {every_events} events"),
        Secret { keys, .. } => keys.join(", "),
        Policy {
            allow_ingress_from,
            allow_egress_to,
            allow_egress_hosts,
            ..
        } => format!(
            "ingress from [{}], egress to [{}]",
            allow_ingress_from.join(", "),
            allow_egress_to
                .iter()
                .map(|p| format!("{}:{}", p.app, p.port))
                .chain(allow_egress_hosts.iter().cloned())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Grant {
            role,
            on,
            privileges,
        } => format!("{role} on {on}: {}", privileges.join(", ")),
        Namespace { .. } => String::new(),
    }
}

fn replay(
    file: &Path,
    store: Store,
    path: &Path,
    url: Option<&str>,
    genesis: bool,
    verify: bool,
    to: Option<u64>,
) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(replay_inner(file, store, path, url, genesis, verify, to))
}

/// The caller is already on the evaluator's stack (`main`), and a current-thread runtime folds the
/// log on that same thread, so this one needs no size of its own.
async fn replay_inner(
    file: &Path,
    store: Store,
    path: &Path,
    url: Option<&str>,
    genesis: bool,
    verify: bool,
    to: Option<u64>,
) -> Result<()> {
    let placed = compiled(file)?;
    let log = open_store(store, path, url).await?;
    let backend = beck_eval::backend(&placed);
    let runtime = beck_rt::Runtime::new(placed, backend)?;
    let head = log.head().await?;
    let target = to.unwrap_or(head);

    let started = std::time::Instant::now();
    let (state, at) = if genesis {
        beck_rt::replay_from_genesis(&runtime, log.as_ref()).await?
    } else {
        beck_rt::replay_to(&runtime, log.as_ref(), target).await?
    };
    let elapsed = started.elapsed();

    println!("store              {}", log.kind());
    println!("head               {head}");
    println!("folded to          {at}");
    println!("state digest       {}", hex(&beck_core::digest(&state)));
    println!(
        "fold               {:.3} s ({:.0} events/s)",
        elapsed.as_secs_f64(),
        at as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
    );

    if verify {
        // Fold twice and compare, then compare the snapshot path against a fold from genesis.
        // Snapshots are an optimisation; one that disagrees with the log is a bug we want CI to
        // find, not a fact to trust (D3).
        let (again, _) = beck_rt::replay_to(&runtime, log.as_ref(), target).await?;
        let (from_genesis, _) = beck_rt::replay_from_genesis(&runtime, log.as_ref()).await?;
        let d1 = beck_core::digest(&state);
        let d2 = beck_core::digest(&again);
        let d3 = beck_core::digest(&from_genesis);
        if d1 != d2 {
            bail!("replay is not deterministic: two folds of the same log disagree");
        }
        if head == target && d1 != d3 {
            bail!("the snapshot path disagrees with a fold from genesis");
        }
        println!("\nreplay is exact: two folds agree, and the snapshot path agrees with genesis.");
    }
    Ok(())
}

/// Resolve `--platform`, listing what there is when the name is wrong.
///
/// A closed `ValueEnum` would be tidier and would put the list of platforms in two places. It is
/// one place — `beck_infra::platform::all()` — so a new `Platform` implementation is reachable from
/// the command line without editing the CLI, which is most of the point of the trait.
fn platform_named(name: &str) -> Result<Box<dyn beck_infra::Platform>> {
    beck_infra::platform::by_name(name).ok_or_else(|| {
        anyhow!(
            "unknown platform `{name}`. Known: {}",
            beck_infra::platform::all()
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

fn up(file: &Path, out: &Path, dry_run: bool, platform: &str) -> Result<()> {
    let platform = platform_named(platform)?;
    let placed = compiled(file)?;
    let source = read(file)?;
    let written = beck_infra::emit_with(&placed, &source, out, platform.as_ref())?;
    eprintln!(
        "emitted {} files to {} for `{}`",
        written.len(),
        out.display(),
        platform.name()
    );
    // Whatever this platform cannot express is said before anything is applied, not after.
    for (what, why) in platform.unsupported(&beck_infra::graph(&placed)) {
        eprintln!(
            "  note: {what} is not expressible on `{}` — {why}",
            platform.name()
        );
    }
    if dry_run {
        eprintln!("--dry-run: not touching a target");
        return Ok(());
    }
    beck_infra::up_with(out, platform.as_ref())
}

async fn open_store(
    store: Store,
    path: &Path,
    url: Option<&str>,
) -> Result<Arc<dyn beck_rt::LogStore>> {
    Ok(match store {
        Store::Memory => Arc::new(beck_rt::MemoryLog::new()),
        Store::Redb => Arc::new(beck_rt::RedbLog::open(path)?),
        Store::Postgres => {
            let url = url.context("--url or BECK_POSTGRES_URL is required for --store postgres")?;
            Arc::new(beck_rt::PgLog::connect(url).await?)
        }
    })
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
