use spacetimedb::{log, table, ReducerContext, Table};

#[cfg(feature = "debug_reducers")]
use spacetimedb::reducer;

pub const TAUNT_FORCED_MS: u64 = 3000;

#[table(accessor = game_taunt_lock)]
pub struct TauntLock {
    #[primary_key]
    pub creature_guid: u64,
    pub forced_target_guid: u64,
    pub forced_until_ms: u64,
}

use crate::{game_melee_attack, game_world_entity};

#[table(
    accessor = game_threat,
    public,
    index(accessor = by_creature, btree(columns = [creature_guid])),

    index(accessor = by_source, btree(columns = [source_guid]))
)]
pub struct ThreatEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub source_guid: u64,
    pub threat: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleSelectedThreat {
    pub creature_guid: u64,
    pub source_guid: u64,
    pub percent: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleAllThreat {
    pub creature_guid: u64,
    pub percent: i32,
}

pub const DAMAGE_THREAT_PERMILLE: i64 = 1000;

pub const HEAL_THREAT_PERMILLE: i64 = 500;

pub(crate) fn threat_from_damage(damage: u32) -> i64 {
    damage as i64 * DAMAGE_THREAT_PERMILLE / 1000
}

pub(crate) fn threat_from_heal_each(effective_heal: u32, mob_count: u32) -> i64 {
    if mob_count == 0 {
        return 0;
    }
    (effective_heal as i64 * HEAL_THREAT_PERMILLE / 1000) / mob_count as i64
}

pub(crate) fn threat_after_reduction(current: i64, amount: i64) -> i64 {
    if amount <= 0 {
        return current;
    }
    (current - amount).max(0)
}

pub(crate) fn threat_after_percent_scale(current: i64, percent: i32) -> i64 {
    let multiplier = i64::from(percent).saturating_add(100).max(0);
    let scaled = i128::from(current.max(0)) * i128::from(multiplier) / 100;
    scaled.min(i128::from(i64::MAX)) as i64
}

pub(crate) fn add_threat(ctx: &ReducerContext, creature_guid: u64, source_guid: u64, amount: i64) {
    if amount < 0 {
        log::warn!("add_threat: negative amount {amount} for creature {creature_guid} source {source_guid} (upstream bug?)");
        return;
    }
    if amount == 0 {
        return;
    }
    let source_stance = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .map(|s| crate::spell::stance_threat_pct(s.stance))
        .unwrap_or(0);
    let threat_mult = (100
        + crate::spell::combat_field_bonus(ctx, source_guid, crate::spell::COMBAT_THREAT) as i64
        + source_stance as i64)
        .max(0);
    let amount = amount * threat_mult / 100;
    if amount == 0 {
        return;
    }
    crate::loot::tag::record_first_threat(ctx, creature_guid, source_guid);
    let threats = ctx.db.game_threat();
    if let Some(mut row) = threats
        .by_creature()
        .filter(&creature_guid)
        .find(|r| r.source_guid == source_guid)
    {
        row.threat = row.threat.saturating_add(amount);
        threats.id().update(row);
    } else {
        threats.insert(ThreatEntry {
            id: 0,
            creature_guid,
            source_guid,
            threat: amount,
        });
    }
}

pub(crate) fn scale_selected_threat(ctx: &ReducerContext, operation: ScaleSelectedThreat) {
    let threats = ctx.db.game_threat();
    if let Some(mut row) = threats
        .by_creature()
        .filter(&operation.creature_guid)
        .find(|row| row.source_guid == operation.source_guid)
    {
        row.threat = threat_after_percent_scale(row.threat, operation.percent);
        threats.id().update(row);
    }
}

pub(crate) fn scale_all_threat(ctx: &ReducerContext, operation: ScaleAllThreat) {
    let threats = ctx.db.game_threat();
    let rows: Vec<u64> = threats
        .by_creature()
        .filter(&operation.creature_guid)
        .map(|row| row.id)
        .collect();
    for id in rows {
        if let Some(mut row) = threats.id().find(id) {
            row.threat = threat_after_percent_scale(row.threat, operation.percent);
            threats.id().update(row);
        }
    }
}

pub(crate) fn threat_of(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) -> i64 {
    ctx.db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .find(|r| r.source_guid == source_guid)
        .map(|r| r.threat)
        .unwrap_or(0)
}

pub(crate) fn top_threat_target(ctx: &ReducerContext, creature_guid: u64) -> Option<u64> {
    let entities = ctx.db.game_world_entity();
    let creature = entities.guid().find(creature_guid)?;
    let (creature_map, creature_instance) = (creature.map_id, creature.instance_id);
    ctx.db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .filter_map(|r| {
            let src = entities.guid().find(r.source_guid)?;
            if src.dead || src.map_id != creature_map || src.instance_id != creature_instance {
                return None;
            }
            Some((r.source_guid, r.threat))
        })
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(g, _)| g)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PartyThreatFacts {
    pub top_target_guid: Option<u64>,
    pub party_has_threat: bool,
}

pub(crate) fn party_threat_facts(
    ctx: &ReducerContext,
    creature_guid: u64,
    party_guids: &std::collections::BTreeSet<u64>,
    limit: usize,
) -> Result<PartyThreatFacts, ()> {
    let entities = ctx.db.game_world_entity();
    let creature = entities.guid().find(creature_guid).ok_or(())?;
    let rows: Vec<_> = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .take(limit.saturating_add(1))
        .collect();
    if rows.len() > limit {
        return Err(());
    }
    let valid: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            entities.guid().find(row.source_guid).is_some_and(|source| {
                !source.dead
                    && (source.map_id, source.instance_id)
                        == (creature.map_id, creature.instance_id)
            })
        })
        .collect();
    Ok(PartyThreatFacts {
        top_target_guid: valid
            .iter()
            .max_by(|a, b| {
                a.threat
                    .cmp(&b.threat)
                    .then(b.source_guid.cmp(&a.source_guid))
            })
            .map(|row| row.source_guid),
        party_has_threat: valid
            .iter()
            .any(|row| party_guids.contains(&row.source_guid)),
    })
}

