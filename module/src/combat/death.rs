use spacetimedb::{table, ReducerContext, Table, TimeDuration};

#[cfg(feature = "debug_reducers")]
use spacetimedb::ScheduleAt;

#[cfg(feature = "debug_reducers")]
use crate::game_corpse_loot;
use crate::{
    game_aura, game_creature_spawn, game_creature_template, game_threat, game_world_entity,
    WorldEntity,
};

pub(crate) const RESPAWN_MICROS: i64 = 15_000_000;
pub(crate) const CORPSE_DECAY_MICROS: i64 = 60_000_000;

pub(crate) const DRAIN_SOUL_SPELL_ID: u32 = 1120;
pub(crate) const SOUL_SHARD_ENTRY: u32 = 6265;

#[table(accessor = game_creature_lethal_damage_floor)]
pub struct CreatureLethalDamageFloor {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
    pub definition_revision: u64,
    pub notification_sent: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct FinalDamage {
    pub amount: u32,
    lethal_prevented: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CreatureDeathAttribution {
    source_guid: Option<u64>,
    reward_guid: Option<u64>,
}

impl CreatureDeathAttribution {
    fn credited(killer_guid: Option<u64>) -> Self {
        Self {
            source_guid: killer_guid,
            reward_guid: killer_guid,
        }
    }

    fn suicide(creature_guid: u64) -> Self {
        Self {
            source_guid: Some(creature_guid),
            reward_guid: None,
        }
    }
}

pub(crate) fn set_lethal_damage_floor(
    ctx: &ReducerContext,
    creature_guid: u64,
    definition_revision: u64,
    enabled: bool,
) -> bool {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return false;
    };
    if creature.is_player() || creature.dead {
        return false;
    }
    let floors = ctx.db.game_creature_lethal_damage_floor();
    if !enabled {
        floors.creature_guid().delete(creature_guid);
        return true;
    }
    let row = CreatureLethalDamageFloor {
        creature_guid,
        definition_revision,
        notification_sent: false,
    };
    match floors.creature_guid().find(creature_guid) {
        Some(_) => {
            floors.creature_guid().update(row);
        }
        None => {
            floors.insert(row);
        }
    }
    true
}

pub(crate) fn clear_lethal_damage_floor(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_lethal_damage_floor()
        .creature_guid()
        .delete(creature_guid);
}

pub(crate) fn clear_stale_lethal_damage_floor(
    ctx: &ReducerContext,
    creature_guid: u64,
    current_revision: u64,
) {
    let floors = ctx.db.game_creature_lethal_damage_floor();
    if floors
        .creature_guid()
        .find(creature_guid)
        .is_some_and(|row| row.definition_revision != current_revision)
    {
        floors.creature_guid().delete(creature_guid);
    }
}

pub(crate) fn force_creature_death(ctx: &ReducerContext, creature_guid: u64) -> bool {
    kill_creature_with_attribution(
        ctx,
        creature_guid,
        CreatureDeathAttribution::suicide(creature_guid),
    )
}

pub(crate) fn final_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    post_mitigation: u32,
) -> FinalDamage {
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return FinalDamage {
            amount: post_mitigation,
            lethal_prevented: false,
        };
    };
    lethal_floor_amount(
        target.health,
        post_mitigation,
        lethal_floor_protects(ctx, target_guid),
    )
}

pub(crate) fn lethal_floor_protects(ctx: &ReducerContext, target_guid: u64) -> bool {
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return false;
    };
    let floors = ctx.db.game_creature_lethal_damage_floor();
    !target.is_player()
        && !target.dead
        && floors.creature_guid().find(target_guid).is_some_and(|row| {
            let current = crate::creatures::current_definition_revision(ctx, target_guid);
            if row.definition_revision == current.value {
                true
            } else {
                floors.creature_guid().delete(target_guid);
                false
            }
        })
}

fn lethal_floor_amount(health: u32, post_mitigation: u32, protected: bool) -> FinalDamage {
    let lethal_prevented = protected && is_lethal(health, post_mitigation);
    FinalDamage {
        amount: if lethal_prevented {
            health.saturating_sub(1)
        } else {
            post_mitigation
        },
        lethal_prevented,
    }
}

