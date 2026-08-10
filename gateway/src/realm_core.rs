//! The realm-core split, written against a trait instead of a live connection.
//!
//! # Why this file exists
//!
//! An adversarial review ran 16 mutations against the realm-core auth code. Eight went
//! red — every one of them in `config.rs`, the pure resolver layer. Eight SURVIVED, and every one
//! of those lived in `CoordinatorStore` or `impl WorldStore for Coordinator`: the layer that
//! actually performs the split. Among the survivors were *"`account()` reads SRP6 material off the
//! world DB"* and *"`lookup_session` reads the session key K off the world DB's cache"* — the exact
//! regression the realm-core split exists to prevent, invisible to a fully green suite. The reason was
//! mechanical, not cultural: `Coordinator` wraps a live SpacetimeDB websocket, so nothing in the
//! crate could execute those bodies without a node.
//!
//! # The seam
//!
//! The same one the transfer transport uses, and for the same reason: make the
//! PRODUCTION function generic over the store type, and put the store behind a small trait. So
//! [`RealmDb`] is `Coordinator` reduced to the fifteen calls the realm-core split actually makes;
//! `Coordinator` implements it by forwarding to its own inherent methods (Rust resolves inherent
//! methods first, so those forwards are views, not recursion), and [`fake::Handle`] implements it
//! over an in-memory two-database topology. Every function below is then run BY THE TESTS, not
//! modelled by them — a harness that re-implemented the rules would reproduce the exact failure
//! this file is about.
//!
//! What is NOT modelled here is the transport: `Coordinator`'s own one-line bodies (the websocket
//! read, the `call_reducer!`) are substituted wholesale by the fake. That layer is pinned by
//! exact-shape equality on the forwarding impl (`the_coordinator_forwards_are_views_not_logic`),
//! because a `contains` scan is defeated by leaving the text in a dead branch. The module pins its
//! own equivalent (`CtxShard`) exactly the same way, and a later measurement showed why that is still the right
//! instrument: a cargo-mutants run over that surface MISSED every mutation in the adapter, because a
//! mutation tool can only ask whether a test fails and no headless test can drive the real
//! connection. The same holds here.

use anyhow::Result;

use crate::config::{HomeShard, ShardMap};
use crate::stdb::{AccountRow, RealmRow};
use crate::world::WorldSession;

/// A SpacetimeDB database handle, reduced to exactly what the realm-core split touches.
///
/// Implemented by [`crate::stdb::Coordinator`] (production) and [`fake::Handle`] (tests). Every
/// method has an identically-named inherent method on `Coordinator`, so the production impl is a
/// block of forwards and no behaviour moves when a caller becomes generic.
pub(crate) trait RealmDb: Clone + Sized + Send + Sync {
    /// The database this handle targets.
    fn shard_name(&self) -> &str;
    /// Is the gateway running against more than one database? `false` is the unconfigured default.
    fn is_sharded(&self) -> bool;
    /// The routing table this handle resolves through.
    fn shard_map(&self) -> &ShardMap;
    /// The **realm-core** handle: the database that owns accounts, sessions and the character→shard
    /// index. Unconfigured → this handle (one database, today's behaviour). Configured but not
    /// connected → `Err`, which every auth path must propagate rather than fall back to the world
    /// database's stale auth cache.
    fn realm_core(&self) -> Result<Self>;
    /// The connected WORLD SHARDS, default first. Realm-core is never in it: it holds no characters.
    fn world_shards(&self) -> Vec<(String, Self)>;

    // --- auth reads
    fn account_by_username(&self, username: &str) -> Result<Option<AccountRow>>;
    fn session_key(&self, account_id: u64) -> Result<Option<[u8; 40]>>;
    fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]>;
    fn character_count(&self, account_id: u64) -> Result<u8>;
    fn realm(&self) -> Result<RealmRow>;

    // --- auth write
    fn establish_session(
        &self,
        account_id: u64,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()>;

    // --- The LOGON socket's lease on the account's cached per-account connection.
    //
    // `bound_identity` above is what OPENS that connection, so a logon that authenticates and then
    // walks away leaks its websocket fd and its SDK pump thread for the gateway's lifetime — the
    // half of the reclaim that the world-tier accounting never reached. These two are the same
    // `stdb::AccountSessions` refcount the world tier attaches to, which is what makes the
    // logon→world handover free: the world session's attach lands while the logon's lease is still
    // held (or inside its grace), so the connection is reused rather than rebuilt.
    /// Register this logon socket as a live user of `account_id`'s cached connection. Idempotent
    /// per socket — see `CoordinatorStore::lease`. `account_id` is the WORLD shard's id, the key
    /// `player_conn` caches under; realm-core's own account id names nothing there.
    fn attach_account_session(&self, account_id: u64);
    /// Retire it when the logon socket closes. DEFERRED: it never releases the connection outright,
    /// because the account's next socket is normally the world session that reuses it. See
    /// `stdb::AccountSessions` for the grace and why the epoch gate is the wrong instrument here.
    fn detach_account_session_deferred(&self, account_id: u64);

    // --- the character→shard index
    /// Where THIS database's own rows say the character is. `None` = not here.
    fn character_location(&self, guid: u64) -> Option<(u32, u64)>;
    /// This database's `game_character_shard` entry for `guid` — a HINT, never the authority.
    fn character_shard(&self, guid: u64) -> Option<(u32, u64)>;
    /// Write `guid`'s location into this database's character→shard index.
    fn set_character_shard(&self, guid: u64, map_id: u32, instance_id: u64) -> Result<()>;
    /// Does THIS database hold an in-flight escrow (`game_transfer_out`) for `guid`? A shard
    /// answering `true` is the SOURCE of a resumed transfer and wins outright over any shard merely
    /// holding a durable row for the guid — see [`locate_home_shard`]. The one method here whose
    /// `Coordinator` forward is not a bare call (it narrows `escrow_row`'s `Option<TransferOut>` to
    /// a bool) — the narrowing itself carries no routing decision, so it cannot silently pick the
    /// wrong database the way a real forward-body change could.
    fn has_escrow(&self, guid: u64) -> bool;

    // --- Load sampling. The gateway is the one component that can see the whole realm (every
    // shard's metrics endpoint, every session, every live position), so it samples; realm-core just
    // records what it is told (`module/src/load.rs`, ring-buffered per key). `session_count` reads
    // THIS handle's own shard; `record_shard_load` is fired against whichever handle the CALLER
    // holds — in production that is always the `realm_core()` handle, since that table is only
    // ever read from there.
    /// Live player sessions cached on THIS shard's coordinator connection — an approximate
    /// per-shard population (a point-in-time snapshot, not a windowed average).
    fn session_count(&self) -> usize;
    /// Record `shard`'s occupancy + session sample.
    fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
    ) -> Result<()>;
}

// ===============================================================================================
//  The world handshake's account→K lookup
// ===============================================================================================

/// The world handshake's account→K lookup, split across the two databases that own the two halves
/// of the answer. The body of `impl WorldStore for Coordinator::lookup_session`.
///
/// **K comes from realm-core**, which is the whole point of a realm-wide session table: the session
/// key is realm state, not
/// gateway state, so a gateway that is killed and restarted mid-session re-reads it and the same
/// handshake succeeds — now no matter which gateway, on which shard, the client reconnects to.
///
/// **The account id comes from THIS (world) shard**, and that is not a detail. `Account.id` is
/// `#[auto_inc]`, so it is a *per-database* surrogate key; `game_character.account_id` and every
/// world-phase call (`characters`, `player_login`, `create_character`, …) mean the world shard's
/// id. The cross-database key is the USERNAME — which the client sends in `CMSG_AUTH_SESSION` and
/// the logon protocol carries end to end. Passing realm-core's id to the world shard would bind one
/// player to another player's characters, silently, on any deployment whose two databases were not
/// seeded in identical order. Never do it.
///
/// With realm-core unconfigured [`RealmDb::realm_core`] IS this handle, so both reads hit one
/// database and this is today's code path with an extra (identical) lookup.
pub(crate) fn lookup_session<D: RealmDb>(
    db: &D,
    account_name: &str,
) -> Result<Option<WorldSession>> {
    let realm_core = db.realm_core()?;
    let Some(authoritative) = realm_core.account_by_username(account_name)? else {
        return Ok(None);
    };
    // Deliberate simplification: no ban check here. The logon challenge already refuses a banned
    // account BEFORE K is ever written, so the only case a handshake check would add is "banned
    // while a live session row exists" — and the right answer to that is a live kick
    // (subscribe to the account row, disconnect on `banned`), not a one-shot check at handshake
    // time. Adding one here would also change unconfigured-realm-core behavior, which the split
    // promises it will not. Ceiling: a ban does not take effect until the player's next logon.
    let Some(session_key) = realm_core.session_key(authoritative.id)? else {
        return Ok(None);
    };
    let Some(local) = db.account_by_username(account_name)? else {
        // `Ok(None)` (reject the handshake), NOT the id from realm-core: that id names whichever
        // account this world shard happened to issue it to. Unreachable with realm-core
        // unconfigured — the read above is the same read against the same database — so this
        // arm cannot change single-database behaviour.
        log::error!(
            "account {account_name} authenticated on realm-core but has no account row on world \
             shard {} — provision it there (accounts are keyed across databases by USERNAME; ids \
             are per-database)",
            db.shard_name()
        );
        return Ok(None);
    };
    Ok(Some(WorldSession {
        account_id: local.id,
        session_key,
    }))
}

// ===============================================================================================
//  The character→shard index
// ===============================================================================================

