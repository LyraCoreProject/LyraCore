//! Connection tier: the `Coordinator` facade + its inner shared state, the live privileged
//! connection (with the reconnect watchdog), the per-account player connections, the
//! `call_reducer!` macro, and the connection lifecycle constructors. Reads/reducers/subscriptions
//! live in sibling modules as additional `impl Coordinator` blocks.

use crate::config::{GatewayConfig, ShardMap};
use anyhow::{anyhow, Context, Result};
use spacetimedb_sdk::{DbContext, Identity, SubscriptionHandle as _};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use super::account_sessions::{AccountSessions, SessionEpochs};
use super::bindings::*;

/// Shared handle to the privileged coordination connection **of one shard**.
///
/// Field 0 is the shard this handle talks to — every read, every reducer call, and every
/// subscription in the `reads`/`reducers`/`subscriptions` sibling modules goes through it, so
/// "which database does this call hit" is decided entirely by which handle you hold. Field 1 is
/// the shared, shard-independent routing table + connection set: `shard_for` mints the
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
/// epochs (a per-shard `SessionEpochs` would silently break the stale-logout gate).
pub(crate) struct ShardSet {
    map: ShardMap,
    /// db name → that database's coordinator connection. Always contains the default database;
    /// an extra shard that failed to connect is ABSENT (routing then degrades to the default).
    conns: HashMap<String, Arc<CoordinatorInner>>,
    sessions: SessionEpochs,
    /// How many live world SOCKETS each account currently has. Separate from `sessions`
    /// above on purpose — see [`AccountSessions`].
    live_sessions: AccountSessions,
    /// The gateway-wide shared area-of-interest view — the cell index plus the viewer
    /// registry that every shard's coordinator dispatch routes through. Shard-INDEPENDENT for the
    /// same reason `sessions` is: it answers a question about SESSIONS, and guids are globally
    /// unique across databases (realm-core hands each shard a disjoint guid range), so one index
    /// spans the whole realm.
    world: Arc<super::world_view::WorldView>,
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
    /// Whether this connection subscribes the multi-database tables (realm-core's `game_character_shard`)
    /// — kept so the watchdog rebuilds with the SAME subscription set it was created with. See
    /// [`coordinator_queries`].
    sharded_tables: bool,
    /// The CALL-PIPE POOL — extra connections to the same database used exclusively for
    /// reducer calls, round-robined by `call_pipe()`, so seated players' call traffic and login
    /// calls stop serializing on one websocket (measured wall: ~1000 seats/pipe). Each pipe
    /// carries one tiny liveness subscription (`game_config`, 1 row), never the coordinator set.
    /// Empty when `LYRACORE_CALL_PIPES` <= 1 — calls then ride `coord()` exactly as before.
    /// Deliberate simplification: dead pipes don't self-heal — `call_pipe()` skips inactive ones and falls back to
    /// the watchdogged `coord()`; extending the watchdog to rebuild pipes is the upgrade path.
    call_pipes: Vec<RwLock<LiveConn>>,
    call_pipe_next: std::sync::atomic::AtomicUsize,
    /// The per-shard movement batch — the hot path pushes one `GwMove` per inbound
    /// heartbeat and the 40ms flush task sends the whole tick as ONE `gw_movement_batch`
    /// transaction (was: one transaction per heartbeat — ~10k tx/s of per-transaction machinery
    /// at 2000 players, the measured 92%-writer wall).
    pub(crate) motion_batch: Mutex<Vec<super::bindings::GwMove>>,
    /// Per-account player connections (each with its own node-issued identity == the account's
    /// bound identity). Opened on first need (at logon, when `bound_identity` is read) and reused
    /// for the world phase so `player_login`/`movement_update` run as the player, not the owner.
    players: Mutex<HashMap<u64, Arc<PlayerConn>>>,
    /// Re-arm hook for a connection-scoped relay that has no per-login registration point to
    /// self-heal through after a reconnect (the bot-invite relay is the first of these, and
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
    /// A LIST, not a slot: there are now two one-shot coordinator relays per shard — the
    /// bot-invite relay and the shared AOI dispatch — and a slot would silently let whichever
    /// armed second overwrite the first's re-arm.
    pub(crate) on_reconnect: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
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

    /// The connection for the NEXT reducer call: round-robin over the call-pipe pool,
    /// skipping dead pipes; no pool (or every pipe dead) falls back to the watchdogged
    /// coordinator connection, which is exactly the behavior before the pool existed.
    pub(crate) fn call_pipe(&self) -> std::sync::RwLockReadGuard<'_, LiveConn> {
        let n = self.call_pipes.len();
        if n > 0 {
            let start = self
                .call_pipe_next
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            for k in 0..n {
                let g = self.call_pipes[(start + k) % n]
                    .read()
                    .unwrap_or_else(|p| p.into_inner());
                if g.conn.is_active() {
                    return g;
                }
            }
            log::warn!("call-pipe pool: every pipe inactive — falling back to the coordinator connection");
        }
        self.coord()
    }

    /// The per-account player-connection cache, recovering a poisoned lock (matches the file's
    /// discipline — see `coord()`). `player_conn`'s doc above documents two realm-wide outages that
    /// came from a stale critical section poisoning this very lock; that section is narrowed now,
    /// but recovering here is defense-in-depth against any future one — a stale cache beats
    /// every subsequent lookup panicking for every account on the shard.
    fn players(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Arc<PlayerConn>>> {
        self.players.lock().unwrap_or_else(|p| {
            log::error!(
                "players lock poisoned (a prior panic in a critical section) — recovering"
            );
            p.into_inner()
        })
    }
}

/// Build one CALL-ONLY pipe — same connect shape as [`connect_blocking`] but subscribing a
/// single one-row table (`game_config`) purely so the `LiveConn` subscription handle exists and
/// its liveness signal works. Reducer calls need no cache.
fn connect_call_pipe(uri: String, db_name: String, token: Option<String>) -> Result<LiveConn> {
    let conn = DbConnection::builder()
        .with_uri(&uri)
        .with_database_name(&db_name)
        .with_token(token)
        .on_connect(|_ctx, identity, _token| log::info!("call pipe connected as {identity}"))
        .on_connect_error(|_ctx, err| log::error!("call pipe connect error: {err}"))
        .on_disconnect(|_ctx, err| log::warn!("call pipe connection closed: {err:?}"))
        .build()
        .map_err(|e| anyhow!("call pipe build/connect failed: {e}"))?;
    let pump = conn.run_threaded();
    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let tx_err = tx.clone();
    let sub = conn
        .subscription_builder()
        .on_applied(move |_ctx| {
            let _ = tx.send(Ok(()));
        })
        .on_error(move |_ctx, err| {
            let _ = tx_err.send(Err(format!("{err}")));
        })
        .subscribe(vec!["SELECT * FROM game_config"]);
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(anyhow!("call pipe subscription error: {e}")),
        Err(_) => return Err(anyhow!("call pipe subscription not applied within 15s")),
    }
    Ok(LiveConn {
        conn,
        _pump: pump,
        _sub: sub,
    })
}

