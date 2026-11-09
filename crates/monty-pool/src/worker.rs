//! A single worker the pool drives over the wire protocol: either a local
//! `monty subprocess` child (framed stdio) or a remote child reached over a
//! gRPC stream (see [`crate::remote`]). Both expose the same async
//! send/recv/kill surface so the checkout turn loop is transport-agnostic.
//!
//! `recv` must be cancel-safe: the checkout races each turn against a
//! `tokio::time` deadline, and dropping a plain `read_exact` future mid-frame
//! would lose bytes and desync the stream. For the subprocess transport,
//! rather than paying for a pump task and channel per worker (a cross-task
//! wakeup per event), cancel-safety comes from keeping the partial-frame
//! state *in the worker*: [`FrameRecv`] holds the buffer and fill offset
//! across polls, so a dropped `recv` future loses nothing and the next call
//! resumes exactly where it stopped.
//!
//! The remote transport runs a background reader task instead — not for
//! cancel-safety but for liveness: an idle client polls nothing, so a server's
//! `Ping` would go unanswered and a keepalive server would drop the session
//! as vanished. `recv` reads the task's channel, which is cancel-safe.

use std::{
    env, io,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use monty_proto::{
    DEFAULT_MAX_DECODE_BYTES, FrameError, MAX_FRAME_LEN, decode_frame_with_budget, encode_framed_into, pb,
};
use monty_types::ExecutionClock;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::{Handle, RuntimeFlavor},
    task::block_in_place,
    time::timeout,
};

#[cfg(feature = "telemetry")]
use crate::telemetry::{TelemetryContext, metrics::TurnMetrics, tracing::Recorder};
use crate::{
    MontyTransport, PoolConfig, PoolError, RemoteDisconnect,
    cpu_clock::Clock,
    remote::{Remote, RemoteWorker},
};

/// A worker plus its recycle counter. The transport-specific I/O halves live
/// in [`WorkerKind`]; both transports keep any partial-read state inside the
/// worker so `recv` is cancel-safe (see the module docs).
pub(crate) struct Worker {
    kind: WorkerKind,
    /// Host-selected allocation budget for decoding each protocol frame.
    max_decode_bytes: usize,
    /// Wire length of the last event `recv` returned, which the streamed-call budget charges.
    last_frame_len: usize,
    /// Pool-assigned generation, independent of PID reuse.
    pub(crate) id: u64,
    /// Checkouts this worker has served, for `max_checkouts_per_worker`.
    pub(crate) checkouts_served: u32,
    /// Records an adapter checkout's protocol turns, created only while needed.
    #[cfg(feature = "telemetry")]
    recorder: Option<Box<Recorder>>,
    /// Records aggregate turn metrics, for every checkout this worker serves
    /// rather than only the traced ones.
    #[cfg(feature = "telemetry")]
    metrics: Option<Box<TurnMetrics>>,
}

/// The two transports a worker can speak the protocol over. Both variants are
/// boxed: the stream's channel halves and the subprocess handles + frame-read
/// state are each far larger than a pointer, and workers move through the
/// idle queue by value.
enum WorkerKind {
    Subprocess(Box<SubprocessWorker>),
    Remote(Box<RemoteWorker>),
}

/// A local `monty subprocess` child: the process handle (kill-on-drop), its
/// framed stdin, and the cancel-safe stdout frame reader.
struct SubprocessWorker {
    child: Child,
    /// The child's CPU clock, recorded at spawn in CPU mode; `None` in wall mode.
    cpu_clock: Option<Clock>,
    stdin: ChildStdin,
    recv: FrameRecv,
    /// Reused encode buffer for outgoing frames (see [`Worker::send`]).
    send_buf: Vec<u8>,
}

