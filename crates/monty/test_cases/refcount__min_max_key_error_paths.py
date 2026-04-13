# Tests reference counting when min()/max() key functions raise on the first item.
#
# Both the iterable form and the multiple-argument form must guard the initial
# candidate before calling the user-provided key function. Otherwise the first
# winner leaks when key evaluation raises before the comparison loop starts.


def raising_key(value):
    raise ValueError('boom')


item_iter = ['iter']
item_multi = ['multi']
other_multi = ['other']

try:
    max([item_iter], key=raising_key)
    assert False, 'max(iterable, key=raising_key) should raise ValueError'
except ValueError as e:
    assert e.args == ('boom',), 'max iterable key error should propagate unchanged'

try:
    min(item_multi, other_multi, key=raising_key)
    assert False, 'min(arg1, arg2, key=raising_key) should raise ValueError'
except ValueError as e:
    assert e.args == ('boom',), 'min multi-arg key error should propagate unchanged'

# The temporary argument container for max() and the current winner slots in
# both builtin code paths must be released after the handled exception.
# ref-counts={'item_iter': 1, 'item_multi': 1, 'other_multi': 1}
