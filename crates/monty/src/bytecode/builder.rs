//! Builder for emitting bytecode during compilation.
//!
//! `CodeBuilder` provides methods for emitting opcodes and operands, handling
//! forward jumps with patching, and tracking source locations for tracebacks.

use std::collections::HashSet;

use super::{
    code::{Code, ConstPool, ExceptionEntry, LocationEntry},
    op::Opcode,
};
use crate::{intern::StringId, parse::CodeRange, value::Value};

/// Builder for emitting bytecode during compilation.
///
/// Handles encoding opcodes and operands into raw bytes, managing forward jumps
/// that need patching, and tracking source locations for traceback generation.
///
/// # Usage
///
/// ```ignore
/// let mut builder = CodeBuilder::new();
/// builder.set_location(some_range, None);
/// builder.emit(Opcode::LoadNone);
/// builder.emit_u8(Opcode::LoadLocal, 0);
/// let jump = builder.emit_jump(Opcode::JumpIfFalse);
/// // ... emit more code ...
/// builder.patch_jump(jump);
/// let code = builder.build(num_locals);
/// ```
#[derive(Debug, Default)]
pub struct CodeBuilder {
    /// The bytecode being built.
    bytecode: Vec<u8>,

    /// Constants collected during compilation.
    constants: Vec<Value>,

    /// Source location entries for traceback generation.
    location_table: Vec<LocationEntry>,

    /// Exception handler entries.
    exception_table: Vec<ExceptionEntry>,

    /// Current source location (set before emitting instructions).
    current_location: Option<CodeRange>,

    /// Current focus location within the source range.
    current_focus: Option<CodeRange>,

    /// Current stack depth for tracking max stack usage.
    current_stack_depth: u16,

    /// Maximum stack depth seen during compilation.
    max_stack_depth: u16,

    /// Local variable names indexed by slot number.
    ///
    /// Populated during compilation to enable proper NameError messages
    /// when accessing undefined local variables.
    local_names: Vec<Option<StringId>>,

    /// Local variable slots that are assigned somewhere in this function.
    ///
    /// Used to determine whether to raise `UnboundLocalError` or `NameError`
    /// when loading an undefined local variable.
    assigned_locals: HashSet<u16>,
}

impl CodeBuilder {
    /// Creates a new empty CodeBuilder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the current source location for subsequent instructions.
    ///
    /// This location will be recorded in the location table when the next
    /// instruction is emitted. Call this before emitting instructions that
    /// correspond to source code.
    pub fn set_location(&mut self, range: CodeRange, focus: Option<CodeRange>) {
        self.current_location = Some(range);
        self.current_focus = focus;
    }

    /// Emits a no-operand instruction and updates stack depth tracking.
    pub fn emit(&mut self, op: Opcode) {
        self.record_location();
        self.bytecode.push(op as u8);
        // Track stack effect for opcodes with known fixed effects
        if let Some(effect) = op.stack_effect() {
            self.adjust_stack(effect);
        }
    }

