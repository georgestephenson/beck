//! `beck doc` — module documentation, and the language reference.
//!
//! Two things share this file because they share a claim: **nothing here is written twice.** A
//! module's page is derived from the module ([`beck_core::docgen`]); the reference pages are
//! derived from the tables the compiler itself reads.
//!
//! | Reference page | Derived from |
//! |---|---|
//! | Error index | [`beck_diag::index::INDEX`], cross-checked against every code the workspace emits |
//! | CLI reference | the `clap` command tree — the same one that parses the arguments |
//! | Effects and tiers | [`beck_core::Tier::discharges`], *run* to build §3.3's matrix |
//! | Prelude | [`beck_core::prelude::prims`] and [`beck_core::prelude::types`] |
//! | Forms | [`beck_syntax::sym::RESERVED_FORMS`] |
//!
//! The discharge matrix is the one worth pointing at: it is not a transcription of §3.3's table,
//! it is `Tier::discharges` evaluated at every (tier, atom) pair. If the solver's rule changes, the
//! page changes with it, and the CI drift gate says so in the diff.
//!
//! # The drift gate
//!
//! `docs/reference/` is checked in. `beck doc reference --check` regenerates it in memory and fails
//! if what is on disk differs — the gate the `docs` workflow runs on every pull request. Checked in
//! rather than built-only because a generated file that nobody sees in a diff is a generated file
//! nobody notices going wrong ([`docs/20-phase-2-report.md`](../../../../../docs/20-phase-2-report.md)
//! §20.4 item 8 is what that costs).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use beck_core::docgen::{self, Docs};
use beck_core::ty::{Effect, Tier, TyDecl, CONCRETE_TIERS};
use beck_diag::index::{self, Stage};

/// What a generated page is written as.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// Markdown — the form that is checked in, reviewed and diffed.
    Md,
    /// A self-contained HTML page — the form that is published.
    Html,
    /// JSON — the form a tool consumes. Module pages only.
    Json,
}

// ------------------------------------------------------------------------ a module's own page

/// `beck doc <file>` — one module's reference documentation.
///
/// The program is compiled by the caller rather than here, because a module that **imports**
/// another cannot be read on its own: `lib/decimal.beck` is written over `lib/bignum.beck`, and a
/// single-file read of it cannot find `Big`. `docs/56` §56.5 is where that was found — the first
/// library in the tree to import another was the first to notice.
pub fn module(
    project: &beck_core::project::Project,
    out: Option<&Path>,
    format: Format,
    stdout: bool,
) -> Result<()> {
    let docs = Docs::of_interface(&project.interface, &project.program.docs);

    let rendered = match format {
        Format::Md => docs.to_markdown(),
        Format::Html => docs.to_html(),
        Format::Json => docs.to_json(),
    };

    let (with, all) = docs.documented();
    if stdout {
        print!("{rendered}");
        return Ok(());
    }
    let dir = out.unwrap_or_else(|| Path::new("doc"));
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.{}", docs.module, ext(format)));
    std::fs::write(&path, &rendered).with_context(|| format!("writing {}", path.display()))?;
    println!(
        "{} — {all} published names, {with} documented",
        path.display()
    );
    Ok(())
}

fn ext(f: Format) -> &'static str {
    match f {
        Format::Md => "md",
        Format::Html => "html",
        Format::Json => "json",
    }
}

// -------------------------------------------------------------------------- the language reference

/// One generated reference page: the file it is written to, and its two renderings.
struct Page {
    slug: &'static str,
    title: &'static str,
    blurb: &'static str,
    markdown: String,
    body: String,
}

