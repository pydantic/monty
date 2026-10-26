//! Implementation of Python's `tomllib` module: `loads()`, `load()` and
//! `TOMLDecodeError`.
//!
//! Parsing is a port of CPython's pure-Python `tomllib/_parser.py`, kept
//! message-for-message compatible, split by stage:
//! - [`parser`] turns TOML text into a Rust [`parser::TomlTable`] tree
//! - [`value`] converts that tree into Monty values once parsing succeeded
//!
//! Building the tree first keeps the grammar and the table/namespace rules
//! (dotted keys, `[table]` redefinition, frozen inline containers) free of heap
//! borrows; the conversion then allocates each value exactly once.

mod parser;
mod value;

use std::borrow::Cow;

use memchr::{memchr_iter, memrchr};
use monty_types::ResourceTracker;
use parser::{TomlError, TomlTable};

use super::ModuleFunctions;
use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    codecs::Codec,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    os_dispatch::PostConversionEffect,
    types::Module,
    value::Value,
};

/// Functions exposed by the `tomllib` module.
///
/// The display form matches the Python-visible function name so reprs look
/// like normal builtins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum TomllibFunctions {
    /// `tomllib.loads(s)` parses TOML text into a dict.
    Loads,
    /// `tomllib.load(fp)` parses a binary file object into a dict.
    Load,
}

/// Creates the `tomllib` module and allocates it on the heap.
///
/// The module exposes everything CPython's does: `loads`, `load` and
/// `TOMLDecodeError`.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Tomllib, vm.interns);
    module.set_attr(
        StaticStrings::Loads,
        Value::ModuleFunction(ModuleFunctions::Tomllib(TomllibFunctions::Loads)),
        vm,
    );
    module.set_attr(
        StaticStrings::Load,
        Value::ModuleFunction(ModuleFunctions::Tomllib(TomllibFunctions::Load)),
        vm,
    );
    module.set_attr(
        StaticStrings::TomlDecodeError,
        Value::Builtin(Builtins::ExcType(ExcType::TomlDecodeError)),
        vm,
    );
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a `tomllib` module function call.
///
/// `load()` may suspend for the host to read the file, so the dispatcher
/// returns a `CallResult`.
pub(super) fn call(vm: &mut VM<'_>, function: TomllibFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        TomllibFunctions::Loads => call_loads(vm, args).map(CallResult::Value),
        TomllibFunctions::Load => call_load(vm, args),
    }
}

/// Argument shape for `tomllib.loads(s, /, *, parse_float=float)`.
///
/// CPython's `loads` is a pure-Python `def`, so signature errors use
/// `style = def` wording. `parse_float` is not implemented: leaving it off the
/// struct makes the macro reject it as an unexpected keyword.
#[derive(FromArgs)]
#[from_args(name = "loads", style = def)]
struct TomlLoadsArgs {
    #[from_args(pos_only)]
    s: Value,
}

/// Implements `tomllib.loads(s)`.
///
/// Only `str` input is accepted; anything else raises CPython's
/// `Expected str object, not '{type}'` `TypeError`.
fn call_loads(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let TomlLoadsArgs { s } = TomlLoadsArgs::from_args(args, vm)?;
    defer_drop!(s, vm);
    let table = match s.to_str(vm) {
        Ok(source) => parse_document(source, &vm.heap.tracker)?,
        Err(_) => return Err(ExcType::toml_loads_type_error(&s.py_type_name(vm))),
    };
    value::table_into_value(table, vm)
}

/// Argument shape for `tomllib.load(fp, /, *, parse_float=float)`; see
/// [`TomlLoadsArgs`] for why `parse_float` is absent.
#[derive(FromArgs)]
#[from_args(name = "load", style = def)]
struct TomlLoadArgs {
    #[from_args(pos_only)]
    fp: Value,
}

