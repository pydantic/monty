from collections import deque

# Container lookups (`in`, `[]`, `.get`, `.pop`, `.remove`, ...) call user
# `__eq__` methods that may mutate the container being searched. CPython
# restarts dict/set probes, clamps `list.remove`, and raises for deque;
# Monty matches (see issue #729 — these previously panicked the worker).


# === dict: `in` with a clearing __eq__ restarts the probe (issue #729 repro) ===
D = {}


class DictClearer:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        D.clear()
        for i in range(3000):
            D[i] = i
        return False


for j in range(10):
    D[DictClearer()] = j

assert (DictClearer() in D) is False
assert len(D) == 3000


# === dict: `.get` and `.pop` with defaults survive the mutation ===
D2 = {}


class DictClearer2:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        D2.clear()
        for i in range(100):
            D2[i] = i
        return False


for j in range(10):
    D2[DictClearer2()] = j

assert D2.get(DictClearer2(), 'missing') == 'missing'
assert D2.pop(DictClearer2(), 'missing') == 'missing'


# === dict: `[]` raises KeyError after the restarted probe finds nothing ===
D3 = {}


class DictClearer3:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        D3.clear()
        for i in range(100):
            D3[i] = i
        return False


for j in range(10):
    D3[DictClearer3()] = j

# the KeyError message is the missing key's repr, which contains an object
# address, so only the exception type can be asserted
try:
    D3[DictClearer3()]
    assert False, 'expected KeyError'
except KeyError:
    pass


# === dict: assignment restarts the probe, then inserts ===
D4 = {}


class DictClearer4:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        D4.clear()
        for i in range(100):
            D4[i] = i
        return False


for j in range(10):
    D4[DictClearer4()] = j

D4[DictClearer4()] = 'x'
assert len(D4) == 101


# === dict/set: same-length mutation never compares a stale queued index ===
def stale_candidate_lookup(kind):
    target = {} if kind == 'dict' else set()
    active = False
    mutated = False

    class Key:
        def __hash__(self):
            return 1

        def __eq__(self, other):
            nonlocal active, mutated
            if isinstance(other, int):
                raise RuntimeError('compared against mismatched hash')
            if active and not mutated:
                mutated = True
                if kind == 'dict':
                    target.pop(keys[-1])
                    target[100] = None
                else:
                    target.remove(keys[-1])
                    target.add(100)
            return False

    keys = [Key() for _ in range(4)]
    for key in keys:
        if kind == 'dict':
            target[key] = None
        else:
            target.add(key)
    active = True
    return Key() in target


assert stale_candidate_lookup('dict') is False
assert stale_candidate_lookup('set') is False


# === dict/set: a mutation that leaves the candidate in place compares once ===
# CPython's `lookdict` only restarts when the compared entry itself changed, so
# an insertion elsewhere must not repeat the comparison
insert_calls = []
D5 = {}


class DictInserter:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        insert_calls.append('eq')
        D5[len(D5) + 100] = 0
        return True


D5[DictInserter()] = 'v'
assert D5[DictInserter()] == 'v'
assert insert_calls == ['eq']
assert len(D5) == 2

insert_calls.clear()
S3 = set()


class SetInserter:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        insert_calls.append('eq')
        S3.add(len(S3) + 100)
        return False


S3.add(SetInserter())
assert (SetInserter() in S3) is False
assert insert_calls == ['eq']
assert len(S3) == 2


# === dict/set: a colliding key inserted by __eq__ is still found ===
# the probe re-reads the candidates and meets the new key, as CPython does by
# carrying on through the live table — and like CPython it never re-compares a
# candidate it already compared
def inserted_candidate_lookup(kind, calls):
    target = {} if kind == 'dict' else set()
    inserted = False

    class Wanted:
        def __hash__(self):
            return 1

        def __eq__(self, other):
            calls.append('wanted')
            return isinstance(other, Wanted)

    wanted = Wanted()

    class Inserter:
        def __hash__(self):
            return 1

        def __eq__(self, other):
            nonlocal inserted
            calls.append('inserter')
            if not inserted:
                inserted = True
                if kind == 'dict':
                    target[wanted] = 'inserted'
                else:
                    target.add(wanted)
            return False

    if kind == 'dict':
        target[Inserter()] = 'original'
        return target.get(Wanted(), 'MISS')
    target.add(Inserter())
    return Wanted() in target


