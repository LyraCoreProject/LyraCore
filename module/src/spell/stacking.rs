//! Stacking-group exclusion + CC diminishing returns — work-item 192. `module/src/spell/cast/targeting.rs`'s
//! `aura_apply` (already the sole `game_aura` insert site — verified by grep, see the work-item report) is
//! the ONLY caller of `apply_group_conflict`/`resolve_dr_for_target`, so every aura placement in the engine
//! routes through this file (the "filtered_gossip_options lesson": one chokepoint, never a second copy of
//! the policy).
//!
//! Two concerns, two pure decision fns, sharing one result type (`ApplyDecision`) — they're mutually
//! exclusive per effect (a spell effect is either a stacking-group member OR a CC-DR effect, never both in
//! today's data), so there's no shared "incoming application" struct pretending to unify them:
//!   1. STACKING GROUPS (`game_spell_group` + `game_spell_group_rule`) — same-effect exclusive families
//!      across casters/spells (Fortitude, Blessings, armor debuffs, ...). Module-only tables (no
//!      `public`, no gateway binding — mirrors the `game_spell_chain`/`game_spell_learn` precedent),
//!      because `game_aura` itself carries everything the client needs (the eviction just deletes rows);
//!      NO `game_aura` column was added for this (verified: see the report's "zero subscribed-schema
//!      change" note). Decided by [`resolve_group_conflict`].
//!   2. CC DIMINISHING RETURNS (`game_dr_state`) — PLAYER targets only; same-category CC within 15s of
//!      the PREVIOUS REMOVAL lands at 100/50/25/0% duration. The window starts at REMOVAL (natural expiry
//!      OR dispel), never at apply — the classically-misimplemented part the work item calls out. Decided
//!      by [`resolve_dr`].

use spacetimedb::{log, table, ReducerContext, Table};

use crate::game_world_entity;
// Aura/SpellEffect row types, the `game_aura`/`game_spell_chain` accessors, and the A_CONTROL/P_MECHANIC/
// M_* taxonomy consts are all re-exported by `mod.rs` (`pub use tables::*` / `pub(crate) use taxonomy::*`).
use super::*;

// ===========================================================================================
//  Tables (module-only: NO `public`, no Timestamp, no gateway binding — same shape/rationale as
//  `game_spell_chain`/`game_spell_learn`). `game_aura` itself is UNCHANGED (verified below in the report):
//  strength/category are computed at apply time, so this whole feature lands with zero subscribed-schema
//  change.
// ===========================================================================================

/// One `(group_id, spell_id)` membership row — a group has 1+ member spells. `id` is the PK (not
/// `(group_id, spell_id)`) because SpacetimeDB tables need a single-column PK and this one is purely a
/// membership list (no natural single-column key). [static]
#[table(accessor = game_spell_group, index(accessor = by_group, btree(columns = [group_id])), index(accessor = by_spell, btree(columns = [spell_id])))]
pub struct SpellGroup {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub group_id: u32,
    pub spell_id: u32,
}

/// The stacking rule for `group_id` (PK — one rule row per group). `rule`: 0 STACKS (a no-op group — every
/// member stacks normally, present for completeness/future data), 1 EXCLUSIVE (one aura from the group per
/// target, any caster — latest application evicts every other member), 2 EXCLUSIVE_PER_CASTER (one per
/// CASTER per target — Blessings: a paladin's own Blessing replaces their prior one, another paladin's
/// stands), 3 EXCLUSIVE_STRONGER (refuse the incoming application if a STRONGER member is present, else
/// replace every member). [static]
#[table(accessor = game_spell_group_rule)]
pub struct SpellGroupRule {
    #[primary_key]
    pub group_id: u32,
    pub rule: u8,
    /// Whether spell-chain ranks are comparable within this group. Cross-family groups such as
    /// armor debuffs deliberately compare their effective magnitude only: a rank number from
    /// Sunder Armor does not describe the strength of Expose Armor or Faerie Fire.
    #[default(true)]
    pub rank_is_comparable: bool,
}

/// Rule byte constants (`game_spell_group_rule.rule`) — mirrors the work item's architecture section
/// verbatim. `seed.rs` and `group_rule_from_u8` are the only readers. `RULE_STACKS` (0) is never matched
/// BY NAME in `group_rule_from_u8` (it's the `_` wildcard fallback, so any future rule byte the importer
/// doesn't recognize degrades to it too) — `#[allow(dead_code)]` documents that as deliberate, not a bug.
#[allow(dead_code)]
pub(crate) const RULE_STACKS: u8 = 0;
pub(crate) const RULE_EXCLUSIVE: u8 = 1;
pub(crate) const RULE_EXCLUSIVE_PER_CASTER: u8 = 2;
pub(crate) const RULE_EXCLUSIVE_STRONGER: u8 = 3;

