use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Notify};
use uuid::Uuid;

use super::frame::{encode, Decoder, Frame, FrameKind, FrameLimits, ProtocolError};
use super::stream::{decode_credit, decode_end, encode_credit, encode_end, Peer, Streams};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlEnvelope {
    pub wire_version: u16,
    pub message_type: ControlType,
    pub request_id: Uuid,
    pub method: String,
    pub deadline_ms: u64,
    pub binding_epoch: Option<String>,
    pub transport_generation: Option<u64>,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    Request,
    Response,
    Error,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    Closed,
    DeadlineExceeded,
    Protocol,
    Remote {
        code: String,
        retryable: bool,
        retry_after_ms: Option<u64>,
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamOpen {
    stream_id: u64,
    size: u64,
    sha256: String,
}

struct PendingCall {
    response: oneshot::Sender<Result<Value, RpcError>>,
    method: String,
    binding_epoch: Option<String>,
    transport_generation: Option<u64>,
}

type Pending = Arc<Mutex<HashMap<Uuid, PendingCall>>>;

pub trait ReverseRpcHandler: Send + Sync {
    fn handle(&self, method: &str, payload: &Value) -> Result<Value, String>;
}

#[derive(Default)]
struct IncomingData {
    open: HashMap<u64, IncomingObject>,
    completed: HashMap<u64, Result<PathBuf, RpcError>>,
}

struct IncomingObject {
    path: PathBuf,
    file: File,
    size: u64,
    written: u64,
    granted: u64,
}

struct SharedStreams {
    outgoing: Mutex<Streams>,
    incoming: Mutex<Streams>,
    incoming_data: Mutex<IncomingData>,
    changed: Notify,
    max_chunk: usize,
    stream_window: usize,
    max_object: u64,
    max_requests: usize,
    max_streams: usize,
    closed: AtomicBool,
    handshake_complete: AtomicBool,
}

#[derive(Clone)]
pub struct FramedSession {
    tx: mpsc::Sender<Frame>,
    pending: Pending,
    streams: Arc<SharedStreams>,
    wire_version: u16,
}

impl FramedSession {
    #[cfg(test)]
    pub fn start<R, W>(reader: R, writer: W, limits: FrameLimits, wire_version: u16) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::start_inner(reader, writer, limits, wire_version, None, true)
    }

    pub fn start_with_handler<R, W>(
        reader: R,
        writer: W,
        limits: FrameLimits,
        wire_version: u16,
        reverse: Option<Arc<dyn ReverseRpcHandler>>,
    ) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::start_inner(reader, writer, limits, wire_version, reverse, false)
    }

    fn start_inner<R, W>(
        mut reader: R,
        mut writer: W,
        limits: FrameLimits,
        wire_version: u16,
        reverse: Option<Arc<dyn ReverseRpcHandler>>,
        handshake_complete: bool,
    ) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<Frame>(64);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let streams = Arc::new(SharedStreams {
            outgoing: Mutex::new(Streams::with_object_limit(
                Peer::Host,
                32,
                limits.max_object,
            )),
            incoming: Mutex::new(Streams::with_object_limit(
                Peer::Plugin,
                32,
                limits.max_object,
            )),
            incoming_data: Mutex::new(IncomingData::default()),
            changed: Notify::new(),
            max_chunk: limits.max_binary,
            stream_window: limits.stream_window,
            max_object: limits.max_object,
            max_requests: 64,
            max_streams: 32,
            closed: AtomicBool::new(false),
            handshake_complete: AtomicBool::new(handshake_complete),
        });
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let Ok(bytes) = encode(&frame) else { break };
                if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
        });
        let reader_pending = Arc::clone(&pending);
        let reader_streams = Arc::clone(&streams);
        let reader_tx = tx.clone();
        tokio::spawn(async move {
            let mut decoder = Decoder::new(limits);
            let mut bytes = [0_u8; 16 * 1024];
            let mut protocol_ok = true;
            'read: loop {
                let count = match reader.read(&mut bytes).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let frames = match decoder.push(&bytes[..count]) {
                    Ok(v) => v,
                    Err(_) => {
                        protocol_ok = false;
                        break;
                    }
                };
                for frame in frames {
                    if !reader_streams.handshake_complete.load(Ordering::Acquire)
                        && frame.kind != FrameKind::Control
                    {
                        protocol_ok = false;
                        break 'read;
                    }
                    let result = match frame.kind {
                        FrameKind::Control => {
                            handle_control(
                                &frame,
                                wire_version,
                                &reader_pending,
                                &reader_streams,
                                &reader_tx,
                                reverse.as_ref(),
                            )
                            .await
                        }
                        FrameKind::Credit => handle_credit(&frame, &reader_streams),
                        FrameKind::Binary => {
                            handle_binary(&frame, &reader_streams, &reader_tx).await
                        }
                        FrameKind::End => handle_end(&frame, &reader_streams),
                        FrameKind::Abort => handle_abort(&frame, &reader_streams),
                        FrameKind::Ping => reader_tx
                            .send(Frame {
                                kind: FrameKind::Pong,
                                stream_id: 0,
                                sequence: 0,
                                payload: frame.payload,
                            })
                            .await
                            .map_err(|_| RpcError::Closed),
                        FrameKind::Pong => Ok(()),
                    };
                    if result.is_err() {
                        protocol_ok = false;
                        break 'read;
                    }
                }
            }
            let failure = if protocol_ok {
                RpcError::Closed
            } else {
                RpcError::Protocol
            };
            reader_streams.closed.store(true, Ordering::Release);
            if let Ok(mut pending) = reader_pending.lock() {
                for (_, call) in pending.drain() {
                    let _ = call.response.send(Err(failure.clone()));
                }
            }
            if let Ok(mut data) = reader_streams.incoming_data.lock() {
                for id in data.open.keys().copied().collect::<Vec<_>>() {
                    data.completed.insert(id, Err(failure.clone()));
                }
                for (_, object) in data.open.drain() {
                    let _ = std::fs::remove_file(object.path);
                }
            }
            reader_streams.changed.notify_waiters();
        });
        Self {
            tx,
            pending,
            streams,
            wire_version,
        }
    }

    pub async fn call(
        &self,
        method: &str,
        payload: Value,
        deadline: Duration,
        binding_epoch: Option<String>,
        transport_generation: Option<u64>,
    ) -> Result<Value, RpcError> {
        let (request_id, response) = self
            .begin_call(
                method,
                payload,
                deadline,
                binding_epoch,
                transport_generation,
            )
            .await?;
        self.wait_call(request_id, response, deadline).await
    }

    async fn begin_call(
        &self,
        method: &str,
        payload: Value,
        deadline: Duration,
        binding_epoch: Option<String>,
        transport_generation: Option<u64>,
    ) -> Result<(Uuid, oneshot::Receiver<Result<Value, RpcError>>), RpcError> {
        let request_id = Uuid::now_v7();
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| RpcError::Closed)?;
            if pending.len() >= self.streams.max_requests {
                return Err(RpcError::Protocol);
            }
            pending.insert(
                request_id,
                PendingCall {
                    response: response_tx,
                    method: method.to_owned(),
                    binding_epoch: binding_epoch.clone(),
                    transport_generation,
                },
            );
        }
        if let Err(error) = self
            .send(ControlEnvelope {
                wire_version: self.wire_version,
                message_type: ControlType::Request,
                request_id,
                method: method.to_owned(),
                deadline_ms: deadline.as_millis().min(u64::MAX as u128) as u64,
                binding_epoch,
                transport_generation,
                payload,
            })
            .await
        {
            self.pending
                .lock()
                .map_err(|_| RpcError::Closed)?
                .remove(&request_id);
            return Err(error);
        }
        Ok((request_id, response_rx))
    }

    async fn wait_call(
        &self,
        request_id: Uuid,
        response: oneshot::Receiver<Result<Value, RpcError>>,
        deadline: Duration,
    ) -> Result<Value, RpcError> {
        match tokio::time::timeout(deadline, response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => {
                self.pending
                    .lock()
                    .map_err(|_| RpcError::Closed)?
                    .remove(&request_id);
                let _ = self
                    .send(ControlEnvelope {
                        wire_version: self.wire_version,
                        message_type: ControlType::Cancel,
                        request_id,
                        method: "cancel".into(),
                        deadline_ms: 0,
                        binding_epoch: None,
                        transport_generation: None,
                        payload: Value::Null,
                    })
                    .await;
                Err(RpcError::DeadlineExceeded)
            }
        }
    }

    /// Opens an odd Host-owned stream, correlates it to the RPC payload using
    /// `streamId`, and transfers bytes only after cumulative receiver credit.
    pub async fn call_with_bytes(
        &self,
        method: &str,
        mut payload: Value,
        bytes: &[u8],
        deadline: Duration,
        binding_epoch: Option<String>,
        transport_generation: Option<u64>,
    ) -> Result<Value, RpcError> {
        if !payload.is_object() {
            return Err(RpcError::Protocol);
        }
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let id = self.open_outgoing(bytes.len() as u64, digest).await?;
        payload
            .as_object_mut()
            .expect("payload shape checked")
            .insert("streamId".into(), Value::from(id));
        let call = self
            .begin_call(
                method,
                payload,
                deadline,
                binding_epoch,
                transport_generation,
            )
            .await;
        let (request_id, response) = match call {
            Ok(call) => call,
            Err(error) => {
                let _ = self.abort_outgoing(id).await;
                return Err(error);
            }
        };
        let send = self.send_outgoing(id, bytes, digest);
        let wait = self.wait_call(request_id, response, deadline);
        match tokio::try_join!(send, wait) {
            Ok((_, result)) => Ok(result),
            Err(error) => {
                let _ = self.abort_outgoing(id).await;
                Err(error)
            }
        }
    }

    pub async fn receive_file(
        &self,
        stream_id: u64,
        deadline: Duration,
    ) -> Result<PathBuf, RpcError> {
        let wait = async {
            loop {
                let notified = self.streams.changed.notified();
                if self.streams.closed.load(Ordering::Acquire) {
                    return Err(RpcError::Closed);
                }
                if let Some(result) = self
                    .streams
                    .incoming_data
                    .lock()
                    .map_err(|_| RpcError::Closed)?
                    .completed
                    .remove(&stream_id)
                {
                    return result;
                }
                notified.await;
            }
        };
        tokio::time::timeout(deadline, wait)
            .await
            .map_err(|_| RpcError::DeadlineExceeded)?
    }

    #[cfg(test)]
    async fn receive_bytes(&self, stream_id: u64, deadline: Duration) -> Result<Vec<u8>, RpcError> {
        let path = self.receive_file(stream_id, deadline).await?;
        let bytes = std::fs::read(&path).map_err(|_| RpcError::Closed);
        let _ = std::fs::remove_file(path);
        bytes
    }

    pub async fn cancel_binding(
        &self,
        binding_epoch: &str,
        before_generation: u64,
    ) -> Result<(), RpcError> {
        let request_ids = self
            .pending
            .lock()
            .map_err(|_| RpcError::Closed)?
            .iter()
            .filter_map(|(request_id, call)| {
                (call.binding_epoch.as_deref() == Some(binding_epoch)
                    && call
                        .transport_generation
                        .is_some_and(|value| value < before_generation))
                .then_some(*request_id)
            })
            .collect::<Vec<_>>();
        for request_id in request_ids {
            if let Some(call) = self
                .pending
                .lock()
                .map_err(|_| RpcError::Closed)?
                .remove(&request_id)
            {
                let _ = call.response.send(Err(RpcError::Remote {
                    code: "cancelled".into(),
                    retryable: false,
                    retry_after_ms: None,
                    detail: "binding transport generation was fenced".into(),
                }));
            }
            self.send(ControlEnvelope {
                wire_version: self.wire_version,
                message_type: ControlType::Cancel,
                request_id,
                method: "cancel".into(),
                deadline_ms: 0,
                binding_epoch: Some(binding_epoch.to_owned()),
                transport_generation: Some(before_generation),
                payload: Value::Null,
            })
            .await?;
        }
        Ok(())
    }

    async fn open_outgoing(&self, size: u64, sha256: [u8; 32]) -> Result<u64, RpcError> {
        let id = self
            .streams
            .outgoing
            .lock()
            .map_err(|_| RpcError::Closed)?
            .allocate(size, sha256)?;
        let payload = serde_json::to_value(StreamOpen {
            stream_id: id,
            size,
            sha256: hex(&sha256),
        })
        .map_err(|_| RpcError::Protocol)?;
        if let Err(error) = self
            .send(ControlEnvelope {
                wire_version: self.wire_version,
                message_type: ControlType::Request,
                request_id: Uuid::now_v7(),
                method: "stream.open".into(),
                deadline_ms: 0,
                binding_epoch: None,
                transport_generation: None,
                payload,
            })
            .await
        {
            self.streams
                .outgoing
                .lock()
                .map_err(|_| RpcError::Closed)?
                .discard(id);
            return Err(error);
        }
        Ok(id)
    }

    async fn send_outgoing(&self, id: u64, bytes: &[u8], sha256: [u8; 32]) -> Result<(), RpcError> {
        let mut offset = 0usize;
        let mut sequence = 1u32;
        while offset < bytes.len() {
            loop {
                let notified = self.streams.changed.notified();
                if self.streams.closed.load(Ordering::Acquire) {
                    return Err(RpcError::Closed);
                }
                let credit = self
                    .streams
                    .outgoing
                    .lock()
                    .map_err(|_| RpcError::Closed)?
                    .credit(id)? as usize;
                if credit > offset {
                    let end = bytes
                        .len()
                        .min(credit)
                        .min(offset.saturating_add(self.streams.max_chunk));
                    let frame = Frame {
                        kind: FrameKind::Binary,
                        stream_id: id,
                        sequence,
                        payload: bytes[offset..end].to_vec(),
                    };
                    self.tx
                        .send(frame.clone())
                        .await
                        .map_err(|_| RpcError::Closed)?;
                    self.streams
                        .outgoing
                        .lock()
                        .map_err(|_| RpcError::Closed)?
                        .accept_data(&frame)?;
                    offset = end;
                    sequence = sequence.checked_add(1).ok_or(RpcError::Protocol)?;
                    break;
                }
                notified.await;
            }
        }
        let frame = Frame {
            kind: FrameKind::End,
            stream_id: id,
            sequence,
            payload: encode_end(bytes.len() as u64, sha256),
        };
        self.tx
            .send(frame.clone())
            .await
            .map_err(|_| RpcError::Closed)?;
        let mut outgoing = self.streams.outgoing.lock().map_err(|_| RpcError::Closed)?;
        outgoing.finish(&frame, bytes.len() as u64, sha256)?;
        outgoing.retire(id)?;
        Ok(())
    }

    async fn abort_outgoing(&self, id: u64) -> Result<(), RpcError> {
        let sequence = match self
            .streams
            .outgoing
            .lock()
            .map_err(|_| RpcError::Closed)?
            .next_sequence(id)
        {
            Ok(sequence) => sequence,
            Err(_) => return Ok(()),
        };
        let frame = Frame {
            kind: FrameKind::Abort,
            stream_id: id,
            sequence,
            payload: Vec::new(),
        };
        let _ = self.tx.send(frame.clone()).await;
        let mut outgoing = self.streams.outgoing.lock().map_err(|_| RpcError::Closed)?;
        outgoing.abort(&frame)?;
        outgoing.retire(id)?;
        Ok(())
    }

    async fn send(&self, envelope: ControlEnvelope) -> Result<(), RpcError> {
        let payload = serde_json::to_vec(&envelope).map_err(|_| RpcError::Protocol)?;
        self.tx
            .send(Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: 0,
                payload,
            })
            .await
            .map_err(|_| RpcError::Closed)
    }

    pub fn complete_handshake(&self) {
        self.streams
            .handshake_complete
            .store(true, Ordering::Release);
    }
}

