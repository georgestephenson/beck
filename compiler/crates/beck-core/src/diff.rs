//! Structural diff of two `Html` values — the producer half of "the browser is
//! `fold(apply_patch, initial_html, patch_stream)`".
//!
//! Which side produces the ops is the mode: in Mode A the server diffs two renderings and streams
//! them; in Mode B the browser renders locally and diffs its own two renderings ([`crate::render`]).
//! It is the same function either way, which is why it lives here rather than in the runtime.
//!
//! Properties this implementation holds, because the whole Mode A story rests on them:
//!
//! * **Skipping.** Equal structural hashes ⇒ no ops, no descent. A patch is O(changed), not
//!   O(tree) (§5.1).
//! * **Keyed children.** Lists reorder by `Move`, not by rebuilding — which is also what preserves
//!   focus and scroll position in the browser (§5.1 "frame identity").
//! * **Sequential application.** Ops are emitted in the order the client must apply them; each
//!   index is valid against the DOM as it exists at that moment, which is why removals descend and
//!   insertions ascend.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::html::Html;

/// A node address: child indices from the root of the subscription's frame.
pub type Path = Vec<u32>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Replace the node at `path` wholesale (tag or key changed).
    Replace {
        path: Path,
        html: Html,
    },
    SetText {
        path: Path,
        text: String,
    },
    SetAttr {
        path: Path,
        name: String,
        value: String,
    },
    RemoveAttr {
        path: Path,
        name: String,
    },
    /// Insert `html` as child `index` of the element at `path`.
    Insert {
        path: Path,
        index: u32,
        html: Html,
    },
    Remove {
        path: Path,
        index: u32,
    },
    /// Move child `from` to `to` within the element at `path`; `from > to` always.
    Move {
        path: Path,
        from: u32,
        to: u32,
    },
}

impl Op {
    /// Wire encoding: a positional array whose head is the op tag.
    pub fn to_wire(&self) -> Value {
        match self {
            Op::Replace { path, html } => json!([0, path, html.to_wire()]),
            Op::SetText { path, text } => json!([1, path, text]),
            Op::SetAttr { path, name, value } => json!([2, path, name, value]),
            Op::RemoveAttr { path, name } => json!([3, path, name]),
            Op::Insert { path, index, html } => json!([4, path, index, html.to_wire()]),
            Op::Remove { path, index } => json!([5, path, index]),
            Op::Move { path, from, to } => json!([6, path, from, to]),
        }
    }
}

/// Diff two views of the same frame.
pub fn diff(old: &Html, new: &Html) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut path = Vec::new();
    diff_node(old, new, &mut path, &mut ops);
    ops
}

fn diff_node(old: &Html, new: &Html, path: &mut Path, ops: &mut Vec<Op>) {
    if old.hash() == new.hash() {
        return; // the subtree provably cannot have changed
    }
    match (old, new) {
        (Html::Text { .. }, Html::Text { text, .. }) => ops.push(Op::SetText {
            path: path.clone(),
            text: text.clone(),
        }),
        (
            Html::Element {
                tag: old_tag,
                key: old_key,
                attrs: old_attrs,
                children: old_children,
                ..
            },
            Html::Element {
                tag: new_tag,
                key: new_key,
                attrs: new_attrs,
                children: new_children,
                ..
            },
        ) if old_tag == new_tag && old_key == new_key => {
            diff_attrs(old_attrs, new_attrs, path, ops);
            diff_children(old_children, new_children, path, ops);
        }
        _ => ops.push(Op::Replace {
            path: path.clone(),
            html: new.clone(),
        }),
    }
}

fn diff_attrs(old: &[(String, String)], new: &[(String, String)], path: &Path, ops: &mut Vec<Op>) {
    for (name, value) in new {
        match old.iter().find(|(k, _)| k == name) {
            Some((_, old_value)) if old_value == value => {}
            _ => ops.push(Op::SetAttr {
                path: path.clone(),
                name: name.clone(),
                value: value.clone(),
            }),
        }
    }
    for (name, _) in old {
        if !new.iter().any(|(k, _)| k == name) {
            ops.push(Op::RemoveAttr {
                path: path.clone(),
                name: name.clone(),
            });
        }
    }
}

fn diff_children(old: &[Html], new: &[Html], path: &mut Path, ops: &mut Vec<Op>) {
    if keyed(old) && keyed(new) {
        diff_keyed(old, new, path, ops);
    } else {
        diff_positional(old, new, path, ops);
    }
}

/// Keyed iff every child carries a key and the keys are unique — otherwise the reconciliation
/// below would be ambiguous, and a positional diff is the honest fallback.
fn keyed(children: &[Html]) -> bool {
    if children.is_empty() {
        return false;
    }
    let mut seen = HashSet::with_capacity(children.len());
    children.iter().all(|c| match c.key_of() {
        Some(k) => seen.insert(k),
        None => false,
    })
}

