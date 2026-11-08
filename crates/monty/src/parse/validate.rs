//! Compiler checks that must run before lowering or stringizing annotations.

use std::{borrow::Cow, mem};

use ahash::{AHashMap, AHashSet};
use ruff_python_ast::{
    self as ast, Expr, ExprContext, Pattern, Stmt,
    token::TokenKind,
    visitor::{self, Visitor},
};
use ruff_python_parser::{Parsed, UnsupportedSyntaxErrorKind};
use ruff_text_size::{Ranged, TextRange};

use super::{MAX_NESTING_DEPTH, ParseError, code_range};
use crate::intern::StringId;

mod names;
use names::{Names, SymbolId, SymbolName};

pub(super) fn module(
    parsed: &Parsed<ast::ModModule>,
    filename: StringId,
    future_annotations: bool,
) -> Result<(), ParseError> {
    reject_parenthesized_keyword_names(parsed, filename)?;
    // Binding history is only needed for declaration order and nonlocal lookup.
    let track_bindings = parsed
        .tokens()
        .iter()
        .any(|token| matches!(token.kind(), TokenKind::Global | TokenKind::Nonlocal));
    let mut validator = Validator::new(filename, future_annotations, track_bindings);
    validator.visit_body(&parsed.syntax().body);
    validator.finish()
}

pub(super) fn expression(parsed: &Parsed<ast::ModExpression>, filename: StringId) -> Result<(), ParseError> {
    reject_parenthesized_keyword_names(parsed, filename)?;
    let mut validator = Validator::new(filename, false, false);
    validator.visit_expr(&parsed.syntax().body);
    validator.finish()
}

/// Bounds raw annotation walks, including format-spec recursion without Expr nodes.
pub(super) fn expression_within_depth(expr: &Expr, remaining: u16) -> bool {
    struct DepthCheck {
        remaining: u16,
        exceeded: bool,
    }
    impl DepthCheck {
        fn enter(&mut self) -> bool {
            if self.exceeded {
                false
            } else if let Some(remaining) = self.remaining.checked_sub(1) {
                self.remaining = remaining;
                true
            } else {
                self.exceeded = true;
                false
            }
        }
    }
    impl<'a> Visitor<'a> for DepthCheck {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.enter() {
                visitor::walk_expr(self, expr);
                self.remaining += 1;
            }
        }
        fn visit_interpolated_string_element(&mut self, element: &'a ast::InterpolatedStringElement) {
            if self.enter() {
                visitor::walk_interpolated_string_element(self, element);
                self.remaining += 1;
            }
        }
    }
    let mut check = DepthCheck {
        remaining,
        exceeded: false,
    };
    check.visit_expr(expr);
    !check.exceeded
}

/// Ruff reports obsolete keyword syntax separately from parse errors. Check it
/// before annotation stringization discards the original parentheses.
fn reject_parenthesized_keyword_names<T>(parsed: &Parsed<T>, filename: StringId) -> Result<(), ParseError> {
    // Other version diagnostics describe valid newer syntax that Monty may support.
    let Some(error) = parsed
        .unsupported_syntax_errors()
        .iter()
        .find(|error| error.kind == UnsupportedSyntaxErrorKind::ParenthesizedKeywordArgumentName)
    else {
        return Ok(());
    };

    Err(ParseError::syntax(
        "expression cannot contain assignment, perhaps you meant \"==\"?",
        code_range(filename, error.range()),
    ))
}

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Module,
    Function { is_async: bool },
    Class,
    TypeParameter,
    GenericDefinition,
}

struct Scope {
    kind: ScopeKind,
    parent: Option<usize>,
    comprehension_depth: usize,
    comprehension_body_depth: usize,
    iteration_names: AHashMap<SymbolId, usize>,
    names: AHashMap<SymbolId, NameState>,
    async_context: AsyncContext,
}

#[derive(Clone, Copy)]
struct AsyncContext {
    allowed: bool,
    comprehension: bool,
}

