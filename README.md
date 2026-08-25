# DICOM WSI Annotation Interoperability Harness

This repository is the neutral Rust study harness for comparing DICOM Whole
Slide Microscopy ANN, SEG, Comprehensive 3D SR, and Parametric Map
implementations against declarative ground truth. It interacts with
`dicom-viewer` only through DICOM files, the `annotation_probe` subprocess, and
its versioned JSON report contracts.

The core profile uses deterministic, non-PHI synthetic cases. External validators,
Orthanc transport, optional implementations, and large-scale workloads belong to the
explicit full profile and never target a remote archive by default.

Rust writes `ground-truth-v1.json`, a declarative semantic oracle constructed
without reading generated DICOM files. The core matrix covers ANN/SEG parsing
and round trips. Separate conversion matrices cover GeoJSON-to-ANN, direct and
SEG-referenced SR, float32 PM, and forced multi-part PM concatenation. A
file/JSON-only Python shim under `shim/` is retained solely for independent
highdicom/pydicom fixture generation and normalization. Rust owns the CLI,
orchestration, comparison, metrics, validation, scaling, transport, and result
generation.

Public schemas and tested examples live under `schema/` and `examples/`.
Implementation and review constraints are recorded in
[Engineering quality gates](docs/ENGINEERING_QUALITY.md).

## Development

```console
uv sync --locked
uv run pytest
uv run ruff check shim
uv run ruff format --check shim
uv run pyright --pythonpath .venv/bin/python shim
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

The cross-repository core gate builds the sibling viewer probe, runs the Rust tests,
executes the synthetic highdicom/viewer matrix, checks the reference shim, and builds
the Rust binary:

```console
./scripts/check-core.sh
```

Use `./scripts/check-quality.sh` for the repository-only executable gate. The explicit full gate is
`ORTHANC_EXECUTABLE=/absolute/path/to/Orthanc ./scripts/check-full.sh`; it fails when Orthanc,
pydcm, a required validator, or the sibling viewer is unavailable.

Set `DICOM_VIEWER_ROOT` only when the viewer checkout is not the default sibling
directory.

## Study profiles

Generate the immutable core artifacts with an explicit run identifier:

```console
cargo run --release --locked -- run-core \
  --probe ../dicom-viewer/target/debug/annotation_probe \
  --results results \
  --run-id core-001
```

The full profile adds all four validator commands, pydcm qualification, required 1M
and attempted 5M coordinate cases, and local Orthanc DICOMweb transport. Install the
optional pinned pydcm environment and supply Orthanc explicitly when it is available:

```console
uv sync --locked --extra pydcm
cargo run --release --locked -- run-full \
  --probe ../dicom-viewer/target/release/annotation_probe \
  --dicom-edition 2026c \
  --orthanc /absolute/path/to/Orthanc \
  --results results \
  --run-id full-001
```

`validate_iods`, `dciodvfy`, `dcentvfy`, and `dcm2json` are discovered from `PATH`.
Each run captures their versions, editions, commands, return codes, stdout, and stderr.
Orthanc is always started with loopback-only access and isolated temporary storage; no
remote archive URL is accepted. If a validator, pydcm capability, or Orthanc is absent,
the observation is retained as unavailable or unqualified rather than silently omitted.

Each completed run contains checksummed fixtures and roundtrips, JSONL/CSV observations,
JSON/CSV summaries, coordinate/mask/runtime/memory figures, and a final schema-v2
`manifest.json`. The manifest records harness and probe Git state, binary and lockfile hashes,
toolchains, Python packages, resource policies, schema versions, and CI identity; unavailable
source identity is `null` with a reason rather than an invented SHA. Run directories are created
exclusively and are never overwritten.

CI uploads the complete core/full directory together with a compressed bundle, the bundle SHA-256,
and the manifest SHA-256. The artifact name includes the immutable GitHub run ID and attempt.
Legacy schema-v1 manifests remain readable, but they do not acquire provenance or retained files
that their original run did not record.
