---
name: climb-conformance
description: Climb a 1.12 Behavior from ranked conformance work as either an implementer or a scenario author.
metadata:
    harness: [claude, codex]
---

# Climb conformance

Take one role for one Behavior. Whoever dispatches the work assigns the scenario author and implementer to different people or agents. The core PR description names the Behavior and your role, and links its harness PR if one exists. Read the [Feature Ledger](../../../conformance/README.md) and [verification steps](../../../docs/testing.md#conformance) before changing evidence.
The scoreboard requires every build-5875 client request to have a Behavior claim or an
out-of-scope reason. Add a ledger entry before taking ranked work when it reports an unclaimed
opcode. New request Behaviors have empty scenario lists; their vmangos references and scope are
starting points for scenario authors.

## Implementer

1. Run `./conformance/scoreboard --json`. Take its first open item with `kind = "implementer"`. If it names `reproduce_with = "audit"`, check its `prerequisites`, then run `./conformance/audit --scenario <id> --check --client-data /path/to/client/Data` using its first scenario. Hand the item to a maintainer when a prerequisite is unmet. Use its first listed scenario for tier 1 work.
2. Run `./conformance/run <scenario>` and `./conformance/run <scenario> --json`. Exit 0 means the comparison completed and its signatures match the Tier 1 baseline, even if rule differences remain. Exit 1 means the baseline changed. Exit 2 means setup failed before comparison; fix setup before judging the Behavior.
3. Read the Behavior's vmangos commit, path and symbol in its ledger TOML. Read that source through a GitHub raw URL. Match the Reference Server's observable result in the Module. A different implementation is fine when the result matches. Ask a maintainer to decide any proposed Deliberate Deviation.
4. Re-run `./conformance/run <scenario> --json`. When signatures change, run `./conformance/baseline <scenario>`. Commit the generated baseline with the gameplay change. Update that Behavior's `known_gaps` to match the remaining evidence. Review the signature diff. If a missing packet reveals a fixture or data difference, record the expected and actual values, source provenance and signature in the PR and `known_gaps`. Keep the measured difference without a tolerance or rule edit. Hand the fixture correction to a scenario or fixture author as a separate change; a maintainer decides disputed data authority.
   If the fixture cannot show a fix, keep the PR ready only when a durable test backed by the vmangos source proves the behaviour and the PR names the fixture gap and its author. Otherwise open it as a draft.
   If the item has `reproduce_with = "audit"`, tier 1 cannot show the fix. Commit the fix, run its scenario audit command again and compare its grouped differences with the committed full audit. The local command writes no report. A maintainer refreshes the append-only full audit separately. Without the prerequisites, open the PR as a draft and hand the audit to a maintainer.
5. Run `./conformance/run <scenario>` once more; it must exit 0 with a matching Tier 1 baseline. Run `cargo fmt --all`. Run no other checks; CI runs them. Open a core PR with [file-pr](../file-pr/SKILL.md). Show added and removed signatures in the PR.

If you change a table schema, read [migration trap §1.2](../../../docs/danger-zones.md#1-do-not-touch-traps) and update every test that inserts into that table. When CI fails, reproduce only the failing test with the command in [testing](../../../docs/testing.md).

When setup cannot be repaired or no fix matches the Reference Server, stop and hand the item to a maintainer with the commands you ran and their output. Keep scenario defects with the scenario author. Every review finding must become an automated check that fails on the old code before its fix merges: a scenario variant (for example a second Character or a reconnect), a property check, a strictness mutation or a fixture check. List review findings that CI missed in the PR, each tagged as uncovered situation, loose rule, fixture artefact, broken invariant or client rejection. Keep scenarios, Golden Traces, Equivalence Rules and the harness pin out of an implementation PR. Preserve the tests and required rungs; add no skips or weaker rules. Generate baselines only with `./conformance/baseline`.

## Scenario author

1. Run `./conformance/scoreboard --json`. Take its first open item with `kind = "scenario_author"`. Read its Feature Ledger TOML and vmangos source reference.
2. In the harness repository, write a TOML scenario with Fixture Verbs, steps, checkpoints and Equivalence Rules. Follow [scenario format and verb additions](https://github.com/LyraCoreProject/wire-harness/blob/f58d550eb316557b64290f61da26339aa9e09ceb/docs/scenarios.md). Add a needed Fixture Verb to the scenario model and both adapters, with their tests, before recording.
   Register the scenario and its trace in `src/catalogue.rs` as [adding a scenario](https://github.com/LyraCoreProject/wire-harness/blob/f58d550eb316557b64290f61da26339aa9e09ceb/CONTRIBUTING.md#adding-a-scenario) describes.
3. Prepare the local Reference Server as its [adapter README](https://github.com/LyraCoreProject/wire-harness/blob/f58d550eb316557b64290f61da26339aa9e09ceb/adapters/vmangos/README.md) directs. Build `vanilla-wire`, then run the checked recorder. It captures and compares two samples:

   ```sh
   cargo build --locked --bin vanilla-wire
   newgrp docker <<'DOCKER'
   adapters/vmangos/reference.sh record-checked scenarios/<scenario>.toml traces/<scenario>.json
   DOCKER
   ```

4. Review the scrubbed Golden Trace and open a PR into the harness repository's `main` branch. After it merges, harness CI tags the tested commit. Do not tag by hand. When the catalogue adds a scenario, make one core PR that advances `.wire-harness-rev`, the workspace `wire-client` revision and `Cargo.lock`, adds the Behavior's `scenarios` entry and its test in `module/tests/conformance/tier1.rs`, and regenerates the Tier 1 baseline. The [pin proposal workflow](../../../conformance/README.md#automated-harness-pin-proposals) skips new scenarios. Run `./conformance/run <scenario>` after that change; it must exit 0 with a matching Tier 1 baseline. CI runs all scenarios with `cargo test -p lyracore-module --test native_conformance -- --ignored`.

Keep gameplay code with the implementer. Only the recorder writes Golden Traces. Commit no client files, Reference Server data or unscrubbed packet text.
