//! Synthetic child for cross-platform supervisor tests. It deliberately has no VM fuel checks.

use std::{hint::black_box, io, thread, time::Duration};

use cpu_time::ThreadTime;
use monty_proto::{FrameReader, WireFunctionCall, pb, write_frame};
use monty_types::{CallArgs, MontyObject, SourceRange};

fn main() {
    let mut reader = FrameReader::new(io::stdin().lock());
    let mut output = io::stdout().lock();
    let mut call_id = 0;
    while let Some(request) = reader.read::<pb::ParentRequest>().unwrap() {
        let kind = request.kind.unwrap();
        let clock = match &kind {
            pb::parent_request::Kind::Configure(config) => config.execution_clock,
            _ => None,
        };
        let event = match kind {
            pb::parent_request::Kind::Feed(feed) if feed.code == "spin" => loop {
                black_box(42);
            },
            pb::parent_request::Kind::Feed(feed) if feed.code == "block" => {
                thread::sleep(Duration::from_secs(60));
                unreachable!("supervisor must terminate the blocked worker");
            }
            pb::parent_request::Kind::Feed(feed) if feed.code == "pause" => pause(&mut call_id),
            pb::parent_request::Kind::ResumeCall(_) => pause(&mut call_id),
            pb::parent_request::Kind::Feed(_) => {
                pb::child_event::Kind::Complete(pb::Complete::from(MontyObject::int(42)))
            }
            pb::parent_request::Kind::Shutdown(_) => return,
            _ => pb::child_event::Kind::Ok(pb::Ok {}),
        };
        write_frame(
            &mut output,
            &pb::ChildEvent {
                kind: Some(event),
                execution_clock: clock,
                ..Default::default()
            },
        )
        .unwrap();
    }
}

fn pause(call_id: &mut u32) -> pb::child_event::Kind {
    let started = ThreadTime::now();
    while started.elapsed() < Duration::from_millis(10) {
        black_box(42);
    }
    *call_id += 1;
    pb::child_event::Kind::FunctionCall(WireFunctionCall::new(
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
