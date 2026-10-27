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

All fields below except `data_fields` and `known_gaps` are required. Use an empty array when no
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
| `data_fields` | Optional array of sourced, typed data-dependent fields, empty when omitted |
| `real_client` | A human-maintained check, described below |
| `prerequisites` | Fixture or world-data requirements in plain words |
| `tests` | Retained Rust evidence, each with a source `path`, test `function` and `rung` |

Ids for Behaviors, systems and deviations start with a lowercase ASCII letter and contain only
lowercase letters, digits and underscores. Scenario ids have no prescribed naming syntax. Empty
and duplicate scenario ids fail. Opcode strings use `0x` and four uppercase hexadecimal digits.
A Behavior must claim at least one request or reply. Claims need not be exclusive across Behaviors.

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

Each `[[data_fields]]` declaration names one exact field location. It requires `scenario`,
`checkpoint`, `stream`, `opcode`, `field_path`, `value_type`, `reason` and `source`.
The scenario must belong to this Behavior. `stream` is a table with `kind = "reply"` or
`kind = "broadcast"` and a stream `id`. `opcode` is a numeric server opcode already claimed in
`replies`. `field_path` is a nonempty array of field names and decimal list indices, with no
wildcards. `value_type` is `unsigned`, `signed`, `float` or `text_sha256`.
`source` holds a pinned `commit`, relative `path` and `symbol`, like `vmangos`.
`reason` explains why this field depends on world data. Duplicate locations and opening-phase declarations fail.

Only an `exact_field` difference at that location, with both values of the declared type and a
packet index, can count as data. Missing fields, type or structure changes, packet counts and order,
timing, identity rules and counted Deliberate Deviations remain rule differences. A declaration
does not change the harness comparison or excuse tier 1 failures. No current ledger entry declares
a data-dependent field. The T2d destination-coordinate observation has no comparator record because
the landing packet is missing.

## Migration and source evidence

The nine entries replace the former `benilla_inventory/behaviors.json`. They retain Behavior ids,
opcode claims, prerequisites and all eight Rust evidence links. The old `support`, `remaining` and
`durable_operations` fields do not belong to this schema. Scope states the required result; test
presence does not certify it. Native Python checks remain in place until a recorded conformance
scenario replaces each one.

The vmangos references name reviewed functions at one full commit. They point to sources, and
contain no copied implementation or world data. Those references identify where to investigate
and record each Behavior; they are not a claim that all nine conform today. All nine real-client
checks remain unverified and their scenario lists are empty.

## Scenario catalogue and tier 1

`wire_client::catalogue` embeds the scenarios and Golden Traces at the revision in
`.wire-harness-rev`. The workspace pins `wire-client` to that full commit SHA. The Module tests
and ledger tests use it only as a dev-dependency. The `harness_pin` Architecture Test checks
that both consumers inherit the same revision as `dev smoke`, and that every workspace member
keeps `wire-client` out of normal and build dependencies, including renamed and target-specific uses.

Every scenario reference must exist in that catalogue and name the Behavior that lists it.
A missing id, mismatched Behavior or invalid Golden Trace fails the ledger check. No local harness
checkout is read and no catalogue check can skip. Both consumers enable `serde_json/float_roundtrip`
for development builds so loading a Golden Trace preserves exact wire-coordinate values. Without
it, decimal parsing can move a recorded coordinate by one f64 ULP and fail an exact comparison.

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
and preserve other scenarios from the same revision. Differences remain in the report and fail the
test. No packet text enters this result. The scoreboard consumes it.

## Taxi data

`taxi_single_hop` uses the harness's money, known-flight-node and creature Fixture Verbs. The
Operator transport calls the same debug reducers as the wire adapter. `debug_learn_taxi_node`
requires the Operator, an existing Character and a storage id in `game_taxi_node`. Imported storage
ids equal client node ids. Learning is idempotent and sends no packets or flight requests.

The seeded three-point taxi fixture is not route 6. Use your client and the pinned classic-db dump
to prepare the actual route through the existing importer, without `--apply`. Golden Traces retain
numeric route points. Commit no packet text, vmangos or client files, or bulk world data. The recorder
hashes text before writing a trace. `taxi-fixture.py` selects unchanged tuples from the importer's
output, not from a MySQL source dump.

```bash
scratch="$HOME/.cache/lyracore-scratch/conformance/taxi"
mkdir -p "$scratch"
cargo run -p lyracore-importer -- --dbc "$LYRACORE_CLIENT_DATA" \
  --taxi-mounts "$LYRACORE_CLASSIC_DB_SQL" > "$scratch/dbc.sql"
cargo run -p lyracore-importer -- --dump "$LYRACORE_CLASSIC_DB_SQL" \
  --dbc "$LYRACORE_CLIENT_DATA" --world-profile alliance-single > "$scratch/world.sql"
python3 conformance/taxi-fixture.py "$scratch/dbc.sql" "$scratch/world.sql" \
  > "$scratch/taxi.sql"
export LYRACORE_TEST_TAXI_SQL="$scratch/taxi.sql"
cargo test -p lyracore-module --test native_conformance taxi_single_hop -- --ignored --nocapture
```

