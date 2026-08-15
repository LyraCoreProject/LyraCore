//! World listener (TCP 8085): handshake, header crypto, char enum, login, movement,
//! disconnect.
//!
//! Like `logon`, the protocol is a transport- and storage-agnostic state machine over a
//! blocking `Read + Write` stream, driven by `wow_world_messages` (vanilla) codecs and
//! `wow_srp`'s vanilla header cipher. Storage/coordination is behind the [`WorldStore`] trait
//! so the handshake + crypto can be unit-tested end-to-end without a database or a real
//! socket (see the tests below, which drive a full handshake with `wow_srp`'s client side).
//!
//! Per-connection protocol state lives in [`WorldConn`] — none of it is game state, so a
//! gateway restart costs only a client reconnect: K is read back
//! from `game_session` and the same handshake re-runs.

use crate::accept::{classify_accept_error, AcceptBackoff, AcceptOutcome};
use crate::stdb::PlayerSubscriptions;
use crate::{codec, config::GatewayConfig, stdb::Coordinator};
use anyhow::{anyhow, Result};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, SendError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use wow_srp::normalized_string::NormalizedString;
use wow_srp::vanilla_header::{DecrypterHalf, EncrypterHalf, ProofSeed};
use wow_world_messages::errors::ExpectedOpcodeError;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{
    CMSG_MESSAGECHAT_ChatType, MovementInfo, PartyOperation, PartyResult,
    SMSG_CAST_RESULT_SimpleSpellCastResult, ServerMessage, CMSG_MESSAGECHAT, SMSG_AUTH_CHALLENGE,
    SMSG_AUTH_RESPONSE, SMSG_CAST_RESULT, SMSG_CHAT_PLAYER_NOT_FOUND, SMSG_SPIRIT_HEALER_CONFIRM,
};

mod coalesce;
mod handlers;
pub mod login_queue;
pub mod loot;
pub mod mail;
pub mod packet_lint;
pub mod party;
mod social;
mod store;
pub mod transfer;
pub mod whisper;
use coalesce::CoalesceState;
use handlers::{
    dispatch_cast, dispatch_item_action, dispatch_melee_action, dispatch_quest_action,
    dispatch_taxi_action, dispatch_vendor_action, handle_bank, handle_char, handle_combat, handle_loot,
    handle_mail, handle_query, handle_trade, handle_trainer, CastOutcome, CastPlayer, CastTransition,
    ItemActionOutcome, ItemActionPlayer, MeleeActionOutcome, MeleeActionPlayer,
    queue_reply_then_arm, QuestActionOutcome, QuestActionPlayer, TaxiActionOutcome, TaxiActionPlayer,
    VendorActionOutcome, VendorActionPlayer,
};
use login_queue::{Admission, LoginQueue};
use social::handle_social;
pub use store::WorldStore;
use transfer::{EscrowedTransfer, TransferPlan};

/// One unit of outbound traffic for the single writer thread. A `Batch` is written contiguously so
/// the login sequence + self-spawn can never be spliced by an async peer event mid-sequence.
pub enum Outbound {
    One(ServerOpcodeMessage),
    Batch(Vec<ServerOpcodeMessage>),
    /// A pre-serialized packet body sent under a RAW opcode — the escape hatch past gtker's
    /// `pub(crate)` update-mask wall (auras 1..47, multi-field item descriptors). `body` is the
    /// packet body WITHOUT the 4-byte server header; the writer frames it exactly like the gtker path
    /// (`[size:u16 BE][opcode:u16 LE]`, `size = 2 + body.len()`, then encrypts the header via the same
    /// `EncrypterHalf`), so the header-cipher stream stays in lockstep with the typed sends. Built by
    /// `codec::build_values_update_raw` from the hand-rolled `update_mask::UpdateMaskValues`.
    Raw {
        opcode: u16,
        body: Vec<u8>,
    },
    /// DEFERRED relay work. The shared AOI dispatch runs on ONE coordinator pump per shard
    /// that every session now depends on, and the SDK forbids parallel `advance_one_message`
    /// callers — so the pump must never encode. It pushes this instead: a closure that does the
    /// per-viewer gating, cache reads and encoding LATER, on this session's own writer thread.
    ///
    /// Ordering is preserved because a job travels the same queue as everything else: whatever the
    /// job produces is written exactly where the job was enqueued, so a CREATE queued before a
    /// VALUES still reaches the wire first.
    ///
    /// The writer runs it inside `catch_unwind`, and it may produce zero packets (the common case
    /// for a row a gate rejects — a job is enqueued per candidate viewer, and rejecting a candidate
    /// costs one dequeue instead of pump time).
    Job(RelayJob),
}

/// One unit of deferred relay work — see [`Outbound::Job`]. Returns the packets to write, in order;
/// nested jobs are not expanded (nothing produces one, and honouring them would make the queue's
/// ordering guarantee depend on recursion depth).
pub type RelayJob = Box<dyn FnOnce() -> Vec<Outbound> + Send>;

/// Queued-but-unwritten `Outbound` items past which the two N²-shaped relays stop enqueueing
/// (see [`SessionTx`] and `subscriptions::shed_motion_at_depth`).
///
/// Sizing, so this is a reasoned number and not a vibe. One `Outbound` is one packet (a `Batch` is
/// one item carrying several), and a writer keeping up with its socket holds this in the single
/// digits — the queue only grows while the socket is refusing bytes. The two costs it sits between:
///   * **Too low → shedding legitimate bursts.** Entering a crowded AOI box queues one
///     `CREATE_OBJECT` per peer in one go (~100 at the 5×5 box's practical occupancy, and every one
///     of them matters — a dropped CREATE means an invisible player until the next re-entry). Login
///     queues its own burst on top. 512 is several times that headroom.
///   * **Too high → the memory path this bounds.** At 371 co-located clients each session receives
///     ~740 peer-motion packets/s (~2 heartbeats/s × ~370 movers), so 512 items ≈ 0.7 s of crowd
///     movement, and at ~50 bytes of body plus `Vec`/enum overhead ≈ 50 KB per session — ~18 MB
///     across 371 sessions, bounded, instead of a queue that grows for as long as the socket stalls.
///
/// So: a session that is more than ~0.7 s behind on peer movement is not "bursting", it is losing,
/// and the newest heartbeat is worth more than the 512 stale ones ahead of it.
pub const EGRESS_SHED_DEPTH: usize = 512;

/// The session's egress sender plus a live count of items **queued but not yet written**.
///
/// The channel stays `std::sync::mpsc` UNBOUNDED on purpose. ~110 of the send sites live in
/// `subscriptions.rs` inside SpacetimeDB SDK `on_insert`/`on_update` callbacks running on the SDK's
/// own pump thread: a blocking send from there (what a bounded channel gives you on a full queue)
/// would stall the pump that feeds every table callback for this connection and deadlock it. So
/// instead of bounding the channel, we MEASURE it and let only the traffic that can safely be lost
/// shed itself — see `subscriptions::shed_motion_at_depth` for which traffic that is and why.
///
/// The counter is the entire new hot-path cost: one `Relaxed` `fetch_add` per enqueue here, one
/// `fetch_sub` per item the writer takes off the queue ([`spawn_writer`]). Nothing is logged,
/// nothing is locked — the standing rule at `subscriptions.rs:208` (a `log::info!` on the relay
/// path made an intermittent failure vanish across six consecutive runs) still holds.
///
/// Every enqueue goes through [`SessionTx::send`], so `depth()` is the true queue length rather
/// than an estimate: the wrapper is what makes that invariant hold without touching ~180 call
/// sites, because `tx.send(..)` resolves to this inherent method with the same signature
/// `Sender::send` had.
#[derive(Clone)]
pub struct SessionTx {
    tx: Sender<Outbound>,
    depth: Arc<AtomicUsize>,
}

impl SessionTx {
    /// Enqueue one outbound unit. Same signature as `mpsc::Sender::send`, so existing call sites
    /// read identically; the only addition is the depth accounting. Incremented BEFORE the send so
    /// the depth can never read low while an item is on the queue, and rolled back when the send
    /// fails (a dead writer half means nothing was queued and nothing will be drained).
    pub fn send(&self, out: Outbound) -> std::result::Result<(), SendError<Outbound>> {
        self.depth.fetch_add(1, Ordering::Relaxed);
        match self.tx.send(out) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.depth.fetch_sub(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Items queued but not yet handed to the socket.
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }

    /// The shared depth counter, for an observer that must NOT keep the channel alive (holding a
    /// `SessionTx` clone would stop the writer's `rx.recv()` from ever returning `Err`).
    ///
    /// Test-only today: the shed path's production observer reads [`SessionTx::depth`] directly, and
    /// the only consumer that needs a detached handle is `world::tests`' fake store.
    #[cfg(test)]
    pub fn depth_handle(&self) -> Arc<AtomicUsize> {
        self.depth.clone()
    }
}

/// One session's egress: the counted sender, the writer's receiver, and the shared depth the writer
/// decrements as it drains. Test seam too — a test can pre-load the depth to drive the shed path.
pub fn session_channel() -> (SessionTx, Receiver<Outbound>, Arc<AtomicUsize>) {
    let (tx, rx) = channel::<Outbound>();
    let depth = Arc::new(AtomicUsize::new(0));
    (
        SessionTx {
            tx,
            depth: depth.clone(),
        },
        rx,
        depth,
    )
}

#[cfg(test)]
impl SessionTx {
    /// Test-only: a sender whose queue already claims `depth` items, for driving the shed path
    /// without queueing half a thousand real packets.
    pub fn with_depth(depth: usize) -> (Self, Receiver<Outbound>) {
        let (tx, rx, d) = session_channel();
        d.store(depth, Ordering::Relaxed);
        (tx, rx)
    }
}

/// A stream that can be split into an independent write handle and shut down to unblock a blocked
/// reader. Implemented for the production `TcpStream` and (test-only) `UnixStream`; both share the
/// underlying fd across clones, so the writer thread can own a clone while the reader keeps the
/// original.
pub trait DuplexStream: Read + Write + Send + Sized + 'static {
    fn try_clone(&self) -> std::io::Result<Self>;
    fn shutdown_both(&self) -> std::io::Result<()>;
}