pub(crate) fn commit_death_prevention(
    ctx: &ReducerContext,
    creature_guid: u64,
    attacker_guid: u64,
) {
    let floors = ctx.db.game_creature_lethal_damage_floor();
    let Some(mut row) = floors.creature_guid().find(creature_guid) else {
        return;
    };
    if row.notification_sent {
        return;
    }
    row.notification_sent = true;
    floors.creature_guid().update(row);
    crate::hooks::fire_on_death_prevented(
        ctx,
        &crate::hooks::DeathPreventedPayload {
            creature_guid,
            attacker_guid,
        },
    );
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_stage_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut creature = entities
        .guid()
        .find(creature_guid)
        .ok_or_else(|| format!("fixture creature does not exist: {creature_guid}"))?;
    if creature.is_player() || creature.dead {
        return Err("fixture target must be a live creature".to_string());
    }
    creature.health = 10;
    creature.stance = 1;
    creature.update(ctx);
    let definition_revision = crate::creatures::current_definition_revision(ctx, creature_guid);
    if definition_revision.value == 0
        || !set_lethal_damage_floor(ctx, creature_guid, definition_revision.value, true)
    {
        return Err("could not enable the fixture lethal floor".to_string());
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_set_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    enabled: bool,
) -> Result<(), String> {
    let revision = crate::creatures::current_definition_revision(ctx, creature_guid);
    set_lethal_damage_floor(ctx, creature_guid, revision.value, enabled)
        .then_some(())
        .ok_or_else(|| "could not update the fixture lethal floor".to_string())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_replace_definition_then_apply_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    packed_definition: String,
    rolled: u32,
) -> Result<(), String> {
    crate::creatures::replace_definition_for_debug(ctx, &packed_definition)?;
    debug_apply_lethal_damage_floor_fixture(ctx, attacker_guid, target_guid, rolled)
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_stage_ranged_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    damage: u32,
    delay_ms: u32,
) -> Result<(), String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(attacker_guid)
        .ok_or_else(|| "fixture ranged attacker does not exist".to_string())?;
    ctx.db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| "fixture ranged target does not exist".to_string())?;
    let land_at = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros(i64::from(delay_ms) * 1_000))
        .unwrap_or(ctx.timestamp);
    ctx.db
        .game_ranged_impact_schedule()
        .insert(crate::RangedImpactSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Time(land_at),
            attacker_guid,
            target_guid,
            damage,
            ranged_spell_id: 75,
            is_crit: false,
        });
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_set_lethal_damage_floor_health_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    health: u32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut creature = entities
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture creature does not exist".to_string())?;
    if creature.dead || health == 0 || health > creature.max_health {
        return Err("fixture health must keep the creature alive".to_string());
    }
    creature.health = health;
    creature.update(ctx);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_apply_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    rolled: u32,
) -> Result<(), String> {
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| format!("fixture target does not exist: {target_guid}"))?;
    if target.dead {
        return Err("fixture target is dead".to_string());
    }
    let (post_mitigation, _) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    let damage = final_damage(ctx, target_guid, post_mitigation);
    apply_hit(
        ctx,
        attacker_guid,
        target_guid,
        damage,
        Hit::weapon(HitSource::MainHand, false),
    );
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_force_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> Result<(), String> {
    force_creature_death(ctx, creature_guid)
        .then_some(())
        .ok_or_else(|| "fixture ForceDeath did not kill a live creature".to_string())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_respawn_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    if !crate::WorldEntity::delete(ctx, creature_guid) {
        return Err("fixture creature does not exist".to_string());
    }
    let spawns = ctx.db.game_creature_spawn();
    let mut spawn = spawns
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture creature has no spawn".to_string())?;
    spawn.respawn_at = ctx.timestamp;
    spawns.guid().update(spawn);
    crate::creatures::tick::pass_respawn(ctx);
    entities
        .guid()
        .find(creature_guid)
        .filter(|creature| !creature.dead)
        .map(|_| ())
        .ok_or_else(|| "fixture creature did not respawn".to_string())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
#[allow(clippy::too_many_arguments)]
pub fn debug_verify_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    creature_guid: u64,
    expected_health: u32,
    expected_dead: bool,
    expected_floor: bool,
    expected_notification: bool,
    expected_absorb: i32,
) -> Result<(), String> {
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture creature is absent".to_string())?;
    if expected_health != u32::MAX && creature.health != expected_health {
        return Err(format!(
            "fixture health mismatch: actual={} expected={expected_health}",
            creature.health
        ));
    }
    if creature.dead != expected_dead {
        return Err(format!(
            "fixture death mismatch: actual={} expected={expected_dead}",
            creature.dead
        ));
    }
    if !expected_dead
        && ctx
            .db
            .game_corpse_loot()
            .by_corpse()
            .filter(&creature_guid)
            .next()
            .is_some()
    {
        return Err("a surviving fixture creature received corpse loot".to_string());
    }
    let floor = ctx
        .db
        .game_creature_lethal_damage_floor()
        .creature_guid()
        .find(creature_guid);
    if floor.is_some() != expected_floor
        || floor
            .as_ref()
            .is_some_and(|row| row.notification_sent != expected_notification)
    {
        return Err("fixture lethal-floor state mismatch".to_string());
    }
    let absorb = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&creature_guid)
        .filter(|aura| aura.eff_kind == crate::spell::A_ABSORB)
        .map(|aura| aura.amount_remaining)
        .sum::<i32>();
    if absorb != expected_absorb {
        return Err(format!(
            "fixture absorb mismatch: actual={absorb} expected={expected_absorb}"
        ));
    }
    Ok(())
}

