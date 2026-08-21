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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

fn diff_children(old: &[Arc<Html>], new: &[Arc<Html>], path: &mut Path, ops: &mut Vec<Op>) {
    let (head, tail) = shared_ends(old, new);
    if both_keyed(old, new, head, tail) {
        diff_keyed_from(
            &old[head..old.len() - tail],
            &new[head..new.len() - tail],
            head as u32,
            path,
            ops,
        );
    } else {
        // The full lists, deliberately. Trimming here would make a shared page and a copied one
        // patch differently — `Remove` and `Insert` are index-based, so dropping a shared suffix
        // moves the indices the positional path emits. `diff::tests::
        // a_shared_page_and_a_copied_one_produce_the_same_ops` is the property that forbids it.
        diff_positional(old, new, path, ops);
    }
}

/// How many children the two lists hold as the *same allocation* at each end.
///
/// An untouched child is literally the node it was, so a run of them needs no ops and no
/// examination. This is a fact about how the page was assembled rather than about what it
/// contains, which is why nothing downstream may let it change the ops it emits.
fn shared_ends(old: &[Arc<Html>], new: &[Arc<Html>]) -> (usize, usize) {
    let mut head = 0;
    while head < old.len() && head < new.len() && Arc::ptr_eq(&old[head], &new[head]) {
        head += 1;
    }
    let mut tail = 0;
    while head + tail < old.len()
        && head + tail < new.len()
        && Arc::ptr_eq(&old[old.len() - 1 - tail], &new[new.len() - 1 - tail])
    {
        tail += 1;
    }
    (head, tail)
}

/// `keyed(old) && keyed(new)`, computed once over what the two lists share instead of twice.
///
/// Whether a list reconciles by key is a question about the **whole** list — a key repeated
/// anywhere makes the reconciliation ambiguous — so this cannot be narrowed to the window the way
/// the reconciliation itself is. But the children at the shared ends are the same allocations in
/// both lists and therefore carry the same keys, so hashing them once answers for both: only the
/// windows differ, and only the windows need hashing twice.
///
/// That is worth having because this check, not the reconciliation, is what one event pays. On a
/// page where a single row changed, the trim leaves a window of one and the two full-list passes
/// were **62% of the diff at 1,000 rows, 87% at 5,000 and 89% at 8,000**.
///
/// The answer is unchanged, and that is the point: this is the same predicate, so no op moves.
fn both_keyed(old: &[Arc<Html>], new: &[Arc<Html>], head: usize, tail: usize) -> bool {
    // `keyed` was false for an empty list, and an empty list on either side still means the
    // positional path.
    if old.is_empty() || new.is_empty() {
        return false;
    }
    let mut seen: HashSet<&str> = HashSet::with_capacity(old.len());
    // The shared ends, hashed once. Their keys are the same in both lists by construction.
    let shared = old[..head]
        .iter()
        .chain(&old[old.len() - tail..])
        .map(|c| c.key_of());
    for key in shared {
        match key {
            Some(k) if seen.insert(k) => {}
            _ => return false,
        }
    }
    // Each window against those, and against itself. The first window is taken back out so the
    // second is measured against the shared ends alone.
    let old_window = &old[head..old.len() - tail];
    let new_window = &new[head..new.len() - tail];
    for window in [old_window, new_window] {
        for child in window {
            match child.key_of() {
                Some(k) if seen.insert(k) => {}
                _ => return false,
            }
        }
        for child in window {
            seen.remove(child.key_of().expect("just inserted"));
        }
    }
    true
}

fn diff_positional(old: &[Arc<Html>], new: &[Arc<Html>], path: &mut Path, ops: &mut Vec<Op>) {
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
            html: (**node).clone(),
        });
    }
}

