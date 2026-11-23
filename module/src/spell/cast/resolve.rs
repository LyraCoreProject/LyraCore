use lyracore_shared::spell::taxonomy::CAST_FLAG_RANGED_AUTO_REPEAT;
use lyracore_shared::spell_custom as custom;
use spacetimedb::{log, ReducerContext, Table, TimeDuration};

use super::depth::TriggerLink;
use crate::combat::AttackOutcome;
use crate::geometry::{distance_3d, is_behind, is_facing};
use crate::{game_creature_template, game_world_entity, WorldEntity};

use crate::{game_item_instance, game_item_template};

use crate::spell::{
    apply_effect, aura_apply, break_channel, break_stealth, cost_gate_blocks, cost_refusal_reason,
    effect_amount, faction_target_violation, is_action_blocked, is_stealthed, level_gate_blocks,
    recipe_reagents, select_targets, CastGateRefusal, CastHandle, CastRefusalKind,
    CastRefusalReason, CastStart, FormRefusal, ItemGrantOutcome, SpellCastEventKind, A_FLAG,
    A_MOUNTED, CAST_FLAG_BINARY, CAST_FLAG_CHANNELED, CAST_FLAG_COMBO_FINISHER,
    CAST_FLAG_INCAP_OPENER, CAST_FLAG_MAGIC_HIT, CAST_FLAG_REQ_BEHIND, CAST_FLAG_REQ_DAGGER,
    CAST_FLAG_REQ_OVERPOWER, CAST_FLAG_REQ_REVENGE, CAST_FLAG_REQ_STEALTH, CAST_FLAG_STEALTH_SAFE,
    CAST_FLAG_WEAPON_PROCS, E_BLINK, E_CREATE_ITEM, E_DAMAGE, E_DISTRACT, E_DUEL,
    E_ENCHANT_ITEM_TEMPORARY, E_FINISHER_DAMAGE, E_INTERRUPT, E_NEXT_SWING, E_PERSISTENT_AREA,
    E_SCRIPTED, E_SUMMON_GUARDIANS, E_SUMMON_HOSTILE, E_TAME_CREATURE, E_WEAPON_STRIKE,
    HUMANOID_TYPE, KIND_AURA_BIT, RESIST_ARMOR, SPELLMOD_OP_CASTING_TIME, SPELL_ATTR_PASSIVE,
    SPELL_ATTR_USES_RANGED_SLOT, T_AREA_ALLY, T_AREA_ENEMY, T_CHAIN_ENEMY, T_TARGET_ALLY,
    T_TARGET_ANY, T_TARGET_ENEMY,
};
use crate::{
    game_aura, game_global_cooldown, game_pending_cast, game_school_lockout, game_spell,
    game_spell_cd, game_spell_effect, game_spell_follow_up, GlobalCooldown, PendingCast, Spell,
    SpellCastEvent, SpellCd, SpellEffect,
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
    /// The item that started the cast; its spell charges decide what the cast takes from it.
    pub cast_item: Option<u64>,
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
            cast_item: None,
            cooldown_category: None,
        }
    }

    fn is_item_cast(&self) -> bool {
        self.cast_item.is_some() || self.cooldown_category.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastPhase {
    Start,
    Completion,
    /// A Trigger Child Cast, resolved at once inside its parent's effects.
    Trigger,
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

/// Resolves a Trigger Child Cast.
pub(crate) fn resolve_trigger_cast(
    ctx: &ReducerContext,
    request: &CastRequest,
) -> Result<(), CastGateRefusal> {
    let spell = LoadedSpell::load(ctx, request.spell_id)?;
    resolve_loaded_cast(ctx, request, &spell, CastPhase::Trigger)
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
    let _link = TriggerLink::enter(ctx.timestamp, spell_id)?;

    let mut caster = crate::helpers::live_entity(ctx, caster_guid)
        .map_err(|_| format!("caster {caster_guid} not in world"))?;

    check_cast_gates(ctx, &caster, spell, request, GateRun::Resolve(phase))?;

    let cast_item = request
        .cast_item
        .map(|item_guid| (item_guid, crate::items::cast_item_use_of(ctx, item_guid)));
    let used_up_item = cast_item
        .filter(|(_, item_use)| *item_use == crate::items::CastItemUse::UseUp)
        .map(|(item_guid, _)| item_guid);
    // Scheduled crafts admit reagents before charging power; all products commit together.
    let craft = if effects.iter().any(|effect| effect.kind == E_CREATE_ITEM) {
        let mut plan = crate::items::ItemStoragePlan::read(ctx, caster_guid, caster.owner_identity);
        if let Some(item_guid) = used_up_item {
            plan.use_up_one(item_guid).map_err(String::from)?;
        }
        for (entry, count) in recipe_reagents(ctx, spell_id) {
            plan.consume(entry, count).map_err(String::from)?;
        }
        plan.check_trade_offers(ctx).map_err(String::from)?;
        Some(Craft { plan, used_up_item })
    } else {
        None
    };
    let plan_takes_cast_item = craft.is_some() && used_up_item.is_some();

    // Flush the cost before effects so self-heals read the charged caster row.
    if caster.is_player() && hdr.cost > 0 && !request.is_item_cast() && !queues_next_swing(effects)
    {
        charge_spell_cost(ctx, &mut caster, hdr);
        caster.update(ctx);
    }

    let untriggered = phase != CastPhase::Trigger;
    // Dismount after charging power so the stale caster cannot overwrite the Mount Projection.
    if untriggered && !effects.iter().any(|e| e.kind == A_MOUNTED) {
        crate::mount::dismount(ctx, caster_guid);
    }

    // Break stealth before effects so recasting Stealth can reapply its aura.
    if untriggered && hdr.cast_flags & CAST_FLAG_STEALTH_SAFE == 0 {
        break_stealth(ctx, caster_guid);
    }

    run_spell_effects(ctx, spell, request, CastOrigin::Direct(phase), craft);

    // vmangos Spell::TakeCastItem runs after SMSG_SPELL_GO and the effects.
    if let Some((item_guid, item_use)) = cast_item.filter(|_| !plan_takes_cast_item) {
        crate::items::take_cast_item(ctx, item_guid, item_use);
    }

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
        start_global_cooldown(ctx, caster_guid, hdr);
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
    AuraTriggered,
    DeadCreatureCallbackTriggered,
    ChanceOnHit,
}

impl CastOrigin {
    fn is_triggered(self) -> bool {
        matches!(
            self,
            Self::Triggered
                | Self::AuraTriggered
                | Self::DeadCreatureCallbackTriggered
                | Self::ChanceOnHit
        )
    }

    // Simplifies vmangos Spell::m_canTrigger; see the ledger gap in spell_cast_kinds.toml.
    fn can_trigger(self, hdr: &Spell) -> bool {
        match self {
            Self::AuraTriggered => false,
            Self::ChanceOnHit => hdr.negative(),
            _ => true,
        }
    }

    fn allows_dead_self(self) -> bool {
        matches!(self, Self::DeadCreatureCallbackTriggered)
    }

    fn is_completion(self) -> bool {
        matches!(self, Self::Direct(CastPhase::Completion))
    }
}

pub(crate) fn charge_spell_cost(ctx: &ReducerContext, caster: &mut WorldEntity, hdr: &Spell) {
    caster.spend_power(hdr.power_type, hdr.cost);
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

// Each landed target, in first-landing order, with the mask of the effect indexes that reached it.
fn remember_landed_effect(landed: &mut Vec<(u64, u8)>, guid: u64, effect_index: u8) {
    let bit = 1 << effect_index;
    match landed
        .iter_mut()
        .find(|(landed_guid, _)| *landed_guid == guid)
    {
        Some((_, mask)) => *mask |= bit,
        None if guid != 0 => landed.push((guid, bit)),
        None => {}
    }
}

/// A craft's storage plan, and the cast item it uses up through the plan or, if dropped, directly.
struct Craft {
    plan: crate::items::ItemStoragePlan,
    used_up_item: Option<u64>,
}

fn grant_crafted_item(
    ctx: &ReducerContext,
    plan: &mut crate::items::ItemStoragePlan,
    e: &SpellEffect,
    hdr: &Spell,
    points: i32,
) -> super::targeting::EffectHit {
    let result = ctx
        .db
        .game_item_template()
        .entry()
        .find(e.p0.max(0) as u32)
        .ok_or_else(|| {
            crate::actor::ActionRefusal::from("craft product has no item template".to_string())
        })
        .and_then(|template| plan.grant(ctx, &template, points.max(1) as u32, false, None));
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
}

/// Each target's immunity: to the whole spell by mechanic or school before its hit roll, as vmangos
/// `SpellHitResult` decides it, and to each effect by mechanic whatever the roll.
#[derive(Default)]
struct ImmuneRefusals {
    header: std::collections::HashMap<u64, bool>,
    refused: std::collections::BTreeSet<u64>,
    exposed: std::collections::BTreeSet<u64>,
}

impl ImmuneRefusals {
    /// Whether the target is immune to the whole spell, asked once per target.
    fn refuses_spell(
        &mut self,
        ctx: &ReducerContext,
        caster_guid: u64,
        target_guid: u64,
        hdr: &Spell,
        effects: &[SpellEffect],
    ) -> bool {
        let harmful = crate::spell::spell_is_harmful(hdr, effects);
        let refused_by_school = |unit: crate::WorldEntity| {
            crate::spell::school_refuses_spell(ctx, caster_guid, &unit, hdr, harmful)
        };
        *self.header.entry(target_guid).or_insert_with(|| {
            crate::spell::is_immune_to_spell(ctx, caster_guid, target_guid, hdr, effects)
                // vmangos tests a queued strike's school when its swing fires.
                || (!queues_next_swing(effects)
                    && ctx
                        .db
                        .game_world_entity()
                        .guid()
                        .find(target_guid)
                        .is_some_and(refused_by_school))
        })
    }

    fn record(&mut self, target_guid: u64, immune: bool) {
        if immune {
            self.refused.insert(target_guid);
        } else {
            self.exposed.insert(target_guid);
        }
    }

    fn refused_by_header(&self, target_guid: u64) -> bool {
        self.header.get(&target_guid) == Some(&true)
    }

    fn refused_whole(&self, target_guid: u64) -> bool {
        self.refused_by_header(target_guid)
            || (self.refused.contains(&target_guid) && !self.exposed.contains(&target_guid))
    }

    fn whole_refusals(&self) -> Vec<u64> {
        let by_header = self.header.iter().filter(|(_, &immune)| immune);
        let mut guids: Vec<u64> = by_header
            .map(|(&guid, _)| guid)
            .chain(self.refused.iter().copied())
            .filter(|&guid| self.refused_whole(guid))
            .collect();
        guids.sort_unstable();
        guids.dedup();
        guids
    }

    /// The cast's miss code for a target: 7 for a header refusal and 8 when only effects were.
    fn miss(&self, target_guid: u64) -> Option<u8> {
        if self.refused_by_header(target_guid) {
            Some(crate::spell::CAST_MISS_IMMUNE)
        } else {
            self.refused_whole(target_guid)
                .then_some(crate::spell::CAST_MISS_IMMUNE2)
        }
    }
}

// vmangos Spell::DoAllEffectOnTarget: a refused harmful spell still procs and draws the unit in.
fn answer_whole_refusals(
    ctx: &ReducerContext,
    immunity: &ImmuneRefusals,
    caster_guid: u64,
    origin: CastOrigin,
    spell: &LoadedSpell,
) {
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    if !crate::spell::spell_is_harmful(hdr, effects) {
        return;
    }
    for t in immunity.whole_refusals() {
        if origin.can_trigger(hdr) {
            crate::spell::proc::run_immune_proc_pass(ctx, caster_guid, t, hdr.spell_id);
        }
        if t != caster_guid && !matches!(origin, CastOrigin::AuraTriggered) {
            engage_refusing_unit(ctx, caster_guid, t);
        }
    }
}

fn engage_refusing_unit(ctx: &ReducerContext, caster_guid: u64, unit_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(unit)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(unit_guid),
    ) else {
        return;
    };
    if unit.dead || !crate::combat::may_harm(ctx, &caster, &unit) {
        return;
    }
    // vmangos SpellHitResult answers SPELL_MISS_EVADE before it tests immunity.
    if !unit.is_player() && crate::creatures::eventai::movement::returning_home(ctx, unit_guid) {
        return;
    }
    if unit.is_player() {
        crate::combat::enter_combat(ctx, caster_guid);
        crate::combat::enter_combat(ctx, unit_guid);
    } else {
        crate::threat::engage_without_threat(ctx, unit_guid, caster_guid);
    }
}

fn run_spell_effects(
    ctx: &ReducerContext,
    spell: &LoadedSpell,
    request: &CastRequest,
    origin: CastOrigin,
    mut craft: Option<Craft>,
) {
    let (caster_guid, target_guid) = (request.caster_guid, request.target_guid);
    let (level, dest) = (request.level, request.dest);
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    let mut landed = Vec::new();
    let triggered = origin.is_triggered();
    let mut total_damage: u32 = 0;
    let mut total_healed: u32 = 0;
    let mut total_resisted: u32 = 0;
    let mut total_absorbed: u32 = 0;
    let mut total_energized: u32 = 0;
    let mut energized_power_type: u8 = 0;
    let mut primary_avoided = AttackOutcome::Normal;
    let mut any_crit = false;
    let mut created_items = Vec::new();
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
    let mut shield_targets = std::collections::BTreeSet::new();
    let mut immunity = ImmuneRefusals::default();
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
            // Before the hit roll, so a miss counts; vmangos skips a cast an aura triggers.
            if t != caster_guid && !matches!(origin, CastOrigin::AuraTriggered) {
                crate::pvp::spell_targeted(ctx, caster_guid, t, e);
            }
            if rolls_weapon && t != caster_guid {
                remember_hit_target(&mut weapon_targets, &mut seen_weapon_targets, t);
            }
            if immunity.refuses_spell(ctx, caster_guid, t, hdr, effects) {
                continue;
            }
            let roll = if rolls_hit && t != caster_guid {
                *target_rolls
                    .entry(t)
                    .or_insert_with(|| hit_roll(ctx, caster_guid, level, t, table, hdr, triggered))
            } else {
                AttackOutcome::Normal
            };
            let immune = crate::spell::is_immune_to_effect(ctx, caster_guid, t, hdr, e);
            immunity.record(t, immune);
            if roll.is_avoided() {
                if t == target_guid {
                    primary_avoided = roll;
                }
                continue;
            }
            if immune {
                log::info!(
                    "spell {} effect {} refused: unit {t} is immune to its mechanic",
                    hdr.spell_id,
                    e.effect_index
                );
                continue;
            }
            if e.kind == E_CREATE_ITEM {
                created_items.push(e.p0.max(0) as u32);
            }
            let hit = if let (E_CREATE_ITEM, Some(craft)) = (e.kind, craft.as_mut()) {
                grant_crafted_item(ctx, &mut craft.plan, e, hdr, points)
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
            if t != caster_guid && hit.dealt > 0 && hdr.cast_flags & CAST_FLAG_WEAPON_PROCS != 0 {
                shield_targets.insert(t);
            }
            remember_landed_effect(&mut landed, t, e.effect_index);
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
    answer_whole_refusals(ctx, &immunity, caster_guid, origin, spell);
    // Spend after every effect has read the points; vmangos keeps them when a finisher misses.
    if hdr.cast_flags & CAST_FLAG_COMBO_FINISHER != 0
        && !primary_avoided.is_avoided()
        && immunity.miss(target_guid).is_none()
    {
        crate::combo::spend_combo(ctx, caster_guid);
    }
    let finish_items = |craft: Option<Craft>| {
        finish_item_creation(
            ctx,
            caster_guid,
            hdr.spell_id,
            craft,
            item_grants,
            item_refused,
            &created_items,
        );
    };
    if queues_next_swing(effects) {
        finish_items(craft);
        return;
    }
    crate::threat::add_table_threat(ctx, caster_guid, hdr.spell_id, &landed);
    for &(target, _) in &landed {
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
            miss_info: immunity.miss(target_guid).unwrap_or_else(|| {
                cast_miss(
                    primary_avoided,
                    weapon_strike,
                    hdr.cast_flags & CAST_FLAG_MAGIC_HIT != 0
                        || hdr.school_mask & !RESIST_ARMOR != 0,
                )
            }),
            energized: total_energized,
            power_type: energized_power_type,
            cooldown_ms: hdr.cooldown_ms,
            client_initiated: request.client_initiated,
            ..SpellCastEvent::signal(ctx, caster_guid, hdr.spell_id, SpellCastEventKind::Go)
        },
    );
    finish_items(craft);
    finish_weapon_hits(ctx, caster_guid, weapon_targets, shield_targets);
    cast_follow_up(ctx, hdr.spell_id, caster_guid, level, &landed);
}

thread_local! {
    // The reducer timestamp of a Follow-up Cast on the call stack; a stale one never matches.
    static FOLLOW_UP_IN_PROGRESS: std::cell::Cell<Option<spacetimedb::Timestamp>> =
        const { std::cell::Cell::new(None) };
}

struct FollowUpInProgress;

impl FollowUpInProgress {
    fn enter(ctx: &ReducerContext) -> Option<Self> {
        FOLLOW_UP_IN_PROGRESS.with(|cell| {
            (cell.get() != Some(ctx.timestamp)).then(|| {
                cell.set(Some(ctx.timestamp));
                Self
            })
        })
    }
}

impl Drop for FollowUpInProgress {
    fn drop(&mut self) {
        FOLLOW_UP_IN_PROGRESS.with(|cell| cell.set(None));
    }
}

// Read docs/schema.md#follow-up-casts for its timing and its loop limit.
fn cast_follow_up(
    ctx: &ReducerContext,
    spell_id: u32,
    caster_guid: u64,
    level: u8,
    landed: &[(u64, u8)],
) {
    if landed.is_empty() {
        return;
    }
    let Some(follow_up) = ctx.db.game_spell_follow_up().spell_id().find(spell_id) else {
        return;
    };
    let Some(_in_progress) = FollowUpInProgress::enter(ctx) else {
        log::info!(
            "spell {spell_id} follow-up {} skipped: it descends from a Follow-up Cast",
            follow_up.follow_up_spell_id
        );
        return;
    };
    let Ok(spell) = LoadedSpell::load(ctx, follow_up.follow_up_spell_id) else {
        log::warn!(
            "spell {spell_id} follow-up {} has no spell row",
            follow_up.follow_up_spell_id
        );
        return;
    };
    for &(target_guid, _) in landed {
        let request = CastRequest::new(caster_guid, spell.header.spell_id, level, target_guid);
        if let Err(reason) =
            cast_triggered_with_origin(ctx, &request, &spell, CastOrigin::Triggered)
        {
            log::info!(
                "spell {spell_id} follow-up {} refused on {target_guid}: {reason}",
                spell.header.spell_id
            );
        }
    }
}

// vmangos sends SMSG_SPELL_GO before effects, so a client cast's item packets and execute log follow it.
fn finish_item_creation(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    craft: Option<Craft>,
    item_grants: u32,
    mut item_refused: bool,
    created_items: &[u32],
) {
    if let Some(Craft { plan, used_up_item }) = craft {
        if !item_refused {
            if let Err(reason) = plan.commit_with_source(ctx, crate::items::ItemPush::SPELL_CREATED)
            {
                log::info!("craft {spell_id} refused: {reason:?}");
                item_refused = true;
            }
        }
        // A refused product drops the plan, but the cast went off and still uses up its item.
        if item_refused {
            if let Some(item_guid) = used_up_item {
                crate::items::use_up_one(ctx, item_guid);
            }
        }
    }
    if !item_refused {
        for _ in 0..item_grants {
            super::targeting::gain_recipe_skill(ctx, caster_guid, spell_id);
        }
    }
    crate::native::spell::item_creation_log(ctx, caster_guid, spell_id, created_items);
}

fn finish_weapon_hits(
    ctx: &ReducerContext,
    caster_guid: u64,
    weapon_targets: Vec<u64>,
    shield_targets: std::collections::BTreeSet<u64>,
) {
    for target in weapon_targets {
        crate::spell::chance_on_hit::roll_chance_on_hit(
            ctx,
            caster_guid,
            target,
            crate::items::equip_slot::MAINHAND,
        );
    }
    for target in shield_targets {
        crate::combat::damage_shield::reflect(ctx, caster_guid, target);
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
    triggered: bool,
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
            if !triggered {
                let source = if ranged {
                    crate::combat::HitSource::Ranged
                } else {
                    crate::combat::HitSource::MeleeSpell
                };
                crate::spell::proc::run_unlanded_proc_pass(
                    ctx,
                    caster_guid,
                    target_guid,
                    &crate::combat::Hit::spell(source, spell.spell_id, false),
                    roll,
                    (0, 0),
                );
            }
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
        | AttackOutcome::Block
        | AttackOutcome::Immune => {}
    }
}

fn cast_miss(roll: AttackOutcome, weapon_strike: bool, magic_hit: bool) -> u8 {
    match roll {
        AttackOutcome::Dodge => crate::spell::CAST_MISS_DODGE,
        AttackOutcome::Parry => crate::spell::CAST_MISS_PARRY,
        AttackOutcome::Miss if weapon_strike || !magic_hit => crate::spell::CAST_MISS_MISS,
        AttackOutcome::Miss => crate::spell::CAST_MISS_RESIST,
        AttackOutcome::Immune => crate::spell::CAST_MISS_IMMUNE,
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

pub(crate) fn cast_aura_triggered(
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
        CastOrigin::AuraTriggered,
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
    let _link = TriggerLink::enter(ctx.timestamp, request.spell_id)?;
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
        TriggerLink::check(ctx.timestamp, request.spell_id)?;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateRun {
    /// A creature's interrupting cast, checked before it breaks the caster's channel.
    InterruptPrecheck(CreatureSpellCasterAdmission),
    /// A bot's cast readiness.
    Readiness(CreatureSpellCasterAdmission),
    Start(CreatureSpellCasterAdmission),
    Resolve(CastPhase),
}

impl GateRun {
    /// vmangos `Spell::CheckCast(strict)`: only the check at timed cast completion is not strict.
    fn is_strict(self) -> bool {
        self != Self::Resolve(CastPhase::Completion)
    }

    fn is_trigger_child(self) -> bool {
        self == Self::Resolve(CastPhase::Trigger)
    }
}

/// One cast Gate; [`cast_gate_order`] decides which Gates a run checks and in what order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CastGate {
    SpellReady,
    CasterAlive,
    CasterLevel,
    GlobalCooldown,
    NonCombat,
    Form,
    Stealth,
    Mount,
    ReactWindow,
    TargetPartition,
    TargetLevel,
    TaxiTarget,
    LineOfSight,
    PeacefulTarget,
    HumanoidTarget,
    TargetFaction,
    TargetImmunity,
    Behind,
    GroundPointLineOfSight,
    Dagger,
    CastItem,
    SpellFocus,
    ProductStorage,
    ItemTarget,
    RangedWeapon,
    Range,
    Facing,
    GroundPointRange,
    Power,
    CasterControl,
    Tame,
    Guardian,
    SummonTarget,
    EffectData,
}

/// vmangos `Spell::CheckCast` order, cut down to the Gates LyraCore has.
const CHECK_CAST_ORDER: [CastGate; 34] = [
    CastGate::SpellReady,
    CastGate::CasterAlive,
    CastGate::CasterLevel,
    CastGate::GlobalCooldown,
    CastGate::NonCombat,
    CastGate::Form,
    CastGate::Stealth,
    CastGate::Mount,
    CastGate::ReactWindow,
    CastGate::TargetPartition,
    CastGate::TargetLevel,
    CastGate::TaxiTarget,
    CastGate::LineOfSight,
    CastGate::PeacefulTarget,
    CastGate::HumanoidTarget,
    CastGate::TargetFaction,
    CastGate::TargetImmunity,
    CastGate::Behind,
    CastGate::GroundPointLineOfSight,
    CastGate::Dagger,
    CastGate::CastItem,
    CastGate::SpellFocus,
    CastGate::ProductStorage,
    CastGate::ItemTarget,
    CastGate::RangedWeapon,
    CastGate::Range,
    CastGate::Facing,
    CastGate::GroundPointRange,
    CastGate::Power,
    CastGate::CasterControl,
    CastGate::Tame,
    CastGate::Guardian,
    CastGate::SummonTarget,
    CastGate::EffectData,
];

impl CastGate {
    /// vmangos `Spell::CheckCast` runs these only when `!m_IsTriggeredSpell`.
    fn skipped_by_trigger_child(self) -> bool {
        matches!(
            self,
            Self::SpellReady
                | Self::CasterAlive
                | Self::GlobalCooldown
                | Self::NonCombat
                | Self::Form
                | Self::Stealth
                | Self::LineOfSight
                | Self::PeacefulTarget
                | Self::ProductStorage
                | Self::Range
                | Self::Facing
                | Self::GroundPointRange
                | Self::Power
                | Self::CasterControl
        )
    }

    fn runs_in(self, run: GateRun) -> bool {
        if run.is_trigger_child() && self.skipped_by_trigger_child() {
            return false;
        }
        match self {
            Self::GlobalCooldown
            | Self::NonCombat
            | Self::Form
            | Self::Stealth
            | Self::PeacefulTarget => run.is_strict(),
            // A completion checks power before every other Gate instead.
            Self::Power => run.is_strict(),
            // An instant checked line of sight at Start.
            Self::LineOfSight => matches!(
                run,
                GateRun::Readiness(_) | GateRun::Start(_) | GateRun::Resolve(CastPhase::Completion)
            ),
            Self::ItemTarget | Self::GroundPointLineOfSight | Self::GroundPointRange => {
                matches!(run, GateRun::Start(_) | GateRun::Resolve(_))
            }
            _ => true,
        }
    }
}

/// The Gates a run checks in order; a completion leads with power.
fn cast_gate_order(run: GateRun) -> impl Iterator<Item = CastGate> {
    let completion_first: &[CastGate] = if run.is_strict() {
        &[]
    } else {
        &[CastGate::Power]
    };
    completion_first.iter().copied().chain(
        CHECK_CAST_ORDER
            .into_iter()
            .filter(move |gate| gate.runs_in(run)),
    )
}

// Gate order decides which Refusal the client sees when two Gates would both refuse.
fn check_cast_gates(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    request: &CastRequest,
    run: GateRun,
) -> Result<(), CastGateRefusal> {
    let target = (request.target_guid != caster.guid)
        .then(|| ctx.db.game_world_entity().guid().find(request.target_guid))
        .flatten();
    let check = GateCheck {
        ctx,
        caster,
        spell,
        request,
        run,
        target: target.as_ref(),
    };
    cast_gate_order(run).try_for_each(|gate| check.gate(gate))
}

struct GateCheck<'a> {
    ctx: &'a ReducerContext,
    caster: &'a WorldEntity,
    spell: &'a LoadedSpell,
    request: &'a CastRequest,
    run: GateRun,
    /// The unit target other than the caster, when it is in the world.
    target: Option<&'a WorldEntity>,
}

impl GateCheck<'_> {
    fn gate(&self, gate: CastGate) -> Result<(), CastGateRefusal> {
        let (ctx, caster, spell, request) = (self.ctx, self.caster, self.spell, self.request);
        let (hdr, effects) = (&spell.header, spell.effects.as_slice());
        let (spell_id, target_guid) = (request.spell_id, request.target_guid);
        let has_effect = |kind| effects.iter().any(|effect| effect.kind == kind);
        match gate {
            CastGate::SpellReady => check_spell_ready(ctx, caster.guid, hdr, request),
            CastGate::CasterAlive => self.check_caster_alive(),
            CastGate::CasterLevel => {
                if level_gate_blocks(caster.is_player(), hdr.spell_level, request.level) {
                    return Err(CastGateRefusal::other(
                        CastRefusalReason::LevelTooLow,
                        format!(
                            "spell {spell_id} requires level {} (caster is level {})",
                            hdr.spell_level, request.level
                        ),
                    ));
                }
                Ok(())
            }
            CastGate::GlobalCooldown => check_global_cooldown(ctx, caster.guid, hdr),
            CastGate::Form => check_form(ctx, caster, spell),
            CastGate::Stealth => {
                // Check stealth before cast acceptance breaks it.
                if target_guid != caster.guid
                    && hdr.cast_flags & CAST_FLAG_REQ_STEALTH != 0
                    && !is_stealthed(ctx, caster.guid)
                {
                    return Err(CastGateRefusal::other(
                        CastRefusalReason::NotStealthed,
                        format!(
                            "spell {spell_id} requires stealth (caster {} is not stealthed)",
                            caster.guid
                        ),
                    ));
                }
                Ok(())
            }
            CastGate::NonCombat => {
                if hdr.non_combat()
                    && caster.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
                {
                    return Err(CastGateRefusal::other(
                        CastRefusalReason::AffectingCombat,
                        format!("spell {spell_id} cannot be cast in combat"),
                    ));
                }
                Ok(())
            }
            CastGate::Mount => crate::mount::check_mount_cast(ctx, caster, effects, spell_id),
            CastGate::ReactWindow => check_react_window(ctx, caster, hdr),
            CastGate::TargetPartition => match self.target {
                // World coordinates are meaningful only within the same map and instance.
                Some(target)
                    if (caster.map_id, caster.instance_id)
                        != (target.map_id, target.instance_id) =>
                {
                    Err(CastGateRefusal::new(
                        CastRefusalKind::OtherPartition,
                        CastRefusalReason::OutOfRange,
                        format!(
                            "target on another map (caster {} != target {})",
                            caster.map_id, target.map_id
                        ),
                    ))
                }
                _ => Ok(()),
            },
            CastGate::TargetLevel => {
                let level = if target_guid == caster.guid {
                    Some(caster.level)
                } else {
                    self.target.map(|target| target.level)
                };
                level.map_or(Ok(()), |level| {
                    crate::spell::source_behavior::check_target_level(hdr.min_target_level, level)
                })
            }
            CastGate::TaxiTarget => check_taxi_target(ctx, caster, effects, target_guid),
            CastGate::LineOfSight => check_targeted_los(ctx, caster, spell, target_guid),
            CastGate::PeacefulTarget => match self.target {
                Some(target)
                    if hdr.cast_flags & CAST_FLAG_INCAP_OPENER != 0
                        && target.unit_flags
                            & lyracore_shared::constants::unit_flags::IN_COMBAT
                            != 0 =>
                {
                    Err(CastGateRefusal::other(
                        CastRefusalReason::Unreported,
                        format!("spell {spell_id} cannot target a unit IN COMBAT ({target_guid})"),
                    ))
                }
                _ => Ok(()),
            },
            CastGate::HumanoidTarget => match self.target {
                Some(target)
                    if hdr.cast_flags & CAST_FLAG_INCAP_OPENER != 0
                        && !is_humanoid(ctx, target) =>
                {
                    Err(CastGateRefusal::other(
                        CastRefusalReason::BadTargets,
                        format!("spell {spell_id} can only target a Humanoid ({target_guid})"),
                    ))
                }
                _ => Ok(()),
            },
            CastGate::TargetFaction => {
                check_target_faction(ctx, caster, effects, target_guid, spell_id)
            }
            CastGate::TargetImmunity => {
                check_target_aura_state(ctx, spell, caster.guid, target_guid)
            }
            CastGate::Behind => self.check_behind(),
            CastGate::GroundPointLineOfSight if has_effect(E_DISTRACT) => {
                check_ground_point_los(ctx, caster, request.dest)
            }
            CastGate::GroundPointLineOfSight => Ok(()),
            CastGate::Dagger => check_dagger(ctx, caster.guid, hdr),
            CastGate::SpellFocus => effects
                .iter()
                .filter(|effect| effect.kind == E_SUMMON_HOSTILE)
                .try_for_each(|effect| {
                    crate::creatures::check_hostile_summon(ctx, caster, hdr, effect).map(|_| ())
                }),
            CastGate::CastItem => check_cast_item(ctx, caster.guid, request.cast_item),
            CastGate::ProductStorage => {
                product_storage(ctx, caster, spell, request.level).map_err(|refusal| {
                    CastGateRefusal {
                        item_refusal: Some(refusal),
                        ..CastGateRefusal::other(
                            CastRefusalReason::Unreported,
                            format!(
                                "spell {spell_id} product cannot be stored: {}",
                                refusal.as_tag()
                            ),
                        )
                    }
                })
            }
            CastGate::ItemTarget => {
                check_item_target(ctx, caster.guid, effects, request.item_target)
            }
            CastGate::RangedWeapon => {
                if caster.is_player() && is_ranged_weapon_strike(hdr, effects) {
                    check_ranged_weapon(ctx, caster.guid, spell_id)?;
                }
                Ok(())
            }
            CastGate::Range => match self.target {
                Some(target) if hdr.range_yd > 0 => {
                    let dist =
                        distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z);
                    check_cast_range("target", dist, hdr.range_yd, self.range_leeway_yd())
                }
                _ => Ok(()),
            },
            CastGate::Facing => self.check_facing(),
            CastGate::GroundPointRange if has_effect(E_DISTRACT) => {
                let (x, y, z) = ground_point(request.dest)?;
                let dist = distance_3d(caster.x, caster.y, caster.z, x, y, z);
                check_cast_range("ground point", dist, hdr.range_yd, self.range_leeway_yd())
            }
            CastGate::GroundPointRange => Ok(()),
            CastGate::Power => check_power(ctx, caster, spell, request),
            CastGate::CasterControl => {
                if is_action_blocked(ctx, caster.guid) {
                    return Err(CastGateRefusal::new(
                        CastRefusalKind::CannotAct,
                        CastRefusalReason::Stunned,
                        format!("caster {} cannot act (stun/poly/fear)", caster.guid),
                    ));
                }
                Ok(())
            }
            CastGate::Tame => {
                if has_effect(E_TAME_CREATURE) {
                    crate::creatures::validate_tame(ctx, caster, target_guid)?;
                }
                Ok(())
            }
            CastGate::Guardian => effects
                .iter()
                .filter(|effect| effect.kind == E_SUMMON_GUARDIANS)
                .try_for_each(|effect| crate::creatures::guardian::check(ctx, caster, effect)),
            CastGate::SummonTarget => self.check_summon_target(),
            CastGate::EffectData => check_effect_data(ctx, spell),
        }
    }

    fn range_leeway_yd(&self) -> f32 {
        range_leeway_yd(self.run.is_strict(), self.caster.is_player())
    }

    fn check_summon_target(&self) -> Result<(), CastGateRefusal> {
        let target_guid = self.request.target_guid;
        if !self
            .spell
            .effects
            .iter()
            .any(|effect| effect.kind == crate::spell::E_SUMMON_PLAYER)
        {
            return Ok(());
        }
        crate::summoning_ritual::summon_refusal(self.ctx, self.caster, target_guid).map_err(
            |reason| {
                CastGateRefusal::other(
                    reason,
                    format!(
                        "spell {} cannot summon {target_guid}",
                        self.request.spell_id
                    ),
                )
            },
        )
    }

    fn check_caster_alive(&self) -> Result<(), CastGateRefusal> {
        let caster = self.caster;
        let allow_dead_creature = match self.run {
            GateRun::InterruptPrecheck(admission)
            | GateRun::Readiness(admission)
            | GateRun::Start(admission) => {
                admission == CreatureSpellCasterAdmission::DeadCreatureCallback
            }
            GateRun::Resolve(_) => {
                dead_callback_cast_admitted(self.ctx, caster.guid, self.request.spell_id)
            }
        };
        if caster_life_allows(caster.is_player(), caster.dead, allow_dead_creature) {
            return Ok(());
        }
        Err(CastGateRefusal::new(
            CastRefusalKind::Dead,
            CastRefusalReason::CasterDead,
            format!("caster {} is dead", caster.guid),
        ))
    }

    fn check_facing(&self) -> Result<(), CastGateRefusal> {
        let caster = self.caster;
        match self.target {
            Some(target)
                if caster.is_player()
                    && self.spell.header.custom_flags & custom::FACE_TARGET != 0
                    && !is_facing(caster.x, caster.y, caster.orientation, target.x, target.y) =>
            {
                Err(CastGateRefusal::other(
                    CastRefusalReason::NotInFront,
                    "target must be in front of caster",
                ))
            }
            _ => Ok(()),
        }
    }

    fn check_behind(&self) -> Result<(), CastGateRefusal> {
        let (caster, hdr, request) = (self.caster, &self.spell.header, self.request);
        let (spell_id, target_guid) = (request.spell_id, request.target_guid);
        if target_guid == caster.guid {
            return Ok(());
        }
        let behind = |target: &WorldEntity| {
            is_behind(caster.x, caster.y, target.x, target.y, target.orientation)
        };
        if let Some(target) = self.target {
            if hdr.custom_flags & custom::BEHIND_TARGET != 0 && !behind(target) {
                return Err(CastGateRefusal::other(
                    CastRefusalReason::NotBehind,
                    "spell must be cast from BEHIND the target",
                ));
            }
        }
        if hdr.cast_flags & CAST_FLAG_REQ_BEHIND != 0 {
            let target = crate::helpers::live_entity(self.ctx, target_guid)
                .map_err(|_| format!("spell {spell_id} target {target_guid} not in world"))?;
            if !behind(&target) {
                return Err(CastGateRefusal::other(
                    CastRefusalReason::NotBehind,
                    format!("spell {spell_id} must be cast from BEHIND the target ({target_guid})"),
                ));
            }
        }
        Ok(())
    }
}

fn check_unit_target(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
    spell_id: u32,
) -> Result<(), CastGateRefusal> {
    check_taxi_target(ctx, caster, effects, target_guid)?;
    check_target_faction(ctx, caster, effects, target_guid, spell_id)
}

fn check_taxi_target(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
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
    Ok(())
}

fn check_target_faction(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    target_guid: u64,
    spell_id: u32,
) -> Result<(), CastGateRefusal> {
    // Target kinds determine faction eligibility; is_negative only describes aura polarity.
    if target_guid == caster.guid {
        return Ok(());
    }
    // Inert DBC markers can name ally targets even when the spell is hostile.
    let hits_enemy = effects.iter().any(|e| {
        e.kind != A_FLAG
            && e.kind != E_DUEL
            && matches!(e.target, T_TARGET_ENEMY | T_AREA_ENEMY | T_CHAIN_ENEMY)
    });
    let hits_ally = effects.iter().any(|e| {
        e.kind != A_FLAG && e.kind != E_DUEL && matches!(e.target, T_TARGET_ALLY | T_AREA_ALLY)
    });
    if !(hits_enemy || hits_ally) {
        return Ok(());
    }
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return Ok(());
    };
    let friendly = !crate::combat::may_harm(ctx, caster, &target);
    let hostile = !crate::combat::may_help(ctx, caster, &target);
    match faction_target_violation(hits_enemy, hits_ally, friendly, hostile) {
        Some(class) => Err(CastGateRefusal::other(
            CastRefusalReason::BadTargets,
            format!("spell {spell_id} targets {class}, so it cannot be cast on unit {target_guid}"),
        )),
        None => Ok(()),
    }
}

