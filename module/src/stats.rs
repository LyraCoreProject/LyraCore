//! Player base stats from the real vanilla curve. Two cmangos-loaded tables —
//! `game_class_level_stats` (base HP/mana per class+level) and `game_level_stats` (the five base
//! attributes per race+class+level) — plus the vanilla derived-stat formulas, replace the flat
//! placeholder `xp::max_health_for_level` and the `max_power = 1000`-for-everyone hardcode. Loaded by
//! the importer's `--dump` mode (plain `spacetime sql` — no Timestamp columns).
//!
//! Every lookup **falls back to the placeholder curve / power-type constant when a row is missing**,
//! so login never depends on the data being present (a fresh `-c` reprovision before an import still
//! works, and an L1 character is 60 HP either way — zero behavior change until the curve is loaded).
//! The base HP/mana is BEFORE the attribute contribution (Warrior L1 `base_health` is 20, not 60);
//! the real pool adds `hp_from_stamina` / `mana_from_intellect`. [static]

use lyracore_shared::packing::power_type;
use spacetimedb::{table, ReducerContext};

use crate::{game_character, game_world_entity, WorldEntity};

// ===========================================================================================
//  Stat-curve tables [static] — loaded from cmangos by the importer (no Timestamp → SQL-loadable)
// ===========================================================================================

/// Base HP/mana per (class, level) — cmangos `player_classlevelstats`. PK packs `(class<<8)|level`
/// (level ≤ 60 fits a byte). The base is pre-attribute; the live pool adds the stamina/intellect bonus.
#[table(accessor = game_class_level_stats, public)]
pub struct ClassLevelStats {
    #[primary_key]
    pub class_level: u32, // (class << 8) | level
    pub class: u8,
    pub level: u32,
    pub base_health: u32,
    pub base_mana: u32,
}

/// The five base attributes per (race, class, level) — cmangos `player_levelstats`. PK packs
/// `(race<<16)|(class<<8)|level`.
#[table(accessor = game_level_stats, public)]
pub struct LevelStats {
    #[primary_key]
    pub race_class_level: u32, // (race<<16) | (class<<8) | level
    pub race: u8,
    pub class: u8,
    pub level: u32,
    pub strength: u32,
    pub agility: u32,
    pub stamina: u32,
    pub intellect: u32,
    pub spirit: u32,
}

/// Lookup key for `game_class_level_stats`.
pub fn class_level_key(class: u8, level: u32) -> u32 {
    ((class as u32) << 8) | (level & 0xFF)
}
/// Lookup key for `game_level_stats`.
pub fn race_class_level_key(race: u8, class: u8, level: u32) -> u32 {
    ((race as u32) << 16) | ((class as u32) << 8) | (level & 0xFF)
}

// ===========================================================================================
//  Vanilla derived-stat formulas (pure — unit-tested)
// ===========================================================================================

/// Vanilla health from stamina: the first 20 points give 1 HP each, every point beyond gives 10.
pub fn hp_from_stamina(stamina: u32) -> u32 {
    if stamina <= 20 {
        stamina
    } else {
        20 + (stamina - 20) * 10
    }
}

/// Vanilla mana from intellect: the first 20 points give 1 mana each, every point beyond gives 15.
pub fn mana_from_intellect(intellect: u32) -> u32 {
    if intellect <= 20 {
        intellect
    } else {
        20 + (intellect - 20) * 15
    }
}

// ===========================================================================================
//  Lookups (DB + formula, with fallback) — the single source of truth for vitals
// ===========================================================================================

/// Max health for (race, class, level): class/level base HP + the stamina contribution. Falls back
/// to the flat placeholder curve when the curve data isn't loaded, so login never breaks on a miss.
pub fn max_health_for(ctx: &ReducerContext, race: u8, class: u8, level: u32) -> u32 {
    let base = ctx
        .db
        .game_class_level_stats()
        .class_level()
        .find(class_level_key(class, level));
    let attrs = ctx
        .db
        .game_level_stats()
        .race_class_level()
        .find(race_class_level_key(race, class, level));
    match (base, attrs) {
        (Some(b), Some(a)) => b.base_health + hp_from_stamina(a.stamina),
        _ => crate::xp::max_health_for_level(level),
    }
}

