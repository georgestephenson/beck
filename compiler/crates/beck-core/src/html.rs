//! `Html` as a value, not a string.
//!
//! Ported from Phase 0's `beck-p0-core::html`, which hand-wrote the output the compiler would
//! generate. In Phase 1 it is the *runtime representation* the compiled `view` builds: the `ui:`
//! macro lowers to `html_el`/`html_text`/`html_attr`/`html_on`/`html_key`, and those primitives
//! build these values. The Phase 0 tests came with it unchanged, which is the point — the value
//! semantics did not move, only who writes the code that produces them.
//!
//! §4.2: "UI trees stay symbolic … a component tree that has already become DOM mutation calls
//! cannot be server-side rendered or pre-rendered at build time." The same value is therefore
//! rendered three ways: to an SSR string (free first paint), to the wire encoding carried by
//! patches, and — the point of the exercise — structurally diffed against its predecessor
//! (`crate::diff`).
//!
//! Every node carries a structural hash computed at construction. §5.1: "because views are
//! signal-derived, the differ knows which subtrees *can't* have changed and skips them". Phase 0
//! has no signal graph, so it approximates that with an O(1) hash comparison per subtree — the
//! same asymptotic effect, without the machinery.

use serde_json::{json, Value};

/// Attribute name under which a keyed node's key is materialised on the wire and in SSR output.
pub const KEY_ATTR: &str = "data-b-k";

/// Elements that carry no children and are written without a closing tag.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Html {
    Text {
        text: String,
        hash: u64,
    },
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        key: Option<String>,
        children: Vec<Html>,
        /// Three accumulators rather than one hash, so that the hash is a function of the node's
        /// *structure* and not of the order the builder methods happened to be called in. Getting
        /// this wrong makes two identical subtrees hash differently, and a differ that trusts a
        /// hash it cannot reproduce is worse than one with no hash at all.
        tag_key_h: u64,
        attrs_h: u64,
        children_h: u64,
    },
}

impl Html {
    pub fn text(s: impl Into<String>) -> Html {
        let text = s.into();
        let hash = fnv_str(FNV_OFFSET ^ 0x01, &text);
        Html::Text { text, hash }
    }

    /// Start an element. Attributes and children are added with the builder methods below; each
    /// step costs one multiply per byte added, and nothing is ever rehashed.
    pub fn el(tag: impl Into<String>) -> Html {
        let tag = tag.into();
        let tag_key_h = fnv_str(FNV_OFFSET ^ 0x02, &tag);
        Html::Element {
            tag,
            attrs: Vec::new(),
            key: None,
            children: Vec::new(),
            tag_key_h,
            attrs_h: FNV_OFFSET,
            children_h: FNV_OFFSET,
        }
    }

    pub fn attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Html {
        if let Html::Element { attrs, attrs_h, .. } = &mut self {
            let (name, value) = (name.into(), value.into());
            *attrs_h = fnv_str(fnv_str(*attrs_h, &name), &value);
            attrs.push((name, value));
        }
        self
    }

    /// Conditional attribute — the `(if t.done "done" "")` shape from the sketch, without emitting
    /// an empty attribute the differ would then have to churn on.
    pub fn attr_if(self, cond: bool, name: impl Into<String>, value: impl Into<String>) -> Html {
        if cond {
            self.attr(name, value)
        } else {
            self
        }
    }

    /// A handler in `view` compiles to a declarative attribute — no user JavaScript exists in Mode
    /// A (§5.1 "Input capture"), so `script-src` can stay near-empty. `command` is the serialised
    /// command constructor the thin client posts back up the socket.
    pub fn on(self, event: &str, command: Value) -> Html {
        self.attr(format!("data-b-{event}"), command.to_string())
    }

    pub fn key(mut self, k: impl Into<String>) -> Html {
        if let Html::Element {
            tag,
            key,
            tag_key_h,
            ..
        } = &mut self
        {
            let k = k.into();
            // Recomputed from the tag rather than folded into whatever is there, so that setting a
            // key is idempotent and order-independent.
            *tag_key_h = fnv_str(fnv_str(FNV_OFFSET ^ 0x02, tag) ^ 0x9e37_79b9_7f4a_7c15, &k);
            *key = Some(k);
        }
        self
    }

    pub fn child(mut self, c: Html) -> Html {
        if let Html::Element {
            children,
            children_h,
            ..
        } = &mut self
        {
            *children_h = fnv_u64(*children_h, c.hash());
            children.push(c);
        }
        self
    }

    pub fn children(mut self, cs: impl IntoIterator<Item = Html>) -> Html {
        for c in cs {
            self = self.child(c);
        }
        self
    }

