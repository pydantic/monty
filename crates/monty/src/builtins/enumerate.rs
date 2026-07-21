//! Implementation of the enumerate() builtin function.

use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{DropGuard, DropWithContext, HeapData},
    resource::ResourceTracker,
    types::{List, allocate_tuple},
    value::Value,
};

/// Implementation of the enumerate() builtin function.
///
/// Returns a list of (index, value) tuples.
/// Note: In Python this returns an iterator, but we return a list for simplicity.
pub fn builtin_enumerate(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let (iterable, start) = extract_enumerate_args(args, vm)?;
    // Guard `start` before building the iterator: a non-iterable `iterable`
    // errors out of `py_iter`, and a heap-backed start must not leak.
    defer_drop!(start, vm);
    let iter = iterable.into_py_iter(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);

    // Get start index (default 0)
    let mut index: i64 = match start {
        Some(Value::Int(n)) => *n,
        Some(Value::Bool(b)) => i64::from(*b),
        Some(v) => {
            let type_name = v.py_type_name(vm);
            return Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("'{type_name}' object cannot be interpreted as an integer"),
            )
            .into());
        }
        None => 0,
    };

    let result: Vec<Value> = Vec::new();
    let mut result_guard = DropGuard::new(result, vm);
    let (result, vm) = result_guard.as_parts_mut();

    while let Some(item) = iter.py_next(vm)? {
        // Create tuple (index, item)
        let tuple_val = allocate_tuple(smallvec![Value::Int(index), item], vm.heap)?;
        result.push(tuple_val);
        index += 1;
    }

    let (result, vm) = result_guard.into_parts();
    let heap_id = vm.heap.allocate(HeapData::List(List::new(result)))?;
    Ok(Value::Ref(heap_id))
}

/// Extracts `enumerate(iterable, start=0)` arguments, faithfully mirroring
/// CPython's hand-written `enumerate_vectorcall` (Objects/enumobject.c) —
/// which `#[derive(FromArgs)]` cannot express. Its quirks are deliberate:
/// keyword names are validated by *position* against the accepted shapes, and
/// zero positionals with an unrecognised keyword shape reports the missing
/// `iterable` even when `iterable=` was actually passed.
fn extract_enumerate_args(args: ArgValues, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<(Value, Option<Value>)> {
    let (mut pos, kwargs) = args.into_parts();
    let n_pos = pos.len();
    let total = n_pos + kwargs.len();
    let mut kw = kwargs.into_iter();
    // Pull up to two of each stream; each accepted call shape is one pattern
    // arm, and the catch-all owns the leftovers on the arity-error path.
    match (pos.next(), pos.next(), kw.next(), kw.next()) {
        // enumerate(x) / enumerate(x, s)
        (Some(iterable), start, None, None) if total <= 2 => Ok((iterable, start)),
        // enumerate(x, start=s)
        (Some(iterable), None, Some(kv), None) => match take_kwarg(kv, "start", vm) {
            Ok(start) => Ok((iterable, Some(start))),
            Err(err) => {
                iterable.drop_with(vm);
                Err(err)
            }
        },
        // enumerate(iterable=x)
        (None, None, Some(kv), None) => Ok((take_kwarg(kv, "iterable", vm)?, None)),
        // enumerate(iterable=x, start=s) — either keyword order
        (None, None, Some(kv0), Some(kv1)) if total == 2 => two_kwarg_form(kv0, kv1, vm),
        // Anything else is an arity error.
        (p0, p1, k0, k1) => {
            ((p0, p1), (k0, k1)).drop_with(vm);
            pos.drop_with(vm);
            kw.drop_with(vm);
            if n_pos == 0 {
                // CPython reports the missing `iterable` for every
                // zero-positional shape it doesn't recognise — even
                // `enumerate(start=1, x=1, iterable=[1])`.
                Err(ExcType::type_error_missing_required_no_pos("enumerate", "iterable"))
            } else {
                Err(ExcType::type_error_method_at_most("enumerate", 2, total))
            }
        }
    }
}

/// The `enumerate(iterable=..., start=...)` form: CPython accepts both
/// keyword orders, checking `start` first and reporting the first
/// out-of-place name.
fn two_kwarg_form(
    kv0: (Value, Value),
    kv1: (Value, Value),
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<(Value, Option<Value>)> {
    // Decide the shape by reference before any ownership moves.
    let swapped = key_check(&kv0.0, "start", vm).is_ok();
    let checked = if swapped {
        key_check(&kv1.0, "iterable", vm)
    } else {
        key_check(&kv0.0, "iterable", vm).and(key_check(&kv1.0, "start", vm))
    };
    match checked {
        Ok(()) => {
            let ((key0, val0), (key1, val1)) = (kv0, kv1);
            (key0, key1).drop_with(vm);
            Ok(if swapped {
                (val1, Some(val0))
            } else {
                (val0, Some(val1))
            })
        }
        Err(err) => {
            (kv0, kv1).drop_with(vm);
            Err(err)
        }
    }
}

/// Consumes a kwarg pair, returning its value if the key is `expected`.
fn take_kwarg(kv: (Value, Value), expected: &str, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Value> {
    match key_check(&kv.0, expected, vm) {
        Ok(()) => {
            let (key, value) = kv;
            key.drop_with(vm);
            Ok(value)
        }
        Err(err) => {
            kv.drop_with(vm);
            Err(err)
        }
    }
}

/// Errors unless `key` is the string `expected`, mirroring CPython's
/// `check_keyword`: a non-string key raises `keywords must be strings`, any
/// other name the `invalid keyword argument` wording.
fn key_check(key: &Value, expected: &str, vm: &VM<'_, impl ResourceTracker>) -> RunResult<()> {
    let Some(key) = key.as_either_str(vm.heap) else {
        return Err(ExcType::type_error_kwargs_nonstring_key());
    };
    let key = key.as_str(vm.interns);
    if key == expected {
        Ok(())
    } else {
        Err(ExcType::type_error_invalid_keyword_argument("enumerate", key))
    }
}
