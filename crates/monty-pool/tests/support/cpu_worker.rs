//! Synthetic worker for the Linux CPU supervisor tests. It deliberately has no VM fuel checks.

use std::{hint::black_box, io, thread, time::Duration};

use cpu_time::ThreadTime;
use monty_proto::{FrameReader, PROTOCOL_VERSION, WireFunctionCall, os_call_to_proto, pb, write_frame};
use monty_types::{CallArgs, IMPORT_STUBS_CALL_ID, MontyObject, OsFunctionCall, SourceRange};

fn main() {
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut output = io::stdout().lock();
    let mut call_id = 0;
    while let Some(request) = reader.read::<pb::ClientRequest>().unwrap() {
        let mut kind = request.kind.unwrap();
        let clock = match &kind {
            pb::client_request::Kind::Configure(config) if config.script_name == "legacy" => {
                assert_eq!(
                    config.protocol_version, PROTOCOL_VERSION,
                    "legacy worker rejects newer protocol versions"
                );
                None
            }
            pb::client_request::Kind::Configure(config) => config.execution_clock,
            _ => None,
        };
        // a `Configure` carrying the session's first request is answered by that request
        if let pb::client_request::Kind::Configure(config) = &mut kind {
            match config.first.take() {
                Some(pb::configure::First::Feed(feed)) => kind = pb::client_request::Kind::Feed(feed),
                Some(pb::configure::First::Load(load)) => kind = pb::client_request::Kind::Load(load),
                None => {}
            }
        }
        let event = match kind {
            pb::client_request::Kind::Feed(feed) if feed.code == "exit" => return,
            pb::client_request::Kind::Feed(feed) if feed.code == "fatal" => {
                // a dying worker's last gasp reports no clock
                let fatal = pb::WorkerEvent {
                    kind: Some(pb::worker_event::Kind::FatalError(pb::FatalError {
                        message: "synthetic fatal error".to_owned(),
                    })),
                    ..Default::default()
                };
                write_frame(&mut output, &fatal).unwrap();
                return;
            }
            pb::client_request::Kind::Feed(feed) if feed.code == "spin" => loop {
                black_box(42);
            },
            pb::client_request::Kind::Feed(feed) if feed.code == "block" => {
                thread::sleep(Duration::from_secs(60));
                unreachable!("supervisor must terminate the blocked worker");
            }
            pb::client_request::Kind::Feed(feed) if feed.code == "pause" => pause(&mut call_id),
            pb::client_request::Kind::Feed(feed) if feed.code == "typing-error" => {
                pb::worker_event::Kind::OsCall(os_call_to_proto(
                    IMPORT_STUBS_CALL_ID,
                    OsFunctionCall::ImportStubs(vec!["tools".to_owned()]),
                    false,
                    &SourceRange::new("supervision.py", 0, 1),
                ))
            }
            pb::client_request::Kind::ResumeCall(resume) if resume.call_id == IMPORT_STUBS_CALL_ID => {
                pb::worker_event::Kind::TypingError(pb::TypingError {
                    diagnostics: "synthetic typing rejection".to_owned(),
                })
            }
            pb::client_request::Kind::ResumeCall(_) => pause(&mut call_id),
            pb::client_request::Kind::Feed(_) => {
                pb::worker_event::Kind::Complete(pb::Complete::from(MontyObject::int(42)))
            }
            pb::client_request::Kind::Shutdown(_) => return,
            _ => pb::worker_event::Kind::Ok(pb::Ok {}),
        };
        write_frame(
            &mut output,
            &pb::WorkerEvent {
                feed_suspended: matches!(event, pb::worker_event::Kind::TypingError(_)).then_some(false),
                kind: Some(event),
                execution_clock: clock,
                ..Default::default()
            },
        )
        .unwrap();
    }
}

fn pause(call_id: &mut u32) -> pb::worker_event::Kind {
    let started = ThreadTime::now();
    while started.elapsed() < Duration::from_millis(10) {
        black_box(42);
    }
    *call_id += 1;
    pb::worker_event::Kind::FunctionCall(WireFunctionCall::new(
        "pause".to_owned(),
        CallArgs::from(vec![]),
        *call_id,
        None,
        false,
        SourceRange {
            filename: "supervision.py".to_owned(),
            start: 0,
            end: 1,
        },
    ))
}
