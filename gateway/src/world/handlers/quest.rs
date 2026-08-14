//! Quest family: the overhead `!`/`?` status and the quest-giver menu enter through
//! `dispatch_quest_action`; the gameobject giver calls the shared menu builder here rather than
//! reaching for the store. `handle_quest` still owns the dialog opcodes that have not moved yet.

use super::super::*;

/// Durable reads the quest family needs, in the seam's own vocabulary so it can be exercised
/// without the broad `WorldStore`.
pub(crate) trait QuestActionStore: Send + Sync {
    /// Every quest `giver_guid` offers or completes for `player_guid` — the input to both the
    /// overhead status icon and the menu. Resolves a creature giver or a gameobject giver.
    fn giver_quest_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>>;

    /// A quest's detail view (the details / offer-reward / request-items screens). `None` if the
    /// quest is not loaded.
    fn quest_detail_view(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>>;

    /// Whether standing (Unfriendly or below) makes this giver refuse to talk.
    fn giver_refuses_interaction(&self, giver_guid: u64, player_guid: u64) -> Result<bool>;
}

impl QuestActionStore for crate::stdb::Coordinator {
    fn giver_quest_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>> {
        crate::stdb::Coordinator::quest_giver_evals(self, giver_guid, player_guid)
    }

    fn quest_detail_view(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
        crate::stdb::Coordinator::quest_detail(self, quest_id)
    }

    fn giver_refuses_interaction(&self, giver_guid: u64, player_guid: u64) -> Result<bool> {
        crate::stdb::Coordinator::npc_refuses_interaction(self, giver_guid, player_guid)
    }
}

/// Who is asking. `self_guid` is `None` before world entry — a questgiver can only be clicked
/// in-world, so those opcodes pass through instead of being evaluated against a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuestActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum QuestActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuestActionErrorClass {
    GameplayRefusal,
    Fatal,
}

fn classify_quest_action_error(error: &anyhow::Error) -> QuestActionErrorClass {
    if error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
    {
        QuestActionErrorClass::Fatal
    } else {
        QuestActionErrorClass::GameplayRefusal
    }
}

/// The quest menu for `giver` (a creature OR a gameobject guid — the evals resolve either) against
/// `self_guid`: vanilla "instant quest" (mangos `SendPreparedQuest`) opens a SINGLE menu-worthy
/// quest's screen directly — accept details for a new quest, the reward screen for a finished
/// turn-in, the "not done yet" request-items screen for one in progress — while a giver with several
/// shows the list. `CMSG_QUESTGIVER_HELLO` (creature giver) and `CMSG_GAMEOBJ_USE` on a
/// `go_type::QUESTGIVER` gameobject (the client never sends HELLO for one) both land here, so the
/// two interactions cannot drift apart. The evals are read ONCE and reused for the whole decision.
pub(crate) fn quest_giver_menu<St: QuestActionStore + ?Sized>(
    store: &St,
    giver: u64,
    self_guid: u64,
) -> Result<Vec<Outbound>> {
    let evals = store.giver_quest_evals(giver, self_guid)?;
    let menu = codec::quest_menu_items(&evals);
    let single = if menu.len() == 1 {
        store.quest_detail_view(menu[0].quest_id)?
    } else {
        None
    };
    let Some(detail) = single else {
        return Ok(vec![Outbound::One(
            ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(Box::new(codec::build_quest_list(
                giver,
                "Greetings.",
                &evals,
            ))),
        )]);
    };
    let turn_in = evals
        .iter()
        .find(|e| e.quest_id == detail.quest_id && e.role == codec::ROLE_END && e.active);
    Ok(match turn_in {
        // A new quest: DETAILS is raw-encoded because its 1.12 reward triples are incomplete in
        // gtker. The turn-in screens below are typed.
        None => {
            let (opcode, body) = codec::build_quest_details_raw(giver, &detail);
            vec![Outbound::Raw { opcode, body }]
        }
        Some(e) if e.complete => vec![Outbound::One(
            ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(codec::build_offer_reward(
                giver, &detail,
            ))),
        )],
        Some(_) => vec![Outbound::One(
            ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                codec::build_request_items(giver, &detail, false),
            )),
        )],
    })
}

