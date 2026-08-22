use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

const CONTROL: u8 = 1;
const BINARY: u8 = 2;
const END: u8 = 3;
const CREDIT: u8 = 4;

#[derive(Default)]
struct GraphState {
    objects: HashMap<String, Vec<u8>>,
    uploads: HashMap<String, (String, Vec<u8>, u64)>,
    namespaces: HashSet<String>,
    download_counts: HashMap<String, usize>,
    capability_bearer_leaks: usize,
    reject_next_graph_request: bool,
    reject_next_token_refresh: bool,
    next_upload_fault: Option<u16>,
}

struct Emulator {
    base: String,
    state: Arc<Mutex<GraphState>>,
    #[cfg(windows)]
    _winsock: WindowsSockets,
}

impl Emulator {
    fn start() -> Self {
        let winsock = WindowsSockets::startup();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let probe = TcpStream::connect(listener.local_addr().unwrap());
        assert!(probe.is_ok());
        drop(probe);
        let state = Arc::new(Mutex::new(GraphState::default()));
        let shared = Arc::clone(&state);
        let public = base.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                let public = public.clone();
                thread::spawn(move || serve_graph(stream, &public, &state));
            }
        });
        Self {
            base,
            state,
            #[cfg(windows)]
            _winsock: winsock,
        }
    }
}

#[cfg(windows)]
struct WindowsSockets;
#[cfg(windows)]
impl WindowsSockets {
    fn startup() -> Self {
        use windows_sys::Win32::Networking::WinSock::{WSAStartup, WSADATA};
        let mut data = std::mem::MaybeUninit::<WSADATA>::zeroed();
        assert_eq!(unsafe { WSAStartup(0x0202, data.as_mut_ptr()) }, 0);
        Self
    }
}
#[cfg(windows)]
impl Drop for WindowsSockets {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Networking::WinSock::WSACleanup();
        }
    }
}

#[derive(Clone, Default)]
struct Secrets {
    current: Option<String>,
    records: HashMap<String, (Vec<u8>, u64)>,
    authorization_required: bool,
}

struct ProviderProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    secrets: Arc<Mutex<Secrets>>,
    next_stream: u64,
}

