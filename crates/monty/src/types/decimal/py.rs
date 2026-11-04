//! The Python-facing `decimal.Decimal`: construction, operators, conversions and
//! the method surface of both `Decimal` and `Context`.
//!
//! Every operation picks its context (an explicit `context=` argument or the session's
//! current one), runs the engine under a [`Ctx`] and then applies the conditions it
//! raised with [`apply_status`], which sets flags and raises trapped signals.
//! Implicit operands follow C `_decimal`: `int` converts exactly, anything else makes
//! an operator return `NotImplemented` and a method raise `TypeError`.

use std::{borrow::Cow, cmp::Ordering, fmt::Write, mem};

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Pow, ToPrimitive, Zero};
use smallvec::smallvec;
use strum::{EnumString, IntoStaticStr};

use super::{
    Coeff, Ctx, Decimal, Special,
    context::{Condition, DecContext, Rounding},
    parse,
    py_context::{apply_status, as_context, current_context, settings_of},
};
use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    expressions::CmpOperator,
    hash::{HashValue, hash_f64, hash_python_long_int, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapObjectRead},
    intern::StaticStrings,
    py_value::PyValue,
    types::{
        BuiltinType, CmpOrder, Complex, LazyHeapSet, LongInt, NamedTuple, PyTrait, Type, allocate_string,
        str::allocate_string_no_interning, tuple::allocate_tuple,
    },
    value::{EitherStr, Value},
};

/// Allocates `decimal` on the heap.
pub(crate) fn decimal_value(decimal: Decimal, vm: &VM<'_>) -> Value {
    Value::Ref(vm.heap.allocate(HeapData::Decimal(decimal)))
}

/// The `Decimal` a value refers to, if it is one.
pub(crate) fn as_decimal<'a>(value: &Value, vm: &'a VM<'_>) -> Option<&'a Decimal> {
    match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Decimal(decimal) => Some(decimal),
            _ => None,
        },
        _ => None,
    }
}

/// Converts an operand as the arithmetic operators do: a `Decimal` as is, an `int`
/// (or `bool`) exactly, anything else `None` for `NotImplemented`.
fn implicit<'a>(value: &Value, vm: &'a VM<'_>) -> RunResult<Option<Cow<'a, Decimal>>> {
    if let Some(decimal) = as_decimal(value, vm) {
        return Ok(Some(Cow::Borrowed(decimal)));
    }
    Ok(match value.py_value(vm) {
        PyValue::Bool(b) => Some(Cow::Owned(Decimal::from_i64(i64::from(b)))),
        PyValue::Int(int) => Some(Cow::Owned(int.with_value(
            vm,
            |small| Ok(Decimal::from_i64(small)),
            parse::from_int,
        )?)),
        _ => None,
    })
}

/// Converts a method operand, raising C `_decimal`'s `TypeError` for anything that
/// is neither a `Decimal` nor an `int`.
fn convert<'a>(value: &Value, vm: &'a VM<'_>) -> RunResult<Cow<'a, Decimal>> {
    implicit(value, vm)?.ok_or_else(|| ExcType::decimal_conversion(&value.py_type_name(vm)))
}

/// Resolves an optional `context=` argument: `None` means the current context.
fn context_arg(value: &Value, vm: &mut VM<'_>) -> RunResult<HeapId> {
    if matches!(value, Value::None) {
        Ok(current_context(vm))
    } else {
        as_context(value, vm).ok_or_else(ExcType::decimal_optional_context)
    }
}

/// Parses a `ROUND_*` string argument.
pub(crate) fn rounding_arg(value: &Value, vm: &VM<'_>) -> RunResult<Rounding> {
    value
        .as_either_str(vm.heap)
        .and_then(|name| Rounding::from_name(name.as_str(vm.interns)))
        .ok_or_else(ExcType::decimal_invalid_rounding)
}

/// An optional `rounding=` argument: `None` keeps the context's mode.
fn optional_rounding(value: &Value, vm: &VM<'_>) -> RunResult<Option<Rounding>> {
    if matches!(value, Value::None) {
        Ok(None)
    } else {
        rounding_arg(value, vm).map(Some)
    }
}

/// Runs `op` under the context at `ctx_id` and applies the conditions it raised.
///
/// The closure gets shared access to the VM to read its operands; the context's flags
/// are only updated (and a trap raised) after it returns successfully.
pub(crate) fn with_ctx<'h, T>(
    vm: &mut VM<'h>,
    ctx_id: HeapId,
    op: impl FnOnce(&mut Ctx<'_>, &VM<'h>) -> RunResult<T>,
) -> RunResult<T> {
    let settings = settings_of(vm, ctx_id);
    let (result, status) = {
        let shared: &VM<'h> = vm;
        let mut ctx = Ctx::new(settings, &shared.heap.tracker);
        let result = op(&mut ctx, shared);
        (result, ctx.status)
    };
    let result = result?;
    apply_status(vm, ctx_id, status)?;
    Ok(result)
}

/// Runs a binary operator against the current context, `None` for `NotImplemented`.
///
/// `reflected` swaps the operands, for `__radd__` and friends.
fn binary_operator(
    this: &HeapObjectRead<'_, Decimal>,
    other: &Value,
    reflected: bool,
    vm: &mut VM<'_>,
    op: fn(&Decimal, &Decimal, &mut Ctx<'_>) -> RunResult<Decimal>,
) -> RunResult<Option<Value>> {
    let id = this.id();
    if implicit(other, vm)?.is_none() {
        return Ok(None);
    }
    let ctx_id = current_context(vm);
    let result = with_ctx(vm, ctx_id, |ctx, vm| {
        let this = decimal_at(vm, id);
        let other = implicit(other, vm)?.expect("checked above");
        if reflected {
            op(&other, this, ctx)
        } else {
            op(this, &other, ctx)
        }
    })?;
    Ok(Some(decimal_value(result, vm)))
}

/// The `Decimal` stored at `id`.
fn decimal_at<'a>(vm: &'a VM<'_>, id: HeapId) -> &'a Decimal {
    match vm.heap.get(id) {
        HeapData::Decimal(decimal) => decimal,
        _ => unreachable!("decimal id points at a different object"),
    }
}

// ============================================================================
// Construction
// ============================================================================

/// Arguments of `decimal.Decimal(value='0', context=None)`.
#[derive(FromArgs)]
#[from_args(name = "Decimal", style = c)]
struct DecimalArgs {
    /// `Undefined` stands for the omitted argument, `Decimal()` being `Decimal('0')`.
    #[from_args(default = Value::Undefined)]
    value: Value,
    #[from_args(default = Value::None)]
    context: Value,
}

/// `decimal.Decimal(value='0', context=None)`: an exact conversion. The context only
/// decides whether a malformed string or an `float` raises.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let DecimalArgs { value, context } = DecimalArgs::from_args(args, vm)?;
    defer_drop!(context, vm);
    let ctx_id = context_arg(context, vm);
    let ctx_id = match ctx_id {
        Ok(id) => id,
        Err(err) => {
            value.drop_with(vm);
            return Err(err);
        }
    };
    if matches!(value, Value::Undefined) {
        return Ok(decimal_value(Decimal::zero(false, 0), vm));
    }
    if as_decimal(&value, vm).is_some() {
        return Ok(value);
    }
    defer_drop!(value, vm);
    let decimal = construct(value, ctx_id, vm)?;
    Ok(decimal_value(decimal, vm))
}

