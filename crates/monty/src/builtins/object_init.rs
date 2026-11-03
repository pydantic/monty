//! Implementation of `object.__init__`, the default instance constructor.

use monty_types::ExcType;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcTypeExt, RunResult},
    heap::{DropWithContext, HeapData, HeapReadOutput},
    intern::StaticStrings,
    types::{py_trait::PyTrait, tuple::allocate_tuple},
    value::{EitherStr, Value},
};

/// `object.__init__(self, /, *args, **kwargs)` — default constructor.
pub fn builtin_object_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let has_kwargs = !kwargs.is_empty();
    kwargs.drop_with(vm.heap);
    let Some(self_val) = pos.next() else {
        return Err(ExcType::type_error(
            "descriptor '__init__' of 'object' object needs an argument",
        ));
    };
    defer_drop!(self_val, vm);
    let has_extra = pos.len() > 0 || has_kwargs;
    pos.drop_with(vm.heap);
    if has_extra {
        if let Value::Ref(id) = &self_val
            && let HeapData::Instance(inst) = vm.heap.get(*id)
            && let HeapData::Class(cls) = vm.heap.get(inst.class())
        {
            let overrides_new = cls.mro_getattr_ref("__new__", vm.heap, vm.interns).is_some();
            let overrides_init = cls.mro_getattr_ref("__init__", vm.heap, vm.interns).is_some();
            if overrides_new && !overrides_init {
                return Ok(Value::None);
            }
            let type_name = if cls.is_exception_class() || overrides_init {
                "object"
            } else {
                cls.name().as_str(vm.interns)
            };
            return Err(ExcType::type_error(format!(
                "{type_name}.__init__() takes exactly one argument (the instance to initialize)"
            )));
        }
        return Err(ExcType::type_error(
            "object.__init__() takes exactly one argument (the instance to initialize)",
        ));
    }
    Ok(Value::None)
}

/// `BaseException.__init__(self, /, *args)` — sets `self.args` and rejects keyword arguments.
pub fn builtin_exception_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (mut pos, kwargs) = args.into_parts();
    let Some(self_val) = pos.next() else {
        kwargs.drop_with(vm.heap);
        return Err(ExcType::type_error(
            "descriptor '__init__' of 'BaseException' object needs an argument",
        ));
    };
    defer_drop!(self_val, vm);

    if let Value::Ref(id) = &self_val {
        match vm.heap.get(*id) {
            HeapData::Instance(inst) if matches!(vm.heap.get(inst.class()), HeapData::Class(cls) if cls.is_exception_class()) =>
            {
                if !kwargs.is_empty() {
                    pos.drop_with(vm.heap);
                    kwargs.drop_with(vm.heap);
                    return Err(ExcType::type_error(
                        "BaseException.__init__() takes no keyword arguments",
                    ));
                }
                kwargs.drop_with(vm.heap);
                let pos_vec: Vec<Value> = pos.collect();
                let args_tuple = allocate_tuple(pos_vec.into(), vm.heap);
                self_val.py_set_attr(
                    &EitherStr::Interned(vm.interns.intern_static(StaticStrings::Args)),
                    args_tuple,
                    vm,
                )?;
                return Ok(Value::None);
            }
            HeapData::Exception(_) => {
                if !kwargs.is_empty() {
                    pos.drop_with(vm.heap);
                    kwargs.drop_with(vm.heap);
                    return Err(ExcType::type_error(
                        "BaseException.__init__() takes no keyword arguments",
                    ));
                }
                kwargs.drop_with(vm.heap);
                return match pos.len() {
                    0 => {
                        if let HeapReadOutput::Exception(exc) = vm.heap.read(*id) {
                            exc.get_mut(vm.heap).set_arg(None);
                        }
                        Ok(Value::None)
                    }
                    1 => {
                        let arg = pos.next().expect("length checked");
                        defer_drop!(arg, vm);
                        let msg = if let Ok(s) = arg.try_to_str(vm) {
                            s.to_owned()
                        } else {
                            let s = arg.py_str(vm)?;
                            defer_drop!(s, vm);
                            s.try_to_str(vm)?.to_owned()
                        };
                        if let HeapReadOutput::Exception(exc) = vm.heap.read(*id) {
                            exc.get_mut(vm.heap).set_arg(Some(msg));
                        }
                        Ok(Value::None)
                    }
                    _ => {
                        pos.drop_with(vm.heap);
                        Err(ExcType::type_error(
                            "exceptions can only be called with zero or one string argument",
                        ))
                    }
                };
            }
            _ => {}
        }
    }

    pos.drop_with(vm.heap);
    kwargs.drop_with(vm.heap);
    let type_name = self_val.py_type_name(vm);
    Err(ExcType::type_error(format!(
        "descriptor '__init__' requires a 'BaseException' object but received a '{type_name}'"
    )))
}
