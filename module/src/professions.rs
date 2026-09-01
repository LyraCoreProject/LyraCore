//! Profession reducers that are feature logic layered OVER a corpse or gear slot, not loot-table
//! machinery themselves (issue #384): SKINNING (a skill-gated corpse interaction modeled on
//! `loot::loot_money`), FISHING (an immediate-catch cast), and ENCHANTING (the per-instance
//! `enchant_id` overlay — which never touches a corpse at all, the reason this split exists).
//!
//! All three still call back into `crate::loot` as a normal cross-module dependency: the shared
//! roll core (`loot::roll_loot_rows`) and the `game_skinning_loot`/`game_fishing_loot` tables stay
//! there as DATA, alongside the other loot-family tables (`game_creature_loot`,
//! `game_pickpocket_loot`, `game_gameobject_loot`) — this module is the feature/reducer layer on
//! top, split out so it stops inflating `loot.rs`. Pure code motion from `loot.rs`; every gate and
//! grant is byte-identical. Old `crate::loot::skin_corpse`/`apply_fish`/`apply_disenchant`/
//! `apply_enchant_item`/`entry_is_beast` call sites (debug.rs's twins) keep compiling unchanged —
//! `loot.rs` re-exports this module's public surface.

use spacetimedb::{ReducerContext};

use crate::game_creature_template;
use crate::game_fishing_loot;
use crate::game_item_instance;
use crate::game_item_template;
use crate::game_player_skill;
use crate::game_skinning_loot;
use crate::game_world_entity;
use crate::loot::LOOT_RANGE_SQ;

// ===========================================================================================
//  SKINNING (professions slice 2) — a skill-gated corpse interaction, modeled on `loot_money`
// ===========================================================================================

/// The leather a skin yields — the REAL vanilla "Light Leather" `game_item_template` TRADE_GOODS entry
/// (2318); the slice mints a flat 1× per skin (a `game_skinning_loot`
/// table keyed on skill→tier is the DEFERRED real model — noted like the `RECIPES` map's
/// single-reagent-per-recipe shortcut).
pub(crate) const LEATHER_ENTRY: u32 = 2318; // "Light Leather" (real imported item)
/// Flat leather count per skin for the slice (DEFERRED: a skill→count table).
pub(crate) const LEATHER_COUNT: u32 = 1;

// Pure predicate over every gate a skinning attempt must pass, argument-shaped so it is unit-testable without a `ReducerContext`.
#[allow(clippy::too_many_arguments)]
/// The pure SKIN gate: may a `corpse` be skinned by a looter on `same_map`, in range? Decides on the
/// same primitives `loot_money` checks plus the beast/already-skinned markers — factored out (like
/// `loot::loot_drops`/`loot::group_pick`) so the gate is unit-testable without a live `ReducerContext`.
/// Returns the reject reason, or `Ok(())` to proceed. (Distance is the already-computed squared value
/// vs `LOOT_RANGE_SQ`.)
///
/// `learned_skinning` is the character's current Skinning skill value (0 = not learned — no skill row),
/// mirroring `gameobject::can_gather`. `creature_level` is the corpse's level. Vanilla requires
/// Skinning trained AND skill >= (creature_level - 1) * 10, so level-1 beasts are always skinnable
/// with Apprentice Skinning (any skill ≥ 1).
pub(crate) fn can_skin(
    looter_dead: bool,
    corpse_is_player: bool,
    corpse_dead: bool,
    same_map: bool,
    dist_sq: f32,
    is_beast: bool,
    already_skinned: bool,
    learned_skinning: u32,
    creature_level: u32,
) -> Result<(), String> {
    if looter_dead {
        return Err("dead players cannot skin".to_string());
    }
    if corpse_is_player {
        return Err("cannot skin a player".to_string());
    }
    if !corpse_dead {
        return Err("target is not a corpse".to_string());
    }
    if !same_map {
        return Err("corpse on another map".to_string());
    }
    if dist_sq > LOOT_RANGE_SQ {
        return Err("corpse out of range".to_string());
    }
    if !is_beast {
        return Err("not a beast — cannot be skinned".to_string());
    }
    if already_skinned {
        return Err("already skinned".to_string());
    }
    // Skill gate: must have trained Skinning AND skill >= (creature_level - 1) * 10.
    // Level-1 beasts require 0 skill so a freshly-trained skinner (skill=1) can immediately skin
    // them, matching vanilla Apprentice Skinning.  Level-2 beasts require 10, level-3 require 20,
    // etc.  saturating_sub guards against a hypothetical level-0 creature (maps to 0 required).
    if learned_skinning == 0 {
        return Err("you have not learned Skinning".to_string());
    }
    let required = creature_level.saturating_sub(1) * 10;
    if learned_skinning < required {
        return Err(format!(
            "your Skinning skill ({learned_skinning}) is too low — need {required}"
        ));
    }
    Ok(())
}

