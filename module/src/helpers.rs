//! Shared lookup helpers used by reducers. Kept out of `lib.rs` to keep the index lean.

// The `#[table(accessor = X)]` macro generates a trait named `X` that provides `ctx.db.X()`; it
// must be in scope wherever a submodule reads a table. Import the accessor traits + row types.
use crate::{
    game_account, game_character, game_operator, game_world_entity, Account, Character, WorldEntity,
};
use spacetimedb::{Identity, ReducerContext, Table};

/// Gate a privileged owner-fired reducer to the trusted operator identity (the gateway coordinator +
/// deploy CLI). Defends `import_*` / account-provisioning from a DIRECT anonymous SpacetimeDB connection
/// that bypasses the gateway — the one check that can't move to the gateway. Fail-closed: rejects until
/// `claim_operator` has run. See [`crate::auth::claim_operator`].
pub fn require_operator(ctx: &ReducerContext) -> Result<(), String> {
    match ctx.db.game_operator().id().find(0) {
        Some(op) if op.identity == ctx.sender() => Ok(()),
        Some(_) => Err("operator only".to_string()),
        None => Err("operator not claimed".to_string()),
    }
}

/// The account currently bound to `identity` (set at `establish_session`), if any.
pub fn account_by_identity(ctx: &ReducerContext, identity: Identity) -> Option<Account> {
    ctx.db
        .game_account()
        .iter()
        .find(|a| a.identity == Some(identity))
}

/// The live world entity owned by `owner`, if in world. Indexed probe through `by_owner` (perf
/// catalog 1.2) — this is the auth prologue of essentially every player-fired reducer, so the old
/// full `game_world_entity` scan was an O(E) cost on EVERY player transaction.
/// Note: creatures all share `Identity::ZERO` as `owner_identity`, so a `ZERO` probe would walk
/// every creature — but no caller ever passes `ZERO` (`ctx.sender()` is a real client identity), and
/// the old scan would have matched the first creature anyway. Same result, same shape, no new guard.
///
/// THE actor chokepoint: every player-fired reducer resolves "who is acting" through here
/// (60+ call sites, all shaped `entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "... not in
/// world")?`). That is why the escrowed-transfer in-transit gate (issue #16) lives here rather than
/// at each caller — an in-transit character reads as "not in world" everywhere, for free.
/// `begin_transfer` also DELETES the live entity row in the same transaction it writes the escrow,
/// so the target side (the ~50 `map_id`/`instance_id` gates, aggro candidate scans, threat lists,
/// the AOI relay) stops seeing the character by construction. This check is the belt to that
/// braces: it fences the actor side even if a live row somehow survives.
///
/// SCOPE: this gates the ACTOR side only. Reducers that reach a character by `character_guid` or by
/// name go through [`character_by_guid`] / [`character_by_name`] instead (issue #30) — see the
/// verdict table in `transfer.rs`'s module doc for the paths that are deliberately NOT gated.
///
/// The candidate is collapsed to AT MOST one row with `.next()` FIRST, then separately checked —
/// never `Iterator::filter` ahead of `.next()`, which would SKIP an in-transit row and hand back
/// the next matching entity instead of none, silently defeating the fence rather than closing it.
/// The sense of the check itself (in-transit ⇒ refuse) lives in [`gate_in_transit`], not here — see
/// its doc for why (issue #64).
pub fn entity_by_owner(ctx: &ReducerContext, owner: Identity) -> Option<WorldEntity> {
    let candidate = ctx.db.game_world_entity().by_owner().filter(&owner).next();
    let in_transit = candidate
        .as_ref()
        .is_some_and(|e| crate::transfer::is_in_transit(ctx, e.guid));
    gate_in_transit(candidate, in_transit)
}

