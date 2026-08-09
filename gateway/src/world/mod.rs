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
//! gateway restart costs only a client reconnect (acceptance criterion #5): K is read back
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
pub mod login_queue;
pub mod loot;
pub mod packet_lint;
pub mod party;
mod social;
pub mod transfer;
pub mod whisper;
use coalesce::CoalesceState;
use login_queue::{Admission, LoginQueue};
use social::handle_social;
use transfer::{EscrowedTransfer, TransferPlan};

/// The `(display_id, inv_type)` ammo block for the auto-shot activation `SMSG_SPELL_START` (097):
/// `Some` only when the ranged slot (17) holds a LAUNCHER (weapon subclass 2/3/18 — bow/gun/crossbow)
/// and a class-6 Projectile stack is in the bags; a wand fires its own bolt (no ammo block), mirroring
/// the swing tick's per-shot rule + the GO relay. Deliberate simplification: inv-type 24
/// (INVTYPE_AMMO) is hardcoded like the GO relay; no-ammo launchers get None (the first shot tick
/// then tears the loop down → cancel).
fn ranged_ammo_display<St: WorldStore + ?Sized>(store: &St, self_guid: u64) -> Option<(u32, u32)> {
    let items = store.player_items(self_guid).ok()?;
    let launcher = items
        .iter()
        .find(|i| i.slot == 17)
        .and_then(|i| store.item_template(i.entry).ok().flatten())?;
    if launcher.class != 2 || !matches!(launcher.subclass, 2 | 3 | 18) {
        return None;
    }
    // min_by_key(slot) + stack_count > 0 mirrors the module's per-shot `find_ammo` pick, so the
    // nocked projectile on the START matches the one each SPELL_GO fires (review find — the client
    // cache iterates unsorted, and a dead stack must not be nocked).
    items
        .iter()
        .filter(|i| i.stack_count > 0)
        .filter(|i| {
            store
                .item_template(i.entry)
                .ok()
                .flatten()
                .is_some_and(|t| t.class == 6)
        })
        .min_by_key(|i| i.slot)
        .and_then(|i| store.item_template(i.entry).ok().flatten())
        .map(|t| (t.display_id, 24))
}

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
    /// DEFERRED relay work (#468). The shared AOI dispatch runs on ONE coordinator pump per shard
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
///     across 371 sessions, bounded, instead of a queue that grows for as long as the socket stalls
///     (work-items 285 / 292).
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

/// How an ITEM-target enchanting cast routes, derived from the spell's effect kind (not a spell id). [094]
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
/// Per-session movement outcome slot for the non-blocking submit path (perf catalog 1.13, #110).
///
/// Fire-and-forget moves the error from the caller's `Result` to a later callback, so two things
/// need somewhere to live: the error itself (applied on the session's next packet, preserving the
/// desync-tolerance behaviour exactly) and the number of submissions still outstanding.
///
/// `in_flight` is what stops fire-and-forget turning a throughput limit into an unbounded-memory
/// one: past `MAX_IN_FLIGHT_MOVES` the session coalesces instead of submitting, which is the same
/// thing the coalescer already does for a fast-moving client — bounded work, newest position wins,
/// no queue to grow.
/// #110 diagnosis: global submit/complete counts for the non-blocking movement path.
///
/// The gateway forwards ~400 movements/s at 200 players while the module reports ~200
/// `movement_update` transactions/s. These two counters split the difference cleanly:
///   * `submitted == completed == ~400/s` -> the SDK delivered every call and got a response, so
///     the module DID run them and its transaction metric is undercounting.
///   * `completed ~200/s` -> half the calls never completed, so they are being dropped between the
///     gateway's send and the module -- a real loss of player movement.
///
/// Movements submitted in a 10s window below which the #109 relay-health check stays quiet. A
/// handful of movements with no relay traffic is just a lone player with nobody nearby to relay to;
/// hundreds is the broken case. 100 ≈ five moving players.
pub const MOVE_ACTIVITY_FLOOR: u64 = 100;

pub static MOVE_SUBMITTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static MOVE_COMPLETED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Default, Debug)]
pub struct MovementFeedback {
    in_flight: std::sync::atomic::AtomicUsize,
    last_err: std::sync::Mutex<Option<String>>,
}

/// Outstanding non-blocking movement submissions allowed per session before the next one is
/// coalesced instead. Small on purpose: a vanilla client heartbeats ~2/s, so anything above a
/// couple in flight means the server is already behind and the newest position is the only one that
/// matters.
pub const MAX_IN_FLIGHT_MOVES: usize = 4;

impl MovementFeedback {
    pub fn record_err(&self, e: String) {
        *self.last_err.lock().unwrap() = Some(e);
    }
    /// Take the pending error, if any — the session applies it on its next packet.
    pub fn take_err(&self) -> Option<String> {
        self.last_err.lock().unwrap().take()
    }
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

pub trait WorldStore: Send + Sync {
    /// Look up the shared session key K (+ account id) for an (already uppercased) account
    /// name. `None` when no live session exists for that account (reject the handshake).
    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>>;

    /// Multi-shard routing (#17): the handle for the shard that OWNS `character_guid`'s location,
    /// resolved once per world entry and then used for EVERY player-scoped call and subscription of
    /// that session (see `on_home_shard!`). `None` means "you are already on the right shard" —
    /// which is what a single-entry shard map, and every mock, always answer, so the session keeps
    /// the handle it was given and behaves byte-identically to the pre-sharding gateway.
    fn home_shard(&self, _character_guid: u64) -> Option<std::sync::Arc<dyn WorldStore>> {
        None
    }

    /// The database this handle targets — routing identity, for logs and for the tests that assert
    /// no call ever escapes the player's home shard. `""` for mocks that don't model shards.
    fn shard_name(&self) -> &str {
        ""
    }

    // --- Cross-database transfer (#19). Every one defaults to the single-database posture, so a
    // --- store that does not shard (and every mock that does not exercise transfers) is unchanged.

    /// Put `character_guid` on the shard that owns its location, running the escrowed transfer if
    /// it is somewhere else, then answer the same question [`home_shard`](Self::home_shard) does.
    /// Called at every world entry; `Err` fails the login rather than letting a half-moved
    /// character into the world on either side.
    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        Ok(self.home_shard(character_guid))
    }

    /// The escrow row this shard holds for `character_guid`, if any — the transfer's identity, its
    /// destination and the serialized character. `None` = not mid-transfer here.
    fn escrowed_transfer(&self, _character_guid: u64) -> Option<EscrowedTransfer> {
        None
    }

    /// Where this shard's durable row says the character is going (`world::teleport_player` wrote
    /// the destination there before despawning the entity). `None` = this shard has no row for it.
    fn character_destination(&self, _character_guid: u64) -> Option<TransferPlan> {
        None
    }