# the second 'inserter' is the nested insertion comparing against the entry
# already stored, before the outer lookup goes on to the key it added
dict_calls = []
assert inserted_candidate_lookup('dict', dict_calls) == 'inserted'
assert dict_calls == ['inserter', 'inserter', 'wanted']

set_calls = []
assert inserted_candidate_lookup('set', set_calls) is True
assert set_calls == ['inserter', 'inserter', 'wanted']


# === dict/set lookup calls the stored candidate's equality first ===
equality_calls = []


class StoredLookupKey:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        equality_calls.append('stored')
        return False


class QueryLookupKey:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        equality_calls.append('query')
        return False


stored_lookup_key = StoredLookupKey()
query_lookup_key = QueryLookupKey()
assert query_lookup_key not in {stored_lookup_key: None}
assert equality_calls == ['stored']
equality_calls.clear()
assert query_lookup_key not in {stored_lookup_key}
assert equality_calls == ['stored']


# === set: `in`, `discard` and `add` with a clearing __eq__ ===
S = set()


class SetClearer:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        S.clear()
        for i in range(100):
            S.add(i)
        return False


for j in range(10):
    S.add(SetClearer())

assert (SetClearer() in S) is False
assert len(S) == 100
S.discard(SetClearer())
assert len(S) == 100
S.add(SetClearer())
assert len(S) == 101


# === set: `remove` raises KeyError after the restarted probe finds nothing ===
S2 = set()


class SetClearer2:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        S2.clear()
        for i in range(100):
            S2.add(i)
        return False


for j in range(10):
    S2.add(SetClearer2())

# as above, the KeyError message contains an object address
try:
    S2.remove(SetClearer2())
    assert False, 'expected KeyError'
except KeyError:
    pass


# === list: a matching __eq__ that shrinks the list clamps the removal ===
L = [1, 2, 3]


class ListClearTrue:
    def __eq__(self, other):
        L.clear()
        return True


L.remove(ListClearTrue())
assert L == []


# === list: a matching __eq__ that shifts the list removes the shifted slot ===
# CPython deletes position 2 even though the match happened there before the
# shift, so the element that moved into it (4) is what goes
L2 = [1, 2, 3, 4]


class ListShiftTrue:
    def __eq__(self, other):
        if other == 3:
            L2.pop(0)
            return True
        return False


L2.remove(ListShiftTrue())
assert L2 == [2, 3]


# === list: `in` walks the live length, so a clearing __eq__ just ends the walk ===
L3 = [1, 2, 3]


class ListClearFalse:
    def __eq__(self, other):
        L3.clear()
        return False


assert (ListClearFalse() in L3) is False
assert L3 == []


# === comparison exceptions are caught in the frame running the opcode ===
class ComparisonRaiser:
    def __eq__(self, other):
        raise ValueError('comparison failed')


try:
    ComparisonRaiser() == 0
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'comparison failed'

try:
    assert ComparisonRaiser() == 0
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'comparison failed'


# === deque: mutation from __eq__ raises during `in` / `index` / `count` ===
dq = deque()


class DequeAppender:
    def __eq__(self, other):
        dq.append(99)
        return False


dq.append(DequeAppender())

try:
    0 in dq
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'

try:
    dq.index(0)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'

try:
    dq.count(0)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'


# === deque: `remove` raises IndexError (CPython quirk), even on a match ===
dq2 = deque()


class DequeAppender2:
    def __init__(self, ret):
        self.ret = ret

    def __eq__(self, other):
        dq2.append(99)
        return self.ret


dq2.append(DequeAppender2(False))

try:
    dq2.remove(0)
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'deque mutated during iteration'

dq3 = deque()


class DequeAppender3:
    def __eq__(self, other):
        dq3.append(99)
        return True


dq3.append(DequeAppender3())

try:
    dq3.remove(0)
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'deque mutated during iteration'


# === deque: a matching compare returns before the mutation check ===
dq4 = deque()


class DequeAppender4:
    def __eq__(self, other):
        dq4.append(99)
        return True


dq4.append(DequeAppender4())
assert (0 in dq4) is True
assert dq4.index(0) == 0


