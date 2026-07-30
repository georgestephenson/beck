//! `(def styles : Css ...)` — a value, flattened to a static stylesheet at build time.
//!
//! There is no runtime CSS story in Beck and there should not be one: the sketch's `styles` is
//! data, so the compiler emits a plain stylesheet and the browser gets a cacheable static asset.

/// A rule is a selector and its declarations, in source order.
pub struct Rule(
    pub &'static str,
    pub &'static [(&'static str, &'static str)],
);

/// The sketch's stylesheet, verbatim.
pub const STYLES: &[Rule] = &[
    Rule(
        "main",
        &[
            ("max-width", "40ch"),
            ("margin", "0 auto"),
            ("font", "16px system-ui"),
        ],
    ),
    Rule(
        ".done",
        &[("text-decoration", "line-through"), ("opacity", "0.5")],
    ),
    // Not in the sketch: the todo list is a `ul` of `li`s with a button per row, and the sketch
    // never says how they sit. Kept minimal and honest — this is presentation, not semantics.
    Rule(
        "ul",
        &[
            ("list-style", "none"),
            ("padding", "0"),
            ("margin", "1rem 0"),
        ],
    ),
    Rule(
        "li",
        &[
            ("display", "flex"),
            ("gap", ".5rem"),
            ("align-items", "baseline"),
        ],
    ),
    Rule("li span", &[("flex", "1"), ("cursor", "pointer")]),
    Rule(
        "input",
        &[("width", "100%"), ("padding", ".5rem"), ("font", "inherit")],
    ),
    Rule(
        "button",
        &[
            ("border", "0"),
            ("background", "none"),
            ("cursor", "pointer"),
            ("font", "inherit"),
        ],
    ),
    Rule("footer", &[("color", "#666")]),
];

/// Flatten to the static stylesheet the image ships.
pub fn stylesheet() -> String {
    let mut out = String::with_capacity(512);
    for Rule(selector, decls) in STYLES {
        out.push_str(selector);
        out.push('{');
        for (property, value) in *decls {
            out.push_str(property);
            out.push(':');
            out.push_str(value);
            out.push(';');
        }
        out.push('}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_to_a_stylesheet() {
        let css = stylesheet();
        assert!(css.starts_with("main{max-width:40ch;margin:0 auto;font:16px system-ui;}"));
        assert!(css.contains(".done{text-decoration:line-through;opacity:0.5;}"));
    }
}
