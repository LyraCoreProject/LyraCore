//! Realm-core: the realm-wide **character→shard index** (issue #20, spec issue #12).
//!
//! # What "realm-core" is
//!
//! Realm-core is not a new service and not a new crate — it is **this same module published under a
//! different database name** (`spacetime publish … realm-core`). Every table in the module exists on
//! every database; which tables are *authoritative* where is a deployment fact the GATEWAY knows, not
//! a schema fact. On realm-core the authoritative tables are `game_account`, `game_session`, and the
//! [`CharacterShard`] index below; on a world shard those first two are a write-through cache
//! and this index is a *forwarding receipt* (see below). Everything else in
//! the module is simply inert on realm-core, because nothing ever writes it there.
//!
//! That is the laziest structure that works: no second wasm crate, no shared-schema crate, no
//! conditional compilation — one artifact, N deployments, and the routing lives in the gateway where
//! every other routing decision already lives (`gateway/src/config.rs`'s `ShardMap`).
//!
//! # The index is a HINT, never the authority
//!
//! `game_character_shard` answers "which (map, instance) — and therefore, through the shard map,
//! which database — owns this character". It stores the LOCATION rather than a database name on
//! purpose: a database name would be invalidated by every shard-map edit (an operator rebalancing a
//! pool would silently strand every indexed character), whereas a location is re-resolvable against
//! whatever map is loaded today.
//!
//! The gateway treats an entry as a hint it must confirm: it accepts the hint only when the shard it
//! names actually holds the character row, and otherwise falls back to probing the connected shards
//! and writes the corrected entry back (`gateway/src/config.rs::resolve_home_shard`, and the tests
//! there). So a stale — or entirely absent, or maliciously wrong — index entry costs one extra cache
//! probe at login and then heals itself. Nothing about correctness rests on the index being right,
//! which is what lets it be updated with a single non-transactional write from a second database.

use spacetimedb::{reducer, table, ReducerContext, Table};

/// Character→shard index entry: "character `character_guid` lives at (`map_id`, `instance_id`)".
///
/// PRIVATE (no `public`): this is realm operations data, not game state — a public table would hand
/// every connected client a directory of every character's whereabouts. The gateway's coordinator
/// connection authenticates with the owner token, which bypasses RLS, so it subscribes to it exactly
/// the way it already subscribes to the private `game_account` / `game_session`. [session]
#[table(accessor = game_character_shard)]
pub struct CharacterShard {
    #[primary_key]
    pub character_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    /// When this entry was written — diagnostics only (nothing reads it to decide anything; the
    /// gateway confirms an entry by probing, never by trusting its age).
    pub updated_micros: i64,
}

// A deleted character must not leave a directory entry behind: guids are reused (`create_character`
// allocates max-referenced + 1), so a stale entry could name a shard for a guid that now belongs to
// someone else. Harmless in itself — the gateway confirms every hint by probing — but one fewer wrong
// answer to reason about, for one line.
crate::character_owned!(delete, fn sweep_delete_game_character_shard(ctx, character_guid) {
    ctx.db.game_character_shard().character_guid().delete(character_guid);
});

// CROSS-DATABASE transport (issue #19 × #20): the directory entry deliberately does NOT ride the
// export blob. It is not character data — it is a routing HINT about WHERE the character is, and the
// blob's whole purpose is to change that. The source's row is snapshotted by `begin_transfer` while
// it still names the SOURCE, so importing it would give the destination a forwarding receipt
// pointing back at the shard the character just left: a login there would be bounced to a database
// that no longer holds it, until the gateway's probe healed it. The two writes that DO keep the
// directory honest are `transfer::do_finish` (the source's own row, rewritten to name the
// destination, in the escrow's transaction) and the gateway's `set_character_shard` against
// realm-core, which is the authoritative copy — driven as a required step of
// `world::transfer::run_transfer` immediately after `finish_transfer` commits (issue #34), and
// re-driven by the login self-heal probe whenever the two disagree.
crate::character_owned!(transfer, fn sweep_transfer_game_character_shard(ctx, character_guid, io) {
    let _ = (ctx, character_guid);
    crate::transfer::not_transported(io);
});

/// Upsert the index entry for `character_guid`. Shared by [`set_character_shard`] (the gateway's
/// write, on realm-core) and by `transfer::do_finish` (the module's write, in the SAME transaction
/// that releases the escrow — issue #20 AC#3).
pub(crate) fn record_shard(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
) {
    let idx = ctx.db.game_character_shard();
    let row = CharacterShard {
        character_guid,
        map_id,
        instance_id,
        updated_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    };
    if idx.character_guid().find(character_guid).is_some() {
        idx.character_guid().update(row);
    } else {
        idx.insert(row);
    }
}