# === deque: `==` with an __eq__ that resizes either operand raises ===
dq_eq_a = deque()
dq_eq_b = deque()


class DequeEqClearer:
    def __init__(self, target):
        self.target = target

    def __eq__(self, other):
        self.target.clear()
        return True


dq_eq_a.append(DequeEqClearer(dq_eq_a))
dq_eq_a.append(1)
dq_eq_b.append(2)
dq_eq_b.append(3)

try:
    dq_eq_a == dq_eq_b
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'

# mutating the right-hand deque is caught too
dq_eq_c = deque()
dq_eq_d = deque()
dq_eq_c.append(DequeEqClearer(dq_eq_d))
dq_eq_c.append(1)
dq_eq_d.append(2)
dq_eq_d.append(3)

try:
    dq_eq_c == dq_eq_d
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'


# === deque: ordering comparisons check the same way ===
dq_lt_a = deque()
dq_lt_b = deque()


class DequeLtClearer:
    def __init__(self, target):
        self.target = target

    def __eq__(self, other):
        self.target.clear()
        return True

    def __lt__(self, other):
        return True


dq_lt_a.append(DequeLtClearer(dq_lt_a))
dq_lt_a.append(1)
dq_lt_b.append(2)
dq_lt_b.append(3)

try:
    dq_lt_a < dq_lt_b
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'


# === deque: an unequal compare returns before the mutation check ===
dq_ne_a = deque()
dq_ne_b = deque()


class DequeNeClearer:
    def __eq__(self, other):
        dq_ne_a.clear()
        return False


dq_ne_a.append(DequeNeClearer())
dq_ne_a.append(1)
dq_ne_b.append(2)
dq_ne_b.append(3)
assert (dq_ne_a == dq_ne_b) is False


# === list: `==` where __eq__ shrinks a side falls back to the live lengths ===
class ListClearer:
    def __init__(self, target):
        self.target = target

    def __eq__(self, other):
        self.target.clear()
        return True

    def __lt__(self, other):
        return True


lst_a = [None, 1]
lst_b = [2, 3]
lst_a[0] = ListClearer(lst_a)
# self is emptied mid-walk, so the lengths no longer match: not equal
assert (lst_a == lst_b) is False
assert lst_a == []

lst_c = [None, 1]
lst_d = [2, 3]
lst_c[0] = ListClearer(lst_d)
# the same holds when it is the right-hand list that shrinks
assert (lst_c == lst_d) is False
assert lst_d == []


# === list: ordering settles on the lengths as they are after the mutation ===
lst_e = [None, 1]
lst_f = [2, 3]
lst_e[0] = ListClearer(lst_e)
assert (lst_e < lst_f) is True


# === list: a partial truncation stops the walk where the shorter list ends ===
lst_g = [0, None, 2, 3, 4]
lst_h = [0, 1, 2, 3, 4]


class ListTruncater:
    def __eq__(self, other):
        while len(lst_g) > 2:
            lst_g.pop()
        return True


lst_g[1] = ListTruncater()
assert (lst_g == lst_h) is False
assert len(lst_g) == 2


# === a user `__eq__` on the probe still runs against native keys ===
class MatchesNative:
    def __init__(self, target):
        self.target = target

    def __hash__(self):
        return hash(self.target)

    def __eq__(self, other):
        return other == self.target


assert {1: 'int'}[MatchesNative(1)] == 'int'
assert {'abc': 'str'}[MatchesNative('abc')] == 'str'
assert MatchesNative(1) in {1}
assert MatchesNative('abc') in {'abc'}


# === a native candidate ahead of a user one keeps CPython's probe order ===
mixed_calls = []


class Colliding:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        return hash(7)

    def __eq__(self, other):
        mixed_calls.append(self.name)
        return False


mixed = {7: 'native', Colliding('stored'): 'user'}
mixed_calls.clear()  # the colliding insert above probes the chain
# the native pair is compared inline, reaching no user code
assert mixed[7] == 'native'
assert mixed_calls == []
# a miss compares the stored key on the left, so the native entry reflects onto
# the probe and the user entry runs its own `__eq__`
assert mixed.get(Colliding('probe'), 'MISS') == 'MISS'
assert mixed_calls == ['probe', 'stored']
