# === TYPE_CHECKING guard ===
# Imports inside TYPE_CHECKING blocks should not raise errors at runtime
# because TYPE_CHECKING is False at runtime, so the import is never executed.
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import also_nonexistent
    from nonexistent_module import something

# Verify TYPE_CHECKING is False at runtime (as expected)
assert TYPE_CHECKING is False, 'TYPE_CHECKING should be False at runtime'


# === Function using TYPE_CHECKING for conditional import ===
def get_type_checking_value():
    if TYPE_CHECKING:
        from another_fake_module import FakeType
    return 'success'


result = get_type_checking_value()
assert result == 'success', 'function with TYPE_CHECKING guard should execute'

# === Nested TYPE_CHECKING blocks ===
if TYPE_CHECKING:
    if True:
        from deeply_nested_fake import DeepFake

# === TYPE_CHECKING in else branch (should not be executed either) ===
x = True
if x:
    pass
else:
    if TYPE_CHECKING:
        from unreachable_module import Unreachable

assert True, 'all TYPE_CHECKING guards work correctly'
