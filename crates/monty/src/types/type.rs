//! Behaviour shared by every native [`Type`]: naming, construction and
//! class-level attribute access, driven by the [`TypeDef`](super::native::TypeDef) table.
//!
//! The enum itself and the per-type facts live in [`super::native`]; this
//! module holds the generic code that reads them, plus the constructors of the
//! primitive types that have no module of their own.

use std::fmt;

use num_bigint::BigInt;

use super::native::Constructor;
pub use super::native::Type;
use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropWithContext, Heap},
    intern::StaticStrings,
    py_value::PyValue,
    types::{
        GenericAlias, LongInt, PyTrait, allocate_tuple,
        bytes::bytes_repr,
        instance::BoundMethod,
        long_int::INT_MAX_STR_DIGITS,
        str::{StringRepr, allocate_string},
    },
    value::{EitherStr, Value},
};

impl Type {
    /// The name CPython's `__name__` reports: [`name`](Self::name) with any
    /// module qualifier stripped (`datetime.date` → `date`). CPython keeps one
    /// dotted `tp_name` per C type and derives the bare `__name__` from it, so
    /// reprs and error messages qualify where `__name__` does not.
    #[must_use]
    pub(crate) fn dunder_name(self) -> &'static str {
        let name = self.name();
        name.rsplit_once('.').map_or(name, |(_, bare)| bare)
    }

    /// [`name`](Self::name) as rendered by CPython's `_PyArg_BadArgument`
    /// ("argument N must be X, not Y") error formatter: identical except that
    /// `NoneType` renders as `"None"`, since CPython special-cases `Py_None`.
    /// Use for the "not Y" half of arg-type error messages only.
    #[must_use]
    pub(crate) fn cpython_arg_name(self) -> &'static str {
        if self == Self::NoneType { "None" } else { self.name() }
    }

    /// Whether subscripting this type builds a `types.GenericAlias` (`list[int]`).
    #[must_use]
    pub(crate) fn has_class_getitem(self) -> bool {
        self.def().class_getitem
    }

    /// Whether multiplication can fall back to CPython's sequence-repetition protocol.
    #[must_use]
    pub(crate) fn has_sequence_repeat(self) -> bool {
        self.def().sequence_repeat
    }

    /// Returns whether this is one of Python's concrete iterator types.
    #[must_use]
    pub(crate) fn is_iterator(self) -> bool {
        self.def().iterator
    }

    /// Converts a callable type to a u8 for the `CallBuiltinType` opcode.
    ///
    /// The values are bytecode operands persisted in dumps, so this is an
    /// explicit table rather than the discriminant; `None` for other types.
    #[must_use]
    pub fn callable_to_u8(self) -> Option<u8> {
        CALLABLE_TYPES
            .iter()
            .position(|ty| *ty == self)
            .map(|index| u8::try_from(index).expect("CALLABLE_TYPES fits in u8"))
    }

    /// Converts a u8 back to a callable `Type` for the `CallBuiltinType` opcode.
    #[must_use]
    pub fn callable_from_u8(id: u8) -> Option<Self> {
        CALLABLE_TYPES.get(usize::from(id)).copied()
    }

    /// Dispatches calls on a type object's attribute (`dict.fromkeys(...)`).
    ///
    /// A type's own class methods run first, then `__class_getitem__`; any
    /// other name is looked up as a plain attribute and the result called, so
    /// `list.__name__()` raises `'str' object is not callable`.
    pub(crate) fn call_class_method(self, attr: &EitherStr, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
        let args = match self.def().class_method {
            Some(class_method) => match class_method(attr, args, vm) {
                Ok(result) => return result,
                Err(args) => args,
            },
            None => args,
        };
        if self.has_class_getitem() && attr.static_string(vm.interns) == Some(StaticStrings::ClassGetitem) {
            // `list.__class_getitem__(int)` is `list[int]`; the error names the
            // bare type as CPython does (`deque.__class_getitem__()`).
            let name = format!("{}.__class_getitem__", self.dunder_name());
            let key = args.get_one_arg(&name, vm.heap)?;
            Ok(CallResult::Value(GenericAlias::subscript(self, key, vm)))
        } else if let Some(value) = self.class_getattr(attr, vm) {
            defer_drop!(value, vm);
            vm.call_function(value, args)
        } else {
            args.drop_with(vm.heap);
            Err(self.attribute_error(attr, vm))
        }
    }

    /// Resolves an attribute read on a type object itself — `list.__name__`,
    /// `bool.__mro__`, `date.max`, `chain.from_iterable` — returning `None` when
    /// there is no such member so the caller raises [`Self::attribute_error`].
    pub(crate) fn class_getattr(self, attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
        let is_dunder_name = attr.static_string(vm.interns).map_or_else(
            || attr.as_str(vm.interns) == "__name__",
            |ss| ss == StaticStrings::DunderName,
        );
        if is_dunder_name {
            return Some(allocate_string(self.dunder_name(), vm.heap));
        }
        let type_value = |ty: Self| Value::Builtin(Builtins::Type(ty));
        Some(match attr.as_str(vm.interns) {
            "__bases__" => allocate_tuple(self.def().bases.iter().copied().map(type_value).collect(), vm.heap),
            "__mro__" => allocate_tuple(self.mro().map(type_value).collect(), vm.heap),
            // `type.mro` is the method itself; any other type's is bound to that type.
            "__class__" => type_value(Self::Type),
            "mro" if self == Self::Type => Value::Builtin(Builtins::Function(BuiltinsFunctions::TypeMro)),
            "mro" => BoundMethod::allocate(
                type_value(self),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::TypeMro)),
                vm.heap,
            ),
            "__init_subclass__" => bind_init_subclass(type_value(self), self.class_constant(attr, vm)?, vm.heap),
            _ => self.class_constant(attr, vm)?,
        })
    }

    /// The class-level members of the type or a native ancestor (`time.max`,
    /// `timezone.utc`, `object.__init__`), which instances also see; `None`
    /// when no such member exists.
    ///
    /// Every lookup allocates a fresh object, so `date.min is date.min` is
    /// `False` where CPython caches (see limitations/datetime.md).
    pub(crate) fn class_constant(self, attr: &EitherStr, vm: &mut VM<'_>) -> Option<Value> {
        // Native ancestors' members are inherited too: `int.__init__` is `object`'s.
        self.mro()
            .find_map(|ty| ty.def().class_attr.and_then(|class_attr| class_attr(attr, vm)))
    }

    /// The `AttributeError` for a name missing from this type object.
    ///
    /// CPython names the class rather than the metaclass here:
    /// `type object 'list' has no attribute 'nonexistent'`.
    pub(crate) fn attribute_error(self, attr: &EitherStr, vm: &VM<'_>) -> RunError {
        ExcType::attribute_error_type(self.name(), attr.as_str(vm.interns))
    }

    /// Calls this type as a constructor (e.g., `list(x)`, `int(x)`).
    ///
    /// Iterator acquisition may suspend in user code; other constructors complete natively.
    pub(crate) fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        match self.def().ctor {
            Constructor::Value(new) => new(vm, args).map(CallResult::Value),
            Constructor::Call(new) => new(vm, args),
            Constructor::Typed(new) => new(self, vm, args).map(CallResult::Value),
            Constructor::None => {
                args.drop_with(vm.heap);
                Err(ExcType::type_error_not_callable(self.name()))
            }
        }
    }
}

