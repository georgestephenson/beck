//! Who is connected now — the roster `presence()` reads.
//!
//! [`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D6: "**Presence** (who is connected
//! now) ships v1 as a first-class non-durable `Signal` — it is both the natural demo of per-session
//! fanout and its permanent stress test."
//!
//! The compiler's half is a source in the signal graph
//! ([`beck_core::signal::Op::Presence`]); this is the fact that source reads. It is deliberately
//! small, and everything interesting about it is a consequence of one sentence: **this is the only
//! input to a view that moves without an event**. A session is not in the log either, and it is
//! fixed for the life of a subscription; the accumulator moves only when the log does.
//!
//! # What that sentence forbids
//!
//! Nothing here is appended, snapshotted or replayed. A process that restarts comes back with an
//! empty roster and fills it as clients reconnect, which is correct rather than lossy: who is
//! connected to a process that no longer exists is nobody. The checker keeps this from mattering
//! anywhere it would — `presence` cannot reach the chokepoint (`B0515`), so no event's existence
//! ever depended on it.
//!
//! # The bound, and why it is here rather than in a later hardening pass
//!
//! The obvious implementation is a map from actor to a count, and that map is **unbounded memory
//! keyed by a string the client chooses** — which is
//! [`docs/82`](../../../../../docs/82-the-edge-report.md) §82.5's finding
//! exactly, one subsystem over. Under [`crate::identity::DevIdentity`] the actor is whatever the
//! connection said it was, so a client opening sockets under fresh names would grow this table
//! until the process died.
//!
//! [`crate::quota`] answers the same problem by sharding into a fixed table, and that answer is not
//! available here: a quota needs a *number* per actor and may share buckets, while a roster needs
//! the actor's **name** and would be nonsense if two names collided. So the bound is a capacity:
//! past [`Config::capacity`] distinct actors, a new one is **not recorded** and
//! [`Registry::refused`] counts it. Presence then under-reports rather than growing, which is the
//! failure this direction should have — a page that says "127 here" when 200 are connected is
//! wrong in a way that costs nothing, and the opposite is a process that dies.
//!
//! An actor already in the roster is never refused, whatever the capacity: the bound is on how many
//! *names* are held, not on how many connections one of them may open.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use beck_core::Value;
use tokio::sync::watch;

/// How large a roster this process will hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// How many distinct actors may be in the roster at once.
    pub capacity: usize,
}

impl Default for Config {
    /// 4,096 actors.
    ///
    /// The number is chosen the way [`crate::quota::Quota::default`]'s is: large enough that no
    /// legitimate deployment of a single process meets it — a Beck process serving more than four
    /// thousand *distinct* identities at one instant has a fanout problem before it has a roster
    /// problem — and small enough that the table is a few hundred kilobytes at worst.
    fn default() -> Config {
        Config { capacity: 4096 }
    }
}

/// The connection set of one application.
pub struct Registry {
    /// The counts, and the published value derived from them.
    ///
    /// One mutex rather than a lock-free map: it is taken twice per *connection* — once on join and
    /// once on leave — and never on a render or an event. The published value is rebuilt under it
    /// so that what a subscriber reads is always a roster that existed.
    inner: Mutex<BTreeMap<Arc<str>, u32>>,
    value: watch::Sender<Value>,
    config: Config,
    refused: AtomicU64,
}

impl Registry {
    pub fn new(config: Config) -> Arc<Registry> {
        Arc::new(Registry {
            inner: Mutex::new(BTreeMap::new()),
            value: watch::channel(beck_core::edge::presence([])).0,
            config,
            refused: AtomicU64::new(0),
        })
    }

    /// The roster as a Beck value: `Map[Str, Int]`, actor to connections.
    pub fn value(&self) -> Value {
        self.value.borrow().clone()
    }

    /// Wake on every change to it. A subscription watches this **only** when the program's page
    /// reads `presence` — a program that never asks who is connected must not re-render when
    /// somebody connects.
    pub fn watch(&self) -> watch::Receiver<Value> {
        self.value.subscribe()
    }

    /// How many actors are in the roster.
    pub fn here(&self) -> usize {
        self.inner.lock().expect("presence").len()
    }

