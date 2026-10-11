//! One opcode has one owning Protocol Family, independent of declaration order.
use super::handlers::*;
use super::social::Social;
use super::*;

macro_rules! protocol_families {
    ($($family:ident => [$($opcode:ident),* $(,)?]),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum Owner { $($family),* }
        const OWNERS: &[(u32, Owner)] = &[
            $($( (<wow_world_messages::vanilla::$opcode as wow_world_messages::Message>::OPCODE, Owner::$family), )*)*
        ];
        impl Owner {
            pub(super) fn handle(self, store: &dyn WorldStore, session: &mut ProtocolSession, request: ProtocolRequest) -> Result<ProtocolReply> {
                match self { $(Self::$family => $family::handle(store, session, request)),* }
            }
        }
    };
}

protocol_families! {
    Character => [
        CMSG_PING,
        CMSG_QUERY_TIME,
        CMSG_CHAR_ENUM,
        CMSG_CHAR_CREATE,
        CMSG_CHAR_DELETE,
        CMSG_PLAYER_LOGIN,
        MSG_MOVE_WORLDPORT_ACK,
        CMSG_LOGOUT_CANCEL,
        CMSG_LOGOUT_REQUEST,
        CMSG_PLAYER_LOGOUT,
        CMSG_PLAYED_TIME,
    ],
    Melee => [
        CMSG_ATTACKSWING,
        CMSG_ATTACKSTOP,
    ],
    Cast => [
        CMSG_CAST_SPELL,
        CMSG_CANCEL_AUTO_REPEAT_SPELL,
        CMSG_CANCEL_CAST,
        CMSG_CANCEL_CHANNELLING,
        CMSG_CANCEL_AURA,
    ],
    Combat => [
        CMSG_SET_SELECTION,
        CMSG_PET_ACTION,
        CMSG_FORCE_RUN_SPEED_CHANGE_ACK,
        CMSG_SETSHEATHED,
    ],
    Auction => [
        MSG_AUCTION_HELLO_Client,
        CMSG_AUCTION_LIST_ITEMS,
        CMSG_AUCTION_LIST_OWNER_ITEMS,
        CMSG_AUCTION_LIST_BIDDER_ITEMS,
        CMSG_AUCTION_PLACE_BID,
        CMSG_AUCTION_SELL_ITEM,
        CMSG_AUCTION_REMOVE_ITEM,
    ],
    Vendor => [
        CMSG_LIST_INVENTORY,
        CMSG_BUY_ITEM,
        CMSG_REPAIR_ITEM,
        CMSG_SELL_ITEM,
        CMSG_BUYBACK_ITEM,
    ],
    Bank => [
        CMSG_BANKER_ACTIVATE,
        CMSG_AUTOBANK_ITEM,
        CMSG_BUY_BANK_SLOT,
        CMSG_AUTOSTORE_BANK_ITEM,
    ],
    Trainer => [
        CMSG_TRAINER_LIST,
        CMSG_TRAINER_BUY_SPELL,
        CMSG_SET_ACTION_BUTTON,
        CMSG_LEARN_TALENT,
        CMSG_SET_FACTION_ATWAR,
        CMSG_SET_WATCHED_FACTION,
    ],
    Item => [
        CMSG_DESTROYITEM,
        CMSG_SPLIT_ITEM,
        CMSG_AUTOEQUIP_ITEM,
        CMSG_AUTOSTORE_BAG_ITEM,
        CMSG_SWAP_INV_ITEM,
        CMSG_SWAP_ITEM,
        CMSG_USE_ITEM,
    ],
    Quest => [
        CMSG_QUESTGIVER_STATUS_QUERY,
        CMSG_QUESTGIVER_HELLO,
        CMSG_QUESTGIVER_QUERY_QUEST,
        CMSG_QUEST_QUERY,
        CMSG_QUESTGIVER_ACCEPT_QUEST,
        CMSG_QUESTGIVER_COMPLETE_QUEST,
        CMSG_QUESTGIVER_REQUEST_REWARD,
        CMSG_QUESTGIVER_CHOOSE_REWARD,
        CMSG_QUESTLOG_REMOVE_QUEST,
        CMSG_PUSHQUESTTOPARTY,
    ],
    Taxi => [
        CMSG_TAXINODE_STATUS_QUERY,
        CMSG_TAXIQUERYAVAILABLENODES,
        CMSG_ACTIVATETAXI,
    ],
    MemberStats => [
        CMSG_REQUEST_PARTY_MEMBER_STATS,
    ],
    Trade => [
        CMSG_INITIATE_TRADE,
        CMSG_BEGIN_TRADE,
        CMSG_CANCEL_TRADE,
        CMSG_SET_TRADE_ITEM,
        CMSG_CLEAR_TRADE_ITEM,
        CMSG_SET_TRADE_GOLD,
        CMSG_ACCEPT_TRADE,
        CMSG_UNACCEPT_TRADE,
        CMSG_BUSY_TRADE,
        CMSG_IGNORE_TRADE,
    ],
    Duel => [
        CMSG_DUEL_ACCEPTED,
        CMSG_DUEL_CANCELLED,
    ],
    Chat => [
        CMSG_CHAT_IGNORED,
    ],
    Channel => [
        CMSG_JOIN_CHANNEL,
        CMSG_LEAVE_CHANNEL,
        CMSG_CHANNEL_PASSWORD,
        CMSG_CHANNEL_SET_OWNER,
        CMSG_CHANNEL_MODERATOR,
        CMSG_CHANNEL_UNMODERATOR,
        CMSG_CHANNEL_MUTE,
        CMSG_CHANNEL_UNMUTE,
        CMSG_CHANNEL_KICK,
        CMSG_CHANNEL_BAN,
        CMSG_CHANNEL_UNBAN,
        CMSG_CHANNEL_INVITE,
        CMSG_CHANNEL_ANNOUNCEMENTS,
        CMSG_CHANNEL_MODERATE,
        CMSG_CHANNEL_LIST,
        CMSG_CHANNEL_OWNER,
    ],
    MeetingStone => [
        CMSG_MEETINGSTONE_JOIN,
        CMSG_MEETINGSTONE_LEAVE,
        CMSG_MEETINGSTONE_INFO,
        MSG_LOOKING_FOR_GROUP_Client,
    ],
    Guild => [
        CMSG_GUILD_QUERY,
        CMSG_GUILD_ROSTER,
        CMSG_GUILD_INFO,
        CMSG_GUILD_CREATE,
        MSG_TABARDVENDOR_ACTIVATE,
        MSG_SAVE_GUILD_EMBLEM_Client,
        CMSG_GUILD_INVITE,
        CMSG_GUILD_ACCEPT,
        CMSG_GUILD_DECLINE,
        CMSG_GUILD_LEAVE,
        CMSG_GUILD_REMOVE,
        CMSG_GUILD_PROMOTE,
        CMSG_GUILD_DEMOTE,
        CMSG_GUILD_LEADER,
        CMSG_GUILD_DISBAND,
        CMSG_GUILD_MOTD,
        CMSG_GUILD_INFO_TEXT,
        CMSG_GUILD_SET_PUBLIC_NOTE,
        CMSG_GUILD_SET_OFFICER_NOTE,
        CMSG_GUILD_RANK,
        CMSG_GUILD_ADD_RANK,
        CMSG_GUILD_DEL_RANK,
        CMSG_PETITION_SHOWLIST,
        CMSG_PETITION_BUY,
        CMSG_PETITION_SHOW_SIGNATURES,
        CMSG_PETITION_QUERY,
        MSG_PETITION_RENAME,
        CMSG_OFFER_PETITION,
        CMSG_PETITION_SIGN,
        MSG_PETITION_DECLINE,
        CMSG_TURN_IN_PETITION,
    ],
    Query => [
        CMSG_BINDER_ACTIVATE,
        MSG_TALENT_WIPE_CONFIRM_Client,
        CMSG_NAME_QUERY,
        CMSG_PET_NAME_QUERY,
        CMSG_INSPECT,
        CMSG_CREATURE_QUERY,
        CMSG_GOSSIP_HELLO,
        CMSG_NPC_TEXT_QUERY,
        CMSG_GOSSIP_SELECT_OPTION,
        CMSG_ITEM_QUERY_SINGLE,
        CMSG_MESSAGECHAT,
        CMSG_TEXT_EMOTE,
        MSG_RANDOM_ROLL_Client,
    ],
    Mail => [
        CMSG_GET_MAIL_LIST,
        MSG_QUERY_NEXT_MAIL_TIME_Client,
        CMSG_ITEM_TEXT_QUERY,
        CMSG_MAIL_MARK_AS_READ,
        CMSG_MAIL_DELETE,
        CMSG_MAIL_RETURN_TO_SENDER,
        CMSG_MAIL_TAKE_MONEY,
        CMSG_MAIL_TAKE_ITEM,
        CMSG_SEND_MAIL,
        CMSG_MAIL_CREATE_TEXT_ITEM,
    ],
    Social => [
        CMSG_WHO,
        CMSG_FRIEND_LIST,
        CMSG_ADD_FRIEND,
        CMSG_ADD_IGNORE,
        CMSG_DEL_FRIEND,
        CMSG_DEL_IGNORE,
        CMSG_GROUP_INVITE,
        CMSG_GROUP_ACCEPT,
        CMSG_GROUP_DECLINE,
        CMSG_GROUP_DISBAND,
        CMSG_GROUP_UNINVITE,
        CMSG_LOOT_METHOD,
        CMSG_GROUP_RAID_CONVERT,
        CMSG_GROUP_SET_LEADER,
        CMSG_GROUP_ASSISTANT_LEADER,
        CMSG_GROUP_UNINVITE_GUID,
        CMSG_GROUP_CHANGE_SUB_GROUP,
        CMSG_GROUP_SWAP_SUB_GROUP,
        MSG_RAID_READY_CHECK_Client,
        MSG_RAID_TARGET_UPDATE_Client,
        MSG_MINIMAP_PING_Client,
    ],
    GameObject => [
        CMSG_GAMEOBJ_USE,
    ],
    LootWindow => [
        CMSG_LOOT,
        CMSG_LOOT_MONEY,
        CMSG_AUTOSTORE_LOOT_ITEM,
        CMSG_LOOT_RELEASE,
    ],
    Loot => [
        CMSG_LOOT_ROLL,
        CMSG_LOOT_MASTER_GIVE,
        CMSG_AREATRIGGER,
        CMSG_GAMEOBJECT_QUERY,
        CMSG_REPOP_REQUEST,
        MSG_CORPSE_QUERY_Client,
        CMSG_RECLAIM_CORPSE,
        CMSG_RESURRECT_RESPONSE,
        CMSG_SELF_RES,
        CMSG_SPIRIT_HEALER_ACTIVATE,
    ],
}

