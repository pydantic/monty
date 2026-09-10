# Mutating a dict during iteration must raise even when the size change lands on
# the LAST element yielded: the terminating step still checks the size guard.
# Regression for DictIteratorState::next_index, which checked exhaustion before
# the size change and so missed the terminal step for keys/items/values/pop alike.

# === for k in d: insert on the last key ===
try:
    d = {'a': 1, 'b': 2}
    for k in d:
        if k == 'b':
            d['z'] = 9
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'

# === d.items(): insert on the last pair ===
try:
    d = {'a': 1, 'b': 2}
    for k, v in d.items():
        if k == 'b':
            d['z'] = 9
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'

# === d.values(): insert on the last value ===
try:
    d = {'a': 1, 'b': 2}
    for v in d.values():
        if v == 2:
            d['z'] = 9
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'

# === delete (pop) on the last key ===
try:
    d = {'a': 1, 'b': 2}
    for k in d:
        if k == 'b':
            d.pop('a')
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'

# === single-element dict: the only element is also the last ===
try:
    d = {'a': 1}
    for k in d:
        d['z'] = 9
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'
