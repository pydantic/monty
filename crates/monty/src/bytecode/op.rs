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

use std::{error, fmt};

use strum::FromRepr;

use crate::bytecode::builder::RelativeOffset;

/// `FormatValue` flag: a format spec was pushed onto the stack ahead of the
/// value. When set, the VM pops the spec before the value. See `Opcode::FormatValue`.
pub const FORMAT_VALUE_HAS_SPEC: u8 = 0x04;

/// `FormatValue` flag: the on-stack format spec is the pre-encoded `Int` form
/// produced by [`crate::fstring::encode_format_spec`], not a string to be
/// parsed at runtime. Only meaningful when [`FORMAT_VALUE_HAS_SPEC`] is also
/// set. The compiler pairs this bit with a `LoadConst` of `Value::Int(encoded)`
/// emitted before the value.
pub const FORMAT_VALUE_STATIC_SPEC: u8 = 0x08;

/// Opcode discriminant - just identifies the instruction type.
///
/// Operands (if any) follow in the bytecode stream and are fetched separately.
/// With `#[repr(u8)]`, each opcode is exactly 1 byte. Uses `strum::FromRepr` for
/// efficient byte-to-opcode conversion (bounds check + transmute).
///
/// Opcode bytes are part of Monty's serialized `Code` format, so existing values
/// must remain stable across releases. Append new opcodes to the end of the enum
/// instead of inserting them into the middle.
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
    /// Load local in call context: pushes `ExtFunction(name_id)` for undefined names
    /// instead of yielding `NameLookup`. Operands: u8 slot, u16 name_id.
    ///
    /// Used when compiling function calls like `foo()` where `foo` is `LocalUnassigned`.
    /// If the variable is defined, behaves identically to `LoadLocal`.
    /// If undefined, pushes an `ExtFunction` value so execution continues to `CallFunction`,
    /// which naturally yields `FunctionCall` instead of `NameLookup`.
    /// The name_id is encoded in the operand to avoid namespace lookup ambiguity.
    LoadLocalCallable,
    /// Wide variant of `LoadLocalCallable`. Operands: u16 slot, u16 name_id.
    LoadLocalCallableW,
    /// Load global in call context: pushes `ExtFunction(name_id)` for undefined names
    /// instead of yielding `NameLookup`. Operands: u16 slot, u16 name_id.
    ///
    /// Used when compiling function calls like `foo()` where `foo` is a global.
    /// If the variable is defined, behaves identically to `LoadGlobal`.
    /// If undefined, pushes an `ExtFunction` value so execution continues to `CallFunction`,
    /// which naturally yields `FunctionCall` instead of `NameLookup`.
    /// The name_id is encoded in the operand because global and local slot indices
    /// belong to different namespaces — using the current frame's local_names would
    /// return the wrong name when called from inside a function.
    LoadGlobalCallable,

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
    /// Flags encoding (see [`FORMAT_VALUE_HAS_SPEC`]/[`FORMAT_VALUE_STATIC_SPEC`]):
    /// - bits 0-1: conversion (0=none, 1=str, 2=repr, 3=ascii)
    /// - bit 2 ([`FORMAT_VALUE_HAS_SPEC`]): a format spec was pushed before
    ///   the value
    /// - bit 3 ([`FORMAT_VALUE_STATIC_SPEC`]): the on-stack spec is the
    ///   pre-encoded `Int` form rather than a string. Only meaningful when
    ///   bit 2 is set
    ///
    /// Pops the value (and optionally format spec), pushes the formatted string.
    FormatValue,
    /// Pop n parts, concatenate for f-string. Operand: u16 count.
    BuildFString,
    /// Build a slice object from stack values. No operand.
    ///
    /// Pops 3 values from stack: step, stop, start (TOS order).
    /// Each value can be None (for default) or an integer.
    /// Creates a `HeapData::Slice` and pushes a `Value::Ref` to it.
    BuildSlice,
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

    // === Comprehension Building ===
    /// Append TOS to list for comprehension. Operand: u8 depth (number of iterators).
    ///
    /// Stack: [..., list, iter1, ..., iterN, value] -> [..., list, iter1, ..., iterN]
    /// Pops value (TOS), appends to list at stack position (len - 2 - depth).
    /// Depth equals the number of nested iterators (generators) in the comprehension.
    ListAppend,
    /// Add TOS to set for comprehension. Operand: u8 depth (number of iterators).
    ///
    /// Stack: [..., set, iter1, ..., iterN, value] -> [..., set, iter1, ..., iterN]
    /// Pops value (TOS), adds to set at stack position (len - 2 - depth).
    /// May raise TypeError if value is unhashable.
    SetAdd,
    /// Set dict[key] = value for comprehension. Operand: u8 depth (number of iterators).
    ///
    /// Stack: [..., dict, iter1, ..., iterN, key, value] -> [..., dict, iter1, ..., iterN]
    /// Pops value (TOS) and key (TOS-1), sets dict[key] = value.
    /// Dict is at stack position (len - 3 - depth).
    /// May raise TypeError if key is unhashable.
    DictSetItem,

    // === Subscript & Attribute ===
    /// a[b]: pop index, pop obj, push result.
    BinarySubscr,
    /// a[b] = c: pop value, pop index, pop obj.
    StoreSubscr,
    // NOTE: DeleteSubscr removed - `del` statement not supported by parser
    /// Pop obj, push obj.attr. Operand: u16 name_id.
    LoadAttr,
    /// Pop module, push module.attr for `from ... import`. Operand: u16 name_id.
    ///
    /// Like `LoadAttr` but raises `ImportError` instead of `AttributeError`
    /// when the attribute is not found. Used for `from module import name`.
    LoadAttrImport,
    /// Pop value, pop obj, set obj.attr. Operand: u16 name_id.
    StoreAttr,
    // NOTE: DeleteAttr removed - `del` statement not supported by parser

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
    /// Call attribute on object. Operands: u16 name_id, u8 arg_count.
    ///
    /// This is used for both method calls (`obj.method(args)`) and module
    /// attribute calls (`module.func(args)`). The attribute is looked up
    /// on the object and called with the given arguments.
    CallAttr,
    /// Call attribute with keyword args. Operands: u16 name_id, u8 pos_count, u8 kw_count, then kw_count u16 name indices.
    ///
    /// Stack: [obj, pos_args..., kw_values...]
    /// After the operands, there are kw_count little-endian u16 values,
    /// each being a StringId index for the corresponding keyword argument name.
    CallAttrKw,
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
    /// Call attribute with *args tuple and **kwargs dict. Operands: u16 name_id, u8 flags.
    ///
    /// Flags:
    /// - bit 0: has kwargs dict on stack
    ///
    /// Stack layout (bottom to top):
    /// - receiver object
    /// - args tuple
    /// - kwargs dict (if flag bit 0 set)
    ///
    /// Used for method calls with `*args` and/or `**kwargs` unpacking.
    CallAttrExtended,

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
    // NOTE: RaiseFrom removed - `raise ... from ...` not supported by parser
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

    // === Async/Await ===
    /// Await the TOS value.
    ///
    /// Handles `ExternalFuture`, `Coroutine`, and `GatherFuture` awaitables.
    /// For `ExternalFuture`: if resolved, pushes result; if pending, blocks task.
    /// For `Coroutine`: validates state is `New`, then starts execution.
    /// For `GatherFuture`: spawns all coroutines as tasks and blocks until completion.
    ///
    /// Raises `TypeError` if TOS is not awaitable.
    /// Raises `RuntimeError` if coroutine/future has already been awaited.
    Await,

    // === Unpacking ===
    /// Unpack TOS into n values. Operand: u8 count.
    UnpackSequence,
    /// Unpack with *rest. Operands: u8 before, u8 after.
    UnpackEx,

    // === Special ===
    /// No operation (for patching/alignment).
    Nop,

    // === Module Operations ===
    /// Load a built-in module onto the stack. Operand: u8 module_id.
    ///
    /// The module_id maps to `BuiltinModule` (0=sys, 1=typing).
    /// Creates the module on the heap and pushes a `Value::Ref` to it.
    LoadModule,
    /// Raises `ModuleNotFoundError` at runtime. Operand: u16 constant index for module name.
    ///
    /// This opcode is emitted when the compiler encounters an import of an unknown module.
    /// Instead of failing at compile time, the error is deferred to runtime so that
    /// imports inside `if TYPE_CHECKING:` blocks or other non-executed code paths
    /// don't cause errors.
    ///
    /// The operand is an index into the constant pool where the module name string is stored.
    RaiseImportError,
    /// Duplicate the top two stack values, preserving order: `[a, b] -> [a, b, a, b]`.
    ///
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    Dup2,
    /// Delete global variable (set to Undefined). Operand: u16 slot.
    ///
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    DeleteGlobal,

    /// Pop a mapping, silently merge into the dict at `depth`. Operand: u8 depth.
    ///
    /// Used for `**expr` unpack inside dict literals, where later keys overwrite earlier ones
    /// (unlike `DictMerge` which raises `TypeError` on duplicate keys).
    ///
    /// Stack: [..., dict, iter1, ..., iterN, mapping] -> [..., dict, iter1, ..., iterN]
    /// Pops mapping (TOS), merges into dict at stack position `len - 2 - depth`.
    /// Raises `TypeError` if `mapping` is not a dict.
    DictUpdate,
    /// Pop an iterable, add all items to set at `depth`. Operand: u8 depth.
    ///
    /// Used for `*expr` unpack inside set literals (e.g., `{*a, 1}`).
    /// Follows the same depth convention as `ListAppend`/`SetAdd`.
    ///
    /// Stack: [..., set, iter1, ..., iterN, iterable] -> [..., set, iter1, ..., iterN]
    /// Pops iterable (TOS), adds each item to set at stack position `len - 2 - depth`.
    /// Raises `TypeError` if iterable is not iterable.
    SetExtend,

    // === Context Managers ===
    /// Enter a context manager: call `__enter__` on TOS, push the result, keep
    /// the context manager underneath for the eventual `__exit__` call.
    ///
    /// Stack: [..., ctx] -> [..., ctx, value]
    ///
    /// Emitted by the compiler at the head of a `with` block. The context
    /// manager stays on the stack across the body so the matching `WithExit`
    /// or `WithExceptStart` can find it. Calls `py_enter` on the heap object,
    /// raising `AttributeError` for objects that don't implement the protocol.
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    BeforeWith,
    /// Normal exit from a `with` block: call `__exit__(None, None, None)` on TOS,
    /// pop the context manager, and push the result of the call.
    ///
    /// Stack: [..., ctx] -> [..., result]
    ///
    /// The compiler emits a trailing `Pop` to discard the result — splitting the
    /// "call + discard" into two opcodes keeps the call shape compatible with
    /// `__exit__` implementations that yield `OsCall`/`External`/`MethodCall`
    /// (the host's resume push lands as the `result`, which `Pop` then drops).
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    WithExit,
    /// Exception path of a `with` block: call `__exit__(type(exc), exc, None)`
    /// and push the truthiness of the result so the compiler can branch on
    /// whether to suppress the exception.
    ///
    /// Stack: [..., ctx, exc] -> [..., ctx, exc, suppress]
    ///
    /// The exception is the value pushed onto the operand stack on entry to
    /// the with-block's exception handler region (analogous to `Try`). The
    /// compiler-emitted control flow following this opcode pops both `ctx` and
    /// `exc` and either swallows (via `ClearException`) or `Reraise`s based on
    /// the `suppress` bool.
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    WithExceptStart,

    // === Comprehension Helpers ===
    /// Move the value at `TOS - n` to TOS, shifting items above it down by one.
    /// Operand: u8 n.
    ///
    /// Used by the comprehension compiler to bring a nested-tuple sub-target
    /// up to TOS so it can be `UnpackSequence`-d (UnpackSequence only operates
    /// on TOS). `n = 0` is a no-op.
    ///
    /// At runtime, implemented as a single `Vec::rotate_left(1)` over the
    /// affected slice, so cost is O(n) shifts but n is bounded by the
    /// comprehension's nesting depth (almost always tiny).
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    LiftToTop,
    /// Raise `UnboundLocalError: cannot access local variable 'NAME' where
    /// it is not associated with a value`. Operand: u16 name_id.
    ///
    /// Emitted by the comprehension compiler at sites where static analysis
    /// proves a comp-target read happens before the corresponding `for`
    /// assigns it — e.g. `[x for x in [1] for _ in [late] for late in [[2]]]`
    /// where `late` is read in an earlier generator's iter expression.
    /// The opcode carries the target's name inline so sibling comprehensions
    /// that reuse comp-var slots still report the right variable name in the
    /// error.
    /// Appended at the end to preserve the serialized byte values of all older opcodes.
    RaiseUnboundLocal,
}

