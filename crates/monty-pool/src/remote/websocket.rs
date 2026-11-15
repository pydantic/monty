//! The WebSocket remote transport: one connection per session, one binary
//! message per protocol frame, with no length prefix.
//!
//! A writer task drains the worker's request channel into the connection and
//! sends a Close frame once the worker drops it; the read half is the
//! session's [`MessageSource`]. The server's reasons travel without free text
//! to parse: a refused upgrade's HTTP status and `monty-cause` header, or a
//! Close frame's code, of which each [`Cause`] has one of its own.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use http::{
    HeaderName, HeaderValue, Response, StatusCode,
    header::{RETRY_AFTER, USER_AGENT},
};
use monty_proto::{CAUSE_METADATA_KEY, Cause, FrameError};
use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::{
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{
        Error as WsError, Message,
        client::IntoClientRequest,
        error::CapacityError,
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};
use tonic::Code;

use super::{
    Ended, MessageSource, REQUEST_CHANNEL_DEPTH, USER_AGENT_VALUE, connection_failure, install_crypto_provider,
    message_limit, refuses_admission, retry_after,
};
use crate::{PoolError, RemoteDisconnect};

/// A connected WebSocket.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Dials a pool's WebSocket sessions. Holds no connection: each session
/// dials its own.
pub(super) struct WebSocketDialer {
    /// The URL dialed, its scheme lowercased; the path and query are kept,
    /// since a relay may route on them.
    url: String,
    /// The TLS configuration for `wss://`, verifying the server against the
    /// OS trust store as gRPC does; `None` for `ws://`.
    tls: Option<Arc<ClientConfig>>,
}

impl WebSocketDialer {
    /// Prepares to dial `url` (whose lowercased scheme is `scheme`, `ws` or
    /// `wss`, and the rest `rest`). A URL that is no valid request target, or
    /// a host without a trust store for `wss://`, fails here.
    pub(super) fn new(url: &str, scheme: &str, rest: &str) -> Result<Self, PoolError> {
        let dial_url = format!("{scheme}://{rest}");
        dial_url
            .as_str()
            .into_client_request()
            .map_err(|err| connection_failure(url, err.to_string()))?;
        let tls = if scheme == "wss" {
            let config = ClientConfig::builder_with_provider(install_crypto_provider())
                .with_safe_default_protocol_versions()
                .and_then(BuilderVerifierExt::with_platform_verifier)
                .map_err(|err| connection_failure(url, err.to_string()))?
                .with_no_client_auth();
            Some(Arc::new(config))
        } else {
            None
        };
        Ok(Self { url: dial_url, tls })
    }

    /// Dials one session with `metadata` as upgrade headers, returning the
    /// request channel and the read half once the server has accepted the
    /// upgrade. See [`super::Remote::open`].
    pub(super) async fn open(
        &self,
        url: &str,
        dial_timeout: Duration,
        metadata: &[(String, String)],
        max_decode_bytes: usize,
    ) -> Result<(mpsc::Sender<Bytes>, WebSocketSource), PoolError> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|err| connection_failure(url, err.to_string()))?;
        // set before the caller's headers so a `user-agent` entry there overrides it
        request
            .headers_mut()
            .insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        for (name, value) in metadata {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|err| PoolError::Configuration(format!("{url}: metadata entry {name:?}: {err}")))?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|err| PoolError::Configuration(format!("{url}: metadata entry {name:?} value: {err}")))?;
            value.set_sensitive(true);
            request.headers_mut().insert(name, value);
        }
        let limit = message_limit(max_decode_bytes);
        let config = WebSocketConfig::default()
            .max_frame_size(Some(limit))
            .max_message_size(Some(limit));
        let connector = self.tls.clone().map(Connector::Rustls);
        let dial = connect_async_tls_with_config(request, Some(config), true, connector);
        // an unrepresentable timeout (`Duration::MAX`) leaves the dial unbounded
        let dialed = match Instant::now().checked_add(dial_timeout) {
            Some(deadline) => timeout_at(deadline, dial).await,
            None => Ok(dial.await),
        };
        let stream = match dialed {
            Ok(Ok((stream, _response))) => stream,
            Ok(Err(WsError::Http(response))) => return Err(refusal(url, &response)),
            Ok(Err(err)) => return Err(connection_failure(url, err.to_string())),
            Err(_) => {
                return Err(connection_failure(
                    url,
                    format!("the server did not accept the session within {dial_timeout:?} (request_timeout)"),
                ));
            }
        };
        let (sink, read_half) = stream.split();
        let (requests, request_body) = mpsc::channel(REQUEST_CHANNEL_DEPTH);
        let writer = tokio::spawn(write_requests(sink, request_body));
        let source = WebSocketSource {
            read_half,
            _writer: AbortOnDrop(writer),
        };
        Ok((requests, source))
    }
}

/// A session's read half, and the writer task it outlives: when the reader
/// finishes or is aborted this drops, aborting a writer stuck on a peer
/// that stopped reading, and the connection closes with both halves gone.
pub(super) struct WebSocketSource {
    read_half: SplitStream<WsStream>,
    /// Held only to abort the writer when this drops.
    _writer: AbortOnDrop,
}

