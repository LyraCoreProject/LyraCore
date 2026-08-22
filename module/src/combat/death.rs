//! Death + the shared damage pipeline (#382 split of the former monolithic `combat/mod.rs`, on top of
//! #370's shared damage pipeline). `kill_creature`/`kill_player` are the two chokepoints every lethal
//! path funnels through so a melee kill, `debug_set_health(0)`, and a lethal DoT tick all produce an
//! IDENTICAL corpse/release. `fold_incoming_damage` (the MODIFIER stage: outgoing % → incoming % →
//! absorb → godmode), `final_damage` (the EventAI lethal floor), and `apply_hit` (the APPLICATION
//! stage: rage + skill-ups, the lethal fork through the two kill chokepoints, the health write,
//! break-on-damage, threat) are the pipeline every damage resolver in `swing.rs` and
//! `spell::apply_target_damage` routes through. See the banner below.
//! `mod.rs` re-exports this module (`pub use death::*`) so every `crate::combat::<sym>` path resolves
//! regardless of which submodule actually defines it.

use spacetimedb::{table, ReducerContext, Table, TimeDuration};

#[cfg(feature = "debug_reducers")]
use spacetimedb::ScheduleAt;

use crate::{
    game_aura, game_creature_spawn, game_creature_spline, game_creature_template, game_threat,
    game_world_entity, WorldEntity,
};
#[cfg(feature = "debug_reducers")]
use crate::{game_corpse_loot, game_spell_cast_event};

// The corpse-lifecycle timers below are used by `kill_creature`'s decay-arm step; `RESPAWN_MICROS` has
// no local reader (the flat-timer fallback lives in `creatures::tick`) but stays here beside
// `CORPSE_DECAY_MICROS` since both are read externally as `crate::combat::{RESPAWN_MICROS,
// CORPSE_DECAY_MICROS}` and describe the same corpse/respawn timeline.
pub(crate) const RESPAWN_MICROS: i64 = 15_000_000; // 15s after the corpse despawns
pub(crate) const CORPSE_DECAY_MICROS: i64 = 60_000_000; // a corpse lingers 60s before it despawns

// Warlock soul-shard generation: Drain Soul's real vanilla script effect (`ChannelDeathItem`) mints a
// Soul Shard when its channel's target dies mid-cast. Our Drain Soul import carries no Rust script hook
// for that effect, so the grant lives here instead, at the ONE place every creature death already
// funnels through. `DRAIN_SOUL_SPELL_ID` is the real vanilla spell id (1120); `SOUL_SHARD_ENTRY` is the
// real vanilla item id (6265, hand-seeded in `seed.rs` since the .import ETL doesn't reliably carry it).
pub(crate) const DRAIN_SOUL_SPELL_ID: u32 = 1120;
pub(crate) const SOUL_SHARD_ENTRY: u32 = 6265;

/// A creature's EventAI-owned lethal floor. Row presence means enabled. It survives engagement and
/// evade resets, then clears on definition replacement, death, despawn, or respawn. Enabling it
/// again replaces the row and rearms the first-prevention notification.
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
    let floors = ctx.db.game_creature_lethal_damage_floor();
    let protected = !target.is_player()
        && !target.dead
        && floors.creature_guid().find(target_guid).is_some_and(|row| {
            let current = crate::creatures::current_definition_revision(ctx, target_guid);
            if row.definition_revision == current.value {
                true
            } else {
                floors.creature_guid().delete(target_guid);
                false
            }
        });
    lethal_floor_amount(target.health, post_mitigation, protected)
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

