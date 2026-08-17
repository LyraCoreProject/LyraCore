//! Test/mock-seed FIXTURES — synthetic spells, items, NPCs, and quests (5xxxx ids) that keep
//! engine mechanics headlessly verifiable on a no-import sandbox (no Spell.dbc — the licensing
//! firewall keeps real client data out of the repo). This is a DIFFERENT fixture family from
//! `seed.rs`'s own map-0 (Northshire) demo content (see that file's header, layer 2): these are
//! mostly synthetic engine-mechanic probes with no map presence, kept in their own file so the kit
//! can grow without dragging `init` itself past readability. The taxi fixture is the exception: its
//! source flight master is restored from the same constructor as its reserved catalogue rows.
//! Every fn here is IDEMPOTENT (insert-if-absent, upsert, or reserved-id replacement) and is shared
//! by `init` and its feature-gated re-runner reducer (init does NOT re-run on an auto-migrate
//! publish, so a dev DB re-seeds through a reducer).

use spacetimedb::{ReducerContext, Table};

use crate::{
    game_creature_family, game_creature_spawn, game_creature_template, game_createinfo_spell,
    game_faction, game_item_template, game_spell, game_spell_effect, game_taxi_node, game_taxi_path,
    game_taxi_path_node, game_world_entity, CreatureFamily, CreatureSpawn, CreatureTemplate,
    CreateinfoSpell, Faction, GameTaxiNode, GameTaxiPath, GameTaxiPathNode, ItemTemplate, Spell,
    SpellEffect, ALL_PLAYABLE_CLASS_MASK, ALL_PLAYABLE_RACE_MASK,
};

/// Canonical fixture-NPC/item constructors — the single source of truth for the synthetic rows
/// that BOTH `seed::init` (fresh publish) and the post-import fixture-restore path
/// (`seed_scenario_fixtures`, invoked by `debug_seed_scenario_fixtures` after a world-ETL
/// re-import truncates `game_creature_template`/`game_item_template`) insert. Before this (#363)
/// the restore path re-authored these as hand-copied literals that drifted from `init`'s own
/// (Profession Trainer: level 30/1500hp/"Cooking & Skinning" vs level 10/100hp/"Fixture"; Test
/// Wolf: money_min/max 0/0 vs 25/50) — the exact cross-shard divergence class #85 was filed to
/// kill, reintroduced by copy-paste. Both callers now build from these fns so there is nothing
/// left to hand-copy out of sync. `init`'s values are treated as authoritative (they carry the
/// original design rationale, preserved below).
pub(crate) const TEST_WOLF_ENTRY: u32 = 51000;
pub(crate) const PROFESSION_TRAINER_ENTRY: u32 = 51001;
pub(crate) const TEST_TAME_BEAST_SPELL: u32 = 50300;
pub(crate) const TEST_TAME_BOAR_ENTRY: u32 = 51006;
const TEST_TAME_BOAR_FAMILY: u32 = 5;
const TEST_HUNTER_CLASS: u8 = 3;

/// "Test Wolf": a dedicated SKINNABLE beast so the skin verify is IMPORT-INDEPENDENT (the demo
/// Chicken is creature_type 8 = Critter → not skinnable). LEVEL 1 is intentional: the skill gate
/// is (creature_level - 1) * 10, so a level-1 beast requires skill 0 — a freshly-trained skinner
/// (skill=1) can skin it immediately without needing debug_set_skill.
pub(crate) fn test_wolf_template() -> CreatureTemplate {
    CreatureTemplate {
        entry: TEST_WOLF_ENTRY,
        name: "Test Wolf".to_string(),
        subname: String::new(),
        display_id: 720, // a wolf model that ships in 5875
        level: 1,
        health: 60,
        faction_template: 14, // Monster (hostile → a usable kill target, like the demo chicken)
        npc_flags: 0,
        unit_flags: 0,
        creature_type: 1, // BEAST (the skinnable gate — cmangos CREATURE_TYPE_BEAST)
        creature_family: 1, // Wolf (cmangos CreatureFamily)
        type_flags: 0x100, // SKINNABLE (cmangos CreatureTypeFlags bit; the creature_type==1 gate is sufficient)
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 2000,
        money_min: 0,
        money_max: 0,
        max_level: 0,
        max_level_health: 0,
        aggro_range: 0, // PASSIVE (engages only when attacked) so the test wolf doesn't maul the login demo
        damage_min: 0,
        damage_max: 0,
        armor: 0,              // set via `spacetime sql` on this row to mock-test mitigation
        pickpocket_loot_id: 0, // not imported — the test wolf has no pickpocket table
        // 0 ⇒ `skin_corpse` falls back to the flat Light Leather — the pre-210 verify flow
        // (debug_skin_nearest → 1x Light Leather) stays byte-identical without a seeded skin table.
        skin_loot_id: 0,
        trainer_type: 0,   // the test wolf is a beast, not a trainer
        trainer_class: 0,
    }
}

