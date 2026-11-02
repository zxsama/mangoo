use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};

pub const CONTROL: u8 = 1;
pub const BINARY: u8 = 2;
pub const END: u8 = 3;
pub const CREDIT: u8 = 4;
pub struct Frame {
    pub kind: u8,
    pub id: u64,
    pub sequence: u32,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub wire_version: u16,
    pub message_type: String,
    pub request_id: uuid::Uuid,
    pub method: String,
    pub deadline_ms: u64,
    pub binding_epoch: Option<String>,
    pub transport_generation: Option<u64>,
    pub payload: Value,
}
pub fn read(reader: &mut impl Read, maximum: usize) -> std::io::Result<Frame> {
    let mut header = [0u8; 17];
    reader.read_exact(&mut header)?;
    let count = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let kind = header[4];
    let id = u64::from_le_bytes(header[5..13].try_into().unwrap());
    let sequence = u32::from_le_bytes(header[13..17].try_into().unwrap());
    let limit = if kind == BINARY { maximum } else { 64 * 1024 };
    if count > limit
        || !matches!(kind, 1..=7)
        || (matches!(kind, 2..=5) != (id != 0))
        || (matches!(kind, 1 | 4 | 6 | 7) && sequence != 0)
        || (matches!(kind, 2 | 3 | 5) && sequence == 0)
    {
        return Err(std::io::Error::other("invalid frame"));
    }
    let mut bytes = vec![0; count];
    reader.read_exact(&mut bytes)?;
    Ok(Frame {
        kind,
        id,
        sequence,
        bytes,
    })
}
pub fn write(writer: &mut impl Write, frame: Frame) -> std::io::Result<()> {
    writer.write_all(&(frame.bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&[frame.kind])?;
    writer.write_all(&frame.id.to_le_bytes())?;
    writer.write_all(&frame.sequence.to_le_bytes())?;
    writer.write_all(&frame.bytes)?;
    writer.flush()
}
pub fn control(writer: &mut impl Write, envelope: &Control) -> std::io::Result<()> {
    write(
        writer,
        Frame {
            kind: CONTROL,
            id: 0,
            sequence: 0,
            bytes: serde_json::to_vec(envelope)?,
        },
    )
}
pub fn respond(
    writer: &mut impl Write,
    request: &Control,
    payload: Value,
    error: bool,
) -> std::io::Result<()> {
    let mut reply = request.clone();
    reply.message_type = if error { "error" } else { "response" }.into();
    reply.payload = payload;
    reply.deadline_ms = 0;
    control(writer, &reply)
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub fn hash(value: &str) -> Result<[u8; 32], std::io::Error> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(std::io::Error::other("invalid hash"));
    }
    let mut hash = [0; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(std::io::Error::other)?;
    }
    Ok(hash)
}
