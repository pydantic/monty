//! Implementation of the `os.path` module — CPython's `posixpath`, which is
//! also what `import posixpath` yields.
//!
//! The sandbox path model is POSIX on every host, so the pure functions
//! (`join`, `split`, `normpath`, `relpath`, ...) are the `posixpath`
//! algorithms, implemented on bytes in [`posix`] so `str` and `bytes`
//! arguments share one code path. The predicates (`exists`, `isfile`,
//! `isdir`, `islink`) yield the same host calls as the `pathlib.Path`
//! methods, the stat getters (`getsize`, `getmtime`, ...) yield `Path.stat`
//! and pick one field on resume, `realpath` yields `Path.resolve`, and
//! `expanduser` / `expandvars` ask the host for `$HOME` / the environment
//! only when the path actually needs them.

use std::iter;

use monty_types::{GetenvArgs, MontyObject, MontyPath, OsFunctionCall};
use smallvec::smallvec;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    builtins::{Builtins, BuiltinsFunctions, candidate_wins},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{StaticStrings, StringId},
    modules::{
        ModuleFunctions,
        os::{PathAccepts, extract_os_path},
    },
    os_dispatch::{PreConversionEffect, StatField, value_to_owned_bytes, value_to_owned_string},
    types::{Bytes, Module, PyTrait, Slice, Type, allocate_tuple, collect_iterable, str::allocate_string},
    value::Value,
};

pub(crate) mod posix;

/// `os.path` module functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum OsPathFunctions {
    Abspath,
    Basename,
    Commonpath,
    Commonprefix,
    Dirname,
    Exists,
    Expanduser,
    Expandvars,
    Getatime,
    Getctime,
    Getmtime,
    Getsize,
    Isabs,
    Isdevdrive,
    Isdir,
    Isfile,
    Isjunction,
    Islink,
    Ismount,
    Join,
    Lexists,
    Normcase,
    Normpath,
    Realpath,
    Relpath,
    Samefile,
    Samestat,
    Split,
    Splitdrive,
    Splitext,
    Splitroot,
}