impl Worker {
    /// Creates a worker for `config`'s transport: a spawned child, or a stream
    /// opened on `remote` (the pool's channel, present exactly when the
    /// transport is remote). `metadata` becomes the stream's request
    /// metadata; a subprocess makes no request and ignores them.
    pub(crate) async fn new(
        config: &PoolConfig,
        remote: Option<&Remote>,
        metadata: &[(String, String)],
    ) -> Result<Self, PoolError> {
        let worker = match &config.transport {
            MontyTransport::Subprocess(binary_path) => Self::subprocess(binary_path, config.execution_clock),
            MontyTransport::Remote(_) => {
                let remote = remote.expect("a remote pool holds its channel");
                remote
                    .open(metadata, config.max_decode_bytes)
                    .await
                    .map(|worker| Self::from_kind(WorkerKind::Remote(Box::new(worker))))
            }
        };
        let worker = worker.map(|mut worker| {
            worker.max_decode_bytes = config.max_decode_bytes;
            worker
        });
        #[cfg(feature = "telemetry")]
        let worker = worker.map(|mut worker| {
            worker.metrics = config
                .metrics
                .clone()
                .map(|metrics| Box::new(TurnMetrics::new(metrics)));
            worker
        });
        worker
    }

    /// Spawns a local `monty subprocess` child with framed pipes.
    ///
    /// There is no spawn-time handshake: a wrong or broken binary surfaces as
    /// an error on the first request the worker serves (typically the
    /// `Configure` of its first checkout).
    ///
    /// CPU mode records the child's CPU clock here, while the child is certainly
    /// unreaped, so later reads can detect a reused PID (see [`Clock`]).
    fn subprocess(binary_path: &PathBuf, execution_clock: ExecutionClock) -> Result<Self, PoolError> {
        let mut command = Command::new(binary_path);
        command
            .arg("subprocess")
            // For extra safety, spawn the worker with an empty environment.
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // the pool must never leak a live sandbox: an abandoned handle
            // kills the child even when no explicit teardown ran
            .kill_on_drop(true);
        // Windows processes misbehave without SystemRoot (CRT and WinAPI
        // lookups); it names the OS install directory and is not sensitive.
        if cfg!(windows)
            && let Ok(system_root) = env::var("SystemRoot")
        {
            command.env("SystemRoot", system_root);
        }
        let mut child = command
            // stderr is inherited: child diagnostics stay visible to the host
            .spawn()
            .map_err(|err| PoolError::Spawn(format!("{}: {err}", binary_path.display())))?;

        // a failure drops `child`, which `kill_on_drop` kills
        let cpu_clock = match execution_clock {
            ExecutionClock::Cpu => {
                Some(Clock::new(&child).map_err(|err| PoolError::Spawn(format!("recording child CPU clock: {err}")))?)
            }
            ExecutionClock::Wall => None,
        };
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self::from_kind(WorkerKind::Subprocess(Box::new(SubprocessWorker {
            child,
            cpu_clock,
            stdin,
            recv: FrameRecv::new(stdout),
            send_buf: Vec::with_capacity(SEND_BUF_CAPACITY),
        }))))
    }

    /// Wraps a transport in a fresh, unnumbered worker; `new` then applies the
    /// pool's decode budget and metrics.
    fn from_kind(kind: WorkerKind) -> Self {
        Self {
            kind,
            max_decode_bytes: DEFAULT_MAX_DECODE_BYTES,
            last_frame_len: 0,
            id: 0,
            checkouts_served: 0,
            #[cfg(feature = "telemetry")]
            recorder: None,
            #[cfg(feature = "telemetry")]
            metrics: None,
        }
    }

    #[cfg(feature = "telemetry")]
    pub(crate) fn callback_context(&self) -> opentelemetry::Context {
        self.recorder
            .as_ref()
            .map_or_else(opentelemetry::Context::new, |recorder| recorder.callback_context())
    }

    /// The span a streamed call's host callback runs under; see `Recorder::streamed_call_context`.
    #[cfg(feature = "telemetry")]
    pub(crate) fn streamed_call_context(&self, call_id: u32) -> opentelemetry::Context {
        self.recorder
            .as_ref()
            .map_or_else(opentelemetry::Context::new, |recorder| {
                recorder.streamed_call_context(call_id)
            })
    }

    /// Sends one request, flushed to the wire — a reply is only ever read
    /// after a request, so an unflushed frame would deadlock both sides. An
    /// oversize frame is rejected *before* any I/O so the stream stays synced
    /// (see `Checkout::request_turn`).
    pub(crate) async fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        let result = match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                // prefix + body in one reused buffer: a single write syscall
                // per request, no per-frame allocation, and a pipe write needs
                // no flush
                encode_framed_into(request, &mut w.send_buf)?;
                w.stdin.write_all(&w.send_buf).await?;
                // body only, matching the remote transport and both receive
                // paths, which never see the 4-byte length prefix
                let len = w.send_buf.len() - 4;
                // don't let one huge frame pin its capacity for the worker's life
                if w.send_buf.capacity() > RETAIN_BUF_MAX {
                    w.send_buf = Vec::with_capacity(SEND_BUF_CAPACITY);
                }
                Ok(len)
            }
            WorkerKind::Remote(w) => w.send(request).await,
        };
        // record only requests that hit the wire: a rejected oversize frame
        // leaves the turn (and any pending suspension) exactly where it was
        #[cfg(feature = "telemetry")]
        if let Ok(len) = result {
            if let Some(recorder) = &mut self.recorder {
                recorder.begin_turn(request);
            }
            if let Some(metrics) = &mut self.metrics {
                metrics.frame("sent", len);
                metrics.begin_turn(request);
            }
            if matches!(
                request.kind,
                Some(pb::parent_request::Kind::Reset(_) | pb::parent_request::Kind::Shutdown(_))
            ) {
                self.recorder = None;
            }
        }
        result.map(drop)
    }

    /// Copies the first recorded stream failure before teardown can add a
    /// local one; `None` for a subprocess.
    pub(crate) fn remote_disconnect(&self) -> Option<Box<RemoteDisconnect>> {
        match &self.kind {
            WorkerKind::Remote(worker) => worker.disconnect(),
            WorkerKind::Subprocess(_) => None,
        }
    }

    /// Attaches the host's trace context, if it captured one, before this
    /// worker starts a checkout.
    #[cfg(feature = "telemetry")]
    pub(crate) fn with_adapter_context(mut self, context: Option<TelemetryContext>) -> Self {
        if let Some(context) = context {
            let worker_pid = self.pid();
            self.recorder
                .get_or_insert_with(|| Box::new(Recorder::new(worker_pid)))
                .set_adapter_context(context);
        }
        self
    }

    /// Marks this worker's session span as a session resumed after its relay shut down.
    #[cfg(feature = "telemetry")]
    pub(crate) fn mark_resumed(&mut self) {
        if let Some(recorder) = &mut self.recorder {
            recorder.mark_resumed();
        }
    }

    /// Receives one event. EOF/close is an error here because within a
    /// checkout the child must never close its side first.
    ///
    /// Cancel-safe: partial-frame state persists in the worker (see the
    /// module docs), which is what lets `Checkout` race a turn against its
    /// deadline.
    pub(crate) async fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        let (event, len) = match &mut self.kind {
            WorkerKind::Subprocess(w) => match w.recv.recv(self.max_decode_bytes).await? {
                Some(framed) => Ok(framed),
                None => Err(FrameError::Truncated), // clean EOF mid-checkout is still a vanished peer
            },
            // events come from the reader task; a channel recv is
            // cancel-safe, preserving the deadline race
            WorkerKind::Remote(w) => w.recv().await,
        }?;
        // only after a complete decode, so a `recv` future dropped mid-frame
        // (deadline race) records nothing — cancel-safety intact
        #[cfg(feature = "telemetry")]
        if let Some(recorder) = &mut self.recorder {
            recorder.event(&event);
        }
        #[cfg(feature = "telemetry")]
        if let Some(metrics) = &mut self.metrics {
            metrics.frame("received", len);
            metrics.event(&event);
        }
        self.last_frame_len = len;
        Ok(event)
    }

    /// Wire length of the event the last `recv` returned.
    pub(crate) const fn last_frame_len(&self) -> usize {
        self.last_frame_len
    }

    /// The OS process id, when the worker is a local subprocess (`None` for a
    /// remote worker, or once the child has been reaped).
    pub(crate) fn pid(&self) -> Option<u32> {
        match &self.kind {
            WorkerKind::Subprocess(w) => w.child.id(),
            WorkerKind::Remote(_) => None,
        }
    }

    /// The child's CPU clock recorded at spawn, for one turn's supervision.
    pub(crate) fn cpu_clock(&self) -> io::Result<Clock> {
        match &self.kind {
            WorkerKind::Subprocess(worker) => worker
                .cpu_clock
                .clone()
                .ok_or_else(|| io::Error::other("worker was spawned without a CPU clock")),
            WorkerKind::Remote(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "remote child CPU clock is unavailable",
            )),
        }
    }

    /// Whether the worker has already died (used to discard workers that died
    /// while idle in the pool). Remote workers are never pooled idle, so
    /// they always report alive here.
    pub(crate) fn is_dead(&mut self) -> bool {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => w.child.try_wait().is_ok_and(|status| status.is_some()),
            WorkerKind::Remote(_) => false,
        }
    }

    /// Reaps a child that is already on its way out, falling back to
    /// [`Self::kill_and_reap`] if it has not gone within `grace`.
    ///
    /// For deaths the child *announces* before exiting — a `FatalError`, where
    /// it deliberately sets an exit code — killing it immediately would race
    /// its own exit and report the signal instead of that code, losing the one
    /// diagnostic the announcement was made to carry.
    pub(crate) async fn reap_or_kill(&mut self, grace: Duration) -> Option<ExitStatus> {
        if let WorkerKind::Subprocess(w) = &mut self.kind
            && let Ok(Ok(status)) = timeout(grace, w.child.wait()).await
        {
            return Some(status);
        }
        self.kill_and_reap().await
    }

    /// Tears the worker down (kills the child / ends the stream) and reaps
    /// it, returning the process exit status when there is one.
    pub(crate) async fn kill_and_reap(&mut self) -> Option<ExitStatus> {
        match &mut self.kind {
            WorkerKind::Subprocess(w) => {
                let _ = w.child.start_kill();
                w.child.wait().await.ok()
            }
            // the caller's capacity guard frees the slot now: a discarded
            // stream's server may still be stopping its work, which no grace
            // period here bounds, and a clean finish is the common path
            WorkerKind::Remote(w) => {
                w.close(|| {});
                None
            }
        }
    }

    /// Ends the stream behind a worker the caller is *not* discarding as
    /// dead — a cleanly finished checkout's single-use remote worker — and
    /// calls `on_closed` once the server has ended it too (or the close gave
    /// up). Subprocess workers, which the pool reuses or kills instead, call
    /// it at once.
    pub(crate) fn close_transport(&mut self, on_closed: impl FnOnce() + Send + 'static) {
        match &mut self.kind {
            WorkerKind::Remote(w) => w.close(on_closed),
            WorkerKind::Subprocess(_) => on_closed(),
        }
    }
}

