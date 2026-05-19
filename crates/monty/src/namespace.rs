/// Unique identifier for variable slots in namespaces (globals and function locals).
///
/// Used by the bytecode compiler to emit slot indices for variable access.
/// The VM uses these indices to read/write values in the globals vector
/// or the stack-inlined locals region.
///
/// Storage is `u16` because every bytecode opcode that takes a namespace
/// slot (`LoadLocal`, `LoadGlobal`, `StoreLocal`, …) encodes the slot in
/// 16 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamespaceId(u16);

impl NamespaceId {
    /// Creates a `NamespaceId` from a `usize` slot index, returning `None` if
    /// the index doesn't fit in `u16`. Callers in the prepare phase wrap the
    /// `None` case in a `ParseError::Syntax` so user-input-driven overflows
    /// surface as a clean `SyntaxError` rather than a panic at emission time.
    pub fn new(index: usize) -> Option<Self> {
        u16::try_from(index).ok().map(Self)
    }

    /// Returns the slot index as the `u16` operand consumed by the bytecode.
    #[inline]
    pub fn as_u16(self) -> u16 {
        self.0
    }

    /// Returns the slot as `usize` for `Vec`/array indexing in the VM.
    #[inline]
    pub fn index(self) -> usize {
        self.0.into()
    }
}
