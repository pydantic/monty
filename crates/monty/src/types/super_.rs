//! Implementation of the super() builtin and the Super proxy type.

use std::{borrow::Cow, fmt::Write};

use super::{
    BuiltinType, LazyHeapSet, PyTrait, Type,
    class::ClassBase,
    instance::{BoundMethod, class_name, is_method_value},
};
use crate::{
    args::ArgValues,
    builtins::{
        Builtins, BuiltinsFunctions,
        isinstance::isinstance_check,
        issubclass::issubclass_check,
        object_init::{builtin_exception_init, builtin_object_init},
        object_init_subclass::builtin_object_init_subclass,
        object_new::{builtin_exception_new, builtin_object_new},
        object_setattr::builtin_object_setattr,
        type_of_ref,
    },
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    namespace::NamespaceId,
    value::{EitherStr, Value},
};

/// A `super` proxy object created by `super(...)`.
///
/// Implements cooperative multiple inheritance method resolution order (MRO)
/// dispatch. Holds the class to search after (`type_arg`) and the target
/// instance or subclass (`obj_arg`).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Super {
    pub(crate) type_arg: Value,
    pub(crate) obj_arg: Option<Value>,
}

impl Super {
    #[must_use]
    pub fn new(type_arg: Value, obj_arg: Option<Value>) -> Self {
        Self { type_arg, obj_arg }
    }
}