use super::*;

pub(crate) fn kill_creature(ctx: &ReducerContext, target_guid: u64, killer: Option<u64>) -> bool {
    kill_creature_with_attribution(ctx, target_guid, CreatureDeathAttribution::credited(killer))
}

fn kill_creature_with_attribution(
    ctx: &ReducerContext,
    target_guid: u64,
    attribution: CreatureDeathAttribution,
) -> bool {
    let killer = attribution.source_guid;
    let reward_source = attribution.reward_guid;
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return false;
    };
    if target.is_player() || target.dead {
        return false;
    }
    clear_lethal_damage_floor(ctx, target_guid);
    if target.owner_guid != 0 {
        crate::creatures::begin_death_dispatch(ctx, target_guid, killer);
        crate::creatures::on_pet_death(ctx, target_guid);
        disengage(ctx, target_guid);
        crate::WorldEntity::delete(ctx, target_guid);
        crate::creatures::clear_live_pet_kind(ctx, target_guid);
        crate::creatures::distraction::clear(ctx, target_guid);
        crate::hooks::fire_on_death(
            ctx,
            &crate::hooks::DeathPayload {
                victim_guid: target_guid,
                killer_guid: killer.unwrap_or(0),
                victim_is_player: false,
            },
        );
        crate::creatures::finish_death_dispatch(ctx, target_guid, killer);
        crate::loot::tag::clear(ctx, target_guid);
        return true;
    }
    crate::creatures::tick::stop_where_rendered(ctx, &mut target);
    let victim_entry = target.entry;
    let victim_level = target.level;
    let victim_instance = target.instance_id;
    let current_target_guid = target.target_guid;
    // Resolve the Loot Tag while live; death hooks still need its reward ceiling.
    let entitlement = crate::loot::tag::death_entitlement(
        ctx,
        target_guid,
        target.x,
        target.y,
        target.map_id,
        target.instance_id,
    );
    let kill_recipients = entitlement
        .as_ref()
        .map(|entitlement| entitlement.recipients.as_slice())
        .unwrap_or_default();
    award_tag_rewards(ctx, &target, target_guid, kill_recipients, reward_source);
    roll_corpse_loot(ctx, &mut target, target_guid, entitlement.as_ref());
    target.health = 0;
    target.dead = true;
    target.update(ctx);
    crate::creatures::begin_death_dispatch(ctx, target_guid, killer);
    // Snapshot threat before disengage clears the ledger used by death hooks.
    let threat_snapshot: Vec<(u64, i64)> = ctx
        .db
        .game_threat()
        .iter()
        .filter(|t| t.creature_guid == target_guid)
        .map(|t| (t.source_guid, t.threat))
        .collect();
    disengage(ctx, target_guid);
    crate::threat::clear_taunt_lock(ctx, target_guid);
    crate::spell::break_channel(ctx, target_guid);
    crate::spell::remove_auras_on_death(ctx, target_guid);
    let spawns = ctx.db.game_creature_spawn();
    if let Some(mut spawn) = spawns.guid().find(target_guid) {
        spawn.despawn_at = ctx
            .timestamp
            .checked_add(TimeDuration::from_micros(CORPSE_DECAY_MICROS))
            .unwrap_or(ctx.timestamp);
        spawns.guid().update(spawn);
    }
    if let Some(killer_guid) = killer {
        if !is_engaged(ctx, killer_guid) {
            if let Some(mut k) = entities.guid().find(killer_guid) {
                k.combat_until_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
                k.update(ctx);
            }
        }
    }
    // Run hooks after death state is settled so handlers observe the complete transition.
    crate::hooks::fire_on_death(
        ctx,
        &crate::hooks::DeathPayload {
            victim_guid: target_guid,
            killer_guid: killer.unwrap_or(0),
            victim_is_player: false,
        },
    );
    crate::hooks::fire_on_creature_death(
        ctx,
        &crate::hooks::CreatureDeathPayload {
            creature_guid: target_guid,
            entry: victim_entry,
            instance_id: victim_instance,
            killer_guid: killer.unwrap_or(0),
            current_target_guid,
            threat_snapshot,
        },
    );
    for &killer_guid in kill_recipients {
        crate::hooks::fire_on_kill(
            ctx,
            &crate::hooks::KillPayload {
                killer_guid,
                victim_guid: target_guid,
                victim_entry,
                victim_level,
            },
        );
    }
    crate::creatures::finish_death_dispatch(ctx, target_guid, killer);
    // Clear the Loot Tag only after every synchronous death callback has observed it.
    crate::loot::tag::clear(ctx, target_guid);
    true
}

