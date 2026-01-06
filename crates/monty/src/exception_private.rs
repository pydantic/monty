use std::{
    borrow::Cow,
    fmt::{self, Write},
};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    args::ArgValues,
    exception_public::{MontyException, StackFrame},
    expressions::ExprLoc,
    heap::{Heap, HeapData},
    intern::{Interns, StringId},
    operators::{CmpOperator, Operator},
    parse::CodeRange,
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{str::string_repr, PyTrait, Type},
    value::Value,
};

/// Python exception types supported by the interpreter.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `Into<&'static str>` implementations.
/// The string representation matches the variant name exactly (e.g., `ValueError` -> "ValueError").
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, IntoStaticStr, Serialize, Deserialize)]
pub enum ExcType {
    /// primary exception class - matches any exception in isinstance checks.
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

    // --- Standalone exception types ---
    AssertionError,
    MemoryError,
    NameError,
    SyntaxError,
    TimeoutError,
    TypeError,
    ValueError,
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
    pub(crate) fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let exc = match args {
            ArgValues::Empty => Ok(SimpleException::new(self, None)),
            ArgValues::One(value) => {
                // Borrow the value to inspect its type, then clean up with drop_with_heap
                let result = match &value {
                    Value::InternString(string_id) => {
                        Ok(SimpleException::new(self, Some(interns.get_str(*string_id).to_owned())))
                    }
                    Value::Ref(heap_id) => {
                        if let HeapData::Str(s) = heap.get(*heap_id) {
                            Ok(SimpleException::new(self, Some(s.as_str().to_owned())))
                        } else {
                            Err(RunError::internal(
                                "exceptions can only be called with zero or one string argument",
                            ))
                        }
                    }
                    _ => Err(RunError::internal(
                        "exceptions can only be called with zero or one string argument",
                    )),
                };
                // Properly clean up the value using drop_with_heap which handles ref-count-panic
                value.drop_with_heap(heap);
                result
            }
            _ => {
                // Clean up any args before returning error
                args.drop_with_heap(heap);
                Err(RunError::internal(
                    "exceptions can only be called with zero or one string argument",
                ))
            }
        }?;
        let heap_id = heap.allocate(HeapData::Exception(exc))?;
        Ok(Value::Ref(heap_id))
    }

    #[must_use]
    pub fn attribute_error(type_: Type, attr: &str) -> RunError {
        exc_fmt!(Self::AttributeError; "'{type_}' object has no attribute '{attr}'").into()
    }

    /// Creates an AttributeError for a dataclass method that requires external call integration.
    ///
    /// This is a temporary error used when dataclass methods are called but the external
    /// call mechanism hasn't been integrated yet.
    #[must_use]
    pub fn attribute_error_method_not_implemented(class_name: &str, method_name: &str) -> RunError {
        exc_fmt!(Self::AttributeError; "'{class_name}' object method '{method_name}' requires external call (not yet implemented)").into()
    }

    /// Creates an AttributeError for when a specific attribute is not found on an object.
    ///
    /// Matches CPython's format: `AttributeError: 'ClassName' object has no attribute 'attr_name'`
    #[must_use]
    pub fn attribute_error_not_found(class_name: &str, attr_name: &str) -> RunError {
        exc_fmt!(Self::AttributeError; "'{class_name}' object has no attribute '{attr_name}'").into()
    }

    /// Creates an AttributeError for attribute assignment on types that don't support it.
    ///
    /// Matches CPython's format for setting attributes on built-in types.
    #[must_use]
    pub fn attribute_error_no_setattr(type_: Type, attr_name: &str) -> RunError {
        exc_fmt!(Self::AttributeError; "'{type_}' object has no attribute '{attr_name}' and no __dict__ for setting new attributes").into()
    }

    /// Creates a FrozenInstanceError for assigning to a frozen dataclass.
    ///
    /// Matches CPython's `dataclasses.FrozenInstanceError` which is a subclass of `AttributeError`.
    /// Message format: "cannot assign to field 'attr_name'"
    #[must_use]
    pub fn frozen_instance_error(attr_name: &str) -> RunError {
        exc_fmt!(Self::FrozenInstanceError; "cannot assign to field '{attr_name}'").into()
    }

    #[must_use]
    pub fn type_error_not_sub(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "'{type_}' object is not subscriptable").into()
    }

    /// Creates a TypeError for item assignment on types that don't support it.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object does not support item assignment`
    #[must_use]
    pub fn type_error_not_sub_assignment(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "'{type_}' object does not support item assignment").into()
    }

    /// Creates a TypeError for unhashable types when calling `hash()`.
    ///
    /// This matches Python 3.14's error message: `TypeError: unhashable type: 'list'`
    #[must_use]
    pub fn type_error_unhashable(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "unhashable type: '{type_}'").into()
    }

    /// Creates a TypeError for unhashable types used as dict keys.
    ///
    /// This matches Python 3.14's error message:
    /// `TypeError: cannot use 'list' as a dict key (unhashable type: 'list')`
    #[must_use]
    pub fn type_error_unhashable_dict_key(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "cannot use '{type_}' as a dict key (unhashable type: '{type_}')").into()
    }

    /// Creates a TypeError for unhashable types used as set elements.
    ///
    /// This matches Python 3.14's error message:
    /// `TypeError: cannot use 'list' as a set element (unhashable type: 'list')`
    #[must_use]
    pub fn type_error_unhashable_set_element(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "cannot use '{type_}' as a set element (unhashable type: '{type_}')").into()
    }

    /// Creates a KeyError for a missing dict key.
    ///
    /// For string keys, uses the raw string value without extra quoting.
    /// For other types, uses repr.
    #[must_use]
    pub fn key_error(key: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunError {
        let key_str = match key {
            Value::InternString(string_id) => interns.get_str(*string_id).to_owned(),
            Value::Ref(id) => {
                if let HeapData::Str(s) = heap.get(*id) {
                    s.as_str().to_owned()
                } else {
                    key.py_repr(heap, interns).into_owned()
                }
            }
            _ => key.py_repr(heap, interns).into_owned(),
        };
        SimpleException::new(Self::KeyError, Some(key_str)).into()
    }

    /// Creates a KeyError for popping from an empty set.
    ///
    /// Matches CPython's error format: `KeyError: 'pop from an empty set'`
    #[must_use]
    pub fn key_error_pop_empty_set() -> RunError {
        exc_fmt!(Self::KeyError; "pop from an empty set").into()
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
    pub fn type_error_arg_count(name: &str, expected: usize, actual: usize) -> RunError {
        if expected == 1 {
            // CPython: "len() takes exactly one argument (2 given)"
            exc_fmt!(Self::TypeError; "{}() takes exactly one argument ({} given)", name, actual).into()
        } else {
            // CPython: "insert expected 2 arguments, got 1"
            exc_fmt!(Self::TypeError; "{} expected {} arguments, got {}", name, expected, actual).into()
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
    pub fn type_error_no_args(name: &str, actual: usize) -> RunError {
        // CPython: "dict.keys() takes no arguments (1 given)"
        exc_fmt!(Self::TypeError; "{}() takes no arguments ({} given)", name, actual).into()
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
    pub fn type_error_at_least(name: &str, min: usize, actual: usize) -> RunError {
        // CPython: "get expected at least 1 argument, got 0"
        exc_fmt!(Self::TypeError; "{} expected at least {} argument, got {}", name, min, actual).into()
    }

    /// Creates a TypeError for when a function receives more arguments than allowed.
    ///
    /// Matches CPython's format: `{name} expected at most {max} arguments, got {actual}`
    ///
    /// # Arguments
    /// * `name` - The function name (e.g., "get", "pop")
    /// * `max` - Maximum number of allowed arguments
    /// * `actual` - Number of arguments actually provided
    #[must_use]
    pub fn type_error_at_most(name: &str, max: usize, actual: usize) -> RunError {
        // CPython: "get expected at most 2 arguments, got 3"
        exc_fmt!(Self::TypeError; "{} expected at most {} arguments, got {}", name, max, actual).into()
    }

    /// Creates a TypeError for missing positional arguments.
    ///
    /// Matches CPython's format: `{name}() missing {count} required positional argument(s): 'a' and 'b'`
    #[must_use]
    pub fn type_error_missing_positional_with_names(name: &str, missing_names: &[&str]) -> RunError {
        let count = missing_names.len();
        let names_str = format_param_names(missing_names);
        if count == 1 {
            exc_fmt!(Self::TypeError; "{}() missing 1 required positional argument: {}", name, names_str).into()
        } else {
            exc_fmt!(Self::TypeError; "{}() missing {} required positional arguments: {}", name, count, names_str)
                .into()
        }
    }

    /// Creates a TypeError for missing keyword-only arguments.
    ///
    /// Matches CPython's format: `{name}() missing {count} required keyword-only argument(s): 'a' and 'b'`
    #[must_use]
    pub fn type_error_missing_kwonly_with_names(name: &str, missing_names: &[&str]) -> RunError {
        let count = missing_names.len();
        let names_str = format_param_names(missing_names);
        if count == 1 {
            exc_fmt!(Self::TypeError; "{}() missing 1 required keyword-only argument: {}", name, names_str).into()
        } else {
            exc_fmt!(Self::TypeError; "{}() missing {} required keyword-only arguments: {}", name, count, names_str)
                .into()
        }
    }

    /// Creates a TypeError for too many positional arguments.
    ///
    /// Matches CPython's format:
    /// - Simple: `{name}() takes {max} positional argument(s) but {actual} were given`
    /// - With kwonly: `{name}() takes {max} positional argument(s) but {actual} positional argument(s) (and N keyword-only argument(s)) were given`
    #[must_use]
    pub fn type_error_too_many_positional(name: &str, max: usize, actual: usize, kwonly_given: usize) -> RunError {
        let takes_word = if max == 1 { "argument" } else { "arguments" };

        if kwonly_given > 0 {
            // CPython includes keyword-only args in the "given" part when present
            let given_word = if actual == 1 { "argument" } else { "arguments" };
            let kwonly_word = if kwonly_given == 1 { "argument" } else { "arguments" };
            exc_fmt!(
                Self::TypeError;
                "{}() takes {} positional {} but {} positional {} (and {} keyword-only {}) were given",
                name, max, takes_word, actual, given_word, kwonly_given, kwonly_word
            )
            .into()
        } else if max == 0 {
            exc_fmt!(Self::TypeError; "{}() takes 0 positional arguments but {} were given", name, actual).into()
        } else {
            exc_fmt!(Self::TypeError; "{}() takes {} positional {} but {} were given", name, max, takes_word, actual)
                .into()
        }
    }

    /// Creates a TypeError for positional-only parameter passed as keyword.
    ///
    /// Matches CPython's format: `{name}() got some positional-only arguments passed as keyword arguments: '{param}'`
    #[must_use]
    pub fn type_error_positional_only(name: &str, param: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{}() got some positional-only arguments passed as keyword arguments: '{}'", name, param).into()
    }

    /// Creates a TypeError for duplicate argument.
    ///
    /// Matches CPython's format: `{name}() got multiple values for argument '{param}'`
    #[must_use]
    pub fn type_error_duplicate_arg(name: &str, param: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{name}() got multiple values for argument '{param}'").into()
    }

    /// Creates a TypeError for unexpected keyword argument.
    ///
    /// Matches CPython's format: `{name}() got an unexpected keyword argument '{key}'`
    #[must_use]
    pub fn type_error_unexpected_keyword(name: &str, key: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{name}() got an unexpected keyword argument '{key}'").into()
    }

    /// Creates a simple TypeError with a custom message.
    #[must_use]
    pub fn type_error(msg: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{msg}").into()
    }

    /// Creates a TypeError for bytes() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: cannot convert '{type}' object to bytes`
    #[must_use]
    pub fn type_error_bytes_init(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "cannot convert '{type_}' object to bytes").into()
    }

    /// Creates a TypeError for calling a non-callable type.
    ///
    /// Matches CPython's format: `TypeError: cannot create '{type}' instances`
    #[must_use]
    pub fn type_error_not_callable(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "cannot create '{type_}' instances").into()
    }

    /// Creates a TypeError for non-iterable type in list/tuple/etc constructors.
    ///
    /// Matches CPython's format: `TypeError: '{type}' object is not iterable`
    #[must_use]
    pub fn type_error_not_iterable(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "'{type_}' object is not iterable").into()
    }

    /// Creates a TypeError for int() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: int() argument must be a string, a bytes-like object or a real number, not '{type}'`
    #[must_use]
    pub fn type_error_int_conversion(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "int() argument must be a string, a bytes-like object or a real number, not '{type_}'").into()
    }

    /// Creates a TypeError for float() constructor with invalid type.
    ///
    /// Matches CPython's format: `TypeError: float() argument must be a string or a real number, not '{type}'`
    #[must_use]
    pub fn type_error_float_conversion(type_: Type) -> RunError {
        exc_fmt!(Self::TypeError; "float() argument must be a string or a real number, not '{type_}'").into()
    }

    /// Creates a ValueError for negative count in bytes().
    ///
    /// Matches CPython's format: `ValueError: negative count`
    #[must_use]
    pub fn value_error_negative_bytes_count() -> RunError {
        exc_static!(Self::ValueError; "negative count").into()
    }

    /// Creates a TypeError for isinstance() arg 2.
    ///
    /// Matches CPython's format: `TypeError: isinstance() arg 2 must be a type, a tuple of types, or a union`
    #[must_use]
    pub fn isinstance_arg2_error() -> RunError {
        exc_static!(Self::TypeError; "isinstance() arg 2 must be a type, a tuple of types, or a union").into()
    }

    /// Creates a ValueError for range() step argument being zero.
    ///
    /// Matches CPython's format: `ValueError: range() arg 3 must not be zero`
    #[must_use]
    pub fn value_error_range_step_zero() -> RunError {
        exc_static!(Self::ValueError; "range() arg 3 must not be zero").into()
    }

    /// Creates a RuntimeError for dict mutation during iteration.
    ///
    /// Matches CPython's format: `RuntimeError: dictionary changed size during iteration`
    #[must_use]
    pub fn runtime_error_dict_changed_size() -> RunError {
        exc_static!(Self::RuntimeError; "dictionary changed size during iteration").into()
    }

    /// Creates a RuntimeError for set mutation during iteration.
    ///
    /// Matches CPython's format: `RuntimeError: Set changed size during iteration`
    #[must_use]
    pub fn runtime_error_set_changed_size() -> RunError {
        exc_static!(Self::RuntimeError; "Set changed size during iteration").into()
    }

    /// Creates a TypeError for functions that don't accept keyword arguments.
    ///
    /// Matches CPython's format: `TypeError: {name}() takes no keyword arguments`
    #[must_use]
    pub fn type_error_no_kwargs(name: &str) -> RunError {
        exc_fmt!(Self::TypeError; "{name}() takes no keyword arguments").into()
    }

    /// Creates an IndexError for list index out of range.
    ///
    /// Matches CPython's format: `IndexError('list index out of range')`
    #[must_use]
    pub fn list_index_error() -> RunError {
        exc_static!(Self::IndexError; "list index out of range").into()
    }

    /// Creates an IndexError for tuple index out of range.
    ///
    /// Matches CPython's format: `IndexError('tuple index out of range')`
    #[must_use]
    pub fn tuple_index_error() -> RunError {
        exc_static!(Self::IndexError; "tuple index out of range").into()
    }

    /// Creates a TypeError for non-integer sequence indices.
    ///
    /// Matches CPython's format: `TypeError('{type}' indices must be integers, not '{index_type}')`
    #[must_use]
    pub fn type_error_indices(type_str: Type, index_type: Type) -> RunError {
        exc_fmt!(Self::TypeError; "{type_str} indices must be integers, not '{index_type}'").into()
    }

    /// Creates a NameError for accessing a free variable (nonlocal/closure) before it's assigned.
    ///
    /// Matches CPython's format: `NameError: cannot access free variable 'x' where it is not
    /// associated with a value in enclosing scope`
    #[must_use]
    pub fn name_error_free_variable(name: &str) -> SimpleException {
        exc_fmt!(Self::NameError; "cannot access free variable '{name}' where it is not associated with a value in enclosing scope")
    }

    /// Creates a NameError for accessing an undefined variable.
    ///
    /// Matches CPython's format: `NameError: name 'x' is not defined`
    #[must_use]
    pub fn name_error(name: &str) -> SimpleException {
        exc_fmt!(Self::NameError; "name '{name}' is not defined")
    }

    /// Creates a NotImplementedError for an unimplemented Python feature.
    ///
    /// Used during parsing when encountering Python syntax that Monty doesn't yet support.
    /// The message format is: "The monty syntax parser does not yet support {feature}"
    #[must_use]
    pub fn not_implemented(feature: &str) -> SimpleException {
        exc_fmt!(Self::NotImplementedError; "The monty syntax parser does not yet support {}", feature)
    }

    /// Creates a ZeroDivisionError for division by zero.
    ///
    /// Matches CPython 3.14's format: `ZeroDivisionError('division by zero')`
    #[must_use]
    pub fn zero_division() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "division by zero")
    }

    /// Creates a ZeroDivisionError for 0 raised to a negative power.
    ///
    /// Matches CPython 3.14's format: `ZeroDivisionError('zero to a negative power')`
    #[must_use]
    pub fn zero_pow_negative() -> SimpleException {
        exc_static!(Self::ZeroDivisionError; "zero to a negative power")
    }

    /// Creates an OverflowError for string/sequence repetition with count too large.
    ///
    /// Matches CPython's format: `OverflowError('cannot fit 'int' into an index-sized integer')`
    #[must_use]
    pub fn overflow_repeat_count() -> SimpleException {
        exc_static!(Self::OverflowError; "cannot fit 'int' into an index-sized integer")
    }

    /// Generates a consistent error for invalid `**kwargs` types.
    #[must_use]
    pub fn kwargs_type_error(callable_name: Option<&str>, type_: Type) -> SimpleException {
        let message = match callable_name {
            Some(name) => format!("{name}() argument after ** must be a mapping, not {type_}"),
            None => format!("argument after ** must be a mapping, not {type_}"),
        };
        SimpleException::new(ExcType::TypeError, Some(message))
    }

    /// Generates the duplicate keyword argument error.
    #[must_use]
    pub fn duplicate_kwarg_error(callable_name: Option<&str>, key: &str) -> SimpleException {
        let message = match callable_name {
            Some(name) => format!("{name}() got multiple values for keyword argument '{key}'"),
            None => format!("got multiple values for keyword argument '{key}'"),
        };
        SimpleException::new(ExcType::TypeError, Some(message))
    }
}

