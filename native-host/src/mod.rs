use crate::StandaloneEnv;
use anyhow::{ensure, Context, Result};
use spacetimedb::client::{
    ClientActorId, ClientConfig, ClientConnection, ClientConnectionReceiver, Protocol, WsVersion,
};
use spacetimedb::host::FunctionArgs;
use spacetimedb_auth::identity::ConnectionAuthCtx;
use spacetimedb_client_api::{ControlStateReadAccess, NodeDelegate};
use spacetimedb_client_api_messages::websocket::v1;
use spacetimedb_lib::{identity::AuthCtx, ConnectionId};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
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

async fn connect(
    ctx: Arc<StandaloneEnv>,
    socket: TcpStream,
    database: &str,
    kind: Listener,
    slot: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    socket.set_nodelay(true)?;
    let database = ctx
        .lookup_database_identity(database)
        .await?
        .context("native database missing")?;
    let db = ctx
        .get_database_by_identity(&database)
        .await?
        .context("native database missing")?;
    let feed = feed::shared(ctx.clone(), database, db.owner_identity).await?;
    let leader = ctx.leader(db.id).await?;
    let mut module_rx = leader.module_watcher().await?;
    let id = ClientActorId {
        identity: db.owner_identity,
        connection_id: new_connection_id(),
        name: ctx.client_actor_index().next_client_name(),
    };
    let (registration, deliveries) = feed.register(id.connection_id)?;
    let auth = ConnectionAuthCtx {
        identity: id.identity,
        jwt_payload: None,
    };
    let connected =
        ClientConnection::call_client_connected_maybe_reject(&mut module_rx, id, auth.clone())
            .await?;
    ClientConnection::spawn(
        id,
        auth,
        AuthCtx::new(db.owner_identity, db.owner_identity),
        config(),
        leader.replica_id,
        module_rx,
        move |mut client, receiver| async move {
            let _slot = slot;
            let _lifetime = ClientLifetime(client.clone());
            let _registration = registration;
            let transport_client = client.clone();
            let serving = async move {
                match kind {
                    Listener::World => serve(transport_client, receiver, deliveries, socket).await,
                    Listener::Logon => {
                        serve_logon(transport_client, receiver, deliveries, socket).await
                    }
                }
            };
            tokio::select! {
                result = serving => if let Err(error) = result {
                    log::debug!("native {kind:?} transport: {error:#}");
                },
                _ = client.watch_module_host() => {}
            }
        },
        connected,
    )
    .await;
    Ok(())
}

async fn request(
    client: &ClientConnection,
    name: &str,
    arguments: serde_json::Value,
) -> Result<()> {
    let result = client
        .call_reducer(
            name,
            FunctionArgs::Json(arguments.to_string().into()),
            0,
            Instant::now(),
            v1::CallReducerFlags::NoSuccessNotify,
        )
        .await?
        .outcome
        .into_result();
    if let Err(error) = result {
        let reason = error.to_string();
        let Some(reply) = reason.strip_prefix("native_reply:") else {
            return Err(error);
        };
        let packets: Vec<(u16, Vec<u8>)> = serde_json::from_str(reply)?;
        let packets: Vec<_> = packets
            .into_iter()
            .map(|(opcode, body)| serde_json::json!({"opcode": opcode, "body": body}))
            .collect();
        ensure!(
            name == "native_world_receive",
            "native rollback reply outside a world request"
        );
        client
            .call_reducer(
                "native_world_reply",
                FunctionArgs::Json(serde_json::json!([packets]).to_string().into()),
                0,
                Instant::now(),
                v1::CallReducerFlags::NoSuccessNotify,
            )
            .await?
            .outcome
            .into_result()?;
    }
    Ok(())
}

async fn serve(
    client: ClientConnection,
    mut receiver: ClientConnectionReceiver,
    mut deliveries: mpsc::Receiver<feed::Delivery>,
    mut socket: TcpStream,
) -> Result<()> {
    let drain = async move {
        let mut messages = Vec::with_capacity(32);
        while receiver.recv_many(&mut messages, 32).await != 0 {
            messages.clear();
            tokio::task::yield_now().await;
        }
    };
    let transport = async move {
        let seed = ProofSeed::new();
        let handshake = async {
            request(
                &client,
                "native_world_open",
                serde_json::json!([seed.seed()]),
            )
            .await?;
            let challenge = deliveries.recv().await.context("native feed closed")?;
            wire::write_plain(&mut socket, challenge).await?;
            let (opcode, body) = wire::read_client(&mut socket, None).await?;
            request(
                &client,
                "native_world_receive",
                serde_json::json!([opcode, body]),
            )
            .await?;
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
                .map_err(|_| {
                    anyhow::anyhow!("Module cipher does not match the connection proof")
                })?;
            let (mut encrypt, decrypt) = crypto.split();
            wire::write_encrypted(&mut socket, &mut encrypt, reply.packets).await?;
            Ok::<_, anyhow::Error>((encrypt, decrypt))
        };
        let (mut encrypt, mut decrypt) =
            tokio::time::timeout(Duration::from_secs(10), handshake).await??;
        let (mut reader, mut writer) = socket.into_split();
        let (activity, mut activity_rx) = watch::channel(tokio::time::Instant::now());
        let input_activity = activity.clone();
        let input = async move {
            loop {
                let (opcode, body) = wire::read_client(&mut reader, Some(&mut decrypt)).await?;
                input_activity.send_replace(tokio::time::Instant::now());
                request(
                    &client,
                    "native_world_receive",
                    serde_json::json!([opcode, body]),
                )
                .await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let output = async move {
            while let Some(delivery) = deliveries.recv().await {
                ensure!(
                    delivery.cipher.is_none(),
                    "native cipher changed after authentication"
                );
                wire::write_encrypted(&mut writer, &mut encrypt, delivery.packets).await?;
                activity.send_replace(tokio::time::Instant::now());
            }
            Ok::<(), anyhow::Error>(())
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
    };
    tokio::select! { result = transport => result, _ = drain => Ok(()) }
}

async fn serve_logon(
    client: ClientConnection,
    mut receiver: ClientConnectionReceiver,
    mut deliveries: mpsc::Receiver<feed::Delivery>,
    mut socket: TcpStream,
) -> Result<()> {
    let drain = async move {
        let mut messages = Vec::with_capacity(32);
        while receiver.recv_many(&mut messages, 32).await != 0 {
            messages.clear();
            tokio::task::yield_now().await;
        }
    };
    let transport = async move {
        request(
            &client,
            "native_logon_open",
            serde_json::json!([socket.peer_addr()?.ip().to_string()]),
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
                request(
                    &client,
                    "native_logon_receive",
                    serde_json::json!([frame, entropy::<32>()]),
                )
                .await?;
                let delivery = deliveries.recv().await.context("native feed closed")?;
                wire::write_logon(&mut socket, delivery).await
            };
            authenticated |= tokio::time::timeout_at(deadline, exchange).await??;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! { result = transport => result, _ = drain => Ok(()) }
}
