# Harness hardening implementation record

This document is the durable implementation and validation record for WAI-001 through WAI-018.
Harness work concluded on 2026-08-20. The harness remains independent: all evaluated
implementations are reached only through files, versioned JSON, subprocesses, validators, or
loopback Orthanc.

## Baseline metadata

- Recorded: 2026-08-20, America/New_York.
- Harness: `abe65b1c66318e41dea71c084f30784b9496447a`, branch `main`, clean tree before this plan.
- Toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, host
  `aarch64-apple-darwin`, LLVM 22.1.6; `cargo 1.97.1`.
- Python: the bare `python` command is unavailable; `uv run python --version` is Python 3.12.9.
  uv is 0.7.17.
- Lock state: repository `package-lock.json` absent. `Cargo.lock` SHA-256 is
  `32bffa30dc3c19c6a71e7c769150babaf00562984791263a41c8a5592edb47b2`; `uv.lock`
  SHA-256 is `866b898c75ba2ef1220b9feb20abcb9e1aafa16bdf8c767ce8d2200e60414766`.
- Machine: Apple M4 Pro, 48 GiB RAM, arm64 macOS/Darwin 25.5.0. Machine load is not isolated;
  performance results must report spread and are comparative local evidence, not universal timing.
- Validators on PATH: `validate_iods` at `/Users/user/.local/bin/validate_iods`, `dciodvfy` at
  `/opt/homebrew/bin/dciodvfy`, `dcentvfy` at `/Users/user/.local/bin/dcentvfy`, and `dcm2json`
  at `/opt/homebrew/bin/dcm2json` (DCMTK 3.7.0 reported by its version command). Exact validator
  provenance will be collected through the harness because CLI version syntax differs.
- Orthanc: executable not on PATH; plugins not inventoried because the server is unavailable.
- Sibling probe: `../dicom-viewer/target/debug/annotation_probe` and release binary are present.
  Sibling SHA is `87aa73d8cf5223ef6ba674e2ef7747453df06442`; its tree contains extensive pre-existing
  changes, including the currently untracked probe source. Binary SHA-256 values are
  `c0a9cc44fd8baa4134567f4420ea4fafefb816e7af48215548db927fdd4a0347` (debug) and
  `2af3da06dac0af0dfe381e90527a4153f50e1492f8b69c553d8d64d09c14e03a` (release).

### Baseline checks

