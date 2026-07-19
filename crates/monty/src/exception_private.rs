use std::{
    borrow::Cow,
    fmt::{self, Display, Write},
};

use serde::{Deserialize, Serialize};
use smallvec::smallvec;
use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_public::{ExcData, JsonErrorData, MontyException, SourceMap, StackFrame, UnicodeErrorData},
    fstring::{FormatError, ascii_escape},
    heap::{HeapData, HeapRead},
    intern::{Interns, StaticStrings, StringId},
    parse::CodeRange,
    resource::ResourceTracker,
    types::{
        PyTrait, Type, allocate_tuple,
        long_int::INT_MAX_STR_DIGITS,
        str::{StringRepr, allocate_string, string_repr_fmt},
    },
    value::{EitherStr, Value},
};

/// Result type alias for operations that can produce a runtime error.
pub type RunResult<T> = Result<T, RunError>;

/// Python exception types supported by the interpreter.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `Into<&'static str>` implementations.
/// The string representation matches the variant name exactly (e.g., `ValueError` -> "ValueError").
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Display, EnumString, IntoStaticStr, Serialize, Deserialize,
)]
pub enum ExcType {
    /// primary exception class - matches any exception in isinstance checks.
    ///
    /// Also the `Default` — required so `Type` (which embeds an `ExcType` in
    /// its `Exception` variant) can derive `strum::EnumIter`.
    #[default]
    Exception,

    /// System exit exceptions
    BaseException,
    SystemExit,
    KeyboardInterrupt,

    // --- ArithmeticError hierarchy ---
    /// Intermediate class for arithmetic errors.
    ArithmeticError,
    /// Subclass of ArithmeticError.
    OverflowError,
    /// Subclass of ArithmeticError.
    ZeroDivisionError,

    // --- LookupError hierarchy ---
    /// Intermediate class for lookup errors.
    LookupError,
    /// Subclass of LookupError.
    IndexError,
    /// Subclass of LookupError.
    KeyError,

    // --- RuntimeError hierarchy ---
    /// Intermediate class for runtime errors.
    RuntimeError,
    /// Subclass of RuntimeError.
    NotImplementedError,
    /// Subclass of RuntimeError.
    RecursionError,

    // --- AttributeError hierarchy ---
    AttributeError,
    /// Subclass of AttributeError (from dataclasses module).
    FrozenInstanceError,

    // --- NameError hierarchy ---
    NameError,
    /// Subclass of NameError - for accessing local variable before assignment.
    UnboundLocalError,

    // --- ValueError hierarchy ---
    ValueError,
    /// Subclass of ValueError - for encoding/decoding errors.
    UnicodeDecodeError,
    /// Subclass of ValueError - for encoding errors (e.g. `str.encode('ascii')`
    /// on a string containing non-ASCII characters).
    UnicodeEncodeError,
    /// Subclass of ValueError for invalid JSON syntax in `json.loads()`.
    #[strum(serialize = "json.JSONDecodeError")]
    JsonDecodeError,

    // --- ImportError hierarchy ---
    /// Import-related errors (module not found, name not in module).
    ImportError,
    /// Subclass of ImportError - for when a module cannot be found.
    ModuleNotFoundError,

    // --- OSError hierarchy ---
    /// OS-related errors (file not found, permission denied, etc.)
    OSError,
    /// Subclass of OSError - for when a file or directory cannot be found.
    FileNotFoundError,
    /// Subclass of OSError - for when a file already exists.
    FileExistsError,
    /// Subclass of OSError - for when a path is a directory but a file was expected.
    IsADirectoryError,
    /// Subclass of OSError - for when a path is not a directory but one was expected.
    NotADirectoryError,
    /// Subclass of OSError - for when an operation is not permitted (e.g., writing
    /// to a read-only mount, or attempting to access a path outside a mounted directory).
    PermissionError,
    /// `io.UnsupportedOperation` - raised by file objects when a requested
    /// operation isn't allowed by the open mode (e.g. `read()` on `'w'`).
    ///
    /// In CPython this inherits from both `OSError` and `ValueError`. Monty's
    /// `ExcType` enum models single parents, but [`Self::is_subclass_of`]
    /// matches `UnsupportedOperation` against both `OSError` and `ValueError`
    /// so `except ValueError:` and `except OSError:` both catch it as in
    /// CPython.
    #[strum(serialize = "io.UnsupportedOperation")]
    UnsupportedOperation,
    /// Subclass of OSError since Python 3.3 (PEP 3151).
    TimeoutError,

    // --- Standalone exception types ---
    AssertionError,
    MemoryError,
    StopIteration,
    SyntaxError,
    TypeError,

    // --- Module-specific exception types ---

    // --- re module ---
    /// `re.PatternError` - raised for invalid regex patterns or unsupported regex features.
    ///
    /// # Behavior Note
    ///
    /// Limited to monty's exception type, `PatternError` does not provide `pattern`, `pos`,
    /// `lineno` and `colno` attributes.
    ///
    /// As per CPython's implementation, it would be hard to convert `fancy-regex`'s error
    /// representations into the required attributes.
    #[strum(serialize = "re.PatternError")]
    RePatternError,
}

impl ExcType {
    /// Checks if this exception type is a subclass of another exception type.
    ///
    /// Implements Python's exception hierarchy for try/except matching:
    /// - `Exception` is the base class for all standard exceptions
    /// - `LookupError` is the base for `KeyError` and `IndexError`
    /// - `ArithmeticError` is the base for `ZeroDivisionError` and `OverflowError`
    /// - `RuntimeError` is the base for `RecursionError` and `NotImplementedError`
    ///
    /// Returns true if `self` would be caught by `except handler_type:`.
    #[must_use]
    pub fn is_subclass_of(self, handler_type: Self) -> bool {
        if self == handler_type {
            return true;
        }
        match handler_type {
            // BaseException catches all exceptions
            Self::BaseException => true,
            // Exception catches everything except BaseException, and direct subclasses: KeyboardInterrupt, SystemExit
            Self::Exception => !matches!(self, Self::BaseException | Self::KeyboardInterrupt | Self::SystemExit),
            // LookupError catches KeyError and IndexError
            Self::LookupError => matches!(self, Self::KeyError | Self::IndexError),
            // ArithmeticError catches ZeroDivisionError and OverflowError
            Self::ArithmeticError => matches!(self, Self::ZeroDivisionError | Self::OverflowError),
            // RuntimeError catches RecursionError and NotImplementedError
            Self::RuntimeError => matches!(self, Self::RecursionError | Self::NotImplementedError),
            // AttributeError catches FrozenInstanceError
            Self::AttributeError => matches!(self, Self::FrozenInstanceError),
            // NameError catches UnboundLocalError
            Self::NameError => matches!(self, Self::UnboundLocalError),
            // ValueError catches UnicodeDecodeError, UnicodeEncodeError, json.JSONDecodeError,
            // and io.UnsupportedOperation (which in CPython has dual OSError + ValueError parentage)
            Self::ValueError => matches!(
                self,
                Self::UnicodeDecodeError
                    | Self::UnicodeEncodeError
                    | Self::JsonDecodeError
                    | Self::UnsupportedOperation
            ),
            // ImportError catches ModuleNotFoundError
            Self::ImportError => matches!(self, Self::ModuleNotFoundError),
            // OSError catches FileNotFoundError, FileExistsError, IsADirectoryError,
            // NotADirectoryError, PermissionError, io.UnsupportedOperation, and
            // TimeoutError (an OSError subclass since Python 3.3)
            Self::OSError => matches!(
                self,
                Self::FileNotFoundError
                    | Self::FileExistsError
                    | Self::IsADirectoryError
                    | Self::NotADirectoryError
                    | Self::PermissionError
                    | Self::UnsupportedOperation
                    | Self::TimeoutError
            ),
            // All other types only match exactly (handled by self == handler_type above)
            _ => false,
        }
    }

    /// Creates an exception instance from an exception type and arguments.
    ///
    /// Handles exception constructors like `ValueError('message')`.
    /// Currently supports zero or one string argument.
    ///
    /// The `interns` parameter provides access to interned string content.
    /// Returns a heap-allocated exception value.
    pub(crate) fn call(self, vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        defer_drop!(args, vm);
        let exc = match args {
            ArgValues::Empty => Ok(SimpleException::new_none(self)),
            ArgValues::One(value) => match value {
                Value::InternString(string_id) => Ok(SimpleException::new_msg(
                    self,
                    vm.interns.get_str(*string_id).to_owned(),
                )),
                Value::Ref(heap_id) => {
                    if let HeapData::Str(s) = vm.heap.get(*heap_id) {
                        Ok(SimpleException::new_msg(self, s.as_str().to_owned()))
                    } else {
                        Err(RunError::internal(
                            "exceptions can only be called with zero or one string argument",
                        ))
                    }
                }
                _ => Err(RunError::internal(
                    "exceptions can only be called with zero or one string argument",
                )),
            },
            _ => Err(RunError::internal(
                "exceptions can only be called with zero or one string argument",
            )),
        }?;
        let heap_id = vm.heap.allocate(HeapData::Exception(exc))?;
        Ok(Value::Ref(heap_id))
    }

    /// Creates an AttributeError for when an attribute is not found (GET operation).
    ///
    /// Sets `hide_caret: true` because CPython doesn't show carets for attribute GET errors.
    #[must_use]
    pub(crate) fn attribute_error(type_name: impl Display, attr: &str) -> RunError {
        let exc = SimpleException::new_msg(
            Self::AttributeError,
            format!("'{type_name}' object has no attribute '{attr}'"),
        );
        RunError::Exc(ExceptionRaise {
            exc,
            frame: None,
            hide_caret: true, // CPython doesn't show carets for attribute GET errors
        })
    }

    /// Creates an AttributeError for a missing attribute on a class object.
    ///
    /// Matches CPython's wording for type objects: `type object 'Foo' has no
    /// attribute 'nope'` (instances use [`Self::attribute_error`] instead).
    /// Sets `hide_caret: true` because CPython doesn't show carets for attribute GET errors.
    #[must_use]
    pub(crate) fn attribute_error_type(class_name: &str, attr: &str) -> RunError {
        let exc = SimpleException::new_msg(
            Self::AttributeError,
            format!("type object '{class_name}' has no attribute '{attr}'"),
        );
        RunError::Exc(ExceptionRaise {
            exc,
            frame: None,
            hide_caret: true, // CPython doesn't show carets for attribute GET errors
        })
    }

