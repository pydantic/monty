# Tests reference counting correctness for **kwargs unpacking


def receive_kwargs(a, b, c):
    return a


# === Heap-allocated values in unpacked dict ===
# All heap objects must be directly referenced by variables for strict matching
list_a = [1, 2, 3]
list_c = [4, 5]
kwargs_dict = {'a': list_a, 'b': 'hello', 'c': list_c}
result = receive_kwargs(**kwargs_dict)
assert result == [1, 2, 3], 'received list via **kwargs'
assert result is list_a, 'should be same object'

# Second call to verify dict reuse works
result2 = receive_kwargs(**kwargs_dict)
assert result2 is list_a, 'second call returns same object'

# list_a: 5 refs (list_a var, kwargs_dict['a'], result, result2, final expr)
# list_c: 2 refs (list_c var, kwargs_dict['c'])
# kwargs_dict: 1 ref
# result: 5 refs (same object as list_a)
# result2: 5 refs (same object as list_a)
result2
# ref-counts={'list_a': 5, 'list_c': 2, 'kwargs_dict': 1, 'result': 5, 'result2': 5}
