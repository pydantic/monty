# monty-pgo

Trains the instrumented worker that Maturin installs during a PGO build.
It runs the interpreter's test-case corpus through subprocess pool sessions, including type checking.
Workers shut down cleanly to flush their profile data.

The driver depends only on `monty-pool`, `monty-types` and Tokio.
Keeping it separate from `monty-datatest` avoids compiling the interpreter and CPython bindings into the training build.
The `pgo-training` Cargo profile disables optimisation, debug information and incremental compilation for the driver;
the worker retains its release and PGO settings.

Run `make dev-py-pgo` for a local PGO build.
The runtime's `pyproject.toml` configures Maturin to invoke this driver with the instrumented worker on `PATH`.
