use std::env;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn rust_cli_owns_help_and_usage_exit_codes() {
    let executable = env!("CARGO_BIN_EXE_wsi-annotation-interop");

    let help = Command::new(executable).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout)
            .contains("Neutral DICOM WSI ANN/SEG/SR/PM interoperability harness")
    );

    let invalid = Command::new(executable)
        .arg("not-a-command")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));

    let full_help = Command::new(executable)
        .args(["run-full", "--help"])
        .output()
        .unwrap();
    assert!(full_help.status.success());
    assert!(String::from_utf8_lossy(&full_help.stdout).contains("--dicom-edition"));
}

#[test]
fn rust_cli_runs_core_and_reports_pydcm_as_nonprimary() {
    let executable = env!("CARGO_BIN_EXE_wsi-annotation-interop");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    let shim = root.join("shim/reference_shim.py");
    let probe = env::var_os("ANNOTATION_PROBE").map_or_else(
        || root.join("../dicom-viewer/target/debug/annotation_probe"),
        PathBuf::from,
    );
    assert!(python.is_file(), "run `uv sync --locked` first");
    assert!(
        probe.is_file(),
        "build annotation_probe or set ANNOTATION_PROBE"
    );

    let qualification = Command::new(executable)
        .args(["--reference-python", python.to_str().unwrap()])
        .args(["--reference-shim", shim.to_str().unwrap()])
        .arg("qualify-pydcm")
        .output()
        .unwrap();
    assert!(qualification.status.success());
    let qualification: Value = serde_json::from_slice(&qualification.stdout).unwrap();
    assert_eq!(qualification["implementation"], "pydcm");
    assert_eq!(qualification["primary_failure"], false);
    assert_eq!(qualification["capabilities"]["ann_read"], true);
    assert_eq!(qualification["capabilities"]["ann_write"], true);
    assert_eq!(qualification["capabilities"]["seg_read"], false);
    assert!(
        qualification["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| {
                reason
                    .as_str()
                    .is_some_and(|text| text.contains("no foreground labels"))
            })
    );

    let directory = tempdir().unwrap();
    let core = Command::new(executable)
        .args(["--reference-python", python.to_str().unwrap()])
        .args(["--reference-shim", shim.to_str().unwrap()])
        .arg("run-core")
        .args(["--probe", probe.to_str().unwrap()])
        .args(["--results", directory.path().to_str().unwrap()])
        .args(["--run-id", "cli-rust-core"])
        .output()
        .unwrap();
    assert!(
        core.status.success(),
        "{}",
        String::from_utf8_lossy(&core.stderr)
    );
    let report: Value = serde_json::from_slice(&core.stdout).unwrap();
    assert_eq!(report["status"], "passed");
    assert!(PathBuf::from(report["manifest"].as_str().unwrap()).is_file());
}
