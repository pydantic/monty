//! Trait + impls for coercing a Python `Value` into a Rust type during
//! argument extraction.
//!
//! Companion to the `#[derive(FromArgs)]` macro in `monty-macros`. The derive
//! generates code that calls [`FromValue::extract_into`] for every positional
//! or keyword argument. The trait owns the cleanup of the input `Value` —
//! constrained impls drop the input, the identity impl for `Value` and the
//! owning [`StrArg`] keep it. Generated callers also need to drop
//! already-extracted owning fields on later error paths; for that they call
//! [`FromValue::drop_extracted`], which knows whether the extracted form holds
//! a heap reference.
//!
//! Failures are structured ([`FromValueFail`]) so the *kind* of failure is
//! reported by the impl rather than re-derived from error messages:
//! `WrongType` lets the extraction site pick CPython's wording, `Raise`
//! surfaces a fully-formed error unchanged.

use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithHeap, HeapData},
    resource::ResourceTracker,
    types::{PyTrait, Type},
    value::Value,
};

/// Error-wording selection for [`FromValue::extract_into`]. Built as a literal
/// by `#[derive(FromArgs)]` at each extraction site to pick how a
/// [`FromValueFail::WrongType`] failure is reported, mirroring CPython's
/// `_PyArg_BadArgument` variants.
pub(crate) enum ArgErrCtx {
    /// No per-site wording: use the impl's [`FromValue::type_error`].
    Plain,
    /// Positional form: `{func_name}() argument {pos} must be {expected}, not {got}`.
    BadArgPos { func_name: &'static str, pos: usize },
    /// Named form: `{func_name}() argument '{arg_name}' must be {expected}, not {got}`.
    BadArgNamed {
        func_name: &'static str,
        arg_name: &'static str,
    },
}

/// A failed [`FromValue`] coercion, split by *why* it failed so extraction
/// sites never have to reverse-engineer the reason from an error message.
pub(crate) enum FromValueFail {
    /// The input value's type is unacceptable; carries the actual type. The
    /// extraction site owns the wording — the per-site `_PyArg_BadArgument`
    /// forms ([`ArgErrCtx`]) or the impl's [`FromValue::type_error`].
    WrongType(Type),
    /// The type was acceptable but the value was not (`ValueError`,
    /// `OverflowError`, resource errors, …). Surfaces unchanged — never
    /// rewritten into a type error.
    Raise(RunError),
}

/// Coerces a `Value` into `Self` during `#[derive(FromArgs)]` argument
/// extraction, consuming the value and handling refcount cleanup on both
/// success and failure paths.
///
/// **Only use `FromValue`-typed fields for functions that are implemented in
/// C in CPython** (builtins and extension modules, where CPython's argument
/// clinic type-checks each argument *while binding* it). Functions that are
/// pure-Python `def`s in CPython (`style = def` structs — the `re` module,
/// `json.dumps`/`loads`, …) must declare fields as raw `Value` and coerce in
/// the function body instead: CPython `def` binding never type-checks, so a
/// coercion failure at extraction time would wrongly preempt later binding
/// errors such as missing arguments or unexpected kwargs.
///
/// Implementations *must* consume the input on every path: either drop it
/// (`drop_with_heap`) once any needed data has been read out, or transfer
/// ownership into `Self` (the identity `Value` impl, [`StrArg`]) — in which
/// case [`drop_extracted`](Self::drop_extracted) must release it.
pub(crate) trait FromValue: Sized {
    /// CPython "must be X" type label used by `_PyArg_BadArgument`-style
    /// error messages ("argument N must be {EXPECTED_TYPE_NAME}, not {Y}").
    ///
    /// `Some("str")`, `Some("int")`, etc. for impls that constrain their input;
    /// `None` for the identity `Value` impl (which accepts any value). Read by
    /// [`extract_into`](Self::extract_into) when the struct sets
    /// [`bad_arg`](../../monty_macros/struct.FromArgs.html) — see
    /// `crates/monty-macros/README.md`.
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
    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail>;

