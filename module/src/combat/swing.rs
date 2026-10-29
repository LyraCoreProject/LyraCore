use spacetimedb::{reducer, ReducerContext, Table, TimeDuration};

use crate::spell::SpellCastEventKind;
use crate::{game_item_template, game_spell, game_world_entity, SpellCastEvent, WorldEntity};

use super::*;

#[reducer]
pub fn tick_melee(ctx: &ReducerContext, _schedule: MeleeSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    melee_pass(ctx);
}

fn melee_pass(ctx: &ReducerContext) {
    leash_pass(ctx);
    aggro_pass(ctx);
    resolve_swing(ctx);
}

fn leash_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let now_ms = (crate::time::now(ctx).to_micros_since_unix_epoch() / 1000) as u32;

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
        if crate::creatures::beyond_combat_backstop(dx * dx + dy * dy + dz * dz) {
            evaders.push(attacker.guid);
            continue;
        }
        if atk.pursuit_ends_ms == 0 {
            seeds.push((atk.attacker_guid, attacker.x, attacker.y));
            continue;
        }
        let (rx, ry) = (target.x - atk.leash_x, target.y - atk.leash_y);
        if crate::creatures::should_evade(now_ms, atk.pursuit_ends_ms, rx * rx + ry * ry) {
            evaders.push(atk.attacker_guid);
        }
    }
    for (creature, x, y) in seeds {
        if let Some(mut row) = melee.attacker_guid().find(creature) {
            row.pursuit_ends_ms = crate::creatures::pursuit_deadline_ms(now_ms);
            row.leash_x = x;
            row.leash_y = y;
            crate::combat::MeleeAttack::update(ctx, row);
        }
    }
    for creature in evaders {
        crate::creatures::eventai_on_evade(ctx, creature);
        disengage(ctx, creature);
        if let Some(mut c) = entities.guid().find(creature) {
            if c.health != c.max_health {
                c.health = c.max_health;
                c.update(ctx);
            }
        }
    }
}

fn aggro_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();

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
        let dx = attacker.x - target.x;
        let dy = attacker.y - target.y;
        let dz = attacker.z - target.z;
        if attacker.is_player()
            && !target.is_player()
            && !target.dead
            && ((is_ranged
                && target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0)
                || (atk.last_swing_ms != 0 && dx * dx + dy * dy + dz * dz <= MELEE_RANGE_LEEWAY_SQ))
            && melee.attacker_guid().find(target.guid).is_none()
            && !new_aggro.iter().any(|(c, _)| *c == target.guid)
        {
            new_aggro.push((target.guid, attacker.guid));
        }
    }
    for (creature, player) in new_aggro {
        arm_creature_engagement(ctx, creature, player, false);
    }
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

    let class = attacker.class();
    let ap = melee_attack_power_for(
        class,
        effective_strength(ctx, attacker),
        effective_agility(ctx, attacker),
        attacker.level,
    ) + aura_attack_power_bonus(ctx, attacker.guid);
    let (raw_min, raw_max) = weapon_swing_range_ap(ap, dmin, dmax, delay);
    let range = apply_offhand_penalty(raw_min, raw_max);

    let (rolled, hit_info, blocked) = roll_swing_with_range(ctx, attacker, &target, range, None);

    crate::spell::break_stealth(ctx, attacker_guid);

    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    let damage = final_damage(ctx, target_guid, dmg);
    let killing_blow = is_killing_blow(ctx, attacker_guid, &target, damage.amount);

    CombatEvent::insert(
        ctx,
        CombatEvent {
            damage: damage.amount,
            hit_info,
            killing_blow,
            blocked_amount: blocked,
            ..CombatEvent::signal_at(ctx, attacker, target_guid)
        },
    );

    if apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        damage,
        Hit::weapon(HitSource::OffHand, hit_info == HIT_CRIT),
    )
    .combat_ended()
    {
        return;
    }

    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_offhand_swing_ms = now_ms;
        crate::combat::MeleeAttack::update(ctx, fresh);
    }
}

