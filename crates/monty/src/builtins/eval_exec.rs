//! Implementation of the eval() and exec() builtin functions.
//!
//! Both compile their source at runtime against the session's intern tables
//! and push a frame for it, so the snippet runs like a called function: it
//! can suspend to the host, and its result lands on the caller's stack.

use std::{str, sync::Arc};

use crate::{
    args::{ArgValues, FromArgs, Signature},
    bytecode::{CallResult, Compiler, FrameGlobals, FrameNamespace, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    expressions::{Identifier, Node},
    function::Function,
    heap::{DropGuard, HeapData, HeapId},
    intern::{CompileInterns, FunctionId, StaticStrings},
    name_map::NameMap,
    parse::{CodeRange, ParseError, parse_expression_with_interner, parse_module_with_filename_id},
    prepare::{SnippetNames, prepare_snippet},
    types::{Type, py_trait::PyTrait},
    value::Value,
};

/// Arguments of `eval(source, /, globals=None, locals=None)`.
#[derive(FromArgs)]
#[from_args(name = "eval", at_most_total)]
struct EvalArgs {
    #[from_args(pos_only)]
    source: Value,
    #[from_args(default = Value::None)]
    globals: Value,
    #[from_args(default = Value::None)]
    locals: Value,
}

/// Arguments of `exec(source, /, globals=None, locals=None, *, closure=None)`.
#[derive(FromArgs)]
#[from_args(name = "exec")]
struct ExecArgs {
    #[from_args(pos_only)]
    source: Value,
    #[from_args(default = Value::None)]
    globals: Value,
    #[from_args(default = Value::None)]
    locals: Value,
    #[from_args(kw_only, default = Value::None)]
    closure: Value,
}

/// Implementation of the `eval()` builtin function.
///
/// Parses `source` as one expression and runs it in the given (or the
/// caller's) namespace; the expression's value is the call's result.
pub fn builtin_eval(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let EvalArgs {
        source,
        globals,
        locals,
    } = EvalArgs::from_args(args, vm)?;
    defer_drop!(source, vm);
    defer_drop!(globals, vm);
    defer_drop!(locals, vm);

    let text = snippet_source("eval", source, vm)?;
    let globals = globals_dict(globals, Builtin::Eval, vm)?;
    let locals = locals_dict(locals, Builtin::Eval, vm)?;
    run_snippet(Builtin::Eval, &text, globals, locals, vm)
}

/// Implementation of the `exec()` builtin function.
///
/// Compiles `source` as a module and runs it in the given (or the caller's)
/// namespace; the call's result is `None`.
pub fn builtin_exec(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ExecArgs {
        source,
        globals,
        locals,
        closure,
    } = ExecArgs::from_args(args, vm)?;
    defer_drop!(source, vm);
    defer_drop!(globals, vm);
    defer_drop!(locals, vm);
    defer_drop!(closure, vm);

    if !matches!(closure, Value::None) {
        return Err(ExcType::type_error(
            "closure can only be used when source is a code object",
        ));
    }
    let text = snippet_source("exec", source, vm)?;
    let globals = globals_dict(globals, Builtin::Exec, vm)?;
    let locals = locals_dict(locals, Builtin::Exec, vm)?;
    run_snippet(Builtin::Exec, &text, globals, locals, vm)
}

/// Which builtin is running: the two parse differently and word their
/// argument errors differently.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Eval,
    Exec,
}

/// Compiles `source` in the namespace `globals` / `locals` describe (borrowed
/// dict ids, validated by the caller) and pushes its frame.
fn run_snippet(
    builtin: Builtin,
    source: &Arc<str>,
    globals: Option<HeapId>,
    locals: Option<HeapId>,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    for dict in globals.into_iter().chain(locals) {
        vm.heap.inc_ref(dict);
    }
    let (names, namespace) = vm.snippet_namespace(globals, locals)?;
    let kind = match builtin {
        Builtin::Eval => SnippetSource::Eval,
        Builtin::Exec => SnippetSource::Exec,
    };
    compile_and_push(kind, source, names, namespace, vm)?;
    Ok(CallResult::FramePushed)
}

