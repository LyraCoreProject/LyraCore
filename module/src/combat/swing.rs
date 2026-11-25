use spacetimedb::{reducer, table, ReducerContext, Table, TimeDuration};

use crate::spell::SpellCastEventKind;
use crate::{game_item_template, game_spell, game_world_entity, SpellCastEvent, WorldEntity};

use super::reach::{in_melee_reach, MeleeBody};
use super::*;
use crate::{
    game_melee_attack, game_ranged_impact_schedule, CombatEvent, MeleeAttack, MeleeSchedule,
    RangedImpactSchedule,
};

#[reducer]
pub fn tick_melee(ctx: &ReducerContext, _schedule: MeleeSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    super::regeneration::regenerate_characters(ctx);
    melee_pass(ctx);
}

fn melee_pass(ctx: &ReducerContext) {
    super::inactive_grid::evade_inactive_grids(ctx);
    leash_pass(ctx);
    aggro_pass(ctx);
    resolve_swing(ctx);
}

fn leash_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let now_ms = crate::time::now_ms_u32(ctx);

    let mut evaders: Vec<u64> = Vec::new();
    let mut seeds: Vec<(u64, f32, f32)> = Vec::new();
    for atk in melee.iter() {
        let (Some(attacker), Some(target)) = (
            entities.guid().find(atk.attacker_guid),
            entities.guid().find(atk.target_guid),
        ) else {
            continue;
        };
        if attacker.is_player() || evaders.contains(&attacker.guid) {
            continue;
        }
        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        if crate::combat::beyond_combat_backstop(dx * dx + dy * dy + dz * dz) {
            evaders.push(attacker.guid);
            continue;
        }
        if atk.pursuit_ends_ms == 0 {
            seeds.push((atk.attacker_guid, attacker.x, attacker.y));
            continue;
        }
        let (rx, ry) = (target.x - atk.leash_x, target.y - atk.leash_y);
        if crate::combat::should_evade(now_ms, atk.pursuit_ends_ms, rx * rx + ry * ry) {
            evaders.push(atk.attacker_guid);
        }
    }
    for (creature, x, y) in seeds {
        if let Some(mut row) = melee.attacker_guid().find(creature) {
            row.pursuit_ends_ms = crate::combat::pursuit_deadline_ms(now_ms);
            row.leash_x = x;
            row.leash_y = y;
            crate::MeleeAttack::update(ctx, row);
        }
    }
    for creature in evaders {
        evade(ctx, creature);
    }
}

pub(crate) fn evade(ctx: &ReducerContext, creature: u64) {
    stop_casting_for_evade(ctx, creature);
    let at_home = crate::creatures::eventai_on_evade(ctx, creature);
    disengage(ctx, creature);
    if let Some(mut c) = ctx.db.game_world_entity().guid().find(creature) {
        if c.health != c.max_health {
            c.health = c.max_health;
            c.update(ctx);
        }
    }
    if at_home {
        crate::creatures::eventai_arrived_home(ctx, creature);
    }
}

/// vmangos `CombatStop(true)` interrupts the evading creature's own cast and channel.
pub(crate) fn stop_casting_for_evade(ctx: &ReducerContext, creature: u64) {
    crate::spell::break_channel(ctx, creature);
}

fn aggro_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let now_ms = crate::time::now_ms_u32(ctx);

    let mut new_aggro: Vec<(u64, u64)> = Vec::new();
    for atk in melee.iter() {
        let (Some(attacker), Some(target)) = (
            entities.guid().find(atk.attacker_guid),
            entities.guid().find(atk.target_guid),
        ) else {
            continue;
        };
        // Retaliation requires a fired attack; entering attack stance alone is not a hit.
        let is_ranged = atk.ranged_spell_id != 0;
        if attacker.is_player()
            && !target.is_player()
            && !target.dead
            && ((is_ranged
                && target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0)
                || (atk.last_swing_ms != 0 && within_moving_reach(ctx, &attacker, &target, now_ms)))
            && melee.attacker_guid().find(target.guid).is_none()
            && !new_aggro.iter().any(|(c, _)| *c == target.guid)
        {
            new_aggro.push((target.guid, attacker.guid));
        }
    }
    for (creature, player) in new_aggro {
        engage_passive_creature(ctx, creature, player);
        arm_creature_engagement(ctx, creature, player, false);
    }
}