    /// How many joins the capacity refused, for the life of this process.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Record a connection. The roster holds it until the returned guard is dropped.
    ///
    /// A guard rather than a pair of calls, for the reason every other guard in this crate is one:
    /// a subscription ends by returning, by erroring or by its socket dying, and a roster that only
    /// removed an actor on the happy path would fill up with people who left hours ago.
    pub fn join(self: &Arc<Self>, actor: &str) -> Guard {
        let held = {
            let mut counts = self.inner.lock().expect("presence");
            let room = counts.len() < self.config.capacity;
            match counts.get_mut(actor) {
                Some(n) => {
                    *n += 1;
                    true
                }
                None if room => {
                    counts.insert(Arc::from(actor), 1);
                    true
                }
                // The bound. Counted rather than logged per occurrence: whoever is doing this is
                // doing it at a rate that would make a log line the denial of service.
                None => {
                    self.refused.fetch_add(1, Ordering::Relaxed);
                    false
                }
            }
        };
        if held {
            self.publish();
        }
        Guard {
            registry: held.then(|| self.clone()),
            actor: Arc::from(actor),
        }
    }

    fn leave(&self, actor: &str) {
        {
            let mut counts = self.inner.lock().expect("presence");
            match counts.get_mut(actor) {
                Some(n) if *n > 1 => *n -= 1,
                Some(_) => {
                    counts.remove(actor);
                }
                None => return,
            }
        }
        self.publish();
    }

    /// Rebuild the published value from the counts.
    ///
    /// `O(actors)` per connection change, which is the shape this trades for: a roster is read on
    /// every render of every subscriber and written once per connection, so the cost belongs on the
    /// write. What a reader gets is one `Arc` bump.
    fn publish(&self) {
        let counts = self.inner.lock().expect("presence");
        let value = beck_core::edge::presence(
            counts
                .iter()
                .map(|(actor, n)| (actor.as_ref(), i64::from(*n))),
        );
        // `send_replace` and not `send`: `send` fails when there is no receiver, and — the part
        // that matters — leaves the value it was given *unpublished*. Nothing may subscribe to a
        // roster until a program that reads one has a connection, so every join before the first
        // subscription would have been lost, including the first client's own.
        self.value.send_replace(value);
    }
}

/// One connection's membership of the roster.
pub struct Guard {
    /// `None` when the capacity refused this join: the guard still exists, so the caller has one
    /// code path, and dropping it removes nothing because nothing was added.
    registry: Option<Arc<Registry>>,
    actor: Arc<str>,
}

impl Guard {
    /// Whether this connection is in the roster. False only when the capacity refused it.
    pub fn recorded(&self) -> bool {
        self.registry.is_some()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(registry) = &self.registry {
            registry.leave(&self.actor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster(r: &Registry) -> Vec<(String, i64)> {
        let value = r.value();
        let map = value.as_map().expect("a map");
        map.iter()
            .map(|(k, v)| {
                (
                    k.as_str().expect("actor").to_string(),
                    v.as_int().expect("count"),
                )
            })
            .collect()
    }

    #[test]
    fn a_connection_joins_and_its_guard_removes_it() {
        let r = Registry::new(Config::default());
        assert_eq!(roster(&r), Vec::new());
        {
            let _ana = r.join("ana");
            assert_eq!(roster(&r), vec![("ana".to_string(), 1)]);
            let _bo = r.join("bo");
            assert_eq!(
                roster(&r),
                vec![("ana".to_string(), 1), ("bo".to_string(), 1)]
            );
        }
        assert_eq!(roster(&r), Vec::new());
    }

    #[test]
    fn one_actor_with_two_tabs_is_one_row_counted_twice() {
        let r = Registry::new(Config::default());
        let first = r.join("ana");
        let second = r.join("ana");
        assert_eq!(roster(&r), vec![("ana".to_string(), 2)]);
        drop(second);
        assert_eq!(roster(&r), vec![("ana".to_string(), 1)]);
        drop(first);
        assert_eq!(roster(&r), Vec::new());
    }

    /// §82.5's finding, one subsystem over: the table is keyed by a string the client chooses, so
    /// what stops it is a capacity rather than a hope.
    #[test]
    fn the_capacity_refuses_rather_than_growing() {
        let r = Registry::new(Config { capacity: 2 });
        let _a = r.join("a");
        let _b = r.join("b");
        let c = r.join("c");
        assert!(!c.recorded(), "the third actor is not in the roster");
        assert_eq!(r.here(), 2);
        assert_eq!(r.refused(), 1);
        // An actor already held is never refused, however full the table is.
        let again = r.join("a");
        assert!(again.recorded());
        assert_eq!(roster(&r), vec![("a".to_string(), 2), ("b".to_string(), 1)]);
        // And dropping the refused guard removes nothing.
        drop(c);
        assert_eq!(r.here(), 2);
    }

    #[test]
    fn a_watcher_wakes_on_a_join_and_on_a_leave() {
        let r = Registry::new(Config::default());
        let mut w = r.watch();
        assert!(!w.has_changed().expect("live"));
        let ana = r.join("ana");
        assert!(w.has_changed().expect("live"));
        w.mark_unchanged();
        drop(ana);
        assert!(w.has_changed().expect("live"));
    }
}
