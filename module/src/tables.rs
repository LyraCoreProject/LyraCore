//! The table catalog: every core table accessor and row type, named by its owning family, plus a
//! row field type where a Package builds that row. The crate root re-exports this list for the
//! schema parity test and the Package API.

pub use crate::account_ownership::{
    game_account_character_owner, game_account_claim, game_account_fence, AccountCharacterOwner,
    AccountClaim, AccountFence,
};
pub use crate::action_bar::{
    game_createinfo_action, game_player_action, CreateinfoAction, PlayerAction,
};
pub use crate::auction::{
    game_auction, game_auction_bid_decision, game_auction_bid_hold, game_auction_expiry,
    game_auction_hold, game_auction_house, game_auction_notice, game_auction_operation_receipt,
    Auction, AuctionBidDecision, AuctionBidHold, AuctionExpiry, AuctionHold,
    AuctionHouseDefinition, AuctionNotice, AuctionOperationReceipt,
};
pub use crate::auth::{
    game_account, game_alpha_test_tools_enrollment, game_guid_allocator, game_guid_range,
    game_operator, game_session, game_session_reaper_schedule, Account, AlphaTestToolsEnrollment,
    GuidAllocator, GuidRange, Operator, Session, SessionReaperSchedule,
};
pub use crate::away::{game_character_away, CharacterAway};
pub use crate::breath::{game_breath_schedule, game_breath_state, BreathSchedule, BreathState};
pub use crate::breath_relay::{game_breath_relay_event, BreathRelayEvent};
pub use crate::bridge::{
    game_addon_message, game_party_command_dispatch_lane, game_party_command_intent,
    game_party_command_issuer, game_party_command_receipt, AddonMessage, PartyCommandDispatchLane,
    PartyCommandIntent, PartyCommandIssuer, PartyCommandReceipt,
};
pub use crate::channel::{
    game_chat_channel, game_chat_channel_ban, game_chat_channel_member,
    game_chat_channel_notice_event, ChatChannel, ChatChannelBan, ChatChannelMember,
    ChatChannelNoticeEvent,
};
pub use crate::character::{game_character, Character};
pub use crate::chat::{
    game_channel_event, game_channel_member, game_character_contact, game_chat_event,
    game_emote_event, game_roll_event, game_system_message_event, game_whisper_event, ChannelEvent,
    ChannelMember, ChatEvent, ContactEntry, EmoteEvent, RollEvent, SystemMessageEvent,
    WhisperEvent,
};
pub use crate::combat::death::{game_creature_lethal_damage_floor, CreatureLethalDamageFloor};
pub use crate::combat::{
    game_combat_event, game_melee_attack, game_melee_schedule, game_ranged_impact_schedule,
    CombatEvent, MeleeAttack, MeleeSchedule, RangedImpactSchedule,
};
pub use crate::combo::{game_combo_point, ComboPoint};
pub use crate::config::{
    game_area, game_area_trigger, game_char_base_info, game_config, game_race_info, game_realm,
    game_start_item, game_start_position, game_taxi_node, game_taxi_path, game_taxi_path_node,
    CharBaseInfo, GameArea, GameAreaTrigger, GameTaxiNode, GameTaxiPath, GameTaxiPathNode,
    RaceInfo, Realm, ServerConfig, StartItem, StartPosition,
};
pub use crate::corpse::{game_corpse, Corpse};
pub use crate::creatures::distraction::{game_creature_distraction, CreatureDistraction};
pub use crate::creatures::{
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
    CreatureRelayTemporaryFaction, CreatureSpawn, CreatureSpell, CreatureSpline, CreatureTemplate,
    CreatureWaypoint, GossipMenu, GossipMenuProfile, GossipMenuProfileOption, GossipOption,
    HunterPet, HunterPetProtocol, LivePetKind, NpcText, NpcTextSlot, PetCareSchedule, PetCommand,
    RelayArrival, RelayContinuation, RelayDefinition, RelayRun,
};
#[cfg(feature = "debug_reducers")]
pub use crate::debug::{
    game_catalogue_fingerprint, game_debug_readout, CatalogueFingerprint, DebugReadout,
};
pub use crate::duel::{
    game_duel, game_duel_event, game_duel_schedule, Duel, DuelEvent, DuelSchedule,
};
pub use crate::encounter::{
    game_encounter_equip, game_encounter_hp_watch, game_encounter_spawn, game_encounter_state,
    EncounterEquip, EncounterHpWatch, EncounterSpawn, EncounterState,
};
pub use crate::exploration::{game_character_explored, CharacterExplored};
pub use crate::faction::{game_faction, game_faction_template, Faction, FactionTemplate};
pub use crate::gameobject::{
    game_gameobject, game_gameobject_pool, game_gameobject_pool_member, game_gameobject_template,
    game_gameobject_trap, game_gameobject_trap_cooldown, game_gameobject_unlocked, game_lock,
    GameLock, GameObject, GameObjectPool, GameObjectPoolMember, GameObjectTemplate, GameObjectTrap,
    GameObjectTrapCooldown, GameObjectUnlocked,
};
pub use crate::gc::{game_event_reaper_schedule, EventReaperSchedule};
pub use crate::gm::{game_world_config, GmWorldConfig};
pub use crate::go_collider::{game_go_collider, GoCollider};
pub use crate::go_model::{game_go_model, GoModel};
pub use crate::graveyard::{game_graveyard, game_graveyard_zone, GraveyardLoc, GraveyardZone};
pub use crate::group::{
    game_bot_invite_intent, game_group, game_group_event, game_group_invite, game_group_member,
    game_group_member_partition, game_group_roster_revision, game_group_target_icon,
    BotInviteIntent, Group, GroupEvent, GroupInvite, GroupMember, GroupMemberPartition,
    GroupRosterRevision, GroupTargetIcon, PartyPartitionState,
};
pub use crate::guild::fee::{
    game_guild_fee_decision, game_guild_fee_hold, GuildFeeDecision, GuildFeeHold,
};
pub use crate::guild::membership::{game_guild_invite, GuildInvite};
pub use crate::guild::petition::{
    game_guild_petition, game_guild_petition_signature, GuildPetition, GuildPetitionSignature,
};
pub use crate::guild::{
    game_guild, game_guild_event, game_guild_member, game_guild_rank, Guild, GuildEvent,
    GuildMember, GuildRank,
};
pub use crate::gw::{
    game_gateway_lease, game_gateway_lease_reaper_schedule, game_gateway_session, GatewayLease,
    GatewayLeaseReaperSchedule, GatewaySession,
};
pub use crate::import_meta::{game_import_meta, ImportMeta};
pub use crate::instance::{
    game_instance, game_instance_binding, game_instance_reaper_schedule, game_instance_removal,
    GameInstance, GameInstanceBinding, InstanceReaperSchedule, InstanceRemoval,
};
pub use crate::items::{
    game_character_buyback, game_item_enchantment, game_item_instance, game_item_property_weight,
    game_item_random_property, game_item_template, game_npc_vendor, BuybackEntry, ItemEnchantment,
    ItemInstance, ItemPropertyWeight, ItemRandomProperty, ItemTemplate, NpcVendor,
};
pub use crate::load::{
    game_region_load, game_shard_load, game_shard_load_total, RegionLoad, ShardLoad, ShardLoadTotal,
};
pub use crate::loot::{
    game_corpse_loot, game_corpse_loot_eligible, game_creature_loot, game_creature_loot_tag_group,
    game_creature_quest_tap, game_creature_quest_tap_member, game_fishing_loot,
    game_gameobject_loot, game_loot_roll, game_loot_roll_promotion_receipt, game_loot_roll_vote,
    game_pickpocket_loot, game_skinning_loot, CorpseLoot, CorpseLootEligible, CreatureLoot,
    CreatureLootTagGroup, CreatureQuestTap, CreatureQuestTapMember, GameFishingLoot,
    GameObjectLoot, GamePickpocketLoot, GameSkinningLoot, LootRoll, LootRollPromotionReceipt,
    LootRollVote,
};
pub use crate::mail::{game_mail, Mail};
pub use crate::mail_catalogue::{
    game_mail_loot, game_mail_template, game_quest_reward_mail, MailLoot, MailTemplate,
    QuestRewardMail,
};
pub use crate::mail_escrow::{
    game_mail_delivery, game_mail_escrow, game_mail_escrow_reaper_schedule, MailDelivery,
    MailEscrow, MailEscrowReaperSchedule,
};
pub use crate::mail_text::{game_item_text, ItemText};
pub use crate::mail_timer::{game_mail_arrival, game_mail_timer, MailArrival, MailTimer};
pub use crate::meeting_stone::{
    game_meeting_stone, game_meeting_stone_party, game_meeting_stone_reminder_schedule,
    game_meeting_stone_seeker, MeetingStone, MeetingStoneParty, MeetingStoneReminderSchedule,
    MeetingStoneSeeker,
};
pub use crate::motion::{
    game_entity_motion_pending, game_motion_publish_schedule, MotionPublishSchedule, PendingMotion,
};
pub use crate::nav::{game_nav_chunk, game_navigation_revision, NavChunk, NavigationRevision};
pub use crate::package_account::{game_package_account, PackageAccount};
pub use crate::package_config::{game_package_config, PackageConfig};
pub use crate::package_import::{game_package_import, PackageImport};
pub use crate::package_teardown::{game_package_teardown, PackageTeardown};
pub use crate::quest::{
    game_areatrigger_teleport, game_character_quest, game_character_quest_event_credit,
    game_creature_quest, game_gameobject_quest, game_quest_cast_objective,
    game_quest_event_requirement, game_quest_objective, game_quest_reward_choice,
    game_quest_reward_item, game_quest_reward_spell, game_quest_template, game_quest_text,
    AreatriggerTeleport, CharacterQuest, CharacterQuestEventCredit, CreatureQuest, GameObjectQuest,
    QuestCastObjective, QuestEventRequirement, QuestObjective, QuestRewardChoice, QuestRewardItem,
    QuestRewardSpell, QuestTemplate, QuestText,
};
pub use crate::realm_chat::{game_realm_chat_event, RealmChatEvent};
pub use crate::realm_core::{
    game_character_shard, game_guid_range_registry, CharacterShard, GuidRangeAssignment,
};
pub use crate::region::{game_map_region, game_region_assignment, MapRegion, RegionAssignment};
pub use crate::reputation::{game_player_reputation, PlayerReputation};
pub use crate::rest::{game_rest_state_event, RestStateEvent};
pub use crate::script_binding::{game_script, Script};
pub use crate::sessionless::{game_sessionless_action_consent, SessionlessActionConsent};
pub use crate::skill::{game_player_skill, PlayerSkill};
pub use crate::skilldata::{
    game_skill_ability, game_skill_availability, game_skill_line, SkillAbility, SkillAvailability,
    SkillLine,
};
pub use crate::spell::cast::resolve::{
    game_creature_dead_callback_cast_admission, CreatureDeadCallbackCastAdmission,
};
pub use crate::spell::stacking::{
    game_dr_state, game_spell_group, game_spell_group_rule, DrState, SpellGroup, SpellGroupRule,
};
pub use crate::spell::{
    game_aura, game_aura_schedule, game_createinfo_spell, game_dynamic_object, game_ground_area,
    game_ground_area_schedule, game_pending_cast, game_pending_spell_impact, game_player_spell,
    game_resurrect_request, game_school_lockout, game_self_resurrect_option, game_spell,
    game_spell_cast_event, game_spell_cd, game_spell_chain, game_spell_cooldown, game_spell_effect,
    game_spell_impact_event, game_spell_learn, game_spell_proc_event, game_spell_reagent, Aura,
    AuraSchedule, CreateinfoSpell, DynamicObject, GroundArea, GroundAreaSchedule, PendingCast,
    PendingSpellImpact, PlayerSpell, ResurrectRequest, SchoolLockout, SelfResurrectOption, Spell,
    SpellCastEvent, SpellCd, SpellChain, SpellCooldown, SpellEffect, SpellImpactEvent, SpellLearn,
    SpellProcEvent, SpellReagent,
};
pub use crate::stats::{game_class_level_stats, game_level_stats, ClassLevelStats, LevelStats};
pub use crate::talent::{
    game_character_talent, game_talent, game_talent_tab, CharacterTalent, Talent, TalentTab,
};
pub use crate::taxi::{
    game_active_taxi_flight, game_character_taxi_node, game_taxi_flight_schedule,
    game_taxi_passenger_spline, game_taxi_service_reply, ActiveTaxiFlight, CharacterTaxiNode,
    TaxiFlightSchedule, TaxiPassengerSpline, TaxiServiceReply,
};
pub use crate::terrain::{game_terrain_chunk, TerrainChunk};
pub use crate::threat::{game_taunt_lock, game_threat, TauntLock, ThreatEntry};
pub use crate::trade::{
    game_trade_event, game_trade_session, game_trade_slot, TradeEvent, TradeSession, TradeSlot,
};
pub use crate::trainer::{game_trainer_spell, TrainerSpell};
pub use crate::transfer::{
    game_bot_transfer_intent, game_transfer_in, game_transfer_out, game_transfer_reaper_schedule,
    BotTransferIntent, TransferIn, TransferOut, TransferReaperSchedule,
};
pub use crate::vmap::{
    game_vmap_chunk, game_vmap_generation, game_vmap_generation_chunk,
    game_vmap_generation_receipt, game_vmap_indoor_cell, game_vmap_nav_coverage,
    game_vmap_nav_coverage_manifest, VmapChunk, VmapGeneration, VmapGenerationChunk,
    VmapGenerationReceipt, VmapIndoorCell, VmapNavCoverage, VmapNavCoverageManifest,
};
pub use crate::weather::{
    game_weather, game_weather_schedule, game_zone_weather, WeatherSchedule, ZoneWeather,
    ZoneWeatherChance,
};
pub use crate::world::{
    game_entity_motion, game_movement_violation, game_teleport_event, game_world_entity,
    game_world_state, game_world_state_name, EntityMotion, MovementViolation, TeleportEvent,
    WorldEntity, WorldState, WorldStateName,
};
pub use crate::xp::{game_levelup_event, game_xp_event, LevelupEvent, XpEvent};
