//! Effective-value folds over aura/gear + the weapon-damage/attack-table roll family (split of the
//! former monolithic `combat/mod.rs`, on top of the shared damage pipeline). Everything here is the
//! "what does this unit's swing look like right now" layer: the `effective_*` stat folds (dodge/parry/
//! block/crit/miss/swing-time/armor/strength/agility/move-speed), the seal-on-swing + queued-strike
//! (Heroic Strike/Cleave) on-next-swing folds, the react-window (Overpower/Revenge) arm/check, the
//! equipped-weapon/ammo lookups, and `roll_swing`/`roll_ranged_swing` — the attack-table rolls that
//! `death`'s pipeline and `swing`'s resolvers consume. No lethal/death handling and no reducers live
//! here; `mod.rs` re-exports this module (`pub use folds::*`) so every `crate::combat::<sym>` path
//! resolves regardless of which submodule actually defines it.

use spacetimedb::ReducerContext;

use crate::{
    game_aura, game_creature_template, game_item_instance, game_item_template, game_spell_effect,
    game_world_entity, ItemInstance, WorldEntity,
};

// Tables' pure formulas/consts (`dodge_chance_bp`, `armor_mitigation_pct`, `CRIT_MULT`, ...) and the
// sibling submodules' re-exports are all pulled in from `mod.rs` (`pub use tables::*` + `pub use
// folds::*`/`death::*`/`engage::*`/`swing::*`) so every symbol resolves the same as before the split.
use super::*;

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
pub(crate) fn aura_attack_power_bonus(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    crate::spell::combat_field_bonus(ctx, unit_guid, crate::spell::COMBAT_ATTACK_POWER).max(0)
        as u32
}