/// `cls.__init_subclass__` is an implicit classmethod, so reading it binds the class.
pub(crate) fn bind_init_subclass(class: Value, hook: Value, heap: &Heap) -> Value {
    BoundMethod::allocate(class, hook, heap)
}

/// The types the `CallBuiltinType` opcode can name, indexed by operand.
///
/// Append-only: the index is a bytecode operand persisted in dumps.
const CALLABLE_TYPES: [Type; 15] = [
    Type::Bool,
    Type::Int,
    Type::Float,
    Type::Str,
    Type::Bytes,
    Type::List,
    Type::Tuple,
    Type::Dict,
    Type::Set,
    Type::FrozenSet,
    Type::Range,
    Type::Slice,
    Type::Iterator,
    Type::Path,
    Type::Complex,
];

/// `float(x=0.0, /)`: numeric coercion, then str/bytes parsing.
pub(crate) fn float_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let Some(v) = args.get_zero_one_arg("float", vm.heap)? else {
        return Ok(Value::Float(0.0));
    };
    defer_drop!(v, vm);
    if let Some(value) = v.py_float(vm)? {
        Ok(Value::Float(value))
    } else {
        match v.py_value(vm) {
            PyValue::Str(value) => Ok(Value::Float(parse_f64_from_str(value.as_str(vm))?)),
            PyValue::Bytes(value) => Ok(Value::Float(parse_f64_from_bytes(value.as_slice(vm))?)),
            _ => Err(ExcType::type_error_float_conversion(&v.py_type_name(vm))),
        }
    }
}

