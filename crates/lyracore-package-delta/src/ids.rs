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
//!  * **Above [`RESERVED_ID_CEIL`]**, which covers the project-wide fixture band and every curated
//!    import band, so no reserved row can ever fall inside a Package band.
//!  * **Readable at a glance**, so the millions column alone says "a Package invented this row".
//!
//! Every band states both ends and both ends are checked by `const _: () = assert!(…)` below: floor
//! at or under ceiling, and floor above [`RESERVED_ID_CEIL`]. A maintainer moving a number gets a
//! compile error rather than a silently unenforceable policy.

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

/// Highest identifier ANY reserved band owns. Every family's Package band floor must clear it.
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

const _: () = assert!(FIXTURE_RESERVED_ID_FLOOR <= FIXTURE_RESERVED_ID_CEIL);

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
