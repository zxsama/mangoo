use std::collections::HashMap;
use std::io::{Read, Write};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::ProviderError;
use crate::MAX_OBJECT_BYTES;

const CONTROL: u8 = 1;
const BINARY: u8 = 2;
const END: u8 = 3;
const CREDIT: u8 = 4;
const ABORT: u8 = 5;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ControlRequest {
    pub envelope: Value,
    pub method: String,
    pub payload: Value,
}

pub struct RpcTransport<R: Read, W: Write> {
    input: R,
    output: W,
    incoming: HashMap<u64, (u64, [u8; 32])>,
    next_stream_id: u64,
}

impl<R: Read, W: Write> RpcTransport<R, W> {
    pub fn new(input: R, output: W) -> Self {
        Self {
            input,
            output,
            incoming: HashMap::new(),
            next_stream_id: 2,
        }
    }

    pub fn next_request(&mut self) -> Result<Option<ControlRequest>, ProviderError> {
        loop {
            let Some((kind, stream_id, sequence, bytes)) =
                read_frame(&mut self.input).map_err(io_error)?
            else {
                return Ok(None);
            };
            if kind != CONTROL || stream_id != 0 || sequence != 0 {
                return Err(ProviderError::protocol(
                    "unexpected frame before control request",
                ));
            }
            let envelope: Value = serde_json::from_slice(&bytes)
                .map_err(|_| ProviderError::protocol("malformed control frame"))?;
            let method = envelope
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if envelope.get("messageType").and_then(Value::as_str) == Some("cancel") {
                continue;
            }
            if envelope.get("messageType").and_then(Value::as_str) != Some("request") {
                return Err(ProviderError::protocol("unexpected control response"));
            }
            let payload = envelope.get("payload").cloned().unwrap_or(Value::Null);
            if method == "stream.open" {
                let id = payload
                    .get("streamId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ProviderError::protocol("missing stream id"))?;
                let size = payload
                    .get("size")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ProviderError::protocol("missing stream size"))?;
                let sha = parse_hash(
                    payload
                        .get("sha256")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ProviderError::protocol("missing stream hash"))?,
                )?;
                if id == 0 || id % 2 == 0 || size > MAX_OBJECT_BYTES {
                    return Err(ProviderError::protocol("invalid Host stream declaration"));
                }
                self.incoming.insert(id, (size, sha));
                write_frame(
                    &mut self.output,
                    CREDIT,
                    id,
                    0,
                    &size.min(8 * 1024 * 1024).to_le_bytes(),
                )
                .map_err(io_error)?;
                continue;
            }
            return Ok(Some(ControlRequest {
                envelope,
                method,
                payload,
            }));
        }
    }

    pub fn receive_host_stream(&mut self, id: u64) -> Result<Vec<u8>, ProviderError> {
        let declaration = self
            .incoming
            .remove(&id)
            .ok_or_else(|| ProviderError::protocol("Host stream was not opened"))?;
        let mut bytes = Vec::with_capacity(declaration.0.min(10 * 1024 * 1024) as usize);
        let mut expected_sequence = 1u32;
        let mut granted = declaration.0.min(8 * 1024 * 1024);
        loop {
            let (kind, stream_id, sequence, payload) = read_frame(&mut self.input)
                .map_err(io_error)?
                .ok_or_else(|| ProviderError::protocol("Host stream ended early"))?;
            if stream_id != id || sequence != expected_sequence {
                return Err(ProviderError::protocol("invalid Host stream sequence"));
            }
            match kind {
                BINARY => {
                    bytes.extend_from_slice(&payload);
                    if bytes.len() as u64 > declaration.0 {
                        return Err(ProviderError::protocol("Host stream exceeds declaration"));
                    }
                    if granted < declaration.0 && bytes.len() as u64 + 2 * 1024 * 1024 >= granted {
                        granted = declaration.0.min(granted.saturating_add(8 * 1024 * 1024));
                        write_frame(&mut self.output, CREDIT, id, 0, &granted.to_le_bytes())
                            .map_err(io_error)?;
                    }
                }
                END => {
                    if payload.len() != 40
                        || u64::from_le_bytes(
                            payload[..8]
                                .try_into()
                                .map_err(|_| ProviderError::protocol("invalid stream terminal"))?,
                        ) != declaration.0
                        || payload[8..] != declaration.1
                        || bytes.len() as u64 != declaration.0
                        || Sha256::digest(&bytes).as_slice() != declaration.1
                    {
                        return Err(ProviderError::protocol("invalid Host stream terminal"));
                    }
                    return Ok(bytes);
                }
                ABORT => return Err(ProviderError::new("cancelled", true, "Host aborted stream")),
                _ => return Err(ProviderError::protocol("unexpected Host stream frame")),
            }
            expected_sequence = expected_sequence.saturating_add(1);
        }
    }

    pub fn send_plugin_stream(&mut self, bytes: &[u8]) -> Result<u64, ProviderError> {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        self.send_plugin_reader(&mut std::io::Cursor::new(bytes), bytes.len() as u64, digest)
    }

    pub fn send_plugin_reader(
        &mut self,
        reader: &mut impl Read,
        size: u64,
        digest: [u8; 32],
    ) -> Result<u64, ProviderError> {
        if size > MAX_OBJECT_BYTES {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "object exceeds Provider limit",
            ));
        }
        let id = self.next_stream_id;
        self.next_stream_id = self
            .next_stream_id
            .checked_add(2)
            .ok_or_else(|| ProviderError::protocol("stream id exhausted"))?;
        let open = json!({"wireVersion":1,"messageType":"request","requestId":uuid::Uuid::now_v7(),"method":"stream.open","deadlineMs":0,"bindingEpoch":null,"transportGeneration":null,"payload":{"streamId":id,"size":size,"sha256":hex(&digest)}});
        self.write_control(&open)?;
        let mut granted = self.read_credit(id)?;
        let mut offset = 0u64;
        let mut sequence = 1u32;
        let mut buffer = vec![0u8; 1024 * 1024];
        while offset < size {
            if offset >= granted {
                granted = self.read_credit(id)?;
            }
            let allowed = (size - offset)
                .min(1024 * 1024)
                .min(granted.saturating_sub(offset)) as usize;
            if allowed == 0 {
                return Err(ProviderError::protocol("non-advancing stream credit"));
            }
            reader
                .read_exact(&mut buffer[..allowed])
                .map_err(io_error)?;
            write_frame(&mut self.output, BINARY, id, sequence, &buffer[..allowed])
                .map_err(io_error)?;
            offset += allowed as u64;
            sequence = sequence.saturating_add(1);
        }
        let mut terminal = Vec::with_capacity(40);
        terminal.extend_from_slice(&size.to_le_bytes());
        terminal.extend_from_slice(&digest);
        write_frame(&mut self.output, END, id, sequence, &terminal).map_err(io_error)?;
        Ok(id)
    }

    pub fn reverse_call(&mut self, method: &str, payload: Value) -> Result<Value, ProviderError> {
        let id = uuid::Uuid::now_v7();
        let request = json!({"wireVersion":1,"messageType":"request","requestId":id,"method":method,"deadlineMs":0,"bindingEpoch":null,"transportGeneration":null,"payload":payload});
        self.write_control(&request)?;
        loop {
            let (kind, stream_id, sequence, bytes) = read_frame(&mut self.input)
                .map_err(io_error)?
                .ok_or_else(|| ProviderError::protocol("Host closed reverse RPC"))?;
            if kind != CONTROL || stream_id != 0 || sequence != 0 {
                return Err(ProviderError::protocol(
                    "unexpected frame during reverse RPC",
                ));
            }
            let response: Value = serde_json::from_slice(&bytes)
                .map_err(|_| ProviderError::protocol("malformed reverse RPC response"))?;
            if response.get("requestId").and_then(Value::as_str) != Some(id.to_string().as_str()) {
                return Err(ProviderError::protocol("reverse RPC response id mismatch"));
            }
            return if response.get("messageType").and_then(Value::as_str) == Some("response") {
                Ok(response.get("payload").cloned().unwrap_or(Value::Null))
            } else {
                Err(ProviderError::new(
                    "permission_denied",
                    false,
                    response
                        .pointer("/payload/detail")
                        .and_then(Value::as_str)
                        .unwrap_or("Host secret operation failed"),
                ))
            };
        }
    }

    pub fn respond(
        &mut self,
        request: &ControlRequest,
        result: Result<Value, ProviderError>,
    ) -> Result<(), ProviderError> {
        let mut envelope = request.envelope.clone();
        match result {
            Ok(value) => {
                envelope["messageType"] = json!("response");
                envelope["payload"] = value;
            }
            Err(error) => {
                envelope["messageType"] = json!("error");
                envelope["payload"] = error.to_payload();
            }
        }
        self.write_control(&envelope)
    }

    fn read_credit(&mut self, id: u64) -> Result<u64, ProviderError> {
        let (kind, stream_id, _, bytes) = read_frame(&mut self.input)
            .map_err(io_error)?
            .ok_or_else(|| ProviderError::protocol("missing stream credit"))?;
        if kind != CREDIT || stream_id != id || bytes.len() != 8 {
            return Err(ProviderError::protocol("invalid stream credit"));
        }
        Ok(u64::from_le_bytes(bytes.as_slice().try_into().map_err(
            |_| ProviderError::protocol("invalid stream credit"),
        )?))
    }

    fn write_control(&mut self, value: &Value) -> Result<(), ProviderError> {
        let payload = serde_json::to_vec(value)
            .map_err(|_| ProviderError::protocol("control serialization failed"))?;
        write_frame(&mut self.output, CONTROL, 0, 0, &payload).map_err(io_error)
    }
}

