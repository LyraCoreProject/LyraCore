//! Dungeon-instancing lifecycle (work-item 190 slices 2+3). Slice 1 landed the substrate — an
//! `instance_id` column on `game_world_entity` (+ `by_grid`), every entity↔entity gate, and the
//! gateway relay gates. This module adds the LIFECYCLE:
//!
//! - **`game_instance`** — one row per live instance (map, owning party, occupancy stamp, reset
//!   flag). `instance_id` is the `#[auto_inc]` PK: SpacetimeDB allocates from 1, so **0 = the open
//!   world** is reserved by construction, never by convention-checking code.
//! - **`game_instance_binding`** — one row per (character, map): which instance that character
//!   re-enters through the dungeon portal. Survives party disband (vanilla: you stay bound to the
//!   instance you entered until it resets/reaps); dropped when the instance is reaped.
//! - **Entry** — `resolve_or_create_instance`, called from the 225 areatrigger hook
//!   (`quest::apply_enter_areatrigger`) when the portal targets a DUNGEON map. Resolve order: **the
//!   party's live instance → the character's own live binding → create** (issue #39 reversed the
//!   first two — the 190 design's binding-first order split a party whose members had entered
//!   separately). Solo entry allowed (binds to the character, `party_id = 0`), and a solo instance
//!   is ADOPTED by the party its holder has since joined so the members behind them join it too.
//! - **`create_instance`** — inserts the row, spawns the per-instance POPULATION (every
//!   `game_creature_spawn` template on the map through the NORMAL `build_creature_entity` path with
//!   the new instance id — templates/spawn rows are NEVER cloned), per-instance COPIES of the
//!   dungeon's interactive gameobjects (DOOR/BUTTON/CHEST/GOOBER, type-gated) — unless
//!   `game_config.hosts_instances` is off, in which case it files the row alone as a LEASE and the
//!   shard that owns the map spawns the population via `ensure_instance` (issue #39). It does NOT arm a
//!   dedicated 229 tick row — the catch-all covers every instance at the same cadence for free (perf
//!   catalog 1.3); `debug_arm_instance_tick` still arms one on demand for a faster cadence.
//! - **Reap (slice 3)** — its own scheduled reducer (`reap_instances`, the `EventReaperSchedule`
//!   precedent — gc.rs is deliberately untouched): reaps an instance empty for
//!   [`INSTANCE_EMPTY_REAP_MICROS`] (30min) or flagged `reset_requested` while empty. Teardown
//!   order per the design: population (entities/corpses/loot/GO copies) → the 228 encounter-kernel
//!   sweep splice → the 229 tick row → bindings → the `game_instance` row.
//!
//! ## Occupancy mechanism (slice 3 decision — stated per the work item)
//! `last_empty_at_micros` is maintained by the REAPER itself, not by the per-instance tick row's
//! sense tick: each `reap_instances` firing makes ONE pass over `game_world_entity` classifying
//! every instance's player-occupancy at once (a `HashSet` of instance ids with a live player), then
//! stamps/clears/reaps per instance. Cost: one entity-table scan per reaper firing (60s), ZERO
//! added work on the hot 0.5s creature tick, and a single writer for the stamp (no tick↔reaper
//! write race). Minute resolution is exact enough for a 30-minute threshold — reap eligibility
//! requires two reaper observations ≥30min apart to BOTH see the instance empty, and any re-entry
//! in between clears the stamp. The 229 sense-tick alternative would add a per-4s write path per
//! instance for no accuracy the 30-minute constant can use. `reset_requested` takes effect on the
//! next firing that observes the instance empty (≤60s latency — acceptable; vanilla's reset is
//! instant but our async reap is invisible to the resetting party, who by definition are outside).
//!
//! ## Respawn-within-a-run (v1 decision — stated per the work item)
//! Instance populations are **entity-only** (the 229 trap: `game_creature_spawn` rows are NOT
//! instance-tagged, and `pass_respawn` builds at instance 0 — riding it would leak respawns into
//! the open world). So trash killed inside a live run does NOT respawn (vanilla 5-mans do respawn
//! trash on a timer; honest v1 gap — per-instance spawn bookkeeping is the future fix), instance
//! corpses do NOT decay mid-run (`pass_decay` is spawn-row-driven; corpses + their loot live until
//! the reap — cosmetically fine for a bounded-lifetime instance, and loot stays takable), and the
//! population is stationary-until-aggro (no waypoint/wander/return-home passes: all three anchor on
//! spawn rows/waypoints keyed by the spawn guid, which per-instance copies deliberately don't
//! share). Aggro/assist/chase/flee/casting all work — they are entity+template+melee-row driven.
//!
//! ## Guid namespaces (collision-proofing, unit-tested)
//! Per-instance creature copies reuse the wave-guid layout (`encounter::wave_guid`:
//! `0xF130 | entry<<24 | low24`) with **bit 23 of the low set** ([`INSTANCE_POP_LOW_BAND`]) — a
//! band disjoint from imported spawn lows (cmangos db guids ≪ 2^23) and from `spawn_wave`'s
//! allocator (which maxes over SPAWN rows only and so must never be able to collide with these
//! spawn-row-less entities). Per-instance GO copies get `0xF110 | bit46 | seq`
//! ([`GO_COPY_BAND`]) — below `gameobject::POOL_TAG` (bit 47), above every static/debug low.
//!
//! Both tables are deliberately **NOT `public` and NOT gateway-subscribed** (checked against
//! `gateway/src/stdb/connection.rs`'s subscription list): the gateway's relay gates key off
//! `game_world_entity.instance_id` (slice 1) and the viewer's own entity row — no client or relay
//! ever reads the instance/binding rows themselves, so no binding files exist for them (the
//! `game_encounter_state` precedent). [server]

use std::collections::{HashMap, HashSet};

use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{
    game_config, game_corpse, game_creature_move_schedule, game_creature_spawn,
    game_creature_template, game_encounter_spawn, game_gameobject, game_gameobject_template,
    game_group, game_world_entity,
};

// ===========================================================================================
//  Constants / policy
// ===========================================================================================

/// The maps that are DUNGEONS — entering an areatrigger portal targeting one of these
/// resolves-or-creates a `game_instance` instead of landing at instance 0.
///
/// MOVED to `lyracore_shared::instance` by issue #48: the GATEWAY needs the same set to check, at
/// startup, that the database which will own a dungeon's instances actually hosts instance
/// populations (`hosts_instances`) — and a set the two tiers could disagree about is exactly how
/// #48's empty dungeons stayed invisible. Re-exported here so every call site and every tripwire in
/// this crate keeps naming it locally. Every entry MUST have an [`entrance_fallback`] arm
/// (unit-pinned below) so a reaped-instance login can never strand.
pub(crate) use lyracore_shared::instance::is_dungeon_map;
/// The set itself has no non-test caller in this crate (`is_dungeon_map` is the read path); it is
/// imported under a distinct name — issue #376 reserves `DUNGEON_MAPS` locally for the
/// dungeon-detail table below — for the cross-tier consistency pin, which is the invariant that
/// keeps the shared set safe to extend from either tier.
#[cfg(test)]
pub(crate) use lyracore_shared::instance::DUNGEON_MAPS as SHARED_DUNGEON_MAP_IDS;

/// One fully-described dungeon (issue #376). This used to be FOUR hand-synchronized `match map_id`
/// sites — [`entrance_fallback`] here, plus `world::graveyard::instance_release_zone` and
/// `...instance_static_fallback` — each of which could independently gain (or omit) an arm for a
/// map, and the omission surfaced only as a runtime `warn!` the first time a release actually hit
/// the gap (`resolve_graveyard`, pre-#376). A `DungeonMap` has no optional fields, so a map is now
/// either fully described here — entrance AND release zone AND release fallback, together — or
/// absent, i.e. not a dungeon; there is no half-configured state left to warn about.
pub(crate) struct DungeonMap {
    pub(crate) map_id: u32,
    /// The open-world position OUTSIDE the entrance — `(map, x, y, z, o)`, the stranding fallback
    /// for a login whose `pending_instance_id` was reaped ("fall back to the entrance at instance
    /// 0", design doc §3).
    ///
    /// PREMISE CORRECTION (vs. the work item's "reverse-lookup the areatrigger pair" option): the
    /// reverse-lookup is NOT implementable with current data — `game_areatrigger_teleport` rows
    /// carry only the TARGET (the entrance row's target is INSIDE the dungeon; the exit row's
    /// target is the coords we want but the row records neither its source map nor which dungeon it
    /// exits, so with a second dungeon imported the lookup is ambiguous). Hence the per-map record,
    /// `[V]`.
    pub(crate) entrance: (u32, f32, f32, f32, f32),
    /// The instance's own zone id for graveyard release (cmangos `game_graveyard_zone.ghost_zone`)
    /// — the map→zone hop `world::graveyard::resolve_zone_id` can't make without terrain.
    pub(crate) release_zone: u32,
    /// Static release floor when nothing is imported — `(map, x, y, z, o)`. NEVER
    /// `world::graveyard::nearest(px, py)` for an instance map: cross-map 2-D distance against the
    /// open-world consts is meaningless (see `world::graveyard`'s section doc).
    pub(crate) release_fallback: (u32, f32, f32, f32, f32),
}