fn resolve_swing(ctx: &ReducerContext) {
    let now_ms = (crate::time::now(ctx).to_micros_since_unix_epoch() / 1000) as u32;
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();

    let attacks: Vec<MeleeAttack> = melee.iter().collect();
    for atk in attacks {
        let Some(attacker) = entities.guid().find(atk.attacker_guid) else {
            crate::combat::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        };
        let Some(mut target) = entities.guid().find(atk.target_guid) else {
            crate::combat::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        };

        if attacker.dead || target.dead {
            crate::combat::MeleeAttack::delete(ctx, atk.attacker_guid);
            continue;
        }
        if melee.attacker_guid().find(atk.attacker_guid).is_none() {
            continue;
        }
        if !crate::combat::may_harm(ctx, &attacker, &target) {
            crate::combat::MeleeAttack::delete(ctx, atk.attacker_guid);
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
                    crate::combat::MeleeAttack::end_auto_repeat(ctx, atk.attacker_guid);
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
                    crate::combat::MeleeAttack::end_auto_repeat(ctx, atk.attacker_guid);
                } else if ranged_due {
                    if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
                        fresh.last_swing_ms = now_ms
                            .wrapping_add(RANGED_INITIAL_SHOT_MS)
                            .wrapping_sub(delay)
                            .max(1);
                        crate::combat::MeleeAttack::update(ctx, fresh);
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
                crate::combat::MeleeAttack::end_auto_repeat(ctx, atk.attacker_guid);
            } else if ranged.is_none() && attacker.is_player() {
                record_swing_error(ctx, &atk, &attacker, block.swing_error(), now_ms);
            }
            continue;
        }

        if ranged.is_none() {
            resolve_offhand_swing(ctx, atk.attacker_guid, atk.target_guid, &attacker, now_ms);
            if melee.attacker_guid().find(atk.attacker_guid).is_none() {
                continue;
            }

            match entities.guid().find(atk.target_guid) {
                Some(refreshed) => target = refreshed,
                None => {
                    crate::combat::MeleeAttack::delete(ctx, atk.attacker_guid);
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
            None => fire_melee_swing(ctx, &attacker, &target, now_ms),
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
    let range_sq = if is_ranged {
        RANGED_RANGE_SQ
    } else {
        let both_moving = now_ms.wrapping_sub(attacker.last_move_ms) < MELEE_LEEWAY_WINDOW_MS
            && now_ms.wrapping_sub(target.last_move_ms) < MELEE_LEEWAY_WINDOW_MS;
        if both_moving {
            if attacker.is_player() {
                MELEE_RANGE_LEEWAY_SQ
            } else {
                MELEE_RANGE_LEEWAY_CREATURE_SQ
            }
        } else {
            MELEE_RANGE_SQ
        }
    };
    if dist_sq > range_sq {
        return Some(SwingBlock::OutOfRange);
    }
    if is_ranged && dist_sq < MELEE_RANGE_SQ {
        return Some(SwingBlock::TooClose);
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
        && !crate::spell::is_facing(
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
        crate::combat::MeleeAttack::update(ctx, fresh);
    }
}

// A Character pays when the swing fires, even on a miss; short of the cost, it swings white.
fn charge_queued_strike(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    queued_spell: u32,
    strike: Option<&crate::spell::Spell>,
) -> bool {
    if !attacker.is_player() {
        return true;
    }
    let Some(mut payer) = ctx.db.game_world_entity().guid().find(attacker.guid) else {
        return false;
    };
    if strike.is_some_and(|strike| payer.power < strike.cost) {
        crate::spell::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                is_interrupted: true,
                failure_reason: crate::spell::CAST_FAIL_NO_POWER,
                ..SpellCastEvent::signal_at(
                    ctx,
                    attacker,
                    queued_spell,
                    SpellCastEventKind::Interrupt,
                )
            },
        );
        return false;
    }
    if let Some(strike) = strike {
        crate::spell::charge_spell_cost(ctx, &mut payer, strike);
    }
    payer.update(ctx);
    true
}

fn arms_revenge(hit_info: u8, blocked: u32) -> bool {
    matches!(hit_info, HIT_DODGE | HIT_PARRY | HIT_BLOCK) || blocked > 0
}

fn fire_melee_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    now_ms: u32,
) {
    let attacker_guid = attacker.guid;
    let target_guid = target.guid;
    let melee = ctx.db.game_melee_attack();

    let queued_spell = attacker.next_swing_spell;
    let queued_header = ctx.db.game_spell().spell_id().find(queued_spell);
    let swing_is_spell = queued_spell != 0
        && charge_queued_strike(ctx, attacker, queued_spell, queued_header.as_ref());
    let strike = swing_is_spell.then(|| {
        queued_header
            .as_ref()
            .map_or_else(Default::default, |strike| {
                queued_strike_damage(ctx, attacker, strike)
            })
    });

    let (mut rolled, hit_info, blocked) = roll_swing(ctx, attacker, target, strike);

    if hit_info == HIT_DODGE {
        arm_react(ctx, attacker_guid, ReactKind::Overpower);
    }

    let (mut seal_portion, mut seal_spell) = (0u32, 0u32);
    if rolled > 0 {
        let (p, s) = seal_holy_on_swing(ctx, attacker);
        seal_portion = p;
        seal_spell = s;
        rolled += seal_portion;
    }

    let queued_fired = swing_is_spell && rolled > 0;
    clear_queued_strike(ctx, attacker_guid);

    crate::spell::break_stealth(ctx, attacker_guid);

    if attacker.is_player() {
        wear_weapon(ctx, attacker_guid);
    }

    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    let damage = final_damage(ctx, target_guid, dmg);
    let (projected_swing, projected_seal) = split_damage_projection(damage.amount, seal_portion);

    CombatEvent::insert(
        ctx,
        CombatEvent {
            damage: projected_swing,
            hit_info,
            killing_blow: is_killing_blow(ctx, attacker_guid, target, damage.amount),
            blocked_amount: blocked,
            spell_swing: swing_is_spell,
            ..CombatEvent::signal_at(ctx, attacker, target_guid)
        },
    );
    if swing_is_spell {
        let cooldown_ms = queued_header.as_ref().map_or(0, |h| h.cooldown_ms);
        crate::spell::start_spell_cooldown(ctx, attacker_guid, queued_spell, cooldown_ms);
        crate::spell::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid,
                is_completion: true,
                damage: if queued_fired { projected_swing } else { 0 },
                is_crit: hit_info == HIT_CRIT,
                swing_hit_info: hit_info,
                cooldown_ms,
                ..SpellCastEvent::signal_at(ctx, attacker, queued_spell, SpellCastEventKind::Go)
            },
        );
    }
    if projected_seal > 0 {
        crate::spell::SpellCastEvent::insert(
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

    let combat_ended = apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        damage,
        if swing_is_spell {
            Hit::spell(HitSource::MainHand, queued_spell, hit_info == HIT_CRIT)
        } else {
            Hit::weapon(HitSource::MainHand, hit_info == HIT_CRIT)
        },
    )
    .combat_ended();
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
        return;
    }

    // Arm Revenge after damage so a stale defender snapshot cannot erase its deadline.
    if arms_revenge(hit_info, blocked) {
        arm_react(ctx, target_guid, ReactKind::Revenge);
    }

    // Re-read before stamping; the off-hand may have updated its clock on this row.
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_swing_ms = now_ms;
        fresh.swing_error = SWING_ERROR_NONE;
        crate::combat::MeleeAttack::update(ctx, fresh);
    }

    // Enter combat after entity writes so a stale snapshot cannot erase the flag.
    enter_combat(ctx, attacker_guid);
    enter_combat(ctx, target_guid);
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
                crate::combat::MeleeAttack::end_auto_repeat(ctx, attacker_guid);
                return;
            }
        }
    } else {
        0
    };

    let (rdmin, rdmax) = if attacker.is_player() {
        let (_, rdmin, rdmax) = ranged_sheet_values(
            effective_agility(ctx, attacker),
            attacker.level,
            Some(weapon),
        );
        (rdmin, rdmax)
    } else {
        (dmin, dmax)
    };
    let (rolled, hit_info) = roll_ranged_swing(ctx, attacker, target, rdmin, rdmax);

    crate::spell::break_stealth(ctx, attacker_guid);

    // Freeze mitigation at launch; resolve lethal floors and the damage log together at impact.
    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);

    let speed = if subclass == ws::WAND {
        WAND_PROJECTILE_SPEED
    } else {
        AUTO_SHOT_PROJECTILE_SPEED
    };
    let travel_ms = crate::spell::projectile_travel_ms(dist_sq.sqrt(), speed);
    CombatEvent::insert(
        ctx,
        CombatEvent {
            damage: 0,
            hit_info,
            ranged_spell_id,
            ammo_display_id,
            impact_delay_ms: travel_ms,
            ..CombatEvent::signal_at(ctx, attacker, target_guid)
        },
    );
    schedule_ranged_impact(
        ctx,
        attacker_guid,
        target_guid,
        dmg,
        ranged_spell_id,
        hit_info == HIT_CRIT,
        travel_ms,
    );
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_swing_ms = now_ms;
        crate::combat::MeleeAttack::update(ctx, fresh);
    }
}