/// Per-`(target_guid, category)` CC diminishing-returns state — RLS-free (server-internal bookkeeping,
/// nothing the client needs directly: DR manifests only as a shorter aura duration), small, reaped by
/// `sweep_dr_state` (wired into the shared event-GC pass — see that fn's doc comment for exactly where).
/// `level` is "how many CC hits have already landed in the current 15s-after-removal window" (1..=3 while
/// a window is live; a target with no live window simply has no row, treated as level 0 — see
/// `dr_pre_level`). `window_expires_micros` is stamped at APPLY time as a PROVISIONAL value (this aura's
/// own scaled expiry + 15s, so a recast of the SAME spell while it's still active reads a sane in-window
/// state) and OVERWRITTEN with the AUTHORITATIVE value at REMOVAL (natural expiry in
/// `scheduler::tick_auras`, or dispel in `effects::dispel_target`) — see `bump_dr_level` / `dr_window_on_removal`.
/// [entity]
#[table(accessor = game_dr_state, index(accessor = by_target, btree(columns = [target_guid])))]
pub struct DrState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub category: u8,
    pub level: u8, // 1..=3
    pub window_expires_micros: i64,
}

/// CC diminishing-returns category constants. These are OUR internal ids (nothing on the wire or in a
/// DBC keys on them; the numbering happens to agree with the one the reference cores use, which keeps
/// cross-checking a category assignment against public documentation cheap) — NOT the same numbering
/// as this crate's own `M_STUN`/`M_ROOT`/`M_FEAR`/`M_POLY` mechanic consts in `taxonomy.rs`, hence the
/// separate mapping fn below rather than reusing the mechanic value directly). Only stun/fear/root/poly are
/// reachable today (the only mechanics we have — `M_CHARM`/`M_SLEEP`/`M_DISORIENT` don't exist yet);
/// charm/sleep/disorient are declared for completeness and to extend cleanly as those mechanics import.
pub(crate) const DR_CAT_STUN: u8 = 1;
pub(crate) const DR_CAT_FEAR: u8 = 2;
// Reserved: no mechanic maps to these yet (`M_CHARM`/`M_SLEEP`/`M_DISORIENT` don't exist in `taxonomy.rs`
// today) — declared now so `dr_category_for_mechanic` extends by adding ONE match arm when they import,
// never a renumbering. `#[allow(dead_code)]` documents that as deliberate.
#[allow(dead_code)]
pub(crate) const DR_CAT_CHARM: u8 = 3;
pub(crate) const DR_CAT_ROOT: u8 = 4;
#[allow(dead_code)]
pub(crate) const DR_CAT_SLEEP: u8 = 5;
pub(crate) const DR_CAT_POLY: u8 = 6;
#[allow(dead_code)]
pub(crate) const DR_CAT_DISORIENT: u8 = 7;

/// 15 seconds, in micros — the DR window length, measured from the CC aura's REMOVAL (not its apply).
pub(crate) const DR_WINDOW_MICROS: i64 = 15_000_000;

// ===========================================================================================
//  Pure types + the two chokepoint fns (resolve_group_conflict / resolve_dr).
// ===========================================================================================

/// A minimal snapshot of an EXISTING aura relevant to a stacking-group decision — just enough to compare
/// against the incoming application without touching the live row inside the pure fn. The caller
/// (`apply_group_conflict`) builds these from `game_aura` + the strength formula below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuraSummary {
    pub aura_id: u64,
    pub caster_guid: u64,
    pub strength: i32,
}

/// Which stacking rule governs the incoming spell's group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupRule {
    Stacks,
    Exclusive,
    ExclusivePerCaster,
    ExclusiveStronger,
}

/// Decode `game_spell_group_rule.rule` — an unrecognized byte (shouldn't happen; defensive) falls back to
/// `Stacks` (the safest no-op: never silently refuses/evicts on a data bug).
pub(crate) fn group_rule_from_u8(rule: u8) -> GroupRule {
    match rule {
        RULE_EXCLUSIVE => GroupRule::Exclusive,
        RULE_EXCLUSIVE_PER_CASTER => GroupRule::ExclusivePerCaster,
        RULE_EXCLUSIVE_STRONGER => GroupRule::ExclusiveStronger,
        _ => GroupRule::Stacks,
    }
}

/// The DR window state read for `(target, category)` BEFORE this application, or `None` if no row exists
/// yet (a target that has never taken this category of CC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DrWindow {
    pub level: u8, // 1..=3, as stored
    pub window_expires_micros: i64,
}

/// The verdict shared by both chokepoints below. `evict` is always empty for a DR decision (DR only
/// scales/refuses, it never evicts another aura — the two concerns don't overlap).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApplyDecision {
    Apply {
        duration_scale_bp: u32,
        evict: Vec<u64>,
    },
    Refuse {
        reason: &'static str,
    },
}