/// A per-account SpacetimeDB connection. Its node-issued `identity` is bound to the account by
/// `establish_session`, so reducers it calls satisfy the module's `ctx.sender == owner` checks.
pub(crate) struct PlayerConn {
    pub(crate) conn: DbConnection,
    pub(crate) identity: Identity,
    /// The `run_threaded` pump, JOINABLE. The pump holds a CLONE of the `DbConnection`, so the
    /// connection and its caches are freed only once this thread RETURNS — dropping the `PlayerConn`
    /// is not enough, somebody has to reap it. `LiveConn` above already does this on a reconnect.
    ///
    /// Read by `release_player_conn_on`, i.e. at the teardown of the account's last world socket.
    pump: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The coordinator subscription set, as a pure function of ONE flag — so the property that
/// matters can be asserted without a node (see the tests at the bottom of this file).
///
/// `sharded_tables` adds the tables that only mean something on a MULTI-DATABASE deployment:
/// realm-core's `game_character_shard`. It is OFF
/// for a single-database gateway, and that is load-bearing rather than an optimization: **a
/// subscription to a table the deployed module does not have FAILS TO APPLY**, which fails
/// `connect_blocking`, which — for the default database — fails `Coordinator::connect` and the
/// whole gateway. Subscribing them unconditionally would therefore mean a gateway restarted
/// before the module was republished never comes back, in a configuration (`LYRACORE_SHARD_MAP` and
/// `LYRACORE_REALM_CORE` both unset) that multi-shard routing and realm-core both promise costs
/// nothing. Nothing reads
/// any of them on a single-database gateway anyway — `WorldStore::home_shard` short-circuits on
/// `is_sharded()` before it looks at the index.
fn coordinator_queries(sharded_tables: bool) -> Vec<&'static str> {
    let mut queries = vec![
        "SELECT * FROM game_realm",
        "SELECT * FROM game_account",
        "SELECT * FROM game_session",
        "SELECT * FROM game_character",
        "SELECT * FROM game_world_entity",
        // The four box-scoped tables now ride THIS one global subscription per shard instead
        // of ~600 per-player 5×5-box subscriptions, and the gateway's own cell index
        // (`stdb::world_index`) decides who sees each row. `game_world_entity` and
        // `game_gameobject` were already here (the coordinator has always read them RLS-bypassed);
        // these two are the whole server-side delta the change needed.
        //
        // The point is NOT that these queries are cheaper to evaluate — a `SELECT *` is the
        // cheapest possible query, but that was never the cost. The cost was that ~600 DISTINCT
        // query strings can neither be shared nor pruned, so every committed transaction woke ~600
        // SDK pumps to re-evaluate them: load average 11–14 on 8 cores at 34% CPU with 7 runnable
        // threads (clockworklabs/SpacetimeDB#2783). One subscription wakes one pump.
        //
        // ⚠ These are the two HIGHEST-RATE tables in the module (one upsert per mover per movement
        // heartbeat, one per creature leg). They are handled by `world_view`'s dispatch, which does
        // index bookkeeping and an enqueue per recipient and NOTHING else on the pump — no
        // encoding, no per-session locks held. Adding work to that path is adding it to a thread
        // every session on the shard depends on.
        "SELECT * FROM game_entity_motion",
        "SELECT * FROM game_creature_spline",
        // The SOURCE-side escrow. The ONE module→gateway data flow the cross-database
        // transfer adds: the export blob is written here by `begin_transfer` and the gateway
        // carries it to the other database's `import_character_blob`. Private table, read
        // through the owner token like game_account/game_session. Also the whole of transfer
        // RECOVERY: a restarted gateway re-derives an in-flight transfer's id, destination and
        // payload from this row alone.
        //
        // BASE list, not `sharded_tables`: the table is the escrow primitive itself, present on every
        // module since long before any of the sharding work, so subscribing it unconditionally
        // cannot brick the restart of a gateway whose module predates multi-database routing. Nothing READS
        // it unsharded (`settle_home_shard` short-circuits on `is_sharded()`).
        "SELECT * FROM game_transfer_out",
        // ── THE COORDINATOR-RELAY RULE ─────────────────────────────────────────────────────────
        // Every relay whose loss leaves the CLIENT stuck in a wrong state — as opposed to merely
        // late — is subscribed HERE, on the stable coordinator connection, and never on the
        // per-player one. The per-player connection's AOI grid subscriptions churn as the player
        // moves, and the SDK's in-flight subscription apply can swallow a concurrent transaction's
        // deltas: an instance-CREATING portal entry (~200 rows + the event, one transaction) lost
        // its teleport event, and the player limbo'd — despawned, with no SMSG_TRANSFER pair ever
        // sent. The owner token bypasses each row's recipient RLS, so EVERY player's rows arrive on
        // this one connection and each session's callback self-filters by its own guid.
        //
        // Members of the class, all subscribed below or registered against `coord()` in
        // `stdb::subscriptions`: teleport, XP/level-up, quest log, item instances, the addon
        // bridge, reputation, explored-area fog, and realm-core's group/whisper twins. Anything
        // added to that list must land here too; anything a dropped callback would only make LATE
        // (peer movement, peer VALUES) belongs on the per-player connection.
        //
        // Teleport events — the TRANSFER relay, and the failure that produced the rule.
        "SELECT * FROM game_teleport_event",
        // XP/level-up events, same rule: they ride large kill transactions and
        // their relays moved off the churning per-player conn onto this stable one.
        "SELECT * FROM game_xp_event",
        "SELECT * FROM game_levelup_event",
        // Bot-initiated (serendipity) group invites: the module cannot reach realm-core
        // to write a party itself, so it writes the DECISION here and the coordinator's
        // `world::party::run_bot_invite` relay executes it — the same guid-based `realm_group_op` a
        // player's own CMSG_GROUP_INVITE takes. Rides this stable connection for the same reason the
        // XP/levelup/teleport relays do: there is no player connection to ride at all here (a bot has
        // none), so the coordinator is the ONLY connection that could ever see this row.
        //
        // BASE list, not `sharded_tables`: bots run on every topology, single-database included —
        // gating this behind sharding would silently stop every serendipity invite on the common
        // single-database dev/test deployment (`local-test-harness`), the opposite of the
        // `game_config` reasoning below but the same list for the same kind of reason. One-time
        // bootstrap cost, same as every other base-list table added after day one: a gateway built
        // against this change must not be RESTARTED until the module carrying this table has been
        // published, or `connect_blocking`'s subscription fails to apply (`coordinator_queries`'s own
        // doc comment).
        "SELECT * FROM game_bot_invite_intent",
        // Addon-bridge messages: the server→client UI stream — coordinator-ridden
        // from day one (the coordinator-relay rule). Any addon UI that streams live state rides
        // this, and a dropped frame is a stuck widget on the client.
        "SELECT * FROM game_addon_message",
        // Explored areas (a live find, same rule): the fog-word relay's coordinator leg — a fresh
        // login's first-movement discovery lands exactly in the per-player AOI-churn window.
        "SELECT * FROM game_character_explored",
        // /roll events: under LYRACORE_SHARED_CALLS the shared dispatch
        // (`world_view::roll_appeared`) relays these from this cache instead of ~N per-player
        // subscriptions. Tiny TTL-reaped event table; subscribed unconditionally so the flag
        // never changes what this connection holds.
        "SELECT * FROM game_roll_event",
        // Rest-state flips: same shape — `world_view::
        // rest_state_appeared` relays each row to its owner's session only. TTL-reaped.
        "SELECT * FROM game_rest_state_event",
        // Ground-area spell visuals: `world_view::dynobj_appeared`
        // relays instance-gated CREATE/DESTROY. Short-lived rows (the area's duration).
        "SELECT * FROM game_dynamic_object",
        // Swing log + combat stance: `world_view::
        // combat_event_appeared` / `melee_engaged` / `melee_disengaged` run the shared
        // per-viewer bodies. Both TTL-reaped/ephemeral combat tables.
        "SELECT * FROM game_combat_event",
        "SELECT * FROM game_melee_attack",
        // The social tier: emote broadcast, range-gated say/yell,
        // membership-gated channel lines — `world_view::emote_appeared`/`chat_appeared`/
        // `channel_appeared`. The member table is the audience source for channel lines.
        "SELECT * FROM game_chat_event",
        "SELECT * FROM game_channel_event",
        "SELECT * FROM game_channel_member",
        "SELECT * FROM game_emote_event",
        // Cast visuals + projectile impact: `world_view::
        // cast_event_appeared` runs the full cast-lock contract per viewer; `impact_appeared`
        // the deferred damage number. Both TTL-reaped event tables.
        "SELECT * FROM game_spell_cast_event",
        "SELECT * FROM game_spell_impact_event",
        // Pet bar spells: tiny static table `offer_peer_create_for` joins for the
        // viewer's own pet CREATE (SMSG_PET_SPELLS slots 3–6) — read HERE since the first
        // shared-dispatch move, but for a while not subscribed here at all: the only subscription
        // left was the per-player copy nothing read any more, and the pet bar came up empty.
        "SELECT * FROM game_creature_cast",
        // The PRIVATE tier: recipient-addressed rows the shared
        // dispatch delivers through the owner-session lookup + private_recipient_audience. The
        // owner token bypasses each row's per-recipient RLS — the gateway predicate IS the
        // privacy guarantee on this path. In the BASE list (not the sharded block) so a
        // single-database gateway's shared path works too; on realm-core the same subscription
        // additionally feeds the per-session cross-shard twins, unchanged.
        "SELECT * FROM game_resurrect_request",
        "SELECT * FROM game_whisper_event",
        "SELECT * FROM game_group_event",
        // Server-wide tunables. The gateway reads ONE column: `hosts_instances`, at
        // startup, to answer "when this realm creates a dungeon instance, will anything actually
        // spawn its population" (`ShardMap::check_instance_hosting`). Before this subscription the
        // gateway could only GUESS — the old `instance_hosting_reminder` warned on every sharded
        // startup regardless of the flag's value, and the far more damaging inverse (flag false with
        // no routing ⇒ every dungeon created with 0 entities) was undetectable in either process.
        //
        // BASE list, not `sharded_tables`, and that is load-bearing: that empty-dungeon failure
        // happens with NO shard map at all, so a single-database gateway is exactly the one that
        // must read the flag. `game_config` is public and predates all of the sharding work, so
        // subscribing it cannot brick the restart of a gateway whose module predates
        // multi-database routing (the hazard `coordinator_queries`' doc comment describes). Its
        // BINDING must stay in sync, though — `hosts_instances` was END-appended after the
        // generated `server_config_type.rs` was last regenerated, which
        // `gateway/tests/schema_parity.rs` now pins.
        "SELECT * FROM game_config",
        "SELECT * FROM game_creature_template",
        "SELECT * FROM game_start_position",
        "SELECT * FROM game_corpse",
        // Items slice-1: the static item definitions (for CMSG_ITEM_QUERY_SINGLE) + every
        // owned-item instance. The coordinator authenticates as the module owner, which bypasses
        // RLS, so it reads all players' item rows (like game_character) to drive each login spawn.
        "SELECT * FROM game_item_template",
        // Static spell headers — spell_cast_time() reads cast_time_ms via coord() so the
        // CMSG_CAST_SPELL handler can clear INSTANT casts synchronously.
        "SELECT * FROM game_spell",
        // Spell effects — the gateway reads these to route ITEM-target enchant/disenchant casts by
        // effect KIND (E_ENCHANT_ITEM/E_DISENCHANT) instead of a hardcoded spell-id list.
        "SELECT * FROM game_spell_effect",
        "SELECT * FROM game_item_instance",
        // Buyback ring: read by the coordinator to rebuild a player's buyback-tab view
        // after sell/buyback and at login (private table — the owner-token coordinator sees it).
        "SELECT * FROM game_character_buyback",
        // Items slice-4: a creature corpse's item loot, read for the loot window.
        "SELECT * FROM game_corpse_loot",
        // Vendors (Tier 2): a creature's vendor stock, read for SMSG_LIST_INVENTORY.
        "SELECT * FROM game_npc_vendor",
        // Gossip menus + NPC text (+ weighted slots) + menu options: per-NPC text
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
        // The GAMEOBJECT twin of `game_creature_quest` — GO 68 "Wanted Poster"
        // starts q176 with no creature giver at all, and the GO 55/56 "Lost Guards" corpses drive
        // the q37/q45/q71 END chain. Without this subscription the coordinator's `RemoteTables`
        // cache of `game_gameobject_quest` stays empty forever (the binding/decode plumbing landed
        // earlier, but nothing ever asked the node to send rows) and `quest_giver_evals`'s GO
        // fallback would silently evaluate against zero relations no matter what the module holds.
        "SELECT * FROM game_gameobject_quest",
        "SELECT * FROM game_character_quest",
        // The player's learned spells, chained into the login spellbook (e.g. Auto Shot).
        "SELECT * FROM game_player_spell",
        // Imported action-bar rows (hand-written binding — see
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
        // Spell-modifier mirror: the coordinator reads every player's A_SPELLMOD auras to
        // build the SMSG_SET_FLAT/PCT_SPELL_MODIFIER packets at login + on a talent pick.
        "SELECT * FROM game_aura",
        // The player's persisted reputation standings, chained into the
        // login SMSG_INITIALIZE_FACTIONS. Without this subscription the coordinator's local cache of
        // game_player_reputation is empty and player_reputations() always returns nothing (the bug
        // this fixes — the relay path for the live SET_FACTION_STANDING update subscribes per-player
        // separately in subscriptions.rs, but the coordinator's own read cache needs its own sub too).
        "SELECT * FROM game_player_reputation",
        // Friends/ignore: every character's contact rows, so the coordinator can
        // build any player's SMSG_FRIEND_LIST/SMSG_IGNORE_LIST (RLS-bypassed, like game_character).
        "SELECT * FROM game_character_contact",
        // Reaction gating: standing-derived reaction for interaction windows —
        // the NPC's template resolves to its parent faction (rep bar or mask fallback). Both
        // static/small (314 + ~470 rows).
        "SELECT * FROM game_faction",
        "SELECT * FROM game_faction_template",
        // Guid ranges. Two one-row tables, subscribed on EVERY deployment because a database
        // that holds no range refuses to create characters at all: `ensure_guid_ranges` reads the
        // allocator's high-water to decide which range the database is already minting from, and
        // reads back the range it ends up holding to log it.
        "SELECT * FROM game_guid_allocator",
        "SELECT * FROM game_guid_range",
    ];
    if sharded_tables {
        // Character→shard index. On realm-core this is the realm-wide directory the login
        // path consults; on a world shard it is the receipt `finish_transfer` writes in the escrow's
        // own transaction. Private, like game_account/game_session — the owner token reads it, no
        // client ever sees it. Multi-database deployments only (see this function's doc comment).
        queries.push("SELECT * FROM game_character_shard");
        // Party state (the realm-core group slice). On REALM-CORE these three are the authoritative party
        // tables the gateway drives and relays from; on a world shard they are that shard's mirror,
        // which the gateway also reads (a session's own roster at world entry, before it has pushed
        // anything). Subscribed on every connection in the set for the same reason accounts are:
        // each handle reads its own database's copy, and the gateway decides which copy
        // is authoritative.
        //
        // `game_group_event` is the RELAY: flag-off on a world shard the per-player connection
        // subscribes it under RLS (the pre-realm-core path), and on realm-core the owner-token
        // coordinator reads every player's rows and each session self-filters by `recipient_guid`
        // — the coordinator-relay rule above. (The event table itself has since moved to the BASE
        // list above so the shared dispatch works on a single-database gateway too; the two
        // party-STATE tables stay multi-database-only.)
        queries.push("SELECT * FROM game_group");
        queries.push("SELECT * FROM game_group_member");
        // Loot rolls — a DIFFERENT reason than every table above: nothing here is a CLIENT
        // relay (`game_group_event` still carries every wire-visible roll transition, unchanged). The
        // gateway's own loot-roll relay (`world::loot::relay_tick`) needs these two PRIVATE tables to
        // do its job: on a WORLD SHARD, `pending_local_rolls` reads a freshly kill-time-created
        // staging roll to promote onto realm-core; on REALM-CORE, the promoted/voted rows are what
        // `remove_member`'s disband branch resolves directly. Subscribed on every connection in the
        // set for the same reason party state is: each handle reads its own database's copy, and the
        // relay decides which copy needs which action.
        queries.push("SELECT * FROM game_loot_roll");
        queries.push("SELECT * FROM game_loot_roll_vote");
        // The guid-range REGISTRY — realm-core's answer to which shard mints from where.
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
/// `sharded_tables` adds the MULTI-DATABASE tables (realm-core's `game_character_shard`) to the
/// subscription set — see [`coordinator_queries`] for why that flag exists and must stay off by
/// default.
fn connect_blocking(
    uri: String,
    db_name: String,
    token: Option<String>,
    sharded_tables: bool,
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
                    // Re-arm any relay with no per-login point to self-heal through — MUST run
                    // after the swap above: the hook re-reads `inner.coord()`, which by now answers the
                    // fresh connection. Cloned out from under its own lock so the hook runs unlocked (it
                    // takes `live`'s read lock itself via `coord()`).
                    let hooks = inner.on_reconnect.lock().unwrap().clone();
                    for hook in hooks {
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

/// How often the loot-roll relay promotes staging rolls and settles resolved winners. Short
/// relative to the 60s roll window: promotion latency is the ONLY thing it bounds (a vote cast in the
/// gap between a roll's kill-time creation and its promotion is rejected as "no roll open on that
/// item" and must be retried — the vote is per-action, so the client's own retry-on-click covers it),
/// and a real client cannot render the roll popup and react to it within one tick anyway.
const LOOT_ROLL_RELAY_POLL: Duration = Duration::from_millis(200);

/// Background loop driving `world::loot::relay_tick`: promotes each world shard's freshly
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

/// Background loop driving `load_sample::sample_and_record`: every `LYRACORE_LOAD_SAMPLE_SECS`
/// (default 30s), scrape the node's `/v1/metrics` for each configured shard's writer occupancy,
/// read this gateway's own session counts, and record all of it onto
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

/// **Every world shard holds a guid range before it can mint a character.**
///
/// Runs once per gateway start, right where `check_instance_hosting` enforces the other
/// cross-database invariant: this is the only place that knows both realm-core and the shard set,
/// and it is the courier between them — a module cannot read another database, so the claim can
/// only be driven from here.
///
/// Per world shard: read the high-water it is already minting from, ask realm-core to assign it a
/// range (idempotent — a shard keeps the one it holds), then install that range on the shard. The
/// mark is what makes the four live databases land on the floors they were given by hand, in any
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
                         row — character creation there stays refused"
                    );
                    continue;
                }
                Err(e) => {
                    log::error!(
                        "shard {shard} could not claim a guid range from {rc_name} ({e:#}) — it \
                         will REFUSE to create characters until it holds one"
                    );
                    continue;
                }
            },
            // No realm-core: there is no registry to arbitrate, so there is also only one authority
            // deciding anything — this gateway. Slot 0 keeps a single-database realm (and every dev
            // run) minting exactly as it did before ranges existed. A SECOND world shard in this
            // state is the colliding-guid hazard itself (two shards minting the same character
            // guids), so it is called out rather than quietly assigned.
            None => {
                if map.shards().len() > 1 {
                    log::error!(
                        "shard {shard} has no realm-core to assign it a guid range, and this \
                         gateway serves {} world shards — set LYRACORE_REALM_CORE, or these shards will \
                         mint COLLIDING character guids",
                        map.shards().len()
                    );
                    continue;
                }
                0
            }
        };
        match install(inner, base) {
            Ok(()) => log::info!("shard {shard} mints character guids from base {base}"),
            Err(e) => log::error!(
                "shard {shard} refused the guid range assigned to it ({e:#}) — it will not create \
                 characters. This means the assignment disagrees with what it has already \
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
        // The party-STATE tables. `game_group_event` and `game_whisper_event`
        // moved OUT of this list to the base set when the shared dispatch landed — both predate sharding, so the
        // restart hazard this list exists for cannot bite them, and the shared dispatch needs
        // them on every coordinator (a cache-only subscription with the flag off: the realm
        // relay registers only on multi-database gateways, so there is no double delivery).
        "SELECT * FROM game_group",
        "SELECT * FROM game_group_member",
        // The loot-roll pair: both PRIVATE, no per-player subscriber to duplicate — the restart hazard alone is why
        // they belong on this list (a module published before they exist refuses the subscription).
        "SELECT * FROM game_loot_roll",
        "SELECT * FROM game_loot_roll_vote",
        // Realm-core's guid-range registry. The per-shard `game_guid_range`/
        // `game_guid_allocator` pair is deliberately NOT here — a database that holds no range
        // cannot create characters at all, so every deployment needs those two, sharded or not.
        "SELECT * FROM game_guid_range_registry",
    ];

    #[test]
    fn a_single_database_gateway_subscribes_no_multi_database_table() {
        // A review caught exactly this class of regression — a table subscribed on every
        // coordinator connection even when unconfigured, which bricks a gateway restarted before
        // the module was republished. Each sharded-only table re-enters the same trap, so the
        // guard gets a NAMED test rather than a comment.
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

/// Fire a reducer and **do not wait** for its completion.
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
    /// Shard-scoped: the connection is opened against THIS handle's database and cached in
    /// THIS shard's `players` map, so a session holding its home-shard handle can only ever open a
    /// player connection — and therefore only ever call reducers and subscribe — on its home shard.
    /// Live player-session count on THIS shard — the size of the very cache `player_conn`
    /// below maintains. Approximate: an account with a cached-but-dead connection (see the
    /// liveness check just below) still counts until its NEXT checkout evicts it, and an account
    /// that never called any reducer that opens a player connection is not counted at all even if
    /// its client socket is open (logon-phase connections only need the owner connection). Good
    /// enough for an ops gauge.
    pub(crate) fn session_count(&self) -> usize {
        self.0.players().len()
    }

    pub(crate) fn player_conn(&self, account_id: u64) -> Result<Arc<PlayerConn>> {
        // ⚠ The cache read is a STATEMENT of its own, and it must stay one.
        //
        // Written as `if let Some(p) = self.0.players.lock().unwrap()…`, the `MutexGuard` temporary
        // from the scrutinee lives until the END of the `if let` body — the edition-2021
        // temporary-scope rule (edition 2024 changed it; this workspace is 2021, see
        // `[workspace.package] edition` in the root `Cargo.toml`). Under that shape the two lines
        // below each took the whole realm down, and both are reachable ONLY when a cached
        // connection is unhealthy — i.e. exactly during a mass-session churn (login storm or mass
        // disconnect), which is where both outages happened:
        //
        //  1. the `remove` re-locks `players` while the scrutinee guard is still held.
        //     `std::sync::Mutex` is not reentrant, so the session thread DEADLOCKS holding the
        //     process-wide `players` lock, and every other session's `player_conn` /
        //     `session_count` queues behind it forever. Each one also strands a tokio
        //     blocking-pool thread (default cap: 512) permanently. This is the "evict and
        //     rebuild a dead conn" path, so that repair has never actually been able to run.
        //  2. `is_active()` is `send_chan.lock().unwrap()` *inside the SDK*. Once a per-player
        //     connection's pump thread has panicked at
        //     `spacetimedb-sdk-2.7.1/src/db_connection.rs:413` ("Unable to send unsubscribe
        //     message…" — the churn crash's log signature, thrown while that very mutex is held) the mutex
        //     is POISONED and `is_active()` panics. Under the old shape it panicked while holding
        //     the `players` guard, poisoning `players` too — after which every `.lock().unwrap()`
        //     on it panics, for every account on this shard.
        //
        // Binding the clone first makes the guard's life one statement long: a panic out of
        // `is_active()` unwinds one session and leaves the cache usable, and the eviction below
        // can no longer self-deadlock.
        let cached = self.0.players().get(&account_id).cloned();
        if let Some(p) = cached {
            // Liveness check on checkout: a module republish closes every websocket
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
            self.0.players().remove(&account_id);
        }
        let uri = self.0.uri.clone();
        let db_name = self.0.db_name.clone();
        // The build must see an ambient runtime, so the SDK's `enter_or_create_runtime()`
        // takes its `Handle::try_current() == Ok` branch and REUSES this runtime instead of
        // creating a private 1-worker runtime per connection. That private runtime was ~1-2 OS
        // threads per distinct account, never released for the gateway's lifetime,
        // and at 500 accounts it is what left the gateway unable to serve logons after a
        // mass-disconnect run.
        //
        // ⚠ "ambient runtime" must NOT mean "on a runtime worker". The previous shape
        // (`handle.spawn(async { block_in_place(build) })`) starves under exactly the load this
        // path exists for. `block_in_place` on a worker GIVES THE WORKER'S CORE AWAY and asks the
        // blocking pool to run it — `runtime::spawn_blocking(move || run(worker))`,
        // tokio-1.53.1 `runtime/scheduler/multi_thread/worker.rs:489`, whose own comment four
        // lines up reads "If we heavily call `spawn_blocking`, there might be no available thread
        // to run this core". This gateway heavily calls `spawn_blocking`: EVERY world session
        // (`world::run`) and EVERY logon handshake (`logon::run`) holds one pool thread for its
        // whole life. So at a few hundred sessions the pool is full, the handed-off core is never
        // picked up, and the runtime loses a core PER IN-FLIGHT BUILD — taking down the SDK
        // message pumps that every other session's reducer completions ride on. Measured offline
        // (see `connect_build_does_not_starve_on_a_full_blocking_pool` below): 8 workers, pool
        // full, 8 builds in flight ⇒ an unrelated task waits 1.7 s; the shape below waits 51 ms.
        // That is what produced the live "player connect task did not answer within 20s" errors and
        // the bimodal movement p95 at ~500 seats.
        //
        // So: build on a THREAD THE RUNTIME DOES NOT OWN, with an `enter()` guard. The guard makes
        // `Handle::try_current()` succeed (keeping the shared-runtime reuse above and the SDK's message
        // loops on this runtime), while the SDK's internal `block_in_place` takes its "outside of
        // the tokio runtime, so blocking is fine" branch — no core handed away, no blocking-pool
        // thread needed, nothing to queue behind.
        //
        // Nothing about the security model moves: the connection is still per-account with its own
        // minted identity, so `ctx.sender` authorisation and the module's RLS filters are untouched.
        let conn = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::Builder::new()
                    .name(format!("stdb-player-{account_id}"))
                    .spawn(move || {
                        let _guard = handle.enter();
                        let built = connect_player_blocking(uri, db_name);
                        // The caller's `recv_timeout` below is a HARD deadline, but it cannot
                        // cancel this build — so a connection that lands late must be cleaned up
                        // HERE or it is orphaned. It was never inserted into `players`, so
                        // `release_player_conn_on` will never see it; the SDK's `DbContextImpl`
                        // has no `Drop`, and the `run_threaded` pump holds a clone, so dropping it
                        // leaves a live websocket + a live pump thread for the process lifetime.
                        // That is the same fd/thread leak the churn fix closed, on the failure path.
                        if let Err(std::sync::mpsc::SendError(Ok(p))) = tx.send(built) {
                            log::warn!(
                                "player conn for account {account_id} arrived after the caller's \
                                 deadline — disconnecting the orphan"
                            );
                            let _ = p.conn.disconnect();
                            let pump = p.pump.lock().unwrap().take();
                            if let Some(h) = pump {
                                let _ = h.join();
                            }
                        }
                    })
                    .context("spawn player connect thread")?;
                rx.recv_timeout(Duration::from_secs(20))
                    .map_err(|_| anyhow!("player connect task did not answer within 20s"))??
            }
            // No ambient runtime: fall back to the original dedicated-thread build. Correct, just
            // one private runtime per connection. Unit-tests-only since the shared-connection work
            // retired the seam view-merge (its `ensure_away` opened away-shard player connections
            // from bare `std::thread`s, which was the one production path into this branch —
            // measured at ~3.6 threads/session co-located, ~5.2 dispersed).
            Err(_) => std::thread::Builder::new()
                .name(format!("stdb-player-{account_id}"))
                .spawn(move || connect_player_blocking(uri, db_name))
                .context("spawn player connect thread")?
                .join()
                .map_err(|_| anyhow!("player connect thread panicked"))??,
        };
        let arc = Arc::new(conn);
        self.0.players().insert(account_id, arc.clone());
        Ok(arc)
    }

    /// Register a live world socket for `account_id` (called once its handshake has resolved the
    /// account). Pairs with [`Coordinator::detach_account_session`]. See [`AccountSessions`].
    pub(crate) fn attach_account_session(&self, account_id: u64) {
        self.1.live_sessions.attach(account_id);
    }

    /// Retire a live world socket for `account_id` at teardown and, iff it was the LAST one,
    /// release the account's cached per-account connections on every shard.
    ///
    /// This is the whole fd/thread reclaim: until it was wired up, each distinct account leaked one
    /// websocket fd and one SDK pump OS thread **per shard it ever touched**, for the gateway's
    /// lifetime. `accept(2)` returns `EMFILE` once the process runs out of fds, and both accept
    /// loops propagate that straight into `main` — so the leak is what ends the process, at a
    /// session count that is a pure function of `ulimit -n`.
    ///
    /// Safety is [`AccountSessions::detach`]'s postcondition, not this function's: it returns true
    /// only when no socket for the account remains, so there is nobody left to cut off. Note this
    /// is a STRONGER gate than the `release_session` epoch arbitration `leave_world` uses — see
    /// [`AccountSessions`] for the character-select case the epoch gate would get wrong.
    pub(crate) fn detach_account_session(&self, account_id: u64) {
        if !self.1.live_sessions.detach(account_id) {
            return;
        }
        self.release_account_conns(account_id);
    }

    /// Retire a live LOGON socket for `account_id`. Never releases immediately: the account's
    /// next socket is normally the world session that reuses this very connection, and the two
    /// closes are not ordered. See [`AccountSessions`] for why releasing here would be worse than
    /// slow, and [`Coordinator::spawn_account_session_reaper`] for what does release it.
    pub(crate) fn detach_account_session_deferred(&self, account_id: u64) {
        self.1
            .live_sessions
            .detach_deferred(account_id, Instant::now());
    }

    /// Release every account parked by a logon close at least `grace` ago that no world session
    /// claimed. Returns how many were released.
    ///
    /// Takes `now`/`grace` rather than reading the clock so the whole predicate is drivable from a
    /// test without sleeping; [`Coordinator::spawn_account_session_reaper`] supplies the real ones.
    pub(crate) fn reap_idle_account_sessions(&self, now: Instant, grace: Duration) -> usize {
        let due = self.1.live_sessions.reap_idle(now, grace);
        for account_id in &due {
            self.release_account_conns(*account_id);
        }
        due.len()
    }

    /// Sweep for logon connections nobody claimed. Without this task nothing ever reclaims
    /// them: `detach_account_session_deferred` deliberately only parks, so the reaper is the whole
    /// release path for the logon tier.
    pub fn spawn_account_session_reaper(&self) {
        let coord = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(ACCOUNT_SESSION_REAP_INTERVAL);
            loop {
                tick.tick().await;
                let n = coord.reap_idle_account_sessions(Instant::now(), LOGON_HANDOVER_GRACE);
                if n > 0 {
                    log::info!(
                        "account-session reaper: released {n} per-account connection(s) whose logon \
                         never entered the world"
                    );
                }
            }
        });
    }

    /// Drop `account_id`'s cached per-account connection on every shard.
    ///
    /// EVERY shard, not just this handle's: the account's home shard can change
    /// mid-session across a cross-database transfer, and the logon tier opens its connection on
    /// the DEFAULT shard regardless of where the world session lands — so an account can hold a
    /// cached `PlayerConn` in more than one shard's `players` map, and each leaks the same fd +
    /// thread. `release_player_conn_on` is a `remove`, so covering a shard twice is inert.
    fn release_account_conns(&self, account_id: u64) {
        for inner in self.1.conns.values() {
            release_player_conn_on(inner, account_id);
        }
        release_player_conn_on(&self.0, account_id);
    }
}

