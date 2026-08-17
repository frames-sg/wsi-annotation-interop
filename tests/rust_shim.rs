#![cfg(unix)]

use std::time::Duration;
use std::{collections::BTreeMap, fs};

use tempfile::tempdir;
use wsi_annotation_interop::shim::ReferenceShim;

#[test]
fn reference_shim_uses_a_json_only_subprocess_contract() {
    let directory = tempdir().unwrap();
    let paths = [
        "source.dcm",
        "pyramid-source.dcm",
        "pyramid-ann.dcm",
        "reordered-seg.dcm",
        "pm.dcm",
        "sr.dcm",
        "sr-seg.dcm",
        "ann-2d-frame.dcm",
        "ann-2d-volume.dcm",
        "ann-3d-common-z.dcm",
        "ann-3d-xyz.dcm",
        "seg-binary.dcm",
        "seg-labelmap.dcm",
        "seg-fractional.dcm",
    ];
    for path in paths {
        fs::write(directory.path().join(path), b"fixture").unwrap();
    }
    let report = serde_json::json!({
        "source": directory.path().join("source.dcm"),
        "pyramid_source": directory.path().join("pyramid-source.dcm"),
        "pyramid_ann": directory.path().join("pyramid-ann.dcm"),
        "reordered_seg": directory.path().join("reordered-seg.dcm"),
        "pm": directory.path().join("pm.dcm"),
        "sr": directory.path().join("sr.dcm"),
        "sr_seg": directory.path().join("sr-seg.dcm"),
        "ann": {
            "2D_FRAME": directory.path().join("ann-2d-frame.dcm"),
            "2D_VOLUME": directory.path().join("ann-2d-volume.dcm"),
            "3D_COMMON_Z": directory.path().join("ann-3d-common-z.dcm"),
            "3D_XYZ": directory.path().join("ann-3d-xyz.dcm"),
        },
        "seg": {
            "BINARY": directory.path().join("seg-binary.dcm"),
            "LABELMAP": directory.path().join("seg-labelmap.dcm"),
            "FRACTIONAL": directory.path().join("seg-fractional.dcm"),
        }
    });
    let script = format!(
        "printf '%s\\n' '{}'",
        serde_json::to_string(&report).unwrap()
    );
    let shim = ReferenceShim::new(
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            script,
            "reference-shim-test".to_owned(),
        ],
        Duration::from_secs(2),
    )
    .unwrap();

    let observed = shim.generate_core(directory.path()).unwrap();

    assert_eq!(observed.source, directory.path().join("source.dcm"));
    assert_eq!(observed.pm, directory.path().join("pm.dcm"));
    assert_eq!(observed.sr, directory.path().join("sr.dcm"));
    assert_eq!(observed.sr_seg, directory.path().join("sr-seg.dcm"));
    assert_eq!(
        observed.ann.keys().cloned().collect::<Vec<_>>(),
        ["2D_FRAME", "2D_VOLUME", "3D_COMMON_Z", "3D_XYZ"]
    );
    assert_eq!(
        observed.seg,
        BTreeMap::from([
            ("BINARY".to_owned(), directory.path().join("seg-binary.dcm")),
            (
                "FRACTIONAL".to_owned(),
                directory.path().join("seg-fractional.dcm")
            ),
            (
                "LABELMAP".to_owned(),
                directory.path().join("seg-labelmap.dcm")
            ),
        ])
    );
    let ground_truth: serde_json::Value =
        serde_json::from_slice(&fs::read(&observed.ground_truth).unwrap()).unwrap();
    assert_eq!(
        ground_truth,
        wsi_annotation_interop::ground_truth::build_core_ground_truth()
    );
}
