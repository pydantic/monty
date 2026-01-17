//! String and bytes interning for efficient storage of literals and identifiers.
//!
//! This module provides interners that store unique strings and bytes in vectors
//! and return indices (`StringId`, `BytesId`) for efficient storage and comparison.
//! This avoids the overhead of cloning strings or using atomic reference counting.
//!
//! The interners are populated during parsing and preparation, then owned by the `Executor`.
//! During execution, lookups are needed only for error messages and repr output.
//!
//! The first string entry (index 0) is always `"<module>"` for module-level code.

use std::{borrow::Cow, sync::LazyLock};

use ahash::AHashMap;

use crate::function::Function;

/// Index into the string interner's storage.
///
/// Uses `u32` to save space (4 bytes vs 8 bytes for `usize`). This limits us to
/// ~4 billion unique interns, which is more than sufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct StringId(u32);

/// The StringId for `"<module>"` - always index 0 in the interner.
pub const MODULE_STRING_ID: StringId = StringId(0);

/// update MAX_ATTR_ID when adding new attrs
const MAX_ATTR_ID: u32 = 61;

/// The StringId for the empty string `""` - interned for allocation-free empty string returns.
pub const EMPTY_STRING: StringId = StringId(MAX_ATTR_ID + 1);

/// Number of ASCII single-character strings pre-interned at startup.
const ASCII_STRING_COUNT: u32 = 128;

/// First StringId reserved for ASCII single-character interns.
/// Starts after MAX_ATTR_ID and EMPTY_STRING.
const ASCII_STRING_START_ID: u32 = MAX_ATTR_ID + 2;

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

/// Base interner with all pre-interned strings, built once on first access.
///
/// Contains `<module>`, all attribute names, and ASCII single-character strings.
/// `InternerBuilder::new()` clones this to avoid rebuilding the base set each time.
static BASE_INTERNER: LazyLock<InternerBuilder> = LazyLock::new(InternerBuilder::build_base);

/// Returns the interned StringId for an ASCII byte.
///
/// These interns are created during `InternerBuilder::new()` and allow
/// allocation-free iteration over ASCII strings.
#[must_use]
pub(crate) fn ascii_string_id(byte: u8) -> StringId {
    StringId(ASCII_STRING_START_ID + u32::from(byte))
}

/// Pre-interned attribute names for container methods.
///
/// These StringIds are assigned at startup in `InternerBuilder::new()` and provide
/// O(1) comparison for common method names without heap allocation.
///
/// Usage: `use crate::intern::attr;` then `attr::APPEND`, `attr::GET`, etc.
///
/// IMPORTANT NOTE: the last (max) attribute ID must be kept as `MAX_ATTR_ID` by updating
/// `MAX_ATTR_ID` when new attrs are added.
///
/// ALSO update `InternerBuilder::new` debug_assertions when adding new attrs!
pub mod attr {
    use super::{MAX_ATTR_ID, StringId};

    // List methods
    pub const APPEND: StringId = StringId(1);
    pub const INSERT: StringId = StringId(2);

    // Dict methods
    pub const GET: StringId = StringId(3);
    pub const KEYS: StringId = StringId(4);
    pub const VALUES: StringId = StringId(5);
    pub const ITEMS: StringId = StringId(6);

    // Shared methods (list, dict, set)
    pub const POP: StringId = StringId(7);
    pub const CLEAR: StringId = StringId(8);
    pub const COPY: StringId = StringId(9);

    // Set methods
    pub const ADD: StringId = StringId(10);
    pub const REMOVE: StringId = StringId(11);
    pub const DISCARD: StringId = StringId(12);
    pub const UPDATE: StringId = StringId(13);
    pub const UNION: StringId = StringId(14);
    pub const INTERSECTION: StringId = StringId(15);
    pub const DIFFERENCE: StringId = StringId(16);
    pub const SYMMETRIC_DIFFERENCE: StringId = StringId(17);
    pub const ISSUBSET: StringId = StringId(18);
    pub const ISSUPERSET: StringId = StringId(19);
    pub const ISDISJOINT: StringId = StringId(20);

