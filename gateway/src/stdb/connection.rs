//! Connection tier: the `Coordinator` facade + its inner shared state, the live privileged
//! connection (with the reconnect watchdog), the per-account player connections, the
//! `call_reducer!` macro, and the connection lifecycle constructors. Reads/reducers/subscriptions
//! live in sibling modules as additional `impl Coordinator` blocks.

use crate::config::{GatewayConfig, ShardMap};
use anyhow::{anyhow, Context, Result};
use spacetimedb_sdk::{DbContext, Identity, SubscriptionHandle as _, Table as _};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use super::bindings::*;

/// Shared handle to the privileged coordination connection **of one shard**.
///
/// Field 0 is the shard this handle talks to — every read, every reducer call, and every
/// subscription in the `reads`/`reducers`/`subscriptions` sibling modules goes through it, so
/// "which database does this call hit" is decided entirely by which handle you hold. Field 1 is
/// the shared, shard-independent routing table + connection set (#17): `shard_for` mints the
/// handle for another shard, and `Coordinator::connect` hands back the DEFAULT shard's handle.
/// With a single-entry shard map the set has exactly one entry and `shard_for` always resolves
/// back to `self` — byte-identical to the pre-sharding gateway.
#[derive(Clone)]
pub struct Coordinator(pub(crate) Arc<CoordinatorInner>, pub(crate) Arc<ShardSet>);

/// The gateway's whole multi-database view: the routing table, one coordinator connection per
/// database it names, and the (gateway-local, shard-INDEPENDENT) session-epoch arbitration.
///
/// Session epochs deliberately live here rather than per-shard: they arbitrate two SOCKETS on one
/// account, which is a gateway concept with no database in it. Keeping them shared means a login
/// that routes to shard B and a teardown that runs on shard B's handle still compare the same
/// epochs (a per-shard `SessionEpochs` would silently break the #42 stale-logout gate).
pub(crate) struct ShardSet {
    map: ShardMap,
    /// db name → that database's coordinator connection. Always contains the default database;
    /// an extra shard that failed to connect is ABSENT (routing then degrades to the default).
    conns: HashMap<String, Arc<CoordinatorInner>>,
    sessions: SessionEpochs,
}

/// Per-account "current in-world session" tracking. The world gateway opens one TCP session per
/// socket, but the cached `PlayerConn` shares ONE node identity across reconnects — so when a stale
/// socket finally tears down and asks the module to delete the player's entity, the module can't
/// tell it apart from a newer session that re-logged in on the same account, and would delete the
/// LIVE player. Each `player_login` claims a fresh epoch (becoming the current owner of the entity);
/// teardown deletes the entity only if its epoch is still current. The gateway must arbitrate this
/// because the distinction (which socket) only exists here, not in the module.
#[derive(Default)]
pub(crate) struct SessionEpochs {
    next: AtomicU64,
    current: Mutex<HashMap<u64, u64>>,
}

impl SessionEpochs {
    /// Claim a fresh epoch for `account_id` and make it current.
    fn claim(&self, account_id: u64) -> u64 {
        let epoch = self.next.fetch_add(1, Ordering::Relaxed);
        self.current.lock().unwrap().insert(account_id, epoch);
        epoch
    }

