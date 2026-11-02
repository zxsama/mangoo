use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::types::EventEnvelope;

pub const WIRE_VERSION: u16 = 1;
pub const ENVELOPE_VERSION: u16 = 1;
// v3 requires all-file collection; v2 immutable history remains readable.
pub const REDUCER_VERSION: u16 = 3;
pub const MIN_READ_REDUCER_VERSION: u16 = 2;
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_SEGMENT_EVENTS: usize = 256;
pub const MAX_SEGMENT_DECODED_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_APPLY_EVENTS: usize = 100;
pub const MAX_APPLY_DECODED_BYTES: usize = 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub enum CodecError {
    Serialize(String),
    Decode(String),
    TooLarge {
        actual: usize,
        limit: usize,
    },
    UnsupportedVersion {
        wire: u16,
        envelope: u16,
        reducer: u16,
    },
    IntegrityMismatch,
}
impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CodecError {}

fn sort_value(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, sort_value(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(sort_value).collect()),
        other => other,
    }
}

pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let value = serde_json::to_value(value).map_err(|e| CodecError::Serialize(e.to_string()))?;
    serde_json::to_vec(&sort_value(value)).map_err(|e| CodecError::Serialize(e.to_string()))
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn event_signing_bytes(event: &EventEnvelope) -> Result<Vec<u8>, CodecError> {
    let mut copy = event.clone();
    copy.integrity_hash = [0; 32];
    canonical_bytes(&copy)
}

pub fn seal_event(mut event: EventEnvelope) -> Result<EventEnvelope, CodecError> {
    let bytes = event_signing_bytes(&event)?;
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(CodecError::TooLarge {
            actual: bytes.len(),
            limit: MAX_EVENT_BYTES,
        });
    }
    event.integrity_hash = sha256(&bytes);
    Ok(event)
}

pub fn encode_event(event: &EventEnvelope) -> Result<Vec<u8>, CodecError> {
    verify_event(event)?;
    let bytes = canonical_bytes(event)?;
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(CodecError::TooLarge {
            actual: bytes.len(),
            limit: MAX_EVENT_BYTES,
        });
    }
    Ok(bytes)
}

pub fn decode_event(bytes: &[u8]) -> Result<EventEnvelope, CodecError> {
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(CodecError::TooLarge {
            actual: bytes.len(),
            limit: MAX_EVENT_BYTES,
        });
    }
    let event = serde_json::from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))?;
    verify_event(&event)?;
    Ok(event)
}

pub fn verify_event(event: &EventEnvelope) -> Result<(), CodecError> {
    if event.wire_version != WIRE_VERSION
        || event.envelope_version != ENVELOPE_VERSION
        || !(MIN_READ_REDUCER_VERSION..=REDUCER_VERSION).contains(&event.reducer_version)
    {
        return Err(CodecError::UnsupportedVersion {
            wire: event.wire_version,
            envelope: event.envelope_version,
            reducer: event.reducer_version,
        });
    }
    if event.actor_seq == 0 || event.stream_uuid != event.actor.stream_uuid {
        return Err(CodecError::Decode(
            "event actor sequence or stream identity is invalid".into(),
        ));
    }
    if let super::types::Operation::SetField { field, value } = &event.operation {
        if field == super::reducer::ROOT_IDENTITY_FIELD {
            super::reducer::decode_root_identity(event.entity_uuid, value)
                .map_err(|_| CodecError::Decode("invalid synchronized root identity".into()))?;
        }
    }
    if sha256(&event_signing_bytes(event)?) != event.integrity_hash {
        return Err(CodecError::IntegrityMismatch);
    }
    Ok(())
}