    /// The error for a [`FromValueFail::WrongType`] failure at a site with no
    /// `_PyArg_BadArgument` wording ([`ArgErrCtx::Plain`], `*args` elements).
    ///
    /// The default is a generic `expected {EXPECTED_TYPE_NAME}, not {got}`;
    /// impls whose wrong-type error can actually surface through a `Plain`
    /// site should override it with the exact CPython message their callers
    /// produce (e.g. the int impls' `'{got}' object cannot be interpreted as
    /// an integer`).
    fn type_error(got: Type) -> RunError {
        let expected = Self::EXPECTED_TYPE_NAME.unwrap_or("a different type");
        ExcType::type_error(format!("expected {expected}, not {got}"))
    }

    /// Drop the *extracted* value (i.e. `Self`) so refcounts stay balanced
    /// when generated `from_args` code aborts after extracting one field but
    /// before completing the struct.
    ///
    /// For primitives this is a no-op; for `Value` / [`StrArg`] it decrements
    /// the held reference.
    fn drop_extracted(self, heap: &mut impl ContainsHeap) {
        // Default: no heap references held. Specialise in impls that hold them.
        let _ = heap;
        drop(self);
    }

    /// Coerce `value` into `Self` and store it in `slot`, applying `ctx`'s
    /// CPython `_PyArg_BadArgument` wording to `WrongType` failures.
    ///
    /// This is what `#[derive(FromArgs)]` calls for every positional/keyword
    /// argument. Centralising it here — rather than inlining the equivalent
    /// tokens at each derive's call sites — keeps the generated code small: the
    /// body is monomorphised once per field type and shared across all derives.
    /// On error the input `value` has already been dropped by `from_value`; the
    /// caller remains responsible for draining the argument iterators.
    ///
    /// [`FromValueFail::Raise`] errors surface unchanged, so an impl can
    /// report a failure that is not a type mismatch — e.g. the int impls
    /// raise `OverflowError` for out-of-range ints — without that error being
    /// clobbered into a bogus "must be int, not int". Use `Raise` only for
    /// checks CPython performs *during* argument conversion; validation
    /// CPython does in the function body belongs in the body (see
    /// `NormForm::parse` in `unicodedata.rs`).
    fn extract_into(
        value: Value,
        slot: &mut Option<Self>,
        vm: &mut VM<'_, impl ResourceTracker>,
        ctx: ArgErrCtx,
    ) -> RunResult<()> {
        match Self::from_value(value, vm) {
            Ok(extracted) => {
                *slot = Some(extracted);
                Ok(())
            }
            Err(FromValueFail::Raise(err)) => Err(err),
            // `EXPECTED_TYPE_NAME` is None only for impls that accept any
            // type and therefore never report `WrongType`; falling back to
            // `type_error` keeps that unreachable arm honest without a panic.
            Err(FromValueFail::WrongType(got)) => Err(match (ctx, Self::EXPECTED_TYPE_NAME) {
                (ArgErrCtx::BadArgPos { func_name, pos }, Some(expected)) => {
                    ExcType::type_error_bad_arg_pos(func_name, pos, expected, got.cpython_arg_name())
                }
                (ArgErrCtx::BadArgNamed { func_name, arg_name }, Some(expected)) => {
                    ExcType::type_error_bad_arg_named(func_name, arg_name, expected, got.cpython_arg_name())
                }
                _ => Self::type_error(got),
            }),
        }
    }
}

impl FromValue for Value {
    fn from_value(value: Self, _vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
        Ok(value)
    }

    fn drop_extracted(self, heap: &mut impl ContainsHeap) {
        self.drop_with_heap(heap);
    }
}

impl FromValue for i32 {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("int");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
        let result = match value {
            Value::Bool(b) => Ok(Self::from(b)),
            // Overflow is a *value* failure: the argument is a genuine int,
            // so a bad-arg rewrite ("must be int, not int") would be wrong.
            // CPython's `i` format range-checks with sign-aware wording.
            Value::Int(i) => Self::try_from(i).map_err(|_| {
                let msg = if i < 0 {
                    "signed integer is less than minimum"
                } else {
                    "signed integer is greater than maximum"
                };
                FromValueFail::Raise(SimpleException::new_msg(ExcType::OverflowError, msg).into())
            }),
            _ if is_long_int(&value, vm) => Err(FromValueFail::Raise(ExcType::overflow_c_long())),
            _ => Err(FromValueFail::WrongType(value.py_type_heap(vm.heap))),
        };
        value.drop_with_heap(vm);
        result
    }

