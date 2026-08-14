//! A persistent ordered map — the language's `Map[K, V]`.
//!
//! # Why this exists
//!
//! [`docs/19-phase-1-report.md`](../../../../../docs/19-phase-1-report.md) §19.4 item 3: the fold was
//! `O(events × rows)` because `map_insert` cloned the whole accumulator. `Arc<BTreeMap>` makes
//! *cloning the handle* cheap and *updating* expensive, which is exactly backwards for a language
//! whose central construct is `state = fold(f, init, events)`.
//!
//! The Phase 1 report proposed uniqueness analysis — let the fold mutate in place when the previous
//! state is dead. That is worth having eventually, but it is the wrong *first* answer: it makes an
//! asymptotic guarantee depend on an optimisation firing. A persistent map gives `O(log n)` updates
//! unconditionally, with or without the analysis, on every backend. Uniqueness analysis then turns
//! `O(log n)` into `O(1)` amortised for the common case, which is a real but secondary win.
//!
//! # Why it is written here rather than depended on
//!
//! `im` and `rpds` are both MPL-2.0, which `deny.toml` does not allow. More to the point, a
//! persistent map is not a third-party concern for a functional language — it is `Map[K, V]`, a
//! type in the surface language, and its performance characteristics are part of the semantics.
//! `docs/01-vision-and-premise.md` §1.5's "we do not write a storage engine" is about substrates,
//! not about the standard library's own data structures.
//!
//! # Why *this* structure
//!
//! Three requirements fix the answer, and it is worth writing down which:
//!
//! 1. **Persistent.** `state = fold(f, init, events)` keeps old states reachable — snapshots hold
//!    them, `replay_to` rebuilds them, the differential harness diffs against them. So updating
//!    cannot mean mutating.
//! 2. **Ordered by key.** Iteration order reaches the rendered page and the state digest, and
//!    §4.8's replay harness compares both bit for bit.
//! 3. **Keys are arbitrary [`Value`](crate::Value)s** — not integers, so no Patricia trie; ordered
//!    by comparison, so no hash table.
//!
//! (2) and (3) together mean a comparison-based ordered dictionary, whose worst case is
//! `Ω(log n)` per operation on information-theoretic grounds. `O(log n)` is therefore optimal and
//! the only question left is *which* balanced search tree. Three are plausible:
//!
//! | scheme          | height     | `len` | used by                        |
//! |-----------------|------------|-------|--------------------------------|
//! | AVL             | ≤1.44 lg n | `O(n)`| OCaml `Map`                    |
//! | red-black       | ≤2 lg n    | `O(n)`| Scala, Java `TreeMap`          |
//! | weight-balanced | ≤2.4 lg n  | `O(1)`| Haskell `Data.Map`, SML/NJ     |
//!
//! Weight-balanced wins here for a language-specific reason: `map_len` is a *prim*, so a program
//! may call it inside a view that already runs once per event. Every node carries its subtree size,
//! so `len` is a field read rather than a traversal. The same sizes give `O(log n)` rank/select if
//! indexing is ever added, and admit the join-based `union`/`intersection`/`difference` of Blelloch,
//! Ferizovic and Sun (2016) at the optimal `O(m log(n/m + 1))` if those become prims.
//!
//! A HAMT (Bagwell 2001; CHAMP, Steindorfer and Vinju 2015) would be a constant factor faster —
//! depth ≤7 rather than ~2.4 lg n — but iterates in *hash* order, which violates (2); recovering
//! key order would cost a sort on every render, and the digest would then depend on a hash function
//! staying stable across compiler versions. It buys no asymptotic improvement, so it loses.
//!
//! # Cost
//!
//! | operation                    | time       | *fresh* nodes |
//! |------------------------------|------------|---------------|
//! | `get`, `contains_key`        | `O(log n)` | 0             |
//! | `insert`, `remove`           | `O(log n)` | `O(log n)`    |
//! | `len`, `is_empty`, `clone`   | `O(1)`     | 0             |
//! | `iter`, `keys`, `values`     | `O(n)`     | 0             |
//!
//! So a fold of `E` events over a map reaching `n` entries costs `O(E log n)` time, against the
//! `O(E · n)` — quadratic when every event adds a row — that copying cost. Live space is `O(n)`
//! nodes plus `O(log n)` per retained version: the path a superseded version rebuilt is freed by
//! its `Arc` the moment the old state is dropped.
//!
//! The price of sharing is per-entry overhead: a node is key + value + `usize` + two `Option<Arc>`
//! plus the `Arc` header, roughly 3–5× a `BTreeMap` entry, which packs ~11 entries to a cache line
//! group. That is the trade — and it is repaid immediately, because the old code allocated a whole
//! copy of the map on *every* event.
//!
//! The remaining constant-factor win is uniqueness: when the previous state is dead, the path could
//! be updated in place (`Arc::get_mut` — Clojure's transients) for `O(1)` allocation. That is a
//! real improvement and it is *not* implemented here, because an asymptotic guarantee should not
//! depend on an optimisation firing.

