//! Combat family: selection, pet commands, melee swing/stop, sheathing, chat channels and the two
//! cancellation opcodes. Every `CMSG_CAST_SPELL` route now belongs to the cast seam in `cast`.

use super::super::*;

/// Combat family: selection, pet commands, melee swing/stop, sheathing, chat channels and the two
/// cancellation opcodes. ⚠️ Holds the two session-fatal `is_desync_error` early-exits on
/// CMSG_ATTACKSWING/CMSG_ATTACKSTOP — a desync means the player's own entity is gone, so the
/// session is torn down for a clean relog, unlike the transient per-swing failures that stay
/// logged and alive.
pub(crate) fn handle_combat<St: WorldStore + ?Sized>(
    tx: &SessionTx,
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
        // Combat (C1): begin melee auto-attack. Arm the server-side engagement (the swing tick
        // applies damage), and ack with SMSG_ATTACKSTART so the client enters combat stance and
        // plays the swing animation. The per-swing damage text comes from the relayed
        // SMSG_ATTACKERSTATEUPDATE; the health bar from the on_update VALUES relay.
        ClientOpcodeMessage::CMSG_ATTACKSWING(s) => {
            let target_guid = s.guid.guid();
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!("world[autoshot]: CMSG_ATTACKSWING target={target_guid} ranged_repeat_active={was_repeat} (account {})", conn.account_id);
            // A failed start_attack (target already dead/despawned, out of range, not in world) is
            // an EXPECTED transient condition, not session-fatal — e.g. the client swings at the
            // Chicken on the same frame it dies. Log + ignore so the player isn't disconnected; only
            // arm + ack the stance when the engagement actually started.
            match store.start_attack(conn.account_id, self_guid, target_guid) {
                Ok(()) => {
                    if let WorldState::InWorld(iw) = &mut conn.state {
                        iw.attacking_target = Some(target_guid);
                        // Switching to melee overwrites the shared game_melee_attack row to a melee
                        // engagement, so a later CMSG_ATTACKSTOP should now be honored.
                        iw.ranged_repeat = false;
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(
                                codec::build_attack_start(iw.self_guid, target_guid),
                            ))),
                        )?;
                    }
                }
                Err(e) => {
                    // A swing at a corpse → reply SMSG_ATTACKSWING_DEADTARGET so the client leaves
                    // combat stance and shows "can't attack — target is dead" (it otherwise hangs in
                    // stance with no swings, since the server refuses to arm the engagement). A friendly
                    // target → SMSG_ATTACKSWING_CANT_ATTACK. These are TRANSIENT per-swing failures.
                    if e.to_string()
                        .contains(lyracore_shared::ERR_ATTACK_TARGET_DEAD)
                    {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET),
                        )?;
                    } else if e.to_string().contains(lyracore_shared::ERR_ATTACK_FRIENDLY) {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK),
                        )?;
                    } else if is_desync_error(&e) {
                        // The player's OWN entity is gone — a desync, NOT a transient swing failure (the
                        // gateway's view went stale, e.g. a schema migration dropped its subscription). No
                        // action can be served, so the player would otherwise hang in combat stance with
                        // no recovery. Propagate as session-fatal → clean socket teardown → the client
                        // shows "Disconnected" and relog re-materializes from durable state.
                        return Err(e.context(
                            "player desync (entity missing) on attackswing — closing session for a clean relog",
                        ));
                    }
                    // Other failures (out of range, retarget races) are transient → log + ignore.
                    log::debug!(
                        "world: start_attack ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Combat (C1): stop auto-attacking; leave combat stance. Best-effort — a stop_attack
        // failure must not drop the session, and the client is always told to leave stance.
        ClientOpcodeMessage::CMSG_ATTACKSTOP => {
            // While a ranged auto-repeat is armed, the client sends CMSG_ATTACKSTOP as part of
            // switching out of melee stance — but melee + ranged share one game_melee_attack row,
            // so honoring it would delete the auto-shot engagement (one-shot-then-stops). The
            // ranged loop is torn down only by CMSG_CANCEL_AUTO_REPEAT_SPELL; ignore the melee stop.
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!(
                "world[autoshot]: CMSG_ATTACKSTOP ranged_repeat_active={was_repeat} (account {})",
                conn.account_id
            );
            if was_repeat {
                return Ok(None);
            }
            if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                // A desync (entity gone) is session-fatal — recover via a clean disconnect, not a
                // silent hang. A transient stop_attack failure stays logged + ignored.
                if is_desync_error(&e) {
                    return Err(e.context(
                        "player desync (entity missing) on attackstop — closing session for a clean relog",
                    ));
                }
                log::debug!(
                    "world: stop_attack ignored (account {}): {e}",
                    conn.account_id
                );
            }
            // `attacking_target` may name a creature the server already killed (the kill sends its
            // own SMSG_ATTACKSTOP and can't reach this thread to clear it); re-sending stop for a
            // now-dead guid is harmless (the client no longer has that unit).
            if let WorldState::InWorld(iw) = &mut conn.state {
                if let Some(target_guid) = iw.attacking_target.take() {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(
                            codec::build_attack_stop(iw.self_guid, target_guid),
                        ))),
                    )?;
                }
            }
        }
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
        // Cancel a buff: the player right-clicked its icon (CMSG_CANCEL_AURA). Best-effort — remove the
        // caller's own aura by spell id; the aura on_delete relay then re-syncs the buff bar. A failure
        // (no such aura / not in world) is per-action — log + ignore, never tear the session down.
        // Chat channels: the client auto-sends JOIN for General/Trade/LocalDefense on
        // zone-in; ack with SMSG_CHANNEL_NOTIFY(YouJoined) so the tab arms (the client won't
        // accept channel lines for a channel it never got the join notice for). Re-joins are
        // idempotent (the module dedupes; vanilla re-acks). Passwords are ignored (no private
        // channels this slice).
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(c) => {
            if let Err(e) = store.join_channel(conn.account_id, self_guid, c.channel_name.clone()) {
                log::debug!(
                    "world: join_channel failed (account {}): {e}",
                    conn.account_id
                );
            } else {
                use wow_world_messages::vanilla::{ChatNotify, SMSG_CHANNEL_NOTIFY};
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(Box::new(
                        SMSG_CHANNEL_NOTIFY {
                            notify_type: ChatNotify::YouJoinedNotice,
                            channel_name: c.channel_name,
                        },
                    ))),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(c) => {
            if let Err(e) = store.leave_channel(conn.account_id, self_guid, c.channel_name.clone()) {
                log::debug!(
                    "world: leave_channel failed (account {}): {e}",
                    conn.account_id
                );
            } else {
                use wow_world_messages::vanilla::{ChatNotify, SMSG_CHANNEL_NOTIFY};
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(Box::new(
                        SMSG_CHANNEL_NOTIFY {
                            notify_type: ChatNotify::YouLeftNotice,
                            channel_name: c.channel_name,
                        },
                    ))),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_CANCEL_AURA(c) => {
            if let Err(e) = store.cancel_aura(conn.account_id, self_guid, c.id) {
                log::debug!(
                    "world: cancel_aura ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Cancel an in-progress cast (CMSG_CANCEL_CAST — Esc / moved / recast). Clear the caller's pending
        // cast server-side so a scheduled completion can't fire a phantom SMSG_SPELL_GO that wedges the
        // client's cast state ("Another action is in progress"). Best-effort — a failure (nothing pending
        // / not in world) is per-action: log + ignore. The client's spell id (_c) is not needed.
        ClientOpcodeMessage::CMSG_CANCEL_CAST(_c) => {
            if let Err(e) = store.cancel_cast(conn.account_id, social::self_guid(conn).unwrap_or(0)) {
                log::debug!(
                    "world: cancel_cast ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
