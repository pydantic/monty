use crate::{
    args::ArgExprs,
    builtins::Builtins,
    callable::Callable,
    fstring::FStringPart,
    intern::{BytesId, FunctionId, StringId},
    namespace::NamespaceId,
    operators::{CmpOperator, Operator},
    parse::{CodeRange, Try},
    value::{Attr, Value},
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
    /// Variable is in the current frame's local namespace
    #[default]
    Local,
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

    /// Returns true if this identifier is equal to another identifier.
    ///
    /// We assume identifiers with the same name and position in code are equal.
    pub fn py_eq(&self, other: &Self) -> bool {
        self.name_id == other.name_id && self.position == other.position
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

/// A prepared AST node ready for execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Node {
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
    OpAssign {
        target: Identifier,
        op: Operator,
        object: ExprLoc,
    },
    SubscriptAssign {
        target: Identifier,
        index: ExprLoc,
        value: ExprLoc,
    },
    /// Attribute assignment (e.g., `point.x = 5` or `a.b.c = 5`).
    ///
    /// Assigns a value to an attribute on an object. For mutable dataclasses,
    /// this sets the field value. Returns an error for immutable objects.
    /// Supports chained attribute access on the left-hand side.
    AttrAssign {
        object: Box<ExprLoc>,
        attr: Attr,
        target_position: CodeRange,
        value: ExprLoc,
    },
    For {
        target: Identifier,
        iter: ExprLoc,
        body: Vec<Node>,
        or_else: Vec<Node>,
    },
    If {
        test: ExprLoc,
        body: Vec<Node>,
        or_else: Vec<Node>,
    },
    FunctionDef(FunctionId),
    /// Try/except/else/finally block.
    ///
    /// Executes body, catches matching exceptions with handlers, runs else if no exception,
    /// and always runs finally.
    Try(Try<Node>),
}

impl Node {
    /// Returns the source code position of this node, if available.
    ///
    /// Most nodes have position info through their expressions. Some nodes
    /// like `ReturnNone` don't have inherent position info and return `None`.
    #[must_use]
    pub fn position(&self) -> Option<CodeRange> {
        match self {
            Self::Expr(expr) => Some(expr.position),
            Self::Return(expr) => Some(expr.position),
            Self::ReturnNone => None,
            Self::Raise(Some(expr)) => Some(expr.position),
            Self::Raise(None) => None,
            Self::Assert { test, .. } => Some(test.position),
            Self::Assign { object, .. } => Some(object.position),
            Self::OpAssign { object, .. } => Some(object.position),
            Self::SubscriptAssign { value, .. } => Some(value.position),
            Self::AttrAssign { value, .. } => Some(value.position),
            Self::For { iter, .. } => Some(iter.position),
            Self::If { test, .. } => Some(test.position),
            Self::FunctionDef(_) => None,
            Self::Try(try_) => try_.body.first().and_then(Node::position),
        }
    }
}