use std::cmp::Ordering;
use std::sync::Arc;

/// The rebalancing constants. `DELTA` bounds how lopsided a node may be; `RATIO` decides single
/// versus double rotation.
///
/// ⟨3, 2⟩ is not a free choice. Adams' "Efficient sets: a balancing act" (1993) published ⟨4, 2⟩,
/// and Hirai and Yamamoto ("Balancing weight-balanced trees", JFP 21(3), 2011) later proved by
/// exhaustive machine-checked search that ⟨4, 2⟩ does **not** preserve the invariant under delete —
/// a real bug that shipped in Haskell's `containers`. ⟨3, 2⟩ is one of the pairs they proved valid
/// for both insert and delete, which is why it is the pair here.
const DELTA: usize = 3;
const RATIO: usize = 2;

#[derive(Debug)]
struct Node<K, V> {
    key: K,
    value: V,
    size: usize,
    left: Option<Arc<Node<K, V>>>,
    right: Option<Arc<Node<K, V>>>,
}

type Link<K, V> = Option<Arc<Node<K, V>>>;

fn size<K, V>(n: &Link<K, V>) -> usize {
    n.as_ref().map_or(0, |n| n.size)
}

/// A persistent ordered map. Cloning is `O(1)` and shares everything.
#[derive(Debug)]
pub struct PMap<K, V> {
    root: Link<K, V>,
}

impl<K, V> Clone for PMap<K, V> {
    fn clone(&self) -> Self {
        PMap {
            root: self.root.clone(),
        }
    }
}

impl<K, V> Default for PMap<K, V> {
    fn default() -> Self {
        PMap { root: None }
    }
}

impl<K: Ord + Clone, V: Clone> PMap<K, V> {
    pub fn new() -> PMap<K, V> {
        PMap::default()
    }

    pub fn len(&self) -> usize {
        size(&self.root)
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let mut cur = self.root.as_ref();
        while let Some(n) = cur {
            cur = match key.cmp(&n.key) {
                Ordering::Less => n.left.as_ref(),
                Ordering::Greater => n.right.as_ref(),
                Ordering::Equal => return Some(&n.value),
            };
        }
        None
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Whether two maps are *the same tree*, not merely equal ones.
    ///
    /// `O(1)`, and the answer the incremental engine needs when deciding whether an event moved a
    /// map at all: structural equality would be `O(n)` per event, which is the cost the engine
    /// exists to avoid. A `false` here means "it may have changed", never "it did".
    pub fn same_root(&self, other: &PMap<K, V>) -> bool {
        match (&self.root, &other.root) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Insert, returning a new map. Shares every subtree the new key did not pass through.
    pub fn insert(&self, key: K, value: V) -> PMap<K, V> {
        PMap {
            root: insert_node(&self.root, key, value),
        }
    }

    /// Remove, returning a new map.
    pub fn remove(&self, key: &K) -> PMap<K, V> {
        PMap {
            root: remove_node(&self.root, key),
        }
    }

    /// Entries in key order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        let mut it = Iter { stack: Vec::new() };
        it.push_left(&self.root);
        it
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }
}

/// What happened to one key between two versions of a map.
///
/// `old` and `new` are both present for an update, one of them for an insert or a remove. Both
/// absent never occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change<K, V> {
    pub key: K,
    pub old: Option<V>,
    pub new: Option<V>,
}

