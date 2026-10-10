//! Trainer and talent opcodes, run through `handle_trainer` against a Fake that holds only the
//! Character, Npc and Trainer Stores the handler is bounded on.

use super::handlers::{handle_trainer, CharacterStore, TrainerBuyOutcome, TrainerStore};
use super::*;
use lyracore_shared::constants::armor_proficiency::{
    PLATE_PASSIVE_SPELL_ID, PLATE_TRAINER_SPELL_ID,
};
use lyracore_shared::trainer::TrainerRefusal;

/// What the Module would answer a trainer window, a buy and a talent pick, set per test.
#[derive(Default)]
struct TrainerFake {
    npc_refuses: bool,
    refuses_class: bool,
    spells: Vec<codec::TrainerSpellView>,
    /// `learn_skill_line` per offering id. An offering absent here teaches an ordinary spell.
    skill_lines: std::collections::HashMap<u32, u32>,
    buy_refusal: Option<TrainerRefusal>,
    /// A failed Durable Request: the result is unknown.
    buy_error: Option<String>,
    /// The known previous rank a buy of a non-stacking chain replaces.
    superseded: Option<u32>,
    /// The spellbook after the buy.
    learned_spells: Vec<u32>,
    /// `(level, class)` of the buying Character.
    level_and_class: Option<(u8, u8)>,
    talent_grant: u32,
    /// `(rank spell taught, rank spell it replaces, points left)`.
    talent_pane: (u32, u32, u32),
}

impl TrainerFake {
    fn buying_as(level: u8, class: u8) -> Self {
        Self {
            level_and_class: Some((level, class)),
            ..Default::default()
        }
    }
}

npc_store_refusing_by!(TrainerFake, npc_refuses);

impl CharacterStore for TrainerFake {
    fn player_learned_spells(&self, _player_guid: u64) -> Result<Vec<u32>> {
        Ok(self.learned_spells.clone())
    }

    fn spell_modifiers(&self, _character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        Vec::new()
    }

    fn characters(&self, _account_id: u64) -> Result<Vec<codec::CharacterView>> {
        unimplemented!("characters")
    }

    fn create_character(
        &self,
        _account_id: u64,
        _name: &str,
        _race: u8,
        _class: u8,
        _gender: u8,
        _appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome> {
        unimplemented!("create_character")
    }

    fn delete_character(
        &self,
        _account_id: u64,
        _character_guid: u64,
    ) -> Result<codec::CharDeleteOutcome> {
        unimplemented!("delete_character")
    }

    fn character_by_guid(&self, _guid: u64) -> Result<Option<codec::CharacterView>> {
        unimplemented!("character_by_guid")
    }

    fn character_exists_on_any_world_shard(&self, _guid: u64) -> Result<bool> {
        unimplemented!("character_exists_on_any_world_shard")
    }

    fn player_skills(&self, _character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        unimplemented!("player_skills")
    }

    fn effective_armor(&self, _guid: u64) -> u32 {
        unimplemented!("effective_armor")
    }

    fn effective_magic_resistances(&self, _guid: u64) -> [u32; 6] {
        unimplemented!("effective_magic_resistances")
    }

    fn player_reputations(&self, _player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        unimplemented!("player_reputations")
    }

    fn player_actions(&self, _player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        unimplemented!("player_actions")
    }
}

impl TrainerStore for TrainerFake {
    fn trainer_serves(&self, _player_guid: u64, _trainer_guid: u64) -> Result<bool> {
        Ok(!self.refuses_class)
    }

    fn trainer_list(
        &self,
        _player_guid: u64,
        _trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        Ok(self.spells.clone())
    }

    fn buy_trainer_spell(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _trainer_guid: u64,
        _spell_id: u32,
    ) -> Result<TrainerBuyOutcome> {
        if let Some(refusal) = self.buy_refusal {
            return Ok(refusal.into());
        }
        match &self.buy_error {
            Some(error) => Err(anyhow!("{error}")),
            None => Ok(TrainerBuyOutcome::Learned),
        }
    }

    fn trainer_offer_skill_line(&self, _trainer_guid: u64, spell_id: u32) -> u32 {
        self.skill_lines.get(&spell_id).copied().unwrap_or(0)
    }

