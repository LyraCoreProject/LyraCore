use spacetimedb::{log, reducer, table, ReducerContext, Table};

use crate::helpers::require_operator;

#[table(accessor = game_realm, public)]
pub struct Realm {
    #[primary_key]
    pub id: u8,
    pub name: String,
    pub address: String,
    pub realm_type: u32,
    pub flags: u8,
    pub population: f32,
    pub timezone: u8,
}

#[table(accessor = game_start_position, public)]
pub struct StartPosition {
    #[primary_key]
    pub race_class: u16,
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

#[table(accessor = game_config, public)]
pub struct ServerConfig {
    #[primary_key]
    pub id: u32,
    pub xp_rate: f32,
    #[default(true)]
    pub nav_enabled: bool,
    #[default(true)]
    pub hosts_instances: bool,
    #[default(false)]
    pub bots_idle: bool,

    #[default(false)]
    pub vmap_enabled: bool,
    #[default(false)]
    pub nav_coverage_enabled: bool,
}

#[table(accessor = game_start_item, public, index(accessor = by_race_class, btree(columns = [race_class])))]
pub struct StartItem {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub race_class: u16,
    pub item_entry: u32,
}

#[table(accessor = game_race_info, public)]
pub struct RaceInfo {
    #[primary_key]
    pub race: u8,
    pub male_display: u32,
    pub female_display: u32,
    pub faction_template: u32,
}

#[table(accessor = game_char_base_info, public)]
pub struct CharBaseInfo {
    #[primary_key]
    pub race_class: u16,
    pub race: u8,
    pub class: u8,
}

#[table(accessor = game_map, public)]
pub struct GameMap {
    #[primary_key]
    pub id: u32,
    pub internal_name: String,
    pub name: String,
    pub map_type: u32,
    pub linked_zone: u32,
    pub parent: u32,
    #[default(0u32)]
    pub player_limit: u32,
    #[default(0u32)]
    pub reset_delay: u32,
    #[default({ -1i32 })]
    pub ghost_entrance_map: i32,
    #[default(0.0f32)]
    pub ghost_entrance_x: f32,
    #[default(0.0f32)]
    pub ghost_entrance_y: f32,
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

#[table(accessor = game_tavern_area_trigger, public)]
pub struct GameTavernAreaTrigger {
    #[primary_key]
    pub trigger_id: u32,
}

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
    pub fare: u32,
}

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

#[reducer]
pub fn restore_taxi_fixture(ctx: &ReducerContext) -> Result<(), String> {
    require_operator(ctx)?;
    crate::seed::seed_taxi_fixture(ctx);
    Ok(())
}

pub(crate) fn validate_realm_address(raw: &str) -> Result<String, String> {
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

#[reducer]
pub fn provision_realm(
    ctx: &ReducerContext,
    id: u8,
    name: String,
    address: String,
    realm_type: u32,
    timezone: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let address = validate_realm_address(&address)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("realm name must not be blank".to_string());
    }
    let realms = ctx.db.game_realm();
    let realm = match realms.id().find(id) {
        Some(existing) => Realm {
            name: name.to_string(),
            address,
            realm_type,
            timezone,
            ..existing
        },
        None => Realm {
            id,
            name: name.to_string(),
            address,
            realm_type,
            flags: 0,
            population: 0.0,
            timezone,
        },
    };
    log::info!(
        "provision_realm: {} {} at {}",
        realm.id,
        realm.name,
        realm.address
    );
    realms.id().delete(id);
    realms.insert(realm);
    Ok(())
}

#[cfg(test)]
mod realm_address_tests {
    use super::*;

    #[test]
    fn a_blank_address_is_refused_rather_than_advertising_nothing() {
        for blank in ["", "   ", "\t\n"] {
            assert_eq!(
                validate_realm_address(blank),
                Err("realm address must not be blank".to_string())
            );
        }
    }

    #[test]
    fn an_address_without_a_port_is_refused() {
        assert_eq!(
            validate_realm_address("192.168.1.50"),
            Err("realm address must be host:port, got `192.168.1.50`".to_string())
        );
        assert_eq!(
            validate_realm_address("realm.example.com"),
            Err("realm address must be host:port, got `realm.example.com`".to_string())
        );
    }

    #[test]
    fn a_port_that_is_not_a_number_in_range_is_refused() {
        for port in ["notaport", "0", "65536", "-1", ""] {
            assert_eq!(
                validate_realm_address(&format!("192.168.1.50:{port}")),
                Err(format!(
                    "realm address port must be 1-65535, got `{port}` in `192.168.1.50:{port}`"
                ))
            );
        }
    }

    #[test]
    fn an_address_without_a_host_is_refused() {
        assert_eq!(
            validate_realm_address(":8085"),
            Err("realm address has no host: `:8085`".to_string())
        );
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
