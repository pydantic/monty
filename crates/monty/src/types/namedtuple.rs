use std::{
    cell::Cell,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    iter::once,
    mem,
};

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
use smallvec::SmallVec;

use super::{CmpOrder, PyTrait, tuple::TupleIterator};
use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, ContainsVM, RecursionToken, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    resource_checks::check_repeat_size,
    types::{
        Dict, Type, allocate_tuple,
        iter::collect_owned_iterable,
        long_int::repeat_count,
        py_trait::LazyHeapSet,
        slice::{normalize_sequence_index, slice_collect_iterator},
        str::allocate_string,
        tuple::TupleVec,
    },
    value::{EitherStr, VALUE_SIZE, Value},
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
    /// The [`NamedTupleClass`] this instance was built from, when it came from a
    /// `collections.namedtuple` factory. `None` for the self-describing named
    /// tuples Monty creates internally (`sys.version_info`, host imports), which
    /// have no class object. When `Some`, the instance owns a counted reference
    /// to the class so `type(p) is Point` resolves by identity (see [`type_of`]).
    class_id: Option<HeapId>,
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
    /// * `field_names` - Field names in order; interned for Monty's internal
    ///   named tuples, owned `String`s for `collections.namedtuple` classes
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
        Self::with_class(name, field_names, items, None)
    }

    /// Creates a named tuple that remembers the [`NamedTupleClass`] it was built
    /// from (a `collections.namedtuple` instance).
    ///
    /// The caller MUST have already incremented the class's refcount — the
    /// instance takes ownership of that reference and releases it in
    /// [`py_dec_ref_ids`](NamedTuple::py_dec_ref_ids).
    ///
    /// # Panics
    ///
    /// Panics if `field_names.len() != items.len()`.
    #[must_use]
    pub fn with_class(
        name: impl Into<EitherStr>,
        field_names: Vec<EitherStr>,
        items: Vec<Value>,
        class_id: Option<HeapId>,
    ) -> Self {
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
            class_id,
            contains_refs,
            cached_hash: Cell::new(None),
        }
    }

    /// Returns the class object this instance was built from, if any.
    #[must_use]
    pub fn class_id(&self) -> Option<HeapId> {
        self.class_id
    }

    /// Returns the type name (e.g., "sys.version_info").
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }

    /// Returns the type name unresolved, for callers that must produce a
    /// `Cow` borrowing only `Interns` (error messages formatted after heap
    /// cleanup) rather than a `&str` borrowing this entry too.
    #[must_use]
    pub fn name_either(&self) -> &EitherStr {
        &self.name
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
    pub fn get_by_index<'a>(&'a self, vm: &'a VM<'h>, index: i64) -> Option<&'a Value> {
        let len = i64::try_from(self.get(vm.heap).items.len()).ok()?;
        let normalized = if index < 0 { index + len } else { index };
        if normalized < 0 || normalized >= len {
            return None;
        }
        self.get(vm.heap).items.get(usize::try_from(normalized).ok()?)
    }

    /// Clones a single item.
    pub(crate) fn clone_item(&self, index: usize, vm: &mut VM<'h>) -> Value {
        self.get(vm.heap).items[index].clone_with_heap(vm)
    }

    /// Clones every item, for the orderings in [`cmp_item_seqs`].
    ///
    /// Preflights the slot bytes so an over-budget clone raises a graceful
    /// `MemoryError` instead of bursting past the allocator's hard limit.
    pub(crate) fn cloned_items(&self, vm: &mut VM<'h>) -> RunResult<Vec<Value>> {
        let len = self.get(vm.heap).len();
        vm.heap.tracker().check_allocation(len.saturating_mul(VALUE_SIZE))?;
        Ok((0..len).map(|i| self.clone_item(i, vm)).collect())
    }

    /// Returns a stack-borrowed lending iterator over the named tuple's items,
    /// holding a recursion-depth token for its entire lifetime.
    ///
    /// Named `iter` despite returning a non-stdlib lending iterator (see
    /// [`NamedTupleIter`]) because that's the obvious entry point for
    /// "iterate this container".
    #[expect(clippy::iter_not_returning_iterator)]
    pub(crate) fn iter(&self, vm: &mut VM<'h>) -> RunResult<NamedTupleIter<'_, 'h>> {
        NamedTupleIter::new(self, vm)
    }

    /// Cross-type equality between NamedTuple and Tuple via HeapRead.
    pub(crate) fn eq_tuple(&self, other: &HeapRead<'h, super::Tuple>, vm: &mut VM<'h>) -> RunResult<bool> {
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
    fn new(tuple: &'a HeapRead<'h, NamedTuple>, vm: &mut VM<'h>) -> RunResult<Self> {
        let token = vm.recursion_token()?;
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
    pub(crate) fn next<'i>(&'i mut self, vm: &mut VM<'h>) -> RunResult<Option<&'i Value>> {
        // Drop the previously-yielded item (no-op when `current` is `Undefined`).
        mem::replace(&mut self.current, Value::Undefined).drop_with(vm.heap);
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
    pub(crate) fn next_with_index<'i>(&'i mut self, vm: &mut VM<'h>) -> RunResult<Option<(usize, &'i Value)>> {
        // Capture before `next` increments `self.index`.
        let position = self.index;
        Ok(self.next(vm)?.map(|item| (position, item)))
    }
}

impl<'h, C: ContainsVM<'h>> DropWithContext<C> for NamedTupleIter<'_, 'h> {
    fn drop_with(self, container: &mut C) {
        self.current.drop_with(container);
        self.token.drop_with(container);
    }
}

/// `PyTrait` implementation for `HeapRead<NamedTuple>`, providing all Python operations
/// on heap-allocated named tuples via short-lived borrow patterns.
impl<'h> PyTrait<'h> for HeapRead<'h, NamedTuple> {
    fn py_is_iterable(&self, _vm: &VM<'h>) -> bool {
        true
    }

    /// Linear search by equality like [`Tuple`](super::Tuple) — a namedtuple is a
    /// tuple subclass in CPython and inherits `tuplecontains`. Without this, `in`
    /// falls back to iteration and allocates a heap `TupleIterator`, which can
    /// trip the allocation limit on a tight heap.
    fn py_contains_impl(&self, _self_id: HeapId, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(el) = iter.next(vm)? {
            if el.py_eq(item, vm)? {
                return Ok(Some(true));
            }
        }
        Ok(Some(false))
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::NamedTuple
    }

    fn py_iter(&self, self_id: Option<HeapId>, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(TupleIterator::from_named_tuple(
            self_id.expect("heap values have an id"),
            vm,
        ))
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).len())
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        // A slice degrades to a plain tuple, as in CPython — the field names
        // describe the original instance only, so they cannot survive a slice.
        if let Value::Ref(key_id) = key
            && let HeapData::Slice(slice_obj) = vm.heap.get(*key_id)
        {
            let items = slice_collect_iterator(vm, slice_obj, self.get(vm.heap).as_vec().iter(), |v| {
                v.clone_with_heap(vm)
            })?;
            return Ok(allocate_tuple(items, vm.heap));
        }

        // Extract integer index from key, returning TypeError if not an int
        let index = match key {
            Value::Int(i) => *i,
            // Reported as `tuple`, not `namedtuple`: CPython raises this from the
            // inherited `tuple.__getitem__`, so the subclass name never appears.
            _ => return Err(ExcType::type_error_indices(Type::Tuple, &key.py_type_name(vm))),
        };

        // Get by index with bounds checking
        match self.get_by_index(vm, index) {
            Some(value) => Ok(value.clone_with_heap(vm.heap)),
            None => Err(ExcType::tuple_index_error()),
        }
    }

    /// `namedtuple + tuple-like` — concatenation into a plain tuple, as in
    /// CPython (the field names describe one instance only, so they cannot
    /// survive concatenation). A non-tuple-like right operand returns `None`.
    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>, _self_id: Option<HeapId>) -> RunResult<Option<Value>> {
        let Some(mut other_items) = cloned_tuple_like_items(other, vm)? else {
            return Ok(None);
        };
        let mut items = self.cloned_items(vm)?;
        items.append(&mut other_items);
        Ok(Some(allocate_tuple(SmallVec::from_vec(items), vm.heap)))
    }

    /// Reflected concatenation (`tuple + namedtuple`), reached when the left
    /// tuple's `py_add_impl` declined the namedtuple right operand.
    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(mut items) = cloned_tuple_like_items(other, vm)? else {
            return Ok(None);
        };
        let mut self_items = self.cloned_items(vm)?;
        items.append(&mut self_items);
        Ok(Some(allocate_tuple(SmallVec::from_vec(items), vm.heap)))
    }

    /// `namedtuple * int` — repetition into a plain tuple, matching CPython's
    /// inherited `tuple.__mul__`.
    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = repeat_count(other, vm)? else {
            return Ok(None);
        };
        let len = self.get(vm.heap).len();
        if count == 0 || len == 0 {
            return Ok(Some(vm.heap.get_empty_tuple()));
        }
        check_repeat_size(len.saturating_mul(mem::size_of::<Value>()), count, vm.heap.tracker())?;
        let mut result: TupleVec = SmallVec::with_capacity(len * count);
        for _ in 0..count {
            for i in 0..len {
                let item = self.get(vm.heap).as_vec()[i].clone_with_heap(vm.heap);
                result.push(item);
            }
            vm.heap.check_time()?;
        }
        Ok(Some(allocate_tuple(result, vm.heap)))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // A namedtuple equals another namedtuple element-wise, and also equals a
        // plain tuple with the same elements (class name is ignored). Both
        // directions of the tuple case are covered here, so `Tuple::py_eq_impl`
        // need not know about namedtuples.
        match other.read_heap(vm) {
            Some(HeapReadOutput::NamedTuple(other)) => {
                if self.get(vm.heap).len() != other.get(vm.heap).len() {
                    return Ok(Some(false));
                }
                let iter = self.iter(vm)?;
                defer_drop_mut!(iter, vm);
                while let Some((i, a)) = iter.next_with_index(vm)? {
                    let b = other.clone_item(i, vm);
                    defer_drop!(b, vm);
                    if !a.py_eq(b, vm)? {
                        return Ok(Some(false));
                    }
                }
                Ok(Some(true))
            }
            Some(HeapReadOutput::Tuple(other)) => Ok(Some(self.eq_tuple(&other, vm)?)),
            _ => Ok(None),
        }
    }

    /// Hashes by element only (not by class name), matching `Tuple::py_hash`
    /// so a `NamedTuple` and a `Tuple` with equal elements share the same hash.
    /// Caches the computed hash on first call (see `Tuple::py_hash` for the
    /// caching rationale).
    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
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

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(self.get(vm.heap).len() > 0)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        // Check depth limit before recursing
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;

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

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        // `_fields` is a data attribute (a tuple of the field names); field
        // lookup by that name is impossible since fields cannot start with `_`.
        // Gated on `class_id`: Monty's internal named tuples (`sys.version_info`,
        // host imports) model CPython *structseqs*, which expose none of the
        // `collections.namedtuple` API.
        if attr.static_string() == Some(StaticStrings::UnderFields) && self.get(vm.heap).class_id().is_some() {
            return Ok(Some(CallResult::Value(self.fields_tuple(vm))));
        }
        // `_field_defaults` lives only on the class (an instance stores field
        // *names* but not defaults), so it is read through `class_id` rather than
        // rebuilt here — the two spellings then cannot drift apart.
        if attr.static_string() == Some(StaticStrings::UnderFieldDefaults)
            && let Some(class_id) = self.get(vm.heap).class_id()
            && let HeapReadOutput::NamedTupleClass(class) = vm.heap.read(class_id)
        {
            return Ok(Some(CallResult::Value(class.field_defaults_dict(vm)?)));
        }
        // `__doc__` / `__module__` are class attributes an instance inherits, so
        // read them from the class object rather than duplicating them per instance.
        if let Some(class_id) = self.get(vm.heap).class_id()
            && let Some(static_attr @ (StaticStrings::DunderDoc | StaticStrings::DunderModule)) = attr.static_string()
            && let HeapData::NamedTupleClass(class) = vm.heap.get(class_id)
        {
            let value = if static_attr == StaticStrings::DunderDoc {
                let doc = synthesise_doc(class, vm.interns);
                allocate_string(doc, vm.heap)
            } else {
                class.module().clone_with_heap(vm.heap)
            };
            return Ok(Some(CallResult::Value(value)));
        }
        if let Some(value) = self.get(vm.heap).get_by_name(attr_name, vm.interns) {
            Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap))))
        } else {
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            Err(ExcType::attribute_error(self.get(vm.heap).name(vm.interns), attr_name))
        }
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        // The `_`-prefixed namedtuple methods. Field names cannot start with `_`,
        // so these never collide with a real field. Only instances built from a
        // `collections.namedtuple` class have them — see the note in `py_getattr`.
        let from_factory = self.get(vm.heap).class_id().is_some();
        // `count`/`index` are inherited from `tuple`, so unlike the `_`-prefixed
        // methods they are available on structseqs too (`sys.version_info.count(0)`
        // works in CPython).
        match attr.static_string() {
            Some(StaticStrings::Count) => return self.method_count(vm, args).map(CallResult::Value),
            Some(StaticStrings::Index) => return self.method_index(vm, args).map(CallResult::Value),
            Some(StaticStrings::DunderGetnewargs) => {
                return self.method_getnewargs(vm, args, from_factory).map(CallResult::Value);
            }
            _ => {}
        }
        match attr.static_string().filter(|_| from_factory) {
            Some(StaticStrings::UnderAsdict) => self.method_asdict(vm, args).map(CallResult::Value),
            Some(StaticStrings::UnderReplace) => self.method_replace(vm, args).map(CallResult::Value),
            Some(StaticStrings::UnderMake) => self.method_make(vm, args).map(CallResult::Value),
            _ => {
                args.drop_with(vm);
                Err(ExcType::attribute_error(
                    self.get(vm.heap).name(vm.interns),
                    attr.as_str(vm.interns),
                ))
            }
        }
    }
}

