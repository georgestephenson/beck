//! `beck` — one binary for the whole toolchain.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.6: "**One
//! binary** serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no separate
//! language server implementation to drift." Everything below goes through the same
//! [`beck_core::compile`], so a diagnostic the CLI prints is the diagnostic the editor will show.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The evaluator is an allocator benchmark wearing a language's clothes: a call allocates its
/// frame, a `let` allocates its scope, and both are freed a few microseconds later. Profiling
/// `awfy/json.beck` put a third of every instruction the process executed inside glibc's
/// `malloc` and `free`. `docs/adr/0019` is the decision and the measurement.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{anyhow, bail, Context, Result};

mod bench;
mod fetch;
mod image;
use beck_core::Placed;
use beck_diag::{Diagnostics, SourceMap};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The number identifies the release; the commit and the triple identify which of its artefacts
/// this is (`docs/28-releases-and-deployment.md` §28.2). `build.rs` supplies the last two, and both
/// read `unknown` when built from a source tree with no git.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("BECK_COMMIT"),
    " ",
    env!("BECK_TARGET"),
    ")"
);

#[derive(Parser)]
#[command(name = "beck", version = VERSION, about = "The Beck compiler and runtime")]
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
    /// A single file, and the same engine a read model would be projected into (§7.8.1).
    Sqlite,
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
        /// Evaluation steps one expectation may take before it is stopped.
        ///
        /// The default is a runaway-program backstop and is right for everything written by hand.
        /// A *benchmark* is the exception — three of the fourteen in `awfy/` need more at the size
        /// their suite measures at (`docs/53` §53.3), and a backstop nothing can raise is a ceiling.
        #[arg(long, default_value_t = beck_eval::DEFAULT_FUEL)]
        fuel: u64,
        /// Write what `expect page matches snapshot` renders, instead of comparing against it.
        ///
        /// The written file is reviewed like any other diff (§21.2). Nothing writes a snapshot
        /// without this flag: one that rewrote itself on disagreement would assert nothing.
        #[arg(long)]
        update: bool,
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
        /// Also serve the read models on the PostgreSQL wire protocol (§5.3).
        ///
        /// Off by default, and loopback only: the port answers every question about the
        /// application's state, and it has no authentication and no transport security. Forward it
        /// rather than exposing it.
        #[arg(long, value_name = "ADDR")]
        pgwire: Option<String>,
        /// This application, as its identity provider knows it.
        ///
        /// The **issuer** is not a flag: a program says who authenticates its clients with
        /// `identity = external(issuer="https://…")`, because §6.5 derives the cluster's egress
        /// rule from the peers a program names and a flag is not one of them. A client id is a
        /// deployment fact — staging and production register different ones — so it is here.
        #[arg(long, value_name = "ID")]
        client_id: Option<String>,
        /// The client secret, for a confidential client. Read from the environment because a
        /// secret on a command line is a secret in the process table.
        #[arg(
            long,
            env = "BECK_CLIENT_SECRET",
            value_name = "SECRET",
            hide_env_values = true
        )]
        client_secret: Option<String>,
        /// Where the issuer sends the browser back — the redirect URI registered with it.
        ///
        /// Defaults to `http://<addr>/auth/callback`, which is what a laptop needs and what a
        /// deployment behind a gateway must override, because the address this process bound is
        /// not the address a browser typed.
        #[arg(long, value_name = "URL")]
        redirect_uri: Option<String>,
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
    /// Compile what can be compiled to native code, and say what could not (§5.2).
    ///
    /// §5.2's dual codegen, over the scalar subset of the language: a definition whose parameters
    /// and result are `Int`, `Float` or `Bool` and whose body is arithmetic, comparison, `if`,
    /// `match` and direct calls. Everything else — anything that needs a heap, and every effect —
    /// stays with the evaluator, and this prints which went which way.
    ///
    /// `--backend llvm` (the default) needs `clang` on the path, or `BECK_CLANG` pointing at one.
    /// `--backend cranelift` needs only a linker, because Cranelift is a crate.
    Native {
        file: PathBuf,
        /// Which code generator: `llvm` for release code, `cranelift` for a fast build (§7.3).
        #[arg(long, default_value = "llvm")]
        backend: String,
        /// Keep the generated IR and the executable here instead of in a temporary directory.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Call a compiled definition and print what it answered.
        ///
        /// `beck native fib.beck --call fib --arg 30`. An argument is read as an `Int` if it looks
        /// like one and a `Float` otherwise; `true` and `false` are `Bool`s.
        #[arg(long)]
        call: Option<String>,
        #[arg(long = "arg")]
        args: Vec<String>,
    },
    /// Write a Mode B component's bundle — the slice a browser downloads (§5.1, `docs/94` §94.4).
    ///
    /// This is not how a deployment gets one. `beck run` derives the bundle from the program it is
    /// executing, so a served slice cannot be of a different program than the running one, and
    /// `beck build` deliberately writes no bundle for the same reason. This is how a *measurement*
    /// gets one: §5.1 budgets "< 150 KB brotli for a typical Mode-B component bundle", and a budget
    /// nothing can weigh is not a budget.
    ///
    /// A Mode A component has no bundle, and asking for one is an error rather than an empty file.
    Bundle {
        file: PathBuf,
        /// Where to write it.
        #[arg(long, short)]
        out: PathBuf,
    },
    /// The bill of materials for what `beck build` emits, as CycloneDX 1.6 JSON.
    ///
    /// Derived from the same object graph the image config is, so the two cannot disagree about
    /// what is in the image. `beck build` writes one beside the manifests; this prints it.
    Sbom {
        file: PathBuf,
        /// Write it here instead of to standard output.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Generate reference documentation — for a module, or for the language itself.
    ///
    /// A module's page is derived from the module: signatures come from inference, effects from
    /// the row, placement from the solver, and prose from `##` doc comments. The language
    /// reference is derived from the compiler's own tables — the diagnostic codes it can emit, the
    /// predicate the placement solver evaluates, the schemes inference reads for the prelude, and
    /// this command tree.
    Doc {
        #[command(subcommand)]
        what: Doc,
    },
    /// Measure the log against every substrate, so the store is a decision and not a habit.
    Bench {
        #[command(subcommand)]
        what: Bench,
    },
    /// Serve the Language Server Protocol on stdin and stdout (§4.6).
    ///
    /// The same front end `beck check` runs, so an editor's squiggle and a CI failure are the same
    /// diagnostic — §4.6's "there is no separate language server implementation to drift". Not
    /// meant to be run by hand: an editor starts it and speaks JSON-RPC to it.
    Lsp,
    /// Build the container image, in this process — no apko, no melange, no daemon (§6.2).
    ///
    /// Resolves the packages `beck build`'s apko config names, fetches them from the Wolfi
    /// repository, unpacks them, adds the toolchain and the program, and writes an OCI image
    /// layout. The result is reproducible: the same inputs produce the same digest, which is the
    /// property §6.2 chose this image format for and which `beck image` run twice will show.
    Image {
        file: PathBuf,
        #[arg(long, default_value = "target/beck/image")]
        out: PathBuf,
        /// What to tag the image in the layout's index.
        #[arg(long, default_value = "dev")]
        tag: String,
        /// The architecture to build for, as the package repository names it.
        #[arg(long, default_value = "x86_64")]
        arch: String,
        /// Where the packages come from.
        #[arg(long, default_value = beck_infra::sbom::REPOSITORY)]
        repository: String,
        /// Where fetched packages are kept. A second build reads them from here.
        #[arg(long, default_value = "target/beck/packages")]
        cache: PathBuf,
        /// Build from the cache alone, and fail rather than reach the network.
        #[arg(long)]
        offline: bool,
        /// The toolchain binary the image ships. Defaults to the running one.
        #[arg(long, value_name = "PATH")]
        binary: Option<PathBuf>,
        /// Sign the image as it is built, with this key.
        #[arg(long, value_name = "KEY.pem")]
        sign: Option<PathBuf>,
    },
    /// Sign the image in an OCI layout, in the form `cosign verify` reads (§6.2).
    Sign {
        /// The layout `beck image` wrote.
        layout: PathBuf,
        /// The private key. Read from `BECK_SIGNING_KEY` when this is not given.
        #[arg(long, value_name = "KEY.pem")]
        key: Option<PathBuf>,
    },
    /// Check a layout's signature against a public key — and that it is over *this* image.
    Verify {
        layout: PathBuf,
        #[arg(long, value_name = "COSIGN.pub")]
        key: PathBuf,
    },
    /// A signing key, and the public half a consumer verifies with.
    Key {
        #[command(subcommand)]
        what: Key,
    },
    /// Write the files a repository needs around a Beck program.
    Init {
        #[command(subcommand)]
        what: Init,
    },
    /// Serve the playground: the compiler and a running application, in a browser tab (§17).
    ///
    /// Rung A is the compiler compiled to WebAssembly — diagnostics, the two surfaces, inferred
    /// placement, the dataflow plan, the read model, the generated Kubernetes objects — with no
    /// server involved at all. Rung B runs the program *in the tab*: a log, a fold and two client
    /// subscriptions in one page, speaking the patch protocol over a `MessageChannel`.
    ///
    /// Needs the module: `cargo build -p beck-play --release --target wasm32-unknown-unknown`, or
    /// `BECK_PLAYGROUND` pointing at one.
    Play {
        #[arg(long, default_value = "127.0.0.1:8081")]
        addr: String,
        /// Write the playground to a directory instead of serving it.
        ///
        /// The result is the whole deployment — §17.1's "costs a CDN" is not a figure of speech,
        /// and this is what makes it checkable: a directory, on any static host.
        #[arg(long, short)]
        out: Option<PathBuf>,
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
enum Key {
    /// Write a new P-256 key pair: `<name>.key` and `<name>.pub`.
    Generate {
        /// The name to write, without an extension.
        #[arg(long, short, default_value = "cosign")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum Init {
    /// The continuous-integration workflow for this program (§28.3).
    ///
    /// Check, test, wire-compat, build — then, from the default branch, an image and a signature.
    /// Written to `.github/workflows/beck.yml`, reviewed and committed like any other file.
    Ci {
        file: PathBuf,
        /// The repository root to write into.
        #[arg(long, short, default_value = ".")]
        out: PathBuf,
        /// Print it instead of writing it.
        #[arg(long)]
        stdout: bool,
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
enum Doc {
    /// One module's reference page: every published name, with its signature, effects and tier.
    Module {
        file: PathBuf,
        /// Where to write it. One file per module, named after the module.
        #[arg(long, short, default_value = "doc")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = docs::Format::Html)]
        format: docs::Format,
        /// Link the page back to the repository it was generated from. HTML only.
        #[arg(long, value_name = "URL")]
        repo: Option<String>,
        /// Print it instead of writing it.
        #[arg(long)]
        stdout: bool,
    },
    /// A written guide, rendered for the published site.
    ///
    /// The reference is derived from the compiler; a guide is written by a person and *checked* by
    /// a harness — every program in `docs/86-getting-started.md` is compiled and run by
    /// `beck-cli/tests/getting_started.rs`. This is what puts the checked file on the site instead
    /// of a second copy of it that nothing compiles.
    Guide {
        /// The markdown file.
        file: PathBuf,
        #[arg(long, short, default_value = "site/guide")]
        out: PathBuf,
        /// Rewrite relative links against this URL — the *directory* the guide lives in, in the
        /// repository it is published from.
        ///
        /// Without it they are left as written, which is right for reading the file in place and
        /// wrong for a static site, where `08-roadmap.md` is not a page.
        #[arg(long, value_name = "URL")]
        link_base: Option<String>,
        /// Link the page back to the repository it was generated from.
        #[arg(long, value_name = "URL")]
        repo: Option<String>,
        /// Print it instead of writing it.
        #[arg(long)]
        stdout: bool,
    },
    /// The language reference: the error index, the command reference, the effect and tier
    /// matrix, the prelude, and the forms.
    ///
    /// Checked in under `docs/reference/` and regenerated by this command, so a change to the
    /// compiler that changes the reference shows up in the diff rather than in a stale page.
    Reference {
        #[arg(long, short, default_value = "../docs/reference")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = docs::Format::Md)]
        format: docs::Format,
        /// Link every generated page back to the repository it was generated from. HTML only.
        #[arg(long, value_name = "URL")]
        repo: Option<String>,
        /// Regenerate in memory and fail if what is on disk differs. The gate CI runs.
        #[arg(long)]
        check: bool,
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
    /// Where a component renders, why, and what that puts on the wire (§5.1).
    ///
    /// Mode A sends the browser a rendering of the state; Mode B sends it the state and renders
    /// locally. This prints which one, what decided it, what crosses, whether the client may apply
    /// a command optimistically — and, for a Mode B component, the size of the bundle it would
    /// have to download.
    Render { file: PathBuf },
    /// The signal graph, and what the splitter made of it — or, given a type, everywhere that
    /// type reaches and everywhere it is refused (§4.7).
    Flow {
        file: PathBuf,
        /// A type name: `beck explain flow ApiKey`.
        ty: Option<String>,
    },
    /// Which views a dataflow plan could maintain by delta, and why the rest could not (§3.8).
    ///
    /// The analysis rather than the plan: it asks whether a *view* is built only from operations
    /// with delta rules, and `beck explain query` prints the operators the view actually compiles
    /// to. A view this reports as `recompute` may still have its collections maintained around
    /// whatever blocked it.
    Incremental {
        file: PathBuf,
        /// One view, by the name `beck explain flow` gives it.
        view: Option<String>,
    },
    /// The view as a dataflow plan, and what query fusion made of it (§4.7, §5.3).
    ///
    /// Every operator, what it reads, what orders its arrangement and which side of the session
    /// cut it is on — then the rewrites that fired, and the ones that matched a rule and were
    /// refused, with the condition that refused each.
    Query {
        file: PathBuf,
        /// The plan as the decomposition produced it, before any rewrite — one operator per
        /// construct the source names, which is what the rules are applied to.
        #[arg(long)]
        unfused: bool,
    },
    /// What one event costs this program's view, operator by operator (§4.7).
    ///
    /// In the engine's own units — applications, entries touched, entries copied, operators
    /// recomputed — as a function of the change `δ` and the collection `n`, so the answer is the
    /// same on every machine.
    Cost { file: PathBuf },
    /// The read model: what an outside SQL client sees, as `create table` (§5.3).
    ///
    /// Nothing executes this DDL. There is no table to create — a read model is the collection the
    /// fold already holds and the arrangement the view engine already maintains, projected — so
    /// this is the shape of the relations `beck run --pgwire` serves rather than a migration.
    Sql { file: PathBuf },
    /// The infrastructure the program's effects imply (§6.5).
    Deploy { file: PathBuf },
    /// What a diagnostic code means — `beck explain error B0341`.
    Error {
        /// The code, as it appears in the diagnostic: `B0341`.
        code: String,
    },
}

mod capture;
mod docs;
mod lsp;

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
    // process getting a SIGSEGV (`docs/27` §27.2).
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
        Cmd::Doc { what } => match what {
            Doc::Module {
                file,
                out,
                format,
                repo,
                stdout,
            } => {
                // Through the project loader, not a single-file read: a module that imports another
                // has to be documented with that other module in scope. The *page* is the root
                // module's own interface rather than the sliced program, which is every module
                // merged (`docs/56` §56.5).
                let (project, map, diags) = checked_project(&file)?;
                eprint!("{}", diags.render(&map));
                let project = project.filter(|_| !diags.has_errors()).ok_or_else(|| {
                    anyhow::anyhow!(
                        "{} does not compile, so it cannot be documented",
                        file.display()
                    )
                })?;
                docs::module(&project, Some(&out), format, stdout, repo.as_deref())
            }
            Doc::Guide {
                file,
                out,
                link_base,
                repo,
                stdout,
            } => docs::guide(&file, &out, link_base.as_deref(), stdout, repo.as_deref()),
            Doc::Reference {
                out,
                format,
                repo,
                check,
            } => docs::reference(&out, format, check, repo.as_deref()),
        },
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
            fuel,
            update,
        } => test_cmd(&file, filter.as_deref(), verbose, runs, fuel, update),
        Cmd::Lsp => lsp::serve(),
        Cmd::Graph { file, json, types } => graph_cmd(&file, json, types),
        Cmd::Impact { file, name, json } => impact_cmd(&file, &name, json),
        Cmd::Run {
            file,
            addr,
            store,
            path,
            url,
            pgwire,
            client_id,
            client_secret,
            redirect_uri,
        } => run(
            &file,
            &addr,
            store,
            &path,
            url.as_deref(),
            pgwire.as_deref(),
            Auth {
                client_id,
                client_secret,
                redirect_uri,
            },
        ),
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
            // A Mode B component needs one artefact these manifests do not describe. The bundle
            // itself is *not* written: the server derives it from the program it is running, so a
            // deployment cannot serve a slice of a different program than the one it is executing.
            if placed.render.mode == beck_core::render::Mode::Client {
                let bundle = beck_core::Bundle::of(&placed);
                println!(
                    "`{}` renders on the client: this deployment also needs the Mode B kernel \n\
                     (`cargo build -p beck-wasm --release --target wasm32-unknown-unknown`, served \n\
                     from BECK_KERNEL). Its bundle is {} bytes and is derived at request time.",
                    bundle.component,
                    bundle.to_bytes().len()
                );
            }
            Ok(())
        }
        Cmd::Native {
            file,
            backend,
            out,
            call,
            args,
        } => native(&file, &backend, out.as_deref(), call.as_deref(), &args),
        Cmd::Bundle { file, out } => {
            let placed = compiled(&file)?;
            if placed.render.mode != beck_core::render::Mode::Client {
                anyhow::bail!(
                    "`{}` renders on the server, so there is no bundle: Mode A sends the browser a \
                     rendering of the state and ships it no application code at all. \
                     `beck explain render {}` prints what would move it.",
                    placed.render.component,
                    file.display()
                );
            }
            let bundle = beck_core::Bundle::of(&placed);
            let bytes = bundle.to_bytes();
            std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            println!(
                "{}: {} bytes, {} definitions, component `{}`",
                out.display(),
                bytes.len(),
                bundle.defs.len(),
                bundle.component
            );
            Ok(())
        }
        Cmd::Sbom { file, out } => {
            let placed = compiled(&file)?;
            let source = read(&file)?;
            let graph = beck_infra::graph(&placed);
            let body = beck_infra::sbom::render(&graph, &source, &placed.wire_id);
            match out {
                Some(path) => {
                    std::fs::write(&path, body)
                        .with_context(|| format!("writing {}", path.display()))?;
                    println!("{}", path.display());
                }
                None => print!("{body}"),
            }
            Ok(())
        }
        Cmd::Image {
            file,
            out,
            tag,
            arch,
            repository,
            cache,
            offline,
            binary,
            sign,
        } => {
            let placed = compiled(&file)?;
            let source = read(&file)?;
            image::build(
                &placed,
                &source,
                &image::Options {
                    out: &out,
                    tag: &tag,
                    arch: &arch,
                    repository: &repository,
                    cache: &cache,
                    offline,
                    binary: binary.as_deref(),
                    sign_with: sign.as_deref(),
                },
            )
        }
        Cmd::Sign { layout, key } => image::attach_signature(&layout, key.as_deref()),
        Cmd::Verify { layout, key } => image::verify(&layout, &key),
        Cmd::Key {
            what: Key::Generate { out },
        } => image::generate_key(&out),
        Cmd::Init {
            what: Init::Ci { file, out, stdout },
        } => {
            let placed = compiled(&file)?;
            let graph = beck_infra::graph(&placed);
            // The path as the repository sees it, which is what every step in the workflow passes:
            // a workflow that named an absolute path from the machine that generated it would run
            // nowhere else.
            let app = file
                .strip_prefix(&out)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            let body = beck_infra::ci::workflow(&graph, &app);
            if stdout {
                print!("{body}");
                return Ok(());
            }
            let path = out.join(beck_infra::ci::WORKFLOW_PATH);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
            println!("{}", path.display());
            Ok(())
        }
        Cmd::Up {
            file,
            out,
            dry_run,
            platform,
        } => up(&file, &out, dry_run, &platform),
        Cmd::Play { addr, out } => play(&addr, out.as_deref()),
    }
}

/// `beck play` — the playground, served or written out (§17.1, §17.2).
fn play(addr: &str, out: Option<&Path>) -> Result<()> {
    if let Some(dir) = out {
        let written = beck_play::serve::write(dir)?;
        for path in &written {
            println!("{}", path.display());
        }
        eprintln!(
            "\n{} files. Serve the directory with anything that serves files — the playground \n\
             needs no server of its own, which is the whole of rung A (docs/17 §17.1).",
            written.len()
        );
        return Ok(());
    }

    let addr: std::net::SocketAddr = addr.parse().context("--addr")?;
    let module = beck_play::serve::module_path();
    if !module.is_file() {
        bail!(
            "no playground module at {}: build it with `cargo build -p beck-play --release \
             --target wasm32-unknown-unknown`, or set BECK_PLAYGROUND",
            module.display()
        );
    }
    eprintln!("the playground is at http://{addr}");
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(beck_eval::STACK_BYTES)
        .build()?
        .block_on(async move {
            let (_tx, rx) = tokio::sync::watch::channel(false);
            beck_play::serve::serve(addr, rx).await
        })
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
            // Say *which* of the three made it a library. The message used to name the merge point
            // whichever code had fired, so a module that had one and lacked a client signal was
            // told to add a line it already had — which is the state every program passes through
            // while it is being written, and the first thing `docs/86` found.
            let why = if slicing.iter().any(|d| d.code == "B0500") {
                "no merge point"
            } else if slicing.iter().any(|d| d.code == "B0501") {
                "no durable state"
            } else {
                "no signal placed on the client"
            };
            println!(
                "ok: {defs} definitions — a library: {why}, so there is nothing to run; \n\
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
        let mut pmap = SourceMap::new();
        let name = module_name(previous);
        let old = beck_core::Interface::parse(&name, &text, &mut pmap, &mut pd);
        if pd.has_errors() {
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

/// One of the two code generators, with the four questions `beck native` asks of either.
///
/// An enum rather than a trait: the two crates are deliberately independent — neither depends on
/// the other's `Artifact` — and a trait here would be a third place where "what a native backend
/// offers" is written down.
enum Compiled {
    Llvm(beck_llvm::Artifact),
    Clif(beck_clif::Artifact),
}

impl Compiled {
    fn report(&self) -> String {
        match self {
            Compiled::Llvm(a) => a.report().to_string(),
            Compiled::Clif(a) => a.report().to_string(),
        }
    }

    fn ir(&self) -> &Path {
        match self {
            Compiled::Llvm(a) => a.ir_path(),
            Compiled::Clif(a) => a.ir_path(),
        }
    }

    fn exe(&self) -> &Path {
        match self {
            Compiled::Llvm(a) => a.executable(),
            Compiled::Clif(a) => a.executable(),
        }
    }

    fn signature(&self, name: &str) -> Option<&beck_llvm::Signature> {
        match self {
            Compiled::Llvm(a) => a.module().signature(name),
            Compiled::Clif(a) => a.module().signature(name),
        }
    }

    fn call(&self, name: &str, args: &[beck_core::Value]) -> Result<beck_core::Value, String> {
        match self {
            Compiled::Llvm(a) => a.call(name, args).map_err(|e| e.message),
            Compiled::Clif(a) => a.call(name, args).map_err(|e| e.message),
        }
    }
}

/// `beck native` — compile to machine code, and account for every definition.
///
/// The report is the command's main output rather than a footnote, because a native backend that
/// covers part of a language is only honest if it says which part. A definition is either in the
/// first list, compiled, or in the second with the reason it is not.
fn native(
    file: &Path,
    backend: &str,
    out: Option<&Path>,
    call: Option<&str>,
    args: &[String],
) -> Result<()> {
    // Not `compiled`: a module with no merge point is a **library**, and a library of arithmetic
    // is exactly what this backend compiles best — `awfy/mandelbrot.beck` is one. `test_cmd` takes
    // the same route for the same reason.
    let (project, map, mut diags) = checked_project(file)?;
    let placed = project.and_then(|p| beck_core::project::slice_or_library(p, &mut diags));
    print!("{}", diags.render(&map));
    let placed = placed.ok_or_else(|| anyhow::anyhow!("{} does not compile", file.display()))?;
    let program = placed.program;

    // One command, two code generators, and the report says which produced the artefact. The two
    // are held to answering the same thing by `cranelift.rs`, so choosing between them is a choice
    // about *build* time rather than about what the program means.
    let compiled: Compiled = match backend {
        "llvm" => {
            let Some(toolchain) = beck_llvm::Toolchain::find() else {
                bail!(
                    "no LLVM toolchain: no `clang` on the path, and BECK_CLANG does not name a \
                     working one. `--backend cranelift` needs only a linker"
                );
            };
            Compiled::Llvm(
                beck_llvm::Artifact::build_with(&program, toolchain, out)
                    .map_err(|e| anyhow::anyhow!(e))?,
            )
        }
        "cranelift" | "clif" => {
            let Some(linker) = beck_clif::Linker::find() else {
                bail!(
                    "no linker: no `cc`, `clang` or `gcc` on the path, and BECK_LINKER does not \
                     name a working one. An object file is not a program"
                );
            };
            Compiled::Clif(
                beck_clif::Artifact::build_with(&program, linker, out)
                    .map_err(|e| anyhow::anyhow!(e))?,
            )
        }
        other => bail!("`{other}` is not a code generator: `llvm` or `cranelift`"),
    };
    print!("{}", compiled.report());
    if out.is_some() {
        println!(
            "\n{}\n{}",
            compiled.ir().display(),
            compiled.exe().display()
        );
    }

    let Some(name) = call else {
        return Ok(());
    };
    let sig = compiled
        .signature(name)
        .ok_or_else(|| anyhow::anyhow!("`{name}` is not one of the definitions that compiled"))?;
    if args.len() != sig.params.len() {
        bail!(
            "`{name}` takes {} arguments, got {}",
            sig.params.len(),
            args.len()
        );
    }
    let mut values = Vec::with_capacity(args.len());
    for (text, want) in args.iter().zip(&sig.params) {
        values.push(match want {
            beck_llvm::Repr::Int => beck_core::Value::Int(
                text.parse()
                    .with_context(|| format!("`{text}` is not an Int"))?,
            ),
            beck_llvm::Repr::Float => beck_core::Value::float(
                text.parse()
                    .with_context(|| format!("`{text}` is not a Float"))?,
            ),
            beck_llvm::Repr::Bool => beck_core::Value::Bool(
                text.parse()
                    .with_context(|| format!("`{text}` is not a Bool"))?,
            ),
            // A record has no notation on a command line — `Point(x=1, y=2)` would be a parser for
            // the language's own literals, written a second time and in a worse place. The
            // definition still compiled, and `--out` writes the artefact that can be called.
            beck_llvm::Repr::Obj(_) => bail!(
                "`{name}` takes a record or a union, and `--call` can only pass Int, Float and Bool"
            ),
        });
    }
    match compiled.call(name, &values) {
        Ok(v) => {
            println!("\n{name} = {}", v.display());
            Ok(())
        }
        // A trap carries the span of the operation that could not answer, so this is a diagnostic
        // and not a status code — the same message the evaluator would have printed.
        Err(e) => bail!("{name}: {e}"),
    }
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
        Explain::Place { file, name } => {
            let placed = compiled(&file)?;
            print!(
                "{}",
                beck_core::place::report(&placed.placement, name.as_deref())
                    .map_err(|e| anyhow!(e))?
            );
            Ok(())
        }
        Explain::Wire { file } => {
            print!("{}", beck_core::split::wire_report(&compiled(&file)?));
            Ok(())
        }
        Explain::Render { file } => {
            let placed = compiled(&file)?;
            let bundle = beck_core::Bundle::of(&placed);
            print!("{}", placed.render.explain(&bundle));
            Ok(())
        }
        Explain::Flow { file, ty: Some(ty) } => {
            let placed = compiled(&file)?;
            print!(
                "{}",
                beck_core::secure::flow_report(&placed.program, &ty).map_err(|e| anyhow!(e))?
            );
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
        Explain::Query { file, unfused } => {
            let placed = compiled(&file)?;
            let plan = beck_core::plan::Plan::unfused(&placed);
            if unfused {
                print!("{}", beck_core::plan::query_report(&plan));
                return Ok(());
            }
            let (plan, fusions) = beck_core::fuse::fuse(plan);
            print!("{}", beck_core::plan::query_report(&plan));
            print!("{}", beck_core::fuse::report(&fusions));
            Ok(())
        }
        Explain::Cost { file } => {
            let placed = compiled(&file)?;
            let plan = beck_core::plan::Plan::compile(&placed);
            print!("{}", beck_core::plan::cost_report(&plan));
            Ok(())
        }
        Explain::Sql { file } => {
            let placed = compiled(&file)?;
            let plan = beck_core::plan::Plan::compile(&placed);
            let schema = beck_core::read::Schema::of(&placed, &plan);
            print!("{}", schema.ddl());
            Ok(())
        }
        Explain::Deploy { file } => {
            let placed = compiled(&file)?;
            print!("{}", beck_infra::graph(&placed).explain());
            Ok(())
        }
        Explain::Error { code } => docs::explain_error(&code),
    }
}

/// `beck run`, on a runtime whose worker threads have the evaluator's stack.
///
/// `#[tokio::main]` cannot say that, and the folds and views of a served program run on these
/// threads: a stack a worker did not have would take the server down rather than the request.
/// What `beck run` was told about who may connect. All four absent is `DevIdentity`.
struct Auth {
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
}

fn run(
    file: &Path,
    addr: &str,
    store: Store,
    path: &Path,
    url: Option<&str>,
    pgwire: Option<&str>,
    auth: Auth,
) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(beck_eval::STACK_BYTES)
        .build()?
        .block_on(serve(file, addr, store, path, url, pgwire, auth))
}

/// Build the identity provider this process will hold, and discover it before anything is served.
///
/// Discovery is at startup rather than at the first login on purpose: a process that cannot reach
/// its identity provider has a configuration problem, and the moment to say so is now — not when
/// somebody tries to sign in.
fn identity(
    declared: Option<&beck_core::check::IdentityDecl>,
    auth: &Auth,
    addr: &str,
    clock: &Arc<dyn beck_core::clock::Clock>,
) -> Result<Option<Arc<beck_rt::oidc::RelyingParty>>> {
    let Some(declared) = declared else {
        if auth.client_id.is_some() {
            anyhow::bail!(
                "`--client-id` needs a program that says who its clients are: add \
                 `identity = external(issuer=\"https://…\")`"
            );
        }
        return Ok(None);
    };
    // `managed()` says the *deployment* provisions the provider, so at rung 0 there is nothing to
    // reach: D6's own answer is that "rung 0 (`beck run`) uses a dev-mode identity", and the
    // deployment supplies the issuer through the environment exactly as it supplies the log's URL.
    let issuer = match declared {
        beck_core::check::IdentityDecl::External { issuer, .. } => issuer.to_string(),
        beck_core::check::IdentityDecl::Managed { .. } => {
            match std::env::var(beck_infra::provider::DEFAULT.issuer_var) {
                Ok(url) if !url.is_empty() => url,
                _ => {
                    eprintln!(
                        "identity     dev — this program's provider is provisioned by `beck \
                         build`, and there is none here"
                    );
                    return Ok(None);
                }
            }
        }
    };
    let client_id = auth.client_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!("this program authenticates against `{issuer}` and needs a `--client-id`")
    })?;
    let redirect = auth
        .redirect_uri
        .clone()
        .unwrap_or_else(|| format!("http://{addr}{}", beck_rt::http::CALLBACK_PATH));

    // Two constructors, and which one is chosen is decided by the *declaration* rather than by the
    // URL's scheme: a provider this deployment provisioned is reached inside one namespace, and one
    // it did not must be reached over TLS (`docs/95` §95.10).
    let mut config = match declared {
        beck_core::check::IdentityDecl::Managed { .. } => {
            beck_rt::oidc::Config::in_cluster(&issuer, client_id, &redirect)
        }
        beck_core::check::IdentityDecl::External { .. } => {
            beck_rt::oidc::Config::new(&issuer, client_id, &redirect)
        }
    };
    config.client_secret = auth.client_secret.clone();
    // Its own client, not the process-global one the evaluator reads. A program's outbound calls
    // and the runtime's calls to its identity provider are two different things to be able to
    // stub, to bound and to read in a log — and the process-global one is installed once and
    // never replaced, so sharing it would make identity depend on whether a program made a call.
    let client: Arc<dyn beck_core::net::Outbound> =
        Arc::new(beck_rt::outbound::HttpOutbound::new()?);
    let party = Arc::new(beck_rt::oidc::RelyingParty::new(
        config,
        clock.clone(),
        client,
    ));
    party
        .refresh()
        .map_err(|why| anyhow::anyhow!("the identity provider could not be read: {why}"))?;
    Ok(Some(party))
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    file: &Path,
    addr: &str,
    store: Store,
    path: &Path,
    url: Option<&str>,
    pgwire: Option<&str>,
    auth: Auth,
) -> Result<()> {
    let placed = compiled(file)?;
    // A serving process is one that may make outbound calls, so it is the process that installs a
    // client. `beck test` deliberately does not: `net.out` is auto-stubbed there (§21.3), and a
    // test that reached a socket would depend on somebody else's uptime.
    beck_rt::outbound::HttpOutbound::install();
    // Built before the app starts and never rebuilt: the program cannot change under a running
    // process, so the dashboard's structural panes are computed once (docs/19 §19.8).
    let dashboard = Arc::new(dashboard(&placed));
    // The one place the process chooses how the program executes. A native backend is a different
    // expression here and nothing else (docs/19 §19.8).
    let backend = beck_eval::backend(&placed);
    // A Mode B page is rendered by a kernel this process serves but does not contain, so the miss
    // is worth reporting when the server starts rather than when somebody's tab comes up blank.
    if placed.render.mode == beck_core::render::Mode::Client {
        let kernel = beck_rt::http::kernel_path();
        if !kernel.is_file() {
            tracing::warn!(
                path = %kernel.display(),
                "`{}` renders on the client and there is no kernel to serve it: \
                 `cargo build -p beck-wasm --release --target wasm32-unknown-unknown`, \
                 or set BECK_KERNEL",
                placed.render.component,
            );
        }
    }
    let log = open_store(store, path, url).await?;
    // Read before the program is moved into the runtime: who authenticates this program's clients
    // is a property of the program (D6), and this is the one place it is turned into a provider.
    let declared = placed.program.identity.clone();
    let runtime = beck_rt::Runtime::new(placed, backend)?;

    let mut config = beck_rt::AppConfig::default();
    let relying_party = identity(declared.as_ref(), &auth, addr, &config.clock)?;
    if let Some(party) = &relying_party {
        config.identity = party.clone();
    }
    let app = beck_rt::App::start(runtime, log, config).await?;
    let (tx, rx) = tokio::sync::watch::channel(false);

    // The key set is fetched again on a timer and whenever a token named a key it does not carry.
    // A task rather than a fetch on the connection path: verifying an ID token must not be a way
    // for an anonymous client to make this process call its identity provider (§95.3).
    if let Some(party) = relying_party.clone() {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    (beck_rt::oidc::REFETCH_FLOOR_MS / 1_000).max(1) as u64,
                ))
                .await;
                let due = party.clone();
                if !due.refresh_due() {
                    continue;
                }
                let attempt = party.clone();
                if let Ok(Err(why)) = tokio::task::spawn_blocking(move || attempt.refresh()).await {
                    tracing::warn!(why, "the identity provider's key set was not refreshed");
                }
            }
        });
    }

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        // Graceful drain: stop accepting, let the fold finish, exit. Everything already
        // acknowledged is already durable, so there is nothing to flush.
        let _ = tx.send(true);
    });

    // Which provider is in force, on the line an operator reads first. `docs/48` §48.3: an
    // operator who cannot tell from the logs whether authentication is on does not have
    // authentication.
    let entry = match &relying_party {
        Some(party) => format!(
            "identity     {} ({}, {} keys), sign in at http://{addr}{}",
            app.identity().kind(),
            party.config().issuer,
            party.key_count(),
            beck_rt::http::LOGIN_PATH,
        ),
        None => format!(
            "identity     {} — this process believes whatever a client says it is",
            app.identity().kind()
        ),
    };
    eprintln!(
        "beck run — store {}, head {}, open http://{addr}/{}\n\
         {entry}\n\
         dashboard    http://{addr}/_beck",
        app.store_kind(),
        app.head(),
        if relying_party.is_some() {
            ""
        } else {
            "?actor=dev"
        },
    );
    if let Some(pg) = pgwire {
        let pg: std::net::SocketAddr = pg.parse()?;
        // Bound here rather than inside the spawned task, so an address this process may not have
        // — anything but loopback — fails the command instead of a background task nobody reads.
        let listener = beck_rt::pgwire::bind(pg).await?;
        let bound = listener.local_addr()?;
        eprintln!("read models   psql -h {} -p {}", bound.ip(), bound.port());
        let for_sql = app.clone();
        tokio::spawn(async move {
            if let Err(e) = beck_rt::pgwire::serve_on(listener, for_sql).await {
                tracing::error!(error = %e, "the read-model port stopped");
            }
        });
    }
    beck_rt::http::serve_with_dashboard(app, addr.parse()?, rx, Some(dashboard)).await
}

