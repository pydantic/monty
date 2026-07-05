from decimal import Decimal
import decimal

# === Decimal vs Decimal equality (numeric, scale-insensitive) ===
assert Decimal('1.5') == Decimal('1.5'), 'identical'
assert Decimal('1.0') == Decimal('1.00'), 'scale-insensitive equality'
assert Decimal('1.0') == Decimal('1'), 'trailing zero vs none'
assert Decimal('0') == Decimal('-0'), 'zero vs negative zero'
assert Decimal('1.5') != Decimal('1.6'), 'distinct values'

# === Decimal vs int / bool ===
assert Decimal(5) == 5, 'decimal == int'
assert 5 == Decimal(5), 'int == decimal'
assert Decimal('5.0') == 5, 'scaled decimal == int'
assert Decimal(1) == True, 'decimal == True'
assert Decimal(0) == False, 'decimal == False'
assert Decimal(5) != True, 'decimal != True'
assert Decimal('1.5') != 1, 'non-integral != int'

# === Decimal vs LongInt (big int beyond i64) ===
assert Decimal(5**40) == 5**40, 'decimal == big int'
assert 5**40 == Decimal(5**40), 'big int == decimal'
assert Decimal(10**30) == 10**30, 'power of ten big int'
assert Decimal('1E+40') == 10**40, 'scientific decimal == big int'
assert Decimal(5**40) != 5**40 + 1, 'big int inequality'

# === Decimal vs float (exact semantics) ===
assert Decimal('0.5') == 0.5, 'exact-in-binary float equal'
assert Decimal('0.1') != 0.1, 'non-exact float not equal'
assert Decimal(0.1) == 0.1, 'Decimal(float) round-trips to the float'
assert Decimal('2') == 2.0, 'integral decimal == float'

# === Ordering ===
assert Decimal('1.5') < Decimal('2'), 'decimal < decimal'
assert Decimal('1.5') < 2, 'decimal < int'
assert Decimal('1.5') < 2.0, 'decimal < float'
assert Decimal('2.5') > 2, 'decimal > int'
assert Decimal('1.5') <= Decimal('1.50'), 'decimal <= equal-scaled'
assert Decimal('1.5') >= Decimal('1.50'), 'decimal >= equal-scaled'
assert 2 > Decimal('1.5'), 'int > decimal'
assert Decimal(5**40) < 5**40 + 1, 'big int ordering'
assert Decimal('-1.5') < 0, 'negative ordering'
assert sorted([Decimal('3'), Decimal('1'), Decimal('2')]) == [Decimal('1'), Decimal('2'), Decimal('3')], 'sortable'

# === Huge-exponent vs small int (magnitude short-circuit, both signs) ===
assert Decimal('1E+6145') > 5, 'huge positive > small int'
assert not (Decimal('1E+6145') == 5), 'huge positive != small int'
assert Decimal('1E+6145') > -5, 'huge positive > negative int'
assert Decimal('-1E+6145') < 5, 'huge negative < positive int'
assert Decimal('-1E+6145') < -5, 'huge negative < small negative int'
assert Decimal('-1E+6145') < 0, 'huge negative < zero'
assert 5 < Decimal('1E+6145'), 'small int < huge positive (reversed)'
# |d| < 1 has zero integer digits, so it is smaller than any nonzero int
assert Decimal('0.5') < 1, 'fraction < 1'
assert Decimal('0.999') < 1, 'near-one fraction < 1'
assert Decimal('-0.5') > -1, 'negative fraction > -1'
assert Decimal('0.5') > 0, 'positive fraction > 0'
assert Decimal('0.5') > -1, 'positive fraction > negative int'

# === Infinity ordering ===
assert Decimal('inf') == Decimal('Infinity'), 'inf equality'
assert Decimal('-inf') < Decimal('inf'), '-inf < inf'
assert Decimal('inf') > 10**1000, 'inf > any int'
assert Decimal('-inf') < Decimal('-1000000'), '-inf < finite'
assert not (Decimal('inf') < Decimal('inf')), 'inf not < inf'

# === NaN: == / != never raise; ordering raises InvalidOperation ===
assert Decimal('NaN') != Decimal('NaN'), 'NaN != NaN'
assert not (Decimal('NaN') == Decimal('NaN')), 'NaN == NaN is False'
assert Decimal('NaN') != 1, 'NaN != int'
assert not (Decimal('NaN') == 1), 'NaN == int is False'

for op in ['lt', 'le', 'gt', 'ge']:
    try:
        if op == 'lt':
            _ = Decimal('NaN') < 1
        elif op == 'le':
            _ = Decimal('NaN') <= 1
        elif op == 'gt':
            _ = Decimal('NaN') > 1
        else:
            _ = Decimal('NaN') >= 1
        assert False, f'expected InvalidOperation for {op}'
    except decimal.InvalidOperation as exc:
        assert str(exc) == "[<class 'decimal.InvalidOperation'>]", f'{op} message'