impl HeapItem for NamedTuple {
    /// Pushes all heap IDs contained in this named tuple onto the stack.
    ///
    /// Called during garbage collection to decrement refcounts of nested values.
    /// When `memory-model-checks` is enabled, also marks all Values as Dereferenced.
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Release the owned reference to the class object (factory instances only).
        if let Some(class_id) = self.class_id {
            stack.push(class_id);
        }
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

// ============================================================================
// Named-tuple `_`-prefixed methods (instance side).
// ============================================================================

impl<'h> HeapRead<'h, NamedTuple> {
    /// `p._fields` — a tuple of the field names, in order.
    fn fields_tuple(&self, vm: &mut VM<'h>) -> Value {
        let names = self.get(vm.heap).field_names.clone();
        fields_tuple_from(&names, vm)
    }

    /// `p.__getnewargs__()` — the arguments needed to rebuild this instance.
    ///
    /// The shape differs by origin, matching CPython: a `collections.namedtuple`
    /// class generates `__getnewargs__` returning `tuple(self)` (its `__new__`
    /// takes one argument per field), whereas a *structseq* returns
    /// `(tuple(self),)` because its `__new__` takes a single sequence. So
    /// `sys.version_info.__getnewargs__()` is `((3, 14, ...),)`, one level deeper.
    fn method_getnewargs(&self, vm: &mut VM<'h>, args: ArgValues, from_factory: bool) -> RunResult<Value> {
        args.check_zero_args("__getnewargs__", vm.heap)?;
        let items: TupleVec = self.cloned_items(vm)?.into_iter().collect();
        let values = allocate_tuple(items, vm.heap);
        if from_factory {
            Ok(values)
        } else {
            // `Value` is deliberately not `Clone` (manual refcounting), so build
            // the one-element wrapper by pushing rather than `from_elem`.
            let mut wrapper = TupleVec::new();
            wrapper.push(values);
            Ok(allocate_tuple(wrapper, vm.heap))
        }
    }

