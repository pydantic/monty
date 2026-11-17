//! `pydantic_core.ValidationError`: collecting line errors during validation,
//! raising the exception, and the methods sandboxed code reads it through;
//! plus the `PydanticUserError` constructors.
//!
//! Validators accumulate [`LineError`]s holding live input values; building the exception
//! snapshots each input (see `snapshot.rs`) and renders the messages, so the
//! exception itself is plain data. `str(exc)` is rendered once at that point
//! and stored as the exception's message; `errors()`, `error_count()`, `json()`
//! and `title` read the stored [`ValidationErrorData`], shared behind an `Arc`
//! so raising and reading never copy the snapshots.

use std::{
    fmt::{self, Write},
    sync::Arc,
};

use jiter::JiterError;
use monty_types::{
    ExcData, LocItem, MontyObject, PydanticUserErrorData, ResourceError, ResourceTracker, ValidationErrorData,
    ValidationInputType, ValidationLineError,
    unstable::{self, MontyNode},
};
use num_bigint::{BigInt, Sign};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData},
    intern::StaticStrings,
    modules::pydantic::{
        catalogue::ErrorType,
        json_writer::{InfNan, JsonWriter},
        snapshot::{Snapshotter, restore_value, str_string, try_repr_string, type_name},
    },
    string_builder::StringBuilder,
    types::{Dict, List, allocate_string, allocate_tuple},
    value::{EitherStr, Value},
};

/// The documentation link every line error ends with, for the pydantic
/// version whose messages the catalogue copies.
pub(crate) const ERROR_URL_PREFIX: &str = "https://errors.pydantic.dev/2.13/v/";

/// The documentation link a `PydanticUserError` with a `code` ends with.
const USER_ERROR_URL_PREFIX: &str = "https://errors.pydantic.dev/2.13/u/";

/// The fixed `code` of `PydanticSchemaGenerationError`.
pub(crate) const SCHEMA_GENERATION_CODE: &str = "schema-for-unknown-type";

/// One failure found during validation, before it is snapshotted.
///
/// Owns `input`: a reference to the failing value, released by `drop_with`
/// or consumed when the exception is raised. `loc` is innermost-first while
/// the error climbs out of the validators and reversed when raised.
#[derive(Debug)]
pub(crate) struct LineError {
    pub(crate) error_type: ErrorType,
    loc: Vec<LocItem>,
    input: Value,
}

impl LineError {
    /// A root-located error taking ownership of `input`.
    pub(crate) fn owned(error_type: ErrorType, input: Value) -> Self {
        Self {
            error_type,
            loc: Vec::new(),
            input,
        }
    }

    /// Prefixes the location with the enclosing container's step.
    pub(crate) fn with_outer_loc(mut self, item: LocItem) -> Self {
        self.loc.push(item);
        self
    }

    /// Replaces the error type, keeping the location and input.
    pub(crate) fn with_type(mut self, error_type: ErrorType) -> Self {
        self.error_type = error_type;
        self
    }
}

/// Why a validator failed: one or more line errors, an interpreter error (a
/// resource limit, a recursion limit) that aborts validation outright, or
/// malformed JSON, which `validate_json` reports as `json_invalid` on the
/// whole document in place of any line errors.
#[derive(Debug)]
pub(crate) enum ValError {
    Lines(Vec<LineError>),
    Run(RunError),
    Json(JiterError),
}

impl ValError {
    /// A single root-located line error taking ownership of `input`.
    pub(crate) fn owned(error_type: ErrorType, input: Value) -> Self {
        Self::Lines(vec![LineError::owned(error_type, input)])
    }
}

impl From<RunError> for ValError {
    fn from(err: RunError) -> Self {
        Self::Run(err)
    }
}

impl From<ResourceError> for ValError {
    fn from(err: ResourceError) -> Self {
        Self::Run(err.into())
    }
}

impl<C: ContainsHeap> DropWithContext<C> for ValError {
    fn drop_with(self, ctx: &mut C) {
        if let Self::Lines(lines) = self {
            lines.drop_with(ctx);
        }
    }
}

