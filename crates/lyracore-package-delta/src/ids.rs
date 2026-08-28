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
/// so — as for items — [`RESERVED_ID_CEIL`] is the binding constraint, not the client headroom.
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
/// Neither table's real `id` column derives from a formula the way `game_spell_effect.id` does —
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
/// already a natural unique key; packing it needs no bound beyond each half's own `u32` width —
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
/// Unlike the families above, no loot table's owning entity is ever Package-invented — a
/// `game_pickpocket_loot` row's `creature_entry`, a `game_gameobject_loot` row's `loot_id`, a
/// `game_skinning_loot` row's `skin_loot_id`, and a `game_fishing_loot` row's `zone_id` all name
/// real client data (creatures, gameobjects, and zones stay out of this issue's scope). So this
/// band is checked against a loot row's OWN surrogate `id`, the same way [`PACKAGE_ITEM_ID_FLOOR`]
/// is checked against `game_item_template.entry` — not against an owning identifier the way the
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
