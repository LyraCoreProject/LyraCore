use crate::config::{GatewayConfig, ShardMap};
use anyhow::{anyhow, Context, Result};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table, TableWithPrimaryKey};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use super::account_sessions::SessionOwnership;
use super::bindings::*;
use super::movement_batch::MovementBatch;

static LIVE_CONN_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CharacterPresenceSnapshot {
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    pub(crate) present: bool,
}

#[derive(Clone)]
pub struct Coordinator(
    pub(crate) Arc<CoordinatorInner>,
    pub(crate) Arc<ShardSet>,
    pub(crate) Option<Arc<SessionOwnership>>,
);

pub(crate) struct ShardSet {
    map: ShardMap,
    pub(crate) package_party_gateway_id: String,
    conns: HashMap<String, Arc<CoordinatorInner>>,
    world: Arc<super::world_view::WorldView>,
    pub(crate) deleted_character_reconciliation_requested: AtomicBool,
    pub(crate) deleted_character_reconciliation_running: AtomicBool,
    pub(crate) guild_cleanup: std::sync::Mutex<crate::world::GuildCleanup>,
}

pub(crate) struct LiveConn {
    pub(crate) conn: Arc<DbConnection>,
    pub(crate) reducer_completion: Arc<ReducerCompletion>,
    _pump: Option<std::thread::JoinHandle<()>>,
    pump_commands: tokio::sync::mpsc::UnboundedSender<PumpCommand>,
    cache_generation: u64,
    character_revision: Arc<AtomicU64>,
    pub(crate) party_memberships: Arc<RwLock<PartyMembershipIndex>>,
    pub(crate) chat_channels: Arc<RwLock<super::reads::ChannelIndex>>,
    pub(crate) auctions: Arc<RwLock<super::auction_holds::AuctionIndex>>,
    pub(crate) mail_escrows: Arc<RwLock<super::reads::MailEscrowIndex>>,
    pub(crate) guilds: Arc<RwLock<super::reads::GuildIndex>>,
    pub(crate) contacts: Arc<RwLock<super::reads::ContactIndex>>,
    pub(crate) character_names: Arc<RwLock<super::reads::CharacterNameIndex>>,
    pub(crate) inventory: Arc<RwLock<super::reads::InventoryIndex>>,
    pub(crate) quests: Arc<RwLock<super::reads::QuestIndex>>,
    pub(crate) loot_tags: Arc<RwLock<super::reads::LootTagIndex>>,
    pub(crate) combat_reads: Arc<RwLock<super::reads::CombatIndex>>,
    cache_snapshot: Arc<RwLock<()>>,
    _sub: SubscriptionHandle,
}

pub(crate) type GroupMemberRows = (u64, Vec<(u64, u64)>);

#[derive(Default)]
pub(crate) struct PartyMembershipIndex {
    by_character: HashMap<u64, BTreeMap<u64, u64>>,
    by_group: HashMap<u64, BTreeMap<u64, u64>>,
}

impl PartyMembershipIndex {
    fn insert(&mut self, row: &GroupMember) {
        self.by_character
            .entry(row.character_guid)
            .or_default()
            .insert(row.id, row.group_id);
        self.by_group
            .entry(row.group_id)
            .or_default()
            .insert(row.id, row.character_guid);
    }

    fn remove(&mut self, row: &GroupMember) {
        if let Some(rows) = self.by_character.get_mut(&row.character_guid) {
            rows.remove(&row.id);
            if rows.is_empty() {
                self.by_character.remove(&row.character_guid);
            }
        }
        if let Some(rows) = self.by_group.get_mut(&row.group_id) {
            rows.remove(&row.id);
            if rows.is_empty() {
                self.by_group.remove(&row.group_id);
            }
        }
    }

    pub(crate) fn group_of(&self, character_guid: u64) -> Option<u64> {
        self.by_character
            .get(&character_guid)?
            .values()
            .next()
            .copied()
    }

    pub(crate) fn contains(&self, character_guid: u64, group_id: u64) -> bool {
        self.by_character
            .get(&character_guid)
            .is_some_and(|rows| rows.values().any(|group| *group == group_id))
    }

    pub(crate) fn member_row_ids(&self, group_id: u64) -> Vec<u64> {
        self.by_group
            .get(&group_id)
            .map(|rows| rows.keys().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn bounded_member_rows(
        &self,
        character_guid: u64,
        member_limit: usize,
    ) -> Result<Option<GroupMemberRows>> {
        let Some(memberships) = self.by_character.get(&character_guid) else {
            return Ok(None);
        };
        if memberships.len() != 1 {
            anyhow::bail!("party command character has more than one membership");
        }
        let group_id = *memberships.values().next().expect("one membership");
        let Some(group) = self.by_group.get(&group_id) else {
            return Ok(None);
        };
        let rows: Vec<_> = group
            .iter()
            .take(member_limit + 1)
            .map(|(row_id, guid)| (*row_id, *guid))
            .collect();
        if rows.len() > member_limit {
            anyhow::bail!("party command roster exceeds the member limit");
        }
        Ok(Some((group_id, rows)))
    }
}

#[cfg(test)]
mod party_membership_index_tests {
    use super::{GroupMember, PartyMembershipIndex};

    fn member(id: u64, group_id: u64, character_guid: u64) -> GroupMember {
        GroupMember {
            id,
            group_id,
            character_guid,
            owner_identity: spacetimedb_sdk::Identity::ZERO,
            raid_slot: 0,
        }
    }

    #[test]
    fn command_rosters_use_join_order_and_refuse_duplicate_or_oversized_membership() {
        let mut index = PartyMembershipIndex::default();
        let first = member(2, 7, 102);
        let second = member(1, 7, 101);
        index.insert(&first);
        index.insert(&second);
        assert_eq!(
            index.bounded_member_rows(102, 2).unwrap(),
            Some((7, vec![(1, 101), (2, 102)]))
        );
        assert!(index.bounded_member_rows(102, 1).is_err());

        let duplicate = member(3, 8, 102);
        index.insert(&duplicate);
        assert!(index.bounded_member_rows(102, 5).is_err());
        index.remove(&duplicate);
        assert_eq!(
            index.bounded_member_rows(102, 2).unwrap(),
            Some((7, vec![(1, 101), (2, 102)]))
        );
    }

    #[test]
    fn a_members_group_and_the_groups_rows_are_indexed_in_join_order() {
        let mut index = PartyMembershipIndex::default();
        index.insert(&member(9, 7, 102));
        index.insert(&member(4, 7, 101));
        assert_eq!(index.group_of(102), Some(7));
        assert_eq!(index.group_of(999), None);
        assert_eq!(index.member_row_ids(7), [4, 9]);
        index.remove(&member(4, 7, 101));
        assert_eq!(index.member_row_ids(7), [9]);
        assert!(index.member_row_ids(8).is_empty());
    }
}

type PumpWork<C = DbConnection> = Box<dyn FnOnce(&C) + Send>;

enum PumpCommand<C = DbConnection> {
    Run(PumpWork<C>),
    AfterMessage(Box<dyn Fn() + Send>),
    Park(std::sync::mpsc::Sender<std::result::Result<(), String>>),
    Resume,
    Stop,
}

async fn pump_messages<C, E, F: std::future::Future<Output = std::result::Result<(), E>>>(
    conn: &C,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<PumpCommand<C>>,
    advance: impl Fn() -> F,
) -> std::result::Result<(), E> {
    let mut parked = false;
    let mut after_message: Option<Box<dyn Fn() + Send>> = None;
    loop {
        tokio::select! {
            // SubscribeApplied queues Park before returning; consume it before another SDK message.
            biased;
            command = commands.recv() => match command {
                Some(PumpCommand::Run(work)) => work(conn),
                Some(PumpCommand::AfterMessage(hook)) => after_message = Some(hook),
                Some(PumpCommand::Park(applied)) => {
                    parked = true;
                    let _ = applied.send(Ok(()));
                }
                Some(PumpCommand::Resume) => parked = false,
                Some(PumpCommand::Stop) | None => return Ok(()),
            },
            result = advance(), if !parked => {
                result?;
                if let Some(hook) = &after_message {
                    hook();
                }
            }
        }
    }
}

async fn advance_cache_snapshot<T>(
    snapshot: &RwLock<()>,
    timing: &crate::stage_timing::PumpTiming,
    next: impl std::future::Future<Output = T>,
) -> T {
    let mut next = std::pin::pin!(next);
    std::future::poll_fn(|context| {
        let started = std::time::Instant::now();
        let _updating = snapshot.write().unwrap();
        let poll = std::future::Future::poll(next.as_mut(), context);
        if poll.is_ready() {
            timing.record_message(started);
        }
        poll
    })
    .await
}

#[cfg(test)]
mod cache_snapshot_tests {
    use super::advance_cache_snapshot;
    use std::future::Future;
    use std::sync::RwLock;
    use std::task::Poll;

    #[tokio::test]
    async fn awaiting_a_message_leaves_the_cache_readable() {
        let snapshot = RwLock::new(());
        let timing = crate::stage_timing::pump_timing("test");
        let mut next = std::pin::pin!(advance_cache_snapshot(
            &snapshot,
            &timing,
            std::future::pending::<()>(),
        ));
        std::future::poll_fn(|context| {
            assert!(next.as_mut().poll(context).is_pending());
            assert!(snapshot.try_read().is_ok());
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn an_applied_cache_is_unreadable_until_all_index_callbacks_finish() {
        let snapshot = RwLock::new(());
        let cache = std::sync::atomic::AtomicU64::new(0);
        let index = std::sync::atomic::AtomicU64::new(0);
        let order = std::sync::atomic::Ordering::Relaxed;
        advance_cache_snapshot(
            &snapshot,
            &crate::stage_timing::pump_timing("test"),
            async {
                cache.store(7, order);
                assert!(snapshot.try_read().is_err());
                index.store(7, order);
            },
        )
        .await;
        let _read = snapshot.read().unwrap();
        assert_eq!(cache.load(order), 7);
        assert_eq!(index.load(order), 7);
    }
}

// SDK 2.7.1 drains pending registrations before the next message, preventing a snapshot gap.
fn start_pump(
    conn: &Arc<DbConnection>,
    role: &str,
    cache_snapshot: Arc<RwLock<()>>,
) -> Result<(
    std::thread::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedSender<PumpCommand>,
)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<PumpCommand>();
    let conn = conn.clone();
    let role = role.to_owned();
    let thread = std::thread::Builder::new()
        .name("stdb-pump".into())
        .spawn(move || {
            let timing = crate::stage_timing::pump_timing(&role);
            let result = runtime.block_on(pump_messages(conn.as_ref(), rx, || {
                advance_cache_snapshot(&cache_snapshot, &timing, conn.advance_one_message_async())
            }));
            if let Err(error) = result {
                if !matches!(error, spacetimedb_sdk::Error::Disconnected) {
                    log::warn!("{role} pump stopped: {error}");
                }
            }
        })?;
    Ok((thread, tx))
}

impl LiveConn {
    // Store reads and queued Relay jobs only; SDK row callbacks hold the snapshot lock.
    pub(crate) fn with_cache_snapshot<T>(&self, read: impl FnOnce() -> T) -> T {
        let _snapshot = match self.cache_snapshot.try_read() {
            Ok(snapshot) => snapshot,
            Err(std::sync::TryLockError::WouldBlock) => {
                let waiting = std::time::Instant::now();
                let snapshot = self.cache_snapshot.read().unwrap();
                crate::stage_timing::SNAPSHOT_WAIT.record(waiting.elapsed());
                snapshot
            }
            Err(std::sync::TryLockError::Poisoned(_)) => self.cache_snapshot.read().unwrap(),
        };
        read()
    }

    pub(super) fn is_healthy(&self) -> bool {
        self.conn.is_active()
            && self._sub.is_active()
            && self._pump.as_ref().is_some_and(|pump| !pump.is_finished())
    }

    pub(super) fn durable_character_presence_snapshot(
        &self,
        guid: u64,
        shard_name: &str,
    ) -> Result<CharacterPresenceSnapshot> {
        if !self.is_healthy() {
            return Err(anyhow!(
                "World Shard {shard_name} has no healthy Coordinator subscription"
            ));
        }
        let generation = self.cache_generation;
        let revision = self.character_revision.clone();
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<_, String>>();
        let tx_error = tx.clone();
        let query = format!("SELECT * FROM game_character WHERE guid = {guid}");
        // on_applied gives a fresh snapshot; the cache can lag another Gateway's commit.
        let subscription = self
            .conn
            .subscription_builder()
            .on_applied(move |ctx| {
                let _ = tx.send(Ok(CharacterPresenceSnapshot {
                    generation,
                    revision: revision.load(Ordering::Acquire),
                    present: ctx.db.game_character().guid().find(&guid).is_some(),
                }));
            })
            .on_error(move |_ctx, error| {
                let _ = tx_error.send(Err(error.to_string()));
            })
            .subscribe(query);
        let snapshot = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(snapshot)) => Ok(snapshot),
            Ok(Err(error)) => Err(anyhow!(
                "World Shard {shard_name} Character presence subscription failed: {error}"
            )),
            Err(error) => Err(anyhow!(
                "World Shard {shard_name} Character presence subscription did not apply: {error}"
            )),
        };
        if let Err(error) = subscription.unsubscribe() {
            log::warn!(
                "World Shard {shard_name} Character presence subscription could not stop: {error}"
            );
            if snapshot.is_ok() {
                return Err(anyhow!(
                    "World Shard {shard_name} Character presence subscription could not stop: \
                     {error}"
                ));
            }
        }
        snapshot
    }

    pub(super) fn character_revision(&self) -> Arc<AtomicU64> {
        self.character_revision.clone()
    }
}

fn replace_slot_ordered<T>(
    slot: &RwLock<T>,
    fresh: T,
    role_label: &str,
    retire: impl FnOnce(&RwLock<T>),
    post_install: impl FnOnce(),
    teardown: impl FnOnce(T),
) {
    retire(slot);
    let old = {
        let mut live = slot.write().unwrap_or_else(|poisoned| {
            log::error!("{role_label} live-slot lock poisoned during replacement; recovering");
            poisoned.into_inner()
        });
        std::mem::replace(&mut *live, fresh)
    };
    post_install();
    teardown(old);
}

// An unwinding hook must stop the fresh pump before partially installed Relays can run.
fn finish_relay_setup<C>(
    commands: &tokio::sync::mpsc::UnboundedSender<PumpCommand<C>>,
    setup: impl FnOnce(),
) {
    struct Completion<'a, C> {
        commands: &'a tokio::sync::mpsc::UnboundedSender<PumpCommand<C>>,
        complete: bool,
    }
    impl<C> Drop for Completion<'_, C> {
        fn drop(&mut self) {
            let command = if self.complete {
                PumpCommand::Resume
            } else {
                PumpCommand::Stop
            };
            let _ = self.commands.send(command);
        }
    }
    let mut completion = Completion {
        commands,
        complete: false,
    };
    setup();
    completion.complete = true;
}

