# 0007. Measure random rolls against source-derived probabilities

Accepted design, 2026-11-05. Slices: white main-hand melee, direct-damage spells, the queued strike, the cast Weapon Strike, Auto Shot and wand Shoot at levels 60 and 63.

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

## Pinned third context: queued strike

Added 2026-11-23 for issue #1026. The first white-melee context gains Heroic Strike rank 1,
spell 78, queued before every main-hand swing. The Character holds Worn Axe, item 37, in the main
hand with axe skill 44 at 300/300. Humans have no axe racial, and cleared starting auras remove
every racial skill bonus. Heroic Strike requires a main-hand weapon, so the first context's
unarmed hands cannot cast it. Every other input, target and position is the first context's.

The pinned database (`464179081673cfd240f7ffe7f0daf96bca0b5a70`) selects `spell_template` 78
build 4695 for build 5875: attributes `0x50014` (next swing without swing damage, ability, not while
shapeshifted, no sheath), attributes ex `0x08000000`, ex3 `0x400` (main-hand weapon), ex2 and ex4 zero, damage
class 2 (melee), school 0, mechanic 0, range index 2 (combat), equipped item class 2 with subclass
mask 173555, one effect 17 (weapon damage, no school) with base points 10 and one-sided die, rage
cost 150, no cast time, cooldown or global cooldown. `item_template` 37 is a quality 1 one-handed
axe (class 2, subclass 0, main-hand type 21), damage 1 to 3, delay 2000, durability 20. Tier 1
swings the axe every 50 ms and gives it no durability. The roll formulas read neither value; an
unbroken axe keeps the context fixed for 50,000 swings. The Reference Server keeps 2000 ms.

Source links use one-based lines at the pinned commit:

- [SpellCaster.cpp:336–425](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  Heroic Strike lacks the always-hit attribute (line 341), and a spell skips the dual-wield
  penalty (line 350). Miss stays 5% and 8%.
- [SpellCaster.cpp:428–729](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  the spell can crit (line 437). Its combat range keeps weapon skill 300 (line 451). It has no
  mechanic to resist (line 485). Dodge and parry follow the first context: 5%, 5% at level 60 and
  6.5%, 14% at level 63. The table blocks a spell only with the completely-blocked attribute, which
  Heroic Strike lacks (line 546). A Character's next-swing spell can glance (line 548): 10%, 40%.
  Only creatures crush (line 550). Heroic Strike lacks no-active-defense (line 576). Crit comes
  from `GetSpellCritChance` (line 499).
- [Unit.cpp:5212–5307](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp):
  a melee damage class reads `GetUnitCriticalChance`, which at
  [Unit.cpp:2552–2596](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) gives 5% and 2% as in the first context. No spell crit
  aura or spell modifier applies.
- [SpellEntry.h:506–523, 887–890, 1110–1121](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellEntry.h):
  effect 17 can crit and attribute `0x4` marks a next-swing spell.
- [SpellDefines.h:832–965](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellDefines.h) defines the attribute bits read above.

The table is a single `urand(0, 9999)` roll without a block band:

| Target level | Miss | Dodge | Parry | Glancing | Crit | Hit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 | 5% | 5% | 5% | 10% | 5% | 70% |
| 63 | 8% | 6.5% | 14% | 40% | 2% | 29.5% |

A landed strike then rolls partial block separately.
[Spell.cpp:1401–1426](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp) calls
[Unit.cpp:2205–2233](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp), whose melee branch calls `IsSpellPartiallyBlocked`
([Unit.cpp:2351–2395](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp)). The target faces the Character and has no no-block
flag. Block chance is 5% plus 0.1% per skill point the Character's weapon skill falls short of
the target's level maximum, capped at 5%: 5% at level 60, 6.5% capped to 5% at level 63. `roll_chance_f(5.0)`
accepts 214748365 of 2^32 draws, as in the spell slice. The overlapping block probabilities are
`8500*214748365/(10000*2^32)` and `7150*214748365/(10000*2^32)`. Creature block value is level/2
plus strength/20 ([Creature.h:225–228](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Creature.h)), so a block always removes damage.

[Spell.cpp:4947–4992](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp) sends the strike's `SMSG_ATTACKERSTATEUPDATE` before
its damage log. It carries spell 78, the table's hit info and the victim state from
[DamageStructs.h:72–134](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/DamageStructs.h): miss is state 0 with `0x10`, dodge state
2, parry state 3, glancing `0x4000` and crit `0x80` on state 1. A partial block adds `0x800`
([Unit.cpp:1617–1653](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp)) and a positive blocked amount in
`SMSG_SPELLNONMELEEDAMAGELOG`. The next-swing-no-damage attribute leaves the swing's own damage
and blocked fields zero. The classifier in `conformance/support/queued_strike_rolls.rs` reads one
own swing per strike and checks the damage log against it. A missing or contradictory packet blocks
the sample; it is not a rate.

Tier 1 enters through `CMSG_CAST_SPELL` before each swing, the ordinary melee scheduler and
delivered packets, with the white seam stream and public seeds. An unretained readiness check
fires two strikes per Character before collection. It requires the queued spell, an own
`SMSG_SPELL_GO` and no white swing. This context reserves its own family: 1e-6 over seven checks,
two levels and three seeds, 42 tests at `1e-6/42`. Each submission's false-failure bound across
the three contexts is therefore 2e-6. The signatures join `white_melee_outcomes`, whose pinned
context this extends: rule `queued_strike_distribution` with field `["outcomes", category]`, or
`queued_strike_sample` with field `["outcomes"]`. The checkpoint is
`queued_strike_level_<level>_<seed slot>`, the stream `swings`, opcode `0x014A`. The merge seed
measures it with white melee.

No Reference Server recording exists for this context yet, and no harness scenario. A later
harness change can move the specification and classifier there with one recorded check.

## Pinned fourth context: cast Weapon Strike

Added 2026-11-23 for issue #1026. The queued strike's Character, axe, skills and target cast
Mortal Strike rank 1, spell 12294, instead of queueing Heroic Strike. The Character sends no
attack request. Mortal Strike is an instant melee Weapon Strike that is not a next-swing spell, so
it exercises the cast branch of the attack table.

The pinned database selects `spell_template` 12294 build 5086 (`mangos.sql:5882`) over build 4878
(`mangos.sql:5884`) for build 5875: attributes `0x50010` (ability, not while shapeshifted, no
sheath), attributes ex `0x08000400` (no threat, refunds power on a miss), ex3 `0x400` (main-hand
weapon), ex2 and ex4 zero, damage class 2, school 0, mechanic 0, range index 2, equipped item
class 2 with subclass mask 173555, rage cost 300, no cast time, category 971 with a 6000 ms
category cooldown, a 1500 ms global cooldown in category 133 and duration index 1. Effect 1 applies
aura 118 (healing taken, -50%) to the target; the importer stores it as an inert Aura. Effect 2 is
effect 121 (normalized weapon damage) with base points 84 and a one-sided die. Tier 1 casts every
50 ms and drops both cooldowns, as it shortens the axe delay. The roll formulas read neither; the
Reference Server keeps them.

Source links use one-based lines at the pinned commit:

- [SpellCaster.cpp:169–236](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  damage class 2 selects `MeleeSpellHitResult`, after evade, immunity and reflection checks that
  do not apply here (lines 221–233).
- [SpellCaster.cpp:732–749](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  the attack type is the main hand (lines 735–736), and the table's crit outcome is the cast's crit
  (lines 742–743). [Spell.cpp:910–923](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp) keeps that result
  and does not roll crit again.
- [SpellCaster.cpp:336–425](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  Mortal Strike lacks the always-hit attribute (line 341), and a spell skips the dual-wield penalty
  (line 351). Miss stays 5% and 8%.
- [SpellCaster.cpp:428–729](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp):
  the spell can crit (line 437). Its combat range keeps weapon skill 300 (line 451). It has no
  mechanic to resist (line 487). Dodge and parry follow the first context: 5%, 5% at level 60 and
  6.5%, 14% at level 63. The table blocks a spell only with the completely-blocked attribute,
  which Mortal Strike lacks (lines 546–547). Only a next-swing spell can glance (lines 548–549),
  and only creatures crush (lines 550–551). Mortal Strike lacks no-active-defense (line 577).
  Crit comes from `GetSpellCritChance` (lines 504–508).
- [Unit.cpp:5212–5316](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp):
  a melee damage class reads `GetUnitCriticalChance` (lines 5295–5302), which at
  [Unit.cpp:2552–2595](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) gives 5% and 2% as in the first
  context. No spell crit aura or spell modifier applies.
- [SpellEntry.h:506–525, 887–890, 1110–1121](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellEntry.h):
  effect 121 can crit, and neither next-swing attribute is set.
- [SpellDefines.h:190–214, 828–981](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellDefines.h) define the
  miss reasons and the attribute bits read above.

The table is a single `urand(0, 9999)` roll with no glancing, block or crushing band:

| Target level | Miss | Dodge | Parry | Crit | Hit |
| --- | ---: | ---: | ---: | ---: | ---: |
| 60 | 5% | 5% | 5% | 5% | 80% |
| 63 | 8% | 6.5% | 14% | 2% | 69.5% |

A landed cast then rolls partial block separately, exactly as the queued strike does.
[Spell.cpp:1369–1429](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp) calls
[Unit.cpp:2205–2233](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp), whose melee branch calls
`IsSpellPartiallyBlocked` ([Unit.cpp:2351–2395](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp)). The
block chance is 5% at both levels, and the overlapping block probabilities are
`8500*214748365/(10000*2^32)` and `7150*214748365/(10000*2^32)`.

vmangos reports every outcome through the Spell path. [Spell.cpp:4529–4561, 4625–4653](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp)
sends one `SMSG_SPELL_GO` to the caster and its observers: a landed target is in the hit list, an
avoided target is in the miss list with reason 1 (miss), 3 (dodge) or 4 (parry). A landed cast then
sends `SMSG_SPELLNONMELEEDAMAGELOG` (Spell.cpp:1429), with crit flag `0x02` and a positive blocked
amount for a partial block ([SpellCaster.cpp:1080–1096, 1253–1258](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp)).
Only a next-swing spell sends `SMSG_ATTACKERSTATEUPDATE` (Spell.cpp:1289–1290, 1424–1426).
`SMSG_SPELLLOGMISS` reports immunity and evasion (Spell.cpp:1614, 1643, 1700) and a resisted damage
shield ([Unit.cpp:1797](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp)), never a miss, dodge or parry. The classifier
in `conformance/support/weapon_strike_rolls.rs` reads one own `SMSG_SPELL_GO` per cast and the
damage log of a landed cast. A missing, extra or contradictory packet blocks the sample; it is not
a rate.

Tier 1 enters through `CMSG_CAST_SPELL`, ordinary cast admission and resolution, and delivered
packets, with the caster seam stream and the public seeds. Each Character's caster stream starts
from the first context's per-stream state. An unretained readiness cast per Character requires an
accepted cast and an own `SMSG_SPELL_GO`. This context reserves its own family: 1e-6 over six
checks, two levels and three seeds, 36 tests at `1e-6/36`. Each submission's false-failure bound
across the four contexts is therefore 3e-6. The signatures join `white_melee_outcomes`, whose
pinned context this extends: rule `weapon_strike_distribution` with field `["outcomes", category]`,
or `weapon_strike_sample` with field `["outcomes"]`. The checkpoint is
`weapon_strike_level_<level>_<seed slot>`, the stream `casts`, opcode `0x0132`. The merge seed
measures it with white melee.

No Reference Server recording exists for this context yet, and no harness scenario. Ranged Weapon
Strikes, which roll only miss and crit, need their own context.

## Pinned fifth and sixth contexts: ranged swings

Added 2026-11-23 for issue #1026. Two auto-repeat ranged swings each get a context: Auto Shot, spell
75, from a gun, and Shoot, spell 5019, from a wand. Both Characters are level-60 humans with agility
100, defense and unarmed skill at 300/300, an empty main hand and no talents, auras or hit and crit
bonuses. The Auto Shot Character is the first context's warrior. It holds gun 2508 in the ranged
slot, bullets 2516 in its backpack, and guns skill 46 at 300/300. Auto Shot is a hunter spell, and a
warrior fires a gun with Shoot Gun, spell 7918. The warrior casting spell 75 is deliberate and
rate-neutral: it keeps the first context's Character, and for a gun the rolls read the class only
through its crit rate. The wand Character is a priest, class 5, because vmangos gives a wand its
weapon's school only for wand-using classes. It holds wand 11287, with wands skill 228 at 300/300.
Each target is the first context's creature, passive so that it never closes to melee, ten yards
ahead for Auto Shot and six for the wand; no roll formula reads distance.

The pinned database selects `spell_template` 75 build 5086 (`mangos.sql:5882`) over build 4222
(`mangos.sql:5887`): attributes `0x50012` (ranged slot, ability, not while shapeshifted, no
sheath), ex2 `0x20` (auto-repeat), ex3 `0x8000` (normal ranged attack), damage class 3 (ranged),
school 0, mechanic 0, range index 114, speed 40, equipped item class 2 with subclass mask 262156
(bow, gun, crossbow) and one effect 58 (weapon damage). It selects 5019 build 5086
(`mangos.sql:5882`) over builds 4449 and 4222: attributes `0x12`, ex2 `0x20`, ex3 `0x408000`
(normal ranged attack, casting cancels auto-repeat), damage class 1, school 0, mechanic 0,
category 351, range index 4, speed 20, subclass mask 524288 (wand) and one effect 17 (weapon
damage, no school). Neither has the always-hit, cannot-crit, no-active-defense or
completely-blocked attribute. `item_template` 2508 (`mangos.sql:2696`) is a quality 1 gun, damage
2 to 5, delay 2300, durability 20, ammo type 3. Item 2516 (`mangos.sql:2696`) is bullets, damage 1
to 2, stack 200. Item 11287 resolves to its patch 7 row (`mangos.sql:2702`): quality 2, arcane
damage 12 to 22, delay 1500, durability 35. `player_crit_per_agility` (`mangos.sql:3897`) gives
rate 20 for both class 1 and class 5 at level 60. Tier 1 fires the gun every 300 ms and the wand
every 400 ms, gives neither durability and gives the ammo a 10,000-round stack. Each delay
outlasts its projectile's flight, so one shot flies at a time. The roll formulas read none of
these values; the Reference Server keeps the pinned ones.

Source links use one-based lines at the pinned commit:

- [Unit.cpp:2733–2778](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp): each ready auto-repeat shot casts its spell
  as a triggered Spell.
- [Spell.cpp:60–71](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp) and
  [SpellEntry.cpp:434–455](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellEntry.cpp): both spells take the ranged attack type,
  Auto Shot through damage class 3 and Shoot through its auto-repeat attribute. A priest's Shoot
  takes the wand's arcane school.
- [SpellCaster.cpp:169–236](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp): a ranged attack with the normal
  ranged attack attribute rolls as damage class ranged (lines 215–224), so both reach
  `MeleeSpellHitResult` after evade, immunity and reflection checks that do not apply.
- [SpellCaster.cpp:732–749](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp): the attack type is ranged (lines
  734–736) and the table's crit is the shot's crit. [Spell.cpp:910–923](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp)
  keeps it.
- [SpellCaster.cpp:336–425](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp): a spell skips the dual-wield penalty
  (line 350). Ranged skill 300 against defense 300 and 315 gives miss 5% and 8%, as in the first
  context.
- [SpellCaster.cpp:428–729](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp): both spells can crit (line 437) and
  require a weapon, so they use the ranged weapon skill (line 451). They have no mechanic to
  resist. A ranged attack rolls no dodge, parry, block, glancing or crushing band (lines 532–541).
  Crit comes from `GetSpellCritChance` (lines 499–509).
- [Unit.cpp:5212–5316](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp): Shoot's ranged attack type selects the ranged
  branch (lines 5234–5237), and both read `GetUnitCriticalChance` (lines 5295–5302) with no school
  crit aura or spell modifier.
- [Unit.cpp:2552–2595](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) reads the ranged crit percentage and subtracts
  0.2% per missing skill point against a creature: 3% at level 63.
- [StatSystem.cpp:531–587](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/StatSystem.cpp) and
  [Player.cpp:5146–5150](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Player.cpp): ranged crit is agility over the class rate, plus
  3% for a priest (lines 563–564) and no skill term at 300/300. Agility 100 over 20 gives 5% for
  the warrior and 8% for the priest.

Each table is a single `urand(0, 9999)` roll with only miss and crit bands:

| Context | Target level | Miss | Crit | Hit |
| --- | --- | ---: | ---: | ---: |
| Auto Shot | 60 | 5% | 5% | 90% |
| Auto Shot | 63 | 8% | 2% | 90% |
| Shoot | 60 | 5% | 8% | 87% |
| Shoot | 63 | 8% | 5% | 87% |

A landed Auto Shot then rolls partial block separately. [Spell.cpp:1415](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp)
calls [Unit.cpp:2205–2233](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp) with the main-hand attack type. Its ranged and
melee damage-class branch calls `IsSpellPartiallyBlocked`
([Unit.cpp:2351–2395](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp)). The target faces the Character and a creature's
block chance is 5% ([Unit.cpp:2515–2550](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp)). The skill term reads the
Character's unarmed skill, so the chance is 5% at level 60 and 6.5% capped to 5% at level 63.
The overlapping block probabilities are `9500*214748365/(10000*2^32)` and
`9200*214748365/(10000*2^32)`. Shoot has magic damage class, so it never rolls block; its context
expects a zero block probability. LyraCore rolls the block in `roll_ranged_swing` after the weapon
draw, from a spell row with damage class ranged only, with the main-hand skill, and the damage log
reports the blocked amount. Partial resistance of the arcane Shoot is not part of the attack
table; `ranged_impact` reports its resisted amount, the classifier accepts a positive one and
measures no resist rate.

A cast ranged Weapon Strike (Aimed Shot, Multi-Shot, Throw) takes the same partial block through
the same Spell.cpp:1415 branch, unless its spell has `SPELL_ATTR_NO_ACTIVE_DEFENSE` or
`SPELL_ATTR_EX3_COMPLETELY_BLOCKED` (`IsSpellPartiallyBlocked`). LyraCore rolls it in
`launch_ranged_strike` from the Caster stream, after the cast's hit roll and the effect's points
draw. The strike's own damage roll is `ctx.random`, not a Caster draw. The blocked amount goes to
the impact. vmangos rolls the block at impact, LyraCore at launch, so a block that follows a launch
state change differs. No Roll Conformance context measures a ranged cast strike: the cast Weapon
Strike context is melee Mortal Strike, so Tier 1 signatures do not move.

A ranged hit is a Spell in vmangos, so `CreateProcExtendMask`
([Unit.cpp:8776–8830](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp))
raises `PROC_EX_BLOCK` beside the hit's quality (`PROC_EX_NORMAL_HIT` or `PROC_EX_CRITICAL_HIT`),
and `PROC_EX_ABSORB` beside the quality for an absorb. Only a white melee full block raises
`PROC_EX_BLOCK` alone. `ranged_impact` runs the unlanded proc pass when a shot lands with no
damage, so a fully blocked or fully absorbed ranged hit raises those bits; the launch folds the
absorbed amount into the impact schedule. A shot that misses or meets an immune target has no
impact, so `fire_ranged_shot` raises its `PROC_EX_MISS` or `PROC_EX_IMMUNE` at launch, after the
GO row, where vmangos raises them at impact. A cast strike's miss runs with the cast.

vmangos reports every shot through the Spell path. [Spell.cpp:4529–4561, 4625–4653](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/Spell.cpp)
sends one `SMSG_SPELL_GO` at launch, with ammo for a ranged spell: a landed target is in the hit
list, a missed target in the miss list with reason 1. A landed shot sends
`SMSG_SPELLNONMELEEDAMAGELOG` on impact (Spell.cpp:1429), with crit flag `0x02` and a positive
blocked amount for a partial block ([SpellCaster.cpp:1080–1096](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/SpellCaster.cpp)). Only a
next-swing spell sends `SMSG_ATTACKERSTATEUPDATE` (Spell.cpp:1289, 1425). LyraCore sends the same
packets: `combat_event` in `module/src/native/combat.rs` writes the GO and its miss list, and
`ranged_impact` in `module/src/combat/swing.rs` logs the damage. The classifier in
`conformance/support/ranged_swing_rolls.rs` pairs each own GO with the damage log of a landed shot,
which can arrive in a later batch. A missing, extra or contradictory packet blocks the sample; it
is not a rate.

Tier 1 enters through `CMSG_CAST_SPELL`, which starts the auto-repeat, then the ordinary melee
scheduler, ranged impact schedule and delivered packets, with the white seam stream and the public
seeds. Each batch advances one weapon delay. An unretained readiness check requires two counted
shots per Character; a shot still in flight counts in the sample. Each context reserves its own
family: 1e-6 over four checks, two levels and three seeds, 24 tests at `1e-6/24`. Each submission's
false-failure bound across the six contexts is therefore 5e-6. The signatures join
`white_melee_outcomes`: rule `auto_shot_distribution` or `wand_shot_distribution` with field
`["outcomes", category]`, or the `_sample` rule with field `["outcomes"]`. The checkpoint is
`<context>_level_<level>_<seed slot>`, the stream `casts`, opcode `0x0132`. The merge seed measures
both with white melee.

No Reference Server recording exists for these contexts yet, and no harness scenario. Thrown
weapons, bows and crossbows use the same table and remain unmeasured.
