//! Character-sheet armor from the coordinator's entity, aura, item and catalogue caches.
//! The fold mirrors Module combat armor, including applied enchantments and Random Properties.
//! Other character-sheet values come directly from the Module's derived entity fields.

use super::bindings::*;
use spacetimedb_sdk::Table;
use std::collections::HashMap;

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

/// Everything the paperdoll shows that GEAR/AURAS move on the STR/AGI/STA/INT/SPI/AP/damage-range/crit
/// half of the sheet (#517 + #532) — a plain row read of `module::spell::recompute_sheet`'s output,
/// never a second fold. `strength`/`agility`/`stamina`/`intellect`/`spirit` are the BASE attributes (the
/// white `UNIT_FIELD_STAT0..4` total); `*_bonus` is the SIGNED aura+gear(+enchant) delta the caller
/// splits into the green/red `PLAYER_FIELD_POSSTAT`/`NEGSTAT` pair via `.max(0)`/`.min(0)` — sign
/// arithmetic only, not aura interpretation, so no drift risk re-enters the gateway. `attack_power` is
/// the stat-derived base (folds effective STR/AGI, so a +STR trinket already moves it) and `ap_mods` is
/// the `A_MOD_COMBAT(ATTACK_POWER)` aura portion alone (Battle Shout) — vanilla renders those through
/// two different wire fields. `dmg_min`/`dmg_max` are `combat::swing_range_ctx`'s own numbers (weapon +
/// AP + disarm folded in module-side), so the sheet can never show a range the swing doesn't roll.
/// `crit_pct` is `sheet_crit_bp`/100.0 — `combat::effective_crit_bp`'s output, the exact band the swing
/// table rolls a crit against.
pub(crate) fn sheet_stats(db: &RemoteTables, guid: u64) -> Option<crate::codec::SheetStatsValues> {
    let e = db.game_world_entity().guid().find(&guid)?;
    // `e.strength`/`agility`/`stamina`/`intellect` are BASE only (unlike `e.spirit`, which
    // `recompute_vitals` already overwrites to the effective value) — the wire field is the white
    // EFFECTIVE total (base + bonus), matching the armor half's `total` (see module docs above), so
    // add the signed `sheet_*_bonus` delta back in here rather than sending the base straight through.
    Some(crate::codec::SheetStatsValues {
        strength: (e.strength as i32 + e.sheet_str_bonus).max(0) as u32,
        agility: (e.agility as i32 + e.sheet_agi_bonus).max(0) as u32,
        stamina: (e.stamina as i32 + e.sheet_sta_bonus).max(0) as u32,
        intellect: (e.intellect as i32 + e.sheet_int_bonus).max(0) as u32,
        spirit: e.spirit,
        str_bonus: e.sheet_str_bonus,
        agi_bonus: e.sheet_agi_bonus,
        sta_bonus: e.sheet_sta_bonus,
        int_bonus: e.sheet_int_bonus,
        spi_bonus: e.sheet_spi_bonus,
        attack_power: e.sheet_ap_base,
        ap_mods: e.sheet_ap_mods,
        dmg_min: e.sheet_dmg_min,
        dmg_max: e.sheet_dmg_max,
        ranged_attack_power: e.sheet_ranged_ap,
        ranged_dmg_min: e.sheet_ranged_dmg_min,
        ranged_dmg_max: e.sheet_ranged_dmg_max,
        // #532: PLAYER_CRIT_PERCENTAGE wants a float percent; `sheet_crit_bp` is basis points
        // (100 bp == 1%), so divide by 100.0 — the sheet value IS `effective_crit_bp`'s output,
        // no second crit formula.
        crit_pct: e.sheet_crit_bp as f32 / 100.0,
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
    // The SDK exposes no non-unique enchantment index. Read the catalogue once per projection.
    let mut enchantments = HashMap::<u32, i32>::new();
    for effect in db.game_item_enchantment().iter().filter(|effect| {
        effect.enchant_id != 0 && effect.kind == lyracore_shared::item_property::ARMOR
    }) {
        let total = enchantments.entry(effect.enchant_id).or_default();
        *total = total.saturating_add(effect.amount);
    }
    let enchant_armor = |id| enchantments.get(&id).copied().unwrap_or(0);
    let templates = db.game_item_template();
    let gear_sum: i32 = db
        .game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == guid && i.slot <= EQUIP_REGION_END)
        .map(|i| {
            templates
                .entry()
                .find(&i.entry)
                .map(|t| {
                    if t.max_durability > 0 && i.durability == 0 {
                        return 0;
                    }
                    let mut armor = t.stat_armor.saturating_add(enchant_armor(i.enchant_id));
                    if i.random_property_id != 0 {
                        if let Some(property) = db
                            .game_item_random_property()
                            .property_id()
                            .find(&i.random_property_id)
                        {
                            let bonus = [
                                property.enchant_id_1,
                                property.enchant_id_2,
                                property.enchant_id_3,
                            ]
                            .into_iter()
                            .map(&enchant_armor)
                            .fold(0i32, i32::saturating_add);
                            armor = armor.saturating_add(bonus);
                        }
                    }
                    gear_armor_contribution(i.slot, armor, t.max_durability, i.durability)
                })
                .unwrap_or(0) // a missing template join never poisons the sum (matches the module)
        })
        .fold(0i32, i32::saturating_add);
    (base + aura_sum + i64::from(gear_sum)).max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
    fn armor_projection_adds_property_effects_only_while_the_item_is_worn_and_unbroken() {
        use crate::accept::BlockingTaskCapacity;
        use crate::config::GatewayConfig;
        use crate::durable_test_support::{poll_until, Standalone, POLL_TIMEOUT};
        use crate::stdb::Coordinator;

        let mut shard = Standalone::start("property-armor");
        shard.publish_module();
        shard.assert_call("claim_operator", &[]);
        shard.assert_call("debug_seed_scenario_fixtures", &[]);
        shard.assert_call("debug_spawn_player_entity", &["1"]);
        shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
        shard.assert_sql("UPDATE game_item_template SET stat_armor = 19 WHERE entry = 5090050");
        shard.assert_sql("INSERT INTO game_item_random_property (property_id,enchant_id_1,enchant_id_2,enchant_id_3,suffix) VALUES (5090101,5090103,0,0,'of the Fixture')");
        shard.assert_sql("INSERT INTO game_item_enchantment (id,enchant_id,effect_index,kind,amount,spell_id,school_mask) VALUES (1303066368,5090103,0,14,2,0,0),(1303066369,5090103,1,14,3,0,0),(1303066624,5090104,0,14,11,0,0)");
        shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
        shard.assert_sql("UPDATE game_item_instance SET random_property_id = 5090101, enchant_id = 5090104 WHERE owner_guid = 1");
        let base: u32 = shard.query_rows("SELECT armor FROM game_world_entity WHERE guid = 1")[0]
            ["armor"]
            .parse()
            .unwrap();
        let cfg = GatewayConfig {
            logon_bind: "127.0.0.1:0".into(),
            world_bind: "127.0.0.1:0".into(),
            stdb_uri: shard.server().into(),
            module_name: shard.shard_name().into(),
            coordinator_token: Some(shard.owner_token()),
            gateway_id: "property-armor-test".into(),
            blocking_task_capacity: BlockingTaskCapacity::new(1),
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let coordinator = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
        assert_eq!(coordinator.effective_armor(1), base);
        shard.assert_call("debug_equip_item", &["1", "23"]);
        assert!(poll_until(POLL_TIMEOUT, || coordinator.effective_armor(1) == base + 35));
        shard.assert_sql("UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1");
        assert!(poll_until(POLL_TIMEOUT, || coordinator.effective_armor(1) == base));
    }

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

    // The former `melee_attack_power_for`/`swing_range_ap` gateway-side mirrors (and the parity tests
    // that pinned them against `lyracore_module`) are GONE (#517): `sheet_stats` above is now a plain
    // read of `module::spell::recompute_sheet`'s output, so there is no gateway-side formula left to
    // drift from the module — the module row IS the source of truth.

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