impl TryFrom<u8> for Opcode {
    type Error = InvalidOpcodeError;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        Self::from_repr(byte).ok_or(InvalidOpcodeError(byte))
    }
}

/// Operand bundle, to be paired with an `Opcode` at emit time.
///
/// `Opcode::stack_effect` consumes this to compute the operand-stack delta in
/// a single exhaustive match. The variants describe the *byte shape* of the
/// in-stream operand — `emit_with_operand` writes the bytes for each variant
/// and the same enum drives stack-effect computation, so byte emission and
/// stack tracking can't drift apart.
///
/// `Operand` is `Copy` (largest variant is ~24 bytes), so it's passed by value
/// throughout.
#[derive(Debug, Clone, Copy)]
pub enum Operand<'a> {
    /// No operand bytes (e.g. `Pop`, `BinaryAdd`).
    None,
    /// Single u8 operand (e.g. `LoadLocal`, `CallFunction`).
    U8(u8),
    /// Single i8 operand.
    I8(i8),
    /// Single u16 operand, little-endian (e.g. `LoadConst`, `BuildList`).
    U16(u16),
    /// Absolute jump target. `emit_with_operand` computes the signed i16
    /// relative offset (`target - (jump_start + 3)`) and writes it to bytecode
    /// as a little-endian i16. Required for jump opcodes: `Jump`, `JumpIfTrue`,
    /// `JumpIfFalse`, `JumpIfTrueOrPop`, `JumpIfFalseOrPop`, `ForIter`.
    ///
    /// Forward jumps pass `current_offset()` as a self-referential placeholder
    /// (yielding a -3 relative offset); `patch_jump` overwrites it once the
    /// real target is known. The placeholder is harmless because `#[must_use]`
    /// on `JumpLabel` catches the "forgot to patch" case at compile time.
    Offset(RelativeOffset),
    /// Two u8 operands (e.g. `UnpackEx`, `CallBuiltinFunction`).
    U8U8(u8, u8),
    /// u8 then u16 little-endian (e.g. `LoadLocalCallable`).
    U8U16(u8, u16),
    /// u16 little-endian then u8 (e.g. `MakeFunction`, `CallAttr`).
    U16U8(u16, u8),
    /// Two u16 little-endian (e.g. `LoadLocalCallableW`, `LoadGlobalCallable`).
    U16U16(u16, u16),
    /// u16 then two u8s (e.g. `MakeClosure`).
    U16U8U8(u16, u8, u8),
    /// `CallFunctionKw` shape: pos_count (u8), kw_count (u8), kw_count * name_id (u16 each).
    CallKw { pos_count: u8, kwname_ids: &'a [u16] },
    /// `CallAttrKw` shape: attr_name_id (u16), pos_count (u8), kw_count (u8), kw_count * name_id (u16 each).
    CallAttrKw {
        attr_name_id: u16,
        pos_count: u8,
        kwname_ids: &'a [u16],
    },
}

