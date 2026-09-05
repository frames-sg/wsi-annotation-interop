# Engineering quality gates

These rules define the checks this repository actually runs. Structural items are review
heuristics; executable gates and manual full-profile requirements are identified separately.

## Before writing code

- Establish accepted inputs, observable outputs, ordering, failure behavior,
  resource limits, and compatibility constraints.
- Search definitions and references with the Rust LSP. Search literals,
  options, diagnostics, and configuration with `rg`.
- Reuse an existing type, validator, transform, writer, or error path when its
  semantics match. If they differ, keep the format-specific code local instead
  of forcing a misleading shared abstraction.
- Verify every proposed dependency in its primary registry and upstream
  documentation before editing a manifest. Record the exact version, features,
  MSRV, license, advisory result, and concrete call site.

## Structural constraints

- Do not add catch-all `utils`, `common`, `helpers`, or `manager` modules,
  facade layers, service registries, speculative traits, compatibility
  wrappers, placeholder branches, or unused flags.
- The four real raster adapters justify exactly one private `RasterSource`
  boundary. No other interface is added until at least two current
  implementations or callers need the same semantics.
- Shared coordinate, code, identity, output-publication, checksum, and error
  behavior lives once. Format decoding stays with its format.
- A materially changed file above roughly 500 lines, function above 75 lines,
  nesting beyond four levels, or repeated branch is a mandatory cohesion
  review. Line count alone never justifies a split.
- A new file must own a real responsibility, dependency direction, lifecycle,
  or test boundary. No `_new`, `_fixed`, `_v2`, backup, or parallel production
  implementation is allowed.

## Implementation constraints

- Use typed input parsing with unknown-field and duplicate-key rejection at
  untrusted boundaries. Never guess codes, coordinates, channel order, identity,
  or lossy behavior.
- Expected input and I/O failures use explicit contextual errors. Production
  code adds no unchecked `unwrap`, `expect`, swallowed error, or silent
  fallback.
- Comments explain DICOM invariants, numerical choices, bounds, or why a
  surprising operation is correct. Delete comments that merely narrate syntax.
- Python remains a file-level highdicom/pydicom oracle. It must not duplicate
  the Rust conversion path.
- Remove debug output, dead branches, unused features, and unreachable CLI
  options before a checkpoint closes.

## Test constraints

- Follow red-green-refactor for every behavior change and retain the failing
  reason in the test’s semantic assertion.
- Assert geometry, pixels, codes, identities, references, diagnostics,
  checksums, and absence of partial output. A test that merely reproduces the
  implementation’s calculation is not an oracle.
- Prefer real TIFF, NPY, Zarr, DICOM, and process boundaries. Synthetic sources
  are appropriate for resource limits but do not replace independent
  highdicom reads or external validators.
- Use mocks only when the real boundary cannot be exercised safely and cheaply.
  Do not expose internals or add an interface solely to make mocking easier.
- Do not weaken assertions, delete passing tests, or update snapshots without
  separately reviewed ground truth.

## Executable gates

`./scripts/check-quality.sh` is the repository-only quality gate. It runs, in order:

- `uv sync --locked` (or `--extra "$UV_SYNC_EXTRA"` when explicitly selected);
- pytest, Ruff formatting, Ruff lint, and Pyright for the independent shim;
- Rustfmt;
- all Rust targets and schema/example integration tests;
- Clippy with warnings denied;
- a locked release build.

`./scripts/check-core.sh` first builds and tests the headless `wsi-annotation-probe` package in the sibling annotations repository, then invokes the
quality gate with its exact path. The sibling is mandatory for this gate; absence or a contract
failure fails the command.

`./scripts/check-full.sh` is the explicit manual/full-runner gate. It requires `pydcm`, all four
validators, an executable `ORTHANC_EXECUTABLE`, and the annotations checkout. It runs the core gate,
builds the release probe, executes the full profile, and publishes only the transactional run. A
missing required component exits nonzero and is not a pass.

CI runs the core gate on pushes and pull requests. The full gate runs only on the labeled
self-hosted workflow-dispatch runner. Both profile jobs retain complete run artifacts and hashes.

## Review and manual checks

At a checkpoint, run the narrow red-green cycle, the applicable executable gate, and inspect the
complete diff and status. Review correctness, security, compatibility, scientific meaning,
cohesion, dependency cost, and benchmark limitations. No named prose-review tool or magic output
phrase is a gate unless it is installed and invoked by a script or workflow.

Dependency advisories, licenses, unused dependencies, cross-target builds, sanitizers, and Miri are
manual checks unless a future pinned tool is added to the executable scripts. Do not claim they
passed based on this policy alone. Orthanc is unavailable—not passed—outside the full gate.
