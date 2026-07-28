# Tests for `collections.deque`.
#
# Every expectation here is derived from CPython 3.14 (see playground/deque-spec.md).
# This file is dual-run against CPython and Monty, so both must agree exactly.
#
# NOT tested here, and why:
# - `del d[i]`: CPython supports it, but the `del` statement is unimplemented
#   Monty-wide (a parse error), so including it would fail to parse the file.
# - `type(d).__name__`: CPython gives the bare 'deque'; Monty gives the qualified
#   'collections.deque' for all module-scoped types. Pre-existing repo-wide
#   divergence, see limitations/collections.md.

from collections import deque

# === Construction ===
assert list(deque()) == [], 'empty deque'
assert list(deque([1, 2, 3])) == [1, 2, 3], 'from list'
assert list(deque('abc')) == ['a', 'b', 'c'], 'from str iterates chars'
assert list(deque(range(3))) == [0, 1, 2], 'from range'
assert list(deque((1, 2))) == [1, 2], 'from tuple'
# a positional maxlen truncates, keeping the RIGHTmost items
assert list(deque([1, 2, 3], 2)) == [2, 3], 'positional maxlen truncates from the left'
assert list(deque([1, 2, 3], maxlen=2)) == [2, 3], 'keyword maxlen truncates from the left'
assert list(deque([1, 2], maxlen=0)) == [], 'maxlen=0 keeps nothing'

# === repr ===
assert repr(deque()) == 'deque([])', 'empty repr'
assert repr(deque([1, 2])) == 'deque([1, 2])', 'unbounded repr'
assert repr(deque([1, 2], maxlen=5)) == 'deque([1, 2], maxlen=5)', 'maxlen shown when bounded'
assert repr(deque(['a'])) == "deque(['a'])", 'element repr is used, not str'

# === len / bool ===
assert len(deque()) == 0, 'empty len'
assert len(deque([1, 2, 3])) == 3, 'len'
assert bool(deque()) is False, 'empty is falsey'
assert bool(deque([0])) is True, 'non-empty is truthy even with falsey contents'

# === Iteration / reversed / membership ===
assert list(deque([1, 2, 3])) == [1, 2, 3], 'iteration order'
assert list(reversed(deque([1, 2, 3]))) == [3, 2, 1], 'reversed()'
assert 2 in deque([1, 2, 3]), 'membership hit'
assert 9 not in deque([1, 2, 3]), 'membership miss'
total = 0
for item in deque([1, 2, 3]):
    total += item
assert total == 6, 'for-loop iteration'

# === Indexing ===
d = deque([10, 20, 30])
assert d[0] == 10, 'index 0'
assert d[2] == 30, 'index last'
assert d[-1] == 30, 'negative index'
assert d[-3] == 10, 'negative index to first'
d[0] = 99
assert list(d) == [99, 20, 30], 'setitem positive'
d[-1] = 77
assert list(d) == [99, 20, 77], 'setitem negative'

# === append / appendleft / pop / popleft ===
d = deque([2])
d.append(3)
assert list(d) == [2, 3], 'append'
d.appendleft(1)
assert list(d) == [1, 2, 3], 'appendleft'
assert d.pop() == 3, 'pop returns the rightmost'
assert d.popleft() == 1, 'popleft returns the leftmost'
assert list(d) == [2], 'pop/popleft mutate in place'

# === extend / extendleft ===
d = deque([1, 2])
d.extend([3, 4])
assert list(d) == [1, 2, 3, 4], 'extend appends in order'
d = deque([1, 2, 3])
d.extendleft([0, -1])
assert list(d) == [-1, 0, 1, 2, 3], 'extendleft REVERSES the added items'
d = deque()
d.extend('ab')
assert list(d) == ['a', 'b'], 'extend accepts any iterable'

# === insert / remove / index / count ===
d = deque([1, 2, 3])
d.insert(1, 99)
assert list(d) == [1, 99, 2, 3], 'insert at index'
d = deque([1, 2, 3, 2])
d.remove(2)
assert list(d) == [1, 3, 2], 'remove drops the FIRST match only'
d = deque([1, 2, 3, 2, 1])
assert d.index(2) == 1, 'index finds first match'
assert d.index(2, 2) == 3, 'index honours start'
assert d.index(2, 0, 2) == 1, 'index honours start and stop'
assert d.count(1) == 2, 'count'
assert d.count(9) == 0, 'count of absent item'

