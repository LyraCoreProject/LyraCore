# 0007. Measure random rolls against source-derived probabilities

Accepted design, 2026-11-05. Slices: white main-hand melee and direct-damage spells at levels 60 and 63.

[ADR 0005](0005-1-12-conformance.md) checks random fields' shape and range. A server that always
crits can satisfy that contract. Roll Conformance compares delivered gameplay outcomes with fixed
probabilities derived from vmangos. vmangos remains the only Reference Server.

## Authority and independence

For roll rates only, this is a deliberate, scoped exception to ADR 0005's recordings-only rule.
Derive expected probabilities exactly from the attack-table and spell-roll code at vmangos commit
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

The crit, block and glancing findings assume the Tier 1 fixture matches the pinned inputs: agility 100, skill 300, unarmed.

## Pinned second context: direct-damage spells

A level-60 human mage (race 1, class 8) casts Scorch rank 1, spell 2948. Intellect is 125.
All equipment slots are empty. There are no talents, auras, hit bonuses, crit bonuses, spell
penetration or procs. The target is creature 38, normal rank, non-pet, type 7, class 1, at level
60 or 63. Fire resistance is zero; all extra and static flags are zero. Both participants stand
still, alive, on map 1 at (16222.1, 16252.1, 12.5872). The target stands two yards ahead and faces
the Character. Each Character has a separate target. Faction 7, idle movement and passive reaction
prevent interference; these inputs do not enter the roll formulas. Starting auras are removed.
Both health pools and the Character's maximum mana are 1,000,000,000. There are no resource
restores during collection. The Reference Server retains Scorch's ordinary 1500-ms cast time and
1500-ms global cooldown. Casts start at least 1600 ms apart; cadence is outside this slice.

Scorch has one direct-damage effect, no projectile delay, periodic damage, applied aura, mechanic,
proc or cooldown beyond the global cooldown. Unlike Frostbolt, it requires no spell aura exception.
It exercises the actual level-difference hit formula and innate partial resistance without gear or
talents. Partial resistance is included because its rate can be derived exactly.

The pinned database is `464179081673cfd240f7ffe7f0daf96bca0b5a70`, archive SHA-256
`9e178f1d70363e92a7b8c259000102dc9739333d79ca00c99ea32dd1fb43e904`. Its `mangos.sql:4077`
contains human mage level-60 create stats `(1,8,60,30,35,45,125,126)`. The fixture sets the additive
intellect modifier to zero and checks delivered intellect 125. `mangos.sql:5879`, `spell_template`
entry 2948/build 5464, supplies school 2, damage class 1, attributes 65536, all extended attributes
zero, effects [2,0,0], no mechanic or aura, cast-time index 16, speed zero and start recovery 1500.
The current build selects that row. The target and its non-combat EventAI context follow the first
slice. The recorder checks its actual fire resistance and maximum health through GM readback before
and after collection. Character level, race/class, intellect, equipment, auras, mana, health, target
level/faction and stationary positions are checked through independently decoded update fields.
Invalid or incomplete context fails setup; it is not a rate finding.

### Spell derivation

All source references below use one-based lines at commit
`d9173eca3ea08dd985c52dabbcc826898d1d0b77`, fetched through raw GitHub URLs only.

- [SpellCaster.cpp:169–237](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp) selects the magic hit table for
  damage class 1, after evade, immunity and reflection checks. Those modifiers are absent here.
- [SpellCaster.cpp:772–880](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp) uses target-minus-caster level
  difference `d`, PvE coefficient 11, and base hit `96-d` percent for `d < 3`, otherwise
  `94-(d-2)*11` percent. All spell, aura, mechanic, debuff and hit modifiers are zero. Scorch is
  non-binary. Hit thresholds are 9600 and 8300. `irand(0,10000)` is inclusive and returns
  `SPELL_MISS_RESIST` for rolls below `10000-hit`. Thus full-resist probabilities are exactly
  `400/10001` and `1700/10001`; landed probabilities H are `9601/10001` and `8301/10001`.
  `SPELL_MISS_MISS` has probability zero for this context.
