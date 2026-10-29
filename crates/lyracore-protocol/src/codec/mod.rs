pub mod addon;
pub mod auth_session;
pub mod channel;
pub mod social;
pub mod update_mask;

pub mod auction;
pub mod breath;
pub mod char;
pub mod combat;
pub mod corpse;
pub mod duel;
pub mod entity;
pub mod gameobject;
pub mod guild;
pub mod item;
pub mod leveling;
pub mod loot;
pub mod mail;
pub mod meeting_stone;
pub mod member_stats;
pub mod movement;
pub mod movement_info;
pub mod npc;
pub mod pet;
pub mod quest;
pub mod taxi;
pub mod trade;
pub mod trainer;
pub mod values;
pub mod weather;

pub use social::{
    build_chat_message_to, build_emote_anim, build_friend_list_response, build_friend_status_raw,
    build_gm_system_message, build_group_decline, build_group_invite, build_group_list,
    build_group_list_raw, build_group_set_leader, build_ignore_list_response, build_minimap_ping,
    build_party_command_result, build_raid_group_only, build_random_roll, build_ready_check,
    build_ready_check_answer, build_realm_chat_line, build_target_icon_list_raw,
    build_target_icon_update, build_text_emote, build_who_response_raw, FriendOnline, FriendView,
    WhoPlayerView,
};

pub use auction::*;
pub use breath::*;
pub use char::*;
pub use combat::*;
pub use corpse::*;
pub use duel::*;
pub use entity::*;
pub use gameobject::*;
pub use guild::*;
pub use item::*;
pub use leveling::*;
pub use loot::*;
pub use mail::*;
pub use meeting_stone::*;
pub use member_stats::*;
pub use movement::*;
pub use movement_info::*;
pub use npc::*;
pub use pet::*;
pub use quest::*;
pub use taxi::*;
pub use trade::*;
pub use trainer::*;
pub use values::*;
pub use weather::*;

