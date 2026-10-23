//! The builtin property, classmethod and staticmethod wrappers.

use std::{
    fmt::{self, Write},
    mem,
};

use super::{BoundMethod, LazyHeapSet, PyTrait, Type, descriptor, instance::bind_method, str::allocate_string};
use crate::{
    args::{ArgValues, FromArgs},
    builtins::type_of_ref,
    bytecode::{CallResult, RunReentryGuard, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    frozen::FrozenFunctions,
    hash::{HashValue, identity_hash},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    modules::ModuleFunctions,
    value::{EitherStr, Value},
};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct BuiltinDescriptor {
    pub kind: Type,
    // Property: fget, fset, fdel, doc. Other wrappers use only the first slot.
    pub values: [Value; 4],
    #[serde(default)]
    pub name: Option<String>,
}

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

#[derive(FromArgs)]
#[from_args(name = "classmethod")]
struct ClassMethodArgs {
    #[from_args(pos_only)]
    function: Value,
}

#[derive(FromArgs)]
#[from_args(name = "staticmethod")]
struct StaticMethodArgs {
    #[from_args(pos_only)]
    function: Value,
}

#[derive(FromArgs)]
#[from_args(name = "property accessor")]
struct AccessorArgs {
    #[from_args(pos_only)]
    receiver: Value,
    #[from_args(pos_only)]
    function: Value,
}

pub(crate) fn property_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PropertyArgs { fget, fset, fdel, doc } = PropertyArgs::from_args(args, vm)?;
    Ok(allocate(
        BuiltinDescriptor {
            kind: Type::Property,
            name: fget.as_ref().and_then(|f| function_name(f, vm)),
            values: [
                fget.unwrap_or(Value::None),
                fset.unwrap_or(Value::None),
                fdel.unwrap_or(Value::None),
                doc.unwrap_or(Value::None),
            ],
        },
        vm,
    ))
}

pub(crate) fn classmethod_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ClassMethodArgs { function } = ClassMethodArgs::from_args(args, vm)?;
    Ok(allocate(
        BuiltinDescriptor {
            kind: Type::ClassMethod,
            name: function_name(&function, vm),
            values: [function, Value::None, Value::None, Value::None],
        },
        vm,
    ))
}

pub(crate) fn staticmethod_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let StaticMethodArgs { function } = StaticMethodArgs::from_args(args, vm)?;
    Ok(allocate(
        BuiltinDescriptor {
            kind: Type::StaticMethod,
            name: function_name(&function, vm),
            values: [function, Value::None, Value::None, Value::None],
        },
        vm,
    ))
}

fn allocate(descriptor: BuiltinDescriptor, vm: &mut VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::BuiltinDescriptor(Box::new(descriptor))))
}

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

fn protocol_get(id: HeapId, instance: &Value, owner: Option<&Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if matches!(instance, Value::None) && owner.is_none_or(|v| matches!(v, Value::None)) {
        return Err(ExcType::type_error("__get__(None, None) is invalid"));
    }
    let HeapData::BuiltinDescriptor(descriptor) = vm.heap.get(id) else {
        unreachable!()
    };
    match descriptor.kind {
        Type::StaticMethod => Ok(CallResult::Value(descriptor.values[0].clone_with_heap(vm))),
        Type::ClassMethod => {
            let func = descriptor.values[0].clone_with_heap(vm.heap);
            let instance = match owner {
                Some(owner) if !matches!(owner, Value::None) => owner.clone_with_heap(vm.heap),
                _ => type_of_ref(vm, instance),
            };
            Ok(CallResult::Value(Value::Ref(
                vm.heap.allocate(HeapData::BoundMethod(BoundMethod { instance, func })),
            )))
        }
        Type::Property => {
            if matches!(instance, Value::None) {
                vm.heap.inc_ref(id);
                return Ok(CallResult::Value(Value::Ref(id)));
            }
            let function = descriptor.values[0].clone_with_heap(vm);
            defer_drop!(function, vm);
            if matches!(function, Value::None) {
                return Err(missing_accessor("getter"));
            }
            // Native getter chains can recurse without installing a Python frame.
            vm.enter_run_reentry()?;
            let mut reentry = RunReentryGuard::new(vm);
            let vm = &mut *reentry;
            vm.call_function(function, ArgValues::One(instance.clone_with_heap(vm.heap)))
        }
        _ => unreachable!("descriptor constructor selects its kind"),
    }
}

pub(crate) fn mutate(id: HeapId, object: &Value, value: Option<Value>, vm: &mut VM<'_>) -> RunResult<CallResult> {
    defer_drop!(value, vm);
    let HeapData::BuiltinDescriptor(descriptor) = vm.heap.get(id) else {
        unreachable!()
    };
    let index = if value.is_some() { 1 } else { 2 };
    let function = descriptor.values[index].clone_with_heap(vm.heap);
    if matches!(function, Value::None) {
        return Err(missing_accessor(if index == 1 { "setter" } else { "deleter" }));
    }
    let mut args = vec![object.clone_with_heap(vm.heap)];
    args.extend(value.as_ref().map(|v| v.clone_with_heap(vm.heap)));
    descriptor::discard_call(function, args, vm)
}

