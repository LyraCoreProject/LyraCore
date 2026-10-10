//! World Session socket tests. Each `<family>_tests.rs` file drives a real encrypted World Session
//! against [`WorldFake`] for one Store family, and `session_tests.rs` covers the session itself.
//! This module holds the imports those files share and the helpers more than one of them uses.

use super::handlers::{
    AuctionInteraction, ChannelOutcome, ChatOutcome, LootWindowRefusal, RealmChatRequest,
    SpeakerFacts, WhisperRequest, WhisperTargetFacts,
};
use super::party::PartyOutcome;
use super::test_support::*;
use super::*;
use crate::read_deadline::{DeadlineClock, PreAuthDeadline};
use lyracore_shared::group::{GroupKind, RaidSlot};
use lyracore_shared::item::ItemRefusal;
use lyracore_shared::loot::LootRefusal;
use lyracore_shared::social::ContactRefusal;
use std::cell::Cell;
use std::io::Cursor;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    BuyBankSlotResult,
    BuyResult,
    BuybackSlot,
    Class,
    ClientMessage,
    Gender,
    GroupLootSetting,
    ItemQuality,
    Language,
    Level,
    MSG_AUCTION_HELLO_Client,
    Map,
    Object,
    Race,
    RollVote,
    SheathState,
    SpellCastTargets,
    SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_Unit,
    Talent,
    TrainingFailureReason,
    WeatherType,
    WorldResult,
    CMSG_ACTIVATETAXI,
    CMSG_ADD_FRIEND,
    CMSG_ADD_IGNORE,
    CMSG_ATTACKSTOP,
    CMSG_ATTACKSWING,
    CMSG_AUCTION_LIST_BIDDER_ITEMS,
    CMSG_AUCTION_LIST_ITEMS,
    CMSG_AUCTION_LIST_OWNER_ITEMS,
    CMSG_AUTOBANK_ITEM,
    CMSG_AUTOEQUIP_ITEM,
    CMSG_AUTOSTORE_BANK_ITEM,
    CMSG_AUTOSTORE_LOOT_ITEM,
    CMSG_BANKER_ACTIVATE,
    CMSG_BUYBACK_ITEM,
    CMSG_BUY_BANK_SLOT,
    CMSG_BUY_ITEM,
    CMSG_CANCEL_AURA,
    CMSG_CANCEL_AUTO_REPEAT_SPELL,
    CMSG_CANCEL_CAST,
    CMSG_CAST_SPELL,
    CMSG_CHAR_CREATE,
    CMSG_CHAR_DELETE,
    CMSG_CHAR_ENUM,
    CMSG_DEL_FRIEND,
    CMSG_DEL_IGNORE,
    CMSG_FRIEND_LIST,
    CMSG_GAMEOBJ_USE,
    CMSG_GOSSIP_HELLO,
    CMSG_GOSSIP_SELECT_OPTION,
    CMSG_GUILD_ACCEPT,
    CMSG_GUILD_DECLINE,
    CMSG_GUILD_DEMOTE,
    CMSG_GUILD_DISBAND,
    CMSG_GUILD_INVITE,
    CMSG_GUILD_LEADER,
    CMSG_GUILD_LEAVE,
    CMSG_GUILD_PROMOTE,
    CMSG_GUILD_REMOVE,
    CMSG_INSPECT,
    CMSG_ITEM_QUERY_SINGLE,
    CMSG_LEARN_TALENT,
    CMSG_LIST_INVENTORY,
    CMSG_LOGOUT_REQUEST,
    CMSG_LOOT,
    CMSG_LOOT_MASTER_GIVE,
    CMSG_LOOT_METHOD,
    CMSG_LOOT_MONEY,
    CMSG_LOOT_RELEASE,
    CMSG_LOOT_ROLL,
    CMSG_NPC_TEXT_QUERY,
    CMSG_PLAYED_TIME,
    CMSG_PLAYER_LOGIN,
    CMSG_QUESTGIVER_CHOOSE_REWARD,
    CMSG_QUESTGIVER_HELLO,
    CMSG_QUESTGIVER_STATUS_QUERY,
    CMSG_QUEST_QUERY,
    // Item guid → slot resolution (vendor sell / armorer repair).
    CMSG_RECLAIM_CORPSE,
    CMSG_REPOP_REQUEST,
    CMSG_RESURRECT_RESPONSE,
    CMSG_SELF_RES,
    CMSG_SETSHEATHED,
    CMSG_SET_SELECTION,
    CMSG_SPIRIT_HEALER_ACTIVATE,
    CMSG_TAXINODE_STATUS_QUERY,
    CMSG_TAXIQUERYAVAILABLENODES,
    CMSG_TRAINER_BUY_SPELL,
    CMSG_TRAINER_LIST,
    CMSG_WHO,
    // Cross-map teleport: the client's world-port-finished ack.
    MSG_MOVE_WORLDPORT_ACK,
    SMSG_WEATHER,
};
use wow_world_messages::Guid;

