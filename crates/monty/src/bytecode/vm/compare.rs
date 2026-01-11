//! Comparison operation helpers for the VM.

use super::VM;
use crate::{
    exception_private::{ExcType, RunError},
    intern::StringId,
    io::PrintWriter,
    resource::ResourceTracker,
    types::PyTrait,
    value::Value,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Equality comparison.
    pub(super) fn compare_eq(&mut self) {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_eq(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
    }

    /// Inequality comparison.
    pub(super) fn compare_ne(&mut self) {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = !lhs.py_eq(&rhs, self.heap, self.interns);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
    }

    /// Ordering comparison with a predicate.
    pub(super) fn compare_ord<F>(&mut self, check: F)
    where
        F: FnOnce(std::cmp::Ordering) -> bool,
    {
        let rhs = self.pop();
        let lhs = self.pop();
        let result = lhs.py_cmp(&rhs, self.heap, self.interns).is_some_and(check);
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(result));
    }

    /// Identity comparison (is/is not).
    ///
    /// Compares identity using `Value::is()` which compares IDs.
    ///
    /// Identity is determined by `Value::id()` which uses:
    /// - Fixed IDs for singletons (None, True, False, Ellipsis)
    /// - Interned string/bytes index for InternString/InternBytes
    /// - HeapId for heap-allocated values (Ref)
    /// - Value-based hashing for immediate types (Int, Float, Function, etc.)
    pub(super) fn compare_is(&mut self, negate: bool) {
        let rhs = self.pop();
        let lhs = self.pop();

        let result = lhs.is(&rhs);

        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        self.push(Value::Bool(if negate { !result } else { result }));
    }

    /// Membership test (in/not in).
    pub(super) fn compare_in(&mut self, negate: bool) -> Result<(), RunError> {
        let container = self.pop(); // container (rhs)
        let item = self.pop(); // item to find (lhs)

        let result = container.py_contains(&item, self.heap, self.interns);

        item.drop_with_heap(self.heap);
        container.drop_with_heap(self.heap);

        let contained = result?;
        self.push(Value::Bool(if negate { !contained } else { contained }));
        Ok(())
    }

    /// Modulo equality comparison: a % b == k
    ///
    /// This is an optimization for patterns like `x % 3 == 0`. The constant k
    /// is provided by the caller (fetched from the constant pool using the
    /// cached code reference in the run loop).
    pub(super) fn compare_mod_eq(&mut self, k: &Value) -> Result<(), RunError> {
        let rhs = self.pop(); // divisor (b)
        let lhs = self.pop(); // dividend (a)

        // Compute a % b
        let mod_result = match k {
            Value::Int(k) => lhs.py_mod_eq(&rhs, *k),
            _ => None,
        };

        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);

        if let Some(is_equal) = mod_result {
            self.push(Value::Bool(is_equal));
            Ok(())
        } else {
            Err(ExcType::type_error("unsupported operand type(s) for %"))
        }
    }

    /// Loads an attribute from an object and pushes it onto the stack.
    ///
    /// Returns an AttributeError if the attribute doesn't exist.
    pub(super) fn load_attr(&mut self, name_id: StringId) -> Result<(), RunError> {
        let obj = self.pop();
        let result = obj.py_get_attr(name_id, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        self.push(result?);
        Ok(())
    }

    /// Stores a value as an attribute on an object.
    ///
    /// Returns an AttributeError if the attribute cannot be set.
    pub(super) fn store_attr(&mut self, name_id: StringId) -> Result<(), RunError> {
        let obj = self.pop();
        let value = self.pop();
        // py_set_attr takes ownership of value and drops it on error
        let result = obj.py_set_attr(name_id, value, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        result
    }
}