impl DuplexStream for std::net::TcpStream {
    fn try_clone(&self) -> std::io::Result<Self> {
        std::net::TcpStream::try_clone(self)
    }
    fn shutdown_both(&self) -> std::io::Result<()> {
        self.shutdown(std::net::Shutdown::Both)
    }
}

#[cfg(all(test, unix))]
impl DuplexStream for std::os::unix::net::UnixStream {
    fn try_clone(&self) -> std::io::Result<Self> {
        std::os::unix::net::UnixStream::try_clone(self)
    }
    fn shutdown_both(&self) -> std::io::Result<()> {
        self.shutdown(std::net::Shutdown::Both)
    }
}

/// Shared session material for one account, read from `game_session` (+ `game_account`).
#[derive(Clone, Debug)]
pub struct WorldSession {
    pub account_id: u64,
    /// The 40-byte SRP6 session key K shared with the logon listener via `game_session`.
    pub session_key: [u8; 40],
}

/// How an ITEM-target enchanting cast routes, derived from the spell's effect kind (not a spell id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantRoute {
    /// E_ENCHANT_ITEM — apply this enchant id to the targeted item.
    Enchant(u32),
    /// E_DISENCHANT — disenchant the targeted item into reagents.
    Disenchant,
}

/// Storage/coordination the world handshake needs. Implemented by [`Coordinator`] directly in
/// production (see `stdb::world_store`) and by an in-memory fake in tests.
///
/// Later phases extend this trait (character list, login, movement); Phase 2 needs only the
/// session-key lookup that proves the stateless/shared-K coordination with the logon tier.
/// Per-session movement outcome slot for the non-blocking submit path.
///
/// Fire-and-forget moves the error from the caller's `Result` to a later callback, so two things
/// need somewhere to live: the error itself (applied on the session's next packet, preserving the
/// desync-tolerance behaviour exactly) and the number of submissions still outstanding.
///
/// `in_flight` is what stops fire-and-forget turning a throughput limit into an unbounded-memory
/// one: past `MAX_IN_FLIGHT_MOVES` the session coalesces instead of submitting, which is the same
/// thing the coalescer already does for a fast-moving client — bounded work, newest position wins,
/// no queue to grow.
/// Global submit/complete counts for the non-blocking movement path.
///
/// The gateway forwards ~400 movements/s at 200 players while the module reports ~200
/// `movement_update` transactions/s. These two counters split the difference cleanly:
///   * `submitted == completed == ~400/s` -> the SDK delivered every call and got a response, so
///     the module DID run them and its transaction metric is undercounting.
///   * `completed ~200/s` -> half the calls never completed, so they are being dropped between the
///     gateway's send and the module -- a real loss of player movement.
///
/// Movements submitted in a 10s window below which the relay-health check stays quiet. A
/// handful of movements with no relay traffic is just a lone player with nobody nearby to relay to;
/// hundreds is the broken case. 100 ≈ five moving players.
pub const MOVE_ACTIVITY_FLOOR: u64 = 100;

pub static MOVE_SUBMITTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static MOVE_COMPLETED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Default, Debug)]
pub struct MovementFeedback {
    in_flight: std::sync::atomic::AtomicUsize,
}

/// Outstanding non-blocking movement submissions allowed per session before the next one is
/// coalesced instead. Small on purpose: a vanilla client heartbeats ~2/s, so anything above a
/// couple in flight means the server is already behind and the newest position is the only one that
/// matters.
pub const MAX_IN_FLIGHT_MOVES: usize = 4;

