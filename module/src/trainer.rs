use std::collections::{HashMap, HashSet};

use lyracore_shared::trainer::{
    is_primary_first_rank, is_primary_profession_line, offering_known, offering_state,
    OfferingGates, OfferingState, TrainerRefusal,
};
use spacetimedb::{table, ReducerContext, Table};

use crate::{
    game_creature_template, game_player_skill, game_spell, game_spell_chain, game_spell_effect,
    game_world_entity, spell::spellbook::game_player_spell, WorldEntity,
};

pub(crate) fn refused(refusal: TrainerRefusal, detail: &str) -> TrainerRefusal {
    spacetimedb::log::info!("trainer refused {}: {detail}", refusal.as_tag());
    refusal
}

#[table(
    accessor = game_trainer_spell,
    public,
    index(accessor = by_trainer, btree(columns = [trainer_entry])),
    index(accessor = by_spell, btree(columns = [spell_id]))
)]
pub struct TrainerSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub trainer_entry: u32,
    pub spell_id: u32,
    pub cost: u32,
    pub required_level: u8,
    #[default(0u32)]
    pub learn_skill_line: u32,
    #[default(75u32)]
    pub learn_skill_cap: u32,
    #[default(0u32)]
    pub required_skill: u32,
    #[default(0u32)]
    pub required_skill_value: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuyGrant {
    Profession(u32, u32),
    Riding(u32),
    Spell(u32),
}

pub(crate) fn rank_prereq_met(prev_spell: u32, knows_prev: bool) -> bool {
    prev_spell == 0 || knows_prev
}

pub(crate) struct Assessment {
    pub known: bool,
    pub gates: OfferingGates,
}

pub(crate) struct Trainee {
    guid: u64,
    learned: HashSet<u32>,
    skills: HashMap<u32, (u32, u32)>,
}