fn is_humanoid(ctx: &ReducerContext, target: &WorldEntity) -> bool {
    target.is_player()
        || ctx
            .db
            .game_creature_template()
            .entry()
            .find(target.entry)
            .is_some_and(|t| t.creature_type == HUMANOID_TYPE)
}

// vmangos `SpellCaster::IsSpellReady`: the spell's own cooldown, then its school lockout.
fn check_spell_ready(
    ctx: &ReducerContext,
    caster_guid: u64,
    hdr: &Spell,
    request: &CastRequest,
) -> Result<(), CastGateRefusal> {
    let spell_id = hdr.spell_id;
    if hdr.attributes & SPELL_ATTR_PASSIVE == 0
        && hdr.cast_flags & CAST_FLAG_RANGED_AUTO_REPEAT == 0
        && !crate::items::equip::spell_ready(
            ctx,
            caster_guid,
            spell_id,
            request.cooldown_category.unwrap_or(hdr.category),
        )
    {
        return Err(CastGateRefusal::new(
            CastRefusalKind::Cooldown,
            CastRefusalReason::NotReady,
            "spell not ready (cooldown)".to_string(),
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
    Ok(())
}

fn check_form(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
) -> Result<(), CastGateRefusal> {
    let (hdr, spell_id) = (&spell.header, spell.header.spell_id);
    if crate::form::locked(ctx, caster.guid)
        && spell
            .effects
            .iter()
            .any(|effect| effect.kind == lyracore_shared::spell::taxonomy::A_SHAPESHIFT)
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::NotShapeshift,
            format!("spell {spell_id} cannot change a Form an Aura locks"),
        ));
    }
    let form = crate::form::shapeshift_state(ctx, caster);
    let requirement =
        crate::spell::FormRequirement::of(ctx, spell_id, form.map(|form| form.form_id));
    if let Some(refusal) = hdr.form_refusal(requirement, form) {
        let reason = match refusal {
            FormRefusal::NotShapeshift => CastRefusalReason::NotShapeshift,
            FormRefusal::OnlyShapeshift => CastRefusalReason::WrongStance,
        };
        return Err(CastGateRefusal::other(
            reason,
            format!("spell {spell_id} cannot be cast in the caster's current form ({form:?})"),
        ));
    }
    Ok(())
}

