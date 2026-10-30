//! `pydantic_core.ValidationError`: collecting line errors during validation,
//! raising the exception, and the methods sandboxed code reads it through.
//!
//! Validators accumulate [`LineError`]s holding live input values; building the exception
//! snapshots each input (see `snapshot.rs`) and renders the messages, so the
//! exception itself is plain data. `str(exc)` is rendered once at that point
//! and stored as the exception's message; `errors()`, `error_count()`, `json()`
//! and `title` read the stored [`ValidationErrorData`].

use monty_types::{ExcData, LocItem, ValidationErrorData, ValidationInputType, ValidationLineError};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, HeapData},
    intern::StaticStrings,
    modules::pydantic::{
        catalogue::ErrorType,
        json_writer::{InfNan, JsonWriter},
        snapshot::{repr_string, restore_value, snapshot_value, type_name},
    },
    types::{Dict, List, PyTrait, allocate_string, allocate_tuple},
    value::{EitherStr, Value},
};

/// The documentation link every line error ends with, for the pydantic
/// version whose messages the catalogue copies.
pub(crate) const ERROR_URL_PREFIX: &str = "https://errors.pydantic.dev/2.13/v/";

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

/// Why a validator failed: one or more line errors, or an interpreter error
/// (a resource limit, a recursion limit) that aborts validation outright.
#[derive(Debug)]
pub(crate) enum ValError {
    Lines(Vec<LineError>),
    Run(RunError),
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
        ValError::Lines(lines) => build(title, input_type, lines, false, vm).into(),
        ValError::Run(err) => err,
    }
}

impl<C: ContainsHeap> DropWithContext<C> for LineError {
    fn drop_with(self, ctx: &mut C) {
        self.input.drop_with(ctx);
    }
}

/// Builds the `ValidationError` for `lines`, snapshotting each input.
fn build(
    title: &str,
    input_type: ValidationInputType,
    lines: Vec<LineError>,
    hide_input: bool,
    vm: &mut VM<'_>,
) -> SimpleException {
    let line_errors = lines
        .into_iter()
        .map(|line| {
            let LineError {
                error_type,
                mut loc,
                input,
            } = line;
            defer_drop!(input, vm);
            loc.reverse();
            ValidationLineError {
                msg: error_type.render_message(input_type),
                ctx: error_type
                    .context()
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
                error_type: error_type.type_str().to_owned(),
                loc,
                input_type_name: type_name(input, vm),
                input: snapshot_value(input, vm),
            }
        })
        .collect();
    let data = ValidationErrorData {
        title: title.to_owned(),
        input_type,
        line_errors,
    };
    let message = display(&data, hide_input);
    SimpleException::new_msg(ExcType::ValidationError, message).with_data(ExcData::Validation(Box::new(data)))
}

/// `str(exc)`: the count line, then each line error as [`pretty_line`] renders it.
fn display(data: &ValidationErrorData, hide_input: bool) -> String {
    let count = data.line_errors.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut out = format!("{count} validation error{plural} for {}\n", data.title);
    for (index, line) in data.line_errors.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        pretty_line(&mut out, line, hide_input);
    }
    out
}

/// One line error in `str(exc)`'s layout: the dotted location on its own
/// line, the message with its type, input and input type, then the URL.
fn pretty_line(out: &mut String, line: &ValidationLineError, hide_input: bool) {
    if !line.loc.is_empty() {
        let loc: Vec<String> = line.loc.iter().map(loc_item_display).collect();
        out.push_str(&loc.join("."));
        out.push('\n');
    }
    out.push_str("  ");
    out.push_str(&line.msg);
    out.push_str(" [type=");
    out.push_str(&line.error_type);
    if !hide_input && line.error_type != "default_factory_not_called" {
        out.push_str(", input_value=");
        write_truncated(out, &line.input.py_repr(), 50);
        out.push_str(", input_type=");
        out.push_str(&line.input_type_name);
    }
    out.push(']');
    out.push_str("\n    For further information visit ");
    out.push_str(ERROR_URL_PREFIX);
    out.push_str(&line.error_type);
}

/// A location step as the dotted path shows it: a key holding a dot is quoted.
fn loc_item_display(item: &LocItem) -> String {
    match item {
        LocItem::Key(key) if key.contains('.') => format!("`{key}`"),
        LocItem::Key(key) => key.clone(),
        LocItem::Index(index) => index.to_string(),
    }
}