/// "Profession Trainer": a dedicated trainer NPC so LEARN-A-PROFESSION is verifiable on a
/// NO-IMPORT dev DB. npc_flags = GOSSIP|TRAINER (0x11) so the trainer window opens, faction 35
/// (FRIENDLY — a trainer you walk up to, NOT a combat target).
pub(crate) fn profession_trainer_template() -> CreatureTemplate {
    CreatureTemplate {
        entry: PROFESSION_TRAINER_ENTRY,
        name: "Profession Trainer".to_string(),
        subname: "Cooking & Skinning".to_string(),
        display_id: 3167, // a generic humanoid model that ships in 5875
        level: 30,
        health: 1500,
        faction_template: 35, // FRIENDLY (a trainer, not a kill target)
        npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
            | lyracore_shared::constants::npc_flags::TRAINER, // 0x11 — gossip-eye + trainer window
        unit_flags: 0,
        creature_type: 7, // Humanoid
        creature_family: 0,
        type_flags: 0,
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 2000,
        money_min: 0,
        money_max: 0,
        max_level: 0,
        max_level_health: 0,
        aggro_range: 0, // never aggros (friendly trainer)
        damage_min: 0,
        damage_max: 0,
        armor: 0,              // a trainer never takes damage anyway
        pickpocket_loot_id: 0, // not imported — a friendly trainer is never pickpocketed
        skin_loot_id: 0,       // not imported — a Humanoid trainer isn't skinnable anyway
        trainer_type: 2,   // TRADESKILLS — serves every class; the gate keys on trainer_class, which stays 0
        trainer_class: 0,
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

/// Import-independent Hunter/tame/boar catalogue fixture. The runtime branches only on the generic
/// effect kind; the synthetic spell id exists solely for headless sandbox scenarios.
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

/// The reserved taxi fixture: two nodes, one directed route, three ordered points, and one nearby
/// source flight master. This is the single module-side constructor used by fresh-database seeding
/// and the operator restore reducer after a map-0 world import replaces the creature roster.
///
/// Delete-then-insert corrects mutated canonical rows and is safe because every id is in the shared
/// reserved namespace. The DBC family's wholesale clear removes obsolete fixture ids if this shape
/// changes; imported low ids are never touched.
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

    ctx.db
        .game_world_entity()
        .guid()
        .delete(taxi::FLIGHT_MASTER_GUID);
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
    });
    crate::creatures::insert_creature_entity(
        ctx,
        crate::build_creature_entity(&spawn, &template, 0, 0),
    );
}

/// "Tempered Blade" — hand-authored reference weapon (licensing firewall: never bulk-imported;
/// display 1542 ships in 5875). `entry` is parameterized: `seed::init` uses the real id 50, the
/// fixture-restore path uses the reserved id `FIXTURE_BLADE` (5090050) so a world-ETL re-import
/// can never collide with it.
pub(crate) fn tempered_blade_template(entry: u32) -> ItemTemplate {
    ItemTemplate {
        class: 2,    // Weapon
        subclass: 7, // Sword (one-hand)
        display_id: 1542,
        quality: 2,         // Uncommon (green)
        inventory_type: 21, // main-hand
        item_level: 12,
        required_level: 1,
        max_durability: 70,
        buy_price: 1200,
        sell_price: 240,
        max_stack: 1,
        damage_min: 8.0,
        damage_max: 12.0,
        delay_ms: 2600,
        // Uncommon (green) gear binds on equip — vanilla's "greens are BoE" rule.
        bonding: crate::items::bonding::BIND_ON_EQUIP,
        ..base_item(entry, "Tempered Blade")
    }
}

/// "Tough Jerky" — hand-authored reference food (licensing firewall: never bulk-imported;
/// display 1542 ships in 5875, placeholder icon). `entry` parameterized the same way as
/// `tempered_blade_template`. spellid_1 (#387) points at "Eating" (50115) — the same food HoT the
/// real Tough Jerky (117) / Tough Hunk of Bread (4540) items use — so this NO-IMPORT fixture stays
/// usable now that `apply_item_use` reads spellid_1 as the single on-use authority.
pub(crate) fn tough_jerky_template(entry: u32) -> ItemTemplate {
    ItemTemplate {
        class: 0,    // Consumable
        subclass: 0, // Food & Drink
        display_id: 1542,
        quality: 0,        // Poor
        inventory_type: 0, // not equippable
        item_level: 1,
        required_level: 1,
        buy_price: 10,
        sell_price: 2,
        max_stack: 20,
        food_type: 1, // Meat
        spellid_1: 50115,                     // "Eating" — A_PERIODIC_HEAL food HoT
        spelltrigger_1: 0,                    // on-use
        bonding: crate::items::bonding::NONE, // plain food — unbound/tradeable
        ..base_item(entry, "Tough Jerky")
    }
}

/// Base-row constructors (#377) — every meaningful field of a fixture `Spell`/`SpellEffect`/
/// `ItemTemplate` is a struct-update override at the call site; every OTHER field (the ~80% that
/// are 0/false/empty on a synthetic fixture row) comes from one of these three fns. Mirrors the
/// `seed.rs` `spell`/`effect` closures' own implicit defaults exactly (so a fixture built this way
/// and one built through the closure agree byte-for-byte), just spelled as a real fn so
/// struct-update syntax can reach it, plus a matching `base_item`. `base_spell`'s `gcd_ms: 1500` is
/// the one non-zero default — it is the vanilla global cooldown, and every fixture spell wants it
/// except the handful that are proc-applied or item-triggered, which override it to 0 at the call
/// site (the override IS the data point, not a workaround). `base_item`'s `buy_count: 1` mirrors
/// `ItemTemplate`'s own `#[default(1u32)]` migration default for the same reason.
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
    }
}

/// See `base_spell`'s doc. `id` is the deterministic `(spell_id<<2)|effect_index` PK every
/// hand-authored effect row in this crate uses.
pub(crate) fn base_effect(spell_id: u32, effect_index: u8) -> SpellEffect {
    SpellEffect {
        id: ((spell_id as u64) << 2) | effect_index as u64,
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
    }
}

/// See `base_spell`'s doc.
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
    }
}

