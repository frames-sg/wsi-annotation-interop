use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryRun {
    pub segment_number: u16,
    pub row: usize,
    pub column_start: usize,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SegmentationMetrics {
    pub dice: f64,
    pub expected_area_pixels: usize,
    pub actual_area_pixels: usize,
    pub area_difference_pixels: usize,
    pub relative_area_difference: f64,
    pub area_difference_mm2: f64,
    pub centroid_distance_pixels: f64,
    pub centroid_distance_um: f64,
    pub hd95_pixels: f64,
    pub hd95_um: f64,
    pub assd_pixels: f64,
    pub assd_um: f64,
    pub expected_components: usize,
    pub actual_components: usize,
    pub expected_holes: usize,
    pub actual_holes: usize,
    pub expected_overlap_pixels: usize,
    pub actual_overlap_pixels: usize,
    pub overlap_difference_pixels: usize,
    pub overlap_difference_mm2: f64,
}

/// Calculate identity-aware binary segmentation metrics from normalized row runs.
///
/// # Errors
///
/// Returns an error when dimensions or spacing are invalid, arithmetic overflows,
/// or any run lies outside the declared mask.
pub fn segmentation_metrics(
    expected_runs: &[BinaryRun],
    actual_runs: &[BinaryRun],
    shape: (usize, usize),
    pixel_spacing_mm: (f64, f64),
) -> Result<SegmentationMetrics, String> {
    let (rows, columns) = shape;
    let (row_spacing, column_spacing) = pixel_spacing_mm;
    if rows == 0 || columns == 0 {
        return Err("mask shape must be positive".to_owned());
    }
    if !row_spacing.is_finite()
        || !column_spacing.is_finite()
        || row_spacing <= 0.0
        || column_spacing <= 0.0
    {
        return Err("pixel spacing must be positive and finite".to_owned());
    }
    let expected = segment_masks(expected_runs, shape)?;
    let actual = segment_masks(actual_runs, shape)?;
    let segment_numbers: BTreeSet<_> = expected.keys().chain(actual.keys()).copied().collect();

    let expected_area = area(&expected)?;
    let actual_area = area(&actual)?;
    let intersection = segment_numbers
        .iter()
        .map(|number| match (expected.get(number), actual.get(number)) {
            (Some(left), Some(right)) => left
                .iter()
                .zip(right)
                .filter(|(left, right)| **left && **right)
                .count(),
            _ => 0,
        })
        .sum::<usize>();
    let total_area = expected_area
        .checked_add(actual_area)
        .ok_or_else(|| "combined mask area overflows usize".to_owned())?;
    let dice = if total_area == 0 {
        1.0
    } else {
        2.0 * usize_as_f64(intersection) / usize_as_f64(total_area)
    };

    let (centroid_pixels, centroid_um) = centroid_distances(
        centroid(&expected, shape),
        centroid(&actual, shape),
        pixel_spacing_mm,
    );
    let pixel_distances = surface_distances(&expected, &actual, shape, (1.0, 1.0));
    let physical_distances = surface_distances(&expected, &actual, shape, pixel_spacing_mm);
    let (hd95_pixels, assd_pixels) = distance_summary(&pixel_distances);
    let (hd95_mm, assd_mm) = distance_summary(&physical_distances);
    let (expected_components, expected_holes) = topology(&expected, shape);
    let (actual_components, actual_holes) = topology(&actual, shape);
    let expected_overlap = overlap_pixels(&expected, shape);
    let actual_overlap = overlap_pixels(&actual, shape);
    let pixel_area_mm2 = row_spacing * column_spacing;
    let area_difference = expected_area.abs_diff(actual_area);
    let overlap_difference = expected_overlap.abs_diff(actual_overlap);

    Ok(SegmentationMetrics {
        dice,
        expected_area_pixels: expected_area,
        actual_area_pixels: actual_area,
        area_difference_pixels: area_difference,
        relative_area_difference: if expected_area == 0 && actual_area == 0 {
            0.0
        } else if expected_area == 0 {
            f64::INFINITY
        } else {
            usize_as_f64(area_difference) / usize_as_f64(expected_area)
        },
        area_difference_mm2: usize_as_f64(area_difference) * pixel_area_mm2,
        centroid_distance_pixels: centroid_pixels,
        centroid_distance_um: centroid_um,
        hd95_pixels,
        hd95_um: hd95_mm * 1000.0,
        assd_pixels,
        assd_um: assd_mm * 1000.0,
        expected_components,
        actual_components,
        expected_holes,
        actual_holes,
        expected_overlap_pixels: expected_overlap,
        actual_overlap_pixels: actual_overlap,
        overlap_difference_pixels: overlap_difference,
        overlap_difference_mm2: usize_as_f64(overlap_difference) * pixel_area_mm2,
    })
}

type Masks = BTreeMap<u16, Vec<bool>>;

fn segment_masks(runs: &[BinaryRun], shape: (usize, usize)) -> Result<Masks, String> {
    let (rows, columns) = shape;
    let pixels = rows
        .checked_mul(columns)
        .ok_or_else(|| "mask dimensions overflow addressable memory".to_owned())?;
    let mut masks = BTreeMap::new();
    for (index, run) in runs.iter().enumerate() {
        if run.segment_number == 0 || run.length == 0 {
            return Err(format!(
                "row run at index {index} has a non-positive identifier or length"
            ));
        }
        let Some(end) = run.column_start.checked_add(run.length) else {
            return Err(format!("row run at index {index} is outside mask bounds"));
        };
        if run.row >= rows || end > columns {
            return Err(format!("row run at index {index} is outside mask bounds"));
        }
        let mask = masks
            .entry(run.segment_number)
            .or_insert_with(|| vec![false; pixels]);
        let start = run.row * columns + run.column_start;
        mask[start..start + run.length].fill(true);
    }
    Ok(masks)
}

fn area(masks: &Masks) -> Result<usize, String> {
    masks.values().try_fold(0usize, |total, mask| {
        total
            .checked_add(mask.iter().filter(|value| **value).count())
            .ok_or_else(|| "mask area overflows usize".to_owned())
    })
}

fn centroid(masks: &Masks, shape: (usize, usize)) -> Option<(f64, f64)> {
    let (_, columns) = shape;
    let mut row_sum = 0.0;
    let mut column_sum = 0.0;
    let mut points = 0usize;
    for mask in masks.values() {
        for (index, selected) in mask.iter().enumerate() {
            if *selected {
                row_sum += usize_as_f64(index / columns);
                column_sum += usize_as_f64(index % columns);
                points += 1;
            }
        }
    }
    (points != 0).then(|| {
        (
            row_sum / usize_as_f64(points),
            column_sum / usize_as_f64(points),
        )
    })
}

fn centroid_distances(
    expected: Option<(f64, f64)>,
    actual: Option<(f64, f64)>,
    spacing: (f64, f64),
) -> (f64, f64) {
    match (expected, actual) {
        (None, None) => (0.0, 0.0),
        (Some(_), None) | (None, Some(_)) => (f64::INFINITY, f64::INFINITY),
        (Some(expected), Some(actual)) => {
            let row_delta = actual.0 - expected.0;
            let column_delta = actual.1 - expected.1;
            let pixels = row_delta.hypot(column_delta);
            let millimeters = (row_delta * spacing.0).hypot(column_delta * spacing.1);
            (pixels, millimeters * 1000.0)
        }
    }
}

fn surface_distances(
    expected: &Masks,
    actual: &Masks,
    shape: (usize, usize),
    sampling: (f64, f64),
) -> Vec<f64> {
    let segment_numbers: BTreeSet<_> = expected.keys().chain(actual.keys()).copied().collect();
    let mut directed = Vec::new();
    for number in segment_numbers {
        let left = expected.get(&number);
        let right = actual.get(&number);
        if left.is_none_or(|mask| !mask.iter().any(|value| *value))
            && right.is_none_or(|mask| !mask.iter().any(|value| *value))
        {
            continue;
        }
        let (Some(left), Some(right)) = (left, right) else {
            return vec![f64::INFINITY];
        };
        if !left.iter().any(|value| *value) || !right.iter().any(|value| *value) {
            return vec![f64::INFINITY];
        }
        let left_surface = surface(left, shape);
        let right_surface = surface(right, shape);
        let right_distances = distance_transform(&right_surface, shape, sampling);
        let left_distances = distance_transform(&left_surface, shape, sampling);
        directed.extend(
            left_surface
                .iter()
                .enumerate()
                .filter_map(|(index, selected)| selected.then_some(right_distances[index])),
        );
        directed.extend(
            right_surface
                .iter()
                .enumerate()
                .filter_map(|(index, selected)| selected.then_some(left_distances[index])),
        );
    }
    directed
}

fn surface(mask: &[bool], shape: (usize, usize)) -> Vec<bool> {
    let (rows, columns) = shape;
    let mut result = vec![false; mask.len()];
    for row in 0..rows {
        for column in 0..columns {
            let index = row * columns + column;
            if !mask[index] {
                continue;
            }
            result[index] = row == 0
                || row + 1 == rows
                || column == 0
                || column + 1 == columns
                || !mask[(row - 1) * columns + column]
                || !mask[(row + 1) * columns + column]
                || !mask[row * columns + column - 1]
                || !mask[row * columns + column + 1];
        }
    }
    result
}

fn distance_transform(features: &[bool], shape: (usize, usize), sampling: (f64, f64)) -> Vec<f64> {
    let (rows, columns) = shape;
    let mut horizontal = vec![f64::INFINITY; features.len()];
    for row in 0..rows {
        let input: Vec<_> = (0..columns)
            .map(|column| {
                if features[row * columns + column] {
                    0.0
                } else {
                    f64::INFINITY
                }
            })
            .collect();
        let transformed = squared_distance_transform_1d(&input, sampling.1);
        horizontal[row * columns..(row + 1) * columns].copy_from_slice(&transformed);
    }
    let mut result = vec![f64::INFINITY; features.len()];
    for column in 0..columns {
        let input: Vec<_> = (0..rows)
            .map(|row| horizontal[row * columns + column])
            .collect();
        let transformed = squared_distance_transform_1d(&input, sampling.0);
        for (row, distance) in transformed.into_iter().enumerate() {
            result[row * columns + column] = distance.sqrt();
        }
    }
    result
}

// Lower-envelope transform from Felzenszwalb and Huttenlocher, generalized for spacing.
fn squared_distance_transform_1d(input: &[f64], spacing: f64) -> Vec<f64> {
    let candidates: Vec<_> = input
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.is_finite().then_some(index))
        .collect();
    if candidates.is_empty() {
        return vec![f64::INFINITY; input.len()];
    }
    let scale = spacing * spacing;
    let mut sites = vec![0usize; candidates.len()];
    let mut boundaries = vec![0.0; candidates.len() + 1];
    let mut envelope = 0usize;
    sites[0] = candidates[0];
    boundaries[0] = f64::NEG_INFINITY;
    boundaries[1] = f64::INFINITY;
    for &site in &candidates[1..] {
        let mut boundary = intersection(input, scale, site, sites[envelope]);
        while boundary <= boundaries[envelope] {
            envelope -= 1;
            boundary = intersection(input, scale, site, sites[envelope]);
        }
        envelope += 1;
        sites[envelope] = site;
        boundaries[envelope] = boundary;
        boundaries[envelope + 1] = f64::INFINITY;
    }
    let mut result = vec![0.0; input.len()];
    envelope = 0;
    for (position, output) in result.iter_mut().enumerate() {
        while boundaries[envelope + 1] < usize_as_f64(position) {
            envelope += 1;
        }
        let delta = usize_as_f64(position.abs_diff(sites[envelope]));
        *output = scale * delta * delta + input[sites[envelope]];
    }
    result
}

