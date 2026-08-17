#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
harness_root="$(cd "${script_directory}/.." && pwd)"
viewer_root="${DICOM_VIEWER_ROOT:-$(cd "${harness_root}/../dicom-viewer" && pwd)}"
probe_path="${viewer_root}/target/debug/annotation_probe"

test -f "${viewer_root}/Cargo.toml"

cargo test --manifest-path "${viewer_root}/Cargo.toml" -p dicom-viewer-core --locked
cargo test --manifest-path "${viewer_root}/Cargo.toml" \
  -p dicom-viewer --bin annotation_probe --locked
cargo build --manifest-path "${viewer_root}/Cargo.toml" \
  -p dicom-viewer --bin annotation_probe --locked

cd "${harness_root}"
uv sync --locked
uv run pytest
uv run ruff format --check shim
uv run ruff check shim
uv run pyright --pythonpath .venv/bin/python shim
cargo fmt --all -- --check
ANNOTATION_PROBE="${probe_path}" cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