    /// Release `epoch`; returns true iff it was still the current epoch (caller owns the entity, so
    /// it's safe to delete it on logout), false if a newer login superseded it (do NOT delete).
    fn release(&self, account_id: u64, epoch: u64) -> bool {
        let mut current = self.current.lock().unwrap();
        if current.get(&account_id) == Some(&epoch) {
            current.remove(&account_id);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod session_epoch_tests {
    use super::SessionEpochs;

    #[test]
    fn newer_login_supersedes_a_stale_session() {
        let s = SessionEpochs::default();
        let acct = 42;
        let a = s.claim(acct); // socket A enters the world
        let b = s.claim(acct); // socket B re-logs on the same account, superseding A
        assert_ne!(a, b);
        // A's late teardown must NOT delete the entity — B owns it now (the bug this fixes).
        assert!(
            !s.release(acct, a),
            "a superseded epoch must not own the entity"
        );
        // B's teardown does own it and deletes once; a double release is a no-op.
        assert!(s.release(acct, b), "the current epoch owns the entity");
        assert!(
            !s.release(acct, b),
            "releasing an already-released epoch is a no-op"
        );
    }

    #[test]
    fn distinct_accounts_are_independent() {
        let s = SessionEpochs::default();
        let e1 = s.claim(1);
        let e2 = s.claim(2);
        assert!(s.release(1, e1));
        assert!(s.release(2, e2));
    }
}

/// The live privileged connection + the handles that keep its pump/subscription alive. Swapped
/// wholesale by the watchdog on a reconnect (the old one's pump thread has already exited and its
/// subscription is dead, so dropping it is clean).
pub(crate) struct LiveConn {
    /// Privileged (owner) connection: reads every table (RLS bypass) and is the cache the
    /// gateway reads through. Reducers gated on the *player's* identity go through `PlayerConn`.
    pub(crate) conn: DbConnection,
    /// Keeps the SDK message-pump thread alive for the connection's lifetime.
    _pump: std::thread::JoinHandle<()>,
    /// Keeps the privileged subscription active for the connection's lifetime.
    _sub: SubscriptionHandle,
}

pub(crate) struct CoordinatorInner {
    /// The live coordinator connection, swappable so the watchdog can reconnect in place. Reads take a
    /// (near-always-uncontended) read lock for the duration of one query/call; the watchdog takes the
    /// write lock only for the instant swap — so the steady-state (no-drop) read path is unchanged.
    live: RwLock<LiveConn>,
    /// Connect params kept so the watchdog can rebuild the connection.
    token: Option<String>,
    /// Connection params for opening per-account player connections lazily.
    uri: String,
    db_name: String,
    /// Whether this connection subscribes the multi-database tables (#20's `game_character_shard`,
    /// #23's `game_map_region` + `game_region_assignment`) — kept so the watchdog rebuilds with the
    /// SAME subscription set it was created with. See [`coordinator_queries`].
    sharded_tables: bool,
    /// Per-account player connections (each with its own node-issued identity == the account's
    /// bound identity). Opened on first need (at logon, when `bound_identity` is read) and reused
    /// for the world phase so `player_login`/`movement_update` run as the player, not the owner.
    players: Mutex<HashMap<u64, Arc<PlayerConn>>>,
    /// Re-arm hook for a connection-scoped relay that has no per-login registration point to
    /// self-heal through after a reconnect (issue #54's bot-invite relay is the first of these, and
    /// as of now the only one — every OTHER coordinator relay re-registers itself on the fresh
    /// connection implicitly, because it is armed from inside a per-player LOGIN, which happens
    /// again after any reconnect. This one is armed once at gateway startup with no login to hang a
    /// re-arm off, so without this hook a reconnect (the watchdog's own doc comment: "self-heal
    /// across a SpacetimeDB migration" — a module republish, which this project does routinely) would
    /// leave it silently registered on a dead, disconnected `LiveConn` forever).
    ///
    /// `None` until [`super::subscriptions::Coordinator::spawn_bot_invite_relay`] sets it (nothing to
    /// re-arm if that was never called); the watchdog invokes whatever is set, if anything, right
    /// after each successful reconnect swap — AFTER, because the hook re-reads `coord()`, which must
    /// already be the fresh connection.
    pub(crate) on_reconnect: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// #72 slice 2's fold-in nit: memoized `game_map_region` decode. `Coordinator::map_regions` used
    /// to rebuild + re-validate the WHOLE table (and re-log every rejected row) on every call; that
    /// was fine at "once per world entry" but the warm-handoff seam check calls it on every cell
    /// crossing a moving player makes. `Arc<Mutex<..>>` — not a plain field — because
    /// [`connect_blocking`] needs a handle to register invalidating callbacks against BEFORE this
    /// struct exists (the very first connect), and the watchdog's reconnect must hand the REBUILT
    /// connection the SAME handle so its callbacks invalidate the one cache `map_regions` actually
    /// reads; see `spawn_coordinator_watchdog`.
    map_regions_cache: Arc<Mutex<Option<lyracore_shared::region::RegionMap>>>,
}

impl CoordinatorInner {
    /// The live coordinator connection (read-locked for the call). A reconnect swaps the inner connection
    /// under the write lock, so callers transparently pick up the fresh connection on their NEXT access.
    /// Recovers a poisoned lock (matches the file's `.lock().unwrap()` discipline) — a stale read beats
    /// poisoning every future request.
    pub(crate) fn coord(&self) -> std::sync::RwLockReadGuard<'_, LiveConn> {
        self.live.read().unwrap_or_else(|p| {
            log::error!(
                "coordinator read lock poisoned (a prior panic in a critical section) — recovering"
            );
            p.into_inner()
        })
    }

    /// #72 slice 2: read the memoized `game_map_region` decode, building it (once) on a miss —
    /// the first call after connect/reconnect, or after the invalidating `on_insert`/`on_delete`
    /// callbacks `connect_blocking` registered have cleared it. `build` is never called on a hit,
    /// which is the whole point: `Coordinator::map_regions` is now hot (the warm-handoff seam
    /// check calls it on every cell crossing), and rebuilding decoded + re-validated the WHOLE
    /// table plus re-logged every rejected row on every single call.
    pub(crate) fn cached_map_regions(
        &self,
        build: impl FnOnce() -> lyracore_shared::region::RegionMap,
    ) -> lyracore_shared::region::RegionMap {
        let mut cache = self.map_regions_cache.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
        let built = build();
        *cache = Some(built.clone());
        built
    }
}

/// A per-account SpacetimeDB connection. Its node-issued `identity` is bound to the account by
/// `establish_session`, so reducers it calls satisfy the module's `ctx.sender == owner` checks.
pub(crate) struct PlayerConn {
    pub(crate) conn: DbConnection,
    pub(crate) identity: Identity,
    /// The `run_threaded` pump, JOINABLE (292). The pump holds a CLONE of the `DbConnection`, so the
    /// connection and its caches are freed only once this thread RETURNS — dropping the `PlayerConn`
    /// is not enough, somebody has to reap it. `LiveConn` above already does this on a reconnect.
    ///
    /// Read ONLY by `release_player_conn`, which is itself unreachable today — see its note.
    #[allow(dead_code)]
    pump: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The coordinator subscription set, as a pure function of ONE flag — so the property that
/// matters can be asserted without a node (see the tests at the bottom of this file).
///
/// `sharded_tables` adds the tables that only mean something on a MULTI-DATABASE deployment:
/// `game_character_shard` (#20) and `game_map_region` / `game_region_assignment` (#23). It is OFF
/// for a single-database gateway, and that is load-bearing rather than an optimization: **a
/// subscription to a table the deployed module does not have FAILS TO APPLY**, which fails
/// `connect_blocking`, which — for the default database — fails `Coordinator::connect` and the
/// whole gateway. Subscribing them unconditionally would therefore mean a gateway restarted
/// before the module was republished never comes back, in a configuration (`LYRACORE_SHARD_MAP` and
/// `LYRACORE_REALM_CORE` both unset) that #17, #20 and #23 all promise costs nothing. Nothing reads
/// any of them on a single-database gateway anyway — `WorldStore::home_shard` short-circuits on
/// `is_sharded()` before it looks at either the index or the region overlay.
fn coordinator_queries(sharded_tables: bool) -> Vec<&'static str> {
    let mut queries = vec![
        "SELECT * FROM game_realm",
        "SELECT * FROM game_account",
        "SELECT * FROM game_session",
        "SELECT * FROM game_character",
        "SELECT * FROM game_world_entity",
        // The SOURCE-side escrow (#19). The ONE module→gateway data flow the cross-database
        // transfer adds: the export blob is written here by `begin_transfer` and the gateway
        // carries it to the other database's `import_character_blob`. Private table, read
        // through the owner token like game_account/game_session. Also the whole of transfer
        // RECOVERY: a restarted gateway re-derives an in-flight transfer's id, destination and
        // payload from this row alone.
        //
        // BASE list, not `sharded_tables`: the table is #16's escrow primitive, present on every
        // module since long before any of the sharding tickets, so subscribing it unconditionally
        // cannot brick the restart of a gateway whose module predates #17/#20/#23. Nothing READS
        // it unsharded (`settle_home_shard` short-circuits on `is_sharded()`).
        "SELECT * FROM game_transfer_out",
        // Teleport events (277): the TRANSFER relay rides THIS stable connection — the
        // per-player conn's AOI grid subscriptions churn mid-flight, and the SDK's in-flight
        // subscription apply could swallow a concurrent transaction's deltas (an instance-
        // CREATING portal entry — ~200 rows + the event — lost its teleport event and the
        // player limbo'd, despawned with no SMSG_TRANSFER pair ever sent). Owner token
        // bypasses the recipient RLS, so every player's events arrive; each session's
        // callback self-filters by mover_guid.
        "SELECT * FROM game_teleport_event",
        // XP/levelup events (279, same rationale): they ride large kill transactions and
        // their relays moved off the churning per-player conn onto this stable one.
        "SELECT * FROM game_xp_event",
        "SELECT * FROM game_levelup_event",
        // Bot-initiated (serendipity) group invites (issue #54): the module cannot reach realm-core
        // to write a party itself, so it writes the DECISION here and the coordinator's
        // `world::party::run_bot_invite` relay executes it — the same guid-based `realm_group_op` a
        // player's own CMSG_GROUP_INVITE takes. Rides this stable connection for the same reason the
        // XP/levelup/teleport relays do: there is no player connection to ride at all here (a bot has
        // none), so the coordinator is the ONLY connection that could ever see this row.
        //
        // BASE list, not `sharded_tables`: bots run on every topology, single-database included —
        // gating this behind sharding would silently stop every serendipity invite on the common
        // single-database dev/test deployment (`local-test-harness`), the opposite of #48's
        // `game_config` reasoning above but the same list for the same kind of reason. One-time
        // bootstrap cost, same as every other base-list table added after day one: a gateway built
        // against this change must not be RESTARTED until the module carrying this table has been
        // published, or `connect_blocking`'s subscription fails to apply (`coordinator_queries`'s own
        // doc comment).
        "SELECT * FROM game_bot_invite_intent",
        // Addon-bridge messages (184): the server→client UI stream — coordinator-ridden
        // from day one (the 279 delivery law). Any addon UI that streams live state rides
        // this, and a dropped frame is a stuck widget on the client.
        "SELECT * FROM game_addon_message",
        // Explored areas (200/279 live find): the fog-word relay's coordinator leg — a fresh
        // login's first-movement discovery lands exactly in the per-player AOI-churn window.
        "SELECT * FROM game_character_explored",
        // Server-wide tunables (issue #48). The gateway reads ONE column: `hosts_instances`, at
        // startup, to answer "when this realm creates a dungeon instance, will anything actually
        // spawn its population" (`ShardMap::check_instance_hosting`). Before this subscription the
        // gateway could only GUESS — #39's `instance_hosting_reminder` warned on every sharded
        // startup regardless of the flag's value, and the far more damaging inverse (flag false with
        // no routing ⇒ every dungeon created with 0 entities) was undetectable in either process.
        //
        // BASE list, not `sharded_tables`, and that is load-bearing: #48's failure mode happens with
        // NO shard map at all, so a single-database gateway is exactly the one that must read the
        // flag. `game_config` is public and predates every sharding ticket, so subscribing it cannot
        // brick the restart of a gateway whose module predates #17/#20/#23 (the hazard
        // `coordinator_queries`' doc comment describes). Its BINDING must stay in sync, though —
        // `hosts_instances` was END-appended by #39 and the generated `server_config_type.rs` had
        // not been regenerated since, which `gateway/tests/schema_parity.rs` now pins.
        "SELECT * FROM game_config",
        "SELECT * FROM game_creature_template",
        "SELECT * FROM game_start_position",
        "SELECT * FROM game_corpse",
        // Items slice-1: the static item definitions (for CMSG_ITEM_QUERY_SINGLE) + every
        // owned-item instance. The coordinator authenticates as the module owner, which bypasses
        // RLS, so it reads all players' item rows (like game_character) to drive each login spawn.
        "SELECT * FROM game_item_template",
        // Static spell headers — spell_cast_time() reads cast_time_ms via coord() so the
        // CMSG_CAST_SPELL handler can clear INSTANT casts synchronously. [083]
        "SELECT * FROM game_spell",
        // Spell effects — the gateway reads these to route ITEM-target enchant/disenchant casts by
        // effect KIND (E_ENCHANT_ITEM/E_DISENCHANT) instead of a hardcoded spell-id list. [094]
        "SELECT * FROM game_spell_effect",
        "SELECT * FROM game_item_instance",
        // Buyback ring (248): read by the coordinator to rebuild a player's buyback-tab view
        // after sell/buyback and at login (private table — the owner-token coordinator sees it).
        "SELECT * FROM game_character_buyback",
        // Items slice-4: a creature corpse's item loot, read for the loot window.
        "SELECT * FROM game_corpse_loot",
        // Vendors (Tier 2): a creature's vendor stock, read for SMSG_LIST_INVENTORY.
        "SELECT * FROM game_npc_vendor",
        // Gossip menus + NPC text (+ weighted slots) + menu options (work-item 217): per-NPC text
        // for SMSG_GOSSIP_MESSAGE title_text_id and SMSG_NPC_TEXT_UPDATE body, and the imported
        // clickable options that take precedence over the vendor/innkeeper synthesis. Imported by
        // the world ETL; fallback to generic greeting / flag-derived options when a creature's
        // entry has no rows.
        "SELECT * FROM game_gossip_menu",
        "SELECT * FROM game_gossip_option",
        "SELECT * FROM game_npc_text",
        "SELECT * FROM game_npc_text_slot",
        // Quests (gateway slice): the static quest tables + every player's quest log. The
        // coordinator bypasses RLS, so game_character_quest is read for any player to compute the
        // overhead `!`/`?` status + the completion gate.
        "SELECT * FROM game_quest_template",
        "SELECT * FROM game_quest_text",
        "SELECT * FROM game_quest_objective",
        "SELECT * FROM game_quest_reward_item",
        "SELECT * FROM game_quest_reward_choice",
        "SELECT * FROM game_creature_quest",
        // Work-item 041: the GAMEOBJECT twin of `game_creature_quest` — GO 68 "Wanted Poster"
        // starts q176 with no creature giver at all, and the GO 55/56 "Lost Guards" corpses drive
        // the q37/q45/q71 END chain. Without this subscription the coordinator's `RemoteTables`
        // cache of `game_gameobject_quest` stays empty forever (the binding/decode plumbing landed
        // earlier, but nothing ever asked the node to send rows) and `quest_giver_evals`'s GO
        // fallback would silently evaluate against zero relations no matter what the module holds.
        "SELECT * FROM game_gameobject_quest",
        "SELECT * FROM game_character_quest",
        // #10: the player's learned spells, chained into the login spellbook (e.g. Auto Shot).
        "SELECT * FROM game_player_spell",
        // Work-item 212: imported action-bar rows (hand-written binding — see
        // `stdb/bindings/player_action_type.rs`'s header note). Empty until a dump import runs;
        // the coordinator's own read cache needs this subscription the same way game_player_spell
        // does above (RLS-bypassed, so player_actions() can read ANY player's rows for login).
        "SELECT * FROM game_player_action",
        // Trainers: the static teach-list (SMSG_TRAINER_LIST) + every player's trained skills. The
        // coordinator bypasses RLS, so game_player_skill is read for any player to compute each
        // profession offering's "known" (gray-out a tier already trained past) in trainer_list.
        "SELECT * FROM game_trainer_spell",
        "SELECT * FROM game_spell_chain",
        "SELECT * FROM game_player_skill",
        // Gameobjects: spawned props + templates, relayed to clients as GameObject CREATE_OBJECT,
        // joined for the query reply + the chest-loot use.
        "SELECT * FROM game_gameobject",
        "SELECT * FROM game_gameobject_template",
        // Talents: static talent metadata for the gateway to look up grant_spell_id on CMSG_LEARN_TALENT,
        // plus every character's spent ranks (RLS-bypassed like game_player_skill) so the live pick
        // relays the rank-spell (SMSG_LEARNED_SPELL/SUPERCEDED) and the decremented
        // PLAYER_CHARACTER_POINTS1 — without these the 5875 talent pane freezes until relog.
        "SELECT * FROM game_talent",
        "SELECT * FROM game_character_talent",
        // Spell-modifier mirror (264): the coordinator reads every player's A_SPELLMOD auras to
        // build the SMSG_SET_FLAT/PCT_SPELL_MODIFIER packets at login + on a talent pick.
        "SELECT * FROM game_aura",
        // #13 slice 2 (work-item 076): the player's persisted reputation standings, chained into the
        // login SMSG_INITIALIZE_FACTIONS. Without this subscription the coordinator's local cache of
        // game_player_reputation is empty and player_reputations() always returns nothing (the bug
        // this fixes — the relay path for the live SET_FACTION_STANDING update subscribes per-player
        // separately in subscriptions.rs, but the coordinator's own read cache needs its own sub too).
        "SELECT * FROM game_player_reputation",
        // Friends/ignore (work-item 130): every character's contact rows, so the coordinator can
        // build any player's SMSG_FRIEND_LIST/SMSG_IGNORE_LIST (RLS-bypassed, like game_character).
        "SELECT * FROM game_character_contact",
        // Reaction gating (195 slice A): standing-derived reaction for interaction windows —
        // the NPC's template resolves to its parent faction (rep bar or mask fallback). Both
        // static/small (314 + ~470 rows).
        "SELECT * FROM game_faction",
        "SELECT * FROM game_faction_template",
        // Guid ranges (#108). Two one-row tables, subscribed on EVERY deployment because a database
        // that holds no range refuses to create characters at all: `ensure_guid_ranges` reads the
        // allocator's high-water to decide which range the database is already minting from, and
        // reads back the range it ends up holding to log it.
        "SELECT * FROM game_guid_allocator",
        "SELECT * FROM game_guid_range",
    ];
    if sharded_tables {
        // Character→shard index (#20). On realm-core this is the realm-wide directory the login
        // path consults; on a world shard it is the receipt `finish_transfer` writes in the escrow's
        // own transaction. Private, like game_account/game_session — the owner token reads it, no
        // client ever sees it. Multi-database deployments only (see this function's doc comment).
        queries.push("SELECT * FROM game_character_shard");
        // Region definitions + the epoch-versioned region→shard assignment (#23). The
        // DEFINITIONS are content data baked by the world ETL onto the world shards; the
        // ASSIGNMENT is authoritative on realm-core. Both are subscribed on every connection
        // in the set because each handle reads its own database's copy and the gateway picks
        // which copy is authoritative (`Coordinator::region_shard_for`), exactly as it does
        // for accounts and sessions.
        queries.push("SELECT * FROM game_map_region");
        queries.push("SELECT * FROM game_region_assignment");
        // Party state (#22, group slice). On REALM-CORE these three are the authoritative party
        // tables the gateway drives and relays from; on a world shard they are that shard's mirror,
        // which the gateway also reads (a session's own roster at world entry, before it has pushed
        // anything). Subscribed on every connection in the set for the same reason accounts and
        // regions are: each handle reads its own database's copy, and the gateway decides which copy
        // is authoritative.
        //
        // `game_group_event` is the RELAY: on a world shard the per-player connection subscribes it
        // under RLS (unchanged, that is the pre-#22 path), and on realm-core the owner-token
        // coordinator reads every player's rows and each session self-filters by `recipient_guid` —
        // the coordinator-relay law from 277/279. A single-database gateway subscribes NONE of the
        // three here and keeps exactly its old relay.
        queries.push("SELECT * FROM game_group");
        queries.push("SELECT * FROM game_group_member");
        queries.push("SELECT * FROM game_group_event");
        // Whispers (#22, whisper slice) — the same relay shape one table over. On REALM-CORE this is
        // where a whisper between two players on DIFFERENT shards is written (a guid is the only
        // realm-wide name a recipient has), read here through the owner token and self-filtered per
        // session by `recipient_guid`. On a world shard the per-player connection keeps subscribing it
        // under RLS, which is the pre-#22 path and the only one a single-database gateway has.
        queries.push("SELECT * FROM game_whisper_event");
        // Loot rolls (#50) — a DIFFERENT reason than every table above: nothing here is a CLIENT
        // relay (`game_group_event` still carries every wire-visible roll transition, unchanged). The
        // gateway's own loot-roll relay (`world::loot::relay_tick`) needs these two PRIVATE tables to
        // do its job: on a WORLD SHARD, `pending_local_rolls` reads a freshly kill-time-created
        // staging roll to promote onto realm-core; on REALM-CORE, the promoted/voted rows are what
        // `remove_member`'s disband branch resolves directly. Subscribed on every connection in the
        // set for the same reason party state is: each handle reads its own database's copy, and the
        // relay decides which copy needs which action.
        queries.push("SELECT * FROM game_loot_roll");
        queries.push("SELECT * FROM game_loot_roll_vote");
        // The guid-range REGISTRY (#108) — realm-core's answer to which shard mints from where.
        // Only meaningful in a multi-database deployment: with one database there is one minter and
        // nothing to arbitrate.
        queries.push("SELECT * FROM game_guid_range_registry");
    }
    queries
}

/// Build + connect the privileged SDK connection on a dedicated OS thread.
///
/// `DbConnection::build()` creates/uses a tokio runtime via `block_in_place`; calling it from a
/// tokio worker or a `spawn_blocking` thread panics. A plain `std::thread` has no ambient runtime,
/// so the SDK builds and owns its own — the supported pattern for a native client.
///
/// `sharded_tables` adds the MULTI-DATABASE tables (#20's `game_character_shard`, #23's
/// `game_map_region` + `game_region_assignment`) to the subscription set — see
/// [`coordinator_queries`] for why that flag exists and must stay off by default.
///
/// `map_regions_cache` is #72 slice 2's memoization handle (see [`CoordinatorInner`]'s field doc):
/// registered here as `on_insert`/`on_delete` callbacks on `game_map_region` that invalidate it,
/// so the first `map_regions()` read after THIS connection (initial connect, or a watchdog
/// reconnect) always rebuilds, and every read in between is free. Gated on `sharded_tables` —
/// unsharded, the table is never subscribed, so a row can never arrive to invalidate against.
fn connect_blocking(
    uri: String,
    db_name: String,
    token: Option<String>,
    sharded_tables: bool,
    map_regions_cache: Arc<Mutex<Option<lyracore_shared::region::RegionMap>>>,
) -> Result<LiveConn> {
    let conn = DbConnection::builder()
        .with_uri(&uri)
        .with_database_name(&db_name)
        .with_token(token)
        .on_connect(|_ctx, identity, _token| log::info!("coordinator connected as {identity}"))
        .on_connect_error(|_ctx, err| log::error!("coordinator connect error: {err}"))
        .on_disconnect(|_ctx, err| log::warn!("coordinator connection closed: {err:?}"))
        .build()
        .map_err(|e| anyhow!("coordinator build/connect failed: {e}"))?;

    if sharded_tables {
        let cache = map_regions_cache.clone();
        conn.db.game_map_region().on_insert(move |_ctx, _row| {
            *cache.lock().unwrap() = None;
        });
        let cache = map_regions_cache.clone();
        conn.db.game_map_region().on_delete(move |_ctx, _row| {
            *cache.lock().unwrap() = None;
        });
    }

    let pump = conn.run_threaded();

    // Block until the privileged subscription is applied so the cache reads below see data.
    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let tx_err = tx.clone();
    let queries = coordinator_queries(sharded_tables);
    let sub = conn
        .subscription_builder()
        .on_applied(move |_ctx| {
            let _ = tx.send(Ok(()));
        })
        .on_error(move |_ctx, err| {
            let _ = tx_err.send(Err(format!("{err}")));
        })
        .subscribe(queries);

    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => log::info!("coordinator subscriptions applied"),
        Ok(Err(e)) => return Err(anyhow!("coordinator subscription error: {e}")),
        Err(_) => {
            return Err(anyhow!(
                "coordinator subscriptions not applied within 15s (node down, or token lacks \
                 access to the private game_account/game_session tables?)"
            ))
        }
    }

    Ok(LiveConn {
        conn,
        _pump: pump,
        _sub: sub,
    })
}

/// How often the coordinator watchdog polls connection liveness.
const COORDINATOR_WATCHDOG_POLL: Duration = Duration::from_secs(3);

/// Background watchdog: detect a dropped coordinator connection and rebuild it IN PLACE so the gateway
/// self-heals across a SpacetimeDB migration / network blip — no manual restart. Each poll it checks
/// BOTH liveness signals: the socket (`conn.is_active()`) AND the subscription (`_sub.is_active()`).
/// Neither subsumes the other — a migration can invalidate the SUBSCRIPTION while the socket stays up
/// (`conn.is_active()` stays true, and the SDK's subscription `on_disconnect` is a no-op), and a raw
/// socket drop leaves the subscription's status at `Applied` — so we heal when EITHER is down. On a drop
/// it rebuilds a fresh connection + resubscribes OFF-LOCK (the up-to-15s apply must not block readers),
/// swaps it in under the write lock (instant), then tears the OLD connection down off-lock (disconnect +
/// join its pump thread, so it's reaped rather than detached-and-leaked — on the subscription-death path
/// the old socket was still live, so its pump is still running). While rebuilding, cache reads return the
/// stale/empty view (infallible `Option` reads → `None`/missing row, e.g. `player_login`'s "not visible"
/// path → a clean relog, never a panic); once the swap lands, every reader picks up the fresh connection
/// on its next access. Retries on failure; never panics.
fn spawn_coordinator_watchdog(inner: Arc<CoordinatorInner>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("stdb-coordinator-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(COORDINATOR_WATCHDOG_POLL);
            {
                // ONE guard for both checks (so a swap can't land between them). Healthy → keep polling.
                let live = inner.coord();
                if live.conn.is_active() && live._sub.is_active() {
                    continue;
                }
            } // drop the read guard BEFORE the (blocking) rebuild
            log::warn!("coordinator connection/subscription down — rebuilding (gateway self-heal)");
            match connect_blocking(
                inner.uri.clone(),
                inner.db_name.clone(),
                inner.token.clone(),
                inner.sharded_tables,
                inner.map_regions_cache.clone(),
            ) {
                Ok(fresh) => {
                    // Swap in the fresh connection under the write lock (instant), capturing the OLD one;
                    // release the lock, THEN tear the old one down off-lock.
                    let old = {
                        let mut guard = inner.live.write().unwrap_or_else(|p| {
                            log::error!("coordinator lock poisoned on reconnect swap — recovering");
                            p.into_inner()
                        });
                        std::mem::replace(&mut *guard, fresh)
                    };
                    log::info!("coordinator reconnected + resubscribed");
                    // Re-arm any relay with no per-login point to self-heal through (issue #54) — MUST run
                    // after the swap above: the hook re-reads `inner.coord()`, which by now answers the
                    // fresh connection. Cloned out from under its own lock so the hook runs unlocked (it
                    // takes `live`'s read lock itself via `coord()`).
                    let hook = inner.on_reconnect.lock().unwrap().clone();
                    if let Some(hook) = hook {
                        hook();
                    }
                    // Deterministic teardown of the old connection: disconnect() winds down its pump (it's
                    // still running on the subscription-death path), then join() reaps the thread. Both
                    // are benign no-ops if the old socket was already dead.
                    let _ = old.conn.disconnect();
                    if let Err(e) = old._pump.join() {
                        log::warn!("old coordinator pump thread panicked on teardown: {e:?}");
                    }
                }
                Err(e) => log::error!("coordinator reconnect failed (will retry): {e:#}"),
            }
        })
        .expect("spawn coordinator watchdog")
}

