//! What a page's classes are, and whether the compiler can know them.
//!
//! [`docs/104-styling-and-the-component-library.md`](../../../../docs/104-styling-and-the-component-library.md)
//! §104.4 is the design and [`docs/08`](../../../../docs/08-roadmap.md) §8.5.4's styling cluster is
//! the order. This suite holds the third item, which is the one everything else in that half is
//! behind: **`class=` takes a list**, and the compiler enumerates what can reach one.
//!
//! The two are one item because they are one idea. A list of alternatives can be enumerated; a
//! string built with `+` cannot, by Beck or by Tailwind's own scanner (§104.3 measured that scanner
//! over this tree and found it reading English prose out of comments and missing a real utility
//! behind a module boundary). So the surface exists to make the analysis possible, and the analysis
//! exists to say when the surface was not used.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_core::style::{classes, Because};

fn compile(name: &str, src: &str) -> beck_core::Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

fn example(file: &str) -> beck_core::Placed {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(file);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
    compile(file, &src)
}

/// A program whose page is one element carrying `attrs`.
fn page(attrs: &str) -> String {
    format!(
        r#"
model State:
    items: Map[Str, Str]

union Command:
    Add(k: Str)

union Event:
    Added(k: Str)

union Rejection:
    Blank

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(k):
            return s.with(items=map_insert(s.items, k, k))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(k):
            if str_is_empty(str_trim(k)):
                return Err(error=Blank)
            return Ok(value=[Added(k=k)])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            div({attrs}): "hello"

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, st, validate)
st: Signal[State] = durable(fold(apply_event, State(items={{}}), events))
page: Signal[Html] = per_session(st, view)
"#
    )
}

/// The rendered page of a program built by [`page`].
fn rendered(attrs: &str) -> String {
    let placed = compile("style.beck", &page(attrs));
    let backend = beck_eval::backend(&placed);
    let runtime = beck_rt::Runtime::new(placed, backend).expect("the program prepares");
    let state = runtime.initial_state().expect("an initial accumulator");
    runtime.view(&state, "ana").expect("a page").render()
}