    fn talent_grant_spell(&self, _talent_id: u32) -> u32 {
        self.talent_grant
    }

    fn set_action_button(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _button: u8,
        _action: u32,
        _action_type: u8,
    ) -> Result<()> {
        unimplemented!("set_action_button")
    }

    fn set_faction_at_war(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _reputation_index: u32,
        _at_war: bool,
    ) -> Result<InteractionOutcome> {
        unimplemented!("set_faction_at_war")
    }

    fn talent_pane_sync(&self, _character_guid: u64, _talent_id: u32) -> (u32, u32, u32) {
        self.talent_pane
    }

    fn talent_points_spent(&self, _character_guid: u64) -> u32 {
        unimplemented!("talent_points_spent")
    }

    fn learn_talent(&self, _account_id: u64, _self_guid: u64, _talent_id: u32) -> Result<()> {
        Ok(())
    }

    fn talent_reset_cost(&self, _character_guid: u64) -> Option<u32> {
        unimplemented!("talent_reset_cost")
    }

    fn reset_talents(
        &self,
        _account_id: u64,
        _self_guid: u64,
        _trainer_guid: u64,
    ) -> Result<InteractionOutcome> {
        unimplemented!("reset_talents")
    }

    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        spell_id
    }

    fn superseded_old_rank(&self, _new_spell: u32, _player_guid: u64) -> Option<u32> {
        self.superseded
    }

    fn character_presence(&self, _guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        Ok(self
            .level_and_class
            .map(|(level, class)| (true, level, class, 0)))
    }
}

/// Send `msg` as Character 1 of account 7. Returns the handler's verdict and what it sent, in order.
/// Phase 5 retargets only this helper.
fn drive(
    store: &TrainerFake,
    msg: impl Into<ClientOpcodeMessage>,
) -> (Result<()>, Vec<ServerOpcodeMessage>) {
    let (tx, rx) = SessionTx::with_depth(0);
    let mut conn = in_world_conn(7, 1);
    let verdict = handle_trainer(&tx, store, &mut conn, msg.into())
        .map(|passed_on| assert!(passed_on.is_none(), "the trainer family owns this opcode"));
    (verdict, drain_outbound(&rx))
}

fn run(store: &TrainerFake, msg: impl Into<ClientOpcodeMessage>) -> Vec<ServerOpcodeMessage> {
    let (verdict, sent) = drive(store, msg);
    verdict.unwrap();
    sent
}

fn kinds(sent: &[ServerOpcodeMessage]) -> String {
    sent.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn buy(id: u32) -> CMSG_TRAINER_BUY_SPELL {
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id,
    }
}

fn pick_talent() -> CMSG_LEARN_TALENT {
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
}

fn one_spell() -> Vec<codec::TrainerSpellView> {
    vec![codec::TrainerSpellView {
        spell_id: 100,
        cost: 10,
        required_level: 1,
        player_level: 1,
        known: false,
        profession: false,
    }]
}

#[test]
fn trainer_buy_success_replies_succeeded_then_pushes_the_learned_spell() {
    let sent = run(&TrainerFake::default(), buy(1234));
    let [ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(bought), ServerOpcodeMessage::SMSG_LEARNED_SPELL(learned)] =
        sent.as_slice()
    else {
        panic!(
            "expected a buy confirmation then the learned spell, got [{}]",
            kinds(&sent)
        );
    };
    assert_eq!(bought.guid.guid(), 70);
    assert_eq!(bought.id, 1234);
    assert_eq!(learned.id, 1234);
}

/// A RIDING purchase teaches a skill, not a spell. Its trainer-list id is a marker with no Spell.dbc
/// row, so the buy confirms and stops: a learned-spell push would hand the client an id it cannot
/// resolve. The skill pane moves from the `game_player_skill` relay.
#[test]
fn a_riding_buy_confirms_without_echoing_the_offering_as_a_learned_spell() {
    let store = TrainerFake {
        skill_lines: [(50132, lyracore_shared::trainer::RIDING_SKILL_LINE)].into(),
        ..Default::default()
    };
    let sent = run(&store, buy(50132));
    assert!(
        matches!(sent.as_slice(), [ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(m)] if m.id == 50132),
        "a riding buy confirms and sends nothing more, got [{}]",
        kinds(&sent)
    );
}

