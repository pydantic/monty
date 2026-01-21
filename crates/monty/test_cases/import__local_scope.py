# Tests that import inside functions binds to local scope, not global

# === Import statement inside function ===
def test_import_local():
    import sys

    return sys.platform


# Call to verify import works inside function
result = test_import_local()
assert isinstance(result, str), 'sys.platform should be a string'

# Verify sys is NOT in global scope after function call
try:
    sys
    assert False, 'sys should not be in global scope'
except NameError:
    pass  # Expected: sys is local to the function


# === From import inside function ===
def test_from_import_local():
    from typing import Any

    return Any


any_result = test_from_import_local()
assert repr(any_result) == 'typing.Any', 'should return typing.Any'

# Verify Any is NOT in global scope after function call
try:
    Any
    assert False, 'Any should not be in global scope'
except NameError:
    pass  # Expected: Any is local to the function


# === Aliased import inside function ===
def test_aliased_import_local():
    import sys as system

    return system.platform


alias_result = test_aliased_import_local()
assert isinstance(alias_result, str), 'system.platform should be a string'

# Verify system is NOT in global scope
try:
    system
    assert False, 'system should not be in global scope'
except NameError:
    pass  # Expected: system is local to the function

# === Global import remains accessible ===
import sys as global_sys

assert isinstance(global_sys.platform, str), 'global import should work'


def use_global_import():
    # This should access the global sys, not create a new local
    return global_sys.platform


assert use_global_import() == global_sys.platform, 'function should access global import'