/// **A list where HTML wants a space-separated value is one.**
///
/// `class=["btn", "primary"]` is `class="btn primary"`, and a string is untouched — an existing
/// program renders the characters it always did, which is what makes this an addition rather than a
/// change. The conditional element is the case the whole item exists for: it is what a program
/// writes instead of `"btn " + variant`.
#[test]
fn a_list_class_is_space_separated_and_a_string_is_unchanged() {
    assert!(
        rendered(r#"class=["btn", "primary"]"#).contains(r#"class="btn primary""#),
        "a list class is joined with a space"
    );
    assert!(
        rendered(r#"class="btn primary""#).contains(r#"class="btn primary""#),
        "a string class is what it always was"
    );
    // The shape §104.4 asks for, in place of a concatenation.
    let conditional = rendered(r#"class=["btn", "primary" if 1 > 0 else "plain"]"#);
    assert!(
        conditional.contains(r#"class="btn primary""#),
        "a conditional element joins like any other: {conditional}"
    );
}

/// **And an attribute HTML does not define as a list is left alone.**
///
/// The table in the lowering is four names, not "every attribute", and the reason is that joining a
/// list into an attribute that takes one value would turn a mistake into a plausible string. This is
/// the half of that decision a test can see: `title` takes one value, so a list stays a list and
/// renders as one — visibly wrong, which is the point.
#[test]
fn an_attribute_that_is_not_a_token_list_does_not_join() {
    let out = rendered(r#"title=["a", "b"]"#);
    assert!(
        !out.contains(r#"title="a b""#),
        "`title` is not a space-separated attribute and must not be joined: {out}"
    );
}

/// **The classes a program can carry are worked out from the program**, through a call and through
/// every branch of an `if`.
///
/// Four of the tree's own pages, and each is the shape §104.4 forecast programs would already be
/// written in: `class=[…, done_class(t)]` whose body one call away is two constant alternatives.
/// The empty alternative contributes nothing, because an empty class is not a class and the browser
/// drops it too.
///
/// The sketch is the interesting row: every class it carries is now a **utility**, so the sheet
/// `beck build` writes for it is complete. `routed.beck` and `board.beck` still name their own, and
/// the analysis does not care which — a name is a name, and whether the compiler has a rule for it
/// is a different question, asked by `a_programs_own_class_names_are_not_mistaken_for_utilities`.
#[test]
fn the_classes_of_a_page_are_enumerated_through_calls_and_branches() {
    for (file, want) in [
        (
            "examples/todo.beck",
            vec![
                "border",
                "flex",
                "flex-1",
                "font-bold",
                "gap-2",
                "items-baseline",
                "line-through",
                "max-w-80",
                "mx-auto",
                "my-4",
                "p-2",
                "p-4",
                "rounded",
                "space-y-2",
                "text-2xl",
                "text-gray-500",
                "w-full",
            ],
        ),
        ("examples/routed.beck", vec!["done", "here"]),
        ("examples/board.beck", vec!["column", "columns"]),
        ("corpus/02-chat.beck", vec!["mine", "theirs"]),
    ] {
        let styles = classes(&example(file).program);
        let got: Vec<&str> = styles.classes.iter().map(|c| &**c).collect();
        assert_eq!(got, want, "{file}");
        assert!(
            styles.dynamic.is_empty(),
            "{file} builds a class at run time and did not before: {:?}",
            styles.dynamic
        );
    }
}

/// **A class the compiler cannot know is said out loud, and the rest of the program is still
/// enumerated.**
///
/// The second half is the one that would be easy to get wrong: a page with one computed class must
/// not lose the classes beside it, or the report would punish a program for one line by going
/// silent about the other ten. And the reason has to name the concatenation rather than the shape
/// underneath it, because that is the difference between a report a reader can act on — write a list
/// — and one they can only observe.
#[test]
fn a_class_built_at_run_time_is_named_and_does_not_hide_the_others() {
    let src = page(r#"class="fixed""#).replace(
        r#"            div(class="fixed"): "hello""#,
        "            div(class=\"fixed\"): \"hello\"\n            \
         span(class=(\"row-\" + session.actor)): \"there\"",
    );
    let styles = classes(&compile("dynamic.beck", &src).program);
    assert_eq!(
        styles.classes.iter().map(|c| &**c).collect::<Vec<_>>(),
        vec!["fixed"],
        "the class beside the computed one is still enumerated"
    );
    assert_eq!(styles.dynamic.len(), 1, "{:?}", styles.dynamic);
    assert_eq!(styles.dynamic[0].because, Because::Concatenated);
    assert_eq!(styles.dynamic[0].in_def, Arc::from("view"));
    assert!(
        styles.dynamic[0].because.because().contains("list"),
        "the reason names the rewrite: {}",
        styles.dynamic[0].because.because()
    );
}

/// **A class read from data is a different refusal from a class that was concatenated**, because a
/// reader does something different about each.
#[test]
fn a_class_read_from_a_value_says_so_rather_than_blaming_a_concatenation() {
    let src = page(r#"class="fixed""#).replace(
        r#"class="fixed""#,
        r#"class=unwrap_or(map_get(s.items, "k"), "")"#,
    );
    let styles = classes(&compile("from-data.beck", &src).program);
    assert_eq!(styles.dynamic.len(), 1, "{:?}", styles.dynamic);
    assert_eq!(styles.dynamic[0].because, Because::FromData);
}

/// The committed oracle, as the lines `compiler/style/generate.sh` wrote.
struct Oracle {
    /// Every candidate, and the rule Tailwind emits for it: at-rules, selector, declarations.
    verdicts: BTreeMap<String, Option<(String, String, String)>>,
    theme: BTreeMap<String, String>,
    properties: BTreeMap<String, String>,
    supports: String,
}

fn oracle() -> Oracle {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../style/expected/tailwind-4.3.3.txt");
    let text = std::fs::read_to_string(&path).expect("the committed oracle is where it belongs");
    let mut out = Oracle {
        verdicts: BTreeMap::new(),
        theme: BTreeMap::new(),
        properties: BTreeMap::new(),
        supports: String::new(),
    };
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split('\t').collect();
        match f[0] {
            "rule" => {
                out.verdicts.insert(
                    f[1].to_string(),
                    Some((f[2].to_string(), f[3].to_string(), f[4].to_string())),
                );
            }
            "none" => {
                out.verdicts.insert(f[1].to_string(), None);
            }
            "theme" => {
                out.theme.insert(f[1].to_string(), f[2].to_string());
            }
            "property" => {
                out.properties.insert(f[1].to_string(), f[2].to_string());
            }
            "supports" => out.supports = f[1].to_string(),
            other => panic!("the oracle says `{other}`, which this gate does not know"),
        }
    }
    out
}

/// Beck's rule for one name, in the oracle's own notation.
fn as_oracle(rule: &beck_core::style::Rule) -> (String, String, String) {
    (
        rule.at.join("|"),
        rule.selector.clone(),
        rule.decls
            .iter()
            .map(|(p, v)| format!("{p}: {v}"))
            .collect::<Vec<_>>()
            .join(";"),
    )
}

/// **Beck's utility table agrees with Tailwind's compiler**, which is the only oracle for it.
///
/// §104.4: "the oracle is Tailwind itself, not a table somebody typed in". That is `clbg/`'s
/// pattern — hold somebody else's published artefact so a wrong constant fails even against a
/// matching wrong expectation — and `compiler/style/generate.sh` is how the artefact is obtained.
/// It is committed rather than produced here, because a gate that installs from a package registry
/// fails when somebody else's server does.
///
/// # It compares the **rule**, not the name
///
/// The table stopped being a predicate when the sheet emitter landed: `beck_core::style::rule`
/// turns a name into declarations, and two tables that agreed about which names exist while
/// disagreeing about what they mean would style a page wrongly with every gate green. So this
/// compares the at-rules, the selector and the declarations, in order, byte for byte.
///
/// # Three buckets, and only two of them are failures
///
/// **Unsound** — Beck accepts a name Tailwind refuses, or renders it differently — is the one that
/// matters: it is a class the compiler puts in a stylesheet that the browser reads differently from
/// every other page on the web. **Wrongly refused** — Tailwind emits a rule and Beck refuses — is a
/// gap unless Beck claims the family. Both are asserted at zero.
///
/// The third is **not** a failure and is printed: a name Tailwind accepts and Beck does not know is
/// a gap in a table this documents as a subset. Counting it is what stops the subset from quietly
/// becoming the claim, and it is the number to watch shrink as the families grow.
#[test]
fn the_utility_table_agrees_with_tailwind() {
    use beck_core::style::rule;

    let oracle = oracle();
    let mut accepted = 0usize;
    let (mut unsound, mut differs, mut gaps, mut wrongly_refused) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (name, want) in &oracle.verdicts {
        match (want, rule(name)) {
            (Some(want), Some(got)) => match as_oracle(&got) == *want {
                true => accepted += 1,
                false => differs.push((name.clone(), want.clone(), as_oracle(&got))),
            },
            (Some(_), None) => gaps.push(name.clone()),
            (None, Some(got)) => unsound.push((name.clone(), as_oracle(&got))),
            (None, None) => wrongly_refused.push(name.clone()),
        }
    }
    let total = accepted + differs.len() + gaps.len() + unsound.len() + wrongly_refused.len();
    assert!(total > 3_000, "only {total} candidates were compared");
    println!(
        "\n  {accepted} of {} names Tailwind emits a rule for are known here, byte for byte; {} \
         are not.\n  {} names Tailwind refuses, and this refuses all of them.",
        accepted + gaps.len() + differs.len(),
        gaps.len(),
        wrongly_refused.len()
    );

    assert!(
        unsound.is_empty(),
        "these are not Tailwind utilities and this table renders them, so a stylesheet built from \
         it would define rules no other page on the web has: {unsound:?}"
    );
    assert!(
        differs.is_empty(),
        "these names are utilities in both and mean different things, which is a page styled \
         wrongly with every other gate green — (name, Tailwind, Beck): {differs:?}"
    );
    // A ratchet rather than a bound: the gaps are the families this table has not taken, and the
    // number is here so that adding one is visible and losing one is a failure.
    assert!(
        accepted >= 3_200,
        "the table renders {accepted} of Tailwind's names and rendered 3,200, so a family was lost"
    );
    assert!(
        !gaps.is_empty(),
        "every candidate Tailwind accepts is one this table knows, so the list is a restatement \
         of the table rather than an oracle over it — widen `compiler/style/candidates.txt`"
    );
    assert!(
        !wrongly_refused.is_empty(),
        "the oracle contains no name Tailwind refuses, so the soundness assertion above is over \
         an empty set and this gate cannot fail"
    );
}

/// **Every name the table accepts was asked about**, which is the assertion that was missing.
///
/// The three buckets above are over `compiler/style/candidates.txt`, and a name that is not in it
/// is in none of them. `beck_core::style::is_utility` accepted `size-screen`, `max-w-auto` and
/// fifteen `-auto` paddings — none of which Tailwind emits anything for, all of which would have
/// gone into a stylesheet as a rule the browser finds nothing behind — through however many green
/// runs of the differential above, because the list had never been asked.
///
/// [`docs/82`](../../../../docs/82-the-edge-report.md) §82.10 is the pattern and this is its cure:
/// the table **enumerates itself**, and the closed part of it has to appear in the list. What the
/// gate cannot reach is the open part — the spacing scale is multiplicative and therefore infinite
/// — and `candidates.txt` carries a sample of it for the differential above.
#[test]
fn every_name_the_table_accepts_was_asked_about() {
    let oracle = oracle();
    let (names, variants) = beck_core::style::enumerate();
    assert!(names.len() > 1_500, "only {} names enumerated", names.len());
    let unasked: Vec<&String> = names
        .iter()
        .filter(|n| !oracle.verdicts.contains_key(*n))
        .collect();
    assert!(
        unasked.is_empty(),
        "this table accepts {} names the oracle was never asked about, so nothing checks what a \
         page carrying one would be styled with: {unasked:?}",
        unasked.len()
    );
    // The variants are crossed with a utility rather than listed alone, because a variant is not a
    // class: `hover` on its own is nothing and `hover:flex` is the thing that has a rule.
    let unasked: Vec<String> = variants
        .iter()
        .map(|v| format!("{v}:flex"))
        .filter(|n| !oracle.verdicts.contains_key(n))
        .collect();
    assert!(
        unasked.is_empty(),
        "these variants were never asked about: {unasked:?}"
    );
}

/// **The theme is Tailwind's**, entry for entry.
///
/// §104.4 takes the design system, and a ramp is the part of it that cannot be derived from
/// anything: `oklch(50.8% 0.118 165.612)` is somebody's taste written down. So the table is
/// transcribed from the oracle's own output and this holds every entry to it.
///
/// **What this gate is and is not.** It cannot catch the transcription that produced the table —
/// comparing a copy against its source is a tautology at the moment the copy is made. What it
/// catches is an edit to the table afterwards, and a pinned version that moves underneath it, which
/// is the whole life of the table after today. Saying so is the point: it is a regression gate
/// rather than a derivation, and a reader who thought otherwise would trust it for something it
/// does not do.
#[test]
fn the_theme_is_tailwinds() {
    let oracle = oracle();
    let (mut wrong, mut unasked) = (Vec::new(), Vec::new());
    for (token, value) in beck_core::style::theme_tokens() {
        match oracle.theme.get(*token) {
            Some(want) if want == value => {}
            Some(want) => wrong.push((token.to_string(), want.clone(), value.to_string())),
            None => unasked.push(token.to_string()),
        }
    }
    assert!(
        wrong.is_empty(),
        "these theme tokens disagree with Tailwind — (token, Tailwind, Beck): {wrong:?}"
    );
    assert!(
        unasked.is_empty(),
        "the theme defines these and the oracle was never asked about them: {unasked:?}"
    );
    let compared = beck_core::style::theme_tokens().len();
    assert!(compared > 250, "only {compared} tokens were compared");
    // The other direction is a gap rather than a failure, exactly as it is for the utilities: a
    // token Tailwind's theme has and this does not is a family this table has not taken.
    println!(
        "\n  {compared} theme tokens, all Tailwind's own; {} of Tailwind's are not here.",
        oracle.theme.len() - compared
    );

    // The registered properties and the fallback condition are the same arrangement one level down:
    // an `initial-value` nobody set is a `border` that draws nothing.
    for (token, value) in beck_core::style::properties() {
        assert_eq!(
            Some(value.replace(' ', "")),
            oracle.properties.get(*token).map(|w| w.replace(' ', "")),
            "`@property {token}` disagrees with Tailwind, or was never asked about"
        );
    }
    assert_eq!(
        beck_core::style::supports().replace(' ', ""),
        oracle.supports.replace(' ', ""),
        "the fallback's condition is not the one Tailwind ships"
    );
}

/// **The sheet a program gets is the rules its classes need, and nothing else.**
///
/// §104.4's first four words — *exact extraction* — as an assertion rather than a description. The
/// sketch carries seventeen classes and the sheet defines seventeen selectors; it defines the six
/// theme tokens those rules read and not the other 287; and it defines a rule for no class the
/// program cannot carry, which is the half a scanner cannot promise (§104.3 measured Tailwind's own
/// over this tree and found it reading English prose out of comments).
#[test]
fn the_sheet_a_program_gets_is_the_rules_its_classes_need() {
    let placed = example("examples/todo.beck");
    let styles = classes(&placed.program);
    let sheet = beck_core::style::stylesheet(&styles);

    let utilities: Vec<&Arc<str>> = styles
        .classes
        .iter()
        .filter(|c| beck_core::style::is_utility(c))
        .collect();
    assert_eq!(
        utilities.len(),
        styles.classes.len(),
        "the sketch carries a class that is not a utility, so this gate is measuring a page that \
         cannot be fully styled: {:?}",
        styles.classes
    );
    for class in &utilities {
        let rule = beck_core::style::rule(class).expect("a utility has a rule");
        assert!(
            sheet.contains(&rule.selector),
            "`{class}` can reach a page and the sheet has no rule for it"
        );
    }
    // And nothing else: every selector in the sheet is one of those, or the preflight's.
    let selectors = sheet
        .lines()
        .filter(|l| l.starts_with('.') || l.starts_with(":where("))
        .count();
    assert_eq!(
        selectors,
        utilities.len(),
        "the sheet defines {selectors} utility rules for {} classes, so it carries rules for \
         classes this program cannot produce",
        utilities.len()
    );

    // The theme is the same argument one level down: a page using one colour defines one colour.
    let defined = sheet
        .lines()
        .find(|l| l.starts_with(":root{"))
        .expect("the sheet defines the tokens its rules read");
    let defined = defined.matches("--").count();
    assert!(
        defined < 12,
        "the sheet defines {defined} theme tokens for a page that uses one colour and one type \
         size, so it is shipping the theme rather than the part of it the rules read"
    );
    for token in ["--color-gray-500", "--spacing", "--text-2xl"] {
        assert!(
            sheet.contains(&format!("{token}:")),
            "the sketch's rules read `{token}` and the sheet does not define it"
        );
    }
    assert!(
        !sheet.contains("--color-emerald-700"),
        "the sheet defines a colour no rule in it reads"
    );
}

/// **Both settings run**, which is what [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8
/// asks of a choice the compiler makes unbidden.
///
/// `AppConfig::styles` is the off switch §104.4 spells `styles = none`, and a default nobody has
/// run is a claim. So this starts the runtime twice and reads what `/beck.css` would serve: the
/// program's own sheet, and nothing at all. What it is **not** is a test that the switch exists —
/// that would pass on a field nothing reads.
#[test]
fn the_stylesheet_has_an_off_switch_and_both_settings_run() {
    let served = |styles: bool| -> String {
        let placed = example("examples/todo.beck");
        let backend = beck_eval::backend(&placed);
        let runtime = beck_rt::Runtime::new(placed, backend).expect("prepares");
        let config = beck_rt::AppConfig {
            styles,
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        rt.block_on(async {
            let app = beck_rt::App::start(runtime, Arc::new(beck_rt::MemoryLog::new()), config)
                .await
                .expect("the app starts");
            app.stylesheet().to_string()
        })
    };

    let on = served(true);
    assert!(
        on.contains(".line-through{") && on.contains("--spacing:"),
        "the default serves no sheet:\n{on}"
    );
    assert!(
        served(false).is_empty(),
        "`styles` off still served a stylesheet, so the switch is not one"
    );
}

/// **A page's own class names are not utilities, and the report says which is which.**
///
/// Every `class=` in this tree is a semantic name — `done`, `mine`, `column` — served by the eight
/// hard-coded rules in `beck-rt/src/css.rs`, and none of them is a Tailwind utility. That is not a
/// defect in those programs and the table must not treat it as one: §104.4 takes Tailwind's *names*
/// for the utilities it has, and a program's own names are the program's own. What the compiler owes
/// is to tell the two apart, so that the item which emits a stylesheet emits rules for the first and
/// leaves the second alone.
#[test]
fn a_programs_own_class_names_are_not_mistaken_for_utilities() {
    use beck_core::style::is_utility;

    for file in [
        "examples/routed.beck",
        "examples/board.beck",
        "corpus/02-chat.beck",
    ] {
        for class in classes(&example(file).program).classes {
            assert!(
                !is_utility(&class),
                "{file}'s `{class}` is being read as a Tailwind utility, so a stylesheet would \
                 claim a rule for it that this program serves itself"
            );
        }
    }
    // And the other direction on one page, so this is not a test that nothing is ever a utility.
    let styles = classes(
        &compile(
            "utilities.beck",
            &page(r#"class=["flex", "items-center", "gap-2", "mine"]"#),
        )
        .program,
    );
    let known: Vec<&str> = styles
        .classes
        .iter()
        .filter(|c| is_utility(c))
        .map(|c| &**c)
        .collect();
    assert_eq!(known, vec!["flex", "gap-2", "items-center"]);
}
