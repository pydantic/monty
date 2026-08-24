//! F-string and value formatting helpers for the VM.

use super::VM;
use crate::{
    bytecode::op::{FORMAT_VALUE_HAS_SPEC, FORMAT_VALUE_STATIC_SPEC},
    defer_drop,
    exception_private::{ExcType, RunError, SimpleException},
    fstring::{
        ParsedFormatSpec, ascii_escape, decode_format_spec, format_string, format_with_spec, validate_string_spec,
    },
    heap::HeapReadOutput,
    resource_checks::check_repeat_size,
    types::{
        PyTrait, date::format_date_strftime, datetime::format_datetime_strftime, str::allocate_string,
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
    ///
    /// Python f-string formatting order:
    /// 1. Apply format spec to original value (type-specific formatting)
    /// 2. Apply conversion flag to the result
    ///
    /// However, conversion flags like !s, !r, !a are applied BEFORE formatting
    /// if the value would be repr'd. The key insight is:
    /// - No conversion: format the original value type
    /// - !s conversion: convert to str first, then format as string
    /// - !r conversion: convert to repr first, then format as string
    /// - !a conversion: convert to ascii repr first, then format as string
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

    pub(crate) fn format_runtime_value(
        &mut self,
        value: &Value,
        conversion: u8,
        format_spec: Option<&str>,
    ) -> Result<String, RunError> {
        if let Some(format_spec) = format_spec {
            // Temporal specs are strftime strings, not mini-language specs.
            if conversion == 0
                && let Some(formatted) = self.try_format_temporal(value, format_spec)?
            {
                return Ok(formatted);
            }

            let spec = self.parse_runtime_format_spec(format_spec, value)?;
            self.format_parsed_value(value, conversion, &spec)
        } else {
            self.convert_value(value, conversion)
        }
    }

    fn format_parsed_value(
        &mut self,
        value: &Value,
        conversion: u8,
        spec: &ParsedFormatSpec,
    ) -> Result<String, RunError> {
        // Keep pad_string from allocating before the tracker sees the width.
        check_repeat_size(spec.width, spec.fill.len_utf8(), &self.heap.tracker)?;

        if conversion == 0 {
            format_with_spec(value, spec, self)
        } else {
            // Conversions happen first, so the spec must be valid for strings.
            let s = self.convert_value(value, conversion)?;
            validate_string_spec(spec)?;
            Ok(format_string(&s, spec)?)
        }
    }

    fn convert_value(&mut self, value: &Value, conversion: u8) -> Result<String, RunError> {
        match conversion {
            2 => str_value_into_string(value.py_repr(self)?, self),
            3 => Ok(ascii_escape(&str_value_into_string(value.py_repr(self)?, self)?)),
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

    /// Adds the value type only to errors where CPython does.
    fn parse_runtime_format_spec(
        &mut self,
        format_spec: &str,
        value_for_error: &Value,
    ) -> Result<ParsedFormatSpec, RunError> {
        format_spec.parse::<ParsedFormatSpec>().map_err(|err| {
            let message = if err.needs_type_suffix() {
                let value_type = value_for_error.py_type_name(self);
                format!("{err} for object of type '{value_type}'")
            } else {
                err.to_string()
            };
            RunError::Exc(SimpleException::new_msg(ExcType::ValueError, message).into())
        })
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