fn check_react_window(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    hdr: &Spell,
) -> Result<(), CastGateRefusal> {
    let spell_id = hdr.spell_id;
    let now_ms = crate::time::now_ms(ctx);
    if hdr.cast_flags & CAST_FLAG_REQ_OVERPOWER != 0
        && !crate::combat::react_window_active(caster.overpower_until_ms, now_ms)
    {
        return Err(CastGateRefusal::other(
            CastRefusalReason::ReactWindowClosed,
            format!(
                "spell {spell_id} requires an Overpower window (the target must have dodged your swing)"
            ),
        ));
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
    Ok(())
}

fn check_dagger(
    ctx: &ReducerContext,
    caster_guid: u64,
    hdr: &Spell,
) -> Result<(), CastGateRefusal> {
    if hdr.cast_flags & CAST_FLAG_REQ_DAGGER == 0 {
        return Ok(());
    }
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
        return Err(CastGateRefusal::other(
            CastRefusalReason::Unreported,
            format!(
                "spell {} requires a dagger equipped in the main hand (caster {caster_guid})",
                hdr.spell_id
            ),
        ));
    }
    Ok(())
}

// vmangos `Spell::CheckPower`: combo points, then power.
fn check_power(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    request: &CastRequest,
) -> Result<(), CastGateRefusal> {
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    let (spell_id, target_guid, caster_guid) = (hdr.spell_id, request.target_guid, caster.guid);
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

    // vmangos `Spell::CheckPower` passes a cast an item starts.
    let power = caster.spendable_power(hdr.power_type);
    if !request.is_item_cast() && cost_gate_blocks(caster.is_player(), power, hdr.cost) {
        return Err(CastGateRefusal::new(
            CastRefusalKind::InsufficientPower,
            cost_refusal_reason(hdr.power_type),
            format!("not enough power: have {power}, need {}", hdr.cost),
        ));
    }
    Ok(())
}

