use spacetimedb::{log, reducer, table, ReducerContext, Table};

use crate::helpers::require_operator;

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
    // END-APPENDED: nav-grid consumption gate (chase pathing + aggro/cast/melee
    // LoS). Default ON since 244 passed (benchmark: nav cost indistinguishable; live: all four
    // wall/fence/chase/hold-fire scenarios). A world WITHOUT nav data imported behaves exactly
    // as before either way (missing chunk = no obstacles known). Toggle: `debug_set_nav_enabled`
    // or `UPDATE game_config SET nav_enabled = false WHERE id = 0`.
    #[default(true)]
    pub nav_enabled: bool,
    // END-APPENDED: does THIS database host dungeon-instance POPULATIONS? Default `true`
    // = every single-database realm behaves exactly as it always has (the portal spawns the dungeon
    // where the player is standing). Set `false` on the OPEN-WORLD shard of a multi-database
    // deployment: `create_instance_with_id` then files the `game_instance` row +
    // binding as a LEASE and spawns nothing, so the world writer never pays for a dungeon whose run
    // happens on another database — the instances shard, where this stays `true`, spawns the
    // population when the gateway mirrors the id there via `ensure_instance`.
    //
    // Deliberately a WORLD POLICY, not a shard id: the module still knows nothing about shards;
    // it only knows whether it hosts instance populations — the `nav_enabled` precedent.
    // Operator-set, like `nav_enabled`: `UPDATE game_config SET hosts_instances = false WHERE id = 0`.
    // Deliberate simplification: one SQL update rather than a gateway→module
    // policy push. Ceiling: an operator who forgets it gets today's behavior (the dungeon spawns
    // on the world shard and is evicted after the transfer) — degraded, never broken. Upgrade
    // path: the gateway derives it from the shard map and asserts it at startup.
    #[default(true)]
    pub hosts_instances: bool,
    // END-APPENDED: park every Package bot where it was spawned — the GOAL and COMBAT brains
    // return immediately, so a crowd neither picks up quests nor grinds its way out of the zone. The
    // WANDER pass deliberately keeps running (6 yd hops around home), because a launch-day crowd
    // milling in a plaza is the movement load worth measuring. The load-test lever: bots that quest
    // and fight measure content, not capacity, and drag the zone's creatures into the number.
    // Operator-set like `nav_enabled`: `UPDATE game_config SET bots_idle = true WHERE id = 0`.
    #[default(false)]
    pub bots_idle: bool,

    #[default(false)]
    pub vmap_enabled: bool,
    // END-APPENDED: let vmap-DERIVED nav coverage join path planning (`nav::fetcher` merges it
    // into the imported grid). Separate from `vmap_enabled`, which gates the exact rays, and from
    // `nav_enabled`, which gates the imported grid itself. Coverage belongs to one vmap generation
    // and only an ACTIVE generation with a complete manifest is ever read, so flipping this on a
    // map without prepared coverage changes nothing. Default OFF: preparation is an operator
    // workflow rolled out per map, and this flag is the one-command rollback. Toggle:
    // `debug_set_nav_coverage_enabled` or
    // `UPDATE game_config SET nav_coverage_enabled = true WHERE id = 0`.
    #[default(false)]
    pub nav_coverage_enabled: bool,
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
/// (see `importer/src/dbc.rs::area_trigger_sql`). The geometric half of inn triggers
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

/// One `TaxiNodes.dbc` row: a named flight point and the two mount displays the 5875 client data
/// assigns to it. `id` is the server-side storage key; `client_node_id` is the unique one-based bit
/// position used by the vanilla 256-bit taxi mask. Imported rows use their DBC id for both, while
/// reserved fixtures retain high storage ids without leaking those values onto the wire. The DBC
/// mount array is Horde first, Alliance second. [static]
#[table(accessor = game_taxi_node, public)]
pub struct GameTaxiNode {
    #[primary_key]
    pub id: u32,
    #[unique]
    pub client_node_id: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub name: String,
    pub mount_display_horde: u32,
    pub mount_display_alliance: u32,
}

/// One directed `TaxiPath.dbc` route. A reverse flight exists only when the DBC contains a separate
/// row in the opposite direction. Its ordered geometry lives in `game_taxi_path_node`. [static]
#[table(
    accessor = game_taxi_path,
    public,
    index(accessor = by_source, btree(columns = [source_node_id])),
    index(accessor = by_route, btree(columns = [source_node_id, destination_node_id]))
)]
pub struct GameTaxiPath {
    #[primary_key]
    pub id: u32,
    pub source_node_id: u32,
    pub destination_node_id: u32,
    /// Copper, copied from `TaxiPath.dbc::cost` after rejecting negative values.
    pub fare: u32,
}

