x = {}
assert list(x.keys(*(), **{})) == []
for positional in [(), ([],)]:
    try:
        x.keys(*positional, bad=[])
    except TypeError as exc:
        assert str(exc) == 'dict.keys() takes no keyword arguments', str(exc)
    else:
        assert False, 'dict.keys accepted a keyword'
x.keys(1)
# Raise=TypeError('dict.keys() takes no arguments (1 given)')