/// The dungeon-detail table — one entry per dungeon, pinned 1:1 against
/// [`lyracore_shared::instance::DUNGEON_MAPS`] by
/// `every_shared_dungeon_map_has_a_dungeon_maps_record` below (the test-time tripwire that replaced
/// the old runtime `warn!`).
///
/// `[V]` Map 36 → Deadmines: entrance ≈ Moonbrook village, Westfall (map 0, ~(-11080, 1520, 46)) —
/// approximate vanilla coords, no dump/client in this sandbox to confirm; the fail-safe is inherent
/// (any error of tens of yards still lands in open-world Westfall — never stranded, never inside WMO
/// geometry). Confirm against your dump's areatrigger_teleport EXIT row (~1448) target and tighten.
/// Release zone 1581 (The Deadmines, cmangos AreaTable — CONFIRM against your own dump's
/// `game_graveyard_zone` rows; a wrong id here just means the zone-linked lookup resolves nothing
/// and the static fallback below applies) and release fallback Sentinel Hill are the same `[V]`
/// estimates the pre-#376 per-site consts always carried.
pub(crate) const DUNGEON_MAPS: &[DungeonMap] = &[DungeonMap {
    map_id: 36,
    entrance: (0, -11080.0, 1520.0, 46.0, 0.0), // [V] Moonbrook, Westfall
    release_zone: 1581,                         // [V] The Deadmines
    release_fallback: (0, -10650.0, 1180.0, 34.0, 0.0), // [V] Sentinel Hill
}];

/// Look up `map_id`'s full dungeon record, or `None` if it is not a dungeon. Pure — the one read
/// path every former match site (entrance / release-zone / release-fallback) now goes through.
pub(crate) fn dungeon(map_id: u32) -> Option<&'static DungeonMap> {
    DUNGEON_MAPS.iter().find(|d| d.map_id == map_id)
}

/// The open-world position OUTSIDE a dungeon's entrance — see [`DungeonMap::entrance`]. Kept as its
/// own function: every call site outside this module (`world.rs`'s stranding guard, the tests below)
/// still names it, and a thin field-read is clearer at the call site than repeating
/// `dungeon(m).map(|d| d.entrance)`.
pub(crate) fn entrance_fallback(map_id: u32) -> Option<(u32, f32, f32, f32, f32)> {
    dungeon(map_id).map(|d| d.entrance)
}

/// Reap an instance after it has been EMPTY this long (30min const, per the 190 design). Vanilla
/// keeps an untouched instance alive ~1h; 30min is the item's chosen constant (deviation noted).
pub(crate) const INSTANCE_EMPTY_REAP_MICROS: i64 = 30 * 60 * 1_000_000;

/// `reap_instances` cadence — minutes-scale per the design (occupancy resolution; see the module
/// doc). 60s: fine-grained enough that `reset_requested` lands fast, coarse enough to cost nothing.
pub(crate) const INSTANCE_REAPER_INTERVAL_MICROS: i64 = 60 * 1_000_000;

/// How long a **LEASE** must read empty before the reaper takes it (issue #21).
///
/// A lease is a `game_instance` row on a database that does not host instance populations
/// (`game_config.hosts_instances = false`, issue #39) — the world shard of a Phase A deployment.
/// It reads EMPTY within seconds of the last party member transferring to the instance shard, i.e.
/// seconds after the run *starts*, because occupancy is counted from live player entities and there
/// are none here any more. Reaping it on the ordinary 30-minute timer therefore deletes the world
/// shard's only record of "this party is in instance N" **while they are still fighting in it**, and
/// the next member through the portal — someone who died, released, and ran back — resolves
/// `InstanceRoute::Create` and is teleported into a fresh, empty dungeon while their party is in the
/// old one. That is the same split #39 fixed from the other direction.
///
/// **The countdown starts at the run's START, not at its end**, which is what sizes this number.
/// The lease reads empty seconds after the party leaves the world shard, so the timer has to exceed
/// the run's WALL-CLOCK LENGTH — it is not an idle grace period. Anything shorter than the longest
/// run the realm intends to support just moves the 30-minute fork further out: a BRD or Stratholme
/// clear runs 3–5 hours in vanilla and a 40-man raid night runs longer, so 3h (the first value this
/// constant held) re-created the same split for exactly the runs that hurt most to lose.
///
/// The error is deliberately one-sided. Too SHORT forks a party mid-run — the bug. Too LONG costs
/// one stub row per run, and its only behavioural effect is that a re-entry within the window
/// resolves the OLD instance id, which the owning shard then re-mints with a fresh population
/// (`ensure_instance`) — i.e. indistinguishable from a new instance to the players. So the value is
/// pushed well past any plausible run rather than tuned: 12h, pinned by
/// `a_lease_only_database_holds_its_stub_rows_far_past_the_thirty_minute_run_timer`.
///
/// Deliberate simplification: a constant, not a protocol — the simplest thing that closes the
/// hole. Upgrade path is the one [`teardown_instance_inner`] already names: realm-core owns the
/// instance→shard index (#22) and the lease stops existing. **Single-database realms never reach
/// this arm at all** — `hosts_instances` defaults to true, so their reap semantics are byte-for-byte
/// what they were (#21 AC#4).
pub(crate) const INSTANCE_LEASE_REAP_MICROS: i64 = 12 * 60 * 60 * 1_000_000;

/// The empty-timer this database reaps instances on: [`INSTANCE_EMPTY_REAP_MICROS`] where the
/// populations actually live, [`INSTANCE_LEASE_REAP_MICROS`] where only leases do. Pure.
pub(crate) fn empty_reap_micros(hosts_populations: bool) -> i64 {
    if hosts_populations {
        INSTANCE_EMPTY_REAP_MICROS
    } else {
        INSTANCE_LEASE_REAP_MICROS
    }
}

/// Cap on instances torn down in ONE reaper firing — bounds the transaction like the corpse
/// reaper's batching discipline (a mass-expiry event tears down over a few firings, not one
/// giant commit).
const REAP_MAX_PER_FIRING: usize = 4;

/// Bit 23 of the 24-bit unit-guid low: the per-instance POPULATION band. Disjoint by construction
/// from imported/seeded spawn-row lows AND from `encounter::spawn_wave`'s allocator (which maxes
/// over spawn rows only — an instance-population entity has no spawn row, so without this band a
/// wave of the same entry could re-allocate a live population guid and panic the insert).
/// Relies on spawn-row lows staying < 2^23 (cmangos creature db guids are ~10^5; `[V]` for exotic
/// dumps — a violation shows up as a loud duplicate-PK insert failure, never silent corruption).
pub(crate) const INSTANCE_POP_LOW_BAND: u64 = 0x80_0000;

/// Bit 46 of a gameobject guid low: the per-instance GO-COPY band. Below `gameobject::POOL_TAG`
/// (bit 47 — pool points), far above static imported db-guid lows and the debug spawner's
/// entry-derived lows (< 2^24).
pub(crate) const GO_COPY_BAND: u64 = 1 << 46;

/// The gameobject types that get per-instance copies at `create_instance` (type-gated per the
/// design: the dungeon's interactive props — doors, levers, chests, quest objects). GATHER nodes
/// and pool points deliberately do NOT copy: the pool/respawn machinery is spawn-row/point-table
/// driven and open-world-only. QUESTGIVER-type GOs are deliberately NOT in this list either (190
/// review LOW): a static instance-0 QUESTGIVER on a dungeon map is UNREACHABLE from inside a run
/// (the use/giver gates require instance equality) — no such GO exists in today's content; if a
/// dungeon ever needs one, add the type here so it copies like the other interactive props.
const GO_COPY_TYPES: [u8; 4] = [
    crate::gameobject::go_type::DOOR,
    crate::gameobject::go_type::BUTTON,
    crate::gameobject::go_type::CHEST,
    crate::gameobject::go_type::GOOBER,
];

// ===========================================================================================
//  Tables [server] — neither is public/gateway-subscribed (see the module doc)
// ===========================================================================================

/// One live dungeon instance. `instance_id` auto_inc from 1 (0 = open world, reserved by
/// construction). `party_id` is the owning `game_group.group_id`, or 0 for a solo-created
/// instance (resolution for those rides `game_instance_binding` only — party_id 0 is NEVER
/// queried by `by_party`, or every solo instance in the world would alias one "party").
/// `last_empty_at_micros`: 0 = occupied/never-observed-empty; else when the reaper first saw it
/// empty. `reset_requested`: flagged by [`reset_instance`], honored by the reaper when empty. [server]
#[table(
    accessor = game_instance,
    index(accessor = by_party, btree(columns = [party_id])),
    index(accessor = by_map, btree(columns = [map_id]))
)]
pub struct GameInstance {
    #[primary_key]
    #[auto_inc]
    pub instance_id: u64,
    pub map_id: u32,
    pub party_id: u64,
    pub created_at: Timestamp,
    pub last_empty_at_micros: i64,
    pub reset_requested: bool,
}

/// One row per (character, map): the instance this character re-enters through that map's portal.
/// REPLACED on a new binding (resolve step 2/3 rebinds), survives party disband (vanilla), dropped
/// by the instance reap and by character deletion (the `character_owned` sweep below). [server]
#[table(
    accessor = game_instance_binding,
    index(accessor = by_character, btree(columns = [character_guid])),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct GameInstanceBinding {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub instance_id: u64,
    pub map_id: u32,
}

