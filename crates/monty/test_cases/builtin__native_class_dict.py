from collections import defaultdict
from datetime import date

namespace = list.__dict__
assert type(namespace).__name__ == 'mappingproxy'
assert namespace is not list.__dict__
assert namespace == list.__dict__
assert namespace['append'] is list.append
assert namespace.get('append') is list.append
assert namespace.get('missing', 42) == 42
assert 'append' in namespace.keys()
assert list.append in namespace.values()
assert ('append', list.append) in namespace.items()
assert len(namespace) == len(list(namespace))
assert list(reversed(namespace)) == list(namespace)[::-1]
assert repr(namespace).startswith('mappingproxy({')
assert str(namespace).startswith('{')
assert 'get' not in defaultdict.__dict__
assert defaultdict.get is dict.get
assert date.__dict__['min'] == date.min

copied = namespace.copy()
assert copied.pop('append') is list.append
assert namespace['append'] is list.append
assert dict(namespace) == namespace
copied.update(namespace)
assert copied == namespace
assert {**namespace} == copied
assert (namespace | {'append': 3})['append'] == 3
assert ({'append': 3} | namespace)['append'] is list.append
match namespace:
    case {'append': method}:
        assert method is list.append
    case _:
        assert False, 'mapping pattern missed append'

try:
    namespace['append'] = None
except TypeError:
    pass
else:
    assert False, 'wrote through a class namespace'

try:
    namespace |= {'extra': True}
except TypeError:
    pass
else:
    assert False, 'updated a class namespace'

assert not hasattr(namespace, 'clear')
try:
    dict.clear(namespace)
except TypeError:
    pass
else:
    assert False, 'dict.clear accepted a class namespace'

raw = dict.__dict__['fromkeys']
assert type(raw).__name__ == 'classmethod_descriptor'
assert raw.__objclass__ is dict
assert raw is dict.__dict__['fromkeys']
assert raw(dict, ['a'], 42) == {'a': 42}
assert type(raw(defaultdict, ['a'])) is defaultdict
assert raw.__get__(None, defaultdict).__self__ is defaultdict
assert raw.__get__({}).__self__ is dict
for wrong in [list, {}]:
    try:
        raw(wrong, [])
    except TypeError:
        pass
    else:
        assert False, 'classmethod descriptor accepted an unrelated receiver'
