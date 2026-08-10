//! The gateway-side EFFECTIVE-armor fold (Approach B) for the character sheet's "Armor" readout
//! (`UNIT_FIELD_RESISTANCES[0]`). The MODULE is left byte-identical: `module::combat::effective_armor`
//! stays an on-demand combat fold and `e.armor` stays BASE (agility*2), so physical-mitigation is
//! untouched. This recomputes the same EFFECTIVE value the sheet must DISPLAY — base armor + every
//! `A_MOD_RESISTANCE(armor)` aura + the armor summed across equipped gear — purely from a connection's
//! subscription cache, so no module change is needed to drive it live.
//!
//! The SAME fold runs at two sites with two caches:
//!   - player CREATE on the COORDINATOR cache (base + gear; `game_aura` isn't in that cache → the aura
//!     term is 0, which the on_aura relay corrects the instant a login-present aura inserts), and
//!   - the aura / gear relays on the PER-PLAYER cache (base + auras + gear; all three tables subscribed).
//!
//! It mirrors `module::combat::effective_armor` term-for-term: `e.armor` base, `spell::resistance_bonus`
//! (A_MOD_RESISTANCE armor auras, amount×stacks), `items::equipped_stat_bonus(Armor)` (worn `stat_armor`,
//! broken-skip). No armor ENCHANT exists in the module (its `ENCHANTS` table is STR/STA only), so there is
//! no enchant overlay term and the gateway matches the module exactly for armor.

use super::bindings::*;
use spacetimedb_sdk::Table;

/// Aura kind `A_MOD_RESISTANCE` — a direct resistance/armor bonus (e.g. Demon Skin). Mirrors
/// `module::spell::taxonomy::A_MOD_RESISTANCE = 0xA1`.
const A_MOD_RESISTANCE: u8 = 0xA1;
/// `RESIST_ARMOR` school bit. The aura's `eff_p0` is a school MASK, so test `& bit != 0` (NOT `== 1`),
/// matching `module::spell::math::resistance_bonus`. Mirrors `module::spell::taxonomy::RESIST_ARMOR = 0x01`.
const RESIST_ARMOR_MASK: u32 = 0x01;
/// Inclusive upper bound of the equipment region (`equip_slot::END == TABARD == 18`). Only worn pieces in
/// slots 0..=18 carry armor; bags/backpack/bank don't count. Mirrors `items::equipped_stat_bonus`.
const EQUIP_REGION_END: u8 = 18;

/// One aura row's armor contribution: `amount × max(stacks, 1)` IF it is an `A_MOD_RESISTANCE` aura whose
/// frozen `eff_p0` school mask overlaps `RESIST_ARMOR` (bit 0); else 0. Mirrors the module's
/// `resistance_bonus` predicate + `stacked_amount`: `stacks` is floored at 1 so a legacy 0-stack row still
/// counts ONCE, and a negative `amount × stacks` (Sunder Armor) subtracts. Pure → unit-tested.
fn aura_armor_contribution(eff_kind: u8, eff_p0: i32, amount: i32, stacks: u8) -> i32 {
    if eff_kind == A_MOD_RESISTANCE && (eff_p0 as u32 & RESIST_ARMOR_MASK) != 0 {
        amount * (stacks.max(1) as i32)
    } else {
        0
    }
}

/// One equipped item's armor contribution: its template `stat_armor`, or 0 if the item is outside the
/// equipment region (`slot > 18`) or BROKEN (`max_durability > 0 && durability == 0`). Mirrors the
/// module's `equipped_stat_bonus(Armor)` (slot gate + `item_is_broken`). A no-durability item
/// (`max_durability == 0`, e.g. a cloak) is never broken. Pure → unit-tested.
fn gear_armor_contribution(slot: u8, stat_armor: i32, max_durability: u32, durability: u32) -> i32 {
    if slot > EQUIP_REGION_END {
        return 0;
    }
    if max_durability > 0 && durability == 0 {
        return 0; // broken → grants no armor until repaired
    }
    stat_armor
}

