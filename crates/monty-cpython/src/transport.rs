//! The wire between this child and its parent, abstracted so the protocol
//! state machine in [`crate::session`] is transport-agnostic.
//!
//! A [`Transport`] moves whole protocol messages: one [`pb::ParentRequest`] in,
//! one [`pb::ChildEvent`] out. The framing lives in the adapter — [`StdioTransport`]
//! prepends monty's 4-byte little-endian length prefix (so the binary can drop
//! in under the existing `monty-pool` as a `--subprocess` worker), while the
//! WebSocket adapters (added later) use one binary message per frame.

use std::{
    cell::RefCell,
    io::{self, Read, Stdin, Stdout, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
};

use monty_proto::{FrameError, FrameReader, decode_frame, encode_to_capped_vec, pb, write_frame};
use tungstenite::{Error as WsError, Message, WebSocket, stream::MaybeTlsStream};

/// A transport shared between the session loop (which reads requests and writes
/// turn-enders) and the in-feed [`crate::pyexec::HostBridge`] (which sends
/// `FunctionCall`s and reads `ResumeCall`s). `Rc<RefCell<…>>` is sound because
/// the child is single-threaded: the two never borrow it at the same time.
pub type SharedTransport = Rc<RefCell<Box<dyn Transport>>>;

/// The result of trying to read one request from the parent.
///
/// The variants mirror the recovery contract of the subprocess child: a
/// `Malformed` frame leaves the stream synced (answer with an error and keep
/// serving), whereas `Eof`/`Fatal` end the session.
pub enum Incoming {
    /// A decoded request to handle.
    Request(pb::ParentRequest),
    /// The parent closed the connection cleanly at a frame boundary.
    Eof,
    /// A frame arrived but its payload failed to decode/validate. The stream is
    /// still in sync, so the caller answers with a turn-ending error and
    /// continues.
    Malformed(String),
    /// The stream desynchronized or the underlying I/O broke — unrecoverable.
    Fatal(String),
}

/// Why sending an event failed.
pub enum SendError {
    /// The encoded event exceeded the wire frame limit. Recoverable when not
    /// mid-suspension (answer with a smaller error instead).
    TooLarge { len: u32, max: u32 },
    /// The underlying transport broke (peer gone) — nothing left to do.
    Io(String),
}

/// Moves whole protocol messages between this child and its parent.
pub trait Transport {
    /// Blocks until the next request arrives, the peer closes, or the stream
    /// breaks.
    fn recv(&mut self) -> Incoming;

    /// Writes one event to the parent.
    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), SendError>;
}

/// stdio transport: framed exactly like `monty --subprocess` (4-byte LE length
/// prefix), so the existing `monty-pool` can spawn and drive this binary
/// unchanged. Diagnostics must go to stderr — stdout carries only frames.
pub struct StdioTransport {
    reader: FrameReader<Stdin>,
    stdout: Stdout,
}

impl StdioTransport {
    /// Builds the transport over the process's stdin/stdout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reader: FrameReader::new(io::stdin()),
            stdout: io::stdout(),
        }
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for StdioTransport {
    fn recv(&mut self) -> Incoming {
        match self.reader.read::<pb::ParentRequest>() {
            Ok(Some(request)) => Incoming::Request(request),
            Ok(None) => Incoming::Eof,
            // A framed-but-undecodable payload (including values failing
            // semantic validation) leaves the stream synced.
            Err(FrameError::Decode(err)) => Incoming::Malformed(err.to_string()),
            Err(err) => Incoming::Fatal(err.to_string()),
        }
    }

    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), SendError> {
        match write_frame(&mut self.stdout, event) {
            Ok(()) => Ok(()),
            Err(FrameError::FrameTooLarge { len, max }) => Err(SendError::TooLarge { len, max }),
            Err(err) => Err(SendError::Io(err.to_string())),
        }
    }
}

/// WebSocket transport: one binary message per protocol frame (no length
/// prefix). Generic over the stream so it covers both `--connect` (which dials a
/// relay/server, possibly over TLS — `MaybeTlsStream`) and `--listen` (which
/// accepts one parent — a plain `TcpStream`).
pub struct WsTransport<S> {
    socket: WebSocket<S>,
}

/// Dials `url` (a relay, or a child-as-server) as a WebSocket client.
pub fn connect(url: &str) -> io::Result<WsTransport<MaybeTlsStream<TcpStream>>> {
    let (socket, _response) = tungstenite::connect(url).map_err(ws_io_error)?;
    Ok(WsTransport { socket })
}

/// Binds `addr` and accepts exactly one parent connection (server mode).
pub fn listen(addr: &str) -> io::Result<WsTransport<TcpStream>> {
    let listener = TcpListener::bind(addr)?;
    let (stream, _peer) = listener.accept()?;
    // `accept` returns a `HandshakeError` (not `tungstenite::Error`); on a
    // blocking stream the handshake never returns `Interrupted`, so flatten it.
    let socket = tungstenite::accept(stream).map_err(|err| io::Error::other(err.to_string()))?;
    Ok(WsTransport { socket })
}

impl<S: Read + Write> Transport for WsTransport<S> {
    fn recv(&mut self) -> Incoming {
        loop {
            match self.socket.read() {
                Ok(Message::Binary(data)) => {
                    return match decode_frame::<pb::ParentRequest>(data.as_ref()) {
                        Ok(request) => Incoming::Request(request),
                        // A framed-but-undecodable payload leaves the stream synced.
                        Err(FrameError::Decode(err)) => Incoming::Malformed(err.to_string()),
                        Err(err) => Incoming::Fatal(err.to_string()),
                    };
                }
                // tungstenite auto-queues the Pong; flush it and keep reading.
                Ok(Message::Ping(_)) => {
                    let _ = self.socket.flush();
                }
                Ok(Message::Pong(_)) => {}
                // A clean close means the parent is done with this session.
                Ok(Message::Close(_)) => return Incoming::Eof,
                Ok(Message::Text(_) | Message::Frame(_)) => {
                    return Incoming::Fatal("unexpected text/raw WebSocket frame".to_owned());
                }
                Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => return Incoming::Eof,
                Err(err) => return Incoming::Fatal(err.to_string()),
            }
        }
    }

    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), SendError> {
        let body = encode_to_capped_vec(event).map_err(|err| match err {
            FrameError::FrameTooLarge { len, max } => SendError::TooLarge { len, max },
            other => SendError::Io(other.to_string()),
        })?;
        self.socket
            .write(Message::Binary(body.into()))
            .map_err(|err| SendError::Io(err.to_string()))?;
        self.socket.flush().map_err(|err| SendError::Io(err.to_string()))?;
        Ok(())
    }
}

/// Maps a tungstenite handshake/connect error onto an `io::Error`.
fn ws_io_error(err: WsError) -> io::Error {
    match err {
        WsError::Io(err) => err,
        other => io::Error::other(other.to_string()),
    }
}