    /// `p.count(x)` — how many items equal `x`, inherited from `tuple`.
    fn method_count(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        let value = args.get_one_arg("tuple.count", vm.heap)?;
        defer_drop!(value, vm);

        let mut count = 0usize;
        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some(item) = iter.next(vm)? {
            if value.py_eq(item, vm)? {
                count += 1;
            }
        }
        Ok(Value::Int(i64::try_from(count).expect("count exceeds i64::MAX")))
    }

    /// `p.index(x[, start[, stop]])` — index of the first item equal to `x`,
    /// inherited from `tuple`.
    fn method_index(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        let pos_args = args.into_pos_only("tuple.index", vm.heap)?;
        defer_drop!(pos_args, vm);

        let len = self.get(vm.heap).len();
        let (value, start, end) = match pos_args.as_slice() {
            [] => return Err(ExcType::type_error_at_least("tuple.index", 1, 0)),
            [value] => (value, 0, len),
            [value, start_arg] => {
                let start = normalize_sequence_index(start_arg.as_int(vm)?, len);
                (value, start, len)
            }
            [value, start_arg, end_arg] => {
                let start = normalize_sequence_index(start_arg.as_int(vm)?, len);
                let end = normalize_sequence_index(end_arg.as_int(vm)?, len).max(start);
                (value, start, end)
            }
            other => return Err(ExcType::type_error_at_most("tuple.index", 3, other.len())),
        };

        let iter = self.iter(vm)?;
        defer_drop_mut!(iter, vm);
        while let Some((idx, item)) = iter.next_with_index(vm)? {
            if idx >= end {
                // No further matches possible inside [start, end).
                break;
            }
            if idx >= start && value.py_eq(item, vm)? {
                return Ok(Value::Int(i64::try_from(idx).expect("index exceeds i64::MAX")));
            }
        }
        Err(ExcType::value_error_not_in_tuple())
    }

