//! String, bytes, and long integer interning for efficient storage of literals and identifiers.
//!
//! This module provides interners that store unique strings, bytes, and long integers in vectors
//! and return indices (`StringId`, `BytesId`, `LongIntId`) for efficient storage and comparison.
//! This avoids the overhead of cloning strings or using atomic reference counting.
//!
//! The interners are populated during parsing and preparation, then owned by the `Executor`.
//! During execution, lookups are needed only for error messages and repr output.
//!
//! StringIds are laid out as follows:
//! * 0 to 128 - single character strings for all 128 ASCII characters
//! * 1000 to count(StaticStrings) - strings StaticStrings
//! * 10_000+ - strings interned per executor

use std::{array, str::FromStr, sync::LazyLock};

use ahash::AHashMap;
use num_bigint::BigInt;
use strum::{EnumString, FromRepr, IntoStaticStr};

use crate::{function::Function, value::Value};

/// Index into the string interner's storage.
///
/// Uses `u32` to save space (4 bytes vs 8 bytes for `usize`). This limits us to
/// ~4 billion unique interns, which is more than sufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct StringId(u32);

impl StringId {
    /// Creates a StringId from a raw index value.
    ///
    /// Used by the bytecode VM to reconstruct StringIds from operands stored
    /// in bytecode. The caller is responsible for ensuring the index is valid.
    #[inline]
    pub fn from_index(index: u16) -> Self {
        Self(u32::from(index))
    }

    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// Returns the StringId for an ASCII byte.
    #[must_use]
    pub fn from_ascii(byte: u8) -> Self {
        Self(u32::from(byte))
    }
}

/// StringId offsets
const STATIC_STRING_ID_OFFSET: u32 = 1000;
const INTERN_STRING_ID_OFFSET: usize = 10_000;

/// Static strings for all 128 ASCII characters, built once on first access.
///
/// Uses `LazyLock` to build the array at runtime (once), leaking the strings to get
/// `'static` lifetime. The leak is intentional and bounded (128 single-byte strings).
static ASCII_STRS: LazyLock<[&'static str; 128]> = LazyLock::new(|| {
    array::from_fn(|i| {
        // Safe: i is always 0-127 for a 128-element array
        let s = char::from(u8::try_from(i).expect("index out of u8 range")).to_string();
        // Leak to get 'static lifetime - this is intentional and bounded (128 bytes total)
        // Reborrow as immutable since we won't mutate
        &*Box::leak(s.into_boxed_str())
    })
});

/// Static string values which are known at compile time and don't need to be interned.
#[repr(u16)]
#[derive(
    Debug, Clone, Copy, FromRepr, EnumString, IntoStaticStr, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[strum(serialize_all = "snake_case")]
pub enum StaticStrings {
    #[strum(serialize = "")]
    EmptyString,
    #[strum(serialize = "<module>")]
    Module,
    // ==========================
    // List methods
    // Also uses shared: POP, CLEAR, COPY, REMOVE
    // Also uses string-shared: INDEX, COUNT
    Append,
    Insert,
    Extend,
    Reverse,
    Sort,

    // ==========================
    // Dict methods
    // Also uses shared: POP, CLEAR, COPY, UPDATE
    Get,
    Keys,
    Values,
    Items,
    Setdefault,
    Popitem,
    Fromkeys,

    // ==========================
    // Shared methods
    // Used by multiple container types: list, dict, set
    Pop,
    Clear,
    Copy,

    // ==========================
    // Set methods
    // Also uses shared: POP, CLEAR, COPY
    Add,
    Remove,
    Discard,
    Update,
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
    Issubset,
    Issuperset,
    Isdisjoint,

    // ==========================
    // String methods
    // Some methods shared with bytes: FIND, INDEX, COUNT, STARTSWITH, ENDSWITH
    // Some methods shared with list/tuple: INDEX, COUNT
    Join,
    // Simple transformations
    Lower,
    Upper,
    Capitalize,
    Title,
    Swapcase,
    Casefold,
    // Predicate methods
    Isalpha,
    Isdigit,
    Isalnum,
    Isnumeric,
    Isspace,
    Islower,
    Isupper,
    Isascii,
    Isdecimal,
    // Search methods (some shared with bytes, list, tuple)
    Find,
    Rfind,
    Index,
    Rindex,
    Count,
    Startswith,
    Endswith,
    // Strip/trim methods
    Strip,
    Lstrip,
    Rstrip,
    Removeprefix,
    Removesuffix,
    // Split methods
    Split,
    Rsplit,
    Splitlines,
    Partition,
    Rpartition,
    // Replace/padding methods
    Replace,
    Center,
    Ljust,
    Rjust,
    Zfill,
    Expandtabs,
    // Keyword argument names for string/bytes methods and constructors
    Tabsize,
    Keepends,
    Object,
    Source,
    // Additional string methods
    Encode,
    Isidentifier,
    Istitle,

    // ==========================
    // Bytes methods
    // Also uses string-shared: FIND, INDEX, COUNT, STARTSWITH, ENDSWITH
    // Also uses most string methods: LOWER, UPPER, CAPITALIZE, TITLE, SWAPCASE,
    // ISALPHA, ISDIGIT, ISALNUM, ISSPACE, ISLOWER, ISUPPER, ISASCII, ISTITLE,
    // RFIND, RINDEX, STRIP, LSTRIP, RSTRIP, REMOVEPREFIX, REMOVESUFFIX,
    // SPLIT, RSPLIT, SPLITLINES, PARTITION, RPARTITION, REPLACE,
    // CENTER, LJUST, RJUST, ZFILL, JOIN
    Decode,
    Hex,
    Fromhex,

    // ==========================
    // sys module strings
    Sys,
    #[strum(serialize = "sys.version_info")]
    SysVersionInfo,
    Version,
    VersionInfo,
    Platform,
    Stdout,
    Stderr,
    Major,
    Minor,
    Micro,
    Releaselevel,
    Serial,
    Final,
    #[strum(serialize = "3.14.0 (Monty)")]
    MontyVersionString,
    Monty,

    // ==========================
    // os.stat_result fields
    #[strum(serialize = "StatResult")]
    OsStatResult,
    StMode,
    StIno,
    StDev,
    StNlink,
    StUid,
    StGid,
    StSize,
    StAtime,
    StMtime,
    StCtime,

    // ==========================
    // typing module strings
    Typing,
    #[strum(serialize = "TYPE_CHECKING")]
    TypeChecking,
    #[strum(serialize = "Any")]
    Any,
    #[strum(serialize = "Optional")]
    Optional,
    #[strum(serialize = "Union")]
    UnionType,
    #[strum(serialize = "List")]
    ListType,
    #[strum(serialize = "Dict")]
    DictType,
    #[strum(serialize = "Tuple")]
    TupleType,
    #[strum(serialize = "Set")]
    SetType,
    #[strum(serialize = "FrozenSet")]
    FrozenSet,
    #[strum(serialize = "Callable")]
    Callable,
    #[strum(serialize = "Type")]
    Type,
    #[strum(serialize = "Sequence")]
    Sequence,
    #[strum(serialize = "Mapping")]
    Mapping,
    #[strum(serialize = "Iterable")]
    Iterable,
    #[strum(serialize = "Iterator")]
    IteratorType,
    #[strum(serialize = "Generator")]
    Generator,
    #[strum(serialize = "ClassVar")]
    ClassVar,
    #[strum(serialize = "Final")]
    FinalType,
    #[strum(serialize = "Literal")]
    Literal,
    #[strum(serialize = "TypeVar")]
    TypeVar,
    #[strum(serialize = "Generic")]
    Generic,
    #[strum(serialize = "Protocol")]
    Protocol,
    #[strum(serialize = "Annotated")]
    Annotated,
    #[strum(serialize = "Self")]
    SelfType,
    #[strum(serialize = "Never")]
    Never,
    #[strum(serialize = "NoReturn")]
    NoReturn,

