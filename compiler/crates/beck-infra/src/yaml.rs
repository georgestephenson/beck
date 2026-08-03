//! A small, deterministic JSON → YAML writer.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.4: "No YAML text, no
//! `kubectl` shelling. The compiler builds a typed `InfraGraph`." YAML exists downstream of that
//! only because [`06`](../../../../../docs/06-kubernetes-and-packaging.md) §6.3 requires it — "teams
//! with GitOps (Argo CD/Flux) must be able to commit it, and refusing them is refusing half the
//! market". So this is an **output format, never an input**.
//!
//! # Why not a YAML library
//!
//! Three reasons, and the first is the one that decides it:
//!
//! 1. **The manifests are golden files.** `tests/manifests.rs` snapshots every emitted object, and
//!    a deploy diff that changes because a dependency changed its quoting heuristics is a diff
//!    nobody can review. Byte-stability is the requirement, and it is easier to guarantee in sixty
//!    lines than to hope for across versions of somebody else's.
//! 2. `serde_yaml` has been archived since 2024. Taking an unmaintained crate into the compiler's
//!    *output path* is a different decision from taking one into a test.
//! 3. There is nothing to parse. The input is always `serde_json::Value` produced from a typed
//!    Kubernetes object, so the scalar cases are exactly four.
//!
//! Phase 0 reached the same conclusion and wrote the same eighty lines
//! ([`phase0/crates/beck-p0-operator/src/yaml.rs`](../../../../phase0/crates/beck-p0-operator/src/yaml.rs)).
//! That file is history and is not edited to track the compiler, so this is a second implementation
//! rather than a shared one — and `tests/manifests.rs` reads every document back with a real YAML
//! parser, so the writer does not mark its own homework.

use serde_json::Value;

/// Render one object as a YAML document (no leading `---`).
pub fn to_yaml(value: &Value) -> String {
    let mut out = String::with_capacity(2048);
    write_value(value, 0, &mut out, false);
    out
}

fn write_value(value: &Value, indent: usize, out: &mut String, inline: bool) {
    match value {
        Value::Object(map) if map.is_empty() => out.push_str(if inline { " {}\n" } else { "{}\n" }),
        Value::Object(map) => {
            if inline {
                out.push('\n');
            }
            for (key, child) in map {
                pad(indent, out);
                out.push_str(&escape_key(key));
                out.push(':');
                write_value(child, indent + 1, out, true);
            }
        }
        Value::Array(items) if items.is_empty() => {
            out.push_str(if inline { " []\n" } else { "[]\n" })
        }
        Value::Array(items) => {
            if inline {
                out.push('\n');
            }
            for item in items {
                pad(indent, out);
                out.push('-');
                match item {
                    // A nested map under a sequence entry shares the dash's line, which is what
                    // every hand-written manifest looks like and what a reviewer expects to see.
                    Value::Object(map) if !map.is_empty() => {
                        let mut first = true;
                        for (key, child) in map {
                            if first {
                                out.push(' ');
                                first = false;
                            } else {
                                pad(indent + 1, out);
                            }
                            out.push_str(&escape_key(key));
                            out.push(':');
                            write_value(child, indent + 2, out, true);
                        }
                    }
                    other => write_value(other, indent + 1, out, true),
                }
            }
        }
        Value::String(s) => {
            if let Some(block) = as_block(s) {
                out.push_str(" |\n");
                for line in block.lines() {
                    pad(indent, out);
                    out.push_str(line);
                    out.push('\n');
                }
            } else {
                out.push(' ');
                out.push_str(&quote(s));
                out.push('\n');
            }
        }
        Value::Number(n) => {
            out.push(' ');
            out.push_str(&n.to_string());
            out.push('\n');
        }
        Value::Bool(b) => {
            out.push(' ');
            out.push_str(if *b { "true" } else { "false" });
            out.push('\n');
        }
        Value::Null => out.push_str(" null\n"),
    }
}

/// A multi-line string is written as a literal block, because a quoted `\n` in a manifest is
/// unreadable and the one place it occurs — the grants SQL — is meant to be read.
fn as_block(s: &str) -> Option<&str> {
    (s.contains('\n') && s.ends_with('\n') && !s.contains('\r')).then_some(s)
}

fn pad(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn escape_key(key: &str) -> String {
    if key.is_empty() || key.chars().any(|c| !plain_key_char(c)) {
        quote(key)
    } else {
        key.to_string()
    }
}

fn plain_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')
}

/// Always quote. A YAML scalar that *looks* like something else is the classic manifest bug — the
/// Norway problem (`no` → `false`), a version that reads as a float, a port that reads as a
/// sexagesimal. Quoting unconditionally costs a little noise and removes the whole class.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Read back with somebody else's parser, so "it round-trips" is a claim about YAML rather than
    /// about this file's own idea of it.
    fn reread(v: &Value) -> Value {
        serde_norway::from_str(&to_yaml(v)).expect("the writer emits YAML")
    }

    #[test]
    fn every_shape_the_manifests_use_round_trips() {
        let v = json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "x", "labels": {"app.kubernetes.io/name": "x"}},
            "spec": {
                "replicas": 1,
                "enabled": true,
                "ports": [{"port": 8080, "targetPort": 8080}],
                "accessModes": ["ReadWriteOnce"],
                "empty_map": {},
                "empty_list": [],
                "nothing": null,
            },
        });
        assert_eq!(reread(&v), v);
    }

    #[test]
    fn a_scalar_that_looks_like_something_else_survives_as_a_string() {
        // The Norway problem, and its relatives. Every one of these is a real manifest bug.
        let v = json!({
            "country": "no",
            "on": "yes",
            "version": "1.10",
            "port": "22:22",
            "empty": "",
            "quoted": "he said \"hi\"",
        });
        assert_eq!(reread(&v), v, "{}", to_yaml(&v));
    }

    #[test]
    fn a_multi_line_string_is_a_readable_block() {
        let v = json!({"data": {"grants.sql": "-- a comment\nGRANT SELECT ON t TO \"r\";\n"}});
        let text = to_yaml(&v);
        assert!(text.contains("grants.sql: |\n"), "{text}");
        assert_eq!(reread(&v), v, "{text}");
    }

    #[test]
    fn the_writer_is_byte_stable() {
        // Golden files are only reviewable if the same input always produces the same bytes.
        let v = json!({"b": 1, "a": [{"z": "1", "y": "2"}]});
        assert_eq!(to_yaml(&v), to_yaml(&v));
        // Keys are sorted, because `serde_json::Map` is a `BTreeMap`: the same object always
        // writes the same bytes whatever order it was built in.
        assert_eq!(to_yaml(&v), "a:\n  - y: \"2\"\n    z: \"1\"\nb: 1\n");
    }
}
