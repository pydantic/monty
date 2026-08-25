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

#[cfg(test)]
use strum::IntoEnumIterator;
use strum::{EnumIter, FromRepr};

use crate::{bytecode::builder::RelativeOffset, expressions::CmpOperator};

/// `FormatValue` flag: a format spec was pushed onto the stack ahead of the
/// value. When set, the VM pops the spec before the value. See `Opcode::FormatValue`.
pub const FORMAT_VALUE_HAS_SPEC: u8 = 0x04;

/// `FormatValue` flag: the on-stack format spec is the pre-encoded `Int` form
/// produced by [`crate::fstring::encode_format_spec`], not a string to be
/// parsed at runtime. Only meaningful when [`FORMAT_VALUE_HAS_SPEC`] is also
/// set. The compiler pairs this bit with a `LoadConst` of `Value::Int(encoded)`
/// emitted before the value.
pub const FORMAT_VALUE_STATIC_SPEC: u8 = 0x08;

/// `Assert`/`AssertFailed` flag: the assert is a fused comparison. When set,
/// the low nibble of the flags operand holds [`CmpOperator::as_operand`] and
/// the opcode pops two comparison operands instead of one test value.
pub const ASSERT_CMP_FLAG: u8 = 0x10;

/// Encodes an optional fused comparison into the `Assert`/`AssertFailed`
/// flags operand: `ASSERT_CMP_FLAG | as_operand` when present, `0` when not.
pub fn assert_flags(cmp_op: Option<CmpOperator>) -> u8 {
    cmp_op.map_or(0, |op| ASSERT_CMP_FLAG | op.as_operand())
}

/// Decodes [`assert_flags`]; `None` for bytes it can never produce (corrupt
/// or hand-built bytecode — callers treat that as an internal error).
#[expect(
    clippy::option_option,
    reason = "outer = valid encoding, inner = fused comparison present"
)]
pub fn decode_assert_flags(flags: u8) -> Option<Option<CmpOperator>> {
    if flags == 0 {
        Some(None)
    } else if flags & ASSERT_CMP_FLAG != 0 {
        // `!ASSERT_CMP_FLAG` keeps any stray high bits so they fail decoding.
        CmpOperator::from_repr(flags & !ASSERT_CMP_FLAG).map(Some)
    } else {
        None
    }
}

/// Opcode discriminant - just identifies the instruction type.
///
/// Operands (if any) follow in the bytecode stream and are fetched separately.
/// With `#[repr(u8)]`, each opcode is exactly 1 byte. Uses `strum::FromRepr` for
/// efficient byte-to-opcode conversion (bounds check + transmute).
///
/// Opcode bytes are part of Monty's serialized `Code` format. Explicit
/// discriminants prevent source reordering or removal from changing that format
/// accidentally; intentional renumbering requires a dump-format version bump.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, FromRepr)]
pub enum Opcode {
    // === Stack Operations (no operand) ===
    /// Discard top of stack.
    Pop = 0,
    /// Duplicate top of stack.
    Dup = 1,
    /// Swap top two: [a, b] -> [b, a].
    Rot2 = 2,
    /// Rotate top three: [a, b, c] -> [c, a, b].
    Rot3 = 3,

    // === Constants & Literals ===
    /// Push constant from pool. Operand: u16 const_id.
    LoadConst = 4,
    /// Push None.
    LoadNone = 5,
    /// Push True.
    LoadTrue = 6,
    /// Push False.
    LoadFalse = 7,
    /// Push small integer (-128 to 127). Operand: i8.
    LoadSmallInt = 8,