// Retaliation always allows the leeway, so a fired swing still counts after either unit stops.
fn within_moving_reach(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    now_ms: u32,
) -> bool {
    let moving = |unit| MeleeBody {
        moving: true,
        ..MeleeBody::of(ctx, unit, now_ms)
    };
    in_melee_reach(&moving(attacker), &moving(target))
}

fn resolve_offhand_swing(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    attacker: &WorldEntity,
    now_ms: u32,
) {
    let Some((dmin, dmax, delay)) = equipped_offhand_weapon_damage(ctx, attacker_guid) else {
        return;
    };
    let melee = ctx.db.game_melee_attack();
    let Some(atk) = melee.attacker_guid().find(attacker_guid) else {
        return;
    };
    if atk.last_offhand_swing_ms != 0 && now_ms.wrapping_sub(atk.last_offhand_swing_ms) < delay {
        return;
    }
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return;
    };
    if attacker.dead || target.dead {
        return;
    }
    if crate::spell::is_disarmed(ctx, attacker_guid) {
        return;
    }
    crate::pvp::harmed(ctx, attacker, &target);

    let range = offhand_swing_range(ctx, attacker, (dmin, dmax, delay));
    crate::conformance_rolls::observe_offhand_range(ctx, range);
    let (rolled, hit_info, blocked) = roll_swing_with_range(ctx, attacker, &target, range, None);

    crate::spell::break_stealth(ctx, attacker_guid);

    let damage = resolve_damage(ctx, attacker_guid, target_guid, rolled);
    let killing_blow = is_killing_blow(ctx, attacker_guid, &target, damage.amount());

    CombatEvent::insert(
        ctx,
        CombatEvent {
            damage: damage.amount(),
            hit_info: hit_info.wire(),
            killing_blow,
            blocked_amount: blocked,
            ..CombatEvent::signal_at(ctx, attacker, target_guid)
        },
    );

    let dealt = damage.amount();
    let hit = Hit::weapon(HitSource::OffHand, hit_info == AttackOutcome::Crit).with_block(blocked);
    let combat_ended = damage
        .land(ctx, attacker_guid, target_guid, hit)
        .combat_ended();
    crate::spell::proc::run_unlanded_proc_pass(
        ctx,
        attacker_guid,
        target_guid,
        &hit,
        hit_info,
        (dealt, damage.absorbed()),
    );
    if combat_ended {
        if affects_victim(hit_info, dealt, blocked) {
            super::damage_shield::reflect(ctx, attacker_guid, target_guid);
        }
        return;
    }
    if affects_victim(hit_info, dealt, blocked) {
        crate::spell::chance_on_hit::roll_chance_on_hit(
            ctx,
            attacker_guid,
            target_guid,
            crate::items::equip_slot::OFFHAND,
        );
        super::damage_shield::reflect(ctx, attacker_guid, target_guid);
    }

    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_offhand_swing_ms = now_ms;
        crate::MeleeAttack::update(ctx, fresh);
    }
}

pub(crate) fn offhand_swing_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    (dmin, dmax, delay): (u32, u32, u32),
) -> (u32, u32) {
    let ap = melee_attack_power(ctx, attacker);
    let (raw_min, raw_max) = weapon_swing_range_ap(ap, dmin, dmax, delay);
    let apply = |base| {
        crate::spell::bonuses::weapon_damage(
            ctx,
            attacker,
            crate::items::equip_slot::OFFHAND,
            base as f32,
        ) as u32
    };
    apply_offhand_penalty(apply(raw_min), apply(raw_max))
}

