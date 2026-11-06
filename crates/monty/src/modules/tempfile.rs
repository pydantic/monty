//! Named temporary files in explicitly supplied host storage.

use monty_types::{CreateExclusiveCallArgs, FileMode, MontyPath, OsFunctionCall, normalize_virtual_path};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    builtins::{
        Builtins,
        open::{newline_argument, validate_ignored_open_kwarg},
    },
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        random::{RandomFunctions, random_dispatch},
    },
    os_dispatch::{PostConversionEffect, value_as_path_str},
    types::{
        Module, PyTrait,
        file::{FileName, Newline},
        random::RandomTarget,
        str::allocate_string,
    },
    value::Value,
    virtual_path::posix_join,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum TempfileFunctions {
    #[strum(serialize = "NamedTemporaryFile")]
    NamedTemporaryFile,
    Gettempdir,
    Gettempprefix,
}

/// Creates the module without initializing random state or accessing storage.
pub(crate) fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Tempfile, vm.interns);
    for (name, function) in [
        (StaticStrings::NamedTemporaryFile, TempfileFunctions::NamedTemporaryFile),
        (StaticStrings::Gettempdir, TempfileFunctions::Gettempdir),
        (StaticStrings::Gettempprefix, TempfileFunctions::Gettempprefix),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Tempfile(function)), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches the supported tempfile functions.
pub(super) fn call(vm: &mut VM<'_>, function: TempfileFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        TempfileFunctions::NamedTemporaryFile => named_file(vm, args),
        TempfileFunctions::Gettempdir | TempfileFunctions::Gettempprefix => {
            args.check_zero_args(
                match function {
                    TempfileFunctions::Gettempdir => "gettempdir",
                    _ => "gettempprefix",
                },
                vm.heap,
            )?;
            Ok(CallResult::Value(allocate_string(
                match function {
                    TempfileFunctions::Gettempdir => "/tmp",
                    _ => "tmp",
                },
                vm.heap,
            )))
        }
    }
}

#[derive(FromArgs)]
#[from_args(name = "NamedTemporaryFile", style = def)]
struct NamedArgs {
    #[from_args(default)]
    mode: Option<StrArg>,
    #[from_args(default = Value::Int(-1))]
    buffering: Value,
    #[from_args(default = Value::None)]
    encoding: Value,
    #[from_args(default = Value::None)]
    newline: Value,
    #[from_args(default = Value::None)]
    suffix: Value,
    #[from_args(default = Value::None)]
    prefix: Value,
    #[from_args(default = Value::None)]
    dir: Value,
    #[from_args(default = Value::Bool(true))]
    delete: Value,
    #[from_args(kw_only, default = Value::None)]
    errors: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    delete_on_close: Value,
}

