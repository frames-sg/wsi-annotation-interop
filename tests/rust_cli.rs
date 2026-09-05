use std::env;
use std::fs;
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
fn rust_cli_reports_pydcm_as_nonprimary() {
    let executable = env!("CARGO_BIN_EXE_wsi-annotation-interop");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    let shim = root.join("shim/reference_shim.py");
    assert!(python.is_file(), "run `uv sync --locked` first");

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
    if qualification["version"].is_null() {
        assert_eq!(qualification["qualified"], false);
        assert_eq!(qualification["capabilities"], serde_json::json!({}));
        assert!(
            qualification["reasons"][0]
                .as_str()
                .is_some_and(|reason| reason.contains("not installed"))
        );
    } else {
        assert_eq!(qualification["capabilities"]["ann_read"], true);
        assert_eq!(qualification["capabilities"]["ann_write"], true);
        assert_eq!(qualification["capabilities"]["seg_read"], false);
        assert!(
            qualification["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason
                    .as_str()
                    .is_some_and(|text| text.contains("no foreground labels")))
        );
    }
}

#[test]
#[ignore = "requires the external annotation_probe built by scripts/check-core.sh"]
fn rust_cli_runs_core_with_external_annotation_probe() {
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
    assert!(matches!(core.status.code(), Some(0 | 1)));
    let report: Value = serde_json::from_slice(&core.stdout).unwrap();
    let manifest = PathBuf::from(report["manifest"].as_str().unwrap());
    assert!(manifest.is_file());
    assert_eq!(report["status"] == "passed", core.status.success());
    if !core.status.success() {
        let observations =
            fs::read_to_string(manifest.parent().unwrap().join("observations.jsonl")).unwrap();
        let failed = observations
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|observation| observation["status"] == "failed")
            .collect::<Vec<_>>();
        assert_eq!(
            failed
                .iter()
                .map(|observation| observation["case_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["seg-labelmap", "seg-fractional"]
        );
        assert!(failed.iter().all(|observation| {
            observation["phases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|phase| phase["error_code"] == "OPERATION_FAILED")
        }));
    }
}
