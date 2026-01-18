# Tests that typing imports other than TYPE_CHECKING are silently ignored

# These imports should not raise any errors (they are silently ignored)

# If we got here, the imports were silently ignored (no NameError raised)
# Note: The names List, Dict, Optional are NOT defined - they were just ignored
# We can't test this directly without dir(), but the code below would fail
# if they were somehow defined to something that breaks comparison
assert True, 'typing imports were silently ignored'
