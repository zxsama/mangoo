#[path = "../src/wire.rs"]
mod wire;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use wire::*;

struct Peer {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    directory: tempfile::TempDir,
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Peer {
    fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_mangoo-ppm-decoder"))
            .env_clear()
            .env("MANGOO_DECODER_TEMP_DIR", directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let mut peer = Self {
            child,
            input,
            output,
            directory,
        };
        let methods = std::collections::BTreeMap::from([
            ("health", 1),
            ("diagnostics", 1),
            ("shutdown", 1),
            ("decoder.decode", 1),
        ]);
        let hello=peer.request("runtime.hello",json!({"nonce":vec![7;32],"wireVersions":[1],"pluginId":"com.mangoo.ppm-decoder","version":"0.1.0","capabilities":["file-decoder.v1"],"grantedPermissions":[],"methodVersions":methods,
            "limits":{"maxControlBytes":65536,"maxBinaryBytes":262144,"maxBufferedBytes":524288,"streamWindowBytes":1048576,"maxConcurrentRequests":1,"maxConcurrentStreams":2,"maxObjectBytes":67108864}}));
        let response = peer.control();
        assert_eq!(response.request_id, hello);
        assert_eq!(response.message_type, "response");
        assert!(response.payload.get("accountUuid").is_none());
        peer
    }
    fn request(&mut self, method: &str, payload: serde_json::Value) -> uuid::Uuid {
        let id = uuid::Uuid::now_v7();
        control(
            &mut self.input,
            &Control {
                wire_version: 1,
                message_type: "request".into(),
                request_id: id,
                method: method.into(),
                deadline_ms: 30000,
                binding_epoch: None,
                transport_generation: None,
                payload,
            },
        )
        .unwrap();
        id
    }
    fn control(&mut self) -> Control {
        let frame = read(&mut self.output, 262144).unwrap();
        assert_eq!(frame.kind, CONTROL);
        serde_json::from_slice(&frame.bytes).unwrap()
    }
    fn decode(
        &mut self,
        bytes: &[u8],
        input_id: u64,
        corrupt_hash: bool,
    ) -> Result<(u32, u32, Vec<u8>), String> {
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        self.request(
            "stream.open",
            json!({"streamId":input_id,"size":bytes.len(),"sha256":hex(&hash)}),
        );
        let credit = read(&mut self.output, 262144).unwrap();
        assert_eq!(credit.kind, CREDIT);
        let mut granted = u64::from_le_bytes(credit.bytes.try_into().unwrap()) as usize;
        let job = uuid::Uuid::now_v7().to_string();
        let request_id=self.request("decoder.decode",json!({"jobId":job,"streamId":input_id,"purpose":"thumbnail","size":bytes.len(),"sha256":hex(&hash),"maxEdge":1024,"maxOutputPixels":10000000,"maxSourcePixels":16000000,"maxHeaderBytes":65536}));
        let mut sequence = 1;
        let mut sent = 0;
        while sent < bytes.len() {
            let end = bytes.len().min(granted).min(sent + 262144);
            write(
                &mut self.input,
                Frame {
                    kind: BINARY,
                    id: input_id,
                    sequence,
                    bytes: bytes[sent..end].to_vec(),
                },
            )
            .unwrap();
            sent = end;
            sequence += 1;
            // Drain the sliding credit after each input chunk. This bounds the
            // child stdout pipe even for an input much larger than its window.
            if granted < bytes.len() {
                let credit = read(&mut self.output, 262144).unwrap();
                assert_eq!(credit.kind, CREDIT);
                granted = u64::from_le_bytes(credit.bytes.try_into().unwrap()) as usize;
            }
        }
        let mut end = (bytes.len() as u64).to_le_bytes().to_vec();
        end.extend_from_slice(&hash);
        if corrupt_hash {
            end[8] ^= 1;
        }
        write(
            &mut self.input,
            Frame {
                kind: END,
                id: input_id,
                sequence,
                bytes: end,
            },
        )
        .unwrap();
        let first = self.control();
        if first.message_type == "error" {
            return Err(first.payload["code"].as_str().unwrap().into());
        }
        assert_eq!(first.method, "stream.open");
        let id = first.payload["streamId"].as_u64().unwrap();
        let size = first.payload["size"].as_u64().unwrap();
        let response = self.control();
        assert_eq!(response.request_id, request_id);
        assert_eq!(response.payload["jobId"], job);
        // Deliberately give very small credit and pause. Content response has
        // arrived, but the child must not send beyond actual granted bytes.
        let mut output = Vec::new();
        let mut sequence = 1;
        while output.len() < (size as usize) {
            std::thread::sleep(std::time::Duration::from_millis(1));
            let credit = (output.len() as u64 + 8192).min(size);
            write(
                &mut self.input,
                Frame {
                    kind: CREDIT,
                    id,
                    sequence: 0,
                    bytes: credit.to_le_bytes().to_vec(),
                },
            )
            .unwrap();
            let frame = read(&mut self.output, 262144).unwrap();
            assert_eq!(frame.kind, BINARY);
            assert_eq!(frame.sequence, sequence);
            assert!(output.len() + frame.bytes.len() <= credit as usize);
            output.extend(frame.bytes);
            sequence += 1;
        }
        let end = read(&mut self.output, 262144).unwrap();
        assert_eq!(end.kind, END);
        assert_eq!(end.sequence, sequence);
        assert_eq!(hex(&Sha256::digest(&output)), first.payload["sha256"]);
        Ok((
            response.payload["width"].as_u64().unwrap() as u32,
            response.payload["height"].as_u64().unwrap() as u32,
            output,
        ))
    }
}

#[test]
fn child_real_p3_p6_stream_and_slow_output_credit() {
    let mut peer = Peer::start();
    assert_eq!(
        peer.decode(b"P3\n2 1\n255\n255 0 0 0 255 0\n", 1, false)
            .unwrap()
            .2,
        [255, 0, 0, 255, 0, 255, 0, 255]
    );
    assert_eq!(
        peer.decode(b"P6\n1 1\n255\n\0\0\xff", 3, false).unwrap().2,
        [0, 0, 255, 255]
    );
    assert_eq!(
        peer.decode(b"P3\n1 1\n15\n1 2 3", 5, false).unwrap_err(),
        "unsupported"
    );
    assert_eq!(
        peer.decode(b"P6\n1 1\n255\n\0", 7, false).unwrap_err(),
        "invalid_data"
    );
    assert_eq!(std::fs::read_dir(peer.directory.path()).unwrap().count(), 0);
}

#[test]
fn child_input_larger_than_window_is_content_driven() {
    let mut bytes = b"P6\n2048 1024\n255\n".to_vec();
    bytes.resize(bytes.len() + 2048 * 1024 * 3, 0);
    let start = b"P6\n2048 1024\n255\n".len();
    for pixel in bytes[start..].chunks_exact_mut(3) {
        pixel[1] = 255;
    }
    assert!(bytes.len() > 1048576 * 4);
    let mut peer = Peer::start();
    let (width, height, rgba) = peer.decode(&bytes, 1, false).unwrap();
    assert_eq!((width, height), (1024, 512));
    assert!(rgba.chunks_exact(4).all(|pixel| pixel == [0, 255, 0, 255]));
}

#[test]
fn child_input_bad_hash_closes_without_result() {
    let mut peer = Peer::start();
    let input = b"P6\n1 1\n255\n\0\0\xff";
    let digest: [u8; 32] = Sha256::digest(input).into();
    peer.request(
        "stream.open",
        json!({"streamId":1,"size":input.len(),"sha256":hex(&digest)}),
    );
    let _ = read(&mut peer.output, 262144).unwrap();
    peer.request("decoder.decode",json!({"jobId":uuid::Uuid::now_v7().to_string(),"streamId":1,"purpose":"thumbnail","size":input.len(),"sha256":hex(&digest),"maxEdge":1024,"maxOutputPixels":10000000,"maxSourcePixels":16000000,"maxHeaderBytes":65536}));
    write(
        &mut peer.input,
        Frame {
            kind: BINARY,
            id: 1,
            sequence: 1,
            bytes: input.to_vec(),
        },
    )
    .unwrap();
    let mut end = (input.len() as u64).to_le_bytes().to_vec();
    end.extend_from_slice(&[0u8; 32]);
    write(
        &mut peer.input,
        Frame {
            kind: END,
            id: 1,
            sequence: 2,
            bytes: end,
        },
    )
    .unwrap();
    let mut output = Vec::new();
    peer.output.read_to_end(&mut output).unwrap();
    assert!(output.is_empty());
    assert!(!peer.child.wait().unwrap().success());
}
