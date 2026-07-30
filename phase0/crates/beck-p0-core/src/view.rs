//! `view` and the queries feeding it — pure, unplaced, "runs anywhere".
//!
//! In Mode A (the v0.1 default, §5.1) the server evaluates this against the current accumulator,
//! diffs successive values, and streams patches; the same function produces the SSR first paint.
//! Phase 0 recomputes the view per event, which §5.3 states is semantically identical to the
//! incremental plan and fine at this scale — and which leaves recompute available as the exact
//! oracle when the differential-dataflow plans arrive in Phase 3.

use serde_json::json;

use crate::domain::{ActorId, Todo, TodoState};
use crate::html::Html;

/// Which slice of the fold a subscription sees.
///
/// `Mine` is §3.8's `todos.map(filter_by(session.user))` — the per-session view whose fanout cost
/// the Phase 0 exit criteria measure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Everyone,
    Mine(ActorId),
}

/// The visible todos, ordered as the sketch orders them: `(sort-by :text (vals todos))`.
pub fn visible<'a>(state: &'a TodoState, scope: &Scope) -> Vec<&'a Todo> {
    let mut todos: Vec<&Todo> = match scope {
        Scope::Everyone => state.todos.values().collect(),
        Scope::Mine(actor) => state.todos.values().filter(|t| &t.owner == actor).collect(),
    };
    // Ties broken by id so the order is total — replay must reproduce the patch stream exactly,
    // and a sort that leaves ties to the input order would only be deterministic by accident.
    todos.sort_by(|a, b| a.text.cmp(&b.text).then(a.id.cmp(&b.id)));
    todos
}

/// `(def remaining (map (fn [ts] (count (filter (fn [t] (not t.done)) (vals ts)))) todos))`
pub fn remaining(todos: &[&Todo]) -> usize {
    todos.iter().filter(|t| !t.done).count()
}

/// `(def view (fn [todos remaining] ...))` — verbatim from the sketch, in `Html` values.
pub fn view(todos: &[&Todo], remaining: usize) -> Html {
    Html::el("main")
        .child(Html::el("h1").child(Html::text("todos")))
        .child(
            Html::el("input")
                .attr("placeholder", "what needs doing?")
                .attr("autofocus", "")
                // `send!(Add {id: uuid!(), text})`: the client mints the id, because a browser
                // here is a replica, not a terminal. `$id` and `$value` are the only two holes the
                // thin client fills.
                .on("enter", json!({"c": "add", "id": "$id", "text": "$value"})),
        )
        .child(Html::el("ul").children(todos.iter().map(|t| row(t))))
        .child(Html::el("footer").child(Html::text(format!("{remaining} remaining"))))
}

fn row(t: &Todo) -> Html {
    let id = t.id.to_string();
    Html::el("li")
        .key(&id)
        .attr_if(t.done, "class", "done")
        .child(
            Html::el("span")
                .on("click", json!({"c": "toggle", "id": id}))
                .child(Html::text(&t.text)),
        )
        .child(
            Html::el("button")
                .on("click", json!({"c": "delete", "id": id}))
                .child(Html::text("×")),
        )
}

/// `(def page (map2 view todos remaining))` — the tier crossing, evaluated server-side in Mode A.
pub fn page(state: &TodoState, scope: &Scope) -> Html {
    let todos = visible(state, scope);
    let remaining = remaining(&todos);
    view(&todos, remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Id;

    fn state() -> TodoState {
        let mut s = TodoState::new();
        for (i, (text, owner, done)) in [
            ("beta", "alice", false),
            ("alpha", "bob", true),
            ("gamma", "alice", false),
        ]
        .into_iter()
        .enumerate()
        {
            s.todos.insert(
                Id::from_u128(i as u128 + 1),
                Todo {
                    id: Id::from_u128(i as u128 + 1),
                    text: text.into(),
                    done,
                    owner: ActorId::new(owner),
                },
            );
        }
        s
    }

    #[test]
    fn the_broadcast_view_sorts_by_text() {
        let s = state();
        let todos = visible(&s, &Scope::Everyone);
        assert_eq!(
            todos.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        assert_eq!(remaining(&todos), 2);
    }

    #[test]
    fn the_per_session_view_shows_only_this_actors_todos() {
        let s = state();
        let mine = visible(&s, &Scope::Mine(ActorId::new("alice")));
        assert_eq!(
            mine.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["beta", "gamma"]
        );
        assert_eq!(remaining(&mine), 2);
        // The filter provably runs server-side: the client is never sent the unfiltered state.
        let html = page(&s, &Scope::Mine(ActorId::new("alice"))).render();
        assert!(!html.contains("alpha"));
    }

    #[test]
    fn rendering_is_deterministic() {
        let s = state();
        assert_eq!(
            page(&s, &Scope::Everyone).render(),
            page(&s, &Scope::Everyone).render()
        );
    }

    #[test]
    fn handlers_are_declarative_attributes_and_no_script_is_emitted() {
        let html = page(&state(), &Scope::Everyone).render();
        assert!(html.contains("data-b-click="));
        assert!(html.contains("data-b-enter="));
        assert!(!html.contains("<script"));
        assert!(!html.contains("javascript:"));
    }
}
