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
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(super) struct Cipher {
    connection_id: ConnectionId,
    pub username: String,
    pub session_key: Vec<u8>,
    pub client_seed: u32,
    pub client_proof: Vec<u8>,
}

#[derive(SpacetimeType, Clone, Copy)]
#[sats(crate = spacetimedb_lib)]
#[cfg_attr(test, derive(Debug, PartialEq))]
enum Audience {
    Connection(ConnectionId),
    Aoi(Aoi),
    Grant(Recipient),
    Revoke(Recipient),
    Clear(ConnectionId),
}

#[derive(SpacetimeType, Clone, Copy)]
#[sats(crate = spacetimedb_lib)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Aoi {
    layer: u8,
    entity_guid: u64,
    exclude_connection: Option<ConnectionId>,
}

#[derive(SpacetimeType, Clone, Copy)]
#[sats(crate = spacetimedb_lib)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Recipient {
    connection_id: ConnectionId,
    layer: u8,
    entity_guid: u64,
}

#[derive(SpacetimeType)]
#[sats(crate = spacetimedb_lib)]
#[cfg_attr(test, derive(Debug, PartialEq))]
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
    router: Router,
}

impl State {
    fn remove(&mut self, connection: ConnectionId) {
        self.clients.remove(&connection);
        self.router.clear(connection);
    }
}

#[derive(Default)]
struct Router {
    recipients: HashMap<(u8, u64), HashSet<ConnectionId>>,
    subjects: HashMap<ConnectionId, HashSet<(u8, u64)>>,
}