    /// `p._asdict()` — a new `dict` mapping each field name to its value.
    fn method_asdict(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        args.check_zero_args("_asdict", vm.heap)?;
        let names = self.get(vm.heap).field_names.clone();
        let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(names.len());
        for (i, name) in names.iter().enumerate() {
            let key = field_name_value(name, vm);
            let value = self.get(vm.heap).items[i].clone_with_heap(vm.heap);
            pairs.push((key, value));
        }
        let dict = Dict::from_pairs(pairs, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
    }

    /// `p._replace(**kwargs)` — a copy with the named fields overridden.
    ///
    /// Unknown field names raise `ValueError: Got unexpected field names: [...]`,
    /// matching CPython's list-repr wording.
    fn method_replace(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        let name = self.get(vm.heap).name.clone();
        let field_names = self.get(vm.heap).field_names.clone();
        let class_id = self.get(vm.heap).class_id;
        let n = field_names.len();
        let mut items: Vec<Value> = (0..n)
            .map(|i| self.get(vm.heap).items[i].clone_with_heap(vm.heap))
            .collect();

        let (pos, kwargs) = args.into_parts();
        let n_pos = pos.len();
        if n_pos > 0 {
            pos.drop_with(vm);
            kwargs.drop_with(vm);
            items.drop_with(vm);
            return Err(ExcType::type_error(format!(
                "_replace() takes 1 positional argument but {} were given",
                n_pos + 1
            )));
        }

        let mut unexpected: Vec<String> = Vec::new();
        for (key, val) in normalize_kwargs(kwargs, vm) {
            let key_str = key.as_str(vm.interns);
            if let Some(i) = field_names.iter().position(|f| f.as_str(vm.interns) == key_str) {
                let old = mem::replace(&mut items[i], val);
                old.drop_with(vm);
            } else {
                unexpected.push(key_str.to_owned());
                val.drop_with(vm);
            }
        }
        if !unexpected.is_empty() {
            items.drop_with(vm);
            return Err(ExcType::type_error(format!(
                "Got unexpected field names: {}",
                py_list_repr(&unexpected)
            )));
        }
        Ok(build_namedtuple(name, field_names, items, class_id, vm))
    }

    /// `p._make(iterable)` — a new instance of the same class from an iterable.
    fn method_make(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        let name = self.get(vm.heap).name.clone();
        let field_names = self.get(vm.heap).field_names.clone();
        let class_id = self.get(vm.heap).class_id;
        make_from_iterable(name, field_names, class_id, vm, args)
    }
}

// ============================================================================
// `NamedTupleClass` — the callable class object produced by
// `collections.namedtuple`.
// ============================================================================

/// The class object returned by `collections.namedtuple('Name', [...])`.
///
/// Analogous to a [`Class`](super::Class) but purpose-built: calling it
/// constructs a [`NamedTuple`] instance (not a generic `Instance`), and it
/// exposes the namedtuple class API (`_fields`, `_field_defaults`, `_make`).
/// Like `Class`, its own [`HeapId`] is its type identity, so `type(p) is Point`
/// resolves by reference (see [`type_of`](crate::builtins::type_::builtin_type)).
///
/// The name is heap-owned ([`EitherStr`]) because factory names are chosen at
/// runtime, after the intern table is frozen.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamedTupleClass {
    /// The class name (e.g. `Point`), used for `__name__`, `repr`, and the
    /// repr of every instance built from it.
    name: EitherStr,
    /// Field names in order.
    field_names: Vec<EitherStr>,
    /// Default values for the trailing `defaults.len()` fields.
    defaults: Vec<Value>,
    /// `__module__`. CPython stores the `module=` argument unvalidated (it can be
    /// any object), substituting the calling module's `__name__` when it is `None`
    /// — always `'__main__'` here, since Monty has a single module.
    module: Value,
    /// True if any default *or* `module` is a `Value::Ref` (GC skip optimization).
    contains_refs: bool,
}

impl NamedTupleClass {
    /// Creates a named-tuple class from its validated name, fields, and defaults.
    ///
    /// The caller MUST have already validated the field names (identifiers,
    /// non-keyword, unique, no leading underscore) — this type does no checking.
    /// `module` is taken by value: the class owns that reference from here on.
    #[must_use]
    pub fn new(name: impl Into<EitherStr>, field_names: Vec<EitherStr>, defaults: Vec<Value>, module: Value) -> Self {
        let contains_refs = defaults.iter().any(|v| matches!(v, Value::Ref(_))) || matches!(module, Value::Ref(_));
        Self {
            name: name.into(),
            field_names,
            defaults,
            module,
            contains_refs,
        }
    }