/// Holy damage a unit's active SEAL aura adds to a LANDED melee swing (Seal of Righteousness): the summed
/// `A_SEAL` aura value, weapon-speed-weighted — vanilla SoR is per-swing, scaled by weapon speed. `(0, 0)`
/// for a unit with no seal (baseline-safe). Pull model — the swing READS the seal aura, no per-spell code.
/// Also returns the seal's SPELL id so the swing can name the yellow proc line after it (114).
pub(crate) fn seal_holy_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
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
pub(crate) fn queued_strike_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
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
pub(crate) fn clear_queued_strike(ctx: &ReducerContext, guid: u64) {
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
pub(crate) enum ReactKind {
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
pub(crate) fn arm_react(ctx: &ReducerContext, guid: u64, kind: ReactKind) {
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
pub(crate) fn effective_strength(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
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
pub(crate) fn effective_agility(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
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
/// buff quickens it — PLUS, only while `guid` carries an active `A_MOUNTED` aura, every active
/// `A_MOD_SPEED(SPEED_MOUNTED)` aura (the mount's own speed effect; DBC-normalized to the nominal +60%/
/// +100% tiers). A mounted-speed aura contributes nothing once the rider dismounts — the fold reads
/// current mount state fresh on every call, so it never needs its own cleanup. The creature movement
/// passes (chase / return / wander) compute their step + leg duration from this instead of the raw
/// constant, so a snared creature closes/roams slower — the movement twin of `effective_swing_time`,
/// server-verifiable via the creature's per-tick position delta. `mount::recompute_mount` folds the SAME
/// value (base 1.0) into `run_speed_mult_bp` for the player wire relay; this copy governs SERVER-driven
/// movement and the anti-cheat max-speed check (`world::score_and_log_movement`), so a mounted player's
/// higher speed is never flagged as a speedhack. No move/mount aura → exactly `base` (baseline-safe). [entity]
pub fn effective_move_speed(ctx: &ReducerContext, guid: u64, base: f32) -> f32 {
    let move_pct = crate::spell::speed_bonus(ctx, guid, crate::spell::SPEED_MOVE);
    let mounted_pct = crate::spell::speed_bonus(ctx, guid, crate::spell::SPEED_MOUNTED);
    let mounted = crate::mount::active_mount_spell(ctx, guid).is_some();
    move_speed_with_pct(base, mounted_move_pct(move_pct, mounted_pct, mounted))
}

/// The mounted-speed fold's PURE core: `SPEED_MOVE` always contributes; `SPEED_MOUNTED` only while
/// `mounted` — so dismounting drops the mount's bonus but an ordinary buff/snare stays folded in, and a
/// leftover `SPEED_MOUNTED` reading (e.g. stale test data) never registers while `A_MOUNTED` is absent.
/// Pure — unit-tested without a `ReducerContext`. [pure]
fn mounted_move_pct(move_pct: i32, mounted_pct: i32, mounted: bool) -> i32 {
    move_pct + if mounted { mounted_pct } else { 0 }
}

/// The player's equipped WEAPON profile in inventory `slot`: `(damage_min, damage_max, delay_ms,
/// subclass)`, or `None` when the slot is EMPTY, its instance has no template, it holds a NON-weapon
/// (a shield/holdable frill in the off-hand, a relic/quiver in the ranged slot), or it holds a BROKEN
/// weapon (has durability and it hit 0 — that unit swings unarmed until it is repaired). The ONE
/// equipped-weapon reader: main-hand / off-hand / ranged all differ only in the slot they name, and
/// keeping three copies of this join is exactly how the "non-weapon" and "broken" gates drift apart.
/// The tooltip `damage_min`/`damage_max` are `f32`; rounded to whole points for the
/// integer swing math (vanilla weapon damage is whole numbers). `subclass` is the weapon subclass
/// (BOW=2, GUN=3, CROSSBOW=18, WAND=19, …) — only the ranged path reads it. [entity]
fn weapon_profile_in_slot(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Option<(u32, u32, u32, u8)> {
    let inst = crate::items::item_in_slot(ctx, player_guid, slot)?;
    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
    if tmpl.class != lyracore_shared::constants::starter_item::CLASS_WEAPON {
        return None; // a non-weapon in the slot contributes no weapon damage
    }
    if crate::items::item_is_broken(&tmpl, &inst) {
        return None; // a BROKEN weapon (has durability and it hit 0) gives no damage
    }
    Some((
        tmpl.damage_min.round() as u32,
        tmpl.damage_max.round() as u32,
        tmpl.delay_ms,
        tmpl.subclass,
    ))
}

/// The player's equipped main-hand WEAPON damage profile `(damage_min, damage_max, delay_ms)`, or
/// `None` when unarmed (see [`weapon_profile_in_slot`] for the full "no weapon here" rule) → the
/// swing falls back to the unarmed range. [entity]
fn equipped_weapon_damage(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32, u32)> {
    weapon_profile_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    )
    .map(|(dmin, dmax, delay, _)| (dmin, dmax, delay))
}

/// The player's equipped OFF-HAND WEAPON damage profile `(damage_min, damage_max, delay_ms)` — dual
/// wield. `None` when the off-hand is empty, holds a non-weapon (a shield/holdable frill, which
/// `effective_block_value` reads instead), or holds a BROKEN weapon. Unlike main-hand there is no
/// unarmed fallback — no live off-hand weapon means no second swing stream at all (the tick simply
/// skips the off-hand roll). [entity]
pub(crate) fn equipped_offhand_weapon_damage(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32)> {
    weapon_profile_in_slot(ctx, player_guid, crate::items::equip_slot::OFFHAND)
        .map(|(dmin, dmax, delay, _)| (dmin, dmax, delay))
}

/// The player's equipped RANGED weapon profile `(damage_min, damage_max, delay_ms, subclass)` from the
/// ranged slot (17) — a bow/gun/crossbow (Auto Shot) or a wand (Shoot) — or `None` when nothing usable
/// is equipped there. The ranged swing tick uses `delay_ms` as its swing interval and rolls damage from
/// `[damage_min, damage_max]`. [entity]
pub(crate) fn equipped_ranged_weapon(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32, u8)> {
    weapon_profile_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)
}

/// The player's loaded ammo: the lowest-slot inventory item that is Projectile ammo (item class 6 —
/// arrows subclass 2 / bullets 3) with stock left. Bow/gun/crossbow shots consume one per shot; `None` =
/// out of ammo (the engagement ends). Deliberate simplification: LOOSE match — any class-6 item
/// fires (we don't enforce the bow→arrow / gun→bullet subclass pairing); any ammo is fine for v1.
pub(crate) fn find_ammo(ctx: &ReducerContext, player_guid: u64) -> Option<ItemInstance> {
    let templates = ctx.db.game_item_template();
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .filter(|i| {
            // Banked ammo is not loaded — only carried stacks fire.
            crate::items::is_carried_slot(i.slot)
                && i.stack_count > 0
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
pub(crate) fn wear_weapon(ctx: &ReducerContext, player_guid: u64) {
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
        // Imported creature damage is anchored to the template's minimum level. An advancing
        // Hunter pet scales that range with its durable live level; summoned and wild creatures
        // retain the existing template range.
        let (base_min, base_max) =
            match (&tmpl, crate::creatures::live_pet_kind(ctx, attacker.guid)) {
                (Some(t), crate::creatures::PetKind::Hunter { .. }) => {
                    crate::creatures::scale_creature_damage_for_level(
                        base_min,
                        base_max,
                        t.level,
                        attacker.level,
                    )
                }
                _ => (base_min, base_max),
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
/// pre-absorb damage AND whether the crit landed; the caller folds absorb + threat + the kill via
/// `spell::apply_target_damage`, and carries the crit into the `Hit` so a crit-only Proc can read it —
/// the roll happens here, so nowhere else can tell. Reusing the swing/crit/armor math means an ability
/// scales off the weapon like a real strike, not a flat number. (Slice scope: specials always land — no
/// miss/dodge/parry roll yet; the attack table for yellow hits is a follow-up.) Uses the module RNG
/// (`ctx.random`, deterministic per reducer transaction). [entity]
pub fn weapon_strike_damage(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: &WorldEntity,
    flat_bonus: i32,
) -> (u32, bool) {
    let (min, max) = swing_range_ctx(ctx, caster);
    let base = min + ctx.random::<u32>() % (max - min + 1);
    let total = (base as i32 + flat_bonus).max(1) as u32;
    let crit = (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, caster);
    let pre_mit = if crit { total * CRIT_MULT } else { total };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), caster.level);
    ((pre_mit * (100 - mit_pct) / 100).max(1), crit)
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
    // The counter-swing's crit is rolled and spent inside the damage: a Triggered hit raises no proc
    // event, so there is no Proc to read the flag.
    let (dmg, _crit) = weapon_strike_damage(ctx, &victim, &attacker, 0);
    // Marked Triggered: the counter-swing is not an action the retaliator took, so it grants nothing
    // and raises no proc event — the same structural reason a Triggered Cast cannot chain a proc.
    crate::spell::apply_target_damage(
        ctx,
        attacker_guid,
        victim_guid,
        dmg as i32,
        crate::combat::Hit::triggered(),
    );
}

/// Roll one melee swing of `attacker` against `target`: `(damage, hit_info, blocked_amount)`. One roll
/// picks the attack-table outcome (bands scaled by the attacker/target level difference, the defender's
/// agility, and — when the defender has a shield — a block band); a second roll gives the damage variance
/// for a landed hit. Avoidance (miss/dodge/parry) deals 0; glancing lands reduced damage and can't crit; a
/// crit is 2× the base; a BLOCK lands a normal hit minus the shield's flat `block_value` (the absorbed
/// amount is the third tuple element, for the wire). Uses the module RNG (`ctx.random`, deterministic per
/// reducer transaction).
pub(crate) fn roll_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
) -> (u32, u8, u32) {
    let range = crate::creatures::scale_hunter_pet_swing(
        ctx,
        attacker.guid,
        swing_range_ctx(ctx, attacker),
    );
    roll_swing_with_range(ctx, attacker, target, range)
}

/// The shared core of `roll_swing`: identical attack-table roll + mitigation, but takes the `[min, max]`
/// damage range EXPLICITLY instead of deriving it from `attacker`'s equipped main-hand
/// (`swing_range_ctx`). Lets the off-hand swing in `resolve_swing` roll through the SAME shared attack
/// table + armor/block math over the off-hand's OWN (penalized) range, instead of re-deriving the
/// main-hand range. `roll_swing` is a thin wrapper that passes `swing_range_ctx(ctx, attacker)`.
pub(crate) fn roll_swing_with_range(
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
    // `fold_incoming_damage`). Unlike other landed hits it is NOT re-floored at 1 — a shield strong enough to
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
pub(crate) fn roll_ranged_swing(
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

#[cfg(test)]
mod mounted_speed_tests {
    use super::{mounted_move_pct, move_speed_with_pct};

    #[test]
    fn mounted_speed_folds_only_while_mounted() {
        // Slow mount (+60%), no ordinary buff, mounted: the fold sees the full +60%.
        assert_eq!(mounted_move_pct(0, 60, true), 60);
        // Fast mount (+100%), mounted.
        assert_eq!(mounted_move_pct(0, 100, true), 100);
        // Same mounted-speed aura amount, but NOT mounted (A_MOUNTED absent) — contributes nothing.
        assert_eq!(mounted_move_pct(0, 60, false), 0);
        // Mounted AND snared: both compose (a mount doesn't cancel an active slow).
        assert_eq!(mounted_move_pct(-20, 60, true), 40);
    }

    #[test]
    fn dismounting_keeps_an_ordinary_move_speed_buff_instead_of_forcing_bare_base() {
        // Dismounted with an ordinary SPEED_MOVE buff (e.g. a totem, +20%) still up: base + that
        // buff, never a blind reset to bare base.
        assert_eq!(mounted_move_pct(20, 0, false), 20);
        // The same buff plus a (now inert, unmounted) leftover SPEED_MOUNTED reading: still just the
        // ordinary buff — the mount's speed never leaks in once A_MOUNTED is gone.
        assert_eq!(mounted_move_pct(20, 60, false), 20);
    }

    #[test]
    fn nominal_60_and_100_percent_mount_tiers_yield_the_exact_run_speed() {
        // BASE_RUN_SPEED (7.0 yd/s) at +60% (slow mount) and +100% (fast mount).
        assert!((move_speed_with_pct(7.0, mounted_move_pct(0, 60, true)) - 11.2).abs() < 1e-4);
        assert!((move_speed_with_pct(7.0, mounted_move_pct(0, 100, true)) - 14.0).abs() < 1e-4);
    }
}