impl MovementFeedback {
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn submitted(&self) {
        self.in_flight
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        MOVE_SUBMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn completed(&self) {
        self.in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        MOVE_COMPLETED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The connection's world-phase sub-state. Encodes the in-world invariant in the TYPE: the relay
/// subscriptions, the combat/loot targets, and the session epoch exist ONLY while in-world, so the
/// dispatch arms match on the state instead of guarding scattered `Option`s. `CMSG_PLAYER_LOGIN`
/// moves `CharSelect → InWorld`; logout / socket teardown moves back, dropping `InWorld` (whose
/// `PlayerSubscriptions` tears its relay callbacks down via RAII).
// `InWorld` is ~400 bytes against `CharSelect`'s zero, which is exactly what
// `clippy::large_enum_variant` flags — deliberately NOT boxed. There is exactly ONE `WorldState`
// per connection (it lives in `WorldConn`, one per session thread), so boxing saves ~400 bytes per
// logged-OUT session and buys an extra pointer hop on every in-world dispatch arm, which is the
// hot path. The memory this enum is sized against is the per-session egress queue, orders of
// magnitude larger (see `EGRESS_SHED_DEPTH`).
#[allow(clippy::large_enum_variant)]
pub enum WorldState {
    /// Character-select: post-handshake, pre-`CMSG_PLAYER_LOGIN`, or after a logout.
    CharSelect,
    /// In the world with a selected character.
    InWorld(InWorld),
}

/// The gossip menu one client is looking at. `options` is indexed by the `gossip_list_id` the client
/// echoes back; past the end is the trailing Farewell line, or a stale click.
pub struct GossipMenuSnapshot {
    pub npc_guid: u64,
    /// `(game_gossip_option.row_id, action)` per menu position. `row_id` is
    /// `codec::SYNTHESIZED_ROW_ID` for the vendor/innkeeper lines the gateway added itself.
    pub options: Vec<(u32, u32)>,
}

/// State that exists only while the player is in the world (the `InWorld` variant payload).
pub struct InWorld {
    /// The selected character's guid (names `SMSG_ATTACKSTART`/`SMSG_ATTACKSTOP`).
    pub self_guid: u64,
    /// RLS-scoped relay subscriptions; their RAII `Drop` removes the callbacks when this is dropped
    /// (on logout or socket teardown), so a relogin can't double-register them.
    pub subs: PlayerSubscriptions,
    /// In-world session epoch for the two-connection race arbitration (see `SessionEpochs`).
    pub session_epoch: u64,
    /// The guid being melee auto-attacked (combat C1), so `CMSG_ATTACKSTOP` can name it. The
    /// authoritative engagement lives in `game_melee_attack`; this is protocol state.
    pub attacking_target: Option<u64>,
    /// The corpse guid with an open loot window (slice 3; `CMSG_LOOT_MONEY` carries no guid).
    pub looting_target: Option<u64>,
    /// A ranged auto-repeat (Auto Shot / wand Shoot) is armed. Melee and ranged share one
    /// `game_melee_attack` row keyed by attacker, so the melee-stop `CMSG_ATTACKSTOP` the client
    /// sends when switching to ranged would collaterally kill the auto-shot. While this is set,
    /// `CMSG_ATTACKSTOP` is ignored — the ranged loop is stopped only by
    /// `CMSG_CANCEL_AUTO_REPEAT_SPELL`.
    pub ranged_repeat: bool,
}

/// Established per-connection protocol state after a successful handshake, owned by the reader
/// thread. The `EncrypterHalf` is NOT here — it moves to the single writer thread, which is the
/// sole writer of the socket (the header cipher is a stateful stream, so exactly one writer may
/// advance it). NOT game state: the cipher is re-derivable from K on reconnect.
pub struct WorldConn {
    pub account_id: u64,
    /// Decrypts inbound CMSG headers (reader-thread only).
    pub decrypt: DecrypterHalf,
    /// Character-select vs in-world; the relay subs + combat/loot/session state live in `InWorld`.
    pub state: WorldState,
    /// Per-connection movement coalescing. Lives directly on `WorldConn` (not
    /// `InWorld`) so it survives a world-port/relogin boundary uneventfully — it only ever sees
    /// traffic while `InWorld` movement is being dispatched, but there's no correctness reason to
    /// tear it down on a state transition, and keeping it here avoids re-plumbing it through
    /// `enter_world`.
    move_coalesce: CoalesceState,
    /// The gossip menu last sent to this client. A select carries only a position into a
    /// condition-filtered list, so re-deriving that list at click time renumbers it under a quest
    /// accepted while the window was open.
    pub(crate) gossip_menu: Option<GossipMenuSnapshot>,
    /// Non-blocking movement submission/backpressure state. Shared batches have no per-entry
    /// reducer verdict, so entity presence drives the desync policy before submission.
    move_feedback: std::sync::Arc<MovementFeedback>,
    /// Movement packets dropped because `MAX_IN_FLIGHT_MOVES` was already outstanding. Logged at
    /// teardown next to the coalescing ratio — if this is large the server is behind, and it is the
    /// number that says so rather than a silent stall.
    move_submit_dropped: u64,
    /// Multi-shard routing: the HOME-shard store handle for the character this session is
    /// playing, resolved at `CMSG_PLAYER_LOGIN` (and re-resolved on a world-port, which can change
    /// map and therefore shard) from the character's location via `WorldStore::home_shard`. `None`
    /// = "the handle the listener already handed us is the right one" — the single-shard default,
    /// and the state of every pre-login message, so nothing about the old path changes.
    ///
    /// This is derived, not authoritative: it is recomputed from durable state on every world
    /// entry, so a gateway restart mid-session re-derives it on the client's relog — the
    /// gateway stays stateless w.r.t. sessions.
    home: Option<std::sync::Arc<dyn WorldStore>>,
    /// The shared session key K, kept from the world handshake so a session that lands on a NON-
    /// realm shard can call `establish_session` THERE — the module's `account_by_identity`
    /// gate on `player_login` needs the account row on that database bound to this connection's
    /// identity, and the logon tier only ever bound it on the realm database.
    ///
    /// NOT game state, and not a new secret: it is the same K the header cipher on this very socket
    /// is already keyed by, read from `game_session` moments earlier in `world_handshake`. `None`
    /// for sessions built by tests that never handshake.
    session_key: Option<[u8; 40]>,
    /// Consecutive movement packets dropped because the coordinator cache has no live entity.
    /// Reset by the first movement whose entity is present. See
    /// [`MOVE_DESYNC_TOLERANCE`] for why the tolerance is bounded rather than unconditional.
    move_desync_drops: u32,
}

/// How many CONSECUTIVE desynced movement packets a session may drop before the desync is treated
/// as real and the session ends.
///
/// The tail of a cross-map port is a handful of packets: the client stops sending movement the
/// instant `SMSG_TRANSFER_PENDING` puts it on the loading screen, so only what was already in
/// flight (hundreds of ms at ~10 packets/s) can land on the despawned entity. A desync that keeps
/// answering past that is the OTHER kind — the entity is gone for good (a schema-change publish tore
/// down the coordinator subscription; the row was deleted under this socket) — and for that one,
/// closing the socket IS the fix: the client shows "Disconnected" and re-materialises everything
/// from durable state on relog. Swallowing it forever would trade the loading-screen hang for a
/// worse one: a player walking around a frozen world, invisible to peers, never disconnected, with
/// no error and no recourse — which is exactly what `is_desync_error`'s session-fatal treatment
/// exists to prevent. 32 is ~3 s of a moving client, an order of magnitude more than any port tail.
const MOVE_DESYNC_TOLERANCE: u32 = 32;

/// Run `$body` against the session's HOME-shard store handle. `$store` is the handle the
/// caller holds (the default/realm shard); `$conn.home` overrides it once the player is in the
/// world. Two arms rather than a coercion because `$store` is a `?Sized` generic and cannot be
/// unsize-coerced to `&dyn WorldStore`; only one arm ever runs.
// Deliberate simplification: a macro instead of threading a routed handle through ~15 handler
// signatures. Ceiling — it only works where `conn` is in scope. Upgrade path: if routing ever
// needs to change MID-session, fold the handle into a session context struct instead.
macro_rules! on_home_shard {
    ($conn:expr, $store:expr, |$s:ident| $body:expr) => {{
        let home = $conn.home.clone();
        match home {
            Some(h) => {
                let $s: &dyn WorldStore = h.as_ref();
                $body
            }
            None => {
                let $s = $store;
                $body
            }
        }
    }};
}
pub(crate) use on_home_shard;

impl WorldConn {
    /// Re-pin this session to the shard that owns `character_guid`'s location. Asked of the
    /// handle currently in use, so a `None` answer means "this handle already owns it" and the
    /// current pin is KEPT — clearing it would silently drop the session back to the default shard.
    /// Called at every world entry (login and world-port), never in between: routing is derived
    /// from durable state, so nothing here survives a gateway restart that shouldn't.
    /// Returns the transfer's error rather than swallowing it: a half-moved character must
    /// fail its login loudly, never proceed into the world on whichever shard happened to answer.
    /// Both are recoverable — the escrow holds and the next login re-drives it — but only one of
    /// them is honest to the player.
    fn route_home<St: WorldStore + ?Sized>(
        &mut self,
        store: &St,
        character_guid: u64,
    ) -> Result<()> {
        // `settle_home_shard`, not `home_shard`: for a session that actually crosses a database
        // boundary this RUNS the escrowed transfer before answering. Its default impl IS
        // `home_shard`, so the single-shard path and every mock are unchanged.
        let resolved = on_home_shard!(self, store, |st| st.settle_home_shard(character_guid))?;
        if let Some(h) = resolved {
            log::info!(
                "world: account {} (guid {character_guid}) routed to shard {}",
                self.account_id,
                h.shard_name()
            );
            self.home = Some(h);
        }
        // Bind this account's identity on the shard we ended up on. On the realm shard the logon
        // tier already did it; on an instance shard the character arrived through
        // `import_character_blob`, which created only a SHADOW account row with no identity — and
        // `world::player_login` resolves its caller through `account_by_identity`, so without this
        // the arriving player cannot log in at all. Idempotent, and re-run on every world entry
        // because the per-player connection mints a fresh identity after any gateway restart.
        if let (Some(h), Some(key)) = (&self.home, &self.session_key) {
            h.bind_shard_session(self.account_id, key)?;
        }
        Ok(())
    }

    /// Leave the world: `InWorld → CharSelect`, dropping the relay subs (stops the peer callbacks;
    /// observers get `DESTROY` via the entity delete) and deleting the entity ONLY if THIS session
    /// still owns it. A stale socket whose player already re-logged on a newer session declines the
    /// `release_session` gate, so we don't vanish the live player (the cached PlayerConn shares one
    /// identity — only the gateway can tell the sockets apart). Returns the `logout` result so
    /// each call site keeps its own error policy: the socket-teardown path logs + swallows it (it is
    /// already ending), while the graceful-logout arm propagates it (session-fatal, as before).
    /// A no-op (`Ok`) when already in `CharSelect`. Call sites that ack the client (graceful logout)
    /// send their SMSG batch BEFORE calling this.
    fn leave_world<St: WorldStore + ?Sized>(&mut self, store: &St) -> Result<()> {
        if let WorldState::InWorld(InWorld {
            subs,
            session_epoch,
            self_guid,
            ..
        }) = std::mem::replace(&mut self.state, WorldState::CharSelect)
        {
            drop(subs);
            let account_id = self.account_id;
            // The `logout` reducer must delete the entity on the shard it LIVES on, so this
            // runs on the home shard like every other player-scoped call. Session epochs are
            // gateway-local and shared across shards, so the same-identity arbitration above is unaffected.
            let outcome = on_home_shard!(self, store, |st| {
                if st.release_session(account_id, session_epoch) {
                    st.logout(account_id, self_guid)
                } else {
                    log::debug!(
                        "world: skipping stale logout for account {account_id} \
                         (superseded by a newer session)"
                    );
                    Ok(())
                }
            });
            // RELEASE the home-shard pin: the socket stays open at character select, and
            // everything served there — char enum/create/delete — is REALM-scoped (`game_account` /
            // `game_character` live on the default database). A pin left over from the character we
            // just logged out of would serve the character list off the instance shard, which is
            // empty: the player would see no characters at all, and a create/delete would write to
            // the wrong database. Cleared even when `logout` failed — the state transition above
            // already happened, so the session is at character select either way.
            self.home = None;
            outcome?;
        }
        Ok(())
    }
}

/// How often a queued connection re-checks whether it has been admitted. Cheap — one mutex
/// lock, no I/O — so this can be short without costing anything measurable even under a full queue.
const QUEUE_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How often a queued connection actually WRITES a refreshed `AUTH_WAIT_QUEUE` to the wire (as
/// opposed to just checking, above). Bounded below by the spec's "every 5-10s or on position
/// change" — resending on every poll tick would spam the socket for no client-visible benefit,
/// since the position number is the only thing that can change. This is ALSO how promptly a clean
/// disconnect while queued is noticed (the write attempt is what surfaces the broken pipe), so the
/// test build uses a much shorter interval — real production behavior is unaffected either way,
/// since a queued connection holds no seat and wastes nothing by lingering briefly after a hangup.
#[cfg(not(test))]
const QUEUE_RESEND_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const QUEUE_RESEND_INTERVAL: Duration = Duration::from_millis(50);

/// Run the world auth handshake over `stream` (gateway-translation.md §3), gated by the unlimited
/// [`LoginQueue`] (`LoginQueue::unlimited()`) — see [`world_handshake_with_queue`] for the
/// admission gate itself. On success returns the established per-connection crypto + account id; on
/// a clean auth failure sends the failure response and returns `Ok(None)`; on a clean disconnect
/// also returns `Ok(None)`.
/// Test-only: production goes through [`world_handshake_with_queue`] (the listener in [`run`] owns
/// the process-wide [`LoginQueue`]), so an unqueued handshake exists purely for tests.
#[cfg(test)]
pub fn world_handshake<S: Read + Write, St: WorldStore + ?Sized>(
    stream: &mut S,
    store: &St,
) -> Result<Option<(WorldConn, EncrypterHalf)>> {
    world_handshake_with_queue(stream, store, &LoginQueue::unlimited())
}

/// Run the world auth handshake over `stream` (gateway-translation.md §3), gated by `queue`.
/// On success returns the established per-connection crypto + account id; on a clean auth failure
/// sends the failure response and returns `Ok(None)`; on a clean disconnect also returns `Ok(None)`.
///
/// Sequence: plaintext `SMSG_AUTH_CHALLENGE` (server seed) -> read plaintext
/// `CMSG_AUTH_SESSION` -> look up K -> validate the client proof digest via `wow_srp` (which
/// also yields the header cipher) -> **admission gate** (queue here if the world is full,
/// resending `AUTH_WAIT_QUEUE` with the caller's position until a seat opens) -> **encrypted**
/// `SMSG_AUTH_RESPONSE` (AUTH_OK). From the AUTH_OK packet on, every header is encrypted — and so is
/// every `AUTH_WAIT_QUEUE` resend, since the cipher already exists by the time the gate is consulted.
///
/// A seat taken from `queue` is held for the lifetime of the returned session; the caller (normally
/// [`run_world_session_with_queue`]) MUST call `queue.depart()` exactly once when that session ends.
/// The one failure mode entirely inside this function — admitted, then the final `AUTH_OK` write
/// itself fails (the client vanished in the instant between admission and delivery) — departs its
/// own seat before propagating the error, so a caller that never sees `Ok(Some(..))` never owes a
/// `depart()` call.
pub fn world_handshake_with_queue<S: Read + Write, St: WorldStore + ?Sized>(
    stream: &mut S,
    store: &St,
    queue: &LoginQueue,
) -> Result<Option<(WorldConn, EncrypterHalf)>> {
    // 1. Plaintext SMSG_AUTH_CHALLENGE with a fresh, single-use server seed (gateway-local
    //    RNG — protocol state, not game state).
    let seed = ProofSeed::new();
    let server_seed = seed.seed();
    SMSG_AUTH_CHALLENGE { server_seed }.write_unencrypted_server(&mut *stream)?;

    // 2. Read the plaintext CMSG_AUTH_SESSION (account, client_seed, client_proof, addons).
    let session = match ClientOpcodeMessage::read_unencrypted(&mut *stream) {
        Ok(ClientOpcodeMessage::CMSG_AUTH_SESSION(s)) => s,
        Ok(other) => return Err(anyhow!("expected CMSG_AUTH_SESSION, got {other}")),
        // Clean client disconnect before sending the session.
        Err(ExpectedOpcodeError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(e) => return Err(anyhow!("world auth read error: {e}")),
    };

    // 3. Map account -> K (read from game_session via the privileged coordinator connection).
    let username = session.username.to_uppercase();
    let Some(WorldSession {
        account_id,
        session_key,
    }) = store.lookup_session(&username)?
    else {
        // No session for this account: the client never authenticated (or it expired). No
        // cipher yet, so the rejection goes out plaintext.
        SMSG_AUTH_RESPONSE::AuthUnknownAccount.write_unencrypted_server(&mut *stream)?;
        return Ok(None);
    };

    // 4. Validate the client proof digest against K. Success also constructs the header
    //    cipher keyed by K (a mismatch == the client does not actually hold K -> reject).
    let ns = NormalizedString::new(&username).map_err(|e| anyhow!("bad username: {e}"))?;
    let crypto = match seed.into_server_header_crypto(
        &ns,
        session_key,
        session.client_proof,
        session.client_seed,
    ) {
        Ok(c) => c,
        Err(_) => {
            // Bad digest. No cipher established, so the failure goes out plaintext.
            SMSG_AUTH_RESPONSE::AuthFailed.write_unencrypted_server(&mut *stream)?;
            return Ok(None);
        }
    };
    let (mut encrypt, decrypt) = crypto.split();

    // 4.5. Admission gate. `queue.request()` on an unlimited queue (the default —
    // `LYRACORE_MAX_SESSIONS` unset) returns `Admitted` immediately with no bookkeeping: a no-op for
    // every gateway that hasn't opted in. Otherwise a full house queues the connection here, BEFORE
    // it can reach CMSG_CHAR_ENUM or CMSG_PLAYER_LOGIN's `subscribe_player_events` — the expensive
    // part measured at 38% of the writer at 500 sessions.
    if let Admission::Queued(ticket) = queue.request() {
        if !wait_for_admission(&mut *stream, &mut encrypt, queue, ticket)? {
            // Clean disconnect while queued: leave the line (no seat was ever taken) and end the
            // session exactly like the pre-auth disconnect path above — no AUTH_OK is ever sent.
            queue.cancel(ticket);
            return Ok(None);
        }
    }

    // 5. SMSG_AUTH_RESPONSE = AUTH_OK + zeroed billing. From here on every header is encrypted.
    if let Err(e) = (SMSG_AUTH_RESPONSE::AuthOk {
        billing_flags: 0,
        billing_rested: 0,
        billing_time: 0,
    })
    .write_encrypted_server(&mut *stream, &mut encrypt)
    {
        // A seat was granted (immediately above, or via the queue) but the client vanished in the
        // instant before we could tell it so. Release the seat here — we're about to return `Err`
        // instead of `Ok(Some(..))`, so no caller will ever see this session to `depart()` it.
        queue.depart();
        return Err(anyhow!("world auth write error (AUTH_OK): {e}"));
    }

    Ok(Some((
        WorldConn {
            account_id,
            decrypt,
            state: WorldState::CharSelect,
            move_coalesce: CoalesceState::default(),
            gossip_menu: None,
            move_feedback: std::sync::Arc::new(MovementFeedback::default()),
            move_submit_dropped: 0,
            home: None,                     // resolved at CMSG_PLAYER_LOGIN
            session_key: Some(session_key), // for establish_session on a non-realm shard
            move_desync_drops: 0,
        },
        encrypt,
    )))
}

/// Block until `ticket` is admitted, resending `AUTH_WAIT_QUEUE` (with the current position) on
/// [`QUEUE_RESEND_INTERVAL`] — immediately on the first check, and again sooner than that cadence if
/// the position changes, so the client's queue screen doesn't sit on a stale number. Polls for
/// admission itself on the much shorter [`QUEUE_POLL_INTERVAL`], since that's a cheap mutex lock
/// with no I/O. Returns `Ok(true)` once admitted, `Ok(false)` on a clean disconnect (the write
/// failed — the client hung up while queued, so the caller should `cancel` rather than treat this as
/// session-fatal), `Err` on any other I/O error.
fn wait_for_admission<S: Write>(
    stream: &mut S,
    encrypt: &mut EncrypterHalf,
    queue: &LoginQueue,
    ticket: login_queue::Ticket,
) -> Result<bool> {
    let mut last_sent: Option<(Instant, usize)> = None;
    loop {
        let Some(position) = queue.position(ticket) else {
            return Ok(true);
        };
        let due = match last_sent {
            None => true,
            Some((_, sent_position)) if sent_position != position => true,
            Some((at, _)) => at.elapsed() >= QUEUE_RESEND_INTERVAL,
        };
        if due {
            let sent = (SMSG_AUTH_RESPONSE::AuthWaitQueue {
                queue_position: position as u32,
            })
            .write_encrypted_server(&mut *stream, encrypt);
            if let Err(e) = sent {
                log::debug!("world: queued connection hung up at position {position}: {e}");
                return Ok(false);
            }
            last_sent = Some((Instant::now(), position));
        }
        std::thread::sleep(QUEUE_POLL_INTERVAL);
    }
}

/// Corruption-diagnosis probe: cheap FNV-1a-64 over a byte slice — not cryptographic, just enough to tell "same
/// bytes" from "different bytes" across two independently-captured rings (this one and
/// the wire harness's) without paying for anything stronger than a multiply-xor per byte.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// One traced frame: the wire-visible `(opcode, declared size)` pair plus a checksum of the body
/// bytes — the same triple the wire harness's own crash ring reports (opcode,
/// declared_body_len, actual_body_len), plus the one field that ring can't have (it only ever
/// sees bytes after the client's decoder already trusts them): whether the CONTENT matches.
#[derive(Clone, Copy)]
struct WriterTraceEntry {
    opcode: u16,
    size: u16,
    checksum: u64,
}

/// Corruption-diagnosis probe: per-session black box for [`spawn_writer`] — the last [`WriterTrace::CAPACITY`]
/// frames handed to the socket, gated behind `LYRACORE_WRITER_TRACE=1` (see
/// `config::writer_trace_enabled`). Recorded from the CONSTRUCTED frame just before the write
/// (unencrypted representation for the typed `One`/`Batch` arms — cheap only because tracing is
/// off by default — matched byte-for-byte against `Outbound::Raw`'s own `(opcode, body)`, since
/// `Raw` never touches gtker's codec at all), not a tee of the physical socket call: a *partial*
/// write cannot silently masquerade as success here (`write_all` is complete-or-`Err` on a
/// blocking socket, and this connection's writer treats every `Err` as session-fatal — see the
/// dump call site), so recording pre-write and recording post-write carry the same information
/// for every frame that reaches this ring. What this DOES catch: the gateway constructing wrong
/// bytes for a frame (a content/data bug) — diff this ring's `(opcode, size)` sequence against
/// the harness's `(opcode, declared_body_len)` sequence at the corruption offset: if they still
/// agree past the last frame this ring saw, the corruption is downstream of the writer.
struct WriterTrace {
    ring: std::collections::VecDeque<WriterTraceEntry>,
}

impl WriterTrace {
    const CAPACITY: usize = 32;

    fn new() -> Self {
        Self {
            ring: std::collections::VecDeque::with_capacity(Self::CAPACITY),
        }
    }

    fn push(&mut self, opcode: u16, size: u16, checksum: u64) {
        if self.ring.len() == Self::CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(WriterTraceEntry {
            opcode,
            size,
            checksum,
        });
    }

    /// Record one [`Outbound`] just before it is handed to the writer's match arm. `Batch`
    /// expands to one entry per contained message — each is its own frame on the wire.
    fn record(&mut self, out: &Outbound) {
        match out {
            Outbound::One(m) => self.record_typed(m),
            Outbound::Batch(v) => {
                for m in v {
                    self.record_typed(m);
                }
            }
            Outbound::Raw { opcode, body } => {
                self.push(*opcode, (2 + body.len()) as u16, fnv1a64(body));
            }
            // Jobs are expanded before the trace sees them (`spawn_writer`), so this arm is
            // unreachable for a real frame; it exists so a future caller cannot silently skip the
            // ring by wrapping a packet in a job.
            Outbound::Job(_) => {}
        }
    }

    /// `write_unencrypted_server` is the exact plaintext framing gtker's `write_encrypted_server`
    /// builds internally before encrypting only the 4 header bytes (traits/vanilla.rs) — so this
    /// reproduces `[size_be:2][opcode_le:2][body]` without needing to invert the header cipher,
    /// at the cost of one extra serialization pass. That cost is real but exists ONLY behind
    /// `LYRACORE_WRITER_TRACE=1` — see its doc comment for why production pays nothing for this.
    fn record_typed(&mut self, m: &ServerOpcodeMessage) {
        let mut buf = Vec::new();
        if m.write_unencrypted_server(&mut buf).is_err() || buf.len() < 4 {
            return; // can't happen on a real message; never worth crashing the trace over
        }
        let size = u16::from_be_bytes([buf[0], buf[1]]);
        let opcode = u16::from_le_bytes([buf[2], buf[3]]);
        self.push(opcode, size, fnv1a64(&buf[4..]));
    }

    /// Dump the ring to `/tmp/gw-writer-crash/<account_id>.txt` on abnormal session end (a write
    /// error — the only ending `spawn_writer` treats as abnormal; a clean disconnect drops `tx`
    /// and the loop just ends via `rx.recv()` returning `Err`, which calls nothing here). Mirrors
    /// the wire harness's own crash-dump shape so the two files sit next to each other for a
    /// diff. Best-effort: a dump failing must never be why the writer thread panics.
    fn dump(&self, account_id: u64, reason: &str) {
        let dir = std::path::Path::new("/tmp/gw-writer-crash");
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("writer trace: could not create {}: {e}", dir.display());
            return;
        }
        let path = dir.join(format!("{account_id}.txt"));
        let mut out = format!(
            "account: {account_id}\nended: {reason}\nlast {} traced frames (oldest first):\n",
            self.ring.len()
        );
        for e in &self.ring {
            out.push_str(&format!(
                "  opcode=0x{:04X} size={} checksum=0x{:016x}\n",
                e.opcode, e.size, e.checksum
            ));
        }
        if let Err(e) = std::fs::write(&path, out) {
            log::warn!("writer trace: could not write {}: {e}", path.display());
        } else {
            log::info!(
                "writer trace: dumped {} ({} frames)",
                path.display(),
                self.ring.len()
            );
        }
    }
}

/// The single writer thread: the SOLE owner of the `EncrypterHalf` and a write-clone of the
/// socket. It drains the channel and writes each `Outbound` in dequeue order, so the header
/// cipher's keystream advances deterministically and no two writers ever interleave. On a write
/// error it shuts the socket down to unblock the reader's blocking read.
///
/// `account_id` exists ONLY to name the `LYRACORE_WRITER_TRACE=1` crash dump — every other
/// behavior here is byte-for-byte what it was before that flag existed.
///
/// Returns `Err` (rather than panicking) when the OS refuses the thread: at the session counts this
/// gateway is built for, thread/FD exhaustion is a REACHABLE operational state, and the honest
/// response is to fail THIS connection — the caller `?`s it exactly like the socket clone above it,
/// so the session closes cleanly and its queue seat is released — instead of unwinding a session
/// thread and taking whatever it holds with it.
fn spawn_writer<S: DuplexStream>(
    mut wsock: S,
    mut encrypt: EncrypterHalf,
    rx: Receiver<Outbound>,
    depth: Arc<AtomicUsize>,
    account_id: u64,
) -> Result<std::thread::JoinHandle<()>> {
    let trace_enabled = crate::config::writer_trace_enabled();
    std::thread::Builder::new()
        .name("world-writer".into())
        .spawn(move || {
            let mut trace = trace_enabled.then(WriterTrace::new);
            while let Ok(out) = rx.recv() {
                // The other half of the depth counter (`SessionTx::send` adds): this item is off the
                // queue, so it no longer counts against `EGRESS_SHED_DEPTH` — decrement BEFORE the
                // (possibly slow, possibly blocking) write, so a session stuck inside one `write_all`
                // is not charged for the packet it is actively pushing.
                //
                // Cannot underflow, and that matters because a wrapped `usize` would shed every
                // peer-motion packet for the rest of the session: the increment happens strictly
                // BEFORE `tx.send`, so an item can only be received after it has been counted, and
                // this thread is the only decrementer. `depth >= queued` therefore holds always.
                depth.fetch_sub(1, Ordering::Relaxed);
                // A deferred relay job runs HERE, on this session's own writer thread, never
                // on the shared coordinator pump that enqueued it. `catch_unwind` because a
                // panicking relay body used to cost one session and must keep costing exactly one
                // session: the writer stays alive and keeps draining the queue.
                let batch: Vec<Outbound> = match out {
                    Outbound::Job(job) => {
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
                            Ok(items) => items,
                            Err(_) => {
                                log::error!(
                                    "world writer: a deferred relay job PANICKED for account \
                                     {account_id} — contained (this session keeps running), but \
                                     the packet(s) it would have produced were lost"
                                );
                                Vec::new()
                            }
                        }
                    }
                    other => vec![other],
                };
                for out in batch {
                if let Some(t) = trace.as_mut() {
                    t.record(&out);
                }
                let res = match out {
                    Outbound::One(m) => {
                        log::debug!("OUT {m}"); // SMSG variant name → crash attribution (tail 'OUT SMSG_*')
                        m.write_encrypted_server(&mut wsock, &mut encrypt)
                    }
                    Outbound::Batch(v) => v.into_iter().try_for_each(|m| {
                        log::debug!("OUT {m}");
                        m.write_encrypted_server(&mut wsock, &mut encrypt)
                    }),
                    // Expanded above; a job can never reach here.
                    Outbound::Job(_) => Ok(()),
                    Outbound::Raw { opcode, body } => {
                        log::debug!("OUT RAW opcode=0x{opcode:04X} ({} body bytes)", body.len());
                        // Packet lint (testing-hardening §3.2): every hand-rolled body is checked
                        // against the root-caused 5875 crash classes; a hit is a loud ERROR the
                        // suite greps for — never a drop (the frame still sends: prod behavior is
                        // observability, the WALL is the suite failing on the grep).
                        for v in packet_lint::lint_raw(opcode, &body) {
                            log::error!("packet-lint VIOLATION opcode=0x{opcode:04X}: {v}");
                        }
                        // Same framing gtker's write_encrypted_server emits: a `size` of
                        // opcode(2) + body, big-endian, then the LE opcode, the 4 bytes encrypted
                        // through the SAME header cipher (so the stream never desyncs), then the body.
                        // size counts opcode(2) + body, NOT the 2 size bytes — matches gtker's
                        // server framing.
                        //
                        // Hardening: a body that overflows the u16 size field would
                        // silently WRAP via `as u16` — the header would then declare a small size
                        // while `write_all` below still sends the full (large) body, so the client's
                        // reader consumes the wrong number of bytes for this frame and every LATER
                        // header on this connection decrypts from the wrong stream offset (a real
                        // desync, indistinguishable from the crowd-scale corruption class investigated
                        // earlier). This used
                        // to be a `debug_assert!`, which compiles out entirely in the release profile
                        // the capacity benchmark and any live deploy actually run — i.e. the one guard
                        // against this class was absent exactly where it would matter. No current
                        // builder produces a body anywhere near this size, so this arm is not known to
                        // fire; it exists so that if one ever does, the connection dies cleanly
                        // instead of corrupting itself and every packet after it.
                        if body.len() > u16::MAX as usize - 2 {
                            log::error!(
                                "packet-lint VIOLATION opcode=0x{opcode:04X}: raw body is {} bytes — \
                                 exceeds the u16 frame-size field and would silently wrap (a stream-\
                                 desync class); refusing to send and ending the session instead of \
                                 corrupting the stream",
                                body.len()
                            );
                            Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "raw outbound body exceeds u16 frame size",
                            ))
                        } else {
                            let size = (2 + body.len()) as u16;
                            encrypt
                                .write_encrypted_server_header(&mut wsock, size, opcode)
                                .and_then(|()| wsock.write_all(&body))
                        }
                    }
                };
                if let Err(e) = res {
                    log::debug!("world writer stopping: {e}");
                    if let Some(t) = trace.as_ref() {
                        t.dump(account_id, &e.to_string());
                    }
                    let _ = wsock.shutdown_both();
                    return;
                }
                }
            }
        })
        .map_err(|e| anyhow!("spawn world writer thread for account {account_id}: {e}"))
}

/// Drive one world connection, gated by the unlimited [`LoginQueue`] — see
/// [`run_world_session_with_queue`] for the admission gate itself. Every existing call site
/// (tests, and anything that doesn't care about it) stays byte-identical to the pre-queue behavior.
/// Test-only, for the same reason as [`world_handshake`]: production drives
/// [`run_world_session_with_queue`] with the listener's shared queue.
#[cfg(test)]
pub fn run_world_session<S: DuplexStream, St: WorldStore + ?Sized>(
    stream: S,
    store: &St,
) -> Result<()> {
    run_world_session_with_queue(stream, store, &LoginQueue::unlimited())
}

/// A successful handshake owns one seat until its writer has stopped.
struct AdmissionSeat<'a> {
    queue: &'a LoginQueue,
}