#[derive(Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent symbol-table facts can hold simultaneously"
)]
struct NameState {
    bound: bool,
    assigned: bool,
    used: bool,
    parameter: bool,
    annotated: bool,
    global: bool,
    nonlocal: bool,
    type_parameter: bool,
}

struct Nonlocal<'a> {
    name: SymbolName<'a>,
    key: SymbolId,
    range: TextRange,
    scope: usize,
}

impl Scope {
    fn new(kind: ScopeKind) -> Self {
        Self {
            kind,
            parent: None,
            comprehension_depth: 0,
            comprehension_body_depth: 0,
            iteration_names: AHashMap::new(),
            names: AHashMap::new(),
            async_context: AsyncContext {
                allowed: matches!(kind, ScopeKind::Module | ScopeKind::Function { is_async: true }),
                comprehension: false,
            },
        }
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent syntax contexts can hold simultaneously"
)]
struct Validator<'a> {
    filename: StringId,
    error: Option<ParseError>,
    depth: u16,
    scopes: Vec<Scope>,
    current_scope: usize,
    names: Names<'a>,
    nonlocals: Vec<Nonlocal<'a>>,
    record_bindings: bool,
    track_bindings: bool,
    // CPython forbids a walrus anywhere inside an iterable clause, even in a lambda body.
    iterable_depth: usize,
    future_annotations: bool,
    skipped_codegen_checks: usize,
    annotation_depth: usize,
    format_depth: usize,
    template_string: bool,
}

impl<'a> Validator<'a> {
    fn new(filename: StringId, future_annotations: bool, track_bindings: bool) -> Self {
        Self {
            filename,
            error: None,
            depth: 0,
            scopes: vec![Scope::new(ScopeKind::Module)],
            current_scope: 0,
            names: Names::new(),
            nonlocals: Vec::new(),
            record_bindings: true,
            track_bindings,
            iterable_depth: 0,
            future_annotations,
            skipped_codegen_checks: 0,
            annotation_depth: 0,
            format_depth: 0,
            template_string: false,
        }
    }

