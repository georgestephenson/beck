//! `beck doc` — a module's reference documentation, derived from the module.
//!
//! [`docs/16-packages-and-ecosystem.md`](../../../../../docs/16-packages-and-ecosystem.md) §16.2 names
//! the model: "documentation generated from types and doc-comments for every published version,
//! automatically". Half of that has existed since Phase 2 — [`crate::iface::Interface`] is every
//! published name's type, effect row and placement, and it is derived rather than declared. The
//! other half is [`beck_syntax::doc`].
//!
//! # What is generated, and what is written
//!
//! | Part of the page | Where it comes from |
//! |---|---|
//! | Signature — parameters, result, type arguments | Inference. Nobody writes it. |
//! | **Effects** — what a name performs | The inferred row (§3.2), closed at the boundary |
//! | **Placement** — which tier it runs on | The solver (§3.4), not an annotation |
//! | Types, fields, variants | The module's own declarations |
//! | Prose | The `##` doc comment, if there is one |
//!
//! Three of those five are things a language with an effect system and a placement solver knows and
//! a hand-written reference page would get wrong within a week. That is the argument for generating
//! this rather than writing it: **a doc comment can go stale, a signature cannot**.
//!
//! # What it deliberately does not do
//!
//! * **No prose is invented.** A name with no doc comment is rendered with its signature and
//!   nothing else, and [`Docs::documented`] counts the difference so a coverage number is a
//!   measurement rather than an impression.
//! * **No cross-module linking.** A type from an imported module renders as its name. The Mere
//!   (§16.2) is where a link between published versions would live, and it is not built.
//! * **Markdown in a doc comment is passed through, not parsed.** The HTML renderer escapes and
//!   preserves paragraph breaks; it is not a Markdown implementation.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use beck_syntax::{sym, Node};

use crate::check::Program;
use crate::iface::{Interface, Item, Kind};
use crate::ty::{Tier, TyDecl};

/// Collect every doc comment in a module's top-level items, keyed by the name it documents.
///
/// The key for a nested declaration is qualified — `Todo.text`, `Event.Toggled` — so one flat map
/// serves the whole module and nothing has to be threaded through the checker.
pub fn collect_docs(items: &[&Node]) -> BTreeMap<Arc<str>, Arc<str>> {
    let mut out = BTreeMap::new();
    for item in items {
        // The doc comment is on the outermost node, which is the `decorate` when there is one
        // (`beck_syntax::doc`), and the name is on the innermost.
        let doc = item.meta.doc.clone();
        let mut inner = *item;
        while inner.is_form(sym::DECORATE) {
            inner = &inner.args[1];
        }
        let Some(name) = declared_name(inner) else {
            continue;
        };
        if let Some(d) = doc.or_else(|| inner.meta.doc.clone()) {
            out.insert(name.clone(), d);
        }
        // Model fields and union variants carry their own, and are named under their type.
        if inner.is_form(sym::MODEL) || inner.is_form(sym::UNION) {
            for member in &inner.args[1..] {
                let Some(d) = member.meta.doc.clone() else {
                    continue;
                };
                let Some(mname) = member.args.first().and_then(|a| a.as_var()) else {
                    continue;
                };
                out.insert(Arc::from(format!("{name}.{}", mname.as_str())), d);
            }
        }
    }
    out
}

fn declared_name(inner: &Node) -> Option<&Arc<str>> {
    const NAMED: &[&str] = &[
        sym::DEF,
        sym::LET,
        sym::VAR,
        sym::MODEL,
        sym::UNION,
        sym::TYPE,
        sym::NEWTYPE,
        sym::TRAIT,
    ];
    if !NAMED.iter().any(|f| inner.is_form(f)) {
        return None;
    }
    inner.args.first().and_then(|a| a.as_var()).map(|s| &s.name)
}

/// One documented name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: Arc<str>,
    /// `def` / `signal` — what the reader is looking at.
    pub kind: &'static str,
    /// The signature as it is written in the language: `add(a: Int, b: Int) -> Int`.
    pub signature: String,
    /// The inferred effect row, as atom names. Empty means pure.
    pub effects: Vec<String>,
    pub tier: Tier,
    pub doc: Option<Arc<str>>,
}

