use crate::creatures::build_creature_entity;
use lyracore_package_delta::fixture_ids::{
    BATTERED_BUCKLER, BATTERED_CHEST, COMBAT_INSIGHT_SPELL, DEMO_COPPER_VEIN, DEMO_PEACEBLOOM,
    DEMO_TEMPERED_BLADE, DEMO_TOUGH_JERKY, EATING_SPELL, ENCHANT_MINOR_STAMINA_SPELL,
    ENCHANT_MINOR_STRENGTH_SPELL, HEAVY_LINEN_BANDAGE_SPELL, LESSER_HEALING_SPELL,
    LINEN_BANDAGE_SPELL, MINOR_HEALING_SPELL, MINOR_MANA_POTION_SPELL, PROFESSION_TRAINER_ENTRY,
    QUICKENING_SPELL, RECRUITS_TUNIC, REFRESHING_WATER_SPELL, ROUGH_COPPER_BOMB_SPELL,
    SUSPICIOUS_LEVER, TEST_CONJURE_SPELL, TEST_FEAR_SPELL, TEST_FIRE_WARD_SPELL, TEST_POLY_SPELL,
    TEST_ROOT_SPELL, TEST_SNARE_SPELL, TEST_STUN_IMMUNITY_SPELL, TEST_STUN_SPELL, TEST_WOLF_ENTRY,
    WELL_FED_SPELL,
};
use lyracore_package_delta::{
    packed_creature_spawn_guid, packed_gameobject_spawn_guid, packed_race_class_id,
    packed_spell_effect_id,
};
use lyracore_package_delta::{DBC_FAMILY, GLOBALS_FAMILY, ITEM_FAMILY, SPELL_FAMILY};
use lyracore_shared::constants;
use spacetimedb::{reducer, Identity, ReducerContext, Table, TimeDuration};

use crate::{
    game_account, game_aura_schedule, game_breath_schedule, game_character, game_creature_loot,
    game_creature_spawn, game_creature_template, game_creature_waypoint, game_duel_schedule,
    game_event_reaper_schedule, game_gameobject_pool, game_gameobject_pool_member,
    game_gameobject_template, game_gateway_lease_reaper_schedule, game_graveyard,
    game_graveyard_zone, game_ground_area_schedule, game_instance_reaper_schedule,
    game_item_template, game_melee_schedule, game_motion_publish_schedule, game_pet_care_schedule,
    game_realm, game_spell, game_spell_effect, game_start_position, Account, AuraSchedule,
    BreathSchedule, Character, CreatureLoot, CreatureSpawn, CreatureTemplate, CreatureWaypoint,
    EventReaperSchedule, GameObject, GameObjectPool, GameObjectPoolMember, GameObjectTemplate,
    GraveyardLoc, GraveyardZone, GroundAreaSchedule, ItemTemplate, MeleeSchedule, PetCareSchedule,
    Realm, Spell, SpellEffect, StartPosition, EVENT_TTL_MICROS,
};
use crate::{game_alpha_test_tools_enrollment, AlphaTestToolsEnrollment};

#[reducer(init)]
pub fn init(ctx: &ReducerContext) {
    seed_production_core(ctx);
    seed_map0_demo_content(ctx);
    seed_spell_registry(ctx);
    grant_tester_kit(ctx);
    crate::items::seed_compatibility_enchantments(ctx);
    arm_interval_schedules(ctx);
}

/// Runs after `seed_curated_content`, which writes the starting spell kit the Tester learns.
fn grant_tester_kit(ctx: &ReducerContext) {
    use constants::start_human_warrior as hw;
    crate::spell::spellbook::grant_createinfo_spells(ctx, 1, Identity::ZERO, hw::RACE, hw::CLASS);
    crate::action_bar::grant_createinfo_actions(ctx, 1, Identity::ZERO, hw::RACE, hw::CLASS);
}

struct CuratedSeed {
    /// The importer family whose import replaces these rows; `None` seeds on every Shard.
    imported_by: Option<&'static str>,
    write: fn(&ReducerContext),
}

const CURATED_SEEDS: &[CuratedSeed] = &[
    real(SPELL_FAMILY, seed_weakened_soul),
    always(seed_pw_shield_fixture),
    always(seed_fixture_catalogue),
    real(GLOBALS_FAMILY, seed_hunter_tame_kit),
    real(DBC_FAMILY, seed_boar_family),
    always(seed_hunter_tame_fixture),
    real(SPELL_FAMILY, seed_stacking_probe_fixture),
    real(ITEM_FAMILY, seed_soul_shard_item),
    real(SPELL_FAMILY, seed_drain_soul_fixture),
    real(SPELL_FAMILY, seed_frost_armor_fixture),
    always(seed_test_proc_fixtures),
    real(SPELL_FAMILY, seed_mana_burn_fixture),
    real(SPELL_FAMILY, seed_demon_skin_fixture),
    always(seed_regen_fixture),
    real(SPELL_FAMILY, seed_stealth_fixture),
    real(DBC_FAMILY, seed_riding_skill_line),
    always(seed_mount_fixture),
    real(GLOBALS_FAMILY, seed_createinfo_spells),
    // The talent import carries no family stamp; a spell import brings the real talent spells.
    real(SPELL_FAMILY, crate::talent::seed_talents),
    // The importer does not import spell groups yet, so imported Realms need these rows too.
    always(seed_spell_groups),
    real(GLOBALS_FAMILY, seed_crit_rates),
];

const fn real(family: &'static str, write: fn(&ReducerContext)) -> CuratedSeed {
    CuratedSeed {
        imported_by: Some(family),
        write,
    }
}

const fn always(write: fn(&ReducerContext)) -> CuratedSeed {
    CuratedSeed {
        imported_by: None,
        write,
    }
}

/// Curated rows and fixtures; see docs/danger-zones.md "After every publish" for the import rule.
pub(crate) fn seed_curated_content(ctx: &ReducerContext) {
    for seed in CURATED_SEEDS {
        if seed
            .imported_by
            .is_some_and(|family| crate::import_meta::is_imported(ctx, family))
        {
            continue;
        }
        (seed.write)(ctx);
    }
}

fn seed_crit_rates(ctx: &ReducerContext) {
    use crate::stats::{game_crit_per_agility, CritPerAgility};
    for (level, rate) in [(1, 4.0), (60, 20.0)] {
        let key = crate::stats::class_level_key(1, level);
        if ctx
            .db
            .game_crit_per_agility()
            .class_level()
            .find(key)
            .is_some()
        {
            continue;
        }
        ctx.db.game_crit_per_agility().insert(CritPerAgility {
            class_level: key,
            class: 1,
            level,
            rate,
        });
    }
}

