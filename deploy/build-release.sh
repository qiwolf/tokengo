#!/usr/bin/env bash
# Linux-native build only. Refuse dirty worktrees so both halves identify one commit.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'Requires Linux x86_64 native builder' >&2; exit 1; }
[[ -z $(git status --porcelain --untracked-files=normal) ]] || { echo 'Commit source before building a release' >&2; exit 1; }
[[ $(trunk --version) == 'trunk 0.21.14' ]] || { echo 'Requires Trunk 0.21.14' >&2; exit 1; }
export RUSTUP_TOOLCHAIN=1.98.0
# Trunk 0.21 expects a boolean; many terminal runners export NO_COLOR=1.
export NO_COLOR=true
cargo build --locked --release --workspace --target x86_64-unknown-linux-gnu
(cd crates/web && trunk build --release --locked)
# --locked and Trunk must leave the source commit unchanged.
[[ -z $(git status --porcelain --untracked-files=normal) ]] || { echo 'Build changed source/lockfile' >&2; exit 1; }
python3 deploy/package.py "${1:-/tmp/tokengo-release}"
