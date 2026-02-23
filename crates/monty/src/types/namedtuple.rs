/// Python named tuple type, combining tuple-like indexing with named attribute access.
///
/// Named tuples are like regular tuples but with field names, providing two ways
/// to access elements:
/// - By index: `version_info[0]` returns the major version
/// - By name: `version_info.major` returns the same value
///
/// Named tuples are:
/// - Immutable (all tuple semantics apply)
/// - Hashable (if all elements are hashable)
/// - Have a descriptive repr: `sys.version_info(major=3, minor=14, ...)`
/// - Support `len()` and iteration
///
/// # Use Case
///
/// This type is used for `sys.version_info` and similar structured tuples where
/// named access improves usability and readability.
use std::fmt::Write;

use ahash::AHashSet;

use super::PyTrait;
use crate::{
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapId},
    intern::{Interns, StringId},
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, Type},
    value::{EitherStr, Value},
};

/// Python named tuple value stored on the heap.
///
/// Wraps a `Vec<Value>` with associated field names and provides both index-based
/// and name-based access. Named tuples are conceptually immutable, though this is
/// not enforced at the type level for internal operations.
///
/// # Reference Counting
///
/// When a named tuple is freed, all contained heap references have their refcounts
/// decremented via `py_dec_ref_ids`.
///
/// # GC Optimization
///
/// The `contains_refs` flag tracks whether the tuple contains any `Value::Ref` items.
/// This allows `py_dec_ref_ids` to skip iteration when the tuple contains only
/// primitive values (ints, bools, None, etc.).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamedTuple {
    /// Type name for repr (e.g., "sys.version_info").
    name: EitherStr,
    /// Field names in order, e.g., `major`, `minor`, `micro`, `releaselevel`, `serial`.
    field_names: Vec<EitherStr>,
    /// Values in order (same length as field_names).
    items: Vec<Value>,
    /// True if any item is a `Value::Ref`. Set at creation time since named tuples are immutable.
    contains_refs: bool,
}

impl NamedTuple {
    /// Creates a new named tuple.
    ///
    /// # Arguments
    ///
    /// * `type_name` - The type name for repr (e.g., "sys.version_info")
    /// * `field_names` - Field names as interned StringIds, in order
    /// * `items` - Values corresponding to each field name
    ///
    /// # Panics
    ///
    /// Panics if `field_names.len() != items.len()`.
    #[must_use]
    pub fn new(name: impl Into<EitherStr>, field_names: Vec<EitherStr>, items: Vec<Value>) -> Self {
        assert_eq!(
            field_names.len(),
            items.len(),
            "NamedTuple field_names and items must have same length"
        );
        let contains_refs = items.iter().any(|v| matches!(v, Value::Ref(_)));
        Self {
            name: name.into(),
            field_names,
            items,
            contains_refs,
        }
    }

    /// Returns the type name (e.g., "sys.version_info").
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }

    /// Returns a reference to the field names.
    #[must_use]
    pub fn field_names(&self) -> &[EitherStr] {
        &self.field_names
    }

    /// Returns a reference to the underlying items vector.
    #[must_use]
    pub fn as_vec(&self) -> &Vec<Value> {
        &self.items
    }

    /// Returns the number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns whether the tuple contains any heap references.
    ///
    /// When false, `py_dec_ref_ids` can skip iteration.
    #[inline]
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// Gets a field value by name.
    ///
    /// Compares field names by actual string content, not just variant type.
    /// This allows lookup to work regardless of whether the field name was
    /// stored as an interned `StringId` or a heap-allocated `String`.
    ///
    /// Returns `Some(value)` if the field exists, `None` otherwise.
    #[must_use]
    pub fn get_by_name(&self, name_str: &str, interns: &Interns) -> Option<&Value> {
        self.field_names
            .iter()
            .position(|field_name| field_name.as_str(interns) == name_str)
            .map(|idx| &self.items[idx])
    }

    /// Gets a field value by index, supporting negative indexing.
    ///
    /// Returns `Some(value)` if the index is in bounds, `None` otherwise.
    /// Uses `index + len` instead of `-index` to avoid overflow on `i64::MIN`.
    #[must_use]
    pub fn get_by_index(&self, index: i64) -> Option<&Value> {
        let len = i64::try_from(self.items.len()).ok()?;
        let normalized = if index < 0 { index + len } else { index };
        if normalized < 0 || normalized >= len {
            return None;
        }
        self.items.get(usize::try_from(normalized).ok()?)
    }
}

impl PyTrait for NamedTuple {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::NamedTuple
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.py_estimate_size()
            + self.field_names.len() * std::mem::size_of::<StringId>()
            + self.items.len() * std::mem::size_of::<Value>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        Some(self.items.len())
    }

    fn py_getitem(&self, key: &Value, heap: &mut Heap<impl ResourceTracker>, _interns: &Interns) -> RunResult<Value> {
        // Extract integer index from key, returning TypeError if not an int
        let index = match key {
            Value::Int(i) => *i,
            _ => return Err(ExcType::type_error_indices(Type::NamedTuple, key.py_type(heap))),
        };

        // Get by index with bounds checking
        match self.get_by_index(index) {
            Some(value) => Ok(value.clone_with_heap(heap)),
            None => Err(ExcType::tuple_index_error()),
        }
    }

    fn py_eq(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> Result<bool, ResourceError> {
        // Compare only by items (not type_name) to match tuple semantics
        // This allows sys.version_info == (3, 14, 0, 'final', 0) to work
        if self.items.len() != other.items.len() {
            return Ok(false);
        }
        guard.increase_err()?;
        for (i1, i2) in self.items.iter().zip(&other.items) {
            if !i1.py_eq(i2, heap, guard, interns)? {
                guard.decrease();
                return Ok(false);
            }
        }
        guard.decrease();
        Ok(true)
    }

    /// Pushes all heap IDs contained in this named tuple onto the stack.
    ///
    /// Called during garbage collection to decrement refcounts of nested values.
    /// When `ref-count-panic` is enabled, also marks all Values as Dereferenced.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - GC optimization for tuples of primitives
        if !self.contains_refs {
            return;
        }
        for obj in &mut self.items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "ref-count-panic")]
                obj.dec_ref_forget();
            }
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        !self.items.is_empty()
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        guard: &mut DepthGuard,
        interns: &Interns,
    ) -> std::fmt::Result {
        // Check depth limit before recursing
        if !guard.increase() {
            return f.write_str("...");
        }

        // Format: type_name(field1=value1, field2=value2, ...)
        write!(f, "{}(", self.name.as_str(interns))?;

        let mut first = true;
        for (field_name, value) in self.field_names.iter().zip(&self.items) {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            f.write_str(field_name.as_str(interns))?;
            f.write_char('=')?;
            value.py_repr_fmt(f, heap, heap_ids, guard, interns)?;
        }

        f.write_char(')')?;
        guard.decrease();
        Ok(())
    }

    fn py_getattr(
        &self,
        attr: &EitherStr,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        let attr_name = attr.as_str(interns);
        if let Some(value) = self.get_by_name(attr_name, interns) {
            Ok(Some(AttrCallResult::Value(value.clone_with_heap(heap))))
        } else {
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            Err(ExcType::attribute_error(self.name(interns), attr_name))
        }
    }
}
