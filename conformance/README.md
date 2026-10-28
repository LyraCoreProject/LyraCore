# Feature Ledger

Each `features/<system>/<behavior>.toml` describes one Behavior's 1.12.1 scope and evidence
requirements. `lyracore-conformance` loads all entries together and refuses malformed fields,
unknown fields, mismatched paths, duplicate ids, unknown dependencies and dependency cycles.
[ADR 0005](../docs/adr/0005-1-12-conformance.md) defines repository ownership and verification tiers.

```bash
cargo test -p lyracore-conformance -- --nocapture
cargo test -p lyracore-protocol --example benilla_inventory
cargo run -p lyracore-protocol --example benilla_inventory
```

The first command checks every ledger file and its scenario references. The second checks retained
Rust evidence declarations. The inventory command reads these same files
and adds source-presence facts. It never runs the named evidence tests.

## Schema version 1

All fields below except `known_gaps` are required. Use an empty array when no
entry exists yet.
All nested tables refuse unknown fields too. Paths are relative, without parent-directory components.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` |
| `id` | Globally unique Behavior id, matching the filename without `.toml` |
| `system` | Scoreboard group, matching the directory name |
| `scope` | The 1.12.1 result in plain words, including boundaries that matter |
| `requests`, `replies` | Client and server opcode claims, written as strings such as `0x018E` |
| `dependencies` | Other Behavior ids that this Behavior requires |
| `vmangos` | Source reference with `commit`, `path` and `symbol`; the commit is a full SHA |
| `required_rungs` | Evidence needed to count the Behavior as done |
| `scenarios` | Plain string ids in the pinned harness catalogue; empty until authored |
| `deliberate_deviations` | Intentional result differences, each with an `id`, `result`, `reason` and HTTPS `source` |
| `known_gaps` | Optional list of defects with `description`, relative core `path`, `symbol` and `evidence`; no status or classification effect |
| `real_client` | A human-maintained check, described below |
| `prerequisites` | Fixture or world-data requirements in plain words |
| `tests` | Retained Rust evidence, each with a source `path`, test `function` and `rung` |

Ids for Behaviors, systems and deviations start with a lowercase ASCII letter and contain only
lowercase letters, digits and underscores. Scenario ids have no prescribed naming syntax. Empty
and duplicate scenario ids fail. Opcode strings use `0x` and four uppercase hexadecimal digits.
A Behavior must claim a request or reply unless it records a known world-data gap with no opcode
claim. Claims need not be exclusive across Behaviors.

The rungs are `unit`, `codec`, `durable_module`, `tier1`, `conformance_audit` and `real_client`.
A required rung is an obligation, not a result. The `tests` array names Rust source evidence from
the first three rungs. Harness scenarios supply tier 1 and audit evidence. The scoreboard joins
those results to the ledger.

Until a person checks the Behavior in a build-5875 client, write:

```toml
[real_client]
status = "unverified"
```

After that check, use `status = "verified"` with a valid `date` in `YYYY-MM-DD` form, `checked_by`
and `evidence`. The last field names the recorded observations. Audits never update this table.
No other handwritten status is accepted. A missing implementation is a gap, not a Deliberate
Deviation; that field records a result LyraCore has chosen to keep different.

Known gaps record observed defects and their comparison evidence. They neither excuse differences
nor change rule/data classification, pass counts or freshness. The scoreboard shows their count
separately. Each entry requires nonempty description, symbol and evidence fields and a relative
source path. Unknown fields, including status, fail.

### Data-dependent fields

The scenario author declares data-dependent fields in the harness scenario's `[[rules.data_fields]]`.
The scenario hash covers them. The comparator emits tagged records for changed data values and
keeps presence, shape, identity, ordering and timing failures as rule records. Core never declares
or reclassifies data fields. Known gaps cannot change a tag or excuse a failure.

## Migration and source evidence

The original nine entries replace the former `benilla_inventory/behaviors.json`. They retain Behavior ids,
opcode claims, prerequisites and all eight Rust evidence links. The old `support`, `remaining` and
`durable_operations` fields do not belong to this schema. Scope states the required result; test
presence does not certify it. Native Python checks remain in place until a recorded conformance
scenario replaces each one.

The vmangos references name reviewed functions at one full commit. They point to sources, and
contain no copied implementation or world data. Those references identify where to investigate
and record each Behavior; they are not a claim that the original nine conform today. Their real-client
checks remain unverified and their scenario lists are empty. Port-gap Behaviors are recorded beside them.

## Scenario catalogue and tier 1

`wire_client::catalogue` embeds the scenarios and Golden Traces at the revision in
`.wire-harness-rev`. The workspace pins `wire-client` to that full commit SHA. The Module tests
and ledger tests use it only as a dev-dependency. The `harness_pin` Architecture Test checks
that both consumers inherit the same revision as `dev smoke`, and that every workspace member
keeps `wire-client` out of normal and build dependencies, including renamed and target-specific uses.
It also requires exactly one resolved package with the pinned git source and commit. Workspace
manifests cannot patch `wire-client`. Every `config*` file in workspace ancestors' `.cargo/`
directories and `$CARGO_HOME` must be free of `paths`, `[source.*]` and `replace-with` overrides.
The resolved `manifest_path` must remain inside `$CARGO_HOME/git/checkouts/` after resolving
symlinks, and its checkout's `HEAD` must equal the pin. The audit applies these checks too.

Every scenario reference must exist in that catalogue and name the Behavior that lists it.
A missing id, mismatched Behavior or invalid Golden Trace fails the ledger check. No local harness
checkout is read and no catalogue check can skip. The pinned harness enables
`serde_json/float_roundtrip` itself, so loading a Golden Trace preserves exact wire-coordinate
values. Core needs no feature override.

Run tier 1 with the disposable Shard commands in [testing](../docs/testing.md#conformance).
The runner prepares the scenario's Character through debug reducers, then runs the
harness's `Transport` over the buffered Operator connection. It decodes replies and compares them
with the Golden Trace using the harness library. Direct and AOI delivery follow the Module's
ordered grants, revokes and clears. Transport Loss, an unexpected close, a cipher transition or
unsupported delayed Host delivery fails the test. Scheduled transactions remain observable through
`wait_for_packets`.

The schema-3 runner records world-entry packets in the opening phase. Core applies their delivery
instructions to establish AOI Grants, then retains the packets as opening observations. The harness
excludes opening observations from comparison and starts comparison at the first step. Core never
filters packets during the scenario or changes its Equivalence Rules.

The test writes `.lyracore/conformance/tier1.json` with the core commit, working-tree state, harness
revision, capture time and the harness comparison. It removes the scenario's previous result before
setup, so a setup failure leaves no old pass. Tests in one process update the report under a lock
and preserve other scenarios from the same revision. Both difference classes remain in the report. Only rule records fail the
test. No packet text enters this result. The scoreboard consumes it.

## Taxi data

`taxi_single_hop` uses the harness's money, known-flight-node and creature Fixture Verbs. The
Operator transport calls the same debug reducers as the wire adapter. `debug_learn_taxi_node`
requires the Operator, an existing Character and a storage id in `game_taxi_node`. Imported storage
ids equal client node ids. Learning is idempotent and sends no packets or flight requests.

The tier 1 test installs the hand-written fixture in
`conformance/support/taxi_route.rs`. It contains only nodes 2 and 4, path 6 with fare 110,
its 28 vertices, and a minimal synthetic template for the scenario's flight master, entry 352.
The node and route numbers were checked against build-5875 `TaxiNodes.dbc`, `TaxiPath.dbc` and
`TaxiPathNode.dbc`. Node text is fixture-authored. Mount display 6852 agrees with the Golden Trace;
no mount template or other world data is needed.

`route_fixture_matches_every_recorded_route_point_and_mount` checks all 27 serialized spline
destinations exactly against vertices 1 through 27, including their wire precision. The trace
starts at the passenger's position, which matches node 2, and omits DBC vertex 0 from its
destinations. Vertex 0 and destination node 4 were checked against the client data; the trace
does not independently establish those two rows. The test also checks the passenger's mount display.

```bash
cargo test -p lyracore-module --test native_conformance taxi_single_hop -- --ignored --nocapture
```

This command needs no imported data or `LYRACORE_TEST_TAXI_SQL`. It starts its own disposable
Shard and fails with the full comparator report for the eight known gameplay gaps. Durable CI
keeps the explicit `taxi_single_hop_matches_its_golden_trace` skip until those gaps are fixed.
Remove the skip in the PR that fixes the last gap. All other conformance checks stay in CI.
Known gaps never excuse a difference or supply a passing baseline.

The [T2g comparison](reports/2026-10-27-t2g.md) records the conformance.4 aligned differences
and maps them to the taxi entry's known gaps. The [T2d comparison](reports/2026-10-27-t2d.md)
retains the earlier conformance.3 evidence. Their imported-data setup describes those historical
runs; tier 1 now uses the bundled fixture.

## Scoreboard

From the repo root:

```bash
./conformance/scoreboard
cargo test -p lyracore-conformance --example scoreboard
```

The script runs the Rust example `scoreboard`. It reads the Feature Ledger and prints system totals
and a row for each Behavior. Tier 1, Conformance Audit and real-client checks have separate columns.
It does not infer a pass from a test declaration or claim overall completion from these columns.
The ledger's other required rungs remain obligations.

The default tier 1 input is `.lyracore/conformance/tier1.json`. The latest audit is the JSON report
under `conformance/reports/` with the greatest `recorded_at_unix_ms`. Each audit has a Markdown report beside its JSON result. To select inputs explicitly:

```bash
./conformance/scoreboard --tier1 /path/to/tier1.json --audit /path/to/audit.json
```

Missing default inputs show `not run`; Behaviors with no scenario show `no scenario`. A missing
explicit input or malformed report fails the command. A changed core, harness revision
or scenario hash shows `stale`. A descendant core commit that changes only `conformance/reports/`
remains current, so committing the report does not invalidate its measurement. A pass requires every scenario listed by the Behavior. Any rule
difference, including a counted Deliberate Deviation, prevents a pass. Rule differences in the Reference Server
comparison also prevent an audit pass. A matching run made with
uncommitted changes shows `unverified (dirty tree)` and contributes no pass to system totals.
Real-client cells come only from the ledger, never from a report.

Counts use unique opcode values. The denominator is every variant of the pinned core
`ClientOpcodeMessage` enum, read from Cargo's dependency source. Claims outside that enum have
a separate count. System totals count shared claims once. The unavailable count includes request
patterns in `responses` and `raw_responses` in `module/src/native/unavailable.rs`. It excludes
control receipts, tests, comments, imports and explicit empty responses for ignored requests.
Per-system and per-Behavior unavailable counts intersect those requests with ledger claims.
Neither opcode claims nor these source counts prove conformance.

### Result schema version 2

Both tiers retain the complete harness `Comparison`. Each record requires the comparator's
`classification` tag, `rule` or `data`. There is no separate `data_differences` array. Version 1
results lack tags and must be rerun. Summary counts never supply classification.

A signature is `(rule, checkpoint, stream, opcode, field_path)`. Packet indices, reasons and
values do not belong to it. The scoreboard groups signatures across all scenarios of a Behavior
and shows distinct signatures and raw records separately for rules and data. Known gaps remain a
separate descriptive count. Missing or unknown tags, unknown Behaviors, unlisted scenarios,
duplicate results and malformed reports fail before freshness checks.

| Field | Meaning |
| --- | --- |
| `schema_version` | `2` |
| `tier` | `tier1` or `conformance_audit` |
| `core_commit` | Full core commit measured by the run |
| `working_tree_dirty` | Whether the run included uncommitted changes |
| `harness_revision` | Full pinned harness commit |
| `recorded_at_unix_ms` | Capture time used to choose the latest report |
| `scenarios` | Result array, with unique scenario ids |
| `scenarios[].id`, `scenarios[].behavior` | Scenario and Behavior ids |
| `scenarios[].scenario_sha256` | Hash from the Golden Trace header |
| `scenarios[].comparison` | Complete tagged comparison of LyraCore with the Golden Trace |
| `scenarios[].reference_comparison` | Audit-only reference-stability comparison of vmangos with that same Golden Trace |
| `scenarios[].reference_sha256` | SHA-256 of the Reference Server candidate, retained only in scratch |
| `scenarios[].capture` | Audit-only LyraCore candidate SHA-256, core commit, Native Host SHA-256 and Module SHA-256 |
| `audit` | Audit-only server provenance and per-Behavior summaries |

`audit` records the Native Host and Module binary hashes, imported world description, and vmangos
binary release, archive hash, source commit and database commit. Its `behaviors` map holds each
Behavior's `candidate` and `reference` summaries. Each summary has `rule` and `data` groups with
`raw_records` and a `signatures` array of `{signature, raw_records}`. The scoreboard recomputes
the LyraCore comparison by replaying its scrubbed candidates in `<date>.traces/` against the
pinned catalogue. Every stored LyraCore difference must equal the recomputed record, including its
tag and values. Reference Server candidates remain in scratch; core retains their SHA-256 and
reference-stability comparison only.
Data tags also require a matching scenario data-field declaration, and checkpoints must exist.
The comparator's empty checkpoint denotes an unmatched rule declaration, not a scenario step.
The scoreboard then checks the summaries and verifies that Markdown equals its JSON rendering.
The `committed_audit_report_replays` test loads the newest committed report and runs validation,
change validation and candidate replay in the Rust workflow's unit-test job. This checks report
integrity, not whether every audited Behavior conforms.

Each LyraCore candidate's bytes must match `capture.sha256`, and its core commit and binary hashes
must match the audit run. At the pinned harness revision, the candidate header has no slot for
those three provenance values; it records only the adapter and scenario metadata. The report
binding cannot prove that a relabeled Golden Trace came from LyraCore. A pass is corroborated by
the durable tier-1 check, which recomputes LyraCore's behavior in CI. The taxi comparison still has
its explicit durable CI skip, so it has no such passing corroboration until its gaps are fixed.

`audit.previous_report` is the preceding JSON filename, or null on the first audit. Each Behavior's
`changes` has `rule` and `data` sets of `fixed` and `regressed` signatures. It is null when no
earlier audit exists or the harness revision or any scenario hash for that Behavior differs. In the
latter case the report says comparison is not possible. Multiplicity and values cannot fix or
regress a signature. The scoreboard checks these sets against the named earlier report. It also
replays that earlier report's LyraCore candidates when its harness revision matches the current pin.
Older reports receive structure checks; the newest report receives current ledger checks, so a
Behavior rename cannot invalidate old reports. Earlier T2d/T2g narrative notes are not audit baselines.

## Conformance Audit

Prepare the pinned Reference Server once with the harness's
[setup instructions](https://github.com/LyraCoreProject/wire-harness/blob/bc83d81cc1992fd1b522cab38c2ea048b5c6db23/adapters/vmangos/README.md).
Stop it before the audit. The runner refuses a running Reference Server or occupied reference ports.
Set the client Data path with `./lyracore config set client-data PATH` before the audit.
Then run:

```bash
./conformance/audit --date 2026-10-28
./conformance/scoreboard
```

The Python command manages the local Realm through `lyracore dev`. The Rust
`audit` example exports catalogue scenarios, compares candidates through the pinned library, and
writes the reports. It shares counting and evidence validation with the scoreboard. Neither core
command reads scenarios or Golden Traces from a harness checkout. Cargo metadata locates the
pinned checkout only to build `vanilla-wire` and run its server adapters.

The command builds `vanilla-wire` and the audit example. It starts an owned local Realm with
`lyracore dev up`, runs `lyracore import --accept` using the client path from `lyracore config`,
and provisions a disposable Account. The import selects the current vmangos `db_latest` snapshot
under `.import/vmangos/` and loads the `alliance-single` world profile. It uses normal SRP login
and encrypted world TCP sessions. Fixture world-entry bypasses are not compiled into this Module.
The audit removes any prior catalogue Characters and retires entry 352 spawns in the taxi fixture's
50-yard cell. The imported flight master there would otherwise be mistaken for the scenario's
own spawn. The report records this adjustment. Tier 1 still uses the bundled route fixture.

All Docker commands run inside a `newgrp docker` heredoc. The command checks the prepared runtime's
pins and provenance, starts its compose services, and resets only the catalogue Characters on the
disposable Reference Server. Every scenario produces both a LyraCore and vmangos candidate in scratch. Both
compare with their embedded Golden Trace; the Reference Server must also retain its recorded
provenance. Reference rule and data counts remain visible even when LyraCore fails.

The command writes `conformance/reports/<date>.md`, `<date>.json` and `<date>.traces/`, refusing
existing outputs. The trace directory contains only LyraCore candidates. Reference Server
candidates stay in the audit's scratch directory and are deleted with it at exit; the report keeps
their SHA-256 and reference-stability results. `--output DIRECTORY` selects another report destination. To replace the single
baseline, capture with the same date into scratch and replace all three outputs together. A report
with that same filename is not used as its own previous report. Rule differences are audit findings
and do not make the command fail; missing captures, infrastructure errors and invalid evidence do. A
failed run cannot write a partial success report. No scenario is skipped, including taxi.
Real-client fields, Equivalence Rules and Deliberate Deviations are untouched.

The runner stops both servers on exit, including capture failures and normal interrupts. Credentials
and crash diagnostics stay under `$HOME/.cache/lyracore-scratch/conformance/` and are deleted when
the run ends. Build output uses its `target/` directory. The prepared Reference Server's runtime
stays under `$HOME/.cache/lyracore-scratch/reference-server/`. Reports contain only provenance,
counts, signatures and scrubbed difference values. Committed LyraCore candidate traces contain
numbers and text digests, with the same scrubbed field types as Golden Traces. Never commit a
Reference Server candidate; Golden Traces belong in the harness. Commit the implementation before
capturing a report for current evidence, then commit only the report and its candidates. Dirty
captures remain unverified.

## Opcode coverage

`tests/opcode_coverage.rs` checks every variant of the pinned `ClientOpcodeMessage` against ledger
request claims, `opcodes-out-of-scope.toml`, and `opcodes-unclaimed.txt`. Out-of-scope entries map a
variant name to a table with `reason` and an optional `source`. That list is currently empty. The
initial allowlist has 285 variants. Adding a Behavior or a scope decision requires removing its
opcodes from the allowlist in the same change. Unknown names, duplicate allowances and overlapping
categories fail.

The Architecture Test embeds the initial 285 unclaimed names and the initially empty out-of-scope
set as constants. Current unclaimed membership must be a subset of that initial set. An
out-of-scope addition requires `source` to name an existing `docs/adr/*.md` that mentions the opcode
by name, or `https://github.com/vmangos/core/blob/<SHA>/src/<path>` with the same `source_commit`
as the pinned Golden Traces.

The test also compares both scope files with earlier versions reachable from `HEAD`. This extra
history check needs no particular commit SHA. It catches restored allowances, exchanges of one
opcode for another, and deletion followed by recreation. Scope files keep their canonical paths;
renames cannot reset the embedded initial sets. A shallow checkout fails with an instruction to
fetch full history; Rust CI uses `fetch-depth: 0`. The scanner uses `cargo metadata --locked`,
which can download missing workspace dependency sources on a fresh checkout. It never changes
the dependency lockfile.

`feat/conformance` merges into `main` with a merge commit to retain the intermediate scope history.
The embedded initial sets still enforce the starting limit after a squash or rebase merge.

## Trainable spells

The scoreboard uses `.import/vmangos/current.json`, the same snapshot selection as `lyracore
import`, or accepts `--vmangos /path/to/mangos.sqlite`. It opens SQLite with `mode=ro` and
`query_only`. A missing snapshot prints `Trainable spells: no data` with the expected path and
leaves the rest of the scoreboard usable. A snapshot path outside `.import/vmangos/`, unreadable
files, malformed schemas and dangling teaching-spell references fail. No vmangos rows enter git.

For each class, "trainable" is the union of:

- Spells offered by class trainers, `trainer_type = 0`, using `trainer_class` from the latest
  `creature_template.patch <= 10`, the 1.12 patch. Join direct `npc_trainer.entry` to that creature
  and `npc_trainer_template.entry` to its `trainer_id`. Keep offers whose inclusive build range
  contains 5875 and whose required level is at most 60. Pet and profession trainers are excluded.
- Spells automatically learned on a class skill line, `learn_on_get_skill` 1 or 2, from the latest
  `skill_line_ability.build <= 5875` per id. The row's `class_mask` must be zero or include that
  class. Class lines are the three class specializations, plus Rogue poisons and lockpicking.
  Hunter pet-training lines, weapons, professions and racials are excluded. A talent or quest-only
  spell without either learning source is outside this denominator.

Use the largest `spell_template.build <= 5875` per entry. Resolve teaching spells through each
`LEARN_SPELL` effect, raw id 36, to its `effectTriggerSpell` before counting. Count each learned id
once per class, keeping ranks separate. A spell must have at least one nonzero effect. Every
nonzero effect must map to a supported kind through the importer rules, in all three slots.

The scoreboard reads trainable spell rows from the selected vmangos SQLite file and calls
`lyracore-importer::spell_kind::supported_effect` for each nonempty effect slot. The importer
uses the same kind mappings and name corrections when building the spell catalogue. The counter
therefore has no copied Rust mapping or Python list of script-name exceptions. Raw 5, 29 and 96
are not blanket support for Blink or Charge. Raw 59, 63 and 91 use importer mappings. Unknown
kinds stay unsupported unless the importer has a named correction.

Class skill lines follow vmangos at
[`d9173eca`](https://github.com/vmangos/core/tree/d9173eca3ea08dd985c52dabbcc826898d1d0b77),
`src/game/SharedDefines.h`.

The Module dispatch is `module/src/spell/cast/targeting.rs::apply_effect`; aura consumers live in
`module/src/spell/` and `crates/lyracore-shared/src/aura.rs`. This is a kind-coverage count. It does
not verify parameters, targeting, triggered spell dependencies, quests, talents or complete spell
behavior. Only counts are printed; spell names and SQLite records stay local.
