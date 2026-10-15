use spacetimedb::{table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_faction_template, game_world_entity, WorldEntity};

use super::*;
use crate::actor::{ActionRefusal, ActionRefusalKind};
use crate::creatures::tick::game_creature_spline;

pub(crate) const COMBAT_DROP_MS: u64 = 6000;

pub(crate) fn enter_combat(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(guid) else {
        return;
    };
    if e.dead {
        return;
    }
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    let already = e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0;
    if already && e.combat_until_ms > now_ms + COMBAT_DROP_MS / 2 {
        return;
    }
    e.combat_until_ms = now_ms + COMBAT_DROP_MS;
    e.unit_flags |= lyracore_shared::constants::unit_flags::IN_COMBAT;
    entities.guid().update(e);
    crate::creatures::distraction::clear(ctx, guid);
}

pub(crate) fn arm_creature_engagement(
    ctx: &ReducerContext,
    creature_guid: u64,
    target_guid: u64,
    assist: bool,
) -> bool {
    let melee = ctx.db.game_melee_attack();
    if melee.attacker_guid().find(creature_guid).is_some() {
        return false;
    }
    melee.insert(MeleeAttack {
        attacker_guid: creature_guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
        swing_error: 0,
    });
    let entities = ctx.db.game_world_entity();
    if let Some(mut creature) = entities.guid().find(creature_guid) {
        if creature.target_guid != target_guid {
            creature.target_guid = target_guid;
            entities.guid().update(creature);
        }
    }
    crate::hooks::fire_on_aggro(
        ctx,
        &crate::hooks::AggroPayload {
            creature_guid,
            target_guid,
            assist,
        },
    );
    true
}

pub(crate) fn refresh_leash(ctx: &ReducerContext, guid_a: u64, guid_b: u64) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    for guid in [guid_a, guid_b] {
        let (Some(mut row), Some(e)) =
            (melee.attacker_guid().find(guid), entities.guid().find(guid))
        else {
            continue;
        };
        if e.is_player() {
            continue;
        }
        row.pursuit_ends_ms = crate::creatures::pursuit_deadline_ms(now_ms);
        row.leash_x = e.x;
        row.leash_y = e.y;
        melee.attacker_guid().update(row);
    }
}

pub(crate) fn disengage(ctx: &ReducerContext, guid: u64) {
    let melee = ctx.db.game_melee_attack();
    let attackers_of_guid: Vec<u64> = melee
        .by_target()
        .filter(&guid)
        .map(|a| a.attacker_guid)
        .collect();
    for a in melee
        .attacker_guid()
        .find(guid)
        .map(|a| a.attacker_guid)
        .into_iter()
        .chain(attackers_of_guid.iter().copied())
    {
        melee.attacker_guid().delete(a);
    }
    use crate::game_threat;
    let threat_members = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&guid)
        .map(|entry| entry.source_guid)
        .collect::<Vec<_>>();
    crate::threat::clear_for_unit(ctx, guid);
    clear_target(ctx, guid);
    for a in &attackers_of_guid {
        clear_target(ctx, *a);
    }
    let mut freed = attackers_of_guid;
    freed.extend(threat_members);
    freed.push(guid);
    let entities = ctx.db.game_world_entity();
    for g in freed {
        if is_engaged(ctx, g) {
            continue;
        }
        // Death clears the Loot Tag after synchronous hooks; live disengage clears it now.
        if !entities.guid().find(g).is_some_and(|entity| entity.dead) {
            crate::loot::tag::clear(ctx, g);
        }
        crate::creatures::reset_engagement(ctx, g);
        if let Some(mut e) = entities.guid().find(g) {
            if e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                e.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
                e.combat_until_ms = 0;
                entities.guid().update(e);
            }
        }
    }
}

pub(crate) fn stop_duel_combat(ctx: &ReducerContext, first_guid: u64, second_guid: u64) {
    let melee = ctx.db.game_melee_attack();
    for attacker_guid in [first_guid, second_guid] {
        if melee
            .attacker_guid()
            .find(attacker_guid)
            .is_some_and(|attack| {
                (attacker_guid == first_guid && attack.target_guid == second_guid)
                    || (attacker_guid == second_guid && attack.target_guid == first_guid)
            })
        {
            melee.attacker_guid().delete(attacker_guid);
        }
    }
    let entities = ctx.db.game_world_entity();
    for guid in [first_guid, second_guid] {
        if is_engaged(ctx, guid) {
            continue;
        }
        if let Some(mut entity) = entities.guid().find(guid) {
            entity.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
            entity.combat_until_ms = 0;
            entities.guid().update(entity);
        }
    }
}