impl<K: Ord + Clone, V: Clone + PartialEq> PMap<K, V> {
    /// The entries that differ between two versions, in key order.
    ///
    /// # Why this is `O(δ log n)` rather than `O(n)`
    ///
    /// This is the operation the whole incremental view engine rests on
    /// ([`docs/23-incremental-views-report.md`](../../../../../docs/23-incremental-views-report.md)):
    /// a fold produces a *whole new accumulator* per event, and a dataflow plan consumes *deltas*,
    /// so something has to turn one into the other. Comparing entry by entry would be `O(n)` per
    /// event, which is the recount §3.8 exists to abolish — the plan downstream would be
    /// incremental and the thing feeding it would not.
    ///
    /// [`insert`](PMap::insert) rebuilds only the path to the key and shares every subtree that
    /// path did not pass through, by `Arc`. So two versions of a map that differ by one insert
    /// share `n - O(log n)` nodes *by pointer*, and a diff that can recognise a shared subtree can
    /// skip all of its entries at once.
    ///
    /// The traversal is an ordered merge of the two trees, with one extra rule: when the heads of
    /// the two remaining sequences are the same subtree by pointer, both are dropped. That is sound
    /// for a reason worth stating, because it is the correctness of the engine: pointer-identical
    /// subtrees hold identical entries, so the two remaining *sorted sequences* share that prefix
    /// exactly, and a merge over sorted sequences reports nothing for a shared prefix. It holds
    /// whatever rebalancing did to the position of that subtree in either tree.
    pub fn diff(&self, next: &PMap<K, V>) -> Vec<Change<K, V>> {
        let mut out = Vec::new();
        let mut a = Walk::new(&self.root);
        let mut b = Walk::new(&next.root);
        loop {
            // The pointer rule, applied before either side is expanded into entries.
            a.skip_shared(&mut b);
            match (a.peek(), b.peek()) {
                (None, None) => return out,
                (Some((k, v)), None) => {
                    out.push(Change {
                        key: k.clone(),
                        old: Some(v.clone()),
                        new: None,
                    });
                    a.bump();
                }
                (None, Some((k, v))) => {
                    out.push(Change {
                        key: k.clone(),
                        old: None,
                        new: Some(v.clone()),
                    });
                    b.bump();
                }
                (Some((ka, va)), Some((kb, vb))) => match ka.cmp(kb) {
                    Ordering::Less => {
                        out.push(Change {
                            key: ka.clone(),
                            old: Some(va.clone()),
                            new: None,
                        });
                        a.bump();
                    }
                    Ordering::Greater => {
                        out.push(Change {
                            key: kb.clone(),
                            old: None,
                            new: Some(vb.clone()),
                        });
                        b.bump();
                    }
                    Ordering::Equal => {
                        if va != vb {
                            out.push(Change {
                                key: ka.clone(),
                                old: Some(va.clone()),
                                new: Some(vb.clone()),
                            });
                        }
                        a.bump();
                        b.bump();
                    }
                },
            }
        }
    }
}

/// An in-order traversal that can be asked whether its next *subtree* is one another traversal is
/// also about to yield.
///
/// The ordinary [`Iter`] pushes the left spine eagerly, which destroys exactly the information the
/// diff needs: once a subtree has been expanded into a stack of nodes, "these two are the same
/// subtree" is no longer a question that can be asked. This keeps unexpanded subtrees on the stack
/// and expands one only when the merge actually needs an entry from it.
struct Walk<'a, K, V> {
    stack: Vec<Task<'a, K, V>>,
}