    fn type_error(got: Type) -> RunError {
        ExcType::type_error_not_integer(got)
    }
}

impl FromValue for i64 {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("int");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
        let result = match value {
            Value::Bool(b) => Ok(Self::from(b)),
            Value::Int(i) => Ok(i),
            _ if is_long_int(&value, vm) => Err(FromValueFail::Raise(ExcType::overflow_c_long())),
            _ => Err(FromValueFail::WrongType(value.py_type_heap(vm.heap))),
        };
        value.drop_with_heap(vm);
        result
    }

    fn type_error(got: Type) -> RunError {
        ExcType::type_error_not_integer(got)
    }
}

/// True when `value` is a Python int wider than i64 (heap or interned
/// `LongInt`). Such values are genuine ints, so fixed-width int impls
/// must raise `OverflowError` rather than report `WrongType` — a bad-arg
/// rewrite would produce the absurd "must be int, not int". Also used by
/// consumer-specific int impls (e.g. `timedelta`'s component extraction).
pub(crate) fn is_long_int(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::InternLongInt(_) => true,
        Value::Ref(id) => matches!(vm.heap.get(*id), HeapData::LongInt(_)),
        _ => false,
    }
}

impl FromValue for bool {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("bool");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
        let result = match value {
            Value::Bool(b) => Ok(b),
            _ => Err(FromValueFail::WrongType(value.py_type_heap(vm.heap))),
        };
        value.drop_with_heap(vm);
        result
    }

    fn type_error(_got: Type) -> RunError {
        SimpleException::new_msg(ExcType::TypeError, "a bool is required").into()
    }
}

/// A `str` argument extracted without copying: validates the value is a
/// string, then keeps the original `Value` (ownership and refcount
/// transferred in) and lends the text on demand via [`StrArg::as_str`].
///
/// Always prefer this over an owned `String` field — copying the text onto
/// the Rust heap is pure overhead for arguments the function only reads. If
/// ownership is genuinely needed, call `.as_str(vm).to_owned()` at the point
/// of storage.
///
/// Holds a heap reference for heap strings, so extracted fields must be
/// released on every path: bind them with `defer_drop!` in the function body
/// (the macro's error paths use [`FromValue::drop_extracted`]).
pub(crate) struct StrArg(Value);

impl FromValue for StrArg {
    const EXPECTED_TYPE_NAME: Option<&'static str> = Some("str");

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
        if value.is_str(vm.heap) {
            Ok(Self(value))
        } else {
            let got = value.py_type_heap(vm.heap);
            value.drop_with_heap(vm);
            Err(FromValueFail::WrongType(got))
        }
    }

    fn drop_extracted(self, heap: &mut impl ContainsHeap) {
        self.0.drop_with_heap(heap);
    }
}

impl StrArg {
    /// Borrows the text. Infallible: the constructor validated str-ness and a
    /// live value's type cannot change.
    pub fn as_str<'a>(&'a self, vm: &'a VM<'_, impl ResourceTracker>) -> &'a str {
        match self.0.to_str(vm) {
            Ok(s) => s,
            Err(_) => unreachable!("StrArg always holds a str"),
        }
    }
}

impl DropWithHeap for StrArg {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.0.drop_with_heap(heap);
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
    // Forward the inner type's label — `Option<StrArg>` reports "str" in
    // bad-arg errors, matching CPython for fields where absence is signalled
    // by `Option::None` rather than by passing `None`. Functions that accept
    // a literal `None` value (e.g. open()'s `encoding=None`) need
    // `"str or None"` wording and should override at the field level.
    const EXPECTED_TYPE_NAME: Option<&'static str> = T::EXPECTED_TYPE_NAME;

    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
        T::from_value(value, vm).map(Some)
    }

    fn type_error(got: Type) -> RunError {
        T::type_error(got)
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
    fn from_value(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Self, FromValueFail> {
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
