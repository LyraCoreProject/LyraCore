use spacetimedb::{ReducerContext, Table, TimeDuration};

use super::resolve::{CastRequest, LoadedSpell, UnitLogs};
use super::targeting::{apply_effect, roll_spell_damage, SpellDamageRoll};
use crate::geometry::{distance_3d, projectile_travel_ms};
use crate::spell::{CAST_MISS_EVADE, E_DAMAGE, E_FINISHER_DAMAGE, E_JUDGEMENT, E_WEAPON_STRIKE};
use crate::{
    game_pending_spell_impact, game_world_entity, PendingSpellImpact, Spell, SpellEffect,
    SpellImpactEvent, WorldEntity,
};

// `PendingSpellImpact` holds the points of effects 0 to 2, as many as vmangos `MAX_EFFECT_INDEX`.
const IMPACT_EFFECTS: u8 = 3;

/// Each unit a travelling spell reaches after its flight, with what lands there.
pub(super) struct SpellFlights<'a> {
    hdr: &'a Spell,
    request: &'a CastRequest,
    triggered: bool,
    // Each unit asked about, in first-asked order; `None` when the spell reaches it in the cast.
    flights: Vec<(u64, Option<PendingSpellImpact>)>,
}

impl<'a> SpellFlights<'a> {
    pub(super) fn new(hdr: &'a Spell, request: &'a CastRequest, triggered: bool) -> Self {
        Self {
            hdr,
            request,
            triggered,
            flights: Vec::new(),
        }
    }

    /// Whether the effect lands on the unit when the spell arrives, recording it for the impact.
    /// A damage effect rolls its crit, resist and block now.
    pub(super) fn defer(
        &mut self,
        ctx: &ReducerContext,
        e: &SpellEffect,
        unit: u64,
        points: i32,
        weapon_crit: Option<bool>,
    ) -> bool {
        let (hdr, caster_guid) = (self.hdr, self.request.caster_guid);
        if e.effect_index >= IMPACT_EFFECTS || !lands_at_impact(e.kind) {
            return false;
        }
        let Some(impact) = self.flight(ctx, unit) else {
            return false;
        };
        if e.kind == E_DAMAGE {
            let roll = roll_spell_damage(ctx, e, hdr, caster_guid, unit, points, weapon_crit);
            impact.carries_damage = true;
            impact.is_crit |= roll.is_crit;
            impact.after_resist = impact.after_resist.saturating_add(roll.after_resist);
            impact.resisted = impact.resisted.saturating_add(roll.resisted);
            impact.blocked = impact.blocked.saturating_add(roll.blocked);
        } else {
            impact.effect_mask |= 1 << e.effect_index;
            *impact.points_mut(e.effect_index) = points;
        }
        true
    }

    /// Schedules each impact that carries anything, and returns the units they fly to.
    pub(super) fn launch(self, ctx: &ReducerContext) -> Vec<u64> {
        let mut in_flight = Vec::new();
        for impact in self.flights.into_iter().filter_map(|(_, impact)| impact) {
            if carries_anything(&impact) {
                in_flight.push(impact.target_guid);
                ctx.db.game_pending_spell_impact().insert(impact);
            }
        }
        in_flight
    }

    fn flight(&mut self, ctx: &ReducerContext, unit: u64) -> Option<&mut PendingSpellImpact> {
        let at = match self.flights.iter().position(|(guid, _)| *guid == unit) {
            Some(at) => at,
            None => {
                let impact = self.impact_toward(ctx, unit);
                self.flights.push((unit, impact));
                self.flights.len() - 1
            }
        };
        self.flights[at].1.as_mut()
    }