fn check_effect_data(ctx: &ReducerContext, spell: &LoadedSpell) -> Result<(), CastGateRefusal> {
    let (spell_id, effects) = (spell.header.spell_id, spell.effects.as_slice());
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
    Ok(())
}

fn check_global_cooldown(
    ctx: &ReducerContext,
    caster_guid: u64,
    hdr: &Spell,
) -> Result<(), CastGateRefusal> {
    let now = crate::time::now(ctx);
    if ctx
        .db
        .game_global_cooldown()
        .by_caster_category()
        .filter((caster_guid, hdr.gcd_category))
        .any(|cooldown| now < cooldown.ready_at)
    {
        return Err(CastGateRefusal::new(
            CastRefusalKind::Cooldown,
            CastRefusalReason::NotReady,
            "spell not ready (global cooldown)".to_string(),
        ));
    }
    Ok(())
}

fn start_global_cooldown(ctx: &ReducerContext, caster_guid: u64, hdr: &Spell) {
    if hdr.gcd_ms == 0 {
        return;
    }
    let now = crate::time::now(ctx);
    let ready_at = now
        .checked_add(TimeDuration::from_micros(i64::from(hdr.gcd_ms) * 1000))
        .unwrap_or(now);
    let cooldowns = ctx.db.game_global_cooldown();
    match cooldowns
        .by_caster_category()
        .filter((caster_guid, hdr.gcd_category))
        .next()
    {
        Some(cooldown) => {
            cooldowns.id().update(GlobalCooldown {
                ready_at,
                ..cooldown
            });
        }
        None => {
            cooldowns.insert(GlobalCooldown {
                id: 0,
                caster_guid,
                category: hdr.gcd_category,
                ready_at,
            });
        }
    }
}

