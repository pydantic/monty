# === Basic exception catching ===
caught = False
try:
    raise ValueError('test')
except ValueError:
    caught = True
assert caught, 'should catch ValueError'

# === Exception variable binding ===
msg = None
try:
    raise TypeError('the message')
except TypeError as e:
    msg = repr(e)
# repr(e) returns "TypeError('the message')" - confirms we caught the right exception
assert msg == "TypeError('the message')", 'should capture exception'

# === Multiple handlers - first match wins ===
which = None
try:
    raise TypeError('type error')
except ValueError:
    which = 'value'
except TypeError:
    which = 'type'
except:
    which = 'bare'
assert which == 'type', 'first matching handler should be used'

# === Bare except catches all ===
caught_bare = False
try:
    raise KeyError('key')
except:
    caught_bare = True
assert caught_bare, 'bare except should catch all'

# === Else block runs when no exception ===
else_ran = False
try:
    x = 1
except:
    pass
else:
    else_ran = True
assert else_ran, 'else should run when no exception'

# === Else block does not run when exception occurs ===
else_ran_with_exc = True
try:
    raise ValueError()
except ValueError:
    pass
else:
    else_ran_with_exc = False
assert else_ran_with_exc, 'else should not run when exception occurs'

# === Finally always runs after try ===
finally_ran = False
try:
    x = 1
finally:
    finally_ran = True
assert finally_ran, 'finally should run after try'

# === Finally runs after exception caught ===
finally_after_catch = False
try:
    raise ValueError()
except ValueError:
    pass
finally:
    finally_after_catch = True
assert finally_after_catch, 'finally should run after exception caught'

# === Bare raise re-raises current exception ===
caught_reraised = False
try:
    try:
        raise ValueError('original')
    except ValueError:
        raise  # bare raise
except ValueError as e:
    caught_reraised = repr(e) == "ValueError('original')"
assert caught_reraised, 'bare raise should re-raise original exception'

# === Nested try/except ===
outer_caught = False
inner_caught = False
try:
    try:
        raise ValueError('inner')
    except ValueError:
        inner_caught = True
        raise TypeError('outer')
except TypeError:
    outer_caught = True
assert inner_caught and outer_caught, 'nested exceptions should work'

# === Exception base class matches all ===
caught_by_base = False
try:
    raise KeyError('key')
except Exception:
    caught_by_base = True
assert caught_by_base, 'Exception should catch all exception types'

# === Tuple of exception types ===
caught_tuple = False
try:
    raise TypeError('type')
except (ValueError, TypeError):
    caught_tuple = True
assert caught_tuple, 'tuple of types should match'


# === Return in try with finally ===
def try_return_finally():
    try:
        return 1
    finally:
        pass


assert try_return_finally() == 1, 'return in try should work with finally'


# === Return in finally overrides try return ===
def finally_return_overrides():
    try:
        return 1
    finally:
        return 2  # type: ignore[returnInFinally]


assert finally_return_overrides() == 2, 'finally return should override try return'

# === Exception in handler propagates ===
handler_exc_propagated = False
try:
    try:
        raise ValueError()
    except ValueError:
        raise TypeError('from handler')
except TypeError as e:
    handler_exc_propagated = repr(e) == "TypeError('from handler')"
assert handler_exc_propagated, 'exception in handler should propagate'


# === Return in finally overrides exception from handler ===
def finally_return_overrides_handler_exc():
    try:
        raise TypeError('Error')
    finally:
        return 'finally wins handler'  # type: ignore


assert finally_return_overrides_handler_exc() == 'finally wins handler', (
    'return in finally should override exception from handler'
)


def finally_return_overrides_handler_exc2():
    try:
        try:
            raise ValueError('inner')
        except ValueError:
            raise TypeError('handler failure')
    finally:
        return 'finally wins handler'  # type: ignore


assert finally_return_overrides_handler_exc2() == 'finally wins handler', (
    'return in finally should override exception from handler'
)


# === Return in finally overrides exception from else ===
def finally_return_overrides_else_exc():
    try:
        try:
            pass
        except ValueError:
            pass
        else:
            raise RuntimeError('else failure')
    finally:
        return 'finally wins else'  # type: ignore


assert finally_return_overrides_else_exc() == 'finally wins else', (
    'return in finally should override exception from else block'
)