    // ==========================
    // asyncio module strings
    Asyncio,
    Gather,
    Run,

    // ==========================
    // os module strings
    Os,
    Getenv,
    Environ,
    Default,

    // ==========================
    // Exception attributes
    Args,

    // ==========================
    // Type attributes
    #[strum(serialize = "__name__")]
    DunderName,

    // ==========================
    // pathlib module strings
    Pathlib,
    #[strum(serialize = "Path")]
    PathClass,

    // Path properties (pure - no I/O)
    Name,
    Parent,
    Stem,
    Suffix,
    Suffixes,
    Parts,

    // Path pure methods (no I/O)
    IsAbsolute,
    Joinpath,
    WithName,
    WithStem,
    WithSuffix,
    AsPosix,
    #[strum(serialize = "__fspath__")]
    Fspath,

    // Path filesystem methods (require OsAccess - yield external calls)
    Exists,
    IsFile,
    IsDir,
    IsSymlink,
    #[strum(serialize = "stat")]
    StatMethod,
    ReadBytes,
    ReadText,
    Iterdir,
    Resolve,
    Absolute,

    // Path write methods (require OsAccess - yield external calls)
    WriteText,
    WriteBytes,
    Mkdir,
    Unlink,
    Rmdir,
    Rename,

    // Slice attributes
    Start,
    Stop,
    Step,

    // ==========================
    // module strings
    // ==========================

    // math module strings
    Math,
    // Rounding
    Round,
    Floor,
    Ceil,
    Trunc,
    // Roots & powers
    Sqrt,
    Isqrt,
    Cbrt,
    Pow,
    Exp,
    Exp2,
    Expm1,
    // Logarithms
    Log,
    Log1p,
    Log2,
    Log10,
    // Float properties
    Fabs,
    Isnan,
    Isinf,
    Isfinite,
    Copysign,
    Isclose,
    Nextafter,
    Ulp,
    // Trigonometric
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    // Hyperbolic
    Sinh,
    Cosh,
    Tanh,
    Asinh,
    Acosh,
    Atanh,
    // Angular conversion
    Degrees,
    Radians,
    // Integer math
    Factorial,
    Gcd,
    Lcm,
    Comb,
    Perm,
    // Modular / decomposition
    Fmod,
    Remainder,
    Modf,
    Frexp,
    Ldexp,
    // Special functions
    Gamma,
    Lgamma,
    Erf,
    Erfc,
    // Constants
    /// `math.pi` constant
    Pi,
    /// `math.e` constant
    #[strum(serialize = "e")]
    MathE,
    /// `math.tau` constant
    Tau,
    /// `math.inf` constant
    #[strum(serialize = "inf")]
    MathInf,
    /// `math.nan` constant
    #[strum(serialize = "nan")]
    MathNan,

    // ==========================
    // json module strings
    /// Module name for `import json`.
    Json,
    /// `json.loads()` function.
    Loads,
    /// `json.dumps()` function.
    Dumps,
    /// `json.JSONDecodeError` exception.
    #[strum(serialize = "JSONDecodeError")]
    JsonDecodeError,
    /// `json.dumps(indent=...)` keyword.
    Indent,
    /// `json.dumps(sort_keys=...)` keyword.
    #[strum(serialize = "sort_keys")]
    SortKeys,
    /// `json.dumps(ensure_ascii=...)` keyword.
    #[strum(serialize = "ensure_ascii")]
    EnsureAscii,
    /// `json.dumps(allow_nan=...)` keyword.
    #[strum(serialize = "allow_nan")]
    AllowNan,
    /// `json.dumps(separators=...)` keyword.
    Separators,
    /// `json.dumps(skipkeys=...)` keyword.
    Skipkeys,

    // ==========================
    // datetime module strings
    Datetime,
    Date,
    Timedelta,
    Timezone,
    Today,
    Now,
    Utc,
    TotalSeconds,
    Tzinfo,
    // date/datetime field attributes
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Microsecond,
    // timedelta constructor/attribute names
    Days,
    Seconds,
    Microseconds,
    Milliseconds,
    Minutes,
    Hours,
    Weeks,
    // timezone constructor kwargs
    Offset,
    // datetime.now() kwarg
    Tz,
    // date/datetime methods
    Isoformat,
    Strftime,
    Weekday,
    Isoweekday,
    Timestamp,
    Strptime,
    Fromisoformat,

    // re module strings
    /// Module name for `import re`.
    Re,
    /// `re.compile()` function
    Compile,
    /// `re.match()` / `pattern.match()` method
    Match,
    /// `re.search()` / `pattern.search()` method
    Search,
    /// `re.fullmatch()` / `pattern.fullmatch()` method
    Fullmatch,
    /// `re.findall()` / `pattern.findall()` method
    Findall,
    /// `re.sub()` / `pattern.sub()` method
    Sub,
    /// `match.group()` method
    Group,
    /// `match.groups()` method
    Groups,
    /// `match.span()` method
    Span,
    /// `match.end()` method
    End,
    /// `re.Pattern`
    #[strum(serialize = "Pattern")]
    PatternClass,
    /// `re.Match`
    #[strum(serialize = "Match")]
    MatchClass,
    /// `pattern.pattern`
    #[strum(serialize = "pattern")]
    PatternAttr,
    /// `match.string`
    #[strum(serialize = "string")]
    StringAttr,
    /// `pattern.flags`
    Flags,
    /// `re.IGNORECASE` flag
    #[strum(serialize = "IGNORECASE")]
    Ignorecase,
    /// `re.I` flag, alias
    #[strum(serialize = "I")]
    I,
    /// `re.MULTILINE` flag
    #[strum(serialize = "MULTILINE")]
    MultilineFlag,
    /// `re.M` flag, alias
    #[strum(serialize = "M")]
    M,
    /// `re.DOTALL` flag
    #[strum(serialize = "DOTALL")]
    DotallFlag,
    /// `re.S` flag, alias
    #[strum(serialize = "S")]
    S,
    /// `re.NOFLAG` flag
    #[strum(serialize = "NOFLAG")]
    NoFlag,
    /// `re.ASCII` flag
    #[strum(serialize = "ASCII")]
    AsciiFlag,
    /// `re.A` flag, alias
    #[strum(serialize = "A")]
    A,
    /// `re.PatternError` exception
    #[strum(serialize = "PatternError")]
    PatternError,
    /// `re.error` exception alias (same as `re.PatternError`)
    #[strum(serialize = "error")]
    Error,
    /// `re.escape()` function
    Escape,
    /// `re.finditer()` / `pattern.finditer()` method
    Finditer,
    /// `match.groupdict()` method
    Groupdict,

