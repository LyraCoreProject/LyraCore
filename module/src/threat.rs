//! Combat — the **threat table**: per-creature aggro bookkeeping that decides WHICH unit an
//! engaged creature attacks. Damage (melee + spell) and EFFECTIVE healing accrue threat per
//! `(creature, source)`; the retarget pass in `tick_creatures` re-points each engaged creature at its
//! highest-threat source every world tick — the "DPS/healer pulls aggro off the tank" mechanic — and a
//! TAUNT tops the taunter's threat to yank it. Threat is cleared when a unit leaves combat (folded into
//! `combat::disengage`, the one canonical "leave combat" teardown). [entity]
//!
//! BASELINE SAFETY: the retarget pass switches a creature's target ONLY when a source STRICTLY
//! out-threats its current target (hysteresis). An empty table (no damage dealt yet) or a single-source
//! table (one attacker) therefore leaves the creature on its current target — byte-identical to the
//! pre-threat proximity/retaliation behavior. The NEW behavior manifests only with TWO+ competing
//! sources, which is exactly the multiplayer threat mechanic.

use spacetimedb::{log, table, ReducerContext, Table};
// `reducer` is used only by the feature-gated `debug_add_threat` below — gate the import to match so a
// production build (feature OFF) doesn't warn on an unused import.
#[cfg(feature = "debug_reducers")]
use spacetimedb::reducer;

/// How long a taunt FORCES the creature onto the taunter regardless of threat (vanilla: taunt is a
/// 3s forced-target + a threat set, not just a threat bump).
pub const TAUNT_FORCED_MS: u64 = 3000;

/// A live taunt FORCED-TARGET window: while `now < forced_until_ms`, the retarget pass PINS
/// `creature_guid`'s melee row on `forced_target_guid` instead of consulting the threat table — the
/// missing half of vanilla taunt (a threat-top alone is re-overtaken by a hard-nuking dps within a
/// second). One row per creature (PK); refreshed by re-taunts; expired
/// rows are lazily reaped by the retarget pass and dropped on creature death. Module-internal AI
/// state (like the threat table) — the gateway never reads it, so no bindings. NEW table →
/// auto-migrates. [entity]
#[table(accessor = game_taunt_lock)]
pub struct TauntLock {
    #[primary_key]
    pub creature_guid: u64,
    pub forced_target_guid: u64,
    pub forced_until_ms: u64,
}

// The `game_melee_attack` accessor (defined in combat/) — heal threat is distributed to the creatures
// currently engaged with the healed unit; `game_world_entity` for the source-validity check.
use crate::{game_melee_attack, game_world_entity};

// ===========================================================================================
//  Table [entity]
// ===========================================================================================

/// One unit's accumulated threat on one creature's table. Logical key `(creature_guid, source_guid)`;
/// an `#[auto_inc]` PK plus a `by_creature` btree index (the creature's small source list is scanned for
/// the source, mirroring the `game_spell_cd` `(caster, spell)` idiom — no lossy u64+u64 PK pack). Public
/// to match the other combat-state tables (`game_melee_attack`/`game_combat_event`): it is pure
/// server-internal AI state the gateway never relays (vanilla 1.12 sends no threat table to the client),
/// but public keeps it SQL-inspectable for verification and consistent with its siblings. No Timestamp →
/// SQL-loadable/inspectable. [entity]
#[table(
    accessor = game_threat,
    public,
    index(accessor = by_creature, btree(columns = [creature_guid])),
    // Perf catalog 1.17: the source side (clear_for_unit on every death/evade/logout, Feint's
    // reduce_threat_from_source) used to full-scan the table.
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

/// Scale one existing source row on a creature's threat table. EventAI uses this operation instead
/// of editing `game_threat` itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleSelectedThreat {
    pub creature_guid: u64,
    pub source_guid: u64,
    pub percent: i32,
}

/// Scale every existing source row on a creature's threat table. EventAI uses this operation
/// instead of editing `game_threat` itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleAllThreat {
    pub creature_guid: u64,
    pub percent: i32,
}

// ===========================================================================================
//  Tuning — threat modifiers as PERMILLE (×1000) so the fractional multipliers are exact integer math
//  (mirrors the rank HP/damage multipliers in creatures/ and combat/).
// ===========================================================================================

/// Threat per point of damage dealt (melee + direct spell), PERMILLE (`1000` = 1.0×). Vanilla physical
/// and most spell damage generates threat equal to the damage; kept a named constant so a future
/// per-school threat modifier (e.g. holy/frost) has a single home.
pub const DAMAGE_THREAT_PERMILLE: i64 = 1000;

