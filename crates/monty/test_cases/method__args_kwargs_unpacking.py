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

# ============================================================
# **kwargs unpacking tests
# ============================================================

# === Basic **kwargs unpacking with dict.update ===
d = {'a': 1}
opts = {'b': 2, 'c': 3}
d.update(**opts)
assert d == {'a': 1, 'b': 2, 'c': 3}, f'update with **kwargs: {d}'

# === Empty **kwargs unpacking ===
d = {'a': 1}
empty_opts = {}
d.update(**empty_opts)
assert d == {'a': 1}, f'update with empty **kwargs: {d}'

# === **kwargs with string keys ===
d = {}
str_opts = {'key1': 'value1', 'key2': 'value2'}
d.update(**str_opts)
assert d == {'key1': 'value1', 'key2': 'value2'}, f'update with string **kwargs: {d}'

# === **kwargs with heap-allocated values ===
d = {}
list_val = [1, 2, 3]
dict_val = {'nested': True}
heap_opts = {'list': list_val, 'dict': dict_val}
d.update(**heap_opts)
assert d['list'] == [1, 2, 3], f'update with list value: {d}'
assert d['dict'] == {'nested': True}, f'update with dict value: {d}'

# === Multiple **kwargs updates ===
d = {'a': 1}
opts1 = {'b': 2}
opts2 = {'c': 3}
d.update(**opts1)
d.update(**opts2)
assert d == {'a': 1, 'b': 2, 'c': 3}, f'multiple updates with **kwargs: {d}'

# === **kwargs overwriting existing keys ===
d = {'a': 1, 'b': 2}
override_opts = {'b': 'new', 'c': 3}
d.update(**override_opts)
assert d == {'a': 1, 'b': 'new', 'c': 3}, f'update overwriting with **kwargs: {d}'

# === Mixed *args and **kwargs with dict.update ===
# dict.update can take a dict positionally AND **kwargs
d = {'a': 1}
pos_update = {'b': 2}
kw_update = {'c': 3}
d.update(pos_update, **kw_update)
assert d == {'a': 1, 'b': 2, 'c': 3}, f'update with pos and **kwargs: {d}'

# === *args tuple unpacking combined with method ===
d = {'a': 1}
args_tuple = ({'x': 10},)
d.update(*args_tuple)
assert d == {'a': 1, 'x': 10}, f'update with *args tuple: {d}'

# === Combined *args and **kwargs ===
d = {}
pos_dict = {'a': 1}
kw_opts = {'b': 2}
d.update(*[pos_dict], **kw_opts)
assert d == {'a': 1, 'b': 2}, f'update with *args and **kwargs: {d}'

# === Regular kwargs combined with **kwargs ===
# This tests the code path where we have both explicit keyword args and **kwargs unpacking
d = {}
extra_opts = {'c': 3}
d.update(a=1, b=2, **extra_opts)
assert d == {'a': 1, 'b': 2, 'c': 3}, f'update with regular kwargs and **kwargs: {d}'

# === Regular kwargs only (no **kwargs) with method call ===
d = {}
d.update(x=10, y=20)
assert d == {'x': 10, 'y': 20}, f'update with regular kwargs only: {d}'

# === Mixed positional, regular kwargs, and **kwargs ===
d = {'existing': 0}
pos_update = {'a': 1}
extra = {'d': 4}
d.update(pos_update, b=2, c=3, **extra)
assert d == {'existing': 0, 'a': 1, 'b': 2, 'c': 3, 'd': 4}, f'update with pos, kwargs, **kwargs: {d}'

# === Empty **kwargs with regular kwargs ===
d = {}
empty_extra = {}
d.update(x=1, **empty_extra)
assert d == {'x': 1}, f'update with kwargs and empty **kwargs: {d}'

# === **kwargs with different keys from regular kwargs ===
d = {}
extra = {'b': 'from_dict'}
d.update(a='original', **extra)
assert d == {'a': 'original', 'b': 'from_dict'}, f'update with different kwargs: {d}'
