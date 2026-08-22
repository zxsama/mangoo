use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const CONTROL: u8 = 1;
const BINARY: u8 = 2;
const END: u8 = 3;
const CREDIT: u8 = 4;

#[derive(Clone)]
struct Upload {
    key: String,
    size: u64,
    sha256: [u8; 32],
    bytes: Vec<u8>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut objects = BTreeMap::<String, Vec<u8>>::new();
    let mut uploads = HashMap::<String, Upload>::new();
    let mut incoming = HashMap::<u64, (u64, [u8; 32])>::new();
    let mut next_stream_id = 2u64;
    let mut authenticated = false;
    let mut account_uuid = String::new();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let Some((kind, stream_id, sequence, payload)) = read_frame(&mut input)? else {
            break;
        };
        if kind != CONTROL || stream_id != 0 || sequence != 0 {
            return Err("unexpected frame before a control request".into());
        }
        let mut envelope: Value = serde_json::from_slice(&payload)?;
        let method = envelope["method"].as_str().unwrap_or_default().to_owned();
        if envelope["messageType"] == "cancel" {
            continue;
        }
        let request = envelope["payload"].clone();
        if method == "stream.open" {
            let id = request["streamId"].as_u64().ok_or("missing stream id")?;
            let size = request["size"].as_u64().ok_or("missing stream size")?;
            let hash = parse_hash(request["sha256"].as_str().ok_or("missing stream hash")?)?;
            if id == 0 || id % 2 == 0 || size > 64 * 1024 * 1024 {
                return Err("invalid host stream".into());
            }
            incoming.insert(id, (size, hash));
            write_frame(&mut output, CREDIT, id, 0, &size.to_le_bytes())?;
            continue;
        }
        envelope["messageType"] = json!("response");
        if method == "shutdown" {
            envelope["payload"] = Value::Null;
            write_response(&mut output, &envelope)?;
            break;
        }
        let result: Result<Value, String> = (|| match method.as_str() {
            "runtime.hello" => {
                account_uuid = request["accountUuid"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                Ok(
                    json!({"nonce":request["nonce"],"wireVersion":1,"pluginId":request["pluginId"],"version":request["version"],"accountUuid":request["accountUuid"],"capabilities":["sync-provider.v1"],"methodVersions":request["methodVersions"],"limits":request["limits"]}),
                )
            }
            "describe" => Ok(
                json!({"pluginId":"com.megle.fake-sync","accountUuid":account_uuid,"capabilities":["sync-provider.v1"],"maxObjectBytes":67108864u64}),
            ),
            "health" => Ok(json!({"ready":true,"detail":null})),
            "diagnostics" => {
                Ok(json!({"implementation":"bundled-framed-child","objects":objects.len()}))
            }
            "auth.begin" => Ok(
                json!({"authorizationUrl":"http://127.0.0.1/fake-authorize","flowId":uuid::Uuid::now_v7().to_string()}),
            ),
            "auth.complete" => {
                authenticated = true;
                Ok(json!({"authenticated":true,"accountDisplayName":"Bundled Fake Account"}))
            }
            "auth.status" => Ok(
                json!({"authenticated":authenticated,"accountDisplayName":"Bundled Fake Account"}),
            ),
            "auth.sign_out" => {
                authenticated = false;
                Ok(Value::Null)
            }
            "delta.list" => Ok(delta_page(&objects, &request)),
            "object.head" => Ok(object_metadata(
                &objects,
                request["key"].as_str().unwrap_or_default(),
            )
            .unwrap_or(Value::Null)),
            "object.get" => {
                let key = request["key"].as_str().ok_or("missing object key")?;
                let bytes = objects.get(key).ok_or("object not found")?.clone();
                if bytes.len() as u64 > request["maxBytes"].as_u64().unwrap_or(0) {
                    return Err("object exceeds caller limit".into());
                }
                let id = next_stream_id;
                next_stream_id += 2;
                let metadata = object_metadata(&objects, key).ok_or("object disappeared")?;
                send_plugin_stream(&mut input, &mut output, id, &bytes)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"metadata":metadata,"streamId":id}))
            }
            "object.get_range" => {
                let key = request["key"].as_str().ok_or("missing object key")?;
                let bytes = objects.get(key).ok_or("object not found")?;
                let start = request["start"].as_u64().ok_or("missing range start")?;
                let end = request["end"].as_u64().ok_or("missing range end")?;
                let max_bytes = request["maxBytes"].as_u64().unwrap_or(0);
                if end <= start || end > bytes.len() as u64 || end - start > max_bytes {
                    return Err("invalid object range".into());
                }
                let range = bytes[start as usize..end as usize].to_vec();
                let id = next_stream_id;
                next_stream_id += 2;
                let metadata = object_metadata(&objects, key).ok_or("object disappeared")?;
                send_plugin_stream(&mut input, &mut output, id, &range)
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"metadata":metadata,"streamId":id,"responseStart":start,"responseEnd":end,"wholeObject":false}),
                )
            }
            "object.put_immutable" => {
                let id = request["streamId"].as_u64().ok_or("missing stream id")?;
                let declaration = incoming.remove(&id).ok_or("stream not open")?;
                let bytes =
                    receive_host_stream(&mut input, id, declaration).map_err(|e| e.to_string())?;
                put_immutable(
                    &mut objects,
                    request["key"].as_str().ok_or("missing key")?,
                    &bytes,
                    request["sha256"].as_str().ok_or("missing hash")?,
                )
            }
            "upload.begin" => {
                let upload_id = uuid::Uuid::now_v7().to_string();
                uploads.insert(
                    upload_id.clone(),
                    Upload {
                        key: request["key"].as_str().ok_or("missing key")?.to_owned(),
                        size: request["size"].as_u64().ok_or("missing size")?,
                        sha256: parse_hash(request["sha256"].as_str().ok_or("missing hash")?)
                            .map_err(|e| e.to_string())?,
                        bytes: Vec::new(),
                    },
                );
                Ok(json!({"uploadId":upload_id,"committedBytes":0}))
            }
            "upload.write" => {
                let id = request["streamId"].as_u64().ok_or("missing stream id")?;
                let declaration = incoming.remove(&id).ok_or("stream not open")?;
                let bytes =
                    receive_host_stream(&mut input, id, declaration).map_err(|e| e.to_string())?;
                let upload = uploads
                    .get_mut(request["uploadId"].as_str().ok_or("missing upload id")?)
                    .ok_or("upload not found")?;
                if request["offset"].as_u64() != Some(upload.bytes.len() as u64)
                    || upload.bytes.len() + bytes.len() > upload.size as usize
                {
                    Err("upload offset or size mismatch".into())
                } else {
                    upload.bytes.extend(bytes);
                    Ok(json!({"committedBytes":upload.bytes.len()}))
                }
            }
            "upload.commit" => {
                let upload = uploads
                    .remove(request["uploadId"].as_str().ok_or("missing upload id")?)
                    .ok_or("upload not found")?;
                if upload.bytes.len() as u64 != upload.size
                    || Sha256::digest(&upload.bytes).as_slice() != upload.sha256
                {
                    Err("upload incomplete or corrupt".into())
                } else {
                    put_immutable(
                        &mut objects,
                        &upload.key,
                        &upload.bytes,
                        &hex(&upload.sha256),
                    )
                }
            }
            "upload.abort" => {
                uploads.remove(request["uploadId"].as_str().unwrap_or_default());
                Ok(Value::Null)
            }
            _ => Err(format!("unknown method: {method}")),
        })();
        match result {
            Ok(value) => envelope["payload"] = value,
            Err(detail) => {
                envelope["messageType"] = json!("error");
                envelope["payload"] =
                    json!({"code":"precondition_failed","retryable":false,"detail":detail});
            }
        }
        write_response(&mut output, &envelope)?;
    }
    Ok(())
}