/// The quest opcodes that own their whole protocol round trip. Anything else — and anything at all
/// before world entry — passes through to `handle_quest`.
pub(crate) fn dispatch_quest_action<St: QuestActionStore + ?Sized>(
    store: &St,
    player: QuestActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<QuestActionOutcome> {
    let Some(self_guid) = player.self_guid else {
        return Ok(QuestActionOutcome::PassThrough(msg));
    };
    match msg {
        // The client polls each nearby questgiver for its overhead icon (`!` available / `?` turn-in).
        ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(q) => {
            let giver = q.guid.guid();
            let status = codec::quest_giver_status(&store.giver_quest_evals(giver, self_guid)?);
            Ok(QuestActionOutcome::Handled {
                outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(
                    Box::new(codec::build_questgiver_status(giver, status)),
                ))],
            })
        }
        // Right-click a questgiver → the quest menu (every quest it offers/completes for this
        // player). An Unfriendly-or-below giver refuses it and answers nothing at all.
        ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(h) => {
            let giver = h.guid.guid();
            let refuses = match store.giver_refuses_interaction(giver, self_guid) {
                Ok(refuses) => refuses,
                // The gate fails open — missing standing data must not lock a player out of a
                // giver — but a dead reducer transport is not missing data and ends the session.
                Err(e)
                    if classify_quest_action_error(&e) == QuestActionErrorClass::GameplayRefusal =>
                {
                    log::debug!(
                        "world: questgiver {giver} interaction gate unavailable (account {}): {e}",
                        player.account_id
                    );
                    false
                }
                Err(e) => return Err(e),
            };
            Ok(QuestActionOutcome::Handled {
                outbound: if refuses {
                    Vec::new()
                } else {
                    quest_giver_menu(store, giver, self_guid)?
                },
            })
        }
        other => Ok(QuestActionOutcome::PassThrough(other)),
    }
}