impl ProviderProcess {
    fn start(base: &str, secrets: Arc<Mutex<Secrets>>) -> Self {
        let executable = std::env::var_os("MEGLE_ONEDRIVE_TEST_EXECUTABLE")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_megle-onedrive-provider").into());
        let mut child = Command::new(executable)
            .env_clear()
            .envs(platform_runtime_env())
            .env("MEGLE_ONEDRIVE_CLIENT_ID", "emulator-public-client")
            .env("MEGLE_ONEDRIVE_AUTHORITY", base)
            .env("MEGLE_ONEDRIVE_EMULATOR_URL", format!("{base}v1.0/"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Self {
            child,
            stdin,
            stdout,
            secrets,
            next_stream: 1,
        }
    }

    fn start_unconfigured(base: &str) -> Self {
        let executable = std::env::var_os("MEGLE_ONEDRIVE_TEST_EXECUTABLE")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_megle-onedrive-provider").into());
        let mut child = Command::new(executable)
            .env_clear()
            .envs(platform_runtime_env())
            .env("MEGLE_ONEDRIVE_AUTHORITY", base)
            .env("MEGLE_ONEDRIVE_EMULATOR_URL", format!("{base}v1.0/"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Self {
            child,
            stdin,
            stdout,
            secrets: Arc::new(Mutex::new(Secrets::default())),
            next_stream: 1,
        }
    }

    fn call(&mut self, method: &str, payload: Value) -> Value {
        let envelope = self.call_envelope(method, payload);
        assert_eq!(envelope["messageType"], "response", "{envelope}");
        envelope["payload"].clone()
    }

    fn call_error(&mut self, method: &str, payload: Value) -> Value {
        let envelope = self.call_envelope(method, payload);
        assert_eq!(envelope["messageType"], "error", "{envelope}");
        envelope["payload"].clone()
    }

    fn call_envelope(&mut self, method: &str, payload: Value) -> Value {
        let request_id = uuid::Uuid::now_v7();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":request_id,"method":method,"deadlineMs":30000,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":payload}),
        );
        self.wait_envelope(request_id)
    }

    fn call_with_bytes(&mut self, method: &str, mut payload: Value, bytes: &[u8]) -> Value {
        let stream = self.next_stream;
        self.next_stream += 2;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":uuid::Uuid::now_v7(),"method":"stream.open","deadlineMs":0,"bindingEpoch":null,"transportGeneration":null,"payload":{"streamId":stream,"size":bytes.len(),"sha256":hex(&digest)}}),
        );
        let (kind, id, _, credit) = read_frame(&mut self.stdout);
        assert_eq!((kind, id), (CREDIT, stream));
        let mut granted = u64::from_le_bytes(credit.try_into().unwrap());
        payload["streamId"] = json!(stream);
        let request_id = uuid::Uuid::now_v7();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":request_id,"method":method,"deadlineMs":30000,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":payload}),
        );
        let mut offset = 0usize;
        let mut sequence = 1;
        while offset < bytes.len() {
            if offset as u64 >= granted {
                let (kind, id, _, credit) = read_frame(&mut self.stdout);
                assert_eq!((kind, id), (CREDIT, stream));
                granted = u64::from_le_bytes(credit.try_into().unwrap());
            }
            let end = bytes.len().min(offset + 1024 * 1024).min(granted as usize);
            write_frame(
                &mut self.stdin,
                BINARY,
                stream,
                sequence,
                &bytes[offset..end],
            );
            offset = end;
            sequence += 1;
        }
        let mut terminal = Vec::from((bytes.len() as u64).to_le_bytes());
        terminal.extend_from_slice(&digest);
        write_frame(&mut self.stdin, END, stream, sequence, &terminal);
        self.wait_response(request_id)
    }

    fn object_get(
        &mut self,
        namespace: &str,
        key: &str,
        range: Option<(u64, u64)>,
    ) -> (Vec<u8>, Value) {
        self.object_get_bounded(namespace, key, range, 96 * 1024 * 1024)
    }

    fn object_get_bounded(
        &mut self,
        namespace: &str,
        key: &str,
        range: Option<(u64, u64)>,
        max_bytes: u64,
    ) -> (Vec<u8>, Value) {
        let request_id = uuid::Uuid::now_v7();
        let (method, payload) = match range {
            Some((start, end)) => (
                "object.get_range",
                json!({"namespace":namespace,"key":key,"start":start,"end":end,"maxBytes":max_bytes}),
            ),
            None => (
                "object.get",
                json!({"namespace":namespace,"key":key,"maxBytes":96*1024*1024u64}),
            ),
        };
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":request_id,"method":method,"deadlineMs":30000,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":payload}),
        );
        loop {
            let (kind, stream, _, payload) = read_frame(&mut self.stdout);
            assert_eq!(kind, CONTROL);
            let envelope: Value = serde_json::from_slice(&payload).unwrap();
            if envelope["messageType"] == "request" {
                if envelope["method"] == "stream.open" {
                    let id = envelope["payload"]["streamId"].as_u64().unwrap();
                    let size = envelope["payload"]["size"].as_u64().unwrap();
                    write_frame(
                        &mut self.stdin,
                        CREDIT,
                        id,
                        0,
                        &size.min(8 * 1024 * 1024).to_le_bytes(),
                    );
                    let (bytes, _) = self.receive_stream(id, size);
                    let response = self.wait_response(request_id);
                    return (bytes, response);
                }
                self.handle_reverse(envelope);
            } else {
                panic!("expected stream.open before response: {envelope}");
            }
            assert_eq!(stream, 0);
        }
    }

    fn receive_stream(&mut self, id: u64, size: u64) -> (Vec<u8>, [u8; 32]) {
        let mut bytes = Vec::with_capacity(size as usize);
        let mut granted = size.min(8 * 1024 * 1024);
        loop {
            let (kind, stream, _, payload) = read_frame(&mut self.stdout);
            assert_eq!(stream, id);
            if kind == BINARY {
                bytes.extend_from_slice(&payload);
                if granted < size && bytes.len() as u64 + 2 * 1024 * 1024 >= granted {
                    granted = size.min(granted + 8 * 1024 * 1024);
                    write_frame(&mut self.stdin, CREDIT, id, 0, &granted.to_le_bytes());
                }
            } else {
                assert_eq!(kind, END);
                let hash = payload[8..].try_into().unwrap();
                return (bytes, hash);
            }
        }
    }

    fn wait_response(&mut self, request_id: uuid::Uuid) -> Value {
        let envelope = self.wait_envelope(request_id);
        assert_eq!(envelope["messageType"], "response", "{envelope}");
        envelope["payload"].clone()
    }

    fn wait_envelope(&mut self, request_id: uuid::Uuid) -> Value {
        loop {
            let (kind, stream, sequence, payload) = read_frame(&mut self.stdout);
            assert_eq!((kind, stream, sequence), (CONTROL, 0, 0));
            let envelope: Value = serde_json::from_slice(&payload).unwrap();
            if envelope["messageType"] == "request" {
                self.handle_reverse(envelope);
                continue;
            }
            if envelope["requestId"] == request_id.to_string() {
                return envelope;
            }
        }
    }

    fn handle_reverse(&mut self, mut envelope: Value) {
        let method = envelope["method"].as_str().unwrap().to_owned();
        let payload = envelope["payload"].clone();
        let purpose = payload
            .get("purpose")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !purpose.is_empty()
                && purpose.len() <= 160
                && purpose.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                }),
            "invalid secret purpose: {purpose}"
        );
        let mut secrets = self.secrets.lock().unwrap();
        let result = match method.as_str() {
            "host.secret.current" => json!({"secretRef":secrets.current,"accountGeneration":1}),
            "host.secret.create" | "host.secret.create_scoped" => {
                let secret_ref = uuid::Uuid::now_v7().to_string();
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(payload["value"].as_str().unwrap())
                    .unwrap();
                secrets.records.insert(secret_ref.clone(), (bytes, 1));
                if method == "host.secret.create" {
                    secrets.current = Some(secret_ref.clone());
                }
                json!({"secretRef":secret_ref,"generation":1})
            }
            "host.secret.get" => {
                let (bytes, generation) = secrets
                    .records
                    .get(payload["secretRef"].as_str().unwrap())
                    .unwrap();
                json!({"value":base64::engine::general_purpose::STANDARD.encode(bytes),"generation":generation})
            }
            "host.secret.cas" | "host.secret.cas_scoped" => {
                let record = secrets
                    .records
                    .get_mut(payload["secretRef"].as_str().unwrap())
                    .unwrap();
                assert_eq!(payload["expectedGeneration"].as_u64(), Some(record.1));
                record.0 = base64::engine::general_purpose::STANDARD
                    .decode(payload["value"].as_str().unwrap())
                    .unwrap();
                record.1 += 1;
                json!({"generation":record.1})
            }
            "host.secret.delete" | "host.secret.delete_scoped" => {
                let key = payload["secretRef"].as_str().unwrap();
                secrets.records.remove(key);
                if secrets.current.as_deref() == Some(key) {
                    secrets.current = None;
                }
                Value::Null
            }
            "host.auth.authorization_required" => {
                secrets.authorization_required = true;
                Value::Null
            }
            _ => panic!("unexpected reverse method {method}"),
        };
        envelope["messageType"] = json!("response");
        envelope["payload"] = result;
        drop(secrets);
        write_control(&mut self.stdin, &envelope);
    }
}

