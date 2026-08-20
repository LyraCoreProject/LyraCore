//! The `init` lifecycle reducer — the single entrypoint that populates a fresh database. `init`
//! itself is a four-line dispatcher (#377) over four banner-stratum fns, each a straight
//! extraction of what used to be one ~1,600-line function (a reader still sees the whole seed by
//! reading top to bottom — the split is fn boundaries, not a reorder):
//!
//! 1. **`seed_production_core`**: realm, server config, the human-warrior start position, the
//!    fallback graveyard/graveyard-zone rows (work-item 209), the TEST account + pre-seeded
//!    character (with its starter spellbook/action-bar kit), and the EventAI on-aggro barks. Every
//!    fresh database needs this regardless of whether it will ever host a real import.
//! 2. **`seed_map0_demo_content`** (the in-body `DECISION (issue #79)` comment has the full
//!    reasoning): NPCs, a starter weapon, profession items, a skinning beast, a profession trainer,
//!    gameobjects, gather nodes, a tier-variety demonstrator, and the temporary Elwynn Forest /
//!    Westfall weather climate, each under its own `// ---` banner.
//!    Every row here is wholesale-replaced the moment a real `importer --apply` run lands for map 0
//!    (or fenced off entirely for any other continent). This is a DIFFERENT fixture family from
//!    `seed/fixtures.rs`'s synthetic engine-mechanic fixtures (5xxxx ids, no map content) — see that
//!    file's header.
//! 3. **`seed_spell_registry`**: a hand-authored spell/item registry, crafted-consumable spells,
//!    1-10-alpha consumable breadth, the mock-seed fixture kits (`seed/fixtures.rs`), enchant/
//!    disenchant, talents, and the stacking-group starter set.
//! 4. **`seed_scheduler_arming`**: the event reaper, instance reaper, creature movement/melee/aura/
//!    ground-AoE/weather ticks. Runs last so nothing fires against a half-seeded database.
//!
//! Base-row constructors (`base_spell`/`base_effect`/`base_item`, `seed/fixtures.rs`) plus the
//! `spell`/`effect` closures below keep the ~700 lines of `Spell`/`SpellEffect`/`ItemTemplate`
//! literals in strata 2-3 to their meaningful fields only — see `base_spell`'s doc for the
//! discipline.
//!
//! Touches every domain, so it imports each table's accessor trait + row type from the crate root.

use lyracore_shared::constants;
use spacetimedb::{reducer, Identity, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{
    build_creature_entity, game_account, game_aura_schedule, game_breath_schedule, game_character,
    game_config, game_creature_loot, game_creature_move_schedule, game_creature_spawn,
    game_creature_template, game_creature_waypoint, game_duel_schedule, game_event_reaper_schedule,
    game_gameobject, game_gameobject_pool, game_gameobject_pool_member, game_gameobject_template,
    game_gateway_lease_reaper_schedule, game_graveyard, game_graveyard_zone,
    game_ground_area_schedule, game_instance_reaper_schedule, game_item_template,
    game_melee_schedule, game_motion_publish_schedule, game_pet_care_schedule, game_realm,
    game_spell, game_spell_effect, game_start_position, Account, AuraSchedule, BreathSchedule,
    Character, CreatureLoot, CreatureMoveSchedule, CreatureSpawn, CreatureTemplate,
    CreatureWaypoint, EventReaperSchedule, GameObject, GameObjectPool, GameObjectPoolMember,
    GameObjectTemplate, GraveyardLoc, GraveyardZone, GroundAreaSchedule, ItemTemplate,
    MeleeSchedule, PetCareSchedule, Realm, ServerConfig, Spell, SpellEffect, StartPosition,
    EVENT_TTL_MICROS,
};
use crate::{game_alpha_test_tools_enrollment, AlphaTestToolsEnrollment};

#[reducer(init)]
pub fn init(ctx: &ReducerContext) {
    // Four banner strata (#377 split these out of what used to be one ~1,600-line fn — see this
    // file's header for what each one seeds and why the split points fall where they do). Order
    // matters: later strata reference nothing from earlier ones (each re-derives its own `hw`
    // alias), but the production core must exist before anything reads `game_config`/`game_realm`,
    // and the scheduler must arm last so nothing fires against a half-seeded database.
    seed_production_core(ctx);
    seed_map0_demo_content(ctx);
    seed_spell_registry(ctx);
    seed_scheduler_arming(ctx);
}

/// Stratum 1 — the production core every fresh database needs regardless of whether it will ever
/// host a real import: realm, server config, the human-warrior start position, the fallback
/// graveyard/graveyard-zone rows (work-item 209), the TEST account + pre-seeded character (with its
/// starter spellbook/action-bar kit), and the EventAI on-aggro barks.
fn seed_production_core(ctx: &ReducerContext) {
    use constants::start_human_warrior as hw;

    // Realm (points at the world gateway).
    ctx.db.game_realm().insert(Realm {
        id: 1,
        name: "LyraCore".to_string(),
        address: "127.0.0.1:8085".to_string(),
        realm_type: 0,
        flags: 0,
        population: 0.0,
        timezone: 1,
    });

    // Server tunables: the xp_rate singleton starts Blizzlike (1.0×). Admins tune it via SQL
    // (`UPDATE game_config SET xp_rate = N WHERE id = 0`) or the `debug_set_xp_rate` reducer.
    // `hosts_instances` starts TRUE — a fresh single-database realm spawns dungeon populations
    // itself, exactly as before #39. A multi-database deployment turns it off on the world shard.
    ctx.db.game_config().insert(ServerConfig {
        id: 0,
        xp_rate: 1.0,
        nav_enabled: true,
        hosts_instances: true,
        bots_idle: false, // bots think by default; the load-test lever freezes them
        vmap_enabled: false, // #521/#523: off until an operator imports vmap data + flips it
        nav_coverage_enabled: false, // off until an operator prepares coverage + flips it
    });

    ctx.db
        .game_alpha_test_tools_enrollment()
        .insert(AlphaTestToolsEnrollment {
            id: 0,
            enabled: true,
        });

    // Human Warrior start position (display 49 = human male native model).
    ctx.db.game_start_position().insert(StartPosition {
        race_class: ((hw::RACE as u16) << 8) | hw::CLASS as u16,
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

    // Graveyard fallback seed (work-item 209): the SAME five Elwynn/Westfall graveyards
    // `world::graveyard`'s hardcoded consts carry, ALSO row-seeded into `game_graveyard` +
    // `game_graveyard_zone` so a fresh unimported DB and the live `graveyard::resolve_graveyard`
    // path agree exactly — mirrors the `game_start_position` precedent (init seeds; the importer's
    // `--dbc`/`--dump` clear+reload overwrite both tables with the real WorldSafeLocs.dbc /
    // game_graveyard_zone data once run). `faction: 469` (Alliance) on every row — Elwynn/Westfall
    // are Alliance-only leveling content in this sandbox.
    const ALLIANCE_FACTION: u32 = 469;
    for (id, name, x, y, z, o, zone_id) in [
        // Orientations carry the consts' values (Northshire's 2.72271 is the verified facing) —
        // the DBC import writes 0.0 here, so the seed is the only source of a real facing today.
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
        (80, "Sentinel Hill", -10650.0, 1180.0, 34.0, 0.0, 40), // [V] id/coords unverified — see world::graveyard
        (81, "Westfall Coast", -11390.0, 1590.0, 6.0, 0.0, 40), // [V] id/coords unverified — see world::graveyard
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
            row_id: 0, // auto_inc
            safe_loc_id: id,
            zone_id,
            faction: ALLIANCE_FACTION,
        });
    }

    // Test account (credentials provisioned later by the gateway via `provision_account`).
    let account = ctx.db.game_account().insert(Account {
        id: 0, // auto_inc
        username: "TEST".to_string(),
        salt: vec![0u8; 32],
        verifier: vec![0u8; 32],
        identity: None,
        banned: false,
        alpha_test_tools: true,
    });

    seed_createinfo_spells(ctx);

    // Pre-seeded character (owner bound at establish_session; ZERO hides it until then).
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
        money: 0, // starts broke; loot fills the purse
        rested_xp: 0,
        last_logout_micros: 0,
        // Hearthstone home = the seeded start position.
        home_map: hw::MAP_ID,
        home_zone: hw::ZONE_ID,
        home_x: hw::X,
        home_y: hw::Y,
        home_z: hw::Z,
        played_total_secs: 0,
        session_start_micros: 0,
        health: 0, // sentinel: spawn at full health
        power: 0,  // sentinel: spawn at starting power
        respec_count: 0,
        death_expire_micros: 0,                                   // never died
        pending_instance_id: 0,                                   // open world
        gm_level: 3, // work-item 223: the seeded Tester is playtest-GM by default
        pending_ghost: false, // alive (work-item 226)
        resting: false, // 196
        rested_since_micros: 0, // 196
        pending_godmode: false, // 289: GM playtest carry — off until `.god` + a map change
        pending_run_speed_mult_bp: crate::world::RUN_SPEED_BP_1X, // 289: 1×
        bank_bag_slots: 0,
    });
    // The seeded character goes through the same creation-time kit grant as `create_character`
    // (rows restamp to the real owner identity at establish_session, like its other owned rows).
    crate::spell::spellbook::grant_createinfo_spells(ctx, 1, Identity::ZERO, hw::RACE, hw::CLASS);
    // Action-bar rows (work-item 212) — same no-op-pre-import grant `create_character` calls.
    crate::action_bar::grant_createinfo_actions(ctx, 1, Identity::ZERO, hw::RACE, hw::CLASS);

    // Creature EventAI (193): the fixture on-aggro barks (Kobold/Defias/Hogger).
    crate::creatures::seed_on_aggro_fixtures(ctx);
}

