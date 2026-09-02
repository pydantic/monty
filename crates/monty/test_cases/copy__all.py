import copy
from collections import Counter, defaultdict, deque, namedtuple
from dataclasses import dataclass
from functools import partial

# === shallow copy of containers ===
lst = [[1], [2]]
shallow = copy.copy(lst)
assert shallow == lst
assert shallow is not lst
assert shallow[0] is lst[0]
shallow.append([3])
assert len(lst) == 2

d = {'a': [1]}
d_copy = copy.copy(d)
assert d_copy == d
assert d_copy is not d
assert d_copy['a'] is d['a']

s = {1, 2}
s_copy = copy.copy(s)
assert s_copy == s
assert s_copy is not s

# === shallow copy returns immutables unchanged ===
t = (1, [2])
assert copy.copy(t) is t
fs = frozenset({1, 2})
assert copy.copy(fs) is fs
assert copy.copy('abc') == 'abc'
assert copy.copy(7) == 7
assert copy.copy(None) is None
r = range(3)
assert copy.copy(r) is r

# === deep copy rebuilds nested containers ===
original = {'x': 1, 'nested': {'y': 2}}
new = copy.deepcopy(original)
new['nested']['y'] = 3
assert original == {'x': 1, 'nested': {'y': 2}}
assert new == {'x': 1, 'nested': {'y': 3}}

nested = [[1], [2]]
deep = copy.deepcopy(nested)
assert deep == nested
assert deep[0] is not nested[0]

# === deep copy of dict keys ===
key = (1, 2)
keyed = {key: 'v'}
keyed_copy = copy.deepcopy(keyed)
assert keyed_copy == keyed
assert keyed_copy[(1, 2)] == 'v'

# === cycles terminate ===
cyclic = []
cyclic.append(cyclic)
cyclic_copy = copy.deepcopy(cyclic)
assert cyclic_copy is not cyclic
assert cyclic_copy[0] is cyclic_copy

cyclic_dict = {}
cyclic_dict['me'] = cyclic_dict
cyclic_dict_copy = copy.deepcopy(cyclic_dict)
assert cyclic_dict_copy['me'] is cyclic_dict_copy

# === a tuple in a cycle closes on one object ===
# The tuple is memoized only once its items are done, so the walk back through
# `cycle_items` copies it in full first; `deepcopy` hands that copy back rather
# than building a second tuple from the same items.
cycle_items = []
cycle_tuple = (cycle_items,)
cycle_items.append(cycle_tuple)
cycle_tuple_copy = copy.deepcopy(cycle_tuple)
assert cycle_tuple_copy[0][0] is cycle_tuple_copy
assert cycle_tuple_copy is not cycle_tuple
assert cycle_tuple_copy[0] is not cycle_items

# A named tuple has no such re-read in CPython, so its cycle does open out into
# a second object; Monty matches.
NamedCycle = namedtuple('NamedCycle', 'items')
named_items = []
named_cycle = NamedCycle(named_items)
named_items.append(named_cycle)
named_cycle_copy = copy.deepcopy(named_cycle)
assert named_cycle_copy.items[0] is not named_cycle_copy

# === objects reached twice stay shared ===
shared = [1]
pair = [shared, shared]
pair_copy = copy.deepcopy(pair)
assert pair_copy[0] is pair_copy[1]
assert pair_copy[0] is not shared

# === deep copy leaves immutables alone ===
assert copy.deepcopy(7) == 7
assert copy.deepcopy('abc') == 'abc'
assert copy.deepcopy(None) is None
assert copy.deepcopy(b'xy') == b'xy'
assert copy.deepcopy(range(3)) == range(3)

# A tuple is rebuilt only when deep-copying changed one of its items.
atomic_tuple = (1, (2, 3))
assert copy.deepcopy(atomic_tuple) is atomic_tuple
mutable_tuple = (1, [2])
mutable_copy = copy.deepcopy(mutable_tuple)
assert mutable_copy == mutable_tuple
assert mutable_copy is not mutable_tuple
assert mutable_copy[1] is not mutable_tuple[1]