- [Random.cpp:28–31, 62–65](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/shared/Utilities/Random.cpp),
  [Random.h:58–62](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/shared/Utilities/Random.h), and
  [MersenneTwister.h:159–163, 202–220](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/dep/include/mersennetwister/MersenneTwister.h)
  define inclusive integer sampling and `roll_chance_f(chance)` as `chance > rand_chance()`.
  The latter compares against exactly `100*U/2^32`, with U uniform on 0 through `2^32-1`.
- [Unit.cpp:2597–2662](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) gives mage crit as
  `3.70f + intellect/(14.77f + 0.65f*level)`, with no aura bonuses. At intellect 125 and level 60,
  single-precision arithmetic gives denominator `53.77000045776367` and chance
  `6.024716377258301` percent (bits `0x40c0ca7a`, exactly `3158685/524288`). The strict comparison
  accepts `ceil(chance*2^32/100) = 258759599` integers. The exact crit probability C is therefore
  `258759599/4294967296`, rather than a rounded basis-point rate.
- [Unit.cpp:5212–5321](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) reads that school crit value, with no creature
  level suppression for magic damage. Target auras, class scripts and spell modifiers are absent.
  [Spell.cpp:900–923](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp) rolls hit and crit separately. Delivered crit
  probability is H*C; ordinary hit probability is H*(1-C).
  [SpellCaster.cpp:1230–1275](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp) carries the crit result into the delivered damage flag;
  Scorch has no ignore-caster-modifiers attribute.
- [SpellMgr.cpp:3342–3394](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellMgr.cpp) makes Scorch non-binary because its
  only effect is direct damage. [SpellEntry.h:506–523, 1110–1122](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellEntry.h)
  allows that effect to crit; the cannot-crit attribute is absent.
- [SpellCaster.cpp:882–924](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp) adds innate creature resistance
  `trunc(8*d*60/63)` to zero fire resistance and zero penetration. This is 0 or 22. Multiplication
  by `0.15f/60`, then by 100 in the resistance roll, yields 0 or approximately 5.50000048 percent.
- [Unit.cpp:1885–1918, 2397–2469](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) interpolates between resistance
  rows at chance 5 and 8. At level 63 the cumulative thresholds are exactly 1 for 75% resistance,
  between 5 and 6 for at least 50%, and between 18 and 19 for at least 25%. Against integer rolls
  0 through 99 this gives widths 1, 5, 13 and 81 for 75%, 50%, 25% and zero resistance. The
  conditional partial-resist probability is exactly 19/100; it is zero at level 60. Damage is
  always positive and large enough for every nonzero quarter to remain positive after dithering.
  [Unit.cpp:1920–1955, 2205–2233](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) applies this roll after a landed
  non-binary cast. Unconditional partial-resist probability is H*19/100 at level 63.
- [Spell.cpp:223–254](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Server/Packets/Spell.cpp) defines the independently decoded
  `SMSG_SPELLNONMELEEDAMAGELOG` layout. [SpellDefines.h:190–229](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellDefines.h)
  defines critical flag 0x02. Positive resisted damage marks partial resistance. `SMSG_SPELL_GO`
  identifies hit targets or full-resist reason 2; reason 1 is a distinct miss. Absorb, block,
  immunity, reflection, negative resistance, other spells and changed targets fail setup.
- [UnitCommands.cpp:1502–1534, 2327–2369](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Commands/UnitCommands.cpp),
  [Unit.cpp:7890–7905](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp),
  [CreatureCommands.cpp:845–872](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Commands/CreatureCommands.cpp), and
  [DebugCommands.cpp:1223–1269](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Commands/DebugCommands.cpp) define intellect, mana,
  passive reaction and private update-field readback used only during fixture preparation/checks.
  [UpdateFields_1_12_1.h:47–100](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/UpdateFields_1_12_1.h) defines the checked
  health, mana, race/class, intellect and fire-resistance words.

| Target level | Miss | Full resist | Crit | Ordinary hit | Partial resist (overlapping) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 60 | 0 | 400/10001 | (9601*258759599)/(10001*4294967296) | (9601*4036207697)/(10001*4294967296) | 0 |
| 63 | 0 | 1700/10001 | (8301*258759599)/(10001*4294967296) | (8301*4036207697)/(10001*4294967296) | (8301*19)/(10001*100) |

