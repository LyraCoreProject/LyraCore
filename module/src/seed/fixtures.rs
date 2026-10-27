use lyracore_package_delta::packed_spell_effect_id;
use spacetimedb::{ReducerContext, Table};

use crate::items::{ALL_PLAYABLE_CLASS_MASK, ALL_PLAYABLE_RACE_MASK};
use crate::{
    game_createinfo_spell, game_creature_family, game_creature_spawn, game_creature_template,
    game_faction, game_item_template, game_spell, game_spell_effect, game_spell_proc_event,
    game_taxi_node, game_taxi_path, game_taxi_path_node, CreateinfoSpell, CreatureFamily,
    CreatureSpawn, CreatureTemplate, Faction, GameTaxiNode, GameTaxiPath, GameTaxiPathNode,
    ItemTemplate, Spell, SpellEffect, SpellProcEvent,
};

pub(crate) const TEST_WOLF_ENTRY: u32 = 51000;
pub(crate) const PROFESSION_TRAINER_ENTRY: u32 = 51001;
pub(crate) const TEST_TAME_BEAST_SPELL: u32 = 50300;
pub(crate) const TEST_TAME_BOAR_ENTRY: u32 = 51006;
const TEST_TAME_BOAR_FAMILY: u32 = 5;
const TEST_HUNTER_CLASS: u8 = 3;
pub(crate) const RIDING_TRAINER_ENTRY: u32 = 51007;

pub(crate) fn test_wolf_template() -> CreatureTemplate {
    CreatureTemplate {
        entry: TEST_WOLF_ENTRY,
        name: "Test Wolf".to_string(),
        subname: String::new(),
        display_id: 720,
        level: 1,
        health: 60,
        faction_template: 14,
        npc_flags: 0,
        unit_flags: 0,
        creature_type: 1,
        creature_family: 1,
        type_flags: 0x100,
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 2000,
        money_min: 0,
        money_max: 0,
        max_level: 0,
        max_level_health: 0,
        aggro_range: 0,
        damage_min: 0,
        damage_max: 0,
        armor: 0,
        pickpocket_loot_id: 0,
        skin_loot_id: 0,
        trainer_type: 0,
        trainer_class: 0,
    }
}

pub(crate) fn profession_trainer_template() -> CreatureTemplate {
    CreatureTemplate {
        entry: PROFESSION_TRAINER_ENTRY,
        name: "Profession Trainer".to_string(),
        subname: "Cooking & Skinning".to_string(),
        display_id: 3167,
        level: 30,
        health: 1500,
        faction_template: 35,
        npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
            | lyracore_shared::constants::npc_flags::TRAINER,
        unit_flags: 0,
        creature_type: 7,
        creature_family: 0,
        type_flags: 0,
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 2000,
        money_min: 0,
        money_max: 0,
        max_level: 0,
        max_level_health: 0,
        aggro_range: 0,
        damage_min: 0,
        damage_max: 0,
        armor: 0,
        pickpocket_loot_id: 0,
        skin_loot_id: 0,
        trainer_type: 2,
        trainer_class: 0,
    }
}

pub(crate) fn riding_trainer_template() -> CreatureTemplate {
    CreatureTemplate {
        entry: RIDING_TRAINER_ENTRY,
        name: "Riding Trainer".to_string(),
        subname: "Riding".to_string(),
        trainer_type: lyracore_shared::trainer::trainer_type::MOUNTS,
        ..profession_trainer_template()
    }
}

fn test_tame_boar_template() -> CreatureTemplate {
    CreatureTemplate {
        entry: TEST_TAME_BOAR_ENTRY,
        name: "Test Boar".to_string(),
        display_id: 503,
        level: 5,
        health: 100,
        faction_template: 14,
        creature_type: 1,
        creature_family: TEST_TAME_BOAR_FAMILY as u8,
        base_attack_time_ms: 2000,
        scale: 1.0,
        ..test_wolf_template()
    }
}

pub(crate) fn seed_hunter_tame_fixture(ctx: &ReducerContext) {
    if !ctx
        .db
        .game_createinfo_spell()
        .iter()
        .any(|row| row.class == TEST_HUNTER_CLASS && row.spell_id == TEST_TAME_BEAST_SPELL)
    {
        ctx.db.game_createinfo_spell().id().delete(50_300u64);
        ctx.db.game_createinfo_spell().insert(CreateinfoSpell {
            id: 50_300,
            race: 0,
            class: TEST_HUNTER_CLASS,
            spell_id: TEST_TAME_BEAST_SPELL,
        });
    }
    if ctx
        .db
        .game_creature_family()
        .family_id()
        .find(TEST_TAME_BOAR_FAMILY)
        .is_none()
    {
        ctx.db.game_creature_family().insert(CreatureFamily {
            family_id: TEST_TAME_BOAR_FAMILY,
            name: "Boar".to_string(),
            pet_food_mask: 0x59,
            pet_talent_type: 0,
            category: 0,
        });
    }
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(TEST_TAME_BOAR_ENTRY)
        .is_none()
    {
        ctx.db
            .game_creature_template()
            .insert(test_tame_boar_template());
    }

    let header = Spell {
        range_yd: 30,
        duration_ms: 10_000,
        cast_flags: crate::spell::SPELL_ATTR_CHANNELED,
        ..base_spell(TEST_TAME_BEAST_SPELL, "Test Tame Beast")
    };
    if ctx
        .db
        .game_spell()
        .spell_id()
        .find(TEST_TAME_BEAST_SPELL)
        .is_some()
    {
        ctx.db.game_spell().spell_id().update(header);
    } else {
        ctx.db.game_spell().insert(header);
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::E_TAME_CREATURE,
            target: crate::spell::T_TARGET_ENEMY,
            ..base_effect(TEST_TAME_BEAST_SPELL, 0)
        },
    );
}