pub(crate) type ValResult<T> = Result<T, ValError>;

/// Turns a validator failure into the exception to raise: a `ValidationError`
/// titled `title` for line errors, or the interpreter error itself.
pub(crate) fn into_run_error(
    error: ValError,
    title: &str,
    input_type: ValidationInputType,
    vm: &mut VM<'_>,
) -> RunError {
    match error {
        ValError::Lines(lines) => build(title, input_type, lines, false, vm).map_or_else(|err| err, RunError::from),
        ValError::Run(err) => err,
        ValError::Json(_) => unreachable!("validate_json reports malformed JSON before raising"),
    }
}

/// The `ValidationError` for malformed JSON: one `json_invalid` line error on
/// the whole document, as pydantic raises before any validation.
pub(crate) fn json_invalid_error(title: &str, description: String, document: Value, vm: &mut VM<'_>) -> RunError {
    let line = LineError::owned(ErrorType::JsonInvalid { error: description }, document);
    build(title, ValidationInputType::Json, vec![line], false, vm).map_or_else(|err| err, RunError::from)
}

impl<C: ContainsHeap> DropWithContext<C> for LineError {
    fn drop_with(self, ctx: &mut C) {
        self.input.drop_with(ctx);
    }
}

/// Builds the `ValidationError` for `lines`, snapshotting each input under one
/// shared budget. Fails only with a resource error raised while snapshotting.
fn build(
    title: &str,
    input_type: ValidationInputType,
    lines: Vec<LineError>,
    hide_input: bool,
    vm: &mut VM<'_>,
) -> RunResult<SimpleException> {
    defer_drop!(lines, vm);
    let mut snapshotter = Snapshotter::new();
    let mut line_errors = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(index)?;
        let error_type = &line.error_type;
        line_errors.push(ValidationLineError {
            msg: error_type.render_message(input_type),
            ctx: error_type
                .context()
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
            error_type: error_type.type_str().to_owned(),
            loc: line.loc.iter().rev().cloned().collect(),
            input_type_name: type_name(&line.input, vm),
            input: snapshotter.snapshot(&line.input, vm)?,
        });
    }
    let data = ValidationErrorData {
        title: title.to_owned(),
        input_type,
        line_errors,
    };
    let message = display(&data, hide_input, &vm.heap.tracker)?;
    Ok(SimpleException::new_msg(ExcType::ValidationError, message).with_data(ExcData::Validation(Arc::new(data))))
}

/// `str(exc)`: the count line, then each line error as [`pretty_line`] renders it.
///
/// Built under the tracker and polled per line, since a `from_exception_data`
/// call can supply any number of lines.
fn display(data: &ValidationErrorData, hide_input: bool, tracker: &ResourceTracker) -> RunResult<String> {
    let count = data.line_errors.len();
    let plural = if count == 1 { "" } else { "s" };
    // A failed write is stashed in the builder and raised by `finish_raw`.
    let mut out = StringBuilder::new(tracker);
    let _ = writeln!(out, "{count} validation error{plural} for {}", data.title);
    for (index, line) in data.line_errors.iter().enumerate() {
        tracker.check_time_every(index)?;
        if index > 0 {
            let _ = out.write_char('\n');
        }
        pretty_line(&mut out, line, hide_input);
    }
    out.finish_raw()
}

