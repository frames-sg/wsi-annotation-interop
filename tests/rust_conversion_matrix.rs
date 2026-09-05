use std::env;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::tempdir;
use wsi_annotation_interop::probe::ViewerProbe;
use wsi_annotation_interop::run_conversion_matrices;
use wsi_annotation_interop::shim::ReferenceShim;

#[test]
#[ignore = "requires the external annotation_probe built by scripts/check-core.sh"]
fn rust_runs_separate_geojson_sr_and_parametric_map_conversion_matrices() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    assert!(python.is_file(), "run `uv sync --locked` first");
    let probe_path = env::var_os("ANNOTATION_PROBE").map_or_else(
        || root.join("../dicom-viewer/target/debug/annotation_probe"),
        PathBuf::from,
    );
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
    let fixtures = reference
        .generate_core(&directory.path().join("fixtures"))
        .unwrap();

    let result = run_conversion_matrices(
        &fixtures,
        &reference,
        &probe,
        &directory.path().join("conversion"),
    )
    .unwrap();

    assert!(
        result.is_ok(),
        "{}",
        result
            .observations
            .iter()
            .map(|item| format!("{}: {}", item.case_id, item.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(
        result
            .observations
            .iter()
            .map(|item| item.case_id.as_str())
            .collect::<Vec<_>>(),
        [
            "geojson-ann",
            "sr-direct",
            "geojson-seg",
            "sr-seg-reference",
            "pm-float32",
            "pm-concatenation",
        ]
    );
    let concatenation = result
        .observations
        .iter()
        .find(|item| item.case_id == "pm-concatenation")
        .unwrap();
    assert_eq!(concatenation.output_paths.len(), 3);
    assert_eq!(concatenation.normalized.as_array().unwrap().len(), 3);
}
