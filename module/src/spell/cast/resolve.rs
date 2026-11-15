use lyracore_shared::spell::taxonomy::CAST_FLAG_RANGED_AUTO_REPEAT;
use lyracore_shared::spell_custom as custom;
use spacetimedb::{log, ReducerContext, Table, TimeDuration};

use crate::combat::AttackOutcome;
use crate::geometry::{distance_3d, is_behind, is_facing};
use crate::{game_creature_template, game_world_entity, WorldEntity};

use crate::{game_item_instance, game_item_template};

use crate::spell::{
    apply_effect, aura_apply, break_channel, break_stealth, cost_gate_blocks, effect_amount,
    faction_target_violation, has_aura, is_action_blocked, is_stealthed, level_gate_blocks,
    recipe_reagents, select_targets, stance_allows, CastGateRefusal, CastHandle, CastRefusalKind,
    CastRefusalReason, CastStart, ItemGrantOutcome, SpellCastEventKind, A_FLAG, A_MOUNTED,
    CAST_FLAG_BINARY, CAST_FLAG_CHANNELED, CAST_FLAG_COMBO_FINISHER, CAST_FLAG_INCAP_OPENER,
    CAST_FLAG_MAGIC_HIT, CAST_FLAG_REQ_BEHIND, CAST_FLAG_REQ_DAGGER, CAST_FLAG_REQ_OVERPOWER,
    CAST_FLAG_REQ_REVENGE, CAST_FLAG_REQ_STEALTH, CAST_FLAG_STEALTH_SAFE, CAST_FLAG_WEAPON_PROCS,
    E_BLINK, E_CREATE_ITEM, E_DAMAGE, E_DISTRACT, E_DUEL, E_ENCHANT_ITEM_TEMPORARY,
    E_FINISHER_DAMAGE, E_INTERRUPT, E_NEXT_SWING, E_PERSISTENT_AREA, E_SCRIPTED,
    E_SUMMON_GUARDIANS, E_SUMMON_HOSTILE, E_TAME_CREATURE, E_WEAPON_STRIKE, HUMANOID_TYPE,
    KIND_AURA_BIT, RESIST_ARMOR, SPELLMOD_OP_CASTING_TIME, SPELL_ATTR_PASSIVE,
    SPELL_ATTR_USES_RANGED_SLOT, T_AREA_ALLY, T_AREA_ENEMY, T_CHAIN_ENEMY, T_TARGET_ALLY,
    T_TARGET_ANY, T_TARGET_ENEMY,
};
use crate::{
    game_aura, game_pending_cast, game_school_lockout, game_spell, game_spell_cd,
    game_spell_cooldown, game_spell_effect, PendingCast, Spell, SpellCastEvent, SpellCd,
    SpellCooldown, SpellEffect,
};

use crate::script_binding::game_script;

/// One cast as its caller asked for it. A timed cast stores it as a [`PendingCast`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CastRequest {
    pub caster_guid: u64,
    pub spell_id: u32,
    pub level: u8,
    pub target_guid: u64,
    pub client_initiated: bool,
    pub dest: Option<(f32, f32, f32)>,
    pub item_target: Option<u64>,
    pub cooldown_category: Option<u32>,
}

