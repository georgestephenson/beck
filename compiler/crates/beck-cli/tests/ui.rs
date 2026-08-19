//! `tests/ui/` — the diagnostic snapshot suite §4.5 asks for.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.5:
//!
//! > A `tests/ui/` snapshot suite (rustc-style): every diagnostic has a committed expected
//! > rendering, so regressions in error quality fail CI. Use `insta` for snapshots. Start this in
//! > week two, not year two.
//!
//! It was not started in week two. [`docs/19-phase-1-report.md`](../../../../docs/19-phase-1-report.md)
//! §19.9 lists it as still missing and assigns it to Phase 2, with the reason it matters: asserting
//! a diagnostic by its code and message "catches regressions in *content* but not in *rendering*".
//! A message that is right and a caret that points at the wrong token is a bug a code assertion
//! cannot see.
//!
//! # What is snapshotted
//!
//! The *rendered* diagnostic — the whole thing a person reads, carets, labels, notes and fix-its.
//! Each case is a program that a reasonable author might write and that the compiler must refuse
//! well. Reviewing a diff here is reviewing the error message, which is the point: §4.5 opens with
//! "for a language whose main feature is inference, error quality *is* the product."
//!
//! Run `cargo insta review` after a deliberate change.

/// Compile a source string and render every diagnostic exactly as the CLI would.
fn render(src: &str) -> String {
    let (_, diags, map) = beck_core::compile_str("t.beck", src);
    let out = diags.render(&map);
    assert!(!out.is_empty(), "this program was supposed to be refused");
    out
}

macro_rules! ui {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(render($src));
        }
    };
}

/// The program most cases perturb, so that a snapshot shows one failure rather than a cascade.
const BASE: &str = include_str!("../../../examples/todo.beck");

fn edited(from: &str, to: &str) -> String {
    let out = BASE.replace(from, to);
    assert_ne!(out, BASE, "the edit `{from}` did not apply");
    out
}

// ---------------------------------------------------------------------------------------------
// Placement, §3.3
// ---------------------------------------------------------------------------------------------

ui!(
    durable_on_the_client,
    &edited("@on(data)\ntodos", "@on(client)\ntodos")
);

ui!(
    ingress_on_the_client,
    &edited("@on(server)\nproposals", "@on(client)\nproposals")
);

ui!(
    a_named_host_from_a_browser,
    &edited(
        "def done_class(t: Todo) -> Str:",
        "def done_class(t: Todo) -> Str uses net.out(telemetry.example.com):"
    )
);

// ---------------------------------------------------------------------------------------------
// Determinism, §3.7
// ---------------------------------------------------------------------------------------------

ui!(
    a_fold_that_mints_an_id,
    &edited(
        "return s.with(todos=map_remove(s.todos, id))",
        "return s.with(todos=map_remove(s.todos, Id(value=uuid())))"
    )
);

ui!(
    a_fold_that_reads_the_clock_through_a_function,
    &edited(
        "def toggled(s: State, id: Id) -> State:",
        "def stamp(id: Id) -> Id:\n    return Id(value=str(now()))\n\ndef toggled(s: State, id: Id) -> State:"
    )
    .replace(
        "return s.with(todos=map_remove(s.todos, id))",
        "return s.with(todos=map_remove(s.todos, stamp(id)))"
    )
);

ui!(
    two_merge_points,
    &edited(
        "@on(server)\nproposals: Stream[Proposal] = merge_clients()",
        "@on(server)\nproposals: Stream[Proposal] = merge_clients()\n\n@on(server)\nother: Stream[Proposal] = merge_clients()"
    )
);

// ---------------------------------------------------------------------------------------------
// Effects, §3.2 and §3.6
// ---------------------------------------------------------------------------------------------

ui!(
    an_undeclared_effect,
    &edited(
        "-> Str:\n    return \"flex gap-2 items-baseline line-through\" if t.done \
         else \"flex gap-2 items-baseline\"",
        "-> Str uses log:\n    return \"done\" if t.done else str(now())"
    )
);

ui!(
    an_effect_that_is_not_an_effect,
    "\
def f() -> Int uses teleport:
    return 1
"
);

// ---------------------------------------------------------------------------------------------
// Security, §3.5
// ---------------------------------------------------------------------------------------------

ui!(
    a_secret_in_a_command,
    &edited(
        "    Toggle(id: Id)\n    Delete(id: Id)",
        "    Toggle(id: Id)\n    Delete(id: Id)\n    Authenticate(token: secret[Str])"
    )
    .replace(
        "        case Delete(id):\n            return if_owned(s, p, id, [Deleted(id=id)])",
        "        case Delete(id):\n            return if_owned(s, p, id, [Deleted(id=id)])\n        case Authenticate(token):\n            return Err(error=BlankText)"
    )
);

ui!(
    a_view_cached_in_the_state,
    &edited(
        "model State:\n    todos: Map[Id, Todo]",
        "model State:\n    todos: Map[Id, Todo]\n    cached: Html"
    )
);

ui!(
    a_capability_with_no_holder,
    &edited(
        "def done_class(t: Todo) -> Str:",
        "def purge(t: Todo) -> Str uses cap.admin:\n    return t.text\n\ndef done_class(t: Todo) -> Str:"
    )
);

// ---------------------------------------------------------------------------------------------
// Types and shape
// ---------------------------------------------------------------------------------------------

ui!(
    a_match_that_misses_a_case,
    &edited(
        "        case Deleted(id):\n            return s.with(todos=map_remove(s.todos, id))\n",
        ""
    )
);

ui!(
    a_loop,
    "\
def total(xs: list[Int]) -> Int:
    for x in xs:
        return x
    return 0
"
);

