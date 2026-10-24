use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use spacetimedb::client::{ClientActorId, ClientConnection, OutboundMessage};
use spacetimedb_auth::identity::ConnectionAuthCtx;
use spacetimedb_client_api::{ControlStateReadAccess, NodeDelegate};
use spacetimedb_client_api_messages::websocket::v2;
use spacetimedb_lib::{bsatn, identity::AuthCtx, ConnectionId, Identity, SpacetimeType};
use tokio::sync::mpsc;

use crate::StandaloneEnv;

#[derive(SpacetimeType)]
#[sats(crate = spacetimedb_lib)]
pub(super) struct Cipher {
    connection_id: ConnectionId,
    pub username: String,
    pub session_key: Vec<u8>,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
}

#[derive(SpacetimeType, Clone, Copy)]
#[sats(crate = spacetimedb_lib)]
enum Audience {
    Connection(ConnectionId),
    Aoi(Aoi),
    Grant(Recipient),
    Revoke(Recipient),
    Clear(ConnectionId),
}

#[derive(SpacetimeType, Clone, Copy)]
#[sats(crate = spacetimedb_lib)]
struct Aoi {
    layer: u8,
    entity_guid: u64,
    exclude_connection: Option<ConnectionId>,
}

#[derive(SpacetimeType, Clone, Copy)]
#[sats(crate = spacetimedb_lib)]
struct Recipient {
    connection_id: ConnectionId,
    layer: u8,
    entity_guid: u64,
}

#[derive(SpacetimeType)]
#[sats(crate = spacetimedb_lib)]
pub(super) struct Packet {
    id: u64,
    audience: Audience,
    pub opcode: Option<u16>,
    pub body: Vec<u8>,
    pub close_after: bool,
    pub not_before_micros: i64,
    pub authentication_complete: bool,
}

#[derive(Default)]
pub(super) struct Delivery {
    pub cipher: Option<Cipher>,
    pub packets: Vec<Arc<Packet>>,
    budget: Option<tokio::sync::OwnedSemaphorePermit>,
}

struct Client {
    sender: mpsc::Sender<Delivery>,
    budget: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct State {
    closed: bool,
    clients: HashMap<ConnectionId, Client>,
    recipients: HashMap<(u8, u64), HashSet<ConnectionId>>,
    subjects: HashMap<ConnectionId, HashSet<(u8, u64)>>,
}

impl State {
    fn remove(&mut self, connection: ConnectionId) {
        self.clients.remove(&connection);
        self.clear(connection);
    }