# === rotate ===
d = deque([1, 2, 3, 4, 5])
d.rotate(1)
assert list(d) == [5, 1, 2, 3, 4], 'rotate(1) moves items right'
d = deque([1, 2, 3, 4, 5])
d.rotate(-1)
assert list(d) == [2, 3, 4, 5, 1], 'rotate(-1) moves items left'
d = deque([1, 2, 3, 4, 5])
d.rotate()
assert list(d) == [5, 1, 2, 3, 4], 'rotate() defaults to 1'
d = deque([1, 2, 3])
d.rotate(10)
assert list(d) == [3, 1, 2], 'rotate wraps modulo len'
d = deque()
d.rotate(1)
assert list(d) == [], 'rotate on empty is a no-op, not an error'

# === reverse / clear / copy ===
d = deque([1, 2, 3])
d.reverse()
assert list(d) == [3, 2, 1], 'reverse in place'
d = deque([1, 2, 3])
d.clear()
assert list(d) == [], 'clear'
assert len(d) == 0, 'clear resets len'
original = deque([1, 2], maxlen=5)
copied = original.copy()
assert list(copied) == [1, 2], 'copy has the same items'
assert copied.maxlen == 5, 'copy preserves maxlen'
copied.append(3)
assert list(original) == [1, 2], 'copy is a distinct object'

# === maxlen attribute and eviction ===
assert deque().maxlen is None, 'unbounded deque has maxlen None'
assert deque([1], maxlen=5).maxlen == 5, 'maxlen attribute'
d = deque([1, 2, 3], maxlen=3)
d.append(4)
assert list(d) == [2, 3, 4], 'append past maxlen evicts from the LEFT'
d = deque([1, 2, 3], maxlen=3)
d.appendleft(0)
assert list(d) == [0, 1, 2], 'appendleft past maxlen evicts from the RIGHT'
d = deque([1, 2, 3], maxlen=3)
d.extend([4, 5])
assert list(d) == [3, 4, 5], 'extend past maxlen evicts from the left'
d = deque([1, 2, 3], maxlen=3)
d.extendleft([0, -1])
assert list(d) == [-1, 0, 1], 'extendleft past maxlen evicts from the right'

# === Equality and ordering ===
assert deque([1, 2]) == deque([1, 2])
assert deque([1, 2]) != deque([2, 1]), 'order matters'
assert deque() == deque()
# a deque is NOT equal to a list with the same items (unlike namedtuple vs tuple)
assert deque([1, 2]) != [1, 2], 'deque is not equal to a list'
# maxlen does not participate in equality
assert deque([1], maxlen=1) == deque([1], maxlen=9), 'maxlen is not part of equality'
assert deque([1, 2]) < deque([1, 3]), 'ordering compares elementwise'
assert deque([1]) < deque([1, 2]), 'shorter prefix sorts first'
assert deque([2]) > deque([1, 9]), 'ordering is lexicographic'
assert deque([1, 2]) <= deque([1, 2]), 'less-or-equal'
assert deque([1, 2]) >= deque([1, 2]), 'greater-or-equal'

# Equal-but-unorderable elements (e.g. None == None) do NOT block ordering:
# CPython checks __eq__ first and only orders non-equal pairs, so a shared
# `None` prefix falls through to the length tiebreak instead of raising.
assert deque([None]) < deque([None, 1]), 'shared None prefix falls through to length'
assert deque([None]) <= deque([None]), 'equal None deques are <='
assert deque([None, 1]) > deque([None]), 'longer sorts after on a shared None prefix'
assert deque([1, None]) < deque([1, None, 2]), 'unorderable element mid-sequence still tiebreaks by length'
# Two DISTINCT NaN elements are never ==-equal, so the pair is the first
# difference and the comparison is unordered (yields False), matching list/tuple.
assert (deque([float('nan')]) < deque([float('nan'), 1])) is False, 'a NaN element makes the ordering false'

# Comparing two *distinct* cyclic deques re-enters the element walk once per
# level; CPython raises RecursionError, and Monty must bound it rather than
# overflowing the host stack.
cyc_a = deque([1, 2])
cyc_b = deque([1, 2])
cyc_a.append(cyc_a)
cyc_b.append(cyc_b)
try:
    cyc_a == cyc_b
    assert False, 'expected RecursionError from cyclic deque equality'
except RecursionError:
    pass
try:
    cyc_a < cyc_b
    assert False, 'expected RecursionError from cyclic deque ordering'
except RecursionError:
    pass
