//! Combat — melee auto-attack, **bidirectional** with **per-unit attack speed**: each unit swings on
//! its own `base_attack_time_ms`, and a creature that is attacked **retaliates** and swings back at its
//! own speed. Each swing writes a `game_combat_event` row for the gateway to relay as
//! `SMSG_ATTACKERSTATEUPDATE` (swing animation + damage text); the health drop flows through the
//! `game_world_entity` `on_update` → VALUES relay. A creature target dies at 0 HP (DESTROY + respawn);
//! a **player** target also dies at 0 HP (health=0 + `dead` → the death/release/ghost corpse-run, see
//! `world::repop`). [entity]/[event]/[server]

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use crate::helpers::entity_by_owner;
use crate::{
    game_aura, game_creature_move_event, game_creature_spawn, game_creature_template,
    game_faction_template, game_item_instance, game_item_template, game_spell,
    game_spell_cast_event, game_spell_effect, game_threat, game_world_entity, ItemInstance,
    SpellCastEvent, WorldEntity,
};

mod tables;
// The ctx-free hit-table / damage / regen math + its consts live in `tables.rs`. `pub use tables::*`
// both brings them into scope for the ctx/db layer below AND re-exports them, so every
// `crate::combat::<sym>` path resolves regardless of which submodule actually defines the symbol.
pub use tables::*;

// The corpse-lifecycle timers below are used by the ctx/db code in this file (kill_creature, the decay
// pass).
pub(crate) const RESPAWN_MICROS: i64 = 15_000_000; // 15s after the corpse despawns
pub(crate) const CORPSE_DECAY_MICROS: i64 = 60_000_000; // a corpse lingers 60s before it despawns

// Warlock soul-shard generation: Drain Soul's real vanilla script effect (`ChannelDeathItem`) mints a
// Soul Shard when its channel's target dies mid-cast. Our Drain Soul import carries no Rust script hook
// for that effect, so the grant lives here instead, at the ONE place every creature death already
// funnels through. `DRAIN_SOUL_SPELL_ID` is the real vanilla spell id (1120); `SOUL_SHARD_ENTRY` is the
// real vanilla item id (6265, hand-seeded in `seed.rs` since the .import ETL doesn't reliably carry it).
pub(crate) const DRAIN_SOUL_SPELL_ID: u32 = 1120;
pub(crate) const SOUL_SHARD_ENTRY: u32 = 6265;

/// Fold a defender combat-rating aura (`A_MOD_COMBAT(field)`, e.g. a talent) onto an attack-table band:
/// `base + combat_field_bonus(target, field)`, clamped to `[0, 10000]` so the band never goes negative or
/// past the full roll line. `bonus == 0` (no such aura) → exactly `base` (baseline-safe). NOTE: unlike the
/// attacker AP/crit/hit folds (which sum aura + EQUIPPED-gear ratings), these defender ratings are
/// aura-only for now — no gear sources parry/dodge/block rating yet, so only `combat_field_bonus`. [entity]
fn fold_band(ctx: &ReducerContext, target_guid: u64, base: u32, field: u8) -> u32 {
    (base as i32 + crate::spell::combat_field_bonus(ctx, target_guid, field)).clamp(0, 10_000)
        as u32
}

/// `target`'s EFFECTIVE dodge chance (bp) vs a swing with skill difference `sd`: the agility-derived
/// `dodge_chance_bp` PLUS any `A_MOD_COMBAT(COMBAT_DODGE)` aura. `roll_swing` builds the dodge band from
/// this and `debug_compute_swing` reads it back. No dodge aura → exactly the agility band (baseline-safe).
pub fn effective_dodge_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        dodge_chance_bp(effective_agility(ctx, target), sd),
        crate::spell::COMBAT_DODGE,
    )
}

/// `target`'s EFFECTIVE parry chance (bp) vs a swing with skill difference `sd`: `parry_chance_bp(sd)` PLUS
/// any `A_MOD_COMBAT(COMBAT_PARRY)` aura (e.g. the Deflection talent). No parry aura → the flat band
/// (baseline-safe). The parry twin of `effective_crit_bp`/`effective_dodge_bp`. [entity]
pub fn effective_parry_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        parry_chance_bp(sd),
        crate::spell::COMBAT_PARRY,
    )
}

/// `target`'s EFFECTIVE block chance (bp) vs a swing with skill difference `sd`: the `block_chance_bp(sd)`
/// band PLUS any `A_MOD_COMBAT(COMBAT_BLOCK)` aura (e.g. the Shield Specialization talent) — but ONLY when
/// the defender has a shield (`block_value > 0`, passed in by the caller so the gate uses the SAME value
/// the HIT_BLOCK damage subtraction does — no recompute, no split-gate divergence). An unshielded unit
/// blocks at 0 regardless of any block aura (you can't block without a shield) — baseline-safe. [entity]
pub fn effective_block_bp(
    ctx: &ReducerContext,
    target: &WorldEntity,
    sd: u32,
    block_value: u32,
) -> u32 {
    if block_value > 0 {
        fold_band(
            ctx,
            target.guid,
            block_chance_bp(sd),
            crate::spell::COMBAT_BLOCK,
        )
    } else {
        0 // no shield → no block band (a block aura is inert without a shield)
    }
}

/// The total attack-power BONUS from a unit's active self-buff auras (e.g. Battle Shout): the sum of
/// `effect_basis` over every aura on `unit_guid` whose spell is a positive-basis self-buff. The
/// convention — a self-buff aura carrying a positive `effect_basis` grants that many attack-power
/// points while active — connects the aura tier to combat: a buffed attacker's effective AP rises, so
/// the swing range (computed via the `_ap` cores) is strictly higher while the buff is up. Delegates to
/// the spell engine's TYPED active-aura read (`combat_field_bonus` sums the frozen `amount` of every
/// `A_MOD_COMBAT` aura on the unit whose `eff_p0 == COMBAT_ATTACK_POWER`). Returns 0 for an un-buffed
/// unit (clamped ≥0), so a no-aura swing is byte-identical to today's range. [entity]
fn aura_attack_power_bonus(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    crate::spell::combat_field_bonus(ctx, unit_guid, crate::spell::COMBAT_ATTACK_POWER).max(0)
        as u32
}

/// Holy damage a unit's active SEAL aura adds to a LANDED melee swing (Seal of Righteousness): the summed
/// `A_SEAL` aura value, weapon-speed-weighted — vanilla SoR is per-swing, scaled by weapon speed. `(0, 0)`
/// for a unit with no seal (baseline-safe). Pull model — the swing READS the seal aura, no per-spell code.
/// Also returns the seal's SPELL id so the swing can name the yellow proc line after it (114).
fn seal_holy_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
    let (amount, spell) = crate::spell::seal_amount_and_spell(ctx, attacker.guid);
    (seal_swing_holy(amount, attacker.base_attack_time_ms), spell)
}

/// Pure: holy damage a seal of coefficient `seal` adds to one swing on an `attack_time_ms`-speed weapon —
/// vanilla SoR ≈ seal_coeff × weapon_speed / 100000 (coeff 108@r1 / 216@r10; a 2.0s weapon → ~2 / ~4). 0
/// for no seal (coeff ≤ 0), so an unsealed swing is byte-identical. [pure]
fn seal_swing_holy(seal: i32, attack_time_ms: u32) -> u32 {
    if seal <= 0 {
        return 0;
    }
    ((seal as u64 * attack_time_ms as u64) / 100_000) as u32
}

/// Pure: the bonus damage a queued on-next-swing strike (`queued_spell_id`, e.g. Heroic Strike) adds to a
/// LANDED melee swing, given the queued spell's E_NEXT_SWING effect `base_points`. 0 when nothing is queued
/// (`queued_spell_id == 0`) or the queued bonus is non-positive (`base_points <= 0`), so an un-queued swing
/// is byte-identical. The bonus is the flat effect base (negative bases are clamped to 0 — a queued strike
/// only ever ADDS). [pure]
fn queued_strike_bonus(queued_spell_id: u32, base_points: i32) -> u32 {
    if queued_spell_id == 0 {
        return 0;
    }
    base_points.max(0) as u32
}

/// Bonus damage the attacker's QUEUED on-next-swing strike (Heroic Strike 78 / Cleave 845) adds to a LANDED
/// melee swing — the pull-model twin of `seal_holy_on_swing`. Reads `attacker.next_swing_spell`; if non-zero,
/// looks up that spell's E_NEXT_SWING effect `base_points` (the flat queued bonus) and returns it. 0 for an
/// un-queued attacker (the common path). Does NOT clear the queue — `clear_queued_strike` does that whenever
/// a swing FIRES (so a miss spends the queue too). Generic over the kind, never a per-spell branch.
fn queued_strike_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let queued = attacker.next_swing_spell;
    if queued == 0 {
        return 0;
    }
    // The queued bonus is the base_points of the spell's E_NEXT_SWING effect (Heroic Strike's flat +N).
    let base_points = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&queued)
        .find(|e| e.kind == crate::spell::E_NEXT_SWING)
        .map(|e| e.base_points)
        .unwrap_or(0);
    queued_strike_bonus(queued, base_points)
}

/// Clear `guid`'s queued on-next-swing strike (set `next_swing_spell = 0`) — called whenever a swing FIRES
/// (landed OR missed), so a missed swing spends the queue too (vanilla: the on-next-swing slot is consumed).
/// Re-fetches a fresh mutable copy (read-your-writes) like the rage block. No-op if nothing is queued or the
/// unit left the world.
fn clear_queued_strike(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if e.next_swing_spell != 0 {
            e.next_swing_spell = 0;
            entities.guid().update(e);
        }
    }
}

/// Reactive-ability window (ms): how long an Overpower / Revenge proc stays usable after it is armed.
/// Vanilla ~5s. The attack table stamps `*_until_ms = now + this`; the cast gate refuses the react spell
/// once `now >= *_until_ms`.
pub(crate) const REACT_WINDOW_MS: u64 = 5000;

/// Which react window a dodge/avoid arms.
#[derive(Clone, Copy)]
enum ReactKind {
    /// The ATTACKER got DODGED → arm its Overpower.
    Overpower,
    /// The DEFENDER dodged/parried/blocked an incoming swing → arm its Revenge.
    Revenge,
}

/// Pure: is a react window armed at `now_ms` given its stored deadline `until_ms`? A window is OPEN while
/// `until_ms > now_ms` (a 0 deadline — never armed, or already lapsed — is closed). The cast gate's check.
/// [pure]
pub(crate) fn react_window_active(until_ms: u64, now_ms: u64) -> bool {
    until_ms > now_ms
}

/// Arm a react window on `guid`: stamp `now_ms + REACT_WINDOW_MS` onto the matching `*_until_ms` field. Used
/// by the attack table — a DODGE of this unit's swing arms Overpower; a dodge/parry/block by this unit arms
/// Revenge. Re-fetches a fresh mutable copy (read-your-writes), since the row's own damage `update` happens
/// later in the swing loop. No-op if the unit left the world.
fn arm_react(ctx: &ReducerContext, guid: u64, kind: ReactKind) {
    let entities = ctx.db.game_world_entity();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    if let Some(mut e) = entities.guid().find(guid) {
        let until = now_ms + REACT_WINDOW_MS;
        match kind {
            ReactKind::Overpower => e.overpower_until_ms = until,
            ReactKind::Revenge => e.revenge_until_ms = until,
        }
        entities.guid().update(e);
    }
}

/// A unit's EFFECTIVE Strength: the stored base attribute plus every active `A_MOD_STAT(STR)` aura
/// (e.g. Mark of the Wild) AND the STR summed across its EQUIPPED gear (item stat bonuses). Folded in
/// before AP/swing derivation so a +STR buff OR a +STR chest is mechanically real (it raises the swing).
/// Aura + gear are added ALONGSIDE the base. Clamped ≥0 so a debuff exceeding base never underflows the
/// `u32`. A unit with no aura AND no stat-bearing gear (a creature, an unequipped player, the starter
/// loadout) gets +0 → the base value, so its readout is byte-identical to before (baseline-safe). [entity]
fn effective_strength(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.strength as i32,
        crate::spell::stat_bonus(ctx, e.guid, crate::spell::STAT_STR),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Strength),
    )
}

/// A unit's EFFECTIVE Agility: base plus every active `A_MOD_STAT(AGI)` aura plus the AGI summed across
/// its EQUIPPED gear. The defender's effective agility drives dodge (the avoidance twin of how effective
/// Strength drives the swing), so an agility buff OR a +AGI item actually raises dodge chance. Aura +
/// gear added alongside the base; clamped ≥0; no aura and no gear → the base value (baseline-safe). [entity]
fn effective_agility(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.agility as i32,
        crate::spell::stat_bonus(ctx, e.guid, crate::spell::STAT_AGI),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Agility),
    )
}

/// A unit's EFFECTIVE armor: the stored base armor (the player's agility*2 line) plus every active
/// `A_MOD_RESISTANCE(armor)` aura plus the bonus armor summed across its EQUIPPED gear — the gear armor
/// is ADDED ON TOP of the agility*2 base, NOT a re-derivation (no double-count). The defender's effective
/// armor drives physical-damage mitigation, so an armor buff OR an armored piece actually softens
/// incoming hits. Clamped ≥0; no aura and no armor gear → the base value (mitigation byte-identical to
/// before — baseline-safe). [entity]
pub fn effective_armor(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.armor as i32,
        crate::spell::resistance_bonus(ctx, e.guid, crate::spell::RESIST_ARMOR),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Armor),
    )
}

/// A unit's EFFECTIVE shield BLOCK VALUE — the flat physical damage a blocked swing absorbs: the
/// equipped off-hand shield's base `block_value` plus a Strength term (`Str / 20`, vanilla-ish), or 0 if
/// the unit has no shield, has a non-shield in the off-hand, or the shield is broken. Drives BOTH the
/// block-band gate (`effective_block_bp`) and the flat subtraction on a `HIT_BLOCK` swing. Creatures carry
/// no equipment so this is always 0 for them; an unshielded player is likewise 0 — so the block band is
/// 0 and combat is byte-identical until a real shield is equipped (baseline-safe). [entity]
pub fn effective_block_value(ctx: &ReducerContext, defender: &WorldEntity) -> u32 {
    let Some(inst) =
        crate::items::item_in_slot(ctx, defender.guid, crate::items::equip_slot::OFFHAND)
    else {
        return 0; // nothing in the off-hand → no shield
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(inst.entry) else {
        return 0; // dangling instance (no template) → contribute nothing
    };
    if tmpl.inventory_type != crate::items::invtype::SHIELD
        || crate::items::item_is_broken(&tmpl, &inst)
    {
        return 0; // a non-shield off-hand, or a broken shield, blocks nothing (mirrors broken-weapon=unarmed)
    }
    // Block value = the shield's flat base + Str/20 (the shielded defender's Strength term). The base is
    // clamped ≥0 (an i32 column; today every shield is ≥0); Strength uses the EFFECTIVE value so a +STR
    // buff/gear raises block too.
    let from_strength = effective_strength(ctx, defender) / 20;
    (tmpl.block_value.max(0) as u32).saturating_add(from_strength)
}

/// The attacker's EFFECTIVE crit chance in basis points: the flat `CRIT_BP` base plus every active
/// `A_MOD_COMBAT(CRIT)` aura (a crit-rating buff like the test "Combat Insight"), clamped to
/// `[0, 10000]`. `bonus == 0` → exactly `CRIT_BP` (the no-aura crit band is byte-identical to before).
/// The ctx-aware twin of how `effective_strength` folds a stat buff into the swing — `roll_swing` builds
/// the table's crit band from this and `debug_compute_swing` reads it back. [entity]
pub fn effective_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    // Aura crit rating (A_MOD_COMBAT(CRIT)) PLUS the crit rating summed across equipped gear — both fold
    // into the same flat `CRIT_BP` base via the pure core. No aura and no +crit gear → bonus 0 → exactly
    // `CRIT_BP` (the no-gear/no-aura crit band is byte-identical to before — baseline-safe).
    // Agility-derived crit, level-suppressed, ADDED to the aura+gear bonus so a higher-Agility attacker
    // crits more (a zero-Agility creature contributes 0 → the flat base, baseline-safe).
    let bonus = crate::spell::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_CRIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Crit)
        + agility_crit_bp(effective_agility(ctx, attacker), attacker.level) as i32;
    crit_bp_with_bonus(bonus)
}

