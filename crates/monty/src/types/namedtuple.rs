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
use std::{
    cell::Cell,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;

use super::PyTrait;
use crate::{
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    hash::HashValue,
    heap::{ContainsHeap, DropWithHeap, HeapId, HeapItem, HeapRead, RecursionToken},
    intern::{Interns, StringId},
    resource::ResourceTracker,
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
    /// Lazily-computed Python hash. Same rationale as [`super::Tuple::cached_hash`].
    #[serde(skip)]
    cached_hash: Cell<Option<HashValue>>,
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
            cached_hash: Cell::new(None),
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
}

impl<'h> HeapRead<'h, NamedTuple> {
    /// Returns `Some(value)` if the index is in bounds, `None` otherwise.
    /// Uses `index + len` instead of `-index` to avoid overflow on `i64::MIN`.
    #[must_use]
    pub fn get_by_index<'a>(&'a self, vm: &'a VM<'h, impl ResourceTracker>, index: i64) -> Option<&'a Value> {
        let len = i64::try_from(self.get(vm.heap).items.len()).ok()?;
        let normalized = if index < 0 { index + len } else { index };
        if normalized < 0 || normalized >= len {
            return None;
        }
        self.get(vm.heap).items.get(usize::try_from(normalized).ok()?)
    }

    /// Clones a single item.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h, impl ResourceTracker>) -> Value {
        self.get(vm.heap).items[index].clone_with_heap(vm)
    }

    /// Returns a stack-borrowed lending iterator over the named tuple's items,
    /// holding a recursion-depth token for its entire lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`NamedTupleIter`]) because that's the obvious entry point for
    /// "iterate this container".
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter<R: ResourceTracker>(&self, vm: &mut VM<'h, R>) -> RunResult<NamedTupleIter<'_, 'h>> {
        NamedTupleIter::new(self, vm)
    }

    /// Cross-type equality between NamedTuple and Tuple via HeapRead.
    pub(crate) fn eq_tuple(
        &self,
        other: &HeapRead<'h, super::Tuple>,
        vm: &mut VM<'h, impl ResourceTracker>,
    ) -> RunResult<bool> {
        if self.get(vm.heap).len() != other.get(vm.heap).as_slice().len() {
            return Ok(false);
        }
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, a)) = iter.next_with_index(vm)? {
            let b = other.clone_item(i, vm);
            defer_drop!(b, vm);
            if !a.py_eq(b, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Stack-borrowed lending iterator over a [`NamedTuple`]'s items.
///
/// Same shape as [`TupleIter`](super::tuple::TupleIter): yields each item by
/// reference, owns the most-recently-yielded item in a `Value::Undefined`
/// sentinel slot, and holds a [`RecursionToken`] for its lifetime. MUST be
/// wrapped in [`defer_drop_mut!`] so the token and the in-flight item are
/// released on every exit path.
///
/// `NamedTuple` is immutable, so there is no size-change detection — only
/// the recursion-depth bound matters here. Named-tuple iteration almost
/// always feeds into operations that recurse (`py_eq`, `py_hash`,
/// `py_repr`), and the token bounds the otherwise-unprotected native stack
/// depth.
pub(crate) struct NamedTupleIter<'a, 'h> {
    tuple: &'a HeapRead<'h, NamedTuple>,
    index: usize,
    token: RecursionToken,
    /// Most-recently-yielded item. `Value::Undefined` when nothing is held.
    current: Value,
}

impl<'a, 'h> NamedTupleIter<'a, 'h> {
    fn new<R: ResourceTracker>(tuple: &'a HeapRead<'h, NamedTuple>, vm: &mut VM<'h, R>) -> RunResult<Self> {
        let token = vm.heap.incr_recursion_depth()?;
        Ok(Self {
            tuple,
            index: 0,
            token,
            current: Value::Undefined,
        })
    }

    /// Advances the iterator and returns a borrow of the next item, or
    /// `Ok(None)` when the tuple is exhausted.
    ///
    /// The returned reference is valid until the next call to `next` (or
    /// until the iterator itself is dropped).
    ///
    /// Performs a [`check_time`](Heap::check_time) on every call so long
    /// Rust-side loops cannot bypass the configured timeout.
    pub(crate) fn next<'i, R: ResourceTracker>(&'i mut self, vm: &mut VM<'h, R>) -> RunResult<Option<&'i Value>> {
        // Drop the previously-yielded item (no-op when `current` is `Undefined`).
        mem::replace(&mut self.current, Value::Undefined).drop_with_heap(vm.heap);
        vm.heap.check_time()?;
        let items = &self.tuple.get(vm.heap).items;
        if self.index >= items.len() {
            return Ok(None);
        }
        self.current = items[self.index].clone_with_heap(vm.heap);
        self.index += 1;
        Ok(Some(&self.current))
    }

    /// Like [`next`](Self::next), but also returns the 0-based position of
    /// the yielded item — useful for `zip`-style sibling-container access.
    pub(crate) fn next_with_index<'i, R: ResourceTracker>(
        &'i mut self,
        vm: &mut VM<'h, R>,
    ) -> RunResult<Option<(usize, &'i Value)>> {
        // Capture before `next` increments `self.index`.
        let position = self.index;
        Ok(self.next(vm)?.map(|item| (position, item)))
    }
}

impl DropWithHeap for NamedTupleIter<'_, '_> {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        self.current.drop_with_heap(heap);
        self.token.drop_with_heap(heap);
    }
}

