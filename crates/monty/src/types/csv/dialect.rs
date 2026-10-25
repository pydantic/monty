//! `_csv.Dialect`: the resolved formatting parameters a reader or writer runs with.
//!
//! Resolution follows CPython's `dialect_new`: an explicit format parameter wins, then the
//! attribute of the `dialect` argument (a registered name, a `_csv.Dialect` or any object with
//! the attributes), then the `excel` default; conversion and validation errors keep CPython's
//! wording and order.

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData},
    intern::StaticStrings,
    types::{CsvObject, PyTrait, Type, str::allocate_string},
    value::{EitherStr, Value},
};

/// The `csv.QUOTE_*` constants; the discriminants are CPython's values, which the module
/// exports and `dialect.quoting` reports. Serialized by name, so the numbering is not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i64)]
pub(crate) enum Quoting {
    Minimal = 0,
    All = 1,
    Nonnumeric = 2,
    None = 3,
    Strings = 4,
    Notnull = 5,
}

impl Quoting {
    /// Every constant, indexed by its value.
    const ALL: [Self; 6] = [
        Self::Minimal,
        Self::All,
        Self::Nonnumeric,
        Self::None,
        Self::Strings,
        Self::Notnull,
    ];

    /// The constant's value.
    pub(crate) const fn as_int(self) -> i64 {
        self as i64
    }

    /// The constant for a `quoting=` value; `None` for anything CPython rejects.
    fn from_int(value: i32) -> Option<Self> {
        usize::try_from(value)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
    }
}

/// A resolved dialect. Readers and writers hold their own copy, so `reader.dialect` is a fresh
/// `_csv.Dialect` object on each access rather than the one passed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Dialect {
    pub(crate) delimiter: char,
    pub(crate) quotechar: Option<char>,
    pub(crate) escapechar: Option<char>,
    pub(crate) doublequote: bool,
    pub(crate) skipinitialspace: bool,
    pub(crate) lineterminator: String,
    pub(crate) quoting: Quoting,
    pub(crate) strict: bool,
}

/// The format parameters a call supplied explicitly; `None` means "not given", which is
/// distinct from an explicit Python `None` (`quotechar=None` turns quoting off).
#[derive(Debug, Default)]
pub(crate) struct FmtParams {
    pub(crate) delimiter: Option<Value>,
    pub(crate) doublequote: Option<Value>,
    pub(crate) escapechar: Option<Value>,
    pub(crate) lineterminator: Option<Value>,
    pub(crate) quotechar: Option<Value>,
    pub(crate) quoting: Option<Value>,
    pub(crate) skipinitialspace: Option<Value>,
    pub(crate) strict: Option<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for FmtParams {
    fn drop_with(self, ctx: &mut C) {
        self.delimiter.drop_with(ctx);
        self.doublequote.drop_with(ctx);
        self.escapechar.drop_with(ctx);
        self.lineterminator.drop_with(ctx);
        self.quotechar.drop_with(ctx);
        self.quoting.drop_with(ctx);
        self.skipinitialspace.drop_with(ctx);
        self.strict.drop_with(ctx);
    }
}

/// The registered dialect names, in `csv.list_dialects()` order.
pub(crate) const DIALECT_NAMES: [&str; 3] = ["excel", "excel-tab", "unix"];

impl Dialect {
    /// `csv.excel`: the default dialect.
    pub(crate) fn excel() -> Self {
        Self {
            delimiter: ',',
            quotechar: Some('"'),
            escapechar: None,
            doublequote: true,
            skipinitialspace: false,
            lineterminator: "\r\n".to_owned(),
            quoting: Quoting::Minimal,
            strict: false,
        }
    }

    /// `csv.excel_tab`: `excel` with a tab delimiter.
    pub(crate) fn excel_tab() -> Self {
        Self {
            delimiter: '\t',
            ..Self::excel()
        }
    }

