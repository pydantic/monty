use std::{borrow::Cow, fmt};

use ruff_python_ast::{
    self as ast, BoolOp, CmpOp, ConversionFlag as RuffConversionFlag, ElifElseClause, Expr as AstExpr,
    InterpolatedStringElement, Keyword, Number, Operator as AstOperator, ParameterWithDefault, Stmt, UnaryOp,
    name::Name,
};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange};

use crate::{
    StackFrame,
    args::{ArgExprs, Kwarg},
    builtins::Builtins,
    exception_private::ExcType,
    exception_public::{CodeLoc, MontyException},
    expressions::{
        Callable, CmpOperator, Comprehension, Expr, ExprLoc, Identifier, Literal, Node, Operator, UnpackTarget,
    },
    fstring::{ConversionFlag, FStringPart, FormatSpec},
    intern::{InternerBuilder, StringId},
    value::Attr,
};

/// A parameter in a function signature with optional default value.
#[derive(Debug, Clone)]
pub struct ParsedParam {
    /// The parameter name.
    pub name: StringId,
    /// The default value expression (evaluated at definition time).
    pub default: Option<ExprLoc>,
}

/// A parsed function signature with all parameter types.
///
/// This intermediate representation captures the structure of Python function
/// parameters before name resolution. Default value expressions are stored
/// as unevaluated AST and will be evaluated during the prepare phase.
#[derive(Debug, Clone, Default)]
pub struct ParsedSignature {
    /// Positional-only parameters (before `/`).
    pub pos_args: Vec<ParsedParam>,
    /// Positional-or-keyword parameters.
    pub args: Vec<ParsedParam>,
    /// Variable positional parameter (`*args`).
    pub var_args: Option<StringId>,
    /// Keyword-only parameters (after `*` or `*args`).
    pub kwargs: Vec<ParsedParam>,
    /// Variable keyword parameter (`**kwargs`).
    pub var_kwargs: Option<StringId>,
}

impl ParsedSignature {
    /// Returns an iterator over all parameter names in the signature.
    ///
    /// Order: pos_args, args, var_args, kwargs, var_kwargs
    pub fn param_names(&self) -> impl Iterator<Item = StringId> + '_ {
        self.pos_args
            .iter()
            .map(|p| p.name)
            .chain(self.args.iter().map(|p| p.name))
            .chain(self.var_args.iter().copied())
            .chain(self.kwargs.iter().map(|p| p.name))
            .chain(self.var_kwargs.iter().copied())
    }
}

/// A raw (unprepared) function definition from the parser.
///
/// Contains the function name, signature, and body as parsed AST nodes.
/// During the prepare phase, this is transformed into `PreparedFunctionDef`
/// with resolved names and scope information.
#[derive(Debug, Clone)]
pub struct RawFunctionDef {
    /// The function name identifier (not yet resolved to a namespace index).
    pub name: Identifier,
    /// The parsed function signature with parameter names and default expressions.
    pub signature: ParsedSignature,
    /// The unprepared function body (names not yet resolved).
    pub body: Vec<ParseNode>,
}

/// Type alias for parsed AST nodes (output of the parser).
///
/// This uses `Node<RawFunctionDef>` where function definitions contain their
/// full unprepared body. After the prepare phase, this becomes `PreparedNode`
/// (aka `Node<PreparedFunctionDef>`).
pub type ParseNode = Node<RawFunctionDef>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Try<N> {
    pub body: Vec<N>,
    pub handlers: Vec<ExceptHandler<N>>,
    pub or_else: Vec<N>,
    pub finally: Vec<N>,
}

/// A parsed exception handler (except clause).
///
/// Represents `except ExcType as name:` or bare `except:` clauses.
/// The exception type and variable binding are both optional.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExceptHandler<N> {
    /// Exception type(s) to catch. None = bare except (catches all).
    pub exc_type: Option<ExprLoc>,
    /// Variable name for `except X as e:`. None = no binding.
    pub name: Option<Identifier>,
    /// Handler body statements.
    pub body: Vec<N>,
}

/// Result of parsing: the AST nodes and the string interner with all interned names.
#[derive(Debug)]
pub struct ParseResult {
    pub nodes: Vec<ParseNode>,
    pub interner: InternerBuilder,
}

pub(crate) fn parse(code: &str, filename: &str) -> Result<ParseResult, ParseError> {
    let mut parser = Parser::new(code, filename);
    match parse_module(code) {
        Ok(parsed) => {
            let module = parsed.into_syntax();
            let nodes = parser.parse_statements(module.body)?;
            Ok(ParseResult {
                nodes,
                interner: parser.interner,
            })
        }
        Err(e) => Err(ParseError::syntax(e.to_string(), parser.convert_range(e.range()))),
    }
}

/// Parser for converting ruff AST to Monty's intermediate ParseNode representation.
///
/// Holds references to the source code and owns a string interner for names.
/// The filename is interned once at construction and reused for all CodeRanges.
pub struct Parser<'a> {
    line_ends: Vec<usize>,
    code: &'a str,
    /// Interned filename ID, used for all CodeRanges created by this parser.
    filename_id: StringId,
    /// String interner for names (variables, functions, etc).
    pub interner: InternerBuilder,
}

impl<'a> Parser<'a> {
    fn new(code: &'a str, filename: &'a str) -> Self {
        // Position of each line in the source code, to convert indexes to line number and column number
        let mut line_ends = vec![];
        for (i, c) in code.chars().enumerate() {
            if c == '\n' {
                line_ends.push(i);
            }
        }
        let mut interner = InternerBuilder::new(code);
        let filename_id = interner.intern(filename);
        Self {
            line_ends,
            code,
            filename_id,
            interner,
        }
    }

    fn parse_statements(&mut self, statements: Vec<Stmt>) -> Result<Vec<ParseNode>, ParseError> {
        statements.into_iter().map(|f| self.parse_statement(f)).collect()
    }

