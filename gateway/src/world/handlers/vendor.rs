//! Vendor family: open a vendor's inventory window + buy/sell. The vendor-action seam below owns
//! the migrated opcodes; `handle_vendor` still carries the ones not yet moved across.

use super::super::*;
use super::push_buyback_view;

/// Durable reads and requests the vendor family needs, in the seam's own vocabulary so it can be
/// exercised without the broad `WorldStore`.
pub(crate) trait VendorActionStore: Send + Sync {
    fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>>;

    fn vendor_refuses_interaction(&self, vendor_guid: u64, player_guid: u64) -> Result<bool>;

    /// Bag slot of the item instance with `item_guid`. Item guids are globally unique, so no
    /// owner check is needed here — the module reducer enforces ownership on the repair call.
    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8>;

    fn vendor_repair(&self, account_id: u64, self_guid: u64, npc_guid: u64, slot: u8)
        -> Result<()>;
}

impl VendorActionStore for crate::stdb::Coordinator {
    fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        crate::stdb::Coordinator::vendor_items(self, vendor_guid)
    }

    fn vendor_refuses_interaction(&self, vendor_guid: u64, player_guid: u64) -> Result<bool> {
        crate::stdb::Coordinator::npc_refuses_interaction(self, vendor_guid, player_guid)
    }

    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8> {
        crate::stdb::Coordinator::item_slot_by_guid(self, 0, item_guid)
    }

    fn vendor_repair(
        &self,
        account_id: u64,
        self_guid: u64,
        npc_guid: u64,
        slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::repair_item(self, account_id, self_guid, npc_guid, slot)
    }
}

/// Who is asking. `self_guid` is `None` before world entry — the character-select state has no
/// actor, so gates that need one are skipped rather than run against a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VendorActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum VendorActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VendorActionErrorClass {
    GameplayRefusal,
    Fatal,
}

fn classify_vendor_action_error(error: &anyhow::Error) -> VendorActionErrorClass {
    if error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
    {
        VendorActionErrorClass::Fatal
    } else {
        VendorActionErrorClass::GameplayRefusal
    }
}

