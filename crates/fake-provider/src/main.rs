use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const CONTROL: u8 = 1;
const BINARY: u8 = 2;
const END: u8 = 3;
const CREDIT: u8 = 4;

#[derive(Clone, Serialize, Deserialize)]
struct Upload {
    key: String,
    size: u64,
    sha256: [u8; 32],
    committed_bytes: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredObject {
    file_name: String,
    size: u64,
    sha256: [u8; 32],
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferCounters {
    upload_wire_bytes: u64,
    download_wire_bytes: u64,
    data_range_wire_bytes: u64,
    manifest_range_wire_bytes: u64,
    range_requests: u64,
    peak_buffered_bytes: u64,
}

struct Store {
    root: PathBuf,
    persistent: bool,
    objects: BTreeMap<String, StoredObject>,
    uploads: HashMap<String, Upload>,
    counters: TransferCounters,
}

impl Store {
    fn open() -> Result<Self, Box<dyn std::error::Error>> {
        let configured = std::env::var_os("MANGOO_PLUGIN_STATE_DIR")
            .or_else(|| std::env::var_os("MANGOO_FAKE_PROVIDER_STATE_DIR"))
            .map(PathBuf::from);
        let persistent = configured.is_some();
        let root = configured.unwrap_or_else(|| {
            std::env::temp_dir()
                .join("mangoo-fake-provider")
                .join(format!("{}-{}", std::process::id(), uuid::Uuid::now_v7()))
        });
        std::fs::create_dir_all(root.join("objects"))?;
        std::fs::create_dir_all(root.join("uploads"))?;
        let objects = read_json_or_default(&root.join("objects.json"))?;
        let uploads = read_json_or_default(&root.join("uploads.json"))?;
        let counters = read_json_or_default(&root.join("counters.json"))?;
        Ok(Self {
            root,
            persistent,
            objects,
            uploads,
            counters,
        })
    }

    fn is_persistent(&self) -> bool {
        self.persistent
    }

    fn object_path(&self, object: &StoredObject) -> PathBuf {
        self.root.join("objects").join(&object.file_name)
    }

    fn upload_path(&self, upload_id: &str) -> PathBuf {
        self.root.join("uploads").join(format!("{upload_id}.part"))
    }

    fn save_objects(&self) -> Result<(), String> {
        write_json_durable(&self.root.join("objects.json"), &self.objects)
    }

    fn save_uploads(&self) -> Result<(), String> {
        write_json_durable(&self.root.join("uploads.json"), &self.uploads)
    }

    fn save_counters(&self) -> Result<(), String> {
        write_json_durable(&self.root.join("counters.json"), &self.counters)
    }

    fn create_upload(&self, upload_id: &str) -> Result<(), String> {
        File::create(self.upload_path(upload_id))
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        self.save_uploads()
    }

    fn append_upload(&mut self, upload_id: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.upload_path(upload_id);
        let committed = self
            .uploads
            .get(upload_id)
            .ok_or_else(|| "upload not found".to_owned())?
            .committed_bytes;
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        if file.metadata().map_err(|error| error.to_string())?.len() != committed {
            return Err("durable upload cursor mismatch".into());
        }
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_data().map_err(|error| error.to_string())?;
        self.uploads
            .get_mut(upload_id)
            .expect("upload checked above")
            .committed_bytes += bytes.len() as u64;
        self.counters.upload_wire_bytes += bytes.len() as u64;
        self.save_uploads()?;
        self.save_counters()
    }

    fn commit_upload(&mut self, upload_id: &str) -> Result<Value, String> {
        let upload = self
            .uploads
            .get(upload_id)
            .cloned()
            .ok_or_else(|| "upload not found".to_owned())?;
        if upload.committed_bytes != upload.size {
            return Err("upload incomplete".into());
        }
        let upload_path = self.upload_path(upload_id);
        let actual = hash_file(&upload_path)?;
        if actual != upload.sha256 {
            return Err("upload digest mismatch".into());
        }
        if let Some(existing) = self.objects.get(&upload.key) {
            let result = if existing.sha256 == actual {
                json!({"status":"already_exists_matching","etag":hex(&actual)})
            } else {
                json!({"status":"hash_conflict","remoteSha256":hex(&existing.sha256)})
            };
            self.uploads.remove(upload_id);
            let _ = std::fs::remove_file(upload_path);
            self.save_uploads()?;
            return Ok(result);
        }
        let file_name = format!("{}.bin", hex(&Sha256::digest(upload.key.as_bytes())));
        std::fs::rename(&upload_path, self.root.join("objects").join(&file_name))
            .map_err(|error| error.to_string())?;
        self.objects.insert(
            upload.key,
            StoredObject {
                file_name,
                size: upload.size,
                sha256: actual,
            },
        );
        self.uploads.remove(upload_id);
        self.save_objects()?;
        self.save_uploads()?;
        Ok(json!({"status":"created","etag":hex(&actual)}))
    }

    fn abort_upload(&mut self, upload_id: &str) -> Result<(), String> {
        self.uploads.remove(upload_id);
        match std::fs::remove_file(self.upload_path(upload_id)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        self.save_uploads()
    }

    fn put_immutable(&mut self, key: &str, bytes: &[u8], expected: &str) -> Result<Value, String> {
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if hex(&actual) != expected {
            return Err("caller hash mismatch".into());
        }
        if let Some(existing) = self.objects.get(key) {
            return Ok(if existing.sha256 == actual {
                json!({"status":"already_exists_matching","etag":hex(&actual)})
            } else {
                json!({"status":"hash_conflict","remoteSha256":hex(&existing.sha256)})
            });
        }
        let file_name = format!("{}.bin", hex(&Sha256::digest(key.as_bytes())));
        let path = self.root.join("objects").join(&file_name);
        let mut file = File::create(&path).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        self.objects.insert(
            key.to_owned(),
            StoredObject {
                file_name,
                size: bytes.len() as u64,
                sha256: actual,
            },
        );
        self.save_objects()?;
        Ok(json!({"status":"created","etag":hex(&actual)}))
    }
}

fn read_json_or_default<T>(path: &Path) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::de::DeserializeOwned + Default,
{
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    }
}

fn write_json_durable(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn hash_file(path: &Path) -> Result<[u8; 32], String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = Store::open()?;
    let configuration: std::collections::BTreeMap<String, String> =
        std::env::var("MANGOO_PLUGIN_CONFIGURATION")
            .ok()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
    let fault_mode = configuration
        .get("fault")
        .cloned()
        .or_else(|| std::env::var("MANGOO_FAKE_PROVIDER_FAULT").ok())
        .unwrap_or_default();
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
        // Deterministic process fixture: an explicitly installed method barrier
        // emulates a blocking remote call. It is local, bounded, and never used
        // by the production Provider.
        if store.persistent
            && std::fs::read_to_string(store.root.join("lifecycle-barrier-method"))
                .is_ok_and(|requested| requested.trim() == method)
        {
            std::fs::write(
                store.root.join("lifecycle-barrier-entered"),
                std::process::id().to_string(),
            )?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while std::time::Instant::now() < deadline
                && !store.root.join("lifecycle-barrier-release").exists()
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
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
        if fault_mode == "permission-loss"
            && !matches!(
                method.as_str(),
                "runtime.hello" | "diagnostics" | "shutdown"
            )
        {
            envelope["messageType"] = json!("error");
            envelope["payload"] = json!({
                "code":"authorization_required",
                "retryable":false,
                "detail":"injected permission loss"
            });
            write_response(&mut output, &envelope)?;
            continue;
        }
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
                json!({"pluginId":"com.mangoo.fake-sync","accountUuid":account_uuid,"capabilities":["sync-provider.v1"],"maxObjectBytes":67108864u64}),
            ),
            "health" => Ok(json!({"ready":true,"detail":null})),
            "diagnostics" => Ok(
                json!({"implementation":"bundled-framed-child","objects":store.objects.len(),"persistent":store.is_persistent(),"transferCounters":store.counters}),
            ),
            "auth.begin" => Ok(
                json!({"authorizationUrl":"https://auth.example.test/authorize?workspace=fixture&state=fixture-state&redirect_uri=https%3A%2F%2Fauth.example.test%2Freturn","flowId":uuid::Uuid::now_v7().to_string(),"expiresAt":4102444800000u64,"completionMode":"manual-response-uri","redirectUri":"https://auth.example.test/return"}),
            ),
            "auth.poll" => Ok(json!({"pending":true})),
            "auth.cancel" => Ok(Value::Null),
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
            "delta.list" => Ok(delta_page(&store.objects, &request, &fault_mode)),
            "object.head" => Ok(object_metadata(
                &store.objects,
                request["key"].as_str().unwrap_or_default(),
            )
            .unwrap_or(Value::Null)),
            "object.get" => {
                let key = request["key"].as_str().ok_or("missing object key")?;
                let object = store.objects.get(key).ok_or("object not found")?.clone();
                if object.size > request["maxBytes"].as_u64().unwrap_or(0) {
                    return Err("object exceeds caller limit".into());
                }
                let bytes =
                    std::fs::read(store.object_path(&object)).map_err(|error| error.to_string())?;
                store.counters.peak_buffered_bytes =
                    store.counters.peak_buffered_bytes.max(bytes.len() as u64);
                let id = next_stream_id;
                next_stream_id += 2;
                let metadata = object_metadata(&store.objects, key).ok_or("object disappeared")?;
                send_plugin_stream(&mut input, &mut output, id, &bytes)
                    .map_err(|e| e.to_string())?;
                store.counters.download_wire_bytes += bytes.len() as u64;
                store.save_counters()?;
                Ok(json!({"metadata":metadata,"streamId":id}))
            }
            "object.get_range" => {
                let key = request["key"].as_str().ok_or("missing object key")?;
                let object = store.objects.get(key).ok_or("object not found")?.clone();
                let start = request["start"].as_u64().ok_or("missing range start")?;
                let end = request["end"].as_u64().ok_or("missing range end")?;
                let max_bytes = request["maxBytes"].as_u64().unwrap_or(0);
                if end <= start || end > object.size || end - start > max_bytes {
                    return Err("invalid object range".into());
                }
                let mut file = File::open(store.object_path(&object)).map_err(|e| e.to_string())?;
                file.seek(SeekFrom::Start(start))
                    .map_err(|e| e.to_string())?;
                let mut range =
                    vec![0_u8; usize::try_from(end - start).map_err(|e| e.to_string())?];
                file.read_exact(&mut range).map_err(|e| e.to_string())?;
                if fault_mode == "short-range" && range.len() > 1 {
                    range.pop();
                }
                store.counters.peak_buffered_bytes =
                    store.counters.peak_buffered_bytes.max(range.len() as u64);
                let id = next_stream_id;
                next_stream_id += 2;
                let metadata = object_metadata(&store.objects, key).ok_or("object disappeared")?;
                send_plugin_stream(&mut input, &mut output, id, &range)
                    .map_err(|e| e.to_string())?;
                store.counters.download_wire_bytes += range.len() as u64;
                store.counters.range_requests += 1;
                if key.starts_with("content/data/") {
                    store.counters.data_range_wire_bytes += range.len() as u64;
                } else if key.starts_with("content/manifests/") {
                    store.counters.manifest_range_wire_bytes += range.len() as u64;
                }
                store.save_counters()?;
                Ok(
                    json!({"metadata":metadata,"streamId":id,"responseStart":start,"responseEnd":end,"wholeObject":false}),
                )
            }
            "object.put_immutable" => {
                let id = request["streamId"].as_u64().ok_or("missing stream id")?;
                let declaration = incoming.remove(&id).ok_or("stream not open")?;
                let bytes =
                    receive_host_stream(&mut input, id, declaration).map_err(|e| e.to_string())?;
                store.counters.peak_buffered_bytes =
                    store.counters.peak_buffered_bytes.max(bytes.len() as u64);
                store.counters.upload_wire_bytes += bytes.len() as u64;
                store.save_counters()?;
                store.put_immutable(
                    request["key"].as_str().ok_or("missing key")?,
                    &bytes,
                    request["sha256"].as_str().ok_or("missing hash")?,
                )
            }
            "upload.begin" => {
                let upload_id = uuid::Uuid::now_v7().to_string();
                let upload = Upload {
                    key: request["key"].as_str().ok_or("missing key")?.to_owned(),
                    size: request["size"].as_u64().ok_or("missing size")?,
                    sha256: parse_hash(request["sha256"].as_str().ok_or("missing hash")?)
                        .map_err(|e| e.to_string())?,
                    committed_bytes: 0,
                };
                store.uploads.insert(upload_id.clone(), upload);
                store.create_upload(&upload_id)?;
                Ok(json!({"uploadId":upload_id,"committedBytes":0}))
            }
            "upload.write" => {
                let id = request["streamId"].as_u64().ok_or("missing stream id")?;
                let declaration = incoming.remove(&id).ok_or("stream not open")?;
                let bytes =
                    receive_host_stream(&mut input, id, declaration).map_err(|e| e.to_string())?;
                store.counters.peak_buffered_bytes =
                    store.counters.peak_buffered_bytes.max(bytes.len() as u64);
                let upload_id = request["uploadId"].as_str().ok_or("missing upload id")?;
                let upload = store.uploads.get(upload_id).ok_or("upload not found")?;
                if request["offset"].as_u64() != Some(upload.committed_bytes)
                    || upload.committed_bytes + bytes.len() as u64 > upload.size
                {
                    Err("upload offset or size mismatch".into())
                } else {
                    store.append_upload(upload_id, &bytes)?;
                    Ok(json!({"committedBytes":store.uploads[upload_id].committed_bytes}))
                }
            }
            "upload.commit" => {
                store.commit_upload(request["uploadId"].as_str().ok_or("missing upload id")?)
            }
            "upload.abort" => {
                store.abort_upload(request["uploadId"].as_str().unwrap_or_default())?;
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

fn delta_page(
    objects: &BTreeMap<String, StoredObject>,
    request: &Value,
    fault_mode: &str,
) -> Value {
    let start = request["pageToken"]
        .as_str()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = request["limit"].as_u64().unwrap_or(100).clamp(1, 100) as usize;
    let mut items = objects
        .iter()
        .skip(start)
        .take(limit)
        .map(|(key, _)| object_metadata(objects, key).unwrap())
        .collect::<Vec<_>>();
    if fault_mode == "reordered-delta" {
        items.reverse();
        if let Some(first) = items.first().cloned() {
            items.push(first);
        }
    }
    let next = start + items.len();
    json!({"objects":items,"pageToken":(next < objects.len()).then(|| next.to_string()),"completedCursor":(next >= objects.len()).then(|| format!("v{}", objects.len()))})
}

fn object_metadata(objects: &BTreeMap<String, StoredObject>, key: &str) -> Option<Value> {
    objects.get(key).map(|object| {
        json!({"key":key,"size":object.size,"sha256":hex(&object.sha256),"etag":hex(&object.sha256)})
    })
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