/// How often the loot-roll relay (#50) promotes staging rolls and settles resolved winners. Short
/// relative to the 60s roll window: promotion latency is the ONLY thing it bounds (a vote cast in the
/// gap between a roll's kill-time creation and its promotion is rejected as "no roll open on that
/// item" and must be retried — the vote is per-action, so the client's own retry-on-click covers it),
/// and a real client cannot render the roll popup and react to it within one tick anyway.
const LOOT_ROLL_RELAY_POLL: Duration = Duration::from_millis(200);

/// Background loop driving `world::loot::relay_tick` (#50): promotes each world shard's freshly
/// staged loot rolls onto realm-core, and settles resolved winners back down. A no-op on an unsharded
/// gateway — `relay_tick` returns immediately when `Coordinator::realm_store()` answers `None` — so
/// this thread costs a single-database deployment nothing but its own idle wakeups.
fn spawn_loot_roll_relay(coordinator: Coordinator) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("stdb-loot-roll-relay".into())
        .spawn(move || {
            let mut won_watermark: u64 = 0;
            loop {
                std::thread::sleep(LOOT_ROLL_RELAY_POLL);
                crate::world::loot::relay_tick(&coordinator, &mut won_watermark);
            }
        })
        .expect("spawn loot-roll relay")
}

/// Background loop driving `load_sample::sample_and_record` (#78): every `LYRACORE_LOAD_SAMPLE_SECS`
/// (default 30s), scrape the node's `/v1/metrics` for each configured shard's writer occupancy,
/// read this gateway's own session + region-population counts, and record all of it onto
/// realm-core — then log one SHARDLOAD line per shard, the same "visible without SQL" convention
/// QUEUESTAT/AOISTAT use (`world/mod.rs`). Costs an unconfigured single-database gateway nothing
/// but its own idle wakeups and one metrics scrape (occupancy still measurable there — realm-core
/// unconfigured just means `record_shard_load` runs against this same database).
fn spawn_load_sampler(coordinator: Coordinator, stdb_uri: String) -> std::thread::JoinHandle<()> {
    let sampler = crate::load_sample::OccupancySampler::from_env(&stdb_uri);
    let interval = crate::load_sample::sample_interval();
    std::thread::Builder::new()
        .name("stdb-load-sampler".into())
        .spawn(move || loop {
            std::thread::sleep(interval);
            let occupancy = sampler.sample_all();
            for line in crate::load_sample::sample_and_record(&coordinator, &occupancy) {
                log::info!("{line}");
            }
        })
        .expect("spawn load sampler")
}