fn missing_accessor(accessor: &str) -> RunError {
    SimpleException::new_msg(ExcType::AttributeError, format!("property has no {accessor}")).into()
}

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
    pub(crate) fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        if matches!(self, Self::Get | Self::Set | Self::Delete | Self::SetName) {
            return self.protocol_call(vm, args);
        }
        let AccessorArgs { receiver, function } = AccessorArgs::from_args(args, vm)?;
        let mut guard = DropGuard::new((receiver, function), vm);
        let ((receiver, _), vm) = guard.as_parts_mut();
        let mut values = if let Value::Ref(id) = receiver
            && let HeapData::BuiltinDescriptor(descriptor) = vm.heap.get(*id)
            && descriptor.kind == Type::Property
        {
            descriptor.values.each_ref().map(|v| v.clone_with_heap(vm))
        } else {
            return Err(ExcType::type_error("property accessor requires a property"));
        };
        let name = if let Value::Ref(id) = receiver {
            let HeapData::BuiltinDescriptor(d) = vm.heap.get(*id) else {
                unreachable!()
            };
            d.name.clone()
        } else {
            unreachable!()
        };
        let ((receiver, function), vm) = guard.into_parts();
        receiver.drop_with(vm);
        let index = match self {
            Self::Getter => 0,
            Self::Setter => 1,
            Self::Deleter => 2,
            _ => unreachable!(),
        };
        mem::replace(&mut values[index], function).drop_with(vm);
        Ok(CallResult::Value(Value::Ref(vm.heap.allocate(
            HeapData::BuiltinDescriptor(Box::new(BuiltinDescriptor {
                kind: Type::Property,
                values,
                name,
            })),
        ))))
    }
}

impl DescriptorMethod {
    fn protocol_call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        let args = args.into_pos_only("descriptor", vm.heap)?;
        defer_drop!(args, vm);
        let Some(Value::Ref(id)) = args.as_slice().first() else {
            return Err(ExcType::type_error("descriptor receiver required"));
        };
        match (self, args.as_slice()) {
            (Self::Get, [_, instance, owner]) => protocol_get(*id, instance, Some(owner), vm),
            (Self::Get, [_, instance]) => protocol_get(*id, instance, None, vm),
            (Self::Set, [_, object, value]) => mutate(*id, object, Some(value.clone_with_heap(vm.heap)), vm),
            (Self::Delete, [_, object]) => mutate(*id, object, None, vm),
            (Self::SetName, [_, _, name]) => {
                set_name(*id, name, vm)?;
                Ok(CallResult::Value(Value::None))
            }
            _ => Err(ExcType::type_error("invalid descriptor arguments")),
        }
    }
}

impl HeapItem for BuiltinDescriptor {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for value in &mut self.values {
            value.py_dec_ref_ids(stack);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, BuiltinDescriptor> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).kind
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
        Ok(write!(f, "<{} object>", self.get(vm.heap).kind)?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let descriptor = self.get(vm.heap);
        let name = attr.as_str(vm.interns);
        if name == "__name__" {
            return Ok(descriptor
                .name
                .as_ref()
                .map(|name| CallResult::Value(allocate_string(name.clone(), vm.heap))));
        }
        let value = if descriptor.kind == Type::Property {
            let index = match name {
                "fget" => Some(0),
                "fset" => Some(1),
                "fdel" => Some(2),
                "__doc__" => Some(3),
                _ => None,
            };
            if let Some(index) = index {
                descriptor.values[index].clone_with_heap(vm)
            } else {
                let method = match name {
                    "getter" => DescriptorMethod::Getter,
                    "setter" => DescriptorMethod::Setter,
                    "deleter" => DescriptorMethod::Deleter,
                    "__get__" => DescriptorMethod::Get,
                    "__set__" => DescriptorMethod::Set,
                    "__delete__" => DescriptorMethod::Delete,
                    "__set_name__" => DescriptorMethod::SetName,
                    _ => return Ok(None),
                };
                bind_method(
                    &Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::Descriptor(method))),
                    self.id(),
                    vm,
                )
            }
        } else if name == "__get__" {
            bind_method(
                &Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::Descriptor(
                    DescriptorMethod::Get,
                ))),
                self.id(),
                vm,
            )
        } else if matches!(name, "__func__" | "__wrapped__") {
            descriptor.values[0].clone_with_heap(vm)
        } else {
            return Ok(None);
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        // Attribute resolution can fail before the argument bundle is consumed.
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
        let descriptor = self.get(vm.heap);
        if descriptor.kind != Type::StaticMethod {
            args.drop_with(vm);
            return Err(ExcType::type_error_not_callable_object(&self.py_type_name(vm)));
        }
        let function = descriptor.values[0].clone_with_heap(vm);
        let mut guard = DropGuard::new((function, args), vm);
        let (_, vm) = guard.as_parts_mut();
        vm.enter_run_reentry()?;
        let ((function, args), vm) = guard.into_parts();
        let mut reentry = RunReentryGuard::new(vm);
        let vm = &mut *reentry;
        defer_drop!(function, vm);
        vm.call_function(function, args)
    }
}

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

pub(crate) fn set_name(id: HeapId, name: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    let Some(name) = name.as_either_str(vm.heap) else {
        return Err(ExcType::type_error("property name must be a string"));
    };
    let name = name.as_str(vm.interns).to_owned();
    let HeapReadOutput::BuiltinDescriptor(d) = vm.heap.read(id) else {
        unreachable!()
    };
    d.get_mut(vm.heap).name = Some(name);
    Ok(())
}
