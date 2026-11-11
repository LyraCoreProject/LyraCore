# Feature Ledger

Each `features/<system>/<behavior>.toml` describes one Behavior's 1.12.1 scope and evidence
requirements. `lyracore-conformance` loads all entries together and refuses malformed fields,
unknown fields, mismatched paths, duplicate ids, unknown dependencies and dependency cycles.
[ADR 0005](../docs/adr/0005-1-12-conformance.md) defines repository ownership and verification tiers.
Use [climb-conformance](../.claude/skills/climb-conformance/SKILL.md) for the separate implementation
and scenario-author loops.

```bash
cargo test -p lyracore-conformance -- --nocapture
cargo test -p lyracore-protocol --example benilla_inventory
cargo run -p lyracore-protocol --example benilla_inventory
```

The first command checks every ledger file and its scenario references. The second checks retained
Rust evidence declarations. The inventory command reads these same files
and adds source-presence facts. It never runs the named evidence tests.

## Schema version 1

All fields below except `known_gaps` and `scope_verification` are required. Use an empty array
when no entry exists yet.
All nested tables refuse unknown fields too. Paths are relative, without parent-directory components.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` |
| `id` | Globally unique Behavior id, matching the filename without `.toml` |
| `system` | Scoreboard group, matching the directory name |
| `scope` | The 1.12.1 result in plain words, including boundaries that matter |
| `scope_verification` | Optional `stock_client_use_unverified` when every request in the Behavior still needs a stock-client-use check; otherwise defaults to `established` |
| `requests`, `replies` | Client and server opcode claims, written as strings such as `0x018E` |
| `dependencies` | Other Behavior ids that this Behavior requires |
| `vmangos` | Source reference with `commit`, `path` and `symbol`; the commit is a full SHA |
| `required_rungs` | Evidence needed to count the Behavior as done |
| `scenarios` | Plain string ids in the pinned harness catalogue; empty until authored |
| `deliberate_deviations` | Intentional result differences, each with an `id`, `result`, `reason` and HTTPS `source` |
| `known_gaps` | Optional list of defects or open stock-client questions with `description`, relative core `path`, `symbol` and `evidence`; work kind derived from path |
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

Known gaps record observed defects or open stock-client questions and their evidence. They neither
excuse differences nor change rule/data classification, pass counts or freshness.
`scope_verification` marks Behaviors whose every request has unverified stock-client use. It
requires at least one request and does not change the denominator. The scoreboard shows known-gap
counts separately. Paths under `importer/` identify Data Gaps; other paths identify behavior gaps.
This work assignment does not change comparator tags or conformance results. Each entry requires
nonempty description, symbol and evidence fields and a relative source path. Unknown fields, including status, fail.

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

The build-5875 request inventory is `client-requests-5875.tsv`, transcribed from the pinned
[vmangos opcode table](https://github.com/vmangos/core/blob/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Server/Protocol/Opcodes_1_12_1.h).
Its 465 sorted `(id, name)` rows have SHA-256
`2ac5acc43e5e0f8eea113ec75626640557e801194324b6af08d561e5a9d8a216`. The coverage test
checks both values so a removed row fails even if its Behavior claim is removed too.
`opcodes-out-of-scope.toml` holds only requests with a cited administrator gate.
System Behaviors keep requests with uncertain stock-client use in the denominator and record the
dispatch result and open question in `known_gaps`.
The scoreboard fails when a request is unclaimed or both claimed and out of scope. It also checks
that every variant in the core `ClientOpcodeMessage` enum has the same number in the inventory.

Maintain the Behavior TOML files, inventory and scope decisions by hand. The coverage
check catches an inventory request with no Behavior or out-of-scope reason. New entries can leave
`replies` empty until a scenario author
checks the packets returned by the Reference Server. A vmangos dispatch entry marked unhandled
uses `BuildOpcodeList` as its source reference because it has no handler.

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
The tier 1 runner supports schema 3 single-Character scenarios and schema 4 named-Character
scenarios. Schema 4 uses one fixture connection and Account per Character. All connections share
the same virtual clock with a fixed starting timestamp, and each receives only packets addressed to it. Reconnecting a Character
keeps its GUID. `bank_slot_purchase` uses a synthetic friendly banker template with NPC flag
`0x100`; `two_character_visibility` checks each recipient's peer CREATE update.
The pinned `run_on` builds step observations with `Observation::from_frame_with_rules`, so
subject-filtered unexpected-packet rules inspect decoded subjects.
The committed `tier1-baseline.json` contains the harness revision, scenario hash, and sorted rule
and data signature sets for every catalogue scenario. A signature is the rule, receiving Character
when named, checkpoint, stream, opcode and field path. It holds no reason or packet values. The
durable test checks provenance and
requires exact set equality, including when a difference disappears. After one scenario changes,
use `./conformance/baseline <scenario>`. It runs only that scenario's tier 1 test and adds or
replaces its entry from the comparison made in that invocation. Other entries stay byte-identical.
To regenerate every entry, run all tier 1 scenarios, then `./conformance/baseline`. Both forms check the core commit,
harness revision and scenario hashes before writing the canonical file.
Review the changed signatures with the behavior change. This baseline does not change comparator
tags, scoreboard pass/fail or the Conformance Audit.

### Pull request signature ratchet

The `Conformance ratchet` job compares the PR's Tier 1 baseline with its merge base. For a
scenario with unchanged `scenario_sha256` and `harness_revision`, any added rule signature fails,
even if another signature was removed. Removed signatures and data changes pass this check.
New scenarios and changed scenario hashes or harness revisions are new measurements: they pass
and are listed in the job summary. The durable tests still require exact equality with the
committed baseline for both signature classes.

A maintainer can apply the PR label `conformance-accept-signature` to accept a deliberate addition.
Keep its reason and added signatures in the PR. Adding or removing the label reruns the check;
the summary retains accepted additions. A maintainer creates the label once in GitHub.

The workflow uses `pull_request_target` and runs only code from the trusted base branch. It reads
the two baselines as Git blobs and reads current labels from the GitHub API. PR files cannot
change the label decision. The token has only `contents: read` and `pull-requests: read`; checkout
credentials are not persisted, PR code is never executed, and no repository secrets or caches
are used. The baseline comes from the merge base, as in the harness strictness allowlist check.
The workflow takes effect after it lands on the base branch; an older base branch must first
receive this tooling. Maintainers can then require the `ratchet` job in branch protection.

### Global tier 1 properties

Every tier 1 run checks each recipient's candidate update stream, including opening packets and
updates outside recorded checkpoints. `./conformance/run` prints property violations and includes
them in `properties` in its JSON result. A violation fails the tier 1 test and the command even
when the Golden Trace signatures still match the baseline.

The checks live in `conformance/support/properties/`, one module per stable property name:

- `buyback_item_integrity` checks that a full sale keeps the Item GUID and that buyback links never
  refer to Items also reachable through equipment, carried slots, bank slots or bag contents. It
  compares consecutive full projections at reducer boundaries; partial-stack sales may clone.
  It does not prove copper conservation or complete wire fidelity.
- `private_fields_stay_private` checks CREATE and VALUES masks against vmangos's 1.12.1
  [update-field flags](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/UpdateFields_1_12_1.h)
  and [recipient rules](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Object.cpp).
  A Character sees its private fields, an item's or pet's owner
  sees owner-only fields, and members of the same Party or Raid see group-only fields. The Module
  does not implement vmangos's Empathy aura or debug target-info cheat, so neither grants special-info
  visibility in tier 1. Padding marked `UF_FLAG_NONE` is never sendable, including to self.
- `client_view_consistency` applies each CREATE and partial VALUES update to the recipient's
  accumulated view. An omitted field keeps its previous value. After a reducer's delivery batch,
  the view must equal a fresh full projection at that moment for every visible object. The
  `conformance_test_fixture` reducer builds that projection with the recipient's CREATE
  visibility rules. This checks incremental updates against the full projection; it does not
  establish that the projection matches 1.12.1. The Golden Trace comparison checks recorded
  moments against the Reference Server. The runner captures every connected recipient after
  each request, connection change and virtual-clock step, including recipients with no packets.
  The hook discovers visible objects from AOI Grants and owned items, so an omitted CREATE or
  entire VALUES packet cannot remove the object from the check. Packets from one reducer form
  one batch because a CREATE and its supplemental VALUES describe one committed state.
  Projection rows are events returned by that reducer on the existing WebSocket. Capture uses
  only that call's rows, while buffering CREATE context across earlier transactions. Each call
  still enumerates AOI Grants and owned items. The fixture compares all codec inputs with the
  previous inputs for that connection and object before rebuilding an encoding. These disposable
  cache keys use derived Debug text plus float bits where update fields carry floats. They are
  equality keys, not hashes or persisted evidence. Entity movement-block inputs are neutralized
  because this property compares update fields. Transport projections remain time-dependent.
  Unchanged rows carry an empty body to reuse the previous decoded fields. Source changes therefore
  trigger a fresh projection even when no packet was delivered. Every delivered update is checked
  against the current projection; objects with neither changed fields nor an update reuse their
  previous comparison. Visibility membership is checked on every batch.
  Clock advancement and the receiving Character's projection share one reducer call. Other connected
  recipients are captured once after that tick. Set `LYRACORE_CONFORMANCE_FULL_PROJECTION=1` on
  `./conformance/run <scenario>` to rebuild, decode and compare every projection for an equivalence
  check. This mode uses the same visibility discovery and batch boundaries.
- `create_kind_matches_freshness` checks kind 3 only when the object was newly added to the world
  while the recipient was viewing it, as vmangos's
  [Map::Add](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Maps/Map.cpp)
  and [Object.cpp](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Object.cpp)
  do. A known object entering view uses kind 2. Characters,
  Transports, elevators and moving Transports use kind 2 as vmangos does. The fixture records
  which connected AOI viewers existed when each object entered the world, then joins that fact
  to each recipient packet id in the same transaction. Later sightings use kind 2. The check
  does not take freshness from the CREATE encoder. The bank fixture also spawns a creature
  after the recorded steps to exercise kind 3 without changing Golden Trace observations.
  It also spawns a door, opens it and closes it so consistency checks exercise an explicit
  zero-valued `GAMEOBJECT_STATE` update.

The projection reducer and its private tables exist only with `conformance_test_fixture`.
`native_debug_gate` checks that a build with only `debug_reducers` exports none of them. To add a
property after a review finds a broken invariant, add one named module, a hand-built violating
stream test, and a scratch reversal of the historical fix that makes a tier 1 run fail. Keep
property failures separate from Golden Trace differences and the Tier 1 baseline.

### Automated harness pin proposals

After the harness CI passes on a relevant `main` push, its workflow tags that commit with the next
`v0.2.0-conformance.N` prerelease. A daily core workflow, also available through
`workflow_dispatch`, compares the newest tag with `.wire-harness-rev`. When the tag is newer, it
updates that file, the workspace `wire-client` revision and `Cargo.lock`. It builds the disposable
Module, runs every tier 1 scenario, runs `./conformance/baseline` without an argument, and reruns
the scenarios against the regenerated baseline. The first run can fail because the committed
baseline still names the old harness revision. The generator requires a completed result for every
catalogue scenario; a setup failure stops the proposal.

The workflow runs comparisons without a credential. It passes the harness revision and a
base64-encoded Tier 1 baseline, capped at 128 KiB before encoding, as job outputs. A separate
runner fetches the tag, validates the baseline and derives its signature summary before using
`HARNESS_PIN_TOKEN` to open or update the single
`ci/auto-harness-pin` PR. Its body shows added and removed rule and data signatures per scenario,
or says when only provenance changed. Signature changes add the `ready-for-human` label. The
workflow never merges the PR. Normal PR checks run on the bot PR.

A scenario has a core tier 1 test when `module/tests/conformance/tier1.rs` defines
`fn <scenario>_matches_its_golden_trace`, the test `./conformance/run` selects, and its Behavior
lists the scenario. If the harness catalogue adds a scenario without one, the workflow names the
missing scenarios in its run summary and opens no PR. The scenario author makes one core PR that advances
the harness pin and `wire-client` revision, updates `Cargo.lock`, adds the test and Feature Ledger
entry, and regenerates the Tier 1 baseline. The separate pin proposal workflow resumes for later
harness revisions that core can already test.

The maintainer must create the core Actions secret `HARNESS_PIN_TOKEN` as a fine-grained personal
access token scoped only to `LyraCoreProject/LyraCore`. Give it repository permissions **Contents:
read and write** and **Pull requests: read and write**. The label endpoint accepts Pull requests:
write. The harness uses its default `GITHUB_TOKEN` with `contents: write` to create tags; it needs
no cross-repository secret. The core token must belong to a principal allowed to open PRs. A PR
created with the default `GITHUB_TOKEN` can leave its PR checks waiting for approval; the PAT lets
them run automatically.

To test the full flow after creating the secret, merge a harness PR that changes an existing
scenario, trace, `src/`, adapter, or Cargo manifest or lockfile. Confirm harness CI creates one new
tag. Run the core `Propose harness pin bump` workflow manually, review its PR body and label, and
confirm that the normal core checks run. Run the workflow again to check that it updates the same
PR without making another. For a new catalogue scenario, confirm the workflow names the missing
core test and opens no PR; then use the combined core PR described above. Merge remains a human
decision.

### Run one scenario

From the repo root, run `./conformance/run taxi_single_hop` or
`./conformance/run taxi_single_hop --json`. The command runs only that scenario's tier 1 durable
test through a disposable Shard. It captures the comparator result in an isolated scratch file,
then prints differences grouped by signature. Each record shows the expected and actual scrubbed
values and the comparator rule's reason. The Behavior's vmangos commit, path and symbol come from
the Feature Ledger. `Golden Trace` reports the comparator's rule result; `Tier 1 baseline` reports
exact signature and provenance equality. The command exits with code 1 when the baseline differs
or a property fails, and code 2 when `cargo` cannot start or the test produces no completed
comparison. In the latter case it prints the test diagnostic. Code 0 means the run completed,
all properties passed, and the Tier 1 baseline matches.
Taxi can exit successfully while its known rule differences remain visible.

`--json` writes one schema version 1 object to stdout. On setup failure it writes an error object
with `scenario`, `error.kind = "setup_failed"`, and `error.diagnostic` from the test output.
Groups sort rule before data, then by signature. Records retain comparator order. The fields are:

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` |
| `scenario`, `behavior` | Catalogue scenario and Feature Ledger Behavior ids |
| `vmangos` | Ledger source object with `commit`, `path` and `symbol` |
| `properties` | Property violations with stable name, recipient, object GUID, field and detail; empty on success |
| `conforms` | True when the comparator reports no rule differences |
| `baseline.matches` | True when the current provenance and rule and data signature sets equal the Tier 1 baseline |
| `baseline.regeneration.rule`, `.data` | Sorted `added` and `removed` signature arrays that regeneration would write |
| `groups[]` | `classification` (`rule` or `data`), `signature` and `records` |
| `groups[].signature` | `rule`, optional receiving `character`, `checkpoint`, `stream`, `opcode`, `field_path` |
| `groups[].records[]` | `packet_index`, rule `reason`, `expected` and `actual` values from the comparator |