    /// Creates an AttributeError for attribute assignment on types that don't support it.
    ///
    /// Matches CPython's format for setting attributes on built-in types.
    #[must_use]
    pub(crate) fn attribute_error_no_setattr(type_: &str, attr_name: &str) -> RunError {
        SimpleException::new_msg(
            Self::AttributeError,
            format!("'{type_}' object has no attribute '{attr_name}' and no __dict__ for setting new attributes"),
        )
        .into()
    }

    /// Creates an AttributeError for a missing module attribute.
    ///
    /// Matches CPython's format: `AttributeError: module 'name' has no attribute 'attr'`
    /// Sets `hide_caret: true` because CPython doesn't show carets for attribute GET errors.
    #[must_use]
    pub(crate) fn attribute_error_module(module_name: &str, attr_name: &str) -> RunError {
        let exc = SimpleException::new_msg(
            Self::AttributeError,
            format!("module '{module_name}' has no attribute '{attr_name}'"),
        );
        RunError::Exc(ExceptionRaise {
            exc,
            frame: None,
            hide_caret: true, // CPython doesn't show carets for attribute GET errors
        })
    }

    #[must_use]
    pub(crate) fn type_error_not_sub(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("'{type_}' object is not subscriptable")).into()
    }

    /// Creates the TypeError for an ordering comparison (`<`, `<=`, `>`, `>=`)
    /// between values whose types define no ordering, e.g. `1 < 'a'` or two
    /// instances of a user class without comparison dunders.
    ///
    /// Matches CPython's format:
    /// `TypeError: '{op}' not supported between instances of '{left}' and '{right}'`
    #[must_use]
    pub(crate) fn type_error_ordering(operator: &str, left_type: &str, right_type: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("'{operator}' not supported between instances of '{left_type}' and '{right_type}'"),
        )
        .into()
    }

    /// Creates a TypeError for awaiting a non-awaitable object.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object can't be awaited`
    #[must_use]
    pub(crate) fn object_not_awaitable(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("'{type_}' object can't be awaited")).into()
    }

    /// Creates the canonical `RuntimeError: cannot reuse already awaited coroutine`,
    /// raised on direct re-await and on cross-gather coroutine reuse.
    #[must_use]
    pub(crate) fn cannot_reuse_already_awaited_coroutine() -> RunError {
        SimpleException::new_msg(Self::RuntimeError, "cannot reuse already awaited coroutine").into()
    }

    /// Creates a TypeError for item assignment on types that don't support it.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object does not support item assignment`
    #[must_use]
    pub(crate) fn type_error_not_sub_assignment(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("'{type_}' object does not support item assignment"),
        )
        .into()
    }

    /// Creates a TypeError for unhashable types when calling `hash()`.
    ///
    /// This matches Python 3.14's error message: `TypeError: unhashable type: 'list'`
    #[must_use]
    pub(crate) fn type_error_unhashable(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("unhashable type: '{type_}'")).into()
    }

    /// Creates a TypeError for unhashable types used as dict keys.
    ///
    /// This matches Python 3.14's error message:
    /// `TypeError: cannot use 'list' as a dict key (unhashable type: 'list')`
    #[must_use]
    pub(crate) fn type_error_unhashable_dict_key(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("cannot use '{type_}' as a dict key (unhashable type: '{type_}')"),
        )
        .into()
    }

    /// Creates a TypeError for unhashable types used as set elements.
    ///
    /// This matches Python 3.14's error message:
    /// `TypeError: cannot use 'list' as a set element (unhashable type: 'list')`
    #[must_use]
    pub(crate) fn type_error_unhashable_set_element(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("cannot use '{type_}' as a set element (unhashable type: '{type_}')"),
        )
        .into()
    }

    /// Creates a KeyError for a missing dict key.
    ///
    /// For string keys, uses the raw string value without extra quoting.
    /// If the key's string conversion fails (e.g. huge LongInt exceeding
    /// `INT_MAX_STR_DIGITS`), falls back to the type name so that a
    /// `KeyError` is always raised rather than a spurious `ValueError`.
    pub(crate) fn key_error(key: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> RunError {
        let key_str = match key.py_str(vm) {
            Ok(key_value) => {
                // `key_value` is a heap `str` `Value`; extract its text and drop it.
                defer_drop!(key_value, vm);
                if let Ok(s) = key_value.to_str(vm) {
                    s.to_owned()
                } else {
                    format!("<{}>", key.py_type_name(vm))
                }
            }
            Err(_) => format!("<{}>", key.py_type_name(vm)),
        };
        SimpleException::new_msg(Self::KeyError, key_str).into()
    }

    /// Creates a KeyError for popping from an empty set.
    ///
    /// Matches CPython's error format: `KeyError: 'pop from an empty set'`
    #[must_use]
    pub(crate) fn key_error_pop_empty_set() -> RunError {
        SimpleException::new_msg(Self::KeyError, "pop from an empty set").into()
    }

    /// Creates a TypeError for when a function receives the wrong number of arguments.
    ///
    /// Matches CPython's error format exactly:
    /// - For 1 expected arg: `{name}() takes exactly one argument ({actual} given)`
    /// - For N expected args: `{name} expected {expected} arguments, got {actual}`
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "len" for builtins, "list.append" for methods)
    /// * `expected` - Number of expected arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub(crate) fn type_error_arg_count(name: &str, expected: usize, actual: usize) -> RunError {
        if expected == 1 {
            // CPython: "len() takes exactly one argument (2 given)"
            SimpleException::new_msg(
                Self::TypeError,
                format!("{name}() takes exactly one argument ({actual} given)"),
            )
            .into()
        } else {
            // CPython: "insert expected 2 arguments, got 1"
            SimpleException::new_msg(
                Self::TypeError,
                format!("{name} expected {expected} arguments, got {actual}"),
            )
            .into()
        }
    }

    /// Creates a TypeError for when a method that takes no arguments receives some.
    ///
    /// Matches CPython's format: `{name}() takes no arguments ({actual} given)`
    ///
    /// # Arguments
    /// * `name` - The method name (e.g., "dict.keys")
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub(crate) fn type_error_no_args(name: &str, actual: usize) -> RunError {
        // CPython: "dict.keys() takes no arguments (1 given)"
        SimpleException::new_msg(Self::TypeError, format!("{name}() takes no arguments ({actual} given)")).into()
    }

    /// Creates a TypeError for when a function receives fewer arguments than required.
    ///
    /// Matches CPython's format: `{name} expected at least {min} argument, got {actual}`
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "get", "pop")
    /// * `min` - Minimum number of required arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub(crate) fn type_error_at_least(name: &str, min: usize, actual: usize) -> RunError {
        // CPython: "get expected at least 1 argument, got 0"
        let plural = if min == 1 { "" } else { "s" };
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name} expected at least {min} argument{plural}, got {actual}"),
        )
        .into()
    }

    /// Creates a TypeError for when a function receives more arguments than allowed.
    ///
    /// Matches CPython's `PyArg_UnpackTuple` format:
    /// - `{name} expected at most {max} argument, got {actual}` (singular when max=1)
    /// - `{name} expected at most {max} arguments, got {actual}` (plural otherwise)
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "get", "pop")
    /// * `max` - Maximum number of allowed arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub(crate) fn type_error_at_most(name: &str, max: usize, actual: usize) -> RunError {
        let plural = if max == 1 { "" } else { "s" };
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name} expected at most {max} argument{plural}, got {actual}"),
        )
        .into()
    }

    /// Creates a TypeError for a `startswith`/`endswith` affix argument that is
    /// neither the expected string type nor a tuple.
    ///
    /// Matches CPython: `{method} first arg must be {expected} or a tuple of {expected}, not {type}`
    /// (`expected` is `str` for `str` methods, `bytes` for `bytes` methods).
    #[must_use]
    pub(crate) fn type_error_affix_arg(method: &str, expected: &str, type_name: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{method} first arg must be {expected} or a tuple of {expected}, not {type_name}"),
        )
        .into()
    }

    /// Creates a TypeError for a non-string element in a `startswith`/`endswith`
    /// affix tuple.
    ///
    /// Matches CPython: `tuple for {method} must only contain {expected}, not {type}`.
    /// Raised lazily while matching — elements after a successful match are never checked.
    #[must_use]
    pub(crate) fn type_error_affix_tuple_item(method: &str, expected: &str, type_name: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("tuple for {method} must only contain {expected}, not {type_name}"),
        )
        .into()
    }

    /// Creates a TypeError for too many arguments to a method or named function.
    ///
    /// Matches CPython's format for method-style calls:
    /// `{name}() takes at most {max} argument ({actual} given)` (singular when max=1)
    /// `{name}() takes at most {max} arguments ({actual} given)` (plural otherwise)
    ///
    /// Use this instead of `type_error_at_most` for methods and type constructors that
    /// CPython formats with parentheses, e.g. `now()`, `timezone()`, `expandtabs()`.
    #[must_use]
    pub(crate) fn type_error_method_at_most(name: &str, max: usize, actual: usize) -> RunError {
        let plural = if max == 1 { "" } else { "s" };
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() takes at most {max} argument{plural} ({actual} given)"),
        )
        .into()
    }

    /// Creates a TypeError for too few positional arguments to a method-style call.
    ///
    /// Matches CPython's format used by methods like `str.replace`:
    /// `{name}() takes at least {min} positional argument ({actual} given)` (singular when min=1)
    /// `{name}() takes at least {min} positional arguments ({actual} given)` (plural otherwise)
    ///
    /// Distinct from [`type_error_at_least`] which uses CPython's
    /// `PyArg_UnpackTuple` wording (no parens, no "positional"). Emitted by
    /// `FromArgs` for any struct with required positional-only fields,
    /// matching CPython's C-method `_PyArg_UnpackKeywords` dispatch.
    #[must_use]
    pub(crate) fn type_error_at_least_positional(name: &str, min: usize, actual: usize) -> RunError {
        let plural = if min == 1 { "" } else { "s" };
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() takes at least {min} positional argument{plural} ({actual} given)"),
        )
        .into()
    }

    /// Creates the bespoke `map()` arity error that CPython hard-codes.
    ///
    /// CPython's `map()` uses a single fixed message regardless of whether
    /// 0 or 1 args were given: `map() must have at least two arguments.`
    /// (note the trailing period). We mirror it here so `map()` / `map(f)`
    /// match byte-for-byte rather than falling through to the generic
    /// "missing N required positional arguments" wording the macro would
    /// otherwise emit.
    #[must_use]
    pub(crate) fn type_error_map_arity() -> RunError {
        SimpleException::new_msg(Self::TypeError, "map() must have at least two arguments.".to_owned()).into()
    }

    /// Creates a TypeError for exact-arity functions reporting too many *or* too few.
    ///
    /// Matches CPython's `PyArg_UnpackTuple` wording for fixed-arity callables
    /// (e.g. `sorted`):
    /// `{name} expected {exact} argument, got {actual}` (singular when exact=1)
    /// `{name} expected {exact} arguments, got {actual}` (plural otherwise)
    ///
    /// Use this for the macro's `expected_exact` attribute.
    #[must_use]
    pub(crate) fn type_error_expected_exact(name: &str, exact: usize, actual: usize) -> RunError {
        let plural = if exact == 1 { "" } else { "s" };
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name} expected {exact} argument{plural}, got {actual}"),
        )
        .into()
    }

    /// Creates a TypeError for missing positional arguments.
    ///
    /// Matches CPython's format: `{name}() missing {count} required positional argument(s): 'a' and 'b'`
    #[must_use]
    pub(crate) fn type_error_missing_positional_with_names(name: &str, missing_names: &[&str]) -> RunError {
        SimpleException::new_msg(Self::TypeError, Self::missing_positional_msg(name, missing_names)).into()
    }

    /// Message body for [`type_error_missing_positional_with_names`], exposed
    /// separately so `args/bind_python.rs` can attach a call position to the same
    /// CPython-exact wording.
    #[must_use]
    pub(crate) fn missing_positional_msg(name: &str, missing_names: &[&str]) -> String {
        let count = missing_names.len();
        let names_str = format_param_names(missing_names);
        if count == 1 {
            format!("{name}() missing 1 required positional argument: {names_str}")
        } else {
            format!("{name}() missing {count} required positional arguments: {names_str}")
        }
    }

    /// Creates a TypeError for missing keyword-only arguments.
    ///
    /// Matches CPython's format: `{name}() missing {count} required keyword-only argument(s): 'a' and 'b'`
    #[must_use]
    pub(crate) fn type_error_missing_kwonly_with_names(name: &str, missing_names: &[&str]) -> RunError {
        let count = missing_names.len();
        let names_str = format_param_names(missing_names);
        if count == 1 {
            SimpleException::new_msg(
                Self::TypeError,
                format!("{name}() missing 1 required keyword-only argument: {names_str}"),
            )
            .into()
        } else {
            SimpleException::new_msg(
                Self::TypeError,
                format!("{name}() missing {count} required keyword-only arguments: {names_str}"),
            )
            .into()
        }
    }

    /// Creates a TypeError for too many positional arguments to a callable whose
    /// positional count is a range (some positional parameters have defaults).
    ///
    /// Matches CPython's pure-Python `def` wording: when `min == max` it emits
    /// `{name}() takes {max} positional argument(s) but {actual} was/were given`;
    /// otherwise `{name}() takes from {min} to {max} positional arguments but
    /// {actual} were given` (always-plural "arguments" in the range form). Both
    /// get the `(and N keyword-only argument(s))` suffix when `kwonly_given > 0`.
    /// Used by `FromArgs` structs marked `py_def` and by user `def` bindings.
    #[must_use]
    pub(crate) fn type_error_too_many_positional_range(
        name: &str,
        min: usize,
        max: usize,
        actual: usize,
        kwonly_given: usize,
    ) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            Self::too_many_positional_range_msg(name, min, max, actual, kwonly_given),
        )
        .into()
    }

    /// Message body for [`type_error_too_many_positional_range`], exposed
    /// separately so `args/bind_python.rs` can attach a call position to the same
    /// CPython-exact wording.
    #[must_use]
    pub(crate) fn too_many_positional_range_msg(
        name: &str,
        min: usize,
        max: usize,
        actual: usize,
        kwonly_given: usize,
    ) -> String {
        let takes = if min == max {
            // `max == 0` still reads "0 positional arguments" (plural) in CPython.
            let takes_word = if max == 1 { "argument" } else { "arguments" };
            format!("{max} positional {takes_word}")
        } else {
            format!("from {min} to {max} positional arguments")
        };
        if kwonly_given > 0 {
            // CPython includes keyword-only args in the "given" part when present
            let given_word = if actual == 1 { "argument" } else { "arguments" };
            let kwonly_word = if kwonly_given == 1 { "argument" } else { "arguments" };
            format!(
                "{name}() takes {takes} but {actual} positional {given_word} (and {kwonly_given} keyword-only {kwonly_word}) were given"
            )
        } else {
            let was_were = if actual == 1 { "was" } else { "were" };
            format!("{name}() takes {takes} but {actual} {was_were} given")
        }
    }

    /// Creates a TypeError for positional-only parameter passed as keyword.
    ///
    /// Matches CPython's format: `{name}() got some positional-only arguments passed as keyword arguments: '{param}'`
    #[must_use]
    pub(crate) fn type_error_positional_only(name: &str, param: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() got some positional-only arguments passed as keyword arguments: '{param}'"),
        )
        .into()
    }

    /// Creates a TypeError for duplicate argument.
    ///
    /// Matches CPython's format: `{name}() got multiple values for argument '{param}'`
    #[must_use]
    pub(crate) fn type_error_duplicate_arg(name: &str, param: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() got multiple values for argument '{param}'"),
        )
        .into()
    }

    /// Creates a TypeError for duplicate keyword argument.
    ///
    /// Matches CPython's format: `{name}() got multiple values for keyword argument '{key}'`
    #[must_use]
    pub(crate) fn type_error_multiple_values(name: &str, key: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() got multiple values for keyword argument '{key}'"),
        )
        .into()
    }

    /// Creates a TypeError for when a positional argument conflicts with a keyword argument
    /// of the same name in a C-implemented type constructor.
    ///
    /// Matches CPython's `PyArg_ParseTupleAndKeywords` format:
    /// `argument for function given by name ('{key}') and position ({pos})`
    ///
    /// The position is 1-indexed, matching CPython's convention. The `func_descriptor` is
    /// typically `"function"` for most C types (like `datetime`), matching CPython's generic
    /// wording for `PyArg_ParseTupleAndKeywords`.
    #[must_use]
    pub(crate) fn type_error_positional_keyword_conflict(func_descriptor: &str, key: &str, pos: usize) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("argument for {func_descriptor} given by name ('{key}') and position ({pos})"),
        )
        .into()
    }

    /// Creates a TypeError for unexpected keyword argument.
    ///
    /// Matches CPython's format: `{name}() got an unexpected keyword argument '{key}'`
    #[must_use]
    pub(crate) fn type_error_unexpected_keyword(name: &str, key: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() got an unexpected keyword argument '{key}'"),
        )
        .into()
    }

    /// Creates a TypeError for unexpected keyword argument in C-implemented types.
    ///
    /// Matches CPython's `PyArg_ParseTupleAndKeywords` format:
    /// `this function got an unexpected keyword argument '{key}'`
    #[must_use]
    pub(crate) fn type_error_c_unexpected_keyword(key: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("this function got an unexpected keyword argument '{key}'"),
        )
        .into()
    }

    /// Creates a TypeError for too many arguments to a C-implemented type.
    ///
    /// Matches CPython's `PyArg_ParseTupleAndKeywords` format:
    /// `function takes at most {max} arguments ({actual} given)`
    #[must_use]
    pub(crate) fn type_error_c_at_most(max: usize, actual: usize) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("function takes at most {max} arguments ({actual} given)"),
        )
        .into()
    }

    /// Variant of [`type_error_c_at_most`] used by C constructors that explicitly
    /// say "positional arguments" (e.g. `datetime`):
    /// `function takes at most {max} positional arguments ({actual} given)`
    #[must_use]
    pub(crate) fn type_error_c_at_most_positional(max: usize, actual: usize) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("function takes at most {max} positional arguments ({actual} given)"),
        )
        .into()
    }

    /// Hybrid wording used by C constructors that mix positional-or-keyword
    /// slots with keyword-only slots (e.g. `datetime`, where `fold` is
    /// keyword-only). CPython's `PyArg_ParseTupleAndKeywords` emits two
    /// distinct messages depending on whether the caller could conceivably
    /// have meant the overflow args to fill the keyword-only tail:
    ///
    /// - `actual <= max_total`: the extra positionals *could* have filled the
    ///   trailing keyword-only slots, so the error pins blame on positional
    ///   overflow specifically — `function takes at most {max_pos} positional
    ///   arguments ({actual} given)`.
    /// - `actual > max_total`: there is no slot of any kind for the extras,
    ///   so the error reports the total slot count without the "positional"
    ///   qualifier — `function takes at most {max_total} arguments ({actual}
    ///   given)`.
    ///
    /// `max_pos` is the number of positional-or-keyword slots; `max_total`
    /// adds the trailing keyword-only slot count. When `max_total == max_pos`
    /// (no kw-only fields) this collapses to [`type_error_c_at_most_positional`].
    #[must_use]
    pub(crate) fn type_error_c_at_most_positional_or_total(
        max_pos: usize,
        max_total: usize,
        actual: usize,
    ) -> RunError {
        if actual > max_total && max_total > max_pos {
            Self::type_error_c_at_most(max_total, actual)
        } else {
            Self::type_error_c_at_most_positional(max_pos, actual)
        }
    }

    /// Creates a TypeError for a missing required argument in a C-implemented type.
    ///
    /// Matches CPython's `PyArg_ParseTupleAndKeywords` format:
    /// `function missing required argument '{arg_name}' (pos {pos})`
    #[must_use]
    pub(crate) fn type_error_c_missing_required(arg_name: &str, pos: usize) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("function missing required argument '{arg_name}' (pos {pos})"),
        )
        .into()
    }

    /// Creates a TypeError for a missing required argument in a C-implemented type,
    /// with a function name prefix.
    ///
    /// Matches CPython's format for types like `timezone`:
    /// `{name}() missing required argument '{arg_name}' (pos {pos})`
    #[must_use]
    pub(crate) fn type_error_c_missing_required_named(name: &str, arg_name: &str, pos: usize) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() missing required argument '{arg_name}' (pos {pos})"),
        )
        .into()
    }

    /// Creates a TypeError matching CPython's `_PyArg_BadArgument`
    /// positional-style wording: `{name}() argument {pos} must be
    /// {expected}, not {got}`.
    ///
    /// Used by the `#[derive(FromArgs)]` macro when the struct opts into
    /// `bad_arg` errors — emitted in place of the inner [`FromValue`]
    /// failure so the caller sees the same wording as CPython's C-implemented
    /// functions (e.g. `strftime() argument 1 must be str, not None`).
    ///
    /// The `got` type label should come from `Type::cpython_arg_name` so
    /// that `NoneType` becomes `"None"` to match CPython's special case.
    ///
    /// [`FromValue`]: crate::args::FromValue
    #[must_use]
    pub(crate) fn type_error_bad_arg_pos(name: &str, pos: usize, expected: &str, got: impl fmt::Display) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() argument {pos} must be {expected}, not {got}"),
        )
        .into()
    }

    /// Creates a TypeError matching CPython's `_PyArg_BadArgument`
    /// named-style wording: `{name}() argument '{arg_name}' must be
    /// {expected}, not {got}`.
    ///
    /// CPython uses this form for C-implemented functions that register
    /// their arguments by name (`open`, `str.encode`, `bytes.decode`, …).
    /// Sibling to [`type_error_bad_arg_pos`]; pick the variant matching the
    /// CPython output for the function being modelled.
    #[must_use]
    pub(crate) fn type_error_bad_arg_named(
        name: &str,
        arg_name: &str,
        expected: &str,
        got: impl fmt::Display,
    ) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() argument '{arg_name}' must be {expected}, not {got}"),
        )
        .into()
    }

    /// Creates a TypeError for **kwargs argument that is not a mapping.
    ///
    /// Matches CPython's format: `{name}() argument after ** must be a mapping, not {type_name}`
    #[must_use]
    pub(crate) fn type_error_kwargs_not_mapping(name: &str, type_name: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{name}() argument after ** must be a mapping, not {type_name}"),
        )
        .into()
    }

    /// Creates a TypeError for `{**x}` dict-literal unpacking where `x` is not a mapping.
    ///
    /// Matches CPython's format: `'{type_name}' object is not a mapping`
    ///
    /// Note: this differs from [`type_error_kwargs_not_mapping`] which is used for
    /// function-call `**kwargs` and includes the function name in the message.
    #[must_use]
    pub(crate) fn type_error_not_mapping(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("'{type_}' object is not a mapping")).into()
    }

    /// Creates a TypeError for **kwargs with non-string keys.
    ///
    /// Matches CPython's format: `{name}() keywords must be strings`
    #[must_use]
    pub(crate) fn type_error_kwargs_nonstring_key() -> RunError {
        SimpleException::new_msg(Self::TypeError, "keywords must be strings").into()
    }

    /// Creates a TypeError for an invalid `tzinfo` argument.
    ///
    /// Matches CPython: `tzinfo argument must be None or of a tzinfo subclass, not type 'int'`
    #[must_use]
    pub(crate) fn type_error_tzinfo(ty: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("tzinfo argument must be None or of a tzinfo subclass, not type '{ty}'"),
        )
        .into()
    }

    /// Creates a simple TypeError with a custom message.
    #[must_use]
    pub(crate) fn type_error(msg: impl fmt::Display) -> RunError {
        SimpleException::new_msg(Self::TypeError, msg).into()
    }

    /// Creates the TypeError raised when a `with` statement's context expression
    /// does not implement the context-manager protocol.
    ///
    /// Matches CPython 3.14's wording, which names the specific missing dunder:
    /// `__exit__` when the protocol check fails outright (`BeforeWith`'s
    /// [`py_is_context_manager`](crate::types::PyTrait::py_is_context_manager)
    /// gate), `__enter__` when a user class defines `__exit__` but not
    /// `__enter__`.
    #[must_use]
    pub(crate) fn type_error_not_context_manager(type_name: impl Display, missing_dunder: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!(
                "'{type_name}' object does not support the context manager protocol (missed {missing_dunder} method)"
            ),
        )
        .into()
    }

    /// Creates a TypeError for `__init__` returning a value other than `None`.
    ///
    /// Matches CPython's format: `TypeError: __init__() should return None, not '{type}'`
    #[must_use]
    pub(crate) fn type_error_init_return(type_name: impl Display) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("__init__() should return None, not '{type_name}'"),
        )
        .into()
    }

    /// Creates a generic `ValueError` with a custom message.
    pub(crate) fn value_error(msg: impl fmt::Display) -> RunError {
        SimpleException::new_msg(Self::ValueError, msg).into()
    }

    /// Creates a TypeError for bytes() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: cannot convert '{type}' object to bytes`
    #[must_use]
    pub(crate) fn type_error_bytes_init(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("cannot convert '{type_}' object to bytes")).into()
    }

    /// Creates a TypeError for calling a non-callable type.
    ///
    /// Matches CPython's format: `TypeError: cannot create '{type}' instances`
    #[must_use]
    pub(crate) fn type_error_not_callable(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("cannot create '{type_}' instances")).into()
    }

    /// Creates a TypeError for calling a non-callable object.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object is not callable`
    #[must_use]
    pub(crate) fn type_error_not_callable_object(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("'{type_}' object is not callable")).into()
    }

    /// Creates a TypeError for non-iterable type in list/tuple/etc constructors.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object is not iterable`
    #[must_use]
    pub(crate) fn type_error_not_iterable(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("'{type_}' object is not iterable")).into()
    }

    /// Creates a TypeError for non-iterable type in PEP 448 `*value` literal unpack.
    ///
    /// Used when `[*expr]`, `(*expr,)` literal unpack encounters a non-iterable — distinct
    /// from [`type_error_not_iterable`] because CPython uses a different message for this context.
    ///
    /// Matches CPython's format: `TypeError: Value after * must be an iterable, not {type}`
    #[must_use]
    pub(crate) fn type_error_value_after_star(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("Value after * must be an iterable, not {type_}"),
        )
        .into()
    }

    /// Creates a TypeError for int() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: int() argument must be a string, a bytes-like object or a real number, not '{type}'`
    #[must_use]
    pub(crate) fn type_error_int_conversion(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("int() argument must be a string, a bytes-like object or a real number, not '{type_}'"),
        )
        .into()
    }

    /// Creates a TypeError for float() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: float() argument must be a string or a real number, not '{type}'`
    #[must_use]
    pub(crate) fn type_error_float_conversion(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("float() argument must be a string or a real number, not '{type_}'"),
        )
        .into()
    }

    /// Creates a ValueError for negative count in bytes().
    ///
    /// Matches CPython's format: `ValueError: negative count`
    #[must_use]
    pub(crate) fn value_error_negative_bytes_count() -> RunError {
        SimpleException::new_msg(Self::ValueError, "negative count").into()
    }

    /// Creates a TypeError for isinstance() arg 2.
    ///
    /// Matches CPython's format: `TypeError: isinstance() arg 2 must be a type, a tuple of types, or a union`
    #[must_use]
    pub(crate) fn isinstance_arg2_error() -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            "isinstance() arg 2 must be a type, a tuple of types, or a union",
        )
        .into()
    }

    /// Creates a TypeError for invalid exception type in except clause.
    ///
    /// Matches CPython's format: `TypeError: catching classes that do not inherit from BaseException is not allowed`
    #[must_use]
    pub(crate) fn except_invalid_type_error() -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            "catching classes that do not inherit from BaseException is not allowed",
        )
        .into()
    }

    /// Creates a ValueError for range() step argument being zero.
    ///
    /// Matches CPython's format: `ValueError: range() arg 3 must not be zero`
    #[must_use]
    pub(crate) fn value_error_range_step_zero() -> RunError {
        SimpleException::new_msg(Self::ValueError, "range() arg 3 must not be zero").into()
    }

    /// Creates a ValueError for slice step being zero.
    ///
    /// Matches CPython's format: `ValueError: slice step cannot be zero`
    #[must_use]
    pub(crate) fn value_error_slice_step_zero() -> RunError {
        SimpleException::new_msg(Self::ValueError, "slice step cannot be zero").into()
    }

    /// Creates a TypeError for slice indices that are not integers or None.
    ///
    /// Matches CPython's format: `TypeError: slice indices must be integers or None or have an __index__ method`
    #[must_use]
    pub(crate) fn type_error_slice_indices() -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            "slice indices must be integers or None or have an __index__ method",
        )
        .into()
    }

    /// Creates a RuntimeError for dict mutation during iteration.
    ///
    /// Matches CPython's format: `RuntimeError: dictionary changed size during iteration`
    #[must_use]
    pub(crate) fn runtime_error_dict_changed_size() -> RunError {
        SimpleException::new_msg(Self::RuntimeError, "dictionary changed size during iteration").into()
    }

    /// Creates a TypeError for `reversed()` on a non-reversible object.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object is not reversible`
    #[must_use]
    pub(crate) fn type_error_not_reversible(type_: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("'{type_}' object is not reversible")).into()
    }

    /// Creates a RuntimeError for an over-deep iterator delegation chain.
    ///
    /// Monty-specific (CPython builds no delegation chain at all) — see
    /// `limitations/builtins.md`.
    #[must_use]
    pub(crate) fn runtime_error_iter_delegation_too_deep() -> RunError {
        SimpleException::new_msg(Self::RuntimeError, "iterator delegation nested too deeply").into()
    }

    /// Creates a RuntimeError for a delegating iterator pointing at a non-iterator.
    ///
    /// Unreachable from Python; only a malformed snapshot can produce it. Raised
    /// rather than panicking so untrusted snapshot data cannot abort the process.
    #[must_use]
    pub(crate) fn runtime_error_iter_delegation_invalid() -> RunError {
        SimpleException::new_msg(Self::RuntimeError, "iterator delegates to a non-iterator").into()
    }

    /// Creates a RuntimeError for set mutation during iteration.
    ///
    /// Matches CPython's format: `RuntimeError: Set changed size during iteration`
    #[must_use]
    pub(crate) fn runtime_error_set_changed_size() -> RunError {
        SimpleException::new_msg(Self::RuntimeError, "Set changed size during iteration").into()
    }

    /// Creates a TypeError for functions that don't accept keyword arguments.
    ///
    /// Matches CPython's format: `TypeError: {name}() takes no keyword arguments`
    #[must_use]
    pub(crate) fn type_error_no_kwargs(name: &str) -> RunError {
        SimpleException::new_msg(Self::TypeError, format!("{name}() takes no keyword arguments")).into()
    }

    /// Creates a NotImplementedError for functions that don't accept keyword arguments.
    #[must_use]
    pub(crate) fn kwargs_not_implemented(name: &str) -> RunError {
        SimpleException::new_msg(
            Self::NotImplementedError,
            format!("{name}() does not yet support keyword arguments"),
        )
        .into()
    }

    /// Creates an IndexError for list index out of range (getitem).
    ///
    /// Matches CPython's format: `IndexError('list index out of range')`
    #[must_use]
    pub(crate) fn list_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "list index out of range").into()
    }

    /// Creates an IndexError for list assignment index out of range (setitem).
    ///
    /// Matches CPython's format: `IndexError('list assignment index out of range')`
    #[must_use]
    pub(crate) fn list_assignment_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "list assignment index out of range").into()
    }

    /// Creates an IndexError for tuple index out of range.
    ///
    /// Matches CPython's format: `IndexError('tuple index out of range')`
    #[must_use]
    pub(crate) fn tuple_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "tuple index out of range").into()
    }

    /// Creates an IndexError for string index out of range.
    ///
    /// Matches CPython's format: `IndexError('string index out of range')`
    #[must_use]
    pub(crate) fn str_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "string index out of range").into()
    }

    /// Creates an IndexError for bytes index out of range.
    ///
    /// Matches CPython's format: `IndexError('index out of range')`
    #[must_use]
    pub(crate) fn bytes_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "index out of range").into()
    }

    /// Creates an IndexError for range index out of range.
    ///
    /// Matches CPython's format: `IndexError('range object index out of range')`
    #[must_use]
    pub(crate) fn range_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "range object index out of range").into()
    }

    /// Creates an IndexError for `re.Match` group index out of range.
    ///
    /// Matches CPython's format: `IndexError('no such group')`
    #[must_use]
    pub(crate) fn re_match_group_index_error() -> RunError {
        SimpleException::new_msg(Self::IndexError, "no such group").into()
    }

    /// Creates a TypeError for non-integer sequence indices (getitem).
    ///
    /// Matches CPython's format: `TypeError('{type}' indices must be integers, not '{index_type}')`
    #[must_use]
    pub(crate) fn type_error_indices(type_str: Type, index_type: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("{type_str} indices must be integers, not '{index_type}'"),
        )
        .into()
    }

    /// Creates a TypeError for non-integer list indices (setitem/assignment).
    ///
    /// Matches CPython's format: `TypeError('list indices must be integers or slices, not {index_type}')`
    #[must_use]
    pub(crate) fn type_error_list_assignment_indices(index_type: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("list indices must be integers or slices, not {index_type}"),
        )
        .into()
    }

    /// Creates a NameError for accessing a free variable (nonlocal/closure) before it's assigned.
    ///
    /// Matches CPython's format: `NameError: cannot access free variable 'x' where it is not
    /// associated with a value in enclosing scope`
    #[must_use]
    pub(crate) fn name_error_free_variable(name: &str) -> SimpleException {
        SimpleException::new_msg(
            Self::NameError,
            format!("cannot access free variable '{name}' where it is not associated with a value in enclosing scope"),
        )
    }

    /// Creates a NameError for accessing an undefined variable.
    ///
    /// Matches CPython's format: `NameError: name 'x' is not defined`
    #[must_use]
    pub(crate) fn name_error(name: &str) -> SimpleException {
        let mut msg = format!("name '{name}' is not defined");
        // add the same suffix as cpython, but only for the modules supported by Monty
        if matches!(name, "asyncio" | "sys" | "typing" | "types" | "re" | "json") {
            write!(&mut msg, ". Did you forget to import '{name}'?").unwrap();
        }
        SimpleException::new_msg(Self::NameError, msg)
    }

    /// Creates an UnboundLocalError for accessing a local variable before assignment.
    ///
    /// Matches CPython's format: `UnboundLocalError: cannot access local variable 'x' where it is not associated with a value`
    #[must_use]
    pub(crate) fn unbound_local_error(name: &str) -> SimpleException {
        SimpleException::new_msg(
            Self::UnboundLocalError,
            format!("cannot access local variable '{name}' where it is not associated with a value"),
        )
    }

    /// Creates a ModuleNotFoundError for when a module cannot be found.
    ///
    /// Matches CPython's format: `ModuleNotFoundError: No module named 'name'`
    /// Sets `hide_caret: true` because CPython doesn't show carets for module not found errors.
    #[must_use]
    pub(crate) fn module_not_found_error(module_name: &str) -> RunError {
        let exc = SimpleException::new_msg(Self::ModuleNotFoundError, format!("No module named '{module_name}'"));
        RunError::Exc(ExceptionRaise {
            exc,
            frame: None,
            hide_caret: true, // CPython doesn't show carets for module not found errors
        })
    }

    /// Creates a NotImplementedError for an unimplemented Python feature.
    ///
    /// Used during parsing when encountering Python syntax that Monty doesn't yet support.
    /// The message format is: "The monty syntax parser does not yet support {feature}"
    #[must_use]
    pub(crate) fn not_implemented(msg: impl fmt::Display) -> SimpleException {
        SimpleException::new_msg(Self::NotImplementedError, msg)
    }

    /// Creates a ZeroDivisionError for division by zero.
    ///
    /// Matches CPython 3.14's format: `ZeroDivisionError('division by zero')`
    #[must_use]
    pub(crate) fn zero_division() -> SimpleException {
        SimpleException::new_msg(Self::ZeroDivisionError, "division by zero")
    }

    /// Creates an OverflowError for string/sequence repetition with count too large.
    ///
    /// Matches CPython's format: `OverflowError('cannot fit 'int' into an index-sized integer')`
    #[must_use]
    pub(crate) fn overflow_repeat_count() -> SimpleException {
        SimpleException::new_msg(Self::OverflowError, "cannot fit 'int' into an index-sized integer")
    }

    /// Creates an IndexError for when an integer index is too large to fit in i64.
    ///
    /// Matches CPython's format: `IndexError: cannot fit 'int' into an index-sized integer`
    #[must_use]
    pub(crate) fn index_error_int_too_large() -> RunError {
        SimpleException::new_msg(Self::IndexError, "cannot fit 'int' into an index-sized integer").into()
    }

    /// Creates an ImportError for when a name cannot be imported from a module.
    ///
    /// Matches CPython's format for built-in modules:
    /// `ImportError: cannot import name 'name' from 'module' (unknown location)`
    ///
    /// Sets `hide_caret: true` because CPython doesn't show carets for import errors.
    #[must_use]
    pub(crate) fn cannot_import_name(name: &str, module_name: &str) -> RunError {
        let exc = SimpleException::new_msg(
            Self::ImportError,
            format!("cannot import name '{name}' from '{module_name}' (unknown location)"),
        );
        RunError::Exc(ExceptionRaise {
            exc,
            frame: None,
            hide_caret: true,
        })
    }

    /// Creates a ValueError when an integer is too large to convert to a decimal string.
    ///
    /// Matches CPython 3.11+'s `sys.int_max_str_digits` error message.
    #[must_use]
    pub(crate) fn value_error_int_too_large_for_str() -> RunError {
        SimpleException::new_msg(
            Self::ValueError,
            format!("Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion"),
        )
        .into()
    }

    /// Creates a ValueError when a decimal string has too many digits for `int()` conversion.
    ///
    /// Includes the actual digit count to help users diagnose the issue.
    #[must_use]
    pub(crate) fn value_error_int_str_too_large(digit_count: usize) -> RunError {
        SimpleException::new_msg(
            Self::ValueError,
            format!(
                "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {digit_count} digits"
            ),
        )
        .into()
    }

    /// Creates a ValueError for `int()` when a string cannot be parsed as an integer.
    ///
    /// Matches CPython's format: `invalid literal for int() with base 10: '...'`.
    /// The caller provides the value pre-formatted (e.g. via `StringRepr`).
    #[must_use]
    pub(crate) fn value_error_invalid_literal_for_int(value: impl fmt::Display) -> RunError {
        SimpleException::new_msg(
            Self::ValueError,
            format!("invalid literal for int() with base 10: {value}"),
        )
        .into()
    }

    /// Creates a ValueError for negative shift count in bitwise shift operations.
    ///
    /// Matches CPython's format: `ValueError: negative shift count`
    #[must_use]
    pub(crate) fn value_error_negative_shift_count() -> RunError {
        SimpleException::new_msg(Self::ValueError, "negative shift count").into()
    }

    /// Creates an OverflowError when converting values to C ssize_t (i64) for operations like length checks.
    ///
    /// Matches CPython's format: `OverflowError: Python int too large to convert to C ssize_t`
    /// Note: CPython uses this message because it tries to convert to ssize_t for the shift amount.
    #[must_use]
    pub(crate) fn overflow_c_ssize_t() -> RunError {
        SimpleException::new_msg(Self::OverflowError, "Python int too large to convert to C ssize_t").into()
    }

    /// Creates an OverflowError when a Python int doesn't fit into a C `int` (i32).
    ///
    /// Matches CPython's format: `OverflowError: Python int too large to convert to C int`
    /// Used by builtins (e.g. `bytes.hex`) that parse arguments via the `i` format code.
    #[must_use]
    pub(crate) fn overflow_c_int() -> RunError {
        SimpleException::new_msg(Self::OverflowError, "Python int too large to convert to C int").into()
    }

    /// Creates an OverflowError when a Python int doesn't fit into a C `long` (i64).
    ///
    /// Matches CPython's format: `OverflowError: Python int too large to convert to C long`
    /// CPython's `i` format code converts through C long first, so ints beyond
    /// i64 report this even for C-int parameters (e.g. `datetime.date(2**100, 1, 1)`).
    #[must_use]
    pub(crate) fn overflow_c_long() -> RunError {
        SimpleException::new_msg(Self::OverflowError, "Python int too large to convert to C long").into()
    }

    /// Creates a TypeError for unsupported binary operations.
    ///
    /// For `+` or `+=` with str/list on the left side, uses CPython's special format:
    /// `can only concatenate {type} (not "{other}") to {type}`
    ///
    /// For other cases, uses the generic format:
    /// `unsupported operand type(s) for {op}: '{left}' and '{right}'`
    #[must_use]
    pub(crate) fn binary_type_error(
        op: &str,
        lhs_type: Type,
        lhs_name: impl Display,
        rhs_name: impl Display,
    ) -> RunError {
        let message = if (op == "+" || op == "+=") && matches!(lhs_type, Type::Str | Type::List) {
            format!("can only concatenate {lhs_name} (not \"{rhs_name}\") to {lhs_name}")
        } else {
            format!("unsupported operand type(s) for {op}: '{lhs_name}' and '{rhs_name}'")
        };
        SimpleException::new_msg(Self::TypeError, message).into()
    }

    /// Creates a TypeError for unsupported unary operations.
    ///
    /// Uses CPython's format: `bad operand type for unary {op}: '{type}'`
    #[must_use]
    pub(crate) fn unary_type_error(op: &str, value_type: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("bad operand type for unary {op}: '{value_type}'"),
        )
        .into()
    }

    /// Creates a TypeError for functions that require an integer argument.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object cannot be interpreted as an integer`
    #[must_use]
    pub(crate) fn type_error_not_integer(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("'{type_}' object cannot be interpreted as an integer"),
        )
        .into()
    }

    /// Creates a ZeroDivisionError for zero raised to a negative power.
    ///
    /// Matches CPython's format: `ZeroDivisionError: zero to a negative power`
    /// Note: CPython uses the same message for both int and float zero ** negative.
    #[must_use]
    pub(crate) fn zero_negative_power() -> RunError {
        SimpleException::new_msg(Self::ZeroDivisionError, "zero to a negative power").into()
    }

    /// Creates an OverflowError for exponents that are too large.
    ///
    /// Matches CPython's format: `OverflowError: exponent too large`
    #[must_use]
    pub(crate) fn overflow_exponent_too_large() -> RunError {
        SimpleException::new_msg(Self::OverflowError, "exponent too large").into()
    }

    /// Creates a ZeroDivisionError for divmod by zero (both integer and float).
    ///
    /// Matches CPython's format: `ZeroDivisionError: division by zero`
    /// Note: CPython uses the same message for both integer and float divmod.
    #[must_use]
    pub(crate) fn divmod_by_zero() -> RunError {
        SimpleException::new_msg(Self::ZeroDivisionError, "division by zero").into()
    }

    /// Creates a TypeError for str.join() when an item is not a string.
    ///
    /// Matches CPython's format: `TypeError: sequence item {index}: expected str instance, {type} found`
    #[must_use]
    pub(crate) fn type_error_join_item(index: usize, item_type: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("sequence item {index}: expected str instance, {item_type} found"),
        )
        .into()
    }

    /// Creates a TypeError for str.join() when the argument is not iterable.
    ///
    /// Matches CPython's format: `TypeError: can only join an iterable`
    #[must_use]
    pub(crate) fn type_error_join_not_iterable() -> RunError {
        SimpleException::new_msg(Self::TypeError, "can only join an iterable").into()
    }

    /// Creates a ValueError for str.index()/str.rindex() when substring is not found.
    ///
    /// Matches CPython's format: `ValueError: substring not found`
    #[must_use]
    pub(crate) fn value_error_substring_not_found() -> RunError {
        SimpleException::new_msg(Self::ValueError, "substring not found").into()
    }

    /// Creates a ValueError for str.partition()/str.rpartition() with empty separator.
    ///
    /// Matches CPython's format: `ValueError: empty separator`
    #[must_use]
    pub(crate) fn value_error_empty_separator() -> RunError {
        SimpleException::new_msg(Self::ValueError, "empty separator").into()
    }

    /// Creates a TypeError for fillchar argument that is not a single character.
    ///
    /// Matches CPython's format: `TypeError: The fill character must be exactly one character long`
    #[must_use]
    pub(crate) fn type_error_fillchar_must_be_single_char() -> RunError {
        SimpleException::new_msg(Self::TypeError, "The fill character must be exactly one character long").into()
    }

    /// Creates a StopIteration exception for when an iterator is exhausted.
    ///
    /// Matches CPython's format: `StopIteration`
    #[must_use]
    pub(crate) fn stop_iteration() -> RunError {
        SimpleException::new_none(Self::StopIteration).into()
    }

    /// Creates a ValueError for list.index() when item is not found.
    ///
    /// Matches CPython's format: `ValueError: list.index(x): x not in list`
    #[must_use]
    pub(crate) fn value_error_not_in_list() -> RunError {
        SimpleException::new_msg(Self::ValueError, "list.index(x): x not in list").into()
    }

    /// Creates a ValueError for tuple.index() when item is not found.
    ///
    /// Matches CPython's format: `ValueError: tuple.index(x): x not in tuple`
    #[must_use]
    pub(crate) fn value_error_not_in_tuple() -> RunError {
        SimpleException::new_msg(Self::ValueError, "tuple.index(x): x not in tuple").into()
    }

    /// Creates a ValueError for list.remove() when item is not found.
    ///
    /// Matches CPython's format: `ValueError: list.remove(x): x not in list`
    #[must_use]
    pub(crate) fn value_error_remove_not_in_list() -> RunError {
        SimpleException::new_msg(Self::ValueError, "list.remove(x): x not in list").into()
    }

    /// Creates an IndexError for popping from an empty list.
    ///
    /// Matches CPython's format: `IndexError: pop from empty list`
    #[must_use]
    pub(crate) fn index_error_pop_empty_list() -> RunError {
        SimpleException::new_msg(Self::IndexError, "pop from empty list").into()
    }

    /// Creates an IndexError for list.pop(index) with invalid index.
    ///
    /// Matches CPython's format: `IndexError: pop index out of range`
    #[must_use]
    pub(crate) fn index_error_pop_out_of_range() -> RunError {
        SimpleException::new_msg(Self::IndexError, "pop index out of range").into()
    }

    /// Creates a KeyError for popping from an empty dict.
    ///
    /// Matches CPython's format: `KeyError: 'popitem(): dictionary is empty'`
    #[must_use]
    pub(crate) fn key_error_popitem_empty_dict() -> RunError {
        SimpleException::new_msg(Self::KeyError, "'popitem(): dictionary is empty'").into()
    }

    /// Creates a LookupError for unknown encoding.
    ///
    /// Matches CPython's format: `LookupError: unknown encoding: {encoding}`
    #[must_use]
    pub(crate) fn lookup_error_unknown_encoding(encoding: &str) -> RunError {
        SimpleException::new_msg(Self::LookupError, format!("unknown encoding: {encoding}")).into()
    }

    /// Creates a UnicodeEncodeError for a run of `start..end` consecutive
    /// characters (character indices, not byte offsets) of `object` — the
    /// full string being encoded — that can't be represented in the target
    /// `codec`. `first_char` is the character at `start`, used for the
    /// single-character message form.
    ///
    /// Matches CPython's format, which differs for a single character vs. a run:
    /// `UnicodeEncodeError: 'ascii' codec can't encode character '\xe9' in
    /// position 1: ordinal not in range(128)` or `... can't encode characters
    /// in position 1-2: ordinal not in range(128)`.
    #[must_use]
    pub(crate) fn unicode_encode_error(
        codec: &str,
        object: &str,
        first_char: char,
        start: usize,
        end: usize,
        reason: &str,
    ) -> RunError {
        // Callers must pass a non-empty range; checked in debug builds only so
        // a wrong caller can't panic the VM in release (it gets a garbled
        // message position instead, which is harmless).
        debug_assert!(
            end > start,
            "unicode_encode_error: end ({end}) must be > start ({start})"
        );
        let msg = if end - start == 1 {
            format!(
                "'{codec}' codec can't encode character '{}' in position {start}: {reason}",
                ascii_escape(&first_char.to_string())
            )
        } else {
            let last = end - 1;
            format!("'{codec}' codec can't encode characters in position {start}-{last}: {reason}")
        };
        SimpleException::new_msg(Self::UnicodeEncodeError, msg)
            .with_data(UnicodeErrorData::encode(codec, object, start, end, reason))
            .into()
    }

    /// Creates a UnicodeDecodeError for the undecodable byte range `start..end`
    /// (byte offsets into `object` — the full input being decoded).
    ///
    /// Matches CPython's format, which differs for a single byte vs. a run:
    /// `UnicodeDecodeError: 'ascii' codec can't decode byte 0xe9 in position 6:
    /// ordinal not in range(128)` or `'utf-8' codec can't decode bytes in
    /// position 0-1: unexpected end of data`.
    #[must_use]
    pub(crate) fn unicode_decode_error(codec: &str, object: &[u8], start: usize, end: usize, reason: &str) -> RunError {
        // Defensive `get`: `start` always indexes a real byte for the errors
        // Monty produces, but a wrong caller must not be able to panic the VM.
        let first_byte = object.get(start).copied().unwrap_or(0);
        SimpleException::new_msg(
            Self::UnicodeDecodeError,
            unicode_decode_error_msg(codec, first_byte, start, end, reason),
        )
        .with_data(UnicodeErrorData::decode(codec, object, start, end, reason))
        .into()
    }

    /// Creates a ValueError for subsequence not found in bytes/str.
    ///
    /// Matches CPython's format: `ValueError: subsection not found`
    #[must_use]
    pub(crate) fn value_error_subsequence_not_found() -> RunError {
        SimpleException::new_msg(Self::ValueError, "subsection not found").into()
    }

    /// Creates a LookupError for unknown error handler.
    ///
    /// Matches CPython's format: `LookupError: unknown error handler name '{name}'`
    #[must_use]
    pub(crate) fn lookup_error_unknown_error_handler(name: &str) -> RunError {
        SimpleException::new_msg(Self::LookupError, format!("unknown error handler name '{name}'")).into()
    }

    /// Creates a TypeError for an encode-only error handler (`xmlcharrefreplace`,
    /// `namereplace`) invoked for a decode error.
    ///
    /// Matches CPython's format: `TypeError: don't know how to handle
    /// UnicodeDecodeError in error callback`
    #[must_use]
    pub(crate) fn type_error_decode_error_callback() -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            "don't know how to handle UnicodeDecodeError in error callback",
        )
        .into()
    }

    /// Creates a NotImplementedError for a decode error handler that would
    /// produce lone surrogates (`surrogateescape` always; `surrogatepass` when
    /// the input actually contains an encoded surrogate).
    ///
    /// CPython's handlers put lone surrogates (e.g. U+DC80–U+DCFF for
    /// `surrogateescape`) in the resulting string. Monty strings are strict
    /// UTF-8 and cannot represent lone surrogates, so these cases cannot be
    /// supported — see `limitations/encoding.md`.
    #[must_use]
    pub(crate) fn not_implemented_surrogate_handler_decode(handler: &str) -> RunError {
        SimpleException::new_msg(
            Self::NotImplementedError,
            format!(
                "the '{handler}' error handler is not supported by Monty for decoding: \
                 Monty strings cannot contain the lone surrogate characters it produces"
            ),
        )
        .into()
    }

    /// Creates a `re.PatternError` for an invalid regex pattern or unsupported regex feature.
    ///
    /// Matches CPython's exception type: `re.PatternError: {message}`
    #[must_use]
    pub(crate) fn re_pattern_error(msg: impl fmt::Display) -> RunError {
        SimpleException::new_msg(Self::RePatternError, msg).into()
    }

    /// Creates a `json.JSONDecodeError` with CPython-compatible location suffix,
    /// formatted as `{message}: line {line} column {column} (char {index})`.
    ///
    /// The fields also travel in a structured [`JsonErrorData`] payload
    /// (with `doc` capped) so hosts can rebuild the real `json.JSONDecodeError`
    /// with its `msg`/`doc`/`pos`/`lineno`/`colno` attributes.
    ///
    /// # Arguments
    ///
    /// * `message` - The bare error message, without the location suffix
    /// * `doc` - The document being parsed (CPython's `exc.doc`)
    /// * `line` - 1-based line of the error (CPython's `exc.lineno`)
    /// * `column` - 1-based column of the error (CPython's `exc.colno`)
    /// * `index` - Character index of the error in `doc` (CPython's `exc.pos`)
    #[must_use]
    pub(crate) fn json_decode_error(message: &str, doc: &[u8], line: usize, column: usize, index: usize) -> RunError {
        SimpleException::new_msg(
            Self::JsonDecodeError,
            format!("{message}: line {line} column {column} (char {index})"),
        )
        .with_data(JsonErrorData::build(message, doc, index, line, column))
        .into()
    }

    /// Creates the `TypeError` used by `json.loads()` for unsupported input types.
    ///
    /// Matches CPython's format:
    /// `the JSON object must be str, bytes or bytearray, not {type}`
    #[must_use]
    pub(crate) fn json_loads_type_error(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("the JSON object must be str, bytes or bytearray, not {type_}"),
        )
        .into()
    }

    /// Creates the `ValueError` used by `json.dumps()` for circular containers.
    ///
    /// Matches CPython's format: `Circular reference detected`
    #[must_use]
    pub(crate) fn json_circular_reference_error() -> RunError {
        SimpleException::new_msg(Self::ValueError, "Circular reference detected").into()
    }

    /// Creates the `TypeError` used by `json.dumps()` for unsupported object types.
    ///
    /// Matches CPython's format:
    /// `Object of type {type} is not JSON serializable`
    #[must_use]
    pub(crate) fn json_not_serializable_error(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("Object of type {type_} is not JSON serializable"),
        )
        .into()
    }

    /// Creates the `TypeError` used by `json.dumps()` for unsupported dict keys.
    ///
    /// Matches CPython's format:
    /// `keys must be str, int, float, bool or None, not {type}`
    #[must_use]
    pub(crate) fn json_invalid_key_error(type_: &str) -> RunError {
        SimpleException::new_msg(
            Self::TypeError,
            format!("keys must be str, int, float, bool or None, not {type_}"),
        )
        .into()
    }

    /// Creates the `ValueError` used by `json.dumps(..., allow_nan=False)`.
    ///
    /// Matches CPython's format:
    /// `Out of range float values are not JSON compliant: {value}`
    #[must_use]
    pub(crate) fn json_nan_error(value: &str) -> RunError {
        SimpleException::new_msg(
            Self::ValueError,
            format!("Out of range float values are not JSON compliant: {value}"),
        )
        .into()
    }
}