/// `beck doc reference` — the whole reference, written or checked.
///
/// `check` is the drift gate: nothing is written, and a file that differs from what the compiler
/// would generate is an error naming the file.
pub fn reference(out: &Path, format: Format, check: bool) -> Result<()> {
    if format == Format::Json {
        bail!("the reference is generated as markdown or html; json is for module pages");
    }
    let pages = build_pages();
    let index = index_page(&pages);

    let mut files: Vec<(PathBuf, String)> = Vec::new();
    for p in &pages {
        let rendered = match format {
            Format::Md => p.markdown.clone(),
            Format::Html | Format::Json => docgen::page(
                &format!("{} — beck", p.title),
                docgen::REFERENCE_PAGE_HOME,
                &breadcrumb(p.title),
                &p.body,
            ),
        };
        files.push((out.join(format!("{}.{}", p.slug, ext(format))), rendered));
    }
    files.push((
        out.join(format!("{}.{}", "README", ext(format))),
        match format {
            Format::Md => index.0,
            _ => docgen::page(
                "The beck language reference",
                docgen::REFERENCE_PAGE_HOME,
                "",
                &index.1,
            ),
        },
    ));
    // A site needs an `index.html`; a directory of markdown in a repository needs a `README.md`.
    if format == Format::Html {
        let last = files.len() - 1;
        files[last].0 = out.join("index.html");
    }

    if check {
        let mut stale = Vec::new();
        for (path, want) in &files {
            match std::fs::read_to_string(path) {
                Ok(have) if &have == want => {}
                Ok(_) => stale.push(format!("  {} differs", path.display())),
                Err(_) => stale.push(format!("  {} is missing", path.display())),
            }
        }
        if !stale.is_empty() {
            bail!(
                "the checked-in reference is not what the compiler generates:\n{}\n\
                 regenerate it: `beck doc reference --out {}`",
                stale.join("\n"),
                out.display()
            );
        }
        println!(
            "the checked-in reference matches the compiler across {} pages",
            files.len()
        );
        return Ok(());
    }

    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    for (path, text) in &files {
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    println!("wrote {} pages to {}", files.len(), out.display());
    Ok(())
}

fn breadcrumb(title: &str) -> String {
    format!(" / {}", docgen::escape(title))
}

fn build_pages() -> Vec<Page> {
    vec![
        errors_page(),
        cli_page(),
        effects_page(),
        prelude_page(),
        forms_page(),
    ]
}

const GENERATED: &str =
    "*Generated by `beck doc reference` from the compiler's own tables. Do not edit: \
     `.github/workflows/docs.yml` fails the build when this file and the compiler disagree.*";

fn index_page(pages: &[Page]) -> (String, String) {
    let mut md = String::from("# The beck language reference\n\n");
    let _ = writeln!(md, "{GENERATED}\n");
    md.push_str(
        "Every page here is derived from the compiler: the error index from the codes it can \
         emit, the command reference from the parser that reads the arguments, the effect and tier \
         tables from the predicate the placement solver evaluates, and the prelude from the \
         schemes inference reads. Nothing on these pages is a transcription that can go stale \
         without the build noticing.\n\n\
         The design documents — what Beck is, and why — are in [`docs/`](../README.md). This is \
         the part a program is checked against.\n\n",
    );
    let mut html = String::from("<h1>The beck language reference</h1>\n<p class=\"muted\">Derived from the compiler on every build.</p>\n<table><tr><th>Page</th><th></th></tr>\n");
    for p in pages {
        let _ = writeln!(md, "- [{}]({}.md) — {}", p.title, p.slug, p.blurb);
        let _ = writeln!(
            html,
            "<tr><td><a href=\"{}.html\">{}</a></td><td>{}</td></tr>",
            p.slug,
            docgen::escape(p.title),
            docgen::escape(p.blurb)
        );
    }
    md.push('\n');
    html.push_str("</table>\n");

    // Published-site links only. The markdown form is read in the repository, where `api/` and
    // `module/` do not exist — they are built alongside the HTML by `.github/workflows/docs.yml`.
    html.push_str(
        "<h2>Also published</h2>\n<table><tr><th>Page</th><th></th></tr>\n\
         <tr><td><a href=\"guide/getting-started.html\">Getting started</a></td>\
         <td>build the compiler, write a program, and see what it worked out on its own — \
         every program on the page is compiled and run by a test</td></tr>\n\
         <tr><td><a href=\"module/todo.html\"><code>todo</code></a></td>\
         <td>the sketch from the original idea, documented by <code>beck doc</code></td></tr>\n\
         <tr><td><a href=\"module/documented.html\"><code>documented</code></a></td>\
         <td>a library written to show what a <code>##</code> comment adds to a derived page</td></tr>\n\
         <tr><td><a href=\"api/beck_core/index.html\">Compiler API</a></td>\
         <td>rustdoc for the nine crates the compiler and runtime are built from</td></tr>\n\
         </table>\n",
    );
    (md, html)
}

// ------------------------------------------------------------------------------------- a guide

/// `beck doc guide <file.md>` — a written guide, rendered into the same shell as everything else.
///
/// The site has two kinds of page and they are made differently on purpose. A reference page is
/// *derived*, and a drift gate holds it to the compiler. A guide is *written*, and what holds it
/// honest is that its programs are compiled and run by a harness — `getting_started.rs` for the one
/// this exists to publish. Rendering it here rather than checking in a second HTML copy is the same
/// rule as `docs/reference/`: one source, and a build that produces the rest.
pub fn guide(file: &Path, out: &Path, link_base: Option<&str>, stdout: bool) -> Result<()> {
    let src =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let links = link_base.map(|base| docgen::Links { base });
    let title = docgen::guide_title(&src).unwrap_or("Guide");
    let body = docgen::guide(&src, links);
    let rendered = docgen::page(
        &format!("{title} — beck"),
        "../index.html",
        " / guide",
        &body,
    );
    if stdout {
        print!("{rendered}");
        return Ok(());
    }
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "guide".to_string());
    // `86-getting-started` is a file name in a numbered directory; `getting-started` is a URL.
    let slug = stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    let path = out.join(format!(
        "{}.html",
        if slug.is_empty() { &stem } else { slug }
    ));
    std::fs::write(&path, &rendered).with_context(|| format!("writing {}", path.display()))?;
    println!("{} — \"{title}\"", path.display());
    Ok(())
}