/// Simple lightweight representation of an exception.
///
/// This is used for performance reasons for common exception patterns.
/// Exception messages use `String` for owned storage.
#[derive(Debug, Clone, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SimpleException {
    exc_type: ExcType,
    arg: Option<String>,
}

impl fmt::Display for SimpleException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.py_repr_fmt(f)
    }
}
impl From<MontyException> for SimpleException {
    fn from(exc: MontyException) -> Self {
        Self {
            exc_type: exc.exc_type(),
            arg: exc.into_message(),
        }
    }
}

impl SimpleException {
    /// Creates a new exception with the given type and optional argument message.
    #[must_use]
    pub fn new(exc_type: ExcType, arg: Option<String>) -> Self {
        SimpleException { exc_type, arg }
    }

    #[must_use]
    pub fn exc_type(&self) -> ExcType {
        self.exc_type
    }

    #[must_use]
    pub fn arg(&self) -> Option<&String> {
        self.arg.as_ref()
    }

    pub(crate) fn py_type(&self) -> Type {
        Type::Exception(self.exc_type)
    }

    /// Returns the exception formatted as Python would display it to the user.
    ///
    /// Format: `ExceptionType: message` (e.g., `NotImplementedError: feature not supported`)
    /// If there's no message, just returns the exception type name.
    #[must_use]
    pub fn py_str(&self) -> String {
        // TODO this is wrong, it doesn't match what cpython does
        let type_str: &'static str = self.exc_type.into();
        match &self.arg {
            Some(arg) => format!("{type_str}: {arg}"),
            None => type_str.to_string(),
        }
    }