fn diff_positional(old: &[Html], new: &[Html], path: &mut Path, ops: &mut Vec<Op>) {
    let common = old.len().min(new.len());
    for i in 0..common {
        path.push(i as u32);
        diff_node(&old[i], &new[i], path, ops);
        path.pop();
    }
    for i in (common..old.len()).rev() {
        ops.push(Op::Remove {
            path: path.clone(),
            index: i as u32,
        });
    }
    for (i, node) in new.iter().enumerate().skip(common) {
        ops.push(Op::Insert {
            path: path.clone(),
            index: i as u32,
            html: node.clone(),
        });
    }
}

fn diff_keyed(old: &[Html], new: &[Html], path: &mut Path, ops: &mut Vec<Op>) {
    let wanted: HashSet<&str> = new.iter().filter_map(Html::key_of).collect();

    // `cursor` mirrors the client's child list as the ops are applied, so every index emitted
    // below is the index the client will see at that point in the stream.
    let mut cursor: Vec<&Html> = old.iter().collect();

    let mut i = 0;
    while i < cursor.len() {
        if cursor[i].key_of().is_some_and(|k| wanted.contains(k)) {
            i += 1;
        } else {
            ops.push(Op::Remove {
                path: path.clone(),
                index: i as u32,
            });
            cursor.remove(i);
        }
    }

    for (j, want) in new.iter().enumerate() {
        let key = want.key_of();
        let found = cursor[j..].iter().position(|c| c.key_of() == key);
        match found {
            Some(0) => {}
            Some(offset) => {
                let from = (j + offset) as u32;
                ops.push(Op::Move {
                    path: path.clone(),
                    from,
                    to: j as u32,
                });
                let node = cursor.remove(j + offset);
                cursor.insert(j, node);
            }
            None => {
                ops.push(Op::Insert {
                    path: path.clone(),
                    index: j as u32,
                    html: want.clone(),
                });
                cursor.insert(j, want);
                continue; // freshly inserted: nothing to diff against
            }
        }
        path.push(j as u32);
        diff_node(cursor[j], want, path, ops);
        path.pop();
    }
}

/// Apply a patch to an `Html` value — the server-side model of what the browser does.
///
/// This exists so the differ can be tested against its own client: `apply(old, diff(old, new)) ==
/// new` is a property, checked below and re-checked by the end-to-end harness against the real
/// browser. It is the Phase 0 stand-in for §4.8's differential harness.
pub fn apply(root: &Html, ops: &[Op]) -> Html {
    let mut root = root.clone();
    for op in ops {
        match op {
            Op::Replace { path, html } => set_node(&mut root, path, html.clone()),
            Op::SetText { path, text } => set_node(&mut root, path, Html::text(text.clone())),
            Op::SetAttr { path, name, value } => {
                let target = node_mut(&mut root, path);
                *target = with_attr(target.clone(), name, Some(value));
            }
            Op::RemoveAttr { path, name } => {
                let target = node_mut(&mut root, path);
                *target = with_attr(target.clone(), name, None);
            }
            Op::Insert { path, index, html } => {
                let parent = node_mut(&mut root, path);
                *parent = rebuild(parent.clone(), |cs| {
                    cs.insert(*index as usize, html.clone());
                });
            }
            Op::Remove { path, index } => {
                let parent = node_mut(&mut root, path);
                *parent = rebuild(parent.clone(), |cs| {
                    cs.remove(*index as usize);
                });
            }
            Op::Move { path, from, to } => {
                let parent = node_mut(&mut root, path);
                *parent = rebuild(parent.clone(), |cs| {
                    let node = cs.remove(*from as usize);
                    cs.insert(*to as usize, node);
                });
            }
        }
    }
    // Every op invalidates the structural hash of the patched node's ancestors, so the tree is
    // rehashed once at the end rather than repaired op by op.
    root.rehash()
}

fn node_mut<'a>(root: &'a mut Html, path: &[u32]) -> &'a mut Html {
    let mut node = root;
    for step in path {
        node = match node {
            Html::Element { children, .. } => &mut children[*step as usize],
            Html::Text { .. } => panic!("patch path descends into a text node"),
        };
    }
    node
}

fn set_node(root: &mut Html, path: &[u32], value: Html) {
    *node_mut(root, path) = value;
}

/// Rebuild an element through the `Html` builder so the structural hash stays consistent.
fn rebuild(node: Html, f: impl FnOnce(&mut Vec<Html>)) -> Html {
    match node {
        Html::Element {
            tag,
            attrs,
            key,
            mut children,
            ..
        } => {
            f(&mut children);
            let mut el = Html::el(tag);
            for (k, v) in attrs {
                el = el.attr(k, v);
            }
            if let Some(k) = key {
                el = el.key(k);
            }
            el.children(children)
        }
        text => text,
    }
}