/// The reconciliation itself, over a window of the two child lists.
///
/// `base` is where that window starts in the client's children, so every index this emits is the
/// index the client will see — the same contract the whole module keeps ("each index is valid
/// against the DOM as it exists at that moment").
fn diff_keyed_from(
    old: &[Arc<Html>],
    new: &[Arc<Html>],
    base: u32,
    path: &mut Path,
    ops: &mut Vec<Op>,
) {
    let wanted: HashSet<&str> = new.iter().filter_map(|c| c.key_of()).collect();

    // Drop the children the new list has no key for, in one pass. A child that survives lands at
    // the index `kept.len()` had when it was reached, because every drop before it has already
    // been applied — so the index each `Remove` carries is the one the client will see, without
    // the list ever being shifted to find it.
    let mut kept: Vec<&Html> = Vec::with_capacity(old.len().min(new.len()));
    for c in old {
        if c.key_of().is_some_and(|k| wanted.contains(k)) {
            kept.push(c);
        } else {
            ops.push(Op::Remove {
                path: path.clone(),
                index: base + kept.len() as u32,
            });
        }
    }

    // Where each surviving child started, and it is what turns "find this key ahead of `j`" into
    // a lookup. A key is taken out as it is claimed, so a list that repeats one — which `keyed`
    // rules out before reconciliation is reached, but which this function should not depend on —
    // treats the second occurrence as new, exactly as searching the remaining children did.
    let mut origin: HashMap<&str, usize> = HashMap::with_capacity(kept.len());
    for (p, c) in kept.iter().enumerate() {
        if let Some(k) = c.key_of() {
            origin.insert(k, p);
        }
    }

    let mut unclaimed = Unclaimed::new(kept.len());
    for (j, want) in new.iter().enumerate() {
        let node = match want.key_of().and_then(|k| origin.remove(k)) {
            Some(p) => {
                // A `Move` lifts a child out and puts it back at `j`, so the children nobody has
                // claimed yet keep their relative order: the one at `p` currently sits exactly as
                // far past `j` as there are unclaimed children before it.
                let offset = unclaimed.ahead_of(p);
                if offset > 0 {
                    ops.push(Op::Move {
                        path: path.clone(),
                        from: base + (j + offset) as u32,
                        to: base + j as u32,
                    });
                }
                unclaimed.claim(p);
                kept[p]
            }
            None => {
                ops.push(Op::Insert {
                    path: path.clone(),
                    index: base + j as u32,
                    html: (**want).clone(),
                });
                continue; // freshly inserted: nothing to diff against
            }
        };
        path.push(base + j as u32);
        diff_node(node, want, path, ops);
        path.pop();
    }
}

/// How many of the client's children, ahead of a given one, the new list has not claimed yet.
///
/// A Fenwick tree over the surviving children's starting positions, holding one for each child
/// still unclaimed. Phase two needs one number per child — the distance it currently sits ahead of
/// where it belongs — and reading that off the child list is a scan, which made reconciliation
/// quadratic in the window: reordering 4,000 keyed rows cost 25 ms of diffing, and doubling the
/// rows quadrupled it. As a rank query it is `O(log w)`, so the pass is `O(w log w)`.
///
/// The op stream is unchanged. This computes the same offsets the scan did.
struct Unclaimed {
    /// One-based, as Fenwick trees are: `tree[0]` is unused so that `i & i.wrapping_neg()`
    /// terminates.
    tree: Vec<u32>,
}

impl Unclaimed {
    /// All `n` children start unclaimed, built in `O(n)` by carrying each cell into its parent
    /// rather than by `n` separate updates.
    fn new(n: usize) -> Self {
        let mut tree = vec![0u32; n + 1];
        for i in 1..=n {
            tree[i] += 1;
            let parent = i + (i & i.wrapping_neg());
            if parent <= n {
                let carried = tree[i];
                tree[parent] += carried;
            }
        }
        Self { tree }
    }

    /// The number of still-unclaimed children whose starting position is before `p`.
    fn ahead_of(&self, p: usize) -> usize {
        let mut i = p;
        let mut sum = 0u32;
        while i > 0 {
            sum += self.tree[i];
            i -= i & i.wrapping_neg();
        }
        sum as usize
    }

