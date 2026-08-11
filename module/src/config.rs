//! Static-data tables: the realm list, starting positions, per-race display info, the legal
//! race/class combos, and the client `AreaTable.dbc`/`AreaTrigger.dbc` zone/trigger data (work-item
//! 209). Pure data definitions — no reducer logic. Categories follow `docs/schema.md`.

use spacetimedb::table;

// ===========================================================================================
//  Static-data tables [static]
// ===========================================================================================

/// One row per realm shown in the realm list. [static]
#[table(accessor = game_realm, public)]
pub struct Realm {
    #[primary_key]
    pub id: u8,
    pub name: String,
    pub address: String, // "ip:port" handed to the client
    pub realm_type: u32,
    pub flags: u8,
    pub population: f32,
    pub timezone: u8,
}

/// Canonical starting position per (race, class) — coords/map/zone for character creation. Loaded
/// from cmangos `playercreateinfo` by the importer's `--dump` mode (the demo seed has only the
/// Human-Warrior row; create_character falls back to it for unseeded combos). [static]
#[table(accessor = game_start_position, public)]
pub struct StartPosition {
    #[primary_key]
    pub race_class: u16, // (race << 8) | class
    pub race: u8,
    pub class: u8,
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub display_id: u32,
}

/// Server-wide tunables (singleton — `id` is always 0). `xp_rate` multiplies ALL XP gains (creature
/// kills + quest turn-ins) so a realm can speed up / slow down leveling for testing or a custom-rate
/// server. A missing row reads as 1.0× (see `xp::xp_rate`), so a fresh DB behaves Blizzlike. SQL-editable
/// (no Timestamp): `UPDATE game_config SET xp_rate = 2.0 WHERE id = 0`; `debug_set_xp_rate` is the harness
/// path. NOT player-callable — only the admin (SQL) / the debug reducer set it, so a client can't self-boost.
#[table(accessor = game_config, public)]
pub struct ServerConfig {
    #[primary_key]
    pub id: u32, // singleton: always 0
    pub xp_rate: f32,
    // END-APPENDED (work-item 243): nav-grid consumption gate (chase pathing + aggro/cast/melee
    // LoS). Default ON since 244 passed (benchmark: nav cost indistinguishable; live: all four
    // wall/fence/chase/hold-fire scenarios). A world WITHOUT nav data imported behaves exactly
    // as before either way (missing chunk = no obstacles known). Toggle: `debug_set_nav_enabled`
    // or `UPDATE game_config SET nav_enabled = false WHERE id = 0`.
    #[default(true)]
    pub nav_enabled: bool,
    // END-APPENDED (issue #39): does THIS database host dungeon-instance POPULATIONS? Default `true`
    // = every single-database realm behaves exactly as it always has (the portal spawns the dungeon
    // where the player is standing). Set `false` on the OPEN-WORLD shard of a multi-database
    // deployment (spec #12 Phase A): `create_instance_with_id` then files the `game_instance` row +
    // binding as a LEASE and spawns nothing, so the world writer never pays for a dungeon whose run
    // happens on another database — the instances shard, where this stays `true`, spawns the
    // population when the gateway mirrors the id there via `ensure_instance`.
    //
    // Deliberately a WORLD POLICY, not a shard id: the module still knows nothing about shards (spec
    // #12), it only knows whether it hosts instance populations — the `nav_enabled` precedent.
    // Operator-set, like `nav_enabled`: `UPDATE game_config SET hosts_instances = false WHERE id = 0`.
    // Deliberate simplification: one SQL line in the Phase A runbook rather than a gateway→module
    // policy push. Ceiling: an operator who forgets it gets today's behavior (the dungeon spawns
    // on the world shard and is evicted after the transfer) — degraded, never broken. Upgrade
    // path: the gateway derives it from the shard map and asserts it at startup.
    #[default(true)]
    pub hosts_instances: bool,
    // END-APPENDED: park every playerbot where it was spawned — the GOAL brain and the COMBAT brain
    // return immediately, so a crowd neither picks up quests nor grinds its way out of the zone. The
    // WANDER pass deliberately keeps running (6 yd hops around home), because a launch-day crowd
    // milling in a plaza is the movement load worth measuring. The load-test lever: bots that quest
    // and fight measure content, not capacity, and drag the zone's creatures into the number.
    // Operator-set like `nav_enabled`: `UPDATE game_config SET bots_idle = true WHERE id = 0`.
    #[default(false)]
    pub bots_idle: bool,
    // END-APPENDED (issue #521, decision #10): exact per-cell vmap collision-triangle consumption
    // gate — the LoS/collision ray queries in `vmap::los_ray`/`vmap::collision_ray`, and (#523)
    // `nav::has_los`'s consumers (aggro/assist/creature-casts/engage/swing-gate/caster hold-range)
    // plus Blink's collision clamp. Default OFF: the standard import pipeline
    // (`importer/scripts/import-world.sh`) has no vmap step, so a normally-provisioned world has
    // ZERO `game_vmap_chunk` rows for every map — and the missing-chunk contract (no row = "no
    // obstruction known here") then reads as "every ray is clear" MAP-WIDE, not just per-cell.
    // That's fine for a partially-covered map (the intended degrade) but wrong as a global
    // default while vmap import is a manual, unwired path (#520/#521). Flip per-map only after
    // `importer --vmap` has actually populated `game_vmap_chunk` for it. Toggle:
    // `debug_set_vmap_enabled` or `UPDATE game_config SET vmap_enabled = true WHERE id = 0`.
    #[default(false)]
    pub vmap_enabled: bool,
}