/// Creates the `os.path` module (named `posixpath`, as CPython's is) and
/// allocates it on the heap. `os.create_module` hangs one off `os.path`;
/// `import os.path` / `import posixpath` build their own.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    /// Shorthand for the function-attribute entries in the table below.
    fn function(f: OsPathFunctions) -> Value {
        Value::ModuleFunction(ModuleFunctions::OsPath(f))
    }

    let attrs = [
        (StaticStrings::Abspath, function(OsPathFunctions::Abspath)),
        (StaticStrings::Basename, function(OsPathFunctions::Basename)),
        (StaticStrings::Commonpath, function(OsPathFunctions::Commonpath)),
        (StaticStrings::Commonprefix, function(OsPathFunctions::Commonprefix)),
        (StaticStrings::Dirname, function(OsPathFunctions::Dirname)),
        (StaticStrings::Exists, function(OsPathFunctions::Exists)),
        (StaticStrings::Expanduser, function(OsPathFunctions::Expanduser)),
        (StaticStrings::Expandvars, function(OsPathFunctions::Expandvars)),
        (StaticStrings::Getatime, function(OsPathFunctions::Getatime)),
        (StaticStrings::Getctime, function(OsPathFunctions::Getctime)),
        (StaticStrings::Getmtime, function(OsPathFunctions::Getmtime)),
        (StaticStrings::Getsize, function(OsPathFunctions::Getsize)),
        (StaticStrings::Isabs, function(OsPathFunctions::Isabs)),
        (StaticStrings::Isdevdrive, function(OsPathFunctions::Isdevdrive)),
        (StaticStrings::Isdir, function(OsPathFunctions::Isdir)),
        (StaticStrings::Isfile, function(OsPathFunctions::Isfile)),
        (StaticStrings::Isjunction, function(OsPathFunctions::Isjunction)),
        (StaticStrings::Islink, function(OsPathFunctions::Islink)),
        (StaticStrings::Ismount, function(OsPathFunctions::Ismount)),
        (StaticStrings::Lexists, function(OsPathFunctions::Lexists)),
        (StaticStrings::Join, function(OsPathFunctions::Join)),
        (StaticStrings::Normcase, function(OsPathFunctions::Normcase)),
        (StaticStrings::Normpath, function(OsPathFunctions::Normpath)),
        (StaticStrings::Realpath, function(OsPathFunctions::Realpath)),
        (StaticStrings::Relpath, function(OsPathFunctions::Relpath)),
        (StaticStrings::Samefile, function(OsPathFunctions::Samefile)),
        (StaticStrings::Samestat, function(OsPathFunctions::Samestat)),
        (StaticStrings::Split, function(OsPathFunctions::Split)),
        (StaticStrings::Splitdrive, function(OsPathFunctions::Splitdrive)),
        (StaticStrings::Splitext, function(OsPathFunctions::Splitext)),
        (StaticStrings::Splitroot, function(OsPathFunctions::Splitroot)),
        // POSIX constants — the sandbox path model is POSIX on every host.
        (StaticStrings::Curdir, Value::InternString(StringId::from_ascii(b'.'))),
        (
            StaticStrings::Pardir,
            Value::InternString(vm.interns.intern_static(StaticStrings::ParentDirString)),
        ),
        (StaticStrings::Extsep, Value::InternString(StringId::from_ascii(b'.'))),
        (StaticStrings::Sep, Value::InternString(StringId::from_ascii(b'/'))),
        (StaticStrings::Pathsep, Value::InternString(StringId::from_ascii(b':'))),
        (
            StaticStrings::Defpath,
            Value::InternString(vm.interns.intern_static(StaticStrings::DefpathString)),
        ),
        (StaticStrings::Altsep, Value::None),
        (
            StaticStrings::Devnull,
            Value::InternString(vm.interns.intern_static(StaticStrings::DevNullString)),
        ),
        // CPython sets this from `sys.platform == 'darwin'`; the sandbox is never darwin.
        (StaticStrings::SupportsUnicodeFilenames, Value::Bool(false)),
    ];

    let mut module = Module::new(StaticStrings::Posixpath, vm.interns);
    for (attr, value) in attrs {
        module.set_attr(attr, value, vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to an `os.path` function.
pub(super) fn call(vm: &mut VM<'_>, functions: OsPathFunctions, args: ArgValues) -> RunResult<CallResult> {
    match functions {
        OsPathFunctions::Abspath => abspath(vm, args).map(CallResult::Value),
        OsPathFunctions::Basename => basename(vm, args).map(CallResult::Value),
        OsPathFunctions::Commonpath => commonpath(vm, args).map(CallResult::Value),
        OsPathFunctions::Commonprefix => commonprefix(vm, args).map(CallResult::Value),
        OsPathFunctions::Dirname => dirname(vm, args).map(CallResult::Value),
        OsPathFunctions::Exists => exists(vm, args),
        OsPathFunctions::Expanduser => expanduser(vm, args),
        OsPathFunctions::Expandvars => expandvars(vm, args),
        OsPathFunctions::Getatime => getatime(vm, args),
        OsPathFunctions::Getctime => getctime(vm, args),
        OsPathFunctions::Getmtime => getmtime(vm, args),
        OsPathFunctions::Getsize => getsize(vm, args),
        OsPathFunctions::Isabs => isabs(vm, args).map(CallResult::Value),
        OsPathFunctions::Isdevdrive => isdevdrive(vm, args).map(CallResult::Value),
        OsPathFunctions::Isdir => isdir(vm, args),
        OsPathFunctions::Isfile => isfile(vm, args),
        OsPathFunctions::Isjunction => isjunction(vm, args).map(CallResult::Value),
        OsPathFunctions::Islink => islink(vm, args),
        OsPathFunctions::Ismount => ismount(vm, args),
        OsPathFunctions::Lexists => lexists(vm, args),
        OsPathFunctions::Join => join(vm, args).map(CallResult::Value),
        OsPathFunctions::Normcase => normcase(vm, args).map(CallResult::Value),
        OsPathFunctions::Normpath => normpath(vm, args).map(CallResult::Value),
        OsPathFunctions::Realpath => realpath(vm, args),
        OsPathFunctions::Relpath => relpath(vm, args).map(CallResult::Value),
        OsPathFunctions::Samefile => samefile(vm, args),
        OsPathFunctions::Samestat => samestat(vm, args).map(CallResult::Value),
        OsPathFunctions::Split => split(vm, args).map(CallResult::Value),
        OsPathFunctions::Splitdrive => splitdrive(vm, args).map(CallResult::Value),
        OsPathFunctions::Splitext => splitext(vm, args).map(CallResult::Value),
        OsPathFunctions::Splitroot => splitroot(vm, args).map(CallResult::Value),
    }
}

// ============================================================================
// Pure functions
// ============================================================================

/// Declares the argument struct of a pure-Python `def f(param)` in
/// `posixpath` / `genericpath`, so missing-argument and keyword errors carry
/// CPython's function and parameter names.
macro_rules! one_path_arg {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $param:ident) => {
        $(#[$doc])*
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            $param: Value,
        }
    };
}

/// `os.path.join(a, *p)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "join")]
struct JoinArgs {
    a: Value,
    #[from_args(varargs)]
    p: Vec<Value>,
}

/// Implementation of `os.path.join(a, *p)`: one kind of path throughout.
/// Any failure re-inspects the arguments like `genericpath._check_arg_types`
/// (`a` after `os.fspath`, `*p` as passed), so the error names the first
/// non-path argument before complaining about mixed kinds.
fn join(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let JoinArgs { a, p } = JoinArgs::from_args(args, vm)?;
    defer_drop!(a, vm);
    defer_drop!(p, vm);
    let first = PathText::fspath(a, vm)?;
    let mut parts = Vec::with_capacity(p.len() + 1);
    parts.push(first);
    for part in p {
        match PathText::fspath(part, vm) {
            Ok(text) if text.is_bytes == parts[0].is_bytes => parts.push(text),
            _ => {
                let kinds = iter::once(parts[0].py_type()).chain(p.iter().map(|arg| arg.py_type_heap(vm.heap)));
                return Err(check_arg_types("join", kinds, vm));
            }
        }
    }
    let joined = posix::join(parts.iter().map(|part| part.bytes.as_slice()));
    Ok(parts[0].allocate(joined, vm.heap))
}

one_path_arg!(
    /// `os.path.split(p)` argument shape.
    SplitArgs,
    "split",
    p
);

/// Implementation of `os.path.split(p)`.
fn split(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SplitArgs { p } = SplitArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    let text = PathText::fspath(p, vm)?;
    let (head, tail) = posix::split(&text.bytes);
    Ok(text.allocate_pair(head, tail, vm.heap))
}

one_path_arg!(
    /// `os.path.splitext(p)` argument shape.
    SplitextArgs,
    "splitext",
    p
);

/// Implementation of `os.path.splitext(p)`.
fn splitext(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SplitextArgs { p } = SplitextArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    let text = PathText::fspath(p, vm)?;
    let (root, ext) = posix::splitext(&text.bytes);
    Ok(text.allocate_pair(root, ext, vm.heap))
}

one_path_arg!(
    /// `os.path.splitdrive(p)` argument shape.
    SplitdriveArgs,
    "splitdrive",
    p
);

/// Implementation of `os.path.splitdrive(p)`: POSIX paths have no drive.
fn splitdrive(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SplitdriveArgs { p } = SplitdriveArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    let text = PathText::fspath(p, vm)?;
    Ok(text.allocate_pair(b"", &text.bytes, vm.heap))
}

/// `os.path.splitroot(p)` argument shape — CPython's is the C
/// `posix._path_splitroot_ex`, whose clinic wording the errors keep.
#[derive(FromArgs)]
#[from_args(name = "_path_splitroot_ex", style = c_named, at_most_total)]
struct SplitrootArgs {
    p: Value,
}

/// Implementation of `os.path.splitroot(p)`: `(drive, root, tail)` with an
/// always-empty drive.
fn splitroot(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SplitrootArgs { p } = SplitrootArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    let text = PathText::extract(p, vm, |type_name| {
        ExcType::type_error_os_path("_path_splitroot_ex", "path", C_PATH_ACCEPTS, type_name)
    })?;
    let (root, tail) = posix::splitroot(&text.bytes);
    let items = smallvec![
        text.allocate(Vec::new(), vm.heap),
        text.allocate(root.to_vec(), vm.heap),
        text.allocate(tail.to_vec(), vm.heap),
    ];
    Ok(allocate_tuple(items, vm.heap))
}

one_path_arg!(
    /// `os.path.basename(p)` argument shape.
    BasenameArgs,
    "basename",
    p
);

/// Implementation of `os.path.basename(p)`.
fn basename(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let BasenameArgs { p } = BasenameArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    let text = PathText::fspath(p, vm)?;
    Ok(text.allocate(posix::basename(&text.bytes).to_vec(), vm.heap))
}

one_path_arg!(
    /// `os.path.dirname(p)` argument shape.
    DirnameArgs,
    "dirname",
    p
);

/// Implementation of `os.path.dirname(p)`.
fn dirname(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DirnameArgs { p } = DirnameArgs::from_args(args, vm)?;
    defer_drop!(p, vm);
    let text = PathText::fspath(p, vm)?;
    Ok(text.allocate(posix::dirname(&text.bytes).to_vec(), vm.heap))
}

one_path_arg!(
    /// `os.path.isabs(s)` argument shape.
    IsabsArgs,
    "isabs",
    s
);

/// Implementation of `os.path.isabs(s)`.
fn isabs(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IsabsArgs { s } = IsabsArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let text = PathText::fspath(s, vm)?;
    Ok(Value::Bool(posix::isabs(&text.bytes)))
}

one_path_arg!(
    /// `os.path.normcase(s)` argument shape.
    NormcaseArgs,
    "normcase",
    s
);

/// Implementation of `os.path.normcase(s)`: the identity on POSIX, after `os.fspath`.
fn normcase(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let NormcaseArgs { s } = NormcaseArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let text = PathText::fspath(s, vm)?;
    Ok(text.into_value(vm.heap))
}

/// `os.path.normpath(path)` argument shape — CPython's is the C
/// `posix._path_normpath`, whose clinic wording the errors keep.
#[derive(FromArgs)]
#[from_args(name = "_path_normpath", style = c_named, at_most_total)]
struct NormpathArgs {
    path: Value,
}

/// Implementation of `os.path.normpath(path)`.
fn normpath(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let NormpathArgs { path } = NormpathArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let text = PathText::extract(path, vm, |type_name| {
        ExcType::type_error_os_path("_path_normpath", "path", C_PATH_ACCEPTS, type_name)
    })?;
    Ok(text.allocate(posix::normpath(&text.bytes), vm.heap))
}

/// The accepted-types phrase of the C path converters behind `normpath` and
/// `splitroot`, which take no file descriptor.
const C_PATH_ACCEPTS: &str = "string, bytes or os.PathLike";

one_path_arg!(
    /// `os.path.abspath(path)` argument shape.
    AbspathArgs,
    "abspath",
    path
);

/// Implementation of `os.path.abspath(path)`: pure, against the sandbox's
/// virtual working directory.
fn abspath(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let AbspathArgs { path } = AbspathArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let text = PathText::fspath(path, vm)?;
    Ok(text.allocate(posix::abspath(vm.env.cwd.as_bytes(), &text.bytes), vm.heap))
}

/// `os.path.relpath(path, start=None)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "relpath", style = def)]
struct RelpathArgs {
    path: Value,
    #[from_args(default = Value::None)]
    start: Value,
}

/// Implementation of `os.path.relpath(path, start=None)`: pure, against the
/// sandbox's virtual working directory. `start=None` means `os.curdir`.
fn relpath(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let RelpathArgs { path, start } = RelpathArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    defer_drop!(start, vm);
    let path_text = PathText::fspath(path, vm)?;
    if path_text.bytes.is_empty() {
        return Err(ExcType::value_error("no path specified"));
    }
    let start_text = if matches!(start, Value::None) {
        PathText {
            bytes: b".".to_vec(),
            is_bytes: path_text.is_bytes,
        }
    } else {
        PathText::fspath(start, vm)?
    };
    // Both are past `os.fspath`, so CPython's `_check_arg_types` can only report a mix.
    if start_text.is_bytes != path_text.is_bytes {
        return Err(ExcType::type_error_mixed_path_components());
    }
    let relative = posix::relpath(vm.env.cwd.as_bytes(), &path_text.bytes, &start_text.bytes);
    Ok(path_text.allocate(relative, vm.heap))
}

one_path_arg!(
    /// `os.path.commonpath(paths)` argument shape.
    CommonpathArgs,
    "commonpath",
    paths
);

/// Implementation of `os.path.commonpath(paths)`: `os.fspath` over the
/// sequence first, so a non-path element fails before an empty sequence or
/// mixed kinds do.
fn commonpath(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CommonpathArgs { paths } = CommonpathArgs::from_args(args, vm)?;
    defer_drop!(paths, vm);
    let items = collect_iterable(paths, vm)?;
    defer_drop!(items, vm);
    let texts = items
        .iter()
        .map(|item| PathText::fspath(item, vm))
        .collect::<RunResult<Vec<_>>>()?;
    let Some(first) = texts.first() else {
        return Err(ExcType::value_error("commonpath() arg is an empty sequence"));
    };
    if texts.iter().any(|text| text.is_bytes != first.is_bytes) {
        return Err(ExcType::type_error_mixed_path_components());
    }
    let slices: Vec<&[u8]> = texts.iter().map(|text| text.bytes.as_slice()).collect();
    let common = posix::commonpath(&slices).map_err(ExcType::value_error)?;
    Ok(first.allocate(common, vm.heap))
}

one_path_arg!(
    /// `os.path.commonprefix(m)` argument shape.
    CommonprefixArgs,
    "commonprefix",
    m
);

/// Implementation of `genericpath.commonprefix(m)`: the longest common
/// leading slice of `min(m)` and `max(m)`. Elements are passed through
/// `os.fspath` unless `m[0]` is a list or tuple, in which case the elements
/// are compared as sequences and the prefix is a slice of the smallest one.
fn commonprefix(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CommonprefixArgs { m } = CommonprefixArgs::from_args(args, vm)?;
    defer_drop!(m, vm);
    if !m.py_bool(vm)? {
        return Ok(allocate_string("", vm.heap));
    }
    let first = m.py_getitem(&Value::Int(0), vm)?;
    defer_drop!(first, vm);
    let nested = matches!(first.py_type_heap(vm.heap), Type::List | Type::Tuple);
    let raw_items = collect_iterable(m, vm)?;
    defer_drop!(raw_items, vm);
    // Guarded before conversion so a later `os.fspath` failure releases the
    // values already converted.
    let items = Vec::with_capacity(raw_items.len());
    defer_drop_mut!(items, vm);
    for item in raw_items {
        items.push(if nested {
            item.clone_with_heap(vm)
        } else {
            PathText::fspath(item, vm)?.into_value(vm.heap)
        });
    }
    let smallest = extreme(items, true, vm)?;
    let largest = extreme(items, false, vm)?;
    common_leading_slice(smallest, largest, vm)
}

/// `min(items)` / `max(items)` by reference: the first item that no later
/// item strictly beats, with `min`'s own `<`-unsupported error.
fn extreme<'a>(items: &'a [Value], is_min: bool, vm: &mut VM<'_>) -> RunResult<&'a Value> {
    let mut best = &items[0];
    for item in &items[1..] {
        if candidate_wins(best, item, is_min, vm)? {
            best = item;
        }
    }
    Ok(best)
}

/// `s1[:i]` for the first `i` where `s1[i] != s2[i]`, or all of `s1`. Strings
/// slice by code point and bytes by byte; anything else is iterated and
/// indexed like CPython does it (so a dict's int keys raise `TypeError`),
/// with `==` on the elements and a slice of `s1` as the result.
fn common_leading_slice(s1: &Value, s2: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    match (s1.py_type_heap(vm.heap), s2.py_type_heap(vm.heap)) {
        (Type::Str, Type::Str) => {
            let a = value_to_owned_string(s1, vm.heap, vm.interns).expect("checked str");
            let b = value_to_owned_string(s2, vm.heap, vm.interns).expect("checked str");
            let end = a
                .char_indices()
                .zip(b.chars())
                .find(|((_, x), y)| x != y)
                .map_or(a.len(), |((i, _), _)| i);
            Ok(allocate_string(&a[..end], vm.heap))
        }
        (Type::Bytes, Type::Bytes) => {
            let a = value_to_owned_bytes(s1, vm.heap, vm.interns).expect("checked bytes");
            let b = value_to_owned_bytes(s2, vm.heap, vm.interns).expect("checked bytes");
            let end = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len());
            Ok(Value::Ref(
                vm.heap.allocate(HeapData::Bytes(Bytes::new(a[..end].to_vec()))),
            ))
        }
        _ => {
            let elements = collect_iterable(s1, vm)?;
            defer_drop!(elements, vm);
            let mut end = elements.len();
            for (i, element) in elements.iter().enumerate() {
                let other = s2.py_getitem(&Value::Int(i64::try_from(i).expect("sequence index fits i64")), vm)?;
                defer_drop!(other, vm);
                if !element.py_eq_operator(other, vm)? {
                    end = i;
                    break;
                }
            }
            let stop = i64::try_from(end).expect("sequence index fits i64");
            let slice = Value::Ref(vm.heap.allocate(HeapData::Slice(Slice::new(None, Some(stop), None))));
            defer_drop!(slice, vm);
            s1.py_getitem(slice, vm)
        }
    }
}

