use anyhow::{ensure, Context, Result};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wow_srp::vanilla_header::{DecrypterHalf, EncrypterHalf};

use super::feed::{Delivery, Packet};

pub(super) async fn read_logon(reader: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>> {
    let opcode = reader.read_u8().await?;
    let mut frame = vec![opcode];
    match opcode {
        0 | 2 => {
            append(reader, &mut frame, 3).await?;
            let size = u16::from_le_bytes(frame[2..4].try_into()?);
            append(reader, &mut frame, usize::from(size)).await?;
        }
        1 => {
            append(reader, &mut frame, 73).await?;
            let telemetry_bytes = usize::from(frame[73]) * 30;
            append(reader, &mut frame, telemetry_bytes + 1).await?;
            match frame.last() {
                Some(0) => {}
                Some(1) => append(reader, &mut frame, 36).await?,
                _ => anyhow::bail!("unknown logon proof security flag"),
            }
        }
        0x10 => append(reader, &mut frame, 4).await?,
        _ => anyhow::bail!("unknown logon frame opcode"),
    }
    Ok(frame)
}

async fn append(
    reader: &mut (impl AsyncRead + Unpin),
    frame: &mut Vec<u8>,
    count: usize,
) -> Result<()> {
    ensure!(frame.len() + count <= 8192, "logon frame exceeds its bound");
    let start = frame.len();
    frame.resize(start + count, 0);
    reader.read_exact(&mut frame[start..]).await?;
    Ok(())
}

pub(super) async fn write_logon(
    writer: &mut (impl AsyncWrite + Unpin),
    delivery: Delivery,
) -> Result<bool> {
    ensure!(
        delivery.cipher.is_none(),
        "world cipher on logon connection"
    );
    let mut authenticated = false;
    for packet in &delivery.packets {
        ensure!(packet.opcode.is_none(), "world packet on logon connection");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_micros();
        if let Ok(not_before) = u128::try_from(packet.not_before_micros) {
            if not_before > now {
                tokio::time::sleep(Duration::from_micros(u64::try_from(not_before - now)?)).await;
            }
        }
        tokio::time::timeout(Duration::from_secs(2), writer.write_all(&packet.body)).await??;
        ensure!(!packet.close_after, "Module closed the native connection");
        authenticated |= packet.authentication_complete;
    }
    Ok(authenticated)
}

pub(super) async fn read_client(
    reader: &mut (impl AsyncRead + Unpin),
    cipher: Option<&mut DecrypterHalf>,
) -> Result<(u32, Vec<u8>)> {
    let mut header = [0; 6];
    header[0] = reader.read_u8().await?;
    tokio::time::timeout(Duration::from_secs(90), async {
        reader.read_exact(&mut header[1..]).await?;
        let (size, opcode) = match cipher {
            Some(cipher) => {
                let header = cipher.decrypt_client_header(header);
                (header.size, header.opcode)
            }
            None => (
                u16::from_be_bytes(header[..2].try_into()?),
                u32::from_le_bytes(header[2..].try_into()?),
            ),
        };
        ensure!(size >= 4, "invalid world request size");
        let mut body = vec![0; usize::from(size) - 4];
        reader.read_exact(&mut body).await?;
        Ok((opcode, body))
    })
    .await?
}

pub(super) async fn write_plain(
    writer: &mut (impl AsyncWrite + Unpin),
    delivery: Delivery,
) -> Result<()> {
    ensure!(
        delivery.cipher.is_none(),
        "unexpected cipher before authentication"
    );
    write_packets(writer, None, delivery.packets).await
}

pub(super) async fn write_encrypted(
    writer: &mut (impl AsyncWrite + Unpin),
    cipher: &mut EncrypterHalf,
    packets: Vec<std::sync::Arc<Packet>>,
) -> Result<()> {
    write_packets(writer, Some(cipher), packets).await
}

async fn write_packets(
    writer: &mut (impl AsyncWrite + Unpin),
    mut cipher: Option<&mut EncrypterHalf>,
    packets: Vec<std::sync::Arc<Packet>>,
) -> Result<()> {
    let mut bytes = Vec::new();
    let mut close = false;
    for packet in packets {
        ensure!(!close, "Module emitted packets after connection close");
        close = packet.close_after;
        let Some(opcode) = packet.opcode else {
            ensure!(
                packet.body.is_empty(),
                "native control record has packet bytes"
            );
            continue;
        };
        let size = u16::try_from(packet.body.len() + 2).context("oversized world response")?;
        match cipher.as_deref_mut() {
            Some(cipher) => bytes.extend_from_slice(&cipher.encrypt_server_header(size, opcode)),
            None => {
                bytes.extend_from_slice(&size.to_be_bytes());
                bytes.extend_from_slice(&opcode.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&packet.body);
    }
    tokio::time::timeout(Duration::from_secs(2), writer.write_all(&bytes)).await??;
    ensure!(!close, "Module closed the native connection");
    Ok(())
}
