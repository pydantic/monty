//! `property`, `staticmethod` and `classmethod` — the builtin descriptors a class
//! body uses to change how a member binds.
//!
//! These objects only hold callables. Binding happens in the attribute lookups of
//! `Instance` and `Class`, through [`bind_class_member`], [`call_class_member`] and
//! the property accessors below. A property getter read as `obj.attr` runs as a pushed
//! frame and so may suspend; setters, and getters reached through a call such as
//! `obj.attr()`, run synchronously (see `limitations/classes.md`).

use std::{fmt::Write, mem::replace};

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    types::{BoundMethod, LazyHeapSet, PyTrait, Type, instance::class_name, py_trait::PyObjectIdentity},
    value::{EitherStr, Value},
};

/// A builtin descriptor object stored as a class member.
///
/// Every `Value` here is an OWNED ref, so `py_dec_ref_ids` and
/// `for_each_child_id` must enumerate all of them.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Descriptor {
    /// `property(fget, fset, fdel, doc)`.
    Property(PropertyFuncs),
    /// `staticmethod(func)`: yields `func` unbound through instances and the class.
    StaticMethod(Value),
    /// `classmethod(func)`: binds the class as the first argument.
    ClassMethod(Value),
    /// `prop.setter` (or `getter`/`deleter`) read as a value, as `@prop.setter`
    /// does. Not a descriptor: calling it copies `property` with `slot` replaced.
    PropertyMethod { property: Value, slot: PropertySlot },
}

/// Which accessor a `property.getter/setter/deleter` call replaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum PropertySlot {
    Getter,
    Setter,
    Deleter,
}

impl PropertySlot {
    /// The slot a `property` method name refers to.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "getter" => Some(Self::Getter),
            "setter" => Some(Self::Setter),
            "deleter" => Some(Self::Deleter),
            _ => None,
        }
    }

    /// The method name, as in `property.setter`.
    fn name(self) -> &'static str {
        match self {
            Self::Getter => "getter",
            Self::Setter => "setter",
            Self::Deleter => "deleter",
        }
    }
}

/// The accessors of a `property`; an absent one is `Value::None`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PropertyFuncs {
    fget: Value,
    fset: Value,
    fdel: Value,
    /// The explicit `doc` argument. `None` falls back to `fget.__doc__` on read.
    doc: Value,
}

/// Argument shape for `property(fget=None, fset=None, fdel=None, doc=None)`.
#[derive(FromArgs)]
#[from_args(name = "property", at_most_total)]
struct PropertyArgs {
    #[from_args(default = Value::None)]
    fget: Value,
    #[from_args(default = Value::None)]
    fset: Value,
    #[from_args(default = Value::None)]
    fdel: Value,
    #[from_args(default = Value::None)]
    doc: Value,
}

/// Argument shape for `staticmethod(func)`, which CPython parses with `PyArg_UnpackTuple`.
#[derive(FromArgs)]
#[from_args(name = "staticmethod", style = unpack)]
struct StaticMethodArgs {
    #[from_args(pos_only)]
    func: Value,
}

/// Argument shape for `classmethod(func)`, parsed like `staticmethod`.
#[derive(FromArgs)]
#[from_args(name = "classmethod", style = unpack)]
struct ClassMethodArgs {
    #[from_args(pos_only)]
    func: Value,
}