// Drain the old pump before publishing the fresh cache; release the slot lock before joining.
fn replace_live_conn(
    slot: &RwLock<LiveConn>,
    fresh: LiveConn,
    role_label: &str,
    post_install: impl FnOnce(),
) {
    let fresh_commands = fresh.pump_commands.clone();
    replace_slot_ordered(
        slot,
        fresh,
        role_label,
        |slot| {
            let (conn, pump, commands) = {
                let mut live = slot
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (
                    live.conn.clone(),
                    live._pump.take(),
                    live.pump_commands.clone(),
                )
            };
            if let Err(error) = conn.disconnect() {
                if !matches!(error, spacetimedb_sdk::Error::Disconnected) {
                    log::warn!("{role_label} old connection disconnect failed: {error}");
                }
            }
            let _ = commands.send(PumpCommand::Resume);
            if let Some(pump) = pump {
                if let Err(error) = pump.join() {
                    log::warn!("{role_label} old pump thread panicked: {error:?}");
                }
            }
        },
        || {
            log::info!("{role_label} replacement installed");
            finish_relay_setup(&fresh_commands, post_install);
        },
        drop,
    );
}

struct CallPipe {
    live: RwLock<LiveConn>,
    reconnecting: AtomicBool,
}

// Remove completion senders under the lock so result and Transport Loss choose one outcome.
pub(crate) struct ReducerCompletion {
    next_id: AtomicU64,
    state: Mutex<ReducerCompletionState>,
}

struct ReducerCompletionState {
    connected: bool,
    pending:
        HashMap<u64, std::sync::mpsc::Sender<std::result::Result<(), ReducerCompletionFailure>>>,
}

#[derive(Debug, Clone)]
pub(crate) enum ReducerCompletionFailure {
    Rejected(String),
    Internal(spacetimedb_sdk::error::InternalError),
    TransportDisconnected,
}

#[derive(Debug)]
pub(crate) enum ReducerCallError {
    Rejected {
        operation: String,
        reason: String,
    },
    Internal {
        operation: String,
        source: spacetimedb_sdk::error::InternalError,
    },
    SdkSend {
        operation: String,
        source: spacetimedb_sdk::Error,
    },
    Fatal(String),
}

impl ReducerCallError {
    fn rejected(operation: &str, reason: String) -> Self {
        Self::Rejected {
            operation: operation.to_string(),
            reason,
        }
    }

    fn internal(operation: &str, source: spacetimedb_sdk::error::InternalError) -> Self {
        Self::Internal {
            operation: operation.to_string(),
            source,
        }
    }

    pub(crate) fn sdk_send(operation: &str, source: spacetimedb_sdk::Error) -> Self {
        Self::SdkSend {
            operation: operation.to_string(),
            source,
        }
    }

    pub(crate) fn fatal(message: String) -> Self {
        Self::Fatal(message)
    }

    #[cfg(test)]
    pub(crate) fn refused(operation: &str, reason: &str) -> Self {
        Self::rejected(operation, reason.to_string())
    }

    #[cfg(test)]
    pub(crate) fn transport_lost(operation: &str) -> Self {
        Self::fatal(format!(
            "{operation} reducer failed: transport disconnected"
        ))
    }
}

impl std::fmt::Display for ReducerCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { operation, reason } => {
                write!(f, "{operation} reducer failed: {reason}")
            }
            Self::Internal { operation, source } => {
                write!(f, "{operation} reducer failed: {source:?}")
            }
            Self::SdkSend { operation, source } => write!(f, "send {operation}: {source}"),
            Self::Fatal(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ReducerCallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal { source, .. } => Some(source),
            Self::SdkSend { source, .. } => Some(source),
            Self::Rejected { .. } | Self::Fatal(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DurableFailure<'e> {
    Refusal { reason: &'e str },
    TransportLoss,
}

pub(crate) fn classify(error: &anyhow::Error) -> DurableFailure<'_> {
    match reducer_refusal_reason(error) {
        Some(reason) => DurableFailure::Refusal { reason },
        None => DurableFailure::TransportLoss,
    }
}

pub(crate) fn reducer_refusal_reason(error: &anyhow::Error) -> Option<&str> {
    error.chain().find_map(|cause| {
        let ReducerCallError::Rejected { reason, .. } = cause.downcast_ref::<ReducerCallError>()?
        else {
            return None;
        };
        Some(reason.as_str())
    })
}

impl ReducerCompletion {
    fn connected() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            state: Mutex::new(ReducerCompletionState {
                connected: true,
                pending: HashMap::new(),
            }),
        }
    }

    pub(crate) fn register(
        &self,
        tx: std::sync::mpsc::Sender<std::result::Result<(), ReducerCompletionFailure>>,
    ) -> std::result::Result<u64, String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.connected {
            return Err("transport disconnected".to_string());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        state.pending.insert(id, tx);
        Ok(id)
    }

    pub(crate) fn finish(
        &self,
        id: u64,
        result: std::result::Result<(), ReducerCompletionFailure>,
    ) {
        if let Some(tx) = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pending
            .remove(&id)
        {
            let _ = tx.send(result);
        }
    }

    pub(crate) fn cancel(&self, id: u64) {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pending
            .remove(&id);
    }

    fn disconnect(&self) {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.connected = false;
            std::mem::take(&mut state.pending)
        };
        for (_, tx) in pending {
            let _ = tx.send(Err(ReducerCompletionFailure::TransportDisconnected));
        }
    }
}

pub(crate) struct CoordinatorInner {
    live: RwLock<LiveConn>,
    token: Option<String>,
    uri: String,
    db_name: String,
    subscription: CoordinatorSubscription,
    call_pipes: Vec<CallPipe>,
    call_pipe_next: std::sync::atomic::AtomicUsize,
    pub(crate) party_command_lane_overflow: AtomicBool,
    pub(crate) bot_transfer_pending_overflow: AtomicBool,
    pub(crate) motion_batch: MovementBatch,
    pub(crate) on_reconnect: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

impl CoordinatorInner {
    pub(crate) fn coord(&self) -> std::sync::RwLockReadGuard<'_, LiveConn> {
        self.live.read().unwrap_or_else(|p| {
            log::error!(
                "coordinator read lock poisoned (a prior panic in a critical section) — recovering"
            );
            p.into_inner()
        })
    }