/// Is `entry`'s creature a BEAST (creature_type == 1)? The skinnable gate — reads the imported
/// `game_creature_template.creature_type` (loaded verbatim by the importer). A missing template (no row)
/// is not a beast (false) — a corpse with no template can't be skinned, never panics.
pub(crate) fn entry_is_beast(ctx: &ReducerContext, entry: u32) -> bool {
    ctx.db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_some_and(|t| t.creature_type == crate::spell::BEAST_TYPE)
}

/// SKIN a beast corpse: the core (resolved guids), modeled on `loot_money`. Requires the player to have
/// TRAINED Skinning at a trainer AND have skill >= corpse_level * 10 (vanilla floor), mirroring the
/// `can_gather` gate. Grants leather, climbs the Skinning line +1, and marks the corpse skinned so it
/// can't be re-skinned. Shared by the debug lever (`debug_skin_nearest`) and a future `CMSG`-routed
/// skin over the open corpse. The skinner must be Loot Tag eligible, and all corpse money and item rows
/// must already be gone. Returns the Refusal reason on any Gate failure, or `Ok` after the grant.
///
/// ROLLBACK: the leather grant (`grant_item`) is `?`-propagated — a full bag returns `Err` BEFORE the
/// `skinned` marker is set, so the reducer tx rolls back and the corpse stays skinnable (no leather is
/// minted on a full bag, and the player can retry), exactly like the COOKING reagent gate.
pub(crate) fn skin_corpse(
    ctx: &ReducerContext,
    looter_guid: u64,
    corpse_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let looter = entities
        .guid()
        .find(looter_guid)
        .ok_or_else(|| "skinner not in world".to_string())?;
    let mut corpse = entities
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| "no such corpse".to_string())?;
    crate::loot::corpse_access_gate(ctx, looter_guid, corpse_guid)?;

    // All gates (the pure decision shared with the unit test): looter alive, a dead non-player BEAST
    // corpse, same map, in range, not already skinned, trained Skinning + skill >= level*10.
    let dist_sq = (corpse.x - looter.x).powi(2)
        + (corpse.y - looter.y).powi(2)
        + (corpse.z - looter.z).powi(2);
    // Learned Skinning skill (0 = not trained — no skill row), mirroring the `can_gather` gather gate.
    let learned_skinning = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&looter_guid)
        .find(|s| s.skill_line == crate::skill::skill_line::SKINNING)
        .map(|s| s.current as u32)
        .unwrap_or(0);
    can_skin(
        looter.dead,
        corpse.is_player(),
        corpse.dead,
        // Map + instance (190 slice 2): `can_skin`'s `same_map` conjunct now carries the
        // instance-equality clause too — the corpse is a `game_world_entity` row, slice-1-tagged.
        corpse.map_id == looter.map_id && corpse.instance_id == looter.instance_id,
        dist_sq,
        entry_is_beast(ctx, corpse.entry),
        corpse.skinned,
        learned_skinning,
        corpse.level,
    )?;
    if !crate::loot::corpse_is_looted(ctx, corpse_guid, corpse.money) {
        return Err("corpse still has loot".to_string());
    }

    // Grant leather — DATA-DRIVEN (work-item 210): the corpse's creature template names a
    // `skin_loot_id` (cmangos `skinning_loot_template`, level-banded across many creatures sharing a
    // band); roll it and grant every winner. `skin_loot_id == 0` (unimported / a seeded/test beast) OR
    // an empty/no-win roll falls back to the flat `LEATHER_ENTRY`/`LEATHER_COUNT` — byte-identical to
    // the pre-210 alpha, so a skin never comes up empty-handed. `?`-rollback: ANY winner's `grant_item`
    // failing (full bag) rolls back the WHOLE skin BEFORE the corpse is marked skinned (the reducer tx
    // rolls the whole call back), so no partial skin — retry after freeing space, exactly the COOKING
    // reagent-gate rollback shape.
    let skin_loot_id = ctx
        .db
        .game_creature_template()
        .entry()
        .find(corpse.entry)
        .map(|t| t.skin_loot_id)
        .unwrap_or(0);
    let rows: Vec<(u32, u32, u32, u32)> = if skin_loot_id != 0 {
        ctx.db
            .game_skinning_loot()
            .by_skin()
            .filter(&skin_loot_id)
            .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id))
            .collect()
    } else {
        Vec::new()
    };
    let winners = crate::loot::roll_loot_rows(ctx, rows);
    if winners.is_empty() {
        crate::items::grant_item(ctx, looter_guid, LEATHER_ENTRY, LEATHER_COUNT)?;
    } else {
        for (item_entry, count) in winners {
            crate::items::grant_item(ctx, looter_guid, item_entry, count)?;
        }
    }

    // Skill-up — the COOKING skill-up hook reused verbatim (no-op if not learned / at the cap;
    // the gate above already confirmed learned_skinning > 0, so the row exists). The DEFAULT sentinel band
    // (orange=1, gray=0 ⇒ always +1) keeps skinning byte-identical to the pre-difficulty slice — a real
    // Skinning difficulty table (gray>0) is DEFERRED.
    crate::skill::gain_profession_skill(ctx, looter_guid, crate::skill::skill_line::SKINNING, 1, 0);

    // Mark skinned so a second skin is rejected (the gate above). Reset for free on respawn
    // (`build_creature_entity`). The corpse decays normally.
    corpse.skinned = true;
    entities.guid().update(corpse);
    Ok(())
}