/// Seed Weakened Soul (6788) + the Test PW:Shield fixture (50072) — the generic linked-debuff mechanic.
/// IDEMPOTENT (inserts only rows that are absent), mirroring `talent::seed_talents`, so
/// it is safe to call from `init` (fresh install) AND from `debug_seed_pw_shield_fixture` on an
/// already-migrated dev DB (where `init` did not re-run).
///
/// Weakened Soul (REAL vanilla id 6788) is the hardcoded Power Word: Shield lockout debuff. Its real
/// Spell.dbc shape (CONFIRMED via a DBC dry-run for work-item 122 — this is NOT the effectless marker
/// earlier believed) is a single `A_IMMUNITY` (0xB1) aura with MiscValue 19 (MECHANIC_SHIELD): vanilla's
/// actual "immune to the shield mechanic" (i.e. can't be re-shielded) effect. 15s duration, holy school
/// (school_mask 2), dispel_type 0 — mirroring the importer's DBC output so a seed-only dev DB matches a
/// full-import DB byte-for-byte. Applied generically by `spell::apply_linked_debuff` whenever an aura
/// effect's `p1` names it; the PW:Shield lockout gate keys on `has_aura(target, 6788)` (presence of ANY
/// 6788 aura), so this aura fires the refusal exactly like the old marker did — now DBC-faithful.
///
/// Test Power Word: Shield (50072) is a PW:Shield-shaped fixture (a single A_ABSORB effect, matching the
/// real live spell 17's DBC shape) that links Weakened Soul (6788) via `p1` —
/// headlessly exercises the generic linked-debuff apply + refusal gate without needing a live client
/// Spell.dbc (none is available in every dev environment; the licensing firewall keeps it out of the
/// repo). Ally-targeted, holy school, absorbs 50 damage. `debug_cast_at(caster_guid, 50072, target_guid)`:
/// places the shield + Weakened Soul; a second cast at the same target within ~15s returns Err (the
/// linked-debuff refusal gate in resolve_cast_at); once Weakened Soul expires, a re-cast succeeds again.
///
/// The REAL live spell 17's imported `A_ABSORB` effect also carries `p1 = 6788` via a by-NAME override in
/// `importer/src/spell.rs` (`power_word_shield_p1_override`, mirroring the `synthetic_seal_effect`
/// precedent) — an operator who re-runs the importer against their own Spell.dbc gets the real spell 17
/// wired through the exact same generic mechanic as this fixture, no engine spell-id references needed.
/// This dev sandbox has no Spell.dbc (licensing firewall), so 50072 remains here purely so the mechanic is
/// headlessly exercisable without one.
pub(crate) fn seed_pw_shield_fixture(ctx: &ReducerContext) {
    // UPSERT (not insert-only): a `debug_seed_pw_shield_fixture` re-run on a dev DB that already has a
    // stale/earlier-shape fixture row corrects it in place, instead of silently keeping the stale data —
    // safe because these are TEST fixture ids (6788 is real-but-otherwise-unused; 50072 is a reserved
    // synthetic id), never touched by player state.
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
    // Drop any stale-shape 6788 effect rows (an earlier fixture seeded an inert A_FLAG at index 0), then
    // insert the DBC-faithful A_IMMUNITY at its real effect index (1). Mirrors the live import shape.
    for e in ctx.db.game_spell_effect().by_spell().filter(&6788u32) {
        ctx.db.game_spell_effect().id().delete(e.id);
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0xB1, // A_IMMUNITY
            base_points: 1,
            target: 2, // T_TARGET_ENEMY (DBC target 2)
            p0: 19,
            p0_kind: 3, // MECHANIC_SHIELD, P_MECHANIC
            ..base_effect(6788, 1)
        },
    );
    if let Some(mut s) = ctx.db.game_spell().spell_id().find(50072u32) {
        s.duration_ms = 30000; // must be nonzero — 0ms would reap the A_ABSORB instantly
        ctx.db.game_spell().spell_id().update(s);
    } else {
        ctx.db.game_spell().insert(Spell {
            range_yd: 30,
            duration_ms: 30000, // 30s shield (vanilla R1 is longer; a real duration is required — A_ABSORB is an aura, 0ms would reap it instantly)
            school_mask: 2,
            ..base_spell(50072, "Test Power Word: Shield")
        });
    }
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0xA2, // A_ABSORB
            base_points: 50,
            target: 2, // T_TARGET_ALLY
            p0: 2,
            p0_kind: 2, // holy school mask, P_SCHOOL_MASK
            p1: 6788,   // links Weakened Soul — the linked-debuff mechanic under test
            ..base_effect(50072, 0)
        },
    );
}

/// Hand-seed the Soul Shard item template (real vanilla item 6265). The .import ETL
/// doesn't reliably carry it, so — mirroring `seed_pw_shield_fixture`'s precedent for a mechanic whose
/// live-DBC row isn't available in every dev environment — it's authored here. A plain, non-equippable,
/// non-sellable trade good (vanilla: Soul Shard cannot be sold to a vendor; `sell_price: 0` encodes
/// that), stacking to 20 like the real item. IDEMPOTENT (inserts only if absent), so it's safe from both
/// `init` (fresh install) and `debug_seed_soul_shard_item` (an already-migrated dev DB where `init` did
/// not re-run).
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
        class: 7,         // Trade Goods
        subclass: 0,      // Trade Goods (generic)
        display_id: 1542, // placeholder icon (5875 fixture, like the other hand-authored items above)
        quality: 1,       // Common (white)
        item_level: 1,
        required_level: 1,
        sell_price: 0, // vendors refuse Soul Shards in real vanilla
        max_stack: 20,
        bonding: crate::items::bonding::BIND_ON_PICKUP, // real vanilla Soul Shard: unsellable + BoP
        ..base_item(SOUL_SHARD, "Soul Shard")
    });
}