impl Descriptor {
    /// `property(fget=None, fset=None, fdel=None, doc=None)`.
    pub(crate) fn property_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let PropertyArgs { fget, fset, fdel, doc } = PropertyArgs::from_args(args, vm)?;
        Ok(allocate(Self::Property(PropertyFuncs { fget, fset, fdel, doc }), vm))
    }

    /// `staticmethod(func)`. Like CPython, `func` is not checked for being callable.
    pub(crate) fn staticmethod_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let StaticMethodArgs { func } = StaticMethodArgs::from_args(args, vm)?;
        Ok(allocate(Self::StaticMethod(func), vm))
    }

    /// `classmethod(func)`. Like CPython, `func` is not checked for being callable.
    pub(crate) fn classmethod_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let ClassMethodArgs { func } = ClassMethodArgs::from_args(args, vm)?;
        Ok(allocate(Self::ClassMethod(func), vm))
    }

    /// The Python type of this descriptor object.
    #[must_use]
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Property(_) => Type::Property,
            Self::StaticMethod(_) => Type::StaticMethod,
            Self::ClassMethod(_) => Type::ClassMethod,
            Self::PropertyMethod { .. } => Type::BuiltinFunction,
        }
    }

    /// Whether this is a `property`, the only data descriptor among the three: it
    /// takes precedence over a same-named instance attribute.
    #[must_use]
    pub(crate) fn is_property(&self) -> bool {
        matches!(self, Self::Property(_))
    }

    /// Whether calling this object directly succeeds; `classmethod` and
    /// `property` objects are not callable.
    #[must_use]
    pub(crate) fn is_callable(&self) -> bool {
        matches!(self, Self::StaticMethod(_) | Self::PropertyMethod { .. })
    }

    /// Invokes `on_child` for each heap id this descriptor owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in self.owned_values() {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Every owned `Value`.
    fn owned_values(&self) -> impl Iterator<Item = &Value> {
        let values: [Option<&Value>; 4] = match self {
            Self::Property(p) => [Some(&p.fget), Some(&p.fset), Some(&p.fdel), Some(&p.doc)],
            Self::StaticMethod(func) | Self::ClassMethod(func) | Self::PropertyMethod { property: func, .. } => {
                [Some(func), None, None, None]
            }
        };
        values.into_iter().flatten()
    }
}

/// Allocates `descriptor` on the heap.
fn allocate(descriptor: Descriptor, vm: &VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::Descriptor(Box::new(descriptor))))
}