/// Converts any value `Decimal()` accepts, raising conversion conditions on `ctx_id`.
fn construct(value: &Value, ctx_id: HeapId, vm: &mut VM<'_>) -> RunResult<Decimal> {
    if let Some(decimal) = as_decimal(value, vm) {
        return Ok(decimal.clone());
    }
    if let Some(text) = value.as_either_str(vm.heap) {
        let parsed = parse::parse_str(text.as_str(vm.interns))?;
        return match parsed {
            Ok(decimal) => Ok(decimal),
            Err(condition) => with_ctx(vm, ctx_id, |ctx, _| Ok(ctx.invalid(condition))),
        };
    }
    match value.py_value(vm) {
        PyValue::Bool(b) => Ok(Decimal::from_i64(i64::from(b))),
        PyValue::Int(int) => int.with_value(vm, |small| Ok(Decimal::from_i64(small)), parse::from_int),
        PyValue::Float(f) => with_ctx(vm, ctx_id, |ctx, _| {
            ctx.raise(Condition::FloatOperation);
            Ok(parse::from_float(f))
        }),
        PyValue::Tuple(_) | PyValue::List(_) | PyValue::NamedTuple(_) => from_tuple(value, ctx_id, vm),
        _ => Err(ExcType::decimal_conversion(&value.py_type_name(vm))),
    }
}

/// `Decimal((sign, digits, exponent))`, with C `_decimal`'s validation.
fn from_tuple(value: &Value, ctx_id: HeapId, vm: &mut VM<'_>) -> RunResult<Decimal> {
    let items = sequence_items(value, vm).expect("caller checked for a sequence");
    defer_drop!(items, vm);
    let [sign, digits, exponent] = items.as_slice() else {
        return Err(ExcType::value_error("argument must be a sequence of length 3"));
    };
    let sign = match sign {
        Value::Int(0) | Value::Bool(false) => false,
        Value::Int(1) | Value::Bool(true) => true,
        _ => return Err(ExcType::value_error("sign must be an integer with the value 0 or 1")),
    };
    let digit_values = sequence_items(digits, vm)
        .filter(|_| !matches!(digits, Value::InternString(_)))
        .ok_or_else(|| ExcType::value_error("coefficient must be a tuple of digits"))?;
    defer_drop!(digit_values, vm);
    let mut coeff = String::with_capacity(digit_values.len());
    for digit in digit_values {
        match digit {
            Value::Int(d @ 0..=9) => {
                if !coeff.is_empty() || *d != 0 {
                    coeff.push(char::from(b'0' + u8::try_from(*d).expect("single digit")));
                }
            }
            _ => return Err(ExcType::value_error("coefficient must be a tuple of digits")),
        }
    }
    parse::check_digit_count(coeff.len())?;
    if let Some(text) = exponent.as_either_str(vm.heap) {
        return match text.as_str(vm.interns) {
            "F" => Ok(Decimal::infinity(sign)),
            "n" => Ok(Decimal::special_nan(
                sign,
                Coeff::from_digits(&coeff),
                Special::QuietNan,
            )),
            "N" => Ok(Decimal::special_nan(
                sign,
                Coeff::from_digits(&coeff),
                Special::SignalingNan,
            )),
            _ => Err(ExcType::value_error(
                "string argument in the third position must be 'F', 'n' or 'N'",
            )),
        };
    }
    let exp = match exponent.py_value(vm) {
        PyValue::Int(int) => int
            .to_i64(vm)
            .ok_or_else(|| ExcType::overflow_error("Python int too large to convert to C ssize_t"))?,
        PyValue::Bool(b) => i64::from(b),
        _ => return Err(ExcType::value_error("exponent must be an integer")),
    };
    if coeff.is_empty() {
        coeff.push('0');
    }
    match parse::checked_exponent(i128::from(exp), coeff.len()) {
        Ok(exp) => Ok(Decimal::from_triple(sign, Coeff::from_digits(&coeff), exp)),
        Err(condition) => with_ctx(vm, ctx_id, |ctx, _| Ok(ctx.invalid(condition))),
    }
}

/// Clones the items of a `tuple` or `list` value.
fn sequence_items(value: &Value, vm: &VM<'_>) -> Option<Vec<Value>> {
    let Value::Ref(id) = value else { return None };
    let items = match vm.heap.get(*id) {
        HeapData::Tuple(tuple) => tuple.as_slice(),
        HeapData::List(list) => list.as_slice(),
        HeapData::NamedTuple(tuple) => tuple.items(),
        _ => return None,
    };
    Some(items.iter().map(|item| item.clone_with_heap(vm.heap)).collect())
}

/// `Decimal.from_float(f)`: exact; an `int` is accepted too.
pub(crate) fn class_from_float(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("Decimal.from_float", vm.heap)?;
    defer_drop!(value, vm);
    let decimal = from_float_arg(value, vm)?;
    Ok(decimal_value(decimal, vm))
}

/// The conversion behind `from_float` and `create_decimal_from_float`.
fn from_float_arg(value: &Value, vm: &VM<'_>) -> RunResult<Decimal> {
    match value.py_value(vm) {
        PyValue::Float(f) => Ok(parse::from_float(f)),
        PyValue::Bool(b) => Ok(Decimal::from_i64(i64::from(b))),
        PyValue::Int(int) => int.with_value(vm, |small| Ok(Decimal::from_i64(small)), parse::from_int),
        _ => Err(ExcType::type_error("argument must be int or float")),
    }
}

/// `Decimal.from_number(number)`: an `int`, `float` or `Decimal`, exactly.
pub(crate) fn class_from_number(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("Decimal.from_number", vm.heap)?;
    if as_decimal(&value, vm).is_some() {
        return Ok(value);
    }
    defer_drop!(value, vm);
    let decimal = match value.py_value(vm) {
        PyValue::Float(_) | PyValue::Int(_) | PyValue::Bool(_) => from_float_arg(value, vm)?,
        _ => return Err(ExcType::decimal_conversion(&value.py_type_name(vm))),
    };
    Ok(decimal_value(decimal, vm))
}

// ============================================================================
// Comparison and hashing
// ============================================================================

/// The operand of a comparison: C `_decimal` also compares against floats (exactly,
/// raising `FloatOperation`) and, for `==`, against complex numbers with no imaginary part.
fn comparison_operand<'a>(
    value: &Value,
    equality: bool,
    vm: &'a VM<'_>,
) -> RunResult<Option<(Cow<'a, Decimal>, bool)>> {
    if let Some(decimal) = implicit(value, vm)? {
        return Ok(Some((decimal, false)));
    }
    Ok(match value.py_value(vm) {
        PyValue::Float(f) => Some((Cow::Owned(parse::from_float(f)), true)),
        PyValue::Complex(c) if equality => {
            let c = *c.get(vm.heap);
            (c.imag == 0.0).then(|| (Cow::Owned(parse::from_float(c.real)), true))
        }
        _ => None,
    })
}

