#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"

echo "=== Building package ==="
npm run build

# Detect current platform
NODE_FILE=$(ls monty.*.node 2>/dev/null | head -1)
if [ -z "$NODE_FILE" ]; then
    echo "Error: No .node file found after build"
    exit 1
fi

# Extract platform from filename (e.g., monty.darwin-arm64.node -> darwin-arm64)
PLATFORM=$(echo "$NODE_FILE" | sed 's/monty\.\(.*\)\.node/\1/')
echo "Detected platform: $PLATFORM"

echo "=== Setting up platform packages ==="
npm run create-npm-dirs

# Copy binary to platform package directory (simulates napi artifacts)
PLATFORM_DIR="npm/$PLATFORM"
if [ ! -d "$PLATFORM_DIR" ]; then
    echo "Error: Platform directory $PLATFORM_DIR not found"
    exit 1
fi
cp "$NODE_FILE" "$PLATFORM_DIR/"

# Add optionalDependencies to main package.json (without publishing)
npx napi prepublish -t npm --skip-optional-publish

echo "=== Creating platform package tgz ==="
cd "$PLATFORM_DIR"
PLATFORM_TGZ=$(npm pack 2>/dev/null)
mv "$PLATFORM_TGZ" "$ROOT_DIR/"
cd "$ROOT_DIR"
echo "Created: $PLATFORM_TGZ"

echo "=== Creating main package tgz ==="
MAIN_TGZ=$(npm pack 2>/dev/null)
echo "Created: $MAIN_TGZ"

echo "=== Installing in smoke-test ==="
cd "$ROOT_DIR/smoke-test"
rm -rf node_modules package-lock.json

# Install platform package first, then main package
npm install "../$PLATFORM_TGZ" --force
npm install "../$MAIN_TGZ" --force

echo "=== Type checking ==="
npm run type-check

echo "=== Running smoke tests ==="
npm test

echo "=== Cleaning up ==="
cd "$ROOT_DIR"
rm -f "$MAIN_TGZ" "$PLATFORM_TGZ"
rm -rf npm/
# Remove optionalDependencies added by napi prepublish (keeps other package.json changes)
npm pkg delete optionalDependencies 2>/dev/null || true

echo "=== Smoke test passed! ==="
