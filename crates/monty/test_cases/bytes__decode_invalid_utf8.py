# Test that bytes.decode raises UnicodeDecodeError for invalid UTF-8
# UnicodeDecodeError is a subclass of ValueError, so it should be caught by both

# Test it raises UnicodeDecodeError
raised_decode_error = False
try:
    b'\xff'.decode()
except UnicodeDecodeError:
    raised_decode_error = True
assert raised_decode_error, 'should raise UnicodeDecodeError for invalid UTF-8'

# Test it can be caught by ValueError (since UnicodeDecodeError is a subclass)
caught_by_value_error = False
try:
    b'\x80\x81'.decode()
except ValueError:
    caught_by_value_error = True
assert caught_by_value_error, 'UnicodeDecodeError should be caught by except ValueError'