# The identity shortcut still resolves a cyclic deque against itself.
assert cyc_a == cyc_a
# Mutually cyclic deques recurse the same way.
mutual_a = deque()
mutual_b = deque()
mutual_a.append(mutual_b)
mutual_b.append(mutual_a)
other_a = deque()
other_b = deque()
other_a.append(other_b)
other_b.append(other_a)
try:
    mutual_a == other_a
    assert False, 'expected RecursionError from mutually cyclic deques'
except RecursionError:
    pass

# === Operators: + * += *= ===
assert list(deque([1]) + deque([2])) == [1, 2], 'deque + deque concatenates'
assert list(deque([1, 2]) * 2) == [1, 2, 1, 2], 'deque * int repeats'
assert list(deque([1, 2]) * 0) == [], 'deque * 0 is empty'
assert list(deque([1, 2]) * -1) == [], 'deque * negative is empty'
d = deque([1])
d += deque([2])
assert list(d) == [1, 2], 'deque += deque'
d = deque([1, 2])
d *= 2
assert list(d) == [1, 2, 1, 2], 'deque *= int'
# `+` preserves the LEFT operand's maxlen
assert (deque([1, 2], maxlen=2) + deque([3])).maxlen == 2, '+ preserves left maxlen'

# === Bounded repetition ===
# Repetition keeps maxlen, so the result truncates to its RIGHTmost items.
# A huge count must NOT materialize the full product before truncating.
assert repr(deque([1, 2], maxlen=2) * 10**9) == 'deque([1, 2], maxlen=2)'
assert repr(deque([1, 2], maxlen=2) * 3) == 'deque([1, 2], maxlen=2)'
assert repr(deque([1, 2], maxlen=2) * 0) == 'deque([], maxlen=2)'
assert repr(deque([1, 2], maxlen=2) * -5) == 'deque([], maxlen=2)'
# 12 items truncate to the last 5
assert repr(deque([1, 2, 3], maxlen=5) * 4) == 'deque([2, 3, 1, 2, 3], maxlen=5)'
assert repr(deque([1, 2, 3], maxlen=5) * 1) == 'deque([1, 2, 3], maxlen=5)'
assert repr(2 * deque([1, 2, 3], maxlen=5)) == 'deque([2, 3, 1, 2, 3], maxlen=5)'
assert repr(deque([1, 2], maxlen=0) * 10**9) == 'deque([], maxlen=0)'
# in-place repetition truncates the same way
d = deque([1, 2, 3], maxlen=5)
d *= 4
assert repr(d) == 'deque([2, 3, 1, 2, 3], maxlen=5)'

# A count too large for a C ssize_t raises before building anything.
try:
    deque([1, 2], maxlen=2) * 2**70
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == "cannot fit 'int' into an index-sized integer", str(exc)
try:
    deque([1, 2]) * 2**70
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == "cannot fit 'int' into an index-sized integer", str(exc)

# === Construction errors ===
try:
    deque(maxlen=-1)
    assert False, 'expected negative maxlen to fail'
except ValueError as exc:
    assert str(exc) == 'maxlen must be non-negative', str(exc)

try:
    deque(maxlen='x')
    assert False, 'expected non-int maxlen to fail'
except TypeError as exc:
    assert str(exc) == 'an integer is required', str(exc)

try:
    deque(5)
    assert False, 'expected non-iterable to fail'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable", str(exc)

# An omitted iterable builds an empty deque, but an explicit `None` is a real
# argument and, like any non-iterable, is rejected.
assert list(deque()) == [], 'omitted iterable is empty'
for _none_call in [lambda: deque(None), lambda: deque(None, 2), lambda: deque(iterable=None)]:
    try:
        _none_call()
        assert False, 'expected an explicit None iterable to fail'
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not iterable", str(exc)

try:
    deque([1], 2, 3)
    assert False, 'expected too many args to fail'
except TypeError as exc:
    assert str(exc) == 'deque() takes at most 2 arguments (3 given)', str(exc)

# === Index errors ===
try:
    deque([1, 2, 3])[3]
    assert False, 'expected out-of-range index to fail'
except IndexError as exc:
    assert str(exc) == 'deque index out of range', str(exc)

try:
    deque([1, 2, 3])[-4]
    assert False, 'expected out-of-range negative index to fail'
except IndexError as exc:
    assert str(exc) == 'deque index out of range', str(exc)

try:
    deque([1, 2, 3])[0:2]
    assert False, 'expected slicing to fail'
