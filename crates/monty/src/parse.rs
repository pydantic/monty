use std::{borrow::Cow, fmt};

use monty_types::{MontyException, StackFrame};
use num_bigint::BigInt;
use num_traits::Num;
use ruff_python_ast::{
    self as ast, BoolOp, CmpOp, ConversionFlag as RuffConversionFlag, ElifElseClause, Expr as AstExpr,
    InterpolatedStringElement, Keyword, Number, Operator as AstOperator, ParameterWithDefault, Stmt, UnaryOp,
    name::Name,
    token::TokenKind,
    visitor::{Visitor, walk_expr},
};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::{
    args::{ArgExprs, CallArg, CallKwarg, Kwarg},
    exception_private::ExcType,
    expressions::{
        AssignTarget, Callable, CmpOperator, Comprehension, DictItem, Expr, ExprLoc, Identifier, ImportName, Literal,
        Node, Operator, SequenceItem, UnpackTarget,
    },
    fstring::{ConversionFlag, FStringPart, FormatSpec, ParsedFormatSpec, encode_format_spec},
    intern::{InternerBuilder, StringId},
    source_map::{SourceMap, StackFrameExt},
    stringize::stringize_annotation,
    types::long_int::INT_MAX_STR_DIGITS,
    value::EitherStr,
};

/// Maximum nesting depth for AST structures during parsing.
/// Matches CPython's limit of ~200 for nested parentheses.
/// This prevents stack overflow from deeply nested structures like `((((x,),),),)`.
#[cfg(not(debug_assertions))]
pub const MAX_NESTING_DEPTH: u16 = 200;
/// In debug builds, we use a lower limit because stack frames are much larger
/// (no inlining, debug info, etc.). The limit is set conservatively to prevent
/// stack overflow while still catching the error before the recursion limit.
#[cfg(debug_assertions)]
pub const MAX_NESTING_DEPTH: u16 = 30;

/// `from __future__ import ...` features whose semantics Monty already provides,
/// so importing them is a no-op rather than an error.
///
/// All but the last are inert in CPython too, having become mandatory by 3.7.
/// `annotations` is still meaningful there, and a no-op here only because Monty
/// stringizes unconditionally (see `limitations/typing.md`) — what it asks for.
const SUPPORTED_FUTURES: [&str; 9] = [
    "nested_scopes",
    "generators",
    "division",
    "absolute_import",
    "with_statement",
    "print_function",
    "unicode_literals",
    "generator_stop",
    "annotations",
];

/// `from __future__ import ...` features Monty does not implement, rejected
/// rather than silently ignored. `barry_as_FLUFL` (PEP 401) makes `<>` the
/// inequality operator and `!=` a `SyntaxError`; Monty parses neither
/// differently. With [`SUPPORTED_FUTURES`] this must cover CPython's
/// `all_feature_names` — a name in neither is reported as undefined.
const UNSUPPORTED_FUTURES: [&str; 1] = ["barry_as_FLUFL"];

/// A parameter in a function signature with optional default value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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

    /// Returns an iterator over every default-value expression in the signature.
    ///
    /// Defaults are evaluated at *definition* time in the **enclosing** scope,
    /// not inside the function being defined. Closure analysis relies on this:
    /// a name referenced by a default (e.g. `def inner(b=a)`) is a capture of
    /// the enclosing scope, so the cell-var pre-pass must scan these as well as
    /// the body (see `collect_cell_vars_from_node`). `*args`/`**kwargs` never
    /// carry defaults, so they are not included.
    pub fn default_exprs(&self) -> impl Iterator<Item = &ExprLoc> + '_ {
        self.pos_args
            .iter()
            .chain(self.args.iter())
            .chain(self.kwargs.iter())
            .filter_map(|p| p.default.as_ref())
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
    /// Whether this is an async function (`async def`).
    pub is_async: bool,
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
    parse_with_interner(code, filename, InternerBuilder::new(code))
}

/// Builds a [`CodeRange`] from an interned filename and a ruff range.
///
/// Free rather than a `Parser` method so a syntax error raised before the parser
/// exists can be located the same way [`Parser::convert_range`] does.
fn code_range(filename: StringId, range: TextRange) -> CodeRange {
    CodeRange {
        filename,
        start_byte: range.start().into(),
        end_byte: range.end().into(),
    }
}

/// Parses code using a caller-provided interner seed.
///
/// This enables incremental compilation flows (e.g. REPL) where existing
/// interned IDs must remain stable across parse invocations.
pub(crate) fn parse_with_interner(
    code: &str,
    filename: &str,
    mut interner: InternerBuilder,
) -> Result<ParseResult, ParseError> {
    // Interned up front so a syntax error can be located without a `Parser`,
    // leaving the parser to be built once, fully populated, after parsing.
    let filename_id = interner.intern(filename);
    let parsed =
        parse_module(code).map_err(|e| ParseError::syntax(e.error.to_string(), code_range(filename_id, e.range())))?;
    // Harvested before `into_syntax` drops the token stream.
    let class_keyword_offsets = parsed
        .tokens()
        .iter()
        .filter(|token| token.kind() == TokenKind::Class)
        .map(Ranged::start)
        .collect();
    let mut parser = Parser::new(code, filename_id, interner, class_keyword_offsets);
    let nodes = parser.parse_statements(parsed.into_syntax().body)?;
    Ok(ParseResult {
        nodes,
        interner: parser.interner,
    })
}

/// Parser for converting ruff AST to Monty's intermediate ParseNode representation.
///
/// Holds references to the source code and owns a string interner for names.
/// The filename is interned once at construction and reused for all CodeRanges.
pub struct Parser<'a> {
    code: &'a str,
    /// Interned filename ID, used for all CodeRanges created by this parser.
    filename_id: StringId,
    /// String interner for names (variables, functions, etc).
    pub interner: InternerBuilder,
    /// Remaining nesting depth budget for recursive structures.
    /// Starts at MAX_NESTING_DEPTH and decrements on each nested level.
    /// When it reaches zero, we return a "Source is too deeply nested" syntax error.
    depth_remaining: u16,
    /// Ascending source offsets of every `class` keyword, taken from the lexer.
    ///
    /// Ruff's AST is abstract — a `StmtClassDef` *is* a class statement, so it
    /// never records where `class` sat, and its range starts at the first
    /// decorator. CPython locates a statement at its keyword, so a decorated
    /// class's traceback frame needs the concrete position. Read only by
    /// [`Parser::class_keyword_range`]; `def`/`async def` will want the same
    /// treatment if function decorators are supported.
    class_keyword_offsets: Vec<TextSize>,
}

impl<'a> Parser<'a> {
    fn new(
        code: &'a str,
        filename_id: StringId,
        interner: InternerBuilder,
        class_keyword_offsets: Vec<TextSize>,
    ) -> Self {
        Self {
            code,
            filename_id,
            interner,
            depth_remaining: MAX_NESTING_DEPTH,
            class_keyword_offsets,
        }
    }

    fn parse_statements(
        &mut self,
        statements: impl IntoIterator<Item = Stmt, IntoIter: ExactSizeIterator>,
    ) -> Result<Vec<ParseNode>, ParseError> {
        // Explicit pre-allocation matters here — `.map(..).collect::<Result<Vec<_>, _>>()`
        // does NOT pre-size the output. Collecting into `Result<Vec<_>, _>` runs the
        // iterator through `iter::try_process`'s `Shunt` adapter (so an `Err` can
        // short-circuit), and `Shunt`'s `size_hint` lower bound is 0 — which loses
        // the `TrustedLen` specialization that would otherwise forward the source
        // iterator's length. Each `Stmt` maps to exactly one `ParseNode`.
        //
        // Accepting `IntoIterator<Item = Stmt, IntoIter: ExactSizeIterator>` lets callers
        // pass either `Vec<Stmt>` or ruff's `ThinVec<Stmt>` without an intermediate copy.
        let iter = statements.into_iter();
        let mut out = Vec::with_capacity(iter.len());
        for stmt in iter {
            out.push(self.parse_statement(stmt)?);
        }
        Ok(out)
    }