    // === Variables ===
    // Specialized no-operand versions for common slots (hot path)
    /// Push local slot 0 (often 'self').
    LoadLocal0 = 9,
    /// Push local slot 1.
    LoadLocal1 = 10,
    /// Push local slot 2.
    LoadLocal2 = 11,
    /// Push local slot 3.
    LoadLocal3 = 12,
    // General versions with operand
    /// Push local variable. Operand: u8 slot.
    LoadLocal = 13,
    /// Push local (wide, slot > 255). Operand: u16 slot.
    LoadLocalW = 14,
    /// Pop and store to local. Operand: u8 slot.
    StoreLocal = 15,
    /// Store local (wide). Operand: u16 slot.
    StoreLocalW = 16,
    /// Push from global namespace. Operand: u16 slot.
    LoadGlobal = 17,
    /// Store to global. Operand: u16 slot.
    StoreGlobal = 18,
    /// Load from closure cell. Operand: u16 slot.
    LoadCell = 19,
    /// Store to closure cell. Operand: u16 slot.
    StoreCell = 20,
    /// Delete local variable. Operand: u8 slot.
    DeleteLocal = 21,
    /// Load global in call context: pushes an external function for undefined names
    /// instead of yielding `NameLookup`. Operands: u16 slot, u16 name_id.
    ///
    /// Used when compiling function calls like `foo()` where `foo` is a global.
    /// If the variable is defined, behaves identically to `LoadGlobal`.
    /// If undefined, pushes an `ExtFunction` value so execution continues to `CallFunction`,
    /// which naturally yields `FunctionCall` instead of `NameLookup`.
    /// The name_id is encoded in the operand because global and local slot indices
    /// belong to different namespaces — using the current frame's local_names would
    /// return the wrong name when called from inside a function.
    LoadGlobalCallable = 22,

    // === Binary Operations (no operand) ===
    /// Add: a + b.
    BinaryAdd = 23,
    /// Subtract: a - b.
    BinarySub = 24,
    /// Multiply: a * b.
    BinaryMul = 25,
    /// Divide: a / b.
    BinaryDiv = 26,
    /// Floor divide: a // b.
    BinaryFloorDiv = 27,
    /// Modulo: a % b.
    BinaryMod = 28,
    /// Power: a ** b.
    BinaryPow = 29,
    /// Bitwise AND: a & b.
    BinaryAnd = 30,
    /// Bitwise OR: a | b.
    BinaryOr = 31,
    /// Bitwise XOR: a ^ b.
    BinaryXor = 32,
    /// Left shift: a << b.
    BinaryLShift = 33,
    /// Right shift: a >> b.
    BinaryRShift = 34,
    /// Matrix multiply: a @ b.
    BinaryMatMul = 35,

    // === Comparison Operations (no operand) ===
    /// Equal: a == b.
    CompareEq = 36,
    /// Not equal: a != b.
    CompareNe = 37,
    /// Less than: a < b.
    CompareLt = 38,
    /// Less than or equal: a <= b.
    CompareLe = 39,
    /// Greater than: a > b.
    CompareGt = 40,
    /// Greater than or equal: a >= b.
    CompareGe = 41,
    /// Identity: a is b.
    CompareIs = 42,
    /// Not identity: a is not b.
    CompareIsNot = 43,
    /// Membership: a in b.
    CompareIn = 44,
    /// Not membership: a not in b.
    CompareNotIn = 45,

    // === Unary Operations (no operand) ===
    /// Logical not: not a.
    UnaryNot = 46,
    /// Negation: -a.
    UnaryNeg = 47,
    /// Positive: +a.
    UnaryPos = 48,
    /// Bitwise invert: ~a.
    UnaryInvert = 49,

    // === In-place Operations (no operand) ===
    /// In-place add: a += b.
    InplaceAdd = 50,
    /// In-place subtract: a -= b.
    InplaceSub = 51,
    /// In-place multiply: a *= b.
    InplaceMul = 52,
    /// In-place divide: a /= b.
    InplaceDiv = 53,
    /// In-place floor divide: a //= b.
    InplaceFloorDiv = 54,
    /// In-place modulo: a %= b.
    InplaceMod = 55,
    /// In-place power: a **= b.
    InplacePow = 56,
    /// In-place bitwise AND: a &= b.
    InplaceAnd = 57,
    /// In-place bitwise OR: a |= b.
    InplaceOr = 58,
    /// In-place bitwise XOR: a ^= b.
    InplaceXor = 59,
    /// In-place left shift: a <<= b.
    InplaceLShift = 60,
    /// In-place right shift: a >>= b.
    InplaceRShift = 61,

