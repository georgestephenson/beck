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