fn resolve_swing(ctx: &ReducerContext) {
    let now_ms = crate::time::now_ms_u32(ctx);
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();

    let attacks: Vec<MeleeAttack> = melee.iter().collect();
    for mut atk in attacks {
        let Some(attacker) = entities.guid().find(atk.attacker_guid) else {
            crate::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        };
        let Some(mut target) = entities.guid().find(atk.target_guid) else {
            crate::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        };

        if attacker.dead || target.dead {
            crate::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        }
        if melee.attacker_guid().find(atk.attacker_guid).is_none() {
            continue;
        }
        if !crate::combat::may_harm(ctx, &attacker, &target) {
            crate::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        }

        if !attacker.is_player() && !crate::creatures::melee_enabled(ctx, attacker.guid) {
            continue;
        }
        if crate::spell::is_action_blocked(ctx, atk.attacker_guid) {
            continue;
        }

        if !attacker.is_player() && crate::creatures::tick::creature_is_routing(ctx, &attacker) {
            continue;
        }

        let ranged = if atk.ranged_spell_id != 0 {
            match equipped_ranged_weapon(ctx, atk.attacker_guid) {
                Some(w) => Some(w),
                None => {
                    crate::MeleeAttack::end_auto_repeat(ctx, atk.attacker_guid);
                    continue;
                }
            }
        } else {
            None
        };

        let ranged_due =
            ranged.is_some_and(|(_, _, delay, _)| now_ms.wrapping_sub(atk.last_swing_ms) >= delay);

        if let Some((_, _, delay, subclass)) = ranged {
            use crate::items::weapon_subclass as ws;
            if attacker.is_player() && is_translating(attacker.movement_flags) {
                if subclass == ws::WAND {
                    crate::MeleeAttack::end_auto_repeat(ctx, atk.attacker_guid);
                } else if ranged_due {
                    if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
                        fresh.last_swing_ms = now_ms
                            .wrapping_add(RANGED_INITIAL_SHOT_MS)
                            .wrapping_sub(delay)
                            .max(1);
                        crate::MeleeAttack::update(ctx, fresh);
                    }
                }
                continue;
            }
        }

        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if let Some(block) =
            swing_blocked(ctx, &attacker, &target, ranged.is_some(), dist_sq, now_ms)
        {
            if ranged_due {
                crate::MeleeAttack::end_auto_repeat(ctx, atk.attacker_guid);
            } else if ranged.is_none() && attacker.is_player() {
                record_swing_error(ctx, &atk, &attacker, block.swing_error(), now_ms);
            }
            continue;
        }

        if ranged.is_none() && has_extra_attacks(ctx, atk.attacker_guid) {
            if !spend_extra_attacks(ctx, &attacker, &target, now_ms) {
                continue;
            }
            atk.last_swing_ms = now_ms;
        }

        if ranged.is_none() {
            resolve_offhand_swing(ctx, atk.attacker_guid, atk.target_guid, &attacker, now_ms);
            if melee.attacker_guid().find(atk.attacker_guid).is_none() {
                continue;
            }

            match entities.guid().find(atk.target_guid) {
                Some(refreshed) => target = refreshed,
                None => {
                    crate::MeleeAttack::delete(ctx, atk.attacker_guid);
                    continue;
                }
            }
            if target.dead {
                continue;
            }
        }

        let swing_interval = match ranged {
            Some((_, _, delay, _)) => delay,
            None => effective_swing_time(ctx, &attacker),
        };
        if atk.last_swing_ms != 0 && now_ms.wrapping_sub(atk.last_swing_ms) < swing_interval {
            continue;
        }

        match ranged {
            Some(weapon) => fire_ranged_shot(
                ctx,
                &attacker,
                &target,
                weapon,
                atk.ranged_spell_id,
                dist_sq,
                now_ms,
            ),
            None => fire_melee_swing(ctx, &attacker, &target, now_ms, false),
        }
    }
}

/// Main-hand swings granted by an extra-attack effect; the next melee tick spends them. They belong
/// to the melee auto-attack: stopping it drops them.
#[table(accessor = game_extra_attack)]
pub struct ExtraAttack {
    #[primary_key]
    pub attacker_guid: u64,
    pub count: u8,
}

crate::outside_table_catalog!(game_extra_attack);

// vmangos SPELL_EFFECT_ADD_EXTRA_ATTACKS: a unit that still holds Extra Attacks gains none.
pub(crate) fn grant_extra_attacks(ctx: &ReducerContext, guid: u64, count: i32) {
    let alive = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|unit| !unit.dead);
    let meleeing = ctx
        .db
        .game_melee_attack()
        .attacker_guid()
        .find(guid)
        .is_some_and(|atk| atk.ranged_spell_id == 0);
    if !alive || !meleeing || count <= 0 || has_extra_attacks(ctx, guid) {
        return;
    }
    ctx.db.game_extra_attack().insert(ExtraAttack {
        attacker_guid: guid,
        count: count.min(i32::from(u8::MAX)) as u8,
    });
}

