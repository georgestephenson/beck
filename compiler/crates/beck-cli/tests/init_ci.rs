//! The workflow `beck init ci` writes, held to the standard this project's own workflows are held
//! to.
//!
//! [`docs/28`](../../../../docs/28-releases-and-deployment.md) §28.3 item 1 specifies the steps.
//! `workflows.rs` is the same set of checks over the workflows in *this* repository, and it exists
//! because one of them was invalid YAML from the day it was written, so every gate in it was
//! silently absent for a phase. A generator that emits such a file emits it into every repository
//! that runs the command, which is the same failure multiplied — so the generated text is parsed
//! with somebody else's YAML parser, checked for the two characters YAML reserves, and compared
//! against the command tree.
//!
//! # The check only a generator needs
//!
//! [`the_workflow_runs_no_command_the_binary_does_not_have`] is the one that could not have been
//! written for a hand-authored file: it reads every `beck <word>` out of the emitted YAML and
//! asserts each is a subcommand `beck --help` lists. A workflow naming a command that was renamed
//! would otherwise fail on its first run, in somebody else's repository, at the step that was
//! supposed to be the gate.

use std::collections::BTreeSet;

use beck_infra::ci;

fn graph() -> beck_infra::InfraGraph {
    let src = include_str!("../../../examples/todo.beck").to_string();
    let (placed, diags, map) = beck_core::compile_str("todo", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    beck_infra::graph(&placed.expect("the sketch slices"))
}

fn workflow() -> String {
    ci::workflow(&graph(), "app.beck")
}

/// Every subcommand the binary has, read from its own help output rather than from a list here.
fn subcommands() -> BTreeSet<String> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
        .arg("--help")
        .output()
        .expect("running `beck --help`");
    assert!(out.status.success(), "`beck --help` failed");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut names = BTreeSet::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with("Options:") {
                break;
            }
            if let Some(word) = line.split_whitespace().next() {
                if word.chars().all(|c| c.is_ascii_lowercase()) && !word.is_empty() {
                    names.insert(word.to_string());
                }
            }
        }
    }
    assert!(
        names.len() > 5,
        "only parsed {names:?} out of the help — this check would pass by looking at nothing"
    );
    names
}

#[test]
fn the_workflow_is_yaml_somebody_elses_parser_accepts() {
    // The failure this file exists for. `serde_norway` is the parser `manifests.rs` reads the
    // generated Kubernetes objects back with, for the same reason: a writer that marks its own
    // homework is not a check.
    let text = workflow();
    let parsed: serde_norway::Value = serde_norway::from_str(&text)
        .unwrap_or_else(|e| panic!("the generated workflow is not YAML: {e}\n{text}"));
    let jobs = parsed
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .expect("a jobs mapping");
    assert!(
        jobs.contains_key(serde_norway::Value::from("check")),
        "the workflow has no check job"
    );
    for (name, job) in jobs {
        let steps = job
            .get("steps")
            .and_then(|s| s.as_sequence())
            .unwrap_or_else(|| panic!("{name:?} has no steps"));
        assert!(!steps.is_empty(), "{name:?} has no steps");
        for step in steps {
            assert!(
                step.get("run").is_some() || step.get("uses").is_some(),
                "{name:?} has a step that neither runs nor uses anything: {step:?}"
            );
        }
    }
}

#[test]
fn no_value_in_the_generated_workflow_starts_with_a_character_yaml_reserves() {
    // `workflows.rs`'s check, applied to what the generator emits. A step named "`beck fmt`
    // round-trips" reads perfectly naturally and makes GitHub Actions reject the whole file.
    for (n, line) in workflow().lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(colon) = trimmed.find(": ") else {
            continue;
        };
        let key = trimmed[..colon].trim_start_matches("- ");
        if !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        let Some(first) = trimmed[colon + 2..].trim_start().chars().next() else {
            continue;
        };
        assert!(
            first != '`' && first != '@',
            "line {}: a value may not begin with `{first}` — YAML rejects the document, not the \
             line:\n  {line}",
            n + 1
        );
    }
}

#[test]
fn the_workflow_runs_no_command_the_binary_does_not_have() {
    let known = subcommands();
    let invoked = ci::invocations(&workflow());
    assert!(
        !invoked.is_empty(),
        "no `beck` invocations found — this check would pass by looking at nothing"
    );
    for command in &invoked {
        assert!(
            known.contains(command),
            "the generated workflow runs `beck {command}`, which is not a subcommand. Known: \
             {known:?}"
        );
    }
}

#[test]
fn the_workflow_runs_the_steps_the_pipeline_is_specified_as() {
    // §28.3 item 1, by name. `--wire-compat` is called out there as the load-bearing one: it is
    // §4.3's firewall placed where it stops a bad deploy rather than reporting one.
    let text = workflow();
    for required in [
        "--locked",      // §3.4's stability guardrail
        "beck test",     // §21.2's tests, in the program
        "--wire-compat", // §4.3, before a deploy rather than after
        "beck build",    // the manifests and the bill of materials
        "beck image",    // §6.2's image, in one process
        "beck sign",     // §6.2's signature
    ] {
        assert!(
            text.contains(required),
            "the generated workflow lost {required}"
        );
    }
    // The image job must not run on a pull request: a signature made over unapproved code is a
    // signature that says the wrong thing.
    assert!(
        text.contains("if: github.ref == 'refs/heads/main'"),
        "the image job is not restricted to the default branch"
    );
}

#[test]
fn the_workflow_names_the_program_it_was_generated_for() {
    let text = ci::workflow(&graph(), "src/todo.beck");
    assert!(text.contains("BECK_APP: src/todo.beck"), "{text}");
    // And nothing else hard-codes a path: every step reads the variable, so one edit moves the
    // program.
    assert_eq!(
        text.matches("src/todo.beck").count(),
        1,
        "the program's path is written more than once"
    );
}

#[test]
fn the_generated_workflow_does_not_promise_an_action_that_does_not_exist() {
    // docs/28 §28.1: no release has been cut, so there is no `setup-beck` action to use. A
    // generator that emitted one would write a workflow that fails on its first run in every
    // repository that generated it — docs/19 §19.4 item 10's "an artefact nobody has executed",
    // shipped as a feature.
    let text = workflow();
    for line in text.lines() {
        let Some(action) = line.trim().strip_prefix("- uses: ") else {
            continue;
        };
        assert!(
            action.starts_with("actions/"),
            "the workflow uses {action}, which is not a first-party action this project can \
             vouch for"
        );
    }
}
