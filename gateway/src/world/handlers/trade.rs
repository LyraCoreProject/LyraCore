//! The Trade Session handshake, initiate / begin / cancel. The module owns
//! every gate; ALL statuses, to both parties, ride the `game_trade_event` relay
//! (`stdb::subscriptions::trade_event_outbound`), so no arm here answers synchronously. Reducer
//! rejections are transient per-action failures (logged, never session-fatal), the vendor-arm
//! convention. Outside the world there is no `self_guid` to act as — dropped silently, like the
//! social family.

use super::super::social::self_guid;
use super::super::*;

/// The Trade Session handshake. Every status, BeginTrade/OpenWindow to the parties, or a refusal
/// back to the caller, rides the `game_trade_event` relay; these calls answer nothing
/// synchronously, and an `Err` is only an unresolved actor (per-action, log + ignore).
pub(crate) trait TradeStore: Send + Sync {
    /// `CMSG_INITIATE_TRADE` — propose a Trade Session against the targeted player.
    fn initiate_trade(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// `CMSG_BEGIN_TRADE` — the proposed target's client answered; the module opens both windows.
    fn begin_trade(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// `CMSG_CANCEL_TRADE` — tear the caller's Trade Session down (`TradeCanceled` to both).
    fn cancel_trade(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// `CMSG_SET_TRADE_ITEM` — `inv_slot` is the ABSOLUTE inventory slot (the gateway maps the
    /// client's (bag, slot) pair, the item-family convention).
    fn set_trade_item(
        &self,
        account_id: u64,
        self_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()>;

    /// `CMSG_CLEAR_TRADE_ITEM`.
    fn clear_trade_item(&self, account_id: u64, self_guid: u64, trade_slot: u8) -> Result<()>;

    /// `CMSG_SET_TRADE_GOLD`, `copper` is the offered amount.
    fn set_trade_gold(&self, account_id: u64, self_guid: u64, copper: u32) -> Result<()>;

    /// `CMSG_ACCEPT_TRADE` — accept the current offer; dual-accept runs the atomic Trade Commit
    /// module-side.
    fn accept_trade(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// `CMSG_UNACCEPT_TRADE`, withdraw an accept; partner hears `BackToTrade`.
    fn unaccept_trade(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// `CMSG_BUSY_TRADE`, decline a pending proposal as busy; initiator hears `Busy`.
    fn busy_trade(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// `CMSG_IGNORE_TRADE`, decline via ignore; initiator hears `IgnoreYou`.
    fn ignore_trade(&self, account_id: u64, self_guid: u64) -> Result<()>;
}

pub(crate) fn handle_trade<St: WorldStore + ?Sized>(
    _tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        ClientOpcodeMessage::CMSG_INITIATE_TRADE(c) => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.initiate_trade(conn.account_id, me, c.guid.guid()) {
                    log::debug!(
                        "world: initiate_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        ClientOpcodeMessage::CMSG_BEGIN_TRADE => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.begin_trade(conn.account_id, me) {
                    log::debug!(
                        "world: begin_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        ClientOpcodeMessage::CMSG_CANCEL_TRADE => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.cancel_trade(conn.account_id, me) {
                    log::debug!(
                        "world: cancel_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        // Offer mutations. The (bag, slot) pair maps onto the module's absolute slots the
        // same way the item family does: only the main pseudo-bag (255) is modelled — items inside
        // equipped sub-bags are logged + ignored, matching the item-action dispatcher's posture.
        ClientOpcodeMessage::CMSG_SET_TRADE_ITEM(c) => {
            const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0
            if let Some(me) = self_guid(conn) {
                if c.bag != MAIN_BAG {
                    log::debug!(
                        "world: set_trade_item from sub-bag {} ignored (account {})",
                        c.bag,
                        conn.account_id
                    );
                } else if let Err(e) =
                    store.set_trade_item(conn.account_id, me, c.trade_slot, c.slot)
                {
                    log::debug!(
                        "world: set_trade_item ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        ClientOpcodeMessage::CMSG_CLEAR_TRADE_ITEM(c) => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.clear_trade_item(conn.account_id, me, c.trade_slot) {
                    log::debug!(
                        "world: clear_trade_item ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        ClientOpcodeMessage::CMSG_SET_TRADE_GOLD(c) => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.set_trade_gold(conn.account_id, me, c.gold.as_int()) {
                    log::debug!(
                        "world: set_trade_gold ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        // Accept / unaccept. The accept body's `unknown1` is padding (vmangos skips it;
        // bots set 1) — dropped here, the module needs only who accepted.
        ClientOpcodeMessage::CMSG_ACCEPT_TRADE(_) => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.accept_trade(conn.account_id, me) {
                    log::debug!(
                        "world: accept_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        ClientOpcodeMessage::CMSG_UNACCEPT_TRADE => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.unaccept_trade(conn.account_id, me) {
                    log::debug!(
                        "world: unaccept_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        // Proposal declines: the client auto-answers a BeginTrade it can't take, busy
        // (already in a dialog) or the initiator is on the ignore list.
        ClientOpcodeMessage::CMSG_BUSY_TRADE => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.busy_trade(conn.account_id, me) {
                    log::debug!(
                        "world: busy_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        ClientOpcodeMessage::CMSG_IGNORE_TRADE => {
            if let Some(me) = self_guid(conn) {
                if let Err(e) = store.ignore_trade(conn.account_id, me) {
                    log::debug!(
                        "world: ignore_trade ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
            Ok(None)
        }
        other => Ok(Some(other)),
    }
}
