name = 'Thing!'[:-1]
C = type(name, (), {})
assert C.__name__ == name
obj = C()
attribute = 'zzq!'[:-1]
setattr(obj, attribute, 123)
assert getattr(obj, attribute) == 123
assert hasattr(obj, attribute)
missing = 'missing!'[:-1]
assert not hasattr(obj, missing)
try:
    getattr(obj, missing)
    assert False
except AttributeError as exc:
    assert 'Thing' in str(exc)
assert getattr(slice(1, 5), 'start!'[:-1]) == 1
for value in [obj, C, list[int]]:
    try:
        getattr(value, 'absent!'[:-1])
        assert False
    except AttributeError:
        pass