except TypeError as exc:
    assert str(exc) == "sequence index must be integer, not 'slice'", str(exc)

try:
    deque([1, 2, 3])['x']
    assert False, 'expected str key to fail'
except TypeError as exc:
    assert str(exc) == "sequence index must be integer, not 'str'", str(exc)

try:
    d = deque([1])
    d[9] = 1
    assert False, 'expected out-of-range setitem to fail'
except IndexError as exc:
    assert str(exc) == 'deque index out of range', str(exc)

# === Empty / missing errors ===
try:
    deque().pop()
    assert False, 'expected pop from empty to fail'
except IndexError as exc:
    assert str(exc) == 'pop from an empty deque', str(exc)

try:
    deque().popleft()
    assert False, 'expected popleft from empty to fail'
except IndexError as exc:
    assert str(exc) == 'pop from an empty deque', str(exc)

try:
    deque([1]).remove(9)
    assert False, 'expected remove of missing item to fail'
except ValueError as exc:
    assert str(exc) == 'deque.remove(x): x not in deque', str(exc)

try:
    deque([1]).index(9)
    assert False, 'expected index of missing item to fail'
except ValueError as exc:
    assert str(exc) == 'deque.index(x): x not in deque', str(exc)

# === index() bounds ===
# Unlike slicing, index() rejects an explicit None bound: it is a real argument,
# not the "omitted" sentinel.
_slice_msg = 'slice indices must be integers or have an __index__ method'
for _bad_bounds in [(2, None), (2, 0, None), (2, 'a'), (2, 1.0)]:
    try:
        deque([1, 2, 3]).index(*_bad_bounds)
        assert False, 'expected non-integer index bound to fail'
    except TypeError as exc:
        assert str(exc) == _slice_msg, str(exc)

# bool is an int subtype, so it is accepted as a bound
assert deque([1, 2, 3]).index(2, True) == 1, 'index accepts a bool bound'
# out-of-range ints clamp rather than overflow, in either direction
assert deque([1, 2, 3]).index(2, -(2**70)) == 1, 'huge negative start clamps to 0'
try:
    deque([1, 2, 3]).index(2, 2**70)
    assert False, 'expected huge start to find nothing'
except ValueError as exc:
    assert str(exc) == 'deque.index(x): x not in deque', str(exc)

# === maxlen errors ===
try:
    d = deque([1, 2, 3], maxlen=3)
    d.insert(1, 99)
    assert False, 'expected insert into a full deque to fail'
except IndexError as exc:
    assert str(exc) == 'deque already at its maximum size', str(exc)

try:
    d = deque([1])
    d.maxlen = 3
    assert False, 'expected maxlen assignment to fail'
except AttributeError as exc:
    # read-only, which CPython words differently from a missing attribute
    expected = "attribute 'maxlen' of 'collections.deque' objects is not writable"
    assert str(exc) == expected, str(exc)

# === Type errors ===
try:
    hash(deque())
    assert False, 'expected deque to be unhashable'
except TypeError as exc:
    assert str(exc) == "unhashable type: 'collections.deque'", str(exc)

try:
    deque([1]) < [2]
    assert False, 'expected deque < list to fail'
except TypeError as exc:
    expected = "'<' not supported between instances of 'collections.deque' and 'list'"
    assert str(exc) == expected, str(exc)

try:
    deque([1]) + [2]
    assert False, 'expected deque + list to fail'
except TypeError as exc:
    assert str(exc) == 'can only concatenate deque (not "list") to deque', str(exc)

try:
    deque([1]).extend(5)
    assert False, 'expected extend of a non-iterable to fail'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable", str(exc)

try:
    deque([1]).rotate('x')
    assert False, 'expected non-int rotate to fail'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer", str(exc)

# === Big-int integer arguments ===
# A big int is a real `int`, so it is accepted anywhere a deque takes an integer
# rather than rejected as a non-integer. One that overflows the C index type
# raises CPython's overflow/index error: subscripting reports an IndexError,
# while maxlen/rotate/insert report an OverflowError.
try:
    deque([1, 2, 3])[2**70]
    assert False, 'expected a huge index to fail'
except IndexError as exc:
    assert str(exc) == "cannot fit 'int' into an index-sized integer", str(exc)

try:
    deque([1, 2, 3])[-(2**70)]
    assert False, 'expected a huge negative index to fail'
except IndexError as exc:
    assert str(exc) == "cannot fit 'int' into an index-sized integer", str(exc)

