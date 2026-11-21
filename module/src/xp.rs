use spacetimedb::{log, table, Identity, ReducerContext, Timestamp};

use crate::game_character;
use crate::game_world_entity;
use crate::WorldEntity;

pub(crate) const LEVEL_CAP: u32 = 60;

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
        crate::native::progression::xp(ctx, guid, &event);
    }
}

impl LevelupEvent {
    fn send(ctx: &ReducerContext, guid: u64, event: Self) {
        crate::native::progression::levelup(ctx, guid, &event);
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

fn grey_level(player_level: u32) -> u32 {
    match player_level {
        0..=5 => 0,
        6..=39 => player_level - 5 - player_level / 10,
        _ => player_level - 1 - player_level / 5,
    }
}

pub(crate) fn is_grey_kill(player_level: u32, mob_level: u32) -> bool {
    mob_level <= grey_level(player_level)
}

fn zero_difference(player_level: u32) -> u32 {
    match player_level {
        0..=7 => 5,
        8..=9 => 6,
        10..=11 => 7,
        12..=15 => 8,
        16..=19 => 9,
        20..=29 => 11,
        30..=39 => 12,
        40..=44 => 13,
        45..=49 => 14,
        50..=54 => 15,
        55..=59 => 16,
        _ => 17,
    }
}

fn base_gain_level_factor(player_level: u32, mob_level: u32) -> f32 {
    if mob_level >= player_level {
        return 1.0 + 0.05 * (mob_level - player_level).min(4) as f32;
    }
    if is_grey_kill(player_level, mob_level) {
        return 0.0;
    }
    let zero_difference = zero_difference(player_level);
    (zero_difference + mob_level).saturating_sub(player_level) as f32 / zero_difference as f32
}

fn base_gain(owner_level: u32, unit_level: u32, mob_level: u32) -> f32 {
    (owner_level * 5 + 45) as f32 * base_gain_level_factor(unit_level, mob_level)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn xp_for_kill(mob_level: u32, player_level: u32) -> u32 {
    base_gain(player_level, player_level, mob_level).round_ties_even() as u32
}

fn is_elite_rank(rank: u8) -> bool {
    matches!(rank, 1..=3)
}

fn elite_factor(rank: u8, non_raid_dungeon: bool) -> f32 {
    match (is_elite_rank(rank), non_raid_dungeon) {
        (false, _) => 1.0,
        (true, true) => 2.5,
        (true, false) => 2.0,
    }
}

pub(crate) struct KillXpRules {
    mob_level: u32,
    rank: u8,
    non_raid_dungeon: bool,
    xp_rate: f32,
}

impl KillXpRules {
    fn new(ctx: &ReducerContext, mob_level: u32, rank: u8, map_id: u32) -> Self {
        Self {
            mob_level,
            rank,
            non_raid_dungeon: crate::instance::is_non_raid_dungeon_map(map_id),
            xp_rate: xp_rate(ctx),
        }
    }
}

pub(crate) fn pet_kill_xp(owner_level: u32, pet_level: u32, rules: &KillXpRules) -> u32 {
    let gain = base_gain(owner_level, pet_level, rules.mob_level);
    if gain == 0.0 {
        return 0;
    }
    (gain * elite_factor(rules.rank, rules.non_raid_dungeon) * rules.xp_rate).round_ties_even()
        as u32
}

fn kill_xp(player_level: u32, rules: &KillXpRules) -> u32 {
    pet_kill_xp(player_level, player_level, rules)
}

fn group_xp_rate(members: u32) -> f32 {
    match members {
        0..=2 => 1.0,
        3 => 1.166,
        4 => 1.3,
        5 => 1.4,
        _ => (1.0 - members as f32 * 0.05).max(0.01),
    }
}

struct PartyKill {
    non_grey_top_level: u32,
    // Every member's share; only a member at or below `non_grey_top_level` is paid it.
    shares: Vec<u32>,
}

fn party_kill(levels: &[u32], rules: &KillXpRules) -> Option<PartyKill> {
    let top_level = levels.iter().copied().max()?;
    let non_grey_top_level = levels
        .iter()
        .copied()
        .filter(|&level| !is_grey_kill(level, rules.mob_level))
        .max()?;
    let xp = kill_xp(non_grey_top_level, rules) as f32;
    let group_rate = group_xp_rate(levels.len() as u32);
    let sum_level = levels.iter().sum::<u32>() as f32;
    let shares = levels
        .iter()
        .map(|&level| {
            let share_rate = group_rate * level as f32 / sum_level;
            if top_level == non_grey_top_level {
                (xp * share_rate) as u32
            } else {
                (xp * share_rate / 2.0 + 1.0) as u32
            }
        })
        .collect();
    Some(PartyKill {
        non_grey_top_level,
        shares,
    })
}

pub(crate) enum PetKillXp<'a> {
    Alone(&'a KillXpRules),
    Party { xp: u32, non_grey_top_level: u32 },
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

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn rank_xp_multiplier(rank: u8) -> u32 {
    if is_elite_rank(rank) {
        2
    } else {
        1
    }
}

fn xp_rate(ctx: &ReducerContext) -> f32 {
    crate::config::server_config(ctx).xp_rate
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
    if player_level >= LEVEL_CAP || exploration_level == 0 {
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

const REST_ACCRUE_PERMILLE_PER_8H: u64 = 25;
const REST_EIGHT_HOURS_MICROS: u64 = 8 * 3600 * 1_000_000;
const REST_CAP_PERMILLE: u64 = 750;

fn rest_bonus(base_xp: u32, pool: u32) -> (u32, u32) {
    let bonus = base_xp.min(pool);
    (base_xp + bonus, bonus)
}

fn rest_pool_after(pool: u32, offline_micros: u64, level: u32) -> u32 {
    let level_xp = xp_to_next_level(level) as u64;
    if level_xp == 0 {
        return 0;
    }
    if offline_micros == 0 {
        return pool;
    }
    // Widen before multiplying; long offline spans overflow u64 before the rested cap applies.
    let accrued =
        ((level_xp as u128) * (REST_ACCRUE_PERMILLE_PER_8H as u128) * (offline_micros as u128)
            / (1000 * REST_EIGHT_HOURS_MICROS as u128)) as u64;
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

pub(crate) fn award_kill_xp(
    ctx: &ReducerContext,
    killed_guid: u64,
    mob_level: u32,
    rank: u8,
    map_id: u32,
    in_party: bool,
    recipients: &[u64],
) {
    let entities = ctx.db.game_world_entity();
    let members: Vec<(u64, u32)> = recipients
        .iter()
        .filter_map(|guid| Some((*guid, entities.guid().find(*guid)?.level)))
        .collect();
    let levels: Vec<u32> = members.iter().map(|(_, level)| *level).collect();
    let rules = KillXpRules::new(ctx, mob_level, rank, map_id);
    let Some(kill) = party_kill(&levels, &rules) else {
        return;
    };
    for ((recipient, level), share) in members.iter().zip(kill.shares) {
        let paid = if *level <= kill.non_grey_top_level {
            share
        } else {
            0
        };
        award_xp(ctx, *recipient, killed_guid, paid);
        let pet_xp = if in_party {
            PetKillXp::Party {
                xp: share,
                non_grey_top_level: kill.non_grey_top_level,
            }
        } else if paid > 0 {
            PetKillXp::Alone(&rules)
        } else {
            continue;
        };
        crate::creatures::award_hunter_pet_kill_progression(ctx, *recipient, pet_xp);
    }
}

fn award_xp(ctx: &ReducerContext, attacker_guid: u64, killed_guid: u64, base: u32) {
    let entities = ctx.db.game_world_entity();
    let Some(mut p) = entities.guid().find(attacker_guid) else {
        return;
    };
    if p.level >= LEVEL_CAP || base == 0 {
        return;
    }
    let chars = ctx.db.game_character();
    let award = match chars.guid().find(attacker_guid) {
        Some(mut c) if c.rested_xp > 0 => {
            let (granted, drained) = rest_bonus(base, c.rested_xp);
            c.rested_xp = c.rested_xp.saturating_sub(drained);
            c.rest_state =
                crate::rest::rest_state_after_bonus(c.rest_state, c.rested_xp, p.level as u8);
            crate::rest::set_rest_state_byte(&mut p, c.rest_state);
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
            created_at: crate::time::now(ctx),
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
        let own = p.own_pool();
        let mana_gained = if own.power_type == lyracore_shared::packing::power_type::MANA {
            p.set_own_pool(own.max, own.max);
            own.max.saturating_sub(delta.old_max_power)
        } else {
            0
        };
        p.next_level_xp = xp_to_next_level(p.level);
        LevelupEvent::send(
            ctx,
            p.guid,
            LevelupEvent {
                id: 0,
                recipient_identity: p.owner_identity,
                new_level: p.level,
                health_gained: gained,
                created_at: crate::time::now(ctx),
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
    if leveled && p.level >= LEVEL_CAP {
        if let Some(mut character) = crate::helpers::character_by_guid(ctx, p.guid) {
            character.level = p.level as u8;
            character.rest_state = lyracore_shared::packing::REST_STATE_NORMAL;
            ctx.db.game_character().guid().update(character);
        }
        crate::rest::set_rest_state_byte(p, lyracore_shared::packing::REST_STATE_NORMAL);
    }
}

pub(crate) fn sweep_expired(ctx: &ReducerContext) {
    crate::gc::reap_expired!(ctx, game_xp_event, crate::EVENT_TTL_MICROS);
    crate::gc::reap_expired!(ctx, game_levelup_event, crate::EVENT_TTL_MICROS);
}

#[cfg(test)]
mod tests {
    use super::{
        accrue_rested_on_login, explore_xp, grey_level, group_xp_rate, kill_xp,
        max_health_for_level, party_kill, pet_kill_xp, progression_after_award, rank_xp_multiplier,
        rest_bonus, rest_pool_after, xp_for_kill, xp_to_next_level, KillXpRules, XpProgression,
        REST_EIGHT_HOURS_MICROS,
    };

    #[test]
    fn explore_xp_scales_off_area_level_and_caps() {
        assert_eq!(explore_xp(10, 10), 95);
        assert_eq!(explore_xp(10, 12), 95);
        assert_eq!(explore_xp(20, 10), 120);
        assert_eq!(explore_xp(10, 16), 90);
        assert_eq!(explore_xp(2, 30), 0);
        assert_eq!(explore_xp(10, 60), 0);
        assert_eq!(explore_xp(0, 10), 0);
    }

    #[test]
    fn kill_xp_scales_the_character_level_base_by_the_level_difference() {
        assert_eq!(xp_for_kill(1, 1), 50);
        assert_eq!(xp_for_kill(10, 10), 95);
        assert_eq!(xp_for_kill(60, 60), 345);
        assert_eq!(xp_for_kill(27, 30), 146);
        assert_eq!(xp_for_kill(50, 60), 142);
        assert_eq!(xp_for_kill(40, 45), 174);
        assert_eq!(xp_for_kill(14, 20), 66);
        assert_eq!(xp_for_kill(11, 10), 100);
        assert_eq!(xp_for_kill(34, 30), 234);
    }

    #[test]
    fn kill_xp_caps_the_bonus_at_four_levels_above() {
        assert_eq!(xp_for_kill(14, 10), 114);
        assert_eq!(xp_for_kill(30, 10), 114);
    }

    #[test]
    fn kill_xp_is_zero_at_or_below_the_grey_level() {
        assert_eq!(xp_for_kill(4, 9), 0);
        assert_eq!(xp_for_kill(5, 9), 30);
        assert_eq!(xp_for_kill(1, 60), 0);
        assert_eq!(xp_for_kill(47, 60), 0);
        assert_eq!(xp_for_kill(48, 60), 101);
    }

    #[test]
    fn grey_level_follows_the_two_vmangos_bands() {
        assert_eq!(grey_level(5), 0);
        assert_eq!(grey_level(6), 1);
        assert_eq!(grey_level(10), 4);
        assert_eq!(grey_level(39), 31);
        assert_eq!(grey_level(40), 31);
        assert_eq!(grey_level(60), 47);
    }

    fn rules(mob_level: u32, rank: u8) -> KillXpRules {
        KillXpRules {
            mob_level,
            rank,
            non_raid_dungeon: false,
            xp_rate: 1.0,
        }
    }

    fn dungeon_rules(mob_level: u32, rank: u8) -> KillXpRules {
        KillXpRules {
            non_raid_dungeon: true,
            ..rules(mob_level, rank)
        }
    }

    fn paid_shares(levels: &[u32], rules: &KillXpRules) -> Vec<u32> {
        party_kill(levels, rules).map_or_else(
            || vec![0; levels.len()],
            |kill| {
                levels
                    .iter()
                    .zip(kill.shares)
                    .map(|(level, share)| {
                        if *level <= kill.non_grey_top_level {
                            share
                        } else {
                            0
                        }
                    })
                    .collect()
            },
        )
    }

    #[test]
    fn an_elite_pays_double_and_two_and_a_half_times_in_a_non_raid_dungeon() {
        assert_eq!(kill_xp(20, &rules(20, 0)), 145);
        assert_eq!(kill_xp(20, &rules(20, 1)), 290);
        assert_eq!(kill_xp(20, &rules(20, 3)), 290);
        assert_eq!(kill_xp(20, &rules(20, 4)), 145);
        assert_eq!(kill_xp(20, &dungeon_rules(20, 1)), 362);
        assert_eq!(kill_xp(20, &dungeon_rules(20, 0)), 145);
    }

    #[test]
    fn the_xp_rate_scales_the_kill_before_rounding() {
        let rated = |mob_level, xp_rate| KillXpRules {
            xp_rate,
            ..rules(mob_level, 0)
        };
        assert_eq!(kill_xp(30, &rated(27, 1.5)), 219);
        assert_eq!(kill_xp(30, &rated(4, 2.0)), 0);
    }

    #[test]
    fn a_hunter_pet_takes_its_base_from_the_owner_and_its_factor_from_itself() {
        assert_eq!(pet_kill_xp(40, 30, &rules(32, 0)), 270);
        assert_eq!(pet_kill_xp(20, 15, &rules(18, 0)), 167);
        assert_eq!(pet_kill_xp(60, 58, &rules(60, 0)), 380);
        assert_eq!(pet_kill_xp(20, 20, &dungeon_rules(20, 1)), 362);
        assert_eq!(pet_kill_xp(60, 20, &rules(10, 0)), 0);
    }

    #[test]
    fn group_rate_grows_to_five_members_then_falls_five_percent_a_member() {
        assert_eq!(group_xp_rate(1), 1.0);
        assert_eq!(group_xp_rate(2), 1.0);
        assert_eq!(group_xp_rate(3), 1.166);
        assert_eq!(group_xp_rate(4), 1.3);
        assert_eq!(group_xp_rate(5), 1.4);
        assert_eq!(group_xp_rate(8), 0.6);
        assert_eq!(group_xp_rate(40), 0.01);
    }

    #[test]
    fn a_lone_member_gets_the_whole_kill() {
        assert_eq!(paid_shares(&[30], &rules(27, 0)), [146]);
        assert_eq!(paid_shares(&[60], &rules(50, 0)), [142]);
        assert_eq!(paid_shares(&[9], &rules(4, 0)), [0]);
    }

    #[test]
    fn a_party_splits_the_top_members_kill_by_level() {
        assert_eq!(paid_shares(&[20, 18], &rules(19, 0)), [69, 62]);
        assert_eq!(paid_shares(&[60, 20], &rules(60, 0)), [258, 86]);
    }

    #[test]
    fn a_full_party_adds_the_group_bonus() {
        assert_eq!(paid_shares(&[60; 5], &rules(60, 0)), [96; 5]);
    }

    #[test]
    fn a_grey_top_member_earns_nothing_and_halves_the_rest() {
        assert_eq!(paid_shares(&[60, 40], &rules(45, 0)), [0, 59]);
    }

    #[test]
    fn a_member_above_the_non_grey_top_still_has_a_share_for_a_pet() {
        let kill = party_kill(&[60, 40], &rules(45, 0)).unwrap();
        assert_eq!(kill.non_grey_top_level, 40);
        assert_eq!(kill.shares, [89, 59]);
    }

    #[test]
    fn a_kill_grey_to_every_member_pays_nobody() {
        assert!(party_kill(&[60, 55], &rules(40, 0)).is_none());
        assert!(party_kill(&[], &rules(40, 0)).is_none());
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
    fn rest_pool_accrues_2_5pct_per_8h_capped_at_75pct_of_next_level() {
        assert_eq!(rest_pool_after(0, REST_EIGHT_HOURS_MICROS, 1), 10);
        assert_eq!(rest_pool_after(0, 3 * REST_EIGHT_HOURS_MICROS, 1), 30);
        assert_eq!(rest_pool_after(100, REST_EIGHT_HOURS_MICROS, 1), 110);
        assert_eq!(rest_pool_after(0, 10_000 * REST_EIGHT_HOURS_MICROS, 1), 300);
        assert_eq!(rest_pool_after(123, 0, 1), 123);
        assert_eq!(rest_pool_after(123, REST_EIGHT_HOURS_MICROS, 60), 0);
        assert_eq!(rest_pool_after(123, 0, 60), 0);
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
            (10, true)
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
            (10, true)
        );
        assert_eq!(
            accrue_rested_on_login(now, eight_h_ago, 0, 1, false),
            (2, true)
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