    /// Folds a flat list of `elif`/`else` clauses into a right-nested `Node::If` tree.
    ///
    /// Ruff hands us the clauses as a flat `Vec`, but the prepared AST and the
    /// bytecode compiler both walk the resulting tree recursively. Each `elif`
    /// clause is therefore counted against the same depth budget that bounds
    /// explicitly nested source constructs — without this, a long flat chain
    /// would produce an AST far deeper than [`MAX_NESTING_DEPTH`] and overflow
    /// the host's native stack during the prepare or compile phases.
    ///
    /// The depth budget consumed during the fold is restored on success so
    /// sibling statements are not penalized. On parse errors the budget is
    /// left decremented; this is harmless because the parser aborts entirely
    /// and `depth_remaining` is never consulted again.
    fn parse_elif_else_clauses(&mut self, clauses: Vec<ElifElseClause>) -> Result<Vec<ParseNode>, ParseError> {
        let mut tail: Vec<ParseNode> = Vec::new();
        let mut levels: u16 = 0;
        for clause in clauses.into_iter().rev() {
            match clause.test {
                Some(test) => {
                    // Account for the extra nesting level this clause adds to
                    // the result tree.
                    self.decr_depth_remaining(|| test.range())?;
                    levels += 1;
                    let test = self.parse_expression(test)?;
                    let body = self.parse_statements(clause.body)?;
                    let or_else = tail;
                    tail = vec![Node::If { test, body, or_else }];
                }
                None => {
                    tail = self.parse_statements(clause.body)?;
                }
            }
        }
        self.depth_remaining += levels;
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
        self.decr_depth_remaining(|| statement.range())?;
        let result = self.parse_statement_impl(statement);
        self.depth_remaining += 1;
        result
    }

