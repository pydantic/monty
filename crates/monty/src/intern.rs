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

use std::{str::FromStr, sync::LazyLock};

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
    std::array::from_fn(|i| {
        // Safe: i is always 0-127 for a 128-element array
        let s = char::from(u8::try_from(i).expect("index out of u8 range")).to_string();
        // Leak to get 'static lifetime - this is intentional and bounded (128 bytes total)
        // Reborrow as immutable since we won't mutate
        &*Box::leak(s.into_boxed_str())
    })
});

/// Static string values which are known at compile time and don't need to be interned.
#[repr(u8)]
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
    #[strum(serialize = "symmetric_difference")]
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
    #[strum(serialize = "sys")]
    Sys,
    #[strum(serialize = "sys.version_info")]
    SysVersionInfo,
    #[strum(serialize = "version")]
    Version,
    #[strum(serialize = "version_info")]
    VersionInfo,
    #[strum(serialize = "platform")]
    Platform,
    #[strum(serialize = "stdout")]
    Stdout,
    #[strum(serialize = "stderr")]
    Stderr,
    #[strum(serialize = "major")]
    Major,
    #[strum(serialize = "minor")]
    Minor,
    #[strum(serialize = "micro")]
    Micro,
    #[strum(serialize = "releaselevel")]
    Releaselevel,
    #[strum(serialize = "serial")]
    Serial,
    #[strum(serialize = "final")]
    Final,
    #[strum(serialize = "3.14.0 (Monty)")]
    MontyVersionString,
    #[strum(serialize = "monty")]
    Monty,

    // ==========================
    // os.stat_result fields
    #[strum(serialize = "StatResult")]
    OsStatResult,
    #[strum(serialize = "st_mode")]
    StMode,
    #[strum(serialize = "st_ino")]
    StIno,
    #[strum(serialize = "st_dev")]
    StDev,
    #[strum(serialize = "st_nlink")]
    StNlink,
    #[strum(serialize = "st_uid")]
    StUid,
    #[strum(serialize = "st_gid")]
    StGid,
    #[strum(serialize = "st_size")]
    StSize,
    #[strum(serialize = "st_atime")]
    StAtime,
    #[strum(serialize = "st_mtime")]
    StMtime,
    #[strum(serialize = "st_ctime")]
    StCtime,

    // ==========================
    // typing module strings
    #[strum(serialize = "typing")]
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
    #[strum(serialize = "asyncio")]
    Asyncio,
    #[strum(serialize = "gather")]
    Gather,

    // ==========================
    // os module strings
    #[strum(serialize = "os")]
    Os,
    #[strum(serialize = "getenv")]
    Getenv,
    #[strum(serialize = "environ")]
    Environ,
    #[strum(serialize = "default")]
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
    #[strum(serialize = "pathlib")]
    Pathlib,
    #[strum(serialize = "Path")]
    PathClass,

    // Path properties (pure - no I/O)
    #[strum(serialize = "name")]
    Name,
    #[strum(serialize = "parent")]
    Parent,
    #[strum(serialize = "stem")]
    Stem,
    #[strum(serialize = "suffix")]
    Suffix,
    #[strum(serialize = "suffixes")]
    Suffixes,
    #[strum(serialize = "parts")]
    Parts,

    // Path pure methods (no I/O)
    #[strum(serialize = "is_absolute")]
    IsAbsolute,
    #[strum(serialize = "joinpath")]
    Joinpath,
    #[strum(serialize = "with_name")]
    WithName,
    #[strum(serialize = "with_stem")]
    WithStem,
    #[strum(serialize = "with_suffix")]
    WithSuffix,
    #[strum(serialize = "as_posix")]
    AsPosix,
    #[strum(serialize = "__fspath__")]
    Fspath,

    // Path filesystem methods (require OsAccess - yield external calls)
    #[strum(serialize = "exists")]
    Exists,
    #[strum(serialize = "is_file")]
    IsFile,
    #[strum(serialize = "is_dir")]
    IsDir,
    #[strum(serialize = "is_symlink")]
    IsSymlink,
    #[strum(serialize = "stat")]
    StatMethod,
    #[strum(serialize = "read_bytes")]
    ReadBytes,
    #[strum(serialize = "read_text")]
    ReadText,
    #[strum(serialize = "iterdir")]
    Iterdir,
    #[strum(serialize = "resolve")]
    Resolve,
    #[strum(serialize = "absolute")]
    Absolute,

    // Path write methods (require OsAccess - yield external calls)
    #[strum(serialize = "write_text")]
    WriteText,
    #[strum(serialize = "write_bytes")]
    WriteBytes,
    #[strum(serialize = "mkdir")]
    Mkdir,
    #[strum(serialize = "unlink")]
    Unlink,
    #[strum(serialize = "rmdir")]
    Rmdir,
    #[strum(serialize = "rename")]
    Rename,

    // Slice attributes
    Start,
    Stop,
    Step,
}

impl StaticStrings {
    /// Attempts to convert a `StringId` back to a `StaticStrings` variant.
    ///
    /// Returns `None` if the `StringId` doesn't correspond to a static string
    /// (e.g., it's an ASCII char or a dynamically interned string).
    pub fn from_string_id(id: StringId) -> Option<Self> {
        let enum_id = id.0.checked_sub(STATIC_STRING_ID_OFFSET)?;
        u8::try_from(enum_id).ok().and_then(Self::from_repr)
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

/// Unique identifier for external functions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ExtFunctionId(u32);

impl ExtFunctionId {
    pub fn new(index: usize) -> Self {
        Self(index.try_into().expect("Invalid external function id"))
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
    external_functions: Vec<String>,
}

impl Interns {
    pub fn new(interner: InternerBuilder, functions: Vec<Function>, external_functions: Vec<String>) -> Self {
        Self {
            strings: interner.strings,
            bytes: interner.bytes,
            long_ints: interner.long_ints,
            functions,
            external_functions,
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

    /// Lookup an external function name by its `ExtFunctionId`
    ///
    /// # Panics
    ///
    /// Panics if the `ExtFunctionId` is invalid.
    #[inline]
    pub fn get_external_function_name(&self, id: ExtFunctionId) -> String {
        self.external_functions
            .get(id.index())
            .expect("External function not found")
            .clone()
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
