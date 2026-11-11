//! Length-prefixed enrollment frames over TLS (split read/write halves).
use super::{
    protocol::{EnrollMessage, IO_TIMEOUT_SECS, MAX_FRAME_BYTES, PARTIAL_FRAME_TIMEOUT_SECS},
    tls::{self, EnrollVersion, EXPORTER_LABEL},
};
use crate::{
    auth::{Authenticator, PublicKey},
    control::PairingFailure,
};
use anyhow::{bail, Context, Result};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
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
    pub version: EnrollVersion,
    /// TLS exporter output shared by exactly the two ends of this connection.
    pub binding: [u8; 32],
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
        read_frame_handshake(&mut self.stream).await
    }

    /// Split into independent halves so persist/cancel never share a duplex Mutex
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

    /// Best-effort refusal: send one frame, then close. The dialer writes its
    /// first frame without waiting, so read (and discard) it before closing;
    /// closing over unread data would reset the connection and lose the reason.
    pub async fn refuse(mut self, code: &str, message: &str) {
        let _ = self
            .write_msg(&EnrollMessage::Error {
                code: code.into(),
                message: message.into(),
            })
            .await;
        let mut sink = [0u8; 1024];
        let _ = timeout(Duration::from_secs(1), async {
            while matches!(self.stream.read(&mut sink).await, Ok(n) if n > 0) {}
        })
        .await;
        let _ = timeout(Duration::from_millis(500), self.stream.shutdown()).await;
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

/// Established session reads: wait for the next frame start until the session
/// ends (expiry or shutdown aborts the reader task). Once the length prefix
/// begins arriving, bound the remainder (partial-frame stall).
pub(crate) async fn read_frame_session<R: AsyncReadExt + Unpin>(
    read: &mut R,
) -> Result<EnrollMessage> {
    let mut len_buf = [0u8; 4];
    read.read_exact(&mut len_buf[..1]).await?;
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

fn established(stream: TlsTcp, peer_addr: SocketAddr) -> Result<EnrollConn> {
    let (peer_key, version) = tls::peer_key_from_common(stream.get_ref().1)?;
    let binding = match &stream {
        TlsStream::Client(s) => {
            s.get_ref()
                .1
                .export_keying_material([0u8; 32], EXPORTER_LABEL, None)
        }
        TlsStream::Server(s) => {
            s.get_ref()
                .1
                .export_keying_material([0u8; 32], EXPORTER_LABEL, None)
        }
    }
    .map_err(|_| anyhow::anyhow!("Enrollment channel binding unavailable"))?;
    Ok(EnrollConn {
        stream,
        peer_key,
        peer_addr,
        version,
        binding,
    })
}

pub async fn accept(tcp: TcpStream, auth: Arc<dyn Authenticator>) -> Result<EnrollConn> {
    let peer_addr = tcp.peer_addr()?;
    let (_, server) = tls::enroll_configs(auth.as_ref()).await?;
    accept_with(tcp, server, peer_addr).await
}

pub(crate) async fn accept_with(
    tcp: TcpStream,
    server: Arc<rustls::ServerConfig>,
    peer_addr: SocketAddr,
) -> Result<EnrollConn> {
    let stream = TlsStream::Server(
        timeout(
            Duration::from_secs(IO_TIMEOUT_SECS),
            TlsAcceptor::from(server).accept(tcp),
        )
        .await
        .context("Enrollment TLS accept timed out")??,
    );
    established(stream, peer_addr)
}

/// Dial and classify why a pairing address did not answer as ClipSync pairing.
pub async fn dial(
    addr: SocketAddr,
    auth: Arc<dyn Authenticator>,
) -> Result<EnrollConn, PairingFailure> {
    let (client, _) = tls::enroll_configs(auth.as_ref())
        .await
        .map_err(|_| PairingFailure::Protocol)?;
    dial_with(addr, client).await
}

pub(crate) async fn dial_with(
    addr: SocketAddr,
    client: Arc<rustls::ClientConfig>,
) -> Result<EnrollConn, PairingFailure> {
    let tcp = match timeout(
        Duration::from_secs(IO_TIMEOUT_SECS),
        TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => {
            return Err(PairingFailure::NotListening)
        }
        Ok(Err(e)) => {
            tracing::debug!("Enrollment dial failed: {e}");
            return Err(PairingFailure::Unreachable);
        }
        Err(_) => return Err(PairingFailure::Unreachable),
    };
    let peer_addr = tcp.peer_addr().map_err(|_| PairingFailure::Unreachable)?;
    let name = "clipsync.local"
        .try_into()
        .map_err(|_| PairingFailure::Protocol)?;
    let stream = match timeout(
        Duration::from_secs(IO_TIMEOUT_SECS),
        TlsConnector::from(client).connect(name, tcp),
    )
    .await
    {
        Ok(Ok(stream)) => TlsStream::Client(stream),
        Ok(Err(e)) => {
            tracing::debug!("Enrollment TLS connect failed: {e}");
            return Err(handshake_failure(&e));
        }
        Err(_) => return Err(PairingFailure::NotPairing),
    };
    established(stream, peer_addr).map_err(|e| {
        tracing::debug!("Enrollment negotiation failed: {e}");
        PairingFailure::NotPairing
    })
}

/// A TLS-level refusal (alert, garbage) means the address is not ClipSync
/// pairing. A connection closed before any reply means a pairing listener
/// dropped it: at capacity, rate limited, or closing its window.
fn handshake_failure(error: &io::Error) -> PairingFailure {
    let tls = error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some());
    match error.kind() {
        _ if tls => PairingFailure::NotPairing,
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => PairingFailure::Busy,
        _ => PairingFailure::NotPairing,
    }
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
