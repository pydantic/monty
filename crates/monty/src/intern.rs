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
//! * 0 to 127 - single character strings for all 128 ASCII characters
//! * 128+ - strings interned per executor
//!
//! Static strings occupy ordinary executor-local slots. Their interner entries
//! retain a [`StaticStrings`] tag for dispatch, while snapshots serialize only
//! their text so another build can load an unknown static string as owned text.

use std::{slice::from_ref, str::FromStr};

use ahash::AHashMap;
use num_bigint::BigInt;
use strum::{EnumCount, EnumString, IntoStaticStr};

#[cfg(feature = "test-hooks")]
use crate::function::FunctionMetadataFault;
use crate::{
    function::Function,
    hash::{ASCII_HASHES, HashValue, WithHash, hash_python_str},
    modules::restore_interned_strings,
};

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
    pub const fn from_ascii(byte: u8) -> Self {
        Self(byte as u32)
    }
}

/// Executor-local intern IDs follow the globally reserved ASCII IDs.
const INTERN_STRING_ID_OFFSET: usize = ASCII_STRS.len();

/// Strings runtime paths can materialize without a corresponding source name.
const CORE_STATIC_STRINGS: &[StaticStrings] = &[
    StaticStrings::EmptyString,
    StaticStrings::Module,
    StaticStrings::NoneRepr,
    StaticStrings::TrueRepr,
    StaticStrings::FalseRepr,
    StaticStrings::EllipsisRepr,
    StaticStrings::NotImplementedRepr,
    StaticStrings::DunderMain,
    StaticStrings::DunderDoc,
];

/// Static strings for all 128 ASCII characters.
///
/// Exposed `pub(crate)` so the [`crate::hash::ASCII_HASHES`] table can hash
/// them in lockstep — both tables must agree on the same `&str` per byte.
pub(crate) static ASCII_STRS: [&str; 128] = const {
    // Initialize array of 128 bytes which will be used as the raw storage
    const ASCII_BYTES: [u8; 128] = const {
        let mut bytes: [u8; 128] = [0; 128];
        let mut i: u8 = 0;
        while i < 128 {
            bytes[i as usize] = i;
            i += 1;
        }
        bytes
    };
    // Index into the above array to build the `&'static str` forms
    let mut strs: [&str; 128] = [""; 128];
    let mut i = 0;
    while i < 128 {
        strs[i] = match str::from_utf8(from_ref(&ASCII_BYTES[i])) {
            Ok(s) => s,
            Err(_) => panic!("invalid ascii byte"),
        };
        i += 1;
    }
    strs
};

