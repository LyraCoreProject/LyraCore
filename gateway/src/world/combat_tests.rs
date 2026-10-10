//! Targeting and sheathing over an encrypted World Session.

use super::*;

#[test]
fn set_sheathed_routes_the_clients_z_press_to_the_store() {
    for (sent, expect) in [
        (SheathState::Unarmed, 0u8),
        (SheathState::Melee, 1),
        (SheathState::Ranged, 2),
    ] {
        let store = std::sync::Arc::new(quest_store());
        let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
        CMSG_SETSHEATHED { sheathed: sent }
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
        drop(client);
        server.join().unwrap();
        assert_eq!(
            store.combat.sheathed.lock().unwrap().as_slice(),
            &[(1, expect)],
            "{sent:?} must reach set_sheathed as byte {expect}"
        );
    }
}

#[test]
fn set_selection_dispatches_set_target_with_the_wire_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SET_SELECTION {
        target: Guid::new(321),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.combat.selected_targets.lock().unwrap().as_slice(),
        &[321]
    );
}