/// One documented type, with its fields or variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeEntry {
    pub name: Arc<str>,
    /// `model` / `union` / `newtype` / `type`.
    pub kind: &'static str,
    pub declaration: String,
    pub doc: Option<Arc<str>>,
    /// Field or variant name, its rendered type, and its own doc comment.
    pub members: Vec<(Arc<str>, String, Option<Arc<str>>)>,
}

/// A module's reference documentation.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Docs {
    pub module: String,
    /// The interface digest this page was generated from — the same value
    /// [`crate::iface::Interface::digest`] publishes, so a page can be matched to a contract.
    pub digest: String,
    pub types: Vec<TypeEntry>,
    pub items: Vec<Entry>,
}

impl Docs {
    /// Derive a module's documentation from the checked, placed program.
    pub fn of(program: &Program) -> Docs {
        Docs::of_interface(&Interface::of(program), &program.docs)
    }

    /// The page for an interface that has already been computed.
    ///
    /// A module that **imports** another is checked as part of a project, and the program that
    /// comes out of the slicer is every module merged — which is right for slicing and wrong for a
    /// documentation page, because `beck doc` on one module would then publish the names of every
    /// module beneath it. [`Project::interface`](crate::project::Project) is the root module's own
    /// contract, and it is what a page is of. `docs/56` §56.5 is where that was found.
    ///
    /// The doc-comment map is the *program's*, because a comment is looked up by the name it
    /// documents and the interface selects which names those are.
    pub fn of_interface(iface: &Interface, comments: &BTreeMap<Arc<str>, Arc<str>>) -> Docs {
        let types = iface
            .types
            .iter()
            .map(|t| type_entry(t, comments))
            .collect();
        let items = iface.items.iter().map(|i| entry(i, comments)).collect();
        Docs {
            module: iface.module.clone(),
            digest: iface.digest(),
            types,
            items,
        }
    }

    /// How many published names carry a doc comment, and how many there are.
    ///
    /// Reported rather than enforced: a coverage gate that fails a build is how a codebase ends up
    /// with `## the id` on a field called `id`. The number is here so it can be looked at.
    pub fn documented(&self) -> (usize, usize) {
        let all = self.items.len() + self.types.len();
        let with = self.items.iter().filter(|i| i.doc.is_some()).count()
            + self.types.iter().filter(|t| t.doc.is_some()).count();
        (with, all)
    }
}