impl Router {
    // Applies one transaction's packets in id order; returns each connection's packets in id order.
    fn route(
        &mut self,
        mut packets: Vec<Arc<Packet>>,
        registered: impl Fn(&ConnectionId) -> bool,
    ) -> HashMap<ConnectionId, Vec<Arc<Packet>>> {
        let mut deliveries = HashMap::<ConnectionId, Vec<Arc<Packet>>>::new();
        packets.sort_unstable_by_key(|packet| packet.id);
        for packet in packets {
            match packet.audience {
                Audience::Clear(connection) => self.clear(connection),
                Audience::Connection(connection) => {
                    deliveries.entry(connection).or_default().push(packet);
                }
                Audience::Aoi(Aoi {
                    layer,
                    entity_guid,
                    exclude_connection,
                }) => {
                    if let Some(viewers) = self.recipients.get(&(layer, entity_guid)) {
                        for connection in viewers {
                            if Some(*connection) != exclude_connection {
                                deliveries
                                    .entry(*connection)
                                    .or_default()
                                    .push(packet.clone());
                            }
                        }
                    }
                }
                Audience::Grant(recipient) => {
                    let connection = recipient.connection_id;
                    if !registered(&connection) {
                        continue;
                    }
                    let subject = (recipient.layer, recipient.entity_guid);
                    self.recipients
                        .entry(subject)
                        .or_default()
                        .insert(connection);
                    self.subjects.entry(connection).or_default().insert(subject);
                    deliveries.entry(connection).or_default().push(packet);
                }
                Audience::Revoke(recipient) => {
                    let connection = recipient.connection_id;
                    let subject = (recipient.layer, recipient.entity_guid);
                    if let Some(recipients) = self.recipients.get_mut(&subject) {
                        recipients.remove(&connection);
                        if recipients.is_empty() {
                            self.recipients.remove(&subject);
                        }
                    }
                    if let Some(subjects) = self.subjects.get_mut(&connection) {
                        subjects.remove(&subject);
                    }
                    deliveries.entry(connection).or_default().push(packet);
                }
            }
        }
        deliveries
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
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = &mut *state;
        let routed = state
            .router
            .route(packets, |connection| state.clients.contains_key(connection));
        for (connection, packets) in routed {
            deliveries.entry(connection).or_default().packets = packets;
        }
        for (connection, mut delivery) in deliveries {
            let Some(client) = state.clients.get(&connection) else {
                continue;
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    include!("native_delivery_rows.rs");

    const CONNECTION: ConnectionId = ConnectionId::from_u128(7);
    const VIEWER: ConnectionId = ConnectionId::from_u128(1);
    const OTHER_VIEWER: ConnectionId = ConnectionId::from_u128(2);
    const EXCLUDED_VIEWER: ConnectionId = ConnectionId::from_u128(3);
    const WORLD_ENTITY: u8 = 0;
    const GAME_OBJECT: u8 = 1;
    const SUBJECT: u64 = 100;
    const OTHER_SUBJECT: u64 = 200;

    fn row(id: u64, audience: Audience, opcode: Option<u16>, body: Vec<u8>) -> Packet {
        Packet {
            id,
            audience,
            opcode,
            body,
            close_after: false,
            not_before_micros: 0,
            authentication_complete: false,
        }
    }

    fn packet(id: u64, audience: Audience) -> Arc<Packet> {
        Arc::new(row(id, audience, None, Vec::new()))
    }

    fn grant(id: u64, connection_id: ConnectionId, entity_guid: u64) -> Arc<Packet> {
        packet(
            id,
            Audience::Grant(Recipient {
                connection_id,
                layer: WORLD_ENTITY,
                entity_guid,
            }),
        )
    }

    fn revoke(id: u64, connection_id: ConnectionId, entity_guid: u64) -> Arc<Packet> {
        packet(
            id,
            Audience::Revoke(Recipient {
                connection_id,
                layer: WORLD_ENTITY,
                entity_guid,
            }),
        )
    }

    fn aoi(id: u64, entity_guid: u64) -> Arc<Packet> {
        packet(
            id,
            Audience::Aoi(Aoi {
                layer: WORLD_ENTITY,
                entity_guid,
                exclude_connection: None,
            }),
        )
    }

    fn route(router: &mut Router, packets: Vec<Arc<Packet>>) -> HashMap<ConnectionId, Vec<u64>> {
        router
            .route(packets, |_| true)
            .into_iter()
            .map(|(connection, packets)| {
                (connection, packets.iter().map(|packet| packet.id).collect())
            })
            .collect()
    }

    fn granted(connections: &[ConnectionId], entity_guid: u64) -> Router {
        let mut router = Router::default();
        let grants = connections
            .iter()
            .zip(1..)
            .map(|(connection, id)| grant(id, *connection, entity_guid))
            .collect();
        route(&mut router, grants);
        router
    }

    #[test]
    fn packets_reach_their_connection_in_id_order() {
        let mut router = Router::default();
        let routed = route(
            &mut router,
            vec![
                packet(5, Audience::Connection(VIEWER)),
                packet(2, Audience::Connection(VIEWER)),
                packet(9, Audience::Connection(VIEWER)),
            ],
        );
        assert_eq!(routed, HashMap::from([(VIEWER, vec![2, 5, 9])]));
    }

    #[test]
    fn grant_delivers_its_create_before_packets_through_it() {
        let mut router = Router::default();
        let routed = route(
            &mut router,
            vec![aoi(2, SUBJECT), grant(1, VIEWER, SUBJECT)],
        );
        assert_eq!(routed, HashMap::from([(VIEWER, vec![1, 2])]));
        let later = route(&mut router, vec![aoi(3, SUBJECT)]);
        assert_eq!(later, HashMap::from([(VIEWER, vec![3])]));
    }

    #[test]
    fn packet_before_a_grant_in_id_order_does_not_use_it() {
        let mut router = Router::default();
        let routed = route(
            &mut router,
            vec![grant(2, VIEWER, SUBJECT), aoi(1, SUBJECT)],
        );
        assert_eq!(routed, HashMap::from([(VIEWER, vec![2])]));
    }

    #[test]
    fn grant_for_an_unregistered_connection_is_dropped() {
        let mut router = Router::default();
        let routed = router.route(vec![grant(1, VIEWER, SUBJECT)], |_| false);
        assert!(routed.is_empty());
        assert!(route(&mut router, vec![aoi(2, SUBJECT)]).is_empty());
    }

    #[test]
    fn grant_covers_only_its_layer() {
        let mut router = granted(&[VIEWER], SUBJECT);
        let other_layer = packet(
            10,
            Audience::Aoi(Aoi {
                layer: GAME_OBJECT,
                entity_guid: SUBJECT,
                exclude_connection: None,
            }),
        );
        assert!(route(&mut router, vec![other_layer]).is_empty());
    }

    #[test]
    fn revoke_removes_its_recipient_before_later_packets() {
        let mut router = granted(&[VIEWER], SUBJECT);
        let routed = route(
            &mut router,
            vec![
                aoi(12, SUBJECT),
                revoke(11, VIEWER, SUBJECT),
                aoi(10, SUBJECT),
            ],
        );
        assert_eq!(routed, HashMap::from([(VIEWER, vec![10, 11])]));
        assert!(route(&mut router, vec![aoi(13, SUBJECT)]).is_empty());
    }

    #[test]
    fn revoke_keeps_other_recipients_and_subjects() {
        let mut router = granted(&[VIEWER, OTHER_VIEWER], SUBJECT);
        route(&mut router, vec![grant(10, VIEWER, OTHER_SUBJECT)]);
        route(&mut router, vec![revoke(11, VIEWER, SUBJECT)]);
        let routed = route(&mut router, vec![aoi(12, SUBJECT), aoi(13, OTHER_SUBJECT)]);
        assert_eq!(
            routed,
            HashMap::from([(VIEWER, vec![13]), (OTHER_VIEWER, vec![12])])
        );
    }

    #[test]
    fn clear_removes_every_grant_of_its_connection() {
        let mut router = granted(&[VIEWER, OTHER_VIEWER], SUBJECT);
        route(&mut router, vec![grant(10, VIEWER, OTHER_SUBJECT)]);
        let routed = route(
            &mut router,
            vec![
                aoi(12, SUBJECT),
                packet(11, Audience::Clear(VIEWER)),
                aoi(13, OTHER_SUBJECT),
                packet(14, Audience::Connection(VIEWER)),
            ],
        );
        assert_eq!(
            routed,
            HashMap::from([(VIEWER, vec![14]), (OTHER_VIEWER, vec![12])])
        );
    }

    #[test]
    fn aoi_packet_reaches_every_recipient_except_the_excluded_connection() {
        let mut router = granted(&[EXCLUDED_VIEWER, VIEWER, OTHER_VIEWER], SUBJECT);
        let excluding = packet(
            10,
            Audience::Aoi(Aoi {
                layer: WORLD_ENTITY,
                entity_guid: SUBJECT,
                exclude_connection: Some(EXCLUDED_VIEWER),
            }),
        );
        let routed = route(&mut router, vec![excluding]);
        assert_eq!(
            routed,
            HashMap::from([(VIEWER, vec![10]), (OTHER_VIEWER, vec![10])])
        );
    }

    #[test]
    fn aoi_packet_without_recipients_reaches_nobody() {
        let mut router = granted(&[VIEWER], SUBJECT);
        assert!(route(&mut router, vec![aoi(10, OTHER_SUBJECT)]).is_empty());
    }

    #[test]
    fn module_cipher_row_decodes() {
        let cipher = Cipher {
            connection_id: CONNECTION,
            username: "AB".into(),
            session_key: vec![1, 2],
            client_seed: 9,
            client_proof: vec![3],
        };
        assert_eq!(bsatn::from_slice::<Cipher>(CIPHER).unwrap(), cipher);
        assert_eq!(bsatn::to_vec(&cipher).unwrap(), CIPHER);
    }

    #[test]
    fn module_packet_rows_decode() {
        let recipient = Recipient {
            connection_id: CONNECTION,
            layer: 2,
            entity_guid: 8,
        };
        let rows = [
            (
                Packet {
                    close_after: true,
                    not_before_micros: 6,
                    ..row(1, Audience::Connection(CONNECTION), Some(0x1dd), vec![42])
                },
                CONNECTION_PACKET,
            ),
            (
                Packet {
                    authentication_complete: true,
                    ..row(
                        2,
                        Audience::Aoi(Aoi {
                            layer: 1,
                            entity_guid: 8,
                            exclude_connection: Some(CONNECTION),
                        }),
                        Some(0xb5),
                        vec![43, 44],
                    )
                },
                AOI_PACKET,
            ),
            (
                row(3, Audience::Grant(recipient), None, Vec::new()),
                GRANT_PACKET,
            ),
            (
                row(4, Audience::Revoke(recipient), Some(0xaa), vec![8]),
                REVOKE_PACKET,
            ),
            (
                row(5, Audience::Clear(CONNECTION), None, Vec::new()),
                CLEAR_PACKET,
            ),
        ];
        for (expected, fixture) in rows {
            assert_eq!(bsatn::from_slice::<Packet>(fixture).unwrap(), expected);
            assert_eq!(bsatn::to_vec(&expected).unwrap(), fixture);
        }
    }
}
