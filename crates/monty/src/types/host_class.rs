use std::{
    fmt::Write,
    hash::{DefaultHasher, Hash, Hasher},
};

use serde::ser::SerializeStruct;

use super::{Dict, LazyHeapSet, PyTrait, attribute_name_value, str::allocate_string};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    hash::HashValue,
    heap::{
        BorrowedHeapRead, BorrowedHeapReadMut, DropGuard, DropWithContext, HeapId, HeapItem, HeapObjectRead, HeapRead,
        HeapReadOutput, heap_read_ref_as_field, heap_read_ref_as_field_mut,
    },
    intern::Interns,
    types::Type,
    value::{EitherStr, Value},
};

/// A host-backed class instance (the heap form of the wire `ClassInstance`).
///
/// Represents an instance of a host-side class: a class name, host identities
/// (`instance_id` = `id(obj)`, `type_id` = `id(type(obj))`), and the eagerly
/// sent attributes. Names missing from `attrs` route back to the host:
/// - calling a public missing attribute yields [`CallResult::MethodCall`]
///   (routed by `instance_id`, the receiver is not passed as an argument);
/// - reading a public missing attribute yields [`CallResult::AttrLookup`]
///   (a lazy attribute lookup; an unanswered lookup raises `AttributeError`).
///
/// Underscore-prefixed names never suspend (dunder probes must stay local).
/// Lazy lookups are NOT cached: every access is a fresh round trip to the
/// host, so host-side mutations stay visible.
///
/// When `frozen` is true the instance rejects `setattr` with
/// `FrozenInstanceError` and is hashable (over its eager attrs); otherwise it
/// is mutable and unhashable, matching frozen-dataclass semantics.
#[derive(Debug)]
pub(crate) struct HostClass {
    /// The class name (e.g., "Point", "User")
    name: EitherStr,
    /// Identity of the instance, from `id(obj)` on the host; 0 when the
    /// instance was defined inside the sandbox (not host-backed).
    instance_id: u64,
    /// Identity of the class, from `id(type(obj))` on the host; 0 for
    /// sandbox-defined instances.
    type_id: u64,
    /// Eagerly-sent attributes, in order (both fields and dynamically added)
    attrs: Dict,
    /// Whether this instance is immutable (affects hashability)
    frozen: bool,
    /// Whether `dataclasses.is_dataclass(obj)` is true on the host side.
    is_dataclass: bool,
}

impl HostClass {
    /// Creates a new host class instance; ownership of `attrs` transfers.
    #[must_use]
    pub fn new(
        name: impl Into<EitherStr>,
        instance_id: u64,
        type_id: u64,
        attrs: Dict,
        frozen: bool,
        is_dataclass: bool,
    ) -> Self {
        Self {
            name: name.into(),
            instance_id,
            type_id,
            attrs,
            frozen,
            is_dataclass,
        }
    }

    /// Returns the class name.
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }

    /// The class name as stored — for callers that need to branch on
    /// interned-vs-heap without an `Interns` in hand.
    #[must_use]
    pub fn name_either(&self) -> &EitherStr {
        &self.name
    }

    /// Returns the host identity of the instance (0 = sandbox-defined).
    #[must_use]
    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    /// Returns the host identity of the class (0 = sandbox-defined).
    #[must_use]
    pub fn type_id(&self) -> u64 {
        self.type_id
    }

    /// Returns a reference to the attrs Dict.
    #[must_use]
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }

    /// Returns whether this instance is frozen (immutable).
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Returns whether the host object is a dataclass instance.
    #[must_use]
    pub fn is_dataclass(&self) -> bool {
        self.is_dataclass
    }
}

impl<'h> HeapRead<'h, HostClass> {
    /// Sets an attribute value.
    ///
    /// The caller transfers ownership of both `name` and `value`. Returns the
    /// old value if the attribute existed (caller must drop it), or None if this
    /// is a new attribute.
    ///
    /// Returns `FrozenInstanceError` if the instance is frozen.
    pub fn set_attr(&mut self, name: Value, value: Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if self.get(vm.heap).frozen {
            defer_drop!(name, vm);
            value.drop_with(vm);
            let name_repr = name.py_repr(vm)?;
            defer_drop!(name_repr, vm);
            let exc = SimpleException::new_msg(
                ExcType::FrozenInstanceError,
                format!("cannot assign to field {}", name_repr.to_str(vm)?),
            );
            return Err(exc.into());
        }
        self.attrs_mut().set(name, value, vm)
    }

    pub fn attrs(&self) -> BorrowedHeapRead<'_, 'h, Dict> {
        heap_read_ref_as_field!(self, HostClass, attrs)
    }

