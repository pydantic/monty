use std::collections::HashMap;

use crate::{
    exception_private::{ExcType, ExceptionRaise, RunError},
    expressions::{Identifier, NameScope},
    heap::{Heap, HeapId},
    intern::Interns,
    parse::CodeRange,
    resource::{ResourceError, ResourceTracker},
    run_frame::RunResult,
    snapshot::ArgumentCache,
    value::Value,
};

/// Unique identifier for values stored inside the namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct NamespaceId(u32);

impl NamespaceId {
    pub fn new(index: usize) -> Self {
        NamespaceId(index.try_into().expect("Invalid namespace id"))
    }

    /// Returns the raw index value.
    #[inline]
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Index for the global (module-level) namespace in Namespaces.
/// At module level, local_idx == GLOBAL_NS_IDX (same namespace).
pub const GLOBAL_NS_IDX: NamespaceId = NamespaceId(0);

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Namespace(Vec<Value>);

impl Namespace {
    fn with_capacity(capacity: usize) -> Self {
        Namespace(Vec::with_capacity(capacity))
    }

    pub fn get(&self, index: NamespaceId) -> &Value {
        &self.0[index.index()]
    }

    pub fn get_opt(&self, index: NamespaceId) -> Option<&Value> {
        self.0.get(index.index())
    }

    pub fn get_mut(&mut self, index: NamespaceId) -> &mut Value {
        &mut self.0[index.index()]
    }

    pub fn set(&mut self, index: NamespaceId, value: Value) {
        self.0[index.index()] = value;
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
    /// Cached partially-evaluated arguments from when an external call suspended.
    ///
    /// When evaluating arguments for a call and one of them triggers an external call,
    /// we cache the already-evaluated arguments here. On resume, we restore from this
    /// cache instead of re-evaluating arguments (which would cause duplicate side effects).
    argument_cache: Option<ArgumentCache>,
    /// Cached return values from user-defined functions that completed after internal external calls.
    ///
    /// Unlike `ext_return_values` which uses index-based lookup for external calls, this map
    /// allows direct lookup by call position. This is needed because function returns may be
    /// interspersed with external call returns, but we need to find the correct function return
    /// by its exact call site position.
    func_return_values: HashMap<CodeRange, Value>,
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
            argument_cache: None,
            func_return_values: HashMap::new(),
        }
    }

    /// Push a return value from an external function call or a completed function.
    ///
    /// Also resets the return pointer to zero so we start getting values from the beginning.
    ///
    /// # Arguments
    /// * `call_position` - The position of the call that should receive this value:
    ///   - `None` for direct external function calls (any call at the right point can consume it)
    ///   - `Some(pos)` for functions that completed after internal external calls (only the
    ///     call at that exact position should consume it)
    /// * `return_value` - The value to cache
    pub fn push_ext_return_value(&mut self, call_position: Option<CodeRange>, return_value: Value) {
        // Don't reset index - let values accumulate and be found by position search
        self.ext_return_values.push((call_position, return_value));
    }

    /// Sets a pending exception from an external function call.
    ///
    /// The next call to `take_ext_return_value` will return this exception as an error,
    /// allowing it to propagate through try/except blocks.
    pub fn set_ext_exception(&mut self, exc: ExceptionRaise) {
        self.next_ext_return_value = 0;
        self.ext_exception = Some(exc);
    }

    /// Gets a return value or exception from an external function call.
    ///
    /// First checks for a pending exception (set via `set_ext_exception`). If found,
    /// returns `Err(RunError::Exc(...))` so the exception propagates through try/except.
    ///
    /// Otherwise checks `func_return_values` map for a matching position.
    /// Values are cloned (not removed) so they're available for re-evaluation.
    /// The map is cleared when function frames complete, handling recursion.
    ///
    /// Returns `Ok(Some(Value))` if a matching return value is found, or `Ok(None)` if not.
    ///
    /// # Arguments
    /// * `heap` - The heap for cloning values
    /// * `current_position` - The position of the call site requesting a cached value
    pub fn take_ext_return_value(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        current_position: CodeRange,
    ) -> RunResult<Option<Value>> {
        // Check for pending exception first
        if let Some(exc) = self.ext_exception.take() {
            return Err(RunError::Exc(exc));
        }

        // Check func_return_values map for exact position match
        // Clone (don't remove) so the value is available for subsequent re-evaluations
        if let Some(value) = self.func_return_values.get(&current_position) {
            return Ok(Some(value.clone_with_heap(heap)));
        }

        Ok(None)
    }