/// Max power for (race, class, level), power-type-aware: mana classes get class/level base mana + the
/// intellect contribution; **rage = 1000** (a 100-point bar stored ×10 — what the live slice uses);
/// **energy = 100**. Falls back to the power-type constant (or 0 mana) when the curve isn't loaded.
pub fn max_power_for(ctx: &ReducerContext, race: u8, class: u8, level: u32) -> u32 {
    match power_type::for_class(class) {
        power_type::MANA => {
            let base = ctx
                .db
                .game_class_level_stats()
                .class_level()
                .find(class_level_key(class, level));
            let attrs = ctx
                .db
                .game_level_stats()
                .race_class_level()
                .find(race_class_level_key(race, class, level));
            match (base, attrs) {
                (Some(b), Some(a)) => b.base_mana + mana_from_intellect(a.intellect),
                _ => 0,
            }
        }
        power_type::ENERGY => 100,
        _ => 1000, // RAGE (and any default): a 100-point rage bar stored ×10
    }
}

/// Starting `power` for a freshly materialized PLAYER: mana classes spawn with a FULL mana pool,
/// energy classes (Rogue) also spawn full (energy is non-persisted in vanilla — always 100/100 at
/// login), and rage starts at 0. Pure so it's unit-testable without a live DB. Lives next to
/// `max_power_for`, which owns power-type semantics.
pub fn starting_power(pt: u8, max_power: u32) -> u32 {
    if pt == power_type::MANA || pt == power_type::ENERGY {
        max_power
    } else {
        0
    }
}

/// The five base attributes for (race, class, level) — `(strength, agility, stamina, intellect,
/// spirit)` — from the cmangos curve, for the player's character sheet (`UNIT_FIELD_STAT0..4`).
/// Falls back to all-zero when the row isn't loaded (the sheet then reads 0s until the importer
/// runs — login is unaffected, and these are display-only state). Reads the SAME `game_level_stats`
/// row `max_health_for`/`max_power_for` use, so the stamina/intellect stay consistent with HP/mana.
pub fn base_attributes_for(
    ctx: &ReducerContext,
    race: u8,
    class: u8,
    level: u32,
) -> (u32, u32, u32, u32, u32) {
    match ctx
        .db
        .game_level_stats()
        .race_class_level()
        .find(race_class_level_key(race, class, level))
    {
        Some(a) => (a.strength, a.agility, a.stamina, a.intellect, a.spirit),
        None => (0, 0, 0, 0, 0),
    }
}

/// Old (pre-recompute) stat-curve values `apply_level_stats` returns alongside its write, so a caller
/// that needs a per-stat popup delta (the ding loop) can diff against the freshly-written `new`
/// side without a second curve lookup. Callers that only want the write (login, a bare level-set)
/// ignore the return value.
pub(crate) struct LevelStatsDelta {
    pub old_max_health: u32,
    pub old_max_power: u32,
    pub old_strength: u32,
    pub old_agility: u32,
    pub old_stamina: u32,
    pub old_intellect: u32,
    pub old_spirit: u32,
}

/// Write the level-derived stat block onto `e` for `(race, class, level)`: the five base attributes,
/// armor (`agility * 2`, classic base armor), and max health/power — from the SAME
/// `base_attributes_for`/`max_health_for`/`max_power_for` curve lookups every call site used to inline
/// separately. Does **not** touch the CURRENT `health`/`power` pool (a ding always fully heals; a
/// level-set refills fully; login instead resumes from the persisted value) or anything outside the
/// stat block (display, faction, position, …) — those stay each caller's own concern.
///
/// This was three hand-mirrored copies (`build_player_entity` at login, `grant_xp`'s ding loop, and
/// `set_character_level`) held in sync only by a comment ("mirrors build_player_entity"). The third
/// one drifted and dropped the armor line, so a level-SET character kept stale armor until its next
/// relog. One function now, called from all three sites — the drift class is gone structurally,
/// not just patched at the one site that happened to be caught.
pub(crate) fn apply_level_stats(
    ctx: &ReducerContext,
    e: &mut WorldEntity,
    race: u8,
    class: u8,
    level: u32,
) -> LevelStatsDelta {
    let old = LevelStatsDelta {
        old_max_health: e.max_health,
        old_max_power: e.max_power,
        old_strength: e.strength,
        old_agility: e.agility,
        old_stamina: e.stamina,
        old_intellect: e.intellect,
        old_spirit: e.spirit,
    };
    let (strength, agility, stamina, intellect, spirit) =
        base_attributes_for(ctx, race, class, level);
    e.strength = strength;
    e.agility = agility;
    e.stamina = stamina;
    e.intellect = intellect;
    e.spirit = spirit;
    e.armor = agility * 2; // classic base armor from agility (2/point); item armor folds in later
    e.max_health = max_health_for(ctx, race, class, level);
    e.max_power = max_power_for(ctx, race, class, level);
    old
}