/// Resolve where `character_guid` lives, from the realm-core index plus a probe of the connected
/// world shards, and repair the index when the two disagree. The first half of
/// `impl WorldStore for Coordinator::home_shard`.
///
/// `None` means "this handle already owns it" — which is the only answer a single-database gateway
/// can give, and it is given without a single read (the `is_sharded` short-circuit below is what
/// makes "`LYRACORE_SHARD_MAP` unset ⇒ byte-identical to the pre-sharding gateway" literal on the login
/// hot path).
///
/// **Still only reachable through `WorldStore::home_shard`, which `world::route_home` never calls**
/// — `stdb::world_store` overrides it with `settle_home_shard`, so this exact function stays a test
/// fixture in production. That no longer means the index goes unread, though:
/// `settle_home_shard` has its OWN index-first lookup, [`locate_home_shard`], built on the same
/// `RealmDb` trait and the same hint→probe→heal shape as this function, so the hint read and the
/// self-heal write both run on the live world-entry path now — through the sibling, not through
/// this one. This function remains the trait DEFAULT's resolver (every mock that does not override
/// `settle_home_shard` still exercises it) and its own tests below.
pub(crate) fn settle_shard_index<D: RealmDb>(db: &D, character_guid: u64) -> Option<HomeShard> {
    // The single-shard short-circuit. With one database in the set the answer is unconditionally
    // "you are already there", and this costs the login neither the index read, nor the
    // `character_location` probe, nor an index write.
    if !db.is_sharded() {
        return None;
    }
    // Consult the realm-core character→shard index first, then confirm/repair it against the shard
    // that actually holds the character. `realm_core()` failing (configured, not connected) costs
    // us the hint, not the login — routing degrades to the probe, the pre-index behavior. Only the
    // AUTH paths fail closed on a missing realm-core.
    let index = db.realm_core().ok();
    let hint = index
        .as_ref()
        .and_then(|rc| rc.character_shard(character_guid));
    let shards = db.world_shards();
    let names: Vec<String> = shards.iter().map(|(name, _)| name.clone()).collect();
    let resolved = crate::config::resolve_home_shard(db.shard_map(), &names, hint, |want| {
        shards
            .iter()
            .find(|(name, _)| name == want)
            .and_then(|(_, handle)| handle.character_location(character_guid))
    })?;
    // The self-heal write-back. Best effort by design: a failed index write costs the NEXT login
    // one extra probe and nothing else, so it must never fail a login that already routed fine.
    if resolved.heal {
        if let Some(rc) = &index {
            if let Err(e) =
                rc.set_character_shard(character_guid, resolved.location.0, resolved.location.1)
            {
                log::warn!(
                    "character {character_guid}: could not heal the realm-core shard index to map \
                     {}/instance {} ({e:#}) — routing is unaffected, the probe will run again",
                    resolved.location.0,
                    resolved.location.1
                );
            }
        }
    }
    Some(resolved)
}

/// Publish a settled transfer's destination into the REALM-CORE character→shard index.
///
/// # Why this is not "a best-effort write the gateway might make"
///
/// The design calls for an index updated *transactionally by the escrow's finish step*.
/// `transfer::do_finish` does exactly that — but only for the database it runs on, because there is
/// no transaction spanning two SpacetimeDB databases. Realm-core's copy is the one `home_shard`
/// actually reads, and before this function existed nothing ever wrote it: `set_character_shard`'s
/// only caller was the login self-heal, so the directory learned about a completed transfer at the
/// character's NEXT login, from a probe, and the "transactional" write landed on world-shard copies
/// nothing reads.
///
/// What is achievable across two databases is REPLICATION of a transactionally-written fact, and
/// that is what this is. `world::transfer::run_transfer` calls it as step 5b — a required step of
/// the drive, strictly after `finish_transfer` returned `Ok`, deriving `(map, instance)` from the
/// same escrow out-row fields `do_finish` recorded its own receipt from. So it can never name a
/// destination for a transfer that did not settle, which is the property "not a separate call that
/// can commit independently" is protecting: a stale-index generator writes the index for transfers
/// that never happened, and this cannot.
///
/// The residual window — the gateway dies between `finish_transfer` and this call, or this call
/// itself fails — IS covered: `settle_home_shard`'s own index lookup,
/// [`locate_home_shard`], probes and heals a stale entry on the next world entry for the character
/// (the recovery path — `settle_transfer`'s holder-is-owner release — still does not re-enter
/// `run_transfer`, so a missed publish is corrected by the NEXT login's probe, not retried
/// immediately). Before that lookup existed this window was open indefinitely: `settle_shard_index`'s probe-and-heal
/// hung off `WorldStore::home_shard`, which `stdb::world_store` overrides with `settle_home_shard`
/// — and that override resolved the character by scanning the connected shards and never read or
/// repaired the index, so a missed publish left realm-core's entry naming the old shard until the
/// character's next COMPLETED TRANSFER rather than its next login.
pub(crate) fn publish_shard_index<D: RealmDb>(
    db: &D,
    character_guid: u64,
    map_id: u32,
    instance_id: u64,
) -> Result<()> {
    db.realm_core()?
        .set_character_shard(character_guid, map_id, instance_id)
}

// ===============================================================================================
//  `settle_home_shard`'s holder lookup — the index is used, not just written
// ===============================================================================================

/// Where `character_guid` CURRENTLY lives, consulting the realm-core character→shard index FIRST
/// and paying the full scan only on a miss. This is `settle_home_shard`'s holder-resolution step —
/// the method `world::route_home` actually calls on every world entry — replacing the unconditional
/// scan (`Coordinator::locate_character`) it used before.
///
/// The realm-core work wrote the index and gave it a self-heal (`settle_shard_index`, above), but that sits
/// behind `WorldStore::home_shard`, which `settle_home_shard` overrides — so in production the index
/// sat next to the login path unread and the self-heal never ran. This
/// function is the fix, written against the same [`RealmDb`] seam for the same reason: so it runs
/// UNDER THE TESTS via [`fake::Handle`] rather than being merely described by a source scan of
/// `Coordinator`'s live-node-only override.
///
/// A hint is trusted only far enough to know WHICH ONE shard to ask — that shard's own row (or
/// escrow) is still the truth, so a wrong or stale hint costs one probe, never a wrong route, and
/// the mismatch is healed back into the index (`heal_shard_entry`, the same rule
/// `settle_shard_index` uses). The miss path — no hint, an unreachable realm-core, or the hinted
/// shard no longer holding the character — scans every connected shard, default first, exactly as
/// the older `locate_character` did: a shard holding an in-flight ESCROW wins outright (between
/// `import_character_blob` and `finish_transfer` BOTH databases can hold a durable row for the guid,
/// and the escrow-holder is the only side a resumed transfer can be driven from — see
/// `world::transfer`), otherwise the first shard whose row answers. In practice the index already
/// names the escrow-holding shard by the time a transfer starts (nothing repoints it to a
/// destination before the transfer settles — that is `publish_shard_index`'s whole contract), so the
/// fast path IS the escrow case too; the scan is what a never-yet-healed or genuinely stale index
/// falls back into.
pub(crate) fn locate_home_shard<D: RealmDb>(db: &D, guid: u64) -> Option<D> {
    // This function's own copy of the short-circuit. `settle_home_shard` (`stdb::world_store`) already
    // guards its call to this function with an identical `if !self.is_sharded() { return Ok(None);
    // }` — that one is production's, stays exactly where it is, and cannot be exercised by any test
    // without a live SpacetimeDB connection (`Coordinator` cannot be constructed offline). This copy
    // is redundant on the happy path (never fires when the caller's guard already returned), but it
    // is the only layer of "an unconfigured gateway reads NOTHING while resolving a home shard" that
    // a test can actually reach — `fake::Handle` runs this function directly. Without it, a single
    // connected database (unsharded) still pays a `character_shard` probe, a `character_location`
    // scan and a `set_character_shard` heal write to reach the one answer "stay put" gives for
    // free — exactly the reads an unset `LYRACORE_REALM_CORE`/`LYRACORE_SHARD_MAP` must never pay.
    if !db.is_sharded() {
        return None;
    }
    let index = db.realm_core().ok();
    let shards = db.world_shards();
    let is_connected = |d: &str| shards.iter().any(|(n, _)| n == d);

    // Fast path: trust the hint only far enough to know which ONE shard to ask.
    if let Some(rc) = &index {
        if let Some(hint) = rc.character_shard(guid) {
            let hinted_name = db
                .shard_map()
                .resolve_connected(hint.0, hint.1, is_connected);
            if let Some((_, hinted)) = shards.iter().find(|(n, _)| n == hinted_name) {
                if hinted.has_escrow(guid) || hinted.character_location(guid).is_some() {
                    heal_shard_entry(rc, guid, hinted);
                    return Some(hinted.clone());
                }
            }
        }
    }

    // Miss: the full scan. A shard holding an escrow wins outright; otherwise prefer a shard whose
    // row AGREES with the shard map, and only then fall back to first-responder order.
    //
    // THE DISAMBIGUATOR. This used to take the first shard that answered at all,
    // so a guid with rows on two shards resolved by ITERATION ORDER — default first — and a stale
    // artefact on the default shard beat the character's real home. Reproduced by a reviewer.
    //
    // The rule, stated so it can be argued with: **a durable row is authoritative only on the shard
    // the shard map assigns its own location to.** A row on `core` saying "I am at map 36" describes
    // a character that belongs on the instances shard, so it is evidence of a leftover, not of
    // residence — the same principle as `heal_shard_entry`'s guard, which is why both now read
    // the same way: CONFIRM where a character is, never infer it from row order.
    //
    // Deliberately a PREFERENCE, not a filter: if the only row anywhere disagrees with the map, it is
    // still returned (`disagreeing` below). Routing to a stale-looking row is what happens today and
    // is recoverable; routing to NOTHING is a failed login. This can therefore only improve on the
    // old behaviour, never regress it.
    let mut agreeing: Option<&D> = None;
    let mut disagreeing: Option<&D> = None;
    for (_, shard) in &shards {
        if shard.has_escrow(guid) {
            if let Some(rc) = &index {
                heal_shard_entry(rc, guid, shard);
            }
            return Some(shard.clone());
        }
        if let Some(loc) = shard.character_location(guid) {
            if shard.shard_map().resolve(loc.0, loc.1) == shard.shard_name() {
                if agreeing.is_none() {
                    agreeing = Some(shard);
                }
            } else if disagreeing.is_none() {
                disagreeing = Some(shard);
            }
        }
    }
    let holder = agreeing.or(disagreeing);
    if let (Some(h), Some(rc)) = (holder, &index) {
        heal_shard_entry(rc, guid, h);
    }
    holder.cloned()
}