/// One line error in `str(exc)`'s layout: the dotted location on its own
/// line, the message with its type, input and input type, then the URL.
fn pretty_line(out: &mut impl Write, line: &ValidationLineError, hide_input: bool) {
    if !line.loc.is_empty() {
        let loc: Vec<String> = line.loc.iter().map(loc_item_display).collect();
        let _ = out.write_str(&loc.join("."));
        let _ = out.write_char('\n');
    }
    let _ = out.write_str("  ");
    let _ = out.write_str(&line.msg);
    let _ = out.write_str(" [type=");
    let _ = out.write_str(&line.error_type);
    if !hide_input && line.error_type != "default_factory_not_called" {
        let _ = out.write_str(", input_value=");
        let mut repr = TruncatedRepr::default();
        let shortened = shorten_leaf(&line.input);
        // Writing into `TruncatedRepr` never fails.
        let _ = shortened
            .as_ref()
            .unwrap_or(&line.input)
            .as_ref()
            .write_unmarked_repr(&mut repr);
        repr.finish(out);
        let _ = out.write_str(", input_type=");
        let _ = out.write_str(&line.input_type_name);
    }
    let _ = out.write_char(']');
    let _ = out.write_str("\n    For further information visit ");
    let _ = out.write_str(ERROR_URL_PREFIX);
    let _ = out.write_str(&line.error_type);
}

/// A long `str` or `bytes` input cut to its two ends, so `str(exc)` need not
/// render a huge repr only to keep 50 bytes of it; `None` for anything else.
///
/// The middle keeps one of each quote the whole value holds, so the repr picks
/// the same quote character; `TruncatedRepr` then cuts that middle away.
fn shorten_leaf(input: &MontyObject) -> Option<MontyObject> {
    const KEEP: usize = 64;
    match unstable::node(input.as_ref()) {
        MontyNode::String(text) if text.len() > 4 * KEEP => {
            let head = &text[..ceil_char_boundary(text, KEEP)];
            let tail = &text[floor_char_boundary(text, text.len() - KEEP)..];
            let quotes = ['\'', '"'].into_iter().filter(|&quote| text.contains(quote));
            Some(MontyObject::string(
                head.chars().chain(quotes).chain(tail.chars()).collect::<String>(),
            ))
        }
        MontyNode::Bytes(bytes) if bytes.len() > 4 * KEEP => {
            let middle: Vec<u8> = b"'\"".iter().copied().filter(|quote| bytes.contains(quote)).collect();
            Some(MontyObject::bytes(
                [&bytes[..KEEP], &middle, &bytes[bytes.len() - KEEP..]].concat(),
            ))
        }
        _ => None,
    }
}

/// A location step as the dotted path shows it: a key holding a dot is quoted.
fn loc_item_display(item: &LocItem) -> String {
    match item {
        LocItem::Key(key) if key.contains('.') => format!("`{key}`"),
        LocItem::Key(key) => key.clone(),
        LocItem::Index(index) => index.to_string(),
    }
}

/// Collects a `repr()` as `str(exc)` shows it, whole up to [`Self::MAX_LEN`]
/// bytes, else its two ends around `...`, keeping only the ends of a huge input.
#[derive(Default)]
struct TruncatedRepr {
    /// The first bytes written, enough to hold a repr that needs no cut.
    head: String,
    /// The last bytes written, starting on a char boundary.
    tail: String,
    /// Bytes written in all.
    len: usize,
}

impl TruncatedRepr {
    /// pydantic's limit on an `input_value` before it is cut.
    const MAX_LEN: usize = 50;
    /// Bytes `tail` keeps: the kept end plus room to reach a char boundary.
    const TAIL_LEN: usize = 32;
    /// Size `tail` grows to before it is trimmed back, so trimming is rare.
    const TAIL_TRIM_AT: usize = 4096;

    /// Appends the repr, cut in the middle if it is over [`Self::MAX_LEN`].
    fn finish(self, out: &mut impl Write) {
        if self.len <= Self::MAX_LEN {
            let _ = out.write_str(&self.head);
        } else {
            let mid = Self::MAX_LEN.div_ceil(2);
            let _ = out.write_str(&self.head[..floor_char_boundary(&self.head, mid)]);
            let _ = out.write_str("...");
            let tail_start = self.len - self.tail.len();
            let cut = (self.len - (mid - 1)) - tail_start;
            let _ = out.write_str(&self.tail[ceil_char_boundary(&self.tail, cut)..]);
        }
    }
}

