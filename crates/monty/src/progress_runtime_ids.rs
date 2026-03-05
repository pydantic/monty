//! Shared type aliases for exposing runtime-ID slices from progress enums.

use crate::runtime_id::RuntimeValueId;

/// Borrowed runtime-ID slices for positional args and keyword `(key, value)` pairs.
pub(crate) type RuntimeIdSlices<'a> = (&'a [RuntimeValueId], &'a [(RuntimeValueId, RuntimeValueId)]);