/// The attacker's EFFECTIVE miss chance in basis points for a swing with the given `skill_diff` (the
/// caller computes it via `crate::skill::skill_diff_ctx`): the skill-derived `miss_chance_bp` base MINUS
/// every active `A_MOD_COMBAT(HIT)` aura + gear hit rating (hit rating REDUCES the chance to miss),
/// floored at 0. `bonus == 0` → exactly `miss_chance_bp(skill_diff)` (the no-aura miss band is
/// byte-identical to before). The hit-rating twin of `effective_crit_bp` — `roll_swing` builds the
/// table's miss band from this and `debug_compute_swing` reads it back. [entity]
pub fn effective_miss_bp(ctx: &ReducerContext, attacker: &WorldEntity, skill_diff: u32) -> u32 {
    // Aura hit rating (A_MOD_COMBAT(HIT)) PLUS the hit rating summed across equipped gear — both REDUCE
    // the miss band (the pure core subtracts the bonus, floored at 0). No aura and no +hit gear → bonus 0
    // → exactly `miss_chance_bp(skill_diff)` (the no-gear/no-aura miss band is byte-identical — baseline-safe).
    let bonus = crate::spell::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_HIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Hit);
    miss_bp_with_bonus(miss_chance_bp(skill_diff), bonus)
}

/// A unit's EFFECTIVE swing interval (ms): its `base_attack_time_ms` adjusted by every active
/// `A_MOD_SPEED(SPEED_SWING)` aura (signed haste/slow %, summed). The melee tick gates each swing on this,
/// so a haste buff (e.g. Quickening) makes the unit attack faster and an attack-speed SLOW (e.g. Thunder
/// Clap, a negative %) makes it slower — the swing-timer twin of the crit/hit folds. No speed aura → the
/// base time (the un-buffed swing cadence is byte-identical — baseline-safe). [entity]
pub fn effective_swing_time(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let speed = crate::spell::speed_bonus(ctx, attacker.guid, crate::spell::SPEED_SWING);
    swing_time_with_haste(attacker.base_attack_time_ms, speed)
}

/// A unit's EFFECTIVE movement speed (yd/s) for a `base` cadence (RUN / WALK): adjusted by every active
/// `A_MOD_SPEED(SPEED_MOVE)` aura (signed %, summed) — a snare (Hamstring, negative %) slows it, a speed
/// buff quickens it. The creature movement passes (chase / return / wander) compute their step + leg
/// duration from this instead of the raw constant, so a snared creature closes/roams slower — the
/// movement twin of `effective_swing_time`, server-verifiable via the creature's per-tick position delta.
/// No move aura → exactly `base` (baseline-safe). Players move client-side, so a player snare additionally
/// needs a `SMSG_FORCE_RUN_SPEED_CHANGE` wire push (deferred); this fold governs SERVER-driven movement. [entity]
pub fn effective_move_speed(ctx: &ReducerContext, guid: u64, base: f32) -> f32 {
    let pct = crate::spell::speed_bonus(ctx, guid, crate::spell::SPEED_MOVE);
    move_speed_with_pct(base, pct)
}

/// The player's equipped main-hand WEAPON damage profile `(damage_min, damage_max, delay_ms)`, or
/// `None` when unarmed (no main-hand item, missing template, or a non-weapon in the slot → fall back
/// to the unarmed range). Looks up the owned item instance in the main-hand equip slot (15) and joins
/// its `game_item_template`. The tooltip `damage_min`/`damage_max` are `f32`; rounded to whole points
/// for the integer swing math (vanilla weapon damage is whole numbers). [entity]
fn equipped_weapon_damage(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32, u32)> {
    let inst = crate::items::item_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    )?;
    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
    if tmpl.class != lyracore_shared::constants::starter_item::CLASS_WEAPON {
        return None; // a non-weapon equipped in the main hand contributes no weapon damage
    }
    if crate::items::item_is_broken(&tmpl, &inst) {
        return None; // a BROKEN weapon (has durability and it hit 0) gives no damage — swings unarmed
    }
    Some((
        tmpl.damage_min.round() as u32,
        tmpl.damage_max.round() as u32,
        tmpl.delay_ms,
    ))
}

/// The player's equipped OFF-HAND WEAPON damage profile `(damage_min, damage_max, delay_ms)` — dual
/// wield. `None` when the off-hand is empty, holds a non-weapon (a shield/holdable frill, which
/// `effective_block_value`/nothing else already reads), or holds a BROKEN weapon. The off-hand twin
/// of `equipped_weapon_damage`; unlike main-hand there is no unarmed fallback — no live off-hand weapon
/// means no second swing stream at all (the tick simply skips the off-hand roll). [entity]
fn equipped_offhand_weapon_damage(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32)> {
    let inst = crate::items::item_in_slot(ctx, player_guid, crate::items::equip_slot::OFFHAND)?;
    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
    if tmpl.class != lyracore_shared::constants::starter_item::CLASS_WEAPON {
        return None; // a shield/holdable off-hand contributes no off-hand swing (0, unchanged)
    }
    if crate::items::item_is_broken(&tmpl, &inst) {
        return None; // a broken off-hand weapon can't swing
    }
    Some((
        tmpl.damage_min.round() as u32,
        tmpl.damage_max.round() as u32,
        tmpl.delay_ms,
    ))
}

/// The player's equipped RANGED weapon damage profile `(damage_min, damage_max, delay_ms)` from the
/// ranged slot (17) — a bow/gun/crossbow (Auto Shot) or a wand (Shoot) — or `None` when nothing usable
/// is equipped there. The ranged twin of `equipped_weapon_damage`; the ranged swing tick uses `delay_ms`
/// as its swing interval and rolls damage from `[damage_min, damage_max]`. [entity]
fn equipped_ranged_weapon(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32, u32, u8)> {
    let inst = crate::items::item_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)?;
    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
    if tmpl.class != lyracore_shared::constants::starter_item::CLASS_WEAPON {
        return None; // a non-weapon in the ranged slot (a relic/quiver) contributes no shot damage
    }
    if crate::items::item_is_broken(&tmpl, &inst) {
        return None; // a broken ranged weapon can't fire
    }
    Some((
        tmpl.damage_min.round() as u32,
        tmpl.damage_max.round() as u32,
        tmpl.delay_ms,
        tmpl.subclass, // weapon subclass: BOW=2, GUN=3, CROSSBOW=18, WAND=19, etc.
    ))
}

/// The player's loaded ammo: the lowest-slot inventory item that is Projectile ammo (item class 6 —
/// arrows subclass 2 / bullets 3) with stock left. Bow/gun/crossbow shots consume one per shot; `None` =
/// out of ammo (the engagement ends). Deliberate simplification: LOOSE match — any class-6 item
/// fires (we don't enforce the bow→arrow / gun→bullet subclass pairing); any ammo is fine for v1.
fn find_ammo(ctx: &ReducerContext, player_guid: u64) -> Option<ItemInstance> {
    let templates = ctx.db.game_item_template();
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .filter(|i| {
            i.stack_count > 0
                && templates
                    .entry()
                    .find(i.entry)
                    .is_some_and(|t| t.class == 6)
        })
        .min_by_key(|i| i.slot)
}

/// Per-swing percent chance the main-hand weapon loses 1 durability. Vanilla wears weapons SLOWLY (the
/// ~10%-on-death loss is the dominant sink); a deterministic 1-per-swing loss would break a
/// ~70-durability weapon in ~2 min of combat — ~10× too fast for that model. Tunable; adding the
/// on-death 10% loss is the parity follow-up. [reference]
const DURABILITY_WEAR_CHANCE_PCT: u32 = 10;

/// Wear a player's equipped main-hand weapon by 1 durability with a `DURABILITY_WEAR_CHANCE_PCT`% chance
/// per swing (saturating at 0). At 0 the weapon is BROKEN — `equipped_weapon_damage` falls back to
/// unarmed until it is repaired (`items::apply_repair_item`). A no-op when there's no main-hand item;
/// creatures don't wear gear.
fn wear_weapon(ctx: &ReducerContext, player_guid: u64) {
    if let Some(mut inst) = crate::items::item_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    ) {
        // Probabilistic, not every swing (see DURABILITY_WEAR_CHANCE_PCT) — a deterministic per-swing
        // loss would wear weapons ~10× too fast. A miss/dodge still rolls wear here (per-hit-only is a
        // refinement).
        if inst.durability > 0 && ctx.random::<u32>() % 100 < DURABILITY_WEAR_CHANCE_PCT {
            inst.durability -= 1;
            ctx.db.game_item_instance().guid().update(inst);
        }
    }
}

/// The `[min, max]` damage range for one of `attacker`'s swings: the flat creature range (creatures
/// carry no attributes) SCALED by the creature's classification rank (elite/rare/boss hit harder), the
/// equipped-weapon range (a player wielding a main-hand weapon), or the unarmed stat range (an unarmed
/// player). The ctx-aware entry point `roll_swing` / `debug_compute_swing` use so the equipped weapon
/// (and now creature rank) actually drives live damage. The pure AP cores (`player_swing_range_ap` /
/// `weapon_swing_range_ap`) carry the unarmed/weapon math (and its unit tests); this layers the ctx
/// reads — effective Strength, aura AP, the equipped weapon, and the creature rank — on top.
pub fn swing_range_ctx(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
    // Warrior Disarm (A_DISARM): a disarmed attacker's main-hand weapon is stripped. Read ONCE — the
    // creature and player branches handle it differently (a creature has no weapon/unarmed split). `false`
    // for any un-disarmed unit, so the whole function stays byte-identical without an A_DISARM aura.
    let disarmed = crate::spell::is_disarmed(ctx, attacker.guid);
    if !attacker.is_player() {
        // Per-creature base swing: the template's imported cmangos MinMeleeDmg/MaxMeleeDmg,
        // falling back to the flat CREATURE_MELEE_MIN/MAX when not imported (`damage_max == 0`) so the
        // seed chicken + any un-updated row are byte-identical to before. Then ELITE/RARE/BOSS scaling
        // multiplies by the template rank (rank 0 → ×1.0).
        let tmpl = ctx.db.game_creature_template().entry().find(attacker.entry);
        let (base_min, base_max) = match &tmpl {
            Some(t) if t.damage_max > 0 => (t.damage_min, t.damage_max),
            _ => (CREATURE_MELEE_MIN, CREATURE_MELEE_MAX),
        };
        let rank = tmpl.map(|t| t.rank).unwrap_or(0);
        let (min, max) = scale_swing_for_rank(base_min, base_max, rank);
        if disarmed {
            // A creature has no weapon/unarmed split (its swing is a flat template range), so instead of
            // zeroing damage we retain a documented fraction (`DISARM_SWING_RETAINED_PCT`) — a tuning
            // approximation of "lost its weapon" that keeps the mechanic meaningful. See its const doc.
            let pct = crate::spell::DISARM_SWING_RETAINED_PCT;
            return (min * pct / 100, max * pct / 100);
        }
        return (min, max);
    }
    // Effective AP = the stat-derived base (from EFFECTIVE Strength, base + any A_MOD_STAT(STR) buff
    // like Mark of the Wild) + every active self-buff aura's AP (Battle Shout). Folded in BEFORE the
    // per-swing bonus so a buff scales with weapon speed, like vanilla. With no auras the effective
    // Strength equals base and `extra == 0` → the range is byte-identical to the delegating wrappers.
    // Class-specific AP curve: Rogue/Hunter fold in effective AGILITY; every other class keeps the
    // Str-class curve (byte-identical).
    let class = attacker.class();
    let ap = melee_attack_power_for(
        class,
        effective_strength(ctx, attacker),
        effective_agility(ctx, attacker),
        attacker.level,
    ) + aura_attack_power_bonus(ctx, attacker.guid);
    // A DISARMED player ignores the equipped main-hand weapon and swings for its UNARMED range (the
    // player has a real unarmed damage model, unlike a creature). An un-disarmed player takes the
    // equipped-weapon path exactly as before (baseline-safe).
    if !disarmed {
        if let Some((dmin, dmax, delay)) = equipped_weapon_damage(ctx, attacker.guid) {
            return weapon_swing_range_ap(ap, dmin, dmax, delay);
        }
    }
    player_swing_range_ap(ap, attacker.base_attack_time_ms)
}

/// Damage of a PHYSICAL weapon-based ABILITY (Mortal Strike and other instant strikes) by
/// `caster` vs `target`: roll the caster's weapon swing range (`swing_range_ctx` — the same weapon + AP
/// the auto-attack uses), add the ability's flat `bonus`, roll a MELEE crit (`×CRIT_MULT` at the caster's
/// effective crit), then shave the target's armor (`armor_mitigation_pct`), floored at 1. Returns the
/// pre-absorb damage; the caller folds absorb + threat + the kill via `spell::apply_target_damage`. Reusing
/// the swing/crit/armor math means an ability scales off the weapon like a real strike, not a flat number.
/// (Slice scope: specials always land — no miss/dodge/parry roll yet; the attack table for yellow hits is
/// a follow-up.) Uses the module RNG (`ctx.random`, deterministic per reducer transaction). [entity]
pub fn weapon_strike_damage(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: &WorldEntity,
    flat_bonus: i32,
) -> u32 {
    let (min, max) = swing_range_ctx(ctx, caster);
    let base = min + ctx.random::<u32>() % (max - min + 1);
    let total = (base as i32 + flat_bonus).max(1) as u32;
    let pre_mit = if (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, caster) {
        total * CRIT_MULT
    } else {
        total
    };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), caster.level);
    (pre_mit * (100 - mit_pct) / 100).max(1)
}

/// Warrior Retaliation (`A_RETALIATE`): when `victim_guid` takes a REAL incoming melee hit from
/// `attacker_guid` and carries an active A_RETALIATE aura, fire ONE free main-hand counter-swing back at the
/// attacker. Reuses the E_WEAPON_STRIKE path — `weapon_strike_damage` (the victim's own weapon + AP + crit +
/// armor) rolls the blow, `apply_target_damage` folds absorb/threat/the kill. Hooked from
/// `break_auras_on_damage`'s melee block (`attacker_guid != 0`), so ONLY a genuine incoming swing triggers it.
///
/// No recursion / infinite loop: the counter-swing lands via `apply_target_damage`, which calls
/// `break_auras_on_damage` with the attacker sentinel `0` (it's an ability hit, not a tracked melee swing), so
/// the counter-swing can NEVER itself re-enter this fn — a Retaliating attacker's own aura is not provoked by
/// the counter, matching vanilla (only real incoming swings trigger). No-op for a victim without an
/// A_RETALIATE aura (the common path) or when either combatant is gone/dead.
///
/// Known limitation: the vanilla ~30-counter-per-Retaliation cap is DEFERRED — tracking a
/// per-aura counter needs a mutable bookkeeping field on the aura row; the 15s aura window
/// already bounds the effect for the slice.
/// [entity]
pub(crate) fn retaliate_on_hit(ctx: &ReducerContext, victim_guid: u64, attacker_guid: u64) {
    let carries_retaliate = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&victim_guid)
        .any(|a| a.eff_kind == crate::spell::A_RETALIATE);
    if !carries_retaliate {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let (Some(victim), Some(attacker)) = (
        entities.guid().find(victim_guid),
        entities.guid().find(attacker_guid),
    ) else {
        return;
    };
    if victim.dead || attacker.dead {
        return; // a dead combatant doesn't trade blows
    }
    let dmg = weapon_strike_damage(ctx, &victim, &attacker, 0);
    crate::spell::apply_target_damage(ctx, attacker_guid, victim_guid, dmg as i32);
}

