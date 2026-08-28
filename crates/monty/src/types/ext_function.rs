use std::sync::Arc;

use crate::value::EitherStr;

/// A host-provided callable identified by its external lookup name.
///
/// The name allocation is shared with the heap's weak external-function cache,
/// which preserves identity while any function with this name remains live.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExtFunction(Arc<str>);

impl ExtFunction {
    /// Creates an external function with an owned, shareable lookup name.
    pub(crate) fn new(name: &str) -> Self {
        Self(Arc::from(name))
    }

    /// Returns the function name as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the shared name used as the weak-cache key.
    pub(crate) fn cache_key(&self) -> Arc<str> {
        Arc::clone(&self.0)
    }

    /// Clones the function name for an external call suspension.
    pub(crate) fn clone_name(&self) -> EitherStr {
        self.0.to_string().into()
    }
}
