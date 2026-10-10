//! Death family: Release Spirit, corpse reclaim and the resurrection paths.

use super::super::*;
use super::combat::ignore_refusal;

/// Release Spirit, corpse reclaim and resurrection.
pub(crate) trait DeathStore: Send + Sync {
    /// Revive the caller after death (`CMSG_REPOP_REQUEST` / Release Spirit): the module
    /// restores full health in place and clears the dead state (the client leaves the death screen
    /// once the restored health replicates).
    fn repop(&self, actor: Actor) -> Result<()>;

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`): the module validates the caller
    /// is a ghost owning the corpse, in range, past the reclaim delay, then resurrects at 50%.
    fn reclaim_corpse(&self, actor: Actor, corpse_guid: u64) -> Result<()>;

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`): `accept=true` revives the
    /// caller at the offer's frozen `%`; either way the offer is consumed. A failure (no pending offer
    /// for the caller) is expected when the offer already lapsed/was answered — per-action, log + ignore.
    fn resurrect_response(&self, actor: Actor, accept: bool) -> Result<()>;

    /// Use the caller's Self-Resurrection Option (`CMSG_SELF_RES`): the module revives the dead
    /// caller in place and spends the option. A Refusal (alive, or no option) is expected after a
    /// race. Per-action: log and ignore.
    fn self_resurrect(&self, actor: Actor) -> Result<()>;

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`): a ghost activates the graveyard Spirit
    /// Healer to res IN PLACE at 50% health/mana + a Resurrection Sickness debuff. `healer_guid` is the
    /// activated healer's guid (passed through to the confirm echo). The module gates on ghost state.
    fn spirit_healer_res(&self, actor: Actor, healer_guid: u64) -> Result<()>;

    /// Find `owner_guid`'s corpse location `(map_id, x, y, z)` for `MSG_CORPSE_QUERY`.
    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>>;
}

/// Release Spirit, the corpse query, corpse reclaim and the three resurrections. Each Refusal is
/// expected after a race, so it is logged and dropped; a revive reaches the client on the entity
/// Relay. A Transport Loss ends the World Session.
pub(crate) fn handle_death<St: DeathStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let actor = social::self_guid(conn).and_then(Actor::new);
    match msg {
        // Release Spirit after death. The client sends this (empty body) when the player
        // clicks Release on the death screen. Revive in place at full health; the restored health
        // replicates via the on_update VALUES relay and the client leaves the death screen.
        // SMSG_CORPSE_RECLAIM_DELAY is now relay-driven (the escalated per-corpse
        // delay, not a flat 30s) — see `on_corpse_insert` in `stdb/subscriptions.rs`, which fires off
        // the SAME `game_corpse` insert `repop`'s reducer call just caused, so no explicit send here.
        ClientOpcodeMessage::CMSG_REPOP_REQUEST => {
            if let Some(actor) = actor {
                ignore_refusal("repop", conn.account_id, store.repop(actor))?;
            }
        }
        // Corpse location query: the client asks where the player's corpse is to draw the
        // map marker + offer "Reclaim Corpse" near it. Reply with the corpse's position, or NotFound.
        ClientOpcodeMessage::MSG_CORPSE_QUERY => {
            if let WorldState::InWorld(iw) = &conn.state {
                let loc = store.corpse_location(iw.self_guid)?;
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::MSG_CORPSE_QUERY(Box::new(
                        codec::build_corpse_query_response(loc)?,
                    ))),
                )?;
            }
        }
        // Reclaim your corpse: the ghost, near its corpse and past the 30s delay, resurrects
        // at 50%. The module validates ownership/ghost/range/delay; a failure (too far, too soon, not
        // a ghost) is expected and silently ignored — the client just stays a ghost.
        ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(r) => {
            if let Some(actor) = actor {
                let result = store.reclaim_corpse(actor, r.guid.guid());
                ignore_refusal("reclaim_corpse", conn.account_id, result)?;
            }
        }
        // Resurrection accept-prompt response: the dead player answered the SMSG_RESURRECT_REQUEST
        // offer. `status` is vanilla's accept(1)/decline(0) byte; the offer's guid is ignored (mirrors
        // `reclaim_corpse`'s own-corpse derivation — the module resolves the pending offer from the
        // CALLER via `ctx.sender()`, never the wire guid). A failure (no pending offer — already
        // answered/lapsed) is expected and silently ignored.
        ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(r) => {
            if let Some(actor) = actor {
                let result = store.resurrect_response(actor, r.status != 0);
                ignore_refusal("resurrect_response", conn.account_id, result)?;
            }
        }
        // The death dialog's second button: use the Self-Resurrection Option the Module wrote into
        // PLAYER_SELF_RES_SPELL. The revive replicates through the entity VALUES relay. A Refusal
        // (already used, already alive) is expected after a race and sends nothing.
        ClientOpcodeMessage::CMSG_SELF_RES => {
            if let Some(actor) = actor {
                ignore_refusal(
                    "self_resurrect",
                    conn.account_id,
                    store.self_resurrect(actor),
                )?;
            }
        }
        // Spirit-Healer resurrection: a ghost activated the graveyard Spirit Healer (npc_flags
        // SPIRITHEALER). The module res's in place at 50% + applies Resurrection Sickness; on success
        // reply with SMSG_SPIRIT_HEALER_CONFIRM (echoing the healer's guid) so the client closes the
        // dialog. The res itself replicates via the entity VALUES relay (health > 0 + cleared ghost
        // bits), exactly like reclaim_corpse. A failure (not a ghost) is per-action — log + ignore.
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(s) => {
            if let Some(actor) = actor {
                let result = store.spirit_healer_res(actor, s.guid.guid());
                let revived = result.is_ok();
                ignore_refusal("spirit_healer_res", conn.account_id, result)?;
                if revived {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(
                            SMSG_SPIRIT_HEALER_CONFIRM { guid: s.guid },
                        )),
                    )?;
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
