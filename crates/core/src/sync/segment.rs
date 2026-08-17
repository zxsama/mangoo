use serde::{Deserialize, Serialize};

use super::codec::{
    canonical_bytes, sha256, CodecError, MAX_SEGMENT_DECODED_BYTES, MAX_SEGMENT_EVENTS,
};
use super::types::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SegmentHeader {
    pub library_uuid: LibraryUuid,
    pub stream_kind: StreamKind,
    pub actor: ActorId,
    pub first_seq: u64,
    pub last_seq: u64,
    pub count: u32,
    pub predecessor_hash: Option<[u8; 32]>,
    pub wire_version: u16,
    pub reducer_version: u16,
    pub decoded_bytes: u64,
    pub payload_hash: [u8; 32],
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SealedSegment {
    pub header: SegmentHeader,
    pub events: Vec<EventEnvelope>,
    pub segment_hash: [u8; 32],
}

pub fn seal_segment(
    events: Vec<EventEnvelope>,
    predecessor_hash: Option<[u8; 32]>,
) -> Result<SealedSegment, CodecError> {
    let first = events
        .first()
        .ok_or_else(|| CodecError::Decode("empty segment".into()))?;
    if events.len() > MAX_SEGMENT_EVENTS {
        return Err(CodecError::TooLarge {
            actual: events.len(),
            limit: MAX_SEGMENT_EVENTS,
        });
    }
    for (offset, event) in events.iter().enumerate() {
        super::codec::verify_event(event)?;
        if event.library_uuid != first.library_uuid
            || event.stream_kind != first.stream_kind
            || event.actor != first.actor
            || event.actor_seq != first.actor_seq + offset as u64
        {
            return Err(CodecError::Decode(
                "segment actor/range is not contiguous".into(),
            ));
        }
    }
    let payload = canonical_bytes(&events)?;
    if payload.len() > MAX_SEGMENT_DECODED_BYTES {
        return Err(CodecError::TooLarge {
            actual: payload.len(),
            limit: MAX_SEGMENT_DECODED_BYTES,
        });
    }
    let header = SegmentHeader {
        library_uuid: first.library_uuid,
        stream_kind: first.stream_kind,
        actor: first.actor.clone(),
        first_seq: first.actor_seq,
        last_seq: events.last().unwrap().actor_seq,
        count: events.len() as u32,
        predecessor_hash,
        wire_version: first.wire_version,
        reducer_version: first.reducer_version,
        decoded_bytes: payload.len() as u64,
        payload_hash: sha256(&payload),
    };
    let segment_hash = sha256(&canonical_bytes(&(&header, &events))?);
    Ok(SealedSegment {
        header,
        events,
        segment_hash,
    })
}

pub fn verify_segment(
    segment: &SealedSegment,
    expected_predecessor: Option<[u8; 32]>,
) -> Result<(), CodecError> {
    if segment.header.predecessor_hash != expected_predecessor {
        return Err(CodecError::IntegrityMismatch);
    }
    let rebuilt = seal_segment(segment.events.clone(), expected_predecessor)?;
    if rebuilt.header != segment.header || rebuilt.segment_hash != segment.segment_hash {
        return Err(CodecError::IntegrityMismatch);
    }
    Ok(())
}
