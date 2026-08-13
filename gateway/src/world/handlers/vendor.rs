//! Vendor family: open a vendor's inventory window + buy/sell. The vendor-action seam below owns
//! the migrated opcodes; `handle_vendor` still carries the ones not yet moved across.

use super::super::*;
use super::push_buyback_view;

/// Durable reads and requests the vendor family needs, in the seam's own vocabulary so it can be
/// exercised without the broad `WorldStore`.
pub(crate) trait VendorActionStore: Send + Sync {
    fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>>;

    fn vendor_refuses_interaction(&self, vendor_guid: u64, player_guid: u64) -> Result<bool>;

    fn vendor_buy(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()>;

    /// The player's buyback ring, newest-first: `(item_entry, stack_count, price)` per entry (≤12).
    fn buyback_slots(&self, player_guid: u64) -> Vec<(u32, u32, u32)>;

    /// Bag slot of the item instance with `item_guid`. Item guids are globally unique, so no
    /// owner check is needed here — the module reducer enforces ownership on the repair call.
    fn vendor_item_slot(&self, item_guid: u64) -> Option<u8>;

    fn vendor_repair(&self, account_id: u64, self_guid: u64, npc_guid: u64, slot: u8)
        -> Result<()>;

    /// Re-purchase the ring entry at 0-based `slot` from `vendor_guid`. The gateway maps the wire
    /// `BuybackSlot` enum via [`BUYBACK_WIRE_SLOT_BASE`] before calling.
    fn vendor_buyback(&self, account_id: u64, self_guid: u64, vendor_guid: u64, slot: u8)
        -> Result<()>;
}

impl VendorActionStore for crate::stdb::Coordinator {
    fn vendor_stock(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>> {
        crate::stdb::Coordinator::vendor_items(self, vendor_guid)
    }

    fn vendor_refuses_interaction(&self, vendor_guid: u64, player_guid: u64) -> Result<bool> {
        crate::stdb::Coordinator::npc_refuses_interaction(self, vendor_guid, player_guid)
    }

    fn vendor_buy(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        crate::stdb::Coordinator::buy_item(
            self, account_id, self_guid, vendor_guid, item_entry, count,
        )
    }

    fn buyback_slots(&self, player_guid: u64) -> Vec<(u32, u32, u32)> {
        crate::stdb::Coordinator::buyback_ring(self, player_guid)
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

    fn vendor_buyback(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::buyback_item(self, account_id, self_guid, vendor_guid, slot)
    }
}

/// Wire slot of the first buyback tab entry (`BuybackSlot::Slot1`); the 12 ring entries render at
/// base + i and a client's slot enum parses back with − base. Both directions share this.
pub(crate) const BUYBACK_WIRE_SLOT_BASE: u16 = 69;

/// The ring the client can hold. Slots past the ring's end render cleared, so evictions and shifts
/// need no memory of what the tab showed before.
const BUYBACK_SLOTS: u16 = 12;

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

/// The one vendor-window-opening implementation, shared by the gated direct open below and the
/// gossip Browse-goods click (`query.rs`), which does not run the gate — matching each path's
/// pre-seam behavior. Replies RAW because gtker's typed SMSG_LIST_INVENTORY is the tbc/wrath shape.
pub(crate) fn vendor_open_outbound<St: VendorActionStore + ?Sized>(
    store: &St,
    vendor_guid: u64,
) -> Result<Vec<Outbound>> {
    let items = store.vendor_stock(vendor_guid)?;
    let (opcode, body) = codec::build_list_inventory_raw(vendor_guid, &items);
    Ok(vec![Outbound::Raw { opcode, body }])
}

/// Stock presence is the is-vendor signal gossip HELLO synthesizes its Browse-goods row from —
/// no `npc_flags` bit for it.
pub(crate) fn vendor_has_stock<St: VendorActionStore + ?Sized>(
    store: &St,
    vendor_guid: u64,
) -> Result<bool> {
    Ok(!store.vendor_stock(vendor_guid)?.is_empty())
}

pub(crate) fn dispatch_vendor_action<St: VendorActionStore + ?Sized>(
    store: &St,
    player: VendorActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<VendorActionOutcome> {
    match msg {
        // A refusing NPC answers nothing at all; an empty stock still answers, or the client waits
        // forever on the window it asked for.
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(c) => {
            let vendor_guid = c.guid.guid();
            if refuses_interaction(store, player, vendor_guid)? {
                return Ok(VendorActionOutcome::Handled {
                    outbound: Vec::new(),
                });
            }
            Ok(VendorActionOutcome::Handled {
                outbound: vendor_open_outbound(store, vendor_guid)?,
            })
        }
        // Successful purchases carry no reply — the item/purse subscriptions deliver the row
        // changes; only a rejection needs an explicit client-visible message.
        ClientOpcodeMessage::CMSG_BUY_ITEM(c) => {
            let vendor_guid = c.vendor.guid();
            let item_entry = c.item;
            let outbound = match store.vendor_buy(
                player.account_id,
                player.self_guid.unwrap_or(0),
                vendor_guid,
                item_entry,
                c.amount as u32,
            ) {
                Ok(()) => Vec::new(),
                Err(e) if classify_vendor_action_error(&e) == VendorActionErrorClass::GameplayRefusal => {
                    log::debug!(
                        "world: vendor_buy failed (account {}): {e}",
                        player.account_id
                    );
                    vec![Outbound::One(ServerOpcodeMessage::SMSG_BUY_FAILED(
                        Box::new(codec::build_buy_failed(vendor_guid, item_entry, &e.to_string())),
                    ))]
                }
                Err(e) => return Err(e),
            };
            Ok(VendorActionOutcome::Handled { outbound })
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
        // CMSG_BUYBACK_ITEM carries a wire BuybackSlot enum (69–81); map to the 0-based ring slot
        // the module reducer takes. A successful re-buy rebuilds the whole tab so shifted and
        // cleared entries appear immediately, but only once there is an actor to render it for.
        ClientOpcodeMessage::CMSG_BUYBACK_ITEM(c) => {
            let vendor_guid = c.guid.guid();
            let slot = c
                .slot
                .as_int()
                .saturating_sub(BUYBACK_WIRE_SLOT_BASE.into()) as u8;
            match store.vendor_buyback(
                player.account_id,
                player.self_guid.unwrap_or(0),
                vendor_guid,
                slot,
            ) {
                Ok(()) => Ok(VendorActionOutcome::Handled {
                    outbound: match player.self_guid {
                        Some(self_guid) => build_buyback_view(store, self_guid),
                        None => Vec::new(),
                    },
                }),
                Err(e) if classify_vendor_action_error(&e) == VendorActionErrorClass::GameplayRefusal => {
                    log::debug!(
                        "world: buyback_item ignored (account {}): {e}",
                        player.account_id
                    );
                    Ok(VendorActionOutcome::Handled {
                        outbound: Vec::new(),
                    })
                }
                Err(e) => Err(e),
            }
        }
        other => Ok(VendorActionOutcome::PassThrough(other)),
    }
}

/// Rebuild the buyback-tab view: a synthesized ITEM object per ring entry (fabricated guid
/// 0x4090…|slot — a client-only object, never a real instance) and ONE raw VALUES update carrying
/// all 12 VendorBuyback INV_SLOT pointers + BUYBACK_PRICE/TIMESTAMP arrays (those arrays are
/// gtker-walled past slot 0 → the shared raw encoder). Cleared slots write guid 0 / price 0.
pub(crate) fn build_buyback_view<St: VendorActionStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Vec<Outbound> {
    let ring = store.buyback_slots(self_guid);
    log::debug!("buyback view: guid={self_guid} ring_len={}", ring.len());
    render_buyback_view(self_guid, &ring)
}

/// World-entry replay of the persisted ring. An empty ring renders NOTHING: the client's descriptor
/// fields start zeroed, so a ring-less login stays byte-identical to one with no buyback tab at all.
/// In-session refreshes always render — a ring that just became empty must clear the tab.
pub(crate) fn build_buyback_view_replay<St: VendorActionStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Vec<Outbound> {
    let ring = store.buyback_slots(self_guid);
    log::debug!("buyback view: guid={self_guid} ring_len={}", ring.len());
    if ring.is_empty() {
        return Vec::new();
    }
    render_buyback_view(self_guid, &ring)
}

fn render_buyback_view(self_guid: u64, ring: &[(u32, u32, u32)]) -> Vec<Outbound> {
    let mut outbound = Vec::new();
    let mut mask = codec::update_mask::UpdateMaskValues::new();
    for i in 0..BUYBACK_SLOTS {
        let wire_slot = BUYBACK_WIRE_SLOT_BASE + i;
        let (fab_guid, price) = match ring.get(i as usize) {
            Some(&(entry, count, price)) => {
                let fab_guid = 0x4090_0000_0000_0000u64 | u64::from(i);
                let view = codec::ItemInstanceView {
                    guid: fab_guid,
                    entry,
                    owner_guid: self_guid,
                    slot: wire_slot as u8,
                    stack_count: count,
                    durability: 0,
                    max_durability: 0,
                    container_slots: 0,
                };
                outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                    Box::new(codec::build_item_create_object(&view)),
                )));
                (fab_guid, price)
            }
            None => (0, 0),
        };
        // PLAYER_FIELD_INV_SLOT guid pair for the buyback wire slot (base 486, 2 words/slot);
        // BUYBACK_PRICE_1 = 1226, BUYBACK_TIMESTAMP_1 = 1238 (5875 indices via gtker impls).
        mask.set_u64(486 + wire_slot * 2, fab_guid);
        mask.set_u32(1226 + i, price);
        mask.set_u32(1238 + i, 0);
    }
    let (opcode, body) = codec::build_values_update_raw(self_guid, &mask);
    outbound.push(Outbound::Raw { opcode, body });
    outbound
}

/// Vendor family (Tier 2): sell/buyback, awaiting migration to the seam above.
/// Forwards to the module reducers; a gameplay `Err` (no stock / no copper / out of range)
/// is per-action — log + ignore like the combat/loot arms, never tear the session down.
pub(crate) fn handle_vendor<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
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
                            Ok(()) => push_buyback_view(tx, store, self_guid)?,
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
            let slot = c
                .slot
                .as_int()
                .saturating_sub(BUYBACK_WIRE_SLOT_BASE.into()) as u8;
            match store.buyback_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.guid.guid(), slot) {
                // The re-bought item's bag CREATE rides the item relay; refresh the tab view.
                Ok(()) => {
                    if let WorldState::InWorld(iw) = &conn.state {
                        push_buyback_view(tx, store, iw.self_guid)?;
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
    use wow_world_messages::vanilla::{
        BuybackSlot, Guid, CMSG_BUYBACK_ITEM, CMSG_BUY_ITEM, CMSG_LIST_INVENTORY, CMSG_PING,
        CMSG_REPAIR_ITEM,
    };

    #[derive(Default)]
    struct InMemoryVendorActions {
        stock_requests: Mutex<Vec<u64>>,
        gate_requests: Mutex<Vec<(u64, u64)>>,
        buy_requests: Mutex<Vec<(u64, u64, u64, u32, u32)>>,
        repair_requests: Mutex<Vec<(u64, u64, u64, u8)>>,
        stock: Vec<codec::VendorItemView>,
        refuses: bool,
        stock_error: Option<String>,
        gate_error: Option<String>,
        buy_error: Option<String>,
        ring: Vec<(u32, u32, u32)>,
        item_slots: Vec<(u64, u8)>,
        repair_error: Option<String>,
        buyback_requests: Mutex<Vec<(u64, u64, u64, u8)>>,
        buyback_error: Option<String>,
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

        fn vendor_buy(
            &self,
            account_id: u64,
            self_guid: u64,
            vendor_guid: u64,
            item_entry: u32,
            count: u32,
        ) -> Result<()> {
            self.buy_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, vendor_guid, item_entry, count));
            match &self.buy_error {
                Some(error) => Err(anyhow::anyhow!("{error}")),
                None => Ok(()),
            }
        }

        fn buyback_slots(&self, _player_guid: u64) -> Vec<(u32, u32, u32)> {
            self.ring.clone()
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

        fn vendor_buyback(
            &self,
            account_id: u64,
            self_guid: u64,
            vendor_guid: u64,
            slot: u8,
        ) -> Result<()> {
            self.buyback_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, vendor_guid, slot));
            match &self.buyback_error {
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
        for (actions, msg) in [
            (
                InMemoryVendorActions {
                    gate_error: Some(
                        "npc_refuses_interaction reducer transport disconnected".into(),
                    ),
                    ..Default::default()
                },
                list_inventory(),
            ),
            (
                InMemoryVendorActions {
                    stock_error: Some("vendor_items reducer transport disconnected".into()),
                    ..Default::default()
                },
                list_inventory(),
            ),
            (
                InMemoryVendorActions {
                    buy_error: Some("buy_item reducer transport disconnected".into()),
                    ..Default::default()
                },
                buy_item(2589, 1),
            ),
            (
                InMemoryVendorActions {
                    repair_error: Some("repair_item reducer transport disconnected".into()),
                    ..Default::default()
                },
                repair_item(0),
            ),
            (
                InMemoryVendorActions {
                    buyback_error: Some("buyback_item reducer transport disconnected".into()),
                    ..Default::default()
                },
                buyback_item(BuybackSlot::Slot1),
            ),
        ] {
            let error = match dispatch_vendor_action(&actions, player(), msg) {
                Err(error) => error,
                Ok(_) => panic!("a dead reducer transport must end the session"),
            };
            assert!(format!("{error:#}").contains("reducer transport disconnected"));
        }
    }

    fn buy_item(item: u32, amount: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_BUY_ITEM(Box::new(CMSG_BUY_ITEM {
            vendor: Guid::new(VENDOR),
            item,
            amount,
            unknown1: 1,
        }))
    }

    fn repair_item(item_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(Box::new(CMSG_REPAIR_ITEM {
            npc: Guid::new(NPC),
            item: Guid::new(item_guid),
        }))
    }

    fn buyback_item(slot: BuybackSlot) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_BUYBACK_ITEM(Box::new(CMSG_BUYBACK_ITEM {
            guid: Guid::new(VENDOR),
            slot,
        }))
    }

    #[test]
    fn a_successful_purchase_requests_the_durable_buy_and_sends_no_packets() {
        let actions = InMemoryVendorActions::default();

        let outcome = dispatch_vendor_action(&actions, player(), buy_item(2589, 3)).unwrap();

        assert!(
            matches!(outcome, VendorActionOutcome::Handled { outbound } if outbound.is_empty())
        );
        assert_eq!(
            actions.buy_requests.lock().unwrap().as_slice(),
            &[(7, 42, VENDOR, 2589, 3)]
        );
    }

    #[test]
    fn a_rejected_purchase_sends_smsg_buy_failed() {
        let actions = InMemoryVendorActions {
            buy_error: Some("not enough money to buy that item".into()),
            ..Default::default()
        };

        let outcome = dispatch_vendor_action(&actions, player(), buy_item(2589, 1)).unwrap();

        assert!(matches!(
            outcome,
            VendorActionOutcome::Handled { outbound }
                if matches!(
                    outbound.as_slice(),
                    [Outbound::One(ServerOpcodeMessage::SMSG_BUY_FAILED(_))]
                )
        ));
    }

    #[test]
    fn a_buyer_without_an_actor_falls_back_to_the_legacy_zero_actor() {
        let actions = InMemoryVendorActions::default();
        let player = VendorActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        dispatch_vendor_action(&actions, player, buy_item(2589, 1)).unwrap();

        assert_eq!(
            actions.buy_requests.lock().unwrap().as_slice(),
            &[(7, 0, VENDOR, 2589, 1)]
        );
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

    const PLAYER_GUID: u64 = 42;

    /// The descriptor half of the tab, rebuilt from the ring independently of the seam so the
    /// fabricated guids and the price/timestamp indices are pinned, not just echoed.
    fn expected_values_update(ring: &[(u32, u32, u32)]) -> (u16, Vec<u8>) {
        let mut mask = codec::update_mask::UpdateMaskValues::new();
        for i in 0..12u16 {
            let (fab_guid, price) = match ring.get(i as usize) {
                Some(&(_, _, price)) => (0x4090_0000_0000_0000u64 | u64::from(i), price),
                None => (0, 0),
            };
            mask.set_u64(486 + (69 + i) * 2, fab_guid);
            mask.set_u32(1226 + i, price);
            mask.set_u32(1238 + i, 0);
        }
        codec::build_values_update_raw(PLAYER_GUID, &mask)
    }

    fn expected_create(i: u16, entry: u32, stack_count: u32) -> ServerOpcodeMessage {
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_item_create_object(
            &codec::ItemInstanceView {
                guid: 0x4090_0000_0000_0000u64 | u64::from(i),
                entry,
                owner_guid: PLAYER_GUID,
                slot: 69 + i as u8,
                stack_count,
                durability: 0,
                max_durability: 0,
                container_slots: 0,
            },
        )))
    }