/// Stratum 2 — Map-0 (Northshire) demo/fixture content (the `DECISION (issue #79)` comment below
/// has the full reasoning): NPCs, a starter weapon, profession items, a skinning beast, a
/// profession trainer, gameobjects, gather nodes, a tier-variety demonstrator, and the temporary
/// Elwynn Forest / Westfall weather climate. Every row here is wholesale-replaced the moment a real
/// `importer --apply` run lands for map 0 (or fenced off entirely for any other continent). This is
/// a DIFFERENT fixture family from `seed/fixtures.rs`'s synthetic engine-mechanic fixtures (5xxxx
/// ids, no map content) — see that file's header.
fn seed_map0_demo_content(ctx: &ReducerContext) {
    use constants::start_human_warrior as hw;

    seed_taxi_fixture(ctx);

    // DECISION (issue #79): everything from here down through the gather-pool block seeds MAP-0
    // (Northshire) spatial content — 4 creature spawns (Chicken 620, Test Wolf 51000, Profession
    // Trainer 51001, Test Flight Master 51006) and up to 5 live `game_gameobject` rows (the
    // chest/goober/2 standalone gather nodes + the tier-pool's one armed point) — into EVERY freshly
    // published database, even one that
    // will only ever host a different continent. `init` KEEPS seeding them: they are the local
    // test-harness/module-test fixtures a bare `spacetime publish` needs (a combat target, a skinnable
    // beast, a profession trainer, a chest/goober/gather-node to exercise `use_gameobject`) — used by
    // the manual verify recipes and `debug_*` reducers throughout this file and `debug.rs`, on ANY
    // database that has not yet been pointed at a real world import. The moment a real
    // `importer --apply` run lands (for map 0 OR any other continent), the four spatial families'
    // wholesale clears permanently replace these rows with real content, and `game_gameobject`'s pool
    // arming (`arm_pool`, gameobject.rs) map-fences itself against `game_terrain_chunk`/
    // `game_nav_chunk` so it can never re-plant the map-0 tier point on a database that has since
    // imported a different continent. So: no database that has ever received a real import needs
    // these fixtures again, and no database needs `init` to STOP seeding them, because the one-
    // continent-per-database guard (`importer/scripts/import-world.sh`) now recognizes them as fixtures, not
    // content, precisely because they never survive a real import.
    // --- NPCs: seed a Chicken (entry 620) near the player spawn so it is in view. ---
    // Real vanilla values; display 304 ships in the base 5875 client. Faction 14
    // ("Monster") is HOSTILE to players (red nameplate + sword cursor + right-click auto-attack) so
    // it is a usable combat target. Real chickens are neutral critters (faction 31), but a neutral
    // unit shows no sword cursor and can't be right-click-attacked — only `/startattack` works — so
    // the demo combat target is made hostile. It never fights back (no creature AI yet).
    const CHICKEN_ENTRY: u32 = 620;
    let chicken_tmpl = ctx.db.game_creature_template().insert(CreatureTemplate {
        entry: CHICKEN_ENTRY,
        name: "Chicken".to_string(),
        subname: String::new(),
        display_id: 304,
        level: 1,
        health: 42,
        faction_template: 14,
        npc_flags: 0,
        unit_flags: 0,
        creature_type: 8, // Critter
        creature_family: 0,
        type_flags: 0,
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 1500, // a quick pecker — visibly faster than the player's 2.0s
        money_min: 5, // a visible copper drop so loot is testable (real chickens drop ~0)
        money_max: 20,
        max_level: 0, // no level range for the demo chicken (stays L1)
        max_level_health: 0,
        // Proximity aggro: 8 yards. A creature debug-spawned ~3 yd from the player self-engages,
        // while the seeded chicken at +15 yd stays PASSIVE at login (>8 yd from the start position)
        // — so the login demo stays calm. 0 on every other seeded template.
        aggro_range: 8,
        // The demo chicken never fights back; 0/0 → swing_range_ctx uses the flat fallback (moot, it
        // never swings). Imported creatures carry their real imported melee range.
        damage_min: 0,
        damage_max: 0,
        armor: 0,              // unmitigated — a demo critter needs no armor
        pickpocket_loot_id: 0, // not imported — the demo chicken has no pickpocket table
        skin_loot_id: 0,       // not imported — the demo chicken isn't a beast anyway
        trainer_type: 0,       // not a trainer
        trainer_class: 0,
    });

    // The live creature is a game_world_entity row of type Unit. Its GUID must carry HIGHGUID_UNIT
    // (0xF130) in the high bits — and, like cmangos, the entry in bits 24..47 — so the vanilla
    // client treats it as a creature (not a player) and queries it correctly.
    let chicken_guid: u64 = (0xF130_u64 << 48) | ((CHICKEN_ENTRY as u64) << 24) | 1;

    // Persistent spawn record: the source of truth that survives death. The live entity is
    // built from spawn + template via the shared helper, so a respawn is identical to this seed.
    let chicken_spawn = ctx.db.game_creature_spawn().insert(CreatureSpawn {
        guid: chicken_guid,
        entry: CHICKEN_ENTRY,
        map_id: hw::MAP_ID,
        x: hw::X + 15.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        // NOT-ARMED (see `creatures::timer_never`): this fixture inserts its live entity directly
        // below, so nothing here depends on an armed timer, and a past stamp would make every
        // due-time range scan visit this row forever.
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_RANDOM, // the demo chicken loiters near its post (critter)
        respawn_secs: 0,                                  // 0 ⇒ the flat fallback respawn timer
    });
    crate::creatures::insert_creature_entity(
        ctx,
        build_creature_entity(&chicken_spawn, &chicken_tmpl, 0, 0),
    ); // fixed roll → the demo chicken stays L1

    // Two patrol waypoints ~8 yards apart: the chicken walks back and forth. The tick picks
    // the farther waypoint each leg, so two points alone give an oscillation with no extra state.
    for (wx, wy) in [(hw::X + 15.0, hw::Y), (hw::X + 15.0, hw::Y + 8.0)] {
        ctx.db.game_creature_waypoint().insert(CreatureWaypoint {
            id: 0,
            creature_guid: chicken_guid,
            x: wx,
            y: wy,
            z: hw::Z,
        });
    }

    // --- Hand-author the starter weapon definition (entry 25 "Worn Shortsword"). ---
    // The instance (the owned copy) is born per-character at CREATION in `items::grant_starter_item` (first login re-runs it as an idempotent safety net) —
    // it can't be seeded here because its `owner_identity` is unknown until a player binds (the RLS
    // filter on `owner_identity = :sender` would hide an Identity::ZERO row). Static definition only.
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
            // The starter weapon is BoP — vanilla-authentic (class starting gear binds the instant
            // it's granted, at `grant_starter_item`).
            bonding: crate::items::bonding::BIND_ON_PICKUP,
            ..base_item(si::ENTRY, "Worn Shortsword")
        });
    }

    // A second, stronger hand-authored weapon (entry 50 "Tempered Blade") so weapon-damage-in-swing
    // is demonstrable: its 8–12 / 2.6s profile is clearly above the Worn Shortsword's 1–3, so equipping
    // it visibly raises the swing readout. Hand-authored reference data (licensing firewall: never
    // bulk-imported), display 1542 ships in 5875. inventory_type 21 = main-hand, quality 2 = Uncommon.
    // Canonical constructor shared with the fixture-restore path (`tempered_blade_template`, #363) —
    // the reserved-id copy under FIXTURE_BLADE stays in sync with this one by construction now.
    ctx.db
        .game_item_template()
        .insert(tempered_blade_template(50));

    // Multi-item starter loadout (items::grant_starter_item) + chicken loot content. Two more
    // hand-authored templates the login grant drops into the backpack: a cloth chest (51) and a food
    // stack (52). display 1542 ships in 5875 (placeholder icon). class 4 = Armor, 0 = Consumable.
    ctx.db.game_item_template().insert(ItemTemplate {
        class: 4,    // Armor
        subclass: 1, // Cloth
        display_id: 1542,
        quality: 1,        // Common (white)
        inventory_type: 5, // INVTYPE_CHEST
        item_level: 5,
        required_level: 1,
        max_durability: 40,
        buy_price: 200,
        sell_price: 40,
        max_stack: 1,
        bonding: crate::items::bonding::NONE, // plain common gear — unbound/tradeable
        ..base_item(51, "Recruit's Tunic")
    });
    // Canonical constructor shared with the fixture-restore path (`tough_jerky_template`, #363).
    ctx.db.game_item_template().insert(tough_jerky_template(52));

    // --- PROFESSION ITEMS: every profession reagent/product/yield points at a REAL vanilla
    // item, so the crafts/gathers/consumables render real names+icons+stats in the 5875 client:
    //   769  Chunk of Boar Meat (cooking reagent; Chicken loot)
    //   2681 Roasted Boar Meat  (cooked product; level-1 eat-food)
    //   2318 Light Leather      (skinning yield + LW reagent — loot::LEATHER_ENTRY)
    //   7277 Handstitched Leather Bracers (LW product)
    //   2770 Copper Ore         (mining yield + smelt reagent)
    //   2447 Peacebloom         (herbalism yield + alchemy reagent)
    //   118  Minor Healing Potion (alchemy product + consumable)
    //   2589 Linen Cloth        (first-aid + tailoring reagent)
    //   1251 Linen Bandage      (first-aid product + consumable)
    //   2996 Bolt of Linen Cloth (tailoring product)
    //   2840 Copper Bar         (smelt product + blacksmith reagent)
    //   2862 Rough Sharpening Stone (blacksmith product)
    // All 12 exist in game_item_template from the import, so NO item_template seed/INSERT is needed here.
    // (Linen Cloth/Copper Ore have no loot source yet — granted via debug_grant_item or the gather node for
    // the verify; relocating reagents onto real loot/vendor sources is a separate node-placement pass.)

    // --- SKINNING: a dedicated SKINNABLE BEAST so the skin verify is IMPORT-INDEPENDENT (the demo Chicken
    // is creature_type 8 = Critter → not skinnable). "Test Wolf" (entry 51000): creature_type 1 (BEAST),
    // creature_family 1 (Wolf), faction 14 (Monster, hostile — a usable kill target like the chicken),
    // spawned near the player start. `debug_kill_nearest(killer, 51000)` makes a beast corpse, then
    // `debug_skin_nearest(killer)` skins it → 1× Light Leather + Skinning 1→2.
    // LEVEL 1 is intentional: the skill gate is (creature_level - 1) * 10, so a level-1 beast requires
    // skill 0 — a freshly-trained skinner (skill=1) can skin it immediately without needing debug_set_skill.
    // INIT-ONLY: the template is SQL-seedable post-publish; the spawn is NOT (its respawn_at/despawn_at
    // are Timestamps) — the parent re-imports OR `debug_spawn_at_feet(guid, 51000)` materializes a live
    // wolf. type_flags 0x100 = SKINNABLE (the skin gate keys on creature_type==1 alone; the flag is
    // carried for data parity).
    // Canonical constructor shared with the fixture-restore path (`test_wolf_template`, #363) —
    // the post-import restore reducer builds the same row from the same fn, so a shard restored
    // after an ETL wipe can never drift from a freshly-published one again.
    let wolf_tmpl = ctx.db.game_creature_template().insert(test_wolf_template());
    let wolf_guid: u64 = (0xF130_u64 << 48) | ((TEST_WOLF_ENTRY as u64) << 24) | 1;
    let wolf_spawn = ctx.db.game_creature_spawn().insert(CreatureSpawn {
        guid: wolf_guid,
        entry: TEST_WOLF_ENTRY,
        map_id: hw::MAP_ID,
        x: hw::X + 10.0,
        y: hw::Y + 5.0,
        z: hw::Z,
        orientation: 0.0,
        respawn_at: crate::creatures::timer_never(ctx), // not armed — the live entity is inserted directly below
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_RANDOM,
        respawn_secs: 0, // 0 ⇒ the flat fallback respawn timer
    });
    crate::creatures::insert_creature_entity(
        ctx,
        build_creature_entity(&wolf_spawn, &wolf_tmpl, 0, 0),
    );

    // --- PROFESSION TRAINER: a dedicated trainer NPC so LEARN-A-PROFESSION is verifiable on a
    // NO-IMPORT dev DB (no cmangos cooking/skinning trainer is reliably seeded — the importer's
    // npc_trainer path is class-only + doesn't tag professions). "Profession Trainer" (entry
    // 51001): npc_flags = GOSSIP|TRAINER (0x11) so the trainer-window opens, faction 35 (FRIENDLY — a
    // trainer you walk up to, NOT a combat target), spawned near the player start so it's in interaction
    // range. INIT-ONLY: the template is SQL-seedable post-publish; the SPAWN is NOT (Timestamps) — the
    // parent re-imports OR `debug_spawn_at_feet(guid, 51001)` materializes a live trainer. The
    // profession-learn OFFERINGS (50080→185 Cooking, 50081→393 Skinning, 50082→165 Leatherworking,
    // 50085→171 Alchemy, 50086→129 First Aid, 50087→197 Tailoring, 50088→164 Blacksmithing —
    // Smelting rides Mining, NO offering) are NOT seeded here — `game_trainer_spell` is populated for
    // this entry by the world-import ETL instead.
    // Canonical constructor shared with the fixture-restore path (`profession_trainer_template`,
    // #363) — same drift-proofing as the Test Wolf above.
    let trainer_tmpl = ctx
        .db
        .game_creature_template()
        .insert(profession_trainer_template());
    let trainer_guid: u64 = (0xF130_u64 << 48) | ((PROFESSION_TRAINER_ENTRY as u64) << 24) | 1;
    let trainer_spawn = ctx.db.game_creature_spawn().insert(CreatureSpawn {
        guid: trainer_guid,
        entry: PROFESSION_TRAINER_ENTRY,
        map_id: hw::MAP_ID,
        x: hw::X - 5.0,
        y: hw::Y + 5.0,
        z: hw::Z,
        orientation: 0.0,
        respawn_at: crate::creatures::timer_never(ctx), // not armed — the live entity is inserted directly below
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE, // a trainer stands at its post
        respawn_secs: 0,                                // 0 ⇒ the flat fallback respawn timer
    });
    crate::creatures::insert_creature_entity(
        ctx,
        build_creature_entity(&trainer_spawn, &trainer_tmpl, 0, 0),
    );

    // The Hearthstone (entry 6948) — every character starts with one (granted in `grant_starter_item`);
    // using it recalls to the bound home (`Character::home_*`). Real vanilla values: class 15 (Misc),
    // display 6418 (the hearthstone icon, ships in 5875), unsellable, non-stacking, no stats.
    // spellid_1/spelltrigger_1 (#387) name its on-use spell — "Call Stone" (50119, seeded in
    // `seed_spell_registry` below), trigger 0 (on-use). `apply_item_use` now reads spellid_1 as the
    // single on-use authority for EVERY item, so this is what makes the Hearthstone usable at all;
    // the old hardcoded entry-id special case in items::ops is retired.
    ctx.db.game_item_template().insert(ItemTemplate {
        class: 15, // Miscellaneous
        display_id: 6418,
        quality: 1, // Common
        item_level: 1,
        required_level: 1,
        max_stack: 1,
        spellid_1: 50119,  // "Call Stone" — E_RECALL_HOME, seeded below
        spelltrigger_1: 0, // on-use
        // Real vanilla Hearthstone: unique + BoP the instant it's granted (starter kit source).
        bonding: crate::items::bonding::BIND_ON_PICKUP,
        ..base_item(constants::starter_item::HEARTHSTONE_ENTRY, "Hearthstone")
    });

    // A hand-authored SHIELD ("Battered Buckler") so shield-block is reachable on a NO-IMPORT dev DB
    // (in production the importer maps real `block_value` for every vanilla shield). Entry 50053 is a
    // synthetic fixture ID above the vanilla item range (max ~24k) so it never collides with an imported
    // item — unlike the low entries 25/50/51/52, which the importer's `DELETE WHERE entry>0` + reload
    // shadow with the real vanilla items at those IDs. Its 25 base block_value fully covers a normal
    // creature swing (1–3), making a blocked hit's 0-damage "full block" clearly demonstrable. class 4 =
    // Armor, subclass 6 = Shield, inventory_type 14 = INVTYPE_SHIELD → equips into the OFF-HAND (16).
    ctx.db.game_item_template().insert(ItemTemplate {
        class: 4,           // Armor
        subclass: 6,        // Shield
        display_id: 1542,   // placeholder icon (ships in 5875)
        quality: 1,         // Common (white)
        inventory_type: 14, // INVTYPE_SHIELD → off-hand
        item_level: 5,
        required_level: 1,
        max_durability: 60,
        buy_price: 300,
        sell_price: 60,
        max_stack: 1,
        block_value: 25, // flat block: fully absorbs a normal creature swing → a clean "full block"
        bonding: crate::items::bonding::NONE, // plain common gear — unbound/tradeable
        ..base_item(50053, "Battered Buckler")
    });

    // Chicken (620) loot table: always drops Tough Jerky (52), and 50% of the time a Recruit's
    // Tunic (51). A creature with no game_creature_loot rows drops no items.
    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: CHICKEN_ENTRY,
        item_entry: 52,
        chance_bp: 10000, // always (100%)
        count: 1,
        group_id: 0, // independent roll
        quest_only: false,
    });
    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: CHICKEN_ENTRY,
        item_entry: 51,
        chance_bp: 5000, // 50%
        count: 1,
        group_id: 0,
        quest_only: false,
    });
    // COOKING reagent drop: the Chicken always drops 1× Chunk of Boar Meat (real item 769) so the cooking
    // loop's reagent is obtainable via the existing loot path (kill → take_loot → reagent in the backpack).
    // Independent roll, always (the reagent must be reliably available for the verify recipe). The chicken
    // is a PLACEHOLDER source; relocating the meat onto a real boar is a separate node-placement pass.
    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: CHICKEN_ENTRY,
        item_entry: 769, // Chunk of Boar Meat (the cooking reagent, real imported item)
        chance_bp: 10000, // always (100%)
        count: 1,
        group_id: 0, // independent roll
        quest_only: false,
    });

    // --- Gameobjects: a CHEST (loot) + a GOOBER (quest-use) by the player spawn so they're in view and
    // the harness can exercise use_gameobject. Synthetic entries above the vanilla GO range so they can
    // never collide with imported GOs. Deliberate simplification: the chest drops a single item
    // (Tough Jerky 52); display ids are 5875 placeholders. GO guids carry HIGHGUID_GAMEOBJECT (0xF110 in
    // bits 48..63 — like the corpse 0xF101 / item 0x4000 scheme) so they never collide with other guids.
    const GO_HIGH: u64 = 0xF110 << 48;
    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: 50100,
            type_id: crate::gameobject::go_type::CHEST,
            display_id: 259, // placeholder chest model (ships in 5875)
            name: "Battered Chest".to_string(),
            data0: 52, // drops Tough Jerky (52), a seeded item template
            data1: 0,
            gather_skill_line: 0, // not a gather node
            respawn_secs: 0, // n/a (a CHEST has no respawn timer); 0 ⇒ the 3-min fallback if ever used
            gather_gray: 0,  // n/a (not a gather node) — the always-skill sentinel
            lock_id: 0,      // work-item 211: unlocked (seed/demo chest)
            size: 0.0,       // no dump size — the gateway renders this at 1.0
        });
    ctx.db.game_gameobject().insert(GameObject {
        guid: GO_HIGH | 1,
        template_entry: 50100,
        map_id: hw::MAP_ID,
        x: hw::X + 5.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0, // a freshly-seeded node is ready (no pending respawn)
        instance_id: 0,       // seeded demo GOs live in the open world (190 slice 2),
        grid_x: lyracore_shared::spatial::grid_cell(hw::X + 5.0, hw::Y).0,
        grid_y: lyracore_shared::spatial::grid_cell(hw::X + 5.0, hw::Y).1,
        cell: lyracore_shared::spatial::cell_id_at(hw::X + 5.0, hw::Y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0, // seed fixtures orient via `orientation` only; codec derives yaw (#515)
    });
    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: 50101,
            type_id: crate::gameobject::go_type::GOOBER,
            display_id: 259, // placeholder
            name: "Suspicious Lever".to_string(),
            data0: 0,
            data1: 0,
            gather_skill_line: 0, // not a gather node
            respawn_secs: 0,      // n/a (a GOOBER has no respawn timer)
            gather_gray: 0,       // n/a (not a gather node)
            lock_id: 0,           // work-item 211: unlocked (seed/demo goober)
            size: 0.0,            // no dump size — the gateway renders this at 1.0
        });
    ctx.db.game_gameobject().insert(GameObject {
        guid: GO_HIGH | 2,
        template_entry: 50101,
        map_id: hw::MAP_ID,
        x: hw::X + 8.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0, // a freshly-seeded node is ready (no pending respawn)
        instance_id: 0,       // seeded demo GOs live in the open world (190 slice 2),
        grid_x: lyracore_shared::spatial::grid_cell(hw::X + 8.0, hw::Y).0,
        grid_y: lyracore_shared::spatial::grid_cell(hw::X + 8.0, hw::Y).1,
        cell: lyracore_shared::spatial::cell_id_at(hw::X + 8.0, hw::Y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0, // seed fixtures orient via `orientation` only; codec derives yaw (#515)
    });

    // --- GATHER nodes: a Copper Vein (MINING) + a Peacebloom (HERBALISM) by the player spawn so the
    // harness can exercise the gather path. `type_id: GATHER`, `data0`: the granted item entry, `data1`: the
    // required skill level (1 → a just-learned 1/75 skill passes immediately), `gather_skill_line`: which skill
    // the use requires. INIT-ONLY: `game_gameobject` carries a Timestamp so a spawn can NOT be SQL-inserted —
    // after a `-c` reprovision the parent uses `debug_spawn_gameobject` (with the data1/skill_line args) instead.
    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: 50102,
            type_id: crate::gameobject::go_type::GATHER,
            display_id: 259, // placeholder model (ships in 5875)
            name: "Copper Vein".to_string(),
            data0: 2770, // grants Copper Ore (real imported item)
            data1: 1,    // required Mining skill level (also the "orange" skill-up floor)
            gather_skill_line: crate::skill::skill_line::MINING, // 186
            respawn_secs: 0, // 0 ⇒ the 3-min RESPAWN_WINDOW_MICROS fallback
            gather_gray: 0, // 0 ⇒ the always-skill sentinel (deterministic +1 every gather)
            lock_id: 0,  // work-item 211: gather nodes don't source a lockId this slice
            size: 0.0,   // no dump size — the ETL carries the real one
        });
    ctx.db.game_gameobject().insert(GameObject {
        guid: GO_HIGH | 3,
        template_entry: 50102,
        map_id: hw::MAP_ID,
        x: hw::X + 6.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0, // a freshly-seeded node is ready (no pending respawn)
        instance_id: 0,       // seeded demo GOs live in the open world (190 slice 2),
        grid_x: lyracore_shared::spatial::grid_cell(hw::X + 6.0, hw::Y).0,
        grid_y: lyracore_shared::spatial::grid_cell(hw::X + 6.0, hw::Y).1,
        cell: lyracore_shared::spatial::cell_id_at(hw::X + 6.0, hw::Y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0, // seed fixtures orient via `orientation` only; codec derives yaw (#515)
    });
    ctx.db
        .game_gameobject_template()
        .insert(GameObjectTemplate {
            entry: 50103,
            type_id: crate::gameobject::go_type::GATHER,
            display_id: 259, // placeholder model
            name: "Peacebloom".to_string(),
            data0: 2447, // grants Peacebloom (real imported herb)
            data1: 1,    // required Herbalism skill level (also the "orange" skill-up floor)
            gather_skill_line: crate::skill::skill_line::HERBALISM, // 182
            respawn_secs: 0, // 0 ⇒ the 3-min RESPAWN_WINDOW_MICROS fallback
            gather_gray: 0, // 0 ⇒ the always-skill sentinel (deterministic +1 every gather)
            lock_id: 0,  // work-item 211: gather nodes don't source a lockId this slice
            size: 0.0,   // no dump size — the ETL carries the real one
        });
    ctx.db.game_gameobject().insert(GameObject {
        guid: GO_HIGH | 4,
        template_entry: 50103,
        map_id: hw::MAP_ID,
        x: hw::X + 7.0,
        y: hw::Y,
        z: hw::Z,
        orientation: 0.0,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0, // a freshly-seeded node is ready (no pending respawn)
        instance_id: 0,       // seeded demo GOs live in the open world (190 slice 2),
        grid_x: lyracore_shared::spatial::grid_cell(hw::X + 7.0, hw::Y).0,
        grid_y: lyracore_shared::spatial::grid_cell(hw::X + 7.0, hw::Y).1,
        cell: lyracore_shared::spatial::cell_id_at(hw::X + 7.0, hw::Y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0, // seed fixtures orient via `orientation` only; codec derives yaw (#515)
    });

    // --- TIER-VARIETY DEMONSTRATOR (gather multinodes): an IN-PLACE Copper point that ~15% of the time
    // presents Tin instead — the authentic cmangos "multinodes subzone" pattern ("a node might spawn a
    // higher tier"). ONE pool, max_active 1, with TWO CO-LOCATED members (identical x,y,z) of
    // differing TIERS: Copper Vein (1731, item Copper Ore 2770, Mining req 1, weight 85) and Tin Vein
    // (1732, item Tin Ore 2771, Mining req 65, weight 15). `arm_pool` weighted-picks one → spawns that tier
    // here; on gather the point flips+arms its timer and pass_gameobject_respawn weighted-RE-ROLLS the tier
    // IN PLACE (co-located ⇒ no wander, repeats allowed ⇒ Copper recurs). A low char (Mining < 65) is
    // skill-gated off a rolled Tin by the existing `can_gather` — the "richer node you can't tap yet" tease.
    // REAL entries 1731/1732 (NOT the synthetic 50102 Copper Vein above): both type 25 GATHER, line 186
    // MINING. INIT-ONLY (the live pool/member rows are made here + arm); a re-import (`DELETE FROM
    // game_gameobject_pool WHERE pool_id > 0`) wipes this pool, so on the live/imported DB it is re-seeded
    // post-import via `debug_setup_gather_pool 2 1 true ...`. pool_id 2
    // is distinct from the debug pool (1) and the importer's roaming base (1000). Ensure the two tier
    // templates exist first (idempotent — the bare seed lacks them; the ETL also loads them).
    for (e, name, item, req) in [
        (1731u32, "Copper Vein", 2770u32, 1u32),
        (1732u32, "Tin Vein", 2771u32, 65u32),
    ] {
        if ctx.db.game_gameobject_template().entry().find(e).is_none() {
            ctx.db
                .game_gameobject_template()
                .insert(GameObjectTemplate {
                    entry: e,
                    type_id: crate::gameobject::go_type::GATHER,
                    display_id: 259, // placeholder model (ships in 5875); the importer carries the real one
                    name: name.to_string(),
                    data0: item, // the granted ore (real imported item)
                    data1: req, // required Mining level (Copper 1 / Tin 65 — the skill-gate teaser)
                    gather_skill_line: crate::skill::skill_line::MINING, // 186
                    respawn_secs: 300, // real vanilla mining-node window (5 min); reroll fires at timer-fire
                    gather_gray: 0,    // always-skill sentinel (deterministic +1 every gather)
                    lock_id: 0, // work-item 211: gather nodes don't source a lockId this slice
                    size: 0.0,  // no dump size — the gateway renders this at 1.0
                });
        }
    }
    const TIER_POOL_ID: u32 = 2;
    ctx.db.game_gameobject_pool().insert(GameObjectPool {
        pool_id: TIER_POOL_ID,
        max_active: 1,  // one live node at the point at a time
        in_place: true, // IN-PLACE tier re-roll (NOT a roaming pool) — gather re-rolls the tier here
    });
    // Two CO-LOCATED members (identical x,y,z,o) — they differ ONLY in template_entry/weight, so the
    // weighted roll changes the TIER, never the position. Real Goldshire-area Copper coord, hand-placed.
    for (entry, weight) in [(1731u32, 85u32), (1732u32, 15u32)] {
        ctx.db
            .game_gameobject_pool_member()
            .insert(GameObjectPoolMember {
                point_id: 0, // auto_inc
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
    // ARM: insert exactly max_active (1) weighted-distinct live rows → a rolled tier is live from init.
    crate::gameobject::arm_pool(ctx, TIER_POOL_ID);

    // --- Weather climate for Elwynn Forest and Westfall ---------------------------------------
    // Hand-authored and TEMPORARY: the world-data import will fill `game_weather` from the cmangos
    // dump and delete this seed in the same change. Idempotent + shared with
    // `debug_repair_after_publish`, which is how an already-migrated database picks it up (init does
    // NOT re-run).
    crate::weather::seed_weather_weights(ctx);
}

/// Stratum 3 — the hand-authored spell/item registry: `game_spell`/`game_spell_effect` rows, the
/// crafted-consumable on-use spells, 1-10-alpha consumable breadth, the mock-seed fixture kits
/// (`seed/fixtures.rs`), enchant/disenchant, talents, and the stacking-group starter set.
fn seed_spell_registry(ctx: &ReducerContext) {
    // --- Spell registry: hand-authored `game_spell` headers + `game_spell_effect` rows (the
    // data-driven effect-row engine). Each spell = a header + 1..3 effect rows; effect.id is the
    // DETERMINISTIC (spell_id<<2)|effect_index. Source of truth for fresh installs; an auto-migrate
    // publish keeps the tables, so the main thread SQL-seeds these rows. (Licensing firewall: curated,
    // never bulk-imported; the Spell.dbc importer is a later workstream.) Two local closures keep the
    // 18/17-field literals from drowning the seed — most columns are defaults (0/false/0.0).
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
            spell_id,
            name: name.to_string(),
            power_type,
            cost,
            cast_time_ms,
            gcd_ms: 1500,
            family_name: 0,
            family_flags: 0,
            cooldown_ms: 0,
            range_yd,
            duration_ms,
            school_mask,
            dispel_type,
            mechanic: 0,
            max_stacks,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative,
            cast_flags: 0,
            stances: 0, // seeded spells have no stance requirement (usable in any stance)
            proc_flags: 0, // seeded spells carry no Proc data
            proc_chance: 0,
            proc_charges: 0,
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
            id: ((spell_id as u64) << 2) | idx as u64,
            spell_id,
            effect_index: idx,
            kind,
            base_points,
            die_sides: 0,
            per_level: 0.0,
            period_ms,
            target,
            radius_yd: 0.0,
            chain_targets: 0,
            trigger_spell: 0,
            effect_mechanic: 0,
            p0,
            p0_kind,
            p1: 0,
            script_id: 0,
            enters_combat: false,
        });
    };
    // kind: 0x01 E_DAMAGE, 0x02 E_HEAL, 0x04 E_DISPEL; 0x90 A_PERIODIC_DAMAGE, 0x91 A_PERIODIC_HEAL,
    // 0xA3 A_MOD_COMBAT, 0xBE A_FLAG. target: 0 T_SELF, 1 T_TARGET_ENEMY, 2 T_TARGET_ALLY, 3 T_TARGET_ANY.
    // p0_kind: 5 P_COMBAT_FIELD, 7 P_FLAG. p0 for A_MOD_COMBAT = 0 COMBAT_ATTACK_POWER.
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
    effect(constants::tracer_spell::SPELL_ID, 0, 0xA3, 30, 0, 0, 0, 5); // +30 AP self-buff
                                                                        // Rend (Warrior rank 1, spell 772) — a physical BLEED: costs 10 RAGE (exercises the cost gate)
                                                                        // and applies a 21s DoT (7 dmg / 3s) to an enemy via the A_PERIODIC_DAMAGE engine. Physical
                                                                        // school (1) → no magic resist, so the bleed ignores armor, exactly like vanilla. Melee range
                                                                        // (5yd), instant, non-dispellable (bleeds).
    spell(772, "Rend", 1, 10, 0, 5, 21000, 1, 0, true, 0);
    effect(772, 0, 0x90, 7, 3000, 1, 0, 0); // bleed 7 dmg / 3s on the target enemy (T_TARGET_ENEMY)
    spell(2050, "Lesser Heal", 0, 30, 1500, 40, 0, 2, 0, false, 0);
    effect(2050, 0, 0x02, 50, 0, 0, 0, 0); // heal self 50
    spell(133, "Fireball", 0, 0, 0, 30, 0, 4, 0, false, 0);
    effect(133, 0, 0x01, 20, 0, 1, 0, 0); // 20 fire dmg to an enemy (lethal -> kill_creature)
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
    effect(11196, 0, 0xBE, 0, 0, 3, 0, 7); // a flag debuff aura on the target
    spell(980, "Curse of Agony", 0, 0, 0, 30, 24000, 32, 2, true, 0);
    effect(980, 0, 0x90, 5, 1000, 1, 0, 0); // DoT 5/1s on an enemy
    spell(139, "Renew", 0, 0, 0, 40, 15000, 2, 1, false, 0);
    effect(139, 0, 0x91, 8, 1000, 2, 0, 0); // HoT 8/1s on an ally
    spell(527, "Dispel Magic", 0, 0, 0, 30, 0, 1, 0, false, 0);
    effect(527, 0, 0x04, 0, 0, 3, 0, 0); // strip debuffs off the target
                                         // Mark of the Wild (1126) — a MULTI-EFFECT buff: ONE spell, THREE ordered A_MOD_STAT (0xA0) effects
                                         // (one cast → three typed aura snapshots, exercising the one→many effect list). Only the
                                         // STR effect is CONSUMED today: it folds into effective Strength (combat::swing_range_ctx → a higher
                                         // swing, server-verifiable via debug_compute_swing). The +AGI/+STA effects are typed aura rows STAGED
                                         // for derive hooks that don't exist yet (AGI→dodge, STA→max-health) — present in data, inert in
                                         // gameplay. Magnitudes are illustrative spike values (real rank-1 MotW is +3 to all five
                                         // attributes); the importer supersedes these. p0_kind 1 = P_STAT_ID.
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
    effect(1126, 0, 0xA0, 5, 0, 0, 0, 1); // +5 Strength (STAT_STR), self
    effect(1126, 1, 0xA0, 5, 0, 0, 1, 1); // +5 Agility  (STAT_AGI), self
    effect(1126, 2, 0xA0, 5, 0, 0, 2, 1); // +5 Stamina  (STAT_STA), self
                                          // Inner Fire (588) — an ARMOR buff: a single A_MOD_RESISTANCE (0xA1) effect whose p0 is the school
                                          // MASK with the armor bit (RESIST_ARMOR 0x01) set. Folds into effective armor (combat::effective_armor)
                                          // so the buffed unit mitigates more physical damage — server-verifiable via debug_compute_swing's
                                          // mitigation_pct. Amount is an illustrative spike value (real Inner Fire is small); p0_kind 2 = P_SCHOOL_MASK.
    spell(588, "Inner Fire", 0, 30, 0, 0, 600000, 2, 1, false, 1);
    effect(588, 0, 0xA1, 2000, 0, 0, 1, 2); // +2000 armor (RESIST_ARMOR mask), self

    // Test Fire Ward (50030) — a MAGIC-resistance buff: a single A_MOD_RESISTANCE (0xA1) effect whose p0
    // is the FIRE school MASK (4, p0_kind 2 P_SCHOOL_MASK), so it folds into fire-school resistance (NOT
    // armor — bit 0 is masked out in apply_resistance). A fire-school E_DAMAGE spell (e.g. Fireball 133,
    // school 4) reads it via resistance_bonus(.., 4) and reduces the hit by combat::resist_mitigation_pct.
    // target 2 (T_TARGET_ALLY) so a debug cast can place it on any chosen unit; is_negative false (a ward).
    spell(50030, "Test Fire Ward", 0, 0, 0, 30, 600000, 4, 0, false, 1);
    effect(50030, 0, 0xA1, 6, 0, 2, 4, 2); // +6 fire resistance (FIRE mask 4), ally; p0_kind 2 = P_SCHOOL_MASK

    // Combat Insight (50000) — a synthetic CRIT/HIT-rating buff: ONE spell, TWO ordered A_MOD_COMBAT
    // (0xA3) effects whose p0 names the combat FIELD (COMBAT_CRIT 1 / COMBAT_HIT 2, p0_kind 5
    // P_COMBAT_FIELD — the same shape as Battle Shout's COMBAT_ATTACK_POWER effect, just a different
    // field). Both fold into the melee attack table (combat::effective_crit_bp/effective_miss_bp): the
    // CRIT effect RAISES the crit band, the HIT effect REDUCES the miss band — server-verifiable via
    // debug_compute_swing's crit_bp/hit_miss_bp. Magnitudes are illustrative spike values (+10% crit,
    // +5% hit, in basis points). Unused spell id reserved for this combat-stat test buff.
    spell(50000, "Combat Insight", 0, 0, 0, 0, 600000, 1, 0, false, 1);
    effect(50000, 0, 0xA3, 1000, 0, 0, 1, 5); // +1000 crit (COMBAT_CRIT), self → +10% crit
    effect(50000, 1, 0xA3, 500, 0, 0, 2, 5); // +500 hit (COMBAT_HIT), self → -5% miss

    // Quickening (50010) — a synthetic melee-HASTE buff: ONE A_MOD_SPEED(0xA4) effect whose p0 names
    // SPEED_SWING (1, p0_kind 6 P_SPEED_KIND); amount is the signed speed PERCENT. Folds into the swing
    // timer (combat::effective_swing_time) so the unit attacks faster — server-verifiable via
    // debug_compute_swing's attack_time_ms. +50% → a 2.0s swing becomes ~1.33s. A_MOD_SPEED is the ONE
    // swing-speed model (the same convention the importer emits).
    spell(50010, "Quickening", 0, 0, 0, 0, 600000, 1, 0, false, 1);
    effect(50010, 0, 0xA4, 50, 0, 0, 1, 6); // +50% melee haste (A_MOD_SPEED, SPEED_SWING), self

    // Test Snare (50011) — a synthetic move-SNARE debuff (Hamstring-shaped): ONE A_MOD_SPEED(0xA4) effect
    // whose p0 names SPEED_MOVE (0, p0_kind 6 P_SPEED_KIND); amount is the signed speed PERCENT (negative =
    // slower). Folds into combat::effective_move_speed so a snared CREATURE chases/returns/wanders slower
    // (server-verifiable via its per-tick position delta). −40% → effective RUN 7.0→4.2 yd/s. target 1
    // (T_TARGET_ENEMY) so a debug cast lands it on a mob; is_negative true. (Player snares additionally
    // need a SMSG_FORCE_RUN_SPEED_CHANGE wire push — deferred; this governs server-driven movement.)
    spell(50011, "Test Snare", 0, 0, 0, 30, 600000, 1, 0, true, 1);
    effect(50011, 0, 0xA4, -40, 0, 1, 0, 6); // −40% move speed (A_MOD_SPEED, SPEED_MOVE), enemy

    // Test Conjure (50050) — a CreateItem (conjure) fixture: ONE E_CREATE_ITEM (0x07) instant effect that
    // mints `base_points` of item p0 into the CASTER's backpack (the same items::grant_item a quest reward
    // uses). Self-target (0); p0 = 5349 "Conjured Muffin" (a REAL vanilla conjure item from the full
    // item_template import), p0_kind = 8 P_ITEM_ENTRY, count = 2. Real Mage conjure /
    // quest CreateItem spells import to this same kind and mint their real items the same way.
    spell(50050, "Test Conjure", 0, 0, 0, 0, 0, 1, 0, false, 0);
    effect(50050, 0, 0x07, 2, 0, 0, 5349, 8); // E_CREATE_ITEM: 2× item 5349 (Conjured Muffin), self

    // Craft RECIPES are no longer seeded (work-item 282): they import from the real Spell.dbc with real
    // reagents (game_spell_reagent) + skill-up bands (game_skill_ability), offered by the real in-box
    // trainers. The old synthetic recipe spells here (2538 — which was even FABRICATED as "Roasted Boar
    // Meat" when the real 2538 is "Charred Wolf Meat"; 50071; 50090-50097) are gone. Crafted-item ON-USE
    // effects (50110-50118 below) stay — those are our alpha on-use behaviour for the real items the real
    // recipes produce, with no DBC replacement yet.

    // Mock-seed fixture kits (see seed/fixtures.rs for each kit's full rationale). Every kit is
    // idempotent and shared with its `debug_seed_*` reducer twin: init does NOT re-run on an
    // auto-migrate publish, so an already-migrated dev DB re-seeds via the debug reducer (same
    // precedent as `talent::seed_talents`/`debug_seed_talents`).
    seed_pw_shield_fixture(ctx); // Weakened Soul (6788) + Test PW:Shield (50072) — linked-debuff mechanic
                                 // issue #85: the scenario-fixture items (Tempered Blade/Tough Jerky) + fixture faction 50900 land
                                 // in fingerprinted catalogue tables (game_item_template/game_faction) — seed them here too, not
                                 // only from debug_seed_scenario_fixtures, so every fresh shard agrees regardless of whether the
                                 // wire-suite harness ever ran against it (see seed::fixtures::seed_fixture_catalogue's doc).
    seed_fixture_catalogue(ctx);
    seed_hunter_tame_fixture(ctx); // Hunter + completed tame + tameable boar tracer fixture
    seed_stacking_probe_fixture(ctx); // the live stacking-family probe's four family members
    seed_soul_shard_item(ctx); // Soul Shard item template (6265)
    seed_drain_soul_fixture(ctx); // Drain Soul (1120) channel — soul-shard generation
    seed_frost_armor_fixture(ctx); // Chilled (6136) + Frost Armor (168) — the Proc engine's live user
    seed_test_proc_fixtures(ctx); // Test Proc Mark/Coin/Charges/Cooldown/PPM/Zap (50140-50145) — the Proc chance, charge, cooldown, rate + damage fixtures
    seed_mana_burn_fixture(ctx); // Mana Burn (8129) — E_POWER_BURN drain-mana-into-damage
    seed_demon_skin_fixture(ctx); // Demon Skin (696 rank 2) — combat-independent health-per-5 tick
    seed_regen_fixture(ctx); // Test Regeneration (50137) — the combat-regen probe's kind-169 source
                             // issue #85 audit: this one was previously reachable ONLY via `debug_seed_stealth_fixture` (never
                             // from init), the same divergence-hazard shape #85 fixed for items/faction — see
                             // `seed::fixtures::seed_stealth_fixture`'s doc.
    seed_stealth_fixture(ctx); // Stealth (1784) — A_STEALTH presence marker
    seed_mount_fixture(ctx); // Test Riding Horse (50310) + Test Dazed (50311) + the Riding skill data

    // Enchant / Disenchant — the ITEM-target enchanting spells. These never run through
    // resolve_cast (they target an item GUID, not a unit); the GATEWAY intercepts CMSG_CAST_SPELL, reads
    // these effect rows, resolves the item GUID→bag slot, and calls enchant_item_on_slot / disenchant_item.
    // The effect row is the ROUTING CLASSIFIER: kind 0x17 E_ENCHANT_ITEM (p0 = enchant_id, p0_kind 10
    // P_ENCHANT_ID — the gateway reads enchant_id off p0) / kind 0x18 E_DISENCHANT (no params). The enchant
    // stat overlay (enchant_id→stat) lives in the module ENCHANTS table; disenchant reagents/skill in the
    // disenchant reducer. A NEW enchant is a data row here (a new id + p0), ZERO gateway code. target 0 is
    // inert (the gateway resolves the item from the cast packet, not the effect target).
    spell(
        50201,
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
    effect(50201, 0, 0x17, 0, 0, 0, 7745, 10); // E_ENCHANT_ITEM: enchant_id 7745 (+3 STR), p0_kind P_ENCHANT_ID
    spell(
        50202,
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
    effect(50202, 0, 0x17, 0, 0, 0, 7748, 10); // E_ENCHANT_ITEM: enchant_id 7748 (+3 STA), p0_kind P_ENCHANT_ID
    spell(13262, "Disenchant", 0, 0, 0, 0, 0, 0, 0, false, 0);
    effect(13262, 0, 0x18, 0, 0, 0, 0, 0); // E_DISENCHANT: no params (module validates + yields dust by item)

    // The BOMB'S ON-USE AoE (50096) — the crafted Rough Copper Bomb (real item 4360, made by the REAL
    // engineering recipe now) casts this on use via USE_EFFECTS. A single E_DAMAGE effect with target
    // T_AREA_ENEMY (4) → the cast engine fans the hit out to every in-radius hostile (aoe_keep /
    // AOE_MAX_TARGETS); radius_yd 0 → the 8yd MELEE_PBAOE_RADIUS_YD splash. is_negative true, school 4.
    // (Synthetic 50096 rather than the dump's real 4064 — 4064 isn't in our curated game_spell; swap the
    // USE_EFFECTS mapping once it imports.) Note: the spell() closure defaults gcd_ms=1500 and the
    // bomb KEEPS it (a thrown bomb shares the global cooldown, vanilla-correct) — do NOT "fix" this to 0.
    spell(50096, "Rough Copper Bomb", 0, 0, 0, 0, 0, 4, 0, true, 0);
    effect(50096, 0, 0x01, 15, 0, 4, 0, 0); // E_DAMAGE 15, T_AREA_ENEMY (4) → 8yd PBAoE fan-out

    // --- CRAFTED-CONSUMABLE ON-USE SPELLS (the items::ops USE_EFFECTS map fires these via begin_cast when
    // the matching consumable is USED — potion/bandage/food). All cost 0 / SELF / item-triggered (gcd_ms 0
    // so an item use shares no GCD with the spell bar), INIT-ONLY (no Timestamp → SQL-seedable
    // post-publish). Magnitudes are DATA placeholders (tune later). ---

    // (1) Minor Healing (50110) — real item 118's on-use: an INSTANT (cast_time 0) self E_HEAL clamped to
    // max by healed_value. school 2 (holy/neutral). base 80 = the vanilla Minor Healing midpoint (70-90;
    // no die-roll plumbing on E_HEAL here, so a fixed 80).
    // item-triggered → NO spell GCD (gcd_ms: 0 overrides base_spell's 1500; potions/food must not
    // trip the spell-bar GCD — the SQL seed already uses 0, this keeps a fresh `-c` DB consistent
    // with it).
    ctx.db.game_spell().insert(Spell {
        school_mask: 2,
        gcd_ms: 0,
        ..base_spell(50110, "Minor Healing")
    });
    effect(50110, 0, 0x02, 80, 0, 0, 0, 0); // E_HEAL 80, T_SELF (id 200440)

    // (2) Linen Bandage (50111) — real item 1251's on-use: a CHANNELED (SPELL_ATTR_CHANNELED 0x0080) HoT
    // that ticks A_PERIODIC_HEAL 8/1s for 8s (= 64 ≈ vanilla's 66 total), breaking early on damage via
    // aura_interrupt bit0 (break_auras_on_damage). A SECOND effect E_TRIGGERs the existing "Recently
    // Bandaged" (11196) 60s debuff so the re-bandage gate (has_aura 11196) can block spam. The `spell`/
    // `effect` closures can't set cast_flags/aura_interrupt/trigger_spell, so this one is a raw literal.
    ctx.db.game_spell().insert(Spell {
        cast_time_ms: 0, // channeled: begin_cast resolves now on the CHANNELED bit, not a cast bar
        gcd_ms: 0,       // item-triggered: no GCD
        range_yd: 0,     // self
        duration_ms: 8000, // 8s channel (vanilla Linen Bandage) → 8 ticks at 1s
        school_mask: 2,  // holy/neutral
        aura_interrupt: 1, // bit0 BREAK_ON_DAMAGE → the HoT drops when the bandaged unit takes damage
        cast_flags: crate::spell::SPELL_ATTR_CHANNELED, // 0x0080 — begin_cast routes the channel branch
        ..base_spell(50111, "Linen Bandage")
    });
    effect(50111, 0, 0x91, 8, 1000, 0, 0, 0); // A_PERIODIC_HEAL 8/1s ×8 = 64 total, T_SELF (id 200444)
                                              // eff1: E_TRIGGER (0x05) the Recently-Bandaged debuff (11196) on SELF — base_effect (the
                                              // closure can't set trigger_spell). id = (50111<<2)|1 = 200445.
    ctx.db.game_spell_effect().insert(SpellEffect {
        kind: 0x05,           // E_TRIGGER
        target: 0,            // T_SELF (the trigger re-casts at the same — self — target)
        trigger_spell: 11196, // existing "Recently Bandaged" 60s A_FLAG debuff (re-bandage gate keys on it)
        ..base_effect(50111, 1)
    });

    // (3) Roasted Boar Meat (real item 2681) is a level-1 food: it EATS to heal (the vital-restore
    // branch), with deliberately NO on-use spell — the Well-Fed stat buff is a higher-level-food
    // mechanic, and granting it on a level-1 food would be non-vanilla (the 5875 client would show
    // an aura the real item never grants).

    // === CONSUMABLE BREADTH (1-10 alpha) — mana potion / drink / over-time food / Well-Fed Cooking buff +
    // the cheap rank-2 potion/bandage. ALL data-only on the existing effect engine (no new engine code).
    // Real vanilla items (USE_EFFECTS in items::ops) cast these via begin_cast; real restore/heal/buff
    // magnitudes, hand-authored to match. The 50113-50118 on-use block + the Well-Fed
    // Cooking recipe 50097. All on-use spells CLEAR gcd_ms to 0 (item use must not lock the spell bar);
    // the 50097 CRAFT keeps the closure's 1500 (a craft shares the GCD, like 2538). Do NOT reuse spell id
    // 50112 — it is a retired id, previously actively deleted by a since-removed seed script; still
    // reserved. ---

    // (The Spiced Wolf Meat recipe 50097 was removed with the other synthetic recipes, 282 — its product
    // item 2680 now comes from the real cooking recipe; its Well-Fed on-use 50116 stays below.)

    // (1) Minor Mana Potion 50113 — instant self mana restore (real item 2455). E_ENERGIZE 160 (Restore
    // Mana 437 = 140-180, midpoint). p0=0 MANA, p0_kind 4 P_POWER_TYPE. MANA-class-gated in apply_item_use.
    ctx.db.game_spell().insert(Spell {
        gcd_ms: 0, // item-triggered: no GCD
        ..base_spell(50113, "Minor Mana Potion")
    });
    effect(50113, 0, 0x03, 160, 0, 0, 0, 4); // E_ENERGIZE 160 (p0=0 MANA, p0_kind 4 P_POWER_TYPE), T_SELF (id 200452)

    // (2) Refreshing Water 50114 — 30s mana-over-time DRINK (real items 159/5350). A_PERIODIC_ENERGIZE 41
    // mana/5s ×6 = 246 over 30s (Drink 430 base 41). MANA-class-gated. p0=0 MANA, p0_kind 4 P_POWER_TYPE.
    ctx.db.game_spell().insert(Spell {
        duration_ms: 30000,
        gcd_ms: 0, // item-triggered: no GCD
        ..base_spell(50114, "Refreshing Water")
    });
    effect(50114, 0, 0x92, 41, 5000, 0, 0, 4); // A_PERIODIC_ENERGIZE 41 mana/5s ×6 = 246 (p0=0 MANA), T_SELF (id 200456)

    // (3) Eating 50115 — 30s health-over-time FOOD (real items 4540/117). A_PERIODIC_HEAL 16/5s ×6 = 96 over
    // 30s (Food 433 base 16). Clamped to max_health each tick. (KEEP legacy 2681 eat-heal untouched.)
    ctx.db.game_spell().insert(Spell {
        duration_ms: 30000,
        gcd_ms: 0, // item-triggered: no GCD
        ..base_spell(50115, "Eating")
    });
    effect(50115, 0, 0x91, 16, 5000, 0, 0, 0); // A_PERIODIC_HEAL 16/5s ×6 = 96 over 30s, T_SELF (id 200460)

    // (4) Well Fed 50116 — the Cooking payoff buff on Spiced Wolf Meat (real item 2680, product of recipe
    // 50097). REAL value: 2680's on-use is spell 5004, which periodic-triggers 19705 "Well Fed" =
    // MOD_STAT +2 Stamina + +2 Spirit, 15-min (DurationIndex 347 = 900000 ms). We apply the +2/+2 directly as
    // the 15-min aura (skipping the periodic-trigger indirection — same net buff). +STA grows max HP (live via
    // recompute_vitals on apply); +SPI is summed by stat_bonus but inert (no Spirit consumer yet).
    ctx.db.game_spell().insert(Spell {
        duration_ms: 900000,
        max_stacks: 1,
        gcd_ms: 0, // item-triggered: no GCD
        ..base_spell(50116, "Well Fed")
    });
    effect(50116, 0, 0xA0, 2, 0, 0, 2, 1); // +2 Stamina (STAT_STA), T_SELF (id 200464)
    effect(50116, 1, 0xA0, 2, 0, 0, 4, 1); // +2 Spirit  (STAT_SPI), T_SELF (id 200465)

    // (5a) Lesser Healing 50117 — instant rank-2 health potion (real item 858). E_HEAL 160 (Healing Potion
    // 440 = 140-180, midpoint), clamped to max by healed_value. Rank-2 of the existing 118/50110.
    ctx.db.game_spell().insert(Spell {
        school_mask: 2,
        gcd_ms: 0, // item-triggered: no GCD
        ..base_spell(50117, "Lesser Healing")
    });
    effect(50117, 0, 0x02, 160, 0, 0, 0, 0); // E_HEAL 160, T_SELF (id 200468)

    // (5b) Heavy Linen Bandage 50118 — channeled rank-2 bandage (real item 2581); raw literal (channeled +
    // E_TRIGGER), mirroring 50111. A_PERIODIC_HEAL 18/1s ×8 = 144 over 8s (First Aid 1159 base 18), breaking
    // early on damage (aura_interrupt bit0). eff1 E_TRIGGERs the SHARED 11196 "Recently Bandaged" cooldown
    // (Gate B widens bandage_cooldown_blocks to 1251|2581 so both bandages share the lockout).
    ctx.db.game_spell().insert(Spell {
        gcd_ms: 0,
        duration_ms: 8000,
        school_mask: 2,
        aura_interrupt: 1,
        cast_flags: crate::spell::SPELL_ATTR_CHANNELED,
        ..base_spell(50118, "Heavy Linen Bandage")
    });
    effect(50118, 0, 0x91, 18, 1000, 0, 0, 0); // A_PERIODIC_HEAL 18/1s ×8 = 144 over 8s, T_SELF (id 200472)
    ctx.db.game_spell_effect().insert(SpellEffect {
        kind: 0x05, // E_TRIGGER
        trigger_spell: 11196,
        ..base_effect(50118, 1)
    }); // id 200473

    // (6) Call Stone 50119 (#387) — the Hearthstone's on-use spell: ONE E_RECALL_HOME (0x1F) instant
    // effect, T_SELF, that teleports the caster to its bound home via `world::recall_to_home`. No
    // cost/cooldown/cast-time (matches the pre-#387 hardcoded path's IMMEDIATE-teleport behavior — the
    // vanilla ~10s cast + 1hr CD is a later follow-up, same as Blink's forward-teleport). Wired onto the
    // Hearthstone template's spellid_1 below (bonding BIND_ON_PICKUP) — `apply_item_use` reads it as
    // ANY other on-use spell now, with one data-driven exception: `spell_keeps_item` (keyed on THIS
    // effect kind, not the item's entry id) skips the stack-consumption every other on-use spell takes,
    // since a recall trinket is never used up. A mount item qualifies through the same predicate.
    ctx.db.game_spell().insert(Spell {
        gcd_ms: 0, // item-triggered: no GCD
        ..base_spell(50119, "Call Stone")
    });
    effect(50119, 0, crate::spell::E_RECALL_HOME, 0, 0, 0, 0, 0); // E_RECALL_HOME, T_SELF (id 200476)

    // Test Stun (50020) — the STUN crowd-control: ONE A_CONTROL (0xB0) effect whose p0 names the
    // MECHANIC M_STUN (1, p0_kind 3 P_MECHANIC), targeting an ENEMY (target 1 T_TARGET_ENEMY). A unit
    // carrying this aura can neither SWING (combat::tick_melee's swing gate) nor ACT/MOVE in creature
    // AI (every tick_creatures pass gates on `is_stunned`/`is_movement_blocked`). duration 600000 (10
    // min) so a forced cast holds long enough to observe across many ticks. is_negative true (a
    // debuff). Magnitude (base_points) is unused by A_CONTROL — the mechanic lives in p0; kept 0.
    spell(50020, "Test Stun", 0, 0, 0, 30, 600000, 1, 0, true, 1);
    effect(50020, 0, 0xB0, 0, 0, 1, 1, 3); // A_CONTROL, p0 = M_STUN (1), p0_kind = P_MECHANIC (3), enemy

    // Test Root (50021) — the ROOT crowd-control: ONE A_CONTROL (0xB0) effect whose p0 names the
    // MECHANIC M_ROOT (2, p0_kind 3 P_MECHANIC), targeting an ENEMY. A unit carrying this aura cannot
    // MOVE (the creature MOVEMENT passes gate on `is_movement_blocked`) but CAN still act — it keeps
    // SWINGING (the swing gate is `is_stunned` only, which root does not trip) and a rooted creature
    // still aggroes/casts. duration 600000; is_negative true. p0 carries the mechanic; base_points 0.
    spell(50021, "Test Root", 0, 0, 0, 30, 600000, 1, 0, true, 1);
    effect(50021, 0, 0xB0, 0, 0, 1, 2, 3); // A_CONTROL, p0 = M_ROOT (2), p0_kind = P_MECHANIC (3), enemy

    // Test Fear (50022) — the FEAR crowd-control: ONE A_CONTROL (0xB0) effect whose p0 names the MECHANIC
    // M_FEAR (3, p0_kind 3 P_MECHANIC), targeting an ENEMY. A feared unit cannot ACT (no swing/cast — the
    // ACTION gates fold fear in) and is force-walked AWAY from the caster by the fear-flee pass each tick
    // ("flees in terror"); it stays engaged so it resumes attacking when the aura ends. SHORT 8s duration
    // (≈2 ticks, like Warlock Fear) — bounded so the test subject doesn't run off the map. aura_interrupt
    // stays 0: base fear does NOT break on damage (unlike polymorph). is_negative true.
    spell(50022, "Test Fear", 0, 0, 0, 30, 8000, 1, 0, true, 1);
    effect(50022, 0, 0xB0, 0, 0, 1, 3, 3); // A_CONTROL, p0 = M_FEAR (3), p0_kind = P_MECHANIC (3), enemy

    // Test Poly (50023, work-item 192) — the POLYMORPH crowd-control: ONE A_CONTROL (0xB0) effect whose p0
    // names the MECHANIC M_POLY (4, p0_kind 3 P_MECHANIC), targeting an ENEMY. `is_incapacitated` gates
    // stun/poly identically (no act, no move) — this fixture exists so CC DIMINISHING RETURNS has a real,
    // debug-castable spell to drive the live-probe runbook (two poly casts on a player target 15s apart or
    // less land at 100/50/25/0%; the same double-cast on a CREATURE target is always full duration — see
    // `spell::stacking`'s DR resolver + the work-item's completion note). 10s duration matches the
    // pure-fn DR test vector's base duration exactly (10s → 5s → 2.5s at levels 1/2/3). is_negative true.
    spell(50023, "Test Poly", 0, 0, 0, 30, 10000, 1, 0, true, 1);
    effect(50023, 0, 0xB0, 0, 0, 1, 4, 3); // A_CONTROL, p0 = M_POLY (4), p0_kind = P_MECHANIC (3), enemy

    // (Break-on-damage test fixture — a stun spell with aura_interrupt bit 0 — is SQL-inserted in the
    // live test, not seeded here: the `spell(...)` closure hard-codes aura_interrupt 0 and the break path
    // works for ANY spell carrying the flag, so no permanent fixture is needed.)

    // Test Stun Immunity (50040) — CC IMMUNITY: ONE A_IMMUNITY (0xB1) aura whose p0 names the MECHANIC
    // M_STUN (1, p0_kind 3 P_MECHANIC). While a unit carries this aura, an incoming A_CONTROL(M_STUN)
    // effect is REFUSED in apply_effect (no stun aura is placed → `is_stunned` stays false). target 2
    // (T_TARGET_ALLY) so a debug cast can place it on any chosen unit; duration 600000; is_negative false
    // (a protective buff). base_points unused (immunity carries no magnitude). Mirrors the Inner Fire /
    // Fire Ward A_* aura-seed shape; verifies the CC-immunity gate.
    spell(
        50040,
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
    effect(50040, 0, 0xB1, 0, 0, 2, 1, 3); // A_IMMUNITY, p0 = M_STUN (1), p0_kind = P_MECHANIC (3), ally

    // Taunt (355) — the THREAT-yank: ONE E_TAUNT (0x06) instant effect targeting an ENEMY. It tops the
    // caster's threat on the target creature to one above the table max, so the next threat-retarget pass
    // (tick_creatures) switches the creature onto the taunter regardless of who out-damaged whom — the
    // classic tank tool. No duration/aura (the topped threat persists in the table); is_negative true.
    // base_points 0 (E_TAUNT ignores magnitude — the effect is "set threat to top"). 30 yd so a debug
    // cast from the player reaches a nearby creature; 0 cost for frictionless verification.
    spell(355, "Taunt", 1, 0, 0, 30, 0, 1, 0, true, 0);
    effect(355, 0, 0x06, 0, 0, 1, 0, 0); // E_TAUNT, target 1 (T_TARGET_ENEMY)

    // Resurrection Sickness (15007) — the Spirit-Healer res penalty: a 10-min (600000 ms) all-stats
    // debuff applied by `spirit_healer_res` (the graveyard res, vs the corpse run which has no penalty).
    // ONE A_MOD_STAT (0xA0) effect, p0 = STAT_ALL (0xFF, p0_kind P_STAT_ID 1), negative amount → −10 to
    // every attribute (the same one→all shape Mark of the Wild's three A_MOD_STAT effects use, folded
    // through the existing stat path). is_negative true; self-target (0). (Vanilla also reduces
    // damage/speed and scales with level; a flat −10 to all stats is the current approximation — the
    // importer can supersede it with the real DBC effect later.)
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
    effect(15007, 0, 0xA0, -10, 0, 0, 0xFF, 1); // −10 to ALL stats (STAT_ALL), self; p0_kind 1 P_STAT_ID

    // Talents — the starter Warrior talent metadata + passive talent spells (reserved 51xxx
    // ids, above the importer's vanilla range, never in a createinfo kit). Idempotent + shared with
    // `debug_seed_talents` (init does NOT re-run on an auto-migrate publish, so the live DB re-seeds via that).
    crate::talent::seed_talents(ctx);

    // Stacking-group starter set (work-item 192) — hand-authored until 102's cmangos `spell_group` SQL
    // dump lands wholesale. Idempotent + shared with `debug_repair_after_publish`, which is how an
    // already-migrated development database picks up reconciled rows (init does NOT re-run).
    seed_spell_groups(ctx);
}

/// Stratum 4 — scheduler arming: the event reaper, instance reaper, creature movement tick, melee
/// swing tick, aura-expiry tick, ground-AoE damage tick, and weather roll. Runs last so nothing
/// fires against a half-seeded database.
fn seed_scheduler_arming(ctx: &ReducerContext) {
    // Schedule the event reaper every 1s.
    ctx.db
        .game_event_reaper_schedule()
        .insert(EventReaperSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(EVENT_TTL_MICROS)),
        });

    // Schedule the instance reaper (work-item 190 slice 3) every 60s — minutes-scale occupancy
    // stamping + the 30min-empty / reset-requested reap. A live DB (auto-migrate publish) never
    // re-runs init, so re-arm there via `debug_rearm_instance_reaper` (the
    // `debug_rearm_creature_tick` precedent).
    ctx.db
        .game_instance_reaper_schedule()
        .insert(crate::instance::InstanceReaperSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                crate::instance::INSTANCE_REAPER_INTERVAL_MICROS,
            )),
        });

    // Schedule the creature movement tick every 0.5s (= ai::MOVE_TICK_MICROS). The movement passes run
    // every tick (smooth, mangos-cadence motion); the O(N) sensing passes only every 8th tick (~4s) —
    // see `tick_creatures`. A live DB (auto-migrate publish) keeps its old interval, so re-arm via the
    // `debug_rearm_creature_tick` reducer (init does NOT re-run on a plain publish).
    // Work-item 229: this seeded row is the GLOBAL/CATCH-ALL ticker (`GLOBAL_TICK_INSTANCE`) — it
    // covers instance 0 AND every instance without a dedicated row of its own (load-bearing; never
    // delete it). Dedicated per-instance rows are inserted by 190 slice 2's create_instance (or, until
    // then, `debug_arm_instance_tick`).
    ctx.db
        .game_creature_move_schedule()
        .insert(CreatureMoveSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
            instance_id: crate::creatures::GLOBAL_TICK_INSTANCE,
        });

    // Schedule the melee swing tick every 100ms. The tick is the timing RESOLUTION for per-unit
    // attack speeds: swings land on a 100ms boundary, so any 0.1s-granular weapon speed is
    // exact. (Scaling note: a global 100ms poll over all engagements is fine at this scale; the
    // event-driven alternative is a one-shot ScheduleAt::Time per swing.)
    ctx.db.game_melee_schedule().insert(MeleeSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(100_000)),
    });

    ctx.db.game_pet_care_schedule().insert(PetCareSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
            crate::creatures::CARE_INTERVAL_MICROS,
        )),
    });

    // Aura-expiry tick every 1s (tracer): drops auras whose timer elapsed (mirrors the melee tick).
    ctx.db.game_aura_schedule().insert(AuraSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(1_000_000)),
    });

    ctx.db.game_breath_schedule().insert(BreathSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(1_000_000)),
    });

    ctx.db.game_duel_schedule().insert(crate::duel::DuelSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
            crate::duel::DUEL_TICK_MICROS,
        )),
    });

    // Ground-AoE damage tick every 500ms (118): drives game_ground_area (Consecration/…). 500ms so a
    // 1s/2s area period fires within ~½ tick of due. Areas gate on their own next_tick_micros.
    ctx.db
        .game_ground_area_schedule()
        .insert(GroundAreaSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
        });

    // Movement republish tick every 50ms = 20 Hz (#461): drains the PRIVATE
    // `game_entity_motion_pending` staging table into the public `game_entity_motion` relay in ONE
    // transaction, so SpacetimeDB's per-transaction subscription sweep runs 20×/s instead of once
    // per movement packet. LOAD-BEARING — without this row peer movement stages and never relays.
    // A live DB (auto-migrate publish) never re-runs init, so `debug_repair_after_publish` ensures
    // it there, and `motion::ensure_schedule_armed` is the unconditional third net.
    // Retune live with `set_motion_tick_ms`.
    ctx.db
        .game_motion_publish_schedule()
        .insert(crate::motion::MotionPublishSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                crate::motion::MOTION_TICK_MICROS,
            )),
        });

    // Gateway lease reaper (#468 stage 4a): despawns the players of a gateway that stopped
    // heartbeating (the shared-connection crash case). Inert while `game_gateway_session` is
    // empty — nothing binds sessions to leases until stage 4d — but armed from day one so the
    // ghost bound exists the moment the first leased session appears. Same three-net story as
    // the motion tick above: `debug_repair_after_publish` ensures it on a live DB.
    ctx.db
        .game_gateway_lease_reaper_schedule()
        .insert(crate::gw::GatewayLeaseReaperSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                crate::gw::LEASE_REAP_MICROS,
            )),
        });

    // Weather roll every 10 minutes: re-rolls each zone that has climate data and writes only the
    // rows whose sky actually changed. Without this row weather never advances past whatever a zone
    // was last forced to. Shared with `debug_repair_after_publish`, which re-arms it on an
    // already-migrated database where `init` does not re-run — one definition of the canonical row,
    // so the two paths cannot arm different intervals.
    crate::weather::rearm_weather_schedule(ctx);
}

