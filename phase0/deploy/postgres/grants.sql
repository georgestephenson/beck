-- Effect-derived database grants (§6.5).
--
-- The privilege set is not a policy someone wrote down; it is what the program's effects imply.
-- The application appends to the log and reads it back. It never updates and never deletes,
-- because "no generic UPDATE/DELETE exists anywhere, because state changes are events by
-- construction". Snapshots are the one thing it writes twice, and even those are insert-only.
--
-- Applied by the deploy; regenerate by re-deriving from the effect row, never by editing.

create role beck_app login;

-- The log: append and read. Nothing else — not even by the owner of the application.
grant insert, select on table beck_log to beck_app;
revoke update, delete, truncate on table beck_log from beck_app;

-- Snapshots: insert-only, because a snapshot is a fact about a position in the log, and a fact
-- that changed was never a fact.
grant insert, select on table beck_snapshot to beck_app;
revoke update, delete, truncate on table beck_snapshot from beck_app;

-- Read models (Phase 3) would be granted ALL to the service that owns them and SELECT to the
-- pgwire-exposed reporting role. There are none in Phase 0: the view is recomputed from the fold,
-- not read from a table.

-- The migration role, used only by the pre-upgrade Job that runs `migrate`/`upcast` (§6.3). It is
-- separate precisely so that the running application cannot perform a migration by accident.
create role beck_migrator login;
grant insert, select on table beck_log to beck_migrator;
grant insert, select on table beck_snapshot to beck_migrator;

-- What is deliberately absent: CREATE on the schema for beck_app (DDL belongs to the deploy), any
-- privilege on other applications' tables, and any superuser grant.
