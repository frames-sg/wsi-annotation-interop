use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::tempdir;
use wsi_annotation_interop::probe::ViewerProbe;
use wsi_annotation_interop::run_conversion_matrices;
use wsi_annotation_interop::shim::{FixtureSet, ReferenceShim};

#[test]
fn failed_conversion_retains_process_evidence() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("unused.dcm");
    let fixtures = FixtureSet {
        source: path.clone(),
        pyramid_source: path.clone(),
        pyramid_ann: path.clone(),
        reordered_seg: path.clone(),
        pm: path.clone(),
        sr: path.clone(),
        sr_seg: path.clone(),
        ground_truth: path,
        ann: BTreeMap::new(),
        seg: BTreeMap::new(),
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let python = root.join(".venv/bin/python");
    let probe = ViewerProbe::new(
        vec![
            python.to_string_lossy().into_owned(),
            "-c".into(),
            "import sys; print('conversion diagnostic', file=sys.stderr); sys.exit(7)".into(),
        ],
        Some(Duration::from_secs(5)),
    )
    .unwrap();
    let reference =
        ReferenceShim::new(vec!["unused-reference".into()], Duration::from_secs(5)).unwrap();
    let result = run_conversion_matrices(
        &fixtures,
        &reference,
        &probe,
        &directory.path().join("results"),
    )
    .unwrap();
    assert_eq!(result.observations.len(), 6);
    for row in result.observations {
        let value = serde_json::to_value(row).unwrap();
        assert_eq!(value["status"], "failed");
        assert!(!value["command"].as_array().unwrap().is_empty());
        assert_eq!(value["report"]["execution_error"]["returncode"], 7);
        assert!(
            value["report"]["execution_error"]["stderr"]
                .as_str()
                .unwrap()
                .contains("conversion diagnostic")
        );
        assert!(value["runtime_ms"].as_f64().is_some_and(|time| time > 0.0));
        assert!(value["report"]["measurements"]["peak_tracked_heap_bytes"].is_null());
    }
}

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

    assert_corruption_is_rejected(&root, &python, &fixtures, &probe, directory.path());
}

fn assert_corruption_is_rejected(
    root: &Path,
    python: &Path,
    fixtures: &FixtureSet,
    probe: &ViewerProbe,
    directory: &Path,
) {
    // Exercise the complete process boundary with an independently altered reader result.
    // A changed value must fail even when shape, counts, and recorded digests still match.
    let wrapper = directory.join("altered_reader.py");
    std::fs::write(&wrapper, r"
import json, subprocess, sys
mode, shim = sys.argv[1:3]
result = subprocess.run([sys.executable, shim, *sys.argv[3:]], capture_output=True, text=True)
if result.returncode:
    sys.stderr.write(result.stderr)
    sys.exit(result.returncode)
document = json.loads(result.stdout)
command = sys.argv[3]
if mode == 'ann' and command == 'normalize-ann':
    document['groups'][0]['geometry']['native_coordinates'][0] += 10
    document['groups'][0]['geometry']['canonical_level0_coordinates'][0] += 10
elif mode == 'measurement' and command == 'normalize-ann':
    document['groups'][0]['measurements'][0]['values'][0] += 10
elif mode == 'seg' and command == 'normalize-seg':
    document['masks']['runs'].append({'segment_number': 1, 'row': 0, 'column_start': 0, 'length': 1})
elif mode == 'pm' and command == 'normalize-pm':
    samples = document['pixel'].setdefault('samples', [0.0] * 256)
    samples[0] = 0.75
elif mode == 'sr' and command == 'normalize-sr' and document['groups'][0]['reference']['kind'] == 'coordinates':
    document['groups'][0]['reference']['graphic_data'][1][0] += 0.01
print(json.dumps(document, allow_nan=False))
").unwrap();
    for (mode, case) in [
        ("ann", "geojson-ann"),
        ("measurement", "geojson-ann"),
        ("seg", "geojson-seg"),
        ("pm", "pm-float32"),
        ("sr", "sr-direct"),
    ] {
        let altered = ReferenceShim::new(
            vec![
                python.to_string_lossy().into_owned(),
                wrapper.to_string_lossy().into_owned(),
                mode.into(),
                root.join("shim/reference_shim.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            Duration::from_mins(1),
        )
        .unwrap();
        let result = run_conversion_matrices(
            fixtures,
            &altered,
            probe,
            &directory.join(format!("altered-{mode}")),
        )
        .unwrap();
        let rejected = result
            .observations
            .iter()
            .find(|item| item.case_id == case)
            .unwrap();
        assert!(
            !rejected.status.is_passed(),
            "{mode} corruption was accepted"
        );
        assert!(
            !rejected.command.is_empty(),
            "lost the executed conversion command"
        );
        assert!(
            !rejected.output_paths.is_empty(),
            "lost the converted artifact"
        );
        assert!(
            rejected.highdicom_readable,
            "a comparison failure is not a decoder failure"
        );
        if mode == "sr" {
            assert!(
                result
                    .observations
                    .iter()
                    .find(|item| item.case_id == "geojson-ann")
                    .unwrap()
                    .status
                    .is_passed(),
                "a later SR failure erased the successful ANN observation"
            );
        }
    }
}
