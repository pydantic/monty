use std::mem;

use ahash::{AHashMap, AHashSet};

use crate::{
    args::{ArgExprs, CallArg, CallKwarg},
    builtins::Builtins,
    expressions::{
        AssignTarget, Callable, CmpOperator, Comprehension, DictItem, Expr, ExprLoc, Identifier, ImportName, Literal,
        NameScope, Node, Operator, PreparedFunctionDef, PreparedNode, SequenceItem, UnpackTarget,
    },
    fstring::{FStringPart, FormatSpec},
    intern::{InternerBuilder, StringId},
    namespace::NamespaceId,
    parse::{CodeRange, ExceptHandler, ParseError, ParseNode, ParseResult, ParsedSignature, RawFunctionDef, Try},
    signature::Signature,
};

/// Builds the `ParseError` raised when a scope's namespace would exceed
/// `NamespaceId`'s `u16` capacity (the bytecode slot operand width).
/// Hoisted so the cold error path stays out of inlined allocator calls.
#[cold]
#[inline(never)]
fn namespace_overflow(position: CodeRange) -> ParseError {
    ParseError::syntax(
        format!("too many distinct names in scope; maximum is {} per scope", u16::MAX),
        position,
    )
}

/// Mutable handle to the module's global name map + slot counter, threaded
/// through nested function preparers so an inner `global X` discovery can
/// allocate a module slot at the point of discovery.
///
/// This replaces the previous "snapshot the module's name_map per function,
/// collect discovered globals into a `discovered_globals` set, materialize
/// post-hoc" bubble-up. With the live borrow we just allocate eagerly: case 1
/// of `get_id` in a function calls `ensure_slot` directly on the module's
/// store, and `prepare_function_def` has nothing to bubble up.
struct GlobalsRef<'g> {
    name_map: &'g mut AHashMap<String, NamespaceId>,
    namespace_size: &'g mut usize,
}

impl GlobalsRef<'_> {
    /// Returns the slot for `name`, allocating a new one if absent.
    fn ensure_slot(&mut self, name: &str, position: CodeRange) -> Result<NamespaceId, ParseError> {
        if let Some(&id) = self.name_map.get(name) {
            return Ok(id);
        }
        let id = NamespaceId::new(*self.namespace_size).ok_or_else(|| namespace_overflow(position))?;
        self.name_map.insert(name.to_owned(), id);
        *self.namespace_size += 1;
        Ok(id)
    }

    /// Re-borrows for shorter-lived use (e.g. passing to a nested inner preparer).
    fn reborrow(&mut self) -> GlobalsRef<'_> {
        GlobalsRef {
            name_map: self.name_map,
            namespace_size: self.namespace_size,
        }
    }
}

/// Result of the prepare phase, containing everything needed to compile and execute code.
///
/// This struct holds the outputs of name resolution and AST transformation:
/// - The namespace size (number of slots needed at module level)
/// - A mapping from variable names to their namespace indices (for ref-count testing)
/// - The transformed AST nodes with all names resolved, ready for compilation
/// - The string interner containing all interned identifiers and filenames
pub struct PrepareResult {
    /// Number of items in the namespace (at module level, this IS the global namespace)
    pub namespace_size: usize,
    /// Maps variable names to their indices in the namespace.
    ///
    /// This map is used by:
    /// - ref-count tests for looking up variables by name
    /// - REPL incremental compilation to preserve stable global slot IDs across snippets
    pub name_map: AHashMap<String, NamespaceId>,
    /// The prepared AST nodes with all names resolved to namespace indices.
    /// Function definitions are inline as `PreparedFunctionDef` variants.
    pub nodes: Vec<PreparedNode>,
    /// The string interner containing all interned identifiers and filenames.
    pub interner: InternerBuilder,
}

/// Prepares parsed nodes for compilation by resolving names and building the initial namespace.
///
/// The namespace will be converted to runtime Objects when execution begins and the heap is available.
/// At module level, the local namespace IS the global namespace.
pub(crate) fn prepare(parse_result: ParseResult, input_names: Vec<String>) -> Result<PrepareResult, ParseError> {
    let ParseResult { nodes, interner } = parse_result;
    let mut p = Prepare::new_module(input_names, &interner)?;
    let mut prepared_nodes = p.prepare_nodes(nodes)?;

    // In the root frame, the last expression is implicitly returned
    // if it's not None. This matches Python REPL behavior where the last expression
    // value is displayed/returned.
    if let Some(Node::Expr(expr_loc)) = prepared_nodes.last()
        && !expr_loc.expr.is_none()
    {
        let new_expr_loc = expr_loc.clone();
        prepared_nodes.pop();
        prepared_nodes.push(Node::Return(Some(new_expr_loc)));
    }

    Ok(PrepareResult {
        namespace_size: p.namespace_size,
        name_map: p.name_map,
        nodes: prepared_nodes,
        interner,
    })
}

/// Prepares parsed nodes for REPL-style incremental compilation using an existing global namespace map.
///
/// Existing bindings keep their original namespace slots; any new names are appended with new slots.
/// This ensures snippets can be compiled independently while sharing one persistent global namespace.
pub(crate) fn prepare_with_existing_names(
    parse_result: ParseResult,
    existing_name_map: AHashMap<String, NamespaceId>,
) -> Result<PrepareResult, ParseError> {
    let ParseResult { nodes, interner } = parse_result;
    let mut p = Prepare::new_module_with_name_map(existing_name_map, &interner);
    let mut prepared_nodes = p.prepare_nodes(nodes)?;

    // In the root frame, the last expression is implicitly returned to match REPL behavior.
    if let Some(Node::Expr(expr_loc)) = prepared_nodes.last()
        && !expr_loc.expr.is_none()
    {
        let new_expr_loc = expr_loc.clone();
        prepared_nodes.pop();
        prepared_nodes.push(Node::Return(Some(new_expr_loc)));
    }

    Ok(PrepareResult {
        namespace_size: p.namespace_size,
        name_map: p.name_map,
        nodes: prepared_nodes,
        interner,
    })
}

/// State machine for the preparation phase that transforms parsed AST nodes into a prepared form.
///
/// This struct maintains the mapping between variable names and their namespace indices,
/// and handles scope resolution. The preparation phase is crucial for converting string-based
/// name lookups into efficient integer-indexed namespace access during compilation and execution.
///
/// For functions, this struct also tracks:
/// - Which variables are declared `global` (should resolve to module namespace)
/// - Which variables are declared `nonlocal` (should resolve to enclosing scope via cells)
/// - Which variables are assigned locally (determines local vs global scope)
/// - Reference to the global name map for resolving global variable references
/// - Enclosing scope information for closure analysis
struct Prepare<'i, 'g> {
    /// Reference to the string interner for looking up names in error messages.
    interner: &'i InternerBuilder,
    /// Maps variable names to their indices in this scope's namespace vector
    name_map: AHashMap<String, NamespaceId>,
    /// Number of items in the namespace
    pub namespace_size: usize,
    /// Names declared as `global` in this scope.
    /// These names will resolve to the global namespace instead of local.
    global_names: AHashSet<String>,
    /// Names that are assigned in this scope (from first-pass scan).
    /// Used in functions to determine if a variable is local (assigned) or global (only read).
    assigned_names: AHashSet<String>,
    /// Names that have been assigned so far during the second pass (in order).
    /// Used to produce the correct error message for `global x` when x was assigned before.
    names_assigned_in_order: AHashSet<String>,
    /// Live mutable handle to the module-level global name map (and its slot
    /// counter), threaded down from the module preparer. Used by functions to
    /// resolve global variable references and to allocate new module slots
    /// at the point of `global X` discovery without a separate bubble-up pass.
    ///
    /// `None` at module level: the module preparer's own `name_map` /
    /// `namespace_size` ARE the module globals, so there's nothing to borrow.
    global_name_map: Option<GlobalsRef<'g>>,
    /// Names that exist as locals in the enclosing function scope.
    /// Used to validate `nonlocal` declarations and resolve captured variables.
    /// None at module level or when there's no enclosing function.
    enclosing_locals: Option<AHashSet<String>>,
    /// This scope's own parameter names.
    ///
    /// Used (together with `assigned_names`) when a nested function captures a
    /// variable, to decide whether *this* scope owns the cell (the name is a
    /// local/param here) or merely passes a captured cell through from further
    /// up. Empty at module scope.
    params: AHashSet<String>,
    /// Maps free variable names (from nonlocal declarations and implicit captures) to their
    /// index in the free_vars vector. Pre-populated with nonlocal names at initialization,
    /// then extended with implicit captures discovered during preparation.
    free_var_map: AHashMap<String, NamespaceId>,
    /// Maps cell variable names to their index in the owned_cells vector.
    /// Pre-populated with cell_var names at initialization (excluding pass-through variables
    /// that are both nonlocal and captured by nested functions), then extended as new
    /// captures are discovered during nested function preparation.
    cell_var_map: AHashMap<String, NamespaceId>,
    /// Number of comprehension-variable slots currently in use (inside a comprehension).
    ///
    /// Allocated bottom-up as comprehension target names are encountered,
    /// released back into the pool when the surrounding comprehension
    /// finishes. Each allocation gets a function-wide unique slot ID; the
    /// compiler maps that ID to an operand-stack offset at emission time
    /// (`Compiler::slot_offsets`).
    comp_var_depth: u16,
    /// Stack of comprehension-name → comp-var-slot maps for the currently active comprehensions.
    ///
    /// Pushed on entry to a comprehension, popped on exit. Read by `get_id` (the
    /// **expression-position** read path) before falling through to the regular
    /// name-resolution cascade so a comprehension target shadows any same-named
    /// enclosing binding. Walrus and other assignment-position stores must bypass
    /// this stack (see [`Prepare::get_id_for_store_target`]) so PEP 572 binding
    /// semantics are preserved.
    comp_name_scopes: Vec<AHashMap<String, u16>>,
}

impl<'i, 'g> Prepare<'i, 'g> {
    /// Allocates the next namespace slot, incrementing `namespace_size`.
    ///
    /// Wraps the recurring `let id = NamespaceId::new(self.namespace_size);
    /// self.namespace_size += 1;` pattern and surfaces a clean `ParseError`
    /// if the scope is about to grow past `NamespaceId`'s `u16` capacity.
    /// Anchors the error to `position` so the traceback caret lands on the
    /// statement that triggered the overflow.
    fn alloc_slot(&mut self, position: CodeRange) -> Result<NamespaceId, ParseError> {
        let id = NamespaceId::new(self.namespace_size).ok_or_else(|| namespace_overflow(position))?;
        self.namespace_size += 1;
        Ok(id)
    }

    /// Returns `true` if this preparer is for module-level code.
    ///
    /// Module scope is defined by the absence of a `global_name_map`: a
    /// function preparer always borrows the module's name map (so it can
    /// resolve / allocate globals) while the module preparer IS the module's
    /// name map. Constructors maintain this invariant, so the absence of the
    /// borrow is an unambiguous discriminator.
    fn is_module_scope(&self) -> bool {
        self.global_name_map.is_none()
    }

    /// Builds the set of names a nested scope (function or lambda) defined
    /// inside this scope may capture from enclosing scopes.
    ///
    /// This is the **transitive** set: our own params + assigned locals + every
    /// name already in our `name_map`/`free_var_map`, **plus** everything we
    /// ourselves can capture (`enclosing_locals`). Threading `enclosing_locals`
    /// through is what lets a deeply nested function capture a variable several
    /// levels up: without it, an intermediate scope that doesn't itself mention
    /// the variable would hide it from its own children. Empty at module scope
    /// (module globals are reached via `global`, not closure capture).
    ///
    /// Names this scope declares `global` are excluded: such a name is not a
    /// local binding here, so a nested function referencing it must resolve to
    /// the module global rather than capture a (non-existent) cell — e.g.
    /// `def mid(): global x; x = 1; def inner(): return x` reads the global `x`.
    fn child_enclosing_locals(&self) -> AHashSet<String> {
        if self.is_module_scope() {
            AHashSet::new()
        } else {
            let mut locals = self.assigned_names.clone();
            locals.extend(self.params.iter().cloned());
            locals.extend(self.name_map.keys().cloned());
            locals.extend(self.free_var_map.keys().cloned());
            if let Some(ref enclosing) = self.enclosing_locals {
                locals.extend(enclosing.iter().cloned());
            }
            locals.retain(|name| !self.global_names.contains(name));
            locals
        }
    }

    /// Records that a nested scope captured `name` from us, placing the cell
    /// slot in the correct map.
    ///
    /// If `name` is one of our own bindings (a parameter or assigned local) we
    /// **own** the cell: it goes in `cell_var_map` and a fresh `Cell` is created
    /// here at call time. Otherwise `name` belongs to a scope further out and
    /// merely passes *through* us: it goes in `free_var_map`, so we in turn
    /// capture the cell from our own enclosing scope and forward it on. This
    /// pass-through classification is what makes multi-level closures work —
    /// misfiling a pass-through as an owned cell would hand the nested scope a
    /// fresh, disconnected cell instead of the real variable.
    ///
    /// Names already recorded (as a cell or free var) are left untouched.
    fn promote_captured_var(&mut self, name: &str, position: CodeRange) -> Result<(), ParseError> {
        if self.cell_var_map.contains_key(name) || self.free_var_map.contains_key(name) {
            return Ok(());
        }
        let slot = if let Some(&existing) = self.name_map.get(name) {
            existing
        } else {
            let slot = self.alloc_slot(position)?;
            self.name_map.insert(name.to_string(), slot);
            slot
        };
        if self.assigned_names.contains(name) || self.params.contains(name) {
            self.cell_var_map.insert(name.to_string(), slot);
        } else {
            self.free_var_map.insert(name.to_string(), slot);
        }
        Ok(())
    }

    /// Builds the parallel free-var slot vectors for a nested scope from its
    /// captured-variable map (`name -> the nested scope's own slot`).
    ///
    /// Returns `(free_var_slots, free_var_enclosing_slots)`: the first holds the
    /// nested scope's own slots (where it installs each captured cell at call
    /// time); the second holds *our* slots it reads those cells from when the
    /// closure is built. Both are ordered by the nested slot so they stay
    /// index-aligned. Panics if a captured name is neither one of our owned
    /// cells nor one of our own free vars — that would be a preparation bug
    /// (the capture should have been recorded by [`Self::promote_captured_var`]).
    fn build_free_var_slots(
        &self,
        inner_free_var_map: AHashMap<String, NamespaceId>,
    ) -> (Vec<NamespaceId>, Vec<NamespaceId>) {
        let mut entries: Vec<_> = inner_free_var_map.into_iter().collect();
        entries.sort_by_key(|(_, inner_slot)| *inner_slot);
        let inner_slots = entries.iter().map(|(_, slot)| *slot).collect();
        let enclosing_slots = entries
            .into_iter()
            .map(|(var_name, _)| {
                if let Some(&slot) = self.cell_var_map.get(&var_name) {
                    slot
                } else if let Some(&slot) = self.free_var_map.get(&var_name) {
                    slot
                } else {
                    panic!("free_var '{var_name}' not found in enclosing scope's cell_var_map or free_var_map");
                }
            })
            .collect();
        (inner_slots, enclosing_slots)
    }