# === Exception variable is cleared after handler ===
# After except handler, the exception variable is deleted (Python 3 behavior)
e_cleared = False
try:
    try:
        raise ValueError('test')
    except ValueError as e:
        pass
    # e should be undefined here in Python 3, accessing it raises NameError
    _ = e  # This should raise NameError
except NameError:
    e_cleared = True
assert e_cleared, 'exception variable should be deleted after handler'

# === Unhandled exception propagates ===
unhandled_propagated = False
try:
    try:
        raise KeyError('unhandled')
    except ValueError:
        pass  # KeyError doesn't match, should propagate
except KeyError as e:
    unhandled_propagated = repr(e) == "KeyError('unhandled')"
assert unhandled_propagated, 'unhandled exception should propagate to outer try'

# === Finally runs before unhandled exception propagates ===
finally_before_propagate = False
try:
    try:
        raise KeyError('propagate')
    except ValueError:
        pass
    finally:
        finally_before_propagate = True
except KeyError:
    pass
assert finally_before_propagate, 'finally should run before exception propagates'

# === Exception in finally replaces original exception ===
finally_exc_wins = False
try:
    try:
        raise ValueError('original')
    finally:
        raise TypeError('from finally')
except TypeError as e:
    finally_exc_wins = repr(e) == "TypeError('from finally')"
except ValueError:
    finally_exc_wins = False  # Should not reach here
assert finally_exc_wins, 'exception in finally should replace original'

# === Exception in else propagates ===
else_exc_propagated = False
try:
    try:
        pass  # No exception in try
    except:
        pass
    else:
        raise ValueError('from else')
except ValueError as e:
    else_exc_propagated = repr(e) == "ValueError('from else')"
assert else_exc_propagated, 'exception in else should propagate'

# === Finally runs after exception in else ===
finally_after_else_exc = False
try:
    try:
        pass
    except:
        pass
    else:
        raise ValueError('else error')
    finally:
        finally_after_else_exc = True
except ValueError:
    pass
assert finally_after_else_exc, 'finally should run after exception in else'

# === Exception hierarchy: LookupError ===
# LookupError should catch KeyError
caught_key_by_lookup = False
try:
    raise KeyError('key')
except LookupError:
    caught_key_by_lookup = True
assert caught_key_by_lookup, 'LookupError should catch KeyError'

# LookupError should catch IndexError
caught_index_by_lookup = False
try:
    raise IndexError('index')
except LookupError:
    caught_index_by_lookup = True
assert caught_index_by_lookup, 'LookupError should catch IndexError'

# LookupError should NOT catch ValueError
caught_value_by_lookup = False
try:
    try:
        raise ValueError('value')
    except LookupError:
        caught_value_by_lookup = True
except ValueError:
    pass
assert not caught_value_by_lookup, 'LookupError should NOT catch ValueError'

# === Exception hierarchy: ArithmeticError ===
# ArithmeticError should catch ZeroDivisionError
caught_zero_by_arith = False
try:
    raise ZeroDivisionError('zero')
except ArithmeticError:
    caught_zero_by_arith = True
assert caught_zero_by_arith, 'ArithmeticError should catch ZeroDivisionError'

# ArithmeticError should catch OverflowError
caught_overflow_by_arith = False
try:
    raise OverflowError('overflow')
except ArithmeticError:
    caught_overflow_by_arith = True
assert caught_overflow_by_arith, 'ArithmeticError should catch OverflowError'

# === Exception hierarchy: RuntimeError ===
# RuntimeError should catch NotImplementedError
caught_notimpl_by_runtime = False
try:
    raise NotImplementedError('not impl')
except RuntimeError:
    caught_notimpl_by_runtime = True
assert caught_notimpl_by_runtime, 'RuntimeError should catch NotImplementedError'

# RuntimeError should catch RecursionError
caught_recursion_by_runtime = False
try:
    raise RecursionError('recursion')
except RuntimeError:
    caught_recursion_by_runtime = True
assert caught_recursion_by_runtime, 'RuntimeError should catch RecursionError'

# === Exception hierarchy in tuple ===
# Tuple containing base class should catch derived
caught_by_tuple_base = False
try:
    raise KeyError('key')
except (ValueError, LookupError):
    caught_by_tuple_base = True
assert caught_by_tuple_base, 'tuple with LookupError should catch KeyError'

