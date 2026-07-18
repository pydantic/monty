# === Basic exception catching ===
caught = False
try:
    raise ValueError('test')
except ValueError:
    caught = True
assert caught

# === Exception variable binding ===
msg = None
try:
    raise TypeError('the message')
except TypeError as e:
    msg = repr(e)
# repr(e) returns "TypeError('the message')" - confirms we caught the right exception
assert msg == "TypeError('the message')"

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
assert which == 'type'

# === Bare except catches all ===
caught_bare = False
try:
    raise KeyError('key')
except:
    caught_bare = True
assert caught_bare

# === Else block runs when no exception ===
else_ran = False
try:
    x = 1
except:
    pass
else:
    else_ran = True
assert else_ran

# === Else block does not run when exception occurs ===
else_ran_with_exc = True
try:
    raise ValueError()
except ValueError:
    pass
else:
    else_ran_with_exc = False
assert else_ran_with_exc

# === Finally always runs after try ===
finally_ran = False
try:
    x = 1
finally:
    finally_ran = True
assert finally_ran

# === Finally runs after exception caught ===
finally_after_catch = False
try:
    raise ValueError()
except ValueError:
    pass
finally:
    finally_after_catch = True
assert finally_after_catch

# === Bare raise re-raises current exception ===
caught_reraised = False
try:
    try:
        raise ValueError('original')
    except ValueError:
        raise  # bare raise
except ValueError as e:
    caught_reraised = repr(e) == "ValueError('original')"
assert caught_reraised

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
assert caught_by_base

# === Tuple of exception types ===
caught_tuple = False
try:
    raise TypeError('type')
except (ValueError, TypeError):
    caught_tuple = True
assert caught_tuple


# === Return in try with finally ===
def try_return_finally():
    try:
        return 1
    finally:
        pass


assert try_return_finally() == 1


# === Return in finally overrides try return ===
def finally_return_overrides():
    try:
        return 1
    finally:
        return 2  # type: ignore[returnInFinally]


assert finally_return_overrides() == 2

# === Exception in handler propagates ===
handler_exc_propagated = False
try:
    try:
        raise ValueError()
    except ValueError:
        raise TypeError('from handler')
except TypeError as e:
    handler_exc_propagated = repr(e) == "TypeError('from handler')"
assert handler_exc_propagated


# === Return in finally overrides exception from handler ===
def finally_return_overrides_handler_exc():
    try:
        raise TypeError('Error')
    finally:
        return 'finally wins handler'  # type: ignore


assert finally_return_overrides_handler_exc() == 'finally wins handler'


def finally_return_overrides_handler_exc2():
    try:
        try:
            raise ValueError('inner')
        except ValueError:
            raise TypeError('handler failure')
    finally:
        return 'finally wins handler'  # type: ignore


assert finally_return_overrides_handler_exc2() == 'finally wins handler'


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


assert finally_return_overrides_else_exc() == 'finally wins else'

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
assert e_cleared

# === Unhandled exception propagates ===
unhandled_propagated = False
try:
    try:
        raise KeyError('unhandled')
    except ValueError:
        pass  # KeyError doesn't match, should propagate
except KeyError as e:
    unhandled_propagated = repr(e) == "KeyError('unhandled')"
assert unhandled_propagated

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
assert finally_before_propagate

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
assert finally_exc_wins

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
assert else_exc_propagated

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
assert finally_after_else_exc

# === Exception hierarchy: LookupError ===
# LookupError should catch KeyError
caught_key_by_lookup = False
try:
    raise KeyError('key')
except LookupError:
    caught_key_by_lookup = True
assert caught_key_by_lookup

# LookupError should catch IndexError
caught_index_by_lookup = False
try:
    raise IndexError('index')
except LookupError:
    caught_index_by_lookup = True
assert caught_index_by_lookup

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
assert caught_zero_by_arith

# ArithmeticError should catch OverflowError
caught_overflow_by_arith = False
try:
    raise OverflowError('overflow')
except ArithmeticError:
    caught_overflow_by_arith = True
assert caught_overflow_by_arith

# === Exception hierarchy: RuntimeError ===
# RuntimeError should catch NotImplementedError
caught_notimpl_by_runtime = False
try:
    raise NotImplementedError('not impl')
except RuntimeError:
    caught_notimpl_by_runtime = True
assert caught_notimpl_by_runtime

# RuntimeError should catch RecursionError
caught_recursion_by_runtime = False
try:
    raise RecursionError('recursion')
except RuntimeError:
    caught_recursion_by_runtime = True
