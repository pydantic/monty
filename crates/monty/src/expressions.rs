use crate::{
    args::ArgExprs,
    builtins::Builtins,
    fstring::FStringPart,
    intern::{BytesId, StringId},
    namespace::NamespaceId,
    parse::{CodeRange, Try},
    signature::Signature,
    value::{Attr, Marker, Value},
};

/// Indicates which namespace a variable reference belongs to.
///
/// This is determined at prepare time based on Python's scoping rules:
/// - Variables assigned in a function are Local (unless declared `global`)
/// - Variables only read (not assigned) that exist at module level are Global
/// - The `global` keyword explicitly marks a variable as Global
/// - Variables declared `nonlocal` or implicitly captured from enclosing scopes
///   are accessed through Cells
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum NameScope {
    /// Variable is in the current frame's local namespace (assigned somewhere in this function).
    ///
    /// If accessed before assignment, raises `UnboundLocalError`.
    #[default]
    Local,
    /// Variable reference that doesn't exist in any scope.
    ///
    /// A local slot is allocated but never assigned. Accessing raises `NameError`
    /// (not `UnboundLocalError`) because the name was never defined anywhere.
    LocalUnassigned,
    /// Variable is in the module-level global namespace
    Global,
    /// Variable accessed through a cell (heap-allocated container).
    ///
    /// Used for both:
    /// - Variables captured from enclosing scopes (free variables)
    /// - Variables in this function that are captured by nested functions (cell variables)
    ///
    /// The namespace slot contains `Value::Ref(cell_id)` pointing to a `HeapData::Cell`.
    /// Access requires dereferencing through the cell.
    Cell,
}

/// An identifier (variable or function name) with source location and scope information.
///
/// The name is stored as a `StringId` which indexes into the string interner.
/// To get the actual string, look it up in the `Interns` storage.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Identifier {
    pub position: CodeRange,
    /// Interned name ID - look up in Interns to get the actual string.
    pub name_id: StringId,
    opt_namespace_id: Option<NamespaceId>,
    /// Which namespace this identifier refers to (determined at prepare time)
    pub scope: NameScope,
}

impl Identifier {
    /// Creates a new identifier with unknown scope (to be resolved during prepare phase).
    pub fn new(name_id: StringId, position: CodeRange) -> Self {
        Self {
            name_id,
            position,
            opt_namespace_id: None,
            scope: NameScope::Local,
        }
    }

    /// Creates a new identifier with resolved namespace index and explicit scope.
    pub fn new_with_scope(name_id: StringId, position: CodeRange, namespace_id: NamespaceId, scope: NameScope) -> Self {
        Self {
            name_id,
            position,
            opt_namespace_id: Some(namespace_id),
            scope,
        }
    }

    pub fn namespace_id(&self) -> NamespaceId {
        self.opt_namespace_id
            .expect("Identifier not prepared with namespace_id")
    }
}

/// Target of a function call expression.
///
/// Represents a callable that can be either:
/// - A builtin function or exception resolved at parse time (`print`, `len`, `ValueError`, etc.)
/// - A name that will be looked up in the namespace at runtime (for callable variables)
///
/// Separate from Value to allow deriving Clone without Value's Clone restrictions.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Callable {
    /// A builtin function like `print`, `len`, `str`, etc.
    Builtin(Builtins),
    /// A name to be looked up in the namespace at runtime (e.g., `x` in `x = len; x('abc')`).
    Name(Identifier),
}