fn targets_unit(target: u8) -> bool {
    matches!(
        target,
        T_TARGET_ENEMY | T_TARGET_ALLY | T_TARGET_ANY | T_AREA_ENEMY | T_AREA_ALLY | T_CHAIN_ENEMY
    )
}

const SPELL_ATTR_EX2_IGNORE_LINE_OF_SIGHT: u32 = 0x4;

fn targeted_los_applies(
    caster_guid: u64,
    target_guid: u64,
    hdr: &Spell,
    effects: &[SpellEffect],
) -> bool {
    target_guid != 0
        && target_guid != caster_guid
        && hdr.attributes_ex_2 & SPELL_ATTR_EX2_IGNORE_LINE_OF_SIGHT == 0
        && effects.iter().any(|effect| targets_unit(effect.target))
}

fn check_targeted_los(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    target_guid: u64,
) -> Result<(), CastGateRefusal> {
    if !targeted_los_applies(caster.guid, target_guid, &spell.header, &spell.effects) {
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

// vmangos `Spell::CheckRange` gives a completion 5 yd more for a player and 2.25 yd for a creature.
fn range_leeway_yd(strict: bool, caster_is_player: bool) -> f32 {
    match (strict, caster_is_player) {
        (true, _) => CAST_RANGE_LEEWAY_YD,
        (false, true) => CAST_RANGE_LEEWAY_YD + 5.0,
        (false, false) => CAST_RANGE_LEEWAY_YD + 2.25,
    }
}

fn check_ground_point_los(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    dest: Option<(f32, f32, f32)>,
) -> Result<(), CastGateRefusal> {
    let point = ground_point(dest)?;
    let caster_at = (caster.x, caster.y, caster.z);
    if !crate::nav::has_los(ctx, caster.map_id, caster.instance_id, caster_at, point) {
        return Err(CastGateRefusal::new(
            CastRefusalKind::NoLineOfSight,
            CastRefusalReason::NoLineOfSight,
            "ground point is not in line of sight".to_string(),
        ));
    }
    Ok(())
}

fn ground_point(dest: Option<(f32, f32, f32)>) -> Result<(f32, f32, f32), CastGateRefusal> {
    dest.ok_or_else(|| {
        CastGateRefusal::new(
            CastRefusalKind::BadTargets,
            CastRefusalReason::BadTargets,
            "spell can only target a ground point".to_string(),
        )
    })
}

fn check_cast_range(
    what: &str,
    dist: f32,
    range_yd: u32,
    leeway_yd: f32,
) -> Result<(), CastGateRefusal> {
    if dist > range_yd as f32 + leeway_yd {
        return Err(CastGateRefusal::new(
            CastRefusalKind::OutOfRange,
            CastRefusalReason::OutOfRange,
            format!("{what} out of range ({dist:.1} yd > {range_yd} + {leeway_yd:.1} yd leeway)"),
        ));
    }
    Ok(())
}

fn check_target_aura_state(
    ctx: &ReducerContext,
    spell: &LoadedSpell,
    caster_guid: u64,
    target_guid: u64,
) -> Result<(), CastGateRefusal> {
    let (hdr, effects) = (&spell.header, spell.effects.as_slice());
    let Some(target) = (target_guid != 0)
        .then(|| ctx.db.game_world_entity().guid().find(target_guid))
        .flatten()
    else {
        return Ok(());
    };
    let refusal = |immune_to: &str| {
        Err(CastGateRefusal::other(
            CastRefusalReason::TargetAuraState,
            format!(
                "spell {} is refused: unit {target_guid} is immune to its {immune_to}",
                hdr.spell_id
            ),
        ))
    };
    if crate::spell::refuses_helpful_spell(ctx, caster_guid, target_guid, hdr, effects) {
        return refusal("mechanic");
    }
    if !crate::spell::spell_is_harmful(hdr, effects)
        && crate::spell::school_refuses_spell(ctx, caster_guid, &target, hdr, false)
    {
        return refusal("school");
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

// vmangos Spell::CheckItems: a cast item in trade, not carried or out of charges fails the cast.
fn check_cast_item(
    ctx: &ReducerContext,
    caster_guid: u64,
    cast_item: Option<u64>,
) -> Result<(), CastGateRefusal> {
    let Some(item_guid) = cast_item else {
        return Ok(());
    };
    let carried = ctx
        .db
        .game_item_instance()
        .guid()
        .find(item_guid)
        .is_some_and(|item| {
            item.owner_guid == caster_guid && crate::items::is_carried_slot(item.slot)
        });
    if !carried || crate::trade::item_is_offered(ctx, caster_guid, item_guid) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::ItemGone,
            format!("the cast item {item_guid} is gone"),
        ));
    }
    if !crate::items::item_charges_remain(ctx, item_guid) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::NoChargesRemain,
            format!("the cast item {item_guid} has no charges left"),
        ));
    }
    Ok(())
}

