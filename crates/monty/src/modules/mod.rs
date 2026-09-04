//! Built-in module implementations.
//!
//! This module provides implementations for Python built-in modules like `sys`, `typing`,
//! and `asyncio`. These are created on-demand when import statements are executed.

use std::fmt::{self, Write};

use strum::FromRepr;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    heap::HeapId,
    intern::{InternerBuilder, Interns, StaticStrings, StringId},
};

pub(crate) mod asyncio;
pub(crate) mod base64;
pub(crate) mod binascii;
pub(crate) mod collections;
pub(crate) mod dataclasses;
pub(crate) mod datetime;
pub(crate) mod functools;
#[cfg(feature = "test-hooks")]
pub(crate) mod gc;
pub(crate) mod itertools;
pub(crate) mod json;
pub(crate) mod math;
pub(crate) mod os;
pub(crate) mod pathlib;
pub(crate) mod re;
pub(crate) mod sys;
pub(crate) mod typing;
pub(crate) mod unicodedata;

/// Built-in modules that can be imported.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr)]
pub(crate) enum StandardLib {
    /// The `sys` module providing system-specific parameters and functions.
    Sys,
    /// The `typing` module providing type hints support.
    Typing,
    /// The `asyncio` module providing async/await support (only `run()` and `gather()` implemented).
    Asyncio,
    /// The `pathlib` module providing object-oriented filesystem paths.
    Pathlib,
    /// The `os` module providing operating system interface (only `getenv()` implemented).
    Os,
    /// The `math` module providing mathematical functions and constants.
    Math,
    /// The `json` module providing JSON parsing and serialization.
    Json,
    /// The `re` module providing regular expression matching.
    Re,
    /// The `datetime` module providing date and time types.
    Datetime,
    /// The `unicodedata` module providing Unicode Character Database access.
    Unicodedata,
    /// The `itertools` module providing lazy iterators (only `count` and
    /// `repeat` implemented).
    Itertools,
    /// The `dataclasses` module providing `@dataclass` and helpers.
    Dataclasses,
    /// The `collections` module providing container datatypes: `deque`,
    /// `namedtuple`, `defaultdict`, and `Counter`.
    Collections,
    /// The `functools` module providing `reduce` and `partial`.
    Functools,
    /// The `base64` module providing the base64/base32/base16 codecs.
    Base64,
    /// The `binascii` module providing binary-to-ASCII conversions, CRC32,
    /// and the `Error` class used by `base64`.
    Binascii,
    /// The `gc` module exposing a single `collect()` for tests. Only present
    /// under the `test-hooks` feature so production sandboxes never see it.
    ///
    /// Gated variants go last because theirs are the only ids allowed to move:
    /// ungated ids are baked into dumps as the `LoadModule` operand, while a
    /// `test-hooks` dump never leaves the build that wrote it. Append new
    /// modules ahead of this block; appending after ties their id to the feature.
    #[cfg(feature = "test-hooks")]
    Gc,
}

impl StandardLib {
    /// Every module available in this build.
    const ALL: &[Self] = &[
        Self::Sys,
        Self::Typing,
        Self::Asyncio,
        Self::Pathlib,
        Self::Os,
        Self::Math,
        Self::Json,
        Self::Re,
        Self::Datetime,
        Self::Unicodedata,
        Self::Itertools,
        Self::Dataclasses,
        Self::Collections,
        Self::Functools,
        Self::Base64,
        Self::Binascii,
        #[cfg(feature = "test-hooks")]
        Self::Gc,
    ];

