# call-external
# Test external call with exception variable

exc_with_ext = None
try:
    raise ValueError('test')
except ValueError as e:
    prefix = concat_strings('caught: ', repr(e))
    exc_with_ext = prefix
assert exc_with_ext == "caught: ValueError('test')", 'ext call with exception variable'