/// Threat per point of EFFECTIVE healing, PERMILLE (`500` = 0.5×). Vanilla heal threat is half the amount
/// healed, SPLIT among the creatures in combat with the healed unit (so a single-mob fight gives the full
/// 0.5×, and a pack divides it). Overheal generates no threat — the caller passes EFFECTIVE healing only.
pub const HEAL_THREAT_PERMILLE: i64 = 500;

// ===========================================================================================
//  Pure helpers (unit-tested off a live module)
// ===========================================================================================

/// Threat generated by `damage` points of damage: `damage * DAMAGE_THREAT_PERMILLE / 1000`. Pure.
pub fn threat_from_damage(damage: u32) -> i64 {
    damage as i64 * DAMAGE_THREAT_PERMILLE / 1000
}

/// Threat generated on EACH of `mob_count` creatures by `effective_heal` points of effective healing,
/// split evenly (vanilla halves the heal and divides it among the mobs in combat with the healed unit):
/// `effective_heal * HEAL_THREAT_PERMILLE / 1000 / mob_count`, integer-floored. `mob_count == 0` → 0 (no
/// one is fighting the healed unit, so a heal aggroes nothing). A single mob gets the full 0.5×. Pure.
pub fn threat_from_heal_each(effective_heal: u32, mob_count: u32) -> i64 {
    if mob_count == 0 {
        return 0;
    }
    (effective_heal as i64 * HEAL_THREAT_PERMILLE / 1000) / mob_count as i64
}

/// A threat row's value after a Feint-style one-time reduction: `(current - amount)`, FLOORED at 0 so a
/// reduction larger than the current threat empties the row rather than going negative. A non-positive
/// `amount` is a no-op (returns `current`) — Feint never ADDS threat. Pure — unit-tested.
pub fn threat_after_reduction(current: i64, amount: i64) -> i64 {
    if amount <= 0 {
        return current;
    }
    (current - amount).max(0)
}

/// Scale a nonnegative threat value by `100 + percent`, rounded down to the next whole threat
/// point. The wide intermediate makes the result stable at the `i64` boundary. A `-100` percent
/// scale returns zero; callers retain the row so a later source action can observe it.
pub fn threat_after_percent_scale(current: i64, percent: i32) -> i64 {
    let multiplier = i64::from(percent).saturating_add(100).max(0);
    let scaled = i128::from(current.max(0)) * i128::from(multiplier) / 100;
    scaled.min(i128::from(i64::MAX)) as i64
}

// ===========================================================================================
//  Table ops (ctx-aware)
// ===========================================================================================

/// Add `amount` threat from `source_guid` to `creature_guid`'s table — UPSERT: bump the existing
/// `(creature, source)` row, else insert a fresh one. A non-positive `amount` (a 0-damage swing, a
/// fully-absorbed hit, an overheal) is a NO-OP so the table is never polluted with zero rows. [entity]
pub fn add_threat(ctx: &ReducerContext, creature_guid: u64, source_guid: u64, amount: i64) {
    // A NEGATIVE amount is never legitimate here (threat sources only ADD) — it signals an upstream
    // calculation bug, so surface it rather than swallowing it silently. Zero is a legitimate no-op
    // (a 0-damage swing / fully-absorbed hit), so it stays quiet.
    if amount < 0 {
        log::warn!("add_threat: negative amount {amount} for creature {creature_guid} source {source_guid} (upstream bug?)");
        return;
    }
    if amount == 0 {
        return;
    }
    // Threat modifier: scale by the SOURCE's signed-PERCENT threat terms summed at the single threat
    // chokepoint — (a) the A_MOD_COMBAT(COMBAT_THREAT) aura (the Defiance talent / Fade), and
    // (b) the SOURCE's Warrior stance (Defensive Stance's +30%, keyed on the `stance` field via
    // `stance_threat_pct`, NOT an aura, so a stance switch clears it for free). No aura + non-Defensive
    // stance → ×1.0 (baseline byte-identical). A >100% reduction floors the multiplier at 0 (threat fully
    // suppressed). [entity]
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
        return; // threat fully reduced → no-op (never insert a zero row)
    }
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