/// Everything the paperdoll shows that GEAR moves, folded from the subscription cache the
/// same way `effective_armor` below is: the five attributes = entity base + equipped `stat_*`
/// bonuses (worn slots 0..=18, broken pieces excluded), melee attack power from the module's
/// `combat::tables` curves (mirrored — Rogue/Hunter agility curve, Str-class default), and the
/// melee damage range = main-hand tooltip damage + the `AP/14 × speed` fold (the SAME numbers the
/// server rolls, ending the "sheet says 5-7 but swings land differently" confusion). CEILING (noted,
/// not wired): AURA contributions (+stat buffs, Battle Shout AP) move combat but not this sheet —
/// the aura relay is the follow-up.
/// Mirror of `module::combat::tables::melee_attack_power_for` (Rogue 4 / Hunter 3 use the
/// agility curve `level*2 + Str + Agi - 20`; everyone else `level*3 + Str*2 - 20`).
fn melee_attack_power_for(class: u8, strength: u32, agility: u32, level: u32) -> u32 {
    match class {
        3 | 4 => (level * 2 + strength + agility).saturating_sub(20),
        _ => (level * 3 + strength * 2).saturating_sub(20),
    }
}

/// Mirror of `module::combat::tables::weapon_swing_range_ap` (AP_PER_DPS = 14) — also covers the
/// unarmed shape when called with the unarmed base range (1..=3) and the unit's base attack time.
fn swing_range_ap(ap: u32, dmg_min: u32, dmg_max: u32, delay_ms: u32) -> (u32, u32) {
    let bonus = ap * delay_ms / (14 * 1000);
    (dmg_min + bonus, dmg_max + bonus)
}

pub(crate) fn sheet_stats(db: &RemoteTables, guid: u64) -> Option<crate::codec::SheetStatsValues> {
    let e = db.game_world_entity().iter().find(|e| e.guid == guid)?;
    // Gear fold: one pass over the owner's worn, unbroken items joined to their templates.
    let (mut g_str, mut g_agi, mut g_sta, mut g_int, mut g_spi) = (0i32, 0i32, 0i32, 0i32, 0i32);
    let mut main_hand: Option<(u32, u32, u32)> = None; // (dmg_min, dmg_max, delay_ms)
    for item in db
        .game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == guid && i.slot <= 18)
    {
        let Some(t) = db
            .game_item_template()
            .iter()
            .find(|t| t.entry == item.entry)
        else {
            continue;
        };
        if t.max_durability > 0 && item.durability == 0 {
            continue; // broken grants nothing (same rule as gear_armor_contribution)
        }
        g_str += t.stat_strength;
        g_agi += t.stat_agility;
        g_sta += t.stat_stamina;
        g_int += t.stat_intellect;
        g_spi += t.stat_spirit;
        if item.slot == 15 && t.damage_max > 0.0 {
            main_hand = Some((t.damage_min as u32, t.damage_max as u32, t.delay_ms));
        }
    }
    let fold = |base: u32, bonus: i32| (base as i32 + bonus).max(0) as u32;
    let strength = fold(e.strength, g_str);
    let agility = fold(e.agility, g_agi);
    let class = (e.unit_bytes_0 >> 8) as u8; // unpack4 byte 1 = class (codec::entity convention)
    let level = e.level;
    let attack_power = melee_attack_power_for(class, strength, agility, level);
    let (dmg_min, dmg_max) = match main_hand {
        Some((mn, mx, delay)) => swing_range_ap(attack_power, mn, mx, delay),
        None => swing_range_ap(attack_power, 1, 3, e.base_attack_time_ms), // unarmed base 1..=3
    };
    Some(crate::codec::SheetStatsValues {
        strength,
        agility,
        stamina: fold(e.stamina, g_sta),
        intellect: fold(e.intellect, g_int),
        spirit: fold(e.spirit, g_spi),
        gear_str: g_str,
        gear_agi: g_agi,
        gear_sta: g_sta,
        gear_int: g_int,
        gear_spi: g_spi,
        attack_power,
        dmg_min,
        dmg_max,
    })
}

/// The POSITIVE aura portion of `guid`'s armor: the sum of positive `A_MOD_RESISTANCE(armor)`
/// contributions only — what the paperdoll renders as the green "(+N)" via
/// `UNIT_FIELD_RESISTANCEBUFFMODSPOSITIVE[0]`. Negative auras (Sunder) are excluded (they'd ride the
/// NEGATIVE twin field — not wired this slice; the ceiling is a red armor readout instead of white).
pub(crate) fn aura_armor_positive(db: &RemoteTables, guid: u64) -> u32 {
    db.game_aura()
        .iter()
        .filter(|a| a.target_guid == guid)
        .map(|a| aura_armor_contribution(a.eff_kind, a.eff_p0, a.amount, a.stacks))
        .filter(|&c| c > 0)
        .sum::<i32>() as u32
}

