use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::connection::{
    Connection, ConnectionCloseReason, ConnectionCommand, ConnectionInbound, ConnectionSharedState,
    RemoteDisconnectReason,
};
use crate::error::server::ServerError;
use crate::server::{PeerId, RaknetServer, RaknetServerEvent};
use crate::transport::{ShardedRuntimeConfig, TransportConfig};

const DEFAULT_ACCEPT_QUEUE_CAPACITY: usize = 512;
const DEFAULT_INBOUND_QUEUE_CAPACITY: usize = 256;
const DEFAULT_COMMAND_QUEUE_CAPACITY: usize = 2048;

struct ListenerRuntime {
    command_tx: mpsc::Sender<ConnectionCommand>,
    accept_rx: mpsc::Receiver<Connection>,
    worker: JoinHandle<()>,
}

struct PeerRuntime {
    addr: SocketAddr,
    inbound_tx: mpsc::Sender<ConnectionInbound>,
    shared: Arc<ConnectionSharedState>,
}

pub struct Listener {
    bind_addr: SocketAddr,
    transport_config: TransportConfig,
    runtime_config: ShardedRuntimeConfig,
    accept_queue_capacity: usize,
    inbound_queue_capacity: usize,
    command_queue_capacity: usize,
    runtime: Option<ListenerRuntime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerMetadata {
    bind_addr: SocketAddr,
    started: bool,
    shard_count: usize,
    advertisement: String,
}

impl ListenerMetadata {
    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub const fn started(&self) -> bool {
        self.started
    }

    pub const fn shard_count(&self) -> usize {
        self.shard_count
    }

    pub fn advertisement(&self) -> &str {
        &self.advertisement
    }
}

impl Listener {
    pub async fn bind(bind_addr: SocketAddr) -> Result<Self, ServerError> {
        let transport_config = TransportConfig {
            bind_addr,
            ..TransportConfig::default()
        };

        Ok(Self {
            bind_addr,
            transport_config,
            runtime_config: ShardedRuntimeConfig::default(),
            accept_queue_capacity: DEFAULT_ACCEPT_QUEUE_CAPACITY,
            inbound_queue_capacity: DEFAULT_INBOUND_QUEUE_CAPACITY,
            command_queue_capacity: DEFAULT_COMMAND_QUEUE_CAPACITY,
            runtime: None,
        })
    }

    pub fn set_pong_data(&mut self, data: impl Into<String>) {
        self.transport_config.advertisement = data.into();
    }

    pub fn pong_data(&self) -> &str {
        &self.transport_config.advertisement
    }

    pub fn set_accept_queue_capacity(&mut self, capacity: usize) {
        self.accept_queue_capacity = capacity.max(1);
    }

    pub fn set_inbound_queue_capacity(&mut self, capacity: usize) {
        self.inbound_queue_capacity = capacity.max(1);
    }

    pub fn set_command_queue_capacity(&mut self, capacity: usize) {
        self.command_queue_capacity = capacity.max(1);
    }