    /// Emits an instruction with a u8 operand and updates stack depth tracking.
    pub fn emit_u8(&mut self, op: Opcode, operand: u8) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.push(operand);
        // Track stack effect - some need operand-based calculation
        self.track_stack_effect_u8(op, operand);
    }

    /// Emits an instruction with an i8 operand and updates stack depth tracking.
    pub fn emit_i8(&mut self, op: Opcode, operand: i8) {
        self.record_location();
        self.bytecode.push(op as u8);
        // Reinterpret i8 as u8 for bytecode encoding
        self.bytecode.push(operand.to_ne_bytes()[0]);
        // Track stack effect for opcodes with known fixed effects
        if let Some(effect) = op.stack_effect() {
            self.adjust_stack(effect);
        }
    }

    /// Emits an instruction with a u16 operand (little-endian) and updates stack depth tracking.
    pub fn emit_u16(&mut self, op: Opcode, operand: u16) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&operand.to_le_bytes());
        // Track stack effect - some need operand-based calculation
        self.track_stack_effect_u16(op, operand);
    }

    /// Emits an instruction with a u16 operand followed by a u8 operand.
    ///
    /// Used for MakeFunction: func_id (u16) + defaults_count (u8)
    /// Used for CallMethod: method_name_id (u16) + arg_count (u8)
    pub fn emit_u16_u8(&mut self, op: Opcode, operand1: u16, operand2: u8) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&operand1.to_le_bytes());
        self.bytecode.push(operand2);
        // Track stack effects based on opcode
        match op {
            Opcode::MakeFunction => {
                // pops defaults_count defaults, pushes function: 1 - defaults_count
                self.adjust_stack(1 - i16::from(operand2));
            }
            Opcode::CallMethod => {
                // pops obj + args, pushes result: 1 - (1 + arg_count) = -arg_count
                self.adjust_stack(-i16::from(operand2));
            }
            _ => {
                if let Some(effect) = op.stack_effect() {
                    self.adjust_stack(effect);
                }
            }
        }
    }

    /// Emits an instruction with a u16 operand followed by two u8 operands.
    ///
    /// Used for MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
    pub fn emit_u16_u8_u8(&mut self, op: Opcode, operand1: u16, operand2: u8, operand3: u8) {
        self.record_location();
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&operand1.to_le_bytes());
        self.bytecode.push(operand2);
        self.bytecode.push(operand3);
        // MakeClosure: pops defaults_count defaults, pushes closure
        // Cell values are captured from locals, not popped from stack
        // Stack effect: 1 - defaults_count
        if op == Opcode::MakeClosure {
            self.adjust_stack(1 - i16::from(operand2));
        } else if let Some(effect) = op.stack_effect() {
            self.adjust_stack(effect);
        }
    }

    /// Emits `CallBuiltinFunction` instruction.
    ///
    /// Operands: builtin_id (u8) + arg_count (u8)
    ///
    /// The builtin_id is the `#[repr(u8)]` discriminant of `BuiltinsFunctions`.
    /// This is an optimization that avoids constant pool lookup and stack manipulation.
    pub fn emit_call_builtin_function(&mut self, builtin_id: u8, arg_count: u8) {
        self.record_location();
        self.bytecode.push(Opcode::CallBuiltinFunction as u8);
        self.bytecode.push(builtin_id);
        self.bytecode.push(arg_count);
        // CallBuiltinFunction: pops args, pushes result. No callable on stack.
        // Stack effect: 1 - arg_count
        self.adjust_stack(1 - i16::from(arg_count));
    }

    /// Emits `CallBuiltinType` instruction.
    ///
    /// Operands: type_id (u8) + arg_count (u8)
    ///
    /// The type_id is the `#[repr(u8)]` discriminant of `BuiltinsTypes`.
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    pub fn emit_call_builtin_type(&mut self, type_id: u8, arg_count: u8) {
        self.record_location();
        self.bytecode.push(Opcode::CallBuiltinType as u8);
        self.bytecode.push(type_id);
        self.bytecode.push(arg_count);
        // CallBuiltinType: pops args, pushes result. No callable on stack.
        // Stack effect: 1 - arg_count
        self.adjust_stack(1 - i16::from(arg_count));
    }

    /// Emits CallFunctionKw with inline keyword names.
    ///
    /// Operands: pos_count (u8) + kw_count (u8) + kw_count * name_id (u16 each)
    ///
    /// The kwname_ids slice contains StringId indices for each keyword argument
    /// name, in order matching how the values were pushed to the stack.
    pub fn emit_call_function_kw(&mut self, pos_count: u8, kwname_ids: &[u16]) {
        self.record_location();
        self.bytecode.push(Opcode::CallFunctionKw as u8);
        self.bytecode.push(pos_count);
        self.bytecode
            .push(u8::try_from(kwname_ids.len()).expect("keyword count exceeds u8"));
        for &name_id in kwname_ids {
            self.bytecode.extend_from_slice(&name_id.to_le_bytes());
        }
        // CallFunctionKw: pops callable + pos_args + kw_args, pushes result
        // Stack effect: 1 - (1 + pos_count + kw_count) = -pos_count - kw_count
        let kw_count = i16::try_from(kwname_ids.len()).expect("keyword count exceeds i16");
        let total_args = i16::from(pos_count) + kw_count;
        self.adjust_stack(-total_args);
    }

    /// Emits CallMethodKw with inline keyword names.
    ///
    /// Operands: method_name_id (u16) + pos_count (u8) + kw_count (u8) + kw_count * name_id (u16 each)
    ///
    /// The kwname_ids slice contains StringId indices for each keyword argument
    /// name, in order matching how the values were pushed to the stack.
    pub fn emit_call_method_kw(&mut self, method_name_id: u16, pos_count: u8, kwname_ids: &[u16]) {
        self.record_location();
        self.bytecode.push(Opcode::CallMethodKw as u8);
        self.bytecode.extend_from_slice(&method_name_id.to_le_bytes());
        self.bytecode.push(pos_count);
        self.bytecode
            .push(u8::try_from(kwname_ids.len()).expect("keyword count exceeds u8"));
        for &name_id in kwname_ids {
            self.bytecode.extend_from_slice(&name_id.to_le_bytes());
        }
        // CallMethodKw: pops obj + pos_args + kw_args, pushes result
        // Stack effect: 1 - (1 + pos_count + kw_count) = -pos_count - kw_count
        let kw_count = i16::try_from(kwname_ids.len()).expect("keyword count exceeds i16");
        let total_args = i16::from(pos_count) + kw_count;
        self.adjust_stack(-total_args);
    }

    /// Emits a forward jump instruction, returning a label to patch later.
    ///
    /// The jump offset is initially set to 0 and must be patched with
    /// `patch_jump()` once the target location is known.
    #[must_use]
    pub fn emit_jump(&mut self, op: Opcode) -> JumpLabel {
        self.record_location();
        let label = JumpLabel(self.bytecode.len());
        self.bytecode.push(op as u8);
        // Placeholder for i16 offset (will be patched)
        self.bytecode.extend_from_slice(&0i16.to_le_bytes());
        // Track stack effect
        match op {
            // ForIter: when successful (not jumping), pushes next value (+1)
            // When exhausted (jumping), pops iterator (-1), but that's after loop
            Opcode::ForIter => self.adjust_stack(1),
            // JumpIfTrueOrPop/JumpIfFalseOrPop: pops when not jumping (fallthrough)
            Opcode::JumpIfTrueOrPop | Opcode::JumpIfFalseOrPop => self.adjust_stack(-1),
            _ => {
                if let Some(effect) = op.stack_effect() {
                    self.adjust_stack(effect);
                }
            }
        }
        label
    }

    /// Patches a forward jump to point to the current bytecode location.
    ///
    /// The offset is calculated relative to the position after the jump
    /// instruction's operand (i.e., where execution would continue if
    /// the jump is not taken).
    ///
    /// # Panics
    ///
    /// Panics if the jump offset exceeds i16 range (-32768..32767), which
    /// indicates the function is too large. This is a compile-time error
    /// rather than silent truncation.
    pub fn patch_jump(&mut self, label: JumpLabel) {
        let target = self.bytecode.len();
        // Offset is relative to position after the jump instruction (opcode + i16 = 3 bytes)
        let target_i64 = i64::try_from(target).expect("bytecode target exceeds i64");
        let label_i64 = i64::try_from(label.0).expect("bytecode label exceeds i64");
        let raw_offset = target_i64 - label_i64 - 3;
        let offset =
            i16::try_from(raw_offset).expect("jump offset exceeds i16 range (-32768..32767); function too large");
        let bytes = offset.to_le_bytes();
        self.bytecode[label.0 + 1] = bytes[0];
        self.bytecode[label.0 + 2] = bytes[1];
    }

    /// Emits a backward jump to a known target offset.
    ///
    /// Unlike forward jumps, backward jumps have a known target at emit time,
    /// so no patching is needed.
    pub fn emit_jump_to(&mut self, op: Opcode, target: usize) {
        self.record_location();
        let current = self.bytecode.len();
        // Offset is relative to position after this instruction (current + 3)
        let target_i64 = i64::try_from(target).expect("bytecode target exceeds i64");
        let current_i64 = i64::try_from(current).expect("bytecode offset exceeds i64");
        let raw_offset = target_i64 - (current_i64 + 3);
        let offset =
            i16::try_from(raw_offset).expect("jump offset exceeds i16 range (-32768..32767); function too large");
        self.bytecode.push(op as u8);
        self.bytecode.extend_from_slice(&offset.to_le_bytes());
        // Track stack effect (jump instructions pop condition)
        if let Some(effect) = op.stack_effect() {
            self.adjust_stack(effect);
        }
    }

    /// Returns the current bytecode offset.
    ///
    /// Use this to record loop start positions for backward jumps.
    #[must_use]
    pub fn current_offset(&self) -> usize {
        self.bytecode.len()
    }

    /// Emits `LoadLocal`, using specialized opcodes for slots 0-3.
    ///
    /// Slots 0-3 use zero-operand opcodes (`LoadLocal0`, etc.) for efficiency.
    /// Slots 4-255 use `LoadLocal` with a u8 operand.
    /// Slots 256+ use `LoadLocalW` with a u16 operand.
    /// Registers a local variable name for a given slot.
    ///
    /// This is called during compilation when we encounter a variable access.
    /// The name is used to generate proper NameError messages.
    pub fn register_local_name(&mut self, slot: u16, name: StringId) {
        let slot_idx = slot as usize;
        // Extend the vector if needed
        if slot_idx >= self.local_names.len() {
            self.local_names.resize(slot_idx + 1, None);
        }
        // Only set if not already set (first occurrence determines the name)
        if self.local_names[slot_idx].is_none() {
            self.local_names[slot_idx] = Some(name);
        }
    }

    /// Registers a local variable slot as "assigned" (vs undefined reference).
    ///
    /// Called during compilation for variables that are assigned somewhere in the function.
    /// Used at runtime to determine whether to raise `UnboundLocalError` (assigned local
    /// accessed before assignment) or `NameError` (name doesn't exist anywhere).
    pub fn register_assigned_local(&mut self, slot: u16) {
        self.assigned_locals.insert(slot);
    }

    /// Emits a `LoadLocal` instruction, using specialized variants for common slots.
    pub fn emit_load_local(&mut self, slot: u16) {
        match slot {
            0 => self.emit(Opcode::LoadLocal0),
            1 => self.emit(Opcode::LoadLocal1),
            2 => self.emit(Opcode::LoadLocal2),
            3 => self.emit(Opcode::LoadLocal3),
            _ => {
                if let Ok(s) = u8::try_from(slot) {
                    self.emit_u8(Opcode::LoadLocal, s);
                } else {
                    self.emit_u16(Opcode::LoadLocalW, slot);
                }
            }
        }
    }

    /// Emits `StoreLocal`, using wide variant for slots > 255.
    pub fn emit_store_local(&mut self, slot: u16) {
        if let Ok(s) = u8::try_from(slot) {
            self.emit_u8(Opcode::StoreLocal, s);
        } else {
            self.emit_u16(Opcode::StoreLocalW, slot);
        }
    }

    /// Adds a constant to the pool, returning its index.
    ///
    /// # Panics
    ///
    /// Panics if the constant pool exceeds 65535 entries. This is a compile-time
    /// error indicating the function has too many constants.
    #[must_use]
    pub fn add_const(&mut self, value: Value) -> u16 {
        let idx = self.constants.len();
        let idx_u16 = u16::try_from(idx).expect("constant pool exceeds u16 range (65535); too many constants");
        self.constants.push(value);
        idx_u16
    }

    /// Adds an exception handler entry.
    ///
    /// Entries should be added in innermost-first order for nested try blocks.
    pub fn add_exception_entry(&mut self, entry: ExceptionEntry) {
        self.exception_table.push(entry);
    }

    /// Returns the current tracked stack depth.
    #[must_use]
    pub fn stack_depth(&self) -> u16 {
        self.current_stack_depth
    }

    /// Builds the final Code object.
    ///
    /// Consumes the builder and returns a Code object containing the
    /// compiled bytecode and all metadata.
    #[must_use]
    pub fn build(self, num_locals: u16) -> Code {
        // Convert local_names from Vec<Option<StringId>> to Vec<StringId>,
        // using StringId::default() for slots with no recorded name
        let local_names: Vec<StringId> = self.local_names.into_iter().map(Option::unwrap_or_default).collect();

        Code::new(
            self.bytecode,
            ConstPool::from_vec(self.constants),
            self.location_table,
            self.exception_table,
            num_locals,
            self.max_stack_depth,
            local_names,
            self.assigned_locals,
        )
    }

    /// Records the current location in the location table if set.
    fn record_location(&mut self) {
        if let Some(range) = self.current_location {
            let offset = u32::try_from(self.bytecode.len()).expect("bytecode length exceeds u32");
            self.location_table
                .push(LocationEntry::new(offset, range, self.current_focus));
        }
    }

    /// Sets the current stack depth to an absolute value.
    ///
    /// Used when compiling code paths that branch and reconverge with different
    /// stack states (e.g., break/continue through finally blocks).
    /// Updates `max_stack_depth` if the new depth exceeds it.
    pub fn set_stack_depth(&mut self, depth: u16) {
        self.current_stack_depth = depth;
        self.max_stack_depth = self.max_stack_depth.max(depth);
    }

    /// Adjusts the stack depth by the given delta.
    ///
    /// Positive values indicate pushes, negative values indicate pops.
    /// Updates `max_stack_depth` if the new depth exceeds it.
    fn adjust_stack(&mut self, delta: i16) {
        let new_depth = i32::from(self.current_stack_depth) + i32::from(delta);
        // Stack depth shouldn't go negative (indicates compiler bug)
        debug_assert!(new_depth >= 0, "Stack depth went negative: {new_depth}");
        // Safe cast: new_depth is non-negative and stack won't exceed u16::MAX in practice
        self.current_stack_depth = u16::try_from(new_depth.max(0)).unwrap_or(u16::MAX);
        self.max_stack_depth = self.max_stack_depth.max(self.current_stack_depth);
    }

    /// Tracks stack effect for opcodes with u8 operand.
    ///
    /// For opcodes with variable effects (like `CallFunction`, `BuildList`),
    /// calculates the effect based on the operand.
    fn track_stack_effect_u8(&mut self, op: Opcode, operand: u8) {
        let effect: i16 = match op {
            // CallFunction pops (callable + args), pushes result: -(1 + arg_count) + 1 = -arg_count
            Opcode::CallFunction => -i16::from(operand),
            // UnpackSequence pops 1, pushes n: n - 1
            Opcode::UnpackSequence => i16::from(operand) - 1,
            // ListAppend/SetAdd pop value: -1 (depth operand doesn't affect stack count)
            Opcode::ListAppend | Opcode::SetAdd => -1,
            // DictSetItem pops key and value: -2
            Opcode::DictSetItem => -2,
            // Default: use fixed effect if available
            _ => op.stack_effect().unwrap_or(0),
        };
        self.adjust_stack(effect);
    }

    /// Tracks stack effect for opcodes with u16 operand.
    ///
    /// For opcodes with variable effects (like `BuildList`, `BuildTuple`),
    /// calculates the effect based on the operand.
    fn track_stack_effect_u16(&mut self, op: Opcode, operand: u16) {
        // Safe cast: operand won't exceed i16::MAX in practice (would be a huge list)
        let operand_i16 = operand.cast_signed();
        let effect: i16 = match op {
            // BuildList/BuildTuple/BuildSet: pop n, push 1: -(n - 1) = 1 - n
            Opcode::BuildList | Opcode::BuildTuple | Opcode::BuildSet => 1 - operand_i16,
            // BuildDict: pop 2n (key-value pairs), push 1: 1 - 2n
            Opcode::BuildDict => 1 - 2 * operand_i16,
            // BuildFString: pop n parts, push 1: 1 - n
            Opcode::BuildFString => 1 - operand_i16,
            // Default: use fixed effect if available
            _ => op.stack_effect().unwrap_or(0),
        };
        self.adjust_stack(effect);
    }

    /// Manually adjust stack depth for complex scenarios.
    ///
    /// Use this when the compiler knows the exact stack effect that can't
    /// be determined from the opcode alone (e.g., exception handlers pushing
    /// an exception value).
    pub fn adjust_stack_depth(&mut self, delta: i16) {
        self.adjust_stack(delta);
    }
}