async fn handle_control(
    frame: &Frame,
    wire: u16,
    pending: &Pending,
    streams: &SharedStreams,
    tx: &mpsc::Sender<Frame>,
    reverse: Option<&Arc<dyn ReverseRpcHandler>>,
) -> Result<(), RpcError> {
    let envelope: ControlEnvelope =
        serde_json::from_slice(&frame.payload).map_err(|_| RpcError::Protocol)?;
    if envelope.wire_version != wire {
        return Err(RpcError::Protocol);
    }
    if !streams.handshake_complete.load(Ordering::Acquire) {
        let hello_response = matches!(
            envelope.message_type,
            ControlType::Response | ControlType::Error
        ) && pending
            .lock()
            .map_err(|_| RpcError::Closed)?
            .get(&envelope.request_id)
            .is_some_and(|call| call.method == "runtime.hello");
        if !hello_response {
            return Err(RpcError::Protocol);
        }
    }
    if envelope.message_type == ControlType::Request && envelope.method == "stream.open" {
        let open: StreamOpen =
            serde_json::from_value(envelope.payload).map_err(|_| RpcError::Protocol)?;
        let sha256 = hex32(&open.sha256)?;
        if open.size > streams.max_object {
            return Err(RpcError::Protocol);
        }
        {
            let data = streams.incoming_data.lock().map_err(|_| RpcError::Closed)?;
            if data.open.len().saturating_add(data.completed.len()) >= streams.max_streams {
                return Err(RpcError::Protocol);
            }
        }
        streams
            .incoming
            .lock()
            .map_err(|_| RpcError::Closed)?
            .register(open.stream_id, open.size, sha256)?;
        let path = std::env::temp_dir().join(format!(
            "megle-plugin-rpc-{}-{}.stream",
            std::process::id(),
            Uuid::now_v7().simple()
        ));
        let file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(_) => {
                streams
                    .incoming
                    .lock()
                    .map_err(|_| RpcError::Closed)?
                    .discard(open.stream_id);
                return Err(RpcError::Closed);
            }
        };
        {
            let mut data = streams.incoming_data.lock().map_err(|_| RpcError::Closed)?;
            data.open.insert(
                open.stream_id,
                IncomingObject {
                    path,
                    file,
                    size: open.size,
                    written: 0,
                    granted: open.size.min(streams.stream_window as u64),
                },
            );
        }
        let credit = open.size.min(streams.stream_window as u64);
        streams
            .incoming
            .lock()
            .map_err(|_| RpcError::Closed)?
            .grant_credit(open.stream_id, credit)?;
        tx.send(Frame {
            kind: FrameKind::Credit,
            stream_id: open.stream_id,
            sequence: 0,
            payload: encode_credit(credit),
        })
        .await
        .map_err(|_| RpcError::Closed)?;
        return Ok(());
    }
    if envelope.message_type == ControlType::Request {
        let result = reverse
            .ok_or(RpcError::Protocol)?
            .handle(&envelope.method, &envelope.payload);
        let response = ControlEnvelope {
            wire_version: wire, message_type: if result.is_ok() { ControlType::Response } else { ControlType::Error },
            request_id: envelope.request_id, method: envelope.method, deadline_ms: 0,
            binding_epoch: envelope.binding_epoch, transport_generation: envelope.transport_generation,
            payload: result.unwrap_or_else(|detail| serde_json::json!({"code":"permission_denied","retryable":false,"detail":detail})),
        };
        tx.send(Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: 0,
            payload: serde_json::to_vec(&response).map_err(|_| RpcError::Protocol)?,
        })
        .await
        .map_err(|_| RpcError::Closed)?;
        return Ok(());
    }
    if !matches!(
        envelope.message_type,
        ControlType::Response | ControlType::Error
    ) {
        return Ok(());
    }
    let sender = {
        let mut pending = pending.lock().map_err(|_| RpcError::Closed)?;
        if let Some(call) = pending.get(&envelope.request_id) {
            if call.binding_epoch != envelope.binding_epoch
                || call.transport_generation != envelope.transport_generation
            {
                return Err(RpcError::Protocol);
            }
        }
        pending.remove(&envelope.request_id)
    };
    if let Some(call) = sender {
        let result = if envelope.message_type == ControlType::Response {
            Ok(envelope.payload)
        } else {
            Err(RpcError::Remote {
                code: envelope
                    .payload
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("internal")
                    .to_owned(),
                retryable: envelope
                    .payload
                    .get("retryable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                retry_after_ms: envelope.payload.get("retryAfterMs").and_then(Value::as_u64),
                detail: redact_text(
                    envelope
                        .payload
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or("remote Provider error"),
                    512,
                ),
            })
        };
        let _ = call.response.send(result);
    }
    Ok(())
}