/// Validates options before the frozen retry loop can request entropy or create a file.
fn named_file(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let NamedArgs {
        mode,
        buffering,
        encoding,
        newline,
        suffix,
        prefix,
        dir,
        delete,
        errors,
        delete_on_close,
    } = NamedArgs::from_args(args, vm)?;
    defer_drop!(mode, vm);
    let values = [
        buffering,
        encoding,
        newline,
        suffix,
        prefix,
        dir,
        delete,
        errors,
        delete_on_close,
    ];
    defer_drop!(values, vm);
    let [
        buffering,
        encoding,
        newline,
        suffix,
        prefix,
        dir,
        delete,
        errors,
        delete_on_close,
    ] = values;
    let mode = mode
        .as_ref()
        .map_or("w+b", |m| m.as_str(vm))
        .parse::<FileMode>()
        .map_err(ExcType::value_error)?;
    if !mode.writable() {
        return Err(ExcType::value_error(
            "NamedTemporaryFile requires a write or append mode",
        ));
    }
    if vm.env.legacy_binary_writes && matches!(mode, FileMode::WriteUpdate(true)) {
        return Err(ExcType::not_implemented("binary update mode requires positional filesystem writes").into());
    }
    let delete = delete.py_bool(vm)?;
    if delete && delete_on_close.py_bool(vm)? {
        return Err(ExcType::value_error(
            "NamedTemporaryFile requires delete_on_close=False when delete=True",
        ));
    }
    validate_ignored_open_kwarg("buffering", buffering, vm)?;
    validate_ignored_open_kwarg("encoding", encoding, vm)?;
    validate_ignored_open_kwarg("errors", errors, vm)?;
    let newline_text = newline_argument(newline, vm)?;
    if mode.is_binary() && newline_text.is_some() {
        return Err(ExcType::value_error("binary mode doesn't take a newline argument"));
    }
    if let Some(text) = &newline_text
        && Newline::parse(text).is_none()
    {
        return Err(ExcType::value_error(format!("illegal newline value: {text}")));
    }
    let prefix = name_part(prefix, "prefix", "tmp", vm)?;
    let suffix = name_part(suffix, "suffix", "", vm)?;
    let dir = if matches!(dir, Value::None) {
        "/tmp".to_owned()
    } else {
        value_as_path_str(dir, vm.heap, vm.interns)
            .ok_or_else(|| ExcType::type_error("dir must be str or pathlib.Path"))?
            .to_owned()
    };
    if dir.contains('\0') {
        return Err(ExcType::value_error("embedded null byte"));
    }
    // Empty relative directories refer to cwd; the absolute name survives later chdir calls.
    let dir = if dir.is_empty() {
        vm.env.cwd.to_string()
    } else {
        posix_join(&vm.env.cwd, &dir)
    };
    let dir = normalize_virtual_path(&dir).into_owned();
    let args = [
        allocate_string(dir, vm.heap),
        allocate_string(prefix, vm.heap),
        allocate_string(suffix, vm.heap),
        allocate_string(mode.as_str(), vm.heap),
        newline.clone_with_heap(vm.heap),
        Value::Bool(delete),
        Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::TemporaryBits)),
        Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::CreateTemporaryFile)),
        Value::Builtin(Builtins::ExcType(ExcType::FileExistsError)),
    ];
    vm.call_frozen_exact(FrozenFunction::NamedTemporaryFile, args)
}

/// Prefixes and suffixes are filename fragments, never paths.
fn name_part(value: &Value, name: &str, default: &str, vm: &VM<'_>) -> RunResult<String> {
    if matches!(value, Value::None) {
        return Ok(default.to_owned());
    }
    let text = value
        .as_str(vm)
        .ok_or_else(|| ExcType::type_error(format!("{name} must be str")))?;
    if text.contains(['/', '\0']) {
        return Err(ExcType::value_error(format!(
            "{name} must not contain '/' or null bytes"
        )));
    }
    Ok(text.to_owned())
}

/// Draws from a separate stream using the existing suspendable entropy machinery.
pub(crate) fn call_bits(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    random_dispatch(RandomTarget::Temporary, RandomFunctions::Getrandbits, args, vm)
}

#[derive(FromArgs)]
#[from_args(name = "_create_temporary_file", style = def)]
struct CreateArgs {
    path: StrArg,
    mode: StrArg,
    newline: Value,
    delete: bool,
}

/// Reserves one candidate and attaches lifecycle state while importing its handle.
pub(crate) fn call_create(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let CreateArgs {
        path,
        mode,
        newline,
        delete,
    } = CreateArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    defer_drop!(mode, vm);
    defer_drop!(newline, vm);
    let path = path.as_str(vm).to_owned();
    let mode = mode.as_str(vm).parse::<FileMode>().map_err(ExcType::value_error)?;
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::CreateExclusive(CreateExclusiveCallArgs {
            path: MontyPath::new(path.clone()),
            mode,
            permissions: 0o600,
        }),
        effect: PostConversionEffect::OpenName {
            name: FileName::Str(path),
            newline: newline_argument(newline, vm)?,
            delete_on_exit: delete,
        }
        .into(),
    })
}

/// A normal frame retains the retry count and candidate across host suspensions.
pub(crate) const CREATE_SOURCE: &str = r"
def _named_file(directory, prefix, suffix, mode, newline, delete, bits, create, exists_error):
    alphabet = 'abcdefghijklmnopqrstuvwxyz0123456789_'
    attempt = 0
    while attempt < 20:
        attempt += 1
        name = ''
        i = 0
        while i < 8:
            i += 1
            index = bits(6)
            while index >= 37:
                index = bits(6)
            name += alphabet[index]
        path = directory.rstrip('/') + '/' + prefix + name + suffix
        try:
            return create(path, mode, newline, delete)
        except exists_error:
            pass
    raise exists_error('No usable temporary filename after 20 attempts')
";
