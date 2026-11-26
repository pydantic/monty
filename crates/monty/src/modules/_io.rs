//! The `io` module: the in-memory streams, the file classes `open()` returns,
//! the seek constants and the io exceptions.
//!
//! `StringIO` and `BytesIO` live in `types::io`. `TextIOWrapper` and the
//! `Buffered*` classes are the types of `open()` files; they are exported so
//! `isinstance` checks work, but cannot be constructed directly. Monty keeps no
//! host file descriptors, so nothing here deals in them (see `limitations/io.md`).

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    builtins::{Builtins, BuiltinsFunctions, open::builtin_open},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropGuard,
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{Module, Type, str::allocate_string},
    value::Value,
};

/// CPython's `io.DEFAULT_BUFFER_SIZE`; nothing in Monty buffers by it.
const DEFAULT_BUFFER_SIZE: i64 = 128 * 1024;

/// The module's functions.
#[monty_macros::explicit_discriminants(checksum = 0x2661_6f66_92f9_7e2f)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum IoFunctions {
    OpenCode = 0,
    TextEncoding = 1,
}

/// Creates the `io` module.
pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Io, vm.interns);
    for (name, ty) in [
        (StaticStrings::StringIo, Type::StringIo),
        (StaticStrings::BytesIo, Type::BytesIo),
        (StaticStrings::TextIoWrapper, Type::TextIOWrapper),
        (StaticStrings::BufferedReader, Type::BufferedReader),
        (StaticStrings::BufferedWriter, Type::BufferedWriter),
        (StaticStrings::BufferedRandom, Type::BufferedRandom),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::Type(ty)), vm);
    }
    for (name, ty) in [
        (StaticStrings::UnsupportedOperation, Type::UnsupportedOperation),
        (StaticStrings::BlockingIoError, Type::BlockingIOError),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::Type(ty)), vm);
    }
    for (name, function) in [
        (StaticStrings::OpenCode, IoFunctions::OpenCode),
        (StaticStrings::TextEncoding, IoFunctions::TextEncoding),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Io(function)), vm);
    }
    module.set_attr(
        StaticStrings::Open,
        Value::Builtin(Builtins::Function(BuiltinsFunctions::Open)),
        vm,
    );
    for (name, value) in [
        (StaticStrings::SeekSet, 0),
        (StaticStrings::SeekCur, 1),
        (StaticStrings::SeekEnd, 2),
        (StaticStrings::DefaultBufferSize, DEFAULT_BUFFER_SIZE),
    ] {
        module.set_attr(name, Value::Int(value), vm);
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

/// `io.open_code(path)`: `open(path, 'rb')` for a `str` path, as CPython's
/// default hook does.
fn open_code(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let OpenCodeArgs { path } = OpenCodeArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let path = allocate_string(path.as_str(vm), vm.heap);
    builtin_open(vm, ArgValues::Two(path, allocate_string("rb", vm.heap)))
}

/// `io.text_encoding(encoding, stacklevel=2, /)`: `encoding` unless it is
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
        Value::None => allocate_string("locale", vm.heap),
        encoding => encoding,
    })
}