pub(crate) fn has_extra_attacks(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_extra_attack()
        .attacker_guid()
        .find(guid)
        .is_some()
}

pub(crate) fn drop_extra_attacks(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_extra_attack().attacker_guid().delete(guid);
}

// Spends each Extra Attack after its swing, so a proc mid-swing grants none (vmangos).
// Returns false when the engagement ended.
fn spend_extra_attacks(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    now_ms: u32,
) -> bool {
    let held = ctx.db.game_extra_attack();
    let entities = ctx.db.game_world_entity();
    let meleeing = || {
        ctx.db
            .game_melee_attack()
            .attacker_guid()
            .find(attacker.guid)
            .is_some()
    };
    loop {
        if !has_extra_attacks(ctx, attacker.guid) {
            return meleeing();
        }
        let (Some(attacker), Some(target)) = (
            entities.guid().find(attacker.guid),
            entities.guid().find(target.guid),
        ) else {
            drop_extra_attacks(ctx, attacker.guid);
            return false;
        };
        if attacker.dead || target.dead {
            drop_extra_attacks(ctx, attacker.guid);
            continue;
        }
        fire_melee_swing(ctx, &attacker, &target, now_ms, true);
        let Some(mut extra) = held.attacker_guid().find(attacker.guid) else {
            return meleeing();
        };
        extra.count = extra.count.saturating_sub(1);
        if extra.count == 0 {
            drop_extra_attacks(ctx, attacker.guid);
        } else {
            held.attacker_guid().update(extra);
        }
    }
}