/// Implementation of the `super(...)` builtin function.
pub fn builtin_super(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (pos_args, kwargs) = args.into_parts();
    if !kwargs.is_empty() {
        pos_args.drop_with(vm.heap);
        kwargs.drop_with(vm.heap);
        return Err(ExcType::type_error("super() takes no keyword arguments"));
    }
    match pos_args.len() {
        0 => zero_arg_super(vm),
        1 => {
            let mut pos = pos_args;
            let type_arg = pos.next().expect("length checked");
            if let Err(e) = validate_super_type(&type_arg, vm) {
                type_arg.drop_with(vm.heap);
                return Err(e);
            }
            let s = Super::new(type_arg, None);
            Ok(Value::Ref(vm.heap.allocate(HeapData::Super(s))))
        }
        2 => {
            let mut pos = pos_args;
            let type_arg = pos.next().expect("length checked");
            let obj_arg = pos.next().expect("length checked");
            if let Err(e) = validate_super_type(&type_arg, vm) {
                type_arg.drop_with(vm.heap);
                obj_arg.drop_with(vm.heap);
                return Err(e);
            }
            if matches!(obj_arg, Value::None) {
                let s = Super::new(type_arg, None);
                return Ok(Value::Ref(vm.heap.allocate(HeapData::Super(s))));
            }
            if let Err(e) = validate_super_obj(&type_arg, &obj_arg, vm) {
                type_arg.drop_with(vm.heap);
                obj_arg.drop_with(vm.heap);
                return Err(e);
            }
            let s = Super::new(type_arg, Some(obj_arg));
            Ok(Value::Ref(vm.heap.allocate(HeapData::Super(s))))
        }
        n => {
            pos_args.drop_with(vm.heap);
            Err(ExcType::type_error(format!(
                "super() expected at most 2 arguments, got {n}"
            )))
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Super> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Super
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let s = self.get(vm.heap);
        let type_arg = s.type_arg.clone_with_heap(vm.heap);
        let obj_arg = s.obj_arg.as_ref().map(|o| o.clone_with_heap(vm.heap));
        defer_drop!(type_arg, vm);
        if let Some(obj) = obj_arg {
            defer_drop!(obj, vm);
            let self_cls = resolve_super_self_class(type_arg, obj, vm)?;
            defer_drop!(self_cls, vm);
            let type_name = type_or_class_name(type_arg, vm);
            let obj_type_name = type_or_class_name(self_cls, vm);
            write!(f, "<super: <class '{type_name}'>, <{obj_type_name} object>>")?;
        } else {
            let type_name = type_or_class_name(type_arg, vm);
            write!(f, "<super: <class '{type_name}'>, NULL>")?;
        }
        Ok(())
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        let s = self.get(vm.heap);

        if attr_str == "__class__" {
            return Ok(Some(CallResult::Value(Value::Builtin(Builtins::Type(
                BuiltinType::Super,
            )))));
        }
        if attr_str == "__thisclass__" {
            let val = s.type_arg.clone_with_heap(vm.heap);
            return Ok(Some(CallResult::Value(val)));
        }
        if attr_str == "__self__" {
            let val = match &s.obj_arg {
                Some(obj) => obj.clone_with_heap(vm.heap),
                None => Value::None,
            };
            return Ok(Some(CallResult::Value(val)));
        }
        if attr_str == "__self_class__" {
            let Some(obj) = s.obj_arg.as_ref().map(|o| o.clone_with_heap(vm.heap)) else {
                return Ok(Some(CallResult::Value(Value::None)));
            };
            let type_arg = s.type_arg.clone_with_heap(vm.heap);
            defer_drop!(obj, vm);
            defer_drop!(type_arg, vm);
            let val = resolve_super_self_class(type_arg, obj, vm)?;
            return Ok(Some(CallResult::Value(val)));
        }

        let Some(obj) = s.obj_arg.as_ref().map(|o| o.clone_with_heap(vm.heap)) else {
            return Err(ExcType::attribute_error("super", attr_str));
        };
        defer_drop!(obj, vm);

        let type_arg = self.get(vm.heap).type_arg.clone_with_heap(vm.heap);
        defer_drop!(type_arg, vm);

        if let Some(val) = super_mro_lookup(type_arg, obj, attr_str, vm) {
            return Ok(Some(CallResult::Value(val)));
        }

        if attr_str == "__new__" {
            let func = if let Some(search_bases) = get_search_bases(type_arg, obj, vm) {
                let mut found = None;
                for base in search_bases {
                    match base {
                        ClassBase::Class(_) => {}
                        ClassBase::BuiltinExc(e) => {
                            found = Some(BoundMethod::allocate(
                                Value::Builtin(Builtins::ExcType(*e)),
                                Value::Builtin(Builtins::Function(BuiltinsFunctions::ExceptionNew)),
                                vm.heap,
                            ));
                            break;
                        }
                        ClassBase::Object => {
                            found = Some(Value::Builtin(Builtins::Function(BuiltinsFunctions::ObjectNew)));
                            break;
                        }
                    }
                }
                found.unwrap_or(Value::Builtin(Builtins::Function(BuiltinsFunctions::ObjectNew)))
            } else {
                Value::Builtin(Builtins::Function(BuiltinsFunctions::ObjectNew))
            };
            return Ok(Some(CallResult::Value(func)));
        }

        if attr_str == "__init__" {
            let mut init_fn = BuiltinsFunctions::ObjectInit;
            if let Some(search_bases) = get_search_bases(type_arg, obj, vm) {
                for base in search_bases {
                    match base {
                        ClassBase::Class(_) => {}
                        ClassBase::BuiltinExc(_) => {
                            init_fn = BuiltinsFunctions::ExceptionInit;
                            break;
                        }
                        ClassBase::Object => break,
                    }
                }
            }
            let bound_val = BoundMethod::allocate(
                obj.clone_with_heap(vm.heap),
                Value::Builtin(Builtins::Function(init_fn)),
                vm.heap,
            );
            return Ok(Some(CallResult::Value(bound_val)));
        }

        if attr_str == "__init_subclass__" {
            let target = match &obj {
                Value::Ref(id) if let HeapData::Instance(inst) = vm.heap.get(*id) => {
                    let cid = inst.class();
                    vm.heap.inc_ref(cid);
                    Value::Ref(cid)
                }
                _ => obj.clone_with_heap(vm.heap),
            };
            let bound_val = BoundMethod::allocate(
                target,
                Value::Builtin(Builtins::Function(BuiltinsFunctions::ObjectInitSubclass)),
                vm.heap,
            );
            return Ok(Some(CallResult::Value(bound_val)));
        }

        if attr_str == "__setattr__" {
            let bound_val = BoundMethod::allocate(
                obj.clone_with_heap(vm.heap),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::ObjectSetattr)),
                vm.heap,
            );
            return Ok(Some(CallResult::Value(bound_val)));
        }

        Err(ExcType::attribute_error("super", attr_str))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);

        if matches!(attr_str, "__thisclass__" | "__self__" | "__self_class__")
            && let Some(CallResult::Value(val)) = self.py_getattr(attr, vm)?
        {
            defer_drop!(val, vm);
            return vm.call_function(val, args);
        }

        let s = self.get(vm.heap);

        let Some(obj) = s.obj_arg.as_ref().map(|o| o.clone_with_heap(vm.heap)) else {
            args.drop_with(vm.heap);
            return Err(ExcType::attribute_error("super", attr_str));
        };
        defer_drop!(obj, vm);

        let type_arg = self.get(vm.heap).type_arg.clone_with_heap(vm.heap);
        defer_drop!(type_arg, vm);

        if attr_str == "__init__" {
            if let Some(search_bases) = get_search_bases(type_arg, obj, vm) {
                for base in search_bases {
                    match base {
                        ClassBase::Class(ancestor_id) => {
                            if let HeapData::Class(ancestor_cls) = vm.heap.get(*ancestor_id)
                                && let Some(member) =
                                    ancestor_cls.namespace().get_by_str("__init__", vm.heap, vm.interns)
                            {
                                let member_val = member.clone_with_heap(vm.heap);
                                if is_method_value(&member_val, vm) {
                                    let bound = BoundMethod {
                                        instance: obj.clone_with_heap(vm.heap),
                                        func: member_val,
                                    };
                                    let bound_id = vm.heap.allocate(HeapData::BoundMethod(bound));
                                    let bound_val = Value::Ref(bound_id);
                                    defer_drop!(bound_val, vm);
                                    return vm.call_function(bound_val, args);
                                }
                                defer_drop!(member_val, vm);
                                return vm.call_function(member_val, args);
                            }
                        }
                        ClassBase::BuiltinExc(_) => {
                            return builtin_exception_init(vm, args.prepend(obj.clone_with_heap(vm.heap)))
                                .map(CallResult::Value);
                        }
                        ClassBase::Object => {
                            return builtin_object_init(vm, args.prepend(obj.clone_with_heap(vm.heap)))
                                .map(CallResult::Value);
                        }
                    }
                }
            }

            return builtin_object_init(vm, args.prepend(obj.clone_with_heap(vm.heap))).map(CallResult::Value);
        }

        if let Some(member) = super_mro_lookup(type_arg, obj, attr_str, vm) {
            defer_drop!(member, vm);
            return vm.call_function(member, args);
        }

        if attr_str == "__new__" {
            if let Some(search_bases) = get_search_bases(type_arg, obj, vm) {
                for base in search_bases {
                    match base {
                        ClassBase::Class(_) => {}
                        ClassBase::BuiltinExc(e) => {
                            return builtin_exception_new(vm, args.prepend(Value::Builtin(Builtins::ExcType(*e))))
                                .map(CallResult::Value);
                        }
                        ClassBase::Object => {
                            return builtin_object_new(vm, args).map(CallResult::Value);
                        }
                    }
                }
            }
            return builtin_object_new(vm, args).map(CallResult::Value);
        }

        if attr_str == "__init_subclass__" {
            let target = match &obj {
                Value::Ref(id) if let HeapData::Instance(inst) = vm.heap.get(*id) => {
                    let cid = inst.class();
                    vm.heap.inc_ref(cid);
                    Value::Ref(cid)
                }
                _ => obj.clone_with_heap(vm.heap),
            };
            return builtin_object_init_subclass(vm, args.prepend(target)).map(CallResult::Value);
        }

        if attr_str == "__setattr__" {
            return builtin_object_setattr(vm, args.prepend(obj.clone_with_heap(vm.heap))).map(CallResult::Value);
        }

        args.drop_with(vm.heap);
        Err(ExcType::attribute_error("super", attr_str))
    }
}