    fn assert_renders_ring(outbound: &[Outbound], ring: &[(u32, u32, u32)]) {
        assert_eq!(outbound.len(), ring.len() + 1);
        for (i, &(entry, count, _)) in ring.iter().enumerate() {
            let expected = expected_create(i as u16, entry, count);
            assert!(
                matches!(&outbound[i], Outbound::One(message) if *message == expected),
                "ring slot {i} did not render its fabricated item"
            );
        }
        let (expected_opcode, expected_body) = expected_values_update(ring);
        assert!(matches!(
            outbound.last().unwrap(),
            Outbound::Raw { opcode, body } if *opcode == expected_opcode && *body == expected_body
        ));
    }

    #[test]
    fn the_buyback_view_renders_one_fabricated_item_per_ring_entry_plus_the_descriptor_update() {
        let ring = vec![(2589, 5, 120), (4540, 1, 30)];
        let actions = InMemoryVendorActions {
            ring: ring.clone(),
            ..Default::default()
        };

        let outbound = build_buyback_view(&actions, PLAYER_GUID);

        assert_renders_ring(&outbound, &ring);
    }

    #[test]
    fn a_full_ring_renders_all_thirteen_wire_slots_from_the_shared_base() {
        let ring: Vec<(u32, u32, u32)> = (0..12).map(|i| (100 + i, 1, 10 * i)).collect();
        let actions = InMemoryVendorActions {
            ring: ring.clone(),
            ..Default::default()
        };

        let outbound = build_buyback_view(&actions, PLAYER_GUID);

        assert_renders_ring(&outbound, &ring);
    }