impl<'h> HeapObjectRead<'h, Decimal> {
    /// `self == other`: `None` for `NotImplemented`. A signaling NaN raises
    /// `InvalidOperation`; comparing with a float records `FloatOperation` silently.
    fn rich_eq(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let id = self.id();
        if comparison_operand(other, true, vm)?.is_none() {
            return Ok(None);
        }
        let ctx_id = current_context(vm);
        let (equal, float) = {
            let shared: &VM<'h> = vm;
            let settings = settings_of(shared, ctx_id);
            let mut ctx = Ctx::new(settings, &shared.heap.tracker);
            let this = decimal_at(shared, id);
            let (other, float) = comparison_operand(other, true, shared)?.expect("checked above");
            let equal = if this.check_nans(Some(&other), &mut ctx).is_some() {
                false
            } else {
                this.cmp_numeric(&other) == Ordering::Equal
            };
            ((equal, ctx.status), float)
        };
        let (equal, mut status) = equal;
        // Equality with a float sets the flag but never traps.
        if float {
            let mut settings = settings_of(vm, ctx_id);
            settings.flags.insert(Condition::FloatOperation);
            update_flags(vm, ctx_id, settings);
        }
        if !status.is_empty() {
            apply_status(vm, ctx_id, mem::take(&mut status))?;
        }
        Ok(Some(equal))
    }

    /// An ordering comparison: `None` for `NotImplemented`. Any NaN operand raises
    /// `InvalidOperation` (`False` when untrapped); a float operand raises `FloatOperation`.
    fn ordering(&self, other: &Value, op: CmpOperator, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(match ordering_against(self.id(), other, false, vm)? {
            Some(CmpOrder::Ordered(ord)) => Some(match op {
                CmpOperator::Lt => ord.is_lt(),
                CmpOperator::LtE => ord.is_le(),
                CmpOperator::Gt => ord.is_gt(),
                CmpOperator::GtE => ord.is_ge(),
                _ => unreachable!("ordering called with a non-ordering operator"),
            }),
            Some(_) => Some(false),
            None => None,
        })
    }
}

/// Orders the `Decimal` at `id` against `other` as `<` does, signalling on the current
/// context; `reflected` reverses the result for `other < decimal`. `None` when
/// `other` is not a number a `Decimal` compares with.
fn ordering_against(id: HeapId, other: &Value, reflected: bool, vm: &mut VM<'_>) -> RunResult<Option<CmpOrder>> {
    if comparison_operand(other, false, vm)?.is_none() {
        return Ok(None);
    }
    let ctx_id = current_context(vm);
    with_ctx(vm, ctx_id, |ctx, vm| {
        let this = decimal_at(vm, id);
        let (other, float) = comparison_operand(other, false, vm)?.expect("checked above");
        if float {
            ctx.raise(Condition::FloatOperation);
        }
        Ok(Some(if this.compare_check_nans(&other, ctx).is_some() {
            CmpOrder::Unordered
        } else {
            let ord = this.cmp_numeric(&other);
            CmpOrder::Ordered(if reflected { ord.reverse() } else { ord })
        }))
    })
}

/// The ordering `Value::py_cmp` uses (sorting, `min`, `max`, sequence comparison)
/// when either operand is a `Decimal`; `None` when neither is.
pub(crate) fn cmp_order(lhs: &Value, rhs: &Value, vm: &mut VM<'_>) -> RunResult<Option<CmpOrder>> {
    let (id, other, reflected) = match (lhs, rhs) {
        (Value::Ref(id), _) if matches!(vm.heap.get(*id), HeapData::Decimal(_)) => (*id, rhs, false),
        (_, Value::Ref(id)) if matches!(vm.heap.get(*id), HeapData::Decimal(_)) => (*id, lhs, true),
        _ => return Ok(None),
    };
    Ok(Some(
        ordering_against(id, other, reflected, vm)?.unwrap_or(CmpOrder::Incomparable),
    ))
}

/// Stores new settings on the context at `id` without checking traps.
fn update_flags(vm: &mut VM<'_>, id: HeapId, settings: DecContext) {
    let context = vm.heap.read_as::<DecContext>(id).expect("decimal context id");
    *context.get_mut(vm.heap) = settings;
}

/// Hashes so that a `Decimal` equal to an `int` or `float` hashes like it.
///
/// Integral values hash as the integer, values a double represents exactly as that
/// float, and anything else by its normalized digits. Integers too long to
/// materialize cheaply hash by their digits too, which breaks `hash(d) == hash(n)`
/// only for integers beyond [`HASH_INTEGER_DIGITS`] digits.
fn hash_decimal(decimal: &Decimal, id: HeapId) -> RunResult<HashValue> {
    match decimal.special() {
        Special::SignalingNan => return Err(ExcType::type_error("Cannot hash a signaling NaN value")),
        Special::QuietNan => return Ok(identity_hash(id)),
        Special::Infinite => {
            return Ok(hash_f64(if decimal.sign() {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }));
        }
        Special::Finite => {}
    }
    if decimal.is_zero() {
        return Ok(HashValue::new(0));
    }
    // Strip trailing zeros so equal values with different exponents hash alike.
    let zeros = decimal.coeff().trailing_zeros();
    let coeff = if zeros == 0 {
        Cow::Borrowed(decimal.coeff())
    } else {
        Cow::Owned(decimal.coeff().shr(zeros).0)
    };
    let exp = decimal.exponent() + i64::try_from(zeros).expect("bounded by the coefficient length");
    let digits = i64::try_from(coeff.digits()).expect("digit count fits in i64");
    if exp >= 0 && digits + exp <= HASH_INTEGER_DIGITS {
        let magnitude = coeff.to_bigint() * super::pow10(exp.unsigned_abs());
        let value = if decimal.sign() { -magnitude } else { magnitude };
        return Ok(hash_python_long_int(&value));
    }
    if exp < 0
        && let Some(f) = exact_fraction_float(&coeff, exp)
    {
        return Ok(hash_f64(if decimal.sign() { -f } else { f }));
    }
    let seed = exp.cast_unsigned() ^ (u64::from(decimal.sign()) << 63);
    Ok(HashValue::new(coeff.digest(seed)))
}

/// Integral values up to this many digits hash exactly like the equal `int`.
const HASH_INTEGER_DIGITS: i64 = 5000;

/// The float exactly equal to `coeff * 10**exp` for `exp < 0` and a coefficient
/// without trailing zeros, or `None` when no float is.
///
/// A non-integral float is `m * 2**-k` with `m` odd and below `2**53`, which is
/// `m * 5**k * 10**-k`: the digits must end in `5` and divide by `5**k`, so most
/// values are rejected by their last digit alone.
#[expect(clippy::cast_precision_loss, reason = "m is below 2**53, so the cast is exact")]
fn exact_fraction_float(coeff: &Coeff, exp: i64) -> Option<f64> {
    // The smallest positive float is `2**-1074`.
    const MAX_K: u64 = 1074;
    let k = exp.unsigned_abs();
    if coeff.last_digit() != 5 || k > MAX_K {
        return None;
    }
    // `m * 5**k` has between `len(5**k)` and `len(5**k) + 16` digits; the window is
    // padded by one on each side for the integer estimate of `k * log10(5)`.
    let five_len = k * 69_897 / 100_000 + 1;
    let len = coeff.digits();
    if len + 1 < five_len || len > five_len + 17 {
        return None;
    }
    let m = if k <= 55
        && let Some(c) = coeff.to_u128()
    {
        // `5**55` fits a `u128`.
        let five_pow = 5u128.pow(u32::try_from(k).ok()?);
        if !c.is_multiple_of(five_pow) {
            return None;
        }
        u64::try_from(c / five_pow).ok()?
    } else {
        let (m, remainder) = coeff.to_bigint().div_rem(&Pow::pow(BigInt::from(5u8), k));
        if !remainder.is_zero() {
            return None;
        }
        m.to_u64()?
    };
    // `ldexp` is exact here: `m` fits the mantissa and `k` stays within the subnormals.
    let shift = -i32::try_from(k).ok()?;
    (m < 1 << 53).then(|| libm::ldexp(m as f64, shift))
}