impl CastRequest {
    pub(crate) fn new(caster_guid: u64, spell_id: u32, level: u8, target_guid: u64) -> Self {
        Self {
            caster_guid,
            spell_id,
            level,
            target_guid,
            client_initiated: false,
            dest: None,
            item_target: None,
            cooldown_category: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastPhase {
    Start,
    Completion,
}

/// A `Spell` row and its effects in cast order.
pub(crate) struct LoadedSpell {
    pub header: Spell,
    pub effects: Vec<SpellEffect>,
}

impl LoadedSpell {
    pub(crate) fn load(ctx: &ReducerContext, spell_id: u32) -> Result<Self, String> {
        let header = ctx
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
        // Interrupts must read the pending cast school before damage effects can clear the cast.
        effects.sort_by_key(|e| (e.kind != E_INTERRUPT, e.effect_index));
        Ok(Self { header, effects })
    }
}

/// Resolves a cast now, skipping its cast time.
pub(crate) fn resolve_cast(
    ctx: &ReducerContext,
    request: &CastRequest,
) -> Result<(), CastGateRefusal> {
    let spell = LoadedSpell::load(ctx, request.spell_id)?;
    resolve_loaded_cast(ctx, request, &spell, CastPhase::Start)
}

/// Resolves a timed cast that came due.
pub(crate) fn complete_cast(
    ctx: &ReducerContext,
    request: &CastRequest,
) -> Result<(), CastGateRefusal> {
    // A caster gone at completion refuses with the live-entity message, before the spell lookup.
    crate::helpers::live_entity(ctx, request.caster_guid)?;
    let spell = LoadedSpell::load(ctx, request.spell_id)?;
    resolve_loaded_cast(ctx, request, &spell, CastPhase::Completion)
}

fn resolve_loaded_cast(
    ctx: &ReducerContext,
    request: &CastRequest,
    spell: &LoadedSpell,
    phase: CastPhase,
) -> Result<(), CastGateRefusal> {
    let CastRequest {
        caster_guid,
        spell_id,
        target_guid,
        ..
    } = *request;
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());

    let mut caster = crate::helpers::live_entity(ctx, caster_guid)
        .map_err(|_| format!("caster {caster_guid} not in world"))?;

    check_cast_gates(ctx, &caster, spell, request, GateRun::Resolve(phase))?;

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
    if caster.is_player() && hdr.cost > 0 && !queues_next_swing(effects) {
        charge_spell_cost(ctx, &mut caster, hdr);
        caster.update(ctx);
    }

    // Dismount after charging power so the stale caster cannot overwrite the Mount Projection.
    if !effects.iter().any(|e| e.kind == A_MOUNTED) {
        crate::mount::dismount(ctx, caster_guid);
    }

    // Break stealth before effects so recasting Stealth can reapply its aura.
    if hdr.cast_flags & CAST_FLAG_STEALTH_SAFE == 0 {
        break_stealth(ctx, caster_guid);
    }

    run_spell_effects(ctx, spell, request, CastOrigin::Direct(phase), craft);

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
    // Spend after the effects so the last thrown weapon still strikes with its own damage.
    if caster_is_player && is_ranged_weapon_strike(hdr, effects) {
        crate::combat::spend_ranged_ammo(ctx, caster_guid);
    }
    if caster_is_player && cast_hits_enemy {
        crate::spell::effects::arm_spell_retaliation(ctx, caster_guid, target_guid);
    }

    if phase == CastPhase::Start {
        start_global_cooldown(ctx, caster_guid, hdr.gcd_ms);
    }

    // A queued strike starts its cooldown when its swing fires.
    if !queues_next_swing(effects) {
        start_spell_cooldown(ctx, caster_guid, spell_id, hdr.cooldown_ms);
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
    Direct(CastPhase),
    Triggered,
    DeadCreatureCallbackTriggered,
    ChanceOnHit,
}

impl CastOrigin {
    fn is_triggered(self) -> bool {
        matches!(
            self,
            Self::Triggered | Self::DeadCreatureCallbackTriggered | Self::ChanceOnHit
        )
    }

    fn allows_dead_self(self) -> bool {
        matches!(self, Self::DeadCreatureCallbackTriggered)
    }

    fn is_completion(self) -> bool {
        matches!(self, Self::Direct(CastPhase::Completion))
    }
}

pub(crate) fn charge_spell_cost(ctx: &ReducerContext, caster: &mut WorldEntity, hdr: &Spell) {
    caster.power = caster.power.saturating_sub(hdr.cost);
    if hdr.cost > 0 && hdr.power_type == lyracore_shared::packing::power_type::MANA {
        let now_ms = crate::time::now_ms(ctx);
        caster.mana_regen_paused_until_ms = now_ms + 5000;
    }
}

pub(crate) fn start_spell_cooldown(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    cooldown_ms: u32,
) {
    if cooldown_ms != 0 {
        record_spell_cooldown(ctx, caster_guid, spell_id, cooldown_ms);
    }
}

pub(crate) fn record_spell_cooldown(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    cooldown_ms: u32,
) {
    let ready_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros((cooldown_ms as i64) * 1000))
        .unwrap_or(crate::time::now(ctx));
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
            category: 0,
            category_ready_micros: 0,
        });
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

fn run_spell_effects(
    ctx: &ReducerContext,
    spell: &LoadedSpell,
    request: &CastRequest,
    origin: CastOrigin,
    mut craft: Option<crate::items::ItemStoragePlan>,
) {
    let (caster_guid, target_guid) = (request.caster_guid, request.target_guid);
    let (level, dest) = (request.level, request.dest);
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    let mut hit_targets = Vec::new();
    let mut seen_hit_targets = std::collections::HashSet::new();
    let triggered = origin.is_triggered();
    let mut total_damage: u32 = 0;
    let mut total_healed: u32 = 0;
    let mut total_resisted: u32 = 0;
    let mut total_absorbed: u32 = 0;
    let mut total_energized: u32 = 0;
    let mut energized_power_type: u8 = 0;
    let mut primary_avoided = AttackOutcome::Normal;
    let mut any_crit = false;
    let mut item_grants = 0;
    let mut item_refused = false;
    // One hit roll per target for the whole spell (CONTEXT.md "Cast Miss"); never the caster.
    let table = if effects
        .iter()
        .any(|e| matches!(e.kind, E_WEAPON_STRIKE | E_FINISHER_DAMAGE))
    {
        HitTable::Weapon {
            ranged: hdr.attributes & SPELL_ATTR_USES_RANGED_SLOT != 0,
        }
    } else {
        HitTable::Spell
    };
    let weapon_strike = matches!(table, HitTable::Weapon { .. });
    let rolls_hit = weapon_strike
        || effects.iter().any(|e| e.kind == E_DAMAGE)
        || (hdr.negative() && hdr.cast_flags & (CAST_FLAG_BINARY | CAST_FLAG_MAGIC_HIT) != 0);
    let mut target_rolls: std::collections::HashMap<u64, AttackOutcome> =
        std::collections::HashMap::new();
    // vmangos rolls the main hand per target, landed or avoided. LyraCore skips a Chance on Hit cast.
    let rolls_weapon =
        hdr.cast_flags & CAST_FLAG_WEAPON_PROCS != 0 && !matches!(origin, CastOrigin::ChanceOnHit);
    let mut weapon_targets = Vec::new();
    let mut seen_weapon_targets = std::collections::HashSet::new();
    for e in effects {
        let draw = crate::conformance_rolls::draw(
            ctx,
            crate::conformance_rolls::RollStream::Caster(caster_guid),
        );
        let points = effect_amount(
            e.base_points,
            e.die_sides,
            e.per_level,
            level as u32,
            hdr.spell_level,
            hdr.max_level,
            draw,
        );
        for t in select_targets(ctx, caster_guid, target_guid, e, dest) {
            if triggered && !triggered_target_admitted(origin, caster_guid, t, is_alive(ctx, t)) {
                continue;
            }
            let roll = if rolls_hit && t != caster_guid {
                *target_rolls
                    .entry(t)
                    .or_insert_with(|| hit_roll(ctx, caster_guid, level, t, table, hdr))
            } else {
                AttackOutcome::Normal
            };
            if rolls_weapon && t != caster_guid {
                remember_hit_target(&mut weapon_targets, &mut seen_weapon_targets, t);
            }
            if roll.is_avoided() {
                if t == target_guid {
                    primary_avoided = roll;
                }
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
                let points = crate::combo::effect_points(ctx, hdr, e, caster_guid, t, points);
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
                    roll == AttackOutcome::Crit,
                    request.item_target,
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
    // Spend after every effect has read the points; vmangos keeps them when a finisher misses.
    if hdr.cast_flags & CAST_FLAG_COMBO_FINISHER != 0 && !primary_avoided.is_avoided() {
        crate::combo::spend_combo(ctx, caster_guid);
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
    crate::SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            target_guid,
            is_completion: origin.is_completion(),
            damage: total_damage,
            healed: total_healed,
            school: school_index,
            is_crit: any_crit,
            resisted: total_resisted,
            absorbed: total_absorbed,
            miss_info: cast_miss(
                primary_avoided,
                weapon_strike,
                hdr.cast_flags & CAST_FLAG_MAGIC_HIT != 0 || hdr.school_mask & !RESIST_ARMOR != 0,
            ),
            energized: total_energized,
            power_type: energized_power_type,
            cooldown_ms: hdr.cooldown_ms,
            client_initiated: request.client_initiated,
            ..SpellCastEvent::signal(ctx, caster_guid, hdr.spell_id, SpellCastEventKind::Go)
        },
    );
    for target in weapon_targets {
        crate::spell::chance_on_hit::roll_chance_on_hit(
            ctx,
            caster_guid,
            target,
            crate::items::equip_slot::MAINHAND,
        );
    }
}

#[derive(Clone, Copy)]
enum HitTable {
    Spell,
    Weapon { ranged: bool },
}

fn hit_roll(
    ctx: &ReducerContext,
    caster_guid: u64,
    level: u8,
    target_guid: u64,
    table: HitTable,
    spell: &Spell,
) -> AttackOutcome {
    let HitTable::Weapon { ranged } = table else {
        return if super::targeting::spell_misses(ctx, level, target_guid, spell) {
            AttackOutcome::Miss
        } else {
            AttackOutcome::Normal
        };
    };
    let entities = ctx.db.game_world_entity();
    match (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) {
        (Some(caster), Some(target)) => {
            let attack_type = if ranged {
                crate::combat::AttackType::Ranged
            } else {
                crate::combat::AttackType::Melee
            };
            let (roll, _) = crate::combat::roll_attack(
                ctx,
                &caster,
                &target,
                attack_type,
                Some(crate::combat::DeliveredSpell::cast(spell)),
            );
            arm_avoidance_reacts(ctx, caster_guid, target_guid, roll);
            roll
        }
        _ => AttackOutcome::Normal,
    }
}

fn arm_avoidance_reacts(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    roll: AttackOutcome,
) {
    use crate::combat::{arm_react, ReactKind};
    match roll {
        AttackOutcome::Dodge => {
            arm_react(ctx, caster_guid, ReactKind::Overpower);
            arm_react(ctx, target_guid, ReactKind::Revenge);
        }
        AttackOutcome::Parry => arm_react(ctx, target_guid, ReactKind::Revenge),
        AttackOutcome::Normal
        | AttackOutcome::Crit
        | AttackOutcome::Miss
        | AttackOutcome::Glancing
        | AttackOutcome::Crushing
        | AttackOutcome::Block => {}
    }
}

fn cast_miss(roll: AttackOutcome, weapon_strike: bool, magic_hit: bool) -> u8 {
    match roll {
        AttackOutcome::Dodge => crate::spell::CAST_MISS_DODGE,
        AttackOutcome::Parry => crate::spell::CAST_MISS_PARRY,
        AttackOutcome::Miss if weapon_strike || !magic_hit => crate::spell::CAST_MISS_MISS,
        AttackOutcome::Miss => crate::spell::CAST_MISS_RESIST,
        AttackOutcome::Normal
        | AttackOutcome::Crit
        | AttackOutcome::Glancing
        | AttackOutcome::Crushing
        | AttackOutcome::Block => crate::spell::CAST_MISS_NONE,
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
    let spell = LoadedSpell::load(ctx, spell_id)?;
    cast_triggered_with_origin(
        ctx,
        &CastRequest::new(caster_guid, spell_id, level, target_guid),
        &spell,
        CastOrigin::Triggered,
    )
}

pub(crate) fn cast_chance_on_hit(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    level: u8,
    target_guid: u64,
) -> Result<(), String> {
    let spell = LoadedSpell::load(ctx, spell_id)?;
    cast_triggered_with_origin(
        ctx,
        &CastRequest::new(caster_guid, spell_id, level, target_guid),
        &spell,
        CastOrigin::ChanceOnHit,
    )
}

fn cast_triggered_with_origin(
    ctx: &ReducerContext,
    request: &CastRequest,
    spell: &LoadedSpell,
    origin: CastOrigin,
) -> Result<(), String> {
    let caster_guid = request.caster_guid;
    let caster = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .ok_or_else(|| format!("caster {caster_guid} not in world"))?;
    check_unit_target(
        ctx,
        &caster,
        &spell.effects,
        request.target_guid,
        request.spell_id,
    )
    .map_err(String::from)?;
    run_spell_effects(ctx, spell, request, origin, None);
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
    let spell = LoadedSpell::load(ctx, request.spell_id)?;
    let cast = CastRequest::new(
        request.caster_guid,
        request.spell_id,
        request.caster_level,
        route.target_guid,
    );
    if route.interrupt_previous {
        if route.mode == CreatureSpellStartMode::Direct {
            let caster = ctx
                .db
                .game_world_entity()
                .guid()
                .find(request.caster_guid)
                .ok_or_else(|| format!("unknown caster {}", request.caster_guid))?;
            check_cast_gates(
                ctx,
                &caster,
                &spell,
                &cast,
                GateRun::InterruptPrecheck(route.admission),
            )?;
        }
        crate::spell::break_channel(ctx, request.caster_guid);
    }
    match route.mode {
        CreatureSpellStartMode::Direct => begin_loaded_cast(ctx, &cast, &spell, route.admission)
            .map(|_| ())
            .map_err(Into::into),
        CreatureSpellStartMode::Triggered => {
            let origin = match route.admission {
                CreatureSpellCasterAdmission::Living => CastOrigin::Triggered,
                CreatureSpellCasterAdmission::DeadCreatureCallback => {
                    CastOrigin::DeadCreatureCallbackTriggered
                }
            };
            cast_triggered_with_origin(ctx, &cast, &spell, origin)
        }
    }
}

#[derive(Clone, Copy)]
enum GateRun {
    /// A creature's interrupting cast, checked before it breaks the caster's channel.
    InterruptPrecheck(CreatureSpellCasterAdmission),
    /// A bot's cast readiness.
    Readiness(CreatureSpellCasterAdmission),
    Start(CreatureSpellCasterAdmission),
    Resolve(CastPhase),
}

// Gate order decides which Refusal the client sees when two Gates would both refuse.
fn check_cast_gates(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    request: &CastRequest,
    run: GateRun,
) -> Result<(), CastGateRefusal> {
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    let (spell_id, target_guid) = (request.spell_id, request.target_guid);
    match run {
        GateRun::InterruptPrecheck(admission)
        | GateRun::Readiness(admission)
        | GateRun::Start(admission) => {
            let allow_dead_creature =
                admission == CreatureSpellCasterAdmission::DeadCreatureCallback;
            check_cast_gate_prefix(ctx, caster, effects, target_guid, allow_dead_creature)?;
            crate::mount::check_mount_cast(ctx, caster, effects, spell_id)?;
            check_source_cast_position(ctx, caster, hdr, target_guid)?;
            check_global_cooldown(ctx, caster.guid)?;
            check_cast_gate_suffix(ctx, caster, spell, request)?;
            if !matches!(run, GateRun::InterruptPrecheck(_)) {
                check_targeted_los(ctx, caster, effects, target_guid)?;
            }
            if matches!(run, GateRun::Start(_)) {
                check_item_target(ctx, caster.guid, effects, request.item_target)?;
            }
        }
        GateRun::Resolve(phase) => {
            let allow_dead_creature = dead_callback_cast_admitted(ctx, caster.guid, spell_id);
            if phase == CastPhase::Completion {
                check_targeted_los(ctx, caster, effects, target_guid)?;
            }
            check_cast_gate_prefix(ctx, caster, effects, target_guid, allow_dead_creature)?;
            crate::mount::check_mount_cast(ctx, caster, effects, spell_id)?;
            check_cast_gate_suffix(ctx, caster, spell, request)?;
            check_item_target(ctx, caster.guid, effects, request.item_target)?;
            check_source_cast_position(ctx, caster, hdr, target_guid)?;
            if phase == CastPhase::Start {
                check_global_cooldown(ctx, caster.guid)?;
            }
            if effects.iter().any(|e| e.kind == E_DISTRACT) {
                check_distract_destination(ctx, caster, request.dest, hdr.range_yd)?;
            }
        }
    }
    Ok(())
}

fn check_unit_target(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
    spell_id: u32,
) -> Result<(), CastGateRefusal> {
    if target_guid != caster.guid
        && effects.iter().any(|effect| targets_unit(effect.target))
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(target_guid)
            .is_some_and(|target| target.is_flying())
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::BadTargets,
            "a taxi passenger cannot be a spell target",
        ));
    }
    // Target kinds determine faction eligibility; is_negative only describes aura polarity.
    if target_guid != caster.guid {
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
                    return Err(CastGateRefusal::other(CastRefusalReason::BadTargets, format!(
                        "spell {spell_id} targets {class}, so it cannot be cast on unit {target_guid}"
                    )));
                }
            }
        }
    }

    Ok(())
}