Expected and actual values use the harness's tagged field JSON and may be null. The text view
prints the same information, including every signature that regeneration would add or remove.

The runner prepares the scenario's Character through debug reducers, then runs the
harness's `Transport` over the buffered Operator connection. It decodes replies and compares them
with the Golden Trace using the harness library. Direct and AOI delivery follow the Module's
ordered grants, revokes and clears. Transport Loss, an unexpected close, a cipher transition or
unsupported delayed Host delivery fails the test. Scheduled transactions remain observable through
`wait_for_packets`. At one deadline, the virtual clock orders events by table kind and then row id.
SpacetimeDB does not guarantee that order in live play.

The schema-3 runner records world-entry packets in the opening phase. Core applies their delivery
instructions to establish AOI Grants, then retains the packets as opening observations. The harness
excludes opening observations from comparison and starts comparison at the first step. Core never
filters packets during the scenario or changes its Equivalence Rules.

The test writes `.lyracore/conformance/tier1.json` with the core commit, working-tree state, harness
revision, capture time and the harness comparison. It removes the scenario's previous result before
setup, so a setup failure leaves no old pass. Tests in one process update the report under a lock
and preserve other scenarios from the same revision. Both difference classes remain in the report.
The comparator marks a Behavior as nonconforming when it finds rule records. The tier 1 baseline
check fails when either signature set changes or provenance differs, regardless of that result.
No packet text enters this result. The scoreboard consumes it.

