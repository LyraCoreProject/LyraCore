use crate::StandaloneEnv;
use anyhow::{ensure, Context, Result};
use spacetimedb::client::{ClientActorId, ClientConfig, ClientConnection, Protocol, WsVersion};
use spacetimedb::host::{FunctionArgs, ReducerCallError, ReducerOutcome};
use spacetimedb_auth::identity::ConnectionAuthCtx;
use spacetimedb_client_api::{ControlStateReadAccess, NodeDelegate};
use spacetimedb_client_api_messages::websocket::v1;
use spacetimedb_lib::{bsatn, identity::AuthCtx, ser::Serialize, ConnectionId, Identity};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use wow_srp::vanilla_header::ProofSeed;

#[path = "native_feed.rs"]
mod feed;
#[path = "native_wire.rs"]
mod wire;

#[derive(Clone, Copy, Debug)]
enum Listener {
    World,
    Logon,
}

pub async fn start(ctx: Arc<StandaloneEnv>) -> Result<()> {
    let mut listeners = Vec::new();
    for (kind, variable) in [
        (Listener::World, "LYRACORE_NATIVE_WORLD_ADDR"),
        (Listener::Logon, "LYRACORE_NATIVE_LOGON_ADDR"),
    ] {
        if let Ok(address) = std::env::var(variable) {
            let address: SocketAddr = address.parse()?;
            let database = std::env::var("LYRACORE_NATIVE_DATABASE")?;
            listeners.push((kind, TcpListener::bind(address).await?, database));
        }
    }
    for (kind, listener, database) in listeners {
        accept(ctx.clone(), listener, database, kind);
    }
    Ok(())
}

fn accept(ctx: Arc<StandaloneEnv>, listener: TcpListener, database: String, kind: Listener) {
    let slots = Arc::new(tokio::sync::Semaphore::new(4096));
    tokio::spawn(async move {
        let mut retry = Duration::from_millis(100);
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(pair) => {
                    retry = Duration::from_millis(100);
                    pair
                }
                Err(error) => {
                    log::warn!("native {kind:?} accept: {error}; retrying in {retry:?}");
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(Duration::from_secs(2));
                    continue;
                }
            };
            let Ok(slot) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let (ctx, database) = (ctx.clone(), database.clone());
            tokio::spawn(async move {
                if let Err(error) = connect(ctx, socket, &database, kind, slot).await {
                    log::debug!("native {kind:?} connection: {error:#}");
                }
            });
        }
    });
}

fn config() -> ClientConfig {
    ClientConfig {
        protocol: Protocol::Binary,
        version: WsVersion::V2,
        compression: v1::Compression::None,
        tx_update_full: false,
        confirmed_reads: true,
    }
}

fn new_connection_id() -> ConnectionId {
    ConnectionId::from_le_byte_array(entropy())
}

fn entropy<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    for part in bytes.chunks_exact_mut(4) {
        part.copy_from_slice(&ProofSeed::new().seed().to_le_bytes());
    }
    bytes
}

// Core can abort a slow subscriber without polling its future to completion.
struct ClientLifetime(ClientConnection);

impl Drop for ClientLifetime {
    fn drop(&mut self) {
        let client = self.0.clone();
        tokio::spawn(async move { client.disconnect().await });
    }
}

/// A native connection's attachment to one Shard. Dropping it disconnects the client from that Shard.
struct Shard {
    client: ClientConnection,
    deliveries: mpsc::Receiver<feed::Delivery>,
    closed: oneshot::Receiver<()>,
    _stop: oneshot::Sender<()>,
    _registration: feed::Registration,
}

