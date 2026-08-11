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
        other => Ok(Some(other)),
    }
}
