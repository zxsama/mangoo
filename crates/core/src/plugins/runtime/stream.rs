use super::frame::{Frame, FrameKind, ProtocolError};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    Host,
    Plugin,
}

impl Peer {
    pub fn owns(self, id: u64) -> bool {
        id != 0
            && match self {
                Self::Host => id % 2 == 1,
                Self::Plugin => id % 2 == 0,
            }
    }
}

#[derive(Debug)]
struct State {
    declared: u64,
    expected_sha256: [u8; 32],
    credit: u64,
    transferred: u64,
    next_sequence: u32,
    terminal: bool,
    hasher: Sha256,
}

/// State checker for unidirectional streams owned by `owner`.
pub struct Streams {
    owner: Peer,
    next_id: u64,
    streams: HashMap<u64, State>,
    used_ids: HashSet<u64>,
    max_streams: usize,
    max_object_bytes: u64,
}

impl Streams {
    pub fn new(owner: Peer, max_streams: usize) -> Self {
        Self::with_object_limit(owner, max_streams, u64::MAX)
    }
    pub fn with_object_limit(owner: Peer, max_streams: usize, max_object_bytes: u64) -> Self {
        Self {
            owner,
            next_id: if owner == Peer::Host { 1 } else { 2 },
            streams: HashMap::new(),
            used_ids: HashSet::new(),
            max_streams,
            max_object_bytes,
        }
    }
    pub fn allocate(&mut self, declared: u64, sha256: [u8; 32]) -> Result<u64, ProtocolError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(2)
            .ok_or(ProtocolError::BufferLimit)?;
        self.register(id, declared, sha256)?;
        Ok(id)
    }
    pub fn register(
        &mut self,
        id: u64,
        declared: u64,
        sha256: [u8; 32],
    ) -> Result<(), ProtocolError> {
        if !self.owner.owns(id) || self.streams.len() >= self.max_streams {
            return Err(ProtocolError::InvalidStreamId);
        }
        if declared > self.max_object_bytes {
            return Err(ProtocolError::BufferLimit);
        }
        if !self.used_ids.insert(id) {
            return Err(ProtocolError::InvalidStreamId);
        }
        self.streams.insert(
            id,
            State {
                declared,
                expected_sha256: sha256,
                credit: 0,
                transferred: 0,
                next_sequence: 1,
                terminal: false,
                hasher: Sha256::new(),
            },
        );
        Ok(())
    }
    /// Equal cumulative credit is idempotent. Regression and credit beyond the
    /// declared size are protocol errors.
    pub fn grant_credit(&mut self, id: u64, cumulative: u64) -> Result<(), ProtocolError> {
        let state = self
            .streams
            .get_mut(&id)
            .ok_or(ProtocolError::InvalidStreamId)?;
        if state.terminal || cumulative < state.credit || cumulative > state.declared {
            return Err(ProtocolError::InvalidSequence);
        }
        state.credit = cumulative;
        Ok(())
    }
    pub fn credit(&self, id: u64) -> Result<u64, ProtocolError> {
        self.streams
            .get(&id)
            .map(|s| s.credit)
            .ok_or(ProtocolError::InvalidStreamId)
    }
    pub fn transferred(&self, id: u64) -> Result<u64, ProtocolError> {
        self.streams
            .get(&id)
            .map(|s| s.transferred)
            .ok_or(ProtocolError::InvalidStreamId)
    }
    pub fn next_sequence(&self, id: u64) -> Result<u32, ProtocolError> {
        self.streams
            .get(&id)
            .map(|state| state.next_sequence)
            .ok_or(ProtocolError::InvalidStreamId)
    }
    pub fn accept_data(&mut self, frame: &Frame) -> Result<(), ProtocolError> {
        let state = self
            .streams
            .get_mut(&frame.stream_id)
            .ok_or(ProtocolError::InvalidStreamId)?;
        if state.terminal
            || frame.kind != FrameKind::Binary
            || frame.sequence != state.next_sequence
        {
            return Err(ProtocolError::InvalidSequence);
        }
        let end = state
            .transferred
            .checked_add(frame.payload.len() as u64)
            .ok_or(ProtocolError::BufferLimit)?;
        if end > state.credit || end > state.declared {
            return Err(ProtocolError::BufferLimit);
        }
        state.hasher.update(&frame.payload);
        state.transferred = end;
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or(ProtocolError::InvalidSequence)?;
        Ok(())
    }
    pub fn finish(
        &mut self,
        frame: &Frame,
        final_size: u64,
        final_sha256: [u8; 32],
    ) -> Result<(), ProtocolError> {
        let state = self
            .streams
            .get_mut(&frame.stream_id)
            .ok_or(ProtocolError::InvalidStreamId)?;
        if state.terminal
            || frame.kind != FrameKind::End
            || frame.sequence != state.next_sequence
            || final_size != state.declared
            || final_size != state.transferred
            || final_sha256 != state.expected_sha256
        {
            return Err(ProtocolError::InvalidSequence);
        }
        let actual: [u8; 32] = state.hasher.clone().finalize().into();
        if actual != final_sha256 {
            return Err(ProtocolError::InvalidSequence);
        }
        state.terminal = true;
        Ok(())
    }
    pub fn abort(&mut self, frame: &Frame) -> Result<(), ProtocolError> {
        let state = self
            .streams
            .get_mut(&frame.stream_id)
            .ok_or(ProtocolError::InvalidStreamId)?;
        if state.terminal || frame.kind != FrameKind::Abort || frame.sequence != state.next_sequence
        {
            return Err(ProtocolError::InvalidSequence);
        }
        state.terminal = true;
        Ok(())
    }
    pub fn retire(&mut self, id: u64) -> Result<(), ProtocolError> {
        let state = self
            .streams
            .get(&id)
            .ok_or(ProtocolError::InvalidStreamId)?;
        if !state.terminal {
            return Err(ProtocolError::InvalidSequence);
        }
        self.streams.remove(&id);
        Ok(())
    }
    pub fn discard(&mut self, id: u64) {
        self.streams.remove(&id);
    }
    pub fn owner(&self) -> Peer {
        self.owner
    }
}

