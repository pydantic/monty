//! String and bytes interning for efficient storage of literals and identifiers.
//!
//! This module provides interners that store unique strings and bytes in vectors
//! and return indices (`StringId`, `BytesId`) for efficient storage and comparison.
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
use strum::{EnumString, FromRepr, IntoStaticStr};

use crate::function::Function;

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
///
/// StringIds are derived from `StaticStrings::Module as u8`.
#[repr(u8)]
#[derive(Clone, Copy, FromRepr, EnumString, IntoStaticStr)]
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
}

impl StaticStrings {
    /// Converts this static string variant to its corresponding `StringId`.
    pub fn as_string_id(self) -> StringId {
        let string_id = self as u32;
        StringId(string_id + STATIC_STRING_ID_OFFSET)
    }

    /// Attempts to convert a `StringId` back to a `StaticStrings` variant.
    ///
    /// Returns `None` if the `StringId` doesn't correspond to a static string
    /// (e.g., it's an ASCII char or a dynamically interned string).
    pub fn from_string_id(id: StringId) -> Option<Self> {
        let enum_id = id.0.checked_sub(STATIC_STRING_ID_OFFSET)?;
        u8::try_from(enum_id).ok().and_then(Self::from_repr)
    }
}

impl PartialEq<StaticStrings> for StringId {
    fn eq(&self, other: &StaticStrings) -> bool {
        *self == other.as_string_id()
    }
}

impl PartialEq<StringId> for StaticStrings {
    fn eq(&self, other: &StringId) -> bool {
        self.as_string_id() == *other
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

/// A string and bytes interner that stores unique values and returns indices for lookup.
///
/// Interns are deduplicated on insertion - interning the same string twice returns
/// the same `StringId`. Bytes are NOT deduplicated (rare enough that it's not worth it).
/// The interner owns all strings/bytes and provides lookup by index.
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
        }
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
            ss.as_string_id()
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

/// Read-only storage for interned string and bytes.
///
/// This provides lookup by `StringId`, `BytesId` and `FunctionId` for interned literals and functions
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Interns {
    strings: Vec<String>,
    bytes: Vec<Vec<u8>>,
    functions: Vec<Function>,
    external_functions: Vec<String>,
}

impl Interns {
    pub fn new(interner: InternerBuilder, functions: Vec<Function>, external_functions: Vec<String>) -> Self {
        Self {
            strings: interner.strings,
            bytes: interner.bytes,
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
}
