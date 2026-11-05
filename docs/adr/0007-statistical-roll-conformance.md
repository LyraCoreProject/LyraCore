# 0007. Measure random rolls against source-derived probabilities

Accepted design, 2026-11-05. First slice: white main-hand melee at levels 60 and 63.

[ADR 0005](0005-1-12-conformance.md) checks random fields' shape and range. A server that always
crits can satisfy that contract. Roll Conformance compares delivered gameplay outcomes with fixed
probabilities derived from vmangos. vmangos remains the only Reference Server.

## Authority and independence

For roll rates only, this is a deliberate, scoped exception to ADR 0005's recordings-only rule.
Derive expected probabilities exactly from the attack-table code at vmangos commit
`d9173eca3ea08dd985c52dabbcc826898d1d0b77`. Read GitHub raw URLs; do not clone vmangos. Record each
formula input, source path and line. The person deriving rates must not fix gameplay for this
Behavior and must not derive expectations from LyraCore's code. Rate differences are findings,
reported as ordinary rule signatures and `known_gaps`; a different implementer handles gameplay.

## Pinned first context and derivation

Use a level-60 human warrior (race 1, class 1), standing, with agility 100, defense 300 and
unarmed skill 162 at 300/300. Every equipment slot is empty, including both hands. Unarmed attacks
are main-hand white melee; the Reference Server retains its ordinary 2000 ms cadence. No talents, buffs, auras, procs,
queued strikes, extra attacks or hit/crit bonuses. No GM attack-speed changes.

Targets are spawned creature entry 38, type 7 humanoid, class 1, rank 0, faction 7,
`static_flags1 = static_flags2 = flags_extra = 0`, non-pet, standing, not stunned, casting or
evading, with no no-parry/no-block/no-defense flags, no auras and normal rank. Set levels to 60
and 63: defense and maximum skill are 300 and 315. Each Character has a separate target, two
yards directly ahead, facing the Character. Both health pools are 1,000,000,000; no deaths or
gear breakage can change the context. Set spawned movement to idle and move the spawn back to
its pinned home before setting level and health. Clear starting auras, including stances. Reference positions start on map 1 at
(16222.1, 16252.1, 12.5872), Character orientation 0; reference pairs share these positions. Tier 1 offsets pairs by minus 200 yards on the y axis
to avoid observer packet cost; their relative position and facing stay identical.
The Reference Server temporarily changes each spawned target from faction 17 to neutral faction 7,
which build-5875 `FactionTemplate.dbc` gives zero faction-group, friend and enemy masks
(SHA-256 `40fe48125299c89d9e9c0169810c000c863aaa22000ae2bb073b55bc8f2036cc`). This
avoids ambient aggression. The GM Island position also excludes nearby guard interference. The attack-table formulas do not read
faction. Reject missing packets, changed skills, levels, stats, position, facing or attack context.

The pinned database dump supplies entry 38 at `mangos.sql:1424`: type 7, class 1, rank 0,
zero extra/static flags, faction 17 and movement type 1. The fixture overrides faction and movement
as above. Its EventAI entries at `mangos.sql:606,649` only say text or emote out of combat; they
add no hit-table modifier.

Source links below use one-based lines at the pinned commit:

- [SpellCaster.h:284–286](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.h):
  maximum skill and creature melee skill are level multiplied by five.

