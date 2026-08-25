use std::collections::{BTreeSet, VecDeque};

use super::distance::{distance_summary, distance_transform};
use super::runs::{Interval, SparseMasks};

pub(crate) struct AdvancedMetrics {
    pub hd95_pixels: f64,
    pub hd95_mm: f64,
    pub assd_pixels: f64,
    pub assd_mm: f64,
    pub expected_components: usize,
    pub actual_components: usize,
    pub expected_holes: usize,
    pub actual_holes: usize,
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    row_start: usize,
    row_end: usize,
    column_start: usize,
    column_end: usize,
}

impl Bounds {
    fn from_runs(runs: impl Iterator<Item = Interval>) -> Option<Self> {
        runs.fold(None, |bounds, run| {
            Some(match bounds {
                None => Self {
                    row_start: run.row,
                    row_end: run.row + 1,
                    column_start: run.start,
                    column_end: run.end,
                },
                Some(bounds) => Self {
                    row_start: bounds.row_start.min(run.row),
                    row_end: bounds.row_end.max(run.row + 1),
                    column_start: bounds.column_start.min(run.start),
                    column_end: bounds.column_end.max(run.end),
                },
            })
        })
    }

    fn shape(self) -> (usize, usize) {
        (
            self.row_end - self.row_start,
            self.column_end - self.column_start,
        )
    }

    fn pixels(self) -> Option<usize> {
        let shape = self.shape();
        shape.0.checked_mul(shape.1)
    }
}

pub(crate) fn advanced_metrics(
    expected: &SparseMasks,
    actual: &SparseMasks,
    spacing: (f64, f64),
    limit: usize,
) -> Result<AdvancedMetrics, usize> {
    let required = required_crop_pixels(expected, actual).unwrap_or(usize::MAX);
    if required > limit {
        return Err(required);
    }
    let (expected_components, expected_holes) = topology(expected);
    let (actual_components, actual_holes) = topology(actual);
    let pixel_distances = surface_distances(expected, actual, (1.0, 1.0));
    let physical_distances = surface_distances(expected, actual, spacing);
    let (hd95_pixels, assd_pixels) = distance_summary(&pixel_distances);
    let (hd95_mm, assd_mm) = distance_summary(&physical_distances);
    Ok(AdvancedMetrics {
        hd95_pixels,
        hd95_mm,
        assd_pixels,
        assd_mm,
        expected_components,
        actual_components,
        expected_holes,
        actual_holes,
    })
}

fn required_crop_pixels(expected: &SparseMasks, actual: &SparseMasks) -> Option<usize> {
    let mut required = 0_usize;
    let segments: BTreeSet<_> = expected.keys().chain(actual.keys()).copied().collect();
    for segment in segments {
        let runs = expected
            .get(&segment)
            .into_iter()
            .flatten()
            .chain(actual.get(&segment).into_iter().flatten())
            .copied();
        required = required.max(Bounds::from_runs(runs)?.pixels()?);
    }
    Some(required)
}

fn topology(masks: &SparseMasks) -> (usize, usize) {
    masks.values().fold((0, 0), |(components, holes), runs| {
        let Some(bounds) = Bounds::from_runs(runs.iter().copied()) else {
            return (components, holes);
        };
        let mask = dense_mask(runs, bounds);
        let shape = bounds.shape();
        (
            components + component_count(&mask, shape),
            holes + hole_count(&mask, shape),
        )
    })
}

fn surface_distances(
    expected: &SparseMasks,
    actual: &SparseMasks,
    sampling: (f64, f64),
) -> Vec<f64> {
    let segments: BTreeSet<_> = expected.keys().chain(actual.keys()).copied().collect();
    let mut directed = Vec::new();
    for segment in segments {
        let (Some(left_runs), Some(right_runs)) = (expected.get(&segment), actual.get(&segment))
        else {
            return vec![f64::INFINITY];
        };
        let Some(bounds) = Bounds::from_runs(left_runs.iter().chain(right_runs).copied()) else {
            continue;
        };
        let shape = bounds.shape();
        let left = dense_mask(left_runs, bounds);
        let right = dense_mask(right_runs, bounds);
        let left_surface = surface(&left, shape);
        let right_surface = surface(&right, shape);
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

fn dense_mask(runs: &[Interval], bounds: Bounds) -> Vec<bool> {
    let shape = bounds.shape();
    let mut mask = vec![false; shape.0 * shape.1];
    for run in runs {
        let row = run.row - bounds.row_start;
        let start = row * shape.1 + run.start - bounds.column_start;
        mask[start..start + run.end - run.start].fill(true);
    }
    mask
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
    let mut count = 0_usize;
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