fn swing_blocked(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    is_ranged: bool,
    dist_sq: f32,
    now_ms: u32,
) -> Option<SwingBlock> {
    if is_ranged {
        if dist_sq > RANGED_RANGE_SQ {
            return Some(SwingBlock::OutOfRange);
        }
        if dist_sq < MELEE_RANGE_SQ {
            return Some(SwingBlock::TooClose);
        }
    } else if !in_melee_reach(
        &MeleeBody::of(ctx, attacker, now_ms),
        &MeleeBody::of(ctx, target, now_ms),
    ) {
        return Some(SwingBlock::OutOfRange);
    }
    if !crate::nav::has_los(
        ctx,
        attacker.map_id,
        attacker.instance_id,
        (attacker.x, attacker.y, attacker.z),
        (target.x, target.y, target.z),
    ) {
        return Some(SwingBlock::NoLineOfSight);
    }
    if attacker.is_player()
        && !crate::geometry::is_facing(
            attacker.x,
            attacker.y,
            attacker.orientation,
            target.x,
            target.y,
        )
    {
        return Some(SwingBlock::BadFacing);
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SwingBlock {
    OutOfRange,
    TooClose,
    NoLineOfSight,
    BadFacing,
}

impl SwingBlock {
    /// 1.12 has no swing message for line of sight (vmangos `Unit::CanAutoAttackTarget`).
    fn swing_error(self) -> u8 {
        match self {
            Self::OutOfRange => SWING_ERROR_NOT_IN_RANGE,
            Self::BadFacing => SWING_ERROR_BAD_FACING,
            Self::TooClose | Self::NoLineOfSight => SWING_ERROR_NONE,
        }
    }
}

/// Writes only on a change, and only when a hand is due: vmangos reports a ready attack only.
fn record_swing_error(
    ctx: &ReducerContext,
    atk: &MeleeAttack,
    attacker: &WorldEntity,
    swing_error: u8,
    now_ms: u32,
) {
    let hand_due = |last_ms: u32, interval_ms: u32| {
        last_ms == 0 || now_ms.wrapping_sub(last_ms) >= interval_ms
    };
    let due = hand_due(atk.last_swing_ms, effective_swing_time(ctx, attacker))
        || equipped_offhand_weapon_damage(ctx, atk.attacker_guid)
            .is_some_and(|(_, _, delay)| hand_due(atk.last_offhand_swing_ms, delay));
    if !due || atk.swing_error == swing_error {
        return;
    }
    let melee = ctx.db.game_melee_attack();
    if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
        fresh.swing_error = swing_error;
        crate::MeleeAttack::update(ctx, fresh);
    }
}

fn arms_revenge(hit_info: AttackOutcome, blocked: u32) -> bool {
    let avoided = match hit_info {
        AttackOutcome::Dodge | AttackOutcome::Parry | AttackOutcome::Block => true,
        AttackOutcome::Normal
        | AttackOutcome::Crit
        | AttackOutcome::Miss
        | AttackOutcome::Glancing
        | AttackOutcome::Crushing
        | AttackOutcome::Immune
        | AttackOutcome::Evade => false,
    };
    avoided || blocked > 0
}

// vmangos AttackerStateUpdate; an extra attack never fires the queued strike.
fn fire_melee_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    now_ms: u32,
    extra: bool,
) {
    let attacker_guid = attacker.guid;
    let target_guid = target.guid;
    let melee = ctx.db.game_melee_attack();
    // vmangos UpdateMeleeAttackingState flags a Character's swing, landed or not; DealDamage flags
    // a pet's owner.
    crate::pvp::harmed(ctx, attacker, target);

    let queued_spell = if extra { 0 } else { attacker.next_swing_spell };
    let queued_header = ctx.db.game_spell().spell_id().find(queued_spell);
    let swing_is_spell = queued_spell != 0
        && super::queued_strike::charge(ctx, attacker, queued_spell, queued_header.as_ref());
    let strike = swing_is_spell.then(|| super::queued_strike::QueuedStrike {
        damage: queued_header
            .as_ref()
            .map_or_else(Default::default, |strike| {
                super::queued_strike::damage(ctx, attacker, strike)
            }),
        spell: DeliveredSpell::queued_strike(queued_header.as_ref()),
    });

    let (mut rolled, hit_info, blocked) = roll_swing(ctx, attacker, target, strike);

    if hit_info == AttackOutcome::Dodge {
        arm_react(ctx, attacker_guid, ReactKind::Overpower);
    }

    let (mut seal_portion, mut seal_spell) = (0u32, 0u32);
    if rolled > 0 {
        let (p, s) = seal_holy_on_swing(ctx, attacker, target_guid);
        seal_portion = p;
        seal_spell = s;
        rolled += seal_portion;
    }

    if !extra {
        super::queued_strike::clear(ctx, attacker_guid);
    }

    crate::spell::break_stealth(ctx, attacker_guid);

    let damage = resolve_damage(ctx, attacker_guid, target_guid, rolled);
    let (projected_swing, projected_seal) = split_damage_projection(damage.amount(), seal_portion);

    // vmangos reports a queued strike before the damage lands, so a killing blow follows it.
    if swing_is_spell {
        let cooldown_ms = queued_header.as_ref().map_or(0, |h| h.cooldown_ms);
        crate::spell::start_spell_cooldown(ctx, attacker_guid, queued_spell, cooldown_ms);
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid,
                is_completion: true,
                damage: projected_swing,
                // A Seal of Righteousness portion shares the absorb, so it is not reported.
                absorbed: if seal_portion == 0 {
                    damage.absorbed()
                } else {
                    0
                },
                is_crit: hit_info == AttackOutcome::Crit,
                swing_hit_info: hit_info.wire(),
                cooldown_ms,
                blocked_amount: blocked,
                strike_swing_report: super::queued_strike::swing_report(queued_header.as_ref())
                    as u8,
                ..SpellCastEvent::signal_at(ctx, attacker, queued_spell, SpellCastEventKind::Go)
            },
        );
    }
    CombatEvent::insert(
        ctx,
        CombatEvent {
            damage: projected_swing,
            hit_info: hit_info.wire(),
            killing_blow: is_killing_blow(ctx, attacker_guid, target, damage.amount()),
            blocked_amount: blocked,
            spell_swing: swing_is_spell,
            ..CombatEvent::signal_at(ctx, attacker, target_guid)
        },
    );
    if projected_seal > 0 {
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid,
                damage: projected_seal,
                school: 1,
                is_proc_log: true,
                ..SpellCastEvent::signal_at(ctx, attacker, seal_spell, SpellCastEventKind::ProcLog)
            },
        );
    }

    let dealt = damage.amount();
    let hit = if swing_is_spell {
        Hit::spell(
            HitSource::MainHand,
            queued_spell,
            hit_info == AttackOutcome::Crit,
        )
    } else {
        Hit::weapon(HitSource::MainHand, hit_info == AttackOutcome::Crit)
    }
    .with_block(blocked);
    // vmangos adds a queued strike's table threat unless it was avoided or refused as immune.
    if swing_is_spell && !hit_info.is_avoided() {
        crate::threat::add_table_threat(
            ctx,
            attacker_guid,
            queued_spell,
            &[(target_guid, crate::threat::EVERY_EFFECT)],
        );
    }
    let combat_ended = damage
        .land(ctx, attacker_guid, target_guid, hit)
        .combat_ended();
    crate::spell::proc::run_unlanded_proc_pass(
        ctx,
        attacker_guid,
        target_guid,
        &hit,
        hit_info,
        (dealt, damage.absorbed()),
    );
    let reflects = if swing_is_spell {
        dealt > 0
            && queued_header
                .as_ref()
                .is_some_and(|spell| spell.cast_flags & crate::spell::CAST_FLAG_WEAPON_PROCS != 0)
    } else {
        affects_victim(hit_info, dealt, blocked)
    };
    if swing_is_spell {
        let school_mask = queued_header
            .as_ref()
            .map_or(1, |spell| u32::from(spell.school_mask));
        crate::creatures::eventai_on_spell_hit(
            ctx,
            attacker_guid,
            target_guid,
            queued_spell,
            school_mask,
        );
    }
    if combat_ended {
        if reflects {
            super::damage_shield::reflect(ctx, attacker_guid, target_guid);
        }
        return;
    }
    // vmangos: a weapon-proc strike rolls even when avoided; a white swing must affect the victim.
    let rolls_weapon = if swing_is_spell {
        queued_header
            .as_ref()
            .is_some_and(|strike| strike.cast_flags & crate::spell::CAST_FLAG_WEAPON_PROCS != 0)
    } else {
        affects_victim(hit_info, dealt, blocked)
    };
    if rolls_weapon {
        crate::spell::chance_on_hit::roll_chance_on_hit(
            ctx,
            attacker_guid,
            target_guid,
            crate::items::equip_slot::MAINHAND,
        );
    }
    if reflects {
        super::damage_shield::reflect(ctx, attacker_guid, target_guid);
    }

    // Arm Revenge after damage so a stale defender snapshot cannot erase its deadline.
    if arms_revenge(hit_info, blocked) {
        arm_react(ctx, target_guid, ReactKind::Revenge);
    }

    // Re-read before stamping; the off-hand may have updated its clock on this row.
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_swing_ms = now_ms;
        fresh.swing_error = SWING_ERROR_NONE;
        crate::MeleeAttack::update(ctx, fresh);
    }

    // Enter combat after entity writes so a stale snapshot cannot erase the flag.
    enter_combat(ctx, attacker_guid);
    enter_combat(ctx, target_guid);
}