/// Chunk size for speculative frame reads: large enough that a typical
/// prefix + body arrives in a single `read` syscall, small enough to sit
/// permanently in every idle worker.
const READ_CHUNK: usize = 8 * 1024;

/// Cap above which the reused send/recv buffers are dropped after use, so one
/// huge frame does not pin megabytes for the worker's lifetime.
const RETAIN_BUF_MAX: usize = 64 * 1024;

/// Initial capacity of the reused send buffer: covers control frames,
/// resumes with small values, and `Configure` (~150 bytes) without a realloc;
/// only large feeds/values grow past it (via `encode_framed_into`'s reserve).
const SEND_BUF_CAPACITY: usize = 1024;

/// Frames at or above this size decode under `block_in_place` (see
/// [`decode_event`]); below it (< ~4ms) inline decode beats the handoff.
pub(crate) const DECODE_OFFLOAD_MIN: usize = 1024 * 1024;

/// A cancel-safe reader of length-prefixed frames from the child's stdout.
///
/// Async `read_exact` is not cancel-safe — dropping it mid-frame loses the
/// bytes already read. Here the accumulation buffer and fill offset are
/// fields, and each await is a single `read` (which never consumes bytes when
/// it returns `Pending`), so a `recv` future dropped at any await point
/// leaves the accumulated bytes in place for the next call to resume from.
///
/// Reads accumulate into one buffer rather than prefix-then-body, so the
/// common whole-frame-ready case costs a single `read` syscall; bytes past
/// the frame boundary (coalesced print events) are retained for the next call.
struct FrameRecv {
    stdout: ChildStdout,
    /// Zero-initialized read buffer; `buf[consumed..filled]` is unparsed data.
    buf: Vec<u8>,
    /// Bytes already handed out as decoded frames.
    consumed: usize,
    /// Bytes read from the pipe so far.
    filled: usize,
}

