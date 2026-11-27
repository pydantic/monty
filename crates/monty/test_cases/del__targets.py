from collections import Counter, defaultdict, deque
from decimal import InvalidOperation, getcontext

# === Names and groups ===
a, b, c = [], {}, [1, 2]
del (a, [b, c[0]]), (), []
assert c == [2]
assert 'a' not in locals() and 'b' not in locals(), 'the names were not unbound'

# Deletion does not fall back to builtins, but a subsequent read does.
len = 42
del len
assert len(c) == 1
try:
    del len
    assert False, 'deleting a name must not delete a builtin'
except NameError as exc:
    assert str(exc) == "name 'len' is not defined"


def clear_global():
    global c
    del c


clear_global()
assert 'c' not in locals(), 'global was not unbound'


def clear_parameter(value):
    def read():
        return value

    assert read() == [42]
    del value
    assert 'value' not in locals(), 'the captured parameter is unbound'
    return read


read_deleted = clear_parameter([42])
try:
    read_deleted()
    assert False, 'expected a free-variable NameError'
except NameError as exc:
    assert str(exc) == "cannot access free variable 'value' where it is not associated with a value in enclosing scope"


def make_clear_nonlocal():
    value = []

    def clear():
        nonlocal value
        del value

    return clear


clear = make_clear_nonlocal()
clear()
try:
    clear()
    assert False, 'expected a free-variable NameError'
except NameError as exc:
    assert str(exc) == "cannot access free variable 'value' where it is not associated with a value in enclosing scope"


def local_even_without_a_store():
    del shadow


shadow = 10
try:
    local_even_without_a_store()
    assert False, 'expected UnboundLocalError'
except UnboundLocalError as exc:
    assert str(exc) == "cannot access local variable 'shadow' where it is not associated with a value"
assert shadow == 10

try:
    raise ValueError('test')
except ValueError as exc:
    del exc


# === Class namespace ===
class Number:
    shadow = 20
    before = shadow
    del shadow
    after = shadow
    shadow = 30
    temporary = [1, 2]
    del temporary[0]
    removed = 40
    del removed


assert Number.before == 20
assert Number.after == 10
assert Number.shadow == 30
assert Number.temporary == [2]
assert not hasattr(Number, 'removed'), 'deleted class member was published'

try:

    class Missing:
        del shadow
except NameError as exc:
    assert str(exc) == "name 'shadow' is not defined"
else:
    assert False, 'class deletion should not fall back to the global'
assert shadow == 10

# Rebinding inserts the key at the end, changing __set_name__ order.
set_names = []


class Descriptor:
    def __set_name__(self, owner, name):
        set_names.append(name)


class Ordered:
    first = Descriptor()
    second = Descriptor()
    del first
    first = Descriptor()


assert set_names == ['second', 'first']

# === Mapping, list and deque targets ===
mapping = {'a': [1], 'b': [2]}
del mapping['a']
assert mapping == {'b': [2]}

counter = Counter({'a': 2})
del counter['a'], counter['missing']
assert counter == Counter()

factory_calls = []


def factory():
    factory_calls.append(None)
    return []


with_default = defaultdict(factory, {'a': [1]})
del with_default['a']
try:
    del with_default['a']
    assert False, 'expected KeyError without calling default_factory'
except KeyError as exc:
    assert str(exc) == "'a'"
assert factory_calls == []

items = list(range(10))
del items[8:1:-3]
assert items == [0, 1, 3, 4, 6, 7, 9]
del items[::2], items[-1]
assert items == [1, 4]
del items[2:1], items[: 1 << 100 : 1 << 100]
assert items == [4]
del items[:]
assert items == []

items = list(range(6))
del items[3:1:-1]
assert items == [0, 1, 4, 5]
del items[-2:]
assert items == [0, 1]

cycle = []
cycle.append(cycle)
del cycle[::-1]
assert cycle == []


class Index:
    def __index__(self):
        return -1


items = [[0], [1]]
del items[Index()]
assert items == [[0]]

queue = deque([[1], [2], [3]])
iterator = iter(queue)
del queue[-2]
assert list(queue) == [[1], [3]]
try:
    next(iterator)
    assert False, 'expected deque mutation to invalidate the iterator'
except RuntimeError as exc:
    assert str(exc) == 'deque mutated during iteration'
del queue[Index()]
assert list(queue) == [[1]]

try:
    del getcontext().flags[InvalidOperation]
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'signal keys cannot be deleted'

# === User item/attribute hooks and evaluation order ===
log = []


class Items:
    def __delitem__(self, key):
        log.append(key)
        return 'ignored'


class Attrs:
    pass


def target():
    log.append('target')
    return Items()


def index():
    log.append('index')
    return 5


obj = Attrs()
obj.x = 1
remains = 100
try:
    del target()[index()], [obj.x, mapping['missing']], remains
    assert False, 'expected deletion to stop at the missing key'
except KeyError as exc:
    assert str(exc) == "'missing'"
assert log == ['target', 'index', 5]
assert not hasattr(obj, 'x'), 'earlier deletions took effect'
assert remains == 100

try:
    del items[100]
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'list assignment index out of range'

try:
    del items[::0]
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'slice step cannot be zero'

immutable = ()
try:
    del immutable[0]
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'tuple' object doesn't support item deletion"

# === Explicit exec namespaces ===
namespace = {'key': 1}
exec('del key', namespace)
assert 'key' not in namespace, 'the snippet deletes from its own namespace'
namespace['key'] = 1
locals_dict = {'key': 2}
exec('del key', namespace, locals_dict)
assert namespace['key'] == 1
assert locals_dict == {}
exec('global key; del key', namespace, locals_dict)
assert 'key' not in namespace, 'global declaration selects the globals dict'