fn seed_production_core(ctx: &ReducerContext) {
    use constants::start_human_warrior as hw;

    ctx.db.game_realm().insert(Realm {
        id: 1,
        name: "LyraCore".to_string(),
        address: "127.0.0.1:8085".to_string(),
        realm_type: 0,
        flags: 0,
        population: 0.0,
        timezone: 1,
    });

    crate::config::save_server_config(ctx, crate::config::server_config(ctx));

    ctx.db
        .game_alpha_test_tools_enrollment()
        .insert(AlphaTestToolsEnrollment {
            id: 0,
            enabled: true,
        });

    ctx.db.game_start_position().insert(StartPosition {
        race_class: packed_race_class_id(hw::RACE, hw::CLASS) as u16,
        race: hw::RACE,
        class: hw::CLASS,
        map_id: hw::MAP_ID,
        zone_id: hw::ZONE_ID,
        x: hw::X,
        y: hw::Y,
        z: hw::Z,
        orientation: hw::ORIENTATION,
        display_id: 49,
    });

    const ALLIANCE_FACTION: u32 = 469;
    for (id, name, x, y, z, o, zone_id) in [
        (
            105u32,
            "Northshire Abbey",
            -8935.33f32,
            -188.646f32,
            80.4165f32,
            2.72271f32,
            12u32,
        ),
        (106, "Goldshire", -9339.59, 171.73, 63.5258, 0.0, 12),
        (
            854,
            "Eastvale Logging Camp",
            -9552.73,
            -1374.84,
            57.0867,
            0.0,
            12,
        ),
        (80, "Sentinel Hill", -10650.0, 1180.0, 34.0, 0.0, 40),
        (81, "Westfall Coast", -11390.0, 1590.0, 6.0, 0.0, 40),
    ] {
        ctx.db.game_graveyard().insert(GraveyardLoc {
            id,
            map_id: hw::MAP_ID,
            x,
            y,
            z,
            o,
            name: name.to_string(),
        });
        ctx.db.game_graveyard_zone().insert(GraveyardZone {
            row_id: 0,
            safe_loc_id: id,
            zone_id,
            faction: ALLIANCE_FACTION,
        });
    }

    let account = ctx.db.game_account().insert(Account {
        id: 0,
        username: "TEST".to_string(),
        salt: vec![0u8; 32],
        verifier: vec![0u8; 32],
        identity: None,
        banned: false,
        alpha_test_tools: true,
        gm_level: 3,
    });

    ctx.db.game_character().insert(Character {
        guid: 1,
        account_id: account.id,
        owner_identity: Identity::ZERO,
        name: "Tester".to_string(),
        race: hw::RACE,
        class: hw::CLASS,
        gender: 0,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        level: 1,
        xp: 0,
        next_level_xp: crate::xp::xp_to_next_level(1),
        map_id: hw::MAP_ID,
        zone_id: hw::ZONE_ID,
        x: hw::X,
        y: hw::Y,
        z: hw::Z,
        orientation: hw::ORIENTATION,
        first_login: true,
        online: false,
        money: 0,
        rested_xp: 0,
        last_logout_micros: 0,
        home_map: hw::MAP_ID,
        home_zone: hw::ZONE_ID,
        home_x: hw::X,
        home_y: hw::Y,
        home_z: hw::Z,
        played_total_secs: 0,
        session_start_micros: 0,
        health: 0,
        power: 0,
        respec_count: 0,
        death_expire_micros: 0,
        pending_instance_id: 0,
        gm_level: 0,
        pending_ghost: false,
        resting: false,
        rested_since_micros: 0,
        pending_godmode: false,
        pending_run_speed_mult_bp: crate::character::RUN_SPEED_BP_1X,
        bank_bag_slots: 0,
        watched_faction_index: -1,
        rest_state: lyracore_shared::packing::REST_STATE_NORMAL,
    });
    crate::creatures::seed_on_aggro_fixtures(ctx);
}

