//! Implementation of the `csv` module.
//!
//! `reader()`, `writer()`, `DictReader`, `DictWriter`, the `QUOTE_*` constants, `Error`, the
//! three built-in dialects (`excel`, `excel_tab`, `unix_dialect`, exposed as `_csv.Dialect`
//! objects), `get_dialect()`, `list_dialects()` and a fixed `field_size_limit()`. There is no
//! dialect registry (`register_dialect`) and no `Sniffer`; see `limitations/csv.md`.
//!
//! A reader over a file nobody has read yet cannot load it from `__next__`, so `reader(f)`
//! and `DictReader(f)` yield the host read with a `LoadThenCall` effect and the call is
//! replayed once the buffer is in place. Every later consumer, `list(reader)` included, then
//! works.

use std::mem;

use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::PostConversionEffect,
    types::{
        BuiltinType, CsvObject, List, Module, PyTrait, Type,
        csv::{
            DIALECT_NAMES, Dialect, DictReader, DictWriter, FIELD_SIZE_LIMIT, FmtParams, Quoting, Reader, Writer,
            write_method,
        },
        iter::collect_iterable,
        str::allocate_string,
    },
    value::Value,
};

/// Module-level `csv` functions; `DictReader` and `DictWriter` are types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum CsvFunctions {
    /// `csv.reader(csvfile, dialect='excel', **fmtparams)`.
    Reader,
    /// `csv.writer(csvfile, dialect='excel', **fmtparams)`.
    Writer,
    /// `csv.get_dialect(name)`.
    GetDialect,
    /// `csv.list_dialects()`.
    ListDialects,
    /// `csv.field_size_limit([new_limit])`.
    FieldSizeLimit,
}

/// Allocates the `csv` module and returns its `HeapId`.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Csv, vm.interns);

    for (name, function) in [
        (StaticStrings::Reader, CsvFunctions::Reader),
        (StaticStrings::Writer, CsvFunctions::Writer),
        (StaticStrings::GetDialect, CsvFunctions::GetDialect),
        (StaticStrings::ListDialects, CsvFunctions::ListDialects),
        (StaticStrings::FieldSizeLimit, CsvFunctions::FieldSizeLimit),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Csv(function)), vm);
    }
    for (name, type_) in [
        (StaticStrings::DictReader, BuiltinType::CsvDictReader),
        (StaticStrings::DictWriter, BuiltinType::CsvDictWriter),
        (StaticStrings::DialectClass, BuiltinType::CsvDialect),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::Type(type_)), vm);
    }
    for (name, quoting) in [
        (StaticStrings::QuoteMinimal, Quoting::Minimal),
        (StaticStrings::QuoteAll, Quoting::All),
        (StaticStrings::QuoteNonnumeric, Quoting::Nonnumeric),
        (StaticStrings::QuoteNone, Quoting::None),
        (StaticStrings::QuoteStrings, Quoting::Strings),
        (StaticStrings::QuoteNotnull, Quoting::Notnull),
    ] {
        module.set_attr(name, Value::Int(quoting.as_int()), vm);
    }
    module.set_attr(
        StaticStrings::ErrorClass,
        Value::Builtin(Builtins::ExcType(ExcType::CsvError)),
        vm,
    );
    // CPython's are classes; Monty exposes the resolved dialects, which `dialect=` accepts alike.
    for (name, dialect) in [
        (StaticStrings::Excel, Dialect::excel()),
        (StaticStrings::ExcelTab, Dialect::excel_tab()),
        (StaticStrings::UnixDialect, Dialect::unix()),
    ] {
        module.set_attr(name, CsvObject::Dialect(dialect).allocate(vm), vm);
    }

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `csv` module function.
pub(super) fn call(vm: &mut VM<'_>, function: CsvFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        CsvFunctions::Reader => call_reader(vm, args),
        CsvFunctions::Writer => call_writer(vm, args).map(CallResult::Value),
        CsvFunctions::GetDialect => call_get_dialect(vm, args).map(CallResult::Value),
        CsvFunctions::ListDialects => call_list_dialects(vm, args).map(CallResult::Value),
        CsvFunctions::FieldSizeLimit => call_field_size_limit(vm, args).map(CallResult::Value),
    }
}