    fn clear(&mut self, connection: ConnectionId) {
        if let Some(subjects) = self.subjects.remove(&connection) {
            for subject in subjects {
                if let Some(recipients) = self.recipients.get_mut(&subject) {
                    recipients.remove(&connection);
                    if recipients.is_empty() {
                        self.recipients.remove(&subject);
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Feed(Mutex<State>);

pub(super) struct Registration {
    feed: Arc<Feed>,
    connection_id: ConnectionId,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.feed
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.connection_id);
    }
}

struct FeedLifetime(Arc<Feed>);

impl Drop for FeedLifetime {
    fn drop(&mut self) {
        let mut state = self
            .0
             .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        state.clients.clear();
    }
}

impl Feed {
    pub(super) fn register(
        self: &Arc<Self>,
        connection_id: ConnectionId,
    ) -> Result<(Registration, mpsc::Receiver<Delivery>)> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure!(!state.closed, "native output subscription stopped");
        ensure!(
            !state.clients.contains_key(&connection_id),
            "native connection id collision"
        );
        let (tx, rx) = mpsc::channel(64);
        state.clients.insert(
            connection_id,
            Client {
                sender: tx,
                budget: Arc::new(tokio::sync::Semaphore::new(2 * 1024 * 1024)),
            },
        );
        Ok((
            Registration {
                feed: self.clone(),
                connection_id,
            },
            rx,
        ))
    }

    fn apply(&self, message: OutboundMessage) -> Result<bool> {
        let OutboundMessage::V2(message) = message else {
            bail!("native feed requires protocol v2")
        };
        let mut deliveries = HashMap::<ConnectionId, Delivery>::new();
        let mut packets = Vec::<Arc<Packet>>::new();
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut row = |table: &str, bytes: &[u8], insert: bool| -> Result<()> {
            match table {
                "game_native_cipher" => {
                    if insert {
                        let cipher: Cipher = bsatn::from_slice(bytes)?;
                        let delivery = deliveries.entry(cipher.connection_id).or_default();
                        ensure!(delivery.cipher.is_none(), "multiple native cipher records");
                        delivery.cipher = Some(cipher);
                    }
                }
                "game_native_packet" => {
                    if insert {
                        packets.push(Arc::new(bsatn::from_slice(bytes)?));
                    }
                }
                _ => bail!("unexpected native table {table}"),
            }
            Ok(())
        };
        let initial = matches!(&message, v2::ServerMessage::SubscribeApplied(_));
        match message {
            v2::ServerMessage::SubscribeApplied(applied) => {
                for table in &applied.rows.tables {
                    for bytes in &table.rows {
                        row(table.table.as_ref(), &bytes, true)?;
                    }
                }
            }
            v2::ServerMessage::TransactionUpdate(update) => {
                for query in &update.query_sets {
                    for table in &query.tables {
                        for rows in &table.rows {
                            match rows {
                                v2::TableUpdateRows::PersistentTable(rows) => {
                                    for bytes in &rows.deletes {
                                        row(table.table_name.as_ref(), &bytes, false)?;
                                    }
                                    for bytes in &rows.inserts {
                                        row(table.table_name.as_ref(), &bytes, true)?;
                                    }
                                }
                                v2::TableUpdateRows::EventTable(rows) => {
                                    for bytes in &rows.events {
                                        row(table.table_name.as_ref(), &bytes, true)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            v2::ServerMessage::SubscriptionError(_) => bail!("native output subscription failed"),
            _ => {}
        }
        packets.sort_unstable_by_key(|packet| packet.id);
        for packet in packets {
            match packet.audience {
                Audience::Clear(connection) => state.clear(connection),
                Audience::Connection(connection) => {
                    deliveries
                        .entry(connection)
                        .or_default()
                        .packets
                        .push(packet);
                }
                Audience::Aoi(Aoi {
                    layer,
                    entity_guid,
                    exclude_connection,
                }) => {
                    if let Some(viewers) = state.recipients.get(&(layer, entity_guid)) {
                        for connection in viewers {
                            if Some(*connection) != exclude_connection {
                                deliveries
                                    .entry(*connection)
                                    .or_default()
                                    .packets
                                    .push(packet.clone());
                            }
                        }
                    }
                }
                Audience::Grant(recipient) => {
                    let connection = recipient.connection_id;
                    if !state.clients.contains_key(&connection) {
                        continue;
                    }
                    let subject = (recipient.layer, recipient.entity_guid);
                    state
                        .recipients
                        .entry(subject)
                        .or_default()
                        .insert(connection);
                    state
                        .subjects
                        .entry(connection)
                        .or_default()
                        .insert(subject);
                    deliveries
                        .entry(connection)
                        .or_default()
                        .packets
                        .push(packet);
                }
                Audience::Revoke(recipient) => {
                    let connection = recipient.connection_id;
                    let subject = (recipient.layer, recipient.entity_guid);
                    if let Some(recipients) = state.recipients.get_mut(&subject) {
                        recipients.remove(&connection);
                        if recipients.is_empty() {
                            state.recipients.remove(&subject);
                        }
                    }
                    if let Some(subjects) = state.subjects.get_mut(&connection) {
                        subjects.remove(&subject);
                    }
                    deliveries
                        .entry(connection)
                        .or_default()
                        .packets
                        .push(packet);
                }
            }
        }
        for (connection, mut delivery) in deliveries {
            let Some(client) = state.clients.get(&connection) else {
                continue;
            };
            delivery.packets.sort_unstable_by_key(|packet| packet.id);
            let size: usize = delivery
                .packets
                .iter()
                .map(|packet| packet.body.len() + 4)
                .sum();
            let permit = u32::try_from(size.max(1))
                .ok()
                .and_then(|size| client.budget.clone().try_acquire_many_owned(size).ok());
            let Some(permit) = permit else {
                state.remove(connection);
                log::warn!("native output byte limit reached for connection {connection}");
                continue;
            };
            delivery.budget = Some(permit);
            if client.sender.try_send(delivery).is_err() {
                state.remove(connection);
                log::warn!("native output queue closed for connection {connection}");
            }
        }
        Ok(initial)
    }
}

pub(super) async fn shared(
    ctx: Arc<StandaloneEnv>,
    database: Identity,
    owner: Identity,
) -> Result<Arc<Feed>> {
    static FEED: tokio::sync::Mutex<Option<Arc<Feed>>> = tokio::sync::Mutex::const_new(None);
    let mut current = FEED.lock().await;
    if let Some(feed) = current.as_ref() {
        if !feed
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed
        {
            return Ok(feed.clone());
        }
    }
    let feed =
        tokio::time::timeout(Duration::from_secs(10), create(ctx, database, owner)).await??;
    *current = Some(feed.clone());
    Ok(feed)
}

async fn create(ctx: Arc<StandaloneEnv>, database: Identity, owner: Identity) -> Result<Arc<Feed>> {
    let db = ctx
        .get_database_by_identity(&database)
        .await?
        .context("native database missing")?;
    let leader = ctx.leader(db.id).await?;
    let mut module_rx = leader.module_watcher().await?;
    let id = ClientActorId {
        identity: owner,
        connection_id: super::new_connection_id(),
        name: ctx.client_actor_index().next_client_name(),
    };
    let auth = ConnectionAuthCtx {
        identity: owner,
        jwt_payload: None,
    };
    let connected =
        ClientConnection::call_client_connected_maybe_reject(&mut module_rx, id, auth.clone())
            .await?;
    let feed = Arc::new(Feed::default());
    let actor_feed = feed.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let client = ClientConnection::spawn(
        id,
        auth,
        AuthCtx::new(owner, owner),
        super::config(),
        leader.replica_id,
        module_rx,
        move |mut client, mut receiver| async move {
            let _client_lifetime = super::ClientLifetime(client.clone());
            let _feed_lifetime = FeedLifetime(actor_feed.clone());
            let process = async move {
                let mut ready = Some(ready_tx);
                let mut messages = Vec::with_capacity(32);
                while receiver.recv_many(&mut messages, 32).await != 0 {
                    for message in messages.drain(..) {
                        match actor_feed.apply(message) {
                            Ok(true) => {
                                if let Some(tx) = ready.take() {
                                    let _ = tx.send(());
                                }
                            }
                            Ok(false) => {}
                            Err(error) => {
                                log::error!("native output subscription: {error:#}");
                                return;
                            }
                        }
                    }
                    tokio::task::yield_now().await;
                }
            };
            tokio::select! { _ = process => {}, _ = client.watch_module_host() => {} }
        },
        connected,
    )
    .await;
    client
        .subscribe_v2(
            v2::Subscribe {
                request_id: 1,
                query_set_id: v2::QuerySetId::new(1),
                query_strings: vec![
                    "SELECT * FROM game_native_cipher".into(),
                    "SELECT * FROM game_native_packet".into(),
                ]
                .into_boxed_slice(),
            },
            Instant::now(),
        )
        .await?;
    ready_rx.await?;
    Ok(feed)
}