/// A Warrior buying Plate Mail at 40. The book update alone leaves the client tinting every plate
/// piece red, so the buy also refreshes the ARMOR mask from the post-buy spellbook.
#[test]
fn a_proficiency_buy_pushes_the_refreshed_armor_mask_after_the_learned_spell() {
    let store = TrainerFake {
        learned_spells: vec![PLATE_PASSIVE_SPELL_ID],
        ..TrainerFake::buying_as(40, 1)
    };
    let sent = run(&store, buy(PLATE_TRAINER_SPELL_ID));
    let [ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(bought), ServerOpcodeMessage::SMSG_LEARNED_SPELL(learned), ServerOpcodeMessage::SMSG_SET_PROFICIENCY(mask)] =
        sent.as_slice()
    else {
        panic!(
            "expected confirmation, learned spell, proficiency mask, got [{}]",
            kinds(&sent)
        );
    };
    assert_eq!(bought.id, 7109);
    assert_eq!(learned.id, 7109);
    assert_eq!(
        mask.class.as_int(),
        4,
        "a proficiency buy refreshes ARMOR only"
    );
    assert!(
        mask.item_sub_class_mask & (1 << lyracore_shared::item::armor_subclass::PLATE) != 0,
        "the trained plate bit must be set: {:#x}",
        mask.item_sub_class_mask
    );
}

/// An ordinary ability purchase changes nothing about what the Character may wear, so it must not
/// resend a proficiency mask.
#[test]
fn an_ordinary_trainer_buy_pushes_no_proficiency_mask() {
    let sent = run(&TrainerFake::buying_as(40, 1), buy(1234));
    assert!(
        matches!(
            sent.as_slice(),
            [
                ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(_),
                ServerOpcodeMessage::SMSG_LEARNED_SPELL(learned)
            ] if learned.id == 1234
        ),
        "expected only the confirmation and the learned spell, got [{}]",
        kinds(&sent)
    );
}

/// A buy whose previous rank is known sends SMSG_SUPERCEDED_SPELL, so the client replaces the old
/// rank's book entry. The first wire slot carries the OLD rank (cmangos order), as the talent
/// rank-upgrade path does.
#[test]
fn trainer_buy_rank_upgrade_supersedes_the_previous_rank_spell() {
    let store = TrainerFake {
        superseded: Some(1233),
        ..Default::default()
    };
    let sent = run(&store, buy(1234));
    let [ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(bought), ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(replaced)] =
        sent.as_slice()
    else {
        panic!(
            "expected a buy confirmation then a superseded spell, got [{}]",
            kinds(&sent)
        );
    };
    assert_eq!(bought.guid.guid(), 70);
    assert_eq!(bought.id, 1234);
    assert_eq!(
        replaced.new_spell_id, 1233,
        "first wire slot carries the OLD rank"
    );
    assert_eq!(
        replaced.old_spell_id, 1234,
        "second wire slot carries the NEW rank"
    );
}

#[test]
fn every_module_refusal_has_one_client_failure_reason() {
    let expected = |refusal| match refusal {
        TrainerRefusal::NotEnoughMoney => TrainingFailureReason::NotEnoughMoney,
        TrainerRefusal::LevelTooLow | TrainerRefusal::PreviousRankMissing => {
            TrainingFailureReason::NotEnoughSkill
        }
        TrainerRefusal::Unavailable | TrainerRefusal::NotOffered | TrainerRefusal::AlreadyKnown => {
            TrainingFailureReason::Unavailable
        }
    };
    for refusal in TrainerRefusal::ALL {
        let store = TrainerFake {
            buy_refusal: Some(refusal),
            ..Default::default()
        };
        let sent = run(&store, buy(1234));
        let [ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(failed)] = sent.as_slice() else {
            panic!("{refusal:?}: expected one failure, got [{}]", kinds(&sent));
        };
        assert_eq!(failed.error, expected(refusal), "{refusal:?}");
        assert_eq!(failed.id, 1234);
    }
}

