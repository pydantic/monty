//! Argument binding for the lazy filter iterator.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    types::map_filter::MapFilter,
};

/// Acquires the source iterator at construction, leaving predicate calls until advancement.
pub fn builtin_filter(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let args = args.reject_kwargs("filter", vm.heap)?;
    let (function, iterable) = args.get_two_args("filter", vm.heap)?;
    MapFilter::new_filter(function, iterable, vm)
}