    fn parse_statement_impl(&mut self, statement: Stmt) -> Result<ParseNode, ParseError> {
        match statement {
            Stmt::FunctionDef(function) => Ok(Node::FunctionDef(self.parse_function_def(function)?)),
            Stmt::ClassDef(c) => self.parse_class_def(c),
            Stmt::Return(ast::StmtReturn { value, .. }) => Ok(Node::Return(match value {
                Some(value) => Some(self.parse_expression(*value)?),
                None => None,
            })),
            Stmt::Delete(d) => Err(ParseError::not_implemented(
                "the 'del' statement",
                self.convert_range(d.range),
            )),
            Stmt::TypeAlias(t) => Err(ParseError::not_implemented("type aliases", self.convert_range(t.range))),
            Stmt::Assign(ast::StmtAssign {
                mut targets,
                value,
                range,
                ..
            }) => {
                // Ruff represents chained assignments (`a = b = 1`) as a single
                // `StmtAssign` with multiple targets. For the common single-target
                // case we produce the existing per-shape nodes so the hot path stays
                // flat; only chained assignments are lowered into `Node::ChainAssign`.
                match targets.len() {
                    0 => Err(ParseError::syntax(
                        "Assignment with no targets".to_string(),
                        self.convert_range(range),
                    )),
                    1 => {
                        let target = targets.pop().expect("len == 1");
                        self.parse_assignment(target, *value)
                    }
                    _ => self.parse_chained_assignment(targets, *value),
                }
            }
            Stmt::AugAssign(ast::StmtAugAssign { target, op, value, .. }) => {
                let op = convert_op(op);
                let value = self.parse_expression(*value)?;
                match *target {
                    AstExpr::Subscript(ast::ExprSubscript {
                        value: object,
                        slice,
                        range,
                        ..
                    }) => Ok(Node::SubscriptOpAssign {
                        target: self.parse_expression(*object)?,
                        index: self.parse_expression(*slice)?,
                        op,
                        value,
                        target_position: self.convert_range(range),
                    }),
                    AstExpr::Attribute(ast::ExprAttribute {
                        value: object,
                        attr,
                        range,
                        ..
                    }) => Ok(Node::AttrOpAssign {
                        object: self.parse_expression(*object)?,
                        attr: EitherStr::Interned(self.interner.intern(attr.id())),
                        op,
                        value,
                        target_position: self.convert_range(range),
                    }),
                    other => Ok(Node::OpAssign {
                        target: self.parse_identifier(other)?,
                        op,
                        value,
                    }),
                }
            }
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
            Stmt::While(ast::StmtWhile { test, body, orelse, .. }) => Ok(Node::While {
                test: self.parse_expression(*test)?,
                body: self.parse_statements(body)?,
                or_else: self.parse_statements(orelse)?,
            }),
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
            Stmt::With(ast::StmtWith {
                is_async,
                items,
                body,
                range,
                ..
            }) => {
                if is_async {
                    return Err(ParseError::not_implemented(
                        "async context managers (async with)",
                        self.convert_range(range),
                    ));
                }
                if items.is_empty() {
                    return Err(ParseError::syntax(
                        "with statement requires at least one context manager",
                        self.convert_range(range),
                    ));
                }
                // Multi-item `with a() as x, b() as y:` is desugared into nested
                // `with` blocks at parse time: the outer `with` runs `a()` and
                // wraps an inner `with` running `b()`, which wraps the user body.
                // CPython evaluates context exprs left-to-right and exits them
                // right-to-left, which is exactly the nested-block semantics —
                // so the lowering is faithful and avoids needing multi-context
                // support in the compiler / VM.
                let position = self.convert_range(range);
                let body = self.parse_statements(body)?;
                let mut parsed_items: Vec<(ExprLoc, Option<UnpackTarget>)> = items
                    .into_iter()
                    .map(|item| -> Result<_, ParseError> {
                        let context = self.parse_expression(item.context_expr)?;
                        let target = match item.optional_vars {
                            Some(expr) => Some(self.parse_unpack_target(*expr)?),
                            None => None,
                        };
                        Ok((context, target))
                    })
                    .collect::<Result<_, _>>()?;
                // Fold from the innermost outward: the last item wraps the user
                // body; each outer item wraps the freshly-built inner `with`.
                //
                // Each synthetic nesting level must be charged against the parser
                // depth budget so a flat `with a, b, c, ...:` statement cannot
                // bypass `MAX_NESTING_DEPTH` and produce an AST that later
                // overflows the host stack during prepare/compile. The budget is
                // restored on success to avoid penalizing sibling statements, in
                // the same pattern used by `parse_elif_else_clauses`.
                let (last_context, last_target) = parsed_items.pop().expect("checked non-empty above");
                let mut node = Node::With {
                    context: last_context,
                    target: last_target,
                    body,
                    position,
                };
                let mut levels: u16 = 0;
                while let Some((context, target)) = parsed_items.pop() {
                    self.decr_depth_remaining(|| range)?;
                    levels += 1;
                    node = Node::With {
                        context,
                        target,
                        body: vec![node],
                        position,
                    };
                }
                self.depth_remaining += levels;
                Ok(node)
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
                let position = self.convert_range(range);
                let import_names = names
                    .iter()
                    .map(|alias_node| {
                        let module_name = self.interner.intern(&alias_node.name);
                        // The binding name is the alias if present, otherwise the module name
                        let binding_name = alias_node
                            .asname
                            .as_ref()
                            .map_or(module_name, |n| self.interner.intern(&n.id));
                        let binding = Identifier::new(binding_name, position);
                        ImportName { module_name, binding }
                    })
                    .collect();
                Ok(Node::Import { names: import_names })
            }
            Stmt::ImportFrom(ast::StmtImportFrom {
                module,
                names,
                level,
                range,
                ..
            }) => {
                let position = self.convert_range(range);
                // Compiler directives, not real imports: they bind nothing and lower
                // to a no-op, so real-world modules import cleanly. Anything Monty
                // does not implement is rejected rather than quietly failing to do
                // what it says. `level == 0` keeps `from .__future__ import x` an
                // ordinary relative import, falling through to the `ImportError`.
                if level == 0 && module.as_ref().is_some_and(|m| m.as_str() == "__future__") {
                    return match names
                        .iter()
                        .find(|a| a.asname.is_some() || !SUPPORTED_FUTURES.contains(&a.name.id.as_str()))
                    {
                        Some(alias) if UNSUPPORTED_FUTURES.contains(&alias.name.id.as_str()) => {
                            Err(ParseError::not_implemented(
                                format!("the '{}' future feature", alias.name.id),
                                self.convert_range(alias.range),
                            ))
                        }
                        // Name checks come first so an aliased unknown feature is
                        // reported as undefined, as CPython does.
                        Some(alias) if !SUPPORTED_FUTURES.contains(&alias.name.id.as_str()) => Err(ParseError::syntax(
                            format!("future feature {} is not defined", alias.name.id),
                            self.convert_range(alias.range),
                        )),
                        // A known feature, so the alias is what selected it. An
                        // alias asks to bind a name and a no-op binds nothing, so
                        // accepting it would fail exactly the way this branch
                        // exists to prevent — the `NameError` would surface later,
                        // far from the import.
                        Some(alias) => Err(ParseError::not_implemented(
                            "aliasing a `__future__` feature",
                            self.convert_range(alias.range),
                        )),
                        None => Ok(Node::Pass),
                    };
                }
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
            Stmt::Break(b) => Ok(Node::Break {
                position: self.convert_range(b.range),
            }),
            Stmt::Continue(c) => Ok(Node::Continue {
                position: self.convert_range(c.range),
            }),
            Stmt::IpyEscapeCommand(i) => Err(ParseError::not_implemented(
                "IPython escape commands",
                self.convert_range(i.range),
            )),
        }
    }

    /// Parses a `def` into a [`RawFunctionDef`].
    ///
    /// Shared by the top-level `Stmt::FunctionDef` arm and by class-body method
    /// parsing in [`parse_class_def`](Self::parse_class_def). Decorators are
    /// rejected here rather than silently ignored — a silently-dropped decorator
    /// changes behaviour without warning, which is unacceptable in a sandbox. The
    /// class-body path rejects decorated methods earlier with a more specific
    /// message, so this only fires for top-level `def`s in practice.
    fn parse_function_def(&mut self, function: ast::StmtFunctionDef) -> Result<RawFunctionDef, ParseError> {
        if !function.decorator_list.is_empty() {
            return Err(ParseError::not_implemented(
                "function decorators",
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
        let is_async = function.is_async;

        Ok(RawFunctionDef {
            name,
            signature,
            body,
            is_async,
        })
    }

    /// Parses a `class Foo: ...` definition into a [`Node::ClassDef`].
    ///
    /// The class body is modelled as a synthetic zero-argument function (like
    /// CPython's class-body code object): the class statements are collected in
    /// source order into a [`RawFunctionDef`] that, when prepared and compiled,
    /// runs in its own scope and returns the assembled `Class`. Methods become
    /// nested `FunctionDef`s; class variables become `Assign`s with arbitrary
    /// expressions (`name = <expr>` / `name: T = <expr>`). Every member name is
    /// recorded in `members`, in source order, for namespace assembly.
    ///
    /// `pass` and `...` are ignored; a leading docstring becomes a `__doc__`
    /// member, and annotated names a stringized `__annotations__`. Class
    /// decorators are supported (enclosing scope, applied bottom-up);
    /// inheritance, function/method decorators, and anything else in the body
    /// are rejected as not-implemented, reserving the syntax for later.
    fn parse_class_def(&mut self, class: ast::StmtClassDef) -> Result<ParseNode, ParseError> {
        let position = self.class_keyword_range(&class);
        // Parsed as ordinary expressions; the compiler emits the apply calls
        // after building the class.
        let decorators = class
            .decorator_list
            .into_iter()
            .map(|d| self.parse_expression(d.expression))
            .collect::<Result<Vec<_>, ParseError>>()?;
        // `class.arguments` carries base classes and metaclass keywords.
        if class
            .arguments
            .is_some_and(|a| !a.args.is_empty() || !a.keywords.is_empty())
        {
            return Err(ParseError::not_implemented(
                "class inheritance and metaclasses",
                position,
            ));
        }

        let name = self.identifier(&class.name.id, class.name.range);
        // The class-body statements (in source order) and the member names they
        // bind. Both methods and class vars are ordinary bindings of the body
        // scope; `members` records the order so the compiler can assemble the
        // namespace dict from the body's locals.
        let mut body = Vec::new();
        let mut members = Vec::new();
        // `(name, "source text")` pairs assembled into `__annotations__` below;
        // always stringized (PEP 563). See `limitations/typing.md`.
        let mut annotations: Vec<DictItem> = Vec::new();

        // CPython stores the class docstring as a real `__doc__` entry in the
        // class dict (`None` when absent), so synthesize a `__doc__ = <docstring
        // or None>` binding as the first class-body statement — `Foo.__doc__` and
        // `obj.__doc__` then work through ordinary namespace lookup. An explicit
        // `__doc__ = ...` later in the body overwrites it, as in CPython.
        let doc_target = Identifier::new(self.interner.intern("__doc__"), self.convert_range(class.name.range));
        let mut doc_value = ExprLoc::new(self.convert_range(class.name.range), Expr::Literal(Literal::None));

        for (i, stmt) in class.body.into_iter().enumerate() {
            match stmt {
                Stmt::FunctionDef(function) => {
                    if !function.decorator_list.is_empty() {
                        return Err(ParseError::not_implemented(
                            "method decorators (classmethod/staticmethod/property)",
                            self.convert_range(function.range),
                        ));
                    }
                    // Parameter defaults evaluate in the class-body scope, so a
                    // walrus target there would become a class member (see
                    // `reject_class_body_walrus`); walrus in the method *body*
                    // binds in the method scope and is fine.
                    for param in function.parameters.iter_non_variadic_params() {
                        if let Some(default) = &param.default {
                            self.reject_class_body_walrus(default)?;
                        }
                    }
                    let method = self.parse_function_def(function)?;
                    members.push(method.name);
                    body.push(Node::FunctionDef(method));
                }
                // `name = <expr>` — a class-level variable.
                Stmt::Assign(ast::StmtAssign {
                    targets, value, range, ..
                }) => {
                    let [
                        AstExpr::Name(ast::ExprName {
                            id, range: name_range, ..
                        }),
                    ] = targets.as_slice()
                    else {
                        return Err(ParseError::not_implemented(
                            "complex class variable targets (only `name = <expr>` is allowed)",
                            self.convert_range(range),
                        ));
                    };
                    let ident = self.identifier(id, *name_range);
                    self.parse_class_var(ident, *value, &mut members, &mut body)?;
                }
                // `name: T [= <expr>]` — an annotated class-level name. The
                // annotation is recorded (stringized) in `__annotations__`; a
                // value additionally makes it a class variable. A bare `name: T`
                // records the annotation but binds no value (matching CPython).
                Stmt::AnnAssign(ast::StmtAnnAssign {
                    target,
                    mut annotation,
                    value,
                    range,
                    ..
                }) => {
                    if let AstExpr::Name(ast::ExprName {
                        id, range: name_range, ..
                    }) = *target
                    {
                        let ident = self.identifier(&id, name_range);
                        annotations.push(self.parse_class_annotation(ident, &mut annotation));
                        if let Some(value) = value {
                            self.parse_class_var(ident, *value, &mut members, &mut body)?;
                        }
                    } else if value.is_some() {
                        // Complex target with a value (`x.y: T = v`) is unsupported;
                        // a bare complex annotation (`x.y: T`) stores nothing in
                        // CPython either, so it is ignored. CPython does still
                        // evaluate the target expression (`undefined.attr: T` raises
                        // `NameError`) where Monty drops it — see
                        // `limitations/typing.md`.
                        return Err(ParseError::not_implemented(
                            "complex class variable targets (only `name = <expr>` is allowed)",
                            self.convert_range(range),
                        ));
                    }
                }
                // `pass` and `...` (the common `class C: ...` stub idiom) are
                // no-ops. A leading string literal is the class docstring and
                // becomes the synthesized `__doc__` value; later bare string
                // literals are no-ops.
                Stmt::Pass(_) => {}
                Stmt::Expr(ast::StmtExpr { value, .. })
                    if matches!(*value, AstExpr::StringLiteral(_) | AstExpr::EllipsisLiteral(_)) =>
                {
                    if i == 0 && matches!(*value, AstExpr::StringLiteral(_)) {
                        doc_value = self.parse_expression(*value)?;
                    }
                }
                other => {
                    return Err(ParseError::not_implemented(
                        "class bodies containing anything other than methods and simple class variables",
                        self.convert_range(other.range()),
                    ));
                }
            }
        }

        // The synthesized `__doc__` binding runs first (like CPython's docstring
        // store); the namespace assembly loads final local values, so an explicit
        // `__doc__` member still wins.
        members.insert(0, doc_target);
        body.insert(
            0,
            Node::Assign {
                target: doc_target,
                object: doc_value,
            },
        );

        // Last statement so the values are assembled after all members exist.
        // Always present (empty dict) to match `Cls.__annotations__ == {}`.
        let annotations_target = Identifier::new(self.interner.intern("__annotations__"), position);
        let body_binds_annotations = members.iter().any(|m| m.name_id == annotations_target.name_id);
        if body_binds_annotations && !annotations.is_empty() {
            // Being last, the synthetic assignment would clobber the body's own
            // binding and lose these entries. CPython stores into whatever the name
            // holds instead — merging into a dict, `TypeError` otherwise.
            return Err(ParseError::not_implemented(
                "assigning `__annotations__` in a class body with annotated names",
                position,
            ));
        }
        // With nothing to store, the body's own binding stands — synthesizing an
        // empty dict over it would break class bodies CPython accepts.
        if !body_binds_annotations {
            members.push(annotations_target);
            body.push(Node::Assign {
                target: annotations_target,
                object: ExprLoc::new(position, Expr::Dict(annotations)),
            });
        }

        // Wrap the body statements in a synthetic zero-arg function. The class
        // name's `name_id` is reused for nicer tracebacks; this function is never
        // registered in any scope (`prepare_class_def` prepares it directly,
        // without binding a function name).
        let body = RawFunctionDef {
            name,
            signature: ParsedSignature::default(),
            body,
            is_async: false,
        };

        Ok(Node::ClassDef {
            name,
            body,
            members,
            decorators,
            position,
        })
    }

    /// The range of a `class` statement from the `class` keyword, excluding
    /// decorators: ruff's `StmtClassDef::range` starts at the first decorator
    /// where CPython starts at the keyword, which would otherwise show decorator
    /// lines in a class-body traceback frame.
    ///
    /// The keyword is located from the lexer's tokens rather than by searching the
    /// source text, so a `class` inside a comment or a decorator's string argument
    /// cannot be mistaken for it. Offsets are ascending, so this class's keyword is
    /// simply the first one at or after its final decorator.
    fn class_keyword_range(&self, class: &ast::StmtClassDef) -> CodeRange {
        let start = match class.decorator_list.last() {
            // Undecorated: ruff's range already starts at the keyword.
            None => class.range.start().into(),
            Some(last_decorator) => {
                let after_decorators = last_decorator.range.end();
                let index = self.class_keyword_offsets.partition_point(|&o| o < after_decorators);
                // Bounded by the name so a malformed lookup cannot borrow a later
                // class's keyword; the fallback is unreachable (a decorated class
                // always has a keyword in this window) but avoids a panic path.
                self.class_keyword_offsets
                    .get(index)
                    .filter(|&&offset| offset < class.name.range.start())
                    .map_or_else(|| class.range.start().into(), |&offset| offset.into())
            }
        };
        CodeRange {
            filename: self.filename_id,
            start_byte: start,
            end_byte: class.range.end().into(),
        }
    }

    /// Builds the `'name': 'annotation'` pair for one annotated class-body name.
    ///
    /// The annotation is stringized rather than evaluated (see
    /// `limitations/typing.md`); [`stringize_annotation`] owns rendering it back
    /// to the text CPython would store.
    fn parse_class_annotation(&mut self, ident: Identifier, annotation: &mut AstExpr) -> DictItem {
        let ann_range = annotation.range();
        let ann_id = self.interner.intern(&stringize_annotation(annotation));
        DictItem::Pair(
            ExprLoc::new(ident.position, Expr::Literal(Literal::Str(ident.name_id))),
            ExprLoc::new(self.convert_range(ann_range), Expr::Literal(Literal::Str(ann_id))),
        )
    }

    /// Parses a class-variable value and records the binding: rejects class-scope
    /// walrus, parses the value expression, and appends the member / `Assign` pair
    /// shared by the `Assign` and `AnnAssign` class-body arms.
    fn parse_class_var(
        &mut self,
        ident: Identifier,
        value: AstExpr,
        members: &mut Vec<Identifier>,
        body: &mut Vec<ParseNode>,
    ) -> Result<(), ParseError> {
        self.reject_class_body_walrus(&value)?;
        let object = self.parse_expression(value)?;
        members.push(ident);
        body.push(Node::Assign { target: ident, object });
        Ok(())
    }

    /// Rejects `:=` that binds in a class-body scope (in class-variable values
    /// and method parameter defaults).
    ///
    /// A walrus target in such an expression binds in the class body, so in
    /// CPython it becomes a class member (`class C: x = (y := 5)` gives `C.y`).
    /// Monty's namespace assembly only records directly-assigned names, so the
    /// binding would be silently dropped — reject the syntax until class-scope
    /// walrus is implemented. A walrus inside a lambda *body* binds in the
    /// lambda's own scope and is allowed (see [`contains_class_scope_walrus`]).
    fn reject_class_body_walrus(&self, expr: &AstExpr) -> Result<(), ParseError> {
        if contains_class_scope_walrus(expr) {
            Err(ParseError::not_implemented(
                "assignment expressions (`:=`) in class bodies",
                self.convert_range(expr.range()),
            ))
        } else {
            Ok(())
        }
    }

    /// `lhs = rhs` — parses a single-target assignment into the appropriate `Node` variant.
    ///
    /// Dispatches on the shape of `lhs` by delegating to `parse_assign_target`, then wraps
    /// the resulting `AssignTarget` together with the parsed RHS into one of the flat
    /// per-shape node variants (`Assign`/`SubscriptAssign`/`AttrAssign`/`UnpackAssign`).
    /// Handles simple assignments (`x = value`), subscript assignments (`dict[key] = value`),
    /// attribute assignments (`obj.attr = value`), and tuple/list unpacking (`a, b = value`).
    fn parse_assignment(&mut self, lhs: AstExpr, rhs: AstExpr) -> Result<ParseNode, ParseError> {
        // Parse the target first so sub-expression evaluation order (container, index, ...)
        // stays consistent with per-shape parsing done before the refactor.
        let target = self.parse_assign_target(lhs)?;
        let rhs = self.parse_expression(rhs)?;
        let node = match target {
            AssignTarget::Name(target) => Node::Assign { target, object: rhs },
            AssignTarget::Subscript {
                target,
                index,
                target_position,
            } => Node::SubscriptAssign {
                target,
                index,
                value: rhs,
                target_position,
            },
            AssignTarget::Attr {
                object,
                attr,
                target_position,
            } => Node::AttrAssign {
                object,
                attr,
                target_position,
                value: rhs,
            },
            AssignTarget::Unpack {
                targets,
                targets_position,
            } => Node::UnpackAssign {
                targets,
                targets_position,
                object: rhs,
            },
        };
        Ok(node)
    }

    /// Parses a chained assignment like `a = b = c = value` into a `Node::ChainAssign`.
    ///
    /// The right-hand side `rhs` is evaluated once, and each entry in `targets` receives
    /// the resulting value in left-to-right order. Each target may be any valid assignment
    /// LHS — a name, subscript, attribute, or unpack pattern — mirroring the shapes handled
    /// by `parse_assignment`.
    fn parse_chained_assignment(&mut self, targets: Vec<AstExpr>, rhs: AstExpr) -> Result<ParseNode, ParseError> {
        let parsed_targets = targets
            .into_iter()
            .map(|t| self.parse_assign_target(t))
            .collect::<Result<Vec<_>, _>>()?;
        let object = self.parse_expression(rhs)?;
        Ok(Node::ChainAssign {
            targets: parsed_targets,
            object,
        })
    }

    /// Parses a single assignment target expression into an `AssignTarget`.
    ///
    /// Central dispatch for assignment-target shapes, shared by `parse_assignment`
    /// (for single-target and annotation-driven assignments) and
    /// `parse_chained_assignment` (for `a = b = value`). Keeping shape dispatch in one
    /// place means adding a new target form only requires updating this function and
    /// its downstream consumers (prepare and compiler).
    fn parse_assign_target(&mut self, lhs: AstExpr) -> Result<AssignTarget, ParseError> {
        match lhs {
            AstExpr::Subscript(ast::ExprSubscript {
                value, slice, range, ..
            }) => Ok(AssignTarget::Subscript {
                target: self.parse_expression(*value)?,
                index: self.parse_expression(*slice)?,
                target_position: self.convert_range(range),
            }),
            AstExpr::Attribute(ast::ExprAttribute { value, attr, range, .. }) => Ok(AssignTarget::Attr {
                object: self.parse_expression(*value)?,
                attr: EitherStr::Interned(self.interner.intern(attr.id())),
                target_position: self.convert_range(range),
            }),
            AstExpr::Tuple(ast::ExprTuple { elts, range, .. }) => {
                let targets_position = self.convert_range(range);
                let targets = elts
                    .into_iter()
                    .map(|e| self.parse_unpack_target(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AssignTarget::Unpack {
                    targets,
                    targets_position,
                })
            }
            AstExpr::List(ast::ExprList { elts, range, .. }) => {
                let targets_position = self.convert_range(range);
                let targets = elts
                    .into_iter()
                    .map(|e| self.parse_unpack_target(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AssignTarget::Unpack {
                    targets,
                    targets_position,
                })
            }
            other => Ok(AssignTarget::Name(self.parse_identifier(other)?)),
        }
    }

    /// Parses an expression from the ruff AST into Monty's ExprLoc representation.
    ///
    /// Includes depth tracking to prevent stack overflow from deeply nested structures.
    /// Matches CPython's limit of 200 for nested parentheses.
    fn parse_expression(&mut self, expression: AstExpr) -> Result<ExprLoc, ParseError> {
        self.decr_depth_remaining(|| expression.range())?;
        let result = self.parse_expression_impl(expression);
        self.depth_remaining += 1;
        result
    }

    fn parse_expression_impl(&mut self, expression: AstExpr) -> Result<ExprLoc, ParseError> {
        match expression {
            AstExpr::BoolOp(ast::ExprBoolOp { op, values, range, .. }) => {
                // Handle chained boolean operations like `a and b and c` by right-folding
                // into nested binary operations: `a and (b and c)`.
                //
                // Ruff hands the operands over as a flat `Vec`, but the fold
                // produces a right-nested `Expr::Op` tree that the prepare and
                // compile phases walk recursively. Count each fold step against
                // the same depth budget that bounds explicitly nested source so
                // a long flat chain cannot overflow the host's native stack
                // downstream. The budget is restored once the fold completes.
                let rust_op = convert_bool_op(op);
                let position = self.convert_range(range);
                let mut values_iter = values.into_iter().rev();

                // Start with the rightmost value
                let last_value = values_iter.next().expect("Expected at least one value in boolean op");
                let mut result = self.parse_expression(last_value)?;

                // Fold from right to left
                let mut levels: u16 = 0;
                for value in values_iter {
                    self.decr_depth_remaining(|| value.range())?;
                    levels += 1;
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
                self.depth_remaining += levels;
                Ok(result)
            }
            AstExpr::Named(ast::ExprNamed {
                target, value, range, ..
            }) => {
                let target_ident = self.parse_identifier(*target)?;
                let value_expr = self.parse_expression(*value)?;
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Named {
                        target: target_ident,
                        value: Box::new(value_expr),
                    },
                ))
            }
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
            AstExpr::Lambda(ast::ExprLambda {
                parameters,
                body,
                range,
                ..
            }) => {
                let position = self.convert_range(range);

                // Intern the lambda name
                let name_id = self.interner.intern("<lambda>");

                // Parse lambda parameters (similar to function parameters)
                let signature = if let Some(params) = parameters {
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

                    ParsedSignature {
                        pos_args,
                        args,
                        var_args,
                        kwargs,
                        var_kwargs,
                    }
                } else {
                    // No parameters (e.g., `lambda: 42`)
                    ParsedSignature::default()
                };

                // Parse the body expression
                let body = Box::new(self.parse_expression(*body)?);

                Ok(ExprLoc::new(
                    position,
                    Expr::LambdaRaw {
                        name_id,
                        signature,
                        body,
                    },
                ))
            }
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
                let mut dict_items = Vec::new();
                for ast::DictItem { key, value } in items {
                    // key is Option<Expr> - None represents ** unpacking (PEP 448)
                    if let Some(key_expr_ast) = key {
                        let key_expr = self.parse_expression(key_expr_ast)?;
                        let value_expr = self.parse_expression(value)?;
                        dict_items.push(DictItem::Pair(key_expr, value_expr));
                    } else {
                        // **expr unpack in a dict literal: later keys silently win
                        let unpack_expr = self.parse_expression(value)?;
                        dict_items.push(DictItem::Unpack(unpack_expr));
                    }
                }
                Ok(ExprLoc::new(position, Expr::Dict(dict_items)))
            }
            AstExpr::Set(ast::ExprSet { elts, range, .. }) => {
                let mut items = Vec::new();
                for e in elts {
                    items.push(self.parse_sequence_item(e)?);
                }
                Ok(ExprLoc::new(self.convert_range(range), Expr::Set(items)))
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
                // Ruff models the key as `Option<Box<Expr>>` to represent the
                // invalid `{**v for ...}` form during error recovery. Real Python
                // forbids dict unpacking in a comprehension, so ruff also emits a
                // syntax error for that case; treat `None` here as the same syntax
                // error to keep behavior consistent if it ever leaks through.
                let key = key.ok_or_else(|| {
                    ParseError::syntax(
                        "dict unpacking is not allowed in dict comprehension".to_string(),
                        self.convert_range(range),
                    )
                })?;
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
            AstExpr::Await(a) => {
                let value = self.parse_expression(*a.value)?;
                Ok(ExprLoc::new(self.convert_range(a.range), Expr::Await(Box::new(value))))
            }
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
            }) => {
                let position = self.convert_range(range);
                let ops_vec = ops.into_vec();
                let comparators_vec = comparators.into_vec();

                // Simple case: single comparison (most common)
                if ops_vec.len() == 1 {
                    return Ok(ExprLoc::new(
                        position,
                        Expr::CmpOp {
                            left: Box::new(self.parse_expression(*left)?),
                            op: convert_compare_op(ops_vec.into_iter().next().unwrap()),
                            right: Box::new(self.parse_expression(comparators_vec.into_iter().next().unwrap())?),
                        },
                    ));
                }

                // Chain comparison: transform to nested And expressions
                self.parse_chain_comparison(*left, ops_vec, comparators_vec, position)
            }
            AstExpr::Call(ast::ExprCall {
                func, arguments, range, ..
            }) => {
                let position = self.convert_range(range);
                let ast::Arguments { args, keywords, .. } = arguments;
                let args_vec = args.into_vec();
                let keywords_vec: Vec<_> = keywords.into_iter().collect();

                // Detect whether we need the generalized path (PEP 448):
                // - multiple *args unpacks, OR
                // - positional argument after *args, OR
                // - multiple **kwargs unpacks
                let needs_generalized = Self::needs_generalized_call(&args_vec, &keywords_vec);

                let args = if needs_generalized {
                    self.parse_generalized_call_args(args_vec, keywords_vec)?
                } else {
                    self.parse_simple_call_args(args_vec, keywords_vec)?
                };
                match *func {
                    AstExpr::Name(ast::ExprName { id, range, .. }) => {
                        // Always create Callable::Name — builtin resolution happens in
                        // the prepare phase with scope awareness, so local assignments
                        // can shadow builtins.
                        let ident = self.identifier(&id, range);
                        let callable = Callable::Name(ident);
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
                                attr: EitherStr::Interned(self.interner.intern(attr.id())),
                                args: Box::new(args),
                            },
                        ))
                    }
                    other => {
                        // Handle arbitrary expression as callable (e.g., lambda calls)
                        let callable = Box::new(self.parse_expression(other)?);
                        Ok(ExprLoc::new(
                            position,
                            Expr::IndirectCall {
                                callable,
                                args: Box::new(args),
                            },
                        ))
                    }
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
                    Number::Int(i) => {
                        if let Some(i) = i.as_i64() {
                            Literal::Int(i)
                        } else {
                            // Integer too large for i64, parse string representation as BigInt.
                            // Handles radix prefixes (0x, 0o, 0b) and underscores.
                            let bi = parse_int_literal(&i.to_string(), position)?;
                            let long_int_id = self.interner.intern_long_int(bi);
                            Literal::LongInt(long_int_id)
                        }
                    }
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
                        attr: EitherStr::Interned(self.interner.intern(attr.id())),
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
                let position = self.convert_range(range);
                // Always create Expr::Name — builtin resolution happens in the prepare
                // phase with scope awareness, so local assignments can shadow builtins.
                let expr = Expr::Name(self.identifier(&id, range));
                Ok(ExprLoc::new(position, expr))
            }
            AstExpr::List(ast::ExprList { elts, range, .. }) => {
                let mut items = Vec::new();
                for e in elts {
                    items.push(self.parse_sequence_item(e)?);
                }
                Ok(ExprLoc::new(self.convert_range(range), Expr::List(items)))
            }
            AstExpr::Tuple(ast::ExprTuple { elts, range, .. }) => {
                let mut items = Vec::new();
                for e in elts {
                    items.push(self.parse_sequence_item(e)?);
                }
                Ok(ExprLoc::new(self.convert_range(range), Expr::Tuple(items)))
            }
            AstExpr::Slice(ast::ExprSlice {
                lower,
                upper,
                step,
                range,
                ..
            }) => {
                let lower = lower.map(|e| self.parse_expression(*e)).transpose()?;
                let upper = upper.map(|e| self.parse_expression(*e)).transpose()?;
                let step = step.map(|e| self.parse_expression(*e)).transpose()?;
                Ok(ExprLoc::new(
                    self.convert_range(range),
                    Expr::Slice {
                        lower: lower.map(Box::new),
                        upper: upper.map(Box::new),
                        step: step.map(Box::new),
                    },
                ))
            }
            AstExpr::IpyEscapeCommand(i) => Err(ParseError::not_implemented(
                "IPython escape commands",
                self.convert_range(i.range),
            )),
        }
    }

    /// Converts an AST expression into a `SequenceItem` for list/tuple/set literals.
    ///
    /// A `Starred` node becomes `SequenceItem::Unpack`; all other expressions
    /// become `SequenceItem::Value`. This is the entry point for PEP 448 unpack
    /// handling in collection literals.
    fn parse_sequence_item(&mut self, expr: AstExpr) -> Result<SequenceItem, ParseError> {
        if let AstExpr::Starred(ast::ExprStarred { value, .. }) = expr {
            Ok(SequenceItem::Unpack(self.parse_expression(*value)?))
        } else {
            Ok(SequenceItem::Value(self.parse_expression(expr)?))
        }
    }

    /// Detects whether a function call needs the generalized `GeneralizedCall` path.
    ///
    /// Returns `true` when the call has:
    /// - More than one `*unpack` among positional args, OR
    /// - A plain positional arg following a `*unpack`, OR
    /// - More than one `**unpack` among keyword args.
    ///
    /// In all these cases the simple `ArgsKargs` representation is insufficient
    /// and `parse_generalized_call_args` must be used instead.
    fn needs_generalized_call(args: &[AstExpr], keywords: &[Keyword]) -> bool {
        let mut seen_star = false;
        for arg in args {
            match arg {
                AstExpr::Starred(_) => {
                    if seen_star {
                        return true; // second *unpack
                    }
                    seen_star = true;
                }
                _ => {
                    if seen_star {
                        return true; // positional after *unpack
                    }
                }
            }
        }
        // Multiple **kwargs unpacks?
        keywords.iter().filter(|k| k.arg.is_none()).count() > 1
    }

    /// Parses function call args for the simple case (at most one * and one **).
    ///
    /// Returns `ArgExprs::new_with_var_kwargs(...)` as before, preserving the
    /// fast path for the vast majority of function calls.
    fn parse_simple_call_args(
        &mut self,
        args_vec: Vec<AstExpr>,
        keywords_vec: Vec<Keyword>,
    ) -> Result<ArgExprs, ParseError> {
        let mut positional_args = Vec::new();
        let mut var_args_expr: Option<ExprLoc> = None;

        for arg_expr in args_vec {
            match arg_expr {
                AstExpr::Starred(ast::ExprStarred { value, .. }) => {
                    var_args_expr = Some(self.parse_expression(*value)?);
                }
                other => {
                    positional_args.push(self.parse_expression(other)?);
                }
            }
        }
        let (kwargs, var_kwargs) = self.parse_keywords(keywords_vec)?;
        Ok(ArgExprs::new_with_var_kwargs(
            positional_args,
            var_args_expr,
            kwargs,
            var_kwargs,
        ))
    }

    /// Parses function call args for the PEP 448 generalized case.
    ///
    /// Builds `Vec<CallArg>` and `Vec<CallKwarg>` preserving the full order of
    /// positional and keyword arguments so the compiler can emit correct
    /// `ListAppend`/`ListExtend`/`DictMerge` sequences.
    fn parse_generalized_call_args(
        &mut self,
        args_vec: Vec<AstExpr>,
        keywords_vec: Vec<Keyword>,
    ) -> Result<ArgExprs, ParseError> {
        let mut call_args = Vec::new();
        for arg_expr in args_vec {
            match arg_expr {
                AstExpr::Starred(ast::ExprStarred { value, .. }) => {
                    call_args.push(CallArg::Unpack(self.parse_expression(*value)?));
                }
                other => {
                    call_args.push(CallArg::Value(self.parse_expression(other)?));
                }
            }
        }

        let mut call_kwargs = Vec::new();
        for kwarg in keywords_vec {
            if let Some(key) = kwarg.arg {
                let key_ident = self.identifier(&key.id, key.range);
                let value = self.parse_expression(kwarg.value)?;
                call_kwargs.push(CallKwarg::Named(Kwarg { key: key_ident, value }));
            } else {
                let unpack_expr = self.parse_expression(kwarg.value)?;
                call_kwargs.push(CallKwarg::Unpack(unpack_expr));
            }
        }

        Ok(ArgExprs::new_generalized(call_args, call_kwargs))
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
                format!("Expected name, got {}", describe_expr_kind(&other)),
                self.convert_range(other.range()),
            )),
        }
    }

    /// Parses a chain comparison expression like `a < b < c < d`.
    ///
    /// Chain comparisons evaluate each intermediate value only once and short-circuit
    /// on the first false result. This creates an `Expr::ChainCmp` node which is
    /// compiled to bytecode using stack manipulation (Dup, Rot) rather than
    /// temporary variables, avoiding namespace pollution.
    fn parse_chain_comparison(
        &mut self,
        left: AstExpr,
        ops: Vec<CmpOp>,
        comparators: Vec<AstExpr>,
        position: CodeRange,
    ) -> Result<ExprLoc, ParseError> {
        let left_expr = self.parse_expression(left)?;
        let comparisons = ops
            .into_iter()
            .zip(comparators)
            .map(|(op, cmp)| Ok((convert_compare_op(op), self.parse_expression(cmp)?)))
            .collect::<Result<Vec<_>, ParseError>>()?;

        Ok(ExprLoc::new(
            position,
            Expr::ChainCmp {
                left: Box::new(left_expr),
                comparisons,
            },
        ))
    }

    /// Parses an unpack target - either a single identifier or a nested tuple.
    ///
    /// Handles patterns like `a` (single variable), `a, b` (flat tuple), or `(a, b), c` (nested).
    /// Includes depth tracking to prevent stack overflow from deeply nested structures.
    fn parse_unpack_target(&mut self, ast: AstExpr) -> Result<UnpackTarget, ParseError> {
        self.decr_depth_remaining(|| ast.range())?;
        let result = self.parse_unpack_target_impl(ast);
        self.depth_remaining += 1;
        result
    }

    fn parse_unpack_target_impl(&mut self, ast: AstExpr) -> Result<UnpackTarget, ParseError> {
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
                // Validate at most one starred target
                let starred_count = targets.iter().filter(|t| matches!(t, UnpackTarget::Starred(_))).count();
                if starred_count > 1 {
                    return Err(ParseError::syntax(
                        "multiple starred expressions in assignment",
                        position,
                    ));
                }
                Ok(UnpackTarget::Tuple { targets, position })
            }
            AstExpr::Starred(ast::ExprStarred { value, range, .. }) => {
                // Starred target must be a simple name
                match *value {
                    AstExpr::Name(ast::ExprName { id, range, .. }) => {
                        Ok(UnpackTarget::Starred(self.identifier(&id, range)))
                    }
                    _ => Err(ParseError::syntax(
                        "starred assignment target must be a name",
                        self.convert_range(range),
                    )),
                }
            }
            AstExpr::List(ast::ExprList { elts, range, .. }) => {
                // List unpacking target [a, b, *rest] - same as tuple
                let position = self.convert_range(range);
                let targets = elts
                    .into_iter()
                    .map(|e| self.parse_unpack_target(e))
                    .collect::<Result<Vec<_>, _>>()?;
                if targets.is_empty() {
                    return Err(ParseError::syntax("empty list in unpack target", position));
                }
                // Validate at most one starred target
                let starred_count = targets.iter().filter(|t| matches!(t, UnpackTarget::Starred(_))).count();
                if starred_count > 1 {
                    return Err(ParseError::syntax(
                        "multiple starred expressions in assignment",
                        position,
                    ));
                }
                Ok(UnpackTarget::Tuple { targets, position })
            }
            other => Err(ParseError::syntax(
                format!("invalid unpacking target: {}", describe_expr_kind(&other)),
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
                let format_spec = match &interp.format_spec {
                    Some(spec) => self.parse_format_spec(spec)?,
                    None => None,
                };
                let mut conversion = convert_conversion_flag(interp.conversion);
                // An explicit empty spec (`f"{x=:}"`) collapses to `None` in
                // `parse_format_spec`, but — unlike the bare debug form
                // (`f"{x=}"`), which defaults to `repr` — it must format with
                // `str`. Mark the conversion `Str` so the compiler's repr
                // default for debug forms is suppressed; this is exact because
                // `format(x, "")` equals `str(x)` for builtins (the same
                // equivalence the empty-spec collapse already relies on).
                if interp.debug_text.is_some()
                    && matches!(conversion, ConversionFlag::None)
                    && interp.format_spec.is_some()
                    && format_spec.is_none()
                {
                    conversion = ConversionFlag::Str;
                }
                // Extract debug prefix for `=` specifier (e.g., f'{a=}' -> "a=")
                let debug_prefix = interp.debug_text.as_ref().map(|dt| {
                    let expr_text = &self.code[interp.expression.range()];
                    self.interner
                        .intern(&format!("{}{}{}", dt.leading(), expr_text, dt.trailing()))
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
    /// Specs with no interpolations take the fast path: their literal text is
    /// concatenated, parsed, and bit-packed into a single `u64` carried inside
    /// `FormatSpec::Static`. The compiler then drops this straight into the
    /// constant pool with no further work, and no per-segment interning is
    /// performed (the parsed spec is the only thing the bytecode needs).
    ///
    /// Two cases force a `FormatSpec::Dynamic`:
    /// 1. Any nested interpolation (e.g. `f"{x:{width}}"`) — the spec must be
    ///    materialized at runtime.
    /// 2. Valid but extreme specs whose width or precision exceed the compact
    ///    encoding (e.g. `f"{x:>1048576}"`). The concatenated literal text is
    ///    interned and emitted as a single-literal dynamic spec so the VM
    ///    re-parses it at runtime.
    fn parse_format_spec(
        &mut self,
        spec: &ast::InterpolatedStringFormatSpec,
    ) -> Result<Option<FormatSpec>, ParseError> {
        let has_interpolation = spec
            .elements
            .iter()
            .any(|e| matches!(e, InterpolatedStringElement::Interpolation(_)));

        if has_interpolation {
            let mut parts = Vec::with_capacity(spec.elements.len());
            for element in &spec.elements {
                match element {
                    InterpolatedStringElement::Literal(lit) => {
                        let string_id = self.interner.intern(&lit.value);
                        parts.push(FStringPart::Literal(string_id));
                    }
                    InterpolatedStringElement::Interpolation(interp) => {
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
            Ok(Some(FormatSpec::Dynamic(parts)))
        } else {
            let static_spec: String = spec
                .elements
                .iter()
                .filter_map(|e| match e {
                    InterpolatedStringElement::Literal(lit) => Some(&*lit.value),
                    InterpolatedStringElement::Interpolation(_) => None,
                })
                .collect();
            // An empty spec (`f"{x:}"`) is identical to no spec (`f"{x}"`) for
            // every builtin type — `format(x, "")` is `str(x)`. Emit no spec so
            // the value takes the plain `str()` path rather than the default
            // formatter, which diverges for some types (e.g. a bare float would
            // otherwise go through `g`: `f"{1234567.0:}"` must be `"1234567.0"`,
            // not `"1.23457e+06"`; a bool must be `"True"`, not `"1"`).
            if static_spec.is_empty() {
                return Ok(None);
            }
            match static_spec.parse::<ParsedFormatSpec>() {
                Ok(parsed) => {
                    if let Some(encoded) = encode_format_spec(&parsed) {
                        Ok(Some(FormatSpec::Static(encoded)))
                    } else {
                        // Valid but too large for the compact encoding — re-parse
                        // the literal at runtime.
                        let string_id = self.interner.intern(&static_spec);
                        Ok(Some(FormatSpec::Dynamic(vec![FStringPart::Literal(string_id)])))
                    }
                }
                // Two kinds of failing spec are deferred to the dynamic
                // (runtime) path rather than rejected here:
                //  - one containing `%`, which may be a `strftime` string for a
                //    date/datetime value (only resolvable once the type is known);
                //  - one whose error CPython raises as a *runtime* `ValueError`
                //    with type-dependent or format-time wording (`Unknown format
                //    code`, grouping conflicts, missing precision) — see
                //    [`ParseFormatSpecError::defer_to_runtime`]. The VM re-parses
                //    the literal and raises the matching error.
                // Genuinely-malformed specs and `usize` overflow still fail at
                // compile time.
                Err(err) if static_spec.contains('%') || err.defer_to_runtime() => {
                    let string_id = self.interner.intern(&static_spec);
                    Ok(Some(FormatSpec::Dynamic(vec![FStringPart::Literal(string_id)])))
                }
                Err(err) => Err(ParseError::syntax(err.to_string(), self.convert_range(spec.range))),
            }
        }
    }

    fn convert_range(&self, range: TextRange) -> CodeRange {
        code_range(self.filename_id, range)
    }

    /// Decrements the depth remaining for nested parentheses.
    /// Returns an error if the depth remaining goes to zero.
    fn decr_depth_remaining(&mut self, get_range: impl FnOnce() -> TextRange) -> Result<(), ParseError> {
        if let Some(depth_remaining) = self.depth_remaining.checked_sub(1) {
            self.depth_remaining = depth_remaining;
            Ok(())
        } else {
            let position = self.convert_range(get_range());
            Err(ParseError::syntax("Source is too deeply nested", position))
        }
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

/// Does `expr` contain a `:=` that binds in the enclosing (class-body) scope?
///
/// Like ruff's `any_over_expr`, but scope-aware for lambdas: a walrus inside a
/// lambda *body* binds in the lambda's own scope (legal CPython, e.g.
/// `class C: f = lambda: (z := 1)`) and is skipped, while lambda parameter
/// *defaults* evaluate in the enclosing scope and are still searched.
/// Comprehensions ARE descended into: CPython also rejects an assignment
/// expression within a comprehension in a class body (as a `SyntaxError`).
fn contains_class_scope_walrus(expr: &AstExpr) -> bool {
    /// Expression visitor that records whether a class-scope-binding walrus
    /// was seen, pruning lambda bodies from the walk.
    struct Finder {
        found: bool,
    }
    impl<'a> Visitor<'a> for Finder {
        fn visit_expr(&mut self, expr: &'a AstExpr) {
            match expr {
                _ if self.found => {}
                AstExpr::Named(_) => self.found = true,
                AstExpr::Lambda(lambda) => {
                    // Only the parameter defaults evaluate in the enclosing scope.
                    for param in lambda.parameters.iter().flat_map(|p| p.iter_non_variadic_params()) {
                        if let Some(default) = param.default.as_deref() {
                            self.visit_expr(default);
                        }
                    }
                }
                _ => walk_expr(self, expr),
            }
        }
    }

    let mut finder = Finder { found: false };
    finder.visit_expr(expr);
    finder.found
}

/// Short human-readable name for an `AstExpr` variant, for use in
/// user-facing parse errors. Avoids the Rust `Debug` formatting of the
/// node, which would leak internal field names, ranges, and struct
/// layout of `ruff_python_ast` into the error message.
fn describe_expr_kind(expr: &AstExpr) -> &'static str {
    match expr {
        AstExpr::Name(_) => "name",
        AstExpr::Starred(_) => "starred expression",
        AstExpr::Attribute(_) => "attribute",
        AstExpr::Subscript(_) => "subscript",
        AstExpr::Call(_) => "function call",
        AstExpr::Tuple(_) => "tuple",
        AstExpr::List(_) => "list",
        AstExpr::Set(_) => "set",
        AstExpr::Dict(_) => "dict",
        AstExpr::NumberLiteral(_) => "number literal",
        AstExpr::StringLiteral(_) => "string literal",
        AstExpr::BytesLiteral(_) => "bytes literal",
        AstExpr::BooleanLiteral(_) => "boolean literal",
        AstExpr::NoneLiteral(_) => "None",
        AstExpr::EllipsisLiteral(_) => "...",
        AstExpr::FString(_) => "f-string",
        AstExpr::TString(_) => "t-string",
        AstExpr::Lambda(_) => "lambda",
        AstExpr::If(_) => "conditional expression",
        AstExpr::BoolOp(_) => "boolean expression",
        AstExpr::BinOp(_) => "binary expression",
        AstExpr::UnaryOp(_) => "unary expression",
        AstExpr::Compare(_) => "comparison",
        AstExpr::Named(_) => "named expression",
        AstExpr::Yield(_) => "yield expression",
        AstExpr::YieldFrom(_) => "yield from expression",
        AstExpr::Await(_) => "await expression",
        AstExpr::ListComp(_) => "list comprehension",
        AstExpr::SetComp(_) => "set comprehension",
        AstExpr::DictComp(_) => "dict comprehension",
        AstExpr::Generator(_) => "generator expression",
        AstExpr::Slice(_) => "slice",
        AstExpr::IpyEscapeCommand(_) => "IPython escape command",
    }
}

/// Source code location for a parsed node, stored as raw byte offsets.
///
/// `CodeRange` is written by the parser for every AST node and must therefore
/// be cheap to construct. Storing just byte offsets (matching ruff's native
/// `TextRange` representation) means producing a `CodeRange` is a single
/// struct assignment — no line/column resolution, no UTF-8 char iteration,
/// no line-index lookup.
///
/// When a diagnostic (traceback, syntax error) actually needs human-readable
/// line/column positions or a source preview line, a [`SourceMap`] is built
/// over the source text once at the diagnostic boundary and used to resolve
/// byte offsets lazily. This keeps the parse hot path O(1) per node while
/// preserving exact CPython-compatible column semantics (`chars().count()`
/// on the relevant line only) at diagnostic time.
#[derive(Clone, Copy, Default, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CodeRange {
    /// Interned filename ID - look up in Interns to get the actual string.
    pub filename: StringId,
    /// Byte offset of the range start within the source text.
    pub start_byte: u32,
    /// Byte offset of the range end (exclusive) within the source text.
    pub end_byte: u32,
}

/// Custom Debug implementation to keep AST-printing output compact.
impl fmt::Debug for CodeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CodeRange{{filename: {:?}, start_byte: {}, end_byte: {}}}",
            self.filename, self.start_byte, self.end_byte
        )
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
    pub(crate) fn not_implemented(msg: impl Into<Cow<'static, str>>, position: CodeRange) -> Self {
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
        let mut source_map = SourceMap::new(source);
        match self {
            Self::Syntax { msg, position } => MontyException::with_traceback(
                ExcType::SyntaxError,
                Some(msg.into_owned()),
                vec![StackFrame::from_position_syntax_error(
                    position,
                    filename,
                    &mut source_map,
                )],
            ),
            Self::NotImplemented { msg, position } => MontyException::with_traceback(
                ExcType::NotImplementedError,
                Some(format!("The monty syntax parser does not yet support {msg}")),
                vec![StackFrame::from_position(position, filename, &mut source_map)],
            ),
            Self::NotSupported { msg, position } => MontyException::with_traceback(
                ExcType::NotImplementedError,
                Some(msg.into_owned()),
                vec![StackFrame::from_position(position, filename, &mut source_map)],
            ),
            Self::Import { msg, position } => MontyException::with_traceback(
                ExcType::ImportError,
                Some(msg.into_owned()),
                vec![StackFrame::from_position_no_caret(position, filename, &mut source_map)],
            ),
        }
    }
}

/// Parses an integer literal string into a `BigInt`, handling radix prefixes and underscores.
///
/// Supports Python integer literal formats:
/// - Decimal: `123`, `1_000_000`
/// - Hexadecimal: `0x1a2b`, `0X1A2B`
/// - Octal: `0o777`, `0O777`
/// - Binary: `0b1010`, `0B1010`
///
/// Check digit limit before the expensive O(n^2) decimal BigInt parse.
/// Only decimal is limited — hex/octal/binary use O(n) algorithms and are handled above.
///
/// Returns `ParseError` if the string cannot be parsed.
fn parse_int_literal(s: &str, position: CodeRange) -> Result<BigInt, ParseError> {
    // Remove underscores (Python allows them as digit separators)
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    let cleaned = cleaned.as_str();

    // Detect radix from prefix
    if cleaned.len() >= 2 {
        let prefix = &cleaned[..2];
        let digits = &cleaned[2..];

        let from_radix = |radix: u32| -> Result<BigInt, ParseError> {
            BigInt::from_str_radix(digits, radix)
                .map_err(|e| ParseError::syntax(format!("invalid integer literal: {s:?}, error: {e}"), position))
        };

        match prefix.to_ascii_lowercase().as_str() {
            "0x" => return from_radix(16),
            "0o" => return from_radix(8),
            "0b" => return from_radix(2),
            _ => {}
        }
    }

    // Default to decimal
    let digit_count = cleaned.bytes().filter(u8::is_ascii_digit).count();
    if digit_count > INT_MAX_STR_DIGITS {
        Err(ParseError::syntax(
            format!(
                "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: \
                 value has {digit_count} digits; consider hexadecimal for large integer literals"
            ),
            position,
        ))
    } else {
        cleaned
            .parse::<BigInt>()
            .map_err(|e| ParseError::syntax(format!("invalid integer literal {s:?}, error: {e}"), position))
    }
}
