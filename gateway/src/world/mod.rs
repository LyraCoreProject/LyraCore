use crate::accept::{classify_accept_error, AcceptBackoff, AcceptOutcome};
use crate::codec::auth_session::{read_auth_session, CMSG_AUTH_SESSION_OPCODE};
use crate::read_deadline::WORLD_AUTH_DEADLINE;
use crate::stdb::PlayerSubscriptions;
use crate::{codec, config::GatewayConfig, stdb::Coordinator};
use anyhow::{anyhow, Result};
#[cfg(test)]
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::SendError;
#[cfg(test)]
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use wow_srp::normalized_string::NormalizedString;
use wow_srp::vanilla_header::{DecrypterHalf, EncrypterHalf, ProofSeed};
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{
    CMSG_MESSAGECHAT_ChatType, MovementInfo, PartyOperation, PartyResult,
    SMSG_CAST_RESULT_SimpleSpellCastResult, ServerMessage, WeatherChangeType, CMSG_MESSAGECHAT,
    SMSG_AUTH_CHALLENGE, SMSG_AUTH_RESPONSE, SMSG_CAST_RESULT, SMSG_CHAT_PLAYER_NOT_FOUND,
    SMSG_SPIRIT_HEALER_CONFIRM,
};

mod actor;
mod chat_flood;
mod coalesce;
pub(crate) mod egress;
mod entry;
mod family;
mod routing;
mod transport;
pub(crate) use family::{ProtocolFamily, ProtocolSession};
use family::{ProtocolReply, ProtocolRequest, WorldSessionAction};
pub(crate) mod guild_fee;
mod handlers;
pub mod login_queue;
pub mod loot;
pub mod mail;
pub mod packet_lint;
pub mod party;
pub(crate) mod party_mirror;
pub mod presence;
pub(crate) mod social;
mod store;
pub mod transfer;
pub mod whisper;
mod who;
pub(crate) use actor::Actor;
use coalesce::CoalesceState;
#[cfg(test)]
pub(crate) use handlers::MeetingStone;
pub(crate) use handlers::{
    character_facts, reconcile_deleted_guild_characters, DurableCharacterFacts, GuildCleanup,
};
use handlers::{decode_auction_browse, OpenLootState, CMSG_AUCTION_LIST_ITEMS_OPCODE};
pub(crate) use handlers::{
    member_stats_tick, resolve_online_character, zone_weather_message, AuctionBrowseRequest,
    AuctionHousePolicy, AuctionInteraction, AuctionPage, AuctionQuery, CancelAuctionOutcome,
    CancelAuctionRequest, ChannelOutcome, ChannelRequest, ChannelRoster, CharacterFacts,
    ChatOutcome, CreateAuctionOutcome, CreateAuctionRequest, GuildEventSnapshot, GuildOutcome,
    GuildRequest, ItemActionResult, LootActionStatus, LootWindowRefusal, LootWindowRequestStatus,
    MeetingStoneOutcome, MemberPresence, MemberStatsRecord, MemberStatsStore, PetRenameOutcome,
    PlaceBidOutcome, PlaceBidRequest, ReadableItem, RealmChatRequest, ResolvedTarget, SeekerFacts,
    SpeakerFacts, TrainerBuyOutcome, WeatherStore, WhisperRequest, WhisperTargetFacts,
};
pub(crate) use handlers::{
    AuctionActionStore, BankStore, CastStore, ChannelActionStore, CharacterStore, ChatActionStore,
    CombatStore, DeathStore, DuelActionStore, GuildActionStore, ItemActionStore, LootWindowStore,
    MeetingStoneActionStore, MeleeActionStore, NpcStore, QuestActionStore, SpeechStore,
    TaxiActionStore, TradeStore, TrainerStore, VendorActionStore,
};
use login_queue::{Admission, LoginQueue};
pub(crate) use loot::LootRollStore;
pub(crate) use mail::MailStore;
pub(crate) use party::PartyStore;

