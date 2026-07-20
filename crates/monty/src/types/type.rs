use std::{borrow::Cow, fmt};

use num_bigint::BigInt;

use crate::{
    args::{ArgValues, FromArgs, is_long_int},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{DropWithContext, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::ResourceTracker,
    types::{
        AttrCallResult, Bytes, Dict, FrozenSet, List, LongInt, MontyIter, Path, PyTrait, Range, Set, Slice, Str,
        TimeZone, Tuple,
        bytes::{bytes_fromhex, bytes_repr},
        date, datetime,
        dict::dict_fromkeys,
        instance::class_name,
        long_int::INT_MAX_STR_DIGITS,
        str::StringRepr,
        timedelta,
    },
    value::Value,
};

/// Represents the Python type of a value.
///
/// This enum is used both for type checking and as a callable constructor.
/// Some variants are Python builtins accessible by name (e.g., `int`, `list`),
/// while others are internal types only available through imports or introspection
/// (e.g., `TextIOWrapper`, `PosixPath`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[strum(serialize_all = "lowercase")]
#[expect(
    clippy::enum_variant_names,
    reason = "`Type` and `NoneType` mirror the Python type names"
)]
pub enum Type {
    Ellipsis,
    Type,
    #[strum(serialize = "NoneType")]
    NoneType,
    Bool,
    Int,
    Float,
    Range,
    Slice,
    Date,
    #[strum(serialize = "datetime.datetime")]
    DateTime,
    TimeDelta,
    TimeZone,
    Str,
    Bytes,
    List,
    Tuple,
    NamedTuple,
    Dict,
    #[strum(serialize = "dict_keys")]
    DictKeys,
    #[strum(serialize = "dict_items")]
    DictItems,
    #[strum(serialize = "dict_values")]
    DictValues,
    Set,
    FrozenSet,
    Dataclass,
    /// An instance of a user-defined class (`class Foo: ...`), carrying the
    /// `HeapId` of its class object so the real class name can be resolved
    /// (via [`Type::name`]) for error messages and reprs. The class
    /// object itself reports [`Type::Type`] (matching `type(Foo) is type`).
    ///
    /// **SAFETY/LIFETIME INVARIANT**: the id is a NON-OWNING, transient
    /// reference — `Type` is `Copy`, untracked by refcounting, and has no
    /// `Drop`. A `Type::Instance` is only valid while the value it was derived
    /// from is alive (an instance holds a counted ref to its class, taken in
    /// `VM::instantiate_class`). It must NEVER be stored long-lived,
    /// serialized into snapshots/const pools, placed in `Builtins::Type` (the
    /// `type()` builtin returns the class object itself for instances), or
    /// converted to `MontyObject` without resolving the name first (the public
    /// boundary enum `MontyType` carries the resolved name as a `String`).
    #[strum(disabled)]
    Instance(HeapId),
    /// Exception types render/parse via `ExcType`'s own strum name
    /// (`"ValueError"`, `"json.JSONDecodeError"`, ...), so this variant is
    /// `#[strum(disabled)]`: every strum consumer (`Display`, [`Type::name`],
    /// [`Type::from_type_name`]) peels `Exception` off explicitly, and
    /// enabling it would make `EnumString` accept the meaningless
    /// `"exception"`.
    #[strum(disabled)]
    Exception(ExcType),
    Function,
    #[strum(serialize = "builtin_function_or_method")]
    BuiltinFunction,
    Cell,
    Iterator,
    #[strum(serialize = "list_iterator")]
    ListIterator,
    /// Coroutine type for async functions and external futures.
    Coroutine,
    Module,
    /// Marker types like stdout/stderr - displays as "_io.TextIOWrapper"
    #[strum(serialize = "_io.TextIOWrapper")]
    TextIOWrapper,
    /// Binary file object returned by `open(..., "rb")`.
    #[strum(serialize = "_io.BufferedReader")]
    BufferedReader,
    /// Binary file object returned by write-only binary modes.
    #[strum(serialize = "_io.BufferedWriter")]
    BufferedWriter,
    /// Binary file object returned by read/write binary modes.
    #[strum(serialize = "_io.BufferedRandom")]
    BufferedRandom,
    /// typing module special forms (Any, Optional, Union, etc.) - displays as "typing._SpecialForm"
    #[strum(serialize = "typing._SpecialForm")]
    SpecialForm,
    /// A filesystem path from `pathlib.Path` - displays as "PosixPath"
    #[strum(serialize = "PosixPath")]
    Path,
    /// A property descriptor - displays as "property"
    Property,
    /// A compiled regex pattern from `re.compile()` - displays as "re.Pattern"
    #[strum(serialize = "re.Pattern")]
    RePattern,
    /// A regex match result from `re.match()` / `re.search()` etc. - displays as "re.Match"
    #[strum(serialize = "re.Match")]
    ReMatch,
}