impl Trainee {
    pub(crate) fn load(ctx: &ReducerContext, guid: u64) -> Self {
        Self {
            guid,
            learned: ctx
                .db
                .game_player_spell()
                .by_character()
                .filter(guid)
                .map(|row| row.spell_id)
                .collect(),
            skills: ctx
                .db
                .game_player_skill()
                .by_character()
                .filter(guid)
                .map(|row| {
                    (
                        row.skill_line,
                        (u32::from(row.current), u32::from(row.max_rank)),
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn assess(
        &self,
        ctx: &ReducerContext,
        offered: &TrainerSpell,
        learn_target: u32,
    ) -> Assessment {
        let line = offered.learn_skill_line;
        let known = offering_known(
            line,
            offered.learn_skill_cap,
            self.skills.get(&line).map(|&(_, max_rank)| max_rank),
            self.learned.contains(&learn_target),
        );
        let previous_rank_missing = line == 0
            && ctx
                .db
                .game_spell_chain()
                .spell_id()
                .find(learn_target)
                .is_some_and(|chain| {
                    !rank_prereq_met(chain.prev_spell, self.learned.contains(&chain.prev_spell))
                });
        let base_required_skill =
            self.skills
                .get(&offered.required_skill)
                .map_or(0, |&(current, _)| {
                    let (_, permanent) =
                        crate::skill::skill_bonuses(ctx, self.guid, offered.required_skill);
                    u32::try_from(i64::from(current) + i64::from(permanent)).unwrap_or(0)
                });
        let held_primaries = self
            .skills
            .keys()
            .filter(|&&held| is_primary_profession_line(held))
            .count();
        Assessment {
            known,
            gates: OfferingGates {
                previous_rank_missing,
                skill_requirement_unmet: offered.required_skill != 0
                    && base_required_skill < offered.required_skill_value,
                primary_first_rank: is_primary_first_rank(line, offered.learn_skill_cap),
                primary_slot_free: lyracore_shared::trainer::free_primary_profession_slots(
                    held_primaries,
                ) > 0,
            },
        }
    }
}

pub(crate) fn trainer_buy_check(
    state: OfferingState,
    money: u32,
    cost: u32,
) -> Result<(), TrainerRefusal> {
    match state {
        OfferingState::Known => return Err(TrainerRefusal::AlreadyKnown),
        OfferingState::LevelTooLow => return Err(TrainerRefusal::LevelTooLow),
        OfferingState::PreviousRankMissing => return Err(TrainerRefusal::PreviousRankMissing),
        OfferingState::SkillTooLow => return Err(TrainerRefusal::SkillTooLow),
        OfferingState::PrimaryProfessionLimit => {
            return Err(TrainerRefusal::PrimaryProfessionLimit)
        }
        OfferingState::Available => {}
    }
    if money < cost {
        return Err(TrainerRefusal::NotEnoughMoney);
    }
    Ok(())
}

// Trainers read E_SCRIPTED as a learn effect, as before E_LEARN_SPELL, unless it teaches the pet.
fn learn_kind(kind: u8, target: u8) -> u8 {
    use lyracore_shared::spell::taxonomy::{E_LEARN_SPELL, E_SCRIPTED, T_CASTER_PET};
    if kind == E_SCRIPTED && target != T_CASTER_PET {
        E_LEARN_SPELL
    } else {
        kind
    }
}

fn learn_target_trigger(effect: &crate::SpellEffect) -> Option<u32> {
    lyracore_shared::trainer::learn_target_trigger(
        learn_kind(effect.kind, effect.target),
        effect.trigger_spell,
    )
}

fn learn_target_from_effects(spell_id: u32, effects: &[crate::SpellEffect]) -> u32 {
    lyracore_shared::trainer::resolve_learn_target(
        spell_id,
        effects
            .iter()
            .map(|effect| lyracore_shared::trainer::LearnEffect {
                index: effect.effect_index,
                id: effect.id,
                kind: learn_kind(effect.kind, effect.target),
                trigger_spell: effect.trigger_spell,
            }),
    )
}

pub(crate) fn resolve_learn_target(ctx: &ReducerContext, spell_id: u32) -> u32 {
    let effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    learn_target_from_effects(spell_id, &effects)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_TRAINER_OFFERING_LIMIT: usize = 16;
#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_WRAPPER_EFFECT_LIMIT: usize = 16;
#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_SPELL_EFFECT_LIMIT: usize = 3;

#[cfg_attr(not(has_packages), allow(dead_code))]
fn profile_limit(detail: impl Into<String>) -> crate::actor::ActionRefusal {
    crate::actor::ActionRefusal::new(crate::actor::ActionRefusalKind::ProfileLimit, detail.into())
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn profile_learn_target(
    ctx: &ReducerContext,
    wrapper_spell: u32,
) -> Result<u32, crate::actor::ActionRefusal> {
    let effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&wrapper_spell)
        .take(PROFILE_SPELL_EFFECT_LIMIT + 1)
        .collect();
    if effects.len() > PROFILE_SPELL_EFFECT_LIMIT {
        return Err(profile_limit(format!(
            "spell {wrapper_spell} has more than {PROFILE_SPELL_EFFECT_LIMIT} effects"
        )));
    }
    Ok(learn_target_from_effects(wrapper_spell, &effects))
}

#[derive(Default)]
#[cfg_attr(not(has_packages), allow(dead_code))]
struct ProfileOfferingScan {
    examined: usize,
    lowest_required_level: Option<u32>,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
impl ProfileOfferingScan {
    fn inspect(
        &mut self,
        ctx: &ReducerContext,
        class: u8,
        level: u32,
        offering: &TrainerSpell,
    ) -> bool {
        self.examined += 1;
        if offering.learn_skill_line != 0 {
            return false;
        }
        let Some(trainer) = ctx
            .db
            .game_creature_template()
            .entry()
            .find(offering.trainer_entry)
        else {
            return false;
        };
        if !lyracore_shared::trainer::serves(class, trainer.trainer_type, trainer.trainer_class) {
            return false;
        }
        let required_level = u32::from(offering.required_level);
        if level >= required_level {
            return true;
        }
        self.lowest_required_level = Some(
            self.lowest_required_level
                .map_or(required_level, |seen| seen.min(required_level)),
        );
        false
    }

    fn refusal(&self, requested_spell: u32, class: u8) -> crate::actor::ActionRefusal {
        if let Some(required_level) = self.lowest_required_level {
            crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::Level,
                format!("spell {requested_spell} requires level {required_level}"),
            )
        } else {
            crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::Class,
                format!("spell {requested_spell} is not offered for class {class}"),
            )
        }
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn sorted_offerings(
    ctx: &ReducerContext,
    spell_id: u32,
    remaining: usize,
) -> (Vec<TrainerSpell>, bool) {
    let mut offerings: Vec<_> = ctx
        .db
        .game_trainer_spell()
        .by_spell()
        .filter(&spell_id)
        .take(remaining + 1)
        .collect();
    offerings.sort_by_key(|offering| offering.id);
    let overflow = offerings.len() > remaining;
    offerings.truncate(remaining);
    (offerings, overflow)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn admit_profile_spell(
    ctx: &ReducerContext,
    spell_id: u32,
    class: u8,
    level: u32,
) -> Result<(), crate::actor::ActionRefusal> {
    let mut scan = ProfileOfferingScan::default();
    let (direct, direct_overflow) = sorted_offerings(ctx, spell_id, PROFILE_TRAINER_OFFERING_LIMIT);
    let direct_matches = if direct.is_empty() {
        false
    } else {
        profile_learn_target(ctx, spell_id)? == spell_id
    };
    if direct_matches {
        for offering in &direct {
            if scan.inspect(ctx, class, level, offering) {
                return Ok(());
            }
        }
        if direct_overflow {
            return Err(profile_limit(format!(
                "spell {spell_id} trainer admission exceeded {PROFILE_TRAINER_OFFERING_LIMIT} offerings"
            )));
        }
    }

    let mut trigger_effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_trigger_spell()
        .filter(&spell_id)
        .take(PROFILE_WRAPPER_EFFECT_LIMIT + 1)
        .collect();
    trigger_effects.sort_by_key(|effect| (effect.spell_id, effect.effect_index, effect.id));
    let trigger_overflow = trigger_effects.len() > PROFILE_WRAPPER_EFFECT_LIMIT;
    trigger_effects.truncate(PROFILE_WRAPPER_EFFECT_LIMIT);
    let mut wrappers: Vec<_> = trigger_effects
        .iter()
        .filter(|effect| learn_target_trigger(effect) == Some(spell_id))
        .map(|effect| effect.spell_id)
        .filter(|wrapper| *wrapper != spell_id)
        .collect();
    wrappers.sort_unstable();
    wrappers.dedup();

    for wrapper in wrappers {
        if profile_learn_target(ctx, wrapper)? != spell_id {
            continue;
        }
        let remaining = PROFILE_TRAINER_OFFERING_LIMIT.saturating_sub(scan.examined);
        if remaining == 0 {
            return Err(profile_limit(format!(
                "spell {spell_id} trainer admission exceeded {PROFILE_TRAINER_OFFERING_LIMIT} offerings"
            )));
        }
        let (wrapper_offerings, offering_overflow) = sorted_offerings(ctx, wrapper, remaining);
        for offering in &wrapper_offerings {
            if scan.inspect(ctx, class, level, offering) {
                return Ok(());
            }
        }
        if offering_overflow {
            return Err(profile_limit(format!(
                "spell {spell_id} trainer admission exceeded {PROFILE_TRAINER_OFFERING_LIMIT} offerings"
            )));
        }
    }
    if trigger_overflow {
        return Err(profile_limit(format!(
            "spell {spell_id} trainer admission exceeded {PROFILE_WRAPPER_EFFECT_LIMIT} reverse trigger effects"
        )));
    }
    Err(scan.refusal(spell_id, class))
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn demo_profile_spell_admitted(class: u8, spell_id: u32) -> bool {
    match class {
        1 => matches!(spell_id, 355 | 6673 | 7386),
        2 => matches!(spell_id, 635 | 853 | 19740 | 20154 | 20271 | 26573),
        5 => matches!(spell_id, 139 | 2050),
        8 => spell_id == 133,
        _ => false,
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn reconcile_profile_spell(
    ctx: &ReducerContext,
    guid: u64,
    spell_id: u32,
) -> Result<bool, crate::actor::ActionRefusal> {
    if crate::spell::knows_spell(ctx, guid, spell_id) {
        return Ok(false);
    }
    let actor = crate::helpers::live_entity(ctx, guid).map_err(|_| {
        crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::MissingActor,
            format!("learner {guid} not in world"),
        )
    })?;
    let spell = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| {
            crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::MissingResource,
                format!("no such spell {spell_id}"),
            )
        })?;
    if actor.level < u32::from(spell.spell_level) {
        return Err(crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::Level,
            format!("spell {spell_id} requires level {}", spell.spell_level),
        ));
    }

    if ctx.db.game_trainer_spell().count() == 0 {
        if !demo_profile_spell_admitted(actor.class(), spell_id) {
            return Err(crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::Class,
                format!(
                    "spell {spell_id} is not available to class {}",
                    actor.class()
                ),
            ));
        }
    } else {
        admit_profile_spell(ctx, spell_id, actor.class(), actor.level)?;
    }
    if let Some(chain) = ctx.db.game_spell_chain().spell_id().find(spell_id) {
        if chain.prev_spell != 0 && !crate::spell::knows_spell(ctx, guid, chain.prev_spell) {
            return Err(crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::Prerequisite,
                format!(
                    "spell {spell_id} requires previous rank {}",
                    chain.prev_spell
                ),
            ));
        }
    }

    crate::spell::learn_spell_with_dependents(ctx, guid, actor.owner_identity, spell_id);
    Ok(true)
}

pub(crate) fn validate_trainer_interaction(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    trainer_guid: u64,
) -> Result<WorldEntity, String> {
    let trainer = crate::npc_interaction::serving_npc(
        ctx,
        caster,
        trainer_guid,
        crate::npc_interaction::NpcService::Trainer,
    )?;
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(trainer.entry)
        .is_some_and(|t| {
            !lyracore_shared::trainer::serves(caster.class(), t.trainer_type, t.trainer_class)
        })
    {
        return Err("trainer does not teach your class".to_string());
    }
    Ok(trainer)
}

pub(crate) fn apply_trainer_buy(
    ctx: &ReducerContext,
    caster_guid: u64,
    trainer_guid: u64,
    spell_id: u32,
) -> Result<(), TrainerRefusal> {
    let entities = ctx.db.game_world_entity();
    let mut caster = entities
        .guid()
        .find(caster_guid)
        .ok_or_else(|| refused(TrainerRefusal::Unavailable, "buyer not in world"))?;
    let trainer = validate_trainer_interaction(ctx, &caster, trainer_guid)
        .map_err(|detail| refused(TrainerRefusal::Unavailable, &detail))?;
    let offered = ctx
        .db
        .game_trainer_spell()
        .by_trainer()
        .filter(&trainer.entry)
        .find(|s| s.spell_id == spell_id)
        .ok_or_else(|| {
            refused(
                TrainerRefusal::NotOffered,
                "trainer does not teach that spell",
            )
        })?;

    let profession_line = offered.learn_skill_line;
    let is_weapon_learn =
        profession_line != 0 && crate::skill::is_combat_skill_line(profession_line);
    let grant = if crate::skill::is_riding_skill_line(profession_line) {
        if ctx
            .db
            .game_creature_template()
            .entry()
            .find(trainer.entry)
            .is_some_and(|t| t.trainer_type != lyracore_shared::trainer::trainer_type::MOUNTS)
        {
            return Err(refused(
                TrainerRefusal::Unavailable,
                "that trainer does not teach riding",
            ));
        }
        BuyGrant::Riding(offered.learn_skill_cap)
    } else if is_weapon_learn {
        BuyGrant::Profession(
            profession_line,
            crate::skill::skill_cap_for_level(caster.level),
        )
    } else if profession_line != 0 {
        BuyGrant::Profession(profession_line, offered.learn_skill_cap)
    } else {
        BuyGrant::Spell(resolve_learn_target(ctx, spell_id))
    };
    let learn_target = match grant {
        BuyGrant::Spell(to_learn) => to_learn,
        _ => spell_id,
    };
    let assessment = Trainee::load(ctx, caster_guid).assess(ctx, &offered, learn_target);
    let state = offering_state(
        assessment.known,
        caster.level,
        u32::from(offered.required_level),
        assessment.gates,
    );
    if let Err(refusal) = trainer_buy_check(state, caster.money, offered.cost) {
        let detail = match refusal {
            TrainerRefusal::AlreadyKnown => "already known".to_string(),
            TrainerRefusal::LevelTooLow => format!("requires level {}", offered.required_level),
            TrainerRefusal::PreviousRankMissing => "requires the previous rank".to_string(),
            TrainerRefusal::SkillTooLow => format!(
                "requires skill {} at {}",
                offered.required_skill, offered.required_skill_value
            ),
            TrainerRefusal::PrimaryProfessionLimit => "no free primary profession slot".to_string(),
            TrainerRefusal::NotEnoughMoney => {
                format!("not enough money (need {})", offered.cost)
            }
            TrainerRefusal::Unavailable | TrainerRefusal::NotOffered => {
                "purchase gate refused the buy".to_string()
            }
        };
        return Err(refused(refusal, &detail));
    }
    caster.money = caster.money.saturating_sub(offered.cost);
    let owner = caster.owner_identity;
    caster.update(ctx);
    match grant {
        BuyGrant::Profession(line, cap) => {
            crate::skill::learn_profession(ctx, caster_guid, owner, line, cap as u16);
            crate::spell::learn_spell_with_dependents(
                ctx,
                caster_guid,
                owner,
                resolve_learn_target(ctx, spell_id),
            );
        }
        BuyGrant::Riding(tier) => crate::skill::learn_riding(ctx, caster_guid, owner, tier as u16),
        BuyGrant::Spell(id) => {
            crate::spell::learn_spell_with_dependents(ctx, caster_guid, owner, id)
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effect_aimed_at_the_caster_pet_is_not_a_learn_effect() {
        use lyracore_shared::spell::taxonomy::{
            E_LEARN_SPELL, E_SCRIPTED, T_CASTER_PET, T_SCRIPTED,
        };
        assert_eq!(learn_kind(E_SCRIPTED, T_CASTER_PET), E_SCRIPTED);
        assert_eq!(learn_kind(E_SCRIPTED, T_SCRIPTED), E_LEARN_SPELL);
        assert_eq!(learn_kind(E_LEARN_SPELL, T_CASTER_PET), E_LEARN_SPELL);
    }

    #[test]
    fn trainer_buy_check_refuses_each_state_with_its_own_refusal() {
        for (state, refusal) in [
            (OfferingState::Known, TrainerRefusal::AlreadyKnown),
            (OfferingState::LevelTooLow, TrainerRefusal::LevelTooLow),
            (
                OfferingState::PreviousRankMissing,
                TrainerRefusal::PreviousRankMissing,
            ),
            (OfferingState::SkillTooLow, TrainerRefusal::SkillTooLow),
            (
                OfferingState::PrimaryProfessionLimit,
                TrainerRefusal::PrimaryProfessionLimit,
            ),
        ] {
            assert_eq!(trainer_buy_check(state, 1000, 10), Err(refusal));
        }
    }

    #[test]
    fn trainer_buy_check_charges_only_an_available_offering_the_buyer_can_afford() {
        assert_eq!(
            trainer_buy_check(OfferingState::Available, 5, 10),
            Err(TrainerRefusal::NotEnoughMoney)
        );
        assert_eq!(trainer_buy_check(OfferingState::Available, 10, 10), Ok(()));
    }

    #[test]
    fn profession_already_capped_compares_stored_max_rank_against_the_offering_cap() {
        assert!(
            !lyracore_shared::trainer::offering_known(185, 75, None, false),
            "never learned -> not capped"
        );
        assert!(
            lyracore_shared::trainer::offering_known(185, 75, Some(75), false),
            "Apprentice re-buy at the same cap is rejected"
        );
        assert!(
            !lyracore_shared::trainer::offering_known(185, 150, Some(75), false),
            "Apprentice-75 has NOT met the Journeyman-150 cap"
        );
        assert!(
            lyracore_shared::trainer::offering_known(185, 150, Some(150), false),
            "a 2nd Journeyman buy (150 >= 150) is rejected"
        );
        assert!(
            !lyracore_shared::trainer::offering_known(185, 75, Some(74), false),
            "one rank below the cap is not yet capped"
        );
    }

    #[test]
    fn a_riding_tier_is_buyable_once_and_the_next_tier_still_sells() {
        assert!(
            !lyracore_shared::trainer::offering_known(762, 75, None, false),
            "an untrained rider can buy Apprentice"
        );
        assert!(
            lyracore_shared::trainer::offering_known(762, 75, Some(75), false),
            "re-buying Apprentice is refused"
        );
        assert!(
            !lyracore_shared::trainer::offering_known(762, 150, Some(75), false),
            "Apprentice-75 has not met the Journeyman-150 tier"
        );
        assert!(
            lyracore_shared::trainer::offering_known(762, 150, Some(150), false),
            "re-buying Journeyman is refused"
        );
    }
}