one_path_arg!(
    /// `os.path.isjunction(path)` argument shape.
    IsjunctionArgs,
    "isjunction",
    path
);

/// Implementation of `os.path.isjunction(path)`: junctions are a Windows
/// concept, so this is `False` once `os.fspath` accepts the argument.
fn isjunction(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IsjunctionArgs { path } = IsjunctionArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    PathText::fspath(path, vm)?;
    Ok(Value::Bool(false))
}

one_path_arg!(
    /// `os.path.isdevdrive(path)` argument shape.
    IsdevdriveArgs,
    "isdevdrive",
    path
);

/// Implementation of `os.path.isdevdrive(path)`: Dev Drives are a Windows
/// concept, so this is `False` once `os.fspath` accepts the argument.
fn isdevdrive(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let IsdevdriveArgs { path } = IsdevdriveArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    PathText::fspath(path, vm)?;
    Ok(Value::Bool(false))
}

/// `os.path.samestat(s1, s2)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "samestat", style = def)]
struct SamestatArgs {
    s1: Value,
    s2: Value,
}

/// Implementation of `os.path.samestat(s1, s2)`:
/// `s1.st_ino == s2.st_ino and s1.st_dev == s2.st_dev`, evaluated in that
/// order so a missing attribute is reported as CPython reports it.
fn samestat(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let SamestatArgs { s1, s2 } = SamestatArgs::from_args(args, vm)?;
    defer_drop!(s1, vm);
    defer_drop!(s2, vm);
    let same =
        attributes_equal(s1, s2, StaticStrings::StIno, vm)? && attributes_equal(s1, s2, StaticStrings::StDev, vm)?;
    Ok(Value::Bool(same))
}