    pub(crate) fn on_pump(&self, work: impl FnOnce(&DbConnection) + Send + 'static) -> Result<()> {
        let pump = self.coord().pump_commands.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        pump.send(PumpCommand::Run(Box::new(move |conn| {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(conn)));
            let _ = tx.send(outcome.map_err(|_| "Relay setup panicked"));
        })))
        .map_err(|_| anyhow!("{} Coordinator pump is stopped", self.db_name))?;
        rx.recv_timeout(Duration::from_secs(15))
            .map_err(|error| anyhow!("{} Relay setup did not complete: {error}", self.db_name))?
            .map_err(|error| anyhow!("{} {error}", self.db_name))
    }

    /// Runs `hook` after each SDK message applies its row callbacks; replaces any earlier hook.
    pub(crate) fn after_each_message(&self, hook: impl Fn() + Send + 'static) -> Result<()> {
        let timing = crate::stage_timing::pump_timing(&coordinator_role_label(&self.db_name));
        self.coord()
            .pump_commands
            .send(PumpCommand::AfterMessage(Box::new(move || {
                let started = std::time::Instant::now();
                hook();
                timing.record_hook(started.elapsed());
            })))
            .map_err(|_| anyhow!("{} Coordinator pump is stopped", self.db_name))
    }

    pub(crate) fn call_pipe(&self) -> std::sync::RwLockReadGuard<'_, LiveConn> {
        let n = self.call_pipes.len();
        if n > 0 {
            let start = self
                .call_pipe_next
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(index) = select_active_call_pipe(n, start, |index| {
                self.call_pipes[index]
                    .live
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_healthy()
            }) {
                return self.call_pipes[index]
                    .live
                    .read()
                    .unwrap_or_else(|p| p.into_inner());
            }
            log::warn!(
                "call-pipe pool: every pipe inactive — falling back to the coordinator connection"
            );
        }
        self.coord()
    }

    /// Row Relays queue before success presentation; waiting here on the pump would deadlock.
    pub(crate) fn visibility_pipe(&self) -> std::sync::RwLockReadGuard<'_, LiveConn> {
        self.coord()
    }
}

fn select_active_call_pipe(
    pipe_count: usize,
    start: usize,
    mut is_active: impl FnMut(usize) -> bool,
) -> Option<usize> {
    (0..pipe_count)
        .map(|offset| (start + offset) % pipe_count)
        .find(|&index| is_active(index))
}

const CALL_PIPE_LIVENESS_QUERY: &str = "SELECT * FROM game_config";

fn call_pipe_queries() -> Vec<&'static str> {
    vec![CALL_PIPE_LIVENESS_QUERY]
}

fn coordinator_role_label(db_name: &str) -> String {
    format!("shard {db_name} coordinator")
}

fn call_pipe_role_label(db_name: &str, index: usize) -> String {
    format!("shard {db_name} call pipe {index}")
}

fn connect_subscribed(
    uri: String,
    db_name: String,
    token: Option<String>,
    queries: Vec<&'static str>,
    role_label: String,
    subscription_timeout_hint: Option<&'static str>,
    park_for_relays: bool,
) -> Result<LiveConn> {
    let reducer_completion = Arc::new(ReducerCompletion::connected());
    let disconnected = reducer_completion.clone();
    let connect_label = role_label.clone();
    let connect_error_label = role_label.clone();
    let disconnect_label = role_label.clone();
    let conn = DbConnection::builder()
        .with_uri(&uri)
        .with_database_name(&db_name)
        .with_token(token)
        .on_connect(move |_ctx, identity, _token| {
            log::info!("{connect_label} connected as {identity}")
        })
        .on_connect_error(move |_ctx, error| {
            log::error!("{connect_error_label} connect error: {error}")
        })
        .on_disconnect(move |_ctx, err| {
            log::warn!("{disconnect_label} connection closed: {err:?}");
            disconnected.disconnect();
        })
        .build()
        .map_err(|error| anyhow!("{role_label} build/connect failed: {error}"))?;

    let conn = Arc::new(conn);
    let cache_snapshot = Arc::new(RwLock::new(()));
    let (pump, pump_commands) = start_pump(&conn, &role_label, cache_snapshot.clone())?;
    let party_memberships = Arc::new(RwLock::new(PartyMembershipIndex::default()));
    let inserted_memberships = party_memberships.clone();
    conn.db.game_group_member().on_insert(move |_ctx, row| {
        inserted_memberships.write().unwrap().insert(row);
    });
    let deleted_memberships = party_memberships.clone();
    conn.db.game_group_member().on_delete(move |_ctx, row| {
        deleted_memberships.write().unwrap().remove(row);
    });
    let updated_memberships = party_memberships.clone();
    conn.db
        .game_group_member()
        .on_update(move |_ctx, old, new| {
            let mut memberships = updated_memberships.write().unwrap();
            memberships.remove(old);
            memberships.insert(new);
        });
    let chat_channels = watch_chat_channels(&conn);
    let auctions = super::auction_holds::watch_auctions(&conn);
    let mail_escrows = super::reads::watch_mail_escrows(&conn);
    let guilds = super::reads::watch_guilds(&conn);
    let contacts = super::reads::watch_contacts(&conn);
    let character_names = super::reads::watch_character_names(&conn);
    let inventory = super::reads::watch_inventory(&conn);
    let quests = super::reads::watch_quest(&conn);
    let loot_tags = super::reads::watch_loot_tag(&conn);
    let combat_reads = super::reads::watch_combat(&conn);
    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let tx_err = tx.clone();
    let applied_commands = pump_commands.clone();
    let sub = conn
        .subscription_builder()
        .on_applied(move |_ctx| {
            if park_for_relays {
                let _ = applied_commands.send(PumpCommand::Park(tx));
            } else {
                let _ = tx.send(Ok(()));
            }
        })
        .on_error(move |_ctx, err| {
            let _ = tx_err.send(Err(format!("{err}")));
        })
        .subscribe(queries);

    let applied = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow!("{role_label} subscription error: {error}")),
        Err(_) => {
            let hint = subscription_timeout_hint
                .map(|hint| format!(" ({hint})"))
                .unwrap_or_default();
            Err(anyhow!(
                "{role_label} subscriptions not applied within 15s{hint}"
            ))
        }
    };
    if let Err(error) = applied {
        let _ = conn.disconnect();
        // A late apply can park the unpublished pump after timeout; Stop must handle either state.
        let _ = pump_commands.send(PumpCommand::Stop);
        if let Err(panic) = pump.join() {
            log::warn!("{role_label} failed subscription pump panicked: {panic:?}");
        }
        return Err(error);
    }
    log::info!("{role_label} subscriptions applied");

    Ok(LiveConn {
        conn,
        reducer_completion,
        _pump: Some(pump),
        pump_commands,
        cache_generation: LIVE_CONN_GENERATION.fetch_add(1, Ordering::Relaxed),
        character_revision: Arc::new(AtomicU64::new(0)),
        party_memberships,
        chat_channels,
        auctions,
        mail_escrows,
        guilds,
        contacts,
        character_names,
        inventory,
        quests,
        loot_tags,
        combat_reads,
        cache_snapshot,
        _sub: sub,
    })
}

fn watch_chat_channels(conn: &DbConnection) -> Arc<RwLock<super::reads::ChannelIndex>> {
    let index = Arc::new(RwLock::new(super::reads::ChannelIndex::default()));
    let inserted = index.clone();
    conn.db.game_chat_channel().on_insert(move |_ctx, row| {
        inserted.write().unwrap().insert_channel(row);
    });
    let updated = index.clone();
    conn.db
        .game_chat_channel()
        .on_update(move |_ctx, old, new| {
            let mut channels = updated.write().unwrap();
            channels.remove_channel(old);
            channels.insert_channel(new);
        });
    let deleted = index.clone();
    conn.db.game_chat_channel().on_delete(move |_ctx, row| {
        deleted.write().unwrap().remove_channel(row);
    });
    let inserted = index.clone();
    conn.db
        .game_chat_channel_member()
        .on_insert(move |_ctx, row| {
            inserted.write().unwrap().insert_member(row);
        });
    let updated = index.clone();
    conn.db
        .game_chat_channel_member()
        .on_update(move |_ctx, old, new| {
            let mut channels = updated.write().unwrap();
            channels.remove_member(old);
            channels.insert_member(new);
        });
    let deleted = index.clone();
    conn.db
        .game_chat_channel_member()
        .on_delete(move |_ctx, row| {
            deleted.write().unwrap().remove_member(row);
        });
    index
}

fn connect_call_pipe(
    uri: String,
    db_name: String,
    token: Option<String>,
    index: usize,
) -> Result<LiveConn> {
    let role_label = call_pipe_role_label(&db_name, index);
    connect_subscribed(
        uri,
        db_name,
        token,
        call_pipe_queries(),
        role_label,
        None,
        false,
    )
}

fn repair_dead_call_pipes(inner: &Arc<CoordinatorInner>) {
    for (index, pipe) in inner.call_pipes.iter().enumerate() {
        let is_healthy = pipe
            .live
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_healthy();
        let already_reconnecting = pipe.reconnecting.swap(true, Ordering::AcqRel);
        if !call_pipe_needs_repair(is_healthy, already_reconnecting) {
            if is_healthy {
                pipe.reconnecting.store(false, Ordering::Release);
            }
            continue;
        }

        let repair = inner.clone();
        let uri = inner.uri.clone();
        let db_name = inner.db_name.clone();
        let token = inner.token.clone();
        std::thread::Builder::new()
            .name(format!("stdb-call-pipe-reconnect-{index}"))
            .spawn(move || {
                let role_label = call_pipe_role_label(&db_name, index);
                match connect_call_pipe(uri, db_name, token, index) {
                    Ok(fresh) => {
                        replace_live_conn(
                            &repair.call_pipes[index].live,
                            fresh,
                            &role_label,
                            || {},
                        );
                    }
                    Err(error) => {
                        log::error!("{role_label} reconnect failed (will retry): {error:#}")
                    }
                }
                repair.call_pipes[index]
                    .reconnecting
                    .store(false, Ordering::Release);
            })
            .expect("spawn call-pipe reconnect thread");
    }
}