    /// # Arguments
    /// * `input_names` - Names that should be pre-registered in the namespace (e.g., input variables)
    /// * `interner` - Reference to the string interner for looking up names
    ///
    /// Returns a `ParseError` if more than `u16::MAX + 1` input names are
    /// supplied — bytecode slot indices are `u16` so the namespace cannot grow
    /// past that. Practically only reachable via misuse by the embedder, since
    /// `input_names` is supplied programmatically, not from user source.
    fn new_module(input_names: Vec<String>, interner: &'i InternerBuilder) -> Result<Self, ParseError> {
        let mut name_map = AHashMap::with_capacity(input_names.len());
        for (index, name) in input_names.into_iter().enumerate() {
            let slot = NamespaceId::new(index).ok_or_else(|| namespace_overflow(CodeRange::default()))?;
            name_map.insert(name, slot);
        }
        let namespace_size = name_map.len();
        Ok(Self {
            interner,
            name_map,
            namespace_size,
            global_names: AHashSet::new(),
            assigned_names: AHashSet::new(),
            names_assigned_in_order: AHashSet::new(),
            global_name_map: None,
            enclosing_locals: None,
            params: AHashSet::new(),
            free_var_map: AHashMap::new(),
            cell_var_map: AHashMap::new(),
            comp_var_depth: 0,
            comp_name_scopes: Vec::new(),
        })
    }

    /// Creates a module-scope Prepare instance from an existing global name map.
    ///
    /// Used by incremental REPL compilation to keep stable slot assignments across snippets.
    fn new_module_with_name_map(name_map: AHashMap<String, NamespaceId>, interner: &'i InternerBuilder) -> Self {
        let namespace_size = name_map
            .values()
            .map(|id| id.index())
            .max()
            .map_or(0, |max_idx| max_idx + 1);

        Self {
            interner,
            name_map,
            namespace_size,
            global_names: AHashSet::new(),
            assigned_names: AHashSet::new(),
            names_assigned_in_order: AHashSet::new(),
            global_name_map: None,
            enclosing_locals: None,
            params: AHashSet::new(),
            free_var_map: AHashMap::new(),
            cell_var_map: AHashMap::new(),
            comp_var_depth: 0,
            comp_name_scopes: Vec::new(),
        }
    }

    /// Creates a new Prepare instance for function-level code.
    ///
    /// Pre-populates `free_var_map` with nonlocal declarations and implicit captures,
    /// and `cell_var_map` with cell variables (excluding pass-through variables).
    ///
    /// # Arguments
    /// * `capacity` - Expected number of nodes
    /// * `params` - Function parameter StringIds (pre-registered in namespace)
    /// * `assigned_names` - Names that are assigned in this function (from first-pass scan)
    /// * `global_names` - Names declared as `global` in this function
    /// * `nonlocal_names` - Names declared as `nonlocal` in this function
    /// * `implicit_captures` - Names captured from enclosing scope without explicit nonlocal
    /// * `global_name_map` - Copy of the module-level name map for global resolution
    /// * `enclosing_locals` - Names that exist as locals in the enclosing function (for nonlocal resolution)
    /// * `cell_var_names` - Names that are captured by nested functions (must be stored in cells)
    /// * `interner` - Reference to the string interner for looking up names
    #[expect(clippy::too_many_arguments)]
    fn new_function(
        capacity: usize,
        params: &[StringId],
        position: CodeRange,
        assigned_names: AHashSet<String>,
        global_names: AHashSet<String>,
        nonlocal_names: AHashSet<String>,
        implicit_captures: AHashSet<String>,
        global_name_map: GlobalsRef<'g>,
        enclosing_locals: Option<AHashSet<String>>,
        cell_var_names: AHashSet<String>,
        interner: &'i InternerBuilder,
    ) -> Result<Self, ParseError> {
        // Reject duplicate parameter names while building the name_map.
        // Ruff's parser accepts `def f(x, x)` that CPython rejects at compile
        // time; without this check, `name_map` is deduplicated by HashMap
        // semantics but each `NamespaceId` comes from the positional index,
        // so the duplicate slot lands past the allocated stack region and
        // panics `load_local` at runtime.
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, string_id) in params.iter().enumerate() {
            let name_str = interner.get_str(*string_id);
            let slot = NamespaceId::new(index).ok_or_else(|| namespace_overflow(position))?;
            if name_map.insert(name_str.to_string(), slot).is_some() {
                return Err(ParseError::syntax(
                    format!("duplicate argument '{name_str}' in function definition"),
                    position,
                ));
            }
        }
        let namespace_size = name_map.len();

        // Namespace layout: params occupy slots `0..namespace_size`, then cell
        // vars, captured free vars, and ordinary locals follow. This pass assigns
        // slots in that *order*, but the regions are no longer guaranteed
        // contiguous: a transitively captured (pass-through) free var can be
        // discovered late and assigned a slot in the locals region. Slots are
        // therefore carried explicitly (`cell_var_slots`/`free_var_slots` on
        // `Function`) and installed individually at frame setup rather than by
        // sequential construction (see `install_closure_cells`).

        // Pre-populate cell_var_map with cell variables FIRST (right after params).
        // Excludes pass-through variables (names that are both nonlocal and captured by
        // nested functions - these stay in free_var_map since we receive the cell, not create it).
        // NOTE: We intentionally do NOT add these to name_map here, because the scope
        // validation checks name_map to detect "used before declaration" errors
        let mut cell_var_map = AHashMap::with_capacity(cell_var_names.len());
        let mut namespace_size = namespace_size;
        for name in cell_var_names {
            if !nonlocal_names.contains(&name) && !implicit_captures.contains(&name) {
                let slot = NamespaceId::new(namespace_size).ok_or_else(|| namespace_overflow(position))?;
                namespace_size += 1;
                cell_var_map.insert(name, slot);
            }
        }

        // Pre-populate free_var_map with nonlocal declarations AND implicit captures SECOND (after cell_vars).
        // Each entry maps name -> namespace slot index where the cell reference will be stored.
        // NOTE: We intentionally do NOT add these to name_map here, because the nonlocal
        // validation in prepare_nodes checks name_map to detect "used before nonlocal declaration"
        let free_var_capacity = nonlocal_names.len() + implicit_captures.len();
        let mut free_var_map = AHashMap::with_capacity(free_var_capacity);
        for name in nonlocal_names {
            let slot = NamespaceId::new(namespace_size).ok_or_else(|| namespace_overflow(position))?;
            namespace_size += 1;
            free_var_map.insert(name, slot);
        }
        // Implicit captures (variables accessed from enclosing scope without explicit nonlocal)
        for name in implicit_captures {
            let slot = NamespaceId::new(namespace_size).ok_or_else(|| namespace_overflow(position))?;
            namespace_size += 1;
            free_var_map.insert(name, slot);
        }

