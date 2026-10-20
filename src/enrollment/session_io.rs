//! Per-session TLS actor: exclusive ownership of read/write halves.
//!
//! Reader and writer run as independent loops. `select!` only joins the two
//! whole loops for termination — never cancels a mid-frame `read_exact` to
//! service an outgoing command (tokio cancellation of `read_u32`/`read_exact`
//! would drop already-consumed bytes and corrupt framing).

use super::{
    channel::{EnrollReader, EnrollWriter},
    protocol::EnrollMessage,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

#[derive(Debug)]
pub enum IoCommand {
    /// Write a frame; optionally signal flush completion.
    Send {
        msg: EnrollMessage,
        done: Option<oneshot::Sender<Result<(), ()>>>,
    },
    /// Best-effort cancel frame then shut down.
    CancelNotify(Uuid),
    Shutdown,
}

impl IoCommand {
    pub fn send(msg: EnrollMessage) -> Self {
        Self::Send { msg, done: None }
    }

    pub fn send_acked(msg: EnrollMessage) -> (Self, oneshot::Receiver<Result<(), ()>>) {
        let (tx, rx) = oneshot::channel();
        (
            Self::Send {
                msg,
                done: Some(tx),
            },
            rx,
        )
    }
}

#[derive(Debug)]
pub enum SessionEvent {
    Message(EnrollMessage),
    Disconnected,
    WriteFailed,
}

pub struct SessionIo {
    pub cmd_tx: mpsc::Sender<IoCommand>,
    pub join: JoinHandle<()>,
}

impl SessionIo {
    pub fn spawn(
        reader: EnrollReader,
        writer: EnrollWriter,
    ) -> (Self, mpsc::Receiver<SessionEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<IoCommand>(16);
        let (evt_tx, evt_rx) = mpsc::channel::<SessionEvent>(16);
        let join = tokio::spawn(async move {
            run_actor(reader, writer, cmd_rx, evt_tx).await;
        });
        (Self { cmd_tx, join }, evt_rx)
    }

    /// Test/harness entry: same actor split over arbitrary duplex halves.
    #[cfg(test)]
    pub fn spawn_raw<R, W>(reader: R, writer: W) -> (Self, mpsc::Receiver<SessionEvent>)
    where
        R: tokio::io::AsyncReadExt + Unpin + Send + 'static,
        W: tokio::io::AsyncWriteExt + Unpin + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel::<IoCommand>(16);
        let (evt_tx, evt_rx) = mpsc::channel::<SessionEvent>(16);
        let join = tokio::spawn(async move {
            run_actor_rw(reader, writer, cmd_rx, evt_tx).await;
        });
        (Self { cmd_tx, join }, evt_rx)
    }

    pub fn abort(&self) {
        self.join.abort();
    }
}

async fn run_actor(
    reader: EnrollReader,
    writer: EnrollWriter,
    cmd_rx: mpsc::Receiver<IoCommand>,
    evt_tx: mpsc::Sender<SessionEvent>,
) {
    run_actor_rw(reader.read, writer.write, cmd_rx, evt_tx).await;
}

async fn run_actor_rw<R, W>(
    mut reader: R,
    mut writer: W,
    mut cmd_rx: mpsc::Receiver<IoCommand>,
    evt_tx: mpsc::Sender<SessionEvent>,
) where
    R: tokio::io::AsyncReadExt + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let writer_loop = async {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                IoCommand::Send { msg, done } => {
                    let result = super::channel::write_frame(&mut writer, &msg).await;
                    let failed = result.is_err();
                    if let Some(tx) = done {
                        let _ = tx.send(if failed { Err(()) } else { Ok(()) });
                    }
                    if failed {
                        let _ = evt_tx.send(SessionEvent::WriteFailed).await;
                        break;
                    }
                }
                IoCommand::CancelNotify(session_id) => {
                    let _ = super::channel::write_frame(
                        &mut writer,
                        &EnrollMessage::Cancel { session_id },
                    )
                    .await;
                    break;
                }
                IoCommand::Shutdown => break,
            }
        }
    };

    let reader_loop = async {
        loop {
            // Full frame reads run to completion — never cancelled by writer cmds.
            match super::channel::read_frame_session(&mut reader).await {
                Ok(m) => {
                    if evt_tx.send(SessionEvent::Message(m)).await.is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = evt_tx.send(SessionEvent::Disconnected).await;
                    break;
                }
            }
        }
    };

    tokio::select! {
        _ = writer_loop => {}
        _ = reader_loop => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrollment::protocol::EnrollMessage;
    use tokio::io::AsyncWriteExt;
    use uuid::Uuid;

    #[tokio::test]
    async fn fragmented_frame_survives_concurrent_write() {
        let (client, server) = tokio::io::duplex(1024);
        let (client_r, mut client_w) = tokio::io::split(client);
        let (server_r, server_w) = tokio::io::split(server);

        let (io, mut evt_rx) = SessionIo::spawn_raw(server_r, server_w);
        let session_id = Uuid::new_v4();
        let peer_session_id = Uuid::new_v4();
        let msg = EnrollMessage::Confirm {
            session_id,
            peer_session_id,
            binding_hex: "ab".repeat(16),
            confirmed: true,
        };
        let payload = serde_json::to_vec(&msg).unwrap();
        let len = (payload.len() as u32).to_be_bytes();

        // Start a concurrent outgoing write while inbound frame is incomplete.
        let cmd_tx = io.cmd_tx.clone();
        let write_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            cmd_tx
                .send(IoCommand::send(EnrollMessage::Cancel {
                    session_id: Uuid::new_v4(),
                }))
                .await
                .unwrap();
        });

        // Deliver header/body in fragments with gaps (would corrupt under select!-cancel).
        client_w.write_all(&len[..2]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        client_w.write_all(&len[2..]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        client_w
            .write_all(&payload[..payload.len() / 2])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        client_w
            .write_all(&payload[payload.len() / 2..])
            .await
            .unwrap();
        client_w.flush().await.unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(2), evt_rx.recv())
            .await
            .expect("event timeout")
            .expect("channel closed");
        match got {
            SessionEvent::Message(EnrollMessage::Confirm {
                session_id: sid,
                confirmed,
                ..
            }) => {
                assert_eq!(sid, session_id);
                assert!(confirmed);
            }
            other => panic!("expected Confirm, got {other:?}"),
        }
        write_task.await.unwrap();
        let _ = io.cmd_tx.send(IoCommand::Shutdown).await;
        // Drain duplex so client half is not dropped mid-write unread.
        let _ = client_r;
    }
}