fn delta_page(objects: &BTreeMap<String, Vec<u8>>, request: &Value) -> Value {
    let start = request["pageToken"]
        .as_str()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = request["limit"].as_u64().unwrap_or(100).clamp(1, 100) as usize;
    let items = objects
        .iter()
        .skip(start)
        .take(limit)
        .map(|(key, _)| object_metadata(objects, key).unwrap())
        .collect::<Vec<_>>();
    let next = start + items.len();
    json!({"objects":items,"pageToken":(next < objects.len()).then(|| next.to_string()),"completedCursor":(next >= objects.len()).then(|| format!("v{}", objects.len()))})
}

fn object_metadata(objects: &BTreeMap<String, Vec<u8>>, key: &str) -> Option<Value> {
    objects.get(key).map(|bytes| {
        let digest = Sha256::digest(bytes);
        json!({"key":key,"size":bytes.len(),"sha256":hex(&digest),"etag":hex(&digest)})
    })
}

fn put_immutable(
    objects: &mut BTreeMap<String, Vec<u8>>,
    key: &str,
    bytes: &[u8],
    expected: &str,
) -> Result<Value, String> {
    let actual = Sha256::digest(bytes);
    if hex(&actual) != expected {
        return Err("caller hash mismatch".into());
    }
    if let Some(existing) = objects.get(key) {
        let remote = Sha256::digest(existing);
        return Ok(if remote.as_slice() == actual.as_slice() {
            json!({"status":"already_exists_matching","etag":hex(&remote)})
        } else {
            json!({"status":"hash_conflict","remoteSha256":hex(&remote)})
        });
    }
    objects.insert(key.to_owned(), bytes.to_vec());
    Ok(json!({"status":"created","etag":hex(&actual)}))
}