// ===========================================================================================
//  FISHING (completing the 13) — a bounded new gather mechanic: an immediate-catch reducer modeled on
//  `skin_corpse` (auto-learn → grant a fish → climb the line). No bobber/cast/timer (DEFERRED), and a
//  LENIENT water gate (alpha: just alive) — the real near-water LiquidType check is a follow-up. [entity]
// ===========================================================================================

/// The small-fish FALLBACK pool — REAL low-level cmangos fish `game_item_template` entries (standard
/// trade-goods present in the imported box). Used when a cast's zone has NO `game_fishing_loot` rows
/// (unimported, or off-slice) — since work-item 210 the real per-zone table (`game_fishing_loot`) is
/// tried FIRST; this is the "never comes up empty-handed" floor beneath it (mirrors the skinning
/// `LEATHER_ENTRY` fallback). If a fish entry isn't in the imported `item_template`, `grant_item`
/// returns Err and the whole `fish` rolls back (no half-catch) — so verify these three are in-box.
pub(crate) const FISH_POOL: &[u32] = &[
    6291, // Raw Brilliant Smallfish
    6303, // Raw Slitherskin Mackerel
    6289, // Raw Longjaw Mud Snapper
];

/// Pure choice of the CAUGHT fish: the zone table's `winner` if the roll produced one, else a
/// deterministic `FISH_POOL` pick via `pool_idx` (the caller passes an already-reduced
/// `ctx.random::<usize>()`, so this stays a pure index lookup). Covers BOTH fallback triggers — no zone
/// rows at all, or a zone table that rolled nothing this cast — with the SAME floor, so a cast never
/// comes up empty-handed. Split out so the fallback decision is unit-testable.
pub(crate) fn pick_caught_fish(winner: Option<u32>, pool_idx: usize) -> u32 {
    winner.unwrap_or_else(|| FISH_POOL[pool_idx % FISH_POOL.len()])
}