/// THE by-guid chokepoint (issue #30) — `entity_by_owner`'s twin for every reducer that reaches a
/// character by `character_guid` instead of through the acting entity. Returns the durable
/// `game_character` row unless the character is mid-transfer, in which case it reads as ABSENT and
/// the caller's existing "no such character" arm fires.
///
/// Reading as absent is deliberate and is what keeps this a zero-cost seam: every call site already
/// has a not-found arm with an error string the gateway already handles, so fencing adds no new
/// player-facing error and no gateway edit (the constraint in issue #30's design notes).
///
/// SCOPE — this gate is the REFUSE verdict, and refusal is NOT the right answer everywhere. Three
/// other verdicts exist and are deliberately not routed through here; the audited table lives in
/// `transfer.rs`'s module doc:
///   * DEFER    — `loot::credit_purse` folds a post-begin `money` delta into the escrowed blob
///     (`transfer::defer_money_delta`); refusing would DROP a third party's copper.
///   * REGENERATE — `auth::establish_session` rewrites `Character.owner_identity`, which is
///     per-CONNECTION derived state the destination rebinds on arrival.
///   * OPEN     — `group::group_accept`/`group_uninvite`/`group_leave`; spec #12 puts group
///     membership on realm-core, settled by issue #22.
///
/// Same `.find()`-then-check ordering as `entity_by_owner` above, and the same shared
/// [`gate_in_transit`] for the sense of the check.
pub fn character_by_guid(ctx: &ReducerContext, guid: u64) -> Option<Character> {
    let candidate = ctx.db.game_character().guid().find(guid);
    let in_transit = candidate
        .as_ref()
        .is_some_and(|c| crate::transfer::is_in_transit(ctx, c.guid));
    gate_in_transit(candidate, in_transit)
}

/// [`character_by_guid`], name-keyed. Case-insensitive ASCII fold — the operator/whisper convention
/// (`/w bob` reaches "Bob"), which the `#[unique]` exact-match name index cannot do, so this is a
/// scan by construction. Same fence, same read-as-absent semantics.
pub fn character_by_name(ctx: &ReducerContext, name: &str) -> Option<Character> {
    let candidate = ctx
        .db
        .game_character()
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name));
    let in_transit = candidate
        .as_ref()
        .is_some_and(|c| crate::transfer::is_in_transit(ctx, c.guid));
    gate_in_transit(candidate, in_transit)
}

/// The decision every in-transit fence in this file reduces to, pulled out pure and generic so its
/// SENSE — not just its presence — can be pinned by a direct assertion instead of a source scan.
///
/// Issue #64: every chokepoint above used to spell this as `.filter(|x| !is_in_transit(..))` at
/// the call site. A scan can confirm `is_in_transit` is called and in what order, but nothing short
/// of running the code can tell `!is_in_transit(..)` apart from `is_in_transit(..)` — same
/// identifiers, same order, opposite meaning. Dropping that one `!` made `entity_by_owner` return
/// ONLY in-transit entities and `None` for everyone else, and all 533 module tests stayed green.
///
/// Now there is no `!` left at any call site for a mutation to flip: each one computes `in_transit`
/// as a plain (unnegated) boolean and hands it here, where this file's
/// `gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one` test pins the branch
/// directly with concrete values — swap the branches and that test fails by name.
pub(crate) fn gate_in_transit<T>(candidate: Option<T>, in_transit: bool) -> Option<T> {
    if in_transit {
        None
    } else {
        candidate
    }
}