/// The quest dialog opcodes that still read the broad `WorldStore`: the quest details + accept, the
/// turn-in offer/complete round-trip, abandon, the client's quest-definition query, and party
/// sharing. Reads are evaluated against the player, so these need the in-world player guid — in
/// CharSelect the opcodes pass through. Reducer rejections (accept/turn-in gates) are per-action:
/// logged, not fatal.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        Guid, QuestGiverStatus, CMSG_PING, CMSG_QUESTGIVER_HELLO, CMSG_QUESTGIVER_STATUS_QUERY,
    };

    #[derive(Default)]
    struct InMemoryQuestActions {
        eval_requests: Mutex<Vec<(u64, u64)>>,
        detail_requests: Mutex<Vec<u32>>,
        gate_requests: Mutex<Vec<(u64, u64)>>,
        evals: Vec<codec::GiverQuestEval>,
        details: Vec<codec::QuestDetailView>,
        refuses: bool,
        gate_error: Option<String>,
    }

    impl QuestActionStore for InMemoryQuestActions {
        fn giver_quest_evals(
            &self,
            giver_guid: u64,
            player_guid: u64,
        ) -> Result<Vec<codec::GiverQuestEval>> {
            self.eval_requests
                .lock()
                .unwrap()
                .push((giver_guid, player_guid));
            Ok(self.evals.clone())
        }

        fn quest_detail_view(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
            self.detail_requests.lock().unwrap().push(quest_id);
            Ok(self
                .details
                .iter()
                .find(|d| d.quest_id == quest_id)
                .cloned())
        }

        fn giver_refuses_interaction(&self, giver_guid: u64, player_guid: u64) -> Result<bool> {
            self.gate_requests
                .lock()
                .unwrap()
                .push((giver_guid, player_guid));
            match &self.gate_error {
                Some(error) => Err(anyhow::anyhow!("{error}")),
                None => Ok(self.refuses),
            }
        }
    }

    const GIVER: u64 = 0xF130_0000_0000_0050;
    const GO_GIVER: u64 = 0xF110_0000_0000_0044;
    const QUEST: u32 = 1234;
    const SELF_GUID: u64 = 42;
    const OP_QUEST_DETAILS: u16 = 0x0188;

    fn player() -> QuestActionPlayer {
        QuestActionPlayer {
            account_id: 7,
            self_guid: Some(SELF_GUID),
        }
    }

    fn status_query(giver: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(CMSG_QUESTGIVER_STATUS_QUERY {
            guid: Guid::new(giver),
        })
    }

    fn hello(giver: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(CMSG_QUESTGIVER_HELLO {
            guid: Guid::new(giver),
        })
    }

    /// One giver↔quest relation. `startable` follows the role, the way the module evaluates it.
    fn eval(quest_id: u32, role: u8, active: bool, complete: bool) -> codec::GiverQuestEval {
        codec::GiverQuestEval {
            quest_id,
            title: "A Threat Within".into(),
            level: 1,
            role,
            startable: role == codec::ROLE_START,
            active,
            complete,
        }
    }

    fn detail(quest_id: u32) -> codec::QuestDetailView {
        codec::QuestDetailView {
            quest_id,
            quest_level: 1,
            zone_or_sort: 12,
            title: "A Threat Within".into(),
            details: String::new(),
            objectives_text: String::new(),
            offer_reward_text: String::new(),
            request_items_text: String::new(),
            money_reward: 0,
            reward_xp: 0,
            next_quest_id: 0,
            max_level_money_reward: 0,
            rewards: Vec::new(),
            choice_rewards: Vec::new(),
            objectives: Vec::new(),
        }
    }

    /// A giver with exactly one menu-worthy quest whose details are loaded — the "instant quest"
    /// fixture the three single-quest screens differ only by eval state on.
    fn one_quest(role: u8, active: bool, complete: bool) -> InMemoryQuestActions {
        InMemoryQuestActions {
            evals: vec![eval(QUEST, role, active, complete)],
            details: vec![detail(QUEST)],
            ..Default::default()
        }
    }

    fn outbound(outcome: QuestActionOutcome) -> Vec<Outbound> {
        match outcome {
            QuestActionOutcome::Handled { outbound } => outbound,
            QuestActionOutcome::PassThrough(_) => panic!("expected the quest module to handle this"),
        }
    }

    /// The same client-visible traffic: same order, same messages, same raw bytes.
    fn same_batch(a: &[Outbound], b: &[Outbound]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|pair| match pair {
                (Outbound::One(a), Outbound::One(b)) => a == b,
                (
                    Outbound::Raw {
                        opcode: a_op,
                        body: a_body,
                    },
                    Outbound::Raw {
                        opcode: b_op,
                        body: b_body,
                    },
                ) => a_op == b_op && a_body == b_body,
                _ => false,
            })
    }

    // ── Overhead status ──────────────────────────────────────────────────────

    #[test]
    fn a_status_query_answers_one_status_for_the_polled_giver() {
        let actions = InMemoryQuestActions {
            evals: vec![eval(QUEST, codec::ROLE_START, false, false)],
            ..Default::default()
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), status_query(GIVER)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(s))]
                if s.guid == Guid::new(GIVER) && s.status == QuestGiverStatus::Available
        ));
        assert_eq!(
            actions.eval_requests.lock().unwrap().as_slice(),
            &[(GIVER, SELF_GUID)]
        );
    }

    // ── The giver menu ───────────────────────────────────────────────────────

    #[test]
    fn a_refusing_giver_answers_nothing_and_its_quests_are_never_read() {
        let actions = InMemoryQuestActions {
            refuses: true,
            ..one_quest(codec::ROLE_START, false, false)
        };

        assert!(outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap()).is_empty());
        assert_eq!(
            actions.gate_requests.lock().unwrap().as_slice(),
            &[(GIVER, SELF_GUID)]
        );
        assert!(actions.eval_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unavailable_interaction_gate_still_opens_the_menu() {
        let actions = InMemoryQuestActions {
            gate_error: Some("no reputation row for that faction".into()),
            ..one_quest(codec::ROLE_START, false, false)
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        assert!(matches!(batch.as_slice(), [Outbound::Raw { .. }]));
    }

    #[test]
    fn a_single_new_quest_opens_its_details_screen_directly() {
        let actions = one_quest(codec::ROLE_START, false, false);

        let batch = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::Raw { opcode: OP_QUEST_DETAILS, body }]
                if body[..8] == GIVER.to_le_bytes() && body[8..12] == QUEST.to_le_bytes()
        ));
    }

    #[test]
    fn a_single_complete_turn_in_opens_the_offer_reward_screen() {
        let actions = one_quest(codec::ROLE_END, true, true);

        let batch = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(r))]
                if r.npc == Guid::new(GIVER) && r.quest_id == QUEST
        ));
    }

    #[test]
    fn a_single_incomplete_turn_in_opens_the_request_items_screen() {
        let actions = one_quest(codec::ROLE_END, true, false);

        let batch = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(r))]
                if r.npc == Guid::new(GIVER) && r.quest_id == QUEST
        ));
    }

    #[test]
    fn a_giver_with_several_menu_quests_opens_the_list() {
        let actions = InMemoryQuestActions {
            evals: vec![
                eval(QUEST, codec::ROLE_START, false, false),
                eval(QUEST + 1, codec::ROLE_START, false, false),
            ],
            details: vec![detail(QUEST), detail(QUEST + 1)],
            ..Default::default()
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(l))]
                if l.npc == Guid::new(GIVER) && l.quest_items.len() == 2
        ));
        // No single quest to open, so no details screen was even considered.
        assert!(actions.detail_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_giver_with_no_menu_quests_opens_an_empty_list() {
        let actions = InMemoryQuestActions::default();

        let batch = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(l))]
                if l.quest_items.is_empty()
        ));
        assert!(actions.detail_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn the_menu_evaluates_the_giver_once_for_the_whole_screen_decision() {
        // The screen choice needs both the menu list and the turn-in state; reading the giver twice
        // would let those two answers disagree inside one request.
        let actions = one_quest(codec::ROLE_END, true, true);

        dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap();

        assert_eq!(
            actions.eval_requests.lock().unwrap().as_slice(),
            &[(GIVER, SELF_GUID)]
        );
        assert_eq!(actions.detail_requests.lock().unwrap().as_slice(), &[QUEST]);
    }

    #[test]
    fn a_gameobject_giver_opens_the_same_screen_as_a_creature_giver() {
        // The client never sends HELLO for a gameobject giver, so `handle_loot` calls the menu
        // directly. Same state, same screen — only the giver guid in the body differs.
        let actions = one_quest(codec::ROLE_START, false, false);

        let creature = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());
        let gameobject = quest_giver_menu(&actions, GIVER, SELF_GUID).unwrap();
        let other_giver = quest_giver_menu(&actions, GO_GIVER, SELF_GUID).unwrap();

        assert!(same_batch(&creature, &gameobject));
        assert!(matches!(
            other_giver.as_slice(),
            [Outbound::Raw { opcode: OP_QUEST_DETAILS, body }] if body[..8] == GO_GIVER.to_le_bytes()
        ));
    }

    // ── Player context and error classification ──────────────────────────────

    #[test]
    fn before_world_entry_the_quest_opcodes_pass_through() {
        let actions = one_quest(codec::ROLE_START, false, false);
        let player = QuestActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        for msg in [status_query(GIVER), hello(GIVER)] {
            assert!(matches!(
                dispatch_quest_action(&actions, player, msg).unwrap(),
                QuestActionOutcome::PassThrough(_)
            ));
        }
        assert!(actions.eval_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn reducer_transport_failure_is_session_fatal() {
        let actions = InMemoryQuestActions {
            gate_error: Some(
                "npc_refuses_interaction reducer transport disconnected: channel closed".into(),
            ),
            ..one_quest(codec::ROLE_START, false, false)
        };

        let error = match dispatch_quest_action(&actions, player(), hello(GIVER)) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    // ── Pass-through ─────────────────────────────────────────────────────────

    #[test]
    fn unrelated_opcodes_pass_through_to_the_next_dispatcher() {
        let actions = InMemoryQuestActions::default();

        let outcome = dispatch_quest_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            QuestActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }
}