impl Drop for AdmissionSeat<'_> {
    fn drop(&mut self) {
        self.queue.depart();
    }
}

/// Drive one world connection: handshake (synchronous, sole writer), then split into a reader
/// (this thread, owns `DecrypterHalf`) and a writer thread (owns `EncrypterHalf` + a socket clone)
/// bridged by an mpsc channel. The reader handles requests and pushes responses; the per-player
/// subscription callbacks (on the SDK pump thread) push peer spawn/move/destroy. On disconnect the
/// reader tears down the subscriptions, logs the player out (removes the entity → peers see
/// `DESTROY`), drops its sender (ending the writer), and joins it.
///
/// `world_handshake_with_queue` may block here (before any of the above starts) if the world
/// is full — the client sees native "Position in queue" screens and this thread just waits, which
/// costs nothing but ITS OWN thread (the listener in `run` spawns one `spawn_blocking` task per
/// accepted socket, so a queued connection never holds up anyone else's accept or handshake). Once
/// admitted (`Ok(Some(..))`), this connection holds a seat in `queue` for the rest of the function —
/// released by its [`AdmissionSeat`] exactly once, no matter which branch got there.
pub fn run_world_session_with_queue<S: DuplexStream, St: WorldStore + ?Sized>(
    mut stream: S,
    store: &St,
    queue: &LoginQueue,
) -> Result<()> {
    let Some((mut conn, encrypt)) = world_handshake_with_queue(&mut stream, store, queue)? else {
        return Ok(());
    };
    let seat = AdmissionSeat { queue };

    let wsock = stream
        .try_clone()
        .map_err(|e| anyhow!("clone world socket for writer: {e}"))?;
    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(wsock, encrypt, rx, depth, conn.account_id)?;

    let result = (|| -> Result<()> {
        // Frames are read RAW (header hand-decrypted) so an addon-language chat — which
        // gtker's `Language` enum cannot decode and which was session-FATAL — can be peeked and
        // routed to the bridge BEFORE typed parsing. Every other frame is re-framed unencrypted
        // and handed to the same gtker parser as before (one memcpy per packet).
        // (`std::io::Read` is already in scope from the file-level import — the raw reads below use
        // it; a second local `use` is a duplicate, not a requirement.)
        // Addon-bridge rate limit: token bucket per connection — 2 tokens/s, burst 20.
        // Excess frames drop with one log line per offense window; module handlers stay
        // unthrottled (they trust this edge like every other opcode).
        let mut addon_tokens: f32 = 20.0;
        let mut addon_refill_at = std::time::Instant::now();
        let mut addon_drop_logged_at: Option<std::time::Instant> = None;
        loop {
            let hdr = match conn.decrypt.read_and_decrypt_client_header(&mut stream) {
                Ok(h) => h,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(anyhow!("world read error: {e}")),
            };
            // `hdr.size` counts the u32 opcode + body.
            let body_len = (hdr.size as usize).saturating_sub(4);
            let mut body = vec![0u8; body_len];
            stream
                .read_exact(&mut body)
                .map_err(|e| anyhow!("world read error (body): {e}"))?;
            if hdr.opcode == codec::addon::CMSG_MESSAGECHAT_OPCODE {
                if let Some(text) = codec::addon::parse_addon_client_chat(&body) {
                    let now = std::time::Instant::now();
                    addon_tokens = (addon_tokens
                        + now.duration_since(addon_refill_at).as_secs_f32() * 2.0)
                        .min(20.0);
                    addon_refill_at = now;
                    if addon_tokens >= 1.0 {
                        addon_tokens -= 1.0;
                        handle_addon_message(store, &conn, &text);
                    } else if addon_drop_logged_at
                        .is_none_or(|t| now.duration_since(t).as_secs() >= 60)
                    {
                        addon_drop_logged_at = Some(now);
                        log::warn!(
                            "addon bridge: rate limit — dropping frames from account {}",
                            conn.account_id
                        );
                    }
                    continue; // addon frames NEVER reach the normal chat path or other players
                }
            }
            let mut framed = Vec::with_capacity(6 + body.len());
            framed.extend_from_slice(&hdr.size.to_be_bytes());
            framed.extend_from_slice(&hdr.opcode.to_le_bytes());
            framed.extend_from_slice(&body);
            let msg = match ClientOpcodeMessage::read_unencrypted(&mut std::io::Cursor::new(framed))
            {
                Ok(m) => m,
                Err(e) => return Err(anyhow!("world read error: {e}")),
            };
            // Every in-world message is dispatched against the player's HOME shard; until
            // CMSG_PLAYER_LOGIN resolves one (and always, with a single-entry shard map) this is
            // the handle the listener passed in — the unchanged single-database path.
            on_home_shard!(conn, store, |st| dispatch(&tx, st, &mut conn, msg))?;
        }
    })();

    // Movement-coalescing measurement: how much did coalescing actually cut? Logged once per
    // connection at disconnect, not per-packet — a live run's tail shows the ratio directly.
    log::debug!(
        "world: movement coalescing account={} received={} forwarded={} submit_dropped={}",
        conn.account_id,
        conn.move_coalesce.received(),
        conn.move_coalesce.forwarded(),
        conn.move_submit_dropped,
    );

    // Teardown: if still in-world, drop the relay subs (stops callbacks; peers get DESTROY via the
    // entity delete) and delete the entity ONLY if THIS session still owns it — a stale socket whose
    // player already re-logged on a newer session declines, so we don't vanish the live player.
    // Teardown is already ending the session, so a `logout` failure is logged + swallowed (not fatal).
    if let Err(e) = conn.leave_world(store) {
        log::warn!("logout for account {} failed: {e:#}", conn.account_id);
    }
    drop(tx);
    let _ = writer.join();
    // The seat covers relay shutdown and releases exactly once after the writer stops.
    drop(seat);
    result
}