pub(crate) fn clear_target(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if !e.is_player() && e.target_guid != 0 {
            e.target_guid = 0;
            entities.guid().update(e);
        }
    }
}

pub(crate) fn break_own_attacks(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_melee_attack().attacker_guid().delete(guid);
    crate::threat::clear_for_unit(ctx, guid);
    clear_target(ctx, guid);
}

pub(crate) fn is_engaged(ctx: &ReducerContext, guid: u64) -> bool {
    let melee = ctx.db.game_melee_attack();
    use crate::game_threat;
    melee.attacker_guid().find(guid).is_some()
        || melee.by_target().filter(&guid).next().is_some()
        || ctx
            .db
            .game_threat()
            .by_source()
            .filter(&guid)
            .any(|entry| melee.attacker_guid().find(entry.creature_guid).is_some())
}

fn may_harm_decision(same_unit: bool, active_duel: bool, friendly: bool) -> bool {
    same_unit || active_duel || !friendly
}

fn may_help_decision(same_unit: bool, active_duel: bool, hostile: bool) -> bool {
    same_unit || (!active_duel && !hostile)
}

pub(crate) fn may_harm(ctx: &ReducerContext, attacker: &WorldEntity, target: &WorldEntity) -> bool {
    may_harm_decision(
        attacker.guid == target.guid,
        crate::duel::active_opponents(ctx, attacker.guid, target.guid),
        crate::faction::is_friendly(ctx, attacker.faction_template, target.faction_template),
    )
}

pub(crate) fn may_help(ctx: &ReducerContext, helper: &WorldEntity, target: &WorldEntity) -> bool {
    may_help_decision(
        helper.guid == target.guid,
        crate::duel::active_opponents(ctx, helper.guid, target.guid),
        crate::faction::is_hostile(ctx, helper.faction_template, target.faction_template),
    )
}

pub(crate) fn is_hostile_target(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
) -> bool {
    attacker.guid != target.guid
        && (crate::duel::active_opponents(ctx, attacker.guid, target.guid)
            || crate::faction::is_hostile(ctx, attacker.faction_template, target.faction_template))
}

pub(crate) fn melee_combatant_guids(ctx: &ReducerContext) -> Vec<u64> {
    ctx.db
        .game_melee_attack()
        .iter()
        .flat_map(|a| [a.attacker_guid, a.target_guid])
        .collect()
}

#[table(
    accessor = game_melee_attack,
    public,

    index(accessor = by_target, btree(columns = [target_guid]))
)]
#[cfg_attr(feature = "native_simulation_prototype", derive(Clone))]
pub struct MeleeAttack {
    #[primary_key]
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub last_swing_ms: u32,
    #[default(0)]
    pub ranged_spell_id: u32,
    #[default(0)]
    pub last_offhand_swing_ms: u32,
    #[default(0)]
    pub rout_ends_ms: u32,
    #[default(0)]
    pub pursuit_ends_ms: u32,
    #[default(0.0)]
    pub leash_x: f32,
    #[default(0.0)]
    pub leash_y: f32,
    /// `SWING_ERROR_*`; the Gateway relays each change to the attacker.
    #[default(0u8)]
    pub swing_error: u8,
}

pub const SWING_ERROR_NONE: u8 = 0;
pub const SWING_ERROR_NOT_IN_RANGE: u8 = 1;
pub const SWING_ERROR_BAD_FACING: u8 = 2;

#[table(
    accessor = game_combat_event,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct CombatEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub damage: u32,
    pub hit_info: u8,
    pub killing_blow: bool,
    pub created_at: Timestamp,
    #[default(0)]
    pub blocked_amount: u32,
    #[default(0)]
    pub ranged_spell_id: u32,
    #[default(0)]
    pub ammo_display_id: u32,
    #[default(false)]
    pub spell_swing: bool,
    #[default(0)]
    pub impact_delay_ms: u32,
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

impl CombatEvent {
    pub(crate) fn signal_at(
        ctx: &ReducerContext,
        attacker: &WorldEntity,
        target_guid: u64,
    ) -> Self {
        let (map_id, instance_id, grid_x, grid_y) = crate::helpers::entity_addr(attacker);
        Self {
            id: 0,
            attacker_guid: attacker.guid,
            target_guid,
            damage: 0,
            hit_info: 0,
            killing_blow: false,
            created_at: ctx.timestamp,
            blocked_amount: 0,
            ranged_spell_id: 0,
            ammo_display_id: 0,
            spell_swing: false,
            impact_delay_ms: 0,
            map_id,
            instance_id,
            grid_x,
            grid_y,
        }
    }
}

