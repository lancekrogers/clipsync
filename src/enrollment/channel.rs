//! Length-prefixed enrollment frames over TLS (split read/write halves).
use super::{
    protocol::{EnrollMessage, IO_TIMEOUT_SECS, MAX_FRAME_BYTES, PARTIAL_FRAME_TIMEOUT_SECS},
    tls::{self, ENROLL_ALPN},
};
use crate::auth::{Authenticator, PublicKey};
use anyhow::{bail, Context, Result};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

type TlsTcp = TlsStream<TcpStream>;

pub struct EnrollConn {
    stream: TlsTcp,
    pub peer_key: PublicKey,
    pub peer_addr: SocketAddr,
}

pub struct EnrollReader {
    pub(crate) read: ReadHalf<TlsTcp>,
}

pub struct EnrollWriter {
    pub(crate) write: WriteHalf<TlsTcp>,
}

impl EnrollConn {
    pub async fn write_msg(&mut self, msg: &EnrollMessage) -> Result<()> {
        write_frame(&mut self.stream, msg).await
    }

    pub async fn read_msg(&mut self) -> Result<EnrollMessage> {
        // Handshake path: bound the whole exchange (no long human idle here).
        read_frame_handshake(&mut self.stream).await
    }

    /// Split into independent halves so confirm/cancel never share a duplex Mutex
    /// with a blocking reader.
    pub fn into_split(self) -> (EnrollReader, EnrollWriter, PublicKey, SocketAddr) {
        let (read, write) = tokio::io::split(self.stream);
        (
            EnrollReader { read },
            EnrollWriter { write },
            self.peer_key,
            self.peer_addr,
        )
    }
}

pub(crate) async fn write_frame<W: AsyncWriteExt + Unpin>(
    write: &mut W,
    msg: &EnrollMessage,
) -> Result<()> {
    let data = serde_json::to_vec(msg)?;
    if data.len() > MAX_FRAME_BYTES {
        bail!("Enrollment oversize frame");
    }
    timeout(Duration::from_secs(IO_TIMEOUT_SECS), async {
        write.write_u32(data.len() as u32).await?;
        write.write_all(&data).await?;
        write.flush().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("Enrollment write timed out")??;
    Ok(())
}

/// Handshake reads: entire frame must arrive within IO_TIMEOUT.
pub(crate) async fn read_frame_handshake<R: AsyncReadExt + Unpin>(
    read: &mut R,
) -> Result<EnrollMessage> {
    timeout(Duration::from_secs(IO_TIMEOUT_SECS), read_frame_body(read))
        .await
        .context("Enrollment read timed out")?
}

/// Established session reads: wait indefinitely for the next frame start so a
/// human can compare fingerprints for the full enrollment TTL. Once the length
/// prefix begins arriving, bound the remainder (partial-frame stall).
pub(crate) async fn read_frame_session<R: AsyncReadExt + Unpin>(
    read: &mut R,
) -> Result<EnrollMessage> {
    let mut len_buf = [0u8; 4];
    // First byte: idle wait (session expiry / Shutdown aborts the reader task).
    read.read_exact(&mut len_buf[..1]).await?;
    // Remainder of length + body: short stall bound.
    timeout(Duration::from_secs(PARTIAL_FRAME_TIMEOUT_SECS), async {
        read.read_exact(&mut len_buf[1..]).await?;
        let length = u32::from_be_bytes(len_buf) as usize;
        if length > MAX_FRAME_BYTES {
            bail!("Enrollment oversize frame");
        }
        let mut data = vec![0; length];
        read.read_exact(&mut data).await?;
        Ok(serde_json::from_slice(&data)?)
    })
    .await
    .context("Enrollment partial frame timed out")?
}

async fn read_frame_body<R: AsyncReadExt + Unpin>(read: &mut R) -> Result<EnrollMessage> {
    let length = read.read_u32().await? as usize;
    if length > MAX_FRAME_BYTES {
        bail!("Enrollment oversize frame");
    }
    let mut data = vec![0; length];
    read.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}

fn peer_key(stream: &TlsTcp) -> Result<PublicKey> {
    let (_, session) = stream.get_ref();
    tls::peer_key_from_common(session)
}

pub async fn accept(tcp: TcpStream, auth: Arc<dyn Authenticator>) -> Result<EnrollConn> {
    let peer_addr = tcp.peer_addr()?;
    let (_, server) = tls::enroll_configs(auth.as_ref()).await?;
    let stream = TlsStream::Server(
        timeout(
            Duration::from_secs(IO_TIMEOUT_SECS),
            TlsAcceptor::from(server).accept(tcp),
        )
        .await
        .context("Enrollment TLS accept timed out")??,
    );
    let peer_key = peer_key(&stream)?;
    Ok(EnrollConn {
        stream,
        peer_key,
        peer_addr,
    })
}

pub async fn dial(addr: SocketAddr, auth: Arc<dyn Authenticator>) -> Result<EnrollConn> {
    let tcp = timeout(
        Duration::from_secs(IO_TIMEOUT_SECS),
        TcpStream::connect(addr),
    )
    .await
    .context("Enrollment dial timed out")??;
    let peer_addr = tcp.peer_addr()?;
    let (client, _) = tls::enroll_configs(auth.as_ref()).await?;
    let stream = TlsStream::Client(
        timeout(
            Duration::from_secs(IO_TIMEOUT_SECS),
            TlsConnector::from(client).connect("clipsync.local".try_into()?, tcp),
        )
        .await
        .context("Enrollment TLS connect timed out")??,
    );
    let peer_key = peer_key(&stream)?;
    let _ = ENROLL_ALPN;
    Ok(EnrollConn {
        stream,
        peer_key,
        peer_addr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrollment::protocol::EnrollMessage;
    use uuid::Uuid;

    #[tokio::test]
    async fn session_read_allows_idle_before_frame() {
        let (mut client, mut server) = tokio::io::duplex(256);
        let msg = EnrollMessage::Cancel {
            session_id: Uuid::new_v4(),
        };
        let reader = tokio::spawn(async move { read_frame_session(&mut server).await });
        // Idle longer than a partial-frame budget would allow if misapplied to wait.
        tokio::time::sleep(Duration::from_millis(80)).await;
        write_frame(&mut client, &msg).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(got, EnrollMessage::Cancel { .. }));
    }
}
