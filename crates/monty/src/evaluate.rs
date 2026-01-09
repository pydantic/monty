use std::cmp::Ordering;

use crate::{
    args::{ArgExprs, ArgValues, Kwarg, KwargsValues},
    callable::Callable,
    exception_private::{exc_err_fmt, ExcType, RunError, SimpleException},
    expressions::{Expr, ExprLoc, NameScope},
    fstring::{fstring_interpolation, ConversionFlag, FStringPart},
    heap::{Heap, HeapData},
    intern::{Interns, StringId},
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces},
    operators::{CmpOperator, Operator},
    parse::CodeRange,
    resource::ResourceTracker,
    run_frame::RunResult,
    snapshot::{AbstractSnapshotTracker, ArgumentCache, ExternalCall},
    types::{Dict, List, PyTrait, Set, Str, Tuple},
    value::{Attr, Value},
};

/// Container for evaluation context that holds all state needed during expression evaluation.
///
/// This struct bundles together the namespaces, local namespace index, heap, and string storage
/// to avoid passing them as separate parameters to every evaluation function.
/// It simplifies function signatures and makes the evaluation code more readable.
///
/// # Lifetimes
/// * `'h` - Lifetime of the mutable borrows (namespaces and heap)
/// * `'s` - Lifetime of the string storage and the print print reference
///
/// # Type Parameters
/// * `T` - The resource tracker type for enforcing execution limits
/// * `W` - The print type for print output
/// * `P` - The snapshot tracker type for recording execution position
pub struct EvaluateExpr<'h, 's, T: ResourceTracker, W: PrintWriter, S: AbstractSnapshotTracker> {
    /// The namespace stack containing all scopes (global, local, etc.)
    pub namespaces: &'h mut Namespaces,
    /// Index of the current local namespace in the namespace stack
    pub local_idx: NamespaceId,
    /// The heap for allocating and managing heap-allocated objects
    pub heap: &'h mut Heap<T>,
    /// String storage for looking up interned names
    pub interns: &'s Interns,
    /// Writer for print output
    pub print: &'s mut W,
    /// Tracker for recording execution position for snapshot/resume
    pub snapshot_tracker: &'h mut S,
}

/// Similar to the legacy `ok!()` macro, this gives shorthand for returning early
/// when a function call result is found
macro_rules! return_ext_call {
    ($expr:expr) => {
        match $expr {
            EvalResult::Value(value) => value,
            EvalResult::ExternalCall(ext_call) => return Ok(EvalResult::ExternalCall(ext_call)),
        }
    };
}
pub(crate) use return_ext_call;

impl<'h, 's, T: ResourceTracker, W: PrintWriter, S: AbstractSnapshotTracker> EvaluateExpr<'h, 's, T, W, S> {
    /// Creates a new `EvaluateExpr` with the given evaluation context.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace stack containing all scopes
    /// * `local_idx` - Index of the current local namespace
    /// * `heap` - The heap for object allocation
    /// * `interns` - String storage for looking up interned names
    /// * `print` - The print for print output
    /// * `snapshot_tracker` - Tracker for recording execution position
    pub fn new(
        namespaces: &'h mut Namespaces,
        local_idx: NamespaceId,
        heap: &'h mut Heap<T>,
        interns: &'s Interns,
        print: &'s mut W,
        snapshot_tracker: &'h mut S,
    ) -> Self {
        Self {
            namespaces,
            local_idx,
            heap,
            interns,
            print,
            snapshot_tracker,
        }
    }

