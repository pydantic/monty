//! Threaded opcode dispatch with bounded batches and cold exit payloads.

use fast::advance_iterator;

use super::{
    AwaitResult, CallResult, CellValue, Closure, DropWithContext, ExcType, ExcTypeExt, FrameExit, FrameNamespace,
    FunctionDefaults, FunctionId, HeapData, IMPORT_STAR_BIND, MATCH_KEYS_REST, Opcode, PyTrait, RunError, StringId, VM,
    Value, decode_assert_flags, recursion,
};
use crate::bytecode::subscription::{GlobalSubscript, LocalSubscript, LocalSubscriptKey};

macro_rules! yield_if_parked {
    ($self:expr, $context:expr) => {
        if $self.current_frame.is_parked {
            return $context.finish($self.pending_futures_exit());
        }
    };
}

/// Tries an operation and routes a Python exception through the active frames.
macro_rules! try_catch {
    ($self:expr, $context:expr, $expr:expr) => {
        if let Err(e) = $expr {
            if let Some(result) = $self.handle_exception(e) {
                return $context.error(result);
            }
            yield_if_parked!($self, $context);
        }
    };
}

/// Routes a Python exception through the active frames.
///
/// Wrapped in a block to allow use in match arm expressions.
macro_rules! catch {
    ($self:expr, $context:expr, $err:expr) => {{
        if let Some(result) = $self.handle_exception($err) {
            return $context.error(result);
        }
        yield_if_parked!($self, $context);
    }};
}

/// Applies a relative jump offset to the current frame IP.
///
/// Uses checked arithmetic to safely compute the new IP, panicking if the
/// jump would result in a negative or overflowing instruction pointer.
macro_rules! jump_relative {
    ($ip:expr, $offset:expr) => {{
        $ip = $ip
            .checked_add_signed($offset.into())
            .expect("jump resulted in negative or overflowing IP");
    }};
}

/// Handles the result of a load operation that may yield a `FrameExit::NameLookup`.
///
/// `load_local` and `load_global` return `Result<Option<FrameExit>, RunError>`:
/// - `Ok(None)`: load succeeded, value is on the stack
/// - `Ok(Some(FrameExit::NameLookup { .. }))`: unresolved name, yield to host
/// - `Err(e)`: exception (e.g., UnboundLocalError)
macro_rules! handle_load_result {
    ($self:expr, $context:expr, $result:expr) => {
        match $result {
            Ok(None) => {}
            Ok(Some(frame_exit)) => return $context.exit(frame_exit),
            Err(e) => catch!($self, $context, e),
        }
    };
}

/// Handles the result of a call operation that returns `CallResult`.
///
/// Used by LoadAttr, the call opcodes, `MatchAttr` and the context-manager
/// opcodes. `Value` and `FramePushed` are handled inline; every suspending or
/// awaiting variant goes through the outlined [`VM::suspend_call_result`], so
/// each expansion adds little to the dispatch loop's stack frame — native
/// recursion (`__repr__` calling `repr`) re-enters that loop once per level.
macro_rules! handle_call_result {
    ($self:expr, $context:expr, $result:expr) => {
        match $result {
            Ok(CallResult::Value(result)) => $self.push(result),
            Ok(CallResult::FramePushed) => {}
            Ok(other) => match $self.suspend_call_result(other) {
                Ok(None) => {}
                Ok(Some(exit)) => return $context.exit(exit),
                Err(err) => catch!($self, $context, err),
            },
            Err(err) => catch!($self, $context, err),
        }
    };
}

mod fast;

type Handler = for<'h> fn(&mut VM<'h>, &[u8], usize, &mut DispatchContext, u32, &DispatchTable) -> Status;

/// Only actual exits write a payload; continuation crosses helpers in one register.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Status {
    Continue,
    Exit,
}

struct DispatchContext {
    countdown: u8,
    batches_until_poll: u8,
    check_limits: bool,
    poll_print: bool,
    reentrant: bool,
    outcome: Option<Result<FrameExit, RunError>>,
}

impl DispatchContext {
    #[cold]
    fn exit(&mut self, exit: FrameExit) -> Status {
        self.finish(Ok(exit))
    }

    #[cold]
    fn error(&mut self, error: RunError) -> Status {
        self.finish(Err(error))
    }

    #[cold]
    fn finish(&mut self, outcome: Result<FrameExit, RunError>) -> Status {
        self.outcome = Some(outcome);
        Status::Exit
    }
}

#[repr(C)]
struct DispatchTable {
    handlers: [Handler; 256],
    checkpoint: Handler,
}

/// Local handler state. Entry points still pass scalars so continuations can tail-call.
struct Dispatch<'a, 'h> {
    vm: &'a mut VM<'h>,
    bytes: &'a [u8],
    ip: usize,
    start: usize,
    context: &'a mut DispatchContext,
    remaining: u32,
    table: &'a DispatchTable,
}

#[expect(clippy::inline_always, reason = "Keep scalar dispatch calls in each handler.")]
impl Dispatch<'_, '_> {
    /// Continues at the cursor, publishing the source position only at a checkpoint.
    #[inline(always)]
    fn next(self) -> Status {
        if self.remaining == 0 {
            self.vm.instruction_ip = self.start;
        }
        next(self.vm, self.bytes, self.ip, self.context, self.remaining, self.table)
    }

    /// Runs the semantic handler from the original operand position.
    #[inline(always)]
    fn fallback(self, opcode: u8) -> Status {
        (SEMANTICS[usize::from(opcode)])(
            self.vm,
            self.bytes,
            self.start + 1,
            self.context,
            self.remaining,
            self.table,
        )
    }