/// Mock-seed Drain Soul (real vanilla spell 1120) as a channel headlessly exercisable
/// without a live Spell.dbc (licensing firewall, same precedent as `seed_pw_shield_fixture`'s Test
/// PW:Shield). A single `A_PERIODIC_DAMAGE` effect on the enemy target — the real spell's periodic
/// shadow-damage tick; the real vanilla script effect (`ChannelDeathItem`, the shard-on-kill grant) has
/// no Rust hook of its own here — it's implemented directly in `combat::kill_creature` (an aura naming
/// this spell id, cast by the killer, on the dying creature). 3s cast, 15s duration / 5 ticks (3s each),
/// shadow school. IDEMPOTENT (inserts only if absent).
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
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0x90, // A_PERIODIC_DAMAGE
            base_points: 45,
            period_ms: 3000,
            target: 1, // T_TARGET_ENEMY
            p0: 32,
            p0_kind: 2, // shadow school mask, P_SCHOOL_MASK
            ..base_effect(DRAIN_SOUL, 0)
        },
    );
}

/// Mock-seed Mana Burn (real vanilla spell 8129) as a single `E_POWER_BURN` effect,
/// headlessly exercisable without a live Spell.dbc import (same precedent as `seed_pw_shield_fixture`'s
/// Test PW:Shield / `seed_drain_soul_fixture`). `base_points 100` = drain up to 100 mana (floor-at-
/// available); `p1 50` = the vanilla EffectMultipleValue 0.5 in basis-points, so a full 100-mana drain
/// deals exactly 50 damage. `p0 0 / p0_kind 4 (P_POWER_TYPE)` documents the MANA gate for data parity
/// with the real importer mapping, though the module's E_POWER_BURN handler reads the target's power
/// type straight off `unit_bytes_0` (never p0). Shadow school (32), enemy target. IDEMPOTENT (inserts
/// only if absent).
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
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0x19, // E_POWER_BURN
            base_points: 100,
            target: 1,  // T_TARGET_ENEMY
            p0_kind: 4, // MANA, P_POWER_TYPE (documentation only — the handler reads unit_bytes_0)
            p1: 50,     // 50bp = vanilla EffectMultipleValue 0.5
            ..base_effect(MANA_BURN, 0)
        },
    );
}

/// Mock-seed Stealth (real vanilla spell 1784) as a self-targeted `A_STEALTH` presence
/// marker, headlessly exercisable without a live Spell.dbc import (same precedent as
/// `seed_pw_shield_fixture`'s Test PW:Shield / `seed_drain_soul_fixture`). A single `A_FLAG`-shaped
/// effect carrying `A_STEALTH` as its `kind` — matches the importer's real mapping (`importer/src/
/// spell.rs`: `"Stealth" => A_STEALTH`). `duration_ms: 0` because A_STEALTH is permanent-until-broken
/// (never timer-reaped; see `spell::taxonomy::A_STEALTH` / `scheduler.rs`'s reap-skip). IDEMPOTENT
/// (inserts only if absent).
///
/// Issue #85 audit: until this call was wired into `init` below, this fixture was reachable ONLY via
/// `debug_seed_stealth_fixture` — the exact bug class #85 fixed for the item/faction fixtures, just
/// for `game_spell`/`game_spell_effect` (the `spells` catalogue-fingerprint family) instead. It was
/// masked live only because 1784 is a REAL vanilla id the Spell.dbc importer already seeds on every
/// shard that has imported, so the insert-if-absent guards below silently no-op post-import — but a
/// freshly-published, not-yet-imported shard that had only this debug reducer run against it would
/// diverge from a sibling that didn't, same as the items/faction case. Now called from `init` too, so
/// every fresh shard agrees unconditionally regardless of import order.
pub(crate) fn seed_stealth_fixture(ctx: &ReducerContext) {
    const STEALTH: u32 = 1784;
    if ctx.db.game_spell().spell_id().find(STEALTH).is_none() {
        ctx.db.game_spell().insert(Spell {
            power_type: 3,
            school_mask: 1,
            ..base_spell(STEALTH, "Stealth")
        });
    }
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_STEALTH,
            target: 0,  // T_SELF
            p0_kind: 7, // P_FLAG
            ..base_effect(STEALTH, 0)
        },
    );
}