    pub fn set_shard_count(&mut self, shard_count: usize) {
        self.runtime_config.shard_count = shard_count.max(1);
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn metadata(&self) -> ListenerMetadata {
        ListenerMetadata {
            bind_addr: self.bind_addr,
            started: self.runtime.is_some(),
            shard_count: self.runtime_config.shard_count.max(1),
            advertisement: self.transport_config.advertisement.clone(),
        }
    }

    pub fn is_started(&self) -> bool {
        self.runtime.is_some()
    }

    pub async fn start(&mut self) -> Result<(), ServerError> {
        if self.runtime.is_some() {
            return Err(ServerError::AlreadyStarted);
        }

        let mut transport_config = self.transport_config.clone();
        transport_config.bind_addr = self.bind_addr;

        transport_config.validate()?;
        self.runtime_config.validate()?;

        let server =
            RaknetServer::start_with_configs(transport_config, self.runtime_config.clone())
                .await
                .map_err(ServerError::from)?;

        let (accept_tx, accept_rx) = mpsc::channel(self.accept_queue_capacity.max(1));
        let (command_tx, command_rx) = mpsc::channel(self.command_queue_capacity.max(1));
        let worker_command_tx = command_tx.clone();
        let inbound_queue_capacity = self.inbound_queue_capacity.max(1);

        let worker = tokio::spawn(async move {
            run_listener_worker(
                server,
                command_rx,
                worker_command_tx,
                accept_tx,
                inbound_queue_capacity,
            )
            .await;
        });

        self.runtime = Some(ListenerRuntime {
            command_tx,
            accept_rx,
            worker,
        });

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), ServerError> {
        let Some(runtime) = self.runtime.take() else {
            return Ok(());
        };

        let (response_tx, response_rx) = oneshot::channel();
        if runtime
            .command_tx
            .send(ConnectionCommand::Shutdown {
                response: response_tx,
            })
            .await
            .is_err()
        {
            let _ = runtime.worker.await;
            return Err(ServerError::CommandChannelClosed);
        }

        let response = response_rx.await.map_err(|_| ServerError::WorkerStopped)?;
        let _ = runtime.worker.await;
        response.map_err(ServerError::from)
    }

    pub async fn accept(&mut self) -> Result<Connection, ServerError> {
        let runtime = self.runtime.as_mut().ok_or(ServerError::NotStarted)?;
        runtime
            .accept_rx
            .recv()
            .await
            .ok_or(ServerError::AcceptChannelClosed)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.worker.abort();
        }
    }
}

async fn run_listener_worker(
    mut server: RaknetServer,
    mut command_rx: mpsc::Receiver<ConnectionCommand>,
    command_tx: mpsc::Sender<ConnectionCommand>,
    accept_tx: mpsc::Sender<Connection>,
    inbound_queue_capacity: usize,
) {
    let mut peers: HashMap<PeerId, PeerRuntime> = HashMap::new();
    let mut peer_ids_by_addr: HashMap<SocketAddr, PeerId> = HashMap::new();

    loop {
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(ConnectionCommand::Send { peer_id, payload, options, response }) => {
                        let result = if peers.contains_key(&peer_id) {
                            server.send_with_options(peer_id, payload, options).await
                        } else {
                            Err(io::Error::new(io::ErrorKind::NotFound, "peer not found"))
                        };
                        let _ = response.send(result);
                    }
                    Some(ConnectionCommand::Disconnect { peer_id, response }) => {
                        let result = disconnect_peer(
                            &mut server,
                            &mut peers,
                            &mut peer_ids_by_addr,
                            peer_id,
                            ConnectionCloseReason::RequestedByLocal,
                        )
                        .await;
                        let _ = response.send(result);
                    }
                    Some(ConnectionCommand::DisconnectNoWait { peer_id }) => {
                        let _ = disconnect_peer(
                            &mut server,
                            &mut peers,
                            &mut peer_ids_by_addr,
                            peer_id,
                            ConnectionCloseReason::RequestedByLocal,
                        )
                        .await;
                    }
                    Some(ConnectionCommand::Shutdown { response }) => {
                        for peer_id in peers.keys().copied().collect::<Vec<_>>() {
                            let _ = server.disconnect(peer_id).await;
                        }

                        close_all_peers(&mut peers, &mut peer_ids_by_addr, ConnectionCloseReason::ListenerStopped);
                        let result = server.shutdown().await;
                        let _ = response.send(result);
                        break;
                    }
                    None => {
                        close_all_peers(&mut peers, &mut peer_ids_by_addr, ConnectionCloseReason::ListenerStopped);
                        let _ = server.shutdown().await;
                        break;
                    }
                }
            }
            server_event = server.next_event() => {
                let Some(server_event) = server_event else {
                    close_all_peers(&mut peers, &mut peer_ids_by_addr, ConnectionCloseReason::ListenerStopped);
                    break;
                };

                match server_event {
                    RaknetServerEvent::PeerConnected { peer_id, addr, .. } => {
                        if let Some(existing) = peers.remove(&peer_id) {
                            peer_ids_by_addr.remove(&existing.addr);
                            close_peer_entry(existing, ConnectionCloseReason::RequestedByLocal);
                        }

                        let shared = Arc::new(ConnectionSharedState::new());
                        let (inbound_tx, inbound_rx) = mpsc::channel(inbound_queue_capacity.max(1));
                        let connection = Connection::new(
                            peer_id,
                            addr,
                            command_tx.clone(),
                            inbound_rx,
                            Arc::clone(&shared),
                        );

                        peers.insert(
                            peer_id,
                            PeerRuntime {
                                addr,
                                inbound_tx,
                                shared,
                            },
                        );
                        peer_ids_by_addr.insert(addr, peer_id);

                        if let Err(err) = accept_tx.try_send(connection) {
                            match err {
                                TrySendError::Full(conn) => {
                                    let _ = disconnect_peer(
                                        &mut server,
                                        &mut peers,
                                        &mut peer_ids_by_addr,
                                        conn.peer_id(),
                                        ConnectionCloseReason::InboundBackpressure,
                                    )
                                    .await;
                                }
                                TrySendError::Closed(conn) => {
                                    let _ = disconnect_peer(
                                        &mut server,
                                        &mut peers,
                                        &mut peer_ids_by_addr,
                                        conn.peer_id(),
                                        ConnectionCloseReason::ListenerStopped,
                                    )
                                    .await;
                                    close_all_peers(
                                        &mut peers,
                                        &mut peer_ids_by_addr,
                                        ConnectionCloseReason::ListenerStopped,
                                    );
                                    let _ = server.shutdown().await;
                                    break;
                                }
                            }
                        }
                    }
                    RaknetServerEvent::PeerDisconnected { peer_id, reason, .. } => {
                        if let Some(entry) = remove_peer(&mut peers, &mut peer_ids_by_addr, peer_id) {
                            close_peer_entry(
                                entry,
                                ConnectionCloseReason::PeerDisconnected(
                                    RemoteDisconnectReason::from(reason),
                                ),
                            );
                        }
                    }
                    RaknetServerEvent::Packet { peer_id, payload, .. } => {
                        if let Some(entry) = peers.get(&peer_id) {
                            match entry.inbound_tx.try_send(ConnectionInbound::Packet(payload)) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    let _ = disconnect_peer(
                                        &mut server,
                                        &mut peers,
                                        &mut peer_ids_by_addr,
                                        peer_id,
                                        ConnectionCloseReason::InboundBackpressure,
                                    )
                                    .await;
                                }
                                Err(TrySendError::Closed(_)) => {
                                    let _ = disconnect_peer(
                                        &mut server,
                                        &mut peers,
                                        &mut peer_ids_by_addr,
                                        peer_id,
                                        ConnectionCloseReason::ListenerStopped,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    RaknetServerEvent::DecodeError { addr, error } => {
                        if let Some(peer_id) = peer_ids_by_addr.get(&addr).copied()
                            && let Some(entry) = peers.get(&peer_id)
                        {
                            let _ = entry
                                .inbound_tx
                                .try_send(ConnectionInbound::DecodeError(error));
                        }
                    }
                    RaknetServerEvent::PeerRateLimited { .. }
                    | RaknetServerEvent::SessionLimitReached { .. }
                    | RaknetServerEvent::ProxyDropped { .. }
                    | RaknetServerEvent::OfflinePacket { .. }
                    | RaknetServerEvent::ReceiptAcked { .. }
                    | RaknetServerEvent::WorkerError { .. }
                    | RaknetServerEvent::WorkerStopped { .. }
                    | RaknetServerEvent::Metrics { .. } => {}
                }
            }
        }
    }

    drop(accept_tx);
}

fn remove_peer(
    peers: &mut HashMap<PeerId, PeerRuntime>,
    peer_ids_by_addr: &mut HashMap<SocketAddr, PeerId>,
    peer_id: PeerId,
) -> Option<PeerRuntime> {
    let entry = peers.remove(&peer_id)?;
    peer_ids_by_addr.remove(&entry.addr);
    Some(entry)
}

async fn disconnect_peer(
    server: &mut RaknetServer,
    peers: &mut HashMap<PeerId, PeerRuntime>,
    peer_ids_by_addr: &mut HashMap<SocketAddr, PeerId>,
    peer_id: PeerId,
    reason: ConnectionCloseReason,
) -> io::Result<()> {
    let result = server.disconnect(peer_id).await;
    if let Some(entry) = remove_peer(peers, peer_ids_by_addr, peer_id) {
        close_peer_entry(entry, reason);
    }
    result
}

fn close_all_peers(
    peers: &mut HashMap<PeerId, PeerRuntime>,
    peer_ids_by_addr: &mut HashMap<SocketAddr, PeerId>,
    reason: ConnectionCloseReason,
) {
    peer_ids_by_addr.clear();
    for (_, entry) in peers.drain() {
        close_peer_entry(entry, reason.clone());
    }
}

fn close_peer_entry(entry: PeerRuntime, reason: ConnectionCloseReason) {
    entry.shared.mark_closed(reason.clone());
    let _ = entry.inbound_tx.try_send(ConnectionInbound::Closed(reason));
}
