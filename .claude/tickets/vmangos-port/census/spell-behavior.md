# Extra spell columns

Source definitions and behavior were read at vmangos commit
`464179081673cfd240f7ffe7f0daf96bca0b5a70`. The implementation is independent Rust.
The definition is `src/game/Spells/SpellDefines.h`, enum `SpellAttributesCustom`.
`SpellEntry.cpp`, `Spell.cpp`, `SpellAuras.cpp`, `SpellCaster.cpp` and `Unit.cpp` supply the behavior.

The Module appends `custom_flags`, `min_target_level` and `single_target_key` to `game_spell`,
and `bonus_coefficient` to `game_spell_effect`. Migration defaults are 0, 0, 0 and -1. The extra
key preserves the source's family/icon comparison and its Judgement and Polymorph categories
for single-target auras. It packs the icon in bits 0..31, family in 32..39, and category in 40..47.
No existing field moves.

| Bit | Source name | Selected spells | Behavior and check |
| --- | --- | ---: | --- |
| 0x0001 | ALLOW_STACK_BETWEEN_CASTER | 24 | Casters share and refresh one stack. Native `spell-source-behavior` verifies two casters produce one aura with two stacks. |
| 0x0002 | NEGATIVE | 4 | Forces debuff polarity. The native check verifies the debuff slot range. |
| 0x0004 | POSITIVE | 5 | Forces buff polarity and wins over NEGATIVE. Unit and native checks cover both bits together. |
| 0x0008 | CHAN_NO_DIST_LIMIT | 0 | The initial range Gate still applies. Accepted channels, including auras without periodic effects, can remain outside range. The native check compares bounded and unlimited channels. |
| 0x0010 | FIXED_DAMAGE | 2 | Damage and healing ignore caster bonuses, including explicit coefficients. Critical hits and target mitigation remain. Unit and native checks cover direct and periodic magnitudes. |
| 0x0020 | IGNORE_ARMOR | 0 | Physical spell damage and weapon strikes skip armor mitigation. The native check compares fixed physical damage with and without this bit. |
| 0x0040 | BEHIND_TARGET | 0 | The caster must be behind the target. The native check covers both orientations. |
| 0x0080 | FACE_TARGET | 1016 | A player caster must face the target. The native check covers both orientations. |
| 0x0100 | SINGLE_TARGET_AURA | 111 | A successful application removes conflicting auras from this caster on other targets. Unit tests cover family/icon and category comparisons; the native check moves the aura between targets. |
| 0x0200 | AURA_APPLY_BREAKS_STEALTH | 1 | Successful application removes the target's other stealth auras. The native check verifies removal. |
| 0x0400 | NOT_REMOVED_ON_EVADE | 95 | Evade cleanup preserves the aura. The native check leashes a creature and compares ordinary and preserved auras. |
| 0x0800 | SEND_CHANNEL_VISUAL | 17 | Held under decision 11. The Module has no SpellVisual channel-kit catalogue or repeating visual-kit delivery. |
| 0x1000 | SEPARATE_AURA_PER_CASTER | 3 | Different casters get separate aura rows and slots. The native check verifies both. |
| 0x2000 | TRIGGER_WEAPON_PROCS | 30 | Held under decision 11. The Module has no weapon-enchant/on-hit dispatcher. Its aura Proc pass does not represent the source item-trigger path. |

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

Classic dry-run SQL keeps its prior bytes. At the SQL execution boundary, the importer fills
only missing appended fields with their defaults because SpacetimeDB SQL requires all columns.
The test fixtures use the same compatibility rule. Hand-running legacy SQL with the SpacetimeDB
CLI requires those appended fields. Old Package Deltas remain valid without rewriting their
bytes. New Datascript clones carry the extra fields. The Fire Nova fixture changed only by
adding the four defaults to its claims; its source hash and all earlier claims are unchanged.

Physical direct spell damage previously bypassed armor unconditionally. The ordinary path now
applies armor, and IGNORE_ARMOR selects the bypass. Ordinary periodic auras from different casters
can coexist; cross-caster stacking requires its flag. Range checks also reject a different map or
Instance when a spell's range is zero. These are gameplay changes, independent of SQL byte parity.