/// THE stacking-group chokepoint (work-item 192). Called by `apply_group_conflict` for a fresh
/// stacking-group member application (a same-spell refresh never reaches this — see that fn's doc). Pure:
/// no DB access, no logging, no side effects — the caller does all the I/O and acts on the returned
/// `ApplyDecision`.
pub(crate) fn resolve_group_conflict(
    caster_guid: u64,
    strength: i32,
    rule: GroupRule,
    existing: &[AuraSummary],
) -> ApplyDecision {
    match rule {
        GroupRule::Stacks => ApplyDecision::Apply {
            duration_scale_bp: 10_000,
            evict: Vec::new(),
        },
        GroupRule::Exclusive => ApplyDecision::Apply {
            duration_scale_bp: 10_000,
            evict: existing.iter().map(|a| a.aura_id).collect(),
        },
        GroupRule::ExclusivePerCaster => ApplyDecision::Apply {
            duration_scale_bp: 10_000,
            evict: existing
                .iter()
                .filter(|a| a.caster_guid == caster_guid)
                .map(|a| a.aura_id)
                .collect(),
        },
        GroupRule::ExclusiveStronger => {
            if let Some(strongest) = existing.iter().map(|a| a.strength).max() {
                if strongest > strength {
                    return ApplyDecision::Refuse { reason: "weaker" };
                }
            }
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: existing.iter().map(|a| a.aura_id).collect(),
            }
        }
    }
}

/// THE CC-diminishing-returns chokepoint (work-item 192). Called by `resolve_dr_for_target` for every
/// `A_CONTROL(mechanic)` effect on a player target. `prior` is the DR window state read for this
/// `(target, category)` BEFORE this application (`None` if the target has never taken this category of
/// CC, or its prior window has lapsed by `now_micros`). Pure: no DB access, no logging, no side effects —
/// the caller does all the I/O and acts on the returned `ApplyDecision`.
pub(crate) fn resolve_dr(prior: Option<DrWindow>, now_micros: i64) -> ApplyDecision {
    match dr_scale_bp(dr_pre_level(prior, now_micros)) {
        Some(bp) => ApplyDecision::Apply {
            duration_scale_bp: bp,
            evict: Vec::new(),
        },
        None => ApplyDecision::Refuse { reason: "immune" },
    }
}

/// The DR level EFFECTIVE for an application at `now_micros`: the stored level IF a window is still live
/// (`now_micros <= window_expires_micros`), else 0 (no row, or the window lapsed — a fresh chain). Shared
/// by `resolve_dr` and `bump_dr_level` (so the "what level are we bumping FROM" logic
/// has exactly one definition).
pub(crate) fn dr_pre_level(prior: Option<DrWindow>, now_micros: i64) -> u8 {
    match prior {
        Some(w) if now_micros <= w.window_expires_micros => w.level,
        _ => 0,
    }
}

/// Duration scale (basis points) for a CC application at `pre_level` (the level BEFORE this hit): 0 → 100%
/// (fresh/reset), 1 → 50%, 2 → 25%, 3+ → `None` (immune — the 4th same-window hit is refused outright).
pub(crate) fn dr_scale_bp(pre_level: u8) -> Option<u32> {
    match pre_level {
        0 => Some(10_000),
        1 => Some(5_000),
        2 => Some(2_500),
        _ => None,
    }
}

/// "Strength" for a stacking-group comparison: rank first (dominant), then the effect's own magnitude as
/// the tiebreaker — per the work item's "rank first then primary-effect magnitude" rule. `magnitude` must
/// already be non-negative (callers pass `abs()`'d, stack-multiplied values — see `apply_group_conflict`).
/// NOTE (documented divergence / follow-up): this reads `rank` uniformly regardless of whether the two
/// auras being compared share a spell NAME. That's correct for a same-family group (Fortitude rank1 vs
/// rank2 — the intended use of "rank first") but would be WRONG for a genuinely cross-family
/// EXCLUSIVE_STRONGER group (armor debuffs: Sunder's rank is Sunder's-own 1..5, Expose Armor's rank is
/// Expose's-own 1..5 — the two numberings aren't comparable) IF `game_spell_chain` ever carries real rows
/// for those ids with differing ranks. Today none of Sunder/Expose/Faerie Fire exist in the curated kit
/// (no `game_spell` row → `rank_of` returns 0 for all three), so the comparison is magnitude-only in
/// practice and the vector below (the 5-stack Sunder-vs-Expose calc) is unaffected. [V] — once work-item
/// 102's real `spell_chain` data lands for these ids, gate `rank_of` on "shares the incoming spell's NAME"
/// before reusing it across a multi-family group, or add a per-group "single-family" flag.
pub(crate) fn compute_strength(rank: u8, magnitude: i32) -> i32 {
    (rank as i32) * 100_000 + magnitude.clamp(0, 99_999)
}

/// Strength with the group's comparison contract applied. Keeping this decision next to the
/// formula makes a heterogeneous group safe even after real spell-chain rows are imported.
pub(crate) fn compute_group_strength(rank_is_comparable: bool, rank: u8, magnitude: i32) -> i32 {
    compute_strength(if rank_is_comparable { rank } else { 0 }, magnitude)
}