impl Opcode {
    /// Returns the operand-stack effect of this opcode paired with `operand`
    /// (positive = push, negative = pop).
    ///
    /// Variable-effect opcodes have explicit `(opcode, operand-variant)` arms;
    /// fixed-effect opcodes match on the opcode alone and ignore the operand
    /// variant. A variable-effect opcode whose operand variant doesn't match
    /// any enumerated arm hits the catch-all panic — this keeps the tracker
    /// honest when a new variable-effect opcode is added without a matching
    /// arm.
    ///
    /// `MakeFunction`/`MakeClosure` have explicit variable arms even though
    /// the "push the function" effect is +1 — the actual effect is
    /// `1 - defaults_count` because defaults are popped from the stack, which
    /// only equals +1 when no defaults are present.
    ///
    /// `emit_jump_to`'s backward-jump path computes its own effect inline
    /// because it doesn't have an `Operand` to pass (the operand is a raw
    /// i16 offset, not a stack-effect-bearing shape).
    #[must_use]
    pub fn stack_effect(self, operand: Operand<'_>) -> i16 {
        #![expect(clippy::allow_attributes, reason = "expect seems broken with enum_glob_use")]
        #[allow(clippy::enum_glob_use, reason = "simplifies churn")]
        use Opcode::*; // allow local import
        match (self, operand) {
            // === Variable-effect: U8 operand ===
            (CallFunction, Operand::U8(arg_count)) => -i16::from(arg_count),
            (CallFunctionExtended, Operand::U8(flags)) => -(1 + i16::from(flags & 0x01)),
            (FormatValue, Operand::U8(flags)) => {
                // Spec is on the stack iff `FORMAT_VALUE_HAS_SPEC` is set —
                // the static/dynamic discriminator (`FORMAT_VALUE_STATIC_SPEC`)
                // doesn't change the pop count.
                if flags & FORMAT_VALUE_HAS_SPEC != 0 { -1 } else { 0 }
            }
            (UnpackSequence, Operand::U8(n)) => i16::from(n) - 1,

            // === Variable-effect: U16 operand ===
            (BuildList | BuildTuple | BuildSet | BuildFString, Operand::U16(n)) => 1 - n.cast_signed(),
            (BuildDict, Operand::U16(n)) => 1 - 2 * n.cast_signed(),

            // === Variable-effect: U8U8 operand ===
            // UnpackEx: pops 1, pushes (before + 1 + after) → before + after.
            (UnpackEx, Operand::U8U8(before, after)) => i16::from(before) + i16::from(after),
            // Builtin calls: no callable on stack, pops args, pushes result → 1 - arg_count.
            (CallBuiltinFunction | CallBuiltinType, Operand::U8U8(_, arg_count)) => 1 - i16::from(arg_count),

            // === Variable-effect: U16U8 operand ===
            (MakeFunction, Operand::U16U8(_, defaults)) => 1 - i16::from(defaults),
            (CallAttr, Operand::U16U8(_, arg_count)) => -i16::from(arg_count),
            (CallAttrExtended, Operand::U16U8(_, flags)) => -(1 + i16::from(flags & 0x01)),

            // === Variable-effect: U16U8U8 operand ===
            // MakeClosure: pops `cell_count` cells AND `defaults_count` defaults,
            // pushes the closure → 1 - defaults - cells.
            (MakeClosure, Operand::U16U8U8(_, defaults, cells)) => 1 - i16::from(defaults) - i16::from(cells),

            // === Variable-effect: variable-length kw operands ===
            // pops callable + pos_args + kw_args, pushes result → -(pos_count + kw_count).
            (CallFunctionKw, Operand::CallKw { pos_count, kwname_ids }) => {
                let kw_count = i16::try_from(kwname_ids.len()).expect("keyword count exceeds i16");
                -(i16::from(pos_count) + kw_count)
            }
            (
                CallAttrKw,
                Operand::CallAttrKw {
                    pos_count, kwname_ids, ..
                },
            ) => {
                let kw_count = i16::try_from(kwname_ids.len()).expect("keyword count exceeds i16");
                -(i16::from(pos_count) + kw_count)
            }

            // === Fixed-effect, no operand ===
            (Pop, Operand::None) => -1,
            (Dup, Operand::None) => 1,
            (Dup2, Operand::None) => 2,
            (Rot2 | Rot3, Operand::None) => 0,
            (LoadNone | LoadTrue | LoadFalse, Operand::None) => 1,
            (LoadLocal0 | LoadLocal1 | LoadLocal2 | LoadLocal3, Operand::None) => 1,
            (
                BinaryAdd | BinarySub | BinaryMul | BinaryDiv | BinaryFloorDiv | BinaryMod | BinaryPow | BinaryAnd
                | BinaryOr | BinaryXor | BinaryLShift | BinaryRShift | BinaryMatMul,
                Operand::None,
            ) => -1,
            (
                CompareEq | CompareNe | CompareLt | CompareLe | CompareGt | CompareGe | CompareIs | CompareIsNot
                | CompareIn | CompareNotIn,
                Operand::None,
            ) => -1,
            (UnaryNot | UnaryNeg | UnaryPos | UnaryInvert, Operand::None) => 0,
            (
                InplaceAdd | InplaceSub | InplaceMul | InplaceDiv | InplaceFloorDiv | InplaceMod | InplacePow
                | InplaceAnd | InplaceOr | InplaceXor | InplaceLShift | InplaceRShift,
                Operand::None,
            ) => -1,
            (BuildSlice, Operand::None) => -2,
            (ListExtend, Operand::None) => -1,
            (ListToTuple, Operand::None) => 0,
            (BinarySubscr, Operand::None) => -1,
            (StoreSubscr, Operand::None) => -3,
            (GetIter | Await, Operand::None) => 0,
            (Raise, Operand::None) => -1,
            (Reraise | ClearException | CheckExcMatch, Operand::None) => 0,
            (ReturnValue, Operand::None) => -1,
            (Nop, Operand::None) => 0,

            // === Fixed-effect, I8 operand ===
            (LoadSmallInt, Operand::I8(_)) => 1,

            // === Fixed-effect, U8 operand ===
            (LoadLocal | LoadModule, Operand::U8(_)) => 1,
            (StoreLocal, Operand::U8(_)) => -1,
            (DeleteLocal, Operand::U8(_)) => 0,
            // `ListAppend`/`SetAdd`/`DictSetItem` carry a u8 stack-depth operand
            // that names which collection below TOS to extend; the stack
            // effect itself is fixed.
            (ListAppend | SetAdd, Operand::U8(_)) => -1,
            (DictSetItem, Operand::U8(_)) => -2,
            // `DictUpdate`/`SetExtend` also take a u8 stack-depth operand.
            (DictUpdate | SetExtend, Operand::U8(_)) => -1,
            // `LiftToTop(n)` reorders the stack — net effect 0.
            (LiftToTop, Operand::U8(_)) => 0,

            // === Fixed-effect, no operand (context managers) ===
            // `BeforeWith` pushes the `__enter__` result on top of the existing ctx.
            (BeforeWith, Operand::None) => 1,
            // `WithExit` pops ctx and pushes the `__exit__` return value; compiler
            // emits a trailing `Pop` to discard.
            (WithExit, Operand::None) => 0,
            // `WithExceptStart` pushes the raw `__exit__` return value above the
            // existing [ctx, exc]; compiler uses `JumpIfTrue` to act on its truthiness.
            (WithExceptStart, Operand::None) => 1,

            // === Fixed-effect, U16 operand ===
            (LoadConst, Operand::U16(_)) => 1,
            (LoadLocalW | LoadGlobal | LoadCell, Operand::U16(_)) => 1,
            (StoreLocalW | StoreGlobal | StoreCell, Operand::U16(_)) => -1,
            (DeleteGlobal, Operand::U16(_)) => 0,
            (CompareModEq, Operand::U16(_)) => -1,
            (LoadAttr | LoadAttrImport, Operand::U16(_)) => 0,
            (StoreAttr, Operand::U16(_)) => -2,
            // `DictMerge` takes a u16 operand carrying the func_name_id for
            // the duplicate-key TypeError message.
            (DictMerge, Operand::U16(_)) => -1,
            // `RaiseImportError` takes a u16 const_id naming the missing module.
            (RaiseImportError, Operand::U16(_)) => 0,
            // `RaiseUnboundLocal(name_id)` always raises — fall-through is dead
            // code, but the tracker absorbs the bytes with effect 0 before the
            // following region starts.
            (RaiseUnboundLocal, Operand::U16(_)) => 0,

            // === Fixed-effect, U8U16 operand ===
            (LoadLocalCallable, Operand::U8U16(..)) => 1,

            // === Fixed-effect, U16U16 operand ===
            (LoadLocalCallableW | LoadGlobalCallable, Operand::U16U16(..)) => 1,

            // === Jumps: fall-through effect (what the tracker absorbs after the bytes are written).
            // Use `Offset` arguments to sanity check that jumps are correctly paired with offsets. ===

            // `Jump` is unconditional and makes the code dead; the 0 here is correct for
            // the moment before that transition.
            (Jump, Operand::Offset(_)) => 0,
            // Conditional jumps pop the condition on either path, so the tracker absorbs the pop immediately.
            (JumpIfTrue | JumpIfFalse | JumpIfTrueOrPop | JumpIfFalseOrPop, Operand::Offset(_)) => -1,
            // `ForIter` adds the the value yielded by the iterator to the stack.
            (ForIter, Operand::Offset(_)) => 1,

            // Catch-all: opcode emitted with the wrong operand variant, or a
            // new opcode added without an arm above. Every opcode has exactly
            // one valid operand shape; pairing them up here means the wrong
            // emit_* helper for a given opcode is caught at stack-effect time
            // rather than producing nonsense bytecode.
            (op, _) => panic!(
                "Opcode::stack_effect: opcode {op:?} paired with wrong operand variant {operand:?} (or missing arm)"
            ),
        }
    }