// vmangos HITINFO_AFFECTS_VICTIM: the swing connected and either dealt damage or was blocked.
fn affects_victim(hit_info: AttackOutcome, dealt: u32, blocked: u32) -> bool {
    !hit_info.is_avoided() && (dealt > 0 || blocked > 0)
}

fn split_damage_projection(final_damage: u32, seal_portion: u32) -> (u32, u32) {
    let seal = seal_portion.min(final_damage);
    (final_damage - seal, seal)
}

fn fire_ranged_shot(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    weapon: (u32, u32, u32, u8),
    ranged_spell_id: u32,
    dist_sq: f32,
    now_ms: u32,
) {
    use crate::items::weapon_subclass as ws;
    let (dmin, dmax, _delay, subclass) = weapon;
    let attacker_guid = attacker.guid;
    let target_guid = target.guid;
    let melee = ctx.db.game_melee_attack();
    let launcher = matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW);

    let ammo_display_id: u32 = if launcher {
        match find_ammo(ctx, attacker_guid) {
            Some(a) => {
                let disp = ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(a.entry)
                    .map(|t| t.display_id)
                    .unwrap_or(0);
                super::folds::spend_one(ctx, a);
                disp
            }
            None => {
                crate::MeleeAttack::end_auto_repeat(ctx, attacker_guid);
                return;
            }
        }
    } else {
        0
    };
    crate::pvp::harmed(ctx, attacker, target);

    let (rdmin, rdmax) = if attacker.is_player() {
        let (_, rdmin, rdmax) = ranged_values(ctx, attacker, Some(weapon));
        (
            crate::spell::bonuses::weapon_damage(
                ctx,
                attacker,
                crate::items::equip_slot::RANGED,
                rdmin,
            ) as u32,
            crate::spell::bonuses::weapon_damage(
                ctx,
                attacker,
                crate::items::equip_slot::RANGED,
                rdmax,
            ) as u32,
        )
    } else {
        (dmin, dmax)
    };
    let roll = roll_ranged_swing(ctx, attacker, target, ranged_spell_id, (rdmin, rdmax));
    let hit_info = roll.outcome;

    crate::spell::break_stealth(ctx, attacker_guid);

    let damage = FrozenDamage::launch(ctx, attacker_guid, target_guid, roll.damage);

    let speed = if subclass == ws::WAND {
        WAND_PROJECTILE_SPEED
    } else {
        AUTO_SHOT_PROJECTILE_SPEED
    };
    let travel_ms = crate::geometry::projectile_travel_ms(dist_sq.sqrt(), speed);
    CombatEvent::insert(
        ctx,
        CombatEvent {
            damage: 0,
            hit_info: hit_info.wire(),
            ranged_spell_id,
            ammo_display_id,
            impact_delay_ms: travel_ms,
            ..CombatEvent::signal_at(ctx, attacker, target_guid)
        },
    );
    if hit_info.is_avoided() {
        // A shot that cannot land has no impact to carry its Miss or Immune procs.
        crate::spell::proc::run_unlanded_proc_pass(
            ctx,
            attacker_guid,
            target_guid,
            &Hit::weapon(HitSource::Ranged, false),
            hit_info,
            (0, 0),
        );
    }
    schedule_ranged_impact(
        ctx,
        attacker_guid,
        target_guid,
        RangedShot {
            damage,
            ranged_spell_id,
            is_crit: hit_info == AttackOutcome::Crit,
            resisted: roll.resisted,
            blocked: roll.blocked,
        },
        travel_ms,
    );
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_swing_ms = now_ms;
        crate::MeleeAttack::update(ctx, fresh);
    }
}