// A deleted character's instance bindings go with it (the tripwire-enforced sweep). Delete-only:
// bindings carry no owner_identity, so there is nothing to restamp (the GroupInvite precedent).
crate::character_owned!(delete, fn sweep_delete_game_instance_binding(ctx, character_guid) {
    let t = ctx.db.game_instance_binding();
    for b in t.by_character().filter(&character_guid).collect::<Vec<_>>() {
        t.id().delete(b.id);
    }
});
// CROSS-DATABASE transport (issue #19): the binding is what "you re-enter YOUR instance" means, and
// on the instance shard it is what `resolve_or_create_instance` reads when the character zones back
// in after a disconnect. `instance_id` is carried VERBATIM — it is the id the gateway mirrored to
// this shard via `ensure_instance`, so the two agree by construction. `id` is a surrogate PK.
crate::character_owned!(transfer, fn sweep_transfer_game_instance_binding(ctx, character_guid, io) {
    table = game_instance_binding,
    by = by_character,
    remint = id,
});

/// Drives the instance reaper (slice 3) — its OWN scheduled table per the `EventReaperSchedule`
/// precedent (gc.rs untouched by design). Seeded by `seed::init` at 60s; a live (auto-migrated)
/// node re-arms via `debug_repair_after_publish` (#378, formerly the standalone
/// `debug_rearm_instance_reaper`) (init does not re-run on a plain publish —
/// danger-zones "init only on fresh publish" rule). [server]
#[table(accessor = game_instance_reaper_schedule, scheduled(reap_instances))]
pub struct InstanceReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

// ===========================================================================================
//  Pure decisions (unit-tested below)
// ===========================================================================================

/// The resolve order, as data: the party's live instance → the character's own live binding →
/// create (issue #39 swapped the first two; see [`route_instance`]). Pure — the DB reads live in
/// [`resolve_or_create_instance`].
#[derive(Debug, PartialEq)]
pub(crate) enum InstanceRoute {
    Own(u64),
    Party(u64),
    Create,
}

pub(crate) fn route_instance(
    own_binding_live: Option<u64>,
    party_live: Option<u64>,
) -> InstanceRoute {
    match (own_binding_live, party_live) {
        // THE PARTY OUTRANKS THE PERSONAL BINDING (issue #39 defect 3). This used to be the other
        // way round, which SPLIT a party: a member who had already entered solo carried a binding
        // to their own `party_id = 0` instance, that binding won, and the rest of the party — who
        // could not see a solo instance through `by_party` — created a second dungeon. Vanilla's
        // rule is the one here: a non-saved 5-man takes the GROUP's instance, and the personal
        // binding is what you fall back to when you have no group (or your group has not entered
        // yet). It still survives disband, because a party-less character reaches the `Own` arm.
        (_, Some(id)) => InstanceRoute::Party(id),
        (Some(id), None) => InstanceRoute::Own(id),
        (None, None) => InstanceRoute::Create,
    }
}

/// The 5-player dungeon cap, enforced at trigger time. `group::GROUP_MAX_MEMBERS` already caps
/// parties at 5, so this is defense-in-depth — but the design says enforce it HERE too, so a
/// future raid-group shape can't silently walk a 40-man through a 5-man portal. Pure.
pub(crate) fn party_size_allows_entry(member_count: usize) -> bool {
    member_count <= crate::group::GROUP_MAX_MEMBERS
}

/// What the reaper does to one instance this firing, given its observed occupancy. Pure —
/// unit-tested; the entity scan and row writes live in [`reap_instances`].
#[derive(Debug, PartialEq)]
pub(crate) enum OccupancyAction {
    /// Players inside and a stale empty-stamp → clear it (re-entry cancels the countdown).
    ClearStamp,
    /// Empty and unstamped → start the 30min countdown now.
    Stamp,
    /// Nothing to do this firing.
    Wait,
    /// Tear it down (30min empty elapsed, or reset requested while empty).
    Reap,
}

pub(crate) fn occupancy_action(
    occupied: bool,
    last_empty_at_micros: i64,
    reset_requested: bool,
    now_micros: i64,
    empty_reap_micros: i64,
) -> OccupancyAction {
    if occupied {
        return if last_empty_at_micros != 0 {
            OccupancyAction::ClearStamp
        } else {
            OccupancyAction::Wait
        };
    }
    if reset_requested {
        // Reset honors the CURRENT emptiness observation, not the stamp — a leader's reset lands
        // on the next firing (≤60s), never waits out the 30min countdown.
        return OccupancyAction::Reap;
    }
    if last_empty_at_micros == 0 {
        return OccupancyAction::Stamp;
    }
    if now_micros - last_empty_at_micros >= empty_reap_micros {
        OccupancyAction::Reap
    } else {
        OccupancyAction::Wait
    }
}

/// Where a login whose `pending_instance_id` was reaped lands (NEVER strand — design trap #3).
/// Pure over the two facts the caller derives ([`entrance_fallback`], [`is_dungeon_map`]);
/// alive-or-ghost is orthogonal and preserved per 226's `pending_ghost` rules at the call site.
#[derive(Debug, PartialEq)]
pub(crate) enum StrandingFallback {
    /// A known dungeon map → its entrance const, instance 0.
    Entrance(u32, f32, f32, f32, f32),
    /// A NON-dungeon map (the dev-map fixture case): the position is fine open-world terrain —
    /// stay in place, drop to instance 0.
    InPlaceOpenWorld,
    /// A dungeon map with no entrance arm (a [`lyracore_shared::instance::DUNGEON_MAPS`] entry missing its
    /// [`entrance_fallback`] — unit-pinned to never happen, but never strand if it does):
    /// hearthstone home, instance 0.
    HearthstoneHome,
}

pub(crate) fn stranding_fallback(
    entrance: Option<(u32, f32, f32, f32, f32)>,
    is_dungeon: bool,
) -> StrandingFallback {
    match (entrance, is_dungeon) {
        (Some((m, x, y, z, o)), _) => StrandingFallback::Entrance(m, x, y, z, o),
        (None, false) => StrandingFallback::InPlaceOpenWorld,
        (None, true) => StrandingFallback::HearthstoneHome,
    }
}

#[allow(dead_code)] // core kept for a future gw_reset_instance twin (#483 deleted the sender-path reducer)
/// May this caller flag this instance for reset? Vanilla "Reset all instances" semantics: never
/// while players are inside; a party instance takes its CURRENT leader, a solo instance
/// (`party_id == 0`) takes any character bound to it (the caller reaches it via their own binding,
/// which IS the bound-to proof). Pure.
pub(crate) fn reset_eligible(party_id: u64, caller_is_leader: bool, occupied: bool) -> bool {
    !occupied && (party_id == 0 || caller_is_leader)
}

// ===========================================================================================
//  Entry: resolve-or-create (the 225 areatrigger hook's target)
// ===========================================================================================

/// A binding/party instance is LIVE for resolution iff its row still exists, is for the right
/// map, and is not flagged for reset (a reset-flagged instance is already condemned — resolving
/// into it would race the reaper; vanilla's reset invalidates the old id immediately).
fn live_instance_for(ctx: &ReducerContext, instance_id: u64, map_id: u32) -> bool {
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .map(|i| i.map_id == map_id && !i.reset_requested)
        .unwrap_or(false)
}

/// Bind `character_guid` to `instance_id` for `map_id` — one row per (character, map), REPLACE on
/// a new binding (delete-then-insert).
fn bind_character(ctx: &ReducerContext, character_guid: u64, instance_id: u64, map_id: u32) {
    let t = ctx.db.game_instance_binding();
    let stale: Vec<u64> = t
        .by_character()
        .filter(&character_guid)
        .filter(|b| b.map_id == map_id)
        .map(|b| b.id)
        .collect();
    for id in stale {
        t.id().delete(id);
    }
    t.insert(GameInstanceBinding {
        id: 0,
        character_guid,
        instance_id,
        map_id,
    });
}

/// The dungeon-entry chokepoint (190 slice 2): resolve which instance of `target_map` this
/// character enters — party's live instance → own live binding → create (#39) — enforcing the 5-player
/// cap at trigger time. Solo entry allowed (binds to the character, `party_id = 0`). A stale
/// binding (instance reaped or reset-flagged) self-heals: the row is dropped and resolution falls
/// through. Returns the instance id to teleport into, or a loud `Err` (cap breach — surfaced by
/// the caller as a warn; there is no "instance full" wire packet in 1.12's portal flow).
pub(crate) fn resolve_or_create_instance(
    ctx: &ReducerContext,
    character_guid: u64,
    target_map: u32,
) -> Result<u64, String> {
    let group = crate::group::group_of(ctx, character_guid);
    let member_count = group
        .as_ref()
        .map(|g| crate::group::members_of(ctx, g.group_id).len())
        .unwrap_or(1);
    if !party_size_allows_entry(member_count) {
        return Err(format!(
            "party of {member_count} exceeds the {}-player dungeon cap",
            crate::group::GROUP_MAX_MEMBERS
        ));
    }

    // 1. The character's own binding for this map, if its instance is still live. A dead binding
    //    is dropped here (self-heal) so it never shadows the party's live instance below.
    let bindings = ctx.db.game_instance_binding();
    let own_binding = bindings
        .by_character()
        .filter(&character_guid)
        .find(|b| b.map_id == target_map);
    let own_live = match own_binding {
        Some(b) if live_instance_for(ctx, b.instance_id, target_map) => Some(b.instance_id),
        Some(b) => {
            bindings.id().delete(b.id);
            None
        }
        None => None,
    };

    // 2. The party's live instance for this map (party_id 0 — solo — is never queried: it is the
    //    "no party" sentinel, not a party; see the GameInstance doc).
    let party_live = group.as_ref().and_then(|g| {
        ctx.db
            .game_instance()
            .by_party()
            .filter(&g.group_id)
            .find(|i| i.map_id == target_map && !i.reset_requested)
            .map(|i| i.instance_id)
    });

    let party_id = group.as_ref().map(|g| g.group_id).unwrap_or(0);
    match route_instance(own_live, party_live) {
        InstanceRoute::Own(id) => {
            // ADOPTION (issue #39 defect 3, the other half of the split): the holder of a SOLO
            // instance who has since joined a party is the first of that party through the portal,
            // and `by_party` cannot see a `party_id = 0` instance — so without this the members
            // behind them would mint a second dungeon and the party would play in two of them.
            // Re-stamping the owner makes the instance they are walking into the party's, which is
            // also the cheap outcome: nobody re-spawns a population that already exists. Only an
            // UNOWNED (solo) instance is adopted — an instance still owned by some other party is
            // never stolen.
            adopt_instance_for_party(ctx, id, party_id);
            Ok(id)
        }
        InstanceRoute::Party(id) => {
            bind_character(ctx, character_guid, id, target_map);
            Ok(id)
        }
        InstanceRoute::Create => {
            let id = create_instance(ctx, target_map, party_id)?;
            bind_character(ctx, character_guid, id, target_map);
            Ok(id)
        }
    }
}

