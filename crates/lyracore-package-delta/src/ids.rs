//! Reserved identifier bands, and the rule that decides which of them a Package may claim.
//!
//! Every constant here is a policy number, not a derived one. They live together so a maintainer
//! changes a band in exactly one place.
//!
//! # A family's Package identifier band
//!
//! Each Import Family whose Packages may INVENT rows gets one band, named
//! `PACKAGE_<FAMILY>_ID_FLOOR` and `PACKAGE_<FAMILY>_ID_CEIL`. The floor follows one formula, and
//! the spell band below is the worked example of it:
//!
//!  * **Two decimal orders above the highest identifier a real client can hold** for that table.
//!    One order of headroom is what a fixture floor takes; two removes the question for a later
//!    client build.
//!  * **Clear of every reserved band** — [`RESERVED_ID_FLOOR`]..=[`RESERVED_ID_CEIL`], which spans
//!    the project-wide fixture band and every curated import band — so no reserved row can ever
//!    fall inside a Package band.
//!  * **Readable at a glance**, so the leading digits alone say "a Package invented this row".
//!
//! Every band states both ends and both ends are checked by `const _: () = assert!(…)` below: floor
//! at or under ceiling, and the whole band outside the reserved span. A maintainer moving a number
//! gets a compile error rather than a silently unenforceable policy.
//!
//! A table with no real client identifiers takes the same formula with its first clause satisfied
//! vacuously, so its band may sit BELOW the reserved span rather than above it. The script band is
//! the worked example of that case, and it is why the reserved span states a floor as well as a
//! ceiling.

// ===============================================================================================
//  Reserved bands. Shared by every family: a Package band has to clear all of them.
// ===============================================================================================

/// Lowest identifier in the project-wide `509xxxx` fixture-reserved band (`docs/danger-zones.md`,
/// `lyracore_shared::constants::STORAGE_ID_FLOOR`). Nothing outside the fixtures may write into it.
pub const FIXTURE_RESERVED_ID_FLOOR: u32 = 5_090_000;

/// Highest identifier in the project-wide `509xxxx` fixture-reserved band.
pub const FIXTURE_RESERVED_ID_CEIL: u32 = 5_099_999;

/// Highest identifier any CURATED import band owns.
///
/// The importer hands out fixed identifiers to rows it invents rather than reads, so its own reload
/// can delete the whole span. The highest such band is the curated trainer overrides
/// (`CURATED_TRAINER_ID_BASE` 5,200,000 plus a 500-wide span, `importer/src/spell.rs`); the
/// createinfo item band (`CREATEINFO_ITEM_ID_FLOOR` 1,000,000, `importer/src/main.rs`) sits below
/// it. A new curated band above this one moves this number.
pub const CURATED_RESERVED_ID_CEIL: u32 = 5_200_499;

/// Lowest identifier any CURATED import band owns: the createinfo item band
/// (`CREATEINFO_ITEM_ID_FLOOR`, `importer/src/main.rs`), which sits below the curated trainer
/// overrides. A new curated band below this one moves this number.
pub const CURATED_RESERVED_ID_FLOOR: u32 = 1_000_000;

/// Lowest identifier ANY reserved band owns. A Package band that sits below the reserved span must
/// clear it.
///
/// Derived from the bands above rather than restated, so adding a band is one edit.
pub const RESERVED_ID_FLOOR: u32 = lower(FIXTURE_RESERVED_ID_FLOOR, CURATED_RESERVED_ID_FLOOR);

/// Highest identifier ANY reserved band owns. A Package band that sits above the reserved span must
/// clear it.
///
/// Derived from the bands above rather than restated, so adding a band is one edit.
pub const RESERVED_ID_CEIL: u32 = higher(FIXTURE_RESERVED_ID_CEIL, CURATED_RESERVED_ID_CEIL);

const fn higher(a: u32, b: u32) -> u32 {
    if a > b {
        a
    } else {
        b
    }
}

const fn lower(a: u32, b: u32) -> u32 {
    if a < b {
        a
    } else {
        b
    }
}

const _: () = assert!(FIXTURE_RESERVED_ID_FLOOR <= FIXTURE_RESERVED_ID_CEIL);
const _: () = assert!(CURATED_RESERVED_ID_FLOOR <= CURATED_RESERVED_ID_CEIL);
const _: () = assert!(RESERVED_ID_FLOOR <= RESERVED_ID_CEIL);

// ===============================================================================================
//  spell
// ===============================================================================================

/// Packed `game_spell_effect` primary key: `(spell_id << 2) | effect_index`.
///
/// This formula is already canonical and load-bearing in the Module (`module/src/spell/tables.rs`,
/// the `SpellEffect.id` column) and in the importer's spell load. A Package Delta never authors the
/// packed value; it names the spell and the effect index, and this derives the key. Two bits carry
/// the index, so the spell identifier keeps its full `u32` width inside a `u64`.
#[must_use]
pub const fn packed_spell_effect_id(spell_id: u32, effect_index: u8) -> u64 {
    (spell_id as u64) << 2 | effect_index as u64
}

