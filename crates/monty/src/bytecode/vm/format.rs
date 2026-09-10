//! F-string and value formatting helpers for the VM.

use std::fmt::Write as _;

use super::VM;
use crate::{
    bytecode::op::{FORMAT_VALUE_HAS_SPEC, FORMAT_VALUE_STATIC_SPEC},
    defer_drop,
    exception_private::{ExcType, RunError, SimpleException},
    fstring::{
        ParseFormatSpecReason, ParsedFormatSpec, decode_format_spec, format_string, format_with_spec,
        validate_string_spec,
    },
    heap::HeapReadOutput,
    string_builder::StringBuilder,
    types::{
        PyTrait, Type, date::format_date_strftime, datetime::format_datetime_strftime, str::allocate_string,
        time::format_time_strftime,
    },
    value::Value,
};

impl VM<'_> {
    /// Builds an f-string by concatenating n string parts from the stack.
    pub(super) fn build_fstring(&mut self, count: usize) -> Result<(), RunError> {
        let this = self;
        let parts = this.pop_n(count);
        defer_drop!(parts, this);
        let mut result = String::new();

        for part in parts.as_slice() {
            let part_str = part.py_str(this)?;
            defer_drop!(part_str, this);
            result.push_str(part_str.to_str(this)?);
        }

        let value = allocate_string(result, this.heap);
        this.push(value);
        Ok(())
    }

    /// Formats a value for f-string interpolation.
    ///
    /// See `Opcode::FormatValue` for the flag layout.
    pub(super) fn format_value(&mut self, flags: u8) -> Result<(), RunError> {
        let this = self;
        let conversion = flags & 0x03;
        let has_format_spec = (flags & FORMAT_VALUE_HAS_SPEC) != 0;
        let static_spec = (flags & FORMAT_VALUE_STATIC_SPEC) != 0;

        // Pop format spec if present (pushed before value, so popped after)
        let format_spec = if has_format_spec { Some(this.pop()) } else { None };

        let value = this.pop();
        defer_drop!(value, this);

        let formatted = match format_spec {
            Some(spec_value) => {
                defer_drop!(spec_value, this);
                if static_spec {
                    // The compiler only sets this flag for encoded integer specs.
                    let Value::Int(encoded) = spec_value else {
                        unreachable!("FORMAT_VALUE_STATIC_SPEC flag without Value::Int on stack");
                    };
                    let spec = decode_format_spec(*encoded);
                    this.format_parsed_value(value, conversion, &spec)?
                } else {
                    let spec = str_value_into_string(spec_value.py_str(this)?, this)?;
                    this.format_runtime_value(value, conversion, Some(&spec))?
                }
            }
            None => this.format_runtime_value(value, conversion, None)?,
        };

        let result = allocate_string(formatted, this.heap);
        this.push(result);
        Ok(())
    }

    /// Formats a value from a runtime format spec.
    pub(crate) fn format_runtime_value(
        &mut self,
        value: &Value,
        conversion: u8,
        format_spec: Option<&str>,
    ) -> Result<String, RunError> {
        if conversion != 0 {
            let converted = self.convert_value(value, conversion)?;
            if let Some(format_spec) = format_spec {
                self.format_runtime_string(&converted, format_spec)
            } else {
                Ok(converted)
            }
        } else if let Some(format_spec) = format_spec {
            if let Some(formatted) = self.try_format_temporal(value, format_spec)? {
                Ok(formatted)
            } else {
                let value_type = value.py_type_name(self);
                let spec = self.parse_runtime_spec(format_spec, &value_type)?;
                self.format_parsed_value(value, 0, &spec)
            }
        } else {
            self.convert_value(value, 0)
        }
    }

    /// Formats an already-converted string from a runtime format spec.
    pub(crate) fn format_runtime_string(&mut self, value: &str, format_spec: &str) -> Result<String, RunError> {
        let spec = self.parse_runtime_spec(format_spec, "str")?;
        self.format_parsed_string(value, &spec)
    }

    /// Formats a value from a parsed format spec.
    fn format_parsed_value(
        &mut self,
        value: &Value,
        conversion: u8,
        spec: &ParsedFormatSpec,
    ) -> Result<String, RunError> {
        if conversion == 0 {
            format_with_spec(value, spec, self)
        } else {
            let s = self.convert_value(value, conversion)?;
            self.format_parsed_string(&s, spec)
        }
    }

    /// Formats a string from a parsed format spec.
    fn format_parsed_string(&self, value: &str, spec: &ParsedFormatSpec) -> Result<String, RunError> {
        validate_string_spec(spec)?;
        format_string(value, spec, &self.heap.tracker)
    }

    /// Applies the f-string and `str.format()` conversion flags to a value.
    pub(crate) fn convert_value(&mut self, value: &Value, conversion: u8) -> Result<String, RunError> {
        match conversion {
            0 | 1 if value.py_type(self) == Type::Str => Ok(value.to_str(self)?.to_owned()),
            2 => str_value_into_string(value.py_repr(self)?, self),
            3 => {
                let value = str_value_into_string(value.py_repr(self)?, self)?;
                let mut escaped = StringBuilder::with_capacity(value.len(), &self.heap.tracker)?;
                for (index, character) in value.chars().enumerate() {
                    self.heap.tracker.check_time_every(index)?;
                    if character.is_ascii() {
                        escaped.push(character)?;
                    } else {
                        let code = u32::from(character);
                        let result = if code <= 0xff {
                            write!(escaped, "\\x{code:02x}")
                        } else if code <= 0xffff {
                            write!(escaped, "\\u{code:04x}")
                        } else {
                            write!(escaped, "\\U{code:08x}")
                        };
                        if result.is_err() {
                            return escaped.finish_raw();
                        }
                    }
                }
                escaped.finish_raw()
            }
            // No conversion and `!s` both use `str()`.
            _ => str_value_into_string(value.py_str(self)?, self),
        }
    }

    /// Keeps temporal strftime specs out of generic mini-language parsing.
    fn try_format_temporal(&mut self, value: &Value, spec_str: &str) -> Result<Option<String>, RunError> {
        let Value::Ref(id) = value else {
            return Ok(None);
        };
        let id = *id;
        let temporal = matches!(
            self.heap.read(id),
            HeapReadOutput::Date(_) | HeapReadOutput::DateTime(_) | HeapReadOutput::Time(_)
        );
        if !temporal {
            return Ok(None);
        }

        // `datetime.__format__("")` falls back to `str()`.
        if spec_str.is_empty() {
            return self.convert_value(value, 0).map(Some);
        }

        let formatted = match self.heap.read(id) {
            HeapReadOutput::Date(d) => format_date_strftime(*d.get(self.heap), spec_str),
            HeapReadOutput::DateTime(d) => format_datetime_strftime(d.get(self.heap), spec_str),
            HeapReadOutput::Time(t) => format_time_strftime(t.get(self.heap), spec_str),
            _ => unreachable!("temporal-ness checked above"),
        };
        formatted.map(Some)
    }

    /// Parses a runtime spec without copying it onto an error path.
    fn parse_runtime_spec(&self, spec: &str, value_type: &str) -> Result<ParsedFormatSpec, RunError> {
        let reason = match ParsedFormatSpec::parse_runtime(spec) {
            Ok(parsed) => return Ok(parsed),
            Err(reason) => reason,
        };
        let mut message = StringBuilder::new(&self.heap.tracker);
        match &reason {
            ParseFormatSpecReason::Malformed => {
                message.push_str("Invalid format specifier '")?;
                message.push_str(spec)?;
                message.push('\'')?;
            }
            ParseFormatSpecReason::NumberOverflow => {
                message.push_str("Invalid format specifier '")?;
                message.push_str(spec)?;
                message.push_str("': width or precision overflows usize")?;
            }
            ParseFormatSpecReason::MissingPrecision => {
                message.push_str("Format specifier missing precision")?;
            }
            ParseFormatSpecReason::UnknownFormatCode(code) => {
                message.push_str("Unknown format code '")?;
                message.push(*code)?;
                message.push('\'')?;
            }
            ParseFormatSpecReason::GroupingConflict(detail) => message.push_str(detail)?,
        }
        if reason.needs_type_suffix() {
            message.push_str(" for object of type '")?;
            message.push_str(value_type)?;
            message.push('\'')?;
        }
        let message = message.finish_raw()?;
        Err(SimpleException::new_msg(ExcType::ValueError, message).into())
    }
}

/// Resolves a `str` `Value` (as returned by `py_str`/`py_repr`) to an owned
/// `String`, dropping the value's heap reference on every path. Used by the
/// f-string conversion arms, which need the text in an owned buffer to feed
/// the mini-language formatter.
fn str_value_into_string(value: Value, vm: &mut VM<'_>) -> Result<String, RunError> {
    defer_drop!(value, vm);
    Ok(value.to_str(vm)?.to_owned())
}