/// One `TaxiPathNode.dbc` row. `id` is the stable DBC key, while `(path_id, node_index)` is the
/// actual flight order. `flags` is the DBC's signed `int32` container, retained verbatim so all 32
/// flag bits survive (including the sign bit); consumers that interpret bits must view its bit
/// pattern as `u32`. Delay is also an `int32` in the vanilla DBC contract, but negative delays are
/// rejected by the importer because elapsed time cannot be negative. [static]
#[table(
    accessor = game_taxi_path_node,
    public,
    index(accessor = by_path_id, btree(columns = [path_id])),
    index(accessor = by_path, btree(columns = [path_id, node_index]))
)]
pub struct GameTaxiPathNode {
    #[primary_key]
    pub id: u32,
    pub path_id: u32,
    pub node_index: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub flags: i32,
    pub delay_ms: i32,
}

/// Restore the reserved catalogue and its map-0 flight master after the normal world ETL replaces
/// spatial fixture rows. The import script calls this only for a map-0 run, so restoring a test NPC
/// cannot contaminate a shard that owns another continent.
#[reducer]
pub fn restore_taxi_fixture(ctx: &ReducerContext) -> Result<(), String> {
    require_operator(ctx)?;
    crate::seed::seed_taxi_fixture(ctx);
    Ok(())
}

// ===========================================================================================
//  Realm address [static] — the one writer for the row above
// ===========================================================================================

/// The pure half of [`set_realm_address`]: `host:port`, trimmed, port in `1..=65535`. Blank is
/// refused rather than written — advertising nothing fails at realm select for every player at once.
pub fn validate_realm_address(raw: &str) -> Result<String, String> {
    let address = raw.trim();
    if address.is_empty() {
        return Err("realm address must not be blank".to_string());
    }
    let (host, port) = address
        .rsplit_once(':')
        .ok_or_else(|| format!("realm address must be host:port, got `{address}`"))?;
    if host.is_empty() {
        return Err(format!("realm address has no host: `{address}`"));
    }
    match port.parse::<u16>() {
        Ok(port) if port > 0 => Ok(address.to_string()),
        _ => Err(format!(
            "realm address port must be 1-65535, got `{port}` in `{address}`"
        )),
    }
}

/// Set the address the realm list advertises (operator-only, like [`crate::gm::set_gm_level`]).
/// Operator-gated because this decides where every client opens its world connection — a
/// player-callable version would let any client redirect the realm.
#[reducer]
pub fn set_realm_address(ctx: &ReducerContext, address: String) -> Result<(), String> {
    require_operator(ctx)?;
    let address = validate_realm_address(&address)?;
    let realms = ctx.db.game_realm();
    let mut realm = realms
        .iter()
        .next()
        .ok_or_else(|| "no game_realm row on this database".to_string())?;
    let previous = std::mem::replace(&mut realm.address, address.clone());
    realms.id().update(realm);
    log::info!("set_realm_address: {previous} -> {address}");
    Ok(())
}

#[cfg(test)]
mod realm_address_tests {
    use super::*;

    #[test]
    fn a_blank_address_is_refused_rather_than_advertising_nothing() {
        for blank in ["", "   ", "\t\n"] {
            assert!(
                validate_realm_address(blank).is_err(),
                "{blank:?} must be refused"
            );
        }
    }

    #[test]
    fn an_address_without_a_port_is_refused() {
        assert!(validate_realm_address("192.168.1.50").is_err());
        assert!(validate_realm_address("realm.example.com").is_err());
    }

    #[test]
    fn a_port_that_is_not_a_number_in_range_is_refused() {
        for bad in [
            "192.168.1.50:notaport",
            "192.168.1.50:0",
            "192.168.1.50:65536",
            "192.168.1.50:-1",
            "192.168.1.50:",
        ] {
            assert!(
                validate_realm_address(bad).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn an_address_without_a_host_is_refused() {
        assert!(validate_realm_address(":8085").is_err());
    }

    #[test]
    fn a_valid_address_is_accepted_and_trimmed() {
        assert_eq!(
            validate_realm_address("  159.69.88.70:8085\n"),
            Ok("159.69.88.70:8085".to_string())
        );
        assert_eq!(
            validate_realm_address("realm.example.com:8085"),
            Ok("realm.example.com:8085".to_string())
        );
        assert_eq!(
            validate_realm_address("[::1]:8085"),
            Ok("[::1]:8085".to_string())
        );
    }
}
