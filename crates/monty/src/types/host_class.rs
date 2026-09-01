use std::{
    fmt::Write,
    hash::{DefaultHasher, Hash, Hasher},
};

use monty_types::{ClassType, DictPairs, MontyType, MontyUuid};
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
/// Represents an instance of a host-side class: a class name, boundary
/// identities (uuids minted by whichever side defined the object), and the
/// eagerly sent attributes. Names missing from `attrs` route back to the host:
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
    /// Identity of the instance, minted by whichever side defined it.
    instance_id: MontyUuid,
    /// Identity of the class, minted by whichever side defined it.
    type_id: MontyUuid,
    /// Whether the class is host-defined (routing target) rather than a
    /// round-tripped sandbox class.
    host_defined: bool,
    /// Direct base classes, carried for the wire `Type`; inheritance is not
    /// functional in the sandbox.
    parents: Vec<MontyType>,
    /// Eagerly-sent attributes, in order (both fields and dynamically added)
    attrs: Dict,
    /// Whether this instance is immutable (affects hashability)
    frozen: bool,
    /// Whether `dataclasses.is_dataclass(obj)` is true on the host side.
    is_dataclass: bool,
}

impl HostClass {
    /// Creates a new host class instance from its wire class type and
    /// instance id; ownership of `attrs` transfers.
    #[must_use]
    pub fn new(class_type: ClassType, instance_id: MontyUuid, attrs: Dict) -> Self {
        Self {
            name: class_type.name.into(),
            instance_id,
            type_id: class_type.id,
            host_defined: class_type.host_defined,
            parents: class_type.parents,
            attrs,
            frozen: class_type.frozen,
            is_dataclass: class_type.is_dataclass,
        }
    }