/// Re-stamp an UNOWNED (`party_id == 0`, i.e. solo-created) instance as `party_id`'s, so the rest
/// of that party resolves into it through `by_party` instead of creating a second one. No-op when
/// the caller has no party, when the instance is already owned, or when the row is gone.
fn adopt_instance_for_party(ctx: &ReducerContext, instance_id: u64, party_id: u64) {
    if party_id == 0 {
        return;
    }
    let instances = ctx.db.game_instance();
    if let Some(mut inst) = instances.instance_id().find(instance_id) {
        if inst.party_id == 0 {
            inst.party_id = party_id;
            instances.instance_id().update(inst);
            log::info!("instance {instance_id} adopted by party {party_id}");
        }
    }
}

// ===========================================================================================
//  create_instance — row + population + GO copies + tick row
// ===========================================================================================

/// Does THIS database host dungeon-instance POPULATIONS (`game_config.hosts_instances`, issue #39)?
/// A missing config row reads `true`, so a database that predates the column — or a fresh one whose
/// seed has not run — behaves exactly as it did before the flag existed. The `nav::nav_enabled`
/// shape, deliberately: a world policy read at the one decision point, not a shard id.
pub(crate) fn hosts_instance_populations(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.hosts_instances)
        .unwrap_or(true)
}

/// Create a live instance of `map_id` owned by `party_id` (0 = solo): insert the `game_instance`
/// row, spawn the per-instance creature population, and copy the map's interactive gameobjects. The
/// creature tick rides the global catch-all row (perf catalog 1.3). Entity-only population — see the module doc's
/// respawn-within-a-run decision. Returns the new instance id.
pub(crate) fn create_instance(
    ctx: &ReducerContext,
    map_id: u32,
    party_id: u64,
) -> Result<u64, String> {
    create_instance_with_id(ctx, 0, map_id, party_id)
}

/// [`create_instance`] with the id supplied by the caller (`0` = let `auto_inc` allocate, the
/// normal path). The explicit form exists for ONE caller: [`ensure_instance`], the gateway's
/// cross-database mirror (issue #19), which has to re-create the SAME instance id on the shard that
/// owns the map so every party member's `game_instance_binding` — which travels in their transfer
/// blob carrying that id verbatim — still resolves.
pub(crate) fn create_instance_with_id(
    ctx: &ReducerContext,
    instance_id: u64,
    map_id: u32,
    party_id: u64,
) -> Result<u64, String> {
    let inst = ctx.db.game_instance().insert(GameInstance {
        instance_id, // 0 → auto_inc allocates from 1, so 0 stays the open world
        map_id,
        party_id,
        created_at: ctx.timestamp,
        last_empty_at_micros: 0, // occupied-until-observed-otherwise (the party is mid-teleport)
        reset_requested: false,
    });
    let instance_id = inst.instance_id;

    // --- THE HOSTING GATE (issue #39 defect 1). ------------------------------------------------
    // A database that does not host instance populations files the row + (the caller's) binding and
    // spawns NOTHING: a LEASE, the exact shape `teardown_instance_inner(delete_row = false)` leaves
    // behind after a cross-database eviction. On the open-world shard of a Phase A deployment this
    // is the whole fix for "the portal spawned 207 creatures + 28 GO copies on the writer that is
    // not going to run the dungeon" — the population is spawned on the shard that owns the map,
    // when the gateway mirrors this id there via `ensure_instance`, which runs on a database where
    // this flag is (and stays) true. Default true ⇒ a single-database realm is unchanged.
    if !hosts_instance_populations(ctx) {
        log::info!(
            "create_instance: instance {instance_id} map {map_id} party {party_id} — LEASE only \
             (game_config.hosts_instances is off: this database does not host instance populations)"
        );
        return Ok(instance_id);
    }

    // --- Population: every spawn template on the map through the NORMAL spawn path. -----------
    // Guid allocation: wave-guid layout with the INSTANCE_POP_LOW_BAND bit (see the module doc's
    // namespace section). One prepass over live entities builds the per-entry max BAND-low so
    // consecutive creates (and two live instances of the same map) never collide.
    let entities = ctx.db.game_world_entity();
    let mut max_band_low: HashMap<u32, u64> = HashMap::new();
    for e in entities.iter() {
        if e.is_player() || e.entry == 0 {
            continue;
        }
        let low = e.guid & 0xFF_FFFF;
        if low & INSTANCE_POP_LOW_BAND != 0 {
            let seq = low & (INSTANCE_POP_LOW_BAND - 1);
            let slot = max_band_low.entry(e.entry).or_insert(0);
            if seq > *slot {
                *slot = seq;
            }
        }
    }
    // Tracked wave/summon spawn rows are NOT population (190 review HIGH): 227's spawn_wave
    // inserts REAL untagged game_creature_spawn rows on the dungeon map (Sneed, VanCleef adds)
    // that persist until THEIR instance's reset/sweep — without this exclusion, a fresh instance
    // created while another run is live (or recently dead) would spawn a pre-summoned Sneed
    // standing on the wreck and duplicate mid-fight adds. Every wave row is tracked in
    // game_encounter_spawn by definition — that table IS the wave registry.
    let tracked_wave_guids: std::collections::HashSet<u64> = ctx
        .db
        .game_encounter_spawn()
        .iter()
        .map(|t| t.guid)
        .collect();
    let spawns: Vec<crate::creatures::CreatureSpawn> = ctx
        .db
        .game_creature_spawn()
        .iter()
        .filter(|s| s.map_id == map_id && !tracked_wave_guids.contains(&s.guid))
        .collect();
    let templates = ctx.db.game_creature_template();
    let mut spawned = 0u32;
    for spawn in &spawns {
        let Some(tmpl) = templates.entry().find(spawn.entry) else {
            continue; // template-less spawn row: the open-world respawn pass skips it too
        };
        let slot = max_band_low.entry(spawn.entry).or_insert(0);
        *slot += 1;
        if *slot >= INSTANCE_POP_LOW_BAND {
            // 2^23 live copies of ONE entry — unreachable in practice; fail LOUD, never wrap into
            // the spawn-row namespace.
            return Err(format!(
                "instance population low exhausted for entry {}",
                spawn.entry
            ));
        }
        let mut entity =
            crate::creatures::build_creature_entity(spawn, &tmpl, ctx.random(), instance_id);
        entity.guid = crate::encounter::wave_guid(spawn.entry, INSTANCE_POP_LOW_BAND | *slot);
        crate::creatures::insert_creature_entity(ctx, entity);
        spawned += 1;
    }

    // --- GO copies: the map's interactive props (type-gated), one copy per instance. -----------
    // Source rows are the STATIC (instance 0) spawns; copies carry the new instance id and a
    // GO_COPY_BAND guid. `state` is copied (startOpen doors keep their initial state).
    let gos = ctx.db.game_gameobject();
    let mut next_go_seq = gos
        .iter()
        .filter(|g| {
            g.guid >> 48 == 0xF110
                && g.guid & GO_COPY_BAND != 0
                && g.guid & crate::gameobject::POOL_TAG == 0
        })
        .map(|g| g.guid & (GO_COPY_BAND - 1))
        .max()
        .unwrap_or(0)
        + 1;
    let sources: Vec<crate::gameobject::GameObject> = gos
        .iter()
        .filter(|g| {
            g.map_id == map_id
                && g.instance_id == 0
                && g.guid & crate::gameobject::POOL_TAG == 0
                && g.guid & GO_COPY_BAND == 0
                && ctx
                    .db
                    .game_gameobject_template()
                    .entry()
                    .find(g.template_entry)
                    .map(|t| GO_COPY_TYPES.contains(&t.type_id))
                    .unwrap_or(false)
        })
        .collect();
    let mut copied = 0u32;
    for src in &sources {
        gos.insert(crate::gameobject::GameObject {
            guid: (0xF110u64 << 48) | GO_COPY_BAND | next_go_seq,
            template_entry: src.template_entry,
            map_id: src.map_id,
            x: src.x,
            y: src.y,
            z: src.z,
            orientation: src.orientation,
            state: src.state,
            created_at: ctx.timestamp,
            respawn_at_micros: 0,
            instance_id,
            grid_x: lyracore_shared::spatial::grid_cell(src.x, src.y).0,
            grid_y: lyracore_shared::spatial::grid_cell(src.x, src.y).1,
            cell: lyracore_shared::spatial::cell_id_at(src.x, src.y),
            // Copy the source door/chest/goober's real spawn quaternion (#515) — a dungeon copy is
            // the same prop at the same orientation, not a reset to identity.
            rotation_0: src.rotation_0,
            rotation_1: src.rotation_1,
            rotation_2: src.rotation_2,
            rotation_3: src.rotation_3,
        });
        next_go_seq += 1;
        copied += 1;
    }

    // --- NO dedicated per-instance tick row (perf catalog 1.3). This used to unconditionally insert
    // one at INSTANCE_TICK_INTERVAL_MICROS — the SAME cadence as the global catch-all, so it bought
    // zero latency smoothing while each firing paid the FIXED per-firing costs the partition does not
    // divide: a transaction commit on the serialized writer, a TickScope rebuild, active_cell_radius,
    // active_cell_creatures' player scan, and the pet phase's candidate list on sense ticks. With M live
    // instances that is M extra tx/s each carrying an O(E) scan, for creature populations that are
    // stationary-until-aggro. The catch-all covers every instance with no dedicated row
    // (`TickScope::from_rows`) — the exact coverage teardown already relies on (step 5 below deletes
    // the row and lets the catch-all take over), so instance creatures tick identically, just inside
    // the catch-all's transaction. An operator who wants a genuinely FASTER cadence for one instance
    // still arms one explicitly via `debug_arm_instance_tick`.

    log::info!(
        "create_instance: instance {instance_id} map {map_id} party {party_id} — {spawned} creatures, {copied} GO copies"
    );
    Ok(instance_id)
}

