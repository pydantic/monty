#!/bin/bash
# Packs @pydantic/monty plus the host's platform binary package and installs
# both into smoke-test/ from the tarballs, verifying the published-package
# experience end to end (binary resolution via the platform package included).
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKSPACE_DIR="$(cd "$ROOT_DIR/../.." && pwd)"

cd "$ROOT_DIR"

echo "=== Building package ==="
npm run build

echo "=== Building the monty binary ==="
cargo build -p monty-cli --manifest-path "$WORKSPACE_DIR/Cargo.toml"

echo "=== Creating platform package ==="
rm -rf npm/
node scripts/create-platform-packages.mjs --current
PLATFORM_DIR=$(ls -d npm/*)
PLATFORM=$(basename "$PLATFORM_DIR")
cp "$WORKSPACE_DIR/target/debug/monty" "$PLATFORM_DIR/"

echo "=== Creating tgz files ==="
cd "$PLATFORM_DIR"
PLATFORM_TGZ=$(npm pack 2>/dev/null)
mv "$PLATFORM_TGZ" "$ROOT_DIR/"
cd "$ROOT_DIR"
MAIN_TGZ=$(npm pack 2>/dev/null)
echo "Created: $PLATFORM_TGZ $MAIN_TGZ"

echo "=== Installing in smoke-test ==="
cd "$ROOT_DIR/smoke-test"
rm -rf node_modules package-lock.json dist

# --no-save/--no-package-lock keep the tarball paths out of the checked-in
# package.json (they're platform- and version-specific, so committing them
# breaks the smoke test everywhere else).
npm install "../$PLATFORM_TGZ" --force --no-save --no-package-lock
npm install "../$MAIN_TGZ" --force --no-save --no-package-lock

echo "=== Type checking ==="
npm run type-check

echo "=== Running smoke tests ==="
# Unset MONTY_BIN so resolution exercises the installed platform package.
env -u MONTY_BIN npm test

echo "=== Cleaning up ==="
cd "$ROOT_DIR"
rm -f "$MAIN_TGZ" "$PLATFORM_TGZ"
rm -rf npm/

echo "=== Smoke test passed! ==="