/// **Issue #108 — every world shard holds a guid range before it can mint a character.**
///
/// Runs once per gateway start, right where `check_instance_hosting` enforces the other
/// cross-database invariant: this is the only place that knows both realm-core and the shard set,
/// and it is the courier between them — a module cannot read another database, so the claim can
/// only be driven from here.
///
/// Per world shard: read the high-water it is already minting from, ask realm-core to assign it a
/// range (idempotent — a shard keeps the one it holds), then install that range on the shard. The
/// mark is what makes the four live databases land on the floors #105 gave them by hand, in any
/// order (`realm_core::assign_slot`).
///
/// **Never fatal.** The guarantee is enforced module-side, in `create_character`, which refuses
/// outright without a range; a shard that fails to claim here simply cannot create characters, and
/// says so at the point of creation. Aborting the gateway would take down a whole realm's *play*
/// over a *creation* problem. Realm-core is skipped: it holds accounts, not characters, and never
/// mints a character guid.
fn ensure_guid_ranges(conns: &HashMap<String, Arc<CoordinatorInner>>, map: &ShardMap) {
    // `call_reducer!` expands to a `?`, so each call needs a Result-returning home.
    fn claim(rc: &Arc<CoordinatorInner>, shard: &str, mark: u64) -> Result<Option<u64>> {
        call_reducer!(
            rc.coord().conn.reducers,
            "claim_guid_range",
            claim_guid_range_then(shard.to_string(), mark)
        )?;
        // The claim committed on THIS connection, so its row normally rides in with the reducer
        // callback — but the cache update is a separate delivery, and losing this read would leave
        // the shard unlicensed (creation refused) until someone restarted the gateway. Same bounded
        // poll `player_login` uses for the same reason, one connection over.
        for _ in 0..100 {
            if let Some(row) = rc
                .coord()
                .conn
                .db
                .game_guid_range_registry()
                .shard_name()
                .find(&shard.to_string())
            {
                return Ok(Some(row.base));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(None)
    }
    fn install(inner: &Arc<CoordinatorInner>, base: u64) -> Result<()> {
        call_reducer!(
            inner.coord().conn.reducers,
            "install_guid_range",
            install_guid_range_then(base)
        )
    }
    let realm_core = map
        .realm_core_db()
        .and_then(|rc| conns.get(rc).map(|c| (rc, c)));
    for shard in map.shards() {
        if Some(shard.as_str()) == map.realm_core_db() {
            continue;
        }
        let Some(inner) = conns.get(&shard) else {
            continue;
        }; // unreachable shard: already logged
        let mark = inner
            .coord()
            .conn
            .db
            .game_guid_allocator()
            .id()
            .find(&0)
            .map_or(0, |a| a.high_water);
        let base = match &realm_core {
            Some((rc_name, rc)) => match claim(rc, &shard, mark) {
                Ok(Some(base)) => base,
                Ok(None) => {
                    log::error!(
                        "{rc_name} accepted the claim for shard {shard} but published no registry \
                         row — character creation there stays refused (#108)"
                    );
                    continue;
                }
                Err(e) => {
                    log::error!(
                        "shard {shard} could not claim a guid range from {rc_name} ({e:#}) — it \
                         will REFUSE to create characters until it holds one (#108)"
                    );
                    continue;
                }
            },
            // No realm-core: there is no registry to arbitrate, so there is also only one authority
            // deciding anything — this gateway. Slot 0 keeps a single-database realm (and every dev
            // run) minting exactly as it did before #108. A SECOND world shard in this state is the
            // #103 hazard itself, so it is called out rather than quietly assigned.
            None => {
                if map.shards().len() > 1 {
                    log::error!(
                        "shard {shard} has no realm-core to assign it a guid range, and this \
                         gateway serves {} world shards — set LYRACORE_REALM_CORE, or these shards will \
                         mint COLLIDING character guids (#103/#108)",
                        map.shards().len()
                    );
                    continue;
                }
                0
            }
        };
        match install(inner, base) {
            Ok(()) => log::info!("shard {shard} mints character guids from base {base} (#108)"),
            Err(e) => log::error!(
                "shard {shard} refused the guid range assigned to it ({e:#}) — it will not create \
                 characters (#108). This means the assignment disagrees with what it has already \
                 minted; do not force it."
            ),
        }
    }
}

/// Build + connect a per-account player connection on a dedicated OS thread (same no-ambient-
/// runtime reasoning as `connect_blocking`). Connects with no token so the node mints a fresh
/// identity; that identity becomes the account's bound identity. Blocks until connected so the
/// identity is known. No subscriptions: this connection only *calls* reducers (so they run as
/// the player); all reads go through the privileged coordinator cache.
fn connect_player_blocking(uri: String, db_name: String) -> Result<PlayerConn> {
    let (id_tx, id_rx) = std::sync::mpsc::channel::<Identity>();
    let conn = DbConnection::builder()
        .with_uri(&uri)
        .with_database_name(&db_name)
        .on_connect(move |_ctx, identity, _token| {
            let _ = id_tx.send(identity);
        })
        .on_connect_error(|_ctx, err| log::error!("player connect error: {err}"))
        .build()
        .map_err(|e| anyhow!("player build/connect failed: {e}"))?;

    let pump = conn.run_threaded();
    let identity = id_rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| anyhow!("player connection not established within 15s"))?;
    log::info!("player connection established as {identity}");

    Ok(PlayerConn {
        conn,
        identity,
        pump: std::sync::Mutex::new(Some(pump)),
    })
}

/// Block on a reducer-completion channel, mapping the outcome to `anyhow`.
pub(crate) fn recv_reducer(
    rx: std::sync::mpsc::Receiver<std::result::Result<(), String>>,
    what: &str,
) -> Result<()> {
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(anyhow!("{what} reducer failed: {e}")),
        Err(_) => Err(anyhow!("{what} reducer timed out after 10s")),
    }
}

#[cfg(test)]
mod coordinator_query_tests {
    use super::coordinator_queries;