    /// Returns the operand-stack delta applied when *this jump opcode is
    /// taken*, i.e. the difference between the pre-emit depth and the depth
    /// that execution arrives at on the jump-taken path.
    ///
    /// Panics for non-jump opcodes.
    #[must_use]
    pub fn jump_taken_stack_effect(self) -> i16 {
        match self {
            // Unconditional jump: stack unchanged on jump-taken.
            Self::Jump => 0,
            // Pop condition on either path.
            Self::JumpIfTrue | Self::JumpIfFalse => -1,
            // Pop condition on fall-through, keep it on jump-taken.
            Self::JumpIfTrueOrPop | Self::JumpIfFalseOrPop => 0,
            // Pop iterator on jump-taken (no value pushed).
            Self::ForIter => -1,
            _ => panic!("Opcode::jump_taken_delta: {self:?} is not a jump opcode"),
        }
    }
}

/// Error returned when attempting to convert an invalid byte to an Opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidOpcodeError(pub u8);

impl fmt::Display for InvalidOpcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid opcode byte: {}", self.0)
    }
}

impl error::Error for InvalidOpcodeError {}

#[cfg(test)]
mod tests {
    use std::mem;

    use super::*;

    #[test]
    fn test_opcode_roundtrip() {
        // Verify that all opcodes from 0 to the last opcode can be converted to u8 and back.
        for byte in 0..=Opcode::RaiseUnboundLocal as u8 {
            let opcode = Opcode::try_from(byte).unwrap();
            assert_eq!(opcode as u8, byte, "opcode {opcode:?} has wrong discriminant");
        }
    }