use anyhow::{anyhow, Result};
use lyracore_shared::constants::speeds;
use lyracore_shared::opcodes::movement as movement_opcodes;
use lyracore_shared::packing::unpack4;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{
    AllowedClass, AllowedRace, Area, BagFamily, Bonding, BuyBankSlotResult, BuyResult, Character,
    CharacterFlags, CharacterGear, Class, CreatureFamily, DamageInfo, DateTime, Faction,
    FactionFlag, FactionInitializer, Gender, Gold, GossipItem, HitInfo, InitialSpell,
    InventoryType, ItemClass, ItemClassAndSubClass, ItemDamageType, ItemFlag, ItemQuality,
    ItemSlot, ItemSpells, ItemStat, ItemStatType, Level, LogoutResult, LogoutSpeed,
    MSG_CORPSE_QUERY_Server, MSG_MOVE_FALL_LAND_Server, MSG_MOVE_HEARTBEAT_Server,
    MSG_MOVE_JUMP_Server, MSG_MOVE_SET_FACING_Server, MSG_MOVE_SET_RUN_MODE_Server,
    MSG_MOVE_SET_WALK_MODE_Server, MSG_MOVE_START_BACKWARD_Server, MSG_MOVE_START_FORWARD_Server,
    MSG_MOVE_START_STRAFE_LEFT_Server, MSG_MOVE_START_STRAFE_RIGHT_Server,
    MSG_MOVE_START_SWIM_Server, MSG_MOVE_START_TURN_LEFT_Server, MSG_MOVE_START_TURN_RIGHT_Server,
    MSG_MOVE_STOP_STRAFE_Server, MSG_MOVE_STOP_SWIM_Server, MSG_MOVE_STOP_Server,
    MSG_MOVE_STOP_TURN_Server, MSG_MOVE_TELEPORT_ACK_Server, Map, Month, MovementBlock,
    MovementBlock_MovementFlags, MovementBlock_MovementFlags_SplineEnabled,
    MovementBlock_SplineFlag, MovementBlock_UpdateFlag, MovementBlock_UpdateFlag_All,
    MovementBlock_UpdateFlag_Living, MovementBlock_UpdateFlag_Transport, MovementInfo,
    MovementInfo_MovementFlags, NpcTextUpdate, NpcTextUpdateEmote, Object, ObjectType,
    PetCommandState, PetEnabled, PetReactState, Power, QuestCompletable, QuestGiverStatus,
    QuestItem, QuestItemRequirement, QuestItemReward, Race, RollVote,
    SMSG_CREATURE_QUERY_RESPONSE_found, SMSG_GAMEOBJECT_QUERY_RESPONSE_found,
    SMSG_ITEM_QUERY_SINGLE_RESPONSE_found, SMSG_MONSTER_MOVE_MonsterMoveType,
    SMSG_PET_SPELLS_action_bars, SMSG_SPELL_GO_CastFlags, SMSG_SPELL_GO_CastFlags_Ammo,
    SMSG_SPELL_START_CastFlags, SMSG_SPELL_START_CastFlags_Ammo, SheatheType, Skill, SkillInfo,
    SkillInfoIndex, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_DestLocation, SpellCastTargets_SpellCastTargetFlags_Unit,
    SpellCooldownStatus, SpellMiss, SpellMissInfo, SpellSchool, SpellTriggerType, SplineFlag,
    TrainerSpell, TrainerSpellState, TrainingFailureReason, TransferAbortReason, UpdateContainer,
    UpdateCorpse, UpdateDynamicObject, UpdateGameObject, UpdateItem, UpdateMask, UpdatePlayer,
    UpdateUnit, Vector3d, VisibleItem, VisibleItemIndex, Weekday, WorldResult,
    MSG_QUEST_PUSH_RESULT, SMSG_ACCOUNT_DATA_TIMES, SMSG_ACTION_BUTTONS, SMSG_ATTACKERSTATEUPDATE,
    SMSG_ATTACKSTART, SMSG_ATTACKSTOP, SMSG_BINDPOINTUPDATE, SMSG_BUY_BANK_SLOT_RESULT,
    SMSG_BUY_FAILED, SMSG_CHAR_CREATE, SMSG_CHAR_DELETE, SMSG_CHAR_ENUM,
    SMSG_CREATURE_QUERY_RESPONSE, SMSG_DESTROY_OBJECT, SMSG_FORCE_RUN_SPEED_CHANGE,
    SMSG_GAMEOBJECT_QUERY_RESPONSE, SMSG_GOSSIP_MESSAGE, SMSG_INITIALIZE_FACTIONS,
    SMSG_INITIAL_SPELLS, SMSG_INSPECT, SMSG_INVENTORY_CHANGE_FAILURE,
    SMSG_ITEM_QUERY_SINGLE_RESPONSE, SMSG_LEARNED_SPELL, SMSG_LEVELUP_INFO,
    SMSG_LOGIN_SETTIMESPEED, SMSG_LOGIN_VERIFY_WORLD, SMSG_LOGOUT_RESPONSE, SMSG_LOOT_MASTER_LIST,
    SMSG_LOOT_MONEY_NOTIFY, SMSG_LOOT_RELEASE_RESPONSE, SMSG_LOOT_REMOVED, SMSG_LOOT_ROLL,
    SMSG_LOOT_ROLL_WON, SMSG_LOOT_START_ROLL, SMSG_MONSTER_MOVE, SMSG_NAME_QUERY_RESPONSE,
    SMSG_NEW_WORLD, SMSG_NPC_TEXT_UPDATE, SMSG_PET_SPELLS, SMSG_PLAYED_TIME,
    SMSG_QUESTGIVER_OFFER_REWARD, SMSG_QUESTGIVER_QUEST_COMPLETE, SMSG_QUESTGIVER_QUEST_LIST,
    SMSG_QUESTGIVER_REQUEST_ITEMS, SMSG_QUESTGIVER_STATUS, SMSG_QUESTUPDATE_ADD_KILL,
    SMSG_QUESTUPDATE_FAILEDTIMER, SMSG_SET_PROFICIENCY, SMSG_SET_REST_START, SMSG_SHOW_BANK,
    SMSG_SPELLNONMELEEDAMAGELOG, SMSG_SPELL_COOLDOWN, SMSG_SPELL_DELAYED, SMSG_SPELL_GO,
    SMSG_SPELL_START, SMSG_TRAINER_BUY_FAILED, SMSG_TRAINER_BUY_SUCCEEDED, SMSG_TRAINER_LIST,
    SMSG_TRANSFER_PENDING, SMSG_TUTORIAL_FLAGS, SMSG_UPDATE_AURA_DURATION, SMSG_UPDATE_OBJECT,
};
use wow_world_messages::Guid;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod property_tests;