pub(crate) fn seed_taxi_fixture(ctx: &ReducerContext) {
    use lyracore_shared::constants::taxi_fixture as taxi;

    let points = ctx.db.game_taxi_path_node();
    for id in taxi::POINT_IDS {
        points.id().delete(id);
    }
    ctx.db.game_taxi_path().id().delete(taxi::PATH_ID);
    ctx.db
        .game_taxi_node()
        .id()
        .delete(taxi::SOURCE_NODE_STORAGE_ID);
    ctx.db
        .game_taxi_node()
        .id()
        .delete(taxi::DESTINATION_NODE_STORAGE_ID);

    ctx.db.game_taxi_node().insert(GameTaxiNode {
        id: taxi::SOURCE_NODE_STORAGE_ID,
        client_node_id: taxi::SOURCE_CLIENT_NODE_ID,
        map_id: taxi::MAP_ID,
        x: taxi::SOURCE_X,
        y: taxi::SOURCE_Y,
        z: taxi::SOURCE_Z,
        name: taxi::SOURCE_NAME.to_string(),
        mount_display_horde: taxi::MOUNT_DISPLAY_HORDE,
        mount_display_alliance: taxi::MOUNT_DISPLAY_ALLIANCE,
    });
    ctx.db.game_taxi_node().insert(GameTaxiNode {
        id: taxi::DESTINATION_NODE_STORAGE_ID,
        client_node_id: taxi::DESTINATION_CLIENT_NODE_ID,
        map_id: taxi::MAP_ID,
        x: taxi::DESTINATION_X,
        y: taxi::DESTINATION_Y,
        z: taxi::DESTINATION_Z,
        name: taxi::DESTINATION_NAME.to_string(),
        mount_display_horde: taxi::MOUNT_DISPLAY_HORDE,
        mount_display_alliance: taxi::MOUNT_DISPLAY_ALLIANCE,
    });
    ctx.db.game_taxi_path().insert(GameTaxiPath {
        id: taxi::PATH_ID,
        source_node_id: taxi::SOURCE_NODE_STORAGE_ID,
        destination_node_id: taxi::DESTINATION_NODE_STORAGE_ID,
        fare: taxi::FARE,
    });
    for (id, node_index, x, y, z) in [
        (
            taxi::POINT_IDS[0],
            0,
            taxi::SOURCE_X,
            taxi::SOURCE_Y,
            taxi::SOURCE_Z,
        ),
        (
            taxi::POINT_IDS[1],
            1,
            taxi::MIDPOINT_X,
            taxi::MIDPOINT_Y,
            taxi::MIDPOINT_Z,
        ),
        (
            taxi::POINT_IDS[2],
            2,
            taxi::DESTINATION_X,
            taxi::DESTINATION_Y,
            taxi::DESTINATION_Z,
        ),
    ] {
        points.insert(GameTaxiPathNode {
            id,
            path_id: taxi::PATH_ID,
            node_index,
            map_id: taxi::MAP_ID,
            x,
            y,
            z,
            flags: 0,
            delay_ms: 0,
        });
    }

    let template = CreatureTemplate {
        entry: taxi::FLIGHT_MASTER_ENTRY,
        name: "Test Flight Master".to_string(),
        subname: "Flight Master".to_string(),
        display_id: 3167,
        level: 30,
        health: 1500,
        faction_template: 35,
        npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
            | lyracore_shared::constants::npc_flags::TAXI,
        unit_flags: 0,
        creature_type: 7,
        creature_family: 0,
        type_flags: 0,
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 2000,
        money_min: 0,
        money_max: 0,
        max_level: 0,
        max_level_health: 0,
        aggro_range: 0,
        damage_min: 0,
        damage_max: 0,
        armor: 0,
        pickpocket_loot_id: 0,
        skin_loot_id: 0,
        trainer_type: 0,
        trainer_class: 0,
    };
    let templates = ctx.db.game_creature_template();
    templates.entry().delete(taxi::FLIGHT_MASTER_ENTRY);
    let template = templates.insert(template);

    crate::WorldEntity::delete(ctx, taxi::FLIGHT_MASTER_GUID);
    let spawns = ctx.db.game_creature_spawn();
    spawns.guid().delete(taxi::FLIGHT_MASTER_GUID);
    let spawn = spawns.insert(CreatureSpawn {
        guid: taxi::FLIGHT_MASTER_GUID,
        entry: taxi::FLIGHT_MASTER_ENTRY,
        map_id: taxi::MAP_ID,
        x: taxi::SOURCE_X,
        y: taxi::SOURCE_Y,
        z: taxi::SOURCE_Z,
        orientation: 0.0,
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: 0,
    });
    crate::creatures::insert_creature_entity(
        ctx,
        crate::creatures::build_creature_entity(ctx, &spawn, &template, 0, 0),
    );
}