    /// `begin_transfer` — freeze + serialize + delete the live entity, in one transaction.
    fn begin_transfer(&self, _plan: &TransferPlan) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `import_character_blob` — materialise the arrival copy from the carried blob.
    fn import_character_blob(&self, _transfer_id: u64, _blob: &[u8]) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `confirm_import` — attest, on the SOURCE, that the destination copy is durable.
    fn confirm_import(&self, _transfer_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `finish_transfer` — delete-last: destroy the source copy and clear the escrow.
    fn finish_transfer(&self, _transfer_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `release_transfer` — drop the arrival copy's fence. Replay-safe: `Ok` when there is nothing
    /// filed under this id, which is why it can be called speculatively at world entry.
    fn release_transfer(&self, _transfer_id: u64) -> Result<()> {
        Ok(())
    }

    /// `set_character_shard` on the REALM-CORE handle — publish where a settled transfer put the
    /// character (issue #34). Called by `transfer::run_transfer` immediately after
    /// `finish_transfer` commits, so it can only ever name a destination the escrow actually
    /// reached; see `crate::realm_core::publish_shard_index` for why that is the strongest form of
    /// "transactional" available across two databases.
    ///
    /// The default is a no-op so a store that does not shard is unchanged — the same posture as
    /// every other transfer method. Production overrides it in `stdb::world_store`.
    fn publish_shard_index(
        &self,
        _character_guid: u64,
        _map_id: u32,
        _instance_id: u64,
    ) -> Result<()> {
        Ok(())
    }

    /// `ensure_instance` — mirror an instance id onto this shard, spawning its population once.
    fn ensure_instance(&self, _instance_id: u64, _map_id: u32, _party_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `evict_instance_population` — drop an instance's population here, keeping the lease row.
    fn evict_instance_population(&self, _instance_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// Bind this shard's per-player connection identity to the account (`establish_session`), so
    /// `player_login` can resolve the caller here. A no-op on the realm shard, where the logon tier
    /// already did it. Called at world entry whenever the session's home shard is not the realm.
    fn bind_shard_session(&self, _account_id: u64, _session_key: &[u8; 40]) -> Result<()> {
        Ok(())
    }

    // --- Realm-wide party state (#22, group slice). Every one defaults to the single-database
    // --- posture, so an unsharded store — and every mock that does not model a realm — is
    // --- unchanged: `realm_store()` answering `None` is what routes every party op back onto the
    // --- player's own shard through the pre-#22 reducers.

    /// The **realm-core** handle: the database that owns party membership realm-wide (#22).
    ///
    /// `None` is not "no realm-core configured" — it is "this gateway runs against ONE database", in
    /// which case that database already is the authority and there is nothing to route. `world::party`
    /// branches on exactly this, so the single-database path never reads a row it did not read before.
    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>> {
        None
    }

    /// Every connected WORLD shard's handle (realm-core excluded — it owns no gameplay reads). The
    /// fan-out set for the roster mirror; empty on a single-database gateway, which is what makes the
    /// mirror push a no-op there.
    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>> {
        Vec::new()
    }

    /// `realm_group_op` — run one party op against the database this handle names (#22). Called on
    /// the realm-core handle; the op byte and argument slots are `lyracore_shared::group::realm_op`.
    fn realm_group_op(
        &self,
        _op: u8,
        _actor_guid: u64,
        _target_guid: u64,
        _arg_a: u8,
        _arg_b: u8,
    ) -> Result<()> {
        Err(anyhow!("this store does not host realm-wide party state"))
    }

    /// The party `character_guid` is in, as THIS handle's database sees it (#22): authoritative on
    /// realm-core, a mirror on a world shard. `None` = not in a party there.
    fn group_roster(&self, _character_guid: u64) -> Result<Option<party::GroupRoster>> {
        Ok(None)
    }

    /// [`group_roster`](Self::group_roster) keyed by the group — the read the mirror push needs for a
    /// party the acting character has just left.
    fn group_roster_by_id(&self, _group_id: u64) -> Result<Option<party::GroupRoster>> {
        Ok(None)
    }

    /// `sync_group_mirror` — replace this shard's mirror of one party with realm-core's roster (#22).
    /// An empty `roster.members` is the disband tombstone.
    fn sync_group_mirror(&self, _roster: &party::GroupRoster) -> Result<()> {
        Ok(())
    }

    /// `realm_whisper` — deliver one whisper against the database this handle names (#22, whisper
    /// slice). Called on the realm-core handle, which is the only one that can address BOTH parties of
    /// a cross-shard whisper: `recipient_guid` is realm-wide, a bound identity is per-database.
    ///
    /// `sender_is_ignored` is the target's ignore-list verdict, resolved by the gateway from the shard
    /// that holds the target's contact rows — realm-core has none. The default errors rather than
    /// silently succeeding: a store that does not host the realm plane must never be *asked*, and
    /// `world::whisper` only asks the handle `realm_store()` handed it.
    fn realm_whisper(
        &self,
        _sender_guid: u64,
        _target_guid: u64,
        _message: String,
        _sender_is_ignored: bool,
    ) -> Result<()> {
        Err(anyhow!("this store does not host realm-wide whispers"))
    }

    // --- Realm-wide loot rolls (#50). Every one defaults to the single-database posture, so an
    // --- unsharded store — and every mock that does not model a realm — is unchanged: `realm_store()`
    // --- answering `None` is what routes `CMSG_LOOT_ROLL` back onto the player's own shard through
    // --- the pre-#50 `loot_roll` reducer, and leaves the relay (`loot::relay_tick`) with nothing to do.

    // Mirrors the `realm_loot_op` REDUCER's parameter list 1:1 — this trait is the seam between them, so the shapes have to match.
    #[allow(clippy::too_many_arguments)]
    /// `realm_loot_op` — run one loot-roll op against the database THIS handle names (#50). Called on
    /// the **realm-core** handle: START promotes a world shard's staging roll, VOTE casts a vote.
    fn realm_loot_op(
        &self,
        _op: u8,
        _corpse_guid: u64,
        _slot: u8,
        _item_entry: u32,
        _actor_guid: u64,
        _vote: u8,
        _deadline_micros: i64,
        _recipients: Vec<u64>,
    ) -> Result<()> {
        Err(anyhow!("this store does not host realm-wide loot rolls"))
    }

    /// Every UNRESOLVED loot roll this WORLD SHARD has created but not yet had promoted onto
    /// realm-core (#50) — the relay's promotion queue. Empty by default, which is what makes the
    /// relay a no-op on an unsharded store and on realm-core's own handle (nothing is ever created
    /// there directly — only `realm_loot_op`'s START arm writes it, and that is not this method).
    fn pending_local_rolls(&self) -> Result<Vec<loot::PendingLootRoll>> {
        Ok(Vec::new())
    }

    /// `settle_loot_roll` — grant a resolved roll's item on THIS world shard, if it holds the
    /// matching corpse row (#50). A no-op default so an unsharded store, and every shard that does
    /// not hold the corpse, are unaffected; the module's own `withheld` guard is what makes a
    /// wrong-shard call harmless in production too.
    fn settle_loot_roll(&self, _corpse_guid: u64, _slot: u8, _winner_guid: u64) -> Result<()> {
        Ok(())
    }

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the relay
    /// has promoted it onto realm-core (#50). A no-op default, matching `sync_group_mirror`'s shape.
    fn clear_promoted_loot_roll(&self, _roll_id: u64) -> Result<()> {
        Ok(())
    }

    // Same shape as `Coordinator::loot_won_since` (watermark + `(corpse, slot, winner)` triples) — the trait mirrors the read it fronts.
    #[allow(clippy::type_complexity)]
    /// Every `ROLL_WON` `game_group_event` row realm-core has pushed with an id greater than
    /// `after_id` (#50) — `(corpse_guid, slot, winner_guid)` triples, plus the new high-water mark to
    /// poll from next. Called on the **realm-core** handle. `(after_id, [])` by default, so the relay
    /// never advances its watermark and never settles anything on an unsharded/mock store.
    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        Ok((after_id, Vec::new()))
    }

    /// The account's characters for the character-select screen (Phase 3). In production this
    /// reads the per-player `game_character` subscription (RLS-restricted to the owner).
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>>;

    /// Create a character for the account (`CMSG_CHAR_CREATE`). Returns the game outcome
    /// (success / name-in-use / failed); `Err` only for an unrecoverable transport failure.
    fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome>;

    /// Delete a character for the account (`CMSG_CHAR_DELETE`, work-item 081). Returns the game
    /// outcome (success/failed); `Err` only for an unrecoverable transport failure. Ownership is
    /// enforced module-side (the character must belong to `account_id`).
    fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<codec::CharDeleteOutcome>;

    /// Enter the world with `character_guid` (Phase 4): calls the `player_login` reducer and
    /// returns the live entity to spawn (from the resulting `game_world_entity` row). Errors if
    /// the character isn't the caller's.
    fn player_login(&self, account_id: u64, character_guid: u64) -> Result<codec::EntityView>;

    /// Persist + relay an inbound movement (Phase 5): calls the `movement_update` reducer with
    /// the mover (= `ctx.sender` on the per-player path; named by `self_guid` on the
    /// `LYRACORE_SHARED_CALLS` path — #468 stage 4b), the opcode to relay, and the
    /// `MovementInfo`. Relayed peer events arrive back on the per-player subscription (Phase 6).
    fn movement_update(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()>;

    /// Movement, submitted WITHOUT waiting for the module's completion (perf catalog 1.13, #110).
    ///
    /// The outcome lands in `feedback` instead, and the session applies it on its next packet. The
    /// default implementation forwards to the blocking `movement_update`, so mock stores and any
    /// future `WorldStore` keep working unchanged — only the live `Coordinator` overrides it.
    fn movement_update_nowait(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
        _feedback: &std::sync::Arc<MovementFeedback>,
    ) -> Result<()> {
        // Returns the error INLINE and deliberately does not also record it in `feedback` — doing
        // both would count one failure twice (once here, once when the caller drains the slot on the
        // next packet), which is exactly what
        // `a_movement_packet_for_a_despawned_entity_never_kills_the_session` caught. A store that
        // answers synchronously has no deferred verdict to report.
        self.movement_update(account_id, self_guid, opcode, info)
    }

    /// Subscribe this player's connection to its per-player views (nearby `game_world_entity`,
    /// addressed `game_movement_event`) and push the resulting peer-spawn / movement-relay / destroy
    /// SMSG onto `tx` (Phase 6/7). The returned guard tears the subscription + callbacks down on
    /// drop. Called once, at `CMSG_PLAYER_LOGIN`, when `self_guid` is known.
    fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        login_instance: u64,
        login_map: u32,
        login_x: f32,
        login_y: f32,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions>;

    /// Remove the player from the world (Phase 7): calls the `logout` reducer so the live
    /// `game_world_entity` row is deleted and observers see the peer vanish. Called on disconnect.
    fn logout(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Look up a character by guid (any owner) to answer `CMSG_NAME_QUERY` — the queried guid is
    /// usually a peer, so this is not account-scoped.
    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>>;

    /// Look up a creature template by entry to answer `CMSG_CREATURE_QUERY` (Tier 2 / NPCs).
    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>>;

    /// Look up an item template by entry to answer `CMSG_ITEM_QUERY_SINGLE` (items slice-1).
    fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>>;

    /// Look up a gameobject template by entry to answer `CMSG_GAMEOBJECT_QUERY`.
    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>>;

    /// The `type_id` of a SPAWNED gameobject by its live guid (work-item 041) — lets
    /// `CMSG_GAMEOBJ_USE` route a `go_type::QUESTGIVER` GO (the Wanted Poster, the Lost Guards
    /// corpses) to the quest window instead of the loot/toggle reducer path. Defaulted to `Ok(None)`
    /// (never a questgiver) so existing `WorldStore` implementors (test mocks) that don't override it
    /// keep their prior CMSG_GAMEOBJ_USE behavior unchanged; only the production `Coordinator` impl
    /// (`stdb::world_store`) overrides it with a real read.
    fn gameobject_type(&self, _go_guid: u64) -> Result<Option<u8>> {
        Ok(None)
    }

    /// Use a gameobject (`CMSG_GAMEOBJ_USE`): a chest rolls its loot, a quest-object grants credit.
    fn use_gameobject(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()>;

    /// Enter an area trigger (`CMSG_AREATRIGGER`): credit any active "explore" quest tied to it.
    fn enter_areatrigger(&self, account_id: u64, self_guid: u64, trigger_id: u32) -> Result<()>;

    /// Forward a parsed addon-bridge command (184) to the module's `client_command` reducer ON
    /// THE PLAYER'S CONNECTION — the handler runs with exactly the player's reducer authority.
    fn client_command(&self, account_id: u64, self_guid: u64, cmd: String, payload: String) -> Result<()>;

    /// Read every item a character owns, for the login item spawns + inventory slots (items slice-1).
    fn player_items(&self, owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>>;
    /// The character's learned skill lines as `(skill_line, current, max_rank)` — feeds the self
    /// CREATE's SkillInfo block (061). Empty when no `game_player_skill` rows exist.
    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>>;

    /// The EFFECTIVE armor for `guid` (base + worn gear armor) for the self-login CREATE's
    /// `UNIT_FIELD_RESISTANCES[0]` — so the character sheet shows real worn armor on relog. Auras aren't
    /// folded here (they self-correct via the on_aura relay). Mirrors the module's combat `effective_armor`.
    fn effective_armor(&self, guid: u64) -> u32;

    /// Read a corpse's item loot for the loot window (items slice-4): `(slot, id, count, display)`,
    /// filtered for `viewer_guid` (work-item 187 slice 0 — a `quest_only` row is shown only to a
    /// viewer who currently needs it, or who already owns a per-member reserved clone of it).
    fn corpse_loot(&self, corpse_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>>;

    /// Read a vendor's stock for `SMSG_LIST_INVENTORY` (Tier 2 / vendors): resolve the vendor's
    /// creature entry from its entity row, join `game_npc_vendor` × `game_item_template`.
    fn vendor_items(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>>;

    /// Standing-derived reaction gate (195 slice A): does this NPC refuse `player_guid` its
    /// interaction WINDOW (gossip/vendor/trainer/questgiver)? Rep-bar factions refuse at
    /// Unfriendly-or-below standing; bar-less factions fall back to the FactionTemplate hostility
    /// masks. Fail-open on missing data.
    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool>;

    /// Buy `count` of `item_entry` from `vendor_guid` (`CMSG_BUY_ITEM`, Tier 2). The module gates
    /// the purchase on the vendor (stock / range / copper); a gameplay `Err` is per-action, not fatal.
    fn buy_item(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()>;

    /// Sell the item in inventory `slot` back to `vendor_guid` (`CMSG_SELL_ITEM`, Tier 2). The gateway
    /// resolves the client's item-instance guid to its slot first (the reducer takes the slot) and
    /// passes the vendor guid the client named so the module can range-gate the sale (like buy).
    fn sell_item(&self, account_id: u64, self_guid: u64, vendor_guid: u64, slot: u8) -> Result<()>;

    /// Re-purchase item in buyback ring slot `slot` (0-based) from vendor (`CMSG_BUYBACK_ITEM`). The
    /// gateway maps `BuybackSlot.as_int() - 69` before calling; the module gates range + copper.
    fn buyback_item(&self, account_id: u64, self_guid: u64, vendor_guid: u64, slot: u8) -> Result<()>;

    /// Repair the item with the given inventory `slot` at REPAIR-NPC `npc_guid` (`CMSG_REPAIR_ITEM`).
    /// The gateway resolves the client's item-instance guid to its slot first (the reducer takes the
    /// slot); the module gates the NPC (REPAIR flag / range) and charges copper. `slot == u8::MAX`
    /// repairs the whole body. A gameplay `Err` (out of range / too poor) is per-action, not fatal.
    fn repair_item(&self, account_id: u64, self_guid: u64, npc_guid: u64, slot: u8) -> Result<()>;

    /// The spells a class trainer (`trainer_guid`) teaches, each pre-folded with the player's level +
    /// known-state for the `SMSG_TRAINER_LIST` Green/Red/Gray rendering (`CMSG_TRAINER_LIST`).
    fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>>;

    /// Buy/learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`). The module gates it
    /// (range / level / cost / not-already-known); `Err` carries a `[N]` gtker failure-reason tag.
    fn buy_trainer_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<()>;

    /// Skin a beast corpse that has been fully looted (no items, no money left). The module gates it
    /// (dead beast, in range, not already skinned); on success the leather lands in the bag via the
    /// item-subscription relay. `Err` = not applicable (not a beast, out of range, already skinned,
    /// or dead player) — the caller falls through to the empty loot window and the player sees nothing.
    fn skin_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()>;

    /// Given an item-instance GUID from a client spell-target, return the bag slot for that item
    /// (so the disenchant / enchant_item reducer can receive a slot, not a GUID).
    fn item_slot_by_guid(&self, account_id: u64, item_guid: u64) -> Option<u8>;

    /// Disenchant the item in `slot` (`CMSG_CAST_SPELL` spell 13262). The module validates skill +
    /// item disenchantability and yields Strange Dust into the bag.
    fn disenchant_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()>;

    /// Apply `enchant_id` to the item in `slot` (`CMSG_CAST_SPELL` for enchant spell). The module
    /// validates skill, consumes reagent dust, and stamps enchant_id on the item instance.
    fn enchant_item_on_slot(&self, account_id: u64, self_guid: u64, slot: u8, enchant_id: u32) -> Result<()>;

    /// Return the `grant_spell_id` for `talent_id` (0 = passive, no ability granted), so the gateway
    /// can push `SMSG_LEARNED_SPELL` for ability talents after a successful `learn_talent`.
    fn talent_grant_spell(&self, talent_id: u32) -> u32;

    /// True iff `spell_id` spawns a ground area (E_PERSISTENT_AREA) — its GO carries no hit list. [118]
    fn spell_is_ground_area(&self, spell_id: u32) -> bool;

    /// True iff `spell_id` is a Fishing cast (E_FISH) — routed to the `fish` reducer. [060]
    fn spell_is_fishing(&self, spell_id: u32) -> bool;

    /// The instant-resolve Fishing catch (060).
    fn fish(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// True iff `spell_id` is an Open-Lock cast (E_OPEN_LOCK) — routed to the `pick_lock` reducer. [119]
    fn spell_is_open_lock(&self, spell_id: u32) -> bool;

    /// Pick the lock on GameObject `go_guid` (`CMSG_CAST_SPELL` for Pick Lock). The module gates it
    /// (range / lock requirement / caster's Lockpicking skill); `Err` = refused (out of range, not
    /// locked, or skill too low) → the gateway answers SMSG_CAST_RESULT::Failure. [119]
    fn pick_lock(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()>;

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`); action 0 clears the slot.
    fn set_action_button(
        &self,
        account_id: u64, self_guid: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()>;

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`, 195 slice B).
    /// `reputation_index` is the client's 0..63 rep-array slot, NOT a faction id.
    fn set_faction_at_war(
        &self,
        account_id: u64, self_guid: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()>;

    /// Talent-pane sync after a successful `learn_talent`: `(teach_spell, superseded_prev,
    /// points_remaining)` — the rank-spell to relay as LEARNED/SUPERCEDED (the 1.12 TalentFrame
    /// derives shown ranks from known rank-spells) and the live PLAYER_CHARACTER_POINTS1 value
    /// (earned − spent). `talent_id = 0` → just the points.
    fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32);

    /// Sum of the character's spent talent ranks — non-zero gates the login points correction.
    fn talent_points_spent(&self, character_guid: u64) -> u32;

    /// The character's active spell-modifier auras as raw (family_mask, op, amount, is_pct) rows —
    /// the SMSG_SET_FLAT/PCT_SPELL_MODIFIER mirror source (264).
    fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)>;

    /// Spend a talent point on `talent_id` (`CMSG_LEARN_TALENT`). The module gates it (points available
    /// / max rank / prerequisites); a gameplay `Err` is per-action, not session-fatal.
    fn learn_talent(&self, account_id: u64, self_guid: u64, talent_id: u32) -> Result<()>;

    /// Equip the item in main-inventory `from_slot` into its matching equipment slot
    /// (`CMSG_AUTOEQUIP_ITEM`). The module resolves the target slot from the item's `inventory_type`
    /// and validates the required-level gate; a gameplay `Err` is per-action, not session-fatal.
    fn equip_item(&self, account_id: u64, self_guid: u64, from_slot: u8) -> Result<()>;

    /// Unequip the item in equipment `from_slot` (0..=18) into a free backpack slot (right-click an
    /// equipped item → `CMSG_AUTOSTORE_BAG_ITEM`). Errors (not equipped / backpack full) are per-action.
    fn unequip_item(&self, account_id: u64, self_guid: u64, from_slot: u8) -> Result<()>;

    /// Use the consumable in main-inventory `slot` (`CMSG_USE_ITEM`) — eat/drink/potion/bandage. The
    /// module applies the item's on-use effect (flat heal for slice food) and decrements the stack.
    /// (Using a Hearthstone routes through here too — the module recalls to the bound home.)
    fn use_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()>;

    /// Bind the caller's hearthstone home to their current position (innkeeper gossip "Make this inn
    /// your home."). No args — the module resolves the caller via `ctx.sender`.
    fn bind_home(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Does the NPC at `guid` carry the innkeeper flag? Gates the "Make this inn your home." gossip
    /// option + the bind select.
    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool>;

    /// Resolve the `title_text_id` to embed in `SMSG_GOSSIP_MESSAGE` for the NPC at `guid`.
    /// Looks up `game_gossip_menu` by creature entry; falls back to 1 (generic greeting).
    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32;

    /// Look up the full weighted greeting (work-item 217, all 8 `npc_text` slots) for a `text_id`.
    /// Returns `None` when no imported `game_npc_text` row exists (the gateway falls back to the
    /// generic greeting string).
    fn npc_text_for_id(&self, text_id: u32) -> Option<codec::NpcTextView>;

    /// The imported gossip menu options for the NPC at `guid` (work-item 217), sorted by
    /// `option_index`, RAW/unfiltered by condition. Empty when nothing is imported for this creature
    /// (the gateway falls back to the flag-derived vendor/innkeeper synthesis).
    fn gossip_options(&self, npc_guid: u64) -> Result<Vec<codec::GossipOptionView>>;

    /// `(taken, rewarded)` for `quest_id` in `guid`'s quest log — feeds the QUEST_TAKEN/QUEST_REWARDED
    /// gossip option conditions (work-item 217, `codec::option_condition_holds`).
    fn quest_status(&self, guid: u64, quest_id: u32) -> (bool, bool);

    /// Move (or swap) the item in main-inventory `from_slot` to `to_slot` (`CMSG_SWAP_INV_ITEM`/
    /// `CMSG_SWAP_ITEM`). The module's move primitive validates equip-slot transitions, so this also
    /// covers drag-to-equip and drag-to-unequip. A gameplay `Err` is per-action, not session-fatal.
    fn move_item(&self, account_id: u64, self_guid: u64, from_slot: u8, to_slot: u8) -> Result<()>;

    /// Evaluate a quest giver's quests against the player for the overhead status icon + the quest
    /// menu (quests gateway slice). See `stdb::reads::quest_giver_evals`.
    fn quest_giver_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>>;

    /// Build a quest's detail view (accept / offer-reward / completion screens). `None` if unloaded.
    fn quest_detail(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>>;

    /// Accept a quest from a giver (`CMSG_QUESTGIVER_ACCEPT_QUEST`). The module gates it; a gameplay
    /// `Err` is per-action, not session-fatal.
    fn accept_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()>;

    /// Turn a completed quest in for its rewards (`CMSG_QUESTGIVER_CHOOSE_REWARD`). The module
    /// validates completion + grants money/XP/items. `reward_index` is the player's pick-1-of-N choice
    /// reward slot (the CMSG `reward` field); ignored by quests with no choice rewards.
    fn turn_in_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()>;

    /// Abandon an active quest (`CMSG_QUESTLOG_REMOVE_QUEST`). The module deletes the quest-log row;
    /// the relay clears the slot. The gateway resolves the client's log SLOT to the quest id first.
    fn abandon_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()>;

    /// Work-item 194 (item-starts-quest): does the item in `owner_guid`'s inventory `slot` carry a
    /// non-zero `start_quest`? `Some((item_guid, quest_id))` if so — `CMSG_USE_ITEM` opens the quest
    /// details screen (item guid as giver) instead of consuming it. `None` for an ordinary item.
    fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)>;

    /// Work-item 194 (sharing): share `quest_id` with the caller's party (`CMSG_PUSHQUESTTOPARTY`).
    /// The module validates the sender is grouped + actively on the quest and pushes per-member
    /// `QUEST_SHARE`/`QUEST_PUSH_RESULT` events; a gameplay `Err` (not grouped / not on the quest) is
    /// per-action, not session-fatal.
    fn push_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()>;

    /// The player's active quests as quest-log descriptor slots (Phase 2 — the L window). Empty if
    /// none. Encoded into the `PLAYER_QUEST_LOG_*` fields + sent via the raw VALUES path.
    fn player_quest_log(&self, player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>>;

    /// The player's LEARNED spells (`game_player_spell`, beyond the class kit) — chained into the
    /// login SMSG_INITIAL_SPELLS so a taught ability (e.g. Auto Shot, #10) reaches the client spellbook.
    fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>>;

    /// The player's persisted reputation standings (`game_player_reputation`) as `(reputation_index,
    /// standing)` pairs — folded into the login `SMSG_INITIALIZE_FACTIONS` (#13 slice 2) so a relog shows
    /// the real standing instead of the all-neutral stub.
    fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>>;

    /// The player's IMPORTED action-bar rows (`game_player_action`, work-item 212) as `(button,
    /// action, action_type)` triples — empty pre-import (the common case today), in which case the
    /// login codec falls back to synthesizing the bar from the spellbook (byte-identical to before
    /// this method existed).
    fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>>;

    /// The player's buyback ring, newest-first: `(item_entry, stack_count, price)` per entry
    /// (≤12). Read by the gateway to rebuild the client's buyback-tab view (248) after
    /// sell/buyback and at login — the table itself is private (coordinator-only).
    fn buyback_ring(&self, player_guid: u64) -> Vec<(u32, u32, u32)>;

    /// The rank a trainer offering actually teaches (LearnSpell wrapper → its trigger; a
    /// self-contained rank resolves to itself). Mirrors the module's buy-time resolution so
    /// SMSG_LEARNED_SPELL books the granted spell, never the wrapper.
    fn resolve_learn_target(&self, spell_id: u32) -> u32;

    /// The KNOWN rank `new_spell` supersedes (258) — Some(prev) drives SMSG_SUPERCEDED_SPELL on a buy.
    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32>;

    /// Is `guid`'s live entity currently in the world? The WORLDPORT_ACK gate (224 review
    /// finding #1): a cross-map transfer despawns the entity until the ack rebuilds it, so
    /// ABSENT = a transfer is genuinely pending; PRESENT = the ack is spurious (double-send or
    /// crafted) and must be ignored — honoring it would tear down and rebuild a live player
    /// (visible blink, gateway combat-bookkeeping reset) at zero cost to the client.
    fn entity_in_world(&self, guid: u64) -> bool;

    /// Record the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3). 0 clears it.
    fn set_target(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// Validate a `CMSG_INSPECT` request: `target_guid` must be a real in-world player, on the
    /// caller's map, in range, and friendly. `Ok(())` → the gateway replies `SMSG_INSPECT(target_guid)`;
    /// `Err` (out of range / hostile / no such target) → silently ignored, matching the other
    /// stateless-gate reducers (`enter_areatrigger`, `use_gameobject`).
    fn inspect(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1).
    fn start_attack(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// Relay a pet command-bar action (`CMSG_PET_ACTION`). `data` is the raw packed action
    /// (flag<<24 | id): flag 0x07 = command (Stay/Follow/Attack/Dismiss), flag 0x06 = react state
    /// (Passive/Defensive/Aggressive). The module decodes + validates (all pet policy lives there).
    fn pet_command(&self, account_id: u64, self_guid: u64, data: u32, target_guid: u64) -> Result<()>;

    /// Start the player's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot / 5019 wand
    /// Shoot), from `CMSG_CAST_SPELL` (#10). Requires a ranged weapon equipped (the module enforces it).
    fn start_ranged_attack(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()>;

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    fn stop_attack(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Cast a spell (`CMSG_CAST_SPELL`, aura tracer). Self-cast; target ignored.
    fn cast_spell(&self, account_id: u64, self_guid: u64, spell_id: u32, target_guid: u64) -> Result<()>;

    /// Cast a GROUND-TARGETED spell at a clicked world point (`CMSG_CAST_SPELL` with a DEST_LOCATION
    /// target block — Flamestrike/Blizzard/Rain of Fire). `(x,y,z)` is the ground click; the module
    /// anchors the AoE/patch there (118 phase 2).
    fn cast_spell_at(
        &self,
        account_id: u64, self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()>;

    /// Cancel one of the caller's own auras by spell id (`CMSG_CANCEL_AURA` — the player right-clicks a
    /// buff icon to remove it). The module deletes the matching aura on the caller; the aura relay then
    /// re-syncs the buff bar.
    fn cancel_aura(&self, account_id: u64, self_guid: u64, spell_id: u32) -> Result<()>;

    /// Cancel the caller's in-progress cast (`CMSG_CANCEL_CAST` — the player pressed Esc, moved, or
    /// recast). The module deletes the caller's pending cast so the scheduled completion never fires a
    /// phantom `SMSG_SPELL_GO` that wedges the client in "Another action is in progress". [083]
    fn cancel_cast(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// The spell's cast time (ms) from the static game_spell header — 0 = instant, None = unknown.
    /// The CMSG_CAST_SPELL handler uses it to clear instant casts synchronously. [083]
    fn spell_cast_time(&self, spell_id: u32) -> Option<u32>;

    /// The live entity's max health (0 if not in world) — the fall-damage flavor line (058) folds
    /// the shared curve against it.
    fn entity_max_health(&self, guid: u64) -> u32;

    /// True iff `spell_id` queues on the caster's next melee swing (Heroic Strike/Cleave). The
    /// CMSG_CAST_SPELL handler then sends NO synchronous START/CAST_RESULT/GO — the swing-fire
    /// emits them (114).
    fn spell_queues_next_swing(&self, spell_id: u32) -> bool;

    /// True iff `spell_id` is an auto-repeat ranged attack (Auto Shot / wand Shoot) — the
    /// `RANGED_AUTO_REPEAT` cast_flags bit. The CMSG_CAST_SPELL handler routes on this instead of a
    /// hardcoded `spell == 75 || 5019` id list; a new ranged auto-repeat ability onboards as data (097).
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool;

    /// Enchant/disenchant routing for `spell_id` from its effect rows — `None` for a normal cast. Lets
    /// the CMSG_CAST_SPELL handler route ITEM-target enchanting by effect KIND (enchant id in the effect
    /// data) instead of a hardcoded spell-id list; a new enchant is a data row, no gateway change. [094]
    fn enchant_route(&self, spell_id: u32) -> Option<EnchantRoute>;

    /// Join a chat channel (065) — the client auto-sends CMSG_JOIN_CHANNEL on zone-in.
    fn join_channel(&self, account_id: u64, self_guid: u64, channel: String) -> Result<()>;

    /// Leave a chat channel (065, CMSG_LEAVE_CHANNEL).
    fn leave_channel(&self, account_id: u64, self_guid: u64, channel: String) -> Result<()>;

    /// Speak into a joined channel (065, the CMSG_MESSAGECHAT Channel arm).
    fn send_channel_message(&self, account_id: u64, self_guid: u64, channel: String, message: String)
        -> Result<()>;

    /// Speak (`CMSG_MESSAGECHAT`, social tier): broadcast a say/yell line. `chat_type` 0 = say, 1 = yell.
    fn send_chat(
        &self,
        account_id: u64,
        self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()>;

    /// Perform an emote (`CMSG_TEXT_EMOTE`, social tier): broadcast the "X dances." line + animation.
    /// `target_guid` (0 = untargeted) is the client's selected target — the gateway resolves it to a
    /// name so the chat line reads "X waves at <target>."
    fn send_emote(
        &self,
        account_id: u64,
        self_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()>;

    /// Broadcast a `/roll` result (`MSG_RANDOM_ROLL_Client`): pick a server-side random in
    /// `[min_roll, max_roll]` and fan the result to all nearby players as `MSG_RANDOM_ROLL_Server`.
    fn send_roll(&self, account_id: u64, self_guid: u64, min_roll: u32, max_roll: u32) -> Result<()>;

    /// Whisper `message` privately to the player named `target_player` (`CMSG_MESSAGECHAT` Whisper).
    fn send_whisper(&self, account_id: u64, self_guid: u64, target_player: String, message: String) -> Result<()>;

    /// Party chat (`CMSG_MESSAGECHAT` Party, `/p`, work-item 199): deliver `message` to every OTHER
    /// current group member plus an echo to the caller, over the `game_group_event` relay (no
    /// gateway-subscribed table — see `module/src/chat.rs::party_chat`'s doc). `Err` when the caller
    /// isn't in a group ([`lyracore_shared::group::err::NOT_IN_GROUP`] — the gateway maps it to
    /// `SMSG_PARTY_COMMAND_RESULT(NotInGroup)`, "You aren't in a party") or on the other
    /// `send_chat`-style rejections (not in world / empty message), which are silently dropped like
    /// say/yell.
    fn party_chat(&self, account_id: u64, self_guid: u64, message: String) -> Result<()>;

    /// GM playtest dot-command (work-item 223): `text` is the raw Say line, STILL carrying its
    /// leading `.` — the Say handler intercepts it BEFORE any chat relay/insert and forwards it here
    /// verbatim (module-side parsing keeps the command set data-free). `Err`'s message is relayed back
    /// to the SENDER ONLY as a system chat line (never broadcast, never a `game_chat_event` row).
    fn gm_command(&self, account_id: u64, self_guid: u64, text: String) -> Result<()>;

    /// Read a corpse's lootable copper for `SMSG_LOOT_RESPONSE` (slice 3). 0 if the target is gone
    /// or not a corpse. Read-only — the actual take is `loot_money`.
    fn loot_target_money(&self, target_guid: u64) -> Result<u32>;

    /// Take the money from a corpse the player has open (`CMSG_LOOT_MONEY`, slice 3): the module
    /// validates dead+range+has-money, moves the copper to the looter, and clears the lootable flag.
    fn loot_money(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// Take one item from the open corpse's loot into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`, slice
    /// 4): the module moves the corpse-loot item in `loot_slot` into a free inventory slot and deletes
    /// the loot row. The item then appears in the bag via the inventory live-relay.
    fn take_loot(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
    ) -> Result<()>;

    /// Revive the caller after death (`CMSG_REPOP_REQUEST` / Release Spirit, slice 4): the module
    /// restores full health in place and clears the dead state (the client leaves the death screen
    /// once the restored health replicates).
    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Claim a fresh in-world session epoch (at player_login) so a stale socket's late logout can't
    /// delete a newer session's entity. The caller presents the returned epoch at teardown.
    fn claim_session(&self, account_id: u64) -> u64;

    /// Release a session epoch at teardown; returns true iff it was still current — i.e. the caller
    /// still owns the entity and may delete it. False means a newer login superseded this session.
    fn release_session(&self, account_id: u64, epoch: u64) -> bool;

    /// Register this world socket as a live user of the account's cached per-account SDK
    /// connection (#447). Called exactly once per admitted session, as soon as the handshake has
    /// resolved the account; paired with [`WorldStore::close_account_session`]. Default no-op for
    /// the mock stores.
    fn open_account_session(&self, _account_id: u64) {}

    /// Retire this world socket at teardown and, iff it was the account's LAST one, release its
    /// cached per-account SDK connections — a websocket fd plus an SDK pump OS thread per shard the
    /// account touched, which before #447 leaked for the whole process lifetime and eventually
    /// exhausted the fd table (`accept(2)` → `EMFILE` → `main` returns `Err`).
    ///
    /// ⚠ The last-socket test is the gate, and it is deliberately STRONGER than the
    /// `release_session` epoch arbitration `leave_world` uses: the epoch says only that no
    /// IN-WORLD session remains, while the cached connection is shared by every socket on the
    /// account including one parked at character select. See `stdb::AccountSessions`.
    /// Default no-op for the mock stores.
    fn close_account_session(&self, _account_id: u64) {}

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`, slice 5): the module validates the caller
    /// is a ghost owning the corpse, in range, past the reclaim delay, then resurrects at 50%.
    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()>;

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`, #014): `accept=true` revives the
    /// caller at the offer's frozen `%`; either way the offer is consumed. A failure (no pending offer
    /// for the caller) is expected when the offer already lapsed/was answered — per-action, log + ignore.
    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()>;

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`): a ghost activates the graveyard Spirit
    /// Healer to res IN PLACE at 50% health/mana + a Resurrection Sickness debuff. `healer_guid` is the
    /// activated healer's guid (passed through to the confirm echo). The module gates on ghost state.
    fn spirit_healer_res(&self, account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()>;

    /// Find `owner_guid`'s corpse location `(map_id, x, y, z)` for `MSG_CORPSE_QUERY` (slice 5).
    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>>;

    /// Return the `combat_until_ms` timestamp for `player_guid`'s entity row (0 if the entity is not
    /// found). Used by the logout handler to deny `CMSG_LOGOUT_REQUEST` while the player is in combat.
    fn player_combat_until_ms(&self, player_guid: u64) -> u64;

    /// All currently-online player characters for `CMSG_WHO → SMSG_WHO`. A player is "online" iff
    /// their guid appears in `game_world_entity` with `entry == 0` (player entity). Joined with
    /// `game_character` for name/race/class/zone; dead players are included (ghosts are online).
    fn online_players(&self) -> Result<Vec<codec::WhoPlayerView>>;

    /// `self_guid`'s friend list + ignore list (guids only) for `CMSG_FRIEND_LIST → SMSG_FRIEND_LIST`
    /// + `SMSG_IGNORE_LIST` (work-item 130). Online friends carry live presence (level/class/zone).
    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<codec::FriendView>, Vec<u64>)>;

    /// Resolve a typed contact name to a character guid (case-insensitive, like `send_whisper`'s
    /// target match), for `CMSG_ADD_FRIEND`/`CMSG_ADD_IGNORE`. `None` if no character has that name.
    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>>;

    /// A character's live presence `(online, level, class, zone_id)` for `SMSG_FRIEND_STATUS`'s
    /// Added-Online-vs-Offline split. `None` if the guid doesn't resolve to any character.
    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>>;

    /// `CMSG_ADD_FRIEND` (the name is already resolved to `target_guid` by the gateway).
    fn add_friend(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_DEL_FRIEND`.
    fn del_friend(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_ADD_IGNORE` (the name is already resolved to `target_guid` by the gateway).
    fn add_ignore(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_DEL_IGNORE`.
    fn del_ignore(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    // The SINGLE-DATABASE party path (`world::party::run`'s `None` arm). Each takes the caller's
    // `self_guid` as well as its account: the account is what identifies the player CONNECTION these
    // reducers run on, and the guid is what identifies the CHARACTER to realm-core on the other arm.
    // Both are threaded through one call site (`world::social`), so the two planes take the same
    // arguments and a mock sees which character the op was for either way.

    /// `CMSG_GROUP_INVITE` (name gateway-resolved). Module Err strings map to PartyResult codes.
    fn group_invite(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_ACCEPT`.
    fn group_accept(&self, account_id: u64, self_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_DECLINE`.
    fn group_decline(&self, account_id: u64, self_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_DISBAND` (the client's "Leave Party").
    fn group_leave(&self, account_id: u64, self_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_UNINVITE` (name gateway-resolved) — the leader kicks a member.
    fn group_uninvite(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_LOOT_METHOD` (work-item 187 slice 1) — the leader sets the party's loot method/
    /// threshold/master. `loot_setting`/`loot_threshold` are the gateway-decoded `GroupLootSetting`/
    /// `ItemQuality` wire bytes, passed straight through (the module adopted the wire ordering
    /// verbatim — zero translation).
    fn group_loot_method(
        &self,
        account_id: u64,
        self_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()>;
    /// `CMSG_LOOT_ROLL` (work-item 187 slices 2-3) — record the caller's need/greed/pass vote.
    fn loot_roll(&self, account_id: u64, self_guid: u64, corpse_guid: u64, loot_slot: u32, vote: u8) -> Result<()>;
    /// `CMSG_LOOT_MASTER_GIVE` (work-item 187 slice 4) — the master looter assigns an above-
    /// threshold row to `target_guid`.
    fn loot_master_give(
        &self,
        account_id: u64, self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<()>;
    /// NOTIFY-ONLY module chokepoint for a gossip-option click (work-item 146) — fired best-effort
    /// before the gateway's own gossip handling; failure never blocks the gossip reply.
    fn gossip_select(
        &self,
        account_id: u64, self_guid: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()>;
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
    /// `CMSG_CANCEL_AUTO_REPEAT_SPELL` (work-item 097).
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
    /// Work-item 231: per-connection movement coalescing. Lives directly on `WorldConn` (not
    /// `InWorld`) so it survives a world-port/relogin boundary uneventfully — it only ever sees
    /// traffic while `InWorld` movement is being dispatched, but there's no correctness reason to
    /// tear it down on a state transition, and keeping it here avoids re-plumbing it through
    /// `enter_world`.
    move_coalesce: CoalesceState,
    /// Non-blocking movement submit state (perf catalog 1.13, #110): outstanding submissions and
    /// the module's deferred verdict. `Arc` because the SDK completion callback outlives this call.
    move_feedback: std::sync::Arc<MovementFeedback>,
    /// Movement packets dropped because `MAX_IN_FLIGHT_MOVES` was already outstanding. Logged at
    /// teardown next to the coalescing ratio — if this is large the server is behind, and it is the
    /// number that says so rather than a silent stall.
    move_submit_dropped: u64,
    /// Multi-shard routing (#17): the HOME-shard store handle for the character this session is
    /// playing, resolved at `CMSG_PLAYER_LOGIN` (and re-resolved on a world-port, which can change
    /// map and therefore shard) from the character's location via `WorldStore::home_shard`. `None`
    /// = "the handle the listener already handed us is the right one" — the single-shard default,
    /// and the state of every pre-login message, so nothing about the old path changes.
    ///
    /// This is derived, not authoritative: it is recomputed from durable state on every world
    /// entry, so a gateway restart mid-session re-derives it on the client's relog (AC#5) — the
    /// gateway stays stateless w.r.t. sessions.
    home: Option<std::sync::Arc<dyn WorldStore>>,
    /// The shared session key K, kept from the world handshake so a session that lands on a NON-
    /// realm shard (#19) can call `establish_session` THERE — the module's `account_by_identity`
    /// gate on `player_login` needs the account row on that database bound to this connection's
    /// identity, and the logon tier only ever bound it on the realm database.
    ///
    /// NOT game state, and not a new secret: it is the same K the header cipher on this very socket
    /// is already keyed by, read from `game_session` moments earlier in `world_handshake`. `None`
    /// for sessions built by tests that never handshake.
    session_key: Option<[u8; 40]>,
    /// Issue #39: consecutive movement packets dropped because the module answered a DESYNC ("mover
    /// not in world"). Reset by the first movement the module accepts. See
    /// [`MOVE_DESYNC_TOLERANCE`] for why the tolerance is bounded rather than unconditional.
    move_desync_drops: u32,
}

/// How many CONSECUTIVE desynced movement packets a session may drop before the desync is treated
/// as real and the session ends (issue #39, adversarial review).
///
/// The tail of a cross-map port is a handful of packets: the client stops sending movement the
/// instant `SMSG_TRANSFER_PENDING` puts it on the loading screen, so only what was already in
/// flight (hundreds of ms at ~10 packets/s) can land on the despawned entity. A desync that keeps
/// answering past that is the OTHER kind — the entity is gone for good (a schema-change publish tore
/// down the coordinator subscription; the row was deleted under this socket) — and for that one,
/// closing the socket IS the fix: the client shows "Disconnected" and re-materialises everything
/// from durable state on relog. Swallowing it forever would trade #39's loading-screen hang for a
/// worse one: a player walking around a frozen world, invisible to peers, never disconnected, with
/// no error and no recourse — which is exactly what `is_desync_error`'s session-fatal treatment
/// exists to prevent. 32 is ~3 s of a moving client, an order of magnitude more than any port tail.
const MOVE_DESYNC_TOLERANCE: u32 = 32;

/// Run `$body` against the session's HOME-shard store handle (#17). `$store` is the handle the
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

impl WorldConn {
    /// Re-pin this session to the shard that owns `character_guid`'s location (#17). Asked of the
    /// handle currently in use, so a `None` answer means "this handle already owns it" and the
    /// current pin is KEPT — clearing it would silently drop the session back to the default shard.
    /// Called at every world entry (login and world-port), never in between: routing is derived
    /// from durable state, so nothing here survives a gateway restart that shouldn't.
    /// Returns the transfer's error rather than swallowing it (#19): a half-moved character must
    /// fail its login loudly, never proceed into the world on whichever shard happened to answer.
    /// Both are recoverable — the escrow holds and the next login re-drives it — but only one of
    /// them is honest to the player.
    fn route_home<St: WorldStore + ?Sized>(
        &mut self,
        store: &St,
        character_guid: u64,
    ) -> Result<()> {
        // `settle_home_shard`, not `home_shard`: for a session that actually crosses a database
        // boundary this RUNS the escrowed transfer (#19) before answering. Its default impl IS
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
    /// identity — only the gateway can tell the sockets apart, #42). Returns the `logout` result so
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
            // The `logout` reducer must delete the entity on the shard it LIVES on (#17), so this
            // runs on the home shard like every other player-scoped call. Session epochs are
            // gateway-local and shared across shards, so the #42 arbitration is unaffected.
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
            // RELEASE the home-shard pin (#17): the socket stays open at character select, and
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

/// How often a queued connection re-checks whether it has been admitted (#180). Cheap — one mutex
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
/// [`LoginQueue`] (`LoginQueue::unlimited()`) — see [`world_handshake_with_queue`] for the #180
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

/// Run the world auth handshake over `stream` (gateway-translation.md §3), gated by `queue` (#180).
/// On success returns the established per-connection crypto + account id; on a clean auth failure
/// sends the failure response and returns `Ok(None)`; on a clean disconnect also returns `Ok(None)`.
///
/// Sequence: plaintext `SMSG_AUTH_CHALLENGE` (server seed) -> read plaintext
/// `CMSG_AUTH_SESSION` -> look up K -> validate the client proof digest via `wow_srp` (which
/// also yields the header cipher) -> **admission gate** (#180: queue here if the world is full,
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

    // 4.5. Admission gate (#180). `queue.request()` on an unlimited queue (the default —
    // `LYRACORE_MAX_SESSIONS` unset) returns `Admitted` immediately with no bookkeeping: a no-op for
    // every gateway that hasn't opted in. Otherwise a full house queues the connection here, BEFORE
    // it can reach CMSG_CHAR_ENUM or CMSG_PLAYER_LOGIN's `subscribe_player_events` — the expensive
    // part #180 measured (38% of the writer at 500 sessions).
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
            move_feedback: std::sync::Arc::new(MovementFeedback::default()),
            move_submit_dropped: 0,
            home: None,                     // resolved at CMSG_PLAYER_LOGIN (#17)
            session_key: Some(session_key), // for establish_session on a non-realm shard (#19)
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

/// #209 probe: cheap FNV-1a-64 over a byte slice — not cryptographic, just enough to tell "same
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

/// #209 probe: per-session black box for [`spawn_writer`] — the last [`WriterTrace::CAPACITY`]
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
/// `account_id` exists ONLY to name the `LYRACORE_WRITER_TRACE=1` crash dump (#209 probe) — every other
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
                // #468: a deferred relay job runs HERE, on this session's own writer thread, never
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
                        // Issue #209 hardening: a body that overflows the u16 size field would
                        // silently WRAP via `as u16` — the header would then declare a small size
                        // while `write_all` below still sends the full (large) body, so the client's
                        // reader consumes the wrong number of bytes for this frame and every LATER
                        // header on this connection decrypts from the wrong stream offset (a real
                        // desync, indistinguishable from the crash class #209 investigated). This used
                        // to be a `debug_assert!`, which compiles out entirely in the release profile
                        // the capacity benchmark and any live deploy actually run — i.e. the one guard
                        // against this class was absent exactly where it would matter. No current
                        // builder produces a body anywhere near this size, so this arm is not known to
                        // fire; it exists so that if one ever does, the connection dies cleanly
                        // instead of corrupting itself and every packet after it.
                        if body.len() > u16::MAX as usize - 2 {
                            log::error!(
                                "packet-lint VIOLATION opcode=0x{opcode:04X}: raw body is {} bytes — \
                                 exceeds the u16 frame-size field and would silently wrap (issue #209 \
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
/// [`run_world_session_with_queue`] for the #180 admission gate itself. Every existing call site
/// (tests, and anything that doesn't care about #180) stays byte-identical to pre-#180 behavior.
/// Test-only, for the same reason as [`world_handshake`]: production drives
/// [`run_world_session_with_queue`] with the listener's shared queue.
#[cfg(test)]
pub fn run_world_session<S: DuplexStream, St: WorldStore + ?Sized>(
    stream: S,
    store: &St,
) -> Result<()> {
    run_world_session_with_queue(stream, store, &LoginQueue::unlimited())
}

/// Drive one world connection: handshake (synchronous, sole writer), then split into a reader
/// (this thread, owns `DecrypterHalf`) and a writer thread (owns `EncrypterHalf` + a socket clone)
/// bridged by an mpsc channel. The reader handles requests and pushes responses; the per-player
/// subscription callbacks (on the SDK pump thread) push peer spawn/move/destroy. On disconnect the
/// reader tears down the subscriptions, logs the player out (removes the entity → peers see
/// `DESTROY`), drops its sender (ending the writer), and joins it.
///
/// #180: `world_handshake_with_queue` may block here (before any of the above starts) if the world
/// is full — the client sees native "Position in queue" screens and this thread just waits, which
/// costs nothing but ITS OWN thread (the listener in `run` spawns one `spawn_blocking` task per
/// accepted socket, so a queued connection never holds up anyone else's accept or handshake). Once
/// admitted (`Ok(Some(..))`), this connection holds a seat in `queue` for the rest of the function —
/// released in the teardown below exactly once, no matter which branch got there.
pub fn run_world_session_with_queue<S: DuplexStream, St: WorldStore + ?Sized>(
    mut stream: S,
    store: &St,
    queue: &LoginQueue,
) -> Result<()> {
    let Some((mut conn, encrypt)) = world_handshake_with_queue(&mut stream, store, queue)? else {
        return Ok(());
    };

    let wsock = stream
        .try_clone()
        .map_err(|e| anyhow!("clone world socket for writer: {e}"))?;
    let (tx, rx, depth) = session_channel();
    let writer = spawn_writer(wsock, encrypt, rx, depth, conn.account_id)?;

    // #447: this socket is now a live user of the account's cached per-account SDK connection.
    // Placed AFTER the last `?` above so every path from here reaches the paired
    // `close_account_session` in the teardown block below (`result` is captured, not propagated,
    // so no in-session error can skip it). Registering earlier would let a `try_clone`/`spawn_writer`
    // failure pin the account's connection for the process lifetime — which is exactly the leak.
    store.open_account_session(conn.account_id);

    let result = (|| -> Result<()> {
        // 184: frames are read RAW (header hand-decrypted) so an addon-language chat — which
        // gtker's `Language` enum cannot decode and which was session-FATAL — can be peeked and
        // routed to the bridge BEFORE typed parsing. Every other frame is re-framed unencrypted
        // and handed to the same gtker parser as before (one memcpy per packet).
        // (`std::io::Read` is already in scope from the file-level import — the raw reads below use
        // it; a second local `use` is a duplicate, not a requirement.)
        // Addon-bridge rate limit (184): token bucket per connection — 2 tokens/s, burst 20.
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
            // Every in-world message is dispatched against the player's HOME shard (#17); until
            // CMSG_PLAYER_LOGIN resolves one (and always, with a single-entry shard map) this is
            // the handle the listener passed in — the unchanged single-database path.
            on_home_shard!(conn, store, |st| dispatch(&tx, st, &mut conn, msg))?;
        }
    })();

    // Work-item 231 measurement (rule 5): how much did coalescing actually cut? Logged once per
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
    // player already re-logged on a newer session declines, so we don't vanish the live player (the
    // cached PlayerConn shares one identity, so only the gateway can tell the sockets apart, #42).
    // Teardown is already ending the session, so a `logout` failure is logged + swallowed (not fatal).
    if let Err(e) = conn.leave_world(store) {
        log::warn!("logout for account {} failed: {e:#}", conn.account_id);
    }
    // #447: retire this socket. When it is the account's LAST one, this releases the cached
    // per-account SpacetimeDB connections (one websocket fd + one SDK pump thread per shard the
    // account touched) that otherwise leak for the gateway's lifetime and eventually exhaust the
    // fd table. Strictly AFTER `leave_world`: that is what drops the AOI tracker (unsubscribing the
    // home rects and every away shard) and runs the `logout` reducer, all of which go THROUGH the
    // connections released here. A reconnect that beat this teardown still holds a socket of its
    // own, so the count is non-zero and nothing is released — see `stdb::AccountSessions`.
    store.close_account_session(conn.account_id);
    // #180: release this connection's seat unconditionally — reaching this line at all means
    // `world_handshake_with_queue` returned `Ok(Some(..))`, which is exactly the one case where it
    // guarantees a seat is held and NOT already departed (its own internal failure path departs
    // before ever returning `Some`). A no-op for the unlimited queue.
    queue.depart();
    drop(tx);
    let _ = writer.join();
    result
}

/// Route one addon-language chat frame (184): parse the `STC` v1 envelope and forward to the
/// module's `client_command` reducer as the player. Foreign prefixes and malformed envelopes
/// drop silently-with-a-debug-line (other servers' addons share the airwaves by accident);
/// reducer errors log and drop — an addon frame is NEVER session-fatal.
fn handle_addon_message<St: WorldStore + ?Sized>(store: &St, conn: &WorldConn, text: &str) {
    let Some((cmd, payload)) = codec::addon::parse_bridge_envelope(text) else {
        log::debug!("addon bridge: non-STC or malformed frame dropped: {text:?}");
        return;
    };
    if let Err(e) = store.client_command(conn.account_id, social::self_guid(conn).unwrap_or(0), cmd.clone(), payload) {
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
/// client on a loading screen (#39). Past that bound it is fatal like everything else.
fn is_desync_error(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("not in world") || s.contains("no live entity")
}

/// Tell a client whose world-port cannot complete that it is off, so its loading screen ends with an
/// error instead of never ending (issue #39 AC#4). Best-effort and infallible by design: it runs on
/// a path that is already failing, and every one of its own failure modes (an unmapped destination
/// map, a dead socket) is strictly less bad than the hang it replaces, so none of them may mask the
/// original error the caller is about to propagate.
///
/// The destination map comes from the character's own durable row — the same row
/// `world::teleport_player` wrote the destination into before it despawned the entity, i.e. the map
/// the client is loading right now. `TransferAbortReason::NotFound` is the closest vanilla reason to
/// "the shard that owns this instance would not take you"; the operator-facing detail is the log line.
fn abort_pending_transfer<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    character_guid: u64,
    cause: &anyhow::Error,
) {
    let dest_map = store
        .character_destination(character_guid)
        .map(|p| p.dest_map_id);
    log::error!(
        "world: world-port for guid {character_guid} cannot complete ({cause:#}) — aborting the \
         client's transfer to map {dest_map:?}. The character is unharmed: the escrow is idempotent \
         and the next login re-drives it."
    );
    let Some(map_id) = dest_map else {
        log::warn!(
            "world: no durable destination for guid {character_guid} — the client gets no \
             SMSG_TRANSFER_ABORTED and will need to reconnect"
        );
        return;
    };
    use wow_world_messages::vanilla::TransferAbortReason;
    match codec::build_transfer_aborted(map_id, TransferAbortReason::NotFound) {
        Ok(msg) => {
            let _ = send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(msg)),
            );
        }
        Err(e) => {
            log::warn!("world: could not build SMSG_TRANSFER_ABORTED for map {map_id}: {e:#}")
        }
    }
}

/// Build + send the player's quest-log descriptor fields as a raw VALUES update (Phase 2). A no-op
/// when the gate is off or the player has no active quests. Shared by login (initial sync) and the
/// quest-log relay (on accept / progress / turn-in). A full sync of all 20 slots, so removals clear.
fn send_quest_log<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    player_guid: u64,
) -> Result<()> {
    if !crate::config::quest_log_fields_enabled() {
        return Ok(());
    }
    let slots = store.player_quest_log(player_guid).unwrap_or_default();
    if slots.is_empty() {
        return Ok(()); // no quests → nothing to show; the CREATE packet already zeroed these fields
    }
    let mask = codec::update_mask::full_quest_log_mask(&slots);
    let (opcode, body) = codec::build_values_update_raw(player_guid, &mask);
    send(tx, Outbound::Raw { opcode, body })
}

/// Route one decrypted client message. Threads it through the per-family `handle_*` free fns (the
/// carve of the former dispatch god-match): each consumes its own opcodes (`Ok(None)`) or passes the
/// message on (`Ok(Some(msg))`), so the disjoint-family chain ends in the movement-relay catch-all.
fn dispatch<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<()> {
    // Work-item 231, rule 2 (the robust flush): a pending coalesced heartbeat may leave the
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
    let Some(msg) = handle_combat(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_loot(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_vendor(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_trainer(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_item(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_quest(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_social(tx, store, conn, msg)? else {
        return Ok(());
    };
    let Some(msg) = handle_query(tx, store, conn, msg)? else {
        return Ok(());
    };
    // Phase 5/6 (§6): MSG_MOVE_* -> movement_update (persist + relay). The relayed peer events
    // come back on this player's game_movement_event subscription and are re-emitted (same
    // opcode + verbatim MovementInfo) to other players by their own subscription callbacks.
    //
    // Work-item 231: every inbound movement packet is classified + coalesced by `CoalesceState`
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
/// exactly what the pre-231 inline code did, unconditionally, for every packet. Now called once
/// per packet `CoalesceState` decides to forward (immediately for a state change, later for a
/// coalesced heartbeat), so a forwarded packet's on-wire effect is unchanged; only its TIMING can
/// differ (a pure heartbeat may forward later than it arrived; nothing else does).
fn forward_movement<St: WorldStore + ?Sized>(
    store: &St,
    conn: &mut WorldConn,
    opcode: u32,
    info: &MovementInfo,
) -> Result<()> {
    // #468 stage 4b: the shared-calls path names the mover by guid instead of by connection.
    // Movement only flows in-world, so the guid is always known here; 0 (its absence) forces the
    // per-player path in the store.
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };
    // A movement packet for an entity that is GONE is not a session-fatal desync — it is the normal
    // tail of a cross-map teleport (issue #39 defect 2). `teleport_player` despawns the live entity
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
    // ...but the tolerance is BOUNDED (adversarial review of #39). "Not in world" is also the answer
    // for a desync that will NEVER heal, and movement is the highest-frequency detector we have; an
    // unconditional swallow turns a socket that used to close cleanly into a player walking around a
    // frozen world forever, never disconnected, with no error — the very outcome `is_desync_error`
    // was introduced to prevent. So: drop the port tail, then give up. See MOVE_DESYNC_TOLERANCE.
    // NON-BLOCKING SUBMIT (perf catalog 1.13, #110). This runs on the session's own socket-reader
    // thread, so blocking here meant a player's NEXT packet could not be read until the previous
    // movement round-tripped to the database and back — measured as a hard ~200 committed
    // movement_update/s across the WHOLE server, unchanged from 100 to 200 players, with the
    // database contributing 2.1 ms of a ~996 ms round-trip.
    //
    // BACKPRESSURE: past MAX_IN_FLIGHT_MOVES outstanding submissions we drop this packet rather than
    // queue it. That is not a loss of fidelity — a movement packet is a POSITION SNAPSHOT, the next
    // one supersedes it, and the coalescer already discards intermediate heartbeats for exactly this
    // reason. Without a bound, fire-and-forget converts a throughput ceiling into unbounded memory.
    let feedback = conn.move_feedback.clone();
    if feedback.in_flight() >= MAX_IN_FLIGHT_MOVES {
        conn.move_submit_dropped += 1;
        return Ok(());
    }
    // The module's verdict arrives on a LATER callback, so apply whatever the previous submissions
    // reported before sending this one — the desync-tolerance behaviour below is unchanged, only
    // its input is now deferred by a packet or two.
    if let Err(e) = feedback
        .take_err()
        .map_or(Ok(()), |e| Err(anyhow!(e)))
        .and_then(|_| store.movement_update_nowait(conn.account_id, self_guid, opcode, info, &feedback))
    {
        if !is_desync_error(&e) {
            return Err(e);
        }
        conn.move_desync_drops += 1;
        if conn.move_desync_drops > MOVE_DESYNC_TOLERANCE {
            log::warn!(
                "world: account {} has desynced on {} consecutive movement packets — this is not a \
                 teleport tail, ending the session so the client relogs from durable state: {e:#}",
                conn.account_id,
                conn.move_desync_drops
            );
            return Err(e);
        }
        log::debug!(
            "world: dropping a movement packet for a despawned entity (account {}, drop {}): {e:#}",
            conn.account_id,
            conn.move_desync_drops
        );
        return Ok(());
    }
    conn.move_desync_drops = 0;
    // AOI: recenter this player's grid-scoped entity subscription if they crossed a cell. No-op
    // when AOI is disabled (no tracker) or the player stayed in-cell. Same-map (the tracker holds
    // the login map; teleport/zone changes re-anchor in a later phase).
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.subs.aoi_update(info.position.x, info.position.y);
    }
    Ok(())
}

/// Rebuild + push the buyback-tab view (248): a synthesized ITEM object per ring entry (fabricated
/// guid 0x4090…|slot — a client-only object, never a real instance) and ONE raw VALUES update
/// carrying all 12 VendorBuyback INV_SLOT pointers + BUYBACK_PRICE/TIMESTAMP arrays (the price/
/// timestamp arrays are gtker-walled past slot 0 → the shared raw encoder). Cleared slots write
/// guid 0 / price 0, so ring shifts and evictions render correctly without tracking prior state.
fn push_buyback_view<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    self_guid: u64,
    skip_if_empty: bool,
) -> Result<()> {
    let ring = store.buyback_ring(self_guid);
    log::debug!("buyback view: guid={self_guid} ring_len={}", ring.len());
    // Login replay of an EMPTY ring is a no-op by construction (the client's descriptor fields
    // start zeroed) — skipping keeps the login sequence byte-identical for ring-less players.
    // In-session callers always push (a ring that just BECAME empty must clear the tab).
    if skip_if_empty && ring.is_empty() {
        return Ok(());
    }
    let mut mask = codec::update_mask::UpdateMaskValues::new();
    for i in 0..12u16 {
        let (fab_guid, price) = match ring.get(i as usize) {
            Some(&(entry, count, price)) => {
                let fab_guid = 0x4090_0000_0000_0000u64 | u64::from(i);
                let view = codec::ItemInstanceView {
                    guid: fab_guid,
                    entry,
                    owner_guid: self_guid,
                    slot: 69 + i as u8,
                    stack_count: count,
                    durability: 0,
                    max_durability: 0,
                    container_slots: 0,
                };
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                        codec::build_item_create_object(&view),
                    ))),
                )?;
                (fab_guid, price)
            }
            None => (0, 0),
        };
        // PLAYER_FIELD_INV_SLOT guid pair for VendorBuyback slot 69+i (base 486, 2 words/slot);
        // BUYBACK_PRICE_1 = 1226, BUYBACK_TIMESTAMP_1 = 1238 (5875 indices via gtker impls).
        mask.set_u64(486 + (69 + i) * 2, fab_guid);
        mask.set_u32(1226 + i, price);
        mask.set_u32(1238 + i, 0);
    }
    let (opcode, body) = codec::build_values_update_raw(self_guid, &mask);
    send(tx, Outbound::Raw { opcode, body })
}

// Per-family dispatch handlers — code-motion of the former dispatch match arms (bodies verbatim,
// incl. in `handle_combat` the two session-fatal `is_desync_error` early-exits on ATTACKSWING/STOP).
// Each returns `Ok(None)` once it consumes its opcode, else `Ok(Some(msg))` to pass the message on.

/// Enter (or RE-enter, work-item 224) the world as `character_guid`: rebuild the live entity, subscribe
/// to its per-player views (a FRESH `created` dedup set every call — the full AOI reset a cross-map
/// re-entry needs), and send the login sequence + self CREATE_OBJECT as one contiguous batch. Shared by
/// `CMSG_PLAYER_LOGIN` (fresh world entry) and `MSG_MOVE_WORLDPORT_ACK` (cross-map re-entry after
/// `teleport_player` despawned the old entity) — see their call sites' doc comments for why reusing this
/// exact path is correct for both. `session_epoch` is the caller's to manage: a fresh login claims a new
/// one; a world-port reuses the existing one (the session itself hasn't changed).
///
/// Drops any PREVIOUS `InWorld` state FIRST (a world-port re-entry has one, scoped to the OLD map/AOI
/// box; a fresh login doesn't) — the old `PlayerSubscriptions`' RAII `Drop` unregisters every callback +
/// tears down the old AOI tracker before this registers new ones, so nothing double-fires and nothing
/// leaks a stale grid subscription pointed at a map the player already left.
fn enter_world<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    character_guid: u64,
    session_epoch: u64,
) -> Result<()> {
    conn.state = WorldState::CharSelect;

    let mut entity = store.player_login(conn.account_id, character_guid)?;
    // Character sheet (UNIT_FIELD_RESISTANCES[0]): override the BASE armor `player_login` set with
    // the EFFECTIVE armor (base + worn gear) so the Armor readout is correct at relog. Armor auras
    // self-correct via the on_aura relay; combat mitigation is unchanged (the module still folds its
    // own effective_armor on demand — this only feeds the display descriptor).
    entity.effective_armor = store.effective_armor(character_guid);
    log::info!(
        "world: entering world guid={character_guid} -> entity at map {} ({:.1},{:.1},{:.1}); subscribing + sending login sequence + self-spawn",
        entity.map_id, entity.x, entity.y, entity.z
    );
    // Items slice-1: the character's owned items. Each becomes an item CREATE_OBJECT sent
    // BEFORE the player self-spawn (so the inventory-slot guid resolves to an object the
    // client already has), and the (slot, guid) pairs seed the player's PLAYER_FIELD_INV_SLOT
    // descriptors. Empty for a character that owns nothing — login is otherwise unchanged.
    let items = store.player_items(character_guid).unwrap_or_default();
    let inventory: Vec<(u8, u64, u32)> = items.iter().map(|i| (i.slot, i.guid, i.entry)).collect();
    let learned = store
        .player_learned_spells(character_guid)
        .unwrap_or_default();
    let skills = store.player_skills(character_guid).unwrap_or_default();
    let reputations = store.player_reputations(character_guid).unwrap_or_default();
    // Work-item 212: imported action-bar rows (empty pre-import — the login codec falls back
    // to synthesizing the bar from `learned` in that case, byte-identical to before).
    let player_actions = store.player_actions(character_guid).unwrap_or_default();
    let mut batch =
        codec::login_sequence_messages(&entity, &learned, &reputations, &player_actions)?;
    for item in &items {
        batch.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_item_create_object(item),
        )));
    }
    batch.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
        codec::build_create_object(&entity, codec::CreateKind::SelfPlayer, &inventory, &skills)?,
    )));
    send(tx, Outbound::Batch(batch))?;
    // Subscribe AFTER the self-spawn batch is on the wire — so the AOI initial-apply creates for
    // entities ALREADY in view (notably a questgiver you spawn right next to) arrive AFTER the
    // client is in-world. Spawning ON a questgiver otherwise left it targetable but with no '!' /
    // right-click: its CREATE raced the login sequence and the client registered it as a plain
    // unit, never polling its quest status. The streaming path (a peer entering view later) was
    // always fine — this makes the login case match it. (work-item 115; missing a peer that
    // inserts in the µs window between this send and the subscribe is negligible.) The dedup set
    // is seeded with self_guid so the player's own row (delivered on initial apply) is skipped.
    let subs = store.subscribe_player_events(
        conn.account_id,
        character_guid,
        entity.instance_id,
        entity.map_id,
        entity.x,
        entity.y,
        tx.clone(),
    )?;
    // 248: replay the buyback-tab view (the ring persists across sessions; without this the tab
    // is empty until the first in-session sell).
    push_buyback_view(tx, store, character_guid, true)?;
    // #22 (group slice): put realm-core's party roster onto the shard this character just entered
    // and re-render the party frame. THIS is what carries a party across a shard boundary now that
    // #19's blob mirror is gone — and it runs on every world entry, so a party formed while the
    // player was on the loading screen lands too. A no-op on a single-database gateway.
    //
    // Failures are logged, not propagated: a party frame that renders late is a cosmetic defect,
    // and failing the login over it would be strictly worse for the player.
    if let Err(e) = party::on_world_entry(tx, store, character_guid) {
        log::warn!("world: party sync at world entry failed for guid {character_guid}: {e:#}");
    }
    // Enter the world: CharSelect → InWorld (a reused connection has no open loot/attack — a world-port
    // re-entry likewise starts clean, since whatever the player was attacking/looting on the old map is
    // meaningless on the new one).
    conn.state = WorldState::InWorld(InWorld {
        self_guid: character_guid,
        subs,
        session_epoch,
        attacking_target: None,
        looting_target: None,
        ranged_repeat: false,
    });
    // Phase 2: the quest-log window. Sent as a separate raw VALUES update AFTER the CREATE
    // (gtker's CREATE can't carry these walled fields), gated behind LYRACORE_QUEST_LOG until verified.
    send_quest_log(tx, store, character_guid)?;
    // #10: if the player carries ammo (a Projectile item, class 6), tell the client it's loaded
    // (PLAYER_AMMO_ID) so Auto Shot is usable. Deliberate simplification: login-time only — no
    // live re-send on pickup/runout (the next login re-syncs; the shot itself gates on the bag
    // having ammo).
    if let Some(ammo_entry) = items.iter().map(|i| i.entry).find(|&e| {
        store
            .item_template(e)
            .ok()
            .flatten()
            .is_some_and(|t| t.class == 6)
    }) {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_player_ammo_id_values(character_guid, ammo_entry),
            ))),
        )?;
    }
    // Talent-pane points correction: the CREATE's PLAYER_CHARACTER_POINTS1 counts points EARNED
    // (level−9; codec/entity.rs), so a character with SPENT points over-reports until a pick.
    // Push the true remaining once, post-CREATE (same partial-VALUES mechanism as the live pick).
    // Skipped for spent == 0 → a fresh character's login stays byte-identical.
    if store.talent_points_spent(character_guid) > 0 {
        let (_, _, remaining) = store.talent_pane_sync(character_guid, 0);
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_talent_points_values(character_guid, remaining),
            ))),
        )?;
    }
    // Spell-modifier mirror (264): tell the client which of its spells the learned passives modify
    // (Improved Fireball's cast-time cut etc.) so ITS cast bars/tooltips match the server's folded
    // timings. One packet per (op, mask-bit) total, the mangos convention; none learned → nothing.
    for m in codec::build_spell_modifier_msgs(&store.spell_modifiers(character_guid)) {
        send(tx, Outbound::One(m))?;
    }
    Ok(())
}

/// Char / world-entry family (§4/§5): character enum + creation (character-select), then enter world
/// (`CMSG_PLAYER_LOGIN`) + graceful logout — the session-lifecycle opcodes.
fn handle_char<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // Phase 3 (§4): character-select screen.
        ClientOpcodeMessage::CMSG_CHAR_ENUM => {
            let characters = store.characters(conn.account_id)?;
            let enum_msg = codec::build_char_enum(&characters)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_ENUM(Box::new(enum_msg))),
            )?;
        }
        // Character creation. Create the row, reply SMSG_CHAR_CREATE; on success the client
        // re-sends CMSG_CHAR_ENUM and the new character appears. A creation failure is NOT
        // session-fatal — report it as a result, never drop the connection.
        ClientOpcodeMessage::CMSG_CHAR_CREATE(c) => {
            let appearance = codec::Appearance {
                skin: c.skin_color,
                face: c.face,
                hair_style: c.hair_style,
                hair_color: c.hair_color,
                facial_hair: c.facial_hair,
            };
            let outcome = store
                .create_character(
                    conn.account_id,
                    c.name.as_str(),
                    c.race.as_int(),
                    c.class.as_int(),
                    c.gender.as_int(),
                    appearance,
                )
                .unwrap_or(codec::CharCreateOutcome::Failed);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_CREATE(
                    codec::build_char_create_response(outcome),
                )),
            )?;
        }
        // Character deletion (work-item 081). Per the wire doc SMSG_CHAR_DELETE alone updates the
        // character-select screen — no re-sent CMSG_CHAR_ENUM needed. Ownership is enforced module-
        // side; a failure is NOT session-fatal, same treatment as CMSG_CHAR_CREATE above.
        ClientOpcodeMessage::CMSG_CHAR_DELETE(d) => {
            let outcome = store
                .delete_character(conn.account_id, d.guid.guid())
                .unwrap_or(codec::CharDeleteOutcome::Failed);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_DELETE(
                    codec::build_char_delete_response(outcome),
                )),
            )?;
        }
        // Phase 4 (§5): enter world -> register peer subscriptions, then login sequence + self
        // CREATE_OBJECT2 as one contiguous batch (so an async peer event can't splice into it).
        ClientOpcodeMessage::CMSG_PLAYER_LOGIN(p) => {
            let character_guid = p.guid.guid();
            // Claim this account's in-world session: become the current owner of the live entity so a
            // stale earlier socket's teardown can't delete it out from under us (#42). A world-port
            // re-entry (below) reuses the EXISTING epoch instead — the session itself hasn't changed,
            // only the map.
            let session_epoch = store.claim_session(conn.account_id);
            // Multi-shard routing (#17): pin this session to the shard that owns the character's
            // location BEFORE `player_login` runs, so the login reducer, the per-player connection
            // it opens, and the AOI subscriptions all land on the home shard — and so does every
            // message after this one (`run_world_session` re-reads `conn.home` per frame).
            // A single-entry shard map never pins anything → `enter_world` runs on `store`, as it
            // always did.
            conn.route_home(store, character_guid)?;
            on_home_shard!(conn, store, |st| enter_world(
                tx,
                st,
                conn,
                character_guid,
                session_epoch
            ))?;
        }
        // Work-item 224 (cross-map teleport): the client's ack that it finished loading the map named
        // by our `SMSG_NEW_WORLD` (sent from the `on_teleport` relay when `teleport_player` despawned
        // the live entity for a cross-map hop). Per gtker's own doc comment on this opcode — "The server
        // should reply with what it normally does to log players into the world" — so this reuses the
        // EXACT same `enter_world` path CMSG_PLAYER_LOGIN uses: rebuild the entity (`player_login` is
        // idempotent here — the ghost-relog branch no-ops because the entity is ALREADY gone), tear down
        // the OLD map's subscriptions and register fresh ones at the new map/position (a brand new
        // `created` dedup set — the full AOI reset this work item requires, the same "initial-subscribe"
        // precedent 145 established), and re-send the (now new-map) login sequence + self CREATE_OBJECT.
        // A spurious/late ack while not mid-transfer (e.g. a double-send) is a no-op — CharSelect has no
        // `self_guid` to re-enter with, so it's silently accepted-and-ignored like every other unsolicited
        // client ack in this dispatch. The session epoch is REUSED (not re-claimed) — nothing about
        // session ownership changed, only the entity/map.
        ClientOpcodeMessage::MSG_MOVE_WORLDPORT_ACK => {
            let resume = match &conn.state {
                WorldState::InWorld(iw) => Some((iw.self_guid, iw.session_epoch)),
                WorldState::CharSelect => None,
            };
            if let Some((character_guid, session_epoch)) = resume {
                // Gate on a REAL pending transfer (224 review finding #1): cross-map teleport
                // despawns the entity until this ack; a live entity means no transfer is in
                // flight and the ack is spurious — ignore it instead of re-entering the world.
                if store.entity_in_world(character_guid) {
                    log::debug!("world: spurious WORLDPORT_ACK ignored (guid {character_guid} still in world)");
                } else {
                    // A world-port changes the map, which can change the owning shard (#17) —
                    // re-resolve before re-entering, exactly as a fresh login does. This is also
                    // where the escrowed cross-database transfer (#19) actually RUNS.
                    //
                    // FAIL LOUDLY, NEVER HANG (issue #39). The client is on a loading screen it
                    // entered because we sent it `SMSG_TRANSFER_PENDING`, and the only thing that
                    // ends that screen is us finishing the world entry. Propagating the error here
                    // closes the socket mid-load, which the 1.12 client renders as an infinite
                    // loading bar — the player is stranded with no message and no recourse, which
                    // is strictly worse than any error. So: tell the client the transfer is off
                    // (`SMSG_TRANSFER_ABORTED`), THEN end the session. Nothing durable is lost —
                    // the escrow is idempotent and the next login re-drives it from the same rows.
                    //
                    // The guard covers the WHOLE world-port, not just its routing step (adversarial
                    // review): re-entry can fail on its own — `player_login` refused by the
                    // stranding guard, a subscription that would not register — with the client on
                    // exactly the same loading screen, and that window is the wider of the two.
                    let mut ported = conn.route_home(store, character_guid);
                    if ported.is_ok() {
                        ported = on_home_shard!(conn, store, |st| enter_world(
                            tx,
                            st,
                            conn,
                            character_guid,
                            session_epoch
                        ));
                    }
                    if let Err(e) = ported {
                        abort_pending_transfer(tx, store, character_guid, &e);
                        return Err(e);
                    }
                }
            }
        }
        // Phase 7: graceful in-game Logout/Exit. Deny if in combat (vanilla behaviour); otherwise
        // ack instantly + complete, remove the entity (observers see DESTROY), drop the peer
        // subscriptions, and return to character-select with the connection still open.
        ClientOpcodeMessage::CMSG_LOGOUT_REQUEST => {
            // In-combat gate: deny logout while combat_until_ms is still in the future. We read the
            // wall-clock here (the gateway is a normal Rust process) and compare against the entity
            // row's ms-epoch timestamp written by `enter_combat`. 0 = never in combat → allowed.
            if let WorldState::InWorld(iw) = &conn.state {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if store.player_combat_until_ms(iw.self_guid) > now_ms {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(
                            codec::logout_denied_in_combat(),
                        )),
                    )?;
                    return Ok(None);
                }
            }
            send(tx, Outbound::Batch(codec::logout_sequence()))?;
            // Leave the world: InWorld → CharSelect drops the relay subs; delete the entity only if
            // we still own it — a newer login on this account supersedes us, and deleting then would
            // vanish them (#42). A `logout` failure here is session-fatal (propagated), as before.
            conn.leave_world(store)?;
        }
        // /played (work-item 029): read the durable total + the live session stamp off the
        // character row and fold them in `build_played_time` so an online player's total keeps
        // ticking without a periodic write. A no-op (no reply) if somehow not in-world or the row
        // vanished — never session-fatal for a display-only command.
        ClientOpcodeMessage::CMSG_PLAYED_TIME => {
            if let WorldState::InWorld(iw) = &conn.state {
                if let Some(c) = store.character_by_guid(iw.self_guid)? {
                    let now_micros = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_PLAYED_TIME(
                            codec::build_played_time(
                                c.played_total_secs,
                                c.session_start_micros,
                                now_micros,
                            ),
                        )),
                    )?;
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Combat family (N3/C1 + aura tracer): selection, melee swing/stop, spell cast. ⚠️ Holds the two
/// session-fatal `is_desync_error` early-exits on CMSG_ATTACKSWING/CMSG_ATTACKSTOP — preserved
/// verbatim (a desync = the player's own entity is gone → tear the session down for a clean relog,
/// unlike the transient per-swing failures that stay logged + alive).
fn handle_combat<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    // #468 stage 4b: the shared-calls path names the actor by guid; 0 (not in world) forces the
    // per-player path in the store.
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };

    match msg {
        // Targeting (N3): record the player's selection server-side (foundation for combat).
        ClientOpcodeMessage::CMSG_SET_SELECTION(s) => {
            store.set_target(conn.account_id, self_guid, s.target.guid())?
        }
        // Pet command bar (CMSG_PET_ACTION): pass the raw packed `data` + target through; the module
        // decodes stay/follow/attack/dismiss + passive/defensive/aggressive and validates ownership. A
        // transient reject (no pet, dead/invalid target) must NOT drop the session — log + ignore, like
        // the start_attack path (do NOT route through is_desync_error).
        ClientOpcodeMessage::CMSG_PET_ACTION(p) => {
            if let Err(e) = store.pet_command(conn.account_id, self_guid, p.data, p.target.guid()) {
                log::debug!(
                    "world: pet_command ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Work-item 223: the client's ack to our `SMSG_FORCE_RUN_SPEED_CHANGE` (`.speed`). We don't
        // gate on the reply (the movement counter/new_speed aren't cross-checked) — explicitly
        // consumed here (rather than falling through to the dispatch tail's `log::debug!` "ignoring"
        // line) so a `.speed` never spams the log or risks a future desync-classifier false-positive.
        ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_) => {}
        // Combat (C1): begin melee auto-attack. Arm the server-side engagement (the swing tick
        // applies damage), and ack with SMSG_ATTACKSTART so the client enters combat stance and
        // plays the swing animation. The per-swing damage text comes from the relayed
        // SMSG_ATTACKERSTATEUPDATE; the health bar from the on_update VALUES relay.
        ClientOpcodeMessage::CMSG_ATTACKSWING(s) => {
            let target_guid = s.guid.guid();
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!("world[097]: CMSG_ATTACKSWING target={target_guid} ranged_repeat_active={was_repeat} (account {})", conn.account_id);
            // A failed start_attack (target already dead/despawned, out of range, not in world) is
            // an EXPECTED transient condition, not session-fatal — e.g. the client swings at the
            // Chicken on the same frame it dies. Log + ignore so the player isn't disconnected; only
            // arm + ack the stance when the engagement actually started.
            match store.start_attack(conn.account_id, self_guid, target_guid) {
                Ok(()) => {
                    if let WorldState::InWorld(iw) = &mut conn.state {
                        iw.attacking_target = Some(target_guid);
                        // Switching to melee overwrites the shared game_melee_attack row to a melee
                        // engagement, so a later CMSG_ATTACKSTOP should now be honored (097).
                        iw.ranged_repeat = false;
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(
                                codec::build_attack_start(iw.self_guid, target_guid),
                            ))),
                        )?;
                    }
                }
                Err(e) => {
                    // A swing at a corpse → reply SMSG_ATTACKSWING_DEADTARGET so the client leaves
                    // combat stance and shows "can't attack — target is dead" (it otherwise hangs in
                    // stance with no swings, since the server refuses to arm the engagement). A friendly
                    // target → SMSG_ATTACKSWING_CANT_ATTACK. These are TRANSIENT per-swing failures.
                    if e.to_string()
                        .contains(lyracore_shared::ERR_ATTACK_TARGET_DEAD)
                    {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET),
                        )?;
                    } else if e.to_string().contains(lyracore_shared::ERR_ATTACK_FRIENDLY) {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK),
                        )?;
                    } else if is_desync_error(&e) {
                        // The player's OWN entity is gone — a desync, NOT a transient swing failure (the
                        // gateway's view went stale, e.g. a schema migration dropped its subscription). No
                        // action can be served, so the player would otherwise hang in combat stance with
                        // no recovery. Propagate as session-fatal → clean socket teardown → the client
                        // shows "Disconnected" and relog re-materializes from durable state.
                        return Err(e.context(
                            "player desync (entity missing) on attackswing — closing session for a clean relog",
                        ));
                    }
                    // Other failures (out of range, retarget races) are transient → log + ignore.
                    log::debug!(
                        "world: start_attack ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Combat (C1): stop auto-attacking; leave combat stance. Best-effort — a stop_attack
        // failure must not drop the session, and the client is always told to leave stance.
        ClientOpcodeMessage::CMSG_ATTACKSTOP => {
            // While a ranged auto-repeat is armed, the client sends CMSG_ATTACKSTOP as part of
            // switching out of melee stance — but melee + ranged share one game_melee_attack row,
            // so honoring it would delete the auto-shot engagement (one-shot-then-stops, 097). The
            // ranged loop is torn down only by CMSG_CANCEL_AUTO_REPEAT_SPELL; ignore the melee stop.
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!(
                "world[097]: CMSG_ATTACKSTOP ranged_repeat_active={was_repeat} (account {})",
                conn.account_id
            );
            if was_repeat {
                return Ok(None);
            }
            if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                // A desync (entity gone) is session-fatal — recover via a clean disconnect, not a
                // silent hang. A transient stop_attack failure stays logged + ignored.
                if is_desync_error(&e) {
                    return Err(e.context(
                        "player desync (entity missing) on attackstop — closing session for a clean relog",
                    ));
                }
                log::debug!(
                    "world: stop_attack ignored (account {}): {e}",
                    conn.account_id
                );
            }
            // `attacking_target` may name a creature the server already killed (the kill sends its
            // own SMSG_ATTACKSTOP and can't reach this thread to clear it); re-sending stop for a
            // now-dead guid is harmless (the client no longer has that unit).
            if let WorldState::InWorld(iw) = &mut conn.state {
                if let Some(target_guid) = iw.attacking_target.take() {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(
                            codec::build_attack_stop(iw.self_guid, target_guid),
                        ))),
                    )?;
                }
            }
        }
        // Aura+spell tracer: cast a spell. On success the module applies the aura + emits the cast
        // event (relayed as SMSG_CAST_RESULT(OK)+SMSG_SPELL_GO + the buff-icon VALUES). On
        // rejection (unknown spell / not in world), reply SMSG_CAST_RESULT::Failure — a SILENT
        // cast-bar reset (NOT a red error; `Success { reason }` is the red-error variant). Self-cast:
        // ignore c.targets. Today CMSG_CAST_SPELL falls into `other =>` and is ignored, so this only
        // enriches safe behavior.
        ClientOpcodeMessage::CMSG_CAST_SPELL(c) => {
            // #10 Ranged auto-attack: Auto Shot + wand Shoot are AUTO-REPEAT ranged attacks (the
            // RANGED_AUTO_REPEAT cast_flags bit — set by the importer from the DBC AttributesEx2 AUTOREPEAT
            // bit, NOT a hardcoded id list; work-item 097), not one-shot casts. Intercept them BEFORE the
            // normal cast path: clear the client cast state (SPELL_START→SPELL_GO, else the action button
            // locks with "Another action is in progress"), then arm the server-side ranged swing loop on
            // the cast's unit target. The loop fires on the ranged-weapon timer until
            // CMSG_CANCEL_AUTO_REPEAT_SPELL / CMSG_ATTACKSTOP.
            if store.spell_is_ranged_auto_repeat(c.spell) {
                // The shot's target rides the cast's SpellCastTargets (UNIT flag). Deliberate
                // simplification: no current-selection fallback — Auto Shot/Shoot are cast ON a
                // target, so the client always includes it.
                let target = c
                    .targets
                    .target_flags
                    .get_unit()
                    .map(|u| u.unit_target.guid())
                    .unwrap_or(0);
                let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
                log::info!(
                    "world[097]: AUTO-REPEAT activate spell={} target={} already_repeating={} (account {})",
                    c.spell, target, was_repeat, conn.account_id
                );
                // [097/vanilla] Arm the server loop FIRST; a rejected activation answers ONLY the raw
                // SMSG_CAST_RESULT(reason) — the 5875 client drops its auto-repeat toggle on a failure
                // result, keeping the client/server toggle in lockstep (vanilla likewise rejects a
                // failed castability check BEFORE sending SPELL_START). The old shape (START first, then a
                // bare typed Failure on rejection) left the client toggled ON over a dead server loop:
                // the NEXT press then sent CMSG_CANCEL_AUTO_REPEAT_SPELL instead of a cast — the
                // "pressing Auto Shot does nothing until I move" bug.
                match store.start_ranged_attack(conn.account_id, self_guid, target, c.spell) {
                    Err(e) => {
                        log::info!("world[097]: start_ranged_attack REJECTED spell={} target={} (account {}): {e}", c.spell, target, conn.account_id);
                        let reason = codec::cast_failure_reason_for(&e.to_string());
                        send(
                            tx,
                            Outbound::Raw {
                                opcode: 0x0130,
                                body: codec::build_cast_result_failed(c.spell, reason),
                            },
                        )?;
                        // A rejected RE-activation (retarget at an invalid new target) drops the
                        // client's toggle on the failure result — tear down the still-firing OLD
                        // loop too, or the server keeps shooting a target the client thinks it
                        // stopped (review find). Fresh activations (was_repeat false) skip the no-op.
                        if was_repeat {
                            if let WorldState::InWorld(iw) = &mut conn.state {
                                iw.ranged_repeat = false;
                            }
                            if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                                log::debug!(
                                    "world: reject-teardown stop_attack ignored (account {}): {e}",
                                    conn.account_id
                                );
                            }
                        }
                    }
                    Ok(()) => {
                        if let WorldState::InWorld(iw) = &mut conn.state {
                            iw.ranged_repeat = true;
                            // [097/vanilla] Activation ack = SMSG_SPELL_START alone: timer 0 (the 0.5s
                            // wind-up is an ATTACK-TIMER, not a cast bar — vmangos GetCastTime skips the
                            // ranged +500ms for auto-repeat; the client animates its own wind-up),
                            // CAST_FLAG_AMMO + ammo block (nocks the arrow — the between-shots aim pose
                            // rides the client's local auto-repeat state), and the real unit target.
                            // No CAST_RESULT(OK) and no GO: the activation cast is parked in the
                            // client's AUTOREPEAT slot and never resolves; each shot's GO comes from
                            // the swing-tick combat-event relay (subscriptions.rs).
                            let ammo = ranged_ammo_display(store, iw.self_guid);
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                                    codec::build_spell_start(
                                        iw.self_guid,
                                        c.spell,
                                        0,
                                        target,
                                        ammo,
                                    ),
                                ))),
                            )?;
                        }
                    }
                }
            } else if let Some(route) = store.enchant_route(c.spell) {
                // Enchant/disenchant spells target an item instance by GUID (routed here by EFFECT KIND,
                // not a spell-id list — a new enchant is a data row). Resolve the GUID → bag slot, then
                // dispatch to the module reducer (disenchant or enchant_item_on_slot, with enchant_id
                // carried in the effect data). These reducers don't emit game_spell_cast_event, so we
                // send START+GO manually to clear the client cast bar. [094]
                use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Item as ItTgt;
                let item_guid = match c.targets.target_flags.get_item() {
                    Some(ItTgt::Item { item }) => item.guid(),
                    _ => 0,
                };
                let result = if item_guid != 0 {
                    match store.item_slot_by_guid(conn.account_id, item_guid) {
                        Some(slot) => match route {
                            EnchantRoute::Disenchant => {
                                store.disenchant_item(conn.account_id, self_guid, slot)
                            }
                            EnchantRoute::Enchant(enchant_id) => {
                                store.enchant_item_on_slot(conn.account_id, self_guid, slot, enchant_id)
                            }
                        },
                        None => Err(anyhow!("enchant: item {item_guid} not in player bag")),
                    }
                } else {
                    Err(anyhow!("enchant: no item target in cast"))
                };
                if let Err(e) = result {
                    log::debug!(
                        "world: enchanting failed (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(Box::new(
                            SMSG_CAST_RESULT {
                                spell: c.spell,
                                result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                            },
                        ))),
                    )?;
                } else if let WorldState::InWorld(iw) = &conn.state {
                    let caster = iw.self_guid;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                            codec::build_spell_start(caster, c.spell, 0, 0, None),
                        ))),
                    )?;
                    send(
                        tx,
                        Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_ok(c.spell),
                        },
                    )?;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                            codec::build_spell_go(caster, c.spell, 0, None),
                        ))),
                    )?;
                }
            } else if store.spell_is_fishing(c.spell) {
                // Fishing (060): instant-resolve — route to the `fish` reducer (lenient alpha gate:
                // auto-learns the skill, grants the catch straight to the bag; the bobber/channel
                // flow is the deferred follow-up). Same manual START→OK→GO clear as the enchant
                // path (the fish reducer emits no game_spell_cast_event). Kind-routed via the
                // synthesized E_FISH effect row — a new fishing tier is a data row.
                match store.fish(conn.account_id, social::self_guid(conn).unwrap_or(0)) {
                    Err(e) => {
                        log::debug!("world: fish failed (account {}): {e}", conn.account_id);
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(Box::new(
                                SMSG_CAST_RESULT {
                                    spell: c.spell,
                                    result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                                },
                            ))),
                        )?;
                    }
                    Ok(()) => {
                        if let WorldState::InWorld(iw) = &conn.state {
                            let caster = iw.self_guid;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                                    codec::build_spell_start(caster, c.spell, 0, 0, None),
                                ))),
                            )?;
                            send(
                                tx,
                                Outbound::Raw {
                                    opcode: 0x0130,
                                    body: codec::build_cast_result_ok(c.spell),
                                },
                            )?;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                                    codec::build_spell_go(caster, c.spell, 0, None),
                                ))),
                            )?;
                        }
                    }
                }
            } else if store.spell_is_open_lock(c.spell) {
                // Pick Lock (119): the cast targets a locked GameObject by GUID (routed here by EFFECT
                // KIND, not a spell-id list — a new open-lock spell is a data row). Decode the GO guid off
                // the cast's SpellCastTargets (GAMEOBJECT flag), call the `pick_lock` reducer, then send
                // START+OK+GO manually to clear the client cast bar (the reducer emits no cast event) —
                // the identical handshake as the enchant/fish arms.
                use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Gameobject as GoTgt;
                let go_guid = match c.targets.target_flags.get_gameobject() {
                    Some(GoTgt::Gameobject { gameobject }) => gameobject.guid(),
                    Some(GoTgt::ObjectUnk { object_unk }) => object_unk.guid(),
                    None => 0,
                };
                let result = if go_guid != 0 {
                    store.pick_lock(conn.account_id, self_guid, go_guid)
                } else {
                    Err(anyhow!("pick_lock: no gameobject target in cast"))
                };
                if let Err(e) = result {
                    log::debug!("world: pick_lock failed (account {}): {e}", conn.account_id);
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(Box::new(
                            SMSG_CAST_RESULT {
                                spell: c.spell,
                                result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                            },
                        ))),
                    )?;
                } else if let WorldState::InWorld(iw) = &conn.state {
                    let caster = iw.self_guid;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                            codec::build_spell_start(caster, c.spell, 0, 0, None),
                        ))),
                    )?;
                    send(
                        tx,
                        Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_ok(c.spell),
                        },
                    )?;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                            codec::build_spell_go(caster, c.spell, 0, None),
                        ))),
                    )?;
                }
            } else {
                // Thread the client's selected unit target (mirrors the ranged path above) so target-keyed
                // effects — combo finishers, enemy spells — see the real target, not the caster. No unit
                // target (self-buffs) → 0 → the module substitutes the caster.
                let target = c
                    .targets
                    .target_flags
                    .get_unit()
                    .map(|u| u.unit_target.guid())
                    .unwrap_or(0);
                // [083] ROOT-CAUSE FIX: for an INSTANT cast, send the caster's START+GO SYNCHRONOUSLY here
                // (as Auto-Shot/Enchant already do), BEFORE dispatching cast_spell. The async relay would
                // deliver START/GO via the game_spell_cast_event subscription callback, which the SpacetimeDB
                // SDK fires AFTER game_aura's (fixed alphabetical callback order, bindings/mod.rs) — so the
                // applied buff reaches the client before START/GO and wedges its cast slot ("Another action in
                // progress" until Esc). Sending them inline guarantees START -> GO -> effects. Timed casts keep
                // the relay path (begin_cast sends START(cast_time); the completion sends GO). The relay
                // suppresses its now-duplicate START/GO to the caster for this instant cast. Unknown spell ->
                // treat as instant (safe: a stray START/GO is harmless; a missing one wedges).
                let instant = store
                    .spell_cast_time(c.spell)
                    .map(|t| t == 0)
                    .unwrap_or(true);
                // 114: an on-next-swing spell (Heroic Strike/Cleave) sends NOTHING here — the client
                // lights the button locally on the press and holds the pending cast; the module's
                // swing-fire emits the CAST_RESULT(OK)+GO (is_completion row) that releases it. The
                // sync START(0)+GO below is exactly what un-lit the button and "resolved" the cast
                // at queue time (the 114 button-lock bug).
                let queues_swing = instant && store.spell_queues_next_swing(c.spell);
                if instant && !queues_swing {
                    if let WorldState::InWorld(iw) = &conn.state {
                        let caster = iw.self_guid;
                        // [083] vmangos sequence: START(0) → CAST_RESULT(OK) → GO.
                        // SMSG_CAST_RESULT(spell_id, 0x00) is the missing server ACK; the 5875 client
                        // requires it before GO to transition m_currentSpells to a clearable state.
                        // Raw 5-byte body (gtker's Success struct would add an erroneous reason byte). [083]
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                                codec::build_spell_start(caster, c.spell, 0, 0, None),
                            ))),
                        )?;
                        send(
                            tx,
                            Outbound::Raw {
                                opcode: 0x0130,
                                body: codec::build_cast_result_ok(c.spell),
                            },
                        )?;
                        // [118] A GROUND-AREA spell (Consecration) hits the ground, not a unit — an
                        // EMPTY hit list (the self-cast fallback put the CASTER in hits[] and the
                        // client played the impact animation ON the paladin).
                        let go = if store.spell_is_ground_area(c.spell) {
                            codec::build_spell_go_area(caster, c.spell)
                        } else {
                            codec::build_spell_go(caster, c.spell, target, None)
                        };
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(go))),
                        )?;
                    }
                }
                // 118 phase 2: a GROUND-TARGETED cast rides a DEST_LOCATION target block (the player
                // clicked the ground — Flamestrike/Blizzard/Rain of Fire). Route it to cast_spell_at with
                // the click coords so the module anchors the AoE/patch there; a normal cast (no dest) keeps
                // the cast_spell path. The START/GO handshake above is unchanged (keyed on `instant`) — a
                // timed ground cast just relays START now + GO at completion like any other timed spell.
                let dest = c
                    .targets
                    .target_flags
                    .get_dest_location()
                    .map(|d| (d.destination.x, d.destination.y, d.destination.z));
                let cast_result = match dest {
                    Some((x, y, z)) => {
                        store.cast_spell_at(conn.account_id, self_guid, c.spell, target, x, y, z)
                    }
                    None => store.cast_spell(conn.account_id, self_guid, c.spell, target),
                };
                if let Err(e) = cast_result {
                    // 040: carry the REASON so the client prints the red error line ("Not enough
                    // rage", "You must be behind your target") — the bare typed Failure only reset
                    // the button silently, leaving server-only gates (behind/stealth/stance/react)
                    // invisible. Raw body: spell + 0x02 + CastFailureReason.
                    let reason = codec::cast_failure_reason_for(&e.to_string());
                    send(
                        tx,
                        Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_failed(c.spell, reason),
                        },
                    )?;
                }
            }
        }
        // #10: stop the ranged auto-repeat loop (the client toggled off / auto-switched to melee).
        // [097/vanilla] `stop_attack` ONLY when a ranged loop is actually armed: the client's
        // melee-press sends CMSG_ATTACKSWING *then* this cancel back-to-back (live-logged), and the
        // swing handler has already overwritten the shared engagement row to MELEE + cleared
        // `ranged_repeat` — an unconditional stop here deleted that just-armed melee row (the
        // "press melee attack twice" bug). Same observable rule the reference cores follow — a no-op when nothing is
        // armed. NO inline ack either — the SMSG_CANCEL_AUTO_REPEAT the client needs on a real
        // teardown is sent by the game_melee_attack on_delete relay (the one choke point), and real
        // cores never ack a client-initiated cancel from the handler (cmangos: echo-loop warning).
        ClientOpcodeMessage::CMSG_CANCEL_AUTO_REPEAT_SPELL => {
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!("world[097]: CMSG_CANCEL_AUTO_REPEAT_SPELL ranged_repeat_active={was_repeat} (account {})", conn.account_id);
            if let WorldState::InWorld(iw) = &mut conn.state {
                iw.ranged_repeat = false;
            }
            if was_repeat {
                if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                    log::debug!(
                        "world: cancel_auto_repeat stop_attack ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Cancel a buff: the player right-clicked its icon (CMSG_CANCEL_AURA). Best-effort — remove the
        // caller's own aura by spell id; the aura on_delete relay then re-syncs the buff bar. A failure
        // (no such aura / not in world) is per-action — log + ignore, never tear the session down.
        // Chat channels (065): the client auto-sends JOIN for General/Trade/LocalDefense on
        // zone-in; ack with SMSG_CHANNEL_NOTIFY(YouJoined) so the tab arms (the client won't
        // accept channel lines for a channel it never got the join notice for). Re-joins are
        // idempotent (the module dedupes; vanilla re-acks). Passwords are ignored (no private
        // channels this slice).
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(c) => {
            if let Err(e) = store.join_channel(conn.account_id, self_guid, c.channel_name.clone()) {
                log::debug!(
                    "world: join_channel failed (account {}): {e}",
                    conn.account_id
                );
            } else {
                use wow_world_messages::vanilla::{ChatNotify, SMSG_CHANNEL_NOTIFY};
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(Box::new(
                        SMSG_CHANNEL_NOTIFY {
                            notify_type: ChatNotify::YouJoinedNotice,
                            channel_name: c.channel_name,
                        },
                    ))),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(c) => {
            if let Err(e) = store.leave_channel(conn.account_id, self_guid, c.channel_name.clone()) {
                log::debug!(
                    "world: leave_channel failed (account {}): {e}",
                    conn.account_id
                );
            } else {
                use wow_world_messages::vanilla::{ChatNotify, SMSG_CHANNEL_NOTIFY};
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(Box::new(
                        SMSG_CHANNEL_NOTIFY {
                            notify_type: ChatNotify::YouLeftNotice,
                            channel_name: c.channel_name,
                        },
                    ))),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_CANCEL_AURA(c) => {
            if let Err(e) = store.cancel_aura(conn.account_id, self_guid, c.id) {
                log::debug!(
                    "world: cancel_aura ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Cancel an in-progress cast (CMSG_CANCEL_CAST — Esc / moved / recast). Clear the caller's pending
        // cast server-side so a scheduled completion can't fire a phantom SMSG_SPELL_GO that wedges the
        // client's cast state ("Another action is in progress"). Best-effort — a failure (nothing pending
        // / not in world) is per-action: log + ignore. The client's spell id (_c) is not needed. [083]
        ClientOpcodeMessage::CMSG_CANCEL_CAST(_c) => {
            if let Err(e) = store.cancel_cast(conn.account_id, social::self_guid(conn).unwrap_or(0)) {
                log::debug!(
                    "world: cancel_cast ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Loot / corpse family (slices 3/4/5): loot a corpse (open window / take money / close window) and
/// the death-recovery flow (release spirit / corpse-location query / reclaim corpse) — grouped as
/// the "corpse-interaction" family.
fn handle_loot<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // Loot (slice 3): right-click a corpse → open the loot window showing its money. The open
        // corpse is tracked on the connection (CMSG_LOOT_MONEY carries no guid). `loot_target_money`
        // reads the corpse's copper from the privileged cache; a non-corpse/empty target yields 0
        // (the client just sees an empty window). The transfer is gated server-side on the take.
        ClientOpcodeMessage::CMSG_LOOT(l) => {
            if let WorldState::InWorld(iw) = &mut conn.state {
                let target_guid = l.guid.guid();
                let money = store.loot_target_money(target_guid)?;
                // Items slice-4: the corpse's item loot fills the loot window alongside the gold.
                // Sent RAW because gtker's typed LootItem omits count/display/random (would desync
                // the 5875 client's parse); `build_loot_response_raw` hand-rolls the full layout.
                // `iw.self_guid` is the VIEWER (work-item 187 slice 0): `corpse_loot` filters
                // `quest_only` rows per viewer, so two grouped looters of the same corpse can see
                // different windows.
                let items = store
                    .corpse_loot(target_guid, iw.self_guid)
                    .unwrap_or_default();
                // Skinning fallback: if the corpse is fully looted (no items, no money left),
                // attempt to skin it. On success the leather appears in the bag via the item
                // subscription relay — the empty loot window is the only feedback (no separate skinning
                // animation in this alpha). On Err (not a beast, already skinned, out of range) the
                // player just sees the empty window, which is the correct safe fallback.
                if items.is_empty() && money == 0 {
                    if let Err(e) = store.skin_corpse(conn.account_id, iw.self_guid, target_guid) {
                        log::debug!("world: skin_corpse noop (account {}): {e}", conn.account_id);
                    }
                }
                iw.looting_target = Some(target_guid);
                let (opcode, body) = codec::build_loot_response_raw(target_guid, money, &items);
                send(tx, Outbound::Raw { opcode, body })?;
            }
        }
        // Take the money from the open corpse. The reducer validates (dead / in-range / has-money)
        // and moves the copper; the new purse rides back as a PLAYER_FIELD_COINAGE VALUES relay and
        // the corpse's cleared lootable flag as a dynamic_flags VALUES relay (the sparkle vanishes).
        // Work-item 221: `SMSG_LOOT_MONEY_NOTIFY` is NO LONGER sent unconditionally here — vanilla
        // sends it ONLY to party members receiving a coin split (the 1.12 client renders it as the
        // "Your share of the loot is X" line); a SOLO looter gets no notify at all and instead relies
        // on the client's own local "You loot X copper" line printed when the coin window clears. A
        // GROUPED split's per-recipient notify rides the `MONEY_SHARE` `game_group_event` relay
        // (`stdb/subscriptions.rs`) instead, exactly like the roll/master-loot notifications. Only
        // `SMSG_LOOT_CLEAR_MONEY` (clears the coin icon from the still-open window) stays here.
        ClientOpcodeMessage::CMSG_LOOT_MONEY => {
            let (self_guid, open) = match &conn.state {
                WorldState::InWorld(iw) => (iw.self_guid, iw.looting_target),
                WorldState::CharSelect => (0, None),
            };
            if let Some(target_guid) = open {
                match store.loot_money(conn.account_id, self_guid, target_guid) {
                    Ok(()) => {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY),
                        )?;
                    }
                    Err(e) => log::debug!(
                        "world: loot_money ignored (account {}): {e}",
                        conn.account_id
                    ),
                }
            }
        }
        // Take an item from the open corpse into the backpack (slice 4). The open corpse is tracked on
        // the connection (the message carries only the loot slot). On success the module moves the item
        // into a free inventory slot — the item then appears in the bag via the inventory live-relay —
        // and deletes the loot row; SMSG_LOOT_REMOVED clears that slot from the open window. A failure
        // (bag full / out of range / already taken) is per-action — log + ignore, never tear the session.
        ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(a) => {
            let (self_guid, open) = match &conn.state {
                WorldState::InWorld(iw) => (iw.self_guid, iw.looting_target),
                WorldState::CharSelect => (0, None),
            };
            if let Some(corpse_guid) = open {
                match store.take_loot(conn.account_id, self_guid, corpse_guid, a.item_slot) {
                    Ok(()) => send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_LOOT_REMOVED(
                            codec::build_loot_removed(a.item_slot),
                        )),
                    )?,
                    Err(e) => log::debug!(
                        "world: take_loot ignored (account {}): {e}",
                        conn.account_id
                    ),
                }
            }
        }
        // Close the loot window: clear the open-corpse state and ack so the client releases the UI.
        ClientOpcodeMessage::CMSG_LOOT_RELEASE(l) => {
            let target_guid = l.guid.guid();
            if let WorldState::InWorld(iw) = &mut conn.state {
                iw.looting_target = None;
            }
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(Box::new(
                    codec::build_loot_release_response(target_guid),
                ))),
            )?;
        }
        // Group loot methods (work-item 187 slices 2-4): a need/greed vote, and the master looter's
        // explicit assign. Both are per-action — a rejection (no roll open, already voted, not the
        // master) is logged + ignored rather than tearing the session; the live vote/winner/master
        // packets ride the `game_group_event` roll relay (`stdb/subscriptions.rs`), not a direct
        // reply here.
        ClientOpcodeMessage::CMSG_LOOT_ROLL(c) => {
            let corpse_guid = c.item.guid();
            let vote = c.vote.as_int();
            // Issue #50: unsharded, `loot::run_vote` is exactly the call above (`store.loot_roll`);
            // sharded, it routes to realm-core instead, so the guid it authorizes as the voter must
            // be the one THIS socket authenticated with, never a literal from the packet.
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = loot::run_vote(
                store,
                conn.account_id,
                self_guid,
                corpse_guid,
                c.item_slot,
                vote,
            ) {
                log::debug!(
                    "world: loot_roll ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(c) => {
            let corpse_guid = c.loot.guid();
            let target_guid = c.player.guid();
            if let Err(e) =
                store.loot_master_give(conn.account_id, social::self_guid(conn).unwrap_or(0), corpse_guid, c.slot_id, target_guid)
            {
                log::debug!(
                    "world: loot_master_give ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Use a gameobject (CMSG_GAMEOBJ_USE): the module rolls a CHEST's loot into game_corpse_loot
        // (keyed on the GO guid) or grants a quest-object's credit. For a chest we then open the loot
        // window the same way CMSG_LOOT does. Known limitation: the post-reducer corpse_loot read
        // can lag the loot-row subscription on the very first use (a re-click's CMSG_LOOT
        // re-serves the same rows); the roll itself is committed server-side. A goober yields no
        // rows → no window.
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(g) => {
            let go_guid = g.guid.guid();
            // The VIEWER for `quest_only` filtering (work-item 187 slice 0) — a chest is only usable
            // in-world; 0 (no real character ever has this guid) is a safe "no viewer" fallback for
            // the CharSelect edge case, hiding every quest_only row rather than guessing.
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            // A QUESTGIVER-type GO (the Wanted Poster GO 68, the Lost Guards corpses GO 55/56 —
            // work-item 041) never rolls loot or grants credit: in vanilla a
            // GAMEOBJECT_TYPE_QUESTGIVER goes straight to the quest window (the same menu/single-quest
            // logic `CMSG_QUESTGIVER_HELLO` uses for a creature giver), so the client's right-click
            // opens the quest dialog on it exactly like an NPC — it never sends a separate
            // CMSG_QUESTGIVER_HELLO for a gameobject giver. Checked BEFORE the ordinary use-reducer
            // path, and short-circuits it entirely (the module's `apply_use_gameobject` treats
            // QUESTGIVER as an inert no-op anyway, so skipping it changes nothing state-side).
            if store.gameobject_type(go_guid)?
                == Some(lyracore_shared::constants::go_type::QUESTGIVER)
            {
                send_questgiver_menu(tx, store, go_guid, self_guid)?;
            } else {
                match store.use_gameobject(conn.account_id, self_guid, go_guid) {
                    Ok(()) => {
                        let items = store.corpse_loot(go_guid, self_guid).unwrap_or_default();
                        if !items.is_empty() {
                            if let WorldState::InWorld(iw) = &mut conn.state {
                                iw.looting_target = Some(go_guid);
                            }
                            let (opcode, body) = codec::build_loot_response_raw(go_guid, 0, &items);
                            send(tx, Outbound::Raw { opcode, body })?;
                        }
                    }
                    Err(e) => log::debug!(
                        "world: use_gameobject ignored (account {}): {e}",
                        conn.account_id
                    ),
                }
            }
        }
        // Enter an area trigger (CMSG_AREATRIGGER): the client fires this when the player physically
        // walks into a trigger zone (e.g. a mine for an "explore" quest). The module credits any active
        // explore quest tied to the trigger id. A transient/no-match result is logged + ignored.
        ClientOpcodeMessage::CMSG_AREATRIGGER(a) => {
            if let Err(e) = store.enter_areatrigger(conn.account_id, social::self_guid(conn).unwrap_or(0), a.trigger_id) {
                log::debug!(
                    "world: enter_areatrigger ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Gameobject template query (CMSG_GAMEOBJECT_QUERY): the client asks for a GO's name/type/display
        // before it renders/interacts. Reply with the template, or the not-found form.
        ClientOpcodeMessage::CMSG_GAMEOBJECT_QUERY(q) => {
            let tmpl = store.gameobject_template(q.entry_id)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(
                    Box::new(codec::build_gameobject_query_response(
                        q.entry_id,
                        tmpl.as_ref(),
                    )),
                )),
            )?;
        }
        // Release Spirit after death (slice 4). The client sends this (empty body) when the player
        // clicks Release on the death screen. Revive in place at full health; the restored health
        // replicates via the on_update VALUES relay and the client leaves the death screen.
        // SMSG_CORPSE_RECLAIM_DELAY is now relay-driven (work-item 201: the escalated per-corpse
        // delay, not a flat 30s) — see `on_corpse_insert` in `stdb/subscriptions.rs`, which fires off
        // the SAME `game_corpse` insert `repop`'s reducer call just caused, so no explicit send here.
        ClientOpcodeMessage::CMSG_REPOP_REQUEST => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.repop(conn.account_id, self_guid) {
                log::debug!("world: repop ignored (account {}): {e}", conn.account_id);
            }
        }
        // Corpse location query (slice 5): the client asks where the player's corpse is to draw the
        // map marker + offer "Reclaim Corpse" near it. Reply with the corpse's position, or NotFound.
        ClientOpcodeMessage::MSG_CORPSE_QUERY => {
            if let WorldState::InWorld(iw) = &conn.state {
                let loc = store.corpse_location(iw.self_guid)?;
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::MSG_CORPSE_QUERY(Box::new(
                        codec::build_corpse_query_response(loc)?,
                    ))),
                )?;
            }
        }
        // Reclaim your corpse (slice 5): the ghost, near its corpse and past the 30s delay, resurrects
        // at 50%. The module validates ownership/ghost/range/delay; a failure (too far, too soon, not
        // a ghost) is expected and silently ignored — the client just stays a ghost.
        ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(r) => {
            if let Err(e) = store.reclaim_corpse(conn.account_id, social::self_guid(conn).unwrap_or(0), r.guid.guid()) {
                log::debug!(
                    "world: reclaim_corpse ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Resurrection accept-prompt response (#014): the dead player answered the SMSG_RESURRECT_REQUEST
        // offer. `status` is vanilla's accept(1)/decline(0) byte; the offer's guid is ignored (mirrors
        // `reclaim_corpse`'s own-corpse derivation — the module resolves the pending offer from the
        // CALLER via `ctx.sender()`, never the wire guid). A failure (no pending offer — already
        // answered/lapsed) is expected and silently ignored.
        ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(r) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.resurrect_response(conn.account_id, self_guid, r.status != 0) {
                log::debug!(
                    "world: resurrect_response ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Spirit-Healer resurrection: a ghost activated the graveyard Spirit Healer (npc_flags
        // SPIRITHEALER). The module res's in place at 50% + applies Resurrection Sickness; on success
        // reply with SMSG_SPIRIT_HEALER_CONFIRM (echoing the healer's guid) so the client closes the
        // dialog. The res itself replicates via the entity VALUES relay (health > 0 + cleared ghost
        // bits), exactly like reclaim_corpse. A failure (not a ghost) is per-action — log + ignore.
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(s) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            match store.spirit_healer_res(conn.account_id, self_guid, s.guid.guid()) {
                Ok(()) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(
                        SMSG_SPIRIT_HEALER_CONFIRM { guid: s.guid },
                    )),
                )?,
                Err(e) => {
                    log::debug!(
                        "world: spirit_healer_res ignored (account {}): {e}",
                        conn.account_id
                    )
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Vendor family (Tier 2): open a vendor's inventory window + buy/sell. `CMSG_LIST_INVENTORY` reads
/// the vendor's stock and replies RAW (gtker's typed `SMSG_LIST_INVENTORY` is the tbc/wrath shape).
/// Buy/sell forward to the module reducers; a gameplay `Err` (no stock / no copper / out of range)
/// is per-action — log + ignore like the combat/loot arms, never tear the session down.
fn handle_vendor<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // Open the vendor window: read the NPC's stock (joined with the item templates) and send the
        // RAW SMSG_LIST_INVENTORY. An empty stock still replies (a vendor with no items shows an
        // empty window) so the client doesn't hang waiting on the open it requested.
        ClientOpcodeMessage::CMSG_LIST_INVENTORY(c) => {
            let vendor_guid = c.guid.guid();
            // 195 slice A: an Unfriendly-or-below (or mask-hostile) vendor refuses the window —
            // silent drop, like the inspect/gameobject gates (vanilla NPCs just don't respond).
            if let WorldState::InWorld(iw) = &conn.state {
                if store
                    .npc_refuses_interaction(vendor_guid, iw.self_guid)
                    .unwrap_or(false)
                {
                    return Ok(None);
                }
            }
            let items = store.vendor_items(vendor_guid)?;
            let (opcode, body) = codec::build_list_inventory_raw(vendor_guid, &items);
            send(tx, Outbound::Raw { opcode, body })?;
        }
        // Buy `amount` of an item ENTRY from the vendor. The module gates it (vendor stock / range /
        // copper) and replicates the new item + purse via the player's subscription; a rejection is a
        // transient per-action failure — logged and relayed to the buyer as SMSG_BUY_FAILED (red
        // on-screen error) so they know *why* the purchase was refused, never session-fatal.
        ClientOpcodeMessage::CMSG_BUY_ITEM(c) => {
            let vendor_guid = c.vendor.guid();
            let item_entry = c.item;
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            if let Err(e) = store.buy_item(
                conn.account_id,
                self_guid,
                vendor_guid,
                item_entry,
                c.amount as u32,
            ) {
                log::debug!("world: buy_item failed (account {}): {e}", conn.account_id);
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_BUY_FAILED(Box::new(
                        codec::build_buy_failed(vendor_guid, item_entry, &e.to_string()),
                    ))),
                )?;
            }
        }
        // Sell an item back to a vendor. CMSG_SELL_ITEM carries the item's INSTANCE guid, but the
        // module's `sell_item` takes the inventory SLOT — so resolve guid → slot from the player's
        // own items, then call. An unmatched guid (already sold / not ours) is logged + ignored.
        ClientOpcodeMessage::CMSG_SELL_ITEM(c) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => Some(iw.self_guid),
                WorldState::CharSelect => None,
            };
            if let Some(self_guid) = self_guid {
                let item_guid = c.item.guid();
                match store
                    .player_items(self_guid)?
                    .into_iter()
                    .find(|i| i.guid == item_guid)
                {
                    Some(inst) => {
                        match store.sell_item(
                            conn.account_id,
                            self_guid,
                            c.vendor.guid(),
                            inst.slot,
                        ) {
                            // 248: reflect the new ring in the buyback tab immediately.
                            Ok(()) => push_buyback_view(tx, store, self_guid, false)?,
                            Err(e) => log::debug!(
                                "world: sell_item ignored (account {}): {e}",
                                conn.account_id
                            ),
                        }
                    }
                    None => log::debug!(
                        "world: sell_item for unknown item guid {item_guid} (account {})",
                        conn.account_id
                    ),
                }
            }
        }
        // Re-buy the last-sold item from a vendor's buyback tab. CMSG_BUYBACK_ITEM carries the vendor
        // guid + a BuybackSlot enum (69–81). Map to 0-based slot index and call the module reducer.
        ClientOpcodeMessage::CMSG_BUYBACK_ITEM(c) => {
            let slot = c.slot.as_int().saturating_sub(69) as u8;
            match store.buyback_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.guid.guid(), slot) {
                // 248: the re-bought item's bag CREATE rides the item relay; refresh the tab view.
                Ok(()) => {
                    if let WorldState::InWorld(iw) = &conn.state {
                        push_buyback_view(tx, store, iw.self_guid, false)?;
                    }
                }
                Err(e) => log::debug!(
                    "world: buyback_item ignored (account {}): {e}",
                    conn.account_id
                ),
            }
        }
        // Repair an item at an armorer. CMSG_REPAIR_ITEM carries the NPC guid + the item INSTANCE guid;
        // the module's repair_item takes the inventory SLOT, so resolve guid → slot from the player's
        // own items (like CMSG_SELL_ITEM). An unmatched guid is logged + ignored; a gameplay Err
        // (out of range / too poor) is per-action, never session-fatal. The client's per-item clicks
        // carry the item guid; the REPAIR-ALL button sends guid 0 (252 live find — the earlier
        // "no repair-all bit" claim here was wrong) → the module's whole-body slot u8::MAX.
        ClientOpcodeMessage::CMSG_REPAIR_ITEM(c) => {
            let self_guid = match &conn.state {
                WorldState::InWorld(iw) => Some(iw.self_guid),
                WorldState::CharSelect => None,
            };
            if let Some(self_guid) = self_guid {
                let item_guid = c.item.guid();
                // Repair ALL (252, live find): the 1.12 client's "repair all" button sends
                // item guid 0 (the earlier per-item-only comment was wrong) — route it to the
                // module's existing whole-body slot (u8::MAX, already implemented + charged).
                if item_guid == 0 {
                    if let Err(e) = store.repair_item(conn.account_id, self_guid, c.npc.guid(), u8::MAX) {
                        log::debug!(
                            "world: repair_all ignored (account {}): {e}",
                            conn.account_id
                        );
                    }
                } else {
                    match store
                        .player_items(self_guid)?
                        .into_iter()
                        .find(|i| i.guid == item_guid)
                    {
                        Some(inst) => {
                            if let Err(e) =
                                store.repair_item(conn.account_id, self_guid, c.npc.guid(), inst.slot)
                            {
                                log::debug!(
                                    "world: repair_item ignored (account {}): {e}",
                                    conn.account_id
                                );
                            }
                        }
                        None => log::debug!(
                            "world: repair_item for unknown item guid {item_guid} (account {})",
                            conn.account_id
                        ),
                    }
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Item-action family (inventory slice): equip / unequip / move items within the player's MAIN
/// inventory container (`INVENTORY_SLOT_BAG_0` = 255 — equipment slots 0..=18 and the backpack 23..=38).
/// The module owns all slot validation (equip rules, required-level, swap, full-backpack); the gateway
/// only maps the client's (bag, slot) addressing onto the module's absolute-slot reducers and relays
/// the resulting row changes back via the player's item subscription. Items inside equipped SUB-bags
/// (bag 19..=22) aren't modelled yet → logged + ignored. Every reducer rejection is a transient
/// per-action failure (logged, never session-fatal), exactly like the vendor buy/sell arms.
fn handle_item<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    const MAIN_BAG: u8 = 255; // INVENTORY_SLOT_BAG_0 — backpack + equipped slots share this pseudo-bag
    const EQUIP_SLOT_END: u8 = 18; // EQUIPMENT_SLOT_END — last equipment slot (main-hand=15, off=16…)
    match msg {
        // Right-click a backpack item → auto-equip into its matching slot (module resolves the slot).
        ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(c) => {
            if c.source_bag == MAIN_BAG {
                let self_guid = match &conn.state {
                    WorldState::InWorld(iw) => iw.self_guid,
                    WorldState::CharSelect => 0,
                };
                if let Err(e) = store.equip_item(conn.account_id, self_guid, c.source_slot) {
                    log::debug!(
                        "world: equip_item rejected (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                            Box::new(codec::build_inventory_change_failure()),
                        )),
                    )?;
                }
            } else {
                log::debug!(
                    "world: autoequip from sub-bag {} unsupported (account {})",
                    c.source_bag,
                    conn.account_id
                );
            }
        }
        // Drag an item between two main-inventory slots — covers drag-to-equip, drag-to-unequip, and
        // backpack rearrange (the module's move primitive validates equip-slot transitions).
        ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(c) => {
            if let Err(e) = store.move_item(
                conn.account_id,
                social::self_guid(conn).unwrap_or(0),
                c.source_slot.as_int(),
                c.destination_slot.as_int(),
            ) {
                log::debug!(
                    "world: move_item rejected (account {}): {e}",
                    conn.account_id
                );
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                        Box::new(codec::build_inventory_change_failure()),
                    )),
                )?;
            }
        }
        // Right-click an equipped item → auto-store it into the first free backpack slot (unequip).
        ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(c) => {
            if c.source_bag == MAIN_BAG && c.source_slot <= EQUIP_SLOT_END {
                if let Err(e) = store.unequip_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.source_slot) {
                    log::debug!(
                        "world: unequip_item rejected (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                            Box::new(codec::build_inventory_change_failure()),
                        )),
                    )?;
                }
            } else {
                log::debug!(
                    "world: autostore (bag {} slot {}) unsupported (account {})",
                    c.source_bag,
                    c.source_slot,
                    conn.account_id
                );
            }
        }
        // Right-click a consumable (food/drink/potion/bandage) → use it. Only the main bag (255) is
        // modeled, so `bag_slot` is our flat inventory slot. The module applies the on-use effect + stack
        // decrement; a per-action Err (not usable / empty) is logged, never session-fatal.
        ClientOpcodeMessage::CMSG_USE_ITEM(c) => {
            if c.bag_index == MAIN_BAG {
                // Item-starts-quest (work-item 194): an item whose template carries `start_quest`
                // opens that quest's DETAILS screen instead of the normal use/consume path — the item
                // is NOT consumed (cmangos's "dropped tabard starts a quest" pattern). Checked BEFORE
                // `use_item` so a start-quest item never falls through to the generic (usually
                // rejected, "not consumable") use path. Needs the in-world player guid to resolve the
                // slot's owner; a CharSelect use (shouldn't happen — items are in-world only) falls
                // through to the ordinary path below, which errors harmlessly.
                let self_guid = match &conn.state {
                    WorldState::InWorld(iw) => Some(iw.self_guid),
                    WorldState::CharSelect => None,
                };
                let start_quest = self_guid.and_then(|g| store.item_start_quest(g, c.bag_slot));
                if let Some((item_guid, quest_id)) = start_quest {
                    if let Some(detail) = store.quest_detail(quest_id)? {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(
                                Box::new(codec::build_quest_details(item_guid, &detail)),
                            )),
                        )?;
                    }
                } else if let Err(e) =
                    store.use_item(conn.account_id, self_guid.unwrap_or(0), c.bag_slot)
                {
                    log::debug!(
                        "world: use_item rejected (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                            Box::new(codec::build_inventory_change_failure()),
                        )),
                    )?;
                }
            } else {
                log::debug!(
                    "world: use_item from sub-bag {} unsupported (account {})",
                    c.bag_index,
                    conn.account_id
                );
            }
        }
        // Cross-container swap. We only model the main inventory, so both ends must be bag 255.
        // (NB: gtker's field is spelled `destionation_slot` — a typo in the generated struct.)
        ClientOpcodeMessage::CMSG_SWAP_ITEM(c) => {
            if c.source_bag == MAIN_BAG && c.destination_bag == MAIN_BAG {
                if let Err(e) = store.move_item(conn.account_id, social::self_guid(conn).unwrap_or(0), c.source_slot, c.destionation_slot)
                {
                    log::debug!(
                        "world: move_item (swap) rejected (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                            Box::new(codec::build_inventory_change_failure()),
                        )),
                    )?;
                }
            } else {
                log::debug!(
                    "world: cross-bag swap unsupported (account {})",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Class-trainer family: open the trainer window (`CMSG_TRAINER_LIST` → `SMSG_TRAINER_LIST`, each spell
/// Green/Red/Gray) and learn a spell (`CMSG_TRAINER_BUY_SPELL` → the module buy →
/// `SMSG_TRAINER_BUY_*` + a live `SMSG_LEARNED_SPELL` so it hits the action bar without a relog).
/// Needs the in-world player guid (a trainer is only clicked in-world); in CharSelect the opcodes
/// pass through. A buy rejection is per-action — surfaced as `SMSG_TRAINER_BUY_FAILED` (reason
/// parsed from the module's `[N]` tag).
fn handle_trainer<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        WorldState::CharSelect => return Ok(Some(msg)),
    };
    match msg {
        ClientOpcodeMessage::CMSG_TRAINER_LIST(c) => {
            let trainer_guid = c.guid.guid();
            // 195 slice A: a trainer that dislikes you refuses the window (silent drop).
            if store
                .npc_refuses_interaction(trainer_guid, self_guid)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            let spells = store.trainer_list(self_guid, trainer_guid)?;
            // Deliberate simplification: a generic greeting — the per-NPC trainer greeting text is
            // a later npc_text slice (same as the vendor's generic gossip line).
            let list =
                codec::build_trainer_list(trainer_guid, &spells, "I can teach you a thing or two.");
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_LIST(Box::new(list))),
            )?;
        }
        ClientOpcodeMessage::CMSG_TRAINER_BUY_SPELL(c) => {
            let trainer_guid = c.guid.guid();
            let spell_id = c.id;
            match store.buy_trainer_spell(conn.account_id, self_guid, trainer_guid, spell_id) {
                Ok(()) => {
                    // Confirm + push the spell live so it appears on the action bar without a relog.
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(Box::new(
                            codec::build_trainer_buy_succeeded(trainer_guid, spell_id),
                        ))),
                    )?;
                    // Book the RESOLVED rank (465), not the wrapper (1875) — the module granted
                    // the trigger spell; echoing the wrapper put "the spell that teaches Devotion
                    // Aura" in the player's General tab until relog.
                    // 258: a RANK UPGRADE (the chain prev is already known) sends SUPERCEDED
                    // instead — the client REPLACES the old rank's book entry (vanilla) rather
                    // than stacking "Rank 1" next to "Rank 2". WIRE ORDER: cmangos writes
                    // old u16 THEN new u16; gtker's field names claim new-first — per the
                    // field-names-lie precedent we follow cmangos, so `new_spell_id` (the FIRST
                    // wire slot) carries the OLD rank. If live verify shows the NEW rank
                    // vanishing instead, swap these two.
                    let resolved = store.resolve_learn_target(spell_id);
                    match store.superseded_old_rank(resolved, self_guid) {
                        Some(old_rank) => {
                            use wow_world_messages::vanilla::SMSG_SUPERCEDED_SPELL;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(
                                    SMSG_SUPERCEDED_SPELL {
                                        new_spell_id: old_rank as u16,
                                        old_spell_id: resolved as u16,
                                    },
                                )),
                            )?;
                        }
                        None => {
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                    codec::build_learned_spell(resolved),
                                )),
                            )?;
                        }
                    }
                }
                Err(e) => {
                    // The module tags its Err with a [N] gtker failure-reason; 1=money, 2=level/req, else generic.
                    let es = e.to_string();
                    let reason = if es.contains("[1]") {
                        1
                    } else if es.contains("[2]") {
                        2
                    } else {
                        0
                    };
                    log::debug!(
                        "world: buy_trainer_spell failed (account {}): {es}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(Box::new(
                            codec::build_trainer_buy_failed(trainer_guid, spell_id, reason),
                        ))),
                    )?;
                }
            }
        }
        // Spend a talent point (`CMSG_LEARN_TALENT`). The module gates points/prereqs; on success the
        // passive aura relay covers stat/buff updates. If this talent also grants a learnable ability
        // (`grant_spell_id != 0`), push `SMSG_LEARNED_SPELL` so the action bar is usable without a relog.
        // Action-bar persistence: the client sends ONE of these per drag/clear and expects the
        // full bar back at login (SMSG_ACTION_BUTTONS). Unhandled until now — every bar change
        // was lost on relog (only the creation-seeded buttons survived; user find via a
        // talent-learned Consecration vanishing from the bar). `action`+`misc` are the client's
        // packed u24 payload (spell id, or item id spilling into misc); best-effort (a failure
        // must never drop the session — the button just won't stick).
        ClientOpcodeMessage::CMSG_SET_ACTION_BUTTON(c) => {
            let action = c.action as u32 | ((c.misc as u32) << 16);
            if let Err(e) =
                store.set_action_button(conn.account_id, self_guid, c.button, action, c.action_type)
            {
                log::debug!(
                    "world: set_action_button ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // The rep pane's At-War checkbox (195 slice B). The wire's `faction` u16 is the client's
        // 0..63 rep-array slot (ReputationListID — gtker's field name lies, the same
        // SET_FACTION_STANDING precedent); `flags` carries the new checkbox state (AT_WAR = 0x02).
        // Best-effort like SET_ACTION_BUTTON — a failure must never drop the session.
        ClientOpcodeMessage::CMSG_SET_FACTION_ATWAR(c) => {
            let reputation_index = c.faction.as_int() as u32;
            let at_war = c.flags.is_at_war();
            if let Err(e) = store.set_faction_at_war(conn.account_id, self_guid, reputation_index, at_war) {
                log::debug!(
                    "world: set_faction_at_war ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        ClientOpcodeMessage::CMSG_LEARN_TALENT(c) => {
            let talent_id = c.talent.as_int();
            let grant_spell_id = store.talent_grant_spell(talent_id);
            match store.learn_talent(conn.account_id, self_guid, talent_id) {
                Ok(()) => {
                    if grant_spell_id != 0 {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                codec::build_learned_spell(grant_spell_id),
                            )),
                        )?;
                    }
                    // Live talent-pane refresh (user bug: "talents work server-side but the UI
                    // doesn't update"). The 1.12 TalentFrame redraws on exactly two events, and a
                    // PASSIVE pick used to send neither: (a) SPELLS_CHANGED — the pane derives a
                    // talent's shown rank from which RANK-SPELL is in the spellbook, so relay the
                    // rank-spell the module just taught (SUPERCEDED replaces the previous rank's
                    // book entry, same cmangos old-then-new wire order as the trainer path);
                    // (b) CHARACTER_POINTS_CHANGED — push the decremented unspent counter.
                    if let WorldState::InWorld(iw) = &conn.state {
                        let self_guid = iw.self_guid;
                        let (teach, superseded, remaining) =
                            store.talent_pane_sync(self_guid, talent_id);
                        if teach != 0 && teach != grant_spell_id {
                            if superseded != 0 {
                                use wow_world_messages::vanilla::SMSG_SUPERCEDED_SPELL;
                                send(
                                    tx,
                                    Outbound::One(ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(
                                        SMSG_SUPERCEDED_SPELL {
                                            new_spell_id: superseded as u16, // cmangos wire order: OLD rides the first slot
                                            old_spell_id: teach as u16,
                                        },
                                    )),
                                )?;
                            } else {
                                send(
                                    tx,
                                    Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                        codec::build_learned_spell(teach),
                                    )),
                                )?;
                            }
                        }
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                codec::build_talent_points_values(self_guid, remaining),
                            ))),
                        )?;
                        // Spell-modifier mirror (264): the pick may have applied an A_SPELLMOD
                        // passive — re-send the aggregated totals so the client's cast bars match
                        // the server's folded timings immediately (idempotent absolute values).
                        for m in codec::build_spell_modifier_msgs(&store.spell_modifiers(self_guid))
                        {
                            send(tx, Outbound::One(m))?;
                        }
                    }
                }
                Err(e) => {
                    log::debug!(
                        "world: learn_talent ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// The quest menu for `giver` (creature OR gameobject guid — `quest_giver_evals` resolves either,
/// work-item 041) against `self_guid`: vanilla "instant quest" (mangos `SendPreparedQuest`) opens a
/// SINGLE menu-worthy quest's screen DIRECTLY (accept details for a new quest, the reward screen for a
/// finished turn-in, the "not done yet" request-items screen for one in progress); a giver with
/// MULTIPLE quests shows the list instead (work-item 112). Shared by `CMSG_QUESTGIVER_HELLO` (a
/// creature giver) and `CMSG_GAMEOBJ_USE` on a `go_type::QUESTGIVER` gameobject (a GO giver) — the two
/// client interactions converge on the exact same window, so this is the single chokepoint that keeps
/// them from drifting apart (mirrors `filtered_gossip_options`'s HELLO/SELECT_OPTION rationale).
fn send_questgiver_menu<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    giver: u64,
    self_guid: u64,
) -> Result<()> {
    let evals = store.quest_giver_evals(giver, self_guid)?;
    let menu = codec::quest_menu_items(&evals);
    let single = if menu.len() == 1 {
        store.quest_detail(menu[0].quest_id)?
    } else {
        None
    };
    if let Some(detail) = single {
        let turn_in = evals
            .iter()
            .find(|e| e.quest_id == detail.quest_id && e.role == codec::ROLE_END && e.active);
        let out = match turn_in {
            Some(e) if e.complete => ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
                codec::build_offer_reward(giver, &detail),
            )),
            Some(_) => ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                codec::build_request_items(giver, &detail, false),
            )),
            None => ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(Box::new(
                codec::build_quest_details(giver, &detail),
            )),
        };
        send(tx, Outbound::One(out))?;
    } else {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_LIST(Box::new(
                codec::build_quest_list(giver, "Greetings.", &evals),
            ))),
        )?;
    }
    Ok(())
}

/// Quest-giver dialog family (quests gateway slice): the overhead `!`/`?` status, the right-click
/// quest menu, the quest details + accept, and the turn-in offer/complete round-trip. Every reply is a
/// gtker-typed `SMSG_QUESTGIVER_*` message (no raw encoder). Reads are evaluated against the player, so
/// these need the in-world player guid — in CharSelect the opcodes pass through (a questgiver can only
/// be clicked in-world). Reducer rejections (accept/turn-in gates) are per-action: logged, not fatal.
fn handle_quest<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        WorldState::CharSelect => return Ok(Some(msg)),
    };
    match msg {
        // The client polls each nearby questgiver for its overhead icon (`!` available / `?` turn-in).
        ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(q) => {
            let giver = q.guid.guid();
            let status = codec::quest_giver_status(&store.quest_giver_evals(giver, self_guid)?);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(Box::new(
                    codec::build_questgiver_status(giver, status),
                ))),
            )?;
        }
        // Right-click a questgiver → the quest menu (every quest it offers/completes for this player).
        // Shared with `handle_loot`'s `CMSG_GAMEOBJ_USE` (work-item 041): a QUESTGIVER-type gameobject
        // opens this exact same menu via `send_questgiver_menu` — the client never sends
        // CMSG_QUESTGIVER_HELLO for a GO giver, only for a creature one.
        ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(h) => {
            // 195 slice A: an Unfriendly-or-below questgiver refuses the menu (silent drop).
            if store
                .npc_refuses_interaction(h.guid.guid(), self_guid)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            send_questgiver_menu(tx, store, h.guid.guid(), self_guid)?;
        }
        // Clicked a quest in the menu → its details + Accept button.
        ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(q) => {
            let giver = q.guid.guid();
            if let Some(detail) = store.quest_detail(q.quest_id)? {
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(
                        Box::new(codec::build_quest_details(giver, &detail)),
                    )),
                )?;
            }
        }
        // The client asks for a quest's full definition (it sends this for any quest id it sees in a
        // PLAYER_QUEST_LOG slot but has no data for). Without this reply the client won't display/count
        // the quest in its log — so this is what makes the quest-log window entry actually appear.
        ClientOpcodeMessage::CMSG_QUEST_QUERY(q) => {
            if let Some(detail) = store.quest_detail(q.quest_id)? {
                // RAW-encoded (gtker's typed layout writes the rep Faction fields as u16 → 4-byte title
                // shift). The hand-rolled body matches the 5875 layout exactly.
                let (opcode, body) = codec::build_quest_query_response_raw(&detail);
                send(tx, Outbound::Raw { opcode, body })?;
            }
        }
        // Abandon a quest from the log ("Abandon Quest"). The payload is a LOG slot (0..19), not a quest
        // id — resolve it via the same slot ordering player_quest_log uses, then call the module reducer
        // (deletes the row). The quest-log relay then re-sends the cleared block, so the slot disappears.
        ClientOpcodeMessage::CMSG_QUESTLOG_REMOVE_QUEST(r) => {
            if let Some(s) = store
                .player_quest_log(self_guid)?
                .into_iter()
                .find(|s| s.slot == r.slot)
            {
                if let Err(e) = store.abandon_quest(conn.account_id, self_guid, s.quest_id) {
                    log::debug!(
                        "world: abandon_quest ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Clicked Accept → the module opens the quest log row (gated). No SMSG on success (the client
        // closes the window itself; the quest-log window is the deferred Phase-2 descriptor slice).
        ClientOpcodeMessage::CMSG_QUESTGIVER_ACCEPT_QUEST(a) => {
            if let Err(e) =
                store.accept_quest(conn.account_id, self_guid, a.guid.guid(), a.quest_id)
            {
                log::debug!(
                    "world: accept_quest ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Opened a turn-in (clicked the `?`): the offer-reward screen if every objective is met, else
        // the request-items "not finished" screen (the module is the authority; this only picks the UI).
        ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(c) => {
            let giver = c.guid.guid();
            if let Some(detail) = store.quest_detail(c.quest_id)? {
                let complete = store
                    .quest_giver_evals(giver, self_guid)?
                    .iter()
                    .any(|e| e.quest_id == c.quest_id && e.role == codec::ROLE_END && e.complete);
                let out = if complete {
                    ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(
                        codec::build_offer_reward(giver, &detail),
                    ))
                } else {
                    ServerOpcodeMessage::SMSG_QUESTGIVER_REQUEST_ITEMS(Box::new(
                        codec::build_request_items(giver, &detail, false),
                    ))
                };
                send(tx, Outbound::One(out))?;
            }
        }
        // Chose the reward → the module grants money/XP/items (gated on completion). On success, the
        // "Quest Complete" popup echoes what was granted (XP via the shared formula, so it matches).
        ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(c) => {
            match store.turn_in_quest(
                conn.account_id,
                self_guid,
                c.guid.guid(),
                c.quest_id,
                c.reward,
            ) {
                Ok(()) => {
                    if let Some(detail) = store.quest_detail(c.quest_id)? {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(
                                Box::new(codec::build_quest_complete(&detail)),
                            )),
                        )?;
                    }
                }
                Err(e) => log::debug!(
                    "world: turn_in_quest ignored (account {}): {e}",
                    conn.account_id
                ),
            }
        }
        // Share a quest with the party (`CMSG_PUSHQUESTTOPARTY`, work-item 194). The module validates
        // grouped + actively-on-the-quest and pushes the per-member `QUEST_SHARE`/`QUEST_PUSH_RESULT`
        // events itself (relayed by `subscriptions.rs`'s `on_group_event`); no direct SMSG here.
        ClientOpcodeMessage::CMSG_PUSHQUESTTOPARTY(p) => {
            if let Err(e) = store.push_quest(conn.account_id, self_guid, p.quest_id) {
                log::debug!(
                    "world: push_quest ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// The NPC's imported gossip options (work-item 217), condition-filtered against `player_guid`'s quest
/// state — the SINGLE chokepoint both `CMSG_GOSSIP_HELLO` (render) and `CMSG_GOSSIP_SELECT_OPTION`
/// (re-derive the click) call, so the two can never disagree about which options are visible (the
/// "HELLO/SELECT_OPTION alignment" trap: a click's `gossip_list_id` indexes into whatever list HELLO
/// actually sent, so SELECT must reproduce that exact list, not just re-read the raw unfiltered rows).
/// Preserves `option_index` order (already sorted by the store read).
fn filtered_gossip_options<St: WorldStore + ?Sized>(
    store: &St,
    npc_guid: u64,
    player_guid: u64,
) -> Result<Vec<codec::GossipOptionView>> {
    let raw = store.gossip_options(npc_guid)?;
    Ok(raw
        .into_iter()
        .filter(|opt| {
            let (taken, rewarded) = store.quest_status(player_guid, opt.cond_value1);
            codec::option_condition_holds(opt.cond_type, taken, rewarded)
        })
        .collect())
}

/// Query / social family: name / creature / item lookups + the gossip / npc-text round-trips, plus
/// the social tier (say / yell / whisper chat + text emotes) — grouped as the stateless
/// request→reply / broadcast opcodes.
fn handle_query<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    // #468 stage 4b: the shared-calls path names the actor by guid; 0 (not in world) forces the
    // per-player path in the store.
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };

    match msg {
        // Name resolution: the client asks for a guid's name to render its plate (else "Unknown").
        //
        // #22 (whisper slice): resolved across every connected shard, not just this one. A guid the
        // client has met across a database boundary — the sender of a cross-shard whisper, which
        // arrives as a GUID because the client resolves whisper names itself — has no row on the
        // asking session's shard, and a dropped reply renders the line with nobody's name on it. On a
        // single-database gateway `world_stores()` is empty, so this is exactly the one read it was.
        ClientOpcodeMessage::CMSG_NAME_QUERY(q) => {
            let guid = q.guid.guid();
            match party::character_anywhere(store, guid)? {
                Some(c) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(Box::new(
                        codec::build_name_query_response(&c)?,
                    ))),
                )?,
                None => log::debug!("world: name query for unknown guid {guid}"),
            }
        }
        // Inspect (work-item 137): validate range + friendly target server-side (the `inspect`
        // reducer), then ack with SMSG_INSPECT(target guid) so the client opens the paperdoll — it
        // renders the target's equipment from fields the client already has (visible-item relay,
        // work-item 087, is the follow-up for full paperdoll correctness). Out of range / hostile /
        // no-such-target → the reducer errors and we silently drop the request, same as the other
        // stateless gates (CMSG_GAMEOBJ_USE, CMSG_AREATRIGGER).
        ClientOpcodeMessage::CMSG_INSPECT(i) => {
            let target_guid = i.guid.guid();
            match store.inspect(conn.account_id, self_guid, target_guid) {
                Ok(()) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_INSPECT(
                        codec::build_inspect_response(target_guid),
                    )),
                )?,
                Err(e) => log::debug!("world: inspect ignored (account {}): {e}", conn.account_id),
            }
        }
        // Creature name resolution (the NPC analogue of CMSG_NAME_QUERY).
        ClientOpcodeMessage::CMSG_CREATURE_QUERY(q) => {
            match store.creature_template(q.creature)? {
                Some(c) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(
                        codec::build_creature_query_response(&c),
                    ))),
                )?,
                None => log::debug!("world: creature query for unknown entry {}", q.creature),
            }
        }
        // Gossip (rank 12, extended by work-item 217): the player right-clicked a gossip NPC
        // (npc_flags GOSSIP bit). Reply with a title (resolved via the NPC_TEXT round-trip below) +
        // either the NPC's IMPORTED menu options (precedence) or the flag-derived vendor/innkeeper
        // synthesis (fallback) + the QUEST section. A gossip-FLAGGED questgiver (npc_flags
        // GOSSIP|QUESTGIVER, e.g. Marshal McBride) delivers its quests here, not via
        // CMSG_QUESTGIVER_HELLO, so fold the same quest menu in (empty for a plain gossip NPC →
        // unchanged).
        ClientOpcodeMessage::CMSG_GOSSIP_HELLO(h) => {
            let npc = h.guid.guid();
            let player_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            // 195 slice A: a gossip NPC that dislikes you doesn't open its menu (silent drop —
            // vanilla unfriendly NPCs just ignore the click).
            if player_guid != 0
                && store
                    .npc_refuses_interaction(npc, player_guid)
                    .unwrap_or(false)
            {
                return Ok(None);
            }
            let quests = match &conn.state {
                WorldState::InWorld(iw) => {
                    codec::quest_menu_items(&store.quest_giver_evals(npc, iw.self_guid)?)
                }
                WorldState::CharSelect => Vec::new(),
            };
            // Imported options (condition-filtered) take FULL precedence over the flag-derived
            // synthesis when present — see `codec::build_gossip_message`'s doc comment.
            let imported = filtered_gossip_options(store, npc, player_guid)?;
            // A vendor that ALSO has the gossip bit gets a "browse goods" menu entry (rank-vendor #6);
            // having stock is the is-vendor signal, so no npc_flags read is needed. An innkeeper gets a
            // "Make this inn your home." entry (hearthstone bind) — that one DOES need the npc_flags read.
            // (Both are ignored once `imported` is nonempty — fallback-only signals.)
            let is_vendor = !store.vendor_items(npc)?.is_empty();
            let is_innkeeper = store.npc_is_innkeeper(npc)?;
            let title_text_id = store.npc_gossip_text_id(npc);
            let imported_opt = (!imported.is_empty()).then_some(imported.as_slice());
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(Box::new(
                    codec::build_gossip_message(
                        npc,
                        title_text_id,
                        quests,
                        imported_opt,
                        is_vendor,
                        is_innkeeper,
                    ),
                ))),
            )?;
        }
        // The client resolves a gossip/quest title text id (sent in SMSG_GOSSIP_MESSAGE) → reply with
        // the NPC's imported (weighted, work-item 217) text, or the generic greeting when none is
        // imported yet.
        ClientOpcodeMessage::CMSG_NPC_TEXT_QUERY(q) => {
            let view = store.npc_text_for_id(q.text_id);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(Box::new(
                    codec::build_npc_text_update(q.text_id, view.as_ref()),
                ))),
            )?;
        }
        // The player clicked a gossip option. Imported menus (work-item 217) route by ACTION (vendor →
        // inventory, innkeeper → bind_home, trainer → SMSG_TRAINER_LIST, everything else including the
        // trailing Farewell → SMSG_GOSSIP_COMPLETE; submenu navigation is deferred, `action_menu_id`
        // stays inert). The fallback (no imported options) keeps the pre-217 vendor/innkeeper synthesis
        // byte-identical: option 0 on a stocked vendor is "browse goods" → the RAW SMSG_LIST_INVENTORY;
        // any other option (or a non-vendor NPC's "Farewell") closes the window.
        ClientOpcodeMessage::CMSG_GOSSIP_SELECT_OPTION(c) => {
            let npc = c.guid.guid();
            let player_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            // CRITICAL: re-derive with the IDENTICAL filter HELLO used (same helper, same player_guid)
            // — the position the client echoes back was assigned against THAT filtered list, so a
            // divergent filter here would route the wrong option (the "HELLO/SELECT_OPTION alignment"
            // trap — see the mock test of the same name).
            let imported = filtered_gossip_options(store, npc, player_guid)?;
            // STABLE option identity (283): resolve the clicked index → its game_gossip_option.row_id
            // BEFORE notifying the module, so a package handler keys on the row_id (immune to menu
            // position) not the volatile index. u32::MAX = the trailing Farewell / a synthesized line /
            // an out-of-range stale click (none of which is a package's minted option).
            let option_row_id = imported
                .get(c.gossip_list_id as usize)
                .map(|o| o.row_id)
                .unwrap_or(u32::MAX);
            // Notify the module (work-item 146: the on_gossip_select hook chokepoint) — best-effort,
            // so a module hiccup never blocks the gossip reply below.
            let _ = store.gossip_select(conn.account_id, self_guid, npc, c.gossip_list_id, option_row_id);
            if !imported.is_empty() {
                use lyracore_shared::constants::gossip_option;
                match imported.get(c.gossip_list_id as usize) {
                    Some(opt) if opt.action == gossip_option::VENDOR => {
                        let items = store.vendor_items(npc)?;
                        let (opcode, body) = codec::build_list_inventory_raw(npc, &items);
                        send(tx, Outbound::Raw { opcode, body })?;
                    }
                    Some(opt) if opt.action == gossip_option::INNKEEPER => {
                        // Bind failure (not in world) is per-action; close the window either way (the
                        // post-bind SMSG_BINDPOINTUPDATE confirmation is cosmetic — sent fresh at next
                        // login; the recall is server-authoritative regardless).
                        let _ = store.bind_home(conn.account_id, social::self_guid(conn).unwrap_or(0));
                        send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                    }
                    Some(opt) if opt.action == gossip_option::TRAINER => {
                        let spells = store.trainer_list(player_guid, npc)?;
                        let list = codec::build_trainer_list(
                            npc,
                            &spells,
                            "I can teach you a thing or two.",
                        );
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_LIST(Box::new(list))),
                        )?;
                    }
                    // BANKER/TAXI/plain-GOSSIP/submenu-link, the trailing Farewell, or an out-of-range
                    // index (a stale click racing a condition change) — close the window. Submenu
                    // navigation (`action_menu_id`) is deferred (217 scope).
                    _ => send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?,
                }
            } else {
                // Fallback (pre-217, byte-identical): vendor/innkeeper synthesis.
                let items = store.vendor_items(npc)?;
                let is_vendor = !items.is_empty();
                if is_vendor && c.gossip_list_id == codec::GOSSIP_OPTION_VENDOR {
                    let (opcode, body) = codec::build_list_inventory_raw(npc, &items);
                    send(tx, Outbound::Raw { opcode, body })?;
                } else if c.gossip_list_id == codec::gossip_option_innkeeper(is_vendor)
                    && store.npc_is_innkeeper(npc)?
                {
                    // "Make this inn your home." → bind the caller's hearthstone home to their current
                    // position (the module recall target).
                    let _ = store.bind_home(conn.account_id, social::self_guid(conn).unwrap_or(0));
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                } else {
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                }
            }
        }
        // Item template resolution (items slice-1): the client queries an item it has encountered
        // (it holds the object) for its name/tooltip/icon. Always reply — `build_item_query_response`
        // emits a NotFound (`found: None`) for an unknown entry so the client stops re-asking.
        ClientOpcodeMessage::CMSG_ITEM_QUERY_SINGLE(q) => {
            let resp = Box::new(codec::build_item_query_response(
                q.item,
                store.item_template(q.item)?.as_ref(),
            ));
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(resp)),
            )?;
        }
        // Social tier: say/yell -> send_chat (insert a broadcast game_chat_event the gateway fans back
        // as SMSG_MESSAGECHAT on every connection's subscription); whisper -> send_whisper (private,
        // per-recipient); party (work-item 199) -> party_chat (per-recipient, RLS-scoped to the
        // caller's CURRENT group). Guild/channel still need systems that don't exist yet and are
        // dropped. No reply on say/yell/party success (the speaker sees their own line via the
        // relay — party echoes back through the SAME per-recipient event a real member would get); a
        // rejected say/yell/whisper-target line is silently dropped, matching vanilla; a rejected
        // party line replies only for "not in a group" (see the Party arm below).
        //
        // GM playtest dot-commands (work-item 223): a Say line starting with `.` diverts BEFORE
        // `send_chat` — never broadcast, never inserted as a `game_chat_event` row — straight to the
        // module's one generic `gm_command` reducer. A reducer `Err` (bad gm_level, unknown command,
        // bad args) is relayed back to the SENDER ONLY as a system chat line (`SMSG_MESSAGECHAT`
        // System); success has no reply (the command's own effect — a teleport, a stat change — is
        // its own feedback).
        ClientOpcodeMessage::CMSG_MESSAGECHAT(c) => {
            let CMSG_MESSAGECHAT {
                chat_type,
                language,
                message,
            } = *c;
            let lang = language.as_int() as u8;
            match chat_type {
                CMSG_MESSAGECHAT_ChatType::Say if message.starts_with('.') => {
                    if let Err(e) = store.gm_command(conn.account_id, self_guid, message) {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                codec::build_gm_system_message(e.to_string()),
                            ))),
                        )?;
                    }
                }
                CMSG_MESSAGECHAT_ChatType::Say => {
                    let _ = store.send_chat(conn.account_id, self_guid, 0, lang, message);
                }
                CMSG_MESSAGECHAT_ChatType::Yell => {
                    let _ = store.send_chat(conn.account_id, self_guid, 1, lang, message);
                }
                // Channel (065): route to the membership-validated reducer; the relay fans it out
                // to members. A rejection (not joined / dead) is per-action — log + drop, vanilla
                // shows nothing for a failed channel line.
                CMSG_MESSAGECHAT_ChatType::Channel { channel } => {
                    if let Err(e) = store.send_channel_message(conn.account_id, self_guid, channel, message) {
                        log::debug!(
                            "world: channel message rejected (account {}): {e}",
                            conn.account_id
                        );
                    }
                }
                // Whisper: private delivery to a named player (+ a "To X:" echo to the sender). The
                // module resolves the name (case-insensitive) → recipient identity and RLS-scopes both
                // rows. A rejected whisper (no such online player) → SMSG_CHAT_PLAYER_NOT_FOUND so the
                // sender sees "No player named X is online" instead of a silent drop. (In practice the
                // only realistic failure is an unknown/offline target — a logged-in player is always
                // "in world" and the client never sends an empty whisper.)
                CMSG_MESSAGECHAT_ChatType::Whisper { target_player } => {
                    // #22 (whisper slice): the whisper goes through `world::whisper`, which decides
                    // WHICH DATABASE carries it — realm-core when the gateway is multi-database (the
                    // only plane on which a target standing on another shard can be named at all),
                    // the player's own shard otherwise. The reply is unchanged on both planes: any
                    // refusal is SMSG_CHAT_PLAYER_NOT_FOUND carrying the name the player typed.
                    //
                    // `social::self_guid` — never a literal: on the realm plane the sender's guid is
                    // an ARGUMENT to the operator-gated reducer, so the guid this socket
                    // authenticated with IS the authorization (a whisper attributed to somebody else
                    // is impersonation, and PR #49's survivor was exactly this substitution).
                    let sender = social::self_guid(conn);
                    if whisper::run(store, conn.account_id, sender, &target_player, message)
                        .is_err()
                    {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(
                                Box::new(SMSG_CHAT_PLAYER_NOT_FOUND {
                                    name: target_player,
                                }),
                            )),
                        )?;
                    }
                }
                // Party (`/p`, work-item 199): routes to the caller's CURRENT group members via
                // `party_chat` (the module validates membership + bounds; the gateway never reads
                // group state itself). Not in a group → the standard `SMSG_PARTY_COMMAND_RESULT`
                // "You aren't in a party" line (the SAME mapping `group_leave`/`group_uninvite`
                // already use for this exact reducer error — see `social::party_result_for`'s
                // needle list). Any OTHER rejection (not in world / empty message) is silently
                // dropped, matching say/yell — the client never sends an empty line anyway.
                CMSG_MESSAGECHAT_ChatType::Party => {
                    if let Err(e) = store.party_chat(conn.account_id, self_guid, message) {
                        if e.to_string()
                            .contains(lyracore_shared::group::err::NOT_IN_GROUP)
                        {
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(
                                    Box::new(codec::build_party_command_result(
                                        PartyOperation::Leave,
                                        String::new(),
                                        PartyResult::NotInGroup,
                                    )),
                                )),
                            )?;
                        }
                    }
                }
                _ => {} // guild/channel/etc. need systems that don't exist yet
            }
        }
        // Social tier: a text emote (/dance, /wave, …) → send_emote (insert a broadcast
        // game_emote_event the gateway fans back as SMSG_TEXT_EMOTE + SMSG_EMOTE). The client supplies
        // the social-emote id, the animation, and its selected target (0 guid = untargeted); the
        // gateway resolves the target guid to a name on relay. Failure is dropped.
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(c) => {
            let _ = store.send_emote(
                conn.account_id,
                self_guid,
                c.text_emote.as_int(),
                c.emote,
                c.target.guid(),
            );
        }
        // /roll: the client sends MSG_RANDOM_ROLL_Client (CMSG direction) with its min/max. We pick
        // a server-side result and broadcast MSG_RANDOM_ROLL_Server to all nearby players via the
        // game_roll_event broadcast table. Failure (not in world) is silently dropped.
        ClientOpcodeMessage::MSG_RANDOM_ROLL(r) => {
            let _ = store.send_roll(conn.account_id, self_guid, r.minimum, r.maximum);
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
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

    // #180: one process-wide admission gate shared by every accepted connection (`Arc` so each
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

    // #109 diagnosis: dump the motion-relay counters every 10s from a background task, so a failing
    // run is localised WITHOUT touching the relay hot path (a log there masks the race entirely).
    let queuestat_queue = login_queue.clone();
    tokio::spawn(async move {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut pc, mut ps, mut pd) = (0u64, 0u64, 0u64);
        let mut idle_logged = false;
        let mut prev_sub = 0u64;
        // #184: the AOI churn counter, read the same way MOTION_* is above — one atomic, one
        // 10s delta, no hot-path log line.
        let mut prev_recenters = 0u64;
        // Trailing fan-out baseline + collapsed-window run, for the #287 signal at the bottom of the
        // loop. Task-local: this is the only place it lives, and nothing on the relay path reads it.
        let mut fan = crate::stdb::subscriptions::FanoutHealth::default();
        // #180: QUEUESTAT baseline — same "diff against last reading" convention as MOTIONSTAT.
        let mut prev_admitted = queuestat_queue.admitted_total();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            // QUEUESTAT (#180): printed only while the line is NONEMPTY — an unconfigured or
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
                // Work-item 287: the ratios are FORMATTED here, not left to the operator's
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
            // #184: AOI subscription-churn counter — recenters (cell crossings → box
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
            // #109 HEALTH SIGNAL. The failure this catches is silent and permanent: the motion
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
                    "MOTION RELAY LOOKS DEAD (#109): {submitted_delta} movements submitted in the \
                     last 10s but the relay callback fired only {} times — peers are almost \
                     certainly frozen for connected players. calls={c} sent={s} completed={comp}. \
                     #109's own cause (the AOI recenter resubscribing a SHORTER query set, so the \
                     first cell crossing dropped game_entity_motion) cannot recur — since #468 there \
                     is no per-player AOI subscription to shorten. This firing now means the SHARED \
                     coordinator dispatch is not running: check the log for a `shared AOI dispatch` \
                     panic line, and that `coordinator connected to shard` was printed for every \
                     database. Restart the gateway to recover play.",
                    c.saturating_sub(pc)
                );
            }
            // FAN-OUT COLLAPSE SIGNAL (finishes work-item 287). The #109 check above only fires when
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
                    "PEER MOVEMENT UNDER-DELIVERING (#287): fan-out is {:.1} observers/move, down \
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
    // #451: the loop that `Error: Too many open files (os error 24)` came out of on 2026-08-07,
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
                         EMFILE, the gateway is out of file descriptors — see #449.",
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
            // #180: `queue` gates admission INSIDE the handshake — a queued connection just blocks
            // this one `spawn_blocking` thread, never the accept loop above.
            if let Err(e) = run_world_session_with_queue(std_sock, &coord, &queue) {
                log::warn!("world session {peer} ended: {e:#}");
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests;