// ============================================================================
// PyTrait
// ============================================================================

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Decimal> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Decimal
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        self.rich_eq(other, vm)
    }

    fn py_cmp_op(&self, other: &Value, op: CmpOperator, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        self.ordering(other, op, vm)
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        hash_decimal(self.get(vm.heap), self.id()).map(Some)
    }

    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(self.get(vm.heap).is_truthy())
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let ctx_id = current_context(vm);
        let capitals = settings_of(vm, ctx_id).capitals;
        let text = self.get(vm.heap).to_sci_string(false, capitals);
        Ok(write!(f, "Decimal('{text}')")?)
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let ctx_id = current_context(vm);
        let capitals = settings_of(vm, ctx_id).capitals;
        let text = self.get(vm.heap).to_sci_string(false, capitals);
        Ok(allocate_string(text, vm.heap))
    }

    fn py_abs_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.unary_operator(vm, Decimal::abs).map(Some)
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.unary_operator(vm, Decimal::neg).map(Some)
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.unary_operator(vm, Decimal::plus).map(Some)
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, false, vm, Decimal::add)
    }

    fn py_radd_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, true, vm, Decimal::add)
    }

    fn py_sub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, false, vm, Decimal::sub)
    }

    fn py_rsub_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, true, vm, Decimal::sub)
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, false, vm, Decimal::mul)
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, true, vm, Decimal::mul)
    }

    fn py_truediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, false, vm, Decimal::div)
    }

    fn py_rtruediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, true, vm, Decimal::div)
    }

    fn py_floordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, false, vm, Decimal::div_int)
    }

    fn py_rfloordiv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, true, vm, Decimal::div_int)
    }

    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, false, vm, Decimal::rem)
    }

    fn py_rmod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        binary_operator(self, other, true, vm, Decimal::rem)
    }

    fn py_divmod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.divmod_operator(other, false, vm)
    }

    fn py_rdivmod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.divmod_operator(other, true, vm)
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.pow_operator(other, modulus, false, vm)
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.pow_operator(other, modulus, true, vm)
    }

    fn py_round_impl(&self, ndigits: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.round(ndigits, vm).map(Some)
    }

    fn py_int_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.to_int_value(Rounding::Down, vm).map(Some)
    }

    fn py_trunc_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.to_int_value(Rounding::Down, vm).map(Some)
    }

    fn py_floor_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.to_int_value(Rounding::Floor, vm).map(Some)
    }

    fn py_ceil_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.to_int_value(Rounding::Ceiling, vm).map(Some)
    }

    fn py_float_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<f64>> {
        self.get(vm.heap).to_f64().map(Some)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let value = match attr.as_str(vm.interns) {
            "real" => self.clone_value(vm.heap),
            "imag" => decimal_value(Decimal::zero(false, 0), vm),
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        value.drop_with(vm);
        let name = name.as_str(vm.interns);
        Err(match name {
            "real" | "imag" => ExcType::attribute_error_not_writable(name, &self.py_type_name(vm)),
            _ => ExcType::attribute_error_no_setattr(&self.py_type_name(vm), name),
        })
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let name = attr.as_str(vm.interns);
        match name.parse::<DecimalMethod>() {
            Ok(method) => self.call_method(method, args, vm).map(CallResult::Value),
            Err(_) => Err(ExcType::attribute_error_method(Type::Decimal, attr, args, vm)),
        }
    }
}

