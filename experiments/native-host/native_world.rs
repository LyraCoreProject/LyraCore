use crate::StandaloneEnv;
use anyhow::{bail, ensure, Context, Result};
use spacetimedb::client::{
    ClientActorId, ClientConfig, ClientConnection, ClientConnectionReceiver, OutboundMessage,
    Protocol, WsVersion,
};
use spacetimedb::host::FunctionArgs;
use spacetimedb_auth::identity::ConnectionAuthCtx;
use spacetimedb_client_api::{ControlStateReadAccess, NodeDelegate};
use spacetimedb_client_api_messages::websocket::{v1, v2};
use spacetimedb_lib::{identity::AuthCtx, ConnectionId, Identity};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use wow_srp::vanilla_header::EncrypterHalf;

#[path = "native_wire.rs"]
mod wire;

pub async fn start(ctx: Arc<StandaloneEnv>) -> Result<()> {
    let Ok(address) = std::env::var("LYRACORE_EXPERIMENT_WOW_ADDR") else {
        return Ok(());
    };
    let address: SocketAddr = address.parse()?;
    ensure!(
        address.ip().is_loopback(),
        "native experiment must bind loopback"
    );
    let database = std::env::var("LYRACORE_EXPERIMENT_DATABASE")?;
    ensure!(
        database.starts_with("disposable-native-"),
        "expected disposable experiment database"
    );
    let variant = std::env::var("LYRACORE_EXPERIMENT_AOI")?;
    ensure!(
        ["cell", "edge", "direct-cell", "direct-edge", "shared-edge"].contains(&variant.as_str()),
        "unsupported AOI variant"
    );
    let listener = TcpListener::bind(address).await?;
    tokio::spawn(async move {
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("native world accept: {error}");
                    break;
                }
            };
            let (ctx, database, variant) = (ctx.clone(), database.clone(), variant.clone());
            tokio::spawn(async move {
                if let Err(error) = connect(ctx, socket, &database, &variant).await {
                    log::warn!("native world: {error:#}");
                }
            });
        }
    });
    Ok(())
}

async fn connect(
    ctx: Arc<StandaloneEnv>,
    mut socket: TcpStream,
    database: &str,
    variant: &str,
) -> Result<()> {
    static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
    let (guid, crypto) =
        tokio::time::timeout(Duration::from_secs(10), wire::server_auth(&mut socket)).await??;
    let database_identity = ctx
        .lookup_database_identity(database)
        .await?
        .context("experiment database missing")?;
    let db = ctx
        .get_database_by_identity(&database_identity)
        .await?
        .context("experiment database missing")?;
    let leader = ctx.leader(db.id).await?;
    let mut module_rx = leader.module_watcher().await?;
    let identity = Identity::from_claims("native-host-experiment", &format!("E{guid}"));
    let id = ClientActorId {
        identity,
        connection_id: ConnectionId::from_u128(
            NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed) as u128
        ),
        name: ctx.client_actor_index().next_client_name(),
    };
    let auth = ConnectionAuthCtx {
        identity,
        jwt_payload: None,
    };
    let shared = if variant == "shared-edge" {
        Some(
            shared_feed(ctx.clone(), database_identity, db.owner_identity)
                .await?
                .register(identity)?,
        )
    } else {
        None
    };
    let connected =
        ClientConnection::call_client_connected_maybe_reject(&mut module_rx, id, auth.clone())
            .await?;
    let config = ClientConfig {
        protocol: Protocol::Binary,
        version: WsVersion::V2,
        compression: v1::Compression::None,
        tx_update_full: false,
        confirmed_reads: true,
    };
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let client = ClientConnection::spawn(
        id,
        auth,
        AuthCtx::new(
            db.owner_identity,
            if variant.starts_with("direct-") {
                db.owner_identity
            } else {
                identity
            },
        ),
        config,
        leader.replica_id,
        module_rx,
        move |client, receiver| async move {
            if start_rx.await.is_ok() {
                if let Err(error) = run(client.clone(), receiver, shared, socket, crypto).await {
                    log::debug!("native world Character {guid}: {error:#}");
                }
            }
            client.disconnect().await;
        },
        connected,
    )
    .await;
    let entered = client
        .call_reducer(
            "enter",
            FunctionArgs::Json(format!("[{guid}]").into()),
            1,
            Instant::now(),
            v1::CallReducerFlags::NoSuccessNotify,
        )
        .await?;
    entered.outcome.into_result()?;
    if variant == "shared-edge" {
        let _ = start_tx.send(());
        return Ok(());
    }
    let query = match variant {
        "direct-cell" => format!("SELECT motion.* FROM viewer_cell JOIN motion ON viewer_cell.cell = motion.cell WHERE viewer_cell.viewer = 0x{identity} AND motion.hidden = false AND (motion.ghost_only = false OR viewer_cell.ghost = true)"),
        "direct-edge" => format!("SELECT motion.* FROM visible_entity JOIN motion ON visible_entity.guid = motion.guid WHERE visible_entity.viewer = 0x{identity}"),
        _ => format!("SELECT * FROM {variant}_motion"),
    };
    client
        .subscribe_v2(
            v2::Subscribe {
                request_id: 2,
                query_set_id: v2::QuerySetId::new(1),
                query_strings: vec![query.into_boxed_str()].into_boxed_slice(),
            },
            Instant::now(),
        )
        .await?;
    let _ = start_tx.send(());
    Ok(())
}

