use super::bindings::*;
use super::connection::LiveConn;

use lyracore_shared::{
    aura::AuraAmount,
    resistance::{self, ResistanceItem},
};

fn aura_values(live: &LiveConn, guid: u64) -> Vec<AuraAmount> {
    live.combat_reads
        .read()
        .unwrap()
        .auras(&live.conn.db, guid)
        .into_iter()
        .map(|a| AuraAmount {
            kind: a.eff_kind,
            parameter: a.eff_p_0,
            amount: a.amount,
            stacks: a.stacks,
        })
        .collect()
}

pub(crate) fn sheet_stats(db: &RemoteTables, guid: u64) -> Option<crate::codec::SheetStatsValues> {
    let e = db.game_world_entity().guid().find(&guid)?;
    // Strength, agility, stamina and intellect are base values; spirit is already effective.
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
        // PLAYER_CRIT_PERCENTAGE uses percent; sheet_crit_bp uses basis points.
        crit_pct: e.sheet_crit_bp as f32 / 100.0,
    })
}

pub(crate) fn aura_armor_positive(live: &LiveConn, guid: u64) -> u32 {
    live.with_cache_snapshot(|| resistance::positive_armor(aura_values(live, guid)))
}

/// Descriptor order is armor, Holy, Fire, Nature, Frost, Shadow, Arcane.
pub(crate) fn effective_resistances(live: &LiveConn, guid: u64) -> [u32; 7] {
    live.with_cache_snapshot(|| {
        let db = &live.conn.db;
        let entity = db.game_world_entity().guid().find(&guid);
        let base_armor = entity.as_ref().map_or(0, |e| e.armor);
        let form_spell = entity.as_ref().and_then(|entity| {
            use lyracore_shared::spell::taxonomy::{STANCE_BEAR, STANCE_DIRE_BEAR};
            if (entity.unit_bytes_0 >> 8) & 0xFF != 11 {
                return None;
            }
            match entity.stance {
                STANCE_BEAR => Some(1178u64),
                STANCE_DIRE_BEAR => Some(9635u64),
                _ => None,
            }
        });
        let form_armor_percent = form_spell
            .and_then(|spell| db.game_spell_effect().id().find(&(spell << 2)))
            .map_or(0, |effect| effect.base_points.max(0));
        let auras = aura_values(live, guid);
        let items: Vec<_> = live
            .inventory
            .read()
            .unwrap()
            .items(db, guid)
            .into_iter()
            .filter_map(|item| {
                let tmpl = db.game_item_template().entry().find(&item.entry)?;
                let property = (item.random_property_id != 0)
                    .then(|| {
                        db.game_item_random_property()
                            .property_id()
                            .find(&item.random_property_id)
                    })
                    .flatten();
                Some(ResistanceItem {
                    slot: item.slot,
                    max_durability: tmpl.max_durability,
                    durability: item.durability,
                    template: [
                        tmpl.stat_armor,
                        tmpl.holy_res,
                        tmpl.fire_res,
                        tmpl.nature_res,
                        tmpl.frost_res,
                        tmpl.shadow_res,
                        tmpl.arcane_res,
                    ],
                    enchant_id: item.enchant_id,
                    property_enchants: property
                        .map_or([0; 3], |p| [p.enchant_id_1, p.enchant_id_2, p.enchant_id_3]),
                })
            })
            .collect();
        let enchant_stat = |id, kind| {
            lyracore_shared::item_property::enchant_stat(
                kind,
                u32::MAX,
                live.combat_reads
                    .read()
                    .unwrap()
                    .enchantments(db, id)
                    .into_iter()
                    .map(|effect| (effect.kind, effect.amount, effect.school_mask)),
            )
        };
        let gear_armor = resistance::resistance_bonus(1, [], items.iter().copied(), enchant_stat);
        let form_armor = (i64::from(gear_armor.max(0)) * i64::from(form_armor_percent) / 100)
            .min(i64::from(i32::MAX)) as i32;
        std::array::from_fn(|index| {
            let mut bonus = resistance::resistance_bonus(
                resistance::SCHOOL_BITS[index],
                auras.iter().copied(),
                items.iter().copied(),
                enchant_stat,
            );
            if index == 0 {
                bonus = bonus.saturating_add(form_armor);
            }
            resistance::effective_resistance(if index == 0 { base_armor } else { 0 }, bonus)
        })
    })
}

pub(crate) fn effective_armor(live: &LiveConn, guid: u64) -> u32 {
    effective_resistances(live, guid)[0]
}

pub(crate) fn effective_magic_resistances(live: &LiveConn, guid: u64) -> [u32; 6] {
    effective_resistances(live, guid)[1..]
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
        shard.assert_call("install_guid_range", &["0"]);
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
        shard.assert_call("debug_fill_aura_slots", &["1", "1", "false", "3"]);
        shard.assert_sql("UPDATE game_aura SET eff_kind = 161, eff_p0 = 1, amount = 100, stacks = 3 WHERE target_guid = 1");
        assert!(poll_until(POLL_TIMEOUT, || {
            let guard = coordinator.0.coord();
            aura_armor_positive(&guard, 1) == 900
        }));
    }
}