# === functions are shared, closures and defaults included ===
def plain(x):
    return x


def with_default(x, seen=()):
    return x, seen


def make_counter():
    total = 0

    def bump(n):
        nonlocal total
        total += n
        return total

    return bump


counter = make_counter()
assert copy.copy(plain) is plain
assert copy.deepcopy(plain) is plain
assert copy.copy(with_default) is with_default
assert copy.deepcopy(with_default) is with_default
assert copy.copy(counter) is counter
assert copy.deepcopy(counter) is counter

# A container holding one is rebuilt, but the functions in it are not.
fn_holder = copy.deepcopy({'f': with_default, 'g': counter, 'rest': [1]})
assert fn_holder['f'] is with_default
assert fn_holder['g'] is counter
assert fn_holder['rest'] == [1]

# Sharing means sharing the cells too: the copy is not a fresh counter.
assert counter(2) == 2
assert fn_holder['g'](3) == 5


# === bound methods are rebound, not refused ===
class Counted:
    def __init__(self, v):
        self.v = v

    def get(self):
        return self.v


owner = Counted([1])
bound = owner.get
# A shallow copy shares the receiver, so it still sees the original mutate.
shallow_bound = copy.copy(bound)
owner.v.append(2)
assert shallow_bound() == [1, 2]

# A deep copy takes the receiver with it and is detached from the original.
detached_owner = Counted([1])
deep_bound = copy.deepcopy(detached_owner.get)
detached_owner.v.append(2)
assert deep_bound() == [1]

# The receiver goes through the memo, so an object and its method copied
# together stay bound to each other rather than to two separate copies.
together = Counted([1])
copied_pair = copy.deepcopy([together, together.get])
copied_pair[0].v.append(9)
assert copied_pair[1]() == [1, 9]
assert together.v == [1]

# An instance holding its own bound method terminates on the memoized shell.
self_bound = Counted([1])
self_bound.cb = self_bound.get
self_bound_copy = copy.deepcopy(self_bound)
self_bound_copy.v.append(7)
assert self_bound_copy.cb() == [1, 7]
assert self_bound.v == [1]


# === partials are rebuilt, not shared ===
def take(*args, **kwargs):
    return (args, kwargs)


# A shallow copy is a new partial over the same callable and bound values.
bound_list = [1]
p = partial(take, bound_list, key=[2])
shallow_p = copy.copy(p)
assert shallow_p is not p
assert shallow_p.func is take
assert shallow_p.args[0] is bound_list
assert shallow_p.keywords['key'] is p.keywords['key']
bound_list.append(9)
assert shallow_p() == (([1, 9],), {'key': [2]})

# A deep copy detaches every bound value; the callable is shared, as
# functions are.
deep_source = [1]
deep_p = copy.deepcopy(partial(take, deep_source, key=[2]))
assert deep_p.func is take
assert deep_p.args[0] is not deep_source
deep_source.append(9)
assert deep_p() == (([1],), {'key': [2]})

# Bound values go through the memo, so structure shared with the partial
# stays shared in the copy.
shared = [1]
memo_pair = copy.deepcopy([shared, partial(take, shared)])
memo_pair[0].append(2)
assert memo_pair[1]() == (([1, 2],), {})
assert shared == [1]

# A partial bound to a container holding it terminates on the memoized copy.
holder = []
cyclic = partial(take, holder)
holder.append(cyclic)
cyclic_copy = copy.deepcopy(cyclic)
assert cyclic_copy.args[0][0] is cyclic_copy

# `partial` flattens at construction, so the copy is over the inner callable.
flat = copy.deepcopy(partial(partial(take, 1), 2))
assert flat.func is take
assert flat() == ((1, 2), {})


# The copied callable is re-checked, as CPython's reconstructor is, so a
# `__deepcopy__` that hands back something uncallable fails the same way.
class CallableHook:
    def __call__(self):
        return 1

    def __deepcopy__(self, memo):
        return 42