fn check_source_cast_position(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    hdr: &Spell,
    target_guid: u64,
) -> Result<(), CastGateRefusal> {
    if target_guid == caster.guid {
        return Ok(());
    }
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return Ok(());
    };
    if caster.is_player()
        && hdr.custom_flags & custom::FACE_TARGET != 0
        && !is_facing(caster.x, caster.y, caster.orientation, target.x, target.y)
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::NotInFront,
            "target must be in front of caster",
        ));
    }
    if hdr.custom_flags & custom::BEHIND_TARGET != 0
        && !is_behind(caster.x, caster.y, target.x, target.y, target.orientation)
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::NotBehind,
            "spell must be cast from BEHIND the target",
        ));
    }
    Ok(())
}

fn check_global_cooldown(ctx: &ReducerContext, caster_guid: u64) -> Result<(), CastGateRefusal> {
    if let Some(cd) = ctx.db.game_spell_cooldown().caster_guid().find(caster_guid) {
        if crate::time::now(ctx) < cd.ready_at {
            return Err(CastGateRefusal::new(
                CastRefusalKind::Cooldown,
                CastRefusalReason::NotReady,
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
    let ready_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros(i64::from(gcd_ms) * 1000))
        .unwrap_or(crate::time::now(ctx));
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
) -> Result<(), CastGateRefusal> {
    let caster_guid = caster.guid;

    if is_action_blocked(ctx, caster_guid) {
        return Err(CastGateRefusal::new(
            CastRefusalKind::CannotAct,
            CastRefusalReason::Stunned,
            format!("caster {caster_guid} cannot act (stun/poly/fear)"),
        ));
    }

    if !caster_life_allows(caster.is_player(), caster.dead, allow_dead_creature) {
        return Err(CastGateRefusal::new(
            CastRefusalKind::Dead,
            CastRefusalReason::CasterDead,
            format!("caster {caster_guid} is dead"),
        ));
    }

    if effects.iter().any(|effect| effect.kind == E_TAME_CREATURE) {
        crate::creatures::validate_tame(ctx, caster, target_guid)?;
    }

    Ok(())
}

fn targets_unit(target: u8) -> bool {
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
        && effects.iter().any(|effect| targets_unit(effect.target))
}

fn check_targeted_los(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
) -> Result<(), CastGateRefusal> {
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
        return Err(CastGateRefusal::new(
            CastRefusalKind::NoLineOfSight,
            CastRefusalReason::NoLineOfSight,
            format!("target {target_guid} is not in line of sight"),
        ));
    }
    Ok(())
}

pub(crate) fn check_cast_start_gates(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    target_guid: u64,
    admission: CreatureSpellCasterAdmission,
) -> Result<(), CastGateRefusal> {
    let request = CastRequest::new(
        caster.guid,
        spell.header.spell_id,
        caster.level as u8,
        target_guid,
    );
    check_cast_gates(ctx, caster, spell, &request, GateRun::Readiness(admission))
}

// Vanilla adds 2 yd combat reach and 8/3 yd moving-target tolerance.
const CAST_RANGE_LEEWAY_YD: f32 = 2.0 + 8.0 / 3.0;

fn check_distract_destination(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    dest: Option<(f32, f32, f32)>,
    range_yd: u32,
) -> Result<(), CastGateRefusal> {
    let caster_at = (caster.x, caster.y, caster.z);
    let point = check_ground_destination(caster_at, dest, range_yd)?;
    if !crate::nav::has_los(ctx, caster.map_id, caster.instance_id, caster_at, point) {
        return Err(CastGateRefusal::new(
            CastRefusalKind::NoLineOfSight,
            CastRefusalReason::NoLineOfSight,
            "ground point is not in line of sight".to_string(),
        ));
    }
    Ok(())
}

fn check_ground_destination(
    caster_at: (f32, f32, f32),
    dest: Option<(f32, f32, f32)>,
    range_yd: u32,
) -> Result<(f32, f32, f32), CastGateRefusal> {
    let Some((x, y, z)) = dest else {
        return Err(CastGateRefusal::new(
            CastRefusalKind::BadTargets,
            CastRefusalReason::BadTargets,
            "spell can only target a ground point".to_string(),
        ));
    };
    let dist = distance_3d(caster_at.0, caster_at.1, caster_at.2, x, y, z);
    check_cast_range("ground point", dist, range_yd)?;
    Ok((x, y, z))
}

fn check_cast_range(what: &str, dist: f32, range_yd: u32) -> Result<(), CastGateRefusal> {
    if dist > range_yd as f32 + CAST_RANGE_LEEWAY_YD {
        return Err(CastGateRefusal::new(
            CastRefusalKind::OutOfRange, CastRefusalReason::OutOfRange,
            format!(
                "{what} out of range ({dist:.1} yd > {range_yd} + {CAST_RANGE_LEEWAY_YD:.1} yd leeway)"
            ),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn check_cast_gate_suffix(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    request: &CastRequest,
) -> Result<(), CastGateRefusal> {
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    let CastRequest {
        spell_id,
        level,
        target_guid,
        cooldown_category,
        ..
    } = *request;
    let caster_guid = caster.guid;
    for effect in effects
        .iter()
        .filter(|effect| effect.kind == E_SUMMON_HOSTILE)
    {
        crate::creatures::check_hostile_summon(ctx, caster, hdr, effect)?;
    }

    for effect in effects
        .iter()
        .filter(|effect| effect.kind == E_SUMMON_GUARDIANS)
    {
        crate::creatures::guardian::check(ctx, caster, effect)?;
    }

    if level_gate_blocks(caster.is_player(), hdr.spell_level, level) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::LevelTooLow,
            format!(
                "spell {spell_id} requires level {} (caster is level {level})",
                hdr.spell_level
            ),
        ));
    }

    if !stance_allows(hdr.stances, caster.stance) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::WrongStance,
            format!(
                "spell {spell_id} cannot be cast in the caster's current stance ({})",
                caster.stance
            ),
        ));
    }

    if ctx
        .db
        .game_school_lockout()
        .by_caster()
        .filter(&caster_guid)
        .any(|r| r.school == hdr.school_mask && r.until > crate::time::now(ctx))
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::SchoolLocked,
            format!(
                "spell {spell_id} school {} is locked (interrupted) for caster {caster_guid}",
                hdr.school_mask
            ),
        ));
    }

    if hdr.attributes & SPELL_ATTR_PASSIVE == 0
        && hdr.cast_flags & CAST_FLAG_RANGED_AUTO_REPEAT == 0
        && !crate::items::equip::spell_ready(
            ctx,
            caster_guid,
            spell_id,
            cooldown_category.unwrap_or(hdr.category),
        )
    {
        return Err(CastGateRefusal::new(
            CastRefusalKind::Cooldown,
            CastRefusalReason::NotReady,
            "spell not ready (cooldown)".to_string(),
        ));
    }

    if hdr.cast_flags & (CAST_FLAG_REQ_OVERPOWER | CAST_FLAG_REQ_REVENGE) != 0 {
        let now_ms = crate::time::now_ms(ctx);
        if hdr.cast_flags & CAST_FLAG_REQ_OVERPOWER != 0
            && !crate::combat::react_window_active(caster.overpower_until_ms, now_ms)
        {
            return Err(CastGateRefusal::other(CastRefusalReason::ReactWindowClosed, format!(
                "spell {spell_id} requires an Overpower window (the target must have dodged your swing)"
            )));
        }
        if hdr.cast_flags & CAST_FLAG_REQ_REVENGE != 0
            && !crate::combat::react_window_active(caster.revenge_until_ms, now_ms)
        {
            return Err(CastGateRefusal::other(
                CastRefusalReason::ReactWindowClosed,
                format!(
                "spell {spell_id} requires a Revenge window (you must have dodged/parried/blocked)"
            ),
            ));
        }
    }

    if target_guid != caster_guid {
        if let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) {
            // World coordinates are meaningful only within the same map and instance.
            if caster.map_id != target.map_id || caster.instance_id != target.instance_id {
                return Err(CastGateRefusal::new(
                    CastRefusalKind::OtherPartition,
                    CastRefusalReason::OutOfRange,
                    format!(
                        "target on another map (caster {} != target {})",
                        caster.map_id, target.map_id
                    ),
                ));
            }
            let dist = distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z);
            if hdr.range_yd > 0 {
                check_cast_range("target", dist, hdr.range_yd)?;
            }
            crate::spell::source_behavior::check_target_level(hdr.min_target_level, target.level)?;
        }
    }

    if target_guid == caster_guid {
        crate::spell::source_behavior::check_target_level(hdr.min_target_level, caster.level)?;
    }

    check_unit_target(ctx, caster, effects, target_guid, spell_id)?;

    if let Some(e) = effects.iter().find(|e| {
        e.kind & KIND_AURA_BIT != 0 && e.p1 != 0 && has_aura(ctx, target_guid, e.p1 as u32)
    }) {
        return Err(CastGateRefusal::other(CastRefusalReason::Unreported, format!(
            "spell {spell_id} refused — target {target_guid} carries the linked debuff {} (Weakened-Soul-style lockout)",
            e.p1
        )));
    }

    if let Some(e) = effects
        .iter()
        .find(|e| e.kind == E_BLINK && e.radius_yd <= 0.0)
    {
        return Err(CastGateRefusal::other(CastRefusalReason::Unreported, format!(
            "spell {spell_id} (Blink) effect {} has no radius — teleport distance is unauthored data",
            e.effect_index
        )));
    }
    if let Some(e) = effects
        .iter()
        .find(|e| e.kind == E_PERSISTENT_AREA && e.radius_yd <= 0.0)
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::Unreported,
            format!(
            "spell {spell_id} (ground-AoE) effect {} has no radius — area size is unauthored data",
            e.effect_index
        ),
        ));
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
        return Err(CastGateRefusal::other(
            CastRefusalReason::Unreported,
            format!(
                "spell {spell_id} effect {} names script {} which is missing or disabled",
                e.effect_index, e.script_id
            ),
        ));
    }

    if target_guid != caster_guid {
        if hdr.cast_flags & CAST_FLAG_REQ_BEHIND != 0 {
            let target = crate::helpers::live_entity(ctx, target_guid)
                .map_err(|_| format!("spell {spell_id} target {target_guid} not in world"))?;
            if !is_behind(caster.x, caster.y, target.x, target.y, target.orientation) {
                return Err(CastGateRefusal::other(
                    CastRefusalReason::NotBehind,
                    format!("spell {spell_id} must be cast from BEHIND the target ({target_guid})"),
                ));
            }
        }
        // Check stealth before cast acceptance breaks it.
        if hdr.cast_flags & CAST_FLAG_REQ_STEALTH != 0 && !is_stealthed(ctx, caster_guid) {
            return Err(CastGateRefusal::other(
                CastRefusalReason::NotStealthed,
                format!(
                    "spell {spell_id} requires stealth (caster {caster_guid} is not stealthed)"
                ),
            ));
        }
        if hdr.cast_flags & CAST_FLAG_INCAP_OPENER != 0 {
            if let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) {
                if target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                    return Err(CastGateRefusal::other(
                        CastRefusalReason::Unreported,
                        format!("spell {spell_id} cannot target a unit IN COMBAT ({target_guid})"),
                    ));
                }
                let is_humanoid = target.is_player()
                    || ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(target.entry)
                        .is_some_and(|t| t.creature_type == HUMANOID_TYPE);
                if !is_humanoid {
                    return Err(CastGateRefusal::other(
                        CastRefusalReason::BadTargets,
                        format!("spell {spell_id} can only target a Humanoid ({target_guid})"),
                    ));
                }
            }
        }
    }

    if hdr.cast_flags & CAST_FLAG_REQ_DAGGER != 0 {
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
            return Err(CastGateRefusal::other(CastRefusalReason::Unreported, format!(
                "spell {spell_id} requires a dagger equipped in the main hand (caster {caster_guid})"
            )));
        }
    }

    if caster.is_player() && is_ranged_weapon_strike(hdr, effects) {
        check_ranged_weapon(ctx, caster_guid, spell_id)?;
    }

    // Creature spells such as NPC Slice and Dice share the finisher flags but hold no points.
    let combo_points = if !caster.is_player() || hdr.cast_flags & CAST_FLAG_COMBO_FINISHER == 0 {
        None
    } else if spends_points_on_target(effects) {
        Some(crate::combo::combo_points(ctx, caster_guid, target_guid))
    } else {
        Some(crate::combo::held_points(ctx, caster_guid))
    };
    if combo_points == Some(0) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::NoComboPoints,
            format!("spell {spell_id} needs combo points on target {target_guid}"),
        ));
    }

    // Creatures have no managed power pool, so only Characters pay spell costs.
    if cost_gate_blocks(caster.is_player(), caster.power, hdr.cost) {
        return Err(CastGateRefusal::new(
            CastRefusalKind::InsufficientPower,
            CastRefusalReason::NoPower,
            format!("not enough power: have {}, need {}", caster.power, hdr.cost),
        ));
    }

    Ok(())
}