    /// Every table that only exists on a MULTI-DATABASE deployment. A subscription to one of these
    /// against a module that predates it FAILS TO APPLY and takes the whole gateway down on
    /// restart (`coordinator_queries`' doc comment) — so an unconfigured gateway must ask for none.
    const MULTI_DB_TABLES: &[&str] = &[
        "SELECT * FROM game_character_shard",
        "SELECT * FROM game_map_region",
        "SELECT * FROM game_region_assignment",
        // #22 (group slice). `game_group_event` is the one entry here that ALSO has a
        // single-database subscriber — the per-player connection, under RLS — so it is listed as a
        // multi-database table for the COORDINATOR only. Subscribing it on the coordinator of an
        // unconfigured gateway would be a second delivery of every group event (the relay would fire
        // twice per row) on top of the restart hazard this list exists for.
        "SELECT * FROM game_group",
        "SELECT * FROM game_group_member",
        "SELECT * FROM game_group_event",
        // #22 (whisper slice), listed for the same reason as `game_group_event` above: the table
        // predates sharding and has a per-player RLS subscriber, so it is a multi-database table for
        // the COORDINATOR only. Subscribing it on an unconfigured gateway's coordinator would be a
        // second delivery of every whisper the moment the realm relay is registered.
        "SELECT * FROM game_whisper_event",
        // #50: both PRIVATE, no per-player subscriber to duplicate — the restart hazard alone is why
        // they belong on this list (a module published before they exist refuses the subscription).
        "SELECT * FROM game_loot_roll",
        "SELECT * FROM game_loot_roll_vote",
        // #108: realm-core's guid-range registry. The per-shard `game_guid_range`/
        // `game_guid_allocator` pair is deliberately NOT here — a database that holds no range
        // cannot create characters at all, so every deployment needs those two, sharded or not.
        "SELECT * FROM game_guid_range_registry",
    ];

    #[test]
    fn a_single_database_gateway_subscribes_no_multi_database_table() {
        // #33's review caught exactly this class of regression — a table subscribed on every
        // coordinator connection even when unconfigured, which bricks a gateway restarted before
        // the module was republished. #23 adds two more tables to the same trap, so the guard
        // gets a NAMED test rather than a comment.
        let single = coordinator_queries(false);
        for table in MULTI_DB_TABLES {
            assert!(
                !single.contains(table),
                "{table} is subscribed on a single-database gateway — a module published before \
                 that table exists will refuse the subscription and the gateway will not start"
            );
        }
    }

    #[test]
    fn the_multi_database_flag_adds_exactly_those_tables_and_changes_nothing_else() {
        // The no-op guarantee, stated as set arithmetic: turning the flag on is PURELY additive,
        // and turning it off leaves the pre-sharding subscription list untouched, in order.
        let single = coordinator_queries(false);
        let sharded = coordinator_queries(true);
        assert_eq!(
            sharded.len(),
            single.len() + MULTI_DB_TABLES.len(),
            "the flag must add the multi-database tables and nothing else"
        );
        assert_eq!(
            &sharded[..single.len()],
            &single[..],
            "the base list must not be reordered"
        );
        for table in MULTI_DB_TABLES {
            assert!(
                sharded.contains(table),
                "{table} must be subscribed when sharded"
            );
        }
        // No duplicates in either shape (a duplicate query is a wasted subscription, and a sign
        // someone re-added a table already in the base list).
        let mut sorted = sharded.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), before, "duplicate subscription query");
    }
}

#[cfg(test)]
mod recv_reducer_tests {
    use super::recv_reducer;

    #[test]
    fn a_completed_reducer_maps_to_ok() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(())).unwrap();
        assert!(recv_reducer(rx, "frobnicate").is_ok());
    }

    #[test]
    fn a_failed_reducer_is_prefixed_with_the_call_label() {
        // The `what` label prefixes the module's raw Err string so a log line attributes the
        // failure to the reducer that ran ("<what> reducer failed: <module error>"). The timeout
        // arm is not driven here (it would block the suite for its full 10s).
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err("not enough copper".to_string())).unwrap();
        let err = recv_reducer(rx, "buy_item").expect_err("a reducer Err must map to Err");
        assert_eq!(
            err.to_string(),
            "buy_item reducer failed: not enough copper"
        );
    }
}

/// Fire a reducer over `$reducers` and block (≤10s) on its completion, mapping the outcome to
/// `anyhow` (evaluates to `Result<()>`). Collapses the channel + status-flatten callback +
/// `recv_reducer` that every reducer wrapper otherwise repeats verbatim; the trailing completion
/// callback is appended after the reducer's own positional args. `$what` (a literal) labels the
/// send-error and timeout messages. The double-`Result` flattening (`InternalError` → `{e:?}`)
/// lives here, so it is one edit instead of nine.
macro_rules! call_reducer {
    ($reducers:expr, $what:literal, $method:ident ( $($arg:expr),* $(,)? )) => {{
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        $reducers
            .$method($($arg,)* move |_ctx, status| {
                let _ = tx.send(match status {
                    Ok(inner) => inner,
                    Err(e) => Err(format!("{e:?}")),
                });
            })
            .map_err(|e| anyhow!(concat!("send ", $what, ": {}"), e))?;
        recv_reducer(rx, $what)
    }};
}
pub(crate) use call_reducer;

/// Fire a reducer and **do not wait** for its completion (perf catalog 1.13, issue #110).
///
/// `call_reducer!` blocks the calling thread on the completion channel. For movement that is a hard
/// throughput ceiling: `forward_movement` runs on the session's own socket-reader thread, so a
/// player's NEXT packet cannot be read until the previous movement has made a full gateway→DB→gateway
/// round-trip. Measured 2026-07-28: **~200 committed `movement_update`/s across the whole server**,
/// unchanged at 100/150/200 players, i.e. a ~996 ms round-trip of which the database accounted for
/// 2.1 ms — the rest was thread wakeup, with 642 threads over 28 cores.
///
/// The outcome is delivered to `$on_done` instead, which runs on the SDK's callback thread. Use this
/// ONLY where no reply packet depends on the result; anything that gates a response must keep the
/// blocking shape.
macro_rules! call_reducer_nowait {
    ($reducers:expr, $what:literal, $method:ident ( $($arg:expr),* $(,)? ), $on_done:expr) => {{
        let done = $on_done;
        $reducers
            .$method($($arg,)* move |_ctx, status| {
                done(match status {
                    Ok(inner) => inner,
                    Err(e) => Err(format!("{e:?}")),
                });
            })
            .map_err(|e| anyhow!(concat!("send ", $what, ": {}"), e))
    }};
}
pub(crate) use call_reducer_nowait;

impl Coordinator {
    /// Get (or lazily open) the per-account player connection. Opening builds a new SDK
    /// connection on a dedicated OS thread; safe to call from the `spawn_blocking` logon/world
    /// handlers (no ambient tokio runtime there). Cached for the gateway's lifetime so the same
    /// node-issued identity is reused across the logon and world phases.
    ///
    /// Shard-scoped (#17): the connection is opened against THIS handle's database and cached in
    /// THIS shard's `players` map, so a session holding its home-shard handle can only ever open a
    /// player connection — and therefore only ever call reducers and subscribe — on its home shard.
    /// Live player-session count on THIS shard (#78) — the size of the very cache `player_conn`
    /// below maintains. Approximate: an account with a cached-but-dead connection (see the
    /// liveness check just below) still counts until its NEXT checkout evicts it, and an account
    /// that never called any reducer that opens a player connection is not counted at all even if
    /// its client socket is open (logon-phase connections only need the owner connection). Good
    /// enough for an ops gauge; see `docs/region-sharding.md`'s staleness note.
    pub(crate) fn session_count(&self) -> usize {
        self.0.players.lock().unwrap().len()
    }

    pub(crate) fn player_conn(&self, account_id: u64) -> Result<Arc<PlayerConn>> {
        if let Some(p) = self.0.players.lock().unwrap().get(&account_id).cloned() {
            // Liveness check on checkout (278): a module republish closes every websocket
            // ("module exited") — the coordinator self-heals, but a cached player conn died
            // silently and its reducer calls go NOWHERE (player_login "timed out after 10s"
            // until a gateway restart). A dead conn is evicted and rebuilt below exactly like
            // a first login; the next logon flow re-binds the fresh identity via
            // `establish_session` as it always does.
            if p.conn.is_active() {
                return Ok(p);
            }
            log::warn!(
                "player conn for account {account_id} is dead (module restart?) — rebuilding"
            );
            self.0.players.lock().unwrap().remove(&account_id);
        }
        let uri = self.0.uri.clone();
        let db_name = self.0.db_name.clone();
        // 292: build on a RUNTIME WORKER, not a bare `std::thread`, so the SDK's
        // `enter_or_create_runtime()` takes its `Handle::try_current() == Ok` branch and REUSES this
        // runtime instead of creating a private 1-worker runtime per connection. That private
        // runtime was ~1-2 OS threads per distinct account, never released for the gateway's
        // lifetime (work-item 292), and at 500 accounts it is what left the gateway unable to serve
        // logons after #285's mass-disconnect run.
        //
        // Measured with a probe before writing this: three connections built on a worker added
        // ZERO `spacetimedb-background-connection` threads. The note on `connect_blocking` above
        // says a tokio worker panics here; that half is WRONG — `block_in_place` exists precisely
        // for multi-thread workers, and only panics on a current-thread runtime or a blocking-pool
        // thread. The `spawn_blocking` half of that note is correct, which is why this hands the
        // build to a spawned task rather than doing it inline (this fn is called FROM
        // `spawn_blocking` handlers).
        //
        // Nothing about the security model moves: the connection is still per-account with its own
        // minted identity, so `ctx.sender` authorisation and the module's RLS filters are untouched.
        let conn = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let (tx, rx) = std::sync::mpsc::channel();
                handle.spawn(async move {
                    let built =
                        tokio::task::block_in_place(|| connect_player_blocking(uri, db_name));
                    let _ = tx.send(built);
                });
                rx.recv_timeout(Duration::from_secs(20))
                    .map_err(|_| anyhow!("player connect task did not answer within 20s"))??
            }
            // No ambient runtime (unit tests, and any future non-tokio caller): fall back to the
            // original dedicated-thread build. Correct, just one private runtime per connection.
            Err(_) => std::thread::Builder::new()
                .name(format!("stdb-player-{account_id}"))
                .spawn(move || connect_player_blocking(uri, db_name))
                .context("spawn player connect thread")?
                .join()
                .map_err(|_| anyhow!("player connect thread panicked"))??,
        };
        let arc = Arc::new(conn);
        self.0
            .players
            .lock()
            .unwrap()
            .insert(account_id, arc.clone());
        Ok(arc)
    }

    /// Release this account's cached connection at session teardown (292).
    ///
    /// Only safe behind the `release_session` gate: a stale socket SHARES this connection with the
    /// session that superseded it (#42), so releasing from a stale teardown would cut a live
    /// player's link. Same arbitration as the entity delete.
    ///
    /// Two earlier attempts at this reclaimed NOTHING, because each connection owned a private tokio
    /// runtime that outlived it. With the build moved onto a shared runtime worker (see
    /// `player_conn`) the only remaining per-connection thread is the pump, and joining it lets the
    /// connection actually drop.
    ///
    /// ⚠ Unreachable today — nothing calls the `WorldStore::release_player_conn` this backs. See
    /// that method's note in `world/mod.rs` for why the call site is a follow-up, not a lint fix.
    #[allow(dead_code)]
    pub(crate) fn release_player_conn(&self, account_id: u64) {
        let Some(p) = self.0.players.lock().unwrap().remove(&account_id) else {
            return;
        };
        if let Err(e) = p.conn.disconnect() {
            log::debug!("292: account {account_id} conn was already inactive ({e})");
        }
        // Reap OFF this thread: the pump must be joined for its caches to be freed, but a teardown
        // must never hang on a pump that refuses to exit.
        let handle = p.pump.lock().unwrap().take();
        if let Some(h) = handle {
            let _ = std::thread::Builder::new()
                .name(format!("stdb-reap-{account_id}"))
                .spawn(move || {
                    let _ = h.join();
                });
        }
    }
}

