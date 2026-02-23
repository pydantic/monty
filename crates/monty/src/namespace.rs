use crate::{
    exception_private::ExceptionRaise,
    heap::{Heap, HeapId},
    parse::CodeRange,
    resource::{ResourceError, ResourceTracker},
    value::Value,
};

/// Unique identifier for values stored inside the namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamespaceId(u32);

impl NamespaceId {
    pub fn new(index: usize) -> Self {
        Self(index.try_into().expect("Invalid namespace id"))
    }

    /// Returns the raw index value.
    ///
    /// Used by the bytecode compiler to emit slot indices for variable access.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Index for the global (module-level) namespace in Namespaces.
/// At module level, local_idx == GLOBAL_NS_IDX (same namespace).
pub(crate) const GLOBAL_NS_IDX: NamespaceId = NamespaceId(0);

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Namespace(Vec<Value>);

impl Namespace {
    fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub fn get(&self, index: NamespaceId) -> &Value {
        &self.0[index.index()]
    }

    #[cfg(feature = "ref-count-return")]
    pub fn get_opt(&self, index: NamespaceId) -> Option<&Value> {
        self.0.get(index.index())
    }

    pub fn get_mut(&mut self, index: NamespaceId) -> &mut Value {
        &mut self.0[index.index()]
    }

    pub fn mut_vec(&mut self) -> &mut Vec<Value> {
        &mut self.0
    }
}

impl IntoIterator for Namespace {
    type Item = Value;
    type IntoIter = std::vec::IntoIter<Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Storage for all namespaces during execution.
///
/// This struct owns all namespace data, allowing safe mutable access through indices.
/// Index 0 is always the global (module-level) namespace.
///
/// # Design Rationale
///
/// Instead of using raw pointers to share namespace access between frames,
/// we use indices into this central namespaces. Since variable scope (Local vs Global)
/// is known at compile time, we only ever need one mutable reference at a time.
///
/// # Closure Support
///
/// Variables captured by closures are stored in cells on the heap, not in namespaces.
/// The `get_var_value` method handles both namespace-based and cell-based variable access.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Namespaces {
    stack: Vec<Namespace>,
    /// if we have an old namespace to reuse, trace its id
    reuse_ids: Vec<NamespaceId>,
    /// Return values from external function calls or functions that completed after internal external calls.
    ///
    /// Each entry is `(call_position, value)`:
    /// - `call_position` is `None` for direct external function calls (any matching position works)
    /// - `call_position` is `Some(pos)` for function return values (only match at that exact call site)
    ///
    /// This distinction is necessary because during argument re-evaluation, we might have multiple
    /// function calls. Only the correct call should receive the cached return value.
    ext_return_values: Vec<(Option<CodeRange>, Value)>,
    /// Index of the next return value to be used.
    ///
    /// Since we can have multiple external function calls within a single statement (e.g. `foo() + bar()`),
    /// we need to keep track of which functions we've already called to continue execution.
    ///
    /// This is somewhat similar to temporal style durable execution, but just within a single statement.
    next_ext_return_value: usize,
    /// Pending exception from an external function call.
    ///
    /// When set, the next call to `take_ext_return_value` will return this error,
    /// allowing it to propagate through try/except blocks.
    ext_exception: Option<ExceptionRaise>,
}

impl Namespaces {
    /// Creates namespaces with the global namespace initialized.
    ///
    /// The global namespace is always at index 0.
    pub fn new(namespace: Vec<Value>) -> Self {
        Self {
            stack: vec![Namespace(namespace)],
            reuse_ids: vec![],
            ext_return_values: vec![],
            next_ext_return_value: 0,
            ext_exception: None,
        }
    }

    /// Gets an immutable slice reference to a namespace by index.
    ///
    /// Used for reading from the enclosing namespace when defining closures,
    /// without requiring mutable access.
    ///
    /// # Panics
    /// Panics if `idx` is out of bounds.
    pub fn get(&self, idx: NamespaceId) -> &Namespace {
        &self.stack[idx.index()]
    }

    /// Gets a mutable slice reference to a namespace by index.
    ///
    /// # Panics
    /// Panics if `idx` is out of bounds.
    pub fn get_mut(&mut self, idx: NamespaceId) -> &mut Namespace {
        &mut self.stack[idx.index()]
    }