fn is_ranged_weapon_strike(hdr: &Spell, effects: &[SpellEffect]) -> bool {
    hdr.attributes & SPELL_ATTR_USES_RANGED_SLOT != 0
        && effects.iter().any(|e| e.kind == E_WEAPON_STRIKE)
}

fn check_ranged_weapon(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
) -> Result<(), CastGateRefusal> {
    use crate::items::weapon_subclass as ws;
    match crate::combat::equipped_ranged_weapon(ctx, caster_guid) {
        None | Some((_, _, _, ws::WAND)) => Err(CastGateRefusal::other(
            CastRefusalReason::Unreported,
            format!("spell {spell_id} needs a ranged weapon (caster {caster_guid})"),
        )),
        Some((_, _, _, ws::BOW | ws::GUN | ws::CROSSBOW))
            if crate::combat::find_ammo(ctx, caster_guid).is_none() =>
        {
            Err(CastGateRefusal::other(
                CastRefusalReason::NoAmmo,
                format!("spell {spell_id} needs ammo (caster {caster_guid})"),
            ))
        }
        Some(_) => Ok(()),
    }
}

// Slice and Dice pairs a self aura with an enemy-targeted dummy, so dummies do not count.
fn spends_points_on_target(effects: &[SpellEffect]) -> bool {
    effects
        .iter()
        .any(|e| e.target == T_TARGET_ENEMY && e.kind != E_SCRIPTED)
}

