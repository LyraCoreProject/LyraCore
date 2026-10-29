use spacetimedb::{table, Identity};

// The named constant avoids unary-expression precedence in the table macro's default serializer.
const NO_WATCHED_FACTION: i32 = -1;

#[table(accessor = game_character, public, index(accessor = by_account, btree(columns = [account_id])))]
pub struct Character {
    #[primary_key]
    pub guid: u64,
    pub account_id: u64,
    pub owner_identity: Identity,
    #[unique]
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub level: u8,
    pub xp: u32,
    pub next_level_xp: u32,
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub first_login: bool,
    pub online: bool,
    #[default(0)]
    pub money: u32,
    #[default(0)]
    pub rested_xp: u32,
    #[default(0u64)]
    pub last_logout_micros: u64,
    #[default(0u32)]
    pub home_map: u32,
    #[default(0u32)]
    pub home_zone: u32,
    #[default(0.0f32)]
    pub home_x: f32,
    #[default(0.0f32)]
    pub home_y: f32,
    #[default(0.0f32)]
    pub home_z: f32,
    #[default(0)]
    pub played_total_secs: u32,
    #[default(0u64)]
    pub session_start_micros: u64,
    #[default(0u32)]
    pub health: u32,
    // Health zero, rather than power zero, identifies an uninitialized vitals snapshot.
    #[default(0u32)]
    pub power: u32,
    #[default(0u32)]
    pub respec_count: u32,
    #[default(0i64)]
    pub death_expire_micros: i64,

    #[default(0u64)]
    pub pending_instance_id: u64,
    // Legacy: debug_repair_after_publish moves a nonzero value onto the Account and zeroes it.
    #[default(0)]
    pub gm_level: u8,

    #[default(false)]
    pub pending_ghost: bool,
    #[default(false)]
    pub resting: bool,
    #[default(0u64)]
    pub rested_since_micros: u64,
    #[default(false)]
    pub pending_godmode: bool,

    #[default(10000)]
    pub pending_run_speed_mult_bp: u32,
    #[default(0)]
    pub bank_bag_slots: u8,
    #[default(NO_WATCHED_FACTION)]
    pub watched_faction_index: i32,
    #[default(0u8)]
    pub rest_state: u8,
}