    /// Reloads the active frame after an operation that can execute Python.
    #[inline(always)]
    fn resume(self) -> Status {
        next(
            self.vm,
            self.vm.current_frame.bytecode,
            self.vm.current_frame.ip,
            self.context,
            self.remaining,
            self.table,
        )
    }

    /// Reads an operand and advances the cursor; fallback retains its original position.
    #[inline(always)]
    fn read_u8(&mut self) -> u8 {
        let value = self.bytes[self.ip];
        self.ip += 1;
        value
    }

    /// Reads a little-endian operand and advances the cursor.
    #[inline(always)]
    fn read_u16(&mut self) -> u16 {
        let value = u16::from_le_bytes([self.bytes[self.ip], self.bytes[self.ip + 1]]);
        self.ip += 2;
        value
    }
}

macro_rules! operation_body {
    (inline; $body:item) => {
        #[inline(always)]
        $body
    };
    (; $body:item) => {
        #[inline(never)]
        $body
    };
}

// Generic families receive the opcode from the same declaration as its fallback.
macro_rules! select_handler {
    ($opcode:ident, $semantic:ident) => {
        $semantic
    };
    ($opcode:ident, $semantic:ident, $fast:ident) => {
        select_handler!(@entry fast::$fast)
    };
    ($opcode:ident, $semantic:ident, $fast:ident::<OP>) => {
        select_handler!(@entry fast::$fast::<{ Opcode::$opcode as u8 }>)
    };
    (@entry $fast:expr) => {
        |vm, bytes, ip, context, remaining, table| {
            $fast(Dispatch { vm, bytes, ip, start: ip - 1, context, remaining, table })
        }
    };
}

