//! Per-domain dispatch handlers and the deeper action dispatchers that own a complete protocol
//! family, plus the couple of helpers shared by two handlers.

use super::*;

// Per-family dispatch handlers — code-motion of the former dispatch match arms (bodies verbatim,
// incl. in `handle_combat` the two session-fatal `is_desync_error` early-exits on ATTACKSWING/STOP).
// Each returns `Ok(None)` once it consumes its opcode, else `Ok(Some(msg))` to pass the message on.

mod bank;
mod char;
mod combat;
mod item;
mod loot;
mod mail;
mod query;
mod quest;
mod trade;
mod trainer;
mod vendor;

pub(crate) use bank::handle_bank;
pub(crate) use char::handle_char;
pub(crate) use combat::handle_combat;
pub(crate) use item::{dispatch_item_action, ItemActionOutcome, ItemActionPlayer, ItemActionStore};
pub(crate) use loot::handle_loot;
pub(crate) use mail::handle_mail;
pub(crate) use query::handle_query;
pub(crate) use quest::{
    dispatch_quest_action, handle_quest, QuestActionOutcome, QuestActionPlayer, QuestActionStore,
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
