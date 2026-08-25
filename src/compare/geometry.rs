use std::cmp::Ordering;

use serde_json::{Map, Value};

pub(super) struct CoordinateErrors {
    pub(super) canonical: Vec<f64>,
    pub(super) z: Vec<f64>,
    pub(super) native: Vec<f64>,
}

pub(super) fn errors(
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
    graphic_type: &str,
) -> Result<CoordinateErrors, String> {
    let canonical = primitive_errors(expected, actual, graphic_type, CoordinateSpec::CANONICAL)?;
    let native = primitive_errors(expected, actual, graphic_type, CoordinateSpec::NATIVE)?;
    Ok(CoordinateErrors {
        canonical: canonical.distances,
        z: canonical.z,
        native: native.distances,
    })
}

#[derive(Clone, Copy)]
struct CoordinateSpec {
    coordinate_key: &'static str,
    dimensions_key: &'static str,
    index_dimensions_key: Option<&'static str>,
    label: &'static str,
    xy_with_separate_z: bool,
    optional: bool,
}

impl CoordinateSpec {
    const CANONICAL: Self = Self {
        coordinate_key: "canonical_level0_coordinates",
        dimensions_key: "canonical_dimensions",
        index_dimensions_key: Some("native_dimensions"),
        label: "canonical",
        xy_with_separate_z: true,
        optional: false,
    };

    const NATIVE: Self = Self {
        coordinate_key: "native_coordinates",
        dimensions_key: "native_dimensions",
        index_dimensions_key: None,
        label: "native",
        xy_with_separate_z: false,
        optional: true,
    };
}

struct PrimitiveErrors {
    distances: Vec<f64>,
    z: Vec<f64>,
}

fn primitive_errors(
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
    graphic_type: &str,
    spec: CoordinateSpec,
) -> Result<PrimitiveErrors, String> {
    if spec.optional
        && !expected.contains_key(spec.coordinate_key)
        && !actual.contains_key(spec.coordinate_key)
    {
        return Ok(PrimitiveErrors {
            distances: Vec::new(),
            z: Vec::new(),
        });
    }
    let coordinate_dimensions = dimensions(expected, spec.dimensions_key)?;
    if dimensions(actual, spec.dimensions_key)? != coordinate_dimensions {
        return Err(format!("{} dimensions differ or are invalid", spec.label));
    }
    if expected.get("primitive_point_indices") != actual.get("primitive_point_indices") {
        return Err("primitive point associations differ".to_owned());
    }
    let index_dimensions = match spec.index_dimensions_key {
        Some(key) => expected
            .get(key)
            .map_or(Ok(coordinate_dimensions), json_dimensions)?,
        None => coordinate_dimensions,
    };
    let actual_index_dimensions = match spec.index_dimensions_key {
        Some(key) => actual
            .get(key)
            .map_or(Ok(coordinate_dimensions), json_dimensions)?,
        None => coordinate_dimensions,
    };
    if index_dimensions != actual_index_dimensions {
        return Err("native dimensions differ or are invalid".to_owned());
    }
    let expected_primitives = primitives(
        expected,
        coordinate_dimensions,
        index_dimensions,
        spec.coordinate_key,
    )?;
    let actual_primitives = primitives(
        actual,
        coordinate_dimensions,
        actual_index_dimensions,
        spec.coordinate_key,
    )?;
    if expected_primitives.len() != actual_primitives.len() {
        return Err(format!("{} primitive counts differ", spec.label));
    }
    compare_primitives(
        expected_primitives,
        actual_primitives,
        coordinate_dimensions,
        graphic_type,
        spec,
    )
}

fn compare_primitives(
    expected: Vec<Vec<Vec<f64>>>,
    actual: Vec<Vec<Vec<f64>>>,
    dimensions: usize,
    graphic_type: &str,
    spec: CoordinateSpec,
) -> Result<PrimitiveErrors, String> {
    let mut distances = Vec::new();
    let mut z = Vec::new();
    for (mut expected_points, actual_points) in expected.into_iter().zip(actual) {
        if expected_points.len() != actual_points.len() {
            return Err(format!("{} primitive point counts differ", spec.label));
        }
        if graphic_type == "ELLIPSE" && expected_points.len() == 4 {
            expected_points = canonical_ellipse(&expected_points);
        }
        let actual_points = align_points(&expected_points, &actual_points, graphic_type);
        for (left, right) in expected_points.iter().zip(&actual_points) {
            if spec.xy_with_separate_z {
                distances.push(distance(&left[..2], &right[..2]));
                if dimensions == 3 {
                    z.push((left[2] - right[2]).abs());
                }
            } else {
                distances.push(distance(left, right));
            }
        }
    }
    Ok(PrimitiveErrors { distances, z })
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
    let scalar_offsets = match raw_starts {
        None if points.is_empty() => Vec::new(),
        Some(raw_starts) if raw_starts.is_empty() && points.is_empty() => Vec::new(),
        None => vec![0],
        Some(raw_starts) if raw_starts.is_empty() => vec![0],
        Some(raw_starts) => raw_starts
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| value.checked_sub(1))
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| "primitive point indices must be positive integers".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
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
