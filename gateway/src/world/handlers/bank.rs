//! Bank family: open the bank window. Same shape as the vendor family.

use super::super::*;
use super::send_show_bank;

/// Bank family: `CMSG_BANKER_ACTIVATE` opens the bank window. A standing-refusing banker gets no
/// reply at all, matching the vendor `CMSG_LIST_INVENTORY` gate.
pub(crate) fn handle_bank<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        ClientOpcodeMessage::CMSG_BANKER_ACTIVATE(c) => {
            let banker_guid = c.guid.guid();
            if let WorldState::InWorld(iw) = &conn.state {
                if store
                    .npc_refuses_interaction(banker_guid, iw.self_guid)
                    .unwrap_or(false)
                {
                    return Ok(None);
                }
            }
            send_show_bank(tx, banker_guid)?;
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
