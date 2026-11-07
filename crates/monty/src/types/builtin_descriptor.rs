//! The builtin property, classmethod and staticmethod wrappers.

use std::fmt::{self, Write};

use super::{LazyHeapSet, PyTrait, Type, instance::bind_method};
use crate::{
    args::{ArgValues, FromArgs},
    builtins::type_of_ref,
    bytecode::{CallResult, RunReentryGuard, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::FrozenFunctions,
    hash::{HashValue, identity_hash},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    modules::ModuleFunctions,
    value::{EitherStr, Value},
};

/// Builtin descriptor state; each wrapper stores only the values its protocol needs.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum BuiltinDescriptor {
    Property {
        fget: Value,
        fset: Value,
        fdel: Value,
        doc: Value,
        /// __set_name__ accepts any Python value, including a non-string name.
        name: Option<Value>,
    },
    ClassMethod(Value),
    StaticMethod(Value),
}

impl BuiltinDescriptor {
    /// Include every owned edge, including names installed by __set_name__, in cycle collection.
    pub(crate) fn visit_children(&self, on_child: &mut impl FnMut(HeapId)) {
        let mut visit = |value: &Value| {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        };
        match self {
            Self::Property {
                fget,
                fset,
                fdel,
                doc,
                name,
            } => {
                for value in [fget, fset, fdel, doc] {
                    visit(value);
                }
                if let Some(name) = name {
                    visit(name);
                }
            }
            Self::ClassMethod(function) | Self::StaticMethod(function) => visit(function),
        }
    }

    /// Python type of the wrapper, independent of its wrapped callable.
    pub(crate) fn kind(&self) -> Type {
        match self {
            Self::Property { .. } => Type::Property,
            Self::ClassMethod(_) => Type::ClassMethod,
            Self::StaticMethod(_) => Type::StaticMethod,
        }
    }
}

/// Optional property accessors; None leaves the corresponding operation unavailable.
#[derive(FromArgs)]
#[from_args(name = "property")]
struct PropertyArgs {
    #[from_args(default)]
    fget: Option<Value>,
    #[from_args(default)]
    fset: Option<Value>,
    #[from_args(default)]
    fdel: Option<Value>,
    #[from_args(default)]
    doc: Option<Value>,
}

/// The callable to bind to the eventual owner class.
#[derive(FromArgs)]
#[from_args(name = "classmethod")]
struct ClassMethodArgs {
    #[from_args(pos_only)]
    function: Value,
}

/// The callable returned unchanged by staticmethod.__get__.
#[derive(FromArgs)]
#[from_args(name = "staticmethod")]
struct StaticMethodArgs {
    #[from_args(pos_only)]
    function: Value,
}

/// Receiver and replacement callable after the accessor-specific arity check.
#[derive(FromArgs)]
#[from_args(name = "property accessor")]
struct AccessorArgs {
    #[from_args(pos_only)]
    receiver: Value,
    #[from_args(pos_only)]
    function: Value,
}

/// Construct a property and initialize its name from the getter, when available.
pub(crate) fn property_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PropertyArgs { fget, fset, fdel, doc } = PropertyArgs::from_args(args, vm)?;
    let name = fget.as_ref().and_then(|f| function_name(f, vm));
    let name = name.map(|name| super::str::allocate_string(name, vm.heap));
    Ok(allocate(
        BuiltinDescriptor::Property {
            fget: fget.unwrap_or(Value::None),
            fset: fset.unwrap_or(Value::None),
            fdel: fdel.unwrap_or(Value::None),
            doc: doc.unwrap_or(Value::None),
            name,
        },
        vm,
    ))
}

/// Construct a descriptor that binds its callable to the owner class.
pub(crate) fn classmethod_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ClassMethodArgs { function } = ClassMethodArgs::from_args(args, vm)?;
    Ok(allocate(BuiltinDescriptor::ClassMethod(function), vm))
}

/// Construct a descriptor that returns its callable without binding.
pub(crate) fn staticmethod_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let StaticMethodArgs { function } = StaticMethodArgs::from_args(args, vm)?;
    Ok(allocate(BuiltinDescriptor::StaticMethod(function), vm))
}

/// Transfer the wrapper's owned values into the heap.
fn allocate(descriptor: BuiltinDescriptor, vm: &mut VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::BuiltinDescriptor(Box::new(descriptor))))
}