/// `a.attr == b.attr` for a plain data attribute.
fn attributes_equal(a: &Value, b: &Value, attr: StaticStrings, vm: &mut VM<'_>) -> RunResult<bool> {
    let x = data_attribute(a, attr, vm)?;
    defer_drop!(x, vm);
    let y = data_attribute(b, attr, vm)?;
    defer_drop!(y, vm);
    x.py_eq_operator(y, vm)
}

/// `getattr(value, attr)` through the synchronous call path, so any
/// stat-like object works; a lazy host attribute reads as absent there.
fn data_attribute(value: &Value, attr: StaticStrings, vm: &mut VM<'_>) -> RunResult<Value> {
    let name = Value::InternString(vm.interns.intern_static(attr));
    let getattr = Value::Builtin(Builtins::Function(BuiltinsFunctions::Getattr));
    vm.evaluate_function(
        "os.path.samestat",
        &getattr,
        ArgValues::Two(value.clone_with_heap(vm), name),
    )
}

// ============================================================================
// Host-backed functions
// ============================================================================

one_path_arg!(
    /// `os.path.exists(path)` argument shape.
    ExistsArgs,
    "exists",
    path
);

/// Implementation of `os.path.exists(path)` — the `Path.exists` host call.
fn exists(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ExistsArgs { path } = ExistsArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    predicate(path, "stat", PathAccepts::Fd, OsFunctionCall::Exists, vm)
}