    // ==========================
    // numpy module strings
    /// Module name for `import numpy`.
    Numpy,
    /// `numpy.array()` function
    #[strum(serialize = "array")]
    NpArray,
    /// `numpy.asanyarray()` alias for `numpy.asarray()` in Monty's ndarray-only subset.
    #[strum(serialize = "asanyarray")]
    NpAsanyarray,
    /// `numpy.zeros()` function
    #[strum(serialize = "zeros")]
    NpZeros,
    /// `numpy.ones()` function
    #[strum(serialize = "ones")]
    NpOnes,
    /// `numpy.subtract()` function
    #[strum(serialize = "subtract")]
    NpSubtract,
    /// `numpy.multiply()` function
    #[strum(serialize = "multiply")]
    NpMultiply,
    /// `numpy.divide()` function
    #[strum(serialize = "divide")]
    NpDivide,
    /// `numpy.true_divide()` function
    #[strum(serialize = "true_divide")]
    NpTrueDivide,
    /// `numpy.floor_divide()` function
    #[strum(serialize = "floor_divide")]
    NpFloorDivide,
    /// `numpy.mod()` function
    #[strum(serialize = "mod")]
    NpMod,
    /// `numpy.equal()` function
    #[strum(serialize = "equal")]
    NpEqual,
    /// `numpy.not_equal()` function
    #[strum(serialize = "not_equal")]
    NpNotEqual,
    /// `numpy.greater()` function
    #[strum(serialize = "greater")]
    NpGreater,
    /// `numpy.greater_equal()` function
    #[strum(serialize = "greater_equal")]
    NpGreaterEqual,
    /// `numpy.less()` function
    #[strum(serialize = "less")]
    NpLess,
    /// `numpy.less_equal()` function
    #[strum(serialize = "less_equal")]
    NpLessEqual,
    /// `numpy.arange()` function
    #[strum(serialize = "arange")]
    NpArange,
    /// `numpy.linspace()` function
    #[strum(serialize = "linspace")]
    NpLinspace,
    /// `numpy.where()` function
    #[strum(serialize = "where")]
    NpWhere,
    /// `numpy.maximum()` function
    Maximum,
    /// `numpy.minimum()` function
    Minimum,
    /// `numpy.unique()` function
    Unique,
    /// `numpy.unique_values()` function
    #[strum(serialize = "unique_values")]
    NpUniqueValues,
    /// `numpy.unique_counts()` function
    #[strum(serialize = "unique_counts")]
    NpUniqueCounts,
    /// `numpy.unique_inverse()` function
    #[strum(serialize = "unique_inverse")]
    NpUniqueInverse,
    /// `numpy.unique_all()` function
    #[strum(serialize = "unique_all")]
    NpUniqueAll,
    /// `numpy.concatenate()` function
    Concatenate,
    /// `numpy.concat()` alias for `numpy.concatenate()`.
    #[strum(serialize = "concat")]
    NpConcat,
    /// Shared: `mean()` method/function
    Mean,
    /// Shared: `std()` method/function
    Std,
    /// Shared: `abs()` method/function — also used by math module
    Abs,
    /// `ndarray.flat` attribute — returns flattened 1D view of the array.
    #[strum(serialize = "flat")]
    NpFlat,
    /// `ndarray.flatten()` method
    Flatten,
    /// `ndarray.tolist()` method
    Tolist,
    /// `ndarray.reshape()` method
    Reshape,
    /// `ndarray.argmin()` method
    Argmin,
    /// `ndarray.argmax()` method
    Argmax,
    /// `ndarray.all()` method
    #[strum(serialize = "all")]
    NpAll,
    /// `ndarray.any()` method
    #[strum(serialize = "any")]
    NpAny,
    /// `ndarray.argsort()` method
    #[strum(serialize = "argsort")]
    NpArgsort,
    /// `numpy.argpartition()` function
    #[strum(serialize = "argpartition")]
    NpArgpartition,
    /// `ndarray.astype()` method
    #[strum(serialize = "astype")]
    NpAstype,
    /// `ndarray.transpose()` method
    #[strum(serialize = "transpose")]
    NpTranspose,
    /// `ndarray.size` attribute
    #[strum(serialize = "size")]
    NpSize,
    /// `ndarray.ndim` attribute
    #[strum(serialize = "ndim")]
    NpNdim,
    /// `ndarray.T` attribute (transpose)
    #[strum(serialize = "T")]
    NpT,
    /// `ndarray.dtype` attribute
    Dtype,
    /// `ndarray.shape` attribute / also used by pathlib `parts`
    #[strum(serialize = "shape")]
    NpShape,
    /// `ndarray.min()` / `numpy.min()` — shared with builtins
    #[strum(serialize = "min")]
    NpMin,
    /// `numpy.amin()` alias for `numpy.min()`.
    #[strum(serialize = "amin")]
    NpAmin,
    /// `ndarray.max()` / `numpy.max()` — shared with builtins
    #[strum(serialize = "max")]
    NpMax,
    /// `numpy.amax()` alias for `numpy.max()`.
    #[strum(serialize = "amax")]
    NpAmax,
    /// `ndarray.sum()` / `numpy.sum()` — shared with builtins
    #[strum(serialize = "sum")]
    NpSum,
    /// `numpy.dot()` function / `ndarray.dot()` method
    #[strum(serialize = "dot")]
    Dot,
    /// `numpy.cumsum()` function / `ndarray.cumsum()` method
    #[strum(serialize = "cumsum")]
    Cumsum,
    /// `numpy.cumulative_sum()` alias for `numpy.cumsum()`.
    #[strum(serialize = "cumulative_sum")]
    NpCumulativeSum,
    /// `numpy.clip()` function / `ndarray.clip()` method
    #[strum(serialize = "clip")]
    Clip,
    /// `numpy.prod()` function / `ndarray.prod()` method
    #[strum(serialize = "prod")]
    NpProd,
    /// `numpy.var()` function / `ndarray.var()` method
    #[strum(serialize = "var")]
    NpVar,
    /// `numpy.full()` function
    #[strum(serialize = "full")]
    NpFull,
    /// `numpy.eye()` function
    #[strum(serialize = "eye")]
    NpEye,
    /// `numpy.empty()` function
    #[strum(serialize = "empty")]
    NpEmpty,
    /// `numpy.zeros_like()` function
    #[strum(serialize = "zeros_like")]
    NpZerosLike,
    /// `numpy.ones_like()` function
    #[strum(serialize = "ones_like")]
    NpOnesLike,
    // Note: numpy.isnan/isinf/isfinite reuse math module's Isnan/Isinf/Isfinite
    /// `numpy.array_equal()` function
    #[strum(serialize = "array_equal")]
    NpArrayEqual,
    /// `numpy.array_equiv()` shape-compatible equality helper.
    #[strum(serialize = "array_equiv")]
    NpArrayEquiv,
    /// `numpy.count_nonzero()` function
    #[strum(serialize = "count_nonzero")]
    NpCountNonzero,
    /// `numpy.median()` function
    #[strum(serialize = "median")]
    NpMedian,
    /// `numpy.power()` function
    #[strum(serialize = "power")]
    NpPower,
    /// `numpy.diff()` function
    #[strum(serialize = "diff")]
    NpDiff,
    /// `numpy.ediff1d()` flattened first-difference helper.
    #[strum(serialize = "ediff1d")]
    NpEdiff1d,
    /// `numpy.fill_diagonal()` in-place diagonal fill helper.
    #[strum(serialize = "fill_diagonal")]
    NpFillDiagonal,
    /// `numpy.put()` in-place flattened index assignment helper.
    #[strum(serialize = "put")]
    NpPut,
    /// `numpy.copyto()` in-place copy helper.
    #[strum(serialize = "copyto")]
    NpCopyto,
    /// `numpy.putmask()` in-place masked assignment helper.
    #[strum(serialize = "putmask")]
    NpPutmask,
    /// `numpy.place()` in-place masked placement helper.
    #[strum(serialize = "place")]
    NpPlace,
    // Note: numpy.append reuses list's Append variant
    /// `numpy.vstack()` function
    #[strum(serialize = "vstack")]
    NpVstack,
    /// `numpy.hstack()` function
    #[strum(serialize = "hstack")]
    NpHstack,
    /// `numpy.dstack()` function
    #[strum(serialize = "dstack")]
    NpDstack,
    /// `numpy.stack()` function
    #[strum(serialize = "stack")]
    NpStack,
    /// `numpy.unstack()` function
    #[strum(serialize = "unstack")]
    NpUnstack,
    /// `numpy.tile()` function
    #[strum(serialize = "tile")]
    NpTile,
    /// `numpy.repeat()` function
    #[strum(serialize = "repeat")]
    NpRepeat,
    // Note: numpy.split reuses string's Split variant
    /// `numpy.nonzero()` function
    #[strum(serialize = "nonzero")]
    NpNonzero,
    /// `numpy.argwhere()` function
    #[strum(serialize = "argwhere")]
    NpArgwhere,
    /// `ndarray.ravel()` method
    #[strum(serialize = "ravel")]
    NpRavel,
    // Note: numpy.min/max/sum/sort reuse existing StaticStrings variants
    // (NpMin, NpMax, NpSum, Sort) which are already defined above.