/// `game_spell_chain.rank` for `spell_id`, or 0 if the spell has no chain row (every id outside a
/// hand-tracked rank family — the common case).
pub(crate) fn rank_of(ctx: &ReducerContext, spell_id: u32) -> u8 {
    ctx.db
        .game_spell_chain()
        .spell_id()
        .find(spell_id)
        .map(|c| c.rank)
        .unwrap_or(0)
}

/// Does `(eff_kind, eff_p0_kind, eff_p0)` name a DR-tracked CC mechanic, AND is `target_guid` a PLAYER?
/// Creatures are explicitly OUT (per the work item: "creatures take full duration, skip the PvE stun cap"
/// — this fn returning `None` for a creature is exactly that skip). The ONE mapping both the apply-side
/// (`cast::aura_apply`, before the row exists — passes the raw effect fields) and the two removal-side
/// hooks (`scheduler::tick_auras`'s expiry pass, `effects::dispel_target` — pass an existing `Aura` row's
/// frozen `eff_*` fields) go through, so "is this aura DR-tracked" has exactly one definition.
pub(crate) fn dr_category_for_effect(
    ctx: &ReducerContext,
    eff_kind: u8,
    eff_p0_kind: u8,
    eff_p0: i32,
    target_guid: u64,
) -> Option<u8> {
    if eff_kind != A_CONTROL || eff_p0_kind != P_MECHANIC {
        return None;
    }
    let is_player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .map(|t| t.is_player())
        .unwrap_or(false);
    if !is_player {
        return None;
    }
    dr_category_for_mechanic(eff_p0)
}

/// Map this crate's `M_*` mechanic const (`taxonomy.rs`) onto the cmangos-numbered DR category. `None` for
/// a mechanic with no DR category yet (there are none today — every mechanic we have maps).
const fn dr_category_for_mechanic(mechanic: i32) -> Option<u8> {
    match mechanic {
        M_STUN => Some(DR_CAT_STUN),
        M_ROOT => Some(DR_CAT_ROOT),
        M_FEAR => Some(DR_CAT_FEAR),
        M_POLY => Some(DR_CAT_POLY),
        _ => None,
    }
}

// ===========================================================================================
//  ctx-bound orchestration — the DB I/O around the two pure fns above. Called from `cast::aura_apply` (the
//  chokepoint) and the two removal paths.
// ===========================================================================================

/// Resolve + ACT on the stacking-group conflict for `spell_id` (if any) landing on `target_guid` — called
/// from `aura_apply`'s FRESH-insert branch only (a same-effect refresh never reaches this — matches "same-
/// spell-same-caster refresh runs BEFORE group logic"). `strength` is the incoming aura's own
/// `compute_strength(...)`. Returns `true` to let the fresh insert proceed (deleting every evicted aura row
/// as a side effect), `false` to have the caller bail out (refused — nothing placed, nothing touched).
pub(crate) fn apply_group_conflict(
    ctx: &ReducerContext,
    spell_id: u32,
    caster_guid: u64,
    target_guid: u64,
    incoming_rank: u8,
    incoming_magnitude: i32,
) -> bool {
    let groups = ctx.db.game_spell_group();
    let Some(group_id) = groups
        .by_spell()
        .filter(&spell_id)
        .map(|g| g.group_id)
        .next()
    else {
        return true; // not in any group — the overwhelming common case
    };
    let rule_row = ctx
        .db
        .game_spell_group_rule()
        .group_id()
        .find(group_id)
        .map(|r| (group_rule_from_u8(r.rule), r.rank_is_comparable))
        .unwrap_or((GroupRule::Stacks, true));
    let (rule, rank_is_comparable) = rule_row;
    if rule == GroupRule::Stacks {
        return true;
    }
    let member_ids: std::collections::HashSet<u32> = groups
        .by_group()
        .filter(&group_id)
        .map(|g| g.spell_id)
        .collect();
    let auras = ctx.db.game_aura();
    let members: Vec<AuraSummary> = auras
        .by_target()
        .filter(&target_guid)
        // Exclude the incoming spell's OWN spell_id: a multi-effect spell calls `aura_apply` once per
        // effect, and an earlier effect's just-inserted row must never count as a conflicting "other"
        // group member against a later effect of the SAME cast.
        .filter(|a| a.spell_id != spell_id && member_ids.contains(&a.spell_id))
        .map(|a| AuraSummary {
            aura_id: a.id,
            caster_guid: a.caster_guid,
            strength: compute_group_strength(
                rank_is_comparable,
                rank_of(ctx, a.spell_id),
                a.amount
                    .saturating_abs()
                    .saturating_mul(a.stacks.max(1) as i32),
            ),
        })
        .collect();
    let strength = compute_group_strength(rank_is_comparable, incoming_rank, incoming_magnitude);
    match resolve_group_conflict(caster_guid, strength, rule, &members) {
        ApplyDecision::Refuse { reason } => {
            log::info!(
                "stacking group {group_id}: spell {spell_id} onto {target_guid} refused ({reason})"
            );
            false
        }
        ApplyDecision::Apply { evict, .. } => {
            for id in evict {
                auras.id().delete(id);
            }
            true
        }
    }
}

