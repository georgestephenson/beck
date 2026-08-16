//! What everybody is doing now — the roster `awareness(f)` reads.
//!
//! [`crate::presence`] holds who is connected; this holds what each of them contributes. The two
//! are separate registries because they move at different rates and for different reasons: a
//! connection joins and leaves, and between those two moments a client may change its
//! contribution any number of times by navigating.
//!
//! The compiler's half is a source in the signal graph ([`beck_core::signal::Op::Awareness`]) and a
//! role beside the view ([`beck_core::split::Roles::awareness`]). The role is a function
//! `Session -> T`; **this** is what applies it, once per connection per change, because the
//! subscribers are the runtime's fact and not the graph's — the signal graph of one program has no
//! way to name another connection's session.
//!
//! # Everything here follows from presence's one sentence
//!
//! Nothing is appended, snapshotted or replayed; a process that restarts comes back empty; the
//! checker keeps `awareness` away from the chokepoint (`B0515`) so no event's existence depended on
//! it. The bound is [`crate::presence`]'s bound for [`crate::presence`]'s reason — the table is
//! keyed by a string the client may choose ([`docs/82`](../../../../../docs/82-the-edge-report.md)
//! §82.5) — and past [`Config::capacity`] distinct actors a new one is **not recorded**, so the
//! roster under-reports rather than growing.
//!
//! # What is different: a value, and therefore a size
//!
//! A roster of counts is bounded by its capacity alone. A roster of *values* is bounded by the
//! capacity times whatever `f` returns, and `f` is the program's — a session's path is a few
//! dozen bytes, and nothing in the type system says it has to be. [`Config::each`] is the second
//! bound this needs and presence does not: a contribution whose rendered size exceeds it is
//! **refused**, the actor keeps whatever it contributed before, and [`Registry::oversized`] counts
//! it. Refusing one client's update is the failure this direction should have, and holding an
//! unbounded value per connection is not.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use beck_core::Value;
use tokio::sync::watch;

/// How large a roster this process will hold, and how large one contribution may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// How many distinct actors may be in the roster at once.
    pub capacity: usize,
    /// How many bytes one actor's contribution may render to.
    pub each: usize,
}

impl Default for Config {
    /// 4,096 actors of 4 KiB each — sixteen megabytes at worst, and that is the number to read this
    /// as, because the product is what the process pays.
    ///
    /// The capacity is [`crate::presence::Config::default`]'s and for its reason. The per-actor
    /// bound is a cursor, a selection or a route with room to spare, and far short of a document.
    fn default() -> Config {
        Config {
            capacity: 4096,
            each: 4096,
        }
    }
}

/// The awareness roster of one application.
pub struct Registry {
    /// Each actor's contribution, and how many connections that actor has.
    ///
    /// The count is here for the same reason presence keeps one: an actor with two tabs leaves the
    /// roster when the second closes, not the first. The *value* is whichever of that actor's
    /// connections published last, which is the only answer available when the roster is keyed by
    /// actor and a person may open two tabs.
    inner: Mutex<BTreeMap<Arc<str>, Entry>>,
    value: watch::Sender<Value>,
    config: Config,
    refused: AtomicU64,
    oversized: AtomicU64,
}

struct Entry {
    connections: u32,
    contribution: Value,
}

impl Registry {
    pub fn new(config: Config) -> Arc<Registry> {
        Arc::new(Registry {
            inner: Mutex::new(BTreeMap::new()),
            value: watch::channel(beck_core::edge::no_awareness()).0,
            config,
            refused: AtomicU64::new(0),
            oversized: AtomicU64::new(0),
        })
    }

    /// The roster as a Beck value: `Map[Str, T]`, actor to contribution.
    pub fn value(&self) -> Value {
        self.value.borrow().clone()
    }

    /// Wake on every change to it. A subscription watches this **only** when the program's page
    /// reads `awareness` — a program that never asks must not re-render when somebody navigates.
    pub fn watch(&self) -> watch::Receiver<Value> {
        self.value.subscribe()
    }

    /// How many actors are in the roster.
    pub fn here(&self) -> usize {
        self.inner.lock().expect("awareness").len()
    }

