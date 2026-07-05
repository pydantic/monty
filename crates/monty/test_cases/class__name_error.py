# Methods skip the class scope: a bare member name is NOT the class attribute,
# so with no matching global it raises NameError. The traceback (frames, line
# numbers, caret markers) matches CPython exactly.
#
# (CPython appends a "Did you mean: 'self.timeout'?" suggestion to the final
# line, which Monty does not implement; the traceback harness strips that
# CPython-only suffix — see scripts/run_traceback.py.)


class Settings:
    timeout = 30

    def describe(self):
        return timeout  # not Settings.timeout; no global -> NameError


Settings().describe()
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__name_error.py", line 17, in <module>
    Settings().describe()
    ~~~~~~~~~~~~~~~~~~~~~
  File "class__name_error.py", line 14, in describe
    return timeout  # not Settings.timeout; no global -> NameError
           ~~~~~~~
NameError: name 'timeout' is not defined
"""