#[allow(clippy::too_many_lines)]
fn seed_map0_demo_content(ctx: &ReducerContext) {
    use constants::start_human_warrior as hw;

    seed_taxi_fixture(ctx);

    const CHICKEN_ENTRY: u32 = 620;
    let chicken_tmpl = ctx.db.game_creature_template().insert(CreatureTemplate {
        display_id: 304,
        level: 1,
        health: 42,
        faction_template: 14,
        creature_type: 8,
        scale: 1.0,
        base_attack_time_ms: 1500,
        money_min: 5,
        money_max: 20,
        aggro_range: 8,
        ..CreatureTemplate::blank(CHICKEN_ENTRY, "Chicken")
    });

    let chicken_guid: u64 = packed_creature_spawn_guid(CHICKEN_ENTRY, 1);

    let chicken_spawn = ctx.db.game_creature_spawn().insert(CreatureSpawn {
        guid: chicken_guid,
        entry: CHICKEN_ENTRY,
        map_id: hw::MAP_ID,
        x: hw::X + 15.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        // Disarm timers for the already-live fixture so due-time scans do not revisit it.
        respawn_at: crate::time::timer_never(ctx),
        despawn_at: crate::time::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_RANDOM,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: 0,
    });
    crate::creatures::insert_creature_entity(
        ctx,
        build_creature_entity(ctx, &chicken_spawn, &chicken_tmpl, 0, 0),
    );

    for (wx, wy) in [(hw::X + 15.0, hw::Y), (hw::X + 15.0, hw::Y + 8.0)] {
        ctx.db.game_creature_waypoint().insert(CreatureWaypoint {
            id: 0,
            creature_guid: chicken_guid,
            x: wx,
            y: wy,
            z: hw::Z,
        });
    }

    {
        use constants::starter_item as si;
        ctx.db.game_item_template().insert(ItemTemplate {
            class: si::CLASS_WEAPON,
            subclass: si::SUBCLASS_SWORD_1H,
            display_id: si::DISPLAY_ID,
            quality: si::QUALITY_POOR,
            inventory_type: si::INVTYPE_WEAPON_MAINHAND,
            item_level: si::ITEM_LEVEL,
            required_level: si::REQUIRED_LEVEL,
            max_durability: si::MAX_DURABILITY,
            buy_price: si::BUY_PRICE,
            sell_price: si::SELL_PRICE,
            max_stack: 1,
            damage_min: si::DAMAGE_MIN,
            damage_max: si::DAMAGE_MAX,
            delay_ms: si::DELAY_MS,
            bonding: crate::items::bonding::BIND_ON_PICKUP,
            ..ItemTemplate::blank(si::ENTRY, "Worn Shortsword")
        });
    }

    ctx.db
        .game_item_template()
        .insert(tempered_blade_template(DEMO_TEMPERED_BLADE));

    ctx.db.game_item_template().insert(ItemTemplate {
        class: 4,
        subclass: 1,
        display_id: 1542,
        quality: 1,
        inventory_type: 5,
        item_level: 5,
        required_level: 1,
        max_durability: 40,
        buy_price: 200,
        sell_price: 40,
        max_stack: 1,
        bonding: crate::items::bonding::NONE,
        ..ItemTemplate::blank(RECRUITS_TUNIC, "Recruit's Tunic")
    });
    ctx.db
        .game_item_template()
        .insert(tough_jerky_template(DEMO_TOUGH_JERKY));

    let wolf_tmpl = ctx.db.game_creature_template().insert(test_wolf_template());
    let wolf_guid: u64 = packed_creature_spawn_guid(TEST_WOLF_ENTRY, 1);
    let wolf_spawn = ctx.db.game_creature_spawn().insert(CreatureSpawn {
        guid: wolf_guid,
        entry: TEST_WOLF_ENTRY,
        map_id: hw::MAP_ID,
        x: hw::X + 10.0,
        y: hw::Y + 5.0,
        z: hw::Z,
        orientation: 0.0,
        respawn_at: crate::time::timer_never(ctx),
        despawn_at: crate::time::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_RANDOM,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: 0,
    });
    crate::creatures::insert_creature_entity(
        ctx,
        build_creature_entity(ctx, &wolf_spawn, &wolf_tmpl, 0, 0),
    );

    let trainer_tmpl = ctx
        .db
        .game_creature_template()
        .insert(profession_trainer_template());
    let trainer_guid: u64 = packed_creature_spawn_guid(PROFESSION_TRAINER_ENTRY, 1);
    let trainer_spawn = ctx.db.game_creature_spawn().insert(CreatureSpawn {
        guid: trainer_guid,
        entry: PROFESSION_TRAINER_ENTRY,
        map_id: hw::MAP_ID,
        x: hw::X - 5.0,
        y: hw::Y + 5.0,
        z: hw::Z,
        orientation: 0.0,
        respawn_at: crate::time::timer_never(ctx),
        despawn_at: crate::time::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: 0,
    });
    crate::creatures::insert_creature_entity(
        ctx,
        build_creature_entity(ctx, &trainer_spawn, &trainer_tmpl, 0, 0),
    );

    ctx.db.game_item_template().insert(ItemTemplate {
        class: 15,
        display_id: 6418,
        quality: 1,
        item_level: 1,
        required_level: 1,
        max_stack: 1,
        spellid_1: 8690,
        spelltrigger_1: 0,
        bonding: crate::items::bonding::BIND_ON_PICKUP,
        ..ItemTemplate::blank(constants::starter_item::HEARTHSTONE_ENTRY, "Hearthstone")
    });

    ctx.db.game_item_template().insert(ItemTemplate {
        class: 4,
        subclass: 6,
        display_id: 1542,
        quality: 1,
        inventory_type: 14,
        item_level: 5,
        required_level: 1,
        max_durability: 60,
        buy_price: 300,
        sell_price: 60,
        max_stack: 1,
        block_value: 25,
        bonding: crate::items::bonding::NONE,
        ..ItemTemplate::blank(BATTERED_BUCKLER, "Battered Buckler")
    });

    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: CHICKEN_ENTRY,
        item_entry: DEMO_TOUGH_JERKY,
        chance_bp: 10000,
        count: 1,
        group_id: 0,
        quest_only: false,
    });
    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: CHICKEN_ENTRY,
        item_entry: RECRUITS_TUNIC,
        chance_bp: 5000,
        count: 1,
        group_id: 0,
        quest_only: false,
    });
    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: CHICKEN_ENTRY,
        item_entry: 769,
        chance_bp: 10000,
        count: 1,
        group_id: 0,
        quest_only: false,
    });

    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: BATTERED_CHEST,
            type_id: lyracore_shared::constants::go_type::CHEST,
            display_id: 259,
            name: "Battered Chest".to_string(),
            data0: DEMO_TOUGH_JERKY,
            data1: 0,
            gather_skill_line: 0,
            respawn_secs: 0,
            gather_gray: 0,
            lock_id: 0,
            size: 0.0,
        });
    GameObject {
        guid: packed_gameobject_spawn_guid(1),
        template_entry: BATTERED_CHEST,
        map_id: hw::MAP_ID,
        x: hw::X + 5.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);
    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: SUSPICIOUS_LEVER,
            type_id: lyracore_shared::constants::go_type::GOOBER,
            display_id: 259,
            name: "Suspicious Lever".to_string(),
            data0: 0,
            data1: 0,
            gather_skill_line: 0,
            respawn_secs: 0,
            gather_gray: 0,
            lock_id: 0,
            size: 0.0,
        });
    GameObject {
        guid: packed_gameobject_spawn_guid(2),
        template_entry: SUSPICIOUS_LEVER,
        map_id: hw::MAP_ID,
        x: hw::X + 8.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);

    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: DEMO_COPPER_VEIN,
            type_id: lyracore_shared::constants::go_type::GATHER,
            display_id: 259,
            name: "Copper Vein".to_string(),
            data0: 2770,
            data1: 1,
            gather_skill_line: crate::skill::skill_line::MINING,
            respawn_secs: 0,
            gather_gray: 0,
            lock_id: 0,
            size: 0.0,
        });
    GameObject {
        guid: packed_gameobject_spawn_guid(3),
        template_entry: DEMO_COPPER_VEIN,
        map_id: hw::MAP_ID,
        x: hw::X + 6.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);
    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: DEMO_PEACEBLOOM,
            type_id: lyracore_shared::constants::go_type::GATHER,
            display_id: 259,
            name: "Peacebloom".to_string(),
            data0: 2447,
            data1: 1,
            gather_skill_line: crate::skill::skill_line::HERBALISM,
            respawn_secs: 0,
            gather_gray: 0,
            lock_id: 0,
            size: 0.0,
        });
    GameObject {
        guid: packed_gameobject_spawn_guid(4),
        template_entry: DEMO_PEACEBLOOM,
        map_id: hw::MAP_ID,
        x: hw::X + 7.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);

    for (e, name, item, req) in [
        (1731u32, "Copper Vein", 2770u32, 1u32),
        (1732u32, "Tin Vein", 2771u32, 65u32),
    ] {
        if ctx.db.game_gameobject_template().entry().find(e).is_none() {
            ctx.db
                .game_gameobject_template()
                .insert(GameObjectTemplate {
                    entry: e,
                    type_id: lyracore_shared::constants::go_type::GATHER,
                    display_id: 259,
                    name: name.to_string(),
                    data0: item,
                    data1: req,
                    gather_skill_line: crate::skill::skill_line::MINING,
                    respawn_secs: 300,
                    gather_gray: 0,
                    lock_id: 0,
                    size: 0.0,
                });
        }
    }
    const TIER_POOL_ID: u32 = 2;
    ctx.db.game_gameobject_pool().insert(GameObjectPool {
        pool_id: TIER_POOL_ID,
        max_active: 1,
        in_place: true,
    });
    for (entry, weight) in [(1731u32, 85u32), (1732u32, 15u32)] {
        ctx.db
            .game_gameobject_pool_member()
            .insert(GameObjectPoolMember {
                point_id: 0,
                pool_id: TIER_POOL_ID,
                template_entry: entry,
                map_id: hw::MAP_ID,
                x: -9620.11,
                y: -46.3336,
                z: 47.3641,
                orientation: 2.04204,
                weight,
            });
    }
    crate::gameobject::arm_pool(ctx, TIER_POOL_ID);

    crate::weather::seed_weather_weights(ctx);
}

