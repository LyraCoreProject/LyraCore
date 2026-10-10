//! Bank family: open the bank window, right-click to bank, right-click to withdraw. Same shape as
//! the vendor family.

use super::super::*;
use super::send_show_bank;
use crate::stdb::{classify, DurableFailure};

/// Bank Durable Requests.
pub(crate) trait BankStore: Send + Sync {
    /// Auto-bank/auto-store-bank the item in `slot` (`CMSG_AUTOBANK_ITEM`/`CMSG_AUTOSTORE_BANK_ITEM`
    /// — right-click to bank, right-click to withdraw). The module infers the direction from `slot`
    /// and resolves the receiving free slot itself; a full destination (bank or carry space) is a
    /// per-action `Err`.
    fn auto_bank_item(&self, actor: Actor, slot: u8) -> Result<()>;

    /// Buy the next bank bag slot from `banker_guid` (`CMSG_BUY_BANK_SLOT`). A refusal `Err` leads
    /// with its `SMSG_BUY_BANK_SLOT_RESULT` code in brackets (the trainer `[N]` precedent).
    fn buy_bank_slot(&self, actor: Actor, banker_guid: u64) -> Result<()>;
}

/// Move the item in `slot_index` between carry space and the bank. A Refusal, or no Character yet,
/// answers `SMSG_INVENTORY_CHANGE_FAILURE`; a Transport Loss ends the World Session.
fn auto_bank<St: BankStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &WorldConn,
    slot_index: u8,
    direction: &str,
) -> Result<()> {
    if let Some(actor) = social::self_guid(conn).and_then(Actor::new) {
        let Err(error) = store.auto_bank_item(actor, slot_index) else {
            return Ok(());
        };
        if matches!(classify(&error), DurableFailure::TransportLoss) {
            return Err(error);
        }
        log::debug!(
            "world: auto_bank_item ({direction}) rejected (account {}): {error}",
            conn.account_id
        );
    }
    send(
        tx,
        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
            Box::new(codec::build_inventory_change_failure()),
        )),
    )
}

/// Bank family: `CMSG_BANKER_ACTIVATE` opens the bank window (a standing-refusing banker gets no
/// reply at all, matching the vendor `CMSG_LIST_INVENTORY` gate); `CMSG_AUTOBANK_ITEM` deposits a
/// carried item into the first free bank slot, `CMSG_AUTOSTORE_BANK_ITEM` withdraws a banked item
/// into the first free carry slot — both resolve to the same module entry point, which infers the
/// direction from the source slot and carries the banker-proximity gate for free (it reuses the
/// move core). Only the main pseudo-bag (255) is addressed, matching the item handler's restriction.
pub(crate) fn handle_bank<St: BankStore + NpcStore + ?Sized>(
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
                auto_bank(tx, store, conn, c.slot_index, "deposit")?;
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
            // No Character yet gets an untagged reason, which answers NotBanker.
            let refusal = match social::self_guid(conn)
                .and_then(Actor::new)
                .map(|actor| store.buy_bank_slot(actor, banker_guid))
            {
                Some(Ok(())) => None,
                None => Some(String::new()),
                Some(Err(error)) => match classify(&error) {
                    DurableFailure::Refusal { reason } => Some(reason.to_string()),
                    DurableFailure::TransportLoss => return Err(error),
                },
            };
            if let Some(reason) = &refusal {
                log::debug!(
                    "world: buy_bank_slot rejected (account {}): {reason}",
                    conn.account_id
                );
            }
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(
                    codec::build_buy_bank_slot_reply(refusal.as_deref().map_or(Ok(()), Err)),
                )),
            )?;
        }
        // Right-click a banked item → withdraw into the first free backpack/bag slot.
        ClientOpcodeMessage::CMSG_AUTOSTORE_BANK_ITEM(c) => {
            if c.bag_index == MAIN_BAG {
                auto_bank(tx, store, conn, c.slot_index, "withdraw")?;
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