impl MessageSource for WebSocketSource {
    async fn next(&mut self) -> Result<Bytes, Ended> {
        loop {
            match self.read_half.next().await {
                Some(Ok(Message::Binary(data))) => return Ok(data),
                // polling is itself what answers: tungstenite queues the pong
                // on read and flushes it during read polling
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(frame))) => return Err(Ended::status(close_disconnect(frame))),
                Some(Ok(Message::Text(_) | Message::Frame(_))) => {
                    return Err(Ended {
                        disconnect: RemoteDisconnect::failure("unexpected non-binary WebSocket message"),
                        error: FrameError::Truncated,
                    });
                }
                None => {
                    return Err(Ended {
                        disconnect: RemoteDisconnect::failure("WebSocket connection closed without a Close frame"),
                        error: FrameError::Truncated,
                    });
                }
                Some(Err(err)) => {
                    return Err(Ended {
                        disconnect: RemoteDisconnect::failure(err.to_string()),
                        error: ws_to_frame_error(err),
                    });
                }
            }
        }
    }
}

/// Aborts a task when dropped.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The writer task: sends each request as one binary message, then a normal
/// Close frame once the worker drops the channel — what a proxied server sees,
/// where a bare TCP drop may stop at the proxy.
///
/// A failed send ends the task without recording anything: if the server
/// closed the connection, the read side holds its Close frame, the reason this
/// session ended, and a local "connection closed" must not shadow it.
async fn write_requests(mut sink: SplitSink<WsStream, Message>, mut requests: mpsc::Receiver<Bytes>) {
    while let Some(body) = requests.recv().await {
        if sink.send(Message::Binary(body)).await.is_err() {
            return;
        }
    }
    let close = Message::Close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "".into(),
    }));
    let _ = timeout(CLOSE_WRITE_TIMEOUT, sink.send(close)).await;
}

/// How long the writer gives its Close frame to reach the OS. Not a latency
/// budget (the echo is the reader's to wait for) — it only rides out
/// back-pressure from a peer that stopped reading.
const CLOSE_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// What a server's Close frame says: a [`Cause`]'s own code reports that cause
/// and its gRPC code, a standard code its nearest gRPC code (see
/// [`close_code_status`]). The reason text is only the message, never parsed.
/// A Close without a code is a plain finish, as gRPC's `OK`.
pub(super) fn close_disconnect(frame: Option<CloseFrame>) -> RemoteDisconnect {
    let Some(frame) = frame else {
        return RemoteDisconnect::status(Code::Ok, None, None);
    };
    let code = u16::from(frame.code);
    let cause = Cause::from_close_code(code);
    let message = if !frame.reason.is_empty() {
        Some(frame.reason.to_string())
    } else if code == NORMAL_CLOSURE {
        None
    } else {
        Some(format!("close code {code}"))
    };
    RemoteDisconnect::status(
        cause.map_or_else(|| close_code_status(code), Cause::code),
        message,
        cause,
    )
}

/// RFC 6455's normal closure: the server finished the session.
const NORMAL_CLOSURE: u16 = 1000;

/// The gRPC code nearest a close code outside the [`Cause`] range.
fn close_code_status(code: u16) -> Code {
    match code {
        NORMAL_CLOSURE => Code::Ok,
        // going away: the server is shutting down
        1001 => Code::Unavailable,
        // policy violation
        1008 => Code::PermissionDenied,
        // message too big
        1009 => Code::ResourceExhausted,
        // internal error
        1011 => Code::Internal,
        _ => Code::Unknown,
    }
}

/// Classifies a refused upgrade. A known `monty-cause` header names the
/// cause and its code; otherwise the HTTP status maps to a gRPC code as
/// gRPC's own HTTP mapping does. A refusal code is a
/// [`PoolError::Refused`]; anything else (a 404, a plain web page) is no
/// Monty server, a [`PoolError::Connect`].
pub(super) fn refusal(url: &str, response: &Response<Option<Vec<u8>>>) -> PoolError {
    let status = response.status();
    let header = |name| response.headers().get(name).and_then(|value| value.to_str().ok());
    let cause = header(CAUSE_METADATA_KEY).and_then(|value| value.parse::<Cause>().ok());
    let code = cause.map_or_else(|| http_status_code(status), Cause::code);
    let mut message = format!("HTTP {status}");
    if let Some(body) = response.body().as_deref().map(describe_body)
        && !body.is_empty()
    {
        message.push_str(": ");
        message.push_str(&body);
    }
    if refuses_admission(code) {
        PoolError::Refused {
            url: url.to_owned(),
            refusal: Box::new(RemoteDisconnect {
                retry_after: header(RETRY_AFTER.as_str()).and_then(|value| retry_after(value, SystemTime::now())),
                ..RemoteDisconnect::status(code, Some(message), cause)
            }),
        }
    } else {
        connection_failure(url, format!("the server refused the WebSocket upgrade: {message}"))
    }
}

/// The gRPC code gRPC's HTTP-to-status mapping gives `status`.
fn http_status_code(status: StatusCode) -> Code {
    match status.as_u16() {
        400 => Code::Internal,
        401 => Code::Unauthenticated,
        403 => Code::PermissionDenied,
        404 => Code::Unimplemented,
        429 | 502 | 503 | 504 => Code::Unavailable,
        _ => Code::Unknown,
    }
}

/// A refusal body for an error message: its first line, trimmed and capped
/// at [`MAX_BODY_CHARS`], since an untrusted server chooses it.
fn describe_body(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    match line.char_indices().nth(MAX_BODY_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

/// The most characters of a refusal body reported.
const MAX_BODY_CHARS: usize = 200;

/// Preserves I/O errors and oversize messages, and classifies every other
/// WebSocket failure as a lost connection.
fn ws_to_frame_error(err: WsError) -> FrameError {
    match err {
        WsError::Io(err) => FrameError::Io(err),
        WsError::Capacity(CapacityError::MessageTooLong { size, max_size }) => FrameError::FrameTooLarge {
            len: u32::try_from(size).unwrap_or(u32::MAX),
            max: u32::try_from(max_size).unwrap_or(u32::MAX),
        },
        _ => FrameError::Truncated,
    }
}