    fn parse_elif_else_clauses(&mut self, clauses: Vec<ElifElseClause>) -> Result<Vec<ParseNode>, ParseError> {
        let mut tail: Vec<ParseNode> = Vec::new();
        for clause in clauses.into_iter().rev() {
            match clause.test {
                Some(test) => {
                    let test = self.parse_expression(test)?;
                    let body = self.parse_statements(clause.body)?;
                    let or_else = tail;
                    let nested = Node::If { test, body, or_else };
                    tail = vec![nested];
                }
                None => {
                    tail = self.parse_statements(clause.body)?;
                }
            }
        }
        Ok(tail)
    }

    /// Parses an exception handler (except clause).
    ///
    /// Handles `except:`, `except ExcType:`, and `except ExcType as name:` forms.
    fn parse_except_handler(
        &mut self,
        handler: ruff_python_ast::ExceptHandler,
    ) -> Result<ExceptHandler<ParseNode>, ParseError> {
        let ruff_python_ast::ExceptHandler::ExceptHandler(h) = handler;
        let exc_type = match h.type_ {
            Some(expr) => Some(self.parse_expression(*expr)?),
            None => None,
        };
        let name = h.name.map(|n| self.identifier(&n.id, n.range));
        let body = self.parse_statements(h.body)?;
        Ok(ExceptHandler { exc_type, name, body })
    }

