//! Class-trainer family: open the trainer window and buy spells. Pure code-motion out of
//! `world/mod.rs`.

use super::super::*;

/// Class-trainer family: open the trainer window (`CMSG_TRAINER_LIST` → `SMSG_TRAINER_LIST`, each spell
/// Green/Red/Gray) and learn a spell (`CMSG_TRAINER_BUY_SPELL` → the module buy →
/// `SMSG_TRAINER_BUY_*` + a live `SMSG_LEARNED_SPELL` so it hits the action bar without a relog).
/// Needs the in-world player guid (a trainer is only clicked in-world); in CharSelect the opcodes
/// pass through. A buy rejection is per-action — surfaced as `SMSG_TRAINER_BUY_FAILED` (reason
/// parsed from the module's `[N]` tag).
pub(crate) fn handle_trainer<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        WorldState::CharSelect => return Ok(Some(msg)),
    };
    match msg {
        ClientOpcodeMessage::CMSG_TRAINER_LIST(c) => {
            let trainer_guid = c.guid.guid();
            // A trainer that dislikes you refuses the window (silent drop).
            if store
                .npc_refuses_interaction(trainer_guid, self_guid)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            // ...and so does one that does not teach your class. Silent drop, not an empty window:
            // an empty list is indistinguishable from a trainer whose offerings were never imported.
            if !store
                .trainer_serves(self_guid, trainer_guid)
                .unwrap_or(true)
            {
                return Ok(None);
            }
            let spells = store.trainer_list(self_guid, trainer_guid)?;
            // Deliberate simplification: a generic greeting — the per-NPC trainer greeting text is
            // a later npc_text slice (same as the vendor's generic gossip line).
            let list =
                codec::build_trainer_list(trainer_guid, &spells, "I can teach you a thing or two.");
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_LIST(Box::new(list))),
            )?;
        }
        ClientOpcodeMessage::CMSG_TRAINER_BUY_SPELL(c) => {
            let trainer_guid = c.guid.guid();
            let spell_id = c.id;
            match store.buy_trainer_spell(conn.account_id, self_guid, trainer_guid, spell_id) {
                Ok(()) => {
                    // Confirm + push the spell live so it appears on the action bar without a relog.
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(Box::new(
                            codec::build_trainer_buy_succeeded(trainer_guid, spell_id),
                        ))),
                    )?;
                    // Book the RESOLVED rank (465), not the wrapper (1875) — the module granted
                    // the trigger spell; echoing the wrapper put "the spell that teaches Devotion
                    // Aura" in the player's General tab until relog.
                    // A RANK UPGRADE (the chain prev is already known) sends SUPERCEDED
                    // instead — the client REPLACES the old rank's book entry (vanilla) rather
                    // than stacking "Rank 1" next to "Rank 2". WIRE ORDER: cmangos writes
                    // old u16 THEN new u16; gtker's field names claim new-first — per the
                    // field-names-lie precedent we follow cmangos, so `new_spell_id` (the FIRST
                    // wire slot) carries the OLD rank. If live verify shows the NEW rank
                    // vanishing instead, swap these two.
                    let resolved = store.resolve_learn_target(spell_id);
                    match store.superseded_old_rank(resolved, self_guid) {
                        Some(old_rank) => {
                            use wow_world_messages::vanilla::SMSG_SUPERCEDED_SPELL;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(
                                    SMSG_SUPERCEDED_SPELL {
                                        new_spell_id: old_rank as u16,
                                        old_spell_id: resolved as u16,
                                    },
                                )),
                            )?;
                        }
                        None => {
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                    codec::build_learned_spell(resolved),
                                )),
                            )?;
                        }
                    }
                }
                Err(e) => {
                    // The module tags its Err with a [N] gtker failure-reason; 1=money, 2=level/req, else generic.
                    let es = e.to_string();
                    let reason = if es.contains("[1]") {
                        1
                    } else if es.contains("[2]") {
                        2
                    } else {
                        0
                    };
                    log::debug!(
                        "world: buy_trainer_spell failed (account {}): {es}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(Box::new(
                            codec::build_trainer_buy_failed(trainer_guid, spell_id, reason),
                        ))),
                    )?;
                }
            }
        }
        // Spend a talent point (`CMSG_LEARN_TALENT`). The module gates points/prereqs; on success the
        // passive aura relay covers stat/buff updates. If this talent also grants a learnable ability
        // (`grant_spell_id != 0`), push `SMSG_LEARNED_SPELL` so the action bar is usable without a relog.
        // Action-bar persistence: the client sends ONE of these per drag/clear and expects the
        // full bar back at login (SMSG_ACTION_BUTTONS). Unhandled until now — every bar change
        // was lost on relog (only the creation-seeded buttons survived; user find via a
        // talent-learned Consecration vanishing from the bar). `action`+`misc` are the client's
        // packed u24 payload (spell id, or item id spilling into misc); best-effort (a failure
        // must never drop the session — the button just won't stick).
        ClientOpcodeMessage::CMSG_SET_ACTION_BUTTON(c) => {
            let action = c.action as u32 | ((c.misc as u32) << 16);
            if let Err(e) =
                store.set_action_button(conn.account_id, self_guid, c.button, action, c.action_type)
            {
                log::debug!(
                    "world: set_action_button ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // The rep pane's At-War checkbox. The wire's `faction` u16 is the client's
        // 0..63 rep-array slot (ReputationListID — gtker's field name lies, the same
        // SET_FACTION_STANDING precedent); `flags` carries the new checkbox state (AT_WAR = 0x02).
        // Best-effort like SET_ACTION_BUTTON — a failure must never drop the session.
        ClientOpcodeMessage::CMSG_SET_FACTION_ATWAR(c) => {
            let reputation_index = c.faction.as_int() as u32;
            let at_war = c.flags.is_at_war();
            if let Err(e) = store.set_faction_at_war(conn.account_id, self_guid, reputation_index, at_war) {
                log::debug!(
                    "world: set_faction_at_war ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        ClientOpcodeMessage::CMSG_LEARN_TALENT(c) => {
            let talent_id = c.talent.as_int();
            let grant_spell_id = store.talent_grant_spell(talent_id);
            match store.learn_talent(conn.account_id, self_guid, talent_id) {
                Ok(()) => {
                    if grant_spell_id != 0 {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                codec::build_learned_spell(grant_spell_id),
                            )),
                        )?;
                    }
                    // Live talent-pane refresh (user bug: "talents work server-side but the UI
                    // doesn't update"). The 1.12 TalentFrame redraws on exactly two events, and a
                    // PASSIVE pick used to send neither: (a) SPELLS_CHANGED — the pane derives a
                    // talent's shown rank from which RANK-SPELL is in the spellbook, so relay the
                    // rank-spell the module just taught (SUPERCEDED replaces the previous rank's
                    // book entry, same cmangos old-then-new wire order as the trainer path);
                    // (b) CHARACTER_POINTS_CHANGED — push the decremented unspent counter.
                    if let WorldState::InWorld(iw) = &conn.state {
                        let self_guid = iw.self_guid;
                        let (teach, superseded, remaining) =
                            store.talent_pane_sync(self_guid, talent_id);
                        if teach != 0 && teach != grant_spell_id {
                            if superseded != 0 {
                                use wow_world_messages::vanilla::SMSG_SUPERCEDED_SPELL;
                                send(
                                    tx,
                                    Outbound::One(ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(
                                        SMSG_SUPERCEDED_SPELL {
                                            new_spell_id: superseded as u16, // cmangos wire order: OLD rides the first slot
                                            old_spell_id: teach as u16,
                                        },
                                    )),
                                )?;
                            } else {
                                send(
                                    tx,
                                    Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                        codec::build_learned_spell(teach),
                                    )),
                                )?;
                            }
                        }
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                codec::build_talent_points_values(self_guid, remaining),
                            ))),
                        )?;
                        // Spell-modifier mirror: the pick may have applied an A_SPELLMOD
                        // passive — re-send the aggregated totals so the client's cast bars match
                        // the server's folded timings immediately (idempotent absolute values).
                        for m in codec::build_spell_modifier_msgs(&store.spell_modifiers(self_guid))
                        {
                            send(tx, Outbound::One(m))?;
                        }
                    }
                }
                Err(e) => {
                    log::debug!(
                        "world: learn_talent ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
