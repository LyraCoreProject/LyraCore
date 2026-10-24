use anyhow::{bail, ensure, Context, Result};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use wow_srp::normalized_string::NormalizedString;
use wow_srp::vanilla_header::{DecrypterHalf, EncrypterHalf, HeaderCrypto, ProofSeed};

pub const BUILD: u32 = 5875;
pub const HEARTBEAT: u16 = 0x00ee;
pub const CREATE: u16 = 0x00a9;
pub const DESTROY: u16 = 0x00aa;
pub const AUTH_RESPONSE: u16 = 0x01ee;
pub const PING: u16 = 0x01dc;
pub const PONG: u16 = 0x01dd;
pub const SESSION_KEY: [u8; 40] = [0x57; 40];

#[derive(Clone, Debug)]
pub struct Motion {
    pub guid: u64,
    pub cell: i64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub tick: u32,
    pub sequence: u64,
    pub hidden: bool,
    pub ghost_only: bool,
}

impl Motion {
    pub fn from_bsatn(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() == 42,
            "unexpected Motion row length {}",
            bytes.len()
        );
        Ok(Self {
            guid: u64::from_le_bytes(bytes[0..8].try_into()?),
            cell: i64::from_le_bytes(bytes[8..16].try_into()?),
            x: f32::from_le_bytes(bytes[16..20].try_into()?),
            y: f32::from_le_bytes(bytes[20..24].try_into()?),
            z: f32::from_le_bytes(bytes[24..28].try_into()?),
            tick: u32::from_le_bytes(bytes[28..32].try_into()?),
            sequence: u64::from_le_bytes(bytes[32..40].try_into()?),
            hidden: bytes[40] != 0,
            ghost_only: bytes[41] != 0,
        })
    }

    pub fn movement_body(&self, include_guid: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(37);
        if include_guid {
            pack_guid(&mut out, self.guid);
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&self.tick.to_le_bytes());
        for value in [self.x, self.y, self.z, 0.0, 0.0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }
}

pub fn now_ms() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32
}

pub fn pack_guid(out: &mut Vec<u8>, guid: u64) {
    let bytes = guid.to_le_bytes();
    let mask = bytes
        .iter()
        .enumerate()
        .fold(0u8, |mask, (i, b)| mask | (u8::from(*b != 0) << i));
    out.push(mask);
    out.extend(bytes.into_iter().filter(|b| *b != 0));
}

pub fn unpack_guid(body: &[u8]) -> Result<(u64, usize)> {
    let mask = *body.first().context("missing packed guid")?;
    let mut used = 1;
    let mut bytes = [0u8; 8];
    for (i, b) in bytes.iter_mut().enumerate() {
        if mask & (1 << i) != 0 {
            *b = *body.get(used).context("truncated packed guid")?;
            used += 1;
        }
    }
    Ok((u64::from_le_bytes(bytes), used))
}

fn create_body(row: &Motion) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(0);
    out.push(2);
    pack_guid(&mut out, row.guid);
    out.push(3);
    out.push(0x40);
    for value in [row.x, row.y, row.z, 0.0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.push(1);
    out.extend_from_slice(&0x17u32.to_le_bytes());
    out.extend_from_slice(&row.guid.to_le_bytes());
    out.extend_from_slice(&9u32.to_le_bytes());
    out.extend_from_slice(&1.0f32.to_le_bytes());
    out
}

pub fn append_server_packet(
    out: &mut Vec<u8>,
    cipher: &mut EncrypterHalf,
    opcode: u16,
    body: &[u8],
) -> Result<()> {
    let size = u16::try_from(body.len() + 2).context("oversized world packet")?;
    out.extend_from_slice(&cipher.encrypt_server_header(size, opcode));
    out.extend_from_slice(body);
    Ok(())
}

pub async fn read_client_packet(
    reader: &mut (impl AsyncRead + Unpin),
    cipher: &mut DecrypterHalf,
) -> Result<(u32, Vec<u8>)> {
    let mut header = [0; 6];
    reader.read_exact(&mut header).await?;
    let header = cipher.decrypt_client_header(header);
    ensure!(
        header.size >= 4 && header.size <= 4096,
        "invalid client packet size"
    );
    let mut body = vec![0; header.size as usize - 4];
    reader.read_exact(&mut body).await?;
    Ok((header.opcode, body))
}

pub async fn read_server_packet(
    reader: &mut (impl AsyncRead + Unpin),
    cipher: &mut DecrypterHalf,
) -> Result<(u16, Vec<u8>)> {
    let mut header = [0; 4];
    reader.read_exact(&mut header).await?;
    let header = cipher.decrypt_server_header(header);
    ensure!(header.size >= 2, "invalid world packet size");
    let mut body = vec![0; header.size as usize - 2];
    reader.read_exact(&mut body).await?;
    Ok((header.opcode, body))
}

