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
use tokio::time::Instant;
use uuid::Uuid;

use super::frame::{encode, Decoder, Frame, FrameKind, FrameLimits, ProtocolError};
use super::stream::{decode_credit, decode_end, encode_credit, encode_end, Peer, Streams};

#[cfg(test)]
#[path = "decoder_rpc_tests.rs"]
mod decoder_rpc_tests;

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
    AdmissionDeadlineExceeded,
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
    completed_files: HashMap<u64, File>,
}

/// Only decoder sessions use this. One operation owns exactly one output.
struct DecoderOutputAdmission {
    directory: PathBuf,
    budget: Option<(Uuid, u64, Option<u64>)>,
}

struct IncomingObject {
    path: PathBuf,
    file: File,
    size: u64,
    written: u64,
    granted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BudgetClass {
    Control,
    Health,
}

#[derive(Default)]
struct Admission {
    fences: HashMap<String, u64>,
    active: Option<ActiveOperation>,
    health_due: bool,
    health_due_at: Option<Instant>,
}

struct ActiveOperation {
    id: Uuid,
    class: BudgetClass,
    binding_epoch: Option<String>,
    generation: Option<u64>,
    dispatched: bool,
    hold_for_stream: bool,
}

/// The owner survives the control reply when the operation includes a stream.
/// Its drop and response completion arbitrate under the same admission lock.
pub(super) struct OperationGuard {
    session: FramedSession,
    id: Uuid,
    deadline: Instant,
    outgoing: Option<u64>,
}

impl OperationGuard {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn complete(&mut self) -> Result<(), RpcError> {
        let mut admission = self
            .session
            .streams
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if admission
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id)
        {
            let active = admission
                .active
                .as_ref()
                .expect("operation identity checked");
            // A final response can be followed by clean EOF before its owner
            // resumes. Preserve that result, but never release fenced work.
            if active
                .binding_epoch
                .as_ref()
                .and_then(|epoch| admission.fences.get(epoch))
                .is_some_and(|minimum| {
                    active
                        .generation
                        .is_none_or(|generation| generation < *minimum)
                })
            {
                return Err(RpcError::Closed);
            }
            if admission
                .active
                .as_ref()
                .is_some_and(|active| active.class == BudgetClass::Health)
            {
                admission.health_due = false;
            }
            admission.active = None;
            if let Some(decoder) = self
                .session
                .streams
                .decoder_output
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
            {
                decoder.budget = None;
            }
            self.session.streams.changed.notify_waiters();
        }
        Ok(())
    }

    fn timeout_error(&self) -> RpcError {
        if self
            .session
            .streams
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id && active.dispatched)
        {
            RpcError::DeadlineExceeded
        } else {
            RpcError::AdmissionDeadlineExceeded
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let mut admission = self
            .session
            .streams
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if admission
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id)
        {
            let dispatched = admission
                .active
                .take()
                .is_some_and(|active| active.dispatched);
            if dispatched {
                // A synchronous child cannot consume Cancel until its old work
                // finishes. Fence the session and let its supervisor stop/join it.
                close_locked(
                    &self.session.streams,
                    &self.session.pending,
                    RpcError::Closed,
                );
            }
            self.session.streams.changed.notify_waiters();
        }
        if let Some(id) = self.outgoing {
            self.session
                .streams
                .outgoing
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .discard(id);
        }
    }
}

#[derive(Clone)]
pub(super) struct SessionActivity(FramedSession);

impl SessionActivity {
    pub(super) fn is_retired(&self) -> bool {
        self.0.streams.closed.load(Ordering::Acquire)
    }
}

fn close_locked(streams: &SharedStreams, pending: &Pending, error: RpcError) {
    streams.closed.store(true, Ordering::Release);
    streams.cancellation.cancel();
    for (_, call) in pending.lock().unwrap_or_else(|p| p.into_inner()).drain() {
        let _ = call.response.send(Err(error.clone()));
    }
    streams.changed.notify_waiters();
}

fn operation_timeout(admission: &Admission) -> RpcError {
    if admission
        .active
        .as_ref()
        .is_some_and(|active| active.dispatched)
    {
        RpcError::DeadlineExceeded
    } else {
        RpcError::AdmissionDeadlineExceeded
    }
}

struct SharedStreams {
    decoder_output: Mutex<Option<DecoderOutputAdmission>>,
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
    admission: Mutex<Admission>,
    cancellation: tokio_util::sync::CancellationToken,
    tasks: std::sync::atomic::AtomicUsize,
}