enum Task<'a, K, V> {
    Sub(&'a Arc<Node<K, V>>),
    Ent(&'a K, &'a V),
}

impl<'a, K, V> Walk<'a, K, V> {
    fn new(root: &'a Link<K, V>) -> Walk<'a, K, V> {
        let mut stack = Vec::new();
        if let Some(n) = root {
            stack.push(Task::Sub(n));
        }
        Walk { stack }
    }

    /// Drop any subtree both traversals are about to yield.
    ///
    /// Repeated, because skipping one shared subtree can expose another underneath it — which is
    /// what happens on the second and later events, when the two versions share several whole
    /// branches rather than one.
    fn skip_shared(&mut self, other: &mut Walk<'a, K, V>) {
        loop {
            let (Some(Task::Sub(x)), Some(Task::Sub(y))) = (self.stack.last(), other.stack.last())
            else {
                return;
            };
            if !Arc::ptr_eq(*x, *y) {
                return;
            }
            self.stack.pop();
            other.stack.pop();
        }
    }

    /// The next entry, expanding subtrees as needed. Leaves it on the stack.
    fn peek(&mut self) -> Option<(&'a K, &'a V)> {
        loop {
            match self.stack.last()? {
                Task::Ent(k, v) => return Some((*k, *v)),
                Task::Sub(n) => {
                    let n = *n;
                    self.stack.pop();
                    // In-order: right subtree deepest, then this entry, then the left subtree.
                    if let Some(r) = &n.right {
                        self.stack.push(Task::Sub(r));
                    }
                    self.stack.push(Task::Ent(&n.key, &n.value));
                    if let Some(l) = &n.left {
                        self.stack.push(Task::Sub(l));
                    }
                }
            }
        }
    }

    fn bump(&mut self) {
        self.stack.pop();
    }
}

impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for PMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        iter.into_iter()
            .fold(PMap::new(), |m, (k, v)| m.insert(k, v))
    }
}

fn node<K, V>(key: K, value: V, left: Link<K, V>, right: Link<K, V>) -> Arc<Node<K, V>> {
    let size = 1 + size(&left) + size(&right);
    Arc::new(Node {
        key,
        value,
        size,
        left,
        right,
    })
}

/// Rebuild a node, restoring the weight balance invariant.
fn balance<K: Clone, V: Clone>(
    key: K,
    value: V,
    left: Link<K, V>,
    right: Link<K, V>,
) -> Arc<Node<K, V>> {
    let (ls, rs) = (size(&left), size(&right));
    if ls + rs <= 1 {
        return node(key, value, left, right);
    }
    if rs > DELTA * ls {
        let r = right.as_ref().expect("rs > 0");
        return if size(&r.left) < RATIO * size(&r.right) {
            // single left rotation
            node(
                r.key.clone(),
                r.value.clone(),
                Some(node(key, value, left, r.left.clone())),
                r.right.clone(),
            )
        } else {
            // double left rotation
            let rl = r.left.as_ref().expect("checked by the ratio test");
            node(
                rl.key.clone(),
                rl.value.clone(),
                Some(node(key, value, left, rl.left.clone())),
                Some(node(
                    r.key.clone(),
                    r.value.clone(),
                    rl.right.clone(),
                    r.right.clone(),
                )),
            )
        };
    }
    if ls > DELTA * rs {
        let l = left.as_ref().expect("ls > 0");
        return if size(&l.right) < RATIO * size(&l.left) {
            node(
                l.key.clone(),
                l.value.clone(),
                l.left.clone(),
                Some(node(key, value, l.right.clone(), right)),
            )
        } else {
            let lr = l.right.as_ref().expect("checked by the ratio test");
            node(
                lr.key.clone(),
                lr.value.clone(),
                Some(node(
                    l.key.clone(),
                    l.value.clone(),
                    l.left.clone(),
                    lr.left.clone(),
                )),
                Some(node(key, value, lr.right.clone(), right)),
            )
        };
    }
    node(key, value, left, right)
}