/// Bind an implicit attribute read using its instance and owning sandbox class.
pub(crate) fn get(id: HeapId, instance: Option<HeapId>, owner: HeapId, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let instance = instance.map_or(Value::None, |id| {
        vm.heap.inc_ref(id);
        Value::Ref(id)
    });
    defer_drop!(instance, vm);
    vm.heap.inc_ref(owner);
    let owner = Value::Ref(owner);
    defer_drop!(owner, vm);
    protocol_get(id, instance, Some(owner), vm)
}

/// Bind explicit __get__ calls too, including builtin receivers such as property(len).__get__([]).
fn protocol_get(id: HeapId, instance: &Value, owner: Option<&Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if matches!(instance, Value::None) && owner.is_none_or(|v| matches!(v, Value::None)) {
        return Err(ExcType::type_error("__get__(None, None) is invalid"));
    }
    let HeapData::BuiltinDescriptor(descriptor) = vm.heap.get(id) else {
        return Err(ExcType::type_error("invalid descriptor receiver"));
    };
    match &**descriptor {
        BuiltinDescriptor::StaticMethod(function) => Ok(CallResult::Value(function.clone_with_heap(vm))),
        BuiltinDescriptor::ClassMethod(function) => {
            let func = function.clone_with_heap(vm.heap);
            let instance = match owner {
                Some(owner) if !matches!(owner, Value::None) => owner.clone_with_heap(vm.heap),
                _ => type_of_ref(vm, instance),
            };
            Ok(CallResult::Value(Value::Ref(vm.heap.allocate(HeapData::BoundMethod(
                super::BoundMethod { instance, func },
            )))))
        }
        BuiltinDescriptor::Property { .. } if matches!(instance, Value::None) => {
            vm.heap.inc_ref(id);
            Ok(CallResult::Value(Value::Ref(id)))
        }
        BuiltinDescriptor::Property { fget, .. } => {
            let function = fget.clone_with_heap(vm);
            defer_drop!(function, vm);
            if matches!(function, Value::None) {
                Err(missing_accessor(id, instance, "getter", vm)?)
            } else {
                // Native getter chains can recurse without installing a Python frame.
                vm.enter_run_reentry()?;
                let mut reentry = RunReentryGuard::new(vm);
                let vm = &mut *reentry;
                vm.call_function(function, ArgValues::One(instance.clone_with_heap(vm.heap)))
            }
        }
    }
}

/// Run a property's setter or deleter, discarding its return value even across suspension.
pub(crate) fn mutate(id: HeapId, object: &Value, value: Option<Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    defer_drop!(value, vm);
    let HeapData::BuiltinDescriptor(descriptor) = vm.heap.get(id) else {
        return Err(ExcType::type_error("invalid descriptor receiver"));
    };
    let BuiltinDescriptor::Property { fset, fdel, .. } = &**descriptor else {
        return Err(ExcType::type_error("descriptor does not support mutation"));
    };
    let (function, accessor) = if value.is_some() {
        (fset, "setter")
    } else {
        (fdel, "deleter")
    };
    let function = function.clone_with_heap(vm.heap);
    defer_drop!(function, vm);
    if matches!(function, Value::None) {
        Err(missing_accessor(id, object, accessor, vm)?)
    } else {
        let mut args = vec![object.clone_with_heap(vm.heap)];
        args.extend(value.as_ref().map(|v| v.clone_with_heap(vm.heap)));
        super::descriptor::discard_call(function.clone_with_heap(vm.heap), args, vm)
    }
}

/// Include the property's current name and the receiver's class in missing-accessor errors.
fn missing_accessor(id: HeapId, object: &Value, accessor: &str, vm: &mut VM<'_>) -> RunResult<RunError> {
    let name = match vm.heap.get(id) {
        HeapData::BuiltinDescriptor(descriptor) => match &**descriptor {
            BuiltinDescriptor::Property { name, .. } => name.as_ref().map(|name| name.clone_with_heap(vm.heap)),
            _ => None,
        },
        _ => None,
    };
    defer_drop!(name, vm);
    let name = name
        .as_ref()
        .map(|name| {
            let repr = name.py_repr(vm)?;
            defer_drop!(repr, vm);
            Ok::<_, RunError>(repr.as_str(vm).expect("repr is a string").to_owned())
        })
        .transpose()?;
    Ok(ExcType::attribute_error_property(
        name.as_deref(),
        &object.py_type_name(vm),
        accessor,
    ))
}

