//! The documentation gates.
//!
//! Generated documentation has one failure mode that hand-written documentation does not: it can
//! be *silently* stale, because nobody edited it and nobody noticed. Three properties close that,
//! and all three are deterministic — no cluster, no network, no Docker.
//!
//! 1. **The error index is complete in both directions.** Every `"Bnnnn"` literal in the
//!    workspace's non-test source is in [`beck_diag::index::INDEX`], and every entry in the index
//!    is a code the compiler can emit. A new diagnostic without an index entry fails here.
//! 2. **The checked-in reference is what the compiler generates.** `beck doc reference --check`
//!    regenerates every page in memory and compares. A change to the placement solver's discharge
//!    rule, to the CLI, or to the prelude shows up as a diff in `docs/reference/`.
//! 3. **`beck doc` runs over the corpus.** Every program that compiles documents, which is the
//!    only way to know the generator does not panic on a shape nobody wrote a unit test for.
//!
//! [`docs/20-phase-2-report.md`](../../../../docs/20-phase-2-report.md) §20.4 item 8 is why these
//! are tests rather than workflow steps: the Phase 1 CI workflow had never run, and nothing said
//! so. A `cargo test` gate cannot be silently skipped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn compiler_root() -> PathBuf {
    // .../compiler/crates/beck-cli → .../compiler
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .to_path_buf()
}

fn repo_root() -> PathBuf {
    compiler_root()
        .parent()
        .expect("the workspace lives in the repository")
        .to_path_buf()
}

/// Every `.rs` file in the workspace that is not a test.
///
/// Test files are excluded because a snapshot suite naming `B0341` is asserting on a diagnostic,
/// not raising one — and `index.rs` is excluded because it is the index itself.
fn compiler_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&compiler_root().join("crates"), &mut out);
    out.retain(|p| {
        let s = p.to_string_lossy();
        !s.contains("/tests/") && !s.ends_with("index.rs")
    });
    out.sort();
    assert!(out.len() > 20, "the source listing is wrong, not the repo");
    out
}

/// The codes the compiler can actually emit, read out of its source.
///
/// A `#[cfg(test)]` module is cut off at its attribute: a unit test that asserts on a code is not
/// a site that raises one.
fn emitted_codes() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for path in compiler_sources() {
        let src = std::fs::read_to_string(&path).expect("a source file is readable");
        let src = match src.find("#[cfg(test)]") {
            Some(i) => &src[..i],
            None => &src[..],
        };
        let bytes: Vec<char> = src.chars().collect();
        for (i, c) in bytes.iter().enumerate() {
            // `"Bnnnn"` — the literal form every diagnostic site writes.
            if *c != '"' || i + 6 >= bytes.len() || bytes[i + 1] != 'B' || bytes[i + 6] != '"' {
                continue;
            }
            let digits: String = bytes[i + 2..i + 6].iter().collect();
            if digits.chars().all(|d| d.is_ascii_digit()) {
                out.insert(format!("B{digits}"));
            }
        }
    }
    out
}

#[test]
fn every_code_the_compiler_emits_is_in_the_error_index() {
    let emitted = emitted_codes();
    let indexed: BTreeSet<String> = beck_diag::index::INDEX
        .iter()
        .map(|e| e.code.to_string())
        .collect();

    let missing: Vec<&String> = emitted.difference(&indexed).collect();
    assert!(
        missing.is_empty(),
        "these codes are raised by the compiler and are not in the error index — add them to \
         `beck-diag/src/index.rs`, then regenerate `docs/reference/`:\n  {missing:?}"
    );

    let orphaned: Vec<&String> = indexed.difference(&emitted).collect();
    assert!(
        orphaned.is_empty(),
        "these codes are in the error index and nothing raises them — an entry cannot outlive its \
         code:\n  {orphaned:?}"
    );

    // A floor, so that a scan which silently matched nothing could not pass this test.
    assert!(emitted.len() > 80, "only found {} codes", emitted.len());
}

fn beck(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(args)
        .current_dir(compiler_root())
        .output()
        .expect("the compiler binary runs")
}

