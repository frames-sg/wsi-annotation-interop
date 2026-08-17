use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ErrorStats {
    pub count: usize,
    pub max: f64,
    pub median: f64,
    pub rms: f64,
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

    let (expected_groups, expected_duplicates) = keyed(expected.get("groups"), "uid");
    let (actual_groups, actual_duplicates) = keyed(actual.get("groups"), "uid");
    if !expected_duplicates.is_empty() || !actual_duplicates.is_empty() {
        findings.push(finding(
            "DUPLICATE_GROUP_UID",
            "groups",
            "annotation group UIDs must be unique",
        ));
    }
    let missing: Vec<_> = expected_groups
        .keys()
        .filter(|key| !actual_groups.contains_key(*key))
        .cloned()
        .collect();
    let unexpected: Vec<_> = actual_groups
        .keys()
        .filter(|key| !expected_groups.contains_key(*key))
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
    for (uid, expected_group) in &expected_groups {
        let Some(actual_group) = actual_groups.get(uid) else {
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

    let (expected_segments, expected_duplicates) = keyed(expected.get("segments"), "number");
    let (actual_segments, actual_duplicates) = keyed(actual.get("segments"), "number");
    if !expected_duplicates.is_empty() || !actual_duplicates.is_empty() {
        findings.push(finding(
            "DUPLICATE_SEGMENT_NUMBER",
            "segments",
            "segment numbers must be unique",
        ));
    }
    if expected_segments.keys().collect::<Vec<_>>() != actual_segments.keys().collect::<Vec<_>>() {
        findings.push(finding(
            "SEGMENT_NUMBER_MISMATCH",
            "segments",
            "segment numbers differ",
        ));
    }
    for (number, expected_segment) in expected_segments {
        if actual_segments
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
    let errors = coordinate_errors(expected_geometry, actual_geometry, graphic_type);
    let native = native_coordinate_errors(expected_geometry, actual_geometry, graphic_type);
    let errors = match (errors, native) {
        (Ok((pixel, z)), Ok(native)) => (pixel, z, native),
        (Err(error), _) | (_, Err(error)) => {
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

fn keyed<'a>(
    items: Option<&'a Value>,
    key: &str,
) -> (BTreeMap<String, &'a Map<String, Value>>, BTreeSet<String>) {
    let mut result = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    let Some(items) = items.and_then(Value::as_array) else {
        return (result, duplicates);
    };
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(value) = object.get(key).and_then(key_value) else {
            continue;
        };
        if result.insert(value.clone(), object).is_some() {
            duplicates.insert(value);
        }
    }
    (result, duplicates)
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

fn coordinate_errors(
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
    graphic_type: &str,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let coordinate_dimensions = dimensions(expected, "canonical_dimensions")?;
    if dimensions(actual, "canonical_dimensions")? != coordinate_dimensions {
        return Err("canonical dimensions differ or are invalid".to_owned());
    }
    if expected.get("primitive_point_indices") != actual.get("primitive_point_indices") {
        return Err("primitive point associations differ".to_owned());
    }
    let index_dimensions = expected
        .get("native_dimensions")
        .map_or(Ok(coordinate_dimensions), json_dimensions);
    let actual_index_dimensions = actual
        .get("native_dimensions")
        .map_or(Ok(coordinate_dimensions), json_dimensions);
    let (index_dimensions, actual_index_dimensions) = (index_dimensions?, actual_index_dimensions?);
    if index_dimensions != actual_index_dimensions {
        return Err("native dimensions differ or are invalid".to_owned());
    }
    let expected_primitives = primitives(
        expected,
        coordinate_dimensions,
        index_dimensions,
        "canonical_level0_coordinates",
    )?;
    let actual_primitives = primitives(
        actual,
        coordinate_dimensions,
        index_dimensions,
        "canonical_level0_coordinates",
    )?;
    if expected_primitives.len() != actual_primitives.len() {
        return Err("primitive counts differ".to_owned());
    }

    let mut pixel_errors = Vec::new();
    let mut z_errors = Vec::new();
    for (mut expected_points, actual_points) in
        expected_primitives.into_iter().zip(actual_primitives)
    {
        if expected_points.len() != actual_points.len() {
            return Err("primitive point counts differ".to_owned());
        }
        if graphic_type == "ELLIPSE" && expected_points.len() == 4 {
            expected_points = canonical_ellipse(&expected_points);
        }
        let actual_points = align_points(&expected_points, &actual_points, graphic_type);
        for (expected_point, actual_point) in expected_points.iter().zip(&actual_points) {
            pixel_errors.push(distance(&expected_point[..2], &actual_point[..2]));
            if coordinate_dimensions == 3 {
                z_errors.push((expected_point[2] - actual_point[2]).abs());
            }
        }
    }
    Ok((pixel_errors, z_errors))
}

fn native_coordinate_errors(
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
    graphic_type: &str,
) -> Result<Vec<f64>, String> {
    if !expected.contains_key("native_coordinates") && !actual.contains_key("native_coordinates") {
        return Ok(Vec::new());
    }
    let coordinate_dimensions = dimensions(expected, "native_dimensions")?;
    if dimensions(actual, "native_dimensions")? != coordinate_dimensions {
        return Err("native dimensions differ or are invalid".to_owned());
    }
    let expected_primitives = primitives(
        expected,
        coordinate_dimensions,
        coordinate_dimensions,
        "native_coordinates",
    )?;
    let actual_primitives = primitives(
        actual,
        coordinate_dimensions,
        coordinate_dimensions,
        "native_coordinates",
    )?;
    if expected_primitives.len() != actual_primitives.len() {
        return Err("native primitive counts differ".to_owned());
    }
    let mut errors = Vec::new();
    for (mut expected_points, actual_points) in
        expected_primitives.into_iter().zip(actual_primitives)
    {
        if expected_points.len() != actual_points.len() {
            return Err("native primitive point counts differ".to_owned());
        }
        if graphic_type == "ELLIPSE" && expected_points.len() == 4 {
            expected_points = canonical_ellipse(&expected_points);
        }
        let actual_points = align_points(&expected_points, &actual_points, graphic_type);
        errors.extend(
            expected_points
                .iter()
                .zip(&actual_points)
                .map(|(left, right)| distance(left, right)),
        );
    }
    Ok(errors)
}

fn primitives(
    geometry: &Map<String, Value>,
    dimensions: usize,
    index_dimensions: usize,
    coordinate_key: &str,
) -> Result<Vec<Vec<Vec<f64>>>, String> {
    let raw = geometry
        .get(coordinate_key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{coordinate_key} must be an array"))?;
    let coordinates: Vec<_> = raw
        .iter()
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("{coordinate_key} must be finite numeric data"))
        })
        .collect::<Result<_, _>>()?;
    if coordinates.len() % dimensions != 0 {
        return Err(format!(
            "{coordinate_key} count is not divisible by its dimensions"
        ));
    }
    let points: Vec<_> = coordinates
        .chunks_exact(dimensions)
        .map(<[f64]>::to_vec)
        .collect();
    let raw_starts = geometry
        .get("primitive_point_indices")
        .and_then(Value::as_array);
    let scalar_offsets = if raw_starts.is_none_or(Vec::is_empty) {
        if points.is_empty() {
            Vec::new()
        } else {
            vec![0]
        }
    } else {
        raw_starts
            .unwrap()
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| value.checked_sub(1))
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| "primitive point indices must be positive integers".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    if scalar_offsets
        .iter()
        .any(|offset| offset % index_dimensions != 0)
    {
        return Err("primitive point index is not aligned to a coordinate tuple".to_owned());
    }
    let starts: Vec<_> = scalar_offsets
        .iter()
        .map(|offset| offset / index_dimensions)
        .collect();
    if !points.is_empty() && starts.first() != Some(&0) {
        return Err("primitive point indices must start at 1".to_owned());
    }
    if starts.iter().any(|start| *start >= points.len()) {
        return Err("primitive point index is outside the coordinate payload".to_owned());
    }
    if starts.windows(2).any(|window| window[0] >= window[1]) {
        return Err("primitive point indices must be strictly increasing".to_owned());
    }
    Ok(starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(points.len());
            points[*start..end].to_vec()
        })
        .collect())
}

fn align_points(expected: &[Vec<f64>], actual: &[Vec<f64>], graphic_type: &str) -> Vec<Vec<f64>> {
    if expected.is_empty() {
        return actual.to_vec();
    }
    if matches!(graphic_type, "POLYGON" | "RECTANGLE") {
        let mut best = actual.to_vec();
        let mut best_error = squared_error(expected, &best);
        for reversed in [false, true] {
            let mut points = actual.to_vec();
            if reversed {
                points.reverse();
            }
            for offset in 0..points.len() {
                let mut candidate = points[offset..].to_vec();
                candidate.extend_from_slice(&points[..offset]);
                let error = squared_error(expected, &candidate);
                if error < best_error {
                    best = candidate;
                    best_error = error;
                }
            }
        }
        best
    } else if graphic_type == "ELLIPSE" && expected.len() == 4 && actual.len() == 4 {
        canonical_ellipse(actual)
    } else {
        actual.to_vec()
    }
}

fn canonical_ellipse(points: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let mut pairs = [
        sorted_pair(&points[0], &points[1]),
        sorted_pair(&points[2], &points[3]),
    ];
    pairs.sort_by(|left, right| {
        distance(&right[0][..2], &right[1][..2])
            .total_cmp(&distance(&left[0][..2], &left[1][..2]))
            .then_with(|| compare_points(&left[0], &right[0]))
            .then_with(|| compare_points(&left[1], &right[1]))
    });
    pairs.into_iter().flatten().collect()
}

fn sorted_pair(left: &[f64], right: &[f64]) -> [Vec<f64>; 2] {
    if compare_points(left, right) == Ordering::Greater {
        [right.to_vec(), left.to_vec()]
    } else {
        [left.to_vec(), right.to_vec()]
    }
}

fn compare_points(left: &[f64], right: &[f64]) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(left, right)| left.total_cmp(right))
        .find(|ordering| *ordering != Ordering::Equal)
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