/// Static string values known at compile time.
///
/// The discriminant is an in-process dispatch detail, never a `StringId` or
/// snapshot identity. Interner entries serialize as text and recover this tag
/// only when the loading build recognizes that text.
#[repr(u16)]
#[derive(Debug, Clone, Copy, EnumCount, EnumString, IntoStaticStr, PartialEq, Eq, Hash)]
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
    Obj,
    Object,
    Source,
    Base,
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
    #[strum(serialize = "__enter__")]
    Enter,
    #[strum(serialize = "__exit__")]
    Exit,

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
    AppendText,
    AppendBytes,
    Mkdir,
    Unlink,
    Rmdir,
    Rename,

    // Path.open(): wraps the same `OsFunction::Open` round-trip as the
    // `open()` builtin. Handled in `Path::py_call_attr` with custom
    // mode/kwarg validation (so it cannot go through the generic
    // `OsFunction::try_from(StaticStrings)` short-circuit).
    Open,

    // ==========================
    // File object methods and attributes
    Read,
    Write,
    Close,
    Flush,
    Readable,
    Writable,
    Seekable,
    Readline,
    Readlines,
    Tell,
    Seek,
    Closed,
    Mode,
    Encoding,
    File,
    Buffering,
    Errors,
    Newline,
    Closefd,
    Opener,
    Repl,
    Old,
    New,

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
    Fold,
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
    // round() kwargs
    Number,
    Ndigits,
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

    // ==========================
    // Kwarg names referenced by `#[derive(FromArgs)]` macros and the
    // hand-written argument extractors they're gradually replacing.
    // These exist purely as `StaticStrings` so the generated dispatch
    // code can use `StringId` equality (O(1)) instead of string compare.
    /// Kwarg name `key` — `sorted(key=...)`, `min(key=...)`, etc.
    Key,
    /// Kwarg name `sep` — `str.split(sep=...)`, `print(sep=...)`, etc.
    Sep,
    /// Kwarg name `maxsplit` — `str.split(maxsplit=...)`, `re.split(maxsplit=...)`.
    Maxsplit,
    /// Kwarg name `strict` — `zip(strict=...)`.
    Strict,
    /// Kwarg name `return_exceptions` — `asyncio.gather(return_exceptions=...)`.
    ReturnExceptions,
    /// Kwarg name `rel_tol` — `math.isclose(rel_tol=...)`.
    RelTol,
    /// Kwarg name `abs_tol` — `math.isclose(abs_tol=...)`.
    AbsTol,
    /// Kwarg name `format` — `date.strftime(format=...)`, `datetime.strftime(format=...)`.
    Format,
    /// Kwarg name `parents` — `Path.mkdir(parents=...)`.
    Parents,
    /// Kwarg name `exist_ok` — `Path.mkdir(exist_ok=...)`.
    ExistOk,

    // ==========================
    // sys module test-hook strings (kept interned unconditionally for the
    // same StringId-stability reason as the gc entries above).
    /// `sys.setrecursionlimit()` function (only callable under `test-hooks`).
    Setrecursionlimit,

    // ==========================
    // unicodedata module strings. The `name()` function reuses the existing
    // `Name` variant (both intern to "name").
    /// Module name for `import unicodedata`.
    Unicodedata,
    /// `unicodedata.normalize()` function.
    Normalize,
    /// `unicodedata.is_normalized()` function.
    #[strum(serialize = "is_normalized")]
    IsNormalized,
    /// `unicodedata.category()` function.
    Category,
    /// `unicodedata.lookup()` function.
    Lookup,
    /// `unicodedata.combining()` function.
    Combining,
    /// `unicodedata.unidata_version` constant.
    #[strum(serialize = "unidata_version")]
    UnidataVersion,

    // ==========================
    // Module dunder values.
    #[strum(serialize = "__main__")]
    DunderMain,

    // ==========================
    // Class dunder attributes.
    /// `__doc__` — synthesized into the namespace of classes created by the
    /// 3-arg `type()` builtin when the caller's dict omits it.
    #[strum(serialize = "__doc__")]
    DunderDoc,

    // ==========================
    // Singleton `repr()`/`str()` values. Interned so `str(None)`, `repr(True)`,
    // `f"{...}"`, `print(False)` etc. resolve to an existing `StringId` instead
    // of allocating a fresh heap string each time — see `Value::py_repr`.
    #[strum(serialize = "None")]
    NoneRepr,
    #[strum(serialize = "True")]
    TrueRepr,
    #[strum(serialize = "False")]
    FalseRepr,
    #[strum(serialize = "Ellipsis")]
    EllipsisRepr,

    // ==========================
    // os module function/constant names. Constants reuse existing variants where the text
    // already exists (`Sep` in the kwarg section, `Name`, single-char ASCII
    // ids for `/`, `.`, `\n`).
    /// `os.listdir()` function.
    Listdir,
    /// `os.makedirs()` function.
    Makedirs,
    /// `os.fspath()` function — distinct from `Fspath` (`__fspath__`).
    #[strum(serialize = "fspath")]
    OsFspath,
    /// `os.altsep` constant name.
    Altsep,
    /// `os.extsep` constant name.
    Extsep,
    /// `os.curdir` constant name.
    Curdir,
    /// `os.pardir` constant name.
    Pardir,
    /// `os.linesep` constant name.
    Linesep,
    /// `os.devnull` constant name.
    Devnull,
    /// Value of `os.name`.
    Posix,
    /// Value of `os.pardir`.
    #[strum(serialize = "..")]
    ParentDirString,
    /// Value of `os.devnull`.
    #[strum(serialize = "/dev/null")]
    DevNullString,
    /// Kwarg name `path` — `os.listdir(path=...)`, `os.stat(path=...)`, etc.
    Path,
    /// Kwarg name `dir_fd` — `os.stat(dir_fd=...)`, `os.mkdir(dir_fd=...)`, etc.
    DirFd,
    /// Kwarg name `follow_symlinks` — `os.stat(follow_symlinks=...)`.
    FollowSymlinks,
    /// Kwarg name `src` — `os.rename(src=...)`, `os.replace(src=...)`.
    Src,
    /// Kwarg name `dst` — `os.rename(dst=...)`, `os.replace(dst=...)`.
    Dst,
    /// Kwarg name `src_dir_fd` — `os.rename(src_dir_fd=...)`.
    SrcDirFd,
    /// Kwarg name `dst_dir_fd` — `os.rename(dst_dir_fd=...)`.
    DstDirFd,

    // itertools module strings; `count`, `start`, `step` and `object` reuse the
    // existing variants of the same name.
    /// Module name for `import itertools`.
    Itertools,
    /// `itertools.repeat()` function.
    Repeat,
    /// `times` keyword argument of `itertools.repeat()`.
    Times,

    // ==========================
    // dataclasses module strings.
    /// Module name for `import dataclasses`.
    Dataclasses,
    /// `dataclasses.dataclass` decorator.
    Dataclass,
    /// `dataclasses.is_dataclass()` function.
    IsDataclass,
    /// The `__dataclass_fields__` class attribute `@dataclass` writes: the
    /// name -> `Field` mapping that drives every synthesized dunder.
    #[strum(serialize = "__dataclass_fields__")]
    DataclassFields,

    // ==========================
    // collections module strings.
    /// Module name for `import collections`.
    Collections,
    /// The `collections.deque` type.
    Deque,
    /// `deque.appendleft()` method.
    Appendleft,
    /// `deque.extendleft()` method.
    Extendleft,
    /// `deque.popleft()` method.
    Popleft,
    /// `deque.rotate()` method.
    Rotate,
    /// `deque.maxlen` attribute (also a constructor keyword argument).
    Maxlen,
    /// `deque(iterable=...)` — the constructor's first parameter, which CPython
    /// also accepts by keyword. Distinct from [`Self::Iterable`], which is the
    /// capitalized `typing.Iterable`.
    #[strum(serialize = "iterable")]
    IterableArg,
    /// The `collections.namedtuple` factory function.
    Namedtuple,
    /// The `collections.defaultdict` factory function.
    Defaultdict,
    /// The `collections.Counter` type/factory.
    #[strum(serialize = "Counter")]
    Counter,
    /// `Counter.most_common()` method.
    #[strum(serialize = "most_common")]
    MostCommon,
    /// `Counter.elements()` method.
    Elements,
    /// `Counter.total()` method.
    Total,
    /// `Counter.subtract()` method.
    Subtract,
    /// `namedtuple(typename=...)` keyword argument.
    Typename,
    /// `namedtuple(field_names=...)` keyword argument.
    #[strum(serialize = "field_names")]
    FieldNames,
    /// `NamedTuple._fields` — tuple of field names.
    #[strum(serialize = "_fields")]
    UnderFields,
    /// `NamedTuple._field_defaults` — dict of defaulted field names to values.
    #[strum(serialize = "_field_defaults")]
    UnderFieldDefaults,
    /// `NamedTuple._make(iterable)` classmethod.
    #[strum(serialize = "_make")]
    UnderMake,
    /// `NamedTuple._replace(**kwargs)` method.
    #[strum(serialize = "_replace")]
    UnderReplace,
    /// `NamedTuple._asdict()` method.
    #[strum(serialize = "_asdict")]
    UnderAsdict,
    /// `namedtuple(..., defaults=...)` keyword argument.
    Defaults,
    /// `namedtuple(..., module=...)` keyword argument.
    #[strum(serialize = "module")]
    ModuleKwarg,
    /// `defaultdict.default_factory` attribute.
    #[strum(serialize = "default_factory")]
    DefaultFactory,
    /// `defaultdict.__missing__` method.
    #[strum(serialize = "__missing__")]
    DunderMissing,
    /// `__module__` — the defining module name, exposed on namedtuple classes.
    #[strum(serialize = "__module__")]
    DunderModule,
    /// `__getnewargs__` — the copy/pickle hook on named tuples.
    #[strum(serialize = "__getnewargs__")]
    DunderGetnewargs,
    /// `__qualname__` — the qualified class name, exposed on namedtuple classes.
    #[strum(serialize = "__qualname__")]
    DunderQualname,

    // ==========================
    // More itertools module strings.
    /// `itertools.pairwise()` function.
    Pairwise,
    /// `itertools.compress()` function.
    Compress,
    /// `data` keyword argument of `itertools.compress()`.
    Data,
    /// `selectors` keyword argument of `itertools.compress()`.
    Selectors,
    /// `itertools.islice()` function.
    Islice,
    /// `itertools.chain()` function.
    Chain,
    /// `itertools.cycle()` function.
    Cycle,
    /// Python's `NotImplemented` singleton representation.
    #[strum(serialize = "NotImplemented")]
    NotImplementedRepr,
    /// The `__dataclass_params__` class attribute `@dataclass` writes: the
    /// options the class was decorated with.
    #[strum(serialize = "__dataclass_params__")]
    DataclassParams,
    // `@dataclass(...)` keyword options. Recognised even where unimplemented,
    // so an unsupported option reports itself rather than looking misspelled.
    /// `@dataclass(init=...)`.
    Init,
    /// `@dataclass(eq=...)`.
    Eq,
    /// `@dataclass(repr=...)`.
    Repr,
    /// `@dataclass(order=...)`.
    Order,
    /// `@dataclass(unsafe_hash=...)`.
    UnsafeHash,
    /// `@dataclass(frozen=...)`.
    Frozen,
    /// `@dataclass(match_args=...)`.
    MatchArgs,
    /// `@dataclass(kw_only=...)`.
    KwOnly,
    /// `@dataclass(slots=...)`.
    Slots,
    /// `@dataclass(weakref_slot=...)`.
    WeakrefSlot,
    /// `dataclasses.FrozenInstanceError` exception.
    #[strum(serialize = "FrozenInstanceError")]
    FrozenInstanceError,
    /// The class parameter of the decorator `@dataclass(...)` returns, which
    /// CPython spells `def wrap(cls)` and so accepts by keyword.
    Cls,
    /// `itertools.takewhile()` function.
    Takewhile,
    /// `itertools.dropwhile()` function.
    Dropwhile,
    /// `itertools.filterfalse()` function.
    Filterfalse,
    /// `itertools.starmap()` function.
    Starmap,

    // ==========================
    // functools module strings.
    /// Module name for `import functools`.
    Functools,
    /// `functools.reduce()` function.
    Reduce,
    /// `initial` keyword argument of `functools.reduce()`.
    Initial,

    // ==========================
    // base64 and binascii module strings
    // Each spells its text out: snake_case would split the digits (`b64_encode`).
    /// Module name for `import base64`.
    #[strum(serialize = "base64")]
    Base64,
    /// `base64.b64encode()` function.
    #[strum(serialize = "b64encode")]
    B64Encode,
    /// `base64.b64decode()` function.
    #[strum(serialize = "b64decode")]
    B64Decode,
    /// `base64.standard_b64encode()` function.
    #[strum(serialize = "standard_b64encode")]
    StandardB64Encode,
    /// `base64.standard_b64decode()` function.
    #[strum(serialize = "standard_b64decode")]
    StandardB64Decode,
    /// `base64.urlsafe_b64encode()` function.
    #[strum(serialize = "urlsafe_b64encode")]
    UrlsafeB64Encode,
    /// `base64.urlsafe_b64decode()` function.
    #[strum(serialize = "urlsafe_b64decode")]
    UrlsafeB64Decode,
    /// `base64.b32encode()` function.
    #[strum(serialize = "b32encode")]
    B32Encode,
    /// `base64.b32decode()` function.
    #[strum(serialize = "b32decode")]
    B32Decode,
    /// `base64.b32hexencode()` function.
    #[strum(serialize = "b32hexencode")]
    B32HexEncode,
    /// `base64.b32hexdecode()` function.
    #[strum(serialize = "b32hexdecode")]
    B32HexDecode,
    /// `base64.b16encode()` function.
    #[strum(serialize = "b16encode")]
    B16Encode,
    /// `base64.b16decode()` function.
    #[strum(serialize = "b16decode")]
    B16Decode,
    /// `base64.encodebytes()` function.
    #[strum(serialize = "encodebytes")]
    Encodebytes,
    /// `base64.decodebytes()` function.
    #[strum(serialize = "decodebytes")]
    Decodebytes,
    /// `altchars` parameter of `base64.b64encode()` / `b64decode()`.
    #[strum(serialize = "altchars")]
    Altchars,
    /// `validate` parameter of `base64.b64decode()`.
    #[strum(serialize = "validate")]
    Validate,
    /// `map01` parameter of `base64.b32decode()`.
    #[strum(serialize = "map01")]
    Map01,
    /// Module name for `import binascii`.
    #[strum(serialize = "binascii")]
    Binascii,
    /// `binascii.Error` exception class — distinct from [`Self::Error`], which
    /// is the lowercase `re.error` alias.
    #[strum(serialize = "Error")]
    ErrorClass,
    /// `base64.MAXBINSIZE` module constant.
    #[strum(serialize = "MAXBINSIZE")]
    MaxBinSize,
    /// `base64.MAXLINESIZE` module constant.
    #[strum(serialize = "MAXLINESIZE")]
    MaxLineSize,
    /// `base64.b85encode()` function.
    #[strum(serialize = "b85encode")]
    B85Encode,
    /// `base64.b85decode()` function.
    #[strum(serialize = "b85decode")]
    B85Decode,
    /// `base64.z85encode()` function.
    #[strum(serialize = "z85encode")]
    Z85Encode,
    /// `base64.z85decode()` function.
    #[strum(serialize = "z85decode")]
    Z85Decode,
    /// `binascii.hexlify()` function.
    #[strum(serialize = "hexlify")]
    Hexlify,
    /// `binascii.unhexlify()` function.
    #[strum(serialize = "unhexlify")]
    Unhexlify,
    /// `binascii.b2a_hex()` function, an alias of `hexlify`.
    #[strum(serialize = "b2a_hex")]
    B2aHex,
    /// `binascii.a2b_hex()` function, an alias of `unhexlify`.
    #[strum(serialize = "a2b_hex")]
    A2bHex,
    /// `binascii.b2a_base64()` function.
    #[strum(serialize = "b2a_base64")]
    B2aBase64,
    /// `binascii.a2b_base64()` function.
    #[strum(serialize = "a2b_base64")]
    A2bBase64,
    /// `binascii.crc32()` function.
    #[strum(serialize = "crc32")]
    Crc32,
    /// `pad` parameter of `base64.b85encode()`.
    #[strum(serialize = "pad")]
    Pad,
    /// `bytes_per_sep` parameter of `binascii.hexlify()`.
    #[strum(serialize = "bytes_per_sep")]
    BytesPerSep,
    /// `strict_mode` parameter of `binascii.a2b_base64()`.
    #[strum(serialize = "strict_mode")]
    StrictMode,
    /// `crc` parameter of `binascii.crc32()`.
    #[strum(serialize = "crc")]
    Crc,
    /// `hexstr` parameter of `binascii.unhexlify()`.
    #[strum(serialize = "hexstr")]
    Hexstr,

    /// `datetime.time` class name.
    Time,
    /// `datetime.timetz` method name.
    Timetz,
    /// `utcoffset()` method of `time`, `datetime` and `timezone`.
    Utcoffset,
    /// `tzname()` method of `time`, `datetime` and `timezone`. (`dst()` reuses
    /// the `Dst` variant already interned for the `os` kwarg of the same name.)
    Tzname,
    /// `timespec` keyword of `time.isoformat()`.
    Timespec,
    /// `functools.partial` type.
    Partial,
    /// `partial.func` attribute.
    Func,
    /// `partial.keywords` attribute.
    Keywords,
    /// `base64.a85encode()` function.
    #[strum(serialize = "a85encode")]
    A85Encode,
    /// `base64.a85decode()` function.
    #[strum(serialize = "a85decode")]
    A85Decode,
    /// `foldspaces` parameter of `base64.a85encode()` / `a85decode()`.
    #[strum(serialize = "foldspaces")]
    Foldspaces,
    /// `wrapcol` parameter of `base64.a85encode()`.
    #[strum(serialize = "wrapcol")]
    Wrapcol,
    /// `adobe` parameter of `base64.a85encode()` / `a85decode()`.
    #[strum(serialize = "adobe")]
    Adobe,
    /// `ignorechars` parameter of `base64.a85decode()`.
    #[strum(serialize = "ignorechars")]
    Ignorechars,
}

