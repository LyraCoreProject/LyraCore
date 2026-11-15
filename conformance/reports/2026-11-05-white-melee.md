# White main-hand melee: first Roll Conformance slice

Measured on 2026-11-05. [ADR 0007](../../docs/adr/0007-statistical-roll-conformance.md) gives the
independent vmangos derivation, every formula input and pinned source lines. This author derived
rates from that source and changed only conformance fixtures and measurement infrastructure.
No gameplay correction is included. vmangos is the only Reference Server.

The crit, block and glancing findings assume the Tier 1 fixture matches the pinned inputs: agility 100, skill 300, unarmed.

## Delivered LyraCore outcomes

Each row contains 50,000 own-attacker `SMSG_ATTACKERSTATEUPDATE` packets. The fixture sends
`CMSG_ATTACKSWING`, advances the ordinary melee scheduler, applies damage, routes recipient
delivery instructions and passes the delivered bytes to the git-pinned harness decoder.
No calculation reducer, server histogram or observer duplicate supplies a count.

| Target / sample | Miss | Dodge | Parry | Block | Glancing | Crit | Hit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 derived | 5% | 5% | 5% | 5% | 10% | 5% | 65% |
| 60 public 1 | 5.038% | 4.842% | 4.872% | 0.000% | 0.000% | 11.704% | 73.544% |
| 60 public 2 | 4.982% | 5.168% | 5.062% | 0.000% | 0.000% | 11.618% | 73.170% |
| 63 derived | 8% | 6.5% | 14% | 5% | 40% | 2% | 24.5% |
| 63 public 1 | 8.802% | 6.474% | 6.640% | 0.000% | 29.654% | 11.666% | 36.764% |
| 63 public 2 | 9.040% | 6.354% | 6.402% | 0.000% | 30.342% | 11.528% | 36.334% |

Category order in the exact counts below is miss, dodge, parry, block, glancing, crit, hit.

```json
[
  [
    [
      2519,
      2421,
      2436,
      0,
      0,
      5852,
      36772
    ],
    [
      4401,
      3237,
      3320,
      0,
      14827,
      5833,
      18382
    ]
  ],
  [
    [
      2491,
      2584,
      2531,
      0,
      0,
      5809,
      36585
    ],
    [
      4520,
      3177,
      3201,
      0,
      15171,
      5764,
      18167
    ]
  ]
]
```

The public seeds are `0x4c79726100000001` and `0x4c79726100000002`. The first dimension is
seed slot; the second is target level 60 then 63. The exact comparator produces ordinary
`white_melee_distribution` signatures for block, glancing, crit and hit at level 60, and miss,
parry, block, glancing, crit and hit at level 63, for each seed: 20 rule signatures. These are
findings. The Feature Ledger retains them as `known_gaps`; this change does not fix gameplay.

The fixture's SQL reads check every Character's actual `game_world_entity.agility` is 100,
`game_character` race/class/level are 1/1/60, and unarmed/defense skills are both 300/300.
The item and aura tables are empty. Thus the agility input to LyraCore's crit calculation equals
the derivation's agility input; there is no fixture stat gap to correct. The computed chances
differ: `effective_crit_bp` in `module/src/combat/folds.rs` uses the existing
`module/src/combat/tables.rs` functions, giving 1000 + floor(100 * 100 / (60 + 4)) = 1156 basis
points (11.56%) at both target levels. The independent vmangos derivation gives 500 and 200
basis points (5% and 2%). This diagnoses the gameplay formula difference; it does not supply
expected rates or replace delivered-packet evidence. Gameplay remains unchanged.

## Repeatability, cost and strictness

- The first public seed was repeated in four additional fresh Shards with the same inputs.
  All per-Shard counts were identical. The second public seed produced different counts.
- At the released harness pin, the release Module collected both public seeds, 200,000 swings
  total, in **16.815 seconds warm**, with maximum per-Shard setup **5.847 seconds**, excluding
  node startup and Module publication. The final all-17 Tier 1 test invocation took 76.38 seconds
  with the release Module already built. Every count equals the control exactly.
- The release Module collected the same public samples in 26.383 seconds warm
  during the repeatability proof. The maximum per-Shard fixture setup was 8.016 seconds, excluding
  node startup and Module publication. A subsequent restored-source run collected them in
  16.999 seconds warm and reproduced every control count. Cold test execution, including a
  release-Wasm rebuild, took 175.82 seconds in the restored run; it is outside the 30-second warm
  collection budget. CPU and disk contention affect these wall times.
- Release harness controls passed: hand-worked exact binomial tails, probabilities zero and one,
  all seven independently decoded wire categories, conflicting/malformed packet rejection,
  attempted avoidance flags distinct from resolved outcomes, and a shifted comparator input.
  The shifted input transfers 1,000 hit samples to crit and is rejected at both levels.
  A short TCP trace cannot claim a full roll sample, even by copying reference metadata.
  Malformed count overflow was reproduced before its fix and now returns an error.
- Release packet-witness strictness detects 28 mutations and excuses one change in stream
  interleaving, with no unexplained survivor. These witness controls complement the rate controls.
