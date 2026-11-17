class Table:
    def __init__(self, result):
        self.result = result

    def __getitem__(self, key):
        if key == 97:
            return self.result
        raise KeyError('missing')


value = ['kept']
source = {'a': value, 98: value}
table = str.maketrans(source)

rep = 'replacement' * 3
mapping = {97: rep, 98: None}
out = 'abcab'.translate(mapping)
wide = 'aébé'.translate(mapping)

user = Table(rep)
user_out = 'abab'.translate(user)

try:
    str.maketrans({'a': value, 2.5: value})
except TypeError:
    pass

try:
    'ab'.translate({97: rep, 98: 1.5})
except TypeError:
    pass

user_out
# ref-counts={'Table': 2, 'value': 5, 'source': 1, 'table': 1, 'rep': 3, 'mapping': 1, 'out': 1, 'wide': 1, 'user': 1, 'user_out': 2}
