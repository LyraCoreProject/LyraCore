//! Targeting and sheathing through the Combat Store.

use super::handlers::{Combat, CombatStore};
use super::*;
use crate::stdb::ReducerCallError;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// What each Character has selected and how it carries its weapons.
#[derive(Default)]
struct CombatFake {
    selected: Mutex<BTreeMap<u64, u64>>,
    sheath: Mutex<BTreeMap<u64, u8>>,
    /// Answered once by the next request, in place of success.
    failure: Mutex<Option<ReducerCallError>>,
}

impl CombatFake {
    fn target_of(&self, character: u64) -> Option<u64> {
        self.selected.lock().unwrap().get(&character).copied()
    }

    fn sheath_of(&self, character: u64) -> Option<u8> {
        self.sheath.lock().unwrap().get(&character).copied()
    }

    fn failing(failure: ReducerCallError) -> Self {
        Self {
            failure: Mutex::new(Some(failure)),
            ..Self::default()
        }
    }

    fn answer(&self) -> Result<()> {
        match self.failure.lock().unwrap().take() {
            Some(failure) => Err(failure.into()),
            None => Ok(()),
        }
    }
}

impl CombatStore for CombatFake {
    fn set_target(&self, actor: Actor, target_guid: u64) -> Result<()> {
        self.answer()?;
        self.selected
            .lock()
            .unwrap()
            .insert(actor.guid(), target_guid);
        Ok(())
    }

    fn pet_command(&self, _actor: Actor, _data: u32, _target_guid: u64) -> Result<()> {
        self.answer()
    }

    fn set_sheathed(&self, actor: Actor, state: u8) -> Result<()> {
        self.answer()?;
        self.sheath.lock().unwrap().insert(actor.guid(), state);
        Ok(())
    }
}

/// Handle `msg` as Character 1 of Account 7.
fn run(store: &CombatFake, msg: impl Into<ClientOpcodeMessage>) {
    let mut session = ProtocolSession::in_world(7, 1);
    let reply = Combat::handle(store, &mut session, ProtocolRequest::Message(msg.into())).unwrap();
    assert!(reply.outbound.is_empty());
}

#[test]
fn set_sheathed_routes_the_clients_z_press_to_the_store() {
    for (sent, expect) in [
        (SheathState::Unarmed, 0u8),
        (SheathState::Melee, 1),
        (SheathState::Ranged, 2),
    ] {
        let store = CombatFake::default();
        run(&store, CMSG_SETSHEATHED { sheathed: sent });
        assert_eq!(
            store.sheath_of(1),
            Some(expect),
            "{sent:?} must reach the Store as byte {expect}"
        );
    }
}

#[test]
fn set_selection_records_the_wire_guid_as_the_target() {
    let store = CombatFake::default();
    run(
        &store,
        CMSG_SET_SELECTION {
            target: Guid::new(321),
        },
    );
    assert_eq!(store.target_of(1), Some(321));
}

fn sheathe() -> CMSG_SETSHEATHED {
    CMSG_SETSHEATHED {
        sheathed: SheathState::Melee,
    }
}

#[test]
fn transport_loss_ends_the_world_session() {
    let store = CombatFake::failing(ReducerCallError::transport_lost("gw_set_sheathed"));
    let mut session = ProtocolSession::in_world(7, 1);
    assert!(Combat::handle(
        &store,
        &mut session,
        ProtocolRequest::Message(sheathe().into())
    )
    .is_err());
}

#[test]
fn refusal_is_ignored_and_the_world_session_continues() {
    let store = CombatFake::failing(ReducerCallError::refused("gw_set_sheathed", "not in world"));
    let mut session = ProtocolSession::in_world(7, 1);
    let reply = Combat::handle(
        &store,
        &mut session,
        ProtocolRequest::Message(sheathe().into()),
    )
    .unwrap();
    assert!(reply.outbound.is_empty());
    assert_eq!(store.sheath_of(1), None);
}
