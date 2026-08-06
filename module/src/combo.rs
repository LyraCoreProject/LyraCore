//! Combo points — the Rogue resource. A per-(owner, target) pool (0..=MAX_COMBO) that GENERATORS
//! (an `E_ADD_COMBO` effect — Sinister Strike / Gouge / Backstab) build on the target, and FINISHERS
//! (`E_FINISHER_DAMAGE` — Eviscerate) read to scale their output, then spend. STATE only: the spell
//! effect KINDS in `spell/cast.rs` operate it generically (no per-spell code; the importer's curated
//! correction tags the generator/finisher effects by name). Mirrors the `game_threat` table shape.
//! Per-target by design — vanilla combo points live on the rogue's CURRENT target. [entity]

use spacetimedb::{table, ReducerContext, Table};

/// Vanilla combo-point cap.
pub const MAX_COMBO: u32 = 5;

/// One (owner → target) combo-point pool. UPSERT by (owner, target); the small per-owner list is
/// scanned via `by_owner` exactly like the threat table's `by_creature`.
#[table(accessor = game_combo_point, public, index(accessor = by_owner, btree(columns = [owner_guid])))]
pub struct ComboPoint {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub owner_guid: u64,
    pub target_guid: u64,
    pub points: u32,
}

/// One generator tick: bump, capped at `MAX_COMBO`. Pure → unit-tested.
pub fn capped_inc(points: u32) -> u32 {
    (points + 1).min(MAX_COMBO)
}

/// Finisher damage: the per-point base × the combo points spent. Pure → unit-tested.
pub fn finisher_damage(per_point: i32, combo: u32) -> i32 {
    per_point.saturating_mul(combo as i32)
}

/// Slice-and-Dice-style combo-FINISHER duration: the spell header's base duration + 3s per combo point
/// (combo capped at `MAX_COMBO`). The +3000ms-per-point curve lives HERE (one named, tested place); the
/// base comes from the DBC header (`hdr.duration_ms`, kept honest at 6000 for SnD). Vanilla SnD:
///   0→6000, 1→9000, 2→12000, 3→15000, 4→18000, 5→21000 (= 6 + 3·combo seconds).
/// 0 combo → base (no scale) — the spend then deletes nothing, so a 0-combo cast is a harmless base-duration
/// buff. Pure → unit-tested.
pub fn finisher_duration_ms(base_ms: u32, combo: u32) -> u32 {
    base_ms + combo.min(MAX_COMBO) * 3_000
}

fn find_row(ctx: &ReducerContext, owner_guid: u64, target_guid: u64) -> Option<ComboPoint> {
    ctx.db
        .game_combo_point()
        .by_owner()
        .filter(&owner_guid)
        .find(|r| r.target_guid == target_guid)
}

/// Combo points `owner` has built on `target` (0 if none).
pub fn combo_points(ctx: &ReducerContext, owner_guid: u64, target_guid: u64) -> u32 {
    find_row(ctx, owner_guid, target_guid)
        .map(|r| r.points)
        .unwrap_or(0)
}

/// Generator effect: +1 combo point on (owner, target), capped at `MAX_COMBO`.
pub fn add_combo_point(ctx: &ReducerContext, owner_guid: u64, target_guid: u64) {
    let combos = ctx.db.game_combo_point();
    if let Some(mut r) = find_row(ctx, owner_guid, target_guid) {
        r.points = capped_inc(r.points);
        combos.id().update(r);
    } else {
        combos.insert(ComboPoint {
            id: 0,
            owner_guid,
            target_guid,
            points: 1,
        });
    }
}

/// Finisher consume: drop (owner, target) back to empty (deletes the row).
pub fn spend_combo(ctx: &ReducerContext, owner_guid: u64, target_guid: u64) {
    let combos = ctx.db.game_combo_point();
    if let Some(r) = find_row(ctx, owner_guid, target_guid) {
        combos.id().delete(r.id);
    }
}

#[cfg(test)]
mod tests {
    use super::capped_inc;

    #[test]
    fn combo_caps_at_five() {
        let mut p = 0;
        for _ in 0..8 {
            p = capped_inc(p);
        }
        assert_eq!(p, super::MAX_COMBO);
    }

    #[test]
    fn finisher_scales_with_points() {
        assert_eq!(super::finisher_damage(5, 3), 15); // 5 per point × 3 combo
        assert_eq!(super::finisher_damage(5, 0), 0); // no combo → no damage
    }

    #[test]
    fn finisher_duration_follows_the_vanilla_snd_curve() {
        // Slice and Dice: header base 6000ms (1 combo point in the DBC) + 3s per combo point.
        // Vanilla 1..5 combo → 9/12/15/18/21s.
        assert_eq!(super::finisher_duration_ms(6000, 1), 9_000);
        assert_eq!(super::finisher_duration_ms(6000, 2), 12_000);
        assert_eq!(super::finisher_duration_ms(6000, 3), 15_000);
        assert_eq!(super::finisher_duration_ms(6000, 4), 18_000);
        assert_eq!(super::finisher_duration_ms(6000, 5), 21_000);
        // 0 combo → base (no scale), and combo above the cap saturates at the 5-point duration.
        assert_eq!(super::finisher_duration_ms(6000, 0), 6_000);
        assert_eq!(super::finisher_duration_ms(6000, 9), 21_000);
    }
}