/// Starting items per (race, class) — the character-creation loadout. Loaded from the client
/// `CharStartOutfit.dbc` by the importer's `--dbc` mode (the cmangos dump's `playercreateinfo_item` is
/// EMPTY, so the outfit DBC is the source). `grant_starter_item` looks these up at character creation and
/// equips the equippable pieces / stows the rest; it falls back to the hand-authored Warrior loadout
/// when the table is empty (pre-import), so login never breaks. Multiple rows per (race, class). [static]
#[table(accessor = game_start_item, public, index(accessor = by_race_class, btree(columns = [race_class])))]
pub struct StartItem {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub race_class: u16, // (race << 8) | class — same key as game_start_position
    pub item_entry: u32,
}

/// Per-race display models + faction, loaded from the client `ChrRaces.dbc` by the importer's
/// `--dbc` mode (importer P1). `player_login` looks this up by the character's race to set the
/// gender-correct body model + nameplate faction, replacing the hardcoded `49`/`1` (it falls back to
/// those — the Human-Male values — when the table isn't loaded, so login never breaks). [static]
#[table(accessor = game_race_info, public)]
pub struct RaceInfo {
    #[primary_key]
    pub race: u8,
    pub male_display: u32, // CreatureDisplayInfo id (== 49 for Human male)
    pub female_display: u32,
    pub faction_template: u32, // ChrRaces.faction (== 1, Player|Alliance, for Human)
}

/// The legal (race, class) combinations, loaded from the client `CharBaseInfo.dbc` by the importer's
/// `--dbc` mode (importer P1). `create_character` rejects a combo absent from this table — server-side
/// defense-in-depth (the client already gates the UI). When the table is empty (unloaded), the gate
/// is skipped so character creation still works pre-import. PK packs `(race<<8)|class`. [static]
#[table(accessor = game_char_base_info, public)]
pub struct CharBaseInfo {
    #[primary_key]
    pub race_class: u16, // (race << 8) | class
    pub race: u8,
    pub class: u8,
}

/// One `AreaTable.dbc` row — a zone OR subzone (`parent_area_id` distinguishes: 0 = top-level zone
/// like Elwynn/Westfall; nonzero = a subzone whose value points at another `game_area.id`, its
/// enclosing zone). Loaded from the client `AreaTable.dbc` by the importer's `--dbc` mode (work-item
/// 209; see `importer/src/dbc.rs::area_sql`). Consumers: rest-state zone naming (196), exploration
/// XP's zone/subzone resolution (200), and `terrain::zone_id_at`'s one-hop subzone→zone chase for
/// graveyard + fishing zone resolution (209/375). `flags`/`faction_group` are the raw
/// `AreaTable.dbc` bitmasks (city/rest-state, PvP sanctuary) — undecoded here; a consumer decodes
/// what it needs. No Timestamp → plain SQL. [static]
#[table(accessor = game_area, public)]
pub struct GameArea {
    #[primary_key]
    pub id: u32,
    pub map_id: u32,
    pub parent_area_id: u32,
    pub area_bit: i32,
    pub flags: u32,
    pub exploration_level: i32,
    pub faction_group: u32,
    pub name: String,
}

/// One `AreaTrigger.dbc` row — a trigger volume (a sphere via `radius`, or a box via
/// `box_length`/`box_width`/`box_height`/`box_yaw`; vanilla trigger definitions use one shape or the
/// other, never both). Loaded from the client `AreaTrigger.dbc` by the importer's `--dbc` mode
/// (work-item 209; see `importer/src/dbc.rs::area_trigger_sql`). The geometric half of inn triggers
/// (196 — "make this inn your home" needs the player standing inside the inn's trigger volume),
/// dungeon entrances (190), and quest explore objectives. No Timestamp → plain SQL. [static]
#[table(accessor = game_area_trigger, public)]
pub struct GameAreaTrigger {
    #[primary_key]
    pub id: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub radius: f32,
    pub box_length: f32,
    pub box_width: f32,
    pub box_height: f32,
    pub box_yaw: f32,
}
