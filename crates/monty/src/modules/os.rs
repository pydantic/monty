//! Implementation of the `os` module.
//!
//! Environment access (`getenv`, `environ`), filesystem wrappers (`listdir`,
//! `stat`, `mkdir`, `makedirs`, `remove`, `unlink`, `rmdir`, `rename`,
//! `replace`), the pure `fspath`, and the POSIX path constants (`sep`,
//! `linesep`, `name`, ...). The sandbox always presents a POSIX view
//! regardless of host OS, so the constants are fixed.
//!
//! Filesystem functions never touch the host directly: they yield an
//! [`OsFunctionCall`] (the same variants `pathlib.Path` methods use) for the
//! host to permit or reject. `os.listdir` reuses the `Iterdir` call and
//! reduces the returned child paths to bare names via
//! [`PendingOsEffect::ListdirNames`] (see `os_dispatch::listdir_names`).
//!
//! `dir_fd` / `follow_symlinks` keyword arguments are parsed for signature
//! parity but rejected with the `NotImplementedError` CPython raises on
//! platforms without them — Monty never supports fd-relative paths.

use monty_types::{GetenvArgs, MkdirCallArgs, MontyObject, MontyPath, OsFunctionCall, RenameCallArgs};

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{HeapData, HeapId},
    intern::{StaticStrings, StringId},
    modules::ModuleFunctions,
    object_bridge::MontyObjectExt,
    os_dispatch::{PendingOsEffect, value_to_owned_string},
    types::{Module, Property, Type, property::ZeroArgOsProperty, str::allocate_string},
    value::Value,
};

/// OS module functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum OsFunctions {
    Getenv,
    Listdir,
    Stat,
    Mkdir,
    Makedirs,
    Remove,
    Unlink,
    Rmdir,
    Rename,
    Replace,
    Fspath,
}