/// Highest `effect_index` a claim may name.
///
/// A vanilla `SpellEntry` carries exactly three effect slots, so the real domain is 0..=2. The
/// packed key reserves two bits and could hold a 3, but no importer or seeder ever writes that row,
/// so a claim on it would target a row that cannot exist.
pub const MAX_SPELL_EFFECT_INDEX: u8 = 2;

/// Lowest spell identifier a Package may INSERT.
///
/// The worked example of the band formula in this module's header. `LyraCore` serves build 5875
/// only, whose highest `Spell.dbc` entry is far below 50,000 — that is why the importer's
/// `SYNTHETIC_SPELL_ID_FLOOR` (`importer/src/spell.rs`) sits there. That floor is a *fixture* floor,
/// not a client-safe one: it leaves less than one decimal order of headroom over real data, and a
/// later client build would run straight through it. Two orders removes the question, and the whole
/// `6xxxxxx` decade is above every reserved band and unclaimed, so a `6` in the millions column
/// means "a Package invented this row".
///
/// A real client renders a spell by looking the identifier up in its OWN `Spell.dbc`, so a Package
/// spell shows no tooltip on an unmodified client. That is expected: the identifier is safe because
/// it can never collide with real or fixture data, not because the client knows it.
pub const PACKAGE_SPELL_ID_FLOOR: u32 = 6_000_000;

/// Highest spell identifier a Package may INSERT. One million identifiers, which is more than the
/// whole vanilla catalogue.
pub const PACKAGE_SPELL_ID_CEIL: u32 = 6_999_999;

/// Lowest spell identifier in the seeded fixture cluster (`module/src/seed.rs`,
/// `module/src/seed/fixtures.rs`). The Module asserts its own fixture spells stay inside
/// `50_000..51_000`; the importer's wholesale clear spares the same rows.
pub const FIXTURE_SPELL_ID_FLOOR: u32 = 50_000;

/// Highest spell identifier in the seeded fixture cluster.
pub const FIXTURE_SPELL_ID_CEIL: u32 = 50_999;

const _: () = assert!(FIXTURE_SPELL_ID_FLOOR <= FIXTURE_SPELL_ID_CEIL);
const _: () = assert!(PACKAGE_SPELL_ID_FLOOR <= PACKAGE_SPELL_ID_CEIL);
const _: () = assert!(FIXTURE_SPELL_ID_CEIL < FIXTURE_RESERVED_ID_FLOOR);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_SPELL_ID_FLOOR);

