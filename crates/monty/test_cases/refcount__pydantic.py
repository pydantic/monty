# A `TypeAdapter`'s arena owns the literal choices, dataclass defaults and
# class objects its validators hold, so a missing arm in `py_dec_ref_ids` /
# `for_each_child_id` only shows up under `ref-count-return` /
# `memory-model-checks`.
#
# `default` ends at 6: its binding, the class attribute, the `Field` default,
# the generated `__init__`'s copy, the arena's copy, and `built.pair`, which
# the validator took from the arena's copy. `Foo` ends at 3: its binding, the
# arena's reference and the built instance's class reference. Validated
# outputs are fresh objects, so `built` ends at 1.
from collections import namedtuple
from dataclasses import dataclass
from typing import Literal

from pydantic import TypeAdapter, ValidationError

choice = 'a'
chooser = TypeAdapter(Literal[choice, 'b'])
assert chooser.validate_python('b') == 'b'

default = (1, 2)


@dataclass
class Foo:
    x: int
    pair: tuple = default


adapter = TypeAdapter(Foo)
built = adapter.validate_python({'x': 1})
assert built.pair is default

# The cycle collector must reach an adapter's values: this list holds the only
# reference to an adapter whose literal holds the list.
cyclic = []
cyclic.append(TypeAdapter(Literal[cyclic]))
cyclic = None

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# the arena leaves the choices alive with no referrer.
dropped = TypeAdapter(Literal[[4, 5]])
dropped = None

# A namedtuple whose JSON ends mid-arguments releases the arguments it read.
Point = namedtuple('Point', ['x', 'y'])
for document in ['{"x": "%s", "y": ' % ('a' * 40), '["%s", ' % ('a' * 40)]:
    try:
        TypeAdapter(Point).validate_json(document)
        assert False, 'expected a ValidationError'
    except ValidationError:
        pass


# Every missing field's error holds the same input, released with the errors.
@dataclass
class Pair:
    a: int
    b: int


for hint in [Pair, Point]:
    try:
        TypeAdapter(hint).validate_json('{"c": [1]}')
        assert False, 'expected a ValidationError'
    except ValidationError:
        pass
    try:
        TypeAdapter(hint).validate_python({'c': [1]})
        assert False, 'expected a ValidationError'
    except ValidationError:
        pass
hint = None

len(choice)
# ref-counts={'default': 6, 'Foo': 3, 'adapter': 1, 'built': 1, 'chooser': 1, 'Point': 1, 'document': 1, 'Pair': 1}
