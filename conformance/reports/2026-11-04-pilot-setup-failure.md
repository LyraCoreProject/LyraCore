# Pilot-end Conformance Audit attempt, 2026-11-04

The full Conformance Audit stopped during Reference Server fixture setup. It did not produce
completed audit evidence. This report records the failed attempt and the limits on the pilot
scaling decision. No gameplay, fixture, comparator, baseline or configuration code changed.

## Run and outcome

- Core: `7368d7cd5f58853616e094c9b2334902cab678ac` (`origin/main` after HTTPS fetch).
- Working tree: clean at launch and unchanged throughout the attempt.
- Harness: `1fcc4ec9c739fd3f6cee7584b846054db189ced1`.
- Command: `./conformance/audit --date 2026-11-04 --client-data /home/computer/WoW/client/Data`.
  This is the scoreboard's `evidence_refresh.next_command` with the client path supplied.
- Start: `2026-11-04T11:00:43.010327Z`; finish: `2026-11-04T12:05:27.373498Z`.
- Wall time: **3,884.36 seconds (1h 4m 44s)**. Process exit code: **1**.
- Attempts: **one**. No retry: the scenario ordering causes the same Account/name collision at
  this commit. Resolving it needs a fixture lifecycle change, outside the instruction to fix nothing.
- SpacetimeDB CLI: isolated 2.14.0 installation under task scratch, with its directory first on
  PATH and `SPACETIME_BIN` set to `spacetimedb-cli`. The default CLI was not changed.
- Fresh import: completed; all selected World Shard and Instance Pool content checks passed.
- Reference candidates: first two scenarios captured. Third failed before capture.
- LyraCore candidates: **none**. The full runner captures all Reference scenarios first.
- Comparison/report phase: **not reached**. No canonical audit JSON or candidate traces were emitted.
  This Markdown file is a failure record and must not be treated as fresh audit evidence.

The runner removed the disposable Realm worktree and restored and stopped its Reference Server.
All five audit TCP ports were free after exit. Task scratch was removed after preserving this
failure record. Existing imported-world snapshot caches and the prepared Reference Server runtime
were retained. Nothing from this audit remains running.

## Wall time by phase

| Phase | Seconds | Result |
| --- | ---: | --- |
| Realm startup, including cold Native Host build, Module build and preflight | 1726.82 | completed |
| Fresh world and client-data import | 1919.19 | completed |
| Reference Server startup | 54.80 | completed |
| `bank_slot_purchase`, Reference | 19.73 | captured, not compared |
| `bank_slot_purchase__observer`, Reference | 26.20 | captured, not compared |
| `bank_slot_purchase__spawn_beside_viewer`, Reference | 3.55 | fixture cleanup failed before capture |
| Other startup/build/cleanup overhead | 134.07 | residual, not separately instrumented |
| Total | 3884.36 | failed |

The residual is total elapsed time minus the printed phase timings. No separate client hashing,
snapshot sealing or comparison duration was printed for this full-run path. No snapshot reuse
was used. `wasm-opt` was unavailable; the CLI continued successfully with the unoptimized Module.

## Confirmed setup finding

**Class: fixture gap.** The full runner reassigns the same globally unique Character name to a
different Account between adjacent catalogue scenarios, but cleanup searches only the new Account.

1. `bank_slot_purchase` and `bank_slot_purchase__observer` put `Banktester` first, on `CONFORMANCE`.
2. `bank_slot_purchase__spawn_beside_viewer` puts viewer `Spawnwatch` first and actor `Banktester`
   second, assigning the actor to `CONFORMANCE2`.
3. `capture_servers` maps Character order to `CONFORMANCE` and peer Accounts, then calls
   `delete_reference_character` on only the assigned Account. The previous capture leaves
   `Banktester` on `CONFORMANCE`.
4. The harness `char-delete` command calls `create_or_find_char`. That function enumerates only
   the logged-in Account, accepts a name-in-use creation response, and searches that Account again.
   It cannot obtain a Character owned by the other Account.

Terminal diagnostic, with no credentials:

```text
Reference Character cleanup failed:
[wire] char-delete probe: logon + char-select only (no world entry)…
Error: character "Banktester" not found after create
```

The failed command used `--account CONFORMANCE2 --character Banktester`. This was not a
reported SRP proof failure; the runner's proof-failure retry does not apply.

Evidence at the captured revisions:

- [Account assignment and pre-capture cleanup](https://github.com/LyraCoreProject/LyraCore/blob/7368d7cd5f58853616e094c9b2334902cab678ac/conformance/audit.py#L1039-L1050).
- [Cleanup command and limited retry](https://github.com/LyraCoreProject/LyraCore/blob/7368d7cd5f58853616e094c9b2334902cab678ac/conformance/audit.py#L770-L784).
- [Observer actor order](https://github.com/LyraCoreProject/wire-harness/blob/1fcc4ec9c739fd3f6cee7584b846054db189ced1/scenarios/generated/bank_slot_purchase__observer.toml#L7-L40).
- [Spawn variant reverses actor order](https://github.com/LyraCoreProject/wire-harness/blob/1fcc4ec9c739fd3f6cee7584b846054db189ced1/scenarios/generated/bank_slot_purchase__spawn_beside_viewer.toml#L7-L30).
- [Character deletion command](https://github.com/LyraCoreProject/wire-harness/blob/1fcc4ec9c739fd3f6cee7584b846054db189ced1/src/scenarios/probes.rs#L206-L217).
- [Account-local create-or-find behavior](https://github.com/LyraCoreProject/wire-harness/blob/1fcc4ec9c739fd3f6cee7584b846054db189ced1/src/lib.rs#L1158-L1187).

This setup defect does not establish a gameplay defect or a Golden Trace mismatch. No fix was made.

## Per-scenario comparison availability

Counts are rule / data **signature sets**, not raw packet records. Tier 1 values below are from
[the committed baseline](../tier1-baseline.json) at the captured core commit. All 16 entries use
the harness revision above. Tier 1 was not rerun. Shared, tier-1-only and audit-only require a
LyraCore audit capture, so they are unavailable for every scenario. Unavailable never means zero.
Reference stability is unknown even for the first two captures: comparison was not reached and
the full runner deleted its temporary Reference candidates during cleanup.

| Scenario | Tier 1 rule / data | Audit rule / data | Shared rule / data | Tier-1-only rule / data | Audit-only rule / data | Reference stability |
| --- | ---: | --- | --- | --- | --- | --- |
| `bank_slot_purchase` | 0 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `bank_slot_purchase__observer` | 0 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `bank_slot_purchase__spawn_beside_viewer` | 0 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `cast_and_interrupt` | 9 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `connection_identity` | 0 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `quest_completion` | 6 / 6 | unavailable | unavailable | unavailable | unavailable | unknown |
| `quest_log` | 0 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `quest_offers` | 0 / 9 | unavailable | unavailable | unavailable | unavailable | unknown |
| `quest_reward` | 16 / 2 | unavailable | unavailable | unavailable | unavailable | unknown |
| `taxi_single_hop` | 0 / 2 | unavailable | unavailable | unavailable | unavailable | unknown |
| `taxi_single_hop__late_observer` | 32 / 2 | unavailable | unavailable | unavailable | unavailable | unknown |
| `taxi_single_hop__reconnect` | 110 / 1 | unavailable | unavailable | unavailable | unavailable | unknown |
| `two_character_visibility` | 16 / 1 | unavailable | unavailable | unavailable | unavailable | unknown |
| `two_character_visibility__opposite_faction` | 21 / 1 | unavailable | unavailable | unavailable | unavailable | unknown |
| `vendor_offers` | 19 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| `vendor_transactions` | 124 / 0 | unavailable | unavailable | unavailable | unavailable | unknown |
| Total | 353 / 24 | unavailable | unavailable | unavailable | unavailable | unknown |

The signature identities remain in the linked baseline; no audit signature identities exist for
this attempt. There are **no established audit-only groups to classify**, because no LyraCore
candidate was captured. This is missing evidence, not a finding that the audit-only set is empty.

## Reference stability and the scaling decision

This attempt cannot establish whether any Reference comparison matches its Golden Trace. The
maintainer reported a same-day single-scenario `0x00AA` drift with evidence withheld, and taxi
weather drift. Those are prior observations supplied with the task, not measurements reproduced
by this attempt. They remain unresolved risks. It would be unsound to declare the Reference
stable or use this failed attempt to certify pilot conformance.

The five earlier taxi audit-only signatures at `3dacd641` are all **unmeasured here**:

| Checkpoint | Rule | Opcode or field | Remains at the audited commit? |
| --- | --- | --- | --- |
| `flight` | `extra_packet` | `0x0143` | unknown |
| `flight` | `extra_packet` | `0x0144` | unknown |
| `flight` | `extra_packet` | `0x0132` | unknown |
| `flight` | `unexpected_packet` | `0x0134` | unknown |
| `flight` | VALUES difference | `field_46` | unknown |

Read-only inspection found that creature aggro target selection excludes dead and godmode
Characters but carries no taxi-flight state in `AggroTarget`:
[selection](https://github.com/LyraCoreProject/LyraCore/blob/7368d7cd5f58853616e094c9b2334902cab678ac/module/src/creatures/cycle/mod.rs#L752-L759),
[target construction](https://github.com/LyraCoreProject/LyraCore/blob/7368d7cd5f58853616e094c9b2334902cab678ac/module/src/creatures/cycle/ctx.rs#L549-L574).
That is an investigation lead if passenger combat packets recur, not proof that these five
signatures remain or share one cause. No classification of uncaptured packets is justified.

## Results potentially flattered by fixtures

The committed tier 1 baseline has zero rule signatures for these seven scenarios:
`bank_slot_purchase`, `bank_slot_purchase__observer`, `bank_slot_purchase__spawn_beside_viewer`,
`connection_identity`, `quest_log`, `quest_offers`, and `taxi_single_hop`.
The last two have data signatures; data differences do not fail rule conformance.

Whether any of those seven differs in the full audit is **unknown**. No scenario can be identified
as fixture-flattered from this attempt. A full audit remains necessary after the setup blocker
is resolved, with Reference stability checked before using candidate differences for scaling.

## Handoff limits

No source, tests, scripts or configuration changed; comment review does not apply. No local test
suite, formatting or clippy checks were run. No real-client check was performed. Git SSH fetch
failed with `Permission denied (publickey)`; authenticated HTTPS fetch succeeded. No `git config`
command was run. The PR contains only this failure report; there is no canonical audit report,
no Reference candidate, no world data and no credential material.

Unfinished: complete catalogue capture on both sides, Reference comparisons, audit signature
sets, audit-only classification and identification of fixture-flattered results. The scaling
decision lacks the requested full-audit evidence.
