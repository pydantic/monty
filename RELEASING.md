# Release Process

## 1. Bump Version

Update version in both files:

```bash
# Edit Cargo.toml - update workspace.package.version AND the internal crate
#   versions in [workspace.dependencies] (monty, monty-macros, monty-proto,
#   monty-pool, monty-type-checking, monty-typeshed). These must all equal
#   workspace.package.version or `cargo publish` will fail.
# Edit crates/monty-js/package.json - update version AND the optionalDependencies
#   versions (they must all equal the package version; CI's
#   create-platform-packages fails if they drift)

# Update Cargo.lock
make lint-rs
```

`Cargo.toml` and `package.json` should have the same version (e.g., `0.0.2`).

## 2. Commit and Push

```bash
git add Cargo.toml Cargo.lock crates/monty-js/package.json
git commit -m "Bump version to X.Y.Z"
git push
```

## 3. Create Release via GitHub UI

1. Go to https://github.com/pydantic/monty/releases/new
2. Click "Choose a tag" and type the new tag name (e.g., `v0.0.2`)
3. Select "Create new tag on publish"
4. Set the release title (e.g., `v0.0.2`)
5. Add release notes
6. Click "Publish release"

## 4. CI Handles Publishing

Once the tag is pushed, CI will:
- Build wheels for all platforms
- Publish to PyPI (`pydantic-monty`)
- Publish to NPM (`@pydantic/monty` + the platform packages carrying the napi library, the `monty` binary, and the wasm build)
- Publish the Rust crates to crates.io (`monty`, `monty-cli`, `monty-macros`, `monty-proto`, `monty-pool`, `monty-type-checking`, `monty-typeshed`) via `cargo publish --workspace`

Monitor the workflow at https://github.com/pydantic/monty/actions

## Pre-release Tags

For pre-releases (alpha, beta, rc), use a tag like `v0.0.2-beta.1`:
- PyPI: Published normally
- NPM: Published with `--tag next` (not `latest`)