    /// The stored `__module__` value, for GC tracing and attribute reads.
    pub(crate) fn module(&self) -> &Value {
        &self.module
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, NamedTupleClass> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        // The type of a class object is `type` (matching `type(Point) is type`).
        Type::Type
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // Class objects compare by identity, resolved before reaching here.
        Ok(None)
    }

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self_id)))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<class '{}'>", self.get(vm.heap).name.as_str(vm.interns))?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let value = match attr.static_string() {
            // `namedtuple` assigns `__qualname__ = typename` outright, so it always
            // equals `__name__` and never picks up a dotted path from an enclosing scope.
            Some(StaticStrings::DunderName | StaticStrings::DunderQualname) => {
                let name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
                allocate_string(name, vm.heap)
            }
            Some(StaticStrings::UnderFields) => {
                let names = self.get(vm.heap).field_names.clone();
                fields_tuple_from(&names, vm)
            }
            Some(StaticStrings::UnderFieldDefaults) => self.field_defaults_dict(vm)?,
            Some(StaticStrings::DunderDoc) => {
                let doc = synthesise_doc(self.get(vm.heap), vm.interns);
                allocate_string(doc, vm.heap)
            }
            Some(StaticStrings::DunderModule) => self.get(vm.heap).module.clone_with_heap(vm.heap),
            _ => {
                return Err(ExcType::attribute_error_type(
                    self.get(vm.heap).name.as_str(vm.interns),
                    attr.as_str(vm.interns),
                ));
            }
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        if attr.static_string() == Some(StaticStrings::UnderMake) {
            make_namedtuple(self_id, vm, args).map(CallResult::Value)
        } else {
            args.drop_with(vm);
            Err(ExcType::attribute_error_type(
                self.get(vm.heap).name.as_str(vm.interns),
                attr.as_str(vm.interns),
            ))
        }
    }
}