/// Borrows the descriptor `value` refers to, if it is one.
pub(crate) fn as_descriptor<'v>(value: &Value, vm: &'v VM<'_>) -> Option<&'v Descriptor> {
    match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Descriptor(descriptor) => Some(descriptor),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `value` is a `property` object.
pub(crate) fn is_property_value(value: &Value, vm: &VM<'_>) -> bool {
    as_descriptor(value, vm).is_some_and(Descriptor::is_property)
}

/// What `obj.name` (`instance` is `Some`) or `Cls.name` (`None`) evaluates to for
/// the class member `member`, without running any Python code.
///
/// `staticmethod` unwraps to its function, `classmethod` binds the class, and a
/// plain function binds the instance. A `property` is returned as-is: running its
/// getter is the caller's job (see [`property_get`]), since only it can run code.
pub(crate) fn bind_class_member(member: &Value, class_id: HeapId, instance: Option<HeapId>, vm: &VM<'_>) -> Value {
    let (receiver, func) = match as_descriptor(member, vm) {
        Some(Descriptor::StaticMethod(func)) => return func.clone_with_heap(vm.heap),
        Some(Descriptor::ClassMethod(func)) => (class_id, func),
        _ => match instance {
            Some(self_id) if is_method_value(member, vm) => (self_id, member),
            _ => return member.clone_with_heap(vm.heap),
        },
    };
    let func = func.clone_with_heap(vm.heap);
    vm.heap.inc_ref(receiver);
    let bound = BoundMethod {
        instance: Value::Ref(receiver),
        func,
    };
    Value::Ref(vm.heap.allocate(HeapData::BoundMethod(bound)))
}

/// Calls the class member `member`, found through `instance` (`Some`) or the
/// class itself (`None`), with CPython's descriptor-binding semantics.
///
/// The bound form of [`bind_class_member`] without allocating a `BoundMethod`.
/// A `property` reached through an instance has its getter run synchronously and
/// the result called; through the class, the property itself is called and fails.
pub(crate) fn call_class_member(
    member: &Value,
    attr: &str,
    class_id: HeapId,
    instance: Option<HeapId>,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    match (MemberKind::of(member, vm), instance) {
        (MemberKind::StaticMethod(func), _) => {
            defer_drop!(func, vm);
            vm.call_function(func, args)
        }
        (MemberKind::ClassMethod(func), _) => {
            defer_drop!(func, vm);
            vm.heap.inc_ref(class_id);
            vm.call_function(func, args.prepend(Value::Ref(class_id)))
        }
        (MemberKind::Property, Some(self_id)) => {
            let value = match property_get_sync(member, self_id, attr, vm) {
                Ok(value) => value,
                Err(err) => {
                    args.drop_with(vm);
                    return Err(err);
                }
            };
            defer_drop!(value, vm);
            vm.call_function(value, args)
        }
        (MemberKind::Other, Some(self_id)) if is_method_value(member, vm) => {
            vm.heap.inc_ref(self_id);
            vm.call_function(member, args.prepend(Value::Ref(self_id)))
        }
        _ => vm.call_function(member, args),
    }
}

/// A class member classified by how it binds, holding an owned clone of the
/// wrapped function so the heap borrow ends before the call.
enum MemberKind {
    StaticMethod(Value),
    ClassMethod(Value),
    Property,
    Other,
}

impl MemberKind {
    fn of(member: &Value, vm: &VM<'_>) -> Self {
        match as_descriptor(member, vm) {
            Some(Descriptor::StaticMethod(func)) => Self::StaticMethod(func.clone_with_heap(vm.heap)),
            Some(Descriptor::ClassMethod(func)) => Self::ClassMethod(func.clone_with_heap(vm.heap)),
            Some(Descriptor::Property(_)) => Self::Property,
            Some(Descriptor::PropertyMethod { .. }) | None => Self::Other,
        }
    }
}

/// Runs the getter of the property `member` for `obj.attr` on the instance `self_id`.
///
/// Returns the getter's `CallResult`, so a Python getter runs as a pushed frame and
/// can suspend on external calls like any method.
pub(crate) fn property_get(member: &Value, self_id: HeapId, attr: &str, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let fget = property_accessor(member, vm, |p| &p.fget);
    defer_drop!(fget, vm);
    if matches!(fget, Value::None) {
        Err(no_getter(self_id, attr, vm))
    } else {
        vm.heap.inc_ref(self_id);
        vm.call_function(fget, ArgValues::One(Value::Ref(self_id)))
    }
}

/// Runs the property getter to completion, for positions that need the value
/// itself (`obj.attr(...)`, `getattr()`) rather than a `CallResult`.
pub(crate) fn property_get_sync(member: &Value, self_id: HeapId, attr: &str, vm: &mut VM<'_>) -> RunResult<Value> {
    let fget = property_accessor(member, vm, |p| &p.fget);
    defer_drop!(fget, vm);
    if matches!(fget, Value::None) {
        Err(no_getter(self_id, attr, vm))
    } else {
        vm.heap.inc_ref(self_id);
        vm.evaluate_function(fget, ArgValues::One(Value::Ref(self_id)))
    }
}

/// Runs the setter of the property `member` for `obj.attr = value`.
///
/// Synchronous because attribute stores cannot push a frame, so a setter that
/// calls an external function raises `NotImplementedError`.
pub(crate) fn property_set(
    member: &Value,
    self_id: HeapId,
    attr: &str,
    value: Value,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let fset = property_accessor(member, vm, |p| &p.fset);
    defer_drop!(fset, vm);
    if matches!(fset, Value::None) {
        value.drop_with(vm);
        let class_id = instance_class_id(self_id, vm);
        Err(ExcType::attribute_error_property_no_setter(
            attr,
            &class_name(class_id, vm.heap, vm.interns),
        ))
    } else {
        vm.heap.inc_ref(self_id);
        let result = vm.evaluate_function(fset, ArgValues::Two(Value::Ref(self_id), value))?;
        result.drop_with(vm);
        Ok(())
    }
}

/// Clones one accessor out of the property `member`.
fn property_accessor(member: &Value, vm: &VM<'_>, pick: impl FnOnce(&PropertyFuncs) -> &Value) -> Value {
    match as_descriptor(member, vm) {
        Some(Descriptor::Property(p)) => pick(p).clone_with_heap(vm.heap),
        _ => unreachable!("property accessor read from a non-property member"),
    }
}

/// CPython's error for reading a property without a getter.
fn no_getter(self_id: HeapId, attr: &str, vm: &VM<'_>) -> RunError {
    let class_id = instance_class_id(self_id, vm);
    ExcType::attribute_error_property_no_getter(attr, &class_name(class_id, vm.heap, vm.interns))
}

/// The class of the instance `self_id`.
fn instance_class_id(self_id: HeapId, vm: &VM<'_>) -> HeapId {
    match vm.heap.get(self_id) {
        HeapData::Instance(instance) => instance.class(),
        _ => unreachable!("property accessed through a non-instance"),
    }
}

/// Whether a class member binds the instance when read through it: a
/// user-defined function or one of the function-like wrappers listed below.
pub(crate) fn is_method_value(value: &Value, vm: &VM<'_>) -> bool {
    match value {
        Value::DefFunction(_) => true,
        Value::Ref(id) => matches!(
            vm.heap.get(*id),
            // `functools.partial` and the `lru_cache` wrapper are descriptors in
            // CPython 3.14 too, and bind the instance the same way: after the
            // arguments they already carry. A `@dataclass`-generated method is a
            // function in CPython.
            HeapData::Closure(_)
                | HeapData::FunctionDefaults(_)
                | HeapData::Partial(_)
                | HeapData::LruCache(_)
                | HeapData::DataclassMethod(_)
        ),
        _ => false,
    }
}

/// Releases the refs a descriptor owns, for one abandoned before it reaches the heap.
impl<C: ContainsHeap> DropWithContext<C> for Descriptor {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Property(p) => [p.fget, p.fset, p.fdel, p.doc].drop_with(ctx),
            Self::StaticMethod(func) | Self::ClassMethod(func) | Self::PropertyMethod { property: func, .. } => {
                func.drop_with(ctx);
            }
        }
    }
}

