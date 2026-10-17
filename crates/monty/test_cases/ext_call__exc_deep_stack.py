# /// monty
# call-external = true
# ///
def level4():
    x = 1
    raise_error('RuntimeError', 'deep error')


def level3():
    level4()


def level2():
    level3()


def level1():
    level2()


level1()
"""
TRACEBACK:
Traceback (most recent call last):
  File "ext_call__exc_deep_stack.py", line 21, in <module>
    level1()
    ~~~~~~~~
  File "ext_call__exc_deep_stack.py", line 18, in level1
    level2()
    ~~~~~~~~
  File "ext_call__exc_deep_stack.py", line 14, in level2
    level3()
    ~~~~~~~~
  File "ext_call__exc_deep_stack.py", line 10, in level3
    level4()
    ~~~~~~~~
  File "ext_call__exc_deep_stack.py", line 6, in level4
    raise_error('RuntimeError', 'deep error')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
RuntimeError: deep error
"""
