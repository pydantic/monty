try:
    raise ValueError
except ValueError as e:
    assert str(e) == ''
    assert repr(e) == 'ValueError()'
else:
    raise AssertionError('should raise an error')

try:
    raise ValueError()
except ValueError as e:
    assert str(e) == ''
    assert repr(e) == 'ValueError()'
else:
    raise AssertionError('should raise an error')
