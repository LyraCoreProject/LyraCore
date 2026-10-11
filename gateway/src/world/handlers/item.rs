//! Item-action dispatcher: protocol mapping and feedback for equip, use, and inventory operations.

use super::super::*;
use super::quest::{item_started_quest, QuestActionStore};
use lyracore_shared::item::ItemRefusal;

const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0 — backpack + equipped slots share this pseudo-bag
const EQUIP_SLOT_END: u8 = 18; // EQUIPMENT_SLOT_END — last equipment slot (main-hand=15, off=16…)

/// How the Module answered one item Durable Request. A Refusal is an outcome; a timeout, transport,
/// or SDK failure stays an `Err` with an unknown durable result and ends the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemActionResult {
    Done,
    Refused(ItemRefusal),
}

impl From<ItemRefusal> for ItemActionResult {
    fn from(refusal: ItemRefusal) -> Self {
        Self::Refused(refusal)
    }
}

pub(crate) trait ItemActionStore: Send + Sync {
    fn destroy_item(&self, actor: Actor, slot: u8, count: u32) -> Result<ItemActionResult>;
    fn equip_item(&self, actor: Actor, from_slot: u8) -> Result<ItemActionResult>;
    fn unequip_item(&self, actor: Actor, from_slot: u8) -> Result<ItemActionResult>;
    fn move_item(&self, actor: Actor, from_slot: u8, to_slot: u8) -> Result<ItemActionResult>;
    fn use_item(&self, actor: Actor, slot: u8) -> Result<ItemActionResult>;
    fn split_item(
        &self,
        actor: Actor,
        from_slot: u8,
        to_slot: u8,
        count: u32,
    ) -> Result<ItemActionResult>;
}

/// Run one durable item request as the session's Actor and render its answer. A session without
/// a Character in the world has nothing to act as, so the client gets an internal-error Refusal.
fn act(
    player: &ProtocolSession,
    operation: &str,
    request: impl FnOnce(Actor) -> Result<ItemActionResult>,
) -> Result<ProtocolReply> {
    let result = match player.self_guid().and_then(Actor::new) {
        Some(actor) => request(actor)?,
        None => {
            log::warn!(
                "world: {operation} has no resolved actor (account {})",
                player.account_id
            );
            ItemRefusal::Internal.into()
        }
    };
    Ok(ProtocolReply::from(inventory_action_outbound(
        player.account_id,
        operation,
        result,
    )))
}

/// A request the Gateway refuses before any Durable Request, such as a bag position it cannot map.
fn refuse(player: &ProtocolSession, operation: &str, refusal: ItemRefusal) -> ProtocolReply {
    ProtocolReply::from(inventory_action_outbound(
        player.account_id,
        operation,
        refusal.into(),
    ))
}

/// A Refusal answers the client with its own result code; a completed action answers nothing.
fn inventory_action_outbound(
    account_id: u64,
    operation: &str,
    result: ItemActionResult,
) -> Vec<Outbound> {
    match result {
        ItemActionResult::Done => Vec::new(),
        ItemActionResult::Refused(refusal) => {
            log::debug!(
                "world: {operation} refused (account {account_id}): {}",
                refusal.as_tag()
            );
            vec![Outbound::One(
                ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
                    codec::build_inventory_refusal(refusal),
                )),
            )]
        }
    }
}

/// A quest-starting item needs the quest operation as well as inventory operations.
pub(crate) struct Item;