/// A reducer timeout leaves the durable result unknown, so it must not reach the client as a
/// gameplay Refusal that says the purchase did not happen. The handler fails; the session ends.
#[test]
fn a_trainer_reducer_timeout_is_not_answered_as_a_refusal() {
    let store = TrainerFake {
        buy_error: Some("gw_trainer_buy reducer timed out after 10s".into()),
        ..Default::default()
    };
    let (verdict, sent) = drive(&store, buy(1234));
    let error = verdict.expect_err("a timed-out trainer reducer must be session-fatal");
    assert!(format!("{error:#}").contains("timed out"));
    assert!(
        sent.is_empty(),
        "nothing claims the purchase failed: [{}]",
        kinds(&sent)
    );
}

/// An ability talent (`grant_spell_id != 0`) pushes the granted spell so the new button works
/// without a relog, then the points push.
#[test]
fn learn_talent_with_a_grant_spell_pushes_learned_spell() {
    let store = TrainerFake {
        talent_grant: 2098,
        ..Default::default()
    };
    let sent = run(&store, pick_talent());
    assert!(
        matches!(
            sent.as_slice(),
            [
                ServerOpcodeMessage::SMSG_LEARNED_SPELL(granted),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_)
            ] if granted.id == 2098
        ),
        "expected the granted spell then the CHARACTER_POINTS1 push, got [{}]",
        kinds(&sent)
    );
}

/// A passive pick still refreshes the TalentFrame: SMSG_LEARNED_SPELL for the rank spell the Module
/// taught (the pane derives shown ranks from known rank spells), then the points push.
#[test]
fn learn_talent_passive_pushes_rank_spell_and_points() {
    let store = TrainerFake {
        talent_pane: (7777, 0, 2),
        ..Default::default()
    };
    let sent = run(&store, pick_talent());
    assert!(
        matches!(
            sent.as_slice(),
            [
                ServerOpcodeMessage::SMSG_LEARNED_SPELL(rank),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_)
            ] if rank.id == 7777
        ),
        "expected the rank spell then the CHARACTER_POINTS1 push, got [{}]",
        kinds(&sent)
    );
}

/// Rank N>1 replaces the previous rank's spell in the book. SMSG_SUPERCEDED_SPELL carries the OLD
/// rank in its first wire slot (cmangos order), as the trainer rank-upgrade path does.
#[test]
fn learn_talent_rank_upgrade_supersedes_the_previous_rank_spell() {
    let store = TrainerFake {
        talent_pane: (7778, 7777, 1),
        ..Default::default()
    };
    let sent = run(&store, pick_talent());
    let [ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(replaced), ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_)] =
        sent.as_slice()
    else {
        panic!(
            "expected a superseded spell then the points push, got [{}]",
            kinds(&sent)
        );
    };
    assert_eq!(
        replaced.new_spell_id, 7777,
        "first wire slot carries the OLD rank"
    );
    assert_eq!(
        replaced.old_spell_id, 7778,
        "second wire slot carries the NEW rank"
    );
}

#[test]
fn trainer_list_replies_smsg_trainer_list_with_the_fixture_spells() {
    let store = TrainerFake {
        spells: one_spell(),
        ..Default::default()
    };
    let sent = run(
        &store,
        CMSG_TRAINER_LIST {
            guid: Guid::new(70),
        },
    );
    let [ServerOpcodeMessage::SMSG_TRAINER_LIST(list)] = sent.as_slice() else {
        panic!("expected one trainer list, got [{}]", kinds(&sent));
    };
    assert_eq!(list.guid, Guid::new(70));
    assert_eq!(list.spells.len(), 1, "the fixture's one spell row");
    assert_eq!(list.spells[0].spell, 100);
}

/// The class gate removes the training service, not the creature: the NPC still talks, which the
/// gossip test `gossip_hides_the_train_and_unlearn_options_for_a_class_the_trainer_does_not_serve` covers.
#[test]
fn trainer_list_is_silently_dropped_for_a_player_the_trainer_does_not_serve() {
    let store = TrainerFake {
        refuses_class: true,
        spells: one_spell(),
        ..Default::default()
    };
    let sent = run(
        &store,
        CMSG_TRAINER_LIST {
            guid: Guid::new(70),
        },
    );
    assert!(
        sent.is_empty(),
        "a trainer that does not serve this class sends no window, got [{}]",
        kinds(&sent)
    );
}