fn intersection(input: &[f64], scale: f64, left: usize, right: usize) -> f64 {
    let left_position = usize_as_f64(left);
    let right_position = usize_as_f64(right);
    ((input[left] + scale * left_position * left_position)
        - (input[right] + scale * right_position * right_position))
        / (2.0 * scale * (left_position - right_position))
}

fn distance_summary(distances: &[f64]) -> (f64, f64) {
    if distances.is_empty() {
        return (0.0, 0.0);
    }
    if distances.iter().any(|value| value.is_infinite()) {
        return (f64::INFINITY, f64::INFINITY);
    }
    let mut sorted = distances.to_vec();
    sorted.sort_by(f64::total_cmp);
    let steps = sorted.len() - 1;
    let lower = (steps / 20) * 19 + ((steps % 20) * 19) / 20;
    let remainder = ((steps % 20) * 19) % 20;
    let upper = lower + usize::from(remainder != 0);
    let fraction = usize_as_f64(remainder) / 20.0;
    let percentile = sorted[lower] + (sorted[upper] - sorted[lower]) * fraction;
    let mean = sorted.iter().sum::<f64>() / usize_as_f64(sorted.len());
    (percentile, mean)
}

fn topology(masks: &Masks, shape: (usize, usize)) -> (usize, usize) {
    masks.values().fold((0, 0), |(components, holes), mask| {
        (
            components + component_count(mask, shape),
            holes + hole_count(mask, shape),
        )
    })
}