fn with_attr(node: Html, name: &str, value: Option<&str>) -> Html {
    match node {
        Html::Element {
            tag,
            attrs,
            key,
            children,
            ..
        } => {
            let mut el = Html::el(tag);
            let mut replaced = false;
            for (k, v) in attrs {
                if k == name {
                    replaced = true;
                    match value {
                        Some(new) => el = el.attr(k, new),
                        None => continue,
                    }
                } else {
                    el = el.attr(k, v);
                }
            }
            if !replaced {
                if let Some(new) = value {
                    el = el.attr(name, new);
                }
            }
            if let Some(k) = key {
                el = el.key(k);
            }
            el.children(children)
        }
        text => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn li(key: &str, text: &str, done: bool) -> Html {
        Html::el("li")
            .key(key)
            .attr_if(done, "class", "done")
            .child(Html::text(text))
    }

    fn list(items: Vec<Html>) -> Html {
        Html::el("main")
            .child(Html::el("h1").child(Html::text("todos")))
            .child(Html::el("ul").children(items))
    }

    #[test]
    fn identical_trees_produce_no_ops() {
        let a = list(vec![li("1", "a", false), li("2", "b", false)]);
        let b = list(vec![li("1", "a", false), li("2", "b", false)]);
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn a_toggle_touches_one_attribute_and_nothing_else() {
        let a = list(vec![li("1", "a", false), li("2", "b", false)]);
        let b = list(vec![li("1", "a", true), li("2", "b", false)]);
        let ops = diff(&a, &b);
        assert_eq!(
            ops,
            vec![Op::SetAttr {
                path: vec![1, 0],
                name: "class".into(),
                value: "done".into(),
            }]
        );
        assert_eq!(apply(&a, &ops), b);
    }

    #[test]
    fn reordering_moves_rather_than_rebuilds() {
        let a = list(vec![
            li("1", "a", false),
            li("2", "b", false),
            li("3", "c", false),
        ]);
        let b = list(vec![
            li("3", "c", false),
            li("1", "a", false),
            li("2", "b", false),
        ]);
        let ops = diff(&a, &b);
        assert_eq!(
            ops,
            vec![Op::Move {
                path: vec![1],
                from: 2,
                to: 0
            }]
        );
        assert_eq!(apply(&a, &ops), b);
    }

    #[test]
    fn insert_remove_and_edit_compose() {
        let a = list(vec![
            li("1", "a", false),
            li("2", "b", false),
            li("3", "c", false),
        ]);
        let b = list(vec![
            li("2", "b!", true),
            li("4", "d", false),
            li("1", "a", false),
        ]);
        let ops = diff(&a, &b);
        assert_eq!(apply(&a, &ops), b, "ops: {ops:#?}");
    }

    #[test]
    fn unkeyed_children_fall_back_to_positional() {
        let a = Html::el("footer").child(Html::text("2 remaining"));
        let b = Html::el("footer").child(Html::text("1 remaining"));
        let ops = diff(&a, &b);
        assert_eq!(
            ops,
            vec![Op::SetText {
                path: vec![0],
                text: "1 remaining".into()
            }]
        );
        assert_eq!(apply(&a, &ops), b);
    }

    #[test]
    fn tag_change_replaces() {
        let a = Html::el("main").child(Html::el("p").child(Html::text("x")));
        let b = Html::el("main").child(Html::el("div").child(Html::text("x")));
        let ops = diff(&a, &b);
        assert!(matches!(ops.as_slice(), [Op::Replace { .. }]));
        assert_eq!(apply(&a, &ops), b);
    }

    #[test]
    fn round_trips_over_a_long_random_walk() {
        // A cheap deterministic PRNG: this is a test, and reproducibility beats entropy.
        let mut seed = 0x243f_6a88_85a3_08d3u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        let mut items: Vec<(u32, String, bool)> = Vec::new();
        let mut next_key = 0u32;
        let mut current = list(vec![]);

        for _ in 0..400 {
            match rand() % 4 {
                0 => {
                    next_key += 1;
                    let at = if items.is_empty() {
                        0
                    } else {
                        (rand() as usize) % (items.len() + 1)
                    };
                    items.insert(at, (next_key, format!("todo {next_key}"), false));
                }
                1 if !items.is_empty() => {
                    let at = (rand() as usize) % items.len();
                    items.remove(at);
                }
                2 if !items.is_empty() => {
                    let at = (rand() as usize) % items.len();
                    items[at].2 = !items[at].2;
                }
                3 if items.len() > 1 => {
                    let from = (rand() as usize) % items.len();
                    let to = (rand() as usize) % items.len();
                    let item = items.remove(from);
                    items.insert(to, item);
                }
                _ => {}
            }

            let next = list(
                items
                    .iter()
                    .map(|(k, t, d)| li(&k.to_string(), t, *d))
                    .collect(),
            );
            let ops = diff(&current, &next);
            assert_eq!(apply(&current, &ops), next, "ops: {ops:#?}");
            current = next;
        }
    }
}
