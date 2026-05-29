//! Trait + impls for coercing a Python `Value` into a Rust type during
//! argument extraction.
//!
//! Companion to the `#[derive(FromArgs)]` macro in `monty-macros`. The derive
//! generates code that calls `FromValue::from_value(arg, vm)` for every
//! positional or keyword argument. The trait owns the cleanup of the input
//! `Value` — primitive impls drop the input, the identity impl for `Value`
//! keeps it. Generated callers also need to drop already-extracted owning
//! fields on later error paths; for that they call
//! [`FromValue::drop_extracted`], which knows whether the extracted form holds
//! a heap reference.

use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::ContainsHeap,
    resource::ResourceTracker,
    types::PyTrait,
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
    /// CPython "must be X" type label used by `_PyArg_BadArgument`-style
    /// error messages ("argument N must be {EXPECTED_TYPE_NAME}, not {Y}").
    ///
    /// `Some("str")`, `Some("int")`, etc. for impls that constrain their input;
    /// `None` for the identity `Value` impl (which accepts any value). The
    /// `#[derive(FromArgs)]` macro reads this via the trait so it can emit
    /// CPython-matching errors when [`bad_arg`](../../monty_macros/struct.FromArgs.html)
    /// is set on the struct — see `crates/monty-macros/README.md`.
    ///
    /// Impls that wrap another `FromValue` (e.g. `Option<T>`) forward this
    /// from the inner type by default; override if you need different wording
    /// (e.g. `"str or None"`).
    const EXPECTED_TYPE_NAME: Option<&'static str> = None;

    /// Convert a `Value` into `Self`. On error, the input value must have
    /// been dropped before returning.
    ///
    /// Takes `&mut VM` so impls can both inspect the heap (for type coercion)
    /// and call `drop_with_heap` on the input; this also lets `LaxBool` route
    /// through `PyTrait::py_bool`.
    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self>;

    /// Drop the *extracted* value (i.e. `Self`) so refcounts stay balanced
    /// when generated `from_args` code aborts after extracting one field but
    /// before completing the struct.
    ///
    /// For primitives this is a no-op; for `Value` / `Vec<Value>` it walks
    /// the contents and decrements references.
    fn drop_extracted(self, heap: &mut impl ContainsHeap) {
        // Default: no heap references held. Specialise in impls that hold them.
        let _ = heap;
        drop(self);
    }
}

impl FromValue for Value {
    fn from_value(value: Self, _vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        Ok(value)
    }

    fn drop_extracted(self, heap: &mut impl ContainsHeap) {
        self.drop_with_heap(heap);
    }
}

impl FromValue for i32 {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("int");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = value.to_i32();
        value.drop_with_heap(vm);
        result
    }
}

impl FromValue for i64 {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("int");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = match value {
            Value::Bool(b) => Ok(Self::from(b)),
            Value::Int(i) => Ok(i),
            _ => Err(type_error_integer_required()),
        };
        value.drop_with_heap(vm);
        result
    }
}

/// Accepts `Int` and `Bool`; widens to `i128`. Used by constructors like
/// `timedelta()` that hold their intermediate component values in `i128` so
/// the overflow check on the normalisation step doesn't silently wrap.
impl FromValue for i128 {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("int");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = match value {
            Value::Bool(b) => Ok(Self::from(b)),
            Value::Int(i) => Ok(Self::from(i)),
            _ => Err(type_error_integer_required()),
        };
        value.drop_with_heap(vm);
        result
    }
}

impl FromValue for bool {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("bool");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = match value {
            Value::Bool(b) => Ok(b),
            _ => Err(type_error_bool_required()),
        };
        value.drop_with_heap(vm);
        result
    }
}

impl FromValue for String {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("str");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = match value.as_either_str(vm.heap) {
            Some(either) => Ok(either.into_string(vm.interns)),
            None => Err(type_error_string_required()),
        };
        value.drop_with_heap(vm);
        result
    }
}

/// `Option<T>` is the natural way to spell "absent or present" arguments
/// (e.g. `date.replace(year=…)` where the kwarg's default comes from the
/// receiver, not from a static constant). Paired with `#[from_args(default)]`
/// on the field, an absent argument resolves to `None` and a present one is
/// delegated to `T::from_value` and wrapped in `Some`.
///
/// Note: an explicit `Value::None` passed by the caller is **not** treated as
/// absent — it is forwarded to `T::from_value`, which will normally reject it.
/// This matches CPython: `date.replace(year=None)` is a `TypeError`, not a
/// no-op.
impl<T: FromValue> FromValue for Option<T> {
    // Forward the inner type's label — `Option<String>` reports "str" in
    // bad-arg errors, matching CPython for fields where absence is signalled
    // by `Option::None` rather than by passing `None`. Functions that accept
    // a literal `None` value (e.g. open()'s `encoding=None`) need
    // `"str or None"` wording and should override at the field level.
    const EXPECTED_TYPE_NAME: Option<&'static str> = T::EXPECTED_TYPE_NAME;

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        T::from_value(value, vm).map(Some)
    }

    fn drop_extracted(self, heap: &mut impl ContainsHeap) {
        if let Some(inner) = self {
            inner.drop_extracted(heap);
        }
    }
}

/// Newtype around `bool` for kwargs that perform Python's *truth test* on the
/// incoming value rather than demanding a strict `bool`. CPython spells this
/// pattern via `if flag:` — empty strings/bytes/collections are falsy, every
/// other heap object is truthy.
///
/// Use this for flags like `Path.mkdir(parents=…, exist_ok=…)` where the
/// CPython signature documents a `bool` but the implementation feeds the raw
/// value through `bool()`. Using plain `bool` would reject `mkdir(parents=[])`
/// with a `TypeError`; that does not match CPython, which silently treats the
/// empty list as `False`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LaxBool(bool);

impl FromValue for LaxBool {
    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Self> {
        let result = value.py_bool(vm);
        value.drop_with_heap(vm);
        Ok(Self(result))
    }
}

impl LaxBool {
    pub fn new(b: bool) -> Self {
        Self(b)
    }

    pub fn bool(self) -> bool {
        self.0
    }
}

fn type_error_integer_required() -> RunError {
    // Match the hardcoded message in `Value::to_i32` so callers that
    // mix-and-match the macro and the hand-written extractor see the same
    // error wording. The literal "(got type float)" is a known wart inherited
    // from the original implementation — it is wrong for non-float inputs but
    // matches what callers and tests already expect.
    SimpleException::new_msg(ExcType::TypeError, "an integer is required (got type float)").into()
}

fn type_error_bool_required() -> RunError {
    SimpleException::new_msg(ExcType::TypeError, "a bool is required").into()
}

fn type_error_string_required() -> RunError {
    SimpleException::new_msg(ExcType::TypeError, "a str is required").into()
}
