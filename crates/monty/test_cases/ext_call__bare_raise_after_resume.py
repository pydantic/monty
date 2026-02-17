# call-external
# Test bare raise after external call resumption in except handler
# This tests that current_exception is properly restored after resuming
# Note: bare raise after resumption only works when exception is bound (as e)

caught_reraised = False
try:
    try:
        raise ValueError('original error')
    except ValueError as e:
        # Make an external call, which will cause a suspend/resume
        x = add_ints(1, 2)
        # After resuming, bare raise should still work (exception restored from binding)
        raise
except ValueError as outer_e:
    caught_reraised = repr(outer_e) == "ValueError('original error')"

assert caught_reraised, 'bare raise after external call should re-raise original exception'

# === Nested handler bare raise after resumption ===
outer_nested_reraise = False
try:
    try:
        raise ValueError('outer error')
    except ValueError:
        try:
            raise KeyError('inner error')
        except KeyError:
            _ = add_ints(1, 2)
        raise
except ValueError as reraised:
    outer_nested_reraise = repr(reraised) == "ValueError('outer error')"

assert outer_nested_reraise, 'bare raise in outer handler should re-raise original exception after nested resumption'