macro_rules! opcode_handlers {
    ($vm:ident, $context:ident; $( $(#[$attr:meta])* $opcode:ident => $name:ident $(using $fast:ident $(::<$generic:ident>)?)? $(with $policy:ident)? $body:block )*) => {
        $( $(#[$attr])*
        #[inline(never)]
        fn $name(vm: &mut VM<'_>, _bytes: &[u8], ip: usize, context: &mut DispatchContext, remaining: u32, table: &DispatchTable) -> Status {
            // Finish all operation-local cleanup before dispatching the next instruction.
            operation_body! { $($policy)?;
            fn execute($vm: &mut VM<'_>, $context: &mut DispatchContext) -> Status {
                let _ = (&$vm, &$context);
                $body;
                Status::Continue
            }
            }
            vm.current_frame.ip = ip;
            vm.instruction_ip = ip - 1;
            let status = execute(vm, context);
            if status != Status::Continue {
                return status;
            }
            next(vm, vm.current_frame.bytecode, vm.current_frame.ip, context, remaining, table)
        })*
        const SEMANTICS: [Handler; 256] = {
            let mut handlers = [invalid as Handler; 256];
            $( $(#[$attr])* { handlers[Opcode::$opcode as usize] = $name; } )*
            handlers
        };
        const HANDLERS: DispatchTable = DispatchTable {
            handlers: {
                let mut handlers = SEMANTICS;
                $( $(#[$attr])* {
                    handlers[Opcode::$opcode as usize] = select_handler!($opcode, $name $(, $fast $(::<$generic>)?)?);
                } )*
                handlers
            },
            checkpoint,
        };
    };
}

opcode_handlers! {
vm, context;
Pop => op_pop using native_stack::<OP> {
    let value = vm.pop();
    value.drop_with(vm);
}
Dup => op_dup using native_stack::<OP> {
    let value = vm.peek().clone_with_heap(vm);
    vm.push(value);
}
Dup2 => op_dup2 using native_stack::<OP> {
    let len = vm.stack.len();
    let first = vm.stack[len - 2].clone_with_heap(vm);
    let second = vm.stack[len - 1].clone_with_heap(vm);
    vm.push(first);
    vm.push(second);
}
Rot2 => op_rot2 using native_stack::<OP> {
    // Swap top two: [a, b] → [b, a]
    let len = vm.stack.len();
    vm.stack.swap(len - 1, len - 2);
}
Rot3 => op_rot3 using native_stack::<OP> {
    // Rotate top three: [a, b, c] → [c, a, b]
    // Uses in-place rotation without cloning
    let len = vm.stack.len();
    // Move c out, then shift a→b→c, then put c at a's position
    // Equivalent to: [..rest, a, b, c] → [..rest, c, a, b]
    vm.stack[len - 3..].rotate_right(1);
}
LoadConst => op_load_const using native_constant {
    let idx = vm.current_frame.fetch_u16();
    let value = vm.current_frame.code.constant(idx);
    vm.push(value.clone_with_heap(vm));
}
LoadNone => op_load_none using native_literal::<OP> {
    vm.push(Value::None);
}
LoadTrue => op_load_true using native_literal::<OP> {
    vm.push(Value::Bool(true));
}
LoadFalse => op_load_false using native_literal::<OP> {
    vm.push(Value::Bool(false));
}
BuildCell => op_build_cell {
    let cell_id = vm.heap.allocate(HeapData::Cell(CellValue(Value::Undefined)));
    vm.push(Value::Ref(cell_id));
}
LoadSmallInt => op_load_small_int using native_literal::<OP> {
    let n = vm.current_frame.fetch_i8();
    vm.push(Value::Int(i64::from(n)));
}
LoadLocal0 => op_load_local0 using native_local::<OP> {
    try_catch!(vm, context, vm.load_local(0));
}
LoadLocal1 => op_load_local1 using native_local::<OP> {
    try_catch!(vm, context, vm.load_local(1));
}
LoadLocal2 => op_load_local2 using native_local::<OP> {
    try_catch!(vm, context, vm.load_local(2));
}
LoadLocal3 => op_load_local3 using native_local::<OP> {
    try_catch!(vm, context, vm.load_local(3));
}
LoadLocal => op_load_local using native_local::<OP> {
    let slot = u16::from(vm.current_frame.fetch_u8());
    try_catch!(vm, context, vm.load_local(slot));
}
LoadLocalW => op_load_local_w using native_local::<OP> {
    let slot = vm.current_frame.fetch_u16();
    try_catch!(vm, context, vm.load_local(slot));
}
StoreLocal => op_store_local using native_store::<OP> {
    let slot = u16::from(vm.current_frame.fetch_u8());
    vm.store_local(slot);
}
StoreLocalW => op_store_local_w using native_store::<OP> {
    let slot = vm.current_frame.fetch_u16();
    vm.store_local(slot);
}
LiftToTop => op_lift_to_top using native_stack::<OP> {
    let n = vm.current_frame.fetch_u8();
    // Move the item at TOS - n to TOS, shifting items in
    // between down by one. Single `rotate_left(1)` on the
    // affected slice does exactly that.
    let len = vm.stack.len();
    let src_idx = len - 1 - n as usize;
    vm.stack[src_idx..].rotate_left(1);
}
SinkTop => op_sink_top {
    let n = usize::from(vm.current_frame.fetch_u8());
    // Move TOS beneath the n items below it: one rotation of the
    // affected slice, the mirror image of `LiftToTop`.
    let len = vm.stack.len();
    vm.stack[len - 1 - n..].rotate_right(1);
}
RaiseUnboundLocal => op_raise_unbound_local {
    let name_idx = vm.current_frame.fetch_u16();
    let name_id = StringId::from_index(name_idx);
    catch!(vm, context, vm.unbound_local_error(0, Some(name_id)));
}
DeleteLocal => op_delete_local {
    let slot = u16::from(vm.current_frame.fetch_u8());
    vm.delete_local(slot);
}
DeleteGlobal => op_delete_global {
    let slot = vm.current_frame.fetch_u16();
    try_catch!(vm, context, vm.delete_global(slot));
}
LoadName => op_load_name {
    let (slot, name_idx, flags) = vm.current_frame.fetch_u16_u16_u8();
    handle_load_result!(vm, context, vm.load_name(slot, StringId::from_index(name_idx), flags));
}
StoreName => op_store_name {
    let (slot, name_idx, flags) = vm.current_frame.fetch_u16_u16_u8();
    try_catch!(vm, context, vm.store_name(slot, StringId::from_index(name_idx), flags));
}
DeleteName => op_delete_name {
    let (slot, name_idx, flags) = vm.current_frame.fetch_u16_u16_u8();
    try_catch!(vm, context, vm.delete_name(slot, StringId::from_index(name_idx), flags));
}
LoadGlobal => op_load_global {
    let slot = vm.current_frame.fetch_u16();
    handle_load_result!(vm, context, vm.load_global(slot));
}
LoadGlobalCallable => op_load_global_callable {
    let (slot, name_idx) = vm.current_frame.fetch_u16_u16();
    let name_id = StringId::from_index(name_idx);
    vm.load_global_callable(slot, name_id);
}
StoreGlobal => op_store_global {
    let slot = vm.current_frame.fetch_u16();
    vm.store_global(slot);
}
LoadCell => op_load_cell {
    let slot = vm.current_frame.fetch_u16();
    try_catch!(vm, context, vm.load_cell(slot));
}
StoreCell => op_store_cell {
    let slot = vm.current_frame.fetch_u16();
    vm.store_cell(slot);
}
DeleteCell => op_delete_cell {
    let slot = vm.current_frame.fetch_u16();
    vm.delete_cell(slot);
}
BinaryAdd => op_binary_add using numeric::<OP> {
    try_catch!(vm, context, vm.binary_add());
}
BinarySub => op_binary_sub using numeric::<OP> {
    try_catch!(vm, context, vm.binary_sub());
}
BinaryMul => op_binary_mul using numeric::<OP> {
    try_catch!(vm, context, vm.binary_mult());
}
BinaryDiv => op_binary_div using division::<OP> {
    try_catch!(vm, context, vm.binary_div());
}
BinaryFloorDiv => op_binary_floor_div using division::<OP> {
    try_catch!(vm, context, vm.binary_floordiv());
}
BinaryMod => op_binary_mod using division::<OP> {
    try_catch!(vm, context, vm.binary_mod());
}
BinaryPow => op_binary_pow {
    try_catch!(vm, context, vm.binary_pow());
}
BinaryAnd => op_binary_and using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_and());
}
BinaryOr => op_binary_or using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_or());
}
BinaryXor => op_binary_xor using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_xor());
}
BinaryLShift => op_binary_l_shift using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_lshift());
}
BinaryRShift => op_binary_r_shift using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_rshift());
}
BinaryMatMul => op_binary_mat_mul {
    try_catch!(vm, context, vm.binary_matmul());
}
CompareEq => op_compare_eq using comparison::<OP> {
    try_catch!(vm, context, vm.compare_eq());
}
CompareNe => op_compare_ne using comparison::<OP> {
    try_catch!(vm, context, vm.compare_ne());
}
CompareLt => op_compare_lt using comparison::<OP> {
    try_catch!(vm, context, vm.compare_lt());
}
CompareLe => op_compare_le using comparison::<OP> {
    try_catch!(vm, context, vm.compare_le());
}
CompareGt => op_compare_gt using comparison::<OP> {
    try_catch!(vm, context, vm.compare_gt());
}
CompareGe => op_compare_ge using comparison::<OP> {
    try_catch!(vm, context, vm.compare_ge());
}
CompareIs => op_compare_is {
    try_catch!(vm, context, vm.compare_is());
}
CompareIsNot => op_compare_is_not {
    try_catch!(vm, context, vm.compare_is_not());
}
CompareIn => op_compare_in {
    try_catch!(vm, context, vm.compare_in());
}
CompareNotIn => op_compare_not_in {
    try_catch!(vm, context, vm.compare_not_in());
}
UnaryNot => op_unary_not using unary::<OP> {
    let value = vm.pop();
    let result = value.py_bool(vm);
    value.drop_with(vm);
    match result {
        Ok(value) => vm.push(Value::Bool(!value)),
        Err(error) => catch!(vm, context, error),
    }
}
UnaryNeg => op_unary_neg using unary::<OP> {
    try_catch!(vm, context, vm.unary_neg());
}
UnaryPos => op_unary_pos using unary::<OP> {
    try_catch!(vm, context, vm.unary_pos());
}
UnaryInvert => op_unary_invert using unary::<OP> {
    try_catch!(vm, context, vm.unary_invert());
}
InplaceAdd => op_inplace_add using numeric::<OP> {
    try_catch!(vm, context, vm.inplace_add());
}
InplaceSub => op_inplace_sub using numeric::<OP> {
    try_catch!(vm, context, vm.inplace_sub());
}
InplaceMul => op_inplace_mul using numeric::<OP> {
    try_catch!(vm, context, vm.inplace_mult());
}
InplaceDiv => op_inplace_div using division::<OP> {
    try_catch!(vm, context, vm.binary_div());
}
InplaceFloorDiv => op_inplace_floor_div using division::<OP> {
    try_catch!(vm, context, vm.binary_floordiv());
}
InplaceMod => op_inplace_mod using division::<OP> {
    try_catch!(vm, context, vm.binary_mod());
}
InplacePow => op_inplace_pow {
    try_catch!(vm, context, vm.binary_pow());
}
InplaceAnd => op_inplace_and using bitwise::<OP> {
    try_catch!(vm, context, vm.inplace_and());
}
InplaceOr => op_inplace_or using bitwise::<OP> {
    try_catch!(vm, context, vm.inplace_or());
}
InplaceXor => op_inplace_xor using bitwise::<OP> {
    try_catch!(vm, context, vm.inplace_xor());
}
InplaceLShift => op_inplace_l_shift using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_lshift());
}
InplaceRShift => op_inplace_r_shift using bitwise::<OP> {
    try_catch!(vm, context, vm.binary_rshift());
}
BuildList => op_build_list {
    let count = vm.current_frame.fetch_u16() as usize;
    vm.build_list(count);
}
BuildTuple => op_build_tuple {
    let count = vm.current_frame.fetch_u16() as usize;
    vm.build_tuple(count);
}
BuildDict => op_build_dict {
    let count = vm.current_frame.fetch_u16() as usize;
    try_catch!(vm, context, vm.build_dict(count));
}
BuildSet => op_build_set {
    let count = vm.current_frame.fetch_u16() as usize;
    try_catch!(vm, context, vm.build_set(count));
}
FormatValue => op_format_value {
    let flags = vm.current_frame.fetch_u8();
    try_catch!(vm, context, vm.format_value(flags));
}
BuildFString => op_build_f_string {
    let count = vm.current_frame.fetch_u16() as usize;
    try_catch!(vm, context, vm.build_fstring(count));
}
BuildSlice => op_build_slice {
    try_catch!(vm, context, vm.build_slice());
}
ListExtend => op_list_extend {
    try_catch!(vm, context, vm.list_extend());
}
ListToTuple => op_list_to_tuple {
    try_catch!(vm, context, vm.list_to_tuple());
}
DictMerge => op_dict_merge {
    let func_name_id = vm.current_frame.fetch_u16();
    try_catch!(vm, context, vm.dict_merge(func_name_id));
}
MethodDictMerge => op_method_dict_merge {
    let func_name_id = vm.current_frame.fetch_u16();
    try_catch!(vm, context, vm.method_dict_merge(func_name_id));
}
DictUpdate => op_dict_update {
    let depth = vm.current_frame.fetch_u8() as usize;
    try_catch!(vm, context, vm.dict_update(depth));
}
SetExtend => op_set_extend {
    let depth = vm.current_frame.fetch_u8() as usize;
    try_catch!(vm, context, vm.set_extend(depth));
}
ListAppend => op_list_append {
    let depth = vm.current_frame.fetch_u8() as usize;
    try_catch!(vm, context, vm.list_append(depth));
}
SetAdd => op_set_add {
    let depth = vm.current_frame.fetch_u8() as usize;
    try_catch!(vm, context, vm.set_add(depth));
}
DictSetItem => op_dict_set_item {
    let depth = vm.current_frame.fetch_u8() as usize;
    try_catch!(vm, context, vm.dict_set_item(depth));
}
LoadSubscrLocal => op_load_subscr_local using native_subscript::<OP> {
    let (object, key) = vm.current_frame.fetch_u8_u8();
    try_catch!(vm, context, vm.subscript_locals_fallback(u16::from(object), LocalSubscriptKey::Local(key)));
}
LoadSubscrConst => op_load_subscr_const using native_subscript::<OP> {
    let (key, object_kind) = vm.current_frame.fetch_u16_u8();
    let operand = LocalSubscript::decode_constant(key, object_kind);
    try_catch!(vm, context, vm.subscript_locals_fallback(u16::from(operand.object()), operand.key()));
}
SubscrToLocal => op_subscr_to_local using native_subscript_store {
    let (slots, destination) = vm.current_frame.fetch_u16_u8();
    let [object, key] = slots.to_le_bytes();
    match vm.subscript_locals_fallback(u16::from(object), LocalSubscriptKey::Local(key)) {
        Ok(()) => vm.store_local(u16::from(destination)),
        Err(error) => catch!(vm, context, error),
    }
}
LoadGlobalSubscr => op_load_global_subscr using native_global_subscript {
    let operand = vm.current_frame.fetch_u16();
    // Boundary fallback retains the original key-load/subscript continuation.
    vm.instruction_ip += 1;
    handle_load_result!(vm, context, vm.load_global(GlobalSubscript::decode(operand).slot()));
}
BinarySubscr => op_binary_subscr with inline {
    if !vm.try_native_stack_subscript() {
        let index = vm.pop();
        let obj = vm.pop();
        let result = obj.py_getitem(&index, vm);
        obj.drop_with(vm);
        index.drop_with(vm);
        match result {
            Ok(v) => vm.push(v),
            Err(e) => catch!(vm, context, e),
        }
    }
}
StoreSubscr => op_store_subscr {
    // Stack order: value, obj, index (TOS)
    let index = vm.pop();
    let mut obj = vm.pop();
    let value = vm.pop();
    let result = obj.py_setitem(index, value, vm);
    obj.drop_with(vm);
    if let Err(e) = result {
        catch!(vm, context, e);
    }
}
LoadAttr => op_load_attr {
    let name_idx = vm.current_frame.fetch_u16();
    let name_id = StringId::from_index(name_idx);
    handle_call_result!(vm, context, vm.load_attr(name_id));
}
LoadSubmodule => op_load_submodule {
    let (name_idx, path_idx) = vm.current_frame.fetch_u16_u16();
    let path_id = StringId::from_index(path_idx);
    // the module a following `LoadAttrImport` names in its `ImportError`
    vm.current_frame.import_from_module = Some(path_id);
    handle_call_result!(vm, context, vm.load_submodule(StringId::from_index(name_idx), path_id));
}
LoadAttrImport => op_load_attr_import {
    let name_idx = vm.current_frame.fetch_u16();
    let name_id = StringId::from_index(name_idx);
    // the compiler emits this only after the statement's `LoadModule`
    let module_id = vm
        .current_frame
        .import_from_module
        .expect("LoadAttrImport runs after its LoadModule set the frame's module");
    handle_call_result!(vm, context, vm.load_attr_import(name_id, module_id));
}
StoreAttr => op_store_attr {
    let name_idx = vm.current_frame.fetch_u16();
    let name_id = StringId::from_index(name_idx);
    try_catch!(vm, context, vm.store_attr(name_id));
}
Jump => op_jump using native_branch::<OP> {
    let offset = vm.current_frame.fetch_i16();
    jump_relative!(vm.current_frame.ip, offset);
}
JumpIfTrue => op_jump_if_true using native_branch::<OP> {
    let offset = vm.current_frame.fetch_i16();
    let cond = vm.pop();
    let result = cond.py_bool(vm);
    cond.drop_with(vm);
    match result {
        Ok(true) => jump_relative!(vm.current_frame.ip, offset),
        Ok(false) => {}
        Err(error) => catch!(vm, context, error),
    }
}
JumpIfFalse => op_jump_if_false using native_branch::<OP> {
    let offset = vm.current_frame.fetch_i16();
    let cond = vm.pop();
    let result = cond.py_bool(vm);
    cond.drop_with(vm);
    match result {
        Ok(false) => jump_relative!(vm.current_frame.ip, offset),
        Ok(true) => {}
        Err(error) => catch!(vm, context, error),
    }
}
JumpIfTrueOrPop => op_jump_if_true_or_pop using native_branch::<OP> {
    let offset = vm.current_frame.fetch_i16();
    let value = vm.pop();
    match value.py_bool(vm) {
        Ok(true) => {
            vm.push(value);
            jump_relative!(vm.current_frame.ip, offset);
        }
        Ok(false) => value.drop_with(vm),
        Err(error) => {
            value.drop_with(vm);
            catch!(vm, context, error);
        }
    }
}
JumpIfFalseOrPop => op_jump_if_false_or_pop using native_branch::<OP> {
    let offset = vm.current_frame.fetch_i16();
    let value = vm.pop();
    match value.py_bool(vm) {
        Ok(true) => value.drop_with(vm),
        Ok(false) => {
            vm.push(value);
            jump_relative!(vm.current_frame.ip, offset);
        }
        Err(error) => {
            value.drop_with(vm);
            catch!(vm, context, error);
        }
    }
}
GetIter => op_get_iter {
    let value = vm.pop();
    let iterator = value.py_iter(vm).and_then(|call| call.require_synchronous(vm));
    value.drop_with(vm);
    match iterator {
        Ok(iterator) => vm.push(iterator),
        Err(e) => catch!(vm, context, e),
    }
}
ForIter => op_for_iter using for_iter {
    let offset = vm.current_frame.fetch_i16();
    let Value::Ref(heap_id) = *vm.peek() else {
        return context.error(RunError::internal("ForIter: expected iterator ref on stack"));
    };
    let iter = vm.heap.read(heap_id);
    let status = advance_iterator(vm, context, iter, heap_id, offset);
    if status != Status::Continue {
        return status;
    }
}
CallFunction => op_call_function {
    let arg_count = vm.current_frame.fetch_u8() as usize;
    handle_call_result!(vm, context, vm.exec_call_function(arg_count));
}
CallBuiltinFunction => op_call_builtin_function {
    let (builtin_id, arg_count) = vm.current_frame.fetch_u8_u8();
    let result = vm.exec_call_builtin_function(builtin_id, arg_count as usize);
    handle_call_result!(vm, context, result);
}
CallBuiltinType => op_call_builtin_type {
    let (type_id, arg_count) = vm.current_frame.fetch_u8_u8();
    let arg_count = arg_count as usize;

    handle_call_result!(vm, context, vm.exec_call_builtin_type(type_id, arg_count));
}
CallFunctionKw => op_call_function_kw {
    // Fetch operands: pos_count, kw_count, then kw_count name indices
    let (pos_count, kw_count) = vm.current_frame.fetch_u8_u8();
    let (pos_count, kw_count) = (pos_count as usize, kw_count as usize);

    handle_call_result!(vm, context, vm.exec_call_function_kw(pos_count, kw_count));
}
CallAttr => op_call_attr {
    // CallAttr: u16 name_id, u8 arg_count
    // Stack: [obj, arg1, arg2, ..., argN] -> [result]
    let (name_idx, arg_count) = vm.current_frame.fetch_u16_u8();
    let name_id = StringId::from_index(name_idx);
    let arg_count = arg_count as usize;

    handle_call_result!(vm, context, vm.exec_call_attr(name_id, arg_count));
}
CallAttrKw => op_call_attr_kw {
    // CallAttrKw: u16 name_id, u8 pos_count, u8 kw_count, then kw_count u16 name indices
    // Stack: [obj, pos_args..., kw_values...] -> [result]
    let (name_idx, pos_count, kw_count) = vm.current_frame.fetch_u16_u8_u8();
    let name_id = StringId::from_index(name_idx);
    let (pos_count, kw_count) = (pos_count as usize, kw_count as usize);

    handle_call_result!(vm, context, vm.exec_call_attr_kw(name_id, pos_count, kw_count));
}
CallFunctionExtended => op_call_function_extended {
    let flags = vm.current_frame.fetch_u8();
    let has_kwargs = (flags & 0x01) != 0;

    handle_call_result!(vm, context, vm.exec_call_function_extended(has_kwargs));
}
CallAttrExtended => op_call_attr_extended {
    let (name_idx, flags) = vm.current_frame.fetch_u16_u8();
    let name_id = StringId::from_index(name_idx);
    let has_kwargs = (flags & 0x01) != 0;

    handle_call_result!(vm, context, vm.exec_call_attr_extended(name_id, has_kwargs));
}
MakeFunction => op_make_function {
    let (func_idx, defaults_count) = vm.current_frame.fetch_u16_u8();
    let func_id = FunctionId::from_index(func_idx);
    let defaults_count = defaults_count as usize;

    // A function defined under an explicit globals dict carries it.
    let globals = vm
        .current_frame
        .namespace
        .as_deref()
        .and_then(FrameNamespace::dict_globals);
    if defaults_count == 0 && globals.is_none() {
        // No defaults - use inline Value::Function (no heap allocation)
        vm.push(Value::DefFunction(func_id));
    } else {
        // Pop default values from stack (drain maintains order: first pushed = first in vec)
        let defaults = vm.pop_n(defaults_count);
        if let Some(globals) = globals {
            vm.heap.inc_ref(globals);
        }

        // Create FunctionDefaults on heap and push reference
        let heap_id = vm.heap.allocate(HeapData::FunctionDefaults(FunctionDefaults {
            func_id,
            defaults,
            globals,
        }));
        vm.push(Value::Ref(heap_id));
    }
}
MakeClosure => op_make_closure {
    let (func_idx, defaults_count, cell_count) = vm.current_frame.fetch_u16_u8_u8();
    let func_id = FunctionId::from_index(func_idx);
    let (defaults_count, cell_count) = (defaults_count as usize, cell_count as usize);

    // Pop cells from stack (pushed after defaults, so on top)
    // Cells are Value::Ref pointing to HeapData::Cell
    // We use individual pops which reverses order, so we need to reverse back
    let mut cells = Vec::with_capacity(cell_count);
    for _ in 0..cell_count {
        // mut needed for dec_ref_forget when memory-model-checks feature is enabled
        #[cfg_attr(not(feature = "memory-model-checks"), expect(unused_mut))]
        let mut cell_val = vm.pop();
        match &cell_val {
            Value::Ref(heap_id) => {
                // Keep the reference - the Closure will own the HeapId
                cells.push(*heap_id);
                // Mark the Value as dereferenced since Closure takes ownership
                // of the reference count (we don't call drop_with because
                // we're not decrementing the refcount, just transferring it)
                #[cfg(feature = "memory-model-checks")]
                cell_val.dec_ref_forget();
            }
            _ => {
                return context.error(RunError::internal("MakeClosure: expected cell reference on stack"));
            }
        }
    }
    // Reverse to get original order (individual pops reverse the order)
    cells.reverse();

    // Pop default values from stack (drain maintains order: first pushed = first in vec)
    let defaults = vm.pop_n(defaults_count);
    let globals = vm
        .current_frame
        .namespace
        .as_deref()
        .and_then(FrameNamespace::dict_globals);
    if let Some(globals) = globals {
        vm.heap.inc_ref(globals);
    }

    // Create Closure on heap and push reference
    let heap_id = vm.heap.allocate(HeapData::Closure(Closure {
        func_id,
        cells: cells.into_boxed_slice(),
        defaults: defaults.into_boxed_slice(),
        globals,
    }));
    vm.push(Value::Ref(heap_id));
}
Raise => op_raise {
    let exc = vm.pop();
    let error = vm.make_exception(&exc, true); // is_raise=true, hide caret
    // Re-raise an instance as-is so `raise e` preserves `e`'s
    // identity, like CPython. A bare type or non-exception has
    // nothing to reuse and rebuilds from the error.
    let raised = match &exc {
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Exception(_)) => Some(exc),
        _ => {
            exc.drop_with(vm);
            None
        }
    };
    if let Some(result) = vm.handle_exception_with_value(error, raised) {
        return context.error(result);
    }
    yield_if_parked!(vm, context);
}
Assert => op_assert {
    match decode_assert_flags(vm.current_frame.fetch_u8()).expect("invalid assert flags in bytecode")
    {
        Some(op) => try_catch!(vm, context, vm.assert_cmp(op)),
        None => try_catch!(vm, context, vm.assert_test()),
    }
}
AssertFailed => op_assert_failed {
    let cmp_op =
        decode_assert_flags(vm.current_frame.fetch_u8()).expect("invalid assert flags in bytecode");
    let error = vm.assert_failed_msg(cmp_op);
    catch!(vm, context, error);
}
RaiseFrom => op_raise_from {
    let cause = vm.pop();
    let exc = vm.pop();
    if let Some(result) = vm.raise_with_cause(exc, cause) {
        return context.error(result);
    }
    yield_if_parked!(vm, context);
}
Reraise => op_reraise {
    // Clone rather than pop: a locally caught bare raise must
    // preserve its enclosing handler's active exception.
    let (error, raised) = vm.reraise_exception();
    if let Some(result) = vm.handle_exception_with_value(error, raised) {
        return context.error(result);
    }
    yield_if_parked!(vm, context);
}
ClearException => op_clear_exception {
    // Pop the current exception from the stack
    // This restores the previous exception context (if any)
    if let Some(exc) = vm.exception_stack.pop() {
        exc.drop_with(vm);
    }
}
CheckExcMatch => op_check_exc_match {
    // Stack: [exception, exc_type] -> [exception, bool]
    let exc_type = vm.pop();
    let exception = vm.peek();
    let result = vm.check_exc_match(exception, &exc_type);
    exc_type.drop_with(vm);
    match result {
        Ok(matched) => vm.push(Value::Bool(matched)),
        // An invalid `except` type (e.g. `except 123:` or a
        // nested tuple) raises a `TypeError`. As in CPython this
        // is an ordinary exception raised while evaluating the
        // clause: it propagates out of the whole `try` and may be
        // caught by an enclosing handler, so route it through the
        // exception machinery rather than aborting the run.
        Err(err) => catch!(vm, context, err),
    }
}
ReturnValue => op_return_value {
    let value = vm.pop();
    if vm.suspended_frames.is_empty() {
        // Last frame - check if this is main task or spawned task
        let is_main_task = vm.is_main_task();

        if is_main_task {
            // Module-level return - we're done
            return context.exit(FrameExit::Return(value));
        }

        // Spawned task completed - handle task completion
        let result = vm.handle_task_completion(value);
        match result {
            Ok(AwaitResult::ValueReady(v)) => {
                vm.push(v);
            }
            Ok(AwaitResult::FramePushed) => {}
            Ok(AwaitResult::Yield(pending)) => {
                // All tasks blocked - return to host
                return context.exit(FrameExit::ResolveFutures(pending));
            }
            Err(e) => catch!(vm, context, e),
        }
        return Status::Continue;
    }
    // Read the initializer flag before popping the frame.
    let is_init = vm.current_frame().is_initializer;
    // Pop current frame; `stop` requests returning to the host
    // (e.g. `evaluate_function`).
    let stop = vm.pop_frame();
    if is_init {
        if !matches!(value, Value::None) {
            // CPython raises at the `Foo(...)` call site: the
            // initializer frame is already popped, so the traceback
            // matches (no `__init__` frame).
            let type_name = value.py_type_name(vm);
            value.drop_with(vm);
            let err = ExcType::type_error_init_return(type_name);
            if stop {
                // The initializer was driven by `evaluate_function`
                // and its frame boundary is already popped —
                // propagate directly rather than unwinding into
                // frames that must not observe this error. The
                // pending instance left on the operand stack is
                // reclaimed by the eventual `handle_exception`
                // stack drain (or final teardown).
                return context.error(err);
            }
            catch!(vm, context, err);
            return Status::Continue;
        }
        // `__init__` returned None — discard it. The instance was
        // pushed onto the caller's stack before this frame ran and
        // is the real result of `Foo(...)`.
        value.drop_with(vm);
        if stop {
            let instance = vm.pop();
            return context.exit(FrameExit::Return(instance));
        }
        // Instance (or imported module) already on the caller's
        // stack — push nothing.
        try_catch!(vm, context, vm.finish_module_init());
    } else if stop {
        // This frame indicated evaluation should stop - return to host with value
        // e.g. `evaluate_function`
        return context.exit(FrameExit::Return(value));
    } else {
        vm.push(value);
    }
}
Await => op_await {
    let result = vm.exec_get_awaitable();
    match result {
        Ok(AwaitResult::ValueReady(value)) => {
            vm.push(value);
        }
        Ok(AwaitResult::FramePushed) => {}
        Ok(AwaitResult::Yield(pending_calls)) => {
            // All tasks are blocked - return control to host
            return context.exit(FrameExit::ResolveFutures(pending_calls));
        }
        Err(e) => {
            catch!(vm, context, e);
        }
    }
}
UnpackSequence => op_unpack_sequence {
    let count = vm.current_frame.fetch_u8() as usize;
    try_catch!(vm, context, vm.unpack_sequence(count));
}
UnpackEx => op_unpack_ex {
    let (before, after) = vm.current_frame.fetch_u8_u8();
    try_catch!(vm, context, vm.unpack_ex(before as usize, after as usize));
}
MatchShape => op_match_shape {
    let (length, flags) = vm.current_frame.fetch_u16_u8();
    vm.match_shape(length, flags);
}
MatchKeys => op_match_keys {
    let mode = vm.current_frame.fetch_u8();
    if mode == MATCH_KEYS_REST {
        try_catch!(vm, context, vm.match_keys_rest());
    } else {
        try_catch!(vm, context, vm.match_keys());
    }
}
MatchClass => op_match_class {
    let nargs = usize::from(vm.current_frame.fetch_u8());
    try_catch!(vm, context, vm.match_class(nargs));
}
MatchAttr => op_match_attr {
    let index = usize::from(vm.current_frame.fetch_u8());
    handle_call_result!(vm, context, vm.match_attr(index));
}
ImportStar => op_import_star {
    let mode = vm.current_frame.fetch_u8();
    if mode == IMPORT_STAR_BIND {
        try_catch!(vm, context, vm.import_star_bind());
    } else {
        handle_call_result!(vm, context, vm.import_star_value(mode));
    }
}
Nop => op_nop using native_nop {
    // No operation
}
LoadModule => op_load_module {
    let module_id = vm.current_frame.fetch_u16();
    // for the `LoadAttrImport`s of a `from ... import`; harmless for a bare `import`
    vm.current_frame.import_from_module = Some(StringId::from_index(module_id));
    handle_call_result!(vm, context, Ok(vm.load_module(module_id)));
}
BeforeWith => op_before_with {
    handle_call_result!(vm, context, vm.exec_before_with());
}
WithExit => op_with_exit {
    handle_call_result!(vm, context, vm.exec_with_exit());
}
WithExceptStart => op_with_except_start {
    handle_call_result!(vm, context, vm.exec_with_except_start());
}
}

