use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value};

mod geometry;
mod statistics;

pub use statistics::ErrorStats;
use statistics::stats;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComparisonResult {
    pub findings: Vec<Finding>,
    pub coordinate_error: ErrorStats,
    pub z_error_mm: ErrorStats,
}

impl ComparisonResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Compare normalized ANN semantics and geometry.
///
/// # Errors
///
/// Returns an error when either document is not a valid ANN semantic object or a
/// coordinate tolerance is negative.
pub fn compare_ann(
    expected: &Value,
    actual: &Value,
    pixel_tolerance: f64,
    millimeter_tolerance: f64,
) -> Result<ComparisonResult, String> {
    if pixel_tolerance < 0.0 || millimeter_tolerance < 0.0 {
        return Err("coordinate tolerances must be non-negative".to_owned());
    }
    let expected = semantic_data(expected, "Ann")?;
    let actual = semantic_data(actual, "Ann")?;
    let mut findings = Vec::new();

    if expected.get("source") != actual.get("source") {
        findings.push(finding(
            "SOURCE_REFERENCE_MISMATCH",
            "source",
            "referenced source identity or geometry differs from ground truth",
        ));
    }
    for key in [
        "series_instance_uid",
        "coordinate_type",
        "pixel_origin_interpretation",
        "referenced_frame_number",
        "content",
    ] {
        if expected.get(key) != actual.get(key) {
            findings.push(finding(
                "ANN_METADATA_MISMATCH",
                key,
                format!("{key} differs from ground truth"),
            ));
        }
    }

    let expected_groups = keyed(expected.get("groups"), "groups", "uid")?;
    let actual_groups = keyed(actual.get("groups"), "groups", "uid")?;
    if !expected_groups.duplicates.is_empty() || !actual_groups.duplicates.is_empty() {
        findings.push(finding(
            "DUPLICATE_GROUP_UID",
            "groups",
            "annotation group UIDs must be unique",
        ));
    }
    let missing: Vec<_> = expected_groups
        .values
        .keys()
        .filter(|key| !actual_groups.values.contains_key(*key))
        .cloned()
        .collect();
    let unexpected: Vec<_> = actual_groups
        .values
        .keys()
        .filter(|key| !expected_groups.values.contains_key(*key))
        .cloned()
        .collect();
    if !missing.is_empty() || !unexpected.is_empty() {
        findings.push(finding(
            "GROUP_UID_MISMATCH",
            "groups",
            format!("missing group UIDs {missing:?}; unexpected group UIDs {unexpected:?}"),
        ));
    }

    let mut pixel_errors = Vec::new();
    let mut z_errors = Vec::new();
    for (uid, expected_group) in &expected_groups.values {
        let Some(actual_group) = actual_groups.values.get(uid) else {
            continue;
        };
        let (group_pixel_errors, group_z_errors) = compare_ann_group(
            uid,
            expected_group,
            actual_group,
            pixel_tolerance,
            millimeter_tolerance,
            &mut findings,
        );
        pixel_errors.extend(group_pixel_errors);
        z_errors.extend(group_z_errors);
    }

    Ok(ComparisonResult {
        findings,
        coordinate_error: stats(&pixel_errors),
        z_error_mm: stats(&z_errors),
    })
}