/// Roll one melee swing of `attacker` against `target`: `(damage, hit_info, blocked_amount)`. One roll
/// picks the attack-table outcome (bands scaled by the attacker/target level difference, the defender's
/// agility, and — when the defender has a shield — a block band); a second roll gives the damage variance
/// for a landed hit. Avoidance (miss/dodge/parry) deals 0; glancing lands reduced damage and can't crit; a
/// crit is 2× the base; a BLOCK lands a normal hit minus the shield's flat `block_value` (the absorbed
/// amount is the third tuple element, for the wire). Uses the module RNG (`ctx.random`, deterministic per
/// reducer transaction).
fn roll_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
) -> (u32, u8, u32) {
    roll_swing_with_range(ctx, attacker, target, swing_range_ctx(ctx, attacker))
}

/// The shared core of `roll_swing`: identical attack-table roll + mitigation, but takes the `[min, max]`
/// damage range EXPLICITLY instead of deriving it from `attacker`'s equipped main-hand
/// (`swing_range_ctx`). Lets the off-hand swing in `resolve_swing` roll through the SAME shared attack
/// table + armor/block math over the off-hand's OWN (penalized) range, instead of re-deriving the
/// main-hand range. `roll_swing` is a thin wrapper that passes `swing_range_ctx(ctx, attacker)`.
fn roll_swing_with_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    range: (u32, u32),
) -> (u32, u8, u32) {
    // The defender's flat shield block value, computed ONCE: it both gates the block band (0 → no band,
    // baseline-safe) and is the flat amount subtracted on a HIT_BLOCK below.
    let block_value = effective_block_value(ctx, target);
    // The skill difference (defender defense skill − attacker weapon skill) driving the four skill-based
    // bands, computed ONCE. With no tracked skill rows this is exactly `(target.level − attacker.level)*5`
    // — the level-derived formula — so an all-untracked fight's table is byte-identical (baseline-safe).
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    let table = AttackTable {
        // Miss/crit fold in the attacker's A_MOD_COMBAT(HIT)/(CRIT) auras: hit rating REDUCES miss,
        // crit rating RAISES crit. With no such aura these equal the flat `miss_chance_bp`/`CRIT_BP`,
        // so an un-buffed swing's table is byte-identical to before.
        miss: effective_miss_bp(ctx, attacker, sd),
        // Dodge/parry/block fold the defender's A_MOD_COMBAT(DODGE/PARRY/BLOCK) auras (talents like
        // Deflection / Shield Spec) on top of the agility/skill bands. No such aura → the plain band
        // (baseline-safe). Block stays shielded-defender-only (0 without a shield, even with a block aura).
        dodge: effective_dodge_bp(ctx, target, sd),
        parry: effective_parry_bp(ctx, target, sd),
        glancing: glancing_chance_bp(attacker.level, target.level),
        // Block is a shielded-DEFENDER-only band (walks after glancing, before crit). 0 for any unit
        // without an equipped shield (every creature, an unshielded player), so those tables are
        // byte-identical to before (baseline-safe).
        block: effective_block_bp(ctx, target, sd, block_value),
        crit: effective_crit_bp(ctx, attacker),
        // Crushing is a CREATURE→PLAYER-only band: a mob 3+ levels above a player lands amplified hits.
        // 0 on every other path (player→mob, PvP, equal/lower-level creature), so those tables are
        // byte-identical to before (baseline-safe).
        crushing: if !attacker.is_player() && target.is_player() {
            crushing_chance_bp(attacker.level, target.level)
        } else {
            0
        },
    };
    let roll = ctx.random::<u32>() % 10_000;
    let outcome = attack_outcome(roll, &table);
    if matches!(outcome, HIT_MISS | HIT_DODGE | HIT_PARRY) {
        return (0, outcome, 0);
    }
    let (min, max) = range;
    let base = min + ctx.random::<u32>() % (max - min + 1);
    let pre_mit = match outcome {
        HIT_CRIT => base * CRIT_MULT,
        HIT_GLANCING => (base * GLANCING_DAMAGE_PCT / 100).max(1),
        HIT_CRUSHING => base * CRUSH_DAMAGE_PCT / 100, // amplified hit (150%) — the glancing twin
        _ => base, // HIT_NORMAL and HIT_BLOCK both land the plain base; block's reduction is applied below
    };
    // Functional armor: the target's armor shaves a percent off the landed hit, applied after
    // crit/glancing, vanilla-style. A 0-armor creature target is unaffected; the player's
    // agility-derived armor softens incoming creature swings. Floored at 1 (a landed hit always lands).
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), attacker.level);
    let dmg = (pre_mit * (100 - mit_pct) / 100).max(1);
    // Shield block: a HIT_BLOCK lands for (post-armor damage − flat block_value), clamped ≥0. This is a
    // physical mitigation like armor, applied here (before the spell-side damage-taken%/absorb folds in
    // resolve_swing). Unlike other landed hits it is NOT re-floored at 1 — a shield strong enough to
    // cover the whole swing is a vanilla "full block" (0 damage). `blocked` is the amount actually
    // absorbed (never more than the pre-block damage), reported on the wire so the client shows "Block N".
    if outcome == HIT_BLOCK {
        let blocked = block_value.min(dmg);
        return (dmg - blocked, HIT_BLOCK, blocked);
    }
    (dmg, outcome, 0)
}

/// Roll one RANGED shot of `attacker` vs `target` from a `[dmin, dmax]` ranged weapon:
/// `(damage, hit_info)`. A reduced attack table vs melee — miss / crit / normal only (a shot can't be
/// dodged-in-v1, parried, or blocked). The miss + crit bands reuse the same `effective_miss_bp` /
/// `effective_crit_bp` as melee; a crit is `CRIT_MULT`×; armor mitigation shaves the landed hit (floored
/// at 1). Known limitation: weapon damage only — no ranged attack-power scaling and no
/// dodge/glancing band in v1.
fn roll_ranged_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    dmin: u32,
    dmax: u32,
) -> (u32, u8) {
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    if (ctx.random::<u32>() % 10_000) < effective_miss_bp(ctx, attacker, sd) {
        return (0, HIT_MISS);
    }
    let is_crit = (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, attacker);
    let base = dmin + ctx.random::<u32>() % (dmax.saturating_sub(dmin) + 1);
    let pre_mit = if is_crit { base * CRIT_MULT } else { base };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), attacker.level);
    let dmg = (pre_mit * (100 - mit_pct) / 100).max(1);
    (dmg, if is_crit { HIT_CRIT } else { HIT_NORMAL })
}

/// Roll a copper loot amount in `[min, max]` via the module RNG. 0 if the range is empty
/// (`max == 0`), so a creature with no money loot drops nothing and its corpse isn't made lootable.
/// `pub(crate)` so the E_PICKPOCKET handler reuses the SAME money-range roll the kill path uses (one
/// formula, not two).
pub(crate) fn roll_money(ctx: &ReducerContext, min: u32, max: u32) -> u32 {
    if max == 0 || max < min {
        return 0;
    }
    min + ctx.random::<u32>() % (max - min + 1)
}

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
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return false;
    };
    if target.is_player() || target.dead {
        return false; // creatures only, and never re-kill a corpse
    }
    // A PET death (owner_guid != 0) is a CLEAN DESPAWN: no XP/loot/corpse/decay. A pet has NO
    // game_creature_spawn row, so the decay pass can never reap a dead pet — without this it would linger as
    // a stale, lootable corpse until the owner re-summons/logs out/dies. Delete it + free its engagements;
    // the owner can re-summon. (A pet kill credits no XP — an enemy killing your Imp gains nothing.)
    if target.owner_guid != 0 {
        disengage(ctx, target_guid);
        entities.guid().delete(target_guid);
        // Notify-hook: a pet death is still a death — fired after the despawn is committed.
        crate::hooks::fire_on_death(
            ctx,
            &crate::hooks::DeathPayload {
                victim_guid: target_guid,
                killer_guid: killer.unwrap_or(0),
                victim_is_player: false,
            },
        );
        return true;
    }
    // Snapshot the victim's identity for the notify-hooks fired at the end of this fn — the corpse row
    // is mutated (and `target` moved into the update) before they fire.
    let victim_entry = target.entry;
    let victim_level = target.level;
    let victim_instance = target.instance_id; // for on_creature_death (work-item 228)
                                              // A player killer gains XP (and may ding) — awarded before the corpse's level is gone.
                                              // Quests: the same killing blow advances the killer's kill objectives for this creature entry
                                              // (no-op for a non-player killer or one with no matching quest). Done before the corpse is gone so
                                              // the entry is still readable; on_creature_killed reads its own quest tables, not the target.
                                              // `kill_recipients` is hoisted OUT of the `if let` block below (work-item 187) so the group-loot
                                              // stamping call further down can reuse the SAME recipient set the XP/quest-credit split used —
                                              // eligibility must never drift between the two.
    let mut kill_recipients: Vec<u64> = Vec::new();
    if let Some(killer_guid) = killer {
        // Elite/rare-elite/boss creatures pay 2× kill XP (vanilla) — the rank + creature_type live on
        // the template (one lookup, reused below for the soul-shard XP/critter gate).
        let template = ctx.db.game_creature_template().entry().find(target.entry);
        let rank = template.as_ref().map(|t| t.rank).unwrap_or(0);
        // Snapshot the killer's level BEFORE award_xp: a kill that dings the killer mutates and
        // persists their level in-place (xp::grant_xp's ding loop), so re-reading it after would pick
        // up the POST-ding level for the shard's grey-clamp check below — misclassifying a kill that
        // just yielded XP at the PRE-ding level as grey at the new, higher level.
        let killer_level = entities
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
            crate::quest::on_creature_killed(ctx, *recipient, target.entry);
        }
        kill_recipients = recipients;
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
    }
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
    // STALE-SNAPSHOT PURGE (267, found live): corpse guids can be REUSED — `debug_spawn_at_feet`
    // allocates max+1 per entry, and a harness SQL teardown deletes the entity WITHOUT running the
    // decay reaper — so a fresh kill on a reused guid inherited a long-dead group's
    // `game_corpse_loot_eligible` rows and split the solo looter's purse with departed members
    // (46c purse → 13c, caught by the quest scenario's money delta). Purge any prior snapshot for
    // this guid at the ONE spot every creature death routes through, BEFORE the fresh loot roll /
    // group snapshot below. No-op for a never-before-used guid (the common path).
    {
        use crate::loot::game_corpse_loot_eligible;
        let eligible = ctx.db.game_corpse_loot_eligible();
        let stale: Vec<u64> = eligible
            .by_corpse()
            .filter(&target_guid)
            .map(|e| e.id)
            .collect();
        for id in stale {
            eligible.id().delete(id);
        }
    }
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
        crate::loot::apply_group_loot_rules(ctx, target_guid, killer_guid, &kill_recipients);
    }
    if loot > 0 || dropped {
        target.dynamic_flags |= lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    }
    target.health = 0;
    target.dead = true;
    entities.guid().update(target);
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
    // Drop the corpse's pending patrol legs so a client subscribing during decay doesn't walk it.
    let move_events = ctx.db.game_creature_move_event();
    let stale_legs: Vec<u64> = move_events
        .iter()
        .filter(|m| m.mover_guid == target_guid)
        .map(|m| m.id)
        .collect();
    for id in stale_legs {
        move_events.id().delete(id);
    }
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
        let melee = ctx.db.game_melee_attack();
        let still_engaged = melee.attacker_guid().find(killer_guid).is_some()
            || melee.by_target().filter(&killer_guid).next().is_some();
        if !still_engaged {
            if let Some(mut k) = entities.guid().find(killer_guid) {
                k.combat_until_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
                entities.guid().update(k);
            }
        }
    }
    // Notify-hooks, fired LAST so handlers observe the fully-committed death: on_death for the victim
    // (every creature-death path funnels through this fn), and on_kill when a player got credit
    // (killer-centric, one per credited kill).
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
    true
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
/// drop the engagements; tear down any channel/cast bar; apply the 10% equipped-durability loss;
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
    victim.health = 0;
    victim.dead = true;
    victim.combat_until_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    entities.guid().update(victim);
    disengage(ctx, victim_guid);
    crate::spell::break_channel(ctx, victim_guid);
    crate::items::apply_death_durability_loss(ctx, victim_guid);
    crate::hooks::fire_on_death(
        ctx,
        &crate::hooks::DeathPayload {
            victim_guid,
            killer_guid,
            victim_is_player: true,
        },
    );
    true
}

// --- Engagement queries over `game_melee_attack` (the single source of truth for who fights whom).
// `attacker_guid` is the PK; an engagement "touches" a unit when it is on EITHER side. These three
// helpers are the ONE place the both-directions rule is encoded, so leash evade, killing blow,
// logout teardown, the patrol-hold gate, and regen all share it instead of re-deriving it inline.

/// How long (ms) a unit stays flagged IN COMBAT after its last hostile action — vanilla's ~5-6s
/// combat-drop. `enter_combat` stamps `now + this`; the tick's combat-drop pass clears the flag past it.
pub(crate) const COMBAT_DROP_MS: u64 = 6000;

/// Put `guid` into combat: stamp `combat_until_ms = now + COMBAT_DROP_MS` and set `UNIT_FLAG_IN_COMBAT`
/// on its `unit_flags`, so OBSERVERS see the combat state — including a pure caster, whom the
/// auto-attack / `game_melee_attack` stance (`SMSG_ATTACKSTART`) can't cover. Idempotent; skips
/// dead/missing. Re-stamps only when newly entering OR the deadline is within half the window, so a
/// sustained fight doesn't write + fire a relay callback every 500ms tick. The flag is CLEARED by the
/// tick's combat-drop pass (`creatures::tick`), not here — so it lingers ~COMBAT_DROP_MS after the last
/// action, like vanilla (NOT instantly on `disengage`). [entity]
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
        return; // already comfortably in combat — skip the redundant write + relay callback
    }
    e.combat_until_ms = now_ms + COMBAT_DROP_MS;
    e.unit_flags |= lyracore_shared::constants::unit_flags::IN_COMBAT;
    entities.guid().update(e);
}