/// Evaluates one stringized annotation for `typing.get_type_hints()`.
///
/// Takes borrowed `globals` / `locals` dict ids (`None` globals means the
/// module slots). The namespace is built here rather than by
/// `snippet_namespace`, which would inherit the calling frame's: the caller is
/// frozen code. Each distinct text and mode compiles once per session, so a
/// decorator calling `get_type_hints` on every call publishes no new functions.
pub(crate) fn eval_hint(
    source: &Value,
    globals: Option<HeapId>,
    locals: Option<HeapId>,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let text = snippet_source("eval", source, vm)?;
    for dict in globals.into_iter().chain(locals) {
        vm.heap.inc_ref(dict);
    }
    let (globals, names) = match (globals, locals) {
        (Some(dict), _) => (FrameGlobals::Dict(dict), SnippetNames::NameOverDict),
        (None, Some(_)) => (FrameGlobals::Slots, SnippetNames::NameOverSlots),
        (None, None) => (FrameGlobals::Slots, SnippetNames::Slots),
    };
    let namespace = Box::new(FrameNamespace::Snippet { globals, locals });
    if let Some(func_id) = vm.interns.hint_snippet(names, &text) {
        vm.push_snippet_frame(func_id, None, namespace)?;
    } else {
        let func_id = compile_and_push(SnippetSource::Eval, &text, names, namespace, vm)?;
        vm.interns.set_hint_snippet(names, text, func_id);
    }
    Ok(CallResult::FramePushed)
}

/// What runtime-compiled source is: it decides how the source parses, the
/// filename its frames show and how its compile errors read.
#[derive(Clone, Copy)]
pub(crate) enum SnippetSource<'a> {
    /// An `eval()` expression, shown as `<string>`.
    Eval,
    /// An `exec()` module body, shown as `<string>`.
    Exec,
    /// An imported `.py` file's module body, shown as its virtual `path`.
    File { path: &'a str },
}

/// Compiles privately and publishes only after the snippet's frame is admitted.
///
/// Provisional global slots are released if compilation or admission fails.
pub(crate) fn compile_and_push(
    kind: SnippetSource<'_>,
    source: &Arc<str>,
    names: SnippetNames,
    namespace: Box<FrameNamespace>,
    vm: &mut VM<'_>,
) -> RunResult<FunctionId> {
    let globals_len = vm.global_names.len();
    let mut namespace_guard = DropGuard::new(namespace, vm);
    let (_, vm) = namespace_guard.as_parts_mut();
    let compiled = compile_snippet(kind, source, names, vm);
    let (namespace, vm) = namespace_guard.into_parts();
    let result = compiled.and_then(|(func_id, overlay)| {
        vm.push_snippet_frame(func_id, Some(overlay), namespace)?;
        vm.globals.resize_with(vm.global_names.len(), || Value::Undefined);
        Ok(func_id)
    });
    if result.is_err() {
        vm.global_names.truncate(globals_len);
    }
    result
}

/// Parses, prepares and compiles `source` into an unpublished overlay, returning
/// the snippet function's ID within it.
fn compile_snippet<'h>(
    kind: SnippetSource<'_>,
    source: &Arc<str>,
    names: SnippetNames,
    vm: &mut VM<'h>,
) -> RunResult<(FunctionId, CompileInterns<'h>)> {
    if !matches!(kind, SnippetSource::File { .. }) && source.contains('\0') {
        return Err(
            SimpleException::new_msg(ExcType::SyntaxError, "source code string cannot contain null bytes").into(),
        );
    }
    let mut overlay = CompileInterns::new(vm.interns);
    let filename_id = match kind {
        SnippetSource::File { path } => overlay.add_file_source(Arc::from(path), Arc::clone(source)),
        SnippetSource::Eval | SnippetSource::Exec => overlay.add_eval_source(Arc::clone(source)),
    };
    let options = vm.env.options;
    let compile_error = |e: ParseError| match kind {
        SnippetSource::File { path } => e.into_file_run_error(path, source),
        SnippetSource::Eval | SnippetSource::Exec => e.into_run_error(source),
    };
    let nodes = match kind {
        SnippetSource::Exec | SnippetSource::File { .. } => {
            parse_module_with_filename_id(source, filename_id, &mut overlay, options.source_scan_threshold)
        }
        SnippetSource::Eval => {
            let trimmed = source.trim_start();
            let skipped = u32::try_from(source.len() - trimmed.len()).unwrap_or(u32::MAX);
            parse_expression_with_interner(trimmed, filename_id, &mut overlay, options.source_scan_threshold)
                .map(|expr| vec![Node::Return(Some(expr))])
                .map_err(|e| e.shifted(skipped))
        }
    }
    .map_err(compile_error)?;

    let globals_by_name = names == SnippetNames::NameOverDict;
    let mut scratch = NameMap::new();
    let globals = if globals_by_name {
        &mut scratch
    } else {
        &mut *vm.global_names
    };
    let nodes = prepare_snippet(nodes, &overlay, globals, names).map_err(compile_error)?;
    let code =
        Compiler::compile_snippet(&nodes, &mut overlay, globals, options, globals_by_name).map_err(|e| match kind {
            SnippetSource::File { path } => e.into_file_run_error(path, source),
            SnippetSource::Eval | SnippetSource::Exec => e.into_run_error(source),
        })?;

    let position = CodeRange {
        filename: filename_id,
        start_byte: 0,
        end_byte: u32::try_from(source.len()).unwrap_or(u32::MAX),
    };
    let function = Function::new(
        Identifier::new(overlay.intern_static(StaticStrings::Module), position),
        Signature::default(),
        0,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        0,
        false,
        code,
    );
    let index = overlay.functions_len();
    let func_id = u16::try_from(index).map(FunctionId::from_index).map_err(|_| {
        SimpleException::new_msg(
            ExcType::SyntaxError,
            format!("session defines too many functions; maximum is {}", u16::MAX),
        )
    })?;

    overlay.push_function(function);
    Ok((func_id, overlay))
}

