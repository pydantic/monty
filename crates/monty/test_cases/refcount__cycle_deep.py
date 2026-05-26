# Deep nested cycle: a chain of 1000 lists where the last one references the
# first, forming one huge cycle. This stresses the iterative implementations
# of `MarkGray`, `Scan`, and `CollectWhite` in the trial-deletion collector —
# a recursive walk would blow the Rust stack on a 1000-deep cycle. We use
# 1000 here (not 10000) because CPython's default recursion limit caps the
# Python-level construction, but the heap-side traversal under
# `memory-model-checks` (where GC fires every dec_ref) is still exercised.
def make_deep_cycle(n):
    head = []
    current = head
    for _ in range(n):
        nxt = []
        current.append(nxt)
        current = nxt
    # Close the cycle
    current.append(head)
    return head


a = make_deep_cycle(1000)
# Reassign to drop the only outside reference; the resulting cycle is
# unreachable and must be freed by the next collection without overflowing
# the Rust stack while iterating the chain.
a = 'done'
a
# Return.str=done
