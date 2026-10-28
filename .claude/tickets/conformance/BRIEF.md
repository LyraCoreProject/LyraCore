# Brief: the 1.12 conformance system

You are building the system that tells LyraCore whether it still behaves like 1.12.1, and that lets
agents implement missing 1.12 behaviors without grading their own work. The decisions below were
settled with the maintainer on 2026-10-26 and 2026-10-27. Don't reopen them. If the code
contradicts one, stop and report.

A separate effort replaces classic-db with the vmangos world database on `feat/vmangos-world-data`
(brief: `.claude/tickets/vmangos-port/BRIEF.md` on that branch). Assume it lands. Don't touch its
files or branches. This work runs alongside it and merges after it.

Read these first, in this order:

1. `AGENTS.md`, `CODING_STANDARDS.md` and `CORE_TERMS.md`, and the parts of `CONTEXT.md` your work
   touches.
2. `.claude/skills/unslop/SKILL.md` and `.claude/skills/file-pr/SKILL.md`. They apply to every
   comment, doc, commit and PR you write.
3. `docs/testing.md` (the verification ladder), `docs/architecture.md` (native host), and
   `docs/benilla-compatibility.md`.
4. The wire harness: `LyraCoreProject/wire-harness`, its `README.md` (the boundary rules) and
   `CONTRIBUTING.md`. `.wire-harness-rev` in this repo pins it.

Facts below were checked at core `f0d12b7f` and harness `68f4a0de`. Check every file:line before
you rely on it.

## Why

LyraCore's goal is to be feature complete for 1.12.1. Today nothing measures that, and nothing stops
an agent from writing a test that agrees with its own mistake. This system defines what 1.12 is,
records what a reference server does, and checks LyraCore against the recording.

## The shape

Two tiers, two repositories.

- **Tier 1, in-process, in LyraCore CI.** Tests drive the Module through its native reducers with
  no TCP, SRP or cipher, and compare the packets it writes against Golden Traces. Fast, per
  behavior, run on every change.
- **Tier 2, the Conformance Audit, by hand.** The harness drives a full LyraCore Realm (real Native
  Host, imported world) and the Reference Server through the same scenarios over the wire, and
  writes a dated report. It runs before a batch of feature work and at milestones. It never gates
  CI.
- **The harness repo** (`wire-harness`) holds what describes 1.12, not LyraCore: scenarios, the
  Fixture Verb vocabulary, Golden Traces, the general Equivalence Rules, the comparator and the
  Reference Server adapter.
- **LyraCore** (`conformance/`) holds what describes LyraCore: the Feature Ledger, Deliberate
  Deviations, audit reports and the scoreboard.

## Settled decisions

1. **"Feature complete" means systems.** Every 1.12 system works in core. Dungeon and raid encounter
   content is a separate track in the official dungeons Package, with its own scoreboard.
2. **vmangos is the Reference Server.** Its observable results are the definition of 1.12 for this
   system. When LyraCore keeps a different *result* on purpose, it records a Deliberate Deviation in
   the ledger, with a reason and a source. cmangos may be read to settle a dispute, never recorded.
3. **Results, not implementation.** Conformance covers what the client receives and the game
   outcome. LyraCore may and should implement things differently (transactional, single-Shard,
   data-driven, more ergonomic or scalable) as long as the result is the same. That freedom is not a
   Deliberate Deviation; Deliberate Deviations are only for results that differ.
4. **What "the same result" means.** Compare decoded packets field by field under Equivalence
   Rules:
   - guids map consistently between the two servers (a bijection per trace);
   - times compare within a tolerance;
   - packets that answer one request keep their order; unrelated broadcasts may interleave;
   - fields driven by random rolls are checked for shape and range, not value.

   Any other permitted difference is a named rule with a reason. General rules live in the
   harness. A rule that exists only for LyraCore is a Deliberate Deviation in core.