fn commit_death_prevention(ctx: &ReducerContext, creature_guid: u64, attacker_guid: u64) {
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
    entities.guid().update(creature);
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
    entities.guid().update(creature);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_verify_ranged_lethal_damage_floor_fixture(
    ctx: &ReducerContext,
    attacker_guid: u64,
    creature_guid: u64,
    expected_health: u32,
    expected_damage_log: u32,
) -> Result<(), String> {
    let creature = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .ok_or_else(|| "fixture ranged target is absent".to_string())?;
    if creature.health != expected_health {
        return Err(format!(
            "fixture ranged health mismatch: actual={} expected={expected_health}",
            creature.health
        ));
    }
    let damage = ctx
        .db
        .game_spell_cast_event()
        .iter()
        .filter(|event| {
            event.caster_guid == attacker_guid
                && event.target_guid == creature_guid
                && event.spell_id == 75
                && event.is_proc_log
        })
        .map(|event| event.damage)
        .max()
        .unwrap_or(0);
    if damage != expected_damage_log {
        return Err(format!(
            "fixture ranged damage log mismatch: actual={damage} expected={expected_damage_log}"
        ));
    }
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
    if !entities.guid().delete(creature_guid) {
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

// Tables' pure formulas/consts and the sibling submodules' re-exports (`roll_money`, `is_engaged`, ...)
// are all pulled in from `mod.rs` (`pub use tables::*` + `pub use folds::*`/`engage::*`/`swing::*`) so
// every symbol resolves the same as before the split.
use super::*;

/// Kill a CREATURE — the SHARED creature-death path. The melee kill, `debug_set_health(0)`, AND lethal
/// spell/DoT damage all funnel through here so they produce an IDENTICAL corpse (no drift between the
/// paths). Steps, in order: award XP to an optional
/// player `killer` (before the level is cleared); roll the template's money loot + the data-driven
/// item loot onto the corpse and mark it `LOOTABLE`; set `health = 0` + `dead` (the sole on-wire
/// corpse signal — NEVER `UNIT_DYNFLAG_DEAD`, which is feign-death); drop the engagement (`disengage`);
/// delete the corpse's stale patrol legs (so a late-subscribing client doesn't walk the corpse); and
/// arm the corpse-decay timer on the spawn row. Returns `true` if it killed a live creature; `false`
/// (no-op) for a missing guid, a player, or an already-dead unit. `killer = None` = no XP credit
/// (a DoT with no attacker, a debug kill). [entity]
pub(crate) fn kill_creature(ctx: &ReducerContext, target_guid: u64, killer: Option<u64>) -> bool {
    kill_creature_with_attribution(ctx, target_guid, CreatureDeathAttribution::credited(killer))
}

fn kill_creature_with_attribution(
    ctx: &ReducerContext,
    target_guid: u64,
    attribution: CreatureDeathAttribution,
) -> bool {
    let killer = attribution.source_guid;
    let reward_killer = attribution.reward_guid;
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return false;
    };
    if target.is_player() || target.dead {
        return false; // creatures only, and never re-kill a corpse
    }
    clear_lethal_damage_floor(ctx, target_guid);
    // A PET death (owner_guid != 0) is a CLEAN DESPAWN: no XP/loot/corpse/decay. A pet has NO
    // game_creature_spawn row, so the decay pass can never reap a dead pet — without this it would linger as
    // a stale, lootable corpse until the owner re-summons/logs out/dies. Delete it + free its engagements;
    // the owner can re-summon. (A pet kill credits no XP — an enemy killing your Imp gains nothing.)
    if target.owner_guid != 0 {
        crate::creatures::begin_death_dispatch(ctx, target_guid, killer);
        crate::creatures::on_pet_death(ctx, target_guid);
        disengage(ctx, target_guid);
        entities.guid().delete(target_guid);
        crate::creatures::clear_live_pet_kind(ctx, target_guid);
        // Notify-hook: a pet death is still a death — fired after the despawn is committed.
        crate::hooks::fire_on_death(
            ctx,
            &crate::hooks::DeathPayload {
                victim_guid: target_guid,
                killer_guid: killer.unwrap_or(0),
                victim_is_player: false,
            },
        );
        crate::creatures::finish_death_dispatch(ctx, target_guid, killer);
        return true;
    }
    // Snapshot the victim's identity for the notify-hooks fired at the end of this fn — the corpse row
    // is mutated (and `target` moved into the update) before they fire.
    let victim_entry = target.entry;
    let victim_level = target.level;
    let victim_instance = target.instance_id; // for on_creature_death (work-item 228)
    let current_target_guid = target.target_guid;
    // A player killer gains XP (and may ding) — awarded before the corpse's level is gone.
    // Quests: the same killing blow advances the killer's kill objectives for this creature entry
    // (no-op for a non-player killer or one with no matching quest). Done before the corpse is gone so
    // the entry is still readable; on_creature_killed reads its own quest tables, not the target.
    // `kill_recipients` is hoisted OUT of the `if let` block below (work-item 187) so the group-loot
    // stamping call further down can reuse the SAME recipient set the XP/quest-credit split used —
    // eligibility must never drift between the two.
    // XP + quest-credit + Drain-Soul-shard rewards for the killer (and its group) — extracted so the
    // death sequence below reads as a table of contents; see `award_killer_rewards` (issue #382).
    let kill_recipients = reward_killer
        .map(|killer_guid| award_killer_rewards(ctx, &target, target_guid, killer_guid))
        .unwrap_or_default();
    // Money + item loot onto the corpse, group-loot stamping, and the LOOTABLE flag — see
    // `roll_corpse_loot` (issue #382).
    roll_corpse_loot(
        ctx,
        &mut target,
        target_guid,
        reward_killer,
        &kill_recipients,
    );
    target.health = 0;
    target.dead = true;
    // #519: a creature killed mid-leg (flee/patrol/chase) still carries an in-flight
    // `game_creature_spline` row with a real duration — left alone, the client keeps interpolating the
    // corpse toward the old destination after death (a Kobold Vermin sliding onward while dead). Same
    // "TOLD to halt instead of left interpolating" 0-duration stop the cycle's chase phase uses when a
    // creature plants to swing, but fired here instead so EVERY lethal path (flee/patrol/chase, not just
    // engaged melee) gets the same treatment at this one death chokepoint. The stop lands at the
    // server's authoritative death position (`target.x/y/z`, unmoved by the kill) so the corpse renders
    // exactly where loot-click range judges it.
    //
    // Deliberately NOT deleted here too, for two independent reasons:
    //   1. SpacetimeDB diffs a transaction's NET effect per row, not each intermediate write — update
    //      then delete of the SAME PK in one transaction nets out, for subscribers, as a bare delete of
    //      the PRE-transaction row. The stop values staged by `emit_move_spline` above would never reach
    //      the wire, and the gateway has no `on_delete` handler for `game_creature_spline` either
    //      (`world_view.rs`), so a bare delete relays nothing — the corpse would keep sliding exactly as
    //      before this fix.
    //   2. `game_creature_spline` deletion is deliberately confined to ONE chokepoint,
    //      `despawn_creature_entity` (issue #359's "canonical despawn checklist" — see its doc + tripwire
    //      in `tick/lifecycle.rs`), which forbids new deletion sites for this table on the same grounds
    //      #395 retired the old per-caller copies. Adding a second deletion path here would be exactly
    //      the divergence #359 exists to prevent.
    // So the row is left for the existing 60s corpse-decay reap (`pass_decay` → `despawn_creature_entity`,
    // a LATER transaction) to clear, same as it always has for every other creature death. This means
    // #519's literal "no row for the dead guid" Done-when only becomes true after that reap, not
    // immediately on kill — the stop-spline packet and position are what a headless check right after
    // kill can assert; row absence is a 60s-later assertion. See the issue-519 comment reconciling this.
    if ctx
        .db
        .game_creature_spline()
        .guid()
        .find(target_guid)
        .is_some()
    {
        let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
        crate::creatures::tick::emit_move_spline(
            ctx,
            target_guid,
            (target.x, target.y, target.z),
            (target.x, target.y, target.z),
            0,
            false,
            now_ms,
            target.map_id,
            target.instance_id,
            (target.grid_x, target.grid_y),
        );
    }
    entities.guid().update(target);
    crate::creatures::begin_death_dispatch(ctx, target_guid, killer);
    // Snapshot the victim's threat table for the death payload BEFORE disengage wipes it — threat
    // is the engine's per-player damage/heal ledger, and hooks fire only after the wipe (see
    // CreatureDeathPayload.threat_snapshot).
    let threat_snapshot: Vec<(u64, i64)> = ctx
        .db
        .game_threat()
        .iter()
        .filter(|t| t.creature_guid == target_guid)
        .map(|t| (t.source_guid, t.threat))
        .collect();
    disengage(ctx, target_guid);
    // A dead creature's taunt lock is meaningless — drop it with the engagement. threat.rs owns the
    // lock table's whole lifecycle; this is its teardown verb.
    crate::threat::clear_taunt_lock(ctx, target_guid);
    // A creature that dies mid-channel (a caster mob) stops channeling immediately — tear down its channel
    // aura + any cast bar. No-op for the common non-caster kill (no channel/cast). A channel aimed AT this
    // dying creature is ended separately by the tick arm (`!target_alive`), so both directions are covered.
    crate::spell::break_channel(ctx, target_guid);
    // A corpse carries no buffs, no DoTs and no crowd control — vanilla sheds them on the death
    // transition itself, for every unit and not just players. Runs AFTER the reward/loot steps above,
    // which still read the victim's live aura set (Drain Soul's channel).
    crate::spell::remove_auras_on_death(ctx, target_guid);
    // Arm the corpse-decay timer on the spawn row (the tick_creatures decay pass gates on it).
    let spawns = ctx.db.game_creature_spawn();
    if let Some(mut spawn) = spawns.guid().find(target_guid) {
        spawn.despawn_at = ctx
            .timestamp
            .checked_add(TimeDuration::from_micros(CORPSE_DECAY_MICROS))
            .unwrap_or(ctx.timestamp);
        spawns.guid().update(spawn);
    }
    // VANILLA combat-drop on kill: leave combat when the mob dies UNLESS something else is still engaged.
    // Vanilla drops you the moment the last hostile link is gone rather than waiting the flat
    // COMBAT_DROP_MS. `disengage` above cleared this mob's rows, so if the killer now has NO melee
    // engagement — neither attacking (its own outgoing row) nor being attacked (any row targeting it) —
    // pull its combat deadline to NOW so the next combat-drop tick clears IN_COMBAT (~0.5s, not 6s). A
    // killer still meleeing another mob, or with another mob on it, keeps its combat. (Applies to any
    // killer; the player is the case that matters.)
    if let Some(killer_guid) = killer {
        if !is_engaged(ctx, killer_guid) {
            if let Some(mut k) = entities.guid().find(killer_guid) {
                k.combat_until_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
                entities.guid().update(k);
            }
        }
    }
    // Notify-hooks fire last so handlers observe the fully committed death: on_death for the victim
    // (every creature-death path funnels through this function), and on_kill when the death has a
    // known source. Reward ownership remains independent of this source identity.
    crate::hooks::fire_on_death(
        ctx,
        &crate::hooks::DeathPayload {
            victim_guid: target_guid,
            killer_guid: killer.unwrap_or(0),
            victim_is_player: false,
        },
    );
    // Encounter kernel (work-item 228): the entry-keyed, instance-stamped creature death — the
    // pet clean-despawn branch above deliberately does NOT fire this (encounters key on wild
    // creatures). Snapshots were taken before the corpse row was mutated.
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
    if let Some(killer_guid) = killer {
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
    true
}

/// XP + quest-credit + Drain-Soul-shard rewards for `target`'s kill, shared across every in-range
/// group member. Extracted out of `kill_creature`'s inline body (issue #382) so the death sequence
/// reads as a table of contents. Returns the kill-recipient set so `roll_corpse_loot`'s group-loot
/// stamping reuses the SAME eligibility split (work-item 187 — must never drift between the two).
fn award_killer_rewards(
    ctx: &ReducerContext,
    target: &WorldEntity,
    target_guid: u64,
    killer_guid: u64,
) -> Vec<u64> {
    // Elite/rare-elite/boss creatures pay 2× kill XP (vanilla) — the rank + creature_type live on
    // the template (one lookup, reused below for the soul-shard XP/critter gate).
    let template = ctx.db.game_creature_template().entry().find(target.entry);
    let rank = template.as_ref().map(|t| t.rank).unwrap_or(0);
    // Snapshot the killer's level BEFORE award_xp: a kill that dings the killer mutates and
    // persists their level in-place (xp::grant_xp's ding loop), so re-reading it after would pick
    // up the POST-ding level for the shard's grey-clamp check below — misclassifying a kill that
    // just yielded XP at the PRE-ding level as grey at the new, higher level.
    let killer_level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(killer_guid)
        .map(|k| k.level)
        .unwrap_or(0);
    // GROUP kill rewards: a grouped killer shares the kill with every in-range living member — each
    // gets 1/n of its OWN level-based XP and full quest kill-credit. An ungrouped killer is a
    // 1-element recipient set, so its reward is unaffected by the group split.
    let recipients = crate::group::kill_reward_recipients(
        ctx,
        killer_guid,
        target.x,
        target.y,
        target.map_id,
        target.instance_id,
    );
    let share_count = recipients.len() as u32;
    for recipient in &recipients {
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
    // Soul shard generation: if `killer_guid` is channeling Drain Soul (1120) on this dying
    // creature — an aura targeting it, cast BY the killer, naming that spell — the killing blow
    // mints 1x Soul Shard into the killer's backpack. Real vanilla grants
    // this off Drain Soul's own `ChannelDeathItem` script effect on a lethal tick; we hook it here
    // instead since that effect imports as an inert E_SCRIPTED no-op (see combat/mod.rs module docs
    // above). Vanilla's actual rule is "the kill yields experience or honor" — no honor system
    // exists yet, so that reduces to: NOT a critter (creature_type 8, template-driven, never yields
    // XP regardless of level) AND NOT grey (`xp::xp_for_kill` > 0, the SAME grey clamp award_xp
    // used two lines up — shard eligibility can never drift from XP eligibility). Best-effort: an
    // inventory-full killer still gets the kill (`grant_item`'s Err is swallowed, never rolls back
    // the death).
    let is_critter = template
        .as_ref()
        .map(|t| t.creature_type == crate::spell::CRITTER_TYPE)
        .unwrap_or(false);
    let yields_xp = crate::xp::xp_for_kill(target.level, killer_level) > 0;
    if !is_critter && yields_xp {
        let draining_soul = ctx
            .db
            .game_aura()
            .by_target()
            .filter(&target_guid)
            .any(|a| a.spell_id == DRAIN_SOUL_SPELL_ID && a.caster_guid == killer_guid);
        if draining_soul {
            let _ = crate::items::grant_item(ctx, killer_guid, SOUL_SHARD_ENTRY, 1);
        }
    }
    recipients
}

/// Roll a dead creature's money + item loot onto its corpse, apply the GROUP loot method's
/// need/greed/round-robin/master-loot stamping, and mark it LOOTABLE if anything dropped. Also purges
/// any stale corpse-loot residue on this guid (issue #358) BEFORE rolling fresh drops — the fix MUST
/// run on every kill so a leftover pickpocket row can't collide with a freshly-rolled kill-drop slot
/// (see `corpse_residue_tripwire` below, which pins the ordering). Extracted out of `kill_creature`'s
/// inline body (issue #382) so the death sequence reads as a table of contents.
fn roll_corpse_loot(
    ctx: &ReducerContext,
    target: &mut WorldEntity,
    target_guid: u64,
    killer: Option<u64>,
    kill_recipients: &[u64],
) {
    // Money loot from the template plus data-driven item loot. Lootable if either dropped.
    // ELITE/RARE/BOSS: the rolled copper is scaled by the template's rank (`scale_money_for_rank`) —
    // an elite/rare/boss is worth more. Rank 0 (every current creature) → ×1.0, so a normal kill's
    // purse is unaffected by the rank scaling.
    let loot = ctx
        .db
        .game_creature_template()
        .entry()
        .find(target.entry)
        .map(|t| {
            crate::loot::scale_money_for_rank(roll_money(ctx, t.money_min, t.money_max), t.rank)
        })
        .unwrap_or(0);
    if loot > 0 {
        target.money = loot;
    }
    // STALE-SNAPSHOT PURGE (267, found live; widened by 358): corpse guids can be REUSED —
    // `debug_spawn_at_feet` allocates max+1 per entry, and a harness SQL teardown deletes the entity
    // WITHOUT running the decay reaper — so a fresh kill on a reused guid inherited a long-dead
    // group's `game_corpse_loot_eligible` rows and split the solo looter's purse with departed
    // members (46c purse → 13c, caught by the quest scenario's money delta). A LIVE creature also
    // accrues its OWN residue on this same guid: `roll_pickpocket_loot` (E_PICKPOCKET) inserts
    // `game_corpse_loot` rows at slots 0.. before it dies, and if the rogue doesn't take them all,
    // this fn's `roll_creature_loot` below would otherwise re-insert kill drops starting at slot 0
    // too — duplicate `(corpse_guid, slot)` pairs break every first-match consumer's addressing.
    // `purge_corpse_residue` closes both classes at the ONE spot every creature death routes
    // through, BEFORE the fresh loot roll / group snapshot below. No-op for a guid with no residue
    // (the common path).
    crate::loot::purge_corpse_residue(ctx, target_guid);
    // Quest-only rows now roll UNCONDITIONALLY (work-item 187 slice 0 fixed 210's recorded
    // divergence — loot.rs module doc decision #1): a debug/environmental kill (`killer = None`) rolls
    // them exactly the same as a player kill; visibility/takability are decided per-viewer/per-taker
    // downstream instead, so `roll_creature_loot` no longer needs the killer at all.
    let dropped = crate::loot::roll_creature_loot(ctx, target.entry, target_guid);
    // Group loot methods (work-item 187 slices 1-4): a GROUPED kill's above-threshold rows may
    // spawn a need/greed roll, below-threshold/round-robin rows get a designated looter stamped, and
    // above-threshold-under-MASTER rows get restricted to the master — all decided HERE at kill
    // time (see loot.rs's module doc for why not lazily "at loot-open"). A no-op for an ungrouped
    // kill or an FFA-method group (`apply_group_loot_rules`'s own early-outs).
    if let Some(killer_guid) = killer {
        crate::loot::apply_group_loot_rules(ctx, target_guid, killer_guid, kill_recipients);
    }
    if loot > 0 || dropped {
        target.dynamic_flags |= lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    }
}

/// Kill a PLAYER — the SHARED player-death path, `kill_creature`'s twin: both swing resolvers and
/// `debug_set_health(0)` funnel through here so every player death is IDENTICAL (channel teardown,
/// combat-deadline zeroing, and everything below all happen exactly once, the same way). `health = 0`
/// is the SOLE on-wire death signal — vanilla 1.12 has no death opcode; the client shows Release Spirit
/// purely from UNIT_FIELD_HEALTH == 0 on its own unit (via the crash-safe build_health_values relay). No
/// corpse/loot/XP (creatures don't loot players); the entity persists (the decay pass only touches
/// creature spawns) until `CMSG_REPOP_REQUEST` → the `repop` reducer revives it. Steps, in order:
/// zero health + set `dead` (gates regen + re-attack); pull `combat_until_ms` to NOW so the
/// gateway's logout gate doesn't block a dead player for up to COMBAT_DROP_MS (vanilla allows dead
/// players to log out immediately; `disengage` clears melee rows but does NOT zero the deadline);
/// drop the engagements; tear down any channel/cast bar; shed the auras death removes (which dismounts
/// a rider); apply the 10% equipped-durability loss;
/// and fire the `on_death` notify-hook LAST so handlers observe the fully-committed death.
/// `killer_guid = 0` = no killer (a debug/environmental death). Returns `false` (no-op) for a
/// missing guid, a creature, or an already-dead player. [entity]
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
    entities.guid().update(victim);
    let killer = (killer_guid != 0).then_some(killer_guid);
    crate::creatures::begin_death_dispatch(ctx, victim_guid, killer);
    disengage(ctx, victim_guid);
    crate::spell::break_channel(ctx, victim_guid);
    // Dying sheds the auras vanilla does not exempt, and converges every projection they fed. A rider
    // who dies is dismounted HERE, by ordinary aura removal — the corpse must not keep its mount, its
    // buffs, or the crowd control that was on it.
    crate::spell::remove_auras_on_death(ctx, victim_guid);
    // A live Trade Session dies with the victim — both windows hear `TradeCanceled` (#123).
    crate::trade::cancel_trade_for(ctx, victim_guid);
    crate::items::apply_death_durability_loss(ctx, victim_guid);
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

// ===========================================================================================
//  The SHARED damage pipeline (#370) [entity]
//
//  Everything that happens to a target AFTER the damage number is rolled used to exist in four
//  near-verbatim copies — the main-hand swing, the off-hand swing, the ranged projectile impact, and
//  `spell::apply_target_damage`. They drifted twice (issue #361: the off-hand ignored Disarm, the
//  ranged impact ignored godmode), which is what a copy of a pipeline always eventually does. The
//  pipeline now lives here, exactly once, in three stages:
//
//    1. `fold_incoming_damage` — the MODIFIER stage: outgoing % → incoming % → absorb → godmode.
//       Every caller that rolls a fresh number runs it; the ranged IMPACT skips it because its
//       damage was already folded (and frozen) at launch, and applies only the godmode re-check.
//    2. `final_damage`: the EventAI lethal floor, after every modifier and before any damage effect.
//    3. `apply_hit`: the APPLICATION stage: rage + skill-ups, the lethal fork through the shared
//       `kill_player`/`kill_creature` chokepoints, the health write, break-on-damage, and threat.
//
//  Each caller writes its own wire event between the folds and application, so the stages remain
//  separate operations.
// ===========================================================================================

/// Where a hit came from. [`HitSource::is_weapon`] is the ONE axis [`apply_hit`] branches on, so
/// "what is different about a spell hit" is a single readable predicate instead of a second copy of
/// the whole pipeline; the finer split exists because the Proc engine fires a DIFFERENT combat event
/// per source (an off-hand swing and a ranged shot are not the same event as a main-hand swing).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HitSource {
    /// A main-hand melee auto-attack swing.
    MainHand,
    /// An off-hand melee auto-attack swing — its own proc event, so an off-hand-only proc fires off
    /// these alone.
    OffHand,
    /// A ranged auto-attack impact (Auto Shot, a wand).
    Ranged,
    /// A melee ABILITY: a weapon strike or a finisher. Its own proc event pair, distinct from the
    /// auto-attack swing that shares its weapon.
    MeleeSpell,
    /// A direct damaging SPELL hit.
    Spell,
    /// A hit produced inside a **Triggered Cast**. Grants nothing, and raises NO proc event — which is
    /// what makes "a proc can never start a proc" structural instead of an exception list.
    Triggered,
}

impl HitSource {
    /// A WEAPON hit — a melee main-hand swing, an off-hand swing, or a ranged projectile impact.
    /// Grants rage BOTH ways, trains weapon/defense skill, KILLS a player at 0 hp, credits a PET's
    /// kill to its owner, and feeds the real attacker guid to the break-on-damage scan (so
    /// Retaliation can counter-swing).
    ///
    /// Everything else — a spell, a melee ability, a Triggered Cast — grants no rage and no
    /// skill-ups; a PLAYER is FLOORED at 1 hp instead of dying (there is no spell-death of players
    /// yet); the kill credit is player-caster-only (a pet's spell doesn't credit its owner — pets
    /// have no spells today); the break-on-damage scan gets the `0` attacker sentinel; and a player
    /// caster's damaging hit ARMS the creature's melee retaliation.
    pub(crate) fn is_weapon(self) -> bool {
        matches!(self, Self::MainHand | Self::OffHand | Self::Ranged)
    }
}

/// One landed hit, as the pipeline and the Proc engine both read it: where it came from, which spell
/// carried it (0 for an auto-attack swing), and whether it crit.
#[derive(Clone, Copy)]
pub(crate) struct Hit {
    pub source: HitSource,
    pub spell_id: u32,
    pub crit: bool,
}

impl Hit {
    /// An auto-attack hit — no spell carried it.
    pub(crate) fn weapon(source: HitSource, crit: bool) -> Self {
        Self {
            source,
            spell_id: 0,
            crit,
        }
    }

    /// A hit carried by `spell_id`, whose school and family the Proc filter may judge.
    pub(crate) fn spell(source: HitSource, spell_id: u32, crit: bool) -> Self {
        Self {
            source,
            spell_id,
            crit,
        }
    }

    /// A hit nobody took an action to land: a Retaliation counter-swing, a ground-area tick. Grants
    /// nothing and raises no proc event.
    pub(crate) fn triggered() -> Self {
        Self {
            source: HitSource::Triggered,
            spell_id: 0,
            crit: false,
        }
    }
}

/// What [`apply_hit`] did. `killed` is what a caller checks to decide whether its own post-hit tail (a
/// swing-timer stamp, an IN_COMBAT stamp, a react window) still has anything to stamp — after a kill
/// the engagement row is already gone and the defender is a corpse. A struct rather than a bare bool
/// so the pipeline can report more (resisted portions, overkill) without touching four signatures.
pub(crate) struct HitOutcome {
    /// The hit went through `kill_creature` / `kill_player`. False for a spell hit that floored a
    /// player at 1 hp, and false for any hit the target survived.
    pub killed: bool,
    /// The hit ended an active Duel at one health. Callers must not re-arm combat afterward.
    pub duel_completed: bool,
}

impl HitOutcome {
    pub(crate) fn combat_ended(self) -> bool {
        self.killed || self.duel_completed
    }
}

/// Pure: does a hit of `dmg` reduce a target at `health` to 0? A 0-damage hit (a miss, a fully
/// absorbed swing, a godmode target) is NEVER lethal, which is the whole reason this is one shared
/// predicate — the melee swing needs the answer up front for its event's `killing_blow` flag, and
/// [`apply_hit`] needs it again for the fork. [pure]
pub(crate) fn is_lethal(health: u32, dmg: u32) -> bool {
    dmg > 0 && health <= dmg
}

/// Stage 1 of the shared pipeline: fold the post-roll damage MODIFIERS onto a freshly-rolled hit and
/// return `(dealt, absorbed)`.
///
/// In order (the order is vanilla's and is load-bearing):
///  1. the ATTACKER's outgoing % — Defensive Stance's −10% (`stance_damage_done_pct`) summed with any
///     `A_MOD_COMBAT(COMBAT_DMG_DONE)` aura on it (Curse of Weakness);
///  2. the TARGET's incoming % — `A_MOD_DAMAGE_TAKEN` (Shield Wall, a vulnerability debuff);
///  3. absorb shields (`A_ABSORB`, e.g. Power Word: Shield) — BEFORE the lethal check, so a
///     fully-absorbed hit can't kill and drains the shield instead of health;
///  4. the GM playtest godmode zero (223's `.god`) — keyed on the TARGET only, so a godmode unit's
///     OUTGOING damage is untouched. Applied AFTER absorb, so a godmode target's shield still drains
///     exactly as it did before (and the zeroed remainder reads as "absorbed").
///
/// `absorbed` is the difference — what shields (plus godmode) soaked — which the spell path surfaces
/// on its cast-GO row for the floating damage breakdown. Weapon callers ignore it. A missing attacker
/// contributes stance 0 / bonus 0 (full damage), the codebase's "missing actor stat → 0" convention.
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

/// Stage 3 of the shared pipeline. Applies the [`FinalDamage`] produced after
/// [`fold_incoming_damage`] to `target_guid`, credited to `attacker_guid`, and returns what it did.
///
/// In order:
///  1. **Attacker gains** (weapon hits only) — rage for dealing damage, and a weapon skill-up when a
///     PLAYER lands on a CREATURE. Both run BEFORE the fork: you gain rage on the killing blow too.
///  2. **The lethal fork** — a target reduced to 0 goes through the SHARED `kill_player` /
///     `kill_creature` chokepoints (corpse, loot, XP, disengage, hooks) and this returns
///     `killed: true`. The one exception is a SPELL hit on a PLAYER, which floors at 1 hp instead.
///     Kill credit: the attacker if it is a player, else its OWNER for a pet's weapon kill, else
///     nobody.
///  3. **The survivor path** — a defense skill-up for a player that took a weapon hit and lived, the
///     health write (floored at 1 by `damaged_value`, which is a no-op here for a weapon hit since a
///     survivor by definition had `health > dmg`), rage for TAKING the hit, then the single entity
///     write. `enter_combat` for a spell hit lands here (a weapon caller stamps IN_COMBAT at its own
///     tail instead — a swing that FIRED flags combat even when it MISSED, which is a fact only the
///     caller can see).
///  4. **Break-on-damage** (drops polymorph, interrupts a timed cast) and **threat** (player →
///     creature only), plus the spell path's engage-on-damage retaliation arm.
///  5. **The proc pass** ([`crate::spell::proc::run_proc_pass`]) — once per lethal branch, and from
///     nowhere else in the module. The attacker's "dealt" Procs run even on a killing blow; the
///     target's "taken" Procs run only on the survivor path.
///
/// A 0-damage hit (miss, fully absorbed, godmode) is a complete no-op: no health write, no kill, no
/// break-on-damage, no threat — matching what the melee swing and `apply_target_damage` have always
/// done, and what the ranged impact now does too (before #370 it ran the survivor path with a 0
/// damage value on a godmode target — the same drift class #361 fixed twice).
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
        return miss; // the target left the world between the roll and here
    };
    if target.dead || (damage.amount == 0 && !damage.lethal_prevented) {
        return miss;
    }
    let dmg = damage.amount;
    let weapon = hit.source.is_weapon();
    // Read the attacker ONCE, fresh: a caller's in-hand snapshot may predate its own writes this tick
    // (the queued-strike rage deduction, an earlier engagement's swing), and the spell path may have
    // no attacker row at all (an environmental / departed caster → not a player, no threat).
    let attacker = entities.guid().find(attacker_guid);
    if attacker
        .as_ref()
        .is_some_and(|attacker| !crate::combat::may_harm(ctx, attacker, &target))
    {
        return miss;
    }
    let attacker_is_player = attacker.as_ref().map(|a| a.is_player()).unwrap_or(false);
    let attacker_owner = attacker.as_ref().map(|a| a.owner_guid).unwrap_or(0);
    let target_is_player = target.is_player();
    if damage.lethal_prevented && dmg == 0 {
        commit_death_prevention(ctx, target_guid, attacker_guid);
        return miss;
    }

    // 1. Attacker-side gains, before the fork (rage lands on the killing blow too).
    if weapon {
        if let Some(a) = &attacker {
            if is_rage_user(a) {
                // Fresh mutable copy (read-your-writes): `a` is still needed read-only below.
                if let Some(mut fresh) = entities.guid().find(attacker_guid) {
                    fresh.power = (fresh.power + rage_from_damage(dmg, true)).min(fresh.max_power);
                    entities.guid().update(fresh);
                }
            }
            // Weapon skill-up: a PLAYER landing a damaging hit on a CREATURE trains the equipped
            // weapon's skill line one step toward cap. Touches only `game_player_skill` rows, so it is
            // independent of the rage write above. A no-op at cap (the normal case).
            if attacker_is_player && !target_is_player {
                crate::skill::gain_weapon_skill(ctx, a);
            }
        }
    }

    // 2. The lethal fork. An active opponent Duel floors its loser at one health and completes
    // before any player-death work can run, regardless of whether the damage was weapon or spell.
    if target_is_player
        && is_lethal(target.health, dmg)
        && crate::duel::active_opponents(ctx, attacker_guid, target_guid)
    {
        target.health = 1;
        entities.guid().update(target);
        if let Some(duel) = crate::duel::active_duel_between(ctx, attacker_guid, target_guid) {
            crate::duel::complete_duel(
                ctx,
                duel.id,
                crate::duel::duel_completion_kind::WON,
                attacker_guid,
                target_guid,
            );
        }
        // The duel finisher dealt real damage to a target that survived it, so both sides' Procs fire,
        // exactly as they would on any other surviving hit.
        crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, true);
        return HitOutcome {
            killed: false,
            duel_completed: true,
        };
    }
    // A spell hit on a PLAYER is the one non-fatal ordinary 0-hp case (floored at 1 below).
    let killed = is_lethal(target.health, dmg) && (weapon || !target_is_player);
    if killed {
        if target_is_player {
            kill_player(ctx, target_guid, attacker_guid);
        } else {
            kill_creature(
                ctx,
                target_guid,
                if attacker_is_player {
                    Some(attacker_guid)
                } else if weapon && attacker_owner != 0 {
                    // A PET's killing blow credits its OWNER, so a Warlock levels + loots off its Imp.
                    Some(attacker_owner)
                } else {
                    None // a wild creature's kill, or a caster that left the world, credits nobody
                },
            );
        }
        // A killing blow still fires the ATTACKER's "dealt" Procs (a Lightning Shield zap that lands
        // the kill still counts); the corpse's own "taken" Procs do not.
        crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, false);
        return HitOutcome {
            killed: true,
            duel_completed: false,
        };
    }
    // 3. The survivor path.
    // Capture the threat predicate before `target` is moved by the write below.
    let accrues_threat = attacker_is_player && !target_is_player;
    if weapon && target_is_player {
        // Defense skill-up: a PLAYER that takes a damaging weapon hit and SURVIVES trains Defense one
        // step toward cap. Read-only on `target`; a no-op at cap (the normal case).
        crate::skill::gain_defense_skill(ctx, &target);
    }
    target.health = crate::spell::damaged_value(target.health, dmg as i32);
    if weapon && is_rage_user(&target) {
        target.power = (target.power + rage_from_damage(dmg, false)).min(target.max_power);
    }
    entities.guid().update(target);
    if damage.lethal_prevented {
        commit_death_prevention(ctx, target_guid, attacker_guid);
    }
    // Damage kept the fight alive: restart the pursuit deadline and re-remember where the creature
    // stands. Both guids go in, so a creature hitting the player refreshes the leash just like the
    // player hitting it. The lethal branch above already returned — a dead pair has no engagement left.
    refresh_leash(ctx, attacker_guid, target_guid);
    if !weapon {
        // The surviving spell target is in combat too. Weapon callers stamp both sides at their own
        // tail (a swing that fired flags combat even on a miss), so this would be redundant for them.
        enter_combat(ctx, target_guid);
    }

    // 4. Break-on-damage drops the target's break-on-damage CC (polymorph etc.) and interrupts its
    // in-progress timed cast (direct damage → `periodic = false`). The attacker guid rides along for a
    // WEAPON hit so Retaliation can counter-swing the assailant; everything else passes the `0`
    // sentinel (a counter-swing answers a real incoming swing only).
    crate::spell::break_auras_on_damage(
        ctx,
        target_guid,
        if weapon { attacker_guid } else { 0 },
        false,
    );
    // Threat accrues on the creature's table so the retarget pass targets the highest-threat attacker.
    // Player → creature only; the lethal branch already returned (a killing blow wiped the table).
    if accrues_threat {
        crate::threat::add_threat(
            ctx,
            target_guid,
            attacker_guid,
            crate::threat::threat_from_damage(dmg),
        );
        if !weapon {
            // ENGAGE-ON-DAMAGE: a player's spell hit pulls the creature into melee (the ranged twin of
            // `aggro_pass`; the melee paths already have their engagement row by construction).
            // Idempotent — shared with the cast-level engage in `resolve_cast_at`.
            crate::spell::arm_spell_retaliation(ctx, attacker_guid, target_guid);
        }
    }
    // 5. The proc pass. Both sides fire here: the attacker's "dealt" Procs and — because the target
    // lived — its "taken" Procs too (Frost Armor chills whoever hit its wearer).
    crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, true);
    HitOutcome {
        killed: false,
        duel_completed: false,
    }
}