#[allow(clippy::too_many_lines)]
fn seed_spell_registry(ctx: &ReducerContext) {
    let spell = |spell_id: u32,
                 name: &str,
                 power_type: u8,
                 cost: u32,
                 cast_time_ms: u32,
                 range_yd: u32,
                 duration_ms: u32,
                 school_mask: u8,
                 dispel_type: u8,
                 is_negative: bool,
                 max_stacks: u8| {
        ctx.db.game_spell().insert(Spell {
            power_type,
            cost,
            cast_time_ms,
            range_yd,
            duration_ms,
            school_mask,
            dispel_type,
            max_stacks,
            is_negative,
            ..Spell::blank(spell_id, name)
        });
    };
    let effect = |spell_id: u32,
                  idx: u8,
                  kind: u8,
                  base_points: i32,
                  period_ms: u32,
                  target: u8,
                  p0: i32,
                  p0_kind: u8| {
        ctx.db.game_spell_effect().insert(SpellEffect {
            kind,
            base_points,
            period_ms,
            target,
            p0,
            p0_kind,
            equip_supported: matches!(kind, 0xa0 | 0xa1),
            ..SpellEffect::blank(spell_id, idx)
        });
    };
    spell(
        constants::tracer_spell::SPELL_ID,
        "Battle Shout",
        1,
        0,
        0,
        0,
        30000,
        1,
        0,
        false,
        1,
    );
    effect(constants::tracer_spell::SPELL_ID, 0, 0xA3, 30, 0, 0, 0, 5);
    spell(772, "Rend", 1, 10, 0, 5, 21000, 1, 0, true, 0);
    effect(772, 0, 0x90, 7, 3000, 1, 0, 0);
    spell(2050, "Lesser Heal", 0, 30, 1500, 40, 0, 2, 0, false, 0);
    effect(2050, 0, 0x02, 50, 0, 2, 0, 0);
    spell(133, "Fireball", 0, 0, 0, 30, 0, 4, 0, false, 0);
    effect(133, 0, 0x01, 20, 0, 1, 0, 0);
    spell(
        11196,
        "Recently Bandaged",
        0,
        0,
        0,
        30,
        60000,
        0,
        0,
        true,
        0,
    );
    effect(11196, 0, 0xBE, 0, 0, 3, 0, 7);
    spell(980, "Curse of Agony", 0, 0, 0, 30, 24000, 32, 2, true, 0);
    effect(980, 0, 0x90, 5, 1000, 1, 0, 0);
    spell(139, "Renew", 0, 0, 0, 40, 15000, 2, 1, false, 0);
    effect(139, 0, 0x91, 8, 1000, 2, 0, 0);
    spell(527, "Dispel Magic", 0, 0, 0, 30, 0, 1, 0, false, 0);
    effect(527, 0, 0x04, 0, 0, 3, 0, 0);
    spell(
        1126,
        "Mark of the Wild",
        0,
        40,
        0,
        30,
        3600000,
        8,
        1,
        false,
        1,
    );
    effect(1126, 0, 0xA0, 5, 0, 0, 0, 1);
    effect(1126, 1, 0xA0, 5, 0, 0, 1, 1);
    effect(1126, 2, 0xA0, 5, 0, 0, 2, 1);
    spell(588, "Inner Fire", 0, 30, 0, 0, 600000, 2, 1, false, 1);
    effect(588, 0, 0xA1, 2000, 0, 0, 1, 2);

    spell(
        TEST_FIRE_WARD_SPELL,
        "Test Fire Ward",
        0,
        0,
        0,
        30,
        600000,
        4,
        0,
        false,
        1,
    );
    effect(TEST_FIRE_WARD_SPELL, 0, 0xA1, 6, 0, 2, 4, 2);

    spell(
        COMBAT_INSIGHT_SPELL,
        "Combat Insight",
        0,
        0,
        0,
        0,
        600000,
        1,
        0,
        false,
        1,
    );
    effect(COMBAT_INSIGHT_SPELL, 0, 0xA3, 1000, 0, 0, 1, 5);
    effect(COMBAT_INSIGHT_SPELL, 1, 0xA3, 500, 0, 0, 2, 5);

    spell(
        QUICKENING_SPELL,
        "Quickening",
        0,
        0,
        0,
        0,
        600000,
        1,
        0,
        false,
        1,
    );
    effect(QUICKENING_SPELL, 0, 0xA4, 50, 0, 0, 1, 6);

    spell(
        TEST_SNARE_SPELL,
        "Test Snare",
        0,
        0,
        0,
        30,
        600000,
        1,
        0,
        true,
        1,
    );
    effect(TEST_SNARE_SPELL, 0, 0xA4, -40, 0, 1, 0, 6);

    spell(
        TEST_CONJURE_SPELL,
        "Test Conjure",
        0,
        0,
        0,
        0,
        0,
        1,
        0,
        false,
        0,
    );
    effect(TEST_CONJURE_SPELL, 0, 0x07, 2, 0, 0, 5349, 8);

    seed_curated_content(ctx);

    spell(
        ENCHANT_MINOR_STRENGTH_SPELL,
        "Enchant: Minor Strength",
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        false,
        0,
    );
    effect(ENCHANT_MINOR_STRENGTH_SPELL, 0, 0x17, 0, 0, 0, 7745, 10);
    spell(
        ENCHANT_MINOR_STAMINA_SPELL,
        "Enchant: Minor Stamina",
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        false,
        0,
    );
    effect(ENCHANT_MINOR_STAMINA_SPELL, 0, 0x17, 0, 0, 0, 7748, 10);
    spell(13262, "Disenchant", 0, 0, 0, 0, 0, 0, 0, false, 0);
    effect(13262, 0, 0x18, 0, 0, 0, 0, 0);

    spell(
        ROUGH_COPPER_BOMB_SPELL,
        "Rough Copper Bomb",
        0,
        0,
        0,
        0,
        0,
        4,
        0,
        true,
        0,
    );
    effect(ROUGH_COPPER_BOMB_SPELL, 0, 0x01, 15, 0, 4, 0, 0);

    ctx.db.game_spell().insert(Spell {
        school_mask: 2,
        gcd_ms: 0,
        ..Spell::blank(MINOR_HEALING_SPELL, "Minor Healing")
    });
    effect(MINOR_HEALING_SPELL, 0, 0x02, 80, 0, 0, 0, 0);

    ctx.db.game_spell().insert(Spell {
        cast_time_ms: 0,
        gcd_ms: 0,
        range_yd: 0,
        duration_ms: 8000,
        school_mask: 2,
        aura_interrupt: 1,
        cast_flags: crate::spell::CAST_FLAG_CHANNELED,
        ..Spell::blank(LINEN_BANDAGE_SPELL, "Linen Bandage")
    });
    effect(LINEN_BANDAGE_SPELL, 0, 0x91, 8, 1000, 0, 0, 0);
    ctx.db.game_spell_effect().insert(SpellEffect {
        kind: 0x05,
        target: 0,
        trigger_spell: 11196,
        ..SpellEffect::blank(LINEN_BANDAGE_SPELL, 1)
    });

    ctx.db.game_spell().insert(Spell {
        gcd_ms: 0,
        ..Spell::blank(MINOR_MANA_POTION_SPELL, "Minor Mana Potion")
    });
    effect(MINOR_MANA_POTION_SPELL, 0, 0x03, 160, 0, 0, 0, 4);

    ctx.db.game_spell().insert(Spell {
        duration_ms: 30000,
        gcd_ms: 0,
        ..Spell::blank(REFRESHING_WATER_SPELL, "Refreshing Water")
    });
    effect(REFRESHING_WATER_SPELL, 0, 0x92, 41, 5000, 0, 0, 4);

    ctx.db.game_spell().insert(Spell {
        duration_ms: 30000,
        gcd_ms: 0,
        ..Spell::blank(EATING_SPELL, "Eating")
    });
    effect(EATING_SPELL, 0, 0x91, 16, 5000, 0, 0, 0);

    ctx.db.game_spell().insert(Spell {
        duration_ms: 900000,
        max_stacks: 1,
        gcd_ms: 0,
        ..Spell::blank(WELL_FED_SPELL, "Well Fed")
    });
    effect(WELL_FED_SPELL, 0, 0xA0, 2, 0, 0, 2, 1);
    effect(WELL_FED_SPELL, 1, 0xA0, 2, 0, 0, 4, 1);

    ctx.db.game_spell().insert(Spell {
        school_mask: 2,
        gcd_ms: 0,
        ..Spell::blank(LESSER_HEALING_SPELL, "Lesser Healing")
    });
    effect(LESSER_HEALING_SPELL, 0, 0x02, 160, 0, 0, 0, 0);

    ctx.db.game_spell().insert(Spell {
        gcd_ms: 0,
        duration_ms: 8000,
        school_mask: 2,
        aura_interrupt: 1,
        cast_flags: crate::spell::CAST_FLAG_CHANNELED,
        ..Spell::blank(HEAVY_LINEN_BANDAGE_SPELL, "Heavy Linen Bandage")
    });
    effect(HEAVY_LINEN_BANDAGE_SPELL, 0, 0x91, 18, 1000, 0, 0, 0);
    ctx.db.game_spell_effect().insert(SpellEffect {
        kind: 0x05,
        trigger_spell: 11196,
        ..SpellEffect::blank(HEAVY_LINEN_BANDAGE_SPELL, 1)
    });

    ctx.db.game_spell().insert(Spell {
        cast_time_ms: 10_000,
        gcd_ms: 0,
        ..Spell::blank(8690, "Hearthstone")
    });
    effect(8690, 0, crate::spell::E_RECALL_HOME, 0, 0, 0, 0, 0);

    spell(
        TEST_STUN_SPELL,
        "Test Stun",
        0,
        0,
        0,
        30,
        600000,
        1,
        0,
        true,
        1,
    );
    effect(TEST_STUN_SPELL, 0, 0xB0, 0, 0, 1, 1, 3);

    spell(
        TEST_ROOT_SPELL,
        "Test Root",
        0,
        0,
        0,
        30,
        600000,
        1,
        0,
        true,
        1,
    );
    effect(TEST_ROOT_SPELL, 0, 0xB0, 0, 0, 1, 2, 3);

    spell(
        TEST_FEAR_SPELL,
        "Test Fear",
        0,
        0,
        0,
        30,
        8000,
        1,
        0,
        true,
        1,
    );
    effect(TEST_FEAR_SPELL, 0, 0xB0, 0, 0, 1, 3, 3);

    spell(
        TEST_POLY_SPELL,
        "Test Poly",
        0,
        0,
        0,
        30,
        10000,
        1,
        0,
        true,
        1,
    );
    effect(TEST_POLY_SPELL, 0, 0xB0, 0, 0, 1, 4, 3);

    spell(
        TEST_STUN_IMMUNITY_SPELL,
        "Test Stun Immunity",
        0,
        0,
        0,
        30,
        600000,
        1,
        0,
        false,
        1,
    );
    effect(TEST_STUN_IMMUNITY_SPELL, 0, 0xB1, 0, 0, 2, 1, 3);

    spell(355, "Taunt", 1, 0, 0, 30, 0, 1, 0, true, 0);
    effect(355, 0, 0x06, 0, 0, 1, 0, 0);

    spell(
        15007,
        "Resurrection Sickness",
        0,
        0,
        0,
        0,
        600000,
        0,
        0,
        true,
        1,
    );
    effect(15007, 0, 0xA0, -10, 0, 0, 0xFF, 1);

    reconcile_curated_starter_role_levels(ctx);
}

