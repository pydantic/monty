//! The gRPC remote transport: one HTTP/2 channel per pool, one
//! `MontySession.Session` stream per session.
//!
//! [`GrpcChannel`] owns the connection (dialed lazily, reconnected by tonic
//! when it drops). Requests reach the stream as its request body, drained from
//! the worker's `mpsc` channel; the response stream is the session's
//! [`MessageSource`], and a status ending it carries the server's reason.

use std::{
    error::Error,
    fmt::Display,
    sync::Arc,
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use h2::Reason;
use http::{Response, StatusCode, uri::PathAndQuery};
use monty_proto::{
    FrameError, MAX_FRAME_LEN,
    grpc::{BytesCodec, Cause, SESSION_PATH},
};
use rustls_platform_verifier::Verifier;
use tokio::{
    sync::mpsc,
    time::{Instant, timeout_at},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    Code, Extensions, Request, Status, Streaming,
    body::Body,
    client::Grpc,
    metadata::{MetadataKey, MetadataMap, MetadataValue},
    transport::{Channel, ClientTlsConfig, Endpoint},
};
use tower::ServiceExt;

use super::{
    Ended, MessageSource, REQUEST_CHANNEL_DEPTH, USER_AGENT_VALUE, code_name, connection_failure,
    install_crypto_provider, message_limit, refuses_admission, retry_after,
};
use crate::{PoolError, RemoteDisconnect};

/// A pool's gRPC channel. Cheap to keep: it connects on the first stream and
/// stays up across checkouts.
pub(super) struct GrpcChannel {
    channel: Channel,
}

impl GrpcChannel {
    /// Prepares the channel for `url` (whose lowercased scheme is `scheme`,
    /// `http` or `https`, and the rest `rest`) without connecting. A path
    /// (other than `/`), query or fragment is refused: tonic keeps only the
    /// scheme and authority, so it would be dropped without a word.
    pub(super) fn connect(
        url: &str,
        scheme: &str,
        rest: &str,
        dial_timeout: Duration,
        keep_alive: Option<Duration>,
        max_processes: usize,
    ) -> Result<Self, PoolError> {
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if !matches!(&rest[authority_end..], "" | "/") {
            return Err(connection_failure(
                url,
                "a URL path, query or fragment is not supported for gRPC; use only the scheme and host, with an \
                 optional port",
            ));
        }
        let connect_error = |err: &dyn Display| connection_failure(url, err.to_string());
        let mut endpoint = Endpoint::from_shared(format!("{scheme}://{rest}"))
            .map_err(|err| connect_error(&err))?
            .user_agent(USER_AGENT_VALUE)
            .map_err(|err| connect_error(&err))?
            .connect_timeout(dial_timeout)
            .tcp_nodelay(true)
            // fixed windows, the connection's well above a stream's: adaptive
            // ones set both to one estimate, so a session whose host stopped
            // reading held the whole connection and starved every other session
            .initial_stream_window_size(stream_window(max_processes))
            .initial_connection_window_size(connection_window(max_processes));
        if let Some(interval) = keep_alive {
            // Hop-local liveness while streams are open: a dead TCP peer is
            // noticed without waiting for a turn to time out (the in-band
            // `Ping` covers proxied hops). Never while idle: a gRPC server's
            // default policy counts a ping without a stream as abuse.
            endpoint = endpoint
                .http2_keep_alive_interval(interval)
                .keep_alive_timeout(KEEP_ALIVE_TIMEOUT)
                .keep_alive_while_idle(false);
        }
        if scheme == "https" {
            // The server certificate is checked against the OS trust store:
            // macOS and Windows verify it themselves, revocation included; Linux
            // reads the distro CA bundle now, and a host with none fails here.
            let verifier = Verifier::new(install_crypto_provider()).map_err(|err| connect_error(&err))?;
            endpoint = endpoint
                .tls_config_with_verifier(ClientTlsConfig::new(), Arc::new(verifier))
                .map_err(|err| connect_error(&err))?;
        }
        Ok(Self {
            channel: endpoint.connect_lazy(),
        })
    }

    /// Opens one session stream with `metadata` as its request metadata,
    /// returning the request channel and the response stream once the server
    /// has admitted it (sent its response headers). See [`super::Remote::open`].
    pub(super) async fn open(
        &self,
        url: &str,
        dial_timeout: Duration,
        metadata: &[(String, String)],
        max_decode_bytes: usize,
    ) -> Result<(mpsc::Sender<Bytes>, Streaming<Bytes>), PoolError> {
        let mut map = MetadataMap::new();
        for (name, value) in metadata {
            let key = MetadataKey::from_bytes(name.as_bytes())
                .map_err(|err| PoolError::Configuration(format!("{url}: metadata entry {name:?}: {err}")))?;
            let value = MetadataValue::try_from(value.as_str())
                .map_err(|err| PoolError::Configuration(format!("{url}: metadata entry {name:?} value: {err}")))?;
            // never indexed: a token must not sit in the connection's HPACK
            // table, which every stream on it shares
            let mut value = value;
            value.set_sensitive(true);
            map.insert(key, value);
        }
        let channel = self.channel.clone().map_response(|mut response: Response<Body>| {
            let status = response.status();
            response.extensions_mut().insert(status);
            response
        });
        let mut grpc = Grpc::new(channel)
            .max_decoding_message_size(message_limit(max_decode_bytes))
            .max_encoding_message_size(MAX_FRAME_LEN as usize);
        // an unrepresentable timeout (`Duration::MAX`) leaves the dial unbounded
        let deadline = Instant::now().checked_add(dial_timeout);
        let mut retries = 0;
        loop {
            let (requests, request_body) = mpsc::channel::<Bytes>(REQUEST_CHANNEL_DEPTH);
            let request = Request::from_parts(map.clone(), Extensions::default(), ReceiverStream::new(request_body));
            let open = async {
                grpc.ready().await.map_err(|err| Status::from_error(err.into()))?;
                grpc.streaming(request, PathAndQuery::from_static(SESSION_PATH), BytesCodec)
                    .await
            };
            let opened = match deadline {
                Some(deadline) => timeout_at(deadline, open).await,
                None => Ok(open.await),
            };
            match opened {
                // tonic opens a stream on any response without a `grpc-status`,
                // so a plain web server's error page would only fail later, mid-
                // session, as an undecodable body: refuse it here as a misconfiguration
                Ok(Ok(response)) if !is_grpc_response(response.metadata()) => {
                    return Err(connection_failure(
                        url,
                        format!(
                            "not a gRPC server: HTTP {}, the response's content-type is {}",
                            response
                                .extensions()
                                .get::<StatusCode>()
                                .map_or_else(|| "status unknown".to_owned(), ToString::to_string),
                            describe_content_type(response.metadata())
                        ),
                    ));
                }
                Ok(Ok(response)) => return Ok((requests, response.into_inner())),
                // the server closed the connection under the stream before
                // taking it (a GOAWAY as it drains, a refused stream): the
                // channel reconnects, and the same budget covers the retries
                Ok(Err(status)) if retries < OPEN_RETRIES && stream_not_taken(&status) => retries += 1,
                // the server's own admission decision, kept structured; a
                // status tonic synthesised for a transport failure has a
                // source error, and any other code (`UNIMPLEMENTED` from a URL
                // that is no Monty server) is a misconfiguration, not a refusal
                Ok(Err(status)) if Error::source(&status).is_none() && refuses_admission(status.code()) => {
                    return Err(PoolError::Refused {
                        url: url.to_owned(),
                        refusal: Box::new(RemoteDisconnect::from_refusal(&status)),
                    });
                }
                Ok(Err(status)) => {
                    let details = RemoteDisconnect::from_connect_status(url, &status);
                    return Err(PoolError::Connect {
                        url: url.to_owned(),
                        details: Box::new(details),
                    });
                }
                Err(_) => {
                    return Err(connection_failure(
                        url,
                        format!("the server did not accept the session within {dial_timeout:?} (request_timeout)"),
                    ));
                }
            }
        }
    }
}

/// The response stream: each message is one protocol frame; the status that
/// ends it is the server's reason.
impl MessageSource for Streaming<Bytes> {
    async fn next(&mut self) -> Result<Bytes, Ended> {
        match self.message().await {
            Ok(Some(body)) => Ok(body),
            // the server ended the stream with an OK status: a session it
            // considers finished, which mid-checkout is still a loss
            Ok(None) => Err(Ended::status(RemoteDisconnect::status(Code::Ok, None, None))),
            Err(status) => Err(match oversized_message(&status) {
                Some(err) => Ended::failure(err),
                None => Ended::status(RemoteDisconnect::from_status(&status)),
            }),
        }
    }
}

/// Whether a response's headers are a gRPC server's: the protocol requires a
/// `content-type` of `application/grpc`, optionally with a `+format` or
/// `;params` suffix, on every response.
fn is_grpc_response(metadata: &MetadataMap) -> bool {
    metadata
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        .is_some_and(|value| {
            value
                .strip_prefix(GRPC_CONTENT_TYPE)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['+', ';']))
        })
}

