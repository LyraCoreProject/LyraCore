//! Hunter-pet feeding, happiness decay and white-swing damage scaling.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{
    game_creature_family, game_hunter_pet, game_item_instance, game_item_template,
    game_live_pet_kind, game_spell_effect, game_world_entity, HunterPet, WorldEntity,
};

/// One Vanilla happiness band. The three visible states are `[0, band)`, `[band, 2*band)` and
/// `[2*band, max]`.
pub const HAPPINESS_BAND: u32 = 333_000;
pub const MAX_HAPPINESS: u32 = 1_000_000;
pub const CARE_INTERVAL_MICROS: i64 = 7_500_000;

/// The single global care clock. Each durable pet carries its own last-applied timestamp, so a late
/// firing catches up by whole intervals and produces the same result as regular firings.
#[table(accessor = game_pet_care_schedule, scheduled(tick_pet_care))]
pub struct PetCareSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HappinessState {
    Unhappy,
    Content,
    Happy,
}

pub fn happiness_state(value: u32) -> HappinessState {
    if value < HAPPINESS_BAND {
        HappinessState::Unhappy
    } else if value < HAPPINESS_BAND * 2 {
        HappinessState::Content
    } else {
        HappinessState::Happy
    }
}

fn happiness_damage_pct(value: u32) -> u32 {
    match happiness_state(value) {
        HappinessState::Unhappy => 75,
        HappinessState::Content => 100,
        HappinessState::Happy => 125,
    }
}

fn scale_damage(range: (u32, u32), happiness: u32) -> (u32, u32) {
    let pct = happiness_damage_pct(happiness);
    (range.0 * pct / 100, range.1 * pct / 100)
}

/// Apply happiness only to the live white-swing range of a durable Hunter pet. Missing and explicit
/// summoned kinds are intentionally identical to ordinary creatures.
pub(crate) fn scale_hunter_pet_swing(
    ctx: &ReducerContext,
    attacker_guid: u64,
    range: (u32, u32),
) -> (u32, u32) {
    let Some(kind) = ctx
        .db
        .game_live_pet_kind()
        .pet_guid()
        .find(attacker_guid)
        .filter(|row| row.hunter_pet_id != 0)
    else {
        return range;
    };
    let Some(pet) = ctx.db.game_hunter_pet().pet_id().find(kind.hunter_pet_id) else {
        return range;
    };
    scale_damage(range, pet.happiness)
}

fn decay_per_interval(loyalty_level: u8, in_combat: bool) -> u32 {
    // CMaNGOS: (140 >> loyalty) * 125 each 7.5s, with its documented 1.5x combat estimate.
    let normal = (140u32 >> loyalty_level.min(7)) * 125;
    if in_combat {
        normal.saturating_mul(3) / 2
    } else {
        normal
    }
}

/// Returns `(new happiness, whole elapsed intervals consumed)`. Remainders stay on the durable
/// timestamp so many regular calls and one catch-up call are exactly equivalent.
#[cfg(test)]
fn decay_elapsed(
    happiness: u32,
    loyalty_level: u8,
    elapsed_micros: i64,
    in_combat: bool,
) -> (u32, i64) {
    let intervals = elapsed_micros.max(0) / CARE_INTERVAL_MICROS;
    let loss = u64::from(decay_per_interval(loyalty_level, in_combat))
        .saturating_mul(intervals as u64)
        .min(u64::from(u32::MAX)) as u32;
    (happiness.saturating_sub(loss), intervals)
}

fn update_care(ctx: &ReducerContext, mut pet: HunterPet) {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let last = pet.care_updated_at.to_micros_since_unix_epoch();
    let Some(live) = super::pet_of(ctx, pet.owner_guid).filter(|live| {
        ctx.db
            .game_live_pet_kind()
            .pet_guid()
            .find(live.guid)
            .is_some_and(|kind| kind.hunter_pet_id == pet.pet_id)
    }) else {
        // Care only advances while the pet is materialized. Reset the anchor so an offline/dead pet
        // does not accrue a surprise catch-up penalty when it next appears.
        pet.care_updated_at = ctx.timestamp;
        ctx.db.game_hunter_pet().pet_id().update(pet);
        return;
    };
    let in_combat = live.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0;
    let intervals = (now - last).max(0) / CARE_INTERVAL_MICROS;
    if intervals == 0 {
        return;
    }
    let pet_id = pet.pet_id;
    let live_guid = live.guid;
    for interval in 1..=intervals {
        let old_happiness = pet.happiness;
        pet.happiness = pet
            .happiness
            .saturating_sub(decay_per_interval(pet.loyalty_level, in_combat));
        pet.care_updated_at = Timestamp::from_micros_since_unix_epoch(
            last.saturating_add(interval.saturating_mul(CARE_INTERVAL_MICROS)),
        );
        ctx.db.game_hunter_pet().pet_id().update(pet.clone());

        let state = match happiness_state(pet.happiness) {
            HappinessState::Happy => super::PetHappinessState::Happy,
            HappinessState::Content => super::PetHappinessState::Content,
            HappinessState::Unhappy => super::PetHappinessState::Unhappy,
        };
        let loyalty_changed = super::update_pet_loyalty_from_care(ctx, pet_id, state);
        pet = ctx
            .db
            .game_hunter_pet()
            .pet_id()
            .find(pet_id)
            .expect("care pet remains present during its transaction");

        // At both lower bounds every later interval is the same no-op. Advance the time anchor in
        // one write instead of looping over an arbitrarily long scheduler outage.
        if pet.happiness == old_happiness && !loyalty_changed {
            pet.care_updated_at = Timestamp::from_micros_since_unix_epoch(
                last.saturating_add(intervals.saturating_mul(CARE_INTERVAL_MICROS)),
            );
            ctx.db.game_hunter_pet().pet_id().update(pet.clone());
            break;
        }
    }
    super::publish_hunter_pet_protocol(ctx, &pet, live_guid);
}

