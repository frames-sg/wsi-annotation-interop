use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tempfile::tempdir;
use wsi_annotation_interop::probe::ViewerProbe;
use wsi_annotation_interop::run_core_profile;
use wsi_annotation_interop::shim::ReferenceShim;

#[test]
fn rust_core_profile_writes_matrix_and_nonprimary_qualification_results() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    let probe_path = env::var_os("ANNOTATION_PROBE").map_or_else(
        || root.join("../dicom-viewer/target/debug/annotation_probe"),
        PathBuf::from,
    );
    assert!(python.is_file(), "run `uv sync --locked` first");
    assert!(
        probe_path.is_file(),
        "build annotation_probe or set ANNOTATION_PROBE"
    );
    let reference = ReferenceShim::new(
        vec![
            python.to_string_lossy().into_owned(),
            root.join("shim/reference_shim.py")
                .to_string_lossy()
                .into_owned(),
        ],
        Duration::from_mins(10),
    )
    .unwrap();
    let probe = ViewerProbe::new(
        vec![probe_path.to_string_lossy().into_owned()],
        Some(Duration::from_mins(10)),
    )
    .unwrap();
    let directory = tempdir().unwrap();

    let result = run_core_profile(&reference, &probe, directory.path(), "rust-core-test").unwrap();

    assert!(result.manifest.is_file());
    let manifest: Value = serde_json::from_slice(&fs::read(&result.manifest).unwrap()).unwrap();
    assert_eq!(
        manifest["metadata"]["reference"]["packages"]["highdicom"],
        "0.28.1"
    );
    assert_eq!(manifest["metadata"]["baseline_definition_version"], 1);
    assert!(
        manifest["metadata"]["reference"]["packages"]
            .as_object()
            .unwrap()
            .contains_key("pydcm")
    );
    let observations: Vec<Value> =
        fs::read_to_string(result.manifest.parent().unwrap().join("observations.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(
        observations
            .iter()
            .filter(|item| item["phase"] == "matrix")
            .count(),
        9
    );
    assert_eq!(
        observations
            .iter()
            .filter(|item| item["phase"] == "conversion")
            .count(),
        6
    );
    let qualification = observations
        .iter()
        .find(|item| item["phase"] == "qualification")
        .unwrap();
    assert_eq!(qualification["primary_failure"], false);
    assert!(matches!(
        qualification["status"].as_str(),
        Some("qualified" | "unqualified")
    ));
    assert_eq!(
        result.ok,
        observations.iter().all(|item| {
            item["phase"] == "qualification" || item["status"].as_str() == Some("passed")
        })
    );
}
