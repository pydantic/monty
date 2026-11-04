for method in ['find', 'count', 'index', 'rfind', 'rindex', 'startswith', 'endswith']:
    for position in ['start', 'end']:
        for mutation in ['clear', 'grow']:
            a = bytearray(b'abc')
            calls = []

            class Bound:
                def __init__(self, name):
                    self.name = name

                def __index__(self):
                    calls.append(self.name)
                    if self.name == position:
                        if mutation == 'clear':
                            a.clear()
                        else:
                            a.extend(b'abc')
                    return 0 if self.name == 'start' else 6

            try:
                if method == 'find':
                    result = a.find(b'a', Bound('start'), Bound('end'))
                elif method == 'count':
                    result = a.count(b'a', Bound('start'), Bound('end'))
                elif method == 'index':
                    result = a.index(b'a', Bound('start'), Bound('end'))
                elif method == 'rfind':
                    result = a.rfind(b'a', Bound('start'), Bound('end'))
                elif method == 'rindex':
                    result = a.rindex(b'a', Bound('start'), Bound('end'))
                elif method == 'startswith':
                    result = a.startswith(b'a', Bound('start'), Bound('end'))
                elif method == 'endswith':
                    result = a.endswith(b'cabc', Bound('start'), Bound('end'))
            except ValueError:
                assert method in ['index', 'rindex'] and mutation == 'clear'
            else:
                expected = {
                    'find': 0 if mutation == 'grow' else -1,
                    'count': 2 if mutation == 'grow' else 0,
                    'index': 0,
                    'rfind': 3 if mutation == 'grow' else -1,
                    'rindex': 3,
                    'startswith': mutation == 'grow',
                    'endswith': mutation == 'grow',
                }
                assert result == expected[method], (method, position, mutation, result)
            assert calls == ['start', 'end']