    /// Caches a return value from a user-defined function that completed after internal external calls.
    ///
    /// Unlike `push_ext_return_value`, this stores the value in a map by position for direct lookup.
    /// This is used when a function frame completes and needs to cache its return value for the
    /// caller to find when re-evaluating the call expression.
    pub fn set_func_return_value(&mut self, call_position: CodeRange, return_value: Value) {
        self.func_return_values.insert(call_position, return_value);
    }

    /// Takes a cached return value for a user-defined function call if one exists.
    ///
    /// This is used for checking cached return values from user-defined functions BEFORE
    /// evaluating arguments. When a function containing external calls completes, its return
    /// value is stored in `func_return_values` by call position.
    ///
    /// Returns the cached value if found, removing it from the cache.
    pub fn take_func_return_value(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        current_position: CodeRange,
    ) -> Option<Value> {
        self.func_return_values
            .remove(&current_position)
            .map(|v| v.clone_with_heap(heap))
    }

    /// Gets a cached return value for a call if one exists.
    ///
    /// This is used for checking cached return values BEFORE evaluating arguments.
    /// When a function or external call completes, its return value is stored by position.
    /// This allows skipping re-evaluation when the same call is encountered during resumption.
    ///
    /// Values are stored in `func_return_values` (a map keyed by position) and are NOT
    /// removed when retrieved. This allows multiple re-evaluations to find the same cached
    /// value. Values are cleared when appropriate (e.g., when a function completes).
    pub fn take_ext_return_value_exact(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        current_position: CodeRange,
    ) -> RunResult<Option<Value>> {
        // Check for pending exception first
        if let Some(exc) = self.ext_exception.take() {
            return Err(RunError::Exc(exc));
        }

        // Check the func_return_values map - this holds all cached return values by position.
        // Clone the value (don't remove) so it's available for subsequent re-evaluations.
        if let Some(value) = self.func_return_values.get(&current_position) {
            return Ok(Some(value.clone_with_heap(heap)));
        }

        Ok(None)
    }

    /// Clears the external return values, exception, and resets the pointer.
    ///
    /// This is called in control flow (for loops, if statements) to clear stale return
    /// values from condition evaluation before entering the body. Does NOT clear
    /// `func_return_values` or `argument_cache` since those are needed for proper
    /// function call caching.
    #[cfg(not(feature = "ref-count-panic"))]
    pub fn clear_ext_return_values(&mut self, _heap: &mut Heap<impl ResourceTracker>) {
        self.ext_return_values.clear();
        self.next_ext_return_value = 0;
        self.ext_exception = None;
    }

    /// Version with proper value dropping for ref-count-panic feature.
    /// See non-feature version for documentation.
    #[cfg(feature = "ref-count-panic")]
    pub fn clear_ext_return_values(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        for (_, value) in &mut self.ext_return_values {
            let v = std::mem::replace(value, Value::Dereferenced);
            v.drop_with_heap(heap);
        }
        self.ext_return_values.clear();
        self.next_ext_return_value = 0;
        self.ext_exception = None;
    }

    /// Clears all cached values when a function frame completes.
    ///
    /// This is called when a function completes to clean up cached values from
    /// internal external calls. Clears:
    /// - `ext_return_values` - external call return values
    /// - `func_return_values` - prevents the caller from seeing the completed function's
    ///   internal cached values (fixes recursion bug)
    /// - `argument_cache` - prevents outer recursion levels' cached args from being used
    ///   by inner levels on resume (fixes argument cache collision in recursion)
    ///
    /// The function's return value is stored AFTER this call (in `set_func_return_value`),
    /// so clearing here is safe - the caller will still find the return value.
    #[cfg(not(feature = "ref-count-panic"))]
    pub fn clear_on_function_complete(&mut self, _heap: &mut Heap<impl ResourceTracker>) {
        self.ext_return_values.clear();
        self.next_ext_return_value = 0;
        self.ext_exception = None;
        self.func_return_values.clear();
        self.argument_cache = None;
    }

