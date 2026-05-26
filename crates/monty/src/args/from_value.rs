//! Trait + impls for coercing a Python `Value` into a Rust type during
//! argument extraction.
//!
//! Companion to the `#[derive(FromArgs)]` macro in `monty-macros`. The derive
//! generates code that calls `FromValue::from_value(arg, heap, interns)` for
//! every positional or keyword argument. The trait owns the cleanup of the
//! input `Value` — primitive impls drop the input, the identity impl for
//! `Value` keeps it. Generated callers also need to drop already-extracted
//! owning fields on later error paths; for that they call
//! [`FromValue::drop_extracted`], which knows whether the extracted form holds
//! a heap reference.

use crate::{
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::Heap,
    intern::Interns,
    resource::ResourceTracker,
    value::Value,
};

/// Coerces a `Value` into `Self`, consuming the value and handling refcount
/// cleanup on both success and failure paths.
///
/// Implementations *must* call `drop_with_heap` on the input value once any
/// heap-allocated data has been extracted (typically: read out a primitive or
/// `String`, then drop). The identity impl for `Value` is the only exception:
/// it transfers ownership of the value into `Self` instead of dropping it.
pub(crate) trait FromValue: Sized {
    /// Convert a `Value` into `Self`. On error, the input value must have
    /// been dropped before returning.
    fn from_value(value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self>;

    /// Drop the *extracted* value (i.e. `Self`) so refcounts stay balanced
    /// when generated `from_args` code aborts after extracting one field but
    /// before completing the struct.
    ///
    /// For primitives this is a no-op; for `Value` / `Vec<Value>` it walks
    /// the contents and decrements references.
    fn drop_extracted(self, heap: &mut Heap<impl ResourceTracker>) {
        // Default: no heap references held. Specialise in impls that hold them.
        let _ = heap;
        drop(self);
    }
}

impl FromValue for Value {
    fn from_value(value: Self, _heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Self> {
        Ok(value)
    }

    fn drop_extracted(self, heap: &mut Heap<impl ResourceTracker>) {
        self.drop_with_heap(heap);
    }
}

impl FromValue for i32 {
    fn from_value(value: Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Self> {
        let result = value.to_i32();
        value.drop_with_heap(heap);
        result
    }
}

impl FromValue for i64 {
    fn from_value(value: Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Self> {
        let result = match value {
            Value::Bool(b) => Ok(Self::from(b)),
            Value::Int(i) => Ok(i),
            _ => Err(type_error_integer_required()),
        };
        value.drop_with_heap(heap);
        result
    }
}

impl FromValue for bool {
    fn from_value(value: Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Self> {
        let result = match value {
            Value::Bool(b) => Ok(b),
            _ => Err(type_error_bool_required()),
        };
        value.drop_with_heap(heap);
        result
    }
}

impl FromValue for String {
    fn from_value(value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        let result = match value.as_either_str(heap) {
            Some(either) => Ok(either.as_str(interns).to_owned()),
            None => Err(type_error_string_required()),
        };
        value.drop_with_heap(heap);
        result
    }
}

/// `Option<T>` — `None` only when the input value is `Value::None`. Distinct
/// from "argument absent" (handled at the struct level via `default`).
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Self> {
        if matches!(value, Value::None) {
            // Value::None is an immediate variant — no heap ref to release.
            Ok(None)
        } else {
            T::from_value(value, heap, interns).map(Some)
        }
    }

    fn drop_extracted(self, heap: &mut Heap<impl ResourceTracker>) {
        if let Some(inner) = self {
            inner.drop_extracted(heap);
        }
    }
}

fn type_error_integer_required() -> RunError {
    SimpleException::new_msg(ExcType::TypeError, "an integer is required").into()
}

fn type_error_bool_required() -> RunError {
    SimpleException::new_msg(ExcType::TypeError, "a bool is required").into()
}

fn type_error_string_required() -> RunError {
    SimpleException::new_msg(ExcType::TypeError, "a str is required").into()
}