impl<'h> HeapObjectRead<'h, Decimal> {
    /// A unary operation (`-d`, `+d`, `abs(d)`) under the current context.
    fn unary_operator(
        &self,
        vm: &mut VM<'h>,
        op: impl FnOnce(&Decimal, &mut Ctx<'_>) -> RunResult<Decimal>,
    ) -> RunResult<Value> {
        let id = self.id();
        let ctx_id = current_context(vm);
        let result = with_ctx(vm, ctx_id, |ctx, vm| op(decimal_at(vm, id), ctx))?;
        Ok(decimal_value(result, vm))
    }

    /// `divmod(self, other)` or, reflected, `divmod(other, self)`.
    fn divmod_operator(&self, other: &Value, reflected: bool, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let id = self.id();
        if implicit(other, vm)?.is_none() {
            return Ok(None);
        }
        let ctx_id = current_context(vm);
        let (q, r) = with_ctx(vm, ctx_id, |ctx, vm| {
            let this = decimal_at(vm, id);
            let other = implicit(other, vm)?.expect("checked above");
            if reflected {
                other.divmod(this, ctx)
            } else {
                this.divmod(&other, ctx)
            }
        })?;
        let pair = smallvec![decimal_value(q, vm), decimal_value(r, vm)];
        Ok(Some(allocate_tuple(pair, vm.heap)))
    }

    /// `self ** other [% modulus]`, or reflected `other ** self [% modulus]`.
    fn pow_operator(
        &self,
        other: &Value,
        modulus: Option<&Value>,
        reflected: bool,
        vm: &mut VM<'h>,
    ) -> RunResult<Option<Value>> {
        let id = self.id();
        if implicit(other, vm)?.is_none() {
            return Ok(None);
        }
        if let Some(modulus) = modulus
            && implicit(modulus, vm)?.is_none()
        {
            return Ok(None);
        }
        let ctx_id = current_context(vm);
        let result = with_ctx(vm, ctx_id, |ctx, vm| {
            let this = decimal_at(vm, id);
            let other = implicit(other, vm)?.expect("checked above");
            let modulus = modulus.map(|m| implicit(m, vm)).transpose()?.flatten();
            if reflected {
                other.pow(this, modulus.as_deref(), ctx)
            } else {
                this.pow(&other, modulus.as_deref(), ctx)
            }
        })?;
        Ok(Some(decimal_value(result, vm)))
    }

    /// `round(d)` to an `int`, or `round(d, n)` to a `Decimal` quantized to `10**-n`.
    fn round(&self, ndigits: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Value> {
        let Some(ndigits) = ndigits.filter(|n| !matches!(n, Value::None)) else {
            return self.to_int_value(Rounding::HalfEven, vm);
        };
        let n = match ndigits.py_value(vm) {
            PyValue::Int(int) => int.to_i64(vm),
            PyValue::Bool(b) => Some(i64::from(b)),
            _ => return Err(ExcType::type_error("optional arg must be an integer")),
        }
        .ok_or_else(|| ExcType::overflow_error("Python int too large to convert to C ssize_t"))?;
        let id = self.id();
        let ctx_id = current_context(vm);
        let result = with_ctx(vm, ctx_id, |ctx, vm| decimal_at(vm, id).round_digits(n, ctx))?;
        Ok(decimal_value(result, vm))
    }

    /// `int(d)`, `math.floor(d)` and friends: an exact integer rounded with `rounding`.
    fn to_int_value(&self, rounding: Rounding, vm: &mut VM<'h>) -> RunResult<Value> {
        let int = self.get(vm.heap).to_integer(rounding, &vm.heap.tracker)?;
        Ok(LongInt::new(int).into_value(vm.heap))
    }

    /// Calls a `Decimal` method.
    fn call_method(&self, method: DecimalMethod, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let id = self.id();
        let qualname = |method: DecimalMethod| format!("Decimal.{}", <&'static str>::from(method));
        match method.shape() {
            MethodShape::NoArgs => {
                args.check_zero_args(&qualname(method), vm.heap)?;
                self.no_arg_method(method, vm)
            }
            MethodShape::Context => {
                let ContextOnlyArgs { context } = ContextOnlyArgs::from_args(args, vm)?;
                defer_drop!(context, vm);
                let ctx_id = context_arg(context, vm)?;
                let unary = method.unary().expect("context-only methods are unary ops");
                let result = with_ctx(vm, ctx_id, |ctx, vm| unary_op(unary, decimal_at(vm, id), ctx))?;
                Ok(result.into_value(vm))
            }
            MethodShape::Other => {
                let OtherArgs { other, context } = OtherArgs::from_args(args, vm)?;
                defer_drop!(other, vm);
                defer_drop!(context, vm);
                let ctx_id = context_arg(context, vm)?;
                let binary = method.binary().expect("binary methods are binary ops");
                let result = with_ctx(vm, ctx_id, |ctx, vm| {
                    let other = convert(other, vm)?;
                    binary_op(binary, decimal_at(vm, id), &other, ctx)
                })?;
                Ok(result.into_value(vm))
            }
            MethodShape::Special => self.special_method(method, args, vm),
        }
    }

    /// The methods taking no arguments.
    fn no_arg_method(&self, method: DecimalMethod, vm: &mut VM<'h>) -> RunResult<Value> {
        let d = self.get(vm.heap);
        Ok(match method {
            DecimalMethod::Adjusted => Value::Int(d.adjusted()),
            DecimalMethod::AsIntegerRatio => {
                let (n, den) = d.as_integer_ratio(&Ctx::new(DecContext::DEFAULT, &vm.heap.tracker))?;
                let pair = smallvec![
                    LongInt::new(n).into_value(vm.heap),
                    LongInt::new(den).into_value(vm.heap)
                ];
                allocate_tuple(pair, vm.heap)
            }
            DecimalMethod::AsTuple => as_tuple(d, vm),
            DecimalMethod::Canonical | DecimalMethod::Conjugate | DecimalMethod::DunderCopy => {
                self.clone_value(vm.heap)
            }
            DecimalMethod::CopyAbs => decimal_value(d.copy_abs(), vm),
            DecimalMethod::CopyNegate => decimal_value(d.copy_negate(), vm),
            DecimalMethod::IsCanonical => Value::Bool(true),
            DecimalMethod::IsFinite => Value::Bool(d.is_finite()),
            DecimalMethod::IsInfinite => Value::Bool(d.is_infinite()),
            DecimalMethod::IsNan => Value::Bool(d.is_nan()),
            DecimalMethod::IsQnan => Value::Bool(d.is_qnan()),
            DecimalMethod::IsSigned => Value::Bool(d.sign()),
            DecimalMethod::IsSnan => Value::Bool(d.is_snan()),
            DecimalMethod::IsZero => Value::Bool(d.is_zero()),
            DecimalMethod::Radix => decimal_value(Decimal::from_i64(10), vm),
            DecimalMethod::DunderReduce => {
                let text = allocate_string(d.to_sci_string(false, true), vm.heap);
                let args = allocate_tuple(smallvec![text], vm.heap);
                allocate_tuple(
                    smallvec![Value::Builtin(Builtins::Type(BuiltinType::Decimal)), args],
                    vm.heap,
                )
            }
            DecimalMethod::DunderComplex => {
                let real = d.to_f64()?;
                Complex::new(real, 0.0).into_value(vm.heap)
            }
            DecimalMethod::DunderFloat => Value::Float(d.to_f64()?),
            DecimalMethod::DunderInt | DecimalMethod::DunderTrunc => self.to_int_value(Rounding::Down, vm)?,
            DecimalMethod::DunderFloor => self.to_int_value(Rounding::Floor, vm)?,
            DecimalMethod::DunderCeil => self.to_int_value(Rounding::Ceiling, vm)?,
            _ => unreachable!("not a zero-argument method"),
        })
    }

    /// The methods with their own signatures.
    fn special_method(&self, method: DecimalMethod, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
        let id = self.id();
        match method {
            DecimalMethod::Quantize => {
                let QuantizeArgs { exp, rounding, context } = QuantizeArgs::from_args(args, vm)?;
                let fields = [exp, rounding, context];
                defer_drop!(fields, vm);
                let [exp, rounding, context] = fields;
                let rounding = optional_rounding(rounding, vm)?;
                let ctx_id = context_arg(context, vm)?;
                let result = with_ctx(vm, ctx_id, |ctx, vm| {
                    let exp = convert(exp, vm)?;
                    decimal_at(vm, id).quantize(&exp, rounding, ctx)
                })?;
                Ok(decimal_value(result, vm))
            }
            DecimalMethod::Fma => {
                let FmaArgs { other, third, context } = FmaArgs::from_args(args, vm)?;
                let fields = [other, third, context];
                defer_drop!(fields, vm);
                let [other, third, context] = fields;
                let ctx_id = context_arg(context, vm)?;
                let result = with_ctx(vm, ctx_id, |ctx, vm| {
                    let other = convert(other, vm)?;
                    let third = convert(third, vm)?;
                    decimal_at(vm, id).fma(&other, &third, ctx)
                })?;
                Ok(decimal_value(result, vm))
            }
            DecimalMethod::ToIntegral | DecimalMethod::ToIntegralExact | DecimalMethod::ToIntegralValue => {
                let RoundingArgs { rounding, context } = RoundingArgs::from_args(args, vm)?;
                let fields = [rounding, context];
                defer_drop!(fields, vm);
                let [rounding, context] = fields;
                let rounding = optional_rounding(rounding, vm)?;
                let ctx_id = context_arg(context, vm)?;
                let exact = method == DecimalMethod::ToIntegralExact;
                let result = with_ctx(vm, ctx_id, |ctx, vm| {
                    let d = decimal_at(vm, id);
                    if exact {
                        d.to_integral_exact(rounding, ctx)
                    } else {
                        d.to_integral_value(rounding, ctx)
                    }
                })?;
                Ok(decimal_value(result, vm))
            }
            DecimalMethod::DunderFormat => {
                let (spec, override_) = format_args(args, vm)?;
                defer_drop!(spec, vm);
                defer_drop!(override_, vm);
                // C `_decimal`'s locale override (`thousands_sep`, `decimal_point`,
                // `grouping`) is checked but not applied: see limitations/decimal.md.
                if let Some(override_) = override_
                    && !matches!(override_, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Dict(_)))
                {
                    return Err(ExcType::type_error("optional argument must be a dict"));
                }
                let Some(spec) = spec.as_either_str(vm.heap) else {
                    return Err(ExcType::type_error("format arg must be str"));
                };
                let spec = spec.as_str(vm.interns).to_owned();
                self.format(&spec, vm)
            }
            DecimalMethod::DunderRound => {
                let n = args
                    .get_zero_one_arg("function", vm.heap)
                    .map_err(|_| ExcType::type_error("function takes at most 1 argument (2 given)"))?;
                defer_drop!(n, vm);
                self.round(n.as_ref(), vm)
            }
            DecimalMethod::DunderDeepcopy => {
                let memo = args.get_one_arg("Decimal.__deepcopy__", vm.heap)?;
                memo.drop_with(vm);
                Ok(self.clone_value(vm.heap))
            }
            _ => unreachable!("not a special-signature method"),
        }
    }

    /// `format(d, spec)` / f-string formatting under the current context.
    pub(crate) fn format(&self, spec: &str, vm: &mut VM<'h>) -> RunResult<Value> {
        let id = self.id();
        let ctx_id = current_context(vm);
        let text = with_ctx(vm, ctx_id, |ctx, vm| decimal_at(vm, id).format_spec(spec, ctx))?;
        Ok(allocate_string_no_interning(text, vm.heap))
    }
}

/// `__format__(spec, override=None)`'s positional-only arguments.
fn format_args(args: ArgValues, vm: &mut VM<'_>) -> RunResult<(Value, Option<Value>)> {
    match args {
        ArgValues::Empty => Err(ExcType::type_error("function takes at least 1 argument (0 given)")),
        other => {
            let count = other.count();
            if count > 2 {
                other.drop_with(vm);
                Err(ExcType::type_error(format!(
                    "function takes at most 2 arguments ({count} given)"
                )))
            } else {
                other.get_one_two_args("__format__", vm.heap)
            }
        }
    }
}

/// `as_tuple()`: `DecimalTuple(sign, digits, exponent)`.
fn as_tuple(d: &Decimal, vm: &VM<'_>) -> Value {
    let digits: smallvec::SmallVec<[Value; 3]> = d
        .coeff_digits()
        .bytes()
        .map(|b| Value::Int(i64::from(b - b'0')))
        .collect();
    let digits = allocate_tuple(digits.into_iter().collect(), vm.heap);
    let exponent = match d.special() {
        Special::Finite => Value::Int(d.exponent()),
        Special::Infinite => allocate_string("F", vm.heap),
        Special::QuietNan => allocate_string("n", vm.heap),
        Special::SignalingNan => allocate_string("N", vm.heap),
    };
    let fields = vec![
        EitherStr::Interned(vm.interns.intern_static(StaticStrings::Sign)),
        EitherStr::Interned(vm.interns.intern_static(StaticStrings::Digits)),
        EitherStr::Interned(vm.interns.intern_static(StaticStrings::Exponent)),
    ];
    let name = EitherStr::Interned(vm.interns.intern_static(StaticStrings::DecimalTuple));
    let tuple = NamedTuple::new(name, fields, vec![Value::Int(i64::from(d.sign())), digits, exponent]);
    Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(tuple))))
}