    /// `csv.unix_dialect`: `\n` line endings and every field quoted.
    pub(crate) fn unix() -> Self {
        Self {
            lineterminator: "\n".to_owned(),
            quoting: Quoting::All,
            ..Self::excel()
        }
    }

    /// The registered dialect called `name`, if any.
    pub(crate) fn by_name(name: &str) -> Option<Self> {
        match name {
            "excel" => Some(Self::excel()),
            "excel-tab" => Some(Self::excel_tab()),
            "unix" => Some(Self::unix()),
            _ => None,
        }
    }

    /// Resolves the dialect a reader, writer or `_csv.Dialect(...)` call runs with.
    ///
    /// `base` is the `dialect` argument: a registered name, a `_csv.Dialect`, or any other object
    /// whose attributes supply the parameters `params` leaves unset. Consumes `params` on every path.
    pub(crate) fn resolve(base: Option<&Value>, params: FmtParams, vm: &mut VM<'_>) -> RunResult<Self> {
        let mut params_guard = DropGuard::new(params, vm);
        let (params, vm) = params_guard.as_parts_mut();
        let base = match base {
            None => Base::Defaults,
            Some(value) if value.is_str(vm.heap) => {
                Base::Dialect(Self::by_name(value.to_str(vm)?).ok_or_else(ExcType::csv_unknown_dialect)?)
            }
            Some(Value::Ref(id))
                if let HeapData::Csv(csv) = vm.heap.get(*id)
                    && let CsvObject::Dialect(dialect) = &**csv =>
            {
                Base::Dialect(dialect.clone())
            }
            Some(value) => Base::Object(value),
        };

        // CPython fetches every parameter the call left unset from `base` first, in this
        // order, and converts them only afterwards. They stay in the guarded `params` until
        // converted, so an earlier parameter's error still releases them.
        params.delimiter = base.take(params.delimiter.take(), StaticStrings::Delimiter, vm)?;
        params.doublequote = base.take(params.doublequote.take(), StaticStrings::Doublequote, vm)?;
        params.escapechar = base.take(params.escapechar.take(), StaticStrings::Escapechar, vm)?;
        params.lineterminator = base.take(params.lineterminator.take(), StaticStrings::Lineterminator, vm)?;
        params.quotechar = base.take(params.quotechar.take(), StaticStrings::Quotechar, vm)?;
        params.quoting = base.take(params.quoting.take(), StaticStrings::Quoting, vm)?;
        params.skipinitialspace = base.take(params.skipinitialspace.take(), StaticStrings::Skipinitialspace, vm)?;
        params.strict = base.take(params.strict.take(), StaticStrings::Strict, vm)?;

        // Explicit `quotechar=None` with no `quoting` selects QUOTE_NONE, decided on the
        // raw values before conversion as CPython does.
        let quoting_unset = params.quoting.is_none();
        let quotechar_is_none = matches!(params.quotechar, Some(Value::None));

        let delimiter = set_char("delimiter", params.delimiter.take(), ',', vm)?;
        let doublequote = set_bool(params.doublequote.take(), true, vm)?;
        let escapechar = set_char_or_none("escapechar", params.escapechar.take(), None, vm)?;
        let lineterminator = set_str("lineterminator", params.lineterminator.take(), "\r\n", vm)?;
        let quotechar = set_char_or_none("quotechar", params.quotechar.take(), Some('"'), vm)?;
        let mut quoting = set_quoting(params.quoting.take(), vm)?;
        let skipinitialspace = set_bool(params.skipinitialspace.take(), false, vm)?;
        let strict = set_bool(params.strict.take(), false, vm)?;

        if quotechar_is_none && quoting_unset {
            quoting = Quoting::None;
        }
        if quoting != Quoting::None && quotechar.is_none() {
            return Err(ExcType::type_error("quotechar must be set if quoting enabled"));
        }
        let dialect = Self {
            delimiter,
            quotechar,
            escapechar,
            doublequote,
            skipinitialspace,
            lineterminator,
            quoting,
            strict,
        };
        dialect.check_char("delimiter", Some(delimiter), true)?;
        dialect.check_char("escapechar", escapechar, !skipinitialspace)?;
        dialect.check_char("quotechar", quotechar, !skipinitialspace)?;
        check_chars("delimiter", "escapechar", Some(delimiter), escapechar)?;
        check_chars("delimiter", "quotechar", Some(delimiter), quotechar)?;
        check_chars("escapechar", "quotechar", escapechar, quotechar)?;
        Ok(dialect)
    }