/// Set `character_guid`'s level and recompute `max_health`/`max_power`/the five base attributes/armor
/// from the real stat curve, via `apply_level_stats` (the SAME writer `player_login` and the ding loop
/// use — not reimplemented). Health/power are refilled to the new max. Persists the level to the
/// durable `game_character` row too, so a relog keeps it. Shared by `debug::debug_set_level`
/// (the test-harness lever) and `gm::gm_command`'s `.level` (work-item 223's playtest kit) so the two
/// paths can never drift. Works for an OFFLINE character too (266): the old live-entity requirement
/// silently no-opped every fixture that set a logged-out character's level (exploration set Ginger to 5
/// while she was offline — the wire phase then ran at her real, drifted level), and login rebuilds the
/// entity from the character row anyway. Errors only when NEITHER a live entity nor a character row exists.
pub(crate) fn set_character_level(
    ctx: &ReducerContext,
    character_guid: u64,
    level: u32,
) -> Result<(), String> {
    // Read the character's (race, class) to key the curve (the entity carries neither directly —
    // race/class live in unit_bytes_0 and on the durable character row). Fall back to the entity's
    // current level on a missing character (a creature has no character row → curve uses placeholder).
    let char_row = ctx.db.game_character().guid().find(character_guid);
    let (race, class) = char_row
        .as_ref()
        .map(|c| (c.race, c.class))
        .unwrap_or((1, 1));

    let entities = ctx.db.game_world_entity();
    let live = entities.guid().find(character_guid);
    if live.is_none() && char_row.is_none() {
        return Err(format!(
            "no live entity or character row for guid {character_guid}"
        ));
    }
    if let Some(mut e) = live {
        e.level = level;
        e.xp = 0; // matches the character-row reset below — banked xp must not re-ding past the set level
                  // …and the THRESHOLD, which the xp reset alone does not fix. `grant_xp`'s ding loop compares
                  // `xp >= next_level_xp`, so a character left holding its OLD level's (much smaller) threshold
                  // dings on its very next kill: a bot set to level 10 hit 11 after one wolf, in every bot test
                  // that stages a level. Same defect family as the xp reset above (266) — that fixed the banked
                  // side and left the bar where it was.
        e.next_level_xp = crate::xp::xp_to_next_level(level);
        apply_level_stats(ctx, &mut e, race, class, level); // attributes + armor + max health/power
        e.health = e.max_health;
        e.power = e.max_power; // refill power to the new max
        entities.guid().update(e);
        // Sheet AP/damage-range are level-derived and only ever move via `recompute_sheet` —
        // without this a GM `.level` set leaves the paperdoll's AP/damage numbers frozen at the
        // pre-set level until an unrelated trigger (aura/gear/relog) fires.
        crate::spell::recompute_sheet(ctx, character_guid);
    }

    // Persist the level to the character row so a relog keeps it. XP resets to 0 (266): banked
    // xp survives a level-set otherwise, and any pool past the new level's threshold re-dings
    // the character on the next xp pass — "set level 5" on a played character silently became
    // level 6+, skewing every level-keyed fixture (exploration's L5 discovery paid the L6 value).
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(character_guid) {
        c.level = level as u8;
        c.xp = 0;
        c.next_level_xp = crate::xp::xp_to_next_level(level); // see the entity write above
        chars.guid().update(c);
    }
    spacetimedb::log::info!("set_character_level: guid {character_guid} -> level {level}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level-set must move the DING BAR, not just clear the banked xp.
    ///
    /// `set_character_level` is ctx glue (playbook §7), so this pins its shape: both rows must be
    /// written, and `next_level_xp` must be recomputed alongside the `xp = 0` on each. Dropping
    /// either write reinstates the live defect — a bot staged at level 10 kept level 1's threshold
    /// and dinged to 11 on its first kill, which silently invalidated every bot test written around
    /// "an L10 bot" (kit level, the GRIND ±3 band, rotation `min_level`). The xp half of this pair
    /// was already fixed once (266); the bar was left where it was.
    #[test]
    fn setting_a_level_also_moves_the_ding_threshold() {
        let body = crate::test_scan::code_of(
            include_str!("stats.rs"),
            "pub(crate) fn set_character_level(",
        );
        for (row, needle) in [
            (
                "entity",
                "e.next_level_xp = crate::xp::xp_to_next_level(level)",
            ),
            (
                "character",
                "c.next_level_xp = crate::xp::xp_to_next_level(level)",
            ),
        ] {
            assert!(
                body.contains(needle),
                "`set_character_level` no longer recomputes the {row} row's ding threshold, so a \
                 levelled character dings on its next kill. Body was:\n{body}"
            );
        }
        // The threshold it writes must actually GROW with the level — a constant would satisfy the
        // scan above while leaving the bar at level 1's value.
        assert!(crate::xp::xp_to_next_level(10) > crate::xp::xp_to_next_level(1));
    }

    /// The level-derived stat block (attributes, armor, max health/power) used to be
    /// hand-mirrored at three call sites — `build_player_entity` (login), `grant_xp`'s ding loop, and
    /// `set_character_level` — kept in sync only by a comment ("mirrors build_player_entity"). The
    /// third one drifted and dropped the armor recompute, so a level-SET character kept stale armor
    /// until its next relog. The fix extracts ONE `apply_level_stats` writer; this pins that (a) the
    /// writer still recomputes armor, and (b) all three sites still route through it rather than
    /// re-inlining the curve lookups — a fresh hand-rolled copy at any of them reintroduces the exact
    /// drift class was filed for.
    #[test]
    fn the_level_recompute_writer_and_all_three_call_sites_stay_wired_together() {
        let writer_body =
            crate::test_scan::code_of(include_str!("stats.rs"), "pub(crate) fn apply_level_stats(");
        assert!(
            writer_body.contains("e.armor = agility * 2"),
            "`apply_level_stats` no longer recomputes armor — every caller relies on this ONE writer \
             for it, so dropping it here breaks all three sites at once, silently. Body was:\n{writer_body}"
        );

        let set_level_body = crate::test_scan::code_of(
            include_str!("stats.rs"),
            "pub(crate) fn set_character_level(",
        );
        assert!(
            set_level_body.contains("apply_level_stats(ctx, &mut e, race, class, level)"),
            "`set_character_level` must route the stat recompute through `apply_level_stats` rather \
             than re-inlining the curve lookups — a hand-rolled copy is exactly how #362 dropped \
             armor the first time. Body was:\n{set_level_body}"
        );

        let login_body = crate::test_scan::code_of(
            include_str!("creatures/spawn.rs"),
            "pub fn build_player_entity(",
        );
        assert!(
            login_body.contains(
                "apply_level_stats(ctx, &mut entity, character.race, character.class, level)"
            ),
            "`build_player_entity` (login) must route through `apply_level_stats` too — this is the \
             site the other two are supposed to mirror. Body was:\n{login_body}"
        );

        let ding_body = crate::test_scan::code_of(include_str!("xp.rs"), "pub(crate) fn grant_xp(");
        assert!(
            ding_body.contains("apply_level_stats(ctx, p, race, class, p.level)"),
            "`grant_xp`'s ding loop must route through `apply_level_stats` too. Body was:\n{ding_body}"
        );
    }

    #[test]
    fn stamina_curve_matches_vanilla_breakpoints() {
        assert_eq!(hp_from_stamina(0), 0);
        assert_eq!(hp_from_stamina(20), 20); // 1 HP/pt up to 20
        assert_eq!(hp_from_stamina(21), 30); // each point beyond 20 = 10 HP
                                             // L1 human Warrior: cmangos basehp 20 + stamina 22 → 20 + (20 + 2*10) = 60, the documented value.
        assert_eq!(20 + hp_from_stamina(22), 60);
    }

    #[test]
    fn intellect_curve_matches_vanilla_breakpoints() {
        assert_eq!(mana_from_intellect(20), 20); // 1 mana/pt up to 20
        assert_eq!(mana_from_intellect(21), 35); // each point beyond 20 = 15 mana
    }

    #[test]
    fn starting_power_full_for_mana_and_energy_empty_for_rage() {
        // Mana classes spawn with a full pool (matches login: `power = max_power`).
        assert_eq!(starting_power(power_type::MANA, 1500), 1500);
        // Energy (Rogue) is non-persisted in vanilla — always full at login.
        assert_eq!(starting_power(power_type::ENERGY, 100), 100);
        // Rage always starts at 0 (earned only through combat/damage).
        assert_eq!(starting_power(power_type::RAGE, 1000), 0);
        // A zero-mana class (curve not loaded) stays 0 even on the mana branch.
        assert_eq!(starting_power(power_type::MANA, 0), 0);
    }

    #[test]
    fn keys_pack_class_race_level_uniquely() {
        assert_eq!(class_level_key(1, 1), (1 << 8) | 1);
        assert_eq!(class_level_key(9, 60), (9 << 8) | 60);
        assert_ne!(class_level_key(1, 60), class_level_key(60, 1));
        assert_eq!(race_class_level_key(1, 1, 1), (1 << 16) | (1 << 8) | 1);
        assert_ne!(race_class_level_key(1, 2, 3), race_class_level_key(3, 2, 1));
    }
}