    /// Evaluates an expression node and returns a value.
    ///
    /// This is the primary evaluation method that recursively evaluates expressions
    /// and returns the resulting value. The returned value may be a heap reference
    /// that the caller is responsible for dropping via `drop_with_heap`.
    pub fn evaluate_use(&mut self, expr_loc: &ExprLoc) -> RunResult<EvalResult<Value>> {
        match &expr_loc.expr {
            Expr::Literal(literal) => Ok(EvalResult::Value((*literal).into())),
            Expr::Builtin(builtins) => Ok(EvalResult::Value(Value::Builtin(*builtins))),
            Expr::Name(ident) => self
                .namespaces
                .get_var_value(self.local_idx, self.heap, ident, self.interns)
                .map(EvalResult::Value),
            Expr::Call { callable, args } => {
                // Check for cached return value BEFORE evaluating arguments.
                // This is critical for resumption: if a function containing an external call
                // returned, its return value is cached with a specific position. When re-evaluating
                // the expression, we must skip argument evaluation entirely to avoid side effects
                // (like nested function calls that would clear the cached value).
                //
                // Only match exact positions here (not None). Cached values with None position are
                // for direct external calls and should be matched in callable.rs, not here.
                if let Some(cached) = self
                    .namespaces
                    .take_ext_return_value_exact(self.heap, expr_loc.position)?
                {
                    return Ok(EvalResult::Value(cached));
                }
                let args = return_ext_call!(self.evaluate_args_cached(args, Some(callable), expr_loc.position)?);
                callable.call(
                    self.namespaces,
                    self.local_idx,
                    self.heap,
                    args,
                    self.interns,
                    self.print,
                    self.snapshot_tracker,
                    expr_loc.position,
                )
            }
            Expr::AttrCall { object, attr, args } => self.attr_call(object, attr, args),
            Expr::AttrGet { object, attr } => self.attr_get(object, attr),
            Expr::Op { left, op, right } => match op {
                // Handle boolean operators with short-circuit evaluation.
                // These return the actual operand value, not a boolean.
                Operator::And => self.eval_and(left, right),
                Operator::Or => self.eval_or(left, right),
                _ => self.eval_op(left, op, right),
            },
            Expr::CmpOp { left, op, right } => {
                let b = return_ext_call!(self.cmp_op(left, op, right)?);
                Ok(EvalResult::Value(b.into()))
            }
            Expr::List(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for e in elements {
                    let v = return_ext_call!(self.evaluate_use(e)?);
                    values.push(v);
                }
                let heap_id = self.heap.allocate(HeapData::List(List::new(values)))?;
                Ok(EvalResult::Value(Value::Ref(heap_id)))
            }
            Expr::Tuple(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for e in elements {
                    let v = return_ext_call!(self.evaluate_use(e)?);
                    values.push(v);
                }
                let heap_id = self.heap.allocate(HeapData::Tuple(Tuple::new(values)))?;
                Ok(EvalResult::Value(Value::Ref(heap_id)))
            }
            Expr::Subscript { object, index } => {
                let obj = return_ext_call!(self.evaluate_use(object)?);
                // Handle external call in index evaluation - must drop obj before returning
                let key = match self.evaluate_use(index)? {
                    EvalResult::Value(v) => v,
                    EvalResult::ExternalCall(ext_call) => {
                        obj.drop_with_heap(self.heap);
                        return Ok(EvalResult::ExternalCall(ext_call));
                    }
                };
                let result = obj.py_getitem(&key, self.heap, self.interns);
                // Drop temporary references to object and key
                obj.drop_with_heap(self.heap);
                key.drop_with_heap(self.heap);
                result.map(EvalResult::Value)
            }
            Expr::Dict(pairs) => {
                let mut eval_pairs = Vec::with_capacity(pairs.len());
                for (key_expr, value_expr) in pairs {
                    let key = return_ext_call!(self.evaluate_use(key_expr)?);
                    let value = return_ext_call!(self.evaluate_use(value_expr)?);
                    eval_pairs.push((key, value));
                }
                let dict = Dict::from_pairs(eval_pairs, self.heap, self.interns)?;
                let dict_id = self.heap.allocate(HeapData::Dict(dict))?;
                Ok(EvalResult::Value(Value::Ref(dict_id)))
            }
            Expr::Set(elements) => {
                let mut set = Set::new();
                for e in elements {
                    let v = return_ext_call!(self.evaluate_use(e)?);
                    set.add(v, self.heap, self.interns)?;
                }
                let set_id = self.heap.allocate(HeapData::Set(set))?;
                Ok(EvalResult::Value(Value::Ref(set_id)))
            }
            Expr::Not(operand) => {
                let b = return_ext_call!(self.evaluate_bool(operand)?);
                Ok(EvalResult::Value(Value::Bool(!b)))
            }
            Expr::UnaryMinus(operand) => {
                let val = return_ext_call!(self.evaluate_use(operand)?);
                match val {
                    Value::Int(n) => Ok(EvalResult::Value(Value::Int(-n))),
                    Value::Float(f) => Ok(EvalResult::Value(Value::Float(-f))),
                    _ => {
                        use crate::exception_private::{exc_fmt, ExcType};
                        let type_name = val.py_type(Some(self.heap));
                        // Drop the value before returning error to avoid ref counting leak
                        val.drop_with_heap(self.heap);
                        Err(
                            exc_fmt!(ExcType::TypeError; "bad operand type for unary -: '{type_name}'")
                                .with_position(expr_loc.position)
                                .into(),
                        )
                    }
                }
            }
            Expr::FString(parts) => self.evaluate_fstring(parts),
            Expr::IfElse { test, body, orelse } => {
                let b = return_ext_call!(self.evaluate_bool(test)?);
                if b {
                    self.evaluate_use(body)
                } else {
                    self.evaluate_use(orelse)
                }
            }
        }
    }

