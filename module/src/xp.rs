use spacetimedb::{log, table, Identity, ReducerContext, Timestamp};

use crate::game_character;
use crate::game_config;
use crate::game_world_entity;
use crate::WorldEntity;

const LEVEL_CAP: u32 = 60;

#[table(accessor = game_xp_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct XpEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub killed_guid: u64,
    pub total_exp: u32,
    pub created_at: Timestamp,
    #[default(true)]
    pub is_kill: bool,
    #[default(0u32)]
    pub rested_bonus: u32,
}

#[table(accessor = game_levelup_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct LevelupEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub new_level: u32,
    pub health_gained: u32,
    pub created_at: Timestamp,
    #[default(0)]
    pub mana_gained: u32,
    #[default(0)]
    pub strength_gained: u32,
    #[default(0)]
    pub agility_gained: u32,
    #[default(0)]
    pub stamina_gained: u32,
    #[default(0)]
    pub intellect_gained: u32,
    #[default(0)]
    pub spirit_gained: u32,
}

impl XpEvent {
    pub(crate) fn send(ctx: &ReducerContext, guid: u64, event: Self) {
        crate::native::xp(ctx, guid, &event);
    }
}

impl LevelupEvent {
    fn send(ctx: &ReducerContext, guid: u64, event: Self) {
        crate::native::levelup(ctx, guid, &event);
    }
}

pub(crate) fn xp_to_next_level(level: u32) -> u32 {
    const XP_PER_LEVEL: [u32; 59] = [
        400, 900, 1400, 2100, 2800, 3600, 4500, 5400, 6500, 7600, 8800, 10100, 11400, 12900, 14400,
        16000, 17700, 19400, 21300, 23200, 25200, 27300, 29400, 31700, 34000, 36400, 38900, 41400,
        44300, 47400, 50800, 54500, 58600, 62800, 67100, 71600, 76100, 80800, 85700, 90700, 95800,
        101000, 106300, 111800, 117500, 123200, 129100, 135100, 141200, 147500, 153900, 160400,
        167100, 173900, 180800, 187900, 195000, 202300, 209800,
    ];
    if level == 0 || level >= LEVEL_CAP {
        return 0;
    }
    XP_PER_LEVEL[(level - 1) as usize]
}

pub(crate) fn max_health_for_level(level: u32) -> u32 {
    60 + 15 * level.saturating_sub(1)
}

pub(crate) fn xp_for_kill(mob_level: u32, player_level: u32) -> u32 {
    if mob_level + 5 < player_level {
        return 0;
    }
    let base = mob_level * 5 + 45;
    if mob_level > player_level {
        base * 6 / 5
    } else {
        base
    }
}

#[derive(Debug, Eq, PartialEq)]
struct XpProgression {
    level: u32,
    xp: u32,
    next_level_xp: u32,
}

fn progression_after_award(
    level: u32,
    current_xp: u32,
    next_level_xp: u32,
    amount: u64,
) -> XpProgression {
    if level >= LEVEL_CAP {
        return XpProgression {
            level,
            xp: 0,
            next_level_xp: 0,
        };
    }

    let mut level = level;
    let mut bank = current_xp as u64 + amount;
    let mut next = next_level_xp;
    while next > 0 && bank >= next as u64 {
        bank -= next as u64;
        level += 1;
        if level >= LEVEL_CAP {
            return XpProgression {
                level,
                xp: 0,
                next_level_xp: 0,
            };
        }
        next = xp_to_next_level(level);
    }

    XpProgression {
        level,
        xp: bank.min(u32::MAX as u64) as u32,
        next_level_xp: next,
    }
}

pub(crate) fn rank_xp_multiplier(rank: u8) -> u32 {
    match rank {
        1..=3 => 2,
        _ => 1,
    }
}

fn xp_rate(ctx: &ReducerContext) -> f32 {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.xp_rate)
        .unwrap_or(1.0)
}

pub(crate) fn split_kill_xp(base: u32, share_count: u32) -> u32 {
    base / share_count.max(1)
}

pub(crate) fn rated_xp(ctx: &ReducerContext, base: u32) -> u32 {
    (base as f32 * xp_rate(ctx)).round() as u32
}

pub(crate) fn explore_base_xp(level: u32) -> u32 {
    level.saturating_mul(5).saturating_add(45)
}

const EXPLORE_LEVEL_BAND: i32 = 5;

const EXPLORE_GREY_TAPER_PCT_PER_LEVEL: i32 = 5;