pub(crate) fn reconcile_curated_starter_role_levels(ctx: &ReducerContext) -> u32 {
    let spells = ctx.db.game_spell();
    let mut repaired = 0;
    for (spell_id, level) in [
        (133, 1),
        (139, 8),
        (168, 1),
        (355, 10),
        (1243, 1),
        (2050, 1),
        (6673, 1),
    ] {
        let Some(mut spell) = spells.spell_id().find(spell_id) else {
            continue;
        };
        let old_curated_shape = spell.gcd_ms == 1500
            && spell.cooldown_ms == 0
            && spell.mechanic == 0
            && spell.aura_interrupt == 0
            && spell.attributes == 0
            && spell.spell_level == 0
            && spell.max_level == 0
            && spell.cast_flags == 0
            && spell.stances == 0
            && spell.family_name == 0
            && spell.family_flags == 0
            && match spell_id {
                133 => {
                    spell.name == "Fireball"
                        && spell.power_type == 0
                        && spell.cost == 0
                        && spell.cast_time_ms == 0
                        && spell.range_yd == 30
                        && spell.duration_ms == 0
                        && spell.school_mask == 4
                        && spell.dispel_type == 0
                        && spell.max_stacks == 0
                        && !spell.is_negative
                        && spell.proc_flags == 0
                        && spell.proc_chance == 0
                        && spell.proc_charges == 0
                }
                139 => {
                    spell.name == "Renew"
                        && spell.power_type == 0
                        && spell.cost == 0
                        && spell.cast_time_ms == 0
                        && spell.cooldown_ms == 0
                        && spell.range_yd == 40
                        && spell.duration_ms == 15_000
                        && spell.school_mask == 2
                        && spell.dispel_type == 1
                        && spell.max_stacks == 0
                        && !spell.is_negative
                        && spell.proc_flags == 0
                        && spell.proc_chance == 0
                        && spell.proc_charges == 0
                }
                168 => {
                    spell.name == "Frost Armor"
                        && spell.power_type == 0
                        && spell.cost == 0
                        && spell.cast_time_ms == 0
                        && spell.range_yd == 0
                        && spell.duration_ms == u32::MAX
                        && spell.school_mask == 16
                        && spell.dispel_type == 0
                        && spell.max_stacks == 0
                        && spell.proc_flags == 0x28
                        && spell.proc_chance == 100
                        && spell.proc_charges == 0
                        && !spell.is_negative
                }
                355 => {
                    spell.name == "Taunt"
                        && spell.power_type == 1
                        && spell.cost == 0
                        && spell.cast_time_ms == 0
                        && spell.cooldown_ms == 0
                        && spell.range_yd == 30
                        && spell.duration_ms == 0
                        && spell.school_mask == 1
                        && spell.dispel_type == 0
                        && spell.max_stacks == 0
                        && spell.is_negative
                        && spell.proc_flags == 0
                        && spell.proc_chance == 0
                        && spell.proc_charges == 0
                }
                1243 => {
                    spell.name == "Power Word: Fortitude"
                        && spell.power_type == 0
                        && spell.cost == 0
                        && spell.cast_time_ms == 0
                        && spell.range_yd == 30
                        && spell.duration_ms == 1_800_000
                        && spell.school_mask == 0
                        && spell.dispel_type == 0
                        && spell.max_stacks == 1
                        && !spell.is_negative
                        && spell.proc_flags == 0
                        && spell.proc_chance == 0
                        && spell.proc_charges == 0
                }
                2050 => {
                    spell.name == "Lesser Heal"
                        && spell.power_type == 0
                        && spell.cost == 30
                        && spell.cast_time_ms == 1500
                        && spell.range_yd == 40
                        && spell.duration_ms == 0
                        && spell.school_mask == 2
                        && spell.dispel_type == 0
                        && spell.max_stacks == 0
                        && !spell.is_negative
                        && spell.proc_flags == 0
                        && spell.proc_chance == 0
                        && spell.proc_charges == 0
                }
                6673 => {
                    spell.name == "Battle Shout"
                        && spell.power_type == 1
                        && spell.cost == 0
                        && spell.cast_time_ms == 0
                        && spell.range_yd == 0
                        && spell.duration_ms == 30_000
                        && spell.school_mask == 1
                        && spell.dispel_type == 0
                        && spell.max_stacks == 1
                        && !spell.is_negative
                        && spell.proc_flags == 0
                        && spell.proc_chance == 0
                        && spell.proc_charges == 0
                }
                _ => false,
            };
        if old_curated_shape {
            spell.spell_level = level;
            spells.spell_id().update(spell);
            repaired += 1;
        }
    }
    repaired
}

