use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;

use bytes::Bytes;
use tracing::{debug, info, warn};

use crate::concurrency::{FastMap, fast_map};
use crate::error::ConfigValidationError;
use crate::handshake::OfflinePacket;
use crate::protocol::reliability::Reliability;
use crate::protocol::sequence24::Sequence24;
use crate::session::RakPriority;
use crate::transport::{
    RemoteDisconnectReason, ShardedRuntimeConfig, ShardedRuntimeEvent, ShardedRuntimeHandle,
    ShardedSendPayload, TransportConfig, TransportEvent, TransportMetricsSnapshot,
    spawn_sharded_runtime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerId(u64);

impl PeerId {
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendOptions {
    pub reliability: Reliability,
    pub channel: u8,
    pub priority: RakPriority,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            reliability: Reliability::ReliableOrdered,
            channel: 0,
            priority: RakPriority::High,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerDisconnectReason {
    Requested,
    RemoteDisconnectionNotification { reason_code: Option<u8> },
    RemoteDetectLostConnection,
    WorkerStopped { shard_id: usize },
}

#[derive(Debug)]
pub enum RaknetServerEvent {
    PeerConnected {
        peer_id: PeerId,
        addr: SocketAddr,
        shard_id: usize,
    },
    PeerDisconnected {
        peer_id: PeerId,
        addr: SocketAddr,
        reason: PeerDisconnectReason,
    },
    Packet {
        peer_id: PeerId,
        addr: SocketAddr,
        payload: Bytes,
        reliability: Reliability,
        reliable_index: Option<Sequence24>,
        sequence_index: Option<Sequence24>,
        ordering_index: Option<Sequence24>,
        ordering_channel: Option<u8>,
    },
    OfflinePacket {
        addr: SocketAddr,
        packet: OfflinePacket,
    },
    ReceiptAcked {
        peer_id: PeerId,
        addr: SocketAddr,
        receipt_id: u64,
    },
    PeerRateLimited {
        addr: SocketAddr,
    },
    SessionLimitReached {
        addr: SocketAddr,
    },
    ProxyDropped {
        addr: SocketAddr,
    },
    DecodeError {
        addr: SocketAddr,
        error: String,
    },
    WorkerError {
        shard_id: usize,
        message: String,
    },
    WorkerStopped {
        shard_id: usize,
    },
    Metrics {
        shard_id: usize,
        snapshot: Box<TransportMetricsSnapshot>,
        dropped_non_critical_events: u64,
    },
}

impl RaknetServerEvent {
    pub fn metrics_snapshot(&self) -> Option<(usize, &TransportMetricsSnapshot, u64)> {
        match self {
            Self::Metrics {
                shard_id,
                snapshot,
                dropped_non_critical_events,
            } => Some((*shard_id, snapshot.as_ref(), *dropped_non_critical_events)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RaknetServerBuilder {
    transport_config: TransportConfig,
    runtime_config: ShardedRuntimeConfig,
}

impl RaknetServerBuilder {
    pub fn transport_config(mut self, config: TransportConfig) -> Self {
        self.transport_config = config;
        self
    }

    pub fn runtime_config(mut self, config: ShardedRuntimeConfig) -> Self {
        self.runtime_config = config;
        self
    }

    pub fn bind_addr(mut self, bind_addr: SocketAddr) -> Self {
        self.transport_config.bind_addr = bind_addr;
        self
    }

    pub fn shard_count(mut self, shard_count: usize) -> Self {
        self.runtime_config.shard_count = shard_count.max(1);
        self
    }

    pub fn transport_config_mut(&mut self) -> &mut TransportConfig {
        &mut self.transport_config
    }

    pub fn runtime_config_mut(&mut self) -> &mut ShardedRuntimeConfig {
        &mut self.runtime_config
    }

    pub async fn start(self) -> io::Result<RaknetServer> {
        self.transport_config
            .validate()
            .map_err(invalid_config_io_error)?;
        self.runtime_config
            .validate()
            .map_err(invalid_config_io_error)?;
        RaknetServer::start_with_configs(self.transport_config, self.runtime_config).await
    }
}

#[derive(Debug, Clone, Copy)]
struct PeerBinding {
    peer_id: PeerId,
    shard_id: usize,
}

pub struct RaknetServer {
    runtime: ShardedRuntimeHandle,
    peers_by_addr: FastMap<SocketAddr, PeerBinding>,
    addrs_by_peer: FastMap<PeerId, SocketAddr>,
    pending_events: VecDeque<RaknetServerEvent>,
    next_peer_id: u64,
}

impl RaknetServer {
    pub fn builder() -> RaknetServerBuilder {
        RaknetServerBuilder::default()
    }

    pub async fn bind(bind_addr: SocketAddr) -> io::Result<Self> {
        Self::builder().bind_addr(bind_addr).start().await
    }

    pub async fn start_with_configs(
        transport_config: TransportConfig,
        runtime_config: ShardedRuntimeConfig,
    ) -> io::Result<Self> {
        transport_config
            .validate()
            .map_err(invalid_config_io_error)?;
        runtime_config.validate().map_err(invalid_config_io_error)?;
        let runtime = spawn_sharded_runtime(transport_config, runtime_config).await?;
        Ok(Self {
            runtime,
            peers_by_addr: fast_map(),
            addrs_by_peer: fast_map(),
            pending_events: VecDeque::new(),
            next_peer_id: 1,
        })
    }

    pub fn peer_addr(&self, peer_id: PeerId) -> Option<SocketAddr> {
        self.addrs_by_peer.get(&peer_id).map(|addr| *addr)
    }

    pub fn peer_shard(&self, peer_id: PeerId) -> Option<usize> {
        let addr = self.addrs_by_peer.get(&peer_id).map(|addr| *addr)?;
        self.peers_by_addr
            .get(&addr)
            .map(|binding| binding.shard_id)
    }

    pub fn peer_id_for_addr(&self, addr: SocketAddr) -> Option<PeerId> {
        self.peers_by_addr.get(&addr).map(|binding| binding.peer_id)
    }

    pub async fn send(&self, peer_id: PeerId, payload: impl Into<Bytes>) -> io::Result<()> {
        self.send_with_options(peer_id, payload, SendOptions::default())
            .await
    }

    pub async fn send_with_options(
        &self,
        peer_id: PeerId,
        payload: impl Into<Bytes>,
        options: SendOptions,
    ) -> io::Result<()> {
        let (addr, shard_id) = self.resolve_peer_route(peer_id)?;

        self.runtime
            .send_payload_to_shard(
                shard_id,
                ShardedSendPayload {
                    addr,
                    payload: payload.into(),
                    reliability: options.reliability,
                    channel: options.channel,
                    priority: options.priority,
                },
            )
            .await
    }

    pub async fn send_with_receipt(
        &self,
        peer_id: PeerId,
        payload: impl Into<Bytes>,
        receipt_id: u64,
    ) -> io::Result<()> {
        self.send_with_options_and_receipt(peer_id, payload, SendOptions::default(), receipt_id)
            .await
    }

    pub async fn send_with_options_and_receipt(
        &self,
        peer_id: PeerId,
        payload: impl Into<Bytes>,
        options: SendOptions,
        receipt_id: u64,
    ) -> io::Result<()> {
        let (addr, shard_id) = self.resolve_peer_route(peer_id)?;

        self.runtime
            .send_payload_to_shard_with_receipt(
                shard_id,
                ShardedSendPayload {
                    addr,
                    payload: payload.into(),
                    reliability: options.reliability,
                    channel: options.channel,
                    priority: options.priority,
                },
                receipt_id,
            )
            .await
    }

    pub async fn disconnect(&mut self, peer_id: PeerId) -> io::Result<()> {
        let (addr, shard_id) = self.resolve_peer_route(peer_id)?;
        info!(
            peer_id = peer_id.as_u64(),
            %addr,
            shard_id,
            "server disconnect requested"
        );

        self.runtime
            .disconnect_peer_from_shard(shard_id, addr)
            .await?;
        self.remove_peer(addr);
        self.pending_events
            .push_back(RaknetServerEvent::PeerDisconnected {
                peer_id,
                addr,
                reason: PeerDisconnectReason::Requested,
            });
        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<RaknetServerEvent> {
        if let Some(event) = self.pending_events.pop_front() {
            return Some(event);
        }

        loop {
            let runtime_event = self.runtime.event_rx.recv().await?;
            self.enqueue_runtime_event(runtime_event);
            if let Some(event) = self.pending_events.pop_front() {
                return Some(event);
            }
        }
    }

    pub async fn shutdown(self) -> io::Result<()> {
        self.runtime.shutdown().await
    }

    fn enqueue_runtime_event(&mut self, runtime_event: ShardedRuntimeEvent) {
        match runtime_event {
            ShardedRuntimeEvent::Transport { shard_id, event } => match event {
                TransportEvent::PeerDisconnected { addr, reason } => {
                    if let Some(peer_id) = self.remove_peer(addr) {
                        let reason = match reason {
                            RemoteDisconnectReason::DisconnectionNotification { reason_code } => {
                                PeerDisconnectReason::RemoteDisconnectionNotification {
                                    reason_code,
                                }
                            }
                            RemoteDisconnectReason::DetectLostConnection => {
                                PeerDisconnectReason::RemoteDetectLostConnection
                            }
                        };
                        info!(
                            peer_id = peer_id.as_u64(),
                            %addr,
                            ?reason,
                            "peer disconnected"
                        );
                        self.pending_events
                            .push_back(RaknetServerEvent::PeerDisconnected {
                                peer_id,
                                addr,
                                reason,
                            });
                    } else {
                        debug!(
                            %addr,
                            ?reason,
                            "received peer disconnect for unknown address"
                        );
                    }
                }
                TransportEvent::ConnectedFrames {
                    addr,
                    frames,
                    receipts,
                    ..
                } => {
                    let (peer_id, is_new) = self.ensure_peer(addr, shard_id);
                    if is_new {
                        info!(
                            peer_id = peer_id.as_u64(),
                            %addr,
                            shard_id,
                            "peer connected"
                        );
                        self.pending_events
                            .push_back(RaknetServerEvent::PeerConnected {
                                peer_id,
                                addr,
                                shard_id,
                            });
                    }

                    for frame in frames {
                        self.pending_events.push_back(RaknetServerEvent::Packet {
                            peer_id,
                            addr,
                            payload: frame.payload,
                            reliability: frame.reliability,
                            reliable_index: frame.reliable_index,
                            sequence_index: frame.sequence_index,
                            ordering_index: frame.ordering_index,
                            ordering_channel: frame.ordering_channel,
                        });
                    }

                    for receipt_id in receipts.acked_receipt_ids {
                        self.pending_events
                            .push_back(RaknetServerEvent::ReceiptAcked {
                                peer_id,
                                addr,
                                receipt_id,
                            });
                    }
                }
                TransportEvent::RateLimited { addr } => {
                    warn!(%addr, "peer rate-limited");
                    self.pending_events
                        .push_back(RaknetServerEvent::PeerRateLimited { addr });
                }
                TransportEvent::SessionLimitReached { addr } => {
                    warn!(%addr, "session limit reached");
                    self.pending_events
                        .push_back(RaknetServerEvent::SessionLimitReached { addr });
                }
                TransportEvent::ConnectedDatagramDroppedNoSession { .. } => {}
                TransportEvent::ProxyDropped { addr } => {
                    debug!(%addr, "proxy router dropped packet");
                    self.pending_events
                        .push_back(RaknetServerEvent::ProxyDropped { addr });
                }
                TransportEvent::DecodeError { addr, error } => {
                    warn!(%addr, %error, "transport decode error");
                    self.pending_events
                        .push_back(RaknetServerEvent::DecodeError {
                            addr,
                            error: error.to_string(),
                        });
                }
                TransportEvent::OfflinePacket { addr, packet } => {
                    self.pending_events
                        .push_back(RaknetServerEvent::OfflinePacket { addr, packet });
                }
            },
            ShardedRuntimeEvent::Metrics {
                shard_id,
                snapshot,
                dropped_non_critical_events,
            } => {
                if dropped_non_critical_events > 0 {
                    debug!(
                        shard_id,
                        dropped_non_critical_events,
                        "non-critical runtime events were dropped before metrics emit"
                    );
                }
                self.pending_events.push_back(RaknetServerEvent::Metrics {
                    shard_id,
                    snapshot,
                    dropped_non_critical_events,
                });
            }
            ShardedRuntimeEvent::WorkerError { shard_id, message } => {
                warn!(shard_id, %message, "runtime worker error");
                self.pending_events
                    .push_back(RaknetServerEvent::WorkerError { shard_id, message });
            }
            ShardedRuntimeEvent::WorkerStopped { shard_id } => {
                warn!(shard_id, "runtime worker stopped");
                let mut disconnected = Vec::new();
                for peer in self.peers_by_addr.iter() {
                    let addr = *peer.key();
                    let binding = *peer.value();
                    if binding.shard_id == shard_id {
                        disconnected.push((addr, binding.peer_id));
                    }
                }
                for (addr, peer_id) in disconnected {
                    self.remove_peer(addr);
                    info!(
                        peer_id = peer_id.as_u64(),
                        %addr,
                        shard_id,
                        "peer disconnected because worker stopped"
                    );
                    self.pending_events
                        .push_back(RaknetServerEvent::PeerDisconnected {
                            peer_id,
                            addr,
                            reason: PeerDisconnectReason::WorkerStopped { shard_id },
                        });
                }
                self.pending_events
                    .push_back(RaknetServerEvent::WorkerStopped { shard_id });
            }
        }
    }

    fn ensure_peer(&mut self, addr: SocketAddr, shard_id: usize) -> (PeerId, bool) {
        if let Some(mut binding) = self.peers_by_addr.get_mut(&addr) {
            if binding.shard_id != shard_id {
                binding.shard_id = shard_id;
            }
            return (binding.peer_id, false);
        }

        let peer_id = PeerId(self.next_peer_id);
        self.next_peer_id = self.next_peer_id.saturating_add(1);
        self.peers_by_addr
            .insert(addr, PeerBinding { peer_id, shard_id });
        self.addrs_by_peer.insert(peer_id, addr);
        (peer_id, true)
    }

    fn remove_peer(&mut self, addr: SocketAddr) -> Option<PeerId> {
        let (_, binding) = self.peers_by_addr.remove(&addr)?;
        self.addrs_by_peer.remove(&binding.peer_id);
        Some(binding.peer_id)
    }

    fn resolve_peer_route(&self, peer_id: PeerId) -> io::Result<(SocketAddr, usize)> {
        let addr = self
            .addrs_by_peer
            .get(&peer_id)
            .map(|entry| *entry)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer id not found"))?;
        let shard_id = self
            .peers_by_addr
            .get(&addr)
            .map(|binding| binding.shard_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer shard binding missing"))?;
        Ok((addr, shard_id))
    }
}

fn invalid_config_io_error(error: ConfigValidationError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{PeerId, RaknetServer, RaknetServerBuilder};
    use crate::transport::{ShardedRuntimeConfig, TransportConfig};
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn builder_mutators_keep_values() {
        let builder = RaknetServerBuilder::default().shard_count(4);
        assert_eq!(builder.runtime_config.shard_count, 4);
    }

    #[test]
    fn peer_id_roundtrip() {
        let peer = PeerId::from_u64(42);
        assert_eq!(peer.as_u64(), 42);
    }

    #[test]
    fn builder_type_is_exposed() {
        let _ = RaknetServer::builder();
    }

    #[tokio::test]
    async fn start_with_invalid_runtime_config_fails_fast() {
        let transport = TransportConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            ..TransportConfig::default()
        };
        let runtime = ShardedRuntimeConfig {
            shard_count: 0,
            ..ShardedRuntimeConfig::default()
        };

        match RaknetServer::start_with_configs(transport, runtime).await {
            Ok(_) => panic!("invalid config must fail before runtime start"),
            Err(err) => assert_eq!(err.kind(), io::ErrorKind::InvalidInput),
        }
    }
}