fn caster_life_allows(is_player: bool, dead: bool, dead_callback: bool) -> bool {
    !dead || (dead_callback && !is_player)
}

// vmangos Spell::CheckCast refuses a temporary enchantment without an item it may enchant.
fn check_item_target(
    ctx: &ReducerContext,
    caster_guid: u64,
    effects: &[SpellEffect],
    item_target: Option<u64>,
) -> Result<(), CastGateRefusal> {
    if !effects.iter().any(|e| e.kind == E_ENCHANT_ITEM_TEMPORARY) {
        return Ok(());
    }
    let owned = item_target
        .and_then(|guid| ctx.db.game_item_instance().guid().find(guid))
        .is_some_and(|item| {
            item.owner_guid == caster_guid && crate::items::is_carried_slot(item.slot)
        });
    if owned {
        Ok(())
    } else {
        Err(CastGateRefusal::other(
            CastRefusalReason::ItemGone,
            "temporary enchantment needs a carried item target",
        ))
    }
}

pub(crate) fn begin_cast(
    ctx: &ReducerContext,
    request: &CastRequest,
    admission: CreatureSpellCasterAdmission,
) -> Result<CastStart, CastGateRefusal> {
    let spell = LoadedSpell::load(ctx, request.spell_id)?;
    begin_loaded_cast(ctx, request, &spell, admission)
}