fn receive_host_stream(
    input: &mut impl Read,
    id: u64,
    declaration: (u64, [u8; 32]),
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::with_capacity(declaration.0.min(8 * 1024 * 1024) as usize);
    let mut expected_sequence = 1u32;
    loop {
        let (kind, stream_id, sequence, payload) =
            read_frame(input)?.ok_or("stream ended early")?;
        if stream_id != id || sequence != expected_sequence {
            return Err("invalid stream sequence".into());
        }
        match kind {
            BINARY => bytes.extend(payload),
            END => {
                if payload.len() != 40
                    || u64::from_le_bytes(payload[..8].try_into()?) != declaration.0
                    || payload[8..] != declaration.1
                    || bytes.len() as u64 != declaration.0
                    || Sha256::digest(&bytes).as_slice() != declaration.1
                {
                    return Err("invalid stream terminal".into());
                }
                return Ok(bytes);
            }
            _ => return Err("unexpected stream frame".into()),
        }
        expected_sequence += 1;
    }
}

fn send_plugin_stream(
    input: &mut impl Read,
    output: &mut impl Write,
    id: u64,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let open = json!({"wireVersion":1,"messageType":"request","requestId":uuid::Uuid::now_v7(),"method":"stream.open","deadlineMs":0,"bindingEpoch":null,"transportGeneration":null,"payload":{"streamId":id,"size":bytes.len(),"sha256":hex(&digest)}});
    write_response(output, &open)?;
    let (kind, credit_id, _, credit) = read_frame(input)?.ok_or("missing stream credit")?;
    if kind != CREDIT || credit_id != id || credit.len() != 8 {
        return Err("invalid stream credit".into());
    }
    let mut granted = u64::from_le_bytes(
        credit
            .as_slice()
            .try_into()
            .map_err(|_| "invalid stream credit")?,
    );
    let mut sequence = 1u32;
    let mut offset = 0usize;
    while offset < bytes.len() {
        if offset as u64 >= granted {
            let (kind, credit_id, _, credit) = read_frame(input)?.ok_or("missing stream credit")?;
            if kind != CREDIT || credit_id != id || credit.len() != 8 {
                return Err("invalid stream credit".into());
            }
            let next = u64::from_le_bytes(
                credit
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid stream credit")?,
            );
            if next <= granted {
                return Err("non-advancing stream credit".into());
            }
            granted = next;
        }
        let end = bytes.len().min(offset + 1024 * 1024).min(granted as usize);
        write_frame(output, BINARY, id, sequence, &bytes[offset..end])?;
        offset = end;
        sequence += 1;
    }
    let mut end = Vec::with_capacity(40);
    end.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    end.extend_from_slice(&digest);
    write_frame(output, END, id, sequence, &end)?;
    Ok(())
}

fn parse_hash(value: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    if value.len() != 64 {
        return Err("invalid hash".into());
    }
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(hash)
}

fn read_frame(input: &mut impl Read) -> std::io::Result<Option<(u8, u64, u32, Vec<u8>)>> {
    let mut header = [0u8; 17];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    if length > 4 * 1024 * 1024 {
        return Err(std::io::Error::other("oversize frame"));
    }
    let mut payload = vec![0u8; length];
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

fn write_response(output: &mut impl Write, envelope: &Value) -> std::io::Result<()> {
    let payload = serde_json::to_vec(envelope).map_err(std::io::Error::other)?;
    write_frame(output, CONTROL, 0, 0, &payload)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
