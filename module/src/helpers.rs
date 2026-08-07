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
/// The candidate is collapsed to AT MOST one row with `.next()` FIRST, then handed to
/// [`gate_by_guid`] — never `Iterator::filter` ahead of `.next()`, which would SKIP an in-transit
/// row and hand back the next matching entity instead of none, silently defeating the fence rather
/// than closing it. The sense of the check itself (in-transit ⇒ refuse) lives in
/// [`gate_in_transit`], not here — see its doc for why (issue #64).
pub fn entity_by_owner(ctx: &ReducerContext, owner: Identity) -> Option<WorldEntity> {
    gate_by_guid(
        ctx,
        ctx.db.game_world_entity().by_owner().filter(&owner).next(),
        |e| e.guid,
    )
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
/// [`gate_by_guid`] for the check itself.
pub fn character_by_guid(ctx: &ReducerContext, guid: u64) -> Option<Character> {
    gate_by_guid(ctx, ctx.db.game_character().guid().find(guid), |c| c.guid)
}

/// [`character_by_guid`], name-keyed. Case-insensitive ASCII fold — the operator/whisper convention
/// (`/w bob` reaches "Bob"), which the `#[unique]` exact-match name index cannot do, so this is a
/// scan by construction. Same fence, same read-as-absent semantics.
pub fn character_by_name(ctx: &ReducerContext, name: &str) -> Option<Character> {
    gate_by_guid(
        ctx,
        ctx.db
            .game_character()
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name)),
        |c| c.guid,
    )
}

/// The ONE place in the tree that asks "is this row's character mid-transfer?" and acts on the
/// answer. All three chokepoints above route through it; `guid_of` is the only thing they differ in.
///
/// It exists because of the shape of the mutation it is defending against (#64/#380). Each
/// chokepoint used to compute its own `in_transit` boolean inline, and a `!` glued anywhere into
/// that expression — onto `is_in_transit`, or onto the `candidate.as_ref().is_some_and(..)` around
/// it — inverts the fence with the same identifiers in the same order, which no `.contains()` scan
/// can see. The answer used to be pinning all three bodies verbatim as strings inside a test, so
/// every legitimate edit had to be made twice. Collapsing them to one call site is the structural
/// version of that pin: there is now a single two-line expression to get wrong, and it is inside
/// `.cargo/mutants.toml`'s cargo-mutants scope — where `replace gate_by_guid -> None` is CAUGHT.
///
/// The DECISION itself stays in [`gate_in_transit`], which is pure and pinned by a direct assertion.
fn gate_by_guid<T>(
    ctx: &ReducerContext,
    candidate: Option<T>,
    guid_of: impl Fn(&T) -> u64,
) -> Option<T> {
    let in_transit = candidate
        .as_ref()
        .is_some_and(|row| crate::transfer::is_in_transit(ctx, guid_of(row)));
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
/// Now there is no `!` left at any call site for a mutation to flip: [`gate_by_guid`] computes
/// `in_transit` as a plain (unnegated) boolean and hands it here, where this file's
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

/// The nearest live entity to `origin` matching `pred`, restricted to `origin`'s OWN `(map_id,
/// instance_id)` partition via [`in_same_partition`] — a raw squared distance compared ACROSS maps
/// or instances is meaningless (two guids can be numerically close while standing on different
/// continents), so every "nearest X" scan must apply this same partition filter before comparing
/// distances. Candidates come off the `by_map` index (map-scoped, not a raw `.iter()` — the
/// partition-discipline tripwire), narrowed to the instance in-code; callers with a natural search
/// radius should prefer `entities_near` instead (grid-indexed, cheaper at scale). Used by the debug
/// "nearest" test levers (`debug_kill_nearest`, `debug_ranged_attack_nearest`, `debug_skin_nearest`)
/// and the nearest-trainer scans in `skill.rs`, which used to duplicate this loop byte-for-byte.
///
/// Every one of those callers is a `debug_reducers` lever ("nearest X" is a test-harness affordance —
/// the real client always names its target), so the whole helper is gated with them: a production
/// build has no caller, and an ungated definition is dead code the cold-clone build gate rejects.
#[cfg(feature = "debug_reducers")]
pub(crate) fn nearest_entity(
    ctx: &ReducerContext,
    origin: &WorldEntity,
    mut pred: impl FnMut(&WorldEntity) -> bool,
) -> Option<WorldEntity> {
    let mut best: Option<(WorldEntity, f32)> = None;
    // `by_map` (not a raw `.iter()`, partition-discipline tripwire) scopes candidates to the
    // origin's own map before `in_same_partition` narrows to its instance too.
    for e in ctx.db.game_world_entity().by_map().filter(&origin.map_id) {
        if !in_same_partition(&e, origin.map_id, origin.instance_id) || !pred(&e) {
            continue;
        }
        let d = (e.x - origin.x).powi(2) + (e.y - origin.y).powi(2);
        if best.as_ref().is_none_or(|(_, bd)| d < *bd) {
            best = Some((e, d));
        }
    }
    best.map(|(e, _)| e)
}

/// Address a `game_group_event` / `game_whisper_event` row: the recipient's bound identity when this
/// database HAS the character (every world shard), [`Identity::ZERO`] when it does not (realm-core,
/// whose only characters are guids in the directory tables). ZERO matches no client — the per-player
/// RLS filter on both tables is `recipient_identity = :sender` and a client's sender is never ZERO —
/// so a ZERO-addressed row is visible to the owner-token coordinator alone, which is exactly who
/// reads realm-core's events. Pure, so the fallback is pinned by a test rather than by a live node.
///
/// Moved here from `group.rs` (issue #371): work-item 187's roll/master-loot notifications
/// (`loot.rs`) and `chat.rs`'s whisper relay (`push_whisper`) both reuse this SAME address rule, so
/// it lives with the other cross-module lookups rather than under the module it happened to be
/// written for first.
pub(crate) fn event_recipient_identity(bound: Option<Identity>) -> Identity {
    bound.unwrap_or(Identity::ZERO)
}

/// The live `game_world_entity` row for `guid`, the canonical "must be in world" fetch (issue #371)
/// that replaces the 38 hand-rolled `ctx.db.game_world_entity().guid().find(guid).ok_or_else(||
/// format!(...))?` call sites scattered across the reducer modules (debug.rs alone had 14). Most of
/// those sites already spelled their error text exactly as this helper's default below; callers whose
/// wording differs and is wire/user-visible (the gateway's `is_desync_error` substring classifier
/// keys on "not in world" / "no live entity", and `combat::validate_attack_target`'s target-lookup
/// error is explicitly load-bearing — see its doc) keep their own string via `.map_err(...)` on top of
/// this helper instead of adopting the default, so the migration is a pure lookup consolidation with
/// zero change to any error string a caller relies on.
pub fn live_entity(ctx: &ReducerContext, guid: u64) -> Result<WorldEntity, String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))
}