pub async fn write_client_packet(
    writer: &mut (impl AsyncWrite + Unpin),
    cipher: &mut EncrypterHalf,
    opcode: u32,
    body: &[u8],
) -> Result<()> {
    let size = u16::try_from(body.len() + 4)?;
    let mut out = Vec::with_capacity(body.len() + 6);
    out.extend_from_slice(&cipher.encrypt_client_header(size, opcode));
    out.extend_from_slice(body);
    writer.write_all(&out).await?;
    Ok(())
}

pub async fn server_auth(socket: &mut TcpStream) -> Result<(u64, HeaderCrypto)> {
    ensure!(
        socket.peer_addr()?.ip().is_loopback(),
        "experiment accepts loopback clients only"
    );
    socket.set_nodelay(true)?;
    let seed = ProofSeed::new();
    let mut challenge = vec![0, 6, 0xec, 1];
    challenge.extend_from_slice(&seed.seed().to_le_bytes());
    socket.write_all(&challenge).await?;
    let mut header = [0; 6];
    socket.read_exact(&mut header).await?;
    let size = u16::from_be_bytes(header[..2].try_into()?);
    ensure!(
        u32::from_le_bytes(header[2..].try_into()?) == 0x1ed,
        "expected CMSG_AUTH_SESSION"
    );
    ensure!((36..=128).contains(&size), "invalid auth packet size");
    let mut body = vec![0; size as usize - 4];
    socket.read_exact(&mut body).await?;
    ensure!(
        u32::from_le_bytes(body[..4].try_into()?) == BUILD,
        "expected build 5875"
    );
    let end = 8 + body[8..]
        .iter()
        .position(|b| *b == 0)
        .context("missing account name")?;
    ensure!(body.len() == end + 29, "invalid auth body length");
    let account = std::str::from_utf8(&body[8..end])?;
    let guid: u64 = account
        .strip_prefix('E')
        .context("expected experiment Account")?
        .parse()?;
    ensure!((1..=5000).contains(&guid), "invalid experiment Character");
    let username = NormalizedString::new(account)?;
    let client_seed = u32::from_le_bytes(body[end + 1..end + 5].try_into()?);
    let proof = body[end + 5..end + 25].try_into()?;
    let crypto = seed
        .into_server_header_crypto(&username, SESSION_KEY, proof, client_seed)
        .map_err(|_| anyhow::anyhow!("world authentication failed"))?;
    Ok((guid, crypto))
}