## White melee roll rates

`white_melee_outcomes` follows [ADR 0007](../docs/adr/0007-statistical-roll-conformance.md).
Its expected probabilities are independently derived from pinned vmangos attack-table source;
its small Golden Trace checks that derivation in practice. It is a scoped exception to the
recordings-only rate authority, and cannot establish attack cadence or all melee parity.

The Tier 1 collector sends native attack and stop requests, advances ordinary scheduled callbacks
in batches and classifies delivered combat bytes with the harness decoder. It counts only each
Character's own attacker stream. It checks attack start, damage, stop and live context; it does
not run the full update-projection properties over the 200,000-swing stream. Other scenarios keep
those properties. Ordinary `white_melee_distribution` signatures and `known_gaps` retain the
findings. The [measurement report](reports/2026-11-05-white-melee.md) records exact counts, warm
cost, repeatability and strictness. A held category signature cannot detect further worsening
within that category. TCP audits explicitly report a missing full roll sample.

## Direct-damage spell roll rates

`spell_direct_damage_outcomes` extends [ADR 0007](../docs/adr/0007-statistical-roll-conformance.md)
with a level-60 mage casting Scorch at level-60 and level-63 creatures. Complete samples use
50,000 casts per level for each public seed. The first core measurement is blocked: passive
targets still damage the mage and cause cast pushback. Ordinary `spell_sample` findings retain
that gap; every rate remains unmeasured. The [spell report](reports/2026-11-05-spell-rolls.md)
records the context, expected rates and diagnostic evidence. Partial counts cannot claim parity.

## Taxi data

`taxi_single_hop` uses the harness's money, known-flight-node and creature Fixture Verbs. The
Operator transport calls the same debug reducers as the wire adapter. `debug_learn_taxi_node`
requires the Operator, an existing Character and a storage id in `game_taxi_node`. Imported storage
ids equal client node ids. Learning is idempotent and sends no packets or flight requests.

The tier 1 test installs the hand-written fixture in
`conformance/support/taxi_route.rs`. It contains nodes 2 and 4, path 6 with fare 110,
its 28 vertices, three area rows, 91 flat terrain cells, and a minimal synthetic template for the
scenario's flight master. The template
entry comes from the pinned scenario's Creature Fixture Verb, currently 523.
The node and route numbers were checked against build-5875 `TaxiNodes.dbc`, `TaxiPath.dbc` and
`TaxiPathNode.dbc`. Node text is fixture-authored. Mount display 6852 agrees with the Golden Trace;
no mount template is needed.

The base scenario also places fixture creature 990002 at route vertex id 44 (index 9) with aggro range 80
and hostile faction template 14. The fixture installs the numeric relations for faction templates
1 and 14 from the pinned World Source. The creature is outside takeoff and landing range;
its only nearby Character is in flight. The range spans the four-second sensing period at
32 yards per second. The existing Golden Trace therefore still requires
no passenger combat. Observer and reconnect scenarios retain their original sparse worlds.

`route_fixture_matches_every_recorded_route_point_and_mount` checks all 27 serialized spline
destinations exactly against vertices 1 through 27, including their wire precision. The trace
starts at the passenger's position, which matches node 2, and omits DBC vertex 0 from its
destinations. Vertex 0 was checked against the client data. Destination node 4 uses the vmangos
`taxi_nodes` coordinates the importer reads, which match the Reference Server's landing correction.
The test also checks the passenger's mount display.

The area ids, flags and exploration levels come from build-5875 `AreaTable.dbc`. Stormwind City
is 1519 with flags 312 (`0x138`) and exploration level 10. Elwynn Forest is 12 and Westfall is 40;
both have flags 64 (`0x40`) and exploration level 0. All three are on map 0 and use Alliance
faction group 2. The area bits 1, 2 and 3 are fixture values; exploration bit numbering is outside
this scenario. Each terrain cell has zero height and no liquid or holes. These values only let the
Module resolve area and rest state during the flight; they are not client terrain data.