    /// How many joins the capacity refused, for the life of this process.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// How many contributions the per-actor bound refused, for the life of this process.
    pub fn oversized(&self) -> u64 {
        self.oversized.load(Ordering::Relaxed)
    }

    /// Record a connection's first contribution. The roster holds it until the guard is dropped.
    ///
    /// A guard for [`crate::presence::Guard`]'s reason: a subscription ends by returning, by
    /// erroring or by its socket dying, and a roster that only removed an actor on the happy path
    /// would fill up with people who left hours ago.
    pub fn join(self: &Arc<Self>, actor: &str, contribution: Value) -> Guard {
        let held = {
            let mut rows = self.inner.lock().expect("awareness");
            let room = rows.len() < self.config.capacity;
            match rows.get_mut(actor) {
                Some(entry) => {
                    entry.connections += 1;
                    true
                }
                None if room => {
                    rows.insert(
                        Arc::from(actor),
                        Entry {
                            connections: 1,
                            contribution: Value::Unit,
                        },
                    );
                    true
                }
                None => {
                    self.refused.fetch_add(1, Ordering::Relaxed);
                    false
                }
            }
        };
        let guard = Guard {
            registry: held.then(|| self.clone()),
            actor: Arc::from(actor),
        };
        if held {
            // Through `update` rather than written above, so the size bound applies to the first
            // contribution as it does to every later one. A joining connection whose contribution
            // is too large is in the roster with nothing in it, and says so.
            self.update(actor, contribution);
        }
        guard
    }

    /// Publish a new contribution for an actor already in the roster.
    ///
    /// Silently does nothing for an actor that is not — the capacity refused them, and a client
    /// navigating should not be a way to get in through a different door.
    ///
    /// Returns whether the roster changed, which is what saves a re-render: the common navigation
    /// republishes the same route.
    pub fn update(&self, actor: &str, contribution: Value) -> bool {
        let changed = {
            let mut rows = self.inner.lock().expect("awareness");
            let Some(entry) = rows.get_mut(actor) else {
                return false;
            };
            // The bound, measured on what the value renders to rather than on a shallow field
            // count: a list of a million empty strings is one field and eight megabytes.
            if contribution.display().len() > self.config.each {
                self.oversized.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            // Structural equality, not the engine's conservative pointer test: a client that
            // navigates to the route it is already on rebuilds an equal value, and answering
            // "changed" there would wake every subscriber for nothing.
            let changed = entry.contribution != contribution;
            entry.contribution = contribution;
            changed
        };
        if changed {
            self.publish();
        }
        changed
    }

    fn leave(&self, actor: &str) {
        {
            let mut rows = self.inner.lock().expect("awareness");
            match rows.get_mut(actor) {
                Some(entry) if entry.connections > 1 => {
                    entry.connections -= 1;
                    // The value stays: the actor is still here through another tab. Which of that
                    // actor's tabs it came from is not something this roster distinguishes.
                    return;
                }
                Some(_) => {
                    rows.remove(actor);
                }
                None => return,
            }
        }
        self.publish();
    }

    /// Rebuild the published value from the rows.
    ///
    /// `O(actors)` per change, and unlike presence's the change here is per *navigation* rather
    /// than per connection. That is the cost this trades knowingly: a roster is read on every
    /// render of every subscriber, so rebuilding it once per change beats rebuilding it once per
    /// reader. If a program ever moves a cursor through here, this is the line that becomes a
    /// delta stream rather than a rebuild.
    fn publish(&self) {
        let rows = self.inner.lock().expect("awareness");
        let value = beck_core::edge::awareness(
            rows.iter()
                .map(|(actor, entry)| (actor.as_ref(), entry.contribution.clone())),
        );
        // `send_replace` for [`crate::presence::Registry::publish`]'s reason: `send` fails when
        // there is no receiver and leaves the value unpublished, so every join before the first
        // subscription would be lost — including the first client's own.
        self.value.send_replace(value);
    }
}

/// One connection's membership of the awareness roster.
pub struct Guard {
    /// `None` when the capacity refused this join.
    registry: Option<Arc<Registry>>,
    actor: Arc<str>,
}

impl Guard {
    /// Whether this connection is in the roster. False only when the capacity refused it.
    pub fn recorded(&self) -> bool {
        self.registry.is_some()
    }