impl<'h> HeapRead<'h, NamedTupleClass> {
    /// `Point._field_defaults` — a dict of the defaulted (trailing) field names
    /// to their default values. Empty when the class has no defaults.
    fn field_defaults_dict(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let n = self.get(vm.heap).field_names.len();
        let n_defaults = self.get(vm.heap).defaults.len();
        let start = n - n_defaults;
        let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(n_defaults);
        for j in 0..n_defaults {
            let key = field_name_value(&self.get(vm.heap).field_names[start + j].clone(), vm);
            let value = self.get(vm.heap).defaults[j].clone_with_heap(vm.heap);
            pairs.push((key, value));
        }
        let dict = Dict::from_pairs(pairs, vm)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
    }
}

impl NamedTupleClass {
    /// Whether any default value is a heap reference (GC-walker skip hint).
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// The default values, in field order (for GC traversal).
    #[must_use]
    pub fn defaults(&self) -> &[Value] {
        &self.defaults
    }
}

impl HeapItem for NamedTupleClass {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if !self.contains_refs {
            return;
        }
        // MUST report exactly the same ids as the `NamedTupleClass` arm of
        // `for_each_child_id` — `module` is owned just like the defaults are.
        for obj in self.defaults.iter_mut().chain(once(&mut self.module)) {
            if let Value::Ref(id) = obj {
                stack.push(*id);
                #[cfg(feature = "memory-model-checks")]
                obj.dec_ref_forget();
            }
        }
    }
}

// ============================================================================
// Construction and shared helpers.
// ============================================================================

/// Lexicographic ordering between two cloned item sequences.
///
/// A namedtuple is a tuple subclass, so it orders element-wise against another
/// namedtuple *or* a plain tuple, with the shorter sequence sorting first when
/// it is a prefix of the longer. The class and field names take no part, so two
/// different namedtuple classes compare purely by value (matching CPython).
///
/// Takes both sides already cloned — the comparison needs `&mut VM`, which the
/// caller cannot hold alongside two live `HeapRead`s. Both vectors are consumed
/// and their items dropped.
///
/// Element-level results propagate exactly as in [`Tuple::py_cmp`]: a `NaN`
/// yields [`CmpOrder::Unordered`] rather than an error, while a genuinely
/// type-mismatched pair yields [`CmpOrder::Incomparable`].
///
/// Charges one recursion level: unlike equality/repr, ordering does not walk a
/// token-bearing `NamedTupleIter` (it compares detached item vecs), so nested
/// namedtuples (`a < b` where each wraps the last) would otherwise recurse
/// through here per level and overflow the host stack instead of raising
/// `RecursionError`. A [`RecursionToken`] rather than a `recursion_guard()` is
/// used because both vecs are still owned on the failure path — the token does
/// not borrow the VM, so they can be dropped before returning the error.
pub(crate) fn cmp_item_seqs(a: Vec<Value>, b: Vec<Value>, vm: &mut VM<'_>) -> RunResult<CmpOrder> {
    let token = match vm.recursion_token() {
        Ok(token) => token,
        Err(err) => {
            a.drop_with(vm);
            b.drop_with(vm);
            return Err(err.into());
        }
    };

    let (a_len, b_len) = (a.len(), b.len());
    let mut result = None;
    for (av, bv) in a.iter().zip(b.iter()) {
        match av.py_cmp(bv, vm) {
            Ok(CmpOrder::Ordered(Ordering::Equal)) => {}
            Ok(CmpOrder::Ordered(ord)) => {
                result = Some(Ok(CmpOrder::Ordered(ord)));
                break;
            }
            Ok(CmpOrder::Unordered) => {
                result = Some(Ok(CmpOrder::Unordered));
                break;
            }
            Ok(CmpOrder::Incomparable) => {
                // CPython checks `__eq__` first and only orders non-equal pairs,
                // so equal-but-unorderable elements (e.g. `None == None`) do not
                // block the comparison.
                match av.py_eq(bv, vm) {
                    Ok(true) => {}
                    Ok(false) => {
                        result = Some(Ok(CmpOrder::Incomparable));
                        break;
                    }
                    Err(e) => {
                        result = Some(Err(e));
                        break;
                    }
                }
            }
            Err(e) => {
                result = Some(Err(e));
                break;
            }
        }
    }
    a.drop_with(vm);
    b.drop_with(vm);
    token.drop_with(vm);
    // All compared pairs were equal, so the shorter sequence sorts first.
    result.unwrap_or_else(|| Ok(CmpOrder::Ordered(a_len.cmp(&b_len))))
}

