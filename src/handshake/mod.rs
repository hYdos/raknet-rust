use std::net::SocketAddr;

use bytes::{Buf, BufMut, Bytes};

use crate::error::{DecodeError, EncodeError};
use crate::protocol::codec::RaknetCodec;
use crate::protocol::constants::{
    DEFAULT_UNCONNECTED_MAGIC, ID_ALREADY_CONNECTED, ID_CONNECTION_BANNED,
    ID_CONNECTION_REQUEST_FAILED, ID_INCOMPATIBLE_PROTOCOL_VERSION, ID_IP_RECENTLY_CONNECTED,
    ID_NO_FREE_INCOMING_CONNECTIONS, ID_OPEN_CONNECTION_REPLY_1, ID_OPEN_CONNECTION_REPLY_2,
    ID_OPEN_CONNECTION_REQUEST_1, ID_OPEN_CONNECTION_REQUEST_2, ID_UNCONNECTED_PING,
    ID_UNCONNECTED_PING_OPEN_CONNECTIONS, ID_UNCONNECTED_PONG, MAXIMUM_MTU_SIZE, MINIMUM_MTU_SIZE,
    Magic,
};

pub const MAX_UNCONNECTED_PONG_MOTD_BYTES: usize = i16::MAX as usize;

#[derive(Debug, Clone)]
pub struct UnconnectedPing {
    pub ping_time: i64,
    pub client_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct UnconnectedPong {
    pub ping_time: i64,
    pub server_guid: u64,
    pub magic: Magic,
    pub motd: Bytes,
}

#[derive(Debug, Clone)]
pub struct OpenConnectionRequest1 {
    pub protocol_version: u8,
    pub mtu: u16,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct OpenConnectionReply1 {
    pub server_guid: u64,
    pub mtu: u16,
    pub cookie: Option<u32>,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct OpenConnectionRequest2 {
    pub server_addr: SocketAddr,
    pub mtu: u16,
    pub client_guid: u64,
    pub cookie: Option<u32>,
    pub client_proof: bool,
    pub parse_path: Request2ParsePath,
    pub magic: Magic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request2ParsePath {
    StrictNoCookie,
    StrictWithCookie,
    AmbiguousPreferredNoCookie,
    AmbiguousPreferredWithCookie,
    LegacyHeuristic,
}

#[derive(Debug, Clone)]
pub struct OpenConnectionReply2 {
    pub server_guid: u64,
    pub server_addr: SocketAddr,
    pub mtu: u16,
    pub use_encryption: bool,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct IncompatibleProtocolVersion {
    pub protocol_version: u8,
    pub server_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct ConnectionRequestFailed {
    pub server_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct AlreadyConnected {
    pub server_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct NoFreeIncomingConnections {
    pub server_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct ConnectionBanned {
    pub server_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub struct IpRecentlyConnected {
    pub server_guid: u64,
    pub magic: Magic,
}

#[derive(Debug, Clone)]
pub enum OfflinePacket {
    UnconnectedPing(UnconnectedPing),
    UnconnectedPingOpenConnections(UnconnectedPing),
    UnconnectedPong(UnconnectedPong),
    OpenConnectionRequest1(OpenConnectionRequest1),
    OpenConnectionReply1(OpenConnectionReply1),
    OpenConnectionRequest2(OpenConnectionRequest2),
    OpenConnectionReply2(OpenConnectionReply2),
    IncompatibleProtocolVersion(IncompatibleProtocolVersion),
    ConnectionRequestFailed(ConnectionRequestFailed),
    AlreadyConnected(AlreadyConnected),
    NoFreeIncomingConnections(NoFreeIncomingConnections),
    ConnectionBanned(ConnectionBanned),
    IpRecentlyConnected(IpRecentlyConnected),
}

impl OfflinePacket {
    pub fn id(&self) -> u8 {
        match self {
            OfflinePacket::UnconnectedPing(_) => ID_UNCONNECTED_PING,
            OfflinePacket::UnconnectedPingOpenConnections(_) => {
                ID_UNCONNECTED_PING_OPEN_CONNECTIONS
            }
            OfflinePacket::UnconnectedPong(_) => ID_UNCONNECTED_PONG,
            OfflinePacket::OpenConnectionRequest1(_) => ID_OPEN_CONNECTION_REQUEST_1,
            OfflinePacket::OpenConnectionReply1(_) => ID_OPEN_CONNECTION_REPLY_1,
            OfflinePacket::OpenConnectionRequest2(_) => ID_OPEN_CONNECTION_REQUEST_2,
            OfflinePacket::OpenConnectionReply2(_) => ID_OPEN_CONNECTION_REPLY_2,
            OfflinePacket::IncompatibleProtocolVersion(_) => ID_INCOMPATIBLE_PROTOCOL_VERSION,
            OfflinePacket::ConnectionRequestFailed(_) => ID_CONNECTION_REQUEST_FAILED,
            OfflinePacket::AlreadyConnected(_) => ID_ALREADY_CONNECTED,
            OfflinePacket::NoFreeIncomingConnections(_) => ID_NO_FREE_INCOMING_CONNECTIONS,
            OfflinePacket::ConnectionBanned(_) => ID_CONNECTION_BANNED,
            OfflinePacket::IpRecentlyConnected(_) => ID_IP_RECENTLY_CONNECTED,
        }
    }

    pub fn encode(&self, dst: &mut impl BufMut) -> Result<(), EncodeError> {
        self.id().encode_raknet(dst)?;
        match self {
            OfflinePacket::UnconnectedPing(pkt)
            | OfflinePacket::UnconnectedPingOpenConnections(pkt) => {
                pkt.ping_time.encode_raknet(dst)?;
                pkt.magic.encode_raknet(dst)?;
                pkt.client_guid.encode_raknet(dst)?;
            }
            OfflinePacket::UnconnectedPong(pkt) => {
                pkt.ping_time.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
                pkt.magic.encode_raknet(dst)?;
                validate_unconnected_pong_motd_len(pkt.motd.len())?;
                let motd_len = u16::try_from(pkt.motd.len())
                    .map_err(|_| EncodeError::OfflinePongMotdTooLong(pkt.motd.len()))?;
                motd_len.encode_raknet(dst)?;
                dst.put_slice(&pkt.motd);
            }
            OfflinePacket::OpenConnectionRequest1(pkt) => {
                validate_mtu(pkt.mtu)?;
                pkt.magic.encode_raknet(dst)?;
                pkt.protocol_version.encode_raknet(dst)?;

                // Req1 MTU is inferred from packet length; remaining bytes are zero padding.
                let padding_len = usize::from(pkt.mtu).saturating_sub(18);
                for _ in 0..padding_len {
                    dst.put_u8(0);
                }
            }
            OfflinePacket::OpenConnectionReply1(pkt) => {
                validate_mtu(pkt.mtu)?;
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
                pkt.cookie.is_some().encode_raknet(dst)?;
                if let Some(cookie) = pkt.cookie {
                    cookie.encode_raknet(dst)?;
                }
                pkt.mtu.encode_raknet(dst)?;
            }
            OfflinePacket::OpenConnectionRequest2(pkt) => {
                validate_mtu(pkt.mtu)?;
                pkt.magic.encode_raknet(dst)?;
                if let Some(cookie) = pkt.cookie {
                    cookie.encode_raknet(dst)?;
                    pkt.client_proof.encode_raknet(dst)?;
                }
                pkt.server_addr.encode_raknet(dst)?;
                pkt.mtu.encode_raknet(dst)?;
                pkt.client_guid.encode_raknet(dst)?;
            }
            OfflinePacket::OpenConnectionReply2(pkt) => {
                validate_mtu(pkt.mtu)?;
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
                pkt.server_addr.encode_raknet(dst)?;
                pkt.mtu.encode_raknet(dst)?;
                pkt.use_encryption.encode_raknet(dst)?;
            }
            OfflinePacket::IncompatibleProtocolVersion(pkt) => {
                pkt.protocol_version.encode_raknet(dst)?;
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
            }
            OfflinePacket::ConnectionRequestFailed(pkt) => {
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
            }
            OfflinePacket::AlreadyConnected(pkt) => {
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
            }
            OfflinePacket::NoFreeIncomingConnections(pkt) => {
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
            }
            OfflinePacket::ConnectionBanned(pkt) => {
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
            }
            OfflinePacket::IpRecentlyConnected(pkt) => {
                pkt.magic.encode_raknet(dst)?;
                pkt.server_guid.encode_raknet(dst)?;
            }
        }

        Ok(())
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, DecodeError> {
        Self::decode_with_magic(src, DEFAULT_UNCONNECTED_MAGIC)
    }

    pub fn decode_with_magic(
        src: &mut impl Buf,
        expected_magic: Magic,
    ) -> Result<Self, DecodeError> {
        let id = u8::decode_raknet(src)?;
        match id {
            ID_UNCONNECTED_PING => {
                decode_ping(src, expected_magic).map(OfflinePacket::UnconnectedPing)
            }
            ID_UNCONNECTED_PING_OPEN_CONNECTIONS => {
                decode_ping(src, expected_magic).map(OfflinePacket::UnconnectedPingOpenConnections)
            }
            ID_UNCONNECTED_PONG => {
                decode_pong(src, expected_magic).map(OfflinePacket::UnconnectedPong)
            }
            ID_OPEN_CONNECTION_REQUEST_1 => {
                decode_request_1(src, expected_magic).map(OfflinePacket::OpenConnectionRequest1)
            }
            ID_OPEN_CONNECTION_REPLY_1 => {
                decode_reply_1(src, expected_magic).map(OfflinePacket::OpenConnectionReply1)
            }
            ID_OPEN_CONNECTION_REQUEST_2 => {
                decode_request_2(src, expected_magic).map(OfflinePacket::OpenConnectionRequest2)
            }
            ID_OPEN_CONNECTION_REPLY_2 => {
                decode_reply_2(src, expected_magic).map(OfflinePacket::OpenConnectionReply2)
            }
            ID_INCOMPATIBLE_PROTOCOL_VERSION => decode_incompatible(src, expected_magic)
                .map(OfflinePacket::IncompatibleProtocolVersion),
            ID_CONNECTION_REQUEST_FAILED => {
                decode_reject_packet(src, expected_magic).map(|(magic, server_guid)| {
                    OfflinePacket::ConnectionRequestFailed(ConnectionRequestFailed {
                        server_guid,
                        magic,
                    })
                })
            }
            ID_ALREADY_CONNECTED => {
                decode_reject_packet(src, expected_magic).map(|(magic, server_guid)| {
                    OfflinePacket::AlreadyConnected(AlreadyConnected { server_guid, magic })
                })
            }
            ID_NO_FREE_INCOMING_CONNECTIONS => {
                decode_reject_packet(src, expected_magic).map(|(magic, server_guid)| {
                    OfflinePacket::NoFreeIncomingConnections(NoFreeIncomingConnections {
                        server_guid,
                        magic,
                    })
                })
            }
            ID_CONNECTION_BANNED => {
                decode_reject_packet(src, expected_magic).map(|(magic, server_guid)| {
                    OfflinePacket::ConnectionBanned(ConnectionBanned { server_guid, magic })
                })
            }
            ID_IP_RECENTLY_CONNECTED => {
                decode_reject_packet(src, expected_magic).map(|(magic, server_guid)| {
                    OfflinePacket::IpRecentlyConnected(IpRecentlyConnected { server_guid, magic })
                })
            }
            _ => Err(DecodeError::InvalidOfflinePacketId(id)),
        }
    }
}

fn validate_magic(magic: Magic, expected_magic: Magic) -> Result<Magic, DecodeError> {
    if magic != expected_magic {
        return Err(DecodeError::InvalidMagic);
    }
    Ok(magic)
}

fn validate_mtu(mtu: u16) -> Result<(), EncodeError> {
    if !(MINIMUM_MTU_SIZE..=MAXIMUM_MTU_SIZE).contains(&mtu) {
        return Err(EncodeError::InvalidMtu(mtu));
    }
    Ok(())
}

pub fn validate_unconnected_pong_motd_len(len: usize) -> Result<(), EncodeError> {
    if len > MAX_UNCONNECTED_PONG_MOTD_BYTES {
        return Err(EncodeError::OfflinePongMotdTooLong(len));
    }
    Ok(())
}

fn decode_ping(src: &mut impl Buf, expected_magic: Magic) -> Result<UnconnectedPing, DecodeError> {
    let ping_time = i64::decode_raknet(src)?;
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let client_guid = u64::decode_raknet(src)?;

    Ok(UnconnectedPing {
        ping_time,
        client_guid,
        magic,
    })
}

fn decode_pong(src: &mut impl Buf, expected_magic: Magic) -> Result<UnconnectedPong, DecodeError> {
    let ping_time = i64::decode_raknet(src)?;
    let server_guid = u64::decode_raknet(src)?;
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let motd_len = u16::decode_raknet(src)? as usize;
    if src.remaining() < motd_len {
        return Err(DecodeError::UnexpectedEof);
    }
    let motd = src.copy_to_bytes(motd_len);

    Ok(UnconnectedPong {
        ping_time,
        server_guid,
        magic,
        motd,
    })
}

fn decode_request_1(
    src: &mut impl Buf,
    expected_magic: Magic,
) -> Result<OpenConnectionRequest1, DecodeError> {
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let protocol_version = u8::decode_raknet(src)?;
    let padding_len = src.remaining();
    let _ = src.copy_to_bytes(padding_len);

    let mtu = (padding_len + 18) as u16;
    Ok(OpenConnectionRequest1 {
        protocol_version,
        mtu,
        magic,
    })
}

fn decode_reply_1(
    src: &mut impl Buf,
    expected_magic: Magic,
) -> Result<OpenConnectionReply1, DecodeError> {
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let server_guid = u64::decode_raknet(src)?;
    let has_cookie = bool::decode_raknet(src)?;
    let cookie = if has_cookie {
        Some(u32::decode_raknet(src)?)
    } else {
        None
    };
    let mtu = u16::decode_raknet(src)?;

    Ok(OpenConnectionReply1 {
        server_guid,
        mtu,
        cookie,
        magic,
    })
}

fn decode_request_2(
    src: &mut impl Buf,
    expected_magic: Magic,
) -> Result<OpenConnectionRequest2, DecodeError> {
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let remaining = src.copy_to_bytes(src.remaining());
    let body = &remaining[..];

    let strict_no_cookie = parse_request_2_candidate(body, false, true);
    let strict_with_cookie = parse_request_2_candidate(body, true, true);

    match (strict_no_cookie, strict_with_cookie) {
        (Ok(candidate), Err(_)) => {
            Ok(candidate.into_request(magic, Request2ParsePath::StrictNoCookie))
        }
        (Err(_), Ok(candidate)) => {
            Ok(candidate.into_request(magic, Request2ParsePath::StrictWithCookie))
        }
        (Ok(no_cookie), Ok(with_cookie)) => {
            let path = if matches!(body.first().copied(), Some(4 | 6)) {
                Request2ParsePath::AmbiguousPreferredNoCookie
            } else {
                Request2ParsePath::AmbiguousPreferredWithCookie
            };
            let chosen = match path {
                Request2ParsePath::AmbiguousPreferredNoCookie => no_cookie,
                Request2ParsePath::AmbiguousPreferredWithCookie => with_cookie,
                _ => unreachable!("ambiguous path must choose one strict candidate"),
            };
            Ok(chosen.into_request(magic, path))
        }
        (Err(_), Err(_)) => {
            let legacy = parse_request_2_legacy(body)?;
            Ok(legacy.into_request(magic, Request2ParsePath::LegacyHeuristic))
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Request2Candidate {
    server_addr: SocketAddr,
    mtu: u16,
    client_guid: u64,
    cookie: Option<u32>,
    client_proof: bool,
}

impl Request2Candidate {
    fn into_request(self, magic: Magic, parse_path: Request2ParsePath) -> OpenConnectionRequest2 {
        OpenConnectionRequest2 {
            server_addr: self.server_addr,
            mtu: self.mtu,
            client_guid: self.client_guid,
            cookie: self.cookie,
            client_proof: self.client_proof,
            parse_path,
            magic,
        }
    }
}

fn parse_request_2_candidate(
    mut body: &[u8],
    with_cookie: bool,
    strict_bool: bool,
) -> Result<Request2Candidate, DecodeError> {
    let mut cookie = None;
    let mut client_proof = false;

    if with_cookie {
        if body.len() < 5 {
            return Err(DecodeError::UnexpectedEof);
        }
        cookie = Some(u32::from_be_bytes([body[0], body[1], body[2], body[3]]));
        let raw_proof = body[4];
        if strict_bool && raw_proof > 1 {
            return Err(DecodeError::InvalidRequest2Layout);
        }
        client_proof = raw_proof == 1;
        body = &body[5..];
    }

    let server_addr = SocketAddr::decode_raknet(&mut body)?;
    let mtu = u16::decode_raknet(&mut body)?;
    let client_guid = u64::decode_raknet(&mut body)?;
    if !body.is_empty() {
        return Err(DecodeError::InvalidRequest2Layout);
    }

    Ok(Request2Candidate {
        server_addr,
        mtu,
        client_guid,
        cookie,
        client_proof,
    })
}

fn parse_request_2_legacy(body: &[u8]) -> Result<Request2Candidate, DecodeError> {
    let mut with_cookie = false;
    if let Some(first) = body.first().copied()
        && first != 4
        && first != 6
    {
        with_cookie = true;
    }
    parse_request_2_candidate(body, with_cookie, false)
}

fn decode_reply_2(
    src: &mut impl Buf,
    expected_magic: Magic,
) -> Result<OpenConnectionReply2, DecodeError> {
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let server_guid = u64::decode_raknet(src)?;
    let server_addr = SocketAddr::decode_raknet(src)?;
    let mtu = u16::decode_raknet(src)?;
    let use_encryption = bool::decode_raknet(src)?;

    Ok(OpenConnectionReply2 {
        server_guid,
        server_addr,
        mtu,
        use_encryption,
        magic,
    })
}

fn decode_incompatible(
    src: &mut impl Buf,
    expected_magic: Magic,
) -> Result<IncompatibleProtocolVersion, DecodeError> {
    let protocol_version = u8::decode_raknet(src)?;
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let server_guid = u64::decode_raknet(src)?;

    Ok(IncompatibleProtocolVersion {
        protocol_version,
        server_guid,
        magic,
    })
}

fn decode_reject_packet(
    src: &mut impl Buf,
    expected_magic: Magic,
) -> Result<(Magic, u64), DecodeError> {
    let magic = validate_magic(Magic::decode_raknet(src)?, expected_magic)?;
    let server_guid = u64::decode_raknet(src)?;
    Ok((magic, server_guid))
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::{
        ConnectionBanned, DEFAULT_UNCONNECTED_MAGIC, MAX_UNCONNECTED_PONG_MOTD_BYTES,
        NoFreeIncomingConnections, OfflinePacket, OpenConnectionRequest1, OpenConnectionRequest2,
        Request2ParsePath, UnconnectedPong,
    };
    use crate::error::{DecodeError, EncodeError};

    fn roundtrip(packet: OfflinePacket) -> OfflinePacket {
        let mut buf = BytesMut::new();
        packet.encode(&mut buf).expect("encode must succeed");
        let mut src = &buf[..];
        OfflinePacket::decode(&mut src).expect("decode must succeed")
    }

    #[test]
    fn no_free_incoming_connections_roundtrip() {
        let packet = OfflinePacket::NoFreeIncomingConnections(NoFreeIncomingConnections {
            server_guid: 0xAA11_BB22_CC33_DD44,
            magic: DEFAULT_UNCONNECTED_MAGIC,
        });
        let decoded = roundtrip(packet);
        match decoded {
            OfflinePacket::NoFreeIncomingConnections(p) => {
                assert_eq!(p.server_guid, 0xAA11_BB22_CC33_DD44);
                assert_eq!(p.magic, DEFAULT_UNCONNECTED_MAGIC);
            }
            _ => panic!("unexpected packet variant"),
        }
    }

    #[test]
    fn connection_banned_roundtrip() {
        let packet = OfflinePacket::ConnectionBanned(ConnectionBanned {
            server_guid: 0x1020_3040_5060_7080,
            magic: DEFAULT_UNCONNECTED_MAGIC,
        });
        let decoded = roundtrip(packet);
        match decoded {
            OfflinePacket::ConnectionBanned(p) => {
                assert_eq!(p.server_guid, 0x1020_3040_5060_7080);
                assert_eq!(p.magic, DEFAULT_UNCONNECTED_MAGIC);
            }
            _ => panic!("unexpected packet variant"),
        }
    }

    #[test]
    fn open_connection_request2_without_cookie_prefers_strict_no_cookie_path() {
        let packet = OfflinePacket::OpenConnectionRequest2(OpenConnectionRequest2 {
            server_addr: "127.0.0.1:19132".parse().expect("valid socket addr"),
            mtu: 1400,
            client_guid: 0x11_22_33_44_55_66_77_88,
            cookie: None,
            client_proof: false,
            parse_path: Request2ParsePath::StrictNoCookie,
            magic: DEFAULT_UNCONNECTED_MAGIC,
        });
        let decoded = roundtrip(packet);
        match decoded {
            OfflinePacket::OpenConnectionRequest2(p) => {
                assert_eq!(p.cookie, None);
                assert_eq!(p.parse_path, Request2ParsePath::StrictNoCookie);
            }
            _ => panic!("unexpected packet variant"),
        }
    }

    #[test]
    fn open_connection_request2_with_cookie_prefers_strict_cookie_path() {
        let packet = OfflinePacket::OpenConnectionRequest2(OpenConnectionRequest2 {
            server_addr: "127.0.0.1:19132".parse().expect("valid socket addr"),
            mtu: 1400,
            client_guid: 0x88_77_66_55_44_33_22_11,
            cookie: Some(0xAABB_CCDD),
            client_proof: true,
            parse_path: Request2ParsePath::StrictWithCookie,
            magic: DEFAULT_UNCONNECTED_MAGIC,
        });
        let decoded = roundtrip(packet);
        match decoded {
            OfflinePacket::OpenConnectionRequest2(p) => {
                assert_eq!(p.cookie, Some(0xAABB_CCDD));
                assert!(p.client_proof);
                assert_eq!(p.parse_path, Request2ParsePath::StrictWithCookie);
            }
            _ => panic!("unexpected packet variant"),
        }
    }

    #[test]
    fn open_connection_request2_legacy_fallback_accepts_non_boolean_proof_byte() {
        let packet = OfflinePacket::OpenConnectionRequest2(OpenConnectionRequest2 {
            server_addr: "127.0.0.1:19132".parse().expect("valid socket addr"),
            mtu: 1400,
            client_guid: 0xAB_CD_EF_01_23_45_67_89,
            cookie: Some(0xAABB_CCDD),
            client_proof: true,
            parse_path: Request2ParsePath::StrictWithCookie,
            magic: DEFAULT_UNCONNECTED_MAGIC,
        });
        let mut buf = BytesMut::new();
        packet.encode(&mut buf).expect("encode must succeed");

        let proof_idx = 1 + 16 + 4;
        buf[proof_idx] = 2;

        let mut src = &buf[..];
        let decoded = OfflinePacket::decode(&mut src).expect("decode must succeed");
        match decoded {
            OfflinePacket::OpenConnectionRequest2(p) => {
                assert_eq!(p.parse_path, Request2ParsePath::LegacyHeuristic);
                assert_eq!(p.cookie, Some(0xAABB_CCDD));
                assert!(!p.client_proof);
            }
            _ => panic!("unexpected packet variant"),
        }
    }

    #[test]
    fn unconnected_pong_encode_rejects_oversized_motd() {
        let oversized = vec![b'a'; MAX_UNCONNECTED_PONG_MOTD_BYTES + 1];
        let packet = OfflinePacket::UnconnectedPong(UnconnectedPong {
            ping_time: 1,
            server_guid: 2,
            magic: DEFAULT_UNCONNECTED_MAGIC,
            motd: oversized.into(),
        });
        let mut buf = BytesMut::new();
        let err = packet
            .encode(&mut buf)
            .expect_err("oversized motd must be rejected");
        assert!(matches!(err, EncodeError::OfflinePongMotdTooLong(_)));
    }

    #[test]
    fn decode_with_custom_magic_accepts_matching_packet() {
        let custom_magic = [
            0x13, 0x57, 0x9B, 0xDF, 0x24, 0x68, 0xAC, 0xF0, 0x10, 0x32, 0x54, 0x76, 0x98, 0xBA,
            0xDC, 0xFE,
        ];
        let packet = OfflinePacket::OpenConnectionRequest1(OpenConnectionRequest1 {
            protocol_version: 10,
            mtu: 1400,
            magic: custom_magic,
        });
        let mut buf = BytesMut::new();
        packet.encode(&mut buf).expect("encode must succeed");

        let mut src = &buf[..];
        let decoded = OfflinePacket::decode_with_magic(&mut src, custom_magic)
            .expect("decode must accept matching custom magic");
        match decoded {
            OfflinePacket::OpenConnectionRequest1(req1) => {
                assert_eq!(req1.magic, custom_magic);
                assert_eq!(req1.mtu, 1400);
            }
            _ => panic!("unexpected packet variant"),
        }
    }

    #[test]
    fn decode_with_custom_magic_rejects_mismatch() {
        let packet_magic = [
            0x13, 0x57, 0x9B, 0xDF, 0x24, 0x68, 0xAC, 0xF0, 0x10, 0x32, 0x54, 0x76, 0x98, 0xBA,
            0xDC, 0xFE,
        ];
        let expected_magic = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ];
        let packet = OfflinePacket::OpenConnectionRequest1(OpenConnectionRequest1 {
            protocol_version: 10,
            mtu: 1400,
            magic: packet_magic,
        });
        let mut buf = BytesMut::new();
        packet.encode(&mut buf).expect("encode must succeed");

        let mut src = &buf[..];
        let err = OfflinePacket::decode_with_magic(&mut src, expected_magic)
            .expect_err("decode must reject mismatched magic");
        assert!(matches!(err, DecodeError::InvalidMagic));
    }
}