pub(crate) fn tempered_blade_template(entry: u32) -> ItemTemplate {
    ItemTemplate {
        class: 2,
        subclass: 7,
        display_id: 1542,
        quality: 2,
        inventory_type: 21,
        item_level: 12,
        required_level: 1,
        max_durability: 70,
        buy_price: 1200,
        sell_price: 240,
        max_stack: 1,
        damage_min: 8.0,
        damage_max: 12.0,
        delay_ms: 2600,
        bonding: crate::items::bonding::BIND_ON_EQUIP,
        ..base_item(entry, "Tempered Blade")
    }
}

pub(crate) fn tough_jerky_template(entry: u32) -> ItemTemplate {
    ItemTemplate {
        class: 0,
        subclass: 0,
        display_id: 1542,
        quality: 0,
        inventory_type: 0,
        item_level: 1,
        required_level: 1,
        buy_price: 10,
        sell_price: 2,
        max_stack: 20,
        food_type: 1,
        spellid_1: 50115,
        spelltrigger_1: 0,
        bonding: crate::items::bonding::NONE,
        ..base_item(entry, "Tough Jerky")
    }
}

pub(crate) fn riding_reins_template(entry: u32) -> ItemTemplate {
    ItemTemplate {
        class: 15,
        subclass: 5,
        display_id: 1542,
        quality: 1,
        inventory_type: 0,
        item_level: 40,
        required_level: 1,
        buy_price: 800_000,
        sell_price: 160_000,
        max_stack: 1,
        spellid_1: FIXTURE_MOUNT_SPELL,
        spelltrigger_1: 0,
        bonding: crate::items::bonding::BIND_ON_PICKUP,
        ..base_item(entry, "Test Riding Reins")
    }
}

pub(crate) fn base_spell(spell_id: u32, name: &str) -> Spell {
    Spell {
        spell_id,
        name: name.to_string(),
        power_type: 0,
        cost: 0,
        cast_time_ms: 0,
        gcd_ms: 1500,
        family_name: 0,
        family_flags: 0,
        cooldown_ms: 0,
        range_yd: 0,
        duration_ms: 0,
        school_mask: 0,
        dispel_type: 0,
        mechanic: 0,
        max_stacks: 0,
        aura_interrupt: 0,
        attributes: 0,
        spell_level: 0,
        max_level: 0,
        is_negative: false,
        cast_flags: 0,
        stances: 0,
        proc_flags: 0,
        proc_chance: 0,
        proc_charges: 0,
        max_duration_ms: 0,
        custom_flags: 0,
        min_target_level: 0,
        single_target_key: 0,
    }
}

pub(crate) fn base_effect(spell_id: u32, effect_index: u8) -> SpellEffect {
    SpellEffect {
        id: packed_spell_effect_id(spell_id, effect_index),
        spell_id,
        effect_index,
        kind: 0,
        base_points: 0,
        die_sides: 0,
        per_level: 0.0,
        period_ms: 0,
        target: 0,
        radius_yd: 0.0,
        chain_targets: 0,
        trigger_spell: 0,
        effect_mechanic: 0,
        p0: 0,
        p0_kind: 0,
        p1: 0,
        script_id: 0,
        enters_combat: false,
        points_per_combo: 0.0,
        bonus_coefficient: -1.0,
    }
}

pub(crate) fn base_item(entry: u32, name: &str) -> ItemTemplate {
    ItemTemplate {
        entry,
        class: 0,
        subclass: 0,
        name: name.to_string(),
        display_id: 0,
        quality: 0,
        inventory_type: 0,
        item_level: 0,
        required_level: 0,
        max_durability: 0,
        buy_price: 0,
        sell_price: 0,
        max_stack: 0,
        damage_min: 0.0,
        damage_max: 0.0,
        delay_ms: 0,
        stat_strength: 0,
        stat_agility: 0,
        stat_stamina: 0,
        stat_intellect: 0,
        stat_spirit: 0,
        stat_crit: 0,
        stat_hit: 0,
        stat_armor: 0,
        block_value: 0,
        restores_power: false,
        spellid_1: 0,
        spelltrigger_1: 0,
        spellid_2: 0,
        spelltrigger_2: 0,
        container_slots: 0,
        sheath: 0,
        bonding: 0,
        holy_res: 0,
        fire_res: 0,
        nature_res: 0,
        frost_res: 0,
        shadow_res: 0,
        arcane_res: 0,
        spellid_3: 0,
        spelltrigger_3: 0,
        spellid_4: 0,
        spelltrigger_4: 0,
        spellid_5: 0,
        spelltrigger_5: 0,
        required_skill: 0,
        required_skill_rank: 0,
        required_reputation_faction: 0,
        required_reputation_rank: 0,
        max_count: 0,
        item_flags: 0,
        page_text: 0,
        start_quest: 0,
        bag_family: 0,
        buy_count: 1,
        food_type: 0,
        allowed_class: ALL_PLAYABLE_CLASS_MASK,
        allowed_race: ALL_PLAYABLE_RACE_MASK,
        random_property: 0,
    }
}