#[table(accessor = game_melee_schedule, scheduled(tick_melee))]
pub struct MeleeSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

#[table(accessor = game_ranged_impact_schedule, scheduled(ranged_impact))]
pub struct RangedImpactSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub damage: u32,
    #[default(0u32)]
    pub ranged_spell_id: u32,
    #[default(false)]
    pub is_crit: bool,
}

pub(crate) fn validate_attack_target(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target_guid: u64,
) -> Result<WorldEntity, ActionRefusal> {
    let target = crate::helpers::live_entity(ctx, target_guid)
        .map_err(|_| ActionRefusal::new(ActionRefusalKind::MissingTarget, "no such target"))?;
    if target.map_id != attacker.map_id || target.instance_id != attacker.instance_id {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "target on another map",
        ));
    }
    if target.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadTarget,
            lyracore_shared::ERR_ATTACK_TARGET_DEAD,
        ));
    }
    if ctx.db.game_faction_template().count() > 0 && !may_harm(ctx, attacker, &target) {
        return Err(lyracore_shared::ERR_ATTACK_FRIENDLY.to_string().into());
    }
    Ok(target)
}

pub(crate) fn apply_start_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    start_attack(ctx, attacker_guid, target_guid, false)
        .map(|_| ())
        .map_err(Into::into)
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackStart {
    Armed,
    AlreadyArmed,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
) -> Result<AttackStart, ActionRefusal> {
    let accepted = start_attack(ctx, attacker_guid, target_guid, true)?;
    face_requested_melee_target(ctx, attacker_guid, target_guid);
    Ok(accepted)
}

fn face_requested_melee_target(ctx: &ReducerContext, attacker_guid: u64, target_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut attacker) = entities.guid().find(attacker_guid) else {
        return;
    };
    if !attacker.is_player() {
        return;
    }
    let Some(target) = entities.guid().find(target_guid) else {
        return;
    };
    let (dx, dy, dz) = (
        target.x - attacker.x,
        target.y - attacker.y,
        target.z - attacker.z,
    );
    if dx * dx + dy * dy + dz * dz > MELEE_RANGE_SQ
        || (dx.abs() <= f32::EPSILON && dy.abs() <= f32::EPSILON)
    {
        return;
    }

    let splines = ctx.db.game_creature_spline();
    let retained = splines.guid().find(attacker_guid);
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch().max(0) as u64;
    if retained.as_ref().is_some_and(|spline| {
        !spline.facing
            && spline.dur_ms > 0
            && spline
                .start_micros
                .saturating_add(u64::from(spline.dur_ms) * 1_000)
                > now_micros
    }) {
        return;
    }

    let orientation = dy.atan2(dx);
    if crate::spell::is_facing(
        attacker.x,
        attacker.y,
        attacker.orientation,
        target.x,
        target.y,
    ) {
        return;
    }
    let now_ms = (now_micros / 1_000) as u32;
    let spline_id = retained.as_ref().map_or(now_ms, |last| {
        crate::creatures::tick::next_spline_id(now_ms, last.spline_id)
    });
    let position = (attacker.x, attacker.y, attacker.z);
    let partition = (attacker.map_id, attacker.instance_id);
    let grid = (attacker.grid_x, attacker.grid_y);
    attacker.orientation = orientation;
    entities.guid().update(attacker);
    crate::creatures::tick::emit_facing_spline(
        ctx,
        attacker_guid,
        position,
        orientation,
        spline_id,
        partition.0,
        partition.1,
        grid,
    );
}

