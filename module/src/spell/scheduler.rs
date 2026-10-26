use spacetimedb::{log, reducer, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use crate::{game_world_entity, WorldEntity};

use super::*;

#[reducer]
pub(crate) fn fire_pending_cast(ctx: &ReducerContext, sched: PendingCast) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    // Release this cast identity before effect hooks can begin another cast.
    ctx.db
        .game_pending_cast()
        .scheduled_id()
        .delete(sched.scheduled_id);
    let result = check_pending_cast_los(ctx, sched.caster_guid, sched.spell_id, sched.target_guid)
        .and_then(|()| {
            resolve_cast_at_typed(
                ctx,
                sched.caster_guid,
                sched.spell_id,
                sched.level,
                sched.target_guid,
                true,
                false,
                sched
                    .has_dest
                    .then_some((sched.dest_x, sched.dest_y, sched.dest_z)),
            )
        });
    clear_dead_callback_cast_admission(ctx, sched.caster_guid, sched.spell_id);
    if let Err(e) = &result {
        log::info!(
            "pending cast {} (spell {}, caster {}) did not resolve: {}",
            sched.scheduled_id,
            sched.spell_id,
            sched.caster_guid,
            e
        );
        // Only a failed SMSG_CAST_RESULT clears the 1.12 client's filled cast bar.
        crate::spell::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                is_interrupted: true,
                failure_reason: CAST_FAIL_INTERRUPTED,
                ..SpellCastEvent::signal(
                    ctx,
                    sched.caster_guid,
                    sched.spell_id,
                    SpellCastEventKind::Interrupt,
                )
            },
        );
    }
    crate::hooks::fire_on_cast_finished(
        ctx,
        &crate::hooks::CastFinishedPayload {
            caster_guid: sched.caster_guid,
            target_guid: sched.target_guid,
            scheduled_id: sched.scheduled_id,
            outcome: match result {
                Ok(()) => CastFinish::Resolved,
                Err(refusal) => CastFinish::Refused(refusal.into()),
            },
        },
    );
}

#[reducer]
pub fn fire_spell_impact(ctx: &ReducerContext, sched: PendingSpellImpact) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let (dealt, absorbed) = apply_target_damage(
        ctx,
        sched.target_guid,
        sched.caster_guid,
        sched.after_resist,
        crate::combat::Hit::spell(
            if sched.triggered {
                crate::combat::HitSource::Triggered
            } else {
                crate::combat::HitSource::Spell
            },
            sched.spell_id,
            sched.is_crit,
        ),
    );
    let (map_id, instance_id, grid_x, grid_y) = crate::helpers::grid_of(ctx, sched.caster_guid);
    crate::spell::SpellImpactEvent::insert(
        ctx,
        SpellImpactEvent {
            id: 0,
            caster_guid: sched.caster_guid,
            target_guid: sched.target_guid,
            spell_id: sched.spell_id,
            created_at: ctx.timestamp,
            damage: dealt,
            school: sched.school_index,
            is_crit: sched.is_crit,
            resisted: sched.resisted,
            absorbed,
            map_id,
            instance_id,
            grid_x,
            grid_y,
        },
    );
}

pub(crate) fn do_cast_spell(
    ctx: &ReducerContext,
    caster: crate::WorldEntity,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), crate::spell::CastGateRefusal> {
    if !knows_spell(ctx, caster.guid, spell_id) {
        return Err(format!("spell {spell_id} is not in the caster's spellbook").into());
    }
    let target = if target_guid == 0 {
        caster.guid
    } else {
        target_guid
    };
    begin_cast(
        ctx,
        caster.guid,
        spell_id,
        caster.level as u8,
        target,
        true,
        None,
    )
}

pub(crate) fn apply_cast_spell_at(
    ctx: &ReducerContext,
    caster: &crate::WorldEntity,
    spell_id: u32,
    target_guid: u64,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), crate::spell::CastGateRefusal> {
    if !knows_spell(ctx, caster.guid, spell_id) {
        return Err(format!("spell {spell_id} is not in the caster's spellbook").into());
    }
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return Err(format!("cast_spell_at: non-finite ground point ({x},{y},{z})").into());
    }
    let target = if target_guid == 0 {
        caster.guid
    } else {
        target_guid
    };
    begin_cast(
        ctx,
        caster.guid,
        spell_id,
        caster.level as u8,
        target,
        true,
        Some((x, y, z)),
    )
}

