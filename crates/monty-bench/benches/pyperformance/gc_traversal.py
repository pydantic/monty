# Adapted from pyperformance bm_gc_traversal at ccc0aeb7ad46d65b6dcd4160e0fdda4d885852dd.
# See README.md for harness changes and LICENSE for upstream terms.
import gc

N_LEVELS = 500


def create_recursive_containers(n_levels):
    current_list = []
    for n in range(n_levels):
        new_list = [None] * n
        for index in range(n):
            new_list[index] = current_list
        current_list = new_list

    return current_list


def benchamark_collection(loops, n_levels):
    _all_cycles = create_recursive_containers(n_levels)
    for _ in range(loops):
        gc.collect()
        collected = gc.collect()

        assert collected is None or collected == 0

    return 0


benchamark_collection(1, N_LEVELS)