fn hole_count(mask: &[bool], shape: (usize, usize)) -> usize {
    let (rows, columns) = shape;
    let mut exterior = vec![false; mask.len()];
    let mut queue = VecDeque::new();
    for row in 0..rows {
        for column in [0, columns - 1] {
            enqueue_background(mask, &mut exterior, &mut queue, row, column, columns);
        }
    }
    for column in 0..columns {
        for row in [0, rows - 1] {
            enqueue_background(mask, &mut exterior, &mut queue, row, column, columns);
        }
    }
    while let Some((row, column)) = queue.pop_front() {
        for (next_row, next_column) in neighbors(row, column, shape, false) {
            enqueue_background(
                mask,
                &mut exterior,
                &mut queue,
                next_row,
                next_column,
                columns,
            );
        }
    }
    let holes: Vec<_> = mask
        .iter()
        .zip(exterior)
        .map(|(selected, exterior)| !selected && !exterior)
        .collect();
    component_count(&holes, shape)
}

fn enqueue_background(
    mask: &[bool],
    exterior: &mut [bool],
    queue: &mut VecDeque<(usize, usize)>,
    row: usize,
    column: usize,
    columns: usize,
) {
    let index = row * columns + column;
    if !mask[index] && !exterior[index] {
        exterior[index] = true;
        queue.push_back((row, column));
    }
}