fn explore_grey_taper_pct(levels_above_area: i32) -> u32 {
    (100 - (levels_above_area - EXPLORE_LEVEL_BAND) * EXPLORE_GREY_TAPER_PCT_PER_LEVEL)
        .clamp(0, 100) as u32
}

pub(crate) fn explore_xp(exploration_level: u32, player_level: u32) -> u32 {
    if player_level >= 60 || exploration_level == 0 {
        return 0;
    }
    let area_award = explore_base_xp(exploration_level);
    match player_level as i32 - exploration_level as i32 {
        under if under < -EXPLORE_LEVEL_BAND => {
            explore_base_xp(player_level + EXPLORE_LEVEL_BAND as u32)
        }
        over if over > EXPLORE_LEVEL_BAND => area_award * explore_grey_taper_pct(over) / 100,
        _ => area_award,
    }
}

const REST_ACCRUE_PERCENT_PER_8H: u64 = 5;
const REST_EIGHT_HOURS_MICROS: u64 = 8 * 3600 * 1_000_000;
const REST_CAP_PERMILLE: u64 = 1500;

fn rest_bonus(base_xp: u32, pool: u32) -> (u32, u32) {
    let bonus = base_xp.min(pool);
    (base_xp + bonus, bonus)
}

fn rest_pool_after(pool: u32, offline_micros: u64, level: u32) -> u32 {
    let level_xp = xp_to_next_level(level) as u64;
    if level_xp == 0 || offline_micros == 0 {
        return pool;
    }
    // Widen before multiplying; long offline spans overflow u64 before the rested cap applies.
    let accrued =
        ((level_xp as u128) * (REST_ACCRUE_PERCENT_PER_8H as u128) * (offline_micros as u128)
            / (100 * REST_EIGHT_HOURS_MICROS as u128)) as u64;
    let cap = level_xp * REST_CAP_PERMILLE / 1000;
    (pool as u64 + accrued).min(cap) as u32
}

pub(crate) fn rest_accrue_live(pool: u32, elapsed_micros: u64, level: u32) -> u32 {
    rest_pool_after(pool, elapsed_micros, level)
}

pub(crate) fn accrue_rested_on_login(
    now_micros: i64,
    last_logout_micros: u64,
    pool: u32,
    level: u32,
    was_resting: bool,
) -> (u32, bool) {
    if last_logout_micros == 0 {
        return (pool, false);
    }
    let offline = (now_micros as u64).saturating_sub(last_logout_micros);
    let effective = if was_resting { offline } else { offline / 4 };
    (rest_pool_after(pool, effective, level), true)
}

pub(crate) fn award_xp(
    ctx: &ReducerContext,
    attacker_guid: u64,
    killed_guid: u64,
    mob_level: u32,
    rank: u8,
    share_count: u32,
) {
    let entities = ctx.db.game_world_entity();
    let Some(mut p) = entities.guid().find(attacker_guid) else {
        return;
    };
    if p.level >= LEVEL_CAP {
        return;
    }
    let base = split_kill_xp(
        rated_xp(
            ctx,
            xp_for_kill(mob_level, p.level) * rank_xp_multiplier(rank),
        ),
        share_count,
    );
    if base == 0 {
        return;
    }
    let chars = ctx.db.game_character();
    let award = match chars.guid().find(attacker_guid) {
        Some(mut c) if c.rested_xp > 0 => {
            let (granted, drained) = rest_bonus(base, c.rested_xp);
            c.rested_xp = c.rested_xp.saturating_sub(drained);
            chars.guid().update(c);
            granted
        }
        Some(_) => base,
        None => {
            log::warn!(
                "award_xp: player entity {attacker_guid} has no character row — rested XP skipped"
            );
            base
        }
    };
    XpEvent::send(
        ctx,
        attacker_guid,
        XpEvent {
            id: 0,
            recipient_identity: p.owner_identity,
            killed_guid,
            total_exp: award,
            created_at: ctx.timestamp,
            is_kill: true,
            rested_bonus: award - base,
        },
    );
    grant_xp(ctx, &mut p, award);
    p.update(ctx);
}

