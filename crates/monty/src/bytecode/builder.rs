//! Builder for emitting bytecode during compilation.
//!
//! `CodeBuilder` provides methods for emitting opcodes and operands, handling
//! forward jumps with patching, and tracking source locations for tracebacks.

use std::collections::HashSet;

use super::{
    code::{Code, ConstPool, ExceptionEntry, LocationEntry},
    compiler::CompileError,
    op::{Opcode, Operand},
};
use crate::{intern::StringId, parse::CodeRange, value::Value};

/// Builder for emitting bytecode during compilation.
///
/// Handles encoding opcodes and operands into raw bytes, managing forward jumps
/// that need patching, and tracking source locations for traceback generation.
///
/// The builder maintains an internal "dead code" state; during dead code emission
/// no bytes are written and no work is done.
///
/// # Usage
///
/// ```ignore
/// let mut builder = CodeBuilder::new();
/// builder.enter_region(0); // open the initial region at depth 0
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

    /// Operand-stack depth at the point the next opcode will be emitted, or
    /// `None` if not emitting a code region.
    ///
    /// Unconditional terminators (`Jump`, `ReturnValue`, `Raise`, `Reraise`,
    /// `RaiseImportError`, `RaiseUnboundLocal`)
    /// finish code regions, transitioning the builder to the dead-code state.
    current_stack_depth: Option<u16>,

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
    /// Creates a new empty `CodeBuilder` in the dead-code state — no region
    /// is open yet. Call `enter_region(0)` (or another depth, for an
    /// exception-table-reached region) before emitting.
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
    pub fn emit(&mut self, op: Opcode) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::None)
    }

    /// Emits an instruction with a u8 operand and updates stack depth tracking.
    pub fn emit_u8(&mut self, op: Opcode, operand: u8) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::U8(operand))
    }

    /// Emits an instruction with an i8 operand and updates stack depth tracking.
    pub fn emit_i8(&mut self, op: Opcode, operand: i8) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::I8(operand))
    }

    /// Emits an instruction with two u8 operands and updates stack depth tracking.
    ///
    /// Used for UnpackEx: before_count (u8) + after_count (u8)
    pub fn emit_u8_u8(&mut self, op: Opcode, operand1: u8, operand2: u8) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::U8U8(operand1, operand2))
    }

    /// Emits an instruction with a u16 operand (little-endian) and updates stack depth tracking.
    pub fn emit_u16(&mut self, op: Opcode, operand: u16) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::U16(operand))
    }

    /// Emits an instruction with a u16 operand followed by a u8 operand.
    ///
    /// Used for `MakeFunction`, `CallAttr`, `CallAttrExtended`.
    pub fn emit_u16_u8(&mut self, op: Opcode, operand1: u16, operand2: u8) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::U16U8(operand1, operand2))
    }

    /// Emits an instruction with a u16 operand followed by two u8 operands.
    ///
    /// Used for MakeClosure: func_id (u16) + defaults_count (u8) + cell_count (u8)
    pub fn emit_u16_u8_u8(
        &mut self,
        op: Opcode,
        operand1: u16,
        operand2: u8,
        operand3: u8,
    ) -> Result<(), CompileError> {
        self.emit_with_operand(op, Operand::U16U8U8(operand1, operand2, operand3))
    }

    /// Emits `CallBuiltinFunction` instruction.
    ///
    /// Operands: builtin_id (u8) + arg_count (u8)
    ///
    /// The builtin_id is the `#[repr(u8)]` discriminant of `BuiltinsFunctions`.
    /// This is an optimization that avoids constant pool lookup and stack manipulation.
    pub fn emit_call_builtin_function(&mut self, builtin_id: u8, arg_count: u8) -> Result<(), CompileError> {
        self.emit_with_operand(Opcode::CallBuiltinFunction, Operand::U8U8(builtin_id, arg_count))
    }

    /// Emits `CallBuiltinType` instruction.
    ///
    /// Operands: type_id (u8) + arg_count (u8)
    ///
    /// The type_id is the `#[repr(u8)]` discriminant of `BuiltinsTypes`.
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    pub fn emit_call_builtin_type(&mut self, type_id: u8, arg_count: u8) -> Result<(), CompileError> {
        self.emit_with_operand(Opcode::CallBuiltinType, Operand::U8U8(type_id, arg_count))
    }

    /// Emits CallFunctionKw with inline keyword names.
    ///
    /// Operands: pos_count (u8) + kw_count (u8) + kw_count * name_id (u16 each)
    ///
    /// The kwname_ids slice contains StringId indices for each keyword argument
    /// name, in order matching how the values were pushed to the stack.
    pub fn emit_call_function_kw(&mut self, pos_count: u8, kwname_ids: &[u16]) -> Result<(), CompileError> {
        self.emit_with_operand(Opcode::CallFunctionKw, Operand::CallKw { pos_count, kwname_ids })
    }

    /// Emits CallAttrKw with inline keyword names.
    ///
    /// Operands: attr_name_id (u16) + pos_count (u8) + kw_count (u8) + kw_count * name_id (u16 each)
    ///
    /// The kwname_ids slice contains StringId indices for each keyword argument
    /// name, in order matching how the values were pushed to the stack.
    pub fn emit_call_attr_kw(
        &mut self,
        attr_name_id: u16,
        pos_count: u8,
        kwname_ids: &[u16],
    ) -> Result<(), CompileError> {
        self.emit_with_operand(
            Opcode::CallAttrKw,
            Operand::CallAttrKw {
                attr_name_id,
                pos_count,
                kwname_ids,
            },
        )
    }

    /// Emits a forward jump instruction, returning a label to patch later.
    ///
    /// After `Jump` the tracker transitions to dead (it's unconditional).
    /// All other jumps continue to fall through.
    ///
    /// Returns a `CompileError` if the jump-taken target depth doesn't fit
    /// in `u16`, mirroring the same overflow case `adjust_stack` detects.
    ///
    /// # Panics
    ///
    /// Panics on non-jump opcodes (`op.jump_taken_stack_effect()` is the
    /// shared invariant check).
    pub fn emit_jump(&mut self, op: Opcode) -> Result<JumpLabel, CompileError> {
        let Some(pre_depth) = self.current_stack_depth else {
            // Dead code, emit dummy jump label
            return Ok(JumpLabel { inner: None });
        };
        // Capture the opcode position (where patch_jump will overwrite the i16)
        // before `emit_with_operand` pushes the bytes.
        let offset = self.current_offset();
        // Capture the source location now so any later `patch_jump` overflow
        // anchors the diagnostic at this jump site rather than the (often
        // unrelated) statement where the patch happens to resolve.
        let source_position = self.current_location.unwrap_or_default();
        // Jump-taken target depth. `jump_taken_delta` panics for non-jumps.
        let target_depth = u16::try_from(i32::from(pre_depth) + i32::from(op.jump_taken_stack_effect()))
            .map_err(|_| self.stack_too_large())?;
        // Emit jump with dummy offset; patch_jump is required to fill in the real offset later.
        self.emit_with_operand(op, Operand::Offset(RelativeOffset(0)))?;
        Ok(JumpLabel {
            inner: Some(JumpLabelInner {
                offset,
                stack_depth: target_depth,
                source_position,
            }),
        })
    }

    /// Patches a forward jump to point to the current bytecode location.
    ///
    /// State transitions: if the builder is emitting dead code, `patch_jump`
    /// re-establishes the live depth from `label.stack_depth`.
    ///
    /// Returns a `CompileError` if the resolved jump offset doesn't fit in
    /// `i16`, which means the function is too large for our bytecode encoding.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the jump label has a different stack depth
    /// compared to the current depth (if live) — this indicates a compiler bug
    /// in stack-effect tracking rather than a user-input-driven overflow.
    pub fn patch_jump(&mut self, label: JumpLabel) -> Result<(), CompileError> {
        let Some(label) = label.inner else {
            // emit_jump was dead code, nothing to do
            return Ok(());
        };

        let stack_depth = self.current_stack_depth.unwrap_or_else(|| {
            self.new_code_region(label.stack_depth);
            label.stack_depth
        });

        let target = JumpTargetInner {
            offset: self.current_offset(),
            stack_depth,
        };

        let offset = calculate_jump_offset(label, target)
            .ok_or_else(|| jump_too_large_at(label.source_position))?
            .as_i16();
        let bytes = offset.to_le_bytes();
        self.bytecode[label.offset.0 + 1] = bytes[0];
        self.bytecode[label.offset.0 + 2] = bytes[1];
        Ok(())
    }

    /// Emits a backward jump to a known target. Any jump opcode is accepted;
    /// `Opcode::jump_taken_delta` is the shared source of truth for the
    /// jump-taken stack effect (and panics for non-jump opcodes).
    ///
    /// Returns a `CompileError` if the jump offset doesn't fit in `i16`,
    /// which means the function is too large for our bytecode encoding.
    ///
    /// # Panics
    /// - Panics if the jump target was emitted in dead code, and the current
    ///   code is live.
    /// - In debug builds, panics if the current stack depth plus the jump's stack
    ///   effect do not match the jump target stack depth.
    /// - Panics on non-jump opcodes.
    pub fn emit_jump_to(&mut self, op: Opcode, target: JumpTarget) -> Result<(), CompileError> {
        let Some(target_depth) = self.current_stack_depth else {
            // Emitting dead code, do no work
            return Ok(());
        };

        let label = JumpLabelInner {
            offset: self.current_offset(),
            stack_depth: target_depth
                .checked_add_signed(op.jump_taken_stack_effect())
                .ok_or_else(|| self.stack_too_large())?,
            // Backward jumps are emitted and resolved at the same spot, so
            // there's no patch-time/emit-time split; either position is the
            // same statement.
            source_position: self.current_location.unwrap_or_default(),
        };
        let Some(target) = target.0 else {
            // Target is dead code
            unreachable!("emit_jump_to: cannot jump from live code to dead code");
        };

        let offset = calculate_jump_offset(label, target).ok_or_else(|| self.jump_too_large())?;
        self.emit_with_operand(op, Operand::Offset(offset))
    }

    /// Returns the current bytecode position as an opaque `Offset`.
    ///
    /// Use this to capture the bounds of try/except/finally regions for
    /// `ExceptionEntry::new`.
    #[must_use]
    pub fn current_offset(&self) -> Offset {
        Offset(self.bytecode.len())
    }

    /// Returns a `JumpTarget` capturing both the current bytecode position and
    /// the stack depth at that position.
    #[must_use]
    pub fn current_jump_target(&self) -> JumpTarget {
        JumpTarget(self.current_stack_depth.map(|depth| JumpTargetInner {
            offset: self.current_offset(),
            stack_depth: depth,
        }))
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

    /// Emits a `RaiseUnboundLocal` opcode carrying the comprehension target
    /// name to be reported in `UnboundLocalError`.
    ///
    /// Used by the comprehension compiler at sites where static analysis
    /// proves the target is read before its `for` clause assigns it.
    pub fn emit_raise_unbound_local(&mut self, name_id: StringId) -> Result<(), CompileError> {
        let name_idx = u16::try_from(name_id.index()).map_err(|_| self.name_id_too_large())?;
        self.emit_with_operand(Opcode::RaiseUnboundLocal, Operand::U16(name_idx))
    }

    /// Emits a `LoadLocal` instruction, using specialized variants for common slots.
    pub fn emit_load_local(&mut self, slot: u16) -> Result<(), CompileError> {
        match slot {
            0 => self.emit(Opcode::LoadLocal0),
            1 => self.emit(Opcode::LoadLocal1),
            2 => self.emit(Opcode::LoadLocal2),
            3 => self.emit(Opcode::LoadLocal3),
            _ => {
                if let Ok(s) = u8::try_from(slot) {
                    self.emit_u8(Opcode::LoadLocal, s)
                } else {
                    self.emit_u16(Opcode::LoadLocalW, slot)
                }
            }
        }
    }

    /// Emits a `LoadLocalCallable` instruction for call-context loads.
    ///
    /// Unlike `emit_load_local`, this does NOT use specialized 0-3 variants since
    /// external function calls are rare enough that the optimization isn't worth
    /// the extra opcode slots. The `name_id` is encoded directly in the operand
    /// to avoid needing to look up the name from the code's local_names array.
    pub fn emit_load_local_callable(&mut self, slot: u16, name_id: StringId) -> Result<(), CompileError> {
        let name_id_u16 = u16::try_from(name_id.index()).map_err(|_| self.name_id_too_large())?;
        if let Ok(s) = u8::try_from(slot) {
            self.emit_with_operand(Opcode::LoadLocalCallable, Operand::U8U16(s, name_id_u16))
        } else {
            self.emit_with_operand(Opcode::LoadLocalCallableW, Operand::U16U16(slot, name_id_u16))
        }
    }

    /// Emits a `LoadGlobalCallable` instruction for call-context loads.
    ///
    /// The `name_id` is encoded directly in the operand to avoid the ambiguity
    /// of looking up global names from a function's local_names array (global slots
    /// and local slots use different namespaces).
    pub fn emit_load_global_callable(&mut self, slot: u16, name_id: StringId) -> Result<(), CompileError> {
        let name_id_u16 = u16::try_from(name_id.index()).map_err(|_| self.name_id_too_large())?;
        self.emit_with_operand(Opcode::LoadGlobalCallable, Operand::U16U16(slot, name_id_u16))
    }

    /// Emits `StoreLocal`, using wide variant for slots > 255.
    pub fn emit_store_local(&mut self, slot: u16) -> Result<(), CompileError> {
        if let Ok(s) = u8::try_from(slot) {
            self.emit_u8(Opcode::StoreLocal, s)
        } else {
            self.emit_u16(Opcode::StoreLocalW, slot)
        }
    }

    /// Adds a constant to the pool, returning its index.
    ///
    /// Returns a `CompileError` if the constant pool would exceed `u16::MAX`
    /// entries — `LoadConst` and related opcodes encode the index in `u16`,
    /// so any function with more than `65 536` distinct constants exceeds the
    /// bytecode format.
    pub fn add_const(&mut self, value: Value) -> Result<u16, CompileError> {
        let idx_u16 = u16::try_from(self.constants.len()).map_err(|_| self.constant_pool_full())?;
        self.constants.push(value);
        Ok(idx_u16)
    }

    /// Adds an exception handler entry built from the given region bounds.
    ///
    /// Entries should be added in innermost-first order for nested try blocks.
    /// Returns a `CompileError` if any of the offsets exceeds the `u32` cap
    /// in `ExceptionEntry` — practically unreachable given the i16 jump
    /// limit, but kept honest for the same reason as `bytecode_too_large`.
    pub fn add_exception_entry(
        &mut self,
        start: Offset,
        end: Offset,
        handler: Offset,
        stack_depth: u16,
        exception_stack_count: u16,
    ) -> Result<(), CompileError> {
        let start = start.as_u32().ok_or_else(|| self.bytecode_too_large())?;
        let end = end.as_u32().ok_or_else(|| self.bytecode_too_large())?;
        let handler = handler.as_u32().ok_or_else(|| self.bytecode_too_large())?;
        let entry = ExceptionEntry::new(start, end, handler, stack_depth, exception_stack_count);
        self.exception_table.push(entry);
        Ok(())
    }

    /// Returns the current stack depth, or `None` if not currently emitting a code region.
    #[must_use]
    pub fn stack_depth(&self) -> Option<u16> {
        self.current_stack_depth
    }

    /// Reports whether the tracker is in the dead-code state.
    ///
    /// Used by compile_block to stop emitting after a terminator and by emit
    /// helpers to decide whether to bother computing live target depths.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.current_stack_depth.is_none()
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
    ///
    /// Returns a `CompileError` if the bytecode offset has grown past
    /// `u32::MAX` — the i16 jump-offset cap means this is practically
    /// unreachable, but `LocationEntry`'s offset is `u32` so we surface the
    /// limit cleanly rather than panic.
    fn record_location(&mut self) -> Result<(), CompileError> {
        if let Some(range) = self.current_location {
            let offset = u32::try_from(self.bytecode.len()).map_err(|_| self.bytecode_too_large())?;
            self.location_table
                .push(LocationEntry::new(offset, range, self.current_focus));
        }
        Ok(())
    }

    /// Opens a new code region at the given stack depth.
    ///
    /// Use this:
    /// - After `CodeBuilder::new()`, with `depth = 0`, to start the initial
    ///   region (the top of a function or module body).
    /// - For points reached via the exception table (handler entries with the
    ///   exception on stack at `base + 1`, finally cleanup) where the depth
    ///   comes from outside the fall-through graph.
    ///
    /// # Panics
    ///
    /// Panics if the builder is currently emitting live code.
    pub fn new_code_region(&mut self, depth: u16) {
        match self.current_stack_depth {
            Some(d) => {
                panic!("enter_region: cannot start new code region at depth {depth} while currently at live depth {d}")
            }
            None => self.current_stack_depth = Some(depth),
        }
        self.max_stack_depth = self.max_stack_depth.max(depth);
    }

    /// Adjusts the stack depth by the given delta.
    ///
    /// Positive values indicate pushes, negative values indicate pops.
    /// In the dead-code state this is a no-op: dead code can be emitted
    /// freely.
    fn adjust_stack(&mut self, delta: i16) -> Result<(), CompileError> {
        let Some(depth) = self.current_stack_depth else {
            return Ok(());
        };
        let new_depth = i32::from(depth) + i32::from(delta);
        // Stack depth shouldn't go negative (indicates compiler bug)
        debug_assert!(new_depth >= 0, "Stack depth went negative: {new_depth}");
        let new_depth = u16::try_from(new_depth.max(0)).map_err(|_| self.stack_too_large())?;
        self.current_stack_depth = Some(new_depth);
        self.max_stack_depth = self.max_stack_depth.max(new_depth);
        Ok(())
    }

    /// Single-source-of-truth emit path for all bytecode emission: records
    /// location, writes opcode + operand bytes, applies the stack-effect
    /// computed by `Opcode::stack_effect`, and transitions the tracker to
    /// dead for unconditional terminators (`Jump`, `ReturnValue`, `Raise`,
    /// `Reraise`).
    ///
    /// In the dead-code state the method silently no-ops. This lets the
    /// compiler drive emission uniformly without gating individual emits
    /// on reachability — dead trailers, epilogues, and orphaned cleanup
    /// pairs all vanish naturally.
    ///
    /// `emit_jump` and `emit_jump_to` route their byte emission through here;
    /// `emit_jump` additionally captures the pre-emit offset and computes the
    /// jump-taken target depth for the returned `JumpLabel`.
    fn emit_with_operand(&mut self, op: Opcode, operand: Operand<'_>) -> Result<(), CompileError> {
        if self.is_dead() {
            return Ok(());
        }
        self.record_location()?;
        self.bytecode.push(op as u8);
        match operand {
            Operand::None => {}
            Operand::U8(b) => self.bytecode.push(b),
            Operand::I8(b) => self.bytecode.push(b.to_ne_bytes()[0]),
            Operand::U16(w) => self.bytecode.extend(w.to_le_bytes()),
            Operand::Offset(relative) => self.bytecode.extend(relative.0.to_le_bytes()),
            Operand::U8U8(a, b) => {
                self.bytecode.push(a);
                self.bytecode.push(b);
            }
            Operand::U8U16(a, w) => {
                self.bytecode.push(a);
                self.bytecode.extend(w.to_le_bytes());
            }
            Operand::U16U8(w, b) => {
                self.bytecode.extend(w.to_le_bytes());
                self.bytecode.push(b);
            }
            Operand::U16U16(w1, w2) => {
                self.bytecode.extend(w1.to_le_bytes());
                self.bytecode.extend(w2.to_le_bytes());
            }
            Operand::U16U8U8(w, b1, b2) => {
                self.bytecode.extend(w.to_le_bytes());
                self.bytecode.push(b1);
                self.bytecode.push(b2);
            }
            Operand::CallKw { pos_count, kwname_ids } => {
                let kw_count = u8::try_from(kwname_ids.len()).map_err(|_| self.kw_count_too_large())?;
                self.bytecode.push(pos_count);
                self.bytecode.push(kw_count);
                for &name_id in kwname_ids {
                    self.bytecode.extend(name_id.to_le_bytes());
                }
            }
            Operand::CallAttrKw {
                attr_name_id,
                pos_count,
                kwname_ids,
            } => {
                let kw_count = u8::try_from(kwname_ids.len()).map_err(|_| self.kw_count_too_large())?;
                self.bytecode.extend(attr_name_id.to_le_bytes());
                self.bytecode.push(pos_count);
                self.bytecode.push(kw_count);
                for &name_id in kwname_ids {
                    self.bytecode.extend(name_id.to_le_bytes());
                }
            }
        }
        self.adjust_stack(op.stack_effect(operand))?;
        if matches!(
            op,
            Opcode::ReturnValue
                | Opcode::Raise
                | Opcode::Reraise
                | Opcode::RaiseImportError
                | Opcode::RaiseUnboundLocal
                | Opcode::Jump
        ) {
            self.current_stack_depth = None;
        }

        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn jump_too_large(&self) -> CompileError {
        jump_too_large_at(self.current_location.unwrap_or_default())
    }

    /// Builds the `CompileError` for a `StringId` that doesn't fit in the
    /// `u16` operand of a name-bearing opcode. Used by emit helpers that
    /// inline the name id directly (e.g. `LoadLocalCallable`).
    ///
    /// The count is `u16::MAX + 1` (`65 536`) because a `u16` operand can
    /// address indices `0..=u16::MAX`, so the format can name that many
    /// distinct interned strings before overflowing.
    #[cold]
    #[inline(never)]
    fn name_id_too_large(&self) -> CompileError {
        CompileError::new(
            format!(
                "module has too many distinct names; the bytecode format supports up to {} interned strings",
                usize::from(u16::MAX) + 1,
            ),
            self.current_location.unwrap_or_default(),
        )
    }

    /// Builds the `CompileError` for a `CallFunctionKw`/`CallAttrKw` keyword
    /// count that doesn't fit in `u8`. Anchored to the builder's current
    /// location (the call expression).
    #[cold]
    #[inline(never)]
    fn kw_count_too_large(&self) -> CompileError {
        CompileError::new(
            format!("call has too many keyword arguments; maximum is {} per call", u8::MAX),
            self.current_location.unwrap_or_default(),
        )
    }

    /// Builds the `CompileError` for a `add_const` that would overflow the
    /// per-`Code` constant pool's `u16` index. One function/module body can
    /// hold at most `u16::MAX + 1` distinct constants (the check fires when
    /// the pool already holds `u16::MAX + 1` entries — indices `0..=u16::MAX`).
    #[cold]
    #[inline(never)]
    fn constant_pool_full(&self) -> CompileError {
        CompileError::new(
            format!(
                "function has too many constants; maximum is {} per function",
                usize::from(u16::MAX) + 1,
            ),
            self.current_location.unwrap_or_default(),
        )
    }

    /// Builds the `CompileError` for an operand-stack depth or `emit_jump`
    /// target depth that doesn't fit in `u16`. The same message is used by
    /// `adjust_stack`, `emit_jump`, and `emit_jump_to` so the user sees one
    /// consistent diagnostic for "too much stuff pushed" regardless of which
    /// path detects it.
    #[cold]
    #[inline(never)]
    fn stack_too_large(&self) -> CompileError {
        CompileError::new(
            "function too large: required stack exceeds u16::MAX",
            self.current_location.unwrap_or_default(),
        )
    }

    /// Builds the `CompileError` for a `record_location` whose bytecode
    /// position doesn't fit in the `u32` field of `LocationEntry`. Practically
    /// unreachable because the `i16` jump-offset cap kicks in at ~32 KB of
    /// bytecode, but kept defensive in case future opcodes loosen that limit.
    /// The count is `u32::MAX + 1` because the check happens on the pre-push
    /// length and the next byte makes it that many bytes total.
    #[cold]
    #[inline(never)]
    fn bytecode_too_large(&self) -> CompileError {
        CompileError::new(
            format!(
                "function bytecode too large; maximum is {} bytes",
                u64::from(u32::MAX) + 1,
            ),
            self.current_location.unwrap_or_default(),
        )
    }
}

/// Builds the `CompileError` for a jump offset that doesn't fit in `i16`,
/// anchored to an explicit source position rather than the builder's current
/// location. Used by `patch_jump` so a forward-jump overflow is reported at
/// the original `emit_jump` site (captured in `JumpLabelInner::source_position`)
/// rather than the unrelated statement where the patch happens to resolve.
#[cold]
#[inline(never)]
fn jump_too_large_at(position: CodeRange) -> CompileError {
    CompileError::new("function too large: jump offset exceeds i16 range", position)
}

/// Label for a forward jump that needs patching.
#[derive(Debug, Clone, Copy)]
pub struct JumpLabel {
    /// `Option` is none to allow for the dead code case, in which case
    /// the jump is unreachable and patch_jump needs do no work.
    inner: Option<JumpLabelInner>,
}

#[derive(Debug, Clone, Copy)]
struct JumpLabelInner {
    /// Position of the jump's opcode byte. `patch_jump` writes the relative
    /// i16 at `offset.0 + 1`.
    offset: Offset,
    /// The stack depth that the jump-taken path leaves on the stack
    /// when the jump is taken to the target. Used in `calculate_jump_offset`
    /// to enforce the invariant that all paths arriving at a given bytecode
    /// position have the same stack depth.
    stack_depth: u16,
    /// Source location of the jump itself (the statement that emitted it),
    /// captured at `emit_jump` time. Used by `patch_jump` so an offset
    /// overflow anchors the diagnostic at the jump site rather than at the
    /// patch site, which is usually a different (and less informative)
    /// statement.
    source_position: CodeRange,
}

/// A position in the bytecode stream.
///
/// Returned by `CodeBuilder::current_offset` and consumed by `emit_jump_to`
/// (as a backward-jump target) and `ExceptionEntry::new` (as the bounds of
/// try/except/finally regions). The wrapped `usize` is intentionally private:
/// `Offset` values can only originate from the builder, which prevents
/// arbitrary integers from being used in places where the bytecode position
/// is a load-bearing invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offset(usize);

impl Offset {
    /// Returns the offset as a `u32` — the serialized form used by
    /// `ExceptionEntry` and `LocationEntry` — or `None` if the bytecode
    /// position exceeds `u32::MAX`.
    ///
    /// Reachable only with > 4 GB of generated bytecode in a single function,
    /// which the `i16` jump-offset cap already prevents in practice. Returns
    /// `Option` rather than panicking so the rare overflow surfaces through
    /// the caller's `CompileError` path (see `CodeBuilder::bytecode_too_large`)
    /// alongside the other limit failures.
    #[must_use]
    pub fn as_u32(self) -> Option<u32> {
        u32::try_from(self.0).ok()
    }
}

/// Relative offset used as jump operand.
///
/// Jumps are computed as per x86 convention: the offset is relative to the position
/// immediately after the jump instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelativeOffset(i16);

impl RelativeOffset {
    #[must_use]
    pub fn as_i16(self) -> i16 {
        self.0
    }
}

/// Calculate the jump offset from a jump instruction at `from` to a target at `to`.
///
/// Returns `None` if the offset doesn't fit in `i16` — the caller (always
/// `&mut self` on the builder) converts that into a `CompileError` anchored
/// to the current source location via `jump_too_large`.
fn calculate_jump_offset(from: JumpLabelInner, to: JumpTargetInner) -> Option<RelativeOffset> {
    // All jumps are currently 3 byte instructions: opcode + i16 offset
    const JUMP_BYTECODE_SIZE: usize = size_of::<Opcode>() + size_of::<RelativeOffset>();

    // Jumps are calculated from after the jump instruction; the label is the position of the jump itself
    let from_i64 = i64::try_from(from.offset.0 + JUMP_BYTECODE_SIZE).expect("bytecode offset exceeds i64");
    let to_i64 = i64::try_from(to.offset.0).expect("bytecode offset exceeds i64");

    // stack depth must match at merge point - if this fails, it indicates the builder
    // is not tracking stack effect correctly for some instructions
    debug_assert_eq!(
        from.stack_depth, to.stack_depth,
        "jump merge: arriving with depth {} but jump target has depth {}",
        from.stack_depth, to.stack_depth,
    );

    let raw_offset = to_i64 - from_i64;
    i16::try_from(raw_offset).ok().map(RelativeOffset)
}

/// Target for a backward jump.
#[derive(Debug, Clone, Copy)]
pub struct JumpTarget(Option<JumpTargetInner>);

#[derive(Debug, Clone, Copy)]
struct JumpTargetInner {
    offset: Offset,
    /// The stack depth that at this position. Used in `calculate_jump_offset`
    /// to enforce the invariant that all paths arriving at a given bytecode.
    stack_depth: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_basic() {
        let mut builder = CodeBuilder::new();
        builder.new_code_region(0);
        builder.emit(Opcode::LoadNone).unwrap();
        builder.emit(Opcode::Pop).unwrap();

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadNone as u8, Opcode::Pop as u8]);
    }

    #[test]
    fn test_emit_u8_operand() {
        let mut builder = CodeBuilder::new();
        builder.new_code_region(0);
        builder.emit_u8(Opcode::LoadLocal, 42).unwrap();

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadLocal as u8, 42]);
    }

    #[test]
    fn test_emit_u16_operand() {
        let mut builder = CodeBuilder::new();
        builder.new_code_region(0);
        builder.emit_u16(Opcode::LoadConst, 0x1234).unwrap();

        let code = builder.build(0);
        assert_eq!(code.bytecode(), &[Opcode::LoadConst as u8, 0x34, 0x12]);
    }

    #[test]
    fn test_forward_jump() {
        let mut builder = CodeBuilder::new();
        builder.new_code_region(0);
        let jump = builder.emit_jump(Opcode::Jump).unwrap();
        builder.new_code_region(0);
        builder.emit(Opcode::LoadNone).unwrap();
        builder.emit(Opcode::Pop).unwrap();
        builder.patch_jump(jump).unwrap();
        builder.emit(Opcode::LoadNone).unwrap(); // Return value
        builder.emit(Opcode::ReturnValue).unwrap();

        let code = builder.build(0);
        assert_eq!(
            code.bytecode(),
            &[
                Opcode::Jump as u8,
                2i16.to_le_bytes()[0],
                2i16.to_le_bytes()[1], // 2 bytes to jump from the end of the jump instruction to the LoadNone after the patch
                Opcode::LoadNone as u8,
                Opcode::Pop as u8,
                Opcode::LoadNone as u8,
                Opcode::ReturnValue as u8,
            ]
        );
    }

    #[test]
    fn test_backward_jump() {
        let mut builder = CodeBuilder::new();
        builder.new_code_region(0);
        let loop_start = builder.current_jump_target();
        builder.emit(Opcode::LoadNone).unwrap(); // offset 0, 1 byte
        builder.emit(Opcode::Pop).unwrap(); // offset 1, 1 byte
        builder.emit_jump_to(Opcode::Jump, loop_start).unwrap(); // offset 2, target 0

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
        builder.new_code_region(0);
        builder.emit_load_local(0).unwrap();
        builder.emit_load_local(1).unwrap();
        builder.emit_load_local(2).unwrap();
        builder.emit_load_local(3).unwrap();
        builder.emit_load_local(4).unwrap();
        builder.emit_load_local(256).unwrap();

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
        builder.new_code_region(0);
        let idx1 = builder.add_const(Value::Int(42)).unwrap();
        let idx2 = builder.add_const(Value::None).unwrap();

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
    }
}