// ===========================================================================================
//  Reap (slice 3) — scheduled reducer + shared teardown
// ===========================================================================================

// ===========================================================================================
//  Cross-database instance placement (issue #19) — the two calls the gateway drives
// ===========================================================================================

/// **Destination side.** Mirror instance `instance_id` of `map_id` onto THIS database, spawning its
/// population if it isn't here yet. Idempotent: the second party member through the portal finds
/// the instance already live and joins it.
///
/// The id is supplied, not allocated, because it was allocated on the SOURCE shard — the
/// areatrigger hook runs where the player was standing, and the module deliberately knows nothing
/// about shards (spec #12: "no module game logic ever reads a shard id"). Every member's
/// `game_instance_binding` carries that id verbatim in their transfer blob, so mirroring it is what
/// makes the party land in one instance.
///
/// Operator-gated: orchestration machinery, never a client action.
#[reducer]
pub fn ensure_instance(
    ctx: &ReducerContext,
    instance_id: u64,
    map_id: u32,
    party_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if instance_id == 0 {
        return Err("instance 0 is the open world — it is never mirrored".to_string());
    }
    if let Some(existing) = ctx.db.game_instance().instance_id().find(instance_id) {
        if existing.map_id != map_id {
            return Err(format!(
                "instance {instance_id} already exists here on map {} — refusing to mirror it as \
                 map {map_id} (two shards allocated the same id, which the shared realm index is \
                 meant to prevent)",
                existing.map_id
            ));
        }
        return Ok(());
    }
    let id = create_instance_with_id(ctx, instance_id, map_id, party_id)?;
    log::info!("ensure_instance: mirrored instance {id} (map {map_id}, party {party_id})");
    Ok(())
}

/// **Source side.** Evict an instance's POPULATION from this database, keeping the `game_instance`
/// row and its bindings as a lease (see [`teardown_instance_inner`]).
///
/// This is what makes issue #19 AC#2 true: the world shard spawned the dungeon when the first
/// player stepped through the portal, but the run happens on the instance shard, so the world
/// writer must stop ticking its creatures. Refuses while a live player is inside — the same guard
/// `teardown_instance` has, and here it is load-bearing: a member who has NOT transferred yet is a
/// live player in that instance on this database, and evicting around them would delete the
/// creatures they are standing next to.
///
/// Operator-gated.
#[reducer]
pub fn evict_instance_population(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if instance_id == 0 {
        return Err("instance 0 is the open world — it is never evicted".to_string());
    }
    if ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_none()
    {
        return Ok(()); // nothing here to evict — replay-safe
    }
    teardown_instance_inner(ctx, instance_id, false);
    Ok(())
}

/// The instance reaper (scheduled, scheduler-only): one entity pass classifies every instance's
/// player-occupancy, then each instance stamps/clears/waits/reaps per [`occupancy_action`].
/// Teardown is batched ([`REAP_MAX_PER_FIRING`]); leftovers reap next firing (60s later).
///
/// **Reaper LOCALITY (#21 AC#1).** Every database runs its own reaper over its own
/// `game_instance` rows and nothing else — there is no cross-database sweep and no shard id
/// anywhere in it. A database that owns no instances therefore does no work at all: the row scan
/// below finds nothing and returns *before* the O(entities) occupancy pass, so an open-world writer
/// in a Phase A deployment pays for the reaper only while it is actually holding leases, and a
/// pool member pays only for the runs it hosts. The empty timer itself is per-database too — see
/// [`empty_reap_micros`].
#[reducer]
pub fn reap_instances(ctx: &ReducerContext, _schedule: InstanceReaperSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return; // scheduler-only (the gc.rs guard)
    }
    let instances: Vec<GameInstance> = ctx.db.game_instance().iter().collect();
    if instances.is_empty() {
        // No instances on THIS database → nothing to classify. Skipping the entity scan is what
        // makes "the reaper lives on the owning database" a cost statement and not just a comment.
        return;
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let reap_after = empty_reap_micros(hosts_instance_populations(ctx));
    let occupied = occupied_instances(ctx);
    let mut reaped = 0usize;
    for mut inst in instances {
        match occupancy_action(
            occupied.contains(&inst.instance_id),
            inst.last_empty_at_micros,
            inst.reset_requested,
            now,
            reap_after,
        ) {
            OccupancyAction::ClearStamp => {
                inst.last_empty_at_micros = 0;
                ctx.db.game_instance().instance_id().update(inst);
            }
            OccupancyAction::Stamp => {
                inst.last_empty_at_micros = now;
                ctx.db.game_instance().instance_id().update(inst);
            }
            OccupancyAction::Reap => {
                if reaped < REAP_MAX_PER_FIRING {
                    teardown_instance(ctx, inst.instance_id);
                    reaped += 1;
                }
                // Over-batch instances keep their stamp and reap next firing.
            }
            OccupancyAction::Wait => {}
        }
    }
}

/// The set of instance ids with at least one live PLAYER entity — one pass classifies every
/// instance at once (playerbots count: a parked bot holds its instance open, correctly).
///
/// Plus every instance CLAIMED by an in-transit character (issue #30, REFUSE verdict). Occupancy is
/// counted from live entities, and `begin_transfer` deletes the live entity — so an instance whose
/// only occupant is mid-transfer would read as empty and get torn down, deleting its
/// `game_instance_binding` rows (a manifest table) out from under a transfer another shard is still
/// driving. Both ends of the hop are held: the escrow's destination (where the character is going)
/// and the durable row's `pending_instance_id` (where `begin_transfer` parked its source instance).
fn occupied_instances(ctx: &ReducerContext) -> HashSet<u64> {
    let mut occupied: HashSet<u64> = ctx
        .db
        .game_world_entity()
        .iter()
        .filter(|e| e.is_player() && e.instance_id != 0)
        .map(|e| e.instance_id)
        .collect();
    occupied.extend(crate::transfer::in_transit_instances(ctx));
    occupied
}

/// Tear one instance down, in the design's order: population (entities + their combat/threat/leg/
/// loot state, player corpses, GO copies + chest loot) → the 228 encounter-kernel sweep splice →
/// the 229 tick row → bindings → the `game_instance` row itself. Refuses instance 0 (the open
/// world) and any instance with a live player inside (belt over the caller's occupancy check —
/// same-transaction, so no race). Shared by the reaper and `debug_reap_instance`.
pub(crate) fn teardown_instance(ctx: &ReducerContext, instance_id: u64) {
    teardown_instance_inner(ctx, instance_id, true)
}

