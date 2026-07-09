# monty-fuzz

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer) targets for
[Monty](https://github.com/pydantic/monty). Fuzz inputs are parsed and
executed under tight resource limits; parse and runtime errors are expected
and ignored — the targets only hunt for panics, crashes, and hangs in the
interpreter itself.

Targets:

- `string_input_panic` — feeds arbitrary strings through parse + execute
- `tokens_input_panic` — assembles syntactically plausible Python from a
  token vocabulary, reaching deeper into execution than random strings

Run with the nightly toolchain from the workspace root:

```console
make fuzz-string_input_panic
make fuzz-tokens_input_panic
# or directly:
cargo +nightly fuzz run --fuzz-dir crates/fuzz string_input_panic
```

`corpus/` holds the accumulated seed inputs; `artifacts/` collects crash
reproductions. Internal to the workspace; not published.