    /// Evaluates an expression node and discards the returned value.
    ///
    /// This is an optimization for statement expressions where the result
    /// is not needed. It avoids unnecessary allocations in some cases
    /// (e.g., pure literals) while still evaluating side effects.
    pub fn evaluate_discard(&mut self, expr_loc: &ExprLoc) -> RunResult<EvalResult<()>> {
        match &expr_loc.expr {
            // TODO, is this right for callable?
            Expr::Literal(_) | Expr::Builtin(_) => Ok(EvalResult::Value(())),
            Expr::Name(ident) => {
                // For discard, we just need to verify the variable exists
                match ident.scope {
                    NameScope::Cell => {
                        // Cell variable - look up from namespace and verify it's a cell
                        let namespace = self.namespaces.get(self.local_idx);
                        if let Value::Ref(cell_id) = namespace.get(ident.namespace_id()) {
                            // Just verify we can access it - don't need the value
                            let _ = self.heap.get_cell_value_ref(*cell_id);
                            Ok(EvalResult::Value(()))
                        } else {
                            panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug");
                        }
                    }
                    _ => self
                        .namespaces
                        .get_var_mut(self.local_idx, ident, self.interns)
                        .map(|_| EvalResult::Value(())),
                }
            }
            Expr::Call { callable, args } => {
                let args = return_ext_call!(self.evaluate_args(args, Some(callable))?);
                let eval_result = callable.call(
                    self.namespaces,
                    self.local_idx,
                    self.heap,
                    args,
                    self.interns,
                    self.print,
                    self.snapshot_tracker,
                    expr_loc.position,
                )?;
                let value = return_ext_call!(eval_result);
                value.drop_with_heap(self.heap);
                Ok(EvalResult::Value(()))
            }
            Expr::AttrCall { object, attr, args } => {
                let result = return_ext_call!(self.attr_call(object, attr, args)?);
                result.drop_with_heap(self.heap);
                Ok(EvalResult::Value(()))
            }
            Expr::AttrGet { object, attr } => {
                let result = return_ext_call!(self.attr_get(object, attr)?);
                result.drop_with_heap(self.heap);
                Ok(EvalResult::Value(()))
            }
            Expr::Op { left, op, right } => {
                // Handle and/or with short-circuit evaluation
                let result = match op {
                    Operator::And => return_ext_call!(self.eval_and(left, right)?),
                    Operator::Or => return_ext_call!(self.eval_or(left, right)?),
                    _ => return_ext_call!(self.eval_op(left, op, right)?),
                };
                result.drop_with_heap(self.heap);
                Ok(EvalResult::Value(()))
            }
            Expr::CmpOp { left, op, right } => self.cmp_op(left, op, right).map(|_| EvalResult::Value(())),
            Expr::List(elements) => {
                for el in elements {
                    return_ext_call!(self.evaluate_discard(el)?);
                }
                Ok(EvalResult::Value(()))
            }
            Expr::Tuple(elements) => {
                for el in elements {
                    return_ext_call!(self.evaluate_discard(el)?);
                }
                Ok(EvalResult::Value(()))
            }
            Expr::Subscript { object, index } => {
                // Must actually perform the subscript to catch IndexError, KeyError, etc.
                let obj = return_ext_call!(self.evaluate_use(object)?);
                // Handle external call in index evaluation - must drop obj before returning
                let key = match self.evaluate_use(index)? {
                    EvalResult::Value(v) => v,
                    EvalResult::ExternalCall(ext_call) => {
                        obj.drop_with_heap(self.heap);
                        return Ok(EvalResult::ExternalCall(ext_call));
                    }
                };
                let result = obj.py_getitem(&key, self.heap, self.interns);
                // Drop temporary references (even on error)
                obj.drop_with_heap(self.heap);
                key.drop_with_heap(self.heap);
                // Drop result if successful, propagate error if not
                result?.drop_with_heap(self.heap);
                Ok(EvalResult::Value(()))
            }
            Expr::Dict(pairs) => {
                for (key_expr, value_expr) in pairs {
                    return_ext_call!(self.evaluate_discard(key_expr)?);
                    return_ext_call!(self.evaluate_discard(value_expr)?);
                }
                Ok(EvalResult::Value(()))
            }
            Expr::Set(elements) => {
                for el in elements {
                    return_ext_call!(self.evaluate_discard(el)?);
                }
                Ok(EvalResult::Value(()))
            }
            Expr::Not(operand) | Expr::UnaryMinus(operand) => self.evaluate_discard(operand),
            Expr::FString(parts) => {
                // Still need to evaluate for side effects, then drop
                let result = return_ext_call!(self.evaluate_fstring(parts)?);
                result.drop_with_heap(self.heap);
                Ok(EvalResult::Value(()))
            }
            Expr::IfElse { test, body, orelse } => {
                let b = return_ext_call!(self.evaluate_bool(test)?);
                if b {
                    self.evaluate_discard(body)
                } else {
                    self.evaluate_discard(orelse)
                }
            }
        }
    }

    /// Evaluates an expression for its truthiness (boolean result).
    ///
    /// This is a specialized helper for conditionals that returns a `bool`
    /// directly rather than a `Value`. It includes optimizations for
    /// comparison operators, `not`, and `and`/`or` to avoid creating
    /// intermediate `Value::Bool` objects.
    pub fn evaluate_bool(&mut self, expr_loc: &ExprLoc) -> RunResult<EvalResult<bool>> {
        match &expr_loc.expr {
            Expr::CmpOp { left, op, right } => self.cmp_op(left, op, right),
            // Optimize `not` to avoid creating intermediate Value::Bool
            Expr::Not(operand) => {
                let val = return_ext_call!(self.evaluate_use(operand)?);
                let result = !val.py_bool(self.heap, self.interns);
                val.drop_with_heap(self.heap);
                Ok(EvalResult::Value(result))
            }
            // Optimize `and`/`or` with short-circuit and direct boolean conversion
            Expr::Op { left, op, right } if matches!(op, Operator::And | Operator::Or) => {
                let result = match op {
                    Operator::And => self.eval_and(left, right)?,
                    Operator::Or => self.eval_or(left, right)?,
                    _ => unreachable!(),
                };
                let value = return_ext_call!(result);
                let bool_result = value.py_bool(self.heap, self.interns);
                value.drop_with_heap(self.heap);
                Ok(EvalResult::Value(bool_result))
            }
            _ => {
                let obj = return_ext_call!(self.evaluate_use(expr_loc)?);
                let result = obj.py_bool(self.heap, self.interns);
                // Drop temporary reference
                obj.drop_with_heap(self.heap);
                Ok(EvalResult::Value(result))
            }
        }
    }

    /// Evaluates a binary operator expression (`+, -, %`, etc.).
    fn eval_op(&mut self, left: &ExprLoc, op: &Operator, right: &ExprLoc) -> RunResult<EvalResult<Value>> {
        let lhs = return_ext_call!(self.evaluate_use(left)?);
        // If evaluating right triggers an external call, we must clean up lhs before returning
        let rhs = match self.evaluate_use(right)? {
            EvalResult::Value(v) => v,
            EvalResult::ExternalCall(ext_call) => {
                lhs.drop_with_heap(self.heap);
                return Ok(EvalResult::ExternalCall(ext_call));
            }
        };
        let op_result: Option<Value> = match op {
            Operator::Add => lhs.py_add(&rhs, self.heap, self.interns)?,
            Operator::Sub => lhs.py_sub(&rhs, self.heap)?,
            Operator::Mod => lhs.py_mod(&rhs),
            Operator::Mult => lhs.py_mult(&rhs, self.heap, self.interns)?,
            Operator::Div => lhs.py_div(&rhs, self.heap)?,
            Operator::FloorDiv => lhs.py_floordiv(&rhs, self.heap)?,
            Operator::Pow => lhs.py_pow(&rhs, self.heap)?,
            _ => {
                // Drop temporary references before early return
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                return Err(RunError::internal(format!("Operator {op:?} not yet implemented")));
            }
        };
        if let Some(object) = op_result {
            // Drop temporary references to operands now that the operation is complete
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Ok(EvalResult::Value(object))
        } else {
            let lhs_type = lhs.py_type(Some(self.heap));
            let rhs_type = rhs.py_type(Some(self.heap));
            // Drop temporary references before returning error
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            SimpleException::operand_type_error(left, op, right, lhs_type, rhs_type)
        }
    }