/// Publish a character's location into the realm-core index. Operator-gated like every other
/// gateway-coordination reducer (`provision_account` / `establish_session`): the index is a routing
/// input, so a client that could write it could redirect another player's login.
///
/// Two callers, both on the gateway's realm-core handle: `world::transfer::run_transfer` publishes
/// the settled destination here as a required step of every escrowed transfer (issue #34 — strictly
/// after `finish_transfer` committed, so it can only ever name a destination the escrow reached),
/// and `home_shard`'s login self-heal rewrites it whenever the fallback probe disagrees with what
/// the index says. Only the FIRST of those runs in production today: the gateway's world-entry
/// resolver is `settle_home_shard`, which overrides `home_shard` and scans the shards instead of
/// consulting this table, so the index is currently written-but-never-read and its self-heal never
/// fires. #39/#21 is what changes that.
#[reducer]
pub fn set_character_shard(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    record_shard(ctx, character_guid, map_id, instance_id);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Guid-range registry (#108) — realm-core answers "which shard mints from where"
// ---------------------------------------------------------------------------------------------

/// How many guids a slot owns. Slot *n* is `[n · GUID_RANGE_SIZE, (n+1) · GUID_RANGE_SIZE)`, which
/// makes #105's hand-applied floors (core 0, world-1 1e9, instances 2e9, realm-core 3e9) slots
/// 0/1/2/3 exactly — so the live databases migrate into this registry by being *recorded*, with no
/// guid changing. Decimal and small on purpose (`set_guid_floor`'s doc): readable in logs and SQL,
/// a billion characters per shard, and far below `2^53`, above which `spacetime call` mangles a u64
/// argument (danger-zones).
pub const GUID_RANGE_SIZE: u64 = 1_000_000_000;

/// Which slot a database that has already minted up to `mark` is *actually* using. Pure.
pub(crate) fn slot_of(mark: u64) -> u32 {
    (mark / GUID_RANGE_SIZE) as u32
}

/// Which slot to assign, given the slots already taken and the one the shard is already minting
/// from. Pure — the whole allocation decision, so the ordering property below is testable without a
/// database.
///
/// **Order independence is the load-bearing property.** Handing out `max + 1` unconditionally would
/// make the outcome depend on the order the gateway happens to claim in: claim `world-1` (already
/// minting at 1e9, slot 1) first and it would take slot 0, then `lyracore` — which holds
/// guids 1..213, i.e. slot 0 — would be handed slot 1 and mint straight into world-1's live
/// characters. Preferring the shard's own slot when it is free makes every live shard land on the
/// range it is already using, in any order.
pub(crate) fn assign_slot(taken: impl Iterator<Item = u32>, desired: u32) -> u32 {
    let taken: Vec<u32> = taken.collect();
    if !taken.contains(&desired) {
        return desired;
    }
    taken.iter().max().map_or(0, |m| m + 1)
}

/// Which shard owns which guid range. **Realm-core's table**, in the same sense as
/// [`CharacterShard`] above: it exists on every database and is authoritative on exactly one.
///
/// PRIVATE for the same reason as the index — realm operations data, not game state.
#[table(accessor = game_guid_range_registry)]
pub struct GuidRangeAssignment {
    #[primary_key]
    pub shard_name: String,
    /// `#[unique]` is the conflict-safety, not a convention: two gateways claiming at once cannot
    /// both land on one slot, because the second insert violates this constraint and its whole
    /// transaction aborts.
    #[unique]
    pub slot: u32,
    pub base: u64,
    pub size: u64,
    pub assigned_micros: i64,
}

/// Assign `shard_name` a guid range, once in that shard's lifetime. Idempotent: a shard that
/// already holds one keeps it — re-running this is how the gateway checks, so it must be free.
///
/// `current_mark` is the shard's own `game_guid_allocator` high-water, which is what makes the
/// four live databases migrate onto their existing floors instead of being shuffled (see
/// [`assign_slot`]). The caller then installs the assigned base on the shard itself with
/// `auth::install_guid_range`; nothing here reaches across databases, because nothing can.
///
/// Operator-gated like every other gateway-coordination reducer: a client that could write this
/// could hand a shard someone else's range.
#[reducer]
pub fn claim_guid_range(
    ctx: &ReducerContext,
    shard_name: String,
    current_mark: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let reg = ctx.db.game_guid_range_registry();
    if let Some(row) = reg.shard_name().find(&shard_name) {
        spacetimedb::log::info!(
            "claim_guid_range: {shard_name} already holds slot {} (base {}) — no reassignment",
            row.slot,
            row.base
        );
        return Ok(());
    }
    let slot = assign_slot(reg.iter().map(|r| r.slot), slot_of(current_mark));
    reg.try_insert(GuidRangeAssignment {
        shard_name: shard_name.clone(),
        slot,
        base: slot as u64 * GUID_RANGE_SIZE,
        size: GUID_RANGE_SIZE,
        assigned_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    })
    .map_err(|e| format!("claim_guid_range: slot {slot} was taken concurrently ({e}) — retry"))?;
    spacetimedb::log::info!(
        "claim_guid_range: {shard_name} assigned slot {slot} (base {}, size {GUID_RANGE_SIZE}); \
         it was minting at {current_mark}",
        slot as u64 * GUID_RANGE_SIZE
    );
    Ok(())
}

#[cfg(test)]
mod guid_range_registry_tests {
    use super::*;

    #[test]
    fn a_shard_keeps_the_slot_it_is_already_minting_from_whatever_the_claim_order() {
        // The four live databases, by their #105 floors — claimed in the WORST order (highest
        // first), which is exactly what a `max + 1` allocator would mangle.
        let live = [
            ("world-1", 1_000_000_101u64),
            ("instances", 2_000_000_000),
            ("core", 213),
        ];
        let mut taken: Vec<u32> = vec![];
        for (name, mark) in live {
            let slot = assign_slot(taken.iter().copied(), slot_of(mark));
            assert_eq!(
                slot,
                slot_of(mark),
                "{name} (minting at {mark}) must keep slot {}, got {slot}",
                slot_of(mark)
            );
            taken.push(slot);
        }
        // …and a brand-new shard, which has minted nothing, cannot be handed core's slot 0.
        let fresh = assign_slot(taken.iter().copied(), slot_of(0));
        assert!(
            !taken.contains(&fresh),
            "a fresh shard was handed a slot already in use"
        );
        assert_eq!(fresh, 3);
    }

    #[test]
    fn slots_partition_the_guid_space() {
        assert_eq!(slot_of(0), 0);
        assert_eq!(slot_of(GUID_RANGE_SIZE - 1), 0);
        assert_eq!(slot_of(GUID_RANGE_SIZE), 1);
        assert_eq!(slot_of(3 * GUID_RANGE_SIZE + 7), 3);
    }
}
