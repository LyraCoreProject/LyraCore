use crate::spell::source_behavior;
use lyracore_shared::constants::tracer_spell;
use lyracore_shared::spell::taxonomy::E_REVIVE_PET;
use lyracore_shared::spell_custom as custom;

use spacetimedb::{log, ReducerContext, Table, TimeDuration, Timestamp};

use crate::game_item_instance;
use crate::{game_character, game_creature_template, game_world_entity, WorldEntity};

use crate::geometry::{distance_3d, projectile_travel_ms};
use crate::spell::tables::{SPELL_ATTR_EX2_IGNORE_LINE_OF_SIGHT, SPELL_ATTR_EX3_ONLY_ON_PLAYER};
use crate::spell::*;

use crate::skilldata::game_skill_ability;

use crate::script_binding::game_script;
use crate::{
    game_aura, game_ground_area, game_pending_spell_impact, game_spell, game_spell_effect,
    game_spell_reagent, Aura, GroundArea, PendingSpellImpact, ResurrectRequest, Spell,
    SpellCastEvent, SpellEffect,
};

#[derive(Clone, Copy, Default)]
pub(crate) enum ItemGrantOutcome {
    #[default]
    None,
    Granted,
    Refused,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct EffectHit {
    pub item: ItemGrantOutcome,
    pub dealt: u32,
    pub resisted: u32,
    pub absorbed: u32,
    pub blocked: u32,
    pub crit: bool,
    pub healed: u32,
    pub energized: u32,
    pub power_type: u8,
}
impl EffectHit {
    fn none() -> Self {
        Self::default()
    }
    fn dmg(dealt: u32, absorbed: u32) -> Self {
        Self {
            dealt,
            absorbed,
            ..Self::default()
        }
    }
}

/// An absent target counts as equal level.
pub(crate) fn spell_misses(
    ctx: &ReducerContext,
    caster_level: u8,
    target_guid: u64,
    spell: &Spell,
) -> bool {
    if spell.cast_flags & CAST_FLAG_IGNORE_RESISTANCES != 0 {
        return false;
    }
    let target = ctx.db.game_world_entity().guid().find(target_guid);
    let target_level = target
        .as_ref()
        .map_or(u32::from(caster_level), |target| target.level);
    let target_is_character = target.as_ref().is_some_and(|target| target.is_player());
    let binary_resistance = if spell.cast_flags & CAST_FLAG_BINARY != 0 {
        crate::spell::rolls::resistance_chance(
            crate::modifiers::resistance_bonus(ctx, target_guid, spell.school_mask),
            u32::from(caster_level),
            target_level,
            target_is_character,
            false,
        )
    } else {
        0.0
    };
    let miss = if spell.cast_flags & CAST_FLAG_ALWAYS_HIT != 0 {
        0
    } else {
        crate::spell::rolls::magic_miss_bp(
            u32::from(caster_level),
            target_level,
            target_is_character,
            binary_resistance,
        )
    };
    crate::spell::rolls::integer_roll(ctx, target_guid, 10_000) < miss
}

pub(crate) fn recipe_reagents(ctx: &ReducerContext, spell_id: u32) -> Vec<(u32, u32)> {
    ctx.db
        .game_spell_reagent()
        .by_spell()
        .filter(&spell_id)
        .map(|r| (r.item_entry, r.count))
        .collect()
}

fn recipe_skill(ctx: &ReducerContext, spell_id: u32) -> Option<(u32, u32, u32)> {
    ctx.db
        .game_skill_ability()
        .iter()
        .filter(|a| a.spell_id == spell_id && crate::skill::is_profession_line(a.skill_line))
        .map(|a| (a.skill_line, a.green.max(0) as u32, a.gray.max(0) as u32))
        .next()
}

pub(crate) fn gain_recipe_skill(ctx: &ReducerContext, caster_guid: u64, spell_id: u32) {
    if let Some((line, green, gray)) = recipe_skill(ctx, spell_id) {
        crate::skill::gain_profession_skill(ctx, caster_guid, line, green, gray);
    }
}

const MELEE_PBAOE_RADIUS_YD: f32 = 8.0;

fn aoe_keep(
    d2: f32,
    r2: f32,
    want_enemy: bool,
    is_caster: bool,
    harmed: bool,
    helped: bool,
) -> bool {
    if d2 > r2 {
        return false;
    }
    if want_enemy {
        !is_caster && harmed
    } else {
        is_caster || helped
    }
}

/// vmangos `SetTargetMap`: a chain's jump count, else the spell's `MaxAffectedTargets` (0: none).
pub(super) fn affected_target_cap(max_affected_targets: u32, e: &SpellEffect) -> Option<usize> {
    if matches!(e.target, T_CHAIN_ENEMY | T_CHAIN_ALLY) {
        Some(chain_length(e.chain_targets))
    } else if max_affected_targets > 0 {
        Some(max_affected_targets as usize)
    } else {
        None
    }
}

/// Keeps `cap` random units of `hits` and always `kept`; one `seed` keeps the same units each call.
fn keep_affected_targets(mut hits: Vec<u64>, cap: usize, kept: u64, seed: u32) -> Vec<u64> {
    if hits.len() <= cap {
        return hits;
    }
    let mut ranked = hits.clone();
    ranked.sort_by_key(|&guid| (guid != kept, affected_target_rank(guid, seed)));
    ranked.truncate(cap);
    hits.retain(|guid| ranked.contains(guid));
    hits
}

/// The units each capped effect of one cast picked; vmangos fills a cast's target map once.
#[derive(Default)]
pub(super) struct CappedPicks {
    seed: Option<u32>,
    picked: std::collections::HashMap<u8, Vec<u64>>,
}

impl CappedPicks {
    /// `hits` cut to `cap`; the first call per effect picks, a later call keeps only those picks.
    pub(super) fn pick(
        &mut self,
        effect_index: u8,
        hits: Vec<u64>,
        cap: usize,
        kept: u64,
        draw: impl FnOnce() -> u32,
    ) -> Vec<u64> {
        if let Some(picked) = self.picked.get(&effect_index) {
            return hits
                .into_iter()
                .filter(|guid| picked.contains(guid))
                .collect();
        }
        let picked = if hits.len() > cap {
            let seed = *self.seed.get_or_insert_with(draw);
            keep_affected_targets(hits, cap, kept, seed)
        } else {
            hits
        };
        self.picked.insert(effect_index, picked.clone());
        picked
    }
}

// The SplitMix64 finalizer, so the rank of one guid tells nothing about another's.
fn affected_target_rank(guid: u64, seed: u32) -> u64 {
    let mut z = guid ^ u64::from(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub(crate) fn helps_units(target: u8) -> bool {
    matches!(
        target,
        T_TARGET_ALLY
            | T_AREA_ALLY
            | T_AREA_NOT_HOSTILE
            | T_PARTY_AREA
            | T_RAID_AREA
            | T_TARGET_SUBGROUP
            | T_CHAIN_ALLY
            | T_TARGET_GROUP_CLASS
    )
}

pub(crate) fn faction_target_violation(
    hits_enemy: bool,
    hits_ally: bool,
    friendly: bool,
    hostile: bool,
) -> Option<&'static str> {
    if hits_enemy && friendly {
        return Some("enemies");
    }
    if hits_ally && hostile {
        return Some("allies");
    }
    None
}

pub(crate) fn select_targets(
    ctx: &ReducerContext,
    caster_guid: u64,
    explicit: u64,
    e: &SpellEffect,
    dest: Option<(f32, f32, f32)>,
    hdr: &Spell,
) -> Vec<u64> {
    // vmangos puts a ground area on the ground, not on a unit: see `place_ground_area`.
    if e.kind == E_PERSISTENT_AREA {
        return Vec::new();
    }
    let one = if explicit == 0 { caster_guid } else { explicit };
    match e.target {
        T_SELF => vec![caster_guid],
        T_TARGET_ENEMY | T_TARGET_ALLY | T_TARGET_ANY => {
            selected_unit(ctx, caster_guid, one).into_iter().collect()
        }
        T_PARTY_AREA | T_RAID_AREA => {
            let Some(caster) = ctx.db.game_world_entity().guid().find(caster_guid) else {
                return Vec::new();
            };
            let raid = e.target == T_RAID_AREA;
            party_area_targets(ctx, &caster, &caster, raid, area_radius(e), hdr.spell_level)
        }
        T_TARGET_SUBGROUP | T_CHAIN_ALLY | T_TARGET_GROUP_CLASS => {
            selected_unit_targets(ctx, caster_guid, one, e, hdr)
        }
        T_AREA_ENEMY | T_AREA_ALLY | T_AREA_NOT_HOSTILE | T_CHAIN_ENEMY => {
            let entities = ctx.db.game_world_entity();
            let Some(caster) = entities.guid().find(caster_guid) else {
                return vec![one];
            };
            let (a_map, a_inst, ax, ay, az) = if let Some((dx, dy, dz)) = dest {
                (caster.map_id, caster.instance_id, dx, dy, dz)
            } else {
                let anchor_guid = if explicit == 0 || explicit == caster_guid {
                    caster_guid
                } else {
                    explicit
                };
                let Some(anchor) = entities.guid().find(anchor_guid) else {
                    return vec![one];
                };
                (
                    anchor.map_id,
                    anchor.instance_id,
                    anchor.x,
                    anchor.y,
                    anchor.z,
                )
            };
            let radius = if e.target == T_CHAIN_ENEMY {
                enemy_chain_reach(e, hdr)
            } else {
                area_radius(e)
            };
            let r2 = radius * radius;
            let want_enemy = matches!(e.target, T_AREA_ENEMY | T_CHAIN_ENEMY);

            let units: Vec<WorldEntity> =
                crate::helpers::area_candidates(ctx, a_map, a_inst, ax, ay, radius)
                    .into_iter()
                    .filter(|c| !c.dead)
                    .filter(|c| {
                        let (dx, dy, dz) = (c.x - ax, c.y - ay, c.z - az);
                        let d2 = dx * dx + dy * dy + dz * dz;
                        if d2 > r2 {
                            return false;
                        }
                        let (harmed, helped) = if want_enemy {
                            (crate::combat::area_may_harm(ctx, &caster, c), false)
                        } else {
                            // `may_help` also keeps an outsider from helping a duelist.
                            let helped = crate::combat::may_help(ctx, &caster, c)
                                && (e.target == T_AREA_NOT_HOSTILE
                                    || crate::combat::area_may_help(ctx, &caster, c));
                            (false, helped)
                        };
                        aoe_keep(d2, r2, want_enemy, c.guid == caster_guid, harmed, helped)
                    })
                    .collect();

            if e.target == T_CHAIN_ENEMY {
                let Some(first) = selected_unit(ctx, caster_guid, explicit)
                    .filter(|&first| first != caster_guid)
                    .and_then(|first| entities.guid().find(first))
                else {
                    return Vec::new();
                };
                return enemy_chain_targets(ctx, &caster, &first, &units, e, hdr);
            }
            let mut hits: Vec<u64> = units.iter().map(|c| c.guid).collect();
            // Missing faction data can make the caster appear unfriendly to itself.
            if !want_enemy && !hits.contains(&caster_guid) {
                hits.push(caster_guid);
            }
            // Falling back to the explicit target on an empty enemy AoE would damage the caster.
            hits
        }
        _ => vec![one],
    }
}

/// The units past `victim` that a firing queued strike's target 6 chain reaches, in jump order.
pub(crate) fn queued_strike_chain(
    ctx: &ReducerContext,
    attacker_guid: u64,
    victim_guid: u64,
    strike: &Spell,
) -> Vec<u64> {
    // The importer keeps a queued strike at T_TARGET_ENEMY; its row still carries the chain.
    let Some(chain) = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&strike.spell_id)
        .find(|e| e.kind == E_NEXT_SWING && e.chain_targets > 1)
    else {
        return Vec::new();
    };
    let chain = SpellEffect {
        target: T_CHAIN_ENEMY,
        ..chain
    };
    select_targets(ctx, attacker_guid, victim_guid, &chain, None, strike)
        .into_iter()
        .filter(|&unit| unit != victim_guid)
        .collect()
}

/// The cast's selected unit, unless it rides a taxi.
fn selected_unit(ctx: &ReducerContext, caster_guid: u64, selected: u64) -> Option<u64> {
    let flying = selected != caster_guid
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(selected)
            .is_some_and(|target| target.is_flying());
    (!flying).then_some(selected)
}

/// vmangos `SetTargetMap` targets 37, 45 and 61: units gathered around the selected unit.
fn selected_unit_targets(
    ctx: &ReducerContext,
    caster_guid: u64,
    one: u64,
    e: &SpellEffect,
    hdr: &Spell,
) -> Vec<u64> {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(selected)) = (
        entities.guid().find(caster_guid),
        selected_unit(ctx, caster_guid, one).and_then(|guid| entities.guid().find(guid)),
    ) else {
        return Vec::new();
    };
    match e.target {
        T_TARGET_SUBGROUP => {
            let Some(center) = subgroup_center(ctx, &caster, &selected) else {
                return Vec::new();
            };
            party_area_targets(
                ctx,
                &caster,
                &center,
                false,
                selected_unit_radius(e, hdr),
                hdr.spell_level,
            )
        }
        T_CHAIN_ALLY => friendly_chain_targets(ctx, &caster, &selected, e, hdr),
        _ => group_class_targets(ctx, &caster, &selected, selected_unit_radius(e, hdr)),
    }
}