/// FISH at the player's current spot: the core (resolved guid), modeled on `skin_corpse`. LENIENT alpha
/// gate (alive only — the near-water LiquidType check is DEFERRED). Auto-learns Fishing on the first cast
/// (idempotent, born 1/75 like skinning), resolves the caster's ZONE (`terrain::zone_id_at`) and rolls
/// `game_fishing_loot` for it — falling back to the flat `FISH_POOL` when the zone is unresolved, has no
/// rows, or the roll lands on nothing (`pick_caught_fish`) — grants the caught fish, climbs the Fishing
/// line +1 (the default sentinel band ⇒ deterministic +1). Shared by the `fish` player reducer + the
/// `debug_fish` twin. ROLLBACK: the `grant_item` is `?`-propagated — a full bag returns Err BEFORE the
/// skill-up commits (the tx rolls back), so no fish is minted on a full bag and the skill doesn't climb.
pub(crate) fn apply_fish(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    let player =
        crate::helpers::live_entity(ctx, guid).map_err(|_| "fisher not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot fish".to_string());
    }
    // AUTO-LEARN Fishing on the first cast (idempotent — a re-cast never resets a climbed value), so the
    // skill-up hook has a row to climb. The very first cast is born 1/75; this cast's +1 takes it to 2/75.
    crate::skill::learn_profession(
        ctx,
        guid,
        player.owner_identity,
        crate::skill::skill_line::FISHING,
        crate::skill::APPRENTICE_CAP,
    );
    // Zone-keyed catch (work-item 210, resolver hoisted to `terrain::zone_id_at` by #375): resolve the
    // caster's zone and roll its real table; an unresolved zone (unimported terrain/AreaTable, or
    // off-slice) rolls an empty Vec, which `roll_loot_rows` correctly turns into zero winners —
    // `pick_caught_fish` then floors to the flat pool either way. `?`-rollback: a full bag fails here
    // BEFORE the skill-up, so nothing is minted and the skill doesn't climb (retry after freeing space).
    let rows: Vec<(u32, u32, u32, u32)> =
        crate::terrain::zone_id_at(ctx, player.map_id, player.x, player.y)
            .map(|zone| {
                ctx.db
                    .game_fishing_loot()
                    .by_zone()
                    .filter(&zone)
                    .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id))
                    .collect()
            })
            .unwrap_or_default();
    let winner = crate::loot::roll_loot_rows(ctx, rows)
        .into_iter()
        .next()
        .map(|(item, _)| item);
    let fish = pick_caught_fish(winner, ctx.random::<usize>());
    crate::items::grant_item(ctx, guid, fish, 1)?;
    // Skill-up — the same hook skinning/cooking reuse. DEFAULT sentinel band (orange=1, gray=0 ⇒ always +1)
    // keeps fishing on the byte-identical deterministic path; a real Fishing difficulty table is DEFERRED.
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::FISHING, 1, 0);
    Ok(())
}

// ===========================================================================================
//  ENCHANTING (completing the 13) — two reducers over the per-instance enchant overlay (`enchant_id` on
//  ItemInstance + the `rules::ENCHANTS` stat table). `disenchant` consumes an equipped item → mats +
//  skill; `enchant_item` stamps an enchant id onto an equipped instance (server-real via the
//  effective-* pipeline). The client glow/green-text is DEFERRED (by-entry item-stat cache). [entity]
// ===========================================================================================

/// The mats a disenchant yields — REAL "Strange Dust" `game_item_template` entry (10940, the low-level
/// enchanting reagent, present in the imported box). The alpha mints a flat 1× per disenchant; a real
/// quality/ilvl→mats table is DEFERRED (the skinning `LEATHER_ENTRY` single-yield shortcut, again).
pub(crate) const DISENCHANT_MATS_ENTRY: u32 = 10940; // "Strange Dust"
pub(crate) const DISENCHANT_MATS_COUNT: u32 = 1;

