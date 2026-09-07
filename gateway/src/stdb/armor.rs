//! Character-sheet resistances from the coordinator's entity, aura, item and catalogue caches.
//! The fold mirrors Module combat resistance, including applied enchantments and Random Properties.
//! Other character-sheet values come directly from the Module's derived entity fields.

use super::bindings::*;
use spacetimedb_sdk::Table;
use std::collections::HashMap;

/// Aura kind `A_MOD_RESISTANCE` — a direct resistance/armor bonus (e.g. Demon Skin). Mirrors
/// `module::spell::taxonomy::A_MOD_RESISTANCE = 0xA1`.
const A_MOD_RESISTANCE: u8 = 0xA1;
const RESISTANCE_BITS: [u32; 7] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40];
const RESISTANCE_KINDS: [u8; 7] = [
    lyracore_shared::item_property::ARMOR,
    lyracore_shared::item_property::HOLY_RESISTANCE,
    lyracore_shared::item_property::FIRE_RESISTANCE,
    lyracore_shared::item_property::NATURE_RESISTANCE,
    lyracore_shared::item_property::FROST_RESISTANCE,
    lyracore_shared::item_property::SHADOW_RESISTANCE,
    lyracore_shared::item_property::ARCANE_RESISTANCE,
];
/// Inclusive upper bound of the equipment region (`equip_slot::END == TABARD == 18`). Only worn pieces in
/// slots 0..=18 carry armor; bags/backpack/bank don't count. Mirrors `items::equipped_stat_bonus`.
const EQUIP_REGION_END: u8 = 18;

/// One aura row's armor contribution: `amount × max(stacks, 1)` IF it is an `A_MOD_RESISTANCE` aura whose
/// frozen `eff_p0` school mask overlaps `RESIST_ARMOR` (bit 0); else 0. Mirrors the module's
/// `resistance_bonus` predicate + `stacked_amount`: `stacks` is floored at 1 so a legacy 0-stack row still
/// counts ONCE, and a negative `amount × stacks` (Sunder Armor) subtracts. Pure → unit-tested.
fn aura_resistance_contribution(
    eff_kind: u8,
    eff_p0: i32,
    amount: i32,
    stacks: u8,
    school_bit: u32,
) -> i32 {
    if eff_kind == A_MOD_RESISTANCE && (eff_p0 as u32 & school_bit) != 0 {
        amount.saturating_mul(i32::from(stacks.max(1)))
    } else {
        0
    }
}

fn aura_armor_contribution(eff_kind: u8, eff_p0: i32, amount: i32, stacks: u8) -> i32 {
    aura_resistance_contribution(eff_kind, eff_p0, amount, stacks, RESISTANCE_BITS[0])
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

/// Effective armor followed by Holy, Fire, Nature, Frost, Shadow and Arcane resistance.
/// The order matches `UNIT_FIELD_RESISTANCES[0..=6]`.
pub(crate) fn effective_resistances(db: &RemoteTables, guid: u64) -> [u32; 7] {
    let base_armor = db
        .game_world_entity()
        .guid()
        .find(&guid)
        .map(|e| e.armor as i64)
        .unwrap_or(0);
    let mut aura = [0i64; 7];
    for row in db.game_aura().iter().filter(|a| a.target_guid == guid) {
        for (index, school_bit) in RESISTANCE_BITS.into_iter().enumerate() {
            aura[index] += i64::from(aura_resistance_contribution(
                row.eff_kind,
                row.eff_p0,
                row.amount,
                row.stacks,
                school_bit,
            ));
        }
    }
    // The SDK exposes no non-unique enchantment index. Read the catalogue once per projection.
    let mut enchantments = HashMap::<(u32, u8), i32>::new();
    for effect in db
        .game_item_enchantment()
        .iter()
        .filter(|effect| effect.enchant_id != 0 && RESISTANCE_KINDS.contains(&effect.kind))
    {
        let total = enchantments
            .entry((effect.enchant_id, effect.kind))
            .or_default();
        *total = total.saturating_add(effect.amount);
    }
    let enchant_stat = |id, kind| enchantments.get(&(id, kind)).copied().unwrap_or(0);
    let templates = db.game_item_template();
    let mut gear = [0i32; 7];
    for item in db
        .game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == guid && i.slot <= EQUIP_REGION_END)
    {
        let Some(tmpl) = templates.entry().find(&item.entry) else {
            continue;
        };
        if tmpl.max_durability > 0 && item.durability == 0 {
            continue;
        }
        let template = [
            tmpl.stat_armor,
            tmpl.holy_res,
            tmpl.fire_res,
            tmpl.nature_res,
            tmpl.frost_res,
            tmpl.shadow_res,
            tmpl.arcane_res,
        ];
        let property = db
            .game_item_random_property()
            .property_id()
            .find(&item.random_property_id);
        for index in 0..RESISTANCE_KINDS.len() {
            let kind = RESISTANCE_KINDS[index];
            let mut amount = template[index].saturating_add(enchant_stat(item.enchant_id, kind));
            if let Some(property) = &property {
                let property_amount = [
                    property.enchant_id_1,
                    property.enchant_id_2,
                    property.enchant_id_3,
                ]
                .into_iter()
                .map(|id| enchant_stat(id, kind))
                .fold(0i32, i32::saturating_add);
                amount = amount.saturating_add(property_amount);
            }
            let amount = if index == 0 {
                gear_armor_contribution(item.slot, amount, tmpl.max_durability, item.durability)
            } else {
                amount
            };
            gear[index] = gear[index].saturating_add(amount);
        }
    }
    std::array::from_fn(|index| {
        let base = if index == 0 { base_armor } else { 0 };
        (base + aura[index] + i64::from(gear[index])).clamp(0, i64::from(u32::MAX)) as u32
    })
}