    /// Resolves an importable module by its Python name.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "sys" => Some(Self::Sys),
            "typing" => Some(Self::Typing),
            "asyncio" => Some(Self::Asyncio),
            "pathlib" => Some(Self::Pathlib),
            "os" => Some(Self::Os),
            "math" => Some(Self::Math),
            "json" => Some(Self::Json),
            "re" => Some(Self::Re),
            "datetime" => Some(Self::Datetime),
            "unicodedata" => Some(Self::Unicodedata),
            "itertools" => Some(Self::Itertools),
            "dataclasses" => Some(Self::Dataclasses),
            "collections" => Some(Self::Collections),
            "functools" => Some(Self::Functools),
            "base64" => Some(Self::Base64),
            "binascii" => Some(Self::Binascii),
            #[cfg(feature = "test-hooks")]
            "gc" => Some(Self::Gc),
            _ => None,
        }
    }

    /// Returns the static tag for this module's import name.
    fn name(self) -> StaticStrings {
        match self {
            Self::Sys => StaticStrings::Sys,
            Self::Typing => StaticStrings::Typing,
            Self::Asyncio => StaticStrings::Asyncio,
            Self::Pathlib => StaticStrings::Pathlib,
            Self::Os => StaticStrings::Os,
            Self::Math => StaticStrings::Math,
            Self::Json => StaticStrings::Json,
            Self::Re => StaticStrings::Re,
            Self::Datetime => StaticStrings::Datetime,
            Self::Unicodedata => StaticStrings::Unicodedata,
            Self::Itertools => StaticStrings::Itertools,
            Self::Dataclasses => StaticStrings::Dataclasses,
            Self::Collections => StaticStrings::Collections,
            Self::Functools => StaticStrings::Functools,
            Self::Base64 => StaticStrings::Base64,
            Self::Binascii => StaticStrings::Binascii,
            #[cfg(feature = "test-hooks")]
            Self::Gc => StaticStrings::Gc,
        }
    }

    /// Registers strings materialized when this module object is created.
    pub(crate) fn intern_strings(self, interner: &mut InternerBuilder) {
        for &value in self.static_strings() {
            interner.intern_static(value);
        }
    }

    /// Returns strings stored by a module or needed by its dispatch tables.
    ///
    /// Keep each list in sync with every `StaticStrings` use in that module;
    /// deserialization also uses it to extend older append-only interners.
    fn static_strings(self) -> &'static [StaticStrings] {
        match self {
            Self::Sys => &[
                StaticStrings::Final,
                StaticStrings::Major,
                StaticStrings::Micro,
                StaticStrings::Minor,
                StaticStrings::Monty,
                StaticStrings::MontyVersionString,
                StaticStrings::Platform,
                StaticStrings::Releaselevel,
                StaticStrings::Serial,
                StaticStrings::Setrecursionlimit,
                StaticStrings::Stderr,
                StaticStrings::Stdout,
                StaticStrings::Sys,
                StaticStrings::SysVersionInfo,
                StaticStrings::Version,
                StaticStrings::VersionInfo,
            ],
            Self::Typing => &[
                StaticStrings::Annotated,
                StaticStrings::Any,
                StaticStrings::Callable,
                StaticStrings::ClassVar,
                StaticStrings::DictType,
                StaticStrings::FinalType,
                StaticStrings::FrozenSet,
                StaticStrings::Generator,
                StaticStrings::Generic,
                StaticStrings::Iterable,
                StaticStrings::IteratorType,
                StaticStrings::ListType,
                StaticStrings::Literal,
                StaticStrings::Mapping,
                StaticStrings::Never,
                StaticStrings::NoReturn,
                StaticStrings::Optional,
                StaticStrings::Protocol,
                StaticStrings::SelfType,
                StaticStrings::Sequence,
                StaticStrings::SetType,
                StaticStrings::TupleType,
                StaticStrings::Type,
                StaticStrings::TypeChecking,
                StaticStrings::TypeVar,
                StaticStrings::Typing,
                StaticStrings::UnionType,
            ],
            Self::Asyncio => &[StaticStrings::Asyncio, StaticStrings::Gather, StaticStrings::Run],
            Self::Pathlib => &[StaticStrings::PathClass, StaticStrings::Pathlib],
            Self::Os => &[
                StaticStrings::Altsep,
                StaticStrings::Curdir,
                StaticStrings::DevNullString,
                StaticStrings::Devnull,
                StaticStrings::Environ,
                StaticStrings::Extsep,
                StaticStrings::Getenv,
                StaticStrings::Linesep,
                StaticStrings::Listdir,
                StaticStrings::Makedirs,
                StaticStrings::Mkdir,
                StaticStrings::Name,
                StaticStrings::Os,
                StaticStrings::OsFspath,
                StaticStrings::Pardir,
                StaticStrings::ParentDirString,
                StaticStrings::Posix,
                StaticStrings::Remove,
                StaticStrings::Rename,
                StaticStrings::Replace,
                StaticStrings::Rmdir,
                StaticStrings::Sep,
                StaticStrings::StatMethod,
                StaticStrings::Unlink,
            ],
            Self::Math => &[
                StaticStrings::Acos,
                StaticStrings::Acosh,
                StaticStrings::Asin,
                StaticStrings::Asinh,
                StaticStrings::Atan,
                StaticStrings::Atan2,
                StaticStrings::Atanh,
                StaticStrings::Cbrt,
                StaticStrings::Ceil,
                StaticStrings::Comb,
                StaticStrings::Copysign,
                StaticStrings::Cos,
                StaticStrings::Cosh,
                StaticStrings::Degrees,
                StaticStrings::Erf,
                StaticStrings::Erfc,
                StaticStrings::Exp,
                StaticStrings::Exp2,
                StaticStrings::Expm1,
                StaticStrings::Fabs,
                StaticStrings::Factorial,
                StaticStrings::Floor,
                StaticStrings::Fmod,
                StaticStrings::Frexp,
                StaticStrings::Gamma,
                StaticStrings::Gcd,
                StaticStrings::Isclose,
                StaticStrings::Isfinite,
                StaticStrings::Isinf,
                StaticStrings::Isnan,
                StaticStrings::Isqrt,
                StaticStrings::Lcm,
                StaticStrings::Ldexp,
                StaticStrings::Lgamma,
                StaticStrings::Log,
                StaticStrings::Log10,
                StaticStrings::Log1p,
                StaticStrings::Log2,
                StaticStrings::Math,
                StaticStrings::MathE,
                StaticStrings::MathInf,
                StaticStrings::MathNan,
                StaticStrings::Modf,
                StaticStrings::Nextafter,
                StaticStrings::Perm,
                StaticStrings::Pi,
                StaticStrings::Pow,
                StaticStrings::Radians,
                StaticStrings::Remainder,
                StaticStrings::Sin,
                StaticStrings::Sinh,
                StaticStrings::Sqrt,
                StaticStrings::Tan,
                StaticStrings::Tanh,
                StaticStrings::Tau,
                StaticStrings::Trunc,
                StaticStrings::Ulp,
            ],
            Self::Json => &[
                StaticStrings::Dumps,
                StaticStrings::Json,
                StaticStrings::JsonDecodeError,
                StaticStrings::Loads,
            ],
            Self::Re => &[
                StaticStrings::A,
                StaticStrings::AsciiFlag,
                StaticStrings::Compile,
                StaticStrings::DotallFlag,
                StaticStrings::Error,
                StaticStrings::Escape,
                StaticStrings::Findall,
                StaticStrings::Finditer,
                StaticStrings::Fullmatch,
                StaticStrings::I,
                StaticStrings::Ignorecase,
                StaticStrings::M,
                StaticStrings::Match,
                StaticStrings::MatchClass,
                StaticStrings::MultilineFlag,
                StaticStrings::NoFlag,
                StaticStrings::PatternClass,
                StaticStrings::PatternError,
                StaticStrings::Re,
                StaticStrings::S,
                StaticStrings::Search,
                StaticStrings::Split,
                StaticStrings::Sub,
            ],
            Self::Datetime => &[
                StaticStrings::Date,
                StaticStrings::Datetime,
                StaticStrings::Time,
                StaticStrings::Timedelta,
                StaticStrings::Timezone,
            ],
            Self::Unicodedata => &[
                StaticStrings::Category,
                StaticStrings::Combining,
                StaticStrings::IsNormalized,
                StaticStrings::Lookup,
                StaticStrings::Name,
                StaticStrings::Normalize,
                StaticStrings::Unicodedata,
                StaticStrings::UnidataVersion,
            ],
            Self::Itertools => &[
                StaticStrings::Chain,
                StaticStrings::Compress,
                StaticStrings::Count,
                StaticStrings::Cycle,
                StaticStrings::Dropwhile,
                StaticStrings::Filterfalse,
                StaticStrings::Islice,
                StaticStrings::Itertools,
                StaticStrings::Pairwise,
                StaticStrings::Repeat,
                StaticStrings::Starmap,
                StaticStrings::Takewhile,
            ],
            Self::Dataclasses => &[
                StaticStrings::Dataclass,
                StaticStrings::DataclassFields,
                StaticStrings::DataclassParams,
                StaticStrings::Dataclasses,
                StaticStrings::FrozenInstanceError,
                StaticStrings::IsDataclass,
            ],
            Self::Collections => &[
                StaticStrings::Collections,
                StaticStrings::Counter,
                StaticStrings::Defaultdict,
                StaticStrings::Deque,
                StaticStrings::DunderMain,
                StaticStrings::Namedtuple,
            ],
            Self::Functools => &[StaticStrings::Functools, StaticStrings::Partial, StaticStrings::Reduce],
            Self::Base64 => &[
                StaticStrings::B16Decode,
                StaticStrings::B16Encode,
                StaticStrings::B32Decode,
                StaticStrings::B32Encode,
                StaticStrings::B32HexDecode,
                StaticStrings::B32HexEncode,
                StaticStrings::B64Decode,
                StaticStrings::B64Encode,
                StaticStrings::B85Decode,
                StaticStrings::B85Encode,
                StaticStrings::Base64,
                StaticStrings::Decodebytes,
                StaticStrings::Encodebytes,
                StaticStrings::MaxBinSize,
                StaticStrings::MaxLineSize,
                StaticStrings::StandardB64Decode,
                StaticStrings::StandardB64Encode,
                StaticStrings::UrlsafeB64Decode,
                StaticStrings::UrlsafeB64Encode,
                StaticStrings::Z85Decode,
                StaticStrings::Z85Encode,
            ],
            Self::Binascii => &[
                StaticStrings::A2bBase64,
                StaticStrings::A2bHex,
                StaticStrings::B2aBase64,
                StaticStrings::B2aHex,
                StaticStrings::Binascii,
                StaticStrings::Crc32,
                StaticStrings::ErrorClass,
                StaticStrings::Hexlify,
                StaticStrings::Unhexlify,
            ],
            #[cfg(feature = "test-hooks")]
            Self::Gc => &[
                StaticStrings::Collect,
                StaticStrings::Disable,
                StaticStrings::Enable,
                StaticStrings::Gc,
            ],
        }
    }

    /// Get the module from a string ID.
    pub fn from_string_id(string_id: StringId, interns: &InternerBuilder) -> Option<Self> {
        match interns.static_string(string_id)? {
            StaticStrings::Sys => Some(Self::Sys),
            StaticStrings::Typing => Some(Self::Typing),
            StaticStrings::Asyncio => Some(Self::Asyncio),
            StaticStrings::Pathlib => Some(Self::Pathlib),
            StaticStrings::Os => Some(Self::Os),
            StaticStrings::Math => Some(Self::Math),
            StaticStrings::Json => Some(Self::Json),
            StaticStrings::Re => Some(Self::Re),
            StaticStrings::Datetime => Some(Self::Datetime),
            StaticStrings::Unicodedata => Some(Self::Unicodedata),
            StaticStrings::Itertools => Some(Self::Itertools),
            StaticStrings::Dataclasses => Some(Self::Dataclasses),
            StaticStrings::Collections => Some(Self::Collections),
            StaticStrings::Functools => Some(Self::Functools),
            StaticStrings::Base64 => Some(Self::Base64),
            StaticStrings::Binascii => Some(Self::Binascii),
            #[cfg(feature = "test-hooks")]
            StaticStrings::Gc => Some(Self::Gc),
            _ => None,
        }
    }

    /// Creates a new instance of this module on the heap.
    ///
    pub fn create(self, vm: &mut VM<'_>) -> HeapId {
        match self {
            Self::Sys => sys::create_module(vm),
            Self::Typing => typing::create_module(vm),
            Self::Asyncio => asyncio::create_module(vm),
            Self::Pathlib => pathlib::create_module(vm),
            Self::Os => os::create_module(vm),
            Self::Math => math::create_module(vm),
            Self::Json => json::create_module(vm),
            Self::Re => re::create_module(vm),
            Self::Datetime => datetime::create_module(vm),
            Self::Unicodedata => unicodedata::create_module(vm),
            Self::Itertools => itertools::create_module(vm),
            Self::Dataclasses => dataclasses::create_module(vm),
            Self::Collections => collections::create_module(vm),
            Self::Functools => functools::create_module(vm),
            Self::Base64 => base64::create_module(vm),
            Self::Binascii => binascii::create_module(vm),
            #[cfg(feature = "test-hooks")]
            Self::Gc => gc::create_module(vm),
        }
    }
}

