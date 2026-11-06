//! Implementation of the issubclass() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapRead, HeapReadOutput},
    types::{Tuple, TypeRef},
    value::Value,
};

/// Arguments of `issubclass(cls, classinfo, /)`.
#[derive(FromArgs)]
#[from_args(name = "issubclass", style = unpack)]
struct IssubclassArgs {
    #[from_args(pos_only)]
    cls: Value,
    #[from_args(pos_only)]
    classinfo: Value,
}

/// Implementation of the issubclass() builtin function.
///
/// Checks whether a class inherits from a class, or from any of a tuple or
/// union of classes.
pub fn builtin_issubclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IssubclassArgs { cls, classinfo } = IssubclassArgs::from_args(args, vm)?;
    defer_drop!(cls, vm);
    defer_drop!(classinfo, vm);
    issubclass_check(cls, classinfo, vm).map(Value::Bool)
}

/// Whether `cls` is `classinfo`, a subclass of it, or of any member of a tuple
/// or union of classes. As in CPython, a tuple or union is unpacked before
/// `cls` is checked, so `issubclass(1, ())` is `False`.
pub(crate) fn issubclass_check(cls: &Value, classinfo: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    match classinfo {
        Value::Ref(id) if let HeapReadOutput::Tuple(tuple) = vm.heap.read(*id) => {
            // A tuple can hold itself many times over, so walking nested
            // tuples is exponential in their depth: bound it by time.
            vm.heap.tracker.check_time()?;
            issubclass_check_tuple(cls, &tuple, vm)
        }
        Value::Ref(id) if let HeapData::Union(union) = vm.heap.get(*id) => {
            let args = union.args(vm.heap);
            defer_drop!(args, vm);
            let Some(HeapReadOutput::Tuple(members)) = args.read_heap(vm) else {
                unreachable!("Union::args is always a tuple")
            };
            issubclass_check_tuple(cls, &members, vm)
        }
        _ => {
            let Some(class) = TypeRef::from_value(cls, vm.heap) else {
                return Err(ExcType::issubclass_arg1_error());
            };
            match TypeRef::from_value(classinfo, vm.heap) {
                Some(base) => Ok(class.is_subclass_of(base, vm.heap)),
                None if matches!(classinfo, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::GenericAlias(_))) => {
                    Err(ExcType::issubclass_parameterized_generic())
                }
                None => Err(ExcType::issubclass_arg2_error()),
            }
        }
    }
}

/// Walks a tuple of classinfo entries, nested tuples included.
fn issubclass_check_tuple<'h>(cls: &Value, tuple: &HeapRead<'h, Tuple>, vm: &mut VM<'h>) -> RunResult<bool> {
    let len = tuple.get(vm.heap).as_slice().len();
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    for i in 0..len {
        let item = tuple.get(vm.heap).as_slice()[i].clone_with_heap(vm.heap);
        defer_drop!(item, vm);
        if issubclass_check(cls, item, vm)? {
            return Ok(true);
        }
    }
    Ok(false)
}
