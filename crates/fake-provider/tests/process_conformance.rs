use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const CONTROL: u8 = 1;
const BINARY: u8 = 2;
const END: u8 = 3;
const CREDIT: u8 = 4;

struct ProviderProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    next_stream_id: u64,
}

impl ProviderProcess {
    fn start() -> Self {
        Self::start_with_fault(None)
    }

    fn start_with_fault(fault: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_megle-fake-provider"));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(fault) = fault {
            command.env("MEGLE_FAKE_PROVIDER_FAULT", fault);
        }
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Self {
            child,
            stdin,
            stdout,
            next_stream_id: 1,
        }
    }

    fn call(&mut self, method: &str, payload: Value) -> Value {
        let response = self.call_envelope(method, payload);
        assert_eq!(response["messageType"], "response", "{response}");
        response["payload"].clone()
    }

    fn call_envelope(&mut self, method: &str, payload: Value) -> Value {
        let request_id = uuid::Uuid::now_v7();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":request_id,"method":method,"deadlineMs":5000,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":payload}),
        );
        loop {
            let (kind, stream, sequence, payload) = read_frame(&mut self.stdout);
            assert_eq!(kind, CONTROL);
            assert_eq!((stream, sequence), (0, 0));
            let response: Value = serde_json::from_slice(&payload).unwrap();
            if response["requestId"] == request_id.to_string() {
                return response;
            }
        }
    }

    fn call_with_bytes(&mut self, method: &str, mut payload: Value, bytes: &[u8]) -> Value {
        let stream_id = self.next_stream_id;
        self.next_stream_id += 2;
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":uuid::Uuid::now_v7(),"method":"stream.open","deadlineMs":0,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":{"streamId":stream_id,"size":bytes.len(),"sha256":hex(&hash)}}),
        );
        let (kind, id, _, credit) = read_frame(&mut self.stdout);
        assert_eq!((kind, id), (CREDIT, stream_id));
        assert!(u64::from_le_bytes(credit.try_into().unwrap()) >= bytes.len() as u64);
        payload["streamId"] = stream_id.into();
        let request_id = uuid::Uuid::now_v7();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":request_id,"method":method,"deadlineMs":5000,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":payload}),
        );
        write_frame(&mut self.stdin, BINARY, stream_id, 1, bytes);
        let mut end = Vec::from((bytes.len() as u64).to_le_bytes());
        end.extend_from_slice(&hash);
        write_frame(&mut self.stdin, END, stream_id, 2, &end);
        let (_, _, _, response) = read_frame(&mut self.stdout);
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["requestId"], request_id.to_string());
        assert_eq!(response["messageType"], "response", "{response}");
        response["payload"].clone()
    }

    fn object_get(&mut self, key: &str) -> Vec<u8> {
        self.object_get_request("object.get", json!({"key":key,"maxBytes":67108864}))
            .0
    }

    fn object_get_range(&mut self, key: &str, start: u64, end: u64) -> (Vec<u8>, Value) {
        self.object_get_request(
            "object.get_range",
            json!({"key":key,"start":start,"end":end,"maxBytes":end-start}),
        )
    }

    fn object_get_request(&mut self, method: &str, payload: Value) -> (Vec<u8>, Value) {
        let request_id = uuid::Uuid::now_v7();
        write_control(
            &mut self.stdin,
            &json!({"wireVersion":1,"messageType":"request","requestId":request_id,"method":method,"deadlineMs":5000,"bindingEpoch":uuid::Uuid::nil(),"transportGeneration":1,"payload":payload}),
        );
        let (_, _, _, open) = read_frame(&mut self.stdout);
        let open: Value = serde_json::from_slice(&open).unwrap();
        let stream_id = open["payload"]["streamId"].as_u64().unwrap();
        let size = open["payload"]["size"].as_u64().unwrap();
        write_frame(&mut self.stdin, CREDIT, stream_id, 0, &size.to_le_bytes());
        let mut bytes = Vec::new();
        loop {
            let (kind, id, _, payload) = read_frame(&mut self.stdout);
            assert_eq!(id, stream_id);
            if kind == BINARY {
                bytes.extend(payload);
            } else {
                assert_eq!(kind, END);
                break;
            }
        }
        let (_, _, _, response) = read_frame(&mut self.stdout);
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["requestId"], request_id.to_string());
        (bytes, response["payload"].clone())
    }
}

#[test]
fn permission_loss_fails_closed_before_provider_state_changes() {
    let mut provider = ProviderProcess::start_with_fault(Some("permission-loss"));
    let denied = provider.call_envelope(
        "delta.list",
        json!({"scope":{"kind":"library"},"cursor":null,"pageToken":null,"limit":10}),
    );
    assert_eq!(denied["messageType"], "error");
    assert_eq!(denied["payload"]["code"], "authorization_required");
    let diagnostics = provider.call("diagnostics", Value::Null);
    assert_eq!(diagnostics["objects"], 0);
}

