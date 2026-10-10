//! Combat family leftovers: selection, pet commands, the run-speed ack and sheathing. Melee start
//! and stop belong to the melee seam in `melee.rs`; every cast opcode, the `CMSG_CAST_SPELL` routes
//! and both cancellations, belongs to the cast seam in `cast`.

use super::super::*;

/// Selection, pet commands and sheathing.
pub(crate) trait CombatStore: Send + Sync {
    /// Record the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3). 0 clears it.
    fn set_target(&self, actor: Actor, target_guid: u64) -> Result<()>;

    /// Relay a pet command-bar action (`CMSG_PET_ACTION`). `data` is the raw packed action
    /// (flag<<24 | id): flag 0x07 = command (Stay/Follow/Attack/Dismiss), flag 0x06 = react state
    /// (Passive/Defensive/Aggressive). The module decodes + validates (all pet policy lives there).
    fn pet_command(&self, actor: Actor, data: u32, target_guid: u64) -> Result<()>;

    /// Draw or stow the player's weapons (`CMSG_SETSHEATHED`, the `Z` key). `state` is 0 stowed /
    /// 1 melee / 2 ranged; the module range-checks it. Writes `UNIT_FIELD_BYTES_2` byte 0, which is
    /// what makes a drawn or stowed weapon visible to OTHER players.
    fn set_sheathed(&self, actor: Actor, state: u8) -> Result<()>;
}

/// A Refusal of a best-effort action is logged and dropped. A Transport Loss ends the World Session.
pub(super) fn ignore_refusal(action: &str, account_id: u64, result: Result<()>) -> Result<()> {
    let Err(error) = result else {
        return Ok(());
    };
    if matches!(
        crate::stdb::classify(&error),
        crate::stdb::DurableFailure::TransportLoss
    ) {
        return Err(error);
    }
    log::debug!("world: {action} ignored (account {account_id}): {error}");
    Ok(())
}

/// Combat family leftovers: selection, pet commands, the run-speed ack and sheathing. Each arm is
/// best-effort. The session-fatal desync exits went to the melee seam with the two melee opcodes
/// that owned them.
pub(crate) fn handle_combat<St: CombatStore + ?Sized>(
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let actor = social::self_guid(conn).and_then(Actor::new);

    match msg {
        // Targeting (N3): record the player's selection server-side (foundation for combat).
        ClientOpcodeMessage::CMSG_SET_SELECTION(s) => {
            let actor = actor.ok_or_else(|| anyhow!("CMSG_SET_SELECTION before world entry"))?;
            store.set_target(actor, s.target.guid())?
        }
        // Pet command bar (CMSG_PET_ACTION): pass the raw packed `data` + target through; the module
        // decodes stay/follow/attack/dismiss + passive/defensive/aggressive and validates ownership. A
        // transient reject (no pet, dead/invalid target) must NOT drop the session — log + ignore, like
        // the start_attack path (do NOT route through is_desync_error).
        ClientOpcodeMessage::CMSG_PET_ACTION(p) => {
            if let Some(actor) = actor {
                let result = store.pet_command(actor, p.data, p.target.guid());
                ignore_refusal("pet_command", conn.account_id, result)?;
            }
        }
        // The client's ack to our `SMSG_FORCE_RUN_SPEED_CHANGE` (`.speed`). We don't
        // gate on the reply (the movement counter/new_speed aren't cross-checked) — explicitly
        // consumed here (rather than falling through to the dispatch tail's `log::debug!` "ignoring"
        // line) so a `.speed` never spams the log or risks a future desync-classifier false-positive.
        ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_) => {}
        // Draw / stow weapons. The client sends this on `Z`, on a weapon swap, and when an
        // ability auto-draws. It is a pure render-state change: nothing gates on it, so a failure is
        // logged and dropped rather than being session-fatal like ATTACKSWING/ATTACKSTOP. gtker
        // already parsed the payload into a `SheathState`, so the byte reaching the module is one of
        // 0/1/2 — the module re-checks anyway, being the trust boundary for every caller.
        ClientOpcodeMessage::CMSG_SETSHEATHED(s) => {
            let state = s.sheathed.as_int();
            if let Some(actor) = actor {
                let result = store.set_sheathed(actor, state);
                ignore_refusal(&format!("set_sheathed({state})"), conn.account_id, result)?;
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
