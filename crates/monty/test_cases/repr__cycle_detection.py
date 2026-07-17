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
