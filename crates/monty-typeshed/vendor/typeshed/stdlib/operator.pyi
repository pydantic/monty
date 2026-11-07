"""Monty's `operator`: a stand-in re-exporting `_operator` until the vendored `operator.py` arrives.
Left out: the dunder aliases (`__add__` ...) and `__all__`."""

from _operator import *
