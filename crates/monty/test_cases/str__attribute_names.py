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

# === Native attribute tables accept heap-backed names ===
attribute = ''.join(['__init', '_subclass__'])
hook = getattr(dict, attribute)
attribute = None
assert hook() is None
values = {1, 2}
assert set.symmetric_difference(values, {2, 3}) == {1, 3}

missing = ''.join(['missing_native', '_attribute_name'])
try:
    getattr(values, missing)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'set' object has no attribute 'missing_native_attribute_name'"
