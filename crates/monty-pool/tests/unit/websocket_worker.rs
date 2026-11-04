//! Transport boundaries that cannot be selected deterministically through a pool checkout.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt, stream};
use monty_proto::{FrameError, pb};
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

use super::{Worker, WorkerKind, read_ws_events};

/// A stream ending without an error or Close still terminates the reader and records the missing Close frame.
#[tokio::test]
async fn an_exhausted_stream_reports_disconnect_details() {
    let (tx, mut rx) = mpsc::channel(1);
    let disconnect = Arc::new(OnceLock::new());
    read_ws_events(stream::empty(), tx, Arc::clone(&disconnect)).await;
    assert!(matches!(rx.recv().await, Some(Err(FrameError::Truncated))));
    assert!(rx.recv().await.is_none());
    let details = disconnect.get().expect("EOF evidence");
    assert_eq!(details.stage, "receive");
    assert_eq!(details.close_code, None);
    assert_eq!(details.close_reason, None);
    assert_eq!(
        details.transport_error.as_deref(),
        Some("WebSocket stream ended without a Close frame")
    );
}

/// A failed write after an idle peer Close must not overwrite the original Close or consume its queued error.
#[tokio::test]
async fn a_failed_send_preserves_the_first_failure_and_queued_event() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut socket = accept_async(stream).await.expect("handshake");
        socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "session retired".into(),
            })))
            .await
            .expect("peer Close");
        while socket.next().await.is_some() {}
    });
    let mut worker = Worker::websocket(&format!("ws://{address}"), Duration::from_secs(5), &[])
        .await
        .expect("worker");
    let WorkerKind::WebSocket(websocket) = &mut worker.kind else {
        panic!("WebSocket worker");
    };
    timeout(Duration::from_secs(5), &mut websocket.reader)
        .await
        .expect("reader completion")
        .expect("reader task");
    let error = worker
        .send(&pb::ParentRequest::default())
        .await
        .expect_err("write after peer Close");
    assert!(matches!(error, FrameError::Truncated));
    let details = worker.websocket_disconnect().expect("original evidence");
    assert_eq!(details.stage, "receive");
    assert_eq!(details.close_code, Some(1008));
    assert_eq!(details.close_reason.as_deref(), Some("session retired"));
    assert_eq!(details.transport_error, None);
    let WorkerKind::WebSocket(websocket) = &mut worker.kind else {
        panic!("WebSocket worker");
    };
    assert!(matches!(websocket.events.try_recv(), Ok(Err(FrameError::Truncated))));
    worker.close_transport().await;
    timeout(Duration::from_secs(5), server)
        .await
        .expect("server completion")
        .expect("server task");
}