pub(crate) fn seed_pw_shield_fixture(ctx: &ReducerContext) {
    let ws_hdr = Spell {
        duration_ms: 15000,
        school_mask: 2,
        ..base_spell(6788, "Weakened Soul")
    };
    if ctx.db.game_spell().spell_id().find(6788u32).is_some() {
        ctx.db.game_spell().spell_id().update(ws_hdr);
    } else {
        ctx.db.game_spell().insert(ws_hdr);
    }
    for e in ctx.db.game_spell_effect().by_spell().filter(&6788u32) {
        ctx.db.game_spell_effect().id().delete(e.id);
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0xB1,
            base_points: 1,
            target: 2,
            p0: 19,
            p0_kind: 3,
            ..base_effect(6788, 1)
        },
    );
    if let Some(mut s) = ctx.db.game_spell().spell_id().find(50072u32) {
        s.duration_ms = 30000;
        ctx.db.game_spell().spell_id().update(s);
    } else {
        ctx.db.game_spell().insert(Spell {
            range_yd: 30,
            duration_ms: 30000,
            school_mask: 2,
            ..base_spell(50072, "Test Power Word: Shield")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0xA2,
            base_points: 50,
            target: 2,
            p0: 2,
            p0_kind: 2,
            p1: 6788,
            ..base_effect(50072, 0)
        },
    );
}

pub(crate) fn seed_soul_shard_item(ctx: &ReducerContext) {
    const SOUL_SHARD: u32 = crate::combat::SOUL_SHARD_ENTRY;
    if ctx
        .db
        .game_item_template()
        .entry()
        .find(SOUL_SHARD)
        .is_some()
    {
        return;
    }
    ctx.db.game_item_template().insert(ItemTemplate {
        class: 7,
        subclass: 0,
        display_id: 1542,
        quality: 1,
        item_level: 1,
        required_level: 1,
        sell_price: 0,
        max_stack: 20,
        bonding: crate::items::bonding::BIND_ON_PICKUP,
        ..base_item(SOUL_SHARD, "Soul Shard")
    });
}

pub(crate) fn seed_drain_soul_fixture(ctx: &ReducerContext) {
    const DRAIN_SOUL: u32 = crate::combat::DRAIN_SOUL_SPELL_ID;
    if ctx.db.game_spell().spell_id().find(DRAIN_SOUL).is_none() {
        ctx.db.game_spell().insert(Spell {
            range_yd: 30,
            duration_ms: 15000,
            school_mask: 32,
            is_negative: true,
            ..base_spell(DRAIN_SOUL, "Drain Soul")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0x90,
            base_points: 45,
            period_ms: 3000,
            target: 1,
            p0: 32,
            p0_kind: 2,
            ..base_effect(DRAIN_SOUL, 0)
        },
    );
}

pub(crate) fn seed_mana_burn_fixture(ctx: &ReducerContext) {
    const MANA_BURN: u32 = 8129;
    if ctx.db.game_spell().spell_id().find(MANA_BURN).is_none() {
        ctx.db.game_spell().insert(Spell {
            cast_time_ms: 1500,
            range_yd: 30,
            school_mask: 32,
            is_negative: true,
            ..base_spell(MANA_BURN, "Mana Burn")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0x19,
            base_points: 100,
            target: 1,
            p0_kind: 4,
            p1: 50,
            ..base_effect(MANA_BURN, 0)
        },
    );
}

pub(crate) fn seed_stealth_fixture(ctx: &ReducerContext) {
    const STEALTH: u32 = 1784;
    if ctx.db.game_spell().spell_id().find(STEALTH).is_none() {
        ctx.db.game_spell().insert(Spell {
            power_type: 3,
            school_mask: 1,
            ..base_spell(STEALTH, "Stealth")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_STEALTH,
            target: 0,
            p0_kind: 7,
            ..base_effect(STEALTH, 0)
        },
    );
}

pub(crate) fn seed_frost_armor_fixture(ctx: &ReducerContext) {
    const CHILLED: u32 = 6136;
    const FROST_ARMOR: u32 = 168;
    if ctx.db.game_spell().spell_id().find(CHILLED).is_none() {
        ctx.db.game_spell().insert(Spell {
            gcd_ms: 0,
            duration_ms: 5000,
            school_mask: 16,
            is_negative: true,
            ..base_spell(CHILLED, "Chilled")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_SPEED,
            base_points: -30,
            target: 1,
            p0_kind: 6,
            ..base_effect(CHILLED, 0)
        },
    );
    if ctx.db.game_spell().spell_id().find(FROST_ARMOR).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: u32::MAX,
            school_mask: 16,
            proc_flags: 0x28,
            proc_chance: 100,
            ..base_spell(FROST_ARMOR, "Frost Armor")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_RESISTANCE,
            base_points: 150,
            target: 0,
            p0: 1,
            p0_kind: 2,
            ..base_effect(FROST_ARMOR, 0)
        },
    );
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_PROC_TRIGGER,
            target: 0,
            trigger_spell: CHILLED,
            ..base_effect(FROST_ARMOR, 1)
        },
    );
}