#[cfg(test)]
mod lethality_tests {
    use super::{is_lethal, lethal_floor_amount, CreatureDeathAttribution, HitOutcome};

    // The one shared lethality predicate: the melee swing needs it up front for its wire event's
    // `killing_blow` flag and `apply_hit` needs it again for the fork, so they must not be two
    // hand-written expressions.
    #[test]
    fn a_zero_damage_hit_is_never_lethal() {
        // A miss, a fully-absorbed swing, and a godmode target all arrive here as 0 damage. None of
        // them may kill — not even a target already sitting at 0 hp.
        assert!(!is_lethal(1, 0));
        assert!(!is_lethal(0, 0));
    }

    #[test]
    fn a_hit_at_or_above_remaining_health_is_lethal() {
        assert!(is_lethal(10, 10)); // exactly lethal
        assert!(is_lethal(10, 11)); // overkill
        assert!(!is_lethal(10, 9)); // one short — the target survives at 1 hp
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

        let force = crate::test_scan::code_of(
            include_str!("death.rs"),
            "pub(crate) fn force_creature_death(",
        );
        assert!(force.contains("CreatureDeathAttribution::suicide(creature_guid)"));
        let death = crate::test_scan::code_of(
            include_str!("death.rs"),
            "fn kill_creature_with_attribution(",
        );
        assert!(death.contains("let killer = attribution.source_guid"));
        assert!(death.contains("let reward_killer = attribution.reward_guid"));
        assert!(death.contains("let kill_recipients = reward_killer"));
        assert!(death.contains("killer_guid: killer.unwrap_or(0)"));
    }

