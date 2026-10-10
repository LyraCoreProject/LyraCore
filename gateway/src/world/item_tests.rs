//! Item actions through their dispatcher.

use super::handlers::InMemoryItemActions;
use super::*;

const PLAYER: ItemActionPlayer = ItemActionPlayer {
    account_id: 7,
    self_guid: Some(1),
};

/// Dispatch one item message and return the packets the session would send for it.
fn try_run(
    actions: &InMemoryItemActions,
    player: ItemActionPlayer,
    msg: impl Into<ClientOpcodeMessage>,
) -> Result<Vec<Outbound>> {
    match dispatch_item_action(actions, player, msg.into())? {
        ItemActionOutcome::Handled { outbound } => Ok(outbound),
        ItemActionOutcome::PassThrough(_) => panic!("the item dispatcher passed the message on"),
    }
}

/// Slot 24 is a backpack slot (>= 23); bag 255 = INVENTORY_SLOT_BAG_0 (main bag).
fn autoequip(source_slot: u8) -> CMSG_AUTOEQUIP_ITEM {
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot,
    }
}

#[test]
fn equip_item_err_sends_smsg_inventory_change_failure() {
    // A gameplay refusal reaches the client as an SMSG_INVENTORY_CHANGE_FAILURE and the next
    // action is served normally.
    let actions = InMemoryItemActions {
        equip_result: Some(Ok(ItemActionResult::Refused(ItemRefusal::CannotEquip))),
        ..Default::default()
    };
    for slot in [24, 25] {
        let sent = try_run(&actions, PLAYER, autoequip(slot)).unwrap();
        assert!(
            matches!(
                sent.as_slice(),
                [Outbound::One(
                    ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_)
                )]
            ),
            "slot {slot}: expected one SMSG_INVENTORY_CHANGE_FAILURE"
        );
    }
}

#[test]
fn item_action_before_player_login_is_handled_without_panicking() {
    // An item frame arriving after the handshake but before CMSG_PLAYER_LOGIN (no selected
    // player) must not panic or error the session: the legacy zero-actor fallback stays a
    // handled gameplay context.
    let actions = InMemoryItemActions::default();
    let player = ItemActionPlayer {
        self_guid: None,
        ..PLAYER
    };
    let sent = try_run(&actions, player, autoequip(24))
        .expect("the legacy zero-actor fallback remains a handled gameplay context");
    assert!(sent.is_empty());
}

#[test]
fn item_reducer_transport_loss_ends_the_world_session() {
    // Reducer transport loss comes back as an error, which ends the session, instead of being
    // translated into gameplay feedback.
    let actions = InMemoryItemActions {
        equip_result: Some(Err(
            "equip_item reducer transport disconnected: channel closed".into(),
        )),
        ..Default::default()
    };
    let error = try_run(&actions, PLAYER, autoequip(24))
        .err()
        .expect("a disconnected item reducer transport must be session-fatal");
    assert!(format!("{error:#}").contains("reducer transport disconnected"));
}
