//! Compilation of `match` statements (PEP 634) to bytecode.
//!
//! The lowering follows CPython's `codegen_pattern_*` family so that capture
//! bindings only happen once the whole pattern has matched: captured values
//! are kept on the operand stack beneath the pattern's temporaries and stored
//! after the pattern succeeds, just before the guard runs. A failing pattern
//! jumps to a per-depth cleanup block that pops its partial state, then falls
//! through to the next `case`.

use std::mem;

use super::{
    builder::JumpLabel,
    compiler::{CompileError, Compiler},
    op::{MATCH_KEYS_REST, MATCH_KEYS_VALUES, MATCH_SHAPE_MAPPING, MATCH_SHAPE_MIN_LEN, Opcode},
};
use crate::{
    expressions::{ExprLoc, Identifier, MatchCase, Pattern, PreparedNode},
    intern::StringId,
    parse::CodeRange,
    value::Value,
};

/// Per-pattern compilation state, mirroring CPython's `pattern_context`.
///
/// Stack layout while a pattern runs, top last:
/// `[..., newest capture, ..., oldest capture, on_top temporaries]`.
/// A new capture is sunk beneath both regions so the final stores pop the
/// oldest capture first; a failure jumps to `fail_pop[on_top + stores.len()]`.
#[derive(Default)]
struct PatternContext {
    /// Names captured so far, in binding order.
    stores: Vec<Identifier>,
    /// Whether an irrefutable pattern (capture or wildcard) may appear here:
    /// only in the last case, in a guarded case, or as a sub-pattern.
    allow_irrefutable: bool,
    /// Jumps to the failure path, indexed by how many stack items the jump
    /// site has above the case's base depth.
    fail_pop: Vec<Vec<JumpLabel>>,
    /// Temporaries the pattern currently holds above its captures.
    on_top: usize,
}

impl PatternContext {
    /// Starts the context for one `case`, or one alternative of an or-pattern.
    fn new(allow_irrefutable: bool) -> Self {
        Self {
            allow_irrefutable,
            ..Self::default()
        }
    }

    /// Records `label` as a jump to the failure path from the current depth.
    fn fail_at(&mut self, pops: usize, label: JumpLabel) {
        if self.fail_pop.len() <= pops {
            self.fail_pop.resize_with(pops + 1, Vec::new);
        }
        self.fail_pop[pops].push(label);
    }

    /// Whether `name` is already captured by this pattern.
    fn captures(&self, name: &Identifier) -> bool {
        self.stores.iter().any(|stored| stored.name_id == name.name_id)
    }
}

