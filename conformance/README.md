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

All fields below are required. Use an empty array when no entry exists yet. All nested tables
refuse unknown fields too. Paths are relative, without parent-directory components.

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
| `real_client` | A human-maintained check, described below |
| `prerequisites` | Fixture or world-data requirements in plain words |
| `tests` | Retained Rust evidence, each with a source `path`, test `function` and `rung` |

Ids for Behaviors, systems and deviations start with a lowercase ASCII letter and contain only
lowercase letters, digits and underscores. Scenario ids have no prescribed naming syntax. Empty
and duplicate scenario ids fail. Opcode strings use `0x` and four uppercase hexadecimal digits.
A Behavior must claim at least one request or reply. Claims need not be exclusive across Behaviors.

The rungs are `unit`, `codec`, `durable_module`, `tier1`, `conformance_audit` and `real_client`.
A required rung is an obligation, not a result. The `tests` array names Rust source evidence from
the first three rungs. Harness scenarios supply tier 1 and audit evidence. Tools will join those
results to the ledger in later milestones.

Until a person checks the Behavior in a build-5875 client, write:

```toml
[real_client]
status = "unverified"
```

After that check, use `status = "verified"` with a valid `date` in `YYYY-MM-DD` form, `checked_by`
and `evidence`. The last field names the recorded observations. Audits never update this table.
No other handwritten status is accepted. A missing implementation is a gap, not a Deliberate
Deviation; that field records a result LyraCore has chosen to keep different.

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
that both consumers inherit the same revision as `dev smoke`.

Every scenario reference must exist in that catalogue and name the Behavior that lists it.
A missing id, mismatched Behavior or invalid Golden Trace fails the ledger check. No local harness
checkout is read and no catalogue check can skip.

Run tier 1 with the disposable Shard commands in [testing](../docs/testing.md#conformance).
`connection_identity` prepares the scenario's Character through debug reducers, then runs the
harness's `Transport` over the buffered Operator connection. It decodes replies and compares them
with the Golden Trace using the harness library. Direct and AOI delivery follow the Module's
ordered grants, revokes and clears. Transport Loss, an unexpected close, a cipher transition or
unsupported delayed Host delivery fails the test. Scheduled transactions remain observable through
`wait_for_packets`.

The pinned v2 runner starts after `prepare`. Core consumes setup delivery instructions to establish
AOI Grants before the first request. It does not compare setup packets. This revision cannot record
an opening phase. T2d must adopt the harness's opening format so login packets become recorded
observations outside comparison. Core never filters packets during the scenario or changes its
Equivalence Rules.

The test writes `.lyracore/conformance/tier1.json` with the core commit, working-tree state, harness
revision, capture time and the harness comparison. It removes the previous result before starting,
so a setup failure leaves no old pass. Differences remain in the report and fail the test. No
packet text enters this result. The scoreboard consumes it in T2c.