/// The snippet's text: a `str`, or `bytes` decoded as UTF-8.
fn snippet_source(name: &str, source: &Value, vm: &mut VM<'_>) -> RunResult<Arc<str>> {
    if let Some(text) = source.as_str(vm) {
        return Ok(Arc::from(text));
    }

    let bytes: &[u8] = match source {
        Value::InternBytes(id) => vm.interns.get_bytes(*id),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(b) => b.as_slice(),
            _ => return Err(source_type_error(name)),
        },
        _ => return Err(source_type_error(name)),
    };
    match str::from_utf8(bytes) {
        Ok(text) => Ok(Arc::from(text)),
        Err(err) => {
            let bad = bytes[err.valid_up_to()];
            let line = bytes[..err.valid_up_to()].split(|b| *b == b'\n').count();
            Err(SimpleException::new_msg(
                ExcType::SyntaxError,
                format!(
                    "Non-UTF-8 code starting with '\\x{bad:02x}' on line {line}, but no encoding declared; \
                     see https://peps.python.org/pep-0263/ for details (<string>, line {line})"
                ),
            )
            .into())
        }
    }
}

/// `TypeError` for a `source` that is neither text nor bytes.
fn source_type_error(name: &str) -> RunError {
    ExcType::type_error(format!("{name}() arg 1 must be a string, bytes or code object"))
}

/// Validates the `globals` argument: `None`, or a dict whose id is returned (borrowed).
fn globals_dict(globals: &Value, builtin: Builtin, vm: &mut VM<'_>) -> RunResult<Option<HeapId>> {
    match globals {
        Value::None => Ok(None),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)) => Ok(Some(*id)),
        other => {
            let ty = other.py_type(vm);
            Err(match builtin {
                // CPython distinguishes a non-dict mapping (anything subscriptable) from the rest.
                Builtin::Eval
                    if matches!(
                        ty,
                        Type::List | Type::Tuple | Type::Str | Type::Bytes | Type::Range | Type::Deque
                    ) =>
                {
                    ExcType::type_error("globals must be a real dict; try eval(expr, {}, mapping)")
                }
                Builtin::Eval => ExcType::type_error("globals must be a dict"),
                Builtin::Exec => ExcType::type_error(format!(
                    "exec() globals must be a dict, not {}",
                    ty.name(vm.heap, vm.interns)
                )),
            })
        }
    }
}

/// Validates the `locals` argument: `None`, or a dict whose id is returned (borrowed).
fn locals_dict(locals: &Value, builtin: Builtin, vm: &mut VM<'_>) -> RunResult<Option<HeapId>> {
    match locals {
        Value::None => Ok(None),
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)) => Ok(Some(*id)),
        other => Err(match builtin {
            Builtin::Eval => ExcType::type_error("locals must be a mapping"),
            Builtin::Exec => ExcType::type_error(format!(
                "locals must be a mapping or None, not {}",
                other.py_type(vm).name(vm.heap, vm.interns)
            )),
        }),
    }
}