/// The interaction gate fails open — missing standing data must not lock a player out of a vendor —
/// but a dead reducer transport is not missing data and ends the session.
fn refuses_interaction<St: VendorActionStore + ?Sized>(
    store: &St,
    player: VendorActionPlayer,
    vendor_guid: u64,
) -> Result<bool> {
    let Some(self_guid) = player.self_guid else {
        return Ok(false);
    };
    match store.vendor_refuses_interaction(vendor_guid, self_guid) {
        Ok(refuses) => Ok(refuses),
        Err(e) if classify_vendor_action_error(&e) == VendorActionErrorClass::GameplayRefusal => {
            log::debug!(
                "world: vendor {vendor_guid} interaction gate unavailable (account {}): {e}",
                player.account_id
            );
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn dispatch_vendor_action<St: VendorActionStore + ?Sized>(
    store: &St,
    player: VendorActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<VendorActionOutcome> {
    match msg {
        // A refusing NPC answers nothing at all; an empty stock still answers, or the client waits
        // forever on the window it asked for. Replies RAW because gtker's typed
        // SMSG_LIST_INVENTORY is the tbc/wrath shape.
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(c) => {
            let vendor_guid = c.guid.guid();
            if refuses_interaction(store, player, vendor_guid)? {
                return Ok(VendorActionOutcome::Handled {
                    outbound: Vec::new(),
                });
            }
            let items = store.vendor_stock(vendor_guid)?;
            let (opcode, body) = codec::build_list_inventory_raw(vendor_guid, &items);
            Ok(VendorActionOutcome::Handled {
                outbound: vec![Outbound::Raw { opcode, body }],
            })
        }
        // CMSG_REPAIR_ITEM carries the item INSTANCE guid, but the module's repair takes the
        // inventory SLOT; guid 0 means repair-all, routed to the whole-body slot instead of a
        // guid lookup. An unmatched guid (already sold / not ours) is a silent no-op.
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(c) => {
            let Some(self_guid) = player.self_guid else {
                return Ok(VendorActionOutcome::Handled {
                    outbound: Vec::new(),
                });
            };
            let item_guid = c.item.guid();
            let slot = if item_guid == 0 {
                Some(u8::MAX)
            } else {
                store.vendor_item_slot(item_guid)
            };
            let Some(slot) = slot else {
                return Ok(VendorActionOutcome::Handled {
                    outbound: Vec::new(),
                });
            };
            match store.vendor_repair(player.account_id, self_guid, c.npc.guid(), slot) {
                Ok(()) => Ok(VendorActionOutcome::Handled {
                    outbound: Vec::new(),
                }),
                Err(e)
                    if classify_vendor_action_error(&e)
                        == VendorActionErrorClass::GameplayRefusal =>
                {
                    log::debug!(
                        "world: repair_item ignored (account {}): {e}",
                        player.account_id
                    );
                    Ok(VendorActionOutcome::Handled {
                        outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                            Box::new(codec::build_gm_system_message(e.to_string())),
                        ))],
                    })
                }
                Err(e) => Err(e),
            }
        }
        other => Ok(VendorActionOutcome::PassThrough(other)),
    }
}

/// Vendor family (Tier 2): buy/sell/buyback, awaiting migration to the seam above.
/// Buy/sell forward to the module reducers; a gameplay `Err` (no stock / no copper / out of range)
/// is per-action — log + ignore like the combat/loot arms, never tear the session down.
pub(crate) fn handle_vendor<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
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
        other => return Ok(Some(other)),
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{Guid, CMSG_LIST_INVENTORY, CMSG_PING, CMSG_REPAIR_ITEM};

    #[derive(Default)]
    struct InMemoryVendorActions {
        stock_requests: Mutex<Vec<u64>>,
        gate_requests: Mutex<Vec<(u64, u64)>>,
        repair_requests: Mutex<Vec<(u64, u64, u64, u8)>>,
        stock: Vec<codec::VendorItemView>,
        refuses: bool,
        stock_error: Option<String>,
        gate_error: Option<String>,
        item_slots: Vec<(u64, u8)>,
        repair_error: Option<String>,
    }

    impl VendorActionStore for InMemoryVendorActions {
        fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
            self.stock_requests.lock().unwrap().push(vendor_guid);
            match &self.stock_error {
                Some(error) => Err(anyhow::anyhow!("{error}")),
                None => Ok(self.stock.clone()),
            }
        }

        fn vendor_refuses_interaction(&self, vendor_guid: u64, player_guid: u64) -> Result<bool> {
            self.gate_requests
                .lock()
                .unwrap()
                .push((vendor_guid, player_guid));
            match &self.gate_error {
                Some(error) => Err(anyhow::anyhow!("{error}")),
                None => Ok(self.refuses),
            }
        }

        fn vendor_item_slot(&self, item_guid: u64) -> Option<u8> {
            self.item_slots
                .iter()
                .find(|(g, _)| *g == item_guid)
                .map(|&(_, s)| s)
        }

        fn vendor_repair(
            &self,
            account_id: u64,
            self_guid: u64,
            npc_guid: u64,
            slot: u8,
        ) -> Result<()> {
            self.repair_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, npc_guid, slot));
            match &self.repair_error {
                Some(error) => Err(anyhow::anyhow!("{error}")),
                None => Ok(()),
            }
        }
    }

    const VENDOR: u64 = 0xF130_0000_0000_0777;
    const NPC: u64 = 0xF130_0000_0000_0200;

    fn player() -> VendorActionPlayer {
        VendorActionPlayer {
            account_id: 7,
            self_guid: Some(42),
        }
    }

    fn list_inventory() -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(CMSG_LIST_INVENTORY {
            guid: Guid::new(VENDOR),
        })
    }

    fn stock_item(item_entry: u32) -> codec::VendorItemView {
        codec::VendorItemView {
            item_entry,
            display_id: 1234,
            buy_price: 500,
            max_durability: 0,
            max_count: 0,
            buy_count: 1,
        }
    }

    #[test]
    fn opening_a_vendor_returns_its_stock_as_the_raw_vendor_window() {
        let actions = InMemoryVendorActions {
            stock: vec![stock_item(2589), stock_item(4540)],
            ..Default::default()
        };

        let outcome = dispatch_vendor_action(&actions, player(), list_inventory()).unwrap();

        let expected = codec::build_list_inventory_raw(VENDOR, &actions.stock);
        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound }
                if matches!(outbound.as_slice(), [Outbound::Raw { opcode, body }]
                    if (*opcode, body.clone()) == expected)
        ));
        assert_eq!(actions.stock_requests.lock().unwrap().as_slice(), &[VENDOR]);
        assert_eq!(
            actions.gate_requests.lock().unwrap().as_slice(),
            &[(VENDOR, 42)]
        );
    }

    #[test]
    fn a_refusing_vendor_answers_nothing_and_its_stock_is_never_read() {
        let actions = InMemoryVendorActions {
            refuses: true,
            stock: vec![stock_item(2589)],
            ..Default::default()
        };

        let outcome = dispatch_vendor_action(&actions, player(), list_inventory()).unwrap();

        assert!(
            matches!(outcome, VendorActionOutcome::Handled { outbound } if outbound.is_empty())
        );
        assert!(actions.stock_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn an_empty_stock_still_opens_the_window() {
        let actions = InMemoryVendorActions::default();

        let outcome = dispatch_vendor_action(&actions, player(), list_inventory()).unwrap();

        let expected = codec::build_list_inventory_raw(VENDOR, &[]);
        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound }
                if matches!(outbound.as_slice(), [Outbound::Raw { opcode, body }]
                    if (*opcode, body.clone()) == expected)
        ));
    }

    #[test]
    fn an_unavailable_interaction_gate_still_opens_the_window() {
        let actions = InMemoryVendorActions {
            gate_error: Some("no standing row for that faction".into()),
            stock: vec![stock_item(2589)],
            ..Default::default()
        };

        let outcome = dispatch_vendor_action(&actions, player(), list_inventory()).unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound } if outbound.len() == 1
        ));
    }

    #[test]
    fn reducer_transport_failure_is_session_fatal() {
        for actions in [
            InMemoryVendorActions {
                gate_error: Some("npc_refuses_interaction reducer transport disconnected".into()),
                ..Default::default()
            },
            InMemoryVendorActions {
                stock_error: Some("vendor_items reducer transport disconnected".into()),
                ..Default::default()
            },
            InMemoryVendorActions {
                repair_error: Some("repair_item reducer transport disconnected".into()),
                ..Default::default()
            },
        ] {
            let msg = if actions.repair_error.is_some() {
                repair_item(0)
            } else {
                list_inventory()
            };
            let error = match dispatch_vendor_action(&actions, player(), msg) {
                Err(error) => error,
                Ok(_) => panic!("a dead reducer transport must end the session"),
            };
            assert!(format!("{error:#}").contains("reducer transport disconnected"));
        }
    }

    fn repair_item(item_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(Box::new(CMSG_REPAIR_ITEM {
            npc: Guid::new(NPC),
            item: Guid::new(item_guid),
        }))
    }

    #[test]
    fn repairing_one_item_resolves_its_guid_to_the_durable_slot() {
        const ITEM: u64 = 0x4000_0000_0000_0042;
        let actions = InMemoryVendorActions {
            item_slots: vec![(ITEM, 7)],
            ..Default::default()
        };

        let outcome = dispatch_vendor_action(&actions, player(), repair_item(ITEM)).unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound } if outbound.is_empty()
        ));
        assert_eq!(
            actions.repair_requests.lock().unwrap().as_slice(),
            &[(7, 42, NPC, 7)]
        );
    }

    #[test]
    fn repairing_item_guid_zero_dispatches_the_whole_body_slot() {
        let actions = InMemoryVendorActions::default();

        let outcome = dispatch_vendor_action(&actions, player(), repair_item(0)).unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound } if outbound.is_empty()
        ));
        assert_eq!(
            actions.repair_requests.lock().unwrap().as_slice(),
            &[(7, 42, NPC, u8::MAX)]
        );
    }

    #[test]
    fn repairing_an_unknown_item_guid_is_a_harmless_no_op() {
        let actions = InMemoryVendorActions::default();

        let outcome = dispatch_vendor_action(&actions, player(), repair_item(0x99)).unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound } if outbound.is_empty()
        ));
        assert!(actions.repair_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rejected_repair_relays_the_same_private_system_message() {
        let actions = InMemoryVendorActions {
            repair_error: Some("not enough money to repair".into()),
            ..Default::default()
        };

        let outcome = dispatch_vendor_action(&actions, player(), repair_item(0)).unwrap();

        let expected = codec::build_gm_system_message("not enough money to repair".to_string());
        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound }
                if matches!(outbound.as_slice(),
                    [Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(m))]
                        if **m == expected)
        ));
    }

    #[test]
    fn a_player_without_an_actor_skips_the_interaction_gate() {
        let actions = InMemoryVendorActions {
            refuses: true,
            ..Default::default()
        };
        let player = VendorActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        let outcome = dispatch_vendor_action(&actions, player, list_inventory()).unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound } if outbound.len() == 1
        ));
        assert!(actions.gate_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn unrelated_opcodes_pass_through_to_the_next_dispatcher() {
        let actions = InMemoryVendorActions::default();

        let outcome = dispatch_vendor_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }
}