try:
    copy.deepcopy(partial(CallableHook()))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'the first argument must be callable'

# === sets and frozensets ===
set_copy = copy.deepcopy({1, 2, 3})
assert set_copy == {1, 2, 3}
frozen_copy = copy.deepcopy(frozenset({1, 2}))
assert frozen_copy == frozenset({1, 2})

# === collections types ===
dq = deque([[1]], maxlen=3)
dq_copy = copy.deepcopy(dq)
assert dq_copy == dq
assert dq_copy.maxlen == 3
assert dq_copy[0] is not dq[0]
dq_shallow = copy.copy(dq)
assert dq_shallow[0] is dq[0]

dd = defaultdict(list)
dd['a'].append(1)
dd_copy = copy.deepcopy(dd)
assert dd_copy == dd
assert dd_copy.default_factory is list
dd_copy['b'].append(2)
assert dd_copy['b'] == [2]
assert 'b' not in dd

# The factory is the reconstructor's argument in CPython, so `deepcopy`
# rebuilds it while `copy.copy` shares it. `list` is a class either way; a
# `partial` is the copyable factory that tells the two apart.
partial_factory = partial(list)
dd_partial = defaultdict(partial_factory)
dd_partial_deep = copy.deepcopy(dd_partial)
assert dd_partial_deep.default_factory is not partial_factory
assert dd_partial_deep['x'] == []
assert copy.copy(dd_partial).default_factory is partial_factory

# A factory-less defaultdict and a Counter keep their flavour through the
# same path.
assert copy.deepcopy(defaultdict(None)).default_factory is None

# The rebuilt factory faces the check the constructor applies, since the
# reconstructor is that constructor. The setter takes any value, so it is what
# gets a non-callable as far as the copy.
dd_bad = defaultdict(list)
dd_bad.default_factory = 42
try:
    copy.deepcopy(dd_bad)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'first argument must be callable or None'

# `None` is the factory-less form rather than a bad factory, so it is kept.
dd_none = defaultdict(list)
dd_none.default_factory = None
dd_none_copy = copy.deepcopy(dd_none)
assert dd_none_copy.default_factory is None
try:
    dd_none_copy['missing']
    assert False, 'expected KeyError'
except KeyError as exc:
    assert str(exc) == "'missing'"

counter_copy = copy.deepcopy(Counter(a=1, b=2))
assert counter_copy == Counter(a=1, b=2)
assert counter_copy.most_common(1) == [('b', 2)]

Point = namedtuple('Point', 'x y')
point = Point([1], 2)
point_copy = copy.deepcopy(point)
assert point_copy == point
assert point_copy.x is not point.x
assert isinstance(point_copy, Point)

# === class instances ===


class Holder:
    calls = 0

    def __init__(self, value):
        self.value = value
        Holder.calls += 1


holder = Holder([1])
calls_before = Holder.calls
deep_holder = copy.deepcopy(holder)
assert deep_holder.value == [1]
assert deep_holder.value is not holder.value
assert isinstance(deep_holder, Holder)
assert Holder.calls == calls_before

shallow_holder = copy.copy(holder)
assert shallow_holder.value is holder.value
shallow_holder.value = [9]
assert holder.value == [1]

# an instance that refers to itself
holder.self_ref = holder
self_ref_copy = copy.deepcopy(holder)
assert self_ref_copy.self_ref is self_ref_copy

# === dataclasses ===


@dataclass
class Config:
    name: str
    tags: list[str]


config = Config('a', ['x'])
config_copy = copy.deepcopy(config)
assert config_copy == config
assert config_copy.tags is not config.tags
config_copy.tags.append('y')
assert config.tags == ['x']


@dataclass(frozen=True)
class Frozen:
    items: list[str]


frozen_copy = copy.deepcopy(Frozen(['a']))
assert frozen_copy == Frozen(['a'])
assert frozen_copy.items == ['a']

# === __copy__ and __deepcopy__ hooks ===