fn call_pipe_needs_repair(is_healthy: bool, already_reconnecting: bool) -> bool {
    !is_healthy && !already_reconnecting
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoordinatorSubscription {
    SingleShard,
    WorldShard,
    RealmCore,
}

impl CoordinatorSubscription {
    fn for_shard(map: &ShardMap, shard: &str) -> Self {
        if map.databases().len() <= 1 {
            Self::SingleShard
        } else if map.realm_core_db() == Some(shard)
            && !map.shards().iter().any(|world| world == shard)
        {
            Self::RealmCore
        } else {
            Self::WorldShard
        }
    }
}

const WORLD_CATALOG_QUERIES: &[&str] = &[
    "SELECT * FROM game_creature_template",
    "SELECT * FROM game_creature_spawn_map",
    "SELECT * FROM game_area",
    "SELECT * FROM game_taxi_node",
    "SELECT * FROM game_taxi_path",
    "SELECT * FROM game_taxi_path_node",
    "SELECT * FROM game_start_position",
    "SELECT * FROM game_spell",
    "SELECT * FROM game_spell_effect",
    "SELECT * FROM game_npc_vendor",
    "SELECT * FROM game_gossip_menu",
    "SELECT * FROM game_gossip_condition",
    "SELECT * FROM game_gossip_option",
    "SELECT * FROM game_gossip_menu_profile",
    "SELECT * FROM game_gossip_menu_profile_option",
    "SELECT * FROM game_creature_gossip_menu_override",
    "SELECT * FROM game_npc_text",
    "SELECT * FROM game_npc_text_slot",
    "SELECT * FROM game_quest_template",
    "SELECT * FROM game_quest_text",
    "SELECT * FROM game_quest_objective",
    "SELECT * FROM game_quest_reward_item",
    "SELECT * FROM game_quest_reward_choice",
    "SELECT * FROM game_creature_quest",
    "SELECT * FROM game_gameobject_quest",
    "SELECT * FROM game_trainer_spell",
    "SELECT * FROM game_spell_chain",
    "SELECT * FROM game_gameobject_template",
    "SELECT * FROM game_talent",
    "SELECT * FROM game_faction",
];

fn coordinator_queries(subscription: CoordinatorSubscription) -> Vec<&'static str> {
    let mut queries = crate::stdb::relays::queries();
    queries.extend([
        "SELECT * FROM game_realm",
        "SELECT * FROM game_account",
        "SELECT * FROM game_session",
        "SELECT * FROM game_account_fence",
        "SELECT * FROM game_account_character_owner",
        "SELECT * FROM game_creature_quest_tap",
        "SELECT * FROM game_creature_quest_tap_member",
        "SELECT * FROM game_creature_loot_tag_group",
        "SELECT * FROM game_corpse_loot_eligible",
        "SELECT * FROM game_auction",
        "SELECT * FROM game_auction_bid_decision",
        "SELECT * FROM game_auction_bid_hold",
        "SELECT * FROM game_auction_hold",
        "SELECT * FROM game_auction_house",
        "SELECT * FROM game_auction_operation_receipt",
        "SELECT * FROM game_guild",
        "SELECT * FROM game_guild_rank",
        "SELECT * FROM game_guild_fee_hold",
        "SELECT * FROM game_guild_fee_decision",
        "SELECT * FROM game_guild_petition_signature",
        "SELECT * FROM game_transfer_out",
        "SELECT * FROM game_transfer_in",
        "SELECT * FROM game_instance",
        "SELECT * FROM game_bot_invite_intent",
        "SELECT * FROM game_party_command_intent WHERE pending = true",
        "SELECT * FROM game_party_command_dispatch_lane",
        "SELECT * FROM game_party_command_receipt",
        "SELECT * FROM game_bot_transfer_intent",
        "SELECT * FROM game_creature_cast",
        "SELECT * FROM game_character_away",
        "SELECT * FROM game_chat_channel",
        "SELECT * FROM game_chat_channel_member",
        "SELECT * FROM game_chat_channel_ban",
        "SELECT * FROM game_meeting_stone",
        "SELECT * FROM game_meeting_stone_seeker",
        "SELECT * FROM game_config",
        "SELECT * FROM game_taxi_service_reply",
        "SELECT * FROM game_item_template",
        "SELECT * FROM game_item_random_property",
        "SELECT * FROM game_item_enchantment",
        "SELECT * FROM game_character_buyback",
        "SELECT * FROM game_corpse_loot",
        "SELECT * FROM game_player_spell",
        "SELECT * FROM game_player_action",
        "SELECT * FROM game_character_talent",
        "SELECT * FROM game_mail",
        "SELECT * FROM game_mail_escrow",
        "SELECT * FROM game_item_text",
        "SELECT * FROM game_faction_template",
        "SELECT * FROM game_guid_allocator",
        "SELECT * FROM game_guid_range",
    ]);
    if subscription != CoordinatorSubscription::RealmCore {
        queries.extend_from_slice(WORLD_CATALOG_QUERIES);
    }
    if subscription != CoordinatorSubscription::SingleShard {
        queries.extend([
            "SELECT * FROM game_character_shard",
            "SELECT * FROM game_group",
            "SELECT * FROM game_group_roster_revision",
            "SELECT * FROM game_group_member_partition",
            "SELECT * FROM game_loot_roll",
            "SELECT * FROM game_loot_roll_vote",
            "SELECT * FROM game_guid_range_registry",
        ]);
    }
    queries
}

/// Build SDK connections on an OS thread outside Tokio to avoid blocking-pool starvation.
fn connect_blocking(
    uri: String,
    db_name: String,
    token: Option<String>,
    subscription: CoordinatorSubscription,
    park_for_relays: bool,
) -> Result<LiveConn> {
    let queries = coordinator_queries(subscription);
    let role_label = coordinator_role_label(&db_name);
    connect_subscribed(
        uri,
        db_name,
        token,
        queries,
        role_label,
        Some("node down, or token lacks access to the private game_account/game_session tables?"),
        park_for_relays,
    )
}

const COORDINATOR_WATCHDOG_POLL: Duration = Duration::from_secs(3);

fn spawn_coordinator_watchdog(inner: Arc<CoordinatorInner>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
.name("stdb-coordinator-watchdog".into())
.spawn(move || {
            let mut previous = inner.motion_batch.delivery.snapshot();
            let mut sampled_at = std::time::Instant::now();
            loop {
                std::thread::sleep(COORDINATOR_WATCHDOG_POLL);
                if sampled_at.elapsed() >= Duration::from_secs(10) {
                    let current = inner.motion_batch.delivery.snapshot();
                    let delta = current.since(previous);
                    if delta.queued > 0 || delta.callbacks > 0 {
                        let live = inner.coord();
                        let line = format!(
                            "MOTIONDELIVERY shard={} elapsed_secs={:.1} queued={} callbacks={} transport_active={} subscription_active={} pump_running={}",
                            inner.db_name,
                            sampled_at.elapsed().as_secs_f64(),
                            delta.queued,
                            delta.callbacks,
                            live.conn.is_active(),
                            live._sub.is_active(),
                            live._pump.as_ref().is_some_and(|pump| !pump.is_finished()),
                        );
                        if delta.is_silent() {
                            log::warn!("{line}: no incoming motion rows. Queue acceptance does not prove Module publishing; inspect motion publishing and Coordinator delivery before choosing recovery.");
                        } else {
                            log::info!("{line}");
                        }
                    }
                    previous = current;
                    sampled_at = std::time::Instant::now();
                }
                repair_dead_call_pipes(&inner);
                {
                    let live = inner.coord();
                    if live.is_healthy() {
                        continue;
                    }
                } // Drop the read guard before the blocking rebuild.
                let role_label = coordinator_role_label(&inner.db_name);
                log::warn!("{role_label} connection/subscription down; rebuilding (gateway self-heal)");
                match connect_blocking(
                    inner.uri.clone(),
                    inner.db_name.clone(),
                    inner.token.clone(),
                    inner.subscription,
                    true,
                ) {
                    Ok(fresh) => {
                        // Publish before re-arming; hooks read fresh cache without the slot lock.
                        replace_live_conn(&inner.live, fresh, &role_label, || {
                            let hooks = inner.on_reconnect.lock().unwrap().clone();
                            for hook in hooks {
                                hook();
                            }
                        });
                    }
                    Err(error) => {
                        log::error!("{role_label} reconnect failed (will retry): {error:#}")
                    }
                }
            }
        })
.expect("spawn coordinator watchdog")
}

const LOOT_ROLL_RELAY_POLL: Duration = Duration::from_millis(200);

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

fn spawn_load_sampler(
    coordinator: Coordinator,
    stdb_uri: String,
    gateway_id: String,
) -> std::thread::JoinHandle<()> {
    let sampler = crate::load_sample::OccupancySampler::from_env(&stdb_uri);
    let interval = crate::load_sample::sample_interval();
    let this_gateway_key = crate::load_sample::gateway_key(&gateway_id);
    std::thread::Builder::new()
        .name("stdb-load-sampler".into())
        .spawn(move || loop {
            std::thread::sleep(interval);
            let occupancy = sampler.sample_all();
            for line in
                crate::load_sample::sample_and_record(&coordinator, &occupancy, this_gateway_key)
            {
                log::info!("{line}");
            }
        })
        .expect("spawn load sampler")
}

fn ensure_guid_ranges(conns: &HashMap<String, Arc<CoordinatorInner>>, map: &ShardMap) {
    fn claim(rc: &Arc<CoordinatorInner>, shard: &str, mark: u64) -> Result<Option<u64>> {
        call_reducer!(
            rc.coord().conn.reducers,
            "claim_guid_range",
            claim_guid_range_then(shard.to_string(), mark)
        )?;
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
        };
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

fn ensure_mail_escrow_range(
    conns: &HashMap<String, Arc<CoordinatorInner>>,
    map: &ShardMap,
    gateway_id: &str,
) {
    let Some(rc_name) = map.realm_core_db() else {
        // Single-shard Escrow ids belong to the Module's GUID Range; a Gateway range would overlap.
        return;
    };
    let Some(rc) = conns.get(rc_name) else { return };
    let claimant = match mail_escrow_claimant(gateway_id) {
        Ok(claimant) => claimant,
        Err(e) => {
            log::error!("could not create an exclusive mail escrow claimant: {e:#}");
            return;
        }
    };
    let desired = lyracore_shared::mail::escrow_range_mark(&claimant);
    if let Err(e) = (|| -> Result<()> {
        call_reducer!(
            rc.coord().conn.reducers,
            "claim_guid_range for mail escrow",
            claim_guid_range_then(claimant.clone(), desired)
        )?;
        let row = (0..100)
            .find_map(|_| {
                let found = rc
                    .coord()
                    .conn
                    .db
                    .game_guid_range_registry()
                    .shard_name()
                    .find(&claimant);
                if found.is_none() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                found
            })
            .ok_or_else(|| anyhow!("mail escrow range claim published no registry row"))?;
        let seen = conns.values().flat_map(|inner| {
            let db = &inner.coord().conn.db;
            db.game_mail_escrow()
                .iter()
                .map(|r| r.escrow_id)
                .chain(db.game_mail_delivery().iter().map(|r| r.escrow_id))
                .collect::<Vec<_>>()
        });
        let (next, end) = lyracore_shared::mail::resume_escrow_range(row.base, row.size, seen)
            .ok_or_else(|| anyhow!("mail escrow id range is exhausted"))?;
        crate::world::mail::install_escrow_id_range(next, end)?;
        log::info!(
            "gateway {gateway_id} mints mail escrow ids from claimed range [{}, {end})",
            row.base
        );
        Ok(())
    })() {
        log::error!(
            "could not claim mail escrow id range: {e:#}; sharded mail value moves will REFUSE"
        );
    }
}

fn mail_escrow_claimant(gateway_id: &str) -> Result<String> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| anyhow!("OS randomness unavailable: {e}"))?;
    let nonce = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("mail-escrow:{gateway_id}:{nonce}"))
}

