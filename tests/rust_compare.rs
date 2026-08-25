use serde_json::{Value, json};
use wsi_annotation_interop::compare::{compare_ann, compare_seg};

fn ann() -> Value {
    json!({
        "source": {
            "sop_instance_uid": "2.25.1",
            "study_instance_uid": "2.25.2",
            "frame_of_reference_uid": "2.25.3"
        },
        "groups": [{
            "uid": "2.25.10",
            "generation_type": "AUTOMATIC",
            "algorithms": [{"name": "model", "version": "1"}],
            "category": {"value": "MORPH", "scheme": "99TEST", "meaning": "Morphology"},
            "property_type": {"value": "TUMOR", "scheme": "99TEST", "meaning": "Tumor"},
            "graphic_type": "POLYGON",
            "geometry": {
                "mode": "Full",
                "native_dimensions": 2,
                "native_coordinates": [0.0, 0.0, 4.0, 0.0, 4.0, 4.0, 0.0, 4.0],
                "canonical_dimensions": 2,
                "canonical_level0_coordinates": [0.0, 0.0, 4.0, 0.0, 4.0, 4.0, 0.0, 4.0],
                "primitive_point_indices": [1]
            }
        }]
    })
}

fn finding_codes(result: &wsi_annotation_interop::compare::ComparisonResult) -> Vec<&str> {
    result
        .findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect()
}

#[test]
fn ann_comparator_accepts_group_reordering_and_polygon_symmetry() {
    let mut expected = ann();
    let mut second = expected["groups"][0].clone();
    second["uid"] = json!("2.25.11");
    expected["groups"].as_array_mut().unwrap().push(second);
    let mut actual = expected.clone();
    actual["groups"][0]["geometry"]["canonical_level0_coordinates"] =
        json!([4.0, 4.0, 4.0, 0.0, 0.0, 0.0, 0.0, 4.0]);
    actual["groups"].as_array_mut().unwrap().reverse();

    let result = compare_ann(&expected, &actual, 1e-6, 1e-9).unwrap();

    assert!(result.is_ok());
    assert!(result.coordinate_error.max.abs() < f64::EPSILON);
}

#[test]
fn ann_comparator_detects_coordinate_algorithm_and_reference_defects() {
    let expected = ann();
    let mut actual = expected.clone();
    actual["source"]["sop_instance_uid"] = json!("2.25.999");
    actual["groups"][0]["algorithms"] = json!([]);
    actual["groups"][0]["geometry"]["canonical_level0_coordinates"][2] = json!(4.01);

    let result = compare_ann(&expected, &actual, 1e-6, 1e-9).unwrap();
    let codes = finding_codes(&result);

    assert!(!result.is_ok());
    assert!(codes.contains(&"SOURCE_REFERENCE_MISMATCH"));
    assert!(codes.contains(&"ALGORITHM_MISMATCH"));
    assert!(codes.contains(&"COORDINATE_TOLERANCE_EXCEEDED"));
    assert!((result.coordinate_error.max - 0.01).abs() < 1e-12);
}

#[test]
fn ann_comparator_detects_scale_qualifier_and_group_identity_defects() {
    let mut expected = ann();
    expected["groups"][0]["category"]["coding_scheme_version"] = json!("1");

    let mut scaled = expected.clone();
    scaled["groups"][0]["geometry"]["canonical_level0_coordinates"][2] = json!(4.04);
    assert!(
        finding_codes(&compare_ann(&expected, &scaled, 1e-6, 1e-9).unwrap())
            .contains(&"COORDINATE_TOLERANCE_EXCEEDED")
    );

    let mut qualifier_lost = expected.clone();
    qualifier_lost["groups"][0]["category"]
        .as_object_mut()
        .unwrap()
        .remove("coding_scheme_version");
    assert!(
        finding_codes(&compare_ann(&expected, &qualifier_lost, 1e-6, 1e-9).unwrap())
            .contains(&"CODE_MISMATCH")
    );

    let mut identity_changed = expected.clone();
    identity_changed["groups"][0]["uid"] = json!("2.25.999");
    assert!(
        finding_codes(&compare_ann(&expected, &identity_changed, 1e-6, 1e-9).unwrap())
            .contains(&"GROUP_UID_MISMATCH")
    );
}

