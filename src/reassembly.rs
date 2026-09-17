use crate::protocol::MAX_FRAME_BYTES;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub const UDP_PAYLOAD: usize = 1200;
pub const MAX_INFLIGHT_FRAMES: usize = 6;
pub const FRAME_TIMEOUT: Duration = Duration::from_millis(750);

struct PendingFrame {
    created: Instant,
    chunks: Vec<Option<Vec<u8>>>,
    received: usize,
    bytes: usize,
    expected: usize,
}

#[derive(Default)]
pub struct Reassembler {
    frames: HashMap<u64, PendingFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassemblyError {
    TooManyChunks,
    InvalidChunk,
    FrameTooLarge,
    SizeMismatch,
}

impl Reassembler {
    pub fn expire(&mut self, now: Instant) {
        self.frames
            .retain(|_, f| now.duration_since(f.created) <= FRAME_TIMEOUT);
    }

    pub fn push(
        &mut self,
        frame_id: u64,
        chunk_index: u32,
        chunk_count: u32,
        expected: usize,
        data: &[u8],
        now: Instant,
    ) -> Result<Option<Vec<u8>>, ReassemblyError> {
        if expected > MAX_FRAME_BYTES {
            return Err(ReassemblyError::FrameTooLarge);
        }
        if chunk_count == 0
            || chunk_count as usize > MAX_FRAME_BYTES.div_ceil(UDP_PAYLOAD - 28) + 1
            || chunk_index >= chunk_count
        {
            return Err(ReassemblyError::TooManyChunks);
        }
        self.expire(now);
        while self
            .frames
            .values()
            .map(|f| f.bytes)
            .sum::<usize>()
            .saturating_add(data.len())
            > MAX_FRAME_BYTES
        {
            let Some(oldest) = self
                .frames
                .iter()
                .min_by_key(|(_, f)| f.created)
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.frames.remove(&oldest);
        }
        if !self.frames.contains_key(&frame_id)
            && self.frames.len() >= MAX_INFLIGHT_FRAMES
            && let Some(oldest) = self
                .frames
                .iter()
                .min_by_key(|(_, f)| f.created)
                .map(|(id, _)| *id)
        {
            self.frames.remove(&oldest);
        }
        let frame = self.frames.entry(frame_id).or_insert_with(|| PendingFrame {
            created: now,
            chunks: vec![None; chunk_count as usize],
            received: 0,
            bytes: 0,
            expected,
        });
        if frame.chunks.len() != chunk_count as usize || frame.expected != expected {
            self.frames.remove(&frame_id);
            return Err(ReassemblyError::InvalidChunk);
        }
        if frame.chunks[chunk_index as usize].is_none() {
            frame.bytes = frame.bytes.saturating_add(data.len());
            if frame.bytes > frame.expected || data.len() > UDP_PAYLOAD {
                self.frames.remove(&frame_id);
                return Err(ReassemblyError::FrameTooLarge);
            }
            frame.chunks[chunk_index as usize] = Some(data.to_vec());
            frame.received += 1;
        }
        if frame.received != frame.chunks.len() {
            return Ok(None);
        }
        let mut completed = Vec::with_capacity(frame.bytes);
        for chunk in &frame.chunks {
            completed.extend_from_slice(chunk.as_ref().unwrap());
        }
        let expected = frame.expected;
        self.frames.remove(&frame_id);
        if completed.len() != expected {
            return Err(ReassemblyError::SizeMismatch);
        }
        Ok(Some(completed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_out_of_order_large_frame() {
        let now = Instant::now();
        let mut r = Reassembler::default();
        assert_eq!(r.push(7, 1, 2, 6, b"def", now).unwrap(), None);
        assert_eq!(
            r.push(7, 0, 2, 6, b"abc", now).unwrap(),
            Some(b"abcdef".to_vec())
        );
    }
}