    // String methods
    pub const JOIN: StringId = StringId(21);
    // Phase 1: Simple transformations
    pub const LOWER: StringId = StringId(22);
    pub const UPPER: StringId = StringId(23);
    pub const CAPITALIZE: StringId = StringId(24);
    pub const TITLE: StringId = StringId(25);
    pub const SWAPCASE: StringId = StringId(26);
    pub const CASEFOLD: StringId = StringId(27);
    // Phase 2: Predicate methods
    pub const ISALPHA: StringId = StringId(28);
    pub const ISDIGIT: StringId = StringId(29);
    pub const ISALNUM: StringId = StringId(30);
    pub const ISNUMERIC: StringId = StringId(31);
    pub const ISSPACE: StringId = StringId(32);
    pub const ISLOWER: StringId = StringId(33);
    pub const ISUPPER: StringId = StringId(34);
    pub const ISASCII: StringId = StringId(35);
    pub const ISDECIMAL: StringId = StringId(36);
    // Phase 3: Search methods
    pub const FIND: StringId = StringId(37);
    pub const RFIND: StringId = StringId(38);
    pub const INDEX: StringId = StringId(39);
    pub const RINDEX: StringId = StringId(40);
    pub const COUNT: StringId = StringId(41);
    pub const STARTSWITH: StringId = StringId(42);
    pub const ENDSWITH: StringId = StringId(43);
    // Phase 4: Strip/trim methods
    pub const STRIP: StringId = StringId(44);
    pub const LSTRIP: StringId = StringId(45);
    pub const RSTRIP: StringId = StringId(46);
    pub const REMOVEPREFIX: StringId = StringId(47);
    pub const REMOVESUFFIX: StringId = StringId(48);
    // Phase 5: Split methods
    pub const SPLIT: StringId = StringId(49);
    pub const RSPLIT: StringId = StringId(50);
    pub const SPLITLINES: StringId = StringId(51);
    pub const PARTITION: StringId = StringId(52);
    pub const RPARTITION: StringId = StringId(53);
    // Phase 6: Replace/modify methods
    pub const REPLACE: StringId = StringId(54);
    pub const CENTER: StringId = StringId(55);
    pub const LJUST: StringId = StringId(56);
    pub const RJUST: StringId = StringId(57);
    pub const ZFILL: StringId = StringId(58);
    // Additional methods
    pub const ENCODE: StringId = StringId(59);
    pub const ISIDENTIFIER: StringId = StringId(60);
    pub const ISTITLE: StringId = StringId(MAX_ATTR_ID);
}

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
    string_map: AHashMap<Cow<'static, str>, StringId>,
    /// Storage for interned interns, indexed by `StringId`.
    strings: Vec<Cow<'static, str>>,
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
        // Clone the base interner with all pre-interned strings
        let mut interner = BASE_INTERNER.clone();

        // Reserve additional capacity for code-specific strings
        // Rough guess: count quotes and divide by 2 (open+close per string)
        let additional_strings = code.bytes().filter(|&b| b == b'"' || b == b'\'').count() >> 1;
        if additional_strings > 0 {
            interner.string_map.reserve(additional_strings);
            interner.strings.reserve(additional_strings);
        }

        interner
    }

    /// Builds the base interner with all pre-interned strings.
    ///
    /// Called once by `BASE_INTERNER` lazy initialization. Contains `<module>`,
    /// all attribute names, and ASCII single-character strings.
    fn build_base() -> Self {
        // +1 for <module>, +1 for empty string
        let base_count = (MAX_ATTR_ID + ASCII_STRING_COUNT + 2) as usize;
        let mut interner = Self {
            string_map: AHashMap::with_capacity(base_count),
            strings: Vec::with_capacity(base_count),
            bytes: Vec::new(),
        };

        // Index 0: "<module>" for module-level code
        let id = interner.intern_static("<module>");
        debug_assert_eq!(id, MODULE_STRING_ID);

        // Pre-intern known attribute names.
        // Order must match the attr::* constants defined above.
        // Note: We separate the intern() call from debug_assert_eq! because
        // debug_assert_eq! is completely removed in release builds.
        let id = interner.intern_static("append");
        debug_assert_eq!(id, attr::APPEND);
        let id = interner.intern_static("insert");
        debug_assert_eq!(id, attr::INSERT);
        let id = interner.intern_static("get");
        debug_assert_eq!(id, attr::GET);
        let id = interner.intern_static("keys");
        debug_assert_eq!(id, attr::KEYS);
        let id = interner.intern_static("values");
        debug_assert_eq!(id, attr::VALUES);
        let id = interner.intern_static("items");
        debug_assert_eq!(id, attr::ITEMS);
        let id = interner.intern_static("pop");
        debug_assert_eq!(id, attr::POP);
        let id = interner.intern_static("clear");
        debug_assert_eq!(id, attr::CLEAR);
        let id = interner.intern_static("copy");
        debug_assert_eq!(id, attr::COPY);
        let id = interner.intern_static("add");
        debug_assert_eq!(id, attr::ADD);
        let id = interner.intern_static("remove");
        debug_assert_eq!(id, attr::REMOVE);
        let id = interner.intern_static("discard");
        debug_assert_eq!(id, attr::DISCARD);
        let id = interner.intern_static("update");
        debug_assert_eq!(id, attr::UPDATE);
        let id = interner.intern_static("union");
        debug_assert_eq!(id, attr::UNION);
        let id = interner.intern_static("intersection");
        debug_assert_eq!(id, attr::INTERSECTION);
        let id = interner.intern_static("difference");
        debug_assert_eq!(id, attr::DIFFERENCE);
        let id = interner.intern_static("symmetric_difference");
        debug_assert_eq!(id, attr::SYMMETRIC_DIFFERENCE);
        let id = interner.intern_static("issubset");
        debug_assert_eq!(id, attr::ISSUBSET);
        let id = interner.intern_static("issuperset");
        debug_assert_eq!(id, attr::ISSUPERSET);
        let id = interner.intern_static("isdisjoint");
        debug_assert_eq!(id, attr::ISDISJOINT);
        let id = interner.intern_static("join");
        debug_assert_eq!(id, attr::JOIN);
        // Phase 1: Simple transformations
        let id = interner.intern_static("lower");
        debug_assert_eq!(id, attr::LOWER);
        let id = interner.intern_static("upper");
        debug_assert_eq!(id, attr::UPPER);
        let id = interner.intern_static("capitalize");
        debug_assert_eq!(id, attr::CAPITALIZE);
        let id = interner.intern_static("title");
        debug_assert_eq!(id, attr::TITLE);
        let id = interner.intern_static("swapcase");
        debug_assert_eq!(id, attr::SWAPCASE);
        let id = interner.intern_static("casefold");
        debug_assert_eq!(id, attr::CASEFOLD);
        // Phase 2: Predicate methods
        let id = interner.intern_static("isalpha");
        debug_assert_eq!(id, attr::ISALPHA);
        let id = interner.intern_static("isdigit");
        debug_assert_eq!(id, attr::ISDIGIT);
        let id = interner.intern_static("isalnum");
        debug_assert_eq!(id, attr::ISALNUM);
        let id = interner.intern_static("isnumeric");
        debug_assert_eq!(id, attr::ISNUMERIC);
        let id = interner.intern_static("isspace");
        debug_assert_eq!(id, attr::ISSPACE);
        let id = interner.intern_static("islower");
        debug_assert_eq!(id, attr::ISLOWER);
        let id = interner.intern_static("isupper");
        debug_assert_eq!(id, attr::ISUPPER);
        let id = interner.intern_static("isascii");
        debug_assert_eq!(id, attr::ISASCII);
        let id = interner.intern_static("isdecimal");
        debug_assert_eq!(id, attr::ISDECIMAL);
        // Phase 3: Search methods
        let id = interner.intern_static("find");
        debug_assert_eq!(id, attr::FIND);
        let id = interner.intern_static("rfind");
        debug_assert_eq!(id, attr::RFIND);
        let id = interner.intern_static("index");
        debug_assert_eq!(id, attr::INDEX);
        let id = interner.intern_static("rindex");
        debug_assert_eq!(id, attr::RINDEX);
        let id = interner.intern_static("count");
        debug_assert_eq!(id, attr::COUNT);
        let id = interner.intern_static("startswith");
        debug_assert_eq!(id, attr::STARTSWITH);
        let id = interner.intern_static("endswith");
        debug_assert_eq!(id, attr::ENDSWITH);
        // Phase 4: Strip/trim methods
        let id = interner.intern_static("strip");
        debug_assert_eq!(id, attr::STRIP);
        let id = interner.intern_static("lstrip");
        debug_assert_eq!(id, attr::LSTRIP);
        let id = interner.intern_static("rstrip");
        debug_assert_eq!(id, attr::RSTRIP);
        let id = interner.intern_static("removeprefix");
        debug_assert_eq!(id, attr::REMOVEPREFIX);
        let id = interner.intern_static("removesuffix");
        debug_assert_eq!(id, attr::REMOVESUFFIX);
        // Phase 5: Split methods
        let id = interner.intern_static("split");
        debug_assert_eq!(id, attr::SPLIT);
        let id = interner.intern_static("rsplit");
        debug_assert_eq!(id, attr::RSPLIT);
        let id = interner.intern_static("splitlines");
        debug_assert_eq!(id, attr::SPLITLINES);
        let id = interner.intern_static("partition");
        debug_assert_eq!(id, attr::PARTITION);
        let id = interner.intern_static("rpartition");
        debug_assert_eq!(id, attr::RPARTITION);
        // Phase 6: Replace/modify methods
        let id = interner.intern_static("replace");
        debug_assert_eq!(id, attr::REPLACE);
        let id = interner.intern_static("center");
        debug_assert_eq!(id, attr::CENTER);
        let id = interner.intern_static("ljust");
        debug_assert_eq!(id, attr::LJUST);
        let id = interner.intern_static("rjust");
        debug_assert_eq!(id, attr::RJUST);
        let id = interner.intern_static("zfill");
        debug_assert_eq!(id, attr::ZFILL);
        // Additional methods
        let id = interner.intern_static("encode");
        debug_assert_eq!(id, attr::ENCODE);
        let id = interner.intern_static("isidentifier");
        debug_assert_eq!(id, attr::ISIDENTIFIER);
        let id = interner.intern_static("istitle");
        debug_assert_eq!(id, attr::ISTITLE);

        // Pre-intern the empty string for allocation-free empty string returns
        let id = interner.intern_static("");
        debug_assert_eq!(id, EMPTY_STRING);

        // Pre-intern ASCII single-character strings so string iteration can reuse interns.
        for byte in 0u8..=127 {
            let id = interner.intern_static(ASCII_STRS[byte as usize]);
            debug_assert_eq!(id, ascii_string_id(byte));
        }

        interner
    }

    /// Interns a string, returning its `StringId`.
    ///
    /// If the string was already interned, returns the existing `StringId`.
    /// Otherwise, stores the string and returns a new `StringId`.
    pub fn intern(&mut self, s: &str) -> StringId {
        *self.string_map.entry(s.to_owned().into()).or_insert_with(|| {
            let id = StringId(self.strings.len().try_into().expect("StringId overflow"));
            self.strings.push(s.to_owned().into());
            id
        })
    }

    fn intern_static(&mut self, s: &'static str) -> StringId {
        *self.string_map.entry(s.into()).or_insert_with(|| {
            let id = StringId(self.strings.len().try_into().expect("StringId overflow"));
            self.strings.push(s.into());
            id
        })
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
    ///
    /// # Panics
    ///
    /// Panics if the `StringId` is invalid (not from this interner).
    #[inline]
    pub fn get_str(&self, id: StringId) -> &str {
        &self.strings[id.index()]
    }
}

/// Read-only storage for interned string and bytes.
///
/// This provides lookup by `StringId`, `BytesId` and `FunctionId` for interned literals and functions
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Interns {
    strings: Vec<Cow<'static, str>>,
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
        &self.strings[id.index()]
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
