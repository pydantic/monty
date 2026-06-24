//! Drives the CPython child over an in-memory transport that plays the parent:
//! it scripts a session (Configure → Feeds → Shutdown) and answers each
//! `FunctionCall` the child emits from a small external-function table, exactly
//! as a real parent would.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use monty::{ExtFunctionResult, MontyObject};
use monty_cpython::{
    run_with_transport,
    transport::{Incoming, SendError, Transport},
};
use monty_proto::pb;

type External = Box<dyn Fn(&[MontyObject]) -> ExtFunctionResult>;

/// An in-memory parent: replays a request script and answers `FunctionCall`s
/// from `externals`, capturing every event the child sends for assertions.
struct ScriptedParent {
    script: VecDeque<pb::ParentRequest>,
    pending_resume: Option<pb::ParentRequest>,
    externals: HashMap<String, External>,
    events: Rc<RefCell<Vec<pb::ChildEvent>>>,
}

impl Transport for ScriptedParent {
    fn recv(&mut self) -> Incoming {
        if let Some(resume) = self.pending_resume.take() {
            return Incoming::Request(resume);
        }
        match self.script.pop_front() {
            Some(request) => Incoming::Request(request),
            None => Incoming::Eof,
        }
    }

    fn send(&mut self, event: &pb::ChildEvent) -> Result<(), SendError> {
        self.events.borrow_mut().push(event.clone());
        // Mirror a real parent: a FunctionCall is answered with a ResumeCall.
        if let Some(pb::child_event::Kind::FunctionCall(call)) = &event.kind {
            let result = match self.externals.get(&call.function_name) {
                Some(handler) => handler(&call.args),
                None => ExtFunctionResult::NotFound(call.function_name.clone()),
            };
            self.pending_resume = Some(resume_call(call.call_id, result));
        }
        Ok(())
    }
}

#[test]
fn drives_a_full_session() {
    let externals: HashMap<String, External> = HashMap::from([(
        "double".to_string(),
        Box::new(|args: &[MontyObject]| match args {
            [MontyObject::Int(n)] => ExtFunctionResult::Return(MontyObject::Int(n * 2)),
            _ => ExtFunctionResult::NotFound("double".to_string()),
        }) as External,
    )]);

    let events = Rc::new(RefCell::new(Vec::new()));
    let parent = ScriptedParent {
        script: VecDeque::from([
            configure(),
            feed("double(21) + 1"),    // host call: 21 -> 42, + 1 -> 43
            feed("print('hello')\n7"), // streamed print, then trailing value
            feed("1 / 0"),             // raises, ends the turn with Error
            shutdown(),
        ]),
        pending_resume: None,
        externals,
        events: events.clone(),
    };

    // Runs to the scripted Shutdown and returns (exit code is not asserted —
    // `ExitCode` is opaque; the captured events below prove the behavior).
    let _ = run_with_transport(Box::new(parent));

    let events = events.borrow();
    let kinds: Vec<_> = events.iter().filter_map(|e| e.kind.as_ref()).collect();

    // First and last turn-enders are the Configure / Shutdown acks.
    assert!(
        matches!(kinds.first(), Some(pb::child_event::Kind::Ok(_))),
        "first event is Ok"
    );
    assert!(
        matches!(kinds.last(), Some(pb::child_event::Kind::Ok(_))),
        "last event is Ok"
    );

    // The host call was emitted with the converted argument.
    let call = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::FunctionCall(c) => Some(c),
            _ => None,
        })
        .expect("a FunctionCall event");
    assert_eq!(call.function_name, "double");
    assert_eq!(call.args, vec![MontyObject::Int(21)]);

    // Both feeds completed; collect the Complete values in order.
    let completes: Vec<MontyObject> = kinds
        .iter()
        .filter_map(|k| match k {
            pb::child_event::Kind::Complete(c) => Some(c.value.clone().unwrap().into_object().unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(completes, vec![MontyObject::Int(43), MontyObject::Int(7)]);

    // The print streamed through as a stdout event.
    let printed: String = kinds
        .iter()
        .filter_map(|k| match k {
            pb::child_event::Kind::Print(p) => Some(p.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(printed, "hello\n");

    // The dividing-by-zero feed ended with a ZeroDivisionError.
    let error = kinds
        .iter()
        .find_map(|k| match k {
            pb::child_event::Kind::Error(e) => e.exception.as_ref(),
            _ => None,
        })
        .expect("an Error event");
    assert_eq!(error.exc_type, "ZeroDivisionError");
}

fn configure() -> pb::ParentRequest {
    request(pb::parent_request::Kind::Configure(pb::Configure {
        monty_version: env!("CARGO_PKG_VERSION").to_string(),
        ..Default::default()
    }))
}

fn feed(code: &str) -> pb::ParentRequest {
    request(pb::parent_request::Kind::Feed(pb::Feed {
        code: code.to_string(),
        ..Default::default()
    }))
}

fn shutdown() -> pb::ParentRequest {
    request(pb::parent_request::Kind::Shutdown(pb::Shutdown {}))
}

fn resume_call(call_id: u32, result: ExtFunctionResult) -> pb::ParentRequest {
    request(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
        call_id,
        result: Some(result.into()),
    }))
}

fn request(kind: pb::parent_request::Kind) -> pb::ParentRequest {
    pb::ParentRequest { kind: Some(kind) }
}