/// Write `guid`'s ACTUAL location (`shard`'s own `character_location`) back into the realm-core
/// index when it disagrees with what the index currently holds — including "holds nothing yet".
/// Best effort, mirroring `settle_shard_index`'s identical rule: a failed heal costs the next login
/// one extra probe and nothing else, so it must never fail a login that already routed fine.
fn heal_shard_entry<D: RealmDb>(rc: &D, guid: u64, shard: &D) {
    let Some(actual) = shard.character_location(guid) else {
        // Matched by escrow alone with no durable row to read a location from — cannot happen before
        // `finish_transfer` (the row is frozen, not deleted, until then), but nothing to write back.
        return;
    };
    // THE HEAL MUST CONFIRM, NEVER PREDICT. The index answers "which SHARD holds this
    // character": every reader resolves the stored `(map, instance)` back through the shard map to a
    // database name — `locate_home_shard`'s fast path does exactly that. So a pair that resolves to
    // some OTHER database records "she lives over there" about a character we just found HERE.
    //
    // `character_location` does not rule that out. It prefers the LIVE entity row and, for a
    // character mid-instance-entry, reports the PENDING destination (`stdb/reads.rs`) — so a
    // character still standing on the source with a portal already stamped reads as the map the
    // DESTINATION owns. Healing that wrote the destination into the index BEFORE `begin_transfer`
    // had even opened the escrow, and the next login's fast path then trusted it: it resolved the
    // destination as the holder, never saw the source's escrow (escrows are read from the holder
    // only), resolved owner == holder, and `settle_transfer` took its no-op branch — leaving the
    // source copy and its escrow stranded with no component left that would ever re-drive them.
    //
    // The legitimate way this index moves to the destination is `publish_shard_index`, driven only
    // after `finish_transfer` returned Ok. This function only ever confirms where a character IS.
    //
    // Note: declining also covers the case where a shard genuinely holds a character whose map
    // the shard map assigns elsewhere (a re-partition, before anything moved her). The index cannot
    // express that — it stores a location, not a shard name — so the honest answer is to leave it
    // alone and let the scan answer, at the cost of one extra probe per login until she moves.
    if shard.shard_map().resolve(actual.0, actual.1) != shard.shard_name() {
        return;
    }
    if rc.character_shard(guid) == Some(actual) {
        return;
    }
    if let Err(e) = rc.set_character_shard(guid, actual.0, actual.1) {
        log::warn!(
            "character {guid}: could not heal the realm-core shard index to map {}/instance {} \
             ({e:#}) — routing is unaffected, the probe will run again",
            actual.0,
            actual.1
        );
    }
}

// ===============================================================================================
//  `delete_character`'s cross-shard routing
// ===============================================================================================

/// Where `delete_character` must run for `guid`, or why it must not run anywhere right now.
///
/// Reuses `locate_home_shard`'s index-first resolution — the SAME routing decision the
/// world-entry path already trusts — rather than inventing a second mechanism. `Ok(Some(owner))` is
/// the shard to delete on (which may legitimately BE `self` — nothing special-cases that, since
/// calling delete on a handle that happens to name `self`'s own database is harmless). `Ok(None)`
/// means the gateway is unsharded, or `guid` resolves nowhere at all (unknown/already-deleted), so
/// the caller falls back to its existing single-database delete and gets the exact NOT_FOUND-shaped
/// answer it always did.
///
/// `Err` means REFUSE: a shard holding an in-flight escrow (`game_transfer_out`) for `guid` is
/// never a valid delete target. Between `import_character_blob` and `finish_transfer` the row can
/// be split across two databases, so deleting either half mid-flight either destroys the character
/// a moment before it lands on the other side, or leaves a frozen copy the resumed transfer then
/// fails against. The caller must surface this as a delete FAILURE — never a silent success, and
/// never a silent no-op indistinguishable from "nothing to delete".
pub(crate) fn resolve_delete_shard<D: RealmDb>(db: &D, guid: u64) -> Result<Option<D>> {
    if !db.is_sharded() {
        return Ok(None);
    }
    let Some(owner) = locate_home_shard(db, guid) else {
        return Ok(None);
    };
    if owner.has_escrow(guid) {
        return Err(anyhow::anyhow!(
            "character {guid} is mid-transfer — an escrow holds it on shard {} — refusing to \
             delete until the transfer settles",
            owner.shard_name()
        ));
    }
    Ok(Some(owner))
}

// ===============================================================================================
//  The two-database fake
// ===============================================================================================