/// Resolve the CC-DR decision for a `dr_category_for_effect`-tagged application landing on `target_guid`
/// at `now_micros`. Read-only (no write) — the caller applies `bump_dr_level` on `Apply`.
pub(crate) fn resolve_dr_for_target(
    ctx: &ReducerContext,
    target_guid: u64,
    category: u8,
    now_micros: i64,
) -> ApplyDecision {
    let prior = ctx
        .db
        .game_dr_state()
        .by_target()
        .filter(&target_guid)
        .find(|r| r.category == category)
        .map(|r| DrWindow {
            level: r.level,
            window_expires_micros: r.window_expires_micros,
        });
    resolve_dr(prior, now_micros)
}

/// Persist the DR level bump after a successful `Apply` from `resolve_dr_for_target` — increments the
/// level (via the SAME `dr_pre_level` the decision was made from, so the bump can never disagree with the
/// decision), stamping a PROVISIONAL `window_expires_micros` = this (already-scaled) aura's own
/// `expires_at_micros` + 15s. See the `DrState` doc comment for why this is provisional (a still-active
/// recast needs SOME in-window value to read; the removal hooks below overwrite it with the real one).
pub(crate) fn bump_dr_level(
    ctx: &ReducerContext,
    target_guid: u64,
    category: u8,
    now_micros: i64,
    expires_at_micros: i64,
) {
    let table = ctx.db.game_dr_state();
    let existing = table
        .by_target()
        .filter(&target_guid)
        .find(|r| r.category == category);
    let prior = existing.as_ref().map(|r| DrWindow {
        level: r.level,
        window_expires_micros: r.window_expires_micros,
    });
    let pre_level = dr_pre_level(prior, now_micros);
    let new_level = (pre_level + 1).min(3);
    let new_window = expires_at_micros + DR_WINDOW_MICROS;
    match existing {
        Some(mut row) => {
            row.level = new_level;
            row.window_expires_micros = new_window;
            table.id().update(row);
        }
        None => {
            table.insert(DrState {
                id: 0,
                target_guid,
                category,
                level: new_level,
                window_expires_micros: new_window,
            });
        }
    }
}

/// Stamp the AUTHORITATIVE DR window on `(target_guid, category)`'s removal — `removed_at_micros + 15s`,
/// per the work item's "window starts at removal" pin. Called from BOTH removal paths:
/// `scheduler::tick_auras`'s natural-expiry pass and `effects::dispel_target`'s dispel. No-op if no row exists
/// (can't happen in practice — a DR-tracked aura always got `bump_dr_level`'d at its own apply — but
/// defensive rather than materializing a level-0 row purely from a removal).
pub(crate) fn dr_window_on_removal(
    ctx: &ReducerContext,
    target_guid: u64,
    category: u8,
    removed_at_micros: i64,
) {
    let table = ctx.db.game_dr_state();
    if let Some(mut row) = table
        .by_target()
        .filter(&target_guid)
        .find(|r| r.category == category)
    {
        row.window_expires_micros = removed_at_micros + DR_WINDOW_MICROS;
        table.id().update(row);
    }
}

