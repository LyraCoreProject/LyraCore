# White main-hand melee: first Roll Conformance slice

Measured on 2026-11-05. [ADR 0007](../../docs/adr/0007-statistical-roll-conformance.md) gives the
independent vmangos derivation, every formula input and pinned source lines. This author derived
rates from that source and changed only conformance fixtures and measurement infrastructure.
No gameplay correction is included. vmangos is the only Reference Server.

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

## Repeatability, cost and strictness

- The first public seed was repeated in four additional fresh Shards with the same inputs.
  All per-Shard counts were identical. The second public seed produced different counts.
- With the final context guards, the release Module collected both public seeds, 200,000 swings
  total, in **16.223 seconds warm**, with maximum per-Shard setup **5.954 seconds**, excluding
  node startup and Module publication. That cold test invocation, including the Module rebuild,
  took 185.50 seconds. The counts equal the control exactly.
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
The future merge-SHA run cannot be executed before that merge exists. The harness PR must merge
and receive its automatic release tag before the core smoke pin can name a published release.
The current `.wire-harness-rev` is explicitly an unpublished review pin; replace its anticipated
tag and branch commit with that tested release before merging core.

The new tables, reducers and scheduler batching exist only under `conformance_test_fixture`.
Production random draw count, order and modulo reduction remain unchanged. Existing schedule
callbacks still execute at their deadlines; equal virtual deadlines retain table-kind/row-id order.
The fixture uses a 50 ms attack interval so each ordinary 100 ms melee tick is ready; the
Reference Server retains 2000 ms unarmed attacks. Attack cadence is outside this slice.

The other Tier 1 baseline entries were regenerated with `./conformance/baseline` from CI evidence
at merge commit `b4704a2dde79c35f2c640f4d19004d55a4dbd6aa` ([durable run](https://github.com/LyraCoreProject/LyraCore/actions/runs/42001719730)).
All 17 comparisons completed without property violations. Unrelated scenarios were not replayed
locally. Their signature sets are unchanged; this refresh advances their harness provenance.
