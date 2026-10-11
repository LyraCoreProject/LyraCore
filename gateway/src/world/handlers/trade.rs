//! The Trade Session handshake, initiate / begin / cancel. The module owns
//! every gate; ALL statuses, to both parties, ride the `game_trade_event` relay
//! (`stdb::subscriptions::trade_event_outbound`), so no arm here answers synchronously. Reducer
//! Refusals are transient per-action failures (logged, never session-fatal), the vendor-arm
//! convention. A Transport Loss ends the World Session. Outside the world there is no Actor to act
//! as, so the opcodes are dropped silently, like the social family.

use super::super::*;
use super::combat::ignore_refusal;

/// The Trade Session handshake. Every status, BeginTrade/OpenWindow to the parties, or a refusal
/// back to the caller, rides the `game_trade_event` relay; these calls answer nothing
/// synchronously, and a Refusal is a per-action failure (log + ignore).
pub(crate) trait TradeStore: Send + Sync {
    /// `CMSG_INITIATE_TRADE` — propose a Trade Session against the targeted player.
    fn initiate_trade(&self, actor: Actor, target_guid: u64) -> Result<()>;

    /// `CMSG_BEGIN_TRADE` — the proposed target's client answered; the module opens both windows.
    fn begin_trade(&self, actor: Actor) -> Result<()>;

    /// `CMSG_CANCEL_TRADE` — tear the caller's Trade Session down (`TradeCanceled` to both).
    fn cancel_trade(&self, actor: Actor) -> Result<()>;

    /// `CMSG_SET_TRADE_ITEM` — `inv_slot` is the ABSOLUTE inventory slot (the gateway maps the
    /// client's (bag, slot) pair, the item-family convention).
    fn set_trade_item(&self, actor: Actor, trade_slot: u8, inv_slot: u8) -> Result<()>;

    /// `CMSG_CLEAR_TRADE_ITEM`.
    fn clear_trade_item(&self, actor: Actor, trade_slot: u8) -> Result<()>;

    /// `CMSG_SET_TRADE_GOLD`, `copper` is the offered amount.
    fn set_trade_gold(&self, actor: Actor, copper: u32) -> Result<()>;

    /// `CMSG_ACCEPT_TRADE` — accept the current offer; dual-accept runs the atomic Trade Commit
    /// module-side.
    fn accept_trade(&self, actor: Actor) -> Result<()>;

    /// `CMSG_UNACCEPT_TRADE`, withdraw an accept; partner hears `BackToTrade`.
    fn unaccept_trade(&self, actor: Actor) -> Result<()>;

    /// `CMSG_BUSY_TRADE`, decline a pending proposal as busy; initiator hears `Busy`.
    fn busy_trade(&self, actor: Actor) -> Result<()>;

    /// `CMSG_IGNORE_TRADE`, decline via ignore; initiator hears `IgnoreYou`.
    fn ignore_trade(&self, actor: Actor) -> Result<()>;
}

pub(crate) struct Trade;

impl<St: TradeStore + ?Sized> ProtocolFamily<St> for Trade {
    fn handle(
        store: &St,
        conn: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let msg = request.message()?;
        let account_id = conn.account_id;
        let actor = conn.self_guid().and_then(Actor::new);
        let result = match (msg, actor) {
            (ClientOpcodeMessage::CMSG_INITIATE_TRADE(c), Some(actor)) => {
                ("initiate_trade", store.initiate_trade(actor, c.guid.guid()))
            }
            (ClientOpcodeMessage::CMSG_BEGIN_TRADE, Some(actor)) => {
                ("begin_trade", store.begin_trade(actor))
            }
            (ClientOpcodeMessage::CMSG_CANCEL_TRADE, Some(actor)) => {
                ("cancel_trade", store.cancel_trade(actor))
            }
            // Offer mutations. The (bag, slot) pair maps onto the module's absolute slots the
            // same way the item family does: only the main pseudo-bag (255) is modelled — items inside
            // equipped sub-bags are logged + ignored, matching the item-action dispatcher's posture.
            (ClientOpcodeMessage::CMSG_SET_TRADE_ITEM(c), Some(actor)) => {
                const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0
                if c.bag != MAIN_BAG {
                    log::debug!(
                        "world: set_trade_item from sub-bag {} ignored (account {account_id})",
                        c.bag
                    );
                    return Ok(ProtocolReply::default());
                }
                (
                    "set_trade_item",
                    store.set_trade_item(actor, c.trade_slot, c.slot),
                )
            }
            (ClientOpcodeMessage::CMSG_CLEAR_TRADE_ITEM(c), Some(actor)) => (
                "clear_trade_item",
                store.clear_trade_item(actor, c.trade_slot),
            ),
            (ClientOpcodeMessage::CMSG_SET_TRADE_GOLD(c), Some(actor)) => (
                "set_trade_gold",
                store.set_trade_gold(actor, c.gold.as_int()),
            ),
            // Accept / unaccept. The accept body's `unknown1` is padding (vmangos skips it;
            // bots set 1) — dropped here, the module needs only who accepted.
            (ClientOpcodeMessage::CMSG_ACCEPT_TRADE(_), Some(actor)) => {
                ("accept_trade", store.accept_trade(actor))
            }
            (ClientOpcodeMessage::CMSG_UNACCEPT_TRADE, Some(actor)) => {
                ("unaccept_trade", store.unaccept_trade(actor))
            }
            // Proposal declines: the client auto-answers a BeginTrade it can't take, busy
            // (already in a dialog) or the initiator is on the ignore list.
            (ClientOpcodeMessage::CMSG_BUSY_TRADE, Some(actor)) => {
                ("busy_trade", store.busy_trade(actor))
            }
            (ClientOpcodeMessage::CMSG_IGNORE_TRADE, Some(actor)) => {
                ("ignore_trade", store.ignore_trade(actor))
            }
            // Outside the world there is no Actor: the trade opcodes are consumed in silence.
            (
                ClientOpcodeMessage::CMSG_INITIATE_TRADE(_)
                | ClientOpcodeMessage::CMSG_BEGIN_TRADE
                | ClientOpcodeMessage::CMSG_CANCEL_TRADE
                | ClientOpcodeMessage::CMSG_SET_TRADE_ITEM(_)
                | ClientOpcodeMessage::CMSG_CLEAR_TRADE_ITEM(_)
                | ClientOpcodeMessage::CMSG_SET_TRADE_GOLD(_)
                | ClientOpcodeMessage::CMSG_ACCEPT_TRADE(_)
                | ClientOpcodeMessage::CMSG_UNACCEPT_TRADE
                | ClientOpcodeMessage::CMSG_BUSY_TRADE
                | ClientOpcodeMessage::CMSG_IGNORE_TRADE,
                None,
            ) => return Ok(ProtocolReply::default()),
            (_, _) => return Err(anyhow!("opcode routed to the wrong Protocol Family")),
        };
        let (action, result) = result;
        ignore_refusal(action, account_id, result)?;
        Ok(ProtocolReply::default())
    }
}