/// Mock-seed Chilled (real vanilla spell 6136) + Frost Armor (real vanilla spell 168) —
/// the reactive proc-on-being-hit-in-melee primitive, headlessly exercisable without a live Spell.dbc
/// import (same precedent as `seed_pw_shield_fixture`/`seed_drain_soul_fixture`/`seed_stealth_fixture`).
///
/// Chilled (6136) is the move-slow the proc applies to a melee attacker: ONE `A_MOD_SPEED` effect, p0 =
/// `SPEED_MOVE` (p0_kind `P_SPEED_KIND`), amount −30 (signed percent, matching vanilla Chilled's slow),
/// frost school, 5s duration. It is loaded through `apply_linked_debuff` (the same "apply spell X's aura
/// effects onto Y" machinery PW:Shield's Weakened Soul link already uses) — its OWN `target` field is
/// irrelevant to that path (the caller supplies the target explicitly), so it's left `T_TARGET_ENEMY` for
/// self-documentation / any future direct-cast use.
///
/// Frost Armor (168) mirrors the real DBC shape the importer maps (`importer/src/spell.rs`): eff0 is the
/// `+armor` self-buff (`A_MOD_RESISTANCE`, p0 = `RESIST_ARMOR` bit); eff1 is the reactive chill, classified
/// as `A_PROC_ON_HIT` with `trigger_spell = 6136` — `break_auras_on_damage`'s proc-on-hit scan reads it off
/// any melee-hit unit carrying this aura and applies Chilled onto the ATTACKER. Permanent self-buff
/// (`duration_ms = u32::MAX`, the importer's infinite-aura sentinel — matches vanilla armor spells' -1 DBC
/// duration). IDEMPOTENT (inserts only if absent), mirroring the other mock-seed fixtures.
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
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_SPEED,
            base_points: -30,
            target: 1,  // T_TARGET_ENEMY
            p0_kind: 6, // SPEED_MOVE, P_SPEED_KIND
            ..base_effect(CHILLED, 0)
        },
    );
    if ctx.db.game_spell().spell_id().find(FROST_ARMOR).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: u32::MAX, // permanent until replaced/dispelled
            school_mask: 16,
            ..base_spell(FROST_ARMOR, "Frost Armor")
        });
    }
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_RESISTANCE,
            base_points: 150,
            target: 0, // T_SELF
            p0: 1,
            p0_kind: 2, // RESIST_ARMOR bit, P_SCHOOL_MASK
            ..base_effect(FROST_ARMOR, 0)
        },
    );
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_PROC_ON_HIT,
            target: 0, // T_SELF
            trigger_spell: CHILLED,
            ..base_effect(FROST_ARMOR, 1)
        },
    );
}

/// Mock-seed Demon Skin (real vanilla spell 696, rank 2) — the COMBAT-INDEPENDENT
/// health-per-5 periodic-tick primitive, headlessly exercisable without a live Spell.dbc import (same
/// precedent as `seed_frost_armor_fixture`/`seed_pw_shield_fixture`/`seed_drain_soul_fixture`).
///
/// Observed vanilla behaviour (cross-checked against the reference cores — a behaviour citation, not a
/// port): aura 84 `SPELL_AURA_MOD_REGEN` ticks on a forced 5000ms period regardless of the DBC's own
/// EffectAmplitude, and heals a LIVING target with no combat gate at all — i.e. it ticks the SAME
/// in-combat or out, unlike the natural spirit-regen pass (out-of-combat-only)
/// or its during-combat-percent cousin `ModRegenDuringCombat`/`A_COMBAT_HEALTH_REGEN_PCT` (implemented
/// separately for Troll Regeneration — a DIFFERENT mechanic, not conflated here). This is
/// exactly the same primitive the engine already runs for Renew/bandages/food (`A_PERIODIC_HEAL`, folded
/// through `tick_auras` with no combat gate), so Demon Skin's eff2 is mock-seeded straight onto it: 5
/// health every 5000ms, matching wowhead classic's "restores 5 Health per 5 sec." tooltip for rank 2.
///
/// eff0 is the existing `+armor` self-buff (`A_MOD_RESISTANCE`, p0 = `RESIST_ARMOR` bit, +120); eff1 is
/// the `A_PERIODIC_HEAL` regen tick. Permanent-for-30-min per the tooltip (`duration_ms = 1_800_000`).
/// IDEMPOTENT (inserts only if absent), mirroring the other mock-seed fixtures.
pub(crate) fn seed_demon_skin_fixture(ctx: &ReducerContext) {
    const DEMON_SKIN: u32 = 696;
    if ctx.db.game_spell().spell_id().find(DEMON_SKIN).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: 1_800_000, // 30 min
            school_mask: 1,
            ..base_spell(DEMON_SKIN, "Demon Skin")
        });
    }
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_MOD_RESISTANCE,
            base_points: 120,
            target: 0, // T_SELF
            p0: 1,
            p0_kind: 2, // RESIST_ARMOR bit, P_SCHOOL_MASK
            ..base_effect(DEMON_SKIN, 0)
        },
    );
    upsert_effect(
        ctx,
        SpellEffect {
            kind: crate::spell::A_PERIODIC_HEAL,
            base_points: 5,
            period_ms: 5000,
            target: 0, // T_SELF
            ..base_effect(DEMON_SKIN, 1)
        },
    );
}

/// Mock-seed COMBAT-REGEN fixture: Test Regeneration (50137) — the one
/// `A_COMBAT_HEALTH_REGEN_PCT` (0xA9) source on a no-import sandbox. Demon Skin 696's regen effect
/// is `A_PERIODIC_HEAL`, not `A_COMBAT_HEALTH_REGEN_PCT`, so without a dedicated source the combat-
/// regen integration probe (test-combat-regen.sh) finds ZERO kind-169 rows on a fresh node and its
/// `HAS_COMBAT_REGEN_EFFECT` gate skips it forever. This fixture (operator-pick over the Troll
/// racial import — no DBC dependency) keeps the combat-regen-gate mechanic headlessly
/// exercisable: self-buff, 5 min, allows 5% of health_regen_per_tick THROUGH combat.
/// IDEMPOTENT (insert-if-absent), same precedent as every other fixture here.
pub(crate) fn seed_regen_fixture(ctx: &ReducerContext) {
    if ctx.db.game_spell().spell_id().find(50137u32).is_none() {
        ctx.db.game_spell().insert(Spell {
            duration_ms: 300_000,
            school_mask: 8,
            ..base_spell(50137, "Test Regeneration")
        });
    }
    // upsert_effect is delete-then-insert, so this is self-correcting on every call — no
    // insert-if-absent guard needed (see upsert_effect's doc).
    upsert_effect(
        ctx,
        SpellEffect {
            kind: 0xA9, // A_COMBAT_HEALTH_REGEN_PCT
            base_points: 5,
            target: 0, // self
            ..base_effect(50137, 0)
        },
    );
}