    // --- Phase 2+ numpy functions ---
    /// `numpy.newaxis` constant (alias for None)
    #[strum(serialize = "newaxis")]
    Newaxis,
    /// `numpy.float64` dtype type object
    #[strum(serialize = "float64")]
    NpFloat64,
    /// `numpy.int64` dtype type object
    #[strum(serialize = "int64")]
    NpInt64,
    /// `numpy.bool_` dtype type object
    #[strum(serialize = "bool_")]
    NpBool_,
    /// `numpy.float32` dtype alias (maps to float64 internally)
    #[strum(serialize = "float32")]
    NpFloat32,
    /// `numpy.int32` dtype alias (maps to int64 internally)
    #[strum(serialize = "int32")]
    NpInt32,
    /// `numpy.bool` dtype alias (maps to bool_ internally)
    #[strum(serialize = "bool")]
    NpBool,
    /// `numpy.int_` dtype alias (maps to int64 internally)
    #[strum(serialize = "int_")]
    NpInt_,
    /// `numpy.intc` dtype alias (maps to int32 internally)
    #[strum(serialize = "intc")]
    NpIntc,
    /// `numpy.intp` dtype alias (maps to int64 internally)
    #[strum(serialize = "intp")]
    NpIntp,
    /// `numpy.long` dtype alias (maps to int64 internally)
    #[strum(serialize = "long")]
    NpLong,
    /// `numpy.longlong` dtype alias (maps to int64 internally)
    #[strum(serialize = "longlong")]
    NpLonglong,
    /// `numpy.byte` dtype alias (maps to int64 internally)
    #[strum(serialize = "byte")]
    NpByte,
    /// `numpy.short` dtype alias (maps to int64 internally)
    #[strum(serialize = "short")]
    NpShort,
    /// `numpy.int8` dtype alias (maps to int64 internally)
    #[strum(serialize = "int8")]
    NpInt8,
    /// `numpy.int16` dtype alias (maps to int64 internally)
    #[strum(serialize = "int16")]
    NpInt16,
    /// `numpy.uint` dtype alias (maps to int64 internally)
    #[strum(serialize = "uint")]
    NpUint,
    /// `numpy.uintc` dtype alias (maps to int32 internally)
    #[strum(serialize = "uintc")]
    NpUintc,
    /// `numpy.uintp` dtype alias (maps to int64 internally)
    #[strum(serialize = "uintp")]
    NpUintp,
    /// `numpy.ubyte` dtype alias (maps to int64 internally)
    #[strum(serialize = "ubyte")]
    NpUbyte,
    /// `numpy.ushort` dtype alias (maps to int64 internally)
    #[strum(serialize = "ushort")]
    NpUshort,
    /// `numpy.uint8` dtype alias (maps to int64 internally)
    #[strum(serialize = "uint8")]
    NpUint8,
    /// `numpy.uint16` dtype alias (maps to int64 internally)
    #[strum(serialize = "uint16")]
    NpUint16,
    /// `numpy.uint32` dtype alias (maps to int64 internally)
    #[strum(serialize = "uint32")]
    NpUint32,
    /// `numpy.uint64` dtype alias (maps to int64 internally)
    #[strum(serialize = "uint64")]
    NpUint64,
    /// `numpy.ulong` dtype alias (maps to int64 internally)
    #[strum(serialize = "ulong")]
    NpUlong,
    /// `numpy.ulonglong` dtype alias (maps to int64 internally)
    #[strum(serialize = "ulonglong")]
    NpUlonglong,
    /// `numpy.float16` dtype alias (maps to float32 internally)
    #[strum(serialize = "float16")]
    NpFloat16,
    /// `numpy.half` dtype alias (maps to float32 internally)
    #[strum(serialize = "half")]
    NpHalf,
    /// `numpy.single` dtype alias (maps to float32 internally)
    #[strum(serialize = "single")]
    NpSingle,
    /// `numpy.double` dtype alias (maps to float64 internally)
    #[strum(serialize = "double")]
    NpDouble,
    /// `numpy.longdouble` dtype alias (maps to float64 internally)
    #[strum(serialize = "longdouble")]
    NpLongdouble,
    /// `numpy.little_endian` constant
    #[strum(serialize = "little_endian")]
    NpLittleEndian,
    /// `numpy.euler_gamma` constant
    #[strum(serialize = "euler_gamma")]
    NpEulerGamma,
    /// `numpy.arcsin()` / `numpy.asin()` function
    #[strum(serialize = "arcsin")]
    NpArcsin,
    /// `numpy.arccos()` / `numpy.acos()` function
    #[strum(serialize = "arccos")]
    NpArccos,
    /// `numpy.arctan()` / `numpy.atan()` function
    #[strum(serialize = "arctan")]
    NpArctan,
    /// `numpy.arctan2()` function — two-argument arctangent
    #[strum(serialize = "arctan2")]
    NpArctan2,
    /// `numpy.angle()` function for real-valued phase angles.
    #[strum(serialize = "angle")]
    NpAngle,
    /// `numpy.arcsinh()` function
    #[strum(serialize = "arcsinh")]
    NpArcsinh,
    /// `numpy.arccosh()` function
    #[strum(serialize = "arccosh")]
    NpArccosh,
    /// `numpy.arctanh()` function
    #[strum(serialize = "arctanh")]
    NpArctanh,
    /// `numpy.sign()` function
    #[strum(serialize = "sign")]
    NpSign,
    /// `numpy.square()` function
    #[strum(serialize = "square")]
    NpSquare,
    /// `numpy.reciprocal()` function
    #[strum(serialize = "reciprocal")]
    NpReciprocal,
    /// `numpy.deg2rad()` function
    #[strum(serialize = "deg2rad")]
    NpDeg2rad,
    /// `numpy.rad2deg()` function
    #[strum(serialize = "rad2deg")]
    NpRad2deg,
    /// `numpy.hypot()` function — hypotenuse
    #[strum(serialize = "hypot")]
    NpHypot,
    /// `numpy.nan_to_num()` function
    #[strum(serialize = "nan_to_num")]
    NpNanToNum,
    /// `numpy.fmin()` function — NaN-ignoring minimum
    #[strum(serialize = "fmin")]
    NpFmin,
    /// `numpy.fmax()` function — NaN-ignoring maximum
    #[strum(serialize = "fmax")]
    NpFmax,
    /// `numpy.rint()` function — round to nearest integer
    #[strum(serialize = "rint")]
    NpRint,
    /// `numpy.around()` alias for `numpy.round()`.
    #[strum(serialize = "around")]
    NpAround,
    /// `numpy.positive()` function — unary +
    #[strum(serialize = "positive")]
    NpPositive,
    /// `numpy.negative()` function — unary -
    #[strum(serialize = "negative")]
    NpNegative,
    /// `numpy.logaddexp()` function.
    #[strum(serialize = "logaddexp")]
    NpLogaddexp,
    /// `numpy.logaddexp2()` function.
    #[strum(serialize = "logaddexp2")]
    NpLogaddexp2,
    /// `numpy.spacing()` function.
    #[strum(serialize = "spacing")]
    NpSpacing,
    /// `numpy.signbit()` function.
    #[strum(serialize = "signbit")]
    NpSignbit,
    /// `numpy.sinc()` function.
    #[strum(serialize = "sinc")]
    NpSinc,
    /// `numpy.heaviside()` function.
    #[strum(serialize = "heaviside")]
    NpHeaviside,
    /// `numpy.fix()` function.
    #[strum(serialize = "fix")]
    NpFix,
    /// `numpy.float_power()` function.
    #[strum(serialize = "float_power")]
    NpFloatPower,
    /// `numpy.divmod()` function.
    #[strum(serialize = "divmod")]
    NpDivmod,
    /// `numpy.bitwise_and()` integer/boolean bitwise AND.
    #[strum(serialize = "bitwise_and")]
    NpBitwiseAnd,
    /// `numpy.bitwise_or()` integer/boolean bitwise OR.
    #[strum(serialize = "bitwise_or")]
    NpBitwiseOr,
    /// `numpy.bitwise_xor()` integer/boolean bitwise XOR.
    #[strum(serialize = "bitwise_xor")]
    NpBitwiseXor,
    /// `numpy.bitwise_not()` integer/boolean bitwise inversion.
    #[strum(serialize = "bitwise_not")]
    NpBitwiseNot,
    /// `numpy.bitwise_invert()` alias for integer/boolean bitwise inversion.
    #[strum(serialize = "bitwise_invert")]
    NpBitwiseInvert,
    /// `numpy.invert()` alias for integer/boolean bitwise inversion.
    #[strum(serialize = "invert")]
    NpInvert,
    /// `numpy.left_shift()` integer bit shift helper.
    #[strum(serialize = "left_shift")]
    NpLeftShift,
    /// `numpy.right_shift()` integer bit shift helper.
    #[strum(serialize = "right_shift")]
    NpRightShift,
    /// `numpy.bitwise_left_shift()` alias for left shift.
    #[strum(serialize = "bitwise_left_shift")]
    NpBitwiseLeftShift,
    /// `numpy.bitwise_right_shift()` alias for right shift.
    #[strum(serialize = "bitwise_right_shift")]
    NpBitwiseRightShift,
    /// `numpy.bitwise_count()` integer population count helper.
    #[strum(serialize = "bitwise_count")]
    NpBitwiseCount,
    /// `numpy.packbits()` packs non-zero bits into bytes.
    #[strum(serialize = "packbits")]
    NpPackbits,
    /// `numpy.unpackbits()` unpacks byte values into bit arrays.
    #[strum(serialize = "unpackbits")]
    NpUnpackbits,
    /// `numpy.bartlett()` window generator.
    #[strum(serialize = "bartlett")]
    NpBartlett,
    /// `numpy.blackman()` window generator.
    #[strum(serialize = "blackman")]
    NpBlackman,
    /// `numpy.hamming()` window generator.
    #[strum(serialize = "hamming")]
    NpHamming,
    /// `numpy.hanning()` window generator.
    #[strum(serialize = "hanning")]
    NpHanning,
    /// `numpy.kaiser()` window generator.
    #[strum(serialize = "kaiser")]
    NpKaiser,
    /// `numpy.i0()` modified Bessel function helper.
    #[strum(serialize = "i0")]
    NpI0,
    /// `numpy.base_repr()` integer base conversion helper.
    #[strum(serialize = "base_repr")]
    NpBaseRepr,
    /// `numpy.binary_repr()` integer binary conversion helper.
    #[strum(serialize = "binary_repr")]
    NpBinaryRepr,
    /// `numpy.conj()` real-valued conjugate helper.
    #[strum(serialize = "conj")]
    NpConj,
    /// `numpy.conjugate()` alias for `numpy.conj()`.
    #[strum(serialize = "conjugate")]
    NpConjugate,
    /// `numpy.real()` real component helper.
    #[strum(serialize = "real")]
    NpReal,
    /// `numpy.real_if_close()` real-valued identity helper for Monty's numeric subset.
    #[strum(serialize = "real_if_close")]
    NpRealIfClose,
    /// `numpy.imag()` imaginary component helper.
    #[strum(serialize = "imag")]
    NpImag,
    /// `numpy.isreal()` element-wise real-valued predicate.
    #[strum(serialize = "isreal")]
    NpIsreal,
    /// `numpy.isrealobj()` object-level real-valued predicate.
    #[strum(serialize = "isrealobj")]
    NpIsrealobj,
    /// `numpy.isposinf()` element-wise positive infinity predicate.
    #[strum(serialize = "isposinf")]
    NpIsposinf,
    /// `numpy.isneginf()` element-wise negative infinity predicate.
    #[strum(serialize = "isneginf")]
    NpIsneginf,
    /// `numpy.iscomplex()` element-wise complex-valued predicate.
    #[strum(serialize = "iscomplex")]
    NpIscomplex,
    /// `numpy.iscomplexobj()` object-level complex-valued predicate.
    #[strum(serialize = "iscomplexobj")]
    NpIscomplexobj,
    /// `numpy.isscalar()` scalar predicate.
    #[strum(serialize = "isscalar")]
    NpIsscalar,
    /// `numpy.iterable()` iterable predicate.
    #[strum(serialize = "iterable")]
    NpIterable,
    /// `numpy.atleast_1d()` shape helper.
    #[strum(serialize = "atleast_1d")]
    NpAtleast1d,
    /// `numpy.atleast_2d()` shape helper.
    #[strum(serialize = "atleast_2d")]
    NpAtleast2d,
    /// `numpy.atleast_3d()` shape helper.
    #[strum(serialize = "atleast_3d")]
    NpAtleast3d,
    /// `numpy.diag_indices()` index helper.
    #[strum(serialize = "diag_indices")]
    NpDiagIndices,
    /// `numpy.diag_indices_from()` index helper.
    #[strum(serialize = "diag_indices_from")]
    NpDiagIndicesFrom,
    /// `numpy.tril_indices()` lower-triangle index helper.
    #[strum(serialize = "tril_indices")]
    NpTrilIndices,
    /// `numpy.tril_indices_from()` lower-triangle index helper.
    #[strum(serialize = "tril_indices_from")]
    NpTrilIndicesFrom,
    /// `numpy.triu_indices()` upper-triangle index helper.
    #[strum(serialize = "triu_indices")]
    NpTriuIndices,
    /// `numpy.triu_indices_from()` upper-triangle index helper.
    #[strum(serialize = "triu_indices_from")]
    NpTriuIndicesFrom,
    /// `numpy.indices()` dense coordinate grid helper.
    #[strum(serialize = "indices")]
    NpIndices,
    /// `numpy.unravel_index()` flat-to-coordinate index helper.
    #[strum(serialize = "unravel_index")]
    NpUnravelIndex,
    /// `numpy.ravel_multi_index()` coordinate-to-flat index helper.
    #[strum(serialize = "ravel_multi_index")]
    NpRavelMultiIndex,
    /// `numpy.nansum()` function
    #[strum(serialize = "nansum")]
    NpNansum,
    /// `numpy.nanmean()` function
    #[strum(serialize = "nanmean")]
    NpNanmean,
    /// `numpy.nanmin()` function
    #[strum(serialize = "nanmin")]
    NpNanmin,
    /// `numpy.nanmax()` function
    #[strum(serialize = "nanmax")]
    NpNanmax,
    /// `numpy.nanstd()` function
    #[strum(serialize = "nanstd")]
    NpNanstd,
    /// `numpy.nanvar()` function
    #[strum(serialize = "nanvar")]
    NpNanvar,
    /// `numpy.nanprod()` function
    #[strum(serialize = "nanprod")]
    NpNanprod,
    /// `numpy.nanmedian()` function
    #[strum(serialize = "nanmedian")]
    NpNanmedian,
    /// `numpy.nanargmin()` function
    #[strum(serialize = "nanargmin")]
    NpNanargmin,
    /// `numpy.nanargmax()` function
    #[strum(serialize = "nanargmax")]
    NpNanargmax,
    /// `numpy.average()` function
    #[strum(serialize = "average")]
    NpAverage,
    /// `numpy.percentile()` function
    #[strum(serialize = "percentile")]
    NpPercentile,
    /// `numpy.quantile()` function
    #[strum(serialize = "quantile")]
    NpQuantile,
    /// `numpy.ptp()` function — peak to peak
    #[strum(serialize = "ptp")]
    NpPtp,
    /// `numpy.cumprod()` function
    #[strum(serialize = "cumprod")]
    NpCumprod,
    /// `numpy.cumulative_prod()` alias for `numpy.cumprod()`.
    #[strum(serialize = "cumulative_prod")]
    NpCumulativeProd,
    /// `numpy.logical_and()` function
    #[strum(serialize = "logical_and")]
    NpLogicalAnd,
    /// `numpy.logical_or()` function
    #[strum(serialize = "logical_or")]
    NpLogicalOr,
    /// `numpy.logical_not()` function
    #[strum(serialize = "logical_not")]
    NpLogicalNot,
    /// `numpy.logical_xor()` function
    #[strum(serialize = "logical_xor")]
    NpLogicalXor,
    /// `numpy.allclose()` function
    #[strum(serialize = "allclose")]
    NpAllclose,
    /// `numpy.isin()` function
    #[strum(serialize = "isin")]
    NpIsin,
    /// `numpy.flip()` function
    #[strum(serialize = "flip")]
    NpFlip,
    /// `numpy.fliplr()` function
    #[strum(serialize = "fliplr")]
    NpFliplr,
    /// `numpy.flipud()` function
    #[strum(serialize = "flipud")]
    NpFlipud,
    /// `numpy.roll()` function
    #[strum(serialize = "roll")]
    NpRoll,
    /// `numpy.expand_dims()` function
    #[strum(serialize = "expand_dims")]
    NpExpandDims,
    /// `numpy.squeeze()` function
    #[strum(serialize = "squeeze")]
    NpSqueeze,
    /// `numpy.delete()` function
    #[strum(serialize = "delete")]
    NpDelete,
    /// `numpy.diag()` function
    #[strum(serialize = "diag")]
    NpDiag,
    /// `numpy.diagflat()` function
    #[strum(serialize = "diagflat")]
    NpDiagflat,
    /// `numpy.diagonal()` function
    #[strum(serialize = "diagonal")]
    NpDiagonal,
    /// `numpy.trace()` function
    #[strum(serialize = "trace")]
    NpTrace,
    /// `numpy.flatnonzero()` function
    #[strum(serialize = "flatnonzero")]
    NpFlatnonzero,
    /// `numpy.asarray()` function
    #[strum(serialize = "asarray")]
    NpAsarray,
    /// `numpy.asarray_chkfinite()` finite-checking array conversion helper.
    #[strum(serialize = "asarray_chkfinite")]
    NpAsarrayChkfinite,
    /// `numpy.ascontiguousarray()` contiguous array conversion helper.
    #[strum(serialize = "ascontiguousarray")]
    NpAscontiguousarray,
    /// `numpy.asfortranarray()` Fortran array conversion helper.
    #[strum(serialize = "asfortranarray")]
    NpAsfortranarray,
    /// `numpy.require()` array requirement helper.
    #[strum(serialize = "require")]
    NpRequire,
    /// `numpy.ix_()` open mesh index helper.
    #[strum(serialize = "ix_")]
    NpIx_,
    /// `numpy.mask_indices()` triangular mask index helper.
    #[strum(serialize = "mask_indices")]
    NpMaskIndices,
    /// `numpy.isfortran()` memory layout predicate.
    #[strum(serialize = "isfortran")]
    NpIsfortran,
    /// `numpy.may_share_memory()` conservative memory overlap predicate.
    #[strum(serialize = "may_share_memory")]
    NpMayShareMemory,
    /// `numpy.shares_memory()` exact memory overlap predicate.
    #[strum(serialize = "shares_memory")]
    NpSharesMemory,
    /// `numpy.column_stack()` function
    #[strum(serialize = "column_stack")]
    NpColumnStack,
    /// `numpy.row_stack()` function — alias for vstack
    #[strum(serialize = "row_stack")]
    NpRowStack,
    /// `numpy.hsplit()` function
    #[strum(serialize = "hsplit")]
    NpHsplit,
    /// `numpy.vsplit()` function
    #[strum(serialize = "vsplit")]
    NpVsplit,
    /// `numpy.dsplit()` function
    #[strum(serialize = "dsplit")]
    NpDsplit,
    /// `numpy.array_split()` function
    #[strum(serialize = "array_split")]
    NpArraySplit,
    /// `numpy.searchsorted()` function
    #[strum(serialize = "searchsorted")]
    NpSearchsorted,
    /// `numpy.lexsort()` indirect stable sorting helper.
    #[strum(serialize = "lexsort")]
    NpLexsort,
    /// `numpy.cov()` covariance helper.
    #[strum(serialize = "cov")]
    NpCov,
    /// `numpy.corrcoef()` correlation coefficient helper.
    #[strum(serialize = "corrcoef")]
    NpCorrcoef,
    /// `numpy.extract()` function
    #[strum(serialize = "extract")]
    NpExtract,
    /// `numpy.trim_zeros()` one-dimensional zero trimming helper.
    #[strum(serialize = "trim_zeros")]
    NpTrimZeros,
    /// `numpy.unwrap()` phase-unwrapping helper.
    #[strum(serialize = "unwrap")]
    NpUnwrap,
    /// `numpy.intersect1d()` function
    #[strum(serialize = "intersect1d")]
    NpIntersect1d,
    /// `numpy.union1d()` function
    #[strum(serialize = "union1d")]
    NpUnion1d,
    /// `numpy.setdiff1d()` function
    #[strum(serialize = "setdiff1d")]
    NpSetdiff1d,
    /// `numpy.setxor1d()` function
    #[strum(serialize = "setxor1d")]
    NpSetxor1d,
    /// `numpy.bincount()` function
    #[strum(serialize = "bincount")]
    NpBincount,
    /// `numpy.digitize()` function
    #[strum(serialize = "digitize")]
    NpDigitize,
    /// `numpy.matmul()` function
    #[strum(serialize = "matmul")]
    NpMatmul,
    /// `numpy.inner()` function
    #[strum(serialize = "inner")]
    NpInner,
    /// `numpy.outer()` function
    #[strum(serialize = "outer")]
    NpOuter,
    /// `numpy.vdot()` function
    #[strum(serialize = "vdot")]
    NpVdot,
    /// `numpy.vecdot()` function
    #[strum(serialize = "vecdot")]
    NpVecdot,
    /// `numpy.matvec()` function
    #[strum(serialize = "matvec")]
    NpMatvec,
    /// `numpy.vecmat()` function
    #[strum(serialize = "vecmat")]
    NpVecmat,
    /// `numpy.cross()` function
    #[strum(serialize = "cross")]
    NpCross,
    /// `numpy.kron()` Kronecker product helper.
    #[strum(serialize = "kron")]
    NpKron,
    /// `numpy.trapezoid()` function
    #[strum(serialize = "trapezoid")]
    NpTrapezoid,
    /// `numpy.vander()` function
    #[strum(serialize = "vander")]
    NpVander,
    /// `numpy.logspace()` function
    #[strum(serialize = "logspace")]
    NpLogspace,
    /// `numpy.geomspace()` function
    #[strum(serialize = "geomspace")]
    NpGeomspace,
    /// `numpy.tri()` function
    #[strum(serialize = "tri")]
    NpTri,
    /// `numpy.tril()` function
    #[strum(serialize = "tril")]
    NpTril,
    /// `numpy.triu()` function
    #[strum(serialize = "triu")]
    NpTriu,
    /// `numpy.identity()` function — alias for eye
    #[strum(serialize = "identity")]
    NpIdentity,
    /// `numpy.meshgrid()` function
    #[strum(serialize = "meshgrid")]
    NpMeshgrid,
    /// `numpy.full_like()` function
    #[strum(serialize = "full_like")]
    NpFullLike,
    /// `numpy.empty_like()` function
    #[strum(serialize = "empty_like")]
    NpEmptyLike,
    /// `numpy.gradient()` function
    #[strum(serialize = "gradient")]
    NpGradient,
    /// `numpy.convolve()` function
    #[strum(serialize = "convolve")]
    NpConvolve,
    /// `numpy.correlate()` function
    #[strum(serialize = "correlate")]
    NpCorrelate,
    /// `numpy.interp()` function — 1D interpolation
    #[strum(serialize = "interp")]
    NpInterp,
    /// `numpy.select()` function
    #[strum(serialize = "select")]
    NpSelect,
    /// `ndarray.item()` method — extract scalar from single-element array
    #[strum(serialize = "item")]
    NpItem,
    /// `numpy.take()` function / `ndarray.take()` method — take elements at indices
    #[strum(serialize = "take")]
    NpTake,
    /// `ndarray.fill()` method — fill array with value
    #[strum(serialize = "fill")]
    NpFill,
    /// `numpy.compress()` function / `ndarray.compress()` method — select elements by boolean condition
    #[strum(serialize = "compress")]
    NpCompress,
    /// `numpy.swapaxes()` function / `ndarray.swapaxes()` method
    #[strum(serialize = "swapaxes")]
    NpSwapaxes,
    /// `numpy.permute_dims()` function — permute ndarray axes
    #[strum(serialize = "permute_dims")]
    NpPermuteDims,
    /// `numpy.matrix_transpose()` function — swap the last two axes
    #[strum(serialize = "matrix_transpose")]
    NpMatrixTranspose,
    /// `numpy.moveaxis()` function — move axes to new positions
    #[strum(serialize = "moveaxis")]
    NpMoveaxis,
    /// `numpy.rollaxis()` function — roll one axis backward
    #[strum(serialize = "rollaxis")]
    NpRollaxis,
    /// `numpy.rot90()` function — rotate a 2-D array by quarter turns
    #[strum(serialize = "rot90")]
    NpRot90,
    /// `ndarray.nbytes` attribute
    #[strum(serialize = "nbytes")]
    NpNbytes,
    /// `ndarray.itemsize` attribute
    #[strum(serialize = "itemsize")]
    NpItemsize,
    /// `numpy.nancumsum()` function
    #[strum(serialize = "nancumsum")]
    NpNancumsum,
    /// `numpy.nancumprod()` function
    #[strum(serialize = "nancumprod")]
    NpNancumprod,

