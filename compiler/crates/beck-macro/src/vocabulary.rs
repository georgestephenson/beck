//! What HTML is: the element, attribute and event names `ui:` will write.
//!
//! # Why this is a table and not a check
//!
//! `ui:` turned any `name=value` into an attribute and any `on_x=` into `data-b-x`, knowing
//! nothing about either. A misspelling was not a compile error, not a lint and not visible in a
//! snapshot review — `span(on_mouseenter=…)` shipped a dead attribute to a browser that listens
//! for five events and passed every gate, and `cls="done"` — the spelling
//! [`docs/01`](../../../../../docs/01-vision-and-premise.md) §1.3's own sketch uses — silently lost
//! a page its styling. [`docs/104`](../../../../../docs/104-styling-and-the-component-library.md)
//! §104.8's Wall 2 is the measurement.
//!
//! It lives here, as data, rather than in the `ui` module's expander, for a scheduling reason:
//! `ui:` is a compiler-provided special case standing in for a user-written macro, and typed
//! macros retire it ([`docs/10`](../../../../../docs/10-decisions.md) D22). A vocabulary buried in
//! today's expander would be written a second time when that happens, and the second copy is the
//! one that would drift. [`docs/12`](../../../../../docs/12-standards-and-conformance.md) §12.4's
//! three accessibility checks — alt text, accessible name, input label — are scheduled over this
//! same tree and read [`ELEMENTS`] rather than a list of their own.
//!
//! # What is checked, and what is not
//!
//! **Events are closed.** [`EVENTS`] is exactly what `beck-rt/client/beck-patch.js` interprets, and
//! `ui.rs`'s `the_event_vocabulary_is_what_the_client_listens_for` reads the client's source to say
//! so. An event the client does not handle is a `data-b-*` attribute wired to nothing, so there is
//! no such thing as a custom one.
//!
//! **Attributes are closed with two open prefixes.** `data-` and `aria-` are HTML's own escape
//! hatches and are admitted by rule rather than by list — which is also the answer to "what if I
//! need an attribute that is genuinely mine": HTML already decided, and it is `data-`.
//!
//! **Elements are not refused**, and the reason is a limit of today's surface rather than a
//! judgement: inside a `ui:` block, a lowercase call whose arguments are all keyword arguments is
//! indistinguishable from an element, so refusing an unknown one would refuse a user's own helper
//! function called by name. [`ELEMENTS`] is therefore a table something else reads — §12.4's
//! checks, and whatever `ui:` becomes when it is a user-written typed macro, which is where a
//! `Html`-returning helper stops looking like a `<div>`.

/// The events the client interprets, and what each one is.
///
/// Five, and the list is not a design — it is a reading of `beck-patch.js`. `enter` is the odd one:
/// the DOM event is `keydown` filtered to the Enter key, because "submit on Enter" is what an
/// `input` wants and a raw keydown is not something a declarative attribute can usefully carry.
///
/// The W3C's ARIA Authoring Practices keyboard tables want arrows, `Home`, `End`, `Escape`,
/// `Space` and typeahead, none of which are here. That gap is the client's to close
/// ([`docs/104`](../../../../../docs/104-styling-and-the-component-library.md) §104.8), and this
/// table is what makes it a *refusal* rather than an attribute that does nothing.
pub const EVENTS: &[(&str, &str)] = &[
    ("click", "a click on this element"),
    ("enter", "the Enter key, in a text input"),
    ("submit", "a form submission"),
    ("input", "each edit to a control's value"),
    ("change", "a committed change to a control's value"),
];

/// Whether `name` is an event `on_` may carry.
pub fn is_event(name: &str) -> bool {
    EVENTS.iter().any(|(e, _)| *e == name)
}

/// Whether `name` is an attribute an element may be given.
///
/// `data-` and `aria-` are admitted by prefix: the first is HTML's own extension point and the
/// second is a namespace with hundreds of members whose spelling is checkable but whose *values*
/// are where the mistakes are, which is §12.4's job rather than this one.
pub fn is_attribute(name: &str) -> bool {
    name.starts_with("data-") || name.starts_with("aria-") || ATTRIBUTES.contains(&name)
}

/// What an element needs before somebody who cannot see it can use it.
///
/// [`docs/12`](../../../../../docs/12-standards-and-conformance.md) §12.4's first three checks, as
/// a table rather than as three `if`s in the expander — for [`ELEMENTS`]'s own reason. A check that
/// matched the literal `"imag"` would never fire and no test over correct programs could notice;
/// as a row it is held to [`ELEMENTS`] by
/// `every_element_a_check_is_about_is_an_element_this_vocabulary_knows`, which is a gate that goes
/// red on the typo.
pub const NAMING: &[(&str, Naming)] = &[
    ("img", Naming::Alt),
    ("button", Naming::TextOrLabel),
    ("input", Naming::Label),
    ("select", Naming::Label),
    ("textarea", Naming::Label),
];

