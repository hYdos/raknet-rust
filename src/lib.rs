pub mod client;
mod concurrency;
pub mod connection;
pub mod error;
pub mod event;
pub mod handshake;
pub mod listener;
mod protocol;
pub mod proxy;
pub mod server;
mod session;
pub mod telemetry;
mod transport;

pub mod low_level {
    pub mod protocol {
        pub use crate::protocol::{
            AckNackPayload, ConnectedControlPacket, Datagram, DatagramHeader, DatagramPayload,
            Frame, FrameHeader, RaknetCodec, Reliability, Sequence24, SequenceRange, SplitInfo,
            ack, codec, connected, constants, datagram, frame, frame_header, primitives,
            reliability, sequence24,
        };
    }

    pub mod session {
        pub use crate::session::tunables;
        pub use crate::session::{
            QueuePayloadResult, RakPriority, ReceiptProgress, Session, SessionMetricsSnapshot,
            SessionState, TrackedDatagram,
        };
    }

    pub mod transport {
        pub use crate::transport::{
            ConnectedFrameDelivery, EventOverflowPolicy, HandshakeHeuristicsConfig,
            IdentityProxyRouter, InboundProxyRoute, OutboundProxyRoute, ProxyRouter,
            QueueDispatchResult, RemoteDisconnectReason, SessionTunables, ShardedRuntimeCommand,
            ShardedRuntimeConfig, ShardedRuntimeEvent, ShardedRuntimeHandle, ShardedSendPayload,
            TransportConfig, TransportEvent, TransportMetricsSnapshot, TransportRateLimitConfig,
            TransportServer,
        };
    }
}

pub use connection::{
    Connection, ConnectionCloseReason, ConnectionId, ConnectionIo, ConnectionMetadata, RecvError,
};
pub use error::{ConfigValidationError, DecodeError, EncodeError};
pub use listener::{Incoming, Listener, ListenerMetadata};
pub use low_level::protocol::{ConnectedControlPacket, Reliability, Sequence24};
pub use low_level::session::RakPriority;
pub use low_level::transport::{
    EventOverflowPolicy, ShardedRuntimeConfig, TransportConfig, TransportMetricsSnapshot,
};