pub(crate) fn do_resurrect_response(
    ctx: &ReducerContext,
    target_guid: u64,
    accept: bool,
) -> Result<(), crate::Refusal> {
    let requests = ctx.db.game_resurrect_request();
    let req = requests
        .target_guid()
        .find(target_guid)
        .ok_or_else(|| "no pending resurrect offer".to_string())?;
    ResurrectRequest::delete(ctx, target_guid);
    if accept {
        if let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) {
            if t.dead {
                let pct = req.points.clamp(1, 100) as u32;
                t.health = revive_amount(t.max_health, req.points);
                t.power = t.max_power.saturating_mul(pct) / 100;
                t.update(ctx);
                crate::ghost::finish_resurrection(ctx, target_guid);
            }
        }
    }
    Ok(())
}

pub(crate) fn do_cancel_cast(
    ctx: &ReducerContext,
    caster: crate::WorldEntity,
) -> Result<(), crate::Refusal> {
    interrupt_cast(ctx, caster.guid);
    break_channel(ctx, caster.guid);
    if caster.next_swing_spell != 0 {
        let mut c = caster;
        c.next_swing_spell = 0;
        c.update(ctx);
    }
    Ok(())
}

pub(crate) fn do_cancel_aura(
    ctx: &ReducerContext,
    player_guid: u64,
    spell_id: u32,
) -> Result<(), crate::Refusal> {
    if crate::ghost::is_ghost_spell(spell_id) {
        return Err("ghost effects end on resurrection".into());
    }
    if let Some(spell) = ctx.db.game_spell().spell_id().find(spell_id) {
        if spell.attributes & SPELL_ATTR_PASSIVE != 0 {
            return Err(format!("spell {spell_id} is passive and cannot be cancelled").into());
        }
        if spell.is_negative {
            return Err(format!("spell {spell_id} is a debuff and cannot be cancelled").into());
        }
    }
    let auras = ctx.db.game_aura();
    let redisplay = auras
        .by_target()
        .filter(&player_guid)
        .any(|a| a.spell_id == spell_id && a.eff_kind == A_TRANSFORM);
    let to_remove: Vec<(u64, bool, bool, bool)> = auras
        .by_target()
        .filter(&player_guid)
        .filter(|a| a.spell_id == spell_id)
        .map(|a| {
            (
                a.id,
                aura_moves_vitals(a.eff_kind, a.eff_p0),
                crate::spell::aura_moves_sheet(a.eff_kind, a.eff_p0),
                crate::mount::mount_aura_moves_mount(a.eff_kind, a.eff_p0),
            )
        })
        .collect();
    if to_remove.is_empty() {
        return Err(format!("no cancelable aura {spell_id} on the caller").into());
    }
    let mut revitalize = false;
    let mut resheet = false;
    let mut remount = false;
    for (id, moves_vitals, moves_sheet, moves_mount) in to_remove {
        crate::Aura::delete(ctx, id);
        revitalize |= moves_vitals;
        resheet |= moves_sheet;
        remount |= moves_mount;
    }
    if revitalize {
        recompute_vitals(ctx, player_guid);
    }
    if resheet {
        recompute_sheet(ctx, player_guid);
    }
    if remount {
        crate::mount::recompute_mount(ctx, player_guid);
    }
    if redisplay {
        recompute_display(ctx, player_guid);
    }
    Ok(())
}

pub(crate) fn is_due_for_expiry(eff_kind: u8, expires_at: Timestamp, now: Timestamp) -> bool {
    expires_at <= now && eff_kind != A_STEALTH
}

