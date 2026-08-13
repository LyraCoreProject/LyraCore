//! Quest-giver dialog family: the overhead `!`/`?` status, the right-click gossip/quest
//! dialogs, accept/turn-in/abandon. Pure code-motion out of `world/mod.rs`.

use super::super::*;
use super::send_questgiver_menu;

/// Quest-giver dialog family (quests gateway slice): the overhead `!`/`?` status, the right-click
/// quest menu, the quest details + accept, and the turn-in offer/complete round-trip. Quest DETAILS is
/// raw-encoded because its 1.12 reward triples are incomplete in gtker; the other replies are typed.
/// Reads are evaluated against the player, so
/// these need the in-world player guid — in CharSelect the opcodes pass through (a questgiver can only
/// be clicked in-world). Reducer rejections (accept/turn-in gates) are per-action: logged, not fatal.
pub(crate) fn handle_quest<St: WorldStore + ?Sized>(
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
        // The client polls each nearby questgiver for its overhead icon (`!` available / `?` turn-in).
        ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(q) => {
            let giver = q.guid.guid();
            let status = codec::quest_giver_status(&store.quest_giver_evals(giver, self_guid)?);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(Box::new(
                    codec::build_questgiver_status(giver, status),
                ))),
            )?;
        }
        // Right-click a questgiver → the quest menu (every quest it offers/completes for this player).
        // Shared with `handle_loot`'s `CMSG_GAMEOBJ_USE`: a QUESTGIVER-type gameobject
        // opens this exact same menu via `send_questgiver_menu` — the client never sends
        // CMSG_QUESTGIVER_HELLO for a GO giver, only for a creature one.
        ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(h) => {
            // An Unfriendly-or-below questgiver refuses the menu (silent drop).
            if store
                .npc_refuses_interaction(h.guid.guid(), self_guid)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            send_questgiver_menu(tx, store, h.guid.guid(), self_guid)?;
        }
        // Clicked a quest in the menu → its details + Accept button.
        ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(q) => {
            let giver = q.guid.guid();
            if let Some(detail) = store.quest_detail(q.quest_id)? {
                send(tx, {
                    let (opcode, body) = codec::build_quest_details_raw(giver, &detail);
                    Outbound::Raw { opcode, body }
                })?;
            }
        }
        // The client asks for a quest's full definition (it sends this for any quest id it sees in a
        // PLAYER_QUEST_LOG slot but has no data for). Without this reply the client won't display/count
        // the quest in its log — so this is what makes the quest-log window entry actually appear.
        ClientOpcodeMessage::CMSG_QUEST_QUERY(q) => {
            if let Some(detail) = store.quest_detail(q.quest_id)? {
                // RAW-encoded (gtker's typed layout writes the rep Faction fields as u16 → 4-byte title
                // shift). The hand-rolled body matches the 5875 layout exactly.
                let (opcode, body) = codec::build_quest_query_response_raw(&detail);
                send(tx, Outbound::Raw { opcode, body })?;
            }
        }
        // Abandon a quest from the log ("Abandon Quest"). The payload is a LOG slot (0..19), not a quest
        // id — resolve it via the same slot ordering player_quest_log uses, then call the module reducer
        // (deletes the row). The quest-log relay then re-sends the cleared block, so the slot disappears.
        ClientOpcodeMessage::CMSG_QUESTLOG_REMOVE_QUEST(r) => {
            if let Some(s) = store
                .player_quest_log(self_guid)?
                .into_iter()
                .find(|s| s.slot == r.slot)
            {
                if let Err(e) = store.abandon_quest(conn.account_id, self_guid, s.quest_id) {
                    log::debug!(
                        "world: abandon_quest ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Clicked Accept → the module opens the quest log row (gated). No SMSG on success (the client
        // closes the window itself; the quest-log window is the deferred Phase-2 descriptor slice).
        ClientOpcodeMessage::CMSG_QUESTGIVER_ACCEPT_QUEST(a) => {
            if let Err(e) =
                store.accept_quest(conn.account_id, self_guid, a.guid.guid(), a.quest_id)
            {
                log::debug!(
                    "world: accept_quest ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Opened a turn-in (clicked the `?`): the offer-reward screen if every objective is met, else
        // the request-items "not finished" screen (the module is the authority; this only picks the UI).
        ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(c) => {
            let giver = c.guid.guid();
            if let Some(detail) = store.quest_detail(c.quest_id)? {
                let complete = store
                    .quest_giver_evals(giver, self_guid)?
                    .iter()
                    .any(|e| e.quest_id == c.quest_id && e.role == codec::ROLE_END && e.complete);
                let out = if complete {
                    ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
                        codec::build_offer_reward(giver, &detail),
                    ))
                } else {
                    ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                        codec::build_request_items(giver, &detail, false),
                    ))
                };
                send(tx, Outbound::One(out))?;
            }
        }
        // Chose the reward → the module grants money/XP/items (gated on completion). On success, the
        // "Quest Complete" popup echoes what was granted (XP via the shared formula, so it matches).
        ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(c) => {
            match store.turn_in_quest(
                conn.account_id,
                self_guid,
                c.guid.guid(),
                c.quest_id,
                c.reward,
            ) {
                Ok(()) => {
                    if let Some(detail) = store.quest_detail(c.quest_id)? {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(
                                Box::new(codec::build_quest_complete(&detail)),
                            )),
                        )?;
                    }
                }
                Err(e) => log::debug!(
                    "world: turn_in_quest ignored (account {}): {e}",
                    conn.account_id
                ),
            }
        }
        // Share a quest with the party (`CMSG_PUSHQUESTTOPARTY`). The module validates
        // grouped + actively-on-the-quest and pushes the per-member `QUEST_SHARE`/`QUEST_PUSH_RESULT`
        // events itself (relayed by `subscriptions.rs`'s `on_group_event`); no direct SMSG here.
        ClientOpcodeMessage::CMSG_PUSHQUESTTOPARTY(p) => {
            if let Err(e) = store.push_quest(conn.account_id, self_guid, p.quest_id) {
                log::debug!(
                    "world: push_quest ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
