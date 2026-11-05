//! Implementation of `object.__init_subclass__`, the default subclass hook.

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::HeapData,
    types::{Type, class::validate_init_subclass_args},
    value::Value,
};

/// `object.__init_subclass__(cls, /, *args, **kwargs)` — default subclass hook.
pub fn builtin_object_init_subclass(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let target_name = if let Some(cls_val) = pos.next() {
        defer_drop!(cls_val, vm);
        match &cls_val {
            Value::Ref(id) if let HeapData::Class(cls) = vm.heap.get(*id) => cls.name().as_str(vm.interns).to_owned(),
            Value::Builtin(Builtins::Type(t)) => Type::from(*t).dunder_name(vm.heap, vm.interns).into_owned(),
            Value::Builtin(Builtins::ExcType(e)) => {
                let name: &'static str = (*e).into();
                name.rsplit_once('.').map_or(name, |(_, b)| b).to_owned()
            }
            _ => cls_val.py_type_name(vm).into_owned(),
        }
    } else {
        "object".to_owned()
    };
    let remaining_args = ArgValues::from_parts(pos.collect(), kwargs);
    validate_init_subclass_args(&target_name, remaining_args, vm.heap)?;
    Ok(Value::None)
}