// ------------------------------------------------------------------------------- the error index

fn errors_page() -> Page {
    let mut md = String::from("# Error index\n\n");
    let _ = writeln!(md, "{GENERATED}\n");
    let _ = writeln!(
        md,
        "Every diagnostic the compiler can raise carries a stable code. `beck explain error \
         B0341` prints one of these entries at the terminal.\n\n\
         The index is held to the compiler by a test: `beck-cli/tests/docs.rs` scans every \
         non-test source file for a `\"Bnnnn\"` literal and fails if the set differs from this \
         table in either direction. **{} codes.**\n",
        index::INDEX.len()
    );

    let mut html = String::from("<h1>Error index</h1>\n");
    let _ = writeln!(
        html,
        "<p class=\"muted\">{} codes, every one the compiler can emit. \
         <code>beck explain error B0341</code> prints one at the terminal.</p>",
        index::INDEX.len()
    );

    for stage in Stage::all() {
        let entries: Vec<_> = index::in_stage(*stage).collect();
        if entries.is_empty() {
            continue;
        }
        let _ = writeln!(md, "\n## {} — `{}`\n", stage.title(), band(&entries));
        md.push_str("| Code | | Meaning |\n|---|---|---|\n");
        let _ = writeln!(
            html,
            "<h2>{} <span class=\"muted\">{}</span></h2>\n<table><tr><th>Code</th><th></th><th>Meaning</th></tr>",
            docgen::escape(stage.title()),
            band(&entries)
        );
        for entry in entries {
            let kind = if entry.warning { "warning" } else { "error" };
            let _ = writeln!(
                md,
                "| `{}` | {kind} | **{}** — {} |",
                entry.code, entry.title, entry.explain
            );
            let _ = writeln!(
                html,
                "<tr><td id=\"{}\"><code>{}</code></td><td><span class=\"tag\">{kind}</span></td>\
                 <td><strong>{}</strong> — {}</td></tr>",
                entry.code.to_lowercase(),
                entry.code,
                docgen::escape(entry.title),
                docgen::escape(entry.explain)
            );
        }
        html.push_str("</table>\n");
    }
    md.push('\n');
    Page {
        slug: "errors",
        title: "Error index",
        blurb: "every diagnostic code the compiler can emit, and what it means",
        markdown: md,
        body: html,
    }
}

