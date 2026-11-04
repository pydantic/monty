import random

rng = random.Random(123456)
for trial in range(40):
    expected = [rng.randrange(256) for _ in range(rng.randrange(24))]
    owner = bytearray(expected)
    views = []
    mappings = []
    readonly = []
    for turn in range(100):
        active = [i for i in range(len(views)) if views[i] is not None]
        operation = rng.randrange(8)
        if operation == 7:
            try:
                owner.append(255)
            except BufferError:
                assert active
            else:
                assert not active
                expected.append(255)
        elif not active or operation == 0:
            views.append(memoryview(owner))
            mappings.append(list(range(len(owner))))
            readonly.append(False)
        else:
            chosen = active[rng.randrange(len(active))]
            view = views[chosen]
            mapping = mappings[chosen]
            if operation == 1:
                key = slice(rng.randrange(-30, 30), rng.randrange(-30, 30), [-3, -1, 1, 2][rng.randrange(4)])
                views.append(view[key])
                mappings.append(mapping[key])
                readonly.append(readonly[chosen])
            elif operation == 2:
                views.append(view.toreadonly())
                mappings.append(list(mapping))
                readonly.append(True)
            elif operation == 3:
                view.release()
                views[chosen] = None
            elif operation == 4 and mapping:
                position = rng.randrange(len(mapping))
                byte = rng.randrange(256)
                try:
                    view[position] = byte
                except TypeError:
                    assert readonly[chosen]
                else:
                    assert not readonly[chosen]
                    expected[mapping[position]] = byte
            elif operation == 5:
                key = slice(None, None, [-2, -1, 1, 2][rng.randrange(4)])
                targets = mapping[key]
                rhs = bytearray([rng.randrange(256) for _ in targets])
                try:
                    view[key] = rhs
                except TypeError:
                    assert readonly[chosen]
                else:
                    assert not readonly[chosen]
                    for target, byte in zip(targets, rhs):
                        expected[target] = byte
            elif operation == 6:
                assert view.tolist() == [expected[i] for i in mapping]
                assert view.obj is owner
            view = None
        assert list(owner) == expected, (trial, turn, operation)
    views = None
    owner.append(0)
    assert list(owner) == expected + [0]