fn component_count(mask: &[bool], shape: (usize, usize)) -> usize {
    let (_, columns) = shape;
    let mut visited = vec![false; mask.len()];
    let mut count = 0;
    for index in 0..mask.len() {
        if visited[index] || !mask[index] {
            continue;
        }
        count += 1;
        visited[index] = true;
        let mut queue = VecDeque::from([(index / columns, index % columns)]);
        while let Some((row, column)) = queue.pop_front() {
            for (next_row, next_column) in neighbors(row, column, shape, true) {
                let next = next_row * columns + next_column;
                if !visited[next] && mask[next] {
                    visited[next] = true;
                    queue.push_back((next_row, next_column));
                }
            }
        }
    }
    count
}

fn neighbors(
    row: usize,
    column: usize,
    shape: (usize, usize),
    diagonal: bool,
) -> impl Iterator<Item = (usize, usize)> {
    let (rows, columns) = shape;
    let row = isize::try_from(row).unwrap_or(isize::MAX);
    let column = isize::try_from(column).unwrap_or(isize::MAX);
    (-1..=1).flat_map(move |row_delta| {
        (-1..=1).filter_map(move |column_delta| {
            if (row_delta == 0 && column_delta == 0)
                || (!diagonal && row_delta != 0 && column_delta != 0)
            {
                return None;
            }
            let next_row = row.checked_add(row_delta)?;
            let next_column = column.checked_add(column_delta)?;
            let next_row = usize::try_from(next_row).ok()?;
            let next_column = usize::try_from(next_column).ok()?;
            (next_row < rows && next_column < columns).then_some((next_row, next_column))
        })
    })
}

fn overlap_pixels(masks: &Masks, shape: (usize, usize)) -> usize {
    let pixels = shape.0 * shape.1;
    let mut multiplicity = vec![0u16; pixels];
    for mask in masks.values() {
        for (count, selected) in multiplicity.iter_mut().zip(mask) {
            *count += u16::from(*selected);
        }
    }
    multiplicity.into_iter().filter(|count| *count > 1).count()
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    // Any value reaching this function already indexes an allocated Vec, far below f64's
    // exact-integer limit on supported targets. Metrics are represented as f64 by contract.
    value as f64
}

#[cfg(test)]
mod tests {
    use super::{distance_transform, squared_distance_transform_1d, usize_as_f64};

    #[test]
    fn distance_transform_handles_leading_infinity_and_spacing() {
        let result = squared_distance_transform_1d(&[f64::INFINITY, 0.0, f64::INFINITY], 0.5);
        for (actual, expected) in result.iter().zip([0.25, 0.0, 0.25]) {
            assert!((actual - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn distance_transform_matches_an_obvious_reference() {
        let shape = (4, 5);
        let mut features = vec![false; shape.0 * shape.1];
        features[1] = true;
        features[3 * shape.1 + 4] = true;
        let sampling = (0.5, 0.25);

        let actual = distance_transform(&features, shape, sampling);
        let feature_points = [(0_usize, 1_usize), (3, 4)];
        for row in 0..shape.0 {
            for column in 0..shape.1 {
                let expected = feature_points
                    .iter()
                    .map(|(feature_row, feature_column)| {
                        let row_delta = usize_as_f64(row.abs_diff(*feature_row));
                        let column_delta = usize_as_f64(column.abs_diff(*feature_column));
                        (row_delta * sampling.0).hypot(column_delta * sampling.1)
                    })
                    .fold(f64::INFINITY, f64::min);
                let observed = actual[row * shape.1 + column];
                assert!((observed - expected).abs() < 1e-12);
            }
        }
    }
}