    fn parse_statement(&mut self, statement: Stmt) -> Result<ParseNode, ParseError> {
        match statement {
            Stmt::FunctionDef(function) => {
                if function.is_async {
                    return Err(ParseError::not_implemented(
                        "async function definitions",
                        self.convert_range(function.range),
                    ));
                }

                let params = &function.parameters;

                // Parse positional-only parameters (before /)
                let pos_args = self.parse_params_with_defaults(&params.posonlyargs)?;

                // Parse positional-or-keyword parameters
                let args = self.parse_params_with_defaults(&params.args)?;

                // Parse *args
                let var_args = params.vararg.as_ref().map(|p| self.interner.intern(&p.name.id));

                // Parse keyword-only parameters (after * or *args)
                let kwargs = self.parse_params_with_defaults(&params.kwonlyargs)?;

                // Parse **kwargs
                let var_kwargs = params.kwarg.as_ref().map(|p| self.interner.intern(&p.name.id));

                let signature = ParsedSignature {
                    pos_args,
                    args,
                    var_args,
                    kwargs,
                    var_kwargs,
                };

                let name = self.identifier(&function.name.id, function.name.range);
                // Parse function body recursively
                let body = self.parse_statements(function.body)?;

                Ok(Node::FunctionDef(RawFunctionDef { name, signature, body }))
            }
            Stmt::ClassDef(c) => Err(ParseError::not_implemented(
                "class definitions",
                self.convert_range(c.range),
            )),
            Stmt::Return(ast::StmtReturn { value, .. }) => match value {
                Some(value) => Ok(Node::Return(self.parse_expression(*value)?)),
                None => Ok(Node::ReturnNone),
            },
            Stmt::Delete(d) => Err(ParseError::not_implemented(
                "the 'del' statement",
                self.convert_range(d.range),
            )),
            Stmt::TypeAlias(t) => Err(ParseError::not_implemented("type aliases", self.convert_range(t.range))),
            Stmt::Assign(ast::StmtAssign {
                targets, value, range, ..
            }) => self.parse_assignment(first(targets, self.convert_range(range))?, *value),
            Stmt::AugAssign(ast::StmtAugAssign { target, op, value, .. }) => Ok(Node::OpAssign {
                target: self.parse_identifier(*target)?,
                op: convert_op(op),
                object: self.parse_expression(*value)?,
            }),
            Stmt::AnnAssign(ast::StmtAnnAssign { target, value, .. }) => match value {
                Some(value) => self.parse_assignment(*target, *value),
                None => Ok(Node::Pass),
            },
            Stmt::For(ast::StmtFor {
                is_async,
                target,
                iter,
                body,
                orelse,
                range,
                ..
            }) => {
                if is_async {
                    return Err(ParseError::not_implemented(
                        "async for loops",
                        self.convert_range(range),
                    ));
                }
                Ok(Node::For {
                    target: self.parse_unpack_target(*target)?,
                    iter: self.parse_expression(*iter)?,
                    body: self.parse_statements(body)?,
                    or_else: self.parse_statements(orelse)?,
                })
            }
            Stmt::While(w) => Err(ParseError::not_implemented("while loops", self.convert_range(w.range))),
            Stmt::If(ast::StmtIf {
                test,
                body,
                elif_else_clauses,
                ..
            }) => {
                let test = self.parse_expression(*test)?;
                let body = self.parse_statements(body)?;
                let or_else = self.parse_elif_else_clauses(elif_else_clauses)?;
                Ok(Node::If { test, body, or_else })
            }
            Stmt::With(ast::StmtWith { is_async, range, .. }) => {
                if is_async {
                    Err(ParseError::not_implemented(
                        "async context managers (async with)",
                        self.convert_range(range),
                    ))
                } else {
                    Err(ParseError::not_implemented(
                        "context managers (with statements)",
                        self.convert_range(range),
                    ))
                }
            }
            Stmt::Match(m) => Err(ParseError::not_implemented(
                "pattern matching (match statements)",
                self.convert_range(m.range),
            )),
            Stmt::Raise(ast::StmtRaise { exc, .. }) => {
                // TODO add cause to Node::Raise
                let expr = match exc {
                    Some(expr) => Some(self.parse_expression(*expr)?),
                    None => None,
                };
                Ok(Node::Raise(expr))
            }
            Stmt::Try(ast::StmtTry {
                body,
                handlers,
                orelse,
                finalbody,
                is_star,
                range,
                ..
            }) => {
                if is_star {
                    Err(ParseError::not_implemented(
                        "exception groups (try*/except*)",
                        self.convert_range(range),
                    ))
                } else {
                    let body = self.parse_statements(body)?;
                    let handlers = handlers
                        .into_iter()
                        .map(|h| self.parse_except_handler(h))
                        .collect::<Result<Vec<_>, _>>()?;
                    let or_else = self.parse_statements(orelse)?;
                    let finally = self.parse_statements(finalbody)?;
                    Ok(Node::Try(Try {
                        body,
                        handlers,
                        or_else,
                        finally,
                    }))
                }
            }
            Stmt::Assert(ast::StmtAssert { test, msg, .. }) => {
                let test = self.parse_expression(*test)?;
                let msg = match msg {
                    Some(m) => Some(self.parse_expression(*m)?),
                    None => None,
                };
                Ok(Node::Assert { test, msg })
            }
            Stmt::Import(ast::StmtImport { names, range, .. }) => {
                // We only support single module imports (e.g., `import sys`)
                // Multi-module imports (e.g., `import sys, os`) are not supported
                let position = self.convert_range(range);
                if names.len() != 1 {
                    return Err(ParseError::not_implemented("multi-module import statements", position));
                }
                let alias_node = &names[0];
                let module_name = self.interner.intern(&alias_node.name);
                // The binding name is the alias if present, otherwise the module name
                let binding_name = alias_node
                    .asname
                    .as_ref()
                    .map_or(module_name, |n| self.interner.intern(&n.id));
                // Create an unresolved identifier (namespace slot will be set during prepare)
                let binding = Identifier::new(binding_name, position);
                Ok(Node::Import { module_name, binding })
            }
            Stmt::ImportFrom(ast::StmtImportFrom {
                module,
                names,
                level,
                range,
                ..
            }) => {
                let position = self.convert_range(range);
                // We only support absolute imports (level 0)
                if level != 0 {
                    return Err(ParseError::import_error(
                        "attempted relative import with no known parent package",
                        position,
                    ));
                }
                // Module name is required for absolute imports
                let module_name = match module {
                    Some(m) => self.interner.intern(&m),
                    None => {
                        return Err(ParseError::import_error(
                            "attempted relative import with no known parent package",
                            position,
                        ));
                    }
                };
                // Parse the imported names
                let names = names
                    .iter()
                    .map(|alias| {
                        // Check for star import which is not supported
                        if alias.name.as_str() == "*" {
                            return Err(ParseError::not_supported(
                                "Wildcard imports (`from ... import *`) are not supported",
                                position,
                            ));
                        }
                        let name = self.interner.intern(&alias.name);
                        // The binding name is the alias if provided, otherwise the import name
                        let binding_name = alias.asname.as_ref().map_or(name, |n| self.interner.intern(&n.id));
                        // Create an unresolved identifier (namespace slot will be set during prepare)
                        let binding = Identifier::new(binding_name, position);
                        Ok((name, binding))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Node::ImportFrom {
                    module_name,
                    names,
                    position,
                })
            }
            Stmt::Global(ast::StmtGlobal { names, range, .. }) => {
                let names = names
                    .iter()
                    .map(|id| self.interner.intern(&self.code[id.range]))
                    .collect();
                Ok(Node::Global {
                    position: self.convert_range(range),
                    names,
                })
            }
            Stmt::Nonlocal(ast::StmtNonlocal { names, range, .. }) => {
                let names = names
                    .iter()
                    .map(|id| self.interner.intern(&self.code[id.range]))
                    .collect();
                Ok(Node::Nonlocal {
                    position: self.convert_range(range),
                    names,
                })
            }
            Stmt::Expr(ast::StmtExpr { value, .. }) => self.parse_expression(*value).map(Node::Expr),
            Stmt::Pass(_) => Ok(Node::Pass),
            Stmt::Break(b) => Err(ParseError::not_implemented(
                "break statements",
                self.convert_range(b.range),
            )),
            Stmt::Continue(c) => Err(ParseError::not_implemented(
                "continue statements",
                self.convert_range(c.range),
            )),
            Stmt::IpyEscapeCommand(i) => Err(ParseError::not_implemented(
                "IPython escape commands",
                self.convert_range(i.range),
            )),
        }
    }

    /// `lhs = rhs` -> `lhs, rhs`
    /// Handles simple assignments (x = value), subscript assignments (dict[key] = value),
    /// attribute assignments (obj.attr = value), and tuple unpacking (a, b = value)
    fn parse_assignment(&mut self, lhs: AstExpr, rhs: AstExpr) -> Result<ParseNode, ParseError> {
        match lhs {
            // Subscript assignment like dict[key] = value
            AstExpr::Subscript(ast::ExprSubscript {
                value, slice, range, ..
            }) => Ok(Node::SubscriptAssign {
                target: self.parse_identifier(*value)?,
                index: self.parse_expression(*slice)?,
                value: self.parse_expression(rhs)?,
                target_position: self.convert_range(range),
            }),
            // Attribute assignment like obj.attr = value (supports chained like a.b.c = value)
            AstExpr::Attribute(ast::ExprAttribute { value, attr, range, .. }) => Ok(Node::AttrAssign {
                object: self.parse_expression(*value)?,
                attr: Attr::Interned(self.interner.intern(attr.id())),
                target_position: self.convert_range(range),
                value: self.parse_expression(rhs)?,
            }),
            // Tuple unpacking like a, b = value or (a, b), c = nested
            AstExpr::Tuple(ast::ExprTuple { elts, range, .. }) => {
                let targets_position = self.convert_range(range);
                let targets = elts
                    .into_iter()
                    .map(|e| self.parse_unpack_target(e)) // Use parse_unpack_target for recursion
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Node::UnpackAssign {
                    targets,
                    targets_position,
                    object: self.parse_expression(rhs)?,
                })
            }
            // Simple identifier assignment like x = value
            _ => Ok(Node::Assign {
                target: self.parse_identifier(lhs)?,
                object: self.parse_expression(rhs)?,
            }),
        }
    }