#[test]
fn ann_comparator_detects_native_coordinate_loss() {
    let expected = ann();
    let mut actual = expected.clone();
    actual["groups"][0]["geometry"]["native_coordinates"][2] = json!(4.01);

    let result = compare_ann(&expected, &actual, 1e-6, 1e-9).unwrap();

    assert!(finding_codes(&result).contains(&"NATIVE_COORDINATE_TOLERANCE_EXCEEDED"));
}

#[test]
fn ann_comparator_canonicalizes_equivalent_ellipse_axes() {
    let mut expected = ann();
    expected["groups"][0]["graphic_type"] = json!("ELLIPSE");
    expected["groups"][0]["geometry"]["canonical_level0_coordinates"] =
        json!([4.0, 0.0, 0.0, 0.0, 2.0, 1.0, 2.0, -1.0]);
    let mut actual = expected.clone();
    actual["groups"][0]["geometry"]["canonical_level0_coordinates"] =
        json!([2.0, -1.0, 2.0, 1.0, 0.0, 0.0, 4.0, 0.0]);

    assert!(compare_ann(&expected, &actual, 1e-6, 1e-9).unwrap().is_ok());
}

#[test]
fn ann_comparator_uses_dicom_scalar_primitive_offsets() {
    let mut expected = ann();
    let coordinates = json!([
        0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 2.0, 2.0, 3.0, 2.0, 3.0, 3.0, 2.0, 3.0
    ]);
    expected["groups"][0]["geometry"]["native_coordinates"] = coordinates.clone();
    expected["groups"][0]["geometry"]["canonical_level0_coordinates"] = coordinates;
    expected["groups"][0]["geometry"]["primitive_point_indices"] = json!([1, 9]);

    assert!(
        compare_ann(&expected, &expected, 1e-6, 1e-9)
            .unwrap()
            .is_ok()
    );
}

#[test]
fn seg_comparator_ignores_run_order_but_detects_segment_semantic_loss() {
    let expected = json!({
        "source": {"sop_instance_uid": "2.25.1"},
        "segmentation_kind": "BINARY",
        "segments": [{"number": 1, "label": "tumor", "tracking_uid": "2.25.9"}],
        "masks": {
            "mode": "FullBinary",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "runs": [
                {"segment_number": 1, "row": 1, "column_start": 0, "length": 2},
                {"segment_number": 1, "row": 0, "column_start": 0, "length": 1}
            ]
        }
    });
    let mut actual = expected.clone();
    actual["masks"]["runs"].as_array_mut().unwrap().reverse();
    assert!(compare_seg(&expected, &actual).unwrap().is_ok());

    actual["segments"][0]
        .as_object_mut()
        .unwrap()
        .remove("tracking_uid");
    let result = compare_seg(&expected, &actual).unwrap();
    assert_eq!(finding_codes(&result), ["SEGMENT_SEMANTICS_MISMATCH"]);
}

#[test]
fn ann_comparator_rejects_malformed_group_entries() {
    let mut malformed = ann();
    malformed["groups"] = json!([null]);

    assert!(
        compare_ann(&malformed, &malformed, 1e-6, 1e-9)
            .unwrap_err()
            .contains("groups[0] must be an object")
    );
}

#[test]
fn seg_comparator_rejects_missing_segment_identity() {
    let malformed = json!({
        "source": {"sop_instance_uid": "2.25.1"},
        "segmentation_kind": "BINARY",
        "segments": [{"label": "tumor"}],
        "masks": {"mode": "FullBinary", "runs": []}
    });

    assert!(
        compare_seg(&malformed, &malformed)
            .unwrap_err()
            .contains("segments[0].number is missing or invalid")
    );
}