/// A response's `content-type` for an error message: quoted, or `missing`.
fn describe_content_type(metadata: &MetadataMap) -> String {
    metadata.get(CONTENT_TYPE).map_or_else(
        || "missing".to_owned(),
        |value| format!("{:?}", value.to_str().unwrap_or("<non-ASCII>")),
    )
}

/// The `content-type` header, as tonic's metadata names it.
const CONTENT_TYPE: &str = "content-type";

/// The media type every gRPC response carries, before any suffix.
const GRPC_CONTENT_TYPE: &str = "application/grpc";

/// How many more times a stream the server never took is opened: once on the
/// connection that was closing, once on the one the channel dials to replace it.
const OPEN_RETRIES: u32 = 2;

/// How long an HTTP/2 `PING` may go unanswered before the connection is
/// judged dead. Connection-level, hop-local: it detects a dead TCP peer, not
/// a vanished server beyond a proxy (that is the in-band `Ping`'s job). The
/// interval between pings is `PoolConfig::keep_alive`.
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// A stream's HTTP/2 receive window, unless the pool is too large for every
/// stream to hold this much (see [`stream_window`]).
pub(super) const STREAM_WINDOW_BYTES: u32 = 2 * 1024 * 1024;

/// The smallest stream window: HTTP/2's default.
pub(super) const MIN_STREAM_WINDOW_BYTES: u32 = 65_535;

