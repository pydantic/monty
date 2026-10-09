# Test cycle detection in repr for self-referential structures

from collections import namedtuple

# Section 1: List self-reference
a = []
a.append(a)
assert repr(a) == '[[...]]'
assert str(a) == '[[...]]'

# Section 2: Dict self-reference
d = {}
d['self'] = d
assert repr(d) == "{'self': {...}}"
assert str(d) == "{'self': {...}}"

# Section 3: Composite - list containing dict containing original list
c = []
e = {'list': c}
c.append(e)
assert repr(c) == "[{'list': [...]}]"
assert repr(e) == "{'list': [{...}]}"
assert str(c) == "[{'list': [...]}]"
assert str(e) == "{'list': [{...}]}"

# Section 4: Multiple references to same cyclic object
f = []
f.append(f)
g = [f, f]
assert repr(g) == '[[[...]], [[...]]]'
assert str(g) == '[[[...]], [[...]]]'

# Section 5: Self-reference alongside non-cyclic elements
cy = [1]
cy.append(cy)
assert repr(cy) == '[1, [...]]'
assert str(cy) == '[1, [...]]'
assert f'{cy}' == '[1, [...]]'
assert f'{cy!s}' == '[1, [...]]'

mapping = {'first': 1}
mapping['self'] = mapping
assert repr(mapping) == "{'first': 1, 'self': {...}}"
assert str(mapping) == "{'first': 1, 'self': {...}}"
assert f'{mapping}' == "{'first': 1, 'self': {...}}"

# Section 6: Tuple participating in an indirect cycle
items = []
wrapped = (items,)
items.append(wrapped)
assert repr(wrapped) == '([(...)],)'
assert str(wrapped) == '([(...)],)'

# Section 7: Namedtuple str keeps its custom repr's nested expansion
Named = namedtuple('Named', 'items')
named_items = []
named = Named(named_items)
named_items.append(named)
assert str(named) == 'Named(items=[Named(items=[...])])'


# Section 8: String and user-defined conversions keep their own behaviour
class Text:
    def __str__(self):
        return 'custom str'

    def __repr__(self):
        return 'custom repr'


assert str('plain') == 'plain'
assert str(Text()) == 'custom str'
assert repr(Text()) == 'custom repr'