5. **The ledger's unit is a Behavior**: one interaction a player would recognize, such as
   `taxi_single_hop`, `corpse_loot`, `pet_spell_cast`. Systems group Behaviors on the scoreboard.
   One TOML file per Behavior under `conformance/features/<system>/<behavior>.toml`, so parallel
   work never edits the same file. The current
   `crates/lyracore-protocol/examples/benilla_inventory/behaviors.json` folds into the ledger and
   then goes away; keep the inventory test working against the new location.
6. **Ledger entries carry no hand-written status.** An entry holds: the 1.12 scope in plain words,
   the opcodes, the Behaviors it depends on, a vmangos source reference (path at a pinned SHA), the
   rungs it needs to count as done, its scenario ids, any Deliberate Deviations, and a
   `real_client` field people fill in. Tools derive everything else.
7. **Golden Traces change only through the recorder.** A trace is written by recording a scenario
   against the Reference Server. Implementation work never edits a trace. Because traces live in
   the harness repo, an implementation PR in core cannot change one.
8. **Traces hold no text.** The recorder replaces every text field (names, quest text, gossip,
   chat) with its SHA-256 digest before writing. The comparator compares digests. Numbers, ids,
   flags and positions stay as they are.
9. **Scenarios are data, in the harness repo.** A scenario names the Behavior it covers, its
   Fixture Verbs, the client packets it sends as typed fields, and its checkpoints. Pick the file
   format (TOML or RON) and explain the choice in the ADR.
10. **Fixture Verbs.** A scenario asks for setup from a small vocabulary: a Character of a race,
    class and level at a place; items, money, spells and known flight nodes to give it; a creature
    or gameobject to spawn at an offset; a quest to put in its log. Each adapter provides each verb:
    LyraCore through debug reducers, vmangos through GM commands from a GM Account (`.levelup`,
    `.additem`, `.learn`, `.npc add`, `.tele` and the like). The vocabulary grows only when a
    scenario needs a new verb, and a verb gets both adapters before any scenario uses it.
11. **World data.** Scenarios rely on fixture-spawned entities and assert rules. The audit report
    sorts divergences into rule differences, which count, and data differences, which are listed
    separately and don't count against conformance.
12. **The harness is a library as well as a client.** The scenario model, the harness's own
    decoders and the comparator are a Rust library. LyraCore's tier 1 runner uses it as a git
    dev-dependency pinned to the same SHA as `.wire-harness-rev`. The harness never depends on a
    LyraCore crate, and it never shares a decoder with LyraCore (its README explains why). This
    replaces any plan to decode with `benilla-protocol`.
13. **The Reference Server runs in Docker, outside every repo.**
    - A compose setup runs vmangos's prebuilt `dev-linux-amd64` binaries with MariaDB.
    - The world database is loaded from the `db_latest` release's MySQL dump. A copy is at
      `$HOME/.cache/lyracore-scratch/vmangos-db/`.
    - Maps, vmaps and mmaps come from the maintainer's own client, extracted with vmangos's
      extractors. The client path is `lyracore config get client-data` (check the command).
    - Everything at runtime lives under `$HOME/.cache/lyracore-scratch/reference-server/`.
    - The compose file and a setup script go in the harness under `adapters/vmangos/`. They hold no
      data.
    - Loading vmangos SQL into the reference server's own MariaDB is fine: ADR 0003 governs
      LyraCore's importer only.
    - Record which vmangos binary release and database commit a recording used, in the trace header.
14. **Scoreboard.** A script or test in core reads `conformance/` and the latest tier 1 results and
    audit report, and prints the scoreboard per system and Behavior, plus data-derived counts:
    - client opcodes claimed by a Behavior, out of all `ClientOpcodeMessage` variants;
    - trainable spells per class whose effect and aura kinds the Module supports, from vmangos
      `spell_template`;
    - `module/src/native/unavailable.rs` entries still open;
    - Behaviors waiting on a real-client check, as their own column. People fill that in through
      `verify-lyracore`; the audit never touches it.

    It stays out of the `lyracore` CLI for now.