#[path = "alpha_test_tools_tests.rs"]
mod alpha_test_tools_tests;
#[path = "auction_tests.rs"]
mod auction_tests;
#[path = "bank_tests.rs"]
mod bank_tests;
#[path = "cast_tests.rs"]
mod cast_tests;
#[path = "channel_tests.rs"]
mod channel_tests;
#[path = "character_tests.rs"]
mod character_tests;
#[path = "chat_tests.rs"]
mod chat_tests;
#[path = "combat_tests.rs"]
mod combat_tests;
#[path = "death_tests.rs"]
mod death_tests;
#[path = "framing_tests.rs"]
mod framing_tests;
#[path = "guild_tests.rs"]
mod guild_tests;
#[path = "item_tests.rs"]
mod item_tests;
#[path = "loot_tests.rs"]
mod loot_tests;
#[path = "loot_window_tests.rs"]
mod loot_window_tests;
#[path = "mail_tests.rs"]
mod mail_tests;
#[path = "melee_tests.rs"]
mod melee_tests;
#[path = "member_stats_tests.rs"]
mod member_stats_tests;
#[path = "npc_tests.rs"]
mod npc_tests;
#[path = "party_mirror_tests.rs"]
mod party_mirror_tests;
#[path = "party_tests.rs"]
mod party_tests;
#[path = "presence_tests.rs"]
mod presence_tests;
#[path = "quest_tests.rs"]
mod quest_tests;
#[path = "session_tests.rs"]
mod session_tests;
#[path = "shard_routing_tests.rs"]
mod shard_routing_tests;
#[path = "social_tests.rs"]
mod social_tests;
#[path = "taxi_tests.rs"]
mod taxi_tests;
#[path = "trade_tests.rs"]
mod trade_tests;
#[path = "trainer_tests.rs"]
mod trainer_tests;
#[path = "transfer_tests.rs"]
mod transfer_tests;
#[path = "vendor_tests.rs"]
mod vendor_tests;
#[path = "weather_tests.rs"]
mod weather_tests;
#[path = "whisper_tests.rs"]
mod whisper_tests;
#[path = "who_tests.rs"]
mod who_tests;
#[path = "wire_corruption_tests.rs"]
mod wire_corruption_tests;

use shard_routing_tests::ShardCallLog;

/// Write one request that answers its actor nothing, then a sentinel request with a guaranteed
/// reply, and block for that reply. This is more than pacing: `enter_world` drains the world entry
/// batch, which ends before the Guild MOTD event `guild_world_entry` sends a fresh-login
/// Guild member (see its own doc comment), so one packet can still be unread in the client's
/// kernel buffer. Reading for a sentinel discards it along the way. Dropping the client with it
/// still queued would close with unread bytes, which the kernel reports to the server as a reset,
/// not a clean EOF (`enter_world`'s doc comment names this exact failure shape). The sentinel is
/// CMSG_PLAYED_TIME, which replies only when the store has a Character row for the caller.
fn sync(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    write: impl FnOnce(&mut UnixStream, &mut EncrypterHalf),
) {
    write(&mut *client, &mut *enc);
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut *client, &mut *enc)
        .unwrap();
    loop {
        if let ServerOpcodeMessage::SMSG_PLAYED_TIME(_) =
            ServerOpcodeMessage::read_encrypted(&mut *client, &mut *dec).unwrap()
        {
            break;
        }
    }
}

