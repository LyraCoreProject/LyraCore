# 0007. Measure random rolls against recorded outcome counts

Proposed, 2026-11-04. Implementation stops at the maintainer decision below.

[ADR 0005](0005-1-12-conformance.md) checks random fields' shape and range. A server that always
crits can satisfy that contract. Roll Conformance adds a distribution check over real gameplay
outcomes, using recorded vmangos counts. It supplements packet checks and keeps vmangos as the
only Reference Server.

## Maintainer decision

Approve a maintainer-owned verifier for a blinded seed, or explicitly accept public seeds alone.
Recommend the verifier. The [trusted ratchet](../../.github/workflows/conformance-ratchet.yml)
deliberately executes only base code and reads PR baselines as data. It has no blinded verifier.
Giving a secret seed to a PR-controlled test would expose it and permit tuning; executing PR code
in that trusted job would break its trust boundary.

Freeze the submitted Module artifact and head SHA before choosing the blinded seed. A trusted
controller runs the artifact in an isolated disposable Shard, without repository credentials or
network egress, using a base-owned runner and reviewed harness revision. It keeps seeds and logs
private until the submission closes, then releases the counts and verdict. A changed head needs
a fresh challenge. Attest artifact hash, head, runner and harness revisions. Maintainers must
choose the owner, isolation, required-check policy and retry policy. Do not put a repository secret
in an ordinary PR job. No current policy authorizes this new verifier.

Two public fixed seeds give local feedback; a third, chosen after submission, prevents tuning a
rate to one known sample. Challenges repeat deterministically, without retry-until-green. This
impedes seed tuning, not arbitrary malicious code: independent review must still catch fixture
detection and fabricated packets. Public seeds alone give weaker assurance and must be named as
such if selected. This decision belongs to the maintainer before implementation proceeds.

The contract below is proposed, not measured. Inspected bases: core
`e3207d491b05c159008d1da3114ddb3c252ea6c1`, harness
`1fcc4ec9c739fd3f6cee7584b846054db189ced1`. Neither has a swing-distribution scenario.

## Reference recording and cost

Start with level-60 Character main-hand white swings against level-60 and level-63 creatures,
from the front. Count miss, dodge, parry, block, glancing, crit and hit separately per level.
Pin numeric race, class, creature entry, equipment, capped weapon skill and defense, orientation,
position and avoidance-enabling state. Use no offhand, queued strike, proc, buff or extra attack.
Verify state before and after capture; skill gains, deaths, equipment breaks, changing context
and ambiguous outcomes invalidate capture. Zero-count categories remain tested.

Record exactly 200,000 eligible swings per level, then an independent checked repeat of the same
size. The first recording is authority; the repeat checks setup and stationarity. Never choose
whichever recording fits core. At a true 5% rate, 200,000 independent draws give approximate
marginal 95% precision of +/-0.096 percentage points; at 50%, +/-0.219 points. These describe
sampling precision, not pass tolerances or simultaneous intervals.

The harness recorder independently decodes `SMSG_ATTACKERSTATEUPDATE`, selects the attacking
Character and bound target, and streams seven integer counts. Reject malformed packets,
conflicting flags, duplicate observations and missing samples. Retain per-Character totals and
1,000-swing block counts to expose changing state; aggregate only verified identical contexts.
Store sample count, duration, scenario SHA-256, classifier version and existing vmangos binary,
source and database provenance. Hash all text through the existing scrubber. The artifact holds
thousands of integers instead of hundreds of thousands of packets. Only the recorder writes it,
in the harness repository. Source-derived probabilities are not recorded evidence.

Extend `reference.sh record-checked`, run under `newgrp docker`, with a bounded streaming count
checkpoint. Its current 4,096-frame checkpoint limit and 180-second window cannot carry this
capture. Keep small attack-start/stop packet witnesses and ordinary packet checks; do not raise
the packet cap or weaken decoding. The new format explicitly records aggregate evidence; existing
Golden Traces keep their packet contract. The scenario hash covers all statistical parameters.

Including the repeat, there are 800,000 reference swings. At an assumed two-second cadence:
444 Character-hours, or 27.8 wall hours with 16 Characters in parallel, plus setup. Schema 4
supports 16 Characters; concurrent starts and drains must be added. Give each its own target and
count no observer copies. This is a one-time recording, never a developer check.

A verified 50-ms cadence would reduce that estimate to 41.7 minutes with 16 Characters, including
the repeat. This is not a measured or approved shortcut. Before using GM attack-speed setup,
prove at the pinned vmangos source and in packet capture that it changes scheduling only, leaves
the attack table and skills unchanged, and produces normal combat packets. Otherwise use ordinary
cadence. Do not patch vmangos or synthesize outcomes. Report actual setup, capture and repeat times.

## Core measurement and path evidence

Use tier 1's virtual clock. The current code establishes the required production path:

1. [`native/world.rs`](../../module/src/native/world.rs) dispatches `CMSG_ATTACKSWING` to
   [`native/combat.rs::attack`](../../module/src/native/combat.rs), starting the melee attack.
2. [`time.rs`](../../module/src/time.rs) dispatches the virtual melee schedule to production
   [`combat/swing.rs::tick_melee`](../../module/src/combat/swing.rs).
3. That tick runs `melee_pass`, `resolve_swing` and `fire_melee_swing`, calls
   [`combat/folds.rs::roll_swing`](../../module/src/combat/folds.rs), applies damage and inserts
   the normal `CombatEvent`.
4. `native/combat.rs::combat_event` builds and delivers `SMSG_ATTACKERSTATEUPDATE`. The harness's
   independent decoder classifies these delivered bytes, outside the Module.

