//! Land mounts — the ground-mount state model. A mount is an ordinary cancelable self aura: the
//! `A_MOUNTED` aura row IS the mounted state, and `WorldEntity.mount_display_id` is a PROJECTION of
//! that aura set for the client's `UNIT_FIELD_MOUNTDISPLAYID`, never a second state machine.
//!
//! The module has no single aura-deletion boundary — auras are removed by `do_cancel_aura`, the
//! expiry reap, dispel and the spellbook unlearn path. Rather than teach each of those to undo a
//! mount, every one of them converges on [`recompute_mount`], which RE-DERIVES the projection from
//! whatever aura rows remain. That is the same collect-a-predicate-then-recompute shape the crate
//! already uses for `aura_moves_vitals` + `recompute_vitals`, and it is what makes every dismount
//! trigger idempotent by construction: running it twice, or on an unmounted player, converges on the
//! same state and writes nothing the second time.
//!
//! TAXI BOUNDARY: a taxi flight owns `mount_display_id` for its whole duration (`taxi.rs`), and the
//! two mechanics share one field. Both entry points here refuse to touch a player in flight. [entity]

use spacetimedb::ReducerContext;

use crate::game_aura;
use crate::game_world_entity;
use crate::spell::{A_MOD_SPEED, A_MOUNTED, SPEED_MOUNTED};

/// Can applying or removing this aura effect move the mount projection? True for the mount state
/// itself, and for the mounted-speed effect that rides alongside it (the speed fold reads that aura
/// only while an `A_MOUNTED` aura is present, so gaining or losing it is also a mount-projection
/// event). Every aura-removal site collects this per deleted row and calls [`recompute_mount`] once
/// afterwards. Pure — the two-argument shape mirrors `aura_moves_vitals(kind, p0)`.
pub(crate) fn mount_aura_moves_mount(eff_kind: u8, eff_p0: i32) -> bool {
    eff_kind == A_MOUNTED || (eff_kind == A_MOD_SPEED && eff_p0 == SPEED_MOUNTED as i32)
}

/// The mount display an aura set projects: the display frozen on an `A_MOUNTED` aura's `p0`, or 0
/// when the set holds none. A negative or absent value reads as 0 (unmounted) so unauthored data can
/// never write a nonsense display. Pure over `(eff_kind, eff_p0)` pairs, so the projection rule is
/// unit-testable without a reducer context.
pub(crate) fn projected_mount_display(effects: impl IntoIterator<Item = (u8, i32)>) -> u32 {
    effects
        .into_iter()
        .find(|(kind, _)| *kind == A_MOUNTED)
        .map(|(_, p0)| p0.max(0) as u32)
        .unwrap_or(0)
}

/// The spell id of `guid`'s active land mount, or `None` when it is not mounted. The one test callers
/// should use — mounted state lives in the aura set, not in `mount_display_id`.
pub(crate) fn active_mount_spell(ctx: &ReducerContext, guid: u64) -> Option<u32> {
    ctx.db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|a| a.eff_kind == A_MOUNTED)
        .min_by_key(|a| a.id)
        .map(|a| a.spell_id)
}