/// One executor-local interned string and its precomputed Python hash.
///
/// Static entries borrow their text through [`StaticStrings`]; owned entries
/// retain source/runtime text unknown to the static registry. Both serialize as
/// plain text, making the tag an optimization rather than snapshot identity.
#[derive(Debug, Clone)]
enum InternedString {
    /// Text recognized by this build's static registry.
    Static(WithHash<StaticStrings>),
    /// Text owned by this executor's interner.
    Owned(WithHash<Box<str>>),
}

impl InternedString {
    /// Creates an entry for compile-time-known text.
    fn static_string(value: StaticStrings) -> Self {
        Self::Static(WithHash::for_static_str(value))
    }

    /// Creates an entry owning text not present in the static registry.
    fn owned(value: String) -> Self {
        Self::Owned(WithHash::for_boxed_str(value.into_boxed_str()))
    }

    /// Returns the interned text.
    fn as_str(&self) -> &str {
        match self {
            Self::Static(value) => (*value.value()).into(),
            Self::Owned(value) => value.value(),
        }
    }

    /// Returns the cached Python hash.
    fn hash(&self) -> HashValue {
        match self {
            Self::Static(value) => value.hash(),
            Self::Owned(value) => value.hash(),
        }
    }

    /// Returns the static tag when this build recognizes the text.
    fn static_value(&self) -> Option<StaticStrings> {
        match self {
            Self::Static(value) => Some(*value.value()),
            Self::Owned(_) => None,
        }
    }
}