/// Formats the message for a `UnicodeDecodeError` covering the byte range
/// `start..end`: CPython's single-byte form (`byte 0x{first_byte:02x} in
/// position {start}`) when the range is one byte, otherwise the range form
/// (`bytes in position {start}-{end - 1}`).
///
/// A free function (rather than folded into [`ExcType::unicode_decode_error`])
/// so the fs layer can produce the identical wording when converting a
/// `MountError::InvalidUtf8` from a text-mode file read into an exception.
pub(crate) fn unicode_decode_error_msg(codec: &str, first_byte: u8, start: usize, end: usize, reason: &str) -> String {
    // Callers must pass a non-empty range; checked in debug builds only so a
    // wrong caller can't panic the VM in release (it gets a garbled message
    // position instead, which is harmless).
    debug_assert!(
        end > start,
        "unicode_decode_error_msg: end ({end}) must be > start ({start})"
    );
    if end - start == 1 {
        format!("'{codec}' codec can't decode byte 0x{first_byte:02x} in position {start}: {reason}")
    } else {
        let last = end - 1;
        format!("'{codec}' codec can't decode bytes in position {start}-{last}: {reason}")
    }
}

/// Simple lightweight representation of an exception.
///
/// This is used for performance reasons for common exception patterns.
/// Exception messages use `String` for owned storage.
#[derive(Debug, Clone, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct SimpleException {
    exc_type: ExcType,
    arg: Option<String>,
    /// Structured payload (e.g. unicode-error constructor fields), carried
    /// through catch/re-raise so it reaches the public `MontyException` when
    /// the exception escapes the sandbox. No `skip_serializing_if`:
    /// exceptions round-trip through non-self-describing snapshot formats
    /// where skipped fields break deserialization.
    #[serde(default)]
    data: ExcData,
}

