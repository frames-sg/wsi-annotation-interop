use std::collections::BTreeSet;
use std::fs;

use serde_json::{Value, json};
use tempfile::tempdir;
use wsi_annotation_interop::results::{RunWriter, collect_provenance};

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
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(manifest["run_id"], "run-001");
    assert_eq!(manifest["metadata"], json!({"profile": "core"}));
    assert_eq!(manifest["provenance"]["schema_version"], 1);
    assert!(manifest["provenance"]["repositories"]["harness"]["sha"].is_string());
    assert!(manifest["provenance"]["build"]["cargo_lock_sha256"].is_string());
    assert!(manifest["provenance"]["build"]["executable_sha256"].is_string());
    assert_eq!(manifest["provenance"]["study"]["matrix_schema_version"], 2);
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
fn git_unavailable_provenance_is_unknown_with_a_reason() {
    let directory = tempdir().unwrap();

    let provenance = collect_provenance(directory.path(), None);
    let value = serde_json::to_value(provenance).unwrap();

    assert!(value["repositories"]["harness"]["sha"].is_null());
    assert!(
        value["repositories"]["harness"]["unknown_reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
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

#[test]
fn final_directory_is_hidden_until_atomic_publication() {
    let directory = tempdir().unwrap();
    let mut writer = RunWriter::new(directory.path(), "transactional", json!({})).unwrap();
    let final_path = directory.path().join("transactional");

    assert!(!final_path.exists());
    assert_ne!(writer.path(), final_path);
    writer
        .write_observations(&[json!({"case_id": "case", "status": "passed"})])
        .unwrap();
    assert!(!final_path.exists());

    let manifest = writer.finalize().unwrap();
    assert_eq!(manifest, final_path.join("manifest.json"));
    assert!(final_path.is_dir());
}

#[test]
fn staged_failure_leaves_no_final_directory_and_same_run_id_can_retry() {
    let directory = tempdir().unwrap();
    let final_path = directory.path().join("retryable");
    {
        let mut writer = RunWriter::new(directory.path(), "retryable", json!({})).unwrap();
        fs::write(writer.path().join("figures"), b"blocks figure directory").unwrap();
        assert!(
            writer
                .write_observations(&[json!({"case_id": "case", "status": "passed"})])
                .is_err()
        );
        assert!(!final_path.exists());
    }
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());

    let mut retry = RunWriter::new(directory.path(), "retryable", json!({})).unwrap();
    retry
        .write_observations(&[json!({"case_id": "case", "status": "passed"})])
        .unwrap();
    retry.finalize().unwrap();
    assert!(final_path.join("manifest.json").is_file());
}

#[test]
fn invalid_observation_fails_before_any_final_artifact_is_visible() {
    let directory = tempdir().unwrap();
    {
        let mut writer = RunWriter::new(directory.path(), "invalid", json!({})).unwrap();
        assert!(
            writer
                .write_observations(&[json!("not an object")])
                .is_err()
        );
        assert!(!directory.path().join("invalid").exists());
        assert!(fs::read_dir(writer.path()).unwrap().next().is_none());
    }
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
}

#[test]
fn manifest_failure_and_destination_collision_never_publish_partial_results() {
    let directory = tempdir().unwrap();
    let manifest_final = directory.path().join("manifest-failure");
    {
        let mut writer = RunWriter::new(directory.path(), "manifest-failure", json!({})).unwrap();
        writer
            .write_observations(&[json!({"case_id": "case", "status": "passed"})])
            .unwrap();
        fs::create_dir(writer.path().join("manifest.json")).unwrap();
        assert!(writer.finalize().is_err());
        assert!(!manifest_final.exists());
    }

    let mut writer = RunWriter::new(directory.path(), "collision", json!({})).unwrap();
    writer
        .write_observations(&[json!({"case_id": "case", "status": "passed"})])
        .unwrap();
    let collision = directory.path().join("collision");
    fs::create_dir(&collision).unwrap();
    assert!(writer.finalize().is_err());
    assert!(fs::read_dir(&collision).unwrap().next().is_none());
}

#[test]
#[ignore = "publication calibration; run in release mode with /usr/bin/time -l"]
fn transactional_publication_calibration() {
    let mut samples = Vec::new();
    for repetition in 0..4 {
        let directory = tempdir().unwrap();
        let mut writer = RunWriter::new(
            directory.path(),
            &format!("publication-{repetition}"),
            json!({"profile": "core"}),
        )
        .unwrap();
        fs::File::create(writer.path().join("large-artifact.dcm"))
            .unwrap()
            .set_len(32 * 1024 * 1024)
            .unwrap();
        writer
            .write_observations(&[json!({"case_id": "case", "status": "passed"})])
            .unwrap();
        let started = std::time::Instant::now();
        let manifest = writer.finalize().unwrap();
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert!(manifest.is_file());
        assert_eq!(
            fs::metadata(writer.path().join("large-artifact.dcm"))
                .unwrap()
                .len(),
            32 * 1024 * 1024
        );
        if repetition > 0 {
            samples.push(elapsed);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "publication artifact_mib=32 median_ms={:.3} min_ms={:.3} max_ms={:.3}",
        samples[1], samples[0], samples[2]
    );
}
