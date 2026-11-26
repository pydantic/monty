//! String-input implementation of `ast.literal_eval`.
//!
//! The converter constructs Monty values directly from Ruff's expression tree.
//! A parsed expression outside the literal grammar raises `ValueError`.
//!
//! `ast.literal_eval('[-1 + 2j, 3 - 4j]')` returns a list of two complex
//! numbers. These forms take a real literal with an optional sign on the
//! left and a bare imaginary literal on the right.
//! `ast.literal_eval('1 + 2')` and `ast.literal_eval('1 + -2j')` raise
//! `ValueError`. Ordinary Python expressions evaluate these to `3` and `1-2j`.
//!
//! Raw CR and CRLF become LF before parsing, including inside quoted
//! literals. Decimal integer literals have a 4,300-digit limit.

use memchr::{memchr, memchr_iter};
use monty_types::ResourceTracker;
use ruff_python_ast::{
    DictItem, Expr, ExprNumberLiteral, Mod, Number, Operator, UnaryOp,
    token::{Token, TokenKind},
};
use ruff_python_parser::Mode;
use ruff_source_file::UniversalNewlines;
use ruff_text_size::{Ranged, TextSize};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropGuard, DropWithContext},
    intern::StaticStrings,
    modules::ModuleFunctions,
    parse::{MAX_NESTING_DEPTH, check_decimal_literal_digits, parse_int_literal, trailing_indent},
    resource_checks::try_vec_with_capacity,
    source_nesting::{expression_nesting_bound_exceeded, parse_shallow},
    types::{
        Complex, Dict, List, LongInt, Module, Set,
        bytes::allocate_bytes,
        long_int::{INT_MAX_STR_DIGITS, decimal_digit_count_ascii},
        str::allocate_string,
        tuple::allocate_tuple,
    },
    value::{VALUE_SIZE, Value},
};

#[monty_macros::explicit_discriminants(checksum = 0x6d5b_d268_a614_2378)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum AstFunctions {
    LiteralEval = 0,
}

pub(super) fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Ast, vm.interns);
    module.set_attr(
        StaticStrings::LiteralEval,
        Value::ModuleFunction(ModuleFunctions::Ast(AstFunctions::LiteralEval)),
        vm,
    );
    module
}

pub(super) fn call(vm: &mut VM<'_>, function: AstFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        AstFunctions::LiteralEval => literal_eval(vm, args),
    }
}

#[derive(FromArgs)]
#[from_args(name = "literal_eval", style = def)]
struct LiteralEvalArgs {
    node_or_string: Value,
}

fn literal_eval(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let LiteralEvalArgs { node_or_string } = LiteralEvalArgs::from_args(args, vm)?;
    defer_drop!(node_or_string, vm);

    let source = node_or_string
        .as_str(vm)
        .ok_or_else(|| ExcType::value_error("malformed node or string"))?
        .trim_start_matches([' ', '\t']);

    // Ruff runs between VM checkpoints. Check the budget before it runs,
    // then check what it used before allocating the result.
    vm.heap.tracker.check_memory_time()?;
    let (expr, normalized) = parse_literal_source(source, &vm.heap.tracker)?;
    vm.heap.tracker.check_memory_time()?;

    let literal = LiteralSource {
        input: node_or_string,
        normalized: normalized.as_deref(),
    };
    literal.eval(&expr, 0, vm)
}

/// The lexer scan bounds Ruff's recursion on nested expressions.
/// A flat chain like `1 + 1 + ...` can still build a deep tree in a loop.
/// [`parse_shallow`] dismantles such trees iteratively, including trees Ruff
/// builds while recovering from syntax errors.
fn parse_literal_source(source: &str, tracker: &ResourceTracker) -> RunResult<(Box<Expr>, Option<String>)> {
    // CPython's _Py_SourceAsString rejects NUL before tokenizing.
    // Ruff accepts it inside quoted literals and comments.
    if source.contains('\0') {
        return Err(
            SimpleException::new_msg(ExcType::SyntaxError, "source code string cannot contain null bytes").into(),
        );
    }
    TextSize::try_from(source.len()).map_err(|_| ExcType::allocation_too_large())?;

    // Ruff skips a leading BOM as it would in a UTF-8 file. CPython's
    // str tokenizer sees U+FEFF as a source character and rejects it.
    if source.starts_with('\u{feff}') {
        return Err(syntax_error("invalid non-printable character U+FEFF", 1));
    }

    // CPython translates raw CR and CRLF to LF before tokenization, even
    // inside quoted literals. An escaped CR is decoded later.
    let normalized = normalize_newlines(source, tracker)?;
    let source = normalized.as_deref().unwrap_or(source);

    if let Some(range) = expression_nesting_bound_exceeded(source, MAX_NESTING_DEPTH) {
        return Err(syntax_error(
            "Source is too deeply nested",
            line_at(source, range.start()),
        ));
    }

    let parsed = parse_shallow(source, Mode::Expression)
        .map_err(|range| syntax_error("Source is too deeply nested", line_at(source, range.start())))?;
    if let Some(error) = parsed.errors().first() {
        return Err(syntax_error(&error.error.to_string(), line_at(source, error.start())));
    }
    if let Some(range) = trailing_indent(source, parsed.tokens()) {
        return Err(syntax_error("unexpected indent", line_at(source, range.start())));
    }

    check_decimal_tokens(source, parsed.tokens(), tracker)?;

    let Mod::Expression(expr) = parsed.into_syntax() else {
        unreachable!("expression mode produces an expression");
    };
    Ok((expr.body, normalized))
}

