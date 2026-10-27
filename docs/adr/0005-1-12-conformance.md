# 0005. Measure 1.12.1 results with independent recordings

Accepted design, 2026-10-27. Implementation proceeds in milestones.

LyraCore needs evidence that its systems behave like 1.12.1. A test written by the implementer can
repeat the implementation's mistake. We compare client-visible results with recordings from vmangos,
using a separate scenario author and independent decoders.

## Decisions

1. Feature completeness covers every 1.12 system in core. Dungeon and raid encounter content belongs
   to the official dungeons Package and has its own scoreboard.
2. vmangos is the Reference Server. Its observable results define 1.12 for these checks. A result
   LyraCore keeps different on purpose needs a Deliberate Deviation with a reason and a source.
   cmangos may help settle a dispute, but recordings come only from vmangos.
3. Compare client packets and game outcomes. LyraCore may use transactional, single-Shard,
   data-driven implementations and different scaling or authoring choices. An implementation
   difference with the same result is not a Deliberate Deviation.
4. Compare decoded packet fields under Equivalence Rules. GUIDs use a consistent bijection per
   trace. Times have a tolerance. Replies to one request retain their order; unrelated broadcasts
   may interleave. Random-roll fields must have the required shape and range. Every other permitted
   difference needs a named rule and reason. General rules belong in the harness repository;
   LyraCore-only differences belong in its Deliberate Deviations.
5. A Behavior is the unit of the Feature Ledger. Systems group Behaviors on the scoreboard. Keep one
   TOML file per Behavior at `conformance/features/<system>/<behavior>.toml`. Fold the nine curated
   opcode-inventory entries into those files and make the inventory read them.
6. A ledger entry holds the 1.12 scope, opcodes, dependencies on other Behaviors, a vmangos source
   path at a pinned SHA, required verification rungs, scenario ids, Deliberate Deviations and a
   human-maintained `real_client` field. Tools derive status; entries have no handwritten status.
7. Only the recorder changes Golden Traces. It records a scenario against the Reference Server.
   Traces live in the harness repository, so core implementation PRs cannot edit them.
8. Before writing a trace, the recorder replaces every text field, including names, quest text,
   gossip and chat, with its SHA-256 digest. The comparator compares digests. Numbers, ids, flags
   and positions remain unchanged. Never commit packet text, vmangos or client files, or bulk
   world data.
