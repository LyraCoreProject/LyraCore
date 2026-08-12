//! Codec: map gateway-side row views to `wow_world_messages` (vanilla) messages.
//!
//! The byte layouts — including the update mask, packed guids, and the movement block — are
//! owned by `wow_world_messages`. This module is the thin mapping from the gateway's flattened
//! row views (`CharacterView`, `EntityView`) to the crate's typed messages: char enum, the
//! `CREATE_OBJECT2` self/peer spawn (built via the crate's `UpdatePlayer::builder()`, which
//! owns the descriptor bit indices), the post-login
//! sequence, the `MSG_MOVE_*` relay, and destroy. Header framing/cipher is delegated to
//! `wow_srp` by the message write methods.
//!
//! The production body is split into packet-family submodules (`char`, `entity`, `values`,
//! `corpse`, `loot`, `leveling`, `combat`, `npc`, `item`, `movement`) behind this thin facade,
//! which re-exports every previously-public symbol so external call sites keep reaching them as
//! `crate::codec::<sym>`. The split is pure code-motion; the shared `use` imports below stay here
//! and the submodules pull them in via `use super::*`. The crash-critical partial-VALUES builders
//! live (verbatim) in `values`.

pub mod addon;
pub mod social;
pub mod update_mask;

// Packet-family submodules (pure code-motion out of the old monolithic body).
mod char;
mod combat;
mod corpse;
mod entity;
mod gameobject;
mod item;
mod leveling;
mod loot;
mod movement;
mod npc;
mod quest;
mod trainer;
mod values;

pub use social::{
    build_channel_message, build_chat_message, build_emote_anim, build_friend_list_response,
    build_friend_status, build_gm_system_message, build_group_decline, build_group_invite,
    build_group_list, build_ignore_list_response, build_party_chat, build_party_command_result,
    build_random_roll, build_text_emote, build_whisper, build_who_response, FriendView,
    WhoPlayerView,
};

// Re-export the family submodules so every previously-public symbol stays reachable as
// `crate::codec::<sym>` (≈50 external call sites depend on this).
pub use char::*;
pub use combat::*;
pub use corpse::*;
pub use entity::*;
pub use gameobject::*;
pub use item::*;
pub use leveling::*;
pub use loot::*;
pub use movement::*;
pub use npc::*;
pub use quest::*;
pub use trainer::*;
pub use values::*;

