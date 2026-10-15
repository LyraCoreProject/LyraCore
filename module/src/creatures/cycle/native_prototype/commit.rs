use spacetimedb::SpacetimeType;

pub const SNAPSHOT_PREFIX: &str = "NATIVE_COMMITTED_SNAPSHOT ";
pub const STATE_TABLE: &str = "game_native_benchmark_state";
pub const GAMEPLAY_TABLES: &[&str] = &[
    "game_world_entity",
    "game_creature_spline",
    "game_melee_attack",
    "game_creature_ai_movement_intent",
];

#[derive(Clone, SpacetimeType)]
pub struct RowChange {
    pub table: String,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

#[derive(Clone, SpacetimeType)]
pub struct Batch {
    pub run: u64,
    pub sequence: u64,
    pub at_micros: u64,
    pub changes: Vec<RowChange>,
}