        Ok(Self {
            interner,
            name_map,
            namespace_size,
            global_names,
            assigned_names,
            names_assigned_in_order: AHashSet::new(),
            global_name_map: Some(global_name_map),
            enclosing_locals,
            params: params.iter().map(|id| interner.get_str(*id).to_string()).collect(),
            free_var_map,
            cell_var_map,
            comp_var_depth: 0,
            comp_name_scopes: Vec::new(),
        })
    }

    /// Recursively prepares a sequence of AST nodes by resolving names and transforming expressions.
    ///
    /// This method processes each node type differently:
    /// - Resolves variable names to namespace indices
    /// - Transforms function calls from identifier-based to builtin type-based
    /// - Handles special cases like implicit returns in root frames
    /// - Validates that names used in attribute calls are already defined
    ///
    /// # Returns
    /// A vector of prepared nodes ready for compilation
    fn prepare_nodes(&mut self, nodes: Vec<ParseNode>) -> Result<Vec<PreparedNode>, ParseError> {
        let nodes_len = nodes.len();
        let mut new_nodes = Vec::with_capacity(nodes_len);
        for node in nodes {
            match node {
                Node::Pass => (),
                Node::Expr(expr) => new_nodes.push(Node::Expr(self.prepare_expression(expr)?)),
                Node::Return(expr) => new_nodes.push(Node::Return(match expr {
                    Some(expr) => Some(self.prepare_expression(expr)?),
                    None => None,
                })),
                Node::Raise(exc) => {
                    let expr = match exc {
                        Some(expr) => {
                            let prepared = self.prepare_expression(expr)?;
                            match prepared.expr {
                                // Handle raising a builtin exception type without instantiation,
                                // e.g. `raise TypeError`. Transform into `raise TypeError()`
                                // so the exception is properly instantiated before being raised.
                                Expr::Builtin(b) => {
                                    let call_expr = Expr::Call {
                                        callable: Callable::Builtin(b),
                                        args: Box::new(ArgExprs::Empty),
                                    };
                                    Some(ExprLoc::new(prepared.position, call_expr))
                                }
                                _ => Some(prepared),
                            }
                        }
                        None => None,
                    };
                    new_nodes.push(Node::Raise(expr));
                }
                Node::Assert { test, msg } => {
                    let test = self.prepare_expression(test)?;
                    let msg = match msg {
                        Some(m) => Some(self.prepare_expression(m)?),
                        None => None,
                    };
                    new_nodes.push(Node::Assert { test, msg });
                }
                Node::Assign { target, object } => {
                    let object = self.prepare_expression(object)?;
                    // Track that this name was assigned before we call get_id
                    self.names_assigned_in_order
                        .insert(self.interner.get_str(target.name_id).to_string());
                    let target = self.get_id(target)?;
                    new_nodes.push(Node::Assign { target, object });
                }
                Node::UnpackAssign {
                    targets,
                    targets_position,
                    object,
                } => {
                    let object = self.prepare_expression(object)?;
                    // Recursively resolve all targets (supports nested tuples)
                    let targets = targets
                        .into_iter()
                        .map(|target| self.prepare_unpack_target(target))
                        .collect::<Result<_, _>>()?;
                    new_nodes.push(Node::UnpackAssign {
                        targets,
                        targets_position,
                        object,
                    });
                }
                Node::OpAssign { target, op, value } => {
                    // Track that this name was assigned
                    self.names_assigned_in_order
                        .insert(self.interner.get_str(target.name_id).to_string());
                    let target = self.get_id(target)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::OpAssign { target, op, value });
                }
                Node::SubscriptOpAssign {
                    target,
                    index,
                    op,
                    value,
                    target_position,
                } => {
                    let target = self.prepare_expression(target)?;
                    let index = self.prepare_expression(index)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::SubscriptOpAssign {
                        target,
                        index,
                        op,
                        value,
                        target_position,
                    });
                }
                Node::SubscriptAssign {
                    target,
                    index,
                    value,
                    target_position,
                } => {
                    // SubscriptAssign doesn't assign to the target itself, just modifies it
                    let target = self.prepare_expression(target)?;
                    let index = self.prepare_expression(index)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::SubscriptAssign {
                        target,
                        index,
                        value,
                        target_position,
                    });
                }
                Node::AttrOpAssign {
                    object,
                    attr,
                    op,
                    value,
                    target_position,
                } => {
                    let object = self.prepare_expression(object)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::AttrOpAssign {
                        object,
                        attr,
                        op,
                        value,
                        target_position,
                    });
                }
                Node::AttrAssign {
                    object,
                    attr,
                    target_position,
                    value,
                } => {
                    // AttrAssign doesn't assign to the object itself, just modifies its attribute
                    let object = self.prepare_expression(object)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::AttrAssign {
                        object,
                        attr,
                        target_position,
                        value,
                    });
                }
                Node::ChainAssign { targets, object } => {
                    // Prepare the single shared right-hand side, then prepare each
                    // target in left-to-right order so name-assignment tracking matches
                    // the source order (`a = b = 1` assigns `a` then `b`).
                    let object = self.prepare_expression(object)?;
                    let targets = targets
                        .into_iter()
                        .map(|t| self.prepare_assign_target(t))
                        .collect::<Result<Vec<_>, _>>()?;
                    new_nodes.push(Node::ChainAssign { targets, object });
                }
                Node::For {
                    target,
                    iter,
                    body,
                    or_else,
                } => {
                    // Prepare target with normal scoping (not comprehension isolation)
                    let target = self.prepare_unpack_target(target)?;
                    new_nodes.push(Node::For {
                        target,
                        iter: self.prepare_expression(iter)?,
                        body: self.prepare_nodes(body)?,
                        or_else: self.prepare_nodes(or_else)?,
                    });
                }
                Node::Break { position } => {
                    new_nodes.push(Node::Break { position });
                }
                Node::Continue { position } => {
                    new_nodes.push(Node::Continue { position });
                }
                Node::While { test, body, or_else } => {
                    new_nodes.push(Node::While {
                        test: self.prepare_expression(test)?,
                        body: self.prepare_nodes(body)?,
                        or_else: self.prepare_nodes(or_else)?,
                    });
                }
                Node::If { test, body, or_else } => {
                    let test = self.prepare_expression(test)?;
                    let body = self.prepare_nodes(body)?;
                    let or_else = self.prepare_nodes(or_else)?;
                    new_nodes.push(Node::If { test, body, or_else });
                }
                Node::FunctionDef(RawFunctionDef {
                    name,
                    signature,
                    body,
                    is_async,
                }) => {
                    let func_node = self.prepare_function_def(name, &signature, body, is_async)?;
                    new_nodes.push(func_node);
                }
                Node::Global { names, position } => {
                    // At module level, `global` is a no-op since all variables are already global.
                    // In functions, the global declarations are already collected in the first pass
                    // (see prepare_function_def), so this is also a no-op at this point.
                    // The actual effect happens in get_id where we check global_names.
                    if !self.is_module_scope() {
                        // Validate that names weren't already used/assigned before `global` declaration
                        for string_id in names {
                            let name_str = self.interner.get_str(string_id);
                            if self.names_assigned_in_order.contains(name_str) {
                                // Name was assigned before the global declaration
                                return Err(ParseError::syntax(
                                    format!("name '{name_str}' is assigned to before global declaration"),
                                    position,
                                ));
                            } else if self.name_map.contains_key(name_str) {
                                // Name was used (but not assigned) before the global declaration
                                return Err(ParseError::syntax(
                                    format!("name '{name_str}' is used prior to global declaration"),
                                    position,
                                ));
                            }
                        }
                    }
                    // Global statements don't produce any runtime nodes
                }
                Node::Nonlocal { names, position } => {
                    // Nonlocal can only be used inside a function, not at module level
                    if self.is_module_scope() {
                        return Err(ParseError::syntax(
                            "nonlocal declaration not allowed at module level",
                            position,
                        ));
                    }
                    // Validate that names weren't already used/assigned before `nonlocal` declaration
                    // and that the binding exists in an enclosing scope
                    for string_id in names {
                        let name_str = self.interner.get_str(string_id);
                        if self.names_assigned_in_order.contains(name_str) {
                            // Name was assigned before the nonlocal declaration
                            return Err(ParseError::syntax(
                                format!("name '{name_str}' is assigned to before nonlocal declaration"),
                                position,
                            ));
                        } else if self.name_map.contains_key(name_str) {
                            // Name was used (but not assigned) before the nonlocal declaration
                            return Err(ParseError::syntax(
                                format!("name '{name_str}' is used prior to nonlocal declaration"),
                                position,
                            ));
                        }
                        // Validate that the binding exists in an enclosing scope
                        if let Some(ref enclosing) = self.enclosing_locals {
                            if !enclosing.contains(name_str) {
                                return Err(ParseError::syntax(
                                    format!("no binding for nonlocal '{name_str}' found"),
                                    position,
                                ));
                            }
                        } else {
                            // No enclosing scope (function defined at module level)
                            // The nonlocal must reference something in an enclosing function
                            return Err(ParseError::syntax(
                                format!("no binding for nonlocal '{name_str}' found"),
                                position,
                            ));
                        }
                    }
                    // Nonlocal statements don't produce any runtime nodes
                }
                Node::Try(Try {
                    body,
                    handlers,
                    or_else,
                    finally,
                }) => {
                    let body = self.prepare_nodes(body)?;
                    let handlers = handlers
                        .into_iter()
                        .map(|h| self.prepare_except_handler(h))
                        .collect::<Result<Vec<_>, _>>()?;
                    let or_else = self.prepare_nodes(or_else)?;
                    let finally = self.prepare_nodes(finally)?;
                    new_nodes.push(Node::Try(Try {
                        body,
                        handlers,
                        or_else,
                        finally,
                    }));
                }
                Node::With {
                    context,
                    target,
                    body,
                    position,
                } => {
                    let context = self.prepare_expression(context)?;
                    let target = match target {
                        Some(t) => Some(self.prepare_unpack_target(t)?),
                        None => None,
                    };
                    let body = self.prepare_nodes(body)?;
                    new_nodes.push(Node::With {
                        context,
                        target,
                        body,
                        position,
                    });
                }
                Node::Import { names } => {
                    let resolved_names = names
                        .into_iter()
                        .map(|import_name| -> Result<_, ParseError> {
                            // Each `import foo [as bar]` binds the alias (or module name)
                            // in the current scope.
                            self.names_assigned_in_order
                                .insert(self.interner.get_str(import_name.binding.name_id).to_string());
                            let resolved_binding = self.get_id(import_name.binding)?;
                            Ok(ImportName {
                                module_name: import_name.module_name,
                                binding: resolved_binding,
                            })
                        })
                        .collect::<Result<_, _>>()?;
                    new_nodes.push(Node::Import { names: resolved_names });
                }
                Node::ImportFrom {
                    module_name,
                    names,
                    position,
                } => {
                    let resolved_names = names
                        .into_iter()
                        .map(|(import_name, binding)| -> Result<_, ParseError> {
                            // Each imported name (or `as` alias) binds in the current scope.
                            self.names_assigned_in_order
                                .insert(self.interner.get_str(binding.name_id).to_string());
                            let resolved_binding = self.get_id(binding)?;
                            Ok((import_name, resolved_binding))
                        })
                        .collect::<Result<_, _>>()?;
                    new_nodes.push(Node::ImportFrom {
                        module_name,
                        names: resolved_names,
                        position,
                    });
                }
            }
        }
        Ok(new_nodes)
    }

    /// Prepares an exception handler by resolving names in the exception type and body.
    ///
    /// The exception variable (if present) is treated as an assigned name in the current scope.
    fn prepare_except_handler(
        &mut self,
        handler: ExceptHandler<ParseNode>,
    ) -> Result<ExceptHandler<PreparedNode>, ParseError> {
        let exc_type = match handler.exc_type {
            Some(expr) => Some(self.prepare_expression(expr)?),
            None => None,
        };
        // The exception variable binding (e.g., `as e:`) is an assignment
        let name = match handler.name {
            Some(ident) => {
                // Track that this name was assigned
                self.names_assigned_in_order
                    .insert(self.interner.get_str(ident.name_id).to_string());
                Some(self.get_id(ident)?)
            }
            None => None,
        };
        let body = self.prepare_nodes(handler.body)?;
        Ok(ExceptHandler { exc_type, name, body })
    }

    /// Prepares an expression by resolving names, transforming calls, and applying optimizations.
    ///
    /// Key transformations performed:
    /// - Name lookups are resolved to namespace indices via `get_id`
    /// - Function calls are resolved from identifiers to builtin types
    /// - Attribute calls validate that the object is already defined (not a new name)
    /// - Lists and tuples are recursively prepared
    /// - Modulo equality patterns like `x % n == k` (constant right-hand side) are optimized to
    ///   `CmpOperator::ModEq`
    ///
    /// # Errors
    /// Returns a NameError if an attribute call references an undefined variable
    fn prepare_expression(&mut self, loc_expr: ExprLoc) -> Result<ExprLoc, ParseError> {
        let ExprLoc { position, expr } = loc_expr;
        let expr = match expr {
            Expr::Literal(object) => Expr::Literal(object),
            Expr::Builtin(callable) => Expr::Builtin(callable),
            Expr::Name(name) => self.resolve_name_or_builtin(name)?,
            Expr::Op { left, op, right } => Expr::Op {
                left: Box::new(self.prepare_expression(*left)?),
                op,
                right: Box::new(self.prepare_expression(*right)?),
            },
            Expr::CmpOp { left, op, right } => Expr::CmpOp {
                left: Box::new(self.prepare_expression(*left)?),
                op,
                right: Box::new(self.prepare_expression(*right)?),
            },
            Expr::ChainCmp { left, comparisons } => Expr::ChainCmp {
                left: Box::new(self.prepare_expression(*left)?),
                comparisons: comparisons
                    .into_iter()
                    .map(|(op, expr)| Ok((op, self.prepare_expression(expr)?)))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Expr::Call { callable, mut args } => {
                // Prepare the arguments
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                // For Name callables, resolve the identifier in the namespace
                // Don't error here if undefined - let runtime raise NameError with proper traceback
                let callable = match callable {
                    Callable::Name(ident) => match self.resolve_name_or_builtin(ident)? {
                        Expr::Builtin(b) => Callable::Builtin(b),
                        Expr::Name(resolved) => Callable::Name(resolved),
                        _ => unreachable!("resolve_name_or_builtin returns Name or Builtin"),
                    },
                    other @ Callable::Builtin(_) => other,
                };
                Expr::Call { callable, args }
            }
            Expr::AttrCall { object, attr, mut args } => {
                // Prepare the object expression (supports chained access like a.b.c.method())
                let object = Box::new(self.prepare_expression(*object)?);
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                Expr::AttrCall { object, attr, args }
            }
            Expr::IndirectCall { callable, mut args } => {
                // Prepare the callable expression (e.g., lambda or any expression returning a callable)
                let callable = Box::new(self.prepare_expression(*callable)?);
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                Expr::IndirectCall { callable, args }
            }
            Expr::AttrGet { object, attr } => {
                // Prepare the object expression (supports chained access like a.b.c)
                let object = Box::new(self.prepare_expression(*object)?);
                Expr::AttrGet { object, attr }
            }
            Expr::List(elements) => {
                let items = elements
                    .into_iter()
                    .map(|item| self.prepare_sequence_item(item))
                    .collect::<Result<_, ParseError>>()?;
                Expr::List(items)
            }
            Expr::Tuple(elements) => {
                let items = elements
                    .into_iter()
                    .map(|item| self.prepare_sequence_item(item))
                    .collect::<Result<_, ParseError>>()?;
                Expr::Tuple(items)
            }
            Expr::Subscript { object, index } => Expr::Subscript {
                object: Box::new(self.prepare_expression(*object)?),
                index: Box::new(self.prepare_expression(*index)?),
            },
            Expr::Dict(dict_items) => {
                let prepared = dict_items
                    .into_iter()
                    .map(|item| match item {
                        DictItem::Pair(k, v) => {
                            Ok(DictItem::Pair(self.prepare_expression(k)?, self.prepare_expression(v)?))
                        }
                        DictItem::Unpack(e) => Ok(DictItem::Unpack(self.prepare_expression(e)?)),
                    })
                    .collect::<Result<_, ParseError>>()?;
                Expr::Dict(prepared)
            }
            Expr::Set(elements) => {
                let items = elements
                    .into_iter()
                    .map(|item| self.prepare_sequence_item(item))
                    .collect::<Result<_, ParseError>>()?;
                Expr::Set(items)
            }
            Expr::Not(operand) => Expr::Not(Box::new(self.prepare_expression(*operand)?)),
            Expr::UnaryMinus(operand) => Expr::UnaryMinus(Box::new(self.prepare_expression(*operand)?)),
            Expr::UnaryPlus(operand) => Expr::UnaryPlus(Box::new(self.prepare_expression(*operand)?)),
            Expr::UnaryInvert(operand) => Expr::UnaryInvert(Box::new(self.prepare_expression(*operand)?)),
            Expr::FString(parts) => {
                let prepared_parts = parts
                    .into_iter()
                    .map(|part| self.prepare_fstring_part(part))
                    .collect::<Result<Vec<_>, ParseError>>()?;
                Expr::FString(prepared_parts)
            }
            Expr::IfElse { test, body, orelse } => Expr::IfElse {
                test: Box::new(self.prepare_expression(*test)?),
                body: Box::new(self.prepare_expression(*body)?),
                orelse: Box::new(self.prepare_expression(*orelse)?),
            },
            Expr::ListComp { elt, generators } => {
                let (generators, elt, _) = self.prepare_comprehension(generators, Some(*elt), None)?;
                Expr::ListComp {
                    elt: Box::new(elt.expect("list comp must have elt")),
                    generators,
                }
            }
            Expr::SetComp { elt, generators } => {
                let (generators, elt, _) = self.prepare_comprehension(generators, Some(*elt), None)?;
                Expr::SetComp {
                    elt: Box::new(elt.expect("set comp must have elt")),
                    generators,
                }
            }
            Expr::DictComp { key, value, generators } => {
                let (generators, _, key_value) = self.prepare_comprehension(generators, None, Some((*key, *value)))?;
                let (key, value) = key_value.expect("dict comp must have key/value");
                Expr::DictComp {
                    key: Box::new(key),
                    value: Box::new(value),
                    generators,
                }
            }
            Expr::LambdaRaw {
                name_id,
                signature,
                body,
            } => {
                // Convert the raw lambda into a prepared lambda expression
                return self.prepare_lambda(name_id, &signature, &body, position);
            }
            Expr::Lambda { .. } => {
                // Lambda should only be created during prepare, never during parsing
                unreachable!("Expr::Lambda should not exist before prepare phase")
            }
            Expr::Slice { lower, upper, step } => Expr::Slice {
                lower: lower.map(|e| self.prepare_expression(*e)).transpose()?.map(Box::new),
                upper: upper.map(|e| self.prepare_expression(*e)).transpose()?.map(Box::new),
                step: step.map(|e| self.prepare_expression(*e)).transpose()?.map(Box::new),
            },
            Expr::Named { target, value } => {
                let value = Box::new(self.prepare_expression(*value)?);
                // Register the target as assigned in this scope
                self.names_assigned_in_order
                    .insert(self.interner.get_str(target.name_id).to_string());
                // Walrus binds in the enclosing scope (PEP 572), NOT in the
                // comprehension's scratch region. Resolve through the
                // assignment-target path which bypasses `comp_name_scopes`.
                let resolved_target = self.get_id_for_store_target(target)?;
                Expr::Named {
                    target: resolved_target,
                    value,
                }
            }
            Expr::Await(value) => Expr::Await(Box::new(self.prepare_expression(*value)?)),
        };

        // Optimization: Transform `(x % n) == value` with any constant right-hand side into a
        // specialized ModEq operator.
        // This is a common pattern in competitive programming (e.g., FizzBuzz checks like `i % 3 == 0`)
        // and can be executed more efficiently with a single modulo operation + comparison
        // instead of separate modulo, then equality check.
        if let Expr::CmpOp { left, op, right } = &expr
            && op == &CmpOperator::Eq
            && let Expr::Literal(Literal::Int(value)) = right.expr
            && let Expr::Op {
                left: left2,
                op,
                right: right2,
            } = &left.expr
            && op == &Operator::Mod
        {
            let new_expr = Expr::CmpOp {
                left: left2.clone(),
                op: CmpOperator::ModEq(value),
                right: right2.clone(),
            };
            return Ok(ExprLoc {
                position: left.position,
                expr: new_expr,
            });
        }

        Ok(ExprLoc { position, expr })
    }

    /// Resolves a name to either `Expr::Builtin` or `Expr::Name` with scope-aware builtin detection.
    ///
    /// Python's name resolution follows LEGB order (Local, Enclosing, Global, Builtin).
    /// Builtins are only used when the name is not found in any other scope. This method
    /// ensures that local assignments (e.g., `int = 42`) properly shadow builtin names.
    ///
    /// We check before calling `get_id` to avoid allocating unnecessary namespace slots.
    /// At module level, a slot allocated for an unassigned builtin would leak into
    /// `global_name_map` for nested functions, causing incorrect resolution.
    fn resolve_name_or_builtin(&mut self, name: Identifier) -> Result<Expr, ParseError> {
        let name_str = self.interner.get_str(name.name_id);

        // Parse-time builtin substitution is a module-scope-only optimization: turning
        // `len(x)` into `CallBuiltinFunction(Len)` skips a `LoadGlobal` round-trip,
        // but it's only safe when we are CERTAIN nothing will rebind the name later.
        //
        // At MODULE scope we have that certainty as long as no prior statement (this
        // snippet) and no prior REPL snippet (the seeded `name_map`) has bound the
        // name. Once either has, we have to defer to runtime so a later read sees the
        // user value.
        //
        // At FUNCTION scope we never have that certainty: the module can rebind a name
        // after the function is compiled (e.g. `def call_sum(): return sum(...)`
        // followed later by `def sum(...)`), and in REPL the rebinding can happen in a
        // future snippet that the current compile can't see. So at function scope we
        // always go through `get_id` and defer the builtin check to runtime.
        if self.is_module_scope() {
            let already_bound = self.names_assigned_in_order.contains(name_str) || self.name_map.contains_key(name_str);
            if !already_bound && let Ok(builtin) = name_str.parse::<Builtins>() {
                return Ok(Expr::Builtin(builtin));
            }
        }

        Ok(Expr::Name(self.get_id(name)?))
    }

    /// Prepares a `SequenceItem` by recursively preparing its inner expression.
    ///
    /// Both `Value` and `Unpack` variants need their expressions prepared
    /// (name resolution, scope analysis, builtin detection, etc.).
    fn prepare_sequence_item(&mut self, item: SequenceItem) -> Result<SequenceItem, ParseError> {
        match item {
            SequenceItem::Value(e) => Ok(SequenceItem::Value(self.prepare_expression(e)?)),
            SequenceItem::Unpack(e) => Ok(SequenceItem::Unpack(self.prepare_expression(e)?)),
        }
    }

    /// Prepares a comprehension with scope isolation for loop variables.
    ///
    /// Comprehension loop variables are isolated from the enclosing scope - they do not
    /// leak after the comprehension completes. CPython scoping rules require:
    ///
    /// 1. The FIRST generator's iter is evaluated in the enclosing scope
    /// 2. ALL loop variables from ALL generators are then shadowed as local
    /// 3. Subsequent generators' iters see all loop vars as local (even if unassigned)
    ///
    /// This means `[y for x in [1] for y in z for z in [[2]]]` raises UnboundLocalError
    /// because `z` is treated as local (it's a loop var in generator 3) when evaluating
    /// generator 2's iter.
    ///
    /// For list/set comprehensions, pass `elt` as Some and `key_value` as None.
    /// For dict comprehensions, pass `elt` as None and `key_value` as Some((key, value)).
    #[expect(clippy::type_complexity)]
    fn prepare_comprehension(
        &mut self,
        generators: Vec<Comprehension>,
        elt: Option<ExprLoc>,
        key_value: Option<(ExprLoc, ExprLoc)>,
    ) -> Result<(Vec<Comprehension>, Option<ExprLoc>, Option<(ExprLoc, ExprLoc)>), ParseError> {
        // Per PEP 572, walrus operators inside comprehensions bind in the ENCLOSING scope.
        // Pre-register walrus targets so they exist in the enclosing namespace BEFORE the
        // comp-name scope is pushed — that way `get_id_for_store_target` resolves them
        // straight to enclosing-scope slots without seeing comp-var.
        let mut walrus_targets: AHashSet<String> = AHashSet::new();
        if let Some(ref e) = elt {
            collect_assigned_names_from_expr(e, &mut walrus_targets, self.interner);
        }
        if let Some((ref k, ref v)) = key_value {
            collect_assigned_names_from_expr(k, &mut walrus_targets, self.interner);
            collect_assigned_names_from_expr(v, &mut walrus_targets, self.interner);
        }
        for generator in &generators {
            // Note: we don't scan iter expressions here because walrus in iterable is not allowed
            for cond in &generator.ifs {
                collect_assigned_names_from_expr(cond, &mut walrus_targets, self.interner);
            }
        }
        // Pre-allocate slots for walrus targets in the enclosing scope.
        // Anchor any namespace-overflow error to the first generator's iter,
        // since the walrus statements themselves can be scattered through the
        // comprehension and don't have a single load-bearing position.
        let comp_pos = generators.first().map(|g| g.iter.position).unwrap_or_default();
        for name in &walrus_targets {
            if !self.name_map.contains_key(name) {
                let slot = self.alloc_slot(comp_pos)?;
                self.name_map.insert(name.clone(), slot);
                self.names_assigned_in_order.insert(name.clone());
            }
        }

        // A comprehension is a single lexical scope even though its generators are
        // written left-to-right. Push one comp scope for the whole comprehension and
        // remember the scratch depth so we can release this comp's slots on exit
        // (sibling comps reuse the slots; high-water mark records peak nesting).
        let saved_var_depth = self.comp_var_depth;
        self.comp_name_scopes.push(AHashMap::new());

        // PEP 709 / CPython: the FIRST generator's iter is evaluated in the
        // *enclosing* scope, before any comp shadowing — that is why
        // `[x for x in x]` inside `def inner(): x = ...; return [x for x in x]`
        // pulls the outer `x` into the iter and then rebinds it. Prepare it now,
        // with the (empty) comp scope already pushed so any walrus or nested
        // lookup follows the same path as the rest of the comprehension; the
        // empty scope can't shadow anything yet.
        let mut generators_iter = generators.into_iter();
        let first_gen = generators_iter
            .next()
            .expect("comprehension must have at least one generator");
        let first_iter = self.prepare_expression(first_gen.iter)?;
        let remaining_gens: Vec<Comprehension> = generators_iter.collect();

        // Predeclare every generator target's names as comp-var slots BEFORE
        // preparing any *remaining* iter expression. This makes references to a
        // later generator's target name (or the first generator's target, in
        // the body) resolve to scratch — raising `UnboundLocalError` at runtime
        // if loaded before the corresponding `for` assigns (the reviewer's
        // `[x for x in [1] for y in z for z in [[2], [3]]]` example).
        let first_target = self.prepare_unpack_target_for_comprehension(first_gen.target)?;
        let mut remaining_targets: Vec<UnpackTarget> = Vec::with_capacity(remaining_gens.len());
        for generator in &remaining_gens {
            remaining_targets.push(self.prepare_unpack_target_for_comprehension(generator.target.clone())?);
        }

        // Now prepare the first generator's filters (with full comp scope visible),
        // then the remaining generators' iter + filters, then the body element.
        let first_ifs = first_gen
            .ifs
            .into_iter()
            .map(|cond| self.prepare_expression(cond))
            .collect::<Result<Vec<_>, _>>()?;

        let mut prepared_generators = Vec::with_capacity(1 + remaining_gens.len());
        prepared_generators.push(Comprehension {
            target: first_target,
            iter: first_iter,
            ifs: first_ifs,
        });
        for (generator, prepared_target) in remaining_gens.into_iter().zip(remaining_targets) {
            let iter = self.prepare_expression(generator.iter)?;
            let ifs = generator
                .ifs
                .into_iter()
                .map(|cond| self.prepare_expression(cond))
                .collect::<Result<Vec<_>, _>>()?;
            prepared_generators.push(Comprehension {
                target: prepared_target,
                iter,
                ifs,
            });
        }

        // Prepare the element / key-value expression(s) in the same comp scope
        // so they too see the comp targets.
        let prepared_elt = match elt {
            Some(e) => Some(self.prepare_expression(e)?),
            None => None,
        };
        let prepared_key_value = match key_value {
            Some((k, v)) => Some((self.prepare_expression(k)?, self.prepare_expression(v)?)),
            None => None,
        };

        // Pop the comp scope and release this comp's comp-var slots back into the pool.
        // The high-water mark already records peak nesting, so sibling comps can reuse
        // these slots without growing the per-frame scratch region.
        self.comp_name_scopes.pop();
        self.comp_var_depth = saved_var_depth;

        Ok((prepared_generators, prepared_elt, prepared_key_value))
    }

    /// Prepares an `AssignTarget` used by chained assignments.
    ///
    /// Resolves identifiers, sub-expressions and nested unpack patterns so that each
    /// target is ready for the compiler. Name-targets are also recorded in
    /// `names_assigned_in_order` just like single-target `Node::Assign` would, so the
    /// observable scope behaviour of `a = b = 1` matches `a = 1; b = 1`.
    fn prepare_assign_target(&mut self, target: AssignTarget) -> Result<AssignTarget, ParseError> {
        match target {
            AssignTarget::Name(ident) => {
                self.names_assigned_in_order
                    .insert(self.interner.get_str(ident.name_id).to_string());
                let ident = self.get_id(ident)?;
                Ok(AssignTarget::Name(ident))
            }
            AssignTarget::Subscript {
                target,
                index,
                target_position,
            } => Ok(AssignTarget::Subscript {
                target: self.prepare_expression(target)?,
                index: self.prepare_expression(index)?,
                target_position,
            }),
            AssignTarget::Attr {
                object,
                attr,
                target_position,
            } => Ok(AssignTarget::Attr {
                object: self.prepare_expression(object)?,
                attr,
                target_position,
            }),
            AssignTarget::Unpack {
                targets,
                targets_position,
            } => {
                let targets = targets
                    .into_iter()
                    .map(|t| self.prepare_unpack_target(t))
                    .collect::<Result<_, _>>()?;
                Ok(AssignTarget::Unpack {
                    targets,
                    targets_position,
                })
            }
        }
    }

    /// Prepares an unpack target by resolving identifiers recursively.
    ///
    /// Handles both single identifiers and nested tuples like `(a, b), c`.
    fn prepare_unpack_target(&mut self, target: UnpackTarget) -> Result<UnpackTarget, ParseError> {
        match target {
            UnpackTarget::Name(ident) => {
                self.names_assigned_in_order
                    .insert(self.interner.get_str(ident.name_id).to_string());
                Ok(UnpackTarget::Name(self.get_id(ident)?))
            }
            UnpackTarget::Starred(ident) => {
                self.names_assigned_in_order
                    .insert(self.interner.get_str(ident.name_id).to_string());
                Ok(UnpackTarget::Starred(self.get_id(ident)?))
            }
            UnpackTarget::Tuple { targets, position } => {
                let resolved_targets = targets
                    .into_iter()
                    .map(|t| self.prepare_unpack_target(t)) // Recursive call
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(UnpackTarget::Tuple {
                    targets: resolved_targets,
                    position,
                })
            }
        }
    }

    /// Predeclares an unpack target's names as comprehension-variable slots.
    ///
    /// Called during the first pass of `prepare_comprehension`, before any
    /// generator iter expressions are walked, so a later generator's target
    /// shadows references to the same name in earlier generators' iters. Each
    /// new name claims the next comp-var slot (recorded in
    /// `comp_var_depth`) and is inserted into the current `comp_name_scopes`
    /// frame. Subsequent reads inside the comprehension resolve through the
    /// scope stack and emit `Load/StoreCompTarget`; outside the comprehension
    /// the slot is unreachable.
    fn prepare_unpack_target_for_comprehension(&mut self, target: UnpackTarget) -> Result<UnpackTarget, ParseError> {
        match target {
            UnpackTarget::Name(ident) => {
                let slot = self.alloc_comp_var_slot(ident.name_id, ident.position)?;
                Ok(UnpackTarget::Name(Identifier::new_with_scope(
                    ident.name_id,
                    ident.position,
                    NamespaceId::new(usize::from(slot)).expect("comp-var slot fits in NamespaceId"),
                    NameScope::CompVar,
                )))
            }
            UnpackTarget::Starred(ident) => {
                let slot = self.alloc_comp_var_slot(ident.name_id, ident.position)?;
                Ok(UnpackTarget::Starred(Identifier::new_with_scope(
                    ident.name_id,
                    ident.position,
                    NamespaceId::new(usize::from(slot)).expect("comp-var slot fits in NamespaceId"),
                    NameScope::CompVar,
                )))
            }
            UnpackTarget::Tuple { targets, position } => {
                let resolved_targets = targets
                    .into_iter()
                    .map(|t| self.prepare_unpack_target_for_comprehension(t))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(UnpackTarget::Tuple {
                    targets: resolved_targets,
                    position,
                })
            }
        }
    }

    /// Allocates the next comp-var slot for `name_id`, registering it in
    /// the topmost comp-name scope and updating the high-water mark.
    ///
    /// Returns the slot index (a `u16`, matching the `Load/StoreCompTarget`
    /// operand width). Raises a syntax error anchored to `position` if the
    /// scratch region would exceed `u16::MAX` slots — the same overflow
    /// behavior `alloc_slot` uses for the regular namespace.
    fn alloc_comp_var_slot(&mut self, name_id: StringId, position: CodeRange) -> Result<u16, ParseError> {
        let slot = self.comp_var_depth;
        let next = slot.checked_add(1).ok_or_else(|| namespace_overflow(position))?;
        self.comp_var_depth = next;
        // No per-slot side table for names — each `Load/StoreCompTarget`
        // opcode carries its target's `name_id` inline. Just push the name
        // onto the active comp scope so subsequent reads inside the
        // comprehension can resolve to this slot.
        let name_str = self.interner.get_str(name_id).to_string();
        let top = self
            .comp_name_scopes
            .last_mut()
            .expect("alloc_comp_var_slot called outside an active comp scope");
        top.insert(name_str, slot);

        Ok(slot)
    }

    /// Prepares a function definition using a two-pass approach for correct scope resolution.
    ///
    /// Pass 1: Scan the function body to collect:
    /// - Names declared as `global`
    /// - Names declared as `nonlocal`
    /// - Names that are assigned (these are local unless declared global/nonlocal)
    ///
    /// Pass 2: Prepare the function body with the scope information from pass 1.
    ///
    /// # Closure Analysis
    ///
    /// When the nested function uses `nonlocal` declarations, those names must exist
    /// in an enclosing scope. The enclosing scope's variable becomes a cell_var
    /// (stored in a heap cell), and the nested function captures it as a free_var.
    fn prepare_function_def(
        &mut self,
        name: Identifier,
        parsed_sig: &ParsedSignature,
        body: Vec<ParseNode>,
        is_async: bool,
    ) -> Result<PreparedNode, ParseError> {
        // Register the function name in the current scope; `def` binds the name.
        self.names_assigned_in_order
            .insert(self.interner.get_str(name.name_id).to_string());
        let name = self.get_id(name)?;

        // Extract param names from the parsed signature for scope analysis
        let param_names: Vec<StringId> = parsed_sig.param_names().collect();

        // Pass 1: Collect scope information from the function body
        let scope_info = collect_function_scope_info(&body, &param_names, self.interner);

        // Names this nested function may capture from enclosing scopes
        // (transitively — see `child_enclosing_locals`).
        let enclosing_locals = self.child_enclosing_locals();

        // Filter potential_captures to get actual implicit captures.
        // Only names that are ALSO in enclosing_locals are true implicit captures.
        // Names NOT in enclosing_locals are either builtins or globals (handled at runtime).
        let implicit_captures: AHashSet<String> = scope_info
            .potential_captures
            .into_iter()
            .filter(|name| enclosing_locals.contains(name))
            .collect();

        // Build a live `GlobalsRef` to the module's name_map + namespace_size.
        // At module scope we ARE the module — borrow our own fields. At nested
        // function scope we re-borrow our parent's `global_name_map`.
        let global_name_map = if let Some(global_name_map) = &mut self.global_name_map {
            global_name_map.reborrow()
        } else {
            GlobalsRef {
                name_map: &mut self.name_map,
                namespace_size: &mut self.namespace_size,
            }
        };

        // Pass 2: Create child preparer for function body with scope info
        let mut inner_prepare = Prepare::new_function(
            body.len(),
            &param_names,
            name.position,
            scope_info.assigned_names,
            scope_info.global_names,
            scope_info.nonlocal_names,
            implicit_captures,
            global_name_map,
            Some(enclosing_locals),
            scope_info.cell_var_names,
            self.interner,
        )?;

        // Prepare the function body
        let prepared_body = inner_prepare.prepare_nodes(body)?;

        // Pull the per-function maps out of `inner_prepare` and drop the inner
        // preparer so its `GlobalsRef` borrow on `self.name_map` is released —
        // the cell-var work below needs to mutate `self.name_map` and can't
        // run while the borrow is live. No bubble-up needed: the inner
        // preparer's `case 1` allocated any `global X` slots directly into
        // our `name_map` via the shared `GlobalsRef`.
        let inner_free_var_map = mem::take(&mut inner_prepare.free_var_map);
        let inner_cell_var_map = mem::take(&mut inner_prepare.cell_var_map);
        let namespace_size = inner_prepare.namespace_size;
        drop(inner_prepare);

        // Record every variable the inner function captured from us, filing
        // each as an owned cell or a pass-through free var (see
        // `promote_captured_var`).
        for captured_name in inner_free_var_map.keys() {
            self.promote_captured_var(captured_name, name.position)?;
        }

        let (free_var_slots, free_var_enclosing_slots) = self.build_free_var_slots(inner_free_var_map);
        let (cell_var_slots, cell_param_indices) = build_cell_slots(inner_cell_var_map, &param_names, self.interner);

        // Build the runtime Signature from the parsed signature
        let pos_args: Vec<StringId> = parsed_sig.pos_args.iter().map(|p| p.name).collect();
        let pos_defaults_count = parsed_sig.pos_args.iter().filter(|p| p.default.is_some()).count();
        let args: Vec<StringId> = parsed_sig.args.iter().map(|p| p.name).collect();
        let arg_defaults_count = parsed_sig.args.iter().filter(|p| p.default.is_some()).count();
        let mut kwargs: Vec<StringId> = Vec::with_capacity(parsed_sig.kwargs.len());
        let mut kwarg_default_map: Vec<Option<usize>> = Vec::with_capacity(parsed_sig.kwargs.len());
        let mut kwarg_default_index = 0;
        for param in &parsed_sig.kwargs {
            kwargs.push(param.name);
            if param.default.is_some() {
                kwarg_default_map.push(Some(kwarg_default_index));
                kwarg_default_index += 1;
            } else {
                kwarg_default_map.push(None);
            }
        }

        let signature = Signature::new(
            pos_args,
            pos_defaults_count,
            args,
            arg_defaults_count,
            parsed_sig.var_args,
            kwargs,
            kwarg_default_map,
            parsed_sig.var_kwargs,
        );

        // Collect and prepare default expressions in order: pos_args -> args -> kwargs
        // Only includes parameters that actually have defaults.
        let mut default_exprs = Vec::with_capacity(signature.total_defaults_count());
        for param in &parsed_sig.pos_args {
            if let Some(ref expr) = param.default {
                default_exprs.push(self.prepare_expression(expr.clone())?);
            }
        }
        for param in &parsed_sig.args {
            if let Some(ref expr) = param.default {
                default_exprs.push(self.prepare_expression(expr.clone())?);
            }
        }
        for param in &parsed_sig.kwargs {
            if let Some(ref expr) = param.default {
                default_exprs.push(self.prepare_expression(expr.clone())?);
            }
        }

        // Return the prepared function definition inline in the AST
        Ok(Node::FunctionDef(PreparedFunctionDef {
            name,
            signature,
            body: prepared_body,
            namespace_size,
            free_var_enclosing_slots,
            free_var_slots,
            cell_var_slots,
            cell_param_indices,
            default_exprs,
            is_async,
        }))
    }

    /// Prepares a lambda expression, converting it into a prepared function definition.
    ///
    /// Lambdas are essentially anonymous functions with an implicit return of their body
    /// expression. This method follows the same preparation logic as `prepare_function_def`
    /// but:
    /// - Uses `<lambda>` as the function name (not registered in scope)
    /// - Wraps the body expression as `Node::Return(body)`
    /// - Returns `ExprLoc` with `Expr::Lambda` instead of `PreparedNode`
    fn prepare_lambda(
        &mut self,
        lambda_name_id: StringId,
        parsed_sig: &ParsedSignature,
        body: &ExprLoc,
        position: CodeRange,
    ) -> Result<ExprLoc, ParseError> {
        // Create a synthetic <lambda> name identifier (not registered in scope)
        let lambda_name = Identifier::new_with_scope(
            lambda_name_id,
            position,
            // Slot 0 is the trivial placeholder; the lambda name never lands
            // in a namespace because lambdas don't have a binding name.
            NamespaceId::new(0).expect("slot 0 fits in u16"),
            NameScope::Local,
        );

        // Wrap the body expression as a return statement for scope analysis
        let body_as_node: ParseNode = Node::Return(Some(body.clone()));
        let body_nodes = vec![body_as_node];

        // Extract param names from the parsed signature for scope analysis
        let param_names: Vec<StringId> = parsed_sig.param_names().collect();

        // Pass 1: Collect scope information from the lambda body
        // (Lambdas can't have global/nonlocal declarations, but can have nested functions)
        let scope_info = collect_function_scope_info(&body_nodes, &param_names, self.interner);

        // Names this lambda may capture from enclosing scopes (transitively).
        let enclosing_locals = self.child_enclosing_locals();

        // Filter potential_captures to get actual implicit captures
        let implicit_captures: AHashSet<String> = scope_info
            .potential_captures
            .into_iter()
            .filter(|name| enclosing_locals.contains(name))
            .collect();

        // Build the live `GlobalsRef` (see `prepare_function_def` for rationale).
        let global_name_map = if self.is_module_scope() {
            GlobalsRef {
                name_map: &mut self.name_map,
                namespace_size: &mut self.namespace_size,
            }
        } else {
            self.global_name_map
                .as_mut()
                .expect("function-scope preparer always has global_name_map")
                .reborrow()
        };

        // Pass 2: Create child preparer for lambda body with scope info
        let mut inner_prepare = Prepare::new_function(
            body_nodes.len(),
            &param_names,
            position,
            scope_info.assigned_names,
            scope_info.global_names,
            scope_info.nonlocal_names,
            implicit_captures,
            global_name_map,
            Some(enclosing_locals),
            scope_info.cell_var_names,
            self.interner,
        )?;

        // Prepare the lambda body
        let prepared_body = inner_prepare.prepare_nodes(body_nodes)?;

        // Pull data out of `inner_prepare` and drop it so its `GlobalsRef`
        // borrow on `self.name_map` is released before we mutate `self.name_map`.
        let inner_free_var_map = mem::take(&mut inner_prepare.free_var_map);
        let inner_cell_var_map = mem::take(&mut inner_prepare.cell_var_map);
        let namespace_size = inner_prepare.namespace_size;
        drop(inner_prepare);

        // Record every variable the lambda captured from us, filing each as an
        // owned cell or a pass-through free var (see `promote_captured_var`).
        for captured_name in inner_free_var_map.keys() {
            self.promote_captured_var(captured_name, position)?;
        }

        let (free_var_slots, free_var_enclosing_slots) = self.build_free_var_slots(inner_free_var_map);
        let (cell_var_slots, cell_param_indices) = build_cell_slots(inner_cell_var_map, &param_names, self.interner);

        // Build the runtime Signature from the parsed signature
        let pos_args: Vec<StringId> = parsed_sig.pos_args.iter().map(|p| p.name).collect();
        let pos_defaults_count = parsed_sig.pos_args.iter().filter(|p| p.default.is_some()).count();
        let args: Vec<StringId> = parsed_sig.args.iter().map(|p| p.name).collect();
        let arg_defaults_count = parsed_sig.args.iter().filter(|p| p.default.is_some()).count();
        let mut kwargs: Vec<StringId> = Vec::with_capacity(parsed_sig.kwargs.len());
        let mut kwarg_default_map: Vec<Option<usize>> = Vec::with_capacity(parsed_sig.kwargs.len());
        let mut kwarg_default_index = 0;
        for param in &parsed_sig.kwargs {
            kwargs.push(param.name);
            if param.default.is_some() {
                kwarg_default_map.push(Some(kwarg_default_index));
                kwarg_default_index += 1;
            } else {
                kwarg_default_map.push(None);
            }
        }

        let signature = Signature::new(
            pos_args,
            pos_defaults_count,
            args,
            arg_defaults_count,
            parsed_sig.var_args,
            kwargs,
            kwarg_default_map,
            parsed_sig.var_kwargs,
        );

        // Collect and prepare default expressions (evaluated in enclosing scope)
        let mut default_exprs = Vec::with_capacity(signature.total_defaults_count());
        for param in &parsed_sig.pos_args {
            if let Some(ref expr) = param.default {
                default_exprs.push(self.prepare_expression(expr.clone())?);
            }
        }
        for param in &parsed_sig.args {
            if let Some(ref expr) = param.default {
                default_exprs.push(self.prepare_expression(expr.clone())?);
            }
        }
        for param in &parsed_sig.kwargs {
            if let Some(ref expr) = param.default {
                default_exprs.push(self.prepare_expression(expr.clone())?);
            }
        }

        // Create the prepared function definition (lambdas are never async)
        let func_def = PreparedFunctionDef {
            name: lambda_name,
            signature,
            body: prepared_body,
            namespace_size,
            free_var_enclosing_slots,
            free_var_slots,
            cell_var_slots,
            cell_param_indices,
            default_exprs,
            is_async: false,
        };

        Ok(ExprLoc::new(
            position,
            Expr::Lambda {
                func_def: Box::new(func_def),
            },
        ))
    }

    /// Resolves an identifier to its namespace index and scope, creating a new entry if needed.
    ///
    /// TODO This whole implementation seems ugly at best.
    ///
    /// This is the core name resolution mechanism with scope-aware resolution:
    ///
    /// **At module level:** All names go to the local namespace (which IS the global namespace).
    ///
    /// **In functions:**
    /// - If name is declared `global` → resolve to global namespace
    /// - If name is declared `nonlocal` → resolve to enclosing scope via Cell
    /// - If name is assigned in this function → resolve to local namespace
    /// - If name exists in global namespace (read-only access) → resolve to global namespace
    /// - Otherwise → resolve to local namespace (will be NameError at runtime)
    ///
    /// Resolves an identifier for an assignment-position store (e.g. walrus target).
    ///
    /// Per PEP 572, walrus operators inside comprehensions bind in the **enclosing**
    /// scope, not the comprehension. The same applies to any other store target
    /// that is not a comprehension's own generator target. Bypassing
    /// `comp_name_scopes` ensures the store can never accidentally land in a
    /// comp-var slot that happens to share its name. Generator target stores
    /// are installed by `prepare_unpack_target_for_comprehension` and never come
    /// through here.
    fn get_id_for_store_target(&mut self, ident: Identifier) -> Result<Identifier, ParseError> {
        let saved_scopes = mem::take(&mut self.comp_name_scopes);
        let result = self.get_id(ident);
        self.comp_name_scopes = saved_scopes;
        result
    }

    fn get_id(&mut self, ident: Identifier) -> Result<Identifier, ParseError> {
        let name_str = self.interner.get_str(ident.name_id);
        let position = ident.position;

        // Read path: check the comp-name scope stack first, top-down. A name
        // bound by a generator target shadows any same-named outer binding
        // *for ordinary expression-position reads inside the comprehension*.
        // Walrus targets and other assignment-position stores take a
        // separate path that bypasses the comp scope (see
        // `get_id_for_store_target`), so this lookup is read-only-safe.
        for scope in self.comp_name_scopes.iter().rev() {
            if let Some(&slot) = scope.get(name_str) {
                return Ok(Identifier::new_with_scope(
                    ident.name_id,
                    position,
                    NamespaceId::new(usize::from(slot)).expect("comp-var slot fits in NamespaceId"),
                    NameScope::CompVar,
                ));
            }
        }

        // At module scope every name is a global — the module's local namespace
        // IS the global namespace, and Python module scope has no
        // `UnboundLocalError`, only `NameError`. Every reference allocates a
        // module slot on first sight; subsequent references reuse it. Reads of
        // never-bound names get a slot too — they need it to store any value
        // resolved by the host.
        //
        // Comprehensions don't reach this branch: their loop variables live in
        // `NameScope::CompVar`, handled by the comp-name scope lookup above.
        if self.is_module_scope() {
            let id = if let Some(existing) = self.name_map.get(name_str).copied() {
                existing
            } else {
                let id = self.alloc_slot(position)?;
                self.name_map.insert(name_str.to_string(), id);
                id
            };
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                id,
                NameScope::Global,
            ));
        }

        // In a function: determine scope based on global_names, nonlocal_names, assigned_names, global_name_map

        // 1. Check if declared `global`
        if self.global_names.contains(name_str) {
            let globals = self
                .global_name_map
                .as_mut()
                .expect("function-scope preparer always has global_name_map");
            // Allocate a module slot eagerly (or reuse an existing one).
            let slot = globals.ensure_slot(name_str, position)?;
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                slot,
                NameScope::Global,
            ));
        }

        // 2. Check if captured from enclosing scope (nonlocal declaration or implicit capture)
        // free_var_map stores namespace slot indices where the cell reference will be stored
        if let Some(&slot) = self.free_var_map.get(name_str) {
            // At runtime, the cell reference is in namespace[slot] as Value::Ref(cell_id)
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                slot,
                NameScope::Cell,
            ));
        }

        // 3. Check if this is a cell variable (captured by nested functions)
        // cell_var_map stores namespace slot indices where the cell reference will be stored
        // At call time, a cell is created and stored as Value::Ref(cell_id) at this slot
        if let Some(&slot) = self.cell_var_map.get(name_str) {
            // The namespace slot was already allocated when cell_var_map was populated
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                slot,
                NameScope::Cell,
            ));
        }

        // 4. Check if assigned in this function (local variable)
        if self.assigned_names.contains(name_str) {
            let id = if let Some(existing) = self.name_map.get(name_str).copied() {
                existing
            } else {
                let id = self.alloc_slot(position)?;
                self.name_map.insert(name_str.to_string(), id);
                id
            };
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                id,
                NameScope::Local,
            ));
        }

        // 5. Check if name was pre-populated in name_map (from function parameters)
        // This ensures parameters shadow both enclosing locals and global variables
        // with the same name. Parameters are added to name_map during
        // FunctionScope::new_function() but are NOT in assigned_names (since they're
        // not assigned in the function body). This MUST be checked before
        // enclosing_locals, otherwise a parameter like `def inner(x)` would be
        // incorrectly resolved as a closure capture when an outer scope also has `x`.
        if let Some(&id) = self.name_map.get(name_str) {
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                id,
                NameScope::Local,
            ));
        }

        // 6. Check if exists in enclosing scope (implicit closure capture)
        // This handles reading variables from enclosing functions without explicit `nonlocal`
        if let Some(ref enclosing) = self.enclosing_locals
            && enclosing.contains(name_str)
        {
            // This is an implicit capture - add to free_var_map with a namespace slot
            let slot = if let Some(&existing_slot) = self.free_var_map.get(name_str) {
                existing_slot
            } else {
                // Allocate a namespace slot for this free variable
                let slot = self.alloc_slot(position)?;
                self.name_map.insert(name_str.to_string(), slot);
                self.free_var_map.insert(name_str.to_string(), slot);
                slot
            };
            return Ok(Identifier::new_with_scope(
                ident.name_id,
                position,
                slot,
                NameScope::Cell,
            ));
        }

        // 7. Fall back to the module global namespace. The name is either
        // already there (an implicit global read of a module-level binding) or
        // we allocate a fresh slot for it (typo, builtin, external function
        // — runtime resolution will find `Undefined` in the slot and either
        // find a builtin or yield to host for name lookup).
        let globals = self
            .global_name_map
            .as_mut()
            .expect("function-scope preparer always has global_name_map");
        let slot = globals.ensure_slot(name_str, position)?;
        Ok(Identifier::new_with_scope(
            ident.name_id,
            position,
            slot,
            NameScope::Global,
        ))
    }

    /// Prepares an f-string part by resolving names in interpolated expressions.
    fn prepare_fstring_part(&mut self, part: FStringPart) -> Result<FStringPart, ParseError> {
        match part {
            FStringPart::Literal(s) => Ok(FStringPart::Literal(s)),
            FStringPart::Interpolation {
                expr,
                conversion,
                format_spec,
                debug_prefix,
            } => {
                let prepared_expr = Box::new(self.prepare_expression(*expr)?);
                let prepared_spec = match format_spec {
                    Some(FormatSpec::Static(s)) => Some(FormatSpec::Static(s)),
                    Some(FormatSpec::Dynamic(parts)) => {
                        let prepared = parts
                            .into_iter()
                            .map(|p| self.prepare_fstring_part(p))
                            .collect::<Result<Vec<_>, _>>()?;
                        Some(FormatSpec::Dynamic(prepared))
                    }
                    None => None,
                };
                Ok(FStringPart::Interpolation {
                    expr: prepared_expr,
                    conversion,
                    format_spec: prepared_spec,
                    debug_prefix,
                })
            }
        }
    }
}