/// The smallest connection window, so a small pool keeps the window it had.
pub(super) const MIN_CONNECTION_WINDOW_BYTES: u32 = 32 * 1024 * 1024;

/// HTTP/2's largest flow-control window.
pub(super) const MAX_WINDOW_BYTES: u32 = (1 << 31) - 1;

/// How many streams the connection window leaves room for: `max_processes`
/// live, as many closing, and one to spare for a neighbour's reply.
fn window_streams(max_processes: usize) -> u32 {
    let streams = max_processes.max(1).saturating_mul(2).saturating_add(1);
    u32::try_from(streams).unwrap_or(u32::MAX)
}

/// A stream's receive window: [`STREAM_WINDOW_BYTES`], shrunk for a pool so
/// large that every stream stalled on it would overrun HTTP/2's largest
/// connection window. Below [`MIN_STREAM_WINDOW_BYTES`] it stops shrinking, so
/// a pool past about 16,000 processes can again be starved by stalled streams.
pub(super) fn stream_window(max_processes: usize) -> u32 {
    (MAX_WINDOW_BYTES / window_streams(max_processes)).clamp(MIN_STREAM_WINDOW_BYTES, STREAM_WINDOW_BYTES)
}

/// The connection's receive window, shared by every stream on the channel:
/// room for each of [`window_streams`] to sit on a full [`stream_window`], so
/// stalled sessions never take the credit a neighbour's reply needs.
pub(super) fn connection_window(max_processes: usize) -> u32 {
    window_streams(max_processes)
        .checked_mul(stream_window(max_processes))
        .map_or(MAX_WINDOW_BYTES, |window| window.min(MAX_WINDOW_BYTES))
        .max(MIN_CONNECTION_WINDOW_BYTES)
}