// Test/mock-seed fixture kits (Test PW:Shield, scenario quest/vendor/trainer, …) live in their
// own file; `init` and the `debug_seed_*` reducers reach them through this re-export, so callers
// keep the `seed::seed_*_fixture` paths.
mod fixtures;
pub(crate) use fixtures::*;

/// The createinfo starting kits — the spells a fresh character knows before any training, copied
/// into `game_player_spell` at creation (`grant_createinfo_spells`). `race == 0` rows are class kits
/// (any race), `class == 0` rows are racials (any class). Only-if-empty: operator edits and deletes
/// stick across restarts. Values are the per-race/class vanilla starting-spell basics — spell ids
/// are interop facts, covered by `docs/data-ingestion.md`'s "What the seeded fixture knowingly
/// contains" carve-out. An id with no `game_spell` row shows in the client book (the client
/// renders it from its own Spell.dbc) and casts as a graceful "unknown spell" Err until imported.
pub(crate) fn seed_createinfo_spells(ctx: &ReducerContext) {
    use crate::spell::spellbook::game_createinfo_spell;
    let table = ctx.db.game_createinfo_spell();
    if table.count() > 0 {
        return;
    }
    for &(race, class, spell_id) in CREATEINFO_KIT {
        table.insert(crate::spell::spellbook::CreateinfoSpell {
            id: 0,
            race,
            class,
            spell_id,
        });
    }
}