impl HeapItem for Descriptor {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Property(p) => {
                p.fget.py_dec_ref_ids(stack);
                p.fset.py_dec_ref_ids(stack);
                p.fdel.py_dec_ref_ids(stack);
                p.doc.py_dec_ref_ids(stack);
            }
            Self::StaticMethod(func) | Self::ClassMethod(func) | Self::PropertyMethod { property: func, .. } => {
                func.py_dec_ref_ids(stack);
            }
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Descriptor> {
    /// Calls a `staticmethod`'s function, or applies a bound `property.setter`.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        match self.get(vm.heap) {
            Descriptor::StaticMethod(func) => {
                let func = func.clone_with_heap(vm.heap);
                defer_drop!(func, vm);
                vm.call_function(func, args)
            }
            Descriptor::PropertyMethod { property, slot } => {
                let (property, slot) = (property.clone_with_heap(vm.heap), *slot);
                defer_drop!(property, vm);
                copy_property_with(property, slot, args, vm).map(CallResult::Value)
            }
            Descriptor::Property(_) | Descriptor::ClassMethod(_) => {
                args.drop_with(vm);
                Err(ExcType::type_error_not_callable_object(&self.py_type_name(vm)))
            }
        }
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).py_type()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// `<property object at 0x…>`, `<staticmethod(<function f at 0x…>)>`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let (prefix, func) = match self.get(vm.heap) {
            Descriptor::Property(_) => (None, Value::None),
            Descriptor::StaticMethod(func) => (Some("staticmethod"), func.clone_with_heap(vm.heap)),
            Descriptor::ClassMethod(func) => (Some("classmethod"), func.clone_with_heap(vm.heap)),
            Descriptor::PropertyMethod { property, slot } => {
                return Ok(write!(
                    f,
                    "<built-in method {} of property object at 0x{:x}>",
                    slot.name(),
                    property.py_identity().encoded()
                )?);
            }
        };
        let Some(prefix) = prefix else {
            return self.py_default_repr_fmt(f, vm);
        };
        defer_drop!(func, vm);
        // The wrapped object may hold this descriptor, so recurse under a guard.
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;
        write!(f, "<{prefix}(")?;
        func.py_repr_fmt(f, vm, heap_ids)?;
        Ok(f.write_str(")>")?)
    }

    /// `fget`/`fset`/`fdel`/`__doc__` on a property, `__func__`/`__wrapped__` on the others.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr = attr.as_str(vm.interns);
        let value = match (self.get(vm.heap), attr) {
            (Descriptor::Property(p), "fget") => p.fget.clone_with_heap(vm.heap),
            (Descriptor::Property(p), "fset") => p.fset.clone_with_heap(vm.heap),
            (Descriptor::Property(p), "fdel") => p.fdel.clone_with_heap(vm.heap),
            (Descriptor::Property(p), "__doc__") => {
                let (doc, fget) = (p.doc.clone_with_heap(vm.heap), p.fget.clone_with_heap(vm.heap));
                return property_doc(doc, fget, vm).map(Some);
            }
            (Descriptor::StaticMethod(func) | Descriptor::ClassMethod(func), "__func__" | "__wrapped__") => {
                func.clone_with_heap(vm.heap)
            }
            (Descriptor::Property(_), name) if let Some(slot) = PropertySlot::from_name(name) => {
                vm.heap.inc_ref(self.id());
                let property = Value::Ref(self.id());
                allocate(Descriptor::PropertyMethod { property, slot }, vm)
            }
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    /// `property.getter/setter/deleter(func)` build a copy with one accessor
    /// replaced; any other readable attribute is called as a value.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        if self.get(vm.heap).is_property()
            && let Some(slot) = PropertySlot::from_name(attr_str)
        {
            vm.heap.inc_ref(self.id());
            let property = Value::Ref(self.id());
            defer_drop!(property, vm);
            return copy_property_with(property, slot, args, vm).map(CallResult::Value);
        }
        let type_name = self.py_type_name(vm);
        match self.py_getattr(attr, vm) {
            Ok(Some(CallResult::Value(value))) => {
                defer_drop!(value, vm);
                vm.call_function(value, args)
            }
            Ok(Some(other)) => {
                other.drop_with(vm);
                Err(ExcType::attribute_error_method(type_name, attr, args, vm))
            }
            Ok(None) => Err(ExcType::attribute_error_method(type_name, attr, args, vm)),
            Err(err) => {
                args.drop_with(vm);
                Err(err)
            }
        }
    }
}