/// Information collected from first-pass scan of a function body.
///
/// This struct holds the scope-related information needed for the second pass
/// of function preparation and for closure analysis.
struct FunctionScopeInfo {
    /// Names declared as `global`
    global_names: AHashSet<String>,
    /// Names declared as `nonlocal`
    nonlocal_names: AHashSet<String>,
    /// Names that are assigned in this scope
    assigned_names: AHashSet<String>,
    /// Names that are captured by nested functions (must be stored in cells)
    cell_var_names: AHashSet<String>,
    /// Names that are referenced but not local, global, or nonlocal.
    /// These are POTENTIAL implicit captures - they may be captures from an enclosing function
    /// OR they may be builtin/global reads. The actual implicit captures are determined
    /// by filtering against enclosing_locals in new_function.
    potential_captures: AHashSet<String>,
}

/// Scans a function body to collect scope information (first phase of preparation).
///
/// This function performs three passes over the AST:
/// 1. Collect global, nonlocal, and assigned names
/// 2. Identify cell_vars (names captured by nested functions)
/// 3. Collect potential implicit captures (referenced but not local/global/nonlocal)
///
/// The collected information includes:
/// - Names declared as `global` (from Global statements)
/// - Names declared as `nonlocal` (from Nonlocal statements)
/// - Names that are assigned (from Assign, OpAssign, For targets, etc.)
/// - Names that are captured by nested functions (cell_var_names)
/// - Names that might be captured from enclosing scope (potential_captures)
///
/// This information is used to determine whether each name reference should resolve
/// to the local namespace, global namespace, or an enclosing scope via cells.
/// Builds the parallel owned-cell vectors for a nested scope from its cell-var
/// map (`name -> slot`).
///
/// Returns `(cell_var_slots, cell_param_indices)`, ordered by slot: the slot
/// where each fresh cell is installed at call time, and the parameter index it
/// should be seeded from when the cell is for one of `params` (else `None`).
fn build_cell_slots(
    cell_var_map: AHashMap<String, NamespaceId>,
    params: &[StringId],
    interner: &InternerBuilder,
) -> (Vec<NamespaceId>, Vec<Option<usize>>) {
    let param_name_to_index: AHashMap<String, usize> = params
        .iter()
        .enumerate()
        .map(|(idx, &name_id)| (interner.get_str(name_id).to_string(), idx))
        .collect();
    let mut entries: Vec<_> = cell_var_map.into_iter().collect();
    entries.sort_by_key(|(_, slot)| *slot);
    let slots = entries.iter().map(|(_, slot)| *slot).collect();
    let param_indices = entries
        .iter()
        .map(|(name, _)| param_name_to_index.get(name).copied())
        .collect();
    (slots, param_indices)
}