/// Writes the canonical static name of every non-[`Instance`](Type::Instance)
/// variant — the single name table backing [`Type::name`] and `MontyType`'s
/// `Display`.
///
/// The names live on the enum via the `IntoStaticStr` derive
/// (`serialize_all = "lowercase"` plus per-variant `serialize` overrides);
/// `Exception` delegates to `ExcType`'s own strum name.
///
/// # Panics
/// On `Instance`, which has no static name — callers with heap access must
/// resolve the real class name via [`Type::name`]. Well-formed data never
/// puts an `Instance` where no heap exists (`Builtins::Type`, `MontyObject`,
/// the wire protocol), so this is a programmer-error tripwire. A crafted
/// snapshot payload *can* smuggle one in, but snapshot bytes are not a
/// panic-free boundary anyway — any bogus `HeapId` in them panics on first
/// heap access.
impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            Self::Exception(exc_type) => exc_type.into(),
            Self::Instance(_) => unreachable!("Type::Instance must be rendered via Type::name"),
            other => other.into(),
        })
    }
}

impl Type {
    /// The Python-visible name of this type: the real class name for
    /// [`Instance`](Self::Instance), the static `Display` name otherwise —
    /// the primary way to render a `Type` in error messages and reprs. The
    /// result borrows only `interns` (never the heap — heap-owned dynamic
    /// class names are cloned into `Cow::Owned`), so it can be captured
    /// before heap-mutating cleanup (`drop_with`) at error sites and
    /// formatted after.
    pub(crate) fn name<'i>(self, heap: &Heap<impl ResourceTracker>, interns: &'i Interns) -> Cow<'i, str> {
        match self {
            Self::Instance(class_id) => class_name(class_id, heap, interns),
            Self::Exception(exc_type) => Cow::Borrowed(exc_type.into()),
            other => Cow::Borrowed(other.into()),
        }
    }

    /// [`name`](Self::name) as rendered by CPython's `_PyArg_BadArgument`
    /// ("argument N must be X, not Y") error formatter: identical except that
    /// `NoneType` renders as `"None"` — CPython special-cases
    /// `arg == Py_None ? "None" : Py_TYPE(arg)->tp_name`, and since `NoneType`
    /// is a singleton, branching on the type is equivalent to branching on the
    /// value. Use for the "not Y" half of arg-type error messages only.
    pub(crate) fn cpython_arg_name<'i>(self, heap: &Heap<impl ResourceTracker>, interns: &'i Interns) -> Cow<'i, str> {
        match self {
            Self::NoneType => Cow::Borrowed("None"),
            other => other.name(heap, interns),
        }
    }

    /// Returns the Python source-level name for builtin types that can be called directly.
    ///
    /// This differs from `Display` for internal representation-only names such as
    /// `Type::Iterator`, which displays as `iterator` for repr/type output but is
    /// exposed as the builtin constructor `iter` in Python source.
    #[must_use]
    pub const fn builtin_name(self) -> Option<&'static str> {
        match self {
            Self::Bool => Some("bool"),
            Self::Int => Some("int"),
            Self::Float => Some("float"),
            Self::Str => Some("str"),
            Self::Bytes => Some("bytes"),
            Self::List => Some("list"),
            Self::Tuple => Some("tuple"),
            Self::Dict => Some("dict"),
            Self::Set => Some("set"),
            Self::FrozenSet => Some("frozenset"),
            Self::Range => Some("range"),
            Self::Slice => Some("slice"),
            Self::Iterator => Some("iter"),
            Self::Type => Some("type"),
            Self::Property => Some("property"),
            _ => None,
        }
    }

    /// Resolves a bare Python name to a builtin type, if it is one.
    ///
    /// Only matches names that are true Python builtins — accessible without any import.
    /// Internal types like `TextIOWrapper`, `PosixPath`, `NoneType`, and `ellipsis` are
    /// intentionally excluded because they require imports or are not directly nameable.
    ///
    /// This replaces the previous strum `FromStr` derive which matched ALL variants,
    /// including internal types that shouldn't be resolvable from bare names.
    #[must_use]
    pub fn from_builtin_name(name: &str) -> Option<Self> {
        match name {
            "bool" => Some(Self::Bool),
            "int" => Some(Self::Int),
            "float" => Some(Self::Float),
            "str" => Some(Self::Str),
            "bytes" => Some(Self::Bytes),
            "list" => Some(Self::List),
            "tuple" => Some(Self::Tuple),
            "dict" => Some(Self::Dict),
            "set" => Some(Self::Set),
            "frozenset" => Some(Self::FrozenSet),
            "range" => Some(Self::Range),
            "slice" => Some(Self::Slice),
            "iter" => Some(Self::Iterator),
            "type" => Some(Self::Type),
            "property" => Some(Self::Property),
            _ => None,
        }
    }

    /// The inverse of `Display`: resolves any string it produces back to the
    /// `Type`, including internal names (`"iterator"`,
    /// `"_io.TextIOWrapper"`, ...) and exception types.
    ///
    /// Unlike [`Type::from_builtin_name`] this is NOT restricted to nameable
    /// builtins — it exists for boundaries that serialize a type by its
    /// display name (e.g. the subprocess wire protocol) and must round-trip
    /// every variant; the round-trip is enforced by a test over all variants.
    /// [`Instance`](Self::Instance) is intentionally excluded (`"object"`
    /// returns `None`): its `HeapId` payload cannot be reconstructed from a
    /// name, and no boundary may carry it.
    #[must_use]
    pub(crate) fn from_type_name(name: &str) -> Option<Self> {
        // `EnumString` parses via the same strum `serialize` attributes that
        // `IntoStaticStr`/`Display` render with, so the two stay in lockstep
        // by construction. Exception types display as their exception name
        // ("ValueError", "json.JSONDecodeError", ...) — fall back to the
        // ExcType parser.
        name.parse::<Self>()
            .ok()
            .or_else(|| name.parse::<ExcType>().ok().map(Self::Exception))
    }

    /// Checks if a value of type `self` is an instance of `other`.
    ///
    /// This handles Python's subtype relationships:
    /// - `bool` is a subtype of `int` (so `isinstance(True, int)` returns True)
    /// - `datetime` is a subtype of `date` (so `isinstance(datetime_obj, date)` returns True)
    #[must_use]
    pub fn is_instance_of(self, other: Self) -> bool {
        if self == other {
            true
        } else if self == Self::Bool && other == Self::Int {
            // bool is a subtype of int in Python
            true
        } else if self == Self::DateTime && other == Self::Date {
            // datetime is a subtype of date in Python
            true
        } else {
            false
        }
    }

    /// Converts a callable type to a u8 for the `CallBuiltinType` opcode.
    ///
    /// Returns `Some(u8)` for types that can be called as constructors,
    /// `None` for non-callable types.
    #[must_use]
    pub fn callable_to_u8(self) -> Option<u8> {
        match self {
            Self::Bool => Some(0),
            Self::Int => Some(1),
            Self::Float => Some(2),
            Self::Str => Some(3),
            Self::Bytes => Some(4),
            Self::List => Some(5),
            Self::Tuple => Some(6),
            Self::Dict => Some(7),
            Self::Set => Some(8),
            Self::FrozenSet => Some(9),
            Self::Range => Some(10),
            Self::Slice => Some(11),
            Self::Iterator => Some(12),
            Self::Path => Some(13),
            _ => None,
        }
    }

    /// Converts a u8 back to a callable `Type` for the `CallBuiltinType` opcode.
    ///
    /// Returns `Some(Type)` for valid callable type IDs, `None` otherwise.
    #[must_use]
    pub fn callable_from_u8(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Bool),
            1 => Some(Self::Int),
            2 => Some(Self::Float),
            3 => Some(Self::Str),
            4 => Some(Self::Bytes),
            5 => Some(Self::List),
            6 => Some(Self::Tuple),
            7 => Some(Self::Dict),
            8 => Some(Self::Set),
            9 => Some(Self::FrozenSet),
            10 => Some(Self::Range),
            11 => Some(Self::Slice),
            12 => Some(Self::Iterator),
            13 => Some(Self::Path),
            _ => None,
        }
    }

    /// Dispatches classmethod calls on builtin type objects (e.g. `dict.fromkeys`).
    ///
    /// Keeps classmethod behavior centralized with type semantics instead of VM call plumbing.
    pub(crate) fn call_class_method(
        self,
        method_id: StringId,
        args: ArgValues,
        vm: &mut VM<'_, impl ResourceTracker>,
    ) -> RunResult<AttrCallResult> {
        match (self, method_id) {
            (Self::Dict, m) if m == StaticStrings::Fromkeys => dict_fromkeys(args, vm).map(AttrCallResult::Value),
            (Self::Bytes, m) if m == StaticStrings::Fromhex => bytes_fromhex(args, vm).map(AttrCallResult::Value),
            (Self::Date, m) if m == StaticStrings::Today => date::class_today(vm.heap, args),
            (Self::Date, m) if m == StaticStrings::Fromisoformat => {
                date::class_fromisoformat(vm.heap, args, vm.interns).map(AttrCallResult::Value)
            }
            (Self::DateTime, m) if m == StaticStrings::Now => datetime::class_now(vm, args),
            (Self::DateTime, m) if m == StaticStrings::Strptime => {
                datetime::class_strptime(vm.heap, args, vm.interns).map(AttrCallResult::Value)
            }
            (Self::DateTime, m) if m == StaticStrings::Fromisoformat => {
                datetime::class_fromisoformat(vm.heap, args, vm.interns).map(AttrCallResult::Value)
            }
            _ => {
                let method_name = vm.interns.get_str(method_id);
                args.drop_with(vm.heap);
                Err(ExcType::attribute_error(self, method_name))
            }
        }
    }

    /// Calls this type as a constructor (e.g., `list(x)`, `int(x)`).
    ///
    /// Dispatches to the appropriate type's init method for container types,
    /// or handles primitive type conversions inline.
    pub(crate) fn call(self, vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        match self {
            // Container types - delegate to init methods
            Self::List => List::init(vm, args),
            Self::Tuple => Tuple::init(vm, args),
            Self::Dict => Dict::init(vm, args),
            Self::Set => Set::init(vm, args),
            Self::FrozenSet => FrozenSet::init(vm, args),
            Self::Str => Str::init(vm, args),
            Self::Bytes => Bytes::init(vm, args),
            Self::Range => Range::init(vm, args),
            Self::Slice => Slice::init(vm, args),
            Self::Date => date::init(vm, args),
            Self::DateTime => datetime::init(vm, args),
            Self::TimeDelta => timedelta::init(vm, args),
            Self::TimeZone => TimeZone::init(vm, args),
            Self::Iterator => MontyIter::init(vm, args),
            Self::Path => Path::init(vm, args),

            // Primitive types - inline implementation
            Self::Int => int_init(vm, args),
            Self::Float => {
                let interns = vm.interns;
                let Some(v) = args.get_zero_one_arg("float", vm.heap)? else {
                    return Ok(Value::Float(0.0));
                };
                defer_drop!(v, vm);
                match v {
                    Value::Float(f) => Ok(Value::Float(*f)),
                    Value::Int(i) => Ok(Value::Float(*i as f64)),
                    Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                    Value::InternString(string_id) => {
                        Ok(Value::Float(parse_f64_from_str(interns.get_str(*string_id))?))
                    }
                    Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
                        HeapData::Str(s) => Ok(Value::Float(parse_f64_from_str(s.as_str())?)),
                        _ => Err(ExcType::type_error_float_conversion(&v.py_type_name(vm))),
                    },
                    _ => Err(ExcType::type_error_float_conversion(&v.py_type_name(vm))),
                }
            }
            Self::Bool => {
                let Some(v) = args.get_zero_one_arg("bool", vm.heap)? else {
                    return Ok(Value::Bool(false));
                };
                defer_drop!(v, vm);
                Ok(Value::Bool(v.py_bool(vm)))
            }

            // Non-callable types - raise TypeError
            _ => Err(ExcType::type_error_not_callable(&self.name(vm.heap, vm.interns))),
        }
    }
}

