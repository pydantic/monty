//! Host calls the sandbox awaited without suspending: [`AwaitedCall`].

use crate::{exceptions::SourceRange, object::CallArgs, os::OsFunctionCall, uuid::MontyUuid};

/// A host call whose result the sandbox `await`s as soon as it is made.
///
/// The interpreter answers such a call with a pending future itself and runs
/// on, so `asyncio.gather(f(), g())` issues both calls before it suspends.
/// They reach the host in the next `ResolveFutures`, which lists them once per
/// announcement; a loaded dump announces every unresolved call again. The
/// host resolves each by `call_id` through `ResolveFutures::resume`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AwaitedCall {
    /// An external function or host-object method call; see `FunctionCall`.
    Function {
        /// The name of the function or method being called.
        function_name: String,
        /// The arguments: one arena holding every positional and keyword value.
        args: CallArgs,
        /// Unique identifier for this call.
        call_id: u32,
        /// Uuid of the routed receiver; `None` for plain external calls.
        object_id: Option<MontyUuid>,
        /// Where the call expression is in the source.
        position: SourceRange,
    },
    /// An OS call a future may answer (`asyncio.sleep`); see `OsCall`.
    Os {
        /// Typed OS-call dispatch value (variant + args).
        function_call: OsFunctionCall,
        /// Unique identifier for this call.
        call_id: u32,
        /// Where the call expression is in the source.
        position: SourceRange,
    },
}

impl AwaitedCall {
    /// The call's identifier, which its result names.
    #[must_use]
    pub fn call_id(&self) -> u32 {
        match self {
            Self::Function { call_id, .. } | Self::Os { call_id, .. } => *call_id,
        }
    }

    /// Where the call expression is in the source.
    #[must_use]
    pub fn position(&self) -> &SourceRange {
        match self {
            Self::Function { position, .. } | Self::Os { position, .. } => position,
        }
    }
}
