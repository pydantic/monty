# Reference counts for values stored through attribute and subscript targets.


class Box:
    def __init__(self):
        self.a = None
        self.b = None


box = Box()
first = [1, 2]
second = [3]
box.a, box.b = first, second
assert box.a is first
assert box.b is second

store = {}
store['x'], store['y'] = second, first
assert store['x'] is second
assert store['y'] is first

# overwriting a target releases the value it held
box.a, box.b = second, second
assert box.a is second
store['x'], store['y'] = first, first
assert store['x'] is first
assert store['y'] is first

# Box: the class, plus the instance's reference to it
# first: first var, store['x'], store['y']
# second: second var, box.a, box.b
# box: box var, final expression
# store: store var
box
# ref-counts={'Box': 2, 'first': 3, 'second': 3, 'box': 2, 'store': 1}
