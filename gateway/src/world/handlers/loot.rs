//! Loot-window action dispatch plus the remaining corpse, GameObject, and group-loot handler.

use super::super::*;
use super::quest::quest_giver_menu;

/// Durable reads and requests needed to open a loot window.
pub(crate) trait LootWindowStore: Send + Sync {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32>;
    fn corpse_loot(&self, target_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>>;
    fn skin_corpse(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()>;
}

impl LootWindowStore for crate::stdb::Coordinator {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        crate::stdb::Coordinator::loot_target_money(self, target_guid)
    }

    fn corpse_loot(&self, target_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>> {
        crate::stdb::Coordinator::corpse_loot(self, target_guid, viewer_guid)
    }

    fn skin_corpse(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::skin_corpse(self, account_id, actor_guid, target_guid)
    }
}

/// The target whose loot window is currently open for this world session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenLootState {
    pub(crate) target_guid: Option<u64>,
}

/// Authenticated player facts needed by loot-window operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LootWindowPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

/// A durable operation the dispatcher requested while producing an outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LootWindowDurableRequest {
    SkinCreature { target_guid: u64 },
}

/// A handled loot request returns all session state and client traffic to apply in order.
pub(crate) enum LootWindowOutcome {
    Handled {
        next_state: OpenLootState,
        durable_request: Option<LootWindowDurableRequest>,
        outbound: Vec<Outbound>,
    },
    PassThrough(ClientOpcodeMessage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LootWindowErrorClass {
    GameplayRefusal,
    Fatal,
}

fn classify_loot_window_error(error: &anyhow::Error) -> LootWindowErrorClass {
    if error
        .chain()
        .any(|cause| {
            let message = cause.to_string();
            message.starts_with("gw_skin reducer failed:")
                && !message.starts_with("gw_skin reducer failed: transport disconnected")
        })
    {
        LootWindowErrorClass::GameplayRefusal
    } else {
        LootWindowErrorClass::Fatal
    }
}

/// Map a client request through the loot-window lifecycle without owning session transport.
pub(crate) fn dispatch_loot_window<St: LootWindowStore + ?Sized>(
    store: &St,
    player: LootWindowPlayer,
    current_state: OpenLootState,
    msg: ClientOpcodeMessage,
) -> Result<LootWindowOutcome> {
    match msg {
        ClientOpcodeMessage::CMSG_LOOT(request) => {
            let Some(viewer_guid) = player.self_guid else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let target_guid = request.guid.guid();
            let money = store.loot_target_money(target_guid)?;
            let items = store.corpse_loot(target_guid, viewer_guid)?;
            let durable_request = if items.is_empty() && money == 0 {
                match store.skin_corpse(player.account_id, viewer_guid, target_guid) {
                    Ok(()) => {}
                    Err(error)
                        if classify_loot_window_error(&error)
                            == LootWindowErrorClass::GameplayRefusal =>
                    {
                        log::debug!(
                            "world: skin_corpse noop (account {}): {error}",
                            player.account_id
                        );
                    }
                    Err(error) => return Err(error),
                }
                Some(LootWindowDurableRequest::SkinCreature { target_guid })
            } else {
                None
            };
            let (opcode, body) = codec::build_loot_response_raw(target_guid, money, &items);
            Ok(LootWindowOutcome::Handled {
                next_state: OpenLootState {
                    target_guid: Some(target_guid),
                },
                durable_request,
                outbound: vec![Outbound::Raw { opcode, body }],
            })
        }
        other => Ok(LootWindowOutcome::PassThrough(other)),
    }
}

/// Remaining loot, GameObject, group-loot, and death-recovery operations not yet migrated to a
/// focused action interface.
pub(crate) fn handle_loot<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::CMSG_LOOT;
    use wow_world_messages::Guid;

    #[derive(Default)]
    struct InMemoryLootWindow {
        money: u32,
        items_by_viewer: HashMap<u64, Vec<codec::LootItemView>>,
        money_reads: Mutex<Vec<u64>>,
        item_reads: Mutex<Vec<(u64, u64)>>,
        skin_requests: Mutex<Vec<(u64, u64, u64)>>,
        skin_error: Option<String>,
    }

    impl LootWindowStore for InMemoryLootWindow {
        fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
            self.money_reads.lock().unwrap().push(target_guid);
            Ok(self.money)
        }

        fn corpse_loot(
            &self,
            target_guid: u64,
            viewer_guid: u64,
        ) -> Result<Vec<codec::LootItemView>> {
            self.item_reads
                .lock()
                .unwrap()
                .push((target_guid, viewer_guid));
            Ok(self
                .items_by_viewer
                .get(&viewer_guid)
                .cloned()
                .unwrap_or_default())
        }

        fn skin_corpse(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
            self.skin_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            self.skin_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!(error.clone())))
        }
    }

    fn player() -> LootWindowPlayer {
        LootWindowPlayer {
            account_id: 7,
            self_guid: Some(42),
        }
    }

    fn open_creature(target_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LOOT(CMSG_LOOT {
            guid: Guid::new(target_guid),
        })
    }

    #[test]
    fn creature_open_returns_the_viewers_current_loot_and_replaces_the_open_target() {
        let mut store = InMemoryLootWindow {
            money: 25,
            ..Default::default()
        };
        store.items_by_viewer.insert(42, vec![(3, 2589, 5, 200)]);

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState {
                target_guid: Some(11),
            },
            open_creature(60),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("creature open passed through")
        };
        assert_eq!(next_state.target_guid, Some(60));
        assert_eq!(durable_request, None);
        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &60u64.to_le_bytes());
        assert_eq!(&body[9..13], &25u32.to_le_bytes());
        assert_eq!(body[13], 1);
        assert_eq!(body[14], 3);
        assert_eq!(&body[15..19], &2589u32.to_le_bytes());
        assert_eq!(&body[19..23], &5u32.to_le_bytes());
        assert_eq!(&body[23..27], &200u32.to_le_bytes());
        assert_eq!(store.money_reads.lock().unwrap().as_slice(), &[60]);
        assert_eq!(store.item_reads.lock().unwrap().as_slice(), &[(60, 42)]);
        assert!(store.skin_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn fully_empty_creature_attempts_skinning_and_returns_an_empty_window() {
        let store = InMemoryLootWindow::default();

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(61),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("creature open passed through")
        };
        assert_eq!(next_state.target_guid, Some(61));
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::SkinCreature { target_guid: 61 })
        );
        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &61u64.to_le_bytes());
        assert_eq!(&body[9..13], &0u32.to_le_bytes());
        assert_eq!(body[13], 0);
        assert_eq!(
            store.skin_requests.lock().unwrap().as_slice(),
            &[(7, 42, 61)]
        );
    }

    #[test]
    fn skinning_refusal_is_a_handled_empty_window() {
        let store = InMemoryLootWindow {
            skin_error: Some("gw_skin reducer failed: target is not skinnable".into()),
            ..Default::default()
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(61),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state: OpenLootState {
                    target_guid: Some(61)
                },
                durable_request: Some(LootWindowDurableRequest::SkinCreature {
                    target_guid: 61
                }),
                outbound,
            } if matches!(outbound.as_slice(), [Outbound::Raw { opcode: 0x0160, body }] if body[13] == 0)
        ));
    }

    #[test]
    fn fatal_skinning_failures_propagate() {
        for message in [
            "gw_skin reducer transport disconnected: channel closed",
            "gw_skin reducer failed: transport disconnected",
            "send gw_skin: connection closed",
            "gw_skin reducer timed out after 10s",
        ] {
            let store = InMemoryLootWindow {
                skin_error: Some(message.into()),
                ..Default::default()
            };

            let error = dispatch_loot_window(
                &store,
                player(),
                OpenLootState::default(),
                open_creature(61),
            )
            .err()
            .expect("fatal skinning failure was handled");

            assert_eq!(error.to_string(), message);
        }
    }
}