    #[test]
    fn test_serialized_opcode_values_remain_stable() {
        // `RaiseImportError` was the tail opcode before `Dup2` was introduced. Keeping it at
        // byte 110 preserves compatibility for serialized runners and snapshots compiled by
        // older versions.
        assert_eq!(Opcode::RaiseImportError as u8, 110);
        assert_eq!(Opcode::Dup2 as u8, 111);
        assert_eq!(Opcode::DeleteGlobal as u8, 112);
        assert_eq!(Opcode::DictUpdate as u8, 113);
        assert_eq!(Opcode::SetExtend as u8, 114);
        // Context-manager opcodes added with the `with` statement support.
        assert_eq!(Opcode::BeforeWith as u8, 115);
        assert_eq!(Opcode::WithExit as u8, 116);
        assert_eq!(Opcode::WithExceptStart as u8, 117);
        // Comprehension-support opcodes appended after the context-manager opcodes.
        assert_eq!(Opcode::LiftToTop as u8, 118);
        assert_eq!(Opcode::RaiseUnboundLocal as u8, 119);
    }

    #[test]
    fn test_invalid_opcode() {
        // Byte just after the last valid opcode should fail
        let result = Opcode::try_from(Opcode::RaiseUnboundLocal as u8 + 1);
        assert!(result.is_err());
        // 255 should also fail
        let result = Opcode::try_from(255u8);
        assert!(result.is_err());
    }

    #[test]
    fn test_opcode_size() {
        // Verify opcode is 1 byte
        assert_eq!(mem::size_of::<Opcode>(), 1);
    }
}
