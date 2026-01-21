use std::collections::hash_map::Entry;

use ahash::{AHashMap, AHashSet};

use crate::{
    args::ArgExprs,
    expressions::{
        Callable, CmpOperator, Comprehension, Expr, ExprLoc, Identifier, Literal, NameScope, Node, Operator,
        PreparedFunctionDef, PreparedNode, UnpackTarget,
    },
    fstring::{FStringPart, FormatSpec},
    intern::{InternerBuilder, StringId},
    namespace::NamespaceId,
    parse::{ExceptHandler, ParseError, ParseNode, ParseResult, ParsedSignature, RawFunctionDef, Try},
    signature::Signature,
};

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
    /// Used for ref-count testing to look up variables by name.
    /// Only available when the `ref-count-return` feature is enabled.
    #[cfg(feature = "ref-count-return")]
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
pub(crate) fn prepare(
    parse_result: ParseResult,
    input_names: Vec<String>,
    external_functions: &[String],
) -> Result<PrepareResult, ParseError> {
    let ParseResult { nodes, interner } = parse_result;
    let mut p = Prepare::new_module(input_names, external_functions, &interner);
    let mut prepared_nodes = p.prepare_nodes(nodes)?;

    // In the root frame, the last expression is implicitly returned
    // if it's not None. This matches Python REPL behavior where the last expression
    // value is displayed/returned.
    if let Some(Node::Expr(expr_loc)) = prepared_nodes.last()
        && !expr_loc.expr.is_none()
    {
        let new_expr_loc = expr_loc.clone();
        prepared_nodes.pop();
        prepared_nodes.push(Node::Return(new_expr_loc));
    }

    Ok(PrepareResult {
        namespace_size: p.namespace_size,
        #[cfg(feature = "ref-count-return")]
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
struct Prepare<'i> {
    /// Reference to the string interner for looking up names in error messages.
    interner: &'i InternerBuilder,
    /// Maps variable names to their indices in this scope's namespace vector
    name_map: AHashMap<String, NamespaceId>,
    /// Number of items in the namespace
    pub namespace_size: usize,
    /// Whether this is the module-level scope.
    /// At module level, all variables are global and `global` keyword is a no-op.
    is_module_scope: bool,
    /// Names declared as `global` in this scope.
    /// These names will resolve to the global namespace instead of local.
    global_names: AHashSet<String>,
    /// Names that are assigned in this scope (from first-pass scan).
    /// Used in functions to determine if a variable is local (assigned) or global (only read).
    assigned_names: AHashSet<String>,
    /// Names that have been assigned so far during the second pass (in order).
    /// Used to produce the correct error message for `global x` when x was assigned before.
    names_assigned_in_order: AHashSet<String>,
    /// Copy of the module-level global name map.
    /// Used by functions to resolve global variable references.
    /// None at module level (not needed since all names are global there).
    global_name_map: Option<AHashMap<String, NamespaceId>>,
    /// Names that exist as locals in the enclosing function scope.
    /// Used to validate `nonlocal` declarations and resolve captured variables.
    /// None at module level or when there's no enclosing function.
    enclosing_locals: Option<AHashSet<String>>,
    /// Maps free variable names (from nonlocal declarations and implicit captures) to their
    /// index in the free_vars vector. Pre-populated with nonlocal names at initialization,
    /// then extended with implicit captures discovered during preparation.
    free_var_map: AHashMap<String, NamespaceId>,
    /// Maps cell variable names to their index in the owned_cells vector.
    /// Pre-populated with cell_var names at initialization (excluding pass-through variables
    /// that are both nonlocal and captured by nested functions), then extended as new
    /// captures are discovered during nested function preparation.
    cell_var_map: AHashMap<String, NamespaceId>,
}

impl<'i> Prepare<'i> {
    /// Creates a new Prepare instance for module-level code.
    ///
    /// At module level, all variables are global. The `global` keyword is a no-op
    /// since all variables are already in the global namespace.
    ///
    /// # Arguments
    /// * `input_names` - Names that should be pre-registered in the namespace (e.g., external variables)
    /// * `external_functions` - Names of external functions to pre-register
    /// * `interner` - Reference to the string interner for looking up names
    fn new_module(input_names: Vec<String>, external_functions: &[String], interner: &'i InternerBuilder) -> Self {
        let mut name_map = AHashMap::with_capacity(input_names.len() + external_functions.len());
        for (index, name) in external_functions.iter().enumerate() {
            name_map.insert(name.clone(), NamespaceId::new(index));
        }
        for (index, name) in input_names.into_iter().enumerate() {
            name_map.insert(name, NamespaceId::new(external_functions.len() + index));
        }
        let namespace_size = name_map.len();
        Self {
            interner,
            name_map,
            namespace_size,
            is_module_scope: true,
            global_names: AHashSet::new(),
            assigned_names: AHashSet::new(),
            names_assigned_in_order: AHashSet::new(),
            global_name_map: None,
            enclosing_locals: None,
            free_var_map: AHashMap::new(),
            cell_var_map: AHashMap::new(),
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
        assigned_names: AHashSet<String>,
        global_names: AHashSet<String>,
        nonlocal_names: AHashSet<String>,
        implicit_captures: AHashSet<String>,
        global_name_map: AHashMap<String, NamespaceId>,
        enclosing_locals: Option<AHashSet<String>>,
        cell_var_names: AHashSet<String>,
        interner: &'i InternerBuilder,
    ) -> Self {
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, string_id) in params.iter().enumerate() {
            name_map.insert(interner.get_str(*string_id).to_string(), NamespaceId::new(index));
        }
        let namespace_size = name_map.len();

        // Namespace layout: [params][cell_vars][free_vars][locals]
        // This predictable layout allows sequential namespace construction at runtime.

        // Pre-populate cell_var_map with cell variables FIRST (right after params).
        // Excludes pass-through variables (names that are both nonlocal and captured by
        // nested functions - these stay in free_var_map since we receive the cell, not create it).
        // NOTE: We intentionally do NOT add these to name_map here, because the scope
        // validation checks name_map to detect "used before declaration" errors
        let mut cell_var_map = AHashMap::with_capacity(cell_var_names.len());
        let mut namespace_size = namespace_size;
        for name in cell_var_names {
            if !nonlocal_names.contains(&name) && !implicit_captures.contains(&name) {
                let slot = namespace_size;
                namespace_size += 1;
                cell_var_map.insert(name, NamespaceId::new(slot));
            }
        }

        // Pre-populate free_var_map with nonlocal declarations AND implicit captures SECOND (after cell_vars).
        // Each entry maps name -> namespace slot index where the cell reference will be stored.
        // NOTE: We intentionally do NOT add these to name_map here, because the nonlocal
        // validation in prepare_nodes checks name_map to detect "used before nonlocal declaration"
        let free_var_capacity = nonlocal_names.len() + implicit_captures.len();
        let mut free_var_map = AHashMap::with_capacity(free_var_capacity);
        for name in nonlocal_names {
            let slot = namespace_size;
            namespace_size += 1;
            free_var_map.insert(name, NamespaceId::new(slot));
        }
        // Implicit captures (variables accessed from enclosing scope without explicit nonlocal)
        for name in implicit_captures {
            let slot = namespace_size;
            namespace_size += 1;
            free_var_map.insert(name, NamespaceId::new(slot));
        }

        Self {
            interner,
            name_map,
            namespace_size,
            is_module_scope: false,
            global_names,
            assigned_names,
            names_assigned_in_order: AHashSet::new(),
            global_name_map: Some(global_name_map),
            enclosing_locals,
            free_var_map,
            cell_var_map,
        }
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
                Node::Return(expr) => new_nodes.push(Node::Return(self.prepare_expression(expr)?)),
                Node::ReturnNone => new_nodes.push(Node::ReturnNone),
                Node::Raise(exc) => {
                    let expr = match exc {
                        Some(expr) => {
                            match expr.expr {
                                // Handle raising an exception type constant without instantiation,
                                // e.g. `raise TypeError`. This is transformed into a call: `raise TypeError()`
                                // so the exception is properly instantiated before being raised.
                                // Also handle raising a builtin constant (unlikely but consistent)
                                Expr::Builtin(b) => {
                                    let call_expr = Expr::Call {
                                        callable: Callable::Builtin(b),
                                        args: Box::new(ArgExprs::Empty),
                                    };
                                    Some(ExprLoc::new(expr.position, call_expr))
                                }
                                Expr::Name(id) => {
                                    // Handle raising a variable - could be an exception type or instance.
                                    // The runtime will determine whether to call it (type) or raise it directly (instance).
                                    let position = id.position;
                                    let (resolved_id, _is_new) = self.get_id(id);
                                    Some(ExprLoc::new(position, Expr::Name(resolved_id)))
                                }
                                _ => Some(self.prepare_expression(expr)?),
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
                    let (target, _) = self.get_id(target);
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
                        .collect();
                    new_nodes.push(Node::UnpackAssign {
                        targets,
                        targets_position,
                        object,
                    });
                }
                Node::OpAssign { target, op, object } => {
                    // Track that this name was assigned
                    self.names_assigned_in_order
                        .insert(self.interner.get_str(target.name_id).to_string());
                    let target = self.get_id(target).0;
                    let object = self.prepare_expression(object)?;
                    new_nodes.push(Node::OpAssign { target, op, object });
                }
                Node::SubscriptAssign {
                    target,
                    index,
                    value,
                    target_position,
                } => {
                    // SubscriptAssign doesn't assign to the target itself, just modifies it
                    let target = self.get_id(target).0;
                    let index = self.prepare_expression(index)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::SubscriptAssign {
                        target,
                        index,
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
                Node::For {
                    target,
                    iter,
                    body,
                    or_else,
                } => {
                    // Prepare target with normal scoping (not comprehension isolation)
                    let target = self.prepare_unpack_target(target);
                    new_nodes.push(Node::For {
                        target,
                        iter: self.prepare_expression(iter)?,
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
                Node::FunctionDef(RawFunctionDef { name, signature, body }) => {
                    let func_node = self.prepare_function_def(name, &signature, body)?;
                    new_nodes.push(func_node);
                }
                Node::Global { names, position } => {
                    // At module level, `global` is a no-op since all variables are already global.
                    // In functions, the global declarations are already collected in the first pass
                    // (see prepare_function_def), so this is also a no-op at this point.
                    // The actual effect happens in get_id where we check global_names.
                    if !self.is_module_scope {
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
                    if self.is_module_scope {
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
                Node::Import { module_name, binding } => {
                    // Resolve the binding identifier to get the namespace slot
                    let (resolved_binding, _) = self.get_id(binding);
                    new_nodes.push(Node::Import {
                        module_name,
                        binding: resolved_binding,
                    });
                }
                Node::ImportFrom {
                    module_name,
                    names,
                    position,
                } => {
                    // Resolve each binding identifier to get namespace slots
                    let resolved_names = names
                        .into_iter()
                        .map(|(import_name, binding)| {
                            let (resolved_binding, _) = self.get_id(binding);
                            (import_name, resolved_binding)
                        })
                        .collect();
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
                Some(self.get_id(ident).0)
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
            Expr::Name(name) => Expr::Name(self.get_id(name).0),
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
            Expr::Call { callable, mut args } => {
                // Prepare the arguments
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                // For Name callables, resolve the identifier in the namespace
                // Don't error here if undefined - let runtime raise NameError with proper traceback
                let callable = match callable {
                    Callable::Name(ident) => Callable::Name(self.get_id(ident).0),
                    // Builtins are already resolved at parse time
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
            Expr::AttrGet { object, attr } => {
                // Prepare the object expression (supports chained access like a.b.c)
                let object = Box::new(self.prepare_expression(*object)?);
                Expr::AttrGet { object, attr }
            }
            Expr::List(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError>>()?;
                Expr::List(expressions)
            }
            Expr::Tuple(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError>>()?;
                Expr::Tuple(expressions)
            }
            Expr::Subscript { object, index } => Expr::Subscript {
                object: Box::new(self.prepare_expression(*object)?),
                index: Box::new(self.prepare_expression(*index)?),
            },
            Expr::Dict(pairs) => {
                let prepared_pairs = pairs
                    .into_iter()
                    .map(|(k, v)| Ok((self.prepare_expression(k)?, self.prepare_expression(v)?)))
                    .collect::<Result<_, ParseError>>()?;
                Expr::Dict(prepared_pairs)
            }
            Expr::Set(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError>>()?;
                Expr::Set(expressions)
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
        // Save current scope state for isolation
        let saved_name_map = self.name_map.clone();
        let saved_assigned_names = self.names_assigned_in_order.clone();
        let saved_free_var_map = self.free_var_map.clone();
        let saved_cell_var_map = self.cell_var_map.clone();
        let saved_enclosing_locals = self.enclosing_locals.clone();

        // Step 1: Prepare first generator's iter in enclosing scope (before any shadowing)
        let mut generators_iter = generators.into_iter();
        let first_gen = generators_iter
            .next()
            .expect("comprehension must have at least one generator");
        let first_iter = self.prepare_expression(first_gen.iter)?;

        // Step 2: Collect and shadow ALL loop variable names from ALL generators.
        // This must happen BEFORE evaluating any subsequent generator's iter expression.
        // We allocate slots but don't mark them as "assigned" yet - this causes
        // UnboundLocalError if a later generator's iter references an earlier-declared
        // but not-yet-assigned loop variable.
        let first_target = self.prepare_unpack_target_for_comprehension(first_gen.target);

        // Collect remaining generators so we can pre-shadow their targets
        let remaining_gens: Vec<Comprehension> = generators_iter.collect();

        // Pre-shadow ALL remaining loop variables before evaluating their iters.
        // This is the key CPython behavior: all loop vars are local to the comprehension,
        // so referencing a later loop var in an earlier iter raises UnboundLocalError.
        let mut preshadowed_targets: Vec<UnpackTarget> = Vec::with_capacity(remaining_gens.len());
        for generator in &remaining_gens {
            preshadowed_targets.push(self.prepare_unpack_target_shadow_only(generator.target.clone()));
        }

        // Prepare first generator's filters (can see first loop variable)
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

        // Step 3: Process remaining generators - their iters now see all loop vars as local
        for (generator, preshadowed_target) in remaining_gens.into_iter().zip(preshadowed_targets) {
            let iter = self.prepare_expression(generator.iter)?;
            let ifs = generator
                .ifs
                .into_iter()
                .map(|cond| self.prepare_expression(cond))
                .collect::<Result<Vec<_>, _>>()?;

            prepared_generators.push(Comprehension {
                target: preshadowed_target,
                iter,
                ifs,
            });
        }

        // Prepare the element expression(s) - can see all loop variables
        let prepared_elt = match elt {
            Some(e) => Some(self.prepare_expression(e)?),
            None => None,
        };
        let prepared_key_value = match key_value {
            Some((k, v)) => Some((self.prepare_expression(k)?, self.prepare_expression(v)?)),
            None => None,
        };

        // Restore scope state - loop variables do not leak to enclosing scope
        self.name_map = saved_name_map;
        self.names_assigned_in_order = saved_assigned_names;
        self.free_var_map = saved_free_var_map;
        self.cell_var_map = saved_cell_var_map;
        self.enclosing_locals = saved_enclosing_locals;

        Ok((prepared_generators, prepared_elt, prepared_key_value))
    }

    /// Prepares an unpack target by resolving identifiers recursively.
    ///
    /// Handles both single identifiers and nested tuples like `(a, b), c`.
    fn prepare_unpack_target(&mut self, target: UnpackTarget) -> UnpackTarget {
        match target {
            UnpackTarget::Name(ident) => {
                self.names_assigned_in_order
                    .insert(self.interner.get_str(ident.name_id).to_string());
                UnpackTarget::Name(self.get_id(ident).0)
            }
            UnpackTarget::Tuple { targets, position } => {
                let resolved_targets: Vec<UnpackTarget> = targets
                    .into_iter()
                    .map(|t| self.prepare_unpack_target(t)) // Recursive call
                    .collect();
                UnpackTarget::Tuple {
                    targets: resolved_targets,
                    position,
                }
            }
        }
    }

    /// Prepares an unpack target for comprehension by allocating fresh namespace slots.
    ///
    /// Unlike regular unpack targets, comprehension targets need new slots to shadow
    /// any existing bindings with the same name.
    fn prepare_unpack_target_for_comprehension(&mut self, target: UnpackTarget) -> UnpackTarget {
        match target {
            UnpackTarget::Name(ident) => {
                let name_str = self.interner.get_str(ident.name_id).to_string();
                let comp_var_id = NamespaceId::new(self.namespace_size);
                self.namespace_size += 1;

                // Shadow any existing binding
                self.shadow_for_comprehension(&name_str, comp_var_id);

                UnpackTarget::Name(Identifier::new_with_scope(
                    ident.name_id,
                    ident.position,
                    comp_var_id,
                    NameScope::Local,
                ))
            }
            UnpackTarget::Tuple { targets, position } => {
                let resolved_targets: Vec<UnpackTarget> = targets
                    .into_iter()
                    .map(|t| self.prepare_unpack_target_for_comprehension(t)) // Recursive call
                    .collect();
                UnpackTarget::Tuple {
                    targets: resolved_targets,
                    position,
                }
            }
        }
    }

    /// Pre-shadows an unpack target for comprehension scoping.
    ///
    /// Allocates namespace slots without marking as assigned, causing UnboundLocalError
    /// if accessed before assignment.
    fn prepare_unpack_target_shadow_only(&mut self, target: UnpackTarget) -> UnpackTarget {
        match target {
            UnpackTarget::Name(ident) => {
                let name_str = self.interner.get_str(ident.name_id).to_string();
                let comp_var_id = NamespaceId::new(self.namespace_size);
                self.namespace_size += 1;

                // Shadow but do NOT add to names_assigned_in_order yet
                self.name_map.insert(name_str.clone(), comp_var_id);
                self.free_var_map.remove(&name_str);
                self.cell_var_map.remove(&name_str);
                if let Some(ref mut enclosing) = self.enclosing_locals {
                    enclosing.remove(&name_str);
                }

                UnpackTarget::Name(Identifier::new_with_scope(
                    ident.name_id,
                    ident.position,
                    comp_var_id,
                    NameScope::Local,
                ))
            }
            UnpackTarget::Tuple { targets, position } => {
                let resolved_targets: Vec<UnpackTarget> = targets
                    .into_iter()
                    .map(|t| self.prepare_unpack_target_shadow_only(t)) // Recursive call
                    .collect();
                UnpackTarget::Tuple {
                    targets: resolved_targets,
                    position,
                }
            }
        }
    }

    /// Shadows a name in all scope maps for comprehension isolation.
    ///
    /// This ensures the comprehension loop variable takes precedence over any
    /// variable with the same name from enclosing scopes.
    fn shadow_for_comprehension(&mut self, name_str: &str, comp_var_id: NamespaceId) {
        // The lookup order in get_id is: global_declarations, free_var_map, cell_var_map,
        // assigned_names, enclosing_locals, then name_map. So we must update/remove from all maps
        // checked before name_map to ensure the comprehension variable shadows any captured
        // variable with the same name.
        self.name_map.insert(name_str.to_string(), comp_var_id);
        self.names_assigned_in_order.insert(name_str.to_string());
        self.free_var_map.remove(name_str);
        self.cell_var_map.remove(name_str);
        // Also remove from enclosing_locals to prevent get_id from re-capturing the variable
        if let Some(ref mut enclosing) = self.enclosing_locals {
            enclosing.remove(name_str);
        }
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
    ) -> Result<PreparedNode, ParseError> {
        // Register the function name in the current scope
        let (name, _) = self.get_id(name);

        // Extract param names from the parsed signature for scope analysis
        let param_names: Vec<StringId> = parsed_sig.param_names().collect();

        // Pass 1: Collect scope information from the function body
        let scope_info = collect_function_scope_info(&body, &param_names, self.interner);

        // Get the global name map to pass to the function preparer
        // At module level, use our own name_map; otherwise use the inherited global_name_map
        let global_name_map = if self.is_module_scope {
            self.name_map.clone()
        } else {
            self.global_name_map.clone().unwrap_or_default()
        };

        // Build enclosing_locals: names that are local to this scope (including params)
        // These are available for `nonlocal` declarations in nested functions
        let enclosing_locals: AHashSet<String> = if self.is_module_scope {
            // At module level, there are no enclosing locals for nonlocal
            // (module-level variables are accessed via `global`, not `nonlocal`)
            AHashSet::new()
        } else {
            // In a function: our params + assigned_names + existing name_map keys
            // are all potentially available as enclosing locals
            let mut locals = self.assigned_names.clone();
            for key in self.name_map.keys() {
                locals.insert(key.clone());
            }
            locals
        };

        // Filter potential_captures to get actual implicit captures.
        // Only names that are ALSO in enclosing_locals are true implicit captures.
        // Names NOT in enclosing_locals are either builtins or globals (handled at runtime).
        let implicit_captures: AHashSet<String> = scope_info
            .potential_captures
            .into_iter()
            .filter(|name| enclosing_locals.contains(name))
            .collect();

        // Pass 2: Create child preparer for function body with scope info
        let mut inner_prepare = Prepare::new_function(
            body.len(),
            &param_names,
            scope_info.assigned_names,
            scope_info.global_names,
            scope_info.nonlocal_names,
            implicit_captures,
            global_name_map,
            Some(enclosing_locals),
            scope_info.cell_var_names,
            self.interner,
        );

        // Prepare the function body
        let prepared_body = inner_prepare.prepare_nodes(body)?;

        // Mark variables that the inner function captures as our cell_vars
        // These are the names that appear in inner_prepare.free_var_map
        // Add to cell_var_map if not already present (may have been pre-populated or added earlier)
        for captured_name in inner_prepare.free_var_map.keys() {
            if !self.cell_var_map.contains_key(captured_name) && !self.free_var_map.contains_key(captured_name) {
                // Only add to cell_var_map if not already a free_var (pass-through case)
                // Allocate a namespace slot for the cell reference
                let slot = match self.name_map.entry(captured_name.clone()) {
                    Entry::Occupied(e) => *e.get(),
                    Entry::Vacant(e) => {
                        let slot = NamespaceId::new(self.namespace_size);
                        self.namespace_size += 1;
                        e.insert(slot);
                        slot
                    }
                };
                self.cell_var_map.insert(captured_name.clone(), slot);
            }
        }

        // Build free_var_enclosing_slots: enclosing namespace slots for captured variables
        // At call time, cells are pushed sequentially, so we only need the enclosing slots.
        // Sort by our slot index to ensure consistent ordering (matches namespace layout).
        let mut free_var_entries: Vec<_> = inner_prepare.free_var_map.into_iter().collect();
        free_var_entries.sort_by_key(|(_, our_slot)| *our_slot);

        let free_var_enclosing_slots: Vec<NamespaceId> = free_var_entries
            .into_iter()
            .map(|(var_name, _our_slot)| {
                // Determine the namespace slot in the enclosing scope where the cell reference lives:
                // - If it's in cell_var_map, it's a cell we own (allocated in this scope)
                // - If it's in free_var_map, it's a cell we captured from further up
                // - Otherwise, this is a prepare-time bug
                if let Some(&slot) = self.cell_var_map.get(&var_name) {
                    slot
                } else if let Some(&slot) = self.free_var_map.get(&var_name) {
                    slot
                } else {
                    panic!("free_var '{var_name}' not found in enclosing scope's cell_var_map or free_var_map");
                }
            })
            .collect();

        // cell_var_count: number of cells to create at call time for variables captured by nested functions
        // Slots are implicitly params.len()..params.len()+cell_var_count in the namespace layout
        let cell_var_count = inner_prepare.cell_var_map.len();
        let namespace_size = inner_prepare.namespace_size;

        // Build cell_param_indices: maps cell indices to parameter indices for captured parameters.
        // When a parameter is captured by a nested function, we need to copy its value into the cell.
        let cell_param_indices: Vec<Option<usize>> = if cell_var_count == 0 {
            Vec::new()
        } else {
            // Build a map from param name (String) to param index
            let param_name_to_index: AHashMap<String, usize> = param_names
                .iter()
                .enumerate()
                .map(|(idx, &name_id)| (self.interner.get_str(name_id).to_string(), idx))
                .collect();

            // Sort cell_var_map entries by slot to get cells in order
            let mut cell_entries: Vec<_> = inner_prepare.cell_var_map.iter().collect();
            cell_entries.sort_by_key(|&(_, slot)| slot);

            // For each cell (in slot order), check if it's a parameter
            cell_entries
                .into_iter()
                .map(|(name, _slot)| param_name_to_index.get(name).copied())
                .collect()
        };

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
            cell_var_count,
            cell_param_indices,
            default_exprs,
        }))
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
    /// # Returns
    /// A tuple of (resolved Identifier with id and scope set, whether this is a new local name).
    fn get_id(&mut self, ident: Identifier) -> (Identifier, bool) {
        let name_str = self.interner.get_str(ident.name_id);

        // At module level, all names are local (which is also the global namespace)
        if self.is_module_scope {
            return match self.name_map.entry(name_str.to_string()) {
                Entry::Occupied(e) => {
                    // Name already exists (from prior assignment or pre-registered)
                    (
                        Identifier::new_with_scope(ident.name_id, ident.position, *e.get(), NameScope::Local),
                        false,
                    )
                }
                Entry::Vacant(e) => {
                    let id = NamespaceId::new(self.namespace_size);
                    self.namespace_size += 1;
                    e.insert(id);
                    // Determine scope: if the name is assigned somewhere (even later in the file),
                    // it's a true local that will raise UnboundLocalError if accessed before assignment.
                    // If the name is never assigned, it's an undefined reference that raises NameError.
                    let scope = if self.names_assigned_in_order.contains(name_str) {
                        NameScope::Local
                    } else {
                        NameScope::LocalUnassigned
                    };
                    (
                        Identifier::new_with_scope(ident.name_id, ident.position, id, scope),
                        true,
                    )
                }
            };
        }

        // In a function: determine scope based on global_names, nonlocal_names, assigned_names, global_name_map

        // 1. Check if declared `global`
        if self.global_names.contains(name_str) {
            if let Some(ref global_map) = self.global_name_map
                && let Some(&global_id) = global_map.get(name_str)
            {
                // Name exists in global namespace
                return (
                    Identifier::new_with_scope(ident.name_id, ident.position, global_id, NameScope::Global),
                    false,
                );
            }
            // Declared global but doesn't exist yet - it will be created when assigned
            // For now, we still need a global index. We'll use a placeholder approach:
            // allocate in global namespace (this is a simplification - in real Python,
            // the global would be created at module level when first assigned)
            // For our implementation, we'll resolve to global but the variable won't exist until assigned.
            // Return a "new" global - but we can't modify global_name_map here.
            // For simplicity, we'll resolve to local with Global scope - runtime will handle the lookup.
            let (id, is_new) = match self.name_map.entry(name_str.to_string()) {
                Entry::Occupied(e) => (*e.get(), false),
                Entry::Vacant(e) => {
                    let id = NamespaceId::new(self.namespace_size);
                    self.namespace_size += 1;
                    e.insert(id);
                    (id, true)
                }
            };
            // Mark as Global scope - runtime will need to handle this specially
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, id, NameScope::Global),
                is_new,
            );
        }

        // 2. Check if captured from enclosing scope (nonlocal declaration or implicit capture)
        // free_var_map stores namespace slot indices where the cell reference will be stored
        if let Some(&slot) = self.free_var_map.get(name_str) {
            // At runtime, the cell reference is in namespace[slot] as Value::Ref(cell_id)
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, slot, NameScope::Cell),
                false, // Not a new local - it's captured from enclosing scope
            );
        }

        // 3. Check if this is a cell variable (captured by nested functions)
        // cell_var_map stores namespace slot indices where the cell reference will be stored
        // At call time, a cell is created and stored as Value::Ref(cell_id) at this slot
        if let Some(&slot) = self.cell_var_map.get(name_str) {
            // The namespace slot was already allocated when cell_var_map was populated
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, slot, NameScope::Cell),
                false, // Not a "new" local - it's a cell variable
            );
        }

        // 4. Check if assigned in this function (local variable)
        if self.assigned_names.contains(name_str) {
            let (id, is_new) = match self.name_map.entry(name_str.to_string()) {
                Entry::Occupied(e) => (*e.get(), false),
                Entry::Vacant(e) => {
                    let id = NamespaceId::new(self.namespace_size);
                    self.namespace_size += 1;
                    e.insert(id);
                    (id, true)
                }
            };
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, id, NameScope::Local),
                is_new,
            );
        }

        // 5. Check if exists in enclosing scope (implicit closure capture)
        // This handles reading variables from enclosing functions without explicit `nonlocal`
        if let Some(ref enclosing) = self.enclosing_locals
            && enclosing.contains(name_str)
        {
            // This is an implicit capture - add to free_var_map with a namespace slot
            let slot = if let Some(&existing_slot) = self.free_var_map.get(name_str) {
                existing_slot
            } else {
                // Allocate a namespace slot for this free variable
                let slot = NamespaceId::new(self.namespace_size);
                self.namespace_size += 1;
                self.name_map.insert(name_str.to_string(), slot);
                self.free_var_map.insert(name_str.to_string(), slot);
                slot
            };
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, slot, NameScope::Cell),
                false, // Not a new local - it's captured from enclosing scope
            );
        }

        // 6. Check if name was pre-populated in name_map (from function parameters)
        // This ensures parameters shadow global variables with the same name.
        // Parameters are added to name_map during FunctionScope::new_function() but are NOT
        // in assigned_names (since they're not assigned in the function body).
        if let Some(&id) = self.name_map.get(name_str) {
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, id, NameScope::Local),
                false, // Not new - was pre-populated from parameters
            );
        }

        // 7. Check if exists in global namespace (implicit global read)
        if let Some(ref global_map) = self.global_name_map
            && let Some(&global_id) = global_map.get(name_str)
        {
            return (
                Identifier::new_with_scope(ident.name_id, ident.position, global_id, NameScope::Global),
                false,
            );
        }

        // 8. Name not found anywhere - allocate a local slot (will be NameError at runtime)
        // This handles names that are only read (never assigned) and don't exist globally.
        // We allocate a local slot that will never be written to.
        // Mark as LocalUnassigned so runtime raises NameError (not UnboundLocalError).
        let (id, is_new) = match self.name_map.entry(name_str.to_string()) {
            Entry::Occupied(e) => (*e.get(), false),
            Entry::Vacant(e) => {
                let id = NamespaceId::new(self.namespace_size);
                self.namespace_size += 1;
                e.insert(id);
                (id, true)
            }
        };
        (
            Identifier::new_with_scope(ident.name_id, ident.position, id, NameScope::LocalUnassigned),
            is_new,
        )
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
        Node::Assign { target, .. } => {
            assigned_names.insert(interner.get_str(target.name_id).to_string());
        }
        Node::UnpackAssign { targets, .. } => {
            // Recursively collect all names from nested unpack targets
            for target in targets {
                collect_names_from_unpack_target(target, assigned_names, interner);
            }
        }
        Node::OpAssign { target, .. } => {
            assigned_names.insert(interner.get_str(target.name_id).to_string());
        }
        Node::SubscriptAssign { .. } => {
            // Subscript assignment doesn't create a new name, it modifies existing container
        }
        Node::AttrAssign { .. } => {
            // Attribute assignment doesn't create a new name, it modifies existing object
        }
        Node::For {
            target, body, or_else, ..
        } => {
            // For loop target is assigned - collect all names from the target
            collect_names_from_unpack_target(target, assigned_names, interner);
            // Recurse into body and else
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        Node::If { body, or_else, .. } => {
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
        // Import creates a binding for the module name (or alias)
        Node::Import { binding, .. } => {
            assigned_names.insert(interner.get_str(binding.name_id).to_string());
        }
        // ImportFrom creates bindings for each imported name (or alias)
        Node::ImportFrom { names, .. } => {
            for (_import_name, binding) in names {
                assigned_names.insert(interner.get_str(binding.name_id).to_string());
            }
        }
        // These don't create new names
        Node::Pass | Node::Expr(_) | Node::Return(_) | Node::ReturnNone | Node::Raise(_) | Node::Assert { .. } => {}
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
            // Find what names are referenced inside this nested function
            let mut referenced = AHashSet::new();
            for n in body {
                collect_referenced_names_from_node(n, &mut referenced, interner);
            }

            // Extract param names from signature for scope analysis
            let param_names: Vec<StringId> = signature.param_names().collect();

            // Collect the nested function's own locals (params + assigned)
            let nested_scope = collect_function_scope_info(body, &param_names, interner);

            // Any name that is:
            // - Referenced by the nested function
            // - Not a local of the nested function
            // - Not declared global in the nested function
            // - In our locals
            // becomes a cell_var
            for name in &referenced {
                if !nested_scope.assigned_names.contains(name)
                    && !param_names.iter().any(|p| interner.get_str(*p) == name)
                    && !nested_scope.global_names.contains(name)
                    && our_locals.contains(name)
                {
                    cell_vars.insert(name.clone());
                }
            }

            // Also check what the nested function explicitly declares as nonlocal
            for name in &nested_scope.nonlocal_names {
                if our_locals.contains(name) {
                    cell_vars.insert(name.clone());
                }
            }
        }
        // Recurse into control flow structures
        Node::For { body, or_else, .. } => {
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        Node::If { body, or_else, .. } => {
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
        // Other nodes don't contain nested function definitions
        _ => {}
    }
}

/// Collects all names referenced (read) in a node and its descendants.
///
/// This is used to find what names a nested function references from enclosing scopes.
fn collect_referenced_names_from_node(node: &ParseNode, referenced: &mut AHashSet<String>, interner: &InternerBuilder) {
    match node {
        Node::Expr(expr) => collect_referenced_names_from_expr(expr, referenced, interner),
        Node::Return(expr) => collect_referenced_names_from_expr(expr, referenced, interner),
        Node::Raise(Some(expr)) => collect_referenced_names_from_expr(expr, referenced, interner),
        Node::Raise(None) => {}
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
        Node::OpAssign { target, object, .. } => {
            // OpAssign reads the target before writing
            referenced.insert(interner.get_str(target.name_id).to_string());
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        Node::SubscriptAssign {
            target, index, value, ..
        } => {
            referenced.insert(interner.get_str(target.name_id).to_string());
            collect_referenced_names_from_expr(index, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        Node::AttrAssign { object, value, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
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
        // Imports create bindings but don't reference names
        Node::Import { .. } | Node::ImportFrom { .. } => {}
        Node::Pass | Node::ReturnNone | Node::Global { .. } | Node::Nonlocal { .. } => {}
    }
}

/// Collects all names referenced in an expression.
fn collect_referenced_names_from_expr(
    expr: &crate::expressions::ExprLoc,
    referenced: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    use crate::expressions::Expr;
    match &expr.expr {
        Expr::Name(ident) => {
            referenced.insert(interner.get_str(ident.name_id).to_string());
        }
        Expr::Literal(_) => {}
        Expr::Builtin(_) => {}
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for item in items {
                collect_referenced_names_from_expr(item, referenced, interner);
            }
        }
        Expr::Dict(pairs) => {
            for (key, value) in pairs {
                collect_referenced_names_from_expr(key, referenced, interner);
                collect_referenced_names_from_expr(value, referenced, interner);
            }
        }
        Expr::Op { left, right, .. } | Expr::CmpOp { left, right, .. } => {
            collect_referenced_names_from_expr(left, referenced, interner);
            collect_referenced_names_from_expr(right, referenced, interner);
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
fn collect_referenced_names_from_args(
    args: &crate::args::ArgExprs,
    referenced: &mut AHashSet<String>,
    interner: &InternerBuilder,
) {
    use crate::args::ArgExprs;
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
        ArgExprs::Kwargs(_) | ArgExprs::ArgsKargs { .. } => {
            // TODO: handle kwargs when needed
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
        UnpackTarget::Name(ident) => {
            names.insert(interner.get_str(ident.name_id).to_string());
        }
        UnpackTarget::Tuple { targets, .. } => {
            for t in targets {
                collect_names_from_unpack_target(t, names, interner);
            }
        }
    }
}
