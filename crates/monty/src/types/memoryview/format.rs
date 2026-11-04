//! Scalar codecs use a deterministic little-endian LP64 layout.
//! Pointer-format values are integers and never become host pointers.

use num_traits::ToPrimitive;

use super::MemoryView;
use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{HeapData, HeapObjectRead},
    types::{LongInt, PyTrait, bytes::allocate_bytes, instance::instance_call_dunder_sync},
    value::{VALUE_SIZE, Value},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BufferFormat(char);

impl BufferFormat {
    pub(crate) fn parse(format: &str) -> RunResult<Self> {
        let format = format.strip_prefix('@').unwrap_or(format);
        let mut chars = format.chars();
        if let Some(code) = chars.next()
            && chars.next().is_none()
            && let Some(format) = Self::from_code(code)
        {
            return Ok(format);
        }
        Err(ExcType::value_error(
            "memoryview: destination format must be a native single character format prefixed with an optional '@'",
        ))
    }

    pub(crate) fn from_code(code: char) -> Option<Self> {
        matches!(
            code,
            'B' | 'b' | 'c' | '?' | 'h' | 'H' | 'i' | 'I' | 'l' | 'L' | 'q' | 'Q' | 'n' | 'N' | 'e' | 'f' | 'd' | 'P'
        )
        .then_some(Self(code))
    }

    pub(crate) fn code(self) -> char {
        self.0
    }

    pub(crate) fn itemsize(self) -> usize {
        match self.0 {
            'B' | 'b' | 'c' | '?' => 1,
            'h' | 'H' | 'e' => 2,
            'i' | 'I' | 'f' => 4,
            'l' | 'L' | 'q' | 'Q' | 'n' | 'N' | 'd' | 'P' => 8,
            _ => unreachable!("validated buffer format"),
        }
    }

    /// Collection preflight includes heap storage for one-byte bytes and large unsigned integers.
    pub(crate) fn allocation_cost_hint(self) -> usize {
        VALUE_SIZE
            + if matches!(self.0, 'c' | 'L' | 'Q' | 'N' | 'P') {
                128
            } else {
                0
            }
    }

    pub(crate) fn unpack(self, bytes: &[u8], vm: &VM<'_>) -> RunResult<Value> {
        if bytes.len() != self.itemsize() {
            return Err(ExcType::value_error("memoryview: invalid item size"));
        }
        Ok(match self.0 {
            'B' => Value::Int(i64::from(bytes[0])),
            'b' => Value::Int(i64::from(bytes[0].cast_signed())),
            'c' => allocate_bytes(vec![bytes[0]], vm.heap),
            '?' => Value::Bool(bytes[0] != 0),
            'h' => Value::Int(i64::from(i16::from_le_bytes(array(bytes)?))),
            'H' => Value::Int(i64::from(u16::from_le_bytes(array(bytes)?))),
            'i' => Value::Int(i64::from(i32::from_le_bytes(array(bytes)?))),
            'I' => Value::Int(i64::from(u32::from_le_bytes(array(bytes)?))),
            'l' | 'q' | 'n' => Value::Int(i64::from_le_bytes(array(bytes)?)),
            'L' | 'Q' | 'N' | 'P' => LongInt::value_from_u128(u128::from(u64::from_le_bytes(array(bytes)?)), vm.heap),
            'e' => Value::Float(decode_half(u16::from_le_bytes(array(bytes)?))),
            'f' => Value::Float(f64::from(f32::from_le_bytes(array(bytes)?))),
            'd' => Value::Float(f64::from_le_bytes(array(bytes)?)),
            _ => unreachable!("validated buffer format"),
        })
    }

