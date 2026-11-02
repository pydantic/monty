//! The function `pydantic.validate_call` returns: the wrapped function, the
//! arena of validators built from its annotations, and one entry per
//! parameter saying which validator checks it.
//!
//! Calling the wrapper validates the arguments as pydantic-core's `arguments`
//! schema does (see `modules/pydantic/validate_call.rs`), then calls the
//! function from a frozen Python frame, so a suspending function body still
//! works. To Python the wrapper is a `function`: it binds as a method and
//! reports the wrapped function's name.

use std::{fmt::Write, rc::Rc};

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, ParameterKind},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropWithContext, HeapId, HeapItem, HeapObjectRead},
    intern::{StaticStrings, StringId},
    modules::pydantic::{
        arena::{Arena, FieldDefault, ValidatorId},
        validate_call::call_wrapper,
    },
    types::{LazyHeapSet, PyTrait, Type, allocate_string},
    value::{EitherStr, Value},
};

/// One parameter of the wrapped function and how a call supplies it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ParamValidator {
    pub(crate) name: StringId,
    pub(crate) kind: ParameterKind,
    pub(crate) validator: ValidatorId,
    /// What an omitted argument becomes: `Required` reports `missing_argument`,
    /// a value or factory from a `Field()` default is supplied by the wrapper,
    /// and `None` leaves the function's own default to the call.
    pub(crate) default: Option<FieldDefault>,
}

impl<C: ContainsHeap> DropWithContext<C> for ParamValidator {
    fn drop_with(self, ctx: &mut C) {
        if let Some(default) = self.default {
            default.drop_with(ctx);
        }
    }
}

/// A `validate_call` wrapper; every `Value` is an OWNED reference.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ValidateCall {
    pub(crate) function: Value,
    pub(crate) arena: Rc<Arena>,
    pub(crate) params: Vec<ParamValidator>,
    /// `validate_return=True`: the validator for the result.
    pub(crate) return_validator: Option<ValidatorId>,
    /// The config's `strict`.
    pub(crate) strict: bool,
    /// The function's name: the `ValidationError` title and `__name__`.
    pub(crate) name: String,
}

impl ValidateCall {
    /// Every heap id the wrapper owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.function {
            on_child(*id);
        }
        self.arena.for_each_child_id(&mut on_child);
        for param in &self.params {
            if let Some(FieldDefault::Value(value) | FieldDefault::Factory(value)) = &param.default
                && let Value::Ref(id) = value
            {
                on_child(*id);
            }
        }
    }
}

impl HeapItem for ValidateCall {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.function.py_dec_ref_ids(stack);
        match Rc::get_mut(&mut self.arena) {
            Some(arena) => arena.py_dec_ref_ids(stack),
            None => self.arena.for_each_child_id(|id| stack.push(id)),
        }
        for param in &mut self.params {
            if let Some(FieldDefault::Value(value) | FieldDefault::Factory(value)) = &mut param.default {
                value.py_dec_ref_ids(stack);
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for ValidateCall {
    fn drop_with(self, ctx: &mut C) {
        self.function.drop_with(ctx);
        if let Ok(arena) = Rc::try_unwrap(self.arena) {
            arena.drop_with(ctx);
        }
        for param in self.params {
            if let Some(default) = param.default {
                default.drop_with(ctx);
            }
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ValidateCall> {
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        call_wrapper(self.id(), args, vm)
    }

    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Function
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

    /// `<function name at 0x...>`, as the wrapped function prints.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(
            f,
            "<function {} at 0x{:x}>",
            self.get(vm.heap).name,
            self.id().index()
        )?)
    }

    /// `__name__` and `__qualname__` name the function; `raw_function` is the function itself.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let value = match attr.static_string(vm.interns) {
            Some(StaticStrings::DunderName | StaticStrings::DunderQualname) => {
                allocate_string(self.get(vm.heap).name.as_str(), vm.heap)
            }
            Some(StaticStrings::RawFunction) => self.get(vm.heap).function.clone_with_heap(vm.heap),
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    /// `wrapper.raw_function(...)` and the like: the attribute, called.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match self.py_getattr(attr, vm)? {
            Some(CallResult::Value(callable)) => {
                defer_drop!(callable, vm);
                vm.call_function(callable, args)
            }
            _ => Err(ExcType::attribute_error_method("function", attr, args, vm)),
        }
    }
}