    /// `dialect_check_char`: a control character, or a character that also ends lines,
    /// cannot serve as a delimiter, quote or escape.
    fn check_char(&self, name: &str, c: Option<char>, allow_space: bool) -> RunResult<()> {
        let Some(c) = c else {
            return Ok(());
        };
        if c == '\r' || c == '\n' || (c == ' ' && !allow_space) {
            Err(ExcType::value_error(format!("bad {name} value")))
        } else if self.lineterminator.contains(c) {
            Err(ExcType::value_error(format!("bad {name} or lineterminator value")))
        } else {
            Ok(())
        }
    }

    /// Whether `attr` names one of the eight `_csv.Dialect` attributes.
    pub(crate) fn is_attr(attr: StaticStrings) -> bool {
        matches!(
            attr,
            StaticStrings::Delimiter
                | StaticStrings::Quotechar
                | StaticStrings::Escapechar
                | StaticStrings::Doublequote
                | StaticStrings::Skipinitialspace
                | StaticStrings::Lineterminator
                | StaticStrings::Quoting
                | StaticStrings::Strict
        )
    }

    /// The value of a `_csv.Dialect` attribute, or `None` for a name that is not one.
    pub(crate) fn attr(&self, attr: StaticStrings, vm: &VM<'_>) -> Option<Value> {
        Some(match attr {
            StaticStrings::Delimiter => char_value(self.delimiter, vm),
            StaticStrings::Quotechar => self.quotechar.map_or(Value::None, |c| char_value(c, vm)),
            StaticStrings::Escapechar => self.escapechar.map_or(Value::None, |c| char_value(c, vm)),
            StaticStrings::Doublequote => Value::Bool(self.doublequote),
            StaticStrings::Skipinitialspace => Value::Bool(self.skipinitialspace),
            StaticStrings::Lineterminator => allocate_string(self.lineterminator.clone(), vm.heap),
            StaticStrings::Quoting => Value::Int(self.quoting.as_int()),
            StaticStrings::Strict => Value::Bool(self.strict),
            _ => return None,
        })
    }
}

/// A single-character `str` value.
fn char_value(c: char, vm: &VM<'_>) -> Value {
    allocate_string(c.to_string(), vm.heap)
}

/// Where unset parameters come from: the defaults, a resolved dialect, or an
/// arbitrary object's attributes (a user class with `delimiter = ';'`).
enum Base<'a> {
    Defaults,
    Dialect(Dialect),
    Object(&'a Value),
}

impl Base<'_> {
    /// The parameter to convert: `explicit` when given, else this base's value, else `None`
    /// for the default. The result is owned; the caller releases it.
    fn take(&self, explicit: Option<Value>, attr: StaticStrings, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
        if explicit.is_some() {
            return Ok(explicit);
        }
        match self {
            Self::Defaults => Ok(None),
            Self::Dialect(dialect) => Ok(dialect.attr(attr, vm)),
            Self::Object(object) => {
                let name = EitherStr::Interned(vm.interns.intern_static(attr));
                match object.py_getattr(&name, vm) {
                    Ok(CallResult::Value(value)) => Ok(Some(value)),
                    // A missing attribute means "use the default", as CPython's `PyErr_Clear` does.
                    Err(err) if err.is_catchable(ExcType::AttributeError) => Ok(None),
                    Err(err) => Err(err),
                    Ok(other) => {
                        other.drop_with(vm);
                        Err(ExcType::not_implemented(format!(
                            "csv: dialect attribute '{}' cannot be computed by a property",
                            attr_name(attr)
                        ))
                        .into())
                    }
                }
            }
        }
    }
}

