# Tests keyword arguments for print()
import sys

# === Dynamic sep via **kwargs ===
dynamic_sep = 's' + 'e' + 'p'
result = print('left', 'right', **{dynamic_sep: '-'})
assert result is None


# === Dynamic end via **kwargs ===
dynamic_end = 'e' + 'n' + 'd'
result2 = print('line', **{dynamic_end: ''})
assert result2 is None


# === file=sys.stdout / sys.stderr ===
# Output goes to the host, so these only assert that both are accepted;
# where the text ends up is covered by the Rust and pytest suites.
assert print('to stdout', file=sys.stdout) is None
assert print('to stderr', file=sys.stderr) is None
assert print('default', file=None) is None


# === sep and end are reported before file ===
try:
    print('x', sep=1, file=3)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'sep must be None or a string, not int'
try:
    print('x', end=1, file=3)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'end must be None or a string, not int'