    /// Evaluates the `and` operator with short-circuit evaluation.
    ///
    /// Returns the first falsy value encountered, or the last value if all are truthy.
    fn eval_and(&mut self, left: &ExprLoc, right: &ExprLoc) -> RunResult<EvalResult<Value>> {
        let lhs = return_ext_call!(self.evaluate_use(left)?);
        if lhs.py_bool(self.heap, self.interns) {
            // Drop left operand since we're returning the right one
            lhs.drop_with_heap(self.heap);
            self.evaluate_use(right)
        } else {
            // Short-circuit: return the falsy left operand
            Ok(EvalResult::Value(lhs))
        }
    }

    /// Evaluates the `or` operator with short-circuit semantics.
    ///
    /// Returns the first truthy value encountered, or the last value if all are falsy.
    fn eval_or(&mut self, left: &ExprLoc, right: &ExprLoc) -> RunResult<EvalResult<Value>> {
        let lhs = return_ext_call!(self.evaluate_use(left)?);
        if lhs.py_bool(self.heap, self.interns) {
            // Short-circuit: return the truthy left operand
            Ok(EvalResult::Value(lhs))
        } else {
            // Drop left operand since we're returning the right one
            lhs.drop_with_heap(self.heap);
            self.evaluate_use(right)
        }
    }

    /// Evaluates a comparison expression and returns the boolean result.
    ///
    /// Comparisons always return bool because Python chained comparisons
    /// (e.g., `1 < x < 10`) would need the intermediate value, but we don't
    /// support chaining yet, so we can return bool directly.
    fn cmp_op(&mut self, left: &ExprLoc, op: &CmpOperator, right: &ExprLoc) -> RunResult<EvalResult<bool>> {
        let lhs = return_ext_call!(self.evaluate_use(left)?);
        // If evaluating right triggers an external call, we must clean up lhs before returning
        let rhs = match self.evaluate_use(right)? {
            EvalResult::Value(v) => v,
            EvalResult::ExternalCall(ext_call) => {
                lhs.drop_with_heap(self.heap);
                return Ok(EvalResult::ExternalCall(ext_call));
            }
        };

        let result = match op {
            CmpOperator::Eq => Some(lhs.py_eq(&rhs, self.heap, self.interns)),
            CmpOperator::NotEq => Some(!lhs.py_eq(&rhs, self.heap, self.interns)),
            CmpOperator::Gt => lhs.py_cmp(&rhs, self.heap, self.interns).map(Ordering::is_gt),
            CmpOperator::GtE => lhs.py_cmp(&rhs, self.heap, self.interns).map(Ordering::is_ge),
            CmpOperator::Lt => lhs.py_cmp(&rhs, self.heap, self.interns).map(Ordering::is_lt),
            CmpOperator::LtE => lhs.py_cmp(&rhs, self.heap, self.interns).map(Ordering::is_le),
            CmpOperator::Is => Some(lhs.is(&rhs)),
            CmpOperator::IsNot => Some(!lhs.is(&rhs)),
            CmpOperator::ModEq(v) => lhs.py_mod_eq(&rhs, *v),
            // In/NotIn are not yet supported
            _ => None,
        };

        if let Some(v) = result {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            Ok(EvalResult::Value(v))
        } else {
            let left_type = lhs.py_type(Some(self.heap));
            let right_type = rhs.py_type(Some(self.heap));
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            SimpleException::cmp_type_error(left, op, right, left_type, right_type)
        }
    }

    /// Calls a method on an object: `object.attr(args)`.
    ///
    /// This evaluates `object`, looks up `attr`, calls the method with `args`,
    /// and handles proper cleanup of temporary values. Supports chained access
    /// like `a.b.c.method()`.
    fn attr_call(&mut self, object_expr: &ExprLoc, attr: &Attr, args: &ArgExprs) -> RunResult<EvalResult<Value>> {
        // Evaluate arguments first to avoid borrow conflicts
        // Note: we pass None for callable since method calls don't have a simple function name
        let args = return_ext_call!(self.evaluate_args(args, None)?);

        // Evaluate the object expression to get the value
        let mut object = return_ext_call!(self.evaluate_use(object_expr)?);

        // Call the method on the object
        let result = object.call_attr(self.heap, attr, args, self.interns);

        // Clean up the object value
        object.drop_with_heap(self.heap);

        result.map(EvalResult::Value)
    }

    /// Evaluates attribute access expression (e.g., `point.x` or `a.b.c`).
    ///
    /// Retrieves the value of an attribute from an object. Currently only
    /// supports dataclass field access. The returned value is cloned with
    /// proper reference counting. Supports chained attribute access.
    fn attr_get(&mut self, object_expr: &ExprLoc, attr: &Attr) -> RunResult<EvalResult<Value>> {
        // Convert attr to Value - uses InternString for interned attrs (no heap alloc)
        let key = attr.to_value(self.heap)?;

        // Evaluate the object expression to get the value
        let value = return_ext_call!(self.evaluate_use(object_expr)?);

        // Dispatch based on value type
        let result = if let Value::Ref(heap_id) = &value {
            let heap_id = *heap_id;
            // Use with_entry_mut to temporarily extract data and allow heap access
            self.heap.with_entry_mut(heap_id, |heap, data| match data {
                HeapData::Dataclass(dc) => {
                    let attr_value = dc.get_attr(&key, heap, self.interns)?;
                    match attr_value {
                        Some(v) => Ok(v.clone_with_heap(heap)),
                        None => Err(ExcType::attribute_error_not_found(dc.name(), attr.as_str(self.interns))),
                    }
                }
                other => {
                    let ty = other.py_type(Some(heap));
                    Err(ExcType::attribute_error(ty, attr.as_str(self.interns)))
                }
            })
        } else {
            let ty = value.py_type(Some(self.heap));
            Err(ExcType::attribute_error(ty, attr.as_str(self.interns)))
        };

        // Clean up the key (no-op for InternString, dec_ref for heap strings)
        key.drop_with_heap(self.heap);

        // Clean up the object value we retrieved
        value.drop_with_heap(self.heap);

        result.map(EvalResult::Value)
    }