The Golden Trace starts in zone 1519 with `field_190 = 32`. It reports Elwynn world states and
weather at 10,583 ms, then Westfall world states and weather at 47,692 ms. Those changes fall
between route vertices 40 and 41, then 2922 and 2923. The hand-chosen boundary cells are
`(785,498)` for Elwynn and `(812,489)` for Westfall. The spline first enters them at about
10,285 ms and 47,369 ms. The other cells follow the route spline between its recorded vertices.
This is a small route-only area map, not a claim about the full zone borders.

```bash
cargo test -p lyracore-module --test native_conformance taxi_single_hop -- --ignored --nocapture
```

This command needs no imported data or `LYRACORE_TEST_TAXI_SQL`. It starts its own disposable
Shard. Durable CI compares its current rule and data signature sets with the committed tier 1
baseline. Known gaps never excuse a difference or supply a passing result.

`two_character_visibility` uses the same Stormwind area row and flat terrain cell `(777,497)`
as the taxi fixture. Both scenario positions fall in that cell. Its human warrior starting outfit
uses item entries 25, 38, 39, 40 and 2362. The pinned Golden Trace records the four visible
armor entries in each peer CREATE update; the fixture supplies only the item template fields
needed to equip them. Entry 25 already has a template in the disposable Shard. The fixture adds
these rows before creating either Character, so area rest state and visible equipment are present
on both arrival streams. The opposite-faction variant adds the orc warrior outfit: entries 6125,
139, 140 and 12282. Its pinned Golden Trace records those visible item entries and the axe's
2900 ms attack interval. Both outfits use synthetic item templates, so remaining field differences
stay in the measured baseline.

The [T2g comparison](reports/2026-10-27-t2g.md) records the conformance.4 aligned differences
and maps them to the taxi entry's known gaps. The [T2d comparison](reports/2026-10-27-t2d.md)
retains the earlier conformance.3 evidence. Their imported-data setup describes those historical
runs; tier 1 now uses the bundled fixture.

## Scoreboard

From the repo root:

```bash
./conformance/scoreboard
./conformance/scoreboard --json
./conformance/scoreboard --summary
cargo test -p lyracore-conformance --example scoreboard
```

The script runs the Rust example `scoreboard`. It reads the Feature Ledger and prints system totals
and a row for each Behavior. Tier 1, Conformance Audit and real-client checks have separate columns.
It does not infer a pass from a test declaration or claim overall completion from these columns.
The ledger's other required rungs remain obligations.

On each push to `main` outside documentation, `Module durable tests` publishes the `conformance-scoreboard-<commit>`
artifact for 90 days and a short job summary. The artifact contains `scoreboard.json` from
`./conformance/scoreboard --json` and the run's `tier1.json`. CI shards whole test targets, so the
job that runs `native_conformance` already records all tier 1 scenarios in one process. No scenario
rerun or shard merge is needed. Added work is scoreboard compilation, two scoreboard invocations
and artifact upload, with no extra durable test time. CI never commits this evidence. Compare artifacts by
commit to track progress within retention; download them for longer retention.

`--summary` prints the same short summary used by CI. It includes Behaviors with scenarios, passes,
recorded rule and data signature totals for each tier, evidence states, open work by kind and
data coverage.
Low counts do not fail the scoreboard. Invalid inputs still fail. A recorded signature total sums
per-Behavior distinct signatures from the supplied report, including stale evidence; the state
counts show whether those measurements are current. Absent evidence has null signature totals.
A pass total of zero alone does not mean measured failure: check `evidence` and `states` in the
summary. `absent` means no report; `not_run` means a Behavior lacks complete evidence.

The system table counts Behaviors with and without scenarios. The open-work list includes every
Behavior without a scenario except entries with only Data Gaps, and every scenario-backed Behavior
without a clean tier 1 and audit pass, or with behavior gaps. A scenario author takes an entry
without a scenario. Data Gaps get separate `data_import` items, including on mixed Behaviors.
An implementer takes an entry with one when its evidence still points to implementation work. If tier 1 passes, or the
best available rule-signature count is zero and the ledger has no behavior gaps, an entry with a missing,
stale, unverified or reference-unstable audit calls for evidence refresh by a maintainer through
`./conformance/audit`.
Scope verification follows established implementation and scenario work, then data import; evidence
refresh comes last. A data importer takes `data_import` items and fixes World Source translation
or imported representation. Its command locates findings in the importer source paths. Each entry
gives a next command. The scenario-author command opens the ledger entry. An implementer normally
runs the first listed scenario; further scenarios are in the Behavior's `scenarios` field.
Each open-work item also has `reproduce_with`: `tier1` or `audit` for work with a scenario, and null
for scenario author, scope verification and data import work. An implementer whose tier 1 result
passes but whose audit reports differences reproduces them with `./conformance/audit --scenario <id> --check`. An audit-only difference usually also means
the tier 1 fixture misses something; a scenario author can fix that fixture.
Run a tier 1 item's `next_command` from the repo root. Exit code 0 means its signature set still
matches the Tier 1 baseline and all properties passed. Code 1 means a baseline change or property
failure, and 2 means setup failed before comparison.
After an improvement, run `./conformance/baseline <scenario>`, then review the changed
signatures. For an audit item, replace `/path/to/client/Data` in `next_command` and check its
`prerequisites`. Implementer items run one scenario without a report. Evidence refresh items use
the full audit command, which chooses an unused report name; audit history is append-only.

The scope-verification group contains Behaviors marked `stock_client_use_unverified`. Its next
command opens the ledger entry so the request-use question can be checked. Within each group, the
list sorts by descending number of other Behaviors that directly depend on the entry, then
descending measured rule-signature count, then descending number of distinct
claimed request opcodes, then ascending Behavior id. It uses the committed tier 1 baseline's
distinct rule signatures when the Behavior has one, then the audit count, then a local tier 1
count. If none exists, it uses the number of distinct behavior gaps. Data Gaps never increase
scenario or implementer priority.
Data-import items sort by distinct Data Gap count, then Behavior id; dependency and opcode counts
are zero for those items. Repeating a known gap does not
increase priority. Dependency edits change the ranking and receive review as such in their PRs.
The formula uses no assigned priority.