impl HeapItem for Super {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.type_arg.py_dec_ref_ids(stack);
        if let Some(obj) = &mut self.obj_arg {
            obj.py_dec_ref_ids(stack);
        }
    }
}
fn zero_arg_super(vm: &mut VM<'_>) -> RunResult<Value> {
    let (base, func_id) = {
        let frame = vm.current_frame();
        (frame.stack_base(), frame.function_id())
    };
    let Some(func_id) = func_id else {
        return Err(ExcType::runtime_error("super(): no arguments"));
    };
    let (has_pos_param, self_cell_slot, class_cell_slot) = {
        let func = vm.interns.get_function(func_id);
        let has_pos_param = func.signature.has_positional_param();
        let self_cell_slot = func
            .cell_param_indices
            .iter()
            .position(|&p| p == Some(0))
            .map(|idx| func.cell_var_slots[idx].index());
        let class_cell_slot = func
            .free_var_slots
            .iter()
            .copied()
            .find(|slot| {
                func.code
                    .local_name(slot.as_u16())
                    .is_some_and(|id| vm.interns.get_str(id) == "__class__")
            })
            .map(NamespaceId::index);
        (has_pos_param, self_cell_slot, class_cell_slot)
    };
    if !has_pos_param {
        return Err(ExcType::runtime_error("super(): no arguments"));
    }

    let self_val = if let Some(slot) = self_cell_slot {
        let Value::Ref(cell_id) = vm.stack[base + slot] else {
            unreachable!("captured parameter slot must hold a Cell reference");
        };
        let HeapData::Cell(cell) = vm.heap.get(cell_id) else {
            unreachable!("captured parameter slot must point to a Cell");
        };
        cell.0.clone_with_heap(vm.heap)
    } else {
        vm.stack[base].clone_with_heap(vm.heap)
    };
    if matches!(self_val, Value::Undefined) {
        return Err(ExcType::runtime_error("super(): arg[0] deleted"));
    }
    defer_drop!(self_val, vm);

    let Some(class_slot) = class_cell_slot else {
        return Err(ExcType::runtime_error("super(): __class__ cell not found"));
    };
    let class_val = {
        let Value::Ref(cell_id) = vm.stack[base + class_slot] else {
            unreachable!("__class__ free-var slot must hold a Cell reference");
        };
        let HeapData::Cell(cell) = vm.heap.get(cell_id) else {
            unreachable!("__class__ free-var slot must point to a Cell");
        };
        cell.0.clone_with_heap(vm.heap)
    };
    if matches!(class_val, Value::Undefined) {
        return Err(ExcType::runtime_error("super(): empty __class__ cell"));
    }
    defer_drop!(class_val, vm);

    if !is_type_value(class_val, vm) {
        let got = class_val.py_type_name(vm);
        return Err(ExcType::runtime_error(format!(
            "super(): __class__ is not a type ({got})"
        )));
    }

    if matches!(self_val, Value::None) {
        let type_arg = class_val.clone_with_heap(vm.heap);
        let s = Super::new(type_arg, None);
        return Ok(Value::Ref(vm.heap.allocate(HeapData::Super(s))));
    }

    validate_super_obj(class_val, self_val, vm)?;

    let type_arg = class_val.clone_with_heap(vm.heap);
    let obj_arg = self_val.clone_with_heap(vm.heap);
    let s = Super::new(type_arg, Some(obj_arg));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Super(s))))
}