/// The frame error for tonic refusing a message above [`message_limit`],
/// which it reports as a local `OUT_OF_RANGE` status naming both sizes.
fn oversized_message(status: &Status) -> Option<FrameError> {
    if status.code() != Code::OutOfRange {
        return None;
    }
    let sizes = status
        .message()
        .strip_prefix("Error, decoded message length too large: found ")?
        .strip_suffix(" bytes")?;
    let (len, max) = sizes.split_once(" bytes, the limit is: ")?;
    Some(FrameError::FrameTooLarge {
        len: len.parse().unwrap_or(u32::MAX),
        max: max.parse().ok()?,
    })
}

impl RemoteDisconnect {
    /// Adds a TLS hint to an HTTP/2 framing failure on a plaintext URL.
    pub(super) fn from_connect_status(url: &str, status: &Status) -> Self {
        let mut details = Self::from_status(status);
        if url
            .split_once("://")
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http"))
            && let Some(error) = &mut details.transport_error
        {
            let mut source = Error::source(status);
            while let Some(cause) = source {
                if cause
                    .downcast_ref::<h2::Error>()
                    .is_some_and(|err| err.reason() == Some(Reason::FRAME_SIZE_ERROR))
                {
                    error.push_str("; the server may require https:// instead of http://");
                    break;
                }
                source = cause.source();
            }
        }
        details
    }

    /// Classifies a status that ended the stream: one the server sent carries
    /// its code, message and cause; one tonic synthesised for a transport
    /// failure (it has a source error) is recorded as that failure instead.
    fn from_status(status: &Status) -> Self {
        if Error::source(status).is_some() {
            Self::failure(describe_status(status))
        } else {
            Self::status(status.code(), Some(status.message().to_owned()), Cause::of(status))
        }
    }

    /// Classifies an admission refusal, keeping the server's `Retry-After`:
    /// retry hints mean nothing once a session exists.
    fn from_refusal(status: &Status) -> Self {
        Self {
            retry_after: status
                .metadata()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| retry_after(value, SystemTime::now())),
            ..Self::from_status(status)
        }
    }
}

/// Whether `status` says the server never took the stream: a `GOAWAY` whose
/// last stream is below it (the server was going away as it was opened), a
/// `REFUSED_STREAM` reset, or hyper cancelling the request because the
/// connection closed before it was sent. HTTP/2 promises such a stream was
/// not processed, so opening it again is safe. h2 reports a `GOAWAY` on a
/// stream only when the stream's id is above the frame's last stream id; a
/// stream the server did process never sees it.
fn stream_not_taken(status: &Status) -> bool {
    let mut source = Error::source(status);
    while let Some(err) = source {
        if let Some(h2) = err.downcast_ref::<h2::Error>() {
            return (h2.is_go_away() && h2.reason() == Some(Reason::NO_ERROR))
                || (h2.is_reset() && h2.reason() == Some(Reason::REFUSED_STREAM));
        }
        if let Some(hyper) = err.downcast_ref::<hyper::Error>()
            && hyper.is_canceled()
        {
            return true;
        }
        source = err.source();
    }
    false
}

/// Renders a status for an error message: its code and message, then the
/// chain of underlying errors tonic attached for a transport failure.
fn describe_status(status: &Status) -> String {
    let mut text = format!("{}: {}", code_name(status.code()), status.message());
    let mut source = Error::source(status);
    while let Some(err) = source {
        text.push_str(": ");
        text.push_str(&err.to_string());
        source = err.source();
    }
    text
}