`--json` emits one JSON object and no text. Schema version 1 has `systems` and `behaviors` objects
keyed by their ledger ids, a `summary`, `open_work` in rank order, `opcode_coverage`, `trainable_spells`,
`tier1_evidence` and `audit_evidence`. Evidence is null when absent; otherwise it has `path` and
the complete validated schema-2 `report`, including scenario comparisons and audit summaries.
`summary` gives `behaviors`, `with_scenario`, `without_scenario`, `open_work_by_kind`, and
`data_coverage`, `tier1` and `audit` objects. Each tier gives `evidence` (`absent` or `present`),
`pass`, Behavior
`states` counts, `scenarios_recorded`, `scenarios_expected`, `rule_signatures` and `data_signatures`.
Each system gives `behaviors`, `with_scenario`, `without_scenario`, `tier1_pass`, `audit_pass`,
`real_client_waiting`, `claimed_opcodes` and `unavailable`. Each Behavior gives its `system`,
`scenarios`, `dependencies`, `required_rungs`, `scope_verification`, `tier1`, `audit`, `real_client`, `claimed_opcodes`,
`unavailable` and `known_gaps`. Tier cells have `state`, the text `display`, and a summary when
measured. State values are `no_scenario`, `not_run`, `stale`, `different`, `unverified`,
`unstable_reference`, `property_violations` and `match`. Summaries retain separate rule and data
signatures and raw record counts. Spell counts give build, source path or missing expected path, and per-class
trainable, supported-kind and unsupported-kind counts. Open-work items give `behavior`, `system`,
`kind`, `reproduce_with`, `next_command`, `prerequisites` and the three numeric ranking inputs. `kind` is
`scenario_author`, `implementer`, `scope_verification`, `data_import` or `evidence_refresh`.
The `open_signatures_or_gaps` ranking input
is the measured signature count or the distinct behavior-gap fallback; data-import items use
distinct Data Gaps. Counts and state names have the same
meaning as the text scoreboard.
`opcode_coverage` reports `claimed`, `out_of_scope`, `unclaimed` and `total` for the build-5875
request inventory. `unclaimed_opcodes` lists the missing numbers; it is empty on a successful run.
The existing `outside_enum` field counts Behavior claims outside `ClientOpcodeMessage`, not outside
the build-5875 inventory.

The default tier 1 input is `.lyracore/conformance/tier1.json`. The latest audit is the JSON report
under `conformance/reports/` with the greatest `recorded_at_unix_ms`. Each audit has a Markdown report beside its JSON result. To select inputs explicitly:

```bash
./conformance/scoreboard --tier1 /path/to/tier1.json --audit /path/to/audit.json
```

Missing default inputs show `not run`; Behaviors with no scenario show `no scenario`. A missing
explicit input or malformed report fails the command. A changed core, harness revision
or scenario hash shows `stale`. A descendant core commit that changes only `conformance/reports/`
remains current, so committing the report does not invalidate its measurement. A pass requires
every scenario listed by the Behavior. Any rule difference, including a counted Deliberate
Deviation, or any tier 1 property violation prevents a pass. Rule differences in the Reference
Server comparison also prevent an audit pass. A matching run made with uncommitted changes shows `unverified (dirty tree)` and contributes no pass to system totals.
Real-client cells come only from the ledger, never from a report.

Counts use unique opcode values. The denominator is every row of `client-requests-5875.tsv`.
Claims outside the pinned core `ClientOpcodeMessage` enum have a separate count. System totals
count shared claims once. The unavailable count includes request
patterns in `responses` and `raw_responses` in `module/src/native/unavailable.rs`. It excludes
control receipts, tests, comments, imports and explicit empty responses for ignored requests.
Per-system and per-Behavior unavailable counts intersect those requests with ledger claims.
Neither opcode claims nor these source counts prove conformance.

### Result schema version 2

Both tiers retain the complete harness `Comparison`. Each record requires the comparator's
`classification` tag, `rule` or `data`. There is no separate `data_differences` array. Version 1
results lack tags and must be rerun. Tier 1 results without `properties` must also be rerun.
Summary counts never supply classification. Property violations have their own nonpassing state
and do not enter comparator signature counts.

A signature is `(rule, character, checkpoint, stream, opcode, field_path)`. `character` is absent
for single-Character scenarios. Packet indices, reasons and values do not belong to it. The
scoreboard groups signatures across all scenarios of a Behavior
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
| `scenarios[].properties` | Required for tier 1, absent for audits; array of violations with `property`, `recipient`, `guid`, optional `field`, and `detail`; empty on success |
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
the durable tier-1 check, which recomputes LyraCore's behavior in CI. The taxi comparison runs in
durable CI against its signature baseline. Its rule differences still prevent a passing result.

`audit.previous_report` is the JSON filename of the newest `conformance_audit` report in
`conformance/reports/` with a strictly earlier `recorded_at_unix_ms`, or null when none exists.
Equal earlier timestamps use filename order; the report being checked is never excluded by name.
Each Behavior's
`changes` has `rule` and `data` sets of `fixed` and `regressed` signatures. It is null when no
earlier audit exists or the harness revision or any scenario hash for that Behavior differs. In the
latter case the report says comparison is not possible. Multiplicity and values cannot fix or
regress a signature. The scoreboard derives the predecessor from the reports directory and checks
these sets against it. It also
replays that earlier report's LyraCore candidates when its harness revision matches the current pin.
Older reports receive structure checks; the newest report receives current ledger checks, so a
Behavior rename cannot invalidate old reports. Earlier T2d/T2g narrative notes are not audit baselines.

### Data coverage

`summary.data_coverage` contains ledger gap totals (`behavior` and `data_import`) and `systems`.
Each system contains those same `gaps`, `by_source` counts keyed by relative source path, and
`port_census` reports linked from importer gap evidence. Gap totals count ledger entries; the
ranking deduplicates identical entries. Classification is derived from the path, with no new
handwritten status or changes to ledger files.

Each census reports `planned_import_rows_by_table` from its vmangos production plan and the
archived Compatibility Manifest (`manifest`), including finding codes, classifications and occurrence
counts. `--summary` prints planned rows and totals of finding occurrences by classification; codes remain
in JSON and the census. Reports retain their source identity and profile in the linked
census document. These are archived planned rows, not a measurement of a running Realm or the
current importer. Findings can overlap and count different units from imported rows. Reports can
also appear under several systems; do not sum them across systems. No source-data percentage or
unsupported-row denominator is inferred. Scenario coverage and comparator data signatures remain
separate measurements. Missing census evidence gives an empty report map, not zero unsupported rows.

## Conformance Audit

Before a full audit, rehearse every catalogue scenario on both servers:

```bash
./conformance/audit --rehearse --client-data /path/to/client/Data
```

Rehearsal uses the full audit's catalogue order and capture path, restoring both servers before
each scenario. It requires an imported-world snapshot that matches both source tiers and never starts or falls
back to a fresh world import. If none exists, run one scenario with `--check --strict-world` to
seal a matching snapshot first. Rehearsal
prints one JSON line per completed scenario with setup success, wall time, and rule/data difference
record counts for both servers. Differences are data and do not stop the remaining captures.
Setup, capture or comparison execution failures exit 2; reaching the end exits 0. It writes no audit
report, accepts neither `--date` nor `--output`, and stops both servers on exit. Scrubbed captures
and comparisons remain in the printed diagnostics directory, outside the repository.