fn collect_function_scope_info(
    nodes: &[ParseNode],
    params: &[StringId],
    interner: &InternerBuilder,
) -> FunctionScopeInfo {
    let mut global_names = AHashSet::new();
    let mut nonlocal_names = AHashSet::new();
    let mut assigned_names = AHashSet::new();
    let mut cell_var_names = AHashSet::new();
    let mut referenced_names = AHashSet::new();

    // First pass: collect global, nonlocal, and assigned names
    for node in nodes {
        collect_scope_info_from_node(
            node,
            &mut global_names,
            &mut nonlocal_names,
            &mut assigned_names,
            interner,
        );
    }

    // Build the set of our locals: params + assigned_names (excluding globals)
    let param_names: AHashSet<String> = params
        .iter()
        .map(|string_id| interner.get_str(*string_id).to_string())
        .collect();

    let our_locals: AHashSet<String> = param_names
        .iter()
        .cloned()
        .chain(assigned_names.iter().cloned())
        .filter(|name| !global_names.contains(name))
        .collect();

    // Second pass: find what nested functions capture from us
    for node in nodes {
        collect_cell_vars_from_node(node, &our_locals, &mut cell_var_names, interner);
    }

    // Third pass: collect all referenced names to identify potential implicit captures.
    // These are names that might be captured from an enclosing function scope.
    // We can't fully determine implicit captures here because we don't know yet what
    // the enclosing scope's locals are - that's determined later when we call new_function.
    for node in nodes {
        collect_referenced_names_from_node(node, &mut referenced_names, interner);
    }

    // Potential implicit captures are names that are:
    // - Referenced in the function body
    // - Not local (not params, not assigned)
    // - Not declared global
    // - Not declared nonlocal (those are handled separately)
    // The actual implicit captures will be filtered against enclosing_locals in new_function.
    let potential_captures: AHashSet<String> = referenced_names
        .into_iter()
        .filter(|name| !our_locals.contains(name) && !global_names.contains(name) && !nonlocal_names.contains(name))
        .collect();

    FunctionScopeInfo {
        global_names,
        nonlocal_names,
        assigned_names,
        cell_var_names,
        potential_captures,
    }
}