    /// Publish this connection's new contribution. Returns whether the roster changed.
    pub fn publish(&self, contribution: Value) -> bool {
        match &self.registry {
            Some(registry) => registry.update(&self.actor, contribution),
            None => false,
        }
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

    fn text(s: &str) -> Value {
        Value::str_(s)
    }

    fn roster(r: &Registry) -> Vec<(String, String)> {
        let value = r.value();
        let map = value.as_map().expect("a map");
        map.iter()
            .map(|(k, v)| {
                (
                    k.as_str().expect("actor").to_string(),
                    v.as_str().expect("contribution").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_connection_contributes_and_its_guard_removes_it() {
        let r = Registry::new(Config::default());
        assert_eq!(roster(&r), Vec::new());
        {
            let _ana = r.join("ana", text("/todos"));
            assert_eq!(roster(&r), vec![("ana".into(), "/todos".to_string())]);
            let _bo = r.join("bo", text("/done"));
            assert_eq!(
                roster(&r),
                vec![
                    ("ana".into(), "/todos".to_string()),
                    ("bo".into(), "/done".to_string())
                ]
            );
        }
        assert_eq!(roster(&r), Vec::new());
    }

    #[test]
    fn navigating_republishes_and_says_whether_anything_moved() {
        let r = Registry::new(Config::default());
        let ana = r.join("ana", text("/todos"));
        assert!(!ana.publish(text("/todos")), "the same route is no change");
        assert!(ana.publish(text("/done")));
        assert_eq!(roster(&r), vec![("ana".into(), "/done".to_string())]);
    }

    #[test]
    fn one_actor_with_two_tabs_leaves_when_the_second_closes() {
        let r = Registry::new(Config::default());
        let first = r.join("ana", text("/todos"));
        let second = r.join("ana", text("/done"));
        assert_eq!(roster(&r), vec![("ana".into(), "/done".to_string())]);
        drop(second);
        assert_eq!(
            roster(&r),
            vec![("ana".into(), "/done".to_string())],
            "still here through the other tab"
        );
        drop(first);
        assert_eq!(roster(&r), Vec::new());
    }

    /// §82.5's finding, keyed by a string the client chooses.
    #[test]
    fn the_capacity_refuses_rather_than_growing() {
        let r = Registry::new(Config {
            capacity: 2,
            each: 4096,
        });
        let _a = r.join("a", text("/a"));
        let _b = r.join("b", text("/b"));
        let c = r.join("c", text("/c"));
        assert!(!c.recorded());
        assert_eq!(r.here(), 2);
        assert_eq!(r.refused(), 1);
        // And a refused actor cannot get in by publishing.
        assert!(!c.publish(text("/c2")));
        assert_eq!(r.here(), 2);
        drop(c);
        assert_eq!(r.here(), 2);
    }

    /// The bound presence does not need: a roster of values is the capacity times the value.
    #[test]
    fn a_contribution_past_the_size_bound_is_refused_and_the_last_one_stands() {
        let r = Registry::new(Config {
            capacity: 8,
            each: 16,
        });
        let ana = r.join("ana", text("/todos"));
        assert_eq!(roster(&r), vec![("ana".into(), "/todos".to_string())]);
        assert!(!ana.publish(text(&"x".repeat(64))));
        assert_eq!(
            roster(&r),
            vec![("ana".into(), "/todos".to_string())],
            "the actor keeps what it last contributed"
        );
        assert_eq!(r.oversized(), 1);
        // A joining connection whose first contribution is too large is in the roster, empty.
        let _bo = r.join("bo", text(&"y".repeat(64)));
        assert_eq!(r.oversized(), 2);
        assert_eq!(r.here(), 2);
    }

    #[test]
    fn a_watcher_wakes_on_a_join_a_change_and_a_leave() {
        let r = Registry::new(Config::default());
        let mut w = r.watch();
        assert!(!w.has_changed().expect("live"));
        let ana = r.join("ana", text("/todos"));
        assert!(w.has_changed().expect("live"));
        w.mark_unchanged();
        ana.publish(text("/done"));
        assert!(w.has_changed().expect("live"));
        w.mark_unchanged();
        drop(ana);
        assert!(w.has_changed().expect("live"));
    }
}