/// Apply one percent scale to `operation.source_guid`'s existing threat row. A source absent from
/// the table stays absent. A zero result remains an entry, matching a percent change rather than a
/// threat clear.
pub fn scale_selected_threat(ctx: &ReducerContext, operation: ScaleSelectedThreat) {
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

/// Apply one percent scale to every existing threat row on `operation.creature_guid`. Rows stay in
/// place when they reach zero, and each row is collected before an update so the table is never
/// changed while it is read.
pub fn scale_all_threat(ctx: &ReducerContext, operation: ScaleAllThreat) {
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

/// `source_guid`'s current threat on `creature_guid`'s table (0 when absent — the retarget hysteresis
/// reads this for the CURRENT target, which may not be on the table after a pure proximity pull). [entity]
pub fn threat_of(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) -> i64 {
    ctx.db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .find(|r| r.source_guid == source_guid)
        .map(|r| r.threat)
        .unwrap_or(0)
}

/// The VALID source with the highest threat on `creature_guid`'s table, or `None` when the table is empty
/// OR every entry's source is gone/dead/off-map. "Valid" = the source still exists in the world, is
/// alive, and shares the creature's map — so a fled-cross-map, logged-out, or dead source is never chosen
/// as the aggro target (their stale row lingers until `clear_for_unit`, but is ignored here). Ties break
/// toward the LOWEST `source_guid` (deterministic); the retarget caller applies hysteresis (switches only
/// on a STRICTLY higher threat than the current target), so a tie never flaps the target. [entity]
pub fn top_threat_target(ctx: &ReducerContext, creature_guid: u64) -> Option<u64> {
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
        // Highest threat wins; on a tie the LOWER guid wins (so `b.0.cmp(&a.0)` makes the smaller guid
        // compare as "greater" for `max_by`, which returns the maximum element).
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(g, _)| g)
}

/// Top `source_guid`'s threat on `creature_guid` to ONE ABOVE the current table maximum — the TAUNT
/// effect. Makes the taunter the highest-threat source so the next retarget pass switches the creature
/// to it, even if the taunter had done little damage (the threat-yank). On an empty table this seeds
/// threat 1 (max defaults to 0), still making the taunter top. Idempotent-ish: if the source is already
/// at/above the new top it adds nothing. Also arms (or refreshes) the `TAUNT_FORCED_MS` forced-target
/// window: the retarget pass asks `forced_target` FIRST, so the creature stays pinned on the taunter
/// regardless of threat until the window expires — the threat top governs what happens AFTER. [entity]
pub fn taunt(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) {
    let max = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .map(|r| r.threat)
        .max()
        .unwrap_or(0);
    // `saturating_add` caps at `i64::MAX` rather than wrapping to `i64::MIN` (which would INVERT the
    // yank into the lowest threat). At the boundary (`max == i64::MAX`) the taunter is already tied for
    // top, so the no-op `current < target` below still holds the aggro — acceptable.
    let target = max.saturating_add(1);
    let current = threat_of(ctx, creature_guid, source_guid);
    if current < target {
        add_threat(ctx, creature_guid, source_guid, target - current);
    }
    // Arm (or refresh) the FORCED-TARGET window — for TAUNT_FORCED_MS the retarget pass pins the
    // creature on the taunter no matter who out-threats whom (the vanilla rule; the threat top above
    // still matters for what happens AFTER the window expires).
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
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

/// `creature_guid`'s live FORCED target (a taunt lock whose window is still open and whose taunter
/// is still a valid aggro target — alive, present, same map), or `None`. This is the lock's whole
/// read side in one place: validity uses the SAME predicate as `top_threat_target`'s source
/// check, and an expired/invalid lock is LAZILY REAPED here, so no caller ever sees a stale row.
/// The retarget pass consults this BEFORE the threat compare. [entity]
pub fn forced_target(ctx: &ReducerContext, creature_guid: u64, now_ms: u64) -> Option<u64> {
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

/// Drop `creature_guid`'s taunt lock unconditionally — the death/teardown path (a dead creature's
/// lock is meaningless). No-op when absent. [entity]
pub fn clear_taunt_lock(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_taunt_lock()
        .creature_guid()
        .delete(creature_guid);
}

/// Distribute heal threat: every CREATURE currently engaged (attacker in a `game_melee_attack` row) with
/// `healed_guid` gains threat crediting `healer_guid`, equal to `effective_heal × 0.5` SPLIT evenly among
/// those creatures — the "healing pulls aggro" mechanic. No-op when the healer isn't a player (a creature
/// self-heal doesn't drive player-threat semantics), when no creature is fighting the healed unit, or when
/// the effective heal is 0 (overheal). Collect-then-add (no table write while iterating). [entity]
pub fn add_heal_threat(
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
    // Creatures attacking the healed unit (melee rows whose target is the healed unit; the attacker must
    // be a creature — a player co-attacker keeps no threat table).
    let mobs: Vec<u64> = ctx
        .db
        .game_melee_attack()
        .by_target() // perf catalog 1.15 — was a full melee-table scan per heal
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

/// Drop every threat row that references `guid` — both `guid`'s OWN table (when it is a CREATURE leaving
/// combat) AND `guid`'s entries on every OTHER creature's table (when it is a SOURCE — a player — dying or
/// logging out). Folded into `combat::disengage`, so "leave combat" clears threat symmetrically in ONE
/// place (a creature that evades/flees/dies drops its memory; a player who dies/logs is forgotten by every
/// mob). Collect-then-delete (never mutate while iterating). [entity]
pub fn clear_for_unit(ctx: &ReducerContext, guid: u64) {
    let threats = ctx.db.game_threat();
    // Perf catalog 1.17: two indexed collects instead of one full scan. A row can only appear in both
    // halves if a unit were a source on its OWN table (self-threat, which nothing generates); the
    // second delete would be a harmless no-op anyway.
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

/// Wipe the entire threat table — used by the creature-roster re-import (`import_creature_spawns`), which
/// deletes every creature entity, so their threat tables would otherwise orphan. Collect-then-delete.
pub fn clear_all(ctx: &ReducerContext) {
    let threats = ctx.db.game_threat();
    let all: Vec<u64> = threats.iter().map(|r| r.id).collect();
    for id in all {
        threats.id().delete(id);
    }
}

/// FEINT: one-time reduction of `source_guid`'s CURRENT threat by `amount` on EVERY creature it is a
/// source on, floored at 0 — so the rogue drops down each engaged mob's aggro list and the retarget pass
/// re-points each mob at its new top-threat source (the tank), with zero retarget-code changes. DISTINCT
/// from Fade (an `A_MOD_COMBAT(COMBAT_THREAT)` percent on FUTURE threat folded inside `add_threat`); this
/// is a one-shot subtraction on the EXISTING rows. A non-positive `amount` is a no-op (never ADDS threat).
///
/// INDEX DISCIPLINE: the caster's source rows come from the `by_source` btree (perf catalog 1.17 added
/// it; this doc used to say the scan was unavoidable). Still collect the ids first, THEN update — never
/// mutate while iterating. [entity]
pub fn reduce_threat_from_source(ctx: &ReducerContext, source_guid: u64, amount: i64) {
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

// ===========================================================================================
//  Debug — inject threat for server-side verification of the retarget mechanic (two synthetic sources
//  on one creature without needing two live player connections). Behind the debug feature gate.
// ===========================================================================================

/// Inject `amount` threat from `source_guid` onto `creature_guid`'s table (the debug entry point to
/// `add_threat`). Lets a test arm two competing threat sources on one creature and watch the retarget
/// pass switch the target — without two live player connections. Permissive (dev), like the other debug
/// reducers. [debug]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_threat_is_one_to_one() {
        // 1.0× — threat equals the damage dealt (the vanilla physical/most-spell modifier).
        assert_eq!(threat_from_damage(0), 0);
        assert_eq!(threat_from_damage(1), 1);
        assert_eq!(threat_from_damage(10), 10);
        assert_eq!(threat_from_damage(12345), 12345);
        // Monotonic in damage.
        assert!(threat_from_damage(100) > threat_from_damage(99));
    }

    #[test]
    fn heal_threat_is_half_split_among_mobs() {
        // 0.5× to a SINGLE mob (the common fight): 100 healed → 50 threat.
        assert_eq!(threat_from_heal_each(100, 1), 50);
        // Split evenly across the pack: 100 healed vs 2 mobs → 25 each.
        assert_eq!(threat_from_heal_each(100, 2), 25);
        // 4 mobs → 12 each (100*500/1000 = 50, /4 = 12 floored).
        assert_eq!(threat_from_heal_each(100, 4), 12);
        // No one in combat with the healed unit → no threat (guards the divide-by-zero too).
        assert_eq!(threat_from_heal_each(100, 0), 0);
        // Zero effective heal (overheal) → no threat regardless of mob count.
        assert_eq!(threat_from_heal_each(0, 3), 0);
        // More mobs → less threat each (the split is real).
        assert!(threat_from_heal_each(1000, 1) > threat_from_heal_each(1000, 5));
    }

    #[test]
    fn feint_reduction_subtracts_and_floors_at_zero() {
        // A partial drop: 100 threat, Feint removes 30 → 70.
        assert_eq!(threat_after_reduction(100, 30), 70);
        // A reduction larger than the current threat empties the row (never negative).
        assert_eq!(threat_after_reduction(20, 50), 0);
        // Exactly to zero.
        assert_eq!(threat_after_reduction(40, 40), 0);
        // A non-positive amount is a no-op — Feint never ADDS threat.
        assert_eq!(threat_after_reduction(100, 0), 100);
        assert_eq!(threat_after_reduction(100, -10), 100);
        // Monotonic: a bigger Feint removes at least as much.
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
