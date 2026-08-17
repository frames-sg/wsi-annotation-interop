# Engineering quality and anti-slop gates

These rules prevent maintainability defects in this repository and in the
sibling Rust converter. They are review heuristics, not an authorship detector:
none of the signals below proves that code was written by AI.

The policy is grounded in three current empirical warnings. A 2026 controlled
study found no systematic downstream maintainability advantage or disadvantage
overall; its first phase nevertheless observed more new duplication, nested
complexity, and complex conditionals in the AI-assisted group
([Borg et al.](https://link.springer.com/article/10.1007/s10664-026-10889-1)).
A study of coding-agent commits found mocks in 36% of agent commits versus 26%
of non-agent commits
([Hora and Robbes](https://arxiv.org/abs/2602.00409)). Package hallucinations can
also turn invented dependency names into a supply-chain attack surface
([Krishna et al.](https://derczynski.com/papers/importing_phantoms.pdf)).

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

## Checkpoint gate

At every implementation checkpoint:

1. Run the focused red-green-refactor cycle and applicable full checks.
2. Review correctness, security, compatibility, scientific meaning, the full
   diff, and status of both repositories.
3. Run `ponytail-review`, whose output is one deletion-oriented finding per
   line in the form `file:Lline: tag — cut; replace with simpler alternative`.
4. Resolve every valid finding outside the review and rerun all invalidated
   checks.
5. Close the checkpoint only when the final review output is exactly
   `Lean already. Ship.`. Any later code change invalidates that pass.

The final gate also runs formatting, Clippy, all tests, feature checks, release
builds, dependency pruning/audit/license checks, the cross-repository core
harness, DICOM validators, the full profile, and local Orthanc transport when
the executable is available. A missing tool is reported as an unverified gate,
never converted into a pass.