    pub fn attrs_mut(&mut self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, HostClass, attrs)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, HostClass> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::HostClass
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        // Host class instances don't have a length
        None
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        let mut value_guard = DropGuard::new(value, vm);
        let name = attribute_name_value(name, value_guard.ctx());
        let (value, vm) = value_guard.into_parts();
        let old_value = self.set_attr(name, value, vm)?;
        old_value.drop_with(vm);
        Ok(())
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::HostClass(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        // Equal only for the same class and equal attrs. The name gate matters
        // for sandbox-origin instances, which all share type_id 0.
        if self.get(vm.heap).type_id() != other.get(vm.heap).type_id()
            || self.get(vm.heap).name(vm.interns) != other.get(vm.heap).name(vm.interns)
        {
            return Ok(Some(false));
        }
        Ok(Some(self.attrs().eq_dict(&other.attrs(), vm)?))
    }

    /// Hashes a frozen instance by its class name and eager attrs in order.
    ///
    /// Mutable (non-frozen) instances return `None` (unhashable).
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        // Only frozen (immutable) instances are hashable
        if !self.get(vm.heap).frozen {
            return Ok(None);
        }
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        let mut hasher = DefaultHasher::new();
        // Hash the class name
        self.get(vm.heap).name.as_str(vm.interns).hash(&mut hasher);
        // Hash each (key, value) attr pair in order
        let attr_count = self.get(vm.heap).attrs.len();
        for i in 0..attr_count {
            let Some((key, value)) = self.get(vm.heap).attrs.item_at(i) else {
                break;
            };
            let key = key.clone_with_heap(vm.heap);
            let value = value.clone_with_heap(vm.heap);
            defer_drop!(key, vm);
            defer_drop!(value, vm);
            match key.py_hash(vm)? {
                Some(h) => h.hash(&mut hasher),
                None => return Ok(None),
            }
            match value.py_hash(vm)? {
                Some(h) => h.hash(&mut hasher),
                None => return Ok(None),
            }
        }
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        // Host class instances are always truthy (like Python objects)
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        // All eager attrs are shown, in order.
        let name = self.get(vm.heap).name(vm.interns).to_owned();
        let attr_count = self.get(vm.heap).attrs.len();
        write_dataclass_repr(f, &name, attr_count, vm, heap_ids, |i, vm| {
            let (key, value) = match self.get(vm.heap).attrs.item_at(i) {
                Some((key, value)) => (key.clone_with_heap(vm.heap), Some(value.clone_with_heap(vm.heap))),
                None => return Ok((String::new(), None)),
            };
            defer_drop!(key, vm);
            // Keys are strings in practice; render a non-string key (possible
            // in host-built attrs) via repr rather than fail.
            let key_str = if let Ok(s) = key.to_str(vm) {
                s.to_owned()
            } else {
                let repr = key.py_repr(vm)?;
                defer_drop!(repr, vm);
                repr.to_str(vm)?.to_owned()
            };
            Ok((key_str, value))
        })
    }

    /// Performs lazy method detection for host class instances.
    ///
    /// If the attribute is a public name (no leading underscore) not found in
    /// the eager attrs, returns `MethodCall` (routed by `instance_id`) so the
    /// VM yields to the host. Otherwise handles the call directly:
    /// - Attributes that exist in attrs but aren't callable produce `TypeError`
    /// - Private/dunder attributes that aren't in attrs produce `AttributeError`
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        // Only public methods (no underscore prefix = no dunders, no private)
        if !attr_str.starts_with('_')
            && self
                .get(vm.heap)
                .attrs
                .get_by_str(attr_str, vm.heap, vm.interns)
                .is_none()
        {
            // The receiver is not passed along — the host resolves it by id.
            let instance_id = self.get(vm.heap).instance_id();
            Ok(CallResult::MethodCall {
                name: attr.clone(),
                args,
                instance_id,
            })
        } else {
            // Not a method call — handle directly
            let method_name = attr.as_str(vm.interns);
            defer_drop!(args, vm);

            // If the attribute exists in attrs, it's a data value (not callable)
            if let Some(value) = self.get(vm.heap).attrs.get_by_str(method_name, vm.heap, vm.interns) {
                let type_name = value.py_type_name(vm);
                Err(ExcType::type_error_not_callable_object(&type_name))
            } else {
                // Attribute doesn't exist — use the class name (e.g., "Point") not "HostClass"
                Err(ExcType::attribute_error(
                    self.get(vm.heap).name(vm.interns),
                    method_name,
                ))
            }
        }
    }

    /// Resolves `obj.attr`: eager attrs first, then a lazy host lookup.
    ///
    /// A public name missing from attrs suspends as [`CallResult::AttrLookup`]
    /// so the host can serve it; underscore-prefixed names raise
    /// `AttributeError` locally (dunder probes must never suspend).
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        match self.get(vm.heap).attrs.get_by_str(attr_name, vm.heap, vm.interns) {
            Some(value) => Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap)))),
            None if !attr_name.starts_with('_') => Ok(Some(CallResult::AttrLookup {
                name: attr.clone(),
                class_name: self.get(vm.heap).name(vm.interns).to_owned(),
                instance_id: self.get(vm.heap).instance_id(),
            })),
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            None => Err(ExcType::attribute_error(self.get(vm.heap).name(vm.interns), attr_name)),
        }
    }
}

