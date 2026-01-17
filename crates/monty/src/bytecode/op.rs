//! Opcode definitions for the bytecode VM.
//!
//! Bytecode is stored as raw `Vec<u8>` for cache efficiency. The `Opcode` enum is a pure
//! discriminant with no data - operands are fetched separately from the byte stream.
//!
//! # Operand Encoding
//!
//! - No suffix, 0 bytes: `BinaryAdd`, `Pop`, `LoadNone`
//! - No suffix, 1 byte (u8/i8): `LoadLocal`, `StoreLocal`, `LoadSmallInt`
//! - `W` suffix, 2 bytes (u16/i16): `LoadLocalW`, `Jump`, `LoadConst`
//! - Compound (multiple operands): `CallFunctionKw` (u8 + u8), `MakeClosure` (u16 + u8)

use strum::FromRepr;

/// Opcode discriminant - just identifies the instruction type.
///
/// Operands (if any) follow in the bytecode stream and are fetched separately.
/// With `#[repr(u8)]`, each opcode is exactly 1 byte. Uses `strum::FromRepr` for
/// efficient byte-to-opcode conversion (bounds check + transmute).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr)]
pub enum Opcode {
    // === Stack Operations (no operand) ===
    /// Discard top of stack.
    Pop,
    /// Duplicate top of stack.
    Dup,
    /// Swap top two: [a, b] -> [b, a].
    Rot2,
    /// Rotate top three: [a, b, c] -> [c, a, b].
    Rot3,

    // === Constants & Literals ===
    /// Push constant from pool. Operand: u16 const_id.
    LoadConst,
    /// Push None.
    LoadNone,
    /// Push True.
    LoadTrue,
    /// Push False.
    LoadFalse,
    /// Push small integer (-128 to 127). Operand: i8.
    LoadSmallInt,

    // === Variables ===
    // Specialized no-operand versions for common slots (hot path)
    /// Push local slot 0 (often 'self').
    LoadLocal0,
    /// Push local slot 1.
    LoadLocal1,
    /// Push local slot 2.
    LoadLocal2,
    /// Push local slot 3.
    LoadLocal3,
    // General versions with operand
    /// Push local variable. Operand: u8 slot.
    LoadLocal,
    /// Push local (wide, slot > 255). Operand: u16 slot.
    LoadLocalW,
    /// Pop and store to local. Operand: u8 slot.
    StoreLocal,
    /// Store local (wide). Operand: u16 slot.
    StoreLocalW,
    /// Push from global namespace. Operand: u16 slot.
    LoadGlobal,
    /// Store to global. Operand: u16 slot.
    StoreGlobal,
    /// Load from closure cell. Operand: u16 slot.
    LoadCell,
    /// Store to closure cell. Operand: u16 slot.
    StoreCell,
    /// Delete local variable. Operand: u8 slot.
    DeleteLocal,

    // === Binary Operations (no operand) ===
    /// Add: a + b.
    BinaryAdd,
    /// Subtract: a - b.
    BinarySub,
    /// Multiply: a * b.
    BinaryMul,
    /// Divide: a / b.
    BinaryDiv,
    /// Floor divide: a // b.
    BinaryFloorDiv,
    /// Modulo: a % b.
    BinaryMod,
    /// Power: a ** b.
    BinaryPow,
    /// Bitwise AND: a & b.
    BinaryAnd,
    /// Bitwise OR: a | b.
    BinaryOr,
    /// Bitwise XOR: a ^ b.
    BinaryXor,
    /// Left shift: a << b.
    BinaryLShift,
    /// Right shift: a >> b.
    BinaryRShift,
    /// Matrix multiply: a @ b.
    BinaryMatMul,

    // === Comparison Operations (no operand) ===
    /// Equal: a == b.
    CompareEq,
    /// Not equal: a != b.
    CompareNe,
    /// Less than: a < b.
    CompareLt,
    /// Less than or equal: a <= b.
    CompareLe,
    /// Greater than: a > b.
    CompareGt,
    /// Greater than or equal: a >= b.
    CompareGe,
    /// Identity: a is b.
    CompareIs,
    /// Not identity: a is not b.
    CompareIsNot,
    /// Membership: a in b.
    CompareIn,
    /// Not membership: a not in b.
    CompareNotIn,
    /// Modulo equality: a % b == k (operand: u16 constant index for k).
    ///
    /// This is an optimization for patterns like `x % 3 == 0` which are common
    /// in Python code. Pops b then a, computes `a % b`, then compares with k.
    CompareModEq,

