//! Bank family: open the bank window, right-click to bank, right-click to withdraw. Same shape as
//! the vendor family.

use super::super::*;
use super::send_show_bank;

/// Bank family: `CMSG_BANKER_ACTIVATE` opens the bank window (a standing-refusing banker gets no
/// reply at all, matching the vendor `CMSG_LIST_INVENTORY` gate); `CMSG_AUTOBANK_ITEM` deposits a
/// carried item into the first free bank slot, `CMSG_AUTOSTORE_BANK_ITEM` withdraws a banked item
/// into the first free carry slot — both resolve to the same module entry point, which infers the
/// direction from the source slot and carries the banker-proximity gate for free (it reuses the
/// move core). Only the main pseudo-bag (255) is addressed, matching the item handler's restriction.
pub(crate) fn handle_bank<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0 — same restriction as the item handler
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
        // Right-click a bag item with the bank open → deposit into the first free bank slot.
        ClientOpcodeMessage::CMSG_AUTOBANK_ITEM(c) => {
            if c.bag_index == MAIN_BAG {
                if let Err(e) = store.auto_bank_item(
                    conn.account_id,
                    social::self_guid(conn).unwrap_or(0),
                    c.slot_index,
                ) {
                    log::debug!(
                        "world: auto_bank_item (deposit) rejected (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                            Box::new(codec::build_inventory_change_failure()),
                        )),
                    )?;
                }
            } else {
                log::debug!(
                    "world: autobank from sub-bag {} unsupported (account {})",
                    c.bag_index,
                    conn.account_id
                );
            }
        }
        // Buy the next bank bag slot from the named banker. Success and every refusal relay
        // `SMSG_BUY_BANK_SLOT_RESULT`; the refusal code rides the module's `[N]` error tag.
        ClientOpcodeMessage::CMSG_BUY_BANK_SLOT(c) => {
            let banker_guid = c.guid.guid();
            let outcome = store.buy_bank_slot(
                conn.account_id,
                social::self_guid(conn).unwrap_or(0),
                banker_guid,
            );
            let err_text = outcome.as_ref().err().map(ToString::to_string);
            if let Some(e) = &err_text {
                log::debug!(
                    "world: buy_bank_slot rejected (account {}): {e}",
                    conn.account_id
                );
            }
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(
                    codec::build_buy_bank_slot_reply(err_text.as_deref().map_or(Ok(()), Err)),
                )),
            )?;
        }
        // Right-click a banked item → withdraw into the first free backpack/bag slot.
        ClientOpcodeMessage::CMSG_AUTOSTORE_BANK_ITEM(c) => {
            if c.bag_index == MAIN_BAG {
                if let Err(e) = store.auto_bank_item(
                    conn.account_id,
                    social::self_guid(conn).unwrap_or(0),
                    c.slot_index,
                ) {
                    log::debug!(
                        "world: auto_bank_item (withdraw) rejected (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                            Box::new(codec::build_inventory_change_failure()),
                        )),
                    )?;
                }
            } else {
                log::debug!(
                    "world: autostore-bank from sub-bag {} unsupported (account {})",
                    c.bag_index,
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
