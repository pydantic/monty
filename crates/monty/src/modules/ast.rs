use std::borrow::Cow;

use memchr::memchr2_iter;
use ruff_python_ast::{DictItem, Expr, ExprNumberLiteral, Mod, Number, Operator, UnaryOp};
use ruff_python_parser::Mode;
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
    parse::{MAX_NESTING_DEPTH, parse_int_literal, trailing_indent},
    resource_checks::try_vec_with_capacity,
    source_nesting::{expression_nesting_bound_exceeded, parse_shallow},
    types::{
        Complex, Dict, List, LongInt, Module, Set, bytes::allocate_bytes, str::allocate_string, tuple::allocate_tuple,
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
    vm.heap.tracker.check_memory_time()?;
    let expr = parse_literal_source(source)?;
    vm.heap.tracker.check_memory_time()?;
    LiteralSource(node_or_string).convert(&expr, 0, vm)
}

fn parse_literal_source(source: &str) -> RunResult<Box<Expr>> {
    if source.contains('\0') {
        return Err(
            SimpleException::new_msg(ExcType::SyntaxError, "source code string cannot contain null bytes").into(),
        );
    }
    TextSize::try_from(source.len()).map_err(|_| ExcType::allocation_too_large())?;
    if source.starts_with('\u{feff}') {
        return Err(syntax_error("invalid non-printable character U+FEFF", 1));
    }
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
    if let Some(range) = trailing_indent(source) {
        return Err(syntax_error("unexpected indent", line_at(source, range.start())));
    }
    let Mod::Expression(expr) = parsed.into_syntax() else {
        unreachable!("expression mode produces an expression");
    };
    Ok(expr.body)
}

// Borrow the source text from the VM only while parsing or formatting an error.
struct LiteralSource<'a>(&'a Value);

impl LiteralSource<'_> {
    fn convert(&self, expr: &Expr, depth: u16, vm: &mut VM<'_>) -> RunResult<Value> {
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
                vm.heap.tracker.check_pending_allocation(expr.range.len().to_usize())?;
                Ok(allocate_string(expr.value.to_string(), vm.heap))
            }
            Expr::BytesLiteral(expr) => {
                vm.heap.tracker.check_pending_allocation(expr.range.len().to_usize())?;
                let bytes = Cow::from(&expr.value).into_owned();
                Ok(allocate_bytes(bytes, vm.heap))
            }
            Expr::NumberLiteral(number) => self.number(number, false, vm),
            Expr::UnaryOp(_) => {
                let (number, negative) = self.signed_number(expr, vm)?;
                self.number(number, negative, vm)
            }
            Expr::BinOp(op) if matches!(op.op, Operator::Add | Operator::Sub) => {
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
                    result.add(self.convert(item, depth + 1, vm)?, vm)?;
                }
                let (set, vm) = guard.into_parts();
                Ok(Value::allocate(set, vm.heap))
            }
            // CPython accepts exactly this call syntax for an empty set.
            Expr::Call(call)
                if call.arguments.is_empty() && matches!(call.func.as_ref(), Expr::Name(name) if name.id == "set") =>
            {
                Ok(Value::allocate(Set::new(), vm.heap))
            }
            _ => Err(self.malformed(expr, vm)),
        }
    }

    fn sequence(&self, items: &[Expr], depth: u16, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
        vm.heap
            .tracker
            .check_pending_allocation(items.len().saturating_mul(VALUE_SIZE))?;
        let mut guard = DropGuard::new(try_vec_with_capacity(items.len())?, vm);
        let (values, vm) = guard.as_parts_mut();
        for (i, item) in items.iter().enumerate() {
            vm.heap.tracker.check_memory_time_every(i)?;
            values.push(self.convert(item, depth + 1, vm)?);
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
            let key = self.convert(key, depth + 1, vm)?;
            let mut key_guard = DropGuard::new(key, &mut *vm);
            let value = self.convert(&item.value, depth + 1, key_guard.ctx())?;
            let (key, vm) = key_guard.into_parts();
            dict.set(key, value, vm)?.drop_with(vm);
        }
        let (dict, vm) = guard.into_parts();
        Ok(Value::allocate(dict, vm.heap))
    }

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
        let source = self.0.as_str(vm).expect("literal source is a string");
        line_at(source.trim_start_matches([' ', '\t']), offset)
    }
}

fn syntax_error(message: &str, line: usize) -> RunError {
    SimpleException::new_msg(ExcType::SyntaxError, format!("{message} (<unknown>, line {line})")).into()
}

fn line_at(source: &str, offset: TextSize) -> usize {
    let source = source.as_bytes();
    let prefix = &source[..offset.to_usize().min(source.len())];
    1 + memchr2_iter(b'\r', b'\n', prefix)
        .filter(|&i| prefix[i] == b'\r' || i == 0 || prefix[i - 1] != b'\r')
        .count()
}
