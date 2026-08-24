//! Reserved spell identifier bands, and the rule that decides which of them a Package may claim.
//!
//! Every constant here is a policy number, not a derived one. They live together so a maintainer
//! changes a band in exactly one place.

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
/// Chosen to be client-safe, which means three things at once:
///
///  * **Above every real client identifier.** `LyraCore` serves build 5875 only, whose highest
///    `Spell.dbc` entry is far below 50,000 — that is why the importer's `SYNTHETIC_SPELL_ID_FLOOR`
///    (`importer/src/spell.rs`) sits there. That floor is a *fixture* floor, not a client-safe one:
///    it leaves less than one decimal order of headroom over real data, and a later client build
///    would run straight through it. Two orders of magnitude of headroom removes the question.
///  * **Above every reserved band the project already owns.** The project-wide fixture band ends
///    at 5,099,999 and the importer's curated trainer band ends at 5,200,499 (both named in
///    `docs/danger-zones.md`), so the whole `6xxxxxx` decade is unclaimed.
///  * **Readable at a glance.** A `6` in the millions column means "a Package invented this row",
///    distinct from the `509xxxx` fixture band and the `52xxxxx` trainer band.
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

/// Lowest identifier in the project-wide `509xxxx` fixture-reserved band (`docs/danger-zones.md`,
/// `lyracore-shared`'s `STORAGE_ID_FLOOR`). Nothing outside the fixtures may write into it.
pub const FIXTURE_RESERVED_ID_FLOOR: u32 = 5_090_000;

/// Highest identifier in the project-wide `509xxxx` fixture-reserved band.
pub const FIXTURE_RESERVED_ID_CEIL: u32 = 5_099_999;

// The bands must not overlap. A maintainer moving one number gets a compile error rather than a
// silently unenforceable policy.
const _: () = assert!(FIXTURE_SPELL_ID_FLOOR <= FIXTURE_SPELL_ID_CEIL);
const _: () = assert!(FIXTURE_RESERVED_ID_FLOOR <= FIXTURE_RESERVED_ID_CEIL);
const _: () = assert!(PACKAGE_SPELL_ID_FLOOR <= PACKAGE_SPELL_ID_CEIL);
const _: () = assert!(FIXTURE_SPELL_ID_CEIL < FIXTURE_RESERVED_ID_FLOOR);
const _: () = assert!(FIXTURE_RESERVED_ID_CEIL < PACKAGE_SPELL_ID_FLOOR);

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