/// vmangos `Spell::CheckItems`: effect 0's product must fit; its fewest count draws no roll.
fn product_storage(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &LoadedSpell,
    level: u8,
) -> Result<(), lyracore_shared::item::ItemRefusal> {
    let Some(effect) = spell
        .effects
        .iter()
        .find(|effect| effect.effect_index == 0 && effect.kind == E_CREATE_ITEM)
        .filter(|_| caster.is_player())
    else {
        return Ok(());
    };
    let hdr = &spell.header;
    let fewest = effect_amount(
        effect.base_points,
        effect.die_sides,
        effect.per_level,
        u32::from(level),
        hdr.spell_level,
        hdr.max_level,
        0,
    )
    .max(1) as u32;
    let product = ctx
        .db
        .game_item_template()
        .entry()
        .find(effect.p0.max(0) as u32)
        .ok_or(lyracore_shared::item::ItemRefusal::ItemNotFound)?;
    crate::items::ItemStoragePlan::read(ctx, caster.guid, caster.owner_identity)
        .can_store_new(ctx, &product, fewest)
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
    TriggerLink::check(ctx.timestamp, spell_id)?;

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
    start_global_cooldown(ctx, caster_guid, hdr);
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
mod ground_point_tests {
    use super::*;

    fn reach(dist: f32, strict: bool, caster_is_player: bool) -> Result<(), CastGateRefusal> {
        check_cast_range(
            "ground point",
            dist,
            30,
            range_leeway_yd(strict, caster_is_player),
        )
    }

    #[test]
    fn a_distract_without_a_ground_point_is_a_bad_target() {
        let refusal = ground_point(None).unwrap_err();
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
        assert!(reach(30.0, true, true).is_ok());
    }

    #[test]
    fn a_ground_point_inside_the_leeway_passes() {
        assert!(reach(34.6, true, true).is_ok());
    }

    #[test]
    fn a_ground_point_beyond_the_leeway_is_out_of_range() {
        let refusal = reach(34.7, true, true).unwrap_err();
        assert_eq!(refusal.refusal.kind, CastRefusalKind::OutOfRange);
        assert_eq!(refusal.reason, CastRefusalReason::OutOfRange);
        assert!(
            refusal.refusal.message.contains("out of range"),
            "{refusal}"
        );
    }

    #[test]
    fn a_player_completion_reaches_5_yd_further_than_its_start() {
        assert!(reach(39.6, false, true).is_ok());
        assert!(reach(39.7, false, true).is_err());
    }

    #[test]
    fn a_creature_completion_reaches_2_25_yd_further_than_its_start() {
        assert!(reach(34.6, true, false).is_ok());
        assert!(reach(34.7, true, false).is_err());
        assert!(reach(36.9, false, false).is_ok());
        assert!(reach(37.0, false, false).is_err());
    }
}

#[cfg(test)]
mod gate_order_tests {
    use super::*;
    use CastGate as G;

    const LIVING: CreatureSpellCasterAdmission = CreatureSpellCasterAdmission::Living;
    const START: GateRun = GateRun::Start(LIVING);
    const COMPLETION: GateRun = GateRun::Resolve(CastPhase::Completion);

    fn order(run: GateRun) -> Vec<CastGate> {
        cast_gate_order(run).collect()
    }

    fn assert_before(run: GateRun, first: CastGate, later: &[CastGate]) {
        let order = order(run);
        let at = |gate| order.iter().position(|&g| g == gate);
        let first_at = at(first).unwrap_or_else(|| panic!("{run:?} skips {first:?}"));
        for &gate in later {
            let later_at = at(gate).unwrap_or_else(|| panic!("{run:?} skips {gate:?}"));
            assert!(
                first_at < later_at,
                "{run:?} checks {gate:?} before {first:?}"
            );
        }
    }

    #[test]
    fn a_start_checks_cooldown_then_death_then_the_global_cooldown_first() {
        assert_eq!(
            order(START)[..4],
            [
                G::SpellReady,
                G::CasterAlive,
                G::CasterLevel,
                G::GlobalCooldown
            ]
        );
        assert_before(
            START,
            G::GlobalCooldown,
            &[G::Mount, G::Behind, G::Facing, G::CasterControl, G::Tame],
        );
    }

    #[test]
    fn a_non_combat_spell_in_combat_is_refused_before_its_form() {
        assert_before(START, G::NonCombat, &[G::Form, G::Mount]);
    }

    #[test]
    fn form_and_stealth_come_before_every_target_gate() {
        for gate in [G::Form, G::Stealth] {
            assert_before(
                START,
                gate,
                &[G::TargetLevel, G::LineOfSight, G::TargetImmunity, G::Range],
            );
        }
    }

    #[test]
    fn target_level_taxi_line_of_sight_and_faction_come_before_immunity() {
        assert_before(START, G::TargetLevel, &[G::TaxiTarget, G::LineOfSight]);
        assert_before(START, G::TaxiTarget, &[G::LineOfSight]);
        assert_before(START, G::LineOfSight, &[G::TargetFaction]);
        assert_before(START, G::TargetFaction, &[G::TargetImmunity]);
        assert_before(START, G::TargetImmunity, &[G::Behind]);
    }

    #[test]
    fn line_of_sight_comes_before_items_range_power_and_caster_control() {
        for run in [START, COMPLETION] {
            assert_before(
                run,
                G::LineOfSight,
                &[
                    G::Behind,
                    G::Dagger,
                    G::RangedWeapon,
                    G::Range,
                    G::Facing,
                    G::CasterControl,
                ],
            );
        }
        assert_before(START, G::LineOfSight, &[G::Power]);
    }

    #[test]
    fn items_come_before_range_and_range_before_facing() {
        assert_before(START, G::Behind, &[G::Dagger, G::ItemTarget, G::Range]);
        for run in [START, COMPLETION] {
            assert_before(run, G::CastItem, &[G::SpellFocus, G::ItemTarget, G::Range]);
            assert_before(run, G::SpellFocus, &[G::ProductStorage]);
            assert_before(run, G::ProductStorage, &[G::ItemTarget, G::Range]);
        }
        assert_before(
            START,
            G::ItemTarget,
            &[G::Range, G::Power, G::CasterControl],
        );
        assert_before(START, G::Range, &[G::Facing, G::GroundPointRange]);
        assert_before(START, G::GroundPointLineOfSight, &[G::GroundPointRange]);
    }

    #[test]
    fn power_comes_before_caster_control_and_the_effect_gates() {
        assert_before(START, G::GroundPointRange, &[G::Power]);
        assert_before(START, G::Power, &[G::CasterControl]);
        assert_before(
            START,
            G::CasterControl,
            &[G::Tame, G::Guardian, G::SummonTarget, G::EffectData],
        );
    }

    #[test]
    fn a_completion_checks_power_before_every_other_gate() {
        let order = order(COMPLETION);
        assert_eq!(order[0], G::Power);
        assert_eq!(order.iter().filter(|&&g| g == G::Power).count(), 1);
    }

    #[test]
    fn a_completion_skips_the_checks_vmangos_runs_only_when_strict() {
        let order = order(COMPLETION);
        for gate in [
            G::GlobalCooldown,
            G::NonCombat,
            G::Form,
            G::Stealth,
            G::PeacefulTarget,
        ] {
            assert!(!order.contains(&gate), "completion checks {gate:?}");
            assert!(
                cast_gate_order(START).any(|g| g == gate),
                "start skips {gate:?}"
            );
        }
        assert!(!COMPLETION.is_strict());
        assert!(GateRun::Resolve(CastPhase::Start).is_strict());
    }

    #[test]
    fn a_trigger_child_cast_skips_the_gates_vmangos_runs_only_for_an_untriggered_spell() {
        let trigger = order(GateRun::Resolve(CastPhase::Trigger));
        for gate in [
            G::SpellReady,
            G::CasterAlive,
            G::GlobalCooldown,
            G::NonCombat,
            G::Form,
            G::Stealth,
            G::PeacefulTarget,
            G::ProductStorage,
            G::LineOfSight,
            G::Range,
            G::Facing,
            G::GroundPointRange,
            G::Power,
            G::CasterControl,
        ] {
            assert!(
                !trigger.contains(&gate),
                "a Trigger Child Cast checks {gate:?}"
            );
        }
        assert!(GateRun::Resolve(CastPhase::Trigger).is_strict());
    }

    #[test]
    fn both_phases_keep_one_order() {
        let moved = [
            G::GlobalCooldown,
            G::NonCombat,
            G::Form,
            G::Stealth,
            G::PeacefulTarget,
            G::Power,
        ];
        let start: Vec<_> = order(START)
            .into_iter()
            .filter(|gate| !moved.contains(gate))
            .collect();
        let completion: Vec<_> = order(COMPLETION)
            .into_iter()
            .filter(|gate| !moved.contains(gate))
            .collect();
        assert_eq!(start, completion);
    }

    #[test]
    fn each_run_checks_only_the_gates_its_request_can_answer() {
        assert_eq!(order(START), CHECK_CAST_ORDER);
        let precheck = order(GateRun::InterruptPrecheck(LIVING));
        let readiness = order(GateRun::Readiness(LIVING));
        let instant_resolve = order(GateRun::Resolve(CastPhase::Start));
        for gate in [
            G::ItemTarget,
            G::GroundPointLineOfSight,
            G::GroundPointRange,
        ] {
            assert!(
                !precheck.contains(&gate) && !readiness.contains(&gate),
                "{gate:?}"
            );
            assert!(instant_resolve.contains(&gate), "{gate:?}");
        }
        assert!(!precheck.contains(&G::LineOfSight));
        assert!(readiness.contains(&G::LineOfSight));
        assert!(!instant_resolve.contains(&G::LineOfSight));
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

    #[test]
    fn a_spell_that_ignores_line_of_sight_skips_the_check() {
        let effects = [SpellEffect {
            target: T_TARGET_ENEMY,
            ..SpellEffect::blank(133, 0)
        }];
        let mut header = Spell::blank(133, "Fixture");
        assert!(targeted_los_applies(1, 2, &header, &effects));
        assert!(!targeted_los_applies(1, 1, &header, &effects));
        header.attributes_ex_2 = SPELL_ATTR_EX2_IGNORE_LINE_OF_SIGHT;
        assert!(!targeted_los_applies(1, 2, &header, &effects));
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
