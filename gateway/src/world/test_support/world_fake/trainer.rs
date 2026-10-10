use super::super::*;

#[derive(Default)]
pub(crate) struct TrainerState {
    /// Spelled as a refusal so derive-Default (false) keeps every fixture trainer serving; the
    /// trait method reads the negation.
    pub(crate) trainer_refuses_class: bool,
    /// When set, the trainer buy answers this Refusal instead of learning the spell.
    pub(crate) trainer_buy_refusal: Option<lyracore_shared::trainer::TrainerRefusal>,
    /// What `talent_grant_spell` returns (0 = passive talent → no SMSG_LEARNED_SPELL push).
    pub(crate) talent_grant: u32,
    /// What `talent_pane_sync` returns: (teach rank-spell, superseded prev, points remaining).
    pub(crate) talent_pane: (u32, u32, u32),
    /// What `superseded_old_rank` returns for a trainer buy — the known previous rank a
    /// non-stacking chain's new rank replaces. `None` (derive-Default) mirrors "no known prior
    /// rank" -> a trainer buy pushes plain SMSG_LEARNED_SPELL.
    pub(crate) trainer_superseded: Option<u32>,
    /// Recorded `reset_talents` dispatches: (account_id, self_guid, trainer_guid) — the unlearn-talents
    /// gossip select.
    pub(crate) reset_talents_calls: std::sync::Mutex<Vec<(u64, u64, u64)>>,
    /// When set, `reset_talents` returns this error instead of recording the call.
    pub(crate) reset_talents_error: Option<String>,
    /// Trainer rows `trainer_list` returns for ANY (player, trainer) pair — the
    /// CMSG_TRAINER_LIST fixture. Empty by default (an empty trainer window).
    pub(crate) trainer_spells: Vec<codec::TrainerSpellView>,
    /// `learn_skill_line` per offering id — the skill-teaching offerings the fixture trainer carries.
    /// Empty by default, so every offering reads as an ordinary spell purchase.
    pub(crate) trainer_offer_skill_lines: std::collections::HashMap<u32, u32>,
}

impl TrainerStore for WorldFake {
    fn trainer_serves(&self, _player_guid: u64, _trainer_guid: u64) -> Result<bool> {
        Ok(!self.trainer.trainer_refuses_class) // default true — every existing fixture trainer serves
    }

    fn trainer_list(
        &self,
        _player_guid: u64,
        _trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        Ok(self.trainer.trainer_spells.clone())
    }

    fn buy_trainer_spell(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _trainer_guid: u64,
        _spell_id: u32,
    ) -> Result<crate::world::TrainerBuyOutcome> {
        if let Some(refusal) = self.trainer.trainer_buy_refusal {
            return Ok(refusal.into());
        }
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(crate::world::TrainerBuyOutcome::Learned),
        }
    }

    fn talent_grant_spell(&self, _talent_id: u32) -> u32 {
        self.trainer.talent_grant
    }

    fn set_faction_at_war(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _reputation_index: u32,
        _at_war: bool,
    ) -> Result<()> {
        Ok(())
    }

    fn set_action_button(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _button: u8,
        _action: u32,
        _action_type: u8,
    ) -> Result<()> {
        Ok(())
    }

    fn talent_pane_sync(&self, _character_guid: u64, _talent_id: u32) -> (u32, u32, u32) {
        self.trainer.talent_pane
    }

    fn talent_points_spent(&self, _character_guid: u64) -> u32 {
        0 // login stays byte-identical in every existing harness test
    }

    fn learn_talent(&self, _account_id: u64, _self_guid: u64, _talent_id: u32) -> Result<()> {
        match &self.trade_error {
            Some(e) => Err(anyhow!("{e}")),
            None => Ok(()),
        }
    }

    fn reset_talents(&self, account_id: u64, self_guid: u64, trainer_guid: u64) -> Result<()> {
        if let Some(e) = &self.trainer.reset_talents_error {
            return Err(anyhow!("{e}"));
        }
        self.trainer.reset_talents_calls.lock().unwrap().push((
            account_id,
            self_guid,
            trainer_guid,
        ));
        Ok(())
    }

    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        spell_id // mock: self-contained ranks (no wrapper table in the mock store)
    }

    fn trainer_offer_skill_line(&self, _trainer_guid: u64, spell_id: u32) -> u32 {
        // The mock's offerings are spell rows unless a test stages a skill-teaching one.
        self.trainer
            .trainer_offer_skill_lines
            .get(&spell_id)
            .copied()
            .unwrap_or(0)
    }

    fn superseded_old_rank(&self, _new_spell: u32, _player_guid: u64) -> Option<u32> {
        self.trainer.trainer_superseded
    }

    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        Ok(self
            .characters
            .iter()
            .find(|c| c.guid == guid)
            // `offline_guids` drives the invite gate's "player not online" arm. Empty by
            // default, so a seeded character is online exactly as it always was.
            .map(|c| {
                (
                    !self.social.offline_guids.contains(&guid),
                    c.level,
                    c.class,
                    c.zone_id,
                )
            }))
    }
}
