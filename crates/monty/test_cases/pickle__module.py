import pickle

assert pickle.DEFAULT_PROTOCOL == pickle.HIGHEST_PROTOCOL == 5
assert pickle.dumps(obj=42, protocol=4) == bytes.fromhex('80044b2a2e')
assert pickle.dumps(42, -1) == bytes.fromhex('80054b2a2e')
assert pickle.dumps(5, 2) == bytes.fromhex('80024b052e')
assert pickle.dumps((3 << 24) | 14, 2) == bytes.fromhex('80024a0e0000032e')

large_int = (1 << 2100) + 3
examples = {
    'scalars': (None, True, False, -1, 255, 256, 65535, 65536, -large_int, large_int, -0.0, 2.5),
    'strings': ('', 'café 👋', 'λ' * 256),
    'bytes': (b'', bytes(range(256)), b'x' * 70000),
    'list': [1, {'nested': True}],
    'tuple': ([], {}),
    'sets': ({1, 2}, frozenset({'x', 'y'}), set(), frozenset()),
    'dict keys': {('a', 5): 1, frozenset({1, 2}): 3},
}
for protocol in range(2, 6):
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

# CPython 3.14 protocol 2, with memoized constructors reused for sets.
legacy = bytes.fromhex(
    '800228635f5f6275696c74696e5f5f0a62797465730a710029527101635f636f646563730a656e636f64650a71025802000000'
    'c3bf710358060000006c6174696e317104867105527106635f5f6275696c74696e5f5f0a7365740a71075d7108284b014b0265'
    '85710952710a68075d710b4b036185710c52710d635f5f6275696c74696e5f5f0a66726f7a656e7365740a710e5d710f4b0461'
    '857110527111680e5d71124b05618571135271147471152e'
)
assert pickle.loads(legacy) == (b'', b'\xff', {1, 2}, {3}, frozenset({4}), frozenset({5}))
assert pickle.loads(bytes.fromhex('80025503666f6f2e')) == 'foo'  # Python 2 ASCII str

# CPython 3.14 protocol 5, including a frame, a tuple and a memo reference.
framed = bytes.fromhex('80059522000000000000005d94288c05636166c3a9948a0b00000000000000000000ff4301009465680086942e')
result = pickle.loads(framed)
assert result[0] == ['café', -(2**80), b'\x00']
assert result[0] is result[1]

recursive_list = pickle.loads(bytes.fromhex('80059506000000000000005d946800612e'))
assert recursive_list[0] is recursive_list
recursive_dict = pickle.loads(bytes.fromhex('8004950d000000000000007d948c0473656c66946800732e'))
assert recursive_dict['self'] is recursive_dict
