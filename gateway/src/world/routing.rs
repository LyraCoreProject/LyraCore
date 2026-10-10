//! Opcode ownership: each client opcode belongs to at most one Protocol Family.

use wow_world_messages::vanilla::opcodes::ClientOpcodeMessage as M;

/// A Protocol Family: the opcodes one handler owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// Character select, world entry and logout. These stay World Session operations.
    Session,
    Melee,
    Cast,
    Combat,
    Loot,
    Death,
    Npc,
    Auction,
    Vendor,
    Bank,
    Trainer,
    Item,
    Quest,
    Taxi,
    MemberStats,
    Social,
    Trade,
    Duel,
    Chat,
    Channel,
    MeetingStone,
    Guild,
    Mail,
    Movement,
    /// Services this realm does not provide, and client acknowledgements it does not need.
    Unavailable,
}

/// The Protocol Family that owns `msg`, or `None` when no family handles its opcode.
///
/// Each opcode appears once. A second owner is an unreachable pattern, and the deny below fails
/// the build. The read loop answers a few opcodes before they reach here: Addon Bridge frames,
/// `CMSG_AUCTION_LIST_ITEMS`, `CMSG_SET_FACTION_ATWAR` and the raw unavailable layouts.
#[deny(unreachable_patterns)]
#[allow(clippy::too_many_lines)] // One arm per Protocol Family.
pub(crate) fn owner(msg: &M) -> Option<Family> {
    Some(match msg {
        M::CMSG_PING(_)
        | M::CMSG_QUERY_TIME
        | M::CMSG_CHAR_ENUM
        | M::CMSG_CHAR_CREATE(_)
        | M::CMSG_CHAR_DELETE(_)
        | M::CMSG_PLAYER_LOGIN(_)
        | M::MSG_MOVE_WORLDPORT_ACK
        | M::CMSG_LOGOUT_CANCEL
        | M::CMSG_LOGOUT_REQUEST
        | M::CMSG_PLAYER_LOGOUT
        | M::CMSG_PLAYED_TIME => Family::Session,

        M::CMSG_ATTACKSWING(_) | M::CMSG_ATTACKSTOP => Family::Melee,

        M::CMSG_CAST_SPELL(_)
        | M::CMSG_CANCEL_AUTO_REPEAT_SPELL
        | M::CMSG_CANCEL_CAST(_)
        | M::CMSG_CANCEL_CHANNELLING(_)
        | M::CMSG_CANCEL_AURA(_) => Family::Cast,

        M::CMSG_SET_SELECTION(_)
        | M::CMSG_PET_ACTION(_)
        | M::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_)
        | M::CMSG_SETSHEATHED(_) => Family::Combat,

        M::CMSG_GAMEOBJ_USE(_)
        | M::CMSG_LOOT(_)
        | M::CMSG_LOOT_MONEY
        | M::CMSG_AUTOSTORE_LOOT_ITEM(_)
        | M::CMSG_LOOT_RELEASE(_)
        | M::CMSG_LOOT_ROLL(_)
        | M::CMSG_LOOT_MASTER_GIVE(_) => Family::Loot,

        M::CMSG_REPOP_REQUEST
        | M::MSG_CORPSE_QUERY
        | M::CMSG_RECLAIM_CORPSE(_)
        | M::CMSG_RESURRECT_RESPONSE(_)
        | M::CMSG_SELF_RES
        | M::CMSG_SPIRIT_HEALER_ACTIVATE(_) => Family::Death,

        M::CMSG_BINDER_ACTIVATE(_)
        | M::MSG_TALENT_WIPE_CONFIRM(_)
        | M::CMSG_NAME_QUERY(_)
        | M::CMSG_PET_NAME_QUERY(_)
        | M::CMSG_INSPECT(_)
        | M::CMSG_CREATURE_QUERY(_)
        | M::CMSG_GOSSIP_HELLO(_)
        | M::CMSG_NPC_TEXT_QUERY(_)
        | M::CMSG_GOSSIP_SELECT_OPTION(_)
        | M::CMSG_ITEM_QUERY_SINGLE(_)
        | M::CMSG_TEXT_EMOTE(_)
        | M::MSG_RANDOM_ROLL(_)
        | M::CMSG_AREATRIGGER(_)
        | M::CMSG_GAMEOBJECT_QUERY(_) => Family::Npc,

        // The read loop answers `CMSG_AUCTION_LIST_ITEMS` from its raw body first.
        M::MSG_AUCTION_HELLO(_)
        | M::CMSG_AUCTION_LIST_ITEMS(_)
        | M::CMSG_AUCTION_LIST_OWNER_ITEMS(_)
        | M::CMSG_AUCTION_LIST_BIDDER_ITEMS(_)
        | M::CMSG_AUCTION_PLACE_BID(_)
        | M::CMSG_AUCTION_SELL_ITEM(_)
        | M::CMSG_AUCTION_REMOVE_ITEM(_) => Family::Auction,

        M::CMSG_LIST_INVENTORY(_)
        | M::CMSG_BUY_ITEM(_)
        | M::CMSG_REPAIR_ITEM(_)
        | M::CMSG_SELL_ITEM(_)
        | M::CMSG_BUYBACK_ITEM(_) => Family::Vendor,

        M::CMSG_BANKER_ACTIVATE(_)
        | M::CMSG_AUTOBANK_ITEM(_)
        | M::CMSG_BUY_BANK_SLOT(_)
        | M::CMSG_AUTOSTORE_BANK_ITEM(_) => Family::Bank,

        M::CMSG_TRAINER_LIST(_)
        | M::CMSG_TRAINER_BUY_SPELL(_)
        | M::CMSG_SET_ACTION_BUTTON(_)
        | M::CMSG_LEARN_TALENT(_) => Family::Trainer,

        M::CMSG_DESTROYITEM(_)
        | M::CMSG_SPLIT_ITEM(_)
        | M::CMSG_AUTOEQUIP_ITEM(_)
        | M::CMSG_AUTOSTORE_BAG_ITEM(_)
        | M::CMSG_SWAP_INV_ITEM(_)
        | M::CMSG_SWAP_ITEM(_)
        | M::CMSG_USE_ITEM(_) => Family::Item,

        M::CMSG_QUESTGIVER_STATUS_QUERY(_)
        | M::CMSG_QUESTGIVER_HELLO(_)
        | M::CMSG_QUESTGIVER_QUERY_QUEST(_)
        | M::CMSG_QUEST_QUERY(_)
        | M::CMSG_QUESTGIVER_ACCEPT_QUEST(_)
        | M::CMSG_QUESTGIVER_COMPLETE_QUEST(_)
        | M::CMSG_QUESTGIVER_REQUEST_REWARD(_)
        | M::CMSG_QUESTGIVER_CHOOSE_REWARD(_)
        | M::CMSG_QUESTLOG_REMOVE_QUEST(_)
        | M::CMSG_PUSHQUESTTOPARTY(_) => Family::Quest,

        M::CMSG_TAXINODE_STATUS_QUERY(_)
        | M::CMSG_TAXIQUERYAVAILABLENODES(_)
        | M::CMSG_ACTIVATETAXI(_) => Family::Taxi,

        M::CMSG_REQUEST_PARTY_MEMBER_STATS(_) => Family::MemberStats,

        M::CMSG_WHO(_)
        | M::CMSG_FRIEND_LIST
        | M::CMSG_ADD_FRIEND(_)
        | M::CMSG_ADD_IGNORE(_)
        | M::CMSG_DEL_FRIEND(_)
        | M::CMSG_DEL_IGNORE(_)
        | M::CMSG_GROUP_INVITE(_)
        | M::CMSG_GROUP_ACCEPT
        | M::CMSG_GROUP_DECLINE
        | M::CMSG_GROUP_DISBAND
        | M::CMSG_GROUP_UNINVITE(_)
        | M::CMSG_LOOT_METHOD(_)
        | M::CMSG_GROUP_RAID_CONVERT
        | M::CMSG_GROUP_SET_LEADER(_)
        | M::CMSG_GROUP_ASSISTANT_LEADER(_)
        | M::CMSG_GROUP_UNINVITE_GUID(_)
        | M::CMSG_GROUP_CHANGE_SUB_GROUP(_)
        | M::CMSG_GROUP_SWAP_SUB_GROUP(_)
        | M::MSG_RAID_READY_CHECK(_)
        | M::MSG_RAID_TARGET_UPDATE(_)
        | M::MSG_MINIMAP_PING(_) => Family::Social,

        M::CMSG_INITIATE_TRADE(_)
        | M::CMSG_BEGIN_TRADE
        | M::CMSG_CANCEL_TRADE
        | M::CMSG_SET_TRADE_ITEM(_)
        | M::CMSG_CLEAR_TRADE_ITEM(_)
        | M::CMSG_SET_TRADE_GOLD(_)
        | M::CMSG_ACCEPT_TRADE(_)
        | M::CMSG_UNACCEPT_TRADE
        | M::CMSG_BUSY_TRADE
        | M::CMSG_IGNORE_TRADE => Family::Trade,

        M::CMSG_DUEL_ACCEPTED(_) | M::CMSG_DUEL_CANCELLED(_) => Family::Duel,

        // Every chat kind. Addon Bridge frames leave in the read loop first.
        M::CMSG_MESSAGECHAT(_) | M::CMSG_CHAT_IGNORED(_) => Family::Chat,

        M::CMSG_JOIN_CHANNEL(_)
        | M::CMSG_LEAVE_CHANNEL(_)
        | M::CMSG_CHANNEL_PASSWORD(_)
        | M::CMSG_CHANNEL_SET_OWNER(_)
        | M::CMSG_CHANNEL_MODERATOR(_)
        | M::CMSG_CHANNEL_UNMODERATOR(_)
        | M::CMSG_CHANNEL_MUTE(_)
        | M::CMSG_CHANNEL_UNMUTE(_)
        | M::CMSG_CHANNEL_KICK(_)
        | M::CMSG_CHANNEL_BAN(_)
        | M::CMSG_CHANNEL_UNBAN(_)
        | M::CMSG_CHANNEL_INVITE(_)
        | M::CMSG_CHANNEL_ANNOUNCEMENTS(_)
        | M::CMSG_CHANNEL_MODERATE(_)
        | M::CMSG_CHANNEL_LIST(_)
        | M::CMSG_CHANNEL_OWNER(_) => Family::Channel,

        M::CMSG_MEETINGSTONE_JOIN(_)
        | M::CMSG_MEETINGSTONE_LEAVE
        | M::CMSG_MEETINGSTONE_INFO
        | M::MSG_LOOKING_FOR_GROUP => Family::MeetingStone,

        M::CMSG_GUILD_QUERY(_)
        | M::CMSG_GUILD_ROSTER
        | M::CMSG_GUILD_INFO
        | M::CMSG_GUILD_CREATE(_)
        | M::MSG_TABARDVENDOR_ACTIVATE(_)
        | M::MSG_SAVE_GUILD_EMBLEM(_)
        | M::CMSG_GUILD_INVITE(_)
        | M::CMSG_GUILD_ACCEPT
        | M::CMSG_GUILD_DECLINE
        | M::CMSG_GUILD_LEAVE
        | M::CMSG_GUILD_REMOVE(_)
        | M::CMSG_GUILD_PROMOTE(_)
        | M::CMSG_GUILD_DEMOTE(_)
        | M::CMSG_GUILD_LEADER(_)
        | M::CMSG_GUILD_DISBAND
        | M::CMSG_GUILD_MOTD(_)
        | M::CMSG_GUILD_INFO_TEXT(_)
        | M::CMSG_GUILD_SET_PUBLIC_NOTE(_)
        | M::CMSG_GUILD_SET_OFFICER_NOTE(_)
        | M::CMSG_GUILD_RANK(_)
        | M::CMSG_GUILD_ADD_RANK(_)
        | M::CMSG_GUILD_DEL_RANK
        | M::CMSG_PETITION_SHOWLIST(_)
        | M::CMSG_PETITION_BUY(_)
        | M::CMSG_PETITION_SHOW_SIGNATURES(_)
        | M::CMSG_PETITION_QUERY(_)
        | M::MSG_PETITION_RENAME(_)
        | M::CMSG_OFFER_PETITION(_)
        | M::CMSG_PETITION_SIGN(_)
        | M::MSG_PETITION_DECLINE(_)
        | M::CMSG_TURN_IN_PETITION(_) => Family::Guild,

        M::CMSG_GET_MAIL_LIST(_)
        | M::MSG_QUERY_NEXT_MAIL_TIME
        | M::CMSG_ITEM_TEXT_QUERY(_)
        | M::CMSG_MAIL_MARK_AS_READ(_)
        | M::CMSG_MAIL_DELETE(_)
        | M::CMSG_MAIL_RETURN_TO_SENDER(_)
        | M::CMSG_MAIL_TAKE_MONEY(_)
        | M::CMSG_MAIL_TAKE_ITEM(_)
        | M::CMSG_SEND_MAIL(_)
        | M::CMSG_MAIL_CREATE_TEXT_ITEM(_) => Family::Mail,

        // `codec::relayed_move_opcode` maps exactly these.
        M::MSG_MOVE_START_FORWARD(_)
        | M::MSG_MOVE_START_BACKWARD(_)
        | M::MSG_MOVE_STOP(_)
        | M::MSG_MOVE_START_STRAFE_LEFT(_)
        | M::MSG_MOVE_START_STRAFE_RIGHT(_)
        | M::MSG_MOVE_STOP_STRAFE(_)
        | M::MSG_MOVE_JUMP(_)
        | M::MSG_MOVE_START_TURN_LEFT(_)
        | M::MSG_MOVE_START_TURN_RIGHT(_)
        | M::MSG_MOVE_STOP_TURN(_)
        | M::MSG_MOVE_SET_RUN_MODE(_)
        | M::MSG_MOVE_SET_WALK_MODE(_)
        | M::MSG_MOVE_FALL_LAND(_)
        | M::MSG_MOVE_START_SWIM(_)
        | M::MSG_MOVE_STOP_SWIM(_)
        | M::MSG_MOVE_SET_FACING(_)
        | M::MSG_MOVE_HEARTBEAT(_) => Family::Movement,

        // Control receipts the client sends after a server-side movement change.
        M::MSG_MOVE_TELEPORT_ACK(_)
        | M::CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK(_)
        | M::CMSG_FORCE_SWIM_SPEED_CHANGE_ACK(_)
        | M::CMSG_FORCE_MOVE_ROOT_ACK(_)
        | M::CMSG_FORCE_MOVE_UNROOT_ACK(_)
        | M::CMSG_MOVE_KNOCK_BACK_ACK(_)
        | M::CMSG_MOVE_HOVER_ACK(_)
        | M::CMSG_MOVE_FEATHER_FALL_ACK(_)
        | M::CMSG_MOVE_WATER_WALK_ACK(_)
        | M::CMSG_FORCE_WALK_SPEED_CHANGE_ACK(_)
        | M::CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK(_)
        | M::CMSG_FORCE_TURN_RATE_CHANGE_ACK(_)
        | M::CMSG_MOVE_SPLINE_DONE(_)
        | M::CMSG_MOVE_TIME_SKIPPED(_)
        | M::CMSG_MOVE_NOT_ACTIVE_MOVER(_)
        | M::CMSG_NEXT_CINEMATIC_CAMERA
        | M::CMSG_COMPLETE_CINEMATIC
        | M::CMSG_SET_ACTIVE_MOVER(_)
        // Services this realm does not provide.
        | M::CMSG_STANDSTATECHANGE(_)
        | M::CMSG_SET_ACTIONBAR_TOGGLES(_)
        | M::CMSG_TUTORIAL_FLAG(_)
        | M::CMSG_TUTORIAL_CLEAR
        | M::CMSG_TUTORIAL_RESET
        | M::CMSG_GMTICKET_SYSTEMSTATUS
        | M::CMSG_GMTICKET_GETTICKET
        | M::CMSG_GMTICKET_DELETETICKET
        | M::MSG_LIST_STABLED_PETS(_)
        | M::CMSG_STABLE_PET(_)
        | M::CMSG_UNSTABLE_PET(_)
        | M::CMSG_BUY_STABLE_SLOT(_)
        | M::CMSG_STABLE_SWAP_PET(_)
        | M::CMSG_OPEN_ITEM(_)
        | M::CMSG_WRAP_ITEM(_)
        | M::CMSG_BATTLEFIELD_LIST(_)
        | M::CMSG_BATTLEFIELD_STATUS
        | M::CMSG_LEAVE_BATTLEFIELD(_)
        | M::CMSG_BATTLEMASTER_JOIN(_)
        | M::CMSG_BATTLEFIELD_PORT(_)
        | M::CMSG_BATTLEMASTER_HELLO(_)
        | M::CMSG_AREA_SPIRIT_HEALER_QUERY(_)
        | M::CMSG_AREA_SPIRIT_HEALER_QUEUE(_)
        | M::MSG_PVP_LOG_DATA
        | M::MSG_BATTLEGROUND_PLAYER_POSITIONS
        | M::CMSG_PET_SET_ACTION(_)
        | M::CMSG_PET_ABANDON(_)
        | M::CMSG_PET_RENAME(_)
        | M::CMSG_PET_STOP_ATTACK(_)
        | M::CMSG_PET_CANCEL_AURA(_)
        | M::CMSG_PET_UNLEARN(_)
        | M::CMSG_PET_SPELL_AUTOCAST(_)
        | M::CMSG_UNLEARN_SKILL(_)
        | M::CMSG_SET_AMMO(_)
        | M::CMSG_MOUNTSPECIAL_ANIM
        | M::CMSG_TOGGLE_PVP(_)
        | M::CMSG_TOGGLE_HELM
        | M::CMSG_TOGGLE_CLOAK
        | M::CMSG_FAR_SIGHT(_)
        | M::CMSG_SUMMON_RESPONSE(_)
        | M::CMSG_QUEST_CONFIRM_ACCEPT(_)
        | M::MSG_QUEST_PUSH_RESULT(_)
        | M::CMSG_RESET_INSTANCES
        | M::CMSG_REQUEST_RAID_INFO
        | M::MSG_INSPECT_HONOR_STATS(_)
        | M::CMSG_BUY_ITEM_IN_SLOT(_) => Family::Unavailable,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::{
        CMSG_MESSAGECHAT_ChatType, Language, MSG_AUCTION_HELLO_Client, MSG_MOVE_HEARTBEAT_Client,
        MovementInfo, MovementInfo_MovementFlags, Vector3d, CMSG_AUTOSTORE_BAG_ITEM,
        CMSG_BANKER_ACTIVATE, CMSG_CHANNEL_LIST, CMSG_DUEL_CANCELLED, CMSG_GAMEOBJ_USE,
        CMSG_LIST_INVENTORY, CMSG_MESSAGECHAT, CMSG_NAME_QUERY, CMSG_QUESTGIVER_HELLO,
        CMSG_REQUEST_PARTY_MEMBER_STATS, CMSG_SET_SELECTION, CMSG_TAXINODE_STATUS_QUERY,
        CMSG_TRAINER_LIST,
    };
    use wow_world_messages::Guid;

    const ALL: [Family; 25] = [
        Family::Session,
        Family::Melee,
        Family::Cast,
        Family::Combat,
        Family::Loot,
        Family::Death,
        Family::Npc,
        Family::Auction,
        Family::Vendor,
        Family::Bank,
        Family::Trainer,
        Family::Item,
        Family::Quest,
        Family::Taxi,
        Family::MemberStats,
        Family::Social,
        Family::Trade,
        Family::Duel,
        Family::Chat,
        Family::Channel,
        Family::MeetingStone,
        Family::Guild,
        Family::Mail,
        Family::Movement,
        Family::Unavailable,
    ];

    /// One opcode each family owns. The match is exhaustive, so a new family needs one here.
    fn representative(family: Family) -> M {
        let guid = Guid::new(42);
        match family {
            Family::Session => M::CMSG_QUERY_TIME,
            Family::Melee => M::CMSG_ATTACKSTOP,
            Family::Cast => M::CMSG_CANCEL_AUTO_REPEAT_SPELL,
            Family::Combat => M::CMSG_SET_SELECTION(CMSG_SET_SELECTION { target: guid }),
            Family::Loot => M::CMSG_GAMEOBJ_USE(CMSG_GAMEOBJ_USE { guid }),
            Family::Death => M::CMSG_REPOP_REQUEST,
            Family::Npc => M::CMSG_NAME_QUERY(CMSG_NAME_QUERY { guid }),
            Family::Auction => M::MSG_AUCTION_HELLO(MSG_AUCTION_HELLO_Client { auctioneer: guid }),
            Family::Vendor => M::CMSG_LIST_INVENTORY(CMSG_LIST_INVENTORY { guid }),
            Family::Bank => M::CMSG_BANKER_ACTIVATE(CMSG_BANKER_ACTIVATE { guid }),
            Family::Trainer => M::CMSG_TRAINER_LIST(CMSG_TRAINER_LIST { guid }),
            Family::Item => M::CMSG_AUTOSTORE_BAG_ITEM(CMSG_AUTOSTORE_BAG_ITEM {
                source_bag: 255,
                source_slot: 23,
                destination_bag: 255,
            }),
            Family::Quest => M::CMSG_QUESTGIVER_HELLO(CMSG_QUESTGIVER_HELLO { guid }),
            Family::Taxi => M::CMSG_TAXINODE_STATUS_QUERY(CMSG_TAXINODE_STATUS_QUERY { guid }),
            Family::MemberStats => {
                M::CMSG_REQUEST_PARTY_MEMBER_STATS(CMSG_REQUEST_PARTY_MEMBER_STATS { guid })
            }
            Family::Social => M::CMSG_FRIEND_LIST,
            Family::Trade => M::CMSG_BEGIN_TRADE,
            Family::Duel => M::CMSG_DUEL_CANCELLED(CMSG_DUEL_CANCELLED { guid }),
            Family::Chat => M::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
                chat_type: CMSG_MESSAGECHAT_ChatType::Say,
                language: Language::Universal,
                message: "hello".into(),
            })),
            Family::Channel => M::CMSG_CHANNEL_LIST(Box::new(CMSG_CHANNEL_LIST {
                channel_name: "General".into(),
            })),
            Family::MeetingStone => M::CMSG_MEETINGSTONE_INFO,
            Family::Guild => M::CMSG_GUILD_ROSTER,
            Family::Mail => M::MSG_QUERY_NEXT_MAIL_TIME,
            Family::Movement => M::MSG_MOVE_HEARTBEAT(Box::new(MSG_MOVE_HEARTBEAT_Client {
                info: MovementInfo {
                    flags: MovementInfo_MovementFlags::empty(),
                    timestamp: 0,
                    position: Vector3d {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    orientation: 0.0,
                    fall_time: 0.0,
                },
            })),
            Family::Unavailable => M::CMSG_TUTORIAL_CLEAR,
        }
    }

    #[test]
    fn each_family_owns_its_representative_opcode() {
        for family in ALL {
            let msg = representative(family);
            assert_eq!(owner(&msg), Some(family), "{msg}");
        }
    }

    #[test]
    fn an_opcode_no_family_handles_has_no_owner() {
        assert_eq!(owner(&M::CMSG_BOOTME), None);
    }
}