/// Free every melee engagement touching `guid` — its own outgoing attack AND any attack on it.
/// The canonical "leave combat" teardown. Collect-then-delete (never mutate while iterating).
pub(crate) fn disengage(ctx: &ReducerContext, guid: u64) {
    let melee = ctx.db.game_melee_attack();
    // Capture the CREATURES attacking `guid` (their target_guid points at `guid`) BEFORE deleting their
    // rows, so we can also drop their stale selection below. Without this a creature whose melee row is
    // gone keeps facing/tracking the dead/ghost player (the row stops the swing, but target_guid lingers
    // and the client renders the mob still selecting the corpse). Generic over any attacker of any unit.
    // Perf catalog 1.15: `by_target` turns these three full scans into own-row probes.
    let attackers_of_guid: Vec<u64> = melee
        .by_target()
        .filter(&guid)
        .map(|a| a.attacker_guid)
        .collect();
    // `touching` = the outgoing row (keyed by PK) plus every incoming one. Deletes are by PK, so the
    // visit order is irrelevant; the SET is identical to the old scan's.
    for a in melee
        .attacker_guid()
        .find(guid)
        .map(|a| a.attacker_guid)
        .into_iter()
        .chain(attackers_of_guid.iter().copied())
    {
        melee.attacker_guid().delete(a);
    }
    // Threat is part of being in combat: leaving it clears `guid` from the threat tables — its OWN table
    // (a creature that evaded/fled/died forgets everyone) AND its entries on every other creature's table
    // (a player who died/logged out is forgotten by every mob). One symmetric clear, the canonical place.
    crate::threat::clear_for_unit(ctx, guid);
    clear_target(ctx, guid);
    // Drop the stale target on every creature that was attacking `guid`. `clear_target` skips players and
    // is a no-op when target_guid == 0, so this only touches creatures that actually pointed at the dying
    // unit — baseline-safe (a creature attacking someone else is untouched) and covers a whole pack.
    for a in &attackers_of_guid {
        clear_target(ctx, *a);
    }
    // 249: leaving combat is IMMEDIATE for anyone this disengage left with no engagement at all —
    // vanilla drops the player's combat the moment the mob evades, not ~COMBAT_DROP_MS later (the
    // decay window still covers DoT/spell-only combat via pass_combat_drop, unchanged).
    let mut freed = attackers_of_guid;
    freed.push(guid);
    let entities = ctx.db.game_world_entity();
    for g in freed {
        let engaged = melee.attacker_guid().find(g).is_some()
            || melee.by_target().filter(&g).next().is_some();
        if engaged {
            continue;
        }
        if let Some(mut e) = entities.guid().find(g) {
            if e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                e.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
                e.combat_until_ms = 0;
                entities.guid().update(e);
            }
        }
    }
}

/// Zero a CREATURE's `target_guid` when it leaves combat (evade / flee / death) so the client stops
/// showing a stale target on a disengaged mob. Skips PLAYERS: a player's `target_guid` is their
/// SELECTION (`CMSG_SET_SELECTION`), which persists out of combat. No-op when already 0 (common path).
pub(crate) fn clear_target(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if !e.is_player() && e.target_guid != 0 {
            e.target_guid = 0;
            entities.guid().update(e);
        }
    }
}

/// Drop only `guid`'s OWN outgoing attack (and clear its threat), LEAVING any attack *on* it intact.
/// Used when a creature FLEES at low HP: it stops swinging and forgets its foes, but whoever is hitting
/// it stays engaged so they can run it down (vanilla "the mob flees, you chase the kill"). Contrast
/// [`disengage`], which frees BOTH directions — using that here deleted the player's row too, dropping
/// them out of combat one swing short of the kill (the "combat just stops at low HP" bug).
pub(crate) fn break_own_attacks(ctx: &ReducerContext, guid: u64) {
    // `attacker_guid` is the PK → at most one outgoing row for `guid`.
    ctx.db.game_melee_attack().attacker_guid().delete(guid);
    crate::threat::clear_for_unit(ctx, guid);
    clear_target(ctx, guid);
}

/// Is `guid` in any melee engagement, attacking or attacked?
pub(crate) fn is_engaged(ctx: &ReducerContext, guid: u64) -> bool {
    let melee = ctx.db.game_melee_attack();
    melee.attacker_guid().find(guid).is_some() || melee.by_target().filter(&guid).next().is_some()
}

/// Every guid currently in combat, for bulk in-combat gates (e.g. regen).
///
/// Includes both sides of every live melee engagement AND any entity whose
/// `UNIT_FLAG_IN_COMBAT` bit is set (covers pure casters, warriors with Bloodrage,
/// and anything else `enter_combat()` flagged without a melee row).  The two sets
/// overlap in practice; callers use `.contains()` so duplicates are harmless.
/// The MELEE half of the combatant set — both sides of every live `game_melee_attack` row. The other
/// half is every entity carrying `UNIT_FLAG_IN_COMBAT` (pure casters, Bloodrage warriors — anything
/// `enter_combat()` flagged without a melee row); perf catalog 1.6 moved that half to the CALLER,
/// because the only caller (`pass_regen`) is already iterating `game_world_entity` and can harvest the
/// flag from its own fresh rows instead of paying a second full table scan for it. A future caller
/// that isn't already scanning must harvest the flag half itself the same way.
pub(crate) fn melee_combatant_guids(ctx: &ReducerContext) -> Vec<u64> {
    ctx.db
        .game_melee_attack()
        .iter()
        .flat_map(|a| [a.attacker_guid, a.target_guid])
        .collect()
}

// ===========================================================================================
//  Combat tables [entity]/[event]
// ===========================================================================================

/// An active melee auto-attack: `attacker_guid` swings `target_guid`. Keyed by attacker (one
/// target at a time). [entity]
#[table(
    accessor = game_melee_attack,
    public,
    // Perf catalog 1.15: every "who is attacking X" question (disengage, kill_creature's
    // still_engaged, is_engaged, combatant_guids, heal threat) used to full-scan this table.
    index(accessor = by_target, btree(columns = [target_guid]))
)]
pub struct MeleeAttack {
    #[primary_key]
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub last_swing_ms: u32, // 0 = never swung (the next tick swings immediately)
    /// 0 = melee auto-attack (the original behavior); 75 (Auto Shot) / 5019 (wand Shoot) = a RANGED
    /// auto-attack — the swing tick then uses the equipped ranged weapon (slot 17), its delay as the
    /// swing interval, a longer range, and a reduced attack table (no parry/block). One engagement per
    /// attacker, so a unit is either meleeing OR shooting. END-appended + `#[default(0)]` → adding it
    /// auto-migrates existing rows (no `-c` wipe). [entity]
    #[default(0)]
    pub ranged_spell_id: u32,
    /// Work-item 037 (dual wield): the OFF-HAND swing's own clock, independent of `last_swing_ms` — an
    /// off-hand weapon has its own `delay_ms`, so it swings on a different cadence than the main hand.
    /// 0 = never swung (the next eligible tick swings immediately), same sentinel as `last_swing_ms`.
    /// Only consulted for a MELEE engagement (`ranged_spell_id == 0`) with a live off-hand weapon
    /// (`equipped_offhand_weapon_damage`) — a ranged engagement or a bare/shielded off-hand never
    /// advances this field. END-appended + `#[default(0)]` → adding it auto-migrates existing rows (no
    /// `-c` wipe). [entity]
    #[default(0)]
    pub last_offhand_swing_ms: u32,
}

/// A logged melee swing to relay as `SMSG_ATTACKERSTATEUPDATE`. Broadcast (public, no RLS), like
/// `game_creature_move_event` — the gateway fans it to in-world clients. [event]
#[table(
    accessor = game_combat_event,
    public,
    // perf catalog 2.3: AOI-box scoping instead of a global `SELECT *`.
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct CombatEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub damage: u32,
    pub hit_info: u8, // 0 normal, 1 crit, 2 miss, 3 dodge, 4 parry, 5 glancing, 6 crushing, 7 block
    pub killing_blow: bool, // the swing that killed the target → gateway also sends SMSG_ATTACKSTOP
    pub created_at: Timestamp,
    /// The flat shield `block_value` this swing absorbed (HIT_BLOCK only; 0 otherwise). Drives the
    /// SMSG_ATTACKERSTATEUPDATE `blocked_amount` wire field so the client shows the "Block N" text.
    /// END-appended + `#[default(0)]` → adding it auto-migrates existing rows (no `-c` wipe). [event]
    #[default(0)]
    pub blocked_amount: u32,
    /// 0 for a melee swing; the ranged spell (75 Auto Shot / 5019 Shoot) for a ranged shot. The gateway
    /// sets the SMSG_ATTACKERSTATEUPDATE `spell_id` field from this so the shot is attributed to the
    /// ranged ability. END-appended + `#[default(0)]` → auto-migrates. [event]
    #[default(0)]
    pub ranged_spell_id: u32,
    /// The `display_id` of the ammo (arrow/bullet) this shot consumed — Auto Shot (75) only; 0 for melee,
    /// wand Shoot, and out-of-data shots. The gateway sets the SMSG_SPELL_GO AMMO flag + this display so
    /// the client renders the arrow projectile. END-appended + `#[default(0)]` → auto-migrates. [event]
    #[default(0)]
    pub ammo_display_id: u32,
    /// True when a queued on-next-swing spell (Heroic Strike/Cleave) FIRED on this landed swing (114):
    /// vanilla REPLACES the white hit — the whole swing is the spell (one yellow named line, carried by
    /// the SpellCastEvent the swing inserts). The gateway then SKIPS the SMSG_ATTACKERSTATEUPDATE for
    /// this row (killing_blow/ATTACKSTOP still honored); `damage` keeps the true total for QA readers.
    /// END-appended + `#[default(false)]` → auto-migrates. [event]
    #[default(false)]
    pub spell_swing: bool,
    /// Projectile travel time (ms) for a RANGED shot — dist / Spell.dbc speed (Auto Shot 40 yd/s,
    /// wand 20 yd/s). 0 = melee (instant). The shot's DAMAGE lands at fire + this: the module's
    /// `ranged_impact` schedule applies health/kill then, and the gateway delays the
    /// SMSG_SPELLNONMELEEDAMAGELOG by the same amount — so the number lands WITH the arrow, not at
    /// the muzzle (user bug: "damage lands earlier than the projectile"). The SMSG_SPELL_GO
    /// (arrow launch) still relays immediately. END-appended + `#[default(0)]` → auto-migrates. [event]
    #[default(0)]
    pub impact_delay_ms: u32,
    // --- AOI columns (perf catalog 2.3), END-appended + TYPED defaults (a bare `0` on a u64
    // encodes as 4 bytes and fails the publish). Stamped from the actor via `helpers::grid_of`;
    // (0,0,0,0) means "no live actor", which matches no box and is correctly never delivered.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

/// Drives the melee swing tick. [server]
#[table(accessor = game_melee_schedule, scheduled(tick_melee))]
pub struct MeleeSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// One in-flight RANGED projectile (097): scheduled at fire + travel time; `ranged_impact` then
/// applies the frozen post-mitigation damage (health/lethal/threat/rage/skill/combat-flag) so the
/// server-side hit lands when the client's arrow does. Damage is FROZEN at launch (rolled + folded
/// there — vanilla folds absorb at impact, but freezing keeps the delayed damage LOG equal to what
/// actually lands; the ≤1s divergence window is noise). Module-private (not gateway-subscribed).
#[table(accessor = game_ranged_impact_schedule, scheduled(ranged_impact))]
pub struct RangedImpactSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub attacker_guid: u64,
    pub target_guid: u64,
    /// Post-mitigation damage frozen at launch (0 = a fully-absorbed shot: nothing to apply).
    pub damage: u32,
}

// ===========================================================================================
//  Reducers
// ===========================================================================================

/// Start (or retarget) the caller's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`).
/// Authorized via `ctx.sender` like all player ops; delegates to the shared `apply_start_attack`
/// core (which the actor verb API exposes by explicit guid). The first tick swings immediately.
#[reducer]
pub fn start_attack(ctx: &ReducerContext, target_guid: u64) -> Result<(), String> {
    let attacker =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "attacker not in world".to_string())?;
    apply_start_attack(ctx, attacker.guid, target_guid)
}

/// Shared core: arm (or retarget) `attacker_guid`'s MELEE auto-attack on `target_guid` — the
/// explicit-guid body behind the `start_attack` reducer and `actor::attack`.
pub(crate) fn apply_start_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    let attacker = ctx
        .db
        .game_world_entity()
        .guid()
        .find(attacker_guid)
        .ok_or_else(|| "attacker not in world".to_string())?;
    // Crowd control: an ACTION-blocked attacker (stunned/polymorphed/feared) cannot ENTER combat —
    // arming an engagement is itself an action. This is the player-command twin of the per-swing gate in
    // `tick_melee` (without it a CC'd player could insert a `game_melee_attack` row whose swings are then
    // all blocked — a hollow "in combat but can't swing" state). Mirrors the cast chokepoint
    // (`resolve_cast_at`) and the creature aggro/cast action passes. Baseline-safe: `false` without a CC
    // aura → an un-CC'd attack command arms exactly as before. (The CC error message deliberately does NOT
    // match the gateway's desync classifier, so it just rejects the command rather than dropping the
    // session.)
    if crate::spell::is_action_blocked(ctx, attacker.guid) {
        return Err(format!(
            "attacker {} cannot act (stun/poly/fear)",
            attacker.guid
        ));
    }
    if target_guid == attacker.guid {
        return Err("cannot attack self".to_string());
    }
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| "no such target".to_string())?;
    if target.map_id != attacker.map_id || target.instance_id != attacker.instance_id {
        return Err("target on another map".to_string());
    }
    if target.dead {
        // Can't attack a corpse during decay. The gateway maps this exact error to
        // SMSG_ATTACKSWING_DEADTARGET so the client leaves combat stance (shared constant).
        return Err(lyracore_shared::ERR_ATTACK_TARGET_DEAD.to_string());
    }
    // Faction gate: reject a swing only at a FRIENDLY target. Hostile (red) AND neutral (yellow —
    // e.g. Elwynn wolves, which are huntable) stay attackable, matching vanilla; only friendly (green)
    // units are protected. The gateway maps this to SMSG_ATTACKSWING_CANT_ATTACK so the client leaves
    // stance. SKIPPED when faction data isn't loaded (table empty) so missing data never blocks combat.
    if ctx.db.game_faction_template().count() > 0
        && crate::faction::is_friendly(ctx, attacker.faction_template, target.faction_template)
    {
        return Err(lyracore_shared::ERR_ATTACK_FRIENDLY.to_string());
    }

    // Arm the engagement (the tick gates each swing on range + timer). Re-arming retargets.
    let melee = ctx.db.game_melee_attack();
    let row = MeleeAttack {
        attacker_guid: attacker.guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0, // melee auto-attack
        last_offhand_swing_ms: 0,
    };
    if melee.attacker_guid().find(attacker.guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    Ok(())
}

/// Start (or retarget) the caller's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot /
/// 5019 wand Shoot), via `CMSG_CAST_SPELL`. Authorized via `ctx.sender`; delegates to the shared
/// `apply_start_ranged_attack` (which `debug_ranged_attack_nearest` reuses).
#[reducer]
pub fn start_ranged_attack(
    ctx: &ReducerContext,
    target_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let attacker =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "attacker not in world".to_string())?;
    // Fall back to the caster's current selection (UNIT_FIELD_TARGET, set by CMSG_SET_SELECTION) when the
    // cast carried no explicit unit target — some clients send Auto Shot/Shoot relying on the selection
    // rather than packing the target into SpellCastTargets.
    let resolved = if target_guid != 0 {
        target_guid
    } else {
        attacker.target_guid
    };
    apply_start_ranged_attack(ctx, attacker.guid, resolved, spell_id)
}

