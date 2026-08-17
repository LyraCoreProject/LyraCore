//! Hunter-pet XP, level-dependent live stats and loyalty progression.

use spacetimedb::ReducerContext;

use crate::{game_creature_template, game_hunter_pet, game_world_entity, HunterPet};

const MAX_LOYALTY_LEVEL: u8 = 6;
const LOYALTY_LEVEL_UP: [u32; 6] = [5_500, 11_500, 17_000, 23_500, 31_000, 39_500];
const LOYALTY_LEVEL_START: [u32; 6] = [2_000, 4_500, 7_000, 10_000, 13_500, 17_500];

/// The care state T2 passes after updating happiness. Keeping the state explicit leaves happiness
/// thresholds and scheduling in the care module while progression owns only loyalty transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetHappinessState {
    Happy,
    Content,
    Unhappy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PetProgress {
    level: u32,
    xp: u32,
}

/// Vanilla Hunter pets need one quarter of the player XP for the same level transition.
pub fn pet_xp_to_next_level(level: u32) -> u32 {
    crate::xp::xp_to_next_level(level) / 4
}

fn advance_pet(mut pet: PetProgress, owner_level: u32, award: u32) -> PetProgress {
    let cap = owner_level.min(60);
    if pet.level > cap {
        pet.level = cap;
        pet.xp = 0;
        return pet;
    }
    if award == 0 || pet.level >= cap {
        if pet.level >= cap {
            pet.xp = 0;
        }
        return pet;
    }

    let mut available = u64::from(pet.xp) + u64::from(award);
    while pet.level < cap {
        let threshold = u64::from(pet_xp_to_next_level(pet.level));
        if threshold == 0 || available < threshold {
            break;
        }
        available -= threshold;
        pet.level += 1;
    }
    pet.xp = if pet.level < cap {
        available.min(u64::from(u32::MAX)) as u32
    } else {
        0
    };
    pet
}

fn loyalty_after_delta(level: u8, progress: u32, delta: i32) -> (u8, u32) {
    let mut level = level.clamp(1, MAX_LOYALTY_LEVEL);
    let mut progress = progress.min(LOYALTY_LEVEL_UP[(level - 1) as usize]);

    if delta < 0 {
        let loss = delta.unsigned_abs();
        if loss > progress {
            if level == 1 {
                return (1, 0);
            }
            level -= 1;
            return (level, LOYALTY_LEVEL_START[(level - 1) as usize]);
        }
        return (level, progress - loss);
    }

    progress = progress.saturating_add(delta as u32);
    while level < MAX_LOYALTY_LEVEL && progress > LOYALTY_LEVEL_UP[(level - 1) as usize] {
        level += 1;
        progress = LOYALTY_LEVEL_START[(level - 1) as usize];
    }
    let ceiling = LOYALTY_LEVEL_UP[(level - 1) as usize];
    (level, progress.min(ceiling))
}

fn care_loyalty_delta(state: PetHappinessState) -> i32 {
    match state {
        PetHappinessState::Happy => 20,
        PetHappinessState::Content => 10,
        PetHappinessState::Unhappy => -20,
    }
}

fn kill_loyalty_bonus(pet_level: u32, loyalty_level: u8) -> u32 {
    if pet_level > 100 {
        return 0;
    }
    ((100 - pet_level) / 10) + u32::from(MAX_LOYALTY_LEVEL - loyalty_level.min(6))
}

/// Apply one care tick's loyalty delta to a durable Hunter pet. T2 owns when the tick happens and
/// how happiness maps to the explicit state; this function owns all threshold and bound behavior.
pub fn update_pet_loyalty_from_care(
    ctx: &ReducerContext,
    pet_id: u64,
    state: PetHappinessState,
) -> bool {
    let pets = ctx.db.game_hunter_pet();
    let Some(mut pet) = pets.pet_id().find(pet_id) else {
        return false;
    };
    let next = loyalty_after_delta(
        pet.loyalty_level,
        pet.loyalty_progress,
        care_loyalty_delta(state),
    );
    if next == (pet.loyalty_level, pet.loyalty_progress) {
        return false;
    }
    pet.loyalty_level = next.0;
    pet.loyalty_progress = next.1;
    pet.loyalty_updated_at = ctx.timestamp;
    pets.pet_id().update(pet);
    true
}

