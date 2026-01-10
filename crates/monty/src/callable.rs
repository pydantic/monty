use crate::{builtins::Builtins, expressions::Identifier, intern::Interns, types::Type};

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

impl Callable {
    /// Returns true if this Callable is equal to another Callable.
    ///
    /// We assume functions with the same name and position in code are equal.
    pub fn py_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Builtin(b1), Self::Builtin(b2)) => b1 == b2,
            (Self::Name(n1), Self::Name(n2)) => n1.py_eq(n2),
            _ => false,
        }
    }

    pub fn py_type(&self) -> Type {
        match self {
            Self::Builtin(b) => b.py_type(),
            Self::Name(_) => Type::Function,
        }
    }

    /// Returns the callable name for error messages.
    ///
    /// For builtins, returns the builtin name (e.g., "print", "len") as a static str.
    /// For named callables, returns the function name from interns.
    pub fn name<'a>(&self, interns: &'a Interns) -> &'a str {
        match self {
            Self::Builtin(Builtins::Function(f)) => (*f).into(),
            Self::Builtin(Builtins::ExcType(e)) => (*e).into(),
            Self::Builtin(Builtins::Type(t)) => (*t).into(),
            Self::Name(ident) => interns.get_str(ident.name_id),
        }
    }
}