/// Label for a forward jump that needs patching.
///
/// Stores the bytecode offset where the jump instruction was emitted.
/// Pass this to `patch_jump()` once the target location is known.
#[derive(Debug, Clone, Copy)]
pub struct JumpLabel(usize);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_basic() {
        let mut builder = CodeBuilder::new();
        builder.emit(Opcode::LoadNone);
        builder.emit(Opcode::Pop);

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadNone as u8, Opcode::Pop as u8]);
    }

    #[test]
    fn test_emit_u8_operand() {
        let mut builder = CodeBuilder::new();
        builder.emit_u8(Opcode::LoadLocal, 42);

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadLocal as u8, 42]);
    }

    #[test]
    fn test_emit_u16_operand() {
        let mut builder = CodeBuilder::new();
        builder.emit_u16(Opcode::LoadConst, 0x1234);

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadConst as u8, 0x34, 0x12]);
    }

    #[test]
    fn test_forward_jump() {
        let mut builder = CodeBuilder::new();
        let jump = builder.emit_jump(Opcode::Jump);
        builder.emit(Opcode::LoadNone); // 1 byte, skipped by jump
        builder.emit(Opcode::LoadNone); // 1 byte, skipped by jump
        builder.patch_jump(jump);
        builder.emit(Opcode::LoadNone); // Return value
        builder.emit(Opcode::ReturnValue);

        let code = builder.build(0);
        // Jump at offset 0, target at offset 5 (after 2x LoadNone)
        // Offset = 5 - 0 - 3 = 2
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::Jump as u8,
                2,
                0, // i16 little-endian = 2
                Opcode::LoadNone as u8,
                Opcode::LoadNone as u8,
                Opcode::LoadNone as u8,
                Opcode::ReturnValue as u8,
            ]
        );
    }

    #[test]
    fn test_backward_jump() {
        let mut builder = CodeBuilder::new();
        let loop_start = builder.current_offset();
        builder.emit(Opcode::LoadNone); // offset 0, 1 byte
        builder.emit(Opcode::Pop); // offset 1, 1 byte
        builder.emit_jump_to(Opcode::Jump, loop_start); // offset 2, target 0

        let code = builder.build(0);
        // Jump at offset 2, target at offset 0
        // Offset = 0 - (2 + 3) = -5
        let expected_offset = (-5i16).to_le_bytes();
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::LoadNone as u8,
                Opcode::Pop as u8,
                Opcode::Jump as u8,
                expected_offset[0],
                expected_offset[1],
            ]
        );
    }

    #[test]
    fn test_load_local_specialization() {
        let mut builder = CodeBuilder::new();
        builder.emit_load_local(0);
        builder.emit_load_local(1);
        builder.emit_load_local(2);
        builder.emit_load_local(3);
        builder.emit_load_local(4);
        builder.emit_load_local(256);

        let code = builder.build(0);
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::LoadLocal0 as u8,
                Opcode::LoadLocal1 as u8,
                Opcode::LoadLocal2 as u8,
                Opcode::LoadLocal3 as u8,
                Opcode::LoadLocal as u8,
                4,
                Opcode::LoadLocalW as u8,
                0,
                1, // 256 in little-endian
            ]
        );
    }

    #[test]
    fn test_add_const() {
        let mut builder = CodeBuilder::new();
        let idx1 = builder.add_const(Value::Int(42));
        let idx2 = builder.add_const(Value::None);

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
    }
}