#[test]
fn duplicate_reordered_delta_pages_remain_idempotent() {
    let mut provider = ProviderProcess::start_with_fault(Some("reordered-delta"));
    let bytes_a = b"first";
    let bytes_b = b"second";
    for (key, bytes) in [
        ("segments/a", bytes_a.as_slice()),
        ("segments/b", bytes_b.as_slice()),
    ] {
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        assert_eq!(
            provider.call_with_bytes(
                "object.put_immutable",
                json!({"key":key,"sha256":hex(&hash)}),
                bytes
            )["status"],
            "created"
        );
    }
    let page = provider.call(
        "delta.list",
        json!({"scope":{"kind":"library"},"cursor":null,"pageToken":null,"limit":10}),
    );
    let keys = page["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["segments/b", "segments/a", "segments/b"]);
    assert_eq!(provider.object_get("segments/a"), bytes_a);
    assert_eq!(provider.object_get("segments/b"), bytes_b);
}

#[test]
fn short_range_response_is_detectably_short_and_not_claimed_as_complete() {
    let mut provider = ProviderProcess::start_with_fault(Some("short-range"));
    let bytes = b"0123456789abcdef";
    let hash: [u8; 32] = Sha256::digest(bytes).into();
    provider.call_with_bytes(
        "object.put_immutable",
        json!({"key":"content/data/a","sha256":hex(&hash)}),
        bytes,
    );
    let (range, metadata) = provider.object_get_range("content/data/a", 2, 12);
    assert_eq!(metadata["responseStart"], 2);
    assert_eq!(metadata["responseEnd"], 12);
    assert_eq!(
        range.len(),
        9,
        "fault must return fewer bytes than declared"
    );
    assert_ne!(range.len() as u64, 12 - 2);
}

impl Drop for ProviderProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn bundled_process_conforms_to_immutable_delta_upload_and_auth_contract() {
    let mut provider = ProviderProcess::start();
    let method_versions = json!({"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1});
    let limits = json!({"maxControlBytes":1048576,"maxBinaryBytes":4194304,"maxBufferedBytes":8388608,"streamWindowBytes":8388608,"maxConcurrentRequests":64,"maxConcurrentStreams":32,"maxObjectBytes":67108864});
    let hello = provider.call("runtime.hello", json!({"nonce":vec![0;32],"wireVersions":[1],"pluginId":"com.megle.fake-sync","version":"1.0.0","accountUuid":"fake-account","grantedPermissions":[],"methodVersions":method_versions,"limits":limits}));
    assert_eq!(hello["wireVersion"], 1);
    assert_eq!(hello["accountUuid"], "fake-account");
    let bytes = b"process-conformance";
    let hash: [u8; 32] = Sha256::digest(bytes).into();
    assert_eq!(
        provider.call_with_bytes(
            "object.put_immutable",
            json!({"key":"segments/a","sha256":hex(&hash)}),
            bytes
        )["status"],
        "created"
    );
    assert_eq!(provider.object_get("segments/a"), bytes);
    let (range_bytes, ranged) = provider.object_get_range("segments/a", 1, 8);
    assert_eq!(range_bytes, &bytes[1..8]);
    assert_eq!(ranged["responseStart"], 1);
    assert_eq!(ranged["responseEnd"], 8);
    assert_eq!(ranged["wholeObject"], false);
    assert_eq!(
        provider.call(
            "delta.list",
            json!({"scope":{"kind":"library"},"cursor":null,"pageToken":null,"limit":1})
        )["objects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let upload = provider.call(
        "upload.begin",
        json!({"key":"checkpoints/a","size":4,"sha256":hex(&Sha256::digest(b"next"))}),
    );
    let upload_id = upload["uploadId"].as_str().unwrap();
    provider.call_with_bytes(
        "upload.write",
        json!({"uploadId":upload_id,"offset":0}),
        b"next",
    );
    assert_eq!(
        provider.call("upload.commit", json!({"uploadId":upload_id}))["status"],
        "created"
    );
    assert!(
        provider.call("auth.complete", json!({"flowId":"test","fields":{}}))["authenticated"]
            .as_bool()
            .unwrap()
    );
    provider.call("shutdown", Value::Null);
    assert!(provider.child.wait().unwrap().success());
}

fn write_control(output: &mut impl Write, value: &Value) {
    write_frame(output, CONTROL, 0, 0, &serde_json::to_vec(value).unwrap());
}
fn write_frame(output: &mut impl Write, kind: u8, stream: u64, sequence: u32, payload: &[u8]) {
    output
        .write_all(&(payload.len() as u32).to_le_bytes())
        .unwrap();
    output.write_all(&[kind]).unwrap();
    output.write_all(&stream.to_le_bytes()).unwrap();
    output.write_all(&sequence.to_le_bytes()).unwrap();
    output.write_all(payload).unwrap();
    output.flush().unwrap();
}
fn read_frame(input: &mut impl Read) -> (u8, u64, u32, Vec<u8>) {
    let mut header = [0u8; 17];
    input.read_exact(&mut header).unwrap();
    let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let mut payload = vec![0u8; size];
    input.read_exact(&mut payload).unwrap();
    (
        header[4],
        u64::from_le_bytes(header[5..13].try_into().unwrap()),
        u32::from_le_bytes(header[13..17].try_into().unwrap()),
        payload,
    )
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
