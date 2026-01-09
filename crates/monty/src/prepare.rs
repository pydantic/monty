use std::collections::hash_map::Entry;

use ahash::{AHashMap, AHashSet};

use crate::{
    args::ArgExprs,
    callable::Callable,
    expressions::{Expr, ExprLoc, Identifier, Literal, NameScope, Node},
    fstring::{FStringPart, FormatSpec},
    function::Function,
    intern::{FunctionId, InternerBuilder, StringId},
    namespace::NamespaceId,
    operators::{CmpOperator, Operator},
    parse::{ExceptHandler, ParseError, ParseNode, ParseResult, ParsedSignature, Try},
    signature::Signature,
};

/// Result of the prepare phase, containing everything needed to execute code.
///
/// This struct holds the outputs of name resolution and AST transformation:
/// - The initial namespace with placeholder values for each variable
/// - A mapping from variable names to their namespace indices (for ref-count testing)
/// - The transformed AST nodes ready for execution
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
    pub nodes: Vec<Node>,
    /// The string interner containing all interned identifiers and filenames.
    pub interner: InternerBuilder,
    /// The functions defined in the module.
    pub functions: Vec<Function>,
}

/// Prepares parsed nodes for execution by resolving names and building the initial namespace.
///
/// The namespace will be converted to runtime Objects when execution begins and the heap is available.
/// At module level, the local namespace IS the global namespace.
pub(crate) fn prepare(
    parse_result: ParseResult,
    input_names: Vec<String>,
    external_functions: &[String],
) -> Result<PrepareResult, ParseError> {
    let ParseResult { nodes, interner } = parse_result;
    let mut functions = Vec::new();
    let mut p = Prepare::new_module(input_names, external_functions, &interner, &mut functions);
    let mut prepared_nodes = p.prepare_nodes(nodes)?;

    // In the root frame,the last expression is implicitly returned
    // if it's not None. This matches Python REPL behavior where the last expression
    // value is displayed/returned.
    if let Some(Node::Expr(expr_loc)) = prepared_nodes.last() {
        if !expr_loc.expr.is_none() {
            let new_expr_loc = expr_loc.clone();
            prepared_nodes.pop();
            prepared_nodes.push(Node::Return(new_expr_loc));
        }
    }

    Ok(PrepareResult {
        namespace_size: p.namespace_size,
        #[cfg(feature = "ref-count-return")]
        name_map: p.name_map,
        nodes: prepared_nodes,
        interner,
        functions,
    })
}