/// Clones the items of a tuple-like value (`tuple` or `namedtuple`) into an
/// owned `Vec`, incrementing refcounts, or returns `Ok(None)` for anything
/// else. Preflights the slot bytes like [`HeapRead::cloned_items`].
///
/// Shared by named-tuple concatenation (`+`), whose result is always a plain
/// tuple regardless of which operand is the namedtuple.
fn cloned_tuple_like_items(value: &Value, vm: &VM<'_>) -> RunResult<Option<Vec<Value>>> {
    let Value::Ref(id) = value else {
        return Ok(None);
    };
    let id = *id;
    let len = match vm.heap.get(id) {
        HeapData::Tuple(t) => t.as_slice().len(),
        HeapData::NamedTuple(nt) => nt.as_vec().len(),
        _ => return Ok(None),
    };
    vm.heap.tracker().check_allocation(len.saturating_mul(VALUE_SIZE))?;
    let mut items = Vec::with_capacity(len);
    for i in 0..len {
        let item = match vm.heap.get(id) {
            HeapData::Tuple(t) => t.as_slice()[i].clone_with_heap(vm.heap),
            HeapData::NamedTuple(nt) => nt.as_vec()[i].clone_with_heap(vm.heap),
            _ => unreachable!("length branch already matched a tuple-like variant"),
        };
        items.push(item);
    }
    Ok(Some(items))
}

/// Builds the synthesised class docstring, e.g. `Point(x, y)`.
///
/// CPython derives it from `repr(field_names)` with the quotes stripped and the
/// outer brackets sliced off, so a *single*-field class keeps the one-tuple's
/// trailing comma (`Q(a,)`) — a quirk worth preserving since it is observable.
/// The result is bounded by the already-tracked field names, so a plain `String`
/// is safe here (no `StringBuilder` amplification risk).
fn synthesise_doc(class: &NamedTupleClass, interns: &Interns) -> String {
    let names = class.field_names.iter().map(|n| n.as_str(interns));
    let arg_list = if class.field_names.len() == 1 {
        format!("{},", names.into_iter().next().expect("length checked"))
    } else {
        names.collect::<Vec<_>>().join(", ")
    };
    format!("{}({})", class.name.as_str(interns), arg_list)
}

/// Constructs a [`NamedTuple`] instance by calling a [`NamedTupleClass`].
///
/// Binds positional and keyword arguments to the class's fields, applies
/// defaults for omitted trailing fields, and reports arity/keyword errors with
/// CPython's exact `<lambda>()` wording (its generated `__new__` is a lambda).
/// Called from the VM's `call_heap_callable` dispatch.
pub(crate) fn construct_namedtuple(class_id: HeapId, vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let HeapData::NamedTupleClass(c) = vm.heap.get(class_id) else {
        unreachable!("construct_namedtuple called on a non-namedtuple-class heap entry");
    };
    let name = c.name.clone();
    let field_names = c.field_names.clone();
    let defaults = c
        .defaults
        .iter()
        .map(|v| v.clone_with_heap(vm.heap))
        .collect::<Vec<_>>();
    let n = field_names.len();
    let n_required = n - defaults.len();
    // Construction errors are reported against the generated `__new__`, e.g.
    // `Point.__new__() missing 1 required positional argument: 'y'`.
    let func_name = format!("{}.__new__", name.as_str(vm.interns));

    let (pos, kwargs) = args.into_parts();
    let n_pos = pos.len();

    // Too many positionals. `__new__(cls, ...)` counts `cls`, so the limit and
    // the count both include it (the `+ 1`s).
    if n_pos > n {
        pos.drop_with(vm);
        kwargs.drop_with(vm);
        defaults.drop_with(vm);
        return Err(ExcType::type_error_too_many_positional_range(
            &func_name,
            n + 1,
            n + 1,
            n_pos + 1,
            0,
        ));
    }

    // Fill positional slots first, leaving the rest empty.
    let mut slots: Vec<Option<Value>> = pos.map(Some).collect();
    slots.resize_with(n, || None);

    // Bind keyword arguments. Errors are deferred to one cleanup point so every
    // owned value is dropped exactly once regardless of which kwarg is bad.
    let mut first_err: Option<RunError> = None;
    let mut leftover: Vec<Value> = Vec::new();
    for (key, val) in normalize_kwargs(kwargs, vm) {
        let key_str = key.as_str(vm.interns);
        match field_names.iter().position(|f| f.as_str(vm.interns) == key_str) {
            Some(i) if slots[i].is_none() => slots[i] = Some(val),
            Some(_) => {
                if first_err.is_none() {
                    first_err = Some(ExcType::type_error_duplicate_arg(&func_name, key_str));
                }
                leftover.push(val);
            }
            None => {
                if first_err.is_none() {
                    first_err = Some(ExcType::type_error_unexpected_keyword(&func_name, key_str));
                }
                leftover.push(val);
            }
        }
    }

    // Consume the class defaults into their trailing slots (only where still
    // empty and no earlier error occurred); drop any unused ones.
    for (slot, default) in slots[n_required..].iter_mut().zip(defaults) {
        if first_err.is_none() && slot.is_none() {
            *slot = Some(default);
        } else {
            default.drop_with(vm);
        }
    }

    // Any still-empty required slot is a missing positional argument.
    if first_err.is_none() {
        let missing: Vec<&str> = (0..n_required)
            .filter(|&i| slots[i].is_none())
            .map(|i| field_names[i].as_str(vm.interns))
            .collect();
        if !missing.is_empty() {
            first_err = Some(ExcType::type_error_missing_positional_with_names(&func_name, &missing));
        }
    }

    if let Some(err) = first_err {
        leftover.drop_with(vm);
        // `Vec<Option<Value>>` releases through the `Vec` and `Option` impls.
        slots.drop_with(vm);
        return Err(err);
    }

    let items: Vec<Value> = slots.into_iter().map(|s| s.expect("all slots bound")).collect();
    Ok(build_namedtuple(name, field_names, items, Some(class_id), vm))
}