Run one scenario while climbing a Behavior:

```bash
./conformance/audit --scenario taxi_single_hop --check --client-data /path/to/client/Data
./conformance/audit --scenario taxi_single_hop --check --json --client-data /path/to/client/Data
./conformance/audit --scenario taxi_single_hop --check --agreement --json --client-data /path/to/client/Data
```

Catalogue fixture creatures can lie outside the `alliance-single` slice. A local `--check`
prepares a catalogue snapshot from the cached base world with the existing importer's
`--include-creatures` option. It loads World Source rows and client catalogues for the required
creature and vendor content; terrain, navigation and collision data are retained. This is a warm
preparation, without a new full world import. The catalogue key covers the base key, required
creature entries and the preparation code. The store keeps this snapshot together with its base.
Rehearsal can prepare this fixture content from a matching cached base; it never starts a fresh
world import or regenerates terrain, navigation or collision data. Full audits build the catalogue
snapshot from their newly imported base world, even when an older catalogue snapshot exists.

The local check captures the scenario on both servers and compares each candidate with its Golden
Trace. It prints the same grouped text or JSON fields as `./conformance/run`. Its `baseline` field
compares rule and data signatures with the newest committed full audit report. Exit 0 means those
signatures match and the Reference Server matches its Golden Trace. Exit 1 means signatures differ.
Exit 2 means setup or capture failed; diagnostics name missing client data, Docker, occupied ports or
a dirty core tree. The scenario check writes no report or committed trace.
If the latest full report has no entry for the scenario or has an older scenario revision, the
check compares against an empty signature set and exits 1.

`--agreement` also runs the scenario in tier 1. Its JSON output has `audit` with the normal
scenario comparison and `agreement` with `tier1_only` and `audit_only` signature arrays. A
`tier1_only` signature points to a likely fixture artefact. An `audit_only` signature points to a
fixture blind spot. The command exits 1 when either array is nonempty or either tier's baseline
differs. A tier 1 setup failure exits 2 with its diagnostic.

The first check creates a disposable Realm worktree and imports the pinned vmangos world. It seals
the imported Realm at `$HOME/.cache/lyracore/conformance-worlds/<key>/`. The key covers Tier A of the
import contract below, the vmangos database pin, the client Data bytes, the CLI pin and the snapshot
format version. `--strict-world` also requires matching Tier B sources. The seal records the full Module schema extracted from the deployment WASM.
Tier B gameplay edits reuse the snapshot with drift reported in an ordinary local check.