/// True when a Package may INSERT a spell at this identifier.
#[must_use]
pub const fn is_package_spell_id(spell_id: u32) -> bool {
    spell_id >= PACKAGE_SPELL_ID_FLOOR && spell_id <= PACKAGE_SPELL_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation: a fixture row is what the Module's own tests assert against, and a Package overwriting
/// one turns those tests into a report about the Package.
#[must_use]
pub const fn is_fixture_reserved_spell_id(spell_id: u32) -> bool {
    (spell_id >= FIXTURE_SPELL_ID_FLOOR && spell_id <= FIXTURE_SPELL_ID_CEIL)
        || (spell_id >= FIXTURE_RESERVED_ID_FLOOR && spell_id <= FIXTURE_RESERVED_ID_CEIL)
}

// ===============================================================================================
//  items
// ===============================================================================================

/// Lowest item identifier a Package may INSERT.
///
/// The second application of the band formula in this module's header. A ClassicDB-shaped
/// `item_template` dump for this client build tops out under 24,000 (`module/src/seed.rs`'s own
/// synthetic-fixture comment); two decimal orders above that is still comfortably under
/// [`RESERVED_ID_CEIL`], so — exactly as for [`PACKAGE_SPELL_ID_FLOOR`] — the reserved bands are
/// the binding constraint, not the client headroom. The floor sits one whole decade above the
/// Package spell range so the millions column stays a family-at-a-glance signal across tables, not
/// only within one: `6` means a Package spell, `7` means a Package item.
pub const PACKAGE_ITEM_ID_FLOOR: u32 = 7_000_000;

/// Highest item identifier a Package may INSERT. One million identifiers, the same width as the
/// Package spell range.
pub const PACKAGE_ITEM_ID_CEIL: u32 = 7_999_999;

const _: () = assert!(PACKAGE_ITEM_ID_FLOOR <= PACKAGE_ITEM_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_ITEM_ID_FLOOR);

/// True when a Package may INSERT an item at this identifier.
#[must_use]
pub const fn is_package_item_id(entry: u32) -> bool {
    entry >= PACKAGE_ITEM_ID_FLOOR && entry <= PACKAGE_ITEM_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation.
///
/// Unlike spells, items have no family-specific fixture cluster: the seeded item fixtures either
/// ride real vanilla entries (`module/src/seed.rs`'s starter weapon, Hearthstone, Soul Shard) or
/// sit inside the project-wide `509xxxx` band (`FIXTURE_BLADE`/`FIXTURE_JERKY`/`FIXTURE_REINS`,
/// `module/src/seed/fixtures.rs`) — so the project-wide band is the whole check.
#[must_use]
pub const fn is_fixture_reserved_item_id(entry: u32) -> bool {
    entry >= FIXTURE_RESERVED_ID_FLOOR && entry <= FIXTURE_RESERVED_ID_CEIL
}

// ===============================================================================================
//  script
// ===============================================================================================

/// Lowest `game_script.script_id` a Package may ship.
///
/// The band formula's third application, and the case where its first clause is vacuous. A
/// Runtime Script has no client-side counterpart at all: no DBC holds one, no import reads one, and
/// no client ever names one. "Two decimal orders above the highest identifier a real client can
/// hold" is therefore satisfied by any positive number, and the band is free to sit LOW rather than
/// being pushed above the reserved span the way the spell band was.
///
/// It sits low deliberately. Every `game_script` row is Package-shipped, so the leading digits are
/// not distinguishing a Package row from an imported one — there are no imported ones. What they
/// distinguish is a shipped script from the two values that are not scripts: `0`, which
/// `game_spell_effect.script_id` already uses to mean "pure data, no script", and the
/// `1`..=99,999 span left free below this floor for anything the engine may one day ship itself.
/// Six digits, and the hundred-thousands column says "a Package shipped this script".
pub const PACKAGE_SCRIPT_ID_FLOOR: u32 = 100_000;

/// Highest `game_script.script_id` a Package may ship. Nine hundred thousand identifiers, against a
/// realm that will run tens of scripts.
pub const PACKAGE_SCRIPT_ID_CEIL: u32 = 999_999;

const _: () = assert!(PACKAGE_SCRIPT_ID_FLOOR <= PACKAGE_SCRIPT_ID_CEIL);
const _: () = assert!(PACKAGE_SCRIPT_ID_FLOOR > 0);
/// The whole band sits below every reserved band, which is what makes a fixture-reserved script
/// identifier unspellable rather than merely refused: there is no runtime fixture check for scripts
/// because a value inside a reserved band cannot pass the band check above it. Moving either number
/// into the reserved span fails the build here instead of quietly making that true.
const _: () = assert!(PACKAGE_SCRIPT_ID_CEIL < RESERVED_ID_FLOOR);

/// True when a Package may ship a Runtime Script at this identifier.
#[must_use]
pub const fn is_package_script_id(script_id: u32) -> bool {
    script_id >= PACKAGE_SCRIPT_ID_FLOOR && script_id <= PACKAGE_SCRIPT_ID_CEIL
}

// ===============================================================================================
//  quests
// ===============================================================================================

/// Lowest quest identifier a Package may INSERT.
///
/// The third application of the band formula in this module's header, one decade above the
/// Package item range so the millions column keeps naming a family: `6` spell, `7` item, `8`
/// quest. A cmangos-shaped `quest_template` dump for this client build stays far below this floor,
/// so, as for items, [`RESERVED_ID_CEIL`] is the binding constraint, not the client headroom.
///
/// One band covers the whole quest family, the same way the Package spell range covers both
/// `game_spell` and `game_spell_effect`: every child table (`game_quest_text` and the rest) names
/// its owning quest by `quest_entry`, so a child row is only ever as Package-owned as the quest it
/// belongs to. Nothing below checks a child table's own surrogate `id` against this band.
pub const PACKAGE_QUEST_ID_FLOOR: u32 = 8_000_000;

/// Highest quest identifier a Package may INSERT.
pub const PACKAGE_QUEST_ID_CEIL: u32 = 8_999_999;

const _: () = assert!(PACKAGE_QUEST_ID_FLOOR <= PACKAGE_QUEST_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_QUEST_ID_FLOOR);

/// True when a Package may INSERT a quest at this identifier.
#[must_use]
pub const fn is_package_quest_id(entry: u32) -> bool {
    entry >= PACKAGE_QUEST_ID_FLOOR && entry <= PACKAGE_QUEST_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. Quests have no family-specific fixture cluster of their own, so the project-wide
/// band is the whole check, the same shape as [`is_fixture_reserved_item_id`].
#[must_use]
pub const fn is_fixture_reserved_quest_id(entry: u32) -> bool {
    entry >= FIXTURE_RESERVED_ID_FLOOR && entry <= FIXTURE_RESERVED_ID_CEIL
}

/// Highest `obj_index` a claim may name on `game_quest_objective` or `game_quest_cast_objective`.
///
/// [`crate::quest::MAX_OBJECTIVES`](../../../module/src/quest.rs) caps a quest at 4 objective
/// slots (0-based), the same "real domain is narrower than the packed field width" shape
/// [`MAX_SPELL_EFFECT_INDEX`] documents for spell effects.
pub const MAX_QUEST_OBJECTIVE_INDEX: u8 = 3;

/// Highest `choice_index` a claim may name on `game_quest_reward_choice`.
///
/// `QuestRewardChoice::choice_index` is documented `0..=5` in the Module (cmangos carries six
/// `RewChoiceItemId`/`Count` slots per quest).
pub const MAX_QUEST_REWARD_CHOICE_INDEX: u8 = 5;

/// Packed `game_quest_objective` / `game_quest_cast_objective` primary key:
/// `(quest_entry << 8) | obj_index`.
///
/// Neither table's real `id` column derives from a formula the way `game_spell_effect.id` does.
/// both are plain surrogates the importer assigns in dump order. A Package Delta needs a
/// deterministic key it can compute without a live counter, so it derives one here instead,
/// mirroring [`packed_spell_effect_id`]'s shape: the wide component low, the narrow index high
/// enough to never collide with it. `obj_index` is bounded by [`MAX_QUEST_OBJECTIVE_INDEX`], so 8
/// bits is more room than it ever uses; `quest_entry` keeps its full `u32` width below that.
#[must_use]
pub const fn packed_quest_objective_id(quest_entry: u32, obj_index: u8) -> u64 {
    (quest_entry as u64) << 8 | obj_index as u64
}

/// Packed `game_quest_reward_choice` primary key: `(quest_entry << 8) | choice_index`.
///
/// Same shape and reasoning as [`packed_quest_objective_id`]; `choice_index` is bounded by
/// [`MAX_QUEST_REWARD_CHOICE_INDEX`].
#[must_use]
pub const fn packed_quest_reward_choice_id(quest_entry: u32, choice_index: u8) -> u64 {
    (quest_entry as u64) << 8 | choice_index as u64
}

/// Packed `game_quest_reward_item` primary key: `(quest_entry << 32) | item_entry`.
///
/// A quest does not guarantee-reward the same item through two separate rows, so the pair is
/// already a natural unique key. Packing it needs no bound beyond each half's own `u32` width,
/// the two exactly fill a `u64`.
#[must_use]
pub const fn packed_quest_reward_item_id(quest_entry: u32, item_entry: u32) -> u64 {
    (quest_entry as u64) << 32 | item_entry as u64
}

// ===============================================================================================
//  loot
// ===============================================================================================

/// Lowest loot-row identifier a Package may INSERT.
///
/// The fourth application of the band formula, one decade above the Package quest range: `9`
/// names a Package loot row.
///
/// Unlike the families above, no loot table's owning entity is ever Package-invented. A
/// `game_pickpocket_loot` row's `creature_entry`, a `game_gameobject_loot` row's `loot_id`, a
/// `game_skinning_loot` row's `skin_loot_id`, and a `game_fishing_loot` row's `zone_id` all name
/// real client data (creatures, gameobjects, and zones stay out of this issue's scope). So this
/// band is checked against a loot row's OWN surrogate `id`, the same way [`PACKAGE_ITEM_ID_FLOOR`]
/// is checked against `game_item_template.entry`, not against an owning identifier the way the
/// quest family's child tables are. One band still covers the whole family: the four loot tables
/// are independent `SpacetimeDB` tables with independent primary-key spaces, so sharing one range
/// across them cannot collide.
pub const PACKAGE_LOOT_ID_FLOOR: u64 = 9_000_000;

/// Highest loot-row identifier a Package may INSERT.
pub const PACKAGE_LOOT_ID_CEIL: u64 = 9_999_999;

const _: () = assert!(PACKAGE_LOOT_ID_FLOOR <= PACKAGE_LOOT_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_LOOT_ID_FLOOR);

/// True when a Package may INSERT a loot row at this identifier.
#[must_use]
pub const fn is_package_loot_id(id: u64) -> bool {
    id >= PACKAGE_LOOT_ID_FLOOR && id <= PACKAGE_LOOT_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. Loot rows have no family-specific fixture cluster of their own, so the project-wide
/// band is the whole check.
#[must_use]
pub const fn is_fixture_reserved_loot_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

// ===============================================================================================
//  casts
// ===============================================================================================

/// Lowest `game_creature_spell.id` a Package may INSERT.
///
/// The fifth application of the band formula, one decade above the Package loot range: `10` names
/// a Package cast row. Past 9,999,999 the millions column takes two digits, which is the cost of
/// keeping the family-at-a-glance signal going.
///
/// `game_creature_cast` carries no band of its own: its primary key is `creature_entry`, a creature
/// template entry, which no Package may invent
/// ([`crate::DeltaError::InsertNotSupported`]). Only `game_creature_spell`'s own surrogate `id` is
/// checked here, the loot shape.
pub const PACKAGE_CAST_ID_FLOOR: u64 = 10_000_000;

/// Highest `game_creature_spell.id` a Package may INSERT.
pub const PACKAGE_CAST_ID_CEIL: u64 = 10_999_999;

const _: () = assert!(PACKAGE_CAST_ID_FLOOR <= PACKAGE_CAST_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_CAST_ID_FLOOR);

/// True when a Package may INSERT a `game_creature_spell` row at this identifier.
#[must_use]
pub const fn is_package_cast_id(id: u64) -> bool {
    id >= PACKAGE_CAST_ID_FLOOR && id <= PACKAGE_CAST_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. The cast family has no fixture cluster of its own, so the project-wide band is the
/// whole check, the same shape as [`is_fixture_reserved_loot_id`].
#[must_use]
pub const fn is_fixture_reserved_cast_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

// ===============================================================================================
//  trainers
// ===============================================================================================

/// Lowest `game_trainer_spell.id` a Package may INSERT.
///
/// The sixth application of the band formula, one decade above the Package cast range: `11` names
/// a Package trainer row.
///
/// Distinct from [`CURATED_RESERVED_ID_CEIL`]'s `CURATED_TRAINER_ID_BASE` span
/// (`importer/src/spell.rs`, 5,200,000 plus a 500-wide span): that band is where the IMPORTER hands
/// out fixed identifiers to curated overrides it invents rather than reads, already covered by
/// [`RESERVED_ID_CEIL`]. This band is where a PACKAGE may insert one. The two are never merged.
pub const PACKAGE_TRAINER_ID_FLOOR: u64 = 11_000_000;

/// Highest `game_trainer_spell.id` a Package may INSERT.
pub const PACKAGE_TRAINER_ID_CEIL: u64 = 11_999_999;

const _: () = assert!(PACKAGE_TRAINER_ID_FLOOR <= PACKAGE_TRAINER_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_TRAINER_ID_FLOOR);

/// True when a Package may INSERT a `game_trainer_spell` row at this identifier.
#[must_use]
pub const fn is_package_trainer_id(id: u64) -> bool {
    id >= PACKAGE_TRAINER_ID_FLOOR && id <= PACKAGE_TRAINER_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. The trainer family has no fixture cluster of its own, so the project-wide band is
/// the whole check, the same shape as [`is_fixture_reserved_loot_id`].
#[must_use]
pub const fn is_fixture_reserved_trainer_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

// ===============================================================================================
//  gossip
// ===============================================================================================

/// Lowest identifier a Package may INSERT in the gossip family.
///
/// The seventh application of the band formula, one decade above the Package trainer range: `12`
/// names a Package gossip row. One band covers the whole family, the loot shape: five independent
/// `SpacetimeDB` tables with independent primary-key spaces cannot collide by sharing a range. It
/// is checked against `game_gossip_menu_profile.menu_id`, `game_gossip_menu_profile_option.row_id`,
/// `game_gossip_option.row_id`, `game_npc_text.text_id` and `game_npc_text_slot.id`.
///
/// `game_gossip_menu` carries no band: its primary key is a creature template entry, which no
/// Package may invent ([`crate::DeltaError::InsertNotSupported`]).
///
/// The band sits above every identifier the gossip base import spares by range: that import deletes
/// `text_id` / `row_id` / `id` below 50,000 and `game_gossip_menu.entry` below 1,000,000
/// (`importer/src/main.rs`, `docs/danger-zones.md`), so a Package row in those tables survives a
/// base reload. The two menu-profile tables it clears whole; a Package row there is restored by the
/// Package Delta stage the reload runs next, the way every family's reload restores its claims.
pub const PACKAGE_GOSSIP_ID_FLOOR: u64 = 12_000_000;

/// Highest identifier a Package may INSERT in the gossip family.
pub const PACKAGE_GOSSIP_ID_CEIL: u64 = 12_999_999;

const _: () = assert!(PACKAGE_GOSSIP_ID_FLOOR <= PACKAGE_GOSSIP_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_GOSSIP_ID_FLOOR);
const _: () = assert!(PACKAGE_TRAINER_ID_CEIL < PACKAGE_GOSSIP_ID_FLOOR);

/// True when a Package may INSERT a gossip row at this identifier.
#[must_use]
pub const fn is_package_gossip_id(id: u64) -> bool {
    id >= PACKAGE_GOSSIP_ID_FLOOR && id <= PACKAGE_GOSSIP_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. The gossip family has no fixture cluster of its own, so the project-wide band is the
/// whole check, the same shape as [`is_fixture_reserved_loot_id`].
#[must_use]
pub const fn is_fixture_reserved_gossip_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

// ===============================================================================================
//  globals
// ===============================================================================================

/// Lowest identifier a Package may INSERT in the globals family.
///
/// The eighth application of the band formula, one decade above the Package gossip range: `13`
/// names a Package globals row. Checked against `game_graveyard_zone.row_id`,
/// `game_createinfo_spell.id` and `game_createinfo_action.row_id` — the three tables of the family
/// whose key is a free surrogate.
///
/// The family's other four tables carry no band at all, because none of their keys is free to
/// invent: `game_class_level_stats`, `game_level_stats` and `game_start_position` key on a race,
/// class and level the client fixes, and `game_areatrigger_teleport` keys on an `AreaTrigger.dbc`
/// trigger id. All four are update-only ([`crate::DeltaError::InsertNotSupported`]).
pub const PACKAGE_GLOBALS_ID_FLOOR: u64 = 13_000_000;

/// Highest identifier a Package may INSERT in the globals family.
pub const PACKAGE_GLOBALS_ID_CEIL: u64 = 13_999_999;

const _: () = assert!(PACKAGE_GLOBALS_ID_FLOOR <= PACKAGE_GLOBALS_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_GLOBALS_ID_FLOOR);
const _: () = assert!(PACKAGE_GOSSIP_ID_CEIL < PACKAGE_GLOBALS_ID_FLOOR);

/// True when a Package may INSERT a globals row at this identifier.
#[must_use]
pub const fn is_package_globals_id(id: u64) -> bool {
    id >= PACKAGE_GLOBALS_ID_FLOOR && id <= PACKAGE_GLOBALS_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. The globals family has no fixture cluster of its own, so the project-wide band is the
/// whole check.
#[must_use]
pub const fn is_fixture_reserved_globals_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

/// Highest `level` a claim may name on `game_class_level_stats` or `game_level_stats`.
///
/// Vanilla stops at 60 and the cmangos stat curves carry no row above it, so a claim past this
/// would name a row that cannot exist. The same "real domain is narrower than the packed field
/// width" shape [`MAX_SPELL_EFFECT_INDEX`] documents: the packed key reserves a whole byte.
pub const MAX_STATS_LEVEL: u8 = 60;

/// Packed `game_class_level_stats` primary key: `(class << 8) | level`.
///
/// Already canonical and load-bearing in the Module (`module/src/stats.rs`'s `class_level_key`),
/// which is why this restates the formula rather than inventing one the way
/// [`packed_quest_objective_id`] had to. A Package Delta never authors the packed value; it names
/// the class and the level, and this derives the key.
#[must_use]
pub const fn packed_class_level_id(class: u8, level: u8) -> u64 {
    ((class as u64) << 8) | level as u64
}

/// Packed `game_level_stats` primary key: `(race << 16) | (class << 8) | level`.
///
/// The Module's `race_class_level_key` (`module/src/stats.rs`), restated for the same reason as
/// [`packed_class_level_id`].
#[must_use]
pub const fn packed_race_class_level_id(race: u8, class: u8, level: u8) -> u64 {
    ((race as u64) << 16) | ((class as u64) << 8) | level as u64
}

/// Packed `game_start_position` primary key: `(race << 8) | class`.
///
/// The Module packs the same pair into a `u16` (`module/src/config.rs`'s `StartPosition`), and
/// `module/src/config.rs`'s `game_start_item` uses the identical packing for its own `race_class`.
#[must_use]
pub const fn packed_race_class_id(race: u8, class: u8) -> u64 {
    ((race as u64) << 8) | class as u64
}

// ===============================================================================================
//  spellmeta
// ===============================================================================================

/// Lowest `game_spell_learn.id` a Package may INSERT.
///
/// The ninth application of the band formula, one decade above the Package globals range: `14`
/// names a Package spell-metadata row.
///
/// It covers ONE table. `game_spell_chain` and `game_spell_proc_event` both key on a spell
/// identifier, not a surrogate, so what a Package may invent there is exactly what it may invent in
/// `game_spell` — [`PACKAGE_SPELL_ID_FLOOR`]..=[`PACKAGE_SPELL_ID_CEIL`], and
/// [`is_fixture_reserved_spell_id`] for the fixtures. Giving those two tables a band of their own
/// would let a Package write metadata for a spell that cannot exist.
pub const PACKAGE_SPELLMETA_ID_FLOOR: u64 = 14_000_000;

/// Highest `game_spell_learn.id` a Package may INSERT.
pub const PACKAGE_SPELLMETA_ID_CEIL: u64 = 14_999_999;

const _: () = assert!(PACKAGE_SPELLMETA_ID_FLOOR <= PACKAGE_SPELLMETA_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_SPELLMETA_ID_FLOOR);
const _: () = assert!(PACKAGE_GLOBALS_ID_CEIL < PACKAGE_SPELLMETA_ID_FLOOR);

/// True when a Package may INSERT a `game_spell_learn` row at this identifier.
#[must_use]
pub const fn is_package_spellmeta_id(id: u64) -> bool {
    id >= PACKAGE_SPELLMETA_ID_FLOOR && id <= PACKAGE_SPELLMETA_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. `game_spell_learn` has no fixture cluster of its own, so the project-wide band is the
/// whole check. Its two spell-keyed siblings use [`is_fixture_reserved_spell_id`] instead, which
/// also covers the seeded fixture spell cluster.
#[must_use]
pub const fn is_fixture_reserved_spellmeta_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}

// ===============================================================================================
//  creatures
// ===============================================================================================

/// Highest value the creature guid encoding leaves for a creature template entry and for a spawn
/// identifier.
///
/// `game_creature_spawn.guid` packs both into 24-bit fields (`importer/src/main.rs`'s `world_guid`,
/// restated by [`packed_creature_spawn_guid`]), so an entry or a spawn identifier above this would
/// wrap and address another creature's row. Every creature band below is asserted against it.
pub const MAX_CREATURE_GUID_COMPONENT: u32 = 0x00FF_FFFF;

/// Lowest identifier a Package may INSERT in the creatures family.
///
/// The tenth application of the band formula in this module's header, one decade above the Package
/// spell metadata range: `15` names a Package creature row. One band covers both of the family's
/// insertable tables, the loot shape — `game_creature_template.entry` and a `game_creature_spawn`
/// claim's `spawn_id`, which are independent identifier spaces.
///
/// The ceiling has a second constraint the earlier families did not: both components sit in 24-bit
/// fields of the spawn guid, so the whole band must stay under
/// [`MAX_CREATURE_GUID_COMPONENT`]. It does, with room to spare, which is also why the gameobject
/// band above it is the last one this encoding can hold.
pub const PACKAGE_CREATURE_ID_FLOOR: u32 = 15_000_000;

/// Highest identifier a Package may INSERT in the creatures family.
pub const PACKAGE_CREATURE_ID_CEIL: u32 = 15_999_999;

/// Lowest creature template entry in the seeded fixture cluster (`module/src/seed/fixtures.rs`'s
/// Test Wolf and its siblings). The Module asserts its own fixture creatures stay inside
/// `51_000..52_000`, one decade above the fixture spell cluster.
pub const FIXTURE_CREATURE_ID_FLOOR: u32 = 51_000;

/// Highest creature template entry in the seeded fixture cluster.
pub const FIXTURE_CREATURE_ID_CEIL: u32 = 51_999;

const _: () = assert!(FIXTURE_CREATURE_ID_FLOOR <= FIXTURE_CREATURE_ID_CEIL);
const _: () = assert!(FIXTURE_CREATURE_ID_CEIL < FIXTURE_RESERVED_ID_FLOOR);
const _: () = assert!(PACKAGE_CREATURE_ID_FLOOR <= PACKAGE_CREATURE_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_CREATURE_ID_FLOOR);
const _: () = assert!(PACKAGE_SPELLMETA_ID_CEIL < PACKAGE_CREATURE_ID_FLOOR as u64);
/// Both halves of a creature spawn guid are 24 bits wide. A band that ran past them would let two
/// Package spawns share one durable guid.
const _: () = assert!(PACKAGE_CREATURE_ID_CEIL <= MAX_CREATURE_GUID_COMPONENT);

/// True when a Package may INSERT a creature template or a creature spawn at this identifier.
#[must_use]
pub const fn is_package_creature_id(id: u32) -> bool {
    id >= PACKAGE_CREATURE_ID_FLOOR && id <= PACKAGE_CREATURE_ID_CEIL
}

/// True when the creature TEMPLATE entry belongs to a seeded fixture. No Package may claim one,
/// under any operation, for the reason [`is_fixture_reserved_spell_id`] gives: a fixture row is
/// what the Module's own tests assert against.
///
/// Both bands, the spell shape. A spawn identifier takes
/// [`is_fixture_reserved_creature_spawn_id`] instead — the `51xxx` cluster is a template-entry
/// space, and real imported spawn identifiers run straight through it.
#[must_use]
pub const fn is_fixture_reserved_creature_id(id: u32) -> bool {
    (id >= FIXTURE_CREATURE_ID_FLOOR && id <= FIXTURE_CREATURE_ID_CEIL)
        || (id >= FIXTURE_RESERVED_ID_FLOOR && id <= FIXTURE_RESERVED_ID_CEIL)
}

/// True when the creature SPAWN identifier belongs to a seeded fixture: the project-wide band
/// alone, because the fixture cluster above names template entries rather than spawns.
#[must_use]
pub const fn is_fixture_reserved_creature_spawn_id(id: u32) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR && id <= FIXTURE_RESERVED_ID_CEIL
}

/// Packed `game_creature_spawn` primary key: `(HIGHGUID_UNIT << 48) | (entry << 24) | spawn_id`.
///
/// Already canonical and load-bearing in the importer (`importer/src/main.rs`'s `world_guid`),
/// which is why this restates the formula rather than inventing one, the way
/// [`packed_class_level_id`] restates the Module's. A Package Delta never authors the guid; it
/// names the creature template and the spawn identifier, and this derives the key. Both components
/// are masked to their 24-bit fields, which is what [`MAX_CREATURE_GUID_COMPONENT`] guards the
/// band against.
#[must_use]
pub const fn packed_creature_spawn_guid(entry: u32, spawn_id: u32) -> u64 {
    const HIGHGUID_UNIT: u64 = 0xF130;
    (HIGHGUID_UNIT << 48)
        | (((entry & MAX_CREATURE_GUID_COMPONENT) as u64) << 24)
        | (spawn_id & MAX_CREATURE_GUID_COMPONENT) as u64
}

// ===============================================================================================
//  gameobjects
// ===============================================================================================

/// Lowest identifier a Package may INSERT in the gameobjects family.
///
/// The eleventh application of the band formula, one decade above the Package creature range: `16`
/// names a Package gameobject row. One band covers the family's three insertable tables —
/// `game_gameobject_template.entry`, `game_gameobject_trap.entry` and a `game_gameobject` claim's
/// `spawn_id`. The first two share ONE identifier space on purpose: a trap row describes the
/// template of the same entry, so a Package trap is exactly as Package-owned as its template.
pub const PACKAGE_GAMEOBJECT_ID_FLOOR: u32 = 16_000_000;

/// Highest identifier a Package may INSERT in the gameobjects family.
pub const PACKAGE_GAMEOBJECT_ID_CEIL: u32 = 16_999_999;

const _: () = assert!(PACKAGE_GAMEOBJECT_ID_FLOOR <= PACKAGE_GAMEOBJECT_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_GAMEOBJECT_ID_FLOOR);
const _: () = assert!(PACKAGE_CREATURE_ID_CEIL < PACKAGE_GAMEOBJECT_ID_FLOOR);

/// True when a Package may INSERT a gameobject template, trap or spawn at this identifier.
#[must_use]
pub const fn is_package_gameobject_id(id: u32) -> bool {
    id >= PACKAGE_GAMEOBJECT_ID_FLOOR && id <= PACKAGE_GAMEOBJECT_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. The gameobjects family has no fixture cluster of its own, so the project-wide band
/// is the whole check.
#[must_use]
pub const fn is_fixture_reserved_gameobject_id(id: u32) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR && id <= FIXTURE_RESERVED_ID_CEIL
}

/// Packed `game_gameobject` primary key: `(HIGHGUID_GAMEOBJECT << 48) | spawn_id`.
///
/// The importer's `go_guid` (`importer/src/main.rs`), restated for the same reason as
/// [`packed_creature_spawn_guid`]. A gameobject guid carries no template entry, so the spawn
/// identifier alone fills the low 48 bits — the whole Package gameobject band fits with four
/// decimal orders to spare, and `module/src/gameobject.rs`'s pool tag at bit 47 stays clear of it.
#[must_use]
pub const fn packed_gameobject_spawn_guid(spawn_id: u32) -> u64 {
    const HIGHGUID_GAMEOBJECT: u64 = 0xF110;
    (HIGHGUID_GAMEOBJECT << 48) | spawn_id as u64
}

// ===============================================================================================
//  creature-ai
// ===============================================================================================

/// Lowest identifier a Package may INSERT in the creature-ai family.
///
/// The twelfth application of the band formula in this module's header, one decade above the
/// Package gameobject range: `17` names a Package `EventAI` row. One band covers the family's three
/// insertable tables — `game_creature_ai_broadcast_text.id`, `game_creature_ai_summon.id` and
/// `game_quest_event_requirement.id`, three independent identifier spaces, the loot shape.
///
/// Stated at `u64` width because the third of those columns is a `u64` and the first two are
/// `u32`s that widen into it. The ceiling still fits a `u32` with three decimal orders to spare,
/// which the assert below holds.
pub const PACKAGE_CREATURE_AI_ID_FLOOR: u64 = 17_000_000;

/// Highest identifier a Package may INSERT in the creature-ai family.
pub const PACKAGE_CREATURE_AI_ID_CEIL: u64 = 17_999_999;

const _: () = assert!(PACKAGE_CREATURE_AI_ID_FLOOR <= PACKAGE_CREATURE_AI_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_CREATURE_AI_ID_FLOOR);
const _: () = assert!((PACKAGE_GAMEOBJECT_ID_CEIL as u64) < PACKAGE_CREATURE_AI_ID_FLOOR);
/// Two of the three keys are `u32` columns, so a band that ran past `u32::MAX` could not be
/// claimed in them at all.
const _: () = assert!(PACKAGE_CREATURE_AI_ID_CEIL <= u32::MAX as u64);

/// True when a Package may INSERT a broadcast text, a summon placement or a quest event
/// requirement at this identifier.
#[must_use]
pub const fn is_package_creature_ai_id(id: u64) -> bool {
    id >= PACKAGE_CREATURE_AI_ID_FLOOR && id <= PACKAGE_CREATURE_AI_ID_CEIL
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation. The creature-ai family has no fixture cluster of its own, so the project-wide band
/// is the whole check.
#[must_use]
pub const fn is_fixture_reserved_creature_ai_id(id: u64) -> bool {
    id >= FIXTURE_RESERVED_ID_FLOOR as u64 && id <= FIXTURE_RESERVED_ID_CEIL as u64
}