assert caught_recursion_by_runtime

# === Exception hierarchy: OSError ===
# OSError should catch TimeoutError (subclass since Python 3.3)
caught_timeout_by_oserror = False
try:
    raise TimeoutError('timed out')
except OSError:
    caught_timeout_by_oserror = True
assert caught_timeout_by_oserror, 'OSError should catch TimeoutError'

# TimeoutError should still be caught specifically before OSError
caught_timeout_specifically = False
try:
    raise TimeoutError('timed out')
except TimeoutError:
    caught_timeout_specifically = True
except OSError:
    pass
assert caught_timeout_specifically, 'TimeoutError handler should match TimeoutError'

try:
    raise TimeoutError('timed out')
except TimeoutError as e:
    assert isinstance(e, TimeoutError), 'exception should be instance of TimeoutError'
    assert isinstance(e, OSError), 'TimeoutError should be instance of OSError'
    assert not isinstance(e, ValueError), 'TimeoutError should not be ValueError'

# === Exception hierarchy in tuple ===
# Tuple containing base class should catch derived
caught_by_tuple_base = False
try:
    raise KeyError('key')
except (ValueError, LookupError):
    caught_by_tuple_base = True
assert caught_by_tuple_base

# === isinstance with exception hierarchy ===
try:
    raise KeyError('key')
except KeyError as e:
    assert isinstance(e, KeyError)
    assert isinstance(e, LookupError)
    assert isinstance(e, Exception)
    assert not isinstance(e, ArithmeticError), 'KeyError should not be ArithmeticError'

try:
    raise ZeroDivisionError('zero')
except ZeroDivisionError as e:
    assert isinstance(e, ZeroDivisionError)
    assert isinstance(e, ArithmeticError)
    assert isinstance(e, Exception)
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
assert multi_no_match_propagated

# === BaseException hierarchy ===
# BaseException should catch all exceptions including Exception subclasses
caught_value_by_base = False
try:
    raise ValueError('value')
except BaseException:
    caught_value_by_base = True
assert caught_value_by_base

caught_key_by_base = False
try:
    raise KeyError('key')
except BaseException:
    caught_key_by_base = True
assert caught_key_by_base

caught_type_by_base = False
try:
    raise TypeError('type')
except BaseException:
    caught_type_by_base = True
assert caught_type_by_base

# BaseException catches KeyboardInterrupt
caught_keyboard_by_base = False
try:
    raise KeyboardInterrupt()
except BaseException:
    caught_keyboard_by_base = True
assert caught_keyboard_by_base

# BaseException catches SystemExit
caught_sysexit_by_base = False
try:
    raise SystemExit()
except BaseException:
    caught_sysexit_by_base = True
assert caught_sysexit_by_base

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
assert caught_value_by_exc

# === isinstance with BaseException ===
try:
    raise ValueError('test')
except ValueError as e:
    assert isinstance(e, BaseException)

try:
    raise KeyboardInterrupt()
except KeyboardInterrupt as e:
    assert isinstance(e, BaseException)
    assert not isinstance(e, Exception), 'KeyboardInterrupt should NOT be instance of Exception'

try:
    raise SystemExit()
except SystemExit as e:
    assert isinstance(e, BaseException)
    assert not isinstance(e, Exception), 'SystemExit should NOT be instance of Exception'

# === Tuple containing BaseException ===
caught_by_tuple_with_base = False
try:
    raise KeyboardInterrupt()
except (ValueError, BaseException):
    caught_by_tuple_with_base = True
assert caught_by_tuple_with_base

# === Nested tuples in except are rejected (CPython parity) ===
# CPython only accepts a single level of tuple in an `except` clause. A nested
# tuple element is not a class inheriting from BaseException, so matching against
# it raises TypeError rather than descending into the sub-tuple. This also bounds
# the matcher: the except type is built at runtime and could otherwise be nested
# arbitrarily deeply, overflowing the host stack inside one bytecode instruction.
#
# The TypeError is raised while evaluating the `except` clause itself, so it
# propagates out of the whole `try` (later `except` clauses of the *same* try are
# not tried against it) and must be caught by an enclosing `try`.
_msg = 'catching classes that do not inherit from BaseException is not allowed'

# Single-element tuple wrapping a tuple.
_rejected = False
try:
    try:
        raise TypeError()
    except ((ValueError,),):
        assert False, 'nested tuple should not be descended into'
except TypeError as exc:
    _rejected = True
    assert str(exc) == _msg, f'unexpected message: {exc}'