[`debug_compute_swing` and `debug_compute_spell`](../../module/src/debug/readout.rs) write rate
and damage readouts. They never roll or apply a swing. Direct calls to `attack_outcome` or
`roll_swing` also skip gameplay. None can supply this evidence.

Collect 50,000 swings per level for each of two public seeds and one blinded seed: 300,000
candidate swings, compared separately by level and seed. Batch repeated 50-ms scheduler advances
inside bounded transactions; drain normal delivered packets between batches and stop at the exact
count. Preserve admission, schedule ordering, range/facing checks and state changes. Never trust
a Module-supplied histogram. A small witness checks admission, landed damage and attack stop.
Missing or malformed samples fail; infrastructure failures are not rate findings.

The warm budget is 30 seconds for both public seeds together, excluding build/publication.
This is an acceptance budget, not a benchmark. Report cold setup and warm collection separately.
The current transport's WebSocket round trip per 50-ms advance is unsuitable. If batching misses
the budget, optimize that path; do not substitute a calculation or silently reduce the sample.

## Determinism

The virtual clock changes `time::now`, not `ReducerContext.timestamp`. SpacetimeDB 2.14.0's
`ReducerContext::random` seeds its private `StdbRng` from that transaction timestamp; no seed setter
is public for a deployed WASM Module. Fixed virtual start time does not fix today's swing draws.

Add a narrow random-input Seam where `roll_swing_with_range` obtains outcome, weapon and block
draws. Production retains existing draws and range reduction. Under `conformance_test_fixture`
only, supply a specified seeded PRNG with durable state and a stream per attacking Character.
Pin its algorithm/version and draw order; never override chances or resolved outcomes. Operator
seed setup resets with the disposable Shard. Control or exclude every other reachable random
effect that can change the context; capped skills and equipment alone do not prove stationarity.

Prove identical counts in two fresh Shards using the same seed/input, a different-seed check,
and detection of a gameplay-rate mutation through delivered packets. Review production and fixture
paths together: only their random input may differ in roll resolution. Structural checks support
that review but do not prove malicious code honest. These execution checks remain unperformed.

## Statistical test

For each category use a two-sided Fisher exact test on reference and candidate rows
`[category_count, total - category_count]`, with the probability-ordered definition in the
[R statistics manual](https://stat.ethz.ch/R-manual/R-devel/library/stats/html/fisher.test.html).
This includes reference uncertainty and handles sparse or zero counts. Use deterministic
hypergeometric calculation, no Monte Carlo, and conservative numerical bounds at the threshold.
Check worked tables and extreme counts against independently computed expected results.

Allocate false failure at most `1e-6` to one reference check plus one frozen candidate submission:
14 reference-repeat comparisons and 42 candidate comparisons. Reject each at `p <= 1e-6 / 56`,
about `1.786e-8`. Bonferroni needs no independence between categories or tests sharing reference
counts. The sampling model does require independent draws within stationary contexts; block
diagnostics do not prove that assumption. Do not discard inconvenient blocks. More contexts or
seeds require a new family allocation.

This bound concerns reference sampling and initial seed selection under equal underlying rates.
Fixed reference, seed, build and fixture give the same verdict on every rerun: no runtime sampling
flake. It is not a lifetime bound across unlimited submissions or a guarantee of detecting every
small shift. Do not search for a passing seed, redraw after failure, or rerecord to make core pass.

## Ledger, ratchet and strictness

Register `white_melee_outcomes` for `combat/auto_attack`, extending that Behavior's scope and source
evidence. Add its tier 1 entry, advance both harness pins together, and generate the baseline with
`conformance/baseline`. Keep gameplay fixes with another implementer; record differences as
`known_gaps`. The scenario author does not change gameplay to obtain a pass.

Emit ordinary `DifferenceClass::Rule` records: rule `white_melee_distribution`, checkpoint by level
and seed slot, broadcast stream `swings`, combat opcode and field path `["outcomes", category]`.
Counts, rates, p-values and signed shifts belong in the report, outside grouping signatures.
Existing signature extraction and ratchet hold these failures; the scoreboard needs no new
denominator or data classification. An existing failure signature does not bound further rate
worsening; expose that limitation in the numerical report. Blinded results attest the frozen head
separately from the public Tier 1 baseline and must be required if that assurance is selected.

Run strictness from a RELEASE build. Make a passing comparator control by proportionally scaling
reference counts to 50,000 with deterministic integer rounding; this is never measured evidence.
Transfer 1,000 hit samples to crit, keeping the total and category shapes valid. Require that
2-percentage-point shift to produce distribution rule signatures at both levels, not a shape or
sample-count failure. Also shift core's actual crit chance by that amount in scratch and require
the delivered-packet measurement to detect it; restore afterward. The first check proves comparator
sensitivity, the second proves gameplay-path sensitivity. Neither has run yet.

## Scope and handoff

Spell misses, resists, procs, damage distributions, glancing damage penalties, offhand/rear attacks,
other levels/skills, PvP and gameplay corrections stay out of this slice. Counts do not establish
all melee parity. Commit no client files, Reference Server data, bulk world rows or unscrubbed text.

After the maintainer decision, implement the scenario and needed Fixture Verbs in both adapters,
record through the checked recorder, then add tier 1 and release strictness. Run only targeted
checks and this scenario. Install SpacetimeDB 2.14.0 under task scratch without changing the default.
Keep PRs draft until evidence is complete; the orchestrator reviews, merges and tags. This design
establishes no measured LyraCore-versus-vmangos distribution, recording cost or tier 1 runtime.