/// Creates the `os` module and allocates it on the heap.
///
/// Functions yield to the host via `OsFunction` callbacks (except the pure
/// `fspath`); constants are fixed POSIX values since the sandbox path model
/// is always POSIX.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    /// Shorthand for the function-attribute entries in the table below.
    fn function(f: OsFunctions) -> Value {
        Value::ModuleFunction(ModuleFunctions::Os(f))
    }

    let attrs = [
        // Callables — each dispatches through `ModuleFunctions::Os`.
        (StaticStrings::Getenv, function(OsFunctions::Getenv)),
        (StaticStrings::Listdir, function(OsFunctions::Listdir)),
        (StaticStrings::StatMethod, function(OsFunctions::Stat)),
        (StaticStrings::Mkdir, function(OsFunctions::Mkdir)),
        (StaticStrings::Makedirs, function(OsFunctions::Makedirs)),
        (StaticStrings::Remove, function(OsFunctions::Remove)),
        (StaticStrings::Unlink, function(OsFunctions::Unlink)),
        (StaticStrings::Rmdir, function(OsFunctions::Rmdir)),
        (StaticStrings::Rename, function(OsFunctions::Rename)),
        (StaticStrings::Replace, function(OsFunctions::Replace)),
        (StaticStrings::OsFspath, function(OsFunctions::Fspath)),
        // os.environ — property that yields the host environment as a dict.
        (
            StaticStrings::Environ,
            Value::Property(Property::Os(ZeroArgOsProperty::GetEnviron)),
        ),
        // POSIX path constants — the sandbox path model is POSIX on every host.
        (StaticStrings::Sep, Value::InternString(StringId::from_ascii(b'/'))),
        (StaticStrings::Altsep, Value::None),
        (StaticStrings::Extsep, Value::InternString(StringId::from_ascii(b'.'))),
        (StaticStrings::Curdir, Value::InternString(StringId::from_ascii(b'.'))),
        (StaticStrings::Pardir, StaticStrings::ParentDirString.into()),
        (StaticStrings::Linesep, Value::InternString(StringId::from_ascii(b'\n'))),
        (StaticStrings::Name, StaticStrings::Posix.into()),
        (StaticStrings::Devnull, StaticStrings::DevNullString.into()),
    ];

    let mut module = Module::new(StaticStrings::Os);
    for (attr, value) in attrs {
        module.set_attr(attr, value, vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to an os module function.
///
/// Returns `CallResult::OsCall` for functions that need host involvement,
/// or `CallResult::Value` for functions that can be computed immediately.
pub(super) fn call(vm: &mut VM<'_>, functions: OsFunctions, args: ArgValues) -> RunResult<CallResult> {
    match functions {
        OsFunctions::Getenv => getenv(vm, args),
        OsFunctions::Listdir => listdir(vm, args),
        OsFunctions::Stat => stat(vm, args),
        OsFunctions::Mkdir => mkdir(vm, args),
        OsFunctions::Makedirs => makedirs(vm, args),
        OsFunctions::Remove => remove(vm, args),
        OsFunctions::Unlink => unlink(vm, args),
        OsFunctions::Rmdir => rmdir(vm, args),
        OsFunctions::Rename => rename(vm, args),
        OsFunctions::Replace => replace(vm, args),
        OsFunctions::Fspath => fspath(vm, args),
    }
}

/// Implementation of `os.getenv(key, default=None)`.
///
/// Hand-rolled rather than `FromArgs`-derived so the `key`-must-be-`str`
/// error matches CPython's wording exactly — CPython routes `os.getenv`
/// through `os.environ.__getitem__`, whose `check_str` helper raises
/// `TypeError("str expected, not <type>")`. That wording is bespoke to
/// `os._Environ` in the CPython stdlib, so it lives inline here rather
/// than as a shared helper.
fn getenv(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let (key_value, default_value) = args.get_one_two_args("os.getenv", vm.heap)?;
    if let Some(key) = key_value.as_either_str(vm.heap) {
        key_value.drop_with(vm.heap);
        Ok(CallResult::OsCall(OsFunctionCall::Getenv(GetenvArgs {
            key: key.into_string(vm.interns),
            default: MontyObject::new(default_value.unwrap_or(Value::None), vm),
        })))
    } else {
        let type_name = key_value.py_type_name_heap(vm.heap, vm.interns);
        key_value.drop_with(vm.heap);

        if let Some(d) = default_value {
            d.drop_with(vm.heap);
        }
        Err(ExcType::type_error(format!("str expected, not {type_name}")))
    }
}

/// `os.listdir(path=None)` argument shape — clinic-parsed with the
/// `at_most_total` pre-count (`listdir() takes at most 1 argument (2 given)`).
#[derive(FromArgs)]
#[from_args(name = "listdir", at_most_total)]
struct ListdirArgs {
    #[from_args(default = Value::None)]
    path: Value,
}

/// Implementation of `os.listdir(path=None)`.
///
/// Reuses the `Iterdir` OS call (host returns full child paths) with a
/// [`PendingOsEffect::ListdirNames`] — armed at dispatch, not here — so the
/// resume reduces them to bare entry names. `None` maps to `'.'` like
/// CPython; Monty has no working directory, so hosts typically reject the
/// relative default.
fn listdir(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ListdirArgs { path } = ListdirArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let path = if matches!(path, Value::None) {
        MontyPath::from(".")
    } else {
        extract_os_path(path, "listdir", "path", PathAccepts::FdOrNone, vm)?
    };
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::Iterdir(path),
        effect: PendingOsEffect::ListdirNames,
    })
}

/// `os.stat(path, *, dir_fd=None, follow_symlinks=True)` argument shape.
/// `follow_symlinks` uses [`LaxBool`] — CPython truth-tests it, so any value
/// is accepted.
#[derive(FromArgs)]
#[from_args(name = "stat", style = c_named)]
struct StatArgs {
    path: Value,
    #[from_args(kw_only, default = Value::None)]
    dir_fd: Value,
    #[from_args(kw_only, default = LaxBool::new(true))]
    follow_symlinks: LaxBool,
}

/// Implementation of `os.stat(path)` — yields the same `Stat` OS call as
/// `Path.stat()`. `follow_symlinks=False` (lstat) is not supported.
fn stat(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let StatArgs {
        path,
        dir_fd,
        follow_symlinks,
    } = StatArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    defer_drop!(dir_fd, vm);
    let path = extract_os_path(path, "stat", "path", PathAccepts::Fd, vm)?;
    check_dir_fd(dir_fd, vm)?;
    if follow_symlinks.bool() {
        Ok(CallResult::OsCall(OsFunctionCall::Stat(path)))
    } else {
        Err(ExcType::not_implemented_os_arg(Some("stat"), "follow_symlinks"))
    }
}

/// `os.mkdir(path, mode=0o777, *, dir_fd=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "mkdir", style = c_named)]
struct MkdirArgs {
    path: Value,
    #[from_args(default = Value::Int(0o777))]
    mode: Value,
    #[from_args(kw_only, default = Value::None)]
    dir_fd: Value,
}

