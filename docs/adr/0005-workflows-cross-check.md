# 0005 — Workflows cross-check each other's YAML validity

**Context.** docs/20 §20.4 item 8: the Phase 1 workflow was invalid YAML from the day it was
written, so every gate in it was silently absent. The in-workspace guard (`workflows.rs`) cannot
catch a recurrence: it only runs via `compiler.yml`, which GitHub refuses whole if that file is
the broken one, and which did not trigger on changes to other workflow files.

**Decision.** Every workflow triggers on `.github/workflows/**` and carries a job that parses
*all* workflow files with a real YAML parser. Whichever file is broken, another file's copy of
the job reports it.

**Consequences.** A workflow edit runs more CI than strictly needed; workflow edits are rare and
the failure mode it buys out is a silent one. PyYAML rejects the reserved-character scalar that
caused the original failure.