pub(crate) const TEST_PROC_MARK: u32 = 50140;
pub(crate) const TEST_PROC_COIN: u32 = 50141;
pub(crate) const TEST_PROC_CHARGES: u32 = 50142;
pub(crate) const TEST_PROC_COOLDOWN: u32 = 50143;
pub(crate) const TEST_PROC_PPM: u32 = 50144;
pub(crate) const TEST_PROC_ZAP: u32 = 50145;

const TEST_PROC_ICD_MS: u32 = 5_000;
const TEST_PROC_PPM_RATE: f32 = 2.0;
const TEST_PROC_ZAP_DAMAGE: i32 = 5;
const TEST_PROC_ZAP_SCHOOL_MASK: i32 = 0x10;

pub(crate) fn seed_test_proc_fixtures(ctx: &ReducerContext) {
    if ctx
        .db
        .game_spell()
        .spell_id()
        .find(TEST_PROC_MARK)
        .is_none()
    {
        ctx.db.game_spell().insert(Spell {
            duration_ms: 1000,
            is_negative: true,
            ..base_spell(TEST_PROC_MARK, "Test Proc Mark")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_FLAG,
            target: 1,
            p0_kind: 7,
            ..base_effect(TEST_PROC_MARK, 0)
        },
    );
    let taken = crate::spell::proc::PROC_FLAG_TAKEN_MELEE_HIT;
    let dealt = crate::spell::proc::PROC_FLAG_SUCCESSFUL_MELEE_HIT;
    for (spell_id, name, flags, chance, charges, overlay) in [
        (TEST_PROC_COIN, "Test Proc Coin", taken, 50u8, 0u8, None),
        (TEST_PROC_CHARGES, "Test Proc Charges", taken, 100, 3, None),
        (
            TEST_PROC_COOLDOWN,
            "Test Proc Cooldown",
            taken,
            100,
            0,
            Some((TEST_PROC_ICD_MS, 0.0)),
        ),
        (
            TEST_PROC_PPM,
            "Test Proc PPM",
            dealt,
            0,
            0,
            Some((0, TEST_PROC_PPM_RATE)),
        ),
    ] {
        if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
            ctx.db.game_spell().insert(Spell {
                duration_ms: u32::MAX,
                proc_flags: flags,
                proc_chance: chance,
                proc_charges: charges,
                ..base_spell(spell_id, name)
            });
        }
        upsert_effect(
            ctx,
            SpellEffect {
                kind: crate::spell::A_PROC_TRIGGER,
                target: 0,
                trigger_spell: TEST_PROC_MARK,
                ..base_effect(spell_id, 0)
            },
        );
        if let Some((icd_ms, ppm_rate)) = overlay {
            if ctx
                .db
                .game_spell_proc_event()
                .spell_id()
                .find(spell_id)
                .is_none()
            {
                ctx.db.game_spell_proc_event().insert(SpellProcEvent {
                    spell_id,
                    proc_flags: 0,
                    proc_ex: 0,
                    school_mask: 0,
                    family_name: 0,
                    family_flags: 0,
                    ppm_rate,
                    custom_chance: 0,
                    icd_ms,
                });
            }
        }
    }
    if ctx.db.game_spell().spell_id().find(TEST_PROC_ZAP).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: u32::MAX,
            school_mask: TEST_PROC_ZAP_SCHOOL_MASK as u8,
            proc_flags: crate::spell::proc::PROC_FLAG_TAKEN_MELEE_HIT,
            proc_chance: 100,
            ..base_spell(TEST_PROC_ZAP, "Test Proc Zap")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_PROC_DAMAGE,
            base_points: TEST_PROC_ZAP_DAMAGE,
            target: 0,
            p0: TEST_PROC_ZAP_SCHOOL_MASK,
            p0_kind: 2,
            ..base_effect(TEST_PROC_ZAP, 0)
        },
    );
}

pub(crate) fn seed_demon_skin_fixture(ctx: &ReducerContext) {
    const DEMON_SKIN: u32 = 696;
    if ctx.db.game_spell().spell_id().find(DEMON_SKIN).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: 1_800_000,
            school_mask: 1,
            ..base_spell(DEMON_SKIN, "Demon Skin")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_RESISTANCE,
            base_points: 120,
            target: 0,
            p0: 1,
            p0_kind: 2,
            ..base_effect(DEMON_SKIN, 0)
        },
    );
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_PERIODIC_HEAL,
            base_points: 5,
            period_ms: 5000,
            target: 0,
            ..base_effect(DEMON_SKIN, 1)
        },
    );
}

pub(crate) fn seed_regen_fixture(ctx: &ReducerContext) {
    if ctx.db.game_spell().spell_id().find(50137u32).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: 300_000,
            school_mask: 8,
            ..base_spell(50137, "Test Regeneration")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0xA9,
            base_points: 5,
            target: 0,
            ..base_effect(50137, 0)
        },
    );
}

pub(crate) const FIXTURE_BLADE: u32 = 5090050;
pub(crate) const FIXTURE_JERKY: u32 = 5090052;
pub(crate) const FIXTURE_REINS: u32 = 5090054;