/// Implementation of `os.mkdir(path, mode=0o777)` — single level, no
/// exist-tolerance. `mode` is validated but ignored: Monty's filesystem
/// backends do not model POSIX permission bits.
fn mkdir(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let MkdirArgs { path, mode, dir_fd } = MkdirArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    defer_drop!(mode, vm);
    defer_drop!(dir_fd, vm);
    let path = extract_os_path(path, "mkdir", "path", PathAccepts::NoFd, vm)?;
    check_mode(mode, vm)?;
    check_dir_fd(dir_fd, vm)?;
    Ok(CallResult::OsCall(OsFunctionCall::Mkdir(MkdirCallArgs {
        path,
        parents: false,
        exist_ok: false,
    })))
}

/// `os.makedirs(name, mode=0o777, exist_ok=False)` argument shape — a pure
/// Python `def` in CPython, hence `style = def`. `exist_ok` uses [`LaxBool`]
/// (truth-tested, never type-checked) like the sibling `PathMkdirArgs`.
#[derive(FromArgs)]
#[from_args(name = "makedirs", style = def)]
struct MakedirsArgs {
    name: Value,
    #[from_args(default = Value::Int(0o777))]
    mode: Value,
    #[from_args(default = LaxBool::new(false))]
    exist_ok: LaxBool,
}

/// Implementation of `os.makedirs(name, mode=0o777, exist_ok=False)` —
/// recursive `mkdir` via the `parents=True` flag on the `Mkdir` OS call.
fn makedirs(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let MakedirsArgs { name, mode, exist_ok } = MakedirsArgs::from_args(args, vm)?;
    defer_drop!(name, vm);
    defer_drop!(mode, vm);
    // CPython's `makedirs` body starts with `path.split(name)`, whose
    // `fspath` call produces this wording (not the `mkdir` converter's).
    let path = extract_path(name, vm, ExcType::type_error_fspath)?;
    check_mode(mode, vm)?;
    Ok(CallResult::OsCall(OsFunctionCall::Mkdir(MkdirCallArgs {
        path,
        parents: true,
        exist_ok: exist_ok.bool(),
    })))
}

/// `os.remove(path, *, dir_fd=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "remove", style = c_named)]
struct RemoveArgs {
    path: Value,
    #[from_args(kw_only, default = Value::None)]
    dir_fd: Value,
}

/// Implementation of `os.remove(path)` — yields the same `Unlink` OS call as
/// `Path.unlink()`.
fn remove(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let RemoveArgs { path, dir_fd } = RemoveArgs::from_args(args, vm)?;
    single_path_call(path, dir_fd, "remove", OsFunctionCall::Unlink, vm)
}

/// `os.unlink(path, *, dir_fd=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "unlink", style = c_named)]
struct UnlinkArgs {
    path: Value,
    #[from_args(kw_only, default = Value::None)]
    dir_fd: Value,
}

/// Implementation of `os.unlink(path)` — alias of [`remove`] with its own
/// name in error messages.
fn unlink(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let UnlinkArgs { path, dir_fd } = UnlinkArgs::from_args(args, vm)?;
    single_path_call(path, dir_fd, "unlink", OsFunctionCall::Unlink, vm)
}

/// Shared body of [`remove`] / [`unlink`] / [`rmdir`]: validate the path and
/// `dir_fd`, then build the OS call via `make_call`.
fn single_path_call(
    path: Value,
    dir_fd: Value,
    func: &'static str,
    make_call: impl FnOnce(MontyPath) -> OsFunctionCall,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    defer_drop!(path, vm);
    defer_drop!(dir_fd, vm);
    let path = extract_os_path(path, func, "path", PathAccepts::NoFd, vm)?;
    check_dir_fd(dir_fd, vm)?;
    Ok(CallResult::OsCall(make_call(path)))
}

/// `os.rmdir(path, *, dir_fd=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "rmdir", style = c_named)]
struct RmdirArgs {
    path: Value,
    #[from_args(kw_only, default = Value::None)]
    dir_fd: Value,
}

/// Implementation of `os.rmdir(path)` — yields the same `Rmdir` OS call as
/// `Path.rmdir()`.
fn rmdir(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let RmdirArgs { path, dir_fd } = RmdirArgs::from_args(args, vm)?;
    single_path_call(path, dir_fd, "rmdir", OsFunctionCall::Rmdir, vm)
}