#[reducer]
pub fn tick_pet_care(ctx: &ReducerContext, _schedule: PetCareSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    for pet in ctx.db.game_hunter_pet().iter().collect::<Vec<_>>() {
        update_care(ctx, pet);
    }
}

fn food_mask(food_type: u8) -> u32 {
    food_type
        .checked_sub(1)
        .filter(|bit| *bit < 32)
        .map_or(0, |bit| 1u32 << bit)
}

fn food_benefit(pet_level: u32, item_level: u8) -> u32 {
    let gap = pet_level.saturating_sub(u32::from(item_level));
    match gap {
        0..=5 => 35_000,
        6..=10 => 17_000,
        11..=14 => 8_000,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FeedEligibility {
    actor_is_player: bool,
    item_owned: bool,
    pet_exists: bool,
    pet_owned: bool,
    pet_is_hunter: bool,
    pet_alive: bool,
    pet_in_combat: bool,
    diet_accepts: bool,
    benefit: u32,
}

impl FeedEligibility {
    fn validate(self) -> Result<u32, &'static str> {
        if !self.actor_is_player || !self.item_owned {
            return Err("food is not owned by the acting player");
        }
        if !self.pet_exists {
            return Err("the player has no live pet");
        }
        if !self.pet_owned || !self.pet_is_hunter {
            return Err("only the owner's Hunter pet can be fed");
        }
        if !self.pet_alive {
            return Err("a dead pet cannot be fed");
        }
        if self.pet_in_combat {
            return Err("a pet in combat cannot be fed");
        }
        if !self.diet_accepts {
            return Err("the pet will not eat that food");
        }
        if self.benefit == 0 {
            return Err("the food is too low level for the pet");
        }
        Ok(self.benefit)
    }
}

/// Resolve a generic item-target spell. The spell effect kind, never its id, selects feeding; the
/// target slot names the exact stack consumed after every pet and food gate succeeds.
pub(crate) fn apply_item_target_spell(
    ctx: &ReducerContext,
    actor_guid: u64,
    spell_id: u32,
    slot: u8,
) -> Result<(), String> {
    if !ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|effect| effect.kind == crate::spell::E_FEED_PET)
    {
        return Err("spell does not support an item target".to_string());
    }
    feed_item(ctx, actor_guid, slot)
}

fn feed_item(ctx: &ReducerContext, actor_guid: u64, slot: u8) -> Result<(), String> {
    let actor = ctx.db.game_world_entity().guid().find(actor_guid);
    let item = crate::items::item_in_slot(ctx, actor_guid, slot);
    let template = item
        .as_ref()
        .and_then(|item| ctx.db.game_item_template().entry().find(item.entry));
    let live = super::pet_of(ctx, actor_guid);
    let kind = live
        .as_ref()
        .and_then(|live| ctx.db.game_live_pet_kind().pet_guid().find(live.guid));
    let durable = kind.as_ref().and_then(|kind| {
        (kind.hunter_pet_id != 0)
            .then(|| ctx.db.game_hunter_pet().pet_id().find(kind.hunter_pet_id))
            .flatten()
    });
    let family = durable.as_ref().and_then(|pet| {
        ctx.db
            .game_creature_family()
            .family_id()
            .find(pet.creature_family)
    });
    let benefit = durable
        .as_ref()
        .zip(template.as_ref())
        .map_or(0, |(pet, item)| food_benefit(pet.level, item.item_level));
    let eligibility = FeedEligibility {
        actor_is_player: actor.as_ref().is_some_and(WorldEntity::is_player),
        item_owned: item.as_ref().is_some_and(|item| {
            item.owner_guid == actor_guid && crate::items::is_carried_slot(item.slot)
        }),
        pet_exists: live.is_some(),
        pet_owned: live
            .as_ref()
            .is_some_and(|pet| pet.owner_guid == actor_guid),
        pet_is_hunter: durable
            .as_ref()
            .is_some_and(|pet| pet.owner_guid == actor_guid),
        pet_alive: live.as_ref().is_some_and(|pet| !pet.dead),
        pet_in_combat: live.as_ref().is_some_and(|pet| {
            pet.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
        }),
        diet_accepts: family
            .as_ref()
            .zip(template.as_ref())
            .is_some_and(|(family, item)| {
                family.pet_food_mask as u32 & food_mask(item.food_type) != 0
            }),
        benefit,
    };
    let benefit = eligibility.validate().map_err(str::to_string)?;

    let mut item = item.expect("validated item exists");
    let items = ctx.db.game_item_instance();
    if item.stack_count > 1 {
        item.stack_count -= 1;
        items.guid().update(item);
    } else {
        items.guid().delete(item.guid);
    }
    let mut pet = durable.expect("validated durable pet exists");
    pet.happiness = pet.happiness.saturating_add(benefit).min(MAX_HAPPINESS);
    ctx.db.game_hunter_pet().pet_id().update(pet.clone());
    super::publish_hunter_pet_protocol(ctx, &pet, live.expect("validated live pet exists").guid);
    Ok(())
}

