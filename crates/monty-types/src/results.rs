//! Host-supplied results fed back into a suspended run:
//! [`NameLookupResult`] and [`ExtFunctionResult`].

use crate::{exceptions::MontyException, object::MontyObject};
/// Result of a name lookup from the host.
///
/// When the VM encounters an unresolved name (or a lazy attribute on a
/// host-backed object), the host provides one of these:
/// - `Value(obj)`: The name resolves to this value (a plain name is cached in
///   its namespace slot; an attribute is re-consulted on every access).
/// - `Undefined`: The name does not exist — `NameError` for a plain name,
///   `AttributeError` for an attribute.
/// - `Error(exc)`: Resolving it raised on the host — the exception is raised
///   inside the sandbox, so `hasattr()` / `getattr()` defaults do not apply.
#[derive(Debug)]
pub enum NameLookupResult {
    /// The name resolves to this value.
    Value(MontyObject),
    /// The name is undefined — the VM raises `NameError` / `AttributeError`.
    Undefined,
    /// Resolving the name raised this exception on the host; the VM raises
    /// it where the lookup suspended, like a failed external call.
    Error(MontyException),
}

impl From<MontyObject> for NameLookupResult {
    fn from(value: MontyObject) -> Self {
        Self::Value(value)
    }
}

impl From<Option<MontyObject>> for NameLookupResult {
    /// `Some` resolves the name, `None` leaves it undefined.
    fn from(value: Option<MontyObject>) -> Self {
        value.map_or(Self::Undefined, Self::Value)
    }
}

impl From<MontyException> for NameLookupResult {
    fn from(exception: MontyException) -> Self {
        Self::Error(exception)
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