/// `PyTrait` implementation for `HeapRead<NamedTuple>`, providing all Python operations
/// on heap-allocated named tuples via short-lived borrow patterns.
impl<'h> PyTrait<'h> for HeapRead<'h, NamedTuple> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::NamedTuple
    }

    fn py_len(&self, vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Value> {
        // Extract integer index from key, returning TypeError if not an int
        let index = match key {
            Value::Int(i) => *i,
            _ => return Err(ExcType::type_error_indices(Type::NamedTuple, key.py_type(vm))),
        };

        // Get by index with bounds checking
        match self.get_by_index(vm, index) {
            Some(value) => Ok(value.clone_with_heap(vm.heap)),
            None => Err(ExcType::tuple_index_error()),
        }
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<bool> {
        if self.get(vm.heap).len() != other.get(vm.heap).len() {
            return Ok(false);
        }
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((i, a)) = iter.next_with_index(vm)? {
            let b = other.clone_item(i, vm);
            defer_drop!(b, vm);
            if !a.py_eq(b, vm)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Hashes by element only (not by class name), matching `Tuple::py_hash`
    /// so a `NamedTuple` and a `Tuple` with equal elements share the same hash.
    /// Caches the computed hash on first call (see `Tuple::py_hash` for the
    /// caching rationale).
    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        if let Some(cached) = self.get(vm.heap).cached_hash.get() {
            return Ok(Some(cached));
        }
        let mut hasher = DefaultHasher::new();
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(item) = iter.next(vm)? {
            match item.py_hash(vm)? {
                Some(h) => h.hash(&mut hasher),
                None => return Ok(None),
            }
        }
        let hash = HashValue::new(hasher.finish());
        self.get(vm.heap).cached_hash.set(Some(hash));
        Ok(Some(hash))
    }

    fn py_bool(&self, vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        self.get(vm.heap).len() > 0
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        // Check depth limit before recursing
        let Ok(token) = vm.heap.incr_recursion_depth() else {
            return Ok(f.write_str("...")?);
        };
        defer_drop!(token, vm);

        write!(f, "{}(", self.get(vm.heap).name.as_str(vm.interns))?;

        let len = self.get(vm.heap).items.len();
        for i in 0..len {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(self.get(vm.heap).field_names[i].as_str(vm.interns))?;
            f.write_char('=')?;
            let value = self.clone_item(i, vm);
            defer_drop!(value, vm);
            value.py_repr_fmt(f, vm, heap_ids)?;
        }

        f.write_char(')')?;
        Ok(())
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        if let Some(value) = self.get(vm.heap).get_by_name(attr_name, vm.interns) {
            Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap))))
        } else {
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            Err(ExcType::attribute_error(self.get(vm.heap).name(vm.interns), attr_name))
        }
    }
}

impl HeapItem for NamedTuple {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
            + self.name.py_estimate_size()
            + self.field_names.len() * mem::size_of::<StringId>()
            + self.items.len() * mem::size_of::<Value>()
    }

    /// Pushes all heap IDs contained in this named tuple onto the stack.
    ///
    /// Called during garbage collection to decrement refcounts of nested values.
    /// When `memory-model-checks` is enabled, also marks all Values as Dereferenced.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Skip iteration if no refs - GC optimization for tuples of primitives
        if !self.contains_refs {
            return;
        }
        for obj in &mut self.items {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                obj.dec_ref_forget();
            }
        }
    }
}
