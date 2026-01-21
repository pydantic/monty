# Tests for typing module type() behavior
#
# CPython's typing module uses various internal types for different constructs.
# Monty simplifies this by using typing._SpecialForm for all typing markers.
# Where CPython also uses _SpecialForm, we use == for exact match.
# Where CPython uses different internal types, we accept both representations.

import typing

# === Types that match between CPython and Monty ===
assert repr(type(typing.Optional)) == "<class 'typing._SpecialForm'>", 'type(Optional)'
assert repr(type(typing.ClassVar)) == "<class 'typing._SpecialForm'>", 'type(ClassVar)'
assert repr(type(typing.Final)) == "<class 'typing._SpecialForm'>", 'type(Final)'
assert repr(type(typing.Union)) == "<class 'type'>", 'type(Union)'

# === Types that differ between CPython and Monty ===
# CPython uses specialized internal types; Monty uses _SpecialForm for all
assert repr(type(typing.Any)) in ("<class 'typing._SpecialForm'>", "<class 'typing._AnyMeta'>"), 'type(Any)'
assert repr(type(typing.Callable)) in ("<class 'typing._SpecialForm'>", "<class 'typing._CallableType'>"), (
    'type(Callable)'
)

# === Verify TYPE_CHECKING is False ===
assert typing.TYPE_CHECKING is False, 'TYPE_CHECKING should be False at runtime'