impl fmt::Write for TruncatedRepr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.len += s.len();
        if self.head.len() <= Self::MAX_LEN {
            let room = Self::MAX_LEN + 1 - self.head.len();
            self.head.push_str(&s[..ceil_char_boundary(s, room.min(s.len()))]);
        }
        if s.len() >= Self::TAIL_LEN {
            self.tail.clear();
            self.tail
                .push_str(&s[floor_char_boundary(s, s.len() - Self::TAIL_LEN)..]);
        } else {
            self.tail.push_str(s);
            if self.tail.len() > Self::TAIL_TRIM_AT {
                let start = floor_char_boundary(&self.tail, self.tail.len() - Self::TAIL_LEN);
                self.tail.drain(..start);
            }
        }
        Ok(())
    }
}

/// The largest char boundary at or below `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    (0..=index).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0)
}

/// The smallest char boundary at or above `index`.
fn ceil_char_boundary(text: &str, index: usize) -> usize {
    (index..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len())
}

/// The `title` attribute of a `ValidationError`; `None` for any other name.
pub(crate) fn getattr(data: &ValidationErrorData, attr: &EitherStr, vm: &VM<'_>) -> Option<Value> {
    match attr.static_string(vm.interns) {
        Some(StaticStrings::Title) => Some(allocate_string(data.title.clone(), vm.heap)),
        _ => None,
    }
}

/// Arguments of `ValidationError.errors(*, include_url=True, include_context=True, include_input=True)`.
#[derive(FromArgs)]
#[from_args(name = "ValidationError.errors", style = def)]
#[expect(clippy::struct_field_names, reason = "the fields are pydantic's keyword names")]
struct ErrorsArgs {
    #[from_args(kw_only, default = Value::Bool(true))]
    include_url: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    include_context: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    include_input: Value,
}

/// Arguments of `ValidationError.json(*, indent=None, include_url=True, include_context=True, include_input=True)`.
#[derive(FromArgs)]
#[from_args(name = "ValidationError.json", style = def)]
struct JsonArgs {
    #[from_args(kw_only, default = Value::None)]
    indent: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    include_url: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    include_context: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    include_input: Value,
}

/// Which optional entries `errors()` and `json()` include.
struct Includes {
    url: bool,
    context: bool,
    input: bool,
}

/// Dispatches a method call on a `ValidationError` instance.
pub(crate) fn call_method(
    data: &ValidationErrorData,
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    match attr.static_string(vm.interns) {
        Some(StaticStrings::Errors) => {
            let ErrorsArgs {
                include_url,
                include_context,
                include_input,
            } = ErrorsArgs::from_args(args, vm)?;
            let includes = includes(include_url, include_context, include_input, vm)?;
            errors_list(data, &includes, vm).map(CallResult::Value)
        }
        Some(StaticStrings::ErrorCount) => {
            args.check_zero_args("ValidationError.error_count", vm.heap)?;
            let count = i64::try_from(data.line_errors.len()).unwrap_or(i64::MAX);
            Ok(CallResult::Value(Value::Int(count)))
        }
        Some(StaticStrings::Json) => {
            let JsonArgs {
                indent,
                include_url,
                include_context,
                include_input,
            } = JsonArgs::from_args(args, vm)?;
            let indent = {
                defer_drop!(indent, vm);
                json_indent(indent, vm)
            };
            let includes = includes(include_url, include_context, include_input, vm);
            errors_json(data, indent?, &includes?, vm).map(CallResult::Value)
        }
        _ => {
            let type_name: &'static str = ExcType::ValidationError.into();
            Err(ExcType::attribute_error_method(type_name, attr, args, vm))
        }
    }
}