    fn parse_expression(&mut self, expression: AstExpr) -> Result<ExprLoc, ParseError> {
        match expression {
            AstExpr::BoolOp(ast::ExprBoolOp { op, values, range, .. }) => {
                // Handle chained boolean operations like `a and b and c` by right-folding
                // into nested binary operations: `a and (b and c)`
                let rust_op = convert_bool_op(op);
                let position = self.convert_range(range);
                let mut values_iter = values.into_iter().rev();

                // Start with the rightmost value
                let last_value = values_iter.next().expect("Expected at least one value in boolean op");
                let mut result = self.parse_expression(last_value)?;

                // Fold from right to left
                for value in values_iter {
                    let left = Box::new(self.parse_expression(value)?);
                    result = ExprLoc::new(
                        position,
                        Expr::Op {
                            left,
                            op: rust_op.clone(),
                            right: Box::new(result),
                        },
                    );
                }
                Ok(result)
            }
            AstExpr::Named(n) => Err(ParseError::not_implemented(
                "named expressions (walrus operator :=)",
                self.convert_range(n.range),
            )),
            AstExpr::BinOp(ast::ExprBinOp {
                left, op, right, range, ..
            }) => {
                let left = Box::new(self.parse_expression(*left)?);
                let right = Box::new(self.parse_expression(*right)?);
                Ok(ExprLoc {
                    position: self.convert_range(range),
                    expr: Expr::Op {
                        left,
                        op: convert_op(op),
                        right,
                    },
                })
            }
            AstExpr::UnaryOp(ast::ExprUnaryOp { op, operand, range, .. }) => match op {
                UnaryOp::Not => {
                    let operand = Box::new(self.parse_expression(*operand)?);
                    Ok(ExprLoc::new(self.convert_range(range), Expr::Not(operand)))
                }
                UnaryOp::USub => {
                    let operand = Box::new(self.parse_expression(*operand)?);
                    Ok(ExprLoc::new(self.convert_range(range), Expr::UnaryMinus(operand)))
                }
                UnaryOp::UAdd => {
                    let operand = Box::new(self.parse_expression(*operand)?);
                    Ok(ExprLoc::new(self.convert_range(range), Expr::UnaryPlus(operand)))
                }
                UnaryOp::Invert => {
                    let operand = Box::new(self.parse_expression(*operand)?);
                    Ok(ExprLoc::new(self.convert_range(range), Expr::UnaryInvert(operand)))
                }
            },
            AstExpr::Lambda(l) => Err(ParseError::not_implemented(
                "lambda expressions",
                self.convert_range(l.range),
            )),
            AstExpr::If(ast::ExprIf {
                test,
                body,
                orelse,
                range,
                ..
            }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::IfElse {
                    test: Box::new(self.parse_expression(*test)?),
                    body: Box::new(self.parse_expression(*body)?),
                    orelse: Box::new(self.parse_expression(*orelse)?),
                },
            )),
            AstExpr::Dict(ast::ExprDict { items, range, .. }) => {
                let position = self.convert_range(range);
                let mut pairs = Vec::new();
                for ast::DictItem { key, value } in items {
                    // key is Option<Expr> - None represents ** unpacking which we don't support yet
                    if let Some(key_expr_ast) = key {
                        let key_expr = self.parse_expression(key_expr_ast)?;
                        let value_expr = self.parse_expression(value)?;
                        pairs.push((key_expr, value_expr));
                    } else {
                        return Err(ParseError::not_implemented(
                            "dictionary unpacking in literals",
                            position,
                        ));
                    }
                }
                Ok(ExprLoc::new(position, Expr::Dict(pairs)))
            }
            AstExpr::Set(ast::ExprSet { elts, range, .. }) => {
                let elements: Result<Vec<_>, _> = elts.into_iter().map(|e| self.parse_expression(e)).collect();
                Ok(ExprLoc::new(self.convert_range(range), Expr::Set(elements?)))
            }
            AstExpr::ListComp(ast::ExprListComp {
                elt, generators, range, ..
            }) => {
                let elt = Box::new(self.parse_expression(*elt)?);
                let generators = self.parse_comprehension_generators(generators)?;
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::ListComp { elt, generators },
                ))
            }
            AstExpr::SetComp(ast::ExprSetComp {
                elt, generators, range, ..
            }) => {
                let elt = Box::new(self.parse_expression(*elt)?);
                let generators = self.parse_comprehension_generators(generators)?;
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::SetComp { elt, generators },
                ))
            }
            AstExpr::DictComp(ast::ExprDictComp {
                key,
                value,
                generators,
                range,
                ..
            }) => {
                let key = Box::new(self.parse_expression(*key)?);
                let value = Box::new(self.parse_expression(*value)?);
                let generators = self.parse_comprehension_generators(generators)?;
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::DictComp { key, value, generators },
                ))
            }
            AstExpr::Generator(ast::ExprGenerator {
                elt, generators, range, ..
            }) => {
                // TODO: When proper generators are implemented, this should produce
                // Expr::Generator instead of Expr::ListComp. Currently we treat generator
                // expressions as list comprehensions since we don't have generator support.
                let elt = Box::new(self.parse_expression(*elt)?);
                let generators = self.parse_comprehension_generators(generators)?;
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::ListComp { elt, generators },
                ))
            }
            AstExpr::Await(a) => Err(ParseError::not_implemented(
                "await expressions",
                self.convert_range(a.range),
            )),
            AstExpr::Yield(y) => Err(ParseError::not_implemented(
                "yield expressions",
                self.convert_range(y.range),
            )),
            AstExpr::YieldFrom(y) => Err(ParseError::not_implemented(
                "yield from expressions",
                self.convert_range(y.range),
            )),
            AstExpr::Compare(ast::ExprCompare {
                left,
                ops,
                comparators,
                range,
                ..
            }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::CmpOp {
                    left: Box::new(self.parse_expression(*left)?),
                    op: convert_compare_op(first(ops.into_vec(), self.convert_range(range))?),
                    right: Box::new(self.parse_expression(first(comparators.into_vec(), self.convert_range(range))?)?),
                },
            )),
            AstExpr::Call(ast::ExprCall {
                func, arguments, range, ..
            }) => {
                let position = self.convert_range(range);
                let ast::Arguments { args, keywords, .. } = arguments;
                let mut positional_args = Vec::new();
                let mut var_args_expr: Option<ExprLoc> = None;
                let mut seen_star = false;

                for arg_expr in args.into_vec() {
                    match arg_expr {
                        AstExpr::Starred(ast::ExprStarred { value, .. }) => {
                            if var_args_expr.is_some() {
                                return Err(ParseError::not_implemented("multiple *args unpacking", position));
                            }
                            var_args_expr = Some(self.parse_expression(*value)?);
                            seen_star = true;
                        }
                        other => {
                            if seen_star {
                                return Err(ParseError::not_implemented(
                                    "positional arguments after *args unpacking",
                                    position,
                                ));
                            }
                            positional_args.push(self.parse_expression(other)?);
                        }
                    }
                }
                // Separate regular kwargs (key=value) from var_kwargs (**expr)
                let (kwargs, var_kwargs) = self.parse_keywords(keywords.into_vec())?;
                let args = ArgExprs::new_with_var_kwargs(positional_args, var_args_expr, kwargs, var_kwargs);
                match *func {
                    AstExpr::Name(ast::ExprName { id, range, .. }) => {
                        let name = id.to_string();
                        // Try to resolve the name as a builtin function or exception type.
                        // If neither, treat it as a name to be looked up at runtime.
                        let callable = if let Ok(builtin) = name.parse::<Builtins>() {
                            Callable::Builtin(builtin)
                        } else {
                            // Name will be looked up in the namespace at runtime
                            let ident = self.identifier(&id, range);
                            Callable::Name(ident)
                        };
                        Ok(ExprLoc::new(
                            position,
                            Expr::Call {
                                callable,
                                args: Box::new(args),
                            },
                        ))
                    }
                    AstExpr::Attribute(ast::ExprAttribute { value, attr, .. }) => {
                        let object = Box::new(self.parse_expression(*value)?);
                        Ok(ExprLoc::new(
                            position,
                            Expr::AttrCall {
                                object,
                                attr: Attr::Interned(self.interner.intern(attr.id())),
                                args: Box::new(args),
                            },
                        ))
                    }
                    other => Err(ParseError::syntax(
                        format!("Expected name or attribute, got {other:?}"),
                        position,
                    )),
                }
            }
            AstExpr::FString(ast::ExprFString { value, range, .. }) => self.parse_fstring(&value, range),
            AstExpr::TString(t) => Err(ParseError::not_implemented(
                "template strings (t-strings)",
                self.convert_range(t.range),
            )),
            AstExpr::StringLiteral(ast::ExprStringLiteral { value, range, .. }) => {
                let string_id = self.interner.intern(&value.to_string());
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Literal(Literal::Str(string_id)),
                ))
            }
            AstExpr::BytesLiteral(ast::ExprBytesLiteral { value, range, .. }) => {
                let bytes: Cow<'_, [u8]> = Cow::from(&value);
                let bytes_id = self.interner.intern_bytes(&bytes);
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Literal(Literal::Bytes(bytes_id)),
                ))
            }
            AstExpr::NumberLiteral(ast::ExprNumberLiteral { value, range, .. }) => {
                let position = self.convert_range(range);
                let const_value = match value {
                    Number::Int(i) => match i.as_i64() {
                        Some(i) => Literal::Int(i),
                        None => return Err(ParseError::not_implemented("integers larger than 64 bits", position)),
                    },
                    Number::Float(f) => Literal::Float(f),
                    Number::Complex { .. } => return Err(ParseError::not_implemented("complex constants", position)),
                };
                Ok(ExprLoc::new(position, Expr::Literal(const_value)))
            }
            AstExpr::BooleanLiteral(ast::ExprBooleanLiteral { value, range, .. }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Literal(Literal::Bool(value)),
            )),
            AstExpr::NoneLiteral(ast::ExprNoneLiteral { range, .. }) => {
                Ok(ExprLoc::new(self.convert_range(range), Expr::Literal(Literal::None)))
            }
            AstExpr::EllipsisLiteral(ast::ExprEllipsisLiteral { range, .. }) => Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Literal(Literal::Ellipsis),
            )),
            AstExpr::Attribute(ast::ExprAttribute { value, attr, range, .. }) => {
                let object = Box::new(self.parse_expression(*value)?);
                let position = self.convert_range(range);
                Ok(ExprLoc::new(
                    position,
                    Expr::AttrGet {
                        object,
                        attr: Attr::Interned(self.interner.intern(attr.id())),
                    },
                ))
            }
            AstExpr::Subscript(ast::ExprSubscript {
                value, slice, range, ..
            }) => {
                let object = Box::new(self.parse_expression(*value)?);
                let index = Box::new(self.parse_expression(*slice)?);
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Subscript { object, index },
                ))
            }
            AstExpr::Starred(s) => Err(ParseError::not_implemented(
                "starred expressions (*expr)",
                self.convert_range(s.range),
            )),
            AstExpr::Name(ast::ExprName { id, range, .. }) => {
                let name = id.to_string();
                let position = self.convert_range(range);
                // Check if the name is a builtin function or exception type
                let expr = if let Ok(builtin) = name.parse::<Builtins>() {
                    Expr::Builtin(builtin)
                } else {
                    Expr::Name(self.identifier(&id, range))
                };
                Ok(ExprLoc::new(position, expr))
            }
            AstExpr::List(ast::ExprList { elts, range, .. }) => {
                let items = elts
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<Result<_, ParseError>>()?;

                Ok(ExprLoc::new(self.convert_range(range), Expr::List(items)))
            }
            AstExpr::Tuple(ast::ExprTuple { elts, range, .. }) => {
                let items = elts
                    .into_iter()
                    .map(|f| self.parse_expression(f))
                    .collect::<Result<_, ParseError>>()?;

                Ok(ExprLoc::new(self.convert_range(range), Expr::Tuple(items)))
            }
            AstExpr::Slice(s) => Err(ParseError::not_implemented("slice syntax", self.convert_range(s.range))),
            AstExpr::IpyEscapeCommand(i) => Err(ParseError::not_implemented(
                "IPython escape commands",
                self.convert_range(i.range),
            )),
        }
    }

    /// Parses keyword arguments, separating regular kwargs from var_kwargs (`**expr`).
    ///
    /// Returns `(kwargs, var_kwargs)` where kwargs is a vec of named keyword arguments
    /// and var_kwargs is an optional expression for `**expr` unpacking.
    fn parse_keywords(&mut self, keywords: Vec<Keyword>) -> Result<(Vec<Kwarg>, Option<ExprLoc>), ParseError> {
        let mut kwargs = Vec::new();
        let mut var_kwargs = None;

        for kwarg in keywords {
            if let Some(key) = kwarg.arg {
                // Regular kwarg: key=value
                let key = self.identifier(&key.id, key.range);
                let value = self.parse_expression(kwarg.value)?;
                kwargs.push(Kwarg { key, value });
            } else {
                // Var kwargs: **expr
                if var_kwargs.is_some() {
                    return Err(ParseError::not_implemented(
                        "multiple **kwargs unpacking",
                        self.convert_range(kwarg.range),
                    ));
                }
                var_kwargs = Some(self.parse_expression(kwarg.value)?);
            }
        }

        Ok((kwargs, var_kwargs))
    }

    fn parse_identifier(&mut self, ast: AstExpr) -> Result<Identifier, ParseError> {
        match ast {
            AstExpr::Name(ast::ExprName { id, range, .. }) => Ok(self.identifier(&id, range)),
            other => Err(ParseError::syntax(
                format!("Expected name, got {other:?}"),
                self.convert_range(other.range()),
            )),
        }
    }

    /// Parses an unpack target - either a single identifier or a nested tuple.
    ///
    /// Handles patterns like `a` (single variable), `a, b` (flat tuple), or `(a, b), c` (nested).
    fn parse_unpack_target(&mut self, ast: AstExpr) -> Result<UnpackTarget, ParseError> {
        match ast {
            AstExpr::Name(ast::ExprName { id, range, .. }) => Ok(UnpackTarget::Name(self.identifier(&id, range))),
            AstExpr::Tuple(ast::ExprTuple { elts, range, .. }) => {
                let position = self.convert_range(range);
                let targets = elts
                    .into_iter()
                    .map(|e| self.parse_unpack_target(e)) // Recursive call for nested tuples
                    .collect::<Result<Vec<_>, _>>()?;
                if targets.is_empty() {
                    return Err(ParseError::syntax("empty tuple in unpack target", position));
                }
                Ok(UnpackTarget::Tuple { targets, position })
            }
            other => Err(ParseError::syntax(
                format!("invalid unpacking target: {other:?}"),
                self.convert_range(other.range()),
            )),
        }
    }

    fn identifier(&mut self, id: &Name, range: TextRange) -> Identifier {
        let string_id = self.interner.intern(id);
        Identifier::new(string_id, self.convert_range(range))
    }

    /// Parses function parameters with optional default values.
    ///
    /// Handles parameters like `a`, `b=10`, `c=None` by extracting the parameter
    /// name and parsing any default expression. Default expressions are stored
    /// as unevaluated AST and will be evaluated during the prepare phase.
    fn parse_params_with_defaults(&mut self, params: &[ParameterWithDefault]) -> Result<Vec<ParsedParam>, ParseError> {
        params
            .iter()
            .map(|p| {
                let name = self.interner.intern(&p.parameter.name.id);
                let default = match &p.default {
                    Some(expr) => Some(self.parse_expression((**expr).clone())?),
                    None => None,
                };
                Ok(ParsedParam { name, default })
            })
            .collect()
    }

    /// Parses comprehension generators (the `for ... in ... if ...` clauses).
    ///
    /// Each generator represents one `for` clause with zero or more `if` filters.
    /// Multiple generators create nested iteration. Supports both single identifiers
    /// (`for x in ...`) and tuple unpacking (`for x, y in ...`).
    fn parse_comprehension_generators(
        &mut self,
        generators: Vec<ast::Comprehension>,
    ) -> Result<Vec<Comprehension>, ParseError> {
        generators
            .into_iter()
            .map(|comp| {
                if comp.is_async {
                    return Err(ParseError::not_implemented(
                        "async comprehensions",
                        self.convert_range(comp.range),
                    ));
                }
                let target = self.parse_unpack_target(comp.target)?;
                let iter = self.parse_expression(comp.iter)?;
                let ifs = comp
                    .ifs
                    .into_iter()
                    .map(|cond| self.parse_expression(cond))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Comprehension { target, iter, ifs })
            })
            .collect()
    }

    /// Parses an f-string value into expression parts.
    ///
    /// F-strings in ruff AST are represented as `FStringValue` containing
    /// `FStringPart`s, which can be either literal strings or `FString`
    /// interpolated sections. Each `FString` contains `InterpolatedStringElements`.
    fn parse_fstring(&mut self, value: &ast::FStringValue, range: TextRange) -> Result<ExprLoc, ParseError> {
        let mut parts = Vec::new();

        for fstring_part in value {
            match fstring_part {
                ast::FStringPart::Literal(lit) => {
                    // Literal string segment - intern for use at runtime
                    let processed = lit.value.to_string();
                    if !processed.is_empty() {
                        let string_id = self.interner.intern(&processed);
                        parts.push(FStringPart::Literal(string_id));
                    }
                }
                ast::FStringPart::FString(fstring) => {
                    // Interpolated f-string section
                    for element in &fstring.elements {
                        let part = self.parse_fstring_element(element)?;
                        parts.push(part);
                    }
                }
            }
        }

        // Optimization: if only one literal part, return as simple string literal
        if parts.len() == 1
            && let FStringPart::Literal(string_id) = parts[0]
        {
            return Ok(ExprLoc::new(
                self.convert_range(range),
                Expr::Literal(Literal::Str(string_id)),
            ));
        }

        Ok(ExprLoc::new(self.convert_range(range), Expr::FString(parts)))
    }

    /// Parses a single f-string element (literal or interpolation).
    fn parse_fstring_element(&mut self, element: &InterpolatedStringElement) -> Result<FStringPart, ParseError> {
        match element {
            InterpolatedStringElement::Literal(lit) => {
                // Intern the literal string for use at runtime
                let processed = lit.value.to_string();
                let string_id = self.interner.intern(&processed);
                Ok(FStringPart::Literal(string_id))
            }
            InterpolatedStringElement::Interpolation(interp) => {
                let expr = Box::new(self.parse_expression((*interp.expression).clone())?);
                let conversion = convert_conversion_flag(interp.conversion);
                let format_spec = match &interp.format_spec {
                    Some(spec) => Some(self.parse_format_spec(spec)?),
                    None => None,
                };
                // Extract debug prefix for `=` specifier (e.g., f'{a=}' -> "a=")
                let debug_prefix = interp.debug_text.as_ref().map(|dt| {
                    let expr_text = &self.code[interp.expression.range()];
                    self.interner
                        .intern(&format!("{}{}{}", dt.leading, expr_text, dt.trailing))
                });
                Ok(FStringPart::Interpolation {
                    expr,
                    conversion,
                    format_spec,
                    debug_prefix,
                })
            }
        }
    }

    /// Parses a format specification, which may contain nested interpolations.
    ///
    /// For static specs (no interpolations), parses the format string into a
    /// `ParsedFormatSpec` at parse time to avoid runtime parsing overhead.
    fn parse_format_spec(&mut self, spec: &ast::InterpolatedStringFormatSpec) -> Result<FormatSpec, ParseError> {
        let mut parts = Vec::new();
        let mut has_interpolation = false;

        for element in &spec.elements {
            match element {
                InterpolatedStringElement::Literal(lit) => {
                    // Intern the literal string
                    let processed = lit.value.to_string();
                    let string_id = self.interner.intern(&processed);
                    parts.push(FStringPart::Literal(string_id));
                }
                InterpolatedStringElement::Interpolation(interp) => {
                    has_interpolation = true;
                    let expr = Box::new(self.parse_expression((*interp.expression).clone())?);
                    let conversion = convert_conversion_flag(interp.conversion);
                    // Format specs within format specs are not allowed in Python,
                    // and debug_prefix doesn't apply to nested interpolations
                    parts.push(FStringPart::Interpolation {
                        expr,
                        conversion,
                        format_spec: None,
                        debug_prefix: None,
                    });
                }
            }
        }

        if has_interpolation {
            Ok(FormatSpec::Dynamic(parts))
        } else {
            // Combine all literal parts into a single static string and parse at parse time
            let static_spec: String = parts
                .into_iter()
                .filter_map(|p| {
                    if let FStringPart::Literal(string_id) = p {
                        Some(self.interner.get_str(string_id).to_owned())
                    } else {
                        None
                    }
                })
                .collect();
            let parsed = static_spec.parse().map_err(|spec_str| {
                ParseError::syntax(
                    format!("Invalid format specifier '{spec_str}'"),
                    self.convert_range(spec.range),
                )
            })?;
            Ok(FormatSpec::Static(parsed))
        }
    }

    fn convert_range(&self, range: TextRange) -> CodeRange {
        let start = range.start().into();
        let (start_line_no, start_line_start, _) = self.index_to_position(start);
        let start = CodeLoc::new(start_line_no, start - start_line_start);

        let end = range.end().into();
        let (end_line_no, end_line_start, _) = self.index_to_position(end);
        let end = CodeLoc::new(end_line_no, end - end_line_start);

        // Store line number for single-line ranges, None for multi-line
        let preview_line = if start_line_no == end_line_no {
            Some(u32::try_from(start_line_no).expect("line number exceeds u32"))
        } else {
            None
        };

        CodeRange::new(self.filename_id, start, end, preview_line)
    }

    fn index_to_position(&self, index: usize) -> (usize, usize, Option<usize>) {
        let mut line_start = 0;
        for (line_no, line_end) in self.line_ends.iter().enumerate() {
            if index <= *line_end {
                return (line_no, line_start, Some(*line_end));
            }
            line_start = *line_end + 1;
        }
        // Content after the last newline (file without trailing newline)
        // line_ends.len() gives the correct 0-indexed line number
        (self.line_ends.len(), line_start, None)
    }
}

