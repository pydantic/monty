# Read-before-write at module scope must raise `NameError`, not
# `UnboundLocalError`. Module scope has no `UnboundLocalError` in Python — only
# function scope does.
#
# Regression: when both a pre-write read and a post-write read of the same name
# appeared at module scope, the post-write read's `Local` compile would call
# `register_assigned_local` on the shared slot, and the pre-write read's
# `LoadGlobal` would then raise `UnboundLocalError` instead of `NameError`.
# Fixed by always tagging module-scope references as `NameScope::Global` so the
# compiler never calls `register_assigned_local` for them.
print(x)
x = 1
print(x)
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__unbound_local_module.py", line 11, in <module>
    print(x)
          ~
NameError: name 'x' is not defined
"""