/// `csv.reader(csvfile, dialect='excel', **fmtparams)`, loading an unread file first.
fn call_reader(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let callable = Value::ModuleFunction(ModuleFunctions::Csv(CsvFunctions::Reader));
    match load_then_call(callable, args, None, vm) {
        Ok(load) => Ok(load),
        Err(args) => reader_init(vm, args).map(CallResult::Value),
    }
}

/// `csv.DictReader(f, ...)` as called from Python, loading an unread file first.
pub(crate) fn dict_reader_call(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let callable = Value::Builtin(Builtins::Type(BuiltinType::CsvDictReader));
    match load_then_call(callable, args, Some("f"), vm) {
        Ok(load) => Ok(load),
        Err(args) => dict_reader_init(vm, args).map(CallResult::Value),
    }
}

/// When the file argument (the first positional, or the keyword `file_keyword` names) is a
/// readable file nobody has read, the host read that loads it, with `callable(args)`
/// replayed on resume. Otherwise hands `args` back.
fn load_then_call(
    callable: Value,
    args: ArgValues,
    file_keyword: Option<&str>,
    vm: &mut VM<'_>,
) -> Result<CallResult, ArgValues> {
    let file = args
        .first_positional()
        .or_else(|| file_keyword.and_then(|name| args.keyword(name, vm.heap, vm.interns)));
    let file_id = match file {
        Some(Value::Ref(id))
            if let HeapReadOutput::OpenFile(file) = vm.heap.read(*id)
                && file.needs_buffer_load(vm) =>
        {
            *id
        }
        _ => return Err(args),
    };
    let HeapReadOutput::OpenFile(file) = vm.heap.read(file_id) else {
        unreachable!("checked above")
    };
    let effect = PostConversionEffect::LoadThenCall {
        file_id,
        callable,
        args,
    };
    Ok(file.buffer_load_call(vm, effect))
}

/// The positional shape of `reader()` / `writer()`: CPython unpacks them with
/// `PyArg_UnpackTuple(args, "", 1, 2)` before the dialect kwargs are parsed.
/// Hands `args` back on success and releases them on failure.
fn check_positional_shape(name: &str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<ArgValues> {
    let error = match args.count() {
        0 => ExcType::type_error_at_least(name, 1, 0),
        n if n > 2 => ExcType::type_error_at_most(name, 2, n),
        _ => return Ok(args),
    };
    args.drop_with(vm);
    Err(error)
}

/// `reader(csvfile, dialect='excel', **fmtparams)` and `writer(...)`, as CPython parses them:
/// the positionals by `PyArg_UnpackTuple` (checked first by `check_positional_shape`), the
/// format parameters by `Dialect`'s anonymous `PyArg_ParseTupleAndKeywords`.
#[derive(FromArgs)]
#[from_args(name = "reader", style = c)]
struct FmtParamArgs {
    #[from_args(pos_only)]
    f: Value,
    #[from_args(default = None)]
    dialect: Option<Value>,
    #[from_args(kw_only, default = None)]
    delimiter: Option<Value>,
    #[from_args(kw_only, default = None)]
    doublequote: Option<Value>,
    #[from_args(kw_only, default = None)]
    escapechar: Option<Value>,
    #[from_args(kw_only, default = None)]
    lineterminator: Option<Value>,
    #[from_args(kw_only, default = None)]
    quotechar: Option<Value>,
    #[from_args(kw_only, default = None)]
    quoting: Option<Value>,
    #[from_args(kw_only, default = None)]
    skipinitialspace: Option<Value>,
    #[from_args(kw_only, default = None)]
    strict: Option<Value>,
}

impl FmtParamArgs {
    /// Splits the parse into the source, the `dialect` argument and the format parameters.
    fn into_parts(self) -> (Value, Option<Value>, FmtParams) {
        let params = FmtParams {
            delimiter: self.delimiter,
            doublequote: self.doublequote,
            escapechar: self.escapechar,
            lineterminator: self.lineterminator,
            quotechar: self.quotechar,
            quoting: self.quoting,
            skipinitialspace: self.skipinitialspace,
            strict: self.strict,
        };
        (self.f, self.dialect, params)
    }
}

/// Builds a `_csv.reader` from already-loaded (or non-file) input.
fn reader_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let args = check_positional_shape("reader", args, vm)?;
    let (csvfile, dialect, params) = FmtParamArgs::from_args(args, vm)?.into_parts();
    defer_drop!(csvfile, vm);
    defer_drop!(dialect, vm);
    let dialect = Dialect::resolve(dialect.as_ref(), params, vm)?;
    let source = csvfile.py_iter(vm)?.require_synchronous(vm)?;
    Ok(CsvObject::Reader(Reader::new(source, dialect)).allocate(vm))
}

