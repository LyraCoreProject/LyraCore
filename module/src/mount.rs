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
use crate::game_player_skill;
use crate::game_skill_ability;
use crate::game_spell_effect;
use crate::game_world_entity;
use crate::spell::{SpellEffect, A_MOD_SPEED, A_MOUNTED, SPEED_MOUNTED};
use crate::WorldEntity;

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

/// Re-derive `guid`'s `mount_display_id` from its current aura rows. Zero `A_MOUNTED` auras clears the
/// display; one restores the value frozen on it. NEVER a delta — that is what makes every caller
/// idempotent. The row is only written when the value actually changed, because the gateway's
/// subscription diff fires on every write and a redundant write would relay a spurious VALUES update.
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
    if e.mount_display_id == display {
        return;
    }
    e.mount_display_id = display;
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

// ===========================================================================================
//  Mount eligibility — the cast gate. [entity]
//
//  Every check below is READ-ONLY, and the whole sweep runs inside `check_cast_gates`, which sits
//  strictly before the cast core spends anything. So a refused mount leaves the item, the aura set,
//  the display, run speed, cooldowns and combat state byte-identical.
// ===========================================================================================

/// Does casting `spell_id` mount its target? Keyed on the presence of an `A_MOUNTED` effect, never a
/// spell id or name — the same data-driven classification `items::ops::apply_item_use` reads to keep a
/// mount item in the bag, so onboarding another mount is data alone. [entity]
pub(crate) fn spell_is_mount(ctx: &ReducerContext, spell_id: u32) -> bool {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|e| e.kind == A_MOUNTED)
}

/// Does a `SkillAbility` race/class mask admit `id`? A 0 mask is the DBC's "everyone" row; otherwise
/// `id`'s bit must be set. Races and classes are 1-based, so id 1 is bit 0 and anything outside 1..=32
/// can never match. Pure — this is what makes a mount tied to another race's riding tradition refuse.
fn mask_admits(mask: u32, id: u8) -> bool {
    mask == 0 || ((1..=32).contains(&id) && mask & (1 << (id - 1)) != 0)
}

/// Does a learned rank meet a `SkillAbility` row's `min_skill`? Where vanilla's 75 (Apprentice) and 150
/// (Journeyman) riding tiers live: the threshold is inclusive, so exactly 75 rides a rank-75 mount and 74
/// does not. A negative/unauthored `min_skill` reads as 0 (no requirement). Pure.
fn rank_satisfies(rank: u16, min_skill: i32) -> bool {
    i32::from(rank) >= min_skill.max(0)
}

/// Is `guid` TRAINED for `spell_id`? True when one of the character's learned skill lines carries a
/// `game_skill_ability` row naming the spell, whose race/class masks admit the character, and whose
/// `min_skill` the learned rank meets. Walking the player's own lines (a handful) and probing the
/// `by_skill_line` index keeps this off a full scan of vanilla's ~10k ability rows.
///
/// Fails CLOSED: a mount whose skill-line data was never imported is uncastable rather than free, which
/// is the whole point of the gate — owning the item must not bypass riding training.
fn trained_for_spell(ctx: &ReducerContext, guid: u64, spell_id: u32, race: u8, class: u8) -> bool {
    let abilities = ctx.db.game_skill_ability();
    ctx.db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .any(|learned| {
            abilities
                .by_skill_line()
                .filter(&learned.skill_line)
                .any(|a| {
                    a.spell_id == spell_id
                        && mask_admits(a.race_mask, race)
                        && mask_admits(a.class_mask, class)
                        && rank_satisfies(learned.current, a.min_skill)
                })
        })
}

