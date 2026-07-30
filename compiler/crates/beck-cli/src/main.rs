//! `beck` — one binary for the whole toolchain.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.6: "**One
//! binary** serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no separate
//! language server implementation to drift." Everything below goes through the same
//! [`beck_core::compile`], so a diagnostic the CLI prints is the diagnostic the editor will show.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use beck_core::Placed;
use beck_diag::{Diagnostics, SourceMap};
use clap::{Parser, Subcommand, ValueEnum};

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
    /// Typecheck, verify placement, and slice the signal graph.
    Check { file: PathBuf },
    /// Print a program in either surface. `beck fmt` on commit normalises to `.beck` (§2.2).
    Fmt {
        file: PathBuf,
        #[arg(long, value_enum, default_value_t = Surface::Py)]
        surface: Surface,
        /// Rewrite the file in place.
        #[arg(long)]
        write: bool,
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
    },
    /// Bring the program up on a local cluster — rung 3 (§6.6).
    Up {
        file: PathBuf,
        #[arg(long, default_value = "target/beck")]
        out: PathBuf,
        /// Emit and validate the manifests without touching a cluster.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum Explain {
    /// Where each definition runs, and why.
    Place { file: PathBuf },
    /// The command channel's content-derived operation id (§4.3).
    Wire { file: PathBuf },
    /// The signal graph, and what the splitter made of it.
    Flow { file: PathBuf },
    /// The infrastructure the program's effects imply (§6.5).
    Deploy { file: PathBuf },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "beck=info,beck_rt=info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Cmd::Check { file } => {
            let (placed, map, diags) = compile(&file)?;
            print!("{}", diags.render(&map));
            match placed {
                Some(p) => {
                    println!(
                        "ok: {} definitions, {} signals, wire id {}",
                        p.program.defs.len(),
                        p.program.signals.len(),
                        p.wire_id
                    );
                    Ok(())
                }
                None => bail!("{} diagnostic(s)", diags.len()),
            }
        }
        Cmd::Fmt {
            file,
            surface,
            write,
        } => fmt(&file, surface, write),
        Cmd::Ast { file, expanded } => ast(&file, expanded),
        Cmd::Explain { what } => explain(what),
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
        Cmd::Build { file, out } => {
            let placed = compiled(&file)?;
            let source = read(&file)?;
            let written = beck_infra::emit(&placed, &source, &out)?;
            for w in &written {
                println!("{}", w.display());
            }
            println!("{} files, wire id {}", written.len(), placed.wire_id);
            Ok(())
        }
        Cmd::Up { file, out, dry_run } => up(&file, &out, dry_run),
    }
}

fn read(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))
}

fn compile(file: &Path) -> Result<(Option<Placed>, SourceMap, Diagnostics)> {
    let src = read(file)?;
    let name = file.display().to_string();
    let mut map = SourceMap::new();
    let id = map.add(name.clone(), src.clone());
    let mut diags = Diagnostics::new();
    let placed = beck_core::compile(id, &name, &src, &mut diags);
    Ok((placed, map, diags))
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

fn explain(what: Explain) -> Result<()> {
    match what {
        Explain::Place { file } => {
            let placed = compiled(&file)?;
            println!("{:<20} {:<8} effects", "definition", "tier");
            for name in &placed.program.def_order {
                let d = &placed.program.defs[name];
                println!(
                    "{:<20} {:<8} {{{}}}",
                    d.name,
                    d.tier.name(),
                    d.effects
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            for s in &placed.program.signals {
                println!(
                    "{:<20} {:<8} {{{}}}   : {}",
                    s.name,
                    s.tier.name(),
                    s.effects
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", "),
                    s.ty
                );
            }
            println!(
                "\nunplaced (`any`) means pure, so it compiles to every tier that needs it — \
                 that duplication is the payoff, not waste."
            );
            Ok(())
        }
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
        Explain::Flow { file } => {
            let placed = compiled(&file)?;
            let r = &placed.roles;
            println!("{:<12} {}", "ingress", r.proposals_name);
            println!("{:<12} {}  (validate)", "events", r.events_name);
            println!("{:<12} {}  (durable fold)", "state", r.state_name);
            println!(
                "{:<12} {}  ({})",
                "page",
                r.page_name,
                if r.view_is_per_session {
                    "per-session view"
                } else {
                    "broadcast view"
                }
            );
            if !r.inlined.is_empty() {
                println!(
                    "\ninlined into the view: {}",
                    r.inlined
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                println!("(full recompute per event; Phase 3 makes these incremental)");
            }
            println!(
                "\none tier crossing: `{}` is @on(client) over state that is @on(data), so the \
                 edge is a single subscription carrying DOM patches.",
                r.page_name
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

#[tokio::main(flavor = "multi_thread")]
async fn run(file: &Path, addr: &str, store: Store, path: &Path, url: Option<&str>) -> Result<()> {
    let placed = compiled(file)?;
    let log = open_store(store, path, url).await?;
    let app = beck_rt::App::start(placed, log, beck_rt::AppConfig::default()).await?;
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        // Graceful drain: stop accepting, let the fold finish, exit. Everything already
        // acknowledged is already durable, so there is nothing to flush.
        let _ = tx.send(true);
    });

    eprintln!(
        "beck run — store {}, head {}, open http://{addr}/?actor=dev",
        app.store_kind(),
        app.head()
    );
    beck_rt::http::serve(app, addr.parse()?, rx).await
}

#[tokio::main(flavor = "current_thread")]
async fn replay(
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
    let runtime = beck_rt::Runtime::new(placed)?;
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

fn up(file: &Path, out: &Path, dry_run: bool) -> Result<()> {
    let placed = compiled(file)?;
    let source = read(file)?;
    let written = beck_infra::emit(&placed, &source, out)?;
    eprintln!("emitted {} files to {}", written.len(), out.display());
    if dry_run {
        eprintln!("--dry-run: not touching a cluster");
        return Ok(());
    }
    beck_infra::up(out)
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
