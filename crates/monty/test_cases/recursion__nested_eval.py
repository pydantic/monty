# Recursive callbacks evaluated by map()/filter()/sorted(key=...) re-enter the
# interpreter on the native Rust stack (via `evaluate_function`), not the
# heap-allocated Python frame stack. Verifies this is bounded by a
# RecursionError rather than a native stack overflow (SIGABRT) that would
# take the whole process down. Also true for CPython (its own C stack has a
# similar, if differently-sized, limit under recursive map/filter/sorted key
# calls), so both interpreters take the try/except path here.

# === Recursive map() ===
def f_map(x):
    return list(map(f_map, [x]))


try:
    f_map(1)
    raise AssertionError('expected RecursionError from unbounded map() self-recursion')
except RecursionError:
    pass


# === Recursive filter() ===
def f_filter(x):
    return list(filter(f_filter, [x]))


try:
    f_filter(1)
    raise AssertionError('expected RecursionError from unbounded filter() self-recursion')
except RecursionError:
    pass


# === Recursive sorted(key=...) ===
def f_sorted(x):
    return sorted([x], key=f_sorted)


try:
    f_sorted(1)
    raise AssertionError('expected RecursionError from unbounded sorted(key=...) self-recursion')
except RecursionError:
    pass


# === Positive case: comfortably under the cap, still correct ===
def double_via_map(x):
    if x <= 0:
        return [0]
    return list(map(lambda v: v, double_via_map(x - 1)))


result = double_via_map(5)
assert result == [0], f'expected [0], got {result}'