/// Helper to collect scope info from a single node.
fn collect_scope_info_from_node(
    node: &ParseNode,
    global_names: &mut AHashSet<String>,
    nonlocal_names: &mut AHashSet<String>,
    assigned_names: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match node {
        Node::Global { names, .. } => {
            for string_id in names {
                global_names.insert(interner.get_str(*string_id).to_string());
            }
        }
        Node::Nonlocal { names, .. } => {
            for string_id in names {
                nonlocal_names.insert(interner.get_str(*string_id).to_string());
            }
        }
        Node::Assign { target, object } => {
            assigned_names.insert(interner.get_str(target.name_id).to_string());
            // Scan value expression for walrus operators
            collect_assigned_names_from_expr(object, assigned_names, interner);
        }
        Node::UnpackAssign { targets, object, .. } => {
            // Recursively collect all names from nested unpack targets
            for target in targets {
                collect_names_from_unpack_target(target, assigned_names, interner);
            }
            // Scan value expression for walrus operators
            collect_assigned_names_from_expr(object, assigned_names, interner);
        }
        Node::OpAssign { target, value, .. } => {
            assigned_names.insert(interner.get_str(target.name_id).to_string());
            // Scan value expression for walrus operators
            collect_assigned_names_from_expr(value, assigned_names, interner);
        }
        Node::SubscriptOpAssign {
            target, index, value, ..
        } => {
            collect_assigned_names_from_expr(target, assigned_names, interner);
            collect_assigned_names_from_expr(index, assigned_names, interner);
            collect_assigned_names_from_expr(value, assigned_names, interner);
        }
        Node::SubscriptAssign {
            target, index, value, ..
        } => {
            // Subscript assignment doesn't create a new name, it modifies existing container
            // But scan expressions for walrus operators
            collect_assigned_names_from_expr(target, assigned_names, interner);
            collect_assigned_names_from_expr(index, assigned_names, interner);
            collect_assigned_names_from_expr(value, assigned_names, interner);
        }
        Node::AttrOpAssign { object, value, .. } => {
            collect_assigned_names_from_expr(object, assigned_names, interner);
            collect_assigned_names_from_expr(value, assigned_names, interner);
        }
        Node::AttrAssign { object, value, .. } => {
            // Attribute assignment doesn't create a new name, it modifies existing object
            // But scan expressions for walrus operators
            collect_assigned_names_from_expr(object, assigned_names, interner);
            collect_assigned_names_from_expr(value, assigned_names, interner);
        }
        Node::ChainAssign { targets, object } => {
            // Each target sees the same shared RHS; treat it like each per-target
            // assignment would be treated individually.
            for target in targets {
                collect_assigned_names_from_assign_target(target, assigned_names, interner);
            }
            collect_assigned_names_from_expr(object, assigned_names, interner);
        }
        Node::For {
            target,
            iter,
            body,
            or_else,
        } => {
            // For loop target is assigned - collect all names from the target
            collect_names_from_unpack_target(target, assigned_names, interner);
            // Scan iter expression for walrus operators
            collect_assigned_names_from_expr(iter, assigned_names, interner);
            // Recurse into body and else
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        Node::While { test, body, or_else } => {
            // Scan test expression for walrus operators
            collect_assigned_names_from_expr(test, assigned_names, interner);
            // Recurse into body and else blocks
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        Node::If { test, body, or_else } => {
            // Scan test expression for walrus operators
            collect_assigned_names_from_expr(test, assigned_names, interner);
            // Recurse into branches
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        Node::FunctionDef(RawFunctionDef { name, .. }) => {
            // Function definition creates a local binding for the function name
            // But we don't recurse into the function body - that's a separate scope
            assigned_names.insert(interner.get_str(name.name_id).to_string());
        }
        Node::Try(Try {
            body,
            handlers,
            or_else,
            finally,
        }) => {
            // Recurse into all blocks
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for handler in handlers {
                // Exception variable name is assigned
                if let Some(ref name) = handler.name {
                    assigned_names.insert(interner.get_str(name.name_id).to_string());
                }
                for n in &handler.body {
                    collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
                }
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in finally {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        Node::With {
            context, target, body, ..
        } => {
            // The `as TARGET` binds names like a for-loop target does.
            if let Some(t) = target {
                collect_names_from_unpack_target(t, assigned_names, interner);
            }
            // Scan the context expression for walrus operators.
            collect_assigned_names_from_expr(context, assigned_names, interner);
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        // Import creates bindings for each module name (or alias)
        Node::Import { names, .. } => {
            for import_name in names {
                assigned_names.insert(interner.get_str(import_name.binding.name_id).to_string());
            }
        }
        // ImportFrom creates bindings for each imported name (or alias)
        Node::ImportFrom { names, .. } => {
            for (_import_name, binding) in names {
                assigned_names.insert(interner.get_str(binding.name_id).to_string());
            }
        }
        // Statements with expressions that may contain walrus operators
        Node::Expr(expr) | Node::Return(Some(expr)) | Node::Raise(Some(expr)) => {
            collect_assigned_names_from_expr(expr, assigned_names, interner);
        }
        Node::Assert { test, msg } => {
            collect_assigned_names_from_expr(test, assigned_names, interner);
            if let Some(m) = msg {
                collect_assigned_names_from_expr(m, assigned_names, interner);
            }
        }
        // These don't create new names
        Node::Pass | Node::Return(None) | Node::Raise(None) | Node::Break { .. } | Node::Continue { .. } => {}
    }
}

/// Collects names assigned by walrus operators (`:=`) within an expression.
///
/// Per PEP 572, walrus operator targets are assignments in the enclosing scope.
/// This function recursively scans expressions to find all `Named` expression targets.
/// It does NOT recurse into lambda bodies as those have their own scope.
fn collect_assigned_names_from_expr(expr: &ExprLoc, assigned_names: &mut AHashSet<String>, interner: &InternerBuilder) {
    match &expr.expr {
        Expr::Named { target, value } => {
            // The target of a walrus operator is assigned in this scope
            assigned_names.insert(interner.get_str(target.name_id).to_string());
            // Also scan the value expression
            collect_assigned_names_from_expr(value, assigned_names, interner);
        }
        // Recurse into sub-expressions
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                let expr = match item {
                    SequenceItem::Value(e) | SequenceItem::Unpack(e) => e,
                };
                collect_assigned_names_from_expr(expr, assigned_names, interner);
            }
        }
        Expr::Dict(dict_items) => {
            for item in dict_items {
                match item {
                    DictItem::Pair(key, value) => {
                        collect_assigned_names_from_expr(key, assigned_names, interner);
                        collect_assigned_names_from_expr(value, assigned_names, interner);
                    }
                    DictItem::Unpack(e) => collect_assigned_names_from_expr(e, assigned_names, interner),
                }
            }
        }
        Expr::Op { left, right, .. } | Expr::CmpOp { left, right, .. } => {
            collect_assigned_names_from_expr(left, assigned_names, interner);
            collect_assigned_names_from_expr(right, assigned_names, interner);
        }
        Expr::ChainCmp { left, comparisons } => {
            collect_assigned_names_from_expr(left, assigned_names, interner);
            for (_, expr) in comparisons {
                collect_assigned_names_from_expr(expr, assigned_names, interner);
            }
        }
        Expr::Not(operand)
        | Expr::UnaryMinus(operand)
        | Expr::UnaryPlus(operand)
        | Expr::UnaryInvert(operand)
        | Expr::Await(operand) => {
            collect_assigned_names_from_expr(operand, assigned_names, interner);
        }
        Expr::Subscript { object, index } => {
            collect_assigned_names_from_expr(object, assigned_names, interner);
            collect_assigned_names_from_expr(index, assigned_names, interner);
        }
        Expr::Call { args, .. } => {
            collect_assigned_names_from_args(args, assigned_names, interner);
        }
        Expr::AttrCall { object, args, .. } => {
            collect_assigned_names_from_expr(object, assigned_names, interner);
            collect_assigned_names_from_args(args, assigned_names, interner);
        }
        Expr::IndirectCall { callable, args } => {
            collect_assigned_names_from_expr(callable, assigned_names, interner);
            collect_assigned_names_from_args(args, assigned_names, interner);
        }
        Expr::AttrGet { object, .. } => {
            collect_assigned_names_from_expr(object, assigned_names, interner);
        }
        Expr::IfElse { test, body, orelse } => {
            collect_assigned_names_from_expr(test, assigned_names, interner);
            collect_assigned_names_from_expr(body, assigned_names, interner);
            collect_assigned_names_from_expr(orelse, assigned_names, interner);
        }
        // Per PEP 572, walrus in comprehensions assigns to the ENCLOSING scope
        Expr::ListComp { elt, generators } | Expr::SetComp { elt, generators } => {
            collect_assigned_names_from_expr(elt, assigned_names, interner);
            for generator in generators {
                collect_assigned_names_from_expr(&generator.iter, assigned_names, interner);
                for cond in &generator.ifs {
                    collect_assigned_names_from_expr(cond, assigned_names, interner);
                }
            }
        }
        Expr::DictComp { key, value, generators } => {
            collect_assigned_names_from_expr(key, assigned_names, interner);
            collect_assigned_names_from_expr(value, assigned_names, interner);
            for generator in generators {
                collect_assigned_names_from_expr(&generator.iter, assigned_names, interner);
                for cond in &generator.ifs {
                    collect_assigned_names_from_expr(cond, assigned_names, interner);
                }
            }
        }
        Expr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation { expr, .. } = part {
                    collect_assigned_names_from_expr(expr, assigned_names, interner);
                }
            }
        }
        Expr::Slice { lower, upper, step } => {
            if let Some(e) = lower {
                collect_assigned_names_from_expr(e, assigned_names, interner);
            }
            if let Some(e) = upper {
                collect_assigned_names_from_expr(e, assigned_names, interner);
            }
            if let Some(e) = step {
                collect_assigned_names_from_expr(e, assigned_names, interner);
            }
        }
        // Lambda bodies have their own scope - walrus inside them doesn't affect us
        Expr::LambdaRaw { .. } | Expr::Lambda { .. } => {}
        // Leaf expressions don't contain walrus operators
        Expr::Literal(_) | Expr::Builtin(_) | Expr::Name(_) => {}
    }
}

/// Helper to collect assigned names from argument expressions.
fn collect_assigned_names_from_args(
    args: &ArgExprs,
    assigned_names: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match args {
        ArgExprs::Empty => {}
        ArgExprs::One(arg) => collect_assigned_names_from_expr(arg, assigned_names, interner),
        ArgExprs::Two(arg1, arg2) => {
            collect_assigned_names_from_expr(arg1, assigned_names, interner);
            collect_assigned_names_from_expr(arg2, assigned_names, interner);
        }
        ArgExprs::Args(args) => {
            for arg in args {
                collect_assigned_names_from_expr(arg, assigned_names, interner);
            }
        }
        ArgExprs::Kwargs(kwargs) => {
            for kwarg in kwargs {
                collect_assigned_names_from_expr(&kwarg.value, assigned_names, interner);
            }
        }
        ArgExprs::ArgsKargs {
            args,
            kwargs,
            var_args,
            var_kwargs,
        } => {
            if let Some(args) = args {
                for arg in args {
                    collect_assigned_names_from_expr(arg, assigned_names, interner);
                }
            }
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    collect_assigned_names_from_expr(&kwarg.value, assigned_names, interner);
                }
            }
            if let Some(var_args) = var_args {
                collect_assigned_names_from_expr(var_args, assigned_names, interner);
            }
            if let Some(var_kwargs) = var_kwargs {
                collect_assigned_names_from_expr(var_kwargs, assigned_names, interner);
            }
        }
        ArgExprs::GeneralizedCall { args, kwargs } => {
            for arg in args {
                match arg {
                    CallArg::Value(e) | CallArg::Unpack(e) => {
                        collect_assigned_names_from_expr(e, assigned_names, interner);
                    }
                }
            }
            for kwarg in kwargs {
                match kwarg {
                    CallKwarg::Named(kw) => {
                        collect_assigned_names_from_expr(&kw.value, assigned_names, interner);
                    }
                    CallKwarg::Unpack(e) => {
                        collect_assigned_names_from_expr(e, assigned_names, interner);
                    }
                }
            }
        }
    }
}

/// Collects cell_vars by analyzing what nested functions capture from our scope.
///
/// For each FunctionDef node, we recursively analyze its body to find what names it
/// references. Any name that is in `our_locals` and referenced by the nested function
/// (not as a local of the nested function) becomes a cell_var.
fn collect_cell_vars_from_node(
    node: &ParseNode,
    our_locals: &AHashSet<String>,
    cell_vars: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match node {
        Node::FunctionDef(RawFunctionDef { signature, body, .. }) => {
            // This nested function's *default* expressions are evaluated in OUR
            // scope at definition time, not inside the nested function — so any
            // name they reference that is one of our locals is captured by us,
            // regardless of the nested function's own params/assignments (cf.
            // the `def f(a=a)` gotcha, where the right-hand `a` is enclosing).
            // Body references are filtered below; defaults are not.
            for default in signature.default_exprs() {
                let mut default_referenced = AHashSet::new();
                collect_referenced_names_from_expr(default, &mut default_referenced, interner);
                for name in &default_referenced {
                    if our_locals.contains(name) {
                        cell_vars.insert(name.clone());
                    }
                }
            }

            // Find what names are referenced inside this nested function
            let mut referenced = AHashSet::new();
            for n in body {
                collect_referenced_names_from_node(n, &mut referenced, interner);
            }

            // Extract param names from signature for scope analysis
            let param_names: Vec<StringId> = signature.param_names().collect();

            // Collect *only* this nested function's own bindings (params +
            // assigned + global/nonlocal declarations). Use
            // `collect_scope_info_from_node`, which does NOT descend into
            // further-nested functions, rather than `collect_function_scope_info`:
            // the latter re-runs this entire cell-var pass for the nested body,
            // which — combined with the transitive recursion below — would make
            // the analysis exponential in nesting depth (`C(d) = 2·C(d-1)`).
            // The deeper captures are found by the explicit recursion instead.
            let mut nested_global = AHashSet::new();
            let mut nested_nonlocal = AHashSet::new();
            let mut nested_assigned = AHashSet::new();
            for n in body {
                collect_scope_info_from_node(
                    n,
                    &mut nested_global,
                    &mut nested_nonlocal,
                    &mut nested_assigned,
                    interner,
                );
            }

            // Any name that is:
            // - Referenced by the nested function
            // - Not a local of the nested function
            // - Not declared global in the nested function
            // - In our locals
            // becomes a cell_var
            for name in &referenced {
                if !nested_assigned.contains(name)
                    && !param_names.iter().any(|p| interner.get_str(*p) == name)
                    && !nested_global.contains(name)
                    && our_locals.contains(name)
                {
                    cell_vars.insert(name.clone());
                }
            }

            // Also check what the nested function explicitly declares as nonlocal
            for name in &nested_nonlocal {
                if our_locals.contains(name) {
                    cell_vars.insert(name.clone());
                }
            }

            // Transitive captures: a function nested *inside* this one can also
            // capture one of our locals (e.g. `outer` -> `mid` -> `inner`
            // reading an `outer` variable), unless an intermediate scope rebinds
            // the name. Recurse into this function's body with our locals minus
            // this function's own bindings, so deeper closures over our
            // variables are recognised as cells *before* their references are
            // resolved — otherwise the variable would be compiled as a plain
            // local here and then promoted inconsistently.
            let mut deeper_locals = our_locals.clone();
            for param_id in &param_names {
                deeper_locals.remove(interner.get_str(*param_id));
            }
            for name in &nested_assigned {
                deeper_locals.remove(name);
            }
            for name in &nested_global {
                deeper_locals.remove(name);
            }
            if !deeper_locals.is_empty() {
                for n in body {
                    collect_cell_vars_from_node(n, &deeper_locals, cell_vars, interner);
                }
            }
        }
        // Recurse into control flow structures
        Node::For {
            iter, body, or_else, ..
        } => {
            collect_cell_vars_from_expr(iter, our_locals, cell_vars, interner);
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        Node::While { test, body, or_else } => {
            collect_cell_vars_from_expr(test, our_locals, cell_vars, interner);
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        Node::If { test, body, or_else } => {
            collect_cell_vars_from_expr(test, our_locals, cell_vars, interner);
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        Node::Try(Try {
            body,
            handlers,
            or_else,
            finally,
        }) => {
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for handler in handlers {
                for n in &handler.body {
                    collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
                }
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in finally {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        Node::With { context, body, .. } => {
            collect_cell_vars_from_expr(context, our_locals, cell_vars, interner);
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        // Handle expressions that may contain lambdas
        Node::Expr(expr) | Node::Return(Some(expr)) => {
            collect_cell_vars_from_expr(expr, our_locals, cell_vars, interner);
        }
        Node::Return(None) => {}
        Node::Assign { object, .. } | Node::UnpackAssign { object, .. } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
        }
        Node::OpAssign { value, .. } => {
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        Node::SubscriptOpAssign {
            target, index, value, ..
        } => {
            collect_cell_vars_from_expr(target, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(index, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        Node::SubscriptAssign {
            target, index, value, ..
        } => {
            collect_cell_vars_from_expr(target, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(index, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        Node::AttrOpAssign { object, value, .. } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        Node::AttrAssign { object, value, .. } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        Node::ChainAssign { targets, object } => {
            for target in targets {
                collect_cell_vars_from_assign_target(target, our_locals, cell_vars, interner);
            }
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
        }
        // Other nodes don't contain nested function definitions or lambdas
        _ => {}
    }
}

/// Collects cell_vars from lambda expressions within an expression.
///
/// Recursively searches through an expression tree to find lambda expressions
/// that capture variables from the enclosing scope.
fn collect_cell_vars_from_expr(
    expr: &ExprLoc,
    our_locals: &AHashSet<String>,
    cell_vars: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    use crate::expressions::Expr;
    match &expr.expr {
        Expr::LambdaRaw { signature, body, .. } => {
            // This lambda's *default* expressions are evaluated in OUR scope at
            // definition time, not inside the lambda — so any name they
            // reference that is one of our locals is captured by us, regardless
            // of the lambda's own params. Crucially the default must NOT be
            // filtered by the lambda's params: in `lambda x=(lambda: x): x()`
            // the inner lambda captures the enclosing `x`, not the param `x`,
            // so filtering would drop the required outer cell. Body references
            // are filtered below; defaults are not.
            for default in signature.default_exprs() {
                let mut default_referenced = AHashSet::new();
                collect_referenced_names_from_expr(default, &mut default_referenced, interner);
                for name in &default_referenced {
                    if our_locals.contains(name) {
                        cell_vars.insert(name.clone());
                    }
                }
            }

            // Find what names are referenced in the lambda body
            let mut referenced = AHashSet::new();
            collect_referenced_names_from_expr(body, &mut referenced, interner);

            // Extract param names from signature
            let param_names: Vec<StringId> = signature.param_names().collect();

            // A body reference becomes a cell_var if it is not one of the
            // lambda's own params (which the lambda binds itself) and is one of
            // our locals.
            for name in &referenced {
                if !param_names.iter().any(|p| interner.get_str(*p) == name) && our_locals.contains(name) {
                    cell_vars.insert(name.clone());
                }
            }

            // Recursively check the lambda body for nested lambdas.
            // For nested lambdas, extend our_locals to include this lambda's parameters
            // so that inner lambdas can find them for closure capture.
            let mut extended_locals = our_locals.clone();
            for param_id in &param_names {
                extended_locals.insert(interner.get_str(*param_id).to_string());
            }
            collect_cell_vars_from_expr(body, &extended_locals, cell_vars, interner);
        }
        // Recurse into sub-expressions
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                let expr = match item {
                    SequenceItem::Value(e) | SequenceItem::Unpack(e) => e,
                };
                collect_cell_vars_from_expr(expr, our_locals, cell_vars, interner);
            }
        }
        Expr::Dict(dict_items) => {
            for item in dict_items {
                match item {
                    DictItem::Pair(key, value) => {
                        collect_cell_vars_from_expr(key, our_locals, cell_vars, interner);
                        collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
                    }
                    DictItem::Unpack(e) => collect_cell_vars_from_expr(e, our_locals, cell_vars, interner),
                }
            }
        }
        Expr::Op { left, right, .. } | Expr::CmpOp { left, right, .. } => {
            collect_cell_vars_from_expr(left, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(right, our_locals, cell_vars, interner);
        }
        Expr::ChainCmp { left, comparisons } => {
            collect_cell_vars_from_expr(left, our_locals, cell_vars, interner);
            for (_, expr) in comparisons {
                collect_cell_vars_from_expr(expr, our_locals, cell_vars, interner);
            }
        }
        Expr::Not(operand) | Expr::UnaryMinus(operand) | Expr::UnaryPlus(operand) | Expr::UnaryInvert(operand) => {
            collect_cell_vars_from_expr(operand, our_locals, cell_vars, interner);
        }
        Expr::Subscript { object, index } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(index, our_locals, cell_vars, interner);
        }
        Expr::Call { args, .. } => {
            collect_cell_vars_from_args(args, our_locals, cell_vars, interner);
        }
        Expr::AttrCall { object, args, .. } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
            collect_cell_vars_from_args(args, our_locals, cell_vars, interner);
        }
        Expr::IndirectCall { callable, args } => {
            collect_cell_vars_from_expr(callable, our_locals, cell_vars, interner);
            collect_cell_vars_from_args(args, our_locals, cell_vars, interner);
        }
        Expr::AttrGet { object, .. } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
        }
        Expr::IfElse { test, body, orelse } => {
            collect_cell_vars_from_expr(test, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(body, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(orelse, our_locals, cell_vars, interner);
        }
        Expr::ListComp { elt, generators } | Expr::SetComp { elt, generators } => {
            collect_cell_vars_from_expr(elt, our_locals, cell_vars, interner);
            for generator in generators {
                collect_cell_vars_from_expr(&generator.iter, our_locals, cell_vars, interner);
                for cond in &generator.ifs {
                    collect_cell_vars_from_expr(cond, our_locals, cell_vars, interner);
                }
            }
        }
        Expr::DictComp { key, value, generators } => {
            collect_cell_vars_from_expr(key, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
            for generator in generators {
                collect_cell_vars_from_expr(&generator.iter, our_locals, cell_vars, interner);
                for cond in &generator.ifs {
                    collect_cell_vars_from_expr(cond, our_locals, cell_vars, interner);
                }
            }
        }
        Expr::FString(parts) => {
            for part in parts {
                if let FStringPart::Interpolation { expr, .. } = part {
                    collect_cell_vars_from_expr(expr, our_locals, cell_vars, interner);
                }
            }
        }
        Expr::Named { value, .. } => {
            // Only scan the value expression for cell vars
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        Expr::Await(value) => {
            collect_cell_vars_from_expr(value, our_locals, cell_vars, interner);
        }
        // Leaf expressions
        Expr::Literal(_) | Expr::Builtin(_) | Expr::Name(_) | Expr::Lambda { .. } | Expr::Slice { .. } => {}
    }
}

/// Helper to collect cell vars from argument expressions.
fn collect_cell_vars_from_args(
    args: &ArgExprs,
    our_locals: &AHashSet<String>,
    cell_vars: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match args {
        ArgExprs::Empty => {}
        ArgExprs::One(arg) => collect_cell_vars_from_expr(arg, our_locals, cell_vars, interner),
        ArgExprs::Two(arg1, arg2) => {
            collect_cell_vars_from_expr(arg1, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(arg2, our_locals, cell_vars, interner);
        }
        ArgExprs::Args(args) => {
            for arg in args {
                collect_cell_vars_from_expr(arg, our_locals, cell_vars, interner);
            }
        }
        ArgExprs::Kwargs(kwargs) => {
            for kwarg in kwargs {
                collect_cell_vars_from_expr(&kwarg.value, our_locals, cell_vars, interner);
            }
        }
        ArgExprs::ArgsKargs {
            args,
            kwargs,
            var_args,
            var_kwargs,
        } => {
            if let Some(args) = args {
                for arg in args {
                    collect_cell_vars_from_expr(arg, our_locals, cell_vars, interner);
                }
            }
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    collect_cell_vars_from_expr(&kwarg.value, our_locals, cell_vars, interner);
                }
            }
            if let Some(var_args) = var_args {
                collect_cell_vars_from_expr(var_args, our_locals, cell_vars, interner);
            }
            if let Some(var_kwargs) = var_kwargs {
                collect_cell_vars_from_expr(var_kwargs, our_locals, cell_vars, interner);
            }
        }
        ArgExprs::GeneralizedCall { args, kwargs } => {
            for arg in args {
                match arg {
                    CallArg::Value(e) | CallArg::Unpack(e) => {
                        collect_cell_vars_from_expr(e, our_locals, cell_vars, interner);
                    }
                }
            }
            for kwarg in kwargs {
                match kwarg {
                    CallKwarg::Named(kw) => {
                        collect_cell_vars_from_expr(&kw.value, our_locals, cell_vars, interner);
                    }
                    CallKwarg::Unpack(e) => {
                        collect_cell_vars_from_expr(e, our_locals, cell_vars, interner);
                    }
                }
            }
        }
    }
}

/// Collects all names referenced (read) in a node and its descendants.
///
/// This is used to find what names a nested function references from enclosing scopes.
fn collect_referenced_names_from_node(node: &ParseNode, referenced: &mut AHashSet<String>, interner: &InternerBuilder) {
    match node {
        Node::Expr(expr) | Node::Return(Some(expr)) | Node::Raise(Some(expr)) => {
            collect_referenced_names_from_expr(expr, referenced, interner);
        }
        Node::Return(None) | Node::Raise(None) => {}
        Node::Assert { test, msg } => {
            collect_referenced_names_from_expr(test, referenced, interner);
            if let Some(m) = msg {
                collect_referenced_names_from_expr(m, referenced, interner);
            }
        }
        Node::Assign { object, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        Node::UnpackAssign { object, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        Node::OpAssign { target, value, .. } => {
            // OpAssign reads the target before writing
            referenced.insert(interner.get_str(target.name_id).to_string());
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Node::SubscriptOpAssign {
            target, index, value, ..
        } => {
            collect_referenced_names_from_expr(target, referenced, interner);
            collect_referenced_names_from_expr(index, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Node::SubscriptAssign {
            target, index, value, ..
        } => {
            collect_referenced_names_from_expr(target, referenced, interner);
            collect_referenced_names_from_expr(index, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Node::AttrOpAssign { object, value, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Node::AttrAssign { object, value, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Node::ChainAssign { targets, object } => {
            for target in targets {
                collect_referenced_names_from_assign_target(target, referenced, interner);
            }
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        Node::For {
            iter, body, or_else, ..
        } => {
            collect_referenced_names_from_expr(iter, referenced, interner);
            for n in body {
                collect_referenced_names_from_node(n, referenced, interner);
            }
            for n in or_else {
                collect_referenced_names_from_node(n, referenced, interner);
            }
        }
        Node::While { test, body, or_else } => {
            collect_referenced_names_from_expr(test, referenced, interner);
            for n in body {
                collect_referenced_names_from_node(n, referenced, interner);
            }
            for n in or_else {
                collect_referenced_names_from_node(n, referenced, interner);
            }
        }
        Node::If { test, body, or_else } => {
            collect_referenced_names_from_expr(test, referenced, interner);
            for n in body {
                collect_referenced_names_from_node(n, referenced, interner);
            }
            for n in or_else {
                collect_referenced_names_from_node(n, referenced, interner);
            }
        }
        Node::FunctionDef(_) => {
            // Don't recurse into nested function bodies - they have their own scope
        }
        Node::Try(Try {
            body,
            handlers,
            or_else,
            finally,
        }) => {
            for n in body {
                collect_referenced_names_from_node(n, referenced, interner);
            }
            for handler in handlers {
                // Exception type expression may reference names
                if let Some(ref exc_type) = handler.exc_type {
                    collect_referenced_names_from_expr(exc_type, referenced, interner);
                }
                for n in &handler.body {
                    collect_referenced_names_from_node(n, referenced, interner);
                }
            }
            for n in or_else {
                collect_referenced_names_from_node(n, referenced, interner);
            }
            for n in finally {
                collect_referenced_names_from_node(n, referenced, interner);
            }
        }
        Node::With { context, body, .. } => {
            collect_referenced_names_from_expr(context, referenced, interner);
            for n in body {
                collect_referenced_names_from_node(n, referenced, interner);
            }
        }
        // Imports create bindings but don't reference names
        Node::Import { .. } | Node::ImportFrom { .. } => {}
        Node::Pass | Node::Global { .. } | Node::Nonlocal { .. } | Node::Break { .. } | Node::Continue { .. } => {}
    }
}

/// Collects all names referenced in an expression.
fn collect_referenced_names_from_expr(expr: &ExprLoc, referenced: &mut AHashSet<String>, interner: &InternerBuilder) {
    match &expr.expr {
        Expr::Name(ident) => {
            referenced.insert(interner.get_str(ident.name_id).to_string());
        }
        Expr::Literal(_) => {}
        Expr::Builtin(_) => {}
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                let expr = match item {
                    SequenceItem::Value(e) | SequenceItem::Unpack(e) => e,
                };
                collect_referenced_names_from_expr(expr, referenced, interner);
            }
        }
        Expr::Dict(dict_items) => {
            for item in dict_items {
                match item {
                    DictItem::Pair(key, value) => {
                        collect_referenced_names_from_expr(key, referenced, interner);
                        collect_referenced_names_from_expr(value, referenced, interner);
                    }
                    DictItem::Unpack(e) => collect_referenced_names_from_expr(e, referenced, interner),
                }
            }
        }
        Expr::Op { left, right, .. } | Expr::CmpOp { left, right, .. } => {
            collect_referenced_names_from_expr(left, referenced, interner);
            collect_referenced_names_from_expr(right, referenced, interner);
        }
        Expr::ChainCmp { left, comparisons } => {
            collect_referenced_names_from_expr(left, referenced, interner);
            for (_, expr) in comparisons {
                collect_referenced_names_from_expr(expr, referenced, interner);
            }
        }
        Expr::Not(operand) | Expr::UnaryMinus(operand) | Expr::UnaryPlus(operand) | Expr::UnaryInvert(operand) => {
            collect_referenced_names_from_expr(operand, referenced, interner);
        }
        Expr::FString(parts) => {
            collect_referenced_names_from_fstring_parts(parts, referenced, interner);
        }
        Expr::Subscript { object, index } => {
            collect_referenced_names_from_expr(object, referenced, interner);
            collect_referenced_names_from_expr(index, referenced, interner);
        }
        Expr::Call { callable, args } => {
            // Check if the callable is a Name reference
            if let Callable::Name(ident) = callable {
                referenced.insert(interner.get_str(ident.name_id).to_string());
            }
            collect_referenced_names_from_args(args, referenced, interner);
        }
        Expr::AttrCall { object, args, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
            collect_referenced_names_from_args(args, referenced, interner);
        }
        Expr::AttrGet { object, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        Expr::IndirectCall { callable, args } => {
            // Collect references from the callable expression and arguments
            collect_referenced_names_from_expr(callable, referenced, interner);
            collect_referenced_names_from_args(args, referenced, interner);
        }
        Expr::IfElse { test, body, orelse } => {
            collect_referenced_names_from_expr(test, referenced, interner);
            collect_referenced_names_from_expr(body, referenced, interner);
            collect_referenced_names_from_expr(orelse, referenced, interner);
        }
        Expr::ListComp { elt, generators } | Expr::SetComp { elt, generators } => {
            collect_referenced_names_from_comprehension(generators, Some(elt), None, referenced, interner);
        }
        Expr::DictComp { key, value, generators } => {
            collect_referenced_names_from_comprehension(generators, None, Some((key, value)), referenced, interner);
        }
        Expr::LambdaRaw { signature, body, .. } => {
            // Build set of parameter names (these are local to the lambda, not free variables)
            let lambda_params: AHashSet<String> = signature
                .param_names()
                .map(|s| interner.get_str(s).to_string())
                .collect();

            // Collect references from the body expression into a temporary set
            let mut body_refs: AHashSet<String> = AHashSet::new();
            collect_referenced_names_from_expr(body, &mut body_refs, interner);

            // Filter out the lambda's own parameters before adding to referenced set.
            // The lambda's parameters are bound by the lambda, not free from outer scope.
            for name in body_refs {
                if !lambda_params.contains(&name) {
                    referenced.insert(name);
                }
            }

            // Default value expressions are evaluated in the enclosing scope, not the lambda's
            // scope, so they can reference outer scope without filtering.
            for param in &signature.pos_args {
                if let Some(ref default) = param.default {
                    collect_referenced_names_from_expr(default, referenced, interner);
                }
            }
            for param in &signature.args {
                if let Some(ref default) = param.default {
                    collect_referenced_names_from_expr(default, referenced, interner);
                }
            }
            for param in &signature.kwargs {
                if let Some(ref default) = param.default {
                    collect_referenced_names_from_expr(default, referenced, interner);
                }
            }
        }
        Expr::Lambda { .. } => {
            // Lambda should only exist after preparation; this function operates on raw expressions
            unreachable!("Expr::Lambda should not exist during scope analysis")
        }
        Expr::Named { value, .. } => {
            // Only the value is referenced; target is being assigned, not read
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Expr::Slice { lower, upper, step } => {
            if let Some(expr) = lower {
                collect_referenced_names_from_expr(expr, referenced, interner);
            }
            if let Some(expr) = upper {
                collect_referenced_names_from_expr(expr, referenced, interner);
            }
            if let Some(expr) = step {
                collect_referenced_names_from_expr(expr, referenced, interner);
            }
        }
        Expr::Await(value) => {
            collect_referenced_names_from_expr(value, referenced, interner);
        }
    }
}

/// Collects referenced names from comprehension expressions.
///
/// Handles the special scoping rules: loop variables are local to the comprehension,
/// so we collect references from iterators and conditions but exclude loop variable names.
fn collect_referenced_names_from_comprehension(
    generators: &[Comprehension],
    elt: Option<&ExprLoc>,
    key_value: Option<(&ExprLoc, &ExprLoc)>,
    referenced: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    // Track loop variable names (these are local to the comprehension)
    let mut comp_locals: AHashSet<String> = AHashSet::new();

    // Collect references from expressions that can see prior loop variables.
    // These need to be filtered against comp_locals before adding to referenced.
    let mut inner_refs: AHashSet<String> = AHashSet::new();

    for (i, comp) in generators.iter().enumerate() {
        if i == 0 {
            // FIRST generator's iter expression truly references enclosing scope
            // (evaluated before any loop variable is defined).
            collect_referenced_names_from_expr(&comp.iter, referenced, interner);
        } else {
            // SUBSEQUENT generators' iter expressions can reference prior loop variables.
            // For example, in `[y for x in xs for y in x]`, the `x` in the second
            // generator's iter is the first generator's loop variable, not outer scope.
            collect_referenced_names_from_expr(&comp.iter, &mut inner_refs, interner);
        }

        // Add this generator's target(s) to local set
        collect_names_from_unpack_target(&comp.target, &mut comp_locals, interner);

        // Filter conditions can see prior loop variables - collect separately
        for cond in &comp.ifs {
            collect_referenced_names_from_expr(cond, &mut inner_refs, interner);
        }
    }

    // Element expression(s) can see all loop variables - collect separately
    if let Some(e) = elt {
        collect_referenced_names_from_expr(e, &mut inner_refs, interner);
    }
    if let Some((k, v)) = key_value {
        collect_referenced_names_from_expr(k, &mut inner_refs, interner);
        collect_referenced_names_from_expr(v, &mut inner_refs, interner);
    }

    // Add inner references that are NOT comprehension-locals to the outer referenced set.
    // Names that ARE comp_locals refer to the comprehension's loop variable, not enclosing scope.
    for name in inner_refs {
        if !comp_locals.contains(&name) {
            referenced.insert(name);
        }
    }
}

/// Collects referenced names from argument expressions.
fn collect_referenced_names_from_args(args: &ArgExprs, referenced: &mut AHashSet<String>, interner: &InternerBuilder) {
    match args {
        ArgExprs::Empty => {}
        ArgExprs::One(e) => collect_referenced_names_from_expr(e, referenced, interner),
        ArgExprs::Two(e1, e2) => {
            collect_referenced_names_from_expr(e1, referenced, interner);
            collect_referenced_names_from_expr(e2, referenced, interner);
        }
        ArgExprs::Args(exprs) => {
            for e in exprs {
                collect_referenced_names_from_expr(e, referenced, interner);
            }
        }
        ArgExprs::Kwargs(kwargs) => {
            for kwarg in kwargs {
                collect_referenced_names_from_expr(&kwarg.value, referenced, interner);
            }
        }
        ArgExprs::ArgsKargs {
            args,
            kwargs,
            var_args,
            var_kwargs,
        } => {
            if let Some(args) = args {
                for e in args {
                    collect_referenced_names_from_expr(e, referenced, interner);
                }
            }
            if let Some(kwargs) = kwargs {
                for kwarg in kwargs {
                    collect_referenced_names_from_expr(&kwarg.value, referenced, interner);
                }
            }
            if let Some(e) = var_args {
                collect_referenced_names_from_expr(e, referenced, interner);
            }
            if let Some(e) = var_kwargs {
                collect_referenced_names_from_expr(e, referenced, interner);
            }
        }
        ArgExprs::GeneralizedCall { args, kwargs } => {
            for arg in args {
                match arg {
                    CallArg::Value(e) | CallArg::Unpack(e) => {
                        collect_referenced_names_from_expr(e, referenced, interner);
                    }
                }
            }
            for kwarg in kwargs {
                match kwarg {
                    CallKwarg::Named(kw) => {
                        collect_referenced_names_from_expr(&kw.value, referenced, interner);
                    }
                    CallKwarg::Unpack(e) => {
                        collect_referenced_names_from_expr(e, referenced, interner);
                    }
                }
            }
        }
    }
}

/// Collects referenced names from f-string parts (both expressions and dynamic format specs).
fn collect_referenced_names_from_fstring_parts(
    parts: &[FStringPart],
    referenced: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    for part in parts {
        if let FStringPart::Interpolation { expr, format_spec, .. } = part {
            collect_referenced_names_from_expr(expr, referenced, interner);
            // Also check dynamic format specs which can contain interpolated expressions
            if let Some(FormatSpec::Dynamic(spec_parts)) = format_spec {
                collect_referenced_names_from_fstring_parts(spec_parts, referenced, interner);
            }
        }
    }
}

/// Collects all names from an unpack target into the given set.
///
/// Recursively traverses nested tuples to find all identifier names.
fn collect_names_from_unpack_target(target: &UnpackTarget, names: &mut AHashSet<String>, interner: &InternerBuilder) {
    match target {
        UnpackTarget::Name(ident) | UnpackTarget::Starred(ident) => {
            names.insert(interner.get_str(ident.name_id).to_string());
        }
        UnpackTarget::Tuple { targets, .. } => {
            for t in targets {
                collect_names_from_unpack_target(t, names, interner);
            }
        }
    }
}

/// Collects newly-assigned names and walrus bindings introduced by a single chained-assign target.
///
/// Mirrors the per-shape logic in `collect_scope_info_from_node` for the non-chained
/// assignment nodes: name/unpack targets bind new names, while subscript/attribute
/// targets only scan their sub-expressions for walrus bindings since they mutate an
/// existing container rather than introducing a new binding.
fn collect_assigned_names_from_assign_target(
    target: &AssignTarget,
    assigned_names: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match target {
        AssignTarget::Name(ident) => {
            assigned_names.insert(interner.get_str(ident.name_id).to_string());
        }
        AssignTarget::Subscript { target, index, .. } => {
            collect_assigned_names_from_expr(target, assigned_names, interner);
            collect_assigned_names_from_expr(index, assigned_names, interner);
        }
        AssignTarget::Attr { object, .. } => {
            collect_assigned_names_from_expr(object, assigned_names, interner);
        }
        AssignTarget::Unpack { targets, .. } => {
            for t in targets {
                collect_names_from_unpack_target(t, assigned_names, interner);
            }
        }
    }
}

/// Collects cell variables referenced by sub-expressions inside a chained-assign target.
///
/// Subscript and attribute targets embed arbitrary expressions that may contain lambdas
/// capturing enclosing variables; pure name/unpack targets do not carry expressions and
/// therefore contribute nothing to the cell-variable set.
fn collect_cell_vars_from_assign_target(
    target: &AssignTarget,
    our_locals: &AHashSet<String>,
    cell_vars: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match target {
        AssignTarget::Subscript { target, index, .. } => {
            collect_cell_vars_from_expr(target, our_locals, cell_vars, interner);
            collect_cell_vars_from_expr(index, our_locals, cell_vars, interner);
        }
        AssignTarget::Attr { object, .. } => {
            collect_cell_vars_from_expr(object, our_locals, cell_vars, interner);
        }
        AssignTarget::Name(_) | AssignTarget::Unpack { .. } => {}
    }
}

/// Collects names referenced (read) by sub-expressions inside a chained-assign target.
///
/// Only subscript and attribute targets read from surrounding state: the container or
/// object expression must be evaluated at store time. Name and unpack targets do not
/// reference any names on the read side.
fn collect_referenced_names_from_assign_target(
    target: &AssignTarget,
    referenced: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    match target {
        AssignTarget::Subscript { target, index, .. } => {
            collect_referenced_names_from_expr(target, referenced, interner);
            collect_referenced_names_from_expr(index, referenced, interner);
        }
        AssignTarget::Attr { object, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        AssignTarget::Name(_) | AssignTarget::Unpack { .. } => {}
    }
}