fn is_type_value(val: &Value, vm: &VM<'_>) -> bool {
    match val {
        Value::Builtin(Builtins::Type(_) | Builtins::ExcType(_) | Builtins::Function(BuiltinsFunctions::Type)) => true,
        Value::Ref(id) => matches!(
            vm.heap.get(*id),
            HeapData::Class(_) | HeapData::NamedTupleClass(_) | HeapData::HostClassType(_)
        ),
        _ => false,
    }
}

fn validate_super_type(val: &Value, vm: &VM<'_>) -> RunResult<()> {
    if is_type_value(val, vm) {
        Ok(())
    } else {
        let got = val.py_type(vm).cpython_arg_name(vm.heap, vm.interns);
        Err(ExcType::type_error(format!(
            "super() argument 1 must be a type, not {got}"
        )))
    }
}

fn type_or_class_name<'i>(val: &Value, vm: &'i VM<'_>) -> Cow<'i, str> {
    match val {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Class(_) => class_name(*id, vm.heap, vm.interns),
            HeapData::NamedTupleClass(nt) => Cow::Borrowed(nt.name(vm.interns)),
            HeapData::HostClassType(ty) => Cow::Borrowed(ty.name(vm.interns)),
            _ => val.py_type_name(vm),
        },
        Value::Builtin(Builtins::Type(t)) => Type::from(*t).name(vm.heap, vm.interns),
        Value::Builtin(Builtins::ExcType(e)) => Cow::Borrowed((*e).into()),
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Type)) => Cow::Borrowed("type"),
        _ => val.py_type_name(vm),
    }
}

