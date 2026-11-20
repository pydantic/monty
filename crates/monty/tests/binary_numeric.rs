use monty::MontyRun;
use monty_types::{CompileOptions, ExcType};

#[test]
fn numeric_fast_paths_and_fallbacks() {
    let code = r"
def check(a, b, add, sub, mul):
    assert a + b == add
    assert a - b == sub
    assert a * b == mul
    assert type(a + b) == type(add)
    result = a
    result += b
    assert result == add
    assert type(result) == type(add)
    assert type(a - b) == type(sub)
    result = a
    result -= b
    assert result == sub
    assert type(result) == type(sub)
    assert type(a * b) == type(mul)
    result = a
    result *= b
    assert result == mul
    assert type(result) == type(mul)

check(7, 2, 9, 5, 14)
check(7.0, 2.0, 9.0, 5.0, 14.0)
check(7, 2.0, 9.0, 5.0, 14.0)
check(7.0, 2, 9.0, 5.0, 14.0)
check(-7, 2, -5, -9, -14)
check(True, 2, 3, -1, 2)
check(7.0, True, 8.0, 6.0, 7.0)
check(9223372036854775807, 2, 9223372036854775809, 9223372036854775805, 18446744073709551614)
check(-9223372036854775808, -1, -9223372036854775809, -9223372036854775807, 9223372036854775808)
check(-9223372036854775808, 1, -9223372036854775807, -9223372036854775809, -9223372036854775808)
check(9223372036854775807, -1, 9223372036854775806, 9223372036854775808, -9223372036854775807)

big = 1267650600228229401496703205376
check(big, 2, 1267650600228229401496703205378, 1267650600228229401496703205374, 2535301200456458802993406410752)
check(2, big, 1267650600228229401496703205378, -1267650600228229401496703205374, 2535301200456458802993406410752)
check(big, big, 2535301200456458802993406410752, 0, 1606938044258990275541962092341162602522202993782792835301376)
check(big, 2.0, 1.2676506002282294e30, 1.2676506002282294e30, 2.535301200456459e30)
check(2.0, big, 1.2676506002282294e30, -1.2676506002282294e30, 2.535301200456459e30)

promoted = 9223372036854775807
promoted = promoted + 1
check(promoted, 2, 9223372036854775810, 9223372036854775806, 18446744073709551616)


def fallback(a, b):
    return a + b, a * 2

assert fallback('a', 'b') == ('ab', 'aa')
assert fallback([1], [2]) == ([1, 2], [1, 1])

def signed_zero(a, b):
    import math
    assert math.copysign(1.0, a * b) == -1.0

signed_zero(-0.0, 2)
signed_zero(2, -0.0)
";
    run_checked(code);
}