/// Arguments of the methods taking only `context=None`.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct ContextOnlyArgs {
    #[from_args(default = Value::None)]
    context: Value,
}

/// Arguments of the binary methods, `(other, context=None)`.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct OtherArgs {
    other: Value,
    #[from_args(default = Value::None)]
    context: Value,
}

/// Arguments of `quantize(exp, rounding=None, context=None)`.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct QuantizeArgs {
    exp: Value,
    #[from_args(default = Value::None)]
    rounding: Value,
    #[from_args(default = Value::None)]
    context: Value,
}

/// Arguments of `fma(other, third, context=None)`.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct FmaArgs {
    other: Value,
    third: Value,
    #[from_args(default = Value::None)]
    context: Value,
}

/// Arguments of the `to_integral*` methods, `(rounding=None, context=None)`.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct RoundingArgs {
    #[from_args(default = Value::None)]
    rounding: Value,
    #[from_args(default = Value::None)]
    context: Value,
}

// ============================================================================
// Shared operations
// ============================================================================

/// The result of a method shared by `Decimal` and `Context`.
pub(crate) enum OpResult {
    Decimal(Decimal),
    Bool(bool),
    Str(String),
    Pair(Decimal, Decimal),
}

impl OpResult {
    /// Converts to a Python value.
    fn into_value(self, vm: &VM<'_>) -> Value {
        match self {
            Self::Decimal(d) => decimal_value(d, vm),
            Self::Bool(b) => Value::Bool(b),
            Self::Str(s) => allocate_string(s, vm.heap),
            Self::Pair(a, b) => allocate_tuple(smallvec![decimal_value(a, vm), decimal_value(b, vm)], vm.heap),
        }
    }
}

/// One-operand operations, named as the `Context` methods are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum UnaryOp {
    Abs,
    Canonical,
    CopyAbs,
    CopyDecimal,
    CopyNegate,
    Exp,
    IsCanonical,
    IsFinite,
    IsInfinite,
    IsNan,
    IsNormal,
    IsQnan,
    IsSigned,
    IsSnan,
    IsSubnormal,
    IsZero,
    Ln,
    Log10,
    Logb,
    LogicalInvert,
    Minus,
    NextMinus,
    NextPlus,
    Normalize,
    NumberClass,
    Plus,
    Sqrt,
    ToEngString,
    ToIntegral,
    ToIntegralExact,
    ToIntegralValue,
    ToSciString,
    #[strum(serialize = "_apply")]
    Apply,
}

/// Runs a one-operand operation.
fn unary_op(op: UnaryOp, a: &Decimal, ctx: &mut Ctx<'_>) -> RunResult<OpResult> {
    Ok(OpResult::Decimal(match op {
        UnaryOp::Abs => a.abs(ctx)?,
        UnaryOp::Canonical | UnaryOp::CopyDecimal => a.clone(),
        UnaryOp::CopyAbs => a.copy_abs(),
        UnaryOp::CopyNegate => a.copy_negate(),
        UnaryOp::Exp => a.exp(ctx)?,
        UnaryOp::Ln => a.ln(ctx)?,
        UnaryOp::Log10 => a.log10(ctx)?,
        UnaryOp::Logb => a.logb(ctx)?,
        UnaryOp::LogicalInvert => a.logical_invert(ctx)?,
        UnaryOp::Minus => a.neg(ctx)?,
        UnaryOp::NextMinus => a.next_minus(ctx)?,
        UnaryOp::NextPlus => a.next_plus(ctx)?,
        UnaryOp::Normalize => a.normalize(ctx)?,
        UnaryOp::Plus => a.plus(ctx)?,
        UnaryOp::Apply => a.fix(ctx)?,
        UnaryOp::Sqrt => a.sqrt(ctx)?,
        UnaryOp::ToIntegral | UnaryOp::ToIntegralValue => a.to_integral_value(None, ctx)?,
        UnaryOp::ToIntegralExact => a.to_integral_exact(None, ctx)?,
        UnaryOp::IsCanonical => return Ok(OpResult::Bool(true)),
        UnaryOp::IsFinite => return Ok(OpResult::Bool(a.is_finite())),
        UnaryOp::IsInfinite => return Ok(OpResult::Bool(a.is_infinite())),
        UnaryOp::IsNan => return Ok(OpResult::Bool(a.is_nan())),
        UnaryOp::IsNormal => return Ok(OpResult::Bool(a.is_normal(&ctx.settings))),
        UnaryOp::IsQnan => return Ok(OpResult::Bool(a.is_qnan())),
        UnaryOp::IsSigned => return Ok(OpResult::Bool(a.sign())),
        UnaryOp::IsSnan => return Ok(OpResult::Bool(a.is_snan())),
        UnaryOp::IsSubnormal => return Ok(OpResult::Bool(a.is_subnormal(&ctx.settings))),
        UnaryOp::IsZero => return Ok(OpResult::Bool(a.is_zero())),
        UnaryOp::NumberClass => return Ok(OpResult::Str(a.number_class(&ctx.settings).to_owned())),
        UnaryOp::ToEngString => return Ok(OpResult::Str(a.to_sci_string(true, ctx.settings.capitals))),
        UnaryOp::ToSciString => return Ok(OpResult::Str(a.to_sci_string(false, ctx.settings.capitals))),
    }))
}