/// `os.rename(src, dst, *, src_dir_fd=None, dst_dir_fd=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "rename", style = c_named)]
struct RenameArgs {
    src: Value,
    dst: Value,
    #[from_args(kw_only, default = Value::None)]
    src_dir_fd: Value,
    #[from_args(kw_only, default = Value::None)]
    dst_dir_fd: Value,
}

/// Implementation of `os.rename(src, dst)`.
fn rename(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let RenameArgs {
        src,
        dst,
        src_dir_fd,
        dst_dir_fd,
    } = RenameArgs::from_args(args, vm)?;
    rename_like(src, dst, src_dir_fd, dst_dir_fd, "rename", vm)
}

/// `os.replace(src, dst, *, src_dir_fd=None, dst_dir_fd=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "replace", style = c_named)]
struct ReplaceArgs {
    src: Value,
    dst: Value,
    #[from_args(kw_only, default = Value::None)]
    src_dir_fd: Value,
    #[from_args(kw_only, default = Value::None)]
    dst_dir_fd: Value,
}

/// Implementation of `os.replace(src, dst)` — same `Rename` OS call as
/// [`rename`]; overwrite behavior is whatever the host backend does.
fn replace(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ReplaceArgs {
        src,
        dst,
        src_dir_fd,
        dst_dir_fd,
    } = ReplaceArgs::from_args(args, vm)?;
    rename_like(src, dst, src_dir_fd, dst_dir_fd, "replace", vm)
}

/// Shared body of [`rename`] / [`replace`]: validate both endpoints and the
/// fd keywords (type errors per-fd first, then the combined
/// `NotImplementedError`, matching CPython's `internal_rename` ordering).
fn rename_like(
    src: Value,
    dst: Value,
    src_dir_fd: Value,
    dst_dir_fd: Value,
    func: &'static str,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    defer_drop!(src, vm);
    defer_drop!(dst, vm);
    defer_drop!(src_dir_fd, vm);
    defer_drop!(dst_dir_fd, vm);
    let src = extract_os_path(src, func, "src", PathAccepts::NoFd, vm)?;
    let dst = extract_os_path(dst, func, "dst", PathAccepts::NoFd, vm)?;
    // Non-short-circuiting `|`: both fds are type-checked before either is
    // rejected as unsupported, matching CPython's converter ordering.
    if dir_fd_specified(src_dir_fd, vm)? | dir_fd_specified(dst_dir_fd, vm)? {
        Err(ExcType::not_implemented_os_arg(Some(func), "src_dir_fd and dst_dir_fd"))
    } else {
        Ok(CallResult::OsCall(OsFunctionCall::Rename(RenameCallArgs { src, dst })))
    }
}

/// `os.fspath(path)` argument shape — clinic-parsed with the `at_most_total`
/// pre-count (`fspath() takes at most 1 argument (2 given)`).
#[derive(FromArgs)]
#[from_args(name = "fspath", style = c_named, at_most_total)]
struct FspathArgs {
    path: Value,
}

/// Implementation of `os.fspath(path)` — pure, no host involvement: `str` and
/// `bytes` pass through unchanged, `Path` yields its string form.
fn fspath(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let FspathArgs { path } = FspathArgs::from_args(args, vm)?;
    match path.py_type_heap(vm.heap) {
        Type::Str | Type::Bytes => Ok(CallResult::Value(path)),
        Type::Path => {
            let text = value_to_owned_string(&path, vm.heap, vm.interns).expect("Path always yields a string");
            path.drop_with(vm.heap);
            Ok(CallResult::Value(allocate_string(text, vm.heap)))
        }
        _ => {
            let type_name = path.py_type_name_heap(vm.heap, vm.interns).into_owned();
            path.drop_with(vm.heap);
            Err(ExcType::type_error_fspath(&type_name))
        }
    }
}

/// Extracts a virtual path from a `str`/`Path` value for an os function,
/// raising the `path_t` converter TypeError
/// (`{func}: {arg} should be {accepted}, not {type}`) for anything else.
fn extract_os_path(
    value: &Value,
    func: &'static str,
    arg: &'static str,
    accepts: PathAccepts,
    vm: &VM<'_>,
) -> RunResult<MontyPath> {
    extract_path(value, vm, |type_name| {
        ExcType::type_error_os_path(func, arg, accepts.phrase_for(value, vm), type_name)
    })
}

