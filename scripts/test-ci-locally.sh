#!/usr/bin/env bash
# Run the same checks as .github/workflows/ci.yml, then the pre-publish
# checks for crates.io. Usage: scripts/test-ci-locally.sh

set -euo pipefail
cd "$(dirname "$0")/.."

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

step() { echo -e "\n${YELLOW}$1${NC}"; }
ok() { echo -e "${GREEN}✓ $1${NC}"; }
fail() { echo -e "${RED}✗ $1${NC}"; exit 1; }

echo -e "${YELLOW}=== Testing CI workflow locally ===${NC}"

step "Step 1: cargo fmt --check"
cargo fmt --all -- --check || fail "formatting (run 'cargo fmt --all')"
ok "fmt"

step "Step 2: clippy (all targets, all features, warnings are errors)"
cargo clippy --all-targets --all-features -- -D warnings || fail "clippy"
ok "clippy"

step "Step 3: tests (all features)"
cargo test --all-features || fail "tests"
ok "tests"

step "Step 4: docs (warnings are errors)"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features || fail "docs"
ok "docs"

step "Step 5: wasm32 check (default features)"
if rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    cargo check --target wasm32-unknown-unknown || fail "wasm32 check"
    ok "wasm32"
else
    echo -e "${YELLOW}⚠ wasm32-unknown-unknown not installed, skipping${NC}"
    echo -e "${YELLOW}  rustup target add wasm32-unknown-unknown${NC}"
fi

step "Step 6: the real-file test needs the vendor fixture"
if [[ -n "${TM25RAY_FIXTURE:-}" ]] || ls ~/Downloads/*/rayfile_SU_CULCP1_VC_100k*.TM25RAY >/dev/null 2>&1; then
    ok "fixture present, tests/real_file.rs ran against it"
else
    echo -e "${YELLOW}⚠ no vendor fixture (TM25RAY_FIXTURE unset), tests/real_file.rs was a no-op${NC}"
fi

echo -e "\n${BLUE}=== Pre-publish checks for crates.io ===${NC}"

step "Step 7: package contents"
cargo package --list --allow-dirty > /dev/null || fail "cargo package --list"
if cargo package --list --allow-dirty | grep -q '^docs/sample/'; then
    fail "sample ray files would ship in the crate; check 'exclude' in Cargo.toml"
fi
ok "package metadata valid, samples excluded"

step "Step 8: cargo publish --dry-run"
cargo publish --dry-run --allow-dirty || fail "dry-run publish"
ok "dry-run publish"

step "Step 9: git state"
if [[ -n $(git status --porcelain) ]]; then
    echo -e "${YELLOW}⚠ uncommitted changes:${NC}"
    git status --short
else
    ok "working tree clean"
fi
branch=$(git branch --show-current)
if [[ "$branch" != "main" ]]; then
    echo -e "${YELLOW}⚠ on branch '$branch', not main${NC}"
else
    ok "on main"
fi
version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
if git tag | grep -qx "v$version"; then
    echo -e "${YELLOW}⚠ tag v$version already exists; bump the version before publishing${NC}"
else
    ok "version $version is not tagged yet"
fi

echo -e "\n${GREEN}=== All CI checks passed ===${NC}"
echo -e "To release:"
echo -e "  ${YELLOW}git tag v$version && git push origin main v$version${NC}"
echo -e "  ${YELLOW}cargo publish${NC}"
