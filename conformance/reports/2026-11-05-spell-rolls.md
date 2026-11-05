# Direct-damage spell Roll Conformance, 2026-11-05

This is Scenario author evidence for `spells/cast_and_interrupt`. Expected rates are derived
only from the pinned vmangos source in [ADR 0007](../../docs/adr/0007-statistical-roll-conformance.md).
No LyraCore gameplay formula, chance, cast admission or damage handling is corrected.

## Context and expected rates

A level-60 human mage, race 1/class 8, has intellect 125 and no equipment, talents, auras,
hit/crit bonuses, penetration or procs. Scorch rank 1 (2948) targets creature 38 at levels 60
and 63, with fire resistance zero and passive reaction. Both participants stand two yards apart,
alive, on map 1 at the GM Island position. Tier 1 separates pairs by 200 yards on the y axis.
Health pools and mage mana are 1,000,000,000. The spell keeps its ordinary 1500-ms cast and
1500-ms global cooldown. The collector starts trials before sending the pinned scenario's
`CMSG_CAST_SPELL` bytes and advances the ordinary schedules by 1600 ms per trial.

The diagnostic fixture translates pinned `mangos.sql:5879` entry 2948/build 5464: cost 50,
range 30, school mask 4, attributes 65536, spell/max level 22/26, family 3, family flags
1073741840, proc flags/charges zero and proc chance 101. The single damage effect has authored
text, internal kind/target 1/1, base points 53 (source 52 plus its offset of one), die sides 13,
per-level points 0.9 and coefficient 0.429. These are numeric fixture inputs, not copied source
rows or expected-rate estimates.

Let H be `9601/10001` or `8301/10001`, from the inclusive magic hit roll. Let C be
`258759599/4294967296`, from the mage intellect formula evaluated as single precision and the
exact 32-bit random comparison. Miss is zero, full resist is `1-H`, crit is `H*C` and ordinary
hit is `H*(1-C)`. At level 63, innate resistance 22 gives 19 resisting integer outcomes out of
100, so overlapping partial resistance is `H*19/100`; it is zero at level 60.

| Category | Expected level 60 | LyraCore level 60 | Expected level 63 | LyraCore level 63 |
| --- | ---: | --- | ---: | --- |
| Miss | 0% | Unmeasured | 0% | Unmeasured |
| Full resist | 3.999600% | Unmeasured | 16.998300% | Unmeasured |
| Crit | 5.783752% | Unmeasured | 5.000617% | Unmeasured |
| Ordinary hit | 90.216648% | Unmeasured | 78.001083% | Unmeasured |
| Partial resist | 0% | Unmeasured | 15.770323% | Unmeasured |

Partial resistance overlaps hit and crit. The submission allocation is `(7+5)*2*3 = 72`,
with exact equal-tail rejection at `1e-6/72`. Incomplete samples never enter those tests.

## First valid-context result

The first valid-context run at `fa737462` stopped at the third cast for a level-60 target
under public seed `0x4c79726100000001`. The mage had completed two ordinary hits, with counts
`[0,0,0,2,0]` ordered miss/full-resist/crit/hit/partial. These are diagnostic witnesses, not a
rate sample. No full 50,000-cast sample exists and no rate verdict is claimed.

One diagnostic replay retained the same result. Only the third START was delivered; it declared
spell 2948 and duration 1500 ms. SQL readback found a pending cast with `pushback_count = 1`,
no pending spell impact, and mage health 999,999,998. The target health was 999,999,851. The
pre-cast check had required target reaction state zero. Passive targets still damage the mage
and delay casts, so the pinned context is blocked by gameplay. This is a finding for a separate
implementer. The sampling window, expected probabilities and gameplay remain unchanged.

The early fixture attempt omitted reaction state and also stopped at a third cast. Its added
pre-cast control failed on that setup with `spell target has no passive reaction state` before
sending any cast. The fixture now supplies the already specified passive input and checks it
before and after collection. That correction supplied no completed statistical sample and did
not tune a rate. 

## Recorded findings and verification limits

The collector represents complete and blocked samples separately. A blocked sample emits an
ordinary `DifferenceClass::Rule` record, rule `spell_sample`, checkpoint by target level and seed,
broadcast stream `casts`, opcode `0x0250`, field `outcomes`. Its count is diagnostic progress
toward 50,000, not a probability estimate. The Feature Ledger keeps this blocker in `known_gaps`.
The generator retains these signatures under the normal new-scenario measurement.


