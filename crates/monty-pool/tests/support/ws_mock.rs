//! An in-process WebSocket server standing in for a remote worker, scripted
//! from a plain thread like `grpc_mock`: each connection the pool dials
//! becomes a [`MockSocket`] with blocking `read` / `send` / `close`, while the
//! pool under test runs on the tokio runtime.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::{HeaderMap, Response, StatusCode};
use monty_proto::MAX_FRAME_LEN;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    task::JoinHandle,
};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{ErrorResponse, Request, Response as UpgradeResponse},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

use crate::opening::Opening;

/// How long [`MockServer::accept`] and [`MockSocket::read`] wait before
/// declaring the test stuck.
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// A listening mock server. Dropping it stops accepting.
pub struct MockServer {
    url: String,
    incoming: mpsc::Receiver<MockSocket>,
    refuse: Arc<Mutex<Option<Refusal>>>,
    task: JoinHandle<()>,
}

/// An HTTP response the next upgrade is refused with.
struct Refusal {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl MockServer {
    /// Binds an ephemeral loopback port and accepts WebSocket upgrades on any path.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, incoming) = mpsc::channel();
        let refuse = Arc::new(Mutex::new(None));
        let task = tokio::spawn(accept_loop(listener, tx, Arc::clone(&refuse)));
        Self {
            url: format!("ws://127.0.0.1:{port}"),
            incoming,
            refuse,
            task,
        }
    }

    /// The URL a pool dials to reach this server.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Blocks until the pool dials the next session. Panics after [`WAIT_TIMEOUT`].
    pub fn accept(&self) -> MockSocket {
        match self.incoming.recv_timeout(WAIT_TIMEOUT) {
            Ok(socket) => socket,
            Err(RecvTimeoutError::Timeout) => panic!("no session was dialed within {WAIT_TIMEOUT:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the mock server stopped before a session was dialed"),
        }
    }

    /// The next session if the pool dials one within `wait`.
    pub fn try_accept(&self, wait: Duration) -> Option<MockSocket> {
        self.incoming.recv_timeout(wait).ok()
    }

    /// Refuses the next upgrade with `status`, `headers` and `body`.
    pub fn refuse_next(&self, status: u16, headers: &[(&str, &str)], body: &str) {
        *self.refuse.lock().unwrap() = Some(Refusal {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.to_owned(),
        });
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Accepts connections until aborted, handing each upgraded one to the script.
async fn accept_loop(listener: TcpListener, incoming: mpsc::Sender<MockSocket>, refuse: Arc<Mutex<Option<Refusal>>>) {
    while let Ok((tcp, _)) = listener.accept().await {
        let incoming = incoming.clone();
        let refusal = refuse.lock().unwrap().take();
        tokio::spawn(async move {
            if let Some(socket) = upgrade(tcp, refusal).await {
                let _ = incoming.send(socket);
            }
        });
    }
}

/// Runs the upgrade (or refuses it), then spawns the task serving the connection.
async fn upgrade(tcp: TcpStream, refusal: Option<Refusal>) -> Option<MockSocket> {
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);
    // tungstenite fixes the callback's signature, large error response and all
    #[expect(clippy::result_large_err)]
    let callback = move |request: &Request, response: UpgradeResponse| {
        *record.lock().unwrap() = Some((
            request.headers().clone(),
            request
                .uri()
                .path_and_query()
                .map_or_else(String::new, ToString::to_string),
        ));
        match refusal {
            Some(refusal) => {
                let mut error = Response::builder().status(StatusCode::from_u16(refusal.status).unwrap());
                for (name, value) in &refusal.headers {
                    error = error.header(name, value);
                }
                let body = (!refusal.body.is_empty()).then_some(refusal.body);
                Err::<UpgradeResponse, ErrorResponse>(error.body(body).unwrap())
            }
            None => Ok(response),
        }
    };
    let config = WebSocketConfig::default()
        .max_frame_size(Some(MAX_FRAME_LEN as usize))
        .max_message_size(Some(MAX_FRAME_LEN as usize));
    let ws = accept_hdr_async_with_config(tcp, callback, Some(config)).await.ok()?;
    let (headers, path) = seen.lock().unwrap().take().expect("the callback ran");
    let (requests_tx, requests) = mpsc::channel();
    let (commands, commands_rx) = unbounded_channel();
    let client_close = Arc::new(Mutex::new(None));
    let pongs = Arc::new(AtomicUsize::new(0));
    tokio::spawn(serve(
        ws,
        requests_tx,
        commands_rx,
        Arc::clone(&client_close),
        Arc::clone(&pongs),
    ));
    Some(MockSocket {
        requests,
        commands,
        headers,
        path,
        read_timeout: WAIT_TIMEOUT,
        opening: Opening::default(),
        client_close,
        pongs,
    })
}

/// What the script asks the connection task to do.
enum Command {
    Send(Message),
    /// Drop the TCP connection without a Close frame.
    Cut,
}

/// The connection task: forwards the pool's binary messages to the script,
/// counts pongs, records the pool's Close frame, and carries out commands.
/// The script dropping its [`MockSocket`] closes with 1000, as a server that finished.
async fn serve(
    mut ws: WebSocketStream<TcpStream>,
    requests: mpsc::Sender<Option<Bytes>>,
    mut commands: UnboundedReceiver<Command>,
    client_close: Arc<Mutex<Option<u16>>>,
    pongs: Arc<AtomicUsize>,
) {
    let mut scripted = true;
    loop {
        tokio::select! {
            message = ws.next() => match message {
                Some(Ok(Message::Binary(body))) => {
                    let _ = requests.send(Some(body));
                }
                Some(Ok(Message::Pong(_))) => {
                    pongs.fetch_add(1, Ordering::SeqCst);
                }
                Some(Ok(Message::Close(frame))) => {
                    *client_close.lock().unwrap() = Some(frame.map_or(NO_STATUS, |frame| u16::from(frame.code)));
                    let _ = requests.send(None);
                }
                Some(Ok(_)) => {}
                None | Some(Err(_)) => {
                    let _ = requests.send(None);
                    return;
                }
            },
            command = commands.recv(), if scripted => match command {
                Some(Command::Send(message)) => {
                    let _ = ws.send(message).await;
                }
                Some(Command::Cut) => return,
                None => {
                    scripted = false;
                    let _ = ws.send(close_message(1000, "")).await;
                }
            },
        }
    }
}

/// RFC 6455's code for a Close frame that carried none.
const NO_STATUS: u16 = 1005;

/// A Close message with `code` and `reason`.
fn close_message(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    }))
}

