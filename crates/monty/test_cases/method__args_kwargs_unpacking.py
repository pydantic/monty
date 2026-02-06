# Tests for method calls with *args unpacking

# === Basic *args unpacking ===
items = ['a', 'b', 'c']
result = '-'.join(*[items])
assert result == 'a-b-c', f'join with *args: {result}'

parts = ['hello', 'world']
result = ' '.join(*[parts])
assert result == 'hello world', f'join with *args list: {result}'

# === Empty *args unpacking ===
result = '-'.join(*[[]])
assert result == '', f'join with empty *args: {result}'

empty = []
result = '-'.join(*[empty])
assert result == '', f'join with empty list via *args: {result}'

# === *args with tuple unpacking ===
values = ('x', 'y', 'z')
result = '|'.join(*[list(values)])
assert result == 'x|y|z', f'join with tuple *args: {result}'

# === String methods with *args ===
s = 'hello world'
args = ('o', 'O')
result = s.replace(*args)
assert result == 'hellO wOrld', f'replace with *args: {result}'

# Count with *args
count_args = ('l',)
result = s.count(*count_args)
assert result == 3, f'count with *args: {result}'

# === List methods with *args ===
my_list = [1, 2, 3]
append_args = [4]
my_list.append(*append_args)
assert my_list == [1, 2, 3, 4], f'append with *args: {my_list}'

my_list = [1, 2, 3]
extend_args = [[4, 5]]
my_list.extend(*extend_args)
assert my_list == [1, 2, 3, 4, 5], f'extend with *args: {my_list}'

my_list = [1, 2, 3]
insert_args = (1, 'x')
my_list.insert(*insert_args)
assert my_list == [1, 'x', 2, 3], f'insert with *args: {my_list}'

# === Dict methods with *args ===
d = {'a': 1, 'b': 2}
get_args = ('a',)
result = d.get(*get_args)
assert result == 1, f'dict.get with *args: {result}'

get_args_default = ('missing', 'default')
result = d.get(*get_args_default)
assert result == 'default', f'dict.get with *args and default: {result}'

# === Mixed positional and *args ===
my_list = [1, 2, 3]
extra_args = ('y',)
my_list.insert(0, *extra_args)
assert my_list == ['y', 1, 2, 3], f'insert with pos and *args: {my_list}'

# === setdefault with *args ===
d = {'a': 1}
args = ('b', 2)
result = d.setdefault(*args)
assert result == 2, f'setdefault with *args: {result}'
assert d == {'a': 1, 'b': 2}, f'dict after setdefault: {d}'

# === pop with *args ===
d = {'a': 1, 'b': 2}
pop_args = ('a',)
result = d.pop(*pop_args)
assert result == 1, f'pop with *args: {result}'
assert d == {'b': 2}, f'dict after pop: {d}'

pop_args_default = ('missing', 'default')
result = d.pop(*pop_args_default)
assert result == 'default', f'pop with *args and default: {result}'

# === String split with *args ===
s = 'a,b,c,d'
split_args = (',',)
result = s.split(*split_args)
assert result == ['a', 'b', 'c', 'd'], f'split with *args: {result}'

split_args_maxsplit = (',', 2)
result = s.split(*split_args_maxsplit)
assert result == ['a', 'b', 'c,d'], f'split with *args maxsplit: {result}'

# === String startswith/endswith with *args ===
s = 'hello'
startswith_args = (('hel', 'hey'),)
result = s.startswith(*startswith_args)
assert result == True, f'startswith with *args tuple: {result}'

endswith_args = ('lo',)
result = s.endswith(*endswith_args)
assert result == True, f'endswith with *args: {result}'

# === List index with *args ===
my_list = [1, 2, 3, 2, 4]
index_args = (2,)
result = my_list.index(*index_args)
assert result == 1, f'index with *args: {result}'

index_args_start = (2, 2)
result = my_list.index(*index_args_start)
assert result == 3, f'index with *args and start: {result}'

# === String find with *args ===
s = 'hello hello'
find_args = ('hello',)
result = s.find(*find_args)
assert result == 0, f'find with *args: {result}'

find_args_start = ('hello', 1)
result = s.find(*find_args_start)
assert result == 6, f'find with *args and start: {result}'
