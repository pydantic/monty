//! Comparison operation helpers for the VM.
//!
//! Comparisons support dunder protocols: when comparing instances, the VM looks
//! up `__eq__`/`__ne__`/`__lt__`/`__le__`/`__gt__`/`__ge__` on the type.

use super::{VM, call::CallResult};
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunError},
    heap::HeapData,
    intern::{StaticStrings, StringId},
    io::PrintWriter,
    resource::ResourceTracker,
    types::{LongInt, PyTrait},
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Equality comparison with dunder support.
    pub(super) fn compare_eq(&mut self) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Try instance dunder first (for __eq__)
        if let Some(result) = self.try_instance_compare(&lhs, &rhs, StaticStrings::DunderEq)? {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            return Ok(result);
        }

        // Fast path: native comparison
        let result = lhs.py_eq(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        Ok(CallResult::Push(Value::Bool(result)))
    }

    /// Inequality comparison with dunder support.
    pub(super) fn compare_ne(&mut self) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Try instance dunder first (for __ne__)
        if let Some(result) = self.try_instance_compare(&lhs, &rhs, StaticStrings::DunderNe)? {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            return Ok(result);
        }

        // Fast path: native comparison
        let result = !lhs.py_eq(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        Ok(CallResult::Push(Value::Bool(result)))
    }

    /// Ordering comparison with dunder support.
    pub(super) fn compare_lt(&mut self) -> Result<CallResult, RunError> {
        self.compare_ord_dunder(StaticStrings::DunderLt, std::cmp::Ordering::is_lt)
    }

    pub(super) fn compare_le(&mut self) -> Result<CallResult, RunError> {
        self.compare_ord_dunder(StaticStrings::DunderLe, std::cmp::Ordering::is_le)
    }

    pub(super) fn compare_gt(&mut self) -> Result<CallResult, RunError> {
        self.compare_ord_dunder(StaticStrings::DunderGt, std::cmp::Ordering::is_gt)
    }

    pub(super) fn compare_ge(&mut self) -> Result<CallResult, RunError> {
        self.compare_ord_dunder(StaticStrings::DunderGe, std::cmp::Ordering::is_ge)
    }

    /// Ordering comparison helper with dunder fallback.
    fn compare_ord_dunder(
        &mut self,
        dunder: StaticStrings,
        check: fn(std::cmp::Ordering) -> bool,
    ) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Try instance dunder first
        if let Some(result) = self.try_instance_compare(&lhs, &rhs, dunder)? {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            return Ok(result);
        }

        // Fast path: native ordering
        let result = lhs.py_cmp(&rhs, self.heap, self.interns).is_some_and(check);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        Ok(CallResult::Push(Value::Bool(result)))
    }

    /// Try to dispatch a comparison via instance dunder.
    ///
    /// Returns `Some(CallResult)` if a dunder was found on either operand,
    /// `None` to fall through to native comparison.
    fn try_instance_compare(
        &mut self,
        lhs: &Value,
        rhs: &Value,
        dunder: StaticStrings,
    ) -> Result<Option<CallResult>, RunError> {
        let dunder_id = dunder.into();

        // Try lhs.__op__(rhs)
        if let Value::Ref(lhs_id) = lhs
            && matches!(self.heap.get(*lhs_id), HeapData::Instance(_))
            && let Some(method) = self.lookup_type_dunder(*lhs_id, dunder_id)
        {
            let rhs_clone = rhs.clone_with_heap(self.heap);
            let result = self.call_dunder(*lhs_id, method, ArgValues::One(rhs_clone))?;
            return Ok(Some(result));
        }

        // For __ne__, if no __ne__ defined, try to negate __eq__
        // (Python falls back to negating __eq__ for __ne__)
        // We don't do that here - native comparison handles it.

        Ok(None)
    }

    /// Identity comparison (is/is not).
    pub(super) fn compare_is(&mut self, negate: bool) {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.is(&rhs);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(if negate { !result } else { result }));
    }

    /// Membership test (in/not in) with dunder support.
    pub(super) fn compare_in(&mut self, negate: bool) -> Result<CallResult, RunError> {
        let container = self.pop();
        let item = self.pop();

        // Try __contains__ dunder on instance
        if let Value::Ref(container_id) = &container
            && matches!(self.heap.get(*container_id), HeapData::Instance(_))
        {
            let dunder_id = StaticStrings::DunderContains.into();
            if let Some(method) = self.lookup_type_dunder(*container_id, dunder_id) {
                let item_clone = item.clone_with_heap(self.heap);
                // __contains__ takes (self, item), returns bool
                // We need to negate if this is 'not in'
                // Store the negate flag so the caller can handle it
                // Actually we can't easily negate after FramePushed.
                // For now, call the dunder and let the result be post-processed.
                // The issue: if it returns FramePushed, we can't negate.
                // Solution: we handle 'not in' by wrapping the result in a separate step.
                // For 'in', just return the result.
                // For 'not in', we need to negate after the frame returns.
                // This is complex, so let's handle the sync case directly.
                let result = self.call_dunder(*container_id, method, ArgValues::One(item_clone))?;
                item.drop_with_heap(self.heap);
                container.drop_with_heap(self.heap);

                if negate {
                    match result {
                        CallResult::Push(v) => {
                            let bool_val = v.py_bool(self.heap, self.interns);
                            v.drop_with_heap(self.heap);
                            return Ok(CallResult::Push(Value::Bool(!bool_val)));
                        }
                        CallResult::FramePushed => {
                            // Set flag to negate the return value when frame returns
                            self.pending_negate_bool = true;
                            return Ok(CallResult::FramePushed);
                        }
                        other => return Ok(other),
                    }
                }

                return Ok(result);
            }
        }

        // Native containment check
        let result = container.py_contains(&item, self.heap, self.interns);
        item.drop_with_heap(self.heap);
        container.drop_with_heap(self.heap);

        let contained = result?;
        Ok(CallResult::Push(Value::Bool(if negate {
            !contained
        } else {
            contained
        })))
    }

    /// Modulo equality comparison: a % b == k
    ///
    /// Returns `CallResult` to support dunder dispatch. When both operands are
    /// native types, returns `Push(Bool)`. When an instance dunder pushes a frame,
    /// the caller must handle the FramePushed + pending_mod_eq_k flow.
    pub(super) fn compare_mod_eq(&mut self, k: &Value) -> Result<CallResult, RunError> {
        let rhs = self.pop();
        let lhs = self.pop();

        // Try fast path for Int/Float types
        let mod_result = match k {
            Value::Int(k_val) => lhs.py_mod_eq(&rhs, *k_val),
            _ => None,
        };

        if let Some(is_equal) = mod_result {
            lhs.drop_with_heap(self.heap);
            rhs.drop_with_heap(self.heap);
            return Ok(CallResult::Push(Value::Bool(is_equal)));
        }

        let mod_value = lhs.py_mod(&rhs, self.heap);

        match mod_value {
            Ok(Some(v)) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                let (k_value, k_needs_drop) = if let Value::InternLongInt(id) = k {
                    let bi = self.interns.get_long_int(*id).clone();
                    (LongInt::new(bi).into_value(self.heap)?, true)
                } else {
                    (k.copy_for_extend(), false)
                };

                let is_equal = v.py_eq(&k_value, self.heap, self.interns);
                v.drop_with_heap(self.heap);
                if k_needs_drop {
                    k_value.drop_with_heap(self.heap);
                }
                Ok(CallResult::Push(Value::Bool(is_equal)))
            }
            Ok(None) => {
                // Native mod returned None - try __mod__/__rmod__ dunder dispatch
                let dunder_id: StringId = StaticStrings::DunderMod.into();
                let reflected_id: Option<StringId> = Some(StaticStrings::DunderRmod.into());

                if let Some(result) = self.try_binary_dunder(&lhs, &rhs, dunder_id, reflected_id)? {
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    // If result is Push, do the == k comparison inline.
                    // If FramePushed, caller must set pending_mod_eq_k.
                    match result {
                        CallResult::Push(mod_val) => {
                            let (k_value, k_needs_drop) = if let Value::InternLongInt(id) = k {
                                let bi = self.interns.get_long_int(*id).clone();
                                (LongInt::new(bi).into_value(self.heap)?, true)
                            } else {
                                (k.copy_for_extend(), false)
                            };
                            let is_equal = mod_val.py_eq(&k_value, self.heap, self.interns);
                            mod_val.drop_with_heap(self.heap);
                            if k_needs_drop {
                                k_value.drop_with_heap(self.heap);
                            }
                            Ok(CallResult::Push(Value::Bool(is_equal)))
                        }
                        CallResult::FramePushed => Ok(CallResult::FramePushed),
                        other => Ok(other),
                    }
                } else {
                    let lhs_type = lhs.py_type(self.heap);
                    let rhs_type = rhs.py_type(self.heap);
                    lhs.drop_with_heap(self.heap);
                    rhs.drop_with_heap(self.heap);
                    Err(ExcType::binary_type_error("%", lhs_type, rhs_type))
                }
            }
            Err(e) => {
                lhs.drop_with_heap(self.heap);
                rhs.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }
}
