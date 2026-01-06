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

/// Pre-interned attribute names for container methods.
///
/// These StringIds are assigned at startup in `InternerBuilder::new()` and provide
/// O(1) comparison for common method names without heap allocation.
///
/// Usage: `use crate::intern::attr;` then `attr::APPEND`, `attr::GET`, etc.
pub mod attr {
    use super::StringId;

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
}

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
        // Order must match the attr::* constants defined above
        debug_assert_eq!(interner.intern("append"), attr::APPEND);
        debug_assert_eq!(interner.intern("insert"), attr::INSERT);
        debug_assert_eq!(interner.intern("get"), attr::GET);
        debug_assert_eq!(interner.intern("keys"), attr::KEYS);
        debug_assert_eq!(interner.intern("values"), attr::VALUES);
        debug_assert_eq!(interner.intern("items"), attr::ITEMS);
        debug_assert_eq!(interner.intern("pop"), attr::POP);
        debug_assert_eq!(interner.intern("clear"), attr::CLEAR);
        debug_assert_eq!(interner.intern("copy"), attr::COPY);
        debug_assert_eq!(interner.intern("add"), attr::ADD);
        debug_assert_eq!(interner.intern("remove"), attr::REMOVE);
        debug_assert_eq!(interner.intern("discard"), attr::DISCARD);
        debug_assert_eq!(interner.intern("update"), attr::UPDATE);
        debug_assert_eq!(interner.intern("union"), attr::UNION);
        debug_assert_eq!(interner.intern("intersection"), attr::INTERSECTION);
        debug_assert_eq!(interner.intern("difference"), attr::DIFFERENCE);
        debug_assert_eq!(interner.intern("symmetric_difference"), attr::SYMMETRIC_DIFFERENCE);
        debug_assert_eq!(interner.intern("issubset"), attr::ISSUBSET);
        debug_assert_eq!(interner.intern("issuperset"), attr::ISSUPERSET);
        debug_assert_eq!(interner.intern("isdisjoint"), attr::ISDISJOINT);

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