/// CPython checks decimal length while building the AST, even for numbers
/// inside calls or other expressions that `literal_eval` rejects.
fn check_decimal_tokens(source: &str, tokens: &[Token], tracker: &ResourceTracker) -> RunResult<()> {
    if source.len() <= INT_MAX_STR_DIGITS {
        return Ok(());
    }

    for (i, token) in tokens.iter().enumerate() {
        tracker.check_time_every(i)?;

        let range = token.range();
        if token.kind() != TokenKind::Int || range.len().to_usize() <= INT_MAX_STR_DIGITS {
            continue;
        }

        let digits = source[range].as_bytes();
        // A valid integer token starting with zero is base-prefixed or zero.
        if digits.starts_with(b"0") {
            continue;
        }

        let digit_count = decimal_digit_count_ascii(digits);
        check_decimal_literal_digits(digit_count).map_err(|msg| syntax_error(&msg, line_at(source, token.start())))?;
    }

    Ok(())
}

fn normalize_newlines(source: &str, tracker: &ResourceTracker) -> RunResult<Option<String>> {
    if memchr(b'\r', source.as_bytes()).is_none() {
        return Ok(None);
    }

    tracker.check_pending_allocation(source.len())?;
    let mut text = String::with_capacity(source.len());

    for (i, line) in source.universal_newlines().enumerate() {
        tracker.check_time_every(i)?;
        text.push_str(line.as_str());
        if line.line_ending().is_some() {
            text.push('\n');
        }
    }

    Ok(Some(text))
}

struct LiteralSource<'a> {
    input: &'a Value,
    normalized: Option<&'a str>,
}