    /// An empty impact due when the spell reaches the unit, or `None` when it lands in the cast.
    fn impact_toward(&self, ctx: &ReducerContext, unit: u64) -> Option<PendingSpellImpact> {
        let (hdr, request) = (self.hdr, self.request);
        if hdr.speed_yps <= 0.0 || unit == request.caster_guid {
            return None;
        }
        let entities = ctx.db.game_world_entity();
        let caster = entities.guid().find(request.caster_guid)?;
        let target = entities.guid().find(unit)?;
        if !crate::helpers::in_same_partition(&target, caster.map_id, caster.instance_id) {
            return None;
        }
        let distance = distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z);
        let travel_ms = projectile_travel_ms(distance, hdr.speed_yps);
        if travel_ms == 0 {
            return None;
        }
        let now = crate::time::now(ctx);
        let due = now
            .checked_add(TimeDuration::from_micros(i64::from(travel_ms) * 1000))
            .unwrap_or(now);
        Some(PendingSpellImpact {
            scheduled_id: 0,
            scheduled_at: crate::time::schedule_time(ctx, due),
            caster_guid: request.caster_guid,
            target_guid: unit,
            spell_id: hdr.spell_id,
            school_index: crate::spell::school_index(hdr.school_mask),
            is_crit: false,
            resisted: 0,
            after_resist: 0,
            triggered: self.triggered,
            blocked: 0,
            carries_damage: false,
            effect_mask: 0,
            level: request.level,
            cast_target_guid: request.target_guid,
            effect_0_points: 0,
            effect_1_points: 0,
            effect_2_points: 0,
        })
    }
}

/// A weapon, Judgement or finisher strike keeps its own path; every other effect waits for impact.
fn lands_at_impact(kind: u8) -> bool {
    !matches!(kind, E_WEAPON_STRIKE | E_JUDGEMENT | E_FINISHER_DAMAGE)
}

fn carries_anything(impact: &PendingSpellImpact) -> bool {
    impact.carries_damage || impact.effect_mask != 0
}

impl PendingSpellImpact {
    fn points_mut(&mut self, effect_index: u8) -> &mut i32 {
        match effect_index {
            0 => &mut self.effect_0_points,
            1 => &mut self.effect_1_points,
            _ => &mut self.effect_2_points,
        }
    }

    fn points(&self, effect_index: u8) -> i32 {
        match effect_index {
            0 => self.effect_0_points,
            1 => self.effect_1_points,
            _ => self.effect_2_points,
        }
    }

    fn lands(&self, e: &SpellEffect) -> bool {
        e.effect_index < IMPACT_EFFECTS && self.effect_mask & (1 << e.effect_index) != 0
    }

    fn damage_roll(&self) -> SpellDamageRoll {
        SpellDamageRoll {
            is_crit: self.is_crit,
            after_resist: self.after_resist,
            resisted: self.resisted,
            blocked: self.blocked,
        }
    }
}

/// Lands a travelling spell on its unit, as vmangos `CheckAtDelay` and `DoAllEffectOnTarget` do.
pub(crate) fn land_spell_impact(ctx: &ReducerContext, impact: &PendingSpellImpact) {
    let entities = ctx.db.game_world_entity();
    let Some(target) = entities.guid().find(impact.target_guid) else {
        return;
    };
    // vmangos lands nothing on a dead unit: it takes no damage, and its Aura holders are refused.
    if target.dead {
        return;
    }
    let caster = entities.guid().find(impact.caster_guid);
    // vmangos finds the unit only on its caster's map.
    if caster.as_ref().is_some_and(|caster| {
        !crate::helpers::in_same_partition(&target, caster.map_id, caster.instance_id)
    }) {
        return;
    }
    let spell = LoadedSpell::load(ctx, impact.spell_id).ok();
    match impact_refusal(ctx, impact, &target, caster.as_ref(), spell.as_ref()) {
        Some(ImpactRefusal::Evade) => {
            crate::native::combat::spell_log_miss(
                ctx,
                impact.caster_guid,
                impact.target_guid,
                impact.spell_id,
                CAST_MISS_EVADE,
            );
            return;
        }
        Some(ImpactRefusal::Immune) => return,
        None => {}
    }
    let mut landed = impact.carries_damage;
    let mut logs = UnitLogs::default();
    // vmangos DoSpellHitOnUnit applies the effects before DoAllEffectOnTarget deals the damage.
    if let Some(LoadedSpell { header, effects }) = &spell {
        for e in effects.iter().filter(|e| impact.lands(e)) {
            let (caster_guid, target_guid) = (impact.caster_guid, impact.target_guid);
            if crate::spell::is_immune_to_effect(ctx, caster_guid, target_guid, header, e) {
                continue;
            }
            let points = impact.points(e.effect_index);
            let hit = apply_effect(
                ctx,
                e,
                header,
                caster_guid,
                target_guid,
                impact.cast_target_guid,
                impact.level,
                points,
                None,
                impact.triggered,
                None,
                None,
            );
            logs.add(target_guid, &hit);
            landed = true;
        }
    }
    if impact.carries_damage {
        deal_impact_damage(ctx, impact);
    }
    if let Some(spell) = spell.as_ref().filter(|_| landed) {
        let hdr = &spell.header;
        crate::creatures::eventai_on_spell_hit(
            ctx,
            impact.caster_guid,
            impact.target_guid,
            hdr.spell_id,
            u32::from(hdr.school_mask),
        );
        logs.log_each(ctx, impact.caster_guid, hdr);
    }
}