/// Writes `text` whole up to `max_len` bytes, else its two ends around `...`.
fn write_truncated(out: &mut String, text: &str, max_len: usize) {
    if text.len() > max_len {
        let mid = max_len.div_ceil(2);
        out.push_str(&text[..floor_char_boundary(text, mid)]);
        out.push_str("...");
        out.push_str(&text[ceil_char_boundary(text, text.len() - (mid - 1))..]);
    } else {
        out.push_str(text);
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
#[from_args(name = "errors", style = def)]
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
#[from_args(name = "json", style = def)]
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
            args.check_zero_args("error_count", vm.heap)?;
            Ok(CallResult::Value(Value::Int(
                i64::try_from(data.line_errors.len()).unwrap_or(i64::MAX),
            )))
        }
        Some(StaticStrings::Json) => {
            let JsonArgs {
                indent,
                include_url,
                include_context,
                include_input,
            } = JsonArgs::from_args(args, vm)?;
            defer_drop!(indent, vm);
            let indent = match indent {
                Value::None => None,
                Value::Int(n) => Some(usize::try_from(*n).unwrap_or(0)),
                other => {
                    return Err(ExcType::type_error(format!(
                        "argument 'indent': '{}' object cannot be interpreted as an integer",
                        other.py_type_name(vm)
                    )));
                }
            };
            let includes = includes(include_url, include_context, include_input, vm)?;
            errors_json(data, indent, &includes, vm).map(CallResult::Value)
        }
        _ => Err(ExcType::attribute_error_method("ValidationError", attr, args, vm)),
    }
}

/// Coerces the three `include_*` flags to booleans, releasing them.
fn includes(url: Value, context: Value, input: Value, vm: &mut VM<'_>) -> RunResult<Includes> {
    defer_drop!(url, vm);
    defer_drop!(context, vm);
    defer_drop!(input, vm);
    Ok(Includes {
        url: url.py_bool(vm)?,
        context: context.py_bool(vm)?,
        input: input.py_bool(vm)?,
    })
}

/// `errors()`: one dict per line error with `type`, `loc`, `msg` and the
/// optional `input`, `ctx` and `url` entries.
fn errors_list(data: &ValidationErrorData, includes: &Includes, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut items = Vec::with_capacity(data.line_errors.len());
    for line in &data.line_errors {
        let dict = match line_dict(line, includes, vm) {
            Ok(dict) => dict,
            Err(err) => {
                items.drop_with(vm);
                return Err(err);
            }
        };
        items.push(dict);
    }
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
    let mut pairs = vec![
        (
            key(StaticStrings::TypeKey, vm),
            allocate_string(line.error_type.clone(), vm.heap),
        ),
        (key(StaticStrings::Loc, vm), allocate_tuple(loc, vm.heap)),
        (key(StaticStrings::Msg, vm), allocate_string(line.msg.clone(), vm.heap)),
    ];
    if includes.input {
        match restore_value(&line.input, vm) {
            Ok(input) => pairs.push((key(StaticStrings::Input, vm), input)),
            Err(err) => {
                pairs.drop_with(vm);
                return Err(err);
            }
        }
    }
    if includes.context && !line.ctx.is_empty() {
        let mut ctx_pairs = Vec::with_capacity(line.ctx.len());
        for (name, value) in &line.ctx {
            match restore_value(value, vm) {
                Ok(value) => ctx_pairs.push((allocate_string(name.clone(), vm.heap), value)),
                Err(err) => {
                    ctx_pairs.drop_with(vm);
                    pairs.drop_with(vm);
                    return Err(err);
                }
            }
        }
        let ctx = match Dict::from_pairs(ctx_pairs, vm) {
            Ok(dict) => Value::Ref(vm.heap.allocate(HeapData::Dict(dict))),
            Err(err) => {
                pairs.drop_with(vm);
                return Err(err);
            }
        };
        pairs.push((key(StaticStrings::Ctx, vm), ctx));
    }
    if includes.url {
        pairs.push((
            key(StaticStrings::Url, vm),
            allocate_string(format!("{ERROR_URL_PREFIX}{}", line.error_type), vm.heap),
        ));
    }
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
#[from_args(name = "from_exception_data", style = def)]
struct FromExceptionDataArgs {
    title: Value,
    line_errors: Value,
    #[from_args(default = Value::None)]
    input_type: Value,
    #[from_args(default = Value::Bool(false))]
    hide_input: Value,
}

/// Dispatches a class method on the `ValidationError` type: only
/// `from_exception_data`, which builds an instance from `errors()`-style dicts.
pub(crate) fn call_class_method(attr: &EitherStr, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if attr.static_string(vm.interns) != Some(StaticStrings::FromExceptionData) {
        return Err(ExcType::attribute_error_method(
            "type object 'ValidationError'",
            attr,
            args,
            vm,
        ));
    }
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
    let Some(title) = title.as_str(vm).map(str::to_owned) else {
        return Err(ExcType::type_error(format!(
            "argument 'title': '{}' object is not an instance of 'str'",
            title.py_type_name(vm)
        )));
    };
    let input_type = match input_type.as_str(vm) {
        None | Some("python") => ValidationInputType::Python,
        Some("json") => ValidationInputType::Json,
        Some(other) => return Err(ExcType::value_error(format!("Invalid error mode: {other}"))),
    };
    let hide_input = hide_input.py_bool(vm)?;
    let entries = sequence_items(line_errors, vm).ok_or_else(|| {
        ExcType::type_error(format!(
            "argument 'line_errors': '{}' object is not an instance of 'list'",
            line_errors.py_type_name(vm)
        ))
    })?;
    defer_drop!(entries, vm);
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        match line_from_dict(entry, vm) {
            Ok(line) => lines.push(line),
            Err(err) => {
                lines.drop_with(vm);
                return Err(err);
            }
        }
    }
    let exc = build(&title, input_type, lines, hide_input, vm);
    Ok(CallResult::Value(Value::Ref(
        vm.heap.allocate(HeapData::Exception(exc)),
    )))
}