/// Builtin property accessors and descriptor protocol methods exposed as bound callables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum DescriptorMethod {
    Getter,
    Setter,
    Deleter,
    Get,
    Set,
    Delete,
    SetName,
}

impl fmt::Display for DescriptorMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Getter => "property.getter",
            Self::Setter => "property.setter",
            Self::Deleter => "property.deleter",
            Self::Get => "descriptor.__get__",
            Self::Set => "descriptor.__set__",
            Self::Delete => "descriptor.__delete__",
            Self::SetName => "property.__set_name__",
        })
    }
}

impl DescriptorMethod {
    /// Parse the bound receiver and Python-visible arguments before dispatching a wrapper method.
    pub(crate) fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        if matches!(self, Self::Get | Self::Set | Self::Delete | Self::SetName) {
            self.protocol_call(vm, args)
        } else {
            let method = self.to_string();
            let args = args.reject_kwargs(&method, vm.heap)?;
            let given = args.count().saturating_sub(1);
            let guard = DropGuard::new(args, vm);
            if given != 1 {
                return Err(ExcType::type_error_arg_count(&method, 1, given));
            }
            let (args, vm) = guard.into_parts();
            let AccessorArgs { receiver, function } = AccessorArgs::from_args(args, vm)?;
            defer_drop!(receiver, vm);
            defer_drop!(function, vm);
            if let Value::Ref(id) = receiver
                && let HeapData::BuiltinDescriptor(descriptor) = vm.heap.get(*id)
                && let BuiltinDescriptor::Property {
                    fget,
                    fset,
                    fdel,
                    doc,
                    name,
                } = &**descriptor
            {
                let property = BuiltinDescriptor::Property {
                    fget: if self == Self::Getter { function } else { fget }.clone_with_heap(vm.heap),
                    fset: if self == Self::Setter { function } else { fset }.clone_with_heap(vm.heap),
                    fdel: if self == Self::Deleter { function } else { fdel }.clone_with_heap(vm.heap),
                    doc: doc.clone_with_heap(vm.heap),
                    name: name.as_ref().map(|name| name.clone_with_heap(vm.heap)),
                };
                Ok(CallResult::Value(allocate(property, vm)))
            } else {
                Err(ExcType::type_error("invalid property receiver"))
            }
        }
    }

    /// Check the overloaded slot-wrapper signatures, counting arguments after the bound receiver.
    fn protocol_call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        let keyword_name = if self == Self::Get {
            "wrapper __get__"
        } else {
            "descriptor"
        };
        let args = args.into_pos_only(keyword_name, vm.heap)?;
        defer_drop!(args, vm);
        let Some(Value::Ref(id)) = args.as_slice().first() else {
            return Err(ExcType::type_error("invalid descriptor receiver"));
        };
        if !matches!(vm.heap.get(*id), HeapData::BuiltinDescriptor(_)) {
            return Err(ExcType::type_error("invalid descriptor receiver"));
        }
        let given = args.len() - 1;
        match (self, args.as_slice()) {
            (Self::Get, [_, instance, owner]) => protocol_get(*id, instance, Some(owner), vm),
            (Self::Get, [_, instance]) => protocol_get(*id, instance, None, vm),
            (Self::Set, [_, object, value]) => mutate(*id, object, Some(value.clone_with_heap(vm.heap)), vm),
            (Self::Delete, [_, object]) => mutate(*id, object, None, vm),
            (Self::SetName, [_, _, name]) => {
                set_name(*id, name, vm)?;
                Ok(CallResult::Value(Value::None))
            }
            _ => Err(match self {
                Self::Get if given < 1 => ExcType::type_error_at_least("__get__", 1, given),
                Self::Get => ExcType::type_error_at_most("__get__", 2, given),
                Self::Set => ExcType::type_error_arg_count("__set__", 2, given),
                Self::Delete => ExcType::type_error(format!("expected 1 argument, got {given}")),
                Self::SetName => ExcType::type_error_too_many_positional_range("__set_name__", 2, 2, given, 0),
                _ => ExcType::type_error("invalid descriptor method"),
            }),
        }
    }
}