/// [`character_by_guid`], REFUSING (`Err`) instead of `Option::None` — the durable-row twin of
/// [`live_entity`] for the issue-#30 REFUSE character fence, hand-rolled as `character_by_guid(ctx,
/// guid).ok_or_else(|| format!("no character ..."))?` at half a dozen call sites (issue #371). Same
/// zero-behavior-change discipline as `live_entity`: the default message below is what most callers
/// already spelled verbatim; a caller with different wording keeps it via `.map_err(...)`.
///
/// Gated like [`nearest_entity`]: every surviving call site is a `debug_reducers` lever (the
/// production REFUSE fences take the `Option` shape from `character_by_guid` directly), so without
/// the feature this is dead code the cold-clone build gate rejects. If a non-debug caller ever wants
/// the `Result` shape, drop the gate rather than re-hand-rolling the `ok_or_else`.
#[cfg(feature = "debug_reducers")]
pub fn require_character(ctx: &ReducerContext, guid: u64) -> Result<Character, String> {
    character_by_guid(ctx, guid).ok_or_else(|| format!("no character {guid}"))
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

/// The same `(map_id, instance_id, grid_x, grid_y)` shape [`grid_of`] returns, read straight off an
/// already-fetched `WorldEntity` — zero `game_world_entity` lookup. The event-constructor `signal_at`
/// variants (`SpellCastEvent`/`CombatEvent`, perf catalog 2.3) and the chat broadcast reducers whose
/// sender/roller entity is fetched up front (`send_emote`/`send_roll`) all use this instead of
/// `grid_of` when the entity is in hand — a landed swing with a seal proc + a queued strike used to
/// pay the `grid_of` PK lookup up to twelve times over for what is, in every case, the SAME row.
/// Pulled out pure (module crate convention — no `ReducerContext` test harness) so the field order
/// can't quietly drift from `grid_of`'s: [`tests::entity_addr_matches_grid_of_field_order`] pins it.
pub(crate) fn entity_addr(e: &WorldEntity) -> (u32, u64, i32, i32) {
    (e.map_id, e.instance_id, e.grid_x, e.grid_y)
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
            cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
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

    /// The event addressing #22 rests on: a recipient this database knows gets their bound identity
    /// (so the per-player RLS still scopes the row on a world shard), and one it does not gets ZERO
    /// — which no client's `:sender` can ever equal, so a realm-core event row is visible to the
    /// owner-token coordinator alone. Moved from `group.rs` with the function itself (issue #371).
    #[test]
    fn a_recipient_without_a_character_row_addresses_the_event_to_nobody() {
        let bound = Identity::from_byte_array([7u8; 32]);
        assert_eq!(event_recipient_identity(Some(bound)), bound);
        assert_eq!(event_recipient_identity(None), Identity::ZERO);
        assert_ne!(
            Identity::ZERO,
            bound,
            "ZERO must not be a reachable client identity"
        );
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

    /// `entity_addr` exists ONLY so the event-constructor `signal_at` variants (`SpellCastEvent`/
    /// `CombatEvent`, issue #369) and the chat broadcast reducers can stamp a row's AOI address off an
    /// already-fetched entity without a redundant `grid_of` lookup. Its whole contract is "same four
    /// fields, same order, as `grid_of`'s `(map_id, instance_id, grid_x, grid_y)` tuple" — a swapped
    /// `grid_x`/`grid_y` here would compile clean (both are `i32`) yet silently mis-place every
    /// affected broadcast in the AOI grid. Four distinct field values (never a repeated 0/1/2/3) so a
    /// transposition can't accidentally read back correct.
    #[test]
    fn entity_addr_matches_grid_of_field_order() {
        let e = entity(1, 7, 42, -3, 11);
        assert_eq!(entity_addr(&e), (7, 42, -3, 11));
    }
}