/// `bool(x=False, /)`: the truth value of `x`.
pub(crate) fn bool_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let Some(v) = args.get_zero_one_arg("bool", vm.heap)? else {
        return Ok(Value::Bool(false));
    };
    defer_drop!(v, vm);
    Ok(Value::Bool(v.py_bool(vm)?))
}

/// `typing.Union(...)`: CPython words this differently from other uncallable types.
pub(crate) fn union_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    args.drop_with(vm);
    Err(ExcType::type_error("cannot create 'typing.Union' instances"))
}

/// Parses a Python `float()` string argument into an `f64`.
///
/// This supports:
/// - Leading/trailing whitespace (e.g. `"  1.5  "`)
/// - The special values `inf`, `-inf`, `infinity`, and `nan` (case-insensitive)
/// - Underscore digit separators, strictly between two digits (`1_000.5`, `1e1_0`)
pub(crate) fn parse_f64_from_str(value: &str) -> RunResult<f64> {
    parse_f64(value).ok_or_else(|| value_error_could_not_convert_string_to_float(StringRepr(value)))
}

/// Parses ASCII bytes without accepting Unicode digits or whitespace encoded as UTF-8.
fn parse_f64_from_bytes(value: &[u8]) -> RunResult<f64> {
    str::from_utf8(value)
        .ok()
        .filter(|text| text.is_ascii())
        .and_then(parse_f64)
        .ok_or_else(|| value_error_could_not_convert_string_to_float(bytes_repr(value)))
}

/// Parses shared float syntax without constructing the caller-specific error.
fn parse_f64(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let stripped;
    let trimmed = if trimmed.contains('_') {
        stripped = strip_underscores(trimmed)?;
        stripped.as_str()
    } else {
        trimmed
    };

    let lower = trimmed.to_ascii_lowercase();
    let parsed = match lower.as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => f64::INFINITY,
        "-inf" | "-infinity" => f64::NEG_INFINITY,
        "nan" | "+nan" => f64::NAN,
        "-nan" => -f64::NAN,
        _ => trimmed.parse::<f64>().ok()?,
    };

    Some(parsed)
}