    // ==========================
    // gc module strings (only reachable when the `test-hooks` feature is enabled,
    // but interned unconditionally so the variant ordering — and therefore every
    // `StringId` used elsewhere — stays stable across feature combinations).
    /// Module name for `import gc`.
    Gc,
    /// `gc.collect()` function.
    Collect,
    /// `gc.disable()` function.
    Disable,
    /// `gc.enable()` function.
    Enable,
}

impl StaticStrings {
    /// Attempts to convert a `StringId` back to a `StaticStrings` variant.
    ///
    /// Returns `None` if the `StringId` doesn't correspond to a static string
    /// (e.g., it's an ASCII char or a dynamically interned string).
    pub fn from_string_id(id: StringId) -> Option<Self> {
        let enum_id = id.0.checked_sub(STATIC_STRING_ID_OFFSET)?;
        u16::try_from(enum_id).ok().and_then(Self::from_repr)
    }
}

/// Converts this static string variant to its corresponding `StringId`.
impl From<StaticStrings> for StringId {
    fn from(value: StaticStrings) -> Self {
        let string_id = value as u32;
        Self(string_id + STATIC_STRING_ID_OFFSET)
    }
}

impl From<StaticStrings> for Value {
    fn from(value: StaticStrings) -> Self {
        Self::InternString(value.into())
    }
}

