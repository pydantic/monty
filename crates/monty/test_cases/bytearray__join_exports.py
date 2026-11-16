def mutate(operation):
    global alias
    if operation == 'append':
        alias.append(120)
    elif operation == 'extend':
        alias.extend(b'x')
    elif operation == 'insert':
        alias.insert(0, 120)
    elif operation == 'clear':
        alias.clear()
    elif operation == 'pop':
        alias.pop()
    elif operation == 'remove':
        alias.remove(44)
    elif operation == 'resize':
        alias.resize(len(alias) + 1)
    elif operation == 'iadd':
        alias += b'x'
    elif operation == 'repeat_zero':
        alias *= 0
    elif operation == 'repeat_twice':
        alias *= 2
    elif operation == 'grow_slice':
        alias[:] = b'longer'
    elif operation == 'shrink_slice':
        alias[:] = b''
    elif operation == 'delete_extended':
        alias[::2] = b''
    elif operation == 'delete_empty_extended':
        alias[1:1:2] = b''
    elif operation == 'same_length':
        alias[:] = b';' * len(alias)
    elif operation == 'same_extended':
        alias[::2] = b';' * len(alias[::2])
    elif operation == 'reverse':
        alias.reverse()
    elif operation == 'extend_empty':
        alias.extend(b'')
    elif operation == 'iadd_empty':
        alias += b''
    elif operation == 'resize_same':
        alias.resize(len(alias))
    elif operation == 'repeat_once':
        alias *= 1
    else:
        assert operation == 'copy'
        copied = alias.copy()
        copied.append(120)


class MutatingItems:
    def __init__(self, operation, phase):
        self.operation = operation
        self.phase = phase
        self.index = 0

    def __iter__(self):
        if self.phase == 'iter':
            mutate(self.operation)
        return self

    def __next__(self):
        if self.index == 0 and self.phase == 'next':
            mutate(self.operation)
        if self.index == 2:
            raise StopIteration
        result = [b'a', b'b'][self.index]
        self.index += 1
        return result


always_blocked = [
    'append',
    'extend',
    'insert',
    'resize',
    'iadd',
    'grow_slice',
    'delete_extended',
    'delete_empty_extended',
]
nonempty_blocked = ['clear', 'pop', 'remove', 'repeat_zero', 'repeat_twice', 'shrink_slice']
allowed = ['same_length', 'reverse', 'extend_empty', 'iadd_empty', 'resize_same', 'repeat_once', 'copy']
for initial in [b'', b',']:
    operations = always_blocked + nonempty_blocked + allowed
    if initial:
        operations.append('same_extended')
    for operation in operations:
        for phase in ['iter', 'next']:
            separator = bytearray(initial)
            alias = separator
            blocked = operation in always_blocked or (bool(initial) and operation in nonempty_blocked)
            try:
                result = separator.join(MutatingItems(operation, phase))
            except BufferError as exc:
                assert blocked, operation
                assert str(exc) == 'Existing exports of data: object cannot be re-sized'
                assert separator == initial
            except IndexError:
                assert not initial and operation == 'pop'
            except ValueError:
                assert not initial and operation == 'remove'
            else:
                assert not blocked, operation
                assert result == b'a' + bytes(separator) + b'b'
            # Every exit must release the export, including iterator exceptions.
            assert alias is separator
            separator.append(120)
            assert separator[-1] == 120


class FailingItems:
    def __iter__(self):
        raise ValueError('iterator failed')


class NestedItems:
    def __iter__(self):
        assert separator.join([b'x', b'y']) == b'x,y'
        try:
            separator.join(FailingItems())
        except ValueError:
            pass
        else:
            assert False
        # Finishing either nested join must leave the outer export active.
        try:
            separator.append(120)
        except BufferError:
            pass
        else:
            assert False
        separator[0] = 59
        return iter([b'a', b'b'])


separator = bytearray(b',')
assert separator.join(NestedItems()) == b'a;b'
separator.append(120)
assert separator == b';x'

for items in [FailingItems(), [b'a', 1], None]:
    separator = bytearray(b',')
    try:
        separator.join(items)
    except (TypeError, ValueError):
        pass
    else:
        assert False
    separator.clear()
    separator.extend(b'new')
    assert separator == b'new'