# reversed operand and float NaN also raise
try:
    _ = 1 < Decimal('NaN')
    assert False, 'expected InvalidOperation'
except decimal.InvalidOperation:
    pass
try:
    _ = Decimal('1') < float('nan')
    assert False, 'expected InvalidOperation'
except decimal.InvalidOperation:
    pass

# === Hashing: equal numbers hash equally across types ===
assert hash(Decimal(5)) == hash(5), 'hash decimal int == hash int'
assert hash(Decimal('5.0')) == hash(5), 'scaled integral hash == int'
assert hash(Decimal(0)) == hash(0), 'hash zero'
assert hash(Decimal(-7)) == hash(-7), 'hash negative int'
assert hash(Decimal('1.5')) == hash(1.5), 'hash decimal == hash float'
assert hash(Decimal('0.5')) == hash(0.5), 'hash exact float'
assert hash(Decimal(5**40)) == hash(5**40), 'hash big integral == hash big int'
assert hash(Decimal('1.0')) == hash(Decimal('1.00')), 'equal decimals hash equal'
assert hash(Decimal('1.0')) == hash(Decimal('1')), 'integral scales hash equal'
# NaN hashes without raising (value is unspecified)
_ = hash(Decimal('NaN'))
_ = hash(Decimal('inf'))

# === Large integer-valued floats: float, int and Decimal hash equally ===
# (integer-valued floats above 2**63 fit D256's precision, so all three agree)
assert hash(Decimal(1e19)) == hash(1e19), 'Decimal(float) == hash float, > i64'
assert hash(Decimal('1e19')) == hash(1e19), 'Decimal(str) integral == hash float'
assert hash(1e19) == hash(10**19), 'integer-valued float == big int hash'
assert hash(Decimal(1e19)) == hash(10**19), 'Decimal == big int hash'
assert hash(2.0**63) == hash(Decimal(2**63)), 'i64 boundary float == Decimal hash'
assert hash(1e25) == hash(Decimal(1e25)), 'larger integral float == Decimal(float) hash'
# float and the equal Decimal always share a dict/set slot
assert 1e19 in {Decimal('1e19')}, 'float in decimal set'
assert Decimal('1e19') in {1e19}, 'decimal in float set'
assert {1e19: 'a'}[Decimal('1e19')] == 'a', 'float-keyed dict found by decimal'
assert len({Decimal('1e19'), 1e19, 10**19}) == 1, 'float, decimal, int collapse to one'

# === Mixed-key dict / set membership ===
d = {Decimal('1'): 'a'}
assert d[1] == 'a', 'int key finds decimal key'
d[1] = 'b'
assert d[Decimal('1')] == 'b', 'decimal key finds int-updated entry'
assert len(d) == 1, 'Decimal(1) and 1 are the same key'

s = {Decimal('2'), 2, Decimal('2.0')}
assert len(s) == 1, 'equal numbers collapse to one set element'
assert Decimal('2') in s, 'membership'
assert 2 in s, 'int membership in decimal set'

# === Signaling NaN: equality raises, hashing raises ===
try:
    Decimal('sNaN') == 1
    assert False, 'expected InvalidOperation from sNaN equality'
except decimal.InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'sNaN eq message'
try:
    hash(Decimal('sNaN'))
    assert False, 'expected TypeError from sNaN hash'
except TypeError as exc:
    assert str(exc) == 'Cannot hash a signaling NaN value', 'sNaN hash message'

# === Ordering against a non-number raises CPython's TypeError ===
try:
    Decimal('1') < 'x'
    assert False, 'expected TypeError from ordering against str'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'decimal.Decimal' and 'str'", 'ordering message'
try:
    [] >= Decimal('1')
    assert False, 'expected TypeError from reversed ordering'
except TypeError as exc:
    assert str(exc) == "'>=' not supported between instances of 'list' and 'decimal.Decimal'", (
        'reversed ordering message'
    )

# === NaN equality is non-reflexive on the == operator, identity-based in containers ===
x = Decimal('NaN')
assert not (x == x), 'NaN == same NaN object is False'
assert x != x, 'NaN != same NaN object is True'
d = {x: 1}
assert d[x] == 1, 'dict lookup uses identity for the same NaN key object'
assert x in [x], 'list membership uses identity'
assert [x].index(x) == 0, 'list.index uses identity'
assert x in {x}, 'set membership uses identity'
assert [x] == [x], 'container equality compares NaN elements by identity'
assert (x,) == (x,), 'tuple equality compares NaN elements by identity'
s = Decimal('sNaN')
try:
    s == s
    assert False, 'expected InvalidOperation from sNaN == sNaN'
except decimal.InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'sNaN self-eq message'
items = [s]
assert items == items and s in items and items.index(s) == 0, 'sNaN containers use identity, no raise'
f = Decimal('1.5')
assert f == f and not (f != f), 'ordinary Decimal self-equality'