impl PartialEq<StaticStrings> for StringId {
    fn eq(&self, other: &StaticStrings) -> bool {
        *self == Self::from(*other)
    }
}

impl PartialEq<StringId> for StaticStrings {
    fn eq(&self, other: &StringId) -> bool {
        StringId::from(*self) == *other
    }
}

/// Index into the bytes interner's storage.
///
/// Separate from `StringId` to distinguish string vs bytes literals at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BytesId(u32);

impl BytesId {
    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Index into the long integer interner's storage.
///
/// Used for integer literals that exceed i64 range. The actual `BigInt` values
/// are stored in the `Interns` table and looked up by index at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LongIntId(u32);

impl LongIntId {
    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Unique identifier for functions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FunctionId(u32);

impl FunctionId {
    /// Creates a FunctionId from a raw index value.
    ///
    /// Used by the bytecode VM to reconstruct FunctionIds from operands stored
    /// in bytecode. The caller is responsible for ensuring the index is valid.
    #[inline]
    pub fn from_index(index: u16) -> Self {
        Self(u32::from(index))
    }

    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A string, bytes, and long integer interner that stores unique values and returns indices for lookup.
///
/// Interns are deduplicated on insertion - interning the same string twice returns
/// the same `StringId`. Bytes and long integers are NOT deduplicated (rare enough that it's not worth it).
/// The interner owns all strings/bytes/long integers and provides lookup by index.
///
/// # Thread Safety
///
/// The interner is not thread-safe. It's designed to be used single-threaded during
/// parsing/preparation, then the values are accessed read-only during execution.
#[derive(Debug, Default, Clone)]
pub struct InternerBuilder {
    /// Maps strings to their indices for deduplication during interning.
    string_map: AHashMap<String, StringId>,
    /// Storage for interned interns, indexed by `StringId`.
    strings: Vec<String>,
    /// Storage for interned bytes literals, indexed by `BytesId`.
    /// Not deduplicated since bytes literals are rare.
    bytes: Vec<Vec<u8>>,
    /// Storage for interned long integer literals, indexed by `LongIntId`.
    /// Not deduplicated since long integer literals are rare.
    long_ints: Vec<BigInt>,
}

impl InternerBuilder {
    /// Creates a new string interner with pre-interned strings.
    ///
    /// Clones from a lazily-initialized base interner that contains all pre-interned
    /// strings (`<module>`, attribute names, ASCII chars). This avoids rebuilding
    /// the base set on every call.
    ///
    /// # Arguments
    /// * `code` - The code being parsed, used for a very rough guess at how many
    ///   additional strings will be interned beyond the base set.
    ///
    /// Pre-interns (via `BASE_INTERNER`):
    /// - Index 0: `"<module>"` for module-level code
    /// - Indices 1-MAX_ATTR_ID: Known attribute names (append, insert, get, join, etc.)
    /// - Indices MAX_ATTR_ID+1..: ASCII single-character strings
    pub fn new(code: &str) -> Self {
        // Reserve capacity for code-specific strings
        // Rough guess: count quotes and divide by 2 (open+close per string)
        let capacity = code.bytes().filter(|&b| b == b'"' || b == b'\'').count() >> 1;
        Self {
            string_map: AHashMap::with_capacity(capacity),
            strings: Vec::with_capacity(capacity),
            bytes: Vec::new(),
            long_ints: Vec::new(),
        }
    }