pub async fn client_auth(socket: &mut TcpStream, guid: u64) -> Result<HeaderCrypto> {
    ensure!(
        socket.peer_addr()?.ip().is_loopback(),
        "experiment accepts loopback endpoints only"
    );
    socket.set_nodelay(true)?;
    let mut challenge = [0; 8];
    socket.read_exact(&mut challenge).await?;
    ensure!(challenge[..4] == [0, 6, 0xec, 1], "invalid challenge");
    let server_seed = u32::from_le_bytes(challenge[4..].try_into()?);
    let username = NormalizedString::new(format!("E{guid}"))?;
    let seed = ProofSeed::new();
    let client_seed = seed.seed();
    let (proof, crypto) = seed.into_client_header_crypto(&username, SESSION_KEY, server_seed);
    let mut body = Vec::new();
    body.extend_from_slice(&BUILD.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(username.as_ref().as_bytes());
    body.push(0);
    body.extend_from_slice(&client_seed.to_le_bytes());
    body.extend_from_slice(&proof);
    body.extend_from_slice(&0u32.to_le_bytes());
    let mut out = Vec::new();
    out.extend_from_slice(&((body.len() + 4) as u16).to_be_bytes());
    out.extend_from_slice(&0x1edu32.to_le_bytes());
    out.extend_from_slice(&body);
    socket.write_all(&out).await?;
    Ok(crypto)
}

pub fn movement_args(body: &[u8]) -> Result<(f32, f32, f32, u32)> {
    ensure!(
        body.len() == 28 && body[..4] == [0; 4],
        "experiment supports plain movement only"
    );
    Ok((
        f32::from_le_bytes(body[8..12].try_into()?),
        f32::from_le_bytes(body[12..16].try_into()?),
        f32::from_le_bytes(body[16..20].try_into()?),
        u32::from_le_bytes(body[4..8].try_into()?),
    ))
}

#[derive(Default)]
pub struct Projection {
    pub created: HashSet<u64>,
}

impl Projection {
    pub fn update(
        &mut self,
        inserts: Vec<Motion>,
        deletes: Vec<Motion>,
        cipher: &mut EncrypterHalf,
    ) -> Result<Vec<u8>> {
        let retained: HashSet<u64> = inserts.iter().map(|m| m.guid).collect();
        let mut out = Vec::new();
        for old in deletes {
            if !retained.contains(&old.guid) && self.created.remove(&old.guid) {
                append_server_packet(&mut out, cipher, DESTROY, &old.guid.to_le_bytes())?;
            }
        }
        for row in inserts {
            if self.created.insert(row.guid) {
                append_server_packet(&mut out, cipher, CREATE, &create_body(&row))?;
            }
            append_server_packet(&mut out, cipher, HEARTBEAT, &row.movement_body(true))?;
        }
        Ok(out)
    }
}

#[derive(Default)]
pub struct Observed {
    pub entities: HashMap<u64, (f32, f32, f32)>,
    pub creates: u64,
    pub destroys: u64,
    pub movements: u64,
    pub bytes: u64,
    pub latency_ms: HashMap<u32, u64>,
    pub newest_tick: HashMap<u64, u32>,
}

impl Observed {
    pub fn receive(&mut self, opcode: u16, body: &[u8], measure_after: u32) -> Result<()> {
        self.bytes += body.len() as u64 + 4;
        match opcode {
            CREATE => {
                ensure!(
                    body.len() >= 6 && body[..6] == [1, 0, 0, 0, 0, 2],
                    "unexpected create block"
                );
                let (guid, _) = unpack_guid(&body[6..])?;
                ensure!(
                    self.entities.insert(guid, (0.0, 0.0, 0.0)).is_none(),
                    "duplicate CREATE for {guid}"
                );
                self.creates += 1;
            }
            DESTROY => {
                ensure!(body.len() == 8, "invalid destroy packet");
                let guid = u64::from_le_bytes(body.try_into()?);
                ensure!(
                    self.entities.remove(&guid).is_some(),
                    "DESTROY before CREATE for {guid}"
                );
                self.destroys += 1;
            }
            HEARTBEAT => {
                let (guid, size) = unpack_guid(body)?;
                let (x, y, z, tick) = movement_args(&body[size..])?;
                let position = self
                    .entities
                    .get_mut(&guid)
                    .with_context(|| format!("movement before CREATE for {guid}"))?;
                *position = (x, y, z);
                if tick != 0 && tick.wrapping_sub(measure_after) < 0x80000000 {
                    if let Some(previous) = self.newest_tick.insert(guid, tick) {
                        ensure!(
                            tick.wrapping_sub(previous) < 0x80000000,
                            "movement reordered for {guid}"
                        );
                    }
                    self.movements += 1;
                    *self
                        .latency_ms
                        .entry(now_ms().wrapping_sub(tick).min(60_000))
                        .or_default() += 1;
                }
            }
            AUTH_RESPONSE => ensure!(body.first() == Some(&0x0c), "authentication refused"),
            PONG => {}
            _ => bail!("unexpected opcode {opcode:#x}"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::Object;

    fn row() -> Motion {
        Motion {
            guid: 0xf130001234000001,
            cell: 0,
            x: 21.0,
            y: -7.5,
            z: 1.0,
            tick: 123,
            sequence: 1,
            hidden: false,
            ghost_only: false,
        }
    }

    fn decode(opcode: u16, body: &[u8]) -> ServerOpcodeMessage {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
        bytes.extend_from_slice(&opcode.to_le_bytes());
        bytes.extend_from_slice(body);
        ServerOpcodeMessage::read_unencrypted(bytes.as_slice()).unwrap()
    }

    #[test]
    fn vanilla_decoder_accepts_entity_creation() {
        let row = row();
        let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) = decode(CREATE, &create_body(&row))
        else {
            panic!("wrong opcode")
        };
        assert_eq!(update.objects.len(), 1);
        let Object::CreateObject { guid3, .. } = &update.objects[0] else {
            panic!("wrong update")
        };
        assert_eq!(guid3.guid(), row.guid);
    }

    #[test]
    fn vanilla_decoder_accepts_movement() {
        let row = row();
        let ServerOpcodeMessage::MSG_MOVE_HEARTBEAT(movement) =
            decode(HEARTBEAT, &row.movement_body(true))
        else {
            panic!("wrong opcode")
        };
        assert_eq!(movement.guid.guid(), row.guid);
        assert_eq!(movement.info.position.x, row.x);
        assert_eq!(movement.info.position.y, row.y);
    }

    #[test]
    fn vanilla_decoder_accepts_destroy() {
        let ServerOpcodeMessage::SMSG_DESTROY_OBJECT(destroy) =
            decode(DESTROY, &row().guid.to_le_bytes())
        else {
            panic!("wrong opcode")
        };
        assert_eq!(destroy.guid.guid(), row().guid);
    }
}