- A real scratch change added 200 basis points to `roll_swing_with_range` crit chance. The same
  delivered-packet collection added the following crit counts, with equal hit reductions and
  every other category unchanged:

  | Seed | Level 60 extra crits | Level 63 extra crits |
  | --- | ---: | ---: |
  | public 1 | 1,008 | 992 |
  | public 2 | 984 | 1,020 |

  The release Module was rebuilt with the mutation, measured, then the source was restored.
  A fresh restored build reproduced the control counts exactly. The mutated crit counts remain
  rejected against the independent fixed probabilities. Since crit already fails in the control,
  this mutation does not create a new grouped signature: the ordinary ratchet cannot detect
  worsening within an already held category. The numerical shift proves the packet path reaches
  the changed gameplay chance; the synthetic passing control proves comparator sensitivity.

The statistical family reserves 42 tests: seven categories, two levels and three seeds.
Each exact equal-tail binomial test rejects at p <= 1e-6 / 42, with a conservative floating-point
allowance. Bonferroni bounds false failure at at most 1e-6 per submission. Fixed seed and build
give a fixed verdict. The merge seed comes from the first 16 hex digits of the merge SHA; a new
signature after merge calls for revert, never rerunning until green. The merge run accepts only
the union of held public category signatures. A held category need not be detected by every new
seed; its absence does not clear the public baseline. A targeted policy control failed under an
exact-signature check and now passes while still rejecting new categories and stale provenance.

## Reference check and remaining scope

One completed vmangos recording retains exactly 2,000 own-attacker swings at each level, from
sixteen Characters. Capture took **508.376 seconds (8m 28.376s)**; fixture setup took
**379.130 seconds (6m 19.130s)**. Together these measured phases took 14m 47.506s, excluding
container startup, Account reset and cleanup. The full valid invocation, from log creation
through its final cleanup write, took about 15m 32.7s. No repeat recording was taken. The independent
classifier and exact diagnostic check accepted both levels:

| Reference target | Miss | Dodge | Parry | Block | Glancing | Crit | Hit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 counts | 95 | 107 | 104 | 86 | 186 | 106 | 1316 |
| 60 rates | 4.75% | 5.35% | 5.20% | 4.30% | 9.30% | 5.30% | 65.80% |
| 63 counts | 148 | 132 | 288 | 122 | 754 | 40 | 516 |
| 63 rates | 7.40% | 6.60% | 14.40% | 6.10% | 37.70% | 2.00% | 25.80% |

This is a small diagnostic for a misread formula, not a precise estimate or a subtle-error test.
Invalid fixture attempts, including one early 20-minute timeout, supplied no statistical evidence.
Readiness checks found moving targets and a starting aura. The final context uses GM Island,
neutral faction 7, idle movement, a pinned spawn home and cleared auras, followed by an unretained
start/stop readiness swing per Character. Every counted sample uses the fixed context. The one
completed valid sample was neither repeated nor selected from multiple completed samples.
The recorder waits or exits busy when another audit owns the lock or ports; it never stops those
containers. Its guard also removes spawned targets on setup failure.

The first slice measures Tier 1 rolls. TCP audits explicitly report a missing roll sample rather
than treating small start/stop witnesses as rate evidence. No stock build-5875 client was run.
The future merge-SHA run cannot be executed before that merge exists. Harness PR #53 merged as
`55252ab4caf50d125c00cffb44969cbf26b892a5` and passed its main CI run. CI published
`v0.2.0-conformance.18` at that exact commit. `.wire-harness-rev`, the workspace `wire-client`
revision and `Cargo.lock` now pin that released commit. The harness checkout is back on `main`.
The durable workflow retains its Markdown, `docs/**` and `.claude/**` push filters from #971;
those pushes cannot change gameplay and do not need another merge-seed measurement.

The new tables, reducers and scheduler batching exist only under `conformance_test_fixture`.
Production random draw count, order and modulo reduction remain unchanged. Existing schedule
callbacks still execute at their deadlines; equal virtual deadlines retain table-kind/row-id order.
The fixture uses a 50 ms attack interval so each ordinary 100 ms melee tick is ready; the
Reference Server retains 2000 ms unarmed attacks. Attack cadence is outside this slice.

Finalization fetched `origin` and confirmed main `ddead53d9cd5c403f03c6a324d2611e49153cdef`
was already merged. `git diff --stat origin/main` was reviewed. All 17 scenarios were replayed
locally at the released harness pin. The first pass completed every comparison with zero property
violations and failed only the expected stale harness provenance checks. `./conformance/baseline`
regenerated all entries from that result. Every signature set is unchanged; only harness provenance
advanced. The final run passed all 17 scenario comparisons and the inactive merge-seed entry point
(18 Rust tests), again with zero property violations. Actual SQL reads verified the pinned inputs
for all 32 Characters. No fixture stat correction or gameplay change was needed.

`cargo test -q --locked -p lyracore-conformance --example scoreboard --example audit` passed
all 73 tests (28 audit, 45 scoreboard). The release tag's source tree is identical to the reviewed
harness commit, so the recorded release strictness proofs apply unchanged.