impl<St: ItemActionStore + QuestActionStore + ?Sized> ProtocolFamily<St> for Item {
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let player = &*session;
        let msg = request.message()?;
        match msg {
            ClientOpcodeMessage::CMSG_DESTROYITEM(c) => {
                match codec::inventory_slot(c.bag, c.slot) {
                    Some(slot) => act(player, "destroy_item", |actor| {
                        store.destroy_item(actor, slot, u32::from(c.amount))
                    }),
                    None => Ok(refuse(player, "destroy_item", ItemRefusal::WrongSlot)),
                }
            }
            ClientOpcodeMessage::CMSG_SPLIT_ITEM(c) => match (
                codec::inventory_slot(c.source_bag, c.source_slot),
                codec::inventory_slot(c.destination_bag, c.destination_slot),
            ) {
                (Some(source), Some(destination)) => act(player, "split_item", |actor| {
                    store.split_item(actor, source, destination, u32::from(c.amount))
                }),
                _ => Ok(refuse(player, "split_item", ItemRefusal::WrongSlot)),
            },
            ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(c) => {
                match codec::inventory_slot(c.source_bag, c.source_slot) {
                    Some(slot) => act(player, "equip_item", |actor| store.equip_item(actor, slot)),
                    None => Ok(refuse(player, "equip_item", ItemRefusal::WrongSlot)),
                }
            }
            ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c)
                if c.source_bag == MAIN_BAG
                    && c.destination_bag == MAIN_BAG
                    && c.source_slot <= EQUIP_SLOT_END =>
            {
                act(player, "unequip_item", |actor| {
                    store.unequip_item(actor, c.source_slot)
                })
            }
            ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(_) => {
                Ok(refuse(player, "autostore", ItemRefusal::NotRightNow))
            }
            ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(c) => act(player, "move_item", |actor| {
                store.move_item(actor, c.source_slot.as_int(), c.destination_slot.as_int())
            }),
            ClientOpcodeMessage::CMSG_SWAP_ITEM(c) => match (
                codec::inventory_slot(c.source_bag, c.source_slot),
                codec::inventory_slot(c.destination_bag, c.destionation_slot),
            ) {
                (Some(source), Some(destination)) => act(player, "move_item", |actor| {
                    store.move_item(actor, source, destination)
                }),
                _ => Ok(refuse(player, "move_item", ItemRefusal::WrongSlot)),
            },
            // A quest-starting item opens quest details without consuming the item.
            ClientOpcodeMessage::CMSG_USE_ITEM(c) => {
                let Some(slot) = codec::inventory_slot(c.bag_index, c.bag_slot) else {
                    return Ok(refuse(player, "use_item", ItemRefusal::WrongSlot));
                };
                match item_started_quest(store, player, slot)? {
                    Some(outbound) => Ok(ProtocolReply::from(outbound)),
                    None => act(player, "use_item", |actor| store.use_item(actor, slot)),
                }
            }
            other => Err(anyhow!("opcode routed to wrong Protocol Family: {other}")),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::stdb::{classify, DurableFailure, ReducerCallError};
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        ItemSlot, CMSG_AUTOEQUIP_ITEM, CMSG_AUTOSTORE_BAG_ITEM, CMSG_SWAP_INV_ITEM, CMSG_SWAP_ITEM,
        CMSG_USE_ITEM,
    };

    #[derive(Default)]
    pub(crate) struct InMemoryItemActions {
        pub(crate) equip_requests: Mutex<Vec<(u64, u8)>>,
        pub(crate) unequip_requests: Mutex<Vec<(u64, u8)>>,
        pub(crate) move_requests: Mutex<Vec<(u64, u8, u8)>>,
        pub(crate) use_requests: Mutex<Vec<(u64, u8)>>,
        pub(crate) equip_result: Option<ItemActionResult>,
        pub(crate) unequip_result: Option<ItemActionResult>,
        pub(crate) move_result: Option<ItemActionResult>,
        pub(crate) use_result: Option<ItemActionResult>,
        /// Every durable call fails with a Transport Loss.
        pub(crate) transport_lost: bool,
        pub(crate) start_quest: Option<(u64, u32)>,
        pub(crate) quest_detail: Option<codec::QuestDetailView>,
    }

    impl InMemoryItemActions {
        /// The Coordinator answers a decoded Refusal or, for a Transport Loss, an error. The Fake
        /// answers in those two shapes.
        fn answer(
            &self,
            operation: &str,
            canned: Option<ItemActionResult>,
        ) -> Result<ItemActionResult> {
            if self.transport_lost {
                return Err(ReducerCallError::transport_lost(operation).into());
            }
            Ok(canned.unwrap_or(ItemActionResult::Done))
        }
    }

    impl ItemActionStore for InMemoryItemActions {
        fn destroy_item(&self, _actor: Actor, _slot: u8, _count: u32) -> Result<ItemActionResult> {
            Ok(ItemRefusal::ItemNotFound.into())
        }
        fn split_item(
            &self,
            _actor: Actor,
            _from_slot: u8,
            _to_slot: u8,
            _count: u32,
        ) -> Result<ItemActionResult> {
            Ok(ItemRefusal::ItemNotFound.into())
        }
        fn equip_item(&self, actor: Actor, from_slot: u8) -> Result<ItemActionResult> {
            self.equip_requests
                .lock()
                .unwrap()
                .push((actor.guid(), from_slot));
            self.answer("gw_equip_item", self.equip_result)
        }

        fn unequip_item(&self, actor: Actor, from_slot: u8) -> Result<ItemActionResult> {
            self.unequip_requests
                .lock()
                .unwrap()
                .push((actor.guid(), from_slot));
            self.answer("gw_unequip_item", self.unequip_result)
        }

        fn move_item(&self, actor: Actor, from_slot: u8, to_slot: u8) -> Result<ItemActionResult> {
            self.move_requests
                .lock()
                .unwrap()
                .push((actor.guid(), from_slot, to_slot));
            self.answer("gw_move_item", self.move_result)
        }

        fn use_item(&self, actor: Actor, slot: u8) -> Result<ItemActionResult> {
            self.use_requests.lock().unwrap().push((actor.guid(), slot));
            self.answer("gw_use_item", self.use_result)
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

        fn turn_in_quest(
            &self,
            _actor: Actor,
            _giver_guid: u64,
            _quest_id: u32,
            _reward_index: u32,
        ) -> Result<()> {
            unreachable!("no item action turns a quest in")
        }

        fn accept_quest(&self, _actor: Actor, _giver_guid: u64, _quest_id: u32) -> Result<()> {
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

        fn abandon_quest(&self, _actor: Actor, _quest_id: u32) -> Result<()> {
            unreachable!("no item action abandons a quest")
        }

        fn push_quest(&self, _actor: Actor, _quest_id: u32) -> Result<()> {
            unreachable!("no item action shares a quest with the party")
        }

        fn quest_status(&self, _player_guid: u64, _quest_id: u32) -> (bool, bool) {
            unreachable!("no item action reads gossip quest gate state")
        }
    }

    fn player() -> ProtocolSession {
        ProtocolSession::in_world(7, 42)
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

    fn handled_without_outbound(outcome: ProtocolReply) {
        assert!(outcome.outbound.is_empty());
    }

    fn inventory_failure(outcome: ProtocolReply) {
        assert!(matches!(
            outcome,
            ProtocolReply { outbound, .. }
                if matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_))])
        ));
    }

    fn handled_outbound(outcome: ProtocolReply) -> Vec<Outbound> {
        outcome.outbound
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

        let outcome = Item::handle(&actions, &mut player(), equip(MAIN_BAG).into()).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.equip_requests.lock().unwrap().as_slice(),
            &[(42, 24)]
        );
    }

    #[test]
    fn equip_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            equip_result: Some(ItemRefusal::CannotEquip.into()),
            ..Default::default()
        };

        inventory_failure(Item::handle(&actions, &mut player(), equip(MAIN_BAG).into()).unwrap());
    }

    #[test]
    fn equip_from_an_invalid_bag_position_returns_a_refusal() {
        let actions = InMemoryItemActions::default();

        inventory_failure(Item::handle(&actions, &mut player(), equip(19).into()).unwrap());

        assert_no_durable_requests(&actions);
    }

    // ── Unequip ──────────────────────────────────────────────────────────────

    #[test]
    fn unequip_of_an_equipped_slot_requests_the_durable_unequip() {
        let actions = InMemoryItemActions::default();

        let outcome = Item::handle(&actions, &mut player(), unequip(15, MAIN_BAG).into()).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.unequip_requests.lock().unwrap().as_slice(),
            &[(42, 15)]
        );
    }

    #[test]
    fn unequip_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            unequip_result: Some(ItemRefusal::InventoryFull.into()),
            ..Default::default()
        };

        inventory_failure(
            Item::handle(&actions, &mut player(), unequip(15, MAIN_BAG).into()).unwrap(),
        );
    }

    #[test]
    fn unequip_from_a_backpack_slot_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        inventory_failure(
            Item::handle(&actions, &mut player(), unequip(24, MAIN_BAG).into()).unwrap(),
        );

        assert_no_durable_requests(&actions);
    }

    #[test]
    fn unequip_into_a_sub_bag_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        inventory_failure(Item::handle(&actions, &mut player(), unequip(15, 19).into()).unwrap());

        assert_no_durable_requests(&actions);
    }

    #[test]
    fn unequip_from_a_sub_bag_requests_no_durable_operation() {
        let actions = InMemoryItemActions::default();

        inventory_failure(
            Item::handle(
                &actions,
                &mut player(),
                ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(CMSG_AUTOSTORE_BAG_ITEM {
                    source_bag: 19,
                    source_slot: 0,
                    destination_bag: MAIN_BAG,
                })
                .into(),
            )
            .unwrap(),
        );

        assert_no_durable_requests(&actions);
    }

    // ── Same-container move ──────────────────────────────────────────────────

    #[test]
    fn move_maps_the_wire_slots_to_the_durable_move() {
        let actions = InMemoryItemActions::default();

        let outcome = Item::handle(
            &actions,
            &mut player(),
            ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(CMSG_SWAP_INV_ITEM {
                source_slot: ItemSlot::MainHand,
                destination_slot: ItemSlot::Inventory1,
            })
            .into(),
        )
        .unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.move_requests.lock().unwrap().as_slice(),
            &[(
                42,
                ItemSlot::MainHand.as_int(),
                ItemSlot::Inventory1.as_int()
            )]
        );
    }

    #[test]
    fn move_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            move_result: Some(ItemRefusal::WrongSlot.into()),
            ..Default::default()
        };

        inventory_failure(
            Item::handle(
                &actions,
                &mut player(),
                ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(CMSG_SWAP_INV_ITEM {
                    source_slot: ItemSlot::Inventory0,
                    destination_slot: ItemSlot::MainHand,
                })
                .into(),
            )
            .unwrap(),
        );
    }

    // ── Same-container swap ──────────────────────────────────────────────────

    #[test]
    fn swap_within_the_main_bag_maps_both_slots_to_the_durable_move() {
        let actions = InMemoryItemActions::default();

        let outcome = Item::handle(&actions, &mut player(), swap(MAIN_BAG).into()).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(
            actions.move_requests.lock().unwrap().as_slice(),
            &[(42, 23, 30)]
        );
    }

    #[test]
    fn swap_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            move_result: Some(ItemRefusal::WrongSlot.into()),
            ..Default::default()
        };

        inventory_failure(Item::handle(&actions, &mut player(), swap(MAIN_BAG).into()).unwrap());
    }

    #[test]
    fn invalid_bag_position_returns_a_refusal_without_a_durable_request() {
        let actions = InMemoryItemActions::default();

        inventory_failure(Item::handle(&actions, &mut player(), swap(19).into()).unwrap());

        assert_no_durable_requests(&actions);
    }

    // ── Ordinary use ─────────────────────────────────────────────────────────

    #[test]
    fn use_from_the_main_bag_requests_the_durable_use_for_the_actor_and_slot() {
        let actions = InMemoryItemActions::default();

        let outcome = Item::handle(&actions, &mut player(), use_item(MAIN_BAG).into()).unwrap();

        handled_without_outbound(outcome);
        assert_eq!(actions.use_requests.lock().unwrap().as_slice(), &[(42, 5)]);
    }

    #[test]
    fn use_refusal_returns_inventory_failure_without_ending_the_session() {
        let actions = InMemoryItemActions {
            use_result: Some(ItemRefusal::ItemNotUsable.into()),
            ..Default::default()
        };

        inventory_failure(
            Item::handle(&actions, &mut player(), use_item(MAIN_BAG).into()).unwrap(),
        );
    }

    #[test]
    fn use_from_a_bag_resolves_its_slot_before_the_durable_request() {
        let actions = InMemoryItemActions::default();

        handled_without_outbound(
            Item::handle(&actions, &mut player(), use_item(19).into()).unwrap(),
        );

        assert_eq!(
            actions.use_requests.lock().unwrap().as_slice(),
            &[(42, 125)]
        );
    }

    // ── Item-starts-quest ────────────────────────────────────────────────────

    #[test]
    fn quest_start_use_returns_details_without_requesting_ordinary_use() {
        let actions = InMemoryItemActions {
            start_quest: Some((0x4000_0000_0000_0099, 1234)),
            quest_detail: Some(quest_detail(1234)),
            ..Default::default()
        };

        let outcome = Item::handle(&actions, &mut player(), use_item(MAIN_BAG).into()).unwrap();

        assert!(matches!(
            outcome,
            ProtocolReply { outbound, .. }
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

        let outcome = Item::handle(&actions, &mut player(), use_item(MAIN_BAG).into()).unwrap();

        handled_without_outbound(outcome);
        assert!(actions.use_requests.lock().unwrap().is_empty());
    }

    // ── Player context and error classification ──────────────────────────────

    #[test]
    fn unresolved_player_requests_nothing_and_every_inventory_action_answers_a_refusal() {
        let actions = InMemoryItemActions::default();
        let mut player = ProtocolSession::new(7, "TESTER".into());

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
            inventory_failure(Item::handle(&actions, &mut player, msg.into()).unwrap());
        }

        assert_no_durable_requests(&actions);
    }

    #[test]
    fn unresolved_player_skips_quest_start_routing_and_answers_a_refusal() {
        let actions = InMemoryItemActions {
            start_quest: Some((0x4000_0000_0000_0099, 1234)),
            quest_detail: Some(quest_detail(1234)),
            ..Default::default()
        };
        let mut player = ProtocolSession::new(7, "TESTER".into());

        let outcome = Item::handle(&actions, &mut player, use_item(MAIN_BAG).into()).unwrap();

        inventory_failure(outcome);
        assert_no_durable_requests(&actions);
    }

    #[test]
    fn every_module_refusal_has_one_client_result_code() {
        use wow_world_messages::vanilla::SMSG_INVENTORY_CHANGE_FAILURE as Failure;
        use wow_world_messages::Guid;
        let (item1, item2, bag_type_subclass) = (Guid::new(0), Guid::new(0), 0u8);
        // The vanilla 1.12 result each Refusal must reach the client as.
        let expected = |refusal| match refusal {
            ItemRefusal::Indestructible => Failure::CantDropSoulbound {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::BagNotEmpty => Failure::CanOnlyDoWithEmptyBags {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::ItemNotFound => Failure::ItemNotFound {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::InventoryFull => Failure::InventoryFull {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::WrongSlot => Failure::ItemDoesntGoToSlot {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::CannotEquip => Failure::ItemCantBeEquipped {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::NoProficiency => Failure::NoRequiredProficiency {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::RequiredSkill => Failure::CantEquipSkill {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::RequiredReputation => Failure::CantEquipReputation {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::PlayerDead => Failure::YouAreDead {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::BankUnavailable => Failure::TooFarAwayFromBank {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::ItemNotUsable => Failure::YouCanNeverUseThatItem {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::NotRightNow => Failure::CantDoRightNow {
                item1,
                item2,
                bag_type_subclass,
            },
            ItemRefusal::Internal => Failure::IntBagError {
                item1,
                item2,
                bag_type_subclass,
            },
        };
        for refusal in ItemRefusal::ALL {
            let actions = InMemoryItemActions {
                equip_result: Some(refusal.into()),
                ..Default::default()
            };

            let outbound = handled_outbound(
                Item::handle(&actions, &mut player(), equip(MAIN_BAG).into()).unwrap(),
            );

            assert!(
                matches!(
                    outbound.as_slice(),
                    [Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(failure))]
                        if **failure == expected(refusal)
                ),
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn a_transport_loss_ends_the_session_where_a_refusal_keeps_it_alive() {
        let lost = InMemoryItemActions {
            transport_lost: true,
            ..Default::default()
        };
        let error = match Item::handle(&lost, &mut player(), equip(MAIN_BAG).into()) {
            Err(error) => error,
            Ok(_) => panic!("a lost transport must end the session"),
        };
        assert!(matches!(classify(&error), DurableFailure::TransportLoss));

        let refusing = InMemoryItemActions {
            equip_result: Some(ItemRefusal::CannotEquip.into()),
            ..Default::default()
        };
        inventory_failure(Item::handle(&refusing, &mut player(), equip(MAIN_BAG).into()).unwrap());
    }

    // ── Pass-through ─────────────────────────────────────────────────────────
}