/// An expression in the AST.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Expr {
    Literal(Literal),
    Builtin(Builtins),
    Name(Identifier),
    /// Function call expression.
    ///
    /// The `callable` can be a Builtin, ExcType (resolved at parse time), or a Name
    /// that will be looked up in the namespace at runtime.
    Call {
        callable: Callable,
        /// ArgExprs is relatively large and would require Box anyway since it uses ExprLoc, so keep Expr small
        /// by using a box here
        args: Box<ArgExprs>,
    },
    /// Method call on an object (e.g., `obj.method(args)`).
    ///
    /// The object expression is evaluated first, then the method is looked up
    /// and called with the given arguments. Supports chained attribute access
    /// like `a.b.c.method()`.
    AttrCall {
        object: Box<ExprLoc>,
        attr: Attr,
        /// same as above for Box
        args: Box<ArgExprs>,
    },
    /// Attribute access expression (e.g., `point.x` or `a.b.c`).
    ///
    /// Retrieves the value of an attribute from an object. For dataclasses,
    /// this returns the field value. For other types, this may trigger
    /// special attribute handling. Supports chained attribute access.
    AttrGet {
        object: Box<ExprLoc>,
        attr: Attr,
    },
    Op {
        left: Box<ExprLoc>,
        op: Operator,
        right: Box<ExprLoc>,
    },
    CmpOp {
        left: Box<ExprLoc>,
        op: CmpOperator,
        right: Box<ExprLoc>,
    },
    List(Vec<ExprLoc>),
    Tuple(Vec<ExprLoc>),
    Subscript {
        object: Box<ExprLoc>,
        index: Box<ExprLoc>,
    },
    Dict(Vec<(ExprLoc, ExprLoc)>),
    /// Set literal expression: `{1, 2, 3}`.
    ///
    /// Note: `{}` is always a dict, not an empty set. Use `set()` for empty sets.
    Set(Vec<ExprLoc>),
    /// Unary `not` expression - evaluates to the boolean negation of the operand's truthiness.
    Not(Box<ExprLoc>),
    /// Unary minus expression - negates a numeric value.
    UnaryMinus(Box<ExprLoc>),
    /// Unary plus expression - returns value as-is for numbers, converts bools to int.
    UnaryPlus(Box<ExprLoc>),
    /// Unary bitwise NOT expression - inverts all bits of an integer.
    UnaryInvert(Box<ExprLoc>),
    /// F-string expression containing literal and interpolated parts.
    ///
    /// At evaluation time, each part is processed in sequence:
    /// - Literal parts are used directly
    /// - Interpolation parts have their expression evaluated, converted, and formatted
    ///
    /// The results are concatenated to produce the final string.
    FString(Vec<FStringPart>),
    /// Conditional expression (ternary operator): `body if test else orelse`
    ///
    /// Only one of body/orelse is evaluated based on the truthiness of test.
    /// This implements short-circuit evaluation - the branch not taken is never executed.
    IfElse {
        test: Box<ExprLoc>,
        body: Box<ExprLoc>,
        orelse: Box<ExprLoc>,
    },
    /// List comprehension: `[elt for target in iter if cond...]`
    ///
    /// Builds a new list by iterating and optionally filtering. Loop variables
    /// are scoped to the comprehension and do not leak to the enclosing scope.
    ListComp {
        elt: Box<ExprLoc>,
        generators: Vec<Comprehension>,
    },
    /// Set comprehension: `{elt for target in iter if cond...}`
    ///
    /// Builds a new set by iterating and optionally filtering. Duplicate values
    /// are deduplicated. Loop variables are scoped to the comprehension.
    SetComp {
        elt: Box<ExprLoc>,
        generators: Vec<Comprehension>,
    },
    /// Dict comprehension: `{key: value for target in iter if cond...}`
    ///
    /// Builds a new dict by iterating and optionally filtering. Later values
    /// overwrite earlier ones for duplicate keys. Loop variables are scoped
    /// to the comprehension.
    DictComp {
        key: Box<ExprLoc>,
        value: Box<ExprLoc>,
        generators: Vec<Comprehension>,
    },
}

/// Target for tuple unpacking - can be a single name or nested tuple.
///
/// Supports recursive structures like `(a, b), c` or `a, (b, c)`.
/// Used in assignment statements, for loop targets, and comprehension targets.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum UnpackTarget {
    /// Single identifier: `a`
    Name(Identifier),
    /// Nested tuple: `(a, b)` - can contain further nested tuples
    Tuple {
        /// The targets to unpack into (can be names or nested tuples)
        targets: Vec<Self>,
        /// Source position covering all targets (for error caret placement)
        position: CodeRange,
    },
}

/// A generator clause in a comprehension: `for target in iter [if cond1] [if cond2]...`
///
/// Represents one `for` clause with zero or more `if` filters. Multiple generators
/// create nested iteration (the rightmost varies fastest).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Comprehension {
    /// Loop variable - either single identifier or tuple unpacking pattern.
    pub target: UnpackTarget,
    /// Iterable expression to loop over.
    pub iter: ExprLoc,
    /// Zero or more filter conditions (all must be truthy for the element to be included).
    pub ifs: Vec<ExprLoc>,
}

impl Expr {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::Literal(Literal::None))
    }
}