impl HeapItem for BuiltinDescriptor {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        match self {
            Self::Property {
                fget,
                fset,
                fdel,
                doc,
                name,
            } => {
                for value in [fget, fset, fdel, doc] {
                    value.py_dec_ref_ids(stack);
                }
                if let Some(name) = name {
                    name.py_dec_ref_ids(stack);
                }
            }
            Self::ClassMethod(function) | Self::StaticMethod(function) => function.py_dec_ref_ids(stack),
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, BuiltinDescriptor> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).kind()
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

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<{} object>", self.get(vm.heap).kind())?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let descriptor = self.get(vm.heap);
        let name = attr.as_str(vm.interns);
        let method = match descriptor {
            BuiltinDescriptor::Property {
                fget,
                fset,
                fdel,
                doc,
                name: property_name,
            } => {
                let value = match name {
                    "fget" => Some(fget),
                    "fset" => Some(fset),
                    "fdel" => Some(fdel),
                    "__doc__" => Some(doc),
                    "__name__" => property_name.as_ref(),
                    _ => None,
                };
                if let Some(value) = value {
                    return Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap))));
                }
                match name {
                    "getter" => DescriptorMethod::Getter,
                    "setter" => DescriptorMethod::Setter,
                    "deleter" => DescriptorMethod::Deleter,
                    "__get__" => DescriptorMethod::Get,
                    "__set__" => DescriptorMethod::Set,
                    "__delete__" => DescriptorMethod::Delete,
                    "__set_name__" => DescriptorMethod::SetName,
                    _ => return Ok(None),
                }
            }
            BuiltinDescriptor::ClassMethod(function) | BuiltinDescriptor::StaticMethod(function) => match name {
                "__get__" => DescriptorMethod::Get,
                "__func__" | "__wrapped__" => return Ok(Some(CallResult::Value(function.clone_with_heap(vm.heap)))),
                "__name__" => {
                    return Ok(function_name(function, vm)
                        .map(|name| CallResult::Value(super::str::allocate_string(name, vm.heap))));
                }
                _ => return Ok(None),
            },
        };
        Ok(Some(CallResult::Value(bind_method(
            &Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::Descriptor(method))),
            self.id(),
            vm,
        ))))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let mut guard = DropGuard::new(args, vm);
        let (_, vm) = guard.as_parts_mut();
        let Some(CallResult::Value(callable)) = self.py_getattr(attr, vm)? else {
            return Err(ExcType::attribute_error(self.py_type(vm), attr.as_str(vm.interns)));
        };
        let (args, vm) = guard.into_parts();
        defer_drop!(callable, vm);
        vm.call_function(callable, args)
    }

    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        if let BuiltinDescriptor::StaticMethod(function) = self.get(vm.heap) {
            let function = function.clone_with_heap(vm);
            let mut guard = DropGuard::new((function, args), vm);
            let (_, vm) = guard.as_parts_mut();
            vm.enter_run_reentry()?;
            let ((function, args), vm) = guard.into_parts();
            let mut reentry = RunReentryGuard::new(vm);
            let vm = &mut *reentry;
            defer_drop!(function, vm);
            vm.call_function(function, args)
        } else {
            args.drop_with(vm);
            Err(ExcType::type_error_not_callable_object(&self.py_type_name(vm)))
        }
    }
}

/// Names available without executing an arbitrary callable's attribute hooks.
fn function_name(function: &Value, vm: &VM<'_>) -> Option<String> {
    let id = match function {
        Value::DefFunction(id) => *id,
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Closure(f) => f.func_id,
            HeapData::FunctionDefaults(f) => f.func_id,
            _ => return None,
        },
        _ => return None,
    };
    Some(vm.interns.get_str(vm.interns.get_function(id).name.name_id).to_owned())
}

/// Store the name passed to property.__set_name__; CPython permits non-string values too.
pub(crate) fn set_name(id: HeapId, name: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let HeapReadOutput::BuiltinDescriptor(descriptor) = vm.heap.read(id) else {
        return Err(ExcType::type_error("invalid property receiver"));
    };
    let name = name.clone_with_heap(vm.heap);
    let old = if let BuiltinDescriptor::Property { name: current, .. } = descriptor.get_mut(vm.heap) {
        current.replace(name)
    } else {
        name.drop_with(vm);
        return Err(ExcType::type_error("invalid property receiver"));
    };
    old.drop_with(vm);
    Ok(())
}
