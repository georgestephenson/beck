//! A small, deterministic JSON → YAML writer.
//!
//! The compiler builds a typed object graph and applies it server-side; YAML exists only because
//! §6.3 requires it — "teams with GitOps (Argo CD/Flux) must be able to commit it, and refusing
//! them is refusing half the market". So this is an *output format*, never an input, and it is
//! deliberately 80 lines rather than a dependency: the manifests are golden files in CI, so the
//! writer must be byte-stable across versions of everything.

use serde_json::Value;

/// Render one object as a YAML document (no leading `---`).
pub fn to_yaml(value: &Value) -> String {
    let mut out = String::with_capacity(4096);
    write_value(value, 0, &mut out, false);
    out
}

/// Render several objects as a multi-document YAML file.
pub fn documents(values: &[Value]) -> String {
    let mut out = String::new();
    for value in values {
        out.push_str("---\n");
        out.push_str(&to_yaml(value));
    }
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
                    // A nested map under a sequence entry shares the dash's line.
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
        scalar => {
            if inline {
                out.push(' ');
            }
            out.push_str(&scalar_to_yaml(scalar));
            out.push('\n');
        }
    }
}

fn pad(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn escape_key(key: &str) -> String {
    if needs_quotes(key) {
        quote(key)
    } else {
        key.to_string()
    }
}

fn scalar_to_yaml(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) if needs_quotes(s) => quote(s),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Quote anything that a YAML parser could read as something other than a plain string. The list
/// is conservative on purpose: a manifest that round-trips wrong is a production incident.
fn needs_quotes(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.parse::<f64>().is_ok() {
        return true;
    }
    const RESERVED: &[&str] = &[
        "true", "false", "null", "yes", "no", "on", "off", "y", "n", "~",
    ];
    if RESERVED.contains(&s.to_ascii_lowercase().as_str()) {
        return true;
    }
    if s.starts_with(' ') || s.ends_with(' ') {
        return true;
    }
    s.chars().next().is_some_and(|c| {
        matches!(
            c,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
    }) || s.contains(": ")
        || s.contains(" #")
        || s.contains('\n')
        || s.contains('\t')
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_nested_maps_and_sequences() {
        let value = json!({
            "apiVersion": "apps/v1",
            "metadata": {"name": "beck-todo", "labels": {"app": "beck-todo"}},
            "spec": {
                "containers": [
                    {"name": "app", "args": ["run", "--store", "postgres"], "ports": [{"containerPort": 8080}]}
                ],
                "replicas": 2
            }
        });
        // Keys come out sorted (serde_json's default map is ordered), which is what makes the
        // emitted manifests golden-file stable. `--store` is quoted because a leading dash would
        // otherwise start a sequence.
        assert_eq!(
            to_yaml(&value),
            concat!(
                "apiVersion: apps/v1\n",
                "metadata:\n",
                "  labels:\n",
                "    app: beck-todo\n",
                "  name: beck-todo\n",
                "spec:\n",
                "  containers:\n",
                "    - args:\n",
                "        - run\n",
                "        - \"--store\"\n",
                "        - postgres\n",
                "      name: app\n",
                "      ports:\n",
                "        - containerPort: 8080\n",
                "  replicas: 2\n",
            )
        );
    }

    #[test]
    fn quotes_what_yaml_would_otherwise_reinterpret() {
        let value =
            json!({"a": "yes", "b": "1.0", "c": "plain", "d": "", "e": "*star", "f": "8080"});
        assert_eq!(
            to_yaml(&value),
            "a: \"yes\"\nb: \"1.0\"\nc: plain\nd: \"\"\ne: \"*star\"\nf: \"8080\"\n"
        );
    }
}