pub(crate) fn grant_xp(ctx: &ReducerContext, p: &mut WorldEntity, amount: u32) {
    if p.level >= LEVEL_CAP {
        p.xp = 0;
        p.next_level_xp = 0;
        return;
    }
    if amount == 0 {
        return;
    }
    let amount = (amount as u64 * crate::gm::xprate_bp(ctx) as u64) / 10_000;
    if amount == 0 {
        return;
    }
    let progression = progression_after_award(p.level, p.xp, p.next_level_xp, amount);
    p.xp = progression.xp;
    let race = p.race();
    let class = p.class();
    let mut leveled = false;
    while p.level < progression.level {
        leveled = true;
        p.level += 1;
        crate::skill::raise_combat_caps(ctx, p.guid, p.level);
        let delta = crate::stats::apply_level_stats(ctx, p, race, class, p.level);
        let strength_gained = p.strength.saturating_sub(delta.old_strength);
        let agility_gained = p.agility.saturating_sub(delta.old_agility);
        let stamina_gained = p.stamina.saturating_sub(delta.old_stamina);
        let intellect_gained = p.intellect.saturating_sub(delta.old_intellect);
        let spirit_gained = p.spirit.saturating_sub(delta.old_spirit);
        let gained = p.max_health.saturating_sub(delta.old_max_health);
        p.health = p.max_health;
        let mana_gained = if lyracore_shared::packing::power_type::for_class(class)
            == lyracore_shared::packing::power_type::MANA
        {
            p.max_power.saturating_sub(delta.old_max_power)
        } else {
            0
        };
        if lyracore_shared::packing::power_type::for_class(class)
            == lyracore_shared::packing::power_type::MANA
        {
            p.power = p.max_power;
        }
        p.next_level_xp = xp_to_next_level(p.level);
        LevelupEvent::send(
            ctx,
            p.guid,
            LevelupEvent {
                id: 0,
                recipient_identity: p.owner_identity,
                new_level: p.level,
                health_gained: gained,
                created_at: ctx.timestamp,
                mana_gained,
                strength_gained,
                agility_gained,
                stamina_gained,
                intellect_gained,
                spirit_gained,
            },
        );
        // Level-up hooks read the payload; the entity row is persisted only after this loop.
        crate::hooks::fire_on_levelup(
            ctx,
            &crate::hooks::LevelupPayload {
                character_guid: p.guid,
                new_level: p.level,
            },
        );
    }
    p.next_level_xp = progression.next_level_xp;
    if leveled {
        // Persist level and stat changes before recompute_sheet re-reads the entity.
        let entities = ctx.db.game_world_entity();
        if let Some(placeholder) = entities.guid().find(p.guid) {
            let moved = std::mem::replace(p, placeholder);
            moved.update(ctx);
            crate::spell::recompute_sheet(ctx, p.guid);
            if let Some(fresh) = entities.guid().find(p.guid) {
                *p = fresh;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accrue_rested_on_login, explore_base_xp, explore_xp, max_health_for_level,
        progression_after_award, rank_xp_multiplier, rest_bonus, rest_pool_after, split_kill_xp,
        xp_for_kill, xp_to_next_level, XpProgression, REST_EIGHT_HOURS_MICROS,
    };

    #[test]
    fn explore_xp_scales_off_area_level_and_caps() {
        assert_eq!(explore_xp(10, 10), 95);
        assert_eq!(explore_xp(10, 12), 95);
        assert_eq!(explore_xp(20, 10), explore_base_xp(15));
        assert_eq!(explore_xp(10, 16), 95 * 95 / 100);
        assert_eq!(explore_xp(2, 30), 0);
        assert_eq!(explore_xp(10, 60), 0);
        assert_eq!(explore_xp(0, 10), 0);
    }

    #[test]
    fn kill_xp_pays_base_greys_out_and_adds_20pct_for_an_out_leveling_mob() {
        assert_eq!(xp_for_kill(1, 1), 50);
        assert_eq!(xp_for_kill(10, 10), 95);
        assert_eq!(xp_for_kill(60, 59), 414);
        assert_eq!(xp_for_kill(60, 60), 345);
        assert_eq!(xp_for_kill(5, 10), 70);
        assert_eq!(xp_for_kill(4, 10), 0);
        assert_eq!(xp_for_kill(1, 60), 0);
        assert_eq!(xp_for_kill(11, 10), 120);
        assert_eq!(xp_for_kill(2, 1), 66);
    }

    #[test]
    fn group_split_divides_evenly_and_never_rounds_up() {
        assert_eq!(split_kill_xp(100, 1), 100);
        assert_eq!(split_kill_xp(100, 3), 33);
        assert_eq!(split_kill_xp(1, 2), 0);
        assert_eq!(split_kill_xp(100, 0), 100);
        assert_eq!(split_kill_xp(100, 5), 20);
    }

    #[test]
    fn xp_progression_discards_awards_and_bank_at_the_level_cap() {
        assert_eq!(
            progression_after_award(60, u32::MAX, 0, u32::MAX as u64),
            XpProgression {
                level: 60,
                xp: 0,
                next_level_xp: 0,
            }
        );
    }

    #[test]
    fn xp_progression_discards_a_large_remainder_when_the_award_reaches_the_cap() {
        assert_eq!(
            progression_after_award(59, 209_799, 209_800, u32::MAX as u64),
            XpProgression {
                level: 60,
                xp: 0,
                next_level_xp: 0,
            }
        );
    }

    #[test]
    fn xp_progression_keeps_the_exact_remainder_across_multiple_levels() {
        assert_eq!(
            progression_after_award(1, 350, 400, 1_000),
            XpProgression {
                level: 3,
                xp: 50,
                next_level_xp: 1_400,
            }
        );
    }

    #[test]
    fn zero_threshold_below_the_cap_remains_inert() {
        assert_eq!(
            progression_after_award(10, 5, 0, 10),
            XpProgression {
                level: 10,
                xp: 15,
                next_level_xp: 0,
            }
        );
    }

    #[test]
    fn rest_bonus_doubles_until_the_pool_drains() {
        assert_eq!(rest_bonus(50, 1000), (100, 50));
        assert_eq!(rest_bonus(50, 20), (70, 20));
        assert_eq!(rest_bonus(50, 0), (50, 0));
        assert_eq!(rest_bonus(50, 50), (100, 50));
    }

    #[test]
    fn rest_pool_accrues_5pct_per_8h_capped_at_1_5_levels() {
        assert_eq!(rest_pool_after(0, REST_EIGHT_HOURS_MICROS, 1), 20);
        assert_eq!(rest_pool_after(0, 3 * REST_EIGHT_HOURS_MICROS, 1), 60);
        assert_eq!(rest_pool_after(100, REST_EIGHT_HOURS_MICROS, 1), 120);
        assert_eq!(rest_pool_after(0, 10_000 * REST_EIGHT_HOURS_MICROS, 1), 600);
        assert_eq!(rest_pool_after(123, 0, 1), 123);
        assert_eq!(rest_pool_after(123, REST_EIGHT_HOURS_MICROS, 60), 123);
    }

    #[test]
    fn elite_rank_pays_double_xp() {
        assert_eq!(rank_xp_multiplier(0), 1);
        assert_eq!(rank_xp_multiplier(1), 2);
        assert_eq!(rank_xp_multiplier(2), 2);
        assert_eq!(rank_xp_multiplier(3), 2);
        assert_eq!(rank_xp_multiplier(4), 1);
        assert_eq!(rank_xp_multiplier(9), 1);
    }

    #[test]
    fn accrue_on_login_needs_a_prior_logout_and_is_consumed_once() {
        let now = 1_000_000 * 3600 * 24;
        assert_eq!(accrue_rested_on_login(now, 0, 0, 1, true), (0, false));
        let eight_h_ago = (now as u64) - REST_EIGHT_HOURS_MICROS;
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, true),
            (20, true)
        );
        assert_eq!(
            accrue_rested_on_login(now, (now as u64) + 999, 5, 1, true),
            (5, true)
        );
    }

    #[test]
    fn offline_rest_rate_is_full_in_a_rest_area_and_quarter_in_the_field() {
        let now = 1_000_000 * 3600 * 24;
        let eight_h_ago = (now as u64) - REST_EIGHT_HOURS_MICROS;
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, true),
            (20, true)
        );
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, false),
            (5, true)
        );
    }

    #[test]
    fn health_curve_agrees_with_login_and_ding() {
        assert_eq!(max_health_for_level(1), 60);
        assert_eq!(max_health_for_level(2), 75);
        assert_eq!(max_health_for_level(3), 90);
    }

    #[test]
    fn xp_curve_matches_vanilla_low_levels_and_caps() {
        assert_eq!(xp_to_next_level(1), 400);
        assert_eq!(xp_to_next_level(2), 900);
        assert_eq!(xp_to_next_level(3), 1400);
        assert_eq!(xp_to_next_level(4), 2100);
        assert_eq!(xp_to_next_level(5), 2800);
        assert_eq!(xp_to_next_level(10), 7600);
        assert_eq!(xp_to_next_level(11), 8800);
        assert_eq!(xp_to_next_level(40), 90700);
        assert_eq!(xp_to_next_level(59), 209800);
        assert_eq!(xp_to_next_level(60), 0, "no XP needed at the cap");
        assert_eq!(xp_to_next_level(61), 0, "past the cap stays 0");
        assert_eq!(xp_to_next_level(0), 0);
    }
}