/// The stacking-group starter set (work-item 192) — hand-authored ahead of 102's cmangos `spell_group`/
/// `spell_group_stack_rules` SQL dump, which will fill `game_spell_group`/`game_spell_group_rule`
/// wholesale and supersede this. Idempotent (only-if-empty, mirroring `seed_createinfo_spells`); shared by
/// `init` and `debug_repair_after_publish` (init does NOT re-run on an auto-migrate publish). It
/// converges its small hand-authored fixture: rules are updated and missing memberships are added,
/// while existing memberships are never duplicated or deleted.
///
/// Every spell id below is a REAL vanilla id (licensing firewall — no bulk data, same posture as
/// `CREATEINFO_KIT`). Membership was reconciled id-by-id against `DBFilesClient/Spell.dbc` read out of a
/// locally owned 1.12.1 client; the rule and comparability columns are not client-derived. See
/// `docs/data-ingestion.md` ("Starter aura families") for the provenance record and what stays unverified.
///
/// `game_spell_group` carries NO foreign key to `game_spell` (SpacetimeDB has none to enforce, and the
/// group/rule tables are deliberately reference-only), so seeding an id this sandbox doesn't have a
/// `game_spell` header for yet is HARMLESS — it simply never matches any live aura until the spell itself
/// is imported (`aura_apply`'s `apply_group_conflict` looks members up by spell_id off the live
/// `game_aura` rows, not the reverse). Only Mark of the Wild (1126), Battle Shout
/// (`tracer_spell::SPELL_ID` = 6673), and the synthetic Well Fed (50116) are in today's curated kit.
///
/// `rank_is_comparable` is true for Battle Shout alone: it is the only family here built from a single
/// rank chain. Every other family mixes chains whose rank numbers describe different spells — Prayer of
/// Fortitude rank 1 carries Power Word: Fortitude rank 5's magnitude — so they compare effect magnitude,
/// with an existing aura's stack count folded in.
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