pub(crate) fn begin_loaded_cast(
    ctx: &ReducerContext,
    request: &CastRequest,
    spell: &LoadedSpell,
    admission: CreatureSpellCasterAdmission,
) -> Result<CastStart, CastGateRefusal> {
    let CastRequest {
        caster_guid,
        spell_id,
        target_guid,
        ..
    } = *request;
    let hdr = &spell.header;

    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    check_cast_gates(ctx, &caster, spell, request, GateRun::Start(admission))?;

    // Break the previous channel before a new channel can place its aura.
    break_channel(ctx, caster_guid);
    if admission == CreatureSpellCasterAdmission::DeadCreatureCallback {
        put_dead_callback_cast_admission(ctx, caster_guid, spell_id);
    }

    // Channels resolve immediately even when their imported cast time is nonzero.
    let (ct_flat, ct_pct) =
        crate::spell::spell_mod(ctx, caster_guid, SPELLMOD_OP_CASTING_TIME, hdr);
    let cast_ms = crate::spell::apply_spell_mod(hdr.cast_time_ms, ct_flat, ct_pct);

    let completed_channel = hdr.cast_flags & CAST_FLAG_CHANNELED != 0
        && spell
            .effects
            .iter()
            .any(|effect| effect.kind == E_TAME_CREATURE);
    if !completed_channel && (cast_ms == 0 || hdr.cast_flags & CAST_FLAG_CHANNELED != 0) {
        let result = resolve_loaded_cast(ctx, request, spell, CastPhase::Start);
        clear_dead_callback_cast_admission(ctx, caster_guid, spell_id);
        return result.map(|_| CastStart::Resolved);
    }

    let completion_ms = if completed_channel {
        hdr.duration_ms.max(1)
    } else {
        cast_ms
    };
    let fire_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros((completion_ms as i64) * 1000))
        .unwrap_or(crate::time::now(ctx));
    let pending = ctx.db.game_pending_cast();
    let stale: Vec<u64> = pending
        .by_caster()
        .filter(&caster_guid)
        .map(|p| p.scheduled_id)
        .collect();
    for id in stale {
        pending.scheduled_id().delete(id);
    }
    let cast = pending.insert(PendingCast::scheduled(
        request,
        crate::time::schedule_time(ctx, fire_at),
    ));
    start_global_cooldown(ctx, caster_guid, hdr.gcd_ms);
    crate::SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            target_guid,
            cast_time_ms: completion_ms,
            ..SpellCastEvent::signal(ctx, caster_guid, spell_id, SpellCastEventKind::Start)
        },
    );
    Ok(CastStart::Started(CastHandle::from_row(ctx, cast)))
}

