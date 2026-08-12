//! Trade family (#120): the Trade Session handshake — initiate / begin / cancel. The module owns
//! every gate; ALL statuses, to both parties, ride the `game_trade_event` relay
//! (`stdb::subscriptions::trade_event_outbound`), so no arm here answers synchronously. Reducer
//! rejections are transient per-action failures (logged, never session-fatal), the vendor-arm
//! convention. Outside the world there is no `self_guid` to act as — dropped silently, like the
//! social family.

use super::super::social::self_guid;
use super::super::*;

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
        // Offer mutations (#121). The (bag, slot) pair maps onto the module's absolute slots the
        // same way the item family does: only the main pseudo-bag (255) is modelled — items inside
        // equipped sub-bags are logged + ignored, `handle_item`'s posture.
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
        // Accept / unaccept (#122). The accept body's `unknown1` is padding (vmangos skips it;
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
        // Proposal declines (#123): the client auto-answers a BeginTrade it can't take — busy
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