/// Reserved fixture ITEM entries (2026-07-16): the scenarios used the mock-seed items 50 (Tempered
/// Blade) and 52 (Tough Jerky), but the world ETL replaces those low entries with whatever real
/// imported items happen to occupy them — the vendor scenario bought a few-copper item where it
/// asserted a 1200c sword, and the quest rewarded something else entirely. Same reserved-id fix as
/// the 509xxxx quest/vendor rows: fixture entries the import never touches. Unlike
/// `seed_scenario_fixtures` below, these consts are read unconditionally from `init` (via
/// `seed_fixture_catalogue`), so they carry no `debug_reducers` `cfg_attr` — they are never dead in
/// a production build.
pub(crate) const FIXTURE_BLADE: u32 = 5090050;
pub(crate) const FIXTURE_JERKY: u32 = 5090052;

/// Insert the two reserved fixture item templates (insert-if-absent) — built from the same
/// `tempered_blade_template`/`tough_jerky_template` constructors the mock-seed's Tempered Blade
/// (50) / Tough Jerky (52) use, under the reserved entries above (#363: this used to be a
/// hand-copied literal that could drift from the mock-seed's).
fn seed_fixture_items(ctx: &ReducerContext) {
    let items = ctx.db.game_item_template();
    if items.entry().find(FIXTURE_BLADE).is_none() {
        items.insert(tempered_blade_template(FIXTURE_BLADE));
    }
    if items.entry().find(FIXTURE_JERKY).is_none() {
        items.insert(tough_jerky_template(FIXTURE_JERKY));
    }
}

/// Reserved fixture FACTION entry (2026-07-16): SYNTHETIC id — was 79, a REAL Faction.dbc id, so on
/// an imported node the insert-if-absent no-op'd against the real row (reputation_index -1, no bar)
/// and the quest's rep reward silently vanished (grant_reputation skips bar-less factions →
/// scenario-quest's "+250 rep" assert failed). 50900 collides with nothing the DBC ships (ids top
/// out ~1000).
pub(crate) const FIXTURE_FACTION: u32 = 50900;

/// Seed the reserved-id CATALOGUE rows the scenario fixtures reference: the two fixture items
/// (`FIXTURE_BLADE`/`FIXTURE_JERKY`) and the fixture faction above. Split out from
/// `seed_scenario_fixtures` (issue #85) and called from `init` too — same precedent as
/// `seed_pw_shield_fixture` — because these rows land in tables the cross-shard catalogue parity
/// check (#82) fingerprints whole (`game_item_template`, `game_faction`): before this, only a shard
/// that had `debug_seed_scenario_fixtures` run against it (historically the wire-suite's target,
/// lyracore) carried them, so its `items`/`dbc_reference` fingerprints permanently disagreed
/// with siblings that never ran the harness reducer — a false catalogue-skew signal, not a real one.
/// Calling this from `init` makes every freshly published shard agree unconditionally, matching how
/// `seed_pw_shield_fixture` already keeps `spells` in agreement. Idempotent (insert-if-absent), so
/// the repeat call from `debug_seed_scenario_fixtures` below is a no-op once `init` has run.
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
            // Slot 60: the real import claims indices 0..=54 of the client's 64-entry rep array
            // (danger-zones §1.4) — 60 stays clear of both the import and the array bound.
            reputation_index: 60,
            base_standing: 0,
        });
    }
}

