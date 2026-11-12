use monty::MontyRun;
use monty_types::CompileOptions;

#[test]
fn numeric_comparisons_and_fallbacks() {
    let code = r"
def check(a, b, expected):
    result = (a == b, a != b, a < b, a <= b, a > b, a >= b)
    assert result == expected
    assert all(type(item) is bool for item in result)
    # Check reversed operands and chained comparisons through their own opcodes.
    reverse = (b == a, b != a, b > a, b >= a, b < a, b <= a)
    assert reverse == expected
    chained = a <= b <= b
    assert chained == expected[3]

less = (False, True, True, True, False, False)
equal = (True, False, False, True, False, True)
greater = (False, True, False, False, True, True)
unordered = (False, True, False, False, False, False)
check(1, 2, less)
check(2, 2, equal)
check(3, 2, greater)
check(1.0, 2.0, less)
check(2.0, 2.0, equal)
check(3.0, 2.0, greater)
check(1, 2.0, less)
check(2.0, 2, equal)
check(3.0, 2, greater)
check(-1, -0.5, less)
check(0, -0.5, greater)
check(0.0, -0.0, equal)
check(-0.0, 0, equal)
check(9007199254740993, 9007199254740992.0, greater)
check(-9007199254740993, -9007199254740992.0, less)
check(9223372036854775807, 9223372036854775808.0, less)
check(-9223372036854775808, -9223372036854775808.0, equal)
check(float('inf'), 9223372036854775807, greater)
check(float('-inf'), -9223372036854775808, less)
check(float('nan'), 0, unordered)
check(float('nan'), 0.0, unordered)
check(float('nan'), float('nan'), unordered)
check(True, 1, equal)
check(False, 1.0, less)
check(1267650600228229401496703205376, 1, greater)
check(1267650600228229401496703205376, 1.2676506002282294e30, equal)
check('a', 'b', less)
check([1], [2], less)

class Custom:
    def __eq__(self, other):
        return [42]

result = Custom() == 1
assert result == [42]
result = Custom() != 1
assert result is False

try:
    result = 1 < 'a'
except TypeError:
    pass
else:
    assert False

number = 1
result = number is number
assert result is True
result = number is not number
assert result is False

other_number = 1.0
result = number == other_number
assert result is True
result = number is other_number
assert result is False
result = number is not other_number
assert result is True

for invert in [False, True]:
    try:
        result = number not in other_number if invert else number in other_number
    except TypeError:
        pass
    else:
        assert False

items = [1, 2]
assert (items is items) is True
assert (items is not items) is False
assert (1 in items) is True
assert (3 not in items) is True
";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    #[cfg(feature = "ref-count-return")]
    {
        let output = runner.run_ref_counts(vec![]).unwrap();
        assert!(output.unreachable.is_empty(), "leaked {:?}", output.unreachable);
    }

    #[cfg(not(feature = "ref-count-return"))]
    runner.run_no_limits(vec![]).unwrap();
}