/// Represents values that can be produced purely from the parser/prepare pipeline.
///
/// Const values are intentionally detached from the runtime heap so we can keep
/// parse-time transformations (constant folding, namespace seeding, etc.) free from
/// reference-count semantics. Only once execution begins are these literals turned
/// into real `Value`s that participate in the interpreter's runtime rules.
///
/// Note: unlike the AST `Constant` type, we store tuples only as expressions since they
/// can't always be recorded as constants.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Literal {
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// An interned string literal. The StringId references the string in the Interns table.
    Str(StringId),
    /// An interned bytes literal. The BytesId references the bytes in the Interns table.
    Bytes(BytesId),
    /// A marker value (e.g., typing constructs like Any, Optional, etc.).
    Marker(Marker),
}

impl From<Literal> for Value {
    /// Converts the literal into its runtime `Value` counterpart.
    ///
    /// This is the only place parse-time data crosses the boundary into runtime
    /// semantics, ensuring every literal follows the same conversion path.
    fn from(literal: Literal) -> Self {
        match literal {
            Literal::Ellipsis => Self::Ellipsis,
            Literal::None => Self::None,
            Literal::Bool(b) => Self::Bool(b),
            Literal::Int(v) => Self::Int(v),
            Literal::Float(v) => Self::Float(v),
            Literal::Str(string_id) => Self::InternString(string_id),
            Literal::Bytes(bytes_id) => Self::InternBytes(bytes_id),
            Literal::Marker(marker) => Self::Marker(marker),
        }
    }
}

/// An expression with its source location.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExprLoc {
    pub position: CodeRange,
    pub expr: Expr,
}

impl ExprLoc {
    pub fn new(position: CodeRange, expr: Expr) -> Self {
        Self { position, expr }
    }
}

/// An AST node parameterized by the function definition type.
///
/// This generic enum represents statements in both parsed and prepared forms:
/// - `Node<RawFunctionDef>` (aka `ParseNode`): Output of the parser, contains unprepared function bodies
/// - `Node<PreparedFunctionDef>` (aka `PreparedNode`): Output of prepare phase, has resolved names
///
/// Some variants (`Pass`, `Global`, `Nonlocal`) only appear in parsed form and are filtered
/// out during the prepare phase.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Node<F> {
    /// No-op statement. Only present in parsed form, filtered out during prepare.
    Pass,
    Expr(ExprLoc),
    Return(ExprLoc),
    ReturnNone,
    Raise(Option<ExprLoc>),
    Assert {
        test: ExprLoc,
        msg: Option<ExprLoc>,
    },
    Assign {
        target: Identifier,
        object: ExprLoc,
    },
    /// Tuple unpacking assignment (e.g., `a, b = some_tuple` or `(a, b), c = nested`).
    ///
    /// The right-hand side is evaluated, then unpacked into the targets in order.
    /// Supports nested unpacking like `(a, b), c = ((1, 2), 'x')`.
    UnpackAssign {
        /// The targets to unpack into (can be names or nested tuples)
        targets: Vec<UnpackTarget>,
        /// Source position covering all targets (for error message caret placement)
        targets_position: CodeRange,
        object: ExprLoc,
    },
    OpAssign {
        target: Identifier,
        op: Operator,
        object: ExprLoc,
    },
    SubscriptAssign {
        target: Identifier,
        index: ExprLoc,
        value: ExprLoc,
        /// Position of the subscript expression (e.g., `lst[10]`) for traceback carets.
        target_position: CodeRange,
    },
    /// Attribute assignment (e.g., `point.x = 5` or `a.b.c = 5`).
    ///
    /// Assigns a value to an attribute on an object. For mutable dataclasses,
    /// this sets the field value. Returns an error for immutable objects.
    /// Supports chained attribute access on the left-hand side.
    AttrAssign {
        object: ExprLoc,
        attr: Attr,
        target_position: CodeRange,
        value: ExprLoc,
    },
    For {
        /// Loop target - either a single identifier or tuple unpacking pattern.
        target: UnpackTarget,
        iter: ExprLoc,
        body: Vec<Self>,
        or_else: Vec<Self>,
    },
    If {
        test: ExprLoc,
        body: Vec<Self>,
        or_else: Vec<Self>,
    },
    FunctionDef(F),
    /// Global variable declaration. Only present in parsed form, consumed during prepare.
    ///
    /// Declares that the listed names refer to module-level (global) variables,
    /// allowing functions to read and write them instead of creating local variables.
    Global {
        position: CodeRange,
        names: Vec<StringId>,
    },
    /// Nonlocal variable declaration. Only present in parsed form, consumed during prepare.
    ///
    /// Declares that the listed names refer to variables in enclosing function scopes,
    /// allowing nested functions to read and write them instead of creating local variables.
    Nonlocal {
        position: CodeRange,
        names: Vec<StringId>,
    },
    /// Try/except/else/finally block.
    ///
    /// Executes body, catches matching exceptions with handlers, runs else if no exception,
    /// and always runs finally.
    Try(Try<Self>),
    /// Import statement (e.g., `import sys`, `import sys as s`).
    ///
    /// Loads a module and binds it to a name in the current namespace.
    Import {
        /// The module name to import (e.g., "sys", "typing").
        module_name: StringId,
        /// The binding target - contains the name (or alias), position, and namespace slot.
        /// After prepare phase, this includes the resolved namespace slot for storing the module.
        binding: Identifier,
    },
    /// From-import statement (e.g., `from typing import TYPE_CHECKING`).
    ///
    /// Imports specific names from a module into the current namespace.
    ImportFrom {
        /// The module name to import from (e.g., "typing").
        module_name: StringId,
        /// Names to import: (import_name, binding) pairs.
        /// The import_name is the name in the module, the binding is the local name
        /// (alias if provided, otherwise the import name) with resolved namespace slot.
        names: Vec<(StringId, Identifier)>,
        /// Source position for error reporting.
        position: CodeRange,
    },
}