fn legacy_lesser_heal_header(spell: &Spell) -> bool {
    spell.spell_id == 2050
        && spell.name == "Lesser Heal"
        && spell.power_type == 0
        && spell.cost == 30
        && spell.cast_time_ms == 1500
        && spell.gcd_ms == 1500
        && spell.cooldown_ms == 0
        && spell.range_yd == 40
        && spell.duration_ms == 0
        && spell.school_mask == 2
        && spell.dispel_type == 0
        && spell.mechanic == 0
        && spell.max_stacks == 0
        && spell.aura_interrupt == 0
        && spell.attributes == 0
        && matches!(spell.spell_level, 0 | 1)
        && spell.max_level == 0
        && !spell.is_negative
        && spell.cast_flags == 0
        && spell.stances == 0
        && spell.family_name == 0
        && spell.family_flags == 0
        && spell.proc_flags == 0
        && spell.proc_chance == 0
        && spell.proc_charges == 0
}

fn legacy_lesser_heal_effect(effect: &SpellEffect) -> bool {
    effect.id == packed_spell_effect_id(2050, 0)
        && effect.spell_id == 2050
        && effect.effect_index == 0
        && effect.kind == 0x02
        && effect.base_points == 50
        && effect.die_sides == 0
        && effect.per_level == 0.0
        && effect.period_ms == 0
        && effect.target == crate::spell::T_SELF
        && effect.radius_yd == 0.0
        && effect.chain_targets == 0
        && effect.trigger_spell == 0
        && effect.effect_mechanic == 0
        && effect.p0 == 0
        && effect.p0_kind == 0
        && effect.p1 == 0
        && effect.script_id == 0
        && !effect.enters_combat
}

pub(crate) fn repair_lesser_heal_target(ctx: &ReducerContext) -> u64 {
    let Some(spell) = ctx.db.game_spell().spell_id().find(2050) else {
        return 0;
    };
    let effects = ctx.db.game_spell_effect();
    let mut spell_effects: Vec<_> = effects.by_spell().filter(&2050u32).take(2).collect();
    if spell_effects.len() != 1 {
        return 0;
    }
    let mut effect = spell_effects.pop().unwrap();
    if !legacy_lesser_heal_header(&spell) || !legacy_lesser_heal_effect(&effect) {
        return 0;
    }
    effect.target = crate::spell::T_TARGET_ALLY;
    effects.id().update(effect);
    1
}

#[derive(Clone, Copy)]
enum Arming {
    Ensure,
    Rearm,
}

fn arm<T: Table>(table: T, arming: Arming, row: impl FnOnce() -> T::Row) -> u64 {
    match arming {
        Arming::Ensure if table.count() > 0 => return 0,
        Arming::Ensure => {}
        Arming::Rearm => {
            let stale: Vec<_> = table.iter().collect();
            for row in stale {
                table.delete(row);
            }
        }
    }
    table.insert(row());
    1
}