pub(crate) use social::ContactOutcome;
pub(crate) use social::SocialStore;
pub use store::{InteractionOutcome, WorldSessionToken, WorldStore};
pub(crate) use store::{SessionStore, ShardRoutingStore};
pub(crate) use transfer::TransferStore;

/// A Batch must stay contiguous so peer Relays cannot interrupt world entry.
pub enum Outbound {
    One(ServerOpcodeMessage),
    Batch(Vec<ServerOpcodeMessage>),
    Raw {
        opcode: u16,
        body: Vec<u8>,
    },
    Shared {
        opcode: u16,
        body: Arc<[u8]>,
    },
    /// SDK advancement is serial per Shard; defer encoding to the bounded worker pool.
    Job(RelayJob),
    /// A movement flush; it takes no lock held across an SDK message, so the writer runs it inline.
    Motion(RelayJob),
}

pub type RelayJob = Box<dyn FnOnce() -> Vec<Outbound> + Send>;

pub const EGRESS_SHED_DEPTH: usize = 512;

/// SDK callbacks must never wait for a World Session to drain its queue.
#[derive(Clone)]
pub struct SessionTx {
    tx: SessionSender,
    depth: Arc<AtomicUsize>,
    close_socket: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Clone)]
enum SessionSender {
    Async(egress::QueueSender),
    #[cfg(test)]
    Sync(Sender<Outbound>),
}

impl SessionTx {
    pub(crate) fn close(&self) {
        if let Some(close) = &self.close_socket {
            close();
        }
    }

    // Count before sending so depth cannot read low; undo the count if the send fails.
    pub fn send(&self, out: Outbound) -> std::result::Result<(), SendError<Outbound>> {
        match &self.tx {
            SessionSender::Async(sender) => sender.send(out),
            #[cfg(test)]
            SessionSender::Sync(sender) => {
                self.depth.fetch_add(1, Ordering::Relaxed);
                sender.send(out).inspect_err(|_| {
                    self.depth.fetch_sub(1, Ordering::Relaxed);
                })
            }
        }
    }

    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn depth_handle(&self) -> Arc<AtomicUsize> {
        self.depth.clone()
    }
}

#[cfg(test)]
pub fn session_channel() -> (SessionTx, Receiver<Outbound>, Arc<AtomicUsize>) {
    let (tx, rx) = channel::<Outbound>();
    let depth = Arc::new(AtomicUsize::new(0));
    (
        SessionTx {
            tx: SessionSender::Sync(tx),
            depth: depth.clone(),
            close_socket: None,
        },
        rx,
        depth,
    )
}

#[cfg(test)]
impl SessionTx {
    pub fn with_depth(depth: usize) -> (Self, Receiver<Outbound>) {
        let (tx, rx, d) = session_channel();
        d.store(depth, Ordering::Relaxed);
        (tx, rx)
    }
}

#[derive(Clone, Debug)]
pub struct WorldSession {
    pub account_id: u64,
    pub session_key: [u8; 40],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantRoute {
    Enchant(u32),
    Disenchant,
}

pub const MOVE_ACTIVITY_FLOOR: u64 = 100;

pub static MOVE_SUBMITTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[allow(clippy::large_enum_variant)]
pub enum WorldState {
    CharSelect,
    InWorld(InWorld),
}

pub struct GossipMenuSnapshot {
    pub npc_guid: u64,
    /// Zero for the entry-owned menu, otherwise the profile id checked by the Module at selection.
    pub source_menu_id: u32,
    /// Menu positions hold row ids and actions; synthesized rows use codec::SYNTHESIZED_ROW_ID.
    pub options: Vec<(u32, u32)>,
}

pub struct InWorld {
    pub self_guid: u64,
    pub subs: PlayerSubscriptions,
    pub attacking_target: Option<u64>,
    pub open_loot: OpenLootState,
    pub ranged_repeat: bool,
}

/// One writer must own the stateful header cipher; concurrent writes corrupt its stream.
pub struct WorldConn {
    pub(crate) protocol: ProtocolSession,
    session_claim: Option<WorldSessionToken>,
    pub decrypt: DecrypterHalf,
    move_coalesce: CoalesceState,
    unavailable_notices: std::collections::HashSet<handlers::UnavailableNotice>,
    store: RoutedStore,
    session_key: Option<[u8; 40]>,
    guild_signed_on: Option<u64>,
    move_desync_drops: u32,
    chat_flood: chat_flood::ChatFloodLimiter,
    entering: Option<login_queue::EntrySlot>,
}

impl std::ops::Deref for WorldConn {
    type Target = ProtocolSession;
    fn deref(&self) -> &Self::Target {
        &self.protocol
    }
}

impl std::ops::DerefMut for WorldConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.protocol
    }
}

