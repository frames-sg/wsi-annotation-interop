use std::collections::BTreeSet;
use std::fs;

use serde_json::{Value, json};
use tempfile::tempdir;
use wsi_annotation_interop::results::RunWriter;

#[test]
fn run_writer_creates_an_exclusive_checksummed_run_with_tables_and_figures() {
    let directory = tempdir().unwrap();
    let mut writer =
        RunWriter::new(directory.path(), "run-001", json!({"profile": "core"})).unwrap();
    writer
        .write_observations(&[json!({
            "case_id": "ann-2d",
            "status": "passed",
            "coordinate_error_max_px": 0.0,
            "dice": 1.0,
            "runtime_ms": 12.0,
            "peak_rss_bytes": 4096,
            "mask_metrics": {"dice": 1.0, "expected_holes": 1, "actual_holes": 1},
        })])
        .unwrap();
    let manifest_path = writer.finalize().unwrap();

    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["run_id"], "run-001");
    assert_eq!(manifest["metadata"], json!({"profile": "core"}));
    assert!(manifest["artifacts"].as_array().unwrap().len() >= 8);
    assert!(writer.path().join("observations.jsonl").is_file());
    assert!(writer.path().join("observations.csv").is_file());
    assert!(writer.path().join("summary.json").is_file());
    assert!(writer.path().join("summary.csv").is_file());
    let figures: BTreeSet<_> = fs::read_dir(writer.path().join("figures"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        figures,
        BTreeSet::from([
            "coordinate-error.svg".to_owned(),
            "mask-metrics.svg".to_owned(),
            "memory.svg".to_owned(),
            "runtime.svg".to_owned(),
        ])
    );
    let mask_figure = fs::read_to_string(writer.path().join("figures/mask-metrics.svg")).unwrap();
    for title in [
        "Dice",
        "Area difference",
        "Centroid distance",
        "HD95",
        "ASSD",
        "Overlap difference",
        "Expected components",
        "Expected holes",
    ] {
        assert!(mask_figure.contains(title), "missing {title} panel");
    }
    assert!(
        RunWriter::new(directory.path(), "run-001", json!({}))
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
}

#[test]
fn run_writer_rejects_unsafe_identifiers_and_second_writes() {
    let directory = tempdir().unwrap();
    assert!(RunWriter::new(directory.path(), "../escape", json!({})).is_err());
    let mut writer = RunWriter::new(directory.path(), "safe", json!({})).unwrap();
    writer
        .write_observations(&[json!({"status": "passed"})])
        .unwrap();

    assert!(
        writer
            .write_observations(&[json!({"status": "passed"})])
            .unwrap_err()
            .to_string()
            .contains("already")
    );
}