    /// Evaluates an f-string by processing its parts sequentially.
    ///
    /// Each part is either:
    /// - Literal: Appended directly to the result
    /// - Interpolation: Evaluate expression, apply conversion, apply format spec
    ///
    /// Reference counting: Intermediate values are properly dropped after formatting.
    /// The final result is a new heap-allocated string.
    fn evaluate_fstring(&mut self, parts: &[FStringPart]) -> RunResult<EvalResult<Value>> {
        let mut result = String::new();

        for part in parts {
            match part {
                FStringPart::Literal(s) => result.push_str(s),
                FStringPart::Interpolation {
                    expr,
                    conversion,
                    format_spec,
                    debug_prefix,
                } => {
                    // Handle debug prefix for `=` specifier (e.g., f'{a=}' outputs "a=<value>")
                    if let Some(prefix_id) = debug_prefix {
                        result.push_str(self.interns.get_str(*prefix_id));
                    }

                    // When debug_prefix is present and no explicit conversion, default to repr
                    let effective_conversion = if debug_prefix.is_some() && *conversion == ConversionFlag::None {
                        ConversionFlag::Repr
                    } else {
                        *conversion
                    };

                    // Evaluate the expression
                    let value = return_ext_call!(self.evaluate_use(expr)?);

                    // Process the interpolation (conversion + formatting)
                    // Note: return_ext_call! will return early on external call, before dropping value
                    // This is intentional - value must stay alive if we need to resume
                    return_ext_call!(fstring_interpolation(
                        self,
                        &mut result,
                        &value,
                        effective_conversion,
                        format_spec.as_ref()
                    )?);

                    // Drop the evaluated value (important for reference counting)
                    value.drop_with_heap(self.heap);
                }
            }
        }

        // Allocate result string on heap
        let heap_id = self.heap.allocate(HeapData::Str(Str::new(result)))?;
        Ok(EvalResult::Value(Value::Ref(heap_id)))
    }

    /// Evaluates function call arguments from expressions to values with caching support.
    ///
    /// This version checks for cached partially-evaluated arguments at the call position.
    /// If cached arguments exist, they are used and evaluation continues from where it left off.
    /// If an external call suspends during argument evaluation, already-evaluated arguments
    /// are cached to avoid re-evaluation (and duplicate side effects) on resume.
    ///
    /// The `callable` parameter is used for error messages when argument unpacking fails.
    /// Pass `None` for method calls where the callable name isn't readily available.
    fn evaluate_args_cached(
        &mut self,
        args_expr: &ArgExprs,
        callable: Option<&Callable>,
        call_position: CodeRange,
    ) -> RunResult<EvalResult<ArgValues>> {
        // Check for cached partially-evaluated arguments
        if let Some(cache) = self.namespaces.take_argument_cache(call_position) {
            return self.resume_args_from_cache(args_expr, cache, callable);
        }

        // Normal evaluation with caching on suspension
        match args_expr {
            ArgExprs::Empty => Ok(EvalResult::Value(ArgValues::Empty)),
            ArgExprs::One(arg) => {
                let arg = return_ext_call!(self.evaluate_use(arg)?);
                Ok(EvalResult::Value(ArgValues::One(arg)))
            }
            ArgExprs::Two(arg1, arg2) => {
                // Evaluate first argument
                let first = match self.evaluate_use(arg1)? {
                    EvalResult::Value(v) => v,
                    EvalResult::ExternalCall(ext_call) => {
                        // First arg suspended - no caching needed (no args evaluated yet)
                        return Ok(EvalResult::ExternalCall(ext_call));
                    }
                };

                // Evaluate second argument
                match self.evaluate_use(arg2)? {
                    EvalResult::Value(second) => Ok(EvalResult::Value(ArgValues::Two(first, second))),
                    EvalResult::ExternalCall(ext_call) => {
                        // Second arg suspended - cache first arg
                        self.namespaces.set_argument_cache(ArgumentCache {
                            call_position,
                            evaluated_args: vec![first],
                            suspended_at_arg: 1,
                        });
                        Ok(EvalResult::ExternalCall(ext_call))
                    }
                }
            }
            ArgExprs::Args(args_exprs) => {
                let args = return_ext_call!(self.evaluate_pos_args_cached(args_exprs, call_position)?);
                Ok(EvalResult::Value(ArgValues::ArgsKargs {
                    args,
                    kwargs: KwargsValues::Empty,
                }))
            }
            ArgExprs::Kwargs(kwargs_exprs) => {
                let inline = return_ext_call!(self.evaluate_kwargs(kwargs_exprs)?);
                Ok(EvalResult::Value(ArgValues::Kwargs(KwargsValues::Inline(inline))))
            }
            ArgExprs::ArgsKargs {
                args,
                var_args,
                kwargs,
                var_kwargs,
            } => self.evaluate_full_args(
                args.as_deref(),
                var_args.as_ref(),
                kwargs.as_deref(),
                var_kwargs.as_ref(),
                callable.map(|c| c.name(self.interns)),
            ),
        }
    }