/// Two-operand operations, named as the `Context` methods are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum BinaryOp {
    Add,
    Compare,
    CompareSignal,
    CompareTotal,
    CompareTotalMag,
    CopySign,
    Divide,
    DivideInt,
    Divmod,
    LogicalAnd,
    LogicalOr,
    LogicalXor,
    Max,
    MaxMag,
    Min,
    MinMag,
    Multiply,
    NextToward,
    Quantize,
    Remainder,
    RemainderNear,
    Rotate,
    SameQuantum,
    Scaleb,
    Shift,
    Subtract,
}

/// Runs a two-operand operation.
fn binary_op(op: BinaryOp, a: &Decimal, b: &Decimal, ctx: &mut Ctx<'_>) -> RunResult<OpResult> {
    Ok(OpResult::Decimal(match op {
        BinaryOp::Add => a.add(b, ctx)?,
        BinaryOp::Subtract => a.sub(b, ctx)?,
        BinaryOp::Multiply => a.mul(b, ctx)?,
        BinaryOp::Divide => a.div(b, ctx)?,
        BinaryOp::DivideInt => a.div_int(b, ctx)?,
        BinaryOp::Remainder => a.rem(b, ctx)?,
        BinaryOp::RemainderNear => a.remainder_near(b, ctx)?,
        BinaryOp::Divmod => {
            let (q, r) = a.divmod(b, ctx)?;
            return Ok(OpResult::Pair(q, r));
        }
        BinaryOp::Compare => a.compare(b, ctx),
        BinaryOp::CompareSignal => a.compare_signal(b, ctx),
        BinaryOp::CompareTotal => a.compare_total(b),
        BinaryOp::CompareTotalMag => a.compare_total_mag(b),
        BinaryOp::CopySign => a.copy_sign(b),
        BinaryOp::LogicalAnd => a.logical_and(b, ctx)?,
        BinaryOp::LogicalOr => a.logical_or(b, ctx)?,
        BinaryOp::LogicalXor => a.logical_xor(b, ctx)?,
        BinaryOp::Max => a.max(b, ctx)?,
        BinaryOp::MaxMag => a.max_mag(b, ctx)?,
        BinaryOp::Min => a.min(b, ctx)?,
        BinaryOp::MinMag => a.min_mag(b, ctx)?,
        BinaryOp::NextToward => a.next_toward(b, ctx)?,
        BinaryOp::Quantize => a.quantize(b, None, ctx)?,
        BinaryOp::Rotate => a.rotate(b, ctx)?,
        BinaryOp::SameQuantum => return Ok(OpResult::Bool(a.same_quantum(b))),
        BinaryOp::Scaleb => a.scaleb(b, ctx)?,
        BinaryOp::Shift => a.shift(b, ctx)?,
    }))
}

/// How a `Decimal` method takes its arguments.
enum MethodShape {
    /// No arguments (`adjusted()`).
    NoArgs,
    /// Only `context=None` (`sqrt()`), running a [`UnaryOp`].
    Context,
    /// `(other, context=None)` (`compare()`), running a [`BinaryOp`].
    Other,
    /// A signature of its own (`quantize`, `fma`, `__format__`, ...).
    Special,
}

/// The methods of `decimal.Decimal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum DecimalMethod {
    Adjusted,
    AsIntegerRatio,
    AsTuple,
    Canonical,
    Compare,
    CompareSignal,
    CompareTotal,
    CompareTotalMag,
    Conjugate,
    CopyAbs,
    CopyNegate,
    CopySign,
    Exp,
    Fma,
    IsCanonical,
    IsFinite,
    IsInfinite,
    IsNan,
    IsNormal,
    IsQnan,
    IsSigned,
    IsSnan,
    IsSubnormal,
    IsZero,
    Ln,
    Log10,
    Logb,
    LogicalAnd,
    LogicalInvert,
    LogicalOr,
    LogicalXor,
    Max,
    MaxMag,
    Min,
    MinMag,
    NextMinus,
    NextPlus,
    NextToward,
    Normalize,
    NumberClass,
    Quantize,
    Radix,
    RemainderNear,
    Rotate,
    SameQuantum,
    Scaleb,
    Shift,
    Sqrt,
    ToEngString,
    ToIntegral,
    ToIntegralExact,
    ToIntegralValue,
    #[strum(serialize = "__copy__")]
    DunderCopy,
    #[strum(serialize = "__deepcopy__")]
    DunderDeepcopy,
    #[strum(serialize = "__reduce__")]
    DunderReduce,
    #[strum(serialize = "__format__")]
    DunderFormat,
    #[strum(serialize = "__round__")]
    DunderRound,
    #[strum(serialize = "__complex__")]
    DunderComplex,
    #[strum(serialize = "__float__")]
    DunderFloat,
    #[strum(serialize = "__int__")]
    DunderInt,
    #[strum(serialize = "__trunc__")]
    DunderTrunc,
    #[strum(serialize = "__floor__")]
    DunderFloor,
    #[strum(serialize = "__ceil__")]
    DunderCeil,
}

impl DecimalMethod {
    /// How the method takes its arguments.
    fn shape(self) -> MethodShape {
        if self.unary().is_some() {
            MethodShape::Context
        } else if self.binary().is_some() {
            MethodShape::Other
        } else if matches!(
            self,
            Self::Quantize
                | Self::Fma
                | Self::ToIntegral
                | Self::ToIntegralExact
                | Self::ToIntegralValue
                | Self::DunderFormat
                | Self::DunderRound
                | Self::DunderDeepcopy
        ) {
            MethodShape::Special
        } else {
            MethodShape::NoArgs
        }
    }

    /// The unary operation behind a `(context=None)` method.
    fn unary(self) -> Option<UnaryOp> {
        Some(match self {
            Self::Exp => UnaryOp::Exp,
            Self::Ln => UnaryOp::Ln,
            Self::Log10 => UnaryOp::Log10,
            Self::Logb => UnaryOp::Logb,
            Self::LogicalInvert => UnaryOp::LogicalInvert,
            Self::NextMinus => UnaryOp::NextMinus,
            Self::NextPlus => UnaryOp::NextPlus,
            Self::Normalize => UnaryOp::Normalize,
            Self::NumberClass => UnaryOp::NumberClass,
            Self::Sqrt => UnaryOp::Sqrt,
            Self::ToEngString => UnaryOp::ToEngString,
            Self::IsNormal => UnaryOp::IsNormal,
            Self::IsSubnormal => UnaryOp::IsSubnormal,
            _ => return None,
        })
    }

    /// The binary operation behind an `(other, context=None)` method.
    fn binary(self) -> Option<BinaryOp> {
        Some(match self {
            Self::Compare => BinaryOp::Compare,
            Self::CompareSignal => BinaryOp::CompareSignal,
            Self::CompareTotal => BinaryOp::CompareTotal,
            Self::CompareTotalMag => BinaryOp::CompareTotalMag,
            Self::CopySign => BinaryOp::CopySign,
            Self::LogicalAnd => BinaryOp::LogicalAnd,
            Self::LogicalOr => BinaryOp::LogicalOr,
            Self::LogicalXor => BinaryOp::LogicalXor,
            Self::Max => BinaryOp::Max,
            Self::MaxMag => BinaryOp::MaxMag,
            Self::Min => BinaryOp::Min,
            Self::MinMag => BinaryOp::MinMag,
            Self::NextToward => BinaryOp::NextToward,
            Self::RemainderNear => BinaryOp::RemainderNear,
            Self::Rotate => BinaryOp::Rotate,
            Self::SameQuantum => BinaryOp::SameQuantum,
            Self::Scaleb => BinaryOp::Scaleb,
            Self::Shift => BinaryOp::Shift,
            _ => return None,
        })
    }
}