impl fmt::Display for SimpleException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.py_repr_fmt(f)
    }
}
impl From<MontyException> for SimpleException {
    fn from(mut exc: MontyException) -> Self {
        Self {
            exc_type: exc.exc_type(),
            data: exc.take_data(),
            arg: exc.into_message(),
        }
    }
}

impl SimpleException {
    /// Creates a new exception with the given type and optional argument message.
    #[must_use]
    pub fn new(exc_type: ExcType, arg: Option<String>) -> Self {
        Self {
            exc_type,
            arg,
            data: ExcData::None,
        }
    }

    /// Creates a new exception with the given type and argument message.
    #[must_use]
    pub fn new_msg(exc_type: ExcType, arg: impl fmt::Display) -> Self {
        Self {
            exc_type,
            arg: Some(arg.to_string()),
            data: ExcData::None,
        }
    }

    /// Creates a new exception with the given type and no argument message.
    #[must_use]
    pub fn new_none(exc_type: ExcType) -> Self {
        Self {
            exc_type,
            arg: None,
            data: ExcData::None,
        }
    }

    /// Attaches a structured payload — see [`ExcData`].
    #[must_use]
    pub fn with_data(mut self, data: ExcData) -> Self {
        self.data = data;
        self
    }

    /// The structured payload, [`ExcData::None`] for most exceptions.
    #[must_use]
    pub fn data(&self) -> &ExcData {
        &self.data
    }