    /// Creates a builder pre-seeded from an existing [`Interns`] table.
    ///
    /// This is used by REPL incremental compilation: previously compiled interned
    /// values keep stable IDs, and newly interned values are appended.
    pub(crate) fn from_interns(interns: &Interns, code: &str) -> Self {
        let mut builder = Self::new(code);
        builder.strings.clone_from(&interns.strings);
        builder.bytes.clone_from(&interns.bytes);
        builder.long_ints.clone_from(&interns.long_ints);

        builder.string_map = builder
            .strings
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let id = StringId(
                    u32::try_from(INTERN_STRING_ID_OFFSET + index).expect("StringId overflow while seeding interner"),
                );
                (value.clone(), id)
            })
            .collect();
        builder
    }

    /// Interns a string, returning its `StringId`.
    ///
    /// * If the string is ascii, return the pre-interned string id
    /// * If the string is a known static string, return the pre-interned string id
    /// * If the string was already interned, returns the existing string id
    /// * Otherwise, stores the string and returns a new string id
    pub fn intern(&mut self, s: &str) -> StringId {
        if s.len() == 1 {
            StringId::from_ascii(s.as_bytes()[0])
        } else if let Ok(ss) = StaticStrings::from_str(s) {
            ss.into()
        } else {
            *self.string_map.entry(s.to_owned()).or_insert_with(|| {
                let string_id = self.strings.len() + INTERN_STRING_ID_OFFSET;
                let id = StringId(string_id.try_into().expect("StringId overflow"));
                self.strings.push(s.to_owned());
                id
            })
        }
    }

