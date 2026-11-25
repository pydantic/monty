# Generators own their bound locals and, once paused, their saved stack. These
# cycles are only reachable through a generator, so collecting them traces
# `Generator::for_each_child_id`; the ref-counts below check nothing leaks.
import gc


def make_cycle(started):
    values = []

    def generate(arg):
        yield arg
        yield values

    g = generate(values)
    values.append(g)
    if started:
        next(g)


def delegation_cycle():
    values = []

    def inner():
        yield values

    def outer():
        yield from inner()

    g = outer()
    values.append(g)
    assert next(g) is values


def stop_value_cycle():
    values = []

    def generate():
        yield 1
        return values

    g = generate()
    next(g)
    try:
        next(g)
    except StopIteration as exc:
        values.append(exc)


for started in [False, True]:
    make_cycle(started)
delegation_cycle()
stop_value_cycle()
gc.collect()

# A reachable suspended generator keeps its locals alive across a collection.


def holds():
    values = []
    values.append(values)
    yield 1
    gc.collect()
    assert values[0] is values
    yield values


kept = holds()
assert next(kept) == 1
gc.collect()
held = next(kept)
assert held[0] is held
assert next(kept, 'done') == 'done'

# ref-counts={'gc': 1, 'kept': 1, 'held': 2}
