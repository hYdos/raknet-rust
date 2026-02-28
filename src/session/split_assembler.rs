use std::collections::HashMap;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};

use crate::error::DecodeError;
use crate::protocol::frame::Frame;

struct SplitEntry {
    header: crate::protocol::frame_header::FrameHeader,
    reliable_index: Option<crate::protocol::sequence24::Sequence24>,
    sequence_index: Option<crate::protocol::sequence24::Sequence24>,
    ordering_index: Option<crate::protocol::sequence24::Sequence24>,
    ordering_channel: Option<u8>,
    part_count: u32,
    received: usize,
    parts: Vec<Option<Bytes>>,
    last_update: Instant,
}

pub struct SplitAssembler {
    entries: HashMap<u16, SplitEntry>,
    ttl: Duration,
    max_parts: u32,
    max_concurrent: usize,
}

impl SplitAssembler {
    pub fn new(ttl: Duration, max_parts: u32, max_concurrent: usize) -> Self {
        let effective_ttl = if ttl.is_zero() {
            Duration::from_secs(30)
        } else {
            ttl
        };

        Self {
            entries: HashMap::new(),
            ttl: effective_ttl,
            max_parts,
            max_concurrent,
        }
    }

    pub fn add(&mut self, frame: Frame, now: Instant) -> Result<Option<Frame>, DecodeError> {
        if !frame.header.is_split {
            return Ok(Some(frame));
        }

        let split = frame.split.as_ref().ok_or(DecodeError::MissingSplitInfo)?;
        if split.part_count == 0 {
            return Err(DecodeError::SplitCountZero);
        }
        if split.part_count > self.max_parts {
            return Err(DecodeError::SplitTooLarge);
        }
        if split.part_index >= split.part_count {
            return Err(DecodeError::SplitIndexOutOfRange);
        }

        if self.entries.len() >= self.max_concurrent && !self.entries.contains_key(&split.part_id) {
            return Err(DecodeError::SplitBufferFull);
        }

        let entry = self
            .entries
            .entry(split.part_id)
            .or_insert_with(|| SplitEntry {
                header: frame.header,
                reliable_index: frame.reliable_index,
                sequence_index: frame.sequence_index,
                ordering_index: frame.ordering_index,
                ordering_channel: frame.ordering_channel,
                part_count: split.part_count,
                received: 0,
                parts: vec![None; split.part_count as usize],
                last_update: now,
            });

        if entry.part_count != split.part_count {
            return Err(DecodeError::SplitCountMismatch);
        }
        if entry.parts.len() != split.part_count as usize {
            return Err(DecodeError::SplitCountMismatch);
        }

        let index = split.part_index as usize;
        if index >= entry.parts.len() {
            return Err(DecodeError::SplitIndexOutOfRange);
        }

        if entry.parts[index].is_some() {
            return Ok(None);
        }

        entry.parts[index] = Some(frame.payload.clone());
        entry.received += 1;
        entry.last_update = now;

        if entry.received != entry.parts.len() {
            return Ok(None);
        }

        let mut merged = BytesMut::new();
        for part in &entry.parts {
            let bytes = part.as_ref().ok_or(DecodeError::SplitCountMismatch)?;
            merged.extend_from_slice(bytes);
        }
        let payload = merged.freeze();

        let assembled = Frame {
            header: crate::protocol::frame_header::FrameHeader {
                reliability: entry.header.reliability,
                is_split: false,
                needs_bas: entry.header.needs_bas,
            },
            bit_length: (payload.len() as u16) << 3,
            reliable_index: entry.reliable_index,
            sequence_index: entry.sequence_index,
            ordering_index: entry.ordering_index,
            ordering_channel: entry.ordering_channel,
            split: None,
            payload,
        };

        self.entries.remove(&split.part_id);
        Ok(Some(assembled))
    }

    pub fn prune(&mut self, now: Instant) -> usize {
        let mut dropped = 0usize;
        self.entries.retain(|_, entry| {
            if now.duration_since(entry.last_update) >= self.ttl {
                dropped += 1;
                false
            } else {
                true
            }
        });
        dropped
    }
}
