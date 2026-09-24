//! Combat family leftovers: selection, pet commands, the run-speed ack and sheathing. Melee start
//! and stop belong to the melee seam in `melee.rs`; every cast opcode, the `CMSG_CAST_SPELL` routes
//! and both cancellations, belongs to the cast seam in `cast`.

use super::super::*;

/// Combat family leftovers: selection, pet commands, the run-speed ack and sheathing. Each arm is
/// best-effort. The session-fatal desync exits went to the melee seam with the two melee opcodes
/// that owned them.
pub(crate) fn handle_combat<St: WorldStore + ?Sized>(
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    // The shared-call path names the actor by guid; 0 (not in world) forces the
    // per-player path in the store.
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };

    match msg {
        // Targeting (N3): record the player's selection server-side (foundation for combat).
        ClientOpcodeMessage::CMSG_SET_SELECTION(s) => {
            store.set_target(conn.account_id, self_guid, s.target.guid())?
        }
        // Pet command bar (CMSG_PET_ACTION): pass the raw packed `data` + target through; the module
        // decodes stay/follow/attack/dismiss + passive/defensive/aggressive and validates ownership. A
        // transient reject (no pet, dead/invalid target) must NOT drop the session — log + ignore, like
        // the start_attack path (do NOT route through is_desync_error).
        ClientOpcodeMessage::CMSG_PET_ACTION(p) => {
            if let Err(e) = store.pet_command(conn.account_id, self_guid, p.data, p.target.guid()) {
                log::debug!(
                    "world: pet_command ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // The client's ack to our `SMSG_FORCE_RUN_SPEED_CHANGE` (`.speed`). We don't
        // gate on the reply (the movement counter/new_speed aren't cross-checked) — explicitly
        // consumed here (rather than falling through to the dispatch tail's `log::debug!` "ignoring"
        // line) so a `.speed` never spams the log or risks a future desync-classifier false-positive.
        ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_) => {}
        // Draw / stow weapons (#101). The client sends this on `Z`, on a weapon swap, and when an
        // ability auto-draws. It is a pure render-state change: nothing gates on it, so a failure is
        // logged and dropped rather than being session-fatal like ATTACKSWING/ATTACKSTOP. gtker
        // already parsed the payload into a `SheathState`, so the byte reaching the module is one of
        // 0/1/2 — the module re-checks anyway, being the trust boundary for every caller.
        ClientOpcodeMessage::CMSG_SETSHEATHED(s) => {
            let state = s.sheathed.as_int();
            if let Err(e) = store.set_sheathed(conn.account_id, self_guid, state) {
                log::debug!(
                    "world: set_sheathed({state}) ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