    // === Collection Building ===
    /// Pop n items, build list. Operand: u16 count.
    BuildList = 62,
    /// Pop n items, build tuple. Operand: u16 count.
    BuildTuple = 63,
    /// Pop 2n items (k/v pairs), build dict. Operand: u16 count.
    BuildDict = 64,
    /// Pop n items, build set. Operand: u16 count.
    BuildSet = 65,
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
    FormatValue = 66,
    /// Pop n parts, concatenate for f-string. Operand: u16 count.
    BuildFString = 67,
    /// Build a slice object from stack values. No operand.
    ///
    /// Pops 3 values from stack: step, stop, start (TOS order).
    /// Each value can be None (for default) or an integer.
    /// Creates a `HeapData::Slice` and pushes a `Value::Ref` to it.
    BuildSlice = 68,
    /// Pop iterable, pop list, extend list with iterable items.
    ///
    /// Used for `*args` unpacking: builds a list of positional args,
    /// then extends it with unpacked iterables.
    ListExtend = 69,
    /// Pop TOS (list), push tuple containing the same elements.
    ///
    /// Used after building the args list to create the final args tuple
    /// for `CallFunctionEx`.
    ListToTuple = 70,
    /// Pop mapping, pop dict, update dict with mapping. Operand: u16 func_name_id.
    ///
    /// Used for `**kwargs` unpacking. The func_name_id is used for error messages
    /// when the mapping contains non-string keys.
    DictMerge = 71,

    // === Comprehension Building ===
    /// Append TOS to list for comprehension. Operand: u8 depth (number of iterators).
    ///
    /// Stack: [..., list, iter1, ..., iterN, value] -> [..., list, iter1, ..., iterN]
    /// Pops value (TOS), appends to list at stack position (len - 2 - depth).
    /// Depth equals the number of nested iterators (generators) in the comprehension.
    ListAppend = 72,
    /// Add TOS to set for comprehension. Operand: u8 depth (number of iterators).
    ///
    /// Stack: [..., set, iter1, ..., iterN, value] -> [..., set, iter1, ..., iterN]
    /// Pops value (TOS), adds to set at stack position (len - 2 - depth).
    /// May raise TypeError if value is unhashable.
    SetAdd = 73,
    /// Set dict[key] = value for comprehension. Operand: u8 depth (number of iterators).
    ///
    /// Stack: [..., dict, iter1, ..., iterN, key, value] -> [..., dict, iter1, ..., iterN]
    /// Pops value (TOS) and key (TOS-1), sets dict[key] = value.
    /// Dict is at stack position (len - 3 - depth).
    /// May raise TypeError if key is unhashable.
    DictSetItem = 74,

    // === Subscript & Attribute ===
    /// a[b]: pop index, pop obj, push result.
    BinarySubscr = 75,
    /// a[b] = c: pop value, pop index, pop obj.
    StoreSubscr = 76,
    // NOTE: DeleteSubscr removed - `del` statement not supported by parser
    /// Pop obj, push obj.attr. Operand: u16 name_id.
    LoadAttr = 77,
    /// Pop module, push module.attr for `from ... import`. Operand: u16 name_id.
    ///
    /// Like `LoadAttr` but raises `ImportError` instead of `AttributeError`
    /// when the attribute is not found. Used for `from module import name`.
    LoadAttrImport = 78,
    /// Pop value, pop obj, set obj.attr. Operand: u16 name_id.
    StoreAttr = 79,
    // NOTE: DeleteAttr removed - `del` statement not supported by parser

