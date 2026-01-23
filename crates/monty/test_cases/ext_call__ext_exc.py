# call-external
# === External function exceptions ===
# Tests for exceptions raised by external functions

# === Basic exception propagation ===

# External function raising ValueError
caught_value_error = False
try:
    result = raise_error('ValueError', 'test error')
    assert False, 'should not reach here'
except ValueError:
    caught_value_error = True
assert caught_value_error, 'ValueError was caught'

# External function raising TypeError
caught_type_error = False
try:
    result = raise_error('TypeError', 'type error message')
    assert False, 'should not reach here'
except TypeError:
    caught_type_error = True
assert caught_type_error, 'TypeError was caught'

# External function raising KeyError
caught_key_error = False
try:
    result = raise_error('KeyError', 'missing key')
    assert False, 'should not reach here'
except KeyError:
    caught_key_error = True
assert caught_key_error, 'KeyError was caught'

# External function raising RuntimeError
caught_runtime_error = False
try:
    result = raise_error('RuntimeError', 'runtime error')
    assert False, 'should not reach here'
except RuntimeError:
    caught_runtime_error = True
assert caught_runtime_error, 'RuntimeError was caught'

# === Exception not caught by wrong handler ===

# ValueError not caught by TypeError handler
caught_outer = False
try:
    try:
        raise_error('ValueError', 'inner error')
    except TypeError:
        assert False, 'TypeError should not catch ValueError'
except ValueError:
    caught_outer = True
assert caught_outer, 'ValueError caught by outer handler'

# === Exception in expression with multiple ext calls ===

# First ext call raises, second should not be called
try:
    x = raise_error('ValueError', 'first') + add_ints(1, 2)
    assert False, 'should not reach here'
except ValueError:
    pass  # Expected

# === External exception in try body with finally ===

finally_ran = False
try:
    raise_error('ValueError', 'in try')
except ValueError:
    pass  # Caught
finally:
    finally_ran = True
assert finally_ran, 'finally ran after external exception caught'

# External exception propagating through finally
outer_caught = False
finally_ran2 = False
try:
    try:
        raise_error('KeyError', 'will propagate')
    except ValueError:
        assert False, 'ValueError should not catch KeyError'
    finally:
        finally_ran2 = True
except KeyError:
    outer_caught = True
assert finally_ran2, 'finally ran before exception propagated'
assert outer_caught, 'exception propagated after finally'

# === Mix of normal returns and exceptions ===

# Normal return, then exception
value1 = add_ints(10, 20)
assert value1 == 30, 'first ext call returned normally'
try:
    raise_error('ValueError', 'after success')
    assert False, 'should not reach here'
except ValueError:
    pass  # Expected

# Exception, then normal return (after catching)
caught_exc = False
try:
    raise_error('TypeError', 'will be caught')
except TypeError:
    caught_exc = True
value2 = add_ints(5, 5)
assert caught_exc, 'exception was caught'
assert value2 == 10, 'ext call after caught exception returned normally'

# === Exception in except handler from external function ===

outer_catch = False
try:
    try:
        raise ValueError('inner')
    except ValueError:
        raise_error('TypeError', 'from handler')
except TypeError:
    outer_catch = True
assert outer_catch, 'exception from handler caught by outer'

# === Exception in else block from external function ===

else_exc_caught = False
try:
    try:
        pass  # No exception
    except:
        assert False, 'should not reach except'
    else:
        raise_error('RuntimeError', 'from else')
except RuntimeError:
    else_exc_caught = True
assert else_exc_caught, 'exception from else block caught'

# === Exception in finally block ===

# Note: exception in finally replaces any pending exception
finally_exc_caught = False
try:
    try:
        pass
    finally:
        raise_error('ValueError', 'from finally')
except ValueError:
    finally_exc_caught = True
assert finally_exc_caught, 'exception from finally caught'

# === Nested try blocks with external exceptions ===

inner_handled = False
outer_handled = False
finally_count = 0
try:
    try:
        raise_error('ValueError', 'inner error')
    except ValueError:
        inner_handled = True
        raise_error('TypeError', 'from inner handler')
    finally:
        finally_count += 1
except TypeError:
    outer_handled = True
finally:
    finally_count += 1

assert inner_handled, 'inner exception was handled'
assert outer_handled, 'exception from inner handler was caught by outer'
assert finally_count == 2, 'both finally blocks ran'
