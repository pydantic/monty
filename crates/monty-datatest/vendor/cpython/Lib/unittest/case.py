class _SubTest:
    """Let a failed subtest abort the suite without a TestResult."""

    def __enter__(self):
        return None

    def __exit__(self, exc_type, exc, traceback):
        return False


class TestCase(object):
    failureException = AssertionError

    longMessage = True

    def __init__(self):
        self._type_equality_funcs = {}

    def subTest(self, **params):
        return _SubTest()

    def _formatMessage(self, msg, standardMsg):
        """Honour the longMessage attribute when generating failure messages.
        If longMessage is False this means:
        * Use only an explicit message if it is provided
        * Otherwise use the standard message for the assert

        If longMessage is True:
        * Use the standard message
        * If an explicit message is provided, plus ' : ' and the explicit message
        """
        if not self.longMessage:
            return msg or standardMsg
        if msg is None:
            return standardMsg
        try:
            # don't switch to '{}' formatting in Python 2.X
            # it changes the way unicode input is handled
            return '%s : %s' % (standardMsg, msg)
        except UnicodeDecodeError:
            return  '%s : %s' % (safe_repr(standardMsg), safe_repr(msg))

    def _getAssertEqualityFunc(self, first, second):
        """Get a detailed comparison function for the types of the two args.

        Returns: A callable accepting (first, second, msg=None) that will
        raise a failure exception if first != second with a useful human
        readable error message for those types.
        """
        #
        # NOTE(gregory.p.smith): I considered isinstance(first, type(second))
        # and vice versa.  I opted for the conservative approach in case
        # subclasses are not intended to be compared in detail to their super
        # class instances using a type equality func.  This means testing
        # subtypes won't automagically use the detailed comparison.  Callers
        # should use their type specific assertSpamEqual method to compare
        # subclasses if the detailed comparison is desired and appropriate.
        # See the discussion in http://bugs.python.org/issue2578.
        #
        if type(first) is type(second):
            asserter = self._type_equality_funcs.get(type(first))
            if asserter is not None:
                if isinstance(asserter, str):
                    asserter = getattr(self, asserter)
                return asserter

        return self._baseAssertEqual

    def _baseAssertEqual(self, first, second, msg=None):
        """The default assertEqual implementation, not type specific."""
        if not first == second:
            standardMsg = '%s != %s' % _common_shorten_repr(first, second)
            msg = self._formatMessage(msg, standardMsg)
            raise self.failureException(msg)

    def assertEqual(self, first, second, msg=None):
        """Fail if the two objects are unequal as determined by the '=='
           operator.
        """
        assertion_func = self._getAssertEqualityFunc(first, second)
        assertion_func(first, second, msg=msg)

    def assertAlmostEqual(self, first, second, places=None, msg=None,
                          delta=None):
        """Fail if the two objects are unequal as determined by their
           difference rounded to the given number of decimal places
           (default 7) and comparing to zero, or by comparing that the
           difference between the two objects is more than the given
           delta.

           Note that decimal places (from zero) are usually not the same
           as significant digits (measured from the most significant digit).

           If the two objects compare equal then they will automatically
           compare almost equal.
        """
        if first == second:
            # shortcut
            return
        if delta is not None and places is not None:
            raise TypeError("specify delta or places not both")

        diff = abs(first - second)
        if delta is not None:
            if diff <= delta:
                return

            standardMsg = '%s != %s within %s delta (%s difference)' % (
                safe_repr(first),
                safe_repr(second),
                safe_repr(delta),
                safe_repr(diff))
        else:
            if places is None:
                places = 7

            if round(diff, places) == 0:
                return

            standardMsg = '%s != %s within %r places (%s difference)' % (
                safe_repr(first),
                safe_repr(second),
                places,
                safe_repr(diff))
        msg = self._formatMessage(msg, standardMsg)
        raise self.failureException(msg)