impl serde::Serialize for InternedString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(self.as_str(), serializer)
    }
}

impl<'de> serde::Deserialize<'de> for InternedString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(match StaticStrings::from_str(&value) {
            Ok(static_string) => Self::static_string(static_string),
            Err(_) => Self::owned(value),
        })
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
#[derive(Debug, Clone)]
pub struct InternerBuilder {
    /// Maps owned strings to their executor-local IDs.
    string_map: AHashMap<String, StringId>,
    /// Maps static tags to the executor-local IDs allocated on first use.
    static_string_ids: Vec<Option<StringId>>,
    /// Storage for all non-ASCII interned strings, indexed by `StringId`.
    strings: Vec<InternedString>,
    /// Storage for interned bytes literals, indexed by `BytesId`. Each
    /// entry carries its precomputed [`HashValue`].
    /// Not deduplicated since bytes literals are rare.
    bytes: Vec<WithHash<Vec<u8>>>,
    /// Storage for interned long integer literals, indexed by `LongIntId`.
    /// Each entry carries its precomputed [`HashValue`].
    /// Not deduplicated since long integer literals are rare.
    long_ints: Vec<WithHash<BigInt>>,
}

impl Default for InternerBuilder {
    fn default() -> Self {
        Self::new("")
    }
}

impl InternerBuilder {
    /// Creates an interner containing the small set of strings any execution
    /// may materialize without an explicit source reference.
    ///
    /// Other static strings receive ordinary slots only when parsing encounters
    /// them or an imported module registers the attributes it materializes.
    /// `code` supplies a rough capacity estimate for additional literals.
    pub fn new(code: &str) -> Self {
        // Reserve capacity for code-specific strings
        // Rough guess: count quotes and divide by 2 (open+close per string)
        let capacity = code.bytes().filter(|&b| b == b'"' || b == b'\'').count() >> 1;
        let mut interner = Self {
            string_map: AHashMap::with_capacity(capacity),
            static_string_ids: vec![None; StaticStrings::COUNT],
            strings: Vec::with_capacity(capacity),
            bytes: Vec::new(),
            long_ints: Vec::new(),
        };
        for &value in CORE_STATIC_STRINGS {
            interner.intern_static(value);
        }
        interner
    }