fn squared_error(expected: &[Vec<f64>], actual: &[Vec<f64>]) -> f64 {
    expected
        .iter()
        .zip(actual)
        .flat_map(|(left, right)| left.iter().zip(right))
        .map(|(left, right)| (left - right).powi(2))
        .sum()
}

fn distance(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (left - right).powi(2))
        .sum::<f64>()
        .sqrt()
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

fn dimensions(geometry: &Map<String, Value>, key: &str) -> Result<usize, String> {
    geometry
        .get(key)
        .ok_or_else(|| format!("{key} is missing"))
        .and_then(json_dimensions)
}

fn json_dimensions(value: &Value) -> Result<usize, String> {
    match value.as_u64() {
        Some(2) => Ok(2),
        Some(3) => Ok(3),
        _ => Err("coordinate dimensions must be 2 or 3".to_owned()),
    }
}

fn stats(values: &[f64]) -> ErrorStats {
    if values.is_empty() {
        return ErrorStats {
            count: 0,
            max: 0.0,
            median: 0.0,
            rms: 0.0,
        };
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[middle]
    };
    ErrorStats {
        count: values.len(),
        max: sorted.last().copied().unwrap_or(0.0),
        median,
        rms: (values.iter().map(|value| value * value).sum::<f64>() / usize_as_f64(values.len()))
            .sqrt(),
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}