    #[test]
    fn an_in_session_refresh_of_an_emptied_ring_still_clears_the_tab() {
        let actions = InMemoryVendorActions::default();

        let outbound = build_buyback_view(&actions, PLAYER_GUID);

        assert_renders_ring(&outbound, &[]);
    }

    #[test]
    fn a_login_replay_of_an_empty_ring_emits_nothing() {
        let actions = InMemoryVendorActions::default();

        assert!(build_buyback_view_replay(&actions, PLAYER_GUID).is_empty());
    }

    #[test]
    fn a_login_replay_of_a_persisted_ring_renders_it_like_an_in_session_refresh() {
        let ring = vec![(2589, 5, 120)];
        let actions = InMemoryVendorActions {
            ring: ring.clone(),
            ..Default::default()
        };

        let outbound = build_buyback_view_replay(&actions, PLAYER_GUID);

        assert_renders_ring(&outbound, &ring);
    }

    #[test]
    fn the_wire_slot_base_parses_the_clients_buyback_slots_back_to_ring_indices() {
        for (slot, expected) in [(BuybackSlot::Slot1, 0u32), (BuybackSlot::Slot13, 12)] {
            assert_eq!(
                slot.as_int().saturating_sub(BUYBACK_WIRE_SLOT_BASE.into()),
                expected
            );
        }
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

    #[test]
    fn buyback_wire_slots_map_to_zero_based_ring_slots_at_the_durable_call() {
        let actions = InMemoryVendorActions::default();

        dispatch_vendor_action(&actions, player(), buyback_item(BuybackSlot::Slot1)).unwrap();
        dispatch_vendor_action(&actions, player(), buyback_item(BuybackSlot::Slot13)).unwrap();

        assert_eq!(
            actions.buyback_requests.lock().unwrap().as_slice(),
            &[(7, 42, VENDOR, 0), (7, 42, VENDOR, 12)]
        );
    }

    #[test]
    fn a_successful_buyback_returns_the_full_rebuilt_view() {
        let ring = vec![(2589, 5, 120), (4540, 1, 30)];
        let actions = InMemoryVendorActions {
            ring: ring.clone(),
            ..Default::default()
        };

        let outcome =
            dispatch_vendor_action(&actions, player(), buyback_item(BuybackSlot::Slot1)).unwrap();

        let outbound = match outcome {
            VendorActionOutcome::Handled { outbound } => outbound,
            VendorActionOutcome::PassThrough(_) => panic!("buyback must be handled"),
        };
        assert_renders_ring(&outbound, &ring);
    }

    #[test]
    fn a_buyback_without_an_actor_falls_back_to_the_legacy_zero_actor_and_renders_no_view() {
        let actions = InMemoryVendorActions {
            ring: vec![(2589, 5, 120)],
            ..Default::default()
        };
        let player = VendorActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        let outcome =
            dispatch_vendor_action(&actions, player, buyback_item(BuybackSlot::Slot1)).unwrap();

        assert!(
            matches!(outcome, VendorActionOutcome::Handled { outbound } if outbound.is_empty())
        );
        assert_eq!(
            actions.buyback_requests.lock().unwrap().as_slice(),
            &[(7, 0, VENDOR, 0)]
        );
    }

    #[test]
    fn a_rejected_buyback_is_silent_and_non_fatal() {
        let actions = InMemoryVendorActions {
            buyback_error: Some("no such buyback slot".into()),
            ..Default::default()
        };

        let outcome =
            dispatch_vendor_action(&actions, player(), buyback_item(BuybackSlot::Slot1)).unwrap();

        assert!(
            matches!(outcome, VendorActionOutcome::Handled { outbound } if outbound.is_empty())
        );
    }
}