const MOVE_DESYNC_TOLERANCE: u32 = 32;

const WHO_THROTTLE: Duration = Duration::from_secs(1);

pub(crate) struct RoutedStore {
    default: Arc<dyn WorldStore>,
    home: Option<Arc<dyn WorldStore>>,
}

impl RoutedStore {
    fn new(default: Arc<dyn WorldStore>) -> Self {
        Self {
            default,
            home: None,
        }
    }

    pub(crate) fn current(&self) -> Arc<dyn WorldStore> {
        self.home.as_ref().unwrap_or(&self.default).clone()
    }

    fn pin(&mut self, home: Arc<dyn WorldStore>) {
        self.home = Some(home);
    }

    fn unpin(&mut self) {
        self.home = None;
    }
}

impl WorldConn {
    fn route_home(&mut self, character_guid: u64) -> Result<()> {
        let resolved = self.store.current().settle_home_shard(character_guid)?;
        if let Some(h) = resolved {
            log::info!(
                "world: account {} (guid {character_guid}) routed to shard {}",
                self.account_id,
                h.shard_name()
            );
            self.store.pin(h);
        }
        if let (Some(h), Some(key)) = (&self.store.home, &self.session_key) {
            h.bind_shard_session(self.account_id, key)?;
        }
        Ok(())
    }

    /// Sign off from the Guild before releasing the Account Claim.
    fn leave_world(&mut self) -> Result<()> {
        let previous = std::mem::replace(&mut self.state, WorldState::CharSelect);
        let signed_on = self.guild_signed_on.take();
        let left = match &previous {
            WorldState::InWorld(iw) => Some(iw.self_guid),
            WorldState::CharSelect => signed_on,
        };
        drop(previous);
        let store = self.store.current();
        if let Some(character_guid) = left {
            handlers::guild_world_exit(&*store, character_guid);
        }
        let outcome = if let Some(token) = self.session_claim.take() {
            store.release_session(token)
        } else {
            Ok(())
        };
        self.store.unpin();
        outcome
    }
}