/// Compare normalized SEG identity, segment semantics, and mask payloads.
///
/// # Errors
///
/// Returns an error when either document is not a valid SEG semantic object.
pub fn compare_seg(expected: &Value, actual: &Value) -> Result<ComparisonResult, String> {
    let expected = semantic_data(expected, "Seg")?;
    let actual = semantic_data(actual, "Seg")?;
    let mut findings = Vec::new();
    if expected.get("source") != actual.get("source") {
        findings.push(finding(
            "SOURCE_REFERENCE_MISMATCH",
            "source",
            "source reference differs",
        ));
    }
    for key in ["series_instance_uid", "segmentation_kind", "content"] {
        if expected.get(key) != actual.get(key) {
            findings.push(finding(
                "SEG_METADATA_MISMATCH",
                key,
                format!("{key} differs"),
            ));
        }
    }

    let expected_segments = keyed(expected.get("segments"), "segments", "number")?;
    let actual_segments = keyed(actual.get("segments"), "segments", "number")?;
    if !expected_segments.duplicates.is_empty() || !actual_segments.duplicates.is_empty() {
        findings.push(finding(
            "DUPLICATE_SEGMENT_NUMBER",
            "segments",
            "segment numbers must be unique",
        ));
    }
    if expected_segments.values.keys().collect::<Vec<_>>()
        != actual_segments.values.keys().collect::<Vec<_>>()
    {
        findings.push(finding(
            "SEGMENT_NUMBER_MISMATCH",
            "segments",
            "segment numbers differ",
        ));
    }
    for (number, expected_segment) in expected_segments.values {
        if actual_segments
            .values
            .get(&number)
            .is_some_and(|actual| *actual != expected_segment)
        {
            findings.push(finding(
                "SEGMENT_SEMANTICS_MISMATCH",
                format!("segments[{number}]"),
                "segment semantics differ",
            ));
        }
    }
    if canonical_masks(expected.get("masks")) != canonical_masks(actual.get("masks")) {
        findings.push(finding(
            "MASK_MISMATCH",
            "masks",
            "normalized mask payload differs",
        ));
    }
    Ok(ComparisonResult {
        findings,
        coordinate_error: stats(&[]),
        z_error_mm: stats(&[]),
    })
}

const CODE_FIELDS: [&str; 5] = [
    "category",
    "property_type",
    "property_type_modifiers",
    "anatomic_regions",
    "primary_anatomic_structures",
];

fn compare_ann_group(
    uid: &str,
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
    pixel_tolerance: f64,
    millimeter_tolerance: f64,
    findings: &mut Vec<Finding>,
) -> (Vec<f64>, Vec<f64>) {
    let path = format!("groups[{uid}]");
    if expected.get("algorithms") != actual.get("algorithms") {
        findings.push(finding(
            "ALGORITHM_MISMATCH",
            format!("{path}.algorithms"),
            "algorithm identification differs from ground truth",
        ));
    }
    for key in [
        "label",
        "description",
        "generation_type",
        "category",
        "property_type",
        "property_type_modifiers",
        "anatomic_regions",
        "primary_anatomic_structures",
        "applies_to_all_optical_paths",
        "referenced_optical_paths",
        "applies_to_all_z_planes",
        "common_z_coordinates_mm",
        "recommended_display_cielab",
        "graphic_type",
        "annotation_count",
        "measurements",
    ] {
        if expected.get(key) != actual.get(key) {
            let code = if CODE_FIELDS.contains(&key) {
                "CODE_MISMATCH"
            } else {
                "GROUP_METADATA_MISMATCH"
            };
            findings.push(finding(
                code,
                format!("{path}.{key}"),
                format!("{key} differs from ground truth"),
            ));
        }
    }
    let (Some(expected_geometry), Some(actual_geometry)) = (
        expected.get("geometry").and_then(Value::as_object),
        actual.get("geometry").and_then(Value::as_object),
    ) else {
        findings.push(finding(
            "INVALID_GEOMETRY",
            format!("{path}.geometry"),
            "geometry must be an object",
        ));
        return (Vec::new(), Vec::new());
    };
    if expected_geometry.get("mode").and_then(Value::as_str) == Some("Digest")
        || actual_geometry.get("mode").and_then(Value::as_str) == Some("Digest")
    {
        compare_geometry_digests(expected_geometry, actual_geometry, &path, findings);
        return (Vec::new(), Vec::new());
    }
    let graphic_type = expected
        .get("graphic_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let errors = match geometry::errors(expected_geometry, actual_geometry, graphic_type) {
        Ok(errors) => (errors.canonical, errors.z, errors.native),
        Err(error) => {
            findings.push(finding(
                "INVALID_GEOMETRY",
                format!("{path}.geometry"),
                error,
            ));
            return (Vec::new(), Vec::new());
        }
    };
    add_tolerance_findings(
        &path,
        expected_geometry,
        (&errors.0, &errors.1, &errors.2),
        (pixel_tolerance, millimeter_tolerance),
        findings,
    );
    (errors.0, errors.1)
}

