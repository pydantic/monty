//! Host-supplied results fed back into a suspended run:
//! [`NameLookupResult`] and [`ExtFunctionResult`].

use crate::{exceptions::MontyException, object::MontyObject};
/// Result of a name lookup from the host.
///
/// When the VM encounters an unresolved name, the host provides one of these:
/// - `Value(obj)`: The name resolves to this value (cached in the namespace for future access).
/// - `Undefined`: The name is truly undefined, causing `NameError`.
#[derive(Debug)]
pub enum NameLookupResult {
    /// The name resolves to this value.
    Value(MontyObject),
    /// The name is undefined — VM will raise `NameError`.
    Undefined,
}

impl From<MontyObject> for NameLookupResult {
    fn from(value: MontyObject) -> Self {
        Self::Value(value)
    }
}

/// Return value or exception from an external function.
#[derive(Debug)]
pub enum ExtFunctionResult {
    /// Continues execution with the return value from the external function.
    Return(MontyObject),
    /// Continues execution with the exception raised by the external function.
    Error(MontyException),
    /// Pending future — the external function is a coroutine.
    ///
    /// The `u32` is the `call_id` from the `FunctionCall` that created this
    /// snapshot. It is used to track the pending future so it can be resolved
    /// later via `ResolveFutures::resume()`.
    Future(u32),
    /// The function was not found, should result in a `NameError` exception.
    NotFound(String),
}
impl From<MontyObject> for ExtFunctionResult {
    fn from(value: MontyObject) -> Self {
        Self::Return(value)
    }
}

impl From<MontyException> for ExtFunctionResult {
    fn from(exception: MontyException) -> Self {
        Self::Error(exception)
    }
}