/// Shared core: arm `attacker_guid`'s RANGED auto-attack on `target_guid` with `spell_id`. Same gates as
/// `start_attack` (CC / self / target exists / same map / not a corpse / not friendly) PLUS a ranged
/// weapon must be equipped (slot 17) — Auto Shot/Shoot are impossible bare-handed. Arms/retargets the one
/// engagement row with `ranged_spell_id = spell_id`; the swing tick then runs the ranged branch.
pub(crate) fn apply_start_ranged_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let attacker = ctx
        .db
        .game_world_entity()
        .guid()
        .find(attacker_guid)
        .ok_or_else(|| "attacker not in world".to_string())?;
    if crate::spell::is_action_blocked(ctx, attacker.guid) {
        return Err(format!(
            "attacker {} cannot act (stun/poly/fear)",
            attacker.guid
        ));
    }
    if target_guid == attacker.guid {
        return Err("cannot attack self".to_string());
    }
    if equipped_ranged_weapon(ctx, attacker.guid).is_none() {
        return Err("no ranged weapon equipped".to_string());
    }
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .ok_or_else(|| "no such target".to_string())?;
    if target.map_id != attacker.map_id || target.instance_id != attacker.instance_id {
        return Err("target on another map".to_string());
    }
    if target.dead {
        return Err(lyracore_shared::ERR_ATTACK_TARGET_DEAD.to_string());
    }
    if ctx.db.game_faction_template().count() > 0
        && crate::faction::is_friendly(ctx, attacker.faction_template, target.faction_template)
    {
        return Err(lyracore_shared::ERR_ATTACK_FRIENDLY.to_string());
    }
    // Activation CheckCast (097/vanilla): vmangos REJECTS the auto-repeat activation for any hard
    // cast failure — the gateway relays the reason as SMSG_CAST_RESULT and the client drops its
    // toggle, so the client's auto-repeat state never outlives a loop that could not start. Without
    // these gates the row armed silently, every shot was suppressed by the same checks in the swing
    // tick, and the client sat toggled-on over a dead loop. Error strings are load-bearing: the
    // gateway's `cast_failure_reason_for` substring-maps them to the 1.12 red-error reasons
    // ("out of range" → 0x59, "too close" → 0x76, "in front" → 0x7C, "line of sight" → 0x2A).
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
            (attacker.x, attacker.y, attacker.z),
            (target.x, target.y, target.z),
        ) {
            return Err("target not in line of sight".to_string());
        }
        // A launcher (bow/gun/crossbow) with an empty quiver rejects at activation — vanilla's
        // SPELL_FAILED_NO_AMMO red error — instead of arming, sending a clean START, and silently
        // cancelling ~500ms later when the first shot's find_ammo comes up empty (review find).
        // Wands consume nothing. The mid-loop run-out keeps the swing tick's teardown+cancel.
        if let Some((_, _, _, subclass)) = equipped_ranged_weapon(ctx, attacker.guid) {
            use crate::items::weapon_subclass as ws;
            if matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW)
                && find_ammo(ctx, attacker.guid).is_none()
            {
                return Err("no ammo for ranged weapon".to_string());
            }
        }
    }
    // Initial-shot wind-up (097): a ranged auto-attack must NOT fire instantly on activation — vanilla's
    // Auto Shot has a ~0.5s cast before the first shot (the user: "we shoot right away, no waiting for the
    // attack timer"). `last_swing_ms == 0` would make the swing tick fire THIS tick; instead seed it so the
    // first shot is `RANGED_INITIAL_SHOT_MS` out. The swing gate fires when `now - last_swing >= delay`, so
    // set `last_swing = now - (delay - initial)` → the first shot lands after `initial` ms, then subsequent
    // shots on the full weapon `delay`. A fast weapon (delay < initial) → seed `now` → waits its full delay.
    // Melee keeps `last_swing_ms: 0` (swings immediately on engage — correct for melee).
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let ranged_delay = equipped_ranged_weapon(ctx, attacker.guid)
        .map(|(_, _, d, _)| d)
        .unwrap_or(0);
    let first_swing_seed = now_ms
        .saturating_sub(ranged_delay.saturating_sub(RANGED_INITIAL_SHOT_MS))
        .max(1); // .max(1): never the 0 "swing now" sentinel
    let melee = ctx.db.game_melee_attack();
    let row = MeleeAttack {
        attacker_guid: attacker.guid,
        target_guid,
        last_swing_ms: first_swing_seed,
        ranged_spell_id: spell_id,
        last_offhand_swing_ms: 0,
    };
    if melee.attacker_guid().find(attacker.guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    Ok(())
}

/// Stop the caller's melee auto-attack (`CMSG_ATTACKSTOP`).
#[reducer]
pub fn stop_attack(ctx: &ReducerContext) -> Result<(), String> {
    let attacker =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "attacker not in world".to_string())?;
    stop_attack_for(ctx, attacker.guid);
    Ok(())
}

/// Shared core: disarm `attacker_guid`'s outgoing auto-attack row (melee or ranged) — the
/// explicit-guid body behind the `stop_attack` reducer and `actor::stop_attack`.
/// Unconditional; a no-op when nothing is armed.
pub(crate) fn stop_attack_for(ctx: &ReducerContext, attacker_guid: u64) {
    ctx.db
        .game_melee_attack()
        .attacker_guid()
        .delete(attacker_guid);
}

/// Swing tick (scheduled; scheduler-only). Two passes:
/// 1. **Aggro** — a creature attacked by a player retaliates: ensure a reciprocal creature→player
///    engagement exists (one target at a time). No creature AI beyond "fight whoever hit me".
/// 2. **Swings** — for each engagement in range whose swing timer elapsed, deal damage
///    (player→7, creature→2). A creature target dies at 0 HP (DESTROY + free attackers + arm
///    respawn); a **player** target also dies at 0 HP (health=0 + `dead`, then release→ghost).
///
/// A leash pass runs first: a creature whose target fled beyond `LEASH_RADIUS` evades.
///
/// Work-item 229 (per-instance ticks): DELIBERATELY LEFT GLOBAL — one schedule row, no `instance_id`
/// scoping. Verified same-instance-by-construction: every pass here outer-loops `game_melee_attack`,
/// and a melee row's attacker/target pair shares one instance at ARM time on every arming path —
/// `apply_start_attack`/`apply_start_ranged_attack` reject a cross-instance target explicitly, this
/// file's `aggro_pass` retaliation mirrors an existing (same-instance) row, and `tick_creatures`'s
/// aggro/assist/pet passes pair within one instance (190 slice 1). So this tick's cost is O(active
/// engagements) — it scales with combat, NOT with instance count, and scoping it per instance would
/// divide an already-small table while adding a per-row entity fetch. (Known pre-existing edge, not
/// widened here: 224's `teleport_player` can move one side of a live pair cross-instance AFTER
/// arming; the leash pass then evades the creature on raw coordinate distance.) The tick_ms
/// smoothing knob is `tick_creatures`'s per-instance row; melee swing timing is already 100ms
/// globally.
#[reducer]
pub fn tick_melee(ctx: &ReducerContext, _schedule: MeleeSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    // Three named passes, run in this order. Each pass re-derives its own table handles /
    // now-timestamp from `ctx` (SpacetimeDB handles are live views, so a re-fetch is behavior-identical).
    // There is NO cross-pass shared state: the leash `evaders`, the aggro `new_aggro`, and the swing
    // `attacks` Vecs are each created and consumed within one pass.
    leash_pass(ctx);
    aggro_pass(ctx);
    resolve_swing(ctx);
}

/// Pass 0 — leash. A creature whose target has fled beyond LEASH_RADIUS evades: it drops ALL its combat
/// (its own attack + anyone attacking it), heals to full, and resumes patrol next tick (its melee rows
/// are gone). Classic "run too far → mob resets". Creatures don't chase yet, so there's no walk-home;
/// the heal flows to clients via the on_update health-VALUES relay.
fn leash_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();

    let mut evaders: Vec<u64> = Vec::new();
    for atk in melee.iter() {
        let (Some(attacker), Some(target)) = (
            entities.guid().find(atk.attacker_guid),
            entities.guid().find(atk.target_guid),
        ) else {
            continue;
        };
        // Only a creature's OWN engagement leashes (attacker is the creature, target is the player).
        if !attacker.is_player() && !evaders.contains(&attacker.guid) {
            let (dx, dy, dz) = (
                target.x - attacker.x,
                target.y - attacker.y,
                target.z - attacker.z,
            );
            if dx * dx + dy * dy + dz * dz > LEASH_RADIUS_SQ {
                evaders.push(attacker.guid);
                continue;
            }
            // Home tether (work-item 046): pass_chase glues the creature to ~5 yd, so the
            // target-distance check above never opens during a kite — measure the creature
            // against its OWN spawn point instead. Spawn-less creatures (pets, purely dynamic
            // spawns) have no tether. 2D like the wander/return home math (z varies on slopes).
            if let Some(sp) = spawns.guid().find(attacker.guid) {
                let (hx, hy) = (sp.x - attacker.x, sp.y - attacker.y);
                if hx * hx + hy * hy > HOME_TETHER_RADIUS_SQ {
                    evaders.push(attacker.guid);
                }
            }
        }
    }
    for creature in evaders {
        // Drop every engagement touching the creature (its attack + attacks on it).
        disengage(ctx, creature);
        // Evade-heal to full (the on_update relay refills the bar for observers).
        if let Some(mut c) = entities.guid().find(creature) {
            if c.health != c.max_health {
                c.health = c.max_health;
                entities.guid().update(c);
            }
        }
    }
}

/// Pass 1 — aggro. A creature with an incoming player attack and no engagement of its own retaliates
/// against that attacker (one target at a time). Collect first, then insert (don't mutate while iterating).
fn aggro_pass(ctx: &ReducerContext) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();

    let mut new_aggro: Vec<(u64, u64)> = Vec::new(); // (creature, player)
    for atk in melee.iter() {
        let (Some(attacker), Some(target)) = (
            entities.guid().find(atk.attacker_guid),
            entities.guid().find(atk.target_guid),
        ) else {
            continue;
        };
        // Retaliate only when the attacker is actually in MELEE RANGE (the same leeway gate the
        // swing tick uses): arming melee auto-attack from 20 yd out must NOT aggro the creature —
        // vanilla mobs engage on proximity-aggro range or a LANDED hit, never on a player merely
        // entering attack stance at distance (user-observed bug). The armed row is harmless until
        // the player closes: their first in-range swing lands the same tick this pass fires.
        // A RANGED auto-repeat (Auto Shot / Shoot) retaliates at any distance — but only once a
        // shot has actually FIRED (the swing tick's `enter_combat` stamped the target IN_COMBAT).
        // Arming alone is NOT a hit (097 rev.2): the first shot is seeded ~500ms out and can be
        // rejected/suppressed, and retaliating at arm time let the wolf aggro-then-instantly-evade
        // on a pull whose first shot never fired ("enters combat, leaves combat immediately").
        let is_ranged = atk.ranged_spell_id != 0;
        let dx = attacker.x - target.x;
        let dy = attacker.y - target.y;
        let dz = attacker.z - target.z;
        // MELEE retaliation additionally requires a swing to have FIRED (`last_swing_ms` is 0 at
        // arm/retarget and only the tail-of-loop stamp sets it, after the range/facing/LoS gates
        // let a swing through). Without this a player standing IN range with their BACK to a
        // neutral wolf aggro'd it just by toggling attack — the facing gate ate every swing, so
        // the wolf was retaliating against an attack that never happened (user find; the melee
        // twin of the ranged IN_COMBAT gate above). Costs one 100ms tick vs the old same-tick arm.
        if attacker.is_player()
            && !target.is_player()
            && !target.dead // a corpse doesn't retaliate
            && ((is_ranged
                && target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0)
                || (atk.last_swing_ms != 0
                    && dx * dx + dy * dy + dz * dz <= MELEE_RANGE_LEEWAY_SQ))
            && melee.attacker_guid().find(target.guid).is_none()
            && !new_aggro.iter().any(|(c, _)| *c == target.guid)
        {
            new_aggro.push((target.guid, attacker.guid));
        }
    }
    for (creature, player) in new_aggro {
        melee.insert(MeleeAttack {
            attacker_guid: creature,
            target_guid: player,
            last_swing_ms: 0,   // swing back immediately
            ranged_spell_id: 0, // creatures retaliate in melee
            last_offhand_swing_ms: 0,
        });
    }
}

/// The OFF-HAND swing (dual wield): a second, independent attack rolled by
/// `resolve_swing` for a MELEE engagement whenever `attacker_guid` has a live off-hand weapon
/// (`equipped_offhand_weapon_damage` — a shield/holdable off-hand short-circuits at `None`, unchanged
/// swing count). Runs on its OWN clock — the off-hand weapon's own `delay_ms`, tracked in
/// `MeleeAttack::last_offhand_swing_ms` — so it fires at a different cadence than the main hand and is
/// NOT gated by the main-hand swing-timer `continue` in `resolve_swing`.
///
/// Deliberately self-contained rather than folded into the main-hand branch's shared locals: it
/// re-fetches its OWN fresh `target`/`attacker` rows (the main-hand branch below may go on to damage or
/// even kill the SAME target this tick) and persists its own `last_offhand_swing_ms` stamp directly —
/// it does not rely on the caller's tail-of-loop `melee.attacker_guid().update(atk)`, which the
/// main-hand branch's various `continue`s (corpse/CC/lethal) may skip. Mirrors the main-hand shape:
/// the SAME shared attack table (`roll_swing_with_range`) and armor/mitigation math, over the off-hand's
/// AP-scaled range REDUCED by `apply_offhand_penalty` (vanilla's 50% dual-wield penalty); the SAME
/// outgoing/incoming damage-modifier folds, absorb, lethal fork (player death / creature kill via the
/// shared `kill_creature`), rage, weapon-skill training, threat, and break-on-damage. Does not arm the
/// Overpower/Revenge react windows or the seal/next-swing procs (those are vanilla MAIN-HAND-only
/// triggers) and does not wear durability (no separate off-hand durability model yet — a
/// deliberate simplification, like the main-hand's own DURABILITY_WEAR_CHANCE_PCT tuning).
fn resolve_offhand_swing(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    attacker: &WorldEntity,
    now_ms: u32,
) {
    // No off-hand weapon (empty, shield/holdable, or broken) → no second stream. The common case for
    // every creature (no equipment) and every player without a second one-hander — a no-op that leaves
    // `last_offhand_swing_ms` untouched, so this is byte-identical to before for them.
    let Some((dmin, dmax, delay)) = equipped_offhand_weapon_damage(ctx, attacker_guid) else {
        return;
    };
    let melee = ctx.db.game_melee_attack();
    let Some(atk) = melee.attacker_guid().find(attacker_guid) else {
        return; // engagement row already gone (freed by something earlier this tick)
    };
    if atk.last_offhand_swing_ms != 0 && now_ms.wrapping_sub(atk.last_offhand_swing_ms) < delay {
        return; // off-hand still on its own cooldown
    }
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return;
    };
    if attacker.dead || target.dead {
        return; // mirrors resolve_swing's corpse guard
    }

    // Effective AP identical to the main-hand fold (swing_range_ctx's player branch), scaled by the
    // OFF-HAND weapon's own delay, then halved by the vanilla dual-wield penalty.
    let class = attacker.class();
    let ap = melee_attack_power_for(
        class,
        effective_strength(ctx, attacker),
        effective_agility(ctx, attacker),
        attacker.level,
    ) + aura_attack_power_bonus(ctx, attacker.guid);
    let (raw_min, raw_max) = weapon_swing_range_ap(ap, dmin, dmax, delay);
    let range = apply_offhand_penalty(raw_min, raw_max);

    let (rolled, hit_info, blocked) = roll_swing_with_range(ctx, attacker, &target, range);

    crate::spell::break_stealth(ctx, attacker_guid);

    let rolled = crate::spell::apply_damage_taken(
        rolled,
        crate::spell::stance_damage_done_pct(attacker.stance)
            + crate::spell::combat_field_bonus(ctx, attacker_guid, crate::spell::COMBAT_DMG_DONE),
    );
    let rolled = crate::spell::apply_damage_taken(
        rolled,
        crate::spell::damage_taken_bonus(ctx, target_guid),
    );
    let dmg = crate::spell::absorb_incoming(ctx, target_guid, rolled);
    // GM playtest godmode (work-item 223's `.god`): INCOMING melee damage no-ops entirely on a
    // godmode TARGET — outgoing (this same unit swinging AS attacker) is unaffected. Zeroing here lets
    // the rest of this fn (rage/skill-up/threat, all gated on `dmg > 0`) fall through unchanged.
    let dmg = if target.godmode { 0 } else { dmg };
    let lethal = dmg > 0 && target.health <= dmg;

    ctx.db.game_combat_event().insert(CombatEvent {
        id: 0,
        attacker_guid,
        target_guid,
        damage: dmg,
        hit_info,
        killing_blow: lethal,
        created_at: ctx.timestamp,
        blocked_amount: blocked,
        ranged_spell_id: 0, // the off-hand stream is always melee, never a ranged auto-attack
        ammo_display_id: 0,
        spell_swing: false,
        impact_delay_ms: 0,

        // perf catalog 2.3: AOI address of the actor, so this row reaches the observers
        // whose box contains them instead of every connected session.
        map_id: crate::helpers::grid_of(ctx, attacker_guid).0,
        instance_id: crate::helpers::grid_of(ctx, attacker_guid).1,
        grid_x: crate::helpers::grid_of(ctx, attacker_guid).2,
        grid_y: crate::helpers::grid_of(ctx, attacker_guid).3,
    });

    if dmg > 0 && is_rage_user(attacker) {
        if let Some(mut a) = entities.guid().find(attacker_guid) {
            a.power = (a.power + rage_from_damage(dmg, true)).min(a.max_power);
            entities.guid().update(a);
        }
    }
    if dmg > 0 && attacker.is_player() && !target.is_player() {
        crate::skill::gain_weapon_skill(ctx, attacker);
    }

    if lethal {
        if target.is_player() {
            kill_player(ctx, target_guid, attacker_guid);
            return;
        }
        kill_creature(
            ctx,
            target_guid,
            if attacker.is_player() {
                Some(attacker_guid)
            } else if attacker.owner_guid != 0 {
                Some(attacker.owner_guid)
            } else {
                None
            },
        );
        return;
    }

    if dmg > 0 {
        let accrues_threat = attacker.is_player() && !target.is_player();
        if target.is_player() {
            crate::skill::gain_defense_skill(ctx, &target);
        }
        target.health = target.health.saturating_sub(dmg);
        if is_rage_user(&target) {
            target.power = (target.power + rage_from_damage(dmg, false)).min(target.max_power);
        }
        entities.guid().update(target);
        crate::spell::break_auras_on_damage(ctx, target_guid, attacker_guid, false);
        if accrues_threat {
            crate::threat::add_threat(
                ctx,
                target_guid,
                attacker_guid,
                crate::threat::threat_from_damage(dmg),
            );
        }
    }

    // Persist the off-hand's own clock — re-fetch fresh (the engagement row may not have been touched
    // by anything above, but read-your-writes is cheap insurance and matches the file's established
    // "re-fetch a fresh mutable copy" pattern for a row written mid-function).
    if let Some(mut fresh) = melee.attacker_guid().find(attacker_guid) {
        fresh.last_offhand_swing_ms = now_ms;
        melee.attacker_guid().update(fresh);
    }
}