impl VM<'_> {
    #[inline(never)]
    pub(super) fn dispatch_threaded(&mut self, check_limits: bool, poll_print: bool) -> Result<FrameExit, RunError> {
        let reentrant = self.run_reentry_depth < recursion::MAX_RUN_REENTRY_DEPTH;
        // Entry fetch executes one instruction before the first countdown decrement.
        let mut context = DispatchContext {
            countdown: if reentrant { 14 } else { 254 },
            batches_until_poll: 16,
            check_limits,
            poll_print,
            reentrant,
            outcome: None,
        };
        loop {
            let remaining = u32::from(context.countdown);
            let status = fetched(
                self,
                self.current_frame.bytecode,
                self.current_frame.ip,
                &mut context,
                remaining,
                &HANDLERS,
            );
            if status != Status::Continue {
                return context.outcome.take().expect("exit status requires a payload");
            }
        }
    }
}

#[expect(clippy::inline_always, reason = "Each handler must end in its own dispatch.")]
#[inline(always)]
fn next(
    vm: &mut VM<'_>,
    bytes: &[u8],
    ip: usize,
    context: &mut DispatchContext,
    remaining: u32,
    table: &DispatchTable,
) -> Status {
    if remaining == 0 {
        return (table.checkpoint)(vm, bytes, ip, context, remaining, table);
    }
    fetched(vm, bytes, ip, context, remaining - 1, table)
}