15. **`native-host/check.py` keeps Host and transport properties only**: TCP, the Login Queue,
    crash recovery, slow readers, World Session replacement. Gameplay scenarios move into
    conformance scenarios one at a time as their Behavior enters the ledger, and are deleted from
    `check.py` once moved. Write the rule down; don't move them all in this work.
16. **Separate roles.** Later, when Behaviors get implemented through this system, a scenario author
    writes and records a scenario in a harness PR, and a different implementer makes tier 1 pass in
    core. Write that rule into the ADR and the orchestration docs. Nobody does both for one
    Behavior.

## Facts to build on

- The Module's whole protocol boundary is three reducers and one table
  (`module/src/native/mod.rs`): `native_world_open` (line 328) starts a connection and writes
  `SMSG_AUTH_CHALLENGE`; `native_world_receive(opcode, body)` (line 353) takes one client packet;
  `native_world_reply` sends Module-encoded refusal replies; server packets come out as rows in the
  private `game_native_packet` table. All three require the Operator and a `ConnectionId`
  (`connection_id`, line 227).
- Durable tests drive Shards through the `spacetime` CLI and `spacetime subscribe`
  (`crates/lyracore-test-support/src/lib.rs`, `subscription.rs`). An HTTP call carries no
  `ConnectionId` as far as we know (unverified), so tier 1 probably needs a WebSocket client that
  connects as the Operator.
- A new connection starts in `NativeWorldState::AwaitingProof`. Tier 1 needs a way in that skips
  SRP.
- The harness's LyraCore adapter is from the Gateway era: `adapters/lyracore/adapter-env.sh:35`
  refuses a checkout without `gateway/`. Its scenarios are ad hoc Rust modes driven by about 50
  shell scripts. Its generic client (`src/lib.rs`, `values_mask.rs`, `spatial.rs`) is the part to
  keep.
- **`lyracore dev smoke` depends on the harness.** The CLI runs `adapters/lyracore/wire.sh` and the
  `vanilla-wire` binary, and recognizes a harness checkout by `Cargo.toml`, `src/` and
  `adapters/lyracore/` (`lyracore-cli` `src/harness.rs:56-77`, `src/project.rs:396-397`). Keep
  that seam working, rewritten for the Native Host. No CLI change is part of this work.
- The vmangos port takes ADR 0004. This work's ADR is **0005**.

## Rules

- **Branches.**
  - Core: base on `feat/conformance` (from `main`). Harness: `feat/conformance` in
    `wire-harness`, from its `main`. Name slice branches `conformance/<slice-id>`.
  - Within a milestone, stack slices: each branches from the previous one's head and its PR
    targets that branch; the first slice's PR targets `feat/conformance`.
  - You may push these branches and open these PRs without asking. Write PR text with the
    `file-pr` skill.
  - Never merge, never touch `main` in either repo, never open a PR into `main`. The orchestrator
    merges after each milestone review. The maintainer merges the final PRs into `main`.
  - Core's `.wire-harness-rev` points at the harness branch's tag and SHA during this work. Use a
    prerelease tag on the harness branch, such as `v0.2.0-conformance.N`, and bump it per
    milestone.
- **File ownership.** Core agents own core files; harness agents own harness files. Don't edit
  `importer/`, the port's tickets, or any `vmangos/*` branch. If you need a change there, report it.
  Expect small conflicts in `CONTEXT.md` and `docs/README.md` with the port; keep your edits there
  small and self-contained.
- **Data.**
  - Never commit, push or upload vmangos data, client files, map extracts, or an unscrubbed trace.
    Committed traces follow decision 8. Test fixtures are small and hand-written.
  - Treat downloads as untrusted. Keep runtime state under
    `$HOME/.cache/lyracore-scratch/reference-server/` and build output under
    `$HOME/.cache/lyracore-scratch/conformance/`, never `/tmp`.
