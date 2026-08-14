//! Item-action dispatcher: protocol mapping and feedback for equip, use, and inventory operations.

use super::super::*;
use super::quest::{item_started_quest, QuestActionPlayer, QuestActionStore};

const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0 — backpack + equipped slots share this pseudo-bag
const EQUIP_SLOT_END: u8 = 18; // EQUIPMENT_SLOT_END — last equipment slot (main-hand=15, off=16…)

pub(crate) trait ItemActionStore: Send + Sync {
    fn equip_item(&self, account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()>;
    fn unequip_item(&self, account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()>;
    fn move_item(&self, account_id: u64, actor_guid: u64, from_slot: u8, to_slot: u8)
        -> Result<()>;
    fn use_item(&self, account_id: u64, actor_guid: u64, slot: u8) -> Result<()>;
}

impl ItemActionStore for crate::stdb::Coordinator {
    fn equip_item(&self, account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()> {
        crate::stdb::Coordinator::equip_item(self, account_id, actor_guid, from_slot)
    }

    fn unequip_item(&self, account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()> {
        crate::stdb::Coordinator::unequip_item(self, account_id, actor_guid, from_slot)
    }

    fn move_item(
        &self,
        account_id: u64,
        actor_guid: u64,
        from_slot: u8,
        to_slot: u8,
    ) -> Result<()> {
        crate::stdb::Coordinator::move_item(self, account_id, actor_guid, from_slot, to_slot)
    }

    fn use_item(&self, account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        crate::stdb::Coordinator::use_item(self, account_id, actor_guid, slot)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum ItemActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemActionErrorClass {
    GameplayRefusal,
    Fatal,
}

fn classify_item_action_error(error: &anyhow::Error) -> ItemActionErrorClass {
    if error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
    {
        ItemActionErrorClass::Fatal
    } else {
        ItemActionErrorClass::GameplayRefusal
    }
}

fn inventory_action_outbound(
    account_id: u64,
    operation: &str,
    result: Result<()>,
) -> Result<Vec<Outbound>> {
    match result {
        Ok(()) => Ok(Vec::new()),
        Err(e) if classify_item_action_error(&e) == ItemActionErrorClass::GameplayRefusal => {
            log::debug!("world: {operation} rejected (account {account_id}): {e}");
            Ok(vec![Outbound::One(
                ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                    codec::build_inventory_change_failure(),
                )),
            )])
        }
        Err(e) => Err(e),
    }
}

/// Quest-starting items are the one item action the quest family owns, so the store must answer
/// both vocabularies. Every `WorldStore` already does.
pub(crate) fn dispatch_item_action<St: ItemActionStore + QuestActionStore + ?Sized>(
    store: &St,
    player: ItemActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<ItemActionOutcome> {
    match msg {
        ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(c) if c.source_bag == MAIN_BAG => {
            let outbound = inventory_action_outbound(
                player.account_id,
                "equip_item",
                store.equip_item(
                    player.account_id,
                    player.self_guid.unwrap_or(0),
                    c.source_slot,
                ),
            )?;
            Ok(ItemActionOutcome::Handled { outbound })
        }
        ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(c) => {
            log::debug!(
                "world: autoequip from sub-bag {} unsupported (account {})",
                c.source_bag,
                player.account_id
            );
            Ok(ItemActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c)
            if c.source_bag == MAIN_BAG
                && c.destination_bag == MAIN_BAG
                && c.source_slot <= EQUIP_SLOT_END =>
        {
            let outbound = inventory_action_outbound(
                player.account_id,
                "unequip_item",
                store.unequip_item(
                    player.account_id,
                    player.self_guid.unwrap_or(0),
                    c.source_slot,
                ),
            )?;
            Ok(ItemActionOutcome::Handled { outbound })
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c) => {
            log::debug!(
                "world: autostore (bag {} slot {}) unsupported (account {})",
                c.source_bag,
                c.source_slot,
                player.account_id
            );
            Ok(ItemActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(c) => {
            let outbound = inventory_action_outbound(
                player.account_id,
                "move_item",
                store.move_item(
                    player.account_id,
                    player.self_guid.unwrap_or(0),
                    c.source_slot.as_int(),
                    c.destination_slot.as_int(),
                ),
            )?;
            Ok(ItemActionOutcome::Handled { outbound })
        }
        ClientOpcodeMessage::CMSG_SWAP_ITEM(c)
            if c.source_bag == MAIN_BAG && c.destination_bag == MAIN_BAG =>
        {
            let outbound = inventory_action_outbound(
                player.account_id,
                "move_item (swap)",
                store.move_item(
                    player.account_id,
                    player.self_guid.unwrap_or(0),
                    c.source_slot,
                    c.destionation_slot,
                ),
            )?;
            Ok(ItemActionOutcome::Handled { outbound })
        }
        ClientOpcodeMessage::CMSG_SWAP_ITEM(_) => {
            log::debug!(
                "world: cross-bag swap unsupported (account {})",
                player.account_id
            );
            Ok(ItemActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        // Using an item that starts a quest opens that quest instead of consuming the item, so the
        // quest family gets first refusal on the slot.
        ClientOpcodeMessage::CMSG_USE_ITEM(c) if c.bag_index == MAIN_BAG => {
            let quest_player = QuestActionPlayer {
                account_id: player.account_id,
                self_guid: player.self_guid,
            };
            let outbound = match item_started_quest(store, quest_player, c.bag_slot)? {
                Some(details) => details,
                None => inventory_action_outbound(
                    player.account_id,
                    "use_item",
                    store.use_item(player.account_id, player.self_guid.unwrap_or(0), c.bag_slot),
                )?,
            };
            Ok(ItemActionOutcome::Handled { outbound })
        }
        ClientOpcodeMessage::CMSG_USE_ITEM(c) => {
            log::debug!(
                "world: use_item from sub-bag {} unsupported (account {})",
                c.bag_index,
                player.account_id
            );
            Ok(ItemActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        other => Ok(ItemActionOutcome::PassThrough(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        ItemSlot, CMSG_AUTOEQUIP_ITEM, CMSG_AUTOSTORE_BAG_ITEM, CMSG_PING, CMSG_SWAP_INV_ITEM,
        CMSG_SWAP_ITEM, CMSG_USE_ITEM,
    };

    #[derive(Default)]
    struct InMemoryItemActions {
        equip_requests: Mutex<Vec<(u64, u64, u8)>>,
        unequip_requests: Mutex<Vec<(u64, u64, u8)>>,
        move_requests: Mutex<Vec<(u64, u64, u8, u8)>>,
        use_requests: Mutex<Vec<(u64, u64, u8)>>,
        equip_error: Option<String>,
        unequip_error: Option<String>,
        move_error: Option<String>,
        use_error: Option<String>,
        start_quest: Option<(u64, u32)>,
        quest_detail: Option<codec::QuestDetailView>,
    }

    impl ItemActionStore for InMemoryItemActions {
        fn equip_item(&self, account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()> {
            self.equip_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, from_slot));
            self.equip_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn unequip_item(&self, account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()> {
            self.unequip_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, from_slot));
            self.unequip_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn move_item(
            &self,
            account_id: u64,
            actor_guid: u64,
            from_slot: u8,
            to_slot: u8,
        ) -> Result<()> {
            self.move_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, from_slot, to_slot));
            self.move_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn use_item(&self, account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
            self.use_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, slot));
            self.use_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }
    }

    /// The item seam reaches the quest family for one thing only: whether the used item starts a
    /// quest. The rest of the quest vocabulary is out of reach from here, and saying so keeps a
    /// future route from quietly reading a canned answer.
    impl QuestActionStore for InMemoryItemActions {
        fn giver_quest_evals(
            &self,
            _giver_guid: u64,
            _player_guid: u64,
        ) -> Result<Vec<codec::GiverQuestEval>> {
            unreachable!("no item action opens a quest giver's menu")
        }

        fn quest_detail_view(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
            Ok(self
                .quest_detail
                .as_ref()
                .filter(|detail| detail.quest_id == quest_id)
                .cloned())
        }

        fn giver_refuses_interaction(&self, _giver_guid: u64, _player_guid: u64) -> Result<bool> {
            unreachable!("no item action runs the giver interaction gate")
        }

        fn accept_quest(
            &self,
            _account_id: u64,
            _self_guid: u64,
            _giver_guid: u64,
            _quest_id: u32,
        ) -> Result<()> {
            unreachable!("no item action accepts a quest")
        }

        fn item_start_quest(&self, _owner_guid: u64, _slot: u8) -> Option<(u64, u32)> {
            self.start_quest
        }

        fn player_quest_log(
            &self,
            _player_guid: u64,
        ) -> Result<Vec<codec::update_mask::QuestLogSlot>> {
            unreachable!("no item action reads the quest log")
        }

        fn abandon_quest(&self, _account_id: u64, _self_guid: u64, _quest_id: u32) -> Result<()> {
            unreachable!("no item action abandons a quest")
        }
    }

    fn player() -> ItemActionPlayer {
        ItemActionPlayer {
            account_id: 7,
            self_guid: Some(42),
        }
    }

    fn equip(source_bag: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(CMSG_AUTOEQUIP_ITEM {
            source_bag,
            source_slot: 24,
        })
    }

    fn unequip(source_slot: u8, destination_bag: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(CMSG_AUTOSTORE_BAG_ITEM {
            source_bag: MAIN_BAG,
            source_slot,
            destination_bag,
        })
    }

    fn swap(source_bag: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_SWAP_ITEM(CMSG_SWAP_ITEM {
            source_bag,
            source_slot: 23,
            destination_bag: MAIN_BAG,
            destionation_slot: 30,
        })
    }

    fn use_item(bag_index: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_USE_ITEM(Box::new(CMSG_USE_ITEM {
            bag_index,
            bag_slot: 5,
            ..Default::default()
        }))
    }

    fn quest_detail(quest_id: u32) -> codec::QuestDetailView {
        codec::QuestDetailView {
            quest_id,
            quest_level: 1,
            zone_or_sort: 12,
            title: "A Threat Within".into(),
            details: String::new(),
            objectives_text: String::new(),
            offer_reward_text: String::new(),
            request_items_text: String::new(),
            money_reward: 0,
            reward_xp: 0,
            next_quest_id: 0,
            max_level_money_reward: 0,
            rewards: Vec::new(),
            choice_rewards: Vec::new(),
            objectives: Vec::new(),
        }
    }

    fn handled_without_outbound(outcome: ItemActionOutcome) {
        assert!(matches!(outcome, ItemActionOutcome::Handled { outbound } if outbound.is_empty()));
    }

    fn inventory_failure(outcome: ItemActionOutcome) {
        assert!(matches!(
            outcome,
            ItemActionOutcome::Handled { outbound }
                if matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_))])
        ));
    }

    fn assert_no_durable_requests(actions: &InMemoryItemActions) {
        assert!(actions.equip_requests.lock().unwrap().is_empty());
        assert!(actions.unequip_requests.lock().unwrap().is_empty());
        assert!(actions.move_requests.lock().unwrap().is_empty());
        assert!(actions.use_requests.lock().unwrap().is_empty());
    }

    // ── Equip ────────────────────────────────────────────────────────────────

    #[test]
    fn equip_from_the_main_bag_requests_the_durable_equip_for_the_actor_and_slot() {
        let actions = InMemoryItemActions::default();

        let outcome = dispatch_item_action(&actions, player(), equip(MAIN_BAG)).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.equip_requests.lock().unwrap().as_slice(),
            &[(7, 42, 24)]
        );
    }

    #[test]
    fn equip_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            equip_error: Some("required level not met".into()),
            ..Default::default()
        };

        inventory_failure(dispatch_item_action(&actions, player(), equip(MAIN_BAG)).unwrap());
    }

    #[test]
    fn equip_from_a_sub_bag_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(dispatch_item_action(&actions, player(), equip(19)).unwrap());

        assert_no_durable_requests(&actions);
    }

    // ── Unequip ──────────────────────────────────────────────────────────────

    #[test]
    fn unequip_of_an_equipped_slot_requests_the_durable_unequip() {
        let actions = InMemoryItemActions::default();

        let outcome = dispatch_item_action(&actions, player(), unequip(15, MAIN_BAG)).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.unequip_requests.lock().unwrap().as_slice(),
            &[(7, 42, 15)]
        );
    }

    #[test]
    fn unequip_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            unequip_error: Some("backpack full".into()),
            ..Default::default()
        };

        inventory_failure(dispatch_item_action(&actions, player(), unequip(15, MAIN_BAG)).unwrap());
    }

