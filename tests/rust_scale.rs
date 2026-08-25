use std::env;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::tempdir;
use wsi_annotation_interop::probe::ViewerProbe;
use wsi_annotation_interop::shim::ReferenceShim;
use wsi_annotation_interop::{ScaleCase, ScaleStatus, default_scale_cases, run_scale_cases};

fn real_adapters() -> (ReferenceShim, ViewerProbe) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    assert!(python.is_file(), "run `uv sync --locked` first");
    let probe = env::var_os("ANNOTATION_PROBE").map_or_else(
        || root.join("../dicom-viewer/target/debug/annotation_probe"),
        PathBuf::from,
    );
    assert!(
        probe.is_file(),
        "build annotation_probe or set ANNOTATION_PROBE"
    );
    (
        ReferenceShim::new(
            vec![
                python.to_string_lossy().into_owned(),
                root.join("shim/reference_shim.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            Duration::from_mins(10),
        )
        .unwrap(),
        ViewerProbe::new(
            vec![probe.to_string_lossy().into_owned()],
            Some(Duration::from_mins(10)),
        )
        .unwrap(),
    )
}

#[test]
fn scale_profile_requires_one_million_and_profiles_five_million_coordinates() {
    let cases = default_scale_cases();

    assert!(
        cases
            .iter()
            .any(|case| case.coordinate_values == 1_000_000 && case.required)
    );
    assert!(
        cases
            .iter()
            .any(|case| case.coordinate_values == 5_000_000 && !case.required)
    );
}

#[test]
fn rust_runs_a_scale_roundtrip_with_digest_payloads() {
    let (reference, probe) = real_adapters();
    let directory = tempdir().unwrap();
    let fixtures = reference
        .generate_core(&directory.path().join("fixtures"))
        .unwrap();
    let qualification = reference
        .qualify_pydcm(
            &fixtures.source,
            &fixtures.ann["2D_VOLUME"],
            &fixtures.seg["BINARY"],
        )
        .unwrap();
    assert_eq!(qualification.implementation, "pydcm");
    assert!(!qualification.primary_failure);

    let observations = run_scale_cases(
        &reference,
        &probe,
        &fixtures.source,
        &directory.path().join("scale"),
        &[ScaleCase {
            coordinate_values: 1_000,
            required: false,
        }],
    )
    .unwrap();

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].status, ScaleStatus::Passed);
    assert_eq!(observations[0].coordinate_error_max_px, Some(0.0));
    assert!(observations[0].input_bytes.unwrap() > 0);
    assert!(observations[0].peak_rss_bytes.unwrap() > 0);
}
