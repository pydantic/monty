//! F-string and value formatting helpers for the VM.

use super::VM;
use crate::{
    exception_private::{ExcType, RunError, SimpleException},
    fstring::{ParsedFormatSpec, ascii_escape, decode_format_spec, format_string, format_with_spec},
    heap::HeapData,
    io::PrintWriter,
    resource::ResourceTracker,
    types::{PyTrait, Str},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
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

        let heap_id = self.heap.allocate(HeapData::Str(Str::new(result)))?;
        self.push(Value::Ref(heap_id));
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
        let conversion = flags & 0x03;
        let has_format_spec = (flags & 0x04) != 0;

        // Pop format spec if present (pushed before value, so popped after)
        let format_spec = if has_format_spec {
            let spec_value = self.pop();
            Some(spec_value)
        } else {
            None
        };

        let value = self.pop();

        // Format with spec applied to original value type, or convert and format as string
        let formatted = if let Some(spec_value) = format_spec {
            // Get the parsed format spec
            let spec = match self.get_format_spec(&spec_value, &value) {
                Ok(s) => s,
                Err(e) => {
                    // Clean up both values before returning error
                    spec_value.drop_with_heap(self.heap);
                    value.drop_with_heap(self.heap);
                    return Err(e);
                }
            };

            // Format based on value type and conversion flag
            // Use a helper closure to handle errors with proper cleanup
            let format_result: Result<String, RunError> = match conversion {
                // No conversion - format original value
                0 => format_with_spec(&value, &spec, self.heap, self.interns),
                // !s - convert to str, format as string
                1 => {
                    let s = value.py_str(self.heap, self.interns);
                    format_string(&s, &spec).map_err(Into::into)
                }
                // !r - convert to repr, format as string
                2 => {
                    let s = value.py_repr(self.heap, self.interns);
                    format_string(&s, &spec).map_err(Into::into)
                }
                // !a - convert to ascii, format as string
                3 => {
                    let s = ascii_escape(&value.py_repr(self.heap, self.interns));
                    format_string(&s, &spec).map_err(Into::into)
                }
                _ => format_with_spec(&value, &spec, self.heap, self.interns),
            };

            // Handle format errors with proper cleanup
            match format_result {
                Ok(result) => {
                    spec_value.drop_with_heap(self.heap);
                    result
                }
                Err(e) => {
                    spec_value.drop_with_heap(self.heap);
                    value.drop_with_heap(self.heap);
                    return Err(e);
                }
            }
        } else {
            // No format spec - just convert based on conversion flag
            match conversion {
                0 => value.py_str(self.heap, self.interns).into_owned(),
                1 => value.py_str(self.heap, self.interns).into_owned(),
                2 => value.py_repr(self.heap, self.interns).into_owned(),
                3 => ascii_escape(&value.py_repr(self.heap, self.interns)),
                _ => value.py_str(self.heap, self.interns).into_owned(),
            }
        };

        value.drop_with_heap(self.heap);

        let heap_id = self.heap.allocate(HeapData::Str(Str::new(formatted)))?;
        self.push(Value::Ref(heap_id));
        Ok(())
    }

    /// Gets a ParsedFormatSpec from a format spec value.
    ///
    /// The `value_for_error` parameter is used to include the value type in error messages.
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
                let value_type = value_for_error.py_type(self.heap);
                spec_str.parse::<ParsedFormatSpec>().map_err(|invalid| {
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