fn position(calls: &[String], what: &str) -> usize {
    calls
        .iter()
        .position(|call| call == what)
        .unwrap_or_else(|| panic!("`{what}` never ran: {calls:?}"))
}

/// A minimal quest detail (everything zero/empty but the id + title) — enough for the build_* codecs.
fn detail_view(quest_id: u32, title: &str) -> codec::QuestDetailView {
    codec::QuestDetailView {
        quest_id,
        quest_level: 1,
        zone_or_sort: 12,
        title: title.into(),
        details: String::new(),
        objectives_text: String::new(),
        offer_reward_text: String::new(),
        request_items_text: String::new(),
        money_reward: 0,
        reward_xp: 0,
        next_quest_id: 0,
        max_level_money_reward: 0,
        rewards: Vec::new(),
        choice_rewards: Vec::new(),
        objectives: Vec::new(),
    }
}

/// One giver↔quest eval with the four booleans the menu/status + complete-branch read.
fn eval(quest_id: u32, role: u8, active: bool, complete: bool) -> codec::GiverQuestEval {
    codec::GiverQuestEval {
        quest_id,
        title: "Q".into(),
        level: 1,
        role,
        startable: role == codec::ROLE_START,
        active,
        complete,
    }
}

fn imported_auction_interaction() -> AuctionInteraction {
    AuctionInteraction {
        house: super::handlers::AuctionHousePolicy {
            id: 1,
            deposit_rate: 5,
            consignment_rate: 5,
        },
        refuses_interaction: false,
    }
}

/// `SMSG_LOOT_RESPONSE`, read raw because the loot window is an `Outbound::Raw` packet.
const OP_LOOT_RESPONSE: u16 = 0x0160;

/// Read one encrypted server frame RAW: decrypt the 4-byte header via the client's `DecrypterHalf`,
/// return `(opcode, body)`. Needed where the wire contract is an `Outbound::Raw` packet (the 5-byte
/// CAST_RESULT ack, the loot/vendor windows) or where the exact opcode ORDER across raw+typed sends
/// is the assertion — gtker's typed reader rejects some of the hand-rolled bodies (it would consume
/// the frame but error), so the bytes are read and pinned directly.
fn read_raw_frame<S: Read>(client: &mut S, dec: &mut DecrypterHalf) -> (u16, Vec<u8>) {
    let h = dec.read_and_decrypt_server_header(&mut *client).unwrap();
    let mut body = vec![0u8; (h.size as usize).saturating_sub(2)];
    client.read_exact(&mut body).unwrap();
    (h.opcode, body)
}

fn open_loot_window(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    target_guid: u64,
) -> Vec<u8> {
    CMSG_LOOT {
        guid: Guid::new(target_guid),
    }
    .write_encrypted_client(&mut *client, enc)
    .unwrap();
    let (opcode, body) = read_raw_frame(client, dec);
    assert_eq!(opcode, OP_LOOT_RESPONSE);
    body
}

/// Open `npc`'s gossip window and drain the menu, so a following `CMSG_GOSSIP_SELECT_OPTION` has the
/// snapshot it resolves against — a click with nothing open selects nothing.
fn gossip_hello(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    npc: u64,
) -> wow_world_messages::vanilla::SMSG_GOSSIP_MESSAGE {
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(npc),
    }
    .write_encrypted_client(&mut *client, enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut *client, dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => *m,
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
}

/// A shorthand imported option builder for the gossip tests.
fn opt(icon: u32, text: &str, action: u32) -> codec::GossipOptionView {
    codec::GossipOptionView {
        icon,
        text: text.to_string(),
        action,
        ..Default::default()
    }
}

fn human_speaker() -> SpeakerFacts {
    SpeakerFacts {
        race: 1,
        chat_tag: 0,
        name: String::new(),
    }
}