/// Hunter pets lose one full happiness band at the shared pet-death chokepoint. The live entity is
/// deleted immediately afterwards, so this can run only once for that death.
pub(crate) fn on_pet_death(ctx: &ReducerContext, pet_guid: u64) {
    let Some(kind) = ctx
        .db
        .game_live_pet_kind()
        .pet_guid()
        .find(pet_guid)
        .filter(|kind| kind.hunter_pet_id != 0)
    else {
        return;
    };
    if let Some(mut pet) = ctx.db.game_hunter_pet().pet_id().find(kind.hunter_pet_id) {
        pet.happiness = pet.happiness.saturating_sub(HAPPINESS_BAND);
        ctx.db.game_hunter_pet().pet_id().update(pet.clone());
        super::publish_hunter_pet_protocol(ctx, &pet, pet_guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligible() -> FeedEligibility {
        FeedEligibility {
            actor_is_player: true,
            item_owned: true,
            pet_exists: true,
            pet_owned: true,
            pet_is_hunter: true,
            pet_alive: true,
            pet_in_combat: false,
            diet_accepts: true,
            benefit: 35_000,
        }
    }

    #[test]
    fn every_feed_gate_refuses_before_consumption() {
        let cases: Vec<Box<dyn Fn(&mut FeedEligibility)>> = vec![
            Box::new(|v| v.actor_is_player = false),
            Box::new(|v| v.item_owned = false),
            Box::new(|v| v.pet_exists = false),
            Box::new(|v| v.pet_owned = false),
            Box::new(|v| v.pet_is_hunter = false),
            Box::new(|v| v.pet_alive = false),
            Box::new(|v| v.pet_in_combat = true),
            Box::new(|v| v.diet_accepts = false),
            Box::new(|v| v.benefit = 0),
        ];
        for change in cases {
            let mut value = eligible();
            change(&mut value);
            assert!(value.validate().is_err(), "invalid feed passed: {value:?}");
        }
    }

    #[test]
    fn food_benefits_match_all_level_bands() {
        assert_eq!(food_benefit(60, 55), 35_000);
        assert_eq!(food_benefit(60, 50), 17_000);
        assert_eq!(food_benefit(60, 46), 8_000);
        assert_eq!(food_benefit(60, 45), 0);
    }

    #[test]
    fn happiness_boundaries_drive_exact_white_damage_ratios() {
        assert_eq!(scale_damage((400, 400), HAPPINESS_BAND - 1), (300, 300));
        assert_eq!(scale_damage((400, 400), HAPPINESS_BAND), (400, 400));
        assert_eq!(scale_damage((400, 400), HAPPINESS_BAND * 2), (500, 500));
    }

    #[test]
    fn catch_up_equals_regular_firings_and_saturates_at_zero() {
        let start = MAX_HAPPINESS;
        let (catch_up, ticks) = decay_elapsed(start, 1, CARE_INTERVAL_MICROS * 9, false);
        let mut regular = start;
        for _ in 0..9 {
            regular = decay_elapsed(regular, 1, CARE_INTERVAL_MICROS, false).0;
        }
        assert_eq!(ticks, 9);
        assert_eq!(catch_up, regular);
        assert_eq!(decay_elapsed(1, 1, CARE_INTERVAL_MICROS, false).0, 0);
    }

    #[test]
    fn decay_preserves_partial_intervals_and_combat_is_faster() {
        assert_eq!(
            decay_elapsed(100_000, 1, CARE_INTERVAL_MICROS - 1, false),
            (100_000, 0)
        );
        assert!(
            decay_elapsed(100_000, 1, CARE_INTERVAL_MICROS, true).0
                < decay_elapsed(100_000, 1, CARE_INTERVAL_MICROS, false).0
        );
    }

    #[test]
    fn food_type_maps_to_the_family_mask() {
        assert_eq!(food_mask(0), 0);
        assert_eq!(food_mask(1), 1);
        assert_eq!(food_mask(4), 8);
    }

    #[test]
    fn scheduled_care_updates_loyalty_and_the_owner_protocol_projection() {
        let body = crate::test_scan::code_of(include_str!("pet_care.rs"), "fn update_care(");
        assert!(body.contains("update_pet_loyalty_from_care"));
        assert!(body.contains("publish_hunter_pet_protocol"));
    }
}