impl FrameRecv {
    fn new(stdout: ChildStdout) -> Self {
        Self {
            stdout,
            buf: Vec::new(),
            consumed: 0,
            filled: 0,
        }
    }

    /// Reads and decodes one frame, returning it with its body size, and
    /// enforcing [`MAX_FRAME_LEN`] before growing the buffer to the announced
    /// size. `Ok(None)` on EOF at a frame boundary — the peer closed between
    /// messages; EOF inside a frame is [`FrameError::Truncated`].
    async fn recv(&mut self, max_decode_bytes: usize) -> Result<Option<(pb::ChildEvent, usize)>, FrameError> {
        // move leftover bytes from a previous coalesced read to the front so
        // the parse below always works from offset 0
        if self.consumed > 0 {
            self.buf.copy_within(self.consumed..self.filled, 0);
            self.filled -= self.consumed;
            self.consumed = 0;
        }
        loop {
            if self.filled >= 4 {
                let len = u32::from_le_bytes(self.buf[..4].try_into().expect("4 bytes"));
                if len > MAX_FRAME_LEN {
                    return Err(FrameError::FrameTooLarge {
                        len,
                        max: MAX_FRAME_LEN,
                    });
                }
                let end = 4 + len as usize;
                if self.filled >= end {
                    let event = decode_event(&self.buf[4..end], max_decode_bytes)?;
                    self.consumed = end;
                    // a fully drained oversize buffer is dropped rather than
                    // pinned for the worker's lifetime
                    if self.buf.len() > RETAIN_BUF_MAX && self.filled == end {
                        self.buf = Vec::new();
                        self.consumed = 0;
                        self.filled = 0;
                    }
                    return Ok(Some((event, len as usize)));
                }
                // Growth is validated against MAX_FRAME_LEN above, keeping
                // byzantine peers bounded to one frame buffer per worker.
                if self.buf.len() < end {
                    self.buf.resize(end, 0);
                }
            } else if self.buf.len() < READ_CHUNK {
                self.buf.resize(READ_CHUNK, 0);
            }
            let n = self.stdout.read(&mut self.buf[self.filled..]).await?;
            if n == 0 {
                // EOF at a frame boundary is a clean close; mid-frame the
                // peer died while writing.
                return if self.filled == 0 {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            self.filled += n;
        }
    }
}

/// Decodes one complete frame body, synchronously on the current thread.
///
/// A max-size frame can cost ~1s of CPU (see `benches/decode.rs`), so frames
/// ≥ [`DECODE_OFFLOAD_MIN`] decode under `block_in_place`: the decode borrows
/// the bytes (no copy), can't outlive a cancelled turn (unlike
/// `spawn_blocking`), and the runtime keeps its worker count. On a
/// current-thread runtime, where `block_in_place` panics, decode is inline.
pub(crate) fn decode_event(body: &[u8], max_decode_bytes: usize) -> Result<pb::ChildEvent, FrameError> {
    // each decode is one synchronous call, so the thread-local decode budgets
    // (scoped by `decode_frame_with_budget`) cannot interleave
    if body.len() < DECODE_OFFLOAD_MIN || Handle::current().runtime_flavor() != RuntimeFlavor::MultiThread {
        decode_frame_with_budget(body, max_decode_bytes)
    } else {
        block_in_place(|| decode_frame_with_budget(body, max_decode_bytes))
    }
}
