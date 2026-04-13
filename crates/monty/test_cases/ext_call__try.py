# call-external
# === External calls in try blocks ===

# Basic external call in try body
result = None
try:
    result = add_ints(10, 20)
except:
    result = -1
assert result == 30, 'ext call in try body'

# Multiple external calls in try body
try:
    a = add_ints(1, 2)
    b = add_ints(3, 4)
    c = add_ints(a, b)
except:
    c = -1
assert c == 10, 'multiple ext calls in try body'

# Nested external calls in try body
try:
    nested = add_ints(add_ints(1, 2), add_ints(3, 4))
except:
    nested = -1
assert nested == 10, 'nested ext calls in try body'

# === External calls in except blocks ===

# External call in except handler
handler_result = None
try:
    raise ValueError('error')
except ValueError:
    handler_result = add_ints(100, 200)
assert handler_result == 300, 'ext call in except handler'

# Multiple external calls in except handler
try:
    raise TypeError('error')
except TypeError:
    x = add_ints(5, 5)
    y = add_ints(10, 10)
    handler_sum = add_ints(x, y)
assert handler_sum == 30, 'multiple ext calls in except handler'

# External call with exception variable
exc_with_ext = None
try:
    raise ValueError('test')
except ValueError as e:
    prefix = concat_strings('caught: ', repr(e))
    exc_with_ext = prefix
assert exc_with_ext == "caught: ValueError('test')", 'ext call with exception variable'

# === External calls in else blocks ===

# External call in else block
else_result = None
try:
    x = 1  # No exception
except:
    else_result = -1
else:
    else_result = add_ints(50, 50)
assert else_result == 100, 'ext call in else block'

# Multiple external calls in else block
try:
    pass
except:
    else_multi = -1
else:
    p = add_ints(1, 2)
    q = add_ints(3, 4)
    else_multi = add_ints(p, q)
assert else_multi == 10, 'multiple ext calls in else block'

# === External calls in finally blocks ===

# External call in finally block
finally_result = None
try:
    x = 1
finally:
    finally_result = add_ints(25, 75)
assert finally_result == 100, 'ext call in finally block'

# Finally with external call after exception caught
finally_after_exc = None
try:
    raise ValueError('error')
except ValueError:
    pass
finally:
    finally_after_exc = add_ints(1, 99)
assert finally_after_exc == 100, 'ext call in finally after caught exception'

# Multiple external calls in finally
try:
    pass
finally:
    f1 = add_ints(10, 20)
    f2 = add_ints(30, 40)
    finally_multi = add_ints(f1, f2)
assert finally_multi == 100, 'multiple ext calls in finally block'

# === External calls across multiple phases ===

# External calls in try, except, and finally
all_phases = []
try:
    all_phases.append(add_ints(1, 0))  # 1
    raise ValueError('error')
except ValueError:
    all_phases.append(add_ints(2, 0))  # 2
finally:
    all_phases.append(add_ints(3, 0))  # 3
assert all_phases == [1, 2, 3], 'ext calls in try, except, and finally'

# External calls in try, else, and finally (no exception)
no_exc_phases = []
try:
    no_exc_phases.append(add_ints(10, 0))  # 10
except:
    no_exc_phases.append(-1)
else:
    no_exc_phases.append(add_ints(20, 0))  # 20
finally:
    no_exc_phases.append(add_ints(30, 0))  # 30
assert no_exc_phases == [10, 20, 30], 'ext calls in try, else, and finally'

# === External calls in nested try blocks ===

# Nested try with external calls at each level
outer_val = None
inner_val = None
try:
    outer_val = add_ints(100, 0)
    try:
        inner_val = add_ints(200, 0)
        raise ValueError('inner')
    except ValueError:
        inner_val = add_ints(inner_val, 50)
except:
    outer_val = -1
assert outer_val == 100, 'ext call in outer try'
assert inner_val == 250, 'ext call in inner try and handler'