/// How long a cached per-account connection outlives the logon socket that opened it when no world
/// session has taken over.
///
/// Sized for the handover, not for the leak: the gap it has to cover is "client closes its logon
/// socket → client completes the world handshake", which is milliseconds in the normal case and
/// bounded by the login queue in the worst one. Erring long is nearly free here — the leak this
/// reclaims is one connection per DISTINCT account that authenticated and never played (the cache
/// dedupes repeats), so it grows over a server's lifetime rather than in bursts, and two minutes of
/// extra retention changes no ceiling. Erring short is not free: a session that rebuilds its
/// connection gets a fresh identity, which `establish_session` has not bound.
///
/// ⚠ A client that sits in the login queue for longer than this reaches
/// `open_account_session` after its connection was already reaped, and its `player_login` then
/// fails `account_by_identity` — recoverable by reconnecting (the next logon re-binds), and inert
/// on the default topology, where `LYRACORE_MAX_SESSIONS` is unset and nothing ever queues.
const LOGON_HANDOVER_GRACE: Duration = Duration::from_secs(120);

/// How often [`Coordinator::spawn_account_session_reaper`] sweeps.
const ACCOUNT_SESSION_REAP_INTERVAL: Duration = Duration::from_secs(30);

/// Release one shard's cached connection for `account_id`: drop it from the cache, disconnect the
/// websocket, and reap the pump thread off-thread.
///
/// Two earlier attempts at this reclaimed NOTHING, because each connection owned a private tokio
/// runtime that outlived it. With the build moved onto a shared runtime worker (see
/// [`Coordinator::player_conn`]) the only remaining per-connection thread is the pump, and joining
/// it lets the connection actually drop.
///
/// ⚠ Not safe on its own — a stale socket SHARES this connection with the session that superseded
/// it. Only [`Coordinator::detach_account_session`] may call it, and only once the account's
/// last live socket is gone.
fn release_player_conn_on(inner: &CoordinatorInner, account_id: u64) {
    let Some(p) = inner.players().remove(&account_id) else {
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

impl Coordinator {
    /// Connect ONE database and wrap it in a watchdogged `CoordinatorInner`. `sharded_tables` — see
    /// [`connect_blocking`]; a single-database gateway passes `false` and subscribes exactly the
    /// tables it subscribed before realm-core existed.
    async fn connect_shard(
        cfg: &GatewayConfig,
        db_name: &str,
        sharded_tables: bool,
    ) -> Result<Arc<CoordinatorInner>> {
        let uri = cfg.stdb_uri.clone();
        let db = db_name.to_string();
        let token = cfg.coordinator_token.clone();
        // Build on a dedicated OS thread (no tokio context), join it off the reactor.
        let build_thread = std::thread::Builder::new()
            .name("stdb-coordinator-connect".into())
            .spawn(move || connect_blocking(uri, db, token, sharded_tables))
            .context("spawn coordinator connect thread")?;
        let live = tokio::task::spawn_blocking(move || -> Result<LiveConn> {
            build_thread
                .join()
                .map_err(|_| anyhow!("coordinator connect thread panicked"))?
        })
        .await
        .context("join coordinator connect task")??;
        // The call-pipe pool. Total pipes = LYRACORE_CALL_PIPES (default 4); <=1 keeps
        // the single-connection call path. A pipe that fails to build is logged and
        // skipped — the pool degrades toward `coord()`, never fails the gateway.
        let want_pipes = crate::config::call_pipes();
        let mut call_pipes = Vec::new();
        if want_pipes > 1 {
            for i in 0..want_pipes {
                let (u, d, t) = (
                    cfg.stdb_uri.clone(),
                    db_name.to_string(),
                    cfg.coordinator_token.clone(),
                );
                let built = tokio::task::spawn_blocking(move || {
                    std::thread::Builder::new()
                        .name(format!("stdb-call-pipe-{i}"))
                        .spawn(move || connect_call_pipe(u, d, t))
                        .expect("spawn call pipe thread")
                        .join()
                        .map_err(|_| anyhow!("call pipe connect thread panicked"))?
                })
                .await;
                match built {
                    Ok(Ok(pipe)) => call_pipes.push(RwLock::new(pipe)),
                    Ok(Err(e)) => log::warn!("call pipe {i} failed to build (skipped): {e:#}"),
                    Err(e) => log::warn!("call pipe {i} join failed (skipped): {e:#}"),
                }
            }
            log::info!(
                "call-pipe pool: {} of {want_pipes} pipes up for {db_name}",
                call_pipes.len()
            );
        }
        let inner = Arc::new(CoordinatorInner {
            live: RwLock::new(live),
            token: cfg.coordinator_token.clone(),
            uri: cfg.stdb_uri.clone(),
            db_name: db_name.to_string(),
            sharded_tables,
            call_pipes,
            call_pipe_next: std::sync::atomic::AtomicUsize::new(0),
            motion_batch: Mutex::new(Vec::new()),
            players: Mutex::new(HashMap::new()),
            on_reconnect: Mutex::new(Vec::new()),
        });
        spawn_coordinator_watchdog(inner.clone());
        // The movement batch flusher — one gw_movement_batch transaction per 40ms tick
        // (25/s, the same cadence philosophy as publish_motion's write batching). Fire-and-forget
        // per flush; a failed send logs and drops that tick's moves (the next heartbeat
        // supersedes them anyway — movement is idempotent positional state).
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(40));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let batch: Vec<super::bindings::GwMove> = {
                        let mut b = inner.motion_batch.lock().unwrap_or_else(|p| p.into_inner());
                        if b.is_empty() {
                            continue;
                        }
                        std::mem::take(&mut *b)
                    };
                    // Batch shape: cap each call at 128 moves and spread the chunks across
                    // the pipe pool — a single ~400-move transaction head-of-line-blocked login
                    // calls for its whole execution (the measured 1970→1071 seat regression);
                    // several ~5ms transactions interleave with them instead.
                    for chunk in batch.chunks(128) {
                        let n = chunk.len();
                        let pipe = inner.call_pipe();
                        if let Err(e) = pipe.conn.reducers.gw_movement_batch(chunk.to_vec()) {
                            log::warn!(
                                "gw_movement_batch send failed ({n} moves dropped this tick): {e}"
                            );
                        }
                    }
                }
            });
        }
        Ok(inner)
    }

    /// Connect the coordinator set: one privileged connection per database in the shard map,
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
        // The character→shard index only exists to answer "which of several databases owns
        // this". One database ⇒ nothing to answer, nothing subscribes it, and the subscription set
        // is exactly the pre-sharding one — which is what makes "the env vars unset ⇒ today's
        // gateway" true of the wire, not just of the routing logic.
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
        // The ROUTING is ours and `game_config.hosts_instances` is each database's, and
        // this is the only place both are known. Reads the flag out of each coordinator's just-
        // applied subscription cache (`connect_blocking` blocks until it is applied, so the row is
        // there); a database with no row reads `None` ⇒ "hosts", the module's own default. Silent
        // when correctly configured — including the ordinary single-database realm — a WARNING when
        // the world shard spawns populations it will not run, and FATAL when nothing anywhere
        // will spawn them (every dungeon then gets created with 0 entities).
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
        let world = Arc::new(super::world_view::WorldView::new(crate::config::aoi_enabled()));
        let coordinator = Self(
            home,
            Arc::new(ShardSet {
                map,
                conns,
                sessions: SessionEpochs::default(),
                live_sessions: AccountSessions::default(),
                world,
            }),
        );
        // Arm the SHARED area-of-interest dispatch — one registration per shard instead of
        // one subscription per player. Must run before any session can log in, and must be re-armed
        // after a coordinator reconnect (the watchdog treats a module republish as one), which is
        // what the `on_reconnect` hook below is for: the callbacks are bound to a `LiveConn` that
        // the swap replaces.
        coordinator.arm_shared_world_view();
        // The loot-roll relay: promotes each world shard's staging loot rolls onto realm-core and settles
        // resolved winners back down. A no-op loop on an unsharded gateway (`relay_tick` returns
        // immediately when `realm_store()` is `None`), so this costs a single-database deployment
        // nothing but one idle thread.
        spawn_loot_roll_relay(coordinator.clone());
        // The load sampler: per-shard writer occupancy + session counts, sampled on a timer and recorded
        // onto realm-core so an operator can answer "which shard is hot" with `spacetime sql`
        // alone.
        spawn_load_sampler(coordinator.clone(), cfg.stdb_uri.clone());
        Ok(coordinator)
    }

    /// The gateway-wide shared area-of-interest view — the cell index + viewer registry the
    /// coordinator dispatch routes through. Shard-independent: every handle answers the same one.
    pub(crate) fn world_view(&self) -> Arc<super::world_view::WorldView> {
        self.1.world.clone()
    }

    /// Register the shared AOI relays on EVERY connected shard's coordinator connection, and install
    /// the watchdog re-arm hook that keeps them alive across a reconnect.
    ///
    /// The shard ORDER fixes the `ShardId`s the index stores, so this runs once, before any session
    /// exists, and the vector is never reordered afterwards. The re-arm re-registers on the fresh
    /// `LiveConn` — the index itself survives untouched, which is deliberate: a reconnect must not
    /// blank every player's view. It does re-seed, because a fresh subscription's apply repopulates
    /// the cache without firing per-row `on_insert`.
    fn arm_shared_world_view(&self) {
        let view = self.world_view();
        let shards = self.all_shards();
        view.set_shards(shards.clone());
        for (id, shard) in shards.iter().enumerate() {
            super::world_view::arm_shard(view.clone(), shard.clone(), id);
            let (hook_view, hook_shard) = (view.clone(), shard.clone());
            shard
                .0
                .on_reconnect
                .lock()
                .unwrap()
                .push(Arc::new(move || {
                    super::world_view::arm_shard(hook_view.clone(), hook_shard.clone(), id);
                    super::world_view::seed_from_caches(&hook_view);
                }));
        }
        super::world_view::seed_from_caches(&view);
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
    /// character. Same contract — `None` means "already this handle's shard" — with the one
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
    /// failure signal cannot tell them apart (a live bug read exactly this way: a database
    /// that was trivially connected produced a "not connected" warning because the caller asked for
    /// its OWN shard — the fix is to never call this with `db == self.shard_name()` in the first
    /// place, not to guess here after the fact). The warning below only fires for the SECOND case —
    /// `db` genuinely absent from the connected set — so it stays silent on every "already home"
    /// call, which is the common case for `shard_for`/`instance_shard_for`/`settle_home_shard`.
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

    /// Every connected shard's handle, DEFAULT FIRST. Two callers: the character-select list
    /// (a character parked inside an instance lives on the instance shard, and the player must
    /// still see it) and the party mirror fan-out (`world_stores`).
    ///
    /// ⚠ This is exactly `world_shards()` with the names dropped — REALM-CORE IS NOT IN
    /// IT. It was originally written over `conns`, which was the same set only while the default
    /// database was also the auth database; once realm-core is a third connection, probing it for characters
    /// asks a database that owns none and (worse) could answer with the shadow account rows the
    /// transfer's `import_character_blob` writes there. `ShardMap::shards()` excludes it by
    /// construction, so the probe set is the right one for both topologies.
    ///
    /// Deliberate simplification: O(shards) per login for a caller that needs every handle. The
    /// holder LOOKUP no longer pays this unconditionally — `realm_core::locate_home_shard`
    /// tries the realm-core `character_shard` hint first and only falls back to scanning
    /// `world_shards()` (this method's underlying set) on a miss.
    pub(crate) fn all_shards(&self) -> Vec<Coordinator> {
        self.world_shards()
            .into_iter()
            .map(|(_, coord)| coord)
            .collect()
    }

    // Realm-core removed `realm_shard()` — the old "the default shard is where accounts AND
    // characters live" assumption. Both of its jobs now have a more precise owner: accounts and sessions belong to `realm_core()`
    // below, and "which shard holds this character" is answered by probing (`world_shards()`) rather
    // than by assuming the default — the assumption that stops being true the first time a transfer
    // actually moves someone. It had no other callers.

    /// The **realm-core** handle: the database that owns accounts, sessions, and the
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
    /// when the realm-core index has no hint. Realm-core is excluded by `ShardMap::shards`: it
    /// holds no characters.
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
    /// TEMPORARY probe: can a player `DbConnection` be built on a multi-thread runtime WORKER?
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

/// The process-wide `players` lock must never be held across the liveness check.
///
/// `player_conn` is the one place a `MutexGuard` on `Coordinator`'s shared `players` map could
/// outlive its own statement, and under edition 2021 the `if let ... = mutex.lock().unwrap()...`
/// shape makes it do exactly that. The consequences are two different realm-wide outages (a
/// self-deadlock on the eviction `remove`, and poisoning `players` when the SDK's `is_active()`
/// panics on its own poisoned mutex) — both reachable only when a cached player connection is
/// unhealthy, which is the mass-session-churn state (login storm / mass disconnect). Neither can be
/// reproduced in a unit test: both need a live SpacetimeDB node and a per-player connection whose
/// pump thread has already panicked. (`CoordinatorInner::players()` now recovers a poisoned lock the same way
/// `coord()`/`call_pipe()` do, so a future fallible call inside a narrowed critical section
/// degrades to a logged error instead of reproducing either outage.)
///
/// So it is pinned the way this crate pins its other live-only invariants — a behavioural test of
/// the LANGUAGE RULE (so the scan below is not lexical superstition), plus a source scan of the one
/// function. `deadlocks` is asserted with `try_lock`, never `lock`: a test that actually deadlocked
/// would hang the suite rather than fail it.
#[cfg(test)]
mod player_conn_lock_scope {
    use crate::test_scan::code_of;

    /// The hazard itself, on a plain `Mutex<HashMap>` — the exact shape `player_conn` used to have.
    /// If a future edition bump makes this pass, the scan below becomes optional rather than wrong,
    /// and this test is where that shows up.
    #[test]
    fn an_if_let_scrutinee_guard_is_still_held_inside_the_body() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        let players: Mutex<HashMap<u64, u64>> = Mutex::new(HashMap::from([(1u64, 7u64)]));
        if let Some(_p) = players.lock().unwrap().get(&1).cloned() {
            assert!(
                players.try_lock().is_err(),
                "the scrutinee `MutexGuard` was dropped before the `if let` body — the edition-2021 \
                 temporary-scope rule this module guards against no longer applies. Re-read \
                 `player_conn`'s comment before relaxing anything."
            );
        } else {
            panic!("fixture is wrong: the key must be present");
        }
        // Control: bound to a `let` first, the guard is gone by the time the body runs — which is
        // precisely the fix `player_conn` applies.
        let cached = players.lock().unwrap().get(&1).cloned();
        if let Some(_p) = cached {
            assert!(
                players.try_lock().is_ok(),
                "binding the clone to its own `let` must end the guard's life at that statement"
            );
        }
    }

    #[test]
    fn player_conn_never_locks_players_in_an_if_let_scrutinee() {
        let src = include_str!("connection.rs");
        let body = code_of(
            src,
            "pub(crate) fn player_conn(&self, account_id: u64) -> Result<Arc<PlayerConn>> {",
        );
        for line in body.lines() {
            let t = line.trim();
            let scrutinee_lock = (t.starts_with("if let ")
                || t.starts_with("while let ")
                || t.starts_with("match "))
                && t.contains("players")
                && t.contains(".lock()");
            assert!(
                !scrutinee_lock,
                "`player_conn` locks `players` in a `{t}` scrutinee. Under edition 2021 that guard \
                 lives to the end of the body, which (a) self-deadlocks on the eviction `remove` \
                 and (b) poisons the process-wide `players` map if the SDK's `is_active()` panics \
                 on its own poisoned mutex — the two realm-wide outages this pins. Bind the \
                 cloned value to its own `let` first."
            );
        }
        assert!(
            body.contains("let cached = self.0.players().get(&account_id).cloned();"),
            "`player_conn` no longer takes the cached connection out of the map in a statement of \
             its own — the one thing that keeps the `players` guard from outliving the liveness \
             check. Body was:\n{body}"
        );
    }
}