/// How one element is given a name a screen reader can announce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    /// `alt`, which HTML requires of every image and whose empty value means "decorative".
    Alt,
    /// Its own text, or a label attribute when it has none — the icon-button case.
    TextOrLabel,
    /// A label attribute, or an `id` a `label(for=…)` elsewhere points at.
    Label,
}

/// What this element needs, if it is one of the ones that need anything.
pub fn naming(tag: &str) -> Option<Naming> {
    NAMING.iter().find(|(e, _)| *e == tag).map(|(_, n)| *n)
}

/// The attributes that give an element an accessible name, in the order a fix-it should offer them.
///
/// `title` is last and is included rather than recommended: it is a real naming mechanism and it is
/// also a tooltip, so a program that has one is not refused, and nothing here suggests reaching for
/// it first.
pub const LABELLING: &[&str] = &["aria-label", "aria-labelledby", "title"];

/// Whether `name` is an element this vocabulary knows.
///
/// Nothing in the `ui` module refuses on this today — the module documentation says why — but
/// §12.4's accessibility checks are written against it.
pub fn is_element(name: &str) -> bool {
    ELEMENTS.contains(&name)
}

/// Spellings that are not typos, and what each one means here.
///
/// A distance search cannot find these. `cls` is **one** edit from `cols` and two from `class`, so
/// the nearest name is the wrong one — and `cls=` is what
/// [`docs/01`](../../../../../docs/01-vision-and-premise.md) §1.3's sketch writes, faithfully,
/// because the original conversation did. It is therefore the first thing a reader arriving from
/// that page will type, and the first suggestion they should get.
pub const ALIASES: &[(&str, &str)] = &[
    ("cls", "class"),
    ("classname", "class"),
    ("class-name", "class"),
    ("htmlfor", "for"),
];

/// Spellings of an event that are not typos either.
///
/// `keydown` is the case worth having: the client *does* listen for it — filtered to the Enter key,
/// and registered under the name `enter`, because "submit on Enter" is what a text input wants and
/// a raw keydown is not something a declarative attribute can usefully carry. So somebody writing
/// `on_keydown` has guessed the DOM's name for the thing that exists, which no distance search will
/// find: `keydown` is seven edits from `enter`.
pub const EVENT_ALIASES: &[(&str, &str)] = &[("keydown", "enter"), ("keypress", "enter")];

/// What an event name that does not exist most likely meant, or nothing.
///
/// Nothing is the common answer and it is the right one: `mouseenter`, `focus` and `blur` are
/// events the client does not have, not misspellings of events it does, and a suggestion there
/// would send a reader to rewrite a handler that was never going to work.
pub fn event_suggestion(written: &str) -> Option<&'static str> {
    if let Some((_, meant)) = EVENT_ALIASES.iter().find(|(from, _)| *from == written) {
        return Some(meant);
    }
    nearest(written, EVENTS.iter().map(|(e, _)| *e))
}

/// What an attribute name that does not exist most likely meant.
///
/// Three rules in order, and the first is the one that matters most in a language whose keyword
/// arguments are snake_case. `ui:` turns `_` into `-`, so a program that writes `max_length=`
/// reaches HTML as `max-length` — and the attribute is `maxlength`, with no separator at all.
/// **Squashing the hyphens out and looking again** catches every attribute of that shape at once —
/// `maxlength`, `tabindex`, `colspan`, `autofocus`, `novalidate`, `formaction`, `playsinline` — and
/// it is a rule rather than the forty-line table the same coverage would take by hand.
pub fn suggestion(written: &str) -> Option<&'static str> {
    let squashed: String = written.chars().filter(|c| *c != '-').collect();
    if let Some(found) = ATTRIBUTES.iter().find(|a| **a == squashed) {
        return Some(found);
    }
    if let Some((_, meant)) = ALIASES.iter().find(|(from, _)| *from == written) {
        return Some(meant);
    }
    nearest(written, ATTRIBUTES.iter().copied())
}

/// The known name closest to `typo`, when one is close enough to be worth suggesting.
///
/// Ordinary Levenshtein distance with a threshold that scales with the word. It is the last of
/// [`suggestion`]'s three rules rather than the only one, because the mistakes this vocabulary
/// exists to catch are systematic rather than random.
pub fn nearest<'a>(typo: &str, among: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in among {
        let d = distance(typo, candidate);
        let allowed = if candidate.len().max(typo.len()) <= 4 {
            2
        } else {
            3
        };
        if d <= allowed && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, candidate));
        }
    }
    best.map(|(_, name)| name)
}

