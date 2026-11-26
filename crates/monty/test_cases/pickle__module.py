import pickle

assert pickle.DEFAULT_PROTOCOL == pickle.HIGHEST_PROTOCOL == 5
assert pickle.dumps(obj=42, protocol=4) == bytes.fromhex('80044b2a2e')
assert pickle.dumps(42, -1) == bytes.fromhex('80054b2a2e')

large_int = (1 << 2100) + 3
examples = {
    'scalars': (None, True, False, -1, 255, 256, 65535, 65536, -large_int, large_int, -0.0, 2.5),
    'strings': ('', 'café 👋', 'λ' * 256),
    'bytes': (b'', b'\x00\xff', b'x' * 70000),
    'list': [1, {'nested': True}],
    'tuple': ([], {}),
    'sets': ({1, 2}, frozenset({'x', 'y'}), set(), frozenset()),
    'dict keys': {('a', 5): 1, frozenset({1, 2}): 3},
}
for protocol in (4, 5):
    assert pickle.loads(pickle.dumps(examples, protocol) + b'trailing') == examples

    shared = ['heap']
    result = pickle.loads(pickle.dumps((shared, shared), protocol))
    assert result[0] is result[1]

    many = [[i] for i in range(300)]
    result = pickle.loads(pickle.dumps((many, many[-1]), protocol))
    assert result[0][-1] is result[1]

    recursive = []
    recursive.append(recursive)
    result = pickle.loads(pickle.dumps(recursive, protocol))
    assert result[0] is result

    child = {}
    parent = (child,)
    child['parent'] = parent
    result = pickle.loads(pickle.dumps(parent, protocol))
    assert result[0]['parent'] is result

# CPython 3.14 protocol 5, including a frame, a tuple and a memo reference.
framed = bytes.fromhex('80059522000000000000005d94288c05636166c3a9948a0b00000000000000000000ff4301009465680086942e')
result = pickle.loads(framed)
assert result[0] == ['café', -(2**80), b'\x00']
assert result[0] is result[1]

recursive_list = pickle.loads(bytes.fromhex('80059506000000000000005d946800612e'))
assert recursive_list[0] is recursive_list
recursive_dict = pickle.loads(bytes.fromhex('8004950d000000000000007d948c0473656c66946800732e'))
assert recursive_dict['self'] is recursive_dict