fn band(entries: &[&index::CodeEntry]) -> String {
    match (entries.first(), entries.last()) {
        (Some(a), Some(b)) if a.code != b.code => format!("{}–{}", a.code, b.code),
        (Some(a), _) => a.code.to_string(),
        _ => String::new(),
    }
}

// --------------------------------------------------------------------------- the command reference

fn cli_page() -> Page {
    use clap::CommandFactory;
    let cmd = crate::Cli::command();

    let mut md = String::from("# Command reference\n\n");
    let _ = writeln!(md, "{GENERATED}\n");
    md.push_str(
        "One binary serves the whole toolchain (§4.6), so this is the complete surface. Every \
         entry below is read from the same command tree that parses the arguments.\n",
    );
    let mut html = String::from(
        "<h1>Command reference</h1>\n<p class=\"muted\">Read from the same command tree that \
         parses the arguments.</p>\n",
    );

    for sub in cmd.get_subcommands() {
        write_command(&mut md, &mut html, sub, &["beck".to_string()]);
    }
    Page {
        slug: "cli",
        title: "Command reference",
        blurb: "every `beck` subcommand, its arguments and its flags",
        markdown: md,
        body: html,
    }
}

fn write_command(md: &mut String, html: &mut String, cmd: &clap::Command, path: &[String]) {
    let mut full = path.to_vec();
    full.push(cmd.get_name().to_string());
    let name = full.join(" ");
    let about = cmd
        .get_long_about()
        .or_else(|| cmd.get_about())
        .map(|a| a.to_string())
        .unwrap_or_default();

    let _ = writeln!(md, "\n## `{name}`\n");
    let _ = writeln!(
        html,
        "<h2 id=\"{}\"><code>{}</code></h2>",
        docgen::anchor(&name.replace(' ', "-")),
        docgen::escape(&name)
    );
    if !about.is_empty() {
        let _ = writeln!(md, "{about}\n");
        html.push_str(&docgen::prose(&about));
        html.push('\n');
    }

    let args: Vec<&clap::Arg> = cmd
        .get_arguments()
        .filter(|a| a.get_id() != "help")
        .collect();
    if !args.is_empty() {
        md.push_str("| Argument | | |\n|---|---|---|\n");
        html.push_str("<table><tr><th>Argument</th><th></th><th></th></tr>\n");
        for a in args {
            let spelling = arg_spelling(a);
            let help = a
                .get_long_help()
                .or_else(|| a.get_help())
                .map(|h| h.to_string())
                .unwrap_or_default()
                .replace('\n', " ");
            let values = arg_values(a);
            let _ = writeln!(md, "| `{spelling}` | {values} | {help} |");
            let _ = writeln!(
                html,
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                docgen::escape(&spelling),
                docgen::escape(&values),
                docgen::escape(&help)
            );
        }
        md.push('\n');
        html.push_str("</table>\n");
    }
    for sub in cmd.get_subcommands() {
        write_command(md, html, sub, &full);
    }
}

fn arg_spelling(a: &clap::Arg) -> String {
    match (a.get_long(), a.get_short()) {
        (Some(l), Some(s)) => format!("-{s}, --{l}"),
        (Some(l), None) => format!("--{l}"),
        (None, Some(s)) => format!("-{s}"),
        (None, None) => format!("<{}>", a.get_id().as_str().to_uppercase()),
    }
}

fn arg_values(a: &clap::Arg) -> String {
    // A flag takes no value, and clap reports `true`/`false` as its possible ones. Printing those
    // would say a flag is written `--write true`, which it is not.
    if !a.get_num_args().map(|n| n.takes_values()).unwrap_or(true) {
        return String::new();
    }
    let possible: Vec<String> = a
        .get_possible_values()
        .iter()
        .map(|v| v.get_name().to_string())
        .collect();
    if !possible.is_empty() {
        return possible.join(" \\| ");
    }
    a.get_value_names()
        .and_then(|n| n.first().map(|v| v.to_string()))
        .unwrap_or_else(|| "value".to_string())
}