pub(crate) fn taunt(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) {
    let max = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .map(|r| r.threat)
        .max()
        .unwrap_or(0);
    // The forced-target window still holds taunt when threat cannot exceed i64::MAX.
    let target = max.saturating_add(1);
    let current = threat_of(ctx, creature_guid, source_guid);
    if current < target {
        add_threat(ctx, creature_guid, source_guid, target - current);
    }
    let now_ms = (crate::time::now(ctx).to_micros_since_unix_epoch() / 1000) as u64;
    let locks = ctx.db.game_taunt_lock();
    let row = TauntLock {
        creature_guid,
        forced_target_guid: source_guid,
        forced_until_ms: now_ms + TAUNT_FORCED_MS,
    };
    if locks.creature_guid().find(creature_guid).is_some() {
        locks.creature_guid().update(row);
    } else {
        locks.insert(row);
    }
}

pub(crate) fn forced_target(ctx: &ReducerContext, creature_guid: u64, now_ms: u64) -> Option<u64> {
    let locks = ctx.db.game_taunt_lock();
    let lock = locks.creature_guid().find(creature_guid)?;
    let entities = ctx.db.game_world_entity();
    let taunter_valid = entities.guid().find(creature_guid).is_some_and(|c| {
        entities
            .guid()
            .find(lock.forced_target_guid)
            .is_some_and(|t| !t.dead && t.map_id == c.map_id && t.instance_id == c.instance_id)
    });
    if now_ms < lock.forced_until_ms && taunter_valid {
        Some(lock.forced_target_guid)
    } else {
        locks.creature_guid().delete(creature_guid);
        None
    }
}

pub(crate) fn clear_taunt_lock(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_taunt_lock()
        .creature_guid()
        .delete(creature_guid);
}

