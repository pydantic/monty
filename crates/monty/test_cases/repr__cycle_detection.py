# Test cycle detection in repr for self-referential structures

# Section 1: List self-reference
a = []
a.append(a)
assert repr(a) == '[[...]]'

# Section 2: Dict self-reference
d = {}
d['self'] = d
assert repr(d) == "{'self': {...}}"

# Section 3: Composite - list containing dict containing original list
c = []
e = {'list': c}
c.append(e)
assert repr(c) == "[{'list': [...]}]"
assert repr(e) == "{'list': [{...}]}"

# Section 4: Multiple references to same cyclic object
f = []
f.append(f)
g = [f, f]
assert repr(g) == '[[[...]], [[...]]]'

# Section 5: List containing exception containing list
exc_lst = []
exc = ValueError(exc_lst)
exc_lst.append(exc)
assert repr(exc_lst) == '[ValueError([...])]'

# Section 6: Self-referential exception repr / str raises RecursionError
e_self = ValueError('x')
e_self.args = (e_self,)
try:
    repr(e_self)
    assert False, 'expected RecursionError'
except RecursionError:
    pass

try:
    str(e_self)
    assert False, 'expected RecursionError'
except RecursionError:
    pass


# Section 7: Failing __str__ in exception argument
class BadStr:
    def __str__(self):
        raise RuntimeError('str failed')


bad_exc = ValueError(BadStr())
try:
    str(bad_exc)
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'str failed'