/// An `indent=` argument as PyO3 extracts a `usize`: `None`, a bool or a non-negative int.
pub(crate) fn json_indent(indent: &Value, vm: &VM<'_>) -> RunResult<Option<usize>> {
    let too_big = |value: &BigInt| {
        if value.sign() == Sign::Minus {
            ExcType::overflow_error_negative_unsigned()
        } else {
            ExcType::overflow_error("int too big to convert")
        }
    };
    match indent {
        Value::None => Ok(None),
        Value::Bool(b) => Ok(Some(usize::from(*b))),
        Value::Int(n) => usize::try_from(*n)
            .map(Some)
            .map_err(|_| ExcType::overflow_error_negative_unsigned()),
        Value::InternLongInt(id) => Err(too_big(vm.interns.get_long_int(*id))),
        Value::Ref(id) if let HeapData::LongInt(li) = vm.heap.get(*id) => Err(too_big(li.inner())),
        other => Err(ExcType::type_error_arg_not_integer("indent", &other.py_type_name(vm))),
    }
}

/// Checks the three `include_*` flags are bools, as PyO3 does, releasing them.
fn includes(url: Value, context: Value, input: Value, vm: &mut VM<'_>) -> RunResult<Includes> {
    defer_drop!(url, vm);
    defer_drop!(context, vm);
    defer_drop!(input, vm);
    Ok(Includes {
        url: bool_arg("include_url", url, vm)?,
        context: bool_arg("include_context", context, vm)?,
        input: bool_arg("include_input", input, vm)?,
    })
}

/// A `bool` argument as PyO3 extracts one: no truthiness, only `True` or `False`.
pub(crate) fn bool_arg(name: &str, value: &Value, vm: &VM<'_>) -> RunResult<bool> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        other => Err(ExcType::type_error_arg_not_instance(
            name,
            &other.py_type_name(vm),
            "bool",
        )),
    }
}

/// `errors()`: one dict per line error with `type`, `loc`, `msg` and the
/// optional `input`, `ctx` and `url` entries.
fn errors_list(data: &ValidationErrorData, includes: &Includes, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut guard = DropGuard::new(Vec::with_capacity(data.line_errors.len()), vm);
    let (items, vm) = guard.as_parts_mut();
    for line in &data.line_errors {
        items.push(line_dict(line, includes, vm)?);
    }
    let (items, vm) = guard.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(items)))))
}

