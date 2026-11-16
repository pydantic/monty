from decimal import Decimal


for digits, expected in (
    ((True,), '1'),
    ((False,), '0'),
    ((1, False), '10'),
    ((False, True, False), '10'),
    ([True, False, True], '101'),
):
    for sign in (0, 1):
        result = Decimal((sign, digits, 0))
        assert str(result) == ('-' if sign else '') + expected
        assert all(type(digit) is int for digit in result.as_tuple().digits)

# Accepting bool as an integer digit must not admit other coefficient types or ranges.
for invalid in (-1, 10, 1.0, '1', None):
    try:
        Decimal((0, (True, invalid), 0))
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'coefficient must be a tuple of digits'