    fn finish(self) -> Result<(), ParseError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        // Later local assignments can shadow an enclosing type parameter, so resolve after visiting each scope.
        for declaration in self.nonlocals {
            let mut scope_id = Some(declaration.scope);
            let mut class_shadow = false;
            while let Some(id) = scope_id {
                let scope = &self.scopes[id];
                let name = scope.names.get(&declaration.key).copied().unwrap_or_default();
                if id != declaration.scope {
                    if matches!(scope.kind, ScopeKind::Function { .. }) && name.global {
                        return Err(ParseError::syntax(
                            format!("no binding for nonlocal '{}' found", declaration.name),
                            code_range(self.filename, declaration.range),
                        ));
                    }
                    if (name.bound || name.parameter) && !name.global && !name.nonlocal {
                        if matches!(scope.kind, ScopeKind::Function { .. }) {
                            break;
                        }
                        // Class locals hide type parameters from descendants, but are not closure bindings.
                        class_shadow |= scope.kind == ScopeKind::Class;
                    }
                }
                if name.type_parameter {
                    if class_shadow {
                        break;
                    }
                    return Err(ParseError::syntax(
                        format!("nonlocal binding not allowed for type parameter '{}'", declaration.name),
                        code_range(self.filename, declaration.range),
                    ));
                }
                scope_id = scope.parent;
            }
        }
        Ok(())
    }

    fn reject(&mut self, message: impl Into<Cow<'static, str>>, range: TextRange) {
        if self.error.is_none() {
            self.error = Some(ParseError::syntax(message, code_range(self.filename, range)));
        }
    }

    fn enter(&mut self, range: TextRange) -> bool {
        if self.error.is_some() {
            return false;
        }
        if self.depth == MAX_NESTING_DEPTH {
            self.reject("Source is too deeply nested", range);
            return false;
        }
        self.depth += 1;
        true
    }

    fn scope(&mut self) -> &mut Scope {
        &mut self.scopes[self.current_scope]
    }

    fn push_scope(&mut self, mut scope: Scope) {
        scope.parent = Some(self.current_scope);
        self.current_scope = self.scopes.len();
        self.scopes.push(scope);
    }

    fn pop_scope(&mut self) {
        self.current_scope = self.scope().parent.expect("only nested scopes are popped");
    }

    fn check_binding(&mut self, name: &str, range: TextRange, context: ExprContext) {
        if name == "__debug__" {
            let message = if context == ExprContext::Del {
                "cannot delete __debug__"
            } else {
                "cannot assign to __debug__"
            };
            self.reject(message, range);
        }
    }

    fn bind_name(&mut self, name: &'a str, range: TextRange, context: ExprContext) {
        self.check_binding(name, range, context);
        if self.record_bindings && self.track_bindings {
            let key = self.names.key(name);
            let state = self.scope().names.entry(key).or_default();
            state.bound = true;
            state.assigned = true;
        }
    }

    fn bind_import(&mut self, name: &'a str, range: TextRange) {
        self.check_binding(name, range, ExprContext::Store);
        if self.record_bindings && self.track_bindings {
            // Imports bind closures, but CPython permits them before a global/nonlocal declaration.
            let key = self.names.key(name);
            self.scope().names.entry(key).or_default().bound = true;
        }
    }

    fn visit_function(&mut self, function: &'a ast::StmtFunctionDef) {
        self.bind_name(function.name.as_str(), function.name.range(), ExprContext::Store);
        for decorator in &function.decorator_list {
            self.visit_decorator(decorator);
        }
        if let Some(type_params) = &function.type_params {
            self.visit_type_params(type_params);
        }
        let generic = function.type_params.is_some();
        self.visit_signature(&function.parameters, generic);
        if let Some(annotation) = &function.returns {
            self.visit_signature_annotation(annotation, generic);
        }

        let mut scope = Scope::new(ScopeKind::Function {
            is_async: function.is_async,
        });
        if self.track_bindings {
            for parameter in &function.parameters {
                scope
                    .names
                    .entry(self.names.key(parameter.name().as_str()))
                    .or_default()
                    .parameter = true;
            }
            if let Some(parameters) = &function.type_params {
                for parameter in &parameters.type_params {
                    scope
                        .names
                        .entry(self.names.key(parameter.name().as_str()))
                        .or_default()
                        .type_parameter = true;
                }
            }
        }
        self.push_scope(scope);
        self.visit_body(&function.body);
        self.pop_scope();
    }

    fn visit_class(&mut self, class: &'a ast::StmtClassDef) {
        self.bind_name(class.name.as_str(), class.name.range(), ExprContext::Store);
        for decorator in &class.decorator_list {
            self.visit_decorator(decorator);
        }
        let enclosing_context = self.names.context.clone();
        if let Some(type_params) = &class.type_params {
            self.names.context = Names::class_context(class.name.as_str(), true);
            self.visit_type_params(type_params);
        }
        if let Some(arguments) = &class.arguments {
            if class.type_params.is_some() {
                self.push_scope(Scope::new(ScopeKind::GenericDefinition));
            }
            self.visit_arguments(arguments);
            if class.type_params.is_some() {
                self.pop_scope();
            }
        }

        let mut scope = Scope::new(ScopeKind::Class);
        if self.track_bindings
            && let Some(parameters) = &class.type_params
        {
            for parameter in &parameters.type_params {
                scope
                    .names
                    .entry(self.names.key(parameter.name().as_str()))
                    .or_default()
                    .type_parameter = true;
            }
        }
        self.names.context = Names::class_context(class.name.as_str(), false);
        self.push_scope(scope);
        self.visit_body(&class.body);
        self.pop_scope();
        self.names.context = enclosing_context;
    }

    fn visit_comprehension_expr(
        &mut self,
        generators: &'a [ast::Comprehension],
        values: &[&'a Expr],
        is_generator: bool,
    ) {
        let enclosing_context = self.scope().async_context;
        let body_context = AsyncContext {
            // Generator bodies execute in their own async-capable function; their first iterable does not.
            allowed: is_generator || enclosing_context.allowed,
            comprehension: true,
        };
        if !body_context.allowed
            && let Some(generator) = generators.iter().find(|generator| generator.is_async)
        {
            self.reject(
                "asynchronous comprehension outside of an asynchronous function",
                generator.range,
            );
        }
        // All targets count, including those in later clauses and enclosing comprehensions.
        let mut names = Vec::new();
        for generator in generators {
            let mut pending = vec![&generator.target];
            while let Some(target) = pending.pop() {
                match target {
                    Expr::Name(name) => names.push(self.names.key(name.id.as_str())),
                    Expr::Tuple(tuple) => pending.extend(tuple.elts.iter()),
                    Expr::List(list) => pending.extend(list.elts.iter()),
                    Expr::Starred(starred) => pending.push(&starred.value),
                    _ => {}
                }
            }
        }

        let scope = self.scope();
        scope.comprehension_depth += 1;
        for &name in &names {
            *scope.iteration_names.entry(name).or_default() += 1;
        }

        let annotation_depth = self.annotation_depth;
        for (index, generator) in generators.iter().enumerate() {
            let record_bindings = mem::replace(&mut self.record_bindings, false);
            self.visit_expr(&generator.target);
            self.record_bindings = record_bindings;
            self.iterable_depth += 1;
            // Only the first iterable shares an annotation's evaluation scope.
            self.annotation_depth = if index == 0 { annotation_depth } else { 0 };
            self.visit_expr(&generator.iter);
            self.iterable_depth -= 1;
            self.annotation_depth = 0;
            self.scope().comprehension_body_depth += usize::from(index == 0);
            self.scope().async_context = body_context;
            for condition in &generator.ifs {
                self.visit_expr(condition);
            }
        }
        for value in values {
            self.visit_expr(value);
        }
        self.annotation_depth = annotation_depth;

        let scope = self.scope();
        scope.async_context = enclosing_context;
        scope.comprehension_depth -= 1;
        scope.comprehension_body_depth -= usize::from(!generators.is_empty());
        for name in names {
            let count = scope
                .iteration_names
                .get_mut(&name)
                .expect("iteration name was registered");
            *count -= 1;
            if *count == 0 {
                scope.iteration_names.remove(&name);
            }
        }
    }

    fn visit_named_expr(&mut self, named: &'a ast::ExprNamed) {
        if self.iterable_depth > 0 {
            self.reject(
                "assignment expression cannot be used in a comprehension iterable expression",
                named.range,
            );
        } else if let Expr::Name(target) = &*named.target {
            let key = self.names.key(target.id.as_str());
            let scope = self.scope();
            if scope.iteration_names.contains_key(&key) {
                self.reject(
                    format!(
                        "assignment expression cannot rebind comprehension iteration variable '{}'",
                        target.id
                    ),
                    named.range,
                );
            } else if scope.kind == ScopeKind::Class && scope.comprehension_depth > 0 {
                self.reject(
                    "assignment expression within a comprehension cannot be used in a class body",
                    named.range,
                );
            } else if scope.kind == ScopeKind::TypeParameter && scope.comprehension_depth > 0 {
                self.reject(
                    "assignment expression within a comprehension cannot be used in a TypeVar bound",
                    named.range,
                );
            } else if scope.kind == ScopeKind::GenericDefinition {
                let message = if scope.comprehension_depth > 0 {
                    "assignment expression within a comprehension cannot be used within the definition of a generic"
                } else {
                    "named expression cannot be used within the definition of a generic"
                };
                self.reject(message, named.range);
            }
        }
        self.visit_expr(&named.value);
        self.visit_expr(&named.target);
    }

    fn visit_type_expression(&mut self, expr: &'a Expr, context: &'static str) {
        if !expression_within_depth(expr, MAX_NESTING_DEPTH - self.depth) {
            self.reject("Source is too deeply nested", expr.range());
            return;
        }
        if let Some((kind, range)) = super::forbidden_in_annotation(expr) {
            self.reject(format!("{kind} cannot be used within {context}"), range);
            return;
        }
        self.push_scope(Scope::new(ScopeKind::TypeParameter));
        self.visit_expr(expr);
        self.pop_scope();
    }

    fn visit_declaration(&mut self, names: &'a [ast::Identifier], global: bool) {
        let kind = if global { "global" } else { "nonlocal" };
        for name in names {
            let text = name.as_str();
            let key = self.names.key(text);
            let state = self.scope().names.get(&key).copied().unwrap_or_default();
            let message = if state.parameter {
                Some(format!("name '{text}' is parameter and {kind}"))
            } else if state.used {
                Some(format!("name '{text}' is used prior to {kind} declaration"))
            } else if state.annotated {
                Some(format!("annotated name '{text}' can't be {kind}"))
            } else if state.assigned {
                Some(format!("name '{text}' is assigned to before {kind} declaration"))
            } else {
                None
            };
            if let Some(message) = message {
                self.reject(message, name.range());
            }
            let scope = self.scope();
            if global {
                scope.names.entry(key).or_default().global = true;
            } else {
                scope.names.entry(key).or_default().nonlocal = true;
                self.nonlocals.push(Nonlocal {
                    name: self.names.display(text),
                    key,
                    range: name.range(),
                    scope: self.current_scope,
                });
            }
        }
    }

    fn visit_signature_annotation(&mut self, annotation: &'a Expr, generic: bool) {
        if generic {
            self.push_scope(Scope::new(ScopeKind::GenericDefinition));
        }
        self.visit_annotation(annotation);
        if generic {
            self.pop_scope();
        }
    }

    fn visit_signature(&mut self, parameters: &'a ast::Parameters, generic: bool) {
        let mut seen = AHashSet::new();
        for parameter in parameters {
            let parameter = parameter.as_parameter();
            self.check_binding(parameter.name.as_str(), parameter.name.range(), ExprContext::Store);
            if !seen.insert(self.names.key(parameter.name.as_str())) {
                self.reject(
                    format!("duplicate argument '{}' in function definition", parameter.name),
                    parameter.name.range(),
                );
            }
        }
        // Defaults execute in the enclosing scope, outside a generic's annotation scope.
        for parameter in parameters.iter_non_variadic_params() {
            if let Some(default) = &parameter.default {
                self.visit_expr(default);
            }
        }
        for parameter in parameters {
            if let Some(annotation) = &parameter.as_parameter().annotation {
                self.visit_signature_annotation(annotation, generic);
            }
        }
    }

    fn visit_assignment_sequence(&mut self, elements: &'a [Expr], range: TextRange) {
        for element in elements {
            if let Expr::Starred(starred) = element {
                if self.enter(starred.range) {
                    self.visit_expr(&starred.value);
                    self.depth -= 1;
                }
            } else {
                self.visit_expr(element);
            }
        }
        if self.skipped_codegen_checks == 0
            && elements.iter().filter(|expr| matches!(expr, Expr::Starred(_))).count() > 1
        {
            self.reject("multiple starred expressions in assignment", range);
        }
    }
}