pub(super) fn owner(opcode: u32) -> Option<Owner> {
    OWNERS
        .iter()
        .find_map(|&(owned, owner)| (owned == opcode).then_some(owner))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn duplicate(owners: &[(u32, Owner)]) -> Option<(u32, Owner, Owner)> {
        let mut seen = std::collections::HashMap::new();
        for &(opcode, owner) in owners {
            if let Some(previous) = seen.insert(opcode, owner) {
                return Some((opcode, previous, owner));
            }
        }
        None
    }
    #[test]
    fn each_opcode_has_one_owner() {
        assert_eq!(duplicate(OWNERS), None);
    }
    #[test]
    fn competing_ownership_is_detected() {
        assert_eq!(
            duplicate(&[(0x125, Owner::Trainer), (0x125, Owner::Query)]),
            Some((0x125, Owner::Trainer, Owner::Query))
        );
    }
    #[test]
    fn raw_requests_have_owners() {
        assert_eq!(owner(0x258), Some(Owner::Auction));
        assert_eq!(owner(0x125), Some(Owner::Trainer));
        assert_eq!(owner(0x318), Some(Owner::Trainer));
    }

    #[test]
    fn unsupported_chat_kinds_are_silent_through_the_opcode_owner() {
        use wow_world_messages::vanilla::Language;
        let store = super::super::test_support::WorldFake::default();
        let mut session = ProtocolSession::in_world(7, 42);
        for chat_type in [
            CMSG_MESSAGECHAT_ChatType::Battleground,
            CMSG_MESSAGECHAT_ChatType::BattlegroundLeader,
            CMSG_MESSAGECHAT_ChatType::System,
            CMSG_MESSAGECHAT_ChatType::TextEmote,
        ] {
            let request = ClientOpcodeMessage::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
                chat_type,
                language: Language::Universal,
                message: "ignored".into(),
            }));
            let reply = owner(<CMSG_MESSAGECHAT as wow_world_messages::Message>::OPCODE)
                .unwrap()
                .handle(&store, &mut session, request.into())
                .unwrap();
            assert!(reply.outbound.is_empty());
            assert!(reply.after_queue.is_none());
        }
    }
}