fn entry(i: &Item, docs: &BTreeMap<Arc<str>, Arc<str>>) -> Entry {
    let (kind, signature) = match &i.kind {
        Kind::Function {
            typarams,
            params,
            ret,
        } => (
            "def",
            format!(
                "{}{}({}) -> {ret}",
                i.name,
                // A generic definition publishes what it quantifies over (§3.6), so the page shows
                // it: `pair[T](a: T, b: T)` is a different contract from `pair(a: T, b: T)`.
                if typarams.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", typarams.join(", "))
                },
                params
                    .iter()
                    .map(|(n, t)| format!("{n}: {t}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Kind::Signal { ty } => ("signal", format!("{}: {ty}", i.name)),
    };
    Entry {
        name: i.name.clone(),
        kind,
        signature,
        effects: i.effects.iter().map(|e| e.name().to_string()).collect(),
        tier: i.tier,
        doc: docs.get(&i.name).cloned(),
    }
}

fn type_entry(t: &TyDecl, docs: &BTreeMap<Arc<str>, Arc<str>>) -> TypeEntry {
    let name = t.name().clone();
    let member_doc = |m: &str| {
        docs.get(&Arc::from(format!("{name}.{m}")) as &Arc<str>)
            .cloned()
    };
    let (kind, declaration, members) = match t {
        TyDecl::Model { fields, .. } => (
            "model",
            format!("model {name}"),
            fields
                .iter()
                .map(|(f, ty)| (f.clone(), format!("{ty}"), member_doc(f)))
                .collect(),
        ),
        TyDecl::Union { variants, .. } => (
            "union",
            format!("union {name}"),
            variants
                .iter()
                .map(|v| {
                    let fields = v
                        .fields
                        .iter()
                        .map(|(f, ty)| format!("{f}: {ty}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let rendered = if v.fields.is_empty() {
                        v.name.to_string()
                    } else {
                        format!("{}({fields})", v.name)
                    };
                    (v.name.clone(), rendered, member_doc(&v.name))
                })
                .collect(),
        ),
        TyDecl::Newtype { inner, .. } => (
            "newtype",
            format!("type {name} = newtype[{inner}]"),
            Vec::new(),
        ),
        TyDecl::Alias { ty, .. } => ("type", format!("type {name} = {ty}"), Vec::new()),
    };
    TypeEntry {
        name: name.clone(),
        kind,
        declaration,
        doc: docs.get(&name).cloned(),
        members,
    }
}

// ---------------------------------------------------------------------------- renderers

impl Docs {
    /// Markdown — the form that is checked in and diffed.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Module `{}`\n", self.module);
        let (with, all) = self.documented();
        let _ = writeln!(
            out,
            "Generated by `beck doc`. Signatures, effects and placements are derived from the \
             module and are not written by hand; prose comes from `##` doc comments.\n"
        );
        let _ = writeln!(
            out,
            "- Interface digest: `{}`\n- Documented: {with}/{all} published names\n",
            self.digest
        );

        if !self.types.is_empty() {
            let _ = writeln!(out, "## Types\n");
            for t in &self.types {
                let _ = writeln!(out, "### `{}`\n", t.name);
                let _ = writeln!(out, "```beck\n{}\n```\n", t.declaration);
                if let Some(d) = &t.doc {
                    let _ = writeln!(out, "{d}\n");
                }
                if !t.members.is_empty() {
                    let heading = if t.kind == "union" {
                        "Variant"
                    } else {
                        "Field"
                    };
                    let _ = writeln!(out, "| {heading} | |\n|---|---|");
                    for (m, ty, doc) in &t.members {
                        // A variant renders whole (`Added(id: Id)`); a field is name and type.
                        let shown = if t.kind == "union" {
                            ty.clone()
                        } else {
                            format!("{m}: {ty}")
                        };
                        let _ = writeln!(
                            out,
                            "| `{}` | {} |",
                            shown.replace('|', "\\|"),
                            doc.as_deref().unwrap_or("").replace('\n', " ")
                        );
                    }
                    out.push('\n');
                }
            }
        }

        if !self.items.is_empty() {
            let _ = writeln!(out, "## Names\n");
            let _ = writeln!(out, "| Name | Runs on | Effects |\n|---|---|---|");
            for i in &self.items {
                let _ = writeln!(
                    out,
                    "| [`{}`](#{}) | `{}` | {} |",
                    i.name,
                    anchor(&i.name),
                    i.tier.name(),
                    effects_md(&i.effects)
                );
            }
            out.push('\n');
            for i in &self.items {
                let _ = writeln!(out, "### `{}`\n", i.name);
                let _ = writeln!(out, "```beck\n{}\n```\n", i.signature);
                let _ = writeln!(
                    out,
                    "*{}* — runs on `{}`, performs {}.\n",
                    i.kind,
                    i.tier.name(),
                    effects_md(&i.effects)
                );
                if let Some(d) = &i.doc {
                    let _ = writeln!(out, "{d}\n");
                }
            }
        }
        out
    }

    /// JSON — the form a tool consumes. Hand-written rather than derived, so the shape is a
    /// decision in this file rather than a consequence of the Rust field names.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{{\n  \"module\": {},\n  \"digest\": {},\n  \"types\": [",
            json_str(&self.module),
            json_str(&self.digest)
        );
        for (n, t) in self.types.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "\n    {{\"name\": {}, \"kind\": {}, \"declaration\": {}, \"doc\": {}, \"members\": [",
                json_str(&t.name),
                json_str(t.kind),
                json_str(&t.declaration),
                json_opt(&t.doc)
            );
            for (m, (name, ty, doc)) in t.members.iter().enumerate() {
                if m > 0 {
                    out.push(',');
                }
                let _ = write!(
                    out,
                    "{{\"name\": {}, \"type\": {}, \"doc\": {}}}",
                    json_str(name),
                    json_str(ty),
                    json_opt(doc)
                );
            }
            out.push_str("]}");
        }
        out.push_str("\n  ],\n  \"items\": [");
        for (n, i) in self.items.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "\n    {{\"name\": {}, \"kind\": {}, \"signature\": {}, \"tier\": {}, \"effects\": [{}], \"doc\": {}}}",
                json_str(&i.name),
                json_str(i.kind),
                json_str(&i.signature),
                json_str(i.tier.name()),
                i.effects
                    .iter()
                    .map(|e| json_str(e))
                    .collect::<Vec<_>>()
                    .join(", "),
                json_opt(&i.doc)
            );
        }
        out.push_str("\n  ]\n}\n");
        out
    }
}

