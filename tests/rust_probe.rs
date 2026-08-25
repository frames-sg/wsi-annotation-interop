#![cfg(unix)]

#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wsi_annotation_interop::probe::{
    GeoJsonCoordinateSpace, GeoJsonTarget, PayloadMode, RasterChannels, ViewerProbe,
};

fn shell_probe(script: String, timeout: Duration) -> ViewerProbe {
    ViewerProbe::new(
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            script,
            "annotation-probe-test".to_owned(),
        ],
        Some(timeout),
    )
    .unwrap()
}

fn print_json(value: &Value, delay: bool) -> String {
    let json = serde_json::to_string(value).unwrap();
    let delay = if delay { "sleep 0.05; " } else { "" };
    format!("{delay}printf '%s\\n' '{json}'")
}

#[test]
fn viewer_probe_validates_success_and_records_external_peak_rss() {
    let report: Value = serde_json::from_str(include_str!("data/ann-report.json")).unwrap();
    let probe = shell_probe(print_json(&report, true), Duration::from_secs(2));

    let observation = probe
        .inspect(
            Path::new("source.dcm"),
            Path::new("ann.dcm"),
            None,
            PayloadMode::Digest,
        )
        .unwrap();

    assert_eq!(observation.report["status"], "ok");
    assert_eq!(observation.command.last().unwrap(), "ann.dcm");
    assert!(observation.command.iter().any(|part| part == "--payload"));
    assert!(observation.peak_rss_bytes > 0);
    assert!(observation.rss_sampled);
    assert_eq!(observation.sample_interval_ms, Some(100));
    assert!(!observation.stderr_truncated);
    assert!(observation.stdout_total_bytes > 0);
}

#[test]
fn viewer_probe_returns_a_structured_failure() {
    let report = json!({
        "schema_version": 1,
        "status": "error",
        "operation": "inspect",
        "implementation": {"name": "dicom-viewer", "version": "0.1.0"},
        "error": {"code": "OPERATION_FAILED", "message": "bad annotation"}
    });
    let probe = shell_probe(
        format!("{}; exit 1", print_json(&report, false)),
        Duration::from_secs(2),
    );

    let error = probe
        .inspect(
            Path::new("source.dcm"),
            Path::new("ann.dcm"),
            None,
            PayloadMode::Digest,
        )
        .unwrap_err();

    assert!(error.to_string().contains("bad annotation"));
    assert_eq!(error.report.unwrap()["error"]["code"], "OPERATION_FAILED");
}

#[test]
fn viewer_probe_terminates_a_timed_out_process_tree() {
    let probe = shell_probe("sleep 60".to_owned(), Duration::from_millis(50));
    let started = Instant::now();

    let error = probe
        .inspect(
            Path::new("source.dcm"),
            Path::new("ann.dcm"),
            None,
            PayloadMode::Digest,
        )
        .unwrap_err();

    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn viewer_probe_rejects_truncated_json() {
    let probe = shell_probe("printf '{'".to_owned(), Duration::from_secs(2));

    let error = probe
        .inspect(
            Path::new("source.dcm"),
            Path::new("ann.dcm"),
            None,
            PayloadMode::Digest,
        )
        .unwrap_err();

    assert!(error.to_string().contains("invalid JSON"));
}

#[test]
fn viewer_probe_owns_geojson_conversion_arguments_and_schema() {
    let report: Value =
        serde_json::from_str(include_str!("../examples/conversion-report-v1.json")).unwrap();
    let probe = shell_probe(print_json(&report, false), Duration::from_secs(2));

    let observation = probe
        .convert_geojson_bundle(
            Path::new("source.dcm"),
            Some(Path::new("canonical.dcm")),
            Path::new("mapping.json"),
            GeoJsonCoordinateSpace::Level0Pixels,
            &[GeoJsonTarget::Ann, GeoJsonTarget::Seg, GeoJsonTarget::Sr],
            Path::new("bundle"),
            Path::new("annotations.geojson"),
            false,
        )
        .unwrap();

    assert_eq!(observation.report["operation"], "convert-geojson");
    assert_eq!(
        observation.command,
        [
            "/bin/sh",
            "-c",
            observation.command[2].as_str(),
            "annotation-probe-test",
            "convert-geojson",
            "--source",
            "source.dcm",
            "--canonical-source",
            "canonical.dcm",
            "--mapping",
            "mapping.json",
            "--coordinate-space",
            "level0-pixels",
            "--target",
            "ann",
            "--target",
            "seg",
            "--target",
            "sr",
            "--output-dir",
            "bundle",
            "annotations.geojson",
        ]
    );
}

#[test]
fn viewer_probe_rejects_a_conversion_report_for_the_wrong_operation() {
    let report: Value =
        serde_json::from_str(include_str!("../examples/conversion-report-v1.json")).unwrap();
    let probe = shell_probe(print_json(&report, false), Duration::from_secs(2));

    let error = probe
        .convert_raster_bundle(
            Path::new("source.dcm"),
            None,
            Path::new("profile.json"),
            RasterChannels::Auto,
            Path::new("maps"),
            None,
            Path::new("probability.npy"),
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("reported operation convert-geojson")
    );
}

#[test]
#[ignore = "adapter calibration; run in release mode"]
fn representative_probe_adapter_calibration() {
    let report: Value = serde_json::from_str(include_str!("data/ann-report.json")).unwrap();
    let probe = shell_probe(print_json(&report, false), Duration::from_secs(2));
    let mut samples = Vec::new();
    for repetition in 0..8 {
        let observation = probe
            .inspect(
                Path::new("source.dcm"),
                Path::new("ann.dcm"),
                None,
                PayloadMode::Full,
            )
            .unwrap();
        assert_eq!(observation.report["status"], "ok");
        if repetition > 0 {
            samples.push(observation.elapsed_ms);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "probe-adapter repetitions=7 median_ms={:.3} min_ms={:.3} max_ms={:.3}",
        samples[3], samples[0], samples[6]
    );
}

#[test]
#[cfg(target_os = "linux")]
fn viewer_probe_invokes_non_utf8_paths_without_loss() {
    let directory = tempfile::tempdir().unwrap();
    let annotation = directory
        .path()
        .join(PathBuf::from(OsString::from_vec(b"ann-\xff.dcm".to_vec())));
    fs::write(&annotation, b"dicom").unwrap();
    let report: Value = serde_json::from_str(include_str!("data/ann-report.json")).unwrap();
    let script = format!(
        "for last do :; done; test -f \"$last\" || exit 9; {}",
        print_json(&report, false)
    );
    let probe = ViewerProbe::from_program(
        OsString::from("/bin/sh"),
        vec![
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("annotation-probe-test"),
        ],
        Some(Duration::from_secs(2)),
    )
    .unwrap();

    let observation = probe
        .inspect(
            Path::new("source.dcm"),
            &annotation,
            None,
            PayloadMode::Digest,
        )
        .unwrap();

    assert_eq!(observation.report["status"], "ok");
}
