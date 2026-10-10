//! `Coordinator`'s [`TrainerStore`] adapter.

use anyhow::Result;

use crate::codec;
use crate::world::TrainerStore;

use crate::stdb::Coordinator;

impl TrainerStore for Coordinator {
    fn trainer_serves(&self, player_guid: u64, trainer_guid: u64) -> Result<bool> {
        self.trainer_serves(player_guid, trainer_guid)
    }

    fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        self.trainer_list(player_guid, trainer_guid)
    }

    fn buy_trainer_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<crate::world::TrainerBuyOutcome> {
        self.buy_trainer_spell(account_id, self_guid, trainer_guid, spell_id)
    }

    fn trainer_offer_skill_line(&self, trainer_guid: u64, spell_id: u32) -> u32 {
        self.trainer_offer_skill_line(trainer_guid, spell_id)
    }

    fn talent_grant_spell(&self, talent_id: u32) -> u32 {
        self.talent_by_id(talent_id)
            .map(|t| t.grant_spell_id)
            .unwrap_or(0)
    }

    fn set_faction_at_war(
        &self,
        account_id: u64,
        self_guid: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()> {
        self.set_faction_at_war(account_id, self_guid, reputation_index, at_war)
    }

    fn set_action_button(
        &self,
        account_id: u64,
        self_guid: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()> {
        self.set_action_button(account_id, self_guid, button, action, action_type)
    }

    fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32) {
        self.talent_pane_sync(character_guid, talent_id)
    }

    fn talent_points_spent(&self, character_guid: u64) -> u32 {
        self.talent_points_spent(character_guid)
    }

    fn learn_talent(&self, account_id: u64, self_guid: u64, talent_id: u32) -> Result<()> {
        self.learn_talent(account_id, self_guid, talent_id)
    }

    fn reset_talents(&self, account_id: u64, self_guid: u64, trainer_guid: u64) -> Result<()> {
        self.reset_talents(account_id, self_guid, trainer_guid)
    }

    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        self.resolve_learn_target(spell_id)
    }

    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32> {
        self.superseded_old_rank(new_spell, player_guid)
    }

    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        self.character_presence(guid)
    }
}