    /// Interns a string, returning its `StringId`.
    ///
    /// ASCII characters use their reserved IDs. All other strings are
    /// deduplicated in the executor-local table; recognized static text stores
    /// a compact tag rather than an owned allocation.
    pub fn intern(&mut self, s: &str) -> StringId {
        intern_str(&mut self.string_map, &mut self.static_string_ids, &mut self.strings, s)
    }

    /// Interns a known static string into this executor's ordinary ID space.
    pub(crate) fn intern_static(&mut self, value: StaticStrings) -> StringId {
        intern_static(&mut self.static_string_ids, &mut self.strings, value)
    }

    /// Looks up the `StringId` for an ASCII character or previously interned string.
    ///
    /// Mirrors [`Interns::get_string_id_by_name`] so the compiler can resolve
    /// builtin names before the runtime table is built.
    pub fn get_string_id_by_name(&self, s: &str) -> Option<StringId> {
        get_string_id_by_name(&self.string_map, &self.static_string_ids, s)
    }

    /// Interns bytes, returning its `BytesId`.
    ///
    /// Unlike interns, bytes are not deduplicated (bytes literals are rare).
    pub fn intern_bytes(&mut self, b: &[u8]) -> BytesId {
        let id = BytesId(self.bytes.len().try_into().expect("BytesId overflow"));
        self.bytes.push(WithHash::for_bytes(b.to_vec()));
        id
    }