    /// Version with proper value dropping for ref-count-panic feature.
    /// See non-feature version for documentation.
    #[cfg(feature = "ref-count-panic")]
    pub fn clear_on_function_complete(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        for (_, value) in &mut self.ext_return_values {
            let v = std::mem::replace(value, Value::Dereferenced);
            v.drop_with_heap(heap);
        }
        self.ext_return_values.clear();
        self.next_ext_return_value = 0;
        self.ext_exception = None;
        for (_, value) in self.func_return_values.drain() {
            value.drop_with_heap(heap);
        }
        if let Some(cache) = self.argument_cache.take() {
            for value in cache.evaluated_args {
                value.drop_with_heap(heap);
            }
        }
    }

    /// Clears all cached return values after a statement completes.
    ///
    /// This is called after each statement in iter mode to ensure cached values
    /// don't persist across statements (e.g., across loop iterations).
    /// Clears both ext_return_values and func_return_values.
    #[cfg(not(feature = "ref-count-panic"))]
    pub fn clear_statement_cache(&mut self, _heap: &mut Heap<impl ResourceTracker>) {
        self.ext_return_values.clear();
        self.next_ext_return_value = 0;
        self.ext_exception = None;
        self.func_return_values.clear();
        self.argument_cache = None;
    }

    /// Clears all cached return values after a statement completes.
    /// Version with proper value dropping for ref-count-panic feature.
    #[cfg(feature = "ref-count-panic")]
    pub fn clear_statement_cache(&mut self, heap: &mut Heap<impl ResourceTracker>) {
        for (_, value) in &mut self.ext_return_values {
            let v = std::mem::replace(value, Value::Dereferenced);
            v.drop_with_heap(heap);
        }
        self.ext_return_values.clear();
        self.next_ext_return_value = 0;
        self.ext_exception = None;
        for (_, value) in self.func_return_values.drain() {
            value.drop_with_heap(heap);
        }
        if let Some(cache) = self.argument_cache.take() {
            for value in cache.evaluated_args {
                value.drop_with_heap(heap);
            }
        }
    }

    /// Sets the argument cache for partially-evaluated arguments.
    ///
    /// Called when an external call suspends during argument evaluation to save
    /// the already-evaluated arguments for later resumption.
    pub fn set_argument_cache(&mut self, cache: ArgumentCache) {
        self.argument_cache = Some(cache);
    }

    /// Takes the argument cache if it matches the given call position.
    ///
    /// Returns the cached partially-evaluated arguments if this is the correct
    /// call site, allowing resumption without re-evaluating side effects.
    /// Returns `None` if no cache exists or the position doesn't match.
    pub fn take_argument_cache(&mut self, call_position: CodeRange) -> Option<ArgumentCache> {
        if let Some(cache) = &self.argument_cache {
            if cache.call_position == call_position {
                return self.argument_cache.take();
            }
        }
        None
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
        // Check recursion depth BEFORE memory allocation (fail fast)
        // Depth excludes global namespace (stack[0]), so current depth = stack.len() - 1
        let current_depth = self.stack.len() - 1;
        heap.tracker().check_recursion_depth(current_depth)?;

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
        // Clean up any cached function return values
        for value in std::mem::take(&mut self.func_return_values).into_values() {
            value.drop_with_heap(heap);
        }
        // Clean up any cached arguments
        if let Some(cache) = self.argument_cache.take() {
            for value in cache.evaluated_args {
                value.drop_with_heap(heap);
            }
        }
        // Clear any pending exception
        self.ext_exception = None;
    }