fn insert_node<K: Ord + Clone, V: Clone>(link: &Link<K, V>, key: K, value: V) -> Link<K, V> {
    match link {
        None => Some(node(key, value, None, None)),
        Some(n) => Some(match key.cmp(&n.key) {
            // Only the path to the key is rebuilt; `n.right`/`n.left` are shared by pointer.
            Ordering::Less => balance(
                n.key.clone(),
                n.value.clone(),
                insert_node(&n.left, key, value),
                n.right.clone(),
            ),
            Ordering::Greater => balance(
                n.key.clone(),
                n.value.clone(),
                n.left.clone(),
                insert_node(&n.right, key, value),
            ),
            Ordering::Equal => node(key, value, n.left.clone(), n.right.clone()),
        }),
    }
}

fn remove_node<K: Ord + Clone, V: Clone>(link: &Link<K, V>, key: &K) -> Link<K, V> {
    let n = link.as_ref()?;
    Some(match key.cmp(&n.key) {
        Ordering::Less => balance(
            n.key.clone(),
            n.value.clone(),
            remove_node(&n.left, key),
            n.right.clone(),
        ),
        Ordering::Greater => balance(
            n.key.clone(),
            n.value.clone(),
            n.left.clone(),
            remove_node(&n.right, key),
        ),
        Ordering::Equal => match (&n.left, &n.right) {
            (None, None) => return None,
            (None, Some(r)) => r.clone(),
            (Some(l), None) => l.clone(),
            (Some(_), Some(r)) => {
                // Replace with the successor, then remove it from the right subtree.
                let (sk, sv) = min_entry(r);
                balance(sk, sv, n.left.clone(), remove_min(&n.right))
            }
        },
    })
}

fn min_entry<K: Clone, V: Clone>(n: &Arc<Node<K, V>>) -> (K, V) {
    let mut cur = n;
    while let Some(l) = &cur.left {
        cur = l;
    }
    (cur.key.clone(), cur.value.clone())
}

fn remove_min<K: Clone, V: Clone>(link: &Link<K, V>) -> Link<K, V> {
    let n = link.as_ref()?;
    match &n.left {
        None => n.right.clone(),
        Some(_) => Some(balance(
            n.key.clone(),
            n.value.clone(),
            remove_min(&n.left),
            n.right.clone(),
        )),
    }
}

pub struct Iter<'a, K, V> {
    stack: Vec<&'a Arc<Node<K, V>>>,
}

impl<'a, K, V> Iter<'a, K, V> {
    fn push_left(&mut self, mut link: &'a Link<K, V>) {
        while let Some(n) = link {
            self.stack.push(n);
            link = &n.left;
        }
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let n = self.stack.pop()?;
        self.push_left(&n.right);
        Some((&n.key, &n.value))
    }
}

// ---- equality, ordering and hashing are structural, over the sorted entries ----

impl<K: Ord + Clone + PartialEq, V: Clone + PartialEq> PartialEq for PMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<K: Ord + Clone, V: Clone + Eq> Eq for PMap<K, V> {}

impl<K: Ord + Clone, V: Clone + Ord> PartialOrd for PMap<K, V> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord + Clone, V: Clone + Ord> Ord for PMap<K, V> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_and_ordered_iteration() {
        let mut m = PMap::new();
        for i in [5, 3, 8, 1, 4, 7, 9, 2, 6] {
            m = m.insert(i, i * 10);
        }
        assert_eq!(m.len(), 9);
        assert_eq!(m.get(&4), Some(&40));
        assert_eq!(m.get(&99), None);
        assert_eq!(
            m.keys().copied().collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>()
        );