/// Truncates f64 to i64 with clamping for out-of-range values.
///
/// Python's `int(float)` truncates toward zero. For values outside i64 range,
/// we clamp to i64::MAX/MIN (Python would use arbitrary precision ints, which
/// we don't support).
fn f64_to_i64_truncate(value: f64) -> i64 {
    // trunc() rounds toward zero, matching Python's int(float) behavior
    let truncated = value.trunc();
    if truncated >= i64::MAX as f64 {
        i64::MAX
    } else if truncated <= i64::MIN as f64 {
        i64::MIN
    } else {
        // SAFETY for clippy: truncated is guaranteed to be in (i64::MIN, i64::MAX)
        // after the bounds checks above, so truncation cannot overflow
        #[expect(clippy::cast_possible_truncation, reason = "bounds checked above")]
        let result = truncated as i64;
        result
    }
}

/// Parses a Python `float()` string argument into an `f64`.
///
/// This supports:
/// - Leading/trailing whitespace (e.g. `"  1.5  "`)
/// - The special values `inf`, `-inf`, `infinity`, and `nan` (case-insensitive)
///
/// Underscore digit separators are not currently supported.
fn parse_f64_from_str(value: &str) -> RunResult<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(value_error_could_not_convert_string_to_float(value));
    }

    let lower = trimmed.to_ascii_lowercase();
    let parsed = match lower.as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => f64::INFINITY,
        "-inf" | "-infinity" => f64::NEG_INFINITY,
        "nan" | "+nan" => f64::NAN,
        "-nan" => -f64::NAN,
        _ => trimmed
            .parse::<f64>()
            .map_err(|_| value_error_could_not_convert_string_to_float(value))?,
    };

    Ok(parsed)
}