/// `csv.writer(csvfile, dialect='excel', **fmtparams)`.
fn call_writer(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let args = check_positional_shape("writer", args, vm)?;
    let (target, dialect, params) = FmtParamArgs::from_args(args, vm)?.into_parts();
    defer_drop!(dialect, vm);
    let mut target_guard = DropGuard::new(target, vm);
    let (target, vm) = target_guard.as_parts_mut();
    let dialect = Dialect::resolve(dialect.as_ref(), params, vm)?;
    let Some(write) = write_method(target, vm)? else {
        return Err(ExcType::type_error("argument 1 must have a \"write\" method"));
    };
    let (target, vm) = target_guard.into_parts();
    Ok(CsvObject::Writer(Writer::new(target, write, dialect)).allocate(vm))
}

/// `csv.get_dialect(name)`: one of the built-in dialects.
fn call_get_dialect(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let GetDialectArgs { name } = GetDialectArgs::from_args(args, vm)?;
    defer_drop!(name, vm);
    let dialect = if name.is_str(vm.heap) {
        Dialect::by_name(name.to_str(vm)?)
    } else {
        None
    };
    dialect
        .map(|dialect| CsvObject::Dialect(dialect).allocate(vm))
        .ok_or_else(ExcType::csv_unknown_dialect)
}

/// `csv.list_dialects()`: the built-in dialect names.
fn call_list_dialects(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("list_dialects", vm.heap)?;
    let names = DIALECT_NAMES
        .iter()
        .map(|name| allocate_string(*name, vm.heap))
        .collect();
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(names)))))
}

/// `get_dialect(name)`: Argument Clinic, so `name` may be a keyword.
#[derive(FromArgs)]
#[from_args(name = "get_dialect", style = c_named, at_most_total)]
struct GetDialectArgs {
    name: Value,
}

/// `csv.field_size_limit([new_limit])`: reports CPython's default; the limit cannot change.
fn call_field_size_limit(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let FieldSizeLimitArgs { new_limit: limit } = FieldSizeLimitArgs::from_args(args, vm)?;
    defer_drop!(limit, vm);
    match limit {
        None => Ok(Value::Int(
            i64::try_from(FIELD_SIZE_LIMIT).expect("the limit fits an int"),
        )),
        Some(limit) if limit.py_type(vm) == Type::Int && !matches!(limit, Value::Bool(_)) => {
            Err(ExcType::not_implemented("field_size_limit(): changing the limit is not supported").into())
        }
        Some(_) => Err(ExcType::type_error("limit must be an integer")),
    }
}

/// `field_size_limit(new_limit=<unrepresentable>)`: Argument Clinic, so the keyword form works.
#[derive(FromArgs)]
#[from_args(name = "field_size_limit", style = c_named, at_most_total)]
struct FieldSizeLimitArgs {
    #[from_args(default)]
    new_limit: Option<Value>,
}

