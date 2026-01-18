# xfail=cpython
# Tests that unknown typing imports are silently ignored
# xfail reason: CPython raises ImportError for unknown typing names,
# Monty silently ignores them to be more permissive with type hints

from typing import SomeUnknownTypingConstruct

# Unknown typing constructs are silently ignored - they don't raise errors
# but they also don't create any binding, so accessing them would fail

# We can verify the import succeeded without error by reaching this point
assert True, 'unknown typing imports were silently ignored'