    // === Unary Operations (no operand) ===
    /// Logical not: not a.
    UnaryNot,
    /// Negation: -a.
    UnaryNeg,
    /// Positive: +a.
    UnaryPos,
    /// Bitwise invert: ~a.
    UnaryInvert,

    // === In-place Operations (no operand) ===
    /// In-place add: a += b.
    InplaceAdd,
    /// In-place subtract: a -= b.
    InplaceSub,
    /// In-place multiply: a *= b.
    InplaceMul,
    /// In-place divide: a /= b.
    InplaceDiv,
    /// In-place floor divide: a //= b.
    InplaceFloorDiv,
    /// In-place modulo: a %= b.
    InplaceMod,
    /// In-place power: a **= b.
    InplacePow,
    /// In-place bitwise AND: a &= b.
    InplaceAnd,
    /// In-place bitwise OR: a |= b.
    InplaceOr,
    /// In-place bitwise XOR: a ^= b.
    InplaceXor,
    /// In-place left shift: a <<= b.
    InplaceLShift,
    /// In-place right shift: a >>= b.
    InplaceRShift,

    // === Collection Building ===
    /// Pop n items, build list. Operand: u16 count.
    BuildList,
    /// Pop n items, build tuple. Operand: u16 count.
    BuildTuple,
    /// Pop 2n items (k/v pairs), build dict. Operand: u16 count.
    BuildDict,
    /// Pop n items, build set. Operand: u16 count.
    BuildSet,
    /// Format a value for f-string interpolation. Operand: u8 flags.
    ///
    /// Flags encoding:
    /// - bits 0-1: conversion (0=none, 1=str, 2=repr, 3=ascii)
    /// - bit 2: has format spec on stack (pop fmt_spec first, then value)
    /// - bit 3: has static format spec (operand includes u16 const_id after flags)
    ///
    /// Pops the value (and optionally format spec), pushes the formatted string.
    FormatValue,
    /// Pop n parts, concatenate for f-string. Operand: u16 count.
    BuildFString,
    /// Pop iterable, pop list, extend list with iterable items.
    ///
    /// Used for `*args` unpacking: builds a list of positional args,
    /// then extends it with unpacked iterables.
    ListExtend,
    /// Pop TOS (list), push tuple containing the same elements.
    ///
    /// Used after building the args list to create the final args tuple
    /// for `CallFunctionEx`.
    ListToTuple,
    /// Pop mapping, pop dict, update dict with mapping. Operand: u16 func_name_id.
    ///
    /// Used for `**kwargs` unpacking. The func_name_id is used for error messages
    /// when the mapping contains non-string keys.
    DictMerge,

    // === Subscript & Attribute ===
    /// a[b]: pop index, pop obj, push result.
    BinarySubscr,
    /// a[b] = c: pop value, pop index, pop obj.
    StoreSubscr,
    /// del a[b]: pop index, pop obj.
    DeleteSubscr,
    /// Pop obj, push obj.attr. Operand: u16 name_id.
    LoadAttr,
    /// Pop value, pop obj, set obj.attr. Operand: u16 name_id.
    StoreAttr,
    /// Pop obj, delete obj.attr. Operand: u16 name_id.
    DeleteAttr,