fn handle_credit(frame: &Frame, streams: &SharedStreams) -> Result<(), RpcError> {
    let cumulative = decode_credit(&frame.payload)?;
    streams
        .outgoing
        .lock()
        .map_err(|_| RpcError::Closed)?
        .grant_credit(frame.stream_id, cumulative)?;
    streams.changed.notify_waiters();
    Ok(())
}

async fn handle_binary(
    frame: &Frame,
    streams: &SharedStreams,
    tx: &mpsc::Sender<Frame>,
) -> Result<(), RpcError> {
    streams
        .incoming
        .lock()
        .map_err(|_| RpcError::Closed)?
        .accept_data(frame)?;
    let next_credit = {
        let mut data = streams.incoming_data.lock().map_err(|_| RpcError::Closed)?;
        let object = data
            .open
            .get_mut(&frame.stream_id)
            .ok_or(RpcError::Protocol)?;
        let next = object
            .written
            .checked_add(frame.payload.len() as u64)
            .ok_or(RpcError::Protocol)?;
        if next > object.size {
            return Err(RpcError::Protocol);
        }
        object
            .file
            .write_all(&frame.payload)
            .map_err(|_| RpcError::Closed)?;
        object.written = next;
        let next_credit = object
            .size
            .min(object.written.saturating_add(streams.stream_window as u64));
        if next_credit > object.granted {
            object.granted = next_credit;
            Some(next_credit)
        } else {
            None
        }
    };
    if let Some(next_credit) = next_credit {
        streams
            .incoming
            .lock()
            .map_err(|_| RpcError::Closed)?
            .grant_credit(frame.stream_id, next_credit)?;
        tx.send(Frame {
            kind: FrameKind::Credit,
            stream_id: frame.stream_id,
            sequence: 0,
            payload: encode_credit(next_credit),
        })
        .await
        .map_err(|_| RpcError::Closed)?;
    }
    Ok(())
}