/// Creates the `ValueError` raised by `float()` when a string cannot be parsed.
///
/// Matches CPython's message format: `could not convert string to float: '...'`.
fn value_error_could_not_convert_string_to_float(value: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("could not convert string to float: {}", StringRepr(value)),
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
fn int_init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
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
            let interns = vm.interns;
            match x {
                Value::InternString(string_id) => parse_int_from_str(interns.get_str(*string_id), base, vm.heap),
                Value::InternBytes(bytes_id) => parse_int_from_bytes(interns.get_bytes(*bytes_id), base, vm.heap),
                Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
                    HeapData::Str(s) => parse_int_from_str(s.as_str(), base, vm.heap),
                    HeapData::Bytes(b) => parse_int_from_bytes(b.as_slice(), base, vm.heap),
                    _ => Err(ExcType::type_error_int_non_string_with_base()),
                },
                _ => Err(ExcType::type_error_int_non_string_with_base()),
            }
        }
    }
}

/// `int(x)` with no base: numeric coercion plus base-10 str/bytes parsing.
fn int_convert(x: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    let interns = vm.interns;
    match x {
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::Float(f) => Ok(Value::Int(f64_to_i64_truncate(*f))),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        Value::InternString(string_id) => parse_int_from_str(interns.get_str(*string_id), 10, vm.heap),
        Value::InternBytes(bytes_id) => parse_int_from_bytes(interns.get_bytes(*bytes_id), 10, vm.heap),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(s) => parse_int_from_str(s.as_str(), 10, vm.heap),
            HeapData::Bytes(b) => parse_int_from_bytes(b.as_slice(), 10, vm.heap),
            HeapData::LongInt(_) => Ok(x.clone_with_heap(vm.heap)),
            _ => Err(ExcType::type_error_int_conversion(&x.py_type_name(vm))),
        },
        _ => Err(ExcType::type_error_int_conversion(&x.py_type_name(vm))),
    }
}