/// Levenshtein distance, two rows at a time.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            row[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

/// Every element name this vocabulary knows: HTML's, and the SVG subset a chart is drawn with.
///
/// SVG names that carry a capital — `linearGradient`, `foreignObject`, `clipPath` — are absent
/// because they cannot be *written* today: the `ui` module takes an element head to be all lowercase,
/// so a camel-cased tag is read as a function call. Listing what cannot be expressed would make
/// this table a wish.
pub const ELEMENTS: &[&str] = &[
    // Document and sections
    "html",
    "head",
    "body",
    "title",
    "base",
    "link",
    "meta",
    "style",
    "script",
    "noscript",
    "template",
    "slot",
    "main",
    "header",
    "footer",
    "nav",
    "article",
    "aside",
    "section",
    "search",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hgroup",
    "address",
    // Grouping
    "p",
    "hr",
    "pre",
    "blockquote",
    "ol",
    "ul",
    "menu",
    "li",
    "dl",
    "dt",
    "dd",
    "figure",
    "figcaption",
    "div",
    // Text
    "a",
    "em",
    "strong",
    "small",
    "s",
    "cite",
    "q",
    "dfn",
    "abbr",
    "ruby",
    "rt",
    "rp",
    "data",
    "time",
    "code",
    "var",
    "samp",
    "kbd",
    "sub",
    "sup",
    "i",
    "b",
    "u",
    "mark",
    "bdi",
    "bdo",
    "span",
    "br",
    "wbr",
    "ins",
    "del",
    // Embedded
    "picture",
    "source",
    "img",
    "iframe",
    "embed",
    "object",
    "video",
    "audio",
    "track",
    "map",
    "area",
    "canvas",
    // Tables
    "table",
    "caption",
    "colgroup",
    "col",
    "tbody",
    "thead",
    "tfoot",
    "tr",
    "td",
    "th",
    // Forms
    "form",
    "label",
    "input",
    "button",
    "select",
    "datalist",
    "optgroup",
    "option",
    "textarea",
    "output",
    "progress",
    "meter",
    "fieldset",
    "legend",
    // Interactive
    "details",
    "summary",
    "dialog",
    // SVG, lowercase only — see this table's own note
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "image",
    "marker",
    "mask",
    "pattern",
    "stop",
    "desc",
    "filter",
];

/// Every attribute name this vocabulary knows, beyond the `data-` and `aria-` prefixes.
///
/// Flat rather than per element. A per-element table is what an HTML validator has and it is a
/// larger claim than this needs to make: what [`docs/104`](../../../../../docs/104-styling-and-the-component-library.md)
/// §104.8 measured is *misspellings* — `cls` for `class` — and a name nothing in HTML has is a
/// misspelling whichever element it lands on. Refusing `colspan` on a `<div>` is a different
/// feature, and one whose false refusals would cost more than it catches.
pub const ATTRIBUTES: &[&str] = &[
    // Global
    "accesskey",
    "autocapitalize",
    "autocorrect",
    "autofocus",
    "class",
    "contenteditable",
    "dir",
    "draggable",
    "enterkeyhint",
    "hidden",
    "id",
    "inert",
    "inputmode",
    "is",
    "itemid",
    "itemprop",
    "itemref",
    "itemscope",
    "itemtype",
    "lang",
    "nonce",
    "popover",
    "role",
    "slot",
    "spellcheck",
    "style",
    "tabindex",
    "title",
    "translate",
    "writingsuggestions",
    // Links and media
    "href",
    "hreflang",
    "target",
    "download",
    "ping",
    "rel",
    "referrerpolicy",
    "src",
    "srcset",
    "sizes",
    "alt",
    "loading",
    "decoding",
    "fetchpriority",
    "crossorigin",
    "usemap",
    "ismap",
    "width",
    "height",
    "poster",
    "preload",
    "autoplay",
    "loop",
    "muted",
    "controls",
    "playsinline",
    "kind",
    "srclang",
    "default",
    "media",
    "as",
    "integrity",
    "defer",
    "async",
    "nomodule",
    "charset",
    "content",
    "http-equiv",
    "allow",
    "allowfullscreen",
    "sandbox",
    "srcdoc",
    "coords",
    "shape",
    "type",
    "cite",
    "datetime",
    "open",
    "start",
    "reversed",
    "value",
    "label",
    "span",
    // Forms
    "accept",
    "accept-charset",
    "action",
    "autocomplete",
    "capture",
    "checked",
    "cols",
    "dirname",
    "disabled",
    "enctype",
    "for",
    "form",
    "formaction",
    "formenctype",
    "formmethod",
    "formnovalidate",
    "formtarget",
    "list",
    "max",
    "maxlength",
    "method",
    "min",
    "minlength",
    "multiple",
    "name",
    "novalidate",
    "pattern",
    "placeholder",
    "readonly",
    "required",
    "rows",
    "selected",
    "size",
    "step",
    "wrap",
    "high",
    "low",
    "optimum",
    // Tables
    "colspan",
    "rowspan",
    "headers",
    "scope",
    "abbr",
    // Invokers, which are what §104.9 says replace a handler outright
    "command",
    "commandfor",
    "popovertarget",
    "popovertargetaction",
    // SVG: geometry, painting and text
    "viewBox",
    "preserveAspectRatio",
    "xmlns",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "d",
    "points",
    "pathLength",
    "dx",
    "dy",
    "rotate",
    "transform",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "opacity",
    "clip-path",
    "clip-rule",
    "marker-start",
    "marker-mid",
    "marker-end",
    "text-anchor",
    "dominant-baseline",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "letter-spacing",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "patternUnits",
    "maskUnits",
    "markerWidth",
    "markerHeight",
    "refX",
    "refY",
    "orient",
    "vector-effect",
    "shape-rendering",
    "paint-order",
];

