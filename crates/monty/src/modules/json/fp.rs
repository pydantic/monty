//! The file-object side of `json.load()` and `json.dump()`.
//!
//! An `open()` handle is read and written through its own buffered-read and
//! one-shot-write machinery, so `json.load` on an unread file suspends to the
//! host exactly as `fp.read()` would. Any other object is used through its
//! `read` / `write` attribute, called synchronously (see `limitations/json.md`).

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, HeapId, HeapReadOutput},
    intern::StaticStrings,
    types::str::allocate_string,
    value::{EitherStr, Value},
};

/// Where `json.load` reads from or `json.dump` writes to.
pub(super) enum FileTarget {
    /// An `open()` handle; the id is borrowed from the caller's `fp` value.
    OpenFile(HeapId),
    /// `fp.read` / `fp.write` of any other object, owned until called.
    Method(Value),
}

impl FileTarget {
    /// Resolves `fp` for `json.load`.
    pub(super) fn reader(fp: &Value, vm: &mut VM<'_>) -> RunResult<Self> {
        Self::resolve(fp, StaticStrings::Read, vm)
    }

    /// Resolves `fp` for `json.dump`, running the checks `fp.write(str)` makes
    /// before reaching the host (text mode, open, writable; a callable `write`
    /// otherwise) so they are raised before the document is encoded, as
    /// CPython's first chunk would.
    pub(super) fn writer(fp: &Value, vm: &mut VM<'_>) -> RunResult<Self> {
        match Self::resolve(fp, StaticStrings::Write, vm)? {
            Self::OpenFile(file_id) => {
                if let HeapReadOutput::OpenFile(file) = vm.heap.read(file_id) {
                    file.check_text_write(vm)?;
                }
                Ok(Self::OpenFile(file_id))
            }
            Self::Method(write) if write.is_callable(vm) => Ok(Self::Method(write)),
            Self::Method(write) => {
                defer_drop!(write, vm);
                Err(ExcType::type_error_not_callable_object(&write.py_type_name(vm)))
            }
        }
    }

    /// `fp.write(text)` for `json.dump`: the host write for an `open()` handle
    /// (answering `None`), or a synchronous call of the `write` method.
    pub(super) fn write(self, text: String, vm: &mut VM<'_>) -> RunResult<CallResult> {
        match self {
            Self::OpenFile(file_id) => {
                let HeapReadOutput::OpenFile(mut file) = vm.heap.read(file_id) else {
                    unreachable!("FileTarget::OpenFile points to an OpenFile")
                };
                file.write_text_discarding_count(vm, text)
            }
            Self::Method(write) => {
                defer_drop!(write, vm);
                let chunk = allocate_string(text, vm.heap);
                let returned = vm.evaluate_function(write, ArgValues::One(chunk))?;
                returned.drop_with(vm);
                Ok(CallResult::Value(Value::None))
            }
        }
    }

    /// `fp` as an `open()` handle, else its `method` attribute: CPython's
    /// `AttributeError` names the type when it is missing.
    fn resolve(fp: &Value, method: StaticStrings, vm: &mut VM<'_>) -> RunResult<Self> {
        if let Value::Ref(id) = fp
            && matches!(vm.heap.read(*id), HeapReadOutput::OpenFile(_))
        {
            Ok(Self::OpenFile(*id))
        } else {
            let attr = EitherStr::Interned(vm.interns.intern_static(method));
            match fp.py_getattr(&attr, vm)? {
                CallResult::Value(callable) => Ok(Self::Method(callable)),
                // A host object's attribute would need a round trip the module
                // function cannot wait for.
                suspended => {
                    suspended.drop_with(vm);
                    Err(SimpleException::new_msg(
                        ExcType::NotImplementedError,
                        format!("json: `{}` of a host object is not supported", attr.as_str(vm.interns)),
                    )
                    .into())
                }
            }
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for FileTarget {
    fn drop_with(self, heap: &mut C) {
        if let Self::Method(callable) = self {
            callable.drop_with(heap);
        }
    }
}