    /// Interns a long integer, returning its `LongIntId`.
    ///
    /// Big integers are not deduplicated since literals exceeding i64 are rare.
    pub fn intern_long_int(&mut self, bi: BigInt) -> LongIntId {
        let id = LongIntId(self.long_ints.len().try_into().expect("LongIntId overflow"));
        self.long_ints.push(WithHash::for_long_int(bi));
        id
    }

    /// Looks up a string by its `StringId`.
    #[inline]
    pub fn get_str(&self, id: StringId) -> &str {
        get_str(&self.strings, id)
    }

    /// Returns the static tag stored in an executor-local string slot.
    pub(crate) fn static_string(&self, id: StringId) -> Option<StaticStrings> {
        get_static_string(&self.strings, id)
    }
}

/// Interns `s` into the executor-local string table.
///
/// ASCII remains globally addressable; every other string receives an ordinary
/// dense interner slot. Static text retains a tag in that slot rather than
/// encoding the tag in its `StringId`.
fn intern_str(
    string_map: &mut AHashMap<String, StringId>,
    static_string_ids: &mut [Option<StringId>],
    strings: &mut Vec<InternedString>,
    s: &str,
) -> StringId {
    if s.len() == 1 {
        StringId::from_ascii(s.as_bytes()[0])
    } else if let Ok(value) = StaticStrings::from_str(s) {
        intern_static(static_string_ids, strings, value)
    } else {
        *string_map.entry(s.to_owned()).or_insert_with(|| {
            let id = next_string_id(strings.len());
            strings.push(InternedString::owned(s.to_owned()));
            id
        })
    }
}

/// Interns a static tag into an append-only executor-local table.
fn intern_static(
    static_string_ids: &mut [Option<StringId>],
    strings: &mut Vec<InternedString>,
    value: StaticStrings,
) -> StringId {
    if let Some(id) = static_string_ids[value as usize] {
        id
    } else {
        let id = next_string_id(strings.len());
        strings.push(InternedString::static_string(value));
        static_string_ids[value as usize] = Some(id);
        id
    }
}

/// Returns the next dense executor-local string ID.
fn next_string_id(strings_len: usize) -> StringId {
    let index = strings_len + INTERN_STRING_ID_OFFSET;
    StringId(index.try_into().expect("StringId overflow"))
}

/// Reverse of [`get_str`]: the `StringId` for `s`, or `None` if never interned.
fn get_string_id_by_name(
    string_map: &AHashMap<String, StringId>,
    static_string_ids: &[Option<StringId>],
    s: &str,
) -> Option<StringId> {
    if s.len() == 1 {
        Some(StringId::from_ascii(s.as_bytes()[0]))
    } else if let Ok(value) = StaticStrings::from_str(s) {
        static_string_ids[value as usize]
    } else {
        string_map.get(s).copied()
    }
}

/// Looks up a string by its `StringId`.
///
/// # Panics
///
/// Panics if the ID is neither ASCII nor a slot in this interner.
fn get_str(strings: &[InternedString], id: StringId) -> &str {
    if let Some(ascii_str) = ASCII_STRS.get(id.index()) {
        ascii_str
    } else {
        strings[id.index() - INTERN_STRING_ID_OFFSET].as_str()
    }
}

/// Returns the static tag stored at `id`, if any.
fn get_static_string(strings: &[InternedString], id: StringId) -> Option<StaticStrings> {
    id.index()
        .checked_sub(INTERN_STRING_ID_OFFSET)
        .and_then(|index| strings.get(index))
        .and_then(InternedString::static_value)
}

