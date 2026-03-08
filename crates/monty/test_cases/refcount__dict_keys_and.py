inner = ('x',)
rhs = [(inner,), []]

try:
    {}.keys() & rhs
    assert False, 'dict_keys intersection should reject unhashable iterable items'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a set element (unhashable type: 'list')", (
        'dict_keys intersection should surface the recoverable set-element hash error'
    )

# ref-counts={'inner': 2, 'rhs': 1}