one_path_arg!(
    /// `os.path.isfile(path)` argument shape.
    IsfileArgs,
    "isfile",
    path
);

/// Implementation of `os.path.isfile(path)` — the `Path.is_file` host call.
fn isfile(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let IsfileArgs { path } = IsfileArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    predicate(path, "stat", PathAccepts::Fd, OsFunctionCall::IsFile, vm)
}

one_path_arg!(
    /// `os.path.isdir(s)` argument shape.
    IsdirArgs,
    "isdir",
    s
);

/// Implementation of `os.path.isdir(s)` — the `Path.is_dir` host call.
fn isdir(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let IsdirArgs { s } = IsdirArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    predicate(s, "stat", PathAccepts::Fd, OsFunctionCall::IsDir, vm)
}

one_path_arg!(
    /// `os.path.islink(path)` argument shape.
    IslinkArgs,
    "islink",
    path
);

/// Implementation of `os.path.islink(path)` — the `Path.is_symlink` host
/// call. CPython reaches it through `os.lstat`, whose converter takes no fd.
fn islink(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let IslinkArgs { path } = IslinkArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    predicate(path, "lstat", PathAccepts::NoFd, OsFunctionCall::IsSymlink, vm)
}

one_path_arg!(
    /// `os.path.ismount(path)` argument shape.
    IsmountArgs,
    "ismount",
    path
);