# === isinstance with exception hierarchy ===
try:
    raise KeyError('key')
except KeyError as e:
    assert isinstance(e, KeyError), 'exception should be instance of KeyError'
    assert isinstance(e, LookupError), 'KeyError should be instance of LookupError'
    assert isinstance(e, Exception), 'KeyError should be instance of Exception'
    assert not isinstance(e, ArithmeticError), 'KeyError should not be ArithmeticError'

try:
    raise ZeroDivisionError('zero')
except ZeroDivisionError as e:
    assert isinstance(e, ZeroDivisionError), 'exception should be instance of ZeroDivisionError'
    assert isinstance(e, ArithmeticError), 'ZeroDivisionError should be instance of ArithmeticError'
    assert isinstance(e, Exception), 'ZeroDivisionError should be instance of Exception'
    assert not isinstance(e, LookupError), 'ZeroDivisionError should not be LookupError'

# === Multiple handlers where none match ===
# Exception should propagate when no handler matches
multi_no_match_propagated = False
try:
    try:
        raise MemoryError('out of memory')
    except ValueError:
        pass
    except TypeError:
        pass
    except KeyError:
        pass
except MemoryError as e:
    multi_no_match_propagated = repr(e) == "MemoryError('out of memory')"
assert multi_no_match_propagated, 'exception should propagate when no handler matches'

# === BaseException hierarchy ===
# BaseException should catch all exceptions including Exception subclasses
caught_value_by_base = False
try:
    raise ValueError('value')
except BaseException:
    caught_value_by_base = True
assert caught_value_by_base, 'BaseException should catch ValueError'

caught_key_by_base = False
try:
    raise KeyError('key')
except BaseException:
    caught_key_by_base = True
assert caught_key_by_base, 'BaseException should catch KeyError'

caught_type_by_base = False
try:
    raise TypeError('type')
except BaseException:
    caught_type_by_base = True
assert caught_type_by_base, 'BaseException should catch TypeError'

# BaseException catches KeyboardInterrupt
caught_keyboard_by_base = False
try:
    raise KeyboardInterrupt()
except BaseException:
    caught_keyboard_by_base = True
assert caught_keyboard_by_base, 'BaseException should catch KeyboardInterrupt'

# BaseException catches SystemExit
caught_sysexit_by_base = False
try:
    raise SystemExit()
except BaseException:
    caught_sysexit_by_base = True
assert caught_sysexit_by_base, 'BaseException should catch SystemExit'

# === Exception does NOT catch BaseException direct subclasses ===
# Exception should NOT catch KeyboardInterrupt
caught_keyboard_by_exc = False
try:
    try:
        raise KeyboardInterrupt()
    except Exception:
        caught_keyboard_by_exc = True
except BaseException:
    pass
assert not caught_keyboard_by_exc, 'Exception should NOT catch KeyboardInterrupt'

# Exception should NOT catch SystemExit
caught_sysexit_by_exc = False
try:
    try:
        raise SystemExit()
    except Exception:
        caught_sysexit_by_exc = True
except BaseException:
    pass
assert not caught_sysexit_by_exc, 'Exception should NOT catch SystemExit'

# But Exception SHOULD catch regular exceptions
caught_value_by_exc = False
try:
    raise ValueError('test')
except Exception:
    caught_value_by_exc = True
assert caught_value_by_exc, 'Exception should catch ValueError'

# === isinstance with BaseException ===
try:
    raise ValueError('test')
except ValueError as e:
    assert isinstance(e, BaseException), 'ValueError should be instance of BaseException'

try:
    raise KeyboardInterrupt()
except KeyboardInterrupt as e:
    assert isinstance(e, BaseException), 'KeyboardInterrupt should be instance of BaseException'
    assert not isinstance(e, Exception), 'KeyboardInterrupt should NOT be instance of Exception'

try:
    raise SystemExit()
except SystemExit as e:
    assert isinstance(e, BaseException), 'SystemExit should be instance of BaseException'
    assert not isinstance(e, Exception), 'SystemExit should NOT be instance of Exception'

# === Tuple containing BaseException ===
caught_by_tuple_with_base = False
try:
    raise KeyboardInterrupt()
except (ValueError, BaseException):
    caught_by_tuple_with_base = True
assert caught_by_tuple_with_base, 'tuple with BaseException should catch KeyboardInterrupt'