fn read_frame(input: &mut impl Read) -> std::io::Result<Option<(u8, u64, u32, Vec<u8>)>> {
    let mut header = [0u8; 17];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(std::io::Error::other("oversize RPC frame"));
    }
    let mut payload = vec![0; length];
    input.read_exact(&mut payload)?;
    Ok(Some((
        header[4],
        u64::from_le_bytes(header[5..13].try_into().unwrap()),
        u32::from_le_bytes(header[13..17].try_into().unwrap()),
        payload,
    )))
}

fn write_frame(
    output: &mut impl Write,
    kind: u8,
    stream_id: u64,
    sequence: u32,
    payload: &[u8],
) -> std::io::Result<()> {
    output.write_all(&(payload.len() as u32).to_le_bytes())?;
    output.write_all(&[kind])?;
    output.write_all(&stream_id.to_le_bytes())?;
    output.write_all(&sequence.to_le_bytes())?;
    output.write_all(payload)?;
    output.flush()
}

fn parse_hash(value: &str) -> Result<[u8; 32], ProviderError> {
    if value.len() != 64 {
        return Err(ProviderError::protocol("invalid stream hash"));
    }
    let mut hash = [0; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| ProviderError::protocol("invalid stream hash"))?;
    }
    Ok(hash)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn io_error(_: std::io::Error) -> ProviderError {
    ProviderError::new("unavailable", true, "Provider transport failed")
}