assert _rejected

# Tuple of tuples.
_rejected = False
try:
    try:
        raise TypeError()
    except ((ValueError,), (KeyError, TypeError)):
        assert False, 'tuple of tuples should not match'
except TypeError as exc:
    _rejected = True
    assert str(exc) == _msg, f'unexpected message: {exc}'
assert _rejected

# The whole tuple is validated, so an invalid element raises even when an
# earlier element already matched the raised exception.
_rejected = False
try:
    try:
        raise TypeError()
    except (TypeError, (ValueError,)):
        assert False, 'must validate whole tuple, not short-circuit on match'
except TypeError as exc:
    _rejected = True
    assert str(exc) == _msg, f'unexpected message: {exc}'
assert _rejected

# Invalid element appears before a valid match.
_rejected = False
try:
    try:
        raise TypeError()
    except ((ValueError,), TypeError):
        assert False, 'leading nested tuple should be rejected'
except TypeError as exc:
    _rejected = True
    assert str(exc) == _msg, f'unexpected message: {exc}'
assert _rejected

# Deeply nested, runtime-constructed tuple: rejected with TypeError, not a host
# stack overflow and not a RecursionError.
deep = (ValueError,)
for _ in range(1000):
    deep = (deep,)
_rejected = False
try:
    try:
        raise TypeError()
    except deep:
        assert False, 'deeply nested tuple should not be descended into'
except TypeError as exc:
    _rejected = True
    assert str(exc) == _msg, f'unexpected message: {exc}'
assert _rejected

# An empty tuple is valid (never matches): the original exception propagates
# unchanged rather than raising the "catching classes" TypeError.
caught_empty_tuple = False
try:
    try:
        raise TypeError('propagate')
    except ():
        assert False, 'empty tuple never matches'
except TypeError as exc:
    caught_empty_tuple = True
    assert str(exc) == 'propagate', f'unexpected message: {exc}'
assert caught_empty_tuple

# A large flat tuple built at runtime still matches normally.
big_flat = tuple([ValueError] * 5000 + [TypeError])
caught_big_flat = False
try:
    raise TypeError()
except big_flat:
    caught_big_flat = True
assert caught_big_flat

# === Exception state cleared on `return` from inside an except handler ===
# When `return` exits an except clause, the exception is cleared from the
# active-exception state before any surrounding finally runs and before
# control returns to the caller. A bare `raise` inside that finally (or
# in subsequent code in the caller) must therefore see `RuntimeError(
# "No active exception to reraise")` rather than re-raising the exception
# the except clause had just been handling.


# Bare raise inside a try/except inside a finally that runs after
# return-from-except: should be caught as RuntimeError, not ValueError.
def _return_from_except_then_bare_raise_in_finally() -> None:
    try:
        try:
            raise ValueError('original')
        except ValueError:
            return
    finally:
        try:
            raise  # bare reraise — exception should already be cleared
        except ValueError:
            assert False, '`return` from except must clear the exception before finally runs'
        except RuntimeError as exc:
            assert str(exc) == 'No active exception to reraise'


_return_from_except_then_bare_raise_in_finally()


# Return from a doubly-nested except handler should clear EVERY enclosing
# handler's exception state, not just the innermost.
def _return_from_doubly_nested_except() -> None:
    try:
        try:
            try:
                raise ValueError('inner')
            except ValueError:
                raise TypeError('middle')
        except TypeError:
            return
    finally:
        try:
            raise
        except (ValueError, TypeError):
            assert False, "`return` from doubly-nested except must clear every handler's exception state"
        except RuntimeError as exc:
            assert str(exc) == 'No active exception to reraise'


_return_from_doubly_nested_except()


# After a function returns from inside an except clause, the caller's
# active-exception state should NOT contain that function's exception.
def _returns_from_except_no_finally() -> str:
    try:
        raise ValueError('original')
    except ValueError:
        return 'returned'


assert _returns_from_except_no_finally() == 'returned'
try:
    raise  # bare raise in caller — no exception should be active here
except ValueError:
    assert False, "caller should not see inner function's exception as current"
except RuntimeError as exc:
    assert str(exc) == 'No active exception to reraise'


# === Exception state cleared when an exception propagates past handlers ===
# When an exception is raised from inside an except clause and is caught
# by a sibling/outer handler, the inner (abandoned) handler's exception
# must be cleared from the active-exception state — its trailer that
# would normally pop it is dead code (the handler body terminated via
# raise rather than falling through). Without this, a bare `raise` later
# resurrects the abandoned exception instead of producing
# `RuntimeError("No active exception to reraise")`.