ui!(
    a_bodyless_definition_in_a_module,
    "\
def promised(x: Int) -> Int
"
);

ui!(
    a_definition_named_after_a_form,
    "\
def record(x: Int) -> Int:
    return x
"
);

ui!(
    a_program_that_is_not_an_application,
    "def helper(x: Int) -> Int:\n    return x + 1\n"
);

// ---------------------------------------------------------------------------------------------
// Tests written in Beck, §21.2–§21.3
//
// A test construct is the first thing an outside developer reaches for, so its refusals are the
// first diagnostics they see. Each of these is a mistake somebody will make on their first day.
// ---------------------------------------------------------------------------------------------

ui!(
    a_test_that_performs_an_effect,
    &format!(
        "{}\ndef phone_home() -> Bool uses net.out(status.example.com):\n    return True\n\n\
         test \"is the world still there\":\n    expect phone_home()\n",
        BASE
    )
);

ui!(
    a_stub_for_something_nothing_performs,
    &format!(
        "{}\ntest \"a stub with nowhere to go\":\n    stub net.out(nobody.example.com): True\n",
        BASE
    )
);

ui!(
    a_stub_for_the_durable_fold,
    &format!(
        "{}\ntest \"the database is real and in memory\":\n    stub durable: True\n",
        BASE
    )
);

ui!(
    a_given_that_is_not_this_programs_log,
    &format!(
        "{}\ntest \"a fixture, not a log\":\n    given [1, 2, 3]\n",
        BASE
    )
);

ui!(
    a_statement_that_is_not_a_clause,
    &format!(
        "{}\ntest \"a setUp that is not one\":\n    fixture = 1\n    expect list_len(events) == 0\n",
        BASE
    )
);

// `expect page …` now has two continuations, so the refusal has to name both — a message that
// still said "expected `contains` and a string" would send a reader looking for a syntax error in
// the thing they wrote correctly (docs/22 §22.10).
ui!(
    a_page_expectation_that_is_neither_contains_nor_a_snapshot,
    &format!(
        "{}\ntest \"neither\":\n    given []\n    expect page resembles \"milk\"\n",
        BASE
    )
);

ui!(
    a_test_in_a_program_with_nothing_to_fold,
    "def helper(x: Int) -> Int:\n    return x + 1\n\ntest \"x\":\n    given []\n"
);

ui!(
    a_stub_body_where_two_definitions_perform_the_effect,
    &format!(
        "{}\ndef charge(n: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\n\
         def refund(n: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\n\
         test \"which one's arguments?\":\n    stub net.out(pay.example.com):\n        return n > 1\n",
        BASE
    )
);

ui!(
    bare_case_arms_with_more_than_one_argument,
    &format!(
        "{}\ndef charge(n: Int, tries: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\n\
         test \"what is the scrutinee?\":\n    stub net.out(pay.example.com):\n        case 1:\n            return True\n        case _:\n            return False\n",
        BASE
    )
);

// ---------------------------------------------------------------------------------------------
// The `ui:` vocabulary, docs/104 §104.8's Wall 2
// ---------------------------------------------------------------------------------------------

ui!(
    an_event_the_client_does_not_listen_for,
    &edited(
        "span(class=[\"flex-1\"], on_click=Toggle(id=t.id)): t.text",
        "span(on_keydown=Toggle(id=t.id)): t.text"
    )
);

// `cls=` is the spelling docs/01 §1.3's own sketch uses, so this is the message a reader arriving
// from that page gets. A distance search would say `cols`, which is one edit away and useless.
ui!(
    the_spelling_the_original_sketch_uses,
    &edited("class=done_class(t)", "cls=done_class(t)")
);

// The systematic mistake rather than the random one: `ui:` turns `_` into `-`, and HTML spells
// this attribute with nothing in between.
ui!(
    an_attribute_written_the_way_the_language_spells_everything_else,
    &edited(
        "placeholder=\"what needs doing?\", autofocus=\"on\",",
        "placeholder=\"what needs doing?\", autofocus=\"on\", max_length=\"80\","
    )
);

// ---------------------------------------------------------------------------------------------
// Accessibility, docs/12 §12.4's first three checks
// ---------------------------------------------------------------------------------------------

// The commonest real failure of WCAG 3.3.2, and this is the message the four programs in this tree
// that had it would have got. The label is different from the one an unadorned control gets, on
// purpose: somebody who wrote a placeholder believes they *have* labelled it, so the sentence has
// to say why that is not so rather than repeat the rule.
ui!(
    a_placeholder_is_not_a_label,
    &edited("aria_label=\"what needs doing?\", ", "")
);

ui!(
    an_image_with_no_alt_text,
    &edited(
        "h1(class=[\"text-2xl\", \"font-bold\"]): \"todos\"",
        "img(src=\"/logo.png\")"
    )
);

// A button with no children and no `aria_label` — the icon-button mistake, which is the one shape
// where a button that looks fine in a browser announces nothing at all.
ui!(
    a_button_that_announces_nothing,
    &edited(
        "span(class=[\"flex-1\"], on_click=Toggle(id=t.id)): t.text",
        "button(on_click=Toggle(id=t.id))"
    )
);

// ---------------------------------------------------------------------------------------------
// D1's non-durable fold, docs/104 §104.8's Wall 1
// ---------------------------------------------------------------------------------------------

// A program whose only accumulator is an unwrapped fold used to be reported as a *library* with no
// durable state, which sends its author to add the `durable` they deliberately left off.
ui!(
    a_fold_nobody_wrapped_in_durable,
    &edited(
        "todos: Signal[State] = durable(fold(",
        "todos: Signal[State] = (fold("
    )
);