impl<'a> Visitor<'a> for Validator<'a> {
    fn visit_body(&mut self, body: &'a [Stmt]) {
        for stmt in body {
            if self.error.is_some() {
                break;
            }
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if !self.enter(stmt.range()) {
            return;
        }
        match stmt {
            Stmt::Import(ast::StmtImport {
                is_lazy: true, range, ..
            })
            | Stmt::ImportFrom(ast::StmtImportFrom {
                is_lazy: true, range, ..
            }) => {
                // Ignoring this flag would turn a lazy import into an eager one.
                self.reject("invalid syntax", *range);
            }
            Stmt::FunctionDef(function) => self.visit_function(function),
            Stmt::ClassDef(class) => self.visit_class(class),
            Stmt::Return(return_) if !matches!(self.scope().kind, ScopeKind::Function { .. }) => {
                self.reject("'return' outside function", return_.range);
            }
            Stmt::Global(global) => self.visit_declaration(&global.names, true),
            Stmt::Nonlocal(nonlocal) => self.visit_declaration(&nonlocal.names, false),
            Stmt::Try(try_) => {
                for handler in try_.handlers.iter().take(try_.handlers.len().saturating_sub(1)) {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    if handler.type_.is_none() {
                        self.reject("default 'except:' must be last", handler.range);
                    }
                }
                visitor::walk_stmt(self, stmt);
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    let binding = alias.asname.as_ref().unwrap_or(&alias.name);
                    let name = binding.as_str().split('.').next().unwrap_or_default();
                    self.bind_import(name, binding.range());
                }
            }
            Stmt::ImportFrom(import) => {
                if import.level == 0
                    && import
                        .module
                        .as_ref()
                        .is_some_and(|module| module.as_str() == "__future__")
                    && import.names.iter().any(|alias| alias.name.as_str() == "annotations")
                {
                    self.future_annotations = true;
                }
                for alias in &import.names {
                    let binding = alias.asname.as_ref().unwrap_or(&alias.name);
                    self.bind_import(binding.as_str(), binding.range());
                }
            }
            Stmt::AnnAssign(annotation) => {
                if let Some(value) = &annotation.value {
                    self.visit_expr(value);
                }
                // Local and non-simple annotations are checked by the symbol table but never compiled.
                if self.track_bindings
                    && annotation.simple
                    && let Expr::Name(name) = &*annotation.target
                {
                    let text = name.id.as_str();
                    let key = self.names.key(text);
                    let scope = self.scope();
                    let state = scope.names.get(&key).copied().unwrap_or_default();
                    let declaration = if scope.kind != ScopeKind::Module && state.global {
                        Some("global")
                    } else if state.nonlocal {
                        Some("nonlocal")
                    } else {
                        None
                    };
                    if let Some(declaration) = declaration {
                        self.reject(format!("annotated name '{text}' can't be {declaration}"), name.range);
                    }
                    self.scope().names.entry(key).or_default().annotated = true;
                }
                let skipped = matches!(self.scope().kind, ScopeKind::Function { .. }) || !annotation.simple;
                self.skipped_codegen_checks += usize::from(skipped);
                self.visit_annotation(&annotation.annotation);
                self.skipped_codegen_checks -= usize::from(skipped);
                let record_bindings = self.record_bindings;
                if !annotation.simple && annotation.value.is_none() && matches!(*annotation.target, Expr::Name(_)) {
                    self.record_bindings = false;
                }
                self.visit_expr(&annotation.target);
                self.record_bindings = record_bindings;
            }
            _ => visitor::walk_stmt(self, stmt),
        }
        self.depth -= 1;
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if !self.enter(expr.range()) {
            return;
        }
        match expr {
            Expr::Name(name) if matches!(name.ctx, ExprContext::Store | ExprContext::Del) => {
                self.bind_name(name.id.as_str(), name.range, name.ctx);
            }
            Expr::Name(name) => {
                // Only the first iterable executes in the enclosing scope; other comprehension reads belong to it.
                if self.record_bindings && self.track_bindings && self.scope().comprehension_body_depth == 0 {
                    let key = self.names.key(name.id.as_str());
                    self.scope().names.entry(key).or_default().used = true;
                }
            }
            Expr::Tuple(tuple) if tuple.ctx == ExprContext::Store => {
                self.visit_assignment_sequence(&tuple.elts, tuple.range);
            }
            Expr::List(list) if list.ctx == ExprContext::Store => {
                self.visit_assignment_sequence(&list.elts, list.range);
            }
            Expr::Starred(starred) if starred.ctx == ExprContext::Store => {
                self.visit_expr(&starred.value);
                if self.skipped_codegen_checks == 0 {
                    self.reject("starred assignment target must be in a list or tuple", starred.range);
                }
            }
            Expr::Attribute(attribute) if matches!(attribute.ctx, ExprContext::Store | ExprContext::Del) => {
                self.check_binding(attribute.attr.as_str(), attribute.attr.range(), attribute.ctx);
                self.visit_expr(&attribute.value);
            }
            Expr::Named(named) => self.visit_named_expr(named),
            Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                self.push_scope(Scope::new(ScopeKind::Function { is_async: false }));
                let annotation_depth = mem::take(&mut self.annotation_depth);
                self.visit_expr(&lambda.body);
                self.annotation_depth = annotation_depth;
                self.pop_scope();
            }
            Expr::Await(await_) if self.annotation_depth == 0 && !self.scope().async_context.allowed => {
                match (
                    self.scopes[self.current_scope].async_context.comprehension,
                    self.scopes[self.current_scope].kind,
                ) {
                    (true, _) => self.reject(
                        "asynchronous comprehension outside of an asynchronous function",
                        await_.range,
                    ),
                    (_, ScopeKind::Function { is_async: false }) => {
                        self.reject("'await' outside async function", await_.range);
                    }
                    (_, ScopeKind::Class) => self.reject("'await' outside function", await_.range),
                    (_, ScopeKind::GenericDefinition) => self.reject(
                        "await expression cannot be used within the definition of a generic",
                        await_.range,
                    ),
                    _ => self.visit_expr(&await_.value),
                }
            }
            Expr::Yield(_) | Expr::YieldFrom(_) if self.scope().kind == ScopeKind::GenericDefinition => {
                self.reject(
                    "yield expression cannot be used within the definition of a generic",
                    expr.range(),
                );
            }
            Expr::ListComp(comp) => self.visit_comprehension_expr(&comp.generators, &[&comp.elt], false),
            Expr::SetComp(comp) => self.visit_comprehension_expr(&comp.generators, &[&comp.elt], false),
            Expr::DictComp(comp) => {
                let mut values = Vec::with_capacity(2);
                if let Some(key) = &comp.key {
                    values.push(&**key);
                }
                values.push(&*comp.value);
                self.visit_comprehension_expr(&comp.generators, &values, false);
            }
            Expr::Generator(comp) => self.visit_comprehension_expr(&comp.generators, &[&comp.elt], true),
            Expr::FString(_) | Expr::TString(_) => {
                // An f-string expression inside a field starts an independent format specification.
                let format_depth = mem::take(&mut self.format_depth);
                let template_string = mem::replace(&mut self.template_string, matches!(expr, Expr::TString(_)));
                visitor::walk_expr(self, expr);
                self.template_string = template_string;
                self.format_depth = format_depth;
            }
            _ => visitor::walk_expr(self, expr),
        }
        self.depth -= 1;
    }

    fn visit_parameters(&mut self, parameters: &'a ast::Parameters) {
        self.visit_signature(parameters, false);
    }

    fn visit_annotation(&mut self, annotation: &'a Expr) {
        if !expression_within_depth(annotation, MAX_NESTING_DEPTH - self.depth) {
            self.reject("Source is too deeply nested", annotation.range());
            return;
        }
        if let Some((kind, range)) = super::forbidden_in_annotation(annotation) {
            self.reject(format!("{kind} cannot be used within an annotation"), range);
            return;
        }
        // Stringized annotations skip code generation's call and assignment-target checks.
        self.skipped_codegen_checks += usize::from(self.future_annotations);
        self.annotation_depth += 1;
        let record_bindings = mem::replace(&mut self.record_bindings, false);
        let async_context = self.scope().async_context;
        self.scope().async_context = AsyncContext {
            allowed: false,
            comprehension: false,
        };
        self.visit_expr(annotation);
        self.scope().async_context = async_context;
        self.record_bindings = record_bindings;
        self.annotation_depth -= 1;
        self.skipped_codegen_checks -= usize::from(self.future_annotations);
    }

    fn visit_arguments(&mut self, arguments: &'a ast::Arguments) {
        let mut seen = AHashSet::new();
        for keyword in &arguments.keywords {
            if let Some(name) = &keyword.arg
                && self.skipped_codegen_checks == 0
                && !seen.insert(name.as_str())
            {
                self.reject(format!("keyword argument repeated: {name}"), keyword.range);
            }
        }
        visitor::walk_arguments(self, arguments);
    }

    fn visit_keyword(&mut self, keyword: &'a ast::Keyword) {
        if let Some(name) = &keyword.arg {
            self.check_binding(name.as_str(), name.range(), ExprContext::Store);
        }
        visitor::walk_keyword(self, keyword);
    }

    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(handler_) = handler;
        if let Some(name) = &handler_.name {
            self.bind_name(name.as_str(), name.range(), ExprContext::Store);
        }
        visitor::walk_except_handler(self, handler);
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        if !self.enter(pattern.range()) {
            return;
        }
        if let Pattern::MatchValue(value) = pattern
            && matches!(*value.value, Expr::FString(_) | Expr::TString(_))
        {
            self.reject("patterns may only match literals and attribute lookups", value.range);
        }
        let name = match pattern {
            Pattern::MatchAs(pattern) => pattern.name.as_ref(),
            Pattern::MatchStar(pattern) => pattern.name.as_ref(),
            Pattern::MatchMapping(pattern) => pattern.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = name {
            self.bind_name(name.as_str(), name.range(), ExprContext::Store);
        }
        visitor::walk_pattern(self, pattern);
        self.depth -= 1;
    }

    fn visit_pattern_keyword(&mut self, keyword: &'a ast::PatternKeyword) {
        self.check_binding(keyword.attr.as_str(), keyword.attr.range(), ExprContext::Store);
        visitor::walk_pattern_keyword(self, keyword);
    }

    fn visit_interpolated_string_element(&mut self, element: &'a ast::InterpolatedStringElement) {
        // Nested format specs recurse without visiting an Expr node.
        if !self.enter(element.range()) {
            return;
        }
        if let ast::InterpolatedStringElement::Interpolation(interpolation) = element {
            // CPython permits the outer field and two nested format specifications.
            if self.format_depth >= 3 {
                let message = if self.template_string {
                    "t-string: expressions nested too deeply"
                } else {
                    "f-string: expressions nested too deeply"
                };
                self.reject(message, interpolation.range);
            } else {
                self.visit_expr(&interpolation.expression);
                if let Some(spec) = &interpolation.format_spec {
                    self.format_depth += 1;
                    for element in &spec.elements {
                        self.visit_interpolated_string_element(element);
                    }
                    self.format_depth -= 1;
                }
            }
        }
        self.depth -= 1;
    }

    fn visit_type_param(&mut self, parameter: &'a ast::TypeParam) {
        let name = parameter.name();
        self.check_binding(name.as_str(), name.range(), ExprContext::Store);
        if name.as_str() == "__classdict__" {
            self.reject(
                "reserved name '__classdict__' cannot be used for type parameter",
                name.range(),
            );
        }
        if let ast::TypeParam::TypeVar(type_var) = parameter
            && let Some(bound) = &type_var.bound
        {
            let context = if matches!(&**bound, Expr::Tuple(_)) {
                "a TypeVar constraint"
            } else {
                "a TypeVar bound"
            };
            self.visit_type_expression(bound, context);
        }
        if let Some(default) = parameter.default() {
            let context = match parameter {
                ast::TypeParam::TypeVar(_) => "a TypeVar default",
                ast::TypeParam::TypeVarTuple(_) => "a TypeVarTuple default",
                ast::TypeParam::ParamSpec(_) => "a ParamSpec default",
            };
            self.visit_type_expression(default, context);
        }
    }

    fn visit_type_params(&mut self, parameters: &'a ast::TypeParams) {
        let mut seen = AHashSet::new();
        let mut default_seen = false;
        for parameter in &parameters.type_params {
            let name = parameter.name();
            self.names.type_parameter(name.as_str());
            if !seen.insert(self.names.key(name.as_str())) {
                self.reject(format!("duplicate type parameter '{name}'"), name.range());
            } else if default_seen && parameter.default().is_none() {
                self.reject(
                    format!("non-default type parameter '{name}' follows default type parameter"),
                    name.range(),
                );
            }
            default_seen |= parameter.default().is_some();
            self.visit_type_param(parameter);
        }
    }
}