fn refreshed_pet_health(
    current_health: u32,
    current_max_health: u32,
    base_level: u32,
    base_health: u32,
    level: u32,
) -> (u32, u32) {
    let old_max = current_max_health.max(1);
    let new_max = if base_level == 0 {
        old_max
    } else {
        ((u64::from(base_health.max(1)) * u64::from(level.max(1))) / u64::from(base_level))
            .clamp(1, u64::from(u32::MAX)) as u32
    };
    let health = (u64::from(current_health) * u64::from(new_max))
        .div_ceil(u64::from(old_max))
        .clamp(1, u64::from(new_max)) as u32;
    (health, new_max)
}

fn refresh_live_level(ctx: &ReducerContext, pet: &HunterPet, level: u32) {
    let entities = ctx.db.game_world_entity();
    let Some(mut live) = super::pet_of(ctx, pet.owner_guid) else {
        return;
    };
    if !eligible_live_pet(pet.pet_id, super::live_pet_kind(ctx, live.guid), live.dead) {
        return;
    }
    let Some(template) = ctx
        .db
        .game_creature_template()
        .entry()
        .find(pet.creature_entry)
    else {
        live.level = level;
        entities.guid().update(live);
        return;
    };
    let base_health = super::scale_health_for_rank(template.health, template.rank);
    let (health, max_health) = refreshed_pet_health(
        live.health,
        live.max_health,
        template.level,
        base_health,
        level,
    );
    live.level = level;
    live.health = health;
    live.max_health = max_health;
    entities.guid().update(live);
}

fn eligible_live_pet(pet_id: u64, kind: super::PetKind, dead: bool) -> bool {
    !dead && kind == (super::PetKind::Hunter { pet_id })
}

