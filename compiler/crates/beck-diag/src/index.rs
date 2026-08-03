//! The error index: every code the compiler can emit, in one place.
//!
//! [`crate::Diagnostic::code`] promised "a stable code (`B0101`) so that tests, docs and the
//! eventual error index agree". This is that index, and the agreement is a test rather than an
//! intention: `beck-cli/tests/docs.rs` scans every non-test source file in the workspace for a
//! `"Bnnnn"` literal and fails if the set it finds differs from [`INDEX`] in either direction. A
//! code cannot be added without an entry, and an entry cannot outlive its code.
//!
//! # What an entry says, and where it comes from
//!
//! Every diagnostic site in the compiler already carries a label, usually a note and often a
//! fix-it — prose the project wrote next to the condition that raises it.
//! [`CodeEntry::explain`] is that prose, condensed. Nothing here is a second, independent account
//! of what the compiler does, because a second account is a thing that goes out of date.
//!
//! Ranges are not decorative: the first two digits say which stage refused the program, which is
//! usually the first question.
//!
//! | Range | Stage |
//! |---|---|
//! | `B01xx` | Reading the source — lexer, S-expression reader, parser |
//! | `B02xx` | Macro expansion, and the `ui` block it desugars |
//! | `B03xx` | Names, types and effects — the checker |
//! | `B04xx` | Placement (`B040x`) and security (`B041x`) |
//! | `B05xx` | The signal graph and the slicer |
//! | `B06xx` | Modules, interfaces and separate compilation |
//! | `B07xx` | Tests written in Beck |

/// The stage that raises a code.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Syntax,
    Macros,
    Types,
    Placement,
    Security,
    Signals,
    Modules,
    Tests,
}

impl Stage {
    pub fn title(&self) -> &'static str {
        match self {
            Stage::Syntax => "Reading the source",
            Stage::Macros => "Macro expansion",
            Stage::Types => "Names, types and effects",
            Stage::Placement => "Placement",
            Stage::Security => "Placement and security",
            Stage::Signals => "The signal graph and the slicer",
            Stage::Modules => "Modules and interfaces",
            Stage::Tests => "Tests written in Beck",
        }
    }

    /// Every stage, in the order the compiler runs them — which is the order the index is read in.
    pub fn all() -> &'static [Stage] {
        &[
            Stage::Syntax,
            Stage::Macros,
            Stage::Types,
            Stage::Placement,
            Stage::Security,
            Stage::Signals,
            Stage::Modules,
            Stage::Tests,
        ]
    }
}

/// One code, as the index describes it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CodeEntry {
    pub code: &'static str,
    pub stage: Stage,
    /// The diagnostic's message, with the parts that vary at the call site generalised.
    pub title: &'static str,
    /// Why the compiler refuses this, in the terms the diagnostic itself uses.
    pub explain: &'static str,
    /// True for the codes raised as warnings rather than errors.
    pub warning: bool,
}

/// Look a code up. The lookup `beck explain error B0301` and the rendered index both use.
pub fn lookup(code: &str) -> Option<&'static CodeEntry> {
    INDEX.iter().find(|e| e.code.eq_ignore_ascii_case(code))
}

/// Every code in one stage, in code order.
pub fn in_stage(stage: Stage) -> impl Iterator<Item = &'static CodeEntry> {
    INDEX.iter().filter(move |e| e.stage == stage)
}

const fn e(
    code: &'static str,
    stage: Stage,
    title: &'static str,
    explain: &'static str,
) -> CodeEntry {
    CodeEntry {
        code,
        stage,
        title,
        explain,
        warning: false,
    }
}

const fn w(
    code: &'static str,
    stage: Stage,
    title: &'static str,
    explain: &'static str,
) -> CodeEntry {
    CodeEntry {
        code,
        stage,
        title,
        explain,
        warning: true,
    }
}