    #[must_use]
    pub fn exc_type(&self) -> ExcType {
        self.exc_type
    }

    #[must_use]
    pub fn arg(&self) -> Option<&String> {
        self.arg.as_ref()
    }

    /// str() for an exception
    #[must_use]
    pub fn py_str(&self) -> String {
        match (self.exc_type, &self.arg) {
            // KeyError expecificaly uses repr of the key for str(exc)
            (ExcType::KeyError, Some(exc)) => StringRepr(exc).to_string(),
            (_, Some(arg)) => arg.to_owned(),
            (_, None) => String::new(),
        }
    }
}

impl<'h> HeapRead<'h, SimpleException> {
    pub(crate) fn py_type(&self, vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Exception(self.get(vm.heap).exc_type)
    }
}

impl SimpleException {
    /// Returns the exception formatted as Python would repr it.
    pub fn py_repr_fmt(&self, f: &mut impl Write) -> fmt::Result {
        let type_str: &'static str = self.exc_type.into();
        write!(f, "{type_str}(")?;

        if let Some(arg) = &self.arg {
            string_repr_fmt(arg, f)?;
        }

        f.write_char(')')
    }

    pub(crate) fn with_frame(self, frame: RawStackFrame) -> ExceptionRaise {
        ExceptionRaise {
            exc: self,
            frame: Some(frame),
            hide_caret: false,
        }
    }