/// `property.getter/setter/deleter(func)`: a copy of `property` with the accessor
/// in `slot` replaced by the single argument.
fn copy_property_with(property: &Value, slot: PropertySlot, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let method = match slot {
        PropertySlot::Getter => "property.getter",
        PropertySlot::Setter => "property.setter",
        PropertySlot::Deleter => "property.deleter",
    };
    let func = args.get_one_arg(method, vm.heap)?;
    let Some(Descriptor::Property(p)) = as_descriptor(property, vm) else {
        unreachable!("property methods are only bound to properties")
    };
    let mut copy = PropertyFuncs {
        fget: p.fget.clone_with_heap(vm.heap),
        fset: p.fset.clone_with_heap(vm.heap),
        fdel: p.fdel.clone_with_heap(vm.heap),
        doc: p.doc.clone_with_heap(vm.heap),
    };
    let target = match slot {
        PropertySlot::Getter => &mut copy.fget,
        PropertySlot::Setter => &mut copy.fset,
        PropertySlot::Deleter => &mut copy.fdel,
    };
    replace(target, func).drop_with(vm);
    Ok(allocate(Descriptor::Property(copy), vm))
}

/// `property.__doc__`: the explicit `doc`, else the getter's docstring (CPython
/// copies it at construction; reading it lazily gives the same answer).
fn property_doc(doc: Value, fget: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if !matches!(doc, Value::None) || matches!(fget, Value::None) {
        fget.drop_with(vm);
        return Ok(CallResult::Value(doc));
    }
    defer_drop!(fget, vm);
    let name = EitherStr::Interned(vm.interns.intern_static(StaticStrings::DunderDoc));
    match fget.py_getattr(&name, vm) {
        Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => Ok(CallResult::Value(Value::None)),
        result => result,
    }
}