    /// Looks up a variable by name in the appropriate namespace based on the scope index for mutation.
    ///
    /// # Arguments
    /// * `local_idx` - Index of the local namespace in namespaces
    /// * `ident` - The identifier to look up (contains heap_id and scope)
    /// * `interns` - String storage for looking up variable names in error messages
    ///
    /// # Returns
    /// A mutable reference to the Value at the identifier's location, or NameError if undefined.
    pub fn get_var_mut(
        &mut self,
        local_idx: NamespaceId,
        ident: &Identifier,
        interns: &Interns,
    ) -> RunResult<&mut Value> {
        let ns_idx = match ident.scope {
            NameScope::Local => local_idx,
            NameScope::Global => GLOBAL_NS_IDX,
            NameScope::Cell => {
                // Cell access should use get_var_value which handles cell dereferencing
                panic!("Cell access should use get_var_value, not get_var_mut");
            }
        };
        let namespace = self.get_mut(ns_idx);

        if let Some(value) = namespace.0.get_mut(ident.namespace_id().index()) {
            if !matches!(value, Value::Undefined) {
                return Ok(value);
            }
        }
        Err(ExcType::name_error(interns.get_str(ident.name_id))
            .with_position(ident.position)
            .into())
    }

    /// Looks up a variable by name in the appropriate namespace based on the scope index.
    ///
    /// # Arguments
    /// * `local_idx` - Index of the local namespace in namespaces
    /// * `ident` - The identifier to look up (contains heap_id and scope)
    /// * `interns` - String storage for looking up variable names in error messages
    ///
    /// # Returns
    /// An immutable reference to the Value at the identifier's location, or NameError if undefined.
    pub fn get_var(&self, local_idx: NamespaceId, ident: &Identifier, interns: &Interns) -> RunResult<&Value> {
        let ns_idx = match ident.scope {
            NameScope::Local => local_idx,
            NameScope::Global => GLOBAL_NS_IDX,
            NameScope::Cell => {
                // Cell access should use get_var_value which handles cell dereferencing
                panic!("Cell access should use get_var_value, not get_var_mut");
            }
        };
        let namespace = self.get(ns_idx);

        if let Some(value) = namespace.0.get(ident.namespace_id().index()) {
            if !matches!(value, Value::Undefined) {
                return Ok(value);
            }
        }
        Err(ExcType::name_error(interns.get_str(ident.name_id))
            .with_position(ident.position)
            .into())
    }

    /// Gets a variable's value, handling Local, Global, and Cell scopes.
    ///
    /// This is the primary method for reading variable values during expression evaluation.
    /// It handles all scope types:
    /// - `Local` - reads directly from the local namespace
    /// - `Global` - reads directly from the global namespace (index 0)
    /// - `Cell` - namespace slot contains `Value::Ref(cell_id)`, reads through the cell
    ///
    /// # Arguments
    /// * `local_idx` - Index of the local namespace in namespaces
    /// * `heap` - The heap for cell access and cloning ref-counted values
    /// * `ident` - The identifier to look up (contains heap_id and scope)
    /// * `interns` - String storage for looking up variable names in error messages
    ///
    /// # Returns
    /// A cloned copy of the value (with refcount incremented for Ref values), or NameError if undefined.
    pub fn get_var_value(
        &self,
        local_idx: NamespaceId,
        heap: &mut Heap<impl ResourceTracker>,
        ident: &Identifier,
        interns: &Interns,
    ) -> RunResult<Value> {
        // Determine which namespace to use
        let ns_idx = match ident.scope {
            NameScope::Global => GLOBAL_NS_IDX,
            _ => local_idx, // Local and Cell both use local namespace
        };

        match ident.scope {
            NameScope::Cell => {
                // Cell access - namespace slot contains Value::Ref(cell_id)
                let namespace = &self.stack[ns_idx.index()];
                if let Value::Ref(cell_id) = namespace.get(ident.namespace_id()) {
                    let value = heap.get_cell_value(*cell_id);
                    // Cell may be undefined if accessed before assignment in enclosing scope
                    if matches!(value, Value::Undefined) {
                        let name = interns.get_str(ident.name_id);
                        Err(ExcType::name_error_free_variable(name).into())
                    } else {
                        Ok(value)
                    }
                } else {
                    panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug");
                }
            }
            _ => {
                // Local or Global scope - direct namespace access
                self.get_var(ns_idx, ident, interns)
                    .map(|object| object.clone_with_heap(heap))
            }
        }
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
        self.stack.iter().flat_map(|namespace| {
            namespace
                .0
                .iter()
                .filter_map(|value| if let Value::Ref(id) = value { Some(*id) } else { None })
        })
    }
}