Both public seeds stopped at the third cast in every Shard part. The recorded progress totals
16 completed outcomes per level/seed, from eight Characters. Counts below are diagnostics only:

| Seed | Target level | Miss / full resist / crit / hit / partial | Verdict |
| --- | ---: | --- | --- |
| public 1 | 60 | `[0, 1, 0, 15, 0]` | Blocked; rates unmeasured |
| public 1 | 63 | `[0, 4, 0, 12, 0]` | Blocked; rates unmeasured |
| public 2 | 60 | `[0, 0, 0, 16, 0]` | Blocked; rates unmeasured |
| public 2 | 63 | `[0, 2, 0, 14, 0]` | Blocked; rates unmeasured |

Delivered opcode `0x014A` identifies each target creature as the attacker and its mage as the
victim. This confirms retaliation, alongside the pending cast's pushback counter. NPC damage
uses ordinary randomness, so later diagnostic health values may differ. The first valid-context
and diagnostic-replay values above remain the original observations.

Four signatures are retained: `spell_sample` at `level_60_public_1`, `level_63_public_1`,
`level_60_public_2` and `level_63_public_2`. Every signature has stream `broadcast:casts`,
opcode `0x0250` and field `outcomes`. No `spell_distribution` rate signature is asserted.

Full-rate repeatability, changed-seed rate behavior and a two-point gameplay-chance sensitivity
proof cannot be established while the context is blocked. No chance mutation was made. The
harness's released exact-tail, classifier and shifted-input controls remain separate evidence.
The core control `blocked_spell_samples_never_claim_rate_evidence` checks that partial progress
produces missing-sample findings and cannot become binomial rate evidence.

The main workflow replays this scenario once with the first 16 hexadecimal digits of the merge
SHA, after the public runs. It uses the union of held public signatures, normalized by level and
seed slot, through the existing white-melee policy. A new signature calls for revert, without
retry. This future merge sample cannot run before the merge exists.

Harness PR [#55](https://github.com/LyraCoreProject/wire-harness/pull/55) merged as
`004d8d740dbbc5d06548b6710d6ede0f682c3807`. Every main CI job passed and CI published
`v0.2.0-conformance.19` at that commit. All three core pins advance together. The only accepted
Reference Server replacement is the 12,433-byte Golden Trace with SHA-256
`958c3f88f01d229477a7376bad28881fdba5c3d31e3bcd62f68d62533d97239f`; ADR 0007 retains both
reference samples and the single approved exception. There was no new Reference Server capture.

No stock client or live Realm was used. Fixture tables and reducers remain behind
`conformance_test_fixture`. Unconfigured Characters and ordinary builds retain their production
random calls, count, order and modulo reduction. Existing packet witnesses and this blocked
sample do not establish spell-roll parity. A held missing-sample signature cannot bound a rate
or establish that a later gameplay change produces a complete sample.

## Final baseline and focused verification

The branch was rebased onto main `e210e089`, including vendor #1005 and Character-deletion
#1007. The new missing-sample control passed. `./conformance/run spell_direct_damage_outcomes
--json` then exited zero: the four held findings matched, `conforms` remained false and rates
remained unmeasured. The two new evidence-validation controls also passed. They accept only
pinned spell levels, seed slots, categories, rational expected means and bounded counts. The
sample control first failed before its validation fix. The three audit tests that CI reported
then passed individually after the compatibility update; no replay or Markdown guard was removed.

CI run [42111970478](https://github.com/LyraCoreProject/LyraCore/actions/runs/42111970478)
completed all 18 scenario comparisons with zero property violations. Every rule and data
signature set matched the committed sets. Its tested merge commit
`4defe8c674308a660d2d6ebb9c2e454c8b605f46` has the same source tree as PR head `e31e9ee2`.
The unmodified Tier 1 report has SHA-256
`94946f0dccb860336ab32ca329a3a690e18156c2b593c66a00e3857836560af4`.

`./conformance/baseline` regenerated the complete baseline from that report while checked out
at the tested commit, then the task branch was restored. Only the 17 existing harness revisions
changed; the new scenario already used conformance.19. No signature or scenario hash changed.
No generated report or baseline was hand-edited. The remaining fast-CI compatibility fixes
change evidence validation and its tests, not the Module or comparator outcomes.
