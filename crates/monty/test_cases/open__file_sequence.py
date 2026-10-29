# /// monty
# mount-fs = true
# ///
text_path = root / 'sequence.txt'
binary_path = root / 'sequence.bin'
empty_path = root / 'sequence_empty.txt'
text_path.write_bytes(b'first\r\n\rsecond\nlast')
binary_path.write_bytes(b'first\r\n\rsecond\nlast')
empty_path.write_bytes(b'')
newline_cases = (
    (None, ['first\n', '\n', 'second\n', 'last']),
    ('', ['first\r\n', '\r', 'second\n', 'last']),
    ('\n', ['first\r\n', '\rsecond\n', 'last']),
    ('\r', ['first\r', '\n\r', 'second\nlast']),
    ('\r\n', ['first\r\n', '\rsecond\nlast']),
)


def sequence_type(constructor, evaluated):
    evaluated.append('constructor')
    return constructor


def argument(file, evaluated):
    evaluated.append('argument')
    return file


for constructor in (list, tuple):
    for newline, lines in newline_cases:
        with open(text_path, encoding='utf-8', newline=newline) as file:
            result = constructor(file)
            assert type(result) is constructor
            assert result == constructor(lines)
            assert constructor(file) == constructor()
            assert file.read() == ''

    with open(binary_path, 'rb') as file:
        assert constructor(file) == constructor([b'first\r\n', b'\rsecond\n', b'last'])
    for mode in ('r', 'rb'):
        with open(empty_path, mode, encoding=None) as file:
            assert constructor(file) == constructor()

    unicode_path = root / 'sequence_unicode.txt'
    unicode_path.write_bytes('α\n\nβ'.encode())
    with open(unicode_path, encoding='utf-8') as file:
        assert constructor(file) == constructor(['α\n', '\n', 'β'])

    with open(text_path, encoding='utf-8') as file:
        assert file.read(2) == 'fi'
        assert constructor(file) == constructor(['rst\n', '\n', 'second\n', 'last'])
        file.seek(0)
        assert constructor(file) == constructor(['first\n', '\n', 'second\n', 'last'])
    with open(text_path, encoding='utf-8') as file:
        assert next(file) == 'first\n'
        assert constructor(file) == constructor(['\n', 'second\n', 'last'])
    with open(text_path, encoding='utf-8') as file:
        assert [line for line in file] == ['first\n', '\n', 'second\n', 'last']

    evaluated = []
    with open(text_path, encoding='utf-8') as file:
        assert sequence_type(constructor, evaluated)(argument(file, evaluated)) == constructor(
            ['first\n', '\n', 'second\n', 'last']
        )
        assert evaluated == ['constructor', 'argument']

    with open(text_path, encoding='utf-8') as file:
        alias = constructor
        keywords = {}
        assert alias(*[file], **keywords) == constructor(['first\n', '\n', 'second\n', 'last'])

    with open(text_path, encoding='utf-8') as file:
        try:
            constructor(file, None)
            assert False, 'expected TypeError'
        except TypeError:
            pass
        try:
            constructor(iterable=file)
            assert False, 'expected TypeError'
        except TypeError:
            pass
        try:
            constructor(file, unexpected=True)
            assert False, 'expected TypeError'
        except TypeError:
            pass
        assert file.tell() == 0
        assert constructor(file) == constructor(['first\n', '\n', 'second\n', 'last'])

    with open(text_path, encoding='utf-8') as file:
        pass
    try:
        constructor(file)
        assert False, 'expected ValueError'
    except ValueError:
        pass
    with open(root / 'sequence_write.txt', 'w', encoding='utf-8') as file:
        try:
            constructor(file)
            assert False, 'expected OSError'
        except OSError:
            pass

    marker = []
    result = constructor([marker])
    assert result[0] is marker
    assert constructor() == constructor([])
