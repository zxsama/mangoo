use std::collections::VecDeque;

pub const HEADER_LEN: usize = 17;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 1,
    Binary = 2,
    End = 3,
    Credit = 4,
    Abort = 5,
    Ping = 6,
    Pong = 7,
}
impl TryFrom<u8> for FrameKind {
    type Error = ProtocolError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Ok(match v {
            1 => Self::Control,
            2 => Self::Binary,
            3 => Self::End,
            4 => Self::Credit,
            5 => Self::Abort,
            6 => Self::Ping,
            7 => Self::Pong,
            _ => return Err(ProtocolError::UnknownKind),
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub stream_id: u64,
    pub sequence: u32,
    pub payload: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    UnknownKind,
    FrameTooLarge,
    InvalidStreamId,
    InvalidSequence,
    BufferLimit,
}
#[derive(Debug, Clone)]
pub struct FrameLimits {
    pub max_control: usize,
    pub max_binary: usize,
    pub max_buffered: usize,
    pub stream_window: usize,
    pub max_object: u64,
}
impl Default for FrameLimits {
    fn default() -> Self {
        Self {
            max_control: 1024 * 1024,
            max_binary: 4 * 1024 * 1024,
            max_buffered: 8 * 1024 * 1024,
            stream_window: 8 * 1024 * 1024,
            // Media data packs are bounded at 96 MiB. The RPC stream remains
            // windowed at 8 MiB, so this raises the permitted object envelope
            // without increasing the in-memory buffering budget.
            max_object: 96 * 1024 * 1024,
        }
    }
}
pub struct Decoder {
    buffer: VecDeque<u8>,
    limits: FrameLimits,
}
impl Decoder {
    pub fn new(limits: FrameLimits) -> Self {
        Self {
            buffer: VecDeque::new(),
            limits,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, ProtocolError> {
        if self.buffer.len() + bytes.len() > self.limits.max_buffered {
            return Err(ProtocolError::BufferLimit);
        }
        self.buffer.extend(bytes);
        let mut out = Vec::new();
        loop {
            if self.buffer.len() < HEADER_LEN {
                break;
            }
            let h: Vec<_> = self.buffer.iter().take(HEADER_LEN).copied().collect();
            let len = u32::from_le_bytes(h[0..4].try_into().unwrap()) as usize;
            let kind = FrameKind::try_from(h[4])?;
            let limit = if kind == FrameKind::Binary {
                self.limits.max_binary
            } else {
                self.limits.max_control
            };
            if len > limit {
                return Err(ProtocolError::FrameTooLarge);
            }
            if self.buffer.len() < HEADER_LEN + len {
                break;
            }
            let stream_id = u64::from_le_bytes(h[5..13].try_into().unwrap());
            let sequence = u32::from_le_bytes(h[13..17].try_into().unwrap());
            validate_header(kind, stream_id, sequence)?;
            self.buffer.drain(..HEADER_LEN);
            let payload = self.buffer.drain(..len).collect();
            out.push(Frame {
                kind,
                stream_id,
                sequence,
                payload,
            });
        }
        Ok(out)
    }
}
pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    validate_header(frame.kind, frame.stream_id, frame.sequence)?;
    if frame.payload.len() > u32::MAX as usize {
        return Err(ProtocolError::FrameTooLarge);
    }
    let mut out = Vec::with_capacity(HEADER_LEN + frame.payload.len());
    out.extend_from_slice(&(frame.payload.len() as u32).to_le_bytes());
    out.push(frame.kind as u8);
    out.extend_from_slice(&frame.stream_id.to_le_bytes());
    out.extend_from_slice(&frame.sequence.to_le_bytes());
    out.extend_from_slice(&frame.payload);
    Ok(out)
}
fn validate_header(kind: FrameKind, id: u64, seq: u32) -> Result<(), ProtocolError> {
    let stream = matches!(
        kind,
        FrameKind::Binary | FrameKind::End | FrameKind::Credit | FrameKind::Abort
    );
    if stream != (id != 0) {
        return Err(ProtocolError::InvalidStreamId);
    }
    if matches!(
        kind,
        FrameKind::Control | FrameKind::Credit | FrameKind::Ping | FrameKind::Pong
    ) && seq != 0
    {
        return Err(ProtocolError::InvalidSequence);
    }
    if matches!(kind, FrameKind::Binary | FrameKind::End | FrameKind::Abort) && seq == 0 {
        return Err(ProtocolError::InvalidSequence);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_and_coalesced() {
        let a = Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: 0,
            payload: b"one".to_vec(),
        };
        let b = Frame {
            kind: FrameKind::Binary,
            stream_id: 1,
            sequence: 1,
            payload: b"two".to_vec(),
        };
        let all = [encode(&a).unwrap(), encode(&b).unwrap()].concat();
        let mut d = Decoder::new(FrameLimits::default());
        assert!(d.push(&all[..5]).unwrap().is_empty());
        assert_eq!(d.push(&all[5..]).unwrap(), vec![a, b]);
    }
    #[test]
    fn rejects_stdout_noise_and_oversize() {
        let mut d = Decoder::new(FrameLimits {
            max_control: 1,
            max_binary: 1,
            max_buffered: 100,
            stream_window: 16,
            max_object: 100,
        });
        let f = Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: 0,
            payload: vec![1, 2],
        };
        assert_eq!(
            d.push(&encode(&f).unwrap()).unwrap_err(),
            ProtocolError::FrameTooLarge
        );
        assert!(d.push(&[0, 0, 0, 0, 99]).is_err());
    }
}