impl Drop for ProviderProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn real_child_runs_oauth_secret_delta_object_range_and_resumable_upload_against_emulator() {
    let emulator = Emulator::start();
    let secrets = Arc::new(Mutex::new(Secrets::default()));
    let namespace = "018f0000-0000-7000-8000-000000000001";
    let mut provider = ProviderProcess::start(&emulator.base, Arc::clone(&secrets));
    hello(&mut provider);
    assert_eq!(provider.call("health", Value::Null)["ready"], true);
    let auth = provider.call(
        "auth.begin",
        json!({"callbackUrl":format!("{}callback", emulator.base)}),
    );
    let auth_url = Url::parse(auth["authorizationUrl"].as_str().unwrap()).unwrap();
    let state = auth_url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let status = provider.call(
        "auth.complete",
        json!({"flowId":auth["flowId"],"fields":{"state":state,"code":"emulator-code"}}),
    );
    assert_eq!(status["authenticated"], true);
    assert_eq!(status["accountKind"], "personal");

    let descriptor = |namespace: &str, seed_key: &str, seed_hash: &str| {
        serde_json::to_vec(&json!({
            "version":1,
            "libraryUuid":namespace,
            "bindingEpoch":"018f0000-0000-7000-8000-00000000beef",
            "namespace":namespace,
            "seed":{
                "checkpointUuid":"018f0000-0000-7000-8000-00000000cafe",
                "objectKey":seed_key,
                "sha256":seed_hash
            },
            "dependencies":[]
        }))
        .unwrap()
    };
    let small = b"immutable-object";
    let hash = hex(&Sha256::digest(small));
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"segments/immutable.json","sha256":hash}),
            small
        )["status"],
        "created"
    );
    assert_eq!(
        provider.call(
            "object.head",
            json!({"namespace":namespace,"key":"segments/immutable.json"})
        )["sha256"],
        hash
    );
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"segments/immutable.json","sha256":hash}),
            small
        )["status"],
        "already_exists_matching"
    );
    let conflict = provider.call_with_bytes(
        "object.put_immutable",
        json!({"namespace":namespace,"key":"segments/immutable.json","sha256":hex(&Sha256::digest(b"different"))}),
        b"different",
    );
    assert_eq!(conflict["status"], "hash_conflict");
    let first_descriptor = descriptor(namespace, "segments/immutable.json", &hash);
    let first_descriptor_hash = hex(&Sha256::digest(&first_descriptor));
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"library-descriptor","sha256":first_descriptor_hash}),
            &first_descriptor,
        )["status"],
        "created"
    );
    assert_eq!(
        provider
            .object_get(namespace, "segments/immutable.json", Some((2, 8)))
            .0,
        &small[2..8]
    );
    let fallback = b"range-fallback-body";
    let fallback_hash = hex(&Sha256::digest(fallback));
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"segments/range-fallback.json","sha256":fallback_hash}),
            fallback,
        )["status"],
        "created"
    );
    let (fallback_bytes, fallback_result) = provider.object_get_bounded(
        namespace,
        "segments/range-fallback.json",
        Some((2, 8)),
        fallback.len() as u64,
    );
    assert_eq!(fallback_bytes, fallback);
    assert_eq!(fallback_result["wholeObject"], true);

    let malformed_range = b"malformed-content-range";
    let malformed_range_hash = hex(&Sha256::digest(malformed_range));
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"segments/bad-content-range.json","sha256":malformed_range_hash}),
            malformed_range,
        )["status"],
        "created"
    );
    let bad_range = provider.call_error(
        "object.get_range",
        json!({"namespace":namespace,"key":"segments/bad-content-range.json","start":2,"end":8,"maxBytes":malformed_range.len()}),
    );
    assert_eq!(bad_range["code"], "protocol_error");

    let truncated = b"truncated-download";
    let truncated_hash = hex(&Sha256::digest(truncated));
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"segments/truncated.json","sha256":truncated_hash}),
            truncated,
        )["status"],
        "created"
    );
    let truncated_error = provider.call_error(
        "object.head",
        json!({"namespace":namespace,"key":"segments/truncated.json"}),
    );
    assert_eq!(truncated_error["code"], "protocol_error");

    let corrupt_fallback = b"corrupt-fallback-body";
    let corrupt_fallback_hash = hex(&Sha256::digest(corrupt_fallback));
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"namespace":namespace,"key":"segments/corrupt-fallback.json","sha256":corrupt_fallback_hash}),
            corrupt_fallback,
        )["status"],
        "created"
    );
    let corrupt_error = provider.call_error(
        "object.get_range",
        json!({"namespace":namespace,"key":"segments/corrupt-fallback.json","start":2,"end":8,"maxBytes":corrupt_fallback.len()}),
    );
    assert_eq!(corrupt_error["code"], "protocol_error");

    emulator.state.lock().unwrap().objects.insert(
        format!("{namespace}/segments/over-limit.json"),
        b"small-body-with-oversized-metadata".to_vec(),
    );
    let over_limit = provider.call_error(
        "object.head",
        json!({"namespace":namespace,"key":"segments/over-limit.json"}),
    );
    assert_eq!(over_limit["code"], "protocol_error");
    {
        let mut state = emulator.state.lock().unwrap();
        for key in [
            "segments/bad-content-range.json",
            "segments/truncated.json",
            "segments/corrupt-fallback.json",
            "segments/over-limit.json",
        ] {
            state.objects.remove(&format!("{namespace}/{key}"));
        }
    }

    emulator.state.lock().unwrap().reject_next_graph_request = true;
    assert_eq!(
        provider.call(
            "object.head",
            json!({"namespace":namespace,"key":"segments/immutable.json"})
        )["sha256"],
        hash
    );
    assert_eq!(
        secrets
            .lock()
            .unwrap()
            .records
            .values()
            .map(|(_, generation)| *generation)
            .max(),
        Some(2)
    );

    let cursor_reset = provider.call_error(
        "delta.list",
        json!({"scope":{"kind":"library","namespace":namespace},"cursor":format!("{}v1.0/delta-gone?token=secret",emulator.base),"pageToken":null,"limit":100}),
    );
    assert_eq!(cursor_reset["code"], "cursor_reset");
    assert!(!cursor_reset.to_string().contains("token=secret"));
    let throttled = provider.call_error(
        "delta.list",
        json!({"scope":{"kind":"library","namespace":namespace},"cursor":null,"pageToken":format!("{}v1.0/throttled",emulator.base),"limit":100}),
    );
    assert_eq!(throttled["code"], "throttled");
    assert_eq!(throttled["retryAfterMs"], 3000);

    let empty_page = provider.call(
        "delta.list",
        json!({"scope":{"kind":"library","namespace":namespace},"cursor":null,"pageToken":format!("{}v1.0/delta-empty-next",emulator.base),"limit":100}),
    );
    assert!(empty_page["objects"].as_array().unwrap().is_empty());
    let continuation = empty_page["pageToken"].as_str().unwrap().to_owned();

    let large = vec![0x5a; 72 * 1024 * 1024];
    let large_hash = hex(&Sha256::digest(&large));
    let upload = provider.call("upload.begin", json!({"namespace":namespace,"key":"content/data/large.pack","size":large.len(),"sha256":large_hash}));
    let upload_id = upload["uploadId"].as_str().unwrap().to_owned();
    emulator.state.lock().unwrap().next_upload_fault = Some(416);
    assert_eq!(
        provider.call_with_bytes(
            "upload.write",
            json!({"namespace":namespace,"uploadId":upload_id,"offset":0}),
            &large[..10 * 1024 * 1024]
        )["committedBytes"],
        10 * 1024 * 1024u64
    );
    drop(provider);

    let mut provider = ProviderProcess::start(&emulator.base, Arc::clone(&secrets));
    hello(&mut provider);
    assert_eq!(
        provider.call("auth.status", Value::Null)["authenticated"],
        true
    );
    let resumed_page = provider.call(
        "delta.list",
        json!({"scope":{"kind":"library","namespace":namespace},"cursor":null,"pageToken":continuation,"limit":100}),
    );
    assert_eq!(resumed_page["objects"].as_array().unwrap().len(), 2);
    assert!(resumed_page["completedCursor"].as_str().is_some());
    assert_eq!(
        provider.call_with_bytes(
            "upload.write",
            json!({"namespace":namespace,"uploadId":upload_id,"offset":10*1024*1024u64}),
            &large[10 * 1024 * 1024..20 * 1024 * 1024]
        )["committedBytes"],
        20 * 1024 * 1024u64
    );
    for offset in (20 * 1024 * 1024..70 * 1024 * 1024).step_by(10 * 1024 * 1024) {
        assert_eq!(
            provider.call_with_bytes(
                "upload.write",
                json!({"namespace":namespace,"uploadId":upload_id,"offset":offset}),
                &large[offset..offset + 10 * 1024 * 1024]
            )["committedBytes"],
            (offset + 10 * 1024 * 1024) as u64
        );
    }
    emulator.state.lock().unwrap().next_upload_fault = Some(503);
    assert_eq!(
        provider.call_with_bytes(
            "upload.write",
            json!({"namespace":namespace,"uploadId":upload_id,"offset":70*1024*1024u64}),
            &large[70 * 1024 * 1024..]
        )["committedBytes"],
        large.len() as u64
    );
    assert_eq!(
        provider.call(
            "upload.commit",
            json!({"namespace":namespace,"uploadId":upload_id})
        )["status"],
        "created"
    );
    assert_eq!(
        provider
            .object_get(namespace, "content/data/large.pack", None)
            .0,
        large
    );

    let aborted = provider.call("upload.begin", json!({"namespace":namespace,"key":"content/data/aborted.pack","size":20*1024*1024u64,"sha256":hex(&Sha256::digest(vec![0u8;20*1024*1024]))}));
    assert_eq!(emulator.state.lock().unwrap().uploads.len(), 1);
    provider.call(
        "upload.abort",
        json!({"namespace":namespace,"uploadId":aborted["uploadId"]}),
    );
    assert!(emulator.state.lock().unwrap().uploads.is_empty());
    assert_eq!(secrets.lock().unwrap().records.len(), 1);

    let quota = provider.call_error(
        "upload.begin",
        json!({"namespace":namespace,"key":"content/data/quota.pack","size":20*1024*1024u64,"sha256":hex(&Sha256::digest(vec![1u8;20*1024*1024]))}),
    );
    assert_eq!(quota["code"], "precondition_failed");
    for index in 2..=8u128 {
        let isolated_namespace = format!("018f0000-0000-7000-8000-{index:012}");
        let bytes = format!("library-{index}").into_bytes();
        let hash = hex(&Sha256::digest(&bytes));
        assert_eq!(
            provider.call_with_bytes(
                "object.put_immutable",
                json!({"namespace":isolated_namespace,"key":"segments/isolation.json","sha256":hash}),
                &bytes,
            )["status"],
            "created"
        );
        let library_descriptor = descriptor(&isolated_namespace, "segments/isolation.json", &hash);
        let descriptor_hash = hex(&Sha256::digest(&library_descriptor));
        assert_eq!(
            provider.call_with_bytes(
                "object.put_immutable",
                json!({"namespace":isolated_namespace,"key":"library-descriptor","sha256":descriptor_hash}),
                &library_descriptor,
            )["status"],
            "created"
        );
        let delta = provider.call("delta.list", json!({"scope":{"kind":"library","namespace":isolated_namespace},"cursor":null,"pageToken":null,"limit":100}));
        assert_eq!(delta["objects"].as_array().unwrap().len(), 2);
        assert!(delta["objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|object| object["sha256"] == hash));
    }
    let first_delta = provider.call("delta.list", json!({"scope":{"kind":"library","namespace":namespace},"cursor":null,"pageToken":null,"limit":100}));
    assert_eq!(first_delta["objects"].as_array().unwrap().len(), 4);
    let root_delta = provider.call(
        "delta.list",
        json!({"scope":{"kind":"root"},"cursor":null,"pageToken":null,"limit":100}),
    );
    assert_eq!(root_delta["objects"].as_array().unwrap().len(), 8);
    assert_eq!(emulator.state.lock().unwrap().capability_bearer_leaks, 0);

    provider.call(
        "upload.begin",
        json!({
            "namespace": namespace,
            "key": "content/data/pending-sign-out.pack",
            "size": 20 * 1024 * 1024u64,
            "sha256": hex(&Sha256::digest(vec![2u8; 20 * 1024 * 1024])),
        }),
    );
    assert_eq!(secrets.lock().unwrap().records.len(), 2);
    provider.call("auth.sign_out", Value::Null);
    assert!(secrets.lock().unwrap().records.is_empty());
    assert_eq!(
        provider.call("auth.status", Value::Null)["authenticated"],
        false
    );
    provider.call("shutdown", Value::Null);
    assert!(provider.child.wait().unwrap().success());
}

#[test]
fn invalid_grant_retires_all_secrets_and_requires_authorization() {
    let emulator = Emulator::start();
    let secrets = Arc::new(Mutex::new(Secrets::default()));
    let namespace = "018f0000-0000-7000-8000-000000000001";
    let mut provider = ProviderProcess::start(&emulator.base, Arc::clone(&secrets));
    hello(&mut provider);
    let auth = provider.call(
        "auth.begin",
        json!({"callbackUrl":format!("{}callback", emulator.base)}),
    );
    let auth_url = Url::parse(auth["authorizationUrl"].as_str().unwrap()).unwrap();
    let state = auth_url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    provider.call(
        "auth.complete",
        json!({"flowId":auth["flowId"],"fields":{"state":state,"code":"emulator-code"}}),
    );
    provider.call(
        "upload.begin",
        json!({
            "namespace": namespace,
            "key": "content/data/pending-reauth.pack",
            "size": 20 * 1024 * 1024u64,
            "sha256": hex(&Sha256::digest(vec![3u8; 20 * 1024 * 1024])),
        }),
    );
    emulator.state.lock().unwrap().reject_next_graph_request = true;
    emulator.state.lock().unwrap().reject_next_token_refresh = true;
    let error = provider.call_error(
        "object.head",
        json!({"namespace":namespace,"key":"segments/missing.json"}),
    );
    assert_eq!(error["code"], "unauthenticated");
    let secrets = secrets.lock().unwrap();
    assert!(secrets.records.is_empty());
    assert!(secrets.authorization_required);
}

#[test]
fn missing_public_client_configuration_is_a_degraded_health_state() {
    let emulator = Emulator::start();
    let mut provider = ProviderProcess::start_unconfigured(&emulator.base);
    hello(&mut provider);
    let health = provider.call("health", Value::Null);
    assert_eq!(health["ready"], false);
    assert!(health["detail"]
        .as_str()
        .unwrap()
        .contains("public client ID"));
    provider.call("shutdown", Value::Null);
    assert!(provider.child.wait().unwrap().success());
}

fn hello(provider: &mut ProviderProcess) {
    let methods = json!({"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1});
    let limits = json!({"maxControlBytes":1048576,"maxBinaryBytes":4194304,"maxBufferedBytes":8388608,"streamWindowBytes":8388608,"maxConcurrentRequests":64,"maxConcurrentStreams":32,"maxObjectBytes":96*1024*1024u64});
    let ack = provider.call("runtime.hello", json!({"nonce":vec![7;32],"wireVersions":[1],"pluginId":"com.megle.onedrive","version":"1.0.0","accountUuid":"account-one","grantedPermissions":[],"methodVersions":methods,"limits":limits}));
    assert_eq!(ack["pluginId"], "com.megle.onedrive");
}

fn serve_graph(mut stream: TcpStream, base: &str, state: &Arc<Mutex<GraphState>>) {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.windows(4).any(|v| v == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = bytes.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let first = headers.lines().next().unwrap();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap();
    let target = parts.next().unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let body = &bytes[header_end..header_end + length];
    let authorization = headers
        .lines()
        .any(|line| line.to_ascii_lowercase().starts_with("authorization:"));
    let url = Url::parse(&format!("http://emulator{target}")).unwrap();
    let path = url.path().to_owned();
    let path = path.as_str();
    let reject_graph_request = authorization && path.starts_with("/v1.0/") && {
        let mut state = state.lock().unwrap();
        std::mem::take(&mut state.reject_next_graph_request)
    };
    let (status, extra, response) = if reject_graph_request {
        (
            401,
            vec![],
            json!({"error":{"code":"InvalidAuthenticationToken"}})
                .to_string()
                .into_bytes(),
        )
    } else if path.ends_with("/oauth2/v2.0/token") && method == "POST" {
        let reject_refresh = {
            let mut state = state.lock().unwrap();
            std::mem::take(&mut state.reject_next_token_refresh)
        };
        if reject_refresh {
            (
                400,
                vec![],
                json!({"error":"invalid_grant","error_description":"expired refresh credential"})
                    .to_string()
                    .into_bytes(),
            )
        } else {
            (200, vec![], json!({"access_token":"emulator-access","refresh_token":"emulator-refresh","expires_in":3600,"scope":"Files.ReadWrite.AppFolder offline_access"}).to_string().into_bytes())
        }
    } else if path == "/v1.0/delta-gone" && method == "GET" {
        (
            410,
            vec![],
            json!({"error":{"code":"resyncChangesApplyDifferences"}})
                .to_string()
                .into_bytes(),
        )
    } else if path == "/v1.0/throttled" && method == "GET" {
        (
            429,
            vec![(String::from("Retry-After"), String::from("3"))],
            json!({"error":{"code":"activityLimitReached"}})
                .to_string()
                .into_bytes(),
        )
    } else if path == "/v1.0/delta-empty-next" && method == "GET" {
        (
            200,
            vec![],
            json!({"value":[],"@odata.nextLink":format!("{base}v1.0/delta-final-page")})
                .to_string()
                .into_bytes(),
        )
    } else if path == "/v1.0/delta-final-page" && method == "GET" {
        let value = state
            .lock()
            .unwrap()
            .objects
            .get("018f0000-0000-7000-8000-000000000001/segments/immutable.json")
            .unwrap()
            .clone();
        let item = json!({"name":"immutable.json","size":value.len(),"eTag":hex(&Sha256::digest(&value)),"parentReference":{"path":"/drive/root:/Apps/Megle/Megle/libraries/018f0000-0000-7000-8000-000000000001/segments"}});
        (
            200,
            vec![],
            json!({"value":[item.clone(),item],"@odata.deltaLink":format!("{base}v1.0/delta-final")})
                .to_string()
                .into_bytes(),
        )
    } else if path == "/v1.0/me/drive" && method == "GET" {
        (200, vec![], json!({"id":"drive-personal-1","driveType":"personal","owner":{"user":{"id":"object-1","displayName":"Emulator Account"}}}).to_string().into_bytes())
    } else if path == "/v1.0/me/drive/special/approot" && method == "GET" {
        (
            200,
            vec![],
            json!({"id":"approot"}).to_string().into_bytes(),
        )
    } else if path.ends_with("/children") && method == "POST" {
        let value: Value = serde_json::from_slice(body).unwrap();
        if let Some(name) = value["name"].as_str() {
            if uuid::Uuid::parse_str(name).is_ok() {
                state.lock().unwrap().namespaces.insert(name.to_owned());
            }
        }
        (201, vec![], json!({"id":format!("folder-{}",value["name"].as_str().unwrap()),"name":value["name"],"folder":{}}).to_string().into_bytes())
    } else if path.ends_with("/delta") && method == "GET" {
        let state = state.lock().unwrap();
        let namespace = path
            .strip_prefix("/v1.0/me/drive/items/folder-")
            .and_then(|value| value.strip_suffix("/delta"));
        let values = if namespace == Some("libraries") {
            state
                .namespaces
                .iter()
                .map(|namespace| json!({"name":namespace,"folder":{},"size":0,"parentReference":{"path":"/drive/root:/Apps/Megle/Megle/libraries"}}))
                .collect::<Vec<_>>()
        } else {
            state
                .objects
                .iter()
                .filter_map(|(key, value)| {
                    let name = match namespace {
                        Some(namespace) => key.strip_prefix(&format!("{namespace}/"))?,
                        None => key.as_str(),
                    };
                    let (parent, name) = name.rsplit_once('/').unwrap_or(("", name));
                    Some(json!({"name":name,"size":value.len(),"eTag":hex(&Sha256::digest(value)),"parentReference":{"path":format!("/drive/root:/Apps/Megle/Megle/libraries/{}/{}",namespace.unwrap_or_default(),parent).trim_end_matches('/')}}))
                })
                .collect::<Vec<_>>()
        };
        (
            200,
            vec![],
            json!({"value":values,"@odata.deltaLink":format!("{base}v1.0/delta-final")})
                .to_string()
                .into_bytes(),
        )
    } else if path.contains(":/createUploadSession") && method == "POST" {
        let key = graph_key(path, ":/createUploadSession");
        if key.ends_with("quota.pack") {
            (
                507,
                vec![],
                json!({"error":{"code":"quotaLimitReached"}})
                    .to_string()
                    .into_bytes(),
            )
        } else {
            let id = uuid::Uuid::now_v7().to_string();
            state
                .lock()
                .unwrap()
                .uploads
                .insert(id.clone(), (key, Vec::new(), 0));
            (
                200,
                vec![],
                json!({"uploadUrl":format!("{base}upload/{id}?sig=secret"),"expirationDateTime":"2099-01-01T00:00:00Z"})
                    .to_string()
                    .into_bytes(),
            )
        }
    } else if let Some(id) = path.strip_prefix("/upload/") {
        if authorization {
            state.lock().unwrap().capability_bearer_leaks += 1;
        }
        match method {
            "PUT" => {
                let content_range = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-range"))
                            .map(|(_, value)| value.trim().to_owned())
                    })
                    .unwrap();
                let total = content_range
                    .split('/')
                    .nth(1)
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                let mut state = state.lock().unwrap();
                let (complete, next_offset) = {
                    let upload = state.uploads.get_mut(id).unwrap();
                    upload.1.extend_from_slice(body);
                    upload.2 = total;
                    (upload.1.len() as u64 == total, upload.1.len())
                };
                let fault = state.next_upload_fault.take();
                if complete {
                    let (key, bytes, _) = state.uploads.remove(id).unwrap();
                    state.objects.insert(key, bytes);
                    fault.map_or_else(
                        || {
                            (
                                201,
                                vec![],
                                json!({"id":"uploaded"}).to_string().into_bytes(),
                            )
                        },
                        |status| {
                            (
                                status,
                                vec![],
                                json!({"error":{"code":"injectedUploadFault"}})
                                    .to_string()
                                    .into_bytes(),
                            )
                        },
                    )
                } else {
                    fault.map_or_else(
                        || {
                            (
                                202,
                                vec![],
                                json!({"nextExpectedRanges":[format!("{next_offset}-")]})
                                    .to_string()
                                    .into_bytes(),
                            )
                        },
                        |status| {
                            (
                                status,
                                vec![],
                                json!({"error":{"code":"injectedUploadFault"}})
                                    .to_string()
                                    .into_bytes(),
                            )
                        },
                    )
                }
            }
            "GET" => {
                let state = state.lock().unwrap();
                let offset = state.uploads.get(id).map(|v| v.1.len()).unwrap_or(0);
                (
                    200,
                    vec![],
                    json!({"nextExpectedRanges":[format!("{offset}-")]})
                        .to_string()
                        .into_bytes(),
                )
            }
            "DELETE" => {
                state.lock().unwrap().uploads.remove(id);
                (204, vec![], Vec::new())
            }
            _ => (405, vec![], Vec::new()),
        }
    } else if let Some(encoded) = path.strip_prefix("/download/") {
        if authorization {
            state.lock().unwrap().capability_bearer_leaks += 1;
        }
        let key = percent_decode(encoded);
        let mut state = state.lock().unwrap();
        let mut data = state.objects.get(&key).unwrap().clone();
        let download_count = state.download_counts.entry(key.clone()).or_default();
        *download_count += 1;
        let download_count = *download_count;
        drop(state);
        let range = headers.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("range"))
                .map(|(_, value)| value.trim().to_owned())
        });
        if key.ends_with("truncated.json") && !data.is_empty() {
            data.pop();
        }
        if key.ends_with("corrupt-fallback.json") && download_count > 1 && !data.is_empty() {
            data[0] ^= 0xff;
        }
        if let Some(range) = range.filter(|_| {
            !key.ends_with("range-fallback.json") && !key.ends_with("corrupt-fallback.json")
        }) {
            let (start, end) = range
                .strip_prefix("bytes=")
                .unwrap()
                .split_once('-')
                .unwrap();
            let start = start.parse::<usize>().unwrap();
            let end = end.parse::<usize>().unwrap();
            let header_start = if key.ends_with("bad-content-range.json") {
                start + 1
            } else {
                start
            };
            (
                206,
                vec![(
                    String::from("Content-Range"),
                    format!("bytes {header_start}-{end}/{}", data.len()),
                )],
                data[start..=end].to_vec(),
            )
        } else {
            (200, vec![], data)
        }
    } else if path.contains("/special/approot:/") {
        let is_content = path.ends_with(":/content");
        let key = graph_key(path, if is_content { ":/content" } else { "" });
        match method {
            "PUT" => {
                let mut state = state.lock().unwrap();
                if state.objects.contains_key(&key) {
                    (409, vec![], json!({"error":{"code":"nameAlreadyExists"}}).to_string().into_bytes())
                } else {
                    state.objects.insert(key, body.to_vec());
                    (201,vec![],json!({"id":"object","eTag":hex(&Sha256::digest(body))}).to_string().into_bytes())
                }
            }
            "GET" if is_content => (302,vec![("Location".into(),format!("{base}download/{}",percent_encode(&key)))],Vec::new()),
            "GET" => match state.lock().unwrap().objects.get(&key) { Some(data)=>(200,vec![],json!({"id":"object","name":key.rsplit('/').next(),"size":if key.ends_with("over-limit.json") { 96*1024*1024u64+1 } else { data.len() as u64 },"eTag":hex(&Sha256::digest(data))}).to_string().into_bytes()),None=>(404,vec![],json!({"error":{"code":"itemNotFound"}}).to_string().into_bytes()) },
            _ => (405,vec![],Vec::new()),
        }
    } else {
        (
            404,
            vec![],
            json!({"error":{"code":"notFound"}})
                .to_string()
                .into_bytes(),
        )
    };
    let reason = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        302 => "Found",
        404 => "Not Found",
        409 => "Conflict",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n", response.len());
    for (key, value) in extra {
        head.push_str(&format!("{key}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response);
}

fn graph_key(path: &str, suffix: &str) -> String {
    let prefix = "/v1.0/me/drive/special/approot:/Megle/libraries/";
    let rest = path.strip_prefix(prefix).unwrap();
    percent_decode(rest.strip_suffix(suffix).unwrap_or(rest))
}
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn percent_decode(value: &str) -> String {
    url::form_urlencoded::parse(value.as_bytes())
        .map(|(k, v)| format!("{k}{v}"))
        .collect::<String>()
}
fn write_control(output: &mut impl Write, value: &Value) {
    write_frame(output, CONTROL, 0, 0, &serde_json::to_vec(value).unwrap())
}
fn write_frame(output: &mut impl Write, kind: u8, stream: u64, sequence: u32, payload: &[u8]) {
    output
        .write_all(&(payload.len() as u32).to_le_bytes())
        .unwrap();
    output.write_all(&[kind]).unwrap();
    output.write_all(&stream.to_le_bytes()).unwrap();
    output.write_all(&sequence.to_le_bytes()).unwrap();
    output.write_all(payload).unwrap();
    output.flush().unwrap()
}
fn read_frame(input: &mut impl Read) -> (u8, u64, u32, Vec<u8>) {
    let mut header = [0; 17];
    input.read_exact(&mut header).unwrap();
    let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let mut payload = vec![0; size];
    input.read_exact(&mut payload).unwrap();
    (
        header[4],
        u64::from_le_bytes(header[5..13].try_into().unwrap()),
        u32::from_le_bytes(header[13..].try_into().unwrap()),
        payload,
    )
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn platform_runtime_env() -> HashMap<String, String> {
    ["SystemRoot", "WINDIR"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect()
}
