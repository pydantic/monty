"""Makes `python -m pydantic_monty` run the `monty` binary; see [`_cli`]."""

from __future__ import annotations

from ._cli import main

raise SystemExit(main())