#[cfg(test)]
mod mail_escrow_claimant_tests {
    use super::mail_escrow_claimant;

    #[test]
    fn live_gateway_processes_claim_distinct_ranges() {
        let first = mail_escrow_claimant("gateway-a").unwrap();
        let second = mail_escrow_claimant("gateway-a").unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with("mail-escrow:gateway-a:"));
    }
}

pub(crate) fn recv_reducer(
    rx: std::sync::mpsc::Receiver<std::result::Result<(), ReducerCompletionFailure>>,
    what: &str,
) -> Result<()> {
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(ReducerCompletionFailure::Rejected(reason))) => {
            Err(ReducerCallError::rejected(what, reason).into())
        }
        Ok(Err(ReducerCompletionFailure::Internal(source))) => {
            Err(ReducerCallError::internal(what, source).into())
        }
        Ok(Err(ReducerCompletionFailure::TransportDisconnected)) => Err(ReducerCallError::fatal(
            format!("{what} reducer failed: transport disconnected"),
        )
        .into()),
        Err(_) => {
            Err(ReducerCallError::fatal(format!("{what} reducer timed out after 10s")).into())
        }
    }
}

pub(crate) fn recv_reducer_on(
    rx: std::sync::mpsc::Receiver<std::result::Result<(), ReducerCompletionFailure>>,
    what: &str,
    completion: &ReducerCompletion,
    call_id: u64,
) -> Result<()> {
    let result = recv_reducer(rx, what);
    completion.cancel(call_id);
    result
}

#[cfg(test)]
mod coordinator_query_tests {
    use super::{
        call_pipe_queries, coordinator_queries, CoordinatorSubscription, ShardMap,
        CALL_PIPE_LIVENESS_QUERY, WORLD_CATALOG_QUERIES,
    };

    #[test]
    fn coordinator_subscribes_source_map_coverage_in_each_topology() {
        for subscription in [
            CoordinatorSubscription::SingleShard,
            CoordinatorSubscription::WorldShard,
        ] {
            assert!(coordinator_queries(subscription)
                .contains(&"SELECT * FROM game_creature_spawn_map"));
        }
    }

    #[test]
    fn coordinator_subscribes_bot_intents_in_each_topology() {
        for subscription in [
            CoordinatorSubscription::SingleShard,
            CoordinatorSubscription::WorldShard,
        ] {
            let queries = coordinator_queries(subscription);
            for query in [
                "SELECT * FROM game_bot_invite_intent",
                "SELECT * FROM game_bot_transfer_intent",
            ] {
                assert!(
                    queries.contains(&query),
                    "missing {query}, subscription={subscription:?}"
                );
            }
        }
    }

    #[test]
    fn realm_core_skips_world_catalogs_and_keeps_every_control_and_relay_query() {
        let world = coordinator_queries(CoordinatorSubscription::WorldShard);
        let realm = coordinator_queries(CoordinatorSubscription::RealmCore);
        let removed: Vec<_> = world
            .iter()
            .filter(|query| !realm.contains(query))
            .copied()
            .collect();
        assert_eq!(removed, WORLD_CATALOG_QUERIES);
        for query in crate::stdb::relays::queries().into_iter().chain([
            "SELECT * FROM game_bot_invite_intent",
            "SELECT * FROM game_bot_transfer_intent",
            "SELECT * FROM game_party_command_intent WHERE pending = true",
            "SELECT * FROM game_party_command_dispatch_lane",
            "SELECT * FROM game_transfer_out",
            "SELECT * FROM game_transfer_in",
            "SELECT * FROM game_character_shard",
            "SELECT * FROM game_account",
            "SELECT * FROM game_account_fence",
            "SELECT * FROM game_account_character_owner",
            "SELECT * FROM game_item_template",
            "SELECT * FROM game_item_random_property",
            "SELECT * FROM game_item_enchantment",
            "SELECT * FROM game_faction_template",
        ]) {
            assert!(realm.contains(&query), "missing Realm-core query {query}");
        }
    }

    #[test]
    fn a_realm_core_named_as_a_world_owner_keeps_world_catalogs() {
        let map = ShardMap::parse("world", "0:*=realm").with_realm_core(Some("realm"));
        let role = CoordinatorSubscription::for_shard(&map, "realm");
        assert_eq!(role, CoordinatorSubscription::WorldShard);
        assert!(coordinator_queries(role).contains(&"SELECT * FROM game_spell"));
    }

    #[test]
    fn subscription_roles_keep_all_world_shards_ready_without_viewers() {
        let map = ShardMap::parse("world", "0:*=world;1:*=idle").with_realm_core(Some("realm"));
        assert_eq!(map.shards(), ["world", "idle"]);
        for name in map.shards() {
            let role = CoordinatorSubscription::for_shard(&map, &name);
            assert_eq!(role, CoordinatorSubscription::WorldShard);
            let queries = coordinator_queries(role);
            assert!(queries.contains(&"SELECT * FROM game_spell"));
            assert!(queries.contains(&"SELECT * FROM game_bot_invite_intent"));
            assert!(queries.contains(&"SELECT * FROM game_bot_transfer_intent"));
        }
        assert_eq!(
            CoordinatorSubscription::for_shard(&map, "realm"),
            CoordinatorSubscription::RealmCore
        );
        let single = ShardMap::parse("world", "");
        assert_eq!(
            CoordinatorSubscription::for_shard(&single, "world"),
            CoordinatorSubscription::SingleShard
        );
    }

    #[test]
    fn coordinator_reconnect_restores_the_saved_subscription_scope() {
        fn restores_scope(source: &str) -> bool {
            use syn::visit::Visit;
            struct SavedScope(bool);
            impl<'ast> Visit<'ast> for SavedScope {
                fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
                    if let syn::Expr::Path(path) = call.func.as_ref() {
                        if path.path.is_ident("connect_blocking") {
                            self.0 = matches!(call.args.iter().nth(3), Some(syn::Expr::Field(field))
                                if matches!(&field.member, syn::Member::Named(member) if member == "subscription")
                                && matches!(field.base.as_ref(), syn::Expr::Path(base) if base.path.is_ident("inner")));
                        }
                    }
                    syn::visit::visit_expr_call(self, call);
                }
            }
            let file = syn::parse_file(source).unwrap();
            let watchdog = file
                .items
                .into_iter()
                .find_map(|item| match item {
                    syn::Item::Fn(function)
                        if function.sig.ident == "spawn_coordinator_watchdog" =>
                    {
                        Some(function)
                    }
                    _ => None,
                })
                .unwrap();
            let mut scope = SavedScope(false);
            scope.visit_block(&watchdog.block);
            scope.0
        }
        let source = include_str!("connection.rs");
        assert!(restores_scope(source));
        assert!(!restores_scope(&source.replace(
            "inner.subscription,",
            "CoordinatorSubscription::WorldShard,",
        )));
    }

    const MULTI_DB_TABLES: &[&str] = &[
        "SELECT * FROM game_character_shard",
        "SELECT * FROM game_group",
        "SELECT * FROM game_group_roster_revision",
        "SELECT * FROM game_group_member_partition",
        "SELECT * FROM game_loot_roll",
        "SELECT * FROM game_loot_roll_vote",
        "SELECT * FROM game_guid_range_registry",
    ];

    #[test]
    fn a_single_database_gateway_subscribes_no_multi_database_table() {
        let single = coordinator_queries(CoordinatorSubscription::SingleShard);
        for table in MULTI_DB_TABLES {
            assert!(
                !single.contains(table),
                "{table} is subscribed on a single-database gateway — a module published before \
                 that table exists will refuse the subscription and the gateway will not start"
            );
        }
    }

    #[test]
    fn loot_tag_projection_rows_are_in_every_base_subscription() {
        let queries = coordinator_queries(CoordinatorSubscription::SingleShard);
        for table in [
            "SELECT * FROM game_creature_quest_tap",
            "SELECT * FROM game_creature_quest_tap_member",
            "SELECT * FROM game_creature_loot_tag_group",
            "SELECT * FROM game_corpse_loot_eligible",
            "SELECT * FROM game_group_member",
        ] {
            assert!(
                queries.contains(&table),
                "missing Loot Tag projection row: {table}"
            );
        }
    }

    #[test]
    fn the_multi_database_flag_adds_exactly_those_tables_and_changes_nothing_else() {
        let single = coordinator_queries(CoordinatorSubscription::SingleShard);
        let sharded = coordinator_queries(CoordinatorSubscription::WorldShard);
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
        let mut sorted = sharded.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), before, "duplicate subscription query");
    }

    #[test]
    fn coordinator_and_call_pipes_keep_separate_query_policies() {
        let pipe_queries = call_pipe_queries();
        assert_eq!(
            pipe_queries,
            vec![CALL_PIPE_LIVENESS_QUERY],
            "a reducer-only pipe must carry only its game_config liveness signal"
        );

        let coordinator_queries = coordinator_queries(CoordinatorSubscription::SingleShard);
        assert!(coordinator_queries.len() > pipe_queries.len());
        assert!(coordinator_queries.contains(&CALL_PIPE_LIVENESS_QUERY));
    }
}

