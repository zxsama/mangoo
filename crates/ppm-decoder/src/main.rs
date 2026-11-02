mod wire;

use mangoo_ppm_decoder::{
    decode_ppm, CHUNK_BYTES, MAX_HEADER_BYTES, MAX_INPUT_BYTES, MAX_OUTPUT_PIXELS,
    MAX_SOURCE_PIXELS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use wire::*;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Limits {
    max_control_bytes: u64,
    max_binary_bytes: u64,
    max_buffered_bytes: u64,
    stream_window_bytes: u64,
    max_concurrent_requests: u32,
    max_concurrent_streams: u32,
    max_object_bytes: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Hello {
    nonce: [u8; 32],
    wire_versions: Vec<u16>,
    plugin_id: String,
    version: String,
    capabilities: Vec<String>,
    granted_permissions: Vec<Value>,
    method_versions: BTreeMap<String, u16>,
    limits: Limits,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamOpen {
    stream_id: u64,
    size: u64,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecodeRequest {
    job_id: String,
    stream_id: u64,
    purpose: String,
    size: u64,
    sha256: String,
    max_edge: u32,
    max_output_pixels: u64,
    max_source_pixels: u64,
    max_header_bytes: u64,
}
struct Input {
    open: StreamOpen,
    file: File,
    written: u64,
    granted: u64,
    sequence: u32,
    hash: Sha256,
}

fn main() {
    if run().is_err() {
        std::process::exit(1);
    }
}
fn invalid() -> std::io::Error {
    std::io::Error::other("invalid decoder protocol")
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = std::io::stdin().lock();
    let mut writer = std::io::stdout().lock();
    let frame = read(&mut reader, CHUNK_BYTES)?;
    if frame.kind != CONTROL {
        return Err(invalid().into());
    }
    let request: Control = serde_json::from_slice(&frame.bytes)?;
    if request.method != "runtime.hello"
        || request.wire_version != 1
        || request.message_type != "request"
        || request.binding_epoch.is_some()
        || request.transport_generation.is_some()
    {
        return Err(invalid().into());
    }
    let hello: Hello = serde_json::from_value(request.payload.clone())?;
    let methods: BTreeMap<String, u16> = ["health", "diagnostics", "shutdown", "decoder.decode"]
        .into_iter()
        .map(|v| (v.to_owned(), 1))
        .collect();
    if hello.plugin_id != "com.mangoo.ppm-decoder"
        || hello.version != env!("CARGO_PKG_VERSION")
        || hello.wire_versions != [1]
        || hello.capabilities != ["file-decoder.v1"]
        || hello.method_versions != methods
        || hello.limits.max_control_bytes > MAX_HEADER_BYTES
        || hello.limits.max_binary_bytes == 0
        || hello.limits.max_binary_bytes > CHUNK_BYTES as u64
        || hello.limits.stream_window_bytes == 0
        || hello.limits.stream_window_bytes > 8 * 1024 * 1024
        || hello.limits.max_object_bytes > MAX_INPUT_BYTES
        || hello
            .granted_permissions
            .iter()
            .any(|permission| permission.get("kind") != Some(&json!("temp-storage")))
    {
        return Err(invalid().into());
    }
    respond(
        &mut writer,
        &request,
        json!({"nonce":hello.nonce,"wireVersion":1,"pluginId":hello.plugin_id,
        "version":hello.version,"capabilities":hello.capabilities,"methodVersions":hello.method_versions,"limits":hello.limits}),
        false,
    )?;
    let directory = std::env::var_os("MANGOO_DECODER_TEMP_DIR")
        .map(std::path::PathBuf::from)
        .ok_or_else(invalid)?;
    let window = hello.limits.stream_window_bytes;
    let chunk = hello.limits.max_binary_bytes as usize;
    let mut input: Option<Input> = None;
    let mut pending: Option<(Control, DecodeRequest)> = None;
    let mut last_input = 0;
    let mut next_output = 2u64;
    loop {
        let frame = match read(&mut reader, chunk) {
            Ok(frame) => frame,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        match frame.kind {
            CONTROL => {
                let request: Control = serde_json::from_slice(&frame.bytes)?;
                if request.wire_version != 1
                    || request.message_type != "request"
                    || request.binding_epoch.is_some()
                    || request.transport_generation.is_some()
                {
                    return Err(invalid().into());
                }
                match request.method.as_str() {
                    "stream.open" => {
                        let open: StreamOpen = serde_json::from_value(request.payload)?;
                        hash(&open.sha256)?;
                        if input.is_some()
                            || pending.is_some()
                            || open.stream_id % 2 != 1
                            || open.stream_id <= last_input
                            || open.size > hello.limits.max_object_bytes
                        {
                            return Err(invalid().into());
                        }
                        last_input = open.stream_id;
                        let granted = open.size.min(window);
                        write(
                            &mut writer,
                            Frame {
                                kind: CREDIT,
                                id: open.stream_id,
                                sequence: 0,
                                bytes: granted.to_le_bytes().to_vec(),
                            },
                        )?;
                        input = Some(Input {
                            open,
                            file: tempfile::tempfile_in(&directory)?,
                            written: 0,
                            granted,
                            sequence: 1,
                            hash: Sha256::new(),
                        });
                    }
                    "decoder.decode" => {
                        let decode: DecodeRequest =
                            serde_json::from_value(request.payload.clone())?;
                        let source = input.as_ref().ok_or_else(invalid)?;
                        if pending.is_some()
                            || decode.stream_id != source.open.stream_id
                            || decode.size != source.open.size
                            || decode.sha256 != source.open.sha256
                            || uuid::Uuid::parse_str(&decode.job_id).is_err()
                            || !matches!(decode.purpose.as_str(), "thumbnail" | "preview")
                            || decode.max_edge == 0
                            || decode.max_edge
                                > if decode.purpose == "thumbnail" {
                                    1024
                                } else {
                                    3072
                                }
                            || decode.max_output_pixels > MAX_OUTPUT_PIXELS
                            || decode.max_source_pixels > MAX_SOURCE_PIXELS
                            || decode.max_header_bytes > MAX_HEADER_BYTES
                        {
                            return Err(invalid().into());
                        }
                        pending = Some((request, decode));
                    }
                    "health" | "diagnostics" if input.is_none() => {
                        respond(&mut writer, &request, json!({"healthy":true}), false)?
                    }
                    "shutdown" => {
                        respond(&mut writer, &request, json!({}), false)?;
                        return Ok(());
                    }
                    _ => return Err(invalid().into()),
                }
            }
            BINARY => {
                let source = input.as_mut().ok_or_else(invalid)?;
                if frame.id != source.open.stream_id
                    || frame.sequence != source.sequence
                    || frame.bytes.is_empty()
                    || source.written + frame.bytes.len() as u64 > source.granted
                {
                    return Err(invalid().into());
                }
                source.file.write_all(&frame.bytes)?;
                source.hash.update(&frame.bytes);
                source.written += frame.bytes.len() as u64;
                source.sequence = source.sequence.checked_add(1).ok_or_else(invalid)?;
                let credit = source.open.size.min(source.written + window);
                if credit > source.granted {
                    source.granted = credit;
                    write(
                        &mut writer,
                        Frame {
                            kind: CREDIT,
                            id: frame.id,
                            sequence: 0,
                            bytes: credit.to_le_bytes().to_vec(),
                        },
                    )?;
                }
            }
            END => {
                let mut source = input.take().ok_or_else(invalid)?;
                if frame.id != source.open.stream_id
                    || frame.sequence != source.sequence
                    || frame.bytes.len() != 40
                    || source.written != source.open.size
                    || u64::from_le_bytes(frame.bytes[..8].try_into()?) != source.written
                    || frame.bytes[8..] != hash(&source.open.sha256)?
                    || source.hash.finalize().as_slice() != &frame.bytes[8..]
                {
                    return Err(invalid().into());
                }
                let (request, decode) = pending.take().ok_or_else(invalid)?;
                source.file.seek(SeekFrom::Start(0))?;
                match decode_ppm(
                    &mut source.file,
                    decode.max_edge,
                    decode.max_source_pixels,
                    decode.max_output_pixels,
                    decode.max_header_bytes,
                ) {
                    Ok(image) => {
                        let (width, height) = image.dimensions();
                        let rgba = image.into_raw();
                        let digest: [u8; 32] = Sha256::digest(&rgba).into();
                        let id = next_output;
                        next_output = next_output.checked_add(2).ok_or_else(invalid)?;
                        let open = Control {
                            wire_version: 1,
                            message_type: "request".into(),
                            request_id: uuid::Uuid::now_v7(),
                            method: "stream.open".into(),
                            deadline_ms: 0,
                            binding_epoch: None,
                            transport_generation: None,
                            payload: json!({"streamId":id,"size":rgba.len(),"sha256":hex(&digest)}),
                        };
                        control(&mut writer, &open)?;
                        respond(
                            &mut writer,
                            &request,
                            json!({"jobId":decode.job_id,"streamId":id,"width":width,"height":height,"format":"rgba8","size":rgba.len(),"sha256":hex(&digest)}),
                            false,
                        )?;
                        send_output(&mut reader, &mut writer, id, &rgba, digest, chunk)?;
                    }
                    Err(error) => respond(
                        &mut writer,
                        &request,
                        json!({"code":error.code(),"retryable":false,"detail":"PPM input cannot be decoded"}),
                        true,
                    )?,
                }
            }
            6 => write(
                &mut writer,
                Frame {
                    kind: 7,
                    id: 0,
                    sequence: 0,
                    bytes: frame.bytes,
                },
            )?,
            _ => return Err(invalid().into()),
        }
    }
}

fn send_output(
    reader: &mut impl Read,
    writer: &mut impl Write,
    id: u64,
    bytes: &[u8],
    digest: [u8; 32],
    chunk: usize,
) -> std::io::Result<()> {
    let mut offset = 0usize;
    let mut sequence = 1u32;
    let mut granted = 0usize;
    while offset < bytes.len() {
        if offset == granted {
            let credit = read(reader, chunk)?;
            if credit.kind != CREDIT || credit.id != id || credit.bytes.len() != 8 {
                return Err(invalid());
            }
            let next =
                u64::from_le_bytes(credit.bytes[..].try_into().map_err(|_| invalid())?) as usize;
            if next <= granted || next > bytes.len() {
                return Err(invalid());
            }
            granted = next;
        }
        let end = bytes.len().min(granted).min(offset + chunk);
        write(
            writer,
            Frame {
                kind: BINARY,
                id,
                sequence,
                bytes: bytes[offset..end].to_vec(),
            },
        )?;
        offset = end;
        sequence = sequence.checked_add(1).ok_or_else(invalid)?;
    }
    let mut end = (bytes.len() as u64).to_le_bytes().to_vec();
    end.extend_from_slice(&digest);
    write(
        writer,
        Frame {
            kind: END,
            id,
            sequence,
            bytes: end,
        },
    )
}