struct BlockingStreamWork(Arc<SharedStreams>);
impl BlockingStreamWork {
    fn new(streams: &Arc<SharedStreams>) -> Self {
        streams.tasks.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(streams))
    }
}
impl Drop for BlockingStreamWork {
    fn drop(&mut self) {
        self.0.tasks.fetch_sub(1, Ordering::AcqRel);
        self.0.changed.notify_waiters();
    }
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
    pub(super) fn decoder_job_dispatched_for_test(&self) -> bool {
        self.streams
            .admission
            .lock()
            .unwrap()
            .active
            .as_ref()
            .is_some_and(|active| active.dispatched && active.hold_for_stream)
    }
    /// Must be called before hello. Decoder output has no named temporary path;
    /// the OS releases its file on close, including process termination.
    pub(super) fn restrict_decoder_output(&self, directory: PathBuf) {
        *self
            .streams
            .decoder_output
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(DecoderOutputAdmission {
            directory,
            budget: None,
        });
    }

    pub(super) async fn call_with_input_file(
        &self,
        mut payload: Value,
        input: tokio::fs::File,
        size: u64,
        sha256: [u8; 32],
        maximum_output: u64,
        deadline: Instant,
    ) -> Result<(Value, OperationGuard), RpcError> {
        if !payload.is_object() {
            return Err(RpcError::Protocol);
        }
        let mut operation = self
            .acquire_operation(deadline, BudgetClass::Control, None, None, true)
            .await?;
        {
            let mut decoder = self
                .streams
                .decoder_output
                .lock()
                .map_err(|_| RpcError::Closed)?;
            let decoder = decoder.as_mut().ok_or(RpcError::Protocol)?;
            decoder.budget = Some((operation.id, maximum_output, None));
        }
        let transfer = async {
            let id = self.open_outgoing(&mut operation, size, sha256).await?;
            payload
                .as_object_mut()
                .ok_or(RpcError::Protocol)?
                .insert("streamId".into(), Value::from(id));
            let response = self
                .begin_call(&operation, "decoder.decode", payload, None, None)
                .await?;
            let (sent, result) = tokio::join!(
                self.send_outgoing_reader(id, input, size, sha256),
                self.wait_call(response, deadline),
            );
            sent?;
            let result = match result {
                Ok(result) => result,
                Err(error @ RpcError::Remote { .. }) => {
                    let accepted = self
                        .streams
                        .decoder_output
                        .lock()
                        .map_err(|_| RpcError::Closed)?
                        .as_ref()
                        .and_then(|decoder| decoder.budget)
                        .and_then(|(_, _, accepted)| accepted);
                    if accepted.is_some() {
                        return Err(RpcError::Protocol);
                    }
                    operation.complete()?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            Ok::<Value, RpcError>(result)
        };
        let result = tokio::time::timeout_at(deadline, transfer)
            .await
            .map_err(|_| RpcError::DeadlineExceeded)??;
        Ok((result, operation))
    }

    pub(super) async fn receive_decoder_file(
        &self,
        operation: &OperationGuard,
        stream_id: u64,
    ) -> Result<File, RpcError> {
        let wait = async {
            loop {
                let changed = self.streams.changed.notified();
                if self.streams.closed.load(Ordering::Acquire) {
                    return Err(RpcError::Closed);
                }
                {
                    let decoder = self
                        .streams
                        .decoder_output
                        .lock()
                        .map_err(|_| RpcError::Closed)?;
                    let (owner, _, accepted) = decoder
                        .as_ref()
                        .and_then(|value| value.budget)
                        .ok_or(RpcError::Protocol)?;
                    if owner != operation.id || accepted.is_some_and(|id| id != stream_id) {
                        return Err(RpcError::Protocol);
                    }
                }
                if let Some(file) = self
                    .streams
                    .incoming_data
                    .lock()
                    .map_err(|_| RpcError::Closed)?
                    .completed_files
                    .remove(&stream_id)
                {
                    return Ok(file);
                }
                changed.await;
            }
        };
        tokio::time::timeout_at(operation.deadline, wait)
            .await
            .map_err(|_| RpcError::DeadlineExceeded)?
    }

    async fn send_outgoing_reader<R: AsyncRead + Unpin>(
        &self,
        id: u64,
        mut reader: R,
        size: u64,
        sha256: [u8; 32],
    ) -> Result<(), RpcError> {
        let mut offset = 0u64;
        let mut sequence = 1u32;
        while offset < size {
            let changed = self.streams.changed.notified();
            if self.streams.closed.load(Ordering::Acquire) {
                return Err(RpcError::Closed);
            }
            let credit = self
                .streams
                .outgoing
                .lock()
                .map_err(|_| RpcError::Closed)?
                .credit(id)?;
            if credit <= offset {
                changed.await;
                continue;
            }
            let length = (size - offset)
                .min(credit - offset)
                .min(self.streams.max_chunk.min(256 * 1024) as u64)
                as usize;
            // No read-ahead without credit, and at most four queued frames.
            let mut payload = vec![0; length];
            reader
                .read_exact(&mut payload)
                .await
                .map_err(|_| RpcError::Protocol)?;
            let frame = Frame {
                kind: FrameKind::Binary,
                stream_id: id,
                sequence,
                payload,
            };
            self.streams
                .outgoing
                .lock()
                .map_err(|_| RpcError::Closed)?
                .accept_data(&frame)?;
            self.tx.send(frame).await.map_err(|_| RpcError::Closed)?;
            offset += length as u64;
            sequence = sequence.checked_add(1).ok_or(RpcError::Protocol)?;
        }
        let mut extra = [0u8; 1];
        if reader
            .read(&mut extra)
            .await
            .map_err(|_| RpcError::Closed)?
            != 0
        {
            return Err(RpcError::Protocol);
        }
        let frame = Frame {
            kind: FrameKind::End,
            stream_id: id,
            sequence,
            payload: encode_end(size, sha256),
        };
        {
            let mut streams = self.streams.outgoing.lock().map_err(|_| RpcError::Closed)?;
            streams.finish(&frame, size, sha256)?;
            streams.retire(id)?;
        }
        self.tx.send(frame).await.map_err(|_| RpcError::Closed)
    }

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
        // Bound queued bytes by a small number of negotiated frames, including
        // credit/control frames. Decoder frames are additionally capped at 256 KiB.
        let (tx, mut rx) = mpsc::channel::<Frame>(4);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let streams = Arc::new(SharedStreams {
            decoder_output: Mutex::new(None),
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
            admission: Mutex::new(Admission::default()),
            cancellation: tokio_util::sync::CancellationToken::new(),
            tasks: std::sync::atomic::AtomicUsize::new(2),
        });
        let writer_streams = streams.clone();
        let writer_pending = pending.clone();
        tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    biased;
                    _ = writer_streams.cancellation.cancelled() => break,
                    frame = rx.recv() => match frame { Some(frame) => frame, None => break },
                };
                let Ok(bytes) = encode(&frame) else { break };
                let write = async {
                    writer.write_all(&bytes).await?;
                    writer.flush().await
                };
                tokio::select! {
                    biased;
                    _ = writer_streams.cancellation.cancelled() => break,
                    result = write => if result.is_err() { break; },
                };
            }
            {
                let _admission = writer_streams
                    .admission
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                close_locked(&writer_streams, &writer_pending, RpcError::Closed);
            }
            writer_streams.tasks.fetch_sub(1, Ordering::AcqRel);
            writer_streams.changed.notify_waiters();
        });
        let reader_pending = Arc::clone(&pending);
        let reader_streams = Arc::clone(&streams);
        let reader_tx = tx.clone();
        tokio::spawn(async move {
            let mut decoder = Decoder::new(limits);
            let mut bytes = [0_u8; 16 * 1024];
            let mut protocol_ok = true;
            'read: loop {
                let read = tokio::select! {
                    biased;
                    _ = reader_streams.cancellation.cancelled() => break,
                    read = reader.read(&mut bytes) => read,
                };
                let count = match read {
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
                    let handle = async {
                        match frame.kind {
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
                            FrameKind::End => {
                                let streams = Arc::clone(&reader_streams);
                                let frame = frame.clone();
                                let ownership = BlockingStreamWork::new(&streams);
                                tokio::task::spawn_blocking(move || {
                                    let _ownership = ownership;
                                    handle_end(&frame, &streams)
                                })
                                .await
                                .map_err(|_| RpcError::Closed)?
                            }
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
                        }
                    };
                    let result = tokio::select! {
                        biased;
                        _ = reader_streams.cancellation.cancelled() => break 'read,
                        result = handle => result,
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
            {
                let _admission = reader_streams
                    .admission
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                close_locked(&reader_streams, &reader_pending, failure.clone());
            }
            if let Ok(mut data) = reader_streams.incoming_data.lock() {
                data.completed_files.clear();
                for id in data.open.keys().copied().collect::<Vec<_>>() {
                    data.completed.insert(id, Err(failure.clone()));
                }
                for (_, completed) in data.completed.drain() {
                    if let Ok(path) = completed {
                        let _ = std::fs::remove_file(path);
                    }
                }
                for (_, object) in data.open.drain() {
                    drop(object.file);
                    let _ = std::fs::remove_file(object.path);
                }
            }
            reader_streams.changed.notify_waiters();
            reader_streams.tasks.fetch_sub(1, Ordering::AcqRel);
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
        let class = if method == "health" {
            BudgetClass::Health
        } else {
            BudgetClass::Control
        };
        self.call_with_budget(
            method,
            payload,
            deadline,
            class,
            binding_epoch,
            transport_generation,
        )
        .await
    }

    pub(super) fn activity(&self) -> SessionActivity {
        SessionActivity(self.clone())
    }

    pub(super) async fn call_with_budget(
        &self,
        method: &str,
        payload: Value,
        budget: Duration,
        class: BudgetClass,
        binding_epoch: Option<String>,
        transport_generation: Option<u64>,
    ) -> Result<Value, RpcError> {
        let (value, _operation) = self
            .call_owned(
                method,
                payload,
                budget,
                class,
                binding_epoch,
                transport_generation,
                false,
            )
            .await?;
        Ok(value)
    }

    pub(super) async fn call_owned(
        &self,
        method: &str,
        payload: Value,
        budget: Duration,
        class: BudgetClass,
        binding_epoch: Option<String>,
        transport_generation: Option<u64>,
        hold_for_stream: bool,
    ) -> Result<(Value, OperationGuard), RpcError> {
        let mut operation = self
            .acquire_operation(
                Instant::now() + budget,
                class,
                binding_epoch.clone(),
                transport_generation,
                hold_for_stream,
            )
            .await?;
        let response = self
            .begin_call(
                &operation,
                method,
                payload,
                binding_epoch,
                transport_generation,
            )
            .await?;
        let value = match self.wait_call(response, operation.deadline).await {
            Ok(value) => value,
            Err(error @ RpcError::Remote { .. }) => {
                // A rejected download has no returned stream to retain.
                let _ = operation.complete();
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        Ok((value, operation))
    }

    async fn acquire_operation(
        &self,
        deadline: Instant,
        class: BudgetClass,
        binding_epoch: Option<String>,
        generation: Option<u64>,
        hold_for_stream: bool,
    ) -> Result<OperationGuard, RpcError> {
        loop {
            let changed = self.streams.changed.notified();
            {
                let mut admission = self
                    .streams
                    .admission
                    .lock()
                    .map_err(|_| RpcError::Closed)?;
                self.check_admission(&admission, binding_epoch.as_deref(), generation)?;
                if Instant::now() >= deadline {
                    return Err(RpcError::AdmissionDeadlineExceeded);
                }
                if admission
                    .health_due_at
                    .is_some_and(|due| Instant::now() >= due)
                {
                    admission.health_due = true;
                }
                if admission.active.is_none()
                    && (!admission.health_due || class == BudgetClass::Health)
                {
                    let id = Uuid::now_v7();
                    admission.active = Some(ActiveOperation {
                        id,
                        class,
                        binding_epoch,
                        generation,
                        dispatched: false,
                        hold_for_stream,
                    });
                    return Ok(OperationGuard {
                        session: self.clone(),
                        id,
                        deadline,
                        outgoing: None,
                    });
                }
            }
            tokio::select! {
                biased;
                _ = self.streams.cancellation.cancelled() => return Err(RpcError::Closed),
                _ = changed => {},
                _ = tokio::time::sleep_until(deadline) => return Err(RpcError::AdmissionDeadlineExceeded),
            }
        }
    }

    async fn begin_call(
        &self,
        operation: &OperationGuard,
        method: &str,
        payload: Value,
        binding_epoch: Option<String>,
        transport_generation: Option<u64>,
    ) -> Result<oneshot::Receiver<Result<Value, RpcError>>, RpcError> {
        let (response_tx, response_rx) = oneshot::channel();
        let permit = tokio::time::timeout_at(operation.deadline, self.tx.reserve())
            .await
            .map_err(|_| operation.timeout_error())?
            .map_err(|_| RpcError::Closed)?;
        let mut admission = self
            .streams
            .admission
            .lock()
            .map_err(|_| RpcError::Closed)?;
        self.check_admission(&admission, binding_epoch.as_deref(), transport_generation)?;
        let remaining = operation.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(operation_timeout(&admission));
        }
        let envelope = ControlEnvelope {
            wire_version: self.wire_version,
            message_type: ControlType::Request,
            request_id: operation.id,
            method: method.to_owned(),
            deadline_ms: remaining.as_millis().min(u64::MAX as u128) as u64,
            binding_epoch: binding_epoch.clone(),
            transport_generation,
            payload,
        };
        // Serialize before registration; construction failure cannot leak pending ownership.
        let payload = serde_json::to_vec(&envelope).map_err(|_| RpcError::Protocol)?;
        let mut pending = self.pending.lock().map_err(|_| RpcError::Closed)?;
        if pending.len() >= self.streams.max_requests {
            return Err(RpcError::Protocol);
        }
        let active = admission
            .active
            .as_mut()
            .filter(|active| active.id == operation.id)
            .ok_or(RpcError::Closed)?;
        pending.insert(
            operation.id,
            PendingCall {
                response: response_tx,
                method: method.to_owned(),
                binding_epoch,
                transport_generation,
            },
        );
        active.dispatched = true;
        permit.send(Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: 0,
            payload,
        });
        Ok(response_rx)
    }

    fn check_admission(
        &self,
        admission: &Admission,
        epoch: Option<&str>,
        generation: Option<u64>,
    ) -> Result<(), RpcError> {
        if self.streams.closed.load(Ordering::Acquire)
            || epoch
                .and_then(|epoch| admission.fences.get(epoch))
                .is_some_and(|minimum| generation.is_none_or(|generation| generation < *minimum))
        {
            return Err(RpcError::Closed);
        }
        Ok(())
    }

    pub fn close_admission(&self) {
        let _admission = self
            .streams
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        close_locked(&self.streams, &self.pending, RpcError::Closed);
    }

    pub async fn join_closed(&self) {
        loop {
            let changed = self.streams.changed.notified();
            if self.streams.tasks.load(Ordering::Acquire) == 0 {
                break;
            }
            changed.await;
        }
    }

    async fn wait_call(
        &self,
        response: oneshot::Receiver<Result<Value, RpcError>>,
        deadline: Instant,
    ) -> Result<Value, RpcError> {
        tokio::time::timeout_at(deadline, response)
            .await
            .map_err(|_| RpcError::DeadlineExceeded)?
            .map_err(|_| RpcError::Closed)?
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
        let mut operation = self
            .acquire_operation(
                Instant::now() + deadline,
                BudgetClass::Control,
                binding_epoch.clone(),
                transport_generation,
                true,
            )
            .await?;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let expires_at = operation.deadline;
        let transfer = async {
            let id = self
                .open_outgoing(&mut operation, bytes.len() as u64, digest)
                .await?;
            payload
                .as_object_mut()
                .expect("payload shape checked")
                .insert("streamId".into(), Value::from(id));
            let response = self
                .begin_call(
                    &operation,
                    method,
                    payload,
                    binding_epoch,
                    transport_generation,
                )
                .await?;
            let (_, result) = tokio::try_join!(
                self.send_outgoing(id, bytes, digest),
                self.wait_call(response, expires_at),
            )?;
            operation.complete()?;
            Ok(result)
        };
        match tokio::time::timeout_at(expires_at, transfer).await {
            Ok(result) => result,
            Err(_) => Err(operation.timeout_error()),
        }
    }

    #[cfg(test)]
    pub async fn receive_file(
        &self,
        stream_id: u64,
        deadline: Duration,
    ) -> Result<PathBuf, RpcError> {
        self.receive_file_until(stream_id, Instant::now() + deadline)
            .await
    }

    async fn receive_file_until(
        &self,
        stream_id: u64,
        deadline: Instant,
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
        tokio::time::timeout_at(deadline, wait)
            .await
            .map_err(|_| RpcError::DeadlineExceeded)?
    }

    pub async fn receive_file_for_context(
        &self,
        stream_id: u64,
        deadline: Instant,
        binding_epoch: &str,
        generation: u64,
    ) -> Result<PathBuf, RpcError> {
        let receive = self.receive_file_until(stream_id, deadline);
        tokio::pin!(receive);
        loop {
            let changed = self.streams.changed.notified();
            self.check_admission(
                &*self
                    .streams
                    .admission
                    .lock()
                    .map_err(|_| RpcError::Closed)?,
                Some(binding_epoch),
                Some(generation),
            )?;
            tokio::select! {
                result = &mut receive => {
                    let path = result?;
                    let check = self.check_admission(&*self.streams.admission.lock().map_err(|_| RpcError::Closed)?, Some(binding_epoch), Some(generation));
                    if let Err(error) = check { let _ = std::fs::remove_file(path); return Err(error); }
                    return Ok(path);
                },
                _ = changed => {},
            }
        }
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
        {
            let mut admission = self
                .streams
                .admission
                .lock()
                .map_err(|_| RpcError::Closed)?;
            let minimum = admission
                .fences
                .entry(binding_epoch.to_owned())
                .or_default();
            *minimum = (*minimum).max(before_generation);
        }
        self.streams.changed.notify_waiters();
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

    async fn open_outgoing(
        &self,
        operation: &mut OperationGuard,
        size: u64,
        sha256: [u8; 32],
    ) -> Result<u64, RpcError> {
        let id = self
            .streams
            .outgoing
            .lock()
            .map_err(|_| RpcError::Closed)?
            .allocate(size, sha256)?;
        operation.outgoing = Some(id);
        let payload = serde_json::to_value(StreamOpen {
            stream_id: id,
            size,
            sha256: hex(&sha256),
        })
        .map_err(|_| RpcError::Protocol)?;
        let envelope = ControlEnvelope {
            wire_version: self.wire_version,
            message_type: ControlType::Request,
            request_id: Uuid::now_v7(),
            method: "stream.open".into(),
            deadline_ms: 0,
            binding_epoch: None,
            transport_generation: None,
            payload,
        };
        let payload = serde_json::to_vec(&envelope).map_err(|_| RpcError::Protocol)?;
        let permit = tokio::time::timeout_at(operation.deadline, self.tx.reserve())
            .await
            .map_err(|_| operation.timeout_error())?
            .map_err(|_| RpcError::Closed)?;
        let mut admission = self
            .streams
            .admission
            .lock()
            .map_err(|_| RpcError::Closed)?;
        let active = admission
            .active
            .as_ref()
            .filter(|active| active.id == operation.id)
            .ok_or(RpcError::Closed)?;
        self.check_admission(
            &admission,
            active.binding_epoch.as_deref(),
            active.generation,
        )?;
        if Instant::now() >= operation.deadline {
            return Err(operation_timeout(&admission));
        }
        admission
            .active
            .as_mut()
            .expect("operation identity checked")
            .dispatched = true;
        permit.send(Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: 0,
            payload,
        });
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
        let decoder_directory = {
            let mut decoder = streams
                .decoder_output
                .lock()
                .map_err(|_| RpcError::Closed)?;
            if let Some(decoder) = decoder.as_mut() {
                let (_, maximum, accepted) = decoder.budget.as_mut().ok_or(RpcError::Protocol)?;
                if accepted.is_some() || open.size == 0 || open.size > *maximum {
                    return Err(RpcError::Protocol);
                }
                *accepted = Some(open.stream_id);
                Some(decoder.directory.clone())
            } else {
                None
            }
        };
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
        let path = if decoder_directory.is_some() {
            PathBuf::new()
        } else {
            std::env::temp_dir().join(format!(
                "mangoo-plugin-rpc-{}-{}.stream",
                std::process::id(),
                Uuid::now_v7().simple()
            ))
        };
        let file = match if let Some(directory) = decoder_directory {
            tempfile::tempfile_in(directory)
        } else {
            OpenOptions::new().create_new(true).write(true).open(&path)
        } {
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
        // Response completion and dropped ownership use the same lock order.
        // A completed control call must not retire its reusable session on drop.
        let mut admission = streams.admission.lock().map_err(|_| RpcError::Closed)?;
        let mut pending = pending.lock().map_err(|_| RpcError::Closed)?;
        if let Some(call) = pending.get(&envelope.request_id) {
            if call.binding_epoch != envelope.binding_epoch
                || call.transport_generation != envelope.transport_generation
            {
                return Err(RpcError::Protocol);
            }
        }
        let call = pending.remove(&envelope.request_id);
        if call.is_some()
            && admission
                .active
                .as_ref()
                .is_some_and(|active| active.id == envelope.request_id && !active.hold_for_stream)
        {
            if admission
                .active
                .as_ref()
                .is_some_and(|active| active.class == BudgetClass::Health)
            {
                admission.health_due = false;
                admission.health_due_at = None;
            }
            admission.active = None;
            streams.changed.notify_waiters();
        }
        call
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
    streams: &Arc<SharedStreams>,
    tx: &mpsc::Sender<Frame>,
) -> Result<(), RpcError> {
    streams
        .incoming
        .lock()
        .map_err(|_| RpcError::Closed)?
        .accept_data(frame)?;
    let owned_streams = Arc::clone(streams);
    let owned_frame = frame.clone();
    let ownership = BlockingStreamWork::new(streams);
    let next_credit = tokio::task::spawn_blocking(move || {
        let _ownership = ownership;
        let streams = owned_streams;
        let frame = &owned_frame;
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
            Ok(Some(next_credit))
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(|_| RpcError::Closed)??;
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
    if object.path.as_os_str().is_empty() {
        data.completed_files.insert(frame.stream_id, object.file);
    } else {
        drop(object.file);
        data.completed.insert(frame.stream_id, Ok(object.path));
    }
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
    async fn reader_eof_retires_and_joins_idle_writer() {
        let (host, child) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(host);
        let session = FramedSession::start(reader, writer, FrameLimits::default(), 1);
        drop(child);
        tokio::time::timeout(Duration::from_millis(200), session.join_closed())
            .await
            .expect("reader EOF must cancel and join the idle writer");
        assert!(session.activity().is_retired());
        assert!(session.streams.cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn writer_failure_retires_session_and_wakes_pending_call() {
        let (reader, _live_peer) = tokio::io::duplex(4096);
        let (writer, closed_peer) = tokio::io::duplex(4096);
        drop(closed_peer);
        let session = FramedSession::start(reader, writer, FrameLimits::default(), 1);
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            session.call("health", Value::Null, Duration::from_secs(2), None, None),
        )
        .await
        .expect("writer failure must wake the call before its deadline");
        assert_eq!(result, Err(RpcError::Closed));
        tokio::time::timeout(Duration::from_millis(200), session.join_closed())
            .await
            .expect("writer failure must cancel and join the reader");
        assert!(session.activity().is_retired());
        assert!(session.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stream_completion_cannot_release_fenced_ownership() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(host);
        let session = FramedSession::start(reader, writer, FrameLimits::default(), 1);
        let call_session = session.clone();
        let call = tokio::spawn(async move {
            call_session
                .call_owned(
                    "object.get",
                    Value::Null,
                    Duration::from_secs(1),
                    BudgetClass::Control,
                    Some("binding".into()),
                    Some(1),
                    true,
                )
                .await
        });
        let mut response = envelope(&read_frame(&mut child).await);
        response.message_type = ControlType::Response;
        response.payload = serde_json::json!({"streamId":2});
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
        let (_, mut operation) = call.await.unwrap().unwrap();
        session.cancel_binding("binding", 2).await.unwrap();
        assert_eq!(operation.complete(), Err(RpcError::Closed));
        drop(operation);
        assert!(session.activity().is_retired());
        session.join_closed().await;
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
    async fn dispatched_deadline_retires_session_and_rejects_late_response() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        let child_task = tokio::spawn(async move {
            let request = read_frame(&mut child).await;
            let mut next = [0u8; 1];
            assert_eq!(child.read(&mut next).await.unwrap(), 0);
            let mut late = envelope(&request);
            late.message_type = ControlType::Response;
            let frame = Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: 0,
                payload: serde_json::to_vec(&late).unwrap(),
            };
            assert!(child.write_all(&encode(&frame).unwrap()).await.is_err());
        });
        assert_eq!(
            session
                .call("health", Value::Null, Duration::from_millis(5), None, None)
                .await
                .unwrap_err(),
            RpcError::DeadlineExceeded
        );
        tokio::time::timeout(Duration::from_secs(1), session.join_closed())
            .await
            .unwrap();
        child_task.await.unwrap();
        assert!(session.pending.lock().unwrap().is_empty());
        assert_eq!(
            session
                .call("health", Value::Null, Duration::from_secs(1), None, None)
                .await,
            Err(RpcError::Closed)
        );
    }

    #[tokio::test]
    async fn queued_deadline_does_not_dispatch_or_retire_active_call() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        let active_session = session.clone();
        let active = tokio::spawn(async move {
            active_session
                .call("describe", Value::Null, Duration::from_secs(1), None, None)
                .await
        });
        let request = read_frame(&mut child).await;
        assert_eq!(
            session
                .call("health", Value::Null, Duration::from_millis(10), None, None)
                .await,
            Err(RpcError::AdmissionDeadlineExceeded)
        );
        assert!(!session.activity().is_retired());
        assert_eq!(session.pending.lock().unwrap().len(), 1);
        let mut response = envelope(&request);
        response.message_type = ControlType::Response;
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
        active.await.unwrap().unwrap();
        assert!(session.streams.admission.lock().unwrap().active.is_none());
        let next_session = session.clone();
        let next = tokio::spawn(async move {
            next_session
                .call("health", Value::Null, Duration::from_secs(1), None, None)
                .await
        });
        let request = read_frame(&mut child).await;
        let mut response = envelope(&request);
        assert_eq!(response.method, "health");
        response.message_type = ControlType::Response;
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
        next.await.unwrap().unwrap();
        assert!(!session.activity().is_retired());
        session.close_admission();
        session.join_closed().await;
    }

    #[tokio::test]
    async fn upload_deadline_covers_credit_wait_after_early_control_response() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        let child_task = tokio::spawn(async move {
            assert_eq!(
                envelope(&read_frame(&mut child).await).method,
                "stream.open"
            );
            let mut response = envelope(&read_frame(&mut child).await);
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
            let mut next = [0u8; 1];
            assert_eq!(child.read(&mut next).await.unwrap(), 0);
        });
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                session.call_with_bytes(
                    "put",
                    serde_json::json!({}),
                    b"data",
                    Duration::from_millis(20),
                    None,
                    None,
                )
            )
            .await
            .unwrap(),
            Err(RpcError::DeadlineExceeded)
        );
        session.join_closed().await;
        child_task.await.unwrap();
        assert!(session.pending.lock().unwrap().is_empty());
        assert!(session.streams.admission.lock().unwrap().active.is_none());
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

    #[tokio::test]
    async fn plugin_lifecycle_binding_fence_rejects_calls_registered_after_cancel() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        session.cancel_binding("binding", 2).await.unwrap();
        assert_eq!(
            session
                .call(
                    "object.head",
                    Value::Null,
                    Duration::from_secs(1),
                    Some("binding".into()),
                    Some(1)
                )
                .await,
            Err(RpcError::Closed)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), read_frame(&mut child))
                .await
                .is_err()
        );
        let responder = tokio::spawn(async move {
            let request = read_frame(&mut child).await;
            let mut response = envelope(&request);
            response.message_type = ControlType::Response;
            response.payload = Value::Null;
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
            .call(
                "object.head",
                Value::Null,
                Duration::from_secs(1),
                Some("binding".into()),
                Some(2)
            )
            .await
            .is_ok());
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn plugin_lifecycle_fences_stream_after_control_response_completed() {
        let (host, mut child) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(host);
        let session = FramedSession::start(r, w, FrameLimits::default(), 1);
        let (stream_started, stream_seen) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let task = tokio::spawn(async move {
            let request = read_frame(&mut child).await;
            let mut response = envelope(&request);
            response.message_type = ControlType::Response;
            response.payload = serde_json::json!({"streamId":2});
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
                    sha256: hex(&Sha256::digest(b"data")),
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
            assert_eq!(read_frame(&mut child).await.kind, FrameKind::Credit);
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Binary,
                    stream_id: 2,
                    sequence: 1,
                    payload: b"da".to_vec(),
                },
            )
            .await;
            stream_started.send(()).unwrap();
            let _ = released.await;
        });
        session
            .call(
                "object.get",
                Value::Null,
                Duration::from_secs(1),
                Some("binding".into()),
                Some(1),
            )
            .await
            .unwrap();
        stream_seen.await.unwrap();
        let path = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(object) = session
                    .streams
                    .incoming_data
                    .lock()
                    .unwrap()
                    .open
                    .get(&2)
                    .filter(|object| object.written == 2)
                {
                    break object.path.clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(path.exists());
        assert!(session.pending.lock().unwrap().is_empty());
        session.cancel_binding("binding", 2).await.unwrap();
        assert_eq!(
            session
                .receive_file_for_context(2, Instant::now() + Duration::from_secs(1), "binding", 1)
                .await,
            Err(RpcError::Closed)
        );
        session.close_admission();
        tokio::time::timeout(Duration::from_secs(1), session.join_closed())
            .await
            .unwrap();
        assert!(
            !path.exists(),
            "joined session must remove its partial stream file"
        );
        assert!(session
            .streams
            .incoming_data
            .lock()
            .unwrap()
            .open
            .is_empty());
        assert_eq!(
            session
                .call("health", Value::Null, Duration::from_secs(1), None, None)
                .await,
            Err(RpcError::Closed)
        );
        release.send(()).unwrap();
        task.await.unwrap();
    }
}