9. Scenarios are data in the harness repository. Each names its Behavior, Fixture Verbs, typed
   client packets and checkpoints. The harness chose TOML because packet fields and ordered steps
   remain readable in diffs and it matches the Feature Ledger's format. Its
   [scenario format](https://github.com/LyraCoreProject/wire-harness/blob/c24ebb3c69613c7d442cc737f199e7a1f181da52/docs/scenarios.md)
   rejects unknown fields so typos cannot silently change a recording.
10. Fixture Verbs describe setup: a Character's race, class, level and position; items, money,
    spells and flight nodes to grant; a creature or gameobject at an offset; a quest in the log.
    Add a verb only for a scenario that needs it. Both adapters must implement it before use.
    LyraCore uses debug reducers. vmangos uses a GM Account and GM commands such as `.levelup`,
    `.additem`, `.learn`, `.npc add` and `.tele`.
11. Scenarios spawn their fixtures and assert rules. Audit reports separate rule differences from
    data differences. Only rule differences count against conformance. A data difference must match
    a typed data-dependent field in the Feature Ledger, with a pinned source and reason. The
    scoreboard re-derives classification and rejects reports it cannot classify from those
    declarations. Counted Deliberate Deviations remain rule differences.
12. The harness is a Rust library as well as a Headless Client. Its scenario model, independent
    decoders and comparator serve the tier 1 runner through a git dev-dependency. Pin it to the
    same SHA as `.wire-harness-rev`. The harness never depends on a LyraCore crate or shares a
    decoder with core. Do not use `benilla-protocol` to decode tier 1 results.
13. Run the Reference Server in Docker outside all repositories. Use vmangos's prebuilt
    `dev-linux-amd64` binaries and MariaDB with the `db_latest` MySQL dump. Extract maps, vmaps and
    mmaps from the maintainer's own client with vmangos's extractors. Keep runtime files under
    `$HOME/.cache/lyracore-scratch/reference-server/`. The harness owns only the compose setup and
    setup script at `adapters/vmangos/`. Each trace header records the binary release and database
    commit. Loading SQL into that MariaDB is permitted; [ADR 0003](0003-parse-world-data-never-execute.md)
    governs LyraCore's importer, not the Reference Server.
14. A core script or test reads the Feature Ledger, latest tier 1 results and latest audit report.
    It prints each system and Behavior, claimed client opcodes out of all `ClientOpcodeMessage`
    variants, supported trainable spells per class from vmangos `spell_template`, open entries in
    `module/src/native/unavailable.rs`, and a separate column for pending real-client checks.
    Spell support depends on the Module's effect and aura kinds. People record real-client checks
    through `verify-lyracore`; an audit never changes them. Keep this out of the `lyracore` CLI.
15. `native-host/check.py` retains Host and transport checks: TCP, Login Queue, crash recovery,
    slow readers and World Session replacement. Move each gameplay scenario into conformance as
    its Behavior enters the ledger, then remove that check from `check.py`. Do not move them all
    as part of this work.
16. For later Behavior implementation, one author writes and records the scenario in a harness PR.
    A different implementer makes tier 1 pass in core. Nobody performs both roles for one Behavior.
    The orchestration instructions must retain this separation.

## Two verification tiers

Tier 1 runs on every core change in CI. It drives the Module through native reducers and compares
emitted packets with Golden Traces. It bypasses WoW TCP, SRP and the header cipher. The initial
mechanism test must prove that an Operator WebSocket connection can enter the world through a
feature-gated debug reducer, call `native_world_receive`, and subscribe to `game_native_packet`.
The tier 1 runner compares `connection_identity` and `taxi_single_hop` with their Golden Traces.
CI explicitly skips `taxi_single_hop_matches_its_golden_trace` in `module-durable.yml`, with a
pointer to `conformance/features/taxi/taxi_single_hop.toml`. It needs imported route data and has
open rule differences. A manual run still fails with the full comparator report. Keep no
known-differences baseline; the other conformance checks continue to run in durable CI.

Tier 2 is the manual Conformance Audit. The harness drives a complete local LyraCore Realm with
its Native Host and imported world, and the Reference Server, through the same wire scenarios.
Run it before feature batches and at milestones. It writes a dated report and never gates CI.

The harness repository owns scenarios, Fixture Verbs, Golden Traces, general Equivalence Rules,
the comparator and Reference Server adapter. Core's `conformance/` owns the Feature Ledger,
Deliberate Deviations, reports and scoreboard. The CLI's `dev smoke` entry through
`adapters/lyracore/wire.sh` and `vanilla-wire` remains intact while the harness adapter changes to
the Native Host.

## Milestone 1 implementation record

The Module currently requires the Operator and a connection id in
[`native/mod.rs`](../../module/src/native/mod.rs). `native_world_open` starts in `AwaitingProof`;
`game_native_packet` is a private event table. The durable fixture now offers a synchronous Operator WebSocket
connection in `lyracore-test-support`. It uses SpacetimeDB 2.7.1's JSON protocol. A CLI subscription
cannot submit reducer calls on its existing connection, and the SDK would require generated
Module bindings. Each call returns its own subscribed inserts. The connection also buffers every
subscribed `game_native_packet` insert across transactions, including scheduled reducers and other
callers. `drain_packets` takes the received rows; `wait_for_packets` reads until a batch arrives or
its timeout expires. Tier 1 must interpret the delivery audiences, including AOI instructions,
to reconstruct each Character's packet stream.

Connection ids decode directly as `u128`; row text stays as raw JSON for typed decoding by callers.
This avoids `serde_json`'s `arbitrary_precision` feature, which would change number representation
and generic deserialization in other workspace crates through Cargo feature unification.

`debug_native_enter_world` exists only with `conformance_test_fixture`, which implies
`debug_reducers`. `lyracore publish` enables only `debug_reducers`, so it does not export this
reducer or its private connection table. The durable fixture enables the extra feature explicitly.
The reducer requires the Operator and an open connection in `AwaitingProof`. It records the
Account and owner for that connection, then enters the world without writing a Session, Account
identity or cipher key. Requests on that fixture connection bypass the Session proof Gate but still
check the Account and its ban state. Closing the connection removes its fixture record. Network
authentication continues to require the real Session and proof, even in a fixture build.

The Architecture Test `native_debug_gate` checks the separate feature gate and detects its removal
or replacement with a weaker condition. Its durable test builds with only `debug_reducers` and
checks that the published Module does not export the fixture reducer.

The durable `native_conformance` target checks non-Operator refusal, preserved login credentials,
refusal of the former fixture key and delivery from a later transaction. Its ping decoder now uses
the harness library. Run it with the command in [testing](../testing.md#conformance).

C1c adds schema version 1 and a shared loader in `lyracore-conformance`. TOML keeps the ledger
readable without a generator and gives each Behavior its own file for parallel work. The loader
checks field types, ids and file paths, source references, required rungs, dependency cycles and
human verification records. The [schema reference](../../conformance/README.md) states the fields.

The nine original inventory entries now live in `conformance/features/`. The inventory reads that
same loader and still checks retained Rust test declarations without running them. Entries contain
no handwritten support status. The nine migrated entries have no scenarios yet; source evidence
does not stand in for recordings.

## Milestone 2 tier 1

Core pins `wire-client` to `v0.2.0-conformance.3`, commit
`c24ebb3c69613c7d442cc737f199e7a1f181da52`, through a git dev-dependency at the full SHA in
`.wire-harness-rev`. An Architecture Test checks agreement and forbids normal or build dependencies
on `wire-client` in every workspace member. The ledger reads only
`wire_client::catalogue`; every scenario must exist and name the Behavior that lists it.

The core `Transport` prepares a Character with debug reducers and uses the buffered Operator
connection. It handles delivery audiences before passing plaintext packets to `runner::run_on`.
The harness owns decoding and comparison. Core passes no Equivalence Rule overrides.
`connection_identity` and `taxi_single_hop` have ledger entries and durable Golden Trace comparisons.

At this pin `FixtureAdapter::prepare` returns a TCP `WireClient`, so core uses `run_on` with its
own `Transport`. The schema-3 runner records login packets in the opening phase. Core applies their
delivery instructions to establish AOI Grants and retains the packets as opening observations.
The harness excludes opening observations from comparison and begins comparison at the first step.
All packets received during scenario steps reach the comparator. See the [runner contract](../../conformance/README.md#scenario-catalogue-and-tier-1)
for failure handling and the local result file.

T2c adds `conformance/scoreboard`. It joins the ledger with local tier 1 evidence and the latest
JSON audit report, keeping real-client checks in their own column. It derives opcode and unavailable
request counts from core's dependency source and `unavailable.rs`. Missing, stale and malformed
evidence cannot become a pass. A matching dirty-tree run is unverified and adds no system pass.
Audit data classifications require exact scalar field locations and matching value types in the
ledger. Packet structure, ordering, missing fields, timing and identity differences remain rules.
[The scoreboard contract](../../conformance/README.md#scoreboard)
defines the counts and the report format for the Milestone 3 audit producer.