#[cfg(test)]
mod tests {

    /// Every element a §12.4 check is about is an element this vocabulary knows.
    ///
    /// The gate that makes [`NAMING`] a table worth having rather than three tag names moved. A
    /// check written against `"imag"` would never fire, and nothing that compiles correct programs
    /// could notice — which is `docs/82` §82.10's pattern exactly: the failure to guard against is
    /// a check that *cannot* fail.
    #[test]
    fn every_element_a_check_is_about_is_an_element_this_vocabulary_knows() {
        for (element, _) in super::NAMING {
            assert!(
                super::is_element(element),
                "`{element}` is checked for an accessible name and is not an element"
            );
        }
    }

    /// And every attribute those checks accept as a name is one an element may carry.
    #[test]
    fn every_labelling_attribute_is_an_attribute() {
        for name in super::LABELLING {
            assert!(super::is_attribute(name), "`{name}` is not an attribute");
        }
    }
    use super::*;

    #[test]
    fn the_tables_are_sorted_within_their_groups_and_hold_no_duplicate() {
        for (what, table) in [("elements", ELEMENTS), ("attributes", ATTRIBUTES)] {
            let mut seen: Vec<&str> = table.to_vec();
            seen.sort_unstable();
            let count = seen.len();
            seen.dedup();
            assert_eq!(seen.len(), count, "{what} holds a name twice");
        }
        let mut events: Vec<&str> = EVENTS.iter().map(|(e, _)| *e).collect();
        events.sort_unstable();
        events.dedup();
        assert_eq!(events.len(), EVENTS.len());
    }

    #[test]
    fn the_prefixes_are_open_and_everything_else_is_closed() {
        assert!(is_attribute("data-anything-at-all"));
        assert!(is_attribute("aria-label"));
        assert!(is_attribute("class"));
        assert!(!is_attribute("cls"));
        assert!(
            !is_attribute("data"),
            "`data` alone is the element, not a prefix"
        );
        // A `data` *attribute* exists on `<object>`, and is in the table for that reason.
        assert!(ATTRIBUTES.contains(&"content"));
    }

    #[test]
    fn what_a_name_that_does_not_exist_most_likely_meant() {
        // The rule, which is the one that earns its place: a snake_case guess at an attribute
        // HTML spells with no separator. `ui:` turns `_` into `-` before this ever sees it.
        for (written, meant) in [
            ("max-length", "maxlength"),
            ("tab-index", "tabindex"),
            ("col-span", "colspan"),
            ("auto-focus", "autofocus"),
            ("no-validate", "novalidate"),
            ("plays-inline", "playsinline"),
        ] {
            assert_eq!(suggestion(written), Some(meant), "{written}");
        }
        // The alias, which a distance search gets wrong: `cls` is one edit from `cols`.
        assert_eq!(nearest("cls", ATTRIBUTES.iter().copied()), Some("cols"));
        assert_eq!(
            suggestion("cls"),
            Some("class"),
            "the spelling docs/01 §1.3's sketch uses is the case this exists for"
        );
        // And an ordinary typo, which the distance search is still there for.
        assert_eq!(suggestion("hight"), Some("height"));
        for absent in ["mouseenter", "focus", "blur"] {
            assert_eq!(
                event_suggestion(absent),
                None,
                "an event the client does not have is not a typo for one it does"
            );
        }
        assert_eq!(
            event_suggestion("keydown"),
            Some("enter"),
            "the client does listen for keydown; it is registered under the key it filters to"
        );
    }
}