# Triple-nested: `raise X` → `raise Y` → `raise Z`, then bare raise outside.
# Each abandoned handler should be cleared.
try:
    try:
        try:
            raise ValueError('first')
        except ValueError:
            raise TypeError('second')
    except TypeError:
        raise KeyError('third')
except KeyError as third:
    assert str(third) == "'third'"

try:
    raise
except RuntimeError as exc:
    assert str(exc) == 'No active exception to reraise'


# Raising from a NESTED try body inside an except clause must NOT clear
# the surrounding handler's exception — the inner raise is caught locally
# and the outer handler is still active. After the inner try-except
# completes, a bare `raise` in the outer handler should re-raise the
# outer's original exception, not produce RuntimeError.
try:
    raise ValueError('outer')
except ValueError as caught:
    try:
        raise KeyError('inner')
    except KeyError:
        pass  # inner caught locally; outer's ValueError should remain active

    # Bare raise here should re-raise the outer's ValueError.
    try:
        raise
    except ValueError as bare:
        assert str(bare) == 'outer'


# Function-call boundary: an exception raised and caught inside a callee
# must not leak active-exception state back to the caller. Probe via bare
# `raise` in the caller after the callee returns.
def _callee_raises_and_handles():
    try:
        raise ValueError('callee internal')
    except ValueError:
        pass


_callee_raises_and_handles()
try:
    raise
except RuntimeError as exc:
    assert str(exc) == 'No active exception to reraise'


# === Return through inner try-finally inside an except keeps outer exception ===
# When `return` is inside a try-finally nested inside an except handler,
# the inner finally must run with the OUTER except's exception still
# active — a bare `raise` inside the inner finally should re-raise the
# outer exception, not produce RuntimeError. The active-exception cleanup
# for the outer except must be deferred until after the inner finally
# completes.

_return_through_inner_finally_log: list[tuple[str, str]] = []


def _return_through_inner_finally() -> str:
    try:
        raise ValueError('outer')
    except ValueError:
        try:
            return 'done'
        finally:
            try:
                raise  # outer ValueError should still be the active exception
            except ValueError as caught:
                _return_through_inner_finally_log.append(('ValueError', str(caught)))
            except RuntimeError as e:
                _return_through_inner_finally_log.append(('RuntimeError', str(e)))
    return 'unreachable'


assert _return_through_inner_finally() == 'done'
assert _return_through_inner_finally_log == [('ValueError', 'outer')], (
    f'expected outer ValueError to remain active inside inner finally, got {_return_through_inner_finally_log!r}'
)


# After the function returns, the caller's active-exception state must
# still be clean — the outer except's exception was cleared on return.
try:
    raise
except RuntimeError as exc:
    assert str(exc) == 'No active exception to reraise'


# === Return through TWO nested finallys inside two excepts ===
# Each finally runs with the textually-active exception still on top
# of `exception_stack` — the inner finally sees the inner except's
# exception, the outer finally sees the outer except's exception.
# This exercises both the per-finally cleanup boundary in
# `emit_return_routing` and the clone-not-pop behavior of bare `raise`
# (without which the inner finally's locally-caught reraise would
# strand the outer except's entry).

_two_finally_log: list[tuple[str, str, str]] = []


def _return_through_two_finallys() -> str:
    try:
        raise ValueError('A')
    except ValueError:
        try:  # outer try has finally
            try:
                raise TypeError('B')
            except TypeError:
                try:  # inner try has finally
                    return 'done'
                finally:
                    try:
                        raise  # outer=A and inner=B both active; reraises B
                    except TypeError as t:
                        _two_finally_log.append(('inner_finally', 'TypeError', str(t)))
                    except ValueError as v:
                        _two_finally_log.append(('inner_finally', 'ValueError', str(v)))
        finally:
            try:
                raise  # only A is still active here; B was bound to the inner except that we exited
            except ValueError as v:
                _two_finally_log.append(('outer_finally', 'ValueError', str(v)))
            except TypeError as t:
                _two_finally_log.append(('outer_finally', 'TypeError', str(t)))
            except RuntimeError as r:
                _two_finally_log.append(('outer_finally', 'RuntimeError', str(r)))
    return 'fallback'


assert _return_through_two_finallys() == 'done'
assert _two_finally_log == [
    ('inner_finally', 'TypeError', 'B'),
    ('outer_finally', 'ValueError', 'A'),
], f'unexpected log {_two_finally_log!r}'