    // === Function Calls ===
    /// Call TOS with n positional args. Operand: u8 arg_count.
    CallFunction = 80,
    /// Call a builtin function directly. Operands: u8 builtin_id, u8 arg_count.
    ///
    /// The builtin_id is the discriminant of `BuiltinsFunctions` (via `FromRepr`).
    /// This is an optimization over `LoadConst + CallFunction` that avoids:
    /// - Constant pool lookup
    /// - Pushing/popping the callable on the stack
    /// - Runtime type dispatch in call_function
    CallBuiltinFunction = 81,
    /// Call a builtin type constructor directly. Operands: u8 type_id, u8 arg_count.
    ///
    /// The type_id is the discriminant of `BuiltinsTypes` (via `FromRepr`).
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    CallBuiltinType = 82,
    /// Call with positional and keyword args.
    ///
    /// Operands: u8 pos_count, u8 kw_count, then kw_count u16 name indices.
    ///
    /// Stack: [callable, pos_args..., kw_values...]
    /// After the two count bytes, there are kw_count little-endian u16 values,
    /// each being a StringId index for the corresponding keyword argument name.
    CallFunctionKw = 83,
    /// Call attribute on object. Operands: u16 name_id, u8 arg_count.
    ///
    /// This is used for both method calls (`obj.method(args)`) and module
    /// attribute calls (`module.func(args)`). The attribute is looked up
    /// on the object and called with the given arguments.
    CallAttr = 84,
    /// Call attribute with keyword args. Operands: u16 name_id, u8 pos_count, u8 kw_count, then kw_count u16 name indices.
    ///
    /// Stack: [obj, pos_args..., kw_values...]
    /// After the operands, there are kw_count little-endian u16 values,
    /// each being a StringId index for the corresponding keyword argument name.
    CallAttrKw = 85,
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
    CallFunctionExtended = 86,
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
    CallAttrExtended = 87,

    // === Control Flow ===
    /// Unconditional relative jump. Operand: i16 offset.
    Jump = 88,
    /// Jump if TOS truthy, always pop. Operand: i16 offset.
    JumpIfTrue = 89,
    /// Jump if TOS falsy, always pop. Operand: i16 offset.
    JumpIfFalse = 90,
    /// Jump if TOS truthy (keep), else pop. Operand: i16 offset.
    JumpIfTrueOrPop = 91,
    /// Jump if TOS falsy (keep), else pop. Operand: i16 offset.
    JumpIfFalseOrPop = 92,

    // === Iteration ===
    /// Convert TOS to iterator.
    GetIter = 93,
    /// Advance iterator or jump to end. Operand: i16 offset.
    ForIter = 94,

    // === Function Definition ===
    /// Create function object. Operand: u16 func_id.
    MakeFunction = 95,
    /// Create closure. Operands: u16 func_id, u8 cell_count.
    MakeClosure = 96,

    // === Exception Handling ===
    // Note: No SetupTry/PopExceptHandler - we use static exception_table
    /// Raise TOS as exception.
    Raise = 97,
    // NOTE: RaiseFrom removed - `raise ... from ...` not supported by parser
    /// Re-raise current exception (bare `raise`).
    Reraise = 98,
    /// Clear current_exception when exiting except block.
    ClearException = 99,
    /// Check if exception matches type for except clause.
    ///
    /// Stack: [..., exception, exc_type] -> [..., exception, bool]
    /// Validates that exc_type is a valid exception type (ExcType or tuple of ExcTypes).
    /// If invalid, raises TypeError. If valid, pushes True if exception matches, else False.
    CheckExcMatch = 100,

    // === Return ===
    /// Return TOS from function.
    ReturnValue = 101,

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
    Await = 102,

    // === Unpacking ===
    /// Unpack TOS into n values. Operand: u8 count.
    UnpackSequence = 103,
    /// Unpack with *rest. Operands: u8 before, u8 after.
    UnpackEx = 104,

    // === Special ===
    /// No operation (for patching/alignment).
    Nop = 105,

    // === Module Operations ===
    /// Load a built-in module onto the stack. Operand: u8 module_id.
    ///
    /// The module_id maps to `BuiltinModule` (0=sys, 1=typing).
    /// Creates the module on the heap and pushes a `Value::Ref` to it.
    LoadModule = 106,
    /// Raises `ModuleNotFoundError` at runtime. Operand: u16 constant index for module name.
    ///
    /// This opcode is emitted when the compiler encounters an import of an unknown module.
    /// Instead of failing at compile time, the error is deferred to runtime so that
    /// imports inside `if TYPE_CHECKING:` blocks or other non-executed code paths
    /// don't cause errors.
    ///
    /// The operand is an index into the constant pool where the module name string is stored.
    RaiseImportError = 107,
    /// Duplicate the top two stack values, preserving order: `[a, b] -> [a, b, a, b]`.
    ///
    Dup2 = 108,
    /// Delete global variable (set to Undefined). Operand: u16 slot.
    ///
    DeleteGlobal = 109,