try:
    d = deque([1, 2, 3])
    d[2**70] = 9
    assert False, 'expected a huge setitem index to fail'
except IndexError as exc:
    assert str(exc) == "cannot fit 'int' into an index-sized integer", str(exc)

_ssize_msg = 'Python int too large to convert to C ssize_t'
try:
    deque(maxlen=2**70)
    assert False, 'expected a huge maxlen to fail'
except OverflowError as exc:
    assert str(exc) == _ssize_msg, str(exc)

try:
    deque([1, 2, 3], 2**70)
    assert False, 'expected a huge positional maxlen to fail'
except OverflowError as exc:
    assert str(exc) == _ssize_msg, str(exc)

try:
    deque(maxlen=-(2**70))
    assert False, 'expected a huge negative maxlen to overflow before the sign check'
except OverflowError as exc:
    assert str(exc) == _ssize_msg, str(exc)

try:
    deque([1, 2, 3]).rotate(2**70)
    assert False, 'expected a huge rotate count to fail'
except OverflowError as exc:
    assert str(exc) == _ssize_msg, str(exc)

try:
    deque([1, 2, 3]).insert(2**70, 'x')
    assert False, 'expected a huge insert index to fail'
except OverflowError as exc:
    assert str(exc) == _ssize_msg, str(exc)

# === In-place add (`+=` is extend, so ANY iterable works) ===
d = deque([1])
d += [2, 3]
assert list(d) == [1, 2, 3], '+= accepts a list'
d = deque([1])
d += 'ab'
assert list(d) == [1, 'a', 'b'], '+= accepts a str, one item per char'
d = deque([1])
d += range(2)
assert list(d) == [1, 0, 1], '+= accepts a range'
d = deque([1])
d += (2,)
assert list(d) == [1, 2], '+= accepts a tuple'
d = deque([1], maxlen=2)
d += [2, 3]
assert list(d) == [2, 3], '+= evicts past maxlen'
d = deque([1, 2])
d += d
assert list(d) == [1, 2, 1, 2], '+= with itself extends by the original items'

try:
    d = deque([1])
    d += 5
    assert False, 'expected += of a non-iterable to fail'
except TypeError as exc:
    # from the iterator protocol, NOT the concatenation error `+` would give
    assert str(exc) == "'int' object is not iterable", str(exc)

# === Mutation during iteration ===
# A deque tracks a mutation counter, so ANY structural change invalidates a live
# iterator — even ones that leave the length unchanged.
try:
    d = deque([1, 2, 3])
    for _item in d:
        d.append(9)
    assert False, 'expected append during iteration to fail'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration', str(exc)

try:
    d = deque([1, 2, 3])
    for _item in d:
        d.rotate(1)
    assert False, 'expected rotate during iteration to fail'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration', str(exc)

try:
    d = deque([1, 2, 3])
    for _item in d:
        d.rotate(0)
    assert False, 'expected rotate(0) during iteration to fail'
except RuntimeError as exc:
    # a no-op rotate still counts as a mutation, as long as len > 1
    assert str(exc) == 'deque mutated during iteration', str(exc)

try:
    d = deque([1, 2, 3])
    for _item in d:
        d.append(9)
        d.popleft()
    assert False, 'expected a length-preserving mutation to fail'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration', str(exc)

# the mutation is caught even on the LAST iteration, where the loop was about to end
try:
    d = deque([1, 2])
    seen = 0
    for _item in d:
        seen += 1
        if seen == 2:
            d.append(3)
            d.popleft()
    assert False, 'expected a mutation on the final iteration to fail'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration', str(exc)

# === Operations that are LEGAL during iteration ===
# CPython leaves its state counter alone for these, so they must not raise.
d = deque([1, 2, 3])
for _item in d:
    d.reverse()
assert len(d) == 3, 'reverse() during iteration is allowed'

d = deque([1, 2, 3])
for _item in d:
    d[0] = 9
assert d[0] == 9, 'item assignment during iteration is allowed'

d = deque([1, 2, 3])
reads = 0
for _item in d:
    reads += d.count(1) + d.index(2) + len(d.copy())
assert reads == 15, 'read-only methods during iteration are allowed'

d = deque([1, 2, 3])
for _item in d:
    d.extend([])
assert list(d) == [1, 2, 3], 'extending by an empty iterable is not a mutation'

d = deque([1, 2, 3])
for _item in d:
    try:
        d.remove(99)
    except ValueError:
        pass
assert list(d) == [1, 2, 3], 'a remove() that raises does not invalidate the iterator'