| Command | Result |
|---|---|
| `uv sync --locked` | Passed; removed optional `pydcm==0.4.5` from the environment. |
| `uv run pytest` | Passed: 7 tests. |
| `uv run ruff format --check shim` | Passed: 11 files formatted. |
| `uv run ruff check shim` | Passed. |
| `uv run pyright --pythonpath .venv/bin/python shim` | Passed: 0 errors. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo test --all-targets --locked` | Failed in existing CLI expectation: `rust_cli_runs_core_and_reports_pydcm_as_nonprimary` expected `primary_failure == true`, observed JSON `null`; prior unit and CLI help tests passed. |
| `cargo clippy --all-targets --locked -- -D warnings` | Passed. |
| `cargo build --release --locked` | Passed. |
| `./scripts/check-core.sh` | Pending; sibling is available but dirty. |

## Findings

Status values: confirmed, obsolete, in progress, fixed, blocked, or unverified.

| ID | Status and current evidence | Files | Regression/performance evidence | Acceptance criteria / constraints |
|---|---|---|---|---|
| WAI-001 | Fixed: typed runner defaults to 100 ms sampling, takes one startup inventory, then refreshes only tracked PIDs; wait polling is independently 10 ms. Timeout takes a final tree inventory for termination. | `src/process.rs`, adapters/provenance | Ten focused tests pass: configuration, metadata, disabled sampling, timeout/tree, early close/EOF, reader failure, bounded evidence, non-UTF-8. Release calibration recorded below. | Accepted: no repeated 1 ms all-system loop; sampler configuration and evidence are published. |
| WAI-002 | Fixed: readers drain to EOF while retaining at most 16 MiB stdout and 4 MiB stderr, with total/truncated metadata; JSON adapters fail closed on truncated stdout; known-validator-defect classifiers reject any truncated stream; Orthanc log readers retain 4 MiB per stream. | process/probe/shim/validators/Orthanc | Process/probe/validator/Orthanc suites pass, including timeout evidence, log producers, and truncated known-defect rejection. | Accepted for every subprocess stream. |
| WAI-003 | Fixed: validated canonical intervals merge same-segment overlap/adjacency; area, identity-aware intersection/Dice, centroid, and multi-segment overlap operate on runs. Topology and surface distances use exact per-segment occupied crops capped at 4,000,000 cells by default. | `src/metrics.rs`, `src/metrics/{runs,crop,distance}.rs`, `tests/rust_metrics.rs` | 48 deterministic randomized two-segment masks match an independent dense/brute-force oracle; huge sparse, large multi-segment, ordering, normalization, holes, diagonal connectivity, spacing, empty, overflow, and resource tests pass. Release timing and 7 MiB whole-test RSS recorded below. | Accepted: no full-slide dense allocation; core metrics scale with run/event count; versioned advanced resource limitation retains core results. |
| WAI-004 | Fixed: matrix cases append typed phase observations and derive overall status; no `case_observation` error collapse remains. Probe commands, reports, elapsed/RSS, output bounds, artifacts, comparison findings, and exact late errors survive. | matrix/probe/observation/schema | Recorder unit test plus a real subprocess/reference late-output-normalization regression pass and validate against v2. | Accepted: late errors retain earlier evidence and top-level compatibility fields are derived. |
| WAI-005 | Fixed in harness: expected read-only SEG rejection requires exact `REWRITE_UNSUPPORTED` and no output artifact. Generic `OPERATION_FAILED` and artifact-present cases fail. | `src/matrix.rs`, probe evidence | Focused generic/precise/artifact test passes. Live sibling cases fail clearly with exact generic code and message. | Harness accepted. Cross-repository blocker: sibling SHA `87aa73d8…` must emit `REWRITE_UNSUPPORTED` specifically for unsupported SEG rewrites. |
| WAI-006 | Fixed: `RunWriter` creates a hidden sibling `TempDir`, writes and syncs all artifacts there, writes/verifies the manifest last, syncs staging, checks destination collision, atomically renames, then syncs the parent. Drop cleans failed staging. | `src/results/writer.rs`, `src/results/manifest.rs`, tables/figures in `results.rs`, tests | Six real-filesystem tests cover pre-artifact validation, figure failure, manifest failure, cleanup/retry, collision, hidden staging, successful publication, and duplicate publication. | Accepted: final directory is all-or-nothing and retryable after failed writer drop. Standard-library rename has a documented narrow no-clobber race between final existence check and rename. |
| WAI-007 | Fixed: each object is a separate streaming chunked STOW request; its bounded DICOM JSON SQ/UI response must report only that SOP UID once and no failures. QIDO requires typed UI/value structure and exactly one matching UID. Direct and multipart WADO stream through capped temporary files and publish only a complete unambiguous DICOM part. | `src/orthanc.rs`, `tests/rust_orthanc.rs` | Twelve focused tests plus 1/8/32 MiB calibration cover per-instance identity, strict structure, limits, malformed boundaries/missing parts, and cleanup. | Accepted: design and measured RSS show no complete-object client copy. |
| WAI-008 | Fixed: STOW/QIDO/WADO have explicit limits; Orthanc stdout/stderr are concurrently drained with 4 MiB retained caps and truncation fields; startup retries three times only for constrained bind-collision signatures; all attempts retain isolated loopback configuration and cleanup. | `src/orthanc.rs`, `src/orthanc/local.rs`, `src/profiles.rs`, `tests/rust_orthanc.rs` | Synthetic process writes 5 MiB to each stream without blocking and records both truncations; a bind-collision program is launched exactly three times; transport failure tests prove final-path absence. | Accepted: all transport/log paths deliberate and bounded; released-port race mitigated with constrained retry. Real Orthanc remains unavailable locally. |
| WAI-009 | Fixed: `profiles::baseline::run` is the single owner of fixture generation, core matrix, conversion matrices, qualification, tagged baseline observations, and typed matrix/conversion status. Core publishes that result; full destructures the same result and appends validator, scale, and Orthanc arms before publication. Both record baseline definition version 1. | `src/profiles.rs`, `src/profiles/baseline.rs`, `tests/rust_profiles.rs` | Typed status truth table passes; real core profile test passes and verifies the definition version plus overall status derived from its exact baseline observations. | Accepted: only one `generate_core`, `run_core_matrix`, and `run_conversion_matrices` profile call site remains. A real full profile remains unavailable without Orthanc. |
| WAI-010 | Fixed: canonical level-0 and native coordinates are described by two `CoordinateSpec` values and processed by one primitive extraction/alignment/distance engine. The spec explicitly chooses coordinate/dimension keys, index-dimension rule, optionality, and XY-plus-Z versus all-dimension distance. | `src/compare.rs`, `src/compare/geometry.rs`, `src/compare/statistics.rs`, `tests/rust_compare.rs` | Nine tests pass across polygon symmetry, ellipse axes, scalar primitive offsets, canonical/native errors, duplicate identities, malformed group entries, missing segment identities, SEG semantics, and tolerance findings. | Accepted: one primitive engine drives both forms; no silently skipped malformed list/key entries. |
| WAI-011 | Fixed: ANN and writable SEG now traverse one linear `run_case` phase machine. A typed `CaseDomain` retains separate ANN/SEG normalization and comparison methods; typed `RewritePolicy` sends non-binary SEG through the strict expected-rejection branch. One `CaseRecorder` centralizes commands, time, RSS, artifacts, failure evidence, and status. | `src/matrix.rs`, `src/matrix/observation.rs` | Four recorder/lifecycle tests pass; live release/debug probe runs retain their distinct generic-rejection and LABELMAP-inspection failures. | Accepted: no duplicated complete phase machine; domain comparison remains explicit and read-only policy is typed. |
| WAI-012 | Fixed: publication/manifest/provenance, profile baseline, matrix observation/recording, metric runs/crops/distances, comparison geometry/statistics, validator spec/discovery/version/observation, and three known-defect policies have concrete owned modules. Shared matrix flow remains deliberately linear and explicit; profile/result fronts are below the review threshold. | named modules | All focused suites and Clippy pass after decomposition. | Accepted: no catch-all layer or speculative registry; further splitting the auditable case flow would reduce locality rather than cohesion. |
| WAI-013 | Fixed: one internal `CommandSpec` owns `OsString` program/args, execution specs, and deliberately lossy display provenance. CLI, probe, shim, validators, and Orthanc invoke paths without prior conversion; duplicate `path_text` is gone. Probe operation/payload/targets, matrix phase/status, profile, validator status/invocation, conversion matrix/target/status, scale status, rewrite policy, and transport kind are typed. | command adapters and domain DTOs | Non-UTF-8 runner passes; Linux adapter case is compiled there because Darwin rejects its invalid-byte fixture. All adapter/domain suites pass. | Accepted: remaining `Vec<String>` fields are serialized provenance or public string-based validator configuration, not path invocation. |
| WAI-014 | Fixed: implementation-only conversion, ground-truth, matrix, profile, scale, and process modules are private; the binary/tests consume narrow root reexports. Public modules are the intentional comparator, metric, DICOMweb, adapter, result, schema, shim, and validator APIs. | `src/lib.rs`, main/tests | All targets compile through narrowed paths. | Accepted: public surface follows intended independent harness components, not every implementation module. |
| WAI-015 | Fixed for newly produced/CI runs: schema-v2 manifests retain harness/probe Git SHA, dirty/branch state, probe and harness executable hashes, toolchains, target/profile/RUSTFLAGS/features, lock hashes, Python/package identity, OS/architecture/CI identity, schema/profile/resource policies, and locations of validator/Orthanc evidence. Git-unavailable is null with a reason. Core and full CI upload the complete directory plus compressed bundle, bundle hash, and manifest hash. | `src/results/provenance.rs`, writer/schema/profiles, workflow, README/result retention note | Seven result tests, 12 schema tests, and a real core profile pass; legacy v1 compatibility and Git-unavailable behavior are explicit. | Accepted for future CI retention. Permanent release URLs remain an authorized release action and were not fabricated; legacy committed summaries remain explicitly incomplete. |
| WAI-016 | Fixed: `check-quality.sh` is the one repository gate; `check-core.sh` composes sibling build/tests with it; `check-full.sh` requires pydcm, four validators, executable Orthanc, and the sibling before running the full profile. Policy and workflow name these exact gates. Ceremonial ponytail/magic-output and unenforced audit/license claims were replaced with explicit manual status. | quality doc, three scripts, workflow, README | Shell syntax passes; quality gate reached the known baseline optional-pydcm test mismatch, which was corrected to assert explicit unavailable semantics; its later cross-repository matrix blocker remains honest. | Accepted: scripts do not overlap subtly; mandatory absence fails full, and unexecuted manual checks are not claimed. |
| WAI-017 | Fixed: release calibrations cover disabled/default/fast/legacy sampling, representative probe and validator adapters, dense-reference versus sparse metrics, 1/8/32 MiB streamed DICOMweb round trips with fixed-buffer server I/O, and 32 MiB transactional publication. Every benchmark has warm-up, repeated samples, checksum/semantic assertions, caps, and disclosed whole-process RSS limits. | ignored calibration tests and ledger | Exact commands/tables below; short runner cells remain explicitly unqualified and no unmatched prior-run timing is used. | Accepted: overhead and limitations accompany every performance/memory claim. |
| WAI-018 | Fixed: matrix observations use strict v2 phases while derived top-level compatibility fields remain; run manifests now use strict schema v2 with provenance schema v1. A compatibility reader recognizes committed v1 manifests without fabricating absent provenance. Probe/conversion v1 remain unchanged. | matrix and run schemas, `src/schema.rs`, examples/tests | Matrix v2, manifest v2 negative-version, generated-manifest, and committed-v1 compatibility tests pass. | Accepted: all changed formats are versioned and old summaries remain readable with their limitations explicit. |

## Checkpoints and dependencies

1. **Measurement foundation** — WAI-001, WAI-002, WAI-013 command core, WAI-017. Required before
   trusting later runtime/RSS measurements.
2. **Sparse scientific metrics** — WAI-003. Depends on measurement foundation.
3. **Evidence and schemas** — WAI-004, WAI-005, WAI-011, WAI-018. Required before orchestration
   cleanup so failure semantics are fixed first.
4. **Transactional publication** — WAI-006 plus the result-owned portion of WAI-012.
5. **DICOMweb transport** — WAI-007 and WAI-008. Depends on bounded process/output policies.
6. **Profile composition** — WAI-009; publication occurs only after complete assembly.
7. **Matrix and comparator cohesion** — WAI-010 through WAI-013; preserve domain-specific logic.
8. **Public surface and validator cohesion** — WAI-012 through WAI-014.
9. **Provenance, artifact retention, and gates** — WAI-015, WAI-016, WAI-018.
10. **Final calibrated validation and deletion review** — rerun every available gate and audit all IDs.

## Decision log

- 2026-08-20 — Process monitoring uses a typed `ProcessSpec` with `OsString` arguments, optional
  sampling, and a 100 ms default interval. One initial inventory discovers already-started children;
  subsequent samples refresh only tracked PIDs. Wait polling is separately 10 ms.
- 2026-08-20 — stdout/stderr will each have explicit byte limits and metadata for retained,
  observed, and truncated content. JSON contract output is fail-closed when incomplete; diagnostic
  validator text may be visibly truncated but cannot qualify a known defect when incomplete.
- 2026-08-20 — Sparse runs will be sorted and canonicalized per segment/row, merging adjacent and
  overlapping same-segment intervals after validation. Area/intersection/centroid/overlap remain
  run-based. Topology/surface work uses bounded occupied crops and explicit resource status.
- 2026-08-20 — Phase evidence will be append-only within a case. Overall status is a deterministic
  reduction over typed phases; compatibility fields are derived in matrix observation schema v2.
- 2026-08-20 — Read-only SEG safe rejection accepts only `REWRITE_UNSUPPORTED` plus absence of an
  output artifact. Generic `OPERATION_FAILED` is intentionally a failed phase even when its message
  happens to mention unsupported input.
- 2026-08-20 — Results publish through a sibling incomplete staging directory, manifest last,
  verified before atomic rename. Final destinations are never partially visible.
- 2026-08-20 — Failed staging is automatically deleted by `TempDir`; an error never creates the
  final path. The standard library has no portable directory `rename_noreplace`, so a narrow race
  remains between the final existence check and `rename`; adding a platform dependency was rejected
  at this checkpoint because ordinary run roots are single-writer and the destination is rechecked.
- 2026-08-20 — DICOMweb uses streaming request/response paths supported by current ureq 3.4.0;
  one-object-per-STOW is acceptable if per-instance identity is rigorously parsed and study
  semantics remain intact. No new HTTP framework is planned.
- 2026-08-20 — STOW requests are chunked from a multipart header, the input `File`, and trailer;
  one request corresponds to one SOP Instance UID. STOW/QIDO retain at most 4 MiB per response.
  WADO stages at most 512 MiB by default and uses a streaming boundary parser with 8 KiB MIME-line
  and 64 KiB copy buffers. Published retrievals use no-clobber persistence only after validation.
- 2026-08-20 — Local Orthanc logs use the same concurrent bounded stream readers as other child
  processes with 4 MiB retained per stream. A startup retry is allowed only when startup/log text
  contains a constrained bind-collision signature; three isolated attempts bound the retry.
- 2026-08-20 — Existing v1 probe/conversion schemas remain intact. New study phase/provenance
  contracts receive new version identifiers and compatibility tests.
- 2026-08-20 — Public APIs are retained only for intentional external comparison/writer/validator
  use; test-only access moves to unit tests or narrow re-exports.
- Rejected: importing sibling production Rust, sharing the Python oracle implementation, a plugin
  framework, full-slide dense fallback in production, unbounded temporary memory, generic failure
  as expected rejection, and speculative dependencies.

## Validation ledger

See baseline table for commands already executed. Each later entry must include the exact command,
exit status, and relevant counts. Unavailable external tools are recorded as unverified, not passed.

- 2026-08-20 Phase 1 red: `cargo test --lib process::tests --locked` failed to compile because
  `ProcessSpec`, bounded captures, sampling metadata, and typed configuration errors did not exist.
- 2026-08-20 Phase 1 green: `cargo test --lib process::tests --locked` passed 10, ignored 1 explicit
  calibration test; `cargo test --test rust_probe --locked` passed 6; `cargo test --test
  rust_validators --locked` passed 7.
- 2026-08-20 Phase 1 quality: `cargo fmt --all -- --check` passed; `cargo clippy --all-targets
  --locked -- -D warnings` initially failed on large error variants, a 109-line probe executor, and
  excess observation booleans. Evidence was boxed/grouped and construction consolidated; the same
  Clippy command then passed.
- 2026-08-20 status/diff review: only task files listed by `git status --short`; no sibling/user
  files changed. Diff stat at checkpoint: 7 source/test files changed with 729 insertions and 119
  deletions, plus this new plan.
- 2026-08-20 Phase 2 red: `cargo test --test rust_metrics --locked` failed to compile because typed
  resource status, metric limits, optional advanced fields, and the limited entry point did not
  exist.
- 2026-08-20 Phase 2 green: `cargo test --test rust_metrics --locked` passed 13 and ignored 1
  explicit calibration; this includes 48 deterministic randomized two-segment cases checked against
  a separate per-pixel/brute-force oracle. `cargo test --test rust_matrix --locked` passed 2,
  including the complete available highdicom/viewer core matrix.
- 2026-08-20 Phase 2 quality: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked --
  -D warnings`, and `git diff --check` passed. `cargo test --all-targets --locked` again reached only
  the same baseline CLI failure (`primary_failure` null versus true) after 13 library tests passed
  and one calibration test was ignored.