/// The starter families themselves: `(group_id, rule, rank_is_comparable, members)`. A const rather
/// than a local so the reconciliation tests read the rows instead of scanning this file's text.
pub(crate) const SPELL_GROUPS: &[(u32, u8, bool, &[u32])] = {
    use crate::spell::stacking::{
        RULE_EXCLUSIVE, RULE_EXCLUSIVE_PER_CASTER, RULE_EXCLUSIVE_STRONGER,
    };
    &[
        // 1: Mark of the Wild ranks 1-7 / Gift of the Wild ranks 1-2 (Druid stat buff family).
        (
            1,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[1126, 5232, 6756, 5234, 8907, 9884, 9885, 21849, 21850],
        ),
        // 2: Power Word: Fortitude ranks 1-6 / Prayer of Fortitude ranks 1-2 (Priest stamina family).
        (
            2,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[1243, 1244, 1245, 2791, 10937, 10938, 21562, 21564],
        ),
        // 3: Paladin Blessings — per-caster exclusive (a paladin's OWN blessing replaces their prior
        // one; another paladin's stands separately). A Greater Blessing is the same buff as its single-
        // target form, so it shares the family. Freedom/Protection/Sacrifice are deliberately absent:
        // whether they share this exclusivity is a rule question no client table answers.
        (
            3,
            RULE_EXCLUSIVE_PER_CASTER,
            false,
            &[
                19740, 19834, 19835, 19836, 19837, 19838, 25291, // Might r1-7
                19742, 19850, 19852, 19853, 19854, 25290, // Wisdom r1-6
                20217, // Kings
                1038,  // Salvation
                20911, 20912, 20913, 20914, // Sanctuary r1-4
                19977, 19978, 19979, // Light r1-3
                25782, 25916, // Greater Might r1-2
                25894, 25918, // Greater Wisdom r1-2
                25890, // Greater Light
                25895, // Greater Salvation
                25898, // Greater Kings
                25899, // Greater Sanctuary
            ],
        ),
        // 4: Battle Shout ranks 1-7 (rank 1 = `tracer_spell::SPELL_ID`, IN the curated kit). One rank
        // chain, so this is the only family whose rank numbers may be compared directly.
        (
            4,
            RULE_EXCLUSIVE_STRONGER,
            true,
            &[6673, 5242, 6192, 11549, 11550, 11551, 25289],
        ),
        // 5: Armor-debuff family (Sunder Armor / Expose Armor / Faerie Fire, including the Feral form)
        // — cross-spell, any caster.
        (
            5,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[
                7386, 7405, 8380, 11596, 11597, // Sunder Armor r1-5
                8647, 8649, 8650, 11197, 11198, // Expose Armor r1-5
                770, 778, 9749, 9907, // Faerie Fire r1-4
                16857, 17390, 17391, 17392, // Faerie Fire (Feral) r1-4
            ],
        ),
        // 6: Intellect (Arcane Intellect ranks 1-5 / Arcane Brilliance).
        (
            6,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[1459, 1460, 1461, 10156, 10157, 23028],
        ),
        // 7: Spirit (Divine Spirit ranks 1-4 / Prayer of Spirit).
        (
            7,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[14752, 14818, 14819, 27841, 27681],
        ),
        // 8: Shadow Protection ranks 1-3 / Prayer of Shadow Protection.
        (
            8,
            RULE_EXCLUSIVE_STRONGER,
            false,
            &[976, 10957, 10958, 27683],
        ),
        // 9: Well Fed — the food buff family, plus this sandbox's synthetic 50116. Eating replaces the
        // active food buff even when the new meal is worse, so this family is EXCLUSIVE, not
        // EXCLUSIVE_STRONGER: a strength gate would refuse the meal instead of applying it.
        (
            9,
            RULE_EXCLUSIVE,
            false,
            &[
                50116, 19705, 19706, 19708, 19709, 19710, 19711, 24799, 24870, 25694, 25941,
            ],
        ),
    ]
};