/// State machine for the preparation phase that transforms parsed AST nodes into an executable form.
///
/// This struct maintains the mapping between variable names and their namespace indices,
/// builds the initial namespace with Literals (pre-runtime), and handles scope resolution.
/// The preparation phase is crucial for converting string-based name lookups into efficient
/// integer-indexed namespace access during execution.
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
    /// container for functions
    functions: &'i mut Vec<Function>,
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
    /// * `capacity` - Expected number of nodes, used to preallocate the name map
    /// * `input_names` - Names that should be pre-registered in the namespace (e.g., external variables)
    /// * `interner` - Reference to the string interner for looking up names
    /// * `functions` - Reference to the functions container
    fn new_module(
        input_names: Vec<String>,
        external_functions: &[String],
        interner: &'i InternerBuilder,
        functions: &'i mut Vec<Function>,
    ) -> Self {
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
            functions,
        }
    }

    /// Creates a new Prepare instance for function-level code.
    ///
    /// Pre-populates `free_var_map` with nonlocal declarations and `cell_var_map` with
    /// cell variables (excluding pass-through variables that are both nonlocal and cell).
    ///
    /// # Arguments
    /// * `capacity` - Expected number of nodes
    /// * `params` - Function parameter StringIds (pre-registered in namespace)
    /// * `assigned_names` - Names that are assigned in this function (from first-pass scan)
    /// * `global_names` - Names declared as `global` in this function
    /// * `nonlocal_names` - Names declared as `nonlocal` in this function
    /// * `global_name_map` - Copy of the module-level name map for global resolution
    /// * `enclosing_locals` - Names that exist as locals in the enclosing function (for nonlocal resolution)
    /// * `cell_var_names` - Names that are captured by nested functions (must be stored in cells)
    /// * `interner` - Reference to the string interner for looking up names
    #[allow(clippy::too_many_arguments)]
    fn new_function(
        capacity: usize,
        params: &[StringId],
        assigned_names: AHashSet<String>,
        global_names: AHashSet<String>,
        nonlocal_names: AHashSet<String>,
        global_name_map: AHashMap<String, NamespaceId>,
        enclosing_locals: Option<AHashSet<String>>,
        cell_var_names: AHashSet<String>,
        interner: &'i InternerBuilder,
        functions: &'i mut Vec<Function>,
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
            if !nonlocal_names.contains(&name) {
                let slot = namespace_size;
                namespace_size += 1;
                cell_var_map.insert(name, NamespaceId::new(slot));
            }
        }

        // Pre-populate free_var_map with nonlocal declarations SECOND (after cell_vars).
        // Each entry maps name -> namespace slot index where the cell reference will be stored.
        // NOTE: We intentionally do NOT add these to name_map here, because the nonlocal
        // validation in prepare_nodes checks name_map to detect "used before nonlocal declaration"
        let mut free_var_map = AHashMap::with_capacity(nonlocal_names.len());
        for name in nonlocal_names {
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
            functions,
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
    /// A vector of prepared nodes ready for execution
    fn prepare_nodes(&mut self, nodes: Vec<ParseNode>) -> Result<Vec<Node>, ParseError> {
        let nodes_len = nodes.len();
        let mut new_nodes = Vec::with_capacity(nodes_len);
        for node in nodes {
            match node {
                ParseNode::Pass => (),
                ParseNode::Expr(expr) => new_nodes.push(Node::Expr(self.prepare_expression(expr)?)),
                ParseNode::Return(expr) => new_nodes.push(Node::Return(self.prepare_expression(expr)?)),
                ParseNode::ReturnNone => new_nodes.push(Node::ReturnNone),
                ParseNode::Raise(exc) => {
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
                                    // Don't error here if undefined - let runtime raise NameError with proper traceback.
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
                ParseNode::Assert { test, msg } => {
                    let test = self.prepare_expression(test)?;
                    let msg = match msg {
                        Some(m) => Some(self.prepare_expression(m)?),
                        None => None,
                    };
                    new_nodes.push(Node::Assert { test, msg });
                }
                ParseNode::Assign { target, object } => {
                    let object = self.prepare_expression(object)?;
                    // Track that this name was assigned before we call get_id
                    self.names_assigned_in_order
                        .insert(self.interner.get_str(target.name_id).to_string());
                    let (target, _) = self.get_id(target);
                    new_nodes.push(Node::Assign { target, object });
                }
                ParseNode::OpAssign { target, op, object } => {
                    // Track that this name was assigned
                    self.names_assigned_in_order
                        .insert(self.interner.get_str(target.name_id).to_string());
                    let target = self.get_id(target).0;
                    let object = self.prepare_expression(object)?;
                    new_nodes.push(Node::OpAssign { target, op, object });
                }
                ParseNode::SubscriptAssign { target, index, value } => {
                    // SubscriptAssign doesn't assign to the target itself, just modifies it
                    let target = self.get_id(target).0;
                    let index = self.prepare_expression(index)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::SubscriptAssign { target, index, value });
                }
                ParseNode::AttrAssign {
                    object,
                    attr,
                    target_position,
                    value,
                } => {
                    // AttrAssign doesn't assign to the object itself, just modifies its attribute
                    let object = Box::new(self.prepare_expression(*object)?);
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::AttrAssign {
                        object,
                        attr,
                        target_position,
                        value,
                    });
                }
                ParseNode::For {
                    target,
                    iter,
                    body,
                    or_else,
                } => {
                    // Track that the loop variable is assigned
                    self.names_assigned_in_order
                        .insert(self.interner.get_str(target.name_id).to_string());
                    new_nodes.push(Node::For {
                        target: self.get_id(target).0,
                        iter: self.prepare_expression(iter)?,
                        body: self.prepare_nodes(body)?,
                        or_else: self.prepare_nodes(or_else)?,
                    });
                }
                ParseNode::If { test, body, or_else } => {
                    let test = self.prepare_expression(test)?;
                    let body = self.prepare_nodes(body)?;
                    let or_else = self.prepare_nodes(or_else)?;
                    new_nodes.push(Node::If { test, body, or_else });
                }
                ParseNode::FunctionDef { name, signature, body } => {
                    let func_node = self.prepare_function_def(name, signature, body)?;
                    new_nodes.push(func_node);
                }
                ParseNode::Global { names, position } => {
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
                ParseNode::Nonlocal { names, position } => {
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
                ParseNode::Try(Try {
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
            }
        }
        Ok(new_nodes)
    }

    /// Prepares an exception handler by resolving names in the exception type and body.
    ///
    /// The exception variable (if present) is treated as an assigned name in the current scope.
    fn prepare_except_handler(&mut self, handler: ExceptHandler<ParseNode>) -> Result<ExceptHandler<Node>, ParseError> {
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
        };

        // Optimization: Transform `(x % n) == value` with any constant right-hand side into a
        // specialized ModEq operator.
        // This is a common pattern in competitive programming (e.g., FizzBuzz checks like `i % 3 == 0`)
        // and can be executed more efficiently with a single modulo operation + comparison
        // instead of separate modulo, then equality check.
        if let Expr::CmpOp { left, op, right } = &expr {
            if op == &CmpOperator::Eq {
                if let Expr::Literal(Literal::Int(value)) = right.expr {
                    if let Expr::Op {
                        left: left2,
                        op,
                        right: right2,
                    } = &left.expr
                    {
                        if op == &Operator::Mod {
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
                    }
                }
            }
        }

        Ok(ExprLoc { position, expr })
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
        parsed_sig: ParsedSignature,
        body: Vec<ParseNode>,
    ) -> Result<Node, ParseError> {
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

        // Pass 2: Create child preparer for function body with scope info
        let mut inner_prepare = Prepare::new_function(
            body.len(),
            &param_names,
            scope_info.assigned_names,
            scope_info.global_names,
            scope_info.nonlocal_names,
            global_name_map,
            Some(enclosing_locals),
            scope_info.cell_var_names,
            self.interner,
            self.functions,
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
            cell_entries.sort_by_key(|(_, &slot)| slot);

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

        let function_id = FunctionId::new(self.functions.len());
        self.functions.push(Function::new(
            name,
            signature,
            prepared_body,
            namespace_size,
            free_var_enclosing_slots,
            cell_var_count,
            cell_param_indices,
            default_exprs,
        ));

        // Return the final FunctionDef node
        Ok(Node::FunctionDef(function_id))
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

        // In a function: determine scope based on global_names, nonlocal_names, assigned_names, global_name_map

        // 1. Check if declared `global`
        if self.global_names.contains(name_str) {
            if let Some(ref global_map) = self.global_name_map {
                if let Some(&global_id) = global_map.get(name_str) {
                    // Name exists in global namespace
                    return (
                        Identifier::new_with_scope(ident.name_id, ident.position, global_id, NameScope::Global),
                        false,
                    );
                }
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
        if let Some(ref enclosing) = self.enclosing_locals {
            if enclosing.contains(name_str) {
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
        if let Some(ref global_map) = self.global_name_map {
            if let Some(&global_id) = global_map.get(name_str) {
                return (
                    Identifier::new_with_scope(ident.name_id, ident.position, global_id, NameScope::Global),
                    false,
                );
            }
        }

        // 8. Name not found anywhere - resolve to local (will be NameError at runtime)
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
            Identifier::new_with_scope(ident.name_id, ident.position, id, NameScope::Local),
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
#[allow(clippy::struct_field_names)] // Field names are descriptive and consistent with Python terminology
struct FunctionScopeInfo {
    /// Names declared as `global`
    global_names: AHashSet<String>,
    /// Names declared as `nonlocal`
    nonlocal_names: AHashSet<String>,
    /// Names that are assigned in this scope
    assigned_names: AHashSet<String>,
    /// Names that are captured by nested functions (must be stored in cells)
    cell_var_names: AHashSet<String>,
}

/// Scans a function body to collect scope information (first pass of two-pass preparation).
///
/// This function recursively walks the AST to find:
/// - Names declared as `global` (from Global statements)
/// - Names declared as `nonlocal` (from Nonlocal statements)
/// - Names that are assigned (from Assign, OpAssign, For targets, etc.)
/// - Names that are captured by nested functions (cell_var_names)
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
    let our_locals: AHashSet<String> = params
        .iter()
        .map(|string_id| interner.get_str(*string_id).to_string())
        .chain(assigned_names.iter().cloned())
        .filter(|name| !global_names.contains(name))
        .collect();

    // Second pass: find what nested functions capture from us
    for node in nodes {
        collect_cell_vars_from_node(node, &our_locals, &mut cell_var_names, interner);
    }

    FunctionScopeInfo {
        global_names,
        nonlocal_names,
        assigned_names,
        cell_var_names,
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
        ParseNode::Global { names, .. } => {
            for string_id in names {
                global_names.insert(interner.get_str(*string_id).to_string());
            }
        }
        ParseNode::Nonlocal { names, .. } => {
            for string_id in names {
                nonlocal_names.insert(interner.get_str(*string_id).to_string());
            }
        }
        ParseNode::Assign { target, .. } => {
            assigned_names.insert(interner.get_str(target.name_id).to_string());
        }
        ParseNode::OpAssign { target, .. } => {
            assigned_names.insert(interner.get_str(target.name_id).to_string());
        }
        ParseNode::SubscriptAssign { .. } => {
            // Subscript assignment doesn't create a new name, it modifies existing container
        }
        ParseNode::AttrAssign { .. } => {
            // Attribute assignment doesn't create a new name, it modifies existing object
        }
        ParseNode::For {
            target, body, or_else, ..
        } => {
            // For loop target is assigned
            assigned_names.insert(interner.get_str(target.name_id).to_string());
            // Recurse into body and else
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        ParseNode::If { body, or_else, .. } => {
            // Recurse into branches
            for n in body {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, nonlocal_names, assigned_names, interner);
            }
        }
        ParseNode::FunctionDef { name, .. } => {
            // Function definition creates a local binding for the function name
            // But we don't recurse into the function body - that's a separate scope
            assigned_names.insert(interner.get_str(name.name_id).to_string());
        }
        ParseNode::Try(Try {
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
        // These don't create new names
        ParseNode::Pass
        | ParseNode::Expr(_)
        | ParseNode::Return(_)
        | ParseNode::ReturnNone
        | ParseNode::Raise(_)
        | ParseNode::Assert { .. } => {}
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
        ParseNode::FunctionDef { signature, body, .. } => {
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
        ParseNode::For { body, or_else, .. } => {
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        ParseNode::If { body, or_else, .. } => {
            for n in body {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
            for n in or_else {
                collect_cell_vars_from_node(n, our_locals, cell_vars, interner);
            }
        }
        ParseNode::Try(Try {
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
        ParseNode::Expr(expr) => collect_referenced_names_from_expr(expr, referenced, interner),
        ParseNode::Return(expr) => collect_referenced_names_from_expr(expr, referenced, interner),
        ParseNode::Raise(Some(expr)) => collect_referenced_names_from_expr(expr, referenced, interner),
        ParseNode::Raise(None) => {}
        ParseNode::Assert { test, msg } => {
            collect_referenced_names_from_expr(test, referenced, interner);
            if let Some(m) = msg {
                collect_referenced_names_from_expr(m, referenced, interner);
            }
        }
        ParseNode::Assign { object, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        ParseNode::OpAssign { target, object, .. } => {
            // OpAssign reads the target before writing
            referenced.insert(interner.get_str(target.name_id).to_string());
            collect_referenced_names_from_expr(object, referenced, interner);
        }
        ParseNode::SubscriptAssign { target, index, value } => {
            referenced.insert(interner.get_str(target.name_id).to_string());
            collect_referenced_names_from_expr(index, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        ParseNode::AttrAssign { object, value, .. } => {
            collect_referenced_names_from_expr(object, referenced, interner);
            collect_referenced_names_from_expr(value, referenced, interner);
        }
        ParseNode::For {
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
        ParseNode::If { test, body, or_else } => {
            collect_referenced_names_from_expr(test, referenced, interner);
            for n in body {
                collect_referenced_names_from_node(n, referenced, interner);
            }
            for n in or_else {
                collect_referenced_names_from_node(n, referenced, interner);
            }
        }
        ParseNode::FunctionDef { .. } => {
            // Don't recurse into nested function bodies - they have their own scope
        }
        ParseNode::Try(Try {
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
        ParseNode::Pass | ParseNode::ReturnNone | ParseNode::Global { .. } | ParseNode::Nonlocal { .. } => {}
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
        Expr::Not(operand) | Expr::UnaryMinus(operand) => {
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
            if let crate::callable::Callable::Name(ident) = callable {
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