/// Disenchant/enchant apply only to GEAR (weapon or armor class) — not trade goods, reagents, or stacks.
/// Gates the "disenchant a stack of 20 ore for 1 dust" foot-gun AND the "enchant the dust slot" stale-write
/// (with the target forced to gear, the mats-consume can never touch it). Deliberate
/// simplification: class gate only; the quality≥uncommon gate stays deferred (any weapon/armor
/// disenchants in the alpha).
fn require_gear(ctx: &ReducerContext, entry: u32) -> Result<(), String> {
    const ITEM_CLASS_WEAPON: u8 = 2;
    const ITEM_CLASS_ARMOR: u8 = 4;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry)
        .ok_or_else(|| format!("no template for item {entry}"))?;
    if tmpl.class != ITEM_CLASS_WEAPON && tmpl.class != ITEM_CLASS_ARMOR {
        return Err(format!(
            "item {entry} is not enchantable (weapon/armor only)"
        ));
    }
    Ok(())
}

/// DISENCHANT the item in `slot`: consume it (delete the instance), grant enchanting mats, climb
/// Enchanting +1. The core (resolved guid), shared by the `disenchant` reducer + `debug_disenchant` twin.
/// GATE: alive + a weapon/armor item present in `slot` (see `require_gear`). Auto-learns Enchanting on the
/// first disenchant (idempotent). ROLLBACK: `grant_item` is `?`-propagated BEFORE the instance is deleted,
/// so a full bag fails the whole tx and the item is NOT consumed (no item destroyed without delivering mats).
pub(crate) fn apply_disenchant(ctx: &ReducerContext, guid: u64, slot: u8) -> Result<(), String> {
    let player =
        crate::helpers::live_entity(ctx, guid).map_err(|_| "enchanter not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot disenchant".to_string());
    }
    let inst = crate::items::item_in_slot(ctx, guid, slot)
        .ok_or_else(|| format!("no item in slot {slot}"))?;
    require_gear(ctx, inst.entry)?; // weapon/armor only — never destroy a trade-good stack for 1 dust
                                    // AUTO-LEARN Enchanting on the first disenchant (idempotent), giving the skill-up hook a row to climb.
    crate::skill::learn_profession(
        ctx,
        guid,
        player.owner_identity,
        crate::skill::skill_line::ENCHANTING,
        crate::skill::APPRENTICE_CAP,
    );
    // Grant mats FIRST (the `?`-rollback point): a full bag fails here BEFORE the item is consumed, so a
    // failed disenchant destroys nothing and grants nothing (the tx rolls back) — retry after freeing space.
    crate::items::grant_item(ctx, guid, DISENCHANT_MATS_ENTRY, DISENCHANT_MATS_COUNT)?;
    // Consume the disenchanted item (delete the instance) and climb the line. After the grant succeeded, so
    // the delete + skill-up only run on a delivered disenchant.
    ctx.db.game_item_instance().guid().delete(inst.guid);
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::ENCHANTING, 1, 0);
    // If the disenchanted item was EQUIPPED gear (slot 0..=18), its stats just left the body — re-derive
    // max HP/mana (a disenchanted +Sta piece shrinks the health bar). A bag item (>18) touches no equip slot.
    if slot <= crate::items::equip_slot::END {
        crate::spell::recompute_vitals(ctx, guid);
        crate::spell::recompute_sheet(ctx, guid);
    }
    Ok(())
}