    /// Returns the exception formatted as Python would repr it.
    pub fn py_repr_fmt(&self, f: &mut impl Write) -> std::fmt::Result {
        let type_str: &'static str = self.exc_type.into();
        write!(f, "{type_str}(")?;

        if let Some(arg) = &self.arg {
            f.write_str(&string_repr(arg))?;
        }

        f.write_char(')')
    }

    pub(crate) fn with_frame(self, frame: RawStackFrame) -> ExceptionRaise {
        ExceptionRaise {
            exc: self,
            frame: Some(frame),
        }
    }

    pub(crate) fn with_position(self, position: CodeRange) -> ExceptionRaise {
        ExceptionRaise {
            exc: self,
            frame: Some(RawStackFrame::from_position(position)),
        }
    }

    /// Creates a TypeError for binary operator type mismatches.
    ///
    /// For `+` with str/list on the left side, uses CPython's special format:
    /// `can only concatenate {type} (not "{other}") to {type}`
    ///
    /// For other cases, uses the generic format:
    /// `unsupported operand type(s) for {op}: '{left}' and '{right}'`
    pub(crate) fn operand_type_error<T>(
        left: &ExprLoc,
        op: &Operator,
        right: &ExprLoc,
        left_type: Type,
        right_type: Type,
    ) -> RunResult<T> {
        let new_position = left.position.extend(&right.position);

        // CPython uses a special message for str/list + operations
        let message = if *op == Operator::Add && (left_type == Type::Str || left_type == Type::List) {
            format!("can only concatenate {left_type} (not \"{right_type}\") to {left_type}")
        } else {
            format!("unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
        };

        Err(SimpleException::new(ExcType::TypeError, Some(message))
            .with_position(new_position)
            .into())
    }

    pub(crate) fn cmp_type_error<T>(
        left: &ExprLoc,
        op: &CmpOperator,
        right: &ExprLoc,
        left_type: Type,
        right_type: Type,
    ) -> RunResult<T> {
        let new_position = left.position.extend(&right.position);

        let e =
            exc_fmt!(ExcType::TypeError; "'{op}' not supported between instances of '{left_type}' and '{right_type}'");

        Err(e.with_position(new_position).into())
    }

    /// Creates a TypeError for augmented assignment operator type mismatches (e.g., `+=`).
    ///
    /// For `+=` with str/list on the left side, uses CPython's special format:
    /// `can only concatenate {type} (not "{other}") to {type}`
    ///
    /// For other cases, uses the generic format:
    /// `unsupported operand type(s) for {op}: '{left}' and '{right}'`
    ///
    /// Returns a `SimpleException` without frame info - caller should add the frame.
    pub(crate) fn augmented_assign_type_error(op: &Operator, left_type: Type, right_type: Type) -> Self {
        let message = if *op == Operator::Add && (left_type == Type::Str || left_type == Type::List) {
            format!("can only concatenate {left_type} (not \"{right_type}\") to {left_type}")
        } else {
            format!("unsupported operand type(s) for {op}: '{left_type}' and '{right_type}'")
        };
        Self::new(ExcType::TypeError, Some(message))
    }
}

macro_rules! exc_static {
    ($error_type:expr; $msg:expr) => {
        crate::exception_private::SimpleException::new($error_type, Some($msg.into()))
    };
}
pub(crate) use exc_static;

macro_rules! exc_fmt {
    ($error_type:expr; $($fmt_args:tt)*) => {
        crate::exception_private::SimpleException::new($error_type, Some(format!($($fmt_args)*).into()))
    };
}
pub(crate) use exc_fmt;

macro_rules! exc_err_static {
    ($error_type:expr; $msg:expr) => {
        Err(crate::exception_private::exc_static!($error_type; $msg).into())
    };
}
pub(crate) use exc_err_static;

// TODO remove this, we should always set position before creating the Err
macro_rules! exc_err_fmt {
    ($error_type:expr; $($fmt_args:tt)*) => {
        Err(crate::exception_private::exc_fmt!($error_type; $($fmt_args)*).into())
    };
}
pub(crate) use exc_err_fmt;

/// A raised exception with optional stack frame for traceback.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExceptionRaise {
    pub exc: SimpleException,
    /// The stack frame where the exception was raised (first in vec is closest "bottom" frame).
    pub frame: Option<RawStackFrame>,
}