    /// Rebuild the tree, recomputing every structural hash bottom-up.
    ///
    /// Needed after in-place surgery on a node: an edit deep in a tree invalidates the hash of
    /// every ancestor, and a stale hash is worse than no hash — the differ would skip a subtree
    /// that did change.
    pub fn rehash(&self) -> Html {
        match self {
            Html::Text { text, .. } => Html::text(text.clone()),
            Html::Element {
                tag,
                attrs,
                key,
                children,
                ..
            } => {
                let mut el = Html::el(tag.clone());
                for (k, v) in attrs {
                    el = el.attr(k.clone(), v.clone());
                }
                if let Some(k) = key {
                    el = el.key(k.clone());
                }
                el.children(children.iter().map(Html::rehash))
            }
        }
    }

    pub fn hash(&self) -> u64 {
        match self {
            Html::Text { hash, .. } => *hash,
            Html::Element {
                tag_key_h,
                attrs_h,
                children_h,
                ..
            } => fnv_u64(fnv_u64(*tag_key_h, *attrs_h), *children_h),
        }
    }

    pub fn key_of(&self) -> Option<&str> {
        match self {
            Html::Element { key, .. } => key.as_deref(),
            Html::Text { .. } => None,
        }
    }

    pub fn child_at(&self, i: usize) -> Option<&Html> {
        match self {
            Html::Element { children, .. } => children.get(i),
            Html::Text { .. } => None,
        }
    }

    /// Node count — the denominator of "how much of the tree did the diff actually touch".
    pub fn node_count(&self) -> usize {
        match self {
            Html::Text { .. } => 1,
            Html::Element { children, .. } => {
                1 + children.iter().map(Html::node_count).sum::<usize>()
            }
        }
    }

    /// The wire encoding: a text node is a JSON string, an element is `[tag, attrs, children]`.
    ///
    /// Positional and terse because it rides in every patch. §4.4 specifies a field-tagged binary
    /// encoding for Beck↔Beck traffic; the thin client is a browser, and JSON costs it zero bytes
    /// of decoder — see `crate::patch::Codec` for the measured comparison.
    pub fn to_wire(&self) -> Value {
        match self {
            Html::Text { text, .. } => Value::String(text.clone()),
            Html::Element {
                tag,
                attrs,
                key,
                children,
                ..
            } => {
                // Pairs rather than an object, because a JSON object is not ordered and this one
                // has to be: the client sets attributes in the order it reads them, so an object
                // — which `serde_json` sorts — makes a rebuilt element carry its attributes in a
                // different order than the same element the server rendered into the document. It
                // is invisible until something compares the two, and then it is the difference
                // between "the DOM is the page" and "the DOM is nearly the page"
                // (`docs/94` §94.7).
                let mut pairs: Vec<Value> = attrs.iter().map(|(k, v)| json!([k, v])).collect();
                if let Some(k) = key {
                    pairs.push(json!([KEY_ATTR, k]));
                }
                json!([
                    tag,
                    Value::Array(pairs),
                    children.iter().map(Html::to_wire).collect::<Vec<_>>()
                ])
            }
        }
    }

    /// Server-side render. "First paint is free SSR: evaluate pure `view` against the current
    /// accumulator, ship HTML."
    ///
    /// Emitted without any inter-element whitespace, deliberately: patch paths are child indices,
    /// and a pretty-printer would insert text nodes that the server's tree does not have, so the
    /// first patch after hydration would address the wrong node.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1024);
        self.render_into(&mut out);
        out
    }

    pub fn render_into(&self, out: &mut String) {
        match self {
            Html::Text { text, .. } => escape_text_into(text, out),
            Html::Element {
                tag,
                attrs,
                key,
                children,
                ..
            } => {
                out.push('<');
                out.push_str(tag);
                for (k, v) in attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    escape_attr_into(v, out);
                    out.push('"');
                }
                if let Some(k) = key {
                    out.push(' ');
                    out.push_str(KEY_ATTR);
                    out.push_str("=\"");
                    escape_attr_into(k, out);
                    out.push('"');
                }
                out.push('>');
                if VOID_TAGS.contains(&tag.as_str()) {
                    return;
                }
                for c in children {
                    c.render_into(out);
                }
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
}

/// The node `html_el(tag, attrs, children)` builds, out of the three values it is given.
///
/// **One function, two callers.** The evaluator reaches it from `Prim::HtmlEl`; the native heap's
/// decoder reaches it because a compiled `view` answers with the *arguments* rather than with a
/// tree — a `Str`, a list of attributes and a list of children — and the tree is built here on the
/// way out. What lives in this function is not the assembly but the three rules that go with it:
/// an attribute with an empty value is **dropped** rather than emitted, a handler becomes
/// `data-b-<event>` carrying the command as JSON, and a key sets the node's key rather than an
/// attribute. Each is a decision the differ downstream depends on, and a second spelling of any of
/// them would be a compiled page that differs from an interpreted one in a way no type can catch.
pub fn element(
    tag: &crate::core::Value,
    attrs: &[crate::core::Value],
    children: &[crate::core::Value],
) -> Result<Html, String> {
    use crate::core::{AttrValue, Value as Val};

    let mut el = Html::el(tag.display());
    for a in attrs {
        match a {
            Val::Attr(at) => match &**at {
                // An empty attribute value is dropped rather than emitted, so the differ has
                // nothing to churn on — Phase 0's `attr_if`.
                AttrValue::Plain(k, v) => {
                    if !v.is_empty() {
                        el = el.attr(k.to_string(), v.to_string());
                    }
                }
                AttrValue::On(ev, cmd) => el = el.on(ev, cmd.to_json()),
                AttrValue::Key(k) => el = el.key(k.to_string()),
            },
            other => return Err(format!("not an attribute: {}", other.display())),
        }
    }
    for ch in children {
        match ch.as_html() {
            Some(h) => el = el.child(h.clone()),
            None => return Err(format!("not an Html child: {}", ch.display())),
        }
    }
    Ok(el)
}

/// The node `html_text(v)` builds: a tree that is *already* a tree is spliced, and anything else is
/// its rendering as text.
///
/// The same two callers as [`element`], and the same reason: `ui:` lowers every non-element child
/// through `html_text`, so "or Html" deciding differently in a compiled view than in an interpreted
/// one would make a view composed out of functions render its parts as escaped markup in one
/// backend and as markup in the other (`docs/94` §94.4).
pub fn text_of(v: &crate::core::Value) -> Html {
    match v {
        crate::core::Value::Html(h) => (**h).clone(),
        other => Html::text(other.display()),
    }
}

fn escape_text_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

/// Escape a string for an attribute value the *shell* writes rather than a view.
///
/// The runtime builds the surrounding document with `format!` and puts two values from outside the
/// program into it — the actor's name and its claims — and those are the identity provider's
/// strings, not the program's. Exported so that they go through the same escaping the view's own
/// attributes do rather than a second one written beside it.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_attr_into(s, &mut out);
    out
}