    #[test]
    fn active_duel_lethal_hits_bypass_the_player_death_chokepoint() {
        let body = crate::test_scan::code_of(include_str!("death.rs"), "pub(crate) fn apply_hit(");
        let duel_floor = body
            .find("crate::duel::active_opponents")
            .expect("duel finisher is checked in the shared damage seam");
        let player_death = body
            .find("kill_player(ctx, target_guid, attacker_guid)")
            .expect("ordinary player death remains behind the shared seam");
        assert!(duel_floor < player_death);
        assert!(body.contains("target.health = 1;"));
        assert!(body.contains("duel_completion_kind::WON"));
    }

    #[test]
    fn duel_completion_is_a_combat_terminal_without_becoming_a_death() {
        let outcome = HitOutcome {
            killed: false,
            duel_completed: true,
        };
        assert!(outcome.combat_ended());

        let body =
            crate::test_scan::code_of(include_str!("death.rs"), "pub(crate) fn kill_player(");
        assert!(body.contains("crate::duel::interrupt_duel_for(ctx, victim_guid)"));
    }

    #[test]
    fn death_hooks_run_before_engagement_reset_on_player_and_pet_death() {
        let src = include_str!("death.rs");
        let creature = crate::test_scan::code_of(src, "fn kill_creature_with_attribution(");
        let pet_end = creature
            .find("let victim_entry = target.entry")
            .expect("pet death stays before ordinary creature death");
        assert_in_order(
            &creature[..pet_end],
            [
                "begin_death_dispatch",
                "disengage(ctx, target_guid)",
                "fire_on_death",
                "finish_death_dispatch",
            ],
        );

        let player = crate::test_scan::code_of(src, "pub(crate) fn kill_player(");
        assert_in_order(
            &player,
            [
                "begin_death_dispatch",
                "disengage(ctx, victim_guid)",
                "fire_on_death",
                "finish_death_dispatch",
            ],
        );
    }

