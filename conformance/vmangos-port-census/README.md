# World Source port census

These reports record the transition from classic-db to vmangos at patch 10 and client build 5875.
The Source Profile enforces column contracts and approved values. Reported counts do not gate
imports. The current import floors and their reasons are in
[vmangos-floors.md](../../importer/manifests/vmangos-floors.md).

The comparison commands in these reports refer to the temporary census reader. To reproduce them,
use Core commit `65ecc0cd` and the source identities recorded in each report. The final importer reads
only vmangos SQLite. Source snapshots and exported plans remain local.

Milestone 5 assigns `game_quest_event_requirement` to `creature-ai`. It emits two rows for
`full-world` and one for `alliance-single`; `quests` no longer writes that table. The archived
Milestone 4 family counts predate that ownership change. Combined rows are unchanged.

The [Feature Ledger](../README.md) records the remaining gaps, including
the generated per-creature behavior comparison. These reports describe source and import plans;
they do not establish real-client conformance.