#[test]
fn the_checked_in_reference_is_what_the_compiler_generates() {
    let out = repo_root().join("docs").join("reference");
    let result = beck(&[
        "doc",
        "reference",
        "--out",
        &out.to_string_lossy(),
        "--check",
    ]);
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn the_reference_names_every_page_it_writes() {
    // The index page is generated from the same list the pages are, so a page that stops being
    // written stops being linked. What this asserts is that the list is not empty and the links
    // resolve — the `docs` workflow's link gate covers the rest.
    let dir = repo_root().join("docs").join("reference");
    let readme = std::fs::read_to_string(dir.join("README.md")).expect("the index is checked in");
    for page in ["errors", "cli", "effects", "prelude", "forms"] {
        assert!(
            readme.contains(&format!("({page}.md)")),
            "the reference index does not link {page}.md"
        );
        assert!(
            dir.join(format!("{page}.md")).exists(),
            "{page}.md is linked and not checked in"
        );
    }
}

#[test]
fn beck_doc_documents_every_corpus_program() {
    let corpus = compiler_root().join("corpus");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("the corpus is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    programs.sort();
    assert!(programs.len() > 20, "the corpus listing is wrong");

    for path in &programs {
        let rel = format!("corpus/{}", path.file_name().unwrap().to_string_lossy());
        for format in ["md", "html", "json"] {
            let out = beck(&["doc", "module", &rel, "--format", format, "--stdout"]);
            assert!(
                out.status.success(),
                "`beck doc module {rel} --format {format}` failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                out.stdout.len() > 100,
                "`beck doc module {rel} --format {format}` produced nothing"
            );
        }
    }
}

#[test]
fn the_documented_example_documents_every_name_it_publishes() {
    // The dogfood: `examples/documented.beck` exists to be the thing `beck doc` renders, so a
    // published name without a doc comment there is a gap in the example rather than in a program.
    let out = beck(&[
        "doc",
        "module",
        "examples/documented.beck",
        "--format",
        "json",
        "--stdout",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`beck doc --format json` emits valid JSON");

    let mut undocumented = Vec::new();
    for group in ["types", "items"] {
        for entry in json[group].as_array().expect("an array") {
            if entry["doc"].is_null() {
                undocumented.push(entry["name"].as_str().unwrap_or("?").to_string());
            }
        }
    }
    assert!(
        undocumented.is_empty(),
        "examples/documented.beck publishes names with no `##` comment: {undocumented:?}"
    );
}

#[test]
fn a_doc_comment_survives_beck_fmt() {
    // `beck fmt` prints from the AST, so an ordinary comment cannot survive it. A doc comment is
    // metadata on the node and must — otherwise formatting a documented module deletes its
    // documentation, which is a worse failure than not having doc comments at all.
    let out = beck(&["fmt", "examples/documented.beck"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let formatted = String::from_utf8_lossy(&out.stdout);
    for line in [
        "## An amount of money in minor units — pence, cents — never a float.",
        "    ## How many. A line with a quantity of zero is legal and contributes nothing.",
        "    ## Carriage. Free delivery is `Shipping(Money(0))` rather than the absence of this variant.",
    ] {
        assert!(
            formatted.contains(line),
            "`beck fmt` dropped a doc comment:\n  {line}\n--- formatted ---\n{formatted}"
        );
    }
    // And the ordinary comments are gone, as they always were: the distinction is the point.
    assert!(
        !formatted.contains("# A documented library"),
        "an ordinary comment should not survive `beck fmt`"
    );
}

#[test]
fn every_link_in_the_generated_site_resolves() {
    // The same property the `docs` workflow's `links` job asserts for markdown, for the HTML the
    // `publish` job uploads. It found a real one: a module page is written a directory below the
    // reference pages, so the shell's header link back to the index is not the same href from
    // both. Built into a temporary directory in the layout the workflow uses.
    //
    // Built with `--repo` because the workflow builds it with `--repo`, and the assertion at the
    // end is that the link back to the repository reaches *every* page rather than the one page
    // whoever added it happened to look at.
    let site = std::env::temp_dir().join(format!("beck-doc-site-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&site);
    let module_dir = site.join("module");
    let guide_dir = site.join("guide");

    let out = beck(&[
        "doc",
        "reference",
        "--out",
        &site.to_string_lossy(),
        "--format",
        "html",
        "--repo",
        REPO,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for source in ["examples/todo.beck", "examples/documented.beck"] {
        let out = beck(&[
            "doc",
            "module",
            source,
            "--out",
            &module_dir.to_string_lossy(),
            "--format",
            "html",
            "--repo",
            REPO,
        ]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // The index links to the guide, so the guide is part of the site rather than beside it: this
    // test is what says the workflow cannot publish one without the other.
    let guide = repo_root().join("docs/86-getting-started.md");
    let out = beck(&[
        "doc",
        "guide",
        &guide.to_string_lossy(),
        "--out",
        &guide_dir.to_string_lossy(),
        "--link-base",
        "https://example.invalid/repo/docs",
        "--repo",
        REPO,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut pages: Vec<PathBuf> = Vec::new();
    for dir in [&site, &module_dir, &guide_dir] {
        for entry in std::fs::read_dir(dir)
            .expect("the site was written")
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "html") {
                pages.push(path);
            }
        }
    }
    assert!(pages.len() >= 8, "only {} pages built", pages.len());

    // Every page kind the workflow publishes links back to the repository — the reference pages,
    // the index, the guide and the module pages are four different call sites into the shell, and
    // a fifth added later is what this is here to catch.
    let mut without: Vec<String> = Vec::new();
    for page in &pages {
        let text = std::fs::read_to_string(page).expect("a page is readable");
        if !text.contains(&format!("href=\"{REPO}\"")) {
            without.push(
                page.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
            );
        }
    }
    assert!(
        without.is_empty(),
        "{} of {} published pages carry no link back to the repository: {}",
        without.len(),
        pages.len(),
        without.join(", ")
    );

    // `api/` is rustdoc's, built by a separate step; a link into it is checked by the workflow's
    // own `test -f`, not here.
    let mut broken = Vec::new();
    for page in &pages {
        let text = std::fs::read_to_string(page).expect("a page is readable");
        for target in hrefs(&text) {
            if target.starts_with("http") || target.starts_with("api/") {
                continue;
            }
            let resolved = page.parent().expect("a page has a parent").join(&target);
            if !resolved.exists() {
                broken.push(format!(
                    "{}: {target}",
                    page.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&site);
    assert!(
        broken.is_empty(),
        "the generated site has links that do not resolve:\n  {}",
        broken.join("\n  ")
    );
}

/// The repository the site under test is generated from. Not this project's URL: what is asserted
/// is that whatever `--repo` is given reaches every page, and a real URL would pass that assertion
/// for a page that had it hard-coded.
const REPO: &str = "https://example.invalid/owner/repo";

/// Every `href` in a page, with any fragment stripped. Deliberately not an HTML parser: the pages
/// are generated by one function, and what this needs to see is exactly what that function writes.
fn hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("href=\"") {
        rest = &rest[i + 6..];
        let Some(end) = rest.find('"') else { break };
        let target = rest[..end].split('#').next().unwrap_or("").to_string();
        if !target.is_empty() {
            out.push(target);
        }
        rest = &rest[end..];
    }
    out
}

#[test]
fn explain_error_answers_for_every_indexed_code() {
    // One process per code would be 92 processes; three is enough to show the path works, and the
    // index's own unit tests cover the table.
    for code in ["B0341", "b0341", "B0707"] {
        let out = beck(&["explain", "error", code]);
        assert!(
            out.status.success(),
            "`beck explain error {code}` failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("raised by:"),
            "`beck explain error {code}` printed no entry"
        );
    }
    let out = beck(&["explain", "error", "B9999"]);
    assert!(!out.status.success(), "an unknown code should be an error");
}

/// Every relative link in every checked-in markdown file lands on a file that exists.
///
/// `.github/workflows/docs.yml` has a job that does this, and that was the whole problem: moving
/// `beck-rt/src/diff.rs` into `beck-core` broke a link in a report, `cargo test --workspace` was
/// green, and CI was the first thing to say so. A gate that only exists in CI is a gate whose
/// feedback arrives after a push — and this one is a `git ls-files` and a regular expression, so
/// there is no reason for it to live there and not here.
///
/// The workflow keeps its copy. Two implementations of a rule this small is cheaper than a rule
/// that is only enforced in one place, and `docs.rs` already asserts that every shell command in
/// the instructions runs.
#[test]
fn every_relative_link_in_the_markdown_lands_on_a_file_that_exists() {
    let root = repo_root();
    let listed = std::process::Command::new("git")
        .current_dir(&root)
        .args(["ls-files", "*.md"])
        .output()
        .expect("git lists the tracked markdown");
    let files: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    assert!(
        !files.is_empty(),
        "no markdown files found — the listing is wrong, not the repository"
    );

    let mut bad = Vec::new();
    for file in &files {
        let path = root.join(file);
        let text = std::fs::read_to_string(&path).expect("a tracked file is readable");
        let text = strip_code(&text);
        let base = path.parent().expect("a file has a directory").to_path_buf();
        for target in markdown_links(&text) {
            if target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with('#')
            {
                continue;
            }
            let named = target.split('#').next().unwrap_or(&target);
            if !base.join(named).exists() {
                bad.push(format!("{file}: {target}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} link(s) name a file that does not exist:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Fenced blocks and inline spans are prose about code, not links to follow.
fn strip_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut ticks = false;
        for c in line.chars() {
            if c == '`' {
                ticks = !ticks;
            } else if !ticks {
                out.push(c);
            }
        }
        out.push('\n');
    }
    out
}

/// `[text](target)` — the target of every inline link.
fn markdown_links(text: &str) -> Vec<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '[' {
            if let Some(close) = (i..bytes.len()).find(|&j| bytes[j] == ']') {
                if close + 1 < bytes.len() && bytes[close + 1] == '(' {
                    if let Some(end) = (close + 2..bytes.len()).find(|&j| bytes[j] == ')') {
                        let target: String = bytes[close + 2..end].iter().collect();
                        if !target.contains(char::is_whitespace) {
                            out.push(target);
                        }
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// A relative link out of a rustdoc page resolves to the file it names.
///
/// The four properties above are about the *generated Beck reference*. This one is about the other
/// generated documentation — `cargo doc` — and it closes a gap nothing was watching: a `//!` header
/// linking to `docs/04-compiler-architecture.md` is rendered into a page whose URL depth is the
/// **module's**, not the file's, so `beck_core/index.html` and `beck_core/prelude/index.html` need a
/// different number of `../` for the same target. Every submodule in the tree was one level short,
/// and `beck-core/src/digest.rs` was the single file that had it right.
///
/// `cargo doc` does not check these — it verifies intra-doc links, and a relative path to a
/// markdown file outside the output tree is not one — so the arithmetic is done here instead:
/// `target/doc/<crate>/<module path>/` is four levels below the repository root plus one per module.
#[test]
fn a_relative_link_out_of_a_rustdoc_page_lands_on_the_file_it_names() {
    let mut checked = 0usize;
    for path in compiler_sources() {
        let Ok(rest) = path.strip_prefix(compiler_root().join("crates")) else {
            continue;
        };
        let parts: Vec<&str> = rest.iter().filter_map(|p| p.to_str()).collect();
        let Some(src) = parts.iter().position(|p| *p == "src") else {
            continue;
        };
        let module = &parts[src + 1..];
        // `target/doc/<crate>/` is four levels under the repository root; each module below the
        // crate root adds one. `lib.rs`/`main.rs` are the crate root; `foo/mod.rs` is one module.
        let depth = match module {
            ["lib.rs"] | ["main.rs"] => 0,
            _ if module.last() == Some(&"mod.rs") => module.len() - 1,
            _ => module.len(),
        };
        let text = std::fs::read_to_string(&path).expect("readable");
        for link in text.split("](").skip(1) {
            let Some(target) = link.split(')').next() else {
                continue;
            };
            if !target.starts_with("../") || !target.contains("docs/") {
                continue;
            }
            let ups = target.matches("../").count();
            assert_eq!(
                ups,
                4 + depth,
                "{}: `{target}` needs {} levels, not {ups} — \
                 a rustdoc page for this module sits {} below the repository root",
                path.display(),
                4 + depth,
                4 + depth,
            );
            let named = target.trim_start_matches("../");
            assert!(
                repo_root().join(named).exists(),
                "{}: `{target}` names {named}, which does not exist",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(
        checked > 40,
        "the link sweep found almost nothing: {checked}"
    );
}

/// The published guide is the checked file, rendered — not a second copy of it.
///
/// `docs/86-getting-started.md` is gated twice over: `getting_started.rs` compiles and runs every
/// program in it, and this asserts the page the site serves is made from that same file. The two
/// together are what let the site claim a tutorial whose examples work, which is
/// [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) §8.5.4's exit criterion having a
/// prerequisite met rather than a page written.
#[test]
fn the_published_guide_is_the_checked_guide() {
    let source = repo_root().join("docs/86-getting-started.md");
    let markdown = std::fs::read_to_string(&source).expect("the guide is checked in");
    let out = beck(&[
        "doc",
        "guide",
        &source.to_string_lossy(),
        "--stdout",
        "--link-base",
        "https://example.invalid/repo/docs",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let html = String::from_utf8_lossy(&out.stdout);

    // Every fenced block in the file reaches the page. This is the assertion that matters: a
    // renderer that silently drops a block would publish a tutorial with a step missing.
    let fences = markdown
        .lines()
        .filter(|l| l.trim_start().starts_with("```"))
        .count();
    assert!(fences >= 20, "the guide has {fences} fenced blocks");
    assert_eq!(
        html.matches("<pre><code>").count(),
        fences / 2,
        "the rendered guide has a different number of code blocks than the source"
    );

    // Its relative links are rewritten, because `08-roadmap.md` is not a page on a static site.
    assert!(
        html.contains("https://example.invalid/repo/docs/08-roadmap.md"),
        "a relative link was published unrewritten"
    );
    assert!(
        !html.contains("href=\"08-roadmap.md\""),
        "a link was published as written rather than as a URL"
    );
    // And a link that walks out of the guide's directory lands beside it rather than inside it.
    assert!(
        html.contains("https://example.invalid/repo/compiler/")
            || !markdown.contains("](../compiler/"),
        "a `..` link was resolved without leaving the guide's directory"
    );
    // And nothing in it is raw HTML from the source.
    assert!(!html.contains("<script"), "the guide rendered a script tag");
}

/// Every shell command in `AGENTS.md` is one the shell would actually run.
///
/// One deterministic property, and it is the one that bit: an environment assignment whose value
/// contains a space has to be quoted. Unquoted, `RUSTDOCFLAGS=-D warnings cargo doc …` is the
/// assignment `RUSTDOCFLAGS=-D` followed by the command `warnings`, which fails with "command not
/// found" — so the instruction reads as a verification step and performs none. That is worse than a
/// missing step, because whoever follows it believes the check ran (`docs/88` §88.8).
///
/// Deliberately narrow. This does not run the commands — several take minutes and one needs a
/// cluster — and it has no view on whether they are the right commands. It asserts the one thing a
/// reader cannot see by looking.
#[test]
fn every_shell_command_in_the_instructions_runs() {
    let text = std::fs::read_to_string(repo_root().join("AGENTS.md")).expect("AGENTS.md");
    let mut bad = Vec::new();
    // Commands appear in backticks in prose rather than in fenced blocks, so the span between two
    // backticks is the unit — the same thing a reader would copy.
    for span in text.split('`').skip(1).step_by(2) {
        let Some(assignment) = span.split_whitespace().next() else {
            continue;
        };
        let Some((name, value)) = assignment.split_once('=') else {
            continue;
        };
        // An environment assignment: an upper-case name before the first `=`, and something after
        // it that the rest of the line continues.
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        let rest = span[assignment.len()..].trim_start();
        if rest.is_empty() {
            continue;
        }
        let quoted = value.starts_with('"') || value.starts_with('\'');
        // A flag as the value means the argument after it belongs to the flag, not to the command:
        // `X=-D warnings cargo doc` runs `warnings`, not `cargo`.
        if !quoted && value.starts_with('-') {
            bad.push(span.to_string());
        }
    }
    assert!(
        bad.is_empty(),
        "AGENTS.md gives a command whose environment assignment is unquoted, so the shell would \
         run the next word as the command rather than passing it as part of the value:\n  {}",
        bad.join("\n  ")
    );
}

/// The leading number of `104-the-release-and-the-installer-report.md`, or of `104 — the release`.
///
/// `None` where the text does not begin with a number: `README.md` is an index rather than a
/// numbered document, and two reports open with prose instead of their number.
fn leading_number(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A document's sections are numbered for the document they are in.
///
/// `docs/` numbers a document by its filename and its sections `§N.1`, `§N.2` … of that same
/// number. That is what makes `§104.7` a reference a reader can follow — and, because every
/// document's headings carry its own filename's number, what makes a section number unique across
/// the directory without anything having to check for a collision.
///
/// The gap this is the shape of is the one `AGENTS.md` tells you to expect: the counter collides
/// whenever two branches write a report, so a document gets **renumbered on merge** — and a rename
/// moves the filename while leaving the headings where they were.
/// `104-the-release-and-the-installer-report.md` landed carrying `101`, the number
/// `101-the-heap-report.md` already had. So `§101.5` named a section in each of two documents, and
/// the twenty-nine references to `§104.x` written in `README.md`, `AGENTS.md`, `CHANGELOG.md`,
/// `release/README.md` and six other documents named a heading that did not exist. The prose inside
/// the report had been corrected to `§104.x` and the headings had not, which is why reading it did
/// not show the fault: the half a reader sees was right.
///
/// A reference is checked from the *defining* end rather than the citing end on purpose. Reports
/// are history and are not edited to track a later change, so a rule that every `§N.M` in the
/// repository resolves would be enforced against files nobody may correct. This one is enforced
/// against the document that owns the number, which is the file a rename is free to fix.
#[test]
fn a_documents_sections_are_numbered_for_the_document_they_are_in() {
    let root = repo_root();
    let listed = Command::new("git")
        .current_dir(&root)
        .args(["ls-files", "docs/*.md"])
        .output()
        .expect("git lists the tracked documents");
    // `docs/adr/` numbers decisions on its own scheme and `docs/reference/` is generated, so the
    // convention this asserts is the one directory it belongs to.
    let files: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .filter(|f| f.matches('/').count() == 1)
        .map(str::to_string)
        .collect();
    assert!(
        files.len() > 50,
        "{} documents found — the listing is wrong, not the repository",
        files.len()
    );

    let mut bad = Vec::new();
    let mut checked = 0usize;
    for file in &files {
        let name = file
            .rsplit('/')
            .next()
            .expect("a path has a last component");
        let Some(number) = leading_number(name) else {
            continue;
        };
        checked += 1;
        let text = std::fs::read_to_string(root.join(file)).expect("a tracked file is readable");

        if let Some(titled) = text
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("# "))
            .and_then(|rest| leading_number(rest.trim_start()))
        {
            if titled != number {
                bad.push(format!(
                    "{file}: the title is numbered {titled} and the file is {number}"
                ));
            }
        }

        for line in text.lines() {
            let Some(rest) = line.strip_prefix("##") else {
                continue;
            };
            let rest = rest.trim_start_matches('#').trim_start();
            // A heading opening with `§` is *citing* a section rather than declaring one —
            // `### §24.6's O(n), paid once instead of removed` is a subheading about another
            // document, and the sign is what says so: nothing in `docs/` declares a section with it.
            if rest.starts_with('§') {
                continue;
            }
            // `## 104.7 What has been executed` is a numbered section; `## Phase 4 — …` is not, and
            // neither is a heading that merely opens with a figure — the digit after the point is
            // what separates a section number from `## 1.5× the evaluator`.
            let Some((head, tail)) = rest.split_once('.') else {
                continue;
            };
            if !tail.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let Some(section) = leading_number(head) else {
                continue;
            };
            if section != number {
                bad.push(format!(
                    "{file}: a heading is numbered {section} — `{}`",
                    line.trim()
                ));
            }
        }
    }

    assert!(checked > 50, "only {checked} numbered documents were read");
    assert!(
        bad.is_empty(),
        "{} heading(s) carry a number that is not their document's, so a `§N.M` reference to them \
         resolves to the wrong document or to nothing:\n  {}",
        bad.len(),
        bad.join("\n  ")
    );
}