fn handle_end(frame: &Frame, streams: &SharedStreams) -> Result<(), RpcError> {
    let (size, sha256) = decode_end(&frame.payload)?;
    streams
        .incoming
        .lock()
        .map_err(|_| RpcError::Closed)?
        .finish(frame, size, sha256)?;
    streams
        .incoming
        .lock()
        .map_err(|_| RpcError::Closed)?
        .retire(frame.stream_id)?;
    let mut data = streams.incoming_data.lock().map_err(|_| RpcError::Closed)?;
    let mut object = data
        .open
        .remove(&frame.stream_id)
        .ok_or(RpcError::Protocol)?;
    if object.written != size {
        let _ = std::fs::remove_file(&object.path);
        return Err(RpcError::Protocol);
    }
    object.file.flush().map_err(|_| RpcError::Closed)?;
    object.file.sync_all().map_err(|_| RpcError::Closed)?;
    drop(object.file);
    data.completed.insert(frame.stream_id, Ok(object.path));
    drop(data);
    streams.changed.notify_waiters();
    Ok(())
}

fn handle_abort(frame: &Frame, streams: &SharedStreams) -> Result<(), RpcError> {
    streams
        .incoming
        .lock()
        .map_err(|_| RpcError::Closed)?
        .abort(frame)?;
    streams
        .incoming
        .lock()
        .map_err(|_| RpcError::Closed)?
        .retire(frame.stream_id)?;
    let mut data = streams.incoming_data.lock().map_err(|_| RpcError::Closed)?;
    if let Some(object) = data.open.remove(&frame.stream_id) {
        let _ = std::fs::remove_file(object.path);
    }
    data.completed
        .insert(frame.stream_id, Err(RpcError::Closed));
    drop(data);
    streams.changed.notify_waiters();
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hex32(value: &str) -> Result<[u8; 32], RpcError> {
    if value.len() != 64 {
        return Err(RpcError::Protocol);
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| RpcError::Protocol)?;
    }
    Ok(out)
}

