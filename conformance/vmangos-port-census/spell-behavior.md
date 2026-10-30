# Extra spell columns

Source definitions and behavior were read at vmangos commit
`464179081673cfd240f7ffe7f0daf96bca0b5a70`. The implementation is independent Rust.
The definition is `src/game/Spells/SpellDefines.h`, enum `SpellAttributesCustom`.
`SpellEntry.cpp`, `Spell.cpp`, `SpellAuras.cpp`, `SpellCaster.cpp` and `Unit.cpp` supply the behavior.

The Module appends `custom_flags`, `min_target_level` and `single_target_key` to `game_spell`,
and `bonus_coefficient` to `game_spell_effect`. Migration defaults are 0, 0, 0 and -1. The extra
key preserves the source's family/icon comparison and its Judgement and Polymorph categories
for single-target auras. It packs the icon in bits 0..31, family in 32..39, and category in 40..47.
Seal and blessing classification precede Judgement, including overlapping family masks.
No existing field moves.

| Bit | Source name | Selected spells | Behavior and check |
| --- | --- | ---: | --- |
| 0x0001 | ALLOW_STACK_BETWEEN_CASTER | 24 | Casters share and refresh one stack. Native `spell-source-behavior` verifies two casters produce one aura with two stacks. |
| 0x0002 | NEGATIVE | 4 | Forces debuff polarity. The native check verifies the debuff slot range. |
| 0x0004 | POSITIVE | 5 | Forces buff polarity and wins over NEGATIVE. Unit and native checks cover both bits together. |
| 0x0008 | CHAN_NO_DIST_LIMIT | 0 | The initial range Gate still applies. Only the channel target supplies the distance check, not other area recipients. Accepted channels, including auras without periodic effects, can remain outside range. The native check compares bounded and unlimited channels. |
| 0x0010 | FIXED_DAMAGE | 2 | Damage and healing ignore caster bonuses, including explicit coefficients. Critical hits and target mitigation remain. Unit and native checks cover direct and periodic magnitudes. |
| 0x0020 | IGNORE_ARMOR | 0 | Physical spell damage and weapon strikes skip armor mitigation. The native check compares fixed physical damage with and without this bit. |
| 0x0040 | BEHIND_TARGET | 0 | At cast start and completion, the caster must be behind the target. Triggered spells and channel ticks bypass this check. |
| 0x0080 | FACE_TARGET | 1016 | At cast start and completion, a Character caster must face the target. Of these spells, 416 have a cast time. Native checks cover completion Refusals and turning during triggered 7268 missiles. |
| 0x0100 | SINGLE_TARGET_AURA | 111 | A successful application removes conflicting auras from this caster on other targets. Unit tests cover family/icon and category comparisons; the native check moves the aura between targets. |
| 0x0200 | AURA_APPLY_BREAKS_STEALTH | 1 | Successful application removes the target's other stealth auras. The native check verifies removal. |
| 0x0400 | NOT_REMOVED_ON_EVADE | 95 | Evade preserves this aura and finite positive auras cast by players. The native check compares player buffs, permanent buffs, debuffs and creature buffs. |
| 0x0800 | SEND_CHANNEL_VISUAL | 17 | Held under decision 11. The Module has no SpellVisual channel-kit catalogue or repeating visual-kit delivery. |
| 0x1000 | SEPARATE_AURA_PER_CASTER | 3 | Different casters get separate aura rows and slots. The native check verifies both. |
| 0x2000 | TRIGGER_WEAPON_PROCS | 30 | Held under decision 11. Chance on Hit honors it for spell hits that are not Triggered Casts. The Module has no weapon-enchant dispatcher. |

The held bits remain in the catalogue and the manifest reports 47 affected entries. Their
behavior stays unsupported. Unknown bits fail the source reader.

The source has 24,689 nonnegative effect coefficients, including unused effect slots.
An explicit coefficient scales the caster's spell or healing power without the automatic
coefficient's level penalty. Zero suppresses that bonus. Negative coefficients keep the
Module's existing fallback. Periodic effects freeze the scaled amount when the aura is applied.
The fallback is not a claim that every existing Module spell formula matches vmangos.

Minimum target levels occur on three spells, 20631 at 50, 2639 at 5, and 6114 at 8. The cast Gate
runs before power is charged. Its typed Refusal is `TargetLevelTooLow`; the native check pins
client result 0x2b and confirms that refusal leaves power and auras unchanged.

Classic spell SQL keeps its prior bytes. The creature DELETE changes the full import output as
recorded in [the revised classic baseline](classic-baseline-m3.md). At the SQL execution boundary, the importer fills
only missing appended fields with their defaults because SpacetimeDB SQL requires all columns.
The importer and Rust test support call one shared rewriter. Python fixtures derive defaults from
`crates/lyracore-shared/src/import_sql_defaults.txt`, the same list the Rust rewriter reads.
The pinned `lyracore` CLI invokes the importer with `--apply`, so it reaches that rewriter. Hand-running legacy SQL with the SpacetimeDB
CLI requires those appended fields. Old Package Deltas remain valid without rewriting their
bytes. New Datascript clones carry the extra fields. The Fire Nova fixture changed only by
adding the four defaults to its claims; its source hash and all earlier claims are unchanged.

Physical direct spell damage previously bypassed armor unconditionally. The ordinary path now
applies armor, and IGNORE_ARMOR selects the bypass. Ordinary periodic auras from different casters
can coexist; cross-caster stacking requires its flag. Range checks also reject a different map or
Instance when a spell's range is zero. These are gameplay changes, independent of SQL byte parity.

## Periodic triggers on both World Sources

At base `a56bce7d`, `A_PERIODIC_TRIGGER` in `scheduler.rs` called `resolve_cast_at` with
`is_completion = false`. Each tick checked caster life and action state, stance, caster level,
school lockout, global and spell cooldowns, range and faction targeting. It also ran effect-specific
Gates, charged power, dismounted the caster, broke stealth unless permitted, started cooldowns and
allowed ordinary hit Procs. It did not start a cast timer.

That route did not call `check_targeted_los`. LOS was checked when the holder was admitted and for
pending cast completion, not for the periodic child. Thus the claim that this stack removed a
periodic-child LOS check was incorrect.

The stack routes periodic children through `cast_triggered`. They skip child range, facing,
cooldown and power-cost checks, consume no power, start no cooldown, do not dismount or break
stealth, and produce triggered hits that cannot fire further Procs. The restored faction-target
Gate still refuses an ally-targeted effect on a hostile unit and an enemy-targeted effect on a
friendly unit. Damage also retains its `may_harm` Gate. The scheduler still ends periodic triggers
when the caster cannot act or their retained target is dead; effect execution retains its target-life
check. These changes also apply to classic-db Realms because the scheduler is shared.

The relevant vmangos checks at
[Spell.cpp, 4641790](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Spells/Spell.cpp)
agree on triggered cooldown bypass at lines 5312 and 5328, LOS bypass at 5481, faction targeting
outside that bypass at 5590-5613, and range bypass at 5703. Ordinary completion calls
`CheckCast(false)` at 3662 and reaches `CheckRange` with facing at 6938. This establishes those
Gates, not complete conformance of every triggered effect.

The native check uses small, hand-authored 5143 and 7268 fixtures with the pinned source's relevant
flags, target, period and coefficient. It observes two Arcane Missile spell-go packets after the
caster turns away. Synthetic periodic tests also exercise faction changes, cost, cooldown and
child range. The existing channel-holder range check remains, including for Arcane Missiles.
`CastRefusalKind` remains exhaustive, without `#[non_exhaustive]`, as requested.