fn seed_fixture_items(ctx: &ReducerContext) {
    let items = ctx.db.game_item_template();
    if items.entry().find(FIXTURE_BLADE).is_none() {
        items.insert(tempered_blade_template(FIXTURE_BLADE));
    }
    if items.entry().find(FIXTURE_JERKY).is_none() {
        items.insert(tough_jerky_template(FIXTURE_JERKY));
    }
    if items.entry().find(FIXTURE_REINS).is_none() {
        items.insert(riding_reins_template(FIXTURE_REINS));
    }
}

pub(crate) const FIXTURE_MOUNT_SPELL: u32 = 50310;
pub(crate) const FIXTURE_DAZED_SPELL: u32 = 50311;
const FIXTURE_MOUNT_DISPLAY: i32 = 1147;
const FIXTURE_MOUNT_ABILITY_ID: u64 = 5_090_020;

pub(crate) fn seed_mount_fixture(ctx: &ReducerContext) {
    use crate::{game_skill_ability, game_skill_line, SkillAbility, SkillLine};

    let lines = ctx.db.game_skill_line();
    if lines
        .skill_line()
        .find(crate::skill::skill_line::RIDING)
        .is_none()
    {
        lines.insert(SkillLine {
            skill_line: crate::skill::skill_line::RIDING,
            category: 0,
            name: "Riding".to_string(),
        });
    }

    let mount = Spell {
        gcd_ms: 0,
        duration_ms: u32::MAX,
        ..base_spell(FIXTURE_MOUNT_SPELL, "Test Riding Horse")
    };
    if ctx
        .db
        .game_spell()
        .spell_id()
        .find(FIXTURE_MOUNT_SPELL)
        .is_some()
    {
        ctx.db.game_spell().spell_id().update(mount);
    } else {
        ctx.db.game_spell().insert(mount);
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOUNTED,
            p0: FIXTURE_MOUNT_DISPLAY,
            p0_kind: crate::spell::P_DISPLAY_ID,
            ..base_effect(FIXTURE_MOUNT_SPELL, 0)
        },
    );
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_SPEED,
            base_points: 60,
            p0: crate::spell::SPEED_MOUNTED as i32,
            p0_kind: crate::spell::P_SPEED_KIND,
            ..base_effect(FIXTURE_MOUNT_SPELL, 1)
        },
    );

    ctx.db
        .game_skill_ability()
        .id()
        .delete(FIXTURE_MOUNT_ABILITY_ID);
    ctx.db.game_skill_ability().insert(SkillAbility {
        id: FIXTURE_MOUNT_ABILITY_ID,
        spell_id: FIXTURE_MOUNT_SPELL,
        skill_line: crate::skill::skill_line::RIDING,
        race_mask: 0,
        class_mask: 0,
        min_skill: 75,
        acquire_method: 0,
        gray: 0,
        green: 0,
    });

    let dazed = Spell {
        range_yd: 30,
        duration_ms: 4_000,
        is_negative: true,
        ..base_spell(FIXTURE_DAZED_SPELL, "Test Dazed")
    };
    if ctx
        .db
        .game_spell()
        .spell_id()
        .find(FIXTURE_DAZED_SPELL)
        .is_some()
    {
        ctx.db.game_spell().spell_id().update(dazed);
    } else {
        ctx.db.game_spell().insert(dazed);
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_SPEED,
            base_points: -50,
            target: crate::spell::T_TARGET_ENEMY,
            p0: crate::spell::SPEED_MOVE as i32,
            p0_kind: crate::spell::P_SPEED_KIND,
            ..base_effect(FIXTURE_DAZED_SPELL, 0)
        },
    );
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::E_DISMOUNT,
            target: crate::spell::T_TARGET_ENEMY,
            ..base_effect(FIXTURE_DAZED_SPELL, 1)
        },
    );
}

pub(crate) const FIXTURE_FACTION: u32 = 50900;

