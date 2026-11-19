# A snippet that fails to compile releases the namespace dicts it was given.
bad_globals = {}
bad_locals = {}
for _ in range(2):
    try:
        exec('await x', bad_globals, bad_locals)
    except SyntaxError:
        pass
# ref-counts={'bad_globals': 1, 'bad_locals': 1}