// ------------------------------------------------------------------------- effects and tiers

/// Every atom the reference names, with the sentence that says what it is.
///
/// The parameterised atoms are shown with a placeholder argument, because `net.out(api.example)` is
/// one atom per host and the set is open.
fn atoms() -> Vec<(Effect, &'static str)> {
    vec![
        (Effect::Ingress, "The merge point: arbitrary interleaving of client proposals. A program has exactly one."),
        (Effect::Durable, "A persistent accumulator — the log."),
        (Effect::Dom, "Touches the document."),
        (Effect::Nondet, "Reads a clock or a random source, or mints an id."),
        (Effect::NetOut(std::sync::Arc::from("host")), "An outbound call to a named host. The host is what becomes a NetworkPolicy peer (§6.5)."),
        (Effect::NetIn, "Accepts inbound connections."),
        (Effect::FsRead(std::sync::Arc::from("path")), "Reads a path."),
        (
            Effect::FsWrite(std::sync::Arc::from("path")),
            "Writes a path.",
        ),
        (Effect::Env, "Reads process environment."),
        (Effect::Spawn, "Starts concurrent work."),
        (Effect::Cap(std::sync::Arc::from("x")), "A capability the caller must hold. Forgetting an auth check leaves `cap.*` undischarged — a compile error, not a pentest finding (§3.5)."),
        (Effect::Partial, "May diverge or panic."),
        (
            Effect::Raises(std::sync::Arc::from("E")),
            "May fail with a value of the named type. A signature without this provably cannot \
             fail; `try:` reifies it into a `Result[T, E]`.",
        ),
        (Effect::ExternalRead(std::sync::Arc::from("store")), "Reads a store the program does not own — §3.8's escape hatch."),
        (Effect::ExternalWrite(std::sync::Arc::from("store")), "Writes a store the program does not own."),
        (Effect::Ambient(beck_core::row::Ambient::Log), "Ambient: available everywhere, elided from signatures, never a reason to place anything."),
        (Effect::Ambient(beck_core::row::Ambient::Metrics), "Ambient, as `log` is."),
    ]
}

fn effects_page() -> Page {
    let mut md = String::from("# Effects, and the tiers that discharge them\n\n");
    let _ = writeln!(md, "{GENERATED}\n");
    md.push_str(
        "A definition's effect row is **inferred**, not declared (§3.2), and its placement is \
         **solved** from that row (§3.4). The matrix below is not a transcription of the design \
         document's table — it is `Tier::discharges` evaluated at every pair, which is the \
         predicate the solver itself calls. A `uses` clause narrows what a definition may perform; \
         it never widens it.\n\n",
    );
    let mut html = String::from(
        "<h1>Effects, and the tiers that discharge them</h1>\n<p class=\"muted\">\
         <code>Tier::discharges</code> evaluated at every pair — the predicate the placement \
         solver calls.</p>\n",
    );

    let header: Vec<String> = CONCRETE_TIERS
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    let _ = writeln!(md, "| Atom | {} | any | |", header.join(" | "));
    let _ = writeln!(
        md,
        "|---|{}---|---|",
        CONCRETE_TIERS.iter().map(|_| "---|").collect::<String>()
    );
    let _ = writeln!(
        html,
        "<table><tr><th>Atom</th>{}<th>any</th><th></th></tr>",
        header
            .iter()
            .map(|h| format!("<th>{h}</th>"))
            .collect::<String>()
    );
    for (atom, blurb) in atoms() {
        let cells: Vec<&str> = CONCRETE_TIERS
            .iter()
            .map(|t| if t.discharges(&atom) { "yes" } else { "—" })
            .collect();
        let any = if Tier::Any.discharges(&atom) {
            "yes"
        } else {
            "—"
        };
        let _ = writeln!(
            md,
            "| `{}` | {} | {any} | {blurb} |",
            atom.name(),
            cells.join(" | ")
        );
        let _ = writeln!(
            html,
            "<tr><td><code>{}</code></td>{}<td>{any}</td><td>{}</td></tr>",
            docgen::escape(&atom.name()),
            cells
                .iter()
                .map(|c| format!("<td>{c}</td>"))
                .collect::<String>(),
            docgen::escape(blurb)
        );
    }
    md.push('\n');
    html.push_str("</table>\n");

    md.push_str(
        "`any` is not a fourth tier: it means *unplaced* — legal everywhere, and compiled into \
         each tier that calls it. An atom no single tier discharges is `B0400`; a written \
         `@on(...)` that contradicts the row is `B0401`.\n",
    );
    html.push_str(
        "<p><code>any</code> is not a fourth tier: it means <em>unplaced</em> — legal everywhere, \
         and compiled into each tier that calls it.</p>\n",
    );
    Page {
        slug: "effects",
        title: "Effects and tiers",
        blurb: "the effect atoms, and which tier can discharge each",
        markdown: md,
        body: html,
    }
}

