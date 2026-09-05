use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::tempdir;
use wsi_annotation_interop::probe::ViewerProbe;
use wsi_annotation_interop::shim::{FixtureSet, ReferenceShim};
use wsi_annotation_interop::{build_core_ground_truth, run_core_matrix};

#[test]
fn rust_matrix_rejects_tampered_ground_truth_before_running_tools() {
    for mutate in [
        |truth: &mut serde_json::Value| {
            truth["cases"].as_object_mut().unwrap().remove("ann-3d-xyz");
        },
        |truth: &mut serde_json::Value| truth["schema_version"] = serde_json::json!(2),
    ] {
        let directory = tempdir().unwrap();
        let ground_truth = directory.path().join("ground-truth-v1.json");
        let mut truth = build_core_ground_truth();
        mutate(&mut truth);
        fs::write(&ground_truth, serde_json::to_vec(&truth).unwrap()).unwrap();
        let fixtures = FixtureSet {
            source: PathBuf::new(),
            pyramid_source: PathBuf::new(),
            pyramid_ann: PathBuf::new(),
            reordered_seg: PathBuf::new(),
            pm: PathBuf::new(),
            sr: PathBuf::new(),
            sr_seg: PathBuf::new(),
            ground_truth,
            ann: BTreeMap::default(),
            seg: BTreeMap::default(),
        };
        let reference =
            ReferenceShim::new(vec!["/bin/false".to_owned()], Duration::from_secs(1)).unwrap();
        let probe = ViewerProbe::new(vec!["/bin/false".to_owned()], None).unwrap();

        let error = run_core_matrix(
            &fixtures,
            &reference,
            &probe,
            &directory.path().join("output"),
        )
        .unwrap_err();

        assert!(error.contains("differs from the Rust declarative oracle"));
    }
}

#[test]
#[ignore = "requires the external annotation_probe built by scripts/check-core.sh"]
fn rust_runs_the_complete_highdicom_viewer_core_matrix() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    assert!(
        python.is_file(),
        "run `uv sync --locked` before the cross-language matrix test"
    );
    let probe_path = env::var_os("ANNOTATION_PROBE").map_or_else(
        || root.join("../dicom-viewer/target/debug/annotation_probe"),
        PathBuf::from,
    );
    assert!(
        probe_path.is_file(),
        "build annotation_probe or set ANNOTATION_PROBE"
    );
    let shim = ReferenceShim::new(
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
    let fixtures = shim
        .generate_core(&directory.path().join("fixtures"))
        .unwrap();

    let result = run_core_matrix(
        &fixtures,
        &shim,
        &probe,
        &directory.path().join("roundtrips"),
    )
    .unwrap();

    assert!(
        result.is_ok(),
        "{}",
        result
            .observations
            .iter()
            .map(|observation| format!("{}: {}", observation.case_id, observation.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(result.observations.len(), 9);
    assert!(
        result
            .observations
            .iter()
            .all(|observation| observation.inspect_equal)
    );
    assert!(
        result
            .observations
            .iter()
            .filter(|observation| matches!(
                observation.case_id.as_str(),
                "seg-labelmap" | "seg-fractional"
            ))
            .all(|observation| observation.rewrite_rejected)
    );
    let pyramid = result
        .observations
        .iter()
        .find(|observation| observation.case_id == "ann-2d-volume-level1")
        .unwrap();
    assert_eq!(pyramid.pyramid_level, 1);
    assert_eq!(pyramid.coordinate_error_max_px, Some(0.0));
    let binary = result
        .observations
        .iter()
        .find(|observation| observation.case_id == "seg-binary")
        .unwrap();
    assert_eq!(binary.dice, Some(1.0));
}
