use std::collections::BTreeMap;

use super::{BinaryRun, usize_as_f64};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Interval {
    pub row: usize,
    pub start: usize,
    pub end: usize,
}

pub(crate) type SparseMasks = BTreeMap<u16, Vec<Interval>>;

pub(crate) fn canonicalize(
    runs: &[BinaryRun],
    shape: (usize, usize),
) -> Result<SparseMasks, String> {
    let mut ordered = Vec::with_capacity(runs.len());
    for (index, run) in runs.iter().enumerate() {
        if run.segment_number == 0 || run.length == 0 {
            return Err(format!(
                "row run at index {index} has a non-positive identifier or length"
            ));
        }
        let Some(end) = run.column_start.checked_add(run.length) else {
            return Err(format!("row run at index {index} is outside mask bounds"));
        };
        if run.row >= shape.0 || end > shape.1 {
            return Err(format!("row run at index {index} is outside mask bounds"));
        }
        ordered.push((
            run.segment_number,
            Interval {
                row: run.row,
                start: run.column_start,
                end,
            },
        ));
    }
    ordered.sort_unstable_by_key(|(segment, interval)| {
        (*segment, interval.row, interval.start, interval.end)
    });
    let mut masks: SparseMasks = BTreeMap::new();
    for (segment, interval) in ordered {
        let entries = masks.entry(segment).or_default();
        if let Some(previous) = entries.last_mut()
            && previous.row == interval.row
            && interval.start <= previous.end
        {
            previous.end = previous.end.max(interval.end);
        } else {
            entries.push(interval);
        }
    }
    Ok(masks)
}

pub(crate) fn area(masks: &SparseMasks) -> Result<usize, String> {
    masks.values().flatten().try_fold(0_usize, |total, run| {
        total
            .checked_add(run.end - run.start)
            .ok_or_else(|| "mask area overflows usize".to_owned())
    })
}

pub(crate) fn intersection_area(left: &SparseMasks, right: &SparseMasks) -> Result<usize, String> {
    let mut total = 0_usize;
    for (segment, left_runs) in left {
        let Some(right_runs) = right.get(segment) else {
            continue;
        };
        let (mut left_index, mut right_index) = (0, 0);
        while left_index < left_runs.len() && right_index < right_runs.len() {
            let left_run = left_runs[left_index];
            let right_run = right_runs[right_index];
            if left_run.row < right_run.row
                || (left_run.row == right_run.row && left_run.end <= right_run.start)
            {
                left_index += 1;
                continue;
            }
            if right_run.row < left_run.row
                || (left_run.row == right_run.row && right_run.end <= left_run.start)
            {
                right_index += 1;
                continue;
            }
            let overlap = left_run.end.min(right_run.end) - left_run.start.max(right_run.start);
            total = total
                .checked_add(overlap)
                .ok_or_else(|| "mask intersection overflows usize".to_owned())?;
            if left_run.end <= right_run.end {
                left_index += 1;
            }
            if right_run.end <= left_run.end {
                right_index += 1;
            }
        }
    }
    Ok(total)
}

pub(crate) fn centroid(masks: &SparseMasks) -> Option<(f64, f64)> {
    let mut row_sum = 0.0;
    let mut column_sum = 0.0;
    let mut points = 0_usize;
    for run in masks.values().flatten() {
        let length = run.end - run.start;
        row_sum += usize_as_f64(run.row) * usize_as_f64(length);
        let first = usize_as_f64(run.start);
        let last = usize_as_f64(run.end - 1);
        column_sum += (first + last) * usize_as_f64(length) / 2.0;
        points = points.checked_add(length)?;
    }
    (points != 0).then(|| {
        (
            row_sum / usize_as_f64(points),
            column_sum / usize_as_f64(points),
        )
    })
}

pub(crate) fn overlap_pixels(masks: &SparseMasks) -> Result<usize, String> {
    let mut rows: BTreeMap<usize, Vec<(usize, i32)>> = BTreeMap::new();
    for run in masks.values().flatten() {
        let events = rows.entry(run.row).or_default();
        events.push((run.start, 1));
        events.push((run.end, -1));
    }
    let mut overlap = 0_usize;
    for events in rows.values_mut() {
        events.sort_unstable_by_key(|event| event.0);
        let mut active = 0_i32;
        let mut previous = events[0].0;
        let mut index = 0;
        while index < events.len() {
            let position = events[index].0;
            if active > 1 {
                overlap = overlap
                    .checked_add(position - previous)
                    .ok_or_else(|| "overlap area overflows usize".to_owned())?;
            }
            while index < events.len() && events[index].0 == position {
                active += events[index].1;
                index += 1;
            }
            previous = position;
        }
    }
    Ok(overlap)
}
