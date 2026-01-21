# === Typing markers via from import ===
from typing import Any, Callable, Dict, List, Optional, Set, Tuple, Union

# These are now assigned to Marker values (not silently ignored)
# Test repr() to verify they have the correct string representation
assert repr(Any) == 'typing.Any', f'Any repr should be Any, got {Any!r}'
assert repr(Optional) == 'typing.Optional', f'Optional repr should be Optional, got {Optional!r}'
assert repr(Union) == "<class 'typing.Union'>", f'Union repr should be <class typing.Union>, got {Union!r}'
assert repr(List) == 'typing.List', f'List repr should be List, got {List!r}'
assert repr(Dict) == 'typing.Dict', f'Dict repr should be Dict, got {Dict!r}'
assert repr(Tuple) == 'typing.Tuple', f'Tuple repr should be Tuple, got {Tuple!r}'
assert repr(Set) == 'typing.Set', f'Set repr should be Set, got {Set!r}'
assert repr(Callable) == 'typing.Callable', f'Callable repr should be Callable, got {Callable!r}'

# === Typing markers via module import ===
import typing

assert repr(typing.Any) == 'typing.Any'
assert repr(typing.Optional) == 'typing.Optional'
assert repr(typing.Union) == "<class 'typing.Union'>"

# === Aliased imports ===
from typing import Any as AnyType

assert repr(AnyType) == 'typing.Any'