/// `Point._make(iterable)` — builds an instance from an iterable of values.
fn make_namedtuple(class_id: HeapId, vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (name, field_names) = match vm.heap.get(class_id) {
        HeapData::NamedTupleClass(c) => (c.name.clone(), c.field_names.clone()),
        _ => unreachable!("make_namedtuple called on a non-namedtuple-class heap entry"),
    };
    make_from_iterable(name, field_names, Some(class_id), vm, args)
}

/// Shared `_make` body: collect the iterable, length-check it against the
/// fields, and build the instance.
fn make_from_iterable(
    name: EitherStr,
    field_names: Vec<EitherStr>,
    class_id: Option<HeapId>,
    vm: &mut VM<'_>,
    args: ArgValues,
) -> RunResult<Value> {
    let n = field_names.len();
    let iterable = args.get_one_arg("_make", vm.heap)?;
    let items = collect_owned_iterable::<Vec<Value>>(iterable, vm)?;
    if items.len() != n {
        let got = items.len();
        items.drop_with(vm);
        return Err(ExcType::type_error(format!("Expected {n} arguments, got {got}")));
    }
    Ok(build_namedtuple(name, field_names, items, class_id, vm))
}

/// Allocates a [`NamedTuple`] instance, taking ownership of `items` and, when
/// `class_id` is `Some`, an incremented reference to the class object.
pub(crate) fn build_namedtuple(
    name: EitherStr,
    field_names: Vec<EitherStr>,
    items: Vec<Value>,
    class_id: Option<HeapId>,
    vm: &mut VM<'_>,
) -> Value {
    let nt = NamedTuple::with_class(name, field_names, items, class_id);
    let id = vm.heap.allocate(HeapData::NamedTuple(Box::new(nt)));
    // The instance owns a reference to its class object (see `py_dec_ref_ids`).
    if let Some(cid) = class_id {
        vm.heap.inc_ref(cid);
    }
    Value::Ref(id)
}

/// Builds a tuple `Value` of the given field names (as string values).
fn fields_tuple_from(names: &[EitherStr], vm: &mut VM<'_>) -> Value {
    let items: Vec<Value> = names.iter().map(|name| field_name_value(name, vm)).collect();
    allocate_tuple(SmallVec::from_vec(items), vm.heap)
}

/// Converts an interned/heap field name into a string `Value`.
fn field_name_value(name: &EitherStr, vm: &mut VM<'_>) -> Value {
    match name {
        EitherStr::Interned(id) => Value::InternString(*id),
        EitherStr::Heap(s) => allocate_string(s.clone(), vm.heap),
    }
}

/// Normalizes call kwargs into `(name, value)` pairs, dropping non-interned
/// keys' heap references. Non-string keys (only reachable via `**` unpacking)
/// map to an empty name, which then surfaces as an unexpected-keyword error.
fn normalize_kwargs(kwargs: KwargsValues, vm: &mut VM<'_>) -> Vec<(EitherStr, Value)> {
    match kwargs {
        KwargsValues::Empty => Vec::new(),
        KwargsValues::Inline(kvs) => kvs.into_iter().map(|(id, v)| (EitherStr::Interned(id), v)).collect(),
        KwargsValues::Pairs(kvs) => kvs.into_iter().map(|(k, v)| (take_key(k, vm), v)).collect(),
        KwargsValues::Dict(dict) => dict.into_iter().map(|(k, v)| (take_key(k, vm), v)).collect(),
    }
}

/// Extracts a string key from a `Value`, releasing its heap reference.
fn take_key(key: Value, vm: &mut VM<'_>) -> EitherStr {
    let either = key.as_either_str(vm.heap).unwrap_or(EitherStr::Heap(String::new()));
    key.drop_with(vm);
    either
}

/// Renders a slice of names as a Python list repr, e.g. `['a', 'b']`.
fn py_list_repr(names: &[String]) -> String {
    let inner = names.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ");
    format!("[{inner}]")
}