// Run periodic effects before expiry so the final tick at the boundary is delivered.
#[reducer]
#[allow(clippy::too_many_lines)]
pub fn tick_auras(ctx: &ReducerContext, _schedule: AuraSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let now = ctx.timestamp;
    let now_micros = now.to_micros_since_unix_epoch();
    let auras = ctx.db.game_aura();
    let entities = ctx.db.game_world_entity();

    let mut due: Vec<Aura> = auras
        .by_next_tick()
        .filter(1..=now_micros)
        .filter(|a| a.expires_at >= now && a.period_ms > 0)
        .collect();
    // Aura id order determines killing-blow credit when several DoTs share a target.
    due.sort_unstable_by_key(|a| a.id);

    let mut pending: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    let mut pending_power: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    let mut dot_damaged: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut dying: std::collections::HashMap<u64, Option<u64>> = std::collections::HashMap::new();
    let mut finishing_duels: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut duel_losers: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut duel_finishes: Vec<(u64, u64, u64)> = Vec::new();
    // Defer channel effects that may kill targets until folds and cadence updates finish.
    let mut channel_fires: Vec<(u64, u32, u8, u64)> = Vec::new();
    let mut channel_end: Vec<u64> = Vec::new();
    for a in &due {
        let Some(target) = entities.guid().find(a.target_guid) else {
            continue;
        };
        if target.dead {
            continue;
        }
        match a.eff_kind {
            A_PERIODIC_DAMAGE => {
                let caster = entities.guid().find(a.caster_guid);
                if caster
                    .as_ref()
                    .is_some_and(|caster| !crate::combat::may_harm(ctx, caster, &target))
                {
                    continue;
                }
                let active_duel =
                    crate::duel::active_duel_between(ctx, a.caster_guid, a.target_guid);
                if active_duel
                    .as_ref()
                    .is_some_and(|duel| finishing_duels.contains(&duel.id))
                {
                    continue;
                }
                if dying.contains_key(&a.target_guid) {
                    continue;
                }
                let cur = pending
                    .get(&a.target_guid)
                    .copied()
                    .unwrap_or(target.health);
                let school = ctx
                    .db
                    .game_spell()
                    .spell_id()
                    .find(a.spell_id)
                    .map(|s| s.school_mask)
                    .unwrap_or(0);
                let amount =
                    resist_magic_damage(ctx, a.target_guid, school, a.level as u32, a.amount);
                crate::spell::SpellCastEvent::insert(
                    ctx,
                    SpellCastEvent {
                        damage: amount.max(0) as u32,
                        school: crate::spell::school_index(school),
                        resisted: a.amount.saturating_sub(amount).max(0) as u32,
                        ..SpellCastEvent::periodic_log(ctx, a.caster_guid, a.spell_id, &target)
                    },
                );
                if amount > 0 {
                    dot_damaged.insert(a.target_guid);
                    if !target.is_player()
                        && entities
                            .guid()
                            .find(a.caster_guid)
                            .map(|c| c.is_player())
                            .unwrap_or(false)
                    {
                        crate::threat::add_threat(
                            ctx,
                            a.target_guid,
                            a.caster_guid,
                            crate::threat::threat_from_damage(amount as u32),
                        );
                    }
                }
                if target.is_player() && amount > 0 && (amount as u32) >= cur {
                    if let Some(duel) = active_duel {
                        pending.insert(a.target_guid, 1);
                        finishing_duels.insert(duel.id);
                        duel_losers.insert(a.target_guid);
                        duel_finishes.push((duel.id, a.caster_guid, a.target_guid));
                    } else {
                        pending.insert(a.target_guid, damaged_value(cur, amount));
                    }
                } else if !target.is_player() && amount > 0 && (amount as u32) >= cur {
                    if crate::combat::lethal_floor_protects(ctx, a.target_guid) {
                        pending.insert(a.target_guid, 1);
                        crate::combat::commit_death_prevention(ctx, a.target_guid, a.caster_guid);
                    } else {
                        let killer = entities
                            .guid()
                            .find(a.caster_guid)
                            .filter(|c| c.is_player())
                            .map(|_| a.caster_guid);
                        dying.insert(a.target_guid, killer);
                        pending.remove(&a.target_guid);
                    }
                } else {
                    pending.insert(a.target_guid, damaged_value(cur, amount));
                }
            }
            A_PERIODIC_HEAL => {
                if dying.contains_key(&a.target_guid) || duel_losers.contains(&a.target_guid) {
                    continue;
                }
                let cur = pending
                    .get(&a.target_guid)
                    .copied()
                    .unwrap_or(target.health);
                let new_health = healed_value(cur, target.max_health, a.amount);
                pending.insert(a.target_guid, new_health);
                if cur < target.max_health {
                    crate::spell::SpellCastEvent::insert(
                        ctx,
                        SpellCastEvent {
                            healed: a.amount.max(0) as u32,
                            ..SpellCastEvent::periodic_log(ctx, a.caster_guid, a.spell_id, &target)
                        },
                    );
                }
                let effective = new_health.saturating_sub(cur);
                crate::threat::add_heal_threat(ctx, a.caster_guid, a.target_guid, effective);
            }
            A_PERIODIC_ENERGIZE if energizes_pool(a.eff_p0_kind, a.eff_p0, target.power_type()) => {
                let cur = pending_power
                    .get(&a.target_guid)
                    .copied()
                    .unwrap_or(target.power);
                pending_power.insert(
                    a.target_guid,
                    energized_value(cur, target.max_power, a.amount),
                );
                crate::spell::SpellCastEvent::insert(
                    ctx,
                    SpellCastEvent {
                        energized: a.amount.max(0) as u32,
                        power_type: target.power_type(),
                        ..SpellCastEvent::periodic_log(ctx, a.caster_guid, a.spell_id, &target)
                    },
                );
                if a.enters_combat {
                    crate::combat::enter_combat(ctx, a.caster_guid);
                }
            }
            A_PERIODIC_TRIGGER => {
                let target_alive = entities
                    .guid()
                    .find(a.channel_target)
                    .map(|t| !t.dead)
                    .unwrap_or(false);
                if is_action_blocked(ctx, a.caster_guid) || !target_alive {
                    channel_end.push(a.id);
                } else {
                    channel_fires.push((a.caster_guid, a.eff_p1 as u32, a.level, a.channel_target));
                }
            }
            _ => {}
        }
    }
    flush_pool(ctx, &pending, |e, v| e.health = v);
    flush_pool(ctx, &pending_power, |e, v| e.power = v);
    for (duel_id, winner_guid, loser_guid) in duel_finishes {
        crate::duel::complete_duel(
            ctx,
            duel_id,
            crate::duel::duel_completion_kind::WON,
            winner_guid,
            loser_guid,
        );
    }
    for (guid, killer) in &dying {
        crate::combat::kill_creature(ctx, *guid, *killer);
    }
    // Death handling may delete or change due auras; advance only surviving rows.
    for a in due {
        let Some(mut a2) = auras.id().find(a.id) else {
            continue;
        };
        a2.next_tick_micros += (a2.period_ms as i64) * 1000;
        a2.update(ctx);
    }

    for id in channel_end {
        crate::Aura::delete(ctx, id);
    }
    for (caster_guid, trigger_spell, level, channel_target) in channel_fires {
        if trigger_spell == 0 {
            continue;
        }
        if let Err(e) = resolve_cast_at(
            ctx,
            caster_guid,
            trigger_spell,
            level,
            channel_target,
            false,
            false,
            None,
        ) {
            log::info!(
                "channel tick: caster {caster_guid}'s trigger {trigger_spell} at {channel_target} did not resolve: {e}"
            );
        }
    }

    let expired: Vec<Aura> = auras
        .by_expiry()
        .filter(..=now)
        .filter(|a| is_due_for_expiry(a.eff_kind, a.expires_at, now))
        .collect();
    log::debug!(
        "tick_auras: expiry index scan visited {} of {} total aura rows",
        expired.len(),
        auras.count()
    );
    let redisplay: std::collections::BTreeSet<u64> = expired
        .iter()
        .filter(|a| a.eff_kind == A_TRANSFORM)
        .map(|a| a.target_guid)
        .collect();
    let mut revitalize: Vec<u64> = Vec::new();
    let mut resheet: Vec<u64> = Vec::new();
    let mut remount: Vec<u64> = Vec::new();
    for a in &expired {
        if aura_moves_vitals(a.eff_kind, a.eff_p0) && !revitalize.contains(&a.target_guid) {
            revitalize.push(a.target_guid);
        }
        if crate::spell::aura_moves_sheet(a.eff_kind, a.eff_p0) && !resheet.contains(&a.target_guid)
        {
            resheet.push(a.target_guid);
        }
        if crate::mount::mount_aura_moves_mount(a.eff_kind, a.eff_p0)
            && !remount.contains(&a.target_guid)
        {
            remount.push(a.target_guid);
        }
    }
    for a in &expired {
        if let Some(category) = crate::spell::stacking::dr_category_for_effect(
            ctx,
            a.eff_kind,
            a.eff_p0_kind,
            a.eff_p0,
            a.target_guid,
        ) {
            crate::spell::stacking::dr_window_on_removal(ctx, a.target_guid, category, now_micros);
        }
    }
    for a in &expired {
        crate::Aura::delete(ctx, a.id);
    }
    for target_guid in revitalize {
        recompute_vitals(ctx, target_guid);
    }
    for target_guid in resheet {
        recompute_sheet(ctx, target_guid);
    }
    for target_guid in remount {
        crate::mount::recompute_mount(ctx, target_guid);
    }
    for target_guid in redisplay {
        recompute_display(ctx, target_guid);
    }

    // Break auras after cadence advancement to avoid advancing or deleting them twice.
    for target_guid in dot_damaged {
        break_auras_on_damage(ctx, target_guid, 0, true);
    }
}

