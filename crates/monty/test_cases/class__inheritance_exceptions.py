# Custom exceptions retain their identity when raised, caught, and re-raised.


class AppError(Exception):
    pass


class InputError(AppError):
    pass


error = InputError('invalid input')
assert error.args == ('invalid input',)
assert str(error) == 'invalid input'
assert isinstance(error, InputError)
assert isinstance(error, AppError)
assert isinstance(error, Exception)

try:
    raise error
except InputError as caught:
    assert caught is error
else:
    assert False, 'the custom class must catch its own instances'

try:
    raise error
except AppError as caught:
    assert caught is error
else:
    assert False, 'the parent class must catch a descendant'

try:
    try:
        raise error
    except AppError:
        raise
except Exception as caught:
    assert caught is error
else:
    assert False, 'bare raise must preserve custom exception identity'

try:
    raise AppError
except AppError as caught:
    assert type(caught) is AppError
    assert caught.args == ()
else:
    assert False, 'raising a custom class must construct an instance'

assert repr(AppError()) == 'AppError()'
assert str(AppError()) == ''
assert str(AppError(42)) == '42'
assert repr(AppError(42)) == 'AppError(42)'
assert AppError(1, 2, 3).args == (1, 2, 3)
assert str(AppError(1, 2, 3)) == '(1, 2, 3)'
assert repr(AppError(1, 2, 3)) == 'AppError(1, 2, 3)'

DynamicError = type('DynamicError', (AppError,), {})
assert isinstance(DynamicError('dynamic'), Exception)
try:
    raise DynamicError('dynamic')
except AppError as caught:
    assert type(caught) is DynamicError


class MissingKey(KeyError):
    pass


assert str(MissingKey(42)) == '42'
assert str(MissingKey('key')) == "'key'"


class OtherError(Exception):
    pass


def fail():
    raise error


try:
    fail()
except OtherError:
    assert False, 'siblings must not match'
except (OtherError, AppError) as caught:
    assert caught is error
else:
    assert False, 'function unwinding must preserve custom exceptions'

try:
    try:
        raise error
    except (AppError, 42):
        assert False, 'the whole handler tuple must be validated'
except TypeError:
    pass
else:
    assert False


class Stop(BaseException):
    pass


assert isinstance(Stop(), BaseException)
assert not isinstance(Stop(), Exception)
try:
    raise Stop('stop')
except Exception:
    assert False
except Stop as caught:
    assert str(caught) == 'stop'


class DetailedError(AppError):
    def __init__(self, message, *, code=7):
        self.code = code


class ChildError(DetailedError):
    pass


detailed = ChildError('details', code=9)
assert detailed.code == 9
assert detailed.args == ('details',)
try:
    raise detailed
except DetailedError as caught:
    assert caught is detailed
    assert caught.code == 9


class Manager:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        assert exc_type is InputError
        assert exc is error
        return True


with Manager():
    raise error


class Plain:
    pass


for invalid in [Plain, Plain()]:
    try:
        raise invalid
    except TypeError:
        pass
    else:
        assert False, 'plain classes are not exceptions'