    /// Rebuilds the wire [`ClassType`] this instance's class crossed in as.
    /// The `type` branch of an instance never carries eager class attrs, so
    /// `attrs` is always empty here.
    #[must_use]
    pub fn class_type(&self, interns: &Interns) -> ClassType {
        ClassType {
            name: self.name.as_str(interns).to_owned(),
            id: self.type_id,
            host_defined: self.host_defined,
            parents: self.parents.clone(),
            is_dataclass: self.is_dataclass,
            frozen: self.frozen,
            attrs: DictPairs::default(),
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

    /// Returns the identity of the instance.
    #[must_use]
    pub fn instance_id(&self) -> MontyUuid {
        self.instance_id
    }

    /// Returns the identity of the class.
    #[must_use]
    pub fn type_id(&self) -> MontyUuid {
        self.type_id
    }

    /// Returns a reference to the attrs Dict.
    #[must_use]
    pub fn attrs(&self) -> &Dict {
        &self.attrs
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
        // Equal only for the same class (uuids are collision-free, so no name
        // gate is needed) and equal attrs.
        if self.get(vm.heap).type_id() != other.get(vm.heap).type_id() {
            return Ok(Some(false));
        }
        Ok(Some(self.attrs().eq_dict(&other.attrs(), vm)?))
    }

    /// Hashes a frozen instance by its class name and eager attrs.
    ///
    /// Per-pair hashes are folded with a wrapping sum so the result is
    /// independent of attr insertion order — `py_eq_impl` compares via the
    /// order-insensitive `eq_dict`, and equal values must hash equal even
    /// when two wrappers sent the same attrs in different orders.
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
        // Fold each (key, value) attr pair order-independently
        let mut attrs_hash = 0u64;
        let attr_count = self.get(vm.heap).attrs.len();
        for i in 0..attr_count {
            let Some((key, value)) = self.get(vm.heap).attrs.item_at(i) else {
                break;
            };
            let key = key.clone_with_heap(vm.heap);
            let value = value.clone_with_heap(vm.heap);
            defer_drop!(key, vm);
            defer_drop!(value, vm);
            let mut pair_hasher = DefaultHasher::new();
            match key.py_hash(vm)? {
                Some(h) => h.hash(&mut pair_hasher),
                None => return Ok(None),
            }
            match value.py_hash(vm)? {
                Some(h) => h.hash(&mut pair_hasher),
                None => return Ok(None),
            }
            attrs_hash = attrs_hash.wrapping_add(pair_hasher.finish());
        }
        attrs_hash.hash(&mut hasher);
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
            let Some((key, _)) = self.get(vm.heap).attrs.item_at(i) else {
                return Ok((String::new(), None));
            };
            let key = key.clone_with_heap(vm.heap);
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
            // The value is cloned only after the fallible key formatting, so a
            // formatting error cannot strand an unguarded clone; re-reading at
            // index `i` also observes any mutation a key `__repr__` performed,
            // matching `write_dataclass_repr`'s resolve-just-before-write rule.
            let value = self
                .get(vm.heap)
                .attrs
                .item_at(i)
                .map(|(_, v)| v.clone_with_heap(vm.heap));
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
            let object_id = self.get(vm.heap).instance_id();
            Ok(CallResult::MethodCall {
                name: attr.clone(),
                args,
                object_id,
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
                object_id: self.get(vm.heap).instance_id(),
                type_object: false,
            })),
            // underscore-prefixed: raise locally with the host class's real
            // name (not the static `HostClass` py_type placeholder)
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

/// The type object for a [`HostClass`] — returned by `type(x)` and produced
/// when a host passes a bare class in (a `Type` input with a host origin).
///
/// The real class lives on the host, so this is a stand-in naming it (repr
/// `<class 'Point'>`, equality by class identity) plus its eagerly sent
/// class attrs. Public names missing from `attrs` route back to the host
/// like [`HostClass`] attrs do (lazy class attrs, classmethod calls); each
/// `type(x)` call allocates a fresh one, so `type(a) is type(b)` is `False`
/// even for the same class (use `==`) — see `limitations/classes.md`. Not
/// usable with `isinstance`; calling it suspends a `__call__` method call to
/// the host, whose own policy decides whether construction is allowed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct HostClassType {
    /// The class name (e.g. "Point").
    name: EitherStr,
    /// Identity of the class, matching [`HostClass::type_id`].
    type_id: MontyUuid,
    /// Whether the class is host-defined (a host routing target).
    host_defined: bool,
    /// Direct base classes, carried for the wire `Type`.
    parents: Vec<MontyType>,
    /// Whether `dataclasses.is_dataclass` is true for the class.
    is_dataclass: bool,
    /// Whether instances reject `setattr` with `FrozenInstanceError`.
    frozen: bool,
    /// Eagerly-sent class attributes (class constants). Excluded from
    /// equality/hash, which go by class identity alone.
    attrs: Dict,
}

impl HostClassType {
    /// Creates the type object for a host class from its wire class type and
    /// the already-converted eager class attrs (empty for `type(x)` results).
    #[must_use]
    pub fn new(name: EitherStr, class_type: ClassType, attrs: Dict) -> Self {
        Self {
            name,
            type_id: class_type.id,
            host_defined: class_type.host_defined,
            parents: class_type.parents,
            is_dataclass: class_type.is_dataclass,
            frozen: class_type.frozen,
            attrs,
        }
    }

    /// Returns the class name.
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }

    /// Whether the class is host-defined — the only kind an instantiation
    /// request can ever succeed for.
    #[must_use]
    pub fn host_defined(&self) -> bool {
        self.host_defined
    }

    /// Identity of the class.
    #[must_use]
    pub fn type_id(&self) -> MontyUuid {
        self.type_id
    }

    /// Rebuilds the wire [`ClassType`] this type object crosses out as —
    /// minus `attrs`, which hold heap `Value`s: the object bridge converts
    /// and appends them when the type crosses out as a value.
    #[must_use]
    pub fn class_type(&self, interns: &Interns) -> ClassType {
        ClassType {
            name: self.name.as_str(interns).to_owned(),
            id: self.type_id,
            host_defined: self.host_defined,
            parents: self.parents.clone(),
            is_dataclass: self.is_dataclass,
            frozen: self.frozen,
            attrs: DictPairs::default(),
        }
    }
}