/// `beck test` — §21.2's construct, run.
///
/// Note what this command does not take: no `--url`, no `--store`, no address. A test performs no
/// effects (`B0700` is a compile error) and its subject's effects are stubbed, so there is nothing
/// to point at anything.
fn test_cmd(
    file: &Path,
    filter: Option<&str>,
    verbose: bool,
    runs: u64,
    fuel: u64,
    update: bool,
) -> Result<()> {
    // Not `compiled`: a module with no merge point is a **library**, and a library's tests are the
    // ones a developer most wants to run (docs/22 §22.6, docs/25 §25.6 item 1, docs/27 §27.2).
    // `slice_or_library` gives one back instead of refusing it; every other diagnostic still does.
    let (project, map, mut diags) = checked_project(file)?;
    let placed = project.and_then(|p| beck_core::project::slice_or_library(p, &mut diags));
    print!("{}", diags.render(&map));
    let placed = placed.ok_or_else(|| anyhow::anyhow!("{} does not compile", file.display()))?;
    if placed.program.tests.is_empty() {
        eprintln!("no `test` or `property` blocks in {}", file.display());
        return Ok(());
    }
    let backend = beck_eval::backend_with_fuel(&placed, fuel);
    let opts = beck_rt::testing::Options {
        filter: filter.map(str::to_string),
        runs,
        base_dir: file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
        update_snapshots: update,
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
        IdentityProvider { volume_gb, .. } => format!("{volume_gb}Gi, one realm"),
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
        Store::Sqlite => Arc::new(beck_rt::SqliteLog::open(path)?),
        Store::Postgres => {
            let url = url.context("--url or BECK_POSTGRES_URL is required for --store postgres")?;
            Arc::new(beck_rt::PgLog::connect(url).await?)
        }
    })
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