/// Implementation of `os.path.ismount(path)`: every existing path counts as a
/// mount point, so this is the `Path.exists` host call. The sandbox cannot
/// see where the host's mounts begin, and answering `True` is what keeps the
/// usual "walk up until a mount point" loops terminating.
fn ismount(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let IsmountArgs { path } = IsmountArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    predicate(path, "lstat", PathAccepts::NoFd, OsFunctionCall::Exists, vm)
}

one_path_arg!(
    /// `os.path.lexists(path)` argument shape.
    LexistsArgs,
    "lexists",
    path
);

/// Implementation of `os.path.lexists(path)`: `Path.exists`, and when that is
/// `False`, `Path.is_symlink` (a dangling symlink exists without a target);
/// see [`PreConversionEffect::Lexists`].
fn lexists(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let LexistsArgs { path } = LexistsArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let path = extract_os_path(path, "lstat", "path", PathAccepts::NoFd, vm)?;
    if path.is_empty() {
        Ok(CallResult::Value(Value::Bool(false)))
    } else {
        Ok(CallResult::OsCallWithEffect {
            effect: PreConversionEffect::Lexists {
                path: path.as_str().to_owned(),
            }
            .into(),
            call: OsFunctionCall::Exists(path),
        })
    }
}

/// `os.path.samefile(f1, f2)` argument shape.
#[derive(FromArgs)]
#[from_args(name = "samefile", style = def)]
struct SamefileArgs {
    f1: Value,
    f2: Value,
}

/// Implementation of `os.path.samefile(f1, f2)`: `Path.stat` on each in turn,
/// then `samestat` on the replies (see [`PreConversionEffect::SamefileFirst`]).
/// `f2` is extracted now but its errors are deferred to after the first
/// stat, which is when CPython's `os.stat(f2)` would raise them.
fn samefile(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let SamefileArgs { f1, f2 } = SamefileArgs::from_args(args, vm)?;
    defer_drop!(f1, vm);
    defer_drop!(f2, vm);
    let first = extract_os_path(f1, "stat", "path", PathAccepts::Fd, vm)?;
    if first.is_empty() {
        return Err(ExcType::file_not_found_error(""));
    }
    let second = match value_to_owned_string(f2, vm.heap, vm.interns) {
        Some(second) => Ok(second),
        None => Err((
            PathAccepts::Fd.phrase_for(f2, vm).to_owned(),
            f2.py_type_name_heap(vm.heap, vm.interns).into_owned(),
        )),
    };
    Ok(CallResult::OsCallWithEffect {
        effect: PreConversionEffect::SamefileFirst {
            first: first.as_str().to_owned(),
            second,
        }
        .into(),
        call: OsFunctionCall::Stat(first),
    })
}

/// Shared body of the predicates: the `os.stat` / `os.lstat` converter
/// error for a bad type, `False` for the empty path (which CPython's `stat`
/// fails on without the host's help), else the host call. A NUL byte is
/// answered `False` by the VM before the call leaves the sandbox.
fn predicate(
    path: &Value,
    func: &'static str,
    accepts: PathAccepts,
    make_call: impl FnOnce(MontyPath) -> OsFunctionCall,
    vm: &VM<'_>,
) -> RunResult<CallResult> {
    let path = extract_os_path(path, func, "path", accepts, vm)?;
    if path.is_empty() {
        Ok(CallResult::Value(Value::Bool(false)))
    } else {
        Ok(CallResult::OsCall(make_call(path)))
    }
}