fn first<T: fmt::Debug>(v: Vec<T>, position: CodeRange) -> Result<T, ParseError> {
    if v.len() == 1 {
        v.into_iter()
            .next()
            .ok_or_else(|| ParseError::syntax("Expected 1 element, got 0", position))
    } else {
        Err(ParseError::syntax(
            format!("Expected 1 element, got {} (raw: {v:?})", v.len()),
            position,
        ))
    }
}

fn convert_op(op: AstOperator) -> Operator {
    match op {
        AstOperator::Add => Operator::Add,
        AstOperator::Sub => Operator::Sub,
        AstOperator::Mult => Operator::Mult,
        AstOperator::MatMult => Operator::MatMult,
        AstOperator::Div => Operator::Div,
        AstOperator::Mod => Operator::Mod,
        AstOperator::Pow => Operator::Pow,
        AstOperator::LShift => Operator::LShift,
        AstOperator::RShift => Operator::RShift,
        AstOperator::BitOr => Operator::BitOr,
        AstOperator::BitXor => Operator::BitXor,
        AstOperator::BitAnd => Operator::BitAnd,
        AstOperator::FloorDiv => Operator::FloorDiv,
    }
}

fn convert_bool_op(op: BoolOp) -> Operator {
    match op {
        BoolOp::And => Operator::And,
        BoolOp::Or => Operator::Or,
    }
}

