use super::super::*;

#[derive(Default)]
pub(crate) struct TrainerState {
    pub(crate) spells: Vec<codec::TrainerSpellView>,
    pub(crate) reputation_at_war: std::sync::Mutex<std::collections::BTreeMap<u32, bool>>,
    pub(crate) talent_reset_quote: Option<u32>,
    pub(crate) reset_talents_refusal: Option<String>,
    /// Spelled as a refusal so derive-Default (false) keeps every fixture trainer serving; the
    /// trait method reads the negation.
    pub(crate) trainer_refuses_class: bool,
    /// Recorded `reset_talents` dispatches: (actor guid, trainer_guid) — the unlearn-talents
    /// gossip select.
    pub(crate) reset_talents_calls: std::sync::Mutex<Vec<(u64, u64)>>,
}

impl TrainerStore for WorldFake {
    fn talent_reset_cost(&self, _character_guid: u64) -> Option<u32> {
        self.trainer.talent_reset_quote
    }

    fn trainer_serves(&self, _player_guid: u64, _trainer_guid: u64) -> Result<bool> {
        Ok(!self.trainer.trainer_refuses_class) // default true — every existing fixture trainer serves
    }

    fn trainer_list(
        &self,
        _player_guid: u64,
        _trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        Ok(self.trainer.spells.clone())
    }

    fn buy_trainer_spell(
        &self,
        _actor: Actor,
        _trainer_guid: u64,
        _spell_id: u32,
    ) -> Result<crate::world::TrainerBuyOutcome> {
        Ok(crate::world::TrainerBuyOutcome::Learned)
    }

    fn talent_grant_spell(&self, _talent_id: u32) -> u32 {
        0
    }

    fn set_faction_at_war(
        &self,
        _actor: Actor,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<InteractionOutcome> {
        self.trainer
            .reputation_at_war
            .lock()
            .unwrap()
            .insert(reputation_index, at_war);
        Ok(InteractionOutcome::Done)
    }

    fn set_action_button(
        &self,
        _actor: Actor,
        _button: u8,
        _action: u32,
        _action_type: u8,
    ) -> Result<()> {
        Ok(())
    }

    fn talent_pane_sync(&self, _character_guid: u64, _talent_id: u32) -> (u32, u32, u32) {
        (0, 0, 0)
    }

    fn talent_points_spent(&self, _character_guid: u64) -> u32 {
        0 // login stays byte-identical in every existing harness test
    }

    fn learn_talent(&self, _actor: Actor, _talent_id: u32) -> Result<()> {
        Ok(())
    }

    fn reset_talents(&self, actor: Actor, trainer_guid: u64) -> Result<InteractionOutcome> {
        if let Some(e) = &self.trainer.reset_talents_refusal {
            return Ok(InteractionOutcome::Refused(e.clone()));
        }
        self.trainer
            .reset_talents_calls
            .lock()
            .unwrap()
            .push((actor.guid(), trainer_guid));
        Ok(InteractionOutcome::Done)
    }

    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        spell_id // self-contained ranks (no wrapper table in the Fake)
    }

    fn trainer_offer_skill_line(&self, _trainer_guid: u64, _spell_id: u32) -> u32 {
        0 // every offering reads as an ordinary spell purchase
    }

    fn superseded_old_rank(&self, _new_spell: u32, _player_guid: u64) -> Option<u32> {
        None
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