/// `(race, class, spell_id)` rows; 0 = wildcard. Kits verified against classic-db
/// `playercreateinfo_spell` (race=1); everything else a class owns is trainer-taught.
pub(crate) const CREATEINFO_KIT: &[(u8, u8, u32)] = &[
    // Warrior: Heroic Strike + Battle Stance.
    (0, 1, 78),
    (0, 1, 2457),
    // Paladin: Holy Light + Seal of Righteousness (Devotion Aura/Judgement are trainer-taught).
    (0, 2, 635),
    (0, 2, 20154),
    // Hunter: Auto Shot (ranged auto-attack) + Raptor Strike + Serpent Sting.
    (0, 3, 75),
    (0, 3, 2973),
    (0, 3, 1978),
    // Rogue: Sinister Strike + Eviscerate (Stealth is trainer-taught).
    (0, 4, 1752),
    (0, 4, 2098),
    // Priest: Smite + Lesser Heal (PW:Fortitude/Inner Fire/Dispel Magic are trainer-taught).
    (0, 5, 585),
    (0, 5, 2050),
    // Shaman: Lightning Bolt + Healing Wave + Earth Shock.
    (0, 7, 403),
    (0, 7, 331),
    (0, 7, 8042),
    // Mage: Fireball + Frost Armor (Frostbolt/Fire Blast are trainer-taught).
    (0, 8, 133),
    (0, 8, 168),
    // Warlock: Shadow Bolt + Demon Skin (Immolate/Summon Imp are trainer-taught at L1).
    (0, 9, 686),
    (0, 9, 687),
    // Druid: Mark of the Wild + Wrath + Moonfire + Healing Touch + Rejuvenation.
    (0, 11, 1126),
    (0, 11, 5176),
    (0, 11, 8921),
    (0, 11, 5185),
    (0, 11, 774),
    // Human racials: Sword Spec, The Human Spirit, Diplomacy, Perception, Mace Spec
    // (non-Human racials land when those races do).
    (1, 0, 20597),
    (1, 0, 20598),
    (1, 0, 20599),
    (1, 0, 20600),
    (1, 0, 20864),
];