// -------------------------------------------------------------------------------- the prelude

fn prelude_page() -> Page {
    let mut md = String::from("# The prelude\n\n");
    let _ = writeln!(md, "{GENERATED}\n");
    md.push_str(
        "Every name in scope in every module, with the type inference reads for it. Small on \
         purpose: §3.2's promise is that effect polymorphism is what keeps *one* standard library, \
         so `map_list` is one definition whatever its function argument does, and mapping an \
         effectful function over a list is effectful in exactly that way.\n\n",
    );
    let mut html = String::from(
        "<h1>The prelude</h1>\n<p class=\"muted\">Every name in scope in every module, with the \
         scheme inference reads for it.</p>\n",
    );

    let traits = beck_core::prelude::traits();
    if !traits.is_empty() {
        md.push_str(
            "## Traits\n\nWhat a user's type may implement to join something the language already \
             has. `Num` is SICP §2.5.1's generic arithmetic: `+`, `-`, `*` and `/` resolve through \
             it for an operand that is neither `Int` nor `Float` nor `Str`, so a numeric type is \
             something a program declares rather than something the compiler has a list of.\n\n| Trait | Methods |\n|---|---|\n",
        );
        html.push_str(
            "<h2>Traits</h2>\n<p class=\"muted\">What a user's type may implement to join \
             something the language already has.</p>\n<table><tr><th>Trait</th><th>Methods</th></tr>\n",
        );
        for t in &traits {
            let methods: Vec<String> = t
                .methods
                .iter()
                .map(|m| {
                    let params: Vec<String> = m
                        .params
                        .iter()
                        .map(|(n, ty)| {
                            if n.as_ref() == "self" {
                                "self".to_string()
                            } else {
                                format!("{n}: {ty}")
                            }
                        })
                        .collect();
                    format!("def {}({}) -> {}", m.name, params.join(", "), m.ret)
                })
                .collect();
            let _ = writeln!(md, "| `{}` | `{}` |", t.name, methods.join("`, `"));
            let _ = writeln!(
                html,
                "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>",
                docgen::escape(&t.name),
                docgen::escape(&methods.join(", "))
            );
        }
        md.push('\n');
        html.push_str("</table>\n");
    }

    let types = beck_core::prelude::types();
    if !types.is_empty() {
        md.push_str("## Types\n\n| Type | Declaration |\n|---|---|\n");
        html.push_str("<h2>Types</h2>\n<table><tr><th>Type</th><th>Declaration</th></tr>\n");
        for (name, decl) in &types {
            let rendered = generalise(&render_ty_decl(decl));
            // A union renders with `|` between variants, which is also the table's cell separator.
            let _ = writeln!(md, "| `{name}` | `{}` |", rendered.replace('|', "\\|"));
            let _ = writeln!(
                html,
                "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>",
                docgen::escape(name),
                docgen::escape(&rendered)
            );
        }
        md.push('\n');
        html.push_str("</table>\n");
    }

    let mut prims: BTreeMap<&str, String> = BTreeMap::new();
    for (name, _, scheme) in beck_core::prelude::prims() {
        prims.insert(name, generalise(&format!("{}", scheme.ty)));
    }
    let _ = writeln!(md, "## Names\n\n{} of them.\n", prims.len());
    md.push_str("| Name | Type |\n|---|---|\n");
    let _ = writeln!(
        html,
        "<h2>Names</h2>\n<p class=\"muted\">{} of them.</p>\n<table><tr><th>Name</th><th>Type</th></tr>",
        prims.len()
    );
    for (name, ty) in &prims {
        let _ = writeln!(md, "| `{name}` | `{ty}` |");
        let _ = writeln!(
            html,
            "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>",
            docgen::escape(name),
            docgen::escape(ty)
        );
    }
    md.push('\n');
    html.push_str("</table>\n");
    // The scheme above is the whole row for every name but one, and a reader who assumed it was
    // the whole row for `http_fetch` would think an outbound call performs nothing but a failure.
    let caveat = "`http_fetch` is the one name whose row the table above cannot state in full: it \
                  also performs `net.out(host)`, for the host written as its first argument. That \
                  argument has to be a literal — the cluster's egress policy is derived from those \
                  atoms, so a computed host is a call the deployment could not be told about.";
    let _ = writeln!(md, "{caveat}\n");
    let _ = writeln!(html, "<p>{}</p>", docgen::escape(caveat));
    Page {
        slug: "prelude",
        title: "The prelude",
        blurb: "every name in scope in every module, with its inferred scheme",
        markdown: md,
        body: html,
    }
}

