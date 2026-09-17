//! Implementation of the eval() and exec() builtin functions.
//!
//! Both compile their source at runtime against the session's intern tables
//! and push a frame for it, so the snippet runs like a called function: it
//! can suspend to the host, and its result lands on the caller's stack.

use std::{str, sync::Arc};

use crate::{
    args::{ArgValues, FromArgs, Signature},
    bytecode::{CallResult, Compiler, FrameNamespace, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    expressions::{Identifier, Node},
    function::Function,
    heap::{DropGuard, HeapData, HeapId},
    intern::{CompileInterns, FunctionId, StaticStrings},
    name_map::NameMap,
    parse::{CodeRange, parse_expression_with_interner, parse_module_with_filename_id},
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
    if source.contains('\0') {
        return Err(
            SimpleException::new_msg(ExcType::SyntaxError, "source code string cannot contain null bytes").into(),
        );
    }
    for dict in globals.into_iter().chain(locals) {
        vm.heap.inc_ref(dict);
    }
    let (names, namespace) = vm.snippet_namespace(globals, locals)?;
    let globals_len = vm.global_names.len();
    let result = compile_and_push(builtin, source, names, namespace, vm);
    if result.is_err() {
        vm.global_names.truncate(globals_len);
    }
    result
}

/// Compiles privately and publishes only after the snippet's frame is admitted.
fn compile_and_push(
    builtin: Builtin,
    source: &Arc<str>,
    names: SnippetNames,
    namespace: Box<FrameNamespace>,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    let mut namespace_guard = DropGuard::new(namespace, vm);
    let (_, vm) = namespace_guard.as_parts_mut();
    let mut overlay = CompileInterns::new(vm.interns);
    let filename_id = overlay.add_eval_source(Arc::clone(source));
    let nodes = match builtin {
        Builtin::Exec => parse_module_with_filename_id(source, filename_id, &mut overlay),
        Builtin::Eval => {
            let trimmed = source.trim_start();
            let skipped = u32::try_from(source.len() - trimmed.len()).unwrap_or(u32::MAX);
            parse_expression_with_interner(trimmed, filename_id, &mut overlay)
                .map(|expr| vec![Node::Return(Some(expr))])
                .map_err(|e| e.shifted(skipped))
        }
    }
    .map_err(|e| e.into_run_error(source))?;

    let options = vm.env.options;
    let globals_by_name = names == SnippetNames::NameOverDict;
    let mut scratch = NameMap::new();
    let globals = if globals_by_name {
        &mut scratch
    } else {
        &mut *vm.global_names
    };
    let nodes = prepare_snippet(nodes, &overlay, globals, names).map_err(|e| e.into_run_error(source))?;
    let code = Compiler::compile_snippet(&nodes, &mut overlay, globals, options, globals_by_name)
        .map_err(|e| e.into_run_error(source))?;

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
    let (namespace, vm) = namespace_guard.into_parts();
    vm.push_snippet_frame(func_id, overlay, namespace)?;
    vm.globals.resize_with(vm.global_names.len(), || Value::Undefined);
    Ok(CallResult::FramePushed)
}

/// The snippet's text: a `str`, or `bytes` decoded as UTF-8.
fn snippet_source(name: &str, source: &Value, vm: &mut VM<'_>) -> RunResult<Arc<str>> {
    let bytes: &[u8] = match source {
        Value::InternString(id) => return Ok(Arc::from(vm.interns.get_str(*id))),
        Value::InternBytes(id) => vm.interns.get_bytes(*id),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => return Ok(Arc::from(s.as_str())),
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
