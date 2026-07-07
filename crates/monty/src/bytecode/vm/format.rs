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
    resource::{ResourceTracker, check_repeat_size},
    types::{PyTrait, date::format_date_strftime, datetime::format_datetime_strftime, str::allocate_string},
    value::Value,
};

impl<T: ResourceTracker> VM<'_, T> {
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

        let value = allocate_string(result, this.heap)?;
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

        // Format with spec applied to original value type, or convert and format as string
        let formatted = if let Some(spec_value) = format_spec {
            defer_drop!(spec_value, this);

            // date/datetime: with no conversion flag, CPython hands the whole
            // spec to the value's `__format__`, which treats it as a strftime
            // string (`f"{dt:%Y-%m-%d}"`). Only the runtime (dynamic) spec path
            // carries the raw string; a valid mini-language spec on a temporal
            // value (rare/nonsensical) still takes the generic route below.
            let temporal = if conversion == 0 && !static_spec {
                this.try_format_temporal(value, spec_value)?
            } else {
                None
            };

            if let Some(formatted) = temporal {
                formatted
            } else {
                let spec = this.get_format_spec(spec_value, value, static_spec)?;

                // Pre-check: reject format specs with huge width before pad_string
                // allocates an untracked Rust String.
                check_repeat_size(spec.width, spec.fill.len_utf8(), this.heap.tracker())?;

                if conversion == 0 {
                    // No conversion: format the original value through its own
                    // type (`format_with_spec` does the type-specific validation).
                    format_with_spec(value, &spec, this)?
                } else {
                    // `!s`/`!r`/`!a` convert to a string first, so the spec is now
                    // a *string* spec: CPython applies it to the converted text and
                    // rejects flags that are illegal there (`#`, `,`, `+`, a non-`s`
                    // type, …). Validate via the same `validate_string_spec` the
                    // `str` branch of `format_with_spec` uses, then format.
                    let s = match conversion {
                        2 => str_value_into_string(value.py_repr(this)?, this)?,
                        3 => ascii_escape(&str_value_into_string(value.py_repr(this)?, this)?),
                        // `!s` (1) and any unused bit pattern fall back to `str()`.
                        _ => str_value_into_string(value.py_str(this)?, this)?,
                    };
                    validate_string_spec(&spec)?;
                    format_string(&s, &spec)?
                }
            }
        } else {
            // No format spec - just convert based on conversion flag
            match conversion {
                0 => str_value_into_string(value.py_str(this)?, this)?,
                1 => str_value_into_string(value.py_str(this)?, this)?,
                2 => str_value_into_string(value.py_repr(this)?, this)?,
                3 => ascii_escape(&str_value_into_string(value.py_repr(this)?, this)?),
                _ => str_value_into_string(value.py_str(this)?, this)?,
            }
        };

        let result = allocate_string(formatted, this.heap)?;
        this.push(result);
        Ok(())
    }

    /// Formats a `date`/`datetime` value by treating the spec as a `strftime`
    /// string, mirroring CPython's `__format__` for temporal types
    /// (`f"{dt:%Y-%m-%d}"`).
    ///
    /// Returns `Ok(None)` for any non-temporal value so the caller falls back
    /// to the generic mini-language formatter. `spec_value` is the runtime
    /// (dynamic) spec string; an empty spec maps to `str()`, matching
    /// `datetime.__format__('')`.
    fn try_format_temporal(&mut self, value: &Value, spec_value: &Value) -> Result<Option<String>, RunError> {
        let this = self;
        let Value::Ref(id) = value else {
            return Ok(None);
        };
        let id = *id;
        let temporal = matches!(
            this.heap.read(id),
            HeapReadOutput::Date(_) | HeapReadOutput::DateTime(_)
        );
        if !temporal {
            return Ok(None);
        }

        let spec_str_value = spec_value.py_str(this)?;
        defer_drop!(spec_str_value, this);
        let spec_str = spec_str_value.to_str(this)?;
        // An empty (dynamic) spec behaves like `str()`; strftime("") is also
        // "" but routing explicitly keeps the intent clear.
        if spec_str.is_empty() {
            return str_value_into_string(value.py_str(this)?, this).map(Some);
        }

        let formatted = match this.heap.read(id) {
            HeapReadOutput::Date(d) => format_date_strftime(*d.get(this.heap), spec_str),
            HeapReadOutput::DateTime(d) => format_datetime_strftime(d.get(this.heap), spec_str),
            _ => unreachable!("temporal-ness checked above"),
        };
        formatted.map(Some)
    }

    /// Resolves a format spec value pushed by `compile_format_value` into a
    /// [`ParsedFormatSpec`].
    ///
    /// `static_spec` is the discriminator from the `FormatValue` flags
    /// ([`FORMAT_VALUE_STATIC_SPEC`]): when set, the compiler emitted the
    /// spec as `Value::Int(encoded)` and we just decode the bit-packed form;
    /// otherwise the spec was constructed at runtime and we parse its string
    /// representation. `value_for_error` is used to include the formatted
    /// value's type in the parse-error message; we only fetch `py_type()`
    /// on the error path.
    fn get_format_spec(
        &mut self,
        spec_value: &Value,
        value_for_error: &Value,
        static_spec: bool,
    ) -> Result<ParsedFormatSpec, RunError> {
        if static_spec {
            // Compiler invariant: the static-spec flag is only emitted
            // alongside a LoadConst of Value::Int(encoded).
            let Value::Int(encoded) = spec_value else {
                unreachable!("FORMAT_VALUE_STATIC_SPEC flag without Value::Int on stack");
            };
            Ok(decode_format_spec(*encoded))
        } else {
            let this = self;
            let spec_str_value = spec_value.py_str(this)?;
            defer_drop!(spec_str_value, this);
            spec_str_value.to_str(this)?.parse::<ParsedFormatSpec>().map_err(|err| {
                // CPython suffixes the value's type onto some spec errors
                // (`Invalid format specifier`, `Unknown format code`) but not
                // others (`Format specifier missing precision`, the `Cannot
                // specify …` grouping conflicts), which are self-contained.
                let message = if err.needs_type_suffix() {
                    let value_type = value_for_error.py_type_name(this);
                    format!("{err} for object of type '{value_type}'")
                } else {
                    err.to_string()
                };
                RunError::Exc(SimpleException::new_msg(ExcType::ValueError, message).into())
            })
        }
    }
}

/// Resolves a `str` `Value` (as returned by `py_str`/`py_repr`) to an owned
/// `String`, dropping the value's heap reference on every path. Used by the
/// f-string conversion arms, which need the text in an owned buffer to feed
/// the mini-language formatter.
fn str_value_into_string(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Result<String, RunError> {
    defer_drop!(value, vm);
    Ok(value.to_str(vm)?.to_owned())
}