/// Route one addon-language chat frame: parse the `STC` v1 envelope and forward to the
/// module's `client_command` reducer as the player. Foreign prefixes and malformed envelopes
/// drop silently-with-a-debug-line (other servers' addons share the airwaves by accident);
/// reducer errors log and drop — an addon frame is NEVER session-fatal.
fn handle_addon_message<St: WorldStore + ?Sized>(store: &St, conn: &WorldConn, text: &str) {
    let Some((cmd, payload)) = codec::addon::parse_bridge_envelope(text) else {
        log::debug!("addon bridge: non-STC or malformed frame dropped: {text:?}");
        return;
    };
    if let Err(e) = store.client_command(
        conn.account_id,
        social::self_guid(conn).unwrap_or(0),
        cmd.clone(),
        payload,
    ) {
        log::info!(
            "addon bridge: command {cmd:?} from account {} failed: {e:#}",
            conn.account_id
        );
    }
}

/// Does this reducer error mean the player DESYNCED from the module — their `game_world_entity` row is
/// gone? This happens when a schema-change publish tears down the gateway's coordinator subscription
/// mid-session (the gateway keeps the old, now-stale view, so `entity_by_owner` resolves nothing), or the
/// entity was deleted out from under this socket. Unlike a TRANSIENT per-action failure (a dead target,
/// an out-of-range cast, a retarget race), a desync means NO further action can ever be served on this
/// session — so callers propagate it as session-FATAL to force a clean disconnect: `run_world_session`
/// unwinds, closes the socket, and the client shows "Disconnected" and re-materializes everything on
/// relog from durable state, instead of the player silently hanging (can't attack, can't log out). Matched
/// on the module's `entity_by_owner` failure text ("… not in world" / "no live entity …").
///
/// ONE caller does not propagate it immediately: `forward_movement` tolerates
/// [`MOVE_DESYNC_TOLERANCE`] consecutive desynced movement packets first, because the tail of a
/// cross-map port lands on a legitimately despawned entity and killing the socket there hangs the
/// client on a loading screen. Past that bound it is fatal like everything else.
fn is_desync_error(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("not in world") || s.contains("no live entity")
}

