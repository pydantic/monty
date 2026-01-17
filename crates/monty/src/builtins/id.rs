//! Implementation of the id() builtin function.

use crate::{args::ArgValues, exception_private::RunResult, heap::Heap, resource::ResourceTracker, value::Value};

/// Implementation of the id() builtin function.
///
/// Returns the identity of an object (unique integer for the object's lifetime).
pub fn builtin_id(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("id", heap)?;
    let id = value.id();
    // For heap values, we intentionally don't drop to prevent heap slot reuse
    // which would cause id([]) == id([]) to return True (same slot reused).
    // For immediate values, dropping is a no-op since they don't use heap slots.
    // This is an acceptable trade-off: small leak for heap values passed to id(),
    // but correct semantics for value identity.
    if matches!(value, Value::Ref(_)) {
        #[cfg(feature = "ref-count-panic")]
        std::mem::forget(value);
    } else {
        value.drop_with_heap(heap);
    }
    // Python's id() returns a signed integer; reinterpret bits for large values
    // On 64-bit: large addresses wrap to negative; on 32-bit: always fits positive
    #[expect(
        clippy::cast_possible_wrap,
        reason = "Python id() returns signed; wrapping intentional"
    )]
    let id_i64 = id as i64;
    Ok(Value::Int(id_i64))
}
