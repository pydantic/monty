def run():
    exec('\n\nfrom . import missing', {'__name__': 'test'})


run()
"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__exec_import_traceback.py", line 5, in <module>
    run()
    ~~~~~
  File "builtin__exec_import_traceback.py", line 2, in run
    exec('\n\nfrom . import missing', {'__name__': 'test'})
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
  File "<string>", line 3, in <module>
ImportError: attempted relative import with no known parent package
"""