/// Write one cast-lifecycle transition into the session. A transition only exists while the player
/// is in the world, so a character-select session ignores it.
fn apply_cast_transition(conn: &mut WorldConn, transition: CastTransition) {
    if let (Some(armed), WorldState::InWorld(iw)) = (transition.ranged_repeat, &mut conn.state) {
        iw.ranged_repeat = armed;
    }
}

/// Route one decrypted client message through the per-family handlers. Each stage either consumes
/// its opcode or passes it onward, so the disjoint-family chain ends in the movement-relay catch-all.
fn dispatch<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<()> {
    // Coalescing rule 2 (the robust flush): a pending coalesced heartbeat may leave the
    // MODULE's stored position a window behind the wire. Any non-movement opcode (loot/vendor/
    // gossip/etc. range checks, in particular) must see the CURRENT position, so flush it FIRST —
    // one check here removes the whole "stale position mid-coalesce" class instead of auditing
    // every position-sensitive reducer individually. A movement opcode flushes (if needed) as part
    // of its own classification below, not here.
    if codec::relayed_move_opcode(&msg).is_none() {
        if let Some((opcode, info)) = conn.move_coalesce.flush_now() {
            forward_movement(store, conn, opcode, &info)?;
        }
    }

    let Some(msg) = handle_char(tx, store, conn, msg)? else {
        return Ok(());
    };
    let msg = match dispatch_melee_action(store, MeleeActionPlayer::from_conn(conn), msg)? {
        MeleeActionOutcome::Handled {
            transition,
            outbound,
        } => {
            transition.apply(&mut conn.state);
            for message in outbound {
                send(tx, message)?;
            }
            return Ok(());
        }
        MeleeActionOutcome::PassThrough(msg) => msg,
    };
    // The cast seam owns every cast opcode. Melee runs first only because its two opcodes are
    // disjoint from the cast set; neither seam sees the other's traffic.
    let msg = match dispatch_cast(
        store,
        CastPlayer {
            account_id: conn.account_id,
            self_guid: social::self_guid(conn),
            ranged_repeat: matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat),
        },
        msg,
    )? {
        CastOutcome::Handled {
            transition,
            outbound,
        } => {
            // The transition lands first: the batch is what the client sees, and session state
            // must already agree with it when the next request arrives.
            apply_cast_transition(conn, transition);
            for message in outbound {
                send(tx, message)?;
            }
            return Ok(());
        }
        CastOutcome::PassThrough(msg) => msg,
    };
    let Some(msg) = handle_combat(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_loot(tx, store, conn, msg)? else {
        return Ok(());
    };
    let msg = match dispatch_vendor_action(
        store,
        VendorActionPlayer {
            account_id: conn.account_id,
            self_guid: social::self_guid(conn),
        },
        msg,
    )? {
        VendorActionOutcome::Handled { outbound } => {
            for message in outbound {
                send(tx, message)?;
            }
            return Ok(());
        }
        VendorActionOutcome::PassThrough(msg) => msg,
    };
    let Some(msg) = handle_bank(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_trainer(tx, store, conn, msg)? else {
        return Ok(());
    };
    let msg = match dispatch_item_action(
        store,
        ItemActionPlayer {
            account_id: conn.account_id,
            self_guid: social::self_guid(conn),
        },
        msg,
    )? {
        ItemActionOutcome::Handled { outbound } => {
            for message in outbound {
                send(tx, message)?;
            }
            return Ok(());
        }
        ItemActionOutcome::PassThrough(msg) => msg,
    };
    let msg = match dispatch_quest_action(
        store,
        QuestActionPlayer {
            account_id: conn.account_id,
            self_guid: social::self_guid(conn),
        },
        msg,
    )? {
        QuestActionOutcome::Handled { outbound } => {
            for message in outbound {
                send(tx, message)?;
            }
            return Ok(());
        }
        QuestActionOutcome::PassThrough(msg) => msg,
    };
    let msg = match dispatch_taxi_action(
        store,
        TaxiActionPlayer {
            self_guid: social::self_guid(conn),
        },
        msg,
    )? {
        TaxiActionOutcome::Handled { outbound } => {
            for message in outbound {
                send(tx, message)?;
            }
            return Ok(());
        }
        TaxiActionOutcome::Activated {
            outbound,
            character_guid,
            arm,
        } => {
            // Queue the vanilla result first. Arming mutates the entity and spline tables, whose
            // callbacks enqueue behind this item on the same writer even if they fire immediately.
            queue_reply_then_arm(tx, store, outbound, character_guid, arm)?;
            return Ok(());
        }
        TaxiActionOutcome::PassThrough(msg) => msg,
    };
    let Some(msg) = handle_social(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_trade(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_query(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_mail(tx, store, conn, msg)? else {
        return Ok(());
    };
    // Phase 5/6 (§6): MSG_MOVE_* -> movement_update (persist + relay). The relayed peer events
    // come back on this player's game_movement_event subscription and are re-emitted (same
    // opcode + verbatim MovementInfo) to other players by their own subscription callbacks.
    //
    // Every inbound movement packet is classified + coalesced by `CoalesceState`
    // before it ever reaches the module — a STATE-CHANGE (or a heartbeat whose flags/heading
    // drifted) forwards immediately, byte-identical to before this item; a pure same-vector
    // heartbeat is HELD until the cadence window elapses (or something else flushes it), dropping
    // the sub-yard same-vector intermediates the item targets. See `coalesce.rs` for the decision
    // table and unit tests, and `docs/` for the item's write-up.
    if let Some(opcode) = codec::relayed_move_opcode(&msg) {
        // 058: a landing may hurt. The MODULE applies the damage inside movement_update (it parses
        // fall_time from the raw MovementInfo body); this sends only the "You fall and lose N
        // health" flavor line — computed from the SAME lyracore_shared::env curve, so the line and the
        // health drop can never disagree. gtker types the wire's u32-ms fall time as f32 (a
        // field-TYPE lie; cmangos reads u32) — reinterpret the raw bits.
        if let ClientOpcodeMessage::MSG_MOVE_FALL_LAND(mv) = &msg {
            if let WorldState::InWorld(iw) = &conn.state {
                let fall_time_ms = mv.info.fall_time.to_bits();
                let dmg = lyracore_shared::env::fall_damage(
                    fall_time_ms,
                    store.entity_max_health(iw.self_guid),
                );
                if dmg > 0 {
                    use wow_world_messages::vanilla::{
                        EnvironmentalDamageType, SMSG_ENVIRONMENTAL_DAMAGE_LOG,
                    };
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(
                            Box::new(SMSG_ENVIRONMENTAL_DAMAGE_LOG {
                                guid: wow_world_messages::Guid::new(iw.self_guid),
                                damage_type: EnvironmentalDamageType::Fall,
                                damage: dmg,
                                absorb: 0,
                                resist: 0,
                            }),
                        )),
                    )?;
                }
            }
        }
        if let Some(info) = msg.movement_info() {
            for (fwd_opcode, fwd_info) in conn.move_coalesce.on_movement_now(opcode, info) {
                forward_movement(store, conn, fwd_opcode, &fwd_info)?;
            }
        }
    } else {
        log::debug!("world: ignoring {msg} (account {})", conn.account_id);
    }
    Ok(())
}

/// Actually forward one (already-classified) movement packet to the module + recenter AOI —
/// exactly what the pre-coalescing inline code did, unconditionally, for every packet. Now called once
/// per packet `CoalesceState` decides to forward (immediately for a state change, later for a
/// coalesced heartbeat), so a forwarded packet's on-wire effect is unchanged; only its TIMING can
/// differ (a pure heartbeat may forward later than it arrived; nothing else does).
fn forward_movement<St: WorldStore + ?Sized>(
    store: &St,
    conn: &mut WorldConn,
    opcode: u32,
    info: &MovementInfo,
) -> Result<()> {
    // The shared-call path names the mover by guid instead of by connection.
    // Movement only flows in-world, so the guid is always known here; 0 (its absence) forces the
    // per-player path in the store.
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };
    // A movement packet for an entity that is GONE is not a session-fatal desync — it is the normal
    // tail of a cross-map teleport. `teleport_player` despawns the live entity
    // the moment the portal's reducer commits, but the client only learns about it when
    // `SMSG_TRANSFER_PENDING`/`SMSG_NEW_WORLD` reach it, so every packet already in flight lands
    // afterwards — and the wider that window is (a dungeon-entry transaction that spawns a
    // 200-creature population is hundreds of milliseconds), the more certain at least one is. With
    // this propagating, one stray heartbeat closed the socket WHILE THE CLIENT WAS ON THE LOADING
    // SCREEN: no `MSG_MOVE_WORLDPORT_ACK` ever came back, the escrowed transfer that runs on that
    // ack never ran, and the player hung forever with no error and no recourse. A hang is the worst
    // outcome there is, and a dropped movement packet costs nothing: the client is authoritative for
    // its own position and re-sends one every ~100ms (and, mid-port, is about to be rebuilt from the
    // durable row anyway). CMSG_ATTACKSWING/ATTACKSTOP keep their session-fatal treatment — those
    // desyncs have no self-healing follow-up.
    // ...but the tolerance is BOUNDED. "Not in world" is also the answer
    // for a desync that will NEVER heal, and movement is the highest-frequency detector we have; an
    // unconditional swallow turns a socket that used to close cleanly into a player walking around a
    // frozen world forever, never disconnected, with no error — the very outcome `is_desync_error`
    // was introduced to prevent. So: drop the port tail, then give up. See MOVE_DESYNC_TOLERANCE.
    // NON-BLOCKING SUBMIT. This runs on the session's own socket-reader
    // thread, so blocking here meant a player's NEXT packet could not be read until the previous
    // movement round-tripped to the database and back — measured as a hard ~200 committed
    // movement_update/s across the WHOLE server, unchanged from 100 to 200 players, with the
    // database contributing 2.1 ms of a ~996 ms round-trip.
    //
    // BACKPRESSURE: past MAX_IN_FLIGHT_MOVES outstanding submissions we drop this packet rather than
    // queue it. That is not a loss of fidelity — a movement packet is a POSITION SNAPSHOT, the next
    // one supersedes it, and the coalescer already discards intermediate heartbeats for exactly this
    // reason. Without a bound, fire-and-forget converts a throughput ceiling into unbounded memory.
    // A shared movement batch logs and skips individual reducer rejections, so it cannot supply a
    // per-entry callback. The coordinator cache is the authoritative desync signal before queueing.
    if !store.entity_in_world(self_guid) {
        conn.move_desync_drops += 1;
        if conn.move_desync_drops > MOVE_DESYNC_TOLERANCE {
            let e = anyhow!("player not in world (guid {self_guid})");
            log::warn!(
                "world: account {} has desynced on {} consecutive movement packets — this is not a \
                 teleport tail, ending the session so the client relogs from durable state: {e:#}",
                conn.account_id,
                conn.move_desync_drops
            );
            return Err(e);
        }
        log::debug!(
            "world: dropping a movement packet for a despawned entity (account {}, drop {})",
            conn.account_id,
            conn.move_desync_drops
        );
        return Ok(());
    }
    conn.move_desync_drops = 0;
    let feedback = conn.move_feedback.clone();
    if feedback.in_flight() >= MAX_IN_FLIGHT_MOVES {
        conn.move_submit_dropped += 1;
        return Ok(());
    }
    store.movement_update_nowait(conn.account_id, self_guid, opcode, info, &feedback)?;
    // AOI: recenter this player's grid-scoped entity subscription if they crossed a cell. No-op
    // when AOI is disabled (no tracker) or the player stayed in-cell. Same-map (the tracker holds
    // the login map; teleport/zone changes re-anchor in a later phase).
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.subs.aoi_update(info.position.x, info.position.y);
    }
    Ok(())
}