fn escape_attr_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv_str(mut h: u64, s: &str) -> u64 {
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h ^= 0xff;
    h.wrapping_mul(FNV_PRIME)
}

fn fnv_u64(mut h: u64, v: u64) -> u64 {
    for b in v.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_hash_distinguishes_structure_not_just_content() {
        let a = Html::el("li").child(Html::text("x"));
        let b = Html::el("li").child(Html::text("y"));
        let c = Html::el("li").child(Html::text("x"));
        let nested = Html::el("li").child(Html::el("span").child(Html::text("x")));
        assert_ne!(a.hash(), b.hash());
        assert_eq!(a.hash(), c.hash());
        assert_ne!(a.hash(), nested.hash());
        assert_ne!(
            Html::el("li").key("1").hash(),
            Html::el("li").key("2").hash()
        );
        assert_ne!(
            Html::el("li").attr("class", "done").hash(),
            Html::el("li").attr("class", "").hash()
        );
    }

    #[test]
    fn structural_hash_is_independent_of_builder_call_order() {
        let a = Html::el("li")
            .key("k")
            .attr("class", "done")
            .child(Html::text("x"));
        let b = Html::el("li")
            .attr("class", "done")
            .child(Html::text("x"))
            .key("k");
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.rehash().hash(), a.hash());
    }

    #[test]
    fn ssr_escapes_and_emits_no_stray_whitespace() {
        let tree = Html::el("main")
            .child(Html::el("h1").child(Html::text("a < b & c")))
            .child(Html::el("input").attr("value", "\"quoted\""));
        assert_eq!(
            tree.render(),
            "<main><h1>a &lt; b &amp; c</h1><input value=\"&quot;quoted&quot;\"></main>"
        );
    }

    #[test]
    fn wire_encoding_is_positional() {
        let tree = Html::el("li")
            .key("k1")
            .attr("class", "done")
            .child(Html::text("x"));
        assert_eq!(
            tree.to_wire(),
            json!(["li", [["class", "done"], ["data-b-k", "k1"]], ["x"]])
        );
    }

    /// Attributes cross in the order the program wrote them, not in the order a map would sort
    /// them into.
    ///
    /// A JSON object is unordered, and `serde_json`'s is a `BTreeMap`, so this used to emit
    /// `autofocus` before `placeholder` whatever the source said — which meant an element the
    /// client rebuilt from a patch carried its attributes in a different order than the same
    /// element the server rendered into the document. Nothing was wrong with the page; it simply
    /// was not the same page, and only a browser comparing the two could see it
    /// (`docs/94` §94.12).
    #[test]
    fn attributes_cross_in_the_order_they_were_written() {
        let tree = Html::el("input")
            .attr("placeholder", "what needs doing?")
            .attr("autofocus", "on");
        assert_eq!(
            tree.to_wire(),
            json!([
                "input",
                [["placeholder", "what needs doing?"], ["autofocus", "on"]],
                []
            ])
        );
    }
}