Partial resistance overlaps ordinary hits and crits. Only the first four columns sum to one.
The TOML stores exact rational numerators and denominators. Expectations never come from the
small recording or from LyraCore calculations.

## Small Reference Server check: white melee

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

PRs run public fixed seeds `0x4c79726100000001` and `0x4c79726100000002`. Every gameplay-relevant push to `main`
runs a third seed derived deterministically from the merge commit SHA, which does not exist before
merge. It runs in the durable job that already runs tier 1. No secret or isolated controller is
needed. Interpret the first 16 hexadecimal SHA digits as a big-endian u64 seed. A failure is found just after merge and handled by revert. Never retry until green.
A fixed seed, fixture and build give a fixed verdict: there is no runtime sampling flake.
The durable workflow retains its push path filters for Markdown, `docs/**` and `.claude/**`; those pushes cannot change gameplay.

Use an exact equal-tail binomial test for each category against its fixed probability:
p = min(1, 2 min(P[X <= observed], P[X >= observed])). This is a conservative two-sided exact
test for discrete counts. Sum binomial masses relative to the mode, avoiding underflow, and add
a conservative floating-point allowance before comparing with the rejection threshold. Include
hand-worked tails and probabilities zero and one in targeted tests. No Monte Carlo or two-sample
Fisher test.

Reserve a family false-failure bound of at most 1e-6 per submission over seven white-melee
categories plus five spell checks, two levels and three seeds: 72 tests. Reject each at
p <= 1e-6 / 72 (about 1.389e-8). Partial resistance overlaps spell hits and crits. Bonferroni
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
report that limitation. Public fixed seeds keep the exact baseline check. The merge run uses
the union of held public signatures, normalized by level and category, and fails on additions.
A different seed can fail to detect a known wrong rate; that absence does not clear a gap or
justify reverting unchanged gameplay. New signatures still require revert, without retry.
Record observed gameplay differences as `known_gaps`, without fixing them.

Run strictness in release builds. A synthetic passing control has each expected basis-point
interval multiplied by five to total 50,000. Transfer 1,000 hit samples to crit; require crit
rate signatures at both levels. This control is never measured evidence. Also make a real scratch
change of +2 percentage points to crit chance, measure delivered packets, require detection and
restore the source. Do not change gameplay in this PR.

Procs, damage distributions, glancing damage penalties, offhand/rear attacks, other spells,
levels/skills and PvP remain outside these slices. The counts do not establish all
melee parity. TCP audits currently report an explicit missing-roll-sample signature; their
small packet witnesses do not claim to measure these rates. Keep PRs draft while iterating, then ready; the orchestrator reviews and merges.
Do not merge or tag. Use only targeted checks and this scenario, SpacetimeDB 2.14.0 in task scratch,
and leave the default CLI unchanged.


## Small Reference Server check: spells

Record one completed check with 2,000 casts per target level: sixteen Characters, eight per
level, 250 casts each, five 50-cast blocks per Character. Keep ordinary 1500-ms Scorch casts
and start them at least 1600 ms apart. Use the shared Reference Server lock. An unretained
readiness cast checks each fixture; it supplies no second rate recording. Record setup and
capture time, classifier version and existing binary/source/database provenance. Retain one
cast-start witness and integer counts; scrub text and discard the packet stream. Only the
recorder writes this Golden Trace. The diagnostic checks for a misread formula and does not
estimate the expected probabilities or enter the submission verdict.

The harness collector binds each trial to its Character, target and spell. It requires one
1500-ms `SMSG_SPELL_START`, one `SMSG_SPELL_GO`, and one delivered damage log for a landed cast.
It counts full resists from `SMSG_SPELL_GO`, crits/hits from the damage log and positive resisted
damage as an overlapping partial resist. Refused, duplicate, missing or ineligible outcomes
fail setup. Observer copies cannot add samples. There is no server-supplied histogram.

Register `spell_direct_damage_outcomes` under `spells/cast_and_interrupt` after the harness PR
merges. Tier 1 must enter through `CMSG_CAST_SPELL`, cast admission, ordinary cast scheduling,
damage application and delivered spell packets. It collects 50,000 casts per level for each of
the two public seeds, with a third derived from the merge SHA on main. Use the fixture-only
random Seam in `module/src/conformance_rolls.rs`, retain production draws, and prove repeatability
and gameplay-rate sensitivity through delivered packets. Core fixture implementation and its
measurement report follow the reviewed harness release in one pin/baseline PR.