- 2026-08-20 Phase 3 red: `cargo test --lib matrix::tests --locked` failed to compile before the
  typed `MatrixPhase`, `PhaseStatus`, `PhaseObservation`, and `CaseRecorder` existed.
- 2026-08-20 Phase 3 green: `cargo test --lib matrix::tests --locked` passed 4, covering a real
  subprocess late reference-output failure, append-only evidence, deterministic status derivation,
  and precise rejection/no-artifact semantics. `cargo test --test rust_schema --locked` passed 10,
  including the new v2 example and negative cases. `cargo clippy --all-targets --locked -- -D
  warnings` and `git diff --check` passed after evidence-size and construction cleanup.
- 2026-08-20 Phase 3 cross-repository gates: `cargo test --test rust_matrix --locked` passed the
  tampered-oracle test and failed the live matrix assertion only for `seg-labelmap` and
  `seg-fractional`; both retained evidence and reported code `OPERATION_FAILED` with the sibling
  message `unsupported input: only binary SEG export is supported`. `cargo test --test
  rust_profiles --locked` consequently failed `result.ok`. These are required contract failures,
  not unavailable tools and not reclassified passes.
- 2026-08-20 Phase 4 red: `cargo test --test rust_results --locked` passed the two legacy tests and
  failed three new transactional tests because `RunWriter::new` exposed the final directory.