/// Live entities in the grid cells covering `radius` yards around `(x, y)` on `map_id` +
/// `instance_id`, read through the `by_grid` btree index — the scale-safe replacement for a full
/// `game_world_entity` scan. Coverage is by WHOLE cells (`spatial::GRID_CELL_SIZE`), so the result
/// is a superset of the exact circle: callers keep their own precise distance check. Cost scales
/// with the neighborhood's population, not the world's. `instance_id` is a REQUIRED param (no
/// default) so the compiler finds every call site (work-item 190 slice 1) — pass the ACTING
/// entity's own `instance_id`; every slice-1 caller is at instance 0, so behavior is unchanged.
pub(crate) fn entities_near(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    radius: f32,
) -> Vec<WorldEntity> {
    let (gx0, gx1, gy0, gy1) = lyracore_shared::spatial::covering_cell_box(x, y, radius);
    let entities = ctx.db.game_world_entity();
    let mut out = Vec::new();
    for gx in gx0..=gx1 {
        for gy in gy0..=gy1 {
            // `by_grid`'s own exact-tuple match already scopes this to (map_id, instance_id, gx, gy),
            // so `in_same_partition` is a belt-and-suspenders re-check of the SAME predicate — kept as
            // a real (non-mocked-ctx) call so the isolation key is unit-testable (see `tests` below;
            // the module crate has no `ReducerContext` test harness by design).
            out.extend(
                entities
                    .by_grid()
                    .filter((map_id, instance_id, gx, gy))
                    .filter(|e| in_same_partition(e, map_id, instance_id)),
            );
        }
    }
    out
}

/// Is `e` in the `(map_id, instance_id)` isolation partition `entities_near` queries for? The exact
/// predicate the `by_grid` filter tuple encodes (work-item 190 slice 1) — pulled out pure so the
/// instance dimension is unit-testable without a live `ReducerContext`.
pub(crate) fn in_same_partition(e: &WorldEntity, map_id: u32, instance_id: u64) -> bool {
    e.map_id == map_id && e.instance_id == instance_id
}