- [SpellCaster.cpp:116–158](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  empty main hand selects unarmed skill; creatures use their melee skill for defense.
- [SpellCaster.cpp:336–425](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  base miss 5%, no offhand penalty or hit bonus. With skill difference d = 300 - defense,
  miss is 5 - 0.1d for d >= -10, otherwise 5 - 0.2d. Results: 5%, 8%.
- [SpellCaster.cpp:446–468, 582–624](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  dodge is 5 - 0.1d; parry is 5 - 0.2d for d >= -10, otherwise 5 - 0.6d.
  Results: dodge 5%, 6.5%; parry 5%, 14%.
- [SpellCaster.cpp:625–679](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  glance is min(40, 10 - 2d)%; creature block is min(5, 5 - 0.1d)%.
  Results: glance 10%, 40%; block 5%, 5%.
- [Unit.cpp:2474–2550](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp):
  standing, non-casting humanoid creature base dodge, parry and block are each 5%.
- [Player.cpp:5146–5150](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Player.cpp),
  [StatSystem.cpp:531–587](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/StatSystem.cpp),
  [ObjectMgr.cpp:5087–5124, 5332–5343](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/ObjectMgr.cpp):
  crit is agility / class rate, with no warrior class offset or skill adjustment at 300/300.
  Database `464179081673cfd240f7ffe7f0daf96bca0b5a70`, `player_crit_per_agility`
  (class=1, level=60) supplies rate 20: `mangos.sql:3897`, tuple `(1,60,20)`, in the pinned
  `db-4641790.zip` archive (SHA-256
  `9e178f1d70363e92a7b8c259000102dc9739333d79ca00c99ea32dd1fb43e904`). Agility 100 / 20 gives 5%.
- [Unit.cpp:2552–2596](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp):
  creature defense subtracts 0.2% per missing skill from crit: 5%, 2%.
- [SpellCaster.cpp:428–729](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  uniform integer roll 0..9999 uses cumulative miss, dodge, parry, glance, block, crit, hit.
  These contexts do not clip a category. Hit is the remaining probability; crushing is impossible.

| Target level | Miss | Dodge | Parry | Block | Glancing | Crit | Hit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 | 5% | 5% | 5% | 5% | 10% | 5% | 65% |
| 63 | 8% | 6.5% | 14% | 5% | 40% | 2% | 24.5% |

Classification follows
[DamageStructs.h:74–82, 115–134](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/DamageStructs.h):
crit is hit-info bit `0x80`, block `0x800`, glance `0x4000`; victim states 2, 3 and 5 denote
dodge, parry and block. Bits `0x100`, `0x200` and `0x400` denote attempted avoidance rolls,
not resolved outcomes.

These are integer interval widths over 10,000 possible rolls, not estimated rates. The first
[measurement report](../../conformance/reports/2026-11-05-white-melee.md) records delivered counts,
strictness and cost; differences remain findings.

## Small Reference Server check

Record about 2,000 eligible swings per level once, without a repeat recording. Sixteen parallel
Characters, eight per level with 250 swings each, target about eight minutes at ordinary cadence,
plus setup. If parallel setup costs too much, use fewer and accept a longer run under about 30
minutes. Report the actual setup and recording times. Never stop another audit's containers;
use the shared Reference Server lock and wait or retry when busy.

The check confirms that the derived rates match vmangos in practice. It catches a misread formula,
not a subtle error. It does not estimate the expected probabilities. No seven-hour or 28-hour
recordings, repeated samples or GM attack-speed shortcut are authorized.

The recorder independently decodes `SMSG_ATTACKERSTATEUPDATE`, counts only the bound attacker's
own target, and retains per-Character counts plus 50-swing blocks. Keep small attack start/stop
packet witnesses. Each Character first passes an unretained preparation start/stop swing check;
this is a fixture readiness check, not a repeat rate recording. Store elapsed time, scenario SHA-256, classifier version and existing pinned
binary/source/database provenance in the harness. Scrub all text. Never retain the large packet
stream or trust a server-supplied histogram. Invalid context or incomplete capture is a setup
failure, not evidence against a rate.

## Gameplay path and deterministic random input

Tier 1 enters through `CMSG_ATTACKSWING`, native attack admission, `tick_melee`, `roll_swing`,
damage application and delivered `SMSG_ATTACKERSTATEUPDATE`. The harness's independent decoder
classifies the delivered bytes. `debug_compute_swing` never rolls or applies a swing and cannot
supply evidence. Direct calls to a calculation or Module-produced counts cannot supply it either.

Collect 50,000 swings per target level per seed. Tier 1 pins the fixture-only attack interval to
50 ms so each ordinary 100 ms melee tick has a ready swing. This changes scheduling input only; the
pinned `RollMeleeOutcomeAgainst` formula never reads attack interval. The Reference Server retains
ordinary 2000 ms attacks. No haste aura or GM attack-speed command is used. This slice checks
roll rates, not attack cadence. Batch ordinary scheduler advances in bounded
transactions and drain delivered packets between batches. Equal deadlines retain the existing
virtual-clock order of table kind then row id; live scheduling does not promise this order.
Preserve schedule order, admission,
range and facing checks. Check attack start, damage and stop with small witnesses. Missing or
malformed samples fail setup. The warm budget for both public seeds together is 30 seconds,
excluding build/publication; report setup and warm collection separately. Do not reduce the sample
or substitute calculations to meet it.

The virtual clock does not seed `ReducerContext::random`. Add a narrow Seam at
`roll_swing_with_range`'s outcome, weapon and block draws, gated by `conformance_test_fixture`.
Use durable SplitMix64 state per attacking Character, with specified stream ids and draw order:
outcome, weapon, block, each the low 32 bits of the next SplitMix64 output. Stream ids are
0..15, eight per level, starting from seed XOR (stream * 0xd1b54a32d192ed03), with wrapping
64-bit multiplication. Each of two fresh Shards per level holds four Characters. Keep production draws
and modulo range reduction unchanged. Seed setup is Operator-only and resets in each disposable
Shard. Exclude other random effects that can change this context.

Prove identical counts in two fresh Shards with the same seed and input, different counts for a
different seed, comparator sensitivity and actual gameplay-rate sensitivity through packets.

## Seeds and exact test

PRs run public fixed seeds `0x4c79726100000001` and `0x4c79726100000002`. Every push to `main`
runs a third seed derived deterministically from the merge commit SHA, which does not exist before
merge. It runs in the durable job that already runs tier 1. No secret or isolated controller is
needed. Interpret the first 16 hexadecimal SHA digits as a big-endian u64 seed. A failure is found just after merge and handled by revert. Never retry until green.
A fixed seed, fixture and build give a fixed verdict: there is no runtime sampling flake.

Use an exact equal-tail binomial test for each category against its fixed probability:
p = min(1, 2 min(P[X <= observed], P[X >= observed])). This is a conservative two-sided exact
test for discrete counts. Sum binomial masses relative to the mode, avoiding underflow, and add
a conservative floating-point allowance before comparing with the rejection threshold. Include
hand-worked tails and probabilities zero and one in targeted tests. No Monte Carlo or two-sample
Fisher test.

Reserve a family false-failure bound of at most 1e-6 per submission over seven categories, two
levels and three seeds: 42 tests, reject each at p <= 1e-6 / 42 (about 2.381e-8). Bonferroni
needs no independence across categories or tests. Within a stationary context the sampling model
requires independent draws; block counts do not prove that assumption. The bound concerns seed
selection across submissions, not a lifetime bound over unlimited submissions. More contexts or
seeds require a new family allocation. The small reference check is diagnostic and is reported
separately from this submission verdict.

## Signatures and strictness

Register `white_melee_outcomes` for `combat/auto_attack`, with its tier 1 test and harness pin.
Emit ordinary `DifferenceClass::Rule` records: rule `white_melee_distribution`, checkpoint by
level and seed slot, broadcast stream `swings`, opcode `0x014A`, field `["outcomes", category]`.
The ordinary signature baseline and ratchet retain failures. Counts, rates, p-values and shifts
remain in the numerical report. A held signature does not bound further worsening of that rate;
report that limitation. The merge run maps its category signatures to both public seed slots
before the same baseline check, so the held gaps apply without a seed-specific baseline.
Record observed gameplay differences as `known_gaps`, without fixing them.

Run strictness in release builds. A synthetic passing control has each expected basis-point
interval multiplied by five to total 50,000. Transfer 1,000 hit samples to crit; require crit
rate signatures at both levels. This control is never measured evidence. Also make a real scratch
change of +2 percentage points to crit chance, measure delivered packets, require detection and
restore the source. Do not change gameplay in this PR.

Spell misses, resists, procs, damage distributions, glancing damage penalties, offhand/rear
attacks, other levels/skills and PvP remain outside this slice. The counts do not establish all
melee parity. TCP audits currently report an explicit missing-roll-sample signature; their
small packet witnesses do not claim to measure these rates. Keep PRs draft while iterating, then ready; the orchestrator reviews and merges.
Do not merge or tag. Use only targeted checks and this scenario, SpacetimeDB 2.14.0 in task scratch,
and leave the default CLI unchanged.
