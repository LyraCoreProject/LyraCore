use super::super::*;

#[derive(Default)]
pub(crate) struct QuestState {
    /// Quest-giver evals returned by `giver_quest_evals` (the menu/status input).
    pub(crate) quest_evals: Vec<codec::GiverQuestEval>,
    /// Quest details `quest_detail_view(id)` resolves from (matched by `quest_id`).
    pub(crate) quest_details: Vec<codec::QuestDetailView>,
    /// The player's quest-log slots `player_quest_log` returns (drives the login descriptor block).
    pub(crate) quest_log_slots: Vec<codec::update_mask::QuestLogSlot>,
    pub(crate) quest_log_read_error: Option<String>,
    pub(crate) quest_log_after_subscribe:
        Option<Result<Vec<codec::update_mask::QuestLogSlot>, String>>,
    /// Recorded `turn_in_quest` dispatches: (actor, giver, quest, reward_index) — so the
    /// choose-reward socket test asserts the player's pick reached the store unchanged.
    pub(crate) turned_in: std::sync::Mutex<Vec<(u64, u64, u32, u32)>>,
    /// A Reward Letter a successful `turn_in_quest` files as Escrow for the Character, as
    /// `gw_turn_in_quest` does in its own transaction.
    pub(crate) turn_in_reward_letter: Option<crate::world::mail::HeldEscrow>,
    /// The caller's quest log for `quest_status`, as `(quest_id, rewarded)` pairs — a quest id present
    /// here is "taken"; `rewarded` distinguishes active vs. turned-in. Absent = never seen.
    /// Behind a `Mutex` so a test can change the log WHILE a gossip window is open — the
    /// HELLO→SELECT race the menu snapshot exists to close.
    pub(crate) quest_log: std::sync::Mutex<Vec<(u32, bool)>>,
}

impl QuestActionStore for WorldFake {
    fn giver_quest_evals(
        &self,
        _giver_guid: u64,
        _player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>> {
        let mut evaluations = self.quest.quest_evals.clone();
        if self.benilla_gameplay.is_some() {
            let log = self.quest.quest_log.lock().unwrap();
            for evaluation in &mut evaluations {
                if evaluation.role == codec::ROLE_END {
                    evaluation.active = log.contains(&(evaluation.quest_id, false));
                    evaluation.complete &= evaluation.active;
                }
            }
        }
        Ok(evaluations)
    }

    fn quest_detail_view(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
        Ok(self
            .quest
            .quest_details
            .iter()
            .find(|d| d.quest_id == quest_id)
            .cloned())
    }

    fn giver_refuses_interaction(&self, _giver_guid: u64, _player_guid: u64) -> Result<bool> {
        Ok(self.npc_refuses)
    }

    fn accept_quest(&self, _actor: Actor, _giver_guid: u64, quest_id: u32) -> Result<()> {
        if self.benilla_gameplay.is_some() {
            let mut log = self.quest.quest_log.lock().unwrap();
            if !log.iter().any(|&(id, _)| id == quest_id) {
                log.push((quest_id, false));
            }
        }
        Ok(())
    }

    /// No socket test drives an item-started quest — the route is proved at the quest seam.
    fn item_start_quest(&self, _owner_guid: u64, _slot: u8) -> Option<(u64, u32)> {
        None
    }

    fn player_quest_log(&self, _player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>> {
        if let Some(error) = &self.quest.quest_log_read_error {
            return Err(anyhow!(error.clone()));
        }
        if !self.session.subscribed.lock().unwrap().is_empty() {
            if let Some(slots) = &self.quest.quest_log_after_subscribe {
                return slots.clone().map_err(|error| anyhow!(error));
            }
        }
        Ok(self.quest.quest_log_slots.clone())
    }

    fn abandon_quest(&self, _actor: Actor, _quest_id: u32) -> Result<()> {
        Ok(())
    }

    fn push_quest(&self, _actor: Actor, _quest_id: u32) -> Result<()> {
        Ok(())
    }

    fn quest_status(&self, _player_guid: u64, quest_id: u32) -> (bool, bool) {
        match self
            .quest
            .quest_log
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == quest_id)
        {
            Some((_, rewarded)) => (true, *rewarded),
            None => (false, false),
        }
    }

    fn turn_in_quest(
        &self,
        actor: Actor,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        if let Some(e) = &self.trade_error {
            return Err(crate::stdb::ReducerCallError::refused("gw_turn_in_quest", e).into());
        }
        if let Some(state) = &self.benilla_gameplay {
            let refused =
                |reason| crate::stdb::ReducerCallError::refused("gw_turn_in_quest", reason);
            let mut log = self.quest.quest_log.lock().unwrap();
            let Some((_, rewarded)) = log.iter_mut().find(|(id, _)| *id == quest_id) else {
                return Err(refused("quest not active").into());
            };
            if *rewarded {
                return Err(refused("quest already rewarded").into());
            }
            let detail = self
                .quest_detail_view(quest_id)?
                .ok_or_else(|| refused("quest absent"))?;
            state.lock().unwrap().copper += detail.money_reward;
            *rewarded = true;
        }
        self.quest.turned_in.lock().unwrap().push((
            actor.guid(),
            giver_guid,
            quest_id,
            reward_index,
        ));
        if let Some(letter) = self.quest.turn_in_reward_letter.clone() {
            self.mail
                .attested
                .lock()
                .unwrap()
                .push((letter.escrow_id, false));
            self.mail
                .mail_escrows
                .lock()
                .unwrap()
                .push((letter.recipient_guid, letter));
        }
        if let (Some(item), Some(tx)) = (
            self.session.turn_in_reward_item.clone(),
            self.session.turn_in_tx.lock().unwrap().take(),
        ) {
            let mut relay = vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(codec::build_item_create_object(&item)),
            ))];
            if let Some(pointer) =
                codec::build_inv_slot_values(item.owner_guid, item.slot, item.guid)
            {
                relay.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                    Box::new(pointer),
                )));
            }
            relay.push(Outbound::One(ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(
                Box::new(codec::build_item_push_result(
                    item.owner_guid,
                    255,
                    item.slot as u32,
                    item.entry,
                    item.stack_count,
                    false,
                    0,
                )),
            )));
            tx.send(Outbound::Job(Box::new(move || relay)))
                .map_err(|_| anyhow!("reward item relay writer is gone"))?;
        }
        Ok(())
    }
}