    pub(crate) fn with_position(self, position: CodeRange) -> ExceptionRaise {
        ExceptionRaise {
            exc: self,
            frame: Some(RawStackFrame::from_position(position)),
            hide_caret: false,
        }
    }
}

impl<'h> HeapRead<'h, SimpleException> {
    /// Gets an attribute from this exception.
    ///
    /// Handles the `.args` attribute by allocating a tuple containing the message.
    /// Returns `Err(AttributeError)` for all other attributes.
    pub fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        // Fast path: interned strings can be matched by ID
        let is_args = attr
            .static_string()
            .map_or_else(|| attr.as_str(vm.interns) == "args", |ss| ss == StaticStrings::Args);

        if is_args {
            // Construct tuple with 0 or 1 elements based on whether arg exists
            let elements = if let Some(arg_str) = &self.get(vm.heap).arg {
                smallvec![allocate_string(arg_str.as_str(), vm.heap)?]
            } else {
                smallvec![]
            };
            Ok(Some(CallResult::Value(allocate_tuple(elements, vm.heap)?)))
        } else {
            Ok(None)
        }
    }
}

/// A raised exception with optional stack frame for traceback.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExceptionRaise {
    pub exc: SimpleException,
    /// The stack frame where the exception was raised (first in vec is closest "bottom" frame).
    pub frame: Option<RawStackFrame>,
    /// Whether to hide the caret marker when creating the stack frame.
    ///
    /// CPython doesn't show carets for attribute GET errors, but does show them
    /// for attribute SET errors. This flag allows error creators to specify
    /// whether the caret should be hidden.
    #[serde(default)]
    pub hide_caret: bool,
}

