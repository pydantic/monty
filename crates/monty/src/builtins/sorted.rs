//! Implementation of the sorted() builtin function.

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    exception_private::{RunError, RunResult},
    heap::{DropGuard, DropWithContext, Heap},
    resource_checks::check_estimated_size,
    sorting::parse_and_sort,
    types::{List, iter::collect_owned_iterable, sequence::SequenceItems},
    value::{VALUE_SIZE, Value},
};

fn clone_items_with_checks(items: SequenceItems<'_>, heap: &Heap) -> Result<Vec<Value>, Box<(Vec<Value>, RunError)>> {
    let len = items.len();
    if let Err(e) = check_estimated_size(len.saturating_mul(VALUE_SIZE), &heap.tracker) {
        return Err(Box::new((Vec::new(), e.into())));
    }
    let mut cloned = Vec::with_capacity(len);
    if let Err(err) = items.extend_cloned(&mut cloned, heap) {
        return Err(Box::new((cloned, err)));
    }
    Ok(cloned)
}

pub fn builtin_sorted(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    // CPython collects the iterable before checking the sort options.
    let (positional, options) = match args {
        ArgValues::ArgsKargs { args, kwargs } => (
            ArgValues::from_parts(args, KwargsValues::Empty),
            ArgValues::Kwargs(kwargs),
        ),
        ArgValues::Kwargs(_) => (ArgValues::Empty, args),
        positional => (positional, ArgValues::Empty),
    };
    let mut options = DropGuard::new(options, vm);
    let vm = options.ctx();
    let iterable = positional.get_one_arg("sorted", vm.heap)?;

    let items = if let Some(items) = SequenceItems::from_value(&iterable, vm.heap) {
        let res = clone_items_with_checks(items, vm.heap);
        iterable.drop_with(vm);
        match res {
            Ok(items) => items,
            Err(boxed) => {
                let (partial, err) = *boxed;
                partial.drop_with(vm);
                return Err(err);
            }
        }
    } else {
        collect_owned_iterable(iterable, vm)?
    };
    let (options, vm) = options.into_parts();
    let mut items_guard = DropGuard::new(items, vm);
    let (items, vm) = items_guard.as_parts_mut();

    parse_and_sort("sorted", items, options, vm)?;

    let (items, vm) = items_guard.into_parts();
    Ok(Value::allocate(List::new(items), vm.heap))
}