/// ENCHANT the item in `target_slot` with `enchant_id`: validate the id (it must be a known enchant in
/// `rules::ENCHANTS`), consume the enchanting mats, stamp `enchant_id` onto the instance, climb Enchanting.
/// The core (resolved guid), shared by the `enchant_item` reducer + `debug_enchant_item` twin. The enchant
/// is server-REAL: `equipped_stat_bonus` now folds `enchant_stat(enchant_id, ..)`, so an equipped enchanted
/// piece moves the effective-* readouts (swing/dodge/armor/crit/hit + max HP/mana via recompute_vitals).
/// The client glow/green-text is DEFERRED (the 5875 client caches item stats by ENTRY). ROLLBACK:
/// `remove_items` (the mats consume) is `?`-propagated BEFORE the instance is updated, so a missing-mats
/// enchant changes nothing.
pub(crate) fn apply_enchant_item(
    ctx: &ReducerContext,
    guid: u64,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), String> {
    let player =
        crate::helpers::live_entity(ctx, guid).map_err(|_| "enchanter not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot enchant".to_string());
    }
    // The enchant id must be a known, applyable enchant — a client can't stamp an arbitrary id.
    if !crate::items::is_known_enchant(enchant_id) {
        return Err(format!("unknown enchant {enchant_id}"));
    }
    let mut inst = crate::items::item_in_slot(ctx, guid, target_slot)
        .ok_or_else(|| format!("no item in slot {target_slot}"))?;
    require_gear(ctx, inst.entry)?; // weapon/armor only — also keeps the target ≠ the mats (no stale write)
                                    // Consume the enchanting mats FIRST (the `?`-rollback point): not enough Strange Dust → Err here,
                                    // BEFORE the instance is stamped, so the item is unchanged and no skill is gained.
    crate::items::remove_items(ctx, guid, DISENCHANT_MATS_ENTRY, DISENCHANT_MATS_COUNT)?;
    // Stamp the enchant onto THIS instance (the per-instance overlay). The overlay folds through
    // `equipped_stat_bonus` for every effective-* consumer — server-real, no client display this slice.
    let instances = ctx.db.game_item_instance();
    inst.enchant_id = enchant_id;
    instances.guid().update(inst);
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::ENCHANTING, 1, 0);
    // If the enchanted item is EQUIPPED gear (slot 0..=18) and the enchant moves a pool stat (Stamina/
    // Intellect), re-derive max HP/mana so the bar grows. `recompute_vitals` is a no-op when the derived
    // max is unchanged (e.g. a +Strength weapon enchant), so this is safe to call unconditionally for gear.
    if target_slot <= crate::items::equip_slot::END {
        crate::spell::recompute_vitals(ctx, guid);
        crate::spell::recompute_sheet(ctx, guid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- SKINNING (professions slice 2) ----

    /// The SKIN gate: every reject condition fails with its reason, and ONLY an alive looter at a dead,
    /// in-range, same-map BEAST corpse that isn't already skinned proceeds (with sufficient Skinning skill).
    /// This is the pure decision the live `skin_corpse` keys all its rejects off (mirrors `loot_money`'s
    /// gate, plus beast + skinned + skill checks).
    #[test]
    fn skin_gate_admits_only_a_fresh_in_range_beast_corpse() {
        // Helpers: learned=1 (freshly trained), creature_level=1 (requires (1-1)*10=0) — the baseline pass.
        let learned: u32 = 1;
        let lvl: u32 = 1; // required = (1-1)*10 = 0 <= learned → passes

        // The all-pass baseline: alive looter, dead non-player beast, same map, in range, not skinned,
        // trained Skinning at sufficient skill.
        assert!(can_skin(false, false, true, true, 0.0, true, false, learned, lvl).is_ok());
        // In range at exactly the cap (10 yd)² is still OK (inclusive, matching loot_money's `>`).
        assert!(can_skin(
            false,
            false,
            true,
            true,
            LOOT_RANGE_SQ,
            true,
            false,
            learned,
            lvl
        )
        .is_ok());

        // Each reject, in the order the gate checks them:
        assert_eq!(
            can_skin(true, false, true, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "dead players cannot skin"
        );
        assert_eq!(
            can_skin(false, true, true, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "cannot skin a player"
        );
        assert_eq!(
            can_skin(false, false, false, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "target is not a corpse"
        );
        assert_eq!(
            can_skin(false, false, true, false, 0.0, true, false, learned, lvl).unwrap_err(),
            "corpse on another map"
        );
        assert_eq!(
            can_skin(
                false,
                false,
                true,
                true,
                LOOT_RANGE_SQ + 1.0,
                true,
                false,
                learned,
                lvl
            )
            .unwrap_err(),
            "corpse out of range"
        );
        // NON-BEAST rejection (e.g. the demo Chicken, creature_type 8 → is_beast=false).
        assert_eq!(
            can_skin(false, false, true, true, 0.0, false, false, learned, lvl).unwrap_err(),
            "not a beast — cannot be skinned"
        );
        // RE-SKIN rejection (already_skinned=true) — the marker that stops a second skin of the same corpse.
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, true, learned, lvl).unwrap_err(),
            "already skinned"
        );
        // NOT TRAINED: learned=0 → "you have not learned Skinning".
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 0, lvl).unwrap_err(),
            "you have not learned Skinning"
        );
        // SKILL TOO LOW: learned=5, creature_level=2 (requires (2-1)*10=10) → rejected.
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 5, 2).unwrap_err(),
            "your Skinning skill (5) is too low — need 10"
        );
        // BOUNDARY: learned exactly equals the requirement → admitted (lvl=2 → required=10).
        assert!(can_skin(false, false, true, true, 0.0, true, false, 10, 2).is_ok());
        // Higher-level creature: creature_level=3 requires (3-1)*10=20; learned=19 rejected, 20 admitted.
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 19, 3).unwrap_err(),
            "your Skinning skill (19) is too low — need 20"
        );
        assert!(can_skin(false, false, true, true, 0.0, true, false, 20, 3).is_ok());
    }

    /// Renamed (was `skinning_yields_one_light_leather_2318` — "yields" oversold a behavioral check when
    /// this only guards the constants): the leather product is the REAL "Light Leather" TRADE_GOODS entry
    /// (2318), minted a flat 1 per skin (the skill→count table is DEFERRED). A wire-facing drift guard.
    #[test]
    fn skin_leather_constants_are_the_real_light_leather_2318_flat_one() {
        assert_eq!(LEATHER_ENTRY, 2318);
        assert_eq!(LEATHER_COUNT, 1);
    }

    // ---- FISHING (completing the 13) ----

    /// Re-scoped (was `fish_grants_one_pooled_fish_and_climbs_fishing`): the "grants one" / "climbs
    /// fishing" narrative modeled `apply_fish`'s grant+skill-up with a local loop that never calls
    /// `apply_fish` (a ctx fn) — not exercised here. What's real and worth guarding: the pool itself,
    /// which `apply_fish` indexes with `ctx.random::<usize>() % FISH_POOL.len()` — a drift guard on its
    /// exact non-empty, real-entry contents (an empty pool would divide by zero at cast time).
    #[test]
    fn fish_pool_is_nonempty_and_lists_the_real_low_level_fish_entries() {
        assert!(
            !FISH_POOL.is_empty(),
            "fishing must have a pool to roll (an empty pool divides by zero)"
        );
        assert_eq!(
            FISH_POOL,
            &[6291, 6303, 6289],
            "the three real low-level fish entries"
        );
    }

    /// `pick_caught_fish` — the fallback floor: a real winner always wins; `None` (no zone rows, or a
    /// zone table that rolled nothing) floors to `FISH_POOL` at the given index, wrapping via `%` so an
    /// out-of-range `pool_idx` (a raw `ctx.random::<usize>()`) never panics.
    #[test]
    fn pick_caught_fish_prefers_the_winner_else_floors_to_the_pool() {
        assert_eq!(
            pick_caught_fish(Some(6291), 0),
            6291,
            "a real winner is never overridden by the pool"
        );
        assert_eq!(pick_caught_fish(None, 0), FISH_POOL[0]);
        assert_eq!(
            pick_caught_fish(None, FISH_POOL.len()),
            FISH_POOL[0],
            "wraps via %, never panics OOB"
        );
        assert_eq!(pick_caught_fish(None, 1), FISH_POOL[1]);
    }

    // ---- ENCHANTING: disenchant (completing the 13) ----

    /// Re-scoped (was `disenchant_consumes_item_and_grants_dust_and_skill`): the consume/grant/skill-up
    /// narrative modeled `apply_disenchant`'s effect with local counters that never call it (a ctx fn) —
    /// not exercised here. What's real and worth guarding: the mats entry/count `apply_disenchant` grants,
    /// a drift guard against the wire-facing item id ever moving.
    #[test]
    fn disenchant_mats_are_the_real_strange_dust_entry() {
        assert_eq!(
            DISENCHANT_MATS_ENTRY, 10940,
            "Strange Dust (real imported enchanting reagent)"
        );
        assert_eq!(
            DISENCHANT_MATS_COUNT, 1,
            "the alpha mints a flat 1x Strange Dust"
        );
    }
}