/// Re-derive `guid`'s `mount_display_id` AND relayed run speed from its current aura rows. Zero
/// `A_MOUNTED` auras clears the display; one restores the value frozen on it. The speed side reuses
/// `combat::effective_move_speed` at base 1.0 (SPEED_MOVE always, SPEED_MOUNTED only while mounted), so
/// `run_speed_mult_bp` carries the exact multiplier — 16 000 for a 60% mount, 10 000 for an unmounted
/// rider with no other speed aura — and the existing `run_speed_mult_bp` subscription diff (shared with
/// GM `.speed`) turns that into `SMSG_FORCE_RUN_SPEED_CHANGE` with no gateway change needed. NEVER a
/// delta — that is what makes every caller idempotent. Both fields are read-modify-written TOGETHER and
/// the row is only written when at least one actually changed, because the gateway's subscription diff
/// fires on every write and a redundant write would relay a spurious VALUES / force-run-speed update.
///
/// A player in a taxi flight is left completely alone: `taxi.rs` owns the shared display field for the
/// duration of the flight and clears it itself on landing.
pub(crate) fn recompute_mount(ctx: &ReducerContext, guid: u64) {
    if crate::taxi::is_in_flight(ctx, guid) {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(guid) else {
        return;
    };
    let display = projected_mount_display(
        ctx.db
            .game_aura()
            .by_target()
            .filter(&guid)
            .map(|a| (a.eff_kind, a.eff_p0)),
    );
    let speed_bp = (crate::combat::effective_move_speed(ctx, guid, 1.0) * 10_000.0).round() as u32;
    if e.mount_display_id == display && e.run_speed_mult_bp == speed_bp {
        return;
    }
    e.mount_display_id = display;
    e.run_speed_mult_bp = speed_bp;
    entities.guid().update(e);
}

/// The one shared land-dismount operation. Removes EVERY aura row belonging to the active mount spell
/// — so the paired mounted-speed effect goes with the mount itself — then recomputes the projection.
/// A no-op (and never an error) when `guid` is not mounted, so an action chokepoint can call it
/// unconditionally, and calling it twice changes nothing the second time.
///
/// Every dismount trigger routes here: `E_DISMOUNT` resolution, mount replacement, and the accepted
/// action / indoor-transition hooks. Manual `CMSG_CANCEL_AURA` and natural expiry reach the same end
/// state through ordinary aura removal plus [`recompute_mount`].
pub(crate) fn dismount(ctx: &ReducerContext, guid: u64) {
    if crate::taxi::is_in_flight(ctx, guid) {
        return;
    }
    let Some(spell_id) = active_mount_spell(ctx, guid) else {
        return;
    };
    let auras = ctx.db.game_aura();
    let ids: Vec<u64> = auras
        .by_target()
        .filter(&guid)
        .filter(|a| a.spell_id == spell_id)
        .map(|a| a.id)
        .collect();
    for id in ids {
        auras.id().delete(id);
    }
    recompute_mount(ctx, guid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::{
        A_MOD_SPEED, A_MOD_STAT, A_MOUNTED, A_PERIODIC_DAMAGE, SPEED_MOUNTED, SPEED_MOVE,
        SPEED_SWING,
    };

    /// The removal-convergence predicate every aura-deletion site collects. Only the mount state and
    /// its paired mounted-speed effect move the projection; an ordinary buff, a DoT, or a NON-mounted
    /// speed aura (a snare, a haste) must not drag a recompute onto every unrelated aura removal.
    #[test]
    fn mount_aura_moves_mount_covers_the_mount_pair_only() {
        assert!(mount_aura_moves_mount(A_MOUNTED, 0));
        assert!(mount_aura_moves_mount(A_MOUNTED, 1147));
        assert!(mount_aura_moves_mount(A_MOD_SPEED, SPEED_MOUNTED as i32));

        assert!(!mount_aura_moves_mount(A_MOD_SPEED, SPEED_MOVE as i32));
        assert!(!mount_aura_moves_mount(A_MOD_SPEED, SPEED_SWING as i32));
        assert!(!mount_aura_moves_mount(A_MOD_STAT, SPEED_MOUNTED as i32));
        assert!(!mount_aura_moves_mount(A_PERIODIC_DAMAGE, 0));
    }

    /// The projection rule: the display comes off the `A_MOUNTED` aura's frozen `p0`, and an aura set
    /// without one projects 0. Unauthored (negative) data reads as unmounted rather than as a garbage
    /// display id.
    #[test]
    fn projected_mount_display_reads_the_mount_aura_only() {
        assert_eq!(projected_mount_display([]), 0);
        assert_eq!(
            projected_mount_display([(A_MOD_STAT, 5), (A_PERIODIC_DAMAGE, 12)]),
            0
        );
        assert_eq!(
            projected_mount_display([(A_MOD_SPEED, SPEED_MOUNTED as i32), (A_MOUNTED, 1147)]),
            1147
        );
        assert_eq!(projected_mount_display([(A_MOUNTED, -3)]), 0);
    }

    /// Idempotence, at the level the recompute actually decides it: the projection is a pure function
    /// of the aura set, so re-deriving it from an unchanged set yields the same value — which is why
    /// `recompute_mount` can skip the write and why a second `dismount` changes nothing. Removing the
    /// mount spell's rows (both effects together, as `dismount` does) converges on 0.
    #[test]
    fn projection_is_stable_and_converges_on_removal() {
        let mounted = [(A_MOUNTED, 1147), (A_MOD_SPEED, SPEED_MOUNTED as i32)];
        assert_eq!(projected_mount_display(mounted), 1147);
        assert_eq!(projected_mount_display(mounted), 1147);

        let after_dismount: [(u8, i32); 0] = [];
        assert_eq!(projected_mount_display(after_dismount), 0);
        assert_eq!(projected_mount_display(after_dismount), 0);
    }

    /// Mount replacement (story 28): applying a second mount dismounts the first, so the aura set is
    /// never allowed to hold two `A_MOUNTED` rows. Should one appear anyway, the projection stays
    /// deterministic rather than flickering between displays.
    #[test]
    fn projected_mount_display_is_deterministic_with_a_stray_second_mount() {
        assert_eq!(
            projected_mount_display([(A_MOUNTED, 1147), (A_MOUNTED, 2404)]),
            1147
        );
    }

    /// Both ctx-bound entry points refuse a player in a taxi flight BEFORE reading or writing
    /// anything — `taxi.rs` owns `mount_display_id` for the whole flight, and the two mechanics share
    /// the field. A scan tripwire because the guard is the first statement of a reducer-bound fn and
    /// there is no `ReducerContext` harness in this crate.
    #[test]
    fn taxi_flight_is_guarded_before_any_mount_write() {
        for signature in ["pub(crate) fn recompute_mount(", "pub(crate) fn dismount("] {
            let body = crate::test_scan::shape_of(include_str!("mount.rs"), signature);
            assert!(
                body.starts_with("{ if crate::taxi::is_in_flight(ctx, guid) { return; }"),
                "`{signature}` must refuse a taxi passenger before touching mount state — the taxi \
                 flight owns `mount_display_id` and its own aura-free presentation. Body was:\n{body}"
            );
        }
    }

    /// The speed half of the recompute: `run_speed_mult_bp` is derived from the SAME
    /// `combat::effective_move_speed` fold the anti-cheat movement check reads (base 1.0, so the field
    /// carries the exact multiplier), in the SAME read-modify-write as the display — so a mount/dismount
    /// relays through the EXISTING `run_speed_mult_bp` subscription diff with no gateway change, and
    /// never writes when NEITHER field actually changed. Scan-pinned like the taxi guard above; the
    /// FOLD's sense (mounted vs not, base-plus-buff) is asserted directly on `combat`'s
    /// `mounted_move_pct` pure core, not re-derived here.
    #[test]
    fn recompute_mount_folds_effective_speed_into_run_speed_mult_bp() {
        let body =
            crate::test_scan::code_of(include_str!("mount.rs"), "pub(crate) fn recompute_mount(");
        assert!(
            body.contains("crate::combat::effective_move_speed(ctx, guid, 1.0)"),
            "`recompute_mount` must derive `run_speed_mult_bp` from the SAME mounted-speed fold the \
             anti-cheat movement check reads, or the wire relay and the server's own speed check can \
             disagree. Body was:\n{body}"
        );
        assert!(
            body.contains("e.mount_display_id == display && e.run_speed_mult_bp == speed_bp"),
            "`recompute_mount` must skip the write when NEITHER the display NOR the speed changed, or \
             a redundant write relays a spurious VALUES / force-run-speed update. Body was:\n{body}"
        );
    }

    /// `dismount` drops the WHOLE mount spell, not just the `A_MOUNTED` row, so the paired
    /// mounted-speed effect can never survive as an orphaned buff — then converges on the recompute
    /// rather than clearing the field by hand.
    #[test]
    fn dismount_removes_the_whole_mount_spell_then_recomputes() {
        let body = crate::test_scan::shape_of(include_str!("mount.rs"), "pub(crate) fn dismount(");
        assert!(
            body.contains("a.spell_id == spell_id"),
            "`dismount` must delete every aura row of the active mount spell, or the mounted-speed \
             effect outlives the mount. Body was:\n{body}"
        );
        assert!(
            body.contains("recompute_mount(ctx, guid);"),
            "`dismount` must converge on `recompute_mount` instead of writing the projection \
             directly. Body was:\n{body}"
        );
    }

    /// The end-to-end cancel seam (story 24): `CMSG_CANCEL_AURA` lands on `gw_cancel_aura`, which
    /// calls `do_cancel_aura`, which collects [`mount_aura_moves_mount`] alongside the vitals/sheet
    /// predicates and recomputes the projection after the deletes. There is no `ReducerContext`
    /// harness in this crate, so the chain's PRESENCE is pinned by scan; the SENSE of the recompute
    /// is asserted directly on [`projected_mount_display`] above.
    #[test]
    fn cancel_aura_reaches_the_mount_recompute() {
        let gw = crate::test_scan::code_of(include_str!("gw.rs"), "pub fn gw_cancel_aura(");
        assert!(
            gw.contains("crate::spell::do_cancel_aura(ctx, actor_guid, spell_id)"),
            "`gw_cancel_aura` no longer routes to the cancel core. Body was:\n{gw}"
        );
        let cancel = crate::test_scan::code_of(
            include_str!("spell/scheduler.rs"),
            "pub(crate) fn do_cancel_aura(",
        );
        assert!(
            cancel.contains("crate::mount::mount_aura_moves_mount(a.eff_kind, a.eff_p0)")
                && cancel.contains("crate::mount::recompute_mount(ctx, player_guid);"),
            "`do_cancel_aura` must collect the mount predicate while deleting and recompute the \
             projection afterwards, or right-clicking the mount buff off leaves a stuck model. \
             Body was:\n{cancel}"
        );
    }

    /// The apply half: `aura_apply` is the single `game_aura` insertion boundary, so a mount becomes
    /// active exactly there. It dismounts a DIFFERENT active mount first (story 28) and recomputes
    /// after the row is placed. `E_DISMOUNT` resolves through the same shared operation.
    #[test]
    fn apply_and_dismount_effect_use_the_shared_operation() {
        let targeting = include_str!("spell/cast/targeting.rs");
        let apply = crate::test_scan::code_of(targeting, "pub(crate) fn aura_apply(");
        assert!(
            apply.contains("crate::mount::dismount(ctx, target_guid);"),
            "`aura_apply` must dismount a prior land mount before placing a new one. Body was:\n{apply}"
        );
        assert!(
            apply.contains("crate::mount::recompute_mount(ctx, target_guid);"),
            "`aura_apply` must recompute the mount projection after placing the aura, or the frozen \
             display never reaches the client. Body was:\n{apply}"
        );
        assert!(
            targeting.contains(
                "crate::mount::dismount(ctx, target_guid);\n            EffectHit::none()"
            ),
            "the `E_DISMOUNT` arm must call the one shared dismount operation"
        );
    }

    /// Removal convergence, the whole point of this module: the crate has no single aura-DELETION
    /// boundary, so this pins the complete census of `game_aura` deletion sites. Each file below is
    /// either wired to the mount recompute or cannot remove a mount aura at all, with the reason.
    /// A new deletion path changes this count and forces that decision to be made explicitly rather
    /// than silently leaving a stuck mount display behind.
    #[test]
    fn every_aura_deletion_site_is_accounted_for() {
        // (file, deletions, why it is safe)
        const CENSUS: &[(&str, usize, &str)] = &[
            (
                "module/src/mount.rs",
                1,
                "dismount — the shared operation itself; recomputes",
            ),
            (
                "module/src/spell/cast/targeting.rs",
                1,
                "displace_auras — called only from aura_apply, which recomputes after the insert",
            ),
            (
                "module/src/spell/control.rs",
                2,
                "break_auras_on_damage (ordinary damage must NOT dismount — the mount's DBC \
                 interrupt bit is underwater-cancel, not break-on-damage) and break_channel \
                 (A_PERIODIC_TRIGGER rows only)",
            ),
            (
                "module/src/spell/effects.rs",
                2,
                "dispel_target — wired; absorb_incoming — deletes the spent A_ABSORB row only",
            ),
            (
                "module/src/spell/math.rs",
                2,
                "remove_seal_auras and break_stealth — both scoped to their own aura kind",
            ),
            (
                "module/src/spell/scheduler.rs",
                3,
                "do_cancel_aura and the expiry reap — both wired; the channel-end pass deletes \
                 A_PERIODIC_TRIGGER rows only",
            ),
            (
                "module/src/spell/spellbook.rs",
                1,
                "strip_spell_auras — wired",
            ),
            (
                "module/src/spell/stacking.rs",
                1,
                "apply_group_conflict — evicts only members of the incoming spell's own stacking \
                 group; no group contains a mount spell",
            ),
            (
                "module/src/spell/tables.rs",
                1,
                "sweep_delete_game_aura — the character-deletion sweep; the entity goes with it",
            ),
        ];

        let mut found: Vec<(String, usize)> = Vec::new();
        for path in module_sources() {
            let source = std::fs::read_to_string(&path).expect("module source is readable");
            let deletes = crate::test_scan::raw_table_reads(&source, &["game_aura"], |c, idx| {
                c[idx..].trim_start().starts_with(".id().delete(")
            })
            .len();
            if deletes > 0 {
                found.push((rel(&path), deletes));
            }
        }
        let expected: Vec<(String, usize)> = CENSUS
            .iter()
            .map(|(file, n, _)| ((*file).to_string(), *n))
            .collect();
        assert_eq!(
            found, expected,
            "the `game_aura` deletion census changed. Every deletion site must either call \
             `mount::recompute_mount` when a removed row satisfies `mount_aura_moves_mount`, or be \
             listed here with the reason it cannot remove a mount aura — otherwise a mount can be \
             deleted while `mount_display_id` keeps projecting it."
        );
    }

    /// Every `.rs` file under `module/src`, sorted, so the census above cannot be sidestepped by
    /// putting a new aura-deletion path outside the spell module.
    fn module_sources() -> Vec<std::path::PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("module source directory is readable") {
                let path = entry.expect("module source entry is readable").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&crate::test_scan::repo_root().join("module/src"), &mut out);
        out.sort();
        out
    }

    fn rel(path: &std::path::Path) -> String {
        path.strip_prefix(crate::test_scan::repo_root())
            .unwrap_or(path)
            .display()
            .to_string()
    }
}