pub(crate) fn add_heal_threat(
    ctx: &ReducerContext,
    healer_guid: u64,
    healed_guid: u64,
    effective_heal: u32,
) {
    let entities = ctx.db.game_world_entity();
    let healer_is_player = entities
        .guid()
        .find(healer_guid)
        .map(|e| e.is_player())
        .unwrap_or(false);
    if !healer_is_player || effective_heal == 0 {
        return;
    }
    let mobs: Vec<u64> = ctx
        .db
        .game_melee_attack()
        .by_target()
        .filter(&healed_guid)
        .map(|a| a.attacker_guid)
        .filter(|g| {
            entities
                .guid()
                .find(g)
                .map(|e| !e.is_player())
                .unwrap_or(false)
        })
        .collect();
    let each = threat_from_heal_each(effective_heal, mobs.len() as u32);
    for creature in mobs {
        add_threat(ctx, creature, healer_guid, each);
    }
}

pub(crate) fn clear_for_unit(ctx: &ReducerContext, guid: u64) {
    let threats = ctx.db.game_threat();
    let stale: Vec<u64> = threats
        .by_creature()
        .filter(&guid)
        .chain(threats.by_source().filter(&guid))
        .map(|r| r.id)
        .collect();
    for id in stale {
        threats.id().delete(id);
    }
}

pub(crate) fn clear_all(ctx: &ReducerContext) {
    let threats = ctx.db.game_threat();
    let all: Vec<u64> = threats.iter().map(|r| r.id).collect();
    for id in all {
        threats.id().delete(id);
    }
}

pub(crate) fn reduce_threat_from_source(ctx: &ReducerContext, source_guid: u64, amount: i64) {
    if amount <= 0 {
        return;
    }
    let threats = ctx.db.game_threat();
    let hits: Vec<u64> = threats
        .by_source()
        .filter(&source_guid)
        .map(|r| r.id)
        .collect();
    for id in hits {
        if let Some(mut row) = threats.id().find(id) {
            row.threat = threat_after_reduction(row.threat, amount);
            threats.id().update(row);
        }
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_add_threat(
    ctx: &ReducerContext,
    creature_guid: u64,
    source_guid: u64,
    amount: i64,
) -> Result<(), String> {
    add_threat(ctx, creature_guid, source_guid, amount);
    Ok(())
}

pub(crate) fn engage_without_threat(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) {
    let table = ctx.db.game_threat();
    if !table
        .by_creature()
        .filter(&creature_guid)
        .any(|row| row.source_guid == source_guid)
    {
        table.insert(ThreatEntry {
            id: 0,
            creature_guid,
            source_guid,
            threat: 0,
        });
    }
    crate::combat::enter_combat(ctx, creature_guid);
    crate::combat::enter_combat(ctx, source_guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_threat_is_one_to_one() {
        assert_eq!(threat_from_damage(0), 0);
        assert_eq!(threat_from_damage(1), 1);
        assert_eq!(threat_from_damage(10), 10);
        assert_eq!(threat_from_damage(12345), 12345);
        assert!(threat_from_damage(100) > threat_from_damage(99));
    }

    #[test]
    fn heal_threat_is_half_split_among_mobs() {
        assert_eq!(threat_from_heal_each(100, 1), 50);
        assert_eq!(threat_from_heal_each(100, 2), 25);
        assert_eq!(threat_from_heal_each(100, 4), 12);
        assert_eq!(threat_from_heal_each(100, 0), 0);
        assert_eq!(threat_from_heal_each(0, 3), 0);
        assert!(threat_from_heal_each(1000, 1) > threat_from_heal_each(1000, 5));
    }

    #[test]
    fn feint_reduction_subtracts_and_floors_at_zero() {
        assert_eq!(threat_after_reduction(100, 30), 70);
        assert_eq!(threat_after_reduction(20, 50), 0);
        assert_eq!(threat_after_reduction(40, 40), 0);
        assert_eq!(threat_after_reduction(100, 0), 100);
        assert_eq!(threat_after_reduction(100, -10), 100);
        assert!(threat_after_reduction(1000, 100) >= threat_after_reduction(1000, 200));
    }

    #[test]
    fn percent_scales_use_fixed_point_rounding_and_keep_zero() {
        assert_eq!(threat_after_percent_scale(5, 50), 7);
        assert_eq!(threat_after_percent_scale(5, -50), 2);
        assert_eq!(threat_after_percent_scale(5, -100), 0);
        assert_eq!(threat_after_percent_scale(i64::MAX, 100), i64::MAX);
    }
}