impl Coordinator {
    /// Connect ONE database and wrap it in a watchdogged `CoordinatorInner`. `sharded_tables` — see
    /// [`connect_blocking`]; a single-database gateway passes `false` and subscribes exactly the
    /// tables it subscribed before #20.
    async fn connect_shard(
        cfg: &GatewayConfig,
        db_name: &str,
        sharded_tables: bool,
    ) -> Result<Arc<CoordinatorInner>> {
        let uri = cfg.stdb_uri.clone();
        let db = db_name.to_string();
        let token = cfg.coordinator_token.clone();
        // Created BEFORE the connection so `connect_blocking` has a handle to register its
        // `game_map_region` invalidation callbacks against; stored on `CoordinatorInner` below so a
        // later watchdog reconnect hands the rebuilt connection the SAME handle (see that field's doc).
        let map_regions_cache = Arc::new(Mutex::new(None));
        let cache_for_connect = map_regions_cache.clone();
        // Build on a dedicated OS thread (no tokio context), join it off the reactor.
        let build_thread = std::thread::Builder::new()
            .name("stdb-coordinator-connect".into())
            .spawn(move || connect_blocking(uri, db, token, sharded_tables, cache_for_connect))
            .context("spawn coordinator connect thread")?;
        let live = tokio::task::spawn_blocking(move || -> Result<LiveConn> {
            build_thread
                .join()
                .map_err(|_| anyhow!("coordinator connect thread panicked"))?
        })
        .await
        .context("join coordinator connect task")??;
        let inner = Arc::new(CoordinatorInner {
            live: RwLock::new(live),
            token: cfg.coordinator_token.clone(),
            uri: cfg.stdb_uri.clone(),
            db_name: db_name.to_string(),
            sharded_tables,
            players: Mutex::new(HashMap::new()),
            on_reconnect: Mutex::new(None),
            map_regions_cache,
        });
        spawn_coordinator_watchdog(inner.clone());
        Ok(inner)
    }

    /// Connect the coordinator set: one privileged connection per database in the shard map (#17),
    /// and return the handle for the DEFAULT (realm/home) database — the one the logon + world
    /// listeners are handed, exactly as before. With no `LYRACORE_SHARD_MAP` this is one connection and
    /// nothing else changes.
    ///
    /// An EXTRA shard that fails to connect is logged and left out of the set rather than failing
    /// the gateway: routing to a missing shard degrades to the default database, i.e. to today's
    /// behavior. The default database's own failure is still fatal (it always was).
    pub async fn connect(cfg: &GatewayConfig) -> Result<Self> {
        if cfg.coordinator_token.is_none() {
            log::warn!(
                "LYRACORE_COORDINATOR_TOKEN is unset; the coordinator will connect anonymously and \
                 cannot read the private game_account/game_session tables"
            );
        }
        let map = ShardMap::from_env(&cfg.module_name);
        // The character→shard index (#20) and the region tables (#23) only exist to answer "which
        // of several databases owns this". One database ⇒ nothing to answer, nothing subscribes
        // them, and the subscription set is exactly the pre-#17/#20/#23 one — which is what makes
        // "the env vars unset ⇒ today's gateway" true of the wire, not just of the routing logic.
        let sharded_tables = map.databases().len() > 1;
        let mut conns: HashMap<String, Arc<CoordinatorInner>> = HashMap::new();
        for db in map.databases() {
            match Self::connect_shard(cfg, &db, sharded_tables).await {
                Ok(inner) => {
                    log::info!("coordinator connected to shard {db}");
                    conns.insert(db, inner);
                }
                // The default database is listed FIRST, so its failure aborts before any other.
                Err(e) if db == map.default_db() => return Err(e),
                // Realm-core down is NOT a routing degradation — it is an auth outage. Nobody can
                // log in until it is back (`realm_core()` fails closed), but the world sessions
                // already in progress keep playing, so this is a warning, not a fatal.
                Err(e) if Some(db.as_str()) == map.realm_core_db() => log::error!(
                    "realm-core database {db} unreachable ({e:#}) — LOGONS WILL BE REFUSED until it \
                     is reachable (the gateway will NOT fall back to the world DB's auth cache); \
                     the coordinator watchdog is not running for it, so restart the gateway once \
                     realm-core is up"
                ),
                Err(e) => log::error!(
                    "shard {db} unreachable ({e:#}) — routing to it will fall back to the default \
                     database {}",
                    map.default_db()
                ),
            }
        }
        if let Some(rc) = map.realm_core_db() {
            log::info!(
                "realm-core active: accounts, sessions, and the character→shard index live on {rc}"
            );
        }
        let home = conns
            .get(map.default_db())
            .cloned()
            .ok_or_else(|| anyhow!("default shard {} missing from the set", map.default_db()))?;
        if conns.len() > 1 {
            log::info!(
                "shard map active: {} databases {:?}",
                conns.len(),
                map.databases()
            );
        }
        // Issue #48: the ROUTING is ours and `game_config.hosts_instances` is each database's, and
        // this is the only place both are known. Reads the flag out of each coordinator's just-
        // applied subscription cache (`connect_blocking` blocks until it is applied, so the row is
        // there); a database with no row reads `None` ⇒ "hosts", the module's own default. Silent
        // when correctly configured — including the ordinary single-database realm — a WARNING when
        // the world shard spawns populations it will not run (#39), and FATAL when nothing anywhere
        // will spawn them (#48: every dungeon created with 0 entities).
        let hosts_instances = |db: &str| -> Option<bool> {
            conns
                .get(db)
                .and_then(|inner| inner.coord().conn.db.game_config().id().find(&0))
                .map(|cfg| cfg.hosts_instances)
        };
        map.check_instance_hosting(hosts_instances, |d| conns.contains_key(d))
            .enforce()
            .map_err(|msg| anyhow!("{msg}"))?;
        ensure_guid_ranges(&conns, &map);
        let coordinator = Self(
            home,
            Arc::new(ShardSet {
                map,
                conns,
                sessions: SessionEpochs::default(),
            }),
        );
        // Issue #50: promotes each world shard's staging loot rolls onto realm-core and settles
        // resolved winners back down. A no-op loop on an unsharded gateway (`relay_tick` returns
        // immediately when `realm_store()` is `None`), so this costs a single-database deployment
        // nothing but one idle thread.
        spawn_loot_roll_relay(coordinator.clone());
        // Issue #78: per-shard writer occupancy + session counts + per-region population, sampled
        // on a timer and recorded onto realm-core so an operator can answer "which shard is hot,
        // which region is crowded, should I activate a seam" with `spacetime sql` alone. See
        // `docs/region-sharding.md`.
        spawn_load_sampler(coordinator.clone(), cfg.stdb_uri.clone());
        Ok(coordinator)
    }

    /// The database name this handle targets — the routing identity of every call made through it.
    pub fn shard_name(&self) -> &str {
        &self.0.db_name
    }

    /// Is the gateway actually running against more than one database? `false` is the unconfigured
    /// default and lets the routing paths short-circuit to exactly the pre-sharding behavior — no
    /// location read, no handle clone, no pin.
    pub(crate) fn is_sharded(&self) -> bool {
        self.1.conns.len() > 1
    }

    /// The handle for the shard that owns `(map_id, instance_id)`, or `None` when that is already
    /// THIS handle's shard (the single-shard default: nothing to swap).
    ///
    /// An owner that is named by the map but failed to connect degrades to the DEFAULT database —
    /// never to whatever handle happened to ask. Degrading to the asker looks the same from the
    /// default handle but is wrong from any other: a session already pinned to shard B that ports
    /// to a map owned by an unreachable shard C would KEEP its B pin and be served by a database
    /// that owns neither its old location nor its new one. Collapsing to the default is the
    /// documented safety property — a bad shard map can only ever fall back to today's behavior.
    pub fn shard_for(&self, map_id: u32, instance_id: u64) -> Option<Coordinator> {
        let db = self
            .1
            .map
            .resolve_connected(map_id, instance_id, |d| self.1.conns.contains_key(d));
        // Always present: `resolve_connected` only ever answers a connected database or the
        // default, and `connect` proves the default is in the set.
        self.shard_handle(db)
    }

    /// [`shard_for`](Self::shard_for) for an INSTANCE, given the database that currently holds the
    /// character (#21). Same contract — `None` means "already this handle's shard" — with the one
    /// extra rule that keeps a live dungeon run on its pool member when the operator re-points that
    /// bucket to a newly added instances database. The decision is the pure, unit-tested
    /// [`crate::config::ShardMap::instance_owner`]; this only supplies it with the connected set.
    pub(crate) fn instance_shard_for(
        &self,
        map_id: u32,
        instance_id: u64,
        holder: &str,
    ) -> Option<Coordinator> {
        let db = self.1.map.instance_owner(map_id, instance_id, holder, |d| {
            self.1.conns.contains_key(d)
        });
        self.shard_handle(db)
    }

