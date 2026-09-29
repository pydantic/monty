# runaway

A list that never stops growing, then a follow-up in the same session.
The session has a 30 MB `max_memory`; the primary request asks for a list of a million 10 KB strings and must end in
a `MemoryError` raised inside the sandbox (`expect_error='MemoryError'`).
The follow-up asks the model to rebind `chunks` to `None` and return the sum of squares of 1 to 1000, which proves
the session is still usable once the global holding the memory is released.

A time limit behaves differently: `max_duration_secs` is a budget for the whole session, so after a `TimeoutError`
nothing else runs in that session.
That is why the case uses memory, not time, to show recovery.

Scored with the `expected_error` assertion on the primary run, `follow_up_correct` on 333833500, and zero host calls
on both turns.