- 2026-08-20 Phase 4 green: `cargo test --test rust_results --locked` passed 6 after staged
  publication. It covers invalid input before artifacts, a real figure-path failure, manifest-path
  failure, staging cleanup and same-ID retry, destination collision, successful manifest-last
  publication, and second publication rejection. `cargo clippy --all-targets --locked -- -D
  warnings`, `cargo fmt --all -- --check`, and `git diff --check` passed.
- 2026-08-20 Phase 4 cohesion: publication moved to `src/results/writer.rs` (195 lines), manifest
  verification/hash/sync to `src/results/manifest.rs` (152 lines), and the remaining table/figure
  code is 362 lines. Production `expect` calls were removed from the result path.
- 2026-08-20 Phase 5 red: the initial capped-log regression failed because the synthetic shell
  redirected stderr to `/dev/null`; after correcting the test, the former file-log design was
  replaced by bounded pipe readers. The collision-retry regression then failed with one launch
  versus the required three before retry implementation.
- 2026-08-20 Phase 5 green: `cargo test --test rust_orthanc --locked` passed 12 tests. These exercise
  loopback configuration, missing binary, 4 MiB log caps/truncation, three constrained collision
  attempts, streamed STOW/QIDO/WADO, per-instance success/failure, malformed STOW, 16-byte QIDO
  limit, 16-byte WADO limit, multipart success, invalid boundary, missing DICOM part, and no partial
  publication.
