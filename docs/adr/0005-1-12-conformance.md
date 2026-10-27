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
   and positions remain unchanged. Never commit an unscrubbed recording or client or world data.
9. Scenarios are data in the harness repository. Each names its Behavior, Fixture Verbs, typed
   client packets and checkpoints. The harness milestone chooses TOML or RON and records its
   reason here when that format is available.
10. Fixture Verbs describe setup: a Character's race, class, level and position; items, money,
    spells and flight nodes to grant; a creature or gameobject at an offset; a quest in the log.
    Add a verb only for a scenario that needs it. Both adapters must implement it before use.
    LyraCore uses debug reducers. vmangos uses a GM Account and GM commands such as `.levelup`,
    `.additem`, `.learn`, `.npc add` and `.tele`.
11. Scenarios spawn their fixtures and assert rules. Audit reports separate rule differences from
    data differences. Only rule differences count against conformance.
12. The harness is a Rust library as well as a Headless Client. Its scenario model, independent
    decoders and comparator serve the tier 1 runner through a git dev-dependency. Pin it to the
    same SHA as `.wire-harness-rev`. The harness never depends on a LyraCore crate or shares a
    decoder with core. Do not use `benilla-protocol` to decode tier 1 results. The first mechanism
    test uses gtker directly until the harness library is available.
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
It does not yet establish conformance to a recorded Behavior.

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
Module bindings. Each call returns the subscribed inserts from its own transaction; separate
subscriptions remain the path for observing broadcasts.

`debug_native_enter_world` exists only with `debug_reducers`. It requires the Operator and an open
connection in `AwaitingProof`. It establishes a fixture Session and world proof, then uses normal
world entry. Later requests still pass the normal Account and Session Gates. The Architecture Test
`native_debug_gate` checks the feature gate and detects its removal or replacement with a weaker
condition. No production authentication Gate is bypassed without the feature.

The durable `native_conformance` test enters a Character, sends `CMSG_PING` through
`native_world_receive`, reads the private packet event on the same connection and decodes
`SMSG_PONG` directly with gtker. It also checks the non-Operator refusal. The decoder is a single
function for replacement with the harness library in Milestone 2. This proves the mechanism, not
agreement with a Golden Trace. Run it with the command in [testing](../testing.md#conformance).

C1c adds schema version 1 and a shared loader in `lyracore-conformance`. TOML keeps the ledger
readable without a generator and gives each Behavior its own file for parallel work. The loader
checks field types, ids and file paths, source references, required rungs, dependency cycles and
human verification records. The [schema reference](../../conformance/README.md) states the fields.

The nine original inventory entries now live in `conformance/features/`. The inventory reads that
same loader and still checks retained Rust test declarations without running them. Entries contain
no handwritten support status. Their initial scenario lists are empty; source evidence does not
stand in for recordings. The Architecture Test validates references when a pinned harness catalogue
is present and prints a clear skip for the current pre-catalogue pin. The assumed catalogue path
and shape remain in one test function for Milestone 2 integration.

The harness scenario format is pending the parallel harness milestone. The existing pin stays
unchanged in Milestone 1. Milestone 2 updates its prerelease tag and full SHA and adds the tier 1
library dependency at that same SHA.