impl From<SimpleException> for ExceptionRaise {
    fn from(exc: SimpleException) -> Self {
        ExceptionRaise { exc, frame: None }
    }
}

impl From<MontyException> for ExceptionRaise {
    fn from(exc: MontyException) -> Self {
        Self {
            exc: exc.into(),
            frame: None,
        }
    }
}

impl ExceptionRaise {
    /// Returns the exception formatted as Python would display it to the user.
    ///
    /// Format: `ExceptionType: message` (e.g., `NotImplementedError: feature not supported`)
    #[must_use]
    pub fn py_str(&self) -> String {
        self.exc.py_str()
    }

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

    /// Like `add_caller_frame`, but suppresses caret display in the traceback.
    ///
    /// Used for errors where CPython doesn't show carets (e.g., AttributeError).
    pub(crate) fn add_caller_frame_no_caret(&mut self, position: CodeRange, name: StringId) {
        self.add_caller_frame_inner(position, name, true);
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
    #[must_use]
    pub fn into_python_exception(self, interns: &Interns, source: &str) -> MontyException {
        let traceback = self
            .frame
            .map(|frame| {
                let mut frames = Vec::new();
                let mut current = Some(&frame);
                while let Some(f) = current {
                    frames.push(StackFrame::from_raw(f, interns, source));
                    current = f.parent.as_deref();
                }
                // Reverse so outermost frame is first (Python's "most recent call last" ordering)
                frames.reverse();
                frames
            })
            .unwrap_or_default();

        MontyException::new_full(self.exc.exc_type(), self.exc.arg().cloned(), traceback)
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
    pub parent: Option<Box<RawStackFrame>>,
    /// Whether to hide the caret marker in the traceback for this frame.
    ///
    /// Set to `true` for:
    /// - `raise` statements (CPython doesn't show carets for raise)
    /// - `AttributeError` on attribute access (CPython doesn't show carets for these)
    pub hide_caret: bool,
}

impl RawStackFrame {
    pub(crate) fn new(position: CodeRange, frame_name: StringId, parent: Option<&RawStackFrame>) -> Self {
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
#[derive(Debug)]
pub enum RunError {
    /// Internal interpreter error - indicates a bug in Monty, not user code.
    Internal(Cow<'static, str>),
    /// Catchable Python exception (e.g., ValueError, TypeError).
    Exc(ExceptionRaise),
    /// Uncatchable Python exception from resource limits (MemoryError, TimeoutError, RecursionError).
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

impl RunError {
    /// Converts this runtime error to a `MontyException` for the public API.
    ///
    /// Internal errors are converted to `RuntimeError` exceptions with no traceback.
    #[must_use]
    pub fn into_python_exception(self, interns: &Interns, source: &str) -> MontyException {
        match self {
            Self::Exc(exc) | Self::UncatchableExc(exc) => exc.into_python_exception(interns, source),
            Self::Internal(err) => MontyException::runtime_error(format!("Internal error in monty: {err}")),
        }
    }

    pub fn internal(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal(msg.into())
    }

    /// Sets the frame on the exception.
    ///
    /// Used when an exception is raised from code that doesn't have access to position
    /// information (like `ForIterator::for_next`) but the caller does.
    pub(crate) fn set_frame(self, frame: RawStackFrame) -> Self {
        match self {
            Self::Exc(mut exc) => {
                exc.frame = Some(frame);
                Self::Exc(exc)
            }
            Self::UncatchableExc(mut exc) => {
                exc.frame = Some(frame);
                Self::UncatchableExc(exc)
            }
            Self::Internal(_) => self, // Internal errors don't have frames
        }
    }
}

/// Formats a list of parameter names for error messages.
///
/// Examples:
/// - `["a"]` -> `'a'`
/// - `["a", "b"]` -> `'a' and 'b'`
/// - `["a", "b", "c"]` -> `'a', 'b' and 'c'`
fn format_param_names(names: &[&str]) -> String {
    match names.len() {
        0 => String::new(),
        1 => format!("'{}'", names[0]),
        2 => format!("'{}' and '{}'", names[0], names[1]),
        _ => {
            let last = names.last().unwrap();
            let rest: Vec<_> = names[..names.len() - 1].iter().map(|n| format!("'{n}'")).collect();
            format!("{} and '{last}'", rest.join(", "))
        }
    }
}