/// The Python spelling of a parameter name, for error messages.
fn attr_name(attr: StaticStrings) -> &'static str {
    attr.into()
}

/// `_set_char`: a one-character `str`.
fn set_char(name: &str, value: Option<Value>, default: char, vm: &mut VM<'_>) -> RunResult<char> {
    let Some(value) = value else {
        return Ok(default);
    };
    let result = single_char(name, &value, "a unicode character", vm);
    value.drop_with(vm);
    result
}

/// `_set_char_or_none`: a one-character `str`, or `None` to unset.
fn set_char_or_none(
    name: &str,
    value: Option<Value>,
    default: Option<char>,
    vm: &mut VM<'_>,
) -> RunResult<Option<char>> {
    let Some(value) = value else {
        return Ok(default);
    };
    let result = if matches!(value, Value::None) {
        Ok(None)
    } else {
        single_char(name, &value, "a unicode character or None", vm).map(Some)
    };
    value.drop_with(vm);
    result
}

/// The character of a one-character `str`, with CPython's two wordings for a wrong
/// type and a wrong length.
fn single_char(name: &str, value: &Value, expected: &str, vm: &VM<'_>) -> RunResult<char> {
    if value.is_str(vm.heap) {
        let text = value.to_str(vm)?;
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(ExcType::type_error(format!(
                "\"{name}\" must be {expected}, not a string of length {}",
                text.chars().count()
            ))),
        }
    } else {
        Err(ExcType::type_error(format!(
            "\"{name}\" must be {expected}, not {}",
            value.py_type_name(vm)
        )))
    }
}

/// `_set_bool`: any value, through `bool()`.
fn set_bool(value: Option<Value>, default: bool, vm: &mut VM<'_>) -> RunResult<bool> {
    let Some(value) = value else {
        return Ok(default);
    };
    let result = value.py_bool(vm);
    value.drop_with(vm);
    result
}

/// `_set_str`: any `str`.
fn set_str(name: &str, value: Option<Value>, default: &str, vm: &mut VM<'_>) -> RunResult<String> {
    let Some(value) = value else {
        return Ok(default.to_owned());
    };
    let result = if value.is_str(vm.heap) {
        value.to_str(vm).map(str::to_owned)
    } else {
        Err(ExcType::type_error(format!(
            "\"{name}\" must be a string, not {}",
            value.py_type_name(vm)
        )))
    };
    value.drop_with(vm);
    result
}

/// `_set_int` plus `dialect_check_quoting`: an exact `int` naming a `QUOTE_*` constant.
/// The conversion goes through a C `int`, so a value beyond it is an `OverflowError`.
fn set_quoting(value: Option<Value>, vm: &mut VM<'_>) -> RunResult<Quoting> {
    let Some(value) = value else {
        return Ok(Quoting::Minimal);
    };
    let result = match value {
        Value::Int(n) => match i32::try_from(n) {
            Ok(n) => Quoting::from_int(n).ok_or_else(|| ExcType::type_error("bad \"quoting\" value")),
            Err(_) => Err(ExcType::overflow_c_int()),
        },
        Value::Bool(_) => Err(ExcType::type_error("\"quoting\" must be an integer, not bool")),
        _ if value.py_type(vm) == Type::Int => Err(ExcType::overflow_c_int()),
        _ => Err(ExcType::type_error(format!(
            "\"quoting\" must be an integer, not {}",
            value.py_type_name(vm)
        ))),
    };
    value.drop_with(vm);
    result
}

/// `dialect_check_chars`: two set characters must differ.
fn check_chars(name1: &str, name2: &str, c1: Option<char>, c2: Option<char>) -> RunResult<()> {
    if c1.is_some() && c1 == c2 {
        Err(ExcType::value_error(format!("bad {name1} or {name2} value")))
    } else {
        Ok(())
    }
}
