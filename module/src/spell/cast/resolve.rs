use spacetimedb::{log, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{game_creature_template, game_world_entity, WorldEntity};

use crate::items::game_item_template;

use crate::spell::*;

use crate::script_binding::game_script;

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_cast_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,
    is_completion: bool,
    client_initiated: bool,
    dest: Option<(f32, f32, f32)>,
) -> Result<(), String> {
    resolve_cast_at_typed(
        ctx,
        caster_guid,
        spell_id,
        level,
        target_guid,
        is_completion,
        client_initiated,
        dest,
    )
    .map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_cast_at_typed(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,
    is_completion: bool,
    client_initiated: bool,
    dest: Option<(f32, f32, f32)>,
) -> Result<(), CastRefusal> {
    let hdr = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| format!("unknown spell {spell_id}"))?;

    let mut caster = crate::helpers::live_entity(ctx, caster_guid)
        .map_err(|_| format!("caster {caster_guid} not in world"))?;

    // Interrupts must read the pending cast school before damage effects can clear the cast.
    let mut effects: Vec<SpellEffect> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    effects.sort_by_key(|e| (e.kind != E_INTERRUPT, e.effect_index));

    check_cast_gates(ctx, &caster, &hdr, &effects, target_guid, spell_id, level)?;
    if !is_completion {
        check_global_cooldown(ctx, caster_guid)?;
    }
    if effects.iter().any(|e| e.kind == E_DISTRACT) {
        check_distract_destination(ctx, &caster, dest, hdr.range_yd)?;
    }

    // Scheduled crafts admit reagents before charging power; all products commit together.
    let craft = if effects.iter().any(|effect| effect.kind == E_CREATE_ITEM) {
        let mut plan = crate::items::ItemStoragePlan::read(ctx, caster_guid, caster.owner_identity);
        for (entry, count) in recipe_reagents(ctx, spell_id) {
            plan.consume(entry, count).map_err(String::from)?;
        }
        plan.check_trade_offers(ctx).map_err(String::from)?;
        Some(plan)
    } else {
        None
    };

    // Flush the cost before effects so self-heals read the charged caster row.
    if caster.is_player() && hdr.cost > 0 && !queues_next_swing(&effects) {
        let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
        caster.power = caster.power.saturating_sub(hdr.cost);
        if hdr.power_type == lyracore_shared::packing::power_type::MANA {
            caster.mana_regen_paused_until_ms = now_ms + 5000;
        }
        caster.update(ctx);
    }

    // Dismount after charging power so the stale caster cannot overwrite the Mount Projection.
    if !effects.iter().any(|e| e.kind == A_MOUNTED) {
        crate::mount::dismount(ctx, caster_guid);
    }

    // Break stealth before effects so recasting Stealth can reapply its aura.
    if hdr.cast_flags & SPELL_ATTR_STEALTH_SAFE == 0 {
        break_stealth(ctx, caster_guid);
    }

    run_spell_effects(
        ctx,
        &hdr,
        &effects,
        caster_guid,
        target_guid,
        level,
        CastOrigin::Direct {
            is_completion,
            client_initiated,
        },
        dest,
        craft,
    );

    let cast_hits_enemy = effects
        .iter()
        .any(|e| matches!(e.target, T_TARGET_ENEMY | T_AREA_ENEMY | T_CHAIN_ENEMY));
    let caster_is_player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .map(|c| c.is_player())
        .unwrap_or(false);
    if caster_is_player && cast_hits_enemy {
        crate::spell::effects::arm_spell_retaliation(ctx, caster_guid, target_guid);
    }

    if !is_completion {
        start_global_cooldown(ctx, caster_guid, hdr.gcd_ms);
    }

    if hdr.cooldown_ms > 0 {
        let ready_at = ctx
            .timestamp
            .checked_add(TimeDuration::from_micros((hdr.cooldown_ms as i64) * 1000))
            .unwrap_or(ctx.timestamp);
        let cds = ctx.db.game_spell_cd();
        if let Some(mut row) = cds
            .by_caster()
            .filter(&caster_guid)
            .find(|r| r.spell_id == spell_id)
        {
            row.ready_at = ready_at;
            cds.id().update(row);
        } else {
            cds.insert(SpellCd {
                id: 0,
                caster_guid,
                spell_id,
                ready_at,
            });
        }
    }

    crate::hooks::fire_on_cast_resolved(
        ctx,
        &crate::hooks::CastResolvedPayload {
            caster_guid,
            spell_id,
            target_guid,
        },
    );
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum CastOrigin {
    Direct {
        is_completion: bool,
        client_initiated: bool,
    },
    Triggered,
    DeadCreatureCallbackTriggered,
}

impl CastOrigin {
    fn is_triggered(self) -> bool {
        matches!(self, Self::Triggered | Self::DeadCreatureCallbackTriggered)
    }

    fn allows_dead_self(self) -> bool {
        matches!(self, Self::DeadCreatureCallbackTriggered)
    }

    fn cast_event_flags(self) -> (bool, bool) {
        match self {
            Self::Direct {
                is_completion,
                client_initiated,
            } => (is_completion, client_initiated),
            Self::Triggered | Self::DeadCreatureCallbackTriggered => (false, false),
        }
    }
}

// Queuing emits no GO, which would release the client ability before its swing fires.
fn queues_next_swing(effects: &[SpellEffect]) -> bool {
    effects.iter().any(|e| e.kind == E_NEXT_SWING)
}

fn remember_hit_target(
    ordered: &mut Vec<u64>,
    seen: &mut std::collections::HashSet<u64>,
    guid: u64,
) {
    if guid != 0 && seen.insert(guid) {
        ordered.push(guid);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_spell_effects(
    ctx: &ReducerContext,
    hdr: &Spell,
    effects: &[SpellEffect],
    caster_guid: u64,
    target_guid: u64,
    level: u8,
    origin: CastOrigin,
    dest: Option<(f32, f32, f32)>,
    mut craft: Option<crate::items::ItemStoragePlan>,
) {
    let mut hit_targets = Vec::new();
    let mut seen_hit_targets = std::collections::HashSet::new();
    let triggered = origin.is_triggered();
    let mut total_damage: u32 = 0;
    let mut total_healed: u32 = 0;
    let mut total_resisted: u32 = 0;
    let mut total_absorbed: u32 = 0;
    let mut total_energized: u32 = 0;
    let mut energized_power_type: u8 = 0;
    let mut primary_missed = false;
    let mut any_crit = false;
    let mut item_grants = 0;
    let mut item_refused = false;
    // One hit roll per target for the whole spell (CONTEXT.md "Cast Miss"); never the caster.
    let rolls_hit = effects.iter().any(|e| e.kind == E_DAMAGE);
    let mut missed_targets: std::collections::HashMap<u64, bool> = std::collections::HashMap::new();
    for e in effects {
        let points = effect_amount(
            e.base_points,
            e.die_sides,
            e.per_level,
            level as u32,
            hdr.spell_level,
            hdr.max_level,
            ctx.random::<u32>(),
        );
        for t in select_targets(ctx, caster_guid, target_guid, e, dest) {
            if triggered && !triggered_target_admitted(origin, caster_guid, t, is_alive(ctx, t)) {
                continue;
            }
            if rolls_hit
                && t != caster_guid
                && *missed_targets
                    .entry(t)
                    .or_insert_with(|| super::targeting::spell_misses(ctx, level, t))
            {
                primary_missed |= t == target_guid;
                continue;
            }
            let hit = if e.kind == E_CREATE_ITEM && craft.is_some() {
                let result = ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(e.p0.max(0) as u32)
                    .ok_or_else(|| {
                        crate::actor::ActionRefusal::from(
                            "craft product has no item template".to_string(),
                        )
                    })
                    .and_then(|template| {
                        craft.as_mut().expect("craft was checked").grant(
                            ctx,
                            &template,
                            points.max(1) as u32,
                            false,
                            None,
                        )
                    });
                if let Err(ref reason) = result {
                    log::info!("craft {} refused: {reason:?}", hdr.spell_id);
                }
                super::targeting::EffectHit {
                    item: if result.is_ok() {
                        ItemGrantOutcome::Granted
                    } else {
                        ItemGrantOutcome::Refused
                    },
                    ..Default::default()
                }
            } else {
                // Combo finishers use the explicit enemy even for self-targeted effects.
                apply_effect(
                    ctx,
                    e,
                    hdr,
                    caster_guid,
                    t,
                    target_guid,
                    level,
                    points,
                    dest,
                    triggered,
                )
            };
            match hit.item {
                ItemGrantOutcome::None => {}
                ItemGrantOutcome::Granted => item_grants += 1,
                ItemGrantOutcome::Refused => item_refused = true,
            }
            remember_hit_target(&mut hit_targets, &mut seen_hit_targets, t);
            if t == target_guid {
                total_damage = total_damage.saturating_add(hit.dealt);
                total_resisted = total_resisted.saturating_add(hit.resisted);
                total_absorbed = total_absorbed.saturating_add(hit.absorbed);
                total_healed = total_healed.saturating_add(hit.healed);
                if hit.energized > 0 {
                    total_energized = total_energized.saturating_add(hit.energized);
                    energized_power_type = hit.power_type;
                }
                any_crit |= hit.crit;
            }
        }
    }
    if !item_refused {
        if let Some(plan) = craft {
            if let Err(reason) = plan.commit(ctx) {
                log::info!("craft {} refused: {reason:?}", hdr.spell_id);
                item_refused = true;
            }
        }
    }
    if !item_refused {
        for _ in 0..item_grants {
            super::targeting::gain_recipe_skill(ctx, caster_guid, hdr.spell_id);
        }
    }
    if queues_next_swing(effects) {
        return;
    }
    for target in hit_targets {
        crate::creatures::eventai_on_spell_hit(
            ctx,
            caster_guid,
            target,
            hdr.spell_id,
            u32::from(hdr.school_mask),
        );
    }
    let school_index = crate::spell::school_index(hdr.school_mask);
    let (is_completion, client_initiated) = origin.cast_event_flags();
    crate::spell::SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            target_guid,
            is_completion,
            damage: total_damage,
            healed: total_healed,
            school: school_index,
            is_crit: any_crit,
            resisted: total_resisted,
            absorbed: total_absorbed,
            miss_info: cast_miss(primary_missed, hdr.school_mask),
            energized: total_energized,
            power_type: energized_power_type,
            cooldown_ms: hdr.cooldown_ms,
            client_initiated,
            ..SpellCastEvent::signal(ctx, caster_guid, hdr.spell_id, SpellCastEventKind::Go)
        },
    );
}

fn cast_miss(missed: bool, school_mask: u8) -> u8 {
    const SCHOOL_MASK_PHYSICAL: u8 = 0x01;
    match (missed, school_mask) {
        (false, _) => crate::spell::CAST_MISS_NONE,
        (true, 0 | SCHOOL_MASK_PHYSICAL) => crate::spell::CAST_MISS_MISS,
        (true, _) => crate::spell::CAST_MISS_RESIST,
    }
}

fn is_alive(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .is_some_and(|e| !e.dead)
}

fn triggered_target_admitted(
    origin: CastOrigin,
    caster_guid: u64,
    target_guid: u64,
    target_alive: bool,
) -> bool {
    target_alive || (origin.allows_dead_self() && target_guid == caster_guid)
}

pub(crate) fn cast_triggered(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,
) -> Result<(), String> {
    cast_triggered_with_origin(
        ctx,
        caster_guid,
        spell_id,
        level,
        target_guid,
        CastOrigin::Triggered,
    )
}

fn cast_triggered_with_origin(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,
    origin: CastOrigin,
) -> Result<(), String> {
    let hdr = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| format!("unknown spell {spell_id}"))?;
    let mut effects: Vec<SpellEffect> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    effects.sort_by_key(|e| (e.kind != E_INTERRUPT, e.effect_index));
    run_spell_effects(
        ctx,
        &hdr,
        &effects,
        caster_guid,
        target_guid,
        level,
        origin,
        None,
        None,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreatureSpellStartMode {
    Direct,
    Triggered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreatureSpellCasterAdmission {
    Living,
    DeadCreatureCallback,
}

#[spacetimedb::table(accessor = game_creature_dead_callback_cast_admission)]
pub struct CreatureDeadCallbackCastAdmission {
    #[primary_key]
    #[unique]
    pub caster_guid: u64,
    pub spell_id: u32,
}

fn dead_callback_cast_admitted(ctx: &ReducerContext, caster_guid: u64, spell_id: u32) -> bool {
    ctx.db
        .game_creature_dead_callback_cast_admission()
        .caster_guid()
        .find(caster_guid)
        .is_some_and(|admission| admission.spell_id == spell_id)
}

fn put_dead_callback_cast_admission(ctx: &ReducerContext, caster_guid: u64, spell_id: u32) {
    let admissions = ctx.db.game_creature_dead_callback_cast_admission();
    let admission = CreatureDeadCallbackCastAdmission {
        caster_guid,
        spell_id,
    };
    if admissions.caster_guid().find(caster_guid).is_some() {
        admissions.caster_guid().update(admission);
    } else {
        admissions.insert(admission);
    }
}

pub(crate) fn clear_dead_callback_cast_admission(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
) {
    let admissions = ctx.db.game_creature_dead_callback_cast_admission();
    if admissions
        .caster_guid()
        .find(caster_guid)
        .is_some_and(|admission| admission.spell_id == spell_id)
    {
        admissions.caster_guid().delete(caster_guid);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreatureSpellTarget {
    Unit(u64),
    None,
    CasterArea,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CreatureSpellStart {
    pub caster_guid: u64,
    pub caster_level: u8,
    pub spell_id: u32,
    pub mode: CreatureSpellStartMode,
    pub target: CreatureSpellTarget,
    pub interrupt_previous: bool,
    pub admission: CreatureSpellCasterAdmission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CreatureSpellRoute {
    mode: CreatureSpellStartMode,
    target_guid: u64,
    interrupt_previous: bool,
    admission: CreatureSpellCasterAdmission,
}

fn creature_spell_route(request: CreatureSpellStart) -> CreatureSpellRoute {
    CreatureSpellRoute {
        mode: request.mode,
        target_guid: match request.target {
            CreatureSpellTarget::Unit(guid) => guid,
            CreatureSpellTarget::None | CreatureSpellTarget::CasterArea => 0,
        },
        interrupt_previous: request.interrupt_previous,
        admission: request.admission,
    }
}

pub(crate) fn start_creature_spell(
    ctx: &ReducerContext,
    request: CreatureSpellStart,
) -> Result<(), String> {
    let route = creature_spell_route(request);
    if route.interrupt_previous {
        match route.mode {
            CreatureSpellStartMode::Direct => {
                let hdr = ctx
                    .db
                    .game_spell()
                    .spell_id()
                    .find(request.spell_id)
                    .ok_or_else(|| format!("unknown spell {}", request.spell_id))?;
                let caster = ctx
                    .db
                    .game_world_entity()
                    .guid()
                    .find(request.caster_guid)
                    .ok_or_else(|| format!("unknown caster {}", request.caster_guid))?;
                let mut effects: Vec<SpellEffect> = ctx
                    .db
                    .game_spell_effect()
                    .by_spell()
                    .filter(&request.spell_id)
                    .collect();
                effects.sort_by_key(|effect| (effect.kind != E_INTERRUPT, effect.effect_index));
                check_cast_gates_with_admission(
                    ctx,
                    &caster,
                    &hdr,
                    &effects,
                    route.target_guid,
                    request.spell_id,
                    request.caster_level,
                    route.admission == CreatureSpellCasterAdmission::DeadCreatureCallback,
                )?;
            }
            CreatureSpellStartMode::Triggered => {
                if ctx
                    .db
                    .game_spell()
                    .spell_id()
                    .find(request.spell_id)
                    .is_none()
                {
                    return Err(format!("unknown spell {}", request.spell_id));
                }
            }
        }
        crate::spell::break_channel(ctx, request.caster_guid);
    }
    match route.mode {
        CreatureSpellStartMode::Direct => begin_cast_with_admission(
            ctx,
            request.caster_guid,
            request.spell_id,
            request.caster_level,
            route.target_guid,
            false,
            None,
            route.admission,
        )
        .map(|_| ())
        .map_err(Into::into),
        CreatureSpellStartMode::Triggered => {
            let origin = match route.admission {
                CreatureSpellCasterAdmission::Living => CastOrigin::Triggered,
                CreatureSpellCasterAdmission::DeadCreatureCallback => {
                    CastOrigin::DeadCreatureCallbackTriggered
                }
            };
            cast_triggered_with_origin(
                ctx,
                request.caster_guid,
                request.spell_id,
                request.caster_level,
                route.target_guid,
                origin,
            )
        }
    }
}

fn check_cast_gates(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    hdr: &Spell,
    effects: &[SpellEffect],
    target_guid: u64,
    spell_id: u32,
    level: u8,
) -> Result<(), CastRefusal> {
    let allow_dead_creature = dead_callback_cast_admitted(ctx, caster.guid, spell_id);
    check_cast_gate_prefix(ctx, caster, effects, target_guid, allow_dead_creature)?;
    crate::mount::check_mount_cast(ctx, caster, effects, spell_id)?;
    check_cast_gate_suffix(ctx, caster, hdr, effects, target_guid, spell_id, level)
}

#[allow(clippy::too_many_arguments)]
fn check_cast_gates_with_admission(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    hdr: &Spell,
    effects: &[SpellEffect],
    target_guid: u64,
    spell_id: u32,
    level: u8,
    allow_dead_creature: bool,
) -> Result<(), CastRefusal> {
    check_cast_gate_prefix(ctx, caster, effects, target_guid, allow_dead_creature)?;
    crate::mount::check_mount_cast(ctx, caster, effects, spell_id)?;
    check_global_cooldown(ctx, caster.guid)?;
    check_cast_gate_suffix(ctx, caster, hdr, effects, target_guid, spell_id, level)
}

fn check_global_cooldown(ctx: &ReducerContext, caster_guid: u64) -> Result<(), CastRefusal> {
    if let Some(cd) = ctx.db.game_spell_cooldown().caster_guid().find(caster_guid) {
        if is_on_cooldown(
            ctx.timestamp.to_micros_since_unix_epoch(),
            cd.ready_at.to_micros_since_unix_epoch(),
        ) {
            return Err(CastRefusal::new(
                CastRefusalKind::Cooldown,
                "spell not ready (global cooldown)".to_string(),
            ));
        }
    }
    Ok(())
}

fn start_global_cooldown(ctx: &ReducerContext, caster_guid: u64, gcd_ms: u32) {
    if gcd_ms == 0 {
        return;
    }
    let ready_at = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros(i64::from(gcd_ms) * 1000))
        .unwrap_or(ctx.timestamp);
    let cooldowns = ctx.db.game_spell_cooldown();
    let cooldown = SpellCooldown {
        caster_guid,
        ready_at,
    };
    if cooldowns.caster_guid().find(caster_guid).is_some() {
        cooldowns.caster_guid().update(cooldown);
    } else {
        cooldowns.insert(cooldown);
    }
}

fn check_cast_gate_prefix(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
    allow_dead_creature: bool,
) -> Result<(), CastRefusal> {
    let caster_guid = caster.guid;

    if is_action_blocked(ctx, caster_guid) {
        return Err(CastRefusal::new(
            CastRefusalKind::CannotAct,
            format!("caster {caster_guid} cannot act (stun/poly/fear)"),
        ));
    }

    if !caster_life_allows(caster.is_player(), caster.dead, allow_dead_creature) {
        return Err(CastRefusal::new(
            CastRefusalKind::Dead,
            format!("caster {caster_guid} is dead"),
        ));
    }

    if effects.iter().any(|effect| effect.kind == E_TAME_CREATURE) {
        crate::creatures::validate_tame(ctx, caster, target_guid)?;
    }

    Ok(())
}

fn target_kind_requires_los(target: u8) -> bool {
    matches!(
        target,
        T_TARGET_ENEMY | T_TARGET_ALLY | T_TARGET_ANY | T_AREA_ENEMY | T_AREA_ALLY | T_CHAIN_ENEMY
    )
}

fn targeted_los_applies(
    caster_is_player: bool,
    caster_guid: u64,
    target_guid: u64,
    effects: &[SpellEffect],
) -> bool {
    caster_is_player
        && target_guid != 0
        && target_guid != caster_guid
        && effects
            .iter()
            .any(|effect| target_kind_requires_los(effect.target))
}

fn check_targeted_los(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
) -> Result<(), CastRefusal> {
    if !targeted_los_applies(caster.is_player(), caster.guid, target_guid, effects) {
        return Ok(());
    }
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return Ok(());
    };
    if (caster.map_id, caster.instance_id) != (target.map_id, target.instance_id) {
        return Ok(());
    }
    if !crate::nav::has_los(
        ctx,
        caster.map_id,
        caster.instance_id,
        (caster.x, caster.y, caster.z),
        (target.x, target.y, target.z),
    ) {
        return Err(CastRefusal::new(
            CastRefusalKind::NoLineOfSight,
            format!("target {target_guid} is not in line of sight"),
        ));
    }
    Ok(())
}

pub(crate) fn check_cast_start_gates(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell_id: u32,
    target_guid: u64,
    admission: CreatureSpellCasterAdmission,
) -> Result<(), CastRefusal> {
    let hdr = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| format!("unknown spell {spell_id}"))?;
    let mut effects: Vec<SpellEffect> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    effects.sort_by_key(|effect| (effect.kind != E_INTERRUPT, effect.effect_index));
    check_cast_gates_with_admission(
        ctx,
        caster,
        &hdr,
        &effects,
        target_guid,
        spell_id,
        caster.level as u8,
        admission == CreatureSpellCasterAdmission::DeadCreatureCallback,
    )?;
    check_targeted_los(ctx, caster, &effects, target_guid)
}

pub(crate) fn check_pending_cast_los(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), CastRefusal> {
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    let effects: Vec<SpellEffect> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    check_targeted_los(ctx, &caster, &effects, target_guid)
}

// Vanilla adds 2 yd combat reach and 8/3 yd moving-target tolerance.
const CAST_RANGE_LEEWAY_YD: f32 = 2.0 + 8.0 / 3.0;

fn check_distract_destination(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    dest: Option<(f32, f32, f32)>,
    range_yd: u32,
) -> Result<(), CastRefusal> {
    let caster_at = (caster.x, caster.y, caster.z);
    let point = check_ground_destination(caster_at, dest, range_yd)?;
    if !crate::nav::has_los(ctx, caster.map_id, caster.instance_id, caster_at, point) {
        return Err(CastRefusal::new(
            CastRefusalKind::NoLineOfSight,
            "ground point is not in line of sight".to_string(),
        ));
    }
    Ok(())
}

fn check_ground_destination(
    caster_at: (f32, f32, f32),
    dest: Option<(f32, f32, f32)>,
    range_yd: u32,
) -> Result<(f32, f32, f32), CastRefusal> {
    let Some((x, y, z)) = dest else {
        return Err(CastRefusal::new(
            CastRefusalKind::BadTargets,
            "spell can only target a ground point".to_string(),
        ));
    };
    let dist = distance_3d(caster_at.0, caster_at.1, caster_at.2, x, y, z);
    check_cast_range("ground point", dist, range_yd)?;
    Ok((x, y, z))
}

fn check_cast_range(what: &str, dist: f32, range_yd: u32) -> Result<(), CastRefusal> {
    if dist > range_yd as f32 + CAST_RANGE_LEEWAY_YD {
        return Err(CastRefusal::new(
            CastRefusalKind::OutOfRange,
            format!(
                "{what} out of range ({dist:.1} yd > {range_yd} + {CAST_RANGE_LEEWAY_YD:.1} yd leeway)"
            ),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn check_cast_gate_suffix(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    hdr: &Spell,
    effects: &[SpellEffect],
    target_guid: u64,
    spell_id: u32,
    level: u8,
) -> Result<(), CastRefusal> {
    let caster_guid = caster.guid;
    for effect in effects
        .iter()
        .filter(|effect| effect.kind == E_SUMMON_HOSTILE)
    {
        crate::creatures::check_hostile_summon(ctx, caster, hdr, effect)?;
    }

    if level_gate_blocks(caster.is_player(), hdr.spell_level, level) {
        return Err(CastRefusal::from(format!(
            "spell {spell_id} requires level {} (caster is level {level})",
            hdr.spell_level
        )));
    }

    if !stance_allows(hdr.stances, caster.stance) {
        return Err(CastRefusal::from(format!(
            "spell {spell_id} cannot be cast in the caster's current stance ({})",
            caster.stance
        )));
    }

    if let Some(lock) = ctx
        .db
        .game_school_lockout()
        .by_caster()
        .filter(&caster_guid)
        .find(|r| {
            r.school == hdr.school_mask
                && school_locked(
                    ctx.timestamp.to_micros_since_unix_epoch(),
                    r.until.to_micros_since_unix_epoch(),
                )
        })
    {
        let _ = lock;
        return Err(CastRefusal::from(format!(
            "spell {spell_id} school {} is locked (interrupted) for caster {caster_guid}",
            hdr.school_mask
        )));
    }

    if hdr.cooldown_ms > 0 {
        if let Some(cd) = ctx
            .db
            .game_spell_cd()
            .by_caster()
            .filter(&caster_guid)
            .find(|r| r.spell_id == spell_id)
        {
            if !is_ready_at(
                ctx.timestamp.to_micros_since_unix_epoch(),
                cd.ready_at.to_micros_since_unix_epoch(),
            ) {
                return Err(CastRefusal::new(
                    CastRefusalKind::Cooldown,
                    "spell not ready (cooldown)".to_string(),
                ));
            }
        }
    }

    if hdr.cast_flags & (SPELL_ATTR_REQ_OVERPOWER | SPELL_ATTR_REQ_REVENGE) != 0 {
        let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
        if hdr.cast_flags & SPELL_ATTR_REQ_OVERPOWER != 0
            && !crate::combat::react_window_active(caster.overpower_until_ms, now_ms)
        {
            return Err(CastRefusal::from(format!(
                "spell {spell_id} requires an Overpower window (the target must have dodged your swing)"
            )));
        }
        if hdr.cast_flags & SPELL_ATTR_REQ_REVENGE != 0
            && !crate::combat::react_window_active(caster.revenge_until_ms, now_ms)
        {
            return Err(CastRefusal::from(format!(
                "spell {spell_id} requires a Revenge window (you must have dodged/parried/blocked)"
            )));
        }
    }

    if hdr.range_yd > 0 && target_guid != caster_guid {
        if let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) {
            // World coordinates are meaningful only within the same map and instance.
            if caster.map_id != target.map_id || caster.instance_id != target.instance_id {
                return Err(CastRefusal::new(
                    CastRefusalKind::OtherPartition,
                    format!(
                        "target on another map (caster {} != target {})",
                        caster.map_id, target.map_id
                    ),
                ));
            }
            let dist = distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z);
            check_cast_range("target", dist, hdr.range_yd)?;
        }
    }

    // Target kinds determine faction eligibility; is_negative only describes aura polarity.
    if target_guid != caster_guid {
        // Inert DBC markers can name ally targets even when the spell is hostile.
        let hits_enemy = effects.iter().any(|e| {
            e.kind != A_FLAG
                && e.kind != E_DUEL
                && matches!(e.target, T_TARGET_ENEMY | T_AREA_ENEMY | T_CHAIN_ENEMY)
        });
        let hits_ally = effects.iter().any(|e| {
            e.kind != A_FLAG && e.kind != E_DUEL && matches!(e.target, T_TARGET_ALLY | T_AREA_ALLY)
        });
        if hits_enemy || hits_ally {
            if let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) {
                let friendly = !crate::combat::may_harm(ctx, caster, &target);
                let hostile = !crate::combat::may_help(ctx, caster, &target);
                if let Some(class) =
                    faction_target_violation(hits_enemy, hits_ally, friendly, hostile)
                {
                    return Err(CastRefusal::from(format!(
                        "spell {spell_id} targets {class} — cannot be cast on this unit ({target_guid})"
                    )));
                }
            }
        }
    }

    if let Some(e) = effects.iter().find(|e| {
        e.kind & KIND_AURA_BIT != 0 && e.p1 != 0 && has_aura(ctx, target_guid, e.p1 as u32)
    }) {
        return Err(CastRefusal::from(format!(
            "spell {spell_id} refused — target {target_guid} carries the linked debuff {} (Weakened-Soul-style lockout)",
            e.p1
        )));
    }

    if let Some(e) = effects
        .iter()
        .find(|e| e.kind == E_BLINK && e.radius_yd <= 0.0)
    {
        return Err(CastRefusal::from(format!(
            "spell {spell_id} (Blink) effect {} has no radius — teleport distance is unauthored data",
            e.effect_index
        )));
    }
    if let Some(e) = effects
        .iter()
        .find(|e| e.kind == E_PERSISTENT_AREA && e.radius_yd <= 0.0)
    {
        return Err(CastRefusal::from(format!(
            "spell {spell_id} (ground-AoE) effect {} has no radius — area size is unauthored data",
            e.effect_index
        )));
    }

    if let Some(e) = effects.iter().find(|e| {
        e.kind == E_SCRIPTED
            && e.script_id != 0
            && !ctx
                .db
                .game_script()
                .script_id()
                .find(e.script_id)
                .is_some_and(|script| script.enabled)
    }) {
        return Err(CastRefusal::from(format!(
            "spell {spell_id} effect {} names script {} which is missing or disabled",
            e.effect_index, e.script_id
        )));
    }

    if target_guid != caster_guid {
        if hdr.cast_flags & SPELL_ATTR_REQ_BEHIND != 0 {
            let target = crate::helpers::live_entity(ctx, target_guid)
                .map_err(|_| format!("spell {spell_id} target {target_guid} not in world"))?;
            if !is_behind(caster.x, caster.y, target.x, target.y, target.orientation) {
                return Err(CastRefusal::from(format!(
                    "spell {spell_id} must be cast from BEHIND the target ({target_guid})"
                )));
            }
        }
        // Check stealth before cast acceptance breaks it.
        if hdr.cast_flags & SPELL_ATTR_REQ_STEALTH != 0 && !is_stealthed(ctx, caster_guid) {
            return Err(CastRefusal::from(format!(
                "spell {spell_id} requires stealth (caster {caster_guid} is not stealthed)"
            )));
        }
        if hdr.cast_flags & SPELL_ATTR_INCAP_OPENER != 0 {
            if let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) {
                if target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                    return Err(CastRefusal::from(format!(
                        "spell {spell_id} cannot target a unit IN COMBAT ({target_guid})"
                    )));
                }
                let is_humanoid = target.is_player()
                    || ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(target.entry)
                        .is_some_and(|t| t.creature_type == HUMANOID_TYPE);
                if !is_humanoid {
                    return Err(CastRefusal::from(format!(
                        "spell {spell_id} can only target a Humanoid ({target_guid})"
                    )));
                }
            }
        }
    }

    if hdr.cast_flags & SPELL_ATTR_REQ_DAGGER != 0 {
        let has_dagger =
            crate::items::item_in_slot(ctx, caster_guid, crate::items::equip_slot::MAINHAND)
                .and_then(|inst| {
                    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
                    if crate::items::item_is_broken(&tmpl, &inst) {
                        return None;
                    }
                    Some(
                        tmpl.class == lyracore_shared::constants::starter_item::CLASS_WEAPON
                            && tmpl.subclass == crate::items::weapon_subclass::DAGGER,
                    )
                })
                .unwrap_or(false);
        if !has_dagger {
            return Err(CastRefusal::from(format!(
                "spell {spell_id} requires a dagger equipped in the main hand (caster {caster_guid})"
            )));
        }
    }

    // Creatures have no managed power pool, so only Characters pay spell costs.
    if cost_gate_blocks(caster.is_player(), caster.power, hdr.cost) {
        return Err(CastRefusal::new(
            CastRefusalKind::InsufficientPower,
            format!("not enough power: have {}, need {}", caster.power, hdr.cost),
        ));
    }

    Ok(())
}

fn caster_life_allows(is_player: bool, dead: bool, dead_callback: bool) -> bool {
    !dead || (dead_callback && !is_player)
}

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn resolve_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
) -> Result<(), String> {
    resolve_cast_at(
        ctx,
        caster_guid,
        spell_id,
        level,
        caster_guid,
        false,
        false,
        None,
    )
}

pub(crate) fn begin_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,

    client_initiated: bool,
    dest: Option<(f32, f32, f32)>,
) -> Result<(), String> {
    begin_cast_with_admission(
        ctx,
        caster_guid,
        spell_id,
        level,
        target_guid,
        client_initiated,
        dest,
        CreatureSpellCasterAdmission::Living,
    )
    .map(|_| ())
    .map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_cast_with_admission(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,
    client_initiated: bool,
    dest: Option<(f32, f32, f32)>,
    admission: CreatureSpellCasterAdmission,
) -> Result<CastStart, CastRefusal> {
    let hdr = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| format!("unknown spell {spell_id}"))?;

    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    let mut effects: Vec<SpellEffect> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    effects.sort_by_key(|effect| (effect.kind != E_INTERRUPT, effect.effect_index));
    check_cast_gates_with_admission(
        ctx,
        &caster,
        &hdr,
        &effects,
        target_guid,
        spell_id,
        level,
        admission == CreatureSpellCasterAdmission::DeadCreatureCallback,
    )?;
    check_targeted_los(ctx, &caster, &effects, target_guid)?;

    // Break the previous channel before a new channel can place its aura.
    break_channel(ctx, caster_guid);
    if admission == CreatureSpellCasterAdmission::DeadCreatureCallback {
        put_dead_callback_cast_admission(ctx, caster_guid, spell_id);
    }

    // Channels resolve immediately even when their imported cast time is nonzero.
    let (ct_flat, ct_pct) =
        crate::spell::math::spell_mod(ctx, caster_guid, SPELLMOD_OP_CASTING_TIME, &hdr);
    let cast_ms = crate::spell::math::apply_spell_mod(hdr.cast_time_ms, ct_flat, ct_pct);

    let completed_channel = hdr.cast_flags & SPELL_ATTR_CHANNELED != 0
        && ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(&spell_id)
            .any(|effect| effect.kind == E_TAME_CREATURE);
    if !completed_channel && (cast_ms == 0 || hdr.cast_flags & SPELL_ATTR_CHANNELED != 0) {
        let result = resolve_cast_at_typed(
            ctx,
            caster_guid,
            spell_id,
            level,
            target_guid,
            false,
            client_initiated,
            dest,
        );
        clear_dead_callback_cast_admission(ctx, caster_guid, spell_id);
        return result.map(|_| CastStart::Resolved);
    }

    let completion_ms = if completed_channel {
        hdr.duration_ms.max(1)
    } else {
        cast_ms
    };
    let fire_at = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros((completion_ms as i64) * 1000))
        .unwrap_or(ctx.timestamp);
    let pending = ctx.db.game_pending_cast();
    let stale: Vec<u64> = pending
        .by_caster()
        .filter(&caster_guid)
        .map(|p| p.scheduled_id)
        .collect();
    for id in stale {
        pending.scheduled_id().delete(id);
    }
    let (has_dest, (dx, dy, dz)) = match dest {
        Some(p) => (true, p),
        None => (false, (0.0, 0.0, 0.0)),
    };
    let cast = pending.insert(PendingCast {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(fire_at),
        caster_guid,
        spell_id,
        level,
        target_guid,
        pushback_count: 0,
        has_dest,
        dest_x: dx,
        dest_y: dy,
        dest_z: dz,
    });
    start_global_cooldown(ctx, caster_guid, hdr.gcd_ms);
    crate::spell::SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            target_guid,
            cast_time_ms: completion_ms,
            ..SpellCastEvent::signal(ctx, caster_guid, spell_id, SpellCastEventKind::Start)
        },
    );
    Ok(CastStart::Started(cast.into()))
}