/// Registers current module attributes missing from a deserialized interner.
///
/// This lets a newer build add a static module attribute without invalidating
/// snapshots whose interner already contains that module's import name.
pub(crate) fn restore_interned_strings(interns: &mut Interns) {
    for &module in StandardLib::ALL {
        let name: &'static str = module.name().into();
        if interns.get_string_id_by_name(name).is_some() {
            for &value in module.static_strings() {
                interns.intern_static(value);
            }
        }
    }
}

/// All stdlib module function (but not builtins).
///
/// Serde encodes these by declaration index and every dump reaches them through
/// `Value::ModuleFunction`, so ALWAYS APPEND new variants, ahead of the gated
/// block — inserting one misdecodes old dumps into the wrong function instead
/// of failing. The leading alphabetical run is an accident, not a rule;
/// reordering needs a `DUMP_VERSION` bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum ModuleFunctions {
    Asyncio(asyncio::AsyncioFunctions),
    Collections(collections::CollectionsFunctions),
    Json(json::JsonFunctions),
    Math(math::MathFunctions),
    Os(os::OsFunctions),
    Re(re::ReFunctions),
    Unicodedata(unicodedata::UnicodedataFunctions),
    Itertools(itertools::ItertoolsFunctions),
    Dataclasses(dataclasses::DataclassesFunctions),
    Functools(functools::FunctoolsFunctions),
    Base64(base64::Base64Functions),
    Binascii(binascii::BinasciiFunctions),
    /// `gc` module functions — only present under the `test-hooks` feature.
    /// See [`gc`] for why it is gated; as in [`StandardLib`], the gated block
    /// goes last and new variants are appended ahead of it.
    #[cfg(feature = "test-hooks")]
    Gc(gc::GcFunctions),
    /// `sys` module functions — only present under the `test-hooks` feature.
    /// Production `sys` is attribute-only; the test feature adds callables
    /// like `setrecursionlimit` that fixtures use to align behavior with
    /// CPython. See [`sys`] for the rationale.
    #[cfg(feature = "test-hooks")]
    Sys(sys::SysFunctions),
}