fn award_tag_rewards(
    ctx: &ReducerContext,
    target: &WorldEntity,
    target_guid: u64,
    recipients: &[u64],
    reward_source: Option<u64>,
) {
    let template = ctx.db.game_creature_template().entry().find(target.entry);
    let rank = template.as_ref().map(|t| t.rank).unwrap_or(0);
    let share_count = recipients.len() as u32;
    for recipient in recipients {
        crate::xp::award_xp(
            ctx,
            *recipient,
            target_guid,
            target.level,
            rank,
            share_count,
        );
        crate::creatures::award_hunter_pet_kill_progression(ctx, *recipient, target.level, rank);
        crate::quest::on_creature_killed(ctx, *recipient, target.entry);
    }
    let is_critter = template
        .as_ref()
        .map(|t| t.creature_type == crate::spell::CRITTER_TYPE)
        .unwrap_or(false);
    if let Some(reward_guid) = reward_source.filter(|guid| recipients.contains(guid)) {
        let reward_level = ctx
            .db
            .game_world_entity()
            .guid()
            .find(reward_guid)
            .map(|character| character.level)
            .unwrap_or(0);
        let yields_xp = crate::xp::xp_for_kill(target.level, reward_level) > 0;
        let draining_soul = ctx
            .db
            .game_aura()
            .by_target()
            .filter(&target_guid)
            .any(|a| a.spell_id == DRAIN_SOUL_SPELL_ID && a.caster_guid == reward_guid);
        if !is_critter && yields_xp && draining_soul {
            let _ = crate::items::grant_item(ctx, reward_guid, SOUL_SHARD_ENTRY, 1);
        }
    }
}

fn roll_corpse_loot(
    ctx: &ReducerContext,
    target: &mut WorldEntity,
    target_guid: u64,
    entitlement: Option<&crate::loot::tag::DeathEntitlement>,
) {
    crate::loot::purge_corpse_residue(ctx, target_guid);
    let Some(entitlement) = entitlement else {
        target.money = 0;
        target.dynamic_flags &= !lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        return;
    };
    crate::loot::tag::record_corpse_eligibility(ctx, target_guid, &entitlement.recipients);
    let loot = ctx
        .db
        .game_creature_template()
        .entry()
        .find(target.entry)
        .map(|template| {
            crate::loot::scale_money_for_rank(
                roll_money(ctx, template.money_min, template.money_max),
                template.rank,
            )
        })
        .unwrap_or(0);
    if loot > 0 {
        target.money = loot;
    }
    let dropped = crate::loot::roll_creature_loot(ctx, target.entry, target_guid);
    if let Some(group_id) = entitlement.group_id {
        crate::loot::apply_group_loot_rules(ctx, target_guid, group_id);
    }
    if loot > 0 || dropped {
        target.dynamic_flags |= lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    }
}