/// A prepared function definition with resolved names and scope information.
///
/// This is created during the prepare phase and contains everything needed to
/// compile the function to bytecode. The function body has all names resolved
/// to namespace indices with proper scoping.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreparedFunctionDef {
    /// The function name identifier with resolved namespace index.
    pub name: Identifier,
    /// The function signature with parameter names and default counts.
    pub signature: Signature,
    /// The prepared function body with resolved names.
    pub body: Vec<Node<Self>>,
    /// Number of local variable slots needed in the namespace.
    pub namespace_size: usize,
    /// Enclosing namespace slots for variables captured from enclosing scopes.
    ///
    /// At definition time: look up cell HeapId from enclosing namespace at each slot.
    /// At call time: captured cells are pushed sequentially (our slots are implicit).
    pub free_var_enclosing_slots: Vec<NamespaceId>,
    /// Number of cell variables (captured by nested functions).
    ///
    /// At call time, this many cells are created and pushed right after params.
    /// Their slots are implicitly params.len()..params.len()+cell_var_count.
    pub cell_var_count: usize,
    /// Maps cell variable indices to their corresponding parameter indices, if any.
    ///
    /// When a parameter is also captured by nested functions (cell variable), its value
    /// must be copied into the cell after binding. Each entry corresponds to a cell
    /// (index 0..cell_var_count), and contains `Some(param_index)` if that cell is for
    /// a parameter, or `None` otherwise.
    pub cell_param_indices: Vec<Option<usize>>,
    /// Prepared default value expressions, evaluated at function definition time.
    ///
    /// Layout: `[pos_defaults...][arg_defaults...][kwarg_defaults...]`
    /// Each group contains only the parameters that have defaults, in declaration order.
    /// The counts in `signature` indicate how many defaults exist for each group.
    pub default_exprs: Vec<ExprLoc>,
}

/// Type alias for prepared AST nodes (output of prepare phase).
pub type PreparedNode = Node<PreparedFunctionDef>;

/// Binary operators for arithmetic, bitwise, and boolean operations.
///
/// Uses strum `Display` derive with per-variant serialization for operator symbols.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Operator {
    // `+`
    Add,
    // `-`
    Sub,
    // `*`
    Mult,
    // `@`
    MatMult,
    // `/`
    Div,
    // `%`
    Mod,
    // `**`
    Pow,
    // `<<`
    LShift,
    // `>>`
    RShift,
    // `|`
    BitOr,
    // `^`
    BitXor,
    // `&`
    BitAnd,
    // `//`
    FloorDiv,
    // bool operators
    // `and`
    And,
    // `or`
    Or,
}

/// Defined separately since these operators always return a bool
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CmpOperator {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    Is,
    IsNot,
    In,
    NotIn,
    // we should support floats too, either via a Number type, or ModEqInt and ModEqFloat
    ModEq(i64),
}
