# /// monty
# call-external = true
# ///
class Bad:
    def __iter__(self):
        return self

    def __next__(self):
        add_ints(1, 2)
        raise ValueError('bad item')


def consume():
    return len(list(Bad()))


consume()
"""
TRACEBACK:
Traceback (most recent call last):
  File "ext_call__native_continuation_tb_list.py", line 17, in <module>
    consume()
    ~~~~~~~~~
  File "ext_call__native_continuation_tb_list.py", line 14, in consume
    return len(list(Bad()))
               ~~~~~~~~~~~
  File "ext_call__native_continuation_tb_list.py", line 10, in __next__
    raise ValueError('bad item')
ValueError: bad item
"""
