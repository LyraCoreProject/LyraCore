---
name: climb-conformance
description: Climb a 1.12 Behavior from ranked conformance work as either an implementer or a scenario author.
metadata:
    harness: [claude, codex]
---

# Climb conformance

Take one role for one Behavior. Record the scenario author, implementer and harness PR in the implementation brief. Keep the roles with different people or agents. Read the [Feature Ledger](../../../conformance/README.md) and [verification steps](../../../docs/testing.md#conformance) before changing evidence.

## Implementer

1. Run `./conformance/scoreboard --json`. Take its first open item with `kind = "implementer"`. If it names `reproduce_with = "audit"`, check every `prerequisites` entry before running its `next_command`; hand the item to a maintainer when a prerequisite is unmet. Use its first listed scenario for tier 1 work.
2. Run `./conformance/run <scenario>` and `./conformance/run <scenario> --json`. Exit 0 means the comparison completed and its signatures match the Tier 1 baseline, even if rule differences remain. Exit 1 means the baseline changed. Exit 2 means setup failed before comparison; fix setup before judging the Behavior.
3. Read the Behavior's vmangos commit, path and symbol in its ledger TOML. Read that source through a GitHub raw URL. Match the Reference Server's observable result in the Module. A different implementation is fine when the result matches. Ask a maintainer to decide any proposed Deliberate Deviation.
4. Re-run both forms of `./conformance/run <scenario>`. When signatures disappear, run both durable tier 1 tests from [testing](../../../docs/testing.md#conformance), then `./conformance/baseline`. Commit the generated baseline with the gameplay change. Update that Behavior's `known_gaps` to match the remaining evidence. Review the signature diff.
5. Run the CI checks in [testing](../../../docs/testing.md) and open a core PR with [file-pr](../file-pr/SKILL.md). Show added and removed signatures in the PR.

Keep scenario defects with the scenario author. Keep scenarios, Golden Traces, Equivalence Rules and the harness pin out of an implementation PR. Preserve the tests and required rungs; add no skips or weaker rules. Generate baselines only with `./conformance/baseline`.

## Scenario author

1. Run `./conformance/scoreboard --json`. Take its first open item with `kind = "scenario_author"`. Read its Feature Ledger TOML and vmangos source reference.
2. In the harness repository, write a TOML scenario with Fixture Verbs, steps, checkpoints and Equivalence Rules. Follow [scenario format and verb additions](https://github.com/LyraCoreProject/wire-harness/blob/ad27050ef5c91a1f5ba7337dce29d5d31233898b/docs/scenarios.md). Add a needed Fixture Verb to the scenario model and both adapters, with their tests, before recording.
3. Prepare the local Reference Server as its [adapter README](https://github.com/LyraCoreProject/wire-harness/blob/ad27050ef5c91a1f5ba7337dce29d5d31233898b/adapters/vmangos/README.md) directs. Build `vanilla-wire`, then run the checked recorder. It captures and compares two samples:

   ```sh
   cargo build --locked --bin vanilla-wire
   newgrp docker <<'DOCKER'
   adapters/vmangos/reference.sh record-checked scenarios/<scenario>.toml traces/<scenario>.json
   DOCKER
   ```

4. Review the scrubbed Golden Trace and open a harness PR into `feat/conformance`. The orchestrator tags a prerelease after that PR merges. Then make a separate core PR that bumps `.wire-harness-rev` and the `wire-client` git revision, adds the Behavior's `scenarios` entry and a test in `module/tests/conformance/tier1.rs`, runs both tier 1 tests, and generates the Tier 1 baseline with `./conformance/baseline`.

Keep gameplay code with the implementer. Only the recorder writes Golden Traces. Commit no client files, Reference Server data or unscrubbed packet text.