pub(crate) fn effective_armor(db: &RemoteTables, guid: u64) -> u32 {
    effective_resistances(db, guid)[0]
}

pub(crate) fn effective_magic_resistances(db: &RemoteTables, guid: u64) -> [u32; 6] {
    effective_resistances(db, guid)[1..]
        .try_into()
        .expect("the magic resistance slice has six schools")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
    fn resistance_projection_adds_property_effects_only_while_the_item_is_worn_and_unbroken() {
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
        shard.assert_sql("UPDATE game_item_template SET stat_armor = 19, holy_res = 1, fire_res = 2, nature_res = 3, frost_res = 4, shadow_res = 5, arcane_res = 6 WHERE entry = 5090050");
        shard.assert_sql("INSERT INTO game_item_random_property (property_id,enchant_id_1,enchant_id_2,enchant_id_3,suffix) VALUES (5090101,5090103,0,0,'of the Fixture')");
        shard.assert_sql("INSERT INTO game_item_enchantment (id,enchant_id,effect_index,kind,amount,spell_id,school_mask) VALUES (1303066368,5090103,0,14,2,0,0),(1303066369,5090103,1,14,3,0,0),(1303066370,5090103,2,8,10,0,2),(1303066371,5090103,3,9,20,0,4),(1303066372,5090103,4,10,30,0,8),(1303066373,5090103,5,11,40,0,16),(1303066374,5090103,6,12,50,0,32),(1303066375,5090103,7,13,60,0,64),(1303066624,5090104,0,14,11,0,0),(1303066625,5090104,1,9,7,0,4)");
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
        assert_eq!(coordinator.effective_magic_resistances(1), [0; 6]);
        shard.assert_call("debug_equip_item", &["1", "23"]);
        assert!(poll_until(POLL_TIMEOUT, || coordinator.effective_armor(1) == base + 35));
        assert!(poll_until(POLL_TIMEOUT, || {
            coordinator.effective_magic_resistances(1) == [11, 29, 33, 44, 55, 66]
        }));
        shard.assert_sql("UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1");
        assert!(poll_until(POLL_TIMEOUT, || coordinator.effective_armor(1) == base));
        assert!(poll_until(POLL_TIMEOUT, || {
            coordinator.effective_magic_resistances(1) == [0; 6]
        }));
    }

    #[test]
    fn armor_aura_contribution_matches_module_predicate() {
        // A_MOD_RESISTANCE(0xA1) whose eff_p0 mask includes RESIST_ARMOR(0x01) → amount × max(stacks,1).
        assert_eq!(aura_armor_contribution(0xA1, 0x01, 160, 1), 160); // Demon Skin-style flat +160
                                                                      // stacks floored at 1: a legacy 0-stack row still counts ONCE (not zero).
        assert_eq!(aura_armor_contribution(0xA1, 0x01, 160, 0), 160);
        // Stacking, negative (Sunder Armor-style): amount × stacks subtracts.
        assert_eq!(aura_armor_contribution(0xA1, 0x01, -90, 5), -450);
        assert_eq!(
            aura_armor_contribution(0xA1, 0x01, i32::MAX, u8::MAX),
            i32::MAX
        );
        assert_eq!(
            aura_armor_contribution(0xA1, 0x01, i32::MIN, u8::MAX),
            i32::MIN
        );
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
