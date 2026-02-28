use std::io;
use std::net::SocketAddr;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use bytes::Bytes;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::server::{PeerDisconnectReason, PeerId, SendOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionCloseReason {
    RequestedByLocal,
    PeerDisconnected(PeerDisconnectReason),
    ListenerStopped,
    InboundBackpressure,
    TransportError(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecvError {
    #[error("connection closed: {reason:?}")]
    ConnectionClosed { reason: ConnectionCloseReason },
    #[error("decode error: {message}")]
    DecodeError { message: String },
    #[error("connection receive channel closed")]
    ChannelClosed,
}

pub mod queue {
    use thiserror::Error;

    #[derive(Debug, Error, Clone, PartialEq, Eq)]
    pub enum SendQueueError {
        #[error("connection command channel closed")]
        CommandChannelClosed,
        #[error("connection command response dropped")]
        ResponseDropped,
        #[error("transport send failed: {message}")]
        Transport { message: String },
    }
}

#[derive(Debug)]
pub(crate) enum ConnectionInbound {
    Packet(Bytes),
    DecodeError(String),
    Closed(ConnectionCloseReason),
}

#[derive(Debug)]
pub(crate) enum ConnectionCommand {
    Send {
        peer_id: PeerId,
        payload: Bytes,
        options: SendOptions,
        response: oneshot::Sender<io::Result<()>>,
    },
    Disconnect {
        peer_id: PeerId,
        response: oneshot::Sender<io::Result<()>>,
    },
    DisconnectNoWait {
        peer_id: PeerId,
    },
    Shutdown {
        response: oneshot::Sender<io::Result<()>>,
    },
}

#[derive(Debug)]
pub(crate) struct ConnectionSharedState {
    closed: AtomicBool,
    close_reason: Mutex<Option<ConnectionCloseReason>>,
}

impl ConnectionSharedState {
    pub(crate) fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
            close_reason: Mutex::new(None),
        }
    }

    pub(crate) fn mark_closed(&self, reason: ConnectionCloseReason) {
        self.closed.store(true, Ordering::Release);
        if let Ok(mut guard) = self.close_reason.lock() {
            *guard = Some(reason);
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn close_reason(&self) -> Option<ConnectionCloseReason> {
        self.close_reason
            .lock()
            .ok()
            .and_then(|guard| (*guard).clone())
    }
}

pub struct Connection {
    pub address: SocketAddr,
    peer_id: PeerId,
    command_tx: mpsc::Sender<ConnectionCommand>,
    inbound_rx: mpsc::Receiver<ConnectionInbound>,
    shared: Arc<ConnectionSharedState>,
}

impl Connection {
    pub(crate) fn new(
        peer_id: PeerId,
        address: SocketAddr,
        command_tx: mpsc::Sender<ConnectionCommand>,
        inbound_rx: mpsc::Receiver<ConnectionInbound>,
        shared: Arc<ConnectionSharedState>,
    ) -> Self {
        Self {
            address,
            peer_id,
            command_tx,
            inbound_rx,
            shared,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn close_reason(&self) -> Option<ConnectionCloseReason> {
        self.shared.close_reason()
    }

    pub async fn send_with_options(
        &self,
        payload: impl Into<Bytes>,
        options: SendOptions,
    ) -> Result<(), queue::SendQueueError> {
        if self.shared.is_closed() {
            return Err(queue::SendQueueError::Transport {
                message: "connection already closed".to_string(),
            });
        }

        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(ConnectionCommand::Send {
                peer_id: self.peer_id,
                payload: payload.into(),
                options,
                response: response_tx,
            })
            .await
            .map_err(|_| queue::SendQueueError::CommandChannelClosed)?;

        match response_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(queue::SendQueueError::Transport {
                message: err.to_string(),
            }),
            Err(_) => Err(queue::SendQueueError::ResponseDropped),
        }
    }

    pub async fn send_bytes(&self, payload: impl Into<Bytes>) -> Result<(), queue::SendQueueError> {
        self.send_with_options(payload, SendOptions::default())
            .await
    }

    pub async fn send(
        &mut self,
        stream: &[u8],
        _immediate: bool,
    ) -> Result<(), queue::SendQueueError> {
        self.send_bytes(Bytes::copy_from_slice(stream)).await
    }

    pub async fn recv_bytes(&mut self) -> Result<Bytes, RecvError> {
        match self.inbound_rx.recv().await {
            Some(ConnectionInbound::Packet(payload)) => Ok(payload),
            Some(ConnectionInbound::DecodeError(message)) => {
                Err(RecvError::DecodeError { message })
            }
            Some(ConnectionInbound::Closed(reason)) => {
                self.shared.mark_closed(reason.clone());
                Err(RecvError::ConnectionClosed { reason })
            }
            None => {
                if let Some(reason) = self.shared.close_reason() {
                    Err(RecvError::ConnectionClosed { reason })
                } else {
                    self.shared
                        .mark_closed(ConnectionCloseReason::ListenerStopped);
                    Err(RecvError::ChannelClosed)
                }
            }
        }
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>, RecvError> {
        self.recv_bytes().await.map(|payload| payload.to_vec())
    }

    pub async fn close(&self) {
        if self.shared.is_closed() {
            return;
        }

        let (response_tx, response_rx) = oneshot::channel();
        if self
            .command_tx
            .send(ConnectionCommand::Disconnect {
                peer_id: self.peer_id,
                response: response_tx,
            })
            .await
            .is_err()
        {
            self.shared
                .mark_closed(ConnectionCloseReason::ListenerStopped);
            return;
        }

        let _ = response_rx.await;
    }

    pub async fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.shared.is_closed() {
            return;
        }

        let _ = self
            .command_tx
            .try_send(ConnectionCommand::DisconnectNoWait {
                peer_id: self.peer_id,
            });
    }
}