pub(crate) fn launch_ranged_strike(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    rolled: u32,
    spell: &crate::Spell,
    is_crit: bool,
) {
    let (rolled, blocked) = super::folds::block_ranged_strike(ctx, attacker, target, spell, rolled);
    let damage = FrozenDamage::launch(ctx, attacker.guid, target.guid, rolled);
    let (dx, dy, dz) = (
        target.x - attacker.x,
        target.y - attacker.y,
        target.z - attacker.z,
    );
    let travel_ms = crate::geometry::projectile_travel_ms(
        (dx * dx + dy * dy + dz * dz).sqrt(),
        AUTO_SHOT_PROJECTILE_SPEED,
    );
    schedule_ranged_impact(
        ctx,
        attacker.guid,
        target.guid,
        RangedShot {
            damage,
            ranged_spell_id: spell.spell_id,
            is_crit,
            resisted: 0,
            blocked,
        },
        travel_ms,
    );
}

struct RangedShot {
    damage: FrozenDamage,
    ranged_spell_id: u32,
    is_crit: bool,
    resisted: u32,
    blocked: u32,
}

fn schedule_ranged_impact(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    shot: RangedShot,
    travel_ms: u32,
) {
    let RangedShot {
        damage,
        ranged_spell_id,
        is_crit,
        resisted,
        blocked,
    } = shot;
    if damage.amount() == 0 && damage.absorbed() == 0 && resisted == 0 && blocked == 0 {
        return;
    }
    let land_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros((travel_ms as i64) * 1000))
        .unwrap_or(crate::time::now(ctx));
    ctx.db
        .game_ranged_impact_schedule()
        .insert(RangedImpactSchedule {
            scheduled_id: 0,
            scheduled_at: crate::time::schedule_time(ctx, land_at),
            attacker_guid,
            target_guid,
            damage: damage.amount(),
            ranged_spell_id,
            is_crit,
            school: if ranged_spell_id == 5019 {
                super::folds::ranged_school(ctx, attacker_guid)
            } else {
                ctx.db
                    .game_spell()
                    .spell_id()
                    .find(ranged_spell_id)
                    .map_or(0, |spell| crate::spell::school_index(spell.school_mask))
            },
            resisted,
            blocked,
            absorbed: damage.absorbed(),
        });
}