- **vmangos and cmangos sources.** Never clone either repository, not even sparse, not even into
  `/tmp`. Read files with `gh api repos/vmangos/core/contents/<path>?ref=<sha>` or raw URLs, and cite
  the SHA. Downloading vmangos release assets (binaries and database) is allowed. Never copy vmangos
  code; reimplement from reading it.
- **No outward actions.** Don't post on vmangos, cmangos or any other upstream. Don't file issues.
  Don't touch production or any deployed Realm.
- **Verifying.** Use only a local Realm you started yourself, on your own ports. Before you call a
  failing test pre-existing, show it fails the same way on the base commit.
- **Words.** Use glossary terms. Add these to `CONTEXT.md` in the slice that introduces them, one
  entry each: **Conformance Audit**, **Reference Server**, **Golden Trace**, **Equivalence Rule**,
  **Deliberate Deviation**, **Fixture Verb**, **Feature Ledger**, **Behavior**. Check the existing
  glossary first and reuse a term rather than add a near-duplicate.
- **Comment review.** Complete the comment review in `CODING_STANDARDS.md` on every slice and report
  its result.
- **Stopping.** Stop and report rather than guess when a decision above doesn't fit what you find,
  when keeping `dev smoke` working would need a CLI change, or when the Reference Server can't run
  from the prebuilt binaries.

## Milestones and slices

Each slice is one PR with its tests. Stop at each milestone for review. Don't start the next
milestone in the same run.

### Milestone 1: the two foundations, in parallel

Two agents with no shared files.

**Core (agent A).**

- **C1a. ADR 0005 and terms.** Draft `docs/adr/0005-1-12-conformance.md` recording decisions 1-16,
  and the glossary entries. Add the ADR to `docs/README.md`. Update it as later slices teach you
  things.
- **C1b. The in-process spike.** Prove the mechanism, nothing more:
  1. Add a WebSocket client that connects as the Operator to `lyracore-test-support`, giving tests a
     real `ConnectionId`. Reuse the SDK or the CLI's subscription path if one fits; explain the
     choice.
  2. Add a debug reducer, built only with the `debug_reducers` feature, that moves a native
     connection straight into the world for a given Character, skipping SRP. It must be impossible
     to reach without that feature; add an Architecture Test if one doesn't already cover it.
  3. Send one client packet through `native_world_receive` and read the resulting
     `game_native_packet` rows.
  4. Decode them with a decoder independent of LyraCore's codec. Until the harness library exists,
     use the gtker crates directly in the test, and leave a clear seam for decision 12.

  One durable test proves it end to end, for example `CMSG_PING` or a `/who` request.
- **C1c. The ledger format.** Define the TOML schema, a loader with validation, and an
  Architecture Test that every ledger file parses and names existing scenario ids once the harness
  pin carries scenarios. Fold the 9 entries of `behaviors.json` into it and move the inventory test
  over.

**Harness (agent B).**

- **H1a. Clear the Gateway era.** Delete the Gateway-era adapter and the shell scenarios, including
  `adapters/lyracore/playerbots`. Rewrite `adapters/lyracore/wire.sh` and the adapter environment
  for the Native Host so `lyracore dev smoke` keeps working. Show it working against a local Realm.
- **H1b. The library.** Split the crate into a library (client, decoders, scenario model,
  comparator) and the `vanilla-wire` binary. Keep the boundary rules in the README.
- **H1c. Scenarios and Fixture Verbs.** The scenario format (decision 9), the verb vocabulary
  (decision 10) with a trait each adapter implements, and a runner that plays a scenario over the
  wire.
- **H1d. Recorder and comparator.** The recorder writes a Golden Trace with the text scrub
  (decision 8) and a header naming the server, its binary release and database commit. The
  comparator applies the Equivalence Rules (decision 4) and reports differences in a form both a
  test assertion and an audit report can use. Unit-test every rule.
- **H1e. The Reference Server.** `adapters/vmangos/`: the compose setup and setup script
  (decision 13), the vmangos Fixture Verb adapter, and a GM Account. Show a login and one recorded
  trace from it.