#[expect(clippy::inline_always, reason = "Keep instruction fetch in each handler.")]
#[inline(always)]
fn fetched(
    vm: &mut VM<'_>,
    bytes: &[u8],
    ip: usize,
    context: &mut DispatchContext,
    remaining: u32,
    table: &DispatchTable,
) -> Status {
    let byte = bytes[ip];
    (table.handlers[usize::from(byte)])(vm, bytes, ip + 1, context, remaining, table)
}

#[cold]
#[inline(never)]
fn checkpoint(
    vm: &mut VM<'_>,
    bytes: &[u8],
    ip: usize,
    context: &mut DispatchContext,
    remaining: u32,
    table: &DispatchTable,
) -> Status {
    vm.current_frame.ip = ip;
    // Nested chains unwind every 16 instructions, but keep the baseline polling cadence.
    if context.reentrant {
        context.batches_until_poll -= 1;
        if context.batches_until_poll != 0 {
            context.countdown = 15;
            return Status::Continue;
        }
        context.batches_until_poll = 16;
    }
    try_catch!(
        vm,
        context,
        vm.dispatch_checkpoint(context.check_limits, context.poll_print)
    );
    // Unwind the dispatch chain before starting the next batch.
    context.countdown = if context.reentrant { 15 } else { 255 };
    let _ = (bytes, table, remaining);
    Status::Continue
}

fn invalid(_: &mut VM<'_>, _: &[u8], _: usize, _: &mut DispatchContext, _: u32, _: &DispatchTable) -> Status {
    panic!("invalid opcode in bytecode")
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use strum::IntoEnumIterator;

    use super::{Handler, Opcode, SEMANTICS, invalid};

    #[test]
    fn every_opcode_has_a_handler() {
        for opcode in Opcode::iter() {
            assert!(
                !ptr::fn_addr_eq(SEMANTICS[opcode as usize], invalid as Handler),
                "{opcode:?}"
            );
        }
    }
}