pub(crate) fn seed_fixture_catalogue(ctx: &ReducerContext) {
    seed_fixture_items(ctx);
    if ctx
        .db
        .game_faction()
        .faction_id()
        .find(FIXTURE_FACTION)
        .is_none()
    {
        ctx.db.game_faction().insert(Faction {
            faction_id: FIXTURE_FACTION,
            // The client has 64 reputation slots; the real import uses 0..=54.
            reputation_index: 60,
            base_standing: 0,
        });
    }
}

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
#[allow(clippy::too_many_lines)]
pub(crate) fn seed_scenario_fixtures(ctx: &ReducerContext) {
    use crate::quest::quest_role;

    use crate::{
        game_creature_quest, game_npc_vendor, game_quest_objective, game_quest_reward_item,
        game_quest_template, game_quest_text, game_trainer_spell,
    };

    seed_fixture_catalogue(ctx);
    seed_hunter_tame_fixture(ctx);

    const QUEST: u32 = 50900;
    const QUESTGIVER: u32 = 51003;
    const VENDOR: u32 = 51004;
    const WOLF: u32 = 51000;
    if ctx.db.game_quest_template().entry().find(QUEST).is_none() {
        ctx.db.game_quest_template().insert(crate::QuestTemplate {
            entry: QUEST,
            min_level: 0,
            quest_level: 2,
            title: "Wolf Cull".to_string(),
            reward_money: 150,
            reward_xp: 90,
            prev_quest_id: 0,
            required_races: 0,
            required_classes: 0,
            zone_or_sort: 12,
            rew_rep_faction_1: FIXTURE_FACTION,
            rew_rep_value_1: 250,
            rew_rep_faction_2: 0,
            rew_rep_value_2: 0,
            src_item: 0,
            src_item_count: 0,
            repeatable: true,
            next_quest_id: 0,
            limit_time: 0,
            reward_money_max_level: 0,
            quest_type: 0,
        });
        ctx.db.game_quest_text().insert(crate::QuestText {
            quest_entry: QUEST,
            details: "The test wolves multiply. Cull two of them.".to_string(),
            objectives: "Kill 2 Test Wolves.".to_string(),
            offer_reward_text: "The pack thins. Well done.".to_string(),
            request_items_text: "Are the wolves culled?".to_string(),
        });
        // Explicit reserved ids avoid collisions with the import, which does not advance auto_inc.
        ctx.db.game_quest_objective().id().delete(5090000u64);
        ctx.db.game_quest_objective().insert(crate::QuestObjective {
            id: 5090000,
            quest_entry: QUEST,
            obj_index: 0,
            kind: crate::quest::objective_kind::KILL_CREATURE,
            target_entry: WOLF,
            required_count: 2,
        });
        ctx.db.game_quest_reward_item().id().delete(5090001u64);
        ctx.db
            .game_quest_reward_item()
            .insert(crate::QuestRewardItem {
                id: 5090001,
                quest_entry: QUEST,
                item_entry: FIXTURE_JERKY,
                count: 2,
            });
        ctx.db.game_creature_quest().id().delete(5090002u64);
        ctx.db.game_creature_quest().insert(crate::CreatureQuest {
            id: 5090002,
            creature_entry: QUESTGIVER,
            quest_entry: QUEST,
            role: quest_role::START,
        });
        ctx.db.game_creature_quest().id().delete(5090003u64);
        ctx.db.game_creature_quest().insert(crate::CreatureQuest {
            id: 5090003,
            creature_entry: QUESTGIVER,
            quest_entry: QUEST,
            role: quest_role::END,
        });
    }

    let templates = ctx.db.game_creature_template();
    if templates.entry().find(WOLF).is_none() {
        templates.insert(test_wolf_template());
    }
    if let Some(mut wolf) = templates.entry().find(WOLF) {
        if wolf.money_max == 0 {
            wolf.money_min = 25;
            wolf.money_max = 50;
            templates.entry().update(wolf);
        }
    }
    if templates.entry().find(PROFESSION_TRAINER_ENTRY).is_none() {
        templates.insert(profession_trainer_template());
    }
    const WOLF_ELDER: u32 = 51002;
    if templates.entry().find(WOLF_ELDER).is_none() {
        templates.insert(CreatureTemplate {
            entry: WOLF_ELDER,
            name: "Test Wolf Elder".to_string(),
            subname: String::new(),
            display_id: 720,
            level: 8,
            health: 300,
            faction_template: 14,
            npc_flags: 0,
            unit_flags: 0,
            creature_type: 1,
            creature_family: 1,
            type_flags: 0x100,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 25,
            money_max: 50,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 20,
            damage_min: 2,
            damage_max: 4,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
            trainer_type: 0,
            trainer_class: 0,
        });
    }
    if templates.entry().find(QUESTGIVER).is_none() {
        templates.insert(CreatureTemplate {
            entry: QUESTGIVER,
            name: "Scenario Questgiver".to_string(),
            subname: "Wolf Cull".to_string(),
            display_id: 3167,
            level: 10,
            health: 500,
            faction_template: 35,
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP | 0x2,
            unit_flags: 0,
            creature_type: 7,
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0,
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
            trainer_type: 0,
            trainer_class: 0,
        });
    }
    if templates.entry().find(VENDOR).is_none() {
        templates.insert(CreatureTemplate {
            entry: VENDOR,
            name: "Scenario Vendor".to_string(),
            subname: "Blades & Repairs".to_string(),
            display_id: 3167,
            level: 10,
            health: 500,
            faction_template: 35,
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
                | lyracore_shared::constants::npc_flags::VENDOR
                | lyracore_shared::constants::npc_flags::REPAIR,
            unit_flags: 0,
            creature_type: 7,
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0,
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
            trainer_type: 0,
            trainer_class: 0,
        });
    }
    let vendor_rows = ctx.db.game_npc_vendor();
    if !vendor_rows
        .by_vendor()
        .filter(&VENDOR)
        .any(|r| r.item_entry == FIXTURE_BLADE)
    {
        vendor_rows.id().delete(5090010u64);
        vendor_rows.insert(crate::NpcVendor {
            id: 5090010,
            creature_entry: VENDOR,
            item_entry: FIXTURE_BLADE,
            slot: 0,
            max_count: 0,
        });
    }
    if !vendor_rows
        .by_vendor()
        .filter(&VENDOR)
        .any(|r| r.item_entry == FIXTURE_JERKY)
    {
        vendor_rows.id().delete(5090011u64);
        vendor_rows.insert(crate::NpcVendor {
            id: 5090011,
            creature_entry: VENDOR,
            item_entry: FIXTURE_JERKY,
            slot: 1,
            max_count: 0,
        });
    }

    const TRAINER: u32 = 51001;
    const LESSER_HEAL: u32 = 2050;
    let offerings = ctx.db.game_trainer_spell();
    if !offerings
        .by_trainer()
        .filter(&TRAINER)
        .any(|r| r.spell_id == LESSER_HEAL)
    {
        offerings.id().delete(5090012u64);
        offerings.insert(crate::TrainerSpell {
            id: 5090012,
            trainer_entry: TRAINER,
            spell_id: LESSER_HEAL,
            cost: 100,
            required_level: 1,
            learn_skill_line: 0,
            learn_skill_cap: 75,
        });
    }

    const WEAPON_MASTER: u32 = 51005;
    const LEARN_AXE_1H: u32 = 50130;
    const LEARN_POLEARM: u32 = 50131;
    if templates.entry().find(WEAPON_MASTER).is_none() {
        templates.insert(CreatureTemplate {
            entry: WEAPON_MASTER,
            name: "Woo Ping".to_string(),
            subname: "Weapon Master".to_string(),
            display_id: 3167,
            level: 30,
            health: 1500,
            faction_template: 35,
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
                | lyracore_shared::constants::npc_flags::TRAINER,
            unit_flags: 0,
            creature_type: 7,
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0,
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
            trainer_type: 2,
            trainer_class: 0,
        });
    }
    if !offerings
        .by_trainer()
        .filter(&WEAPON_MASTER)
        .any(|r| r.spell_id == LEARN_AXE_1H)
    {
        offerings.id().delete(5090013u64);
        offerings.insert(crate::TrainerSpell {
            id: 5090013,
            trainer_entry: WEAPON_MASTER,
            spell_id: LEARN_AXE_1H,
            cost: 100,
            required_level: 1,
            learn_skill_line: crate::skill::skill_line::AXE_1H,
            learn_skill_cap: 0,
        });
    }
    if !offerings
        .by_trainer()
        .filter(&WEAPON_MASTER)
        .any(|r| r.spell_id == LEARN_POLEARM)
    {
        offerings.id().delete(5090014u64);
        offerings.insert(crate::TrainerSpell {
            id: 5090014,
            trainer_entry: WEAPON_MASTER,
            spell_id: LEARN_POLEARM,
            cost: 100,
            required_level: 60,
            learn_skill_line: crate::skill::skill_line::POLEARM,
            learn_skill_cap: 0,
        });
    }

    seed_mount_fixture(ctx);
    if templates.entry().find(RIDING_TRAINER_ENTRY).is_none() {
        templates.insert(riding_trainer_template());
    }
    for (row_id, marker, cost, required_level) in [
        (
            5090015u64,
            crate::skill::LEARN_APPRENTICE_RIDING_SPELL_ID,
            100u32,
            1u8,
        ),
        (
            5090016u64,
            crate::skill::LEARN_JOURNEYMAN_RIDING_SPELL_ID,
            1000,
            60,
        ),
    ] {
        let Some(tier) = crate::skill::riding_marker_rank(marker) else {
            continue;
        };
        if offerings
            .by_trainer()
            .filter(&RIDING_TRAINER_ENTRY)
            .any(|r| r.spell_id == marker)
        {
            continue;
        }
        offerings.id().delete(row_id);
        offerings.insert(crate::TrainerSpell {
            id: row_id,
            trainer_entry: RIDING_TRAINER_ENTRY,
            spell_id: marker,
            cost,
            required_level,
            learn_skill_line: crate::skill::skill_line::RIDING,
            learn_skill_cap: tier as u32,
        });
    }
}