/// Scenario-runner mock-seed: everything the four wire scenarios need on a
/// no-import sandbox, insert-if-absent like every other fixture here. Same precedent as
/// `seed_pw_shield_fixture` — call via `debug_seed_scenario_fixtures` post-publish.
///
/// - faction 79 with a real reputation bar (rep index 5) so a quest rep reward lands in
///   `game_player_reputation` (grant_reputation skips bar-less factions).
/// - quest 50900 "Wolf Cull": kill 2x Test Wolf (51000), rewards 150c + 90 XP + 2x Tough Jerky (52)
///   + 250 rep with faction 79. REPEATABLE so suite runs stay green without deleting the log row.
/// - questgiver NPC template 51003 (starts + ends 50900).
/// - vendor/repairer NPC template 51004 selling Tempered Blade (50) + Tough Jerky (52).
/// - trainer offering on the seeded Profession Trainer (51001): Lesser Heal (2050, a seeded 1.5s
///   heal) for 100c at level 1 — the train-and-cast scenario's purchase.
/// - Weapon Master NPC template 51005 ("Woo Ping", work-item 202): a second GOSSIP|TRAINER creature
///   (mirrors the 51004 vendor block) offering 1H Axe (skill line 44, marker 50130, required_level 1,
///   100c) and Polearm (skill line 229, marker 50131, required_level 60, 100c — the level-refusal
///   fixture). Both rows carry `learn_skill_line` set to a COMBAT line, so `apply_trainer_buy` routes
///   them onto the weapon fork (level-derived cap, presence-known) instead of the profession fork;
///   `learn_skill_cap` is irrelevant/ignored on that fork (kept at 0, never read).
///
/// Sole consumer today is the feature-gated harness reducer (`debug::debug_seed_scenario_fixtures`),
/// so a build WITHOUT `debug_reducers` (a production publish, or a `cargo clippy` that does not
/// unify the module's features) sees this fn itself as dead — silenced ONLY here, never
/// unconditionally (contrast `FIXTURE_BLADE`/`FIXTURE_JERKY` above, which stay reachable from `init`
/// in every build and so carry no such attribute).
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn seed_scenario_fixtures(ctx: &ReducerContext) {
    use crate::quest::quest_role;

    use crate::{
        game_creature_quest, game_npc_vendor, game_quest_objective, game_quest_reward_item,
        game_quest_template, game_quest_text, game_trainer_spell,
    };

    // Reserved fixture items + faction first — the quest reward/rep/vendor stock below reference
    // them. Also called unconditionally from `init` now (see `seed_fixture_catalogue`'s doc); this
    // call stays so an already-migrated dev DB that only ever runs the debug reducer still gets them.
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
            reward_money_max_level: 0, // fixture sets reward_xp explicitly, so this is unused here
        });
        ctx.db.game_quest_text().insert(crate::QuestText {
            quest_entry: QUEST,
            details: "The test wolves multiply. Cull two of them.".to_string(),
            objectives: "Kill 2 Test Wolves.".to_string(),
            offer_reward_text: "The pack thins. Well done.".to_string(),
            request_items_text: "Are the wolves culled?".to_string(),
        });
        // EXPLICIT reserved ids (not the auto_inc 0 sentinel): the world ETL imports these quest
        // tables with explicit dump ids, leaving the table's sequence allocator BEHIND the data —
        // an id-0 insert then allocates an id that already exists and PANICS (errno 12; the
        // fixture-seed rollback found live 2026-07-15). Fixed ids in the 509xx fixture range are
        // idempotent with the delete below and can never collide with dump rows.
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
                item_entry: FIXTURE_JERKY, // reserved fixture Tough Jerky (see seed_fixture_items)
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

    // 060/187 recurring trap: the world ETL truncates game_creature_template and reloads from the
    // dump — the INIT-seeded fixture templates (Test Wolf 51000, Profession Trainer 51001) vanish
    // on every re-import, breaking the wire scenarios until someone reseeds by hand. Re-seed them
    // HERE (this reducer is the operator's idempotent post-import fixture restore) from the SAME
    // canonical constructors `seed::init` uses (#363: this used to be a hand-copied literal that
    // drifted — Profession Trainer was level 10/100hp/"Fixture" here vs level 30/1500hp/"Cooking &
    // Skinning" in init, and Test Wolf's money_min/max disagreed too).
    let templates = ctx.db.game_creature_template();
    if templates.entry().find(WOLF).is_none() {
        templates.insert(test_wolf_template());
    }
    // The quest-loop's LOOT step needs a coin window: give the Test Wolf pocket change if it has
    // none yet (runs AFTER the insert-if-absent above so it converges to the same values whether
    // the wolf was just (re)inserted by this reducer or already existed from `init`; kill-time
    // money rolls read the template either way).
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
    // "Test Wolf Elder" (51002) — the BOT-SUITE fight fixture (266). The playerbots tests level
    // their bots to clear the cast level-gate (Taunt 355 = spell_level 10), which greys the L1
    // Test Wolf 51000 (aggro_radius returns 0 at a >=8 level gap) — so its wolves stop aggroing and
    // the tank has nothing to Taunt. This one is level 9: non-grey to a level-10 bot (20-level gap
    // rule), so it proximity-aggros and pays real kill XP. SEPARATE id keeps the scenario_quest
    // "grey L1 wolf pays 0 kill XP" fixture (51000) untouched — the done-when's hard constraint.
    const WOLF_ELDER: u32 = 51002;
    if templates.entry().find(WOLF_ELDER).is_none() {
        templates.insert(CreatureTemplate {
            entry: WOLF_ELDER,
            name: "Test Wolf Elder".to_string(),
            subname: String::new(),
            display_id: 720,
            level: 8, // diff 2 vs an L10 bot — inside the goals.rs GRIND ±3 band, non-grey
            health: 300, // survives a 1s top-up window vs an L10 trio's burst; solo-killable in ~15s
            faction_template: 14, // Monster (hostile — same as 51000)
            npc_flags: 0,
            unit_flags: 0,
            creature_type: 1, // BEAST (flee_eligible false — never routs mid-fight)
            creature_family: 1, // Wolf
            type_flags: 0x100, // SKINNABLE
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 25,
            money_max: 50,
            max_level: 0,
            max_level_health: 0,
            // EXPLICIT 20yd (aggro_radius returns an override verbatim, beating the grey rule) —
            // so proximity aggro survives ANY future bot level, not just level 10.
            aggro_range: 20,
            damage_min: 2, // low on purpose: a solo bot-goals bot survives 3 grind kills healer-less
            damage_max: 4,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
            trainer_type: 0,   // not a trainer
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
            faction_template: 35, // FRIENDLY
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP | 0x2, // 0x2 = UNIT_NPC_FLAG_QUESTGIVER (1.12)
            unit_flags: 0,
            creature_type: 7, // Humanoid
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
            pickpocket_loot_id: 0, // not imported — a Humanoid questgiver has no pickpocket table
            skin_loot_id: 0,       // not imported — a Humanoid questgiver isn't skinnable anyway
            trainer_type: 0,   // not a trainer
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
            pickpocket_loot_id: 0, // not imported — a Humanoid vendor has no pickpocket table
            skin_loot_id: 0,       // not imported — a Humanoid vendor isn't skinnable anyway
            trainer_type: 0,   // not a trainer
            trainer_class: 0,
        });
    }
    let vendor_rows = ctx.db.game_npc_vendor();
    // Explicit reserved ids (same errno-12 sequence-desync fix as the quest rows above: the ETL
    // imports vendor/trainer rows with explicit ids, leaving the sequence behind the data).
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

    const TRAINER: u32 = 51001; // the init-seeded Profession Trainer (already GOSSIP|TRAINER)
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

    // --- WEAPON MASTER (work-item 202): "Woo Ping" (51005) sells weapon proficiencies for gold —
    // the vanilla weapon-master shape (a trainer-list row whose `learn_skill_line` names a weapon line
    // instead of a spell/profession). Mirrors the 51004 vendor block: GOSSIP|TRAINER, faction 35
    // (FRIENDLY, never a kill target).
    const WEAPON_MASTER: u32 = 51005;
    // Marker spell ids for the weapon-learn offerings — NEVER resolved as real spells (no `game_spell`
    // header/effects), same convention as the profession markers in `skill.rs`; MUST match
    // `skill::LEARN_AXE_1H_SPELL_ID` / `skill::LEARN_POLEARM_SPELL_ID` (the debug reducer's twin lookup).
    const LEARN_AXE_1H: u32 = 50130; // -> learn_skill_line = AXE_1H (44)
    const LEARN_POLEARM: u32 = 50131; // -> learn_skill_line = POLEARM (229)
    if templates.entry().find(WEAPON_MASTER).is_none() {
        templates.insert(CreatureTemplate {
            entry: WEAPON_MASTER,
            name: "Woo Ping".to_string(),
            subname: "Weapon Master".to_string(),
            display_id: 3167,
            level: 30,
            health: 1500,
            faction_template: 35, // FRIENDLY (a trainer, not a kill target)
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
                | lyracore_shared::constants::npc_flags::TRAINER,
            unit_flags: 0,
            creature_type: 7, // Humanoid
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0, // never aggros (friendly trainer)
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0, // not imported — a friendly weapon master has no pickpocket table
            skin_loot_id: 0,       // not imported — a Humanoid trainer isn't skinnable anyway
            trainer_type: 2,   // TRADESKILLS: the weapon master serves every class, matching the real Woo Ping (entry 11867) in the dump
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
            learn_skill_cap: 0, // ignored on the weapon fork (cap is level-derived)
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
            required_level: 60, // the level-refusal fixture (Ginger's default level is well below 60)
            learn_skill_line: crate::skill::skill_line::POLEARM,
            learn_skill_cap: 0,
        });
    }
}