pub(crate) fn launch_ranged_strike(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    rolled: u32,
    spell_id: u32,
    is_crit: bool,
) {
    let (dmg, _absorbed) = fold_incoming_damage(ctx, attacker.guid, target.guid, rolled);
    let (dx, dy, dz) = (
        target.x - attacker.x,
        target.y - attacker.y,
        target.z - attacker.z,
    );
    let travel_ms = crate::spell::projectile_travel_ms(
        (dx * dx + dy * dy + dz * dz).sqrt(),
        AUTO_SHOT_PROJECTILE_SPEED,
    );
    schedule_ranged_impact(
        ctx,
        attacker.guid,
        target.guid,
        dmg,
        spell_id,
        is_crit,
        travel_ms,
    );
}

fn schedule_ranged_impact(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    damage: u32,
    ranged_spell_id: u32,
    is_crit: bool,
    travel_ms: u32,
) {
    if damage == 0 {
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
            damage,
            ranged_spell_id,
            is_crit,
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
    if target.dead || shot.damage == 0 {
        return;
    }
    if !crate::combat::may_harm(ctx, &attacker, &target) {
        return;
    }

    let dmg = if target.godmode { 0 } else { shot.damage };
    let damage = final_damage(ctx, shot.target_guid, dmg);
    // An auto-repeat shot lands as a white hit; a Weapon Strike keeps its spell and crit.
    let strike = ctx
        .db
        .game_spell()
        .spell_id()
        .find(shot.ranged_spell_id)
        .is_some_and(|spell| {
            spell.cast_flags & lyracore_shared::spell::taxonomy::SPELL_ATTR_RANGED_AUTO_REPEAT == 0
        });
    let hit = if strike {
        Hit::spell(HitSource::Ranged, shot.ranged_spell_id, shot.is_crit)
    } else {
        Hit::weapon(HitSource::Ranged, false)
    };
    let outcome = apply_hit(ctx, shot.attacker_guid, shot.target_guid, damage, hit);
    if damage.amount > 0 && shot.ranged_spell_id != 0 {
        crate::spell::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid: shot.target_guid,
                damage: damage.amount,
                is_crit: shot.is_crit,
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
    use super::{arms_revenge, HIT_CRIT, HIT_DODGE, HIT_NORMAL};

    #[test]
    fn a_blocked_crit_arms_revenge() {
        assert!(arms_revenge(HIT_CRIT, 30));
        assert!(arms_revenge(HIT_DODGE, 0));
        assert!(!arms_revenge(HIT_NORMAL, 0));
    }
}