fn upsert_effect(ctx: &spacetimedb::ReducerContext, row: SpellEffect) {
    ctx.db.game_spell_effect().id().delete(row.id);
    ctx.db.game_spell_effect().insert(row);
}

pub(crate) fn seed_stacking_probe_fixture(ctx: &ReducerContext) {
    const PROBE_SPELLS: &[(u32, &str, u8, i32, u8, i32)] = &[
        (1243, "Power Word: Fortitude", 0xA0, 2, 1, 3),
        (21562, "Prayer of Fortitude", 0xA0, 2, 1, 26),
        (19740, "Blessing of Might", 0xA3, 0, 5, 20),
        (19742, "Blessing of Wisdom", 0xA0, 4, 1, 10),
    ];
    for &(spell_id, name, kind, p0, p0_kind, magnitude) in PROBE_SPELLS {
        if ctx.db.game_spell().spell_id().find(spell_id).is_some() {
            continue;
        }
        ctx.db.game_spell().insert(Spell {
            range_yd: 30,
            duration_ms: 1_800_000,
            max_stacks: 1,
            ..base_spell(spell_id, name)
        });
        upsert_effect(
            ctx,
            SpellEffect {
                kind,
                base_points: magnitude,
                target: 2,
                p0,
                p0_kind,
                ..base_effect(spell_id, 0)
            },
        );
    }
}