fn validate_super_obj(type_arg: &Value, obj_arg: &Value, vm: &mut VM<'_>) -> RunResult<()> {
    if matches!(obj_arg, Value::None) {
        return Ok(());
    }
    if isinstance_check(obj_arg, type_arg, vm)? {
        return Ok(());
    }
    if is_type_value(obj_arg, vm) {
        if issubclass_check(obj_arg, type_arg, vm)? {
            return Ok(());
        }
        let obj_name = type_or_class_name(obj_arg, vm);
        let type_name = type_or_class_name(type_arg, vm);
        return Err(ExcType::type_error(format!(
            "super(type, obj): obj (type {obj_name}) is not an instance or subtype of type ({type_name})."
        )));
    }
    let obj_name = obj_arg.py_type_name(vm);
    let type_name = type_or_class_name(type_arg, vm);
    Err(ExcType::type_error(format!(
        "super(type, obj): obj (instance of {obj_name}) is not an instance or subtype of type ({type_name})."
    )))
}

fn get_search_bases<'a>(type_arg: &Value, obj_arg: &Value, vm: &'a VM<'_>) -> Option<&'a [ClassBase]> {
    let self_class_id = match obj_arg {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Instance(inst) => inst.class(),
            HeapData::Class(_) => *id,
            _ => return None,
        },
        _ => return None,
    };

    let HeapData::Class(self_class) = vm.heap.get(self_class_id) else {
        return None;
    };

    if let Value::Ref(target_id) = type_arg {
        if *target_id == self_class_id {
            Some(self_class.mro())
        } else {
            let pos = self_class
                .mro()
                .iter()
                .position(|b| matches!(b, ClassBase::Class(id) if id == target_id))?;
            Some(&self_class.mro()[pos + 1..])
        }
    } else if let Value::Builtin(Builtins::ExcType(target_exc)) = type_arg {
        let pos = self_class
            .mro()
            .iter()
            .position(|b| matches!(b, ClassBase::BuiltinExc(exc) if exc == target_exc))?;
        Some(&self_class.mro()[pos + 1..])
    } else if matches!(type_arg, Value::Builtin(Builtins::Type(BuiltinType::Object))) {
        let pos = self_class.mro().iter().position(|b| matches!(b, ClassBase::Object))?;
        Some(&self_class.mro()[pos + 1..])
    } else {
        None
    }
}

/// Resolves `__self_class__` for a `super` instance: if `obj_arg` is a class or
/// subtype object, returns it directly; otherwise returns the type of the instance.
fn resolve_super_self_class(type_arg: &Value, obj_arg: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    if is_type_value(obj_arg, vm) && issubclass_check(obj_arg, type_arg, vm)? {
        Ok(obj_arg.clone_with_heap(vm.heap))
    } else {
        Ok(type_of_ref(vm, obj_arg))
    }
}

fn super_mro_lookup(type_arg: &Value, obj_arg: &Value, attr_str: &str, vm: &mut VM<'_>) -> Option<Value> {
    let search_bases = get_search_bases(type_arg, obj_arg, vm)?;

    let is_obj_class = matches!(
        obj_arg,
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Class(_))
    );

    for base in search_bases {
        match base {
            ClassBase::Class(ancestor_id) => {
                if let HeapData::Class(ancestor_cls) = vm.heap.get(*ancestor_id)
                    && let Some(member) = ancestor_cls.namespace().get_by_str(attr_str, vm.heap, vm.interns)
                {
                    let member_val = member.clone_with_heap(vm.heap);
                    if attr_str != "__new__"
                        && (!is_obj_class || attr_str == "__init_subclass__")
                        && is_method_value(&member_val, vm)
                    {
                        let receiver = if attr_str == "__init_subclass__"
                            && let Value::Ref(id) = obj_arg
                            && let HeapData::Instance(inst) = vm.heap.get(*id)
                        {
                            let cid = inst.class();
                            vm.heap.inc_ref(cid);
                            Value::Ref(cid)
                        } else {
                            obj_arg.clone_with_heap(vm.heap)
                        };
                        return Some(BoundMethod::allocate(receiver, member_val, vm.heap));
                    }
                    return Some(member_val);
                }
            }
            ClassBase::BuiltinExc(_) | ClassBase::Object => {
                if attr_str == "__init__" || attr_str == "__new__" {
                    return None;
                }
            }
        }
    }

    None
}