#[cfg(test)]
mod ground_destination_tests {
    use super::*;

    const CASTER: (f32, f32, f32) = (0.0, 0.0, 0.0);

    #[test]
    fn a_distract_without_a_ground_point_is_a_bad_target() {
        let refusal = check_ground_destination(CASTER, None, 30).unwrap_err();
        assert_eq!(refusal.refusal.kind, CastRefusalKind::BadTargets);
        assert_eq!(refusal.reason, CastRefusalReason::BadTargets);
        assert!(
            refusal
                .refusal
                .message
                .contains("only target a ground point"),
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
        assert_eq!(refusal.refusal.kind, CastRefusalKind::OutOfRange);
        assert_eq!(refusal.reason, CastRefusalReason::OutOfRange);
        assert!(
            refusal.refusal.message.contains("out of range"),
            "{refusal}"
        );
    }
}

#[cfg(test)]
mod companion_los_tests {
    use super::*;
    use crate::spell::{T_SCRIPTED, T_SELF};

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
            assert!(targets_unit(target));
        }
        assert!(!targets_unit(T_SELF));
        assert!(!targets_unit(T_SCRIPTED));
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
            0,
        );
    }
}

pub(crate) fn apply_item_auras(
    ctx: &ReducerContext,
    header: &Spell,
    guid: u64,
    level: u8,
    item: u64,
) {
    let mut header = header.clone();
    if header.equip_passive() && (header.duration_index == 0 || header.visual_id == 0) {
        header.duration_ms = u32::MAX;
    } else if header.duration_ms < 200 {
        header.duration_ms = 300;
    }
    for effect in ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(header.spell_id)
        .filter(|effect| effect.equip_supported && passive_applies_effect_kind(effect.kind))
    {
        if ctx
            .db
            .game_aura()
            .by_target()
            .filter(guid)
            .any(|aura| aura.effect_id == effect.id && aura.item_guid(ctx) == item)
        {
            continue;
        }
        let points = effect_amount(
            effect.base_points,
            effect.die_sides,
            effect.per_level,
            level as u32,
            header.spell_level,
            header.max_level,
            ctx.random::<u32>(),
        );
        aura_apply(ctx, &effect, &header, guid, guid, guid, level, points, item);
    }
}

