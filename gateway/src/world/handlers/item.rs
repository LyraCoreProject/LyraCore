//! Item-action family: equip / unequip / move items within the player's inventory. Pure
//! code-motion out of `world/mod.rs`.

use super::super::*;

/// Item-action family (inventory slice): equip / unequip / move items within the player's MAIN
/// inventory container (`INVENTORY_SLOT_BAG_0` = 255 — equipment slots 0..=18 and the backpack 23..=38).
/// The module owns all slot validation (equip rules, required-level, swap, full-backpack); the gateway
/// only maps the client's (bag, slot) addressing onto the module's absolute-slot reducers and relays
/// the resulting row changes back via the player's item subscription. Items inside equipped SUB-bags
/// (bag 19..=22) aren't modelled yet → logged + ignored. Every reducer rejection is a transient
/// per-action failure (logged, never session-fatal), exactly like the vendor buy/sell arms.
pub(crate) fn handle_item<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0 — backpack + equipped slots share this pseudo-bag
    const EQUIP_SLOT_END: u8 = 18; // EQUIPMENT_SLOT_END — last equipment slot (main-hand=15, off=16…)
    match msg {
        // Right-click a backpack item → auto-equip into its matching slot (module resolves the slot).
        ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(c) => {
            if c.source_bag == MAIN_BAG {
                let self_guid = match &conn.state {
                    WorldState::InWorld(iw) => iw.self_guid,
                    WorldState::CharSelect => 0,
                };
                if let Err(e) = store.equip_item(conn.account_id, self_guid, c.source_slot) {
                    log::debug!(
                        "world: equip_item rejected (account {}): {e}",
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
                    "world: autoequip from sub-bag {} unsupported (account {})",
                    c.source_bag,
                    conn.account_id
                );
            }
        }
        // Drag an item between two main-inventory slots — covers drag-to-equip, drag-to-unequip, and
        // backpack rearrange (the module's move primitive validates equip-slot transitions).
        ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(c) => {
            if let Err(e) = store.move_item(
                conn.account_id,
                social::self_guid(conn).unwrap_or(0),
                c.source_slot.as_int(),
                c.destination_slot.as_int(),
            ) {
                log::debug!(
                    "world: move_item rejected (account {}): {e}",
                    conn.account_id
                );
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                        Box::new(codec::build_inventory_change_failure()),
                    )),
                )?;
            }
        }
        // Right-click an equipped item → auto-store it into the first free backpack slot (unequip).
        ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c) => {
            if c.source_bag == MAIN_BAG && c.source_slot <= EQUIP_SLOT_END {
                if let Err(e) = store.unequip_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.source_slot) {
                    log::debug!(
                        "world: unequip_item rejected (account {}): {e}",
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
                    "world: autostore (bag {} slot {}) unsupported (account {})",
                    c.source_bag,
                    c.source_slot,
                    conn.account_id
                );
            }
        }
        // Right-click a consumable (food/drink/potion/bandage) → use it. Only the main bag (255) is
        // modeled, so `bag_slot` is our flat inventory slot. The module applies the on-use effect + stack
        // decrement; a per-action Err (not usable / empty) is logged, never session-fatal.
        ClientOpcodeMessage::CMSG_USE_ITEM(c) => {
            if c.bag_index == MAIN_BAG {
                // Item-starts-quest: an item whose template carries `start_quest`
                // opens that quest's DETAILS screen instead of the normal use/consume path — the item
                // is NOT consumed (cmangos's "dropped tabard starts a quest" pattern). Checked BEFORE
                // `use_item` so a start-quest item never falls through to the generic (usually
                // rejected, "not consumable") use path. Needs the in-world player guid to resolve the
                // slot's owner; a CharSelect use (shouldn't happen — items are in-world only) falls
                // through to the ordinary path below, which errors harmlessly.
                let self_guid = match &conn.state {
                    WorldState::InWorld(iw) => Some(iw.self_guid),
                    WorldState::CharSelect => None,
                };
                let start_quest = self_guid.and_then(|g| store.item_start_quest(g, c.bag_slot));
                if let Some((item_guid, quest_id)) = start_quest {
                    if let Some(detail) = store.quest_detail(quest_id)? {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(
                                Box::new(codec::build_quest_details(item_guid, &detail)),
                            )),
                        )?;
                    }
                } else if let Err(e) =
                    store.use_item(conn.account_id, self_guid.unwrap_or(0), c.bag_slot)
                {
                    log::debug!(
                        "world: use_item rejected (account {}): {e}",
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
                    "world: use_item from sub-bag {} unsupported (account {})",
                    c.bag_index,
                    conn.account_id
                );
            }
        }
        // Cross-container swap. We only model the main inventory, so both ends must be bag 255.
        // (NB: gtker's field is spelled `destionation_slot` — a typo in the generated struct.)
        ClientOpcodeMessage::CMSG_SWAP_ITEM(c) => {
            if c.source_bag == MAIN_BAG && c.destination_bag == MAIN_BAG {
                if let Err(e) = store.move_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.source_slot, c.destionation_slot)
                {
                    log::debug!(
                        "world: move_item (swap) rejected (account {}): {e}",
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
                    "world: cross-bag swap unsupported (account {})",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