    #[test]
    fn unequip_from_a_backpack_slot_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(
            dispatch_item_action(&actions, player(), unequip(24, MAIN_BAG)).unwrap(),
        );

        assert_no_durable_requests(&actions);
    }

    #[test]
    fn unequip_into_a_sub_bag_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(
            dispatch_item_action(&actions, player(), unequip(15, 19)).unwrap(),
        );

        assert_no_durable_requests(&actions);
    }

    #[test]
    fn unequip_from_a_sub_bag_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(
            dispatch_item_action(
                &actions,
                player(),
                ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(CMSG_AUTOSTORE_BAG_ITEM {
                    source_bag: 19,
                    source_slot: 0,
                    destination_bag: MAIN_BAG,
                }),
            )
            .unwrap(),
        );

        assert_no_durable_requests(&actions);
    }

    // ── Same-container move ──────────────────────────────────────────────────

    #[test]
    fn move_maps_the_wire_slots_to_the_durable_move() {
        let actions = InMemoryItemActions::default();

        let outcome = dispatch_item_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(CMSG_SWAP_INV_ITEM {
                source_slot: ItemSlot::MainHand,
                destination_slot: ItemSlot::Inventory1,
            }),
        )
        .unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.move_requests.lock().unwrap().as_slice(),
            &[(
                7,
                42,
                ItemSlot::MainHand.as_int(),
                ItemSlot::Inventory1.as_int()
            )]
        );
    }

    #[test]
    fn move_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            move_error: Some("cannot equip that there".into()),
            ..Default::default()
        };

        inventory_failure(
            dispatch_item_action(
                &actions,
                player(),
                ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(CMSG_SWAP_INV_ITEM {
                    source_slot: ItemSlot::Inventory0,
                    destination_slot: ItemSlot::MainHand,
                }),
            )
            .unwrap(),
        );
    }

    // ── Same-container swap ──────────────────────────────────────────────────

    #[test]
    fn swap_within_the_main_bag_maps_both_slots_to_the_durable_move() {
        let actions = InMemoryItemActions::default();

        let outcome = dispatch_item_action(&actions, player(), swap(MAIN_BAG)).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.move_requests.lock().unwrap().as_slice(),
            &[(7, 42, 23, 30)]
        );
    }

    #[test]
    fn swap_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            move_error: Some("cannot equip that there".into()),
            ..Default::default()
        };

        inventory_failure(dispatch_item_action(&actions, player(), swap(MAIN_BAG)).unwrap());
    }

    #[test]
    fn cross_container_swap_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(dispatch_item_action(&actions, player(), swap(19)).unwrap());

        assert_no_durable_requests(&actions);
    }

    // ── Ordinary use ─────────────────────────────────────────────────────────

    #[test]
    fn use_from_the_main_bag_requests_the_durable_use_for_the_actor_and_slot() {
        let actions = InMemoryItemActions::default();

        let outcome = dispatch_item_action(&actions, player(), use_item(MAIN_BAG)).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.use_requests.lock().unwrap().as_slice(),
            &[(7, 42, 5)]
        );
    }

    #[test]
    fn use_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            use_error: Some("item is not usable".into()),
            ..Default::default()
        };

        inventory_failure(dispatch_item_action(&actions, player(), use_item(MAIN_BAG)).unwrap());
    }

    #[test]
    fn use_from_a_sub_bag_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(dispatch_item_action(&actions, player(), use_item(19)).unwrap());

        assert_no_durable_requests(&actions);
    }

    // ── Item-starts-quest ────────────────────────────────────────────────────

    #[test]
    fn quest_start_use_returns_details_without_requesting_ordinary_use() {
        let actions = InMemoryItemActions {
            start_quest: Some((0x4000_0000_0000_0099, 1234)),
            quest_detail: Some(quest_detail(1234)),
            ..Default::default()
        };

        let outcome = dispatch_item_action(&actions, player(), use_item(MAIN_BAG)).unwrap();

        assert!(matches!(
            outcome,
            ItemActionOutcome::Handled { outbound }
                if matches!(outbound.as_slice(), [Outbound::Raw { opcode: 0x0188, body }]
                    if body[..8] == 0x4000_0000_0000_0099u64.to_le_bytes()
                        && body[8..12] == 1234u32.to_le_bytes())
        ));
        assert!(actions.use_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn unavailable_quest_details_do_not_consume_the_start_quest_item() {
        let actions = InMemoryItemActions {
            start_quest: Some((99, 1234)),
            ..Default::default()
        };

        let outcome = dispatch_item_action(&actions, player(), use_item(MAIN_BAG)).unwrap();

        handled_without_outbound(outcome);
        assert!(actions.use_requests.lock().unwrap().is_empty());
    }

    // ── Player context and error classification ──────────────────────────────

    #[test]
    fn unresolved_player_uses_the_legacy_zero_actor_for_every_durable_inventory_action() {
        let actions = InMemoryItemActions::default();
        let player = ItemActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        for msg in [
            equip(MAIN_BAG),
            unequip(15, MAIN_BAG),
            ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(CMSG_SWAP_INV_ITEM {
                source_slot: ItemSlot::MainHand,
                destination_slot: ItemSlot::Inventory1,
            }),
            swap(MAIN_BAG),
            use_item(MAIN_BAG),
        ] {
            dispatch_item_action(&actions, player, msg).unwrap();
        }

        assert_eq!(
            actions.equip_requests.lock().unwrap().as_slice(),
            &[(7, 0, 24)]
        );
        assert_eq!(
            actions.unequip_requests.lock().unwrap().as_slice(),
            &[(7, 0, 15)]
        );
        assert_eq!(
            actions.move_requests.lock().unwrap().as_slice(),
            &[
                (
                    7,
                    0,
                    ItemSlot::MainHand.as_int(),
                    ItemSlot::Inventory1.as_int()
                ),
                (7, 0, 23, 30)
            ]
        );
        assert_eq!(
            actions.use_requests.lock().unwrap().as_slice(),
            &[(7, 0, 5)]
        );
    }

    #[test]
    fn unresolved_player_skips_quest_start_routing_and_requests_ordinary_use() {
        let actions = InMemoryItemActions {
            start_quest: Some((0x4000_0000_0000_0099, 1234)),
            quest_detail: Some(quest_detail(1234)),
            ..Default::default()
        };
        let player = ItemActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        let outcome = dispatch_item_action(&actions, player, use_item(MAIN_BAG)).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.use_requests.lock().unwrap().as_slice(),
            &[(7, 0, 5)]
        );
    }

    #[test]
    fn reducer_transport_failure_is_session_fatal() {
        let actions = InMemoryItemActions {
            equip_error: Some("equip_item reducer transport disconnected: channel closed".into()),
            ..Default::default()
        };

        let error = match dispatch_item_action(&actions, player(), equip(MAIN_BAG)) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    // ── Pass-through ─────────────────────────────────────────────────────────

    #[test]
    fn unrelated_opcodes_pass_through_to_the_next_dispatcher() {
        let actions = InMemoryItemActions::default();

        let outcome = dispatch_item_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            ItemActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }
}