/// The grid address of `guid`'s live entity, for stamping a broadcast event row (perf catalog 2.3).
///
/// The event tables — spell casts/impacts, combat swings, emotes, rolls — carried no spatial
/// columns, so each was subscribed `SELECT *` by EVERY per-player connection: the database
/// evaluated and shipped every swing and every spell visual anywhere in the world to all P
/// sessions, which then discarded the out-of-scope ones via the gateway's `created` set. Write
/// volume is small (~5/s at 500 players) but the FAN-OUT is P, so it was ~13% of all deliveries
/// and it is the term that grows with population rather than with local density.
///
/// Stamping these four columns lets the AOI tracker subscribe them as grid-box ranges, exactly like
/// `game_world_entity`/`game_entity_motion`/`game_creature_spline`.
///
/// Falls back to `(0, 0, 0, 0)` when the actor has no live entity — a caster who logged out
/// mid-flight, say. Such a row simply matches no box and is never delivered, which is the correct
/// outcome: nobody can see an actor who is not in the world.
pub(crate) fn grid_of(ctx: &ReducerContext, guid: u64) -> (u32, u64, i32, i32) {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .map(|e| (e.map_id, e.instance_id, e.grid_x, e.grid_y))
        .unwrap_or((0, 0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `WorldEntity` for the partition test — every field but `map_id`/`instance_id`/
    /// `grid_x`/`grid_y` is a neutral zero/default, mirroring `combat/tables.rs`'s `entity_for_regen`.
    fn entity(guid: u64, map_id: u32, instance_id: u64, grid_x: i32, grid_y: i32) -> WorldEntity {
        WorldEntity {
            guid,
            owner_identity: Identity::ZERO,
            account_id: 0,
            map_id,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            grid_x,
            grid_y,
            last_move_ms: 0,
            type_mask: 0,
            entry: 0,
            scale_x: 1.0,
            health: 1,
            max_health: 1,
            power: 0,
            max_power: 0,
            level: 1,
            faction_template: 0,
            unit_bytes_0: 0,
            display_id: 0,
            native_display_id: 0,
            unit_flags: 0,
            base_attack_time_ms: 2000,
            dynamic_flags: 0,
            dead: false,
            player_bytes: 0,
            player_bytes_2: 0,
            player_bytes_3: 0,
            player_flags: 0,
            xp: 0,
            next_level_xp: 0,
            target_guid: 0,
            money: 0,
            unit_bytes_1: 0,
            strength: 0,
            agility: 0,
            stamina: 0,
            intellect: 0,
            spirit: 0,
            npc_flags: 0,
            armor: 0,
            leg_ends_ms: 0,
            wp_target: 0,
            movement_flags: 0,
            combat_until_ms: 0,
            pickpocketed: false,
            next_swing_spell: 0,
            overpower_until_ms: 0,
            revenge_until_ms: 0,
            stance: 0,
            owner_guid: 0,
            skinned: false,
            mana_regen_paused_until_ms: 0,
            death_expire_micros: 0,
            instance_id,
            run_speed_mult_bp: 10_000,
            godmode: false,
            resting: false,
        }
    }

    /// The real regression this slice exists to prevent: two entities sharing a map AND a grid cell
    /// but living in DIFFERENT instances must never both come back from the same `entities_near`
    /// partition query — `in_same_partition` (the pure predicate `entities_near` actually filters
    /// with) must separate them into disjoint result sets. Fails if the instance dimension is ever
    /// dropped from the filter (e.g. a future edit narrows it back to `e.map_id == map_id` alone).
    #[test]
    fn entities_near_partitions_distinct_instances_on_the_same_cell_into_disjoint_sets() {
        let same_map = 1u32;
        let same_cell = (3i32, -4i32);
        let population = [
            entity(100, same_map, 0, same_cell.0, same_cell.1), // open world
            entity(101, same_map, 0, same_cell.0, same_cell.1), // open world
            entity(200, same_map, 7, same_cell.0, same_cell.1), // instance 7
            entity(201, same_map, 9, same_cell.0, same_cell.1), // instance 9 (a third, distinct instance)
            entity(300, same_map + 1, 0, same_cell.0, same_cell.1), // different MAP entirely
        ];

        let instance_0: Vec<u64> = population
            .iter()
            .filter(|e| in_same_partition(e, same_map, 0))
            .map(|e| e.guid)
            .collect();
        let instance_7: Vec<u64> = population
            .iter()
            .filter(|e| in_same_partition(e, same_map, 7))
            .map(|e| e.guid)
            .collect();
        let instance_9: Vec<u64> = population
            .iter()
            .filter(|e| in_same_partition(e, same_map, 9))
            .map(|e| e.guid)
            .collect();

        assert_eq!(instance_0, vec![100, 101]);
        assert_eq!(instance_7, vec![200]);
        assert_eq!(instance_9, vec![201]);

        // Pairwise disjoint — the same cell, three different instances, zero overlap.
        for a in &instance_0 {
            assert!(!instance_7.contains(a) && !instance_9.contains(a));
        }
        for a in &instance_7 {
            assert!(!instance_0.contains(a) && !instance_9.contains(a));
        }
    }

    /// THE fix for issue #64. `entity_by_owner` / `character_by_guid` / `character_by_name` all
    /// reduce to this one pure decision, so pinning it here pins the sense of all three chokepoints
    /// at once — no `ReducerContext` needed, unlike the reducers that call it.
    ///
    /// This is the direct behavioural equivalent of the mutation the issue was filed over: swap the
    /// two branches below (`in_transit => candidate` / `else => None`) and this test fails by name.
    /// The old code had no such test — only a source scan checking that `is_in_transit` appeared in
    /// the right order — and a single dropped `!` at the call site satisfied that scan while
    /// returning ONLY in-transit rows.
    #[test]
    fn gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one() {
        assert_eq!(
            gate_in_transit(Some("normal_character"), false),
            Some("normal_character"),
            "not in transit: the candidate must pass through untouched"
        );
        assert_eq!(
            gate_in_transit(Some("in_transit_character"), true),
            None,
            "in transit: the candidate must be refused (read as absent), never handed back"
        );
        // Nothing found in the first place — in-transit-ness is moot either way.
        assert_eq!(gate_in_transit(None::<&str>, false), None);
        assert_eq!(gate_in_transit(None::<&str>, true), None);
    }
}
