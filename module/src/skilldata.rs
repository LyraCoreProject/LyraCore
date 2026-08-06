//! Skill-fabric static data: `SkillLine.dbc` (the skill lines themselves — weapon/defense lines +
//! the 12 professions + a long tail of NPC-only/deprecated lines), `SkillLineAbility.dbc` (which
//! spell each line teaches/grants, at which skill threshold, and the green/gray skill-up difficulty
//! band for recipes), and `SkillRaceClassInfo.dbc` (which lines a given race/class combo can actually
//! train, and at what character level). Loaded by the importer's `--dbc` mode (see
//! `importer/src/dbc.rs`); no Timestamp columns, so every table here is plain SQL-loadable
//! (`spacetime sql` clear+reload, no reducer needed — same convention as `game_faction_template`/
//! `game_race_info` in `faction.rs`/`config.rs`).
//!
//! SCOPE: this is the DATA half of work-item 208 — the import + tables + the ONE wired consumer
//! (profession trainer tier caps, resolved in `importer/src/main.rs`'s `PROFESSION_LEARN` +
//! `assert_profession_tier_parity`, sourced from `SkillRaceClassInfo.dbc`'s `skill_tier` foreign key
//! into `SkillTiers.dbc` — NOT from `SkillLine.dbc` itself, correcting the original item spec). The
//! remaining consumers each ride their own follow-up item: recipe skill-up ranges from
//! `game_skill_ability.gray`/`.green` (replacing the sentinel-band shortcut `skill::skillup_chance_bp`'s
//! callers currently use), weapon-master availability from `game_skill_availability` (work-item 202's
//! data half), and auto-learned spells at thresholds from `game_skill_ability`'s AUTOLEARN rows
//! (`acquire_method`). `skill.rs`'s hand-authored `skill_line` consts + `class_weapon_skill_lines`
//! now carry a deprecation pointer to these tables (see there) — the consts stay because the wire
//! protocol is still keyed on the same verbatim vanilla ids; they're just no longer the
//! AUTHORITATIVE source for "what skill lines exist".

use spacetimedb::table;

/// One `SkillLine.dbc` row: a skill line's category (the client skill-pane grouping —
/// `SkillLineCategory.dbc`'s raw id, kept as the plain foreign key, not separately imported — same
/// "store the raw key" convention as `game_faction`/`game_race_info`) and display name. ~135 rows in
/// vanilla (weapon/defense lines + the 12 professions + NPC-only/deprecated lines the client never
/// shows a player). [static]
#[table(accessor = game_skill_line, public)]
pub struct SkillLine {
    #[primary_key]
    pub skill_line: u32,
    pub category: u32,
    pub name: String,
}

/// One `SkillLineAbility.dbc` row: a `(spell, skill_line)` pairing plus its race/class gate,
/// acquisition rule, and recipe skill-up band. ~9-10k rows in vanilla (every spell/recipe/
/// gather-yield tied to a skill line, times every race/class variant that can use it).
///
/// `min_skill` and `green` both read the SAME underlying DBC field
/// (`SkillLineAbilityRow::trivial_skill_line_rank_low`) — vanilla reuses that one field as the skill
/// threshold at which an `acquire_method`-automatic spell auto-arrives AND as the green
/// (still-likely-to-skill-up) floor for a player-learned recipe. Stored under both names so each
/// follow-up consumer (the AUTOLEARN-at-threshold item vs. the recipe-skill-up-range item) reads its
/// own vocabulary without cross-referencing another column — see `importer/src/dbc.rs::skill_ability_sql`
/// for the exact mapping. `gray` (`trivial_skill_line_rank_high`) is the ceiling above which a recipe
/// never skills up again. `acquire_method` is the DBC's raw enum (no strings in the DBC; see
/// `importer/src/dbc.rs::AUTOLEARN_ACQUIRE_METHOD` for the value this item treats as "automatic"). [static]
#[table(accessor = game_skill_ability, public, index(accessor = by_skill_line, btree(columns = [skill_line])))]
pub struct SkillAbility {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub spell_id: u32,
    pub skill_line: u32,
    pub race_mask: u32,
    pub class_mask: u32,
    pub min_skill: i32,
    pub acquire_method: i32,
    pub gray: i32,
    pub green: i32,
}

/// One `SkillRaceClassInfo.dbc` row: which (race, class) combos a skill line is available to, its
/// availability flags (e.g. a profession-only or class-only gate), and the minimum character level to
/// pick it up. ~700-800 rows in vanilla. This is also where each profession's `skill_tier` foreign key
/// lives — `importer/src/dbc.rs::profession_tier_values` resolves it into `SkillTiers.dbc`'s
/// Apprentice→Artisan cap array (SkillTiers itself has no `game_*` table — it's a resolve-time-only
/// join, never stored directly). [static]
#[table(accessor = game_skill_availability, public, index(accessor = by_skill_line, btree(columns = [skill_line])))]
pub struct SkillAvailability {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub skill_line: u32,
    pub race_mask: u32,
    pub class_mask: u32,
    pub flags: i32,
    pub min_level: i32,
}
