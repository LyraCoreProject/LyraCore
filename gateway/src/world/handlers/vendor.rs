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
        other => Ok(VendorActionOutcome::PassThrough(other)),
    }
}

/// Vendor family (Tier 2): sell/buyback/repair, awaiting migration to the seam above.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{Guid, CMSG_BUY_ITEM, CMSG_LIST_INVENTORY, CMSG_PING};

    #[derive(Default)]
    struct InMemoryVendorActions {
        stock_requests: Mutex<Vec<u64>>,
        gate_requests: Mutex<Vec<(u64, u64)>>,
        buy_requests: Mutex<Vec<(u64, u64, u64, u32, u32)>>,
        stock: Vec<codec::VendorItemView>,
        refuses: bool,
        stock_error: Option<String>,
        gate_error: Option<String>,
        buy_error: Option<String>,
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
    }

    const VENDOR: u64 = 0xF130_0000_0000_0777;

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