/// May a land mount be used at `(x, y, z)`? The ONE shared indoor predicate — the cast gate and the
/// movement heartbeat both read it rather than interpreting MOGP flags themselves, so there is a single
/// place for the WMO rule to be right.
///
/// FAILS OPEN by construction: `vmap::area_info` returns `None` when vmap is disabled, when no generation
/// is active, or when the probe finds no WMO group, and all three read as outdoors. The mount feature is
/// therefore fully shippable and correct with vmap off.
pub(crate) fn mount_cast_allowed_here(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> bool {
    !crate::vmap::area_info(ctx, map_id, x, y, z).is_some_and(|info| info.indoor)
}

/// Is `entity`'s head under a liquid surface? The SAME predicate player breath uses
/// (`terrain::liquid_level_at` + `env::is_submerged`), not a second water-position interpretation:
/// wading through shallow water, or floating at the surface, is not submerged and never refuses a mount.
fn is_submerged_here(ctx: &ReducerContext, entity: &WorldEntity) -> bool {
    let level = crate::terrain::liquid_level_at(ctx, entity.map_id, entity.x, entity.y);
    lyracore_shared::env::is_submerged(
        entity.z,
        level.unwrap_or_default(),
        level.is_some(),
        entity.movement_flags,
    )
}

/// The land-mount cast gate: refuse a mount spell unless the caster is a trained rider of the right line
/// and rank, alive, out of combat, outdoors and not submerged. `Ok(())` for every spell that carries no
/// `A_MOUNTED` effect, so the cast core calls this unconditionally.
///
/// Only PLAYERS are gated. A creature has no `game_player_skill` rows and no riding concept, so gating it
/// would refuse every scripted mount a mob is given, exactly like the level and cost gates' `is_player()`
/// carve-outs beside this one.
pub(crate) fn check_mount_cast(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    spell_id: u32,
) -> Result<(), String> {
    if !effects.iter().any(|e| e.kind == A_MOUNTED) || !caster.is_player() {
        return Ok(());
    }
    if !trained_for_spell(ctx, caster.guid, spell_id, caster.race(), caster.class()) {
        return Err(format!("spell {spell_id} requires riding training"));
    }
    // A corpse and a ghost are both refused. `caster.dead` is already checked by the shared cast gate;
    // the GHOST player flag is the released-spirit state that outlives it.
    if caster.dead || caster.player_flags & lyracore_shared::constants::player_flags::GHOST != 0 {
        return Err("you cannot mount while dead".to_string());
    }
    // The module's authoritative engagement state, never a gateway session flag — a mount is not a
    // combat escape.
    if crate::combat::is_engaged(ctx, caster.guid) {
        return Err("you cannot mount while in combat".to_string());
    }
    if !mount_cast_allowed_here(ctx, caster.map_id, caster.x, caster.y, caster.z) {
        return Err("you cannot mount indoors".to_string());
    }
    if is_submerged_here(ctx, caster) {
        return Err("you cannot mount in deep water".to_string());
    }
    Ok(())
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

    /// The riding rank tiers (stories 13/14), asserted on the boundary the gate actually compares:
    /// `min_skill` is inclusive, so 74 fails a rank-75 mount and 75 rides it, 149 fails a rank-150
    /// mount and 150 rides it. Unauthored (negative) data imposes no requirement rather than becoming
    /// an unreachable threshold.
    #[test]
    fn rank_satisfies_is_inclusive_at_the_vanilla_riding_tiers() {
        assert!(!rank_satisfies(74, 75));
        assert!(rank_satisfies(75, 75));
        assert!(rank_satisfies(150, 75));

        assert!(!rank_satisfies(149, 150));
        assert!(rank_satisfies(150, 150));

        assert!(rank_satisfies(0, 0));
        assert!(rank_satisfies(0, -1));
    }

    /// The race/class gate on a `SkillAbility` row (story 15): a 0 mask is the DBC's universal row, a
    /// populated one admits only its own bits, and an out-of-range id can never match. Races and classes
    /// are 1-based, so Human (1) is bit 0 — an off-by-one here would let every race ride every
    /// race-specific line.
    #[test]
    fn mask_admits_treats_race_and_class_ids_as_one_based_bits() {
        assert!(mask_admits(0, 1));
        assert!(mask_admits(0, 11));

        let human = 1u32 << 0;
        let orc = 1u32 << 1;
        assert!(mask_admits(human, 1));
        assert!(!mask_admits(human, 2));
        assert!(mask_admits(human | orc, 2));

        assert!(!mask_admits(human, 0));
        assert!(!mask_admits(u32::MAX, 33));
    }

    /// The gate is a PRE-SPEND sweep: it lives in `check_cast_gates`, whose whole contract is that
    /// nothing in it writes and everything that spends runs strictly after it returns `Ok` (story 33).
    /// Pinned by scan because the crate has no `ReducerContext` harness — what is asserted is that the
    /// call sits inside that function, not merely somewhere in the cast core.
    #[test]
    fn the_mount_gate_runs_inside_the_pre_spend_cast_sweep() {
        let gates = crate::test_scan::code_of(
            include_str!("spell/cast/resolve.rs"),
            "fn check_cast_gates(",
        );
        assert!(
            gates.contains("crate::mount::check_mount_cast(ctx, caster, effects, spell_id)?"),
            "the mount gate must run inside `check_cast_gates` — anywhere later and a refused mount \
             has already charged power or started a cooldown. Body was:\n{gates}"
        );
    }

    /// The gate reads the SHARED indoor predicate instead of testing MOGP flags itself, so T3's WMO
    /// correctness fix lands in one place, and it reuses the same submerged predicate player breath
    /// uses rather than inventing a second water-position interpretation.
    #[test]
    fn the_gate_reads_the_shared_indoor_and_liquid_predicates() {
        let src = include_str!("mount.rs");
        let allowed = crate::test_scan::code_of(src, "pub(crate) fn mount_cast_allowed_here(");
        assert!(
            allowed.contains("crate::vmap::area_info(ctx, map_id, x, y, z)"),
            "the indoor predicate must go through the shared WMO area query. Body was:\n{allowed}"
        );
        let submerged = crate::test_scan::code_of(src, "fn is_submerged_here(");
        assert!(
            submerged.contains("lyracore_shared::env::is_submerged(")
                && submerged.contains("crate::terrain::liquid_level_at("),
            "the liquid gate must reuse the breath predicate, not re-derive one. Body was:\n{submerged}"
        );
        let gate = crate::test_scan::code_of(src, "pub(crate) fn check_mount_cast(");
        assert!(
            gate.contains("mount_cast_allowed_here(ctx, caster.map_id")
                && !gate.contains("mogp")
                && !gate.contains("area_info"),
            "the gate must ask the shared predicate rather than inline a WMO flag test or its own \
             area query. Body was:\n{gate}"
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