    /// Pop a mapping, silently merge into the dict at `depth`. Operand: u8 depth.
    ///
    /// Used for `**expr` unpack inside dict literals, where later keys overwrite earlier ones
    /// (unlike `DictMerge` which raises `TypeError` on duplicate keys).
    ///
    /// Stack: [..., dict, iter1, ..., iterN, mapping] -> [..., dict, iter1, ..., iterN]
    /// Pops mapping (TOS), merges into dict at stack position `len - 2 - depth`.
    /// Raises `TypeError` if `mapping` is not a dict.
    DictUpdate = 110,
    /// Pop an iterable, add all items to set at `depth`. Operand: u8 depth.
    ///
    /// Used for `*expr` unpack inside set literals (e.g., `{*a, 1}`).
    /// Follows the same depth convention as `ListAppend`/`SetAdd`.
    ///
    /// Stack: [..., set, iter1, ..., iterN, iterable] -> [..., set, iter1, ..., iterN]
    /// Pops iterable (TOS), adds each item to set at stack position `len - 2 - depth`.
    /// Raises `TypeError` if iterable is not iterable.
    SetExtend = 111,

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
    BeforeWith = 112,
    /// Normal exit from a `with` block: call `__exit__(None, None, None)` on TOS,
    /// pop the context manager, and push the result of the call.
    ///
    /// Stack: [..., ctx] -> [..., result]
    ///
    /// The compiler emits a trailing `Pop` to discard the result — splitting the
    /// "call + discard" into two opcodes keeps the call shape compatible with
    /// `__exit__` implementations that yield `OsCall`/`External`/`MethodCall`
    /// (the host's resume push lands as the `result`, which `Pop` then drops).
    WithExit = 113,
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
    WithExceptStart = 114,

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
    LiftToTop = 115,
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
    RaiseUnboundLocal = 116,
    /// Method-call variant of [`Opcode::DictMerge`]: same stack effect, same
    /// duplicate-key semantics, but the error wording is qualified with the
    /// receiver's Python type — e.g. `list.sort()` instead of bare `sort()`.
    ///
    /// Emitted by the compiler for `CallAttrExtended` paths where the receiver
    /// is at known stack depth 4 below TOS at the time the op runs
    /// (`[receiver, args_tuple, kwargs_dict, mapping]`). Matches CPython's
    /// `obj.method() got multiple values for keyword argument 'X'` form,
    /// which CPython produces because it has the bound method's `__qualname__`
    /// available — we synthesise the equivalent by peeking the receiver.
    MethodDictMerge = 117,

    // Both assert opcodes use `assert_flags`; `ASSERT_CMP_FLAG` selects the
    // fused-comparison form and the low nibble stores the comparison operator.
    /// Fused bare `assert`: stack [..., test] -> [...], or with
    /// `ASSERT_CMP_FLAG` [..., lhs, rhs] -> [...]. Falls through on success;
    /// failures raise `AssertionError` with the operand repr(s), except
    /// literal `False` which has no detail. Comparison errors match `Compare*`.
    Assert = 118,
    /// Raise for failed `assert test, msg`: stack [..., test, msg] -> raises,
    /// or with `ASSERT_CMP_FLAG` [..., lhs, rhs, msg] -> raises. The message
    /// comes first, with introspected detail appended when available.
    AssertFailed = 119,
    /// Allocate a closure cell initialized to `Undefined` and push its reference.
    ///
    /// Used for captured targets in inlined comprehensions.
    BuildCell = 120,
    /// Unbind a closure cell's contents (CPython's `DELETE_DEREF`): later
    /// loads raise the free-variable `NameError`. Emitted by the implicit
    /// cleanup of a captured `except ... as` target. Operand: u16 slot.
    DeleteCell = 121,
}
// Samuel: do not remove this comment!
// NOTE: opcodes serialize as a single byte, hard-capping this enum at 256
// variants — roughly half are already taken. Spend slots sparingly: prefer a
// flags/operand encoding on one opcode (e.g. `Assert`/`FormatValue`) over a
// family of near-identical opcodes, unless the instruction is hot enough that
// decoding the discriminating operand would cost measurable dispatch time.