#[reducer]
pub fn ranged_impact(ctx: &ReducerContext, shot: RangedImpactSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(attacker) = entities.guid().find(shot.attacker_guid) else {
        return;
    };
    let Some(target) = entities.guid().find(shot.target_guid) else {
        return;
    };
    if target.dead
        || (shot.damage == 0 && shot.absorbed == 0 && shot.resisted == 0 && shot.blocked == 0)
    {
        return;
    }
    if !crate::combat::may_harm(ctx, &attacker, &target) {
        return;
    }
    // vmangos Spell::CheckAtDelay tests evade again when the shot lands.
    if crate::creatures::eventai::movement::returning_home(ctx, shot.target_guid) {
        if shot.ranged_spell_id != 0 {
            crate::native::combat::spell_log_miss(
                ctx,
                shot.attacker_guid,
                shot.target_guid,
                shot.ranged_spell_id,
                crate::spell::CAST_MISS_EVADE,
            );
        }
        return;
    }
    // An auto-repeat shot lands as a white hit; a Weapon Strike also keeps its spell.
    let strike = ctx
        .db
        .game_spell()
        .spell_id()
        .find(shot.ranged_spell_id)
        .is_some_and(|spell| {
            spell.cast_flags & lyracore_shared::spell::taxonomy::CAST_FLAG_RANGED_AUTO_REPEAT == 0
        });
    // vmangos breaks a Weapon Strike's stealth on hit, not at launch.
    if strike {
        crate::spell::break_stealth(ctx, shot.attacker_guid);
    }

    let damage = FrozenDamage::stored(shot.damage, shot.absorbed).resolve(ctx, &target);
    let hit = if strike {
        Hit::spell(HitSource::Ranged, shot.ranged_spell_id, shot.is_crit)
    } else {
        Hit::weapon(HitSource::Ranged, shot.is_crit)
    }
    .with_block(shot.blocked);
    let dealt = damage.amount();
    let outcome = damage.land(ctx, shot.attacker_guid, shot.target_guid, hit);
    crate::spell::proc::run_unlanded_proc_pass(
        ctx,
        shot.attacker_guid,
        shot.target_guid,
        &hit,
        AttackOutcome::Normal,
        (dealt, damage.absorbed()),
    );
    let reports_damage = damage.amount() > 0 || shot.resisted > 0 || shot.blocked > 0;
    if reports_damage && shot.ranged_spell_id != 0 {
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid: shot.target_guid,
                damage: damage.amount(),
                is_crit: shot.is_crit,
                school: shot.school,
                resisted: shot.resisted,
                blocked_amount: shot.blocked,
                is_proc_log: true,
                ..SpellCastEvent::signal_at(
                    ctx,
                    &attacker,
                    shot.ranged_spell_id,
                    SpellCastEventKind::ProcLog,
                )
            },
        );
    }
    if outcome.duel_completed {
        return;
    }
    enter_combat(ctx, shot.attacker_guid);
    enter_combat(ctx, shot.target_guid);
}

#[cfg(test)]
mod damage_pipeline_drift_tests {
    #[test]
    fn seal_projection_sums_to_the_floored_final_damage() {
        assert_eq!(super::split_damage_projection(0, 5), (0, 0));
        assert_eq!(super::split_damage_projection(3, 5), (0, 3));
        assert_eq!(super::split_damage_projection(9, 5), (4, 5));
    }
}

#[cfg(test)]
mod revenge_tests {
    use super::{arms_revenge, AttackOutcome};

    #[test]
    fn a_blocked_crit_arms_revenge() {
        assert!(arms_revenge(AttackOutcome::Crit, 30));
        assert!(arms_revenge(AttackOutcome::Dodge, 0));
        assert!(!arms_revenge(AttackOutcome::Normal, 0));
    }
}