/// Owned copies of a list's or tuple's items; `None` for anything else.
fn sequence_items(value: &Value, vm: &mut VM<'_>) -> Option<Vec<Value>> {
    let Value::Ref(id) = value else { return None };
    match vm.heap.get(*id) {
        HeapData::List(list) => Some(list.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
        HeapData::Tuple(tuple) => Some(tuple.as_slice().iter().map(|v| v.clone_with_heap(vm.heap)).collect()),
        _ => None,
    }
}

/// One `from_exception_data` entry: a dict with `type`, `loc`, `input` and an optional `ctx`.
fn line_from_dict(entry: &Value, vm: &mut VM<'_>) -> RunResult<LineError> {
    if !matches!(entry, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_))) {
        return Err(ExcType::type_error(format!(
            "'{}' object is not an instance of 'dict'",
            entry.py_type_name(vm)
        )));
    }
    let get = |name: &str, vm: &VM<'_>| {
        dict_of(entry, vm)
            .get_by_str(name, vm.heap, vm.interns)
            .map(|value| value.clone_with_heap(vm.heap))
    };
    let type_value = get("type", vm).ok_or_else(|| ExcType::type_error("'type' required in line error"))?;
    defer_drop!(type_value, vm);
    let Some(type_name) = type_value.as_str(vm).map(str::to_owned) else {
        return Err(ExcType::type_error("'type' must be a str"));
    };
    let ctx = get("ctx", vm).unwrap_or(Value::None);
    defer_drop!(ctx, vm);
    let ctx_pairs: Vec<(String, Value)> = dict_pairs(ctx, vm)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| {
            let name = key.as_str(vm).map(str::to_owned);
            key.drop_with(vm);
            if let Some(name) = name {
                Some((name, value))
            } else {
                value.drop_with(vm);
                None
            }
        })
        .collect();
    let error_type = ErrorType::from_name(&type_name, &ctx_pairs, vm);
    for (_, value) in ctx_pairs {
        value.drop_with(vm);
    }
    let error_type = error_type?;
    let input = get("input", vm).unwrap_or(Value::None);
    let mut line = LineError::owned(error_type, input);
    if let Some(loc) = get("loc", vm) {
        defer_drop!(loc, vm);
        let Some(items) = sequence_items(loc, vm) else {
            line.drop_with(vm);
            return Err(ExcType::type_error("'loc' must be a tuple or list"));
        };
        defer_drop!(items, vm);
        // Stored innermost-first, so the outermost step is pushed last.
        for item in items.iter().rev() {
            // pydantic keeps an int, a string, or the repr of anything else.
            let step = match item {
                Value::Int(index) => LocItem::Index(*index),
                other => match other.as_str(vm) {
                    Some(key) => LocItem::Key(key.to_owned()),
                    None => LocItem::Key(repr_string(other, vm)),
                },
            };
            line = line.with_outer_loc(step);
        }
    }
    Ok(line)
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
