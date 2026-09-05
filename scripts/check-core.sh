#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
harness_root="$(cd "${script_directory}/.." && pwd)"
annotations_root="${WSI_DICOM_ANNOTATIONS_ROOT:-$(cd "${harness_root}/../wsi-dicom-annotations" && pwd)}"
probe_path="${annotations_root}/target/debug/annotation_probe"

test -f "${annotations_root}/Cargo.toml"

cargo test --manifest-path "${annotations_root}/Cargo.toml" -p wsi-dicom-annotations --locked
cargo test --manifest-path "${annotations_root}/Cargo.toml" \
  -p wsi-annotation-probe --bin annotation_probe --locked
cargo build --manifest-path "${annotations_root}/Cargo.toml" \
  -p wsi-annotation-probe --bin annotation_probe --locked

ANNOTATION_PROBE="${probe_path}" "${script_directory}/check-quality.sh"

cd "${harness_root}"

run_external_test() {
  local test_target="$1"
  local test_name="$2"
  ANNOTATION_PROBE="${probe_path}" cargo test --locked \
    --test "${test_target}" "${test_name}" -- --exact --ignored
}

run_external_test rust_cli rust_cli_runs_core_with_external_annotation_probe
run_external_test rust_matrix rust_runs_the_complete_highdicom_viewer_core_matrix
run_external_test rust_conversion_matrix \
  rust_runs_separate_geojson_sr_and_parametric_map_conversion_matrices
run_external_test rust_profiles \
  rust_core_profile_writes_matrix_and_nonprimary_qualification_results
run_external_test rust_scale rust_runs_a_scale_roundtrip_with_digest_payloads
