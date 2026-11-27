//! Namespace introspection with vars().

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult},
    frozen::FrozenFunction,
    types::descriptor,
    value::Value,
};

pub(crate) const SOURCE: &str = r"
def _vars(get, obj, attribute_error, type_error):
    try:
        return get(obj, '__dict__')
    except attribute_error:
        raise type_error('vars() argument must have __dict__ attribute')
";

pub(super) fn builtin_vars(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let Some(obj) = args.get_zero_one_arg("vars", vm.heap)? else {
        return vm.locals_dict().map(CallResult::Value);
    };
    vm.call_frozen_exact(
        FrozenFunction::Vars,
        [
            descriptor::attribute_helper(),
            obj,
            Value::Builtin(Builtins::Type(ExcType::AttributeError.into())),
            Value::Builtin(Builtins::Type(ExcType::TypeError.into())),
        ],
    )
}
