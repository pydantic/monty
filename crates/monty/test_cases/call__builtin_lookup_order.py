# xfail=monty
# Builtin method lookup should fail before evaluating arguments.
events = []


def argument():
    events.append(42)


for receiver in (list, []):
    try:
        receiver.missing(argument())
    except AttributeError:
        pass
    else:
        assert False

assert events == []