/// Writes `ClassName(f1=v1, ...)`, shared by the host-supplied [`HostClass`] and
/// native `@dataclass` instances so the two renderings cannot drift.
///
/// Each caller supplies its own field list via `field`, mapping an index to that
/// field's name and a cloned value (dropped here). A cycle renders `...`, a
/// `None` value `<?>`, and exhausting `max_duration` truncates `...[timeout]`.
///
/// `field` is resolved immediately before that field is written, never all up
/// front, so a `__repr__` that mutates a later field is observed — matching the
/// left-to-right evaluation of CPython's generated f-string.
pub(crate) fn write_dataclass_repr<'h>(
    f: &mut impl Write,
    name: &str,
    field_count: usize,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
    field: impl Fn(usize, &mut VM<'h>) -> RunResult<(String, Option<Value>)>,
) -> RunResult<()> {
    let Ok(mut guard) = vm.recursion_guard() else {
        return Ok(f.write_str("...")?);
    };
    let vm = &mut *guard;
    f.write_str(name)?;
    f.write_char('(')?;
    for i in 0..field_count {
        if i > 0 {
            // Same between-item checkpoint as sequence repr, so a wide instance
            // cannot outrun `max_duration`.
            if vm.heap.tracker.check_memory_time_every(i).is_err() {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        // Guarded before anything is written, so a formatter error on the name
        // cannot strand the value the callback just cloned.
        let (field_name, value) = field(i, &mut *vm)?;
        defer_drop!(value, vm);
        f.write_str(&field_name)?;
        f.write_char('=')?;
        match value {
            Some(value) => value.py_repr_fmt(f, vm, heap_ids)?,
            None => f.write_str("<?>")?,
        }
    }
    Ok(f.write_char(')')?)
}

impl HeapItem for HostClass {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Delegate to the attrs Dict which handles all nested heap references
        self.attrs.py_dec_ref_ids(stack);
    }
}

/// The type object `type(x)` returns for a [`HostClass`] instance.
///
/// Host classes have no real class object in the sandbox, so `type(x)`
/// materializes this lightweight stand-in naming the real class (repr
/// `<class 'Point'>`, equality by class identity). Each `type(x)` call
/// allocates a fresh one, so `type(a) is type(b)` is `False` even for the
/// same class (use `==`) — see `limitations/classes.md`. Not callable and
/// not usable with `isinstance`, since the class itself lives on the host.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct HostClassType {
    /// The class name (e.g. "Point").
    name: EitherStr,
    /// Host identity of the class, matching [`HostClass::type_id`].
    type_id: u64,
}

impl HostClassType {
    /// Creates the type object for a host class.
    #[must_use]
    pub fn new(name: EitherStr, type_id: u64) -> Self {
        Self { name, type_id }
    }

    /// Returns the class name.
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, HostClassType> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Type
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::HostClassType(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        // Same class identity: type_id plus the name gate for sandbox-origin
        // instances, which all share type_id 0 (mirrors HostClass::py_eq_impl).
        Ok(Some(
            self.get(vm.heap).type_id == other.get(vm.heap).type_id
                && self.get(vm.heap).name(vm.interns) == other.get(vm.heap).name(vm.interns),
        ))
    }

    /// Hashes by class identity, consistent with `py_eq_impl` — so equal type
    /// objects collide in dicts/sets even though each `type(x)` call allocates
    /// a fresh one.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).name(vm.interns).hash(&mut hasher);
        self.get(vm.heap).type_id.hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<class '{}'>", self.get(vm.heap).name(vm.interns))?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        if attr_name == "__name__" {
            let name = self.get(vm.heap).name(vm.interns).to_owned();
            Ok(Some(CallResult::Value(allocate_string(name, vm.heap))))
        } else {
            // CPython wording for missing attrs on a type object.
            Err(ExcType::attribute_error_type(
                self.get(vm.heap).name(vm.interns),
                attr_name,
            ))
        }
    }
}

impl HeapItem for HostClassType {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Name and type_id hold no heap references.
    }
}

// Custom serde implementation for HostClass; serializes all six fields so
// suspended state (dumps) round-trips exactly.
impl serde::Serialize for HostClass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HostClass", 6)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("instance_id", &self.instance_id)?;
        state.serialize_field("type_id", &self.type_id)?;
        state.serialize_field("attrs", &self.attrs)?;
        state.serialize_field("frozen", &self.frozen)?;
        state.serialize_field("is_dataclass", &self.is_dataclass)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for HostClass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct HostClassData {
            name: EitherStr,
            instance_id: u64,
            type_id: u64,
            attrs: Dict,
            frozen: bool,
            is_dataclass: bool,
        }
        let hc = HostClassData::deserialize(deserializer)?;
        Ok(Self {
            name: hc.name,
            instance_id: hc.instance_id,
            type_id: hc.type_id,
            attrs: hc.attrs,
            frozen: hc.frozen,
            is_dataclass: hc.is_dataclass,
        })
    }
}
