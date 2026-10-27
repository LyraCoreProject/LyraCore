use crate::spell::source_behavior;
use lyracore_shared::constants::tracer_spell;
use lyracore_shared::spell_custom as custom;

use spacetimedb::{log, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use crate::{game_character, game_creature_template, game_world_entity};

use crate::spell::*;

use crate::skilldata::game_skill_ability;

use crate::script_binding::game_script;

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
    fn heal(healed: u32) -> Self {
        Self {
            healed,
            ..Self::default()
        }
    }
}

/// An absent target counts as equal level.
pub(crate) fn spell_misses(ctx: &ReducerContext, caster_level: u8, target_guid: u64) -> bool {
    let target_level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .map(|t| t.level)
        .unwrap_or(caster_level as u32);
    (ctx.random::<u32>() % 10_000) < spell_miss_chance_bp(caster_level as u32, target_level)
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
const AOE_MAX_TARGETS: usize = 20;

fn aoe_keep(
    d2: f32,
    r2: f32,
    want_enemy: bool,
    is_caster: bool,
    hostile: bool,
    friendly: bool,
) -> bool {
    if d2 > r2 {
        return false;
    }
    if want_enemy {
        !is_caster && hostile
    } else {
        is_caster || friendly
    }
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
) -> Vec<u64> {
    let one = if explicit == 0 { caster_guid } else { explicit };
    match e.target {
        T_SELF => vec![caster_guid],
        T_TARGET_ENEMY | T_TARGET_ALLY | T_TARGET_ANY => vec![one],
        T_AREA_ENEMY | T_AREA_ALLY | T_CHAIN_ENEMY => {
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
            let radius = if e.radius_yd > 0.0 {
                e.radius_yd
            } else {
                MELEE_PBAOE_RADIUS_YD
            };
            let r2 = radius * radius;
            let want_enemy = matches!(e.target, T_AREA_ENEMY | T_CHAIN_ENEMY);

            let mut hits: Vec<u64> = entities
                .iter()
                .filter(|c| c.map_id == a_map && c.instance_id == a_inst && !c.dead)
                .filter_map(|c| {
                    let (dx, dy, dz) = (c.x - ax, c.y - ay, c.z - az);
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 > r2 {
                        return None;
                    }
                    let (hostile, friendly) = if want_enemy {
                        (crate::combat::is_hostile_target(ctx, &caster, &c), false)
                    } else {
                        (false, crate::combat::may_help(ctx, &caster, &c))
                    };
                    aoe_keep(d2, r2, want_enemy, c.guid == caster_guid, hostile, friendly)
                        .then_some(c.guid)
                })
                .collect();

            // Missing faction data can make the caster appear unfriendly to itself.
            if !want_enemy && !hits.contains(&caster_guid) {
                hits.push(caster_guid);
            }
            let cap = if e.target == T_CHAIN_ENEMY && e.chain_targets > 0 {
                e.chain_targets as usize
            } else {
                AOE_MAX_TARGETS
            };
            hits.truncate(cap);
            // Falling back to the explicit target on an empty enemy AoE would damage the caster.
            hits
        }
        _ => {
            let _ = ctx;
            vec![one]
        }
    }
}

fn next_stacks(current: u8, max_stacks: u8) -> u8 {
    (current.max(1) + 1).min(max_stacks.max(1))
}

// The 1.12 client chooses buff or debuff frames by slot index rather than aura flags.
fn pick_aura_slot(
    existing: &[(u8, u32)],
    spell_id: u32,
    is_negative: bool,
    buff_slot_count: u8,
    slot_count: u8,
) -> Option<u8> {
    if let Some(&(slot, _)) = existing.iter().find(|(_, sp)| *sp == spell_id) {
        return Some(slot); // All effects of one spell share one client icon.
    }
    let mut range = if is_negative {
        buff_slot_count..slot_count
    } else {
        0..buff_slot_count
    };
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
    shed_auras(ctx, target_guid, displaced);
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
) {
    let now = ctx.timestamp;
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
        source_behavior::bonus_magnitude(points, power, e.bonus_coefficient, hdr.custom_flags, true)
    } else {
        points
    };
    let duration_ms = if hdr.cast_flags & SPELL_ATTR_COMBO_FINISHER != 0 {
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
    let period_ms = if e.period_ms == 0 && hdr.cast_flags & SPELL_ATTR_CHANNELED != 0 {
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
    if !hdr.name.is_empty() && !crate::ghost::is_ghost_spell(hdr.spell_id) {
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
    if !coexist && !cross_caster_stack {
        let replaced = auras
            .by_target()
            .filter(&target_guid)
            .filter(|a| a.effect_id == e.id && a.caster_guid != caster_guid)
            .collect();
        shed_auras(ctx, target_guid, replaced);
    }
    let existing = auras
        .by_target()
        .filter(&target_guid)
        .find(|a| a.effect_id == e.id && (cross_caster_stack || a.caster_guid == caster_guid));
    let amount_remaining = if e.kind == A_ABSORB { points.max(0) } else { 0 };
    let channel_target =
        if e.kind == A_PERIODIC_TRIGGER || hdr.cast_flags & SPELL_ATTR_CHANNELED != 0 {
            cast_target_guid
        } else {
            0
        };
    // p1 holds linked debuffs, so trigger auras freeze trigger_spell in eff_p1.
    let eff_p1 = if e.kind == A_PERIODIC_TRIGGER || e.kind == A_PROC_TRIGGER {
        e.trigger_spell as i32
    } else {
        e.p1
    };
    // Refreshing refills proc charges but preserves any running internal cooldown.
    let proc = crate::spell::proc::freeze_profile(ctx, e.kind, hdr);
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
        if !crate::spell::stacking::apply_group_conflict(
            ctx,
            hdr.spell_id,
            caster_guid,
            target_guid,
            crate::spell::stacking::rank_of(ctx, hdr.spell_id),
            points.saturating_abs(),
        ) {
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
        let Some(slot) = pick_aura_slot(
            &existing,
            hdr.spell_id,
            hdr.negative(),
            tracer_spell::BUFF_SLOT_COUNT,
            tracer_spell::AURA_SLOTS,
        ) else {
            log::info!(
                "aura slot overflow: all {} slots on {target_guid} full — dropping spell {} (loud: relayed to caster {caster_guid})",
                tracer_spell::AURA_SLOTS,
                hdr.spell_id
            );
            crate::spell::SpellCastEvent::insert(
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
        .insert(ctx);
    }

    source_behavior::finish_aura_apply(ctx, hdr, e, caster_guid, target_guid);

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

    if aura_moves_vitals(e.kind, e.p0) {
        recompute_vitals(ctx, target_guid);
    }
    if crate::spell::aura_moves_sheet(e.kind, e.p0) {
        recompute_sheet(ctx, target_guid);
    }
    if crate::mount::mount_aura_moves_mount(e.kind, e.p0) {
        crate::mount::recompute_mount(ctx, target_guid);
    }

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
        if caster_is_player && target_is_creature {
            crate::threat::add_threat(ctx, target_guid, caster_guid, (level as i64) * 5);
        }
    }
    if e.kind == A_TRANSFORM {
        recompute_display(ctx, target_guid);
    }
}

pub(crate) fn apply_linked_debuff(
    ctx: &ReducerContext,
    linked_spell_id: u32,
    caster_guid: u64,
    target_guid: u64,
    level: u8,
) {
    let Some(linked_hdr) = ctx.db.game_spell().spell_id().find(linked_spell_id) else {
        log::info!("linked debuff {linked_spell_id} has no game_spell header — skipped");
        return;
    };
    for e in ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&linked_spell_id)
        .filter(|e| e.kind & KIND_AURA_BIT != 0)
    {
        let points = effect_amount(
            e.base_points,
            e.die_sides,
            e.per_level,
            level as u32,
            linked_hdr.spell_level,
            linked_hdr.max_level,
            ctx.random::<u32>(),
        );
        aura_apply(
            ctx,
            &e,
            &linked_hdr,
            caster_guid,
            target_guid,
            target_guid,
            level,
            points,
        );
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
        EffectKind::Charge
        | EffectKind::Blink
        | EffectKind::RecallHome
        | EffectKind::Dismount
        | EffectKind::PersistentArea => {
            apply_reposition_effect(ctx, e, hdr, caster_guid, target_guid, points, dest)
        }
        EffectKind::CreateItem | EffectKind::Pickpocket => {
            apply_inventory_effect(ctx, e, caster_guid, target_guid, points)
        }
        EffectKind::Resurrect
        | EffectKind::SummonPet
        | EffectKind::TameCreature
        | EffectKind::SetStance => apply_character_effect(ctx, e, caster_guid, target_guid, points),
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
            let removed = dispel_target(ctx, target_guid, category);
            log::info!("dispel (cat {category}) on {target_guid} removed {removed} debuff(s)");
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
        | EffectKind::AuraControl
        | EffectKind::AuraImmunity
        | EffectKind::AuraModDetectRange
        | EffectKind::AuraMounted
        | EffectKind::AuraProcDamage
        | EffectKind::AuraSelfResurrect
        | EffectKind::AuraTransform
        | EffectKind::AuraModPowerRegenPct
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
    if e.kind == A_CONTROL
        && e.p0_kind == P_MECHANIC
        && is_immune_to_mechanic(ctx, target_guid, e.p0)
    {
        log::info!(
            "spell {} A_CONTROL(mechanic {}) refused — target {target_guid} is immune",
            e.spell_id,
            e.p0
        );
        return EffectHit::none();
    }
    aura_apply(
        ctx,
        e,
        hdr,
        caster_guid,
        target_guid,
        cast_target_guid,
        level,
        points,
    );
    if e.p1 != 0 {
        apply_linked_debuff(ctx, e.p1 as u32, caster_guid, target_guid, level);
    }
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
        E_DAMAGE => {
            let sp = spell_power(ctx, caster_guid, hdr.school_mask);
            let scaled = source_behavior::bonus_magnitude(
                points,
                sp,
                e.bonus_coefficient,
                hdr.custom_flags,
                false,
            );
            let (dmg_flat, dmg_pct) =
                crate::spell::math::spell_mod(ctx, caster_guid, SPELLMOD_OP_DAMAGE, hdr);
            let scaled = if hdr.custom_flags & custom::FIXED_DAMAGE != 0 {
                scaled
            } else {
                crate::spell::math::apply_spell_mod(scaled.max(0) as u32, dmg_flat, dmg_pct) as i32
            };
            let is_crit = (ctx.random::<u32>() % 10_000) < spell_crit_bp(ctx, caster_guid);
            let crit_scaled = apply_spell_crit(scaled, is_crit);
            let after_resist =
                source_behavior::mitigate_spell(ctx, hdr, caster_guid, target_guid, crit_scaled);
            let resisted = crit_scaled.saturating_sub(after_resist).max(0) as u32;

            if hdr.range_yd > 0 && target_guid != caster_guid {
                if let (Some(caster), Some(target)) = (
                    ctx.db.game_world_entity().guid().find(caster_guid),
                    ctx.db.game_world_entity().guid().find(target_guid),
                ) {
                    if caster.map_id == target.map_id && caster.instance_id == target.instance_id {
                        let dist =
                            distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z);
                        let travel_ms = projectile_travel_ms(dist, PROJECTILE_SPEED_YPS);
                        if travel_ms > 0 {
                            let fire_at = ctx
                                .timestamp
                                .checked_add(TimeDuration::from_micros((travel_ms as i64) * 1000))
                                .unwrap_or(ctx.timestamp);
                            ctx.db
                                .game_pending_spell_impact()
                                .insert(PendingSpellImpact {
                                    scheduled_id: 0,
                                    scheduled_at: ScheduleAt::Time(fire_at),
                                    caster_guid,
                                    target_guid,
                                    spell_id: hdr.spell_id,
                                    school_index: crate::spell::school_index(hdr.school_mask),
                                    is_crit,
                                    resisted,
                                    after_resist,
                                    triggered,
                                });
                            // Impact emits the damage log; this GO must carry no damage.
                            return EffectHit::none();
                        }
                    }
                }
            }
            let (dealt, absorbed) = apply_target_damage(
                ctx,
                target_guid,
                caster_guid,
                after_resist,
                hit_of(hdr, triggered, crate::combat::HitSource::Spell, is_crit),
            );
            EffectHit {
                dealt,
                resisted,
                absorbed,
                crit: is_crit,
                healed: 0,
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
                crate::combat::launch_ranged_strike(
                    ctx,
                    &caster,
                    &target,
                    dmg,
                    hdr.spell_id,
                    strike_crit,
                );
                return EffectHit::none();
            }
            let (dealt, absorbed) = apply_target_damage(
                ctx,
                target_guid,
                caster_guid,
                dmg as i32,
                hit_of(
                    hdr,
                    triggered,
                    crate::combat::HitSource::MeleeSpell,
                    strike_crit,
                ),
            );
            EffectHit {
                crit: strike_crit,
                ..EffectHit::dmg(dealt, absorbed)
            }
        }
        E_JUDGEMENT => {
            let seal = seal_amount(ctx, caster_guid);
            if seal > 0 {
                let (dealt, absorbed) = apply_target_damage(
                    ctx,
                    target_guid,
                    caster_guid,
                    seal,
                    hit_of(hdr, triggered, crate::combat::HitSource::Spell, false),
                );
                remove_seal_auras(ctx, caster_guid);
                EffectHit::dmg(dealt, absorbed)
            } else {
                EffectHit::none()
            }
        }
        E_FINISHER_DAMAGE => {
            let entities = ctx.db.game_world_entity();
            let (Some(caster), Some(target)) = (
                entities.guid().find(caster_guid),
                entities.guid().find(target_guid),
            ) else {
                return EffectHit::none();
            };
            let dmg = crate::combat::finisher_strike_damage(
                ctx,
                &caster,
                &target,
                hdr,
                points,
                strike_crit,
            );
            let (dealt, absorbed) = apply_target_damage(
                ctx,
                target_guid,
                caster_guid,
                dmg as i32,
                hit_of(
                    hdr,
                    triggered,
                    crate::combat::HitSource::MeleeSpell,
                    strike_crit,
                ),
            );
            EffectHit {
                crit: strike_crit,
                ..EffectHit::dmg(dealt, absorbed)
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
        E_HEAL => EffectHit::heal(apply_heal(ctx, hdr, e, caster_guid, target_guid, points)),
        E_HEAL_MAX_HEALTH => {
            if let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) {
                let before = t.health;
                t.health = t.max_health;
                let effective = t.health.saturating_sub(before);
                t.update(ctx);
                crate::threat::add_heal_threat(ctx, caster_guid, target_guid, effective);
            }
            EffectHit::none()
        }
        E_ENERGIZE => {
            let mut hit = EffectHit::none();
            if let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) {
                t.power = energized_value(t.power, t.max_power, points);
                // vmangos `EnergizeBySpell` logs the full grant, not the gain.
                hit.energized = points.max(0) as u32;
                hit.power_type = t.power_type();
                t.update(ctx);
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
            let damage = mana_burn_damage(drained, e.p1);
            let (dealt, absorbed) = apply_target_damage(
                ctx,
                target_guid,
                caster_guid,
                damage as i32,
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
            if let Some(mut c) = ctx.db.game_world_entity().guid().find(caster_guid) {
                c.next_swing_spell = e.spell_id;
                c.update(ctx);
            }
            EffectHit::none()
        }
        _ => EffectHit::none(),
    }
}

fn apply_reposition_effect(
    ctx: &ReducerContext,
    e: &SpellEffect,
    hdr: &Spell,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
    dest: Option<(f32, f32, f32)>,
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
            crate::world::recall_to_home(ctx, caster_guid);
            EffectHit::none()
        }
        E_DISMOUNT => {
            crate::mount::dismount(ctx, target_guid);
            EffectHit::none()
        }
        E_PERSISTENT_AREA => {
            create_ground_area(ctx, e, hdr, caster_guid, target_guid, points, dest);
            EffectHit::none()
        }
        _ => EffectHit::none(),
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
            // Vanilla CreateItem grants to the caster regardless of the explicit target.
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
            } else if let Err(err) = crate::items::grant_item(ctx, caster_guid, entry, count) {
                log::info!(
                    "spell {} E_CREATE_ITEM: could not grant {count}x item {entry} to {caster_guid}: {err}",
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
                    crate::loot::roll_pickpocket_loot(ctx, target_entry, target_guid);
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
                if let Err(err) = resolve_cast_at(
                    ctx,
                    caster_guid,
                    e.trigger_spell,
                    level,
                    target_guid,
                    false,
                    false,
                    None,
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
                        created_at: ctx.timestamp,
                    }
                    .insert(ctx);
                }
            }
            EffectHit::none()
        }
        E_SUMMON_PET => {
            let entry = e.p0.max(0) as u32;
            crate::creatures::summon_pet(ctx, caster_guid, entry);
            EffectHit::none()
        }
        E_TAME_CREATURE => {
            if let Err(error) = crate::creatures::tame_creature(ctx, caster_guid, target_guid) {
                log::info!("tame completion for caster {caster_guid} and target {target_guid} failed: {error}");
            }
            EffectHit::none()
        }
        E_SET_STANCE => {
            let new_stance = e.p0.clamp(0, u8::MAX as i32) as u8;
            if let Some(mut c) = ctx.db.game_world_entity().guid().find(caster_guid) {
                if c.stance != new_stance {
                    let power_type = lyracore_shared::packing::power_type::for_class(c.class());
                    let clears_power = stance_switch_clears_power(c.stance, new_stance, power_type);
                    c.stance = new_stance;
                    if clears_power {
                        c.power = 0;
                    }
                    // Preserve byte 3, which carries the ghost visibility flag.
                    let form_id = client_form_for_stance(new_stance) << 16;
                    c.unit_bytes_1 = (c.unit_bytes_1 & 0xFF00_0000) | form_id;
                    let c = c.update(ctx);
                    crate::combat::interrupt_form_bound_queued_strike(ctx, &c);
                } else if form_recast_toggles_off(c.stance, new_stance) {
                    // Druid caster form uses zero; client_form_for_stance(0) is Battle Stance.
                    c.stance = 0;
                    c.unit_bytes_1 &= 0xFF00_0000;
                    let c = c.update(ctx);
                    crate::combat::interrupt_form_bound_queued_strike(ctx, &c);
                }
            }
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
) -> u32 {
    let sp = healing_power(ctx, caster_guid, hdr.school_mask);
    let scaled =
        source_behavior::bonus_magnitude(points, sp, e.bonus_coefficient, hdr.custom_flags, false);
    apply_direct_heal(ctx, caster_guid, target_guid, scaled)
}

pub(crate) fn apply_direct_heal(
    ctx: &ReducerContext,
    healer_guid: u64,
    target_guid: u64,
    basis: i32,
) -> u32 {
    if let Some(mut t) = ctx.db.game_world_entity().guid().find(target_guid) {
        let before = t.health;
        t.health = healed_value(t.health, t.max_health, basis);
        let effective = t.health.saturating_sub(before);
        t.update(ctx);
        crate::threat::add_heal_threat(ctx, healer_guid, target_guid, effective);
        effective
    } else {
        0
    }
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
            crate::world::teleport_player(
                ctx,
                caster_guid,
                caster.map_id,
                caster.instance_id,
                nx,
                ny,
                caster.z,
                caster.orientation,
            );
            return;
        }
        yd -= 2.0;
    }
}

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
    let now = ctx.timestamp;
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
        amount: amount.max(0),
        school_mask: hdr.school_mask,
        period_ms,
        next_tick_micros: now.to_micros_since_unix_epoch() + (period_ms as i64) * 1000,
        expires_at,
    });
    crate::spell::DynamicObject {
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
}

// Vanilla HIGHGUID_DYNAMICOBJECT occupies the top 16 bits as 0xF100.
pub(crate) fn dynamic_object_guid(area_id: u64) -> u64 {
    (0xF100u64 << 48) | area_id
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
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    crate::creatures::tick::emit_move_spline(
        ctx,
        caster_guid,
        (caster.x, caster.y, caster.z),
        (nx, ny, nz),
        duration_ms,
        true,
        now_ms,
        caster.map_id,
        caster.instance_id,
        (caster.grid_x, caster.grid_y),
        None,
    );
    // The spline animates the client; combat needs the authoritative landing position immediately.
    if let Some(mut c) = entities.guid().find(caster_guid) {
        let clock = crate::world::MoveClock::realm(ctx, caster_guid, now_ms);
        c.place(nx, ny, nz, clock).unexplored();
        c.orientation = facing;
        c.update(ctx);
    }
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