#[test]
fn numeric_division_fast_paths_and_fallbacks() {
    let code = r"
import math

def check(a, b, quotient, floor, remainder):
    assert a / b == quotient
    assert a // b == floor
    assert a % b == remainder
    assert type(a / b) == type(quotient)
    result = a
    result /= b
    assert result == quotient
    assert type(result) == type(quotient)
    assert type(a // b) == type(floor)
    result = a
    result //= b
    assert result == floor
    assert type(result) == type(floor)
    assert type(a % b) == type(remainder)
    result = a
    result %= b
    assert result == remainder
    assert type(result) == type(remainder)

for a, b, floor, remainder in [(7, 2, 3, 1), (-7, 2, -4, 1), (7, -2, -4, -1), (-7, -2, 3, -1)]:
    check(a, b, float(a) / float(b), floor, remainder)
    check(float(a), b, float(a) / float(b), float(floor), float(remainder))
    check(a, float(b), float(a) / float(b), float(floor), float(remainder))
    check(float(a), float(b), float(a) / float(b), float(floor), float(remainder))

check(1.0, 0.1, 10.0, 9.0, 0.09999999999999995)
check(-9223372036854775808, -1, 9223372036854775808.0, 9223372036854775808, 0)
check(1267650600228229401496703205376, 2, 6.338253001141147e29, 633825300114114700748351602688, 0)
check(True, 2, 0.5, 0, 1)

# Converting either operand to float first gives a different rounded quotient.
a = 9007199254740993
b = 3
assert a / b == 3002399751580331.0
a /= b
assert a == 3002399751580331.0

def check_zero(a, b):
    for op in ['/', '//', '%', '/=', '//=', '%=']:
        try:
            if op == '/':
                a / b
            elif op == '//':
                a // b
            elif op == '%':
                a % b
            elif op == '/=':
                a /= b
            elif op == '//=':
                a //= b
            else:
                a %= b
        except ZeroDivisionError:
            pass
        else:
            assert False
    assert 7 + 2 == 9

for a in [7, 7.0, 1 << 100]:
    for b in [0, 0.0, -0.0]:
        check_zero(a, b)

for a, b in [(-0.0, 2), (0, -2.0), (0.0, -2)]:
    assert math.copysign(1.0, a / b) == -1.0
    assert math.copysign(1.0, a // b) == -1.0
for a, b in [(0, -2.0), (0.0, -2), (4.0, -2.0)]:
    assert math.copysign(1.0, a % b) == -1.0

for a, b in [(float('nan'), 2.0), (float('inf'), 2), (float('-inf'), 2.0)]:
    assert math.isnan(a // b)
    assert math.isnan(a % b)
assert math.isinf(float('inf') / 2)
check(2, float('inf'), 0.0, 0.0, 2.0)

def format_mod(a, b):
    return a % b
assert format_mod('%s', 'value') == 'value'
";
    run_checked(code);
}

#[test]
fn inplace_numeric_fallbacks_preserve_container_identity() {
    run_checked(
        r"
from collections import Counter, deque

items = [1]
alias = items
items += [2]
assert items is alias
assert alias == [1, 2]

queue = deque([1])
alias = queue
queue += [2]
assert queue is alias
assert list(alias) == [1, 2]

counts = Counter({'a': 2, 'b': 1})
alias = counts
counts += Counter({'a': 1})
assert counts is alias
assert alias == Counter({'a': 3, 'b': 1})
counts -= Counter({'a': 1, 'b': 1})
assert counts is alias
assert alias == Counter({'a': 2})

text = 'a'
text += 'b'
assert text == 'ab'
",
    );
}

fn run_checked(code: &str) {
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    #[cfg(feature = "ref-count-return")]
    {
        let output = runner.run_ref_counts(vec![]).unwrap();
        assert!(output.unreachable.is_empty(), "leaked {:?}", output.unreachable);
    }

    #[cfg(not(feature = "ref-count-return"))]
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn test_int_bit_length() {
    run_checked(
        r"
assert (0).bit_length() == 0
assert (1).bit_length() == 1
assert (-1).bit_length() == 1
assert (2).bit_length() == 2
assert (-2).bit_length() == 2
assert (37).bit_length() == 6
assert (-37).bit_length() == 6
assert (1024).bit_length() == 11
assert (2**62).bit_length() == 63
assert (2**63 - 1).bit_length() == 63
assert (-(2**63 - 1)).bit_length() == 63
assert (-9223372036854775808).bit_length() == 64
assert (2**63).bit_length() == 64
assert (-(2**63)).bit_length() == 64
assert (2**100).bit_length() == 101
assert (-2**100).bit_length() == 101
assert (True).bit_length() == 1
assert (False).bit_length() == 0
",
    );
}

#[test]
fn test_int_bit_count() {
    run_checked(
        r"
assert (0).bit_count() == 0
assert (1).bit_count() == 1
assert (-1).bit_count() == 1
assert (2).bit_count() == 1
assert (3).bit_count() == 2
assert (37).bit_count() == 3
assert (-37).bit_count() == 3
assert (15).bit_count() == 4
assert (-15).bit_count() == 4
assert (-9223372036854775808).bit_count() == 1
assert (2**63 - 1).bit_count() == 63
assert (-(2**63 - 1)).bit_count() == 63
assert (2**100).bit_count() == 1
assert (-2**100).bit_count() == 1
assert (2**100 - 1).bit_count() == 100
assert (-(2**100 - 1)).bit_count() == 100
assert (True).bit_count() == 1
assert (False).bit_count() == 0
",
    );
}

#[test]
fn test_int_bit_methods_arguments() {
    for code in [
        "(37).bit_length(1)",
        "(37).bit_count(1)",
        "(2**100).bit_length(1)",
        "(2**100).bit_count(1)",
        "True.bit_length(1)",
        "True.bit_count(1)",
        "(37).bit_length(x=1)",
        "(37).bit_count(x=1)",
    ] {
        let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        let err = ex.run_no_limits(vec![]).unwrap_err();
        assert_eq!(err.exc_type(), ExcType::TypeError, "failed for code: {code}");
    }

    for code in ["(37).foo()", "(2**100).foo()", "True.foo()"] {
        let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        let err = ex.run_no_limits(vec![]).unwrap_err();
        assert_eq!(err.exc_type(), ExcType::AttributeError, "failed for code: {code}");
    }
}