const QUEUE_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[cfg(not(test))]
const QUEUE_RESEND_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const QUEUE_RESEND_INTERVAL: Duration = Duration::from_millis(50);

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Clone, Copy)]
struct WriterTraceEntry {
    opcode: u16,
    size: u16,
    checksum: u64,
}

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

    fn dump(&self, account_id: u64, reason: &str) {
        self.dump_to(
            std::path::Path::new("/tmp/gw-writer-crash"),
            account_id,
            reason,
        );
    }

    fn dump_to(&self, dir: &std::path::Path, account_id: u64, reason: &str) {
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

struct AddonLimit {
    tokens: f32,
    refill_at: Instant,
    drop_logged_at: Option<Instant>,
}

impl Default for AddonLimit {
    fn default() -> Self {
        Self {
            tokens: 20.0,
            refill_at: Instant::now(),
            drop_logged_at: None,
        }
    }
}

fn process_frame(
    tx: &SessionTx,
    conn: &mut WorldConn,
    addon: &mut AddonLimit,
    hdr: wow_srp::vanilla_header::ClientHeader,
    body: Vec<u8>,
) -> Result<()> {
    if hdr.opcode == codec::addon::CMSG_MESSAGECHAT_OPCODE {
        if let Some((chat_type, text)) = codec::addon::parse_addon_client_chat(&body) {
            if codec::addon::is_bridge_prefixed(&text) {
                let now = std::time::Instant::now();
                addon.tokens = (addon.tokens
                    + now.duration_since(addon.refill_at).as_secs_f32() * 2.0)
                    .min(20.0);
                addon.refill_at = now;
                if addon.tokens >= 1.0 {
                    addon.tokens -= 1.0;
                    handle_addon_message(&*conn.store.current(), conn, &text)?;
                } else if addon
                    .drop_logged_at
                    .is_none_or(|t| now.duration_since(t).as_secs() >= 60)
                {
                    addon.drop_logged_at = Some(now);
                    log::warn!(
                        "addon bridge: dropping frames above the rate limit for account {}",
                        conn.account_id
                    );
                }
                return Ok(());
            }
            if !codec::addon::typed_addon_chat_type(chat_type) {
                return Ok(());
            }
        }
    }
    if hdr.opcode == CMSG_AUCTION_LIST_ITEMS_OPCODE {
        let request = decode_auction_browse(&body).map_err(|e| anyhow!("world read error: {e}"))?;
        let store = conn.store.current();
        dispatch(
            tx,
            &*store,
            conn,
            hdr.opcode,
            ProtocolRequest::AuctionBrowse(request),
        )?;
        return Ok(());
    }
    if hdr.opcode == CMSG_AUTH_SESSION_OPCODE {
        return Err(anyhow!("world read error: CMSG_AUTH_SESSION after auth"));
    }
    if hdr.opcode == 0x0125 {
        let store = conn.store.current();
        dispatch(
            tx,
            &*store,
            conn,
            hdr.opcode,
            ProtocolRequest::FactionAtWar(body),
        )?;
        return Ok(());
    }
    if hdr.opcode == 0x0318 {
        let store = conn.store.current();
        dispatch(
            tx,
            &*store,
            conn,
            hdr.opcode,
            ProtocolRequest::WatchedFaction(body),
        )?;
        return Ok(());
    }
    if handlers::is_silent_movement_request(hdr.opcode)
        || handlers::is_ignored_raw_request(hdr.opcode)
    {
        return Ok(());
    }
    if let Some(replies) = handlers::raw_unavailable_outbound(hdr.opcode, &body)? {
        if matches!(conn.state, WorldState::InWorld(_)) {
            for reply in replies {
                send(tx, reply)?;
            }
        }
        return Ok(());
    }
    let msg = match codec::read_movement_client(hdr.opcode, &body)? {
        Some(msg) => msg,
        None => {
            let mut framed = Vec::with_capacity(6 + body.len());
            framed.extend_from_slice(&hdr.size.to_be_bytes());
            framed.extend_from_slice(&hdr.opcode.to_le_bytes());
            framed.extend_from_slice(&body);
            ClientOpcodeMessage::read_unencrypted(&mut std::io::Cursor::new(framed))
                .map_err(|e| anyhow!("world read error: {e}"))?
        }
    };
    let store = conn.store.current();
    dispatch(tx, &*store, conn, hdr.opcode, msg.into())?;
    Ok(())
}

fn is_game_master<St: ChatActionStore + ?Sized>(store: &St, character_guid: u64) -> bool {
    match store.speaker_gm_level(character_guid) {
        Ok(gm_level) => gm_level > 0,
        Err(error) => {
            log::debug!("world: GM level read for {character_guid} failed: {error:#}");
            false
        }
    }
}

fn speaker_race<St: ChatActionStore + ?Sized>(store: &St, character_guid: u64) -> Option<u8> {
    match store.speaker_facts(character_guid) {
        Ok(facts) => facts.map(|facts| facts.race),
        Err(error) => {
            log::debug!("world: speaker facts read for {character_guid} failed: {error:#}");
            None
        }
    }
}

fn handle_addon_message<St: SessionStore + ?Sized>(
    store: &St,
    conn: &WorldConn,
    text: &str,
) -> Result<()> {
    let Some((cmd, payload)) = codec::addon::parse_bridge_envelope(text) else {
        log::debug!("addon bridge: non-STC or malformed frame dropped: {text:?}");
        return Ok(());
    };
    let Some(actor) = conn.actor() else {
        log::info!(
            "addon bridge: command {cmd:?} from account {} dropped outside the world",
            conn.account_id
        );
        return Ok(());
    };
    match store.client_command(actor, cmd.clone(), payload) {
        Ok(()) => Ok(()),
        Err(e) => match crate::stdb::classify(&e) {
            crate::stdb::DurableFailure::Refusal { .. } => {
                log::info!(
                    "addon bridge: command {cmd:?} from account {} refused: {e:#}",
                    conn.account_id
                );
                Ok(())
            }
            crate::stdb::DurableFailure::TransportLoss => Err(e),
        },
    }
}

fn is_desync_error(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("not in world") || s.contains("no live entity")
}

fn dispatch(
    tx: &SessionTx,
    store: &dyn WorldStore,
    conn: &mut WorldConn,
    opcode: u32,
    request: ProtocolRequest,
) -> Result<()> {
    let movement = match &request {
        ProtocolRequest::Message(message) => codec::relayed_move_opcode(message).is_some(),
        _ => false,
    };
    if !movement {
        if let Some((opcode, info)) = conn.move_coalesce.flush_now() {
            forward_movement(store, conn, opcode, &info)?;
        }
    }
    if let ProtocolRequest::Message(message) = &request {
        let speaker = conn.self_guid();
        if let Some(answer) = conn.chat_flood.judge(
            message,
            Instant::now(),
            || speaker.is_some_and(|guid| is_game_master(store, guid)),
            || speaker.and_then(|guid| speaker_race(store, guid)),
        ) {
            return send(tx, Outbound::One(answer));
        }
    }
    if let Some(owner) = routing::owner(opcode) {
        let reply = owner.handle(store, &mut conn.protocol, request)?;
        return complete_reply(tx, store, conn, reply);
    }
    let msg = request.message()?;
    if let Some(opcode) = codec::relayed_move_opcode(&msg) {
        // gtker types fall_time as f32, but vanilla stores u32 milliseconds; reinterpret the bits.
        if let ClientOpcodeMessage::MSG_MOVE_FALL_LAND(mv) = &msg {
            if let WorldState::InWorld(iw) = &conn.state {
                let fall_time_ms = if mv.info.flags.get_on_transport().is_some() {
                    0
                } else {
                    mv.info.fall_time.to_bits()
                };
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
    } else if let Some((mover, opcode, info)) = codec::acknowledged_move(&msg) {
        // A teleport outdates the pose; see docs/benilla-compatibility.md#root-receipts.
        let teleporting = match &conn.state {
            WorldState::InWorld(iw) => iw.subs.teleport_pending(),
            _ => false,
        };
        if conn.self_guid() == Some(mover) && !teleporting {
            for (fwd_opcode, fwd_info) in conn.move_coalesce.on_movement_now(opcode, &info) {
                forward_movement(store, conn, fwd_opcode, &fwd_info)?;
            }
        }
    } else {
        if let (ClientOpcodeMessage::MSG_MOVE_TELEPORT_ACK(_), WorldState::InWorld(iw)) =
            (&msg, &conn.state)
        {
            iw.subs.teleport_acknowledged();
        }
        if handlers::is_control_receipt(&msg) {
            return Ok(());
        }
        if let ClientOpcodeMessage::CMSG_SET_ACTIVE_MOVER(request) = &msg {
            if conn.self_guid() == Some(request.guid.guid()) {
                return Ok(());
            }
            return send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                    codec::build_gm_system_message(
                        "Control of another mover is not available on this realm.".into(),
                    ),
                ))),
            );
        }
        if let Some(outbound) = handlers::unavailable_outbound(&msg, &mut conn.unavailable_notices)
        {
            for message in outbound {
                send(tx, message)?;
            }
        } else {
            log::debug!("world: ignoring {msg} (account {})", conn.account_id);
        }
    }
    Ok(())
}