    pub(crate) fn pack<'h>(
        self,
        value: &Value,
        view: &HeapObjectRead<'h, MemoryView>,
        vm: &mut VM<'h>,
    ) -> RunResult<Vec<u8>> {
        match self.0 {
            'c' => {
                let bytes = match value {
                    Value::InternBytes(id) => Some(vm.interns.get_bytes(*id)),
                    Value::Ref(id) => match vm.heap.get(*id) {
                        HeapData::Bytes(bytes) => Some(bytes.as_slice()),
                        _ => None,
                    },
                    _ => None,
                };
                let Some(bytes) = bytes else {
                    return Err(self.invalid_type());
                };
                let [byte] = bytes else {
                    return Err(self.invalid_value());
                };
                Ok(vec![*byte])
            }
            '?' => Ok(vec![u8::from(buffer_truth(value, vm)?)]),
            'e' | 'f' | 'd' => {
                let value = value
                    .py_float(vm)
                    .map_err(|error| self.conversion_error(error))?
                    .ok_or_else(|| self.invalid_type())?;
                // Float conversion may release the view; that error precedes half-format overflow.
                view.get(vm.heap).check_live()?;
                match self.0 {
                    'e' => encode_half(value)
                        .map(|bits| bits.to_le_bytes().to_vec())
                        .ok_or_else(|| self.invalid_value()),
                    'f' => Ok(encode_float(value).to_vec()),
                    'd' => Ok(value.to_le_bytes().to_vec()),
                    _ => unreachable!("float format"),
                }
            }
            _ => self.pack_integer(value, vm),
        }
    }

    fn pack_integer(self, value: &Value, vm: &mut VM<'_>) -> RunResult<Vec<u8>> {
        let index = if self.0 == 'P' {
            // CPython's pointer converter accepts actual ints rather than the index protocol.
            if !matches!(value, Value::Int(_) | Value::Bool(_)) && value.as_long_int(vm).is_none() {
                return Err(self.invalid_type());
            }
            value.clone_with_heap(vm.heap)
        } else {
            value
                .py_index_impl(vm)
                .map_err(|error| self.conversion_error(error))?
                .ok_or_else(|| self.invalid_type())?
        };
        defer_drop!(index, vm);
        let n = match index {
            Value::Int(n) => Some(i128::from(*n)),
            Value::Bool(n) => Some(i128::from(u8::from(*n))),
            _ => index.as_long_int(vm).and_then(ToPrimitive::to_i128),
        }
        .ok_or_else(|| self.invalid_value())?;
        let signed = matches!(self.0, 'b' | 'h' | 'i' | 'l' | 'q' | 'n');
        let bits = self.itemsize() * 8;
        let min = if self.0 == 'P' {
            i128::from(i64::MIN)
        } else if signed {
            -(1i128 << (bits - 1))
        } else {
            0
        };
        let max = if signed {
            (1i128 << (bits - 1)) - 1
        } else {
            (1i128 << bits) - 1
        };
        if n < min || n > max {
            return Err(self.invalid_value());
        }
        let encoded = if n < 0 {
            i64::try_from(n).expect("checked signed range").cast_unsigned()
        } else {
            u64::try_from(n).expect("checked unsigned range")
        };
        Ok(encoded.to_le_bytes()[..self.itemsize()].to_vec())
    }

    fn invalid_type(self) -> RunError {
        ExcType::type_error(format!("memoryview: invalid type for format '{}'", self.0))
    }

    fn invalid_value(self) -> RunError {
        ExcType::value_error(format!("memoryview: invalid value for format '{}'", self.0))
    }

    fn conversion_error(self, error: RunError) -> RunError {
        if let RunError::Exc(raise) = &error {
            let kind = raise.exc.exc_type();
            if kind.is_subclass_of(ExcType::TypeError) {
                return self.invalid_type();
            }
            if kind.is_subclass_of(ExcType::ValueError) || kind.is_subclass_of(ExcType::OverflowError) {
                return self.invalid_value();
            }
        }
        error
    }
}

fn buffer_truth(value: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    if let Value::Ref(id) = value
        && matches!(vm.heap.get(*id), HeapData::Instance(_))
    {
        if let Some(result) = instance_call_dunder_sync(*id, "__bool__", None, vm)? {
            defer_drop!(result, vm);
            return match result {
                Value::Bool(value) => Ok(*value),
                _ => Err(ExcType::type_error(format!(
                    "__bool__ should return bool, returned {}",
                    result.py_type_name(vm)
                ))),
            };
        }
        if let Some(result) = instance_call_dunder_sync(*id, "__len__", None, vm)? {
            defer_drop!(result, vm);
            let length = result.as_int_with_overflow(vm, || ExcType::overflow_index_sized_int().into())?;
            if length < 0 {
                return Err(ExcType::value_error("__len__() should return >= 0"));
            }
            return Ok(length != 0);
        }
    }
    value.py_bool(vm)
}

fn array<const N: usize>(bytes: &[u8]) -> RunResult<[u8; N]> {
    bytes
        .try_into()
        .map_err(|_| ExcType::value_error("memoryview: invalid item size"))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "IEEE binary32 packing intentionally rounds binary64 and permits infinity"
)]
fn encode_float(value: f64) -> [u8; 4] {
    (value as f32).to_le_bytes()
}