async fn attach(
    ctx: &Arc<StandaloneEnv>,
    database: Identity,
    connection_id: ConnectionId,
) -> Result<Shard> {
    let db = ctx
        .get_database_by_identity(&database)
        .await?
        .context("native database missing")?;
    let feed = feed::shared(ctx.clone(), database, db.owner_identity).await?;
    let leader = ctx.leader(db.id).await?;
    let mut module_rx = leader.module_watcher().await?;
    let id = ClientActorId {
        identity: db.owner_identity,
        connection_id,
        name: ctx.client_actor_index().next_client_name(),
    };
    let (registration, deliveries) = feed.register(connection_id)?;
    let auth = ConnectionAuthCtx {
        identity: id.identity,
        jwt_payload: None,
    };
    let connected =
        ClientConnection::call_client_connected_maybe_reject(&mut module_rx, id, auth.clone())
            .await?;
    let (stop, stopped) = oneshot::channel::<()>();
    let (closing, closed) = oneshot::channel::<()>();
    let client = ClientConnection::spawn(
        id,
        auth,
        AuthCtx::new(db.owner_identity, db.owner_identity),
        config(),
        leader.replica_id,
        module_rx,
        move |mut client, mut receiver| async move {
            let _closing = closing;
            let _lifetime = ClientLifetime(client.clone());
            let drain = async move {
                let mut messages = Vec::with_capacity(32);
                while receiver.recv_many(&mut messages, 32).await != 0 {
                    messages.clear();
                    tokio::task::yield_now().await;
                }
            };
            tokio::select! {
                _ = drain => {},
                _ = stopped => {},
                _ = client.watch_module_host() => {},
            }
        },
        connected,
    )
    .await;
    Ok(Shard {
        client,
        deliveries,
        closed,
        _stop: stop,
        _registration: registration,
    })
}

async fn connect(
    ctx: Arc<StandaloneEnv>,
    socket: TcpStream,
    database: &str,
    kind: Listener,
    slot: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let _slot = slot;
    socket.set_nodelay(true)?;
    let database = ctx
        .lookup_database_identity(database)
        .await?
        .context("native database missing")?;
    let connection_id = new_connection_id();
    let shard = attach(&ctx, database, connection_id).await?;
    let served = match kind {
        Listener::World => serve(&ctx, shard, connection_id, socket).await,
        Listener::Logon => serve_logon(shard, socket).await,
    };
    if let Err(error) = served {
        log::debug!("native {kind:?} transport: {error:#}");
    }
    Ok(())
}

async fn request(
    client: &ClientConnection,
    reducer: &str,
    arguments: &impl Serialize,
) -> Result<()> {
    let Err(error) = call(client, reducer, arguments).await?.into_result() else {
        return Ok(());
    };
    let reason = error.to_string();
    let Some(reply) = reason.strip_prefix("native_reply:") else {
        return Err(error);
    };
    let packets: Vec<(u16, Vec<u8>)> = serde_json::from_str(reply)?;
    ensure!(
        reducer == "native_world_receive",
        "native rollback reply outside a world request"
    );
    call(client, "native_world_reply", &(packets,))
        .await?
        .into_result()
}

// Arguments must match the reducer parameters' order and BSATN layout; trailing bytes go unchecked.
async fn call(
    client: &ClientConnection,
    reducer: &str,
    arguments: &impl Serialize,
) -> Result<ReducerOutcome> {
    let arguments = FunctionArgs::Bsatn(bsatn::to_vec(arguments)?.into());
    let timer = Instant::now();
    let invoke = || {
        client.call_reducer(
            reducer,
            arguments.clone(),
            0,
            timer,
            v1::CallReducerFlags::NoSuccessNotify,
        )
    };
    let result = match invoke().await {
        // NoSuchModule means the replaced Module refused dispatch before the request could execute.
        Err(ReducerCallError::NoSuchModule(_)) => invoke().await,
        result => result,
    };
    Ok(result?.outcome)
}