#[cfg(test)]
mod eventai_hit_tests {
    use super::*;
    use crate::spell::{A_CONTROL, T_SELF};

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
            points_per_combo: 0.0,
            bonus_coefficient: -1.0,
            equip_supported: false,
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
    fn slice_and_dice_reads_held_points_and_kidney_shot_reads_the_target() {
        let aimed = |kind, target| SpellEffect {
            target,
            ..effect(kind)
        };
        let slice_and_dice = [aimed(E_SCRIPTED, T_TARGET_ENEMY), aimed(A_FLAG, T_SELF)];
        let kidney_shot = [aimed(A_CONTROL, T_TARGET_ENEMY)];
        assert!(!spends_points_on_target(&slice_and_dice));
        assert!(spends_points_on_target(&kidney_shot));
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
}

#[cfg(test)]
mod cast_miss_tests {
    use super::*;

    #[test]
    fn a_failed_hit_roll_misses_a_physical_spell_and_resists_a_magic_one() {
        assert_eq!(
            cast_miss(AttackOutcome::Normal, false, true),
            crate::spell::CAST_MISS_NONE
        );
        assert_eq!(
            cast_miss(AttackOutcome::Crit, true, false),
            crate::spell::CAST_MISS_NONE
        );
        assert_eq!(
            cast_miss(AttackOutcome::Miss, false, false),
            crate::spell::CAST_MISS_MISS
        );
        assert_eq!(
            cast_miss(AttackOutcome::Miss, false, true),
            crate::spell::CAST_MISS_RESIST
        );
    }

    #[test]
    fn a_weapon_spell_reports_its_physical_avoidance_whatever_its_school() {
        assert_eq!(
            cast_miss(AttackOutcome::Miss, true, true),
            crate::spell::CAST_MISS_MISS
        );
        assert_eq!(
            cast_miss(AttackOutcome::Dodge, true, false),
            crate::spell::CAST_MISS_DODGE
        );
        assert_eq!(
            cast_miss(AttackOutcome::Parry, true, false),
            crate::spell::CAST_MISS_PARRY
        );
    }
}