pub(crate) fn kill_player(ctx: &ReducerContext, victim_guid: u64, killer_guid: u64) -> bool {
    let entities = ctx.db.game_world_entity();
    let Some(mut victim) = entities.guid().find(victim_guid) else {
        return false;
    };
    if !victim.is_player() || victim.dead {
        return false;
    }
    crate::duel::interrupt_duel_for(ctx, victim_guid);
    victim.health = 0;
    victim.dead = true;
    victim.combat_until_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    victim.update(ctx);
    let killer = (killer_guid != 0).then_some(killer_guid);
    crate::creatures::begin_death_dispatch(ctx, victim_guid, killer);
    disengage(ctx, victim_guid);
    crate::spell::break_channel(ctx, victim_guid);
    crate::spell::record_self_resurrect_option(ctx, victim_guid);
    crate::spell::remove_auras_on_death(ctx, victim_guid);
    crate::trade::cancel_trade_for(ctx, victim_guid);
    if crate::items::apply_death_durability_loss(ctx, victim_guid) {
        crate::group::push_event(
            ctx,
            victim_guid,
            lyracore_shared::group::event_kind::DEATH_DURABILITY,
            0,
            String::new(),
        );
    }
    crate::hooks::fire_on_death(
        ctx,
        &crate::hooks::DeathPayload {
            victim_guid,
            killer_guid,
            victim_is_player: true,
        },
    );
    crate::creatures::finish_death_dispatch(ctx, victim_guid, killer);
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HitSource {
    MainHand,
    OffHand,
    Ranged,
    MeleeSpell,
    Spell,
    Triggered,
}

impl HitSource {
    pub(crate) fn is_weapon(self) -> bool {
        matches!(self, Self::MainHand | Self::OffHand | Self::Ranged)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Hit {
    pub source: HitSource,
    pub spell_id: u32,
    pub crit: bool,
}

impl Hit {
    pub(crate) fn weapon(source: HitSource, crit: bool) -> Self {
        Self {
            source,
            spell_id: 0,
            crit,
        }
    }

    pub(crate) fn spell(source: HitSource, spell_id: u32, crit: bool) -> Self {
        Self {
            source,
            spell_id,
            crit,
        }
    }

    pub(crate) fn triggered() -> Self {
        Self {
            source: HitSource::Triggered,
            spell_id: 0,
            crit: false,
        }
    }
}

pub(crate) struct HitOutcome {
    pub killed: bool,
    pub duel_completed: bool,
}

impl HitOutcome {
    pub(crate) fn combat_ended(self) -> bool {
        self.killed || self.duel_completed
    }
}

pub(crate) fn is_lethal(health: u32, dmg: u32) -> bool {
    dmg > 0 && health <= dmg
}

/// A lethal hit between active Duel opponents ends the Duel instead: `apply_hit` floors the loser.
pub(crate) fn duel_takes_lethal_hit(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target: &WorldEntity,
) -> bool {
    target.is_player() && crate::duel::active_opponents(ctx, attacker_guid, target.guid)
}

pub(crate) fn is_killing_blow(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target: &WorldEntity,
    dmg: u32,
) -> bool {
    is_lethal(target.health, dmg) && !duel_takes_lethal_hit(ctx, attacker_guid, target)
}

// Absorb before lethal checks and godmode so even a zeroed hit drains shields.
pub(crate) fn fold_incoming_damage(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    rolled: u32,
) -> (u32, u32) {
    let entities = ctx.db.game_world_entity();
    let attacker_stance = entities
        .guid()
        .find(attacker_guid)
        .map(|a| a.stance)
        .unwrap_or(0);
    let outgoing_pct = crate::spell::stance_damage_done_pct(attacker_stance)
        + crate::spell::combat_field_bonus(ctx, attacker_guid, crate::spell::COMBAT_DMG_DONE);
    let after_outgoing = crate::spell::apply_damage_pct(rolled, outgoing_pct);
    let incoming = crate::spell::apply_damage_pct(
        after_outgoing,
        crate::spell::damage_taken_bonus(ctx, target_guid),
    );
    let dealt = crate::spell::absorb_incoming(ctx, target_guid, incoming);
    let godmode = entities
        .guid()
        .find(target_guid)
        .map(|t| t.godmode)
        .unwrap_or(false);
    let dealt = if godmode { 0 } else { dealt };
    (dealt, incoming - dealt)
}

pub(crate) fn apply_hit(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    damage: FinalDamage,
    hit: Hit,
) -> HitOutcome {
    let miss = HitOutcome {
        killed: false,
        duel_completed: false,
    };
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return miss;
    };
    if target.dead || (damage.amount == 0 && !damage.lethal_prevented) {
        return miss;
    }
    let dmg = damage.amount;
    let weapon = hit.source.is_weapon();
    let attacker = entities.guid().find(attacker_guid);
    if attacker
        .as_ref()
        .is_some_and(|attacker| !crate::combat::may_harm(ctx, attacker, &target))
    {
        return miss;
    }
    let attacker_is_player = attacker.as_ref().map(|a| a.is_player()).unwrap_or(false);
    let reward_source = crate::loot::tag::controlling_character(ctx, attacker_guid);
    let target_is_player = target.is_player();
    if damage.lethal_prevented && dmg == 0 {
        // A lethal floor still refreshes pursuit so the scripted fight cannot evade.
        refresh_leash(ctx, attacker_guid, target_guid);
        commit_death_prevention(ctx, target_guid, attacker_guid);
        return miss;
    }
    if !target_is_player {
        crate::loot::tag::record_first_threat(ctx, target_guid, attacker_guid);
    }

    if weapon {
        if let Some(a) = &attacker {
            if is_rage_user(a) {
                if let Some(mut fresh) = entities.guid().find(attacker_guid) {
                    fresh.power = (fresh.power + rage_from_damage(dmg, true)).min(fresh.max_power);
                    fresh.update(ctx);
                }
            }
            if attacker_is_player && !target_is_player {
                crate::skill::gain_weapon_skill(ctx, a);
            }
        }
    }

    if is_lethal(target.health, dmg) && duel_takes_lethal_hit(ctx, attacker_guid, &target) {
        target.health = 1;
        target.update(ctx);
        if let Some(duel) = crate::duel::active_duel_between(ctx, attacker_guid, target_guid) {
            crate::duel::complete_duel(
                ctx,
                duel.id,
                crate::duel::duel_completion_kind::WON,
                attacker_guid,
                target_guid,
            );
        }
        crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, true);
        return HitOutcome {
            killed: false,
            duel_completed: true,
        };
    }
    let killed = is_lethal(target.health, dmg) && (weapon || !target_is_player);
    if killed {
        if target_is_player {
            kill_player(ctx, target_guid, attacker_guid);
        } else {
            kill_creature_with_attribution(
                ctx,
                target_guid,
                CreatureDeathAttribution {
                    source_guid: attacker.as_ref().map(|_| attacker_guid),
                    reward_guid: reward_source,
                },
            );
        }
        crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, false);
        return HitOutcome {
            killed: true,
            duel_completed: false,
        };
    }
    let accrues_threat = !target_is_player && reward_source.is_some();
    if weapon && target_is_player {
        crate::skill::gain_defense_skill(ctx, &target);
    }
    target.health = crate::spell::damaged_value(target.health, dmg as i32);
    if weapon && is_rage_user(&target) {
        target.power = (target.power + rage_from_damage(dmg, false)).min(target.max_power);
    }
    target.update(ctx);
    if damage.lethal_prevented {
        commit_death_prevention(ctx, target_guid, attacker_guid);
    }
    refresh_leash(ctx, attacker_guid, target_guid);
    if !weapon {
        enter_combat(ctx, target_guid);
    }

    crate::spell::break_auras_on_damage(
        ctx,
        target_guid,
        if weapon { attacker_guid } else { 0 },
        false,
    );
    if accrues_threat {
        crate::threat::add_threat(
            ctx,
            target_guid,
            attacker_guid,
            crate::threat::threat_from_damage(dmg),
        );
        if !weapon {
            crate::spell::arm_spell_retaliation(ctx, attacker_guid, target_guid);
        }
    }
    crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, true);
    HitOutcome {
        killed: false,
        duel_completed: false,
    }
}

