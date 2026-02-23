//! F-string and value formatting helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError, SimpleException},
    fstring::{ParsedFormatSpec, ascii_escape, decode_format_spec, format_string, format_with_spec},
    resource::ResourceTracker,
    types::{PyTrait, str::allocate_string},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, '_, T> {
    /// Builds an f-string by concatenating n string parts from the stack.
    pub(super) fn build_fstring(&mut self, count: usize) -> Result<(), RunError> {
        let parts = self.pop_n(count);
        let mut result = String::new();

        for part in parts {
            // Each part should be a string (interned or heap-allocated)
            let part_str = part.py_str(self.heap, self.interns);
            result.push_str(&part_str);
            part.drop_with_heap(self.heap);
        }

        let value = allocate_string(result, self.heap)?;
        self.push(value);
        Ok(())
    }

    /// Formats a value for f-string interpolation.
    ///
    /// Flags encoding:
    /// - bits 0-1: conversion (0=none, 1=str, 2=repr, 3=ascii)
    /// - bit 2: has format spec on stack
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
        let has_format_spec = (flags & 0x04) != 0;

        // Pop format spec if present (pushed before value, so popped after)
        let format_spec = if has_format_spec { Some(this.pop()) } else { None };

        let value = this.pop();
        defer_drop!(value, this);

        // Format with spec applied to original value type, or convert and format as string
        let formatted = if let Some(spec_value) = format_spec {
            defer_drop!(spec_value, this);

            let spec = this.get_format_spec(spec_value, value)?;

            match conversion {
                // No conversion - format original value
                0 => format_with_spec(value, &spec, this.heap, this.interns)?,
                // !s - convert to str, format as string
                1 => {
                    let s = value.py_str(this.heap, this.interns);
                    format_string(&s, &spec)?
                }
                // !r - convert to repr, format as string
                2 => {
                    let s = value.py_repr(this.heap, this.interns);
                    format_string(&s, &spec)?
                }
                // !a - convert to ascii, format as string
                3 => {
                    let s = ascii_escape(&value.py_repr(this.heap, this.interns));
                    format_string(&s, &spec)?
                }
                _ => format_with_spec(value, &spec, this.heap, this.interns)?,
            }
        } else {
            // No format spec - just convert based on conversion flag
            match conversion {
                0 => value.py_str(this.heap, this.interns).into_owned(),
                1 => value.py_str(this.heap, this.interns).into_owned(),
                2 => value.py_repr(this.heap, this.interns).into_owned(),
                3 => ascii_escape(&value.py_repr(this.heap, this.interns)),
                _ => value.py_str(this.heap, this.interns).into_owned(),
            }
        };

        let result = allocate_string(formatted, this.heap)?;
        this.push(result);
        Ok(())
    }

    /// Gets a ParsedFormatSpec from a format spec value.
    ///
    /// The `value_for_error` parameter is used to include the value type in error messages.
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    fn get_format_spec(&self, spec_value: &Value, value_for_error: &Value) -> Result<ParsedFormatSpec, RunError> {
        match spec_value {
            Value::Int(n) if *n < 0 => {
                // Decode the encoded format spec; n < 0 ensures (-n - 1) >= 0
                let encoded = u64::try_from((-*n) - 1).expect("format spec encoding validated non-negative");
                Ok(decode_format_spec(encoded))
            }
            _ => {
                // Dynamic format spec - parse the string
                let spec_str = spec_value.py_str(self.heap, self.interns);
                spec_str.parse::<ParsedFormatSpec>().map_err(|invalid| {
                    // Only fetch type in error path
                    let value_type = value_for_error.py_type(self.heap);
                    RunError::Exc(
                        SimpleException::new_msg(
                            ExcType::ValueError,
                            format!("Invalid format specifier '{invalid}' for object of type '{value_type}'"),
                        )
                        .into(),
                    )
                })
            }
        }
    }
}