/// Implements `tomllib.load(fp)` for file objects returned by `open()`.
///
/// CPython calls `fp.read()` and decodes the bytes. An unread file is loaded
/// with a [`PostConversionEffect::LoadThenCall`] that replays this call, which
/// then finds the buffer in place. Anything that is not a file object raises
/// the `AttributeError` CPython's `fp.read()` would.
fn call_load(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let TomlLoadArgs { fp } = TomlLoadArgs::from_args(args, vm)?;
    defer_drop!(fp, vm);
    let Value::Ref(file_id) = fp else {
        return Err(ExcType::attribute_error(fp.py_type_name(vm), "read"));
    };
    let HeapReadOutput::OpenFile(mut file) = vm.heap.read(*file_id) else {
        return Err(ExcType::attribute_error(fp.py_type_name(vm), "read"));
    };
    file.check_readable(vm)?;
    if !file.is_binary(vm) {
        return Err(ExcType::toml_load_text_mode_error());
    }
    if file.needs_buffer_load(vm) {
        let effect = PostConversionEffect::LoadThenCall {
            file_id: *file_id,
            callable: Value::ModuleFunction(ModuleFunctions::Tomllib(TomllibFunctions::Load)),
            args: ArgValues::One(fp.clone_with_heap(vm.heap)),
        };
        Ok(file.buffer_load_call(vm, effect))
    } else {
        let content = file.read_all(vm)?;
        parse_file_content(content, vm).map(CallResult::Value)
    }
}

/// Decodes the `bytes` a binary `read()` produced as UTF-8 (CPython's
/// `b.decode()`) and parses the text.
fn parse_file_content(content: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(content, vm);
    let bytes = match content {
        Value::InternBytes(bytes_id) => vm.interns.get_bytes(*bytes_id),
        Value::Ref(heap_id) if let HeapData::Bytes(bytes) = vm.heap.get(*heap_id) => bytes.as_slice(),
        other => {
            return Err(RunError::internal(format!(
                "tomllib.load: binary read returned {}",
                other.py_type_name(vm)
            )));
        }
    };
    let source = Codec::Utf8.decode(bytes, "strict")?;
    let table = parse_document(&source, &vm.heap.tracker)?;
    value::table_into_value(table, vm)
}

/// Parses TOML text into its table tree, raising CPython's exception on failure.
///
/// `\r\n` is normalised to `\n` first, as CPython does, so string values and
/// error positions are computed against the normalised document (which is also
/// the `doc` the exception reports).
fn parse_document(source: &str, tracker: &ResourceTracker) -> RunResult<TomlTable> {
    let src: Cow<'_, str> = if source.contains("\r\n") {
        Cow::Owned(source.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(source)
    };
    parser::parse(&src, tracker).map_err(|error| toml_error_to_run_error(error, &src))
}

/// Converts a parser failure into the Python exception CPython raises for it.
fn toml_error_to_run_error(error: TomlError, doc: &str) -> RunError {
    match error {
        TomlError::Decode { msg, pos } => {
            let (index, line, column, at_end) = error_coordinates(doc, pos);
            ExcType::toml_decode_error(&msg, doc, index, line, column, at_end)
        }
        TomlError::Recursion(msg) => SimpleException::new_msg(ExcType::RecursionError, msg).into(),
        TomlError::Run(error) => *error,
    }
}

/// CPython's `(pos, lineno, colno)` for a byte offset into `doc`, plus whether
/// the offset is at or past the end: `pos` counts characters, `lineno` is
/// 1-based and `colno` is the character column, as `TOMLDecodeError.__init__`
/// derives them from `doc`.
///
/// `byte_pos` may lie past the end of `doc` (an escape cut off by EOF), in
/// which case the overshoot is kept so `pos` matches CPython's.
fn error_coordinates(doc: &str, byte_pos: usize) -> (usize, usize, usize, bool) {
    let bytes = doc.as_bytes();
    let clamped = byte_pos.min(bytes.len());
    let prefix = &bytes[..clamped];
    let pos = count_chars(prefix) + (byte_pos - clamped);
    let lineno = memchr_iter(b'\n', prefix).count() + 1;
    let colno = match memrchr(b'\n', prefix) {
        Some(newline) => pos - count_chars(&bytes[..newline]),
        None => pos + 1,
    };
    (pos, lineno, colno, byte_pos >= bytes.len())
}

/// Number of characters in a UTF-8 prefix: every byte that is not a
/// continuation byte starts one.
fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| !matches!(byte, 0x80..=0xBF)).count()
}
