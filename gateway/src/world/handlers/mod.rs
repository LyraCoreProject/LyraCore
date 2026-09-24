//! Per-domain dispatch handlers and the deeper action dispatchers that own a complete protocol
//! family, plus the couple of helpers shared by two handlers.

use super::*;

// Two shapes live here. A `handle_*` handler is code-motion of the former dispatch match arms
// (bodies verbatim): it sends on the socket itself and returns `Ok(None)` once it consumes its
// opcode, else `Ok(Some(msg))` to pass the message on. A `dispatch_*_action` seam (auction, channel,
// chat, item, melee, quest and vendor) owns a whole protocol family instead: it takes a narrow store
// trait and a player context, decides refusal-versus-fatal itself, and returns the outbound batch
// for the world session to send, so the family can be tested without a socket.

mod auction;
mod bank;
mod cast;
mod channel;
mod char;
mod chat;
mod combat;
mod duel;
mod guild;
mod item;
mod loot;
mod mail;
mod melee;
mod member_stats;
mod query;
mod quest;
mod taxi;
mod trade;
mod trainer;
mod vendor;
mod weather;

pub(crate) use auction::{
    decode_auction_browse, dispatch_auction_action, dispatch_auction_browse_action,
    AuctionActionOutcome, AuctionActionPlayer, AuctionActionStore, AuctionBrowseRequest,
    AuctionPage, AuctionQuery, CreateAuctionOutcome, CreateAuctionRequest, PlaceBidOutcome,
    PlaceBidRequest, CMSG_AUCTION_LIST_ITEMS_OPCODE,
};
#[cfg(test)]
pub(crate) use auction::{AuctionHousePolicy, AuctionInteraction};
pub(crate) use bank::handle_bank;
pub(crate) use cast::{dispatch_cast, CastOutcome, CastPlayer, CastStore, CastTransition};
pub(crate) use channel::{
    dispatch_channel_action, ChannelActionOutcome, ChannelActionStore, ChannelOutcome,
    ChannelRequest, ChannelRoster,
};
pub(crate) use char::handle_char;
pub(crate) use chat::{
    dispatch_chat_action, ChatActionOutcome, ChatActionPlayer, ChatActionStore, ChatOutcome,
    RealmChatRequest, SpeakerFacts,
};
pub(crate) use combat::handle_combat;
pub(crate) use duel::{dispatch_duel_action, DuelActionOutcome, DuelActionPlayer, DuelActionStore};
pub(crate) use guild::{
    dispatch_guild_action, guild_projection, guild_sign_on, guild_world_entry, guild_world_exit,
    is_guild_dot_command, run_guild_dot_command, CharacterFacts, GuildActionOutcome,
    GuildActionPlayer, GuildActionStore, GuildEventSnapshot, GuildOutcome, GuildRequest,
};
pub(crate) use item::{
    dispatch_item_action, ItemActionOutcome, ItemActionPlayer, ItemActionResult, ItemActionStore,
};
pub(crate) use loot::{
    dispatch_loot_window, handle_loot, LootActionStatus, LootWindowOutcome, LootWindowPlayer,
    LootWindowRefusal, LootWindowRequestStatus, LootWindowStore, OpenLootState,
};
pub(crate) use mail::handle_mail;
pub(crate) use melee::{
    dispatch_melee_action, MeleeActionOutcome, MeleeActionPlayer, MeleeActionStore,
};
#[cfg(test)]
pub(crate) use member_stats::MemberSnapshot;
pub(crate) use member_stats::{
    dispatch_member_stats, member_stats_tick, MemberPresence, MemberStatsOutcome,
    MemberStatsPlayer, MemberStatsRecord, MemberStatsStore,
};
pub(crate) use query::handle_query;
pub(crate) use quest::{
    dispatch_quest_action, quest_giver_menu, QuestActionOutcome, QuestActionPlayer,
    QuestActionStore,
};
pub(crate) use taxi::{
    dispatch_taxi_action, queue_reply_then_arm, TaxiActionOutcome, TaxiActionPlayer,
    TaxiActionStore,
};
pub(crate) use trade::handle_trade;
pub(crate) use trainer::{handle_trainer, TrainerBuyOutcome};
pub(crate) use vendor::{
    dispatch_vendor_action, VendorActionOutcome, VendorActionPlayer, VendorActionStore,
};
pub(crate) use weather::{zone_weather_message, WeatherStore};

/// Open the bank window for `banker_guid`. Single chokepoint for `CMSG_BANKER_ACTIVATE` and the
/// BANKER gossip option, so the two entry points cannot drift apart.
fn send_show_bank(tx: &SessionTx, banker_guid: u64) -> Result<()> {
    send(
        tx,
        Outbound::One(ServerOpcodeMessage::SMSG_SHOW_BANK(codec::build_show_bank(
            banker_guid,
        ))),
    )
}