enum ImpactRefusal {
    Evade,
    Immune,
}

fn impact_refusal(
    ctx: &ReducerContext,
    impact: &PendingSpellImpact,
    target: &WorldEntity,
    caster: Option<&WorldEntity>,
    spell: Option<&LoadedSpell>,
) -> Option<ImpactRefusal> {
    // vmangos Spell::CheckAtDelay tests evade again when the spell lands.
    if crate::creatures::eventai::movement::returning_home(ctx, target.guid) {
        return Some(ImpactRefusal::Evade);
    }
    let harmful = impact.carries_damage
        || spell.is_some_and(|spell| crate::spell::spell_is_harmful(&spell.header, &spell.effects));
    // vmangos DoSpellHitOnUnit: a harmful travelling spell that reaches a friendly unit evades.
    if harmful && caster.is_some_and(|caster| !crate::combat::may_harm(ctx, caster, target)) {
        return Some(ImpactRefusal::Evade);
    }
    let LoadedSpell { header, effects } = spell?;
    let caster_guid = impact.caster_guid;
    // vmangos CheckAtDelay marks a unit that became immune in flight; nothing lands and no log goes.
    (crate::spell::is_immune_to_spell(ctx, caster_guid, target.guid, header, effects)
        || crate::spell::school_refuses_spell(ctx, caster_guid, target, header, harmful))
    .then_some(ImpactRefusal::Immune)
}

fn deal_impact_damage(ctx: &ReducerContext, impact: &PendingSpellImpact) {
    let roll = impact.damage_roll();
    let (dealt, absorbed) = if roll.deals_damage() {
        crate::combat::deal_blocked_spell_damage(
            ctx,
            impact.caster_guid,
            impact.target_guid,
            roll.after_resist,
            roll.blocked,
            crate::combat::Hit::spell(
                if impact.triggered {
                    crate::combat::HitSource::Triggered
                } else {
                    crate::combat::HitSource::Spell
                },
                impact.spell_id,
                roll.is_crit,
            ),
        )
    } else {
        (0, 0)
    };
    let (map_id, instance_id, grid_x, grid_y) = crate::helpers::grid_of(ctx, impact.caster_guid);
    SpellImpactEvent::insert(
        ctx,
        SpellImpactEvent {
            id: 0,
            caster_guid: impact.caster_guid,
            target_guid: impact.target_guid,
            spell_id: impact.spell_id,
            created_at: crate::time::now(ctx),
            damage: dealt,
            school: impact.school_index,
            is_crit: roll.is_crit,
            resisted: roll.resisted,
            absorbed,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            blocked: roll.blocked,
        },
    );
}
