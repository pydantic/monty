//! `_test_cm()` builtin — constructs a synthetic context manager.
//!
//! **REMOVE THIS FILE** along with [`crate::types::test_cm`] once a real
//! production type covers the `with` statement branches this helper exists
//! to test. See that module's docstring for the full removal checklist.
//!
//! The function is intentionally not listed in CPython's builtins; it's
//! only present under the `test-hooks` cargo feature so a production
//! sandbox can never construct one.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::HeapData,
    resource::ResourceTracker,
    types::{PyTrait, TestContextManager},
    value::Value,
};

/// `_test_cm()` / `_test_cm(behavior)` / `_test_cm(behavior, payload)` —
/// constructs a synthetic context manager configured by a behavior string
/// and optional payload.
///
/// Positional API rather than kwargs to keep the implementation small (no
/// keyword parsing) and the test sites readable. The supported behaviors
/// are:
///
/// | behavior          | payload    | effect                                              |
/// | ----------------- | ---------- | --------------------------------------------------- |
/// | (none)            | —          | passthrough: returns self on enter, None on exit    |
/// | `"suppress"`      | (none)     | `__exit__` returns True on the exception path       |
/// | `"enter_value"`   | int        | `__enter__` returns the int instead of self         |
/// | `"raise_on_enter"`| str        | `__enter__` raises `ValueError(payload)`            |
/// | `"raise_on_exit"` | str        | `__exit__` raises `ValueError(payload)`             |
///
/// Each behavior pins exactly one branch of the `with` machinery, so a
/// single test can verify that branch in isolation.
pub fn builtin_test_cm(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let TestCmArgs { behavior, payload } = TestCmArgs::from_args(args, vm)?;

    let mut cm = TestContextManager::new();
    if let Some(behavior_value) = behavior {
        defer_drop!(behavior_value, vm);
        let Some(behavior_str) = value_as_owned_str(behavior_value, vm) else {
            if let Some(p) = payload {
                p.drop_with_heap(vm);
            }
            return Err(ExcType::type_error(format!(
                "_test_cm() behavior must be str, not {}",
                behavior_value.py_type(vm)
            )));
        };
        configure(&mut cm, &behavior_str, payload, vm)?;
    } else if let Some(p) = payload {
        // payload supplied with no behavior is a usage error
        p.drop_with_heap(vm);
        return Err(ExcType::type_error(
            "_test_cm() payload requires a leading behavior argument".to_owned(),
        ));
    }

    let heap_id = vm.heap.allocate(HeapData::TestContextManager(cm))?;
    Ok(Value::Ref(heap_id))
}

/// Positional-only argument shape for `_test_cm([behavior[, payload]])`.
/// Both fields are pos_only because this test helper deliberately exposes
/// no kwarg surface — there's no CPython equivalent to mirror, and the
/// tests that drive it always pass positionals.
#[derive(FromArgs)]
#[from_args(name = "_test_cm")]
struct TestCmArgs {
    #[from_args(pos_only, default)]
    behavior: Option<Value>,
    #[from_args(pos_only, default)]
    payload: Option<Value>,
}

/// Applies the chosen behavior to `cm`, consuming the payload (if any).
///
/// Caller already copied the behavior string out of the original `Value`
/// (which is inside a `defer_drop!` guard), so we take a borrow here.
fn configure(
    cm: &mut TestContextManager,
    behavior: &str,
    payload: Option<Value>,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    match behavior {
        "suppress" => {
            if let Some(p) = payload {
                p.drop_with_heap(vm);
                return Err(ExcType::type_error("_test_cm('suppress') takes no payload".to_owned()));
            }
            cm.suppress = true;
        }
        "enter_value" => {
            let Some(p) = payload else {
                return Err(ExcType::type_error(
                    "_test_cm('enter_value', n) requires an int payload".to_owned(),
                ));
            };
            defer_drop!(p, vm);
            let Value::Int(n) = p else {
                return Err(ExcType::type_error(format!(
                    "_test_cm('enter_value', n) requires int payload, not {}",
                    p.py_type(vm)
                )));
            };
            cm.enter_value = Some(*n);
        }
        "raise_on_enter" => {
            cm.raise_on_enter = Some(extract_str_payload("raise_on_enter", payload, vm)?);
        }
        "raise_on_exit" => {
            cm.raise_on_exit = Some(extract_str_payload("raise_on_exit", payload, vm)?);
        }
        other => {
            if let Some(p) = payload {
                p.drop_with_heap(vm);
            }
            return Err(ExcType::type_error(format!("_test_cm() unknown behavior '{other}'")));
        }
    }
    Ok(())
}

/// Extracts a required string payload, surfacing CPython-style TypeErrors
/// for missing/wrong-typed payloads.
fn extract_str_payload(
    behavior: &str,
    payload: Option<Value>,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<String> {
    let Some(p) = payload else {
        return Err(ExcType::type_error(format!(
            "_test_cm('{behavior}', msg) requires a str payload"
        )));
    };
    defer_drop!(p, vm);
    let Some(s) = value_as_owned_str(p, vm) else {
        return Err(ExcType::type_error(format!(
            "_test_cm('{behavior}', msg) requires str payload, not {}",
            p.py_type(vm)
        )));
    };
    Ok(s)
}

/// Reads the underlying string out of a `Value`, copying to an owned `String`.
///
/// Used here (rather than `Value::as_either_str`) because we need a freshly
/// owned `String` for storage on `TestContextManager`, not a borrow tied to
/// the heap / interns. Returns `None` for non-string `Value` variants.
fn value_as_owned_str(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> Option<String> {
    match value {
        Value::InternString(id) => Some(vm.interns.get_str(*id).to_owned()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str().to_owned()),
            _ => None,
        },
        _ => None,
    }
}
