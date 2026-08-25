use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use wsi_annotation_interop::build_core_ground_truth;

#[test]
fn core_ground_truth_is_deterministic_and_covers_the_matrix() {
    let first = build_core_ground_truth();
    let second = build_core_ground_truth();

    assert_eq!(first, second);
    assert_eq!(first["schema_version"], 1);
    let case_ids: BTreeSet<_> = first["cases"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        case_ids,
        BTreeSet::from([
            "ann-2d-frame",
            "ann-2d-volume",
            "ann-2d-volume-level1",
            "ann-3d-common-z",
            "ann-3d-xyz",
            "seg-binary",
            "seg-binary-reordered",
            "seg-fractional",
            "seg-labelmap",
        ])
    );
}

#[test]
fn core_ground_truth_preserves_required_graphics_coordinates_and_mask_digests() {
    let truth = build_core_ground_truth();
    let cases = truth["cases"].as_object().unwrap();
    let graphics: BTreeSet<_> = cases["ann-3d-xyz"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["graphic_type"].as_str().unwrap())
        .collect();

    assert_eq!(
        graphics,
        BTreeSet::from(["ELLIPSE", "POINT", "POLYGON", "POLYLINE", "RECTANGLE"])
    );
    assert_eq!(
        cases["ann-2d-frame"]["pixel_origin_interpretation"],
        "FRAME"
    );
    assert_eq!(
        cases["ann-3d-common-z"]["groups"][0]["geometry"]["native_dimensions"],
        2
    );
    assert_eq!(
        cases["ann-3d-xyz"]["groups"][0]["geometry"]["native_dimensions"],
        3
    );
    assert_eq!(
        cases["seg-binary"]["masks"]["sha256"],
        "bcbd27e313ed35d89dea566ab5215d0b746a73b2e94fbd576b631ea71dd470c1"
    );
    assert_eq!(
        cases["seg-labelmap"]["masks"]["sha256"],
        "bee789ccd3db202213cbcb554714d218bc51beee91cfbdae6f1fcfe6140f55c0"
    );
    assert_eq!(
        cases["seg-fractional"]["masks"]["sha256"],
        "2b8845e05330bd44181e24a94b06c6045a87a109a5e44d19686ecba9824456d2"
    );
}

#[test]
fn core_ground_truth_matches_the_canonical_legacy_oracle() {
    let canonical_json = serde_json::to_vec(&build_core_ground_truth()).unwrap();

    assert_eq!(
        format!("{:x}", Sha256::digest(canonical_json)),
        "d6255973874aff07b769d3b18145b7e5dc7d248fa3dc14ca868de5b3de7d5d88"
    );
}
