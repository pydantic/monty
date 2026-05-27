# mount-fs

# === Basic with + open ===
with open(root / 'open_with.txt', 'w') as f:
    assert f.write('hello') == 5, 'write returns char count'
    assert f.closed == False, 'file is open inside with'
assert f.closed == True, 'file is closed after with'
assert (root / 'open_with.txt').read_text() == 'hello', 'write committed after with'

# === Read with `with` ===
with open(root / 'hello.txt') as r:
    assert r.read() == 'hello world\n', 'read works inside with'
assert r.closed == True, 'reader closed after with'

# === `__enter__` returns the file itself ===
pre = open(root / 'hello.txt')
with pre as bound:
    assert bound is pre, '__enter__ returns the file itself'
assert pre.closed == True, 'file closed after with even when ctx is precreated'

# === Closed on exception ===
errfile = open(root / 'hello.txt')
caught = None
try:
    with errfile:
        raise ValueError('boom')
except ValueError as e:
    caught = str(e)
assert caught == 'boom', 'exception propagates out of with'
assert errfile.closed == True, 'file closed even on exception'

# === Without `as` target ===
with open(root / 'hello.txt'):
    pass

# === Nested with ===
with open(root / 'open_with_a.txt', 'w') as a:
    with open(root / 'open_with_b.txt', 'w') as b:
        a.write('A')
        b.write('B')
assert a.closed == True, 'outer closed after nested with'
assert b.closed == True, 'inner closed after nested with'
assert (root / 'open_with_a.txt').read_text() == 'A', 'outer write persisted'
assert (root / 'open_with_b.txt').read_text() == 'B', 'inner write persisted'

# === Multi-item with (desugared to nested) ===
with open(root / 'open_with_m1.txt', 'w') as m1, open(root / 'open_with_m2.txt', 'w') as m2:
    m1.write('M1')
    m2.write('M2')
assert m1.closed == True, 'first manager closed after multi-item with'
assert m2.closed == True, 'second manager closed after multi-item with'
assert (root / 'open_with_m1.txt').read_text() == 'M1', 'first manager write persisted'
assert (root / 'open_with_m2.txt').read_text() == 'M2', 'second manager write persisted'

# === Multi-item with: exception inside body closes both ===
mexa = open(root / 'hello.txt')
mexb = open(root / 'hello.txt')
caught = None
try:
    with mexa as _ma, mexb as _mb:
        raise ValueError('multi-item-boom')
except ValueError as e:
    caught = str(e)
assert caught == 'multi-item-boom', 'exception in multi-item with propagates'
assert mexa.closed == True, 'first manager closed on exception'
assert mexb.closed == True, 'second manager closed on exception'

# === Multi-item with: bare items without `as` ===
with open(root / 'hello.txt'), open(root / 'hello.txt'):
    pass


# === `return` inside with calls __exit__ ===
def write_and_return(path):
    with open(path, 'w') as out:
        out.write('via-return')
        return out


returned = write_and_return(root / 'open_with_ret.txt')
assert returned.closed == True, 'file closed when returning from inside with'
assert (root / 'open_with_ret.txt').read_text() == 'via-return', 'write persisted on return path'

# === `break` inside with calls __exit__ ===
break_file = None
for _ in range(1):
    with open(root / 'open_with_break.txt', 'w') as bf:
        break_file = bf
        bf.write('via-break')
        break
assert break_file.closed == True, 'file closed when breaking from inside with'
assert (root / 'open_with_break.txt').read_text() == 'via-break', 'write persisted on break path'

# === `continue` inside with calls __exit__ ===
cont_files = []
cont_paths = [root / 'open_with_cont_0.txt', root / 'open_with_cont_1.txt']
for i in range(2):
    with open(cont_paths[i], 'w') as cf:
        cont_files.append(cf)
        cf.write('iter-' + str(i))
        continue
assert all(f.closed for f in cont_files), 'files closed when continuing from inside with'
assert cont_paths[0].read_text() == 'iter-0', 'first iteration persisted'
assert cont_paths[1].read_text() == 'iter-1', 'second iteration persisted'

# === Direct `__enter__()` / `__exit__()` invocation ===
direct = open(root / 'hello.txt')
entered = direct.__enter__()
assert entered is direct, '__enter__ direct call returns self'
assert direct.__exit__(None, None, None) is None, '__exit__ returns None'
assert direct.closed == True, 'direct __exit__ closes the file'

# === `__enter__` on a closed file raises ===
closed = open(root / 'hello.txt')
closed.close()
err = None
try:
    with closed:
        assert False, 'should not enter body when ctx is closed'
except ValueError as e:
    err = str(e)
assert err == 'I/O operation on closed file.', 'closed file rejected by __enter__'