/// Removes digit-separator underscores, or returns `None` when one is not
/// strictly between two digits (CPython's `_Py_string_to_number_with_underscores`).
pub(crate) fn strip_underscores(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        if c == '_' {
            let before = i.checked_sub(1).map(|j| bytes[j]);
            let after = bytes.get(i + 1);
            if !(before.is_some_and(|b| b.is_ascii_digit()) && after.is_some_and(u8::is_ascii_digit)) {
                return None;
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Creates a `float()` ValueError using the original str/bytes argument's Python representation.
fn value_error_could_not_convert_string_to_float(value: impl fmt::Display) -> RunError {
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("could not convert string to float: {value}"),
    )
    .into()
}

/// Argument shape for `int(x=..., /, base=...)`. `x` is positional-only with
/// no kwarg id, so `int(x=1)` reports an unknown keyword exactly like CPython;
/// `vectorcall` + `at_most_total` model `long_vectorcall`'s dual arity wording.
#[derive(FromArgs)]
#[from_args(name = "int", at_most_total, vectorcall)]
struct IntArgs {
    #[from_args(pos_only, default)]
    x: Option<Value>,
    #[from_args(default)]
    base: Option<Value>,
}

/// Implements the `int()` constructor: numeric coercion, and str/bytes
/// parsing with an optional base (auto-detected when `base=0`).
pub(crate) fn int_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IntArgs { x, base } = IntArgs::from_args(args, vm)?;
    let Some(x) = x else {
        // `int()` → 0; `int(base=N)` complains about the missing value even
        // before validating the base, matching `long_new_impl`'s ordering.
        return match base {
            None => Ok(Value::Int(0)),
            Some(base) => {
                base.drop_with(vm);
                Err(ExcType::type_error_int_missing_string_argument())
            }
        };
    };
    defer_drop!(x, vm);
    match base {
        None => int_convert(x, vm),
        Some(base) => {
            let base = int_base(base, vm)?;
            match x.py_value(vm) {
                PyValue::Str(value) => parse_int_from_str(value.as_str(vm), base, vm.heap),
                PyValue::Bytes(value) => parse_int_from_bytes(value.as_slice(vm), base, vm.heap),
                _ => Err(ExcType::type_error_int_non_string_with_base()),
            }
        }
    }
}

/// `int(x)` with no base: numeric coercion plus base-10 str/bytes parsing.
fn int_convert(x: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if let Some(value) = x.py_int_impl(vm)? {
        Ok(value)
    } else if let Some(value) = x.py_index_impl(vm)? {
        Ok(value)
    } else {
        match x.py_value(vm) {
            PyValue::Str(value) => parse_int_from_str(value.as_str(vm), 10, vm.heap),
            PyValue::Bytes(value) => parse_int_from_bytes(value.as_slice(vm), 10, vm.heap),
            _ => Err(ExcType::type_error_int_conversion(&x.py_type_name(vm))),
        }
    }
}

/// Resolves the `base` argument to `0` or `2..=36`, consuming the value.
///
/// Mirrors CPython: the base goes through `PyNumber_AsSsize_t(obase, NULL)`,
/// which *clamps* out-of-i64 ints instead of raising — so a `LongInt` base
/// lands in the range error, not `OverflowError`; non-integers raise
/// `TypeError` before the range is checked.
fn int_base(base: Value, vm: &mut VM<'_>) -> RunResult<u32> {
    defer_drop!(base, vm);
    let n = base.saturating_index(vm)?;
    match u32::try_from(n) {
        Ok(0) => Ok(0),
        Ok(b @ 2..=36) => Ok(b),
        _ => Err(ExcType::value_error_int_base_range()),
    }
}

/// Parses a Python `int()` string argument into an `Int` or `LongInt`.
///
/// `base` is `0` (auto-detect from a `0x`/`0o`/`0b` prefix) or `2..=36`.
/// Returns `Value::Int` if the value fits in i64, otherwise allocates a
/// `LongInt` on the heap. Returns `ValueError` on failure.
fn parse_int_from_str(value: &str, base: u32, heap: &Heap) -> RunResult<Value> {
    // Fast path: plain base-10 literals parse directly (no whitespace,
    // underscores or prefix handling needed).
    if base == 10
        && let Ok(int) = value.parse::<i64>()
    {
        return Ok(Value::Int(int));
    }
    let trimmed = value.trim();
    // Preserve the allocation-free path for whitespace-padded i64 values
    // without retrying unchanged inputs that already failed above.
    if base == 10
        && trimmed.len() != value.len()
        && let Ok(int) = trimmed.parse::<i64>()
    {
        Ok(Value::Int(int))
    } else {
        let invalid = || ExcType::value_error_invalid_literal_for_int(base, StringRepr(value));
        parse_int_digits(trimmed, base, &invalid, heap)
    }
}

/// Parses a Python `int()` bytes argument using ASCII whitespace rules.
///
/// Unlike `str`, bytes must not treat UTF-8 encodings of Unicode whitespace as
/// separators. Failures repr the input as a bytes literal, matching CPython.
fn parse_int_from_bytes(bytes: &[u8], base: u32, heap: &Heap) -> RunResult<Value> {
    let invalid = || ExcType::value_error_invalid_literal_for_int(base, bytes_repr(bytes));
    match str::from_utf8(bytes.trim_ascii()) {
        Ok(s) => parse_int_digits(s, base, &invalid, heap),
        Err(_) => Err(invalid()),
    }
}

/// Tracks what the previous character was while scanning an int literal, to
/// enforce CPython's underscore rules: `_` allowed only between digits or
/// directly after a base prefix, never leading, trailing, or doubled.
enum IntScanState {
    Start,
    Prefix,
    Digit,
    Underscore,
}

/// Parses a whitespace-trimmed str/bytes int literal: sign, base prefix,
/// underscore placement, digit limits, and BigInt promotion.
fn parse_int_digits(value: &str, base: u32, invalid: &impl Fn() -> RunError, heap: &Heap) -> RunResult<Value> {
    let (negative, body) = match value.strip_prefix(['+', '-']) {
        Some(rest) => (value.starts_with('-'), rest),
        None => (false, value),
    };

    // Resolve the effective base and strip any `0x`/`0o`/`0b` prefix. For
    // `base=0` a leading zero *without* a prefix is only legal if every digit
    // is zero (CPython rejects `010` as an ambiguous octal-looking literal).
    let bytes = body.as_bytes();
    let mut digits = body;
    let mut effective_base = if base == 0 { 10 } else { base };
    let mut error_if_nonzero = false;
    if bytes.first() == Some(&b'0') {
        let prefix_base = match bytes.get(1) {
            Some(b'x' | b'X') => Some(16),
            Some(b'o' | b'O') => Some(8),
            Some(b'b' | b'B') => Some(2),
            Some(_) if base == 0 => {
                error_if_nonzero = true;
                None
            }
            _ => None,
        };
        if let Some(prefix_base) = prefix_base
            && (base == 0 || base == prefix_base)
        {
            effective_base = prefix_base;
            digits = &body[2..];
        }
    }
    let had_prefix = digits.len() != body.len();

    // Validate digits and underscore placement, collecting the cleaned digits
    // (no underscores) behind the sign. Untracked `String`, but bounded by the
    // input which is itself an already-tracked string.
    let mut cleaned = String::with_capacity(usize::from(negative) + digits.len());
    if negative {
        cleaned.push('-');
    }
    let mut state = if had_prefix {
        IntScanState::Prefix
    } else {
        IntScanState::Start
    };
    for c in digits.chars() {
        if c == '_' {
            if !matches!(state, IntScanState::Digit | IntScanState::Prefix) {
                return Err(invalid());
            }
            state = IntScanState::Underscore;
        } else if c.is_digit(effective_base) {
            cleaned.push(c);
            state = IntScanState::Digit;
        } else {
            return Err(invalid());
        }
    }
    // Must end on a digit: rejects empty input, a bare prefix/sign, and
    // trailing underscores in one check.
    if !matches!(state, IntScanState::Digit) {
        return Err(invalid());
    }
    if error_if_nonzero && cleaned.bytes().any(|b| !matches!(b, b'0' | b'-')) {
        return Err(invalid());
    }

    if let Ok(int) = i64::from_str_radix(&cleaned, effective_base) {
        return Ok(Value::Int(int));
    }
    // CPython's int↔str digit limit applies only to bases that are not a
    // power of two (where conversion cost is linear, not quadratic).
    let digit_count = cleaned.len() - usize::from(negative);
    if !effective_base.is_power_of_two() && digit_count > INT_MAX_STR_DIGITS {
        return Err(ExcType::value_error_int_str_too_large(digit_count));
    }
    match BigInt::parse_bytes(cleaned.as_bytes(), effective_base) {
        Some(bi) => Ok(LongInt::new(bi).into_value(heap)),
        // Unreachable in practice: every char was validated as a digit above.
        None => Err(invalid()),
    }
}
