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
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{HeapId, HeapItem, HeapRead},
    intern::{Interns, StringId},
    resource::{ResourceError, ResourceTracker},
    types::Type,
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

    /// Returns the number of items in the tuple.
    ///
    /// Alias for `len()` used by `HeapReader` for direct item access.
    #[must_use]
    pub(crate) fn items_len(&self) -> usize {
        self.items.len()
    }

    /// Returns a reference to the item at the given index.
    ///
    /// Used by `HeapReader` for direct item access without cloning.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.items.len()`.
    #[must_use]
    pub(crate) fn item_ref(&self, idx: usize) -> &Value {
        &self.items[idx]
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
}

impl<'h> HeapRead<'h, NamedTuple> {
    /// Clones a single item using the two-phase borrow pattern.
    ///
    /// For `Value::Ref`, copies the `HeapId` via a short-lived shared borrow, then
    /// increments the refcount via a separate mutable operation. For immediate values,
    /// reads via a short-lived borrow and clones without touching the heap.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Value {
        let ref_id = match self.get(vm.heap).item_ref(index) {
            Value::Ref(id) => Some(*id),
            _ => None,
        };
        if let Some(id) = ref_id {
            vm.heap.inc_ref(id);
            Value::Ref(id)
        } else {
            self.get(vm.heap).item_ref(index).clone_immediate()
        }
    }

    /// Delegates to `py_eq` for backward compatibility with `value.rs` call sites.
    pub(crate) fn eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        self.py_eq(other, vm)
    }

    /// Cross-type equality between NamedTuple and Tuple via HeapRead.
    ///
    /// Uses index-based item access with short-lived borrows to compare elements
    /// without holding a heap borrow across `py_eq` calls.
    pub(crate) fn eq_tuple(
        &self,
        other: &HeapRead<'h, super::Tuple>,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<bool, ResourceError> {
        let a_len = self.get(vm.heap).len();
        if a_len != other.get(vm.heap).as_slice().len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        for i in 0..a_len {
            vm.heap.check_time()?;
            let a_val = self.clone_item(i, vm);
            let b_val = other.clone_item(i, vm);
            let result = a_val.py_eq(&b_val, vm);
            a_val.drop_with_heap(vm);
            b_val.drop_with_heap(vm);
            if !result? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// `PyTrait` implementation for `HeapRead<NamedTuple>`, providing all Python operations
/// on heap-allocated named tuples via short-lived borrow patterns.
impl<'h> PyTrait<'h> for HeapRead<'h, NamedTuple> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::NamedTuple
    }

    fn py_len(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_bool(&self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        self.get(vm.heap).len() > 0
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let nt = self.get(vm.heap);
        namedtuple_repr_fmt(&nt.name, &nt.field_names, &nt.items, f, vm, heap_ids)
    }

    /// Element-wise equality using the short-lived borrow pattern.
    ///
    /// Compares only by items (not type name) to match tuple semantics,
    /// allowing `sys.version_info == (3, 14, 0, 'final', 0)` to work.
    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        let a_len = self.get(vm.heap).len();
        if a_len != other.get(vm.heap).len() {
            return Ok(false);
        }
        let token = vm.heap.incr_recursion_depth()?;
        defer_drop!(token, vm);
        for i in 0..a_len {
            vm.heap.check_time()?;
            let a_val = self.clone_item(i, vm);
            let b_val = other.clone_item(i, vm);
            let result = a_val.py_eq(&b_val, vm);
            a_val.drop_with_heap(vm);
            b_val.drop_with_heap(vm);
            if !result? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Subscript access via HeapRead. Handles integer indices with negative indexing.
    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Value> {
        let index = match key {
            Value::Int(i) => *i,
            _ => return Err(ExcType::type_error_indices(Type::NamedTuple, key.py_type(vm))),
        };

        let len = self.get(vm.heap).items_len();
        let len_i64 = i64::try_from(len).expect("namedtuple length exceeds i64::MAX");
        let normalized = if index < 0 { index + len_i64 } else { index };
        if normalized < 0 || normalized >= len_i64 {
            return Err(ExcType::tuple_index_error());
        }

        let idx = usize::try_from(normalized).expect("namedtuple index validated non-negative");
        Ok(self.clone_item(idx, vm))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        if let Some(value) = self.get(vm.heap).get_by_name(attr_name, vm.interns) {
            Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap))))
        } else {
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            Err(ExcType::attribute_error(self.get(vm.heap).name(vm.interns), attr_name))
        }
    }
}

/// Writes the repr of a named tuple to a formatter.
///
/// Format: `type_name(field1=value1, field2=value2, ...)`
pub(crate) fn namedtuple_repr_fmt(
    name: &EitherStr,
    field_names: &[EitherStr],
    items: &[Value],
    f: &mut impl Write,
    vm: &VM<'_, '_, impl ResourceTracker>,
    heap_ids: &mut AHashSet<HeapId>,
) -> RunResult<()> {
    // Check depth limit before recursing
    let heap = &*vm.heap;
    let Some(token) = heap.incr_recursion_depth_for_repr() else {
        return Ok(f.write_str("...")?);
    };
    crate::defer_drop_immutable_heap!(token, heap);

    write!(f, "{}(", name.as_str(vm.interns))?;

    let mut first = true;
    for (field_name, value) in field_names.iter().zip(items) {
        if !first {
            f.write_str(", ")?;
        }
        first = false;
        f.write_str(field_name.as_str(vm.interns))?;
        f.write_char('=')?;
        value.py_repr_fmt(f, vm, heap_ids)?;
    }

    f.write_char(')')?;
    Ok(())
}

impl HeapItem for NamedTuple {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.py_estimate_size()
            + self.field_names.len() * std::mem::size_of::<StringId>()
            + self.items.len() * std::mem::size_of::<Value>()
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
}
