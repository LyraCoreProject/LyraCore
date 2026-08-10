//! Vendor family: open a vendor's inventory window + buy/sell. Pure code-motion out of
//! `world/mod.rs`.

use super::super::*;
use super::push_buyback_view;

/// Vendor family (Tier 2): open a vendor's inventory window + buy/sell. `CMSG_LIST_INVENTORY` reads
/// the vendor's stock and replies RAW (gtker's typed `SMSG_LIST_INVENTORY` is the tbc/wrath shape).
/// Buy/sell forward to the module reducers; a gameplay `Err` (no stock / no copper / out of range)
/// is per-action — log + ignore like the combat/loot arms, never tear the session down.
pub(crate) fn handle_vendor<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // Open the vendor window: read the NPC's stock (joined with the item templates) and send the
        // RAW SMSG_LIST_INVENTORY. An empty stock still replies (a vendor with no items shows an
        // empty window) so the client doesn't hang waiting on the open it requested.
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(c) => {
            let vendor_guid = c.guid.guid();
            // An Unfriendly-or-below (or mask-hostile) vendor refuses the window —
            // silent drop, like the inspect/gameobject gates (vanilla NPCs just don't respond).
            if let WorldState::InWorld(iw) = &conn.state {
                if store
                    .npc_refuses_interaction(vendor_guid, iw.self_guid)
                    .unwrap_or(false)
                {
                    return Ok(None);
                }
            }
            let items = store.vendor_items(vendor_guid)?;
            let (opcode, body) = codec::build_list_inventory_raw(vendor_guid, &items);
            send(tx, Outbound::Raw { opcode, body })?;
        }
        // Buy `amount` of an item ENTRY from the vendor. The module gates it (vendor stock / range /
        // copper) and replicates the new item + purse via the player's subscription; a rejection is a
        // transient per-action failure — logged and relayed to the buyer as SMSG_BUY_FAILED (red
        // on-screen error) so they know *why* the purchase was refused, never session-fatal.
        ClientOpcodeMessage::CMSG_BUY_ITEM(c) => {
            let vendor_guid = c.vendor.guid();
            let item_entry = c.item;
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.buy_item(
                conn.account_id,
                self_guid,
                vendor_guid,
                item_entry,
                c.amount as u32,
            ) {
                log::debug!("world: buy_item failed (account {}): {e}", conn.account_id);
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(
                        codec::build_buy_failed(vendor_guid, item_entry, &e.to_string()),
                    ))),
                )?;
            }
        }
        // Sell an item back to a vendor. CMSG_SELL_ITEM carries the item's INSTANCE guid, but the
        // module's `sell_item` takes the inventory SLOT — so resolve guid → slot from the player's
        // own items, then call. An unmatched guid (already sold / not ours) is logged + ignored.
        ClientOpcodeMessage::CMSG_SELL_ITEM(c) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => Some(iw.self_guid),
                WorldState::CharSelect => None,
            };
            if let Some(self_guid) = self_guid {
                let item_guid = c.item.guid();
                match store
                    .player_items(self_guid)?
                    .into_iter()
                    .find(|i| i.guid == item_guid)
                {
                    Some(inst) => {
                        match store.sell_item(
                            conn.account_id,
                            self_guid,
                            c.vendor.guid(),
                            inst.slot,
                        ) {
                            // Reflect the new ring in the buyback tab immediately.
                            Ok(()) => push_buyback_view(tx, store, self_guid, false)?,
                            Err(e) => log::debug!(
                                "world: sell_item ignored (account {}): {e}",
                                conn.account_id
                            ),
                        }
                    }
                    None => log::debug!(
                        "world: sell_item for unknown item guid {item_guid} (account {})",
                        conn.account_id
                    ),
                }
            }
        }
        // Re-buy the last-sold item from a vendor's buyback tab. CMSG_BUYBACK_ITEM carries the vendor
        // guid + a BuybackSlot enum (69–81). Map to 0-based slot index and call the module reducer.
        ClientOpcodeMessage::CMSG_BUYBACK_ITEM(c) => {
            let slot = c.slot.as_int().saturating_sub(69) as u8;
            match store.buyback_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.guid.guid(), slot) {
                // The re-bought item's bag CREATE rides the item relay; refresh the tab view.
                Ok(()) => {
                    if let WorldState::InWorld(iw) = &conn.state {
                        push_buyback_view(tx, store, iw.self_guid, false)?;
                    }
                }
                Err(e) => log::debug!(
                    "world: buyback_item ignored (account {}): {e}",
                    conn.account_id
                ),
            }
        }
        // Repair an item at an armorer. CMSG_REPAIR_ITEM carries the NPC guid + the item INSTANCE guid;
        // the module's repair_item takes the inventory SLOT, so resolve guid → slot from the player's
        // own items (like CMSG_SELL_ITEM). An unmatched guid is logged + ignored; a gameplay Err
        // (out of range / too poor / NPC can't repair) is per-action, never session-fatal — but unlike
        // the earlier debug-log-and-swallow, it's ALSO relayed to the player as a self-only system chat
        // line (`SMSG_MESSAGECHAT` System, like the GM dot-command error path) so a rejected repair
        // isn't indistinguishable from a client that never sent the packet (#514). This does NOT by
        // itself close #514: it only turns 3 of the issue's 4 candidate causes (NPC-gate rejection,
        // a cost that exceeds the player's purse, the module's own error paths) from a silent no-op
        // into a diagnosable, visible one — a real "not enough money" now reads as exactly that
        // instead of nothing happening. Whether `rules::repair_cost`'s 1-copper-per-point proxy
        // actually OVER-charges relative to the client's own DBC-driven estimate (issue candidate
        // cause 2) is unconfirmed without a live cost comparison, and cause 3 (does the button even
        // send the packet) is `needs-live-eyeball` in the issue itself — neither is guessable
        // headlessly, so neither is touched here. The client's per-item clicks carry the item guid;
        // the REPAIR-ALL button sends guid 0 (a live-verified finding — the earlier "no repair-all
        // bit" claim here was wrong) → the module's whole-body slot u8::MAX.
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(c) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => Some(iw.self_guid),
                WorldState::CharSelect => None,
            };
            if let Some(self_guid) = self_guid {
                let item_guid = c.item.guid();
                // Repair ALL: the 1.12 client's "repair all" button sends
                // item guid 0 (the earlier per-item-only comment was wrong) — route it to the
                // module's existing whole-body slot (u8::MAX, already implemented + charged).
                if item_guid == 0 {
                    if let Err(e) = store.repair_item(conn.account_id, self_guid, c.npc.guid(), u8::MAX) {
                        log::debug!(
                            "world: repair_all ignored (account {}): {e}",
                            conn.account_id
                        );
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                codec::build_gm_system_message(e.to_string()),
                            ))),
                        )?;
                    }
                } else {
                    match store
                        .player_items(self_guid)?
                        .into_iter()
                        .find(|i| i.guid == item_guid)
                    {
                        Some(inst) => {
                            if let Err(e) =
                                store.repair_item(conn.account_id, self_guid, c.npc.guid(), inst.slot)
                            {
                                log::debug!(
                                    "world: repair_item ignored (account {}): {e}",
                                    conn.account_id
                                );
                                send(
                                    tx,
                                    Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                        codec::build_gm_system_message(e.to_string()),
                                    ))),
                                )?;
                            }
                        }
                        None => log::debug!(
                            "world: repair_item for unknown item guid {item_guid} (account {})",
                            conn.account_id
                        ),
                    }
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