pub fn redact_stderr_jsonl(line: &[u8], max_bytes: usize) -> String {
    let truncated = &line[..line.len().min(max_bytes)];
    let parsed: Value = serde_json::from_slice(truncated)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(truncated).into_owned()));
    redact_text(&parsed.to_string(), max_bytes)
}
fn redact_text(value: &str, max_bytes: usize) -> String {
    let mut out = value.to_owned();
    for marker in [
        "access_token",
        "refresh_token",
        "authorization",
        "password",
        "client_secret",
        "code",
    ] {
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(index) = lower.find(marker) else {
                break;
            };
            let start = index + marker.len();
            let end = out[start..]
                .find([',', '}', '\n'])
                .map_or(out.len(), |v| start + v);
            if out[start..end].contains("[REDACTED]") {
                break;
            }
            out.replace_range(start..end, ":[REDACTED]");
        }
    }
    out.chars().take(max_bytes).collect()
}

impl From<ProtocolError> for RpcError {
    fn from(_: ProtocolError) -> Self {
        Self::Protocol
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Frame {
        let mut header = [0; 17];
        reader.read_exact(&mut header).await.unwrap();
        let n = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let mut p = vec![0; n];
        reader.read_exact(&mut p).await.unwrap();
        Frame {
            kind: FrameKind::try_from(header[4]).unwrap(),
            stream_id: u64::from_le_bytes(header[5..13].try_into().unwrap()),
            sequence: u32::from_le_bytes(header[13..17].try_into().unwrap()),
            payload: p,
        }
    }
    async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: Frame) {
        writer.write_all(&encode(&frame).unwrap()).await.unwrap();
    }
    fn envelope(frame: &Frame) -> ControlEnvelope {
        serde_json::from_slice(&frame.payload).unwrap()
    }