/// `_csv.Dialect(dialect=None, **fmtparams)`: a resolved dialect object.
pub(crate) fn dialect_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DialectArgs {
        dialect,
        delimiter,
        doublequote,
        escapechar,
        lineterminator,
        quotechar,
        quoting,
        skipinitialspace,
        strict,
    } = DialectArgs::from_args(args, vm)?;
    defer_drop!(dialect, vm);
    let params = FmtParams {
        delimiter,
        doublequote,
        escapechar,
        lineterminator,
        quotechar,
        quoting,
        skipinitialspace,
        strict,
    };
    let dialect = Dialect::resolve(dialect.as_ref(), params, vm)?;
    Ok(CsvObject::Dialect(dialect).allocate(vm))
}

/// `Dialect(dialect=None, delimiter=..., ...)`: every parameter optional, `dialect` positional too.
#[derive(FromArgs)]
#[from_args(name = "Dialect", style = c)]
struct DialectArgs {
    #[from_args(default = None)]
    dialect: Option<Value>,
    #[from_args(kw_only, default = None)]
    delimiter: Option<Value>,
    #[from_args(kw_only, default = None)]
    doublequote: Option<Value>,
    #[from_args(kw_only, default = None)]
    escapechar: Option<Value>,
    #[from_args(kw_only, default = None)]
    lineterminator: Option<Value>,
    #[from_args(kw_only, default = None)]
    quotechar: Option<Value>,
    #[from_args(kw_only, default = None)]
    quoting: Option<Value>,
    #[from_args(kw_only, default = None)]
    skipinitialspace: Option<Value>,
    #[from_args(kw_only, default = None)]
    strict: Option<Value>,
}

/// `DictReader(f, fieldnames=None, restkey=None, restval=None, dialect='excel', *args, **kwds)`.
#[derive(FromArgs)]
#[from_args(name = "DictReader.__init__", style = def)]
struct DictReaderArgs {
    f: Value,
    #[from_args(default = Value::None)]
    fieldnames: Value,
    #[from_args(default = Value::None)]
    restkey: Value,
    #[from_args(default = Value::None)]
    restval: Value,
    #[from_args(default = None)]
    dialect: Option<Value>,
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(varkwargs)]
    kwds: KwargsValues,
}

/// Builds a `csv.DictReader`; the wrapped `_csv.reader` gets `f`, `dialect` and the extras.
pub(crate) fn dict_reader_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DictReaderArgs {
        f,
        fieldnames,
        restkey,
        restval,
        dialect,
        args,
        kwds,
    } = DictReaderArgs::from_args(args, vm)?;
    let dialect = dialect.unwrap_or_else(|| excel_name(vm));
    // Everything is guarded before the first fallible step, so any rejection releases it all.
    let dialect_arg = dialect.clone_with_heap(vm.heap);
    let mut owned = DropGuard::new(
        (
            [fieldnames, restkey, restval, dialect],
            forwarded_args(f, dialect_arg, args, kwds),
        ),
        vm,
    );
    let (([fieldnames, ..], _), vm) = owned.as_parts_mut();
    materialize_fieldnames(fieldnames, vm)?;
    let ((owned_values, reader_args), vm) = owned.into_parts();
    let mut owned = DropGuard::new(owned_values, vm);
    let (_, vm) = owned.as_parts_mut();
    let reader = reader_init(vm, reader_args)?;
    let ([fieldnames, restkey, restval, dialect], vm) = owned.into_parts();
    let fieldnames = match fieldnames {
        Value::None => None,
        names => Some(names),
    };
    Ok(CsvObject::DictReader(DictReader::new(reader, fieldnames, restkey, restval, dialect)).allocate(vm))
}

/// The arguments the wrapped `reader()` / `writer()` receives: `f`, `dialect`, then `*args` and `**kwds`.
fn forwarded_args(f: Value, dialect: Value, args: Vec<Value>, kwds: KwargsValues) -> ArgValues {
    let positional = [f, dialect].into_iter().chain(args).collect();
    ArgValues::from_parts(positional, kwds)
}