    /// The handle for `db`, or `None` when `db` is already THIS handle's shard ("nothing to swap",
    /// the answer every routing path gives when the session is where it belongs). One place, so a
    /// second routing overlay cannot grow a second opinion about what "stay put" means.
    ///
    /// `None` therefore means one of TWO different things, and a caller that treats it as a single
    /// failure signal cannot tell them apart (a live #73 bug that read exactly this way: a region
    /// explicitly assigned to the ASKER's own shard produced a "not connected" warning for a database
    /// that was trivially connected — see `Coordinator::split_box_by_shard`'s doc comment for the fix,
    /// which is to never call this with `db == self.shard_name()` in the first place, not to guess
    /// here after the fact). The warning below only fires for the SECOND case — `db` genuinely absent
    /// from the connected set — so it stays silent on every "already home" call, which is the common
    /// case for `shard_for`/`instance_shard_for`/`settle_home_shard`.
    pub(crate) fn shard_handle(&self, db: &str) -> Option<Coordinator> {
        if db == self.shard_name() {
            return None;
        }
        let inner = self.1.conns.get(db);
        if inner.is_none() {
            log::warn!(
                "shard_handle: {db} is not in the connected set (asked by {}); known shards: {:?}",
                self.shard_name(),
                self.1.conns.keys().collect::<Vec<_>>()
            );
        }
        Some(Coordinator(inner?.clone(), self.1.clone()))
    }

    /// Every connected shard's handle, DEFAULT FIRST (#19). Two callers: the character-select list
    /// (a character parked inside an instance lives on the instance shard, and the player must
    /// still see it) and the party mirror fan-out (`world_stores`).
    ///
    /// #20 note: this is now exactly `world_shards()` with the names dropped — REALM-CORE IS NOT IN
    /// IT. #19 wrote this over `conns`, which was the same set only while the default database was
    /// also the auth database; once realm-core is a third connection, probing it for characters
    /// asks a database that owns none and (worse) could answer with the shadow account rows the
    /// transfer's `import_character_blob` writes there. `ShardMap::shards()` excludes it by
    /// construction, so the probe set is the right one for both topologies.
    ///
    /// Deliberate simplification: O(shards) per login for a caller that needs every handle. The
    /// holder LOOKUP no longer pays this unconditionally — `realm_core::locate_home_shard` (#47)
    /// tries the realm-core `character_shard` hint first and only falls back to scanning
    /// `world_shards()` (this method's underlying set) on a miss.
    pub(crate) fn all_shards(&self) -> Vec<Coordinator> {
        self.world_shards()
            .into_iter()
            .map(|(_, coord)| coord)
            .collect()
    }

    /// The DATABASE that owns the REGION a character is standing in (#23), or `None` when regions
    /// have nothing to say — which is every case except "a region containing this point is assigned
    /// to a connected world shard".
    ///
    /// A NAME rather than a handle, because the answer has to be compared with the shard map's on
    /// equal terms: an assignment that names the shard the asking handle is already on means *stay
    /// here*, not *ask the shard map instead* — otherwise the same assignment would route two ways
    /// depending on which handle the session happened to hold when it entered.
    ///
    /// The three inputs come from three places, and each is deliberate:
    /// - the **definitions** from the DEFAULT world shard, because they are content data baked by
    ///   the world ETL (realm-core never runs it, so its copy is empty);
    /// - the **assignment** from realm-core, because that is the spec's authority for it;
    /// - the **position** from `home_db` — the shard `resolve_home_shard` just settled on — because
    ///   during a transfer window two shards can hold a row for one guid, and the *stale* one's
    ///   coordinates must not be what a routing decision is taken on. The walk is only the fallback.
    ///
    /// Falls through to `None` on every failure — a missing realm-core, an empty menu, an
    /// unassigned region, a target that is not a connected world shard — so the caller's existing
    /// `shard_for` answer stands. That is the ticket's whole safety property: routing can only ever
    /// collapse to #17's.
    pub(crate) fn region_db_for(
        &self,
        character_guid: u64,
        home_db: &str,
        map_id: u32,
        instance_id: u64,
    ) -> Option<String> {
        let shards = self.world_shards();
        let (_, default_shard) = shards.first()?;
        let regions = default_shard.map_regions();
        if regions.is_empty() {
            return None; // no seam menu imported: the overwhelmingly common case, and free
        }
        let assignments = self.realm_core().ok()?.region_assignments();
        let (x, y, pending_instance) = shards
            .iter()
            .find(|(name, _)| name == home_db)
            .into_iter()
            .chain(shards.iter())
            .find_map(|(_, coord)| coord.character_position(character_guid))?;
        let db = crate::config::resolve_region_shard(
            &regions,
            &assignments,
            map_id,
            // A character mid-instance-entry has no live entity, so the partition read answers 0
            // while the destination sits in `pending_instance_id`. Either one being non-zero means
            // "not the open world", which is the only thing regions partition.
            if instance_id == 0 {
                pending_instance
            } else {
                instance_id
            },
            x,
            y,
            // WORLD SHARDS only, not every connected database: `conns` also holds realm-core, which
            // owns no characters, so an assignment naming it would route a login into a database
            // with nothing of the player's in it. The shard map can never name it either.
            |d| shards.iter().any(|(name, _)| name == d),
        )?;
        log::info!(
            "character {character_guid} at ({x:.1}, {y:.1}) on map {map_id} is in a region assigned \
             to shard {db}"
        );
        Some(db.to_string())
    }

    /// The movement-path twin of [`region_db_for`](Self::region_db_for) (#72 slice 1 — detection
    /// only, `world::seam`). The caller is driving off a JUST-ARRIVED movement packet, so it already
    /// has `(x, y)` — this skips the position half of a `character_position` read and keeps only the
    /// INSTANCE half of it: a resident mid-dungeon must never be region-routed by an open-world
    /// map's seam menu (`resolve_region_shard` refuses on `instance_id != 0`, but only if it is
    /// handed the real one). Same short-circuit order as `region_db_for` — the menu before the
    /// assignment/instance reads — so a realm with no seam menu imported costs nothing here either.
    ///
    /// `None` on every failure `region_db_for` also falls through on, plus one it can't reach: the
    /// character has no LIVE entity on any connected shard (a moving player always has one, but a
    /// mock or a race is not this function's problem to adjudicate — `None` is always safe, it just
    /// means "the shard map's answer stands").
    pub(crate) fn region_shard_for_point(
        &self,
        character_guid: u64,
        home_db: &str,
        map_id: u32,
        x: f32,
        y: f32,
    ) -> Option<String> {
        let shards = self.world_shards();
        let (_, default_shard) = shards.first()?;
        let regions = default_shard.map_regions();
        if regions.is_empty() {
            return None; // no seam menu imported: the overwhelmingly common case, and free
        }
        let (_, _, instance_id) = shards
            .iter()
            .find(|(name, _)| name == home_db)
            .into_iter()
            .chain(shards.iter())
            .find_map(|(_, coord)| coord.character_position(character_guid))?;
        let assignments = self.realm_core().ok()?.region_assignments();
        let db = crate::config::resolve_region_shard(
            &regions,
            &assignments,
            map_id,
            instance_id,
            x,
            y,
            |d| shards.iter().any(|(name, _)| name == d),
        )?;
        Some(db.to_string())
    }

    /// The AOI tracker's twin of [`region_shard_for_point`](Self::region_shard_for_point) (#73
    /// view-merge rebuild): split a WHOLE box's cells into per-shard [`lyracore_shared::spatial::GridRect`]s
    /// in one pass, instead of resolving each cell into a materialized map and then diffing that
    /// separately — [`crate::stdb::aoi::split_box_by_shard`]'s own doc comment is where the boundary-
    /// line scan lives; this method only supplies the resolver it calls per cell, using the exact
    /// same short-circuits [`region_shard_for_point`](Self::region_shard_for_point) does (no seam menu
    /// imported, realm-core unreachable) so a straddling box and a straddling point can never
    /// disagree about "is view-merge even relevant here".
    ///
    /// `instance_id` is the CALLER's, read from its own connection's cache (the AOI tracker's own
    /// entity row) — never a probe, because unlike `region_db_for` there is no `character_guid` to
    /// look up: the tracker is driven by a movement `(x, y)`, not an identity.
    ///
    /// `None` in the result (never a missing shard) means HOME. Every degenerate case collapses to
    /// one entry: `(None, vec![GridRect::from_box(box_)])` — the whole box, as a rect — which is why
    /// a non-straddling box's home coverage is byte-identical to the old whole-box subscription.
    ///
    /// # A region explicitly assigned to the CALLER's OWN shard is home, not away
    ///
    /// (Bug found live, 2026-08-04, `test-view-merge.sh` gate.) `resolve_region_shard_for_cell`
    /// answers purely "which database does this region's assignment name", with no notion of who is
    /// asking — the same contract `resolve_region_shard`/`region_shard_for_point` already have, and
    /// their callers stay correct because they hand the name straight to
    /// [`shard_handle`](Self::shard_handle), whose OWN `db == self.shard_name()` guard folds "my own
    /// shard" back to "stay" before anything acts on it. This method builds `(shard, rects)` BUCKETS
    /// directly from the resolver's raw answer, before any `shard_handle` call ever runs — so without
    /// the `.filter` below, a region explicitly assigned to the shard the viewer is ALREADY on (a
    /// perfectly ordinary menu — draw one region per shard for symmetry, e.g. "region 1 = the rest of
    /// Elwynn, pinned to lyracore" right next to "region 2 = Goldshire, pinned to
    /// lyracore-world-2") would land its own home coverage in an AWAY bucket named after itself.
    /// `AreaOfInterestTracker::ensure_away` then asks `shard_handle` for that name, gets back the
    /// SAME "None means stay" signal every other caller correctly reads as success, misreads it as
    /// "shard unreachable", and — far worse than a missed away leg — the cells never end up in the
    /// HOME bucket either: `update()`'s `new_home` only ever picks up `None` entries, so a box that
    /// resolves ENTIRELY to "my own shard, by name" subscribes NOTHING at all, home or away. Filtering
    /// the resolver's answer back to `None` here, once, is cheaper and more local than teaching every
    /// consumer of this method's output to recognize its own name.
    pub(crate) fn split_box_by_shard(
        &self,
        box_: &lyracore_shared::spatial::GridBox,
        instance_id: u64,
    ) -> Vec<(Option<String>, Vec<lyracore_shared::spatial::GridRect>)> {
        let home_only = || {
            vec![(
                None,
                vec![lyracore_shared::spatial::GridRect::from_box(box_)],
            )]
        };
        let shards = self.world_shards();
        let Some((_, default_shard)) = shards.first() else {
            return home_only();
        };
        let regions = default_shard.map_regions();
        if regions.is_empty() {
            return home_only(); // no seam menu imported: the overwhelmingly common case, and free
        }
        let Ok(assignments) = self.realm_core().map(|rc| rc.region_assignments()) else {
            return home_only(); // realm-core unreachable: degrade to home, same as every other resolver
        };
        let my_shard = self.shard_name().to_string();
        super::aoi::split_box_by_shard(box_, move |gx, gy| {
            let resolved = crate::config::resolve_region_shard_for_cell(
                &regions,
                &assignments,
                box_.map_id,
                instance_id,
                gx,
                gy,
                |d| shards.iter().any(|(name, _)| name == d),
            )
            .map(str::to_string);
            crate::config::fold_home_shard(resolved, &my_shard)
        })
    }