/// An in-memory realm: N databases, a shard map, and a liveness switch for realm-core.
///
/// It models exactly the four facts the realm-core split reads — accounts, sessions, character
/// locations and the shard index — per database, plus a per-database ACCESS LOG so a test can
/// assert not just *what* answer a function gave but *which database it asked*. That log is what
/// turns "reads SRP6 material off the world DB" from an unobservable mutation into a named failure.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use anyhow::anyhow;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    /// One "database".
    #[derive(Default)]
    pub(crate) struct Db {
        /// `game_account`, keyed by the uppercased username.
        pub accounts: Mutex<HashMap<String, AccountRow>>,
        // Test-double storage: each map mirrors one module TABLE's columns, so the tuple is the row shape.
        #[allow(clippy::type_complexity)]
        /// `game_session`: account_id -> (K, bound identity).
        pub sessions: Mutex<HashMap<u64, ([u8; 40], [u8; 32])>>,
        // Test-double storage: each map mirrors one module TABLE's columns, so the tuple is the row shape.
        #[allow(clippy::type_complexity)]
        /// `game_character`: guid -> (map, instance). Also the character COUNT per account.
        pub characters: Mutex<HashMap<u64, (u64, (u32, u64))>>,
        /// `game_character_shard`.
        pub shard_index: Mutex<HashMap<u64, (u32, u64)>>,
        /// The node-issued identity of this database's per-account player connection.
        pub identities: Mutex<HashMap<u64, [u8; 32]>>,
        /// How many identities this database has ever minted. Stamped into every identity so
        /// a connection that was released and REBUILT is distinguishable from one that was reused —
        /// the node mints a fresh identity per connection, and a rebuilt one is exactly what
        /// `establish_session` has not bound.
        pub mints: Mutex<u8>,
        /// `game_transfer_out`: guids with an in-flight escrow ON this database.
        pub escrows: Mutex<HashSet<u64>>,
        /// This shard's live player-SESSION count (`CoordinatorInner::players.len()`'s fake
        /// equivalent) — set directly by a test, not derived from `characters`/`identities`.
        pub open_sessions: Mutex<usize>,
        /// Every `record_shard_load` call this database RECEIVED, in order.
        pub recorded_shard_loads: Mutex<Vec<(String, f32, u32)>>,
        /// Every `RealmDb` call served by this database, in order.
        pub log: Mutex<Vec<String>>,
    }

    impl Db {
        fn note(&self, what: &str) {
            self.log.lock().unwrap().push(what.to_string());
        }
        pub fn touched(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    pub(crate) struct Realm {
        pub dbs: HashMap<String, Arc<Db>>,
        pub map: ShardMap,
        /// `false` = realm-core is configured but its websocket is down: `realm_core()` must `Err`
        /// rather than serve the world database's stale auth cache.
        pub realm_core_up: bool,
        /// The REAL per-account socket refcount (`crate::stdb::AccountSessions`), not a model
        /// of it — the same type, behind the same predicate, that `stdb::ShardSet` holds. It lives
        /// on the REALM rather than on a `Db` for the same reason it lives on `ShardSet` rather
        /// than on `CoordinatorInner`: it counts SOCKETS, which is a gateway concept with no
        /// database in it, and the connections a release drops are spread across every shard.
        pub sessions: crate::stdb::AccountSessions,
        /// Accounts whose cached per-account connections the reaper released, in order. The only
        /// thing stubbed here is the release ACTION (recording instead of closing a websocket).
        pub released: Mutex<Vec<u64>>,
    }

    /// A handle on ONE database of a [`Realm`] — the fake's `Coordinator`.
    #[derive(Clone)]
    pub(crate) struct Handle {
        pub realm: Arc<Realm>,
        pub db: String,
    }

    impl Handle {
        pub fn at(&self, db: &str) -> Handle {
            Handle {
                realm: self.realm.clone(),
                db: db.to_string(),
            }
        }
        pub fn store(&self) -> &Arc<Db> {
            self.realm
                .dbs
                .get(&self.db)
                .expect("no such database in the fake realm")
        }
        pub fn db_at(&self, name: &str) -> &Arc<Db> {
            self.realm
                .dbs
                .get(name)
                .expect("no such database in the fake realm")
        }

        /// Drive one pass of `Coordinator::reap_idle_account_sessions` — the REAL
        /// `AccountSessions::reap_idle` predicate, with the release action modelled rather than
        /// performed. `grace` is passed in so a test can ask "reap everything that is due right
        /// now" (`Duration::ZERO`) or "nothing is due yet" without sleeping.
        ///
        /// The release drops the account's minted identity on EVERY database, which is what makes
        /// this fake able to show the cost a release actually carries: production's
        /// `release_player_conn_on` removes the cached `PlayerConn` from every shard's `players`
        /// map, so the account's next checkout builds a new connection and the node mints it a
        /// DIFFERENT identity — one `establish_session` has not bound. A test can therefore read
        /// "was this connection rebuilt?" straight off `bound_identity`.
        pub fn reap(&self, grace: std::time::Duration) -> Vec<u64> {
            let due = self
                .realm
                .sessions
                .reap_idle(std::time::Instant::now(), grace);
            for id in &due {
                for db in self.realm.dbs.values() {
                    db.identities.lock().unwrap().remove(id);
                }
            }
            self.realm
                .released
                .lock()
                .unwrap()
                .extend(due.iter().copied());
            due
        }

        /// Accounts released so far.
        pub fn released(&self) -> Vec<u64> {
            self.realm.released.lock().unwrap().clone()
        }
    }

    /// Build a realm. `dbs` are the database names (the first is the default world shard), `rules`
    /// is `LYRACORE_SHARD_MAP` text, `realm_core` is `LYRACORE_REALM_CORE`.
    pub(crate) fn realm(dbs: &[&str], rules: &str, realm_core: Option<&str>) -> Handle {
        let map = ShardMap::parse(dbs[0], rules).with_realm_core(realm_core);
        let realm = Realm {
            dbs: dbs
                .iter()
                .map(|d| (d.to_string(), Arc::new(Db::default())))
                .collect(),
            map,
            realm_core_up: true,
            sessions: crate::stdb::AccountSessions::default(),
            released: Mutex::new(Vec::new()),
        };
        Handle {
            realm: Arc::new(realm),
            db: dbs[0].to_string(),
        }
    }

    /// The same, with realm-core's connection DEAD.
    pub(crate) fn realm_with_dead_core(dbs: &[&str], rules: &str, realm_core: &str) -> Handle {
        let h = realm(dbs, rules, Some(realm_core));
        let mut realm = Arc::try_unwrap(h.realm).ok().expect("sole owner");
        realm.realm_core_up = false;
        Handle {
            realm: Arc::new(realm),
            db: h.db,
        }
    }

    pub(crate) fn account(id: u64, username: &str, salt: u8, verifier: u8) -> AccountRow {
        AccountRow {
            id,
            username: username.to_string(),
            salt: vec![salt; 32],
            verifier: vec![verifier; 32],
            banned: false,
        }
    }

    impl RealmDb for Handle {
        fn shard_name(&self) -> &str {
            &self.db
        }
        fn is_sharded(&self) -> bool {
            self.realm.dbs.len() > 1
        }
        fn shard_map(&self) -> &ShardMap {
            &self.realm.map
        }
        fn realm_core(&self) -> Result<Handle> {
            match self.realm.map.realm_core_db() {
                None => Ok(self.clone()),
                Some(db) if self.realm.realm_core_up => Ok(self.at(db)),
                Some(db) => Err(anyhow!(
                    "realm-core database {db} is not connected — refusing to authenticate against \
                     the world database's stale auth cache"
                )),
            }
        }
        fn world_shards(&self) -> Vec<(String, Handle)> {
            self.realm
                .map
                .shards()
                .into_iter()
                .filter(|d| self.realm.dbs.contains_key(d))
                .map(|d| {
                    let h = self.at(&d);
                    (d, h)
                })
                .collect()
        }
        fn account_by_username(&self, username: &str) -> Result<Option<AccountRow>> {
            let db = self.store();
            db.note(&format!("account_by_username({username})"));
            Ok(db.accounts.lock().unwrap().get(username).cloned())
        }
        fn session_key(&self, account_id: u64) -> Result<Option<[u8; 40]>> {
            let db = self.store();
            db.note(&format!("session_key({account_id})"));
            Ok(db
                .sessions
                .lock()
                .unwrap()
                .get(&account_id)
                .map(|(k, _)| *k))
        }
        fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> {
            let db = self.store();
            db.note(&format!("bound_identity({account_id})"));
            // Mints a stable per-(database, account) identity on first use, exactly as
            // `Coordinator::player_conn` does — and that is the point: two databases hand out
            // DIFFERENT identities for the same account id.
            let mut ids = db.identities.lock().unwrap();
            let n = ids.len();
            let mut mints = db.mints.lock().unwrap();
            Ok(*ids.entry(account_id).or_insert_with(|| {
                *mints += 1;
                let mut id = [0u8; 32];
                id[0] = account_id as u8;
                id[1] = n as u8 + 1;
                id[2] = self.db.len() as u8;
                // MONOTONIC, unlike the three above — a rebuild after a release re-enters an
                // empty slot and would otherwise mint a byte-identical identity, hiding the very
                // cost a release carries.
                id[3] = *mints;
                id
            }))
        }
        fn character_count(&self, account_id: u64) -> Result<u8> {
            let db = self.store();
            db.note(&format!("character_count({account_id})"));
            Ok(db
                .characters
                .lock()
                .unwrap()
                .values()
                .filter(|(a, _)| *a == account_id)
                .count() as u8)
        }
        fn realm(&self) -> Result<RealmRow> {
            self.store().note("realm()");
            Ok(RealmRow {
                id: 1,
                name: "Test".into(),
                address: "127.0.0.1:8085".into(),
                realm_type: 0,
                flags: 0,
                population: 0.0,
                timezone: 1,
            })
        }
        fn establish_session(
            &self,
            account_id: u64,
            session_key: &[u8; 40],
            bound_identity: [u8; 32],
        ) -> Result<()> {
            let db = self.store();
            db.note(&format!("establish_session({account_id})"));
            db.sessions
                .lock()
                .unwrap()
                .insert(account_id, (*session_key, bound_identity));
            Ok(())
        }
        fn attach_account_session(&self, account_id: u64) {
            self.store().note(&format!("attach_session({account_id})"));
            self.realm.sessions.attach(account_id);
        }
        fn detach_account_session_deferred(&self, account_id: u64) {
            self.store().note(&format!("detach_session({account_id})"));
            // Byte-for-byte the production call (`Coordinator::detach_account_session_deferred`):
            // park, never release.
            self.realm
                .sessions
                .detach_deferred(account_id, std::time::Instant::now());
        }
        fn character_location(&self, guid: u64) -> Option<(u32, u64)> {
            let db = self.store();
            db.note(&format!("character_location({guid})"));
            db.characters
                .lock()
                .unwrap()
                .get(&guid)
                .map(|(_, loc)| *loc)
        }
        fn character_shard(&self, guid: u64) -> Option<(u32, u64)> {
            let db = self.store();
            db.note(&format!("character_shard({guid})"));
            db.shard_index.lock().unwrap().get(&guid).copied()
        }
        fn set_character_shard(&self, guid: u64, map_id: u32, instance_id: u64) -> Result<()> {
            let db = self.store();
            db.note(&format!("set_character_shard({guid})"));
            db.shard_index
                .lock()
                .unwrap()
                .insert(guid, (map_id, instance_id));
            Ok(())
        }
        fn has_escrow(&self, guid: u64) -> bool {
            let db = self.store();
            db.note(&format!("has_escrow({guid})"));
            db.escrows.lock().unwrap().contains(&guid)
        }
        fn session_count(&self) -> usize {
            let db = self.store();
            db.note("session_count()");
            *db.open_sessions.lock().unwrap()
        }
        fn record_shard_load(
            &self,
            shard: &str,
            writer_occupancy_pct: f32,
            sessions: u32,
        ) -> Result<()> {
            let db = self.store();
            db.note(&format!("record_shard_load({shard})"));
            db.recorded_shard_loads.lock().unwrap().push((
                shard.to_string(),
                writer_occupancy_pct,
                sessions,
            ));
            Ok(())
        }
    }
}

// ===============================================================================================
//  Tests — the eight mutations the adversarial review could not reach
// ===============================================================================================

/// Each test below names the mutation it kills. In short: all eight live in `CoordinatorStore` /
/// `impl WorldStore for Coordinator`, all
/// eight needed a live SpacetimeDB node before this file existed, and the two that matter most —
/// "read the SRP6 material off the world DB" and "read K off the world DB's cache" — are the exact
/// regression the realm-core auth split was built to prevent.
#[cfg(test)]
mod tests {
    use super::fake::{account, realm, realm_with_dead_core};
    use super::*;
    use crate::logon::{CoordinatorStore, LogonStore};
    use std::time::Duration;

    const WORLD: &str = "world";
    const CORE: &str = "lyracore-realm";
    const INSTANCES: &str = "instances";
    const USER: &str = "PONYTAIL";
    const K: [u8; 40] = [7u8; 40];

    /// The topology every auth test uses, and the one the reviewer flagged as the configuration
    /// that breaks id-carrying: the SAME account has a DIFFERENT `#[auto_inc]` id on each database,
    /// because they were provisioned in different orders. The salt/verifier differ too, so "which
    /// database answered" is readable straight off the returned row.
    fn split_realm() -> super::fake::Handle {
        let h = realm(&[WORLD, CORE], "", Some(CORE));
        // realm-core: the authority. id 9, salt 0xAA.
        h.db_at(CORE)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(9, USER, 0xAA, 0xBB));
        // the world shard: a DIFFERENT id, and the write-through cache's stale SRP6 material.
        h.db_at(WORLD)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(3, USER, 0x11, 0x22));
        h
    }

    // -------------------------------------------------------------------------------------
    // M1 — `account()` reads SRP6 material off the world DB, not realm-core
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_srp6_challenge_material_comes_from_realm_core_never_the_world_shards_cache() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        let a = store
            .account(USER)
            .expect("lookup succeeds")
            .expect("the account exists");
        assert_eq!(
            a.salt,
            [0xAA; 32],
            "the logon challenge was answered with the WORLD shard's salt. `game_account` on a \
             world shard is a write-through CACHE — refreshed at logon and \
             never authoritative — so authenticating against it means a password rotation or a ban \
             applied on realm-core silently stops being enforced. This is the regression the \
             realm-core auth split exists to prevent."
        );
        assert_eq!(a.verifier, [0xBB; 32], "same, for the SRP6 verifier");
        assert_eq!(
            a.id, 9,
            "the challenge must carry REALM-CORE's account id: it is the id the \
             proof step writes the authoritative session row under"
        );
    }

    /// A PROVISIONING corruption, not a client one. `game_account.salt`/`.verifier` are
    /// 32-byte vectors in the schema, and nothing in SpacetimeDB enforces the length: a half-written
    /// migration, a hand-typed `spacetime sql` row, or an importer bug can leave a short blob there.
    ///
    /// `CoordinatorStore::account` must report that as an ERROR. The alternative — coercing a short
    /// blob into a zero-padded `[u8; 32]` — would manufacture exactly the all-zero verifier that the
    /// unprovisioned `init` placeholder uses, and `handle_challenge` special-cases all-zero as
    /// "unknown account". A corrupt row would then present as a wrong username: unfixable by the
    /// player, invisible to the operator, and identical in the log to a typo.
    ///
    /// Both fields are checked separately because they are two independent `try_into`s; a fix that
    /// only guarded one would still ship the other.
    #[test]
    fn a_wrong_length_credential_blob_is_an_error_never_a_zero_padded_verifier() {
        for field in ["salt", "verifier"] {
            let h = realm(&[WORLD, CORE], "", Some(CORE));
            let mut row = account(9, USER, 0xAA, 0xBB);
            match field {
                "salt" => row.salt.truncate(31),
                _ => row.verifier.truncate(16),
            }
            h.db_at(CORE)
                .accounts
                .lock()
                .unwrap()
                .insert(USER.into(), row);

            let store = CoordinatorStore::new(h);
            let err = store.account(USER).expect_err(
                "a wrong-length credential blob must be an error — coercing it to a zero-padded \
                 array forges the all-zero `init` placeholder, and the logon path reads that as \
                 `FailUnknownAccount`: a corrupt account becomes indistinguishable from a typo",
            );
            let text = format!("{err:#}");
            assert!(
                text.contains(field) && text.contains("expected 32"),
                "the error must name WHICH blob is malformed and what was expected, or an operator \
                 cannot act on it; got: {text}"
            );
        }
    }

    #[test]
    fn an_unreachable_realm_core_refuses_the_challenge_rather_than_serving_the_stale_cache() {
        let h = realm_with_dead_core(&[WORLD, CORE], "", CORE);
        h.db_at(WORLD)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(3, USER, 0x11, 0x22));
        let store = CoordinatorStore::new(h);
        assert!(
            store.account(USER).is_err(),
            "a dead realm-core must fail the logon CLOSED. Falling through to the world shard \
             serves the SDK's last-known snapshot of salt/verifier/banned — precisely the stale \
             auth cache fail-closed exists to refuse."
        );
    }

    // -------------------------------------------------------------------------------------
    // M2 — `save_session` carries realm-core's id to the world shard
    // M3 — the world-shard write-through cache write is skipped entirely
    // -------------------------------------------------------------------------------------

    #[test]
    fn save_session_writes_each_database_under_that_databases_own_account_id() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        store
            .save_session(9, USER, &K, [0x5A; 32])
            .expect("both writes land");

        assert_eq!(
            h.db_at(CORE).sessions.lock().unwrap().get(&9).map(|(k, _)| *k),
            Some(K),
            "realm-core's `game_session` must be written under REALM-CORE's id (9) — it is the row \
             every world gateway later reads to complete a handshake"
        );
        assert_eq!(
            h.db_at(WORLD).sessions.lock().unwrap().get(&3).map(|(k, _)| *k),
            Some(K),
            "the world shard's write-through cache write is missing, or landed under the wrong id. \
             Its real job is binding `game_character.owner_identity` for this account's characters \
             so per-owner RLS lets the player see them: skip it and the player enters the world \
             with an empty character list. Carry REALM-CORE's id (9) into it instead and — on any \
             deployment whose two databases were seeded in different orders, which is this one — \
             you bind THIS login to ANOTHER account's characters."
        );
        assert!(
            !h.db_at(WORLD).sessions.lock().unwrap().contains_key(&9),
            "realm-core's account id was carried onto the world shard: it names a different player \
             there"
        );
    }

    #[test]
    fn an_unconfigured_realm_core_writes_exactly_one_session_row() {
        // The byte-identical claim, as behaviour: LYRACORE_REALM_CORE unset ⇒ one database, one write.
        let h = realm(&[WORLD], "", None);
        h.db_at(WORLD)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(3, USER, 1, 2));
        let store = CoordinatorStore::new(h.clone());
        store
            .save_session(3, USER, &K, [0x5A; 32])
            .expect("the single write lands");
        assert_eq!(
            h.db_at(WORLD)
                .touched()
                .iter()
                .filter(|c| c.starts_with("establish_session"))
                .count(),
            1,
            "an unconfigured gateway must write `game_session` exactly ONCE — a second, identical \
             reducer call per logon is not byte-identical to the pre-realm-core gateway"
        );
    }

    // -------------------------------------------------------------------------------------
    // M4 — `bound_identity` / `realms` pass realm-core's id to the world coordinator
    // -------------------------------------------------------------------------------------

    #[test]
    fn every_world_side_call_resolves_the_world_shards_own_account_id_from_the_username() {
        let h = split_realm();
        // Two characters, owned by the WORLD shard's ids 3 (ours) and 9 (someone else's — the id
        // realm-core happens to use for us).
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (3, (0, 0)));
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(200, (9, (0, 0)));
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(201, (9, (0, 0)));
        let store = CoordinatorStore::new(h.clone());

        let realms = store.realms(9, USER).expect("realm list");
        assert_eq!(
            realms[0].number_of_characters, 1,
            "the realm-select screen counted a DIFFERENT account's characters: \
             `game_character.account_id` is the WORLD shard's key, and realm-core's id (9) names \
             another player there"
        );

        let ours = store.bound_identity(9, USER).expect("identity");
        let theirs = h.db_at(WORLD).identities.lock().unwrap().get(&9).copied();
        assert!(
            theirs != Some(ours),
            "the world connection was checked out under REALM-CORE's account id. \
             `Coordinator::player_conn` caches it under that id and `establish_session` stamps its \
             identity onto the world shard, but the world phase later checks the connection out \
             under the WORLD shard's id — so either nobody can enter the world (identity not \
             bound), or, with two accounts' ids transposed between the databases, one player \
             re-owns the other's characters."
        );
    }

    #[test]
    fn a_username_missing_on_the_world_shard_is_an_error_not_a_reused_realm_core_id() {
        let h = realm(&[WORLD, CORE], "", Some(CORE));
        h.db_at(CORE)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(9, USER, 0xAA, 0xBB));
        // …and nothing on the world shard.
        let store = CoordinatorStore::new(h);
        assert!(
            store.save_session(9, USER, &K, [0x5A; 32]).is_err(),
            "falling back to realm-core's id hands the world shard an id that, on any deployment \
             whose databases were not seeded in the same order, names a DIFFERENT player"
        );
    }

    // -------------------------------------------------------------------------------------
    // M5 — `lookup_session` reads K off the world DB's cache
    // M6 — `lookup_session` returns realm-core's id as `WorldSession::account_id`
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_world_handshake_reads_k_from_realm_core_and_the_account_id_from_the_world_shard() {
        let h = split_realm();
        // The authoritative session row (realm-core, id 9) and a STALE world-shard cache row under
        // the world's own id 3. Both exist, as they do live after a logon.
        h.db_at(CORE)
            .sessions
            .lock()
            .unwrap()
            .insert(9, (K, [0x5A; 32]));
        h.db_at(WORLD)
            .sessions
            .lock()
            .unwrap()
            .insert(3, ([0xEE; 40], [0x5A; 32]));

        let s = lookup_session(&h, USER)
            .expect("handshake lookup")
            .expect("a live session");
        assert_eq!(
            s.session_key, K,
            "the world handshake completed against the WORLD shard's cached copy of K. That copy \
             is refreshed at logon and never authoritative: sourcing K from it means a \
             gateway restarted mid-session, or a client reconnecting to a different shard's \
             gateway, authenticates off a snapshot instead of realm state. This is the precise \
             regression the realm-wide session row exists to prevent, and it survived every test \
             before this one."
        );
        assert_eq!(
            s.account_id, 3,
            "`WorldSession::account_id` carried REALM-CORE's id (9) into the world phase. Every \
             world-phase call (`characters`, `player_login`, `create_character`) means the WORLD \
             shard's `#[auto_inc]` key — hand it realm-core's and the player is served another \
             account's characters."
        );
    }

    #[test]
    fn the_world_handshake_fails_closed_when_realm_core_is_unreachable() {
        let h = realm_with_dead_core(&[WORLD, CORE], "", CORE);
        h.db_at(WORLD)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(3, USER, 1, 2));
        h.db_at(WORLD)
            .sessions
            .lock()
            .unwrap()
            .insert(3, ([0xEE; 40], [0x5A; 32]));
        assert!(
            lookup_session(&h, USER).is_err(),
            "with realm-core down the handshake must be refused, not served from the world DB's \
             session cache"
        );
    }

    #[test]
    fn an_unconfigured_realm_core_answers_the_handshake_from_the_one_database() {
        let h = realm(&[WORLD], "", None);
        h.db_at(WORLD)
            .accounts
            .lock()
            .unwrap()
            .insert(USER.into(), account(3, USER, 1, 2));
        h.db_at(WORLD)
            .sessions
            .lock()
            .unwrap()
            .insert(3, (K, [0x5A; 32]));
        let s = lookup_session(&h, USER)
            .expect("lookup")
            .expect("a live session");
        assert_eq!(
            (s.account_id, s.session_key),
            (3, K),
            "single-database behaviour is unchanged"
        );
    }

    // -------------------------------------------------------------------------------------
    // M7 — the self-heal index write-back is removed
    // M8 — the `!is_sharded()` short-circuit is removed
    // -------------------------------------------------------------------------------------

    /// Two world shards + realm-core, with the character resident on `instances`.
    fn routed_realm() -> super::fake::Handle {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        h.db_at(INSTANCES)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 7)));
        h
    }

    #[test]
    fn a_deliberately_stale_index_entry_still_routes_correctly_and_is_healed_in_place() {
        let h = routed_realm();
        // The index insists the character is in the open world on the default shard. It is not.
        h.db_at(CORE)
            .shard_index
            .lock()
            .unwrap()
            .insert(100, (0, 0));

        let resolved = settle_shard_index(&h, 100).expect("the probe finds the character");
        assert_eq!(
            (resolved.db.as_str(), resolved.location),
            (INSTANCES, (36, 7)),
            "a wrong hint must cost one extra probe, never a wrong route — the index is a HINT and \
             the character's own row is the truth"
        );
        assert_eq!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((36, 7)),
            "the stale entry was not healed. Without the write-back every login pays the full \
             shard probe forever, and the index — the thing world entry and instance entry route on \
             — never becomes \
             true. This is the fallback that covers a gateway killed between `finish_transfer` and \
             `publish_shard_index`, so it must keep working."
        );
    }

    #[test]
    fn an_absent_index_entry_is_written_the_first_time_the_probe_answers() {
        let h = routed_realm();
        assert!(settle_shard_index(&h, 100).is_some());
        assert_eq!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((36, 7)),
            "the index must be populated by the probe that had to run without it"
        );
    }

    #[test]
    fn a_correct_index_entry_is_not_rewritten() {
        let h = routed_realm();
        h.db_at(CORE)
            .shard_index
            .lock()
            .unwrap()
            .insert(100, (36, 7));
        settle_shard_index(&h, 100).expect("resolves");
        assert!(
            !h.db_at(CORE).touched().iter().any(|c| c.starts_with("set_character_shard")),
            "an index entry the probe agrees with must not be rewritten — a reducer round trip per \
             login, forever, for a row that is already correct"
        );
    }

    #[test]
    fn a_single_database_gateway_resolves_the_home_shard_without_reading_anything() {
        let h = realm(&[WORLD], "", None);
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (0, 0)));
        assert_eq!(
            settle_shard_index(&h, 100),
            None,
            "one database can only ever answer `stay put`"
        );
        assert!(
            h.db_at(WORLD).touched().is_empty(),
            "the `is_sharded` short-circuit is gone: an unconfigured gateway now pays an index \
             read, a character-location probe and (because the index starts empty) a \
             `set_character_shard` reducer round trip on EVERY login, to reach the one answer it \
             could have given for free. `LYRACORE_SHARD_MAP` unset must be byte-identical to the \
             pre-sharding gateway on the login hot path. Reads were: {:?}",
            h.db_at(WORLD).touched()
        );
    }

    // -------------------------------------------------------------------------------------
    // `locate_home_shard`: `settle_home_shard`'s holder lookup, index-first
    // -------------------------------------------------------------------------------------

    /// **The stranded-copy sequence, whole.** A crash between `import_character_blob` and
    /// `finish_transfer` left the player settled on the destination and a durable copy stranded on
    /// the source, escrow still held, with nothing left that would ever re-drive it. Live evidence:
    ///
    /// ```text
    /// outbound:  settle 13: holder=lyracore      owner=lyracore-instances escrow=false (36/42)
    /// recovery:  settle 13: holder=lyracore-instances owner=lyracore-instances escrow=false (36/42)
    /// ```
    ///
    /// The recovery login resolved the DESTINATION as the holder, so the source's escrow was never
    /// read (`settle_home_shard` reads it from the holder alone), owner came out equal to holder,
    /// and `settle_transfer` returned through its no-op branch — no `run_transfer` line in the log
    /// at all.
    ///
    /// Both phases run through the PUBLIC function, in order, because the defect is the handoff
    /// between them: phase 1's write is what makes phase 2 answer wrongly. Testing either alone
    /// passes — phase 1 "heals the index" and phase 2 "trusts the index", both as designed.
    #[test]
    fn a_mid_portal_heal_never_makes_the_next_login_resolve_the_destination_as_the_holder() {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        // PHASE 1 — the outbound login. She is on WORLD, and her location already carries the
        // portal's destination (`teleport_player` stamps `pending_instance_id` before the transfer
        // is driven), so `character_location` on WORLD answers the map INSTANCES owns.
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 42)));
        assert_eq!(
            locate_home_shard(&h, 100)
                .expect("she is on WORLD")
                .shard_name(),
            WORLD,
            "the outbound resolution itself was never wrong — WORLD holds the only durable row"
        );
        assert_ne!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((36, 42)),
            "the index now claims she lives on the shard that owns map 36, while her only durable \
             row is on WORLD and no escrow has even been opened yet. The index answers WHICH SHARD \
             holds her, and this heal predicted a move instead of confirming one — `publish_shard \
             _index` after a successful `finish_transfer` is the only thing allowed to move it."
        );

        // PHASE 2 — the crash landed between import and finish, then the player logs back in.
        // WORLD holds the frozen source copy and the escrow; INSTANCES holds the imported copy.
        h.db_at(WORLD).escrows.lock().unwrap().insert(100);
        h.db_at(INSTANCES)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 42)));
        assert_eq!(
            locate_home_shard(&h, 100).expect("resolves").shard_name(),
            WORLD,
            "resolution must answer the shard holding the ESCROW — that is the SOURCE, and the only \
             shard `settle_transfer` can re-drive the unfinished transfer from. Answering INSTANCES \
             (which merely holds the imported copy) makes owner == holder, so the resume is never \
             driven and the source copy is stranded forever — reproduced live 3/3 runs."
        );
    }

    /// **The scan must not resolve by iteration order.**
    ///
    /// Reproduced by a reviewer: a guid with a stale row on the DEFAULT shard and its real
    /// row elsewhere, with no index entry, resolved to the default shard because the scan took the
    /// first shard that answered at all and `shards` is default-first.
    ///
    /// The disambiguator is "a durable row is authoritative only on the shard the shard map assigns
    /// its own location to". Mutation targets: drop the `resolve(...) == shard_name()` comparison, or
    /// collapse `agreeing.or(disagreeing)` to `disagreeing.or(agreeing)`, and this goes red.
    #[test]
    fn the_scan_prefers_the_shard_whose_row_agrees_with_the_map_over_iteration_order() {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        // WORLD is the default shard and is visited FIRST. It carries a stale artefact whose location
        // (map 36) the shard map assigns to INSTANCES — so it describes a character living elsewhere.
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 7)));
        // INSTANCES carries the real row, and map 36 resolves to INSTANCES, so it agrees.
        h.db_at(INSTANCES)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 7)));
        // No index entry: this is the fallback-scan path, exactly the reviewer's setup.
        assert_eq!(
            locate_home_shard(&h, 100).expect("resolves").shard_name(),
            INSTANCES,
            "the scan returned the DEFAULT shard's stale artefact because it was visited first. A \
             durable row is authoritative only on the shard the map assigns its own location to; \
             resolving by iteration order sends the player to a shard they do not live on."
        );
    }

    /// …and the preference must never become a FILTER: if the only row anywhere disagrees with the
    /// map, it is still returned. Routing to a stale-looking row is recoverable and is what happens
    /// today; routing to nothing is a failed login.
    ///
    /// Mutation target: change `agreeing.or(disagreeing)` to plain `agreeing` and this goes red.
    #[test]
    fn a_lone_disagreeing_row_still_resolves_rather_than_failing_the_login() {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        // The character's ONLY row is on WORLD, carrying a location the map assigns to INSTANCES —
        // e.g. mid-portal, where `teleport_player` has already stamped the destination.
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 7)));
        assert_eq!(
            locate_home_shard(&h, 100).map(|d| d.shard_name().to_string()),
            Some(WORLD.to_string()),
            "the only durable row in the realm was discarded for disagreeing with the shard map, \
             which turns a recoverable stale row into a login that resolves to nothing at all"
        );
    }

    /// The SAME rule with the roles swapped, and it exists because the test above does not pin the
    /// rule it claims to.
    ///
    /// An adversarial review found a wrong guard that passes all 496 tests:
    ///
    /// ```ignore
    /// if shard.shard_name() == shard.shard_map().default_db() { return; }   // "never heal on the default shard"
    /// ```
    ///
    /// Every other test in this module puts the character on `INSTANCES` when a heal must be
    /// PERMITTED and on `WORLD` (the default) when it must be DECLINED — so "holder disagrees with
    /// the shard map" and "holder is the default database" are indistinguishable across the whole
    /// suite. That mutant is not harmless: it never heals an open-world character at all, and it
    /// fails to fix the stranded copy on the RETURN leg (instances → world), where the real guard declines
    /// `resolve(0,0) == "world" != "instances"` and the mutant permits — putting the index back to
    /// naming `world` while the escrow sits on `instances`, straight back into the strand.
    ///
    /// So: character found on the NON-default shard, carrying a location the default shard owns.
    /// The heal must still decline. Passes with the real guard, red with the mutant.
    #[test]
    fn a_heal_is_declined_by_the_shard_map_not_by_being_the_default_database() {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        // She is durable on INSTANCES, but her location is open-world map 0 — which the shard map
        // assigns to WORLD. Mid-portal home, in other words: the mirror image of phase 1 above.
        h.db_at(INSTANCES)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (0, 0)));
        locate_home_shard(&h, 100);
        assert_ne!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((0, 0)),
            "the heal wrote a location that resolves to WORLD for a character whose durable row is \
             on INSTANCES. The index answers WHICH SHARD holds her, so this entry names the wrong \
             one — and on the return leg of a transfer that strands the copy again, with the escrow on \
             INSTANCES and the index pointing at WORLD."
        );
    }

    /// The read this whole issue is about: a correct index entry must be TRUSTED, not just
    /// consulted-then-ignored. If `locate_home_shard` stopped reading the index (falling straight
    /// through to the scan), this still "routes correctly" by accident — the scan finds the same
    /// answer — so the pin is the ACCESS LOG, not the routing answer: deleting the index
    /// consultation makes both assertions below fail.
    #[test]
    fn locate_home_shard_trusts_a_correct_index_entry_without_scanning_the_other_shard() {
        let h = routed_realm();
        h.db_at(CORE)
            .shard_index
            .lock()
            .unwrap()
            .insert(100, (36, 7));
        let found = locate_home_shard(&h, 100).expect("resolves");
        assert_eq!(
            found.shard_name(),
            INSTANCES,
            "must route to the shard the index names"
        );
        assert!(
            h.db_at(CORE)
                .touched()
                .iter()
                .any(|c| c.starts_with("character_shard(100)")),
            "the realm-core index was never consulted — `settle_home_shard`'s production path \
             would be back to the unconditional scan this lookup exists to replace"
        );
        assert!(
            h.db_at(WORLD).touched().is_empty(),
            "a correct index entry must not cost a scan of the OTHER connected shard — reads \
             were: {:?}",
            h.db_at(WORLD).touched()
        );
    }

    #[test]
    fn a_wrong_index_entry_still_routes_correctly_on_the_production_path_and_is_healed() {
        let h = routed_realm();
        // The index insists the character is in the open world on the default shard. It is not.
        h.db_at(CORE)
            .shard_index
            .lock()
            .unwrap()
            .insert(100, (0, 0));
        let found = locate_home_shard(&h, 100).expect("the scan finds the character");
        assert_eq!(
            found.shard_name(),
            INSTANCES,
            "a wrong hint must cost one extra scan, never a wrong route"
        );
        assert_eq!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((36, 7)),
            "the stale entry must be healed on THIS path — `settle_shard_index`'s identical rule \
             does not run in production (it hangs off the unreachable `home_shard`); this is the \
             function `settle_home_shard` actually calls"
        );
    }

    #[test]
    fn an_absent_index_entry_is_populated_by_the_scan_that_had_to_run_without_it() {
        let h = routed_realm();
        assert!(locate_home_shard(&h, 100).is_some());
        assert_eq!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((36, 7)),
            "the index must be populated by the scan that had to run without it"
        );
    }

    #[test]
    fn a_correct_index_entry_is_not_rewritten_by_locate_home_shard() {
        let h = routed_realm();
        h.db_at(CORE)
            .shard_index
            .lock()
            .unwrap()
            .insert(100, (36, 7));
        locate_home_shard(&h, 100);
        assert!(
            !h.db_at(CORE)
                .touched()
                .iter()
                .any(|c| c.starts_with("set_character_shard")),
            "an index entry the scan agrees with must not be rewritten — a reducer round trip per \
             login, forever, for a row that is already correct"
        );
    }

    #[test]
    fn locate_home_shard_degrades_to_the_scan_when_realm_core_is_unreachable() {
        let h = realm_with_dead_core(&[WORLD, INSTANCES, CORE], "36:*=instances", CORE);
        h.db_at(INSTANCES)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 7)));
        let found = locate_home_shard(&h, 100).expect("the scan still finds the character");
        assert_eq!(
            found.shard_name(),
            INSTANCES,
            "a dead realm-core must cost the hint, not the login — routing degrades to the scan, \
             exactly like `settle_shard_index`"
        );
    }

    /// Sibling of `locate_character_still_prefers_the_shard_holding_the_escrow` (the older scan's
    /// own tripwire): `locate_home_shard`'s FALLBACK scan must keep the same priority, or a resumed
    /// transfer gets driven from the wrong side of a fenced-import window.
    #[test]
    fn locate_home_shard_still_prefers_the_shard_holding_the_escrow_in_the_fallback_scan() {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        // No index entry, so this forces the fallback scan. BOTH databases hold a durable row for
        // guid 100 — the fenced-import window — and WORLD is iterated FIRST (default-first
        // order), so a plain first-found scan would wrongly pick it.
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (0, 0)));
        h.db_at(INSTANCES)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (36, 7)));
        h.db_at(INSTANCES).escrows.lock().unwrap().insert(100);
        let found = locate_home_shard(&h, 100).expect("resolves");
        assert_eq!(
            found.shard_name(),
            INSTANCES,
            "the shard holding the ESCROW must win outright over an earlier-iterated shard that \
             merely holds a durable row — it is the only side a resumed transfer can be driven from"
        );
    }

    /// With `LYRACORE_REALM_CORE` (and `LYRACORE_SHARD_MAP`) unset, the resolver must be
    /// byte-identical to the pre-sharding gateway — no realm-core read, no shard scan, no heal
    /// write. `settle_home_shard`'s OWN `if !self.is_sharded() { return Ok(None); }` (`stdb::
    /// world_store.rs`) already guards this in production, unchanged, but it lives on `Coordinator`
    /// and cannot be exercised by any test without a live SpacetimeDB connection — an adversarial
    /// review confirmed inverting it left all 468 + 57 gateway tests green, caught only by a
    /// source-scan substring check that still matches the inverted line. This function's OWN copy
    /// of the same short-circuit is what a test can actually reach, so it is what is pinned here.
    #[test]
    fn locate_home_shard_short_circuits_on_an_unsharded_gateway_reading_nothing() {
        let h = realm(&[WORLD], "", None);
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(100, (1, (0, 0)));
        assert!(
            locate_home_shard(&h, 100).is_none(),
            "one connected database can only ever answer `stay put`"
        );
        assert!(
            h.db_at(WORLD).touched().is_empty(),
            "an unsharded gateway paid a read resolving its OWN home shard — `LYRACORE_REALM_CORE`/\
             `LYRACORE_SHARD_MAP` unset must cost the login hot path nothing. Reads \
             were: {:?}",
            h.db_at(WORLD).touched()
        );
    }

    // -------------------------------------------------------------------------------------
    // `resolve_delete_shard`: `delete_character`'s cross-shard routing
    // -------------------------------------------------------------------------------------

    /// The bug this routing exists to fix, proven at the resolver: a character resident on a NON-default
    /// shard must resolve to THAT shard, not `None` (which the caller reads as "delete on `self`").
    #[test]
    fn resolve_delete_shard_routes_to_the_shard_actually_holding_the_character() {
        let h = routed_realm();
        let owner = resolve_delete_shard(&h, 100)
            .expect("resolves")
            .expect("must route OFF the default shard — the character lives on `instances`");
        assert_eq!(
            owner.shard_name(),
            INSTANCES,
            "delete must run on the shard actually holding the row, not the default `world` shard \
             — routing it there is exactly the NOT_FOUND-shaped failure players hit"
        );
    }

    /// A character that already lives on the asking handle still resolves — to a handle naming
    /// `self`'s OWN database, not a wrong one. `resolve_delete_shard` does not special-case "already
    /// here" into `None`; the caller treats `Some(self-equivalent)` and `None` identically (both end
    /// up calling `delete_character` on a `Coordinator` pointed at this database), so what matters
    /// is that the resolution never drifts onto a DIFFERENT shard when it should not.
    #[test]
    fn resolve_delete_shard_still_names_self_when_the_character_already_lives_here() {
        let h = realm(&[WORLD, INSTANCES, CORE], "36:*=instances", Some(CORE));
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(200, (1, (0, 0)));
        let owner = resolve_delete_shard(&h, 200)
            .expect("resolves")
            .expect("the character is resident somewhere — it must resolve, not vanish");
        assert_eq!(
            owner.shard_name(),
            WORLD,
            "a character already resident on `self` must resolve back to `self`'s OWN database, \
             never to a different one"
        );
    }

    /// An unknown/never-existed guid must not be manufactured a route either — `None`, so the
    /// caller's existing single-database delete answers NOT_FOUND exactly as it always did.
    #[test]
    fn resolve_delete_shard_answers_none_for_an_unknown_character() {
        let h = routed_realm();
        assert!(
            resolve_delete_shard(&h, 999_999)
                .expect("resolves")
                .is_none(),
            "an unresolvable guid must fall through to `self`'s own NOT_FOUND, not be routed"
        );
    }

    /// The corruption guard (the design constraint on this routing): a character mid-transfer must be REFUSED,
    /// never deleted out from under a resumed transfer.
    #[test]
    fn resolve_delete_shard_refuses_a_character_mid_transfer() {
        let h = routed_realm();
        h.db_at(INSTANCES).escrows.lock().unwrap().insert(100);
        assert!(
            resolve_delete_shard(&h, 100).is_err(),
            "a character with an in-flight escrow must be refused — deleting either half of a \
             split-in-flight row either destroys the character a moment before it lands on the \
             other side, or leaves a frozen copy the resumed transfer then fails against"
        );
    }

    /// This resolver's own copy of the single-shard short-circuit: an unconfigured gateway must pay zero
    /// reads resolving a delete target, exactly like `locate_home_shard`'s identical rule.
    #[test]
    fn resolve_delete_shard_short_circuits_on_an_unsharded_gateway_reading_nothing() {
        let h = realm(&[WORLD], "", None);
        h.db_at(WORLD)
            .characters
            .lock()
            .unwrap()
            .insert(1, (1, (0, 0)));
        assert!(resolve_delete_shard(&h, 1).expect("resolves").is_none());
        assert!(
            h.db_at(WORLD).touched().is_empty(),
            "an unsharded gateway paid a read resolving a delete target — `LYRACORE_REALM_CORE`/\
             `LYRACORE_SHARD_MAP` unset must cost delete nothing new. Reads were: {:?}",
            h.db_at(WORLD).touched()
        );
    }

    // -------------------------------------------------------------------------------------
    // The realm-core index write on transfer completion
    // -------------------------------------------------------------------------------------

    #[test]
    fn publishing_a_settled_transfer_writes_the_realm_core_index_not_the_world_shards() {
        let h = routed_realm();
        publish_shard_index(&h, 100, 36, 7).expect("the publish lands");
        assert_eq!(
            h.db_at(CORE).shard_index.lock().unwrap().get(&100).copied(),
            Some((36, 7)),
            "the transfer's destination was not published to REALM-CORE — the only copy of the \
             index `home_shard` reads. Writing it on the world handle instead lands it on a copy \
             nothing consults."
        );
        assert!(
            h.db_at(WORLD).shard_index.lock().unwrap().is_empty(),
            "the publish must go to realm-core, not to whichever world handle drove the transfer"
        );
    }

    #[test]
    fn publishing_the_index_fails_loudly_when_realm_core_is_unreachable() {
        let h = realm_with_dead_core(&[WORLD, INSTANCES, CORE], "36:*=instances", CORE);
        assert!(
            publish_shard_index(&h, 100, 36, 7).is_err(),
            "a publish that silently swallowed an unreachable realm-core would be exactly the \
             best-effort, independently-committing write this replication exists to remove"
        );
    }

    // -------------------------------------------------------------------------------------
    // The logon tier's connection lease
    //
    // The world tier's half shipped first; what remains is a logon that authenticates and never
    // proceeds to a world session, whose per-account connection (opened by `bound_identity`) nothing
    // ever reclaims — one websocket fd and one SDK pump thread per such account, for the process
    // lifetime. That is the same class that exited the gateway with `Too many open files`.
    //
    // These run the REAL `stdb::AccountSessions` through the REAL `CoordinatorStore`; only the
    // release ACTION is modelled (`fake::Handle::reap`), and it is modelled faithfully enough that a
    // rebuilt connection is visible as a changed identity. `Duration::ZERO` means "sweep everything
    // already parked", so the handover window is exercised without sleeping through it.
    // -------------------------------------------------------------------------------------

    /// A world session's `WorldStore::open_account_session`, which is
    /// `Coordinator::attach_account_session` — the SAME `AccountSessions` the logon store attaches
    /// to, because both reach it through one `Arc<ShardSet>` on one cloned `Coordinator`.
    fn world_session_opens(h: &super::fake::Handle, world_account_id: u64) {
        RealmDb::attach_account_session(h, world_account_id);
    }

    /// The leak this issue is scoped to. Nothing but the reaper can reclaim it: `handle_logon`
    /// returned, the socket is gone, and no world session ever attached.
    #[test]
    fn a_logon_that_never_enters_the_world_has_its_connection_reclaimed() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        store.bound_identity(9, USER).expect("identity");
        assert!(
            h.reap(Duration::ZERO).is_empty(),
            "a logon socket that is still OPEN (sitting on the realm-list screen) is a live user of \
             the connection — reaping it there would break the realm select it is in the middle of"
        );

        drop(store); // the client quit at the realm list; `handle_logon` returned

        assert_eq!(
            h.reap(Duration::ZERO),
            vec![3],
            "the logon's per-account connection was never released. Every account that ever \
             authenticated without playing then holds a websocket fd + an SDK pump thread for the \
             gateway's lifetime, and `accept(2)` eventually returns EMFILE"
        );
    }

    /// DANGER CASE — the handover, and the reason the release is deferred rather than immediate.
    ///
    /// The 1.12 client's logon socket may close before OR after the world handshake; that ordering
    /// is client behaviour we do not control. Releasing at the logon close would therefore rebuild
    /// the connection on most logins — and a rebuilt connection carries a NEW identity, while the
    /// one `establish_session` bound into `game_session` and onto `game_character.owner_identity`
    /// moments earlier is the old one.
    #[test]
    fn the_world_session_inherits_the_connection_the_logon_opened() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        let bound = store.bound_identity(9, USER).expect("identity");
        store
            .save_session(9, USER, &K, bound)
            .expect("the session row is written");

        // The dangerous order: the client drops its logon socket the moment it starts the world
        // connection, so the account is momentarily down to ZERO sockets while still mid-login.
        drop(store);
        assert!(
            h.reap(Duration::from_secs(120)).is_empty(),
            "a sweep landing inside the handover window released the connection. The grace exists \
             precisely because this window is not empty — the logon close and the world handshake \
             are two independent client actions and we control the order of neither"
        );
        world_session_opens(&h, 3); // CMSG_AUTH_SESSION completes, milliseconds later
        assert!(
            h.reap(Duration::ZERO).is_empty(),
            "the connection the world session just took over was released anyway — the handover \
             must CANCEL the pending release, not merely postpone it"
        );

        let world_phase = CoordinatorStore::new(h.clone())
            .bound_identity(9, USER)
            .expect("identity");
        assert_eq!(
            world_phase, bound,
            "the world session got a REBUILT connection. Its identity is not the one \
             `establish_session` bound, so `account_by_identity` fails for every reducer the player \
             calls — and even where it recovers, rebuilding an SDK connection on every single login \
             is exactly the per-account cost the connection cache removed"
        );
    }

    /// The other close order, and the one where the plain refcount already suffices: the world
    /// session attaches while the logon socket is still open, so its close parks nothing at all.
    #[test]
    fn a_logon_close_after_the_world_handshake_parks_nothing() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        let bound = store.bound_identity(9, USER).expect("identity");
        world_session_opens(&h, 3);
        drop(store);
        assert!(h.reap(Duration::ZERO).is_empty());
        assert_eq!(
            CoordinatorStore::new(h.clone())
                .bound_identity(9, USER)
                .expect("identity"),
            bound
        );
    }

    /// DANGER CASE — a world session is already live (a player seated, or parked at character
    /// select) when a fresh logon arrives on the same account. This is the case that makes
    /// `release_session`'s EPOCH the wrong gate: a socket at character select holds no epoch, so an
    /// epoch-gated release would cut it. The socket refcount does not have that blind spot.
    #[test]
    fn a_fresh_logon_never_cuts_a_live_world_sessions_connection() {
        let h = split_realm();
        world_session_opens(&h, 3); // already playing
        let store = CoordinatorStore::new(h.clone());
        let bound = store.bound_identity(9, USER).expect("identity");
        drop(store); // the second client gave up at the realm list

        assert!(
            h.reap(Duration::ZERO).is_empty(),
            "the seated player's connection was released out from under them"
        );
        assert_eq!(
            CoordinatorStore::new(h.clone())
                .bound_identity(9, USER)
                .expect("identity"),
            bound,
            "the seated player's connection was rebuilt, so their bound identity is now stale"
        );
    }

    /// DANGER CASE — two concurrent logons on one account. The first close must not release what
    /// the second is using, and the two together must release exactly once.
    #[test]
    fn two_concurrent_logons_release_the_connection_once_at_the_last_close() {
        let h = split_realm();
        let first = CoordinatorStore::new(h.clone());
        let second = CoordinatorStore::new(h.clone());
        first.bound_identity(9, USER).expect("identity");
        second.bound_identity(9, USER).expect("identity");

        drop(first);
        assert!(
            h.reap(Duration::ZERO).is_empty(),
            "the second logon is still using this connection"
        );
        drop(second);
        assert_eq!(h.reap(Duration::ZERO), vec![3]);
        assert_eq!(h.released(), vec![3], "exactly one release, at the last close");
    }

    /// The lease is taken on the WORLD shard's account id, never the authenticating database's
    /// id. Getting this wrong accounts a connection that does not exist and leaves the real one
    /// unaccounted — a leak that still looks fixed on a single-database deployment, where the two
    /// ids happen to coincide.
    #[test]
    fn the_lease_is_taken_on_the_world_shards_account_id() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        store.bound_identity(9, USER).expect("identity");
        assert_eq!(
            (
                h.realm.sessions.live_count(3),
                h.realm.sessions.live_count(9)
            ),
            (1, 0),
            "the logon leased realm-core's account id (9). `player_conn` caches under the WORLD \
             shard's id (3), so the connection that was actually opened is unaccounted and leaks, \
             while a phantom entry for 9 is what gets released"
        );
        drop(store);
        assert_eq!(h.reap(Duration::ZERO), vec![3]);
    }

    /// `handle_logon` LOOPS: a client may re-run challenge+proof on one socket. Each extra proof
    /// would attach again, but only one `Drop` pairs them — so a double attach pins the connection
    /// at a non-zero count forever, which is the leak wearing the fix's clothes.
    #[test]
    fn re_proving_the_same_account_on_one_socket_leases_at_most_once() {
        let h = split_realm();
        let store = CoordinatorStore::new(h.clone());
        store.bound_identity(9, USER).expect("identity");
        store.bound_identity(9, USER).expect("identity again");
        assert_eq!(h.realm.sessions.live_count(3), 1);
        drop(store);
        assert_eq!(
            h.reap(Duration::ZERO),
            vec![3],
            "a re-proof on the same socket left the account's refcount above zero, so nothing will \
             ever release its connection"
        );
    }

    /// …and re-authenticating as a DIFFERENT account on the same socket hands the first one back,
    /// rather than pinning a connection no socket uses any more.
    #[test]
    fn re_authenticating_as_another_account_hands_the_first_lease_back() {
        let h = split_realm();
        const OTHER: &str = "SECOND";
        h.db_at(CORE)
            .accounts
            .lock()
            .unwrap()
            .insert(OTHER.into(), account(11, OTHER, 0xCC, 0xDD));
        h.db_at(WORLD)
            .accounts
            .lock()
            .unwrap()
            .insert(OTHER.into(), account(4, OTHER, 0x33, 0x44));

        let store = CoordinatorStore::new(h.clone());
        store.bound_identity(9, USER).expect("identity");
        store.bound_identity(11, OTHER).expect("identity");
        assert_eq!(h.realm.sessions.live_count(3), 0);
        assert_eq!(h.realm.sessions.live_count(4), 1);
        assert_eq!(
            h.reap(Duration::ZERO),
            vec![3],
            "the abandoned account's connection stayed pinned by a socket that no longer uses it"
        );
        drop(store);
        assert_eq!(h.reap(Duration::ZERO), vec![4]);
    }

    // -------------------------------------------------------------------------------------
    // The seam's own blind spot: `Coordinator`'s forwarding impl
    // -------------------------------------------------------------------------------------

    /// `impl RealmDb for Coordinator` is the ONE layer the fake substitutes for wholesale, and
    /// every method in it is a one-line forward whose damage would be total and silent — pointing
    /// `realm_core()` at `self`, or `session_key` at the wrong table, is invisible to every test
    /// above. So it is compared for EXACT SHAPE, the same way `module/src/transfer/mod.rs` pins
    /// `CtxShard`: a `contains` scan is defeated by leaving the old text in a dead branch, equality
    /// is not. If a change here is deliberate, re-bless it with the same care.
    ///
    /// `has_escrow` is the one method whose forward is not BARE — it narrows `escrow_row`'s
    /// `Option<TransferOut>` to a bool — and it is spelled out below for the same reason: the
    /// narrowing itself cannot pick the wrong database, but a change to it (e.g. reading a
    /// different table, or always answering `false`) is exactly the kind of edit this test exists
    /// to catch.
    #[test]
    fn the_coordinator_forwards_are_views_not_logic() {
        let src = include_str!("stdb/world_store.rs");
        let at = src
            .find("impl crate::realm_core::RealmDb for Coordinator {")
            .expect("`impl RealmDb for Coordinator` moved out of world_store.rs");
        let body = &src[at..];
        let end = body.find("\n}\n").expect("unterminated impl block");
        let shape = body[..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let want = "impl crate::realm_core::RealmDb for Coordinator { \
            fn shard_name(&self) -> &str { self.shard_name() } \
            fn is_sharded(&self) -> bool { self.is_sharded() } \
            fn shard_map(&self) -> &crate::config::ShardMap { self.shard_map() } \
            fn realm_core(&self) -> Result<Coordinator> { self.realm_core() } \
            fn world_shards(&self) -> Vec<(String, Coordinator)> { self.world_shards() } \
            fn account_by_username(&self, username: &str) -> Result<Option<AccountRow>> { \
            self.account_by_username(username) } \
            fn session_key(&self, account_id: u64) -> Result<Option<[u8; 40]>> { \
            self.session_key(account_id) } \
            fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> { \
            self.bound_identity(account_id) } \
            fn character_count(&self, account_id: u64) -> Result<u8> { \
            self.character_count(account_id) } \
            fn realm(&self) -> Result<RealmRow> { self.realm() } \
            fn establish_session( &self, account_id: u64, session_key: &[u8; 40], bound_identity: [u8; 32], ) \
            -> Result<()> { self.establish_session(account_id, session_key, bound_identity) } \
            fn attach_account_session(&self, account_id: u64) { \
            self.attach_account_session(account_id) } \
            fn detach_account_session_deferred(&self, account_id: u64) { \
            self.detach_account_session_deferred(account_id) } \
            fn character_location(&self, guid: u64) -> Option<(u32, u64)> { \
            self.character_location(guid) } \
            fn character_shard(&self, guid: u64) -> Option<(u32, u64)> { self.character_shard(guid) } \
            fn set_character_shard(&self, guid: u64, map_id: u32, instance_id: u64) -> Result<()> { \
            self.set_character_shard(guid, map_id, instance_id) } \
            fn has_escrow(&self, guid: u64) -> bool { self.escrow_row(guid).is_some() } \
            fn session_count(&self) -> usize { self.session_count() } \
            fn record_shard_load( &self, shard: &str, writer_occupancy_pct: f32, sessions: u32, ) \
            -> Result<()> { self.record_shard_load(shard, writer_occupancy_pct, sessions) }"
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            shape, want,
            "`impl RealmDb for Coordinator` is no longer a block of pass-throughs to \
             `Coordinator`'s own inherent methods. Everything in `realm_core.rs` is tested through \
             this impl with a fake substituted for it, so an edit here — `realm_core()` returning \
             `self.clone()`, `session_key` reading the wrong account — is invisible to all of it \
             while the whole suite stays green."
        );
    }
}