/// vmangos `SetTargetMap`: the effect's radius, else the spell's maximum range.
fn selected_unit_radius(e: &SpellEffect, hdr: &Spell) -> f32 {
    if e.radius_yd > 0.0 {
        e.radius_yd
    } else {
        hdr.range_yd as f32
    }
}

/// vmangos target 37: a pet caster's owner, or a Character caster's selected Character or pet owner.
fn subgroup_center(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    selected: &WorldEntity,
) -> Option<WorldEntity> {
    let entities = ctx.db.game_world_entity();
    if caster.owner_guid != 0 {
        return entities.guid().find(caster.owner_guid);
    }
    if !caster.is_player() {
        return None;
    }
    let center = if selected.is_player() {
        selected.guid
    } else {
        selected.owner_guid
    };
    entities.guid().find(center).filter(WorldEntity::is_player)
}

fn area_radius(e: &SpellEffect) -> f32 {
    if e.radius_yd > 0.0 {
        e.radius_yd
    } else {
        MELEE_PBAOE_RADIUS_YD
    }
}

/// vmangos `FillRaidOrPartyTargets`: `center`'s Subgroup or raid and their pets near `center`.
fn party_area_targets(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    center: &WorldEntity,
    raid: bool,
    radius: f32,
    spell_level: u8,
) -> Vec<u64> {
    // vmangos `GetCharmerOrOwnerPlayerOrPlayerItself`, else `GetCharmerOrOwnerOrSelf`.
    let master = crate::pvp::affecting_character(ctx, center).map_or(
        if center.owner_guid != 0 {
            center.owner_guid
        } else {
            center.guid
        },
        |character| character.guid,
    );
    let group = crate::group::group_of(ctx, master);
    let members: Vec<u64> = match &group {
        Some(own) => {
            let subgroup = crate::group::raid_slot_of(own).subgroup();
            crate::group::members_of(ctx, own.group_id)
                .into_iter()
                .filter(|m| raid || crate::group::raid_slot_of(m).subgroup() == subgroup)
                .map(|m| m.character_guid)
                .collect()
        }
        None => vec![master],
    };
    let entities = ctx.db.game_world_entity();
    let mut hits = Vec::new();
    for member in members
        .into_iter()
        .filter_map(|guid| entities.guid().find(guid))
    {
        if group.is_some() && member.level + 10 < u32::from(spell_level) {
            continue;
        }
        if crate::combat::relation(ctx, caster, &member) == crate::combat::Relation::Hostile {
            continue;
        }
        let pet = crate::creatures::pet_of(ctx, member.guid);
        for unit in std::iter::once(member).chain(pet) {
            let reached = if unit.guid == caster.guid {
                !raid
            } else {
                !unit.dead
                    && crate::helpers::in_same_partition(&unit, center.map_id, center.instance_id)
                    && crate::helpers::dist_sq(center, &unit) <= radius * radius
            };
            if reached {
                hits.push(unit.guid);
            }
        }
    }
    hits
}

/// vmangos target 61: the selected Character's Group members of its class, or that unit alone.
fn group_class_targets(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    selected: &WorldEntity,
    radius: f32,
) -> Vec<u64> {
    let Some(group_id) = character_group(ctx, selected) else {
        return vec![selected.guid];
    };
    let entities = ctx.db.game_world_entity();
    crate::group::members_of(ctx, group_id)
        .into_iter()
        .filter_map(|member| entities.guid().find(member.character_guid))
        .filter(|member| {
            !member.dead
                && member.class() == selected.class()
                && crate::helpers::in_same_partition(member, selected.map_id, selected.instance_id)
                && crate::helpers::dist_sq(selected, member) <= radius * radius
                && crate::combat::relation(ctx, caster, member) != crate::combat::Relation::Hostile
        })
        .map(|member| member.guid)
        .collect()
}

fn character_group(ctx: &ReducerContext, unit: &WorldEntity) -> Option<u64> {
    if !unit.is_player() {
        return None;
    }
    crate::group::group_of(ctx, unit.guid).map(|member| member.group_id)
}

/// vmangos target 45: the selected unit, then the injured friendly units by `ChainHealingOrder`.
fn friendly_chain_targets(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    selected: &WorldEntity,
    e: &SpellEffect,
    hdr: &Spell,
) -> Vec<u64> {
    let count = chain_length(e.chain_targets);
    if count == 1 {
        return vec![selected.guid];
    }
    // vmangos gathers jumps within this reach of the caster, not of the target.
    let reach = selected_unit_radius(e, hdr) + count as f32 * CHAIN_JUMP_YD;
    let link = |unit: &WorldEntity| ChainUnit {
        guid: unit.guid,
        at: (unit.x, unit.y, unit.z),
        group: character_group(ctx, unit),
        missing_health: i64::from(unit.max_health.saturating_sub(unit.health)),
        player: unit.is_player(),
    };
    let injured: Vec<ChainUnit> = crate::helpers::area_candidates(
        ctx,
        caster.map_id,
        caster.instance_id,
        caster.x,
        caster.y,
        reach,
    )
    .into_iter()
    .filter(|c| {
        !c.dead
            && c.health < c.max_health
            && crate::helpers::dist_sq(caster, c) <= reach * reach
            && (c.guid == caster.guid
                || (crate::combat::may_help(ctx, caster, c)
                    && crate::combat::area_may_help(ctx, caster, c)))
    })
    .map(|c| link(&c))
    .collect();
    let checks_los = hdr.attributes_ex_2 & SPELL_ATTR_EX2_IGNORE_LINE_OF_SIGHT == 0;
    let sees = |from: &ChainUnit, to: &ChainUnit| {
        !checks_los || crate::nav::has_los(ctx, caster.map_id, caster.instance_id, from.at, to.at)
    };
    chain_jumps(link(selected), injured, count, sees, chain_healing_rank)
}

/// vmangos target 6 gathers within `radius` for a melee spell, else `radius` plus 10 yd a jump.
fn enemy_chain_reach(e: &SpellEffect, hdr: &Spell) -> f32 {
    let radius = selected_unit_radius(e, hdr);
    if hdr.damage_class == crate::spell::proc::DAMAGE_CLASS_MELEE {
        radius
    } else {
        radius + chain_length(e.chain_targets) as f32 * CHAIN_JUMP_YD
    }
}

/// vmangos target 6: the selected unit, then the nearest units it and each link can see.
fn enemy_chain_targets(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    first: &WorldEntity,
    units: &[WorldEntity],
    e: &SpellEffect,
    hdr: &Spell,
) -> Vec<u64> {
    let link = |unit: &WorldEntity| ChainUnit {
        player: unit.is_player(),
        ..ChainUnit::at(unit.guid, (unit.x, unit.y, unit.z))
    };
    let checks_los = hdr.attributes_ex_2 & SPELL_ATTR_EX2_IGNORE_LINE_OF_SIGHT == 0;
    let only_players = hdr.attributes_ex_3 & SPELL_ATTR_EX3_ONLY_ON_PLAYER != 0;
    let may_jump = |from: &ChainUnit, to: &ChainUnit| {
        chain_may_jump(to, only_players, || {
            !checks_los
                || crate::nav::has_los(ctx, caster.map_id, caster.instance_id, from.at, to.at)
        })
    };
    chain_jumps(
        link(first),
        units.iter().map(link).collect(),
        chain_length(e.chain_targets),
        may_jump,
        chain_distance_sq,
    )
}

fn chain_may_jump(to: &ChainUnit, only_players: bool, sees: impl FnOnce() -> bool) -> bool {
    (!only_players || to.player) && sees()
}

/// One unit a chain may jump to; `group` is a Character's Group.
#[derive(Clone, Copy)]
struct ChainUnit {
    guid: u64,
    at: (f32, f32, f32),
    group: Option<u64>,
    missing_health: i64,
    player: bool,
}

impl ChainUnit {
    fn at(guid: u64, at: (f32, f32, f32)) -> Self {
        Self {
            guid,
            at,
            group: None,
            missing_health: 0,
            player: false,
        }
    }
}

/// vmangos `ChainHealingHash`: the last unit's Group first, then the most injured first.
fn chain_healing_rank(last: &ChainUnit, next: &ChainUnit) -> i64 {
    let same_group = last.group.is_some() && last.group == next.group;
    let tier = if same_group { 20_000 } else { 40_000 };
    tier - next.missing_health
}

fn chain_distance_sq(from: &ChainUnit, to: &ChainUnit) -> f32 {
    (to.at.0 - from.at.0).powi(2) + (to.at.1 - from.at.1).powi(2) + (to.at.2 - from.at.2).powi(2)
}

// vmangos CHAIN_SPELL_JUMP_RADIUS: a chain jumps at most this far from the last unit it hit.
const CHAIN_JUMP_YD: f32 = 10.0;

/// vmangos `EffectChainTarget` 0 or 1 hits the cast's target alone.
fn chain_length(chain_targets: u8) -> usize {
    usize::from(chain_targets.max(1))
}

/// vmangos `SetTargetMap` for a chain: the cast's target, then each time the first unit by `order`.
fn chain_jumps<K: PartialOrd>(
    first: ChainUnit,
    mut units: Vec<ChainUnit>,
    count: usize,
    may_jump: impl Fn(&ChainUnit, &ChainUnit) -> bool,
    order: impl Fn(&ChainUnit, &ChainUnit) -> K,
) -> Vec<u64> {
    units.retain(|unit| unit.guid != first.guid);
    let mut chain = vec![first.guid];
    let mut last = first;
    while chain.len() < count {
        let mut in_reach: Vec<(usize, K)> = units
            .iter()
            .enumerate()
            .filter(|&(_, unit)| chain_distance_sq(&last, unit) <= CHAIN_JUMP_YD * CHAIN_JUMP_YD)
            .map(|(i, unit)| (i, order(&last, unit)))
            .collect();
        in_reach.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let next = in_reach
            .into_iter()
            .find(|&(i, _)| may_jump(&last, &units[i]));
        let Some((next, _)) = next else {
            break;
        };
        last = units.remove(next);
        chain.push(last.guid);
    }
    chain
}