/// The interval schedules `init` and the post-publish repair arm; returns the rows it inserted.
pub(crate) fn arm_interval_schedules(ctx: &ReducerContext) -> u64 {
    use Arming::{Ensure, Rearm};
    let every =
        |micros: i64| crate::time::schedule_interval(ctx, TimeDuration::from_micros(micros));
    let mut armed = 0;
    armed += arm(ctx.db.game_event_reaper_schedule(), Ensure, || {
        EventReaperSchedule {
            scheduled_id: 0,
            scheduled_at: every(EVENT_TTL_MICROS),
        }
    });
    armed += arm(ctx.db.game_instance_reaper_schedule(), Rearm, || {
        crate::instance::InstanceReaperSchedule {
            scheduled_id: 0,
            scheduled_at: every(crate::instance::INSTANCE_REAPER_INTERVAL_MICROS),
        }
    });
    armed += arm(ctx.db.game_melee_schedule(), Ensure, || MeleeSchedule {
        scheduled_id: 0,
        scheduled_at: every(100_000),
    });
    armed += arm(ctx.db.game_pet_care_schedule(), Ensure, || {
        PetCareSchedule {
            scheduled_id: 0,
            scheduled_at: every(crate::creatures::CARE_INTERVAL_MICROS),
        }
    });
    armed += arm(ctx.db.game_aura_schedule(), Ensure, || AuraSchedule {
        scheduled_id: 0,
        scheduled_at: every(1_000_000),
    });
    armed += arm(ctx.db.game_breath_schedule(), Ensure, || BreathSchedule {
        scheduled_id: 0,
        scheduled_at: every(1_000_000),
    });
    armed += arm(ctx.db.game_duel_schedule(), Ensure, || {
        crate::duel::DuelSchedule {
            scheduled_id: 0,
            scheduled_at: every(crate::duel::DUEL_TICK_MICROS),
        }
    });
    armed += arm(ctx.db.game_ground_area_schedule(), Ensure, || {
        GroundAreaSchedule {
            scheduled_id: 0,
            scheduled_at: every(500_000),
        }
    });
    // Ensure, not rearm: scheduled_at keeps the Operator's chosen motion cadence.
    armed += arm(ctx.db.game_motion_publish_schedule(), Ensure, || {
        crate::motion::MotionPublishSchedule {
            scheduled_id: 0,
            scheduled_at: every(crate::motion::MOTION_TICK_MICROS),
        }
    });
    armed += arm(ctx.db.game_gateway_lease_reaper_schedule(), Rearm, || {
        crate::gw::GatewayLeaseReaperSchedule {
            scheduled_id: 0,
            scheduled_at: every(crate::gw::ACCOUNT_CLAIM_REAP_MICROS),
        }
    });
    crate::creatures::tick::arm_creature_tick(
        ctx,
        TimeDuration::from_micros(crate::creatures::SLICED_TICK_MICROS),
    );
    crate::weather::rearm_weather_schedule(ctx);
    crate::meeting_stone::rearm_reminder_schedule(ctx);
    crate::transport::arm_crossings(ctx);
    armed
}

mod fixtures;
#[cfg(feature = "debug_reducers")]
pub(crate) use fixtures::seed_scenario_fixtures;
#[cfg(feature = "debug_reducers")]
pub(crate) use fixtures::FIXTURE_FACTION;
use fixtures::{
    profession_trainer_template, seed_boar_family, seed_demon_skin_fixture,
    seed_drain_soul_fixture, seed_fixture_catalogue, seed_frost_armor_fixture,
    seed_hunter_tame_fixture, seed_hunter_tame_kit, seed_mana_burn_fixture, seed_mount_fixture,
    seed_pw_shield_fixture, seed_regen_fixture, seed_riding_skill_line, seed_soul_shard_item,
    seed_stacking_probe_fixture, seed_stealth_fixture, seed_test_proc_fixtures, seed_weakened_soul,
    test_wolf_template,
};
pub(crate) use fixtures::{seed_taxi_fixture, tempered_blade_template, tough_jerky_template};

fn seed_createinfo_spells(ctx: &ReducerContext) {
    use crate::spell::spellbook::game_createinfo_spell;
    let table = ctx.db.game_createinfo_spell();
    let base_kit = if table.count() == 0 {
        CREATEINFO_KIT
    } else {
        &[]
    };
    let weapon_kit = CREATEINFO_WEAPON_KIT
        .iter()
        .flat_map(|&(race, class, spells)| spells.iter().map(move |&spell| (race, class, spell)));
    let present: std::collections::HashSet<(u8, u8, u32)> = table
        .iter()
        .map(|row| (row.race, row.class, row.spell_id))
        .collect();
    // Imported rows carry explicit ids, so the auto_inc sequence could collide with them.
    let mut next_id = table
        .iter()
        .map(|row| row.id)
        .filter(|&id| !lyracore_package_delta::is_package_globals_id(id))
        .max()
        .unwrap_or(0);
    for (race, class, spell_id) in base_kit.iter().copied().chain(weapon_kit) {
        if present.contains(&(race, class, spell_id)) {
            continue;
        }
        next_id += 1;
        table.insert(crate::spell::spellbook::CreateinfoSpell {
            id: next_id,
            race,
            class,
            spell_id,
        });
    }
}

pub(crate) fn seed_spell_groups(ctx: &ReducerContext) {
    use crate::spell::stacking::{
        game_spell_group, game_spell_group_rule, SpellGroup, SpellGroupRule,
    };
    let groups = ctx.db.game_spell_group();
    let rules = ctx.db.game_spell_group_rule();

    for &(group_id, rule, rank_is_comparable, members) in SPELL_GROUPS {
        let rule_row = SpellGroupRule {
            group_id,
            rule,
            rank_is_comparable,
        };
        if rules.group_id().find(group_id).is_some() {
            rules.group_id().update(rule_row);
        } else {
            rules.insert(rule_row);
        }
        for &spell_id in members {
            if !groups
                .by_group()
                .filter(&group_id)
                .any(|row| row.spell_id == spell_id)
            {
                groups.insert(SpellGroup {
                    id: 0,
                    group_id,
                    spell_id,
                });
            }
        }
    }
}

pub(crate) const SPELL_GROUPS: &[(u32, u8, bool, &[u32])] = {
    use crate::spell::stacking::{
        RULE_EXCLUSIVE, RULE_EXCLUSIVE_PER_CASTER, RULE_EXCLUSIVE_STRONGER,
    };
    &[
        (
            1,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[1126, 5232, 6756, 5234, 8907, 9884, 9885, 21849, 21850],
        ),
        (
            2,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[1243, 1244, 1245, 2791, 10937, 10938, 21562, 21564],
        ),
        (
            3,
            RULE_EXCLUSIVE_PER_CASTER,
            false,
            &[
                19740, 19834, 19835, 19836, 19837, 19838, 25291, 19742, 19850, 19852, 19853, 19854,
                25290, 20217, 1038, 20911, 20912, 20913, 20914, 19977, 19978, 19979, 25782, 25916,
                25894, 25918, 25890, 25895, 25898, 25899,
            ],
        ),
        (
            4,
            RULE_EXCLUSIVE_STRONGER,
            true,
            &[6673, 5242, 6192, 11549, 11550, 11551, 25289],
        ),
        (
            5,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[
                7386, 7405, 8380, 11596, 11597, 8647, 8649, 8650, 11197, 11198, 770, 778, 9749,
                9907, 16857, 17390, 17391, 17392,
            ],
        ),
        (
            6,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[1459, 1460, 1461, 10156, 10157, 23028],
        ),
        (
            7,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[14752, 14818, 14819, 27841, 27681],
        ),
        (
            8,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[976, 10957, 10958, 27683],
        ),
        (
            9,
            RULE_EXCLUSIVE,
            false,
            &[
                WELL_FED_SPELL,
                19705,
                19706,
                19708,
                19709,
                19710,
                19711,
                24799,
                24870,
                25694,
                25941,
            ],
        ),
    ]
};