    /// Creates a new namespace for a function call with memory and recursion tracking.
    ///
    /// This method:
    /// 1. Checks recursion depth limit (fails fast before allocating)
    /// 2. Tracks namespace memory usage through the heap's `ResourceTracker`
    ///
    /// # Arguments
    /// * `namespace_size` - Expected number of values in the namespace
    /// * `heap` - The heap, used to access the resource tracker for memory accounting
    ///
    /// # Returns
    /// * `Ok(NamespaceId)` - Index of the new namespace
    /// * `Err(ResourceError::Recursion)` - If adding this namespace would exceed recursion limit
    /// * `Err(ResourceError::Memory)` - If adding this namespace would exceed memory limits
    pub fn new_namespace(
        &mut self,
        namespace_size: usize,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<NamespaceId, ResourceError> {
        // Track the memory used by this namespace's slots
        let size = namespace_size * std::mem::size_of::<Value>();
        heap.tracker_mut().on_allocate(|| size)?;

        if let Some(reuse_id) = self.reuse_ids.pop() {
            Ok(reuse_id)
        } else {
            let idx = NamespaceId::new(self.stack.len());
            self.stack.push(Namespace::with_capacity(namespace_size));
            Ok(idx)
        }
    }

    /// Registers a pre-built namespace (e.g., from a coroutine) with memory and recursion tracking.
    ///
    /// This is similar to `new_namespace` but takes an already-populated `Vec<Value>` instead
    /// of creating an empty one. Used when starting execution of a coroutine whose namespace
    /// was pre-bound at call time.
    ///
    /// # Arguments
    /// * `namespace` - The pre-built namespace values
    /// * `heap` - The heap, used to access the resource tracker for memory accounting
    ///
    /// # Returns
    /// * `Ok(NamespaceId)` - Index of the registered namespace
    /// * `Err(ResourceError::Recursion)` - If adding this namespace would exceed recursion limit
    /// * `Err(ResourceError::Memory)` - If adding this namespace would exceed memory limits
    pub fn register_prebuilt(
        &mut self,
        namespace: Vec<Value>,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<NamespaceId, ResourceError> {
        // Track the memory used by this namespace's slots
        let size = namespace.len() * std::mem::size_of::<Value>();
        heap.tracker_mut().on_allocate(|| size)?;

        // Try to reuse an existing slot, or push a new one
        if let Some(reuse_id) = self.reuse_ids.pop() {
            // Replace the old namespace with the new one
            self.stack[reuse_id.index()] = Namespace(namespace);
            Ok(reuse_id)
        } else {
            let idx = NamespaceId::new(self.stack.len());
            self.stack.push(Namespace(namespace));
            Ok(idx)
        }
    }

    /// Voids the most recently added namespace (after function returns),
    /// properly cleaning up any heap-allocated values.
    ///
    /// This method:
    /// 1. Tracks the freed memory through the heap's `ResourceTracker`
    /// 2. Decrements reference counts for any `Value::Ref` entries in the namespace
    ///
    /// # Panics
    /// Panics if attempting to pop the global namespace (index 0).
    pub fn drop_with_heap(&mut self, namespace_id: NamespaceId, heap: &mut Heap<impl ResourceTracker>) {
        let namespace = &mut self.stack[namespace_id.index()];
        // Track the freed memory for this namespace
        let size = namespace.0.len() * std::mem::size_of::<Value>();
        heap.tracker_mut().on_free(|| size);

        for value in namespace.0.drain(..) {
            value.drop_with_heap(heap);
        }
        self.reuse_ids.push(namespace_id);
    }

    /// Cleans up the global namespace by dropping all values with proper ref counting.
    ///
    /// Call this before the namespaces is dropped to properly decrement reference counts
    /// for any `Value::Ref` entries in the global namespace and return values.
    ///
    /// Only needed when `ref-count-panic` is enabled, since the Drop impl panics on unfreed Refs.
    #[cfg(feature = "ref-count-panic")]
    pub fn drop_global_with_heap(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        // Clean up global namespace
        let global = self.get_mut(GLOBAL_NS_IDX);
        for value in &mut global.0 {
            let v = std::mem::replace(value, Value::Undefined);
            v.drop_with_heap(heap);
        }
        // Clean up any remaining return values from external function calls
        for (_, value) in std::mem::take(&mut self.ext_return_values) {
            value.drop_with_heap(heap);
        }
        // Clear any pending exception
        self.ext_exception = None;
    }

    /// Returns the global namespace for final inspection (e.g., ref-count testing).
    ///
    /// Consumes the namespaces since the namespace Vec is moved out.
    ///
    /// Only available when the `ref-count-return` feature is enabled.
    #[cfg(feature = "ref-count-return")]
    pub fn into_global(mut self) -> Namespace {
        self.stack.swap_remove(GLOBAL_NS_IDX.index())
    }

    /// Returns an iterator over all HeapIds referenced by values in all namespaces.
    ///
    /// This is used by garbage collection to find all root references. Any heap
    /// object reachable from these roots should not be collected.
    pub fn iter_heap_ids(&self) -> impl Iterator<Item = HeapId> + '_ {
        self.stack
            .iter()
            .flat_map(|namespace| namespace.0.iter().filter_map(Value::ref_id))
    }
}