/// Every code the compiler can emit, in code order.
///
/// Sorted, and the test asserts it: an index a reader has to search is an index nobody reads.
pub const INDEX: &[CodeEntry] = &[
    // ------------------------------------------------------------------ B01xx: reading the source
    e(
        "B0100",
        Stage::Syntax,
        "unrecognised character",
        "The character is not a Beck token. Lexing continues past it so that one stray character \
         does not hide every other problem in the file.",
    ),
    e(
        "B0101",
        Stage::Syntax,
        "inconsistent indentation",
        "This line's indentation matches no enclosing block. Indentation is significant, and \
         §2.6 fixes it as spaces only, four per level — a tab is a lint rather than a width \
         question. Blank and comment-only lines have no indentation at all, so a comment at \
         column zero does not close a block.",
    ),
    e(
        "B0102",
        Stage::Syntax,
        "an invisible control character in the source",
        "A bidirectional formatting character (UTS #39 §4.1; CVE-2021-42574) or a zero-width \
         no-break space away from the start of the file. These change how the text after them is \
         *displayed* without changing what it means, so a reviewer and the compiler can read the \
         same file differently. Write `\\u{...}` if a string genuinely needs one.",
    ),
    e(
        "B0103",
        Stage::Syntax,
        "an identifier outside the ASCII profile",
        "Beck's identifiers are `[A-Za-z_][A-Za-z0-9_]*`, which is UTS #39's ASCII-Only \
         restriction level. Confusable and mixed-script identifiers are therefore not something \
         the compiler checks for; they are something a program cannot contain.",
    ),
    e(
        "B0110",
        Stage::Syntax,
        "expected a single form",
        "`beck ast` and the macro-debugging paths read exactly one S-expression. A file with a \
         second form at top level is a module, and is read with the module reader instead.",
    ),
    e(
        "B0111",
        Stage::Syntax,
        "unbalanced closing delimiter",
        "A `)`, `]` or `}` with no matching opening delimiter.",
    ),
    e(
        "B0112",
        Stage::Syntax,
        "unclosed list",
        "A list opened here and the file ended before it closed. The span points at the opening \
         delimiter, which is the one worth looking at.",
    ),
    e(
        "B0113",
        Stage::Syntax,
        "mismatched closing delimiter",
        "The closing delimiter is not the one this form opened with — `(a]`. Both spans are \
         reported: where it opened, and where it was closed wrongly.",
    ),
    e(
        "B0114",
        Stage::Syntax,
        "empty application",
        "`()` has no meaning: an application's head is its first element, and there is none. \
         Write `unit` for the unit value.",
    ),
    e(
        "B0115",
        Stage::Syntax,
        "unclosed string",
        "A string literal opened and the line or file ended before the closing quote. Beck string \
         literals do not span lines.",
    ),
    e(
        "B0116",
        Stage::Syntax,
        "unreadable character",
        "The S-expression reader cannot begin an atom with this character.",
    ),
    e(
        "B0120",
        Stage::Syntax,
        "unexpected token",
        "The Python-surface parser expected something else here; the message names what. One \
         error per item: the parser recovers to the next top-level item, so a bad line does not \
         make the rest of the file unparseable.",
    ),
    e(
        "B0121",
        Stage::Syntax,
        "nesting is too deep to read",
        "The source nests deeper than the front end follows — `beck_diag::depth::MAX_NESTING` \
         levels of brackets, indentation or S-expression lists. The bound is a fixed count rather \
         than a reading of the stack, so the same file is accepted or refused identically in every \
         build; without it, deep enough input aborted the process with no span at all.",
    ),
    // ------------------------------------------------------------------------ B02xx: macros
    e(
        "B0200",
        Stage::Macros,
        "macro is defined twice",
        "Two macros in one module share a name, and a later definition would silently win.",
    ),
    e(
        "B0201",
        Stage::Macros,
        "macro expansion did not terminate",
        "Expansion ran past the depth limit — usually a macro that expands to a call to itself.",
    ),
    e(
        "B0202",
        Stage::Macros,
        "macro expects an argument",
        "A parameter of the macro received nothing at this call site. The macro's definition is \
         reported as a secondary span.",
    ),
    e(
        "B0203",
        Stage::Macros,
        "too many arguments for macro",
        "More arguments than the macro declares parameters.",
    ),
    e(
        "B0204",
        Stage::Macros,
        "macro returns nothing",
        "A macro body ends without `return quote: …`, so there is no template to instantiate.",
    ),
    e(
        "B0205",
        Stage::Macros,
        "unsupported statement in a macro body",
        "Phase 1 macro bodies are `let` bindings and a final `return quote: …`. Arbitrary \
         compile-time computation arrives with the macro interpreter, which is not built.",
    ),
    e(
        "B0206",
        Stage::Macros,
        "unquoting an unbound name",
        "`$x` inside a template names something that is not a parameter of this macro.",
    ),
    e(
        "B0210",
        Stage::Macros,
        "`ui` needs an indented block",
        "Write `ui:` followed by an indented element.",
    ),
    e(
        "B0211",
        Stage::Macros,
        "`ui` block is empty",
        "A view must produce exactly one root element.",
    ),
    e(
        "B0212",
        Stage::Macros,
        "`ui` block has more than one root",
        "An `Html` value is a single tree. Wrap the elements in one — a `div:` or `main:` block.",
    ),
    e(
        "B0213",
        Stage::Macros,
        "the form nests too deep to expand",
        "The expander walks a form's arguments as deeply as they nest, and stops at the count the \
         reader stops at. This is not `B0201`: nothing here says a macro failed to terminate — the \
         two were one counter until they were separated, and a deep expression with no macros in \
         it reported the wrong one.",
    ),
    // ------------------------------------------------------- B03xx: names, types and effects
    e(
        "B0300",
        Stage::Types,
        "not a tier",
        "`@on(…)` takes `client`, `server`, `data` or `any`.",
    ),
    w(
        "B0301",
        Stage::Types,
        "unsupported decorator",
        "The compiler understands `@on(client|server|data|any)` and `@signal`. A warning rather \
         than an error, so an unknown decorator does not stop a build — but it does nothing.",
    ),
    e(
        "B0302",
        Stage::Types,
        "type is declared twice",
        "Two declarations in this module share a type name.",
    ),
    e(
        "B0303",
        Stage::Types,
        "a top-level parameter needs a type annotation",
        "Inference is intra-module and boundaries are declared (§3.6): a top-level definition's \
         parameters are part of its published signature, so they are written rather than guessed.",
    ),
    e(
        "B0304",
        Stage::Types,
        "needs a return type",
        "Same reason as B0303: a top-level definition's result type is part of its contract.",
    ),
    e(
        "B0305",
        Stage::Types,
        "neither an effect nor a row",
        "A `uses` clause names effect atoms and row aliases, and this is neither. `beck doc \
         reference` lists the atom set; a `row Name = …` declaration in the module is what makes a \
         name for a bundle of them.",
    ),
    e(
        "B0307",
        Stage::Types,
        "unsupported top-level item",
        "This form is not something a module may contain at top level.",
    ),
    e(
        "B0308",
        Stage::Types,
        "expected a type",
        "A type position holds something that is not a type expression.",
    ),
    e(
        "B0310",
        Stage::Types,
        "cannot find type",
        "No declaration, import or builtin of that name is in scope.",
    ),
    e(
        "B0311",
        Stage::Types,
        "wrong number of type arguments",
        "A mention of a type carries one argument per declared parameter — `union Tree[T]` is \
         written `Tree[Int]`, never bare `Tree`.",
    ),
    e(
        "B0312",
        Stage::Types,
        "a type alias defined in terms of itself, or a name the language reserves",
        "An alias is transparent, so a self-referential one describes no type — a `union` may be \
         recursive, an alias may not. The same code covers a definition named after a reserved \
         form such as `record`, which would compile and never be reachable.",
    ),
    e(
        "B0313",
        Stage::Types,
        "a type parameter takes no type arguments",
        "A type parameter of the definition or declaration being read names an unknown type, so it \
         has no structure to apply arguments to: `T[Int]` says nothing, whatever `T` turns out to \
         be at the call site.",
    ),
    e(
        "B0314",
        Stage::Types,
        "a type parameter shadows an existing type",
        "A type parameter is a name the definition or declaration invents, and one that shadowed \
         an existing type would make its fields or its signature read as though they mentioned \
         that type.",
    ),
    e(
        "B0315",
        Stage::Types,
        "a type parameter is repeated",
        "The same name appears twice in one type-parameter list, where the second would silently \
         shadow the first.",
    ),
    e(
        "B0316",
        Stage::Types,
        "a declaration cannot bound its type parameter",
        "A bound says what a body may call, and a `model`, a `union`, a `newtype` and a `type` have \
         no body. The definitions that take the type apart are where the bound belongs.",
    ),
    e(
        "B0320",
        Stage::Types,
        "type mismatch",
        "Unification failed; the message names what was being unified — an argument, a field, a \
         result, or the two branches of an `if`. The branches of an `if` are reported as two \
         alternatives rather than as actual-and-expected: typing one as the other's expectation \
         is what refused SICP exercise 1.43 (docs/27 §27.3).",
    ),
    w(
        "B0330",
        Stage::Types,
        "statements after `return` are unreachable",
        "A `return` ends its block; anything after it never runs.",
    ),
    e(
        "B0331",
        Stage::Types,
        "loops are not available in Phase 1",
        "Everything is an expression and `var` is not yet mutable, so a loop has nothing to \
         accumulate into. Use `map_list`, `filter_list` or `fold`.",
    ),
    e(
        "B0332",
        Stage::Types,
        "a `quote` survived macro expansion",
        "A template reached the checker, which means it was never instantiated. Quoted forms are \
         data for a macro, not code.",
    ),
    e(
        "B0333",
        Stage::Types,
        "a keyword argument outside a call",
        "`name=value` is call syntax and has no meaning as an expression on its own.",
    ),
    e(
        "B0334",
        Stage::Types,
        "unsupported expression",
        "This form has no meaning in expression position.",
    ),
    e(
        "B0335",
        Stage::Types,
        "has no body",
        "A bodyless `def` is a declaration, which is what a `.becki` interface file is made of; \
         an ordinary module has to define what it declares.",
    ),
    e(
        "B0340",
        Stage::Types,
        "cannot find name in this scope",
        "No local, parameter, top-level definition, import or prelude name matches — after \
         hygiene, which means a name a macro introduced is not visible to code the macro did not \
         write.",
    ),
    e(
        "B0341",
        Stage::Types,
        "match is not exhaustive",
        "The missing variants are listed. This is deliberate and load-bearing: adding a variant \
         must break every fold that consumes it, which is what makes a missed migration a compile \
         error rather than a 3 a.m. page.",
    ),
    e(
        "B0342",
        Stage::Types,
        "unsupported pattern",
        "A `case` arm's pattern is a variant applied to field bindings, or `_`. This form is \
         neither.",
    ),
    e(
        "B0343",
        Stage::Types,
        "not a constructor",
        "The head of a pattern must be a variant of the union being matched.",
    ),
    e(
        "B0344",
        Stage::Types,
        "cannot tell which field this binds or sets",
        "A pattern or record update names a field the compiler cannot resolve to the type in hand.",
    ),
    e(
        "B0345",
        Stage::Types,
        "nested patterns are not available in Phase 1",
        "A pattern binds field names to fresh locals; matching inside one of those fields is not \
         built.",
    ),
    e(
        "B0346",
        Stage::Types,
        "cannot tell which model this record builds",
        "A record literal's type comes from what is expected of it, and nothing here says which \
         model it is.",
    ),
    e(
        "B0347",
        Stage::Types,
        "expected a field or method name",
        "The right-hand side of a `.` must be a name.",
    ),
    e(
        "B0348",
        Stage::Types,
        "`with` takes named fields",
        "Functional record update is written `t.with(done=…)`.",
    ),
    e(
        "B0349",
        Stage::Types,
        "no such field on this type",
        "The field named in a `with` is not declared on the model being updated.",
    ),
    e(
        "B0350",
        Stage::Types,
        "no field or function for this type",
        "A `.` reached neither a field of the receiver nor a function that could take it.",
    ),
    e(
        "B0351",
        Stage::Types,
        "wrong number of arguments",
        "A call passes a different number of arguments than the function or constructor takes.",
    ),
    e(
        "B0352",
        Stage::Types,
        "not callable",
        "The callee's type is not a function type.",
    ),
    e(
        "B0353",
        Stage::Types,
        "no such variant",
        "The union being constructed or matched has no variant of that name. The type is named in \
         the message.",
    ),
    e(
        "B0354",
        Stage::Types,
        "cannot construct this type",
        "The name is a type, but not one with a constructor — an alias or a builtin.",
    ),
    e(
        "B0360",
        Stage::Types,
        "cannot be called inside a fold",
        "A fold must be replay-pure, and this would make replay non-deterministic. Time is data on \
         the envelope (`env.at`) and entity ids are minted at the edge: mint the id in the \
         client's command and read it from the event.",
    ),
    e(
        "B0370",
        Stage::Types,
        "performs more than its signature declares",
        "The undeclared atoms are listed. A `uses` clause is the published bound, and widening it \
         is a breaking API change — so the compiler will not widen it for you.",
    ),
    // -------------------------------------------------------------------- B038x: traits
    e(
        "B0380",
        Stage::Types,
        "a trait cannot be declared here",
        "The name already belongs to a type, the trait is declared twice, or the file is a \
         `.becki` — a trait does not cross a module boundary, so an interface may not hold one.",
    ),
    e(
        "B0381",
        Stage::Types,
        "a trait declaration is wrong",
        "A trait holds `def` signatures with no bodies, each mentioning `Self` in a parameter so \
         that a call has something to dispatch on. A method name belongs to one trait only.",
    ),
    e(
        "B0382",
        Stage::Types,
        "an impl does not match its trait",
        "An impl writes bodies and parameter *names*; the types, the return type and the effect \
         row are the trait's. Every method must be implemented, exactly once, and no others.",
    ),
    e(
        "B0383",
        Stage::Types,
        "cannot find the trait or the type this impl names",
        "Both halves of `impl Trait for Type` have to resolve before the impl can be registered.",
    ),
    e(
        "B0384",
        Stage::Types,
        "conflicting implementations",
        "Coherence: one impl per trait per type constructor, and no blanket impl over a type \
         parameter — so what a call means never depends on which impls happen to be in scope.",
    ),
    e(
        "B0385",
        Stage::Types,
        "orphan impl",
        "An impl belongs with the trait or with the type. Implementing somebody else's trait for \
         somebody else's type is what lets two modules supply one and disagree.",
    ),
    e(
        "B0386",
        Stage::Types,
        "no implementation can be chosen here",
        "An implementation comes from a concrete type or from a bound on a type parameter — write \
         `[T: Trait]` to say the parameter has one. A trait method and a bounded definition are \
         both called rather than passed: the implementation is supplied at the call site, so a \
         reference that is never called has nowhere to receive it.",
    ),
    e(
        "B0387",
        Stage::Types,
        "the type does not implement the trait",
        "There is no `impl Trait for Type` in scope for the receiver's type.",
    ),
    e(
        "B0390",
        Stage::Types,
        "the expression nests too deep to check",
        "The checker walks an expression and a type as deeply as they nest, and stops at \
         `beck_diag::depth::MAX_NESTING` levels — the same count the reader stops at, because the \
         checker can be handed a tree a macro produced rather than one anybody typed. Everything \
         downstream walks the `Core` this pass built, so it is bounded by the same number.",
    ),
    e(
        "B0391",
        Stage::Types,
        "a raised value must have a declared type",
        "`raise` performs `raises(T)`, and the atom names `T` so that a handler can say what it \
         catches. A builtin will not do: `raises(Int)` would make every integer failure in a \
         program the same failure, and a handler could not tell them apart.",
    ),
    e(
        "B0392",
        Stage::Types,
        "nothing here can fail, and nothing says what this would catch",
        "A `try:` reifies one failure into a `Result`, and it takes the error type from the \
         enclosing signature's `Result[T, E]` where there is one and from the block's own row \
         where there is not. Neither said anything here. Either the call you meant to make is not \
         there, or the `try:` is left over from a signature that has stopped failing — which is \
         the good case, and the diagnostic is how you find out.",
    ),
    e(
        "B0393",
        Stage::Types,
        "the block can fail in more than one way, and nothing says which to catch",
        "A `Result[T, E]` has one error type, and a `try:` catches one — the rest keep travelling, \
         which is what makes a handler composable. Here nothing named which: give the enclosing \
         definition a `Result[T, E]` return type, and the handler catches that `E`.",
    ),
    e(
        "B0394",
        Stage::Types,
        "the row is declared twice",
        "Two `row Name = …` declarations with the same name. A row alias is a name for a bundle of \
         effect atoms, and a second one would make every `uses` clause mentioning it ambiguous.",
    ),
    e(
        "B0395",
        Stage::Types,
        "the host of an outbound call has to be written at the call site",
        "`http_fetch` performs `net.out(host)`, and §6.5 derives the cluster's egress policy from \
         that atom and nothing else. A host computed at run time is an outbound call the \
         deployment cannot be told about, so the argument is read where it is written. Compute \
         the path, the port, the headers and the body; or take a closure, so the caller names its \
         own host and the row carries the atom out.",
    ),
    e(
        "B0396",
        Stage::Types,
        "that is not a host an outbound call can name",
        "The host becomes a NetworkPolicy peer and a `uses net.out(…)` clause, both of which are \
         written as bare DNS labels — so a scheme, a port or a path in it is a name neither could \
         carry. `origin` is refused for a different reason: it is the one outbound atom a client \
         tier discharges, and a client reaches its own server over the command channel.",
    ),
    // -------------------------------------------------------------------- B04xx: placement
    e(
        "B0400",
        Stage::Placement,
        "performs effects no single tier can discharge",
        "Each tier discharges a fixed set (§3.3). A row no tier covers has to be split across \
         definitions that can each be placed.",
    ),
    e(
        "B0401",
        Stage::Placement,
        "placed on a tier that cannot discharge an effect it performs",
        "The written `@on(…)` and the inferred row disagree. The diagnostic names the atom, the \
         tier, and the tiers that could discharge it. `ingress` is the merge point and only the \
         server holds it; `durable` is the data tier's; `dom` is the browser's.",
    ),
    e(
        "B0402",
        Stage::Placement,
        "a fold function must be replay-pure",
        "The function reached by a `fold` performs effects that replay would not reproduce. Both \
         the fold and the definition are reported.",
    ),
    e(
        "B0403",
        Stage::Placement,
        "a program has exactly one merge point",
        "A second `merge_clients()`. The merge point is where time and nondeterminism enter; two \
         of them would mean two total orders, and replay would no longer be a function of the log.",
    ),
    e(
        "B0404",
        Stage::Placement,
        "cannot be unplaced",
        "`@on(any)` means every tier, and an atom in the row is not discharged on every tier. The \
         fix-it names the tiers that can.",
    ),
    e(
        "B0410",
        Stage::Security,
        "runs on the client, so its value must be Sendable",
        "This value crosses to the browser, and §3.5's claim is that the compiler proves a secret \
         cannot. The offending field and the path that reaches it are both named — `beck explain \
         flow <Type>` prints the same walk.",
    ),
    e(
        "B0411",
        Stage::Security,
        "durable, so its state must be storable",
        "The log is the only description of this program's history; a value it cannot read back \
         is a state replay would not reproduce.",
    ),
    e(
        "B0412",
        Stage::Security,
        "requires a capability nothing can discharge",
        "A `Session` reaches exactly one place in a Beck program: the validator `decide` is given, \
         which is the only function handed a `Proposal`. Authority is one chokepoint (§3.5), so a \
         capability required outside it has no holder.",
    ),
    // ------------------------------------------------------ B05xx: the signal graph and slicer
    e(
        "B0500",
        Stage::Signals,
        "this program has no merge point",
        "A Beck application is a fold over an event stream, and the stream starts at \
         `merge_clients()`. Not an error for a library: this code, with B0501 and B0505, is what \
         says a module is a domain module rather than an application.",
    ),
    e(
        "B0501",
        Stage::Signals,
        "this program has no durable state",
        "`durable(fold(f, init, s))` is what makes the log a database.",
    ),
    e(
        "B0502",
        Stage::Signals,
        "`durable` must wrap a `fold`",
        "Only a fold has an accumulator to persist.",
    ),
    e(
        "B0504",
        Stage::Signals,
        "events must come from `decide`",
        "The fold has no chokepoint upstream of it. `decide` is the sole consumer of ingress and \
         the one place a command becomes an event — §3.5's \"authority is one chokepoint\".",
    ),
    e(
        "B0505",
        Stage::Signals,
        "no signal is placed on the client",
        "`page` is the tier crossing: a `Signal[Html]` the browser subscribes to.",
    ),
    e(
        "B0506",
        Stage::Signals,
        "not a signal",
        "A signal's inputs are other signals. A function is applied *through* a construct — \
         `signal_map(s, f)` — rather than named as an input.",
    ),
    e(
        "B0507",
        Stage::Signals,
        "not a signal construct",
        "§3.7's signal vocabulary is `merge_clients`, `filter_map`, `fold`, `durable`, \
         `signal_map`, `map2`, `per_session` and `decide`.",
    ),
    e(
        "B0508",
        Stage::Signals,
        "unsupported signal expression",
        "A signal is a node in the dataflow, not a computation. The computation goes in a `def` \
         and the signal names it: `summary: Signal[Summary] = signal_map(counts, summarise)`.",
    ),
    e(
        "B0509",
        Stage::Signals,
        "a signal defined in terms of itself",
        "The cycle is printed. A cycle through a `fold` is sound — an accumulator is a value, so \
         the recursion has a bottom, which is why `events → todos → events` is legal. One with no \
         fold in it has no first value to compute from.",
    ),
    e(
        "B0510",
        Stage::Signals,
        "two signals are the page, and there is no router yet",
        "The slicer will slice both; the runtime serves one document per connection, and choosing \
         between them is routing — a Phase 3 client bullet that is not built.",
    ),
    e(
        "B0511",
        Stage::Signals,
        "a program has one authority chokepoint",
        "A second `decide`. §3.5 rests on validation being one place: two of them are two answers \
         to \"may this actor do this\", and the log would record whichever ran.",
    ),
    e(
        "B0512",
        Stage::Signals,
        "the chokepoint does not read a durable fold",
        "`decide` threads the accumulator through validation, so what it reads has to be one — \
         that is what makes first-writer-wins and ownership decidable (§3.7).",
    ),
    e(
        "B0513",
        Stage::Signals,
        "a fold that is not durable",
        "Its accumulator has nowhere to live across a restart. The log is what survives, and \
         `durable` is what says an accumulator is folded from it.",
    ),
    // --------------------------------------------------------- B06xx: modules and interfaces
    e(
        "B0600",
        Stage::Modules,
        "has a body, so it is not a signature",
        "A `.becki` publishes what a module offers, not how it does it. Regenerate it with \
         `beck iface`.",
    ),
    e(
        "B0601",
        Stage::Modules,
        "defined in more than one module",
        "Phase 2 links modules into one namespace and has no qualified reference to tell two \
         definitions apart, so a clash is an error rather than a shadowing rule.",
    ),
    e(
        "B0602",
        Stage::Modules,
        "a module imports itself, directly or through a cycle",
        "A module's interface is derived from its body, so a cycle would mean each module needed \
         the other's contract before either had one. The cycle is printed.",
    ),
    e(
        "B0603",
        Stage::Modules,
        "cannot find module",
        "The loader looked for `<name>.becki` and `<name>.beck` and found neither.",
    ),
    e(
        "B0604",
        Stage::Modules,
        "has an interface but no implementation",
        "An interface is enough to compile against and never enough to run.",
    ),
    e(
        "B0605",
        Stage::Modules,
        "does not match its published interface",
        "The checked-in `.becki` and the module compile to different digests. Regenerate it with \
         `beck iface`, and review the diff — the difference is an API change.",
    ),
    // ------------------------------------------------------------ B07xx: tests written in Beck
    e(
        "B0700",
        Stage::Tests,
        "a test block performs effects",
        "A test block's own row must be empty: an expectation is a pure question about a state, a \
         log and a page. Effects belong to the *subject*, and §21.3 stubs those.",
    ),
    e(
        "B0701",
        Stage::Tests,
        "a property parameter needs a type",
        "The generator is type-directed, so it works from the parameter's declared type. A \
         `property` with no types has nothing to generate.",
    ),
    e(
        "B0702",
        Stage::Tests,
        "not a tier, or not an effect atom",
        "`expect place(name) == tier` takes a tier, and a `stub` names an effect atom.",
    ),
    e(
        "B0703",
        Stage::Tests,
        "not something a stub can stand in for",
        "Time, ids and persistence are not stubbed in Beck and there is nothing to write: the \
         clock is data on the envelope, ids are minted at the edge, and the durable fold is real \
         and in memory.",
    ),
    e(
        "B0704",
        Stage::Tests,
        "nothing in this program performs this atom",
        "The stub would never be reached. The complete list of what a program touches is its \
         effect rows, and this atom is not among them.",
    ),
    e(
        "B0705",
        Stage::Tests,
        "only `given`, `when`, `stub` and `expect` may appear in a test",
        "§21.2: a test names a log, an input and an expectation — there is no fixture to build and \
         no `setUp` to write.",
    ),
    e(
        "B0706",
        Stage::Tests,
        "a clause needs something this program does not have",
        "The state a test arranges is a fold over the program's own event stream, so a program \
         with no `merge_clients` → `decide` → `durable(fold(…))` has nothing for `given` and \
         `when` to mean.",
    ),
    e(
        "B0707",
        Stage::Tests,
        "an atom is performed by more than one definition, so a stub cannot answer from the call",
        "The performers are named. A stub is a value for an effect atom; where two definitions \
         perform the same atom with different result types, one value cannot serve both.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_is_sorted_and_has_no_duplicates() {
        let codes: Vec<&str> = INDEX.iter().map(|e| e.code).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(codes, sorted, "the index must be sorted and unique");
    }

    #[test]
    fn every_entry_says_something() {
        for entry in INDEX {
            assert!(
                entry.code.len() == 5 && entry.code.starts_with('B'),
                "{entry:?}"
            );
            assert!(!entry.title.is_empty(), "{entry:?}");
            // Long enough that "see the source" cannot pass for an explanation.
            assert!(entry.explain.len() > 40, "{entry:?}");
        }
    }

    #[test]
    fn a_code_can_be_looked_up_either_way() {
        assert_eq!(lookup("B0341").map(|e| e.stage), Some(Stage::Types));
        assert_eq!(lookup("b0341").map(|e| e.code), Some("B0341"));
        assert_eq!(lookup("B9999"), None);
    }

    #[test]
    fn every_code_belongs_to_the_stage_its_number_names() {
        for entry in INDEX {
            let band = &entry.code[1..3];
            let expected: &[Stage] = match band {
                "01" => &[Stage::Syntax],
                "02" => &[Stage::Macros],
                "03" => &[Stage::Types],
                // B040x is placement, B041x is the security check that runs with it.
                "04" => &[Stage::Placement, Stage::Security],
                "05" => &[Stage::Signals],
                "06" => &[Stage::Modules],
                "07" => &[Stage::Tests],
                other => panic!("unknown band B{other}xx in {entry:?}"),
            };
            assert!(expected.contains(&entry.stage), "{entry:?}");
        }
    }
}