/// Byte layout of an opcode's in-stream operand.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandShape {
    None = 0,
    U8 = 1,
    I8 = 2,
    U16 = 3,
    Offset = 4,
    U8U8 = 5,
    U16U8 = 6,
    U16U16 = 7,
    U16U8U8 = 8,
    CallKw = 9,
    CallAttrKw = 10,
}

impl Opcode {
    /// Returns this opcode's serialized operand shape.
    fn operand_shape(self) -> OperandShape {
        match self {
            Self::Pop
            | Self::Dup
            | Self::Dup2
            | Self::Rot2
            | Self::Rot3
            | Self::LoadNone
            | Self::LoadTrue
            | Self::LoadFalse
            | Self::LoadLocal0
            | Self::LoadLocal1
            | Self::LoadLocal2
            | Self::LoadLocal3
            | Self::BinaryAdd
            | Self::BinarySub
            | Self::BinaryMul
            | Self::BinaryDiv
            | Self::BinaryFloorDiv
            | Self::BinaryMod
            | Self::BinaryPow
            | Self::BinaryAnd
            | Self::BinaryOr
            | Self::BinaryXor
            | Self::BinaryLShift
            | Self::BinaryRShift
            | Self::BinaryMatMul
            | Self::CompareEq
            | Self::CompareNe
            | Self::CompareLt
            | Self::CompareLe
            | Self::CompareGt
            | Self::CompareGe
            | Self::CompareIs
            | Self::CompareIsNot
            | Self::CompareIn
            | Self::CompareNotIn
            | Self::UnaryNot
            | Self::UnaryNeg
            | Self::UnaryPos
            | Self::UnaryInvert
            | Self::InplaceAdd
            | Self::InplaceSub
            | Self::InplaceMul
            | Self::InplaceDiv
            | Self::InplaceFloorDiv
            | Self::InplaceMod
            | Self::InplacePow
            | Self::InplaceAnd
            | Self::InplaceOr
            | Self::InplaceXor
            | Self::InplaceLShift
            | Self::InplaceRShift
            | Self::BuildSlice
            | Self::ListExtend
            | Self::ListToTuple
            | Self::BinarySubscr
            | Self::StoreSubscr
            | Self::GetIter
            | Self::Raise
            | Self::Reraise
            | Self::ClearException
            | Self::CheckExcMatch
            | Self::ReturnValue
            | Self::Await
            | Self::Nop
            | Self::BeforeWith
            | Self::WithExit
            | Self::WithExceptStart
            | Self::BuildCell => OperandShape::None,
            Self::LoadLocal
            | Self::StoreLocal
            | Self::DeleteLocal
            | Self::FormatValue
            | Self::ListAppend
            | Self::SetAdd
            | Self::DictSetItem
            | Self::CallFunction
            | Self::CallFunctionExtended
            | Self::LoadModule
            | Self::UnpackSequence
            | Self::DictUpdate
            | Self::SetExtend
            | Self::LiftToTop
            | Self::Assert
            | Self::AssertFailed => OperandShape::U8,
            Self::LoadSmallInt => OperandShape::I8,
            Self::LoadConst
            | Self::LoadLocalW
            | Self::StoreLocalW
            | Self::LoadGlobal
            | Self::StoreGlobal
            | Self::LoadCell
            | Self::StoreCell
            | Self::DeleteCell
            | Self::BuildList
            | Self::BuildTuple
            | Self::BuildDict
            | Self::BuildSet
            | Self::BuildFString
            | Self::DictMerge
            | Self::LoadAttr
            | Self::LoadAttrImport
            | Self::StoreAttr
            | Self::RaiseImportError
            | Self::DeleteGlobal
            | Self::RaiseUnboundLocal
            | Self::MethodDictMerge => OperandShape::U16,
            Self::Jump
            | Self::JumpIfTrue
            | Self::JumpIfFalse
            | Self::JumpIfTrueOrPop
            | Self::JumpIfFalseOrPop
            | Self::ForIter => OperandShape::Offset,
            Self::CallBuiltinFunction | Self::CallBuiltinType | Self::UnpackEx => OperandShape::U8U8,
            Self::CallAttr | Self::CallAttrExtended | Self::MakeFunction => OperandShape::U16U8,
            Self::LoadGlobalCallable => OperandShape::U16U16,
            Self::MakeClosure => OperandShape::U16U8U8,
            Self::CallFunctionKw => OperandShape::CallKw,
            Self::CallAttrKw => OperandShape::CallAttrKw,
        }
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
    /// u16 little-endian then u8 (e.g. `MakeFunction`, `CallAttr`).
    U16U8(u16, u8),
    /// Two u16 little-endian (e.g. `LoadGlobalCallable`).
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

impl Operand<'_> {
    /// Returns this operand's serialized byte shape.
    fn shape(self) -> OperandShape {
        match self {
            Self::None => OperandShape::None,
            Self::U8(_) => OperandShape::U8,
            Self::I8(_) => OperandShape::I8,
            Self::U16(_) => OperandShape::U16,
            Self::Offset(_) => OperandShape::Offset,
            Self::U8U8(..) => OperandShape::U8U8,
            Self::U16U8(..) => OperandShape::U16U8,
            Self::U16U16(..) => OperandShape::U16U16,
            Self::U16U8U8(..) => OperandShape::U16U8U8,
            Self::CallKw { .. } => OperandShape::CallKw,
            Self::CallAttrKw { .. } => OperandShape::CallAttrKw,
        }
    }
}

impl Opcode {
    /// Returns the operand-stack effect of this opcode paired with `operand`
    /// (positive = push, negative = pop).
    ///
    /// Returns `i32` because u16-count opcodes (notably `BuildDict`)
    /// can pop up to `2 * u16::MAX` values, which overflows `i16`; the
    /// builder's depth tracker accumulates in `i32` anyway.
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
    pub fn stack_effect(self, operand: Operand<'_>) -> i32 {
        #![expect(clippy::allow_attributes, reason = "expect seems broken with enum_glob_use")]
        #[allow(clippy::enum_glob_use, reason = "simplifies churn")]
        use Opcode::*; // allow local import
        assert_eq!(
            self.operand_shape(),
            operand.shape(),
            "wrong operand shape for {self:?}"
        );
        match (self, operand) {
            // === Variable-effect: U8 operand ===
            (CallFunction, Operand::U8(arg_count)) => -i32::from(arg_count),
            (CallFunctionExtended, Operand::U8(flags)) => -(1 + i32::from(flags & 0x01)),
            (FormatValue, Operand::U8(flags)) => {
                // Spec is on the stack iff `FORMAT_VALUE_HAS_SPEC` is set —
                // the static/dynamic discriminator (`FORMAT_VALUE_STATIC_SPEC`)
                // doesn't change the pop count.
                if flags & FORMAT_VALUE_HAS_SPEC != 0 { -1 } else { 0 }
            }
            (UnpackSequence, Operand::U8(n)) => i32::from(n) - 1,
            // Fused forms pop two test operands; `AssertFailed` also pops the
            // explicit message before entering dead code.
            (Assert, Operand::U8(flags)) => {
                if flags & ASSERT_CMP_FLAG != 0 {
                    -2
                } else {
                    -1
                }
            }
            (AssertFailed, Operand::U8(flags)) => {
                if flags & ASSERT_CMP_FLAG != 0 {
                    -3
                } else {
                    -2
                }
            }

            // === Variable-effect: U16 operand ===
            (BuildList | BuildTuple | BuildSet | BuildFString, Operand::U16(n)) => 1 - i32::from(n),
            (BuildDict, Operand::U16(n)) => 1 - 2 * i32::from(n),

            // === Variable-effect: U8U8 operand ===
            // UnpackEx: pops 1, pushes (before + 1 + after) → before + after.
            (UnpackEx, Operand::U8U8(before, after)) => i32::from(before) + i32::from(after),
            // Builtin calls: no callable on stack, pops args, pushes result → 1 - arg_count.
            (CallBuiltinFunction | CallBuiltinType, Operand::U8U8(_, arg_count)) => 1 - i32::from(arg_count),

            // === Variable-effect: U16U8 operand ===
            (MakeFunction, Operand::U16U8(_, defaults)) => 1 - i32::from(defaults),
            (CallAttr, Operand::U16U8(_, arg_count)) => -i32::from(arg_count),
            (CallAttrExtended, Operand::U16U8(_, flags)) => -(1 + i32::from(flags & 0x01)),

            // === Variable-effect: U16U8U8 operand ===
            // MakeClosure: pops `cell_count` cells AND `defaults_count` defaults,
            // pushes the closure → 1 - defaults - cells.
            (MakeClosure, Operand::U16U8U8(_, defaults, cells)) => 1 - i32::from(defaults) - i32::from(cells),

            // === Variable-effect: variable-length kw operands ===
            // pops callable + pos_args + kw_args, pushes result → -(pos_count + kw_count).
            (CallFunctionKw, Operand::CallKw { pos_count, kwname_ids }) => {
                let kw_count = i32::try_from(kwname_ids.len()).expect("keyword count exceeds i32");
                -(i32::from(pos_count) + kw_count)
            }
            (
                CallAttrKw,
                Operand::CallAttrKw {
                    pos_count, kwname_ids, ..
                },
            ) => {
                let kw_count = i32::try_from(kwname_ids.len()).expect("keyword count exceeds i32");
                -(i32::from(pos_count) + kw_count)
            }

            // === Fixed-effect, no operand ===
            (Pop, Operand::None) => -1,
            (Dup, Operand::None) => 1,
            (Dup2, Operand::None) => 2,
            (Rot2 | Rot3, Operand::None) => 0,
            (LoadNone | LoadTrue | LoadFalse | BuildCell, Operand::None) => 1,
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
            (DeleteGlobal | DeleteCell, Operand::U16(_)) => 0,
            (LoadAttr | LoadAttrImport, Operand::U16(_)) => 0,
            (StoreAttr, Operand::U16(_)) => -2,
            // `DictMerge` takes a u16 operand carrying the func_name_id for
            // the duplicate-key TypeError message. `MethodDictMerge` shares
            // the stack effect and additionally peeks the receiver under
            // the popped operands to qualify the error wording.
            (DictMerge | MethodDictMerge, Operand::U16(_)) => -1,
            // `RaiseImportError` takes a u16 const_id naming the missing module.
            (RaiseImportError, Operand::U16(_)) => 0,
            // `RaiseUnboundLocal(name_id)` always raises — fall-through is dead
            // code, but the tracker absorbs the bytes with effect 0 before the
            // following region starts.
            (RaiseUnboundLocal, Operand::U16(_)) => 0,
            // === Fixed-effect, U16U16 operand ===
            (LoadGlobalCallable, Operand::U16U16(..)) => 1,

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

/// Computes an FNV-1a hash over the canonical opcode table.
#[cfg(test)]
pub(crate) fn opcode_fingerprint() -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = OFFSET_BASIS;
    let mut opcodes = Opcode::iter().collect::<Vec<_>>();
    opcodes.sort_unstable_by_key(|opcode| *opcode as u8);
    for opcode in opcodes {
        let byte = opcode as u8;
        for value in [byte]
            .into_iter()
            .chain(format!("{opcode:?}").bytes())
            .chain([0, opcode.operand_shape() as u8])
        {
            hash ^= u64::from(value);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_flags_roundtrip() {
        // The bare form and every comparison operator must round-trip.
        assert_eq!(decode_assert_flags(assert_flags(None)), Some(None));
        for op in CmpOperator::iter() {
            assert_eq!(decode_assert_flags(assert_flags(Some(op))), Some(Some(op)));
        }
        // Bytes `assert_flags` can't produce are rejected: a cmp nibble
        // without the flag, an out-of-range nibble, and stray high bits.
        assert_eq!(decode_assert_flags(0x01), None);
        assert_eq!(decode_assert_flags(ASSERT_CMP_FLAG | 0x0A), None);
        assert_eq!(decode_assert_flags(0x20 | ASSERT_CMP_FLAG), None);
    }
}