#[cfg(test)]
mod ground_destination_tests {
    use super::*;

    const CASTER: (f32, f32, f32) = (0.0, 0.0, 0.0);

    #[test]
    fn a_distract_without_a_ground_point_is_a_bad_target() {
        let refusal = check_ground_destination(CASTER, None, 30).unwrap_err();
        assert_eq!(refusal.kind, CastRefusalKind::BadTargets);
        assert!(
            refusal.message.contains("only target a ground point"),
            "{refusal}"
        );
    }

    #[test]
    fn a_ground_point_inside_the_range_passes() {
        assert!(check_ground_destination(CASTER, Some((30.0, 0.0, 0.0)), 30).is_ok());
    }

    #[test]
    fn a_ground_point_inside_the_leeway_passes() {
        assert!(check_ground_destination(CASTER, Some((34.6, 0.0, 0.0)), 30).is_ok());
    }

    #[test]
    fn a_ground_point_beyond_the_leeway_is_out_of_range() {
        let refusal = check_ground_destination(CASTER, Some((34.7, 0.0, 0.0)), 30).unwrap_err();
        assert_eq!(refusal.kind, CastRefusalKind::OutOfRange);
        assert!(refusal.message.contains("out of range"), "{refusal}");
    }
}

#[cfg(test)]
mod companion_los_tests {
    use super::*;