/// The EFFECTIVE armor for `guid`, folded from the connection's subscription cache `db`:
/// `max(0, base + Σ armor-aura(amount×stacks) + Σ equipped-gear stat_armor)`. Reproduces
/// `module::combat::effective_armor` term-for-term, so the character-sheet readout EQUALS the armor
/// combat mitigates with. The aura term is naturally 0 on a cache not subscribed to `game_aura` (the
/// coordinator's, used at CREATE — the on_aura relay re-pushes the moment an aura inserts); the
/// per-player cache (relays) carries all three tables. A guid with no entity row → base 0 → returns 0.
pub(crate) fn effective_armor(db: &RemoteTables, guid: u64) -> u32 {
    let base = db
        .game_world_entity()
        .guid()
        .find(&guid)
        .map(|e| e.armor as i64)
        .unwrap_or(0);
    let aura_sum: i64 = db
        .game_aura()
        .iter()
        .filter(|a| a.target_guid == guid)
        .map(|a| aura_armor_contribution(a.eff_kind, a.eff_p0, a.amount, a.stacks) as i64)
        .sum();
    let templates = db.game_item_template();
    let gear_sum: i64 = db
        .game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == guid)
        .map(|i| {
            templates
                .entry()
                .find(&i.entry)
                .map(|t| {
                    gear_armor_contribution(i.slot, t.stat_armor, t.max_durability, i.durability)
                        as i64
                })
                .unwrap_or(0) // a missing template join never poisons the sum (matches the module)
        })
        .sum();
    (base + aura_sum + gear_sum).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_aura_contribution_matches_module_predicate() {
        // A_MOD_RESISTANCE(0xA1) whose eff_p0 mask includes RESIST_ARMOR(0x01) → amount × max(stacks,1).
        assert_eq!(aura_armor_contribution(0xA1, 0x01, 160, 1), 160); // Demon Skin-style flat +160
                                                                      // stacks floored at 1: a legacy 0-stack row still counts ONCE (not zero).
        assert_eq!(aura_armor_contribution(0xA1, 0x01, 160, 0), 160);
        // Stacking, negative (Sunder Armor-style): amount × stacks subtracts.
        assert_eq!(aura_armor_contribution(0xA1, 0x01, -90, 5), -450);
        // A school MASK that merely INCLUDES the armor bit still counts (mask test, not ==).
        assert_eq!(aura_armor_contribution(0xA1, 0x03, 50, 1), 50);
        // A non-armor school (eff_p0 == 0x02, e.g. holy) does NOT touch armor.
        assert_eq!(aura_armor_contribution(0xA1, 0x02, 160, 1), 0);
        // A different aura kind (not A_MOD_RESISTANCE) is ignored.
        assert_eq!(aura_armor_contribution(0x1D, 0x01, 160, 1), 0);
    }

    #[test]
    fn gear_armor_contribution_gates_region_and_broken() {
        // A worn chest (slot 4) with 120 armor contributes it.
        assert_eq!(gear_armor_contribution(4, 120, 100, 100), 120);
        // BROKEN (max_durability > 0, durability == 0) → grants nothing until repaired.
        assert_eq!(gear_armor_contribution(4, 120, 100, 0), 0);
        // A no-durability item (max_durability == 0, e.g. a cloak) is NEVER broken → contributes.
        assert_eq!(gear_armor_contribution(14, 30, 0, 0), 30);
        // Outside the equipment region (a backpack slot 23) contributes nothing.
        assert_eq!(gear_armor_contribution(23, 120, 100, 100), 0);
        // The region boundary (TABARD == 18) is INCLUSIVE.
        assert_eq!(gear_armor_contribution(18, 5, 0, 0), 5);
    }

    /// The two curves above are HAND-MIRRORS of `module::combat::tables` (see their doc comments),
    /// and the release-boundary hardening pass's first coverage baseline found neither of them
    /// pinned by anything: the character sheet could drift from the numbers the server actually rolls and every offline test would
    /// stay green. Same drift class `gateway/tests/schema_parity.rs` guards for table shape, same
    /// answer — compare against the REAL module function through the `lyracore-module`
    /// dev-dependency (`module/src/lib.rs` already `pub use combat::*`), never against a second copy
    /// of the formula, because a copy drifts along with the mirror it is supposed to catch.
    ///
    /// Every class id, not a sample: the mirror's whole risk is its `3 | 4` arm splitting from the
    /// module's `CLASS_HUNTER | CLASS_ROGUE` one, which only an exhaustive sweep catches.
    #[cfg(target_os = "linux")] // needs the lyracore-module dev-dep — see gateway/Cargo.toml
    #[test]
    fn sheet_attack_power_mirror_matches_the_module_curve_for_every_class() {
        for class in 0u8..=11 {
            for (strength, agility, level) in [
                (1, 1, 1),
                (20, 20, 1),
                (20, 40, 5),
                (100, 60, 40),
                (0, 0, 1),
            ] {
                assert_eq!(
                    melee_attack_power_for(class, strength, agility, level),
                    lyracore_module::melee_attack_power_for(class, strength, agility, level),
                    "sheet AP mirror drifted from module::combat::tables::melee_attack_power_for \
                     at class {class}, str {strength}, agi {agility}, level {level}"
                );
            }
        }
        // The saturating floor is part of the contract, not an accident: a level-1 naked caster's
        // `level*3 + Str*2 - 20` is negative and must clamp to 0 on BOTH sides, not wrap.
        assert_eq!(melee_attack_power_for(1, 0, 0, 1), 0);
    }

    /// `swing_range_ap` mirrors TWO module fns at once — `weapon_swing_range_ap` when the sheet has
    /// a main hand, and `player_swing_range_ap` when it does not (`sheet_stats` calls it with the
    /// unarmed base range spelled out as the literal `1, 3`). Both arms are pinned here, so that
    /// literal cannot drift from the module's own `UNARMED_BASE_MIN`/`MAX` either.
    #[cfg(target_os = "linux")] // needs the lyracore-module dev-dep — see gateway/Cargo.toml
    #[test]
    fn sheet_swing_range_mirror_matches_the_module_weapon_and_unarmed_folds() {
        for ap in [0, 30, 140, 1000] {
            for (dmg_min, dmg_max, delay_ms) in
                [(1, 3, 2000), (5, 9, 1600), (43, 65, 3400), (0, 0, 1000)]
            {
                assert_eq!(
                    swing_range_ap(ap, dmg_min, dmg_max, delay_ms),
                    lyracore_module::weapon_swing_range_ap(ap, dmg_min, dmg_max, delay_ms),
                    "sheet weapon swing mirror drifted at ap {ap}, {dmg_min}-{dmg_max}, {delay_ms}ms"
                );
            }
            // The unarmed call shape `sheet_stats` uses: base range 1..=3 + the same AP fold.
            for attack_time_ms in [1000, 2000, 3400] {
                assert_eq!(
                    swing_range_ap(ap, 1, 3, attack_time_ms),
                    lyracore_module::player_swing_range_ap(ap, attack_time_ms),
                    "sheet UNARMED swing mirror drifted at ap {ap}, {attack_time_ms}ms — the \
                     literal `1, 3` in sheet_stats no longer matches the module's unarmed base"
                );
            }
        }
    }

    #[test]
    fn aura_and_gear_terms_compose_additively_before_the_caller_clamps() {
        // `effective_armor` itself needs a live `RemoteTables` subscription cache and is NOT
        // unit-tested here (no ReducerContext mocking) — this only pins that its two REAL pure
        // terms (`aura_armor_contribution`, `gear_armor_contribution`) compose by simple addition
        // with a base, and that the max(0, ..) clamp `effective_armor` applies to that same sum
        // never underflows on a debuff exceeding base + gear.
        let base = 40i64; // agility 20 * 2
        let aura = aura_armor_contribution(0xA1, 0x01, 160, 1) as i64; // Demon Skin
        let gear = gear_armor_contribution(4, 120, 100, 100) as i64; // a chest
        assert_eq!(base + aura + gear, 320);
        // A debuff exceeding base + gear clamps at 0 (never underflows the u32).
        let big_debuff = aura_armor_contribution(0xA1, 0x01, -1000, 1) as i64;
        assert_eq!((base + big_debuff + gear).max(0) as u32, 0);
    }
}