impl HostClassType {
    /// Returns a reference to the eager class attrs Dict.
    #[must_use]
    pub fn attrs(&self) -> &Dict {
        &self.attrs
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
        // Same class identity — uuids are collision-free, so no name gate
        // (mirrors HostClass::py_eq_impl).
        Ok(Some(self.get(vm.heap).type_id == other.get(vm.heap).type_id))
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

    /// Resolves `Type.attr`: `__name__`, then eager class attrs, then a lazy
    /// host lookup for public names on a host-defined class.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        if attr_name == "__name__" {
            let name = self.get(vm.heap).name(vm.interns).to_owned();
            return Ok(Some(CallResult::Value(allocate_string(name, vm.heap))));
        }
        match self.get(vm.heap).attrs.get_by_str(attr_name, vm.heap, vm.interns) {
            Some(value) => Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap)))),
            // A sandbox-origin class has no host wrapper to consult, so a
            // suspension could only ever miss — fail locally instead.
            None if !attr_name.starts_with('_') && self.get(vm.heap).host_defined => Ok(Some(CallResult::AttrLookup {
                name: attr.clone(),
                class_name: self.get(vm.heap).name(vm.interns).to_owned(),
                object_id: self.get(vm.heap).type_id(),
                type_object: true,
            })),
            // CPython wording for missing attrs on a type object.
            None => Err(ExcType::attribute_error_type(
                self.get(vm.heap).name(vm.interns),
                attr_name,
            )),
        }
    }

    /// Mirrors [`HostClass`]'s lazy method detection for classmethods: a
    /// public name missing from the eager class attrs suspends to the host
    /// (routed by the class uuid) when the class is host-defined.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        if !attr_str.starts_with('_')
            && self.get(vm.heap).host_defined
            && self
                .get(vm.heap)
                .attrs
                .get_by_str(attr_str, vm.heap, vm.interns)
                .is_none()
        {
            let object_id = self.get(vm.heap).type_id();
            Ok(CallResult::MethodCall {
                name: attr.clone(),
                args,
                object_id,
            })
        } else {
            defer_drop!(args, vm);
            if let Some(value) = self.get(vm.heap).attrs.get_by_str(attr_str, vm.heap, vm.interns) {
                let type_name = value.py_type_name(vm);
                Err(ExcType::type_error_not_callable_object(&type_name))
            } else {
                Err(ExcType::attribute_error_type(
                    self.get(vm.heap).name(vm.interns),
                    attr_str,
                ))
            }
        }
    }
}

impl HeapItem for HostClassType {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // The eager class attrs are the only heap references.
        self.attrs.py_dec_ref_ids(stack);
    }
}

// Custom serde implementation for HostClass; serializes all eight fields so
// suspended state (dumps) round-trips exactly.
impl serde::Serialize for HostClass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HostClass", 8)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("instance_id", &self.instance_id)?;
        state.serialize_field("type_id", &self.type_id)?;
        state.serialize_field("host_defined", &self.host_defined)?;
        state.serialize_field("parents", &self.parents)?;
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
            instance_id: MontyUuid,
            type_id: MontyUuid,
            host_defined: bool,
            parents: Vec<MontyType>,
            attrs: Dict,
            frozen: bool,
            is_dataclass: bool,
        }
        let hc = HostClassData::deserialize(deserializer)?;
        Ok(Self {
            name: hc.name,
            instance_id: hc.instance_id,
            type_id: hc.type_id,
            host_defined: hc.host_defined,
            parents: hc.parents,
            attrs: hc.attrs,
            frozen: hc.frozen,
            is_dataclass: hc.is_dataclass,
        })
    }
}