    #[test]
    fn los_target_kinds_exclude_self_and_scripted_spells() {
        for target in [
            T_TARGET_ENEMY,
            T_TARGET_ALLY,
            T_TARGET_ANY,
            T_AREA_ENEMY,
            T_AREA_ALLY,
            T_CHAIN_ENEMY,
        ] {
            assert!(target_kind_requires_los(target));
        }
        assert!(!target_kind_requires_los(T_SELF));
        assert!(!target_kind_requires_los(T_SCRIPTED));
    }
}

pub(crate) fn passive_applies_effect_kind(kind: u8) -> bool {
    kind & KIND_AURA_BIT != 0
}

pub(crate) fn apply_spell_auras(
    ctx: &ReducerContext,
    spell_id: u32,
    target_guid: u64,
    level: u8,
    rank: u8,
) {
    let Some(mut hdr) = ctx.db.game_spell().spell_id().find(spell_id) else {
        log::info!("apply_spell_auras: unknown spell {spell_id}");
        return;
    };
    // Passive duration zero means permanent; u32::MAX is the infinite-duration sentinel.
    if hdr.duration_ms == 0 {
        hdr.duration_ms = u32::MAX;
    }
    let effects: Vec<SpellEffect> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .filter(|e| passive_applies_effect_kind(e.kind))
        .collect();
    if effects.is_empty() {
        log::info!(
            "apply_spell_auras: spell {spell_id} has no aura-kind effects — nothing applied"
        );
        return;
    }
    for e in effects {
        let points = (rank as i32) * e.base_points;
        aura_apply(
            ctx,
            &e,
            &hdr,
            target_guid,
            target_guid,
            target_guid,
            level,
            points,
        );
    }
}

