# keyword is vendored from CPython's own source, with a patch; see crates/monty/stdlib/
import keyword
from keyword import iskeyword, issoftkeyword, kwlist, softkwlist

assert keyword.__all__ == ['iskeyword', 'issoftkeyword', 'kwlist', 'softkwlist']

# === Keyword lists ===
assert len(kwlist) == 35
assert kwlist[:5] == ['False', 'None', 'True', 'and', 'as']
assert 'async' in kwlist
assert softkwlist == ['_', 'case', 'match', 'type']

# === Predicates ===
assert iskeyword('for') is True
assert iskeyword('None') is True
assert iskeyword('print') is False
assert iskeyword('match') is False
assert issoftkeyword('match') is True
assert issoftkeyword('_') is True
assert issoftkeyword('for') is False
assert [w for w in ['def', 'x', 'lambda', 'type', 'yield'] if iskeyword(w)] == ['def', 'lambda', 'yield']
assert iskeyword(1) is False

try:
    iskeyword([])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot use 'list' as a set element (unhashable type: 'list')"