    /// Resumes argument evaluation from cached partially-evaluated arguments.
    fn resume_args_from_cache(
        &mut self,
        args_expr: &ArgExprs,
        cache: ArgumentCache,
        callable: Option<&Callable>,
    ) -> RunResult<EvalResult<ArgValues>> {
        match args_expr {
            ArgExprs::Two(_arg1, arg2) => {
                // We have first arg cached, evaluate second
                debug_assert_eq!(cache.suspended_at_arg, 1);
                debug_assert_eq!(cache.evaluated_args.len(), 1);

                let first = cache.evaluated_args.into_iter().next().expect("cached arg");
                match self.evaluate_use(arg2)? {
                    EvalResult::Value(second) => Ok(EvalResult::Value(ArgValues::Two(first, second))),
                    EvalResult::ExternalCall(ext_call) => {
                        // Still suspended - re-cache first arg
                        self.namespaces.set_argument_cache(ArgumentCache {
                            call_position: cache.call_position,
                            evaluated_args: vec![first],
                            suspended_at_arg: 1,
                        });
                        Ok(EvalResult::ExternalCall(ext_call))
                    }
                }
            }
            ArgExprs::Args(args_exprs) => {
                // Resume from cached positional args
                let args = return_ext_call!(self.resume_pos_args_from_cache(
                    args_exprs,
                    cache.call_position,
                    cache.evaluated_args,
                    cache.suspended_at_arg,
                )?);
                Ok(EvalResult::Value(ArgValues::ArgsKargs {
                    args,
                    kwargs: KwargsValues::Empty,
                }))
            }
            _ => {
                // Other variants don't support caching yet - fall back to regular evaluation
                // (this shouldn't happen if caching is working correctly)
                self.evaluate_args(args_expr, callable)
            }
        }
    }

    /// Evaluates function call arguments from expressions to values.
    ///
    /// The `callable` parameter is used for error messages when argument unpacking fails.
    /// Pass `None` for method calls where the callable name isn't readily available.
    fn evaluate_args(&mut self, args_expr: &ArgExprs, callable: Option<&Callable>) -> RunResult<EvalResult<ArgValues>> {
        match args_expr {
            ArgExprs::Empty => Ok(EvalResult::Value(ArgValues::Empty)),
            ArgExprs::One(arg) => {
                let arg = return_ext_call!(self.evaluate_use(arg)?);
                Ok(EvalResult::Value(ArgValues::One(arg)))
            }
            ArgExprs::Two(arg1, arg2) => {
                let first = return_ext_call!(self.evaluate_use(arg1)?);
                match self.evaluate_use(arg2)? {
                    EvalResult::Value(second) => Ok(EvalResult::Value(ArgValues::Two(first, second))),
                    EvalResult::ExternalCall(ext_call) => {
                        first.drop_with_heap(self.heap);
                        Ok(EvalResult::ExternalCall(ext_call))
                    }
                }
            }
            ArgExprs::Args(args_exprs) => {
                let args = return_ext_call!(self.evaluate_pos_args(args_exprs)?);
                Ok(EvalResult::Value(ArgValues::ArgsKargs {
                    args,
                    kwargs: KwargsValues::Empty,
                }))
            }
            ArgExprs::Kwargs(kwargs_exprs) => {
                let inline = return_ext_call!(self.evaluate_kwargs(kwargs_exprs)?);
                Ok(EvalResult::Value(ArgValues::Kwargs(KwargsValues::Inline(inline))))
            }
            ArgExprs::ArgsKargs {
                args,
                var_args,
                kwargs,
                var_kwargs,
            } => self.evaluate_full_args(
                args.as_deref(),
                var_args.as_ref(),
                kwargs.as_deref(),
                var_kwargs.as_ref(),
                callable.map(|c| c.name(self.interns)),
            ),
        }
    }

    /// Collects positional arguments into a vector of evaluated values.
    ///
    /// If evaluation of any argument fails or yields an external call, all previously
    /// evaluated arguments are dropped to maintain correct reference counts.
    fn evaluate_pos_args(&mut self, exprs: &[ExprLoc]) -> RunResult<EvalResult<Vec<Value>>> {
        let mut args: Vec<Value> = Vec::with_capacity(exprs.len());
        for expr in exprs {
            match self.evaluate_use(expr) {
                Ok(EvalResult::Value(value)) => args.push(value),
                Ok(EvalResult::ExternalCall(ext_call)) => {
                    self.drop_values(&mut args);
                    return Ok(EvalResult::ExternalCall(ext_call));
                }
                Err(err) => {
                    self.drop_values(&mut args);
                    return Err(err);
                }
            }
        }
        Ok(EvalResult::Value(args))
    }

    /// Collects positional arguments with caching support for external call resumption.
    ///
    /// On external call suspension, caches already-evaluated arguments to avoid
    /// re-evaluation on resume.
    fn evaluate_pos_args_cached(
        &mut self,
        exprs: &[ExprLoc],
        call_position: CodeRange,
    ) -> RunResult<EvalResult<Vec<Value>>> {
        let mut args: Vec<Value> = Vec::with_capacity(exprs.len());
        for (i, expr) in exprs.iter().enumerate() {
            match self.evaluate_use(expr) {
                Ok(EvalResult::Value(value)) => args.push(value),
                Ok(EvalResult::ExternalCall(ext_call)) => {
                    // Cache already-evaluated args before suspending
                    if !args.is_empty() {
                        self.namespaces.set_argument_cache(ArgumentCache {
                            call_position,
                            evaluated_args: args,
                            suspended_at_arg: i,
                        });
                    }
                    return Ok(EvalResult::ExternalCall(ext_call));
                }
                Err(err) => {
                    self.drop_values(&mut args);
                    return Err(err);
                }
            }
        }
        Ok(EvalResult::Value(args))
    }