# === Method arity errors ===
# CPython is INCONSISTENT here: append/count/copy name the type, index/insert/rotate
# do not. This mirrors METH_O / METH_NOARGS vs PyArg_UnpackTuple in CPython's C code.
try:
    deque().append()
    assert False, 'expected append() with no args to fail'
except TypeError as exc:
    assert str(exc) == 'deque.append() takes exactly one argument (0 given)', str(exc)

try:
    deque().append(1, 2)
    assert False, 'expected append() with two args to fail'
except TypeError as exc:
    assert str(exc) == 'deque.append() takes exactly one argument (2 given)', str(exc)

try:
    deque().count()
    assert False, 'expected count() with no args to fail'
except TypeError as exc:
    assert str(exc) == 'deque.count() takes exactly one argument (0 given)', str(exc)

try:
    deque().copy(1)
    assert False, 'expected copy() with an arg to fail'
except TypeError as exc:
    assert str(exc) == 'deque.copy() takes no arguments (1 given)', str(exc)

try:
    deque().index()
    assert False, 'expected index() with no args to fail'
except TypeError as exc:
    assert str(exc) == 'index expected at least 1 argument, got 0', str(exc)

try:
    deque().insert(1)
    assert False, 'expected insert() with one arg to fail'
except TypeError as exc:
    assert str(exc) == 'insert expected 2 arguments, got 1', str(exc)

try:
    deque().rotate(1, 2)
    assert False, 'expected rotate() with two args to fail'
except TypeError as exc:
    assert str(exc) == 'rotate expected at most 1 argument, got 2', str(exc)

# === str(type()) is qualified ===
assert str(type(deque())) == "<class 'collections.deque'>", 'qualified class repr'

# === `iterable` is a positional-or-keyword parameter ===
assert list(deque(iterable=[1, 2])) == [1, 2], 'the first parameter is accepted by keyword'
assert list(deque(iterable=[1, 2, 3], maxlen=2)) == [2, 3], 'both parameters by keyword'
assert list(deque([1, 2, 3], maxlen=2)) == [2, 3], 'positional iterable with keyword maxlen'

# === Recursive repr renders the cycle like a list ===
# A deque holds its items in a list-shaped bracket, so the cycle placeholder is
# `[...]` inside those brackets, not a bare `...`.
d = deque()
d.append(d)
assert repr(d) == 'deque([[...]])', 'a self-referential deque'
d2 = deque([1])
d2.append(d2)
assert repr(d2) == 'deque([1, [...]])', 'a self-reference alongside a plain item'
d3 = deque()
d3.append([d3])
assert repr(d3) == 'deque([[[...]]])', 'a self-reference nested in a list'


# === Extending appends as it consumes, so a mid-iteration raise keeps what arrived ===
# CPython's `extend` (and `__iadd__`, which *is* extend) appends each item as the
# source yields it, rather than draining the source first — so an iterator that
# raises part-way leaves the earlier items in the deque.
class Raiser:
    def __init__(self, stop):
        self.n = 0
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > self.stop:
            raise ValueError('boom')
        return self.n


d = deque([0])
try:
    d += Raiser(2)
    assert False, 'expected the iterator to raise'
except ValueError:
    pass
assert list(d) == [0, 1, 2]

d = deque([0])
try:
    d.extend(Raiser(2))
    assert False, 'expected the iterator to raise'
except ValueError:
    pass
assert list(d) == [0, 1, 2]

# extendleft pushes each item to the front in turn, so a partial run reverses too
d = deque([0])
try:
    d.extendleft(Raiser(2))
    assert False, 'expected the iterator to raise'
except ValueError:
    pass
assert list(d) == [2, 1, 0]

# a bounded deque evicts while it consumes, so only the surviving window remains
b = deque([9], maxlen=2)
try:
    b += Raiser(3)
    assert False, 'expected the iterator to raise'
except ValueError:
    pass
assert list(b) == [2, 3]

# === Self-extension still uses the original items ===
# Appending while iterating the same deque must not chase its own tail.
s = deque([1, 2])
s += s
assert list(s) == [1, 2, 1, 2]
s2 = deque([1, 2])
s2.extend(s2)
assert list(s2) == [1, 2, 1, 2]
s3 = deque([1, 2])
s3.extendleft(s3)
assert list(s3) == [2, 1, 1, 2]
s4 = deque([1, 2, 3], maxlen=4)
s4 += s4
assert list(s4) == [3, 1, 2, 3]