- 2026-08-20 Phase 5 quality: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked --
  -D warnings`, and the focused Orthanc test command all passed. Real Orthanc/plugin integration is
  unverified because no Orthanc executable is installed. Runtime observations now include log
  truncation booleans.
- 2026-08-20 Phase 6 red: `cargo test --lib profiles::tests --locked` failed to compile because the
  typed shared `BaselineStatus` did not exist.
- 2026-08-20 Phase 6 green: `cargo test --lib profiles::baseline::tests --locked` passed the typed
  matrix/conversion status truth table. `cargo test --test rust_profiles --locked` passed the real
  highdicom/viewer baseline, verifying 9 matrix, 6 conversion, one qualification observation,
  baseline definition version 1, and exact overall-status derivation even with the known sibling
  rewrite-code blocker.
- 2026-08-20 Phase 6 quality: `cargo fmt --all -- --check` and `cargo clippy --all-targets --locked
  -- -D warnings` passed. Profile-source search now finds exactly one call each to `generate_core`,
  `run_core_matrix`, and `run_conversion_matrices`, all in `profiles/baseline.rs`.
- 2026-08-20 Phase 7 refactor: existing typed evidence tests supplied the safety net because phase
  lifecycle consolidation intentionally does not change external behavior. `cargo test --lib
  matrix::tests --locked` passed 4 after ANN and SEG were routed through one `run_case` function
  parameterized by typed domain and rewrite policy. `CaseRecorder` and v2 observation ownership
  moved to `src/matrix/observation.rs`.
- 2026-08-20 Phase 7 live matrix: `cargo test --test rust_matrix --locked` passed the tampered-oracle
  test and again failed only the final complete-matrix assertion for `seg-labelmap` and
  `seg-fractional`, with exact `OPERATION_FAILED` evidence from the unchanged sibling. All ANN and
  writable binary SEG cases completed the shared lifecycle successfully.
- 2026-08-20 Phase 7 quality: `cargo clippy --all-targets --locked -- -D warnings` and `git diff
  --check` passed. The former separate `run_ann`/`run_seg` large-flow allowances were removed; the
  single deliberately linear runner retains one scoped line-count rationale.
- 2026-08-20 Phase 8 red: `cargo test --test rust_compare --locked` passed 7 and failed two new
  regressions because null ANN group entries and SEG entries without a number were silently skipped
  by `keyed`, allowing malformed documents to compare equal.
- 2026-08-20 Phase 8 green: the same command passed 9 after typed collection parsing began rejecting
  non-arrays, non-object entries, and missing/invalid keys with indexed errors. Canonical and native
  coordinate paths now use one `CoordinateSpec`-driven engine in `compare/geometry.rs`; polygon,
  ellipse, primitive-index, native, and canonical tests all exercise it.
- 2026-08-20 Phase 8 quality: `cargo clippy --all-targets --locked -- -D warnings` and `git diff
  --check` passed. Geometry matching is 321 lines, statistics 40, and the ANN/SEG comparison front
  is 456 lines. No comparator semantics were loosened.
- 2026-08-20 Phase 9 command boundary: `CommandSpec` now retains an `OsString` program and argument
  vector, derives bounded `ProcessSpec`, and renders `Vec<String>` only for JSON provenance. Probe,
  shim, validator file arguments, the production CLI, and Orthanc version execution no longer
  convert paths before invocation. Both old `path_text` helpers were removed.
- 2026-08-20 Phase 9 platform test: a Linux-only `ViewerProbe` invalid-byte filename regression was
  added. It cannot execute on this Darwin filesystem (`Illegal byte sequence` while creating the
  fixture), so it is cfg-gated; the lower-level Unix `OsString` runner regression executed and
  passed locally.
- 2026-08-20 Phase 9 validation: `cargo test --lib process::tests --locked` passed 10/ignored one
  calibration; `cargo test --test rust_probe --locked` passed 6; `cargo test --test rust_shim
  --locked` passed 1; `cargo test --test rust_validators --locked` passed 7; `cargo clippy
  --all-targets --locked -- -D warnings` and `git diff --check` passed.
- 2026-08-20 Phase 10 typing/decomposition: validator status is now a serde-stable enum with explicit
  passed, known-defect, unsupported, failed, timed-out, and unavailable variants. Specification,
  executable discovery, version execution/derivation, observation DTOs, and each of the PM, SEG,
  and SR exact known-defect policies have separate ownership. `validators.rs` retains only shared
  orchestration/output classification and is 288 lines.
- 2026-08-20 Phase 10 validation: `cargo test --test rust_validators --locked` passed 7 exact-policy
  tests, including changed-signature and truncated-output fail-closed cases; `cargo test --lib
  validators::tests --locked` passed the interpreter-derived version command; `cargo clippy
  --all-targets --locked -- -D warnings` and `git diff --check` passed.
- 2026-08-20 Phase 11 red: the result test failed to compile before `collect_provenance` existed and
  the legacy manifest test was added before compatibility handling. New writers now fail manifest
  publication if the strict schema-v2 contract does not validate.
- 2026-08-20 Phase 11 green: `cargo test --test rust_results --locked` passed 7, including
  Git-unavailable null-plus-reason and transactional v2 publication. `cargo test --test rust_schema
  --locked` passed 12, including generated v2 rejection as v1 and committed-v1 compatibility.
  `cargo test --test rust_profiles --locked` passed the real profile with harness/probe provenance.
- 2026-08-20 Phase 11 retention: both CI jobs now upload complete run directories. They also build a
  `.tar.gz`, bundle SHA-256, and manifest SHA-256 named with GitHub run ID/attempt. No permanent
  release URL was created because publishing a release is externally visible and not authorized;
  `results/README.md` states that legacy committed summaries are incomplete and does not pretend
  otherwise.
- 2026-08-20 Phase 11 quality: `cargo clippy --all-targets --locked -- -D warnings` and `git diff
  --check` passed after provenance/schema/profile changes.
- 2026-08-20 Phase 12 scripts/policy: added executable `check-quality.sh` and `check-full.sh`; reduced
  `check-core.sh` to sibling preparation plus composition of the quality script. Full requires all
  validators and executable Orthanc before work. CI full now calls the same script. Policy no
  longer claims ponytail, a magic review phrase, dependency audit/license, feature, Miri, or
  cross-target checks are automated when they are not.
- 2026-08-20 Phase 12 gate run: `bash -n scripts/check-quality.sh scripts/check-core.sh
  scripts/check-full.sh` passed. `./scripts/check-quality.sh` passed uv sync, 7 pytest tests, Ruff
  format/lint, Pyright, Rustfmt, 18 library tests (one calibration ignored), CLI help, then exposed
  the baseline optional-pydcm assertion (`capabilities.ann_read` null when pydcm is absent). The CLI
  regression was corrected to require explicit unqualified/unavailable data when absent and its
  exact capability checks when installed; focused rerun passed. The remaining all-target failure is
  the already-recorded exact sibling rewrite-code contract, not missing tooling.
- 2026-08-20 Phase 12 quality: after the test correction, the focused CLI test, `cargo clippy
  --all-targets --locked -- -D warnings`, `git diff --check`, and shell syntax all passed.
- 2026-08-20 Phase 13 release calibrations: sampler test passed after 37.81 s; representative probe
  and validator adapter calibrations passed; sparse calibration passed with whole-process RSS;
  DICOMweb 1/8/32 MiB streaming passed with fixed-buffer server I/O and 8.1 MiB whole-process RSS;
  transactional 32 MiB hashing/publication passed with 31.8 MiB whole-process RSS. Exact commands,
  samples, checksums, and limitations are in the benchmark ledger.
- 2026-08-20 Phase 14 deletion search: the only 1 ms/all-process loop is the ignored legacy
  calibration. Production takes one startup inventory and one timeout-termination inventory; normal
  samples refresh tracked PIDs. `read_to_end` is behind a 4,096-byte `take`; `Vec<bool>` occurs only
  in the explicitly budgeted tight crop. No `path_text`, `case_observation`, wildcard parent import,
  TODO/FIXME/placeholder, production `unwrap`, or production `expect` remains. Cargo/uv manifests
  and dependencies are unchanged.
- 2026-08-20 Phase 14 Python/focused gates: `uv sync --locked`, pytest 7, Ruff format/lint, Pyright,
  Rustfmt, Clippy warnings-denied, release build, and `git diff --check` passed. Focused post-matrix
  suites passed: metrics 13 (1 calibration ignored), Orthanc 12 (1 ignored), probe 6 (1 ignored),
  profiles 1, results 7 (1 ignored), scale 2, schema 12, shim 1, validators 7 (1 ignored).
- 2026-08-20 Phase 14 all-target gate: `cargo test --all-targets --locked` passed all preceding
  library/CLI/compare/conversion/ground-truth tests and failed only the live matrix assertion. The
  release probe at SHA `87aa73d8…` still reports generic `OPERATION_FAILED` for labelmap and
  fractional rewrite. No harness assertion was weakened.
- 2026-08-20 Phase 14 cross-repository gate: `./scripts/check-core.sh` passed 78 viewer-core tests
  (1 ignored), 19 annotation_probe tests, build, Python gates, and harness tests through the live
  matrix. Rebuilding the dirty sibling debug probe exposed a second exact contract problem:
  `seg-labelmap` inspection fails because automatic/semiautomatic annotations lack algorithm
  identification; `seg-fractional` still uses generic `OPERATION_FAILED`. This differs from the
  older release-probe binary and is retained as source/binary provenance, not normalized away.
- 2026-08-20 Phase 14 available full run: release `run-full` with edition 2026c and no Orthanc
  published schema-v2 manifest SHA-256
  `1068e182dddebd3ceff95b191ca0766d608595c908327209d107605f4934ab42`. It recorded 79 passed, 16
  known-validator-defect, 11 unsupported, 2 failed matrix, 1 unqualified pydcm, and 1 unavailable
  Orthanc observation: conversion 6/6 and scale 5/5 passed; validator outcomes were 61 passed, 16
  exact qualified defects, 11 unsupported. The 74 MiB temporary run was moved to Trash after
  inspection and is recoverable there.
- 2026-08-21 deletion review: applied all seven ponytail findings by removing duplicate matrix
  aggregation, enum-to-string compatibility layers, a redundant conversion argument and crop pass,
  a one-use byte-search wrapper, an impossible provenance error channel, and superseded benchmark
  tables. Affected focused suites, Rustfmt, Clippy with warnings denied, release build, and diff
  checks passed. The all-target run again failed only at the unchanged sibling LABELMAP inspection
  and FRACTIONAL rewrite-code contracts. Current tracked stat is 38 files changed with 4,377
  insertions and 2,527 deletions, plus the new plan/schema/script/module files.

## Benchmark ledger

Methodology fixed before optimization:

- Build: `cargo build --release --locked`; benchmark the release executable/test harness.
- Machine: Apple M4 Pro, 48 GiB, arm64 Darwin 25.5.0; normal interactive load disclosed.
- Inputs: deterministic no-op and sleeps near 10 ms, 100 ms, 1 s; representative probe/validator;
  deterministic sparse run sets with recorded dimensions/run counts; increasing DICOM object sizes.
- Runs: at least one warm-up and enough measured repetitions to report median plus min/max or an
  interquartile spread. Exact repetitions will be recorded with results.
- Sampler: disabled, chosen default, and deliberately fast interval; output caps fixed and reported.
- Memory: use process RSS metadata where sampled and an external peak-memory mechanism when locally
  available. Never treat unsampled memory as zero.
- Calibration: measure harness/process-start overhead separately; do not call short-process timing
  precise when overhead is a material fraction.
- Before/after: retain a baseline implementation benchmark only on safe small inputs and ensure
  equivalent work/checksum. Large sparse cases are new-only where dense allocation is unsafe.

### Final sampler and adapter calibration — 2026-08-20

Final sampler command: `cargo test --release --lib
process::tests::calibrates_sampler_overhead_against_legacy_loop --locked -- --ignored --nocapture`.
One warm-up and 7 samples; medians in microseconds `[min, max]`, RSS in bytes. The complete rerun
passed in 37.81 s.

| Workload | Disabled | 100 ms default | 10 ms fast | Legacy all/1 ms |
|---|---:|---:|---:|---:|
| no-op | 15,041 [10,440, 15,096], RSS 0 | 5,967 [5,386, 7,227], RSS 475,136 | 5,966 [5,637, 6,628], RSS 475,136 | 6,192 [5,785, 6,302] |
| sleep 10 ms | 27,980 [25,764, 30,077], RSS 0 | 23,075 [21,244, 34,500], RSS 589,824 | 23,389 [21,207, 34,545], RSS 475,136 | 23,959 [22,149, 27,875] |
| sleep 100 ms | 125,767 [116,347, 134,919], RSS 0 | 126,782 [115,386, 137,850], RSS 622,592 | 124,655 [119,953, 129,743], RSS 1,196,032 | 115,414 [106,378, 118,318] |
| sleep 1 s | 1,024,104 [1,017,886, 1,032,588], RSS 0 | 1,025,798 [1,021,910, 1,035,390], RSS 1,196,032 | 1,026,339 [1,014,698, 1,034,168], RSS 1,196,032 | 1,015,172 [1,005,994, 1,019,741] |

The 10 ms wait poll and process startup dominate short cells; neither the 6–28 ms no-op/10 ms
values nor their ordering are precision claims. At longer durations the key improvement remains
structural: the new sampler does not repeatedly inventory all system processes.

Representative commands:

- `cargo test --release --test rust_probe --locked -- --ignored --exact
  representative_probe_adapter_calibration --nocapture`: 7 samples after warm-up, median 7.283 ms
  [7.058, 9.209], with full JSON parsing and schema validation asserted.
- `cargo test --release --test rust_validators --locked -- --ignored --exact
  representative_validator_adapter_calibration --nocapture`: 7 samples after warm-up, median
  6.655 ms [6.049, 7.154], with 200 diagnostic lines and status/content asserted.

### Final sparse, DICOMweb, and publication calibration — 2026-08-20

The final sparse command repeated the earlier method and passed: dense 32 × 32 median 679 µs
[675, 823], sparse equivalent 399 µs [384, 674], huge two-run slide 11 µs [9, 13], and explicit
wide-crop resource result 0 µs [0, 1]. Checksums were 366/366/7/2; whole test RSS was 7,405,568
bytes. The dense huge-slide case remains deliberately unexecuted.

DICOMweb command: `cargo test --release --test rust_orthanc --locked --no-run`, then
`/usr/bin/time -l target/release/deps/rust_orthanc-b1dafec65582664e --ignored --exact
dicomweb_streaming_calibration --nocapture`. One warm-up and 3 samples per size; both STOW and WADO
stream, the server discards/produces data with fixed buffers, and retrieved length/equality is
asserted.

| Object | Median ms | Range ms |
|---:|---:|---:|
| 1 MiB | 8.091 | 7.090–9.781 |
| 8 MiB | 16.608 | 16.363–18.027 |
| 32 MiB | 37.084 | 36.543–37.189 |

The entire same-process client/server calibration peaked at 8,060,928 bytes RSS and 2,589,032 bytes
Darwin peak footprint. This is an upper bound contaminated by both endpoints, but its essentially
fixed memory across a 32 MiB object demonstrates there is no extra whole-object client copy.

Publication command: `/usr/bin/time -l target/release/deps/rust_results-2ecc202131b1e981 --ignored
--exact transactional_publication_calibration --nocapture`. A sparse 32 MiB artifact was staged,
hashed, schema-validated, synced, and renamed; one warm-up plus 3 samples yielded median 128.638 ms
[127.713, 130.020]. Whole-process RSS was 31,834,112 bytes. This includes test/provenance/schema
runtime and filesystem cache effects; it shows no second 32 MiB in-memory artifact copy, not a
portable storage-throughput claim.

## Completion status

The harness hardening implementation is complete, and the focused harness checks recorded above
passed. The all-target and core gates did not pass because the sibling `annotation_probe` still
violates the LABELMAP/FRACTIONAL SEG contracts. The real Orthanc full gate remains unverified
because an Orthanc executable was unavailable locally. These external gates must not be reported as
passed until they execute successfully.