fn convert_compare_op(op: CmpOp) -> CmpOperator {
    match op {
        CmpOp::Eq => CmpOperator::Eq,
        CmpOp::NotEq => CmpOperator::NotEq,
        CmpOp::Lt => CmpOperator::Lt,
        CmpOp::LtE => CmpOperator::LtE,
        CmpOp::Gt => CmpOperator::Gt,
        CmpOp::GtE => CmpOperator::GtE,
        CmpOp::Is => CmpOperator::Is,
        CmpOp::IsNot => CmpOperator::IsNot,
        CmpOp::In => CmpOperator::In,
        CmpOp::NotIn => CmpOperator::NotIn,
    }
}

/// Converts ruff's ConversionFlag to our ConversionFlag.
fn convert_conversion_flag(flag: RuffConversionFlag) -> ConversionFlag {
    match flag {
        RuffConversionFlag::None => ConversionFlag::None,
        RuffConversionFlag::Str => ConversionFlag::Str,
        RuffConversionFlag::Repr => ConversionFlag::Repr,
        RuffConversionFlag::Ascii => ConversionFlag::Ascii,
    }
}

/// Source code location information for error reporting.
///
/// Contains filename (as StringId), line/column positions, and optionally a line number for
/// extracting the preview line from source during traceback formatting.
///
/// To display the filename, the caller must provide access to the string storage.
#[derive(Clone, Copy, Default, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CodeRange {
    /// Interned filename ID - look up in Interns to get the actual string.
    pub filename: StringId,
    /// Line number (0-indexed) for extracting preview from source. None if range spans multiple lines.
    preview_line: Option<u32>,
    start: CodeLoc,
    end: CodeLoc,
}