/// Resolves the `base` argument to `0` or `2..=36`, consuming the value.
///
/// Mirrors CPython: the base goes through `PyNumber_AsSsize_t(obase, NULL)`,
/// which *clamps* out-of-i64 ints instead of raising — so a `LongInt` base
/// lands in the range error, not `OverflowError`; non-integers raise
/// `TypeError` before the range is checked.
fn int_base(base: Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<u32> {
    let n = match &base {
        Value::Bool(b) => i64::from(*b),
        Value::Int(i) => *i,
        // Clamped by PyNumber_AsSsize_t: any i64-overflowing int is out of range.
        _ if is_long_int(&base, vm) => i64::MAX,
        _ => {
            let err = ExcType::type_error_not_integer(&base.py_type_name(vm));
            base.drop_with(vm);
            return Err(err);
        }
    };
    base.drop_with(vm);
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
fn parse_int_from_str(value: &str, base: u32, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
fn parse_int_from_bytes(bytes: &[u8], base: u32, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
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
fn parse_int_digits(
    value: &str,
    base: u32,
    invalid: &impl Fn() -> RunError,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<Value> {
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
        Some(bi) => Ok(LongInt::new(bi).into_value(heap)?),
        // Unreachable in practice: every char was validated as a digit above.
        None => Err(invalid()),
    }
}