/// [`teardown_instance`], optionally KEEPING the `game_instance` row and its bindings (issue #19).
///
/// `keep_lease = true` is the cross-database eviction: the world shard created this instance when
/// the first player stepped through the portal (the areatrigger hook runs there, and the module
/// deliberately knows nothing about shards), but the RUN happens on the instance shard. Deleting
/// the population here is what makes the world writer stop paying for it; keeping the row is what
/// makes the NEXT party member's `resolve_or_create_instance` resolve to the SAME id instead of
/// minting a second instance the rest of the party is not in.
///
/// Note: the leased row is a stub the world shard never populates again, and it is left to the
/// reaper's empty timer — but NOT the 30-minute one. A lease reads empty seconds after the party
/// transfers out, so a lease-only database reaps on [`INSTANCE_LEASE_REAP_MICROS`] instead (#21);
/// the 30-minute run timer would delete the world shard's record of "this party is in instance N"
/// while they were still in it. The cost is one row per run for the length of that timer. Upgrade
/// path: the instance→shard index moves to realm-core (#22) and neither the lease nor this arm
/// exists.
pub(crate) fn teardown_instance_inner(ctx: &ReducerContext, instance_id: u64, delete_row: bool) {
    if instance_id == 0 {
        return; // never "reap" the open world
    }
    let entities = ctx.db.game_world_entity();
    if entities
        .iter()
        .any(|e| e.instance_id == instance_id && e.is_player())
    {
        log::warn!("teardown_instance: instance {instance_id} has live players — refused");
        return;
    }

    // 1. Population: every non-player entity in the instance (alive or corpse), through the ONE
    //    canonical creature-teardown checklist (issue #359 — this loop, `encounter::despawn_tracked`
    //    and `creatures::tick::pass_decay` had each grown their own copy, and they had diverged on
    //    the loot family, the taunt lock and the #50 withheld gate).
    let doomed: Vec<u64> = entities
        .iter()
        .filter(|e| e.instance_id == instance_id && !e.is_player())
        .map(|e| e.guid)
        .collect();
    for guid in &doomed {
        crate::creatures::despawn_creature_entity(ctx, *guid);
    }

    // 2. Player corpses left in the instance (a ghost who never corpse-ran back). The owner's
    //    stranding outcome is the vanilla one: nothing left to reclaim — spirit healer only
    //    (documented on `player_login`'s fallback and in the runbook).
    let corpses = ctx.db.game_corpse();
    let dead_bodies: Vec<u64> = corpses
        .iter()
        .filter(|c| c.instance_id == instance_id)
        .map(|c| c.guid)
        .collect();
    for guid in dead_bodies {
        corpses.guid().delete(guid);
    }

    // 3. GO copies + any chest loot rows keyed on a copy's guid.
    let gos = ctx.db.game_gameobject();
    let copies: Vec<u64> = gos
        .iter()
        .filter(|g| g.instance_id == instance_id)
        .map(|g| g.guid)
        .collect();
    for guid in &copies {
        crate::loot::reap_corpse_loot_family(ctx, *guid); // a GO is not a creature — the loot family only
        gos.guid().delete(guid);
    }

    // 4. Encounter-kernel state — the 228 splice (documented on sweep_encounter_state): tracked
    //    waves (their untagged spawn rows MUST die here or they'd respawn into instance 0),
    //    encounter state + HP fired-marks, equip rows.
    crate::encounter::sweep_encounter_state(ctx, instance_id);

    // 5. The dedicated 229 tick row (debug_disarm_instance_tick's body) — coverage of the (now
    //    empty) id falls back to the catch-all, which is a no-op for a population of zero.
    let sched = ctx.db.game_creature_move_schedule();
    let ticks: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == instance_id)
        .map(|r| r.scheduled_id)
        .collect();
    for id in ticks {
        sched.scheduled_id().delete(id);
    }

    // 6/7. Bindings, then the instance row itself, last — the design's documented order ends at the
    //    row (within one reducer transaction the order is atomic anyway; keeping it makes the
    //    invariant readable: population never outlives its row).
    //
    //    SKIPPED for a cross-database eviction (`delete_row == false`, issue #19): the row is a
    //    LEASE the world shard keeps so the party's later arrivals resolve to the same instance,
    //    and the bindings are the manifest rows that just travelled to the instance shard in each
    //    member's transfer blob — deleting them here would delete the source copy of state the
    //    destination now owns, which is the one thing a transfer must never do.
    if !delete_row {
        log::info!(
            "teardown_instance: instance {instance_id} population EVICTED ({} entities, {} GO \
             copies) — row + bindings kept as a cross-shard lease",
            doomed.len(),
            copies.len()
        );
        return;
    }
    let bindings = ctx.db.game_instance_binding();
    let stale: Vec<u64> = bindings
        .by_instance()
        .filter(&instance_id)
        .map(|b| b.id)
        .collect();
    for id in stale {
        bindings.id().delete(id);
    }
    ctx.db.game_instance().instance_id().delete(instance_id);
    log::info!(
        "teardown_instance: instance {instance_id} reaped ({} entities, {} GO copies)",
        doomed.len(),
        copies.len()
    );
}

// ===========================================================================================
//  reset_instance — the party-leader / solo reset verb (slice 3 item 8)
// ===========================================================================================