/// Storage for interned strings, bytes, long integers and compiled functions.
///
/// This provides lookup by `StringId`, `BytesId`, `LongIntId` and `FunctionId` for interned literals and functions.
///
/// # Append-only ownership in the REPL
///
/// Ids are stable and only ever appended, so a REPL session never copies this
/// table: it hands it to each snippet via [`into_builder`](Self::into_builder)
/// (or extends it in place with [`intern`](Self::intern)) and takes the extended
/// table back afterwards — whether the snippet succeeded or not.
///
/// # Hash tables
///
/// String entries wrap either a static tag or owned text in [`WithHash`]; bytes
/// and long integers use `WithHash` directly. Hashes are populated eagerly at
/// intern/load time, making the runtime hash methods plain index lookups.
///
/// # Reverse string lookup
///
/// [`get_string_id_by_name`](Self::get_string_id_by_name) returns the
/// `StringId` for a host-supplied `&str`. Owned text uses an in-memory reverse
/// map; static tags use a compact parallel ID table. Both are rebuilt
/// deterministically after deserialization. REPL hot paths
/// such as [`MontyRepl::call_function`](crate::MontyRepl::call_function)
/// and [`MontyRepl::has_function`](crate::MontyRepl::has_function) call this
/// per host-supplied name, so the lookup must be O(1) — not the previous
/// linear scan over `strings`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "InternsWire")]
pub(crate) struct Interns {
    strings: Vec<InternedString>,
    bytes: Vec<WithHash<Vec<u8>>>,
    long_ints: Vec<WithHash<BigInt>>,
    functions: Vec<Function>,
    /// Owned-text reverse lookup for [`Self::get_string_id_by_name`].
    #[serde(skip)]
    string_id_by_name: AHashMap<String, StringId>,
    /// Static-tag reverse lookup, rebuilt from `strings` after loading.
    #[serde(skip)]
    static_string_ids: Vec<Option<StringId>>,
}

impl Default for Interns {
    fn default() -> Self {
        Self::new(InternerBuilder::default(), Vec::new())
    }
}

/// Serialized form of [`Interns`]
#[derive(serde::Deserialize)]
struct InternsWire {
    strings: Vec<InternedString>,
    bytes: Vec<WithHash<Vec<u8>>>,
    long_ints: Vec<WithHash<BigInt>>,
    functions: Vec<Function>,
}

impl From<Interns> for InternsWire {
    fn from(interns: Interns) -> Self {
        Self {
            strings: interns.strings,
            bytes: interns.bytes,
            long_ints: interns.long_ints,
            functions: interns.functions,
        }
    }
}

impl TryFrom<InternsWire> for Interns {
    type Error = String;

    fn try_from(wire: InternsWire) -> Result<Self, Self::Error> {
        let (string_id_by_name, static_string_ids) = build_string_maps(&wire.strings)?;
        let mut interns = Self {
            strings: wire.strings,
            bytes: wire.bytes,
            long_ints: wire.long_ints,
            functions: wire.functions,
            string_id_by_name,
            static_string_ids,
        };
        for &value in CORE_STATIC_STRINGS {
            interns.intern_static(value);
        }
        restore_interned_strings(&mut interns);
        Ok(interns)
    }
}

/// Reverse maps rebuilt from the serialized ordered string table.
type StringMaps = (AHashMap<String, StringId>, Vec<Option<StringId>>);

/// Rebuilds both reverse maps from the canonical ordered string table.
///
/// Duplicate text is rejected because distinct IDs for equal interned strings
/// would invalidate the ID-equality fast path used by Python string equality.
fn build_string_maps(strings: &[InternedString]) -> Result<StringMaps, String> {
    let mut seen = AHashMap::with_capacity(strings.len());
    let mut string_id_by_name = AHashMap::new();
    let mut static_string_ids = vec![None; StaticStrings::COUNT];
    for (index, entry) in strings.iter().enumerate() {
        let id = next_string_id(index);
        if seen.insert(entry.as_str(), id).is_some() {
            return Err(format!("duplicate interned string {:?}", entry.as_str()));
        }
        if let Some(value) = entry.static_value() {
            static_string_ids[value as usize] = Some(id);
        } else {
            string_id_by_name.insert(entry.as_str().to_owned(), id);
        }
    }
    Ok((string_id_by_name, static_string_ids))
}

impl Interns {
    /// Builds the runtime table from a finished parse/prepare interner and the
    /// functions compiled against it.
    pub fn new(interner: InternerBuilder, functions: Vec<Function>) -> Self {
        // `InternerBuilder` already maintains the `String → StringId` map
        // during the parse/prepare phase to deduplicate `intern` calls;
        // we move it across so `Interns::get_string_id_by_name` doesn't
        // have to rebuild the same table from `strings`.
        Self {
            strings: interner.strings,
            bytes: interner.bytes,
            long_ints: interner.long_ints,
            functions,
            string_id_by_name: interner.string_map,
            static_string_ids: interner.static_string_ids,
        }
    }

    /// Inverse of [`new`](Self::new): moves the tables back into a builder so
    /// the next REPL snippet can parse against them, with the function table
    /// alongside for the compiler to extend. Nothing is copied or rehashed.
    pub(crate) fn into_builder(self) -> (InternerBuilder, Vec<Function>) {
        let builder = InternerBuilder {
            string_map: self.string_id_by_name,
            static_string_ids: self.static_string_ids,
            strings: self.strings,
            bytes: self.bytes,
            long_ints: self.long_ints,
        };
        (builder, self.functions)
    }