/// Rewrite a scheme's internal variable numbers as letters.
///
/// A scheme variable's id is the number the prelude happened to mint it with — `?1000000` — and a
/// row variable's is the same. §3.2 writes the signature this page exists to show as
/// `(list[a], (a -> b ! e)) -> list[b] ! e`, and the numbers are not part of it: they are an
/// implementation detail of instantiation, which replaces every one of them at each use.
fn generalise(ty: &str) -> String {
    const TYPE_LETTERS: [&str; 8] = ["a", "b", "c", "d", "g", "h", "i", "j"];
    const ROW_LETTERS: [&str; 4] = ["e", "f", "e2", "e3"];
    let mut types: Vec<String> = Vec::new();
    let mut rows: Vec<String> = Vec::new();
    let mut out = String::with_capacity(ty.len());
    let bytes: Vec<char> = ty.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        // `?123` is a type variable; `e123` is a row variable, but only where `e` starts a word —
        // `env` and `external.read` are effect atoms, not variables.
        let is_type = bytes[i] == '?';
        let is_row = bytes[i] == 'e'
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
            && (i == 0 || !bytes[i - 1].is_alphanumeric());
        if !is_type && !is_row {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start + 1 {
            out.push(bytes[start]);
            continue;
        }
        let id: String = bytes[start..i].iter().collect();
        let (seen, letters) = if is_type {
            (&mut types, &TYPE_LETTERS[..])
        } else {
            (&mut rows, &ROW_LETTERS[..])
        };
        let idx = seen.iter().position(|s| *s == id).unwrap_or_else(|| {
            seen.push(id.clone());
            seen.len() - 1
        });
        out.push_str(letters.get(idx).copied().unwrap_or("?"));
    }
    out
}