async fn serve(
    ctx: &Arc<StandaloneEnv>,
    mut shard: Shard,
    connection_id: ConnectionId,
    mut socket: TcpStream,
) -> Result<()> {
    let seed = ProofSeed::new();
    let (client, deliveries, closed) = (&shard.client, &mut shard.deliveries, &mut shard.closed);
    let handshake = async {
        request(client, "native_world_open", &(seed.seed(),)).await?;
        let challenge = deliveries.recv().await.context("native feed closed")?;
        wire::write_plain(&mut socket, challenge).await?;
        let (opcode, body) = wire::read_client(&mut socket, None).await?;
        request(client, "native_world_receive", &(opcode, body)).await?;
        let reply = deliveries.recv().await.context("native feed closed")?;
        let Some(cipher) = reply.cipher else {
            wire::write_plain(&mut socket, reply).await?;
            anyhow::bail!("Module did not authorize world encryption");
        };
        let username = wow_srp::normalized_string::NormalizedString::new(&cipher.username)?;
        let key = cipher
            .session_key
            .as_slice()
            .try_into()
            .context("invalid native cipher key")?;
        let proof = cipher
            .client_proof
            .as_slice()
            .try_into()
            .context("invalid native cipher proof")?;
        let crypto = seed
            .into_server_header_crypto(&username, key, proof, cipher.client_seed)
            .map_err(|_| anyhow::anyhow!("Module cipher does not match the connection proof"))?;
        let (mut encrypt, decrypt) = crypto.split();
        wire::write_encrypted(&mut socket, &mut encrypt, reply.packets).await?;
        Ok::<_, anyhow::Error>((encrypt, decrypt, cipher))
    };
    let (mut encrypt, mut decrypt, established) = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(10), handshake) => result??,
        _ = closed => return Ok(()),
    };
    let (mut reader, mut writer) = socket.into_split();
    let (activity, mut activity_rx) = watch::channel(tokio::time::Instant::now());
    // A request that was in flight to the Shard before a Handoff may fail once that Shard lets go.
    let (current, current_rx) = watch::channel((0u64, shard.client.clone()));
    let input_activity = activity.clone();
    let input = async move {
        loop {
            let (opcode, body) = wire::read_client(&mut reader, Some(&mut decrypt)).await?;
            input_activity.send_replace(tokio::time::Instant::now());
            let (generation, client) = current_rx.borrow().clone();
            let result = request(&client, "native_world_receive", &(opcode, body)).await;
            if result.is_err() && current_rx.borrow().0 == generation {
                return result;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let output = async move {
        let mut generation = 0;
        let mut adopting = false;
        loop {
            let delivery = tokio::select! {
                delivery = shard.deliveries.recv() => delivery,
                _ = &mut shard.closed => None,
            };
            let Some(delivery) = delivery else {
                return Ok(());
            };
            // The target of a Handoff records the cipher it adopts; it must be the one in use.
            if let Some(cipher) = delivery.cipher {
                ensure!(
                    std::mem::take(&mut adopting) && cipher == established,
                    "native cipher changed after authentication"
                );
            }
            let mut packets = delivery.packets;
            let handoff = packets.iter().position(|packet| packet.handoff().is_some());
            let target = handoff.and_then(|at| packets[at].handoff());
            // Packets after the marker belong to the World Session this Shard no longer holds.
            packets.truncate(handoff.unwrap_or(packets.len()));
            wire::write_encrypted(&mut writer, &mut encrypt, packets).await?;
            activity.send_replace(tokio::time::Instant::now());
            if let Some(database) = target {
                let next = attach(ctx, database, connection_id).await?;
                request(&next.client, "native_world_adopt", &()).await?;
                generation += 1;
                current.send_replace((generation, next.client.clone()));
                shard = next;
                adopting = true;
            }
        }
    };
    let idle = async move {
        loop {
            let deadline = *activity_rx.borrow_and_update() + Duration::from_secs(90);
            tokio::time::timeout_at(deadline, activity_rx.changed()).await??;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! { result = input => result, result = output => result, result = idle => result }
}

async fn serve_logon(shard: Shard, mut socket: TcpStream) -> Result<()> {
    let Shard {
        client,
        mut deliveries,
        mut closed,
        _stop: _attached,
        _registration: _registered,
    } = shard;
    request(
        &client,
        "native_logon_open",
        &(socket.peer_addr()?.ip().to_string(),),
    )
    .await?;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut authenticated = false;
    loop {
        if authenticated {
            deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        }
        let exchange = async {
            let frame = wire::read_logon(&mut socket).await?;
            request(&client, "native_logon_receive", &(frame, entropy::<32>())).await?;
            let delivery = deliveries.recv().await.context("native feed closed")?;
            wire::write_logon(&mut socket, delivery).await
        };
        authenticated |= tokio::select! {
            result = tokio::time::timeout_at(deadline, exchange) => result??,
            _ = &mut closed => return Ok(()),
        };
    }
}