fn add_tolerance_findings(
    path: &str,
    geometry: &Map<String, Value>,
    errors: (&[f64], &[f64], &[f64]),
    tolerances: (f64, f64),
    findings: &mut Vec<Finding>,
) {
    let (pixel_tolerance, millimeter_tolerance) = tolerances;
    if let Some(value) = maximum(errors.0).filter(|value| *value > pixel_tolerance) {
        findings.push(finding(
            "COORDINATE_TOLERANCE_EXCEEDED",
            format!("{path}.geometry.canonical_level0_coordinates"),
            format!("maximum level-0 error {value:.12} px exceeds {pixel_tolerance:.12} px"),
        ));
    }
    if let Some(value) = maximum(errors.1).filter(|value| *value > millimeter_tolerance) {
        findings.push(finding(
            "Z_TOLERANCE_EXCEEDED",
            format!("{path}.geometry.canonical_level0_coordinates"),
            format!("maximum Z error {value:.12} mm exceeds {millimeter_tolerance:.12} mm"),
        ));
    }
    let native_tolerance =
        if geometry.get("canonical_dimensions").and_then(Value::as_u64) == Some(2) {
            pixel_tolerance
        } else {
            millimeter_tolerance
        };
    if let Some(value) = maximum(errors.2).filter(|value| *value > native_tolerance) {
        findings.push(finding(
            "NATIVE_COORDINATE_TOLERANCE_EXCEEDED",
            format!("{path}.geometry.native_coordinates"),
            format!("maximum native-coordinate error {value:.12} exceeds {native_tolerance:.12}"),
        ));
    }
}

fn maximum(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::max)
}

fn finding(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> Finding {
    Finding {
        code: code.into(),
        path: path.into(),
        message: message.into(),
    }
}

fn semantic_data<'a>(
    document: &'a Value,
    expected_type: &str,
) -> Result<&'a Map<String, Value>, String> {
    let object = document
        .as_object()
        .ok_or_else(|| "semantic document must be an object".to_owned())?;
    let Some(semantic) = object.get("semantic") else {
        return Ok(object);
    };
    let semantic = semantic
        .as_object()
        .ok_or_else(|| format!("expected {expected_type} semantic document"))?;
    if semantic.get("object_type").and_then(Value::as_str) != Some(expected_type) {
        return Err(format!("expected {expected_type} semantic document"));
    }
    semantic
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "semantic.data must be an object".to_owned())
}

struct KeyedItems<'a> {
    values: BTreeMap<String, &'a Map<String, Value>>,
    duplicates: BTreeSet<String>,
}

fn keyed<'a>(
    items: Option<&'a Value>,
    collection: &str,
    key: &str,
) -> Result<KeyedItems<'a>, String> {
    let mut result = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    let items = items
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{collection} must be an array"))?;
    for (index, item) in items.iter().enumerate() {
        let object = item
            .as_object()
            .ok_or_else(|| format!("{collection}[{index}] must be an object"))?;
        let value = object
            .get(key)
            .and_then(key_value)
            .ok_or_else(|| format!("{collection}[{index}].{key} is missing or invalid"))?;
        if result.insert(value.clone(), object).is_some() {
            duplicates.insert(value);
        }
    }
    Ok(KeyedItems {
        values: result,
        duplicates,
    })
}

fn key_value(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        Some(format!("s:{value}"))
    } else if let Some(value) = value.as_i64() {
        Some(format!("n:{value}"))
    } else {
        value.as_u64().map(|value| format!("n:{value}"))
    }
}

fn compare_geometry_digests(
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
    path: &str,
    findings: &mut Vec<Finding>,
) {
    let differs = [
        "mode",
        "native_dimensions",
        "canonical_dimensions",
        "native_coordinate_count",
        "canonical_coordinate_count",
        "native_sha256",
        "canonical_level0_sha256",
        "primitive_point_indices",
    ]
    .into_iter()
    .any(|key| expected.get(key) != actual.get(key));
    if differs {
        findings.push(finding(
            "COORDINATE_DIGEST_MISMATCH",
            format!("{path}.geometry"),
            "coordinate counts, representation, or digest differs",
        ));
    }
}

fn canonical_masks(value: Option<&Value>) -> Option<Value> {
    let mut value = value?.clone();
    if let Some(runs) = value.get_mut("runs").and_then(Value::as_array_mut) {
        runs.sort_by_key(|run| {
            (
                run.get("segment_number")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                run.get("row").and_then(Value::as_i64).unwrap_or(0),
                run.get("column_start").and_then(Value::as_i64).unwrap_or(0),
            )
        });
    }
    Some(value)
}