one_path_arg!(
    /// `os.path.getsize(filename)` argument shape.
    GetsizeArgs,
    "getsize",
    filename
);

/// Implementation of `os.path.getsize(filename)`: `os.stat(filename).st_size`.
fn getsize(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let GetsizeArgs { filename } = GetsizeArgs::from_args(args, vm)?;
    defer_drop!(filename, vm);
    stat_field(filename, StatField::Size, vm)
}

one_path_arg!(
    /// `os.path.getmtime(filename)` argument shape.
    GetmtimeArgs,
    "getmtime",
    filename
);

/// Implementation of `os.path.getmtime(filename)`: `os.stat(filename).st_mtime`.
fn getmtime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let GetmtimeArgs { filename } = GetmtimeArgs::from_args(args, vm)?;
    defer_drop!(filename, vm);
    stat_field(filename, StatField::Mtime, vm)
}

one_path_arg!(
    /// `os.path.getatime(filename)` argument shape.
    GetatimeArgs,
    "getatime",
    filename
);

/// Implementation of `os.path.getatime(filename)`: `os.stat(filename).st_atime`.
fn getatime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let GetatimeArgs { filename } = GetatimeArgs::from_args(args, vm)?;
    defer_drop!(filename, vm);
    stat_field(filename, StatField::Atime, vm)
}

one_path_arg!(
    /// `os.path.getctime(filename)` argument shape.
    GetctimeArgs,
    "getctime",
    filename
);

/// Implementation of `os.path.getctime(filename)`: `os.stat(filename).st_ctime`.
fn getctime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let GetctimeArgs { filename } = GetctimeArgs::from_args(args, vm)?;
    defer_drop!(filename, vm);
    stat_field(filename, StatField::Ctime, vm)
}

/// Shared body of the stat getters: the `Path.stat` host call with a
/// [`PreConversionEffect::StatField`] that keeps one field of the reply. The
/// empty path fails as CPython's `stat('')` does, without consulting the host.
fn stat_field(path: &Value, field: StatField, vm: &VM<'_>) -> RunResult<CallResult> {
    let path = extract_os_path(path, "stat", "path", PathAccepts::Fd, vm)?;
    if path.is_empty() {
        Err(ExcType::file_not_found_error(""))
    } else {
        Ok(CallResult::OsCallWithEffect {
            call: OsFunctionCall::Stat(path),
            effect: PreConversionEffect::StatField(field).into(),
        })
    }
}

/// `os.path.realpath(filename, *, strict=False)` argument shape. `strict` is
/// truth-tested like CPython's, hence [`LaxBool`].
#[derive(FromArgs)]
#[from_args(name = "realpath", style = def)]
struct RealpathArgs {
    filename: Value,
    #[from_args(kw_only, default = LaxBool::new(false))]
    strict: LaxBool,
}

/// Implementation of `os.path.realpath(filename)` — the `Path.resolve` host
/// call, with a [`PreConversionEffect::ResolvedPath`] turning the host's path
/// reply into `str`. The empty path resolves to the working directory without
/// a call, as CPython's does. `strict=True` is refused: no host call can
/// promise the existence check it demands.
fn realpath(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let RealpathArgs { filename, strict } = RealpathArgs::from_args(args, vm)?;
    defer_drop!(filename, vm);
    let text = PathText::fspath(filename, vm)?;
    if text.is_bytes {
        // The host boundary takes `str` paths only, like every `os` function.
        let accepted = PathAccepts::NoFd.phrase_for(filename, vm);
        return Err(ExcType::type_error_os_path("lstat", "path", accepted, "bytes"));
    }
    if text.bytes.is_empty() {
        Ok(CallResult::Value(allocate_string(&*vm.env.cwd, vm.heap)))
    } else {
        let path = String::from_utf8(text.bytes).expect("str input");
        Ok(CallResult::OsCallWithEffect {
            call: OsFunctionCall::Resolve(MontyPath::new(path)),
            effect: PreConversionEffect::ResolvedPath { strict: strict.bool() }.into(),
        })
    }
}

one_path_arg!(
    /// `os.path.expanduser(path)` argument shape.
    ExpanduserArgs,
    "expanduser",
    path
);