fn complete_reply(
    tx: &SessionTx,
    store: &dyn WorldStore,
    conn: &mut WorldConn,
    reply: ProtocolReply,
) -> Result<()> {
    reply.complete(tx, |action| match action {
        WorldSessionAction::Login(actor) => entry::login(tx, store, conn, actor),
        WorldSessionAction::WorldPortAck => entry::world_port_ack(tx, store, conn),
        WorldSessionAction::Logout => conn.leave_world(),
        WorldSessionAction::ArmTaxi(actor) => store.arm_taxi_flight(actor),
        WorldSessionAction::RedriveMail(actor) => {
            mail::redrive(store, actor.guid());
            Ok(())
        }
    })
}

fn forward_movement<St: SessionStore + ?Sized>(
    store: &St,
    conn: &mut WorldConn,
    opcode: u32,
    info: &MovementInfo,
) -> Result<()> {
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };
    let in_world = store.entity_in_world(self_guid);
    let Some(actor) = Actor::new(self_guid).filter(|_| in_world) else {
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
    };
    conn.move_desync_drops = 0;
    store.movement_update(actor, opcode, info)?;
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.subs.aoi_update(info.position.x, info.position.y);
    }
    Ok(())
}

fn send(tx: &SessionTx, out: Outbound) -> Result<()> {
    tx.send(out)
        .map_err(|_| anyhow!("world writer gone (client disconnected)"))
}