    /// Mark the child that started at `p` as claimed, so it stops counting towards later offsets.
    fn claim(&mut self, p: usize) {
        let mut i = p + 1;
        while i < self.tree.len() {
            self.tree[i] -= 1;
            i += i & i.wrapping_neg();
        }
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
                    cs.insert(*index as usize, Arc::new(html.clone()));
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
            // `make_mut` and not an index: children are shared with whatever other tree holds
            // them, so descending to patch one has to unshare exactly the spine it walks. Nodes
            // off the path keep their allocation and their refcount.
            Html::Element { children, .. } => Arc::make_mut(&mut children[*step as usize]),
            Html::Text { .. } => panic!("patch path descends into a text node"),
        };
    }
    node
}

fn set_node(root: &mut Html, path: &[u32], value: Html) {
    *node_mut(root, path) = value;
}

/// Rebuild an element through the `Html` builder so the structural hash stays consistent.
fn rebuild(node: Html, f: impl FnOnce(&mut Vec<Arc<Html>>)) -> Html {
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
            el.children_shared(children)
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
            el.children_shared(children)
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

    /// **The same two pages, shared and copied, produce the same ops.**
    ///
    /// [`diff_keyed`] skips the runs of children the two lists hold as the *same allocation*, which
    /// is a fact about how the page was assembled and not about what it contains. So the risk the
    /// trim carries is precisely that it changes the answer — an index computed against a window
    /// rather than the whole list, a move whose source is counted from the wrong place.
    ///
    /// `rehash` rebuilds a tree node for node, so the copy shares nothing and takes the full
    /// reconciliation while the original takes the trimmed one. Every scenario below runs both and
    /// requires the two op streams to be **equal**, and requires each to carry the client from the
    /// old page to the new one. The unit tests above cannot do this job: they build every node
    /// fresh, so nothing in them is ever shared and the trim never runs.
    #[test]
    fn a_shared_page_and_a_copied_one_produce_the_same_ops() {
        fn shared(items: &[Arc<Html>]) -> Html {
            let mut el = Html::el("ul");
            for i in items {
                el = el.child_shared(i.clone());
            }
            el
        }
        let pool: Vec<Arc<Html>> = (0..24)
            .map(|i| Arc::new(li(&format!("k{i}"), &format!("item {i}"), i % 3 == 0)))
            .collect();

        // A deterministic walk over the shapes a page actually takes: prepend (the sketch's own),
        // append, remove from either end and the middle, reorder, and an edit in place.
        let mut seed = 0x5eedu64;
        let mut rand = move |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n.max(1)
        };
        let mut exercised = 0usize;
        for case in 0..200 {
            let take = 4 + rand(16);
            let old_rows: Vec<Arc<Html>> = pool.iter().take(take).cloned().collect();
            let mut new_rows = old_rows.clone();
            match case % 6 {
                0 => new_rows.insert(0, pool[23].clone()),
                1 => new_rows.push(pool[23].clone()),
                2 => {
                    if !new_rows.is_empty() {
                        new_rows.remove(rand(new_rows.len()));
                    }
                }
                3 => new_rows.reverse(),
                4 => {
                    if new_rows.len() > 2 {
                        let n = new_rows.len();
                        new_rows.swap(1, n - 1);
                    }
                }
                _ => {
                    // An edit in place: a *different* node under a key that stays, which is what
                    // makes the trim stop rather than run to the end.
                    let at = rand(new_rows.len());
                    new_rows[at] = Arc::new(li(&format!("k{at}"), "edited", true));
                }
            }
            let (old, new) = (shared(&old_rows), shared(&new_rows));
            // The same two pages with nothing shared: `rehash` rebuilds every node.
            let (old_copy, new_copy) = (old.rehash(), new.rehash());
            assert_eq!(old, old_copy, "rehash must preserve the value, case {case}");

            let trimmed = diff(&old, &new);
            let full = diff(&old_copy, &new_copy);
            assert_eq!(
                trimmed, full,
                "case {case}: trimming the shared ends changed the ops"
            );
            assert_eq!(
                apply(&old, &trimmed),
                new.rehash(),
                "case {case}: round trip"
            );
            if old_rows
                .first()
                .zip(new_rows.first())
                .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
                || old_rows
                    .last()
                    .zip(new_rows.last())
                    .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            {
                exercised += 1;
            }
        }
        // The control: if nothing had a shared end, the two sides above would be the same code
        // path and this test would prove nothing.
        assert!(
            exercised > 100,
            "only {exercised} of 200 cases shared an end, so the trim was barely exercised"
        );
    }

    /// **Whether a list reconciles by key is a question about the whole list, including the part
    /// the two pages share.**
    ///
    /// [`both_keyed`] hashes the shared ends once instead of twice, which is only sound because it
    /// still asks about every child. Narrowing the question to the window — which is what the
    /// reconciliation itself is narrowed to, and so the tempting next step — would answer
    /// differently here: the window below is cleanly keyed while the list holding it is not.
    ///
    /// Nothing else in this file can tell those two apart. Every other case builds children whose
    /// keys are distinct, so a predicate that skipped the shared ends entirely passed all fifteen
    /// of them; this is the one that goes red, which is the whole reason it exists
    /// (`docs/82` §82.10).
    #[test]
    fn a_repeated_key_in_the_part_two_pages_share_forces_the_positional_path() {
        // The duplicate sits in the prefix the two pages hold as the same allocations, and the
        // window between them is a clean two-key reorder.
        let dup_a = Arc::new(li("dup", "first", false));
        let dup_b = Arc::new(li("dup", "second", false));
        let x = Arc::new(li("x", "x", false));
        let y = Arc::new(li("y", "y", false));
        let end = Arc::new(li("end", "end", false));

        let old = Html::el("ul").children_shared([
            Arc::clone(&dup_a),
            Arc::clone(&dup_b),
            Arc::clone(&x),
            Arc::clone(&y),
            Arc::clone(&end),
        ]);
        let new = Html::el("ul").children_shared([
            Arc::clone(&dup_a),
            Arc::clone(&dup_b),
            Arc::clone(&y),
            Arc::clone(&x),
            Arc::clone(&end),
        ]);

        let ops = diff(&old, &new);
        assert!(
            !ops.iter().any(|o| matches!(o, Op::Move { .. })),
            "`dup` is carried by two children, so this list cannot reconcile by key and the \
             positional path is the honest answer — but the ops moved something: {ops:?}"
        );
        assert_eq!(
            apply(&old, &ops),
            new.rehash(),
            "and it still has to round trip"
        );

        // The same two pages with the duplicate resolved *do* reconcile by key, so the assertion
        // above is about the repeat and not about the shape of the test.
        let un_dup = Arc::new(li("dup2", "second", false));
        let keyed_old = Html::el("ul").children_shared([
            Arc::clone(&dup_a),
            Arc::clone(&un_dup),
            Arc::clone(&x),
            Arc::clone(&y),
            Arc::clone(&end),
        ]);
        let keyed_new = Html::el("ul").children_shared([
            Arc::clone(&dup_a),
            Arc::clone(&un_dup),
            Arc::clone(&y),
            Arc::clone(&x),
            Arc::clone(&end),
        ]);
        let keyed_ops = diff(&keyed_old, &keyed_new);
        assert!(
            keyed_ops.iter().any(|o| matches!(o, Op::Move { .. })),
            "with distinct keys the same reorder should move rather than rebuild: {keyed_ops:?}"
        );
        assert_eq!(apply(&keyed_old, &keyed_ops), keyed_new.rehash());
    }

    /// `diff_keyed_from` as it was before the rank structure: a scan of the child list for every
    /// child. Kept as the oracle for the thing that replaced it.
    fn scan_keyed_from(
        old: &[Arc<Html>],
        new: &[Arc<Html>],
        base: u32,
        path: &mut Path,
        ops: &mut Vec<Op>,
    ) {
        let wanted: HashSet<&str> = new.iter().filter_map(|c| c.key_of()).collect();
        let mut cursor: Vec<&Html> = old.iter().map(|c| &**c).collect();
        let mut i = 0;
        while i < cursor.len() {
            if cursor[i].key_of().is_some_and(|k| wanted.contains(k)) {
                i += 1;
            } else {
                ops.push(Op::Remove {
                    path: path.clone(),
                    index: base + i as u32,
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
                    ops.push(Op::Move {
                        path: path.clone(),
                        from: base + (j + offset) as u32,
                        to: base + j as u32,
                    });
                    let node = cursor.remove(j + offset);
                    cursor.insert(j, node);
                }
                None => {
                    ops.push(Op::Insert {
                        path: path.clone(),
                        index: base + j as u32,
                        html: (**want).clone(),
                    });
                    cursor.insert(j, want);
                    continue;
                }
            }
            path.push(base + j as u32);
            diff_node(cursor[j], want, path, ops);
            path.pop();
        }
    }

    /// **The rank structure emits the same ops as the scan it replaced.**
    ///
    /// Reconciliation's output is a contract with a client that has already applied everything
    /// before it, so replacing the scan had to be a faster route to the *same stream* rather than
    /// merely to the same page. Round-tripping cannot see that difference — many distinct streams
    /// land on the same tree, so a differ that emitted `n` redundant moves would still round-trip.
    /// The scan is therefore kept above as the oracle and asserted against directly.
    ///
    /// The cases are built to reach all four outcomes — a child already in place, one that has to
    /// move, one that is new, one that is dropped — and the run asserts it saw each, because a
    /// generator that quietly stopped producing moves would leave this green while testing
    /// nothing.
    #[test]
    fn the_rank_structure_and_the_scan_it_replaced_emit_the_same_ops() {
        let mut seed = 0xd1ffu64;
        let mut rand = move |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n.max(1)
        };
        let (mut moved, mut inserted, mut removed, mut in_place) = (0usize, 0, 0, 0);

        for case in 0..300 {
            let n = rand(24);
            let old: Vec<Arc<Html>> = (0..n)
                .map(|i| Arc::new(li(&format!("k{i}"), &format!("row {i}"), false)))
                .collect();

            // Keep a random subset, permute it by repeated random rotation, edit some of the
            // survivors' content, and splice in children with keys the old list never had.
            let mut kept: Vec<Arc<Html>> = Vec::new();
            for c in &old {
                if rand(4) == 0 {
                    continue;
                }
                if rand(3) == 0 {
                    let key = c.key_of().expect("a keyed child").to_string();
                    let done = rand(2) == 0;
                    kept.push(Arc::new(li(&key, "edited", done)));
                } else {
                    kept.push(Arc::clone(c));
                }
            }
            for _ in 0..rand(6) {
                if !kept.is_empty() {
                    let from = rand(kept.len());
                    let to = rand(kept.len());
                    let node = kept.remove(from);
                    kept.insert(to, node);
                }
            }
            for fresh in 0..rand(4) {
                let at = rand(kept.len() + 1);
                kept.insert(at, Arc::new(li(&format!("fresh{fresh}"), "new", false)));
            }
            let new = kept;

            let mut fast = Vec::new();
            let mut scan = Vec::new();
            diff_keyed_from(&old, &new, 0, &mut Path::default(), &mut fast);
            scan_keyed_from(&old, &new, 0, &mut Path::default(), &mut scan);
            assert_eq!(
                fast,
                scan,
                "case {case}: the rank structure diverged from the scan\n  old {:?}\n  new {:?}",
                old.iter().map(|c| c.key_of()).collect::<Vec<_>>(),
                new.iter().map(|c| c.key_of()).collect::<Vec<_>>(),
            );

            for op in &fast {
                match op {
                    Op::Move { .. } => moved += 1,
                    Op::Insert { .. } => inserted += 1,
                    Op::Remove { .. } => removed += 1,
                    _ => {}
                }
            }
            in_place += new.len().saturating_sub(
                fast.iter()
                    .filter(|o| matches!(o, Op::Move { .. } | Op::Insert { .. }))
                    .count(),
            );
        }
        assert!(
            moved > 100 && inserted > 100 && removed > 100 && in_place > 100,
            "the generator stopped covering an outcome: {moved} moves, {inserted} inserts, \
             {removed} removes, {in_place} left in place"
        );
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
