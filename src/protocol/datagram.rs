use bytes::{Buf, BufMut};

use crate::error::{DecodeError, EncodeError};

use super::ack::AckNackPayload;
use super::codec::RaknetCodec;
use super::constants::{DatagramFlags, RAKNET_DATAGRAM_HEADER_SIZE};
use super::frame::Frame;
use super::sequence24::Sequence24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatagramKind {
    Data,
    Ack,
    Nack,
}

#[derive(Debug, Clone)]
pub enum DatagramPayload {
    Frames(Vec<Frame>),
    Ack(AckNackPayload),
    Nack(AckNackPayload),
}

impl DatagramPayload {
    fn kind(&self) -> DatagramKind {
        match self {
            Self::Frames(_) => DatagramKind::Data,
            Self::Ack(_) => DatagramKind::Ack,
            Self::Nack(_) => DatagramKind::Nack,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DatagramHeader {
    pub flags: DatagramFlags,
    pub sequence: Sequence24,
}

impl RaknetCodec for DatagramHeader {
    fn encode_raknet(&self, dst: &mut impl BufMut) -> Result<(), EncodeError> {
        self.flags.bits().encode_raknet(dst)?;
        self.sequence.encode_raknet(dst)
    }

    fn decode_raknet(src: &mut impl Buf) -> Result<Self, DecodeError> {
        if src.remaining() < 4 {
            return Err(DecodeError::UnexpectedEof);
        }

        let raw_flags = u8::decode_raknet(src)?;
        let (flags, kind) = decode_datagram_flags(raw_flags)?;
        if kind != DatagramKind::Data {
            return Err(DecodeError::InvalidDatagramFlags(raw_flags));
        }
        let sequence = Sequence24::decode_raknet(src)?;

        Ok(Self { flags, sequence })
    }
}

#[derive(Debug, Clone)]
pub struct Datagram {
    pub header: DatagramHeader,
    pub payload: DatagramPayload,
}

impl Datagram {
    pub fn encoded_size(&self) -> usize {
        match &self.payload {
            DatagramPayload::Frames(frames) => {
                RAKNET_DATAGRAM_HEADER_SIZE + frames.iter().map(Frame::encoded_size).sum::<usize>()
            }
            DatagramPayload::Ack(payload) | DatagramPayload::Nack(payload) => {
                1 + payload.encoded_size()
            }
        }
    }

    pub fn encode(&self, dst: &mut impl BufMut) -> Result<(), EncodeError> {
        validate_flags_for_payload(self.header.flags, &self.payload)?;

        match &self.payload {
            DatagramPayload::Frames(frames) => {
                self.header.encode_raknet(dst)?;
                for frame in frames {
                    frame.encode_raknet(dst)?;
                }
            }
            DatagramPayload::Ack(payload) | DatagramPayload::Nack(payload) => {
                self.header.flags.bits().encode_raknet(dst)?;
                payload.encode_raknet(dst)?;
            }
        }
        Ok(())
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, DecodeError> {
        if !src.has_remaining() {
            return Err(DecodeError::UnexpectedEof);
        }

        let raw_flags = src.get_u8();
        let (flags, kind) = decode_datagram_flags(raw_flags)?;

        match kind {
            DatagramKind::Ack => Ok(Self {
                header: DatagramHeader {
                    flags,
                    sequence: Sequence24::new(0),
                },
                payload: DatagramPayload::Ack(AckNackPayload::decode_raknet(src)?),
            }),
            DatagramKind::Nack => Ok(Self {
                header: DatagramHeader {
                    flags,
                    sequence: Sequence24::new(0),
                },
                payload: DatagramPayload::Nack(AckNackPayload::decode_raknet(src)?),
            }),
            DatagramKind::Data => {
                let sequence = Sequence24::decode_raknet(src)?;
                let header = DatagramHeader { flags, sequence };

                let mut frames = Vec::new();
                while src.has_remaining() {
                    frames.push(Frame::decode_raknet(src)?);
                }

                Ok(Self {
                    header,
                    payload: DatagramPayload::Frames(frames),
                })
            }
        }
    }
}

fn decode_datagram_flags(raw_flags: u8) -> Result<(DatagramFlags, DatagramKind), DecodeError> {
    // Roblox uses the high 3 bits to distinguish datagram types:
    //   0x80..=0x8F, 0x84..=0x9F etc. = data datagram (high bit set, not ACK/NACK)
    //   0xC0 = ACK
    //   0xA0 = NACK
    //   0xD0 = client ACK (Roblox-specific, treat as ACK)
    // The lower bits may carry additional info and are NOT standard bitflags,
    // so we use truncate instead of from_bits to accept Roblox's encoding.
    let flags = DatagramFlags::from_bits_truncate(raw_flags);

    if raw_flags & 0x80 == 0 {
        return Err(DecodeError::InvalidDatagramFlags(raw_flags));
    }

    let has_ack = raw_flags == 0xC0 || raw_flags == 0xD0;
    let has_nack = raw_flags == 0xA0;

    if has_ack {
        return Ok((flags, DatagramKind::Ack));
    }
    if has_nack {
        return Ok((flags, DatagramKind::Nack));
    }

    Ok((flags, DatagramKind::Data))
}

fn validate_flags_for_payload(
    flags: DatagramFlags,
    payload: &DatagramPayload,
) -> Result<(), EncodeError> {
    let raw_flags = flags.bits();
    let (_, decoded_kind) = decode_datagram_flags(raw_flags)
        .map_err(|_| EncodeError::InvalidDatagramFlags(raw_flags))?;

    if decoded_kind != payload.kind() {
        return Err(EncodeError::InvalidDatagramFlags(raw_flags));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ack::SequenceRange;

    fn sample_ack_payload() -> AckNackPayload {
        AckNackPayload {
            ranges: vec![SequenceRange {
                start: Sequence24::new(7),
                end: Sequence24::new(7),
            }],
        }
    }

    #[test]
    fn decode_accepts_roblox_datagram_0x83() {
        // Roblox sends datagrams with flags like 0x83, 0x84 etc.
        // These should be accepted as data datagrams.
        let mut src = &b"\x83\x00\x00\x00"[..];
        // Will fail due to no frame data after header, but flags should be accepted.
        let result = Datagram::decode(&mut src);
        // With 4 bytes: flags(1) + seq(3) = header only, no frames → Ok with empty frames
        assert!(result.is_ok(), "0x83 should be accepted as data datagram");
    }

    #[test]
    fn decode_rejects_no_high_bit() {
        let mut src = &b"\x40\0\0"[..];
        let err = Datagram::decode(&mut src).expect_err("no high bit must be rejected");
        assert!(matches!(err, DecodeError::InvalidDatagramFlags(0x40)));
    }

    #[test]
    fn decode_accepts_valid_ack_flags() {
        let payload = sample_ack_payload();
        let mut encoded = Vec::new();
        payload
            .encode_raknet(&mut encoded)
            .expect("ack payload encode");

        let mut src = vec![0xC0];
        src.extend(encoded);
        let decoded = Datagram::decode(&mut src.as_slice()).expect("ack datagram should decode");

        match decoded.payload {
            DatagramPayload::Ack(decoded_payload) => assert_eq!(decoded_payload, payload),
            other => panic!("unexpected payload: {other:?}"),
        }
    }
}