#[cfg(test)]
mod live_replacement_tests {
    use super::{pump_messages, replace_slot_ordered};
    use crate::stdb::world_index::{CellKey, EntityLayer, WorldIndex};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex, RwLock};
    use std::thread::JoinHandle;
    use std::time::Duration;

    #[test]
    fn replacement_installs_before_post_install_and_tears_down_unlocked() {
        let slot = RwLock::new("old");
        let post_install_calls = AtomicUsize::new(0);
        let events = Mutex::new(Vec::new());

        replace_slot_ordered(
            &slot,
            "fresh",
            "test database coordinator",
            |_| {},
            || {
                let live = slot
                    .try_read()
                    .expect("post-install work must run without the live-slot write lock");
                assert_eq!(*live, "fresh", "post-install work must observe fresh");
                post_install_calls.fetch_add(1, Ordering::Relaxed);
                events.lock().unwrap().push("post-install");
            },
            |old| {
                assert_eq!(old, "old");
                assert!(
                    slot.try_write().is_ok(),
                    "teardown must run without the live-slot write lock"
                );
                events.lock().unwrap().push("teardown");
            },
        );

        assert_eq!(*slot.read().unwrap(), "fresh");
        assert_eq!(post_install_calls.load(Ordering::Relaxed), 1);
        assert_eq!(*events.lock().unwrap(), ["post-install", "teardown"]);
    }

    #[test]
    fn a_replacement_cache_cannot_advance_transfer_past_an_old_row_callback() {
        struct ExternalConnection {
            escrow_visible: bool,
            pump: Option<JoinHandle<()>>,
        }

        let source = 0;
        let destination = 1;
        let guid = 7;
        let source_cell = CellKey::at(0, 0, 0, 0);
        let destination_cell = CellKey::at(1, 0, 0, 0);
        for inserting in [true, false] {
            let index = Arc::new(WorldIndex::new(true));
            if !inserting {
                index.upsert_entity(EntityLayer::WorldEntity, guid, source_cell, source, 0);
            }
            let slot = Arc::new(RwLock::new(ExternalConnection {
                escrow_visible: false,
                pump: None,
            }));
            let (entered, callback_entered) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let callback_slot = slot.clone();
            let callback_index = index.clone();
            let pump = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                let (_setup, work) = tokio::sync::mpsc::unbounded_channel();
                let result = runtime.block_on(pump_messages(&(), work, || async {
                    entered.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert!(
                        !callback_slot.read().unwrap().escrow_visible,
                        "an old callback must retain the old cache until it finishes"
                    );
                    callback_index.upsert_entity(
                        EntityLayer::WorldEntity,
                        guid,
                        source_cell,
                        source,
                        0,
                    );
                    Err::<(), ()>(())
                }));
                assert_eq!(result, Err(()));
            });
            slot.write().unwrap().pump = Some(pump);
            callback_entered
                .recv_timeout(Duration::from_secs(5))
                .unwrap();

            let (retiring, retirement_started) = mpsc::channel();
            let replacement_slot = slot.clone();
            let replacement_index = index.clone();
            let replacement = std::thread::spawn(move || {
                replace_slot_ordered(
                    &replacement_slot,
                    ExternalConnection {
                        escrow_visible: true,
                        pump: None,
                    },
                    "source Shard",
                    |slot| {
                        let pump = slot.write().unwrap().pump.take().unwrap();
                        retiring.send(()).unwrap();
                        pump.join().unwrap();
                    },
                    || {
                        assert!(replacement_slot.read().unwrap().escrow_visible);
                        replacement_index.upsert_entity(
                            EntityLayer::WorldEntity,
                            guid,
                            destination_cell,
                            destination,
                            0,
                        );
                    },
                    drop,
                );
            });
            retirement_started
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            let published_before_callback_finished = slot.read().unwrap().escrow_visible;
            release.send(()).unwrap();
            replacement.join().unwrap();

            assert!(!published_before_callback_finished);
            assert_eq!(
                index.shard_of(EntityLayer::WorldEntity, guid),
                Some(destination)
            );
            assert_eq!(
                index.entity_cell(EntityLayer::WorldEntity, guid),
                Some(destination_cell)
            );
        }
    }
}

#[cfg(test)]
mod pump_tests {
    use super::{finish_relay_setup, pump_messages, replace_slot_ordered, PumpCommand};
    use std::cell::RefCell;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct ExternalCache {
        row: Mutex<Option<u64>>,
        projected: Mutex<Option<u64>>,
        registered: AtomicBool,
        pending_registration: AtomicBool,
    }

    #[tokio::test]
    async fn a_quiet_pump_runs_setup_without_waiting_for_an_incoming_message() {
        let (tx, rx) = mpsc::unbounded_channel::<PumpCommand<ExternalCache>>();
        let cache = ExternalCache::default();
        let (done, finished) = tokio::sync::oneshot::channel();
        tx.send(PumpCommand::Run(Box::new(move |_| {
            let _ = done.send(());
        })))
        .unwrap();
        let pumping = pump_messages(&cache, rx, std::future::pending::<Result<(), ()>>);
        tokio::pin!(pumping);
        tokio::select! {
            _ = &mut pumping => panic!("the external connection is still open"),
            outcome = tokio::time::timeout(Duration::from_secs(1), finished) => {
                outcome.expect("setup must wake a quiet pump").unwrap();
            }
        }
    }

    #[tokio::test]
    async fn a_delta_arriving_during_snapshot_setup_is_applied_after_the_snapshot() {
        for (snapshot, delta) in [(None, Some(2)), (Some(1), Some(2)), (Some(1), None)] {
            let cache = ExternalCache::default();
            *cache.row.lock().unwrap() = snapshot;
            let (setup, work) = mpsc::unbounded_channel::<PumpCommand<ExternalCache>>();
            let (incoming, messages) = mpsc::unbounded_channel::<Result<Option<u64>, ()>>();
            let messages = tokio::sync::Mutex::new(messages);
            setup
                .send(PumpCommand::Run(Box::new(move |cache| {
                    cache.pending_registration.store(true, Ordering::Relaxed);
                    let snapshot = *cache.row.lock().unwrap();
                    incoming.send(Ok(delta)).unwrap();
                    incoming.send(Err(())).unwrap();
                    *cache.projected.lock().unwrap() = snapshot;
                })))
                .unwrap();

            let result = pump_messages(&cache, work, || async {
                if cache.pending_registration.swap(false, Ordering::Relaxed) {
                    cache.registered.store(true, Ordering::Relaxed);
                    return Ok(());
                }
                let Some(Ok(row)) = messages.lock().await.recv().await else {
                    return Err(());
                };
                *cache.row.lock().unwrap() = row;
                if cache.registered.load(Ordering::Relaxed) {
                    *cache.projected.lock().unwrap() = row;
                }
                Ok(())
            })
            .await;

            assert_eq!(result, Err(()));
            assert_eq!(*cache.projected.lock().unwrap(), delta);
        }
    }