/// Pass 2 — swings. For each engagement (re-read, since aggro may have added rows) in range whose
/// swing timer elapsed, resolve the swing: roll the attack table, apply damage / absorb / rage /
/// threat / break-on-damage, and handle the lethal fork (player death vs creature kill).
fn resolve_swing(ctx: &ReducerContext) {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let events = ctx.db.game_combat_event();

    let attacks: Vec<MeleeAttack> = melee.iter().collect();
    for atk in attacks {
        // Drop the engagement if either party left the world (also covers rows freed earlier this
        // tick by another engagement's killing blow).
        let Some(attacker) = entities.guid().find(atk.attacker_guid) else {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue;
        };
        let Some(mut target) = entities.guid().find(atk.target_guid) else {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue;
        };

        // A corpse neither swings nor is a valid swing target. With a lingering corpse
        // (the entity persists until the decay pass), a stale snapshot row whose attacker OR target
        // was killed earlier in THIS tick survives the two existence guards above (the corpse row is
        // still present). Without this guard such a row would: let a dead unit keep swinging
        // (attacker.dead); re-enter the killing-blow branch on a 0-HP target (target.dead → double
        // XP, duplicate death event, reset decay timer); and reach the `update` at the loop tail on a
        // row already freed by this tick's killing-blow `disengage`, which PANICS on a missing PK and
        // rolls back the entire tick. `delete` is idempotent if the row was already freed.
        if attacker.dead || target.dead {
            melee.attacker_guid().delete(atk.attacker_guid);
            continue;
        }

        // Crowd control: an ACTION-blocked attacker (stunned/polymorphed/feared) cannot swing. Gated HERE — after the existence/corpse
        // guards (so a stale row is still reaped) but BEFORE the range/swing-timer checks below and the
        // `last_swing_ms` write at the loop tail — so a stun does NOT even reset the swing timer (the
        // attacker resumes its prior cadence the instant the stun ends, the vanilla feel). The
        // engagement ROW is LEFT INTACT (we don't `delete` it): the unit is still in combat, just
        // locked, so it retaliates the moment the stun lifts. `continue` only skips THIS row's swing —
        // the outer `for mut atk in attacks` reads an owned snapshot Vec, so skipping a row never
        // corrupts the iteration (no table cursor is held across the body). A ROOTED attacker is NOT
        // gated here — root blocks movement, not attacks, so a rooted-but-in-range unit keeps swinging.
        // Baseline-safe: `is_action_blocked` is `false` for any unit without a stun/poly/fear aura, so an
        // un-CC'd swing reaches the identical range/timer/roll path it did before — byte-for-byte. A
        // POLYMORPHED or FEARED attacker is action-blocked too, so it can't swing either (a feared unit
        // routs without retaliating).
        if crate::spell::is_action_blocked(ctx, atk.attacker_guid) {
            continue;
        }

        // A ROUTING creature (low-HP flee-eligible humanoid) is RUNNING, not fighting — skip its swing so it
        // doesn't trade blows while it routs. Like the CC gate above, the engagement ROW is LEFT INTACT:
        // fleeing is a SHARED combat state (the flee pass keeps both sides in combat), the runner just stops
        // attacking while it flees. Mirrors the flee pass's creature_will_flee gate (same who-flees rule).
        if !attacker.is_player()
            && crate::creatures::should_flee(attacker.health, attacker.max_health)
            && ctx
                .db
                .game_creature_template()
                .entry()
                .find(attacker.entry)
                .is_some_and(|t| crate::creatures::flee_eligible(t.creature_type))
        {
            continue;
        }

        // RANGED vs MELEE: a ranged auto-attack (Auto Shot 75 / wand Shoot 5019) reads the slot-17
        // weapon, fires on ITS delay over a longer range, and uses a reduced attack table; a melee
        // engagement (ranged_spell_id == 0) is byte-identical to before. The ranged weapon is fetched
        // ONCE (a swing needs its damage + delay); if it's gone (unequipped mid-fight) the engagement
        // ends gracefully rather than firing bare-handed.
        let ranged = if atk.ranged_spell_id != 0 {
            match equipped_ranged_weapon(ctx, atk.attacker_guid) {
                Some(w) => Some(w),
                None => {
                    melee.attacker_guid().delete(atk.attacker_guid);
                    continue;
                }
            }
        } else {
            None
        };

        // (097/vanilla) A RANGED engagement whose shot comes DUE against a hard blocker (out of range /
        // too close / no LoS / not facing) is TORN DOWN, not silently suppressed — the vanilla
        // auto-repeat rule is that a failed check on a DUE ranged shot interrupts
        // the loop, and the row's delete relays the server-initiated SMSG_CANCEL_AUTO_REPEAT that
        // drops the client's toggle. Gated on DUE so a transient blocker BETWEEN shots (the mob
        // circling behind you, an LoS flicker) never cancels — vanilla only checks when the shot is
        // ready. Melee rows keep the silent per-tick `continue` (a melee swing resumes by itself).
        let ranged_due =
            ranged.is_some_and(|(_, _, delay, _)| now_ms.wrapping_sub(atk.last_swing_ms) >= delay);

        // (097/vanilla) Realtime movement rule — vanilla applies this BEFORE any castability
        // check: a PLAYER who is actually TRANSLATING (MOVE_MASK_MOVING — turning in place does
        // not count) CANCELS a wand loop outright, and DEFERS an Auto Shot loop: the due shot
        // re-arms RANGED_INITIAL_SHOT_MS out, over and over while moving, so the first shot after
        // stopping lands ~0.5s later and never fires mid-run (user bug: "we can shoot while
        // running"). Placed before the hard-fail gates so a moving out-of-range player defers
        // rather than cancels (vanilla order). Creatures are exempt (server-driven movement).
        if let Some((_, _, delay, subclass)) = ranged {
            use crate::items::weapon_subclass as ws;
            if attacker.is_player() && attacker.movement_flags & MOVE_MASK_MOVING != 0 {
                if subclass == ws::WAND {
                    // Wand Shoot dies on movement (vmangos: Category 351 → InterruptSpell); the row
                    // delete relays the server-initiated SMSG_CANCEL_AUTO_REPEAT.
                    melee.attacker_guid().delete(atk.attacker_guid);
                } else if ranged_due {
                    if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
                        fresh.last_swing_ms = now_ms
                            .wrapping_add(RANGED_INITIAL_SHOT_MS)
                            .wrapping_sub(delay)
                            .max(1);
                        melee.attacker_guid().update(fresh);
                    }
                }
                continue;
            }
        }

        // Range gate (re-checked each swing; the vanilla client walks into range itself). Ranged reaches
        // farther than melee.
        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        let range_sq = if ranged.is_some() {
            RANGED_RANGE_SQ
        } else {
            // Melee LEEWAY: when BOTH the attacker and the target are moving (a chase), the swing reaches
            // +8/3 yd farther — so a player chasing a FLEEING mob at run-speed parity (stuck ~5yd behind)
            // can still land the hit. A standstill fight keeps the 5yd base. `last_move_ms` is the per-unit
            // move clock (player: world::move; creature: the flee/chase tick).
            let both_moving = now_ms.wrapping_sub(attacker.last_move_ms) < MELEE_LEEWAY_WINDOW_MS
                && now_ms.wrapping_sub(target.last_move_ms) < MELEE_LEEWAY_WINDOW_MS;
            if both_moving {
                // Direction-aware: only a PLAYER chasing a leg-quantized creature needs the
                // padded 9 yd; a creature attacking a continuously-streamed player uses the
                // exact classic 7.67 yd (see tables.rs).
                if attacker.is_player() {
                    MELEE_RANGE_LEEWAY_SQ
                } else {
                    MELEE_RANGE_LEEWAY_CREATURE_SQ
                }
            } else {
                MELEE_RANGE_SQ
            }
        };
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq > range_sq {
            if ranged_due {
                melee.attacker_guid().delete(atk.attacker_guid);
            }
            continue;
        }
        // Auto Shot / wand Shoot have a MINIMUM range (~5 yd): a target in melee range is "too
        // close" — a DUE shot against it cancels the loop (vanilla SPELL_FAILED_TOO_CLOSE →
        // InterruptSpell; the relayed SMSG_CANCEL_AUTO_REPEAT drops the client toggle, so the
        // player's next melee press is a clean single-press swap). Melee has no minimum. [097]
        if ranged.is_some() && dist_sq < MELEE_RANGE_SQ {
            if ranged_due {
                melee.attacker_guid().delete(atk.attacker_guid);
            }
            continue;
        }
        // 243: no swinging THROUGH geometry — a mob parked at the outside of a thin wall sits
        // within the 5 yd 3D reach of a player just inside it (live find: the Rogue Wizard
        // beat a wall-separated player to death by melee). One LoS ray per due swing, both
        // directions symmetric (players can't hit through walls either). `has_los` is `true`
        // whenever nav is off — byte-identical pre-243 combat.
        if !crate::nav::has_los(
            ctx,
            attacker.map_id,
            (attacker.x, attacker.y, attacker.z),
            (target.x, target.y, target.z),
        ) {
            if ranged_due {
                melee.attacker_guid().delete(atk.attacker_guid);
            }
            continue;
        }
        // Facing gate (PLAYERS only): vanilla refuses a swing at a target behind you — without
        // this a fleeing player auto-attacked the chaser at their back. Creatures are exempt
        // (they auto-face their victim; their `orientation` trails their movement direction,
        // so gating them on it would break legitimate mob melee).
        if attacker.is_player()
            && !crate::spell::is_facing(
                attacker.x,
                attacker.y,
                attacker.orientation,
                target.x,
                target.y,
            )
        {
            if ranged_due {
                melee.attacker_guid().delete(atk.attacker_guid);
            }
            continue;
        }

        // Off-hand swing (dual wield): a MELEE engagement (never ranged — Auto Shot/wand
        // have no off-hand analog) whose attacker has a live off-hand WEAPON rolls a SECOND, independent
        // swing on the off-hand's OWN clock (`last_offhand_swing_ms`), gated on the SAME in-range check
        // just above but NOT on the main-hand's swing timer below — so the off-hand still fires on a tick
        // where the main hand is mid-cooldown. Self-contained: re-fetches its own fresh `target` (the
        // main-hand branch below may go on to kill/damage the SAME target this tick) and writes its own
        // event + damage + rage/threat/skill side effects, mirroring the main-hand shape. No-op (and
        // `atk.last_offhand_swing_ms` stays untouched) for a ranged engagement, an unarmed/shielded
        // off-hand, or a timer not yet elapsed — so a main-hand-only attacker's tick is unchanged
        // (baseline-safe).
        if ranged.is_none() {
            resolve_offhand_swing(ctx, atk.attacker_guid, atk.target_guid, &attacker, now_ms);
            // The off-hand swing may have killed/disengaged `target` (or, on a lethal PLAYER off-hand
            // hit, zeroed its health) via its own writes — re-sync the in-hand snapshot the main-hand
            // path below reads/writes so it never clobbers the off-hand's kill with a stale pre-swing
            // copy. A miss/no-off-hand-weapon tick leaves `target` byte-identical (re-fetch is a no-op).
            match entities.guid().find(atk.target_guid) {
                Some(refreshed) => target = refreshed,
                None => {
                    // The off-hand's killing blow fully removed the target (a PET death delete, or a
                    // creature corpse this loop's earlier existence guard would otherwise have caught).
                    melee.attacker_guid().delete(atk.attacker_guid);
                    continue;
                }
            }
            if target.dead {
                // The off-hand swing was the killing blow — the main-hand's own swing this tick is
                // moot (there's nothing left to hit), so stop here exactly like the top-of-loop corpse
                // guard would on the NEXT tick, just one tick earlier for this row.
                continue;
            }
        }

        // Swing timer — melee uses the ATTACKER's own `base_attack_time_ms` shortened by any melee-haste
        // aura (`effective_swing_time`); ranged uses the equipped ranged weapon's `delay_ms` (bows are
        // slow, wands faster). `wrapping_sub` guards the u32 millisecond clock wrapping; the 100ms tick
        // gives 0.1s resolution. Known limitation: no ranged-haste fold in v1.
        let swing_interval = match ranged {
            Some((_, _, delay, _)) => delay,
            None => effective_swing_time(ctx, &attacker),
        };
        if atk.last_swing_ms != 0 && now_ms.wrapping_sub(atk.last_swing_ms) < swing_interval {
            continue;
        }

        // Ammo: passing the range + timer gates means a shot FIRES this tick. Bow/gun/crossbow
        // (weapon subclass 2/3/18) consumes 1 arrow/bullet from the bag and stamps its display id onto
        // the event (so the gateway renders the arrow projectile); out of ammo ends the engagement, which
        // stops the client's auto-shoot too. Wand (subclass 19) + melee consume nothing (display 0).
        // Consumed on the shot regardless of hit/miss, like vanilla.
        use crate::items::weapon_subclass as ws;
        let ammo_display_id: u32 = if let Some((_, _, _, subclass)) = ranged {
            if matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW) {
                match find_ammo(ctx, atk.attacker_guid) {
                    Some(mut a) => {
                        let disp = ctx
                            .db
                            .game_item_template()
                            .entry()
                            .find(a.entry)
                            .map(|t| t.display_id)
                            .unwrap_or(0);
                        a.stack_count -= 1;
                        if a.stack_count == 0 {
                            ctx.db.game_item_instance().guid().delete(a.guid);
                        } else {
                            ctx.db.game_item_instance().guid().update(a);
                        }
                        disp
                    }
                    None => {
                        // Out of ammo → the loop stops; the row delete relays the server-initiated
                        // SMSG_CANCEL_AUTO_REPEAT that drops the client's toggle (097).
                        melee.attacker_guid().delete(atk.attacker_guid);
                        continue;
                    }
                }
            } else {
                0 // wand: flat damage, no ammo
            }
        } else {
            0 // melee: no ammo
        };

        // Roll the swing. Melee: the full vanilla attack table (miss/dodge/parry/glancing/block/crit/
        // crushing). Ranged: a reduced table (miss/crit/normal — a shot can't be parried or blocked) off
        // the ranged weapon's damage range.
        let (mut rolled, hit_info, blocked) = match ranged {
            Some((dmin, dmax, delay, subclass)) => {
                // Ranged AP fold: bow/gun/crossbow (subclass 2/3/18) scales with the
                // shooter's Agility-derived ranged attack power, folded into the weapon range by speed
                // exactly like melee; wand (subclass 19) is FLAT weapon damage (no ranged AP in vanilla).
                // Known limitation: no ranged-AP aura (e.g. Aspect of the Hawk) — we have none.
                let (rdmin, rdmax) = if matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW)
                    && attacker.is_player()
                {
                    let rap =
                        ranged_attack_power(effective_agility(ctx, &attacker), attacker.level);
                    weapon_swing_range_ap(rap, dmin, dmax, delay)
                } else {
                    (dmin, dmax)
                };
                let (d, hi) = roll_ranged_swing(ctx, &attacker, &target, rdmin, rdmax);
                (d, hi, 0)
            }
            None => roll_swing(ctx, &attacker, &target),
        };

        // React windows: a MELEE swing's avoidance outcome ARMS a reactive ability for ~5s. A
        // DODGE of this swing arms the ATTACKER's Overpower; any avoid (dodge/parry/block) of this incoming
        // swing arms the DEFENDER's Revenge. Keyed on the HIT_* outcome, never a spell id — the cast gate
        // (SPELL_ATTR_REQ_OVERPOWER / REQ_REVENGE) reads the stamped `*_until_ms`. Ranged swings can't be
        // dodged/parried/blocked in this engine (`roll_ranged_swing` has no avoidance bands), so this is
        // melee-only — `ranged.is_none()` already holds for any non-`HIT_NORMAL`/`CRIT` outcome, but the
        // guard makes the melee-only intent explicit. Stamped BEFORE the lethal/non-lethal forks so the
        // window always arms (the dodge already cost the attacker its swing).
        if ranged.is_none() && hit_info == HIT_DODGE {
            // A dodge arms the DODGED ATTACKER's Overpower. Safe to stamp here: the attacker row is never
            // written back from a stale snapshot below (the rage block re-fetches a fresh row).
            arm_react(ctx, atk.attacker_guid, ReactKind::Overpower);
        }
        // The defender's Revenge window is armed AFTER the damage application below — stamping it here would
        // be clobbered by the non-lethal `update(target)` writing the pre-stamp `target` snapshot.

        // Seal proc (Seal of Righteousness): a LANDED melee swing adds the attacker's seal holy damage —
        // the swing READS the A_SEAL aura (pull model, no per-spell code). Melee only (seals don't proc on
        // ranged); a 0-roll (miss/dodge/parry) lands nothing and doesn't proc.
        // 114: the portion (and the seal's spell id) is TRACKED so it can be reported as its own yellow
        // named line — it still folds into `rolled` (one health deduction, one lethal check).
        let (mut seal_portion, mut seal_spell) = (0u32, 0u32);
        if ranged.is_none() && rolled > 0 {
            let (p, s) = seal_holy_on_swing(ctx, &attacker);
            seal_portion = p;
            seal_spell = s;
            rolled += seal_portion;
        }

        // On-next-swing QUEUE (Heroic Strike / Cleave): a LANDED melee swing adds the attacker's
        // queued strike bonus (the pull-model twin of the seal proc) — reads `attacker.next_swing_spell` and
        // adds its E_NEXT_SWING base_points. Melee-only; a 0-roll lands nothing. The queue is CLEARED below
        // whenever a melee swing FIRES (even a miss), so a missed swing still spends it (vanilla). The bonus
        // flows through absorb / lethal like the rest of `rolled`. Vanilla REPLACES the white hit — the
        // whole landed swing IS the spell (one yellow named line; live-confirmed 2026-07-11), so
        // `queued_fired` marks the white event `spell_swing` (the gateway skips its ATTACKERSTATEUPDATE)
        // and the spell row below carries the full post-mitigation damage.
        let queued_spell = if ranged.is_none() {
            attacker.next_swing_spell
        } else {
            0
        };
        // 114: the cost was VALIDATED at press but is CHARGED here, when the spell actually fires
        // (vanilla defers the rage deduction to the swing; a MISSED swing still charges — the spell
        // cast and missed). FIZZLE: rage fell below cost between press and swing (out-of-combat
        // drain) → the strike doesn't fire, the swing stays a plain white swing, and the queue is
        // cleared below; an is_interrupted row tells the caster's client to release the lit button.
        let mut swing_is_spell = false;
        if queued_spell != 0 && attacker.is_player() {
            let cost = ctx
                .db
                .game_spell()
                .spell_id()
                .find(queued_spell)
                .map(|h| h.cost)
                .unwrap_or(0);
            // Fresh row (read-your-writes): the seal/roll path above never wrote power.
            if let Some(mut a) = entities.guid().find(atk.attacker_guid) {
                if a.power >= cost {
                    a.power -= cost;
                    entities.guid().update(a);
                    swing_is_spell = true;
                } else {
                    ctx.db.game_spell_cast_event().insert(SpellCastEvent {
                        id: 0,
                        caster_guid: atk.attacker_guid,
                        spell_id: queued_spell,
                        created_at: ctx.timestamp,
                        target_guid: 0,
                        cast_time_ms: 0,
                        is_completion: false,
                        damage: 0,
                        healed: 0,
                        school: 0,
                        is_crit: false,
                        resisted: 0,
                        absorbed: 0,
                        is_interrupted: true, // SMSG_SPELL_FAILURE → the caster's button releases
                        cooldown_ms: 0,
                        delay_ms: 0,
                        is_proc_log: false,
                        swing_hit_info: 0,
                        client_initiated: false,

                        // perf catalog 2.3: AOI address of the actor, so this row is delivered to the
                        // observers whose box contains them instead of to every connected session.
                        map_id: crate::helpers::grid_of(ctx, atk.attacker_guid).0,
                        instance_id: crate::helpers::grid_of(ctx, atk.attacker_guid).1,
                        grid_x: crate::helpers::grid_of(ctx, atk.attacker_guid).2,
                        grid_y: crate::helpers::grid_of(ctx, atk.attacker_guid).3,
                    });
                }
            }
        } else if queued_spell != 0 {
            swing_is_spell = true; // non-player queued strikes (none today) fire without a pool
        }
        let queued_fired = swing_is_spell && rolled > 0;
        if queued_fired {
            rolled += queued_strike_on_swing(ctx, &attacker);
        }
        // Spend the queue on ANY melee swing that fires (landed or missed) — re-fetches the row to clear it.
        // Read-your-writes safe: the bonus above already read `attacker.next_swing_spell` from the snapshot.
        if ranged.is_none() {
            clear_queued_strike(ctx, atk.attacker_guid);
        }

        // Stealth breaks on action: a swing that fires this tick (melee or ranged, hit or miss) drops the
        // attacker's A_STEALTH presence — a stealthed unit that attacks is revealed. The other action
        // chokepoint is the cast path (resolve_cast_at). No-op for the common un-stealthed swing.
        crate::spell::break_stealth(ctx, atk.attacker_guid);

        // Durability: a player's main-hand weapon wears 1 point per swing (after the roll). Creatures
        // don't wear gear. Known limitation: no ranged-weapon durability / ammo consumption in v1.
        if attacker.is_player() && ranged.is_none() {
            wear_weapon(ctx, atk.attacker_guid);
        }

        // Outgoing-damage modifier (attacker stance + A_MOD_COMBAT(COMBAT_DMG_DONE), e.g. Curse of Weakness):
        // Defensive Stance deals −10% (`stance_damage_done_pct`, keyed on the ATTACKER's `stance` field — the
        // in-hand `attacker` snapshot), summed with any active `COMBAT_DMG_DONE` aura on the attacker
        // (`combat_field_bonus` is the same typed chokepoint every other combat-stat aura reads). Folded
        // FIRST (before the defender's damage-taken fold) so it mirrors
        // the spell path's outgoing reduction. 0 for a non-Defensive / non-warrior, non-cursed attacker →
        // `rolled` unchanged (baseline-safe). Reuses `apply_damage_taken` (same signed-percent-floored-at-0
        // math, here a damage-DEALT percent).
        let rolled = crate::spell::apply_damage_taken(
            rolled,
            crate::spell::stance_damage_done_pct(attacker.stance)
                + crate::spell::combat_field_bonus(
                    ctx,
                    atk.attacker_guid,
                    crate::spell::COMBAT_DMG_DONE,
                ),
        );
        // Incoming-damage modifier (A_MOD_DAMAGE_TAKEN — a defensive cooldown like Shield Wall, or a
        // vulnerability debuff) folds in NEXT: a signed percent on the post-armor swing, before absorb.
        // No such aura → `rolled` unchanged (baseline-safe). Shares the exact helper the spell path uses.
        let rolled = crate::spell::apply_damage_taken(
            rolled,
            crate::spell::damage_taken_bonus(ctx, atk.target_guid),
        );
        // Absorb shields (A_ABSORB, e.g. Power Word: Shield) soak the POST-MITIGATION melee damage
        // BEFORE the health deduction and the lethal check, so a fully-absorbed swing can't kill and
        // drains the shield instead of the target's health. With no shield on the target this returns
        // the (modified) damage unchanged (the deduction below is byte-identical to before). A 0-damage
        // swing (miss/dodge/parry) draws nothing (absorb_incoming short-circuits on 0). [entity]
        let dmg = crate::spell::absorb_incoming(ctx, atk.target_guid, rolled);
        // GM playtest godmode (work-item 223's `.god`): INCOMING melee damage no-ops entirely on a
        // godmode TARGET — outgoing (this same unit swinging AS attacker) is unaffected.
        let dmg = if target.godmode { 0 } else { dmg };

        // A real hit (un-absorbed dmg > 0) that brings the target to 0 HP is lethal (a miss never
        // kills; a fully-soaked hit is now dmg == 0, so it can't be lethal either).
        let lethal = dmg > 0 && target.health <= dmg;

        // (097) RANGED shot = a projectile IN THE AIR: freeze the post-mitigation damage here and
        // land it at fire + travel (dist / the real Spell.dbc speed — Auto Shot 40 yd/s, wand 20).
        // The launch event relays the SMSG_SPELL_GO (arrow) NOW and carries `impact_delay_ms` so
        // the gateway holds the damage LOG to the same moment; the scheduled `ranged_impact`
        // applies health/kill/threat/rage/skill/combat then — so the number, the health drop, and
        // the arrow all land together (user bug: "damage lands earlier than the projectile").
        // killing_blow is decided AT IMPACT (health can move mid-flight), so the launch event
        // never claims it; a miss/fully-absorbed shot schedules nothing (the GO's miss list /
        // absence of a log is the whole story). Everything below this fork stays melee-only.
        if let Some((_, _, _, subclass)) = ranged {
            let speed = if subclass == ws::WAND {
                WAND_PROJECTILE_SPEED
            } else {
                AUTO_SHOT_PROJECTILE_SPEED
            };
            let travel_ms = crate::spell::projectile_travel_ms(dist_sq.sqrt(), speed);
            events.insert(CombatEvent {
                id: 0,
                attacker_guid: atk.attacker_guid,
                target_guid: atk.target_guid,
                damage: dmg,
                hit_info,
                killing_blow: false,
                created_at: ctx.timestamp,
                blocked_amount: 0,
                ranged_spell_id: atk.ranged_spell_id,
                ammo_display_id,
                spell_swing: false,
                impact_delay_ms: travel_ms,

                // perf catalog 2.3: AOI address of the actor, so this row is delivered to the
                // observers whose box contains them instead of to every connected session.
                map_id: crate::helpers::grid_of(ctx, atk.attacker_guid).0,
                instance_id: crate::helpers::grid_of(ctx, atk.attacker_guid).1,
                grid_x: crate::helpers::grid_of(ctx, atk.attacker_guid).2,
                grid_y: crate::helpers::grid_of(ctx, atk.attacker_guid).3,
            });
            if dmg > 0 {
                let land_at = ctx
                    .timestamp
                    .checked_add(TimeDuration::from_micros((travel_ms as i64) * 1000))
                    .unwrap_or(ctx.timestamp);
                ctx.db
                    .game_ranged_impact_schedule()
                    .insert(RangedImpactSchedule {
                        scheduled_id: 0,
                        scheduled_at: ScheduleAt::Time(land_at),
                        attacker_guid: atk.attacker_guid,
                        target_guid: atk.target_guid,
                        damage: dmg,
                    });
            }
            // Same fresh-row stamp discipline as the loop tail (which this fork skips).
            if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
                fresh.last_swing_ms = now_ms;
                melee.attacker_guid().update(fresh);
            }
            continue;
        }

        // One swing event per swing; `lethal` (killing_blow) also tells the relay to send
        // SMSG_ATTACKSTOP so the attacker leaves combat stance. Logged before any teardown so the
        // event doesn't depend on whether the target row survives this tick.
        // 114: the seal's holy portion gets its own named yellow line (the proc insert below), so the
        // WHITE event reports the swing minus it. A fired queued strike goes further — the WHOLE swing
        // is the spell (spell_swing → the gateway skips this event's ATTACKERSTATEUPDATE; the spell row
        // below carries the full damage). `dmg` (the total) still drives health/lethal/rage; the seal
        // subtraction is display-only (a pre-modifier flat, so a modified/absorbed swing's split is
        // approximate — totals stay exact).
        events.insert(CombatEvent {
            id: 0,
            attacker_guid: atk.attacker_guid,
            target_guid: atk.target_guid,
            damage: dmg.saturating_sub(seal_portion),
            hit_info,
            killing_blow: lethal,
            created_at: ctx.timestamp,
            blocked_amount: blocked,
            ranged_spell_id: atk.ranged_spell_id,
            ammo_display_id,
            spell_swing: swing_is_spell,
            impact_delay_ms: 0, // melee is instant (the ranged fork above returned already)

            // perf catalog 2.3: AOI address of the actor, so this row is delivered to the
            // observers whose box contains them instead of to every connected session.
            map_id: crate::helpers::grid_of(ctx, atk.attacker_guid).0,
            instance_id: crate::helpers::grid_of(ctx, atk.attacker_guid).1,
            grid_x: crate::helpers::grid_of(ctx, atk.attacker_guid).2,
            grid_y: crate::helpers::grid_of(ctx, atk.attacker_guid).3,
        });
        // 114 FIX (a): the queued on-next-swing strike FIRES with this swing — emit its cast event NOW
        // (never at queue time). is_completion=true rides the timed-completion relay shape, which is
        // exactly the vanilla on-next-swing fire: CAST_RESULT(OK) to the caster (releases the client's
        // pending cast = un-lights the button) + SMSG_SPELL_GO alone (no second START) + the yellow
        // "Heroic Strike hits ..." damage log carrying the WHOLE landed swing (weapon roll + bonus,
        // post-mitigation; the seal's portion stays on its own line). A MISSED swing still emits the
        // row (damage 0, no log) — the GO must fire or the client's button stays lit forever.
        if swing_is_spell {
            ctx.db.game_spell_cast_event().insert(SpellCastEvent {
                id: 0,
                caster_guid: atk.attacker_guid,
                spell_id: queued_spell,
                created_at: ctx.timestamp,
                target_guid: atk.target_guid,
                cast_time_ms: 0,
                is_completion: true,
                damage: if queued_fired {
                    dmg.saturating_sub(seal_portion)
                } else {
                    0
                },
                healed: 0,
                school: 0, // physical — HS/Cleave are weapon strikes
                is_crit: hit_info == HIT_CRIT,
                resisted: 0,
                absorbed: 0,
                is_interrupted: false,
                cooldown_ms: 0,
                delay_ms: 0,
                is_proc_log: false,
                // The swing outcome — on a 0-damage fire the relay shapes the GO miss list from it
                // (yellow "Heroic Strike missed/was dodged/was parried", not a white MISS line).
                swing_hit_info: hit_info,
                client_initiated: false,

                // perf catalog 2.3: AOI address of the actor, so this row is delivered to the
                // observers whose box contains them instead of to every connected session.
                map_id: crate::helpers::grid_of(ctx, atk.attacker_guid).0,
                instance_id: crate::helpers::grid_of(ctx, atk.attacker_guid).1,
                grid_x: crate::helpers::grid_of(ctx, atk.attacker_guid).2,
                grid_y: crate::helpers::grid_of(ctx, atk.attacker_guid).3,
            });
        }
        // 114 FIX (b): the seal's holy portion — a log-only row (is_proc_log): the gateway sends ONLY
        // SMSG_SPELLNONMELEEDAMAGELOG named after the seal spell (yellow "Seal of Righteousness hits
        // ... Holy"), never START/GO (nothing casts; the seal aura is already up — vanilla shape).
        if seal_portion > 0 {
            ctx.db.game_spell_cast_event().insert(SpellCastEvent {
                id: 0,
                caster_guid: atk.attacker_guid,
                spell_id: seal_spell,
                created_at: ctx.timestamp,
                target_guid: atk.target_guid,
                cast_time_ms: 0,
                is_completion: false,
                damage: seal_portion,
                healed: 0,
                school: 1, // holy (school_mask 2 → index 1, the mask→index rule in resolve_cast_at)
                is_crit: false,
                resisted: 0,
                absorbed: 0,
                is_interrupted: false,
                cooldown_ms: 0,
                delay_ms: 0,
                is_proc_log: true,
                swing_hit_info: 0,
                client_initiated: false,

                // perf catalog 2.3: AOI address of the actor, so this row is delivered to the
                // observers whose box contains them instead of to every connected session.
                map_id: crate::helpers::grid_of(ctx, atk.attacker_guid).0,
                instance_id: crate::helpers::grid_of(ctx, atk.attacker_guid).1,
                grid_x: crate::helpers::grid_of(ctx, atk.attacker_guid).2,
                grid_y: crate::helpers::grid_of(ctx, atk.attacker_guid).3,
            });
        }

        // Rage (warriors): the attacker gains rage from dealing damage — applied to the attacker row
        // now so it lands before any death/non-lethal branch (you gain rage on the killing blow too).
        // The target's rage from TAKING damage is granted in the non-lethal branch below. Read-your-
        // writes within this tick makes both directions accumulate on a player who is in two
        // engagements (attacking + being attacked) the same tick.
        if dmg > 0 && is_rage_user(&attacker) {
            // Re-fetch a fresh mutable copy (read-your-writes): `attacker` is still needed read-only
            // below (is_player / award_xp), and WorldEntity isn't Clone.
            if let Some(mut a) = entities.guid().find(atk.attacker_guid) {
                a.power = (a.power + rage_from_damage(dmg, true)).min(a.max_power);
                entities.guid().update(a);
            }
        }

        // Weapon skill-up: a PLAYER landing a damaging hit on a CREATURE trains the equipped
        // weapon's skill line one step toward cap (level*5). Fires on the killing blow too. Touches only
        // game_player_skill rows (not the entity), so it's independent of the rage re-fetch above. A no-op
        // for an at-cap skill (the normal case) — the visible climb is a skill deliberately below cap.
        if dmg > 0 && attacker.is_player() && !target.is_player() {
            crate::skill::gain_weapon_skill(ctx, &attacker);
        }

        if lethal {
            // PLAYER death via the SHARED kill_player chokepoint (see its doc for the full
            // contract — on-wire signal, logout gate, durability, on_death).
            if target.is_player() {
                kill_player(ctx, atk.target_guid, atk.attacker_guid);
                continue;
            }

            // Creature death: XP + corpse + loot + decay, via the shared helper so the
            // melee kill, debug kill, and lethal spell damage stay identical. A player attacker credits
            // the XP; `continue` because the engagement row was freed by `disengage` inside the helper.
            kill_creature(
                ctx,
                atk.target_guid,
                // A player credits its own kill; a PET (owner_guid != 0) credits its OWNER, so a Warlock
                // levels + loots off its Imp's killing blows. A wild creature's kill credits nobody.
                if attacker.is_player() {
                    Some(atk.attacker_guid)
                } else if attacker.owner_guid != 0 {
                    Some(attacker.owner_guid)
                } else {
                    None
                },
            );
            continue;
        }

        // Non-lethal hit: apply damage. A non-lethal hit means health > dmg (the lethal branch above
        // handles reaching 0), so health stays > 0; a miss (dmg 0) changes nothing. The drop flows to
        // observers via the on_update VALUES relay.
        if dmg > 0 {
            // Capture the threat predicate BEFORE the `update` moves `target`: a player's landed damage
            // to a creature accrues threat (creatures keep no threat table — only player→creature counts).
            let accrues_threat = attacker.is_player() && !target.is_player();
            // Defense skill-up: a PLAYER that takes a damaging hit and SURVIVES trains its
            // Defense skill one step toward cap. Read-only on `target` (touches only game_player_skill),
            // before the `update` moves it. A no-op at cap (the normal case).
            if target.is_player() {
                crate::skill::gain_defense_skill(ctx, &target);
            }
            target.health = target.health.saturating_sub(dmg);
            // Rage (warriors): the target gains rage from taking the hit (less than dealing).
            if is_rage_user(&target) {
                target.power = (target.power + rage_from_damage(dmg, false)).min(target.max_power);
            }
            entities.guid().update(target);
            // Break-on-damage: a real melee hit drops the target's break-on-damage CC (polymorph etc.) and
            // interrupts its in-progress timed cast (direct damage, so periodic = false). The attacker guid
            // rides along so an A_PROC_ON_HIT aura on the target (Frost Armor) can reactively chill it.
            crate::spell::break_auras_on_damage(ctx, atk.target_guid, atk.attacker_guid, false);
            // Threat accrues on the creature's table so the retarget pass (tick_creatures) targets the
            // highest-threat attacker. The lethal branch above already returned (a killing blow ends
            // combat; `disengage` cleared the table), so this only fires on a SURVIVING creature.
            if accrues_threat {
                crate::threat::add_threat(
                    ctx,
                    atk.target_guid,
                    atk.attacker_guid,
                    crate::threat::threat_from_damage(dmg),
                );
            }
        }

        // Defender's Revenge window: armed AFTER the damage `update(target)` above so a partial
        // block (dmg>0, non-lethal) cannot clobber the stamp with its pre-stamp `target` snapshot. arm_react
        // re-fetches the defender fresh, so it works for both the dmg==0 (dodge/parry) and dmg>0 (partial
        // block) paths. A lethal block `continue`d earlier — a dead defender needs no window.
        if ranged.is_none() && matches!(hit_info, HIT_DODGE | HIT_PARRY | HIT_BLOCK) {
            arm_react(ctx, atk.target_guid, ReactKind::Revenge);
        }

        // Re-fetch fresh before the tail write: resolve_offhand_swing (above, mid-loop) may have
        // persisted its own `last_offhand_swing_ms` stamp on this same row, and `atk` is still the
        // stale pre-tick snapshot collected at the top of this function. Writing `atk` wholesale here
        // would clobber that off-hand stamp back to its pre-tick value every ordinary swing tick.
        if let Some(mut fresh) = melee.attacker_guid().find(atk.attacker_guid) {
            fresh.last_swing_ms = now_ms;
            melee.attacker_guid().update(fresh);
        }

        // A swing FIRED this tick (hit or miss) → both combatants are IN COMBAT. Set the flag at the
        // loop tail so it lands AFTER every entity write above (the target's health `update` uses a
        // pre-swing snapshot that would otherwise clobber the flag); enter_combat re-fetches fresh.
        // The melee path leaned on SMSG_ATTACKSTART for the client's combat stance, but a RANGED
        // auto-repeat sends no ATTACKSTART (that would animate a melee swing) — so without
        // UNIT_FLAG_IN_COMBAT the hunter dropped to idle between shots and the client never auto-
        // swapped to melee at close range. COMBAT_DROP_MS (6s) > the bow swing, so the flag holds
        // through the gap between shots. [097]
        enter_combat(ctx, atk.attacker_guid);
        enter_combat(ctx, atk.target_guid);
    }
}

