//! `wire_protocol` — encode/decode the Monty subprocess wire protocol from
//! Python, so a sandbox can be driven over any transport (WebSocket, HTTP, a
//! raw socket, a Docker `exec` pipe) instead of only a local subprocess pipe.
//!
//! The crate has two roles:
//!
//! - **Python extension (`cdylib`)** — exposes the four codec functions
//!   ([`wire::encode_parent_request`] etc.) and the message classes mirroring
//!   the protocol's `ParentRequest` / `ChildEvent` oneof arms. Values cross the
//!   boundary as native Python objects via the [`convert`] layer.
//! - **Rust library (`rlib`)** — the Python ↔ `MontyObject` value-conversion
//!   layer ([`convert`], [`dataclass`], [`exceptions`]) lives here and is
//!   linked directly by `monty-python`, which used to own it. Keeping a single
//!   copy avoids two diverging conversions of the same value model.
//!
//! Decoding is deliberately fallible: a frame arriving over a network comes
//! from an untrusted peer, so it validates everything (value depth, enum names,
//! date ranges, the per-call decode budget) exactly as the in-process
//! subprocess protocol does. See `websocket_plan.md` for transport recipes and
//! the protocol footguns.

pub mod convert;
pub mod dataclass;
pub mod events;
pub mod exceptions;
pub mod messages;
pub mod requests;
pub mod wire;

use std::sync::OnceLock;

use pyo3::prelude::*;

/// This build's version, normalized toward PEP 440 (cargo's `-alpha1` →
/// `a1`). Exposed as `__version__` and used as the default `StartSession`
/// `monty_version`, which the child rejects on mismatch.
pub(crate) fn get_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| env!("CARGO_PKG_VERSION").replace("-alpha", "a").replace("-beta", "b"))
}

/// The `_wire_protocol` extension module (re-exported by the `wire_protocol`
/// Python package).
#[pymodule]
mod _wire_protocol {
    use pyo3::prelude::*;

    use super::get_version;
    #[pymodule_export]
    use crate::{
        convert::PyMontyFileHandle as MontyFileHandle,
        events::{
            Complete, DumpResult, Error, FatalError, FunctionCall, NameLookup, OkEvent, OsCall, Print, ResolveFutures,
            TypingError,
        },
        messages::{ExtFunctionResult, FutureResult, Mount, RaisedException, WireStackFrame},
        requests::{Dump, Feed, Load, Reset, ResumeCall, ResumeFutures, ResumeNameLookup, Shutdown, StartSession},
        wire::{decode_child_event, decode_parent_request, encode_child_event, encode_parent_request},
    };

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("__version__", get_version())?;
        Ok(())
    }
}