// Re-read each entity so flushing power preserves the earlier health flush.
fn flush_pool(
    ctx: &ReducerContext,
    pool: &std::collections::HashMap<u64, u32>,
    set: impl Fn(&mut WorldEntity, u32),
) {
    let entities = ctx.db.game_world_entity();
    for (&guid, &value) in pool {
        if let Some(mut target) = entities.guid().find(guid) {
            set(&mut target, value);
            target.update(ctx);
        }
    }
}

#[reducer]
pub fn tick_ground_areas(ctx: &ReducerContext, _schedule: GroundAreaSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    const AOE_MAX_TARGETS: usize = 20;
    let now = ctx.timestamp;
    let now_micros = now.to_micros_since_unix_epoch();
    let areas = ctx.db.game_ground_area();
    let entities = ctx.db.game_world_entity();

    let mut expired: Vec<u64> = Vec::new();
    let mut due: Vec<GroundArea> = Vec::new();
    for a in areas.iter() {
        let expires_micros = a.expires_at.to_micros_since_unix_epoch();
        match classify_ground_tick(a.next_tick_micros, a.period_ms, expires_micros, now_micros) {
            GroundTickStatus::Fire => due.push(a),
            GroundTickStatus::Reap => expired.push(a.id),
            GroundTickStatus::Wait => {}
        }
    }
    for id in expired {
        areas.id().delete(id);
        crate::spell::DynamicObject::delete(ctx, crate::spell::cast::dynamic_object_guid(id));
    }

    for a in &due {
        let caster = entities.guid().find(a.caster_guid);
        let r2 = a.radius_yd * a.radius_yd;
        let targets: Vec<u64> =
            crate::helpers::entities_near(ctx, a.map_id, a.instance_id, a.x, a.y, a.radius_yd)
                .into_iter()
                .filter(|c| !c.dead && c.guid != a.caster_guid)
                .filter_map(|c| {
                    let (dx, dy, dz) = (c.x - a.x, c.y - a.y, c.z - a.z);
                    if dx * dx + dy * dy + dz * dz > r2 {
                        return None;
                    }
                    caster
                        .as_ref()
                        .is_some_and(|caster| crate::combat::is_hostile_target(ctx, caster, &c))
                        .then_some(c.guid)
                })
                .take(AOE_MAX_TARGETS)
                .collect();

        for t in targets {
            let resisted = apply_resistance(ctx, t, a.caster_guid, a.school_mask, a.amount);
            let (dealt, absorbed) = apply_target_damage(
                ctx,
                t,
                a.caster_guid,
                resisted,
                crate::combat::Hit::triggered(),
            );
            if dealt > 0 || absorbed > 0 {
                crate::spell::SpellCastEvent::insert(
                    ctx,
                    crate::spell::SpellCastEvent {
                        target_guid: t,
                        damage: dealt,
                        school: crate::spell::school_index(a.school_mask),
                        resisted: (a.amount.max(0) as u32).saturating_sub(resisted.max(0) as u32),
                        absorbed,
                        is_proc_log: true,
                        ..crate::spell::SpellCastEvent::signal(
                            ctx,
                            a.caster_guid,
                            a.spell_id,
                            SpellCastEventKind::ProcLog,
                        )
                    },
                );
            }
        }
    }

    for a in &due {
        if let Some(mut fresh) = areas.id().find(a.id) {
            fresh.next_tick_micros += (fresh.period_ms as i64) * 1000;
            areas.id().update(fresh);
        }
    }

    if areas.count() == 0 {
        let schedules = ctx.db.game_ground_area_schedule();
        let ids: Vec<u64> = schedules.iter().map(|s| s.scheduled_id).collect();
        for id in ids {
            schedules.scheduled_id().delete(id);
        }
    }
}

