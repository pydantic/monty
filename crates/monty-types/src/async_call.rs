//! A call to a host coroutine function the sandbox did not suspend on: [`AsyncCall`].

use crate::{exceptions::SourceRange, object::CallArgs};

/// A call to a host function marked `is_async` (see `MontyObject::async_function`).
///
/// The sandbox answers such a call with a pending future itself and runs on,
/// so `asyncio.gather(f(), g())` issues both calls before anything blocks.
/// Each is streamed to the host as it is made, for the host to start at once;
/// the host resolves it by `call_id` through `ResolveFutures::resume` when
/// every sandbox task is blocked. A loaded dump streams every unresolved call
/// again, since the restored session's host has seen none of them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsyncCall {
    /// The name of the function being called.
    pub function_name: String,
    /// The arguments: one arena holding every positional and keyword value.
    pub args: CallArgs,
    /// Unique identifier for this call, which its result names.
    pub call_id: u32,
    /// Where the call expression is in the source.
    pub position: SourceRange,
}