impl fmt::Display for ModuleFunctions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Asyncio(func) => write!(f, "{func}"),
            Self::Collections(func) => write!(f, "{func}"),
            Self::Json(func) => write!(f, "{func}"),
            Self::Math(func) => write!(f, "{func}"),
            Self::Os(func) => write!(f, "{func}"),
            Self::Re(func) => write!(f, "{func}"),
            Self::Unicodedata(func) => write!(f, "{func}"),
            Self::Itertools(func) => write!(f, "{func}"),
            Self::Dataclasses(func) => write!(f, "{func}"),
            Self::Functools(func) => write!(f, "{func}"),
            Self::Base64(func) => write!(f, "{func}"),
            Self::Binascii(func) => write!(f, "{func}"),
            #[cfg(feature = "test-hooks")]
            Self::Gc(func) => write!(f, "{func}"),
            #[cfg(feature = "test-hooks")]
            Self::Sys(func) => write!(f, "{func}"),
        }
    }
}

impl ModuleFunctions {
    /// Calls the module function with the given arguments.
    ///
    /// Returns `CallResult` to support both immediate values and OS calls that
    /// require host involvement (e.g., `os.getenv()` needs the host to provide environment variables).
    pub fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        match self {
            Self::Asyncio(functions) => asyncio::call(vm, functions, args),
            Self::Collections(functions) => collections::call(vm, functions, args).map(CallResult::Value),
            Self::Json(functions) => json::call(vm, functions, args).map(CallResult::Value),
            Self::Math(functions) => math::call(vm, functions, args).map(CallResult::Value),
            Self::Os(functions) => os::call(vm, functions, args),
            Self::Re(functions) => re::call(vm, functions, args),
            Self::Unicodedata(functions) => unicodedata::call(vm, functions, args).map(CallResult::Value),
            Self::Itertools(functions) => itertools::call(vm, functions, args).map(CallResult::Value),
            Self::Dataclasses(functions) => dataclasses::call(vm, functions, args).map(CallResult::Value),
            Self::Functools(functions) => functools::call(vm, functions, args).map(CallResult::Value),
            Self::Base64(functions) => base64::call(vm, functions, args).map(CallResult::Value),
            Self::Binascii(functions) => binascii::call(vm, functions, args).map(CallResult::Value),
            #[cfg(feature = "test-hooks")]
            Self::Gc(functions) => gc::call(vm, functions, args).map(CallResult::Value),
            #[cfg(feature = "test-hooks")]
            Self::Sys(functions) => sys::call(vm, functions, args).map(CallResult::Value),
        }
    }

    /// Writes the Python repr() string for this function to a formatter.
    pub fn py_repr_fmt<W: Write>(self, f: &mut W, py_id: impl fmt::LowerHex) -> fmt::Result {
        write!(f, "<function {self} at 0x{py_id:x}>")
    }
}