    #[test]
    fn creature_death_carries_the_selected_opponent_across_disengage() {
        let body = crate::test_scan::code_of(
            include_str!("death.rs"),
            "fn kill_creature_with_attribution(",
        );
        assert_in_order(
            &body,
            [
                "let current_target_guid = target.target_guid",
                "disengage(ctx, target_guid)",
                "current_target_guid,",
            ],
        );
    }

    fn assert_in_order<const N: usize>(body: &str, needles: [&str; N]) {
        let mut cursor = 0;
        for needle in needles {
            let offset = body[cursor..]
                .find(needle)
                .unwrap_or_else(|| panic!("missing `{needle}` in death producer"));
            cursor += offset + needle.len();
        }
    }
}

#[cfg(test)]
mod corpse_residue_tripwire {
    use crate::test_scan::code_of;

    /// Issue #358: `roll_pickpocket_loot` inserts `game_corpse_loot` rows at slots 0.. on the LIVE
    /// creature's guid; if the mob dies before every row is taken, `roll_creature_loot` re-inserts
    /// kill drops starting at slot 0 on the SAME guid, producing duplicate `(corpse_guid, slot)` pairs
    /// that every first-match loot consumer addresses ambiguously. The fix is a purge that MUST run
    /// on every kill, BEFORE the fresh roll — this is `ReducerContext` glue invisible to a behavioural
    /// test (no in-process DB harness here), so the wiring is pinned directly, in two parts since
    /// issue #382 extracted the corpse/loot step out of `kill_creature`'s inline body:
    /// `kill_creature` must still route every kill through `roll_corpse_loot`, and `roll_corpse_loot`
    /// itself must call `loot::purge_corpse_residue` strictly before `roll_creature_loot`. Losing any
    /// of the three pieces silently reopens the collision.
    #[test]
    fn kill_creature_purges_corpse_residue_before_rolling_fresh_loot() {
        let src = include_str!("death.rs");
        let kill_creature_body = code_of(src, "fn kill_creature_with_attribution(");
        assert!(
            kill_creature_body.contains("roll_corpse_loot(")
                && kill_creature_body.contains("reward_killer,")
                && kill_creature_body.contains("&kill_recipients,"),
            "`kill_creature` no longer routes the kill through `roll_corpse_loot` — the purge-before-\
             fresh-roll ordering below is dead code if this call is gone. Body was:\n{kill_creature_body}"
        );
        let body = code_of(src, "fn roll_corpse_loot(");
        assert!(
            body.contains("crate::loot::purge_corpse_residue(ctx, target_guid)"),
            "`roll_corpse_loot` no longer purges corpse residue (game_corpse_loot / \
             game_corpse_loot_eligible) for the dying guid — a leftover pickpocket row can collide \
             with a freshly-rolled kill-drop slot again (issue #358). Body was:\n{body}"
        );
        let purge_at = body
            .find("crate::loot::purge_corpse_residue(ctx, target_guid)")
            .expect("checked above");
        let roll_at = body
            .find("crate::loot::roll_creature_loot(ctx, target.entry, target_guid)")
            .expect("roll_corpse_loot no longer calls roll_creature_loot with its usual signature");
        assert!(
            purge_at < roll_at,
            "`purge_corpse_residue` must run BEFORE `roll_creature_loot` — reordering it after \
             lets the fresh kill-drop rows collide with residue that hasn't been cleared yet."
        );
    }
}