fn enchant_item_temporarily(
    ctx: &ReducerContext,
    e: &SpellEffect,
    caster_guid: u64,
    item_target: Option<u64>,
    seconds: i32,
) {
    let Some(item) = item_target
        .and_then(|guid| ctx.db.game_item_instance().guid().find(guid))
        .filter(|item| item.owner_guid == caster_guid)
    else {
        log::info!(
            "spell {} temporary enchantment has no item target",
            e.spell_id
        );
        return;
    };
    let enchant_id = e.p0.max(0) as u32;
    if !crate::items::is_known_enchant(ctx, enchant_id) {
        log::info!(
            "spell {} names unknown temporary enchantment {enchant_id}",
            e.spell_id
        );
        return;
    }
    let duration_ms = u32::try_from(seconds).unwrap_or(0).saturating_mul(1000);
    crate::items::enchant_item_temporarily(ctx, &item, e.spell_id, enchant_id, duration_ms);
}

fn next_stacks(current: u8, max_stacks: u8) -> u8 {
    (current.max(1) + 1).min(max_stacks.max(1))
}

pub(crate) fn aura_has_visible_slot(
    ctx: &ReducerContext,
    header: &Spell,
    caster_guid: u64,
    target_guid: u64,
) -> bool {
    use lyracore_shared::spell::taxonomy::{CAST_FLAG_AREA_AURA, TOTEM_TYPE};

    if !header.equip_passive() {
        return true;
    }
    if header.cast_flags & CAST_FLAG_AREA_AURA != 0 {
        if caster_guid != target_guid {
            return true;
        }
    } else if header.visual_id != 0 {
        return true;
    }
    ctx.db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .filter(|caster| !caster.is_player() && caster.owner_guid != 0)
        .and_then(|caster| ctx.db.game_creature_template().entry().find(caster.entry))
        .is_some_and(|template| template.creature_type == TOTEM_TYPE)
}

// The 1.12 client chooses buff or debuff frames by slot index rather than aura flags.
fn pick_aura_slot(
    existing: &[(u8, u32)],
    spell_id: u32,
    is_negative: bool,
    buff_slot_count: u8,
    slot_count: u8,
) -> Option<u8> {
    let mut range = if is_negative {
        buff_slot_count..slot_count
    } else {
        0..buff_slot_count
    };
    if let Some(&(slot, _)) = existing
        .iter()
        .find(|(slot, sp)| *sp == spell_id && range.contains(slot))
    {
        return Some(slot);
    }
    range.find(|s| !existing.iter().any(|(es, _)| es == s))
}