pub(crate) fn arm_ground_area_tick(ctx: &ReducerContext) {
    if ctx.db.game_ground_area_schedule().count() > 0 {
        return;
    }
    ctx.db
        .game_ground_area_schedule()
        .insert(GroundAreaSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
        });
}

#[derive(PartialEq, Debug)]
pub(crate) enum GroundTickStatus {
    Fire,
    Reap,
    Wait,
}

// Compare scheduled tick time with expiry so late polling still delivers the final tick.
pub(crate) fn classify_ground_tick(
    next_tick_micros: i64,
    period_ms: u32,
    expires_micros: i64,
    now_micros: i64,
) -> GroundTickStatus {
    if period_ms > 0
        && next_tick_micros != 0
        && now_micros >= next_tick_micros
        && next_tick_micros <= expires_micros
    {
        GroundTickStatus::Fire
    } else if next_tick_micros == 0 || next_tick_micros > expires_micros {
        GroundTickStatus::Reap
    } else {
        GroundTickStatus::Wait
    }
}

fn energizes_pool(eff_p0_kind: u8, eff_p0: i32, power_type: u8) -> bool {
    eff_p0_kind != P_POWER_TYPE || eff_p0 == i32::from(power_type)
}

#[cfg(test)]
mod energize_tests {
    use super::*;
    use lyracore_shared::packing::power_type::{MANA, RAGE};