/// Custom Debug implementation to make displaying code much less verbose.
impl fmt::Debug for CodeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CodeRange{{filename: {:?}, start: {:?}, end: {:?}}}",
            self.filename, self.start, self.end
        )
    }
}

impl CodeRange {
    fn new(filename: StringId, start: CodeLoc, end: CodeLoc, preview_line: Option<u32>) -> Self {
        Self {
            filename,
            preview_line,
            start,
            end,
        }
    }

    /// Returns the start position.
    #[must_use]
    pub fn start(&self) -> CodeLoc {
        self.start
    }

    /// Returns the end position.
    #[must_use]
    pub fn end(&self) -> CodeLoc {
        self.end
    }

    /// Returns the preview line number (0-indexed) if available.
    #[must_use]
    pub fn preview_line_number(&self) -> Option<u32> {
        self.preview_line
    }
}

/// Errors that can occur during parsing or preparation of Python code.
#[derive(Debug, Clone)]
pub enum ParseError {
    /// Error in syntax
    Syntax {
        msg: Cow<'static, str>,
        position: CodeRange,
    },
    /// Missing feature from Monty, we hope to implement in the future.
    /// Message gets prefixed with "The monty syntax parser does not yet support ".
    NotImplemented {
        msg: Cow<'static, str>,
        position: CodeRange,
    },
    /// Missing feature with a custom full message (no prefix added).
    NotSupported {
        msg: Cow<'static, str>,
        position: CodeRange,
    },
    /// Import error (e.g., relative imports without a package).
    Import {
        msg: Cow<'static, str>,
        position: CodeRange,
    },
}

