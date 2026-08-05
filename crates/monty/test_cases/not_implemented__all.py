# === Boolean contexts ===
for operation, name in [
    (lambda: bool(NotImplemented), 'bool'),
    (lambda: not NotImplemented, 'not'),
    (lambda: NotImplemented and True, 'and'),
    (lambda: any([NotImplemented]), 'any'),
]:
    try:
        operation()
        assert False, f'{name} should reject NotImplemented truth testing'
    except TypeError as exc:
        assert str(exc) == 'NotImplemented should not be used in a boolean context', f'{name} error message'