async fn run(
    mut client: ClientConnection,
    mut receiver: ClientConnectionReceiver,
    mut shared: Option<mpsc::Receiver<Delivery>>,
    socket: TcpStream,
    crypto: wow_srp::vanilla_header::HeaderCrypto,
) -> Result<()> {
    let (mut reader, mut writer) = socket.into_split();
    let (mut encrypt, mut decrypt) = crypto.split();
    let (reply_tx, mut reply_rx) = mpsc::channel::<(u16, Vec<u8>)>(64);
    let input_client = client.clone();
    let input = async move {
        loop {
            let (opcode, body) = wire::read_client_packet(&mut reader, &mut decrypt).await?;
            match u16::try_from(opcode)? {
                wire::HEARTBEAT => {
                    let (x, y, z, tick) = wire::movement_args(&body)?;
                    let args = format!("[{x},{y},{z},{tick}]");
                    let result = input_client
                        .call_reducer(
                            "move_character",
                            FunctionArgs::Json(args.into()),
                            0,
                            Instant::now(),
                            v1::CallReducerFlags::NoSuccessNotify,
                        )
                        .await?;
                    result.outcome.into_result()?;
                }
                wire::PING => {
                    ensure!(body.len() == 8, "invalid ping");
                    reply_tx.try_send((wire::PONG, body[..4].to_vec()))?;
                }
                _ => bail!("unsupported experiment opcode {opcode:#x}"),
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let output = async move {
        let mut initial = Vec::new();
        wire::append_server_packet(
            &mut initial,
            &mut encrypt,
            wire::AUTH_RESPONSE,
            &[0x0c, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        )?;
        writer.write_all(&initial).await?;
        let mut projection = wire::Projection::default();
        let mut messages = Vec::with_capacity(32);
        loop {
            let bytes = tokio::select! {
                count = receiver.recv_many(&mut messages, 32) => {
                    if count == 0 { break; }
                    let mut bytes = Vec::new();
                    for message in messages.drain(..) {
                        bytes.extend(project(message, &mut projection, &mut encrypt)?);
                    }
                    bytes
                }
                delivery = async { match &mut shared { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                    let delivery = delivery.context("shared delivery stopped")?;
                    projection.update(delivery.inserts, delivery.deletes, &mut encrypt)?
                }
                Some((opcode, body)) = reply_rx.recv() => {
                    let mut bytes = Vec::new();
                    wire::append_server_packet(&mut bytes, &mut encrypt, opcode, &body)?;
                    bytes
                }
            };
            if !bytes.is_empty() {
                tokio::time::timeout(Duration::from_secs(2), writer.write_all(&bytes)).await??;
            }
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        result = input => result,
        result = output => result,
        result = client.watch_module_host() => { result?; Ok(()) },
    }
}

fn project(
    message: OutboundMessage,
    projection: &mut wire::Projection,
    encrypt: &mut EncrypterHalf,
) -> Result<Vec<u8>> {
    let OutboundMessage::V2(message) = message else {
        bail!("expected v2 internal update")
    };
    let mut inserts = Vec::new();
    let mut deletes = Vec::new();
    match message {
        v2::ServerMessage::SubscribeApplied(applied) => {
            for table in &applied.rows.tables {
                for row in &table.rows {
                    inserts.push(wire::Motion::from_bsatn(&row)?);
                }
            }
        }
        v2::ServerMessage::TransactionUpdate(update) => {
            for query in &update.query_sets {
                for table in &query.tables {
                    for rows in &table.rows {
                        if let v2::TableUpdateRows::PersistentTable(rows) = rows {
                            for row in &rows.inserts {
                                inserts.push(wire::Motion::from_bsatn(&row)?);
                            }
                            for row in &rows.deletes {
                                deletes.push(wire::Motion::from_bsatn(&row)?);
                            }
                        }
                    }
                }
            }
        }
        v2::ServerMessage::SubscriptionError(error) => bail!("subscription failed: {error:?}"),
        v2::ServerMessage::ReducerResult(result) => match result.result {
            v2::ReducerOutcome::Err(error) => bail!("Module refusal: {error:?}"),
            v2::ReducerOutcome::InternalError(error) => bail!("Module failure: {error:?}"),
            _ => {}
        },
        _ => {}
    }
    projection.update(inserts, deletes, encrypt)
}

#[derive(Default)]
struct Delivery {
    inserts: Vec<wire::Motion>,
    deletes: Vec<wire::Motion>,
}

#[derive(Default)]
struct SharedState {
    closed: bool,
    motion: HashMap<u64, wire::Motion>,
    recipients: HashMap<u64, HashSet<Identity>>,
    clients: HashMap<Identity, mpsc::Sender<Delivery>>,
}

#[derive(Default)]
struct SharedFeed(Mutex<SharedState>);

struct FeedLifetime(Arc<SharedFeed>);

impl Drop for FeedLifetime {
    fn drop(&mut self) {
        let mut state = self.0 .0.lock().unwrap();
        state.closed = true;
        state.clients.clear();
    }
}

impl SharedFeed {
    fn register(&self, identity: Identity) -> Result<mpsc::Receiver<Delivery>> {
        let mut state = self.0.lock().unwrap();
        ensure!(!state.closed, "shared feed stopped");
        ensure!(
            !state
                .clients
                .get(&identity)
                .is_some_and(|tx| !tx.is_closed()),
            "Character already connected"
        );
        let (tx, rx) = mpsc::channel(64);
        state.clients.insert(identity, tx);
        Ok(rx)
    }

    fn apply(&self, message: OutboundMessage) -> Result<bool> {
        let OutboundMessage::V2(message) = message else {
            bail!("expected v2 update")
        };
        let mut motions = Vec::new();
        let mut removed_motions = Vec::new();
        let mut grants = Vec::new();
        let mut removed_grants = Vec::new();
        let mut append = |table: &str, bytes: &[u8], insert: bool| -> Result<()> {
            match table {
                "motion" => {
                    let row = wire::Motion::from_bsatn(bytes)?;
                    if insert {
                        motions.push(row);
                    } else {
                        removed_motions.push(row);
                    }
                }
                "visible_entity" => {
                    ensure!(bytes.len() == 48, "unexpected grant length");
                    let viewer = Identity::from_byte_array(bytes[8..40].try_into()?);
                    let guid = u64::from_le_bytes(bytes[40..48].try_into()?);
                    if insert {
                        grants.push((viewer, guid));
                    } else {
                        removed_grants.push((viewer, guid));
                    }
                }
                _ => bail!("unexpected shared table {table}"),
            }
            Ok(())
        };
        let initial = matches!(&message, v2::ServerMessage::SubscribeApplied(_));
        match message {
            v2::ServerMessage::SubscribeApplied(applied) => {
                for table in &applied.rows.tables {
                    for row in &table.rows {
                        append(table.table.as_ref(), &row, true)?;
                    }
                }
            }
            v2::ServerMessage::TransactionUpdate(update) => {
                for query in &update.query_sets {
                    for table in &query.tables {
                        for rows in &table.rows {
                            if let v2::TableUpdateRows::PersistentTable(rows) = rows {
                                for row in &rows.deletes {
                                    append(table.table_name.as_ref(), &row, false)?;
                                }
                                for row in &rows.inserts {
                                    append(table.table_name.as_ref(), &row, true)?;
                                }
                            }
                        }
                    }
                }
            }
            v2::ServerMessage::SubscriptionError(error) => {
                bail!("shared subscription failed: {error:?}")
            }
            _ => {}
        }
        let mut state = self.0.lock().unwrap();
        let mut touched = HashMap::<Identity, HashSet<u64>>::new();
        for (viewer, guid) in &removed_grants {
            state.recipients.entry(*guid).or_default().remove(viewer);
            touched.entry(*viewer).or_default().insert(*guid);
        }
        for (viewer, guid) in grants {
            state.recipients.entry(guid).or_default().insert(viewer);
            touched.entry(viewer).or_default().insert(guid);
        }
        for row in &removed_motions {
            state.motion.remove(&row.guid);
            if let Some(viewers) = state.recipients.get(&row.guid) {
                for viewer in viewers {
                    touched.entry(*viewer).or_default().insert(row.guid);
                }
            }
        }
        for row in motions {
            if let Some(viewers) = state.recipients.get(&row.guid) {
                for viewer in viewers {
                    touched.entry(*viewer).or_default().insert(row.guid);
                }
            }
            state.motion.insert(row.guid, row);
        }
        for (viewer, guids) in touched {
            let Some(sender) = state.clients.get(&viewer) else {
                continue;
            };
            let mut delivery = Delivery::default();
            for guid in guids {
                let row = state
                    .motion
                    .get(&guid)
                    .or_else(|| removed_motions.iter().find(|m| m.guid == guid))
                    .context("grant refers to missing Motion")?;
                if state
                    .recipients
                    .get(&guid)
                    .is_some_and(|set| set.contains(&viewer))
                    && state.motion.contains_key(&guid)
                {
                    delivery.inserts.push(row.clone());
                } else {
                    delivery.deletes.push(row.clone());
                }
            }
            match sender.try_send(delivery) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    log::warn!("native delivery queue full for {viewer}");
                    state.clients.remove(&viewer);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    state.clients.remove(&viewer);
                }
            }
        }
        Ok(initial)
    }
}

async fn shared_feed(
    ctx: Arc<StandaloneEnv>,
    database: Identity,
    owner: Identity,
) -> Result<Arc<SharedFeed>> {
    static FEED: tokio::sync::OnceCell<Arc<SharedFeed>> = tokio::sync::OnceCell::const_new();
    FEED.get_or_try_init(|| async move {
        let db = ctx
            .get_database_by_identity(&database)
            .await?
            .context("missing shared database")?;
        let leader = ctx.leader(db.id).await?;
        let mut module_rx = leader.module_watcher().await?;
        let id = ClientActorId {
            identity: owner,
            connection_id: ConnectionId::from_u128(u128::MAX),
            name: ctx.client_actor_index().next_client_name(),
        };
        let auth = ConnectionAuthCtx {
            identity: owner,
            jwt_payload: None,
        };
        let connected =
            ClientConnection::call_client_connected_maybe_reject(&mut module_rx, id, auth.clone())
                .await?;
        let feed = Arc::new(SharedFeed::default());
        let actor_feed = feed.clone();
        let cleanup_feed = feed.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let client = ClientConnection::spawn(
            id,
            auth,
            AuthCtx::new(owner, owner),
            ClientConfig {
                protocol: Protocol::Binary,
                version: WsVersion::V2,
                compression: v1::Compression::None,
                tx_update_full: false,
                confirmed_reads: true,
            },
            leader.replica_id,
            module_rx,
            move |mut client, mut receiver| async move {
                let _lifetime = FeedLifetime(cleanup_feed);
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
                                    log::error!("shared feed: {error:#}");
                                    return;
                                }
                            }
                        }
                        tokio::task::yield_now().await;
                    }
                };
                tokio::select! { _ = process => {}, _ = client.watch_module_host() => {} }
                client.disconnect().await;
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
                        "SELECT * FROM motion".into(),
                        "SELECT * FROM visible_entity".into(),
                    ]
                    .into_boxed_slice(),
                },
                Instant::now(),
            )
            .await?;
        ready_rx.await?;
        Ok::<_, anyhow::Error>(feed)
    })
    .await
    .cloned()
}