/// Scheduled RANGED-projectile impact (097; scheduler-only): the arrow/bullet/bolt lands — apply
/// the launch-frozen post-mitigation damage with the SAME side-effect set the melee swing applies
/// inline (health/lethal via the shared kill helpers, rage both ways, weapon/defense skill-ups,
/// break-on-damage, threat, IN_COMBAT stamps). Guards re-checked at landing: either side gone or
/// the target already dead → the shot fizzles silently (the client saw the arrow; vanilla eats
/// mid-flight kills the same way). Lethality is decided HERE from fresh health, never at launch.
#[reducer]
pub fn ranged_impact(ctx: &ReducerContext, shot: RangedImpactSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(attacker) = entities.guid().find(shot.attacker_guid) else {
        return;
    };
    let Some(mut target) = entities.guid().find(shot.target_guid) else {
        return;
    };
    if target.dead || shot.damage == 0 {
        return;
    }
    let dmg = shot.damage;
    let target_is_player = target.is_player();
    if target.health <= dmg {
        // Lethal landing — the shared kill chokepoints (XP/corpse/loot/disengage; the attacker's
        // own ranged row is freed by the kill's disengage → the relay cancels the client toggle).
        if target_is_player {
            kill_player(ctx, shot.target_guid, shot.attacker_guid);
        } else {
            kill_creature(
                ctx,
                shot.target_guid,
                if attacker.is_player() {
                    Some(shot.attacker_guid)
                } else if attacker.owner_guid != 0 {
                    Some(attacker.owner_guid)
                } else {
                    None
                },
            );
        }
    } else {
        let accrues_threat = attacker.is_player() && !target_is_player;
        if target_is_player {
            crate::skill::gain_defense_skill(ctx, &target);
        }
        target.health = target.health.saturating_sub(dmg);
        if is_rage_user(&target) {
            target.power = (target.power + rage_from_damage(dmg, false)).min(target.max_power);
        }
        entities.guid().update(target);
        crate::spell::break_auras_on_damage(ctx, shot.target_guid, shot.attacker_guid, false);
        if accrues_threat {
            crate::threat::add_threat(
                ctx,
                shot.target_guid,
                shot.attacker_guid,
                crate::threat::threat_from_damage(dmg),
            );
        }
    }
    // Attacker-side side effects land at impact too (rage for a rage-user with a gun, weapon skill).
    if is_rage_user(&attacker) {
        if let Some(mut a) = entities.guid().find(shot.attacker_guid) {
            a.power = (a.power + rage_from_damage(dmg, true)).min(a.max_power);
            entities.guid().update(a);
        }
    }
    if attacker.is_player() && !target_is_player {
        crate::skill::gain_weapon_skill(ctx, &attacker);
    }
    enter_combat(ctx, shot.attacker_guid);
    enter_combat(ctx, shot.target_guid);
}

