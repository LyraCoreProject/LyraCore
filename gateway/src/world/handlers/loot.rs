//! Loot-window action dispatch plus the remaining corpse, GameObject, and group-loot handler.

use super::super::*;

/// Durable reads and requests needed by the loot-window lifecycle.
pub(crate) trait LootWindowStore: Send + Sync {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32>;
    fn corpse_loot(&self, target_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>>;
    fn use_gameobject(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn skin_corpse(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn loot_money(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus>;
    fn take_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus>;
}

/// The durable adapter distinguishes an expected gameplay refusal from a completed request.
pub(crate) enum LootWindowRequestStatus {
    Applied,
    Refused(anyhow::Error),
}

fn coordinator_request_status(
    operation: &str,
    result: Result<()>,
) -> Result<LootWindowRequestStatus> {
    match result {
        Ok(()) => Ok(LootWindowRequestStatus::Applied),
        Err(error) => {
            let refusal_prefix = format!("{operation} reducer failed:");
            let is_fatal = error.chain().any(|cause| {
                let message = cause.to_string();
                message.contains("transport disconnected")
                    || message.contains("timed out")
                    || message.starts_with("send ")
            });
            let is_gameplay_refusal = error
                .chain()
                .any(|cause| cause.to_string().starts_with(&refusal_prefix));
            if is_gameplay_refusal && !is_fatal {
                Ok(LootWindowRequestStatus::Refused(error))
            } else {
                Err(error)
            }
        }
    }
}

impl LootWindowStore for crate::stdb::Coordinator {
    fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        crate::stdb::Coordinator::loot_target_money(self, target_guid)
    }

    fn corpse_loot(&self, target_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>> {
        crate::stdb::Coordinator::corpse_loot(self, target_guid, viewer_guid)
    }

    fn use_gameobject(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        coordinator_request_status(
            "gw_use_gameobject",
            crate::stdb::Coordinator::use_gameobject(self, account_id, actor_guid, target_guid),
        )
    }

    fn skin_corpse(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        coordinator_request_status(
            "gw_skin",
            crate::stdb::Coordinator::skin_corpse(self, account_id, actor_guid, target_guid),
        )
    }

    fn loot_money(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
    ) -> Result<LootWindowRequestStatus> {
        coordinator_request_status(
            "gw_loot_money",
            crate::stdb::Coordinator::loot_money(self, account_id, actor_guid, target_guid),
        )
    }

    fn take_loot(
        &self,
        account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        loot_slot: u8,
    ) -> Result<LootWindowRequestStatus> {
        coordinator_request_status(
            "gw_take_loot",
            crate::stdb::Coordinator::take_loot(
                self,
                account_id,
                actor_guid,
                target_guid,
                loot_slot,
            ),
        )
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
    UseGameObject { target_guid: u64 },
    SkinCreature { target_guid: u64 },
    TakeMoney { target_guid: u64 },
    TakeItem { target_guid: u64, loot_slot: u8 },
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

fn loot_take_outbound(
    account_id: u64,
    operation: &str,
    result: Result<LootWindowRequestStatus>,
    success: Outbound,
) -> Result<Vec<Outbound>> {
    match result {
        Ok(LootWindowRequestStatus::Applied) => Ok(vec![success]),
        Ok(LootWindowRequestStatus::Refused(error)) => {
            log::debug!("world: {operation} rejected (account {account_id}): {error}");
            Ok(Vec::new())
        }
        Err(error) => Err(error),
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
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(request) => {
            let Some(actor_guid) = player.self_guid else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let target_guid = request.guid.guid();
            let durable_request = Some(LootWindowDurableRequest::UseGameObject { target_guid });
            match store.use_gameobject(player.account_id, actor_guid, target_guid)? {
                LootWindowRequestStatus::Applied => {}
                LootWindowRequestStatus::Refused(error) => {
                    log::debug!(
                        "world: use_gameobject rejected (account {}): {error}",
                        player.account_id
                    );
                    return Ok(LootWindowOutcome::Handled {
                        next_state: current_state,
                        durable_request,
                        outbound: Vec::new(),
                    });
                }
            }
            let items = store.corpse_loot(target_guid, actor_guid)?;
            if items.is_empty() {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request,
                    outbound: Vec::new(),
                });
            }
            let (opcode, body) = codec::build_loot_response_raw(target_guid, 0, &items);
            Ok(LootWindowOutcome::Handled {
                next_state: OpenLootState {
                    target_guid: Some(target_guid),
                },
                durable_request,
                outbound: vec![Outbound::Raw { opcode, body }],
            })
        }
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
                match store.skin_corpse(player.account_id, viewer_guid, target_guid)? {
                    LootWindowRequestStatus::Applied => {}
                    LootWindowRequestStatus::Refused(error) => {
                        log::debug!(
                            "world: skin_corpse noop (account {}): {error}",
                            player.account_id
                        );
                    }
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
        ClientOpcodeMessage::CMSG_LOOT_MONEY => {
            let (Some(actor_guid), Some(target_guid)) =
                (player.self_guid, current_state.target_guid)
            else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let outbound = loot_take_outbound(
                player.account_id,
                "loot_money",
                store.loot_money(player.account_id, actor_guid, target_guid),
                Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY),
            )?;
            Ok(LootWindowOutcome::Handled {
                next_state: current_state,
                durable_request: Some(LootWindowDurableRequest::TakeMoney { target_guid }),
                outbound,
            })
        }
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(request) => {
            let (Some(actor_guid), Some(target_guid)) =
                (player.self_guid, current_state.target_guid)
            else {
                return Ok(LootWindowOutcome::Handled {
                    next_state: current_state,
                    durable_request: None,
                    outbound: Vec::new(),
                });
            };
            let outbound = loot_take_outbound(
                player.account_id,
                "take_loot",
                store.take_loot(
                    player.account_id,
                    actor_guid,
                    target_guid,
                    request.item_slot,
                ),
                Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(
                    codec::build_loot_removed(request.item_slot),
                )),
            )?;
            Ok(LootWindowOutcome::Handled {
                next_state: current_state,
                durable_request: Some(LootWindowDurableRequest::TakeItem {
                    target_guid,
                    loot_slot: request.item_slot,
                }),
                outbound,
            })
        }
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(request) => Ok(LootWindowOutcome::Handled {
            next_state: OpenLootState::default(),
            durable_request: None,
            outbound: vec![Outbound::One(
                ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(
                    codec::build_loot_release_response(request.guid.guid()),
                )),
            )],
        }),
        other => Ok(LootWindowOutcome::PassThrough(other)),
    }
}

/// Remaining group-loot, non-window GameObject, and death-recovery operations not yet migrated to
/// a focused action interface.
pub(crate) fn handle_loot<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
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
    use wow_world_messages::vanilla::{
        CMSG_AUTOSTORE_LOOT_ITEM, CMSG_GAMEOBJ_USE, CMSG_LOOT, CMSG_LOOT_RELEASE,
    };
    use wow_world_messages::Guid;

    #[derive(Default)]
    struct InMemoryLootWindow {
        money: u32,
        items_by_viewer: HashMap<u64, Vec<codec::LootItemView>>,
        money_reads: Mutex<Vec<u64>>,
        item_reads: Mutex<Vec<(u64, u64)>>,
        use_requests: Mutex<Vec<(u64, u64, u64)>>,
        operations: Mutex<Vec<&'static str>>,
        skin_requests: Mutex<Vec<(u64, u64, u64)>>,
        money_take_requests: Mutex<Vec<(u64, u64, u64)>>,
        item_take_requests: Mutex<Vec<(u64, u64, u64, u8)>>,
        skin_refusal: Option<String>,
        skin_fatal_error: Option<String>,
        use_refusal: Option<String>,
        use_fatal_error: Option<String>,
        money_take_refusal: Option<String>,
        money_take_fatal_error: Option<String>,
        item_take_refusal: Option<String>,
        item_take_fatal_error: Option<String>,
    }

    fn request_status(
        refusal: &Option<String>,
        fatal_error: &Option<String>,
    ) -> Result<LootWindowRequestStatus> {
        if let Some(error) = fatal_error {
            Err(anyhow::anyhow!(error.clone()))
        } else if let Some(error) = refusal {
            Ok(LootWindowRequestStatus::Refused(anyhow::anyhow!(
                error.clone()
            )))
        } else {
            Ok(LootWindowRequestStatus::Applied)
        }
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
            self.operations.lock().unwrap().push("read generated loot");
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

        fn use_gameobject(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.operations.lock().unwrap().push("use gameobject");
            self.use_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            request_status(&self.use_refusal, &self.use_fatal_error)
        }

        fn skin_corpse(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.skin_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            request_status(&self.skin_refusal, &self.skin_fatal_error)
        }

        fn loot_money(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
        ) -> Result<LootWindowRequestStatus> {
            self.money_take_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            request_status(&self.money_take_refusal, &self.money_take_fatal_error)
        }

        fn take_loot(
            &self,
            account_id: u64,
            actor_guid: u64,
            target_guid: u64,
            loot_slot: u8,
        ) -> Result<LootWindowRequestStatus> {
            self.item_take_requests.lock().unwrap().push((
                account_id,
                actor_guid,
                target_guid,
                loot_slot,
            ));
            request_status(&self.item_take_refusal, &self.item_take_fatal_error)
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

    fn open_chest(target_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(CMSG_GAMEOBJ_USE {
            guid: Guid::new(target_guid),
        })
    }

    #[test]
    fn chest_use_precedes_generated_loot_and_opens_the_shared_window() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(4, 117, 2, 321)]);
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_chest(90)).unwrap();

        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("chest use passed through")
        };
        assert_eq!(next_state.target_guid, Some(90));
        assert_eq!(
            durable_request,
            Some(LootWindowDurableRequest::UseGameObject { target_guid: 90 })
        );
        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected one raw loot window")
        };
        assert_eq!(*opcode, 0x0160);
        assert_eq!(&body[0..8], &90u64.to_le_bytes());
        assert_eq!(&body[9..13], &0u32.to_le_bytes());
        assert_eq!(body[13], 1);
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject", "read generated loot"]
        );
        assert_eq!(
            store.use_requests.lock().unwrap().as_slice(),
            &[(7, 42, 90)]
        );
        assert_eq!(store.item_reads.lock().unwrap().as_slice(), &[(90, 42)]);
    }

    #[test]
    fn successful_empty_chest_use_keeps_the_current_window_state() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_chest(90)).unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::UseGameObject {
                    target_guid: 90
                }),
                outbound,
            } if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject", "read generated loot"]
        );
    }

    #[test]
    fn refused_chest_use_does_not_read_loot_or_change_window_state() {
        let store = InMemoryLootWindow {
            use_refusal: Some("out of range".into()),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let outcome =
            dispatch_loot_window(&store, player(), current_state, open_chest(90)).unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::UseGameObject {
                    target_guid: 90
                }),
                outbound,
            } if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject"]
        );
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn fatal_chest_use_failure_propagates_without_reading_loot() {
        let store = InMemoryLootWindow {
            use_fatal_error: Some("transport disconnected".into()),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(11),
        };

        let error = dispatch_loot_window(&store, player(), current_state, open_chest(90))
            .err()
            .expect("fatal GameObject failure was handled");

        assert_eq!(error.to_string(), "transport disconnected");
        assert_eq!(
            store.operations.lock().unwrap().as_slice(),
            &["use gameobject"]
        );
        assert!(store.item_reads.lock().unwrap().is_empty());
    }

    fn release(target_guid: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(CMSG_LOOT_RELEASE {
            guid: Guid::new(target_guid),
        })
    }

    fn dispatch_release(current_state: OpenLootState, request_target: u64) -> OpenLootState {
        let outcome = dispatch_loot_window(
            &InMemoryLootWindow::default(),
            player(),
            current_state,
            release(request_target),
        )
        .unwrap();
        let LootWindowOutcome::Handled {
            next_state,
            durable_request,
            outbound,
        } = outcome
        else {
            panic!("release passed through")
        };
        assert_eq!(durable_request, None);
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(response))]
                if response.guid.guid() == request_target
        ));
        next_state
    }

    #[test]
    fn money_take_success_uses_the_open_target_and_only_clears_money() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(60),
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::TakeMoney { target_guid: 60 }),
                outbound,
            } if next_state == current_state
                && matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY)])
        ));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
        );
    }

    #[test]
    fn money_take_refusal_keeps_the_window_without_a_false_clear() {
        let store = InMemoryLootWindow {
            money_take_refusal: Some("corpse has no money".into()),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(60),
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::TakeMoney { target_guid: 60 }),
                outbound,
            } if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.money_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 60)]
        );
    }

    #[test]
    fn money_take_without_an_open_target_has_no_durable_request_or_outbound() {
        let store = InMemoryLootWindow::default();

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            ClientOpcodeMessage::CMSG_LOOT_MONEY,
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state: OpenLootState { target_guid: None },
                durable_request: None,
                outbound,
            } if outbound.is_empty()
        ));
        assert!(store.money_take_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn item_take_success_uses_the_open_target_and_only_removes_the_requested_slot() {
        let store = InMemoryLootWindow::default();
        let current_state = OpenLootState {
            target_guid: Some(75),
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::TakeItem {
                    target_guid: 75,
                    loot_slot: 3,
                }),
                outbound,
            } if next_state == current_state
                && matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed))] if removed.slot == 3)
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 75, 3)]
        );
    }

    #[test]
    fn item_take_refusal_keeps_the_window_without_a_false_slot_removal() {
        let store = InMemoryLootWindow {
            item_take_refusal: Some("inventory full".into()),
            ..Default::default()
        };
        let current_state = OpenLootState {
            target_guid: Some(75),
        };

        let outcome = dispatch_loot_window(
            &store,
            player(),
            current_state,
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state,
                durable_request: Some(LootWindowDurableRequest::TakeItem {
                    target_guid: 75,
                    loot_slot: 3,
                }),
                outbound,
            } if next_state == current_state && outbound.is_empty()
        ));
        assert_eq!(
            store.item_take_requests.lock().unwrap().as_slice(),
            &[(7, 42, 75, 3)]
        );
    }

    #[test]
    fn item_take_without_an_open_target_has_no_durable_request_or_outbound() {
        let store = InMemoryLootWindow::default();

        let outcome = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                item_slot: 3,
            }),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            LootWindowOutcome::Handled {
                next_state: OpenLootState { target_guid: None },
                durable_request: None,
                outbound,
            } if outbound.is_empty()
        ));
        assert!(store.item_take_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn fatal_money_and_item_take_failures_propagate() {
        let operations = [
            (
                "gw_loot_money reducer transport disconnected: channel closed",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
            (
                "gw_loot_money reducer failed: transport disconnected",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
            (
                "send gw_loot_money: connection closed",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
            (
                "gw_loot_money reducer timed out after 10s",
                ClientOpcodeMessage::CMSG_LOOT_MONEY,
            ),
        ];
        for (message, request) in operations {
            let store = InMemoryLootWindow {
                money_take_fatal_error: Some(message.into()),
                ..Default::default()
            };

            let error = dispatch_loot_window(
                &store,
                player(),
                OpenLootState {
                    target_guid: Some(60),
                },
                request,
            )
            .err()
            .expect("fatal money-take failure was handled");

            assert_eq!(error.to_string(), message);
        }

        for message in [
            "gw_take_loot reducer transport disconnected: channel closed",
            "gw_take_loot reducer failed: transport disconnected",
            "send gw_take_loot: connection closed",
            "gw_take_loot reducer timed out after 10s",
        ] {
            let store = InMemoryLootWindow {
                item_take_fatal_error: Some(message.into()),
                ..Default::default()
            };

            let error = dispatch_loot_window(
                &store,
                player(),
                OpenLootState {
                    target_guid: Some(75),
                },
                ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(CMSG_AUTOSTORE_LOOT_ITEM {
                    item_slot: 3,
                }),
            )
            .err()
            .expect("fatal item-take failure was handled");

            assert_eq!(error.to_string(), message);
        }
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
    fn two_viewers_of_one_creature_receive_their_own_visible_items() {
        let mut store = InMemoryLootWindow::default();
        store.items_by_viewer.insert(42, vec![(0, 6948, 1, 100)]);
        store.items_by_viewer.insert(43, vec![(2, 2589, 5, 200)]);

        let first = dispatch_loot_window(
            &store,
            player(),
            OpenLootState::default(),
            open_creature(60),
        )
        .unwrap();
        let second = dispatch_loot_window(
            &store,
            LootWindowPlayer {
                account_id: 8,
                self_guid: Some(43),
            },
            OpenLootState::default(),
            open_creature(60),
        )
        .unwrap();

        let LootWindowOutcome::Handled {
            outbound: first_outbound,
            ..
        } = first
        else {
            panic!("first creature open passed through")
        };
        let LootWindowOutcome::Handled {
            outbound: second_outbound,
            ..
        } = second
        else {
            panic!("second creature open passed through")
        };
        let [Outbound::Raw {
            body: first_body, ..
        }] = first_outbound.as_slice()
        else {
            panic!("expected the first viewer's loot window")
        };
        let [Outbound::Raw {
            body: second_body, ..
        }] = second_outbound.as_slice()
        else {
            panic!("expected the second viewer's loot window")
        };
        assert_eq!(&first_body[15..19], &6948u32.to_le_bytes());
        assert_eq!(&second_body[15..19], &2589u32.to_le_bytes());
        assert_eq!(
            store.item_reads.lock().unwrap().as_slice(),
            &[(60, 42), (60, 43)]
        );
    }

    #[test]
    fn release_clears_the_open_target_and_acknowledges_the_request_target() {
        let next_state = dispatch_release(
            OpenLootState {
                target_guid: Some(60),
            },
            91,
        );
        assert_eq!(next_state, OpenLootState::default());
    }

    #[test]
    fn duplicate_release_keeps_state_empty_and_is_acknowledged() {
        let next_state = dispatch_release(
            OpenLootState {
                target_guid: Some(60),
            },
            60,
        );
        let next_state = dispatch_release(next_state, 60);

        assert_eq!(next_state, OpenLootState::default());
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
            skin_refusal: Some("target is not skinnable".into()),
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
                skin_fatal_error: Some(message.into()),
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