The selection contains two taxi nodes, route 6, its 28 path nodes and creature template 352. The
importer resolves mount creature 541 to a client display in each node, so no extra mount template
is needed by the Module. The durable test loads these rows into its own Shard. Missing input fails
with a setup error; it never substitutes the seeded geometry or skips a comparison. The same SQL
can prepare a disposable Native Host for the harness's LyraCore adapter.

The [T2g comparison](reports/2026-10-27-t2g.md) records the conformance.4 aligned differences
and maps them to the taxi entry's known gaps. The [T2d comparison](reports/2026-10-27-t2d.md)
retains the earlier conformance.3 evidence.
The ignored taxi test still fails with the comparator report when run with imported data.
Durable CI explicitly skips `taxi_single_hop_matches_its_golden_trace`, naming its ledger entry in
the workflow. All other conformance checks remain in CI. This skip records an open Behavior;
there is no known-differences baseline.

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
under `conformance/reports/` with the greatest `recorded_at_unix_ms`. An audit can keep a Markdown
narrative beside that JSON. No audit report exists yet. To select inputs explicitly:

```bash
./conformance/scoreboard --tier1 /path/to/tier1.json --audit /path/to/audit.json
```

Missing default inputs show `not run`; Behaviors with no scenario show `no scenario`. A missing
explicit input or malformed report fails the command. A different core commit, harness revision
or scenario hash shows `stale`. A pass requires every scenario listed by the Behavior. Any harness
difference, including a counted Deliberate Deviation, prevents a pass. A matching run made with
uncommitted changes shows `unverified (dirty tree)` and contributes no pass to system totals.
Real-client cells come only from the ledger, never from a report.

Counts use unique opcode values. The denominator is every variant of the pinned core
`ClientOpcodeMessage` enum, read from Cargo's dependency source. Claims outside that enum have
a separate count. System totals count shared claims once. The unavailable count includes request
patterns in `responses` and `raw_responses` in `module/src/native/unavailable.rs`. It excludes
control receipts, tests, comments, imports and explicit empty responses for ignored requests.
Per-system and per-Behavior unavailable counts intersect those requests with ledger claims.
Neither opcode claims nor these source counts prove conformance. Spell counts arrive in Milestone 3.

### Result schema version 1

The tier 1 runner writes this shape. Audit reports use `tier = "conformance_audit"` and the same
fields. An audit producer partitions the complete harness comparison into `comparison.differences`
for rule differences and `data_differences` for declared data-dependent fields. Both arrays contain
the harness's serialized `Difference` records, unchanged. The scoreboard re-derives each record's
classification from the current ledger's typed, sourced declarations and rejects a report if either
array disagrees. Unknown Behaviors and unlisted scenarios also fail, including extra result rows.
This check applies before freshness checks. Data differences print separately and do not prevent a
clean audit pass. Tier 1 keeps every difference in `comparison.differences` and cannot classify data.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` |
| `tier` | `tier1` or `conformance_audit` |
| `core_commit` | Full core commit SHA measured by the run |
| `working_tree_dirty` | Whether the run included uncommitted changes |
| `harness_revision` | Full pinned harness commit SHA |
| `recorded_at_unix_ms` | Capture time used to choose the latest report |
| `scenarios` | Result array, with unique scenario ids |
| `scenarios[].id`, `scenarios[].behavior` | Scenario and Behavior ids |
| `scenarios[].scenario_sha256` | Hash from the Golden Trace header |
| `scenarios[].comparison` | Harness `Comparison` with its counted differences |
| `scenarios[].data_differences` | Optional audit-only array, empty when omitted |

## Opcode coverage

`tests/opcode_coverage.rs` checks every variant of the pinned `ClientOpcodeMessage` against ledger
request claims, `opcodes-out-of-scope.toml`, and `opcodes-unclaimed.txt`. Out-of-scope entries map a
variant name to a nonempty reason. That list is currently empty. The initial allowlist has 285
variants. Adding a Behavior or a scope decision requires removing its opcodes from the allowlist
in the same change. Unknown names, duplicate allowances and overlapping categories fail.

The Architecture Test compares the current allowlist with every earlier committed version reachable
from `HEAD`. Membership may only shrink, even if an edit removes one opcode and adds another, or
restores a previously removed allowance. The first committed version establishes the starting set.
A shallow checkout fails with an instruction to fetch full history; Rust CI uses `fetch-depth: 0`.
The scanner uses `cargo metadata --locked`, which can download missing workspace dependency
sources on a fresh checkout. It never changes the dependency lockfile.