    /// Interns bytes, returning its `BytesId`.
    ///
    /// Unlike interns, bytes are not deduplicated (bytes literals are rare).
    pub fn intern_bytes(&mut self, b: &[u8]) -> BytesId {
        let id = BytesId(self.bytes.len().try_into().expect("BytesId overflow"));
        self.bytes.push(b.to_vec());
        id
    }

    /// Interns a long integer, returning its `LongIntId`.
    ///
    /// Big integers are not deduplicated since literals exceeding i64 are rare.
    pub fn intern_long_int(&mut self, bi: BigInt) -> LongIntId {
        let id = LongIntId(self.long_ints.len().try_into().expect("LongIntId overflow"));
        self.long_ints.push(bi);
        id
    }

    /// Looks up a string by its `StringId`.
    #[inline]
    pub fn get_str(&self, id: StringId) -> &str {
        get_str(&self.strings, id)
    }
}

/// Looks up a string by its `StringId`.
///
/// # Panics
///
/// Panics if the `StringId` is invalid - not from this interner or ascii chars or StaticStrings.
fn get_str(strings: &[String], id: StringId) -> &str {
    if let Ok(c) = u8::try_from(id.0) {
        ASCII_STRS[c as usize]
    } else if let Some(intern_index) = id.index().checked_sub(INTERN_STRING_ID_OFFSET) {
        &strings[intern_index]
    } else {
        let static_str = StaticStrings::from_string_id(id).expect("Invalid static string ID");
        static_str.into()
    }
}

/// Read-only storage for interned strings, bytes, and long integers.
///
/// This provides lookup by `StringId`, `BytesId`, `LongIntId` and `FunctionId` for interned literals and functions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Interns {
    strings: Vec<String>,
    bytes: Vec<Vec<u8>>,
    long_ints: Vec<BigInt>,
    functions: Vec<Function>,
}

impl Interns {
    pub fn new(interner: InternerBuilder, functions: Vec<Function>) -> Self {
        Self {
            strings: interner.strings,
            bytes: interner.bytes,
            long_ints: interner.long_ints,
            functions,
        }
    }

    /// Looks up a string by its `StringId`.
    ///
    /// # Panics
    ///
    /// Panics if the `StringId` is invalid.
    #[inline]
    pub fn get_str(&self, id: StringId) -> &str {
        get_str(&self.strings, id)
    }

    /// Looks up bytes by their `BytesId`.
    ///
    /// # Panics
    ///
    /// Panics if the `BytesId` is invalid.
    #[inline]
    pub fn get_bytes(&self, id: BytesId) -> &[u8] {
        &self.bytes[id.index()]
    }

    /// Looks up a long integer by its `LongIntId`.
    ///
    /// # Panics
    ///
    /// Panics if the `LongIntId` is invalid.
    #[inline]
    pub fn get_long_int(&self, id: LongIntId) -> &BigInt {
        &self.long_ints[id.index()]
    }

    /// Lookup a function by its `FunctionId`
    ///
    /// # Panics
    ///
    /// Panics if the `FunctionId` is invalid.
    #[inline]
    pub fn get_function(&self, id: FunctionId) -> &Function {
        self.functions.get(id.index()).expect("Function not found")
    }

    /// Looks up the `StringId` for a string, checking ASCII, static strings, and interned strings.
    ///
    /// This is the reverse of `get_str`: given a string, find its StringId.
    /// Used when the host provides a name (e.g., from a NameLookup response) that was
    /// previously interned during preparation.
    ///
    /// Error if the string was never interned.
    pub fn get_string_id_by_name(&self, s: &str) -> Option<StringId> {
        // Check single ASCII char
        if s.len() == 1 {
            return Some(StringId::from_ascii(s.as_bytes()[0]));
        }
        // Check static strings
        if let Ok(ss) = StaticStrings::from_str(s) {
            return Some(ss.into());
        }
        // Check interned strings
        for (i, interned) in self.strings.iter().enumerate() {
            if interned == s {
                return u32::try_from(INTERN_STRING_ID_OFFSET + i).ok().map(StringId);
            }
        }
        None
    }

    /// Sets the compiled functions.
    ///
    /// This is called after compilation to populate the functions that were
    /// compiled from `PreparedFunctionDef` nodes.
    pub fn set_functions(&mut self, functions: Vec<Function>) {
        self.functions = functions;
    }

    /// Returns a clone of the compiled function table.
    ///
    /// Used by REPL incremental compilation to preserve existing function IDs.
    pub(crate) fn functions_clone(&self) -> Vec<Function> {
        self.functions.clone()
    }
}