/// Implementation of `os.path.expanduser(path)`. Only a leading `~` (bare or
/// followed by `/`) needs the host: it becomes an `os.getenv('HOME')` call
/// with a [`PreConversionEffect::ExpandUser`] splicing the answer in. `~user`
/// needs the password database, which the sandbox has no access to, so it is
/// returned unchanged as CPython does for an unknown user.
fn expanduser(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ExpanduserArgs { path } = ExpanduserArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let text = PathText::fspath(path, vm)?;
    if posix::tilde_end(&text.bytes) == Some(1) {
        Ok(CallResult::OsCallWithEffect {
            call: OsFunctionCall::Getenv(GetenvArgs {
                key: "HOME".to_owned(),
                default: MontyObject::none(),
            }),
            effect: PreConversionEffect::ExpandUser {
                tail: text.bytes[1..].to_vec(),
                is_bytes: text.is_bytes,
            }
            .into(),
        })
    } else {
        Ok(CallResult::Value(text.into_value(vm.heap)))
    }
}

one_path_arg!(
    /// `os.path.expandvars(path)` argument shape.
    ExpandvarsArgs,
    "expandvars",
    path
);

/// Implementation of `os.path.expandvars(path)`. A path without `$` is
/// returned as is; otherwise the host's `os.environ` is fetched and a
/// [`PreConversionEffect::ExpandVars`] performs the substitution on resume.
fn expandvars(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ExpandvarsArgs { path } = ExpandvarsArgs::from_args(args, vm)?;
    defer_drop!(path, vm);
    let text = PathText::fspath(path, vm)?;
    if text.bytes.contains(&b'$') {
        Ok(CallResult::OsCallWithEffect {
            call: OsFunctionCall::GetEnviron,
            effect: PreConversionEffect::ExpandVars {
                path: text.bytes,
                is_bytes: text.is_bytes,
            }
            .into(),
        })
    } else {
        Ok(CallResult::Value(text.into_value(vm.heap)))
    }
}

// ============================================================================
// Argument plumbing
// ============================================================================

/// A path argument after `os.fspath`: its bytes, and whether it arrived as
/// `bytes` — results are built with the same type the argument had.
struct PathText {
    bytes: Vec<u8>,
    is_bytes: bool,
}

impl PathText {
    /// `os.fspath(value)`: `str` and `Path` give text, `bytes` gives bytes,
    /// anything else raises the error `type_error` builds from its type name.
    fn extract(value: &Value, vm: &VM<'_>, type_error: impl FnOnce(&str) -> RunError) -> RunResult<Self> {
        if let Some(text) = value_to_owned_string(value, vm.heap, vm.interns) {
            Ok(Self {
                bytes: text.into_bytes(),
                is_bytes: false,
            })
        } else if let Some(bytes) = value_to_owned_bytes(value, vm.heap, vm.interns) {
            Ok(Self { bytes, is_bytes: true })
        } else {
            Err(type_error(&value.py_type_name_heap(vm.heap, vm.interns)))
        }
    }

    /// [`Self::extract`] with `os.fspath`'s own error wording.
    fn fspath(value: &Value, vm: &VM<'_>) -> RunResult<Self> {
        Self::extract(value, vm, ExcType::type_error_fspath)
    }

    /// The type `os.fspath` produced, for `check_arg_types`.
    fn py_type(&self) -> Type {
        if self.is_bytes { Type::Bytes } else { Type::Str }
    }

    /// Allocates `bytes` as the type this argument arrived with.
    fn allocate(&self, bytes: Vec<u8>, heap: &Heap) -> Value {
        allocate_text(bytes, self.is_bytes, heap)
    }

    /// Allocates the argument itself, unchanged, as the type it arrived with.
    fn into_value(self, heap: &Heap) -> Value {
        allocate_text(self.bytes, self.is_bytes, heap)
    }

    /// A two-tuple of [`Self::allocate`]d results, for `split` and friends.
    fn allocate_pair(&self, first: &[u8], second: &[u8], heap: &Heap) -> Value {
        let items = smallvec![
            self.allocate(first.to_vec(), heap),
            self.allocate(second.to_vec(), heap)
        ];
        allocate_tuple(items, heap)
    }
}

/// `bytes` as a `bytes` value, or as the `str` it was sliced from.
fn allocate_text(bytes: Vec<u8>, is_bytes: bool, heap: &Heap) -> Value {
    if is_bytes {
        Value::Ref(heap.allocate(HeapData::Bytes(Bytes::new(bytes))))
    } else {
        let text = String::from_utf8(bytes).expect("path algorithms split at ASCII separators, preserving UTF-8");
        allocate_string(text, heap)
    }
}

/// `genericpath._check_arg_types`: the error for a path operation that
/// failed on its arguments, given their types. The first that is neither
/// `str` nor `bytes` is named, else the arguments mixed `str` and `bytes`.
/// Callers pass the type `os.fspath` produced for the arguments CPython
/// rebinds before the check, and the raw type (a `Path` included) otherwise.
fn check_arg_types(func: &str, kinds: impl IntoIterator<Item = Type>, vm: &VM<'_>) -> RunError {
    for kind in kinds {
        match kind {
            Type::Str | Type::Bytes => {}
            other => return ExcType::type_error_path_argument(func, &other.dunder_name(vm.heap, vm.interns)),
        }
    }
    ExcType::type_error_mixed_path_components()
}