/// Why `player_conn`'s build runs on a thread the runtime does NOT own.
///
/// The gateway spends one tokio blocking-pool thread per world session and per logon handshake, for
/// that session's whole life (`world::run`, `logon::run`). `block_in_place` on a runtime worker gives
/// the worker's core away and asks that same pool to run it (tokio `multi_thread/worker.rs`), so once
/// the pool is full the core is never picked up — the runtime shrinks by one core per in-flight
/// connection build, and every SDK message pump on it stalls with it. These tests pin the difference
/// between the two shapes with no node and no network; they are the offline half of the evidence
/// for that diagnosis (the live half was a ~535-seat ceiling with bimodal connect timeouts).
#[cfg(test)]
mod connect_build_shape {
    use crate::test_scan::code_of;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::runtime::{Builder, Handle, Runtime};

    /// A runtime shaped like the gateway's: `workers` cores and a blocking pool with EVERY slot held
    /// by a task that does not return — the session-per-`spawn_blocking` shape at its ceiling.
    /// Flip the returned flag before dropping the runtime, or the drop waits forever.
    fn saturated(workers: usize, cap: usize) -> (Runtime, Arc<AtomicBool>) {
        let rt = Builder::new_multi_thread()
            .worker_threads(workers)
            .max_blocking_threads(cap)
            .enable_all()
            .build()
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        for _ in 0..cap {
            let stop = stop.clone();
            let ready = ready_tx.clone();
            rt.spawn_blocking(move || {
                let _ = ready.send(());
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        }
        for _ in 0..cap {
            ready_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("the blocking pool never filled");
        }
        (rt, stop)
    }

    /// The failure mode itself. Four workers, pool full, four `block_in_place` calls in flight ⇒ the
    /// runtime has NO core left, so a plain task cannot run at all. In the gateway those tasks are
    /// the SDK's websocket message and parse loops, which is why one slow connect shows up as every
    /// other session's reducer timing out, and as the measured 12.6 s movement p95.
    #[test]
    fn block_in_place_on_a_worker_costs_a_core_when_the_blocking_pool_is_full() {
        let (rt, stop) = saturated(4, 4);
        let handle = rt.handle().clone();
        for _ in 0..4 {
            handle.spawn(async {
                tokio::task::block_in_place(|| std::thread::sleep(Duration::from_secs(2)));
            });
        }
        std::thread::sleep(Duration::from_millis(200));
        let (tx, rx) = mpsc::channel();
        handle.spawn(async move {
            let _ = tx.send(());
        });
        let ran = rx.recv_timeout(Duration::from_millis(500));
        stop.store(true, Ordering::Relaxed);
        drop(rt);
        assert!(
            ran.is_err(),
            "a `block_in_place` build no longer strands the worker's core when the blocking pool is \
             full — tokio changed the hand-off, so `player_conn`'s comment (and the starvation \
             diagnosis it records) needs re-reading before anything relaxes."
        );
    }

    /// The shape `player_conn` uses instead: a thread the runtime does not own, with an `enter()`
    /// guard. Same saturated runtime as the test above, and it does not care.
    #[test]
    fn connect_build_does_not_starve_on_a_full_blocking_pool() {
        let (rt, stop) = saturated(4, 4);
        let handle = rt.handle().clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _guard = handle.enter();
            // Stands in for `DbConnection::build()`, which ends in
            // `block_in_place(|| handle.block_on(WsConnection::connect(..)))` — i.e. it awaits
            // something only this runtime's driver can complete.
            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                })
            });
            let _ = tx.send(());
        });
        let built = rx.recv_timeout(Duration::from_secs(5));
        stop.store(true, Ordering::Relaxed);
        drop(rt);
        assert!(
            built.is_ok(),
            "a connection build on an entered non-runtime thread starved anyway — the whole point \
             of that shape is that it needs neither a core nor a blocking-pool slot."
        );
    }

    /// The other half of the same line: the `enter()` guard is what keeps the SDK on the SHARED runtime.
    /// Without it `Handle::try_current()` fails and `enter_or_create_runtime()` builds a private
    /// 1-worker runtime per connection — ~1-2 OS threads per account, never released.
    #[test]
    fn an_enter_guard_is_what_makes_the_sdk_reuse_the_shared_runtime() {
        let rt = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.handle().clone();
        let seen = std::thread::spawn(move || {
            let bare = Handle::try_current().is_ok();
            let entered = {
                let _guard = handle.enter();
                Handle::try_current().is_ok()
            };
            (bare, entered)
        })
        .join()
        .unwrap();
        assert_eq!(
            seen,
            (false, true),
            "a bare thread must see no runtime (private-runtime path) and an entered one must see \
             this runtime (the shared-runtime path)"
        );
    }

    /// Pin the shape in the source, the way `player_conn_lock_scope` pins its own: the build must not
    /// go back onto a runtime worker.
    #[test]
    fn player_conn_builds_off_the_runtime_workers() {
        let src = include_str!("connection.rs");
        let body = code_of(
            src,
            "pub(crate) fn player_conn(&self, account_id: u64) -> Result<Arc<PlayerConn>> {",
        );
        assert!(
            body.contains("let _guard = handle.enter();"),
            "`player_conn` no longer enters the ambient runtime on its build thread — without the \
             guard the SDK builds a PRIVATE runtime per connection. Body was:\n{body}"
        );
        for line in body.lines() {
            let t = line.trim();
            // Comments in there NAME both shapes on purpose — it is the explanation of why the
            // code below is written the way it is. Only code counts.
            if t.starts_with("//") {
                continue;
            }
            assert!(
                !t.contains("block_in_place("),
                "`player_conn` calls `block_in_place` again ({t}). On a runtime worker that hands \
                 the core to the blocking pool, which every world session already holds a thread \
                 of — the starvation this shape exists to avoid. Build on a thread the runtime does \
                 not own."
            );
            assert!(
                !t.contains("handle.spawn("),
                "`player_conn` spawns the build as a runtime task again ({t}) — see this module's \
                 doc comment on why the build must stay off the runtime's workers."
            );
        }
    }
}
