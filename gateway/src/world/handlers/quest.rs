//! Quest family: the overhead `!`/`?` status, the quest-giver menu, the details and definition
//! screens, accept, the turn-in round trip, log abandon and party sharing all enter through
//! `dispatch_quest_action`; the gameobject giver, the item-started quest, the world-entry
//! descriptor block and the gossip quest section call the shared builders here rather than
//! reaching for the store. Every quest read and reducer the world session needs lives on
//! `QuestActionStore`; `WorldStore` carries none of them. The stdb-tier relays in
//! `subscriptions.rs` (quest-log sync, the shared-quest details screen) sit below the session and
//! render their own copies from the same `codec` builders these functions use.

use super::super::*;
use wow_world_messages::vanilla::QuestItem;

/// The durable reads and reducer calls the quest family needs, in the seam's own vocabulary so it
/// can be exercised without the broad `WorldStore`.
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

    /// Open the quest log row for `quest_id` offered by `giver_guid`. The module gates it, so a
    /// refusal here is a gameplay answer, not a broken session.
    fn accept_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()>;

    /// The quest the item in `slot` starts, as `(item instance guid, quest id)`. `None` when the
    /// item starts no quest — an item is its own quest giver, hence the instance guid.
    fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)>;

    /// Hand a completed quest in to `giver_guid` for its rewards. The module validates completion
    /// and grants money/XP/items; `reward_index` is the player's pick-1-of-N choice reward slot,
    /// ignored by quests with no choice rewards.
    fn turn_in_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()>;

    /// The player's active quests as quest-log descriptor slots (the L window), in slot order.
    /// Empty if none. The one read behind both abandon's slot→quest resolution and the world-entry
    /// descriptor block, so a slot means the same quest in both.
    fn player_quest_log(&self, player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>>;

    /// Abandon an active quest (`CMSG_QUESTLOG_REMOVE_QUEST`). The module deletes the quest-log row;
    /// the relay clears the slot.
    fn abandon_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()>;

    /// Share `quest_id` with the caller's party (`CMSG_PUSHQUESTTOPARTY`). The module validates
    /// grouped + actively-on-the-quest and pushes the per-member `QUEST_SHARE`/`QUEST_PUSH_RESULT`
    /// events itself; a gameplay `Err` is per-action, not session-fatal.
    fn push_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()>;

    /// `(taken, rewarded)` for `quest_id` in `player_guid`'s quest log — feeds the
    /// QUEST_TAKEN/QUEST_REWARDED gossip option conditions.
    fn quest_status(&self, player_guid: u64, quest_id: u32) -> (bool, bool);
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

    fn accept_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()> {
        crate::stdb::Coordinator::accept_quest(self, account_id, self_guid, giver_guid, quest_id)
    }

    fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)> {
        crate::stdb::Coordinator::item_start_quest(self, owner_guid, slot)
    }

    fn turn_in_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        crate::stdb::Coordinator::turn_in_quest(
            self,
            account_id,
            self_guid,
            giver_guid,
            quest_id,
            reward_index,
        )
    }

    fn player_quest_log(&self, player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>> {
        crate::stdb::Coordinator::player_quest_log(self, player_guid)
    }

    fn abandon_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()> {
        crate::stdb::Coordinator::abandon_quest(self, account_id, self_guid, quest_id)
    }

    fn push_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()> {
        crate::stdb::Coordinator::push_quest(self, account_id, self_guid, quest_id)
    }

    fn quest_status(&self, player_guid: u64, quest_id: u32) -> (bool, bool) {
        crate::stdb::Coordinator::quest_status(self, player_guid, quest_id)
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

/// The details + Accept screen for one quest, offered by `giver`. Clicking a quest in a giver's
/// menu and using an item that starts a quest both land here, so the two routes cannot render
/// different screens for the same quest. A quest whose definition is not loaded renders nothing.
/// RAW-encoded for the same reason the menu is: gtker's 1.12 reward triples are incomplete.
pub(crate) fn quest_details_screen<St: QuestActionStore + ?Sized>(
    store: &St,
    giver: u64,
    quest_id: u32,
) -> Result<Vec<Outbound>> {
    Ok(store
        .quest_detail_view(quest_id)?
        .map_or_else(Vec::new, |detail| {
            let (opcode, body) = codec::build_quest_details_raw(giver, &detail);
            vec![Outbound::Raw { opcode, body }]
        }))
}

/// The screen a `CMSG_USE_ITEM` opens when that item starts a quest — the item instance is its own
/// giver. `None` means the item starts no quest, which is the item family's signal to fall through
/// to the ordinary use path; `Some` means the quest module consumed the action, so the item is
/// never used up by opening its own quest.
pub(crate) fn item_started_quest<St: QuestActionStore + ?Sized>(
    store: &St,
    player: QuestActionPlayer,
    slot: u8,
) -> Result<Option<Vec<Outbound>>> {
    let Some(self_guid) = player.self_guid else {
        return Ok(None);
    };
    let Some((item_guid, quest_id)) = store.item_start_quest(self_guid, slot) else {
        return Ok(None);
    };
    Ok(Some(quest_details_screen(store, item_guid, quest_id)?))
}

/// The raw quest-log descriptor VALUES update for `player_guid` — the world-entry (login) copy of
/// the block. The in-session relay renders its own copy in `stdb::subscriptions`'s
/// `quest_log_sync`, off the same `build_quest_log_slots` read and the same
/// `full_quest_log_mask` encoding, so the two cannot describe a slot differently. The one
/// deliberate difference is here: an empty log answers an EMPTY batch, because the client's
/// descriptor fields start zeroed at world entry; the relay always sends, since an all-zero mask
/// is how a turned-in quest's slot gets cleared mid-session.
pub(crate) fn quest_log_update<St: QuestActionStore + ?Sized>(
    store: &St,
    player_guid: u64,
) -> Result<Vec<Outbound>> {
    let slots = store.player_quest_log(player_guid)?;
    if slots.is_empty() {
        return Ok(Vec::new());
    }
    let mask = codec::update_mask::full_quest_log_mask(&slots);
    let (opcode, body) = codec::build_values_update_raw(player_guid, &mask);
    Ok(vec![Outbound::Raw { opcode, body }])
}

/// The quest section of a combined gossip menu for `npc` against `self_guid` — the same evaluation
/// and the same menu-item derivation `quest_giver_menu` uses, so a gossip-flagged questgiver can
/// never show different quest icons than `CMSG_QUESTGIVER_HELLO` would.
pub(crate) fn gossip_quest_items<St: QuestActionStore + ?Sized>(
    store: &St,
    npc: u64,
    self_guid: u64,
) -> Result<Vec<QuestItem>> {
    Ok(codec::quest_menu_items(
        &store.giver_quest_evals(npc, self_guid)?,
    ))
}

/// `(taken, rewarded)` for `quest_id` in `player_guid`'s quest log — the read behind the
/// QUEST_TAKEN/QUEST_REWARDED gossip option conditions that gate a gossip menu row.
pub(crate) fn quest_gate_state<St: QuestActionStore + ?Sized>(
    store: &St,
    player_guid: u64,
    quest_id: u32,
) -> (bool, bool) {
    store.quest_status(player_guid, quest_id)
}

/// The quest opcodes that own their whole protocol round trip. Anything else — and anything at all
/// before world entry — passes through to the next family in the dispatch chain.
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
        // Clicked a quest in the menu → its details + Accept button.
        ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(q) => Ok(QuestActionOutcome::Handled {
            outbound: quest_details_screen(store, q.guid.guid(), q.quest_id)?,
        }),
        // The client asks for a quest's full definition (it sends this for any quest id it sees in a
        // PLAYER_QUEST_LOG slot but has no data for). Without this reply the client won't
        // display/count the quest in its log — so this is what makes the quest-log window entry
        // actually appear. RAW-encoded: gtker's typed layout writes the rep Faction fields as u16,
        // shifting the title by 4 bytes; the hand-rolled body matches the 5875 layout exactly.
        ClientOpcodeMessage::CMSG_QUEST_QUERY(q) => {
            let outbound = store
                .quest_detail_view(q.quest_id)?
                .map_or_else(Vec::new, |detail| {
                    let (opcode, body) = codec::build_quest_query_response_raw(&detail);
                    vec![Outbound::Raw { opcode, body }]
                });
            Ok(QuestActionOutcome::Handled { outbound })
        }
        // Clicked Accept → the module opens the quest log row (gated). No SMSG on success: the
        // client closes the window itself and the quest-log relay carries the new slot.
        ClientOpcodeMessage::CMSG_QUESTGIVER_ACCEPT_QUEST(a) => {
            match store.accept_quest(player.account_id, self_guid, a.guid.guid(), a.quest_id) {
                Ok(()) => {}
                Err(e)
                    if classify_quest_action_error(&e) == QuestActionErrorClass::GameplayRefusal =>
                {
                    log::debug!(
                        "world: accept_quest ignored (account {}): {e}",
                        player.account_id
                    );
                }
                Err(e) => return Err(e),
            }
            Ok(QuestActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        // Opened a turn-in (clicked the `?`): the offer-reward screen when the giver's current
        // evaluation reports the quest complete, else the request-items "not finished" screen. The
        // module is the authority on completion; this only picks the screen and grants nothing.
        ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(c) => {
            let giver = c.guid.guid();
            let Some(detail) = store.quest_detail_view(c.quest_id)? else {
                return Ok(QuestActionOutcome::Handled {
                    outbound: Vec::new(),
                });
            };
            let complete = store
                .giver_quest_evals(giver, self_guid)?
                .iter()
                .any(|e| e.quest_id == c.quest_id && e.role == codec::ROLE_END && e.complete);
            let screen = if complete {
                ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
                    codec::build_offer_reward(giver, &detail),
                ))
            } else {
                ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                    codec::build_request_items(giver, &detail, false),
                ))
            };
            Ok(QuestActionOutcome::Handled {
                outbound: vec![Outbound::One(screen)],
            })
        }
        // Chose the reward → the module grants money/XP/items (gated on completion). The durable
        // turn-in is requested BEFORE any outbound is built, so a refused turn-in can never show a
        // "Quest Complete" popup for rewards the player did not get.
        ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(c) => {
            match store.turn_in_quest(
                player.account_id,
                self_guid,
                c.guid.guid(),
                c.quest_id,
                c.reward,
            ) {
                // The popup echoes the definition's XP/money/items, so what it shows matches what
                // the module granted. Unreadable details drop it — the turn-in already happened.
                Ok(()) => Ok(QuestActionOutcome::Handled {
                    outbound: match store.quest_detail_view(c.quest_id)? {
                        Some(detail) => vec![Outbound::One(
                            ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(Box::new(
                                codec::build_quest_complete(&detail),
                            )),
                        )],
                        None => Vec::new(),
                    },
                }),
                Err(e)
                    if classify_quest_action_error(&e) == QuestActionErrorClass::GameplayRefusal =>
                {
                    log::debug!(
                        "world: turn_in_quest ignored (account {}): {e}",
                        player.account_id
                    );
                    Ok(QuestActionOutcome::Handled {
                        outbound: Vec::new(),
                    })
                }
                Err(e) => Err(e),
            }
        }
        // Abandon a quest from the log ("Abandon Quest"). The payload is a LOG SLOT (0..19), not a
        // quest id — resolve it against the same `player_quest_log` ordering the world-entry block
        // reads, then request the durable abandon. A slot that is not currently in the log (stale
        // window, typo'd click) resolves to nothing and requests nothing — it cannot abandon an
        // arbitrary quest. No SMSG on success: the quest-log relay re-sends the cleared block.
        ClientOpcodeMessage::CMSG_QUESTLOG_REMOVE_QUEST(r) => {
            if let Some(s) = store
                .player_quest_log(self_guid)?
                .into_iter()
                .find(|s| s.slot == r.slot)
            {
                match store.abandon_quest(player.account_id, self_guid, s.quest_id) {
                    Ok(()) => {}
                    Err(e)
                        if classify_quest_action_error(&e)
                            == QuestActionErrorClass::GameplayRefusal =>
                    {
                        log::debug!(
                            "world: abandon_quest ignored (account {}): {e}",
                            player.account_id
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(QuestActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        // Share a quest with the party (`CMSG_PUSHQUESTTOPARTY`). The module validates
        // grouped + actively-on-the-quest and pushes the per-member `QUEST_SHARE`/`QUEST_PUSH_RESULT`
        // events itself (relayed by `subscriptions.rs`'s `on_group_event`); no direct SMSG here.
        ClientOpcodeMessage::CMSG_PUSHQUESTTOPARTY(p) => {
            match store.push_quest(player.account_id, self_guid, p.quest_id) {
                Ok(()) => {}
                Err(e)
                    if classify_quest_action_error(&e) == QuestActionErrorClass::GameplayRefusal =>
                {
                    log::debug!(
                        "world: push_quest ignored (account {}): {e}",
                        player.account_id
                    );
                }
                Err(e) => return Err(e),
            }
            Ok(QuestActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
        other => Ok(QuestActionOutcome::PassThrough(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        Guid, QuestGiverStatus, CMSG_PING, CMSG_PUSHQUESTTOPARTY, CMSG_QUESTGIVER_ACCEPT_QUEST,
        CMSG_QUESTGIVER_CHOOSE_REWARD, CMSG_QUESTGIVER_COMPLETE_QUEST, CMSG_QUESTGIVER_HELLO,
        CMSG_QUESTGIVER_QUERY_QUEST, CMSG_QUESTGIVER_STATUS_QUERY, CMSG_QUESTLOG_REMOVE_QUEST,
        CMSG_QUEST_QUERY,
    };

    /// One durable call of the turn-in round trip, recorded in the order the seam made it. The
    /// completion popup is built from `Detail`, so a `TurnIn` recorded ahead of it is the proof
    /// that no screen can claim completion before the module granted it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TurnInCall {
        TurnIn {
            account_id: u64,
            self_guid: u64,
            giver: u64,
            quest_id: u32,
            reward_index: u32,
        },
        Detail(u32),
    }

    #[derive(Default)]
    struct InMemoryQuestActions {
        eval_requests: Mutex<Vec<(u64, u64)>>,
        detail_requests: Mutex<Vec<u32>>,
        gate_requests: Mutex<Vec<(u64, u64)>>,
        accept_requests: Mutex<Vec<(u64, u64, u64, u32)>>,
        start_quest_requests: Mutex<Vec<(u64, u8)>>,
        turn_in_calls: Mutex<Vec<TurnInCall>>,
        quest_log_requests: Mutex<Vec<u64>>,
        abandon_requests: Mutex<Vec<(u64, u64, u32)>>,
        push_requests: Mutex<Vec<(u64, u64, u32)>>,
        status_requests: Mutex<Vec<(u64, u32)>>,
        evals: Vec<codec::GiverQuestEval>,
        details: Vec<codec::QuestDetailView>,
        refuses: bool,
        gate_error: Option<String>,
        accept_error: Option<String>,
        start_quest: Option<(u64, u32)>,
        turn_in_error: Option<String>,
        quest_log: Vec<codec::update_mask::QuestLogSlot>,
        abandon_error: Option<String>,
        push_error: Option<String>,
        quest_status: (bool, bool),
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
            self.turn_in_calls
                .lock()
                .unwrap()
                .push(TurnInCall::Detail(quest_id));
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

        fn accept_quest(
            &self,
            account_id: u64,
            self_guid: u64,
            giver_guid: u64,
            quest_id: u32,
        ) -> Result<()> {
            self.accept_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, giver_guid, quest_id));
            self.accept_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)> {
            self.start_quest_requests
                .lock()
                .unwrap()
                .push((owner_guid, slot));
            self.start_quest
        }

        fn turn_in_quest(
            &self,
            account_id: u64,
            self_guid: u64,
            giver_guid: u64,
            quest_id: u32,
            reward_index: u32,
        ) -> Result<()> {
            self.turn_in_calls.lock().unwrap().push(TurnInCall::TurnIn {
                account_id,
                self_guid,
                giver: giver_guid,
                quest_id,
                reward_index,
            });
            self.turn_in_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn player_quest_log(
            &self,
            player_guid: u64,
        ) -> Result<Vec<codec::update_mask::QuestLogSlot>> {
            self.quest_log_requests.lock().unwrap().push(player_guid);
            Ok(self.quest_log.clone())
        }

        fn abandon_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()> {
            self.abandon_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, quest_id));
            self.abandon_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn push_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()> {
            self.push_requests
                .lock()
                .unwrap()
                .push((account_id, self_guid, quest_id));
            self.push_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn quest_status(&self, player_guid: u64, quest_id: u32) -> (bool, bool) {
            self.status_requests
                .lock()
                .unwrap()
                .push((player_guid, quest_id));
            self.quest_status
        }
    }

    const GIVER: u64 = 0xF130_0000_0000_0050;
    const GO_GIVER: u64 = 0xF110_0000_0000_0044;
    const ITEM_GIVER: u64 = 0x4000_0000_0000_0099;
    const QUEST: u32 = 1234;
    const SELF_GUID: u64 = 42;
    const BAG_SLOT: u8 = 5;
    const OP_QUEST_DETAILS: u16 = 0x0188;
    const OP_QUEST_QUERY_RESPONSE: u16 = 0x005D;

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

    fn query_quest(giver: u64, quest_id: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(Box::new(CMSG_QUESTGIVER_QUERY_QUEST {
            guid: Guid::new(giver),
            quest_id,
        }))
    }

    fn quest_query(quest_id: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUEST_QUERY(CMSG_QUEST_QUERY { quest_id })
    }

    fn accept(giver: u64, quest_id: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTGIVER_ACCEPT_QUEST(Box::new(CMSG_QUESTGIVER_ACCEPT_QUEST {
            guid: Guid::new(giver),
            quest_id,
        }))
    }

    fn abandon(slot: u8) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTLOG_REMOVE_QUEST(CMSG_QUESTLOG_REMOVE_QUEST { slot })
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

    /// One quest-log slot. `counts`/`state`/`timer` are irrelevant to slot resolution, so every
    /// fixture leaves them at their zero value.
    fn log_slot(slot: u8, quest_id: u32) -> codec::update_mask::QuestLogSlot {
        codec::update_mask::QuestLogSlot {
            slot,
            quest_id,
            counts: Vec::new(),
            state: 0,
            timer: 0,
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

    /// A store where only `QUEST` has a loaded definition — the fixture the details, definition
    /// and accept routes differ on by the quest id they ask for.
    fn loaded_quest() -> InMemoryQuestActions {
        InMemoryQuestActions {
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

    // ── The details screen and the definition query ──────────────────────────

    #[test]
    fn a_details_request_opens_the_whole_screen_for_the_asking_giver() {
        let actions = loaded_quest();

        let batch =
            outbound(dispatch_quest_action(&actions, player(), query_quest(GIVER, QUEST)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::Raw { opcode: OP_QUEST_DETAILS, body }]
                if *body == codec::build_quest_details_raw(GIVER, &detail(QUEST)).1
        ));
    }

    #[test]
    fn a_details_request_for_an_unloaded_quest_answers_nothing() {
        let actions = loaded_quest();

        let batch = outbound(
            dispatch_quest_action(&actions, player(), query_quest(GIVER, QUEST + 1)).unwrap(),
        );

        assert!(batch.is_empty());
    }

    #[test]
    fn a_definition_query_answers_the_raw_vanilla_definition() {
        let actions = loaded_quest();

        let batch =
            outbound(dispatch_quest_action(&actions, player(), quest_query(QUEST)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::Raw { opcode: OP_QUEST_QUERY_RESPONSE, body }]
                if *body == codec::build_quest_query_response_raw(&detail(QUEST)).1
        ));
    }

    #[test]
    fn a_definition_query_for_an_unloaded_quest_answers_nothing() {
        let actions = loaded_quest();

        let batch =
            outbound(dispatch_quest_action(&actions, player(), quest_query(QUEST + 1)).unwrap());

        assert!(batch.is_empty());
    }

    // ── Accept ───────────────────────────────────────────────────────────────

    #[test]
    fn accept_requests_the_durable_accept_for_the_account_player_giver_and_quest() {
        let actions = loaded_quest();

        let batch =
            outbound(dispatch_quest_action(&actions, player(), accept(GIVER, QUEST)).unwrap());

        assert!(batch.is_empty(), "the client closes the window itself");
        assert_eq!(
            actions.accept_requests.lock().unwrap().as_slice(),
            &[(7, SELF_GUID, GIVER, QUEST)]
        );
    }

    #[test]
    fn a_refused_accept_answers_nothing_and_does_not_end_the_session() {
        let actions = InMemoryQuestActions {
            accept_error: Some("quest requires level 10".into()),
            ..loaded_quest()
        };

        let batch =
            outbound(dispatch_quest_action(&actions, player(), accept(GIVER, QUEST)).unwrap());

        assert!(batch.is_empty());
    }

    #[test]
    fn a_dead_transport_on_accept_ends_the_session() {
        let actions = InMemoryQuestActions {
            accept_error: Some(
                "accept_quest reducer transport disconnected: channel closed".into(),
            ),
            ..loaded_quest()
        };

        let error = match dispatch_quest_action(&actions, player(), accept(GIVER, QUEST)) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    // ── The item-started quest ───────────────────────────────────────────────

    #[test]
    fn an_item_that_starts_a_quest_opens_its_details_with_the_item_as_the_giver() {
        let actions = InMemoryQuestActions {
            start_quest: Some((ITEM_GIVER, QUEST)),
            ..loaded_quest()
        };

        let batch = item_started_quest(&actions, player(), BAG_SLOT)
            .unwrap()
            .expect("the quest module owns a quest-starting item");

        assert!(matches!(
            batch.as_slice(),
            [Outbound::Raw { opcode: OP_QUEST_DETAILS, body }]
                if body[..8] == ITEM_GIVER.to_le_bytes() && body[8..12] == QUEST.to_le_bytes()
        ));
        assert_eq!(
            actions.start_quest_requests.lock().unwrap().as_slice(),
            &[(SELF_GUID, BAG_SLOT)]
        );
    }

    #[test]
    fn an_item_that_starts_no_quest_leaves_the_action_to_the_item_family() {
        let actions = loaded_quest();

        assert!(item_started_quest(&actions, player(), BAG_SLOT)
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_item_starting_an_unloaded_quest_still_belongs_to_the_quest_module() {
        // The empty batch is what keeps the item from being consumed: the item family only falls
        // through on `None`, and a missing definition is not a reason to eat the item.
        let actions = InMemoryQuestActions {
            start_quest: Some((ITEM_GIVER, QUEST + 1)),
            ..loaded_quest()
        };

        let batch = item_started_quest(&actions, player(), BAG_SLOT).unwrap();

        assert_eq!(batch.map(|batch| batch.len()), Some(0));
    }

    #[test]
    fn without_a_player_guid_no_item_is_asked_about_its_quest() {
        let actions = InMemoryQuestActions {
            start_quest: Some((ITEM_GIVER, QUEST)),
            ..loaded_quest()
        };
        let player = QuestActionPlayer {
            account_id: 7,
            self_guid: None,
        };

        assert!(item_started_quest(&actions, player, BAG_SLOT)
            .unwrap()
            .is_none());
        assert!(actions.start_quest_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn the_item_route_and_the_giver_route_render_the_same_details_screen() {
        let actions = InMemoryQuestActions {
            start_quest: Some((GIVER, QUEST)),
            ..loaded_quest()
        };

        let from_giver =
            outbound(dispatch_quest_action(&actions, player(), query_quest(GIVER, QUEST)).unwrap());
        let from_item = item_started_quest(&actions, player(), BAG_SLOT)
            .unwrap()
            .expect("the quest module owns a quest-starting item");

        assert!(same_batch(&from_giver, &from_item));
    }

    // ── The quest log: abandon and the world-entry descriptor block ──────────

    #[test]
    fn abandon_resolves_the_log_slot_to_a_quest_id_and_requests_the_durable_abandon() {
        let actions = InMemoryQuestActions {
            quest_log: vec![log_slot(3, 777)],
            ..Default::default()
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), abandon(3)).unwrap());

        assert!(
            batch.is_empty(),
            "the quest-log relay carries the cleared slot"
        );
        assert_eq!(
            actions.abandon_requests.lock().unwrap().as_slice(),
            &[(7, SELF_GUID, 777)]
        );
    }

    #[test]
    fn abandon_of_a_slot_not_in_the_log_requests_nothing() {
        // A stale window or a desynced client can send a slot the log doesn't currently hold — that
        // must resolve to nothing, so it can never abandon whatever quest a live slot happens to hold.
        let actions = InMemoryQuestActions {
            quest_log: vec![log_slot(3, 777)],
            ..Default::default()
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), abandon(9)).unwrap());

        assert!(batch.is_empty());
        assert!(actions.abandon_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn abandon_against_an_empty_log_requests_nothing() {
        let actions = InMemoryQuestActions::default();

        let batch = outbound(dispatch_quest_action(&actions, player(), abandon(0)).unwrap());

        assert!(batch.is_empty());
        assert!(actions.abandon_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_refused_abandon_answers_nothing_and_does_not_end_the_session() {
        let actions = InMemoryQuestActions {
            quest_log: vec![log_slot(3, 777)],
            abandon_error: Some("quest cannot be abandoned mid-escort".into()),
            ..Default::default()
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), abandon(3)).unwrap());

        assert!(batch.is_empty());
    }

    #[test]
    fn a_dead_transport_on_abandon_ends_the_session() {
        let actions = InMemoryQuestActions {
            quest_log: vec![log_slot(3, 777)],
            abandon_error: Some(
                "abandon_quest reducer transport disconnected: channel closed".into(),
            ),
            ..Default::default()
        };

        let error = match dispatch_quest_action(&actions, player(), abandon(3)) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    #[test]
    fn quest_log_update_answers_nothing_for_an_empty_log() {
        let actions = InMemoryQuestActions::default();

        assert!(quest_log_update(&actions, SELF_GUID).unwrap().is_empty());
    }

    #[test]
    fn quest_log_update_answers_the_current_raw_descriptor_for_a_non_empty_log() {
        let slots = vec![log_slot(0, QUEST), log_slot(3, QUEST + 1)];
        let actions = InMemoryQuestActions {
            quest_log: slots.clone(),
            ..Default::default()
        };
        let mask = codec::update_mask::full_quest_log_mask(&slots);
        let want = codec::build_values_update_raw(SELF_GUID, &mask);

        let batch = quest_log_update(&actions, SELF_GUID).unwrap();

        assert!(matches!(
            batch.as_slice(),
            [Outbound::Raw { opcode, body }] if (*opcode, body.clone()) == want
        ));
    }

    #[test]
    fn abandon_resolution_and_the_world_entry_block_read_the_same_slot_ordering() {
        // Both paths ask `player_quest_log` for the same player guid — the one read that decides
        // what a slot means, so the click and the window cannot disagree.
        let actions = InMemoryQuestActions {
            quest_log: vec![log_slot(3, 777)],
            ..Default::default()
        };

        let _ = dispatch_quest_action(&actions, player(), abandon(3)).unwrap();
        let _ = quest_log_update(&actions, SELF_GUID).unwrap();

        assert_eq!(
            actions.quest_log_requests.lock().unwrap().as_slice(),
            &[SELF_GUID, SELF_GUID]
        );
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

    // ── The turn-in round trip ───────────────────────────────────────────────

    fn complete_quest(quest_id: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(Box::new(
            CMSG_QUESTGIVER_COMPLETE_QUEST {
                guid: Guid::new(GIVER),
                quest_id,
            },
        ))
    }

    fn choose_reward(reward: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(Box::new(
            CMSG_QUESTGIVER_CHOOSE_REWARD {
                guid: Guid::new(GIVER),
                quest_id: QUEST,
                reward,
            },
        ))
    }

    /// A finished turn-in whose quest actually pays, so the popup has something to echo.
    fn rewarded_turn_in() -> InMemoryQuestActions {
        InMemoryQuestActions {
            details: vec![codec::QuestDetailView {
                reward_xp: 250,
                money_reward: 1200,
                ..detail(QUEST)
            }],
            ..one_quest(codec::ROLE_END, true, true)
        }
    }

    fn turn_in_of(reward_index: u32) -> TurnInCall {
        TurnInCall::TurnIn {
            account_id: 7,
            self_guid: SELF_GUID,
            giver: GIVER,
            quest_id: QUEST,
            reward_index,
        }
    }

    #[test]
    fn opening_a_complete_turn_in_offers_the_reward_and_grants_nothing() {
        let actions = one_quest(codec::ROLE_END, true, true);

        let batch =
            outbound(dispatch_quest_action(&actions, player(), complete_quest(QUEST)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(r))]
                if r.npc == Guid::new(GIVER) && r.quest_id == QUEST
        ));
        // Opening the screen is a read. The rewards are only granted once a reward is chosen.
        assert!(!actions
            .turn_in_calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| matches!(call, TurnInCall::TurnIn { .. })));
    }

    #[test]
    fn opening_an_incomplete_turn_in_asks_for_the_remaining_items() {
        let actions = one_quest(codec::ROLE_END, true, false);

        let batch =
            outbound(dispatch_quest_action(&actions, player(), complete_quest(QUEST)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(r))]
                if r.npc == Guid::new(GIVER) && r.quest_id == QUEST
        ));
    }

    #[test]
    fn opening_a_turn_in_for_an_unknown_quest_answers_nothing() {
        let actions = one_quest(codec::ROLE_END, true, true);

        let batch =
            outbound(dispatch_quest_action(&actions, player(), complete_quest(QUEST + 1)).unwrap());

        assert!(batch.is_empty());
        // No screen to build, so the giver is never evaluated.
        assert!(actions.eval_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn choosing_a_reward_turns_the_quest_in_before_any_outbound_is_built() {
        // The ordering is the whole point: the popup is built from the detail read, so a turn-in
        // recorded ahead of it is what stops a refusal from showing a false "Quest Complete".
        let actions = rewarded_turn_in();

        dispatch_quest_action(&actions, player(), choose_reward(2)).unwrap();

        assert_eq!(
            actions.turn_in_calls.lock().unwrap().as_slice(),
            &[turn_in_of(2), TurnInCall::Detail(QUEST)]
        );
    }

    #[test]
    fn a_granted_turn_in_answers_the_popup_built_from_the_quest_details() {
        let actions = rewarded_turn_in();

        let batch = outbound(dispatch_quest_action(&actions, player(), choose_reward(0)).unwrap());

        assert!(matches!(
            batch.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(c))]
                if c.quest_id == QUEST
                    && c.experience_reward == 250
                    && c.money_reward.as_int() == 1200
        ));
    }

    #[test]
    fn a_refused_turn_in_answers_nothing_and_never_claims_completion() {
        let actions = InMemoryQuestActions {
            turn_in_error: Some("quest objectives are not complete".into()),
            ..rewarded_turn_in()
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), choose_reward(2)).unwrap());

        assert!(batch.is_empty(), "a refusal must not send a completion popup");
        // The refusal stopped before the popup's detail read, so nothing was built at all.
        assert_eq!(
            actions.turn_in_calls.lock().unwrap().as_slice(),
            &[turn_in_of(2)]
        );
    }

    #[test]
    fn a_granted_turn_in_with_unreadable_details_answers_nothing_and_is_not_retried() {
        let actions = InMemoryQuestActions {
            details: Vec::new(),
            ..one_quest(codec::ROLE_END, true, true)
        };

        let batch = outbound(dispatch_quest_action(&actions, player(), choose_reward(0)).unwrap());

        assert!(batch.is_empty());
        assert_eq!(
            actions.turn_in_calls.lock().unwrap().as_slice(),
            &[turn_in_of(0), TurnInCall::Detail(QUEST)]
        );
    }

    #[test]
    fn a_reducer_transport_failure_on_turn_in_is_session_fatal() {
        let actions = InMemoryQuestActions {
            turn_in_error: Some(
                "turn_in_quest reducer transport disconnected: channel closed".into(),
            ),
            ..rewarded_turn_in()
        };

        let error = match dispatch_quest_action(&actions, player(), choose_reward(2)) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    // ── Party sharing ────────────────────────────────────────────────────────

    fn push_to_party(quest_id: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_PUSHQUESTTOPARTY(CMSG_PUSHQUESTTOPARTY { quest_id })
    }

    #[test]
    fn party_sharing_requests_the_durable_share_and_answers_nothing_on_success() {
        let actions = InMemoryQuestActions::default();

        let batch =
            outbound(dispatch_quest_action(&actions, player(), push_to_party(QUEST)).unwrap());

        assert!(
            batch.is_empty(),
            "the per-member group-event relay is the only feedback path"
        );
        assert_eq!(
            actions.push_requests.lock().unwrap().as_slice(),
            &[(7, SELF_GUID, QUEST)]
        );
    }

    #[test]
    fn a_refused_share_answers_nothing_and_does_not_end_the_session() {
        let actions = InMemoryQuestActions {
            push_error: Some("not on that quest".into()),
            ..Default::default()
        };

        let batch =
            outbound(dispatch_quest_action(&actions, player(), push_to_party(QUEST)).unwrap());

        assert!(batch.is_empty());
    }

    #[test]
    fn a_dead_transport_on_share_ends_the_session() {
        let actions = InMemoryQuestActions {
            push_error: Some("push_quest reducer transport disconnected: channel closed".into()),
            ..Default::default()
        };

        let error = match dispatch_quest_action(&actions, player(), push_to_party(QUEST)) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    // ── The gossip quest section ────────────────────────────────────────────

    #[test]
    fn gossip_quest_items_matches_the_giver_menus_items_for_the_same_evaluation() {
        // Both reads come from the same eval + the same `codec::quest_menu_items` derivation, so a
        // gossip-flagged questgiver's menu icon can never drift from what HELLO would show.
        let actions = InMemoryQuestActions {
            evals: vec![
                eval(QUEST, codec::ROLE_START, false, false),
                eval(QUEST + 1, codec::ROLE_START, false, false),
            ],
            details: vec![detail(QUEST), detail(QUEST + 1)],
            ..Default::default()
        };

        let gossip_items = gossip_quest_items(&actions, GIVER, SELF_GUID).unwrap();
        let menu = outbound(dispatch_quest_action(&actions, player(), hello(GIVER)).unwrap());

        let menu_items = match menu.as_slice() {
            [Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(l))] => &l.quest_items,
            _ => panic!("expected the quest list screen"),
        };
        assert_eq!(menu_items.len(), gossip_items.len());
        for (from_menu, from_gossip) in menu_items.iter().zip(&gossip_items) {
            assert_eq!(from_menu.quest_id, from_gossip.quest_id);
            assert_eq!(from_menu.quest_icon, from_gossip.quest_icon);
            assert_eq!(from_menu.title, from_gossip.title);
        }
    }

    #[test]
    fn a_giver_with_no_quests_contributes_an_empty_gossip_section() {
        let actions = InMemoryQuestActions::default();

        assert!(gossip_quest_items(&actions, GIVER, SELF_GUID)
            .unwrap()
            .is_empty());
    }

    // ── The gossip quest gate ───────────────────────────────────────────────

    #[test]
    fn quest_gate_state_answers_untaken_for_a_quest_never_seen() {
        let actions = InMemoryQuestActions::default();

        assert_eq!(quest_gate_state(&actions, SELF_GUID, QUEST), (false, false));
    }

    #[test]
    fn quest_gate_state_answers_taken_once_the_quest_is_in_the_log() {
        let actions = InMemoryQuestActions {
            quest_status: (true, false),
            ..Default::default()
        };

        assert_eq!(quest_gate_state(&actions, SELF_GUID, QUEST), (true, false));
        assert_eq!(
            actions.status_requests.lock().unwrap().as_slice(),
            &[(SELF_GUID, QUEST)]
        );
    }
}