    #[test]
    fn mana_regen_skips_a_rage_pool() {
        assert!(energizes_pool(P_POWER_TYPE, i32::from(MANA), MANA));
        assert!(!energizes_pool(P_POWER_TYPE, i32::from(MANA), RAGE));
        assert!(energizes_pool(P_PCT_MAX_POWER, 0, RAGE));
    }
}

#[cfg(test)]
mod ground_tick_tests {
    use super::*;

    fn simulate(period_ms: u32, duration_ms: u32, poll_ms: i64, phase_us: i64) -> u32 {
        let period_us = period_ms as i64 * 1000;
        let expires = duration_ms as i64 * 1000;
        let mut next_tick = period_us;
        let mut fires = 0u32;
        let mut now = phase_us;
        loop {
            match classify_ground_tick(next_tick, period_ms, expires, now) {
                GroundTickStatus::Fire => {
                    fires += 1;
                    next_tick += period_us;
                }
                GroundTickStatus::Reap => break,
                GroundTickStatus::Wait => now += poll_ms * 1000,
            }
        }
        fires
    }

    #[test]
    fn every_scheduled_tick_fires_regardless_of_poll_phase() {
        for phase_us in [0, 1, 250_000, 499_999, 500_000, 750_000] {
            assert_eq!(
                simulate(1000, 8000, 500, phase_us),
                8,
                "phase {phase_us}us dropped/added a tick",
            );
        }
    }

    #[test]
    fn coarser_poll_than_period_still_lands_all_ticks() {
        assert_eq!(simulate(2000, 8000, 500, 123_000), 4);
        assert_eq!(simulate(500, 8000, 700, 40_000), 16);
    }

    #[test]
    fn classify_boundaries() {
        assert_eq!(
            classify_ground_tick(8000, 1000, 8000, 8000),
            GroundTickStatus::Fire
        );
        assert_eq!(
            classify_ground_tick(8001, 1000, 8000, 9000),
            GroundTickStatus::Reap
        );
        assert_eq!(
            classify_ground_tick(2000, 1000, 8000, 1500),
            GroundTickStatus::Wait
        );
        assert_eq!(
            classify_ground_tick(0, 1000, 8000, 5000),
            GroundTickStatus::Reap
        );
    }
}