// ============================================================================
// Context methods
// ============================================================================

/// The arithmetic methods of `decimal.Context`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMethod {
    Unary(UnaryOp),
    Binary(BinaryOp),
    Fma,
    Power,
    CreateDecimal,
    CreateDecimalFromFloat,
    Radix,
}

impl ContextMethod {
    /// Resolves a method name.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        if let Ok(op) = name.parse::<UnaryOp>() {
            Some(Self::Unary(op))
        } else if let Ok(op) = name.parse::<BinaryOp>() {
            Some(Self::Binary(op))
        } else {
            match name {
                "fma" => Some(Self::Fma),
                "power" => Some(Self::Power),
                "create_decimal" => Some(Self::CreateDecimal),
                "create_decimal_from_float" => Some(Self::CreateDecimalFromFloat),
                "radix" => Some(Self::Radix),
                _ => None,
            }
        }
    }
}

/// Arguments of `Context.power(a, b, modulo=None)`.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct PowerArgs {
    a: Value,
    b: Value,
    #[from_args(default = Value::None)]
    modulo: Value,
}

/// Calls a `Context` arithmetic method; the operation runs under that context.
pub(crate) fn call_context_method(
    method: ContextMethod,
    ctx_id: HeapId,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    match method {
        ContextMethod::Unary(op) => {
            let name: &'static str = op.into();
            let qualname = format!("Context.{name}");
            let args = args.reject_kwargs(&qualname, vm.heap)?;
            let a = args.get_one_arg(&qualname, vm.heap)?;
            defer_drop!(a, vm);
            if op == UnaryOp::Canonical && as_decimal(a, vm).is_none() {
                return Err(ExcType::type_error("argument must be a Decimal"));
            }
            let result = with_ctx(vm, ctx_id, |ctx, vm| unary_op(op, &*convert(a, vm)?, ctx))?;
            Ok(result.into_value(vm))
        }
        ContextMethod::Binary(op) => {
            let name: &'static str = op.into();
            let [a, b] = exact_args::<2>(args, name, vm)?;
            let operands = [a, b];
            defer_drop!(operands, vm);
            let [a, b] = operands;
            let result = with_ctx(vm, ctx_id, |ctx, vm| {
                binary_op(op, &*convert(a, vm)?, &*convert(b, vm)?, ctx)
            })?;
            Ok(result.into_value(vm))
        }
        ContextMethod::Fma => {
            let operands = exact_args::<3>(args, "fma", vm)?;
            defer_drop!(operands, vm);
            let [a, b, c] = operands;
            let result = with_ctx(vm, ctx_id, |ctx, vm| {
                convert(a, vm)?.fma(&*convert(b, vm)?, &*convert(c, vm)?, ctx)
            })?;
            Ok(decimal_value(result, vm))
        }
        ContextMethod::Power => {
            let PowerArgs { a, b, modulo } = PowerArgs::from_args(args, vm)?;
            let operands = [a, b, modulo];
            defer_drop!(operands, vm);
            let [a, b, modulo] = operands;
            let result = with_ctx(vm, ctx_id, |ctx, vm| {
                let modulo = if matches!(modulo, Value::None) {
                    None
                } else {
                    Some(convert(modulo, vm)?.into_owned())
                };
                convert(a, vm)?.pow(&*convert(b, vm)?, modulo.as_ref(), ctx)
            })?;
            Ok(decimal_value(result, vm))
        }
        ContextMethod::CreateDecimal => {
            let args = args.reject_kwargs("create_decimal", vm.heap)?;
            let num = args
                .get_zero_one_arg("create_decimal", vm.heap)
                .map_err(|_| ExcType::type_error("function takes at most 1 argument (2 given)"))?;
            defer_drop!(num, vm);
            let exact = match num {
                Some(num) => {
                    // A float converts silently-unless-trapped; everything else as `Decimal()`.
                    if let PyValue::Float(f) = num.py_value(vm) {
                        with_ctx(vm, ctx_id, |ctx, _| {
                            ctx.raise(Condition::FloatOperation);
                            Ok(parse::from_float(f))
                        })?
                    } else {
                        construct(num, ctx_id, vm)?
                    }
                }
                None => Decimal::zero(false, 0),
            };
            // Unlike `Decimal()`, the result is rounded to the context.
            let result = with_ctx(vm, ctx_id, |ctx, _| create_fix(&exact, ctx))?;
            Ok(decimal_value(result, vm))
        }
        ContextMethod::CreateDecimalFromFloat => {
            let f = args.reject_kwargs("Context.create_decimal_from_float", vm.heap)?;
            let f = f.get_one_arg("Context.create_decimal_from_float", vm.heap)?;
            defer_drop!(f, vm);
            let exact = from_float_arg(f, vm)?;
            let result = with_ctx(vm, ctx_id, |ctx, _| create_fix(&exact, ctx))?;
            Ok(decimal_value(result, vm))
        }
        ContextMethod::Radix => {
            args.check_zero_args("Context.radix", vm.heap)?;
            Ok(decimal_value(Decimal::from_i64(10), vm))
        }
    }
}

/// `create_decimal`'s rounding: `_fix`, except that a NaN payload longer than the
/// precision is invalid rather than truncated.
fn create_fix(value: &Decimal, ctx: &mut Ctx<'_>) -> RunResult<Decimal> {
    let max_payload = ctx.settings.prec - i64::from(ctx.settings.clamp);
    if value.is_nan() && value.digits() > max_payload {
        Ok(ctx.invalid(Condition::ConversionSyntax))
    } else {
        value.fix(ctx)
    }
}

/// Unpacks exactly `N` positional arguments with C `PyArg_ParseTuple`'s wording.
fn exact_args<const N: usize>(args: ArgValues, name: &str, vm: &mut VM<'_>) -> RunResult<[Value; N]> {
    let args = args.reject_kwargs(name, vm.heap)?;
    let count = args.count();
    if count != N {
        args.drop_with(vm);
        return Err(ExcType::type_error_function_exact(N, count));
    }
    let values: Vec<Value> = args.into_pos_only(name, vm.heap)?.collect();
    Ok(<[Value; N]>::try_from(values).unwrap_or_else(|_| unreachable!("count checked above")))
}

impl Decimal {
    /// `int(d)` with the given rounding: `ValueError` for a NaN, `OverflowError` for an
    /// infinity, as C `_decimal` raises.
    pub(crate) fn to_integer(&self, rounding: Rounding, tracker: &monty_types::ResourceTracker) -> RunResult<BigInt> {
        match self.special() {
            Special::QuietNan | Special::SignalingNan => Err(ExcType::value_error("cannot convert NaN to integer")),
            Special::Infinite => Err(ExcType::overflow_error("cannot convert Infinity to integer")),
            Special::Finite => {
                let settings = DecContext {
                    rounding,
                    ..DecContext::DEFAULT
                };
                let ctx = Ctx::new(settings, tracker);
                let integral = self.rescale(0, rounding, &ctx)?;
                parse::to_integer(&integral, &ctx)
            }
        }
    }
}
