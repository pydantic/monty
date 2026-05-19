# Duplicate parameter names in a function definition are a SyntaxError.
# Previously this panicked the VM at call time because `prepare.rs` sized
# the frame's namespace from the unique name count (HashMap::len) but
# resolved the duplicate to a NamespaceId from the positional index,
# so `load_local` indexed past the allocated stack slots.
def f(x, x):
    return x
# Raise=SyntaxError("duplicate argument 'x' in function definition")