fn displace_auras(
    ctx: &ReducerContext,
    target_guid: u64,
    spell_ids: &std::collections::HashSet<u32>,
) {
    if spell_ids.is_empty() {
        return;
    }
    let auras = ctx.db.game_aura();
    let displaced = auras
        .by_target()
        .filter(&target_guid)
        .filter(|a| spell_ids.contains(&a.spell_id))
        .collect();
    shed_auras(ctx, displaced);
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub(crate) fn aura_apply(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    cast_target_guid: u64,
    level: u8,
    points: i32,
    source_item: u64,
) {
    let now = crate::time::now(ctx);
    let points = if e.p0_kind == lyracore_shared::spell::taxonomy::P_COMBAT_PERCENT {
        points.saturating_mul(100)
    } else {
        points
    };
    let points = if e.p0_kind == P_PCT_MAX_POWER {
        let max_power = ctx
            .db
            .game_world_entity()
            .guid()
            .find(caster_guid)
            .map(|c| c.max_power)
            .unwrap_or(0);
        ((max_power as i64) * (points as i64) / 100) as i32
    } else {
        points
    };
    let points = if matches!(e.kind, A_PERIODIC_DAMAGE | A_PERIODIC_HEAL | A_ABSORB) {
        let power = if e.kind == A_PERIODIC_HEAL {
            healing_power(ctx, caster_guid, hdr.school_mask)
        } else {
            spell_power(ctx, caster_guid, hdr.school_mask)
        };
        if e.kind == A_PERIODIC_DAMAGE {
            bonuses::spell_damage_done_float(ctx, hdr, e, caster_guid, points as f32, true) as i32
        } else {
            source_behavior::bonus_magnitude(points, power, e.bonus_coefficient, hdr.custom_flags)
        }
    } else {
        points
    };
    let duration_ms = if hdr.cast_flags & CAST_FLAG_COMBO_FINISHER != 0 {
        let combo = crate::combo::held_points(ctx, caster_guid);
        crate::combo::finisher_duration_ms(hdr.duration_ms, hdr.max_duration_ms, combo)
    } else {
        hdr.duration_ms
    };

    let now_micros = now.to_micros_since_unix_epoch();
    let dr_category =
        crate::spell::stacking::dr_category_for_effect(ctx, e.kind, e.p0_kind, e.p0, target_guid);
    let duration_ms = if let Some(category) = dr_category {
        match crate::spell::stacking::resolve_dr_for_target(ctx, target_guid, category, now_micros)
        {
            crate::spell::stacking::ApplyDecision::Refuse { reason } => {
                log::info!(
                    "DR: spell {} (category {category}) on {target_guid} refused ({reason})",
                    hdr.spell_id
                );
                return;
            }
            crate::spell::stacking::ApplyDecision::Apply {
                duration_scale_bp, ..
            } => crate::spell::stacking::dr_scaled_duration_ms(duration_ms, duration_scale_bp),
        }
    } else {
        duration_ms
    };
    // DBC duration -1 imports as u32::MAX; i64::MAX is the permanent expiry.
    let expires = if duration_ms == u32::MAX {
        Timestamp::from_micros_since_unix_epoch(i64::MAX)
    } else {
        now.checked_add(TimeDuration::from_micros((duration_ms as i64) * 1000))
            .unwrap_or(now)
    };
    // Channels without periodic effects still need the indexed cadence for range checks.
    let period_ms = if e.period_ms == 0 && hdr.cast_flags & CAST_FLAG_CHANNELED != 0 {
        100
    } else {
        e.period_ms
    };
    let next_tick_micros = if period_ms > 0 {
        now.to_micros_since_unix_epoch() + i64::from(period_ms) * 1000
    } else {
        0
    };

    // Recasting the same mount must preserve sibling effects from this cast.
    if e.kind == A_MOUNTED
        && crate::mount::active_mount_spell(ctx, target_guid)
            .is_some_and(|active| active != hdr.spell_id)
    {
        crate::mount::dismount(ctx, target_guid);
    }
    let auras = ctx.db.game_aura();
    if e.kind == A_SEAL {
        let other_seal_spell_ids: std::collections::HashSet<u32> = auras
            .by_target()
            .filter(&target_guid)
            .filter(|a| a.eff_kind == A_SEAL && a.effect_id != e.id)
            .map(|a| a.spell_id)
            .collect();
        displace_auras(ctx, target_guid, &other_seal_spell_ids);
    }
    // Ghost and Wisp Spirit share a DBC name but must coexist.
    if !hdr.name.is_empty()
        && !crate::ghost::is_ghost_spell(hdr.spell_id)
        && !(source_item != 0 && hdr.equip_passive())
    {
        let spells = ctx.db.game_spell();
        let other_rank_ids: std::collections::HashSet<u32> = auras
            .by_target()
            .filter(&target_guid)
            .filter(|a| a.spell_id != hdr.spell_id)
            .map(|a| a.spell_id)
            .filter(|&sp| {
                spells
                    .spell_id()
                    .find(sp)
                    .map(|s| s.name == hdr.name)
                    .unwrap_or(false)
            })
            .collect();
        displace_auras(ctx, target_guid, &other_rank_ids);
    }
    let coexist = source_behavior::aura_casters_coexist(hdr, e.kind);
    let cross_caster_stack =
        hdr.custom_flags & custom::ALLOW_STACK_BETWEEN_CASTER != 0 && hdr.max_stacks > 0;
    if !coexist && !cross_caster_stack && !(source_item != 0 && hdr.equip_passive()) {
        let replaced = auras
            .by_target()
            .filter(&target_guid)
            .filter(|a| {
                a.effect_id == e.id
                    && (a.caster_guid != caster_guid
                        || ((source_item != 0 || !hdr.equip_passive())
                            && a.item_guid(ctx) != source_item))
            })
            .collect();
        shed_auras(ctx, replaced);
    }
    let mut existing = auras.by_target().filter(&target_guid).find(|a| {
        a.effect_id == e.id
            && a.item_guid(ctx) == source_item
            && (cross_caster_stack || a.caster_guid == caster_guid)
    });
    if existing.as_ref().is_some_and(|aura| {
        (aura.eff_kind, aura.eff_p0, aura.eff_p0_kind) != (e.kind, e.p0, e.p0_kind)
    }) {
        shed_auras(ctx, existing.take().into_iter().collect());
    }
    let amount_remaining = if e.kind == A_ABSORB { points.max(0) } else { 0 };
    let channel_target =
        if e.kind == A_PERIODIC_TRIGGER || hdr.cast_flags & CAST_FLAG_CHANNELED != 0 {
            cast_target_guid
        } else {
            0
        };
    // Trigger Auras freeze trigger_spell in eff_p1.
    let eff_p1 = if e.kind == A_PERIODIC_TRIGGER || e.kind == A_PROC_TRIGGER {
        e.trigger_spell as i32
    } else {
        e.p1
    };
    // Refreshing refills proc charges but preserves any running internal cooldown.
    let proc = crate::spell::proc::freeze_profile(ctx, hdr, caster_guid);
    let refreshing = existing.is_some();
    if let Some(mut a) = existing {
        a.stacks = next_stacks(a.stacks, hdr.max_stacks);
        a.expires_at = expires;
        // Shared stacks retain their original caster and that caster's frozen magnitude.
        if !cross_caster_stack || a.caster_guid == caster_guid {
            a.amount = points;
        }
        a.amount_remaining = amount_remaining;
        a.next_tick_micros = next_tick_micros;
        a.period_ms = period_ms;
        a.applied_at = now;
        a.eff_p1 = eff_p1;
        a.channel_target = channel_target;
        a.proc_flags = proc.flags;
        a.proc_chance = proc.chance;
        a.proc_ppm = proc.ppm;
        a.proc_ex = proc.proc_ex;
        a.proc_school_mask = proc.school_mask;
        a.proc_family_name = proc.family_name;
        a.proc_family_flags = proc.family_flags;
        a.proc_charges = proc.charges;
        a.proc_icd_ms = proc.icd_ms;
        a.update(ctx);
    } else {
        // Evictions must free aura slots before this insert chooses one.
        if !(source_item != 0 && hdr.equip_passive())
            && !crate::spell::stacking::apply_group_conflict(
                ctx,
                hdr.spell_id,
                caster_guid,
                target_guid,
                crate::spell::stacking::rank_of(ctx, hdr.spell_id),
                points.saturating_abs(),
            )
        {
            return;
        }
        let existing: Vec<(u8, u32)> = auras
            .by_target()
            .filter(&target_guid)
            .map(|a| {
                (
                    a.slot,
                    if coexist && a.caster_guid != caster_guid && a.spell_id == hdr.spell_id {
                        u32::MAX
                    } else {
                        a.spell_id
                    },
                )
            })
            .collect();
        let visible = aura_has_visible_slot(ctx, hdr, caster_guid, target_guid);
        let Some(slot) = (if !visible {
            Some(u8::MAX)
        } else {
            pick_aura_slot(
                &existing,
                hdr.spell_id,
                hdr.negative(),
                tracer_spell::BUFF_SLOT_COUNT,
                tracer_spell::AURA_SLOTS,
            )
        }) else {
            log::info!(
                "aura slot overflow: all {} slots on {target_guid} full — dropping spell {} (loud: relayed to caster {caster_guid})",
                tracer_spell::AURA_SLOTS,
                hdr.spell_id
            );
            crate::SpellCastEvent::insert(
                ctx,
                SpellCastEvent {
                    is_interrupted: true,
                    ..SpellCastEvent::signal(
                        ctx,
                        caster_guid,
                        hdr.spell_id,
                        SpellCastEventKind::Interrupt,
                    )
                },
            );
            return;
        };
        Aura {
            id: 0,
            target_guid,
            caster_guid,
            spell_id: hdr.spell_id,
            slot,
            level,
            flags: tracer_spell::AURA_FLAGS,
            applied_at: now,
            expires_at: expires,
            effect_id: e.id,
            eff_kind: e.kind,
            amount: points,
            eff_p0: e.p0,
            eff_p0_kind: e.p0_kind,
            eff_p1,
            period_ms,
            amount_remaining,
            stacks: 1,
            next_tick_micros,
            channel_target,
            enters_combat: e.enters_combat,
            proc_flags: proc.flags,
            proc_chance: proc.chance,
            proc_ppm: proc.ppm,
            proc_ex: proc.proc_ex,
            proc_school_mask: proc.school_mask,
            proc_family_name: proc.family_name,
            proc_family_flags: proc.family_flags,
            proc_charges: proc.charges,
            proc_icd_ms: proc.icd_ms,
            proc_ready_micros: 0,
        }
        .insert_from_item(ctx, source_item);
    }
    if e.kind == lyracore_shared::spell::taxonomy::A_SHAPESHIFT {
        let form_id = e.p0.max(0) as u32;
        if refreshing {
            crate::form::refreshed(ctx, target_guid, hdr.spell_id, form_id);
        } else {
            crate::form::entered(ctx, target_guid, hdr.spell_id, form_id);
        }
    }

    source_behavior::finish_aura_apply(ctx, hdr, caster_guid, target_guid);

    // Advance diminishing returns only after successful aura placement.
    if let Some(category) = dr_category {
        crate::spell::stacking::bump_dr_level(
            ctx,
            target_guid,
            category,
            now_micros,
            expires.to_micros_since_unix_epoch(),
        );
    }

    crate::spell::AuraProjections::of(hdr.spell_id, e.kind, e.p0).refresh(ctx, target_guid);

    if hdr.negative() {
        let entities = ctx.db.game_world_entity();
        let caster_is_player = entities
            .guid()
            .find(caster_guid)
            .map(|c| c.is_player())
            .unwrap_or(false);
        let target_is_creature = entities
            .guid()
            .find(target_guid)
            .map(|t| !t.is_player())
            .unwrap_or(false);
        // vmangos AddThreat(caster) on a harmful hit lists the caster with no threat of its own.
        if caster_is_player && target_is_creature {
            crate::threat::add_threat(ctx, target_guid, caster_guid, 0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    cast_target_guid: u64,
    level: u8,
    points: i32,
    dest: Option<(f32, f32, f32)>,
    triggered: bool,
    strike_crit: bool,
    item_target: Option<u64>,
) -> EffectHit {
    use lyracore_shared::spell::taxonomy::EffectKind;

    let kind = match EffectKind::try_from(e.kind) {
        Ok(kind) => kind,
        // Preserve authored high-bit codes as inert auras when no handler exists.
        Err(raw) if raw & KIND_AURA_BIT != 0 => {
            return apply_aura_effect(
                ctx,
                e,
                hdr,
                caster_guid,
                target_guid,
                cast_target_guid,
                level,
                points,
            );
        }
        Err(raw) => {
            log::info!(
                "spell {} effect kind 0x{raw:02x} not implemented (graceful no-op)",
                e.spell_id
            );
            return EffectHit::none();
        }
    };
    match kind {
        EffectKind::SummonGuardians => {
            if let Err(error) = crate::creatures::guardian::summon(ctx, caster_guid, hdr, e, points)
            {
                spacetimedb::log::warn!("guardian summon refused: {error}");
            }
            EffectHit::none()
        }
        EffectKind::SummonHostile => {
            if let Err(error) = crate::creatures::summon_hostile(ctx, caster_guid, hdr, e) {
                spacetimedb::log::warn!("hostile summon refused: {error}");
            }
            EffectHit::none()
        }
        EffectKind::Distract => {
            if let Some(point) = dest {
                crate::creatures::distraction::distract(ctx, target_guid, point, points);
            }
            EffectHit::none()
        }
        EffectKind::AddExtraAttacks => {
            crate::combat::grant_extra_attacks(ctx, target_guid, points);
            EffectHit::none()
        }
        EffectKind::Duel => {
            if e.p0_kind == P_GAMEOBJECT_ENTRY {
                crate::duel::request_duel(ctx, caster_guid, target_guid, e.p0.max(0) as u32);
            }
            EffectHit::none()
        }
        EffectKind::Damage
        | EffectKind::WeaponStrike
        | EffectKind::Judgement
        | EffectKind::FinisherDamage => apply_damage_effect(
            ctx,
            e,
            hdr,
            caster_guid,
            target_guid,
            points,
            triggered,
            strike_crit,
        ),
        EffectKind::Heal
        | EffectKind::HealMaxHealth
        | EffectKind::Energize
        | EffectKind::ConvertResource
        | EffectKind::PowerBurn
        | EffectKind::AddCombo => {
            apply_restore_effect(ctx, e, hdr, caster_guid, target_guid, points, triggered)
        }
        EffectKind::Taunt
        | EffectKind::Interrupt
        | EffectKind::ReduceThreat
        | EffectKind::NextSwing => {
            apply_threat_effect(ctx, e, hdr, caster_guid, target_guid, points)
        }
        EffectKind::Charge | EffectKind::Blink | EffectKind::RecallHome | EffectKind::Dismount => {
            apply_reposition_effect(ctx, e, caster_guid, target_guid)
        }
        // A ground area has no unit target; `place_ground_area` puts it down.
        EffectKind::PersistentArea => EffectHit::none(),
        EffectKind::CreateItem | EffectKind::Pickpocket => {
            apply_inventory_effect(ctx, e, caster_guid, target_guid, points)
        }
        EffectKind::Resurrect
        | EffectKind::SummonPet
        | EffectKind::RevivePet
        | EffectKind::TameCreature
        | EffectKind::SummonObject
        | EffectKind::SummonPlayer
        | EffectKind::LearnSpell => {
            apply_character_effect(ctx, e, caster_guid, target_guid, points)
        }
        EffectKind::SetStance => EffectHit::none(),
        EffectKind::Trigger | EffectKind::Scripted => {
            apply_delegating_effect(ctx, e, caster_guid, target_guid, level)
        }
        EffectKind::SelfResurrect => {
            log::info!(
                "spell {} self-resurrect effect has no cast path (no-op)",
                e.spell_id
            );
            EffectHit::none()
        }
        EffectKind::Dispel => {
            let category = if e.p0 != 0 {
                e.p0 as u8
            } else {
                hdr.dispel_type
            };
            let removed = dispel_target(ctx, caster_guid, target_guid, category);
            log::info!("dispel (cat {category}) on {target_guid} removed {removed} aura(s)");
            EffectHit::none()
        }
        EffectKind::EnchantItemTemporary => {
            enchant_item_temporarily(ctx, e, caster_guid, item_target, points);
            EffectHit::none()
        }
        // vmangos selects no unit for an Inebriate effect without an implicit target.
        EffectKind::Inebriate if e.target == lyracore_shared::spell::taxonomy::T_SCRIPTED => {
            EffectHit::none()
        }
        EffectKind::Inebriate => {
            crate::drunk::inebriate(ctx, target_guid, points);
            EffectHit::none()
        }
        EffectKind::EnchantItem
        | EffectKind::Disenchant
        | EffectKind::Fish
        | EffectKind::OpenLock
        | EffectKind::FeedPet => {
            log::info!(
                "spell {} effect kind 0x{:02x} has no cast path (no-op)",
                e.spell_id,
                e.kind
            );
            EffectHit::none()
        }
        EffectKind::AuraPeriodicDamage
        | EffectKind::AuraPeriodicHeal
        | EffectKind::AuraPeriodicEnergize
        | EffectKind::AuraPeriodicTrigger
        | EffectKind::AuraModStat
        | EffectKind::AuraModResistance
        | EffectKind::AuraAbsorb
        | EffectKind::AuraModDamageDoneFlat
        | EffectKind::AuraModDamageTakenFlat
        | EffectKind::AuraModSchoolCrit
        | EffectKind::AuraModDamageDonePct
        | EffectKind::AuraModHealingTaken
        | EffectKind::AuraInert
        | EffectKind::AuraModSkill
        | EffectKind::AuraModHealing
        | EffectKind::AuraModCombat
        | EffectKind::AuraModSpeed
        | EffectKind::AuraModHealthPower
        | EffectKind::AuraModDamageTaken
        | EffectKind::AuraSeal
        | EffectKind::AuraStealth
        | EffectKind::AuraCombatHealthRegenPct
        | EffectKind::AuraModStatPct
        | EffectKind::AuraProcTrigger
        | EffectKind::AuraSpellmodFlat
        | EffectKind::AuraSpellmodPct
        | EffectKind::AuraDisarm
        | EffectKind::AuraRetaliate
        | EffectKind::AuraShapeshift
        | EffectKind::AuraFormPowerRetained
        | EffectKind::AuraFormEntryChance
        | EffectKind::AuraControl
        | EffectKind::AuraImmunity
        | EffectKind::AuraModDetectRange
        | EffectKind::AuraMounted
        | EffectKind::AuraProcDamage
        | EffectKind::AuraSelfResurrect
        | EffectKind::AuraTransform
        | EffectKind::AuraModPowerRegenPct
        | EffectKind::AuraDamageShield
        | EffectKind::AuraModPowerRegen
        | EffectKind::AuraModHealthRegen
        | EffectKind::AuraModRegen
        | EffectKind::AuraModHealthRegenPct
        | EffectKind::AuraManaRegenInterrupt
        | EffectKind::AuraFlag => apply_aura_effect(
            ctx,
            e,
            hdr,
            caster_guid,
            target_guid,
            cast_target_guid,
            level,
            points,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_aura_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    cast_target_guid: u64,
    level: u8,
    points: i32,
) -> EffectHit {
    aura_apply(
        ctx,
        e,
        hdr,
        caster_guid,
        target_guid,
        cast_target_guid,
        level,
        points,
        0,
    );
    // Check action blocking after placement so the new control aura participates.
    if e.kind == A_CONTROL && is_action_blocked(ctx, target_guid) {
        break_channel(ctx, target_guid);
    }
    EffectHit::none()
}

fn hit_of(
    hdr: &Spell,
    triggered: bool,
    source: crate::combat::HitSource,
    crit: bool,
) -> crate::combat::Hit {
    crate::combat::Hit::spell(
        if triggered {
            crate::combat::HitSource::Triggered
        } else {
            source
        },
        hdr.spell_id,
        crit,
    )
}

fn direct_spell_crit_chance(
    ctx: &ReducerContext,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
) -> f32 {
    match hdr.damage_class {
        2 | 3 => {
            let entities = ctx.db.game_world_entity();
            let (Some(caster), Some(target)) = (
                entities.guid().find(caster_guid),
                entities.guid().find(target_guid),
            ) else {
                return 0.0;
            };
            if !caster.is_player() && (caster.owner_guid == 0 || caster.owner_guid >> 48 != 0) {
                return 0.0;
            }
            let attack_type = if hdr.damage_class == 3 {
                crate::combat::AttackType::Ranged
            } else {
                crate::combat::AttackType::Melee
            };
            crate::combat::attack_table_for(
                ctx,
                &caster,
                &target,
                attack_type,
                Some(crate::combat::DeliveredSpell::cast(hdr)),
            )
            .crit as f32
                / 100.0
        }
        _ if hdr.school_mask & RESIST_ARMOR != 0 => 0.0,
        _ => {
            spell_crit_chance(ctx, caster_guid)
                + if hdr.damage_class == 1 {
                    bonuses::school_crit(ctx, caster_guid, 1 << school_index(hdr.school_mask))
                } else {
                    0.0
                }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_damage_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
    triggered: bool,
    strike_crit: bool,
) -> EffectHit {
    match e.kind {
        E_DAMAGE | E_FINISHER_DAMAGE => {
            let scaled =
                bonuses::spell_damage_done_float(ctx, hdr, e, caster_guid, points as f32, false);
            let is_crit = if e.kind == E_FINISHER_DAMAGE {
                strike_crit
            } else {
                let draw = crate::conformance_rolls::draw(
                    ctx,
                    crate::conformance_rolls::RollStream::Caster(caster_guid),
                );
                let chance = direct_spell_crit_chance(ctx, hdr, caster_guid, target_guid);
                hdr.cast_flags & CAST_FLAG_CANNOT_CRIT == 0
                    && hdr.attributes_ex_3 & 0x2000_0000 == 0
                    && crate::spell::rolls::chance_succeeds(chance, draw)
            };
            let scaled =
                bonuses::spell_damage_taken(ctx, hdr, e, caster_guid, target_guid, scaled, 1);
            let crit_scaled = if is_crit {
                if matches!(hdr.damage_class, 2 | 3) {
                    ((scaled as i32) * 2) as f32
                } else {
                    apply_spell_crit(scaled as i32, true) as f32
                }
            } else {
                scaled
            };
            let source_behavior::SpellMitigation {
                after_resist,
                resisted,
                blocked,
            } = source_behavior::mitigate_spell(ctx, hdr, caster_guid, target_guid, crit_scaled);

            if e.kind == E_DAMAGE && hdr.range_yd > 0 && target_guid != caster_guid {
                if let (Some(caster), Some(target)) = (
                    ctx.db.game_world_entity().guid().find(caster_guid),
                    ctx.db.game_world_entity().guid().find(target_guid),
                ) {
                    if caster.map_id == target.map_id && caster.instance_id == target.instance_id {
                        let dist =
                            distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z);
                        let travel_ms = projectile_travel_ms(dist, PROJECTILE_SPEED_YPS);
                        if travel_ms > 0 {
                            let fire_at = crate::time::now(ctx)
                                .checked_add(TimeDuration::from_micros((travel_ms as i64) * 1000))
                                .unwrap_or(crate::time::now(ctx));
                            ctx.db
                                .game_pending_spell_impact()
                                .insert(PendingSpellImpact {
                                    scheduled_id: 0,
                                    scheduled_at: crate::time::schedule_time(ctx, fire_at),
                                    caster_guid,
                                    target_guid,
                                    spell_id: hdr.spell_id,
                                    school_index: crate::spell::school_index(hdr.school_mask),
                                    is_crit,
                                    resisted,
                                    after_resist,
                                    triggered,
                                    blocked,
                                });
                            // Impact emits the damage log; this GO must carry no damage.
                            return EffectHit::none();
                        }
                    }
                }
            }
            // vmangos deals spell damage only when it is non-zero before block, absorb and resist.
            let (dealt, absorbed) = if after_resist > 0 || resisted > 0 || blocked > 0 {
                crate::combat::deal_blocked_spell_damage(
                    ctx,
                    caster_guid,
                    target_guid,
                    after_resist,
                    blocked,
                    hit_of(
                        hdr,
                        triggered,
                        if e.kind == E_FINISHER_DAMAGE {
                            crate::combat::HitSource::MeleeSpell
                        } else {
                            crate::combat::HitSource::Spell
                        },
                        is_crit,
                    ),
                )
            } else {
                (0, 0)
            };
            EffectHit {
                dealt,
                resisted,
                absorbed,
                blocked,
                crit: is_crit,
                ..EffectHit::default()
            }
        }
        E_WEAPON_STRIKE => {
            let entities = ctx.db.game_world_entity();
            let (Some(caster), Some(target)) = (
                entities.guid().find(caster_guid),
                entities.guid().find(target_guid),
            ) else {
                return EffectHit::none();
            };
            let strike = crate::combat::WeaponStrike {
                bonus: points,
                percent: e.p0.max(0) as u32,
                normalized: e.p1 != 0,
                ranged: hdr.attributes & SPELL_ATTR_USES_RANGED_SLOT != 0,
            };
            let dmg = crate::combat::weapon_strike_spell_damage(
                ctx,
                &caster,
                &target,
                strike,
                hdr,
                strike_crit,
            );
            if strike.ranged && target_guid != caster_guid {
                // The impact emits the damage log, so this GO carries no damage.
                crate::combat::launch_ranged_strike(ctx, &caster, &target, dmg, hdr, strike_crit);
                return EffectHit::none();
            }
            let (dmg, blocked) = if target_guid == caster_guid {
                (dmg, 0)
            } else {
                crate::combat::block_cast_spell(
                    ctx,
                    &caster,
                    &target,
                    hdr,
                    crate::combat::AttackType::Melee,
                    dmg,
                )
            };
            let (dealt, absorbed) = crate::combat::deal_blocked_spell_damage(
                ctx,
                caster_guid,
                target_guid,
                dmg as i32,
                blocked,
                hit_of(
                    hdr,
                    triggered,
                    crate::combat::HitSource::MeleeSpell,
                    strike_crit,
                ),
            );
            EffectHit {
                blocked,
                crit: strike_crit,
                ..EffectHit::dmg(dealt, absorbed)
            }
        }
        E_JUDGEMENT => {
            let seal = seal_amount(ctx, caster_guid);
            if seal > 0 {
                let seal =
                    bonuses::spell_damage_done_float(ctx, hdr, e, caster_guid, seal as f32, false);
                let seal =
                    bonuses::spell_damage_taken(ctx, hdr, e, caster_guid, target_guid, seal, 1);
                let (dealt, absorbed) = crate::combat::deal_damage(
                    ctx,
                    caster_guid,
                    target_guid,
                    bonuses::round_damage(ctx, target_guid, seal),
                    hit_of(hdr, triggered, crate::combat::HitSource::Spell, false),
                );
                remove_seal_auras(ctx, caster_guid);
                EffectHit::dmg(dealt, absorbed)
            } else {
                EffectHit::none()
            }
        }
        _ => EffectHit::none(),
    }
}

fn apply_restore_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
    triggered: bool,
) -> EffectHit {
    match e.kind {
        E_HEAL => apply_heal(ctx, hdr, e, caster_guid, target_guid, points),
        E_HEAL_MAX_HEALTH => {
            if let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) {
                let before = t.health;
                t.health = t.max_health;
                let effective = t.health.saturating_sub(before);
                t.update(ctx);
                crate::threat::add_spell_heal_threat(
                    ctx,
                    caster_guid,
                    target_guid,
                    effective,
                    hdr.spell_id,
                    crate::threat::HealDelivery::Direct,
                );
            }
            EffectHit::none()
        }
        E_ENERGIZE => {
            let mut hit = EffectHit::none();
            if let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) {
                if let Some(pool) = t.energized_pool(e.p0_kind, e.p0) {
                    t.gain_power(pool.power_type, points.max(0) as u32);
                    // vmangos `EnergizeBySpell` logs the full grant, not the gain.
                    hit.energized = points.max(0) as u32;
                    hit.power_type = pool.power_type;
                    t.update(ctx);
                }
            }
            if e.enters_combat {
                crate::combat::enter_combat(ctx, caster_guid);
            }
            hit
        }
        E_CONVERT_RESOURCE => {
            convert_health_to_power(ctx, caster_guid, points);
            EffectHit::none()
        }
        E_POWER_BURN => {
            let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) else {
                return EffectHit::none();
            };
            if ((t.unit_bytes_0 >> 24) & 0xFF) as u8 != lyracore_shared::packing::power_type::MANA {
                return EffectHit::none();
            }
            let drained = mana_burn_drain(t.power, points);
            if drained == 0 {
                return EffectHit::none();
            }
            t.power -= drained;
            t.update(ctx);
            let damage = bonuses::spell_damage_done_float(
                ctx,
                hdr,
                e,
                caster_guid,
                mana_burn_damage(drained, e.p1) as f32,
                false,
            );
            let damage =
                bonuses::spell_damage_taken(ctx, hdr, e, caster_guid, target_guid, damage, 1);
            let (dealt, absorbed) = crate::combat::deal_damage(
                ctx,
                caster_guid,
                target_guid,
                bonuses::round_damage(ctx, target_guid, damage),
                hit_of(hdr, triggered, crate::combat::HitSource::Spell, false),
            );
            EffectHit::dmg(dealt, absorbed)
        }
        E_ADD_COMBO => {
            crate::combo::add_combo_point(ctx, caster_guid, target_guid);
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

fn apply_threat_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
) -> EffectHit {
    match e.kind {
        E_TAUNT => {
            if ctx
                .db
                .game_world_entity()
                .guid()
                .find(target_guid)
                .map(|t| !t.is_player())
                .unwrap_or(false)
            {
                crate::threat::taunt(ctx, target_guid, caster_guid);
            }
            EffectHit::none()
        }
        E_INTERRUPT => {
            let lockout_ms = if hdr.duration_ms > 0 {
                hdr.duration_ms as i64
            } else {
                5_000
            };
            interrupt_cast_and_lock(ctx, target_guid, lockout_ms);
            EffectHit::none()
        }
        E_REDUCE_THREAT => {
            // Vanilla Feint encodes threat reduction as negative points.
            crate::threat::reduce_threat_from_source(ctx, caster_guid, (points as i64).abs());
            EffectHit::none()
        }
        E_NEXT_SWING => {
            // Vanilla charges a queued strike when its swing fires, including a miss.
            crate::combat::queued_strike::queue(ctx, caster_guid, e.spell_id);
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

fn apply_reposition_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
) -> EffectHit {
    match e.kind {
        E_CHARGE => {
            charge_to_target(ctx, caster_guid, target_guid);
            EffectHit::none()
        }
        E_BLINK => {
            blink_forward(ctx, caster_guid, e.radius_yd);
            EffectHit::none()
        }
        E_RECALL_HOME => {
            if let Err(refusal) = crate::character::recall_to_home(ctx, caster_guid) {
                spacetimedb::log::info!("recall: {caster_guid} stays put: {refusal}");
            }
            EffectHit::none()
        }
        E_DISMOUNT => {
            crate::mount::dismount(ctx, target_guid);
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

/// vmangos `Spell::DoCreateItem`: another caster's cast fills the bags of the Character it targets.
fn create_item_recipient(ctx: &ReducerContext, caster_guid: u64, target_guid: u64) -> u64 {
    let is_character = |guid| {
        ctx.db
            .game_world_entity()
            .guid()
            .find(guid)
            .is_some_and(|entity| entity.is_player())
    };
    if !is_character(caster_guid) && is_character(target_guid) {
        target_guid
    } else {
        caster_guid
    }
}

fn apply_inventory_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
) -> EffectHit {
    match e.kind {
        E_CREATE_ITEM => {
            let recipient = create_item_recipient(ctx, caster_guid, target_guid);
            let entry = e.p0.max(0) as u32;
            let count = points.max(1) as u32;
            if entry == 0 {
                log::info!(
                    "spell {} E_CREATE_ITEM with no item entry (p0=0) — skipped",
                    e.spell_id
                );
                return EffectHit {
                    item: ItemGrantOutcome::Refused,
                    ..EffectHit::default()
                };
            } else if let Err(err) = crate::items::grant_item(ctx, recipient, entry, count) {
                log::info!(
                    "spell {} E_CREATE_ITEM: could not grant {count}x item {entry} to {recipient}: {err}",
                    e.spell_id
                );
                return EffectHit {
                    item: ItemGrantOutcome::Refused,
                    ..EffectHit::default()
                };
            }
            EffectHit {
                item: ItemGrantOutcome::Granted,
                ..EffectHit::default()
            }
        }
        E_PICKPOCKET => {
            let entities = ctx.db.game_world_entity();
            if let Some(mut target) = entities.guid().find(target_guid) {
                if !target.is_player() && !target.dead && !target.pickpocketed {
                    let copper = ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(target.entry)
                        .filter(|t| t.money_max > 0)
                        .map(|t| crate::combat::roll_money(ctx, t.money_min, t.money_max))
                        .unwrap_or_else(|| pickpocket_copper(target.level));
                    if let Some(mut rogue) = entities.guid().find(caster_guid) {
                        rogue.money = rogue.money.saturating_add(copper);
                        rogue.update(ctx);
                    }

                    let target_entry = target.entry;
                    target.pickpocketed = true;
                    target.update(ctx);
                    crate::loot::roll_pickpocket_loot(ctx, target_entry, target_guid, caster_guid);
                }
            }
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

fn apply_delegating_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
    level: u8,
) -> EffectHit {
    match e.kind {
        E_TRIGGER => {
            if e.trigger_spell != 0 && e.trigger_spell != e.spell_id {
                if let Err(err) = super::resolve::resolve_trigger_cast(
                    ctx,
                    &CastRequest::new(caster_guid, e.trigger_spell, level, target_guid),
                ) {
                    log::info!(
                        "spell {} triggered {} which did not resolve: {}",
                        e.spell_id,
                        e.trigger_spell,
                        err
                    );
                }
            }
            EffectHit::none()
        }
        E_SCRIPTED => {
            if e.script_id != 0 {
                // An earlier effect can change or remove this script after the cast Gates ran.
                if let Some(script) = ctx.db.game_script().script_id().find(e.script_id) {
                    if script.enabled {
                        let event = crate::runtime_script::ScriptEvent {
                            name: format!("spell_effect:{}:{}", e.spell_id, e.effect_index),
                            actor: crate::runtime_script::EntityView::read(ctx, caster_guid),
                            target: crate::runtime_script::EntityView::read(ctx, target_guid),
                            ..crate::runtime_script::ScriptEvent::default()
                        };
                        crate::runtime_script::invoke_by_identity(
                            ctx,
                            crate::runtime_script::RuntimeScript {
                                name: &script.name,
                                source: &script.source,
                            },
                            &event,
                        );
                    }
                }
            }
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

fn apply_character_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
) -> EffectHit {
    match e.kind {
        E_RESURRECT => {
            if let Some(t) = ctx.db.game_world_entity().guid().find(target_guid) {
                if t.dead && t.is_player() {
                    let caster_name = ctx
                        .db
                        .game_character()
                        .guid()
                        .find(caster_guid)
                        .map(|c| c.name)
                        .unwrap_or_default();
                    ResurrectRequest {
                        target_guid,
                        target_identity: t.owner_identity,
                        caster_guid,
                        caster_name,
                        points,
                        created_at: crate::time::now(ctx),
                    }
                    .insert(ctx);
                }
            }
            EffectHit::none()
        }
        E_SUMMON_PET => {
            // Call Pet (883) is SPELL_EFFECT_SUMMON_PET with no creature entry.
            match e.p0.max(0) as u32 {
                0 => crate::creatures::call_hunter_pet(ctx, caster_guid),
                entry => crate::creatures::summon_pet(ctx, caster_guid, entry),
            }
            EffectHit::none()
        }
        E_REVIVE_PET => {
            crate::creatures::revive_hunter_pet(ctx, caster_guid, points);
            EffectHit::none()
        }
        E_TAME_CREATURE => {
            if let Err(error) = crate::creatures::tame_creature(ctx, caster_guid, target_guid) {
                log::info!("tame completion for caster {caster_guid} and target {target_guid} failed: {error}");
            }
            EffectHit::none()
        }
        E_SUMMON_OBJECT => {
            crate::summoning_ritual::place(ctx, caster_guid, e.spell_id, e.p0.max(0) as u32);
            EffectHit::none()
        }
        E_SUMMON_PLAYER => {
            crate::summoning_ritual::summon_to_caster(ctx, caster_guid, target_guid);
            EffectHit::none()
        }
        E_LEARN_SPELL => {
            crate::spell::learn_from_effect(ctx, target_guid, e.trigger_spell);
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

fn apply_heal(
    ctx: &ReducerContext,
    hdr: &Spell,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
) -> EffectHit {
    let fixed =
        hdr.custom_flags & custom::FIXED_DAMAGE != 0 || hdr.attributes_ex_3 & 0x2000_0000 != 0;
    let power = if fixed {
        0
    } else {
        healing_power(ctx, caster_guid, hdr.school_mask)
    };
    let scaled = points as f32
        + power as f32 * bonuses::spell_coefficient(ctx, hdr, e, caster_guid, caster_guid, true);
    let scaled = bonuses::healing_taken(ctx, hdr, e, target_guid, scaled, 1);
    let chance = if hdr.family_name == 13 {
        10.0
    } else if hdr.damage_class == 1 {
        spell_crit_chance(ctx, caster_guid)
            + bonuses::school_crit(ctx, caster_guid, 1 << school_index(hdr.school_mask))
    } else {
        0.0
    };
    let draw = crate::conformance_rolls::draw(
        ctx,
        crate::conformance_rolls::RollStream::Caster(caster_guid),
    );
    let crit = !fixed
        && hdr.cast_flags & CAST_FLAG_CANNOT_CRIT == 0
        && crate::spell::rolls::chance_succeeds(chance, draw);
    let scaled = if crit {
        apply_spell_crit(scaled as i32, true)
    } else {
        bonuses::round_damage(ctx, target_guid, scaled)
    };
    let healed = heal_unit(ctx, target_guid, scaled);
    crate::threat::add_spell_heal_threat(
        ctx,
        caster_guid,
        target_guid,
        healed,
        hdr.spell_id,
        crate::threat::HealDelivery::Direct,
    );
    EffectHit {
        healed,
        crit,
        ..EffectHit::default()
    }
}

pub(crate) fn apply_direct_heal(
    ctx: &ReducerContext,
    healer_guid: u64,
    target_guid: u64,
    basis: i32,
) -> u32 {
    let effective = heal_unit(ctx, target_guid, basis);
    crate::threat::add_heal_threat(ctx, healer_guid, target_guid, effective);
    effective
}

fn heal_unit(ctx: &ReducerContext, target_guid: u64, basis: i32) -> u32 {
    let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return 0;
    };
    let before = t.health;
    t.health = healed_value(t.health, t.max_health, basis);
    let effective = t.health.saturating_sub(before);
    t.update(ctx);
    effective
}

fn convert_health_to_power(ctx: &ReducerContext, caster_guid: u64, amount: i32) {
    let amount = amount.max(0) as u32;
    if let Some(mut c) = ctx.db.game_world_entity().guid().find(caster_guid) {
        let drain = amount.min(c.health.saturating_sub(1));
        c.health -= drain;
        c.power = energized_value(c.power, c.max_power, drain as i32);
        c.update(ctx);
    }
}

fn mana_burn_drain(power: u32, points: i32) -> u32 {
    (points.max(0) as u32).min(power)
}

// ratio_bp is a percent despite its name; Mana Burn imports 0.5 as 50.
fn mana_burn_damage(drained: u32, ratio_bp: i32) -> u32 {
    let ratio_bp = if ratio_bp <= 0 { 100 } else { ratio_bp as u32 };
    (drained as u64 * ratio_bp as u64 / 100) as u32
}

fn charge_stop(cx: f32, cy: f32, tx: f32, ty: f32, stop_yd: f32) -> (f32, f32) {
    let (dx, dy) = (cx - tx, cy - ty);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= stop_yd {
        (cx, cy)
    } else {
        (tx + dx / dist * stop_yd, ty + dy / dist * stop_yd)
    }
}

fn blink_dest(x: f32, y: f32, orientation: f32, yd: f32) -> (f32, f32) {
    (x + orientation.cos() * yd, y + orientation.sin() * yd)
}

// One-yard clearance keeps the caster collision radius outside the hit plane.
const BLINK_CLEARANCE_YD: f32 = 1.0;

pub(crate) fn blink_forward(ctx: &ReducerContext, caster_guid: u64, dist_yd: f32) {
    let Some(caster) = ctx.db.game_world_entity().guid().find(caster_guid) else {
        return;
    };
    let (fx, fy) = blink_dest(caster.x, caster.y, caster.orientation, dist_yd);
    let a = [caster.x, caster.y, caster.z];
    let b = [fx, fy, caster.z];
    let mut yd = match crate::vmap::collision_ray(ctx, caster.map_id, caster.instance_id, a, b) {
        Some(hit) => {
            let dx = hit[0] - caster.x;
            let dy = hit[1] - caster.y;
            (dx * dx + dy * dy).sqrt() - BLINK_CLEARANCE_YD
        }
        None => dist_yd,
    };
    while yd > 0.0 {
        let (nx, ny) = blink_dest(caster.x, caster.y, caster.orientation, yd);
        if crate::vmap::vmap_enabled(ctx, caster.map_id)
            || crate::nav::has_los(
                ctx,
                caster.map_id,
                caster.instance_id,
                (caster.x, caster.y, caster.z),
                (nx, ny, caster.z),
            )
        {
            let pose = [nx, ny, caster.z, caster.orientation];
            let (map_id, instance_id) = (caster.map_id, caster.instance_id);
            if let Err(refusal) =
                crate::world::teleport(ctx, caster_guid, map_id, instance_id, pose)
            {
                spacetimedb::log::info!("blink: {caster_guid} stays put: {refusal}");
            }
            return;
        }
        yd -= 2.0;
    }
}

/// A ground point wins; else a `T_SELF` area goes on the caster, any other on the cast's target.
pub(crate) fn place_ground_area(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    cast_target_guid: u64,
    amount: i32,
    dest: Option<(f32, f32, f32)>,
) {
    let anchor_guid = if e.target == T_SELF || cast_target_guid == 0 {
        caster_guid
    } else {
        cast_target_guid
    };
    create_ground_area(ctx, e, hdr, caster_guid, anchor_guid, amount, dest);
}

/// Places the area at the cast's ground point, else at `anchor_guid` (vmangos `setUnitTarget`).
fn create_ground_area(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    anchor_guid: u64,
    amount: i32,
    dest: Option<(f32, f32, f32)>,
) {
    let entities = ctx.db.game_world_entity();
    let (map_id, instance_id, x, y, z) = match dest {
        Some((dx, dy, dz)) => {
            let Some(caster) = entities.guid().find(caster_guid) else {
                return;
            };
            (caster.map_id, caster.instance_id, dx, dy, dz)
        }
        None => {
            let Some(anchor) = entities.guid().find(anchor_guid) else {
                return;
            };
            (
                anchor.map_id,
                anchor.instance_id,
                anchor.x,
                anchor.y,
                anchor.z,
            )
        }
    };
    let now = crate::time::now(ctx);
    if hdr.duration_ms == 0 || hdr.duration_ms == u32::MAX {
        return;
    }
    let Some(expires_at) =
        now.checked_add(TimeDuration::from_micros((hdr.duration_ms as i64) * 1000))
    else {
        return;
    };
    let period_ms = if e.period_ms > 0 { e.period_ms } else { 1000 };
    crate::spell::scheduler::arm_ground_area_tick(ctx);
    let area = ctx.db.game_ground_area().insert(GroundArea {
        id: 0,
        spell_id: e.spell_id,
        caster_guid,
        map_id,
        instance_id,
        x,
        y,
        z,
        radius_yd: e.radius_yd,
        amount: bonuses::spell_damage_done_float(ctx, hdr, e, caster_guid, amount as f32, true)
            as i32,
        school_mask: hdr.school_mask,
        period_ms,
        next_tick_micros: now.to_micros_since_unix_epoch() + (period_ms as i64) * 1000,
        expires_at,
        effect_id: e.id,
    });
    crate::DynamicObject {
        guid: dynamic_object_guid(area.id),
        caster_guid,
        spell_id: e.spell_id,
        map_id,
        instance_id,
        x,
        y,
        z,
        radius_yd: e.radius_yd,
        cell: 0,
    }
    .insert(ctx);
    // A channel Aura from an earlier effect of the spell has already started the channel bar.
    let bar_started = ctx
        .db
        .game_aura()
        .by_caster()
        .filter(caster_guid)
        .any(|aura| aura.spell_id == e.spell_id);
    if hdr.cast_flags & CAST_FLAG_CHANNELED != 0 && !bar_started {
        crate::native::aura::channel_started(ctx, caster_guid, e.spell_id, hdr.duration_ms);
    }
}

fn dynamic_object_guid(area_id: u64) -> u64 {
    lyracore_shared::guid::compose(lyracore_shared::guid::DYNAMIC_OBJECT, area_id)
}

/// A channelled ground area lasts only as long as its caster's channel (vmangos `Spell::cancel`).
pub(crate) fn is_channelled_area(ctx: &ReducerContext, area: &GroundArea) -> bool {
    ctx.db
        .game_spell()
        .spell_id()
        .find(area.spell_id)
        .is_some_and(|spell| spell.cast_flags & CAST_FLAG_CHANNELED != 0)
}

pub(crate) fn holds_channelled_area(ctx: &ReducerContext, caster_guid: u64, spell_id: u32) -> bool {
    ctx.db
        .game_ground_area()
        .by_caster()
        .filter(caster_guid)
        .any(|area| area.spell_id == spell_id && is_channelled_area(ctx, &area))
}

/// Removes the area and its DynamicObject; the caster of a channelled area leaves its channel.
pub(crate) fn end_ground_area(ctx: &ReducerContext, area: &GroundArea) {
    ctx.db.game_ground_area().id().delete(area.id);
    crate::DynamicObject::delete(ctx, dynamic_object_guid(area.id));
    if is_channelled_area(ctx, area) {
        crate::native::aura::channel_ended(ctx, area.caster_guid);
    }
}

fn charge_to_target(ctx: &ReducerContext, caster_guid: u64, target_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    let (sx, sy) = charge_stop(caster.x, caster.y, target.x, target.y, 3.0);
    let (nx, ny) = charge_clamp(ctx, caster.map_id, caster.x, caster.y, sx, sy);
    let facing = (target.y - caster.y).atan2(target.x - caster.x);
    let (ddx, ddy) = (nx - caster.x, ny - caster.y);
    let dist = (ddx * ddx + ddy * ddy).sqrt();
    if dist < 0.5 {
        if let Some(mut c) = entities.guid().find(caster_guid) {
            c.orientation = facing;
            c.update(ctx);
        }
        return;
    }
    let nz = crate::terrain::snap_z(ctx, caster.map_id, caster.instance_id, nx, ny, target.z);
    let duration_ms = ((dist / lyracore_shared::constants::speeds::CHARGE) * 1000.0) as u32;
    let course = crate::creatures::leg::Course::straight((nx, ny, nz), duration_ms, true);
    let mut c = caster;
    if !crate::creatures::leg::begin(ctx, &mut c, course) {
        return;
    }
    // The spline animates the client; combat needs the authoritative landing position immediately.
    let now_ms = crate::time::now_ms_u32(ctx);
    let clock = crate::world::MoveClock::realm(ctx, &c, now_ms);
    c.place(nx, ny, nz, clock).unexplored();
    c.orientation = facing;
    c.update(ctx);
}

// Vanilla Charge stops at obstructions instead of routing around them.
fn charge_clamp(
    ctx: &ReducerContext,
    map_id: u32,
    cx: f32,
    cy: f32,
    sx: f32,
    sy: f32,
) -> (f32, f32) {
    if !crate::nav::nav_enabled(ctx) {
        return (sx, sy);
    }
    let (dx, dy) = (sx - cx, sy - cy);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 1.0 {
        return (sx, sy);
    }
    let steps = dist.ceil() as i32;
    let (mut lx, mut ly) = (cx, cy);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let (px, py) = (cx + dx * t, cy + dy * t);
        if crate::nav::walkable(ctx, map_id, px, py) == Some(false) {
            return (lx, ly);
        }
        (lx, ly) = (px, py);
    }
    (sx, sy)
}

pub(crate) fn revive_amount(max: u32, pct: i32) -> u32 {
    let pct = pct.clamp(1, 100) as u32;
    (max.saturating_mul(pct) / 100).max(1)
}

fn pickpocket_copper(level: u32) -> u32 {
    level.saturating_mul(13).max(1)
}

#[cfg(test)]
mod effect_handler_tests {
    #[test]
    fn revive_amount_is_pct_of_max_floored_at_one() {
        assert_eq!(super::revive_amount(1000, 70), 700);
        assert_eq!(super::revive_amount(0, 70), 1);
        assert_eq!(super::revive_amount(1000, 0), 10);
        assert_eq!(super::revive_amount(1000, 100), 1000);
    }

    #[test]
    fn pickpocket_copper_scales_with_level_and_is_never_zero() {
        assert_eq!(super::pickpocket_copper(10), 130);
        assert_eq!(super::pickpocket_copper(0), 1);
    }

    #[test]
    fn mana_burn_drain_caps_at_current_power() {
        assert_eq!(super::mana_burn_drain(204, 100), 100);
        assert_eq!(super::mana_burn_drain(40, 100), 40);
        assert_eq!(super::mana_burn_drain(0, 100), 0);
    }

    #[test]
    fn mana_burn_damage_applies_the_ratio_and_floors() {
        assert_eq!(super::mana_burn_damage(100, 50), 50);
        assert_eq!(super::mana_burn_damage(41, 50), 20);
        assert_eq!(super::mana_burn_damage(0, 50), 0);
    }

    #[test]
    fn mana_burn_damage_defaults_missing_ratio_to_1to1() {
        assert_eq!(super::mana_burn_damage(100, 0), 100);
        assert_eq!(super::mana_burn_damage(100, -5), 100);
    }

    #[test]
    fn charge_stops_short_of_the_target() {
        let (x, y) = super::charge_stop(0.0, 0.0, 10.0, 0.0, 3.0);
        assert!((x - 7.0).abs() < 0.01 && y.abs() < 0.01);
        assert_eq!(super::charge_stop(9.0, 0.0, 10.0, 0.0, 3.0), (9.0, 0.0));
    }

    #[test]
    fn blink_lands_the_full_distance_forward_along_facing() {
        let (x, y) = super::blink_dest(0.0, 0.0, 0.0, 20.0);
        assert!((x - 20.0).abs() < 0.01 && y.abs() < 0.01);
        let (x, y) = super::blink_dest(0.0, 0.0, std::f32::consts::FRAC_PI_2, 20.0);
        assert!(x.abs() < 0.01 && (y - 20.0).abs() < 0.01);
        let (x, y) = super::blink_dest(5.0, 3.0, std::f32::consts::PI, 20.0);
        assert!((x - -15.0).abs() < 0.01 && (y - 3.0).abs() < 0.01);
    }

    #[test]
    fn next_stacks_increments_and_caps_and_is_noop_for_non_stacking() {
        assert_eq!(super::next_stacks(1, 5), 2);
        assert_eq!(super::next_stacks(4, 5), 5);
        assert_eq!(super::next_stacks(5, 5), 5);
        assert_eq!(super::next_stacks(1, 0), 1);
        assert_eq!(super::next_stacks(1, 1), 1);
        assert_eq!(super::next_stacks(0, 5), 2);
        assert_eq!(super::next_stacks(0, 0), 1);
    }

    #[test]
    fn aoe_keep_in_radius_and_faction() {
        let r2 = 8.0f32 * 8.0;
        assert!(super::aoe_keep(36.0, r2, true, false, true, false));
        assert!(!super::aoe_keep(100.0, r2, true, false, true, false));
        assert!(!super::aoe_keep(4.0, r2, true, true, true, false));
        assert!(!super::aoe_keep(4.0, r2, true, false, false, true));
        assert!(!super::aoe_keep(4.0, r2, true, false, false, false));
        assert!(super::aoe_keep(36.0, r2, false, false, false, true));
        assert!(super::aoe_keep(4.0, r2, false, true, false, false));
        assert!(!super::aoe_keep(4.0, r2, false, false, true, false));
        assert!(!super::aoe_keep(100.0, r2, false, false, false, true));
    }

    #[test]
    fn a_capped_area_keeps_its_cap_and_always_the_casts_target() {
        let hits: Vec<u64> = (1..=25).collect();
        for seed in [0, 1, 7, 0xDEAD_BEEF, u32::MAX] {
            let kept = super::keep_affected_targets(hits.clone(), 3, 19, seed);
            assert_eq!(kept.len(), 3, "seed {seed}");
            assert!(kept.contains(&19), "seed {seed}");
        }
    }

    #[test]
    fn a_capped_area_does_not_always_keep_the_same_units() {
        let hits: Vec<u64> = (1..=25).collect();
        let picks: std::collections::BTreeSet<Vec<u64>> = (0..20)
            .map(|seed| super::keep_affected_targets(hits.clone(), 2, 0, seed))
            .collect();
        assert!(picks.len() > 1, "{picks:?}");
    }

    #[test]
    fn one_cast_keeps_the_same_units_when_another_unit_drops_out() {
        let hits: Vec<u64> = (100..120).collect();
        let first = super::keep_affected_targets(hits.clone(), 4, 0, 42);
        let dropped = hits.iter().find(|guid| !first.contains(guid)).copied();
        let rest: Vec<u64> = hits.into_iter().filter(|&g| Some(g) != dropped).collect();
        assert_eq!(super::keep_affected_targets(rest, 4, 0, 42), first);
    }

    #[test]
    fn a_capped_effect_never_reaches_a_unit_its_cap_did_not_pick() {
        let mut picks = super::CappedPicks::default();
        let first = picks.pick(0, vec![11, 12], 1, 0, || 7);
        assert_eq!(first.len(), 1);
        assert_eq!(
            picks.pick(1, vec![11, 12], 1, 0, || 99),
            first,
            "effects agree"
        );
        let other = if first[0] == 11 { 12 } else { 11 };
        assert_eq!(
            picks.pick(1, vec![other], 1, 0, || 99),
            Vec::<u64>::new(),
            "the picked unit died before the later effect"
        );
        assert_eq!(
            picks.pick(0, vec![other, first[0], 13], 1, 0, || 99),
            first,
            "a unit that walked in is not picked"
        );
    }

    #[test]
    fn a_capped_effect_within_its_cap_keeps_every_unit_but_no_newcomer() {
        let mut picks = super::CappedPicks::default();
        assert_eq!(picks.pick(0, vec![1, 2], 3, 0, || 0), vec![1, 2]);
        assert_eq!(picks.pick(0, vec![1, 2, 3], 3, 0, || 0), vec![1, 2]);
    }

    fn nearest_chain(first: (u64, f32), units: &[(u64, f32)], count: usize) -> Vec<u64> {
        let unit = |&(guid, x): &(u64, f32)| super::ChainUnit::at(guid, (x, 0.0, 0.0));
        let units = units.iter().map(unit).collect();
        super::chain_jumps(
            unit(&first),
            units,
            count,
            |_, _| true,
            super::chain_distance_sq,
        )
    }

    fn injured(guid: u64, x: f32, group: Option<u64>, missing_health: i64) -> super::ChainUnit {
        super::ChainUnit {
            guid,
            at: (x, 0.0, 0.0),
            group,
            missing_health,
            player: false,
        }
    }

    fn healing_chain(
        first: super::ChainUnit,
        units: Vec<super::ChainUnit>,
        count: usize,
    ) -> Vec<u64> {
        super::chain_jumps(first, units, count, |_, _| true, super::chain_healing_rank)
    }

    #[test]
    fn a_chain_jumps_from_its_target_to_the_nearest_unit_within_ten_yards() {
        let units = [(1, 0.0), (2, 9.0), (3, 4.0), (4, 30.0)];
        assert_eq!(nearest_chain((1, 0.0), &units, 2), vec![1, 3]);
        assert_eq!(nearest_chain((1, 0.0), &units, 10), vec![1, 3, 2]);
    }

    #[test]
    fn a_chain_starts_at_its_target_even_when_the_area_rule_skips_it() {
        assert_eq!(
            nearest_chain((7, 0.0), &[(2, 9.0), (3, 4.0)], 3),
            vec![7, 3, 2]
        );
    }

    #[test]
    fn a_chain_without_a_jump_count_hits_its_target_alone() {
        assert_eq!(super::chain_length(0), 1);
        assert_eq!(super::chain_length(1), 1);
        assert_eq!(super::chain_length(3), 3);
        assert_eq!(
            nearest_chain((1, 0.0), &[(2, 1.0)], super::chain_length(0)),
            vec![1]
        );
    }

    #[test]
    fn a_healing_chain_jumps_to_the_last_units_group_before_a_more_injured_outsider() {
        let target = injured(1, 0.0, Some(9), 100);
        let mate = injured(2, 8.0, Some(9), 10);
        let outsider = injured(3, 2.0, None, 5_000);
        assert_eq!(healing_chain(target, vec![outsider, mate], 2), vec![1, 2]);
    }

    #[test]
    fn a_healing_chain_jumps_to_the_most_injured_unit_not_the_nearest() {
        let target = injured(1, 0.0, None, 100);
        let scratched = injured(2, 1.0, None, 10);
        let wounded = injured(3, 9.0, None, 900);
        assert_eq!(
            healing_chain(target, vec![scratched, wounded], 3),
            vec![1, 3, 2]
        );
    }

    #[test]
    fn a_healing_chain_ranks_each_jump_from_the_unit_it_last_healed() {
        let target = injured(1, 0.0, None, 100);
        let first_mate = injured(2, 5.0, Some(4), 950);
        let outsider = injured(3, 9.0, None, 900);
        let second_mate = injured(5, 12.0, Some(4), 10);
        assert_eq!(
            healing_chain(target, vec![first_mate, outsider, second_mate], 3),
            vec![1, 2, 5],
            "the second jump prefers the first mate's Group over the more injured outsider"
        );
    }

    #[test]
    fn a_healing_chain_skips_a_unit_it_may_not_jump_to() {
        let target = injured(1, 0.0, None, 100);
        let hidden = injured(2, 3.0, None, 900);
        let beyond_every_link = injured(3, 17.0, None, 900);
        let seen = injured(4, 6.0, None, 10);
        let chain = super::chain_jumps(
            target,
            vec![hidden, beyond_every_link, seen],
            3,
            |_, next| next.guid != hidden.guid,
            super::chain_healing_rank,
        );
        assert_eq!(chain, vec![1, 4]);
    }

    fn enemy_chain(
        units: &[(u64, f32, bool)],
        only_players: bool,
        wall_between: (u64, u64),
    ) -> Vec<u64> {
        let unit = |&(guid, x, player): &(u64, f32, bool)| super::ChainUnit {
            player,
            ..super::ChainUnit::at(guid, (x, 0.0, 0.0))
        };
        super::chain_jumps(
            unit(&units[0]),
            units[1..].iter().map(unit).collect(),
            units.len(),
            |from, to| {
                super::chain_may_jump(to, only_players, || (from.guid, to.guid) != wall_between)
            },
            super::chain_distance_sq,
        )
    }

    #[test]
    fn an_enemy_chain_skips_a_unit_the_last_link_cannot_see() {
        let units = [(1, 0.0, false), (2, 3.0, false), (3, 6.0, false)];
        assert_eq!(enemy_chain(&units, false, (1, 2)), vec![1, 3, 2]);
        assert_eq!(enemy_chain(&units, false, (9, 9)), vec![1, 2, 3]);
    }

    #[test]
    fn an_enemy_chain_that_only_hits_players_skips_a_creature() {
        let units = [(1, 0.0, false), (2, 3.0, false), (3, 6.0, true)];
        assert_eq!(enemy_chain(&units, true, (9, 9)), vec![1, 3]);
        assert_eq!(enemy_chain(&units, false, (9, 9)), vec![1, 2, 3]);
    }

    #[test]
    fn a_melee_enemy_chain_gathers_within_its_reach_and_any_other_chain_ten_yards_more_a_jump() {
        let chain = super::SpellEffect {
            target: super::T_CHAIN_ENEMY,
            radius_yd: 5.0,
            chain_targets: 3,
            ..super::SpellEffect::blank(1, 0)
        };
        let melee = super::Spell {
            damage_class: crate::spell::proc::DAMAGE_CLASS_MELEE,
            ..super::Spell::blank(1, "Cleave")
        };
        assert_eq!(super::enemy_chain_reach(&chain, &melee), 5.0);
        let ranged_melee = super::Spell {
            range_yd: 5,
            ..melee
        };
        let no_radius = super::SpellEffect {
            radius_yd: 0.0,
            ..chain
        };
        assert_eq!(super::enemy_chain_reach(&no_radius, &ranged_melee), 5.0);
        assert_eq!(
            super::enemy_chain_reach(&chain, &super::Spell::blank(2, "Chain")),
            35.0
        );
    }

    #[test]
    fn an_area_within_its_cap_keeps_every_unit_in_order() {
        let hits = vec![9, 3, 5];
        assert_eq!(super::keep_affected_targets(hits.clone(), 3, 5, 1), hits);
    }

    #[test]
    fn only_a_chain_or_a_spell_with_max_affected_targets_has_a_target_cap() {
        let area = super::SpellEffect {
            target: super::T_AREA_ENEMY,
            ..super::SpellEffect::blank(1, 0)
        };
        let chain = super::SpellEffect {
            target: super::T_CHAIN_ENEMY,
            chain_targets: 3,
            ..super::SpellEffect::blank(1, 0)
        };
        assert_eq!(super::affected_target_cap(0, &area), None);
        assert_eq!(super::affected_target_cap(5, &area), Some(5));
        assert_eq!(super::affected_target_cap(5, &chain), Some(3));
    }

    #[test]
    fn aura_slot_is_per_spell_so_a_multi_effect_buff_is_one_icon() {
        assert_eq!(super::pick_aura_slot(&[], 687, false, 32, 48), Some(0));
        assert_eq!(
            super::pick_aura_slot(&[(0, 687)], 687, false, 32, 48),
            Some(0)
        );
        assert_eq!(
            super::pick_aura_slot(&[(0, 687)], 25, false, 32, 48),
            Some(1)
        );
        assert_eq!(
            super::pick_aura_slot(&[(0, 10), (2, 11)], 12, false, 32, 48),
            Some(1)
        );
        let full: Vec<(u8, u32)> = (0u8..3).map(|s| (s, s as u32 + 100)).collect();
        assert_eq!(super::pick_aura_slot(&full, 999, false, 3, 4), None);
        assert_eq!(super::pick_aura_slot(&full, 101, false, 3, 4), Some(1));
    }

    #[test]
    fn a_hidden_holder_does_not_hide_a_visible_holder_of_the_same_spell() {
        assert_eq!(
            super::pick_aura_slot(&[(255, 20598)], 20598, false, 32, 48),
            Some(0)
        );
    }

    #[test]
    fn debuffs_take_the_upper_range_of_aura_slots_buffs_the_lower_range() {
        assert_eq!(super::pick_aura_slot(&[], 6673, false, 32, 48), Some(0));
        assert_eq!(super::pick_aura_slot(&[], 20511, true, 32, 48), Some(32));
        assert_eq!(
            super::pick_aura_slot(&[(32, 20511)], 6673, false, 32, 48),
            Some(0)
        );
        assert_eq!(
            super::pick_aura_slot(&[(0, 6673), (32, 20511)], 12345, true, 32, 48),
            Some(33)
        );
        let buffs_full: Vec<(u8, u32)> = (0u8..32).map(|s| (s, s as u32 + 1)).collect();
        assert_eq!(super::pick_aura_slot(&buffs_full, 999, false, 32, 48), None);
        assert_eq!(
            super::pick_aura_slot(&buffs_full, 999, true, 32, 48),
            Some(32)
        );
        assert_eq!(
            super::pick_aura_slot(&[(32, 20511)], 20511, true, 32, 48),
            Some(32)
        );
    }

    #[test]
    fn sixteen_debuffs_full_refuses_the_seventeenth_leaving_the_sixteen_untouched() {
        let debuffs_full: Vec<(u8, u32)> =
            (32u8..48).map(|slot| (slot, slot as u32 + 1000)).collect();
        assert_eq!(debuffs_full.len(), 16);
        assert_eq!(
            super::pick_aura_slot(&debuffs_full, 9999, true, 32, 48),
            None
        );
        for &(slot, spell_id) in &debuffs_full {
            assert_eq!(
                super::pick_aura_slot(&debuffs_full, spell_id, true, 32, 48),
                Some(slot)
            );
        }
    }

    #[test]
    fn thirty_two_buffs_full_refuses_the_thirty_third_leaving_the_thirty_two_untouched() {
        let buffs_full: Vec<(u8, u32)> = (0u8..32).map(|slot| (slot, slot as u32 + 2000)).collect();
        assert_eq!(buffs_full.len(), 32);
        assert_eq!(
            super::pick_aura_slot(&buffs_full, 8888, false, 32, 48),
            None
        );
        for &(slot, spell_id) in &buffs_full {
            assert_eq!(
                super::pick_aura_slot(&buffs_full, spell_id, false, 32, 48),
                Some(slot)
            );
        }
    }

    #[test]
    fn buff_and_debuff_ranges_are_independent_when_full() {
        let buffs_full: Vec<(u8, u32)> = (0u8..32).map(|slot| (slot, slot as u32 + 2000)).collect();
        let debuffs_full: Vec<(u8, u32)> =
            (32u8..48).map(|slot| (slot, slot as u32 + 1000)).collect();
        assert_eq!(
            super::pick_aura_slot(&buffs_full, 9999, true, 32, 48),
            Some(32)
        );
        assert_eq!(
            super::pick_aura_slot(&debuffs_full, 8888, false, 32, 48),
            Some(0)
        );

        let both_full: Vec<(u8, u32)> = buffs_full
            .iter()
            .chain(debuffs_full.iter())
            .copied()
            .collect();
        assert_eq!(super::pick_aura_slot(&both_full, 9999, true, 32, 48), None);
        assert_eq!(super::pick_aura_slot(&both_full, 8888, false, 32, 48), None);
    }

    #[test]
    fn is_profession_line_covers_the_13_and_excludes_combat_lines() {
        use crate::skill::{is_profession_line, skill_line::*};
        for line in [
            COOKING,
            FIRST_AID,
            TAILORING,
            LEATHERWORKING,
            BLACKSMITHING,
            ENGINEERING,
            ALCHEMY,
            ENCHANTING,
            MINING,
            HERBALISM,
            SKINNING,
            FISHING,
        ] {
            assert!(
                is_profession_line(line),
                "profession line {line} must count"
            );
        }
        for line in [SWORD_1H, AXE_2H, DEFENSE, 237u32, 0] {
            assert!(
                !is_profession_line(line),
                "non-profession line {line} must NOT count"
            );
        }
    }

    #[test]
    fn faction_gate_blocks_only_the_two_exploit_cases() {
        assert_eq!(
            super::faction_target_violation(true, false, true, false),
            Some("enemies")
        );
        assert_eq!(
            super::faction_target_violation(false, true, false, true),
            Some("allies")
        );
        assert_eq!(
            super::faction_target_violation(true, false, false, true),
            None
        );
        assert_eq!(
            super::faction_target_violation(false, true, true, false),
            None
        );
        assert_eq!(
            super::faction_target_violation(true, false, false, false),
            None
        );
        assert_eq!(
            super::faction_target_violation(false, true, false, false),
            None
        );
        assert_eq!(
            super::faction_target_violation(false, false, true, true),
            None
        );
    }
}