/// Which `path_t` converter kinds an `os` function accepts in CPython.
///
/// Drives the accepted-types phrase in the converter TypeError. Monty takes
/// only `str`/`Path`, so the phrase has two forms: CPython's verbatim (so
/// types CPython also rejects get a byte-identical message) and a narrowed
/// one used when the rejected value is a kind CPython *would* have taken —
/// otherwise the error would list the very type it just refused.
#[derive(Clone, Copy)]
enum PathAccepts {
    /// `path_t(allow_fd=False)` — `mkdir`, `remove`, `unlink`, `rmdir`, `rename`.
    NoFd,
    /// `path_t(allow_fd=True)` — `stat`.
    Fd,
    /// `path_t(nullable=True, allow_fd=True)` — `listdir`.
    FdOrNone,
}

impl PathAccepts {
    /// The accepted-types phrase for a value this converter is rejecting.
    fn phrase_for(self, value: &Value, vm: &VM<'_>) -> &'static str {
        // `bytes` paths and integer fds (bools included — CPython fd-converts
        // them with only a RuntimeWarning) are the kinds CPython takes and
        // Monty never will; everything else keeps CPython's wording exactly.
        let refused = match value.py_type_heap(vm.heap) {
            Type::Bytes => true,
            Type::Int | Type::Bool => matches!(self, Self::Fd | Self::FdOrNone),
            _ => false,
        };
        match (self, refused) {
            (Self::NoFd, false) => "string, bytes or os.PathLike",
            (Self::Fd, false) => "string, bytes, os.PathLike or integer",
            (Self::FdOrNone, false) => "string, bytes, os.PathLike, integer or None",
            (Self::NoFd | Self::Fd, true) => "string or os.PathLike",
            (Self::FdOrNone, true) => "string, os.PathLike or None",
        }
    }
}

/// Core `str`/`Path` → [`MontyPath`] extraction; `type_error` supplies the
/// per-callsite wording (the `path_t` converter's or `os.fspath`'s).
fn extract_path(value: &Value, vm: &VM<'_>, type_error: impl FnOnce(&str) -> RunError) -> RunResult<MontyPath> {
    value_to_owned_string(value, vm.heap, vm.interns).map_or_else(
        || Err(type_error(&value.py_type_name_heap(vm.heap, vm.interns))),
        |path| Ok(MontyPath::new(path)),
    )
}

/// Validates a `dir_fd` keyword for the single-fd functions: `None` passes,
/// anything specified is rejected — ints with CPython's platform
/// `NotImplementedError`, other types with the converter TypeError.
fn check_dir_fd(value: &Value, vm: &VM<'_>) -> RunResult<()> {
    if dir_fd_specified(value, vm)? {
        Err(ExcType::not_implemented_os_arg(None, "dir_fd"))
    } else {
        Ok(())
    }
}

/// Type-checks a `dir_fd`-shaped value like CPython's `dir_fd_converter`
/// (`argument should be integer or None, not {type}`; out-of-C-int-range ints
/// raise the fd `OverflowError`), returning whether a (necessarily
/// unsupported) fd was actually supplied.
fn dir_fd_specified(value: &Value, vm: &VM<'_>) -> RunResult<bool> {
    match value {
        Value::None => Ok(false),
        Value::Bool(_) => Ok(true),
        Value::Int(fd) => match i32::try_from(*fd) {
            Ok(_) => Ok(true),
            Err(_) if *fd > 0 => Err(ExcType::overflow_fd_maximum()),
            Err(_) => Err(ExcType::overflow_fd_minimum()),
        },
        _ => match value.py_type_heap(vm.heap) {
            // Big ints (`LongInt`) are outside i64, so far outside C int.
            Type::Int if value.long_int_is_negative(vm) => Err(ExcType::overflow_fd_minimum()),
            Type::Int => Err(ExcType::overflow_fd_maximum()),
            other => Err(ExcType::type_error_dir_fd(&other.name(vm.heap, vm.interns))),
        },
    }
}

/// Validates a `mode` argument as an int fitting C `int`, raising CPython's
/// `'{type}' object cannot be interpreted as an integer` / the C-int
/// `OverflowError`. The value itself is ignored — Monty does not model POSIX
/// permission bits.
fn check_mode(value: &Value, vm: &VM<'_>) -> RunResult<()> {
    match value {
        Value::Bool(_) => Ok(()),
        Value::Int(mode) => i32::try_from(*mode).map(|_| ()).map_err(|_| ExcType::overflow_c_int()),
        _ => match value.py_type_heap(vm.heap) {
            // Big ints (`LongInt`) are outside i64, so far outside C int.
            Type::Int => Err(ExcType::overflow_c_int()),
            other => Err(ExcType::type_error_not_integer(&other.name(vm.heap, vm.interns))),
        },
    }
}
