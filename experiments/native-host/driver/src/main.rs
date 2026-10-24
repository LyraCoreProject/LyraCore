use anyhow::{ensure, Context, Result};
use native_host_experiment_wire as wire;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{tcp::OwnedWriteHalf, TcpStream};
use tokio::sync::mpsc;
use wow_srp::vanilla_header::EncrypterHalf;

struct Client {
    guid: u64,
    writer: OwnedWriteHalf,
    encrypt: EncrypterHalf,
    observed: Arc<Mutex<wire::Observed>>,
}

async fn connect(
    address: String,
    guid: u64,
    expected: usize,
    after: Arc<AtomicU32>,
    failures: mpsc::UnboundedSender<String>,
) -> Result<Client> {
    let mut socket = TcpStream::connect(address).await?;
    let crypto = wire::client_auth(&mut socket, guid).await?;
    let (encrypt, mut decrypt) = crypto.split();
    let (mut reader, writer) = socket.into_split();
    let mut initial = wire::Observed::default();
    while initial.entities.len() < expected || !initial.entities.contains_key(&guid) {
        let (opcode, body) = wire::read_server_packet(&mut reader, &mut decrypt).await?;
        initial.receive(opcode, &body, after.load(Ordering::Relaxed))?;
    }
    let observed = Arc::new(Mutex::new(initial));
    let seen = observed.clone();
    tokio::spawn(async move {
        let result = async {
            loop {
                let (opcode, body) = wire::read_server_packet(&mut reader, &mut decrypt).await?;
                seen.lock()
                    .unwrap()
                    .receive(opcode, &body, after.load(Ordering::Relaxed))?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            let _ = failures.send(format!("Character {guid}: {error:#}"));
        }
    });
    Ok(Client {
        guid,
        writer,
        encrypt,
        observed,
    })
}

fn stats(clients: &[Client], details: bool) -> Value {
    let (mut creates, mut destroys, mut movements, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let mut hist = BTreeMap::<u32, u64>::new();
    let mut entities = BTreeMap::new();
    for client in clients {
        let seen = client.observed.lock().unwrap();
        creates += seen.creates;
        destroys += seen.destroys;
        movements += seen.movements;
        bytes += seen.bytes;
        for (ms, n) in &seen.latency_ms {
            *hist.entry(*ms).or_default() += n;
        }
        if details {
            entities.insert(client.guid, seen.entities.clone());
        }
    }
    let percentile = |fraction: f64| {
        let threshold = ((movements as f64 * fraction).ceil() as u64).max(1);
        let mut cumulative = 0;
        hist.iter().find_map(|(ms, n)| {
            cumulative += n;
            (cumulative >= threshold).then_some(*ms)
        })
    };
    json!({"clients":clients.len(),"creates":creates,"destroys":destroys,"movements":movements,"bytes":bytes,
        "p50_ms":percentile(0.50),"p95_ms":percentile(0.95),"p99_ms":percentile(0.99),"max_ms":hist.last_key_value().map(|(ms,_)|ms),
        "histogram_ms":hist,"entities":entities})
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "usage: driver 127.0.0.1:port count dense|spread"
    );
    let count: usize = args[2].parse()?;
    ensure!((1..=5000).contains(&count), "count must be 1..5000");
    let dense = match args[3].as_str() {
        "dense" => true,
        "spread" => false,
        _ => anyhow::bail!("expected dense or spread"),
    };
    let after = Arc::new(AtomicU32::new(wire::now_ms()));
    let (failure_tx, mut failure_rx) = mpsc::unbounded_channel();
    let mut clients = Vec::new();
    for chunk in (1..=count).collect::<Vec<_>>().chunks(16) {
        let mut pending = tokio::task::JoinSet::new();
        for &guid in chunk {
            let expected = if dense {
                count
            } else {
                32.min(count - (guid - 1) / 32 * 32)
            };
            pending.spawn(tokio::time::timeout(
                std::time::Duration::from_secs(60),
                connect(
                    args[1].clone(),
                    guid as u64,
                    expected,
                    after.clone(),
                    failure_tx.clone(),
                ),
            ));
        }
        while let Some(result) = pending.join_next().await {
            clients.push(result???);
        }
    }
    clients.sort_by_key(|c| c.guid);
    println!("{}", json!({"ready":clients.len()}));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = tokio::select! {
            failure = failure_rx.recv() => anyhow::bail!("{}", failure.context("reader channel closed")?),
            line = lines.next_line() => match line? { Some(line) => line, None => break },
        };
        let command: Value = serde_json::from_str(&line)?;
        match command["op"].as_str().context("missing op")? {
            "stats" | "snapshot" => println!("{}", stats(&clients, command["op"] == "snapshot")),
            "reset" => {
                let tick = wire::now_ms();
                after.store(tick, Ordering::Relaxed);
                for client in &clients {
                    let mut seen = client.observed.lock().unwrap();
                    seen.creates = 0;
                    seen.destroys = 0;
                    seen.movements = 0;
                    seen.bytes = 0;
                    seen.latency_ms.clear();
                    seen.newest_tick.clear();
                }
                println!("{}", json!({"reset_tick":tick}));
            }
            "tick" => {
                let tick = u32::try_from(command["tick"].as_u64().context("missing tick")?)?;
                for client in &mut clients {
                    let (x, y, z) = *client
                        .observed
                        .lock()
                        .unwrap()
                        .entities
                        .get(&client.guid)
                        .context("Character missing from own AOI")?;
                    let row = wire::Motion {
                        guid: client.guid,
                        cell: 0,
                        x,
                        y,
                        z: if z == 0.0 { 1.0 } else { 0.0 },
                        tick,
                        sequence: 0,
                        hidden: false,
                        ghost_only: false,
                    };
                    wire::write_client_packet(
                        &mut client.writer,
                        &mut client.encrypt,
                        wire::HEARTBEAT as u32,
                        &row.movement_body(false),
                    )
                    .await?;
                }
                println!("{}", json!({"sent":clients.len()}));
            }
            "move" => {
                let guid = command["guid"].as_u64().context("missing guid")?;
                let client = clients
                    .iter_mut()
                    .find(|c| c.guid == guid)
                    .context("unknown Character")?;
                let row = wire::Motion {
                    guid,
                    cell: 0,
                    x: command["x"].as_f64().context("missing x")? as f32,
                    y: command["y"].as_f64().context("missing y")? as f32,
                    z: 0.0,
                    tick: wire::now_ms(),
                    sequence: 0,
                    hidden: false,
                    ghost_only: false,
                };
                wire::write_client_packet(
                    &mut client.writer,
                    &mut client.encrypt,
                    wire::HEARTBEAT as u32,
                    &row.movement_body(false),
                )
                .await?;
                println!("{}", json!({"sent":guid}));
            }
            "stop" => {
                println!("{}", stats(&clients, false));
                break;
            }
            op => anyhow::bail!("unknown command {op}"),
        }
    }
    Ok(())
}