#[allow(dead_code)] // core kept for a future gw_reset_instance twin (#483 deleted the sender-path reducer)
/// The [`reset_instance`] body keyed off an explicit guid. Walks the caller's OWN bindings (being
/// bound is the reach — vanilla resets the instances you're bound to), gating each per
/// [`reset_eligible`]. Returns how many instances were flagged, `Err` if none were eligible.
pub(crate) fn apply_reset_instances(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<u32, String> {
    let occupied = occupied_instances(ctx);
    let bindings: Vec<GameInstanceBinding> = ctx
        .db
        .game_instance_binding()
        .by_character()
        .filter(&character_guid)
        .collect();
    let mut flagged = 0u32;
    for b in bindings {
        let Some(mut inst) = ctx.db.game_instance().instance_id().find(b.instance_id) else {
            continue; // already reaped — the binding dies on the reap/next resolve
        };
        let caller_is_leader = ctx
            .db
            .game_group()
            .group_id()
            .find(inst.party_id)
            .map(|g| g.leader_guid == character_guid)
            .unwrap_or(false);
        if !reset_eligible(
            inst.party_id,
            caller_is_leader,
            occupied.contains(&inst.instance_id),
        ) {
            continue;
        }
        if !inst.reset_requested {
            inst.reset_requested = true;
            ctx.db.game_instance().instance_id().update(inst);
            flagged += 1;
        }
    }
    if flagged == 0 {
        Err("no unoccupied instance eligible for reset".to_string())
    } else {
        Ok(flagged)
    }
}

// ===========================================================================================
//  Tests — the pure decisions above (the module crate has no ReducerContext harness by design)
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dungeon_map_set_contains_deadmines_and_no_open_world_map() {
        assert!(is_dungeon_map(36), "Deadmines is the one imported dungeon");
        assert!(!is_dungeon_map(0), "Eastern Kingdoms is open world");
        assert!(!is_dungeon_map(1), "Kalimdor is open world");
    }

    #[test]
    fn every_dungeon_map_has_an_entrance_fallback_arm() {
        // The stranding guard's invariant: a reaped-instance login on ANY dungeon map must have an
        // entrance to fall back to (the HearthstoneHome branch is the never-strand net for a
        // violation of exactly this pin). Checked against the SHARED (cross-tier) id set, not the
        // module-local DUNGEON_MAPS table — see `every_shared_dungeon_map_has_a_dungeon_maps_record`
        // for why that distinction is the actual tripwire (issue #376).
        for &m in SHARED_DUNGEON_MAP_IDS {
            assert!(
                entrance_fallback(m).is_some(),
                "dungeon map {m} has no entrance_fallback arm — logins after a reap would divert to hearthstone"
            );
        }
        // Entrance consts always land at instance-0-capable maps (the open world), never a dungeon.
        let (m, _, _, _, _) = entrance_fallback(36).unwrap();
        assert!(
            !is_dungeon_map(m),
            "an entrance fallback must land on an open-world map"
        );
    }

    /// The cross-tier tripwire (issue #376): [`lyracore_shared::instance::DUNGEON_MAPS`] is the id
    /// list `gateway::config::ShardMap::check_instance_hosting` walks (issue #48) — every id in it
    /// MUST have a full record in this crate's [`DUNGEON_MAPS`], or a login/release on that map
    /// would silently take the "not a dungeon" path every helper here takes for an absent
    /// `DungeonMap`. This is the test-time replacement for the runtime `warn!` `resolve_graveyard`
    /// used to log the first time a release actually hit a half-configured map (deleted by #376 — a
    /// `DungeonMap` has no optional fields, so once an id clears this pin it can never regress into
    /// a half-configured state).
    #[test]
    fn every_shared_dungeon_map_has_a_dungeon_maps_record() {
        for &m in SHARED_DUNGEON_MAP_IDS {
            assert!(
                dungeon(m).is_some(),
                "map {m} is in lyracore_shared::instance::DUNGEON_MAPS (the gateway's \
                 hosting-check set) but has no module::instance::DUNGEON_MAPS record"
            );
        }
        for d in DUNGEON_MAPS {
            assert!(
                is_dungeon_map(d.map_id),
                "map {} has a module::instance::DUNGEON_MAPS record but is missing from \
                 lyracore_shared::instance::DUNGEON_MAPS — its instance-hosting would never be \
                 checked at gateway startup (issue #48)",
                d.map_id
            );
        }
    }

    #[test]
    fn route_prefers_the_party_then_the_personal_binding_then_create() {
        // Issue #39 defect 3 — THE SPLIT. Ginger entered Deadmines solo (instance 4, `party_id 0`),
        // then partied up; the party's own entry made instance 5. With the personal binding ranked
        // first she re-entered 4 while her party was in 5 — two dungeons, one party. The party now
        // wins, which is also vanilla's rule for a non-saved 5-man.
        assert_eq!(route_instance(Some(7), Some(9)), InstanceRoute::Party(9));
        // No party instance → the personal binding still carries you back into your own dungeon,
        // which is what makes a binding survive a disband (a party-less character lands here).
        assert_eq!(route_instance(Some(7), None), InstanceRoute::Own(7));
        assert_eq!(route_instance(None, Some(9)), InstanceRoute::Party(9));
        assert_eq!(route_instance(None, None), InstanceRoute::Create);
    }

    /// #39 defect 3, the half a pure function cannot express: the FIRST party member through the
    /// portal may be the one holding the solo binding, and `by_party` cannot see a `party_id = 0`
    /// instance — so the members behind them would mint a second dungeon unless the instance they
    /// walk into is re-stamped as the party's. Source-scanned (the `transfer.rs` tripwire pattern):
    /// the module crate has no `ReducerContext` harness, and deleting this call site left every
    /// other test in this file green.
    #[test]
    fn the_own_binding_arm_adopts_a_solo_instance_into_the_callers_party() {
        let body = code_of(
            include_str!("instance.rs"),
            "pub(crate) fn resolve_or_create_instance(",
        );
        assert!(
            body.contains("adopt_instance_for_party"),
            "resolve_or_create_instance's Own arm no longer adopts a solo instance into the \
             caller's party. Without it, a party formed AFTER one member's solo entry splits: that \
             member re-enters their own instance and everyone else creates a second one (#39)."
        );
        // ...and adoption must never STEAL an instance another party already owns.
        let adopt = code_of(include_str!("instance.rs"), "fn adopt_instance_for_party(");
        assert!(
            adopt.contains("inst.party_id == 0"),
            "adopt_instance_for_party no longer restricts itself to UNOWNED (solo) instances — it \
             would re-stamp another party's dungeon as this caller's"
        );
        // Adversarial review: a call site plus a guard TEXT is not adoption. Inverting the caller's
        // own no-party guard (`party_id == 0` → `!= 0`) leaves every string above present and every
        // test in this crate green while adoption never runs once — the exact dead-code shape that
        // has defeated this repo's source scans before. Pin the guard's SENSE, not just its words.
        assert!(
            adopt.contains("if party_id == 0 {"),
            "adopt_instance_for_party's no-party guard changed sense — it must return early for a \
             caller with NO party (0) and adopt for everyone else; inverted, adoption is dead code"
        );
        // The party lookup this all hangs off must stay MAP-SCOPED. A party-first order that
        // resolves the group's instance on ANY map sends a member who walks into the Stockades
        // portal into the party's Deadmines instance — and binds them to it under `target_map`.
        assert!(
            body.contains("i.map_id == target_map"),
            "the party's live-instance lookup is no longer filtered to the map being entered — \
             party-first would resolve a member into the group's instance of a DIFFERENT dungeon"
        );
        // Both resolve arms that hand a character an instance they were not already bound to must
        // BIND them (Party and Create). A Party arm that skips it re-mints an instance on that
        // member's next entry — the split this ticket exists to close, one entry later.
        assert_eq!(
            body.matches("bind_character(ctx, character_guid, id, target_map)")
                .count(),
            2,
            "the Party and Create arms must BOTH bind the character to the instance they resolved"
        );
    }

    /// #39 defect 1: entering a portal for a map another database owns must never spawn the
    /// dungeon HERE. The spawn loop is a `ReducerContext` walk with no unit harness, so the gate in
    /// front of it is source-scanned — the mutation that matters (deleting the early return) leaves
    /// every behavioural test in the workspace green.
    #[test]
    fn instance_population_is_gated_on_this_database_hosting_instances() {
        let body = code_of(
            include_str!("instance.rs"),
            "pub(crate) fn create_instance_with_id(",
        );
        let gate = body.find("hosts_instance_populations(ctx)").expect(
            "create_instance_with_id no longer consults game_config.hosts_instances — the \
                     open-world shard is spawning dungeon populations it will never tick (#39)",
        );
        let spawn = body
            .find("build_creature_entity")
            .expect("create_instance_with_id no longer spawns a population at all");
        assert!(
            gate < spawn,
            "the hosting gate must come BEFORE the population spawn, or the world shard pays for \
             the dungeon anyway"
        );
        assert!(
            body[gate..spawn].contains("return Ok(instance_id)"),
            "the hosting gate no longer RETURNS — it must file the lease row and stop, not fall \
             through into the spawn loop"
        );
        // Adversarial review: `find` + an ordering + a `return` all pass for a gate that can never
        // FIRE — `if !hosts_instance_populations(ctx) && <anything false>` keeps every assertion
        // above green while the world shard spawns every dungeon. Pin the condition itself.
        assert!(
            body.contains("if !hosts_instance_populations(ctx) {"),
            "the hosting gate's condition grew an extra term — the gate must be exactly \
             `if !hosts_instance_populations(ctx)`, or it can be made unreachable while every \
             source scan above still passes"
        );
        // The reader itself must default to hosting, so a database with no config row (or one that
        // predates the column) behaves exactly as it did before #39.
        let reader = code_of(
            include_str!("instance.rs"),
            "pub(crate) fn hosts_instance_populations(",
        );
        assert!(
            reader.contains("unwrap_or(true)"),
            "hosts_instance_populations must default to TRUE — a missing game_config row would \
             otherwise turn every single-database realm's dungeons into empty rooms"
        );
        // ...and it must read the COLUMN. A mutation that kept the call, the default and the gate
        // but answered a constant left every test in this crate green.
        assert!(
            reader.contains("c.hosts_instances"),
            "hosts_instance_populations no longer reads game_config.hosts_instances — the gate is \
             wired to a constant and the world shard spawns dungeons regardless of the operator's \
             configuration"
        );
        // ...from the SINGLETON row. `game_config` is keyed on `id = 0` (seed::init, and every
        // operator SQL line in the runbook); reading any other id finds nothing, falls into
        // `unwrap_or(true)`, and hosts instances no matter what the operator configured — with
        // every assertion above still green. Adversarial review: this mutation survived.
        assert!(
            reader.contains("find(0)"),
            "hosts_instance_populations reads a game_config row other than the id=0 singleton — it \
             would find nothing and silently default to hosting on every shard"
        );
    }

    /// The single-database promise: `hosts_instances` must default to ON in BOTH places a value can
    /// come from — the auto-migration default for a live database that predates the column, and the
    /// `seed::init` insert for a fresh one. Either flipped to `false` silently turns every
    /// unconfigured realm's dungeons into empty rooms, and no behavioural test in this workspace
    /// would notice.
    #[test]
    fn hosting_defaults_to_on_in_both_the_column_default_and_the_seed() {
        let cfg = include_str!("config.rs");
        let decl = cfg
            .find("pub hosts_instances: bool")
            .expect("game_config.hosts_instances is gone — #39's routing gate has no input");
        assert!(
            cfg[..decl].trim_end().ends_with("#[default(true)]"),
            "hosts_instances must carry #[default(true)]: it is END-APPENDED to a LIVE table, and a \
             `false` default would switch dungeon spawning off on every existing database the moment \
             it auto-migrates"
        );
        // #377: `init` is a 4-line dispatcher over four banner-stratum fns now (see seed.rs's
        // header) — `game_config` is seeded in stratum 1, `seed_production_core`.
        let seed = code_of(
            include_str!("seed.rs"),
            "fn seed_production_core(ctx: &ReducerContext) {",
        );
        assert!(
            seed.contains("hosts_instances: true"),
            "seed::seed_production_core must seed hosts_instances = true — a fresh single-database \
             realm hosts its own dungeons, exactly as it did before #39"
        );
    }

    /// Isolate one fn body and strip its `//` prose, so a tripwire asserts on CODE and not on the
    /// comment that explains it. Shared as [`crate::test_scan::code_of`] (issue #64 — this used to
    /// be six near-identical, drifted-apart copies).
    use crate::test_scan::code_of;

    #[test]
    fn party_size_cap_admits_up_to_five_and_refuses_six() {
        for n in 1..=5 {
            assert!(party_size_allows_entry(n), "a party of {n} fits a 5-man");
        }
        assert!(
            !party_size_allows_entry(6),
            "a 6th player must be refused at the portal"
        );
    }

    #[test]
    fn occupancy_action_covers_stamp_clear_wait_and_both_reap_paths() {
        let now = 1_800_000_000_000_000i64;
        // The single-database timer — the ONLY one a realm without a shard map ever uses.
        let t = empty_reap_micros(true);
        assert_eq!(t, INSTANCE_EMPTY_REAP_MICROS);
        // Occupied: a stale stamp clears; no stamp → nothing.
        assert_eq!(
            occupancy_action(true, now - 1, false, now, t),
            OccupancyAction::ClearStamp
        );
        assert_eq!(
            occupancy_action(true, 0, false, now, t),
            OccupancyAction::Wait
        );
        // Occupied + reset_requested: NEVER reaped out from under live players.
        assert_eq!(
            occupancy_action(true, 0, true, now, t),
            OccupancyAction::Wait
        );
        // Empty, unstamped → start the countdown.
        assert_eq!(
            occupancy_action(false, 0, false, now, t),
            OccupancyAction::Stamp
        );
        // Empty, stamped just short of 30min → wait; exactly 30min → reap (inclusive boundary).
        let stamped = now - INSTANCE_EMPTY_REAP_MICROS;
        assert_eq!(
            occupancy_action(false, stamped + 1, false, now, t),
            OccupancyAction::Wait
        );
        assert_eq!(
            occupancy_action(false, stamped, false, now, t),
            OccupancyAction::Reap
        );
        // Reset + empty reaps IMMEDIATELY — no 30min wait, stamp or not.
        assert_eq!(
            occupancy_action(false, 0, true, now, t),
            OccupancyAction::Reap
        );
        assert_eq!(
            occupancy_action(false, now - 1, true, now, t),
            OccupancyAction::Reap
        );
    }

    /// **#21 AC#4** — reap semantics on a shard-pool deployment, stated against the single-database
    /// ones they must not change.
    #[test]
    fn a_lease_only_database_holds_its_stub_rows_far_past_the_thirty_minute_run_timer() {
        let now = 1_800_000_000_000_000i64;
        let hosting = empty_reap_micros(true);
        let lease = empty_reap_micros(false);
        assert_eq!(
            hosting, INSTANCE_EMPTY_REAP_MICROS,
            "hosting databases are UNCHANGED"
        );
        assert_eq!(lease, INSTANCE_LEASE_REAP_MICROS);
        assert!(
            lease > hosting,
            "a lease must outlive the run it is a receipt for, or the world shard forgets which \
             instance the party is in while they are still in it"
        );

        // The bug this closes, as data. A party enters Deadmines; every member transfers to the
        // instance shard, so the world shard's lease reads EMPTY from minute ~0 of the run. Forty
        // minutes in, a member dies and runs back through the portal.
        let entered = now - 40 * 60 * 1_000_000;
        assert_eq!(
            occupancy_action(false, entered, false, now, hosting),
            OccupancyAction::Reap,
            "on the ordinary 30-minute timer the world shard reaps the lease MID-RUN — the \
             latecomer then resolves Create and lands in a fresh, empty dungeon"
        );
        assert_eq!(
            occupancy_action(false, entered, false, now, lease),
            OccupancyAction::Wait,
            "with the lease timer the world shard still knows which instance that party is in"
        );

        // The MAGNITUDE, not just the ordering. Adversarial review: the assertions above pass with
        // any lease timer over ~40 minutes (45min was verified green), and a lease's countdown
        // starts at the run's START — it is not an idle grace period, it is a bound on RUN LENGTH.
        // A vanilla BRD or Stratholme clear runs 3–5 hours and a raid night runs longer, so a 3h
        // lease re-creates the exact 30-minute fork for the longest runs. Pin the requirement to
        // the run length the realm intends to support, so shrinking the constant is what fails.
        let longest_supported_run = 8 * 60 * 60 * 1_000_000i64;
        assert!(
            lease > longest_supported_run,
            "the lease timer ({lease}µs) must outlast the LONGEST run it is a receipt for, not \
             merely the 30-minute hosting timer: it starts counting when the party leaves the world \
             shard, i.e. at minute ~0 of the run. Below this bound a long dungeon or raid forks its \
             own party exactly as the 30-minute timer did — the bug #21 set out to fix."
        );
        assert_eq!(
            occupancy_action(false, now - longest_supported_run, false, now, lease),
            OccupancyAction::Wait,
            "an 8-hour raid night must still find its lease on the world shard"
        );

        // It is a longer timer, not an immortal row: the stub is still collected.
        let ancient = now - INSTANCE_LEASE_REAP_MICROS;
        assert_eq!(
            occupancy_action(false, ancient, false, now, lease),
            OccupancyAction::Reap
        );
        // And an occupied instance is never reaped on either timer — the guard is orthogonal (an
        // occupied instance with a stale stamp clears it, exactly as on the hosting timer).
        for t in [hosting, lease] {
            assert_eq!(
                occupancy_action(true, ancient, true, now, t),
                OccupancyAction::ClearStamp
            );
            assert_eq!(
                occupancy_action(true, 0, true, now, t),
                OccupancyAction::Wait
            );
        }
    }

    /// **#21 AC#1**, the half that is a cost statement rather than a behaviour: the reaper on a
    /// database with no instances must not pay for the O(entities) occupancy pass. Source-scanned
    /// like the #39 gate above — the reducer body has no unit harness, and moving the early return
    /// below the scan leaves every behavioural test in the workspace green while the open-world
    /// writer starts paying, once a minute, for a table it has no rows in.
    #[test]
    fn the_reaper_costs_a_database_with_no_instances_nothing() {
        let body = code_of(include_str!("instance.rs"), "pub fn reap_instances(");
        let rows = body
            .find("game_instance().iter().collect()")
            .expect("the reaper reads its rows");
        let bail = body.find("if instances.is_empty()").expect(
            "reap_instances no longer short-circuits on an empty game_instance table — a database \
             that hosts no instances would scan every world entity once a minute for nothing (#21)",
        );
        let scan = body
            .find("occupied_instances(ctx)")
            .expect("the reaper classifies occupancy");
        assert!(
            rows < bail && bail < scan,
            "the empty short-circuit must precede the entity scan"
        );
        assert!(
            body[bail..scan].contains("return;"),
            "the empty short-circuit no longer RETURNS — it must stop, not fall through"
        );
        // The per-database timer has to be READ from this database's own config, or a lease-only
        // shard silently reaps its stubs on the 30-minute run timer (see the AC#4 test above).
        assert!(
            body.contains("empty_reap_micros(hosts_instance_populations(ctx))"),
            "reap_instances no longer derives its empty timer from THIS database's \
             hosts_instances flag — leases would be reaped mid-run on the world shard (#21)"
        );
    }

    /// **#21 AC#1**, per-instance schedules. Nothing that ticks an instance may be armed on a
    /// database that does not host its population: the lease path must file the row and stop.
    #[test]
    fn a_lease_arms_no_per_instance_schedule_and_spawns_no_population() {
        let body = code_of(
            include_str!("instance.rs"),
            "pub(crate) fn create_instance_with_id(",
        );
        let gate = body
            .find("hosts_instance_populations(ctx)")
            .expect("the #39 gate");
        let stop = body
            .find("return Ok(instance_id)")
            .expect("the lease path returns");
        assert!(gate < stop);
        // Everything that costs a writer per instance lives AFTER the lease return. (Today
        // `create_instance` arms no tick row at all — perf catalog 1.3 — so this is the pin that
        // notices if one ever comes back on the wrong side of the gate.)
        for costly in [
            "build_creature_entity",
            "gos.insert(",
            "game_creature_move_schedule",
        ] {
            if let Some(at) = body.find(costly) {
                assert!(
                    at > stop,
                    "`{costly}` runs BEFORE the lease return — a database that does not host \
                     instance populations would pay for this instance anyway (#21 AC#1)"
                );
            }
        }
    }

    #[test]
    fn stranding_fallback_prefers_entrance_then_in_place_then_home() {
        // A known dungeon map: its entrance const (the map-36 arm).
        assert_eq!(
            stranding_fallback(entrance_fallback(36), true),
            StrandingFallback::Entrance(0, -11080.0, 1520.0, 46.0, 0.0)
        );
        // A non-dungeon map (the dev-map fixture instance): stay in place at instance 0 — the
        // position is real open-world terrain.
        assert_eq!(
            stranding_fallback(None, false),
            StrandingFallback::InPlaceOpenWorld
        );
        // A dungeon map with no entrance arm (pinned-never above, but never strand): hearthstone.
        assert_eq!(
            stranding_fallback(None, true),
            StrandingFallback::HearthstoneHome
        );
    }

    #[test]
    fn reset_eligibility_requires_unoccupied_and_leadership_for_party_instances() {
        // Solo instance (party 0): any bound caller, but never while occupied.
        assert!(reset_eligible(0, false, false));
        assert!(!reset_eligible(0, false, true));
        // Party instance: the current leader only, and never while occupied.
        assert!(reset_eligible(42, true, false));
        assert!(
            !reset_eligible(42, false, false),
            "a non-leader member cannot reset"
        );
        assert!(
            !reset_eligible(42, true, true),
            "even the leader cannot reset an occupied instance"
        );
    }

    #[test]
    fn instance_population_low_band_is_disjoint_from_spawn_row_allocators() {
        // spawn_wave's allocator: max over SPAWN-ROW lows + 1 — spawn lows are imported cmangos db
        // guids (~10^5) or seed ordinals, all far below bit 23. The band bit keeps every
        // instance-population low ABOVE anything that allocator can ever produce from them.
        let realistic_spawn_lows = [1u64, 3, 40_000, 150_000];
        let wave_next = realistic_spawn_lows.iter().max().unwrap() + 1;
        assert!(
            wave_next & INSTANCE_POP_LOW_BAND == 0,
            "wave allocation stays below the band"
        );
        // The documented invariant's exact boundary: EVERY spawn low strictly below bit 23 yields
        // a wave allocation (max+1) that is still band-free — only a low at 0x7F_FFFF itself (16×
        // any real cmangos guid) could push max+1 into the band, and that is the loud-duplicate-PK
        // `[V]` the band const documents.
        const { assert!(INSTANCE_POP_LOW_BAND == 0x80_0000) };
        let pop_low = INSTANCE_POP_LOW_BAND | 1;
        assert!(pop_low & INSTANCE_POP_LOW_BAND != 0);
        assert_ne!(pop_low, wave_next);
        // The band bit survives wave_guid's 24-bit low mask (it IS bit 23) — the full guid keeps
        // the entry bits intact, so the client still classifies the copy as a Unit.
        let g = crate::encounter::wave_guid(636, pop_low);
        assert_eq!(g >> 48, 0xF130);
        assert_eq!((g >> 24) & 0xFF_FFFF, 636);
        assert_eq!(g & 0xFF_FFFF, pop_low);
    }

    #[test]
    fn go_copy_band_is_disjoint_from_pool_static_and_debug_namespaces() {
        let copy_guid = (0xF110u64 << 48) | GO_COPY_BAND | 1;
        // Below the pool tag: pool_point_id_of must NOT classify a copy as a pool point (else the
        // GO respawn pass would try to reroll a foreign pool on it).
        const { assert!(GO_COPY_BAND < crate::gameobject::POOL_TAG) };
        assert_eq!(crate::gameobject::pool_point_id_of(copy_guid), None);
        // Above every static/debug low: imported db guids and the debug spawner's entry-derived
        // lows are < 2^24, far under bit 46.
        const { assert!(GO_COPY_BAND > 0xFF_FFFF) };
        // And distinct from a pool point at the same sequence.
        assert_ne!(copy_guid, crate::gameobject::pool_point_guid(1));
    }
}