Emit ordinary `DifferenceClass::Rule` signatures: rule `spell_distribution`, checkpoint by level
and seed slot, stream `casts`, opcode `0x0250`, field `outcomes/<check>`. Record every measured
difference in `known_gaps`, preserving the numerical report and the held-signature limitation
from the first slice. The same public-signature union governs the merge seed. A gameplay
blocker is a finding for a separate implementer. Generic TCP audits report `spell_sample` until
full candidate collection exists; copied reference metadata and short witnesses prove no rate.

### Single replacement exception, 2026-11-05

The first completed spell diagnostic collected 2,000 casts per level and passed its source-derived
rate check. Counts are ordered miss, full resist, crit, ordinary hit, overlapping partial resist:

| Target level | Miss | Full resist | Crit | Ordinary hit | Partial resist |
| --- | ---: | ---: | ---: | ---: | ---: |
| 60 | 0 | 87 | 124 | 1789 | 0 |
| 63 | 0 | 316 | 97 | 1587 | 314 |

Fixture setup took 473,572 ms and capture took 463,490 ms. The author edited the running
`reference.sh` wrapper. When it resumed, a shell read failed; its failure cleanup deleted the
accepted Golden Trace. The sample was lost to that wrapper edit, not rejected on its results.
These aggregate counts survive in the task record; the deleted trace is not reconstructed.

The maintainer approved exactly one replacement capture of 2,000 casts per level as a single
exception to the rule above, "once, without a repeat recording". The context, expected rates,
public seeds and measurement code remain fixed. No script may be edited while capture runs.
A launcher copies the recorder-accepted bytes to a separate path before returning control to
wrapper cleanup. The replacement is the recorded sample regardless of how its results compare
with the lost sample. Report both samples. If this replacement fails or is lost, stop; no third
capture is authorized. This exception does not change the general single-recording rule.

The single approved replacement completed and passed its diagnostic. It is the recorded sample:

| Target level | Miss | Full resist | Crit | Ordinary hit | Partial resist |
| --- | ---: | ---: | ---: | ---: | ---: |
| 60 | 0 | 77 | 114 | 1809 | 0 |
| 63 | 0 | 357 | 100 | 1543 | 324 |

Replacement setup took 464,310 ms; capture took 454,361 ms. All 59 frozen source/input hashes
matched after the checked wrapper exited zero. The launcher copied and synced the accepted
12,433-byte trace before wrapper cleanup; both copies have SHA-256
`958c3f88f01d229477a7376bad28881fdba5c3d31e3bcd62f68d62533d97239f`.
No script changed during capture. No third recording was taken.

### Core spell measurement

The core fixture translates Scorch's additional pinned numeric inputs from `mangos.sql:5879`:
cost 50, range 30, spell/max level 22/26, family 3 and flags 1073741840, proc flags/charges zero
and proc chance 101. Its one damage effect uses source base points 52 plus the offset of one,
die sides 13, per-level points 0.9 and coefficient 0.429. Equipment, talents and auras remain
absent; target reaction is explicitly passive and checked before collection.

The first valid-context attempt at `fa737462` stopped at cast 3 against level 60 under public
seed `0x4c79726100000001`. A diagnostic replay reproduced two completed ordinary hits, followed
by START 1500 ms, a pending cast with one pushback, no pending impact, and mage health
999,999,998 instead of 1,000,000,000. Passive-target retaliation blocks the pinned context.
No gameplay correction or probability adjustment is made. The [spell report](../../conformance/reports/2026-11-05-spell-rolls.md)
records this finding. An earlier fixture attempt lacked passive state; its new pre-cast check
failed before any cast, and the fixture now supplies that already pinned input.

Emit ordinary `spell_sample` Rule signatures for blocked level/seed samples and retain them in
`known_gaps`. Do not test or report partial counts as rates. Full-rate repeatability and gameplay
chance sensitivity remain unmeasured until a separate implementer repairs the gameplay blocker.
The allocation still reserves all 72 tests. Main replays the merge seed once, without retry.