    // #20 removed `realm_shard()` (#17's "the default shard is where accounts and characters live").
    // Both of its jobs now have a more precise owner: accounts and sessions belong to `realm_core()`
    // below, and "which shard holds this character" is answered by probing (`world_shards()`) rather
    // than by assuming the default — the assumption that stops being true the first time a transfer
    // actually moves someone. It had no other callers.

    /// The **realm-core** handle (#20): the database that owns accounts, sessions, and the
    /// character→shard index. Unconfigured (`LYRACORE_REALM_CORE` unset, blank, or naming the world
    /// database) → the default shard, i.e. auth reads and writes land exactly where they land
    /// today. Configured but not connected → `Err`, and every auth path must propagate that rather
    /// than fall back to the world DB's stale auth cache (see `ShardMap::auth_db`).
    ///
    /// "Connected" is checked LIVE, not "was in the set at boot". `conns` is built once at startup
    /// and never shrinks, so membership alone would mean a realm-core that connected and then died
    /// still answers auth — out of the SDK's last-known local cache, which is exactly the stale
    /// salt/verifier/`banned` snapshot fail-closed exists to refuse. The watchdog reconnects it
    /// within a poll, and until it does, logons are refused rather than served from that snapshot.
    /// The unconfigured path never evaluates this predicate at all (`auth_db` short-circuits), so
    /// it costs a single-database gateway nothing.
    pub(crate) fn realm_core(&self) -> Result<Coordinator> {
        let db = self
            .1
            .map
            .auth_db(|d| {
                self.1.conns.get(d).is_some_and(|inner| {
                    let live = inner.coord();
                    live.conn.is_active() && live._sub.is_active()
                })
            })
            .map_err(|db| {
                anyhow!(
                    "realm-core database {db} is not connected — refusing to authenticate against \
                     the world database's stale auth cache"
                )
            })?;
        let inner = self
            .1
            .conns
            .get(db)
            .ok_or_else(|| anyhow!("auth database {db} missing from the coordinator set"))?;
        Ok(Coordinator(inner.clone(), self.1.clone()))
    }

    /// The connected WORLD SHARDS, default first — the probe order for the character→shard fallback
    /// (#20 AC#3). Realm-core is excluded by `ShardMap::shards`: it holds no characters.
    pub(crate) fn world_shards(&self) -> Vec<(String, Coordinator)> {
        self.1
            .map
            .shards()
            .into_iter()
            .filter_map(|db| {
                let inner = self.1.conns.get(&db)?.clone();
                Some((db, Coordinator(inner, self.1.clone())))
            })
            .collect()
    }

    /// The routing table this handle resolves through (read-only; the pure resolvers in
    /// `crate::config` take it by reference).
    pub(crate) fn shard_map(&self) -> &ShardMap {
        &self.1.map
    }

    /// Claim a fresh in-world session epoch for `account_id` (at player_login). See `SessionEpochs`.
    pub fn claim_session(&self, account_id: u64) -> u64 {
        self.1.sessions.claim(account_id)
    }

    /// Release a session epoch at teardown; true iff still current (caller owns the entity → logout).
    pub fn release_session(&self, account_id: u64, epoch: u64) -> bool {
        self.1.sessions.release(account_id, epoch)
    }
}

#[cfg(test)]
mod runtime_share {
    /// TEMPORARY probe (292): can a player `DbConnection` be built on a multi-thread runtime WORKER?
    /// If yes, `enter_or_create_runtime()` reuses the ambient runtime instead of spawning a private
    /// one per connection — removing the ~1-2 leaked threads per account with ctx.sender and RLS
    /// completely untouched. The comment at the top of `connect_blocking` claims a tokio worker
    /// panics here; `block_in_place` is specifically FOR multi-thread workers, so that half is
    /// worth testing rather than trusting.
    fn bg_threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .map(|d| {
                d.filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("comm")).ok())
                    .filter(|c| c.starts_with("spacetimedb-bac"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// `#[ignore]`d: needs a live node on 127.0.0.1:3000. Run it with
    /// `cargo test --bin lyracore-gateway runtime_share -- --ignored --nocapture` after any change to how
    /// player connections are built, and expect a delta of ZERO. A non-zero delta means the build
    /// left the runtime context again and every account is paying for a private runtime.
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn build_on_a_worker_reuses_the_ambient_runtime() {
        let before = bg_threads();
        println!("PROBE before: spacetimedb-bac = {before}");
        let mut held = Vec::new();
        for i in 0..3 {
            let r = tokio::task::block_in_place(|| {
                super::connect_player_blocking(
                    "http://127.0.0.1:3000".to_string(),
                    "lyracore".to_string(),
                )
            });
            match r {
                Ok(c) => {
                    println!("PROBE conn {i}: OK");
                    held.push(c);
                }
                Err(e) => {
                    println!("PROBE conn {i}: FAILED: {e:#}");
                    return;
                }
            }
        }
        let after = bg_threads();
        println!(
            "PROBE after 3: spacetimedb-bac = {after} (delta {})",
            after as i64 - before as i64
        );
        println!(
            "PROBE VERDICT: {}",
            if after <= before {
                "SHARED RUNTIME - no private runtime per connection"
            } else {
                "still one private runtime per connection"
            }
        );
    }

    fn rss_mb() -> f64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
            })
            .map(|kb| kb / 1024.0)
            .unwrap_or(0.0)
    }

    /// 292 memory half: what does a player connection cost with NO subscriptions at all? That
    /// isolates the SDK/connection FLOOR from the subscription row cache, and the two have opposite
    /// fixes — a large floor means fewer connections or an upstream change; a small floor means the
    /// remaining ~26 per-player queries are the target.
    ///
    /// `#[ignore]`d: needs a live node. Run with
    /// `cargo test --bin lyracore-gateway connection_floor -- --ignored --nocapture`.
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connection_floor_without_subscriptions() {
        const N: usize = 40;
        let before = rss_mb();
        println!("FLOOR rss before: {before:.0} MB");
        let mut held = Vec::new();
        for i in 0..N {
            match tokio::task::block_in_place(|| {
                super::connect_player_blocking(
                    "http://127.0.0.1:3000".to_string(),
                    "lyracore".to_string(),
                )
            }) {
                Ok(c) => held.push(c),
                Err(e) => {
                    println!("FLOOR conn {i} failed: {e:#}");
                    return;
                }
            }
        }
        let after = rss_mb();
        println!(
            "FLOOR rss after {N} unsubscribed conns: {after:.0} MB — delta {:.0} MB = {:.2} MB/conn",
            after - before,
            (after - before) / N as f64
        );
        println!(
            "FLOOR threads: {}",
            std::fs::read_dir("/proc/self/task").unwrap().count()
        );
    }

    /// 292 memory half, ATTRIBUTION: cost per subscribed query, measured one query at a time over N
    /// connections. The floor probe above says an unsubscribed connection is ~1MB, so whatever this
    /// finds IS the per-session memory problem. Ordered worst-first in the output so the next
    /// pruning target is obvious.
    ///
    /// `#[ignore]`d: needs a live node. Run with
    /// `cargo test --bin lyracore-gateway subscription_cost -- --ignored --nocapture`.
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn subscription_cost_per_query() {
        use spacetimedb_sdk::DbContext as _;
        const N: usize = 12;
        // The globals still on the per-player list, plus the AOI entity box for comparison.
        // ⚠ Built with `format!`, never as whole literals — and do not spell the select-star-from
        // phrase in a comment here either. `gateway/tests/schema_parity.rs`'s completeness guard
        // scans THIS FILE for that exact phrase to discover what the coordinator subscribes, so a
        // literal (or a comment quoting one) reads to it as a real subscription with no parity
        // manifest entry. It failed both ways once while this probe was being written.
        let tables = [
            "game_gameobject_template",
            "game_creature_cast",
            "game_corpse",
            "game_aura",
            "game_melee_attack",
            "game_dynamic_object",
            "game_channel_member",
            "game_item_instance",
            "game_player_skill",
        ];
        let mut queries: Vec<String> = tables
            .iter()
            .map(|t| format!("{} * FROM {t}", "SELECT"))
            .collect();
        queries.push(format!(
            "{} * FROM game_world_entity WHERE grid_x >= 0 AND grid_x <= 4 AND grid_y >= 0 AND grid_y <= 4",
            "SELECT"
        ));
        let mut results: Vec<(String, f64)> = Vec::new();
        for q in &queries {
            let before = rss_mb();
            let mut held = Vec::new();
            for _ in 0..N {
                let Ok(c) = tokio::task::block_in_place(|| {
                    super::connect_player_blocking(
                        "http://127.0.0.1:3000".to_string(),
                        "lyracore".to_string(),
                    )
                }) else {
                    println!("COST connect failed, aborting");
                    return;
                };
                let (tx, rx) = std::sync::mpsc::channel();
                let tx2 = tx.clone();
                let h = c
                    .conn
                    .subscription_builder()
                    .on_applied(move |_| {
                        let _ = tx.send(true);
                    })
                    .on_error(move |_, _| {
                        let _ = tx2.send(false);
                    })
                    .subscribe(vec![q.to_string()]);
                let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
                held.push((c, h));
            }
            let per = (rss_mb() - before) / N as f64;
            results.push((q.clone(), per));
            println!("COST {per:>7.2} MB/conn   {q}");
            drop(held);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        println!("\nCOST RANKED (worst first):");
        for (q, mb) in &results {
            println!("COST {mb:>7.2} MB/conn   {q}");
        }
    }
}