fn render_ty_decl(d: &TyDecl) -> String {
    let p = d.param_brackets();
    let fields = |fs: &[(std::sync::Arc<str>, beck_core::Ty)]| {
        fs.iter()
            .map(|(f, t)| format!("{f}: {}", d.as_written(t)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match d {
        TyDecl::Model {
            name, fields: fs, ..
        } => format!("model {name}{p} {{{}}}", fields(fs)),
        TyDecl::Union { name, variants, .. } => format!(
            "union {name}{p} = {}",
            variants
                .iter()
                .map(|v| if v.fields.is_empty() {
                    v.name.to_string()
                } else {
                    format!("{}({})", v.name, fields(&v.fields))
                })
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        TyDecl::Newtype { name, inner, .. } => {
            format!("type {name}{p} = newtype[{}]", d.as_written(inner))
        }
        TyDecl::Alias { name, ty, .. } => format!("type {name}{p} = {}", d.as_written(ty)),
    }
}

// ---------------------------------------------------------------------------- the reserved forms

fn forms_page() -> Page {
    let mut md = String::from("# Forms of the language\n\n");
    let _ = writeln!(md, "{GENERATED}\n");
    md.push_str(
        "Beck has two surfaces and one AST (§2.2): a Python-like notation and the canonical \
         S-expression form, which read to structurally identical trees. The heads below are the \
         reserved ones — the forms the checker matches on, and therefore the names nothing in a \
         program may be called (`B0312`).\n\n",
    );
    let mut html = String::from(
        "<h1>Forms of the language</h1>\n<p class=\"muted\">The reserved heads the checker matches \
         on — and therefore the names nothing may be called.</p>\n",
    );
    md.push_str("| Form |\n|---|\n");
    html.push_str("<table><tr><th>Form</th></tr>\n");
    let mut forms: Vec<&str> = beck_syntax::sym::RESERVED_FORMS.to_vec();
    forms.sort_unstable();
    for f in forms {
        let _ = writeln!(md, "| `{f}` |");
        let _ = writeln!(html, "<tr><td><code>{}</code></td></tr>", docgen::escape(f));
    }
    md.push('\n');
    html.push_str("</table>\n");

    md.push_str(
        "\n## Doc comments\n\n\
         `##` in the Python surface and `;;` in the S-expression one, on the lines immediately \
         above a declaration. A blank line ends a run, so a file header documents the file rather \
         than the first definition. `beck fmt` preserves them; an ordinary `#` comment it does \
         not, because the lexer discards it.\n\n\
         ```beck\n\
         ## One item on the list.\n\
         model Todo:\n\
         \x20   ## Stable for the life of the item.\n\
         \x20   id: Id\n\
         ```\n\n\
         A doc comment is metadata, not a form: documenting a definition does not change the \
         program's meaning, does not invalidate a memo, and does not move the module's interface \
         digest.\n",
    );
    html.push_str(
        "<h2>Doc comments</h2>\n<p><code>##</code> in the Python surface and <code>;;</code> in \
         the S-expression one, on the lines immediately above a declaration. A blank line ends a \
         run. <code>beck fmt</code> preserves them; an ordinary <code>#</code> comment it does \
         not, because the lexer discards it.</p>\n\
         <pre><code>## One item on the list.\nmodel Todo:\n    ## Stable for the life of the item.\n    id: Id</code></pre>\n\
         <p>A doc comment is metadata, not a form: documenting a definition does not change the \
         program's meaning and does not move the module's interface digest.</p>\n",
    );
    Page {
        slug: "forms",
        title: "Forms of the language",
        blurb: "the reserved heads, the two surfaces, and doc comments",
        markdown: md,
        body: html,
    }
}

// ---------------------------------------------------------------------- `beck explain error`

/// `beck explain error B0341` — the index entry, at the terminal.
pub fn explain_error(code: &str) -> Result<()> {
    let Some(entry) = index::lookup(code) else {
        bail!(
            "`{code}` is not a diagnostic code. Codes are `Bnnnn`; `beck doc reference` writes \
             the index of all {}.",
            index::INDEX.len()
        );
    };
    let kind = if entry.warning { "warning" } else { "error" };
    println!("{kind}[{}]: {}\n", entry.code, entry.title);
    println!("{}\n", wrap(entry.explain, 88));
    println!("raised by: {}", entry.stage.title());
    Ok(())
}

fn wrap(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for word in text.split_whitespace() {
        if col > 0 && col + 1 + word.len() > width {
            out.push('\n');
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}