#[cfg(test)]
mod lethality_tests {
    use super::{is_lethal, lethal_floor_amount, CreatureDeathAttribution, HitOutcome};

    #[test]
    fn a_zero_damage_hit_is_never_lethal() {
        assert!(!is_lethal(1, 0));
        assert!(!is_lethal(0, 0));
    }

    #[test]
    fn a_hit_at_or_above_remaining_health_is_lethal() {
        assert!(is_lethal(10, 10));
        assert!(is_lethal(10, 11));
        assert!(!is_lethal(10, 9));
    }

    #[test]
    fn lethal_floor_changes_only_protected_lethal_damage() {
        assert_eq!(lethal_floor_amount(10, 4, true).amount, 4);
        assert_eq!(lethal_floor_amount(10, 10, false).amount, 10);
        assert_eq!(lethal_floor_amount(10, 10, true).amount, 9);
        assert_eq!(lethal_floor_amount(1, 20, true).amount, 0);
    }

    #[test]
    fn forced_death_names_self_without_granting_kill_rewards() {
        let attribution = CreatureDeathAttribution::suicide(77);
        assert_eq!(attribution.source_guid, Some(77));
        assert_eq!(attribution.reward_guid, None);
    }

    #[test]
    fn duel_completion_is_a_combat_terminal_without_becoming_a_death() {
        let outcome = HitOutcome {
            killed: false,
            duel_completed: true,
        };
        assert!(outcome.combat_ended());
    }
}