    #[tokio::test]
    async fn fresh_deltas_during_retirement_wait_for_registered_relays() {
        let cache = ExternalCache::default();
        let relayed = AtomicUsize::new(0);
        let (commands, work) = mpsc::unbounded_channel();
        let (incoming, messages) = mpsc::unbounded_channel::<Option<u64>>();
        let messages = tokio::sync::Mutex::new(messages);
        let (applied, parked) = std::sync::mpsc::channel();
        let applied = Mutex::new(Some(applied));
        incoming.send(None).unwrap();
        let pump = RefCell::new(Box::pin(pump_messages(&cache, work, || async {
            if cache.pending_registration.swap(false, Ordering::Relaxed) {
                cache.registered.store(true, Ordering::Relaxed);
                return Ok::<(), ()>(());
            }
            match messages.lock().await.recv().await.unwrap() {
                None => {
                    *cache.row.lock().unwrap() = Some(1);
                    commands
                        .send(PumpCommand::Park(applied.lock().unwrap().take().unwrap()))
                        .unwrap();
                }
                Some(row) => {
                    *cache.row.lock().unwrap() = Some(row);
                    if cache.registered.load(Ordering::Relaxed) {
                        *cache.projected.lock().unwrap() = Some(row);
                        relayed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Ok(())
        })));
        let poll = || {
            pump.borrow_mut()
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
        };
        assert_eq!(poll(), Poll::Pending);
        assert_eq!(parked.try_recv().unwrap(), Ok(()));

        let slot = std::sync::RwLock::new("old");
        replace_slot_ordered(
            &slot,
            "fresh",
            "source Shard",
            |_| {
                incoming.send(Some(2)).unwrap();
                assert_eq!(poll(), Poll::Pending);
                assert_eq!(*cache.row.lock().unwrap(), Some(1));
                assert_eq!(relayed.load(Ordering::Relaxed), 0);
            },
            || {
                assert_eq!(*slot.read().unwrap(), "fresh");
                commands
                    .send(PumpCommand::Run(Box::new(|cache| {
                        cache.pending_registration.store(true, Ordering::Relaxed);
                    })))
                    .unwrap();
                assert_eq!(poll(), Poll::Pending);
                assert_eq!(*cache.row.lock().unwrap(), Some(1));
                assert_eq!(relayed.load(Ordering::Relaxed), 0);
                commands.send(PumpCommand::Resume).unwrap();
                assert_eq!(poll(), Poll::Pending);
            },
            drop,
        );

        assert_eq!(*cache.row.lock().unwrap(), Some(2));
        assert_eq!(*cache.projected.lock().unwrap(), Some(2));
        assert_eq!(relayed.load(Ordering::Relaxed), 1);
        commands.send(PumpCommand::Stop).unwrap();
        assert_eq!(poll(), Poll::Ready(Ok(())));
    }

    #[tokio::test]
    async fn a_quiet_parked_pump_resumes_without_an_incoming_message() {
        let (commands, work) = mpsc::unbounded_channel();
        let (applied, parked) = std::sync::mpsc::channel();
        commands.send(PumpCommand::Park(applied)).unwrap();
        let advances = AtomicUsize::new(0);
        let mut pump = Box::pin(pump_messages(&(), work, || async {
            advances.fetch_add(1, Ordering::Relaxed);
            std::future::pending::<Result<(), ()>>().await
        }));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(pump.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(parked.try_recv().unwrap(), Ok(()));
        assert_eq!(advances.load(Ordering::Relaxed), 0);
        finish_relay_setup(&commands, || {});
        assert_eq!(pump.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(advances.load(Ordering::Relaxed), 1);
        commands.send(PumpCommand::Stop).unwrap();
        assert_eq!(pump.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }

    #[tokio::test]
    async fn the_after_message_hook_runs_once_after_each_applied_message() {
        let (commands, work) = mpsc::unbounded_channel();
        let applied = std::sync::Arc::new(AtomicUsize::new(0));
        let flushed = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = applied.clone();
        let hook_flushed = flushed.clone();
        commands
            .send(PumpCommand::AfterMessage(Box::new(move || {
                assert_eq!(
                    seen.load(Ordering::Relaxed),
                    hook_flushed.load(Ordering::Relaxed) + 1
                );
                hook_flushed.fetch_add(1, Ordering::Relaxed);
            })))
            .unwrap();
        let messages = AtomicUsize::new(0);
        let result = pump_messages(&(), work, || async {
            if messages.fetch_add(1, Ordering::Relaxed) == 3 {
                return Err(());
            }
            applied.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
        .await;
        assert_eq!(result, Err(()));
        assert_eq!(flushed.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn a_panicking_reconnect_hook_stops_its_partially_armed_pump() {
        let (commands, work) = mpsc::unbounded_channel();
        let (applied, _parked) = std::sync::mpsc::channel();
        commands.send(PumpCommand::Park(applied)).unwrap();
        let mut pump = Box::pin(pump_messages(
            &(),
            work,
            std::future::pending::<Result<(), ()>>,
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(pump.as_mut().poll(&mut cx), Poll::Pending);
        let outcome = std::panic::catch_unwind(|| {
            finish_relay_setup(&commands, || panic!("hook failed"));
        });
        assert!(outcome.is_err(), "hook failures still propagate");
        assert_eq!(pump.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }

    #[tokio::test]
    async fn failed_subscription_cleanup_stops_before_or_after_a_late_apply() {
        for stop_first in [true, false] {
            let (commands, work) = mpsc::unbounded_channel();
            let (applied, _parked) = std::sync::mpsc::channel();
            if stop_first {
                commands.send(PumpCommand::Stop).unwrap();
                commands.send(PumpCommand::Park(applied)).unwrap();
            } else {
                commands.send(PumpCommand::Park(applied)).unwrap();
                commands.send(PumpCommand::Stop).unwrap();
            }
            assert_eq!(
                pump_messages(&(), work, std::future::pending::<Result<(), ()>>).await,
                Ok(())
            );
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::{classify, recv_reducer, DurableFailure, ReducerCallError};

    #[test]
    fn a_rejected_reducer_is_a_refusal_with_the_module_reason() {
        let error =
            anyhow::Error::from(ReducerCallError::refused("gw_buy_item", "vendor:no_money"))
                .context("buy phase");
        assert_eq!(
            classify(&error),
            DurableFailure::Refusal {
                reason: "vendor:no_money"
            }
        );
    }

    #[test]
    fn an_sdk_internal_error_is_transport_loss() {
        let error = anyhow::Error::from(ReducerCallError::internal(
            "gw_buy_item",
            spacetimedb_sdk::error::InternalError::failed_parse("ReducerResult", "Message"),
        ));
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }

    #[test]
    fn a_failed_sdk_send_is_transport_loss() {
        let error = anyhow::Error::from(ReducerCallError::sdk_send(
            "gw_buy_item",
            spacetimedb_sdk::Error::Disconnected,
        ));
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }

    #[test]
    fn a_dropped_transport_is_transport_loss() {
        let error = anyhow::Error::from(ReducerCallError::transport_lost("gw_buy_item"));
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }

    #[test]
    fn a_completion_that_never_arrives_is_transport_loss() {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        let error = recv_reducer(rx, "gw_buy_item").expect_err("no completion is an error");
        assert_eq!(error.to_string(), "gw_buy_item reducer timed out after 10s");
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }

    #[test]
    fn an_untyped_error_is_transport_loss_even_when_it_reads_like_a_refusal() {
        let error = anyhow::anyhow!("gw_buy_item reducer failed: vendor:no_money");
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
    }
}

#[cfg(test)]
mod recv_reducer_tests {
    use super::{
        classify, recv_reducer, reducer_refusal_reason, DurableFailure, ReducerCompletion,
        ReducerCompletionFailure,
    };
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn a_completed_reducer_maps_to_ok() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(())).unwrap();
        assert!(recv_reducer(rx, "frobnicate").is_ok());
    }

    #[test]
    fn a_failed_reducer_is_prefixed_with_the_call_label() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err(ReducerCompletionFailure::Rejected(
            "not enough copper".to_string(),
        )))
        .unwrap();
        let err = recv_reducer(rx, "buy_item").expect_err("a reducer Err must map to Err");
        assert_eq!(
            err.to_string(),
            "buy_item reducer failed: not enough copper"
        );
        assert_eq!(reducer_refusal_reason(&err), Some("not enough copper"));
    }

    #[test]
    fn an_sdk_internal_error_is_not_a_gameplay_refusal() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err(ReducerCompletionFailure::Internal(
            spacetimedb_sdk::error::InternalError::failed_parse("ReducerResult", "Message"),
        )))
        .unwrap();

        let err = recv_reducer(rx, "gw_loot_money")
            .expect_err("an SDK InternalError must remain session-fatal");

        assert_eq!(classify(&err), DurableFailure::TransportLoss);
        assert!(err.to_string().starts_with("gw_loot_money reducer failed:"));
    }

    #[test]
    fn a_connection_death_completes_only_its_pending_reducer_waits() {
        let dead = ReducerCompletion::connected();
        let live = ReducerCompletion::connected();
        let (dead_tx, dead_rx) = mpsc::channel();
        let (live_tx, live_rx) = mpsc::channel();
        dead.register(dead_tx).unwrap();
        live.register(live_tx).unwrap();

        dead.disconnect();

        let error = recv_reducer(dead_rx, "gw_loot_money")
            .expect_err("a disconnected reducer completion must be fatal");
        assert_eq!(classify(&error), DurableFailure::TransportLoss);
        assert_eq!(
            error.to_string(),
            "gw_loot_money reducer failed: transport disconnected"
        );
        assert!(live_rx.recv_timeout(Duration::from_millis(20)).is_err());
    }

    #[test]
    fn reducer_completion_wins_or_loses_cleanly_against_disconnect() {
        let completion = Arc::new(ReducerCompletion::connected());
        let (tx, rx) = mpsc::channel();
        let id = completion.register(tx).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let done = completion.clone();
        let done_barrier = barrier.clone();
        let done_thread = std::thread::spawn(move || {
            done_barrier.wait();
            done.finish(id, Ok(()));
        });
        let dropped = completion.clone();
        let dropped_barrier = barrier.clone();
        let disconnect_thread = std::thread::spawn(move || {
            dropped_barrier.wait();
            dropped.disconnect();
        });
        barrier.wait();
        done_thread.join().unwrap();
        disconnect_thread.join().unwrap();

        match rx.recv_timeout(Duration::from_millis(100)).unwrap() {
            Ok(()) => {}
            Err(failure) => assert!(matches!(
                failure,
                ReducerCompletionFailure::TransportDisconnected
            )),
        }
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
    }
}

#[cfg(test)]
mod call_pipe_routing_tests {
    use super::{call_pipe_needs_repair, select_active_call_pipe};

    #[test]
    fn a_dead_call_pipe_is_skipped_for_the_next_live_pipe() {
        assert_eq!(
            select_active_call_pipe(3, 0, |index| index == 1),
            Some(1),
            "a reducer must not be sent through the dead pipe selected by round-robin"
        );
    }

    #[test]
    fn a_repaired_call_pipe_rejoins_reducer_rotation() {
        assert_eq!(
            select_active_call_pipe(3, 0, |index| index == 0),
            Some(0),
            "once a pipe is live again, subsequent reducer traffic should use it"
        );
    }

    #[test]
    fn every_dead_call_pipe_routes_reducers_to_the_coordinator() {
        assert_eq!(
            select_active_call_pipe(3, 0, |_| false),
            None,
            "the watchdogged coordinator is the safe fallback while pipes reconnect"
        );
    }

    #[test]
    fn only_one_repair_attempt_is_started_for_a_dead_call_pipe() {
        assert!(call_pipe_needs_repair(false, false));
        assert!(
            !call_pipe_needs_repair(false, true),
            "a down node must not accumulate watchdog reconnect threads for the same pipe"
        );
        assert!(
            !call_pipe_needs_repair(true, false),
            "healthy pipes require no repair"
        );
    }
}

macro_rules! call_reducer {
    ($live:ident . conn . reducers, $what:literal, $method:ident ( $($arg:expr),* $(,)? )) => {{
        call_reducer!(@call $live, $what, $method($($arg),*))
    }};
    ($owner:ident . 0 . call_pipe() . conn . reducers, $what:literal, $method:ident ( $($arg:expr),* $(,)? )) => {{
        let live = $owner.0.call_pipe();
        call_reducer!(@call live, $what, $method($($arg),*))
    }};
    ($owner:ident . coord() . conn . reducers, $what:literal, $method:ident ( $($arg:expr),* $(,)? )) => {{
        let live = $owner.coord();
        call_reducer!(@call live, $what, $method($($arg),*))
    }};
    (@call $live:ident, $what:literal, $method:ident ( $($arg:expr),* $(,)? )) => {{
        let (tx, rx) = std::sync::mpsc::channel::<
            std::result::Result<(), $crate::stdb::connection::ReducerCompletionFailure>,
        >();
        let completion = $live.reducer_completion.clone();
        let call_id = completion
.register(tx)
.map_err(|e| {
                $crate::stdb::connection::ReducerCallError::fatal(format!(
                    concat!($what, " reducer transport disconnected: {}"),
                    e
                ))
            })?;
        let callback_completion = completion.clone();
        $live
.conn
.reducers
.$method($($arg,)* move |_ctx, status| {
                callback_completion.finish(call_id, match status {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(reason)) => Err(
                        $crate::stdb::connection::ReducerCompletionFailure::Rejected(reason),
                    ),
                    Err(error) => Err(
                        $crate::stdb::connection::ReducerCompletionFailure::Internal(error),
                    ),
                });
            })
.map_err(|e| {
                completion.cancel(call_id);
                $crate::stdb::connection::ReducerCallError::sdk_send($what, e)
            })?;
        $crate::stdb::connection::recv_reducer_on(rx, $what, &completion, call_id)
    }};
}
pub(crate) use call_reducer;

impl Coordinator {
    #[cfg(test)]
    pub(crate) fn park_pump_for_test(&self) {
        let commands = self.0.coord().pump_commands.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        commands.send(PumpCommand::Park(tx)).unwrap();
        rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
    }

    #[cfg(test)]
    pub(crate) fn resume_pump_for_test(&self) {
        self.0
            .coord()
            .pump_commands
            .send(PumpCommand::Resume)
            .unwrap();
    }
}

impl Coordinator {
    async fn connect_shard(
        cfg: &GatewayConfig,
        db_name: &str,
        subscription: CoordinatorSubscription,
    ) -> Result<Arc<CoordinatorInner>> {
        let uri = cfg.stdb_uri.clone();
        let db = db_name.to_string();
        let token = cfg.coordinator_token.clone();
        let role_label = coordinator_role_label(db_name);
        let join_role_label = role_label.clone();
        let build_thread = std::thread::Builder::new()
            .name("stdb-coordinator-connect".into())
            .spawn(move || connect_blocking(uri, db, token, subscription, false))
            .context(format!("spawn {role_label} connect thread"))?;
        let live = tokio::task::spawn_blocking(move || -> Result<LiveConn> {
            build_thread
                .join()
                .map_err(|_| anyhow!("{join_role_label} connect thread panicked"))?
        })
        .await
        .context("join coordinator connect task")??;
        let want_pipes = crate::config::call_pipes();
        let mut call_pipes = Vec::new();
        if want_pipes > 1 {
            for i in 0..want_pipes {
                let role_label = call_pipe_role_label(db_name, i);
                let (u, d, t) = (
                    cfg.stdb_uri.clone(),
                    db_name.to_string(),
                    cfg.coordinator_token.clone(),
                );
                let built = tokio::task::spawn_blocking(move || {
                    std::thread::Builder::new()
                        .name(format!("stdb-call-pipe-{i}"))
                        .spawn(move || connect_call_pipe(u, d, t, i))
                        .expect("spawn call pipe thread")
                        .join()
                        .map_err(|_| anyhow!("call pipe connect thread panicked"))?
                })
                .await;
                match built {
                    Ok(Ok(pipe)) => call_pipes.push(CallPipe {
                        live: RwLock::new(pipe),
                        reconnecting: AtomicBool::new(false),
                    }),
                    Ok(Err(error)) => {
                        log::warn!("{role_label} failed to build (skipped): {error:#}")
                    }
                    Err(error) => log::warn!("{role_label} join failed (skipped): {error:#}"),
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
            subscription,
            call_pipes,
            call_pipe_next: std::sync::atomic::AtomicUsize::new(0),
            party_command_lane_overflow: AtomicBool::new(false),
            bot_transfer_pending_overflow: AtomicBool::new(false),
            motion_batch: MovementBatch::new(),
            on_reconnect: Mutex::new(Vec::new()),
        });
        spawn_coordinator_watchdog(inner.clone());
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(40));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let mut ticks_until_metrics = 250u16;
                loop {
                    tick.tick().await;
                    let failures = inner.motion_batch.drain(|chunk| {
                        let entries = chunk.len();
                        let pipe = inner.call_pipe();
                        let result = pipe.conn.reducers.gw_movement_batch(chunk);
                        crate::movement_batch_metrics::MOVEMENT_BATCH_METRICS
                            .observe(entries, result.is_ok());
                        result
                    });
                    for failure in failures {
                        let n = failure.dropped;
                        let e = failure.error;
                        log::warn!(
                            "gw_movement_batch send failed ({n} moves dropped this tick): {e}"
                        );
                    }
                    ticks_until_metrics -= 1;
                    if ticks_until_metrics == 0 {
                        log::info!(
                            "{}",
                            crate::movement_batch_metrics::MOVEMENT_BATCH_METRICS
                                .snapshot()
                                .log_line()
                        );
                        ticks_until_metrics = 250;
                    }
                }
            });
        }
        Ok(inner)
    }

    pub async fn connect(cfg: &GatewayConfig) -> Result<Self> {
        if cfg.coordinator_token.is_none() {
            log::warn!(
                "LYRACORE_COORDINATOR_TOKEN is unset; the coordinator will connect anonymously and \
                 cannot read the private game_account/game_session tables"
            );
        }
        let map = ShardMap::from_env(&cfg.module_name);
        let mut conns: HashMap<String, Arc<CoordinatorInner>> = HashMap::new();
        for db in map.databases() {
            match Self::connect_shard(cfg, &db, CoordinatorSubscription::for_shard(&map, &db)).await {
                Ok(inner) => {
                    log::info!("coordinator connected to shard {db}");
                    conns.insert(db, inner);
                }
                Err(e) if db == map.default_db() => return Err(e),
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
        let hosts_instances = |db: &str| -> Option<bool> {
            conns
                .get(db)
                .and_then(|inner| inner.coord().conn.db.game_config().id().find(&0))
                .map(|cfg| cfg.hosts_instances)
        };
        let mut imported_maps: Vec<u32> = conns
            .values()
            .flat_map(|inner| {
                inner
                    .coord()
                    .conn
                    .db
                    .game_creature_spawn_map()
                    .iter()
                    .map(|map| map.map_id)
                    .collect::<Vec<_>>()
            })
            .collect();
        imported_maps.sort_unstable();
        imported_maps.dedup();
        map.check_instance_hosting(&imported_maps, hosts_instances, |d| conns.contains_key(d))
            .enforce()
            .map_err(|msg| anyhow!("{msg}"))?;
        ensure_guid_ranges(&conns, &map);
        ensure_mail_escrow_range(&conns, &map, &cfg.gateway_id);
        let world = Arc::new(super::world_view::WorldView::new(
            crate::config::aoi_enabled(),
        ));
        let mut authority_nonce = [0u8; 16];
        getrandom::fill(&mut authority_nonce)
            .map_err(|error| anyhow!("OS randomness unavailable: {error}"))?;
        let package_party_gateway_id = authority_nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let coordinator = Self(
            home,
            Arc::new(ShardSet {
                map,
                package_party_gateway_id,
                conns,
                world,
                deleted_character_reconciliation_requested: AtomicBool::new(false),
                deleted_character_reconciliation_running: AtomicBool::new(false),
                guild_cleanup: Default::default(),
            }),
            None,
        );
        for inner in coordinator.1.conns.values() {
            let shard = Self(inner.clone(), coordinator.1.clone(), None);
            shard.certify_package_party_authority()?;
        }
        coordinator.arm_shared_world_view()?;
        spawn_loot_roll_relay(coordinator.clone());
        super::member_stats_relay::spawn_member_stats_relay(coordinator.clone());
        spawn_load_sampler(
            coordinator.clone(),
            cfg.stdb_uri.clone(),
            cfg.gateway_id.clone(),
        );
        Ok(coordinator)
    }

    pub(crate) fn world_view(&self) -> Arc<super::world_view::WorldView> {
        self.1.world.clone()
    }

    pub(crate) fn is_single_shard_realm(&self) -> bool {
        self.1.map.databases().len() == 1
    }

    /// Register before any World Session; shard-vector order fixes stable ShardIds.
    fn arm_shared_world_view(&self) -> Result<()> {
        let view = self.world_view();
        let shards = self.all_shards();
        view.set_shards(shards.clone());
        for (id, shard) in shards.iter().enumerate() {
            super::world_view::arm_shard(view.clone(), shard.clone(), id)?;
            let (hook_view, hook_shard) = (view.clone(), shard.clone());
            shard.0.on_reconnect.lock().unwrap().push(Arc::new(move || {
                if let Err(error) =
                    super::world_view::arm_shard(hook_view.clone(), hook_shard.clone(), id)
                {
                    log::error!("{} Relay setup failed: {error:#}", hook_shard.shard_name());
                    let _ = hook_shard.0.coord().conn.disconnect();
                }
            }));
        }
        if let Ok(realm) = self.realm_core() {
            if !shards.iter().any(|s| s.shard_name() == realm.shard_name()) {
                let world = self.clone();
                super::world_view::arm_realm_private(view.clone(), realm.clone(), world.clone());
                let (hook_view, hook_realm) = (view.clone(), realm.clone());
                realm.0.on_reconnect.lock().unwrap().push(Arc::new(move || {
                    super::world_view::arm_realm_private(
                        hook_view.clone(),
                        hook_realm.clone(),
                        world.clone(),
                    );
                }));
            }
        }
        Ok(())
    }

    pub fn shard_name(&self) -> &str {
        &self.0.db_name
    }

    pub(crate) fn is_sharded(&self) -> bool {
        self.1.conns.len() > 1
    }

    pub fn shard_for(&self, map_id: u32, instance_id: u64) -> Option<Coordinator> {
        let db = self
            .1
            .map
            .resolve_connected(map_id, instance_id, |d| self.1.conns.contains_key(d));
        self.shard_handle(db)
    }

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
        Some(Coordinator(inner?.clone(), self.1.clone(), self.2.clone()))
    }

    pub(crate) fn all_shards(&self) -> Vec<Coordinator> {
        self.world_shards()
            .into_iter()
            .map(|(_, coord)| coord)
            .collect()
    }

    pub(crate) fn realm_core(&self) -> Result<Coordinator> {
        let db = self
            .1
            .map
            .auth_db(|d| {
                self.1.conns.get(d).is_some_and(|inner| {
                    let live = inner.coord();
                    live.is_healthy()
                })
            })
            .map_err(|db| {
                anyhow!(
                    "Realm-core database {db} is unavailable; refusing to use World Shard authority"
                )
            })?;
        let inner = self.1.conns.get(db).ok_or_else(|| {
            anyhow!("Realm-core authority database {db} missing from the coordinator set")
        })?;
        Ok(Coordinator(inner.clone(), self.1.clone(), self.2.clone()))
    }

    pub(crate) fn world_shards(&self) -> Vec<(String, Coordinator)> {
        self.1
            .map
            .shards()
            .into_iter()
            .filter_map(|db| {
                let inner = self.1.conns.get(&db)?.clone();
                Some((db, Coordinator(inner, self.1.clone(), self.2.clone())))
            })
            .collect()
    }

    pub(crate) fn world_shards_for_absence(&self) -> Result<Vec<(String, Coordinator)>> {
        self.1
            .map
            .shards()
            .into_iter()
            .map(|db| {
                let inner = self.1.conns.get(&db).cloned().ok_or_else(|| {
                    anyhow!("World Shard {db} is configured but has no Coordinator connection")
                })?;
                if !inner.coord().is_healthy() {
                    return Err(anyhow!(
                        "World Shard {db} has no healthy Coordinator subscription"
                    ));
                }
                Ok((db, Coordinator(inner, self.1.clone(), self.2.clone())))
            })
            .collect()
    }

    pub(crate) fn shard_map(&self) -> &ShardMap {
        &self.1.map
    }

    pub(crate) fn configured_world_shards(&self) -> Result<Vec<Coordinator>> {
        self.1
            .map
            .shards()
            .into_iter()
            .map(|name| {
                let inner = self.1.conns.get(&name).ok_or_else(|| {
                    anyhow!("Shard {name} is configured but has no Coordinator connection")
                })?;
                if !inner.coord().is_healthy() {
                    return Err(anyhow!(
                        "Shard {name} has no healthy Coordinator subscription"
                    ));
                }
                Ok(Coordinator(inner.clone(), self.1.clone(), self.2.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod connect_build_shape {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::runtime::{Builder, Handle, Runtime};

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

    #[test]
    fn connect_build_does_not_starve_on_a_full_blocking_pool() {
        let (rt, stop) = saturated(4, 4);
        let handle = rt.handle().clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _guard = handle.enter();
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

    // enter() shares the runtime; without it the SDK creates a private runtime per connection.
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
}