/// Benchmark-only: movement compression totals over the last ten-second window.
fn log_move_compression() {
    use crate::codec::movement::subscriptions as moves;
    use std::sync::atomic::Ordering::Relaxed;
    let runs = moves::MOVE_COMPRESS_RUNS.swap(0, Relaxed);
    if runs > 0 {
        let wire_in = moves::MOVE_COMPRESS_IN.swap(0, Relaxed);
        let wire_out = moves::MOVE_COMPRESS_OUT.swap(0, Relaxed);
        let nanos = moves::MOVE_COMPRESS_NANOS.swap(0, Relaxed);
        log::info!(
            "MOVECOMPRESS runs={:.1}/s in={:.0}B/s out={:.0}B/s ratio={:.3} deflate_cpu={:.3}cores mean={:.1}us",
            runs as f64 / 10.0,
            wire_in as f64 / 10.0,
            wire_out as f64 / 10.0,
            wire_out as f64 / wire_in as f64,
            nanos as f64 / 10e9,
            nanos as f64 / runs as f64 / 1e3,
        );
    }
}

pub async fn run(
    cfg: GatewayConfig,
    coordinator: Coordinator,
    login_queue: Arc<LoginQueue>,
) -> Result<()> {
    let listener = TcpListener::bind(&cfg.world_bind).await?;
    log::info!("world listening on {}", cfg.world_bind);

    let limit = |value: usize| {
        if value == 0 {
            "unlimited".to_string()
        } else {
            value.to_string()
        }
    };
    if login_queue.max_sessions() > 0 || login_queue.max_entering() > 0 {
        log::info!(
            "world: login queue ARMED — max_sessions={} admit_concurrency={} max_entering={}",
            limit(login_queue.max_sessions()),
            limit(login_queue.admit_concurrency()),
            limit(login_queue.max_entering()),
        );
    } else {
        log::info!(
            "world: login queue disabled (LYRACORE_MAX_SESSIONS and LYRACORE_MAX_ENTERING unset/0) — unlimited admission"
        );
    }

    let queuestat_queue = login_queue.clone();
    tokio::spawn(async move {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut pc, mut ps, mut pd) = (0u64, 0u64, 0u64);
        let mut idle_logged = false;
        let mut prev_sub = 0u64;
        let mut prev_recenters = 0u64;
        let (mut prev_handoffs, mut prev_encoded) = (0u64, 0u64);
        let mut fan = crate::stdb::subscriptions::FanoutHealth::default();
        let mut prev_admitted = queuestat_queue.admitted_total();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            queuestat_queue.tick();
            let depth = queuestat_queue.depth();
            let admitted = queuestat_queue.admitted_total();
            if depth > 0 {
                let admitted_per_s = admitted.saturating_sub(prev_admitted) as f64 / 10.0;
                let oldest = queuestat_queue.oldest_wait().unwrap_or_default();
                log::info!(
                    "QUEUESTAT depth={depth} admitted={admitted} (+{:.1}/s) active={} entering={} oldest_wait={:.1}s",
                    admitted_per_s,
                    queuestat_queue.active(),
                    queuestat_queue.entering(),
                    oldest.as_secs_f64(),
                );
            }
            prev_admitted = admitted;
            let (c, s, d) = (
                crate::stdb::subscriptions::MOTION_CALLS.load(Relaxed),
                crate::stdb::subscriptions::MOTION_SENT.load(Relaxed),
                crate::stdb::subscriptions::MOTION_DROPPED.load(Relaxed),
            );
            let sub = MOVE_SUBMITTED.load(Relaxed);
            if c != pc || s != ps || d != pd {
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
                    )
                );
                idle_logged = false;
            } else if c > 0 && !idle_logged {
                log::info!("MOTIONSTAT calls={c} sent={s} dropped={d} — relay went idle");
                idle_logged = true;
            }
            log_move_compression();
            let handoffs = crate::accept::BLOCKING_HANDOFFS.load(Relaxed);
            let encoded = egress::ENCODED_ENTRIES.load(Relaxed);
            let (d_handoffs, d_encoded) = (handoffs - prev_handoffs, encoded - prev_encoded);
            if d_handoffs > 0 {
                log::info!(
                    "HANDOFFSTAT blocking={:.1}/s encoded_entries={:.1}/s",
                    d_handoffs as f64 / 10.0,
                    d_encoded as f64 / 10.0,
                );
            }
            (prev_handoffs, prev_encoded) = (handoffs, encoded);
            let stages = crate::stage_timing::log_line();
            let pumps = crate::stage_timing::pump_lines(std::time::Duration::from_secs(10));
            if d_encoded > 0 {
                log::info!("{stages}");
                for pump in pumps {
                    log::info!("{pump}");
                }
            }
            let recenters = crate::stdb::aoi::AOI_RECENTERS.load(Relaxed);
            let d_recenters = recenters.saturating_sub(prev_recenters);
            if d_recenters > 0 {
                log::info!(
                    "AOISTAT recenters={recenters} (+{d_recenters} in 10s, {:.3}/s)",
                    d_recenters as f64 / 10.0
                );
            }
            prev_recenters = recenters;
            let submitted_delta = sub.saturating_sub(prev_sub);
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
    let mut backoff = AcceptBackoff::new();
    let mut capacity_was_full = false;
    let connections = Arc::new(tokio::sync::Semaphore::new(
        crate::config::world_connection_limit(),
    ));
    let actions = cfg.blocking_task_capacity.clone();
    let encoders = crate::accept::BlockingTaskCapacity::new(crate::config::encoding_workers());
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
                         EMFILE, the Gateway is out of file descriptors; inspect open sockets and \
                         the configured World connection limit.",
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
        let deadline = tokio::time::Instant::now() + WORLD_AUTH_DEADLINE;
        let Ok(connection_permit) = connections.clone().try_acquire_owned() else {
            if !capacity_was_full {
                log::warn!("world connection capacity is full; refusing new connections");
                capacity_was_full = true;
            }
            continue;
        };
        capacity_was_full = false;
        let coord = coordinator.clone();
        let queue = login_queue.clone();
        let actions = actions.clone();
        let encoders = encoders.clone();
        tokio::spawn(async move {
            if let Err(error) = transport::run_at(
                sock,
                Arc::new(coord),
                queue,
                actions,
                encoders,
                connection_permit,
                deadline,
            )
            .await
            {
                log::warn!("world session {peer} ended: {error:#}");
            }
        });
    }
}

#[cfg(all(test, unix))]
mod test_support;
#[cfg(all(test, unix))]
pub(crate) use test_support::run_world_session;
#[cfg(all(test, unix))]
use transport::world_handshake;
#[cfg(all(test, unix))]
pub(crate) mod tests;
