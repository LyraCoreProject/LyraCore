//! Per-domain dispatch handlers and the deeper action dispatchers that own a complete protocol
//! family, plus the couple of helpers shared by two handlers.

use super::*;

// Two shapes live here. A `handle_*` handler is code-motion of the former dispatch match arms
// (bodies verbatim): it sends on the socket itself and returns `Ok(None)` once it consumes its
// opcode, else `Ok(Some(msg))` to pass the message on. A `dispatch_*_action` seam — auction, item,
// melee, quest and vendor — owns a whole protocol family instead: it takes a narrow store trait and
// a player context, decides refusal-versus-fatal itself, and returns the outbound batch for the
// world session to send, so the family can be tested without a socket.

mod bank;
mod auction;
mod cast;
mod char;
mod combat;
mod duel;
mod item;
mod loot;
mod mail;
mod melee;
mod query;
mod quest;
mod taxi;
mod trade;
mod trainer;
mod vendor;

pub(crate) use bank::handle_bank;
pub(crate) use auction::{
    decode_auction_browse, dispatch_auction_action, dispatch_auction_browse_action,
    AuctionActionOutcome, AuctionActionPlayer, AuctionActionStore, AuctionBrowseRequest,
    AuctionPage, AuctionQuery, CreateAuctionOutcome, CreateAuctionRequest, PlaceBidOutcome,
    PlaceBidRequest,
    CMSG_AUCTION_LIST_ITEMS_OPCODE,
};
#[cfg(test)]
pub(crate) use auction::{AuctionEntity, AuctionHousePolicy, AuctionInteraction};
pub(crate) use cast::{dispatch_cast, CastOutcome, CastPlayer, CastStore, CastTransition};
pub(crate) use char::handle_char;
pub(crate) use combat::handle_combat;
pub(crate) use duel::{dispatch_duel_action, DuelActionOutcome, DuelActionPlayer, DuelActionStore};
pub(crate) use item::{dispatch_item_action, ItemActionOutcome, ItemActionPlayer, ItemActionStore};
#[cfg(test)]
pub(crate) use loot::LootWindowRequestStatus;
pub(crate) use loot::{
    dispatch_loot_window, handle_loot, LootWindowOutcome, LootWindowPlayer, LootWindowStore,
    OpenLootState,
};
pub(crate) use mail::handle_mail;
pub(crate) use melee::{
    dispatch_melee_action, MeleeActionOutcome, MeleeActionPlayer, MeleeActionStore,
};
pub(crate) use query::handle_query;
pub(crate) use quest::{
    dispatch_quest_action, quest_giver_menu, QuestActionOutcome, QuestActionPlayer, QuestActionStore,
};
pub(crate) use taxi::{
    dispatch_taxi_action, queue_reply_then_arm, TaxiActionOutcome, TaxiActionPlayer, TaxiActionStore,
};
pub(crate) use trade::handle_trade;
pub(crate) use trainer::handle_trainer;
pub(crate) use vendor::{
    dispatch_vendor_action, VendorActionOutcome, VendorActionPlayer, VendorActionStore,
};

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