    /// Interns a string directly into the runtime table.
    ///
    /// Used by synthetic REPL inputs and lazily created runtime objects; IDs
    /// remain stable because the table is append-only.
    pub(crate) fn intern(&mut self, s: &str) -> StringId {
        intern_str(
            &mut self.string_id_by_name,
            &mut self.static_string_ids,
            &mut self.strings,
            s,
        )
    }

    /// Interns compile-time-known text directly into the append-only table.
    pub(crate) fn intern_static(&mut self, value: StaticStrings) -> StringId {
        intern_static(&mut self.static_string_ids, &mut self.strings, value)
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

    /// Returns the static tag stored in an executor-local string slot.
    pub(crate) fn static_string(&self, id: StringId) -> Option<StaticStrings> {
        get_static_string(&self.strings, id)
    }

    /// Returns this executor's ID for a static string registered at compile time.
    ///
    /// # Panics
    ///
    /// Panics if the compiler did not register a runtime-required string.
    pub(crate) fn static_id(&self, value: StaticStrings) -> StringId {
        self.static_string_ids[value as usize].unwrap_or_else(|| panic!("static string {value:?} was not registered"))
    }

    /// Looks up bytes by their `BytesId`.
    ///
    /// # Panics
    ///
    /// Panics if the `BytesId` is invalid.
    #[inline]
    pub fn get_bytes(&self, id: BytesId) -> &[u8] {
        self.bytes[id.index()].value()
    }

    /// Looks up a long integer by its `LongIntId`.
    ///
    /// # Panics
    ///
    /// Panics if the `LongIntId` is invalid.
    #[inline]
    pub fn get_long_int(&self, id: LongIntId) -> &BigInt {
        self.long_ints[id.index()].value()
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

    /// Injects `fault` into the named function's metadata.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn corrupt_function_metadata_for_tests(&mut self, name: &str, fault: FunctionMetadataFault) {
        let index = self
            .functions
            .iter()
            .position(|function| self.get_str(function.name.name_id) == name)
            .unwrap_or_else(|| panic!("test function '{name}' not found"));
        self.functions[index].corrupt_metadata_for_tests(fault);
    }

    /// Returns the Python hash for an interned string.
    ///
    /// ASCII hashes remain globally lazy. Every executor-local entry, static
    /// or owned, computes and stores its hash once when interned or loaded.
    ///
    /// All three paths must agree with [`hash_python_str`] applied to the
    /// underlying `&str` — interned and heap strings with equal contents
    /// must hash identically.
    ///
    /// # Panics
    ///
    /// Panics if the `StringId` is invalid (same as [`Self::get_str`]).
    #[inline]
    pub fn str_hash(&self, id: StringId) -> HashValue {
        if id.index() < ASCII_STRS.len() {
            ASCII_HASHES.get_or_compute(id.index(), || hash_python_str(ASCII_STRS[id.index()]))
        } else {
            self.strings[id.index() - INTERN_STRING_ID_OFFSET].hash()
        }
    }

    /// Returns the Python hash for interned bytes.
    ///
    /// Reads the [`HashValue`] from the corresponding [`WithHash`] entry
    /// (populated at intern time). Must agree with [`hash_python_bytes`]
    /// applied to the underlying `&[u8]`.
    ///
    /// # Panics
    ///
    /// Panics if the `BytesId` is invalid.
    #[inline]
    pub fn bytes_hash(&self, id: BytesId) -> HashValue {
        self.bytes[id.index()].hash()
    }

    /// Returns the Python hash for an interned long integer.
    ///
    /// Reads the [`HashValue`] from the corresponding [`WithHash`] entry
    /// (populated at intern time). Must agree with [`hash_python_long_int`].
    /// Note that interned long ints are only created for values that don't
    /// fit in `i64` (see `parse.rs`), so the `to_i64()` fast path inside
    /// `hash_python_long_int` is a defensive consistency guarantee rather
    /// than a hot path.
    ///
    /// # Panics
    ///
    /// Panics if the `LongIntId` is invalid.
    #[inline]
    pub fn long_int_hash(&self, id: LongIntId) -> HashValue {
        self.long_ints[id.index()].hash()
    }

    /// Looks up the executor-local `StringId` for previously interned text.
    ///
    /// This is the reverse of [`Self::get_str`]: given a string, find its
    /// `StringId`. The interned-string branch is O(1) via the
    /// `string_id_by_name` reverse map (built once at construction /
    /// deserialization), so the entire lookup stays O(1) regardless of how
    /// many strings have been interned.
    ///
    /// Used when the host provides a name (e.g., from a `NameLookup` response,
    /// [`MontyRepl::call_function`](crate::MontyRepl::call_function),
    /// [`MontyRepl::has_function`](crate::MontyRepl::has_function), or input
    /// injection) that was previously interned during preparation.
    ///
    /// Returns `None` if the string was never interned.
    pub fn get_string_id_by_name(&self, s: &str) -> Option<StringId> {
        get_string_id_by_name(&self.string_id_by_name, &self.static_string_ids, s)
    }
}
