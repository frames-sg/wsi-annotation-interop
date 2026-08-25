#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
harness_root="$(cd "${script_directory}/.." && pwd)"
viewer_root="${DICOM_VIEWER_ROOT:-$(cd "${harness_root}/../dicom-viewer" && pwd)}"
orthanc_executable="${ORTHANC_EXECUTABLE:-}"
run_id="${RUN_ID:-full-$(date -u +%Y%m%dT%H%M%SZ)}"

if [[ -z "${orthanc_executable}" || ! -x "${orthanc_executable}" ]]; then
  echo "full gate requires executable ORTHANC_EXECUTABLE" >&2
  exit 2
fi
for validator in validate_iods dciodvfy dcentvfy dcm2json; do
  if ! command -v "${validator}" >/dev/null 2>&1; then
    echo "full gate requires ${validator} on PATH" >&2
    exit 2
  fi
done

UV_SYNC_EXTRA=pydcm DICOM_VIEWER_ROOT="${viewer_root}" "${script_directory}/check-core.sh"
cargo build --manifest-path "${viewer_root}/Cargo.toml" \
  -p dicom-viewer --bin annotation_probe --release --locked

cd "${harness_root}"
target/release/wsi-annotation-interop run-full \
  --probe "${viewer_root}/target/release/annotation_probe" \
  --orthanc "${orthanc_executable}" \
  --results results \
  --run-id "${run_id}"
