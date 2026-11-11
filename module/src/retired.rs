// Tables nothing writes. A publish that removes a table aborts, so their declarations stay.
use spacetimedb::{table, Identity, Timestamp};

#[table(accessor = game_gateway_lease)]
pub struct GatewayLease {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub identity: Identity,
    pub last_heartbeat: Timestamp,
}

#[table(accessor = game_gateway_session)]
pub struct GatewaySession {
    #[primary_key]
    pub entity_guid: u64,
    pub lease_id: u64,
}

#[table(accessor = game_account_fence, index(accessor = by_character, btree(columns = [character_guid])), index(accessor = by_closed, btree(columns = [closed])))]
pub struct AccountFence {
    #[primary_key]
    pub account_id: u64,
    pub account_name: String,
    pub generation: u64,
    pub request_nonce: u128,
    pub character_guid: u64,
    pub expires_micros: i64,
    pub closed: bool,
}

#[table(
    accessor = game_account_character_owner,
    index(accessor = by_account, btree(columns = [account_id]))
)]
pub struct AccountCharacterOwner {
    #[primary_key]
    pub character_guid: u64,
    pub account_id: u64,
    pub account_name: String,
}

#[table(accessor = game_guid_range_registry)]
pub struct GuidRangeAssignment {
    #[primary_key]
    pub shard_name: String,
    #[unique]
    pub slot: u32,
    pub base: u64,
    pub size: u64,
    pub assigned_micros: i64,
}

#[table(accessor = game_map_region)]
pub struct MapRegion {
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub gx_min: i32,
    pub gx_max: i32,
    pub gy_min: i32,
    pub gy_max: i32,
}

#[table(accessor = game_region_assignment)]
pub struct RegionAssignment {
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub shard: String,
    pub epoch: u64,
    pub updated_micros: i64,
}

#[table(accessor = game_shard_load)]
pub struct ShardLoad {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub shard: String,
    pub sampled_at_micros: i64,
    pub writer_occupancy_pct: f32,
    pub sessions: u32,
    #[default(0u64)]
    pub gateway_key: u64,
}

#[table(accessor = game_shard_load_total)]
pub struct ShardLoadTotal {
    #[primary_key]
    pub shard: String,
    pub sessions: u32,
    pub updated_at_micros: i64,
}

#[table(accessor = game_region_load)]
pub struct RegionLoad {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub sampled_at_micros: i64,
    pub players: u32,
}

#[table(accessor = game_bot_invite_intent, index(accessor = by_actor, btree(columns = [inviter_guid])))]
pub struct BotInviteIntent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub inviter_guid: u64,
    pub target_guid: u64,
    pub created_at: Timestamp,
    #[default(0u8)]
    pub op: u8,
}

#[table(accessor = game_channel_member, public, index(accessor = by_channel, btree(columns = [channel])))]
pub struct ChannelMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub character_guid: u64,
    pub owner_identity: Identity,
}

#[table(accessor = game_channel_event, public)]
pub struct ChannelEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel: String,
    pub channel_display: String,
    pub sender_guid: u64,
    pub message: String,
    pub created_at: Timestamp,
}

#[table(accessor = game_whisper_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct WhisperEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub other_guid: u64,
    pub is_inform: bool,
    pub message: String,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub recipient_guid: u64,
}

#[table(
    accessor = game_roll_event,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct RollEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub roller_guid: u64,
    pub min_roll: u32,
    pub max_roll: u32,
    pub result: u32,
    pub created_at: Timestamp,
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

#[table(accessor = game_guild_fee_decision)]
pub struct GuildFeeDecision {
    #[primary_key]
    pub operation_id: u64,
    pub payer_guid: u64,
    pub kind: u8,
    pub accepted: bool,
    pub refusal: String,
    pub petition_id: u32,
    pub decided_micros: i64,
}

#[table(accessor = game_auction_bid_decision)]
pub struct AuctionBidDecision {
    #[primary_key]
    pub operation_id: u64,
    pub bidder_guid: u64,
    pub auction_id: u32,
    pub offer: u32,
    pub outcome: u8,
    pub revision: u64,
    pub result_bidder_guid: u64,
    pub result_bid: u32,
    pub minimum_increment: u32,
    #[default(0)]
    pub deferred_refund: u32,
    #[default(0)]
    pub accepted_price: u32,
    pub house: u32,
    #[default(0u32)]
    pub item_entry: u32,
    #[default(0u32)]
    pub random_property_id: u32,
    #[default(0u8)]
    pub operation: u8,
}

#[table(accessor = game_loot_roll_promotion_receipt, index(accessor = by_source_slot, btree(columns = [promotion_source, corpse_guid, slot])))]
pub struct LootRollPromotionReceipt {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub promotion_source: Identity,
    pub corpse_guid: u64,
    pub slot: u8,
    pub source_roll_id: u64,
}

#[table(accessor = game_creature_move_event, public)]
pub struct CreatureMoveEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub mover_guid: u64,
    pub start_x: f32,
    pub start_y: f32,
    pub start_z: f32,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub duration_ms: u32,
    pub spline_id: u32,
    pub created_at: Timestamp,
    #[default(false)]
    pub run: bool,
}