impl From<SimpleException> for ExceptionRaise {
    fn from(exc: SimpleException) -> Self {
        Self {
            exc,
            frame: None,
            hide_caret: false,
        }
    }
}

impl From<MontyException> for ExceptionRaise {
    fn from(exc: MontyException) -> Self {
        Self {
            exc: exc.into(),
            frame: None,
            hide_caret: false,
        }
    }
}

impl ExceptionRaise {
    /// Adds a caller's frame as the outermost frame in the traceback chain.
    ///
    /// This is used when an exception propagates up through call frames.
    /// The new frame becomes the ultimate parent (displayed first in traceback,
    /// since tracebacks show "most recent call last").
    ///
    /// Special case: If the innermost frame has no name yet (created with `with_position`),
    /// this sets its name instead of creating a new parent. This happens when the error
    /// is raised from a namespace lookup - the initial frame has the position but not
    /// the function name, which gets filled in as the error propagates.
    pub(crate) fn add_caller_frame(&mut self, position: CodeRange, name: StringId) {
        self.add_caller_frame_inner(position, name, false);
    }

    fn add_caller_frame_inner(&mut self, position: CodeRange, name: StringId, hide_caret: bool) {
        if let Some(ref mut frame) = self.frame {
            // If innermost frame has no name, set it instead of adding a parent
            // This handles errors from namespace lookups which create nameless frames
            if frame.frame_name.is_none() {
                frame.frame_name = Some(name);
                frame.hide_caret = hide_caret;
                return;
            }
            // Find the outermost frame (the one with no parent) and add the new frame as its parent
            let mut current = frame;
            while current.parent.is_some() {
                current = current.parent.as_mut().unwrap();
            }
            let mut new_frame = RawStackFrame::new(position, name, None);
            new_frame.hide_caret = hide_caret;
            current.parent = Some(Box::new(new_frame));
        } else {
            // No frame yet - create one
            let mut new_frame = RawStackFrame::new(position, name, None);
            new_frame.hide_caret = hide_caret;
            self.frame = Some(new_frame);
        }
    }