/// Reap `game_dr_state` rows whose window has long lapsed — pure bookkeeping (a lapsed window already
/// reads as level-0/fresh via `dr_pre_level`, so this is a size-bound, not a correctness fix). WIRED into
/// `gc::reap_movement_events` (module/src/gc.rs) via one bespoke call — `game_dr_state` doesn't fit the
/// file's `reap!` macro (that macro assumes an `id: u64` + `created_at: Timestamp` shape; this table's
/// time column is a plain `i64`, and its liveness test is `window_expires_micros`, not the row's own age),
/// so a bespoke call is the right shape, mirroring the teleport-event / corpse-decay bespoke blocks
/// already in that file.
pub(crate) fn sweep_dr_state(ctx: &ReducerContext) {
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let table = ctx.db.game_dr_state();
    let stale: Vec<u64> = table
        .iter()
        .filter(|r| r.window_expires_micros < now_micros)
        .map(|r| r.id)
        .collect();
    for id in stale {
        table.id().delete(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Stacking groups -----------------------------------------------------------------------

    /// Fortitude rank cross-caster (verbatim work-item vector): rank2(caster A) onto a target already
    /// carrying rank1(caster B) — EXCLUSIVE_STRONGER → Apply, evicting B's aura. The reverse (rank1 onto an
    /// existing rank2) is weaker → Refuse.
    #[test]
    fn fortitude_rank_beats_lower_rank_any_caster_exclusive_stronger() {
        let rank1_b = AuraSummary {
            aura_id: 42,
            caster_guid: 200,
            strength: compute_strength(1, 20),
        };
        assert_eq!(
            resolve_group_conflict(
                100,
                compute_strength(2, 34),
                GroupRule::ExclusiveStronger,
                &[rank1_b],
            ),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![42]
            }
        );

        // Reverse: rank1(A) attempted while target carries rank2(B) → weaker → Refuse.
        let rank2_b = AuraSummary {
            aura_id: 43,
            caster_guid: 200,
            strength: compute_strength(2, 34),
        };
        assert_eq!(
            resolve_group_conflict(
                100,
                compute_strength(1, 20),
                GroupRule::ExclusiveStronger,
                &[rank2_b],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );
    }

    /// Blessings (EXCLUSIVE_PER_CASTER, verbatim vector): Blessing of Might(A) + Blessing of Wisdom(A) on
    /// the same target — the second REPLACES the first (same caster). Might(A) + Wisdom(B) — both stand
    /// (different casters never conflict under this rule).
    #[test]
    fn blessings_are_exclusive_per_caster_not_across_casters() {
        let might_a = AuraSummary {
            aura_id: 1,
            caster_guid: 500,
            strength: 0,
        };
        // Wisdom cast by the SAME caster (500) — replaces Might.
        assert_eq!(
            resolve_group_conflict(500, 0, GroupRule::ExclusivePerCaster, &[might_a]),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![1]
            }
        );
        // Wisdom cast by a DIFFERENT caster (600) — both stand, nothing evicted.
        assert_eq!(
            resolve_group_conflict(600, 0, GroupRule::ExclusivePerCaster, &[might_a]),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
    }

    /// Armor debuffs (EXCLUSIVE_STRONGER, verbatim vector, including the 5-STACK strength calc): Sunder
    /// Armor (450/stack, real vanilla rank-5 value) at a single fresh stack (450) onto an existing Expose
    /// Armor (real vanilla flat 2050) is WEAKER → Refuse. Expose (2050) onto an existing Sunder at its
    /// hard 5-stack cap (450 × 5 = 2250 — the "5-stack strength calc") is ALSO weaker (2250 > 2050) →
    /// Refuse. Neither displaces the other at these real values (armor debuffs aren't in the curated kit
    /// today, so `rank_of` is 0 for both — a pure magnitude comparison, matching the vector).
    #[test]
    fn armor_debuffs_exclusive_stronger_compares_magnitude_including_5_stack_sunder() {
        const SUNDER_PER_STACK: i32 = 450; // real vanilla Sunder Armor rank-5 per-application armor reduction
        const EXPOSE_FLAT: i32 = 2050; // real vanilla Expose Armor flat armor reduction

        // Sunder (fresh, 1 stack) onto an existing Expose: Expose (2050) is stronger → Refuse.
        let expose_existing = AuraSummary {
            aura_id: 7,
            caster_guid: 1,
            strength: compute_strength(0, EXPOSE_FLAT),
        };
        assert_eq!(
            resolve_group_conflict(
                2,
                compute_strength(0, SUNDER_PER_STACK),
                GroupRule::ExclusiveStronger,
                &[expose_existing],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );

        // Expose onto an existing Sunder AT ITS 5-STACK CAP: 450 * 5 = 2250 > Expose's 2050 → Refuse.
        let sunder_5_stack_strength = compute_strength(0, SUNDER_PER_STACK * 5);
        assert_eq!(sunder_5_stack_strength, 2250);
        let sunder_existing = AuraSummary {
            aura_id: 8,
            caster_guid: 2,
            strength: sunder_5_stack_strength,
        };
        assert_eq!(
            resolve_group_conflict(
                1,
                compute_strength(0, EXPOSE_FLAT),
                GroupRule::ExclusiveStronger,
                &[sunder_existing],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );
    }

    /// A plain STACKS-rule group is a complete no-op: full duration, nothing evicted, regardless of what
    /// else is on the target. (The OTHER "no-op" case — a spell not in any group at all — no longer has a
    /// pure-fn equivalent to pin: `apply_group_conflict` now returns `true` directly without ever calling
    /// `resolve_group_conflict`, so there is nothing here to unit-test beyond that caller-side short
    /// circuit.)
    #[test]
    fn stacks_rule_is_a_full_duration_noop() {
        let other = AuraSummary {
            aura_id: 99,
            caster_guid: 1,
            strength: 1_000_000,
        };
        assert_eq!(
            resolve_group_conflict(2, 0, GroupRule::Stacks, &[other]),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
    }

    // --- CC diminishing returns ------------------------------------------------------------------

    /// The verbatim poly DR timeline (t in seconds -> micros): t=0 full (fresh); t=12 (aura #1 expired at
    /// t=10, its removal set the window to expire at t=25) -> 50%, level->2; t=16 (aura #2, still active,
    /// hasn't been removed yet — its own apply-time level bump already put the row at level 2) -> 25%,
    /// level->3; t=20 (aura #3 removed at t=18.5, its removal reset the window to expire at t=33.5, still
    /// live) -> Refuse{immune}; t=40.1 (> t=33.5, window lapsed) -> full duration, level reset.
    #[test]
    // Every timestamp in this vector is written `<n> * SECOND` so the timeline reads off the page in
    // the same units as the doc comment above; `0 * SECOND` is that convention at t=0, not a bug.
    #[allow(clippy::erasing_op)]
    fn poly_dr_timeline_matches_the_verbatim_vector() {
        const SECOND: i64 = 1_000_000;

        // t=0: no prior row -> full duration.
        let t0 = 0 * SECOND;
        let decision0 = resolve_dr(None, t0);
        assert_eq!(
            decision0,
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
        // bump_dr_level's own math (mirrored here, no ctx): pre_level 0 -> new_level 1.
        assert_eq!(dr_pre_level(None, t0), 0);

        // Aura #1 (full 10s) removed at t=10 (natural expiry) -> window row: level 1, expires t=10+15=25.
        let row_after_removal_1 = DrWindow {
            level: 1,
            window_expires_micros: 25 * SECOND,
        };

        // t=12: within window (12 <= 25) at level 1 -> 50%, i.e. 5s of a 10s base.
        let t12 = 12 * SECOND;
        let decision12 = resolve_dr(Some(row_after_removal_1), t12);
        assert_eq!(
            decision12,
            ApplyDecision::Apply {
                duration_scale_bp: 5_000,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(Some(row_after_removal_1), t12), 1);
        // 10s base * 50% = 5s -> expires at t=17. Not removed yet by t=16 (still active): the apply-time
        // bump already advanced the row to level 2 (this models what `bump_dr_level` writes).
        let row_after_apply_2 = DrWindow {
            level: 2,
            window_expires_micros: 17 * SECOND + DR_WINDOW_MICROS,
        };

        // t=16: aura #2 still active (expires t=17) — a recast reads the row AS-IS (no removal happened),
        // level 2 -> 25%, i.e. 2.5s of a 10s base.
        let t16 = 16 * SECOND;
        let decision16 = resolve_dr(Some(row_after_apply_2), t16);
        assert_eq!(
            decision16,
            ApplyDecision::Apply {
                duration_scale_bp: 2_500,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(Some(row_after_apply_2), t16), 2);
        // 10s * 25% = 2.5s -> expires at t=18.5. THIS TIME it naturally expires (removed) at t=18.5,
        // stamping the AUTHORITATIVE window: level 3 (unchanged at removal), expires t=18.5+15=33.5.
        let row_after_removal_3 = DrWindow {
            level: 3,
            window_expires_micros: (18 * SECOND + SECOND / 2) + DR_WINDOW_MICROS,
        };
        assert_eq!(row_after_removal_3.window_expires_micros, 335 * SECOND / 10); // t=33.5s

        // t=20: within window (20 <= 33.5) at level 3 -> immune -> Refuse.
        let t20 = 20 * SECOND;
        let decision20 = resolve_dr(Some(row_after_removal_3), t20);
        assert_eq!(decision20, ApplyDecision::Refuse { reason: "immune" });
        // Refused: the row is untouched (no bump on a Refuse) — level stays 3, window stays 33.5s.
        assert_eq!(dr_pre_level(Some(row_after_removal_3), t20), 3);

        // t=40.1: past the window (40.1 > 33.5) -> lapsed -> full duration, level effectively reset to 0
        // (the next `bump_dr_level` would store level 1, a fresh chain).
        let t40_1 = 40 * SECOND + SECOND / 10;
        let decision40 = resolve_dr(Some(row_after_removal_3), t40_1);
        assert_eq!(
            decision40,
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(Some(row_after_removal_3), t40_1), 0);
    }

    /// Same double-poly scenario against a CREATURE target: `dr_category_for_effect` returns `None` for a
    /// non-player target (checked at the call site, before `resolve_dr_for_target` is ever invoked), so
    /// `aura_apply` never calls `resolve_dr` at all for a creature — modeled here as a `None` prior with no
    /// window ever recorded, which always resolves to full duration regardless of `now`.
    #[test]
    fn creature_target_skips_dr_entirely_full_duration_both_casts() {
        // "Both casts" — calling twice (as if back-to-back on the creature) yields full duration each time,
        // since there is no DR bookkeeping in play for a creature target at all.
        assert_eq!(
            resolve_dr(None, 0),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
        assert_eq!(
            resolve_dr(None, 999_999_999),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
    }

    /// `dr_scale_bp` / `dr_pre_level` in isolation: the 100/50/25/immune ladder + the window-lapse reset,
    /// pinned as a small pure table (the building blocks the timeline test above composes).
    #[test]
    fn dr_scale_ladder_and_window_lapse_reset() {
        assert_eq!(dr_scale_bp(0), Some(10_000));
        assert_eq!(dr_scale_bp(1), Some(5_000));
        assert_eq!(dr_scale_bp(2), Some(2_500));
        assert_eq!(dr_scale_bp(3), None);
        assert_eq!(dr_scale_bp(4), None); // defensive: anything >=3 is immune
                                          // No row -> pre_level 0 regardless of `now`.
        assert_eq!(dr_pre_level(None, 0), 0);
        assert_eq!(dr_pre_level(None, 1_000_000_000), 0);
        // A live window (now == the boundary tick) still counts as in-window (`<=`).
        let w = DrWindow {
            level: 2,
            window_expires_micros: 100,
        };
        assert_eq!(dr_pre_level(Some(w), 100), 2);
        // One micro past the boundary -> lapsed -> 0.
        assert_eq!(dr_pre_level(Some(w), 101), 0);
    }

    /// `dr_category_for_mechanic` maps every mechanic this crate has today; an unmapped mechanic (or the
    /// inert 0 sentinel) is `None`.
    #[test]
    fn dr_category_mapping_covers_every_current_mechanic() {
        assert_eq!(dr_category_for_mechanic(M_STUN), Some(DR_CAT_STUN));
        assert_eq!(dr_category_for_mechanic(M_ROOT), Some(DR_CAT_ROOT));
        assert_eq!(dr_category_for_mechanic(M_FEAR), Some(DR_CAT_FEAR));
        assert_eq!(dr_category_for_mechanic(M_POLY), Some(DR_CAT_POLY));
        assert_eq!(dr_category_for_mechanic(0), None);
        assert_eq!(dr_category_for_mechanic(999), None);
    }

    /// `compute_strength`: rank dominates when ranks differ; magnitude (clamped to [0, 99_999]) is the
    /// tiebreaker within equal rank. A negative-magnitude input (never passed by a caller today — callers
    /// abs() first — but exercised here for the clamp floor) never goes negative.
    #[test]
    fn compute_strength_ranks_first_then_magnitude() {
        assert!(compute_strength(2, 1) > compute_strength(1, 99_999)); // rank 2 beats rank 1's max magnitude
        assert_eq!(compute_strength(0, 100), compute_strength(0, 100));
        assert!(compute_strength(0, 200) > compute_strength(0, 100)); // equal rank -> magnitude decides
        assert_eq!(compute_strength(0, -50), 0); // clamp floor
    }

    #[test]
    fn heterogeneous_groups_ignore_unrelated_spell_chain_ranks() {
        // A rank-five Sunder is not intrinsically stronger than rank-one Expose Armor. In this
        // group their effective armor reduction alone decides the outcome.
        assert!(compute_group_strength(false, 1, 2_050) > compute_group_strength(false, 5, 450));
        // Homogeneous rank families preserve rank-first comparison.
        assert!(compute_group_strength(true, 2, 1) > compute_group_strength(true, 1, 99_999));
    }

    #[test]
    fn every_aura_entry_point_converges_on_the_authoritative_insertion_boundary() {
        let targeting = include_str!("cast/targeting.rs");
        let aura_apply = crate::test_scan::code_of(targeting, "pub(crate) fn aura_apply(");
        assert_eq!(
            aura_apply.matches("auras.insert(Aura {").count(),
            1,
            "aura_apply must retain exactly one game_aura insertion site"
        );
        assert!(aura_apply.contains("apply_group_conflict("));
        assert!(aura_apply.contains("resolve_dr_for_target("));
        assert!(aura_apply.contains("pick_aura_slot("));

        fn rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("spell source directory is readable") {
                let path = entry.expect("spell source entry is readable").path();
                if path.is_dir() {
                    rust_sources(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let mut sources = Vec::new();
        rust_sources(
            &crate::test_scan::repo_root().join("module/src/spell"),
            &mut sources,
        );
        let insertions: Vec<_> = sources
            .iter()
            .filter_map(|path| {
                // This test's own assertion contains the needle as a string literal; it is not a
                // production aura path, so exclude the file containing the tripwire itself.
                if path.file_name().is_some_and(|name| name == "stacking.rs") {
                    return None;
                }
                let source = std::fs::read_to_string(path).expect("spell source is readable");
                source
                    .contains("auras.insert(Aura {")
                    .then(|| path.display().to_string())
            })
            .collect();
        assert_eq!(
            insertions.len(),
            1,
            "a second game_aura insertion path bypasses aura_apply: {insertions:?}"
        );

        // Normal casts and linked effects share `apply_effect`; passive/talent/racial application
        // calls the same boundary from `apply_spell_auras`.
        assert!(targeting.contains("aura_apply(\n            ctx,"));
        let resolve = include_str!("cast/resolve.rs");
        let passive = crate::test_scan::code_of(resolve, "pub(crate) fn apply_spell_auras(");
        assert!(passive.contains("aura_apply("));
    }

    /// `group_rule_from_u8` decodes the four documented rule bytes; an unrecognized byte is the safe
    /// `Stacks` no-op fallback (never silently refuses/evicts on a data bug).
    #[test]
    fn group_rule_decode_and_safe_fallback() {
        assert_eq!(group_rule_from_u8(RULE_STACKS), GroupRule::Stacks);
        assert_eq!(group_rule_from_u8(RULE_EXCLUSIVE), GroupRule::Exclusive);
        assert_eq!(
            group_rule_from_u8(RULE_EXCLUSIVE_PER_CASTER),
            GroupRule::ExclusivePerCaster
        );
        assert_eq!(
            group_rule_from_u8(RULE_EXCLUSIVE_STRONGER),
            GroupRule::ExclusiveStronger
        );
        assert_eq!(group_rule_from_u8(255), GroupRule::Stacks);
    }
}