    /// Resumes positional argument evaluation from cached state.
    fn resume_pos_args_from_cache(
        &mut self,
        exprs: &[ExprLoc],
        call_position: CodeRange,
        mut cached_args: Vec<Value>,
        suspended_at: usize,
    ) -> RunResult<EvalResult<Vec<Value>>> {
        // Continue evaluation from the arg that was being evaluated when we suspended.
        // The arg at suspended_at needs to be re-evaluated (it caused the suspension),
        // and we continue from there.
        for (i, expr) in exprs.iter().enumerate().skip(suspended_at) {
            match self.evaluate_use(expr) {
                Ok(EvalResult::Value(value)) => cached_args.push(value),
                Ok(EvalResult::ExternalCall(ext_call)) => {
                    // Still suspended - re-cache what we have
                    self.namespaces.set_argument_cache(ArgumentCache {
                        call_position,
                        evaluated_args: cached_args,
                        suspended_at_arg: i,
                    });
                    return Ok(EvalResult::ExternalCall(ext_call));
                }
                Err(err) => {
                    self.drop_values(&mut cached_args);
                    return Err(err);
                }
            }
        }
        Ok(EvalResult::Value(cached_args))
    }

    /// Builds fully general arguments supporting all Python call syntax.
    ///
    /// Evaluation order follows Python semantics:
    /// 1. Positional arguments (left to right)
    /// 2. `*args` iterable unpacking
    /// 3. Keyword arguments and `**kwargs` dict unpacking
    ///
    /// On error or external call, all partially evaluated arguments are cleaned up.
    /// The `args` parameter is passed to `build_kwargs` which takes ownership of cleanup
    /// responsibility on error paths.
    fn evaluate_full_args(
        &mut self,
        args_exprs: Option<&[ExprLoc]>,
        var_args_expr: Option<&ExprLoc>,
        kwargs_exprs: Option<&[Kwarg]>,
        var_kwargs_expr: Option<&ExprLoc>,
        callable_name: Option<&str>,
    ) -> RunResult<EvalResult<ArgValues>> {
        let mut args = if let Some(exprs) = args_exprs {
            return_ext_call!(self.evaluate_pos_args(exprs)?)
        } else {
            Vec::new()
        };

        if let Some(var_args) = var_args_expr {
            let value = match self.evaluate_use(var_args) {
                Ok(EvalResult::Value(value)) => value,
                Ok(EvalResult::ExternalCall(ext_call)) => {
                    self.drop_values(&mut args);
                    return Ok(EvalResult::ExternalCall(ext_call));
                }
                Err(err) => {
                    self.drop_values(&mut args);
                    return Err(err);
                }
            };

            let ok = self.extend_args_from_iterable(&mut args, &value);
            value.drop_with_heap(self.heap);
            if !ok {
                self.drop_values(&mut args);
                return exc_err_fmt!(ExcType::TypeError; "argument after * must be an iterable");
            }
        }

        let kwargs = return_ext_call!(self.build_kwargs(kwargs_exprs, var_kwargs_expr, callable_name, &mut args)?);

        if args.is_empty() {
            if kwargs.is_empty() {
                Ok(EvalResult::Value(ArgValues::Empty))
            } else {
                Ok(EvalResult::Value(ArgValues::Kwargs(kwargs)))
            }
        } else {
            Ok(EvalResult::Value(ArgValues::ArgsKargs { args, kwargs }))
        }
    }

    /// Evaluates inline keyword arguments into `(StringId, Value)` pairs.
    ///
    /// Used for the simple case of `foo(a=1, b=2)` without `**kwargs` unpacking.
    /// On error or external call, drops all previously evaluated kwargs.
    fn evaluate_kwargs(&mut self, kwargs_exprs: &[Kwarg]) -> RunResult<EvalResult<Vec<(StringId, Value)>>> {
        let mut inline = Vec::with_capacity(kwargs_exprs.len());
        for kwarg in kwargs_exprs {
            match self.evaluate_use(&kwarg.value) {
                Ok(EvalResult::Value(value)) => inline.push((kwarg.key.name_id, value)),
                Ok(EvalResult::ExternalCall(ext_call)) => {
                    self.drop_inline_kwargs(&mut inline);
                    return Ok(EvalResult::ExternalCall(ext_call));
                }
                Err(err) => {
                    self.drop_inline_kwargs(&mut inline);
                    return Err(err);
                }
            }
        }
        Ok(EvalResult::Value(inline))
    }

    /// Copies items from an iterable heap object into `args`.
    ///
    /// Only supports `list` and `tuple` iterables. Returns `true` on success,
    /// `false` if the value is not a supported iterable type.
    fn extend_args_from_iterable(&mut self, args: &mut Vec<Value>, iterable: &Value) -> bool {
        let Value::Ref(heap_id) = iterable else { return false };

        // Two-phase copy: first collect values while heap is borrowed (copy_for_extend
        // doesn't increment refcount), then increment refcounts after borrow ends.
        // This avoids borrow conflicts between heap.get() and heap.inc_ref().
        let copied_values: Vec<Value> = match self.heap.get(*heap_id) {
            HeapData::Tuple(tuple) => tuple.as_vec().iter().map(Value::copy_for_extend).collect(),
            HeapData::List(list) => list.as_vec().iter().map(Value::copy_for_extend).collect(),
            _ => return false,
        };

        for value in &copied_values {
            if let Value::Ref(id) = value {
                self.heap.inc_ref(*id);
            }
        }
        args.extend(copied_values);
        true
    }

