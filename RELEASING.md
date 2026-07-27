# Release Process

## 1. Bump Version

Edit `Cargo.toml` to bump the version (both the main `version` and the package versions)

Run

```bash
make lint-rs
```

This will update `Cargo.lock`, sync `package.json`/`package-lock.json` (via `crates/monty-js/build.rs`),
and sync `pydantic-monty`'s exact pin on `pydantic-monty-runtime` (via `crates/monty-python/build.rs`).

## 2. Commit and Push

```bash
git checkout -b prepare-release-vX.Y.Z
git add .
git commit -m "Bump version to vX.Y.Z"
git push
```

## 3. Create Release via GitHub UI

Once the PR is merged, create a release in the GitHub UI with a tag matching the version in `Cargo.toml`.

## 4. CI Handles Publishing

Once the tag is pushed, CI will:
- Build wheels for all platforms
- Publish to PyPI (`pydantic-monty`)
- Publish to NPM (`@pydantic/monty` + the platform packages carrying the napi library, the `monty` binary, and the wasm build)
- Publish the Rust crates to crates.io (`monty`, `monty-types`, `monty-fs`, `monty-runtime`, `monty-macros`, `monty-proto`, `monty-pool`, `monty-type-checking`, `monty-typeshed`) via `cargo publish --workspace`

Monitor the workflow at https://github.com/pydantic/monty/actions

## Pre-release Tags

For pre-releases (alpha, beta, rc), use a tag like `v0.0.2-beta.1`:
- PyPI: Published normally
- NPM: Published with `--tag next` (not `latest`)