    /// Converts this exception to a `MontyException` for the public API.
    ///
    /// Uses `Interns` to resolve `StringId` references to actual strings.
    /// Extracts preview lines from the source code for traceback display.
    /// Converts this exception into a public `MontyException`, expanding each
    /// stack frame's raw byte offsets into lines/columns/preview text via a
    /// caller-provided source lookup.
    ///
    /// The caller must supply `source_for` so that frames whose `CodeRange`
    /// points into a *different* source than the one currently executing can
    /// still be resolved. In particular, REPL tracebacks can interleave
    /// frames from multiple snippets (e.g. calling into a function defined
    /// in an earlier feed); resolving those byte offsets against only the
    /// current snippet's source would produce wrong line/column/caret
    /// information. `source_for` is called per unique filename encountered
    /// in the traceback and its result is cached, so each source is scanned
    /// at most once regardless of how many frames share it.
    #[must_use]
    pub fn into_python_exception<'s>(
        self,
        interns: &Interns,
        source_for: impl Fn(&str) -> Option<&'s str>,
    ) -> MontyException {
        // Per-filename SourceMap cache. Typical tracebacks touch 1-3 unique
        // filenames so a tiny `Vec` beats a HashMap on both allocations and
        // lookup cost.
        let mut cache: Vec<(StringId, SourceMap<'s>)> = Vec::new();
        let traceback = self
            .frame
            .map(|frame| {
                let mut frames = Vec::new();
                let mut current = Some(&frame);
                while let Some(f) = current {
                    let fname_id = f.position.filename;
                    let sm_idx = if let Some(i) = cache.iter().position(|(k, _)| *k == fname_id) {
                        i
                    } else {
                        let fname = interns.get_str(fname_id);
                        let src = source_for(fname).unwrap_or("");
                        cache.push((fname_id, SourceMap::new(src)));
                        cache.len() - 1
                    };
                    frames.push(StackFrame::from_raw(f, interns, &mut cache[sm_idx].1));
                    current = f.parent.as_deref();
                }
                // Reverse so outermost frame is first (Python's "most recent call last" ordering)
                frames.reverse();
                frames
            })
            .unwrap_or_default();

        MontyException::new_full(self.exc.exc_type, self.exc.arg, traceback).with_data(self.exc.data)
    }
}

/// A stack frame for traceback information.
///
/// Stores position information and optional function name as StringId.
/// The actual name string must be looked up externally when formatting the traceback.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawStackFrame {
    pub position: CodeRange,
    /// The name of the frame (function name StringId, or None for module-level code).
    pub frame_name: Option<StringId>,
    pub parent: Option<Box<Self>>,
    /// Whether to hide the caret marker in the traceback for this frame.
    ///
    /// Set to `true` for:
    /// - `raise` statements (CPython doesn't show carets for raise)
    /// - `AttributeError` on attribute access (CPython doesn't show carets for these)
    pub hide_caret: bool,
}

impl RawStackFrame {
    pub(crate) fn new(position: CodeRange, frame_name: StringId, parent: Option<&Self>) -> Self {
        Self {
            position,
            frame_name: Some(frame_name),
            parent: parent.map(|p| Box::new(p.clone())),
            hide_caret: false,
        }
    }

    fn from_position(position: CodeRange) -> Self {
        Self {
            position,
            frame_name: None,
            parent: None,
            hide_caret: false,
        }
    }

    /// Creates a new frame for a raise statement (no caret will be shown).
    pub(crate) fn from_raise(position: CodeRange, frame_name: StringId) -> Self {
        Self {
            position,
            frame_name: Some(frame_name),
            parent: None,
            hide_caret: true,
        }
    }
}

/// Runtime error types that can occur during execution.
///
/// Three variants:
/// - `Internal`: Bug in interpreter implementation (static message)
/// - `Exc`: Python exception that can be caught by try/except (when implemented)
/// - `UncatchableExc`: Python exception from resource limits that CANNOT be caught
///
/// `Clone` is implemented so an error can be cached for later re-raising
/// (e.g. a failed `GatherFuture` replaying the same exception on every
/// re-await). Inner data is shallow-clonable: `Cow<'static, str>` is cheap,
/// and `ExceptionRaise` already derives `Clone`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum RunError {
    /// Internal interpreter error - indicates a bug in Monty, not user code.
    Internal(Cow<'static, str>),
    /// Catchable Python exception (e.g., ValueError, TypeError).
    Exc(ExceptionRaise),
    /// Uncatchable Python exception from resource limits (MemoryError, TimeoutError).
    ///
    /// These exceptions display with proper tracebacks like normal Python exceptions,
    /// but cannot be caught by try/except blocks. This prevents untrusted code from
    /// suppressing resource limit violations.
    UncatchableExc(ExceptionRaise),
}

impl From<ExceptionRaise> for RunError {
    fn from(exc: ExceptionRaise) -> Self {
        Self::Exc(exc)
    }
}

impl From<SimpleException> for RunError {
    fn from(exc: SimpleException) -> Self {
        Self::Exc(exc.into())
    }
}

impl From<MontyException> for RunError {
    fn from(exc: MontyException) -> Self {
        Self::Exc(exc.into())
    }
}

impl From<FormatError> for RunError {
    fn from(err: FormatError) -> Self {
        let exc_type = match &err {
            FormatError::Overflow(_) => ExcType::OverflowError,
            FormatError::InvalidAlignment(_) | FormatError::ValueError(_) => ExcType::ValueError,
        };
        Self::Exc(SimpleException::new_msg(exc_type, err).into())
    }
}

impl From<fmt::Error> for RunError {
    /// Converts a `fmt::Error` into a `RunError`.
    ///
    /// In practice, writing to a `String` buffer never fails, so `fmt::Error` only
    /// arises from our explicit error returns (e.g. INT_MAX_STR_DIGITS checks in
    /// `py_repr_fmt`). This impl exists so `write!()?` in `py_repr_fmt` auto-converts
    /// when the method returns `RunResult<()>`.
    fn from(err: fmt::Error) -> Self {
        Self::internal(format!("unexpected formatting error: {err}"))
    }
}

impl RunError {
    /// Converts this runtime error to a `MontyException` for the public API.
    ///
    /// Internal errors are converted to `RuntimeError` exceptions with no traceback.
    /// Converts this runtime error into a public `MontyException`.
    ///
    /// `source_for` is consulted per unique filename referenced by the
    /// traceback — see [`ExceptionRaise::into_python_exception`] for why
    /// this is a lookup rather than a single source string.
    #[must_use]
    pub fn into_python_exception<'s>(
        self,
        interns: &Interns,
        source_for: impl Fn(&str) -> Option<&'s str>,
    ) -> MontyException {
        match self {
            Self::Exc(exc) | Self::UncatchableExc(exc) => exc.into_python_exception(interns, source_for),
            Self::Internal(err) => MontyException::runtime_error(format!("Internal error in monty: {err}")),
        }
    }

    pub fn internal(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal(msg.into())
    }
}

/// Formats a list of parameter names for error messages, matching CPython's
/// `format_missing` joining (note the Oxford comma for three or more names).
///
/// Examples:
/// - `["a"]` -> `'a'`
/// - `["a", "b"]` -> `'a' and 'b'`
/// - `["a", "b", "c"]` -> `'a', 'b', and 'c'`
fn format_param_names(names: &[&str]) -> String {
    match names.len() {
        0 => String::new(),
        1 => format!("'{}'", names[0]),
        2 => format!("'{}' and '{}'", names[0], names[1]),
        _ => {
            let last = names.last().unwrap();
            let rest: Vec<_> = names[..names.len() - 1].iter().map(|n| format!("'{n}'")).collect();
            format!("{}, and '{last}'", rest.join(", "))
        }
    }
}