/// The `errors()` dict for one line error.
fn line_dict(line: &ValidationLineError, includes: &Includes, vm: &mut VM<'_>) -> RunResult<Value> {
    let key = |ss: StaticStrings, vm: &VM<'_>| Value::InternString(vm.interns.intern_static(ss));
    let loc: SmallVec<[Value; 2]> = line
        .loc
        .iter()
        .map(|item| match item {
            LocItem::Key(key) => allocate_string(key.clone(), vm.heap),
            LocItem::Index(index) => Value::Int(*index),
        })
        .collect();
    let pairs = vec![
        (
            key(StaticStrings::TypeKey, vm),
            allocate_string(line.error_type.clone(), vm.heap),
        ),
        (key(StaticStrings::Loc, vm), allocate_tuple(loc, vm.heap)),
        (key(StaticStrings::Msg, vm), allocate_string(line.msg.clone(), vm.heap)),
    ];
    let mut guard = DropGuard::new(pairs, vm);
    let (pairs, vm) = guard.as_parts_mut();
    if includes.input {
        let input = restore_value(&line.input, vm)?;
        pairs.push((key(StaticStrings::Input, vm), input));
    }
    if includes.context && !line.ctx.is_empty() {
        let ctx = ctx_dict(&line.ctx, vm)?;
        pairs.push((key(StaticStrings::Ctx, vm), ctx));
    }
    if includes.url {
        pairs.push((
            key(StaticStrings::Url, vm),
            allocate_string(format!("{ERROR_URL_PREFIX}{}", line.error_type), vm.heap),
        ));
    }
    let (pairs, vm) = guard.into_parts();
    let dict = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// The `ctx` dict of one line error, each value restored from its snapshot.
fn ctx_dict(ctx: &[(String, MontyObject)], vm: &mut VM<'_>) -> RunResult<Value> {
    let mut guard = DropGuard::new(Vec::with_capacity(ctx.len()), vm);
    let (pairs, vm) = guard.as_parts_mut();
    for (name, value) in ctx {
        let value = restore_value(value, vm)?;
        pairs.push((allocate_string(name.clone(), vm.heap), value));
    }
    let (pairs, vm) = guard.into_parts();
    let dict = Dict::from_pairs(pairs, vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// `json()`: the `errors()` list as a JSON document.
fn errors_json(
    data: &ValidationErrorData,
    indent: Option<usize>,
    includes: &Includes,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let mut writer = JsonWriter::new(indent, InfNan::Constants, &vm.heap.tracker);
    writer.begin_array()?;
    for line in &data.line_errors {
        writer.element()?;
        writer.begin_object()?;
        writer.key("type")?;
        writer.string(&line.error_type)?;
        writer.key("loc")?;
        writer.begin_array()?;
        for item in &line.loc {
            writer.element()?;
            match item {
                LocItem::Key(key) => writer.string(key)?,
                LocItem::Index(index) => writer.int(*index)?,
            }
        }
        writer.end_array()?;
        writer.key("msg")?;
        writer.string(&line.msg)?;
        if includes.input {
            writer.key("input")?;
            writer.object(&line.input)?;
        }
        if includes.context && !line.ctx.is_empty() {
            writer.key("ctx")?;
            writer.begin_object()?;
            for (name, value) in &line.ctx {
                writer.key(name)?;
                writer.object(value)?;
            }
            writer.end_object()?;
        }
        if includes.url {
            writer.key("url")?;
            writer.string(&format!("{ERROR_URL_PREFIX}{}", line.error_type))?;
        }
        writer.end_object()?;
    }
    writer.end_array()?;
    let text = writer.finish()?;
    Ok(allocate_string(text, vm.heap))
}

/// Arguments of `ValidationError.from_exception_data(title, line_errors, input_type='python', hide_input=False)`.
#[derive(FromArgs)]
#[from_args(name = "ValidationError.from_exception_data", style = def)]
struct FromExceptionDataArgs {
    title: Value,
    line_errors: Value,
    #[from_args(default)]
    input_type: Option<Value>,
    #[from_args(default)]
    hide_input: Option<Value>,
}

/// The `ValidationError` type's class methods: only `from_exception_data`;
/// any other name hands `args` back for ordinary attribute lookup.
pub(crate) fn class_method(
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> Result<RunResult<CallResult>, ArgValues> {
    if attr.static_string(vm.interns) == Some(StaticStrings::FromExceptionData) {
        Ok(from_exception_data(args, vm))
    } else {
        Err(args)
    }
}

/// `ValidationError.from_exception_data()`: builds an instance from `errors()`-style dicts.
///
/// Checks run in pydantic's order: the typed arguments, then the entries, then
/// the `input_type` value. pydantic panics on a non-`str` title, where this
/// raises a `TypeError` once the other arguments pass.
fn from_exception_data(args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let FromExceptionDataArgs {
        title,
        line_errors,
        input_type,
        hide_input,
    } = FromExceptionDataArgs::from_args(args, vm)?;
    defer_drop!(title, vm);
    defer_drop!(line_errors, vm);
    defer_drop!(input_type, vm);
    defer_drop!(hide_input, vm);
    let entries = list_items(line_errors, vm)
        .ok_or_else(|| ExcType::type_error_arg_not_instance("line_errors", &line_errors.py_type_name(vm), "list"))?;
    defer_drop!(entries, vm);
    let mode = match input_type {
        None => None,
        Some(value) => Some(
            value
                .as_str(vm)
                .map(str::to_owned)
                .ok_or_else(|| ExcType::type_error_arg_not_instance("input_type", &value.py_type_name(vm), "str"))?,
        ),
    };
    let hide_input = match hide_input {
        None => false,
        Some(value) => bool_arg("hide_input", value, vm)?,
    };
    let title = title
        .as_str(vm)
        .map(str::to_owned)
        .ok_or_else(|| ExcType::type_error_arg_not_instance("title", &title.py_type_name(vm), "str"))?;
    let mut lines = DropGuard::new(Vec::with_capacity(entries.len()), vm);
    let (lines_mut, vm) = lines.as_parts_mut();
    for (index, entry) in entries.iter().enumerate() {
        vm.heap.tracker.check_memory_time_every(index)?;
        lines_mut.push(line_from_dict(entry, vm)?);
    }
    let input_type = match mode.as_deref() {
        None | Some("python") => ValidationInputType::Python,
        Some("json") => ValidationInputType::Json,
        Some(other) => return Err(ExcType::value_error(format!("Invalid error mode: {other}"))),
    };
    let (lines, vm) = lines.into_parts();
    let exc = build(&title, input_type, lines, hide_input, vm)?;
    Ok(CallResult::Value(Value::Ref(
        vm.heap.allocate(HeapData::Exception(exc)),
    )))
}

/// Owned copies of a list's items; `None` for anything else, a tuple included.
fn list_items(value: &Value, vm: &VM<'_>) -> Option<Vec<Value>> {
    let Value::Ref(id) = value else { return None };
    match vm.heap.get(*id) {
        HeapData::List(list) => Some(list.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
        _ => None,
    }
}

/// One `from_exception_data` entry: a dict with `type`, `loc`, `input` and an optional `ctx`.
fn line_from_dict(entry: &Value, vm: &mut VM<'_>) -> RunResult<LineError> {
    if !matches!(entry, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_))) {
        return Err(ExcType::type_error_not_instance(&entry.py_type_name(vm), "dict"));
    }
    let get = |name: &str, vm: &VM<'_>| {
        dict_of(entry, vm)
            .get_by_str(name, vm.heap, vm.interns)
            .map(|value| value.clone_with_heap(vm.heap))
    };
    let type_value = get("type", vm).ok_or_else(|| SimpleException::new_msg(ExcType::KeyError, "type"))?;
    defer_drop!(type_value, vm);
    let type_name = type_value
        .as_str(vm)
        .map(str::to_owned)
        .ok_or_else(|| ExcType::type_error("`type` should be a `str` or `PydanticCustomError`"))?;
    let ctx = get("ctx", vm);
    defer_drop!(ctx, vm);
    let ctx_pairs = match ctx {
        None => Vec::new(),
        Some(ctx) => {
            dict_pairs(ctx, vm).ok_or_else(|| ExcType::type_error_not_instance(&ctx.py_type_name(vm), "dict"))?
        }
    };
    defer_drop!(ctx_pairs, vm);
    let error_type = ErrorType::from_name(&type_name, ctx_pairs, vm)?;
    let loc = match get("loc", vm) {
        None => Vec::new(),
        Some(loc) => {
            defer_drop!(loc, vm);
            loc_items(loc, vm)?
        }
    };
    Ok(LineError {
        error_type,
        loc,
        input: get("input", vm).unwrap_or(Value::None),
    })
}

/// A `loc` list or tuple as innermost-first steps.
fn loc_items(loc: &Value, vm: &mut VM<'_>) -> RunResult<Vec<LocItem>> {
    let items = sequence_items(loc, vm)
        .ok_or_else(|| ExcType::type_error("Location must be a list or tuple of strings and ints"))?;
    defer_drop!(items, vm);
    items.iter().rev().map(|item| loc_item(item, vm)).collect()
}

/// One location step as pydantic keeps it: a string, an int (a bool as `0` /
/// `1`), or the repr of anything else.
fn loc_item(item: &Value, vm: &mut VM<'_>) -> RunResult<LocItem> {
    Ok(match item {
        Value::Int(index) => LocItem::Index(*index),
        Value::Bool(b) => LocItem::Index(i64::from(*b)),
        other => match other.as_str(vm) {
            Some(key) => LocItem::Key(key.to_owned()),
            None => LocItem::Key(try_repr_string(other, vm)?),
        },
    })
}

/// Owned copies of a list's or tuple's items; `None` for anything else.
fn sequence_items(value: &Value, vm: &VM<'_>) -> Option<Vec<Value>> {
    let Value::Ref(id) = value else { return None };
    match vm.heap.get(*id) {
        HeapData::List(list) => Some(list.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
        HeapData::Tuple(tuple) => Some(tuple.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
        _ => None,
    }
}

/// The dict a `Value::Ref` names; the caller checked it is one.
fn dict_of<'a>(value: &Value, vm: &'a VM<'_>) -> &'a Dict {
    let Value::Ref(id) = value else {
        unreachable!("line_from_dict checked for a dict")
    };
    match vm.heap.get(*id) {
        HeapData::Dict(dict) => dict,
        _ => unreachable!("line_from_dict checked for a dict"),
    }
}

/// A dict's entries as owned `(key, value)` snapshots, for callers that run
/// Python between steps.
pub(crate) fn dict_pairs(value: &Value, vm: &VM<'_>) -> Option<Vec<(Value, Value)>> {
    let Value::Ref(id) = value else { return None };
    match vm.heap.get(*id) {
        HeapData::Dict(dict) => Some(
            dict.iter()
                .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                .collect(),
        ),
        _ => None,
    }
}

/// Arguments of `PydanticUserError(message, *, code)`; pydantic's errors name
/// the mixin that defines `__init__`.
#[derive(FromArgs)]
#[from_args(name = "PydanticErrorMixin.__init__", style = def)]
struct UserErrorArgs {
    message: Value,
    /// Required, but the derive needs a default for a keyword-only field.
    #[from_args(kw_only, default)]
    code: Option<Value>,
}

/// Arguments of `PydanticSchemaGenerationError(message)`, whose code is fixed.
#[derive(FromArgs)]
#[from_args(name = "PydanticSchemaGenerationError.__init__", style = def)]
struct SchemaGenerationErrorArgs {
    message: Value,
}

/// Calls the `PydanticUserError` or `PydanticSchemaGenerationError` class.
pub(crate) fn new_user_error(exc_type: ExcType, args: ArgValues, vm: &mut VM<'_>) -> RunResult<SimpleException> {
    let (message, code) = if exc_type == ExcType::PydanticSchemaGenerationError {
        let SchemaGenerationErrorArgs { message } = SchemaGenerationErrorArgs::from_args(args, vm)?;
        defer_drop!(message, vm);
        (str_string(message, vm)?, Some(SCHEMA_GENERATION_CODE.to_owned()))
    } else {
        let UserErrorArgs { message, code } = UserErrorArgs::from_args(args, vm)?;
        defer_drop!(message, vm);
        defer_drop!(code, vm);
        let code = match code {
            None => {
                return Err(ExcType::type_error_missing_kwonly_with_names(
                    "PydanticErrorMixin.__init__",
                    &["code"],
                ));
            }
            Some(Value::None) => None,
            Some(code) => Some(str_string(code, vm)?),
        };
        (str_string(message, vm)?, code)
    };
    Ok(user_error(exc_type, message, code))
}

/// A `PydanticUserError` (or subclass): `str()` is `message`, followed by the
/// documentation URL when there is a `code`.
pub(crate) fn user_error(exc_type: ExcType, message: String, code: Option<String>) -> SimpleException {
    let text = match &code {
        Some(code) => format!("{message}\n\nFor further information visit {USER_ERROR_URL_PREFIX}{code}"),
        None => message.clone(),
    };
    let data = PydanticUserErrorData { message, code };
    SimpleException::new_msg(exc_type, text).with_data(ExcData::PydanticUser(Box::new(data)))
}

/// The `message` and `code` attributes of a `PydanticUserError`; `None` for any other name.
pub(crate) fn user_error_getattr(data: &PydanticUserErrorData, attr: &EitherStr, vm: &VM<'_>) -> Option<Value> {
    match attr.static_string(vm.interns) {
        Some(StaticStrings::Message) => Some(allocate_string(data.message.clone(), vm.heap)),
        Some(StaticStrings::Code) => Some(
            data.code
                .as_ref()
                .map_or(Value::None, |code| allocate_string(code.clone(), vm.heap)),
        ),
        _ => None,
    }
}
