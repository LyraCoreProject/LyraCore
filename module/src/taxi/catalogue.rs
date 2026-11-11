use spacetimedb::table;

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

#[derive(Clone)]
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