// Shared imports for the family submodules. Kept at this (private) module level so each submodule
// glob-imports them via `use super::*`; the type aliases / row-view structs they build all live in
// the submodules themselves.
use anyhow::{anyhow, Result};
use lyracore_shared::constants::speeds;
use lyracore_shared::opcodes::movement as movement_opcodes;
use lyracore_shared::packing::unpack4;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{
    Area,
    BagFamily,
    // Item binding: "Binds when picked up/equipped" tooltip line.
    Bonding,
    BuyResult,
    Character,
    CharacterFlags,
    CharacterGear,
    Class,
    ClientMessage,
    CreatureFamily,
    DamageInfo,
    DateTime,
    Faction,
    FactionFlag,
    FactionInitializer,
    Gender,
    Gold,
    GossipItem,
    HitInfo,
    InitialSpell,
    InventoryType,
    ItemClassAndSubClass,
    ItemDamageType,
    // Item flags bitmask + bag-type restriction on the query response.
    ItemFlag,
    ItemQuality,
    ItemSlot,
    ItemSpells,
    ItemStat,
    ItemStatType,
    Level,
    LogoutResult,
    LogoutSpeed,
    MSG_CORPSE_QUERY_Server,
    MSG_MOVE_FALL_LAND_Server,
    MSG_MOVE_HEARTBEAT_Client,
    MSG_MOVE_HEARTBEAT_Server,
    MSG_MOVE_JUMP_Server,
    MSG_MOVE_SET_FACING_Server,
    MSG_MOVE_SET_RUN_MODE_Server,
    MSG_MOVE_SET_WALK_MODE_Server,
    MSG_MOVE_START_BACKWARD_Server,
    MSG_MOVE_START_FORWARD_Server,
    MSG_MOVE_START_STRAFE_LEFT_Server,
    MSG_MOVE_START_STRAFE_RIGHT_Server,
    MSG_MOVE_START_SWIM_Server,
    MSG_MOVE_START_TURN_LEFT_Server,
    MSG_MOVE_START_TURN_RIGHT_Server,
    MSG_MOVE_STOP_STRAFE_Server,
    MSG_MOVE_STOP_SWIM_Server,
    MSG_MOVE_STOP_Server,
    MSG_MOVE_STOP_TURN_Server,
    MSG_MOVE_TELEPORT_ACK_Server,
    Map,
    Month,
    MovementBlock,
    MovementBlock_MovementFlags,
    MovementBlock_UpdateFlag,
    MovementBlock_UpdateFlag_All,
    MovementBlock_UpdateFlag_Living,
    MovementInfo,
    MovementInfo_MovementFlags,
    NpcTextUpdate,
    NpcTextUpdateEmote,
    Object,
    ObjectType,
    PetCommandState,
    PetEnabled,
    PetReactState,
    Power,
    QuestCompletable,
    QuestDetailsEmote,
    QuestGiverStatus,
    QuestItem,
    QuestItemRequirement,
    QuestItemReward,
    Race,
    // Group loot methods: the need/greed vote enum shared by CMSG_LOOT_ROLL /
    // SMSG_LOOT_ROLL / SMSG_LOOT_ROLL_WON.
    RollVote,
    SMSG_CREATURE_QUERY_RESPONSE_found,
    SMSG_GAMEOBJECT_QUERY_RESPONSE_found,
    SMSG_ITEM_QUERY_SINGLE_RESPONSE_found,
    SMSG_LOG_XPGAIN_ExperienceAwardType,
    SMSG_MONSTER_MOVE_MonsterMoveType,
    SMSG_PET_SPELLS_action_bars,
    SMSG_SPELL_GO_CastFlags,
    SMSG_SPELL_GO_CastFlags_Ammo,
    SMSG_SPELL_START_CastFlags,
    SMSG_SPELL_START_CastFlags_Ammo,
    SheatheType,
    Skill,
    SkillInfo,
    SkillInfoIndex,
    SpellCastResult,
    SpellCastTargets,
    SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_Unit,
    SpellCooldownStatus,
    SpellMiss,
    SpellMissInfo,
    SpellSchool,
    SpellTriggerType,
    SplineFlag,
    TrainerSpell,
    TrainerSpellState,
    TrainingFailureReason,
    TransferAbortReason,
    UpdateContainer,
    UpdateCorpse,
    UpdateDynamicObject,
    UpdateGameObject,
    UpdateItem,
    UpdateMask,
    UpdatePlayer,
    UpdateUnit,
    Vector3d,
    // Items slice-2: equip → render on the 3D model via PLAYER_VISIBLE_ITEM.
    VisibleItem,
    VisibleItemIndex,
    Weekday,
    WorldResult,
    MSG_QUEST_PUSH_RESULT,
    SMSG_ACCOUNT_DATA_TIMES,
    SMSG_ACTION_BUTTONS,
    SMSG_ATTACKERSTATEUPDATE,
    SMSG_ATTACKSTART,
    SMSG_ATTACKSTOP,
    SMSG_BINDPOINTUPDATE,
    // Vendor buy failure feedback.
    SMSG_BUY_FAILED,
    SMSG_CHAR_CREATE,
    SMSG_CHAR_DELETE,
    SMSG_CHAR_ENUM,
    SMSG_CREATURE_QUERY_RESPONSE,
    SMSG_DESTROY_OBJECT,
    SMSG_FORCE_RUN_SPEED_CHANGE,
    SMSG_GAMEOBJECT_QUERY_RESPONSE,
    // Gossip (rank 12): the gossip-window message + its menu items + the NPC-text title round-trip.
    SMSG_GOSSIP_MESSAGE,
    SMSG_INITIALIZE_FACTIONS,
    SMSG_INITIAL_SPELLS,
    // Inspect: ack the target guid so the client opens the paperdoll window.
    SMSG_INSPECT,
    // Inventory-action failure feedback (equip / move / store / use rejection).
    SMSG_INVENTORY_CHANGE_FAILURE,
    // Items slice-1: the item-query response + the typed item CREATE_OBJECT descriptors.
    // UpdateContainer is the container (bag) analogue of UpdateItem — same ITEM+CONTAINER mask.
    SMSG_ITEM_QUERY_SINGLE_RESPONSE,
    SMSG_LEARNED_SPELL,
    SMSG_LEVELUP_INFO,
    SMSG_LOGIN_SETTIMESPEED,
    SMSG_LOGIN_VERIFY_WORLD,
    SMSG_LOGOUT_RESPONSE,
    SMSG_LOG_XPGAIN,
    SMSG_LOOT_MASTER_LIST,
    SMSG_LOOT_MONEY_NOTIFY,
    SMSG_LOOT_RELEASE_RESPONSE,
    SMSG_LOOT_REMOVED,
    SMSG_LOOT_ROLL,
    SMSG_LOOT_ROLL_WON,
    // Group loot methods: need/greed rolls + master looter.
    SMSG_LOOT_START_ROLL,
    SMSG_MONSTER_MOVE,
    SMSG_NAME_QUERY_RESPONSE,
    SMSG_NEW_WORLD,
    SMSG_NPC_TEXT_UPDATE,
    // Warlock pet UI: the pet action bar + enable state pushed at summon (guid-only form clears).
    SMSG_PET_SPELLS,
    // /played: total/level played-time reply.
    SMSG_PLAYED_TIME,
    SMSG_QUESTGIVER_OFFER_REWARD,
    SMSG_QUESTGIVER_QUEST_COMPLETE,
    SMSG_QUESTGIVER_QUEST_DETAILS,
    SMSG_QUESTGIVER_QUEST_LIST,
    SMSG_QUESTGIVER_REQUEST_ITEMS,
    // Quests (gateway wire): the questgiver-dialog packets + their list/reward sub-structs (all
    // gtker-typed — vanilla-complete, unlike the raw-encoded loot/vendor paths).
    SMSG_QUESTGIVER_STATUS,
    SMSG_QUESTUPDATE_ADD_KILL,
    // Timed quests + sharing: expiry feedback + the party-share push-result round trip.
    SMSG_QUESTUPDATE_FAILEDTIMER,
    // Resurrection accept-prompt handshake: the offer sent to a dead ally awaiting their
    // CMSG_RESURRECT_RESPONSE accept/decline.
    SMSG_RESURRECT_REQUEST,
    // NOTE: `SMSG_SET_FACTION_STANDING`/`FactionStanding` are deliberately absent — gtker writes the
    // faction ENTRY where 1.12 wants the rep INDEX, so that packet is RAW-encoded in `entity.rs`
    // (`build_faction_standing_raw`). Re-importing the typed pair would resurrect the McBride
    // client-crash (see `update_mask.rs`'s note).
    SMSG_SET_REST_START,
    // Banker activate + the BANKER gossip option: opens the bank window.
    SMSG_SHOW_BANK,
    // Floating spell damage number (the per-cast non-melee damage log).
    SMSG_SPELLNONMELEEDAMAGELOG,
    SMSG_SPELL_COOLDOWN,
    // Cast pushback: the client cast-bar slide signal on a direct-damage pushback hit.
    SMSG_SPELL_DELAYED,
    // Cast-interrupt signal: the cast-bar teardown relayed to the caster when its timed cast is cancelled.
    SMSG_SPELL_FAILURE,
    SMSG_SPELL_GO,
    SMSG_SPELL_START,
    SMSG_TRAINER_BUY_FAILED,
    SMSG_TRAINER_BUY_SUCCEEDED,
    // Class trainers: the trainer-window list + buy result + the live learned-spell push (all gtker-typed
    // vanilla — SMSG_TRAINER_LIST IS vanilla-complete in the crate, unlike the raw vendor list).
    SMSG_TRAINER_LIST,
    // The loud failure: a transfer that cannot proceed must abort the client's loading
    // screen, never leave it spinning.
    SMSG_TRANSFER_ABORTED,
    // Cross-map teleport arrival: loading-screen kickoff + the new-map position.
    SMSG_TRANSFER_PENDING,
    SMSG_TUTORIAL_FLAGS,
    // Buff/debuff timer (the UpdateMask aura array has no duration in 1.12 — sent separately).
    SMSG_UPDATE_AURA_DURATION,
    SMSG_UPDATE_OBJECT,
};
use wow_world_messages::Guid;

#[cfg(test)]
mod tests;

/// Generated-input (property) tests for the hand-rolled decoders — the client-byte parsers
/// and the update-mask encode/decode round trip. Kept apart from `tests` because it is a different
/// kind of assertion (a generated input space, over a fixed seed) and because it also hosts the
/// seeded generator the logon-framing property test reuses.
#[cfg(test)]
pub(crate) mod property_tests;