// ===========================================================================================
//  #223 — seed idempotence + fixture completeness.
//
//  `init` is a `#[reducer(init)]`: it runs ONCE per database and does NOT re-run on an
//  auto-migrate publish. Everything it seeds is therefore either only-if-empty or an upsert, and
//  is additionally reachable from a feature-gated `debug_seed_*` twin so a long-lived shard can be
//  brought forward without a re-provision. Neither property is checkable at runtime here (no
//  `ReducerContext` harness exists by design), so the DATA
//  invariants are asserted directly and the two structural ones are pinned by a source scan
//  through `test_scan::code_of`, which strips comments (a bare `.contains()` on an unstripped body
//  is exactly what a trailing-comment needle defeats — issue #64).
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// The playable classes of vanilla 1.12.1. 6 (Death Knight) and 10 do not exist in this
    /// expansion, which is why `CREATEINFO_KIT`'s class column skips them.
    const VANILLA_CLASSES: &[u8] = &[1, 2, 3, 4, 5, 7, 8, 9, 11];

    /// FIXTURE COMPLETENESS. A character created for a class with no kit arrives in the world with
    /// an empty spellbook: no attack, no heal, nothing on the action bar, and no error anywhere —
    /// the class is simply unplayable, and only a live login reveals it.
    ///
    /// This is the assertion that fires when a class becomes creatable before its kit is written.
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
                "class {class} starts with {} spell(s) {kit:?}. Every vanilla class begins with at \
                 least an attack and one more ability; a class whose kit is missing or truncated \
                 logs in with an empty spellbook and no error to say why",
                kit.len()
            );
        }

        // The other direction: no kit for a class this expansion does not have. Such rows are dead
        // weight that nothing can ever grant, and they hide a typo in the class column.
        for (race, class, spell) in CREATEINFO_KIT {
            assert!(
                *class == 0 || VANILLA_CLASSES.contains(class),
                "kit row (race {race}, class {class}, spell {spell}) names a class that does not \
                 exist in 1.12.1 — nothing can ever be granted from it"
            );
        }
    }

    /// The wildcard convention is `race == 0` = any race (a class kit) and `class == 0` = any class
    /// (a racial). A row with BOTH zero would be granted to every character ever created, which is
    /// never what a kit row means — and it is a plausible typo, because both columns are `u8` and
    /// the two zeros look like the other's wildcard.
    ///
    /// Duplicates matter for the same reason: the seeder inserts every row unconditionally, so a
    /// repeated triple puts the same spell in the book twice.
    #[test]
    fn the_starting_kit_rows_are_unique_and_never_wildcard_on_both_columns() {
        let mut seen = std::collections::BTreeSet::new();
        for row in CREATEINFO_KIT {
            let (race, class, spell) = row;
            assert!(
                !(*race == 0 && *class == 0),
                "kit row {row:?} is a wildcard on BOTH columns — it would be granted to every \
                 character of every race and class"
            );
            assert_ne!(
                *spell, 0,
                "kit row {row:?} grants spell id 0, which is not a spell"
            );
            assert!(
                seen.insert(*row),
                "kit row {row:?} appears twice; the seeder inserts unconditionally, so the spell \
                 lands in the starting spellbook twice"
            );
        }

        // Human (race 1) is the only implemented race, so its racials must be present — the kit is
        // otherwise silently all-class-and-no-race.
        assert!(
            CREATEINFO_KIT.iter().any(|(race, _, _)| *race == 1),
            "no Human racials in the kit; every new character would be missing them with no error"
        );
    }

    /// The starting-kit seeder remains only-if-empty: it must not rewrite an operator's kit edits.
    /// The spell-group seeder is deliberately different: it converges existing development databases
    /// on the reconciled starter rows while avoiding duplicate memberships.
    #[test]
    fn starter_kit_seeder_still_returns_early_before_writing_anything() {
        let src = include_str!("seed.rs");
        let body = crate::test_scan::code_of(
            src,
            "pub(crate) fn seed_createinfo_spells(ctx: &ReducerContext) {",
        );
        let guard_at = body
            .find("if table.count() > 0 {")
            .expect("starting-kit guard is gone");
        let return_at = body[guard_at..]
            .find("return;")
            .expect("starting-kit guard does not return");
        let insert_at = body
            .find(".insert(")
            .expect("starting-kit seeder does not insert");
        assert!(
            guard_at + return_at < insert_at,
            "starting-kit guard must precede its first write"
        );
    }

    /// The seeder converges an already-migrated database instead of skipping it: rules are updated in
    /// place and only absent memberships are inserted, so re-running it duplicates nothing.
    #[test]
    fn spell_group_seeder_updates_rules_and_never_duplicates_memberships() {
        let body = crate::test_scan::code_of(
            include_str!("seed.rs"),
            "pub(crate) fn seed_spell_groups(ctx: &ReducerContext) {",
        );
        assert!(body.contains("rules.group_id().update(rule_row)"));
        assert!(body.contains(".any(|row| row.spell_id == spell_id)"));
    }

    /// Every rank of every Blessing the starter set claims, checked against the 1.12.1 client's
    /// `Spell.dbc` (see `docs/data-ingestion.md`). Ranks went missing here once already.
    #[test]
    fn blessing_family_carries_every_reconciled_rank() {
        let blessings: Vec<u32> = SPELL_GROUPS
            .iter()
            .find(|&&(id, ..)| id == 3)
            .expect("the Blessing family is gone")
            .3
            .to_vec();
        for spell_id in [
            19740, 19834, 19835, 19836, 19837, 19838, 25291, // Might r1-7
            19742, 19850, 19852, 19853, 19854, 25290, // Wisdom r1-6
            20911, 20912, 20913, 20914, // Sanctuary r1-4
            19977, 19978, 19979, // Light r1-3
            20217, 1038, // Kings, Salvation
            25782, 25916, 25894, 25918, 25890, 25895, 25898, 25899, // Greater Blessings
        ] {
            assert!(
                blessings.contains(&spell_id),
                "Blessing rank {spell_id} is missing"
            );
        }
    }

    /// Mark of the Wild's rank 4 was absent from an otherwise complete chain, and Faerie Fire's Feral
    /// cast was absent from the armor-debuff family. Both are client-verified members.
    #[test]
    fn reconciled_families_carry_the_ranks_that_were_missing() {
        let members = |group_id: u32| -> Vec<u32> {
            SPELL_GROUPS
                .iter()
                .find(|&&(id, ..)| id == group_id)
                .unwrap_or_else(|| panic!("group {group_id} is gone"))
                .3
                .to_vec()
        };
        assert!(members(1).contains(&5234), "Mark of the Wild rank 4");
        for feral in [16857, 17390, 17391, 17392] {
            assert!(members(5).contains(&feral), "Faerie Fire (Feral) {feral}");
        }
    }

    /// Rank numbers may only be compared inside one rank chain. Battle Shout is the only such family
    /// here; every other one mixes chains, so a `true` anywhere else silently makes an unrelated rank
    /// number outrank real effect magnitude.
    #[test]
    fn battle_shout_is_the_only_rank_comparable_family() {
        let comparable: Vec<u32> = SPELL_GROUPS
            .iter()
            .filter(|&&(_, _, rank_is_comparable, _)| rank_is_comparable)
            .map(|&(group_id, ..)| group_id)
            .collect();
        assert_eq!(comparable, vec![4]);
    }

    /// The live probe's premise: its two stamina buffs share one magnitude-compared family and its
    /// two Blessings share the per-caster family. A membership or rule edit that breaks either pairing
    /// would leave `docs/aura-stacking-probes.md` describing an outcome the module no longer produces.
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
        // Fortitude and Prayer of Fortitude: one family, strongest-wins, decided by magnitude rather
        // than by rank number, and differently named so the same-name rank sweep stays out of it.
        assert_eq!(family_of(1243), family_of(21562));
        assert_eq!(
            family_of(1243),
            (2, crate::spell::stacking::RULE_EXCLUSIVE_STRONGER, false)
        );
        // Blessing of Might and Blessing of Wisdom: one family, one Blessing per paladin.
        assert_eq!(family_of(19740), family_of(19742));
        assert_eq!(
            family_of(19740).1,
            crate::spell::stacking::RULE_EXCLUSIVE_PER_CASTER
        );
    }

    /// Eating replaces the active food buff even when the new meal is worse, so a strength gate would
    /// refuse the application outright.
    #[test]
    fn well_fed_replaces_rather_than_comparing_strength() {
        let (_, rule, _, members) = *SPELL_GROUPS
            .iter()
            .find(|&&(id, ..)| id == 9)
            .expect("the Well Fed family is gone");
        assert_eq!(rule, crate::spell::stacking::RULE_EXCLUSIVE);
        assert!(members.contains(&19705), "the real Well Fed food buff");
    }

    /// One spell in two exclusive families would make the applied rule depend on membership iteration
    /// order, which `apply_group_conflict` resolves by taking the first match.
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

    /// COLLISION SAFETY for the land-mount fixture (issue #22). Every id it reserves has to sit in a
    /// range the world ETL and the DBC import never write, and none may shadow an existing fixture —
    /// an id that collides silently replaces real imported data on a live shard, or is silently
    /// replaced by it, and either way the headless mount scenario stops testing what it claims to.
    #[test]
    fn the_mount_fixture_ids_stay_inside_the_reserved_ranges() {
        // Spells: the 50xxx synthetic range, and distinct from each other + the nearby tame fixture.
        for id in [FIXTURE_MOUNT_SPELL, FIXTURE_DAZED_SPELL] {
            assert!(
                (50_000..51_000).contains(&id),
                "fixture spell {id} is outside the reserved 50xxx synthetic-spell range"
            );
        }
        // Riding trainer markers live in the same range and must not shadow a mount/Dazed spell — a
        // marker is never resolved as a spell, so a collision would make one of them unreachable.
        let ids = [
            FIXTURE_MOUNT_SPELL,
            FIXTURE_DAZED_SPELL,
            TEST_TAME_BEAST_SPELL,
            crate::skill::LEARN_APPRENTICE_RIDING_SPELL_ID,
            crate::skill::LEARN_JOURNEYMAN_RIDING_SPELL_ID,
        ];
        let unique: std::collections::BTreeSet<u32> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "fixture spell ids collide: {ids:?}");

        // Creature template + item entry: the same reserved families the other fixtures use.
        assert!(
            (51_000..52_000).contains(&RIDING_TRAINER_ENTRY),
            "the riding trainer must use a reserved 51xxx creature entry"
        );
        for entry in [TEST_WOLF_ENTRY, PROFESSION_TRAINER_ENTRY, TEST_TAME_BOAR_ENTRY] {
            assert_ne!(
                entry, RIDING_TRAINER_ENTRY,
                "the riding trainer shadows an existing fixture creature"
            );
        }
        let items = [FIXTURE_BLADE, FIXTURE_JERKY, FIXTURE_REINS];
        for entry in items {
            assert!(
                entry >= 5_090_000,
                "fixture item {entry} is outside the reserved 509xxxx entry range"
            );
        }
        let unique_items: std::collections::BTreeSet<u32> = items.iter().copied().collect();
        assert_eq!(
            unique_items.len(),
            items.len(),
            "fixture item entries collide: {items:?}"
        );
    }

    /// REACHABILITY, which is idempotence's other half. `init` does not re-run on an auto-migrate
    /// publish, so a fixture reachable ONLY from `init` never lands on an already-provisioned
    /// shard, and one reachable only from a `debug_seed_*` reducer never lands on a fresh one
    /// unless a harness happens to call it. Both halves have gone wrong here: the comments in
    /// `init` record `seed_stealth_fixture` having been debug-only (issue #85's audit) and
    /// `seed_fixture_catalogue` being moved into `init` for exactly this reason.
    ///
    /// So: every fixture seeder must be called from at least one of the two, and the failure names
    /// which one is stranded.
    #[test]
    fn every_fixture_seeder_is_reachable_from_init_or_from_a_debug_reducer() {
        let fixtures_src = include_str!("seed/fixtures.rs");
        let init_src = include_str!("seed.rs");
        let debug_src = crate::test_scan::debug_dir_src();

        let seeders: Vec<&str> = fixtures_src
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("pub(crate) fn ")?;
                let name = rest.split('(').next()?;
                name.starts_with("seed_").then_some(name)
            })
            .collect();

        assert!(
            seeders.len() >= 8,
            "found only {} fixture seeders in seed/fixtures.rs — the extraction scan has stopped \
             matching and would pass vacuously. Did the declaration style change?",
            seeders.len()
        );

        for name in seeders {
            let call = format!("{name}(ctx)");
            let from_init = init_src.contains(&call);
            let from_debug = debug_src.contains(&call);
            assert!(
                from_init || from_debug,
                "`{name}` is never called: not from `init` (so it never lands on a fresh shard) \
                 and not from a `debug_*` reducer (so it can never be applied to an existing one). \
                 A fixture nobody seeds is a test that silently stops testing anything."
            );
        }
    }

    /// DRIFT REGRESSION (#363). The post-import fixture-restore path
    /// (`seed_scenario_fixtures`/`seed_fixture_items`, run via `debug_seed_scenario_fixtures`
    /// after a world-ETL re-import truncates `game_creature_template`/`game_item_template`) used
    /// to re-author full `CreatureTemplate`/`ItemTemplate` literals as hand-copies of `init`'s —
    /// and they drifted: the Profession Trainer was level 30/1500hp/"Cooking & Skinning" in
    /// `init` but level 10/100hp/"Fixture" in the restore copy, and the Test Wolf's
    /// money_min/max disagreed too. A shard restored after an ETL wipe therefore carried
    /// different fixtures than a fresh one — the exact cross-shard divergence class #85 was
    /// filed to kill, reintroduced by copy-paste.
    ///
    /// The fix collapses both paths onto ONE canonical constructor per fixture
    /// (`test_wolf_template`, `profession_trainer_template`, `tempered_blade_template`,
    /// `tough_jerky_template`, all in `seed/fixtures.rs`). This pins that both `init` (via
    /// `seed_map0_demo_content`, #377's stratum-2 split-out — see this file's header) and the
    /// restore path call the SAME constructors — a hand-copied struct literal reintroduced in
    /// either one fails this test loudly instead of silently drifting again.
    #[test]
    fn init_and_the_restore_path_build_shared_fixtures_from_the_same_constructor() {
        let seed_src = include_str!("seed.rs");
        let fixtures_src = include_str!("seed/fixtures.rs");

        let init_body = crate::test_scan::code_of(
            seed_src,
            "fn seed_map0_demo_content(ctx: &ReducerContext) {",
        );
        let restore_body = crate::test_scan::code_of(
            fixtures_src,
            "pub(crate) fn seed_scenario_fixtures(ctx: &ReducerContext) {",
        );
        let fixture_items_body = crate::test_scan::code_of(
            fixtures_src,
            "fn seed_fixture_items(ctx: &ReducerContext) {",
        );

        for ctor in ["test_wolf_template()", "profession_trainer_template()"] {
            assert!(
                init_body.contains(ctor),
                "`init` no longer calls `{ctor}` — did a hand-authored CreatureTemplate literal \
                 come back?"
            );
            assert!(
                restore_body.contains(ctor),
                "`seed_scenario_fixtures` (the post-import fixture-restore path) no longer calls \
                 `{ctor}` — a hand-copied literal here is exactly the #363 drift bug."
            );
        }

        for ctor in ["tempered_blade_template(", "tough_jerky_template("] {
            assert!(
                init_body.contains(ctor),
                "`init` no longer calls `{ctor}` — did a hand-authored ItemTemplate literal come \
                 back?"
            );
            assert!(
                fixture_items_body.contains(ctor),
                "`seed_fixture_items` (feeds the restore path's reserved-id fixture catalogue) no \
                 longer calls `{ctor}` — a hand-copied literal here is exactly the #363 drift bug."
            );
        }

        // Belt-and-suspenders, scoped to the fixture the constructor replaced (this function also
        // hand-authors OTHER, non-duplicated fixtures — Scenario Questgiver/Vendor/Weapon Master —
        // which is fine; only a WOLF/PROFESSION_TRAINER-entry literal here would be the drift bug
        // back). Whitespace-collapsed word-pair match, NOT `.contains()` — `target_entry: WOLF`
        // (the quest objective, which is legitimate) would otherwise false-positive on a plain
        // substring search for "entry: WOLF".
        let restore_shape = crate::test_scan::shape_of(
            fixtures_src,
            "pub(crate) fn seed_scenario_fixtures(ctx: &ReducerContext) {",
        );
        let words: Vec<&str> = restore_shape.split(' ').collect();
        for needle in [["entry:", "WOLF,"], ["entry:", "PROFESSION_TRAINER,"]] {
            assert!(
                !words.windows(2).any(|w| w == needle),
                "`seed_scenario_fixtures` hand-authors a `CreatureTemplate {{ {} {} ... }}` \
                 literal again instead of calling the shared constructor — this is how #363 \
                 happened.",
                needle[0],
                needle[1]
            );
        }
    }
}