**Milestone 1 handoff:** the spike test passing; the ledger with the folded entries; `dev smoke`
working on the rewritten adapter; a trace recorded from the Reference Server with its header; the
comparator's rule tests; ADR 0005 so far.

### Milestone 2: the tracer

Single-hop flights, a behavior LyraCore already has, end to end. This tests the system, not the
game.

**What Milestone 1 settled** (added 2026-10-27 after its review; these are decisions now):

- **The harness owns the rules.** Equivalence Rules live in each scenario's TOML under `[rules]`,
  covered by `scenario_sha256`. Core passes only typed, named Deliberate Deviations, and every one
  that applies is counted in the result. Never add a way for a caller to loosen a rule.
- **Every packet counts.** The comparator reports undeclared packets as `unexpected_packet`
  differences unless the scenario names an excusing rule.
- **The opening phase.** Entering the world produces many packets that depend on world data (the
  first LyraCore-against-vmangos comparison of `connection_identity` reported 113 unexpected
  packets with every declared field matching). Scenarios get an opening phase: packets from login
  until the scenario's first step are recorded in the trace but not compared. Comparison starts at
  the first step. Don't excuse world-entry noise with per-opcode rules.
- **Core reaches the harness only through the library.** `wire_client::catalogue` embeds every
  scenario and trace. Core adds `wire-client` as a git dev-dependency at the exact SHA in
  `.wire-harness-rev`; an Architecture Test checks the two agree. Core never reads harness files
  from a checkout path.
- **The fixture reducer** is `debug_native_enter_world`, compiled only with
  `conformance_test_fixture`. `lyracore publish` never enables that feature; keep it that way.
- **Packets across transactions.** The test-support connection buffers every
  `game_native_packet` row for the connection across transactions, with drain and timed-wait
  APIs. Scheduled ticks (a taxi landing) depend on it.
- **Candidate traces.** The harness can run a scenario against any adapter and write a candidate
  trace, marked as a candidate, and compare observations with a Golden Trace.

**Slices.** Harness and core run in parallel; the core agent wires the whole tier 1 path using
`connection_identity` (already in `v0.2.0-conformance.2`) while the harness agent builds the taxi
scenario. A last core slice switches to the taxi tag.

- **T2a (harness).**
  - The opening phase above, with tests. Re-record `connection_identity`.
  - The Fixture Verbs the taxi scenario needs, each with both adapters: give money, mark flight
    nodes known, spawn a creature (the flight master) at an offset from the Character. Allow more
    than one fixture per scenario.
  - The taxi client packets and the harness's own decoders for their replies (node status, node
    list, activate taxi reply, the flight's movement, landing).
  - The `taxi_single_hop` scenario: a Character with money and both nodes known, a flight master at
    an offset, the node list, takeoff, landing. Pick two Alliance nodes on map 0 that both servers'
    data define with the same id (for example Stormwind and Sentinel Hill or Lakeshire); check them
    in vmangos's `taxi_nodes` and in the client's TaxiNodes.dbc. Record its Golden Trace. Explain
    in the scenario's rules every Equivalence Rule a flight needs (a flight's duration and spline
    timing are likely candidates; say why each tolerance is the size it is).
  - Tag `v0.2.0-conformance.3`.
- **T2b (core).**
  - Bump `.wire-harness-rev` to `v0.2.0-conformance.2`; add the `wire-client` dev-dependency and
    the Architecture Test that its `rev` equals the pin's SHA.
  - The tier 1 runner: implement the harness's scenario `Transport` (or whatever the library
    calls the adapter interface) in core over the buffered Operator connection, with the Fixture
    Verbs backed by debug reducers. Replace the C1b gtker decoder with the harness library.
  - `connection_identity` passes in-process against its Golden Trace. Add its ledger entry.
  - The ledger check reads scenario ids from `wire_client::catalogue`. Delete the checkout reader
    and every SKIP path. It fails when a ledger scenario id is missing, and when a scenario's
    `behavior` doesn't name the Behavior that lists it.
