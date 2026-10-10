//! Targeting and sheathing, run through `handle_combat` against a Fake that holds only the Combat
//! Store the handler is bounded on.

use super::handlers::{handle_combat, CombatStore};
use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// What each Character has selected and how it carries its weapons.
#[derive(Default)]
struct CombatFake {
    selected: Mutex<BTreeMap<u64, u64>>,
    sheath: Mutex<BTreeMap<u64, u8>>,
}

impl CombatFake {
    fn target_of(&self, character: u64) -> Option<u64> {
        self.selected.lock().unwrap().get(&character).copied()
    }

    fn sheath_of(&self, character: u64) -> Option<u8> {
        self.sheath.lock().unwrap().get(&character).copied()
    }
}

impl CombatStore for CombatFake {
    fn set_target(&self, _account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.selected.lock().unwrap().insert(self_guid, target_guid);
        Ok(())
    }

    fn pet_command(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _data: u32,
        _target_guid: u64,
    ) -> Result<()> {
        unimplemented!("pet_command")
    }

    fn set_sheathed(&self, _account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.sheath.lock().unwrap().insert(self_guid, state);
        Ok(())
    }
}

/// Send `msg` as Character 1 of account 7. Phase 5 retargets only this helper.
fn run(store: &CombatFake, msg: impl Into<ClientOpcodeMessage>) {
    let mut conn = in_world_conn(7, 1);
    let passed_on = handle_combat(store, &mut conn, msg.into()).unwrap();
    assert!(passed_on.is_none(), "the combat family owns this opcode");
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