#[cfg(test)]
mod eventai_hit_tests {
    use super::*;

    fn effect(kind: u8) -> SpellEffect {
        SpellEffect {
            id: 1,
            spell_id: 1,
            effect_index: 0,
            kind,
            base_points: 0,
            die_sides: 0,
            per_level: 0.0,
            period_ms: 0,
            target: 0,
            radius_yd: 0.0,
            chain_targets: 0,
            trigger_spell: 0,
            effect_mechanic: 0,
            p0: 0,
            p0_kind: 0,
            p1: 0,
            script_id: 0,
            enters_combat: false,
        }
    }

    #[test]
    fn resolved_unit_targets_are_unique_and_keep_first_seen_order() {
        let mut ordered = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for guid in [20, 10, 20, 0, 30, 10] {
            remember_hit_target(&mut ordered, &mut seen, guid);
        }

        assert_eq!(ordered, vec![20, 10, 30]);
    }

    #[test]
    fn a_next_swing_effect_parks_instead_of_dispatching_at_queue_time() {
        assert!(queues_next_swing(&[effect(E_NEXT_SWING)]));
        assert!(!queues_next_swing(&[effect(E_DAMAGE)]));
        assert!(!queues_next_swing(&[]));
    }

    #[test]
    fn creature_spell_routes_keep_mode_target_shape_and_interrupt_intent() {
        let direct = creature_spell_route(CreatureSpellStart {
            caster_guid: 10,
            caster_level: 5,
            spell_id: 20,
            mode: CreatureSpellStartMode::Direct,
            target: CreatureSpellTarget::Unit(30),
            interrupt_previous: true,
            admission: CreatureSpellCasterAdmission::Living,
        });
        let triggered = creature_spell_route(CreatureSpellStart {
            caster_guid: 10,
            caster_level: 5,
            spell_id: 20,
            mode: CreatureSpellStartMode::Triggered,
            target: CreatureSpellTarget::CasterArea,
            interrupt_previous: false,
            admission: CreatureSpellCasterAdmission::DeadCreatureCallback,
        });

        assert_eq!(
            direct,
            CreatureSpellRoute {
                mode: CreatureSpellStartMode::Direct,
                target_guid: 30,
                interrupt_previous: true,
                admission: CreatureSpellCasterAdmission::Living,
            }
        );
        assert_eq!(
            triggered,
            CreatureSpellRoute {
                mode: CreatureSpellStartMode::Triggered,
                target_guid: 0,
                interrupt_previous: false,
                admission: CreatureSpellCasterAdmission::DeadCreatureCallback,
            }
        );
    }