impl ParseError {
    fn not_implemented(msg: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self::NotImplemented {
            msg: msg.into(),
            position,
        }
    }

    fn not_supported(msg: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self::NotSupported {
            msg: msg.into(),
            position,
        }
    }

    fn import_error(msg: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self::Import {
            msg: msg.into(),
            position,
        }
    }

    pub(crate) fn syntax(msg: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
        Self::Syntax {
            msg: msg.into(),
            position,
        }
    }
}

impl ParseError {
    pub fn into_python_exc(self, filename: &str, source: &str) -> MontyException {
        match self {
            Self::Syntax { msg, position } => MontyException::new_full(
                ExcType::SyntaxError,
                Some(msg.into_owned()),
                vec![StackFrame::from_position(position, filename, source)],
            ),
            Self::NotImplemented { msg, position } => MontyException::new_full(
                ExcType::NotImplementedError,
                Some(format!("The monty syntax parser does not yet support {msg}")),
                vec![StackFrame::from_position(position, filename, source)],
            ),
            Self::NotSupported { msg, position } => MontyException::new_full(
                ExcType::NotImplementedError,
                Some(msg.into_owned()),
                vec![StackFrame::from_position(position, filename, source)],
            ),
            Self::Import { msg, position } => MontyException::new_full(
                ExcType::ImportError,
                Some(msg.into_owned()),
                vec![StackFrame::from_position_no_caret(position, filename, source)],
            ),
        }
    }
}