/// The site shell: one stylesheet, inline, no fonts and no scripts.
///
/// The whole generated site is static files that open from `file://` as readily as from a server,
/// which is what keeps the documentation reviewable in a pull request and buildable offline.
/// [`docs/07-dependencies.md`](../../../../../docs/07-dependencies.md) lists mdBook for the eventual
/// book; a reference page needs less than a book does, and less is one fewer dependency.
///
/// `home` is the href of the site index *relative to this page*. It is a parameter rather than a
/// constant because a module page is written one directory down from the reference pages, and a
/// header link that 404s from half the site is worse than no header link.
pub fn page(title: &str, home: &str, breadcrumb: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>{CSS}</style>\n</head>\n<body>\n\
         <header><a href=\"{}\">beck</a>{}</header>\n<main>\n{body}</main>\n\
         <footer>Generated by <code>beck doc</code>. Signatures, effects and placements are \
         derived from the program.</footer>\n</body>\n</html>\n",
        escape(title),
        escape(home),
        breadcrumb,
    )
}

/// Where a module page's header link points: module pages are written into `module/` beneath the
/// site root, so the index is one level up.
pub const MODULE_PAGE_HOME: &str = "../index.html";

/// Where a reference page's header link points — they sit at the site root, beside the index.
pub const REFERENCE_PAGE_HOME: &str = "index.html";

const CSS: &str = "\
:root{color-scheme:light dark;--fg:#1a1a1a;--bg:#fff;--muted:#5a6270;--line:#d8dde5;--code:#f5f6f8;--link:#0a5aa8}\
@media(prefers-color-scheme:dark){:root{--fg:#e6e8ec;--bg:#14161a;--muted:#9aa3b2;--line:#2c313a;--code:#1c1f26;--link:#79b8ff}}\
*{box-sizing:border-box}\
body{margin:0;font:16px/1.6 system-ui,-apple-system,Segoe UI,Roboto,sans-serif;color:var(--fg);background:var(--bg)}\
header,footer{padding:.75rem 1.25rem;border-bottom:1px solid var(--line);font-size:.9rem;color:var(--muted)}\
footer{border-bottom:none;border-top:1px solid var(--line);margin-top:3rem}\
header a{color:var(--link);text-decoration:none;font-weight:600}\
main{max-width:52rem;margin:0 auto;padding:1.5rem 1.25rem 4rem}\
h1{font-size:1.75rem;margin:1rem 0 .25rem}h2{font-size:1.3rem;margin:2.5rem 0 .5rem;padding-bottom:.3rem;border-bottom:1px solid var(--line)}\
h3{font-size:1.05rem;margin:2rem 0 .4rem;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}\
code,pre{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.875em}\
pre{background:var(--code);border:1px solid var(--line);border-radius:6px;padding:.7rem .9rem;overflow-x:auto}\
code:not(pre code){background:var(--code);border-radius:3px;padding:.1em .35em}\
table{border-collapse:collapse;width:100%;margin:.75rem 0;display:block;overflow-x:auto}\
th,td{border:1px solid var(--line);padding:.35rem .6rem;text-align:left;vertical-align:top}\
th{background:var(--code);font-weight:600}\
a{color:var(--link)}.muted{color:var(--muted)}\
.tag{display:inline-block;background:var(--code);border:1px solid var(--line);border-radius:999px;padding:.05rem .55rem;font-size:.78rem;font-family:ui-monospace,monospace;margin-right:.3rem}\
";