/// Advance the active Hunter pet belonging to one authoritative kill-credit recipient. Summoned,
/// dead, missing and mismatched live pets are excluded before any durable write, and a kill gray
/// to the owner awards nothing.
pub(crate) fn award_hunter_pet_kill_progression(
    ctx: &ReducerContext,
    owner_guid: u64,
    mob_level: u32,
    rank: u8,
) {
    let Some(mut pet) = super::hunter_pet_for_owner(ctx, owner_guid) else {
        return;
    };
    let Some(live) = super::pet_of(ctx, owner_guid) else {
        return;
    };
    if !eligible_live_pet(pet.pet_id, super::live_pet_kind(ctx, live.guid), live.dead) {
        return;
    }
    let Some(owner) = ctx.db.game_world_entity().guid().find(owner_guid) else {
        return;
    };
    // Pet XP shares the owner's gray clamp: a kill worth no XP to the owner advances no pet.
    if crate::xp::xp_for_kill(mob_level, owner.level) == 0 {
        return;
    }
    let raw = crate::xp::xp_for_kill(mob_level, pet.level)
        .saturating_mul(crate::xp::rank_xp_multiplier(rank));
    let award = crate::xp::rated_xp(ctx, raw);
    if award == 0 {
        return;
    }

    let next = advance_pet(
        PetProgress {
            level: pet.level,
            xp: pet.pet_xp,
        },
        owner.level,
        award,
    );
    let loyalty = loyalty_after_delta(
        pet.loyalty_level,
        pet.loyalty_progress,
        kill_loyalty_bonus(next.level, pet.loyalty_level) as i32,
    );
    let level_changed = next.level != pet.level;
    pet.level = next.level;
    pet.pet_xp = next.xp;
    pet.loyalty_level = loyalty.0;
    pet.loyalty_progress = loyalty.1;
    pet.loyalty_updated_at = ctx.timestamp;
    if level_changed {
        refresh_live_level(ctx, &pet, next.level);
    }
    ctx.db.game_hunter_pet().pet_id().update(pet.clone());
    super::publish_hunter_pet_protocol(ctx, &pet, live.guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_curve_is_one_quarter_of_the_player_curve() {
        assert_eq!(pet_xp_to_next_level(1), 100);
        assert_eq!(pet_xp_to_next_level(10), 1_900);
        assert_eq!(pet_xp_to_next_level(60), 0);
    }

    #[test]
    fn exact_threshold_and_multi_level_awards_advance_deterministically() {
        assert_eq!(
            advance_pet(PetProgress { level: 1, xp: 0 }, 10, 100),
            PetProgress { level: 2, xp: 0 }
        );
        assert_eq!(
            advance_pet(PetProgress { level: 1, xp: 90 }, 10, 360),
            PetProgress { level: 3, xp: 125 }
        );
    }

    #[test]
    fn owner_cap_clears_xp_and_overflow_cannot_wrap() {
        assert_eq!(
            advance_pet(PetProgress { level: 2, xp: 224 }, 3, u32::MAX),
            PetProgress { level: 3, xp: 0 }
        );
        assert_eq!(
            advance_pet(
                PetProgress {
                    level: 3,
                    xp: u32::MAX
                },
                3,
                1
            ),
            PetProgress { level: 3, xp: 0 }
        );
        assert_eq!(
            advance_pet(PetProgress { level: 4, xp: 100 }, 3, 1),
            PetProgress { level: 3, xp: 0 }
        );
    }

    #[test]
    fn care_states_cross_and_drop_loyalty_thresholds() {
        assert_eq!(care_loyalty_delta(PetHappinessState::Happy), 20);
        assert_eq!(care_loyalty_delta(PetHappinessState::Content), 10);
        assert_eq!(care_loyalty_delta(PetHappinessState::Unhappy), -20);
        assert_eq!(loyalty_after_delta(1, 5_500, 10), (2, 4_500));
        assert_eq!(loyalty_after_delta(2, 10, -20), (1, 2_000));
    }

    #[test]
    fn loyalty_exact_threshold_waits_and_both_bounds_saturate() {
        assert_eq!(loyalty_after_delta(1, 5_490, 10), (1, 5_500));
        assert_eq!(loyalty_after_delta(1, 0, -20), (1, 0));
        assert_eq!(loyalty_after_delta(6, 39_490, 20), (6, 39_500));
    }

    #[test]
    fn level_refresh_preserves_health_proportion_and_grows_maximum() {
        assert_eq!(refreshed_pet_health(50, 100, 10, 100, 12), (60, 120));
    }

    #[test]
    fn only_the_matching_live_hunter_pet_is_eligible() {
        assert!(eligible_live_pet(
            42,
            super::super::PetKind::Hunter { pet_id: 42 },
            false
        ));
        assert!(!eligible_live_pet(
            42,
            super::super::PetKind::Summoned,
            false
        ));
        assert!(!eligible_live_pet(
            42,
            super::super::PetKind::Hunter { pet_id: 42 },
            true
        ));
    }

    #[test]
    fn pet_award_is_gated_on_the_owner_gray_clamp() {
        let body = crate::test_scan::code_of(
            include_str!("pet_progression.rs"),
            "pub(crate) fn award_hunter_pet_kill_progression(",
        );
        assert!(
            body.contains("xp_for_kill(mob_level, owner.level)"),
            "pet XP must reuse the owner's kill-eligibility clamp"
        );
    }

    #[test]
    fn kill_credit_recipients_are_the_only_pet_award_seam() {
        let body = crate::test_scan::code_of(
            include_str!("../combat/death.rs"),
            "fn award_killer_rewards(",
        );
        let recipient_loop = body
            .find("for recipient in &recipients")
            .expect("kill rewards still iterate authoritative recipients");
        let pet_award = body
            .find("award_hunter_pet_kill_progression(ctx, *recipient")
            .expect("pet progression remains inside the shared kill award");
        assert!(pet_award > recipient_loop);
        assert_eq!(
            body.matches("award_hunter_pet_kill_progression(").count(),
            1
        );
    }
}