#[cfg(test)]
mod seal_tests {
    use super::seal_swing_holy;

    #[test]
    fn seal_swing_holy_scales_with_coeff_and_speed() {
        assert_eq!(seal_swing_holy(0, 2000), 0); // no seal → nothing (unsealed swing unchanged)
        assert_eq!(seal_swing_holy(108, 2000), 2); // Seal of Righteousness r1 on a 2.0s weapon
        assert_eq!(seal_swing_holy(216, 2000), 4); // r10 → double the per-swing holy
        assert_eq!(seal_swing_holy(216, 3600), 7); // a slower weapon lands more per swing
    }
}

#[cfg(test)]
mod next_swing_tests {
    use super::queued_strike_bonus;

    // The pure bonus-add logic of the on-next-swing queue (Heroic Strike / Cleave). The ctx-aware
    // `queued_strike_on_swing` reads `next_swing_spell` + the effect base_points and delegates here; this
    // tests the bonus computation + the no-queue clear path in isolation.
    #[test]
    fn no_queue_adds_nothing() {
        // queued_spell_id == 0 → no queued strike → 0 bonus, regardless of any base_points value (an
        // un-queued swing is byte-identical to a plain white swing).
        assert_eq!(queued_strike_bonus(0, 0), 0);
        assert_eq!(queued_strike_bonus(0, 137), 0); // a stale base_points is ignored when nothing is queued
    }

    #[test]
    fn queued_strike_adds_its_flat_base() {
        // A queued Heroic Strike (78) adds its flat E_NEXT_SWING base_points to the swing.
        assert_eq!(queued_strike_bonus(78, 11), 11); // Heroic Strike r1 flat bonus
        assert_eq!(queued_strike_bonus(845, 5), 5); // Cleave's queued bonus
    }

    #[test]
    fn non_positive_base_clamps_to_zero() {
        // A queued spell whose effect base is 0 / negative only ever ADDS (never subtracts) — clamp to 0,
        // so a mis-imported queued spell can't turn a swing into a heal.
        assert_eq!(queued_strike_bonus(78, 0), 0);
        assert_eq!(queued_strike_bonus(78, -50), 0);
    }
}

#[cfg(test)]
mod react_window_tests {
    use super::react_window_active;

    // The pure window-active check the Overpower / Revenge cast gate uses. A window is OPEN while its
    // stored deadline is strictly AFTER now; a 0 deadline (never armed) or a lapsed one is CLOSED.
    #[test]
    fn unarmed_window_is_closed() {
        // 0 = never armed → the gate refuses (Overpower can't be cast without a dodge).
        assert!(!react_window_active(0, 1_000));
        assert!(!react_window_active(0, 0));
    }

    #[test]
    fn armed_window_is_open_until_deadline() {
        // Armed at now=1000 for 5s → deadline 6000. Open while now < 6000, closed at/after it.
        let until = 6_000u64;
        assert!(react_window_active(until, 1_000)); // just armed → open
        assert!(react_window_active(until, 5_999)); // 1ms before expiry → still open
        assert!(!react_window_active(until, 6_000)); // exactly at the deadline → closed (strict >)
        assert!(!react_window_active(until, 6_001)); // past the deadline → closed
    }
}