/// Escape text for HTML. The one place `&`, `<` and `>` are handled, so no renderer has to
/// remember to.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// A doc comment as HTML: escaped, with a blank line starting a new paragraph.
///
/// Not a Markdown parser, and does not pretend to be one — the module docs above say so.
pub fn prose(doc: &str) -> String {
    doc.split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", escape(p.trim())))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Docs {
    /// HTML — the form that is published.
    pub fn to_html(&self) -> String {
        let mut b = String::new();
        let (with, all) = self.documented();
        let _ = writeln!(b, "<h1>Module <code>{}</code></h1>", escape(&self.module));
        let _ = writeln!(
            b,
            "<p class=\"muted\">Interface digest <code>{}</code> · {with}/{all} published names \
             documented</p>",
            escape(&self.digest)
        );

        if !self.types.is_empty() {
            b.push_str("<h2>Types</h2>\n");
            for t in &self.types {
                let _ = write!(
                    b,
                    "<h3 id=\"{}\">{}</h3>\n<pre><code>{}</code></pre>\n",
                    anchor(&t.name),
                    escape(&t.name),
                    escape(&t.declaration)
                );
                if let Some(d) = &t.doc {
                    b.push_str(&prose(d));
                    b.push('\n');
                }
                if !t.members.is_empty() {
                    let heading = if t.kind == "union" {
                        "Variant"
                    } else {
                        "Field"
                    };
                    let _ = writeln!(b, "<table><tr><th>{heading}</th><th></th></tr>");
                    for (m, ty, doc) in &t.members {
                        let shown = if t.kind == "union" {
                            ty.clone()
                        } else {
                            format!("{m}: {ty}")
                        };
                        let _ = write!(
                            b,
                            "<tr><td><code>{}</code></td><td>{}</td></tr>",
                            escape(&shown),
                            doc.as_deref().map(prose).unwrap_or_default()
                        );
                    }
                    b.push_str("</table>\n");
                }
            }
        }

        if !self.items.is_empty() {
            b.push_str(
                "<h2>Names</h2>\n<table><tr><th>Name</th><th>Runs on</th><th>Effects</th></tr>\n",
            );
            for i in &self.items {
                let _ = write!(
                    b,
                    "<tr><td><a href=\"#{}\"><code>{}</code></a></td><td><code>{}</code></td><td>{}</td></tr>",
                    anchor(&i.name),
                    escape(&i.name),
                    i.tier.name(),
                    effects_html(&i.effects)
                );
            }
            b.push_str("</table>\n");
            for i in &self.items {
                let _ = write!(
                    b,
                    "<h3 id=\"{}\">{}</h3>\n<pre><code>{}</code></pre>\n\
                     <p><span class=\"tag\">{}</span><span class=\"tag\">on {}</span>{}</p>\n",
                    anchor(&i.name),
                    escape(&i.name),
                    escape(&i.signature),
                    i.kind,
                    i.tier.name(),
                    effects_html(&i.effects)
                );
                if let Some(d) = &i.doc {
                    b.push_str(&prose(d));
                    b.push('\n');
                }
            }
        }
        page(
            &format!("Module {} — beck", self.module),
            MODULE_PAGE_HOME,
            &format!(" / module <code>{}</code>", escape(&self.module)),
            &b,
        )
    }
}

fn effects_html(effects: &[String]) -> String {
    if effects.is_empty() {
        "<span class=\"muted\">no effects</span>".to_string()
    } else {
        effects
            .iter()
            .map(|e| format!("<span class=\"tag\">{}</span>", escape(e)))
            .collect::<Vec<_>>()
            .join("")
    }
}