A check can also reuse a snapshot after adding ordinary private tables. Every existing table,
default, access flag, event flag and named type must remain unchanged. Existing reducer definitions
and explicit names must remain present. Schedules, outboxes, lifecycle declarations and other schema
sections must remain unchanged. New public or event tables and changes to existing table columns,
including defaulted end-appended columns, take the cold path. A successful SpacetimeDB migration
alone does not establish that a column default needs no backfill; see
[danger zones](../docs/danger-zones.md#1-do-not-touch-traps).

Checks restore the stopped snapshot with its original Module, then `dev up` publishes the current
Module through SpacetimeDB's normal migration check. The pinned CLI calls
`debug_repair_after_publish`; `init` does not run again. Before capture, the runner verifies the
published schema against the deployment WASM and checks the imported revision. A failed publish,
initialization or schema verification stops the restored Realm and prints the diagnostic before
falling back to a fresh import. A failed fresh start fails the check. `--require-snapshot` forbids
both a missing or incompatible snapshot and the cold fallback after an upgrade failure.

The check output includes `world_origin`: `fresh_import`, `snapshot`, `upgraded_snapshot`,
`snapshot_gameplay_drift` or `upgraded_snapshot_gameplay_drift`. Text
output prints the same value as `World origin`. Agreement JSON carries it inside `audit`. This is
local check evidence only. Full audits always import a fresh world in that invocation, seal it,
and restore that new snapshot before every scenario. They never reuse an earlier invocation's
import. Equivalence between upgraded and freshly imported worlds is not claimed.

Snapshots from the older schema-keyed format take the cold path once. The store retains the newest
two snapshots and removes older ones after a successful seal. A catalogue snapshot retains its base
world as the second snapshot, so fixture changes cannot evict the expensive base import. A cold import that replaces an
incompatible snapshot keeps the old seal until the new import is sealed. Abandoned `.pending`
directories are removed under the shared snapshot-store lock. Copying requires the world size plus
10% or 256 MiB of free space, whichever reserve is larger. A failed seal retains the stopped imported
world and its password. The next check at the same core commit retries sealing without importing
again. An interrupted import requires inspection before cleanup. Do not edit a sealed snapshot.

Each check owns its scratch directory before building deployment artifacts; a concurrent check
using that directory fails before it can replace them. Each check restores the imported data before
capture. The active Realm, target directory and temporary files live under `$HOME/.cache/lyracore-scratch/m5c/`,
or `LYRACORE_AUDIT_SCRATCH` when set. The runner waits for its Native Host to exit before replacing or deleting data. It records
process ownership before startup and checks the CLI's PID identities before stopping it. It retains
those records across a shutdown timeout, even when the CLI clears its own records. Native Host build
output survives gameplay checkout changes; `dev up` still checks the current Host source and
patches. Tier 1 agreement builds use a separate persistent Cargo target directory at
`scenario-audit/tier1-target/` under the audit scratch directory. This keeps their WASM feature set
from replacing deployment build output between checks.

The Reference Server marker records a unique Docker Compose project and container IDs before any
service starts. The clean baseline comes from the pinned, checksum-verified database archive in a
new MariaDB volume. Preparation creates and verifies the two initial Accounts without starting world
timers, then seals a dump with its checksum and password hashes. An existing prepared volume never
supplies this baseline. Every owned check restores it before starting the world. Ownership lasts
through reference comparison and provenance checks. Any failure leaves the marker invalid and stops
the owned containers. Recovery keeps the same container IDs and can retry an interrupted restore.
Changed IDs stop the command before it touches the containers. `./conformance/audit --stop` restores
the Reference Server, stops owned services and removes the active worktree; it retains the sealed
snapshots and build output. The full audit uses the same Reference Server recovery and stops it on
exit. Full audits, rehearsals and local checks use the same per-scenario restore path. This resets
Character names across Accounts, Account state, money, quests, items, reputation, fixture creatures,
spawned objects, vendor stock and restock timers. Required LyraCore Accounts are provisioned after
every restore with the sealed password, so a larger scenario catalogue needs no new import.

Phase timings go to stderr, leaving JSON on stdout. They include builds, schema extraction, client
hashing, Realm startup, import, snapshot sealing, both captures and comparison. Agreement also
reports its tier 1 time. The runner builds one deployment WASM and uses that file for schema
extraction, preflight bindings and publication. With at least four available CPUs, both server
restores overlap, then both wire captures run concurrently. A warm check prints
`import: 0.00s (cached snapshot)` and identifies its world origin.

A failed reference comparison prints its scrubbed differences, including signatures and expected and
actual values, before exit 2. Both comparisons and captures survive work-directory cleanup under
`$HOME/.cache/lyracore/conformance-worlds/diagnostics/`; stderr prints the exact directory. These
diagnostics do not count as a completed audit. Password and Operator token files stay private to the
Realm and snapshot directories.

### Import contract

This is a base-world audit. It imports the vmangos world and client data; it applies no Package
Deltas. The snapshot has two source tiers:

- **Tier A**, `IMPORT_CONTRACT` in `audit.py`, changes the snapshot key and requires a cold import.
  It covers importer sources, scripts and manifests; shared, Package Delta and protocol crates;
  workspace dependencies and toolchain; Module build inputs; initialization, import reducers,
  row-building helpers and the table definitions they write. The shared
  `native-host/src/motion_opcodes.rs` include also stays in Tier A. Data inputs include the Runtime
  Script event catalogue used by `module/build.rs` and the generated event names included by shared
  code. Module paths are explicit files, except for the
  Package Import helpers, whose complete directory stays in Tier A.
- **Tier B** is every remaining Rust source under `module/src`. It includes gameplay reached by
  initialization and is deliberately a superset of that call graph. A local `--check` reuses the
  snapshot after Tier B changes, reports `snapshot_gameplay_drift` and lists every changed, added or
  removed file in `changed_gameplay_files`. Text output prints the same list. An additive schema
  upgrade with gameplay drift reports `upgraded_snapshot_gameplay_drift`.

This distinction preserves the world-building callbacks. `seed::init` calls the production, map-0
demo, spell-registry and scheduler seed functions. The demo uses
`creatures::spawn::build_creature_entity` and `insert_creature_entity`, then `WorldEntity::insert`
in `world/simulation.rs`, AOI and the creature-spawn hook in `creatures/eventai/edges.rs`. That hook
can reach the EventAI engine, movement and combat. `seed::seed_production_core` calls
`spellbook::grant_createinfo_spells`, which also leads to Native Host spell notices. The importer
calls terrain, navigation, vmap, GameObject, creature-spawn and EventAI import reducers; spawn
replacement reaches `creatures/tick/lifecycle.rs` and gameplay cleanup.
`importer/scripts/import-world.sh` then calls `debug_repair_after_publish`, `restore_taxi_fixture`
and `arm_all_pools`. Repair seeds catalogues and schedules and repairs legacy rows. These calls
remain intact. Tier B changes mean the restored world can differ from a fresh import.

Use `--strict-world` with `--scenario ID --check` to require matching Tier B fingerprints too. A
Tier B change then requires a cold import; `--require-snapshot` refuses it. Strict mode can reuse an
ordinary snapshot when both tiers match. A fresh strict import updates that same snapshot. Full
audits always import fresh, regardless of either tier, and remain required evidence for the freshly
built world. Rehearsal requires both tiers to match and never imports; use a strict check to refresh
a drifted base world before rehearsal.

The seal records a per-file Tier B manifest from the import. A catalogue snapshot inherits its base
world's manifest, so preparing fixtures cannot erase earlier gameplay drift. Snapshots without this
provenance take the cold path once. `world_origin: snapshot` means both source tiers match; it does
not turn a local check into a full audit.

Mixed files stay in Tier A in full: `seed.rs`, `seed/fixtures.rs`, `operations.rs`, `config.rs`,
`auth.rs`, `operator_content/spells.rs`, `auction.rs`, `mail_reward.rs`, `stats.rs`, `gossip.rs`,
`transport/mod.rs`, `helpers.rs`, `time.rs`, `world.rs`, `world/{simulation,placement}.rs`,
`character.rs`, `instance.rs`, `gameobject.rs`, `go_collider.rs`, `go_model.rs`, `region.rs`,
`terrain.rs`, `nav.rs`, `vmap.rs`, `faction.rs`, `graveyard.rs`, `breath.rs`, `gc.rs`,
`combat/engage.rs`, `skill.rs`, `skilldata.rs`, `action_bar.rs`, `xp.rs`, `talent.rs`, `weather.rs`,
`duel.rs`, `motion.rs`, `gw.rs`, `quest.rs`, `areatrigger.rs`, `trainer.rs`, `meeting_stone/mod.rs`, `loot/mod.rs`,
`items/{tables,properties,temporary_enchantment}.rs`,
`creatures/{catalogue,spawn,pet_care,population}.rs`, `creatures/tick/mod.rs`,
`creatures/eventai/{fixtures,loader,model,stored,tables,relay,presentation}.rs`, and
`spell/{tables,spellbook,stacking,proc,chance_on_hit}.rs`. Their gameplay edits still require a cold
import.

Compared with the earlier directory contract, `module/src/debug` moves to Tier B. The complete
`creatures`, `spell`, `items`, `loot` and `meeting_stone` directories become the explicit Tier A
files above and Tier B for their remaining files. `quest/fixtures.rs` also moves to Tier B. In particular, `creatures/ai.rs`, `combat/*`
except `engage.rs`, all `native/*`, and spell casting, effects, control, scheduling, targeting and
outcome code now stay warm in ordinary checks, with drift reported. There is no production source
split.

The Architecture Test in `tests/import_contract.rs` extends the old child-module coverage check. It
parses the entire Module, including external `#[path]` children, rather than trying to infer which
dynamic hook can run. Every compiled source must belong to at least one tier; table definitions
named by importer SQL and Package Delta code must be in Tier A. Literal `include!`, `include_str!`
and `include_bytes!` inputs must belong to Tier A; literal source includes are followed recursively.
`CARGO_MANIFEST_DIR` inputs resolve at the crate root; generated `OUT_DIR` source is covered by
the strict build script and its inputs. Unknown computed inputs require review. It also checks the
build script and proves
coverage fails when a required seed, import or gameplay file is removed from both tiers. Comments
and test-only modules do not create dependencies. Review the Tier A classification when an import or
initialization helper gains a dependency; put new data files in Tier A. Adding importer code or
changing table data still changes Tier A even if the compiled schema is compatible. Empty new tables
do not supply imported rows.

Prepare the pinned Reference Server once with the harness's
[setup instructions](https://github.com/LyraCoreProject/wire-harness/blob/f58d550eb316557b64290f61da26339aa9e09ceb/adapters/vmangos/README.md).
Stop it before a full audit. The full runner requires a clean core working tree and free Reference Server
ports 13724 and 18085.
Pass the client Data path directly, or set it with `./lyracore config set client-data PATH`.
The audit wrapper enters the Docker group through a `newgrp docker` heredoc:

```bash
./conformance/audit --date 2026-10-28-c5 --client-data /path/to/client/Data
./conformance/scoreboard
```

The Python command creates a disposable Git worktree under
`$HOME/.cache/lyracore-scratch/m5c/` and manages its own Realm through `lyracore dev`.
It refuses a changed CLI pin until the isolation contract has been reviewed. The worktree owns
its `.lyracore/native-data`, JWT keys and Operator token; the audit stops its Native Host and
deletes the worktree after capture. Its loopback ports 3000, 3724 and 8085 must be free.
The Rust `audit` example exports catalogue scenarios, compares candidates through the pinned library, and
writes the reports. It shares counting and evidence validation with the scoreboard. Neither core
command reads scenarios or Golden Traces from a harness checkout. Cargo metadata locates the
pinned checkout only to build `vanilla-wire` and run its server adapters.

The command builds `vanilla-wire` and the audit example. It starts an owned local Realm with
`lyracore dev up`, runs `lyracore import --accept` using the supplied client path,
and provisions enough disposable Accounts for the largest named-Character scenario. The import
selects the current vmangos `db_latest` snapshot
under `.import/vmangos/` and loads the `alliance-single` world profile. It uses normal SRP login
and encrypted world TCP sessions. Fixture world-entry bypasses are not compiled into this Module.
The audit starts with an empty Shard and retains imported creature spawns. The taxi scenario uses
flight-master entry 523 because an imported entry 352 in the same cell could be mistaken for its
fixture spawn during setup. Tier 1 still uses the bundled route
fixture.

All Docker commands run inside a `newgrp docker` heredoc. The command checks the prepared runtime's
pins and provenance, restores the pristine Reference Server database, starts its compose services,
and prepares peer Accounts before each scenario. Every scenario produces both a
LyraCore and vmangos candidate in scratch. Both
compare with their embedded Golden Trace; the Reference Server must also retain its recorded
provenance. Reference rule and data counts remain visible even when LyraCore fails.

The command writes `conformance/reports/<date>.md`, `<date>.json` and `<date>.traces/`. Audit
history is append-only: each run needs a new date or suffix, such as `2026-10-28-c6`, and the
command refuses existing outputs. The trace directory contains only LyraCore candidates. Reference
Server candidates remain in local diagnostics; the report keeps their SHA-256 and reference-stability
results. `--output DIRECTORY` selects another report destination. Rule differences are audit findings and do not make the command fail; missing
captures, infrastructure errors and invalid evidence do. A failed run cannot write a partial
success report. No scenario is skipped, including taxi.
Real-client fields, Equivalence Rules and Deliberate Deviations are untouched.

The full audit runner stops both servers on exit, including capture failures and normal interrupts.
Credentials are removed with the active Realm. Scrubbed captures and crash diagnostics remain under
`$HOME/.cache/lyracore/conformance-worlds/diagnostics/`. Build output uses
`scenario-audit/target/` under the audit scratch directory. The prepared Reference Server's runtime
stays under `$HOME/.cache/lyracore-scratch/reference-server/`. Reports contain only provenance,
counts, signatures and scrubbed difference values. Committed LyraCore candidate traces contain
numbers and text digests, with the same scrubbed field types as Golden Traces. Never commit a
Reference Server candidate; Golden Traces belong in the harness. Commit the implementation before
capturing a report for current evidence, then commit only the report and its candidates. Dirty
captures remain unverified.

## Opcode coverage

`tests/opcode_coverage.rs` checks that every row of `client-requests-5875.tsv` is claimed by a
ledger Behavior or listed in `opcodes-out-of-scope.toml`, never both. It also checks every variant in
the pinned `ClientOpcodeMessage` enum against the inventory. Unknown names and overlapping
categories fail. Each out-of-scope entry maps an opcode name to a reason and a `source` that names
an existing `docs/adr/*.md` mentioning the opcode by name, or
`https://github.com/vmangos/core/blob/<SHA>/src/<path>` with the same `source_commit` as the pinned
Golden Traces.
The scanner uses `cargo metadata --locked`, which can download missing workspace dependency
sources on a fresh checkout. It never changes the dependency lockfile.

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

## Quest data

Tier 1 uses `support/quest_world.rs` for quests 7, 15 and 433. The numeric quest templates,
kill objectives and creature 1977's start and end relations were checked in
the local vmangos world snapshot at database commit
`464179081673cfd240f7ffe7f0daf96bca0b5a70`, the source used by the harness quest recordings.
Quests 7 and 15 require ten kills of creatures 6 and 257. Quest 433 requires ten kills of
creature 1117 and rewards 300 copper, 660 base XP and 75 reputation with factions 47 and 54.
It has no item or spell rewards. See the pinned harness `docs/quests.md` for the recorded scope.

Quest and creature text is fixture-authored, because Reference Server text stays out of the
repository, so tier 1 records text digest differences that the imported audit does not have.
The fixture preserves creature 1977's display, faction, level and quest-giver flag.
Its health and combat values are inert placeholders: the scenarios scope out creatures and
perform no combat. Quest grants bypass prerequisite eligibility and preserve fixture order.
These are approved tier 1 fixtures, not imported world data or gameplay changes.

## Transaction and spell data

Tier 1 uses `support/transaction_world.rs`, `support/character_world.rs` and
`support/spell_world.rs` for the five scenarios added by harness PRs #46–#50. Numeric values
come from the read-only SQLite snapshot at database commit
`464179081673cfd240f7ffe7f0daf96bca0b5a70`. All text is fixture-authored.
The fixtures preserve the ordered vendor stock, item prices, displays, buy counts and durability,
starting outfits, level stats and quest 2158's choice rewards, 110 XP and faction 72 reward 25.
Spell 5504 costs 60 mana and takes 3000 ms to create two items 5350.

The current vendor table cannot store restock seconds, offer flags or condition links.
Vendor 66 item 6270 restocks after 7200 seconds. Vendor 9499 item 11325 has stock 10 and
restocks after 60 seconds; item 15759 has stock 1, restocks after 3600 seconds and has offer
flags 3. Vendor 11188 items 13480, 6149 and 3928 have stock 1 and restock after 7200 seconds;
item 13480 has offer flags 3. Item 14526 uses condition 4000.
Condition 4000 is content-patch equality with zero; patch 10
must hide its offer. `Player::IsVendorItemVisible` at pinned source `d9173eca` checks that
condition through `CONDITION_WOW_PATCH` in `src/game/Conditions.cpp`. The fixture omits that
patch-hidden offer to project patch 10 membership, since `game_npc_vendor` cannot retain a
condition link. This does not implement vendor conditions in gameplay; that gap remains in the ledger.
Quest 2158's flags 8 are seeded through `game_quest_flags`.
Creature health and combat values are inert placeholders; these scenarios do no creature combat.
The baselines record differences from unsupported data and gameplay without weakening comparison.
