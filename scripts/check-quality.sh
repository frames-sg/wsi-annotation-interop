#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
harness_root="$(cd "${script_directory}/.." && pwd)"

cd "${harness_root}"

uv_sync_args=(--locked)
if [[ -n "${UV_SYNC_EXTRA:-}" ]]; then
  uv_sync_args+=(--extra "${UV_SYNC_EXTRA}")
fi
uv sync "${uv_sync_args[@]}"
uv run pytest
uv run ruff format --check shim
uv run ruff check shim
uv run pyright --pythonpath .venv/bin/python shim
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