- **T2c (core).** The scoreboard script (decision 14): per system and Behavior, with the opcode
  count, `unavailable.rs` count and the real-client column. Spell counts come in Milestone 3.
- **T2d (core, after T2a's tag).** Bump the pin to `v0.2.0-conformance.3`. Add the core backing for
  the new Fixture Verbs. `taxi_single_hop` passes in-process (tier 1) against its trace. Add its
  ledger entry. Then play it over the wire against a local LyraCore Realm through the harness's
  LyraCore adapter, write a candidate trace and compare it with the Golden Trace.

If a divergence shows up, sort it per decision 11 and report it; fix nothing in gameplay unless the
fix is trivial and clearly a LyraCore bug. Never loosen a rule to make LyraCore pass: a real
difference stays a reported difference.

**Milestone 2 handoff:** tier 1 and the wire run both green against the same trace (or the sorted
divergences); the scoreboard output.

### Milestone 3: the first audit

**What Milestone 2 settled** (added 2026-10-27 after its review and re-check; these are decisions
now):

- **Taxi gaps are fixed after the merge.** The 8 known gaps on `taxi_single_hop` stay unfixed on
  this branch. They become the first Behaviors built through the system, with taxi chains, once
  this branch and the vmangos port are in `main`. The orchestrator owns that work end to end and
  merges its PRs, including into `main`, after tests and review.
- **A hand-written route fixture is allowed.** Core tests may carry the 28 vertices of taxi path 6
  (and the two nodes) as a small hand-written fixture, so the taxi comparison needs no imported
  data. The public Golden Trace already holds these numbers. No other world data, and nothing
  larger. The durable CI skip of the taxi comparison stays until its gaps are fixed; remove it in
  the same PR that fixes the last gap.
- **One way to declare data differences, in the harness.** Data-dependent fields are declared in
  the scenario (`[[rules.data_fields]]`), owned by the scenario author and covered by
  `scenario_sha256`. The comparator emits a record for each, tagged as data, so decision 11's
  "listed separately" holds. The core ledger's `data_fields` are removed; the scoreboard and audit
  take classification from the comparator's tags only.
- **Group records by signature.** The scoreboard and audit count differences grouped by (rule,
  checkpoint, stream, opcode, field path), and also show raw record counts. Fixed/regressed
  tracking uses signatures.
- **Known gaps can't excuse anything.** They stay descriptive and never change classification or
  pass/fail.

**Slices.** H3-0 and C3-1 run in parallel; C3-2 follows both.

- **H3-0 (harness).**
  - Data fields emit tagged data records instead of passing silently.
  - Enable serde_json `float_roundtrip` in `wire-client` itself, with a precision test.
  - `passenger_zone_flags_delivery` applies only to VALUES updates (`kind == 0`).
  - `validate` refuses a single-sample identity or clock rule whose reason doesn't say what is
    unchecked.
  - Weather: when `weather_type` is 0, `grade` and `sound` must be 0.
  - Document how alignment treats two unmapped GUIDs in swapped order.
  - Reduce alignment cost (memoize scores or pre-filter by key) with a benchmark test on 1,000
    packets.
  - Tag `v0.2.0-conformance.5`.
- **C3-1 (core).** Work that doesn't need the new tag:
  - A3b, A3c and A3d below.
  - The taxi route fixture (decision above); the taxi tier 1 test uses it instead of
    `LYRACORE_TEST_TAXI_SQL`.
  - The pin Architecture Test also checks `source` and `rev` for every crate that declares
    `wire-client`.
  - ADR 0005: tier 1 wording (the taxi comparison is skipped in CI until its gaps are fixed).
- **C3-2 (core, after H3-0's tag and C3-1).**
  - Bump the pin to `.5`. Drop the core `float_roundtrip` feature. Remove the ledger
    `data_fields`; classification comes from comparator tags.
  - Signature grouping in the scoreboard.
  - A3a: the audit, run once, report committed.

- **A3a (core).** The audit command or script: runs every scenario against both servers over the
  wire, writes `conformance/reports/<date>.md` with rule divergences, data divergences, fixed and
  regressed since the last report. Run it once and commit the report.
- **A3b (core).** The Architecture Test that every `ClientOpcodeMessage` variant is claimed by a
  Behavior or listed as out of scope, with today's gaps on an allowlist that only shrinks.
- **A3c (core).** Trainable spells per class from vmangos `spell_template` (largest build ≤ 5875)
  against supported effect and aura kinds, added to the scoreboard. Read the vmangos SQLite file
  from the scratch copy; don't depend on the port's importer code.
- **A3d (core).** Write the `check.py` rule (decision 15) and the separate-roles rule (decision 16)
  into `docs/testing.md`, the ADR, and `.claude/skills/orchestrate-backlog/SKILL.md`.

**Milestone 3 handoff:** the first audit report, the scoreboard with all counts, and the allowlist
sizes.

The first new Behavior built through the system, taxi chains, starts only after this branch and the
vmangos port have both merged into `main`.

### Milestone 4: the climbing loop

Added 2026-10-28. **The end goal of this system is to give agents tools to hill-climb to 1.12
parity.** Judge every change by whether it makes that loop faster, more trustworthy, or harder to
game. Milestone 4 starts after `feat/conformance` merges into `main`, and branches from `main`.

**Decisions:**

- **A ratchet in CI.** Each Behavior with a scenario has a committed tier 1 baseline: the set of
  rule signatures the comparator reports today. CI runs tier 1 and requires the signature set to
  equal the baseline. A new signature fails (a regression). A removed signature also fails until
  the same PR regenerates the baseline, so every improvement is visible in review. The baseline is
  written only by a command from comparator output, never by hand, holds no reason text, and never
  changes classification, the scoreboard's pass/fail, or the audit. With the ratchet, the taxi
  comparison runs in CI again (its baseline is today's signatures) and the durable skip goes.
- **One command for the inner loop.** `conformance run <scenario>` runs tier 1 for one scenario
  and prints the grouped signatures, each with expected and actual values, the rule and its
  reason, and the Behavior's vmangos source reference. `--json` gives the same for agents.
- **A machine-readable scoreboard** (`--json`) with a ranked list of open work. Rank by a stated,
  simple formula (for example: Behaviors other Behaviors depend on, then signature count, then
  claimed-opcode count); explain it in the README. No hand-written priority.
- **Every Behavior gets a scenario.** The scoreboard shows scenario coverage per system. A
  Behavior without a scenario is open work for a scenario author.
- **Separate roles stay.** A scenario author writes and records scenarios in harness PRs; an
  implementer climbs in core. The climbing skill enforces it.

**Slices.**

- **M4a (core).** The ratchet and its regeneration command; taxi back in durable CI.
- **M4b (core).** `conformance run` with text and JSON output.
- **M4c (core).** Scoreboard JSON and the ranked open-work list; scenario coverage.
- **M4d (core).** `.claude/skills/climb-conformance/SKILL.md`: the loop for an implementer (pick
  the top open item from the scoreboard JSON, confirm a scenario exists or hand off to an author,
  reproduce with `conformance run`, implement, run tier 1 and the ratchet, regenerate the baseline,
  open a PR) and for a scenario author (Fixture Verbs, recording against the Reference Server,
  tagging a harness prerelease, bumping the pin). Follow `writing-for-agents`.
- **M4e (harness).** Agent-runnable recording: one command that brings up the Reference Server,
  records a scenario, checks a repeat recording matches, and writes the trace. Fixture Verbs as the
  first new scenarios need them. Fold in the two parked nits (single-sample validation against the
  Golden Trace, the intermittent SRP retry).
- **M4f (first climb).** The 8 taxi gaps, through the skill, by an implementer who did not write
  the scenario. Each fix shrinks the ratchet baseline. The orchestrator merges these PRs into
  `main` after tests and review.