impl<'a> Compiler<'a, '_> {
    /// Compiles `match SUBJECT: case ...` (CPython's `codegen_match_inner`).
    ///
    /// The subject is evaluated once. Every case but the last duplicates it
    /// before matching, because a pattern consumes the value it is matched
    /// against on both the success and the failure path. A trailing bare `_`
    /// case is compiled as plain fall-through code.
    pub(super) fn compile_match(
        &mut self,
        subject: &ExprLoc,
        cases: &'a [MatchCase<PreparedNode>],
        position: CodeRange,
    ) -> Result<(), CompileError> {
        self.compile_expr(subject)?;
        let has_default = cases.len() > 1 && cases.last().is_some_and(|case| case.pattern.is_wildcard());
        let tested = cases.len() - usize::from(has_default);
        let mut end_jumps = Vec::with_capacity(cases.len());
        for (i, case) in cases[..tested].iter().enumerate() {
            let is_last_tested = i == tested - 1;
            self.code.set_location(case.position, None);
            if !is_last_tested {
                self.code.emit(Opcode::Dup)?;
            }
            let allow_irrefutable = case.guard.is_some() || i == cases.len() - 1;
            let mut pc = PatternContext::new(allow_irrefutable);
            self.compile_pattern(&case.pattern, &mut pc)?;
            debug_assert_eq!(pc.on_top, 0, "pattern left temporaries on the stack");
            // It's a match: bind the captured names (oldest capture is on top).
            for name in &pc.stores {
                self.compile_store(name)?;
            }
            if let Some(guard) = &case.guard {
                self.compile_expr(guard)?;
                let label = self.code.emit_jump(Opcode::JumpIfFalse)?;
                pc.fail_at(0, label);
            }
            if !is_last_tested {
                // Done with this case's copy of the subject.
                self.code.set_location(case.position, None);
                self.code.emit(Opcode::Pop)?;
            }
            self.compile_block(&case.body)?;
            end_jumps.push(self.code.emit_jump(Opcode::Jump)?);
            self.code.set_location(case.position, None);
            self.emit_fail_pops(&mut pc)?;
        }
        if has_default {
            // The last tested case consumed the subject, so `case _:` is plain code.
            let case = &cases[tested];
            self.code.set_location(case.position, None);
            if let Some(guard) = &case.guard {
                self.compile_expr(guard)?;
                end_jumps.push(self.code.emit_jump(Opcode::JumpIfFalse)?);
            }
            self.compile_block(&case.body)?;
        }
        self.code.set_location(position, None);
        for jump in end_jumps {
            self.code.patch_jump(jump)?;
        }
        Ok(())
    }

    /// Compiles one pattern against the value on top of the stack, consuming
    /// it on every path. Dispatches on the pattern shape (`codegen_pattern`).
    fn compile_pattern(&mut self, pattern: &Pattern, pc: &mut PatternContext) -> Result<(), CompileError> {
        let position = pattern.position();
        self.code.set_location(position, None);
        match pattern {
            Pattern::Value(value) => {
                self.compile_expr(value)?;
                self.code.set_location(position, None);
                self.code.emit(Opcode::CompareEq)?;
                self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)
            }
            Pattern::Singleton(value) => {
                self.compile_expr(value)?;
                self.code.set_location(position, None);
                self.code.emit(Opcode::CompareIs)?;
                self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)
            }
            Pattern::As {
                pattern: None, name, ..
            } => {
                if !pc.allow_irrefutable {
                    let message = match name {
                        Some(name) => format!(
                            "name capture '{}' makes remaining patterns unreachable",
                            self.interns.get_str(name.name_id)
                        ),
                        None => "wildcard makes remaining patterns unreachable".to_owned(),
                    };
                    return Err(CompileError::new(message, position));
                }
                self.pattern_store_name(name.as_ref(), position, pc)
            }
            Pattern::As {
                pattern: Some(inner),
                name,
                ..
            } => {
                // Keep a copy to bind once the inner pattern has matched.
                pc.on_top += 1;
                self.code.emit(Opcode::Dup)?;
                self.compile_pattern(inner, pc)?;
                pc.on_top -= 1;
                self.pattern_store_name(name.as_ref(), position, pc)
            }
            // Only reached through sequence unpacking, which has already built the list.
            Pattern::Star { name, .. } => self.pattern_store_name(name.as_ref(), position, pc),
            Pattern::Sequence { patterns, .. } => self.compile_pattern_sequence(patterns, position, pc),
            Pattern::Mapping {
                keys, patterns, rest, ..
            } => self.compile_pattern_mapping(keys, patterns, rest.as_ref(), position, pc),
            Pattern::Class {
                cls,
                patterns,
                kwd_attrs,
                kwd_patterns,
                ..
            } => self.compile_pattern_class(cls, patterns, kwd_attrs, kwd_patterns, position, pc),
            Pattern::Or { patterns, .. } => self.compile_pattern_or(patterns, position, pc),
        }
    }

    /// Compiles a nested pattern, where irrefutable shapes are always allowed.
    fn compile_subpattern(&mut self, pattern: &Pattern, pc: &mut PatternContext) -> Result<(), CompileError> {
        let allow_irrefutable = mem::replace(&mut pc.allow_irrefutable, true);
        let result = self.compile_pattern(pattern, pc);
        pc.allow_irrefutable = allow_irrefutable;
        result
    }

    /// `[a, *rest, b]`: shape and length test, then per-item sub-patterns.
    fn compile_pattern_sequence(
        &mut self,
        patterns: &[Pattern],
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let size = patterns.len();
        let star = patterns.iter().position(|p| matches!(p, Pattern::Star { .. }));
        let only_wildcards = patterns.iter().all(|p| p.is_wildcard() || p.is_star_wildcard());
        let star_is_wildcard = star.is_some_and(|i| patterns[i].is_star_wildcard());
        // The subject stays on top during the shape check.
        pc.on_top += 1;
        let (length, flags) = match star {
            Some(_) => (size - 1, MATCH_SHAPE_MIN_LEN),
            None => (size, 0),
        };
        let length = pattern_count_u16(length, "sequence", position)?;
        self.code.emit_u16_u8(Opcode::MatchShape, length, flags)?;
        self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)?;
        // Whatever comes next consumes the subject.
        pc.on_top -= 1;
        if only_wildcards {
            // `[]`, `[_, _]`, `[_, *_]`: the length check was the whole test.
            self.code.emit(Opcode::Pop)
        } else if star_is_wildcard {
            self.compile_sequence_subscr(patterns, star, position, pc)
        } else {
            self.compile_sequence_unpack(patterns, star, position, pc)
        }
    }

    /// Unpacks the whole sequence (`UnpackSequence` / `UnpackEx`) and matches
    /// each item, so a named star capture gets its list for free.
    fn compile_sequence_unpack(
        &mut self,
        patterns: &[Pattern],
        star: Option<usize>,
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let size = patterns.len();
        if let Some(star) = star {
            let before = pattern_count_u8(star, "sequence", position)?;
            let after = pattern_count_u8(size - star - 1, "sequence", position)?;
            self.code.emit_u8_u8(Opcode::UnpackEx, before, after)?;
        } else {
            let count = pattern_count_u8(size, "sequence", position)?;
            self.code.emit_u8(Opcode::UnpackSequence, count)?;
        }
        // The first item is on top; each sub-pattern consumes one.
        pc.on_top += size;
        for pattern in patterns {
            pc.on_top -= 1;
            self.compile_subpattern(pattern, pc)?;
        }
        Ok(())
    }

    /// Indexes the items individually when the star is `*_`, so the surplus
    /// items are never collected. Items after the star use negative indexes.
    fn compile_sequence_subscr(
        &mut self,
        patterns: &[Pattern],
        star: Option<usize>,
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let size = patterns.len();
        // Keep the subject while indexing into it.
        pc.on_top += 1;
        for (i, pattern) in patterns.iter().enumerate() {
            if pattern.is_wildcard() || Some(i) == star {
                continue;
            }
            self.code.set_location(pattern.position(), None);
            self.code.emit(Opcode::Dup)?;
            let index = if star.is_some_and(|star| i > star) {
                i64::try_from(i).map_err(|_| too_many_subpatterns("sequence", position))?
                    - i64::try_from(size).map_err(|_| too_many_subpatterns("sequence", position))?
            } else {
                i64::try_from(i).map_err(|_| too_many_subpatterns("sequence", position))?
            };
            self.compile_int_constant(index)?;
            self.code.emit(Opcode::BinarySubscr)?;
            self.compile_subpattern(pattern, pc)?;
        }
        pc.on_top -= 1;
        self.code.set_location(position, None);
        self.code.emit(Opcode::Pop)
    }

    /// `{'k': v, **rest}`: mapping test, key lookup via `MatchKeys`, then the
    /// value sub-patterns and the optional `**rest` dict.
    fn compile_pattern_mapping(
        &mut self,
        keys: &[ExprLoc],
        patterns: &[Pattern],
        rest: Option<&Identifier>,
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let size = keys.len();
        pc.on_top += 1;
        let length = pattern_count_u16(size, "mapping", position)?;
        self.code
            .emit_u16_u8(Opcode::MatchShape, length, MATCH_SHAPE_MAPPING | MATCH_SHAPE_MIN_LEN)?;
        self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)?;
        if size == 0 && rest.is_none() {
            // `{}` matches any mapping: done with the subject.
            pc.on_top -= 1;
            return self.code.emit(Opcode::Pop);
        }
        for key in keys {
            self.compile_expr(key)?;
        }
        self.code.set_location(position, None);
        self.code.emit_u16(Opcode::BuildTuple, length)?;
        self.code.emit_u8(Opcode::MatchKeys, MATCH_KEYS_VALUES)?;
        // The keys tuple and the values tuple (or None) now sit above the subject.
        pc.on_top += 2;
        self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)?;
        self.unpack_subpattern_values(size, pc)?;
        for pattern in patterns {
            pc.on_top -= 1;
            self.compile_subpattern(pattern, pc)?;
        }
        // Success: consume the keys tuple and the subject.
        pc.on_top -= 2;
        self.code.set_location(position, None);
        if let Some(rest) = rest {
            self.code.emit_u8(Opcode::MatchKeys, MATCH_KEYS_REST)?;
            self.pattern_store_name(Some(rest), position, pc)
        } else {
            self.code.emit(Opcode::Pop)?;
            self.code.emit(Opcode::Pop)
        }
    }

    /// `Point(0, y=1)`: `MatchClass` checks the instance and resolves the
    /// attribute names (`__match_args__` then keywords); one `MatchAttr` per
    /// sub-pattern then reads its attribute, so a host-side read can suspend.
    /// The class stays on the stack so `MatchAttr` can name it in errors.
    fn compile_pattern_class(
        &mut self,
        cls: &ExprLoc,
        patterns: &[Pattern],
        kwd_attrs: &[StringId],
        kwd_patterns: &[Pattern],
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let nargs = pattern_count_u8(patterns.len(), "class", position)?;
        pattern_count_u8(patterns.len() + kwd_patterns.len(), "class", position)?;
        let nattrs = pattern_count_u16(kwd_attrs.len(), "class", position)?;
        self.compile_expr(cls)?;
        for attr in kwd_attrs {
            let idx = self.code.add_const(Value::InternString(*attr))?;
            self.code.emit_u16(Opcode::LoadConst, idx)?;
        }
        self.code.set_location(position, None);
        self.code.emit_u16(Opcode::BuildTuple, nattrs)?;
        self.code.emit_u8(Opcode::MatchClass, nargs)?;
        // The subject and class stay, with the names tuple (or None) above them.
        pc.on_top += 3;
        self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)?;
        for (index, pattern) in patterns.iter().chain(kwd_patterns).enumerate() {
            self.code.set_location(position, None);
            self.code
                .emit_u8(Opcode::MatchAttr, pattern_count_u8(index, "class", position)?)?;
            pc.on_top += 1;
            self.jump_to_fail_pop(pc, Opcode::JumpIfFalse)?;
            pc.on_top -= 1;
            self.compile_subpattern(pattern, pc)?;
        }
        // Success: drop the names tuple, the class and the subject.
        pc.on_top -= 3;
        self.code.set_location(position, None);
        self.code.emit(Opcode::Pop)?;
        self.code.emit(Opcode::Pop)?;
        self.code.emit(Opcode::Pop)
    }

    /// Replaces the mapping values tuple on top of the stack with its `count`
    /// items, first item on top, adjusting `on_top` so each sub-pattern consumes one.
    fn unpack_subpattern_values(&mut self, count: usize, pc: &mut PatternContext) -> Result<(), CompileError> {
        if count == 0 {
            self.code.emit(Opcode::Pop)?;
        } else {
            let count_u8 = pattern_count_u8(count, "mapping", self.code.current_position())?;
            self.code.emit_u8(Opcode::UnpackSequence, count_u8)?;
        }
        pc.on_top += count;
        pc.on_top -= 1;
        Ok(())
    }

    /// `A | B | C`: each alternative runs against a copy of the subject with
    /// its own context; all must bind the same names, which are reordered on
    /// the stack to the first alternative's order (`codegen_pattern_or`).
    fn compile_pattern_or(
        &mut self,
        patterns: &[Pattern],
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let outer_stores = mem::take(&mut pc.stores);
        let outer_allow_irrefutable = pc.allow_irrefutable;
        let outer_fail_pop = mem::take(&mut pc.fail_pop);
        let outer_on_top = pc.on_top;
        // The names bound by the first alternative, which the others must match.
        let mut control: Option<Vec<Identifier>> = None;
        let mut end_jumps = Vec::with_capacity(patterns.len());
        for (i, alternative) in patterns.iter().enumerate() {
            *pc = PatternContext::new(i == patterns.len() - 1 && outer_allow_irrefutable);
            self.code.set_location(alternative.position(), None);
            self.code.emit(Opcode::Dup)?;
            self.compile_pattern(alternative, pc)?;
            match &control {
                None => control = Some(pc.stores.clone()),
                Some(control) => self.reorder_alternative_stores(control, position, pc)?,
            }
            end_jumps.push(self.code.emit_jump(Opcode::Jump)?);
            self.emit_fail_pops(pc)?;
        }
        pc.stores = outer_stores;
        pc.allow_irrefutable = outer_allow_irrefutable;
        pc.fail_pop = outer_fail_pop;
        pc.on_top = outer_on_top;
        // No alternative matched: discard the remaining subject and fail.
        self.code.set_location(position, None);
        self.code.emit(Opcode::Pop)?;
        self.jump_to_fail_pop(pc, Opcode::Jump)?;
        for jump in end_jumps {
            self.code.patch_jump(jump)?;
        }
        // Sink the alternative's captures beneath the outer captures, the outer
        // temporaries and the subject copy, then drop that copy.
        let control = control.unwrap_or_default();
        let depth = control.len() + pc.on_top + pc.stores.len();
        for name in control {
            self.emit_sink_top(depth, position)?;
            if pc.captures(&name) {
                return Err(duplicate_capture(self.interns.get_str(name.name_id), position));
            }
            pc.stores.push(name);
        }
        self.code.emit(Opcode::Pop)
    }

    /// Permutes an alternative's captures on the stack into `control`'s order,
    /// or reports that the alternatives bind different names.
    fn reorder_alternative_stores(
        &mut self,
        control: &[Identifier],
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        if pc.stores.len() != control.len() {
            return Err(CompileError::new("alternative patterns bind different names", position));
        }
        // Work from the deepest slot up: once `stores[icontrol..]` agrees with
        // `control[icontrol..]`, rotating the top `icontrol + 1` items leaves it alone.
        for icontrol in (0..control.len()).rev() {
            let Some(istores) = pc.stores.iter().position(|s| s.name_id == control[icontrol].name_id) else {
                return Err(CompileError::new("alternative patterns bind different names", position));
            };
            if istores != icontrol {
                debug_assert!(istores < icontrol);
                let rotations = istores + 1;
                let rotated: Vec<Identifier> = pc.stores.drain(..rotations).collect();
                let at = icontrol - istores;
                pc.stores.splice(at..at, rotated);
                for _ in 0..rotations {
                    self.emit_sink_top(icontrol, position)?;
                }
            }
        }
        Ok(())
    }

    /// Binds (or, for `_`, discards) the value on top of the stack as a capture:
    /// it is sunk beneath the pattern's temporaries and earlier captures so the
    /// stores can be emitted in order once the whole pattern matches.
    fn pattern_store_name(
        &mut self,
        name: Option<&Identifier>,
        position: CodeRange,
        pc: &mut PatternContext,
    ) -> Result<(), CompileError> {
        let Some(name) = name else {
            return self.code.emit(Opcode::Pop);
        };
        if pc.captures(name) {
            return Err(duplicate_capture(self.interns.get_str(name.name_id), position));
        }
        self.emit_sink_top(pc.on_top + pc.stores.len(), position)?;
        pc.stores.push(*name);
        Ok(())
    }

    /// Moves the top of the stack beneath the `depth` items below it.
    fn emit_sink_top(&mut self, depth: usize, position: CodeRange) -> Result<(), CompileError> {
        let depth_u8 = u8::try_from(depth).map_err(|_| CompileError::new("too many names in pattern", position))?;
        if depth == 0 {
            Ok(())
        } else {
            self.code.emit_u8(Opcode::SinkTop, depth_u8)
        }
    }

    /// Emits a jump to the failure path from the current pattern depth.
    fn jump_to_fail_pop(&mut self, pc: &mut PatternContext, op: Opcode) -> Result<(), CompileError> {
        let pops = pc.on_top + pc.stores.len();
        let label = self.code.emit_jump(op)?;
        pc.fail_at(pops, label);
        Ok(())
    }

    /// Emits the failure path: one landing pad per depth, each popping one
    /// item and falling into the next, so every failed jump ends at the case's
    /// base depth (`emit_and_reset_fail_pop`).
    fn emit_fail_pops(&mut self, pc: &mut PatternContext) -> Result<(), CompileError> {
        let fail_pop = mem::take(&mut pc.fail_pop);
        let mut levels = fail_pop.into_iter().rev();
        let Some(deepest) = levels.next() else {
            return Ok(());
        };
        for label in deepest {
            self.code.patch_jump(label)?;
        }
        for labels in levels {
            self.code.emit(Opcode::Pop)?;
            for label in labels {
                self.code.patch_jump(label)?;
            }
        }
        Ok(())
    }

    /// Pushes an integer constant, using the compact small-int form when it fits.
    fn compile_int_constant(&mut self, value: i64) -> Result<(), CompileError> {
        if let Ok(small) = i8::try_from(value) {
            self.code.emit_i8(Opcode::LoadSmallInt, small)
        } else {
            let idx = self.code.add_const(Value::Int(value))?;
            self.code.emit_u16(Opcode::LoadConst, idx)
        }
    }
}

/// `SyntaxError: multiple assignments to name 'x' in pattern`.
fn duplicate_capture(name: &str, position: CodeRange) -> CompileError {
    CompileError::new(format!("multiple assignments to name '{name}' in pattern"), position)
}

/// Fits a sub-pattern count into a `u8` bytecode operand.
fn pattern_count_u8(count: usize, kind: &str, position: CodeRange) -> Result<u8, CompileError> {
    u8::try_from(count).map_err(|_| too_many_subpatterns(kind, position))
}

/// Fits a sub-pattern count into a `u16` bytecode operand.
fn pattern_count_u16(count: usize, kind: &str, position: CodeRange) -> Result<u16, CompileError> {
    u16::try_from(count).map_err(|_| too_many_subpatterns(kind, position))
}

/// The pattern has more parts than the bytecode operands can encode.
fn too_many_subpatterns(kind: &str, position: CodeRange) -> CompileError {
    CompileError::new(format!("too many sub-patterns in {kind} pattern"), position)
}
