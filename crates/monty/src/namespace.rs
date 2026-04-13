/// Unique identifier for variable slots in namespaces (globals and function locals).
///
/// Used by the bytecode compiler to emit slot indices for variable access.
/// The VM uses these indices to read/write values in the globals vector
/// or the stack-inlined locals region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamespaceId(u32);

impl NamespaceId {
    pub fn new(index: usize) -> Self {
        Self(index.try_into().expect("Invalid namespace id"))
    }

    /// Returns the raw index value.
    ///
    /// Used by the bytecode compiler to emit slot indices for variable access.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