/// Idempotent fixture effect write: `SpellEffect.id` is a DETERMINISTIC PK
/// `(spell_id<<2)|effect_index` (NOT auto_inc), so a plain `insert` PANICS (errno 12,
/// unique-exists) whenever the curated importer has already written the same effect row — a
/// re-imported kit + a fixture re-seed collided live 2026-07-15. Delete-then-insert keeps the
/// fixture authoritative for its own rows without tripping the constraint.
///
/// THE DISCIPLINE (#377): delete-then-insert is idempotent BY CONSTRUCTION — calling it twice with
/// the same row is a no-op, and calling it with a changed shape self-corrects the row in place. So
/// every call site below calls this UNCONDITIONALLY, with no `if find(id).is_none() { ... }` guard
/// around it. A guard doesn't just add noise: it makes the delete dead code (the branch that would
/// run it never fires when the row already exists), which silently turns "re-seed self-corrects"
/// back into "re-seed only fills gaps" — exactly the bug class this fn exists to prevent. If a
/// future fixture effect ever needs to preserve an operator's hand-edit instead of overwriting it,
/// that is a genuinely different policy and needs a differently-named helper, not a guard bolted
/// onto this one.
fn upsert_effect(ctx: &spacetimedb::ReducerContext, row: SpellEffect) {
    ctx.db.game_spell_effect().id().delete(row.id);
    ctx.db.game_spell_effect().insert(row);
}

/// Stacking-family probe fixture — the four real family members a live aura-stacking probe needs
/// (`docs/aura-stacking-probes.md`).
///
/// A curated sandbox carries only rank 1 of each aura family and every one of those is self-cast,
/// so neither "the stronger member wins from either caster" nor "two paladins, one target" can be
/// staged on it without these rows. Power Word: Fortitude and Prayer of Fortitude are the
/// magnitude-compared EXCLUSIVE_STRONGER pair (family 2); Blessing of Might and Blessing of Wisdom
/// are the EXCLUSIVE_PER_CASTER pair (family 3). All four are ally-targeted so a second caster can
/// reach somebody else's target.
///
/// These are REAL vanilla ids, so a database whose catalogue already holds one keeps its own row:
/// an imported Spell.dbc is authoritative and a fixture must never overwrite it.
pub(crate) fn seed_stacking_probe_fixture(ctx: &ReducerContext) {
    // (spell_id, name, effect kind, p0, p0_kind, magnitude)
    const PROBE_SPELLS: &[(u32, &str, u8, i32, u8, i32)] = &[
        // Family 2 — A_MOD_STAT(STAT_STA), compared by magnitude. The pair is deliberately drawn from
        // the family's two DIFFERENT chains: `aura_apply` displaces a same-NAME other rank before any
        // family policy runs, so two ranks of one chain would never reach the strength comparison.
        (1243, "Power Word: Fortitude", 0xA0, 2, 1, 3),
        (21562, "Prayer of Fortitude", 0xA0, 2, 1, 26),
        // Family 3 — one Blessing per paladin per target. Might is A_MOD_COMBAT(attack power),
        // Wisdom an A_MOD_STAT(STAT_SPI) stand-in for its mana regen.
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
                target: 2, // T_TARGET_ALLY — the probe's second caster buffs someone else's target
                p0,
                p0_kind,
                ..base_effect(spell_id, 0)
            },
        );
    }
}