/// `DictWriter(f, fieldnames, restval='', extrasaction='raise', dialect='excel', *args, **kwds)`.
#[derive(FromArgs)]
#[from_args(name = "DictWriter.__init__", style = def)]
struct DictWriterArgs {
    f: Value,
    fieldnames: Value,
    #[from_args(default = None)]
    restval: Option<Value>,
    #[from_args(default = None)]
    extrasaction: Option<Value>,
    #[from_args(default = None)]
    dialect: Option<Value>,
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(varkwargs)]
    kwds: KwargsValues,
}

/// Builds a `csv.DictWriter`; the wrapped `_csv.writer` gets `f`, `dialect` and the extras.
pub(crate) fn dict_writer_init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DictWriterArgs {
        f,
        fieldnames,
        restval,
        extrasaction,
        dialect,
        args,
        kwds,
    } = DictWriterArgs::from_args(args, vm)?;
    let restval = restval.unwrap_or_else(|| static_str(StaticStrings::EmptyString, vm));
    let extrasaction = extrasaction.unwrap_or_else(|| static_str(StaticStrings::Raise, vm));
    let dialect = dialect.unwrap_or_else(|| excel_name(vm));
    // Everything is guarded before the first fallible step, so any rejection releases it all.
    let dialect_arg = dialect.clone_with_heap(vm.heap);
    let mut owned = DropGuard::new(
        (
            [fieldnames, restval, extrasaction],
            forwarded_args(f, dialect_arg, args, kwds),
        ),
        vm,
    );
    let (([fieldnames, _, extrasaction], _), vm) = owned.as_parts_mut();
    materialize_fieldnames(fieldnames, vm)?;
    let lowered = check_extrasaction(extrasaction, vm)?;
    mem::replace(extrasaction, lowered).drop_with(vm);
    let ((owned_values, writer_args), vm) = owned.into_parts();
    let mut owned = DropGuard::new(owned_values, vm);
    let (_, vm) = owned.as_parts_mut();
    let writer = call_writer(vm, writer_args)?;
    let ([fieldnames, restval, extrasaction], vm) = owned.into_parts();
    Ok(CsvObject::DictWriter(DictWriter::new(writer, fieldnames, restval, extrasaction)).allocate(vm))
}

/// `if fieldnames is not None and iter(fieldnames) is fieldnames: fieldnames = list(fieldnames)`,
/// as both constructors open: an iterator is read once into a list, and a `fieldnames`
/// that is not iterable at all is rejected here rather than at the first row.
fn materialize_fieldnames(fieldnames: &mut Value, vm: &mut VM<'_>) -> RunResult<()> {
    if matches!(fieldnames, Value::None) {
        return Ok(());
    }
    let iterator = fieldnames.py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iterator, vm);
    if iterator.is(fieldnames) {
        let names = collect_iterable(fieldnames, vm)?;
        let list = Value::Ref(vm.heap.allocate(HeapData::List(List::new(names))));
        mem::replace(fieldnames, list).drop_with(vm);
    }
    Ok(())
}

/// `extrasaction.lower()`, which must be `'raise'` or `'ignore'` and is what gets stored.
fn check_extrasaction(extrasaction: &Value, vm: &VM<'_>) -> RunResult<Value> {
    if !extrasaction.is_str(vm.heap) {
        return Err(ExcType::attribute_error(extrasaction.py_type_name(vm), "lower"));
    }
    let text = extrasaction.to_str(vm)?;
    let lowered = text.to_lowercase();
    if lowered == "raise" || lowered == "ignore" {
        Ok(allocate_string(lowered, vm.heap))
    } else {
        Err(ExcType::value_error(format!(
            "extrasaction ({text}) must be 'raise' or 'ignore'"
        )))
    }
}

/// The default `dialect` argument, the string `'excel'`.
fn excel_name(vm: &VM<'_>) -> Value {
    static_str(StaticStrings::Excel, vm)
}

/// An interned static string as a value.
fn static_str(name: StaticStrings, vm: &VM<'_>) -> Value {
    Value::InternString(vm.interns.intern_static(name))
}