        let without = m.remove(&5);
        assert_eq!(without.len(), 8);
        assert_eq!(without.get(&5), None);
        // …and the original is untouched, which is the whole point.
        assert_eq!(m.get(&5), Some(&50));
        assert_eq!(m.len(), 9);
    }

    #[test]
    fn replacing_a_key_does_not_grow_the_map() {
        let m = PMap::new().insert("a", 1).insert("a", 2);
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&"a"), Some(&2));
    }

    #[test]
    fn an_insert_shares_all_but_the_path_it_rebuilt() {
        // The property the fold's asymptotics rest on: inserting into a map of n entries allocates
        // O(log n) nodes, not n. Counted by pointer identity against the original.
        let mut base = PMap::new();
        for i in 0..1024 {
            base = base.insert(i, i);
        }
        let next = base.insert(9999, 9999);

        fn nodes<K, V>(link: &Link<K, V>, out: &mut Vec<*const Node<K, V>>) {
            if let Some(n) = link {
                out.push(Arc::as_ptr(n));
                nodes(&n.left, out);
                nodes(&n.right, out);
            }
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        nodes(&base.root, &mut a);
        nodes(&next.root, &mut b);
        let shared = b.iter().filter(|p| a.contains(p)).count();
        let fresh = b.len() - shared;

        assert_eq!(base.len(), 1024);
        assert_eq!(next.len(), 1025);
        assert!(
            fresh <= 40,
            "an insert into a 1024-entry map rebuilt {fresh} nodes; O(log n) is ~10-40 with \
             rebalancing, O(n) would be ~1024"
        );
        assert!(
            shared > 900,
            "only {shared} of {} nodes were shared",
            b.len()
        );
    }

    #[test]
    fn the_tree_stays_balanced_under_sorted_insertion() {
        // Ascending keys are the worst case for a naive BST and the common case for the fold,
        // because ids often arrive in order.
        let mut m = PMap::new();
        for i in 0..10_000 {
            m = m.insert(i, i);
        }
        fn depth<K, V>(link: &Link<K, V>) -> usize {
            match link {
                None => 0,
                Some(n) => 1 + depth(&n.left).max(depth(&n.right)),
            }
        }
        let d = depth(&m.root);
        assert_eq!(m.len(), 10_000);
        assert!(
            d < 40,
            "depth {d} for 10,000 ascending inserts is not balanced"
        );
    }

    #[test]
    fn removal_keeps_the_map_ordered_and_balanced() {
        let mut m: PMap<i32, i32> = (0..2_000).map(|i| (i, i)).collect();
        for i in (0..2_000).step_by(2) {
            m = m.remove(&i);
        }
        assert_eq!(m.len(), 1_000);
        let keys: Vec<i32> = m.keys().copied().collect();
        assert!(
            keys.windows(2).all(|w| w[0] < w[1]),
            "iteration is not ordered"
        );
        assert_eq!(keys[0], 1);
        assert!(m.get(&0).is_none() && m.get(&1).is_some());
    }

    /// Every node's subtree size is right, the keys are in order, and neither child outweighs the
    /// other by more than `DELTA` — the invariant `balance` exists to maintain. Returns the size so
    /// the check is one pass.
    fn check_invariant<K: Ord, V>(link: &Link<K, V>, lo: Option<&K>, hi: Option<&K>) -> usize {
        let Some(n) = link else { return 0 };
        if let Some(lo) = lo {
            assert!(&n.key > lo, "key order violated");
        }
        if let Some(hi) = hi {
            assert!(&n.key < hi, "key order violated");
        }
        let ls = check_invariant(&n.left, lo, Some(&n.key));
        let rs = check_invariant(&n.right, Some(&n.key), hi);
        assert_eq!(n.size, 1 + ls + rs, "a cached subtree size is stale");
        if ls + rs > 1 {
            assert!(
                ls <= DELTA * rs && rs <= DELTA * ls,
                "weight invariant violated: {ls} against {rs}"
            );
        }
        n.size
    }

    /// One pseudo-random history of inserts and removes, checked against `BTreeMap` as an oracle
    /// with the structural invariant verified after *every* operation.
    fn random_history(seed: u64, steps: u32, keyspace: u32) {
        use std::collections::BTreeMap;
        let mut s = seed | 1; // xorshift dies at zero
        let mut rand = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };

        let mut ours: PMap<u32, u32> = PMap::new();
        let mut oracle: BTreeMap<u32, u32> = BTreeMap::new();
        for step in 0..steps {
            let k = (rand() % keyspace as u64) as u32;
            // Insert-heavy, then delete-heavy, so the tree both grows and shrinks through every
            // rebalancing path rather than hovering at one size.
            let insert_pct = if step < steps / 2 { 70 } else { 30 };
            if rand() % 100 < insert_pct {
                ours = ours.insert(k, step);
                oracle.insert(k, step);
            } else {
                ours = ours.remove(&k);
                oracle.remove(&k);
            }
            check_invariant(&ours.root, None, None);
            assert_eq!(
                ours.len(),
                oracle.len(),
                "size diverged at step {step}, seed {seed}"
            );
        }
        assert!(
            ours.iter()
                .map(|(k, v)| (*k, *v))
                .eq(oracle.iter().map(|(k, v)| (*k, *v))),
            "entries diverged from the oracle, seed {seed}"
        );
    }

    #[test]
    fn random_histories_of_inserts_and_removes_match_a_btreemap() {
        // The class of bug this catches is the one `an_insert_shares_all_but_the_path_it_rebuilt`
        // cannot: a rotation that loses an entry or breaks the balance invariant only under some
        // particular interleaving of inserts and deletes. Seeds are fixed, so a failure is a
        // reproducible failure rather than a story about one; several of them, because a single
        // history explores one path through `balance` and there are six.
        //
        // This is the test that decides whether `DELTA`/`RATIO` are right. It is *not* strong
        // enough to have found the published ⟨4, 2⟩ counterexample on its own — see the note on
        // those constants: the parameters rest on Hirai and Yamamoto's proof, and this test
        // confirms the implementation maintains what they proved maintainable.
        for seed in 0..32u64 {
            random_history(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 3_000, 256);
        }
        // One much larger keyspace, so the tree is deep and mostly-distinct keys rather than a
        // small set churned repeatedly.
        random_history(0xDEAD_BEEF, 20_000, 8_192);
    }

    #[test]
    fn a_diff_reports_exactly_what_changed() {
        let base: PMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        assert!(base.diff(&base).is_empty());

        let inserted = base.insert(1000, 7);
        assert_eq!(
            base.diff(&inserted),
            vec![Change {
                key: 1000,
                old: None,
                new: Some(7)
            }]
        );
        assert_eq!(
            inserted.diff(&base),
            vec![Change {
                key: 1000,
                old: Some(7),
                new: None
            }]
        );

        let updated = base.insert(50, -1);
        assert_eq!(
            base.diff(&updated),
            vec![Change {
                key: 50,
                old: Some(50),
                new: Some(-1)
            }]
        );
        // Re-inserting the value it already has is not a change: the engine downstream must not be
        // told to redo work for an event that moved nothing.
        assert!(base.diff(&base.insert(50, 50)).is_empty());

        let removed = base.remove(&3);
        assert_eq!(
            base.diff(&removed),
            vec![Change {
                key: 3,
                old: Some(3),
                new: None
            }]
        );
    }

    #[test]
    fn a_diff_against_an_empty_map_is_every_entry() {
        let m: PMap<i32, i32> = (0..10).map(|i| (i, i * 2)).collect();
        let empty = PMap::new();
        let inserts = empty.diff(&m);
        assert_eq!(inserts.len(), 10);
        assert!(inserts.iter().all(|c| c.old.is_none()));
        // In key order, so a downstream operator can build an ordered arrangement from it without
        // sorting.
        assert!(inserts.windows(2).all(|w| w[0].key < w[1].key));
        assert_eq!(m.diff(&empty).len(), 10);
    }

    /// The property the incremental view engine's asymptotics rest on: diffing two versions that
    /// differ by one insert visits `O(log n)` nodes, not `n`.
    ///
    /// Counted rather than timed, because a wall-clock assertion in CI is a flake. The counter is
    /// the number of *entries* the merge had to look at, which is what an `O(n)` implementation
    /// would drive to `n`.
    #[test]
    fn diffing_two_versions_that_share_structure_visits_a_handful_of_entries() {
        let mut base: PMap<u32, u32> = PMap::new();
        for i in 0..8192 {
            base = base.insert(i, i);
        }
        let next = base.insert(4096, 999);

        // `Walk` yields entries; count how many either side had to expand.
        let mut visited = 0usize;
        let mut a = Walk::new(&base.root);
        let mut b = Walk::new(&next.root);
        loop {
            a.skip_shared(&mut b);
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some(_), None) => {
                    visited += 1;
                    a.bump();
                }
                (None, Some(_)) => {
                    visited += 1;
                    b.bump();
                }
                (Some((ka, _)), Some((kb, _))) => {
                    visited += 1;
                    match ka.cmp(kb) {
                        Ordering::Less => a.bump(),
                        Ordering::Greater => b.bump(),
                        Ordering::Equal => {
                            a.bump();
                            b.bump();
                        }
                    }
                }
            }
        }
        // Printed so that docs/24 §23.5's number is reproducible rather than remembered.
        println!("diffing an 8,192-entry map after one insert looked at {visited} entries");
        assert!(
            visited < 64,
            "diffing an 8192-entry map after one insert looked at {visited} entries; \
             O(log n) is a few dozen, O(n) would be 8192"
        );
        assert_eq!(base.diff(&next).len(), 1);
    }

    #[test]
    fn a_diff_matches_a_btreemap_oracle_over_random_histories() {
        use std::collections::BTreeMap;
        let mut s = 0x5EED_1234u64;
        let mut rand = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..64 {
            let mut ours: PMap<u32, u32> = PMap::new();
            let mut oracle: BTreeMap<u32, u32> = BTreeMap::new();
            for _ in 0..200 {
                let k = (rand() % 64) as u32;
                if rand() % 100 < 60 {
                    ours = ours.insert(k, (rand() % 8) as u32);
                    oracle.insert(k, *ours.get(&k).unwrap());
                } else {
                    ours = ours.remove(&k);
                    oracle.remove(&k);
                }
            }
            // A second history from the same start, so the two maps differ in many places at once
            // rather than by a single path.
            let mut other = ours.clone();
            let mut other_oracle = oracle.clone();
            for _ in 0..60 {
                let k = (rand() % 64) as u32;
                if rand() % 100 < 50 {
                    other = other.insert(k, (rand() % 8) as u32);
                    other_oracle.insert(k, *other.get(&k).unwrap());
                } else {
                    other = other.remove(&k);
                    other_oracle.remove(&k);
                }
            }

            let mut expected: Vec<Change<u32, u32>> = Vec::new();
            for k in oracle
                .keys()
                .chain(other_oracle.keys())
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
            {
                let (old, new) = (oracle.get(&k).copied(), other_oracle.get(&k).copied());
                if old != new {
                    expected.push(Change { key: k, old, new });
                }
            }
            assert_eq!(ours.diff(&other), expected);
        }
    }

    #[test]
    fn equality_and_ordering_are_structural() {
        let a: PMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        // Built in a different order: same map.
        let b: PMap<i32, i32> = (0..50).rev().map(|i| (i, i)).collect();
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert!(a < a.insert(50, 50));
    }
}
