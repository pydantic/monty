"""Raise and catch custom exceptions through custom and native parents."""


class ValidationError(ValueError):
    def __init__(self, message):
        self.message = message


class MissingField(ValidationError):
    pass


error = MissingField('name is required')
assert error.args == ('name is required',)
assert str(error) == 'name is required'
assert isinstance(error, ValidationError)
assert isinstance(error, ValueError)
assert isinstance(error, Exception)
assert issubclass(MissingField, ValueError)

try:
    raise error
except ValidationError as caught:
    assert caught is error
    print('Caught through a custom parent:', str(caught))
    try:
        raise
    except ValueError as reraised:
        assert reraised is error
        print('Caught through a native parent:', repr(reraised))

# Raising a class constructs an instance with no arguments.
class SimpleError(Exception):
    pass


try:
    raise SimpleError
except SimpleError as caught:
    assert type(caught) is SimpleError
    assert caught.args == ()
    print('Raising a class creates:', caught.__class__.__name__)