fn decode_half(bits: u16) -> f64 {
    let exponent = (bits >> 10) & 31;
    let fraction = f64::from(bits & 1023);
    let magnitude = match exponent {
        0 => fraction * 2f64.powi(-24),
        // Widen NaNs by moving their payload bits, preserving signaling state.
        31 => f64::from_bits(0x7ff0_0000_0000_0000 | (u64::from(bits & 1023) << 42)),
        _ => (1.0 + fraction / 1024.0) * 2f64.powi(i32::from(exponent) - 15),
    };
    if bits & 0x8000 != 0 { -magnitude } else { magnitude }
}

fn encode_half(value: f64) -> Option<u16> {
    let bits = value.to_bits();
    let sign = u16::try_from((bits >> 48) & 0x8000).expect("sign bit");
    let exponent = i32::from(u16::try_from((bits >> 52) & 0x7ff).expect("exponent bits")) - 1023;
    let fraction = bits & ((1u64 << 52) - 1);
    if exponent == 1024 {
        let payload = u16::try_from(fraction >> 42).expect("half NaN payload");
        // Truncating a nonzero payload to zero must not turn a NaN into infinity.
        let payload = if fraction != 0 && payload == 0 { 0x200 } else { payload };
        return Some(sign | 0x7c00 | payload);
    }
    if exponent < -25 {
        return Some(sign);
    }
    let significand = fraction | (1u64 << 52);
    if exponent < -14 {
        let rounded = round_shift(significand, u32::try_from(28 - exponent).expect("subnormal shift"));
        return Some(sign | u16::try_from(rounded).expect("half subnormal"));
    }
    if exponent > 15 {
        return None;
    }
    let mut exponent = exponent;
    let mut rounded = round_shift(significand, 42);
    if rounded == 2048 {
        exponent += 1;
        rounded = 1024;
    }
    if exponent > 15 {
        return None;
    }
    let exponent = u16::try_from(exponent + 15).expect("half exponent");
    let fraction = u16::try_from(rounded - 1024).expect("half fraction");
    Some(sign | (exponent << 10) | fraction)
}

/// Round ties to even directly from binary64, avoiding double rounding through binary32.
fn round_shift(value: u64, shift: u32) -> u64 {
    let high = value >> shift;
    let low = value & ((1u64 << shift) - 1);
    let halfway = 1u64 << (shift - 1);
    high + u64::from(low > halfway || (low == halfway && high & 1 != 0))
}

#[cfg(test)]
mod tests {
    use super::{BufferFormat, decode_half, encode_half};

    #[test]
    fn half_patterns_round_trip() {
        for bits in 0..=u16::MAX {
            assert_eq!(encode_half(decode_half(bits)), Some(bits));
        }
    }

    #[test]
    fn half_midpoints_round_to_even() {
        for lower in 0..0x7bffu16 {
            let upper = lower + 1;
            let midpoint = decode_half(lower).midpoint(decode_half(upper));
            let tied = if lower & 1 == 0 { lower } else { upper };
            for (value, expected) in [
                (midpoint.next_down(), lower),
                (midpoint, tied),
                (midpoint.next_up(), upper),
            ] {
                assert_eq!(encode_half(value), Some(expected));
                assert_eq!(encode_half(-value), Some(expected | 0x8000));
            }
        }
    }

    #[test]
    fn half_rounding_and_overflow() {
        assert_eq!(encode_half(65519.0), Some(0x7bff));
        assert_eq!(encode_half(65520.0), None);
        assert_eq!(encode_half(2f64.powi(-25)), Some(0));
        assert_eq!(encode_half(2f64.powi(-24)), Some(1));
        assert_eq!(encode_half(-0.0), Some(0x8000));
        assert_eq!(encode_half(f64::INFINITY), Some(0x7c00));
        assert_eq!(encode_half(f64::from_bits(0x7ff0_0000_0000_0001)), Some(0x7e00));
        assert_eq!(encode_half(f64::from_bits(0xfff4_0000_0000_0000)), Some(0xfd00));
        assert_eq!(encode_half(1.0 + 2f64.powi(-11)), Some(0x3c00));
        assert_eq!(encode_half(1.0 + 3.0 * 2f64.powi(-11)), Some(0x3c02));
    }

    #[test]
    fn parse_native_formats() {
        for code in "Bbc?hHiIlLqQnNefdP".chars() {
            assert_eq!(BufferFormat::parse(&format!("@{code}")).unwrap().code(), code);
        }
        for code in ["", "@", "@@B", "<i", "=i", "2i", "F", "D", "w", "x"] {
            assert!(BufferFormat::parse(code).is_err());
        }
    }
}