    /// Builds keyword arguments from inline kwargs and/or `**kwargs` unpacking.
    ///
    /// Takes ownership of `args` cleanup responsibility: on any error or external call,
    /// this function will drop `args` before returning. On success, `args` is untouched.
    fn build_kwargs(
        &mut self,
        inline_exprs: Option<&[Kwarg]>,
        var_kwargs_expr: Option<&ExprLoc>,
        callable_name: Option<&str>,
        args: &mut Vec<Value>,
    ) -> RunResult<EvalResult<KwargsValues>> {
        if let Some(var_kwargs) = var_kwargs_expr {
            return self.build_kwargs_from_mapping(inline_exprs, var_kwargs, callable_name, args);
        }

        if let Some(kwargs_exprs) = inline_exprs {
            let mut inline = Vec::with_capacity(kwargs_exprs.len());
            for kwarg in kwargs_exprs {
                match self.evaluate_use(&kwarg.value) {
                    Ok(EvalResult::Value(value)) => inline.push((kwarg.key.name_id, value)),
                    Ok(EvalResult::ExternalCall(ext_call)) => {
                        self.drop_values(args);
                        self.drop_inline_kwargs(&mut inline);
                        return Ok(EvalResult::ExternalCall(ext_call));
                    }
                    Err(err) => {
                        self.drop_values(args);
                        self.drop_inline_kwargs(&mut inline);
                        return Err(err);
                    }
                }
            }
            return Ok(EvalResult::Value(KwargsValues::Inline(inline)));
        }

        Ok(EvalResult::Value(KwargsValues::Empty))
    }

    /// Builds keyword arguments when `**kwargs` dict unpacking is present.
    ///
    /// Merges the unpacked dict with any inline keyword arguments. Inline kwargs
    /// take precedence and will error on duplicate keys (matching Python semantics).
    ///
    /// Like `build_kwargs`, takes ownership of `args` cleanup responsibility.
    fn build_kwargs_from_mapping(
        &mut self,
        inline_exprs: Option<&[Kwarg]>,
        var_kwargs_expr: &ExprLoc,
        callable_name: Option<&str>,
        args: &mut Vec<Value>,
    ) -> RunResult<EvalResult<KwargsValues>> {
        let var_kwargs_value = match self.evaluate_use(var_kwargs_expr) {
            Ok(EvalResult::Value(value)) => value,
            Ok(EvalResult::ExternalCall(ext_call)) => {
                self.drop_values(args);
                return Ok(EvalResult::ExternalCall(ext_call));
            }
            Err(err) => {
                self.drop_values(args);
                return Err(err);
            }
        };

        let Value::Ref(heap_id) = &var_kwargs_value else {
            let type_name = var_kwargs_value.py_type(Some(self.heap));
            var_kwargs_value.drop_with_heap(self.heap);
            self.drop_values(args);
            return Err(ExcType::kwargs_type_error(callable_name, type_name).into());
        };

        let HeapData::Dict(dict) = self.heap.get(*heap_id) else {
            let type_ = var_kwargs_value.py_type(Some(self.heap));
            var_kwargs_value.drop_with_heap(self.heap);
            self.drop_values(args);
            return Err(ExcType::kwargs_type_error(callable_name, type_).into());
        };

        // Two-phase copy pattern (see extend_args_from_iterable for explanation)
        let copied_pairs: Vec<(Value, Value)> = dict
            .iter()
            .map(|(k, v)| (k.copy_for_extend(), v.copy_for_extend()))
            .collect();

        for (key, value) in &copied_pairs {
            if let Value::Ref(id) = key {
                self.heap.inc_ref(*id);
            }
            if let Value::Ref(id) = value {
                self.heap.inc_ref(*id);
            }
        }

        let mut kwargs = match Dict::from_pairs(copied_pairs, self.heap, self.interns) {
            Ok(dict) => dict,
            Err(err) => {
                var_kwargs_value.drop_with_heap(self.heap);
                self.drop_values(args);
                return Err(err);
            }
        };
        var_kwargs_value.drop_with_heap(self.heap);

        // Merge inline kwargs into the dict, checking for duplicates
        if let Some(kwargs_exprs) = inline_exprs {
            for kwarg in kwargs_exprs {
                let key = Value::InternString(kwarg.key.name_id);
                let has_duplicate = match kwargs.get(&key, self.heap, self.interns) {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(err) => {
                        self.drop_values(args);
                        self.drop_dict(kwargs);
                        return Err(err);
                    }
                };
                if has_duplicate {
                    let error = ExcType::duplicate_kwarg_error(callable_name, self.interns.get_str(kwarg.key.name_id));
                    self.drop_values(args);
                    self.drop_dict(kwargs);
                    return Err(error.into());
                }

                let value = match self.evaluate_use(&kwarg.value) {
                    Ok(EvalResult::Value(v)) => v,
                    Ok(EvalResult::ExternalCall(ext_call)) => {
                        self.drop_values(args);
                        self.drop_dict(kwargs);
                        return Ok(EvalResult::ExternalCall(ext_call));
                    }
                    Err(err) => {
                        self.drop_values(args);
                        self.drop_dict(kwargs);
                        return Err(err);
                    }
                };
                match kwargs.set(key, value, self.heap, self.interns) {
                    Ok(Some(old_value)) => old_value.drop_with_heap(self.heap),
                    Ok(None) => {}
                    Err(err) => {
                        self.drop_values(args);
                        self.drop_dict(kwargs);
                        return Err(err);
                    }
                }
            }
        }

        Ok(EvalResult::Value(KwargsValues::Dict(kwargs)))
    }

    /// Drops positional arguments, ensuring each `Value` decrements its refcount.
    fn drop_values(&mut self, values: &mut Vec<Value>) {
        for value in values.drain(..) {
            value.drop_with_heap(self.heap);
        }
    }

    /// Drops inline kwargs, only releasing the stored value `Value`s.
    fn drop_inline_kwargs(&mut self, inline: &mut Vec<(StringId, Value)>) {
        for (_, value) in inline.drain(..) {
            value.drop_with_heap(self.heap);
        }
    }

    /// Drops all entries in a dict by consuming it.
    fn drop_dict(&mut self, dict: Dict) {
        for (key, value) in dict {
            key.drop_with_heap(self.heap);
            value.drop_with_heap(self.heap);
        }
    }
}

/// Return value from evaluating an expression.
///
/// Can be either a value or a marker indicating we must yield control to the host to call
/// this function.
#[derive(Debug)]
#[must_use]
pub enum EvalResult<T> {
    Value(T),
    ExternalCall(ExternalCall),
}