/// Push one outbound unit to the writer; a closed channel means the writer/socket is gone, so end
/// the session cleanly.
fn send(tx: &SessionTx, out: Outbound) -> Result<()> {
    tx.send(out)
        .map_err(|_| anyhow!("world writer gone (client disconnected)"))
}

// ===========================================================================================
//  Async listener (bridges accepted sockets to the blocking handler)
// ===========================================================================================

pub async fn run(cfg: GatewayConfig, coordinator: Coordinator) -> Result<()> {
    let listener = TcpListener::bind(&cfg.world_bind).await?;
    log::info!("world listening on {}", cfg.world_bind);

    // One process-wide admission gate shared by every accepted connection (`Arc` so each
    // `spawn_blocking` task below can hold its own clone across the blocking call). Unconfigured
    // (`LYRACORE_MAX_SESSIONS` unset) is `LoginQueue::unlimited()` — logged plainly so "why is nobody
    // queuing" is answerable from the startup log alone.
    let login_queue = Arc::new(LoginQueue::from_env());
    if login_queue.max_sessions() > 0 {
        log::info!(
            "world: login queue ARMED — max_sessions={} admit_concurrency={}",
            login_queue.max_sessions(),
            if login_queue.admit_concurrency() == 0 {
                "unlimited".to_string()
            } else {
                login_queue.admit_concurrency().to_string()
            }
        );
    } else {
        log::info!(
            "world: login queue disabled (LYRACORE_MAX_SESSIONS unset/0) — unlimited admission"
        );
    }

    // Dump the motion-relay counters every 10s from a background task, so a failing
    // run is localised WITHOUT touching the relay hot path (a log there masks the race entirely).
    let queuestat_queue = login_queue.clone();
    tokio::spawn(async move {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut pc, mut ps, mut pd) = (0u64, 0u64, 0u64);
        let mut idle_logged = false;
        let mut prev_sub = 0u64;
        // The AOI churn counter, read the same way MOTION_* is above — one atomic, one
        // 10s delta, no hot-path log line.
        let mut prev_recenters = 0u64;
        // Trailing fan-out baseline + collapsed-window run, for the fan-out-collapse warning at the
        // bottom of the loop. Task-local: this is the only place it lives, and nothing on the relay
        // path reads it.
        let mut fan = crate::stdb::subscriptions::FanoutHealth::default();
        // QUEUESTAT baseline — same "diff against last reading" convention as MOTIONSTAT.
        let mut prev_admitted = queuestat_queue.admitted_total();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            // QUEUESTAT: printed only while the line is NONEMPTY — an unconfigured or
            // uncontended gateway must not print this every 10s forever, same rule as AOISTAT.
            // Also serves as a drain backstop (`LoginQueue::tick`) in case every queued
            // connection's own poll cadence is somehow slower than this.
            queuestat_queue.tick();
            let depth = queuestat_queue.depth();
            let admitted = queuestat_queue.admitted_total();
            if depth > 0 {
                let admitted_per_s = admitted.saturating_sub(prev_admitted) as f64 / 10.0;
                let oldest = queuestat_queue.oldest_wait().unwrap_or_default();
                log::info!(
                    "QUEUESTAT depth={depth} admitted={admitted} (+{:.1}/s) active={} oldest_wait={:.1}s",
                    admitted_per_s,
                    queuestat_queue.active(),
                    oldest.as_secs_f64(),
                );
            }
            prev_admitted = admitted;
            let (c, s, d) = (
                crate::stdb::subscriptions::MOTION_CALLS.load(Relaxed),
                crate::stdb::subscriptions::MOTION_SENT.load(Relaxed),
                crate::stdb::subscriptions::MOTION_DROPPED.load(Relaxed),
            );
            // Only on CHANGE, plus one line on the transition to idle — a long-lived server with
            // nobody moving must not print this forever.
            if c != pc || s != ps || d != pd {
                let (sub, comp) = (MOVE_SUBMITTED.load(Relaxed), MOVE_COMPLETED.load(Relaxed));
                // The ratios are FORMATTED here, not left to the operator's
                // arithmetic — 371 co-located clients under-delivered peer movement by 37 % and
                // this line said nothing, because raw totals climb just as fast when each one is
                // reaching fewer observers than it should.
                log::info!(
                    "{}",
                    crate::stdb::subscriptions::motionstat_line(
                        c,
                        s,
                        d,
                        c - pc,
                        s - ps,
                        d - pd,
                        sub.saturating_sub(prev_sub),
                        sub,
                        comp,
                    )
                );
                idle_logged = false;
            } else if c > 0 && !idle_logged {
                log::info!("MOTIONSTAT calls={c} sent={s} dropped={d} — relay went idle");
                idle_logged = true;
            }
            // AOI subscription-churn counter — recenters (cell crossings → box
            // re-subscribes) across every session in the last 10s. Only printed on change, same
            // rule as MOTIONSTAT above; a server with nobody crossing a cell boundary must not
            // print this every 10s forever.
            let recenters = crate::stdb::aoi::AOI_RECENTERS.load(Relaxed);
            let d_recenters = recenters.saturating_sub(prev_recenters);
            if d_recenters > 0 {
                log::info!(
                    "AOISTAT recenters={recenters} (+{d_recenters} in 10s, {:.3}/s)",
                    d_recenters as f64 / 10.0
                );
            }
            prev_recenters = recenters;
            // HEALTH SIGNAL. The failure this catches is silent and permanent: the motion
            // subscription stops delivering while the module keeps accepting movement, so the
            // server looks healthy — players are moving, reducers are committing — and every peer
            // simply stands still. Observed running for 2.5 HOURS on one gateway before anyone
            // noticed, and it does not self-heal.
            //
            // The two counters make it unambiguous without needing to know the trigger: movement
            // being SUBMITTED (players are active) while the relay callback fires almost never can
            // only mean delivery is broken. Deliberately a loud WARN and not a silent metric — the
            // whole defect is that nothing said anything.
            let (sub, comp) = (MOVE_SUBMITTED.load(Relaxed), MOVE_COMPLETED.load(Relaxed));
            let submitted_delta = sub.saturating_sub(prev_sub);
            if submitted_delta > MOVE_ACTIVITY_FLOOR && c.saturating_sub(pc) * 10 < submitted_delta
            {
                log::warn!(
                    "MOTION RELAY LOOKS DEAD: {submitted_delta} movements submitted in the \
                     last 10s but the relay callback fired only {} times — peers are almost \
                     certainly frozen for connected players. calls={c} sent={s} completed={comp}. \
                     The historical cause (the AOI recenter resubscribing a SHORTER query set, so \
                     the first cell crossing dropped game_entity_motion) cannot recur — the shared- \
                     call path removed the per-player AOI subscription entirely. This firing now \
                     means the SHARED \
                     coordinator dispatch is not running: check the log for a `shared AOI dispatch` \
                     panic line, and that `coordinator connected to shard` was printed for every \
                     database. Restart the gateway to recover play.",
                    c.saturating_sub(pc)
                );
            }
            // FAN-OUT COLLAPSE SIGNAL. The health-signal check above only fires when
            // the relay goes (almost) silent. The 371-client incident was the PARTIAL version of the
            // same failure: 63 % of the movement delivered, `delivery` reading ~100 % because the
            // shortfall was in `calls`, and the one field that showed it — `fanout` — meaning nothing
            // to a reader who does not already know it should have read ~371. So compare fan-out
            // against the gateway's OWN recent baseline and say so out loud. All of it inside this
            // 10-second task: zero hot-path cost, per `subscriptions.rs:208`. Read
            // `fanout_health_step`'s doc comment for what this deliberately does NOT catch (a gateway
            // degraded since startup has a low baseline and never trips it).
            let fanout =
                crate::stdb::subscriptions::fanout_ratio(c.saturating_sub(pc), submitted_delta);
            let (next_fan, fanout_warn) =
                crate::stdb::subscriptions::fanout_health_step(fan, fanout, submitted_delta);
            if let Some(low_windows) = fanout_warn {
                log::warn!(
                    "PEER MOVEMENT UNDER-DELIVERING: fan-out is {:.1} observers/move, down \
                     from a trailing baseline of {:.1}, for {low_windows} consecutive 10s windows \
                     while {submitted_delta} movements were submitted. Players are moving and \
                     nothing is erroring, but peers update less often than they move — the silent \
                     63 % under-delivery shape of the 371-client run. \
                     calls={c} sent={s} dropped={d}. This is a CHANGE against this gateway's own \
                     recent behaviour, not an absolute expectation: check whether client count, the \
                     module's movement tick, or the SDK pump's callback throughput changed in the \
                     last minute.",
                    fanout.unwrap_or(0.0),
                    fan.baseline.unwrap_or(0.0),
                );
            }
            fan = next_fan;
            prev_sub = sub;
            (pc, ps, pd) = (c, s, d);
        }
    });
    // The loop that `Error: Too many open files (os error 24)` came out of on 2026-08-07,
    // taking every session on the realm with it. A transient accept errno now costs ONE connection.
    // See `crate::accept` for which errnos are fatal and why the list is shaped that way.
    let mut backoff = AcceptBackoff::new();
    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(pair) => {
                backoff.record_success();
                pair
            }
            Err(e) => match classify_accept_error(&e) {
                AcceptOutcome::Fatal => {
                    log::error!(
                        "world listener is unusable and cannot accept again: {e} — ending the \
                         world task"
                    );
                    return Err(e.into());
                }
                AcceptOutcome::Retry => {
                    let delay = backoff.record_failure();
                    log::warn!(
                        "world accept failed ({e}); skipping this connection (consecutive={}, \
                         backing off {}ms). Players already in world are unaffected; if this is \
                         EMFILE, the gateway is out of file descriptors — check for a leaked \
                         per-account SDK connection (each account's cached connection releases at \
                         session teardown).",
                        backoff.consecutive(),
                        delay.as_millis()
                    );
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    continue;
                }
            },
        };
        let coord = coordinator.clone();
        let queue = login_queue.clone();
        // wow_world_messages uses blocking std::io codecs, so run the per-connection state
        // machine on a blocking task with the socket in blocking mode (mirrors `logon`).
        // Per-SOCKET calls (a dup and an fcntl on the fd we just accepted), so they fail for the
        // same reasons accept does — EMFILE above all. Drop the one connection, keep the realm.
        let std_sock = match sock.into_std().and_then(|s| {
            s.set_nonblocking(false)?;
            Ok(s)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("world connection {peer} could not be handed to a blocking task: {e}");
                continue;
            }
        };
        tokio::task::spawn_blocking(move || {
            // `Coordinator` implements `WorldStore` directly (see `stdb::world_store`) — no wrapper.
            // `queue` gates admission INSIDE the handshake — a queued connection just blocks
            // this one `spawn_blocking` thread, never the accept loop above.
            if let Err(e) = run_world_session_with_queue(std_sock, &coord, &queue) {
                log::warn!("world session {peer} ended: {e:#}");
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests;
