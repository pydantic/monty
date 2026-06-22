//! The four codec functions, plus the encode-side dispatch that turns a Python
//! `ParentRequest` / `ChildEvent` object into its `pb` envelope.
//!
//! Decoding goes through `pb::*::decode`, which validates everything (value
//! depth, enum names, date ranges) because the bytes come from an untrusted
//! peer. Each decode resets the per-call host-memory budget
//! ([`monty_proto::reset_decode_budget`]) — this codec does its own framing via
//! the transport, so it cannot rely on `FrameReader` to reset it.

use monty_proto::pb;
use prost::Message;
use pyo3::{exceptions::PyTypeError, prelude::*, types::PyBytes};

use crate::{events, messages::proto_err, requests};

/// Encodes a `ParentRequest` (a `StartSession`/`Feed`/`Resume*`/... object) to the
/// raw protobuf bytes a client sends to the sandbox. No length prefix — the
/// transport frames the message (a WebSocket already does; a byte stream needs
/// its own 4-byte length prefix).
#[pyfunction]
pub fn encode_parent_request<'py>(py: Python<'py>, request: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let kind = parent_request_kind(request)?;
    let message = pb::ParentRequest { kind: Some(kind) };
    Ok(PyBytes::new(py, &message.encode_to_vec()))
}

/// Decodes the bytes of a `ParentRequest` (server side: a request from a
/// client) into the matching message object.
#[pyfunction]
pub fn decode_parent_request(py: Python<'_>, data: &[u8]) -> PyResult<Py<PyAny>> {
    monty_proto::reset_decode_budget();
    let request = pb::ParentRequest::decode(data).map_err(proto_err)?;
    let kind = request.kind.ok_or_else(|| proto_err("ParentRequest has no kind"))?;
    requests::request_from_proto(py, kind)
}

/// Encodes a `ChildEvent` (a `Print`/`FunctionCall`/`Complete`/... object) to
/// the raw protobuf bytes a server sends back to the client.
#[pyfunction]
pub fn encode_child_event<'py>(py: Python<'py>, event: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let message = child_event_message(event)?;
    Ok(PyBytes::new(py, &message.encode_to_vec()))
}

/// Decodes the bytes of a `ChildEvent` (client side: an event/response from the
/// sandbox) into the matching message object.
#[pyfunction]
pub fn decode_child_event(py: Python<'_>, data: &[u8]) -> PyResult<Py<PyAny>> {
    monty_proto::reset_decode_budget();
    let event = pb::ChildEvent::decode(data).map_err(proto_err)?;
    events::event_from_proto(py, event)
}

/// Downcasts a Python object to the matching `ParentRequest` arm and builds its
/// `pb::parent_request::Kind`. Returns `TypeError` for anything else.
fn parent_request_kind(request: &Bound<'_, PyAny>) -> PyResult<pb::parent_request::Kind> {
    if let Ok(r) = request.cast::<requests::StartSession>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::Feed>() {
        r.get().to_kind()
    } else if let Ok(r) = request.cast::<requests::ResumeCall>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::ResumeNameLookup>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::ResumeFutures>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::Dump>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::Load>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::Reset>() {
        Ok(r.get().to_kind())
    } else if let Ok(r) = request.cast::<requests::Shutdown>() {
        Ok(r.get().to_kind())
    } else {
        Err(PyTypeError::new_err(format!(
            "expected a ParentRequest (StartSession, Feed, ResumeCall, ...), got {}",
            request.get_type().name()?
        )))
    }
}

/// Downcasts a Python object to the matching `ChildEvent` arm and builds its
/// `pb::ChildEvent`. Returns `TypeError` for anything else.
fn child_event_message(event: &Bound<'_, PyAny>) -> PyResult<pb::ChildEvent> {
    if let Ok(e) = event.cast::<events::Print>() {
        e.get().to_event()
    } else if let Ok(e) = event.cast::<events::FunctionCall>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::OsCall>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::NameLookup>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::ResolveFutures>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::Complete>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::Error>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::TypingError>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::DumpResult>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::OkEvent>() {
        Ok(e.get().to_event())
    } else if let Ok(e) = event.cast::<events::FatalError>() {
        Ok(e.get().to_event())
    } else {
        Err(PyTypeError::new_err(format!(
            "expected a ChildEvent (Print, FunctionCall, Complete, ...), got {}",
            event.get_type().name()?
        )))
    }
}