pub fn encode_credit(cumulative: u64) -> Vec<u8> {
    cumulative.to_le_bytes().to_vec()
}
pub fn decode_credit(payload: &[u8]) -> Result<u64, ProtocolError> {
    Ok(u64::from_le_bytes(
        payload
            .try_into()
            .map_err(|_| ProtocolError::InvalidSequence)?,
    ))
}
pub fn encode_end(size: u64, sha256: [u8; 32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(40);
    payload.extend_from_slice(&size.to_le_bytes());
    payload.extend_from_slice(&sha256);
    payload
}
pub fn decode_end(payload: &[u8]) -> Result<(u64, [u8; 32]), ProtocolError> {
    if payload.len() != 40 {
        return Err(ProtocolError::InvalidSequence);
    }
    Ok((
        u64::from_le_bytes(payload[..8].try_into().expect("length checked")),
        payload[8..].try_into().expect("length checked"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hash(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }
    #[test]
    fn parity_no_reuse_and_cumulative_credit() {
        let digest = hash(b"data");
        let mut host = Streams::new(Peer::Host, 2);
        let mut plugin = Streams::new(Peer::Plugin, 2);
        assert_eq!(host.allocate(4, digest).unwrap(), 1);
        assert_eq!(plugin.allocate(4, digest).unwrap(), 2);
        assert!(host.register(1, 4, digest).is_err());
        host.grant_credit(1, 4).unwrap();
        host.grant_credit(1, 4).unwrap();
        assert!(host.grant_credit(1, 3).is_err());
        assert!(host.grant_credit(1, 5).is_err());
    }
    #[test]
    fn sequence_credit_terminal_size_and_hash() {
        let digest = hash(b"data");
        let mut streams = Streams::new(Peer::Plugin, 2);
        streams.register(2, 4, digest).unwrap();
        streams.grant_credit(2, 2).unwrap();
        streams
            .accept_data(&Frame {
                kind: FrameKind::Binary,
                stream_id: 2,
                sequence: 1,
                payload: b"da".to_vec(),
            })
            .unwrap();
        assert!(streams
            .accept_data(&Frame {
                kind: FrameKind::Binary,
                stream_id: 2,
                sequence: 3,
                payload: b"ta".to_vec()
            })
            .is_err());
        streams.grant_credit(2, 4).unwrap();
        streams
            .accept_data(&Frame {
                kind: FrameKind::Binary,
                stream_id: 2,
                sequence: 2,
                payload: b"ta".to_vec(),
            })
            .unwrap();
        let end = Frame {
            kind: FrameKind::End,
            stream_id: 2,
            sequence: 3,
            payload: encode_end(4, digest),
        };
        streams.finish(&end, 4, digest).unwrap();
        assert!(streams.finish(&end, 4, digest).is_err());
    }
}
