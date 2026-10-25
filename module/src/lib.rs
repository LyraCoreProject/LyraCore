#[macro_export]
macro_rules! character_owned {
    (delete, fn $name:ident($ctx:ident, $guid:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext, $guid: u64) $body
    };
    (restamp, fn $name:ident($ctx:ident, $guid:ident, $identity:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $identity: spacetimedb::Identity,
        ) $body
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        by = $index:ident,
        remint = $pk:ident $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$index().filter(&$guid).collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, mut row| {
                    // Surrogate keys are Shard-local; mint new keys at the destination.
                    row.$pk = 0;
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        by = $index:ident,
        keep_key $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$index().filter(&$guid).collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, row| {
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        primary_key = $key:ident $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$key().find($guid).into_iter().collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, row| {
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (not_transported, fn $name:ident()) => {
        pub(crate) fn $name(
            _ctx: &spacetimedb::ReducerContext,
            _character_guid: u64,
            io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::not_transported(io);
        }
    };
}

#[macro_export]
macro_rules! game_tick_pass {
    (fn $name:ident($ctx:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext) $body
    };
}

#[macro_export]
macro_rules! game_hook {
    ($event:ident, fn $name:ident($ctx:ident, $payload:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $payload: &$crate::hooks::payload_for::$event,
        ) $body
    };
}

#[macro_export]
macro_rules! game_package_characters {
    (fn $name:ident($ctx:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext) -> Vec<u64> $body
    };
}

#[macro_export]
macro_rules! game_client_command {
    ($parse:ident, $apply:ident, $reply:ident) => {};
}

#[macro_export]
macro_rules! encounter_package {
    ($binding:ident, fn $name:ident($ctx:ident, $instance_id:ident, $signal:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $instance_id: u64,
            $signal: $crate::encounter::EncounterSignal,
        ) -> Result<(), String> $body
    };
}

include!(concat!(env!("OUT_DIR"), "/character_sweeps.rs"));

include!(concat!(env!("OUT_DIR"), "/package_mods.rs"));

include!(concat!(env!("OUT_DIR"), "/package_registries.rs"));

mod account_ownership;
mod action_bar;
mod actor;
mod auction;
mod auth;
mod away;
mod breath;
mod breath_relay;
mod bridge;
mod fatigue;

mod channel;
mod character;
mod chat;
mod combat;
mod config;
mod corpse;
mod creatures;
mod exploration;

#[cfg(feature = "debug_reducers")]
mod debug;

mod duel;
mod encounter;
mod faction;
mod gameobject;
mod gc;
mod gm;

mod go_collider;
#[cfg(feature = "debug_reducers")]
mod go_collider_fixture;
mod go_model;
mod gossip;
mod graveyard;
pub use tables::{game_gossip_condition, GossipCondition};
mod group;
pub mod guild;
mod gw;
mod helpers;
pub mod hooks;
mod import_meta;

mod combo;
mod ghost;
mod instance;
mod items;
mod load;
mod loot;

mod mail;

mod mail_catalogue;

mod mail_escrow;

mod mail_reward;

mod mail_text;

mod mail_timer;

mod meeting_stone;

mod motion;

mod mount;
mod nav;

mod operations;
mod operator_content;

mod package;

mod package_account;

mod package_config;
#[cfg(feature = "debug_reducers")]
pub(crate) mod package_fixture;

mod package_import;

mod package_teardown;

#[cfg(test)]
pub(crate) mod package_test;
mod professions;

#[cfg(test)]
mod publish_safety;
mod quest;

mod realm_chat;
mod realm_core;
mod refusal;
pub(crate) use refusal::Refusal;
mod region;
mod reputation;
mod rest;

mod runtime_script;

mod script_binding;
mod seed;
mod sessionless;
mod skill;
mod skilldata;
mod spell;
mod stats;
mod tables;
mod talent;
mod taxi;
mod terrain;
#[cfg(test)]
mod test_scan;
mod threat;
mod trade;
mod trainer;
mod transfer;
mod transport;

mod vmap;

#[cfg(test)]
mod tripwires;

mod aoi;
mod native;
mod weather;
mod world;
mod xp;

pub use tables::{
    game_account, game_alpha_test_tools_enrollment, game_guid_allocator, game_guid_range,
    game_operator, game_session, game_session_reaper_schedule, Account, AlphaTestToolsEnrollment,
    GuidAllocator, GuidRange, Operator, Session, SessionReaperSchedule,
};
pub use tables::{
    game_account_character_owner, game_account_claim, game_account_fence, AccountCharacterOwner,
    AccountClaim, AccountFence,
};
pub use tables::{
    game_active_taxi_flight, game_character_taxi_node, game_taxi_flight_schedule,
    game_taxi_passenger_spline, game_taxi_service_reply, ActiveTaxiFlight, CharacterTaxiNode,
    TaxiFlightSchedule, TaxiPassengerSpline, TaxiServiceReply,
};
pub use tables::{
    game_addon_message, game_party_command_dispatch_lane, game_party_command_intent,
    game_party_command_issuer, game_party_command_receipt, AddonMessage, PartyCommandDispatchLane,
    PartyCommandIntent, PartyCommandIssuer, PartyCommandReceipt,
};
pub use tables::{
    game_area, game_area_trigger, game_char_base_info, game_config, game_race_info, game_realm,
    game_start_item, game_start_position, game_taxi_node, game_taxi_path, game_taxi_path_node,
    CharBaseInfo, GameArea, GameAreaTrigger, GameTaxiNode, GameTaxiPath, GameTaxiPathNode,
    RaceInfo, Realm, ServerConfig, StartItem, StartPosition,
};
pub use tables::{
    game_areatrigger_teleport, game_character_quest, game_character_quest_event_credit,
    game_creature_quest, game_gameobject_quest, game_quest_cast_objective,
    game_quest_event_requirement, game_quest_objective, game_quest_reward_choice,
    game_quest_reward_item, game_quest_reward_spell, game_quest_template, game_quest_text,
    AreatriggerTeleport, CharacterQuest, CharacterQuestEventCredit, CreatureQuest, GameObjectQuest,
    QuestCastObjective, QuestEventRequirement, QuestObjective, QuestRewardChoice, QuestRewardItem,
    QuestRewardSpell, QuestTemplate, QuestText,
};
pub use tables::{
    game_auction, game_auction_bid_decision, game_auction_bid_hold, game_auction_expiry,
    game_auction_hold, game_auction_house, game_auction_notice, game_auction_operation_receipt,
    Auction, AuctionBidDecision, AuctionBidHold, AuctionExpiry, AuctionHold,
    AuctionHouseDefinition, AuctionNotice, AuctionOperationReceipt,
};
pub use tables::{
    game_aura, game_aura_schedule, game_createinfo_spell, game_dynamic_object, game_ground_area,
    game_ground_area_schedule, game_pending_cast, game_pending_spell_impact, game_player_spell,
    game_resurrect_request, game_school_lockout, game_self_resurrect_option, game_spell,
    game_spell_cast_event, game_spell_cast_event_live, game_spell_cd, game_spell_chain,
    game_spell_cooldown, game_spell_effect, game_spell_impact_event, game_spell_impact_event_live,
    game_spell_learn, game_spell_proc_event, game_spell_reagent, Aura, AuraSchedule,
    CreateinfoSpell, DynamicObject, GroundArea, GroundAreaSchedule, PendingCast,
    PendingSpellImpact, PlayerSpell, ResurrectRequest, SchoolLockout, SelfResurrectOption, Spell,
    SpellCastEvent, SpellCd, SpellChain, SpellCooldown, SpellEffect, SpellImpactEvent, SpellLearn,
    SpellProcEvent, SpellReagent,
};
pub use tables::{
    game_bot_invite_intent, game_group, game_group_event, game_group_invite, game_group_member,
    game_group_member_partition, game_group_roster_revision, game_group_target_icon,
    BotInviteIntent, Group, GroupEvent, GroupInvite, GroupMember, GroupMemberPartition,
    GroupRosterRevision, GroupTargetIcon, PartyPartitionState,
};
pub use tables::{
    game_bot_transfer_intent, game_transfer_in, game_transfer_out, game_transfer_reaper_schedule,
    BotTransferIntent, TransferIn, TransferOut, TransferReaperSchedule,
};
pub use tables::{game_breath_relay_event, BreathRelayEvent};
pub use tables::{game_breath_schedule, game_breath_state, BreathSchedule, BreathState};
#[cfg(feature = "debug_reducers")]
pub use tables::{
    game_catalogue_fingerprint, game_debug_readout, CatalogueFingerprint, DebugReadout,
};
pub use tables::{
    game_channel_event, game_channel_member, game_character_contact, game_chat_event,
    game_chat_event_live, game_emote_event, game_emote_event_live, game_roll_event,
    game_system_message_event, game_whisper_event, ChannelEvent, ChannelMember, ChatEvent,
    ContactEntry, EmoteEvent, RollEvent, SystemMessageEvent, WhisperEvent,
};
pub use tables::{game_character, Character};
pub use tables::{game_character_away, CharacterAway};
pub use tables::{
    game_character_buyback, game_item_enchantment, game_item_instance, game_item_property_weight,
    game_item_random_property, game_item_template, game_npc_vendor, BuybackEntry, ItemEnchantment,
    ItemInstance, ItemPropertyWeight, ItemRandomProperty, ItemTemplate, NpcVendor,
};
pub use tables::{game_character_explored, CharacterExplored};
pub use tables::{
    game_character_shard, game_guid_range_registry, CharacterShard, GuidRangeAssignment,
};
pub use tables::{
    game_character_talent, game_talent, game_talent_tab, CharacterTalent, Talent, TalentTab,
};
pub use tables::{
    game_chat_channel, game_chat_channel_ban, game_chat_channel_member,
    game_chat_channel_notice_event, ChatChannel, ChatChannelBan, ChatChannelMember,
    ChatChannelNoticeEvent,
};
pub use tables::{game_class_level_stats, game_level_stats, ClassLevelStats, LevelStats};
pub use tables::{
    game_combat_event, game_combat_event_live, game_melee_attack, game_melee_schedule,
    game_ranged_impact_schedule, CombatEvent, MeleeAttack, MeleeSchedule, RangedImpactSchedule,
};
pub use tables::{game_combo_point, ComboPoint};
pub use tables::{game_corpse, Corpse};
pub use tables::{
    game_corpse_loot, game_corpse_loot_eligible, game_creature_loot, game_creature_loot_tag_group,
    game_creature_quest_tap, game_creature_quest_tap_member, game_fishing_loot,
    game_gameobject_loot, game_loot_roll, game_loot_roll_promotion_receipt, game_loot_roll_vote,
    game_pickpocket_loot, game_skinning_loot, CorpseLoot, CorpseLootEligible, CreatureLoot,
    CreatureLootTagGroup, CreatureQuestTap, CreatureQuestTapMember, GameFishingLoot,
    GameObjectLoot, GamePickpocketLoot, GameSkinningLoot, LootRoll, LootRollPromotionReceipt,
    LootRollVote,
};
pub use tables::{game_createinfo_action, game_player_action, CreateinfoAction, PlayerAction};
pub use tables::{
    game_creature_ai_broadcast_text, game_creature_ai_definition, game_creature_ai_event,
    game_creature_ai_forced_despawn, game_creature_ai_movement_intent,
    game_creature_ai_movement_path_waypoint, game_creature_ai_relay_arrival,
    game_creature_ai_relay_continuation, game_creature_ai_relay_definition,
    game_creature_ai_relay_run, game_creature_ai_reset_deferral, game_creature_ai_returning_home,
    game_creature_ai_rule_state, game_creature_ai_spell_metadata, game_creature_ai_state,
    game_creature_ai_summon, game_creature_ai_summon_expiry, game_creature_ai_summon_origin,
    game_creature_cast, game_creature_family, game_creature_gossip_menu_override,
    game_creature_move_event, game_creature_move_schedule, game_creature_presentation,
    game_creature_relay_temporary_faction, game_creature_spawn, game_creature_spell,
    game_creature_spline, game_creature_template, game_creature_waypoint, game_gossip_menu,
    game_gossip_menu_profile, game_gossip_menu_profile_option, game_gossip_option, game_hunter_pet,
    game_hunter_pet_protocol, game_live_pet_kind, game_npc_text, game_npc_text_slot,
    game_pet_care_schedule, game_pet_command, CreatureAiBroadcastText, CreatureAiDefinition,
    CreatureAiEvent, CreatureAiForcedDespawn, CreatureAiMovementIntent,
    CreatureAiMovementPathWaypoint, CreatureAiResetDeferral, CreatureAiReturningHome,
    CreatureAiRuleState, CreatureAiSpellMetadata, CreatureAiState, CreatureAiSummon,
    CreatureAiSummonExpiry, CreatureAiSummonOrigin, CreatureCast, CreatureFamily,
    CreatureGossipMenuOverride, CreatureMoveEvent, CreatureMoveSchedule, CreaturePresentation,
    CreatureRelayTemporaryFaction, CreatureSpawn, CreatureSpawnMap, CreatureSpell, CreatureSpline,
    CreatureTemplate, CreatureWaypoint, GossipMenu, GossipMenuProfile, GossipMenuProfileOption,
    GossipOption, HunterPet, HunterPetProtocol, LivePetKind, NpcText, NpcTextSlot, PetCareSchedule,
    PetCommand, RelayArrival, RelayContinuation, RelayDefinition, RelayRun,
};
pub use tables::{game_creature_dead_callback_cast_admission, CreatureDeadCallbackCastAdmission};
pub use tables::{game_creature_distraction, CreatureDistraction};
pub use tables::{game_creature_lethal_damage_floor, CreatureLethalDamageFloor};
pub use tables::{
    game_dr_state, game_spell_group, game_spell_group_rule, DrState, SpellGroup, SpellGroupRule,
};
pub use tables::{game_duel, game_duel_event, game_duel_schedule, Duel, DuelEvent, DuelSchedule};
pub use tables::{
    game_encounter_equip, game_encounter_hp_watch, game_encounter_spawn, game_encounter_state,
    EncounterEquip, EncounterHpWatch, EncounterSpawn, EncounterState,
};
pub use tables::{
    game_entity_motion, game_movement_violation, game_teleport_event, game_world_entity,
    game_world_state, game_world_state_name, EntityMotion, MovementViolation, TeleportEvent,
    WorldEntity, WorldState, WorldStateName,
};
pub use tables::{
    game_entity_motion_pending, game_motion_publish_schedule, MotionPublishSchedule, PendingMotion,
};
pub use tables::{game_event_reaper_schedule, EventReaperSchedule};
pub use tables::{game_faction, game_faction_template, Faction, FactionTemplate};
pub use tables::{
    game_gameobject, game_gameobject_pool, game_gameobject_pool_member, game_gameobject_template,
    game_gameobject_trap, game_gameobject_trap_cooldown, game_gameobject_unlocked, game_lock,
    GameLock, GameObject, GameObjectPool, GameObjectPoolMember, GameObjectTemplate, GameObjectTrap,
    GameObjectTrapCooldown, GameObjectUnlocked,
};
pub use tables::{
    game_gateway_lease, game_gateway_lease_reaper_schedule, game_gateway_session, GatewayLease,
    GatewayLeaseReaperSchedule, GatewaySession,
};
pub use tables::{game_go_collider, GoCollider};
pub use tables::{game_go_model, GoModel};
pub use tables::{game_graveyard, game_graveyard_zone, GraveyardLoc, GraveyardZone};
pub use tables::{
    game_guild, game_guild_event, game_guild_member, game_guild_rank, Guild, GuildEvent,
    GuildMember, GuildRank,
};
pub use tables::{game_guild_fee_decision, game_guild_fee_hold, GuildFeeDecision, GuildFeeHold};
pub use tables::{game_guild_invite, GuildInvite};
pub use tables::{
    game_guild_petition, game_guild_petition_signature, GuildPetition, GuildPetitionSignature,
};
pub use tables::{game_import_meta, game_world_import, ImportMeta, WorldImport};
pub use tables::{
    game_instance, game_instance_binding, game_instance_reaper_schedule, game_instance_removal,
    GameInstance, GameInstanceBinding, InstanceReaperSchedule, InstanceRemoval,
};
pub use tables::{game_item_text, ItemText};
pub use tables::{game_levelup_event, game_xp_event, LevelupEvent, XpEvent};
pub use tables::{game_mail, Mail};
pub use tables::{game_mail_arrival, game_mail_timer, MailArrival, MailTimer};
pub use tables::{
    game_mail_delivery, game_mail_escrow, game_mail_escrow_reaper_schedule, MailDelivery,
    MailEscrow, MailEscrowReaperSchedule,
};
pub use tables::{
    game_mail_loot, game_mail_template, game_quest_reward_mail, MailLoot, MailTemplate,
    QuestRewardMail,
};
pub use tables::{game_map_region, game_region_assignment, MapRegion, RegionAssignment};
pub use tables::{
    game_meeting_stone, game_meeting_stone_party, game_meeting_stone_reminder_schedule,
    game_meeting_stone_seeker, MeetingStone, MeetingStoneParty, MeetingStoneReminderSchedule,
    MeetingStoneSeeker,
};
pub use tables::{game_nav_chunk, game_navigation_revision, NavChunk, NavigationRevision};
pub use tables::{game_package_account, PackageAccount};
pub use tables::{game_package_config, PackageConfig};
pub use tables::{game_package_import, PackageImport};
pub use tables::{game_package_teardown, PackageTeardown};
pub use tables::{game_player_reputation, PlayerReputation};
pub use tables::{game_player_skill, PlayerSkill};
pub use tables::{game_realm_chat_event, RealmChatEvent};
pub use tables::{
    game_region_load, game_shard_load, game_shard_load_total, RegionLoad, ShardLoad, ShardLoadTotal,
};
pub use tables::{game_rest_state_event, RestStateEvent};
pub use tables::{game_script, Script};
pub use tables::{game_sessionless_action_consent, SessionlessActionConsent};
pub use tables::{
    game_skill_ability, game_skill_availability, game_skill_line, SkillAbility, SkillAvailability,
    SkillLine,
};
pub use tables::{game_taunt_lock, game_threat, TauntLock, ThreatEntry};
pub use tables::{game_terrain_chunk, TerrainChunk};
pub use tables::{
    game_trade_event, game_trade_session, game_trade_slot, TradeEvent, TradeSession, TradeSlot,
};
pub use tables::{game_trainer_spell, TrainerSpell};
pub use tables::{
    game_vmap_chunk, game_vmap_generation, game_vmap_generation_chunk,
    game_vmap_generation_receipt, game_vmap_indoor_cell, game_vmap_nav_coverage,
    game_vmap_nav_coverage_manifest, VmapChunk, VmapGeneration, VmapGenerationChunk,
    VmapGenerationReceipt, VmapIndoorCell, VmapNavCoverage, VmapNavCoverageManifest,
};
pub use tables::{
    game_weather, game_weather_schedule, game_zone_weather, WeatherSchedule, ZoneWeather,
    ZoneWeatherChance,
};
pub use tables::{game_world_config, GmWorldConfig};

pub(crate) const EVENT_TTL_MICROS: i64 = 1_000_000;

pub(crate) const INVITE_TTL_MICROS: i64 = 120 * 1_000_000;
