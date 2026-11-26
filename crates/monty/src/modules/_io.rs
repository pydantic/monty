//! The `_io` module, CPython's C half of `io`: the stream classes, `open`,
//! `open_code`, `text_encoding`, the io exceptions and `DEFAULT_BUFFER_SIZE`.
//!
//! `StringIO` and `BytesIO` live in `types::io`. `TextIOWrapper` and the
//! `Buffered*` classes are the types of `open()` files; they are exported so
//! `isinstance` checks work, but cannot be constructed directly. Monty keeps no
//! host file descriptors, so nothing here deals in them (see `limitations/io.md`).
//! CPython's `FileIO`, `BufferedRWPair`, `IncrementalNewlineDecoder` and the
//! `_IOBase` classes are absent.

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    builtins::{Builtins, BuiltinsFunctions, open::builtin_open},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropGuard,
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{Module, Type},
    value::Value,
};

/// CPython's `DEFAULT_BUFFER_SIZE`; nothing in Monty buffers by it.
const DEFAULT_BUFFER_SIZE: i64 = 128 * 1024;

/// The module's functions.
#[monty_macros::explicit_discriminants(checksum = 0x2661_6f66_92f9_7e2f)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum IoFunctions {
    OpenCode = 0,
    TextEncoding = 1,
}

/// Creates the `_io` module. The vendored `io.py` imports every name it defines.
pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::UnderIo, vm.interns);
    let ty = |ty| Value::Builtin(Builtins::Type(ty));
    let function = |function| Value::ModuleFunction(ModuleFunctions::Io(function));
    for (name, value) in [
        (StaticStrings::DefaultBufferSize, Value::Int(DEFAULT_BUFFER_SIZE)),
        (StaticStrings::BlockingIoError, ty(Type::BlockingIOError)),
        (StaticStrings::UnsupportedOperation, ty(Type::UnsupportedOperation)),
        (
            StaticStrings::Open,
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Open)),
        ),
        (StaticStrings::OpenCode, function(IoFunctions::OpenCode)),
        (StaticStrings::TextEncoding, function(IoFunctions::TextEncoding)),
        (StaticStrings::StringIo, ty(Type::StringIo)),
        (StaticStrings::BytesIo, ty(Type::BytesIo)),
        (StaticStrings::TextIoWrapper, ty(Type::TextIOWrapper)),
        (StaticStrings::BufferedReader, ty(Type::BufferedReader)),
        (StaticStrings::BufferedWriter, ty(Type::BufferedWriter)),
        (StaticStrings::BufferedRandom, ty(Type::BufferedRandom)),
    ] {
        module.set_attr(name, value, vm);
    }
    module
}

/// Dispatches a call to one of the module's functions.
pub(super) fn call(vm: &mut VM<'_>, function: IoFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        IoFunctions::OpenCode => open_code(vm, args),
        IoFunctions::TextEncoding => text_encoding(vm, args).map(CallResult::Value),
    }
}

/// `open_code(path)`.
#[derive(FromArgs)]
#[from_args(name = "open_code", style = c_named, bad_arg_named)]
struct OpenCodeArgs {
    path: StrArg,
}

/// `_io.open_code(path)`: `open(path, 'rb')` for a `str` path, as CPython's
/// default hook does.
fn open_code(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let OpenCodeArgs { path } = OpenCodeArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let path = Value::from_str(path.as_str(vm), vm.heap);
    builtin_open(vm, ArgValues::Two(path, Value::from_str("rb", vm.heap)))
}

/// `_io.text_encoding(encoding, stacklevel=2, /)`: `encoding` unless it is
/// `None`, then `'locale'`, which `open()` accepts as Monty's UTF-8.
fn text_encoding(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let (encoding, stacklevel) = args
        .reject_kwargs("_io.text_encoding", vm.heap)?
        .get_one_two_args("text_encoding", vm.heap)?;
    let mut encoding_guard = DropGuard::new(encoding, vm);
    if let Some(stacklevel) = stacklevel {
        let (_, vm) = encoding_guard.as_parts_mut();
        defer_drop!(stacklevel, vm);
        // `stacklevel` is a C `int`, so an `int` past `i64` and one past `i32` raise alike.
        let stacklevel = stacklevel.as_int_with_overflow(vm, ExcType::overflow_c_int)?;
        i32::try_from(stacklevel).map_err(|_| ExcType::overflow_c_int())?;
    }
    let (encoding, vm) = encoding_guard.into_parts();
    Ok(match encoding {
        Value::None => Value::from_str("locale", vm.heap),
        encoding => encoding,
    })
}