/// The server side of one session, driven synchronously.
pub struct MockSocket {
    /// Binary messages from the pool; `None` once it closed or dropped the connection.
    requests: mpsc::Receiver<Option<Bytes>>,
    commands: UnboundedSender<Command>,
    /// The upgrade request's headers.
    pub headers: HeaderMap,
    /// The upgrade request's path and query.
    pub path: String,
    read_timeout: Duration,
    opening: Opening,
    /// The code of the pool's Close frame once it sent one ([`NO_STATUS`] without a code).
    client_close: Arc<Mutex<Option<u16>>>,
    pongs: Arc<AtomicUsize>,
}

impl MockSocket {
    /// Blocks for the next request, a `Configure` carrying a first request
    /// split in two (see [`Opening`]); `None` once the pool ended its side.
    pub fn read(&mut self) -> Option<Bytes> {
        if let Some(request) = self.opening.take_pending() {
            return Some(request);
        }
        let message = self.read_unsplit()?;
        Some(self.opening.split(message))
    }

    /// Blocks for the next request message as the pool sent it.
    pub fn read_unsplit(&mut self) -> Option<Bytes> {
        match self.requests.recv_timeout(self.read_timeout) {
            Ok(message) => message,
            Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) => panic!("no request arrived within {:?}", self.read_timeout),
        }
    }

    /// Bounds every later `read`.
    pub fn set_read_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
    }

    /// Sends one event message; a `Configure`'s `Ok` is held for the request's reply.
    pub fn send(&mut self, body: Bytes) -> Result<(), ()> {
        match self.opening.reply(body) {
            Some(body) => self.command(Command::Send(Message::Binary(body))),
            None => Ok(()),
        }
    }

    /// Sends a WebSocket ping frame, which the pool answers while it polls.
    pub fn ping(&self) {
        let _ = self.command(Command::Send(Message::Ping(Bytes::from_static(b"hi"))));
    }

    /// Sends a text message, which the protocol never uses.
    pub fn send_text(&self, text: &str) {
        let _ = self.command(Command::Send(Message::Text(text.into())));
    }

    /// How many pongs the pool has sent to [`Self::ping`]s so far.
    pub fn pongs(&self) -> usize {
        self.pongs.load(Ordering::SeqCst)
    }

    /// Waits for the pool's Close frame and returns its code; panics if none
    /// arrives within [`WAIT_TIMEOUT`].
    pub fn client_close_code(&self) -> u16 {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            if let Some(code) = *self.client_close.lock().unwrap() {
                return code;
            }
            assert!(Instant::now() < deadline, "the pool sent no Close frame");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Ends the session with a Close frame, as a server dropping it by policy.
    pub fn close(self, code: u16, reason: &str) {
        // the task keeps reading until the pool's echo ends the connection
        let _ = self.command(Command::Send(close_message(code, reason)));
    }

    /// Drops the TCP connection without a Close frame.
    pub fn cut(self) {
        let _ = self.command(Command::Cut);
    }

    fn command(&self, command: Command) -> Result<(), ()> {
        self.commands.send(command).map_err(drop)
    }
}
