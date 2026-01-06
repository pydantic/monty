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

// Pre-interned attribute names for container methods.
// These must match the order in `InternerBuilder::new()`.

/// StringId for "append" - list method
pub const ATTR_APPEND: StringId = StringId(1);
/// StringId for "insert" - list method
pub const ATTR_INSERT: StringId = StringId(2);
/// StringId for "get" - dict method
pub const ATTR_GET: StringId = StringId(3);
/// StringId for "keys" - dict method
pub const ATTR_KEYS: StringId = StringId(4);
/// StringId for "values" - dict method
pub const ATTR_VALUES: StringId = StringId(5);
/// StringId for "items" - dict method
pub const ATTR_ITEMS: StringId = StringId(6);
/// StringId for "pop" - shared method (list, dict, set)
pub const ATTR_POP: StringId = StringId(7);
/// StringId for "clear" - shared method
pub const ATTR_CLEAR: StringId = StringId(8);
/// StringId for "copy" - shared method
pub const ATTR_COPY: StringId = StringId(9);
/// StringId for "add" - set method
pub const ATTR_ADD: StringId = StringId(10);
/// StringId for "remove" - set method
pub const ATTR_REMOVE: StringId = StringId(11);
/// StringId for "discard" - set method
pub const ATTR_DISCARD: StringId = StringId(12);
/// StringId for "update" - set/dict method
pub const ATTR_UPDATE: StringId = StringId(13);
/// StringId for "union" - set method
pub const ATTR_UNION: StringId = StringId(14);
/// StringId for "intersection" - set method
pub const ATTR_INTERSECTION: StringId = StringId(15);
/// StringId for "difference" - set method
pub const ATTR_DIFFERENCE: StringId = StringId(16);
/// StringId for "symmetric_difference" - set method
pub const ATTR_SYMMETRIC_DIFFERENCE: StringId = StringId(17);
/// StringId for "issubset" - set method
pub const ATTR_ISSUBSET: StringId = StringId(18);
/// StringId for "issuperset" - set method
pub const ATTR_ISSUPERSET: StringId = StringId(19);
/// StringId for "isdisjoint" - set method
pub const ATTR_ISDISJOINT: StringId = StringId(20);

impl StringId {
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
    pub fn new(index: usize) -> Self {
        Self(index.try_into().expect("Invalid function id"))
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
#[derive(Debug, Default)]
pub struct InternerBuilder {
    /// Maps strings to their indices for deduplication during interning.
    map: AHashMap<String, StringId>,
    /// Storage for interned interns, indexed by `StringId`.
    strings: Vec<String>,
    /// Storage for interned bytes literals, indexed by `BytesId`.
    /// Not deduplicated since bytes literals are rare.
    bytes: Vec<Vec<u8>>,
}

impl InternerBuilder {
    /// Creates a new string interner with pre-interned strings.
    ///
    /// Pre-interns:
    /// - Index 0: `"<module>"` for module-level code
    /// - Indices 1-20: Known attribute names (append, insert, get, etc.)
    pub fn new() -> Self {
        let mut interner = Self::default();

        // Index 0: "<module>" for module-level code
        debug_assert_eq!(interner.intern("<module>"), MODULE_STRING_ID);

        // Pre-intern known attribute names (indices 1-20)
        // Order must match the ATTR_* constants defined above
        debug_assert_eq!(interner.intern("append"), ATTR_APPEND);
        debug_assert_eq!(interner.intern("insert"), ATTR_INSERT);
        debug_assert_eq!(interner.intern("get"), ATTR_GET);
        debug_assert_eq!(interner.intern("keys"), ATTR_KEYS);
        debug_assert_eq!(interner.intern("values"), ATTR_VALUES);
        debug_assert_eq!(interner.intern("items"), ATTR_ITEMS);
        debug_assert_eq!(interner.intern("pop"), ATTR_POP);
        debug_assert_eq!(interner.intern("clear"), ATTR_CLEAR);
        debug_assert_eq!(interner.intern("copy"), ATTR_COPY);
        debug_assert_eq!(interner.intern("add"), ATTR_ADD);
        debug_assert_eq!(interner.intern("remove"), ATTR_REMOVE);
        debug_assert_eq!(interner.intern("discard"), ATTR_DISCARD);
        debug_assert_eq!(interner.intern("update"), ATTR_UPDATE);
        debug_assert_eq!(interner.intern("union"), ATTR_UNION);
        debug_assert_eq!(interner.intern("intersection"), ATTR_INTERSECTION);
        debug_assert_eq!(interner.intern("difference"), ATTR_DIFFERENCE);
        debug_assert_eq!(interner.intern("symmetric_difference"), ATTR_SYMMETRIC_DIFFERENCE);
        debug_assert_eq!(interner.intern("issubset"), ATTR_ISSUBSET);
        debug_assert_eq!(interner.intern("issuperset"), ATTR_ISSUPERSET);
        debug_assert_eq!(interner.intern("isdisjoint"), ATTR_ISDISJOINT);

        interner
    }

    /// Interns a string, returning its `StringId`.
    ///
    /// If the string was already interned, returns the existing `StringId`.
    /// Otherwise, stores the string and returns a new `StringId`.
    pub fn intern(&mut self, s: &str) -> StringId {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = StringId(self.strings.len().try_into().expect("StringId overflow"));
        self.strings.push(s.to_owned());
        self.map.insert(s.to_owned(), id);
        id
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

    /// Looks up bytes by their `BytesId`.
    ///
    /// # Panics
    ///
    /// Panics if the `BytesId` is invalid (not from this interner).
    #[inline]
    pub fn get_bytes(&self, id: BytesId) -> &[u8] {
        &self.bytes[id.index()]
    }

    /// Consumes the interner and returns the strings and bytes storage.
    ///
    /// This is used when transferring ownership to the `Executor`.
    pub fn into_storage(self) -> (Vec<String>, Vec<Vec<u8>>) {
        (self.strings, self.bytes)
    }
}

/// Read-only storage for interned string and bytes.
///
/// This provides lookup by `StringId`, `BytesId` and `FunctionId` for interned literals and functions
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Interns {
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
}