impl LiteralSource<'_> {
    /// Bounds native stack use while evaluating nested containers.
    fn eval(&self, expr: &Expr, depth: u16, vm: &mut VM<'_>) -> RunResult<Value> {
        if depth == MAX_NESTING_DEPTH {
            return Err(syntax_error(
                "Source is too deeply nested",
                self.line_at(expr.start(), vm),
            ));
        }

        match expr {
            Expr::NoneLiteral(_) => Ok(Value::None),
            Expr::EllipsisLiteral(_) => Ok(Value::Ellipsis),
            Expr::BooleanLiteral(expr) => Ok(Value::Bool(expr.value)),

            Expr::StringLiteral(expr) => {
                let len = expr.value.len();
                vm.heap.tracker.check_pending_allocation(len)?;
                let mut text = String::with_capacity(len);
                for part in &expr.value {
                    text.push_str(part.as_str());
                }
                Ok(allocate_string(text, vm.heap))
            }
            Expr::BytesLiteral(expr) => {
                let len = expr.value.len();
                vm.heap.tracker.check_pending_allocation(len)?;
                let mut bytes = try_vec_with_capacity(len)?;
                for part in &expr.value {
                    bytes.extend_from_slice(part.as_slice());
                }
                Ok(allocate_bytes(bytes, vm.heap))
            }

            Expr::NumberLiteral(number) => self.number(number, false, vm),
            Expr::UnaryOp(_) => {
                let (number, negative) = self.signed_number(expr, vm)?;
                self.number(number, negative, vm)
            }
            Expr::BinOp(op) if matches!(op.op, Operator::Add | Operator::Sub) => {
                // CPython accepts complex forms such as `-1 + 2j` and `3 - 4j`.
                // The real term may have a unary sign. The imaginary term is a
                // bare literal, so `1 + -2j` is rejected.
                let (left, negative) = self.signed_number(&op.left, vm)?;
                let Expr::NumberLiteral(right) = op.right.as_ref() else {
                    return Err(self.malformed(&op.right, vm));
                };

                if matches!(left.value, Number::Complex { .. }) || !matches!(right.value, Number::Complex { .. }) {
                    return Err(self.malformed(expr, vm));
                }

                let left = self.number(left, negative, vm)?;
                defer_drop!(left, vm);

                let right = self.number(right, false, vm)?;
                defer_drop!(right, vm);

                match op.op {
                    Operator::Add => left.py_add(right, vm),
                    _ => left.py_sub(right, vm),
                }
            }

            Expr::List(list) => {
                let values = self.sequence(&list.elts, depth, vm)?;
                Ok(Value::allocate(List::new(values), vm.heap))
            }
            Expr::Tuple(tuple) => {
                let values = self.sequence(&tuple.elts, depth, vm)?;
                Ok(allocate_tuple(SmallVec::from_vec(values), vm.heap))
            }
            Expr::Dict(dict) => self.dict(&dict.items, depth, vm),
            Expr::Set(set) => {
                let mut guard = DropGuard::new(Set::new(), vm);
                let (result, vm) = guard.as_parts_mut();

                for (i, item) in set.elts.iter().enumerate() {
                    vm.heap.tracker.check_memory_time_every(i)?;
                    let value = self.eval(item, depth + 1, vm)?;
                    result.add(value, vm)?;
                }

                let (set, vm) = guard.into_parts();
                Ok(Value::allocate(set, vm.heap))
            }

            // `literal_eval()` makes one exception for call syntax. `set()`
            // represents an empty set because `{}` represents an empty dict.
            Expr::Call(call)
                if call.arguments.is_empty() && matches!(call.func.as_ref(), Expr::Name(name) if name.id == "set") =>
            {
                Ok(Value::allocate(Set::new(), vm.heap))
            }

            _ => Err(self.malformed(expr, vm)),
        }
    }

    /// Keeps each converted element in the guard until the entire sequence
    /// succeeds, so an error releases the elements already built.
    fn sequence(&self, items: &[Expr], depth: u16, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
        vm.heap
            .tracker
            .check_pending_allocation(items.len().saturating_mul(VALUE_SIZE))?;

        let mut guard = DropGuard::new(try_vec_with_capacity(items.len())?, vm);
        let (values, vm) = guard.as_parts_mut();
        for (i, item) in items.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(i)?;
            values.push(self.eval(item, depth + 1, vm)?);
        }

        Ok(guard.into_inner())
    }

    fn dict(&self, items: &[DictItem], depth: u16, vm: &mut VM<'_>) -> RunResult<Value> {
        let mut guard = DropGuard::new(Dict::new(), vm);
        let (dict, vm) = guard.as_parts_mut();

        for (i, item) in items.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(i)?;

            let key = item
                .key
                .as_ref()
                .ok_or_else(|| ExcType::value_error("malformed node or string: None"))?;
            let key = self.eval(key, depth + 1, vm)?;

            // Release the key if conversion of the value fails.
            let mut key_guard = DropGuard::new(key, &mut *vm);
            let value = self.eval(&item.value, depth + 1, key_guard.ctx())?;
            let (key, vm) = key_guard.into_parts();
            dict.set(key, value, vm)?.drop_with(vm);
        }

        let (dict, vm) = guard.into_parts();
        Ok(Value::allocate(dict, vm.heap))
    }

    /// Unwraps a single optional sign. The operand must be a number literal.
    fn signed_number<'a>(&self, expr: &'a Expr, vm: &VM<'_>) -> RunResult<(&'a ExprNumberLiteral, bool)> {
        let (operand, negative) = match expr {
            Expr::UnaryOp(op) if matches!(op.op, UnaryOp::UAdd | UnaryOp::USub) => {
                (op.operand.as_ref(), op.op == UnaryOp::USub)
            }
            _ => (expr, false),
        };
        match operand {
            Expr::NumberLiteral(number) => Ok((number, negative)),
            _ => Err(self.malformed(operand, vm)),
        }
    }

    fn number(&self, expr: &ExprNumberLiteral, negative: bool, vm: &VM<'_>) -> RunResult<Value> {
        match &expr.value {
            Number::Int(value) => {
                if let Some(value) = value.as_i64() {
                    return Ok(Value::Int(if negative { -value } else { value }));
                }

                vm.heap
                    .tracker
                    .check_pending_allocation(expr.range.len().to_usize().saturating_mul(3))?;
                let value = parse_int_literal(&value.to_string())
                    .map_err(|msg| syntax_error(&msg, self.line_at(expr.start(), vm)))?;
                let value = if negative { -value } else { value };
                Ok(LongInt::new(value).into_value(vm.heap))
            }
            Number::Float(value) => Ok(Value::Float(if negative { -*value } else { *value })),
            Number::Complex { imag, .. } => {
                // Negation flips the real zero too. CPython renders `-0j` as `(-0-0j)`.
                let (real, imag) = if negative { (-0.0, -*imag) } else { (0.0, *imag) };
                Ok(Complex::new(real, imag).into_value(vm.heap))
            }
        }
    }

    fn malformed(&self, expr: &Expr, vm: &VM<'_>) -> RunError {
        ExcType::value_error(format_args!(
            "malformed node or string on line {}",
            self.line_at(expr.start(), vm)
        ))
    }

    fn line_at(&self, offset: TextSize, vm: &VM<'_>) -> usize {
        // Ruff's offsets refer to the text after trimming and newline translation.
        let source = self.normalized.unwrap_or_else(|| {
            self.input
                .as_str(vm)
                .expect("literal source is a string")
                .trim_start_matches([' ', '\t'])
        });
        line_at(source, offset)
    }
}

/// Uses `<unknown>`, the default filename for CPython's `ast.parse()`.
fn syntax_error(message: &str, line: usize) -> RunError {
    SimpleException::new_msg(ExcType::SyntaxError, format!("{message} (<unknown>, line {line})")).into()
}

/// Counts LF in the source passed to Ruff.
fn line_at(source: &str, offset: TextSize) -> usize {
    let source = source.as_bytes();
    let prefix = &source[..offset.to_usize().min(source.len())];
    1 + memchr_iter(b'\n', prefix).count()
}