    #[tokio::test]
    async fn binary_upload_obeys_credit_and_is_not_in_control_json() {
        let (host, mut child) = tokio::io::duplex(16 * 1024);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        let task = tokio::spawn(async move {
            let open = read_frame(&mut child).await;
            let open_env = envelope(&open);
            assert_eq!(open_env.method, "stream.open");
            let desc: StreamOpen = serde_json::from_value(open_env.payload).unwrap();
            let request = read_frame(&mut child).await;
            assert!(!String::from_utf8_lossy(&request.payload).contains("ZGF0YQ=="));
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Credit,
                    stream_id: desc.stream_id,
                    sequence: 0,
                    payload: encode_credit(2),
                },
            )
            .await;
            let first = read_frame(&mut child).await;
            assert_eq!(first.payload, b"da");
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Credit,
                    stream_id: desc.stream_id,
                    sequence: 0,
                    payload: encode_credit(4),
                },
            )
            .await;
            let second = read_frame(&mut child).await;
            assert_eq!(second.payload, b"ta");
            let _end = read_frame(&mut child).await;
            let mut response = envelope(&request);
            response.message_type = ControlType::Response;
            response.payload = serde_json::json!({"ok":true});
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Control,
                    stream_id: 0,
                    sequence: 0,
                    payload: serde_json::to_vec(&response).unwrap(),
                },
            )
            .await;
        });
        assert!(session
            .call_with_bytes(
                "put",
                serde_json::json!({"key":"x"}),
                b"data",
                Duration::from_secs(1),
                None,
                None,
            )
            .await
            .unwrap()["ok"]
            .as_bool()
            .unwrap());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn receives_even_stream_and_validates_hash() {
        let (host, mut child) = tokio::io::duplex(16 * 1024);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        let digest: [u8; 32] = Sha256::digest(b"data").into();
        let open = ControlEnvelope {
            wire_version: 1,
            message_type: ControlType::Request,
            request_id: Uuid::now_v7(),
            method: "stream.open".into(),
            deadline_ms: 0,
            binding_epoch: None,
            transport_generation: None,
            payload: serde_json::to_value(StreamOpen {
                stream_id: 2,
                size: 4,
                sha256: hex(&digest),
            })
            .unwrap(),
        };
        write_frame(
            &mut child,
            Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: 0,
                payload: serde_json::to_vec(&open).unwrap(),
            },
        )
        .await;
        let credit = read_frame(&mut child).await;
        assert_eq!(decode_credit(&credit.payload).unwrap(), 4);
        write_frame(
            &mut child,
            Frame {
                kind: FrameKind::Binary,
                stream_id: 2,
                sequence: 1,
                payload: b"data".to_vec(),
            },
        )
        .await;
        write_frame(
            &mut child,
            Frame {
                kind: FrameKind::End,
                stream_id: 2,
                sequence: 2,
                payload: encode_end(4, digest),
            },
        )
        .await;
        assert_eq!(
            session
                .receive_bytes(2, Duration::from_secs(1))
                .await
                .unwrap(),
            b"data"
        );
    }

    #[tokio::test]
    async fn receives_object_larger_than_memory_window_with_sliding_credit() {
        let limits = FrameLimits {
            max_control: 1024 * 1024,
            max_binary: 1024,
            max_buffered: 16 * 1024,
            stream_window: 2048,
            max_object: 8192,
        };
        let (host, mut child) = tokio::io::duplex(16 * 1024);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, limits, 1);
        let bytes = vec![0x5a; 6144];
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let open = ControlEnvelope {
            wire_version: 1,
            message_type: ControlType::Request,
            request_id: Uuid::now_v7(),
            method: "stream.open".into(),
            deadline_ms: 0,
            binding_epoch: None,
            transport_generation: None,
            payload: serde_json::to_value(StreamOpen {
                stream_id: 2,
                size: bytes.len() as u64,
                sha256: hex(&digest),
            })
            .unwrap(),
        };
        write_frame(
            &mut child,
            Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: 0,
                payload: serde_json::to_vec(&open).unwrap(),
            },
        )
        .await;
        assert_eq!(
            decode_credit(&read_frame(&mut child).await.payload).unwrap(),
            2048
        );
        for (index, chunk) in bytes.chunks(1024).enumerate() {
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Binary,
                    stream_id: 2,
                    sequence: index as u32 + 1,
                    payload: chunk.to_vec(),
                },
            )
            .await;
            if index < 4 {
                let credit = read_frame(&mut child).await;
                assert_eq!(
                    decode_credit(&credit.payload).unwrap(),
                    ((index + 3) * 1024).min(bytes.len()) as u64
                );
            }
        }
        write_frame(
            &mut child,
            Frame {
                kind: FrameKind::End,
                stream_id: 2,
                sequence: 7,
                payload: encode_end(bytes.len() as u64, digest),
            },
        )
        .await;
        assert_eq!(
            session
                .receive_bytes(2, Duration::from_secs(1))
                .await
                .unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn deadline_sends_cancel_and_late_response_is_ignored() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        tokio::spawn(async move {
            let request = read_frame(&mut child).await;
            let cancel = read_frame(&mut child).await;
            assert_eq!(envelope(&cancel).message_type, ControlType::Cancel);
            let mut late = envelope(&request);
            late.message_type = ControlType::Response;
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Control,
                    stream_id: 0,
                    sequence: 0,
                    payload: serde_json::to_vec(&late).unwrap(),
                },
            )
            .await;
        });
        assert_eq!(
            session
                .call("health", Value::Null, Duration::from_millis(5), None, None)
                .await
                .unwrap_err(),
            RpcError::DeadlineExceeded
        );
    }

    #[tokio::test]
    async fn response_with_wrong_transport_generation_closes_the_session() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        tokio::spawn(async move {
            let request = read_frame(&mut child).await;
            let mut response = envelope(&request);
            response.message_type = ControlType::Response;
            response.transport_generation = Some(8);
            response.payload = serde_json::json!({"ok":true});
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Control,
                    stream_id: 0,
                    sequence: 0,
                    payload: serde_json::to_vec(&response).unwrap(),
                },
            )
            .await;
        });
        assert_eq!(
            session
                .call(
                    "delta.list",
                    Value::Null,
                    Duration::from_secs(1),
                    Some("binding".into()),
                    Some(7),
                )
                .await,
            Err(RpcError::Protocol)
        );
    }
    #[test]
    fn stderr_is_bounded_and_redacted() {
        let log = redact_stderr_jsonl(br#"{"access_token":"secret","message":"ok"}"#, 128);
        assert!(!log.contains("secret"));
        assert!(log.len() <= 128);
    }
}