fn start_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    retain_current: bool,
) -> Result<AttackStart, ActionRefusal> {
    let attacker = crate::helpers::live_entity(ctx, attacker_guid).map_err(|_| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "attacker not in world")
    })?;
    if attacker.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadActor,
            "dead attackers cannot attack",
        ));
    }
    if crate::spell::is_action_blocked(ctx, attacker.guid) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            format!("attacker {} cannot act (stun/poly/fear)", attacker.guid),
        ));
    }
    if target_guid == attacker.guid {
        return Err("cannot attack self".to_string().into());
    }
    validate_attack_target(ctx, &attacker, target_guid)?;

    // Dismount only after the last attack Gate so a Refusal leaves the mount intact.
    crate::mount::dismount(ctx, attacker.guid);
    if retain_current
        && ctx
            .db
            .game_melee_attack()
            .attacker_guid()
            .find(attacker_guid)
            .is_some_and(|attack| attack.target_guid == target_guid && attack.ranged_spell_id == 0)
    {
        return Ok(AttackStart::AlreadyArmed);
    }

    if !attacker.is_player() {
        if arm_creature_engagement(ctx, attacker.guid, target_guid, false) {
            return Ok(AttackStart::Armed);
        }
        if let Some(mut creature) = ctx.db.game_world_entity().guid().find(attacker.guid) {
            if creature.target_guid != target_guid {
                creature.target_guid = target_guid;
                ctx.db.game_world_entity().guid().update(creature);
            }
        }
    }

    let melee = ctx.db.game_melee_attack();
    let row = MeleeAttack {
        attacker_guid: attacker.guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
        swing_error: 0,
    };
    if melee.attacker_guid().find(attacker.guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    Ok(AttackStart::Armed)
}

pub(crate) fn apply_start_ranged_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let attacker = crate::helpers::live_entity(ctx, attacker_guid)
        .map_err(|_| "attacker not in world".to_string())?;
    if attacker.dead {
        return Err("dead attackers cannot attack".to_string());
    }
    if crate::spell::is_action_blocked(ctx, attacker.guid) {
        return Err(format!(
            "attacker {} cannot act (stun/poly/fear)",
            attacker.guid
        ));
    }
    if target_guid == attacker.guid {
        return Err("cannot attack self".to_string());
    }

    let ranged_weapon = equipped_ranged_weapon(ctx, attacker.guid)
        .ok_or_else(|| "no ranged weapon equipped".to_string())?;
    let target = validate_attack_target(ctx, &attacker, target_guid)?;
    {
        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq > RANGED_RANGE_SQ {
            return Err("ranged target out of range".to_string());
        }
        if dist_sq < MELEE_RANGE_SQ {
            return Err("ranged target too close".to_string());
        }
        if !crate::spell::is_facing(
            attacker.x,
            attacker.y,
            attacker.orientation,
            target.x,
            target.y,
        ) {
            return Err("target must be in front of you".to_string());
        }
        if !crate::nav::has_los(
            ctx,
            attacker.map_id,
            attacker.instance_id,
            (attacker.x, attacker.y, attacker.z),
            (target.x, target.y, target.z),
        ) {
            return Err("target not in line of sight".to_string());
        }

        use crate::items::weapon_subclass as ws;
        if matches!(ranged_weapon.3, ws::BOW | ws::GUN | ws::CROSSBOW)
            && find_ammo(ctx, attacker.guid).is_none()
        {
            return Err("no ammo for ranged weapon".to_string());
        }
    }
    crate::mount::dismount(ctx, attacker.guid);
    // Delay the first Auto Shot by its initial wind-up; zero is the immediate-swing sentinel.
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let ranged_delay = ranged_weapon.2;
    let first_swing_seed = now_ms
        .saturating_sub(ranged_delay.saturating_sub(RANGED_INITIAL_SHOT_MS))
        .max(1);
    let melee = ctx.db.game_melee_attack();
    let row = MeleeAttack {
        attacker_guid: attacker.guid,
        target_guid,
        last_swing_ms: first_swing_seed,
        ranged_spell_id: spell_id,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
        swing_error: 0,
    };
    if melee.attacker_guid().find(attacker.guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    Ok(())
}

pub(crate) fn stop_attack_for(ctx: &ReducerContext, attacker_guid: u64) {
    ctx.db
        .game_melee_attack()
        .attacker_guid()
        .delete(attacker_guid);
}

#[cfg(test)]
mod duel_relation_tests {
    use super::{may_harm_decision, may_help_decision};

    #[test]
    fn active_duel_is_the_only_friendly_fire_exception() {
        assert!(!may_harm_decision(false, false, true));
        assert!(may_harm_decision(false, true, true));
        assert!(may_harm_decision(false, false, false));
        assert!(may_harm_decision(true, false, true));
    }

    #[test]
    fn active_opponents_stop_being_helpful_targets() {
        assert!(may_help_decision(false, false, false));
        assert!(!may_help_decision(false, true, false));
        assert!(!may_help_decision(false, false, true));
        assert!(may_help_decision(true, false, true));
    }
}