fn effects_md(effects: &[String]) -> String {
    if effects.is_empty() {
        "no effects".to_string()
    } else {
        effects
            .iter()
            .map(|e| format!("`{e}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A GitHub-flavoured heading anchor for a name.
pub fn anchor(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_opt(s: &Option<Arc<str>>) -> String {
    match s {
        Some(s) => json_str(s),
        None => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
## One item on the list.
model Todo:
    ## Stable for the life of the item.
    id: Str
    text: Str

## Adds two numbers.
def add(a: Int, b: Int) -> Int:
    return a
";

    /// Through the placement solver, because a page's `runs on` column is the solver's answer.
    fn docs_of(src: &str) -> Docs {
        let (placed, diags, map) = crate::compile_or_library_str("t.beck", src);
        assert!(!diags.has_errors(), "{}", diags.render(&map));
        Docs::of(&placed.expect("a library compiles").program)
    }

    #[test]
    fn a_signature_is_derived_and_the_prose_is_written() {
        let docs = docs_of(SRC);
        let add = docs
            .items
            .iter()
            .find(|i| i.name.as_ref() == "add")
            .unwrap();
        assert_eq!(add.signature, "add(a: Int, b: Int) -> Int");
        assert_eq!(add.doc.as_deref(), Some("Adds two numbers."));
        assert!(add.effects.is_empty(), "{:?}", add.effects);
    }

    #[test]
    fn an_undocumented_name_gets_a_signature_and_no_invented_prose() {
        let docs = docs_of("def f(a: Int) -> Int:\n    return a\n");
        let f = docs.items.iter().find(|i| i.name.as_ref() == "f").unwrap();
        assert_eq!(f.doc, None);
        assert_eq!(f.signature, "f(a: Int) -> Int");
        assert_eq!(docs.documented(), (0, 1));
    }

    #[test]
    fn a_models_fields_carry_their_own_documentation() {
        let docs = docs_of(SRC);
        let todo = docs
            .types
            .iter()
            .find(|t| t.name.as_ref() == "Todo")
            .unwrap();
        assert_eq!(todo.doc.as_deref(), Some("One item on the list."));
        assert_eq!(todo.members[0].0.as_ref(), "id");
        assert_eq!(
            todo.members[0].2.as_deref(),
            Some("Stable for the life of the item.")
        );
        assert_eq!(todo.members[1].2, None, "text is undocumented");
    }

    #[test]
    fn documenting_a_module_does_not_change_its_contract() {
        // The digest is the firewall (§3.6). Adding a doc comment must not move it, or every
        // downstream module would rebuild because somebody wrote a sentence.
        let plain = docs_of("def f(a: Int) -> Int:\n    return a\n");
        let documented = docs_of("## Now documented.\ndef f(a: Int) -> Int:\n    return a\n");
        assert_eq!(plain.digest, documented.digest);
        assert_ne!(plain.items[0].doc, documented.items[0].doc);
    }

    #[test]
    fn the_json_is_parseable_and_carries_the_derived_facts() {
        let json = docs_of(SRC).to_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let add = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "add")
            .unwrap();
        assert_eq!(add["signature"], "add(a: Int, b: Int) -> Int");
        assert_eq!(add["tier"], "any");
        assert_eq!(add["doc"], "Adds two numbers.");
    }

    #[test]
    fn a_doc_comment_cannot_inject_html() {
        let docs =
            docs_of("## <script>alert(1)</script> & \"quoted\".\ndef f() -> Int:\n    return 1\n");
        let html = docs.to_html();
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn the_markdown_names_every_published_item() {
        let md = docs_of(SRC).to_markdown();
        assert!(md.contains("### `add`"), "{md}");
        assert!(md.contains("add(a: Int, b: Int) -> Int"), "{md}");
        assert!(md.contains("### `Todo`"), "{md}");
        assert!(md.contains("Stable for the life of the item."), "{md}");
    }
}