# === External calls in exception type expression ===
# (Exception type is evaluated at handler matching time)

# External call producing value used after try
post_try = None
try:
    pre = add_ints(5, 5)
except:
    pre = -1
post_try = add_ints(pre, 10)
assert post_try == 20, 'ext call result used after try block'

# === External call in finally with unhandled exception ===
# Finally should still run even when exception propagates
finally_with_propagate = None
try:
    try:
        finally_with_propagate = add_ints(0, 0)  # Initialize
        raise KeyError('unhandled')
    except ValueError:
        pass  # Won't catch KeyError
    finally:
        finally_with_propagate = add_ints(42, 0)  # Should still run
except KeyError:
    pass  # Catch propagated exception
assert finally_with_propagate == 42, 'ext call in finally should run even with unhandled exception'

# === External call in except handler that then raises ===
handler_before_raise = None
try:
    try:
        raise ValueError('original')
    except ValueError:
        handler_before_raise = add_ints(10, 0)  # External call before raising
        raise TypeError('from handler')
except TypeError:
    pass
assert handler_before_raise == 10, 'ext call in handler before raising'

# === External call in else block that then raises ===
else_before_raise = None
try:
    try:
        pass  # No exception
    except:
        pass
    else:
        else_before_raise = add_ints(20, 0)  # External call before raising
        raise ValueError('from else')
except ValueError:
    pass
assert else_before_raise == 20, 'ext call in else before raising'

# === External call preserves state across try/except ===
state_before = add_ints(1000, 0)
state_after = None
try:
    state_after = add_ints(state_before, 1)
    raise ValueError('test')
except ValueError:
    state_after = add_ints(state_after, 10)
finally:
    state_after = add_ints(state_after, 100)
assert state_after == 1111, 'state preserved across try/except with ext calls'

# === Multiple except handlers with external calls ===
which_handler = None
try:
    raise TypeError('test')
except ValueError:
    which_handler = add_ints(1, 0)
except TypeError:
    which_handler = add_ints(2, 0)
except KeyError:
    which_handler = add_ints(3, 0)
assert which_handler == 2, 'ext call in correct handler with multiple handlers'

# === External call in finally with pending exception (after handler raises) ===
finally_after_handler_raise = None
try:
    try:
        raise ValueError('original')
    except ValueError:
        finally_after_handler_raise = add_ints(10, 0)  # External call before raising
        raise TypeError('from handler')
    finally:
        # This external call should work even though there's a pending exception
        finally_after_handler_raise = add_ints(finally_after_handler_raise, 5)
except TypeError:
    pass
assert finally_after_handler_raise == 15, 'ext call in finally with pending exception from handler'

# === External call in finally with pending exception (no matching handler) ===
finally_with_pending_exc = None
try:
    try:
        finally_with_pending_exc = add_ints(0, 0)
        raise KeyError('no handler')
    except ValueError:
        pass  # Won't catch KeyError
    finally:
        # This external call should work even though KeyError is pending
        finally_with_pending_exc = add_ints(100, 0)
except KeyError:
    pass  # Catch it here
assert finally_with_pending_exc == 100, 'ext call in finally with unhandled exception pending'

# === External call in finally with return (uses simple values) ===
# Note: External calls in user-defined functions are not supported,
# so we test pending return with built-in operations only
finally_return_result = None
try:
    finally_return_result = 'in_try'
finally:
    pass  # finally runs but doesn't override
assert finally_return_result == 'in_try', 'finally runs with pending value'

# === Multiple external calls in finally with pending exception ===
multi_finally = None
try:
    try:
        raise ValueError('test')
    except TypeError:
        pass  # Won't match
    finally:
        a = add_ints(1, 2)
        b = add_ints(3, 4)
        multi_finally = add_ints(a, b)
except ValueError:
    pass
assert multi_finally == 10, 'multiple ext calls in finally with pending exception'