pub(crate) const CREATEINFO_KIT: &[(u8, u8, u32)] = &[
    (0, 1, 78),
    (0, 1, 2457),
    (0, 2, 635),
    (0, 2, 20154),
    (0, 3, 75),
    (0, 3, 2973),
    (0, 3, 1978),
    (0, 4, 1752),
    (0, 4, 2098),
    (0, 5, 585),
    (0, 5, 2050),
    (0, 7, 403),
    (0, 7, 331),
    (0, 7, 8042),
    (0, 8, 133),
    (0, 8, 168),
    (0, 9, 686),
    (0, 9, 687),
    (0, 11, 1126),
    (0, 11, 5176),
    (0, 11, 8921),
    (0, 11, 5185),
    (0, 11, 774),
    (1, 0, 20597),
    (1, 0, 20598),
    (1, 0, 20599),
    (1, 0, 20600),
    (1, 0, 20864),
];

// The weapon proficiency rows of vmangos playercreateinfo_spell, by race and class.
pub(crate) const CREATEINFO_WEAPON_KIT: &[(u8, u8, &[u32])] = &[
    (1, 1, &[196, 198, 201]),
    (1, 2, &[198, 199]),
    (1, 4, &[1180, 2567]),
    (1, 5, &[198, 5009]),
    (1, 8, &[227, 5009]),
    (1, 9, &[1180, 5009]),
    (2, 1, &[196, 197, 201]),
    (2, 3, &[196, 264]),
    (2, 4, &[1180, 2567]),
    (2, 7, &[198, 227]),
    (2, 9, &[1180, 5009]),
    (3, 1, &[196, 197, 198]),
    (3, 2, &[198, 199]),
    (3, 3, &[196, 266]),
    (3, 4, &[1180, 2567]),
    (3, 5, &[198, 5009]),
    (4, 1, &[198, 201, 1180]),
    (4, 3, &[264, 1180]),
    (4, 4, &[1180, 2567]),
    (4, 5, &[198, 5009]),
    (4, 11, &[227, 1180]),
    (5, 1, &[201, 202, 1180]),
    (5, 4, &[1180, 2567]),
    (5, 5, &[198, 5009]),
    (5, 8, &[227, 5009]),
    (5, 9, &[1180, 5009]),
    (6, 1, &[196, 198, 199]),
    (6, 3, &[196, 266]),
    (6, 7, &[198, 227]),
    (6, 11, &[198, 227]),
    (7, 1, &[198, 201, 1180]),
    (7, 4, &[1180, 2567]),
    (7, 8, &[227, 5009]),
    (7, 9, &[1180, 5009]),
    (8, 1, &[196, 1180, 2567]),
    (8, 3, &[196, 264]),
    (8, 4, &[1180, 2567]),
    (8, 5, &[198, 5009]),
    (8, 7, &[198, 227]),
    (8, 8, &[227, 5009]),
];

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA_CLASSES: &[u8] = &[1, 2, 3, 4, 5, 7, 8, 9, 11];

    #[test]
    fn every_playable_vanilla_class_has_a_starting_kit() {
        for &class in VANILLA_CLASSES {
            let kit: Vec<u32> = CREATEINFO_KIT
                .iter()
                .filter(|(_, c, _)| *c == class)
                .map(|(_, _, spell)| *spell)
                .collect();
            assert!(
                kit.len() >= 2,
                "class {class} starts with {} spell(s) {kit:?}; every class needs at least two",
                kit.len()
            );
        }

        for (race, class, spell) in CREATEINFO_KIT {
            assert!(
                *class == 0 || VANILLA_CLASSES.contains(class),
                "kit row (race {race}, class {class}, spell {spell}) names a class absent from 1.12.1"
            );
        }
    }

    #[test]
    fn the_starting_kit_rows_are_unique_and_never_wildcard_on_both_columns() {
        let mut seen = std::collections::BTreeSet::new();
        for row in CREATEINFO_KIT {
            let (race, class, spell) = row;
            assert!(
                !(*race == 0 && *class == 0),
                "kit row {row:?} is a wildcard on both race and class"
            );
            assert_ne!(
                *spell, 0,
                "kit row {row:?} grants spell id 0, which is not a spell"
            );
            assert!(seen.insert(*row), "kit row {row:?} appears twice");
        }

        assert!(
            CREATEINFO_KIT.iter().any(|(race, _, _)| *race == 1),
            "no Human racials in the kit; every new character would be missing them with no error"
        );
    }

    #[test]
    fn every_race_and_class_starts_with_weapon_proficiencies() {
        let mut seen = std::collections::BTreeSet::new();
        for &(race, class, spells) in CREATEINFO_WEAPON_KIT {
            assert!(
                seen.insert((race, class)),
                "race {race} class {class} twice"
            );
            assert!(VANILLA_CLASSES.contains(&class) && (1..=8).contains(&race));
            assert!(!spells.is_empty(), "race {race} class {class}");
            for &spell in spells {
                assert!(
                    lyracore_shared::item::is_weapon_proficiency_spell(spell),
                    "race {race} class {class} spell {spell}"
                );
            }
        }
        assert_eq!(seen.len(), 40, "vanilla has 40 race and class pairs");
    }

    #[test]
    fn the_live_probe_fixture_spells_sit_in_the_families_the_probe_documents() {
        let family_of = |spell_id: u32| {
            SPELL_GROUPS
                .iter()
                .find(|&&(_, _, _, members)| members.contains(&spell_id))
                .map(|&(group_id, rule, rank_is_comparable, _)| {
                    (group_id, rule, rank_is_comparable)
                })
                .unwrap_or_else(|| panic!("probe spell {spell_id} is in no family"))
        };
        assert_eq!(family_of(1243), family_of(21562));
        assert_eq!(
            family_of(1243),
            (2, crate::spell::stacking::RULE_EXCLUSIVE_STRONGER, false)
        );
        assert_eq!(family_of(19740), family_of(19742));
        assert_eq!(
            family_of(19740).1,
            crate::spell::stacking::RULE_EXCLUSIVE_PER_CASTER
        );
    }

    #[test]
    fn well_fed_replaces_rather_than_comparing_strength() {
        let (_, rule, _, members) = *SPELL_GROUPS
            .iter()
            .find(|&&(id, ..)| id == 9)
            .expect("the Well Fed family is gone");
        assert_eq!(rule, crate::spell::stacking::RULE_EXCLUSIVE);
        assert!(members.contains(&19705), "the real Well Fed food buff");
    }

    #[test]
    fn no_spell_belongs_to_two_families() {
        let mut seen = std::collections::HashMap::new();
        for &(group_id, _, _, members) in SPELL_GROUPS {
            for &spell_id in members {
                if let Some(other) = seen.insert(spell_id, group_id) {
                    panic!("spell {spell_id} is in groups {other} and {group_id}");
                }
            }
        }
    }
}
