//! Loot / corpse family: loot a corpse (open window / take money / close window) and the
//! master-loot / group-roll flows. Pure code-motion out of `world/mod.rs`.

use super::super::*;
use super::quest::quest_giver_menu;

/// Loot / corpse family (slices 3/4/5): loot a corpse (open window / take money / close window) and
/// the death-recovery flow (release spirit / corpse-location query / reclaim corpse) — grouped as
/// the "corpse-interaction" family.
pub(crate) fn handle_loot<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // Loot (slice 3): right-click a corpse → open the loot window showing its money. The open
        // corpse is tracked on the connection (CMSG_LOOT_MONEY carries no guid). `loot_target_money`
        // reads the corpse's copper from the privileged cache; a non-corpse/empty target yields 0
        // (the client just sees an empty window). The transfer is gated server-side on the take.
        ClientOpcodeMessage::CMSG_LOOT(l) => {
            if let WorldState::InWorld(iw) = &mut conn.state {
                let target_guid = l.guid.guid();
                let money = store.loot_target_money(target_guid)?;
                // Items slice-4: the corpse's item loot fills the loot window alongside the gold.
                // Sent RAW because gtker's typed LootItem omits count/display/random (would desync
                // the 5875 client's parse); `build_loot_response_raw` hand-rolls the full layout.
                // `iw.self_guid` is the VIEWER: `corpse_loot` filters
                // `quest_only` rows per viewer, so two grouped looters of the same corpse can see
                // different windows.
                let items = store
                    .corpse_loot(target_guid, iw.self_guid)
                    .unwrap_or_default();
                // Skinning fallback: if the corpse is fully looted (no items, no money left),
                // attempt to skin it. On success the leather appears in the bag via the item
                // subscription relay — the empty loot window is the only feedback (no separate skinning
                // animation in this alpha). On Err (not a beast, already skinned, out of range) the
                // player just sees the empty window, which is the correct safe fallback.
                if items.is_empty() && money == 0 {
                    if let Err(e) = store.skin_corpse(conn.account_id, iw.self_guid, target_guid) {
                        log::debug!("world: skin_corpse noop (account {}): {e}", conn.account_id);
                    }
                }
                iw.looting_target = Some(target_guid);
                let (opcode, body) = codec::build_loot_response_raw(target_guid, money, &items);
                send(tx, Outbound::Raw { opcode, body })?;
            }
        }
        // Take the money from the open corpse. The reducer validates (dead / in-range / has-money)
        // and moves the copper; the new purse rides back as a PLAYER_FIELD_COINAGE VALUES relay and
        // the corpse's cleared lootable flag as a dynamic_flags VALUES relay (the sparkle vanishes).
        // `SMSG_LOOT_MONEY_NOTIFY` is NO LONGER sent unconditionally here — vanilla
        // sends it ONLY to party members receiving a coin split (the 1.12 client renders it as the
        // "Your share of the loot is X" line); a SOLO looter gets no notify at all and instead relies
        // on the client's own local "You loot X copper" line printed when the coin window clears. A
        // GROUPED split's per-recipient notify rides the `MONEY_SHARE` `game_group_event` relay
        // (`stdb/subscriptions.rs`) instead, exactly like the roll/master-loot notifications. Only
        // `SMSG_LOOT_CLEAR_MONEY` (clears the coin icon from the still-open window) stays here.
        ClientOpcodeMessage::CMSG_LOOT_MONEY => {
            let (self_guid, open) = match &conn.state {
                WorldState::InWorld(iw) => (iw.self_guid, iw.looting_target),
                WorldState::CharSelect => (0, None),
            };
            if let Some(target_guid) = open {
                match store.loot_money(conn.account_id, self_guid, target_guid) {
                    Ok(()) => {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY),
                        )?;
                    }
                    Err(e) => log::debug!(
                        "world: loot_money ignored (account {}): {e}",
                        conn.account_id
                    ),
                }
            }
        }
        // Take an item from the open corpse into the backpack (slice 4). The open corpse is tracked on
        // the connection (the message carries only the loot slot). On success the module moves the item
        // into a free inventory slot — the item then appears in the bag via the inventory live-relay —
        // and deletes the loot row; SMSG_LOOT_REMOVED clears that slot from the open window. A failure
        // (bag full / out of range / already taken) is per-action — log + ignore, never tear the session.
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(a) => {
            let (self_guid, open) = match &conn.state {
                WorldState::InWorld(iw) => (iw.self_guid, iw.looting_target),
                WorldState::CharSelect => (0, None),
            };
            if let Some(corpse_guid) = open {
                match store.take_loot(conn.account_id, self_guid, corpse_guid, a.item_slot) {
                    Ok(()) => send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(
                            codec::build_loot_removed(a.item_slot),
                        )),
                    )?,
                    Err(e) => log::debug!(
                        "world: take_loot ignored (account {}): {e}",
                        conn.account_id
                    ),
                }
            }
        }
        // Close the loot window: clear the open-corpse state and ack so the client releases the UI.
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(l) => {
            let target_guid = l.guid.guid();
            if let WorldState::InWorld(iw) = &mut conn.state {
                iw.looting_target = None;
            }
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(
                    codec::build_loot_release_response(target_guid),
                ))),
            )?;
        }
        // Group loot methods: a need/greed vote, and the master looter's
        // explicit assign. Both are per-action — a rejection (no roll open, already voted, not the
        // master) is logged + ignored rather than tearing the session; the live vote/winner/master
        // packets ride the `game_group_event` roll relay (`stdb/subscriptions.rs`), not a direct
        // reply here.
        ClientOpcodeMessage::CMSG_LOOT_ROLL(c) => {
            let corpse_guid = c.item.guid();
            let vote = c.vote.as_int();
            // Unsharded, `loot::run_vote` is exactly the call above (`store.loot_roll`);
            // sharded, it routes to realm-core instead, so the guid it authorizes as the voter must
            // be the one THIS socket authenticated with, never a literal from the packet.
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = loot::run_vote(
                store,
                conn.account_id,
                self_guid,
                corpse_guid,
                c.item_slot,
                vote,
            ) {
                log::debug!(
                    "world: loot_roll ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(c) => {
            let corpse_guid = c.loot.guid();
            let target_guid = c.player.guid();
            if let Err(e) =
                store.loot_master_give(conn.account_id, social::self_guid(conn).unwrap_or(0), corpse_guid, c.slot_id, target_guid)
            {
                log::debug!(
                    "world: loot_master_give ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Use a gameobject (CMSG_GAMEOBJ_USE): the module rolls a CHEST's loot into game_corpse_loot
        // (keyed on the GO guid) or grants a quest-object's credit. For a chest we then open the loot
        // window the same way CMSG_LOOT does. Known limitation: the post-reducer corpse_loot read
        // can lag the loot-row subscription on the very first use (a re-click's CMSG_LOOT
        // re-serves the same rows); the roll itself is committed server-side. A goober yields no
        // rows → no window.
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(g) => {
            let go_guid = g.guid.guid();
            // The VIEWER for `quest_only` filtering — a chest is only usable
            // in-world; 0 (no real character ever has this guid) is a safe "no viewer" fallback for
            // the CharSelect edge case, hiding every quest_only row rather than guessing.
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            // A QUESTGIVER-type GO (the Wanted Poster GO 68, the Lost Guards corpses GO 55/56)
            // never rolls loot or grants credit: in vanilla a
            // GAMEOBJECT_TYPE_QUESTGIVER goes straight to the quest window (the same menu/single-quest
            // logic `CMSG_QUESTGIVER_HELLO` uses for a creature giver), so the client's right-click
            // opens the quest dialog on it exactly like an NPC — it never sends a separate
            // CMSG_QUESTGIVER_HELLO for a gameobject giver. Checked BEFORE the ordinary use-reducer
            // path, and short-circuits it entirely (the module's `apply_use_gameobject` treats
            // QUESTGIVER as an inert no-op anyway, so skipping it changes nothing state-side).
            if store.gameobject_type(go_guid)?
                == Some(lyracore_shared::constants::go_type::QUESTGIVER)
            {
                for message in quest_giver_menu(store, go_guid, self_guid)? {
                    send(tx, message)?;
                }
            } else {
                match store.use_gameobject(conn.account_id, self_guid, go_guid) {
                    Ok(()) => {
                        let items = store.corpse_loot(go_guid, self_guid).unwrap_or_default();
                        if !items.is_empty() {
                            if let WorldState::InWorld(iw) = &mut conn.state {
                                iw.looting_target = Some(go_guid);
                            }
                            let (opcode, body) = codec::build_loot_response_raw(go_guid, 0, &items);
                            send(tx, Outbound::Raw { opcode, body })?;
                        }
                    }
                    Err(e) => log::debug!(
                        "world: use_gameobject ignored (account {}): {e}",
                        conn.account_id
                    ),
                }
            }
        }
        // Enter an area trigger (CMSG_AREATRIGGER): the client fires this when the player physically
        // walks into a trigger zone (e.g. a mine for an "explore" quest). The module credits any active
        // explore quest tied to the trigger id. A transient/no-match result is logged + ignored.
        ClientOpcodeMessage::CMSG_AREATRIGGER(a) => {
            if let Err(e) = store.enter_areatrigger(conn.account_id, social::self_guid(conn).unwrap_or(0), a.trigger_id) {
                log::debug!(
                    "world: enter_areatrigger ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Gameobject template query (CMSG_GAMEOBJECT_QUERY): the client asks for a GO's name/type/display
        // before it renders/interacts. Reply with the template, or the not-found form.
        ClientOpcodeMessage::CMSG_GAMEOBJECT_QUERY(q) => {
            let tmpl = store.gameobject_template(q.entry_id)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(
                    Box::new(codec::build_gameobject_query_response(
                        q.entry_id,
                        tmpl.as_ref(),
                    )),
                )),
            )?;
        }
        // Release Spirit after death (slice 4). The client sends this (empty body) when the player
        // clicks Release on the death screen. Revive in place at full health; the restored health
        // replicates via the on_update VALUES relay and the client leaves the death screen.
        // SMSG_CORPSE_RECLAIM_DELAY is now relay-driven (the escalated per-corpse
        // delay, not a flat 30s) — see `on_corpse_insert` in `stdb/subscriptions.rs`, which fires off
        // the SAME `game_corpse` insert `repop`'s reducer call just caused, so no explicit send here.
        ClientOpcodeMessage::CMSG_REPOP_REQUEST => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.repop(conn.account_id, self_guid) {
                log::debug!("world: repop ignored (account {}): {e}", conn.account_id);
            }
        }
        // Corpse location query (slice 5): the client asks where the player's corpse is to draw the
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
        // Reclaim your corpse (slice 5): the ghost, near its corpse and past the 30s delay, resurrects
        // at 50%. The module validates ownership/ghost/range/delay; a failure (too far, too soon, not
        // a ghost) is expected and silently ignored — the client just stays a ghost.
        ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(r) => {
            if let Err(e) = store.reclaim_corpse(conn.account_id, social::self_guid(conn).unwrap_or(0), r.guid.guid()) {
                log::debug!(
                    "world: reclaim_corpse ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Resurrection accept-prompt response: the dead player answered the SMSG_RESURRECT_REQUEST
        // offer. `status` is vanilla's accept(1)/decline(0) byte; the offer's guid is ignored (mirrors
        // `reclaim_corpse`'s own-corpse derivation — the module resolves the pending offer from the
        // CALLER via `ctx.sender()`, never the wire guid). A failure (no pending offer — already
        // answered/lapsed) is expected and silently ignored.
        ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(r) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.resurrect_response(conn.account_id, self_guid, r.status != 0) {
                log::debug!(
                    "world: resurrect_response ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Spirit-Healer resurrection: a ghost activated the graveyard Spirit Healer (npc_flags
        // SPIRITHEALER). The module res's in place at 50% + applies Resurrection Sickness; on success
        // reply with SMSG_SPIRIT_HEALER_CONFIRM (echoing the healer's guid) so the client closes the
        // dialog. The res itself replicates via the entity VALUES relay (health > 0 + cleared ghost
        // bits), exactly like reclaim_corpse. A failure (not a ghost) is per-action — log + ignore.
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(s) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            match store.spirit_healer_res(conn.account_id, self_guid, s.guid.guid()) {
                Ok(()) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(
                        SMSG_SPIRIT_HEALER_CONFIRM { guid: s.guid },
                    )),
                )?,
                Err(e) => {
                    log::debug!(
                        "world: spirit_healer_res ignored (account {}): {e}",
                        conn.account_id
                    )
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