    // === Function Calls ===
    /// Call TOS with n positional args. Operand: u8 arg_count.
    CallFunction,
    /// Call a builtin function directly. Operands: u8 builtin_id, u8 arg_count.
    ///
    /// The builtin_id is the discriminant of `BuiltinsFunctions` (via `FromRepr`).
    /// This is an optimization over `LoadConst + CallFunction` that avoids:
    /// - Constant pool lookup
    /// - Pushing/popping the callable on the stack
    /// - Runtime type dispatch in call_function
    CallBuiltinFunction,
    /// Call a builtin type constructor directly. Operands: u8 type_id, u8 arg_count.
    ///
    /// The type_id is the discriminant of `BuiltinsTypes` (via `FromRepr`).
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    CallBuiltinType,
    /// Call with positional and keyword args.
    ///
    /// Operands: u8 pos_count, u8 kw_count, then kw_count u16 name indices.
    ///
    /// Stack: [callable, pos_args..., kw_values...]
    /// After the two count bytes, there are kw_count little-endian u16 values,
    /// each being a StringId index for the corresponding keyword argument name.
    CallFunctionKw,
    /// Call method. Operands: u16 name_id, u8 arg_count.
    CallMethod,
    /// Call method with keyword args. Operands: u16 name_id, u8 pos_count, u8 kw_count, then kw_count u16 name indices.
    ///
    /// Stack: [obj, pos_args..., kw_values...]
    /// After the operands, there are kw_count little-endian u16 values,
    /// each being a StringId index for the corresponding keyword argument name.
    CallMethodKw,
    /// Call a defined function with *args tuple and **kwargs dict. Operand: u8 flags.
    ///
    /// Flags:
    /// - bit 0: has kwargs dict on stack
    ///
    /// Stack layout (bottom to top):
    /// - callable
    /// - args tuple
    /// - kwargs dict (if flag bit 0 set)
    ///
    /// Used for calls with `*args` and/or `**kwargs` unpacking.
    CallFunctionExtended,

    // === Control Flow ===
    /// Unconditional relative jump. Operand: i16 offset.
    Jump,
    /// Jump if TOS truthy, always pop. Operand: i16 offset.
    JumpIfTrue,
    /// Jump if TOS falsy, always pop. Operand: i16 offset.
    JumpIfFalse,
    /// Jump if TOS truthy (keep), else pop. Operand: i16 offset.
    JumpIfTrueOrPop,
    /// Jump if TOS falsy (keep), else pop. Operand: i16 offset.
    JumpIfFalseOrPop,

    // === Iteration ===
    /// Convert TOS to iterator.
    GetIter,
    /// Advance iterator or jump to end. Operand: i16 offset.
    ForIter,

    // === Function Definition ===
    /// Create function object. Operand: u16 func_id.
    MakeFunction,
    /// Create closure. Operands: u16 func_id, u8 cell_count.
    MakeClosure,

    // === Exception Handling ===
    // Note: No SetupTry/PopExceptHandler - we use static exception_table
    /// Raise TOS as exception.
    Raise,
    /// Raise TOS from TOS-1.
    RaiseFrom,
    /// Re-raise current exception (bare `raise`).
    Reraise,
    /// Clear current_exception when exiting except block.
    ClearException,
    /// Check if exception matches type for except clause.
    ///
    /// Stack: [..., exception, exc_type] -> [..., exception, bool]
    /// Validates that exc_type is a valid exception type (ExcType or tuple of ExcTypes).
    /// If invalid, raises TypeError. If valid, pushes True if exception matches, else False.
    CheckExcMatch,

    // === Return ===
    /// Return TOS from function.
    ReturnValue,

    // === Unpacking ===
    /// Unpack TOS into n values. Operand: u8 count.
    UnpackSequence,
    /// Unpack with *rest. Operands: u8 before, u8 after.
    UnpackEx,

    // === Special ===
    /// No operation (for patching/alignment).
    Nop,
}

impl TryFrom<u8> for Opcode {
    type Error = InvalidOpcodeError;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        Self::from_repr(byte).ok_or(InvalidOpcodeError(byte))
    }
}

/// Error returned when attempting to convert an invalid byte to an Opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidOpcodeError(pub u8);

impl std::fmt::Display for InvalidOpcodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid opcode byte: {}", self.0)
    }
}

impl std::error::Error for InvalidOpcodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opcode_roundtrip() {
        // Verify that all opcodes from 0 to Nop can be converted to u8 and back
        for byte in 0..=Opcode::Nop as u8 {
            let opcode = Opcode::try_from(byte).unwrap();
            assert_eq!(opcode as u8, byte, "opcode {opcode:?} has wrong discriminant");
        }
    }

    #[test]
    fn test_invalid_opcode() {
        // Byte just after the last valid opcode should fail
        let result = Opcode::try_from(Opcode::Nop as u8 + 1);
        assert!(result.is_err());
        // 255 should also fail
        let result = Opcode::try_from(255u8);
        assert!(result.is_err());
    }

    #[test]
    fn test_opcode_size() {
        // Verify opcode is 1 byte
        assert_eq!(std::mem::size_of::<Opcode>(), 1);
    }
}
