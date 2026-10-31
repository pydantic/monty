# === raise ... from sets __cause__ and __suppress_context__ ===
try:
    raise ValueError('a') from KeyError('k')
except ValueError as e:
    assert repr(e.__cause__) == "KeyError('k')"
    assert e.__context__ is None
    assert e.__suppress_context__ is True

# `from None` suppresses the context without a cause
try:
    try:
        raise KeyError('k')
    except KeyError:
        raise ValueError('b') from None
except ValueError as e:
    assert e.__cause__ is None
    assert repr(e.__context__) == "KeyError('k')"
    assert e.__suppress_context__ is True

# a bare class as the cause is instantiated
try:
    raise ValueError('c') from KeyError
except ValueError as e:
    assert repr(e.__cause__) == 'KeyError()'
    assert e.__suppress_context__ is True

# a bare class as the exception is instantiated too
try:
    raise ValueError from None
except ValueError as e:
    assert e.args == ()
    assert e.__cause__ is None
    assert e.__suppress_context__ is True

# the cause keeps its identity
_cause = KeyError('k')
try:
    raise ValueError('d') from _cause
except ValueError as e:
    assert e.__cause__ is _cause

# so does the raised instance
_err = ValueError('e')
try:
    raise _err from _cause
except ValueError as e:
    assert e is _err
    assert _err.__cause__ is _cause

# raising the same instance again replaces its cause
try:
    raise _err from KeyError('other')
except ValueError as e:
    assert repr(e.__cause__) == "KeyError('other')"
try:
    raise _err from None
except ValueError as e:
    assert e.__cause__ is None
    assert e.__suppress_context__ is True

# === __context__ is the exception being handled ===
try:
    try:
        raise KeyError('k')
    except KeyError:
        raise ValueError('f')
except ValueError as e:
    assert e.__cause__ is None
    assert repr(e.__context__) == "KeyError('k')"
    assert e.__suppress_context__ is False

# `from` records both
try:
    try:
        raise KeyError('k')
    except KeyError as k:
        raise ValueError('g') from k
except ValueError as e:
    assert repr(e.__cause__) == "KeyError('k')"
    assert e.__context__ is e.__cause__
    assert e.__suppress_context__ is True

# exceptions raised by builtins inside a handler get a context too
try:
    try:
        raise KeyError('k')
    except KeyError:
        1 / 0
except ZeroDivisionError as e:
    assert repr(e.__context__) == "KeyError('k')"

# the innermost handled exception is the context
try:
    try:
        raise KeyError('k')
    except KeyError:
        try:
            raise TypeError('t')
        except TypeError:
            raise ValueError('h')
except ValueError as e:
    assert repr(e.__context__) == "TypeError('t')"
    assert repr(e.__context__.__context__) == "KeyError('k')"

# a finished handler is no longer context
try:
    try:
        raise KeyError('k')
    except KeyError:
        pass
    raise ValueError('i')
except ValueError as e:
    assert e.__context__ is None

# a bare re-raise of the handled exception does not chain it to itself
try:
    try:
        raise KeyError('k')
    except KeyError:
        raise
except KeyError as e:
    assert e.__context__ is None
try:
    try:
        raise KeyError('k')
    except KeyError as k:
        raise k
except KeyError as e:
    assert e.__context__ is None

# a context link that would loop is cut, as CPython cuts it
_e1 = ValueError('e1')
_e2 = ValueError('e2')
try:
    try:
        raise _e1
    except ValueError:
        try:
            raise _e2
        except ValueError:
            pass
except ValueError:
    pass
assert _e2.__context__ is _e1
try:
    try:
        raise _e2
    except ValueError:
        raise _e1
except ValueError as e:
    assert e is _e1
    assert _e1.__context__ is _e2
    assert _e2.__context__ is None


# context is attached across function calls
def _fail():
    raise TypeError('inner')


try:
    try:
        raise KeyError('k')
    except KeyError:
        _fail()
except TypeError as e:
    assert repr(e.__context__) == "KeyError('k')"

# === a fresh exception has an empty chain ===
_fresh = ValueError('x')
assert _fresh.__cause__ is None
assert _fresh.__context__ is None
assert _fresh.__suppress_context__ is False

# === the cause and exception are validated in CPython's order ===
for _call, _msg in [
    (lambda: exec("raise ValueError('x') from 5"), 'exception causes must derive from BaseException'),
    (lambda: exec("raise ValueError('x') from 'y'"), 'exception causes must derive from BaseException'),
    (lambda: exec("raise ValueError('x') from int"), 'exception causes must derive from BaseException'),
    (lambda: exec("raise 5 from ValueError('x')"), 'exceptions must derive from BaseException'),
]:
    try:
        _call()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == _msg
        assert e.__cause__ is None
        assert e.__suppress_context__ is False