    #[test]
    fn only_a_dead_creature_callback_cast_can_apply_to_its_dead_caster() {
        assert!(!triggered_target_admitted(
            CastOrigin::Triggered,
            10,
            10,
            false,
        ));
        assert!(triggered_target_admitted(
            CastOrigin::DeadCreatureCallbackTriggered,
            10,
            10,
            false,
        ));
        assert!(!triggered_target_admitted(
            CastOrigin::DeadCreatureCallbackTriggered,
            10,
            20,
            false,
        ));
    }

    #[test]
    fn dead_callback_admission_is_limited_to_dead_creatures() {
        assert!(caster_life_allows(false, true, true));
        assert!(!caster_life_allows(false, true, false));
        assert!(!caster_life_allows(true, true, true));
        assert!(caster_life_allows(true, false, false));
    }
}

#[cfg(test)]
mod cast_miss_tests {
    use super::*;

    #[test]
    fn a_failed_hit_roll_misses_a_physical_spell_and_resists_a_magic_one() {
        // School masks: 0 none, 1 physical, 4 fire, 32 shadow.
        assert_eq!(cast_miss(false, 4), crate::spell::CAST_MISS_NONE);
        assert_eq!(cast_miss(true, 0), crate::spell::CAST_MISS_MISS);
        assert_eq!(cast_miss(true, 1), crate::spell::CAST_MISS_MISS);
        assert_eq!(cast_miss(true, 4), crate::spell::CAST_MISS_RESIST);
        assert_eq!(cast_miss(true, 32), crate::spell::CAST_MISS_RESIST);
    }
}