class Hooked:
    def __copy__(self):
        return 'shallow hook'

    def __deepcopy__(self, memo):
        return 'deep hook'


assert copy.copy(Hooked()) == 'shallow hook'
assert copy.deepcopy(Hooked()) == 'deep hook'


# A hook's result is memoized like any other copy, so an object reached twice
# stays one object even though the hook, not the copier, built it.
class HookedList:
    def __deepcopy__(self, memo):
        return ['from hook']


hooked_twice = HookedList()
hooked_pair = copy.deepcopy([hooked_twice, hooked_twice])
assert hooked_pair[0] is hooked_pair[1]


# `copy` calls its hook only `if copier is not None`, so setting one to `None`
# opts the class out and leaves the ordinary attribute copy.
class OptedOut:
    __copy__ = None
    __deepcopy__ = None

    def __init__(self):
        self.items = [1]


opted_out = OptedOut()
opted_out_shallow = copy.copy(opted_out)
assert isinstance(opted_out_shallow, OptedOut)
assert opted_out_shallow.items is opted_out.items

opted_out_deep = copy.deepcopy(opted_out)
assert isinstance(opted_out_deep, OptedOut)
assert opted_out_deep.items is not opted_out.items
opted_out_deep.items.append(2)
assert opted_out.items == [1]

# === the memo argument ===
memo = {}
memo_source = [[1]]
memo_copy = copy.deepcopy(memo_source, memo)
assert memo_copy == memo_source
assert memo[id(memo_source)] is memo_copy
assert copy.deepcopy(memo_source, memo) is memo_copy

# === mutation of the source during a copy ===
# Copying an item runs Python, which can reach back and change the source.
# Each container behaves as it does under CPython's own iteration.


class Shrink:
    def __deepcopy__(self, memo):
        shrinking.clear()
        return 'ok'


shrinking = [Shrink(), 'a', 'b']
assert copy.deepcopy(shrinking) == ['ok']


class Grow:
    def __deepcopy__(self, memo):
        if len(growing) < 3:
            growing.append('added')
        return 'ok'


growing = [Grow(), 'a']
assert copy.deepcopy(growing) == ['ok', 'a', 'added']


class ClearSet:
    def __hash__(self):
        return 1

    def __deepcopy__(self, memo):
        set_source.clear()
        return 'ok'


# A set is snapshotted before its members are copied, so clearing it mid-walk
# leaves the copy whole.
set_source = {ClearSet(), 1, 2}
assert copy.deepcopy(set_source) == {'ok', 1, 2}


class GrowDict:
    def __deepcopy__(self, memo):
        dict_source['added'] = 1
        return 'ok'


dict_source = {'k': GrowDict(), 'z': 2}
try:
    copy.deepcopy(dict_source)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'


# Growing while the *last* pair is copied still raises: the size is checked
# before the walk can decide it is finished, as CPython's dict iterator does.
class GrowLastDict:
    def __deepcopy__(self, memo):
        last_dict_source['added'] = 1
        return 'ok'


last_dict_source = {'k': GrowLastDict()}
try:
    copy.deepcopy(last_dict_source)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'


class GrowAttrs:
    def __deepcopy__(self, memo):
        attr_source.added = 1
        return 'ok'


class Holds:
    def __init__(self, k):
        self.k = k


attr_source = Holds(GrowAttrs())
attr_source.z = 2
try:
    copy.deepcopy(attr_source)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'


# `__dict__` is walked the same way, last attribute included.
class GrowLastAttrs:
    def __deepcopy__(self, memo):
        last_attr_source.added = 1
        return 'ok'


last_attr_source = Holds(GrowLastAttrs())
try:
    copy.deepcopy(last_attr_source)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'


class ClearDeque:
    def __deepcopy__(self, memo):
        deque_source.clear()
        return 'ok'


deque_source = deque([ClearDeque(), 1, 2])
try:
    copy.deepcopy(deque_source)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'

# === types that cannot be copied ===
try:
    copy.deepcopy(copy)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot pickle 'module' object"
