//! The `.py` file search behind an `import` call the session's
//! `external_modules` do not answer: each candidate file is read through the
//! `os=` handler first, then the feed's mounts, and the first file found is
//! sent back for the sandbox to run as the module.

use monty_pool::{ResumeValue, TurnEvent};
use monty_proto::python::InstanceStore;
use monty_types::{
    CallArgs, ExtFunctionResult, IMPORT_CALL, ImportArgs, ModuleFile, ModuleRead, MontyPath, OsFunctionCall,
};
use pyo3::{exceptions::PyRuntimeError, prelude::*};

use crate::{
    async_dispatch::python_future,
    callback_context::CallbackContext,
    external::{py_err_to_ext_result, py_obj_to_ext_result},
    get_not_handled,
    pool::{OsDispatch, SharedCheckout, block_on_sync, dispatch_os_parts, pool_err_to_py},
};

/// [`import_search`] for a drive loop's suspension event.
pub(crate) fn module_import_search(event: &TurnEvent) -> Option<ImportArgs> {
    match event {
        TurnEvent::OsCall {
            function_name, args, ..
        } => import_search(function_name, args),
        _ => None,
    }
}

/// The search an `import` call asks for, when it has a search path: a call
/// from a sandbox predating file imports has none, and gets no file.
pub(crate) fn import_search(function_name: &str, args: &CallArgs) -> Option<ImportArgs> {
    (function_name == IMPORT_CALL)
        .then(|| ImportArgs::from_call_args(args))
        .flatten()
        .filter(|search| !search.search_path.is_empty())
}

/// Searches for `search`'s file for the sync `Monty` drives. An async `os=`
/// handler raises, as for any other OS call outside `AsyncMonty`.
/// Call with the callback context entered, as for `os=` calls.
pub(crate) fn find_import_file_sync(
    py: Python<'_>,
    search: &ImportArgs,
    os: Option<&Py<PyAny>>,
    instances: &InstanceStore,
    checkout: &SharedCheckout,
) -> PyResult<ResumeValue> {
    for path in search.candidates() {
        let read = match dispatch_os_parts(py, READ_TEXT, &read_text_args(&path), os, instances, false) {
            OsDispatch::Answer(answer) => read_from_answer(answer),
            OsDispatch::Coroutine(coro) => {
                // Closed so CPython does not warn that it was never awaited.
                coro.bind(py).call_method0("close")?;
                return Err(PyRuntimeError::new_err("async os callbacks require AsyncMonty"));
            }
        };
        let read = match read {
            ModuleRead::NotHandled => py.detach(|| block_on_sync(read_from_mounts(checkout, &path)))??,
            read => read,
        };
        if let Some(answer) = search_answer(path, read) {
            return Ok(answer);
        }
    }
    Ok(ResumeValue::NotFound)
}

/// Searches for `search`'s file for the `AsyncMonty` drives, awaiting an
/// async `os=` handler's answer in place. Each handler call runs under
/// `callback_context`; the awaits and mount reads run outside it.
pub(crate) async fn find_import_file_async(
    search: &ImportArgs,
    os: Option<&Py<PyAny>>,
    instances: &InstanceStore,
    checkout: &SharedCheckout,
    callback_context: &CallbackContext,
    native: &opentelemetry::Context,
) -> PyResult<ResumeValue> {
    for path in search.candidates() {
        let dispatched = Python::attach(|py| {
            let _guard = callback_context.enter(py, native)?;
            PyResult::Ok(dispatch_os_parts(
                py,
                READ_TEXT,
                &read_text_args(&path),
                os,
                instances,
                true,
            ))
        })?;
        let read = match dispatched {
            OsDispatch::Answer(answer) => read_from_answer(answer),
            OsDispatch::Coroutine(coro) => read_from_coroutine(coro, instances).await?,
        };
        let read = match read {
            ModuleRead::NotHandled => read_from_mounts(checkout, &path).await?,
            read => read,
        };
        if let Some(answer) = search_answer(path, read) {
            return Ok(answer);
        }
    }
    Ok(ResumeValue::NotFound)
}

/// The OS call a candidate file is read with, as `open()`'s handler sees it.
const READ_TEXT: &str = "Path.read_text";

/// The arguments of a `Path.read_text` of `path`, as the sandbox would send them.
fn read_text_args(path: &str) -> CallArgs {
    OsFunctionCall::ReadText(MontyPath::new(path.to_owned())).to_args()
}

/// Classifies the `os=` handler's answer to reading a candidate.
fn read_from_answer(answer: ResumeValue) -> ModuleRead {
    match answer {
        ResumeValue::Return(value) => ModuleRead::from_result(ExtFunctionResult::Return(value)),
        ResumeValue::Error(exc) => ModuleRead::from_exception(exc),
        ResumeValue::NotHandled | ResumeValue::NotFound => ModuleRead::NotHandled,
        ResumeValue::Future | ResumeValue::ModuleFile(_) => {
            unreachable!("an os= handler answers with a value, an exception or NOT_HANDLED")
        }
    }
}

/// Awaits an async `os=` handler's answer to reading a candidate, which may
/// decline it with `NOT_HANDLED` as a sync handler would.
async fn read_from_coroutine(coro: Py<PyAny>, instances: &InstanceStore) -> PyResult<ModuleRead> {
    let answer = python_future(coro)?.await;
    Python::attach(|py| {
        Ok(match answer {
            Ok(value) if value.is(get_not_handled(py)?) => ModuleRead::NotHandled,
            Ok(value) => ModuleRead::from_result(py_obj_to_ext_result(value.bind(py), instances)),
            Err(err) => ModuleRead::from_result(py_err_to_ext_result(py, &err)),
        })
    })
}

/// Reads a candidate through the feed's mounts once the `os=` handler declined it.
async fn read_from_mounts(checkout: &SharedCheckout, path: &str) -> PyResult<ModuleRead> {
    let mut guard = checkout.lock().await;
    let Some(checkout) = guard.as_mut() else {
        return Ok(ModuleRead::NotHandled);
    };
    let read = checkout.read_from_mounts(path).await;
    read.map_err(|err| Python::attach(|py| pool_err_to_py(py, err)))
}

/// The import's answer once a candidate is read, or `None` to read the next.
fn search_answer(path: String, read: ModuleRead) -> Option<ResumeValue> {
    match read {
        ModuleRead::Text(source) => Some(ResumeValue::ModuleFile(ModuleFile { path, source })),
        ModuleRead::Error(exc) => Some(ResumeValue::Error(exc)),
        ModuleRead::Missing | ModuleRead::NotHandled => None,
    }
}
